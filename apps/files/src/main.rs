//! Files: browse, open and organise files.
//!
//!     cargo run -p neo-files [folder]
//!     cargo run -p neo-files -- --snapshot target/snapshots
//!
//! Double-click or press Enter to open. Backspace goes back, Delete moves
//! the selection to the Trash, and typing in the search field filters the
//! current folder.

// Release builds on Windows open no console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod folders;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use neo::prelude::*;
use neo::{Image, Key, KeyEvent, Modifiers, Point, Proxy, Size, WindowGeometry};
use neo_desktop::fs::{file_icon, friendly_time, home_dir, human_size, move_to_trash, natural_cmp, open_file, roots, user_dir};
use neo_desktop::ui::{nav_item, notice, section, split};
use neo_desktop::Desktop;

/// Rows beyond this are not built; searching narrows the list instead.
const MAX_ROWS: usize = 800;
const DOUBLE_CLICK: Duration = Duration::from_millis(450);

#[derive(Clone, Debug)]
struct Entry {
    name: String,
    path: PathBuf,
    dir: bool,
    link: bool,
    size: u64,
    modified: Option<SystemTime>,
    /// When the file was created. Some Linux file systems do not record it.
    created: Option<SystemTime>,
}

impl Entry {
    fn hidden(&self) -> bool {
        self.name.starts_with('.')
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SortBy {
    Name,
    Size,
    Modified,
    Created,
}

/// An inline name field for a new folder or a rename.
#[derive(Clone, Debug)]
enum Naming {
    NewFolder(String),
    Rename { from: PathBuf, name: String },
}

struct Files {
    desktop: Desktop,
    dir: PathBuf,
    entries: Vec<Entry>,
    stamp: Option<SystemTime>,
    back: Vec<PathBuf>,
    forward: Vec<PathBuf>,
    /// The selected entries, in the order they were chosen.
    selected: Vec<PathBuf>,
    /// Where a Shift-click's range starts: the last entry clicked on its own.
    anchor: Option<PathBuf>,
    last_click: Option<(PathBuf, Instant)>,
    query: String,
    show_hidden: bool,
    sort: SortBy,
    descending: bool,
    naming: Option<Naming>,
    /// The open context menu: what was right-clicked (nothing, for the
    /// folder's own background) and where.
    menu: Option<(Option<PathBuf>, Point)>,
    status: Option<(Tone, String)>,
    proxy: Option<Proxy<Msg>>,
    view: ViewMode,
    /// The window's width, to work out how many tiles fit in a row.
    width: f32,
    thumbs: std::collections::HashMap<PathBuf, Thumb>,
    /// Pictures waiting for the thumbnail worker, tagged with the folder
    /// visit they belong to.
    thumb_queue: Option<std::sync::mpsc::Sender<(u64, PathBuf)>>,
    /// Counts folder visits, so the worker can skip work for a folder that
    /// is no longer shown.
    visit: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// How much each folder seen holds, kept while moving around so going
    /// back shows them at once, and measured again on each visit.
    sizes: std::collections::HashMap<PathBuf, FolderSize>,
    /// Folders waiting for the measuring workers, tagged like thumbnails.
    size_queue: Option<std::sync::mpsc::Sender<(u64, PathBuf)>>,
    /// The folders bookmarked in the sidebar, in order.
    bookmarks: Vec<PathBuf>,
    /// Where they are kept. Tests keep them nowhere unless they say where.
    bookmarks_file: Option<PathBuf>,
    /// The menu open on a bookmark, and where.
    bookmark_menu: Option<(PathBuf, Point)>,
}

/// What is known of how much a folder holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FolderSize {
    /// Being added up, with nothing known yet.
    Measuring,
    Known(u64),
    /// It could not be read.
    Unknown,
}

#[derive(Clone, Debug)]
enum Msg {
    Go(PathBuf),
    Back,
    Forward,
    Up,
    Click(PathBuf, Modifiers),
    SelectAll,
    Open,
    Query(String),
    Hidden,
    Sort(SortBy),
    NewFolder,
    Rename,
    NameInput(String),
    NameSubmit,
    NameCancel,
    Trash,
    Select(isize),
    /// A right click on an entry, or on the empty part of the folder.
    Context(Option<PathBuf>, Point),
    CloseMenu,
    Choose(Choice),
    /// Files were let go over a folder: this one, a subfolder or a place.
    Drop(PathBuf, Vec<PathBuf>),
    /// A drop finished copying or moving.
    Dropped(Result<neo_desktop::fs::Transferred, String>),
    View(ViewMode),
    Geometry(WindowGeometry),
    /// The worker finished a thumbnail, or found the file is not a picture.
    Thumb(PathBuf, Option<Image>),
    /// A worker finished adding up what a folder holds.
    Measured(PathBuf, Option<u64>),
    /// Take a folder out of the sidebar.
    /// Things were dropped on the Bookmarks heading: the folders among them
    /// are bookmarked, not copied.
    BookmarkDrop(Vec<PathBuf>),
    RemoveBookmark(PathBuf),
    /// A right click on a bookmark.
    BookmarkMenu(PathBuf, Point),
    CloseBookmarkMenu,
    Poll,
    /// The Settings entry and panel every Neo app has.
    Desktop(neo_desktop::DesktopMsg),
}

/// How the folder's contents are laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ViewMode {
    /// One entry a row, with size and dates.
    List,
    /// The same, with tighter rows to fit more in.
    Compact,
    /// A grid of tiles, with pictures shown as thumbnails.
    Grid,
}

/// A picture's thumbnail for the grid.
#[derive(Clone, Debug, PartialEq)]
enum Thumb {
    /// Asked for; the worker has not answered yet.
    Loading,
    Ready(Image),
    /// Not a picture that can be read; the tile shows the file's icon.
    Failed,
}

/// Width of a grid tile and the space between tiles.
const TILE_W: f32 = 132.0;
const TILE_GAP: f32 = 6.0;
/// The longest side of a thumbnail, in pixels. Twice the tile's picture
/// box, so it stays sharp on high-density screens.
const THUMB_SIDE: u32 = 224;

/// Reads a picture and shrinks it to thumbnail size, turned the way the
/// camera recorded it should be shown.
fn thumbnail(path: &Path) -> Option<Image> {
    use image::{DynamicImage, ImageDecoder, ImageReader};
    let mut decoder = ImageReader::open(path).ok()?.with_guessed_format().ok()?.into_decoder().ok()?;
    let orientation = decoder.orientation().ok()?;
    let mut picture = DynamicImage::from_decoder(decoder).ok()?;
    picture.apply_orientation(orientation);
    // Shrink large pictures; small ones are shown as they are.
    let small = if picture.width().max(picture.height()) > THUMB_SIDE { picture.thumbnail(THUMB_SIDE, THUMB_SIDE) } else { picture }.into_rgba8();
    let (w, h) = small.dimensions();
    Some(Image::frame(w, h, small.into_raw()))
}

/// A row of the context menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Choice {
    Open,
    /// Open a terminal in the folder that was clicked, or in this folder.
    Terminal,
    Rename,
    Trash,
    NewFolder,
    Hidden,
    /// Put the folder clicked, or this one, in the sidebar, or take it out.
    Bookmark,
}

fn read_dir(dir: &Path) -> std::io::Result<Vec<Entry>> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir)?.flatten() {
        let path = e.path();
        let link = e.file_type().is_ok_and(|t| t.is_symlink());
        // Follow links for size and kind, but keep broken ones.
        let meta = std::fs::metadata(&path).or_else(|_| e.metadata());
        let (dir, size, modified, created) = match &meta {
            Ok(m) => (m.is_dir(), if m.is_dir() { 0 } else { m.len() }, m.modified().ok(), m.created().ok()),
            Err(_) => (false, 0, None, None),
        };
        out.push(Entry { name: e.file_name().to_string_lossy().into_owned(), path, dir, link, size, modified, created });
    }
    Ok(out)
}

fn dir_stamp(dir: &Path) -> Option<SystemTime> {
    std::fs::metadata(dir).and_then(|m| m.modified()).ok()
}

impl Files {
    fn new(dir: PathBuf) -> Self {
        let mut f = Self {
            desktop: Desktop::load(),
            dir: PathBuf::new(),
            entries: vec![],
            stamp: None,
            back: vec![],
            forward: vec![],
            selected: vec![],
            anchor: None,
            last_click: None,
            query: String::new(),
            show_hidden: false,
            sort: SortBy::Name,
            descending: false,
            naming: None,
            menu: None,
            status: None,
            proxy: None,
            view: ViewMode::List,
            width: 1040.0,
            thumbs: Default::default(),
            thumb_queue: None,
            visit: Default::default(),
            sizes: Default::default(),
            size_queue: None,
            bookmarks_file: (!cfg!(test)).then(folders::bookmarks_file),
            bookmarks: vec![],
            bookmark_menu: None,
        };
        if let Some(file) = &f.bookmarks_file {
            f.bookmarks = folders::load_bookmarks(file);
        }
        f.load(dir);
        f
    }

    /// Shows `dir` without touching history.
    fn load(&mut self, dir: PathBuf) {
        match read_dir(&dir) {
            Ok(entries) => {
                self.stamp = dir_stamp(&dir);
                self.entries = entries;
                if self.dir != dir {
                    // Thumbnails belong to the folder; drop them and any still queued.
                    self.thumbs.clear();
                    self.visit.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    self.selected.clear();
                    self.anchor = None;
                    self.query.clear();
                    self.naming = None;
                }
                self.dir = dir;
                self.status = None;
                self.request_thumbs();
                self.request_sizes();
            }
            Err(e) => self.status = Some((Tone::Bad, format!("Could not open {}: {e}", dir.display()))),
        }
    }

    fn navigate(&mut self, dir: PathBuf) {
        if dir == self.dir {
            return;
        }
        let from = self.dir.clone();
        self.load(dir.clone());
        if self.dir == dir {
            self.back.push(from);
            self.forward.clear();
        }
    }

    /// Asks the worker for thumbnails of the pictures shown, in grid view.
    fn request_thumbs(&mut self) {
        if self.view != ViewMode::Grid {
            return;
        }
        let Some(queue) = self.thumb_queue.clone() else { return };
        let visit = self.visit.load(std::sync::atomic::Ordering::Relaxed);
        let wanted: Vec<PathBuf> = self.visible().iter().take(MAX_ROWS).filter(|e| !e.dir && neo_desktop::fs::has_extension(&e.path, neo_desktop::fs::IMAGE_EXTENSIONS)).map(|e| e.path.clone()).filter(|p| !self.thumbs.contains_key(p)).collect();
        for path in wanted {
            self.thumbs.insert(path.clone(), Thumb::Loading);
            let _ = queue.send((visit, path));
        }
    }

    /// Asks for each folder shown to be measured. What was known from an
    /// earlier visit stays showing until the new count is in.
    fn request_sizes(&mut self) {
        let visit = self.visit.load(std::sync::atomic::Ordering::Relaxed);
        let dirs: Vec<PathBuf> = self.entries.iter().filter(|e| e.dir && !e.link).map(|e| e.path.clone()).collect();
        for path in dirs {
            self.sizes.entry(path.clone()).or_insert(FolderSize::Measuring);
            match &self.size_queue {
                Some(queue) => {
                    let _ = queue.send((visit, path));
                }
                // No workers, as in tests: measure now.
                None => {
                    let size = folders::measure(&path, &|| true);
                    self.sizes.insert(path, size.map_or(FolderSize::Unknown, FolderSize::Known));
                }
            }
        }
    }

    /// How much an entry holds: a file's length, or a folder's contents
    /// once they have been added up.
    fn size_of(&self, e: &Entry) -> Option<u64> {
        if !e.dir {
            return Some(e.size);
        }
        match self.sizes.get(&e.path) {
            Some(FolderSize::Known(n)) => Some(*n),
            _ => None,
        }
    }

    /// Puts a folder in the sidebar, or takes it out, and writes it down.
    fn set_bookmark(&mut self, path: PathBuf, on: bool) {
        let had = self.bookmarks.contains(&path);
        if on && !had && path.is_dir() {
            self.bookmarks.push(path);
        } else if !on && had {
            self.bookmarks.retain(|b| *b != path);
        } else {
            return;
        }
        if let Some(file) = &self.bookmarks_file
            && let Err(e) = folders::save_bookmarks(file, &self.bookmarks)
        {
            self.status = Some((Tone::Bad, format!("Could not save the sidebar: {e}")));
        }
    }

    /// Selects one entry and nothing else, and makes it the range anchor.
    fn select_only(&mut self, path: PathBuf) {
        self.anchor = Some(path.clone());
        self.selected = vec![path];
    }

    fn reload(&mut self) {
        let dir = self.dir.clone();
        self.load(dir);
    }

    /// Entries after the hidden filter, search and sort.
    fn visible(&self) -> Vec<&Entry> {
        let q = self.query.to_lowercase();
        let mut v: Vec<&Entry> = self.entries.iter().filter(|e| self.show_hidden || !e.hidden()).filter(|e| q.is_empty() || e.name.to_lowercase().contains(&q)).collect();
        v.sort_by(|a, b| {
            let dirs_first = b.dir.cmp(&a.dir);
            let by = match self.sort {
                SortBy::Name => natural_cmp(&a.name, &b.name),
                SortBy::Size => self.size_of(a).cmp(&self.size_of(b)).then_with(|| natural_cmp(&a.name, &b.name)),
                SortBy::Modified => a.modified.cmp(&b.modified),
                SortBy::Created => a.created.cmp(&b.created),
            };
            dirs_first.then(if self.descending { by.reverse() } else { by })
        });
        v
    }

    fn open_entry(&mut self, path: PathBuf) {
        let Some(e) = self.entries.iter().find(|e| e.path == path) else { return };
        if e.dir {
            self.navigate(path);
        } else if let Err(err) = open_file(&path) {
            self.status = Some((Tone::Bad, format!("Could not open {}: {err}", e.name)));
        }
    }

    fn finish_naming(&mut self) {
        let Some(naming) = self.naming.take() else { return };
        let result = match &naming {
            Naming::NewFolder(name) => valid_name(name).and_then(|n| std::fs::create_dir(self.dir.join(n)).map(|_| self.dir.join(n))),
            Naming::Rename { from, name } => valid_name(name).and_then(|n| {
                let to = self.dir.join(n);
                if to.symlink_metadata().is_ok() && &to != from {
                    return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, format!("{n} already exists")));
                }
                std::fs::rename(from, &to).map(|_| to)
            }),
        };
        match result {
            Ok(path) => {
                self.reload();
                self.select_only(path);
            }
            Err(e) => {
                self.status = Some((Tone::Bad, e.to_string()));
                self.naming = Some(naming);
            }
        }
    }
}

fn valid_name(name: &str) -> std::io::Result<&str> {
    let n = name.trim();
    if n.is_empty() || n == "." || n == ".." || n.contains('/') || n.contains('\0') {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Names can't be empty or contain a slash."));
    }
    Ok(n)
}

impl App for Files {
    type Message = Msg;

    fn title(&self) -> String {
        let name = self.dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| self.dir.display().to_string());
        format!("{name} — Files")
    }

    fn window(&self) -> WindowSettings {
        WindowSettings { size: Size::new(1040.0, 680.0), min_size: Some(Size::new(640.0, 400.0)), app_id: Some("org.neo.Files".into()), ..Default::default() }
    }

    fn app_menu(&self) -> Vec<MenuEntry<Msg>> {
        self.desktop.app_menu(Msg::Desktop)
    }

    fn theme(&self, system: Scheme) -> Theme {
        self.desktop.theme(system)
    }

    fn start(&mut self, proxy: Proxy<Msg>) {
        self.proxy = Some(proxy.clone());
        // One worker makes thumbnails, so a folder of photos does not swamp the machine.
        let (tx, rx) = std::sync::mpsc::channel::<(u64, PathBuf)>();
        let visit = self.visit.clone();
        let thumbs = proxy.clone();
        std::thread::spawn(move || {
            let proxy = thumbs;
            for (wanted_in, path) in rx {
                // The folder has changed since this was asked for.
                if visit.load(std::sync::atomic::Ordering::Relaxed) != wanted_in {
                    continue;
                }
                let image = thumbnail(&path);
                if !proxy.send(Msg::Thumb(path, image)) {
                    return;
                }
            }
        });
        self.thumb_queue = Some(tx);
        self.request_thumbs();
        // A few workers add up folders, each taking the next one waiting, so
        // one huge folder does not hold up the rest.
        let (tx, rx) = std::sync::mpsc::channel::<(u64, PathBuf)>();
        let rx = std::sync::Arc::new(std::sync::Mutex::new(rx));
        for _ in 0..3 {
            let (rx, visit, proxy) = (rx.clone(), self.visit.clone(), proxy.clone());
            std::thread::spawn(move || {
                loop {
                    let Ok((wanted_in, path)) = rx.lock().unwrap_or_else(|e| e.into_inner()).recv() else { return };
                    // Given up as soon as the folder shown changes.
                    let current = || visit.load(std::sync::atomic::Ordering::Relaxed) == wanted_in;
                    if !current() {
                        continue;
                    }
                    let size = folders::measure(&path, &current);
                    // Nothing, if it stopped because the folder changed;
                    // otherwise the size, or that it could not be read.
                    if (size.is_some() || current()) && !proxy.send(Msg::Measured(path, size)) {
                        return;
                    }
                }
            });
        }
        self.size_queue = Some(tx);
        self.request_sizes();
    }

    fn on_window_geometry(&self, geometry: WindowGeometry) -> Option<Msg> {
        Some(Msg::Geometry(geometry))
    }

    fn subscriptions(&self) -> Vec<Subscription<Msg>> {
        vec![Desktop::subscription(Msg::Poll)]
    }

    fn on_key(&self, k: &KeyEvent) -> Option<Msg> {
        let cmd = k.modifiers.command();
        match &k.key {
            Key::Up if k.modifiers.alt => Some(Msg::Up),
            Key::Up => Some(Msg::Select(-1)),
            Key::Down => Some(Msg::Select(1)),
            Key::Enter => Some(Msg::Open),
            Key::Backspace if !cmd => Some(Msg::Back),
            Key::Backspace | Key::Delete => Some(Msg::Trash),
            Key::Left if k.modifiers.alt => Some(Msg::Back),
            Key::Right if k.modifiers.alt => Some(Msg::Forward),
            Key::Character(c) if cmd && c == "a" => Some(Msg::SelectAll),
            Key::Character(c) if cmd && c == "h" => Some(Msg::Hidden),
            Key::Character(c) if cmd && c == "n" && k.modifiers.shift => Some(Msg::NewFolder),
            _ => None,
        }
    }

    fn update(&mut self, m: Msg) {
        match m {
            Msg::Go(p) => self.navigate(p),
            Msg::Back => {
                if let Some(p) = self.back.pop() {
                    let from = self.dir.clone();
                    self.load(p);
                    self.forward.push(from);
                }
            }
            Msg::Forward => {
                if let Some(p) = self.forward.pop() {
                    let from = self.dir.clone();
                    self.load(p);
                    self.back.push(from);
                }
            }
            Msg::Up => {
                if let Some(parent) = self.dir.parent() {
                    let child = self.dir.clone();
                    self.navigate(parent.to_path_buf());
                    self.select_only(child);
                }
            }
            Msg::Click(p, modifiers) => {
                if modifiers.shift {
                    // Everything from the anchor to here, in the order shown.
                    let order: Vec<PathBuf> = self.visible().iter().map(|e| e.path.clone()).collect();
                    let anchor = self.anchor.clone().filter(|a| order.contains(a)).unwrap_or_else(|| p.clone());
                    if let (Some(a), Some(b)) = (order.iter().position(|x| *x == anchor), order.iter().position(|x| *x == p)) {
                        let range = order[a.min(b)..=a.max(b)].to_vec();
                        // With Command or Ctrl too, the range adds to what is selected.
                        if !modifiers.command() {
                            self.selected.clear();
                        }
                        for path in range {
                            if !self.selected.contains(&path) {
                                self.selected.push(path);
                            }
                        }
                        self.anchor = Some(anchor);
                    }
                    self.last_click = None;
                } else if modifiers.command() {
                    // Add or remove this one entry.
                    match self.selected.iter().position(|x| *x == p) {
                        Some(i) => {
                            self.selected.remove(i);
                        }
                        None => self.selected.push(p.clone()),
                    }
                    self.anchor = Some(p);
                    self.last_click = None;
                } else {
                    let now = Instant::now();
                    let double = self.last_click.as_ref().is_some_and(|(q, t)| *q == p && now.duration_since(*t) < DOUBLE_CLICK);
                    self.select_only(p.clone());
                    if double {
                        self.last_click = None;
                        self.open_entry(p);
                    } else {
                        self.last_click = Some((p, now));
                    }
                }
            }
            Msg::SelectAll => {
                self.selected = self.visible().iter().map(|e| e.path.clone()).collect();
                self.anchor = self.selected.first().cloned();
            }
            Msg::Open => {
                // One entry opens as it is. Of several, only the files open:
                // a folder among them would take the view away.
                let several = self.selected.len() > 1;
                let targets: Vec<PathBuf> = self.selected.iter().filter(|p| !several || !p.is_dir()).cloned().collect();
                for p in targets {
                    self.open_entry(p);
                }
            }
            Msg::Query(q) => {
                self.query = q;
                self.request_thumbs();
            }
            Msg::Hidden => {
                self.show_hidden = !self.show_hidden;
                self.request_thumbs();
            }
            Msg::Sort(s) => {
                if self.sort == s {
                    self.descending = !self.descending;
                } else {
                    self.sort = s;
                    self.descending = s != SortBy::Name;
                }
            }
            Msg::NewFolder => self.naming = Some(Naming::NewFolder("New Folder".into())),
            Msg::Rename => {
                // One name at a time.
                if let [p] = self.selected.as_slice() {
                    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    self.naming = Some(Naming::Rename { from: p.clone(), name });
                }
            }
            Msg::NameInput(s) => match &mut self.naming {
                Some(Naming::NewFolder(n)) | Some(Naming::Rename { name: n, .. }) => *n = s,
                None => {}
            },
            Msg::NameSubmit => self.finish_naming(),
            Msg::NameCancel => self.naming = None,
            Msg::Trash => {
                let targets = std::mem::take(&mut self.selected);
                if targets.is_empty() {
                    return;
                }
                let name = |p: &PathBuf| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let mut moved = Vec::new();
                let mut failed: Option<(PathBuf, std::io::Error)> = None;
                for p in &targets {
                    match move_to_trash(p) {
                        Ok(_) => moved.push(p.clone()),
                        Err(e) => {
                            // Stop at the first problem and keep the rest selected.
                            failed = Some((p.clone(), e));
                            break;
                        }
                    }
                }
                self.reload();
                self.selected = targets.into_iter().filter(|p| !moved.contains(p)).collect();
                self.anchor = None;
                self.status = Some(match (&failed, moved.as_slice()) {
                    (Some((p, e)), _) => (Tone::Bad, format!("Could not move {} to the Trash: {e}", name(p))),
                    (None, [one]) => (Tone::Good, format!("Moved {} to the Trash.", name(one))),
                    (None, many) => (Tone::Good, format!("Moved {} items to the Trash.", many.len())),
                });
            }
            Msg::Select(d) => {
                let v = self.visible();
                if v.is_empty() {
                    return;
                }
                // The arrows move from the entry chosen last.
                let cur = self.selected.last().and_then(|s| v.iter().position(|e| &e.path == s));
                let next = match cur {
                    None if d > 0 => 0,
                    None => v.len() - 1,
                    Some(i) => (i as isize + d).clamp(0, v.len() as isize - 1) as usize,
                };
                let path = v[next].path.clone();
                self.select_only(path);
            }
            Msg::Drop(target, sources) => {
                // What came from this folder is moved; anything else is copied.
                let here = self.dir.clone();
                let work = move || neo_desktop::fs::transfer(&sources, &target, Some(&here)).map_err(|e| e.to_string());
                match self.proxy.clone() {
                    // Copying can take a while; keep the window responsive.
                    Some(proxy) => {
                        self.status = Some((Tone::Muted, "Working…".into()));
                        std::thread::spawn(move || proxy.send(Msg::Dropped(work())));
                    }
                    None => self.update(Msg::Dropped(work())),
                }
            }
            Msg::Dropped(result) => {
                self.reload();
                let count = |n: usize| if n == 1 { "1 item".to_string() } else { format!("{n} items") };
                self.status = match result {
                    Ok(done) if done.moved + done.copied == 0 => None,
                    Ok(done) if done.copied == 0 => Some((Tone::Good, format!("Moved {}.", count(done.moved)))),
                    Ok(done) if done.moved == 0 => Some((Tone::Good, format!("Copied {}.", count(done.copied)))),
                    Ok(done) => Some((Tone::Good, format!("Moved {} and copied {}.", count(done.moved), count(done.copied)))),
                    Err(e) => Some((Tone::Bad, format!("That didn't finish: {e}"))),
                };
                self.selected.retain(|p| p.symlink_metadata().is_ok());
            }
            Msg::View(mode) => {
                self.view = mode;
                self.request_thumbs();
            }
            Msg::Geometry(g) => self.width = g.frame.w,
            Msg::Thumb(path, image) => {
                // Only for a picture still wanted: the folder may have changed.
                if let Some(slot) = self.thumbs.get_mut(&path) {
                    *slot = image.map_or(Thumb::Failed, Thumb::Ready);
                }
            }
            Msg::Measured(path, size) => {
                // A folder that could not be read keeps any size known before.
                match size {
                    Some(n) => {
                        self.sizes.insert(path, FolderSize::Known(n));
                    }
                    None => {
                        let slot = self.sizes.entry(path).or_insert(FolderSize::Unknown);
                        if *slot == FolderSize::Measuring {
                            *slot = FolderSize::Unknown;
                        }
                    }
                }
            }
            Msg::BookmarkDrop(paths) => {
                for path in paths.into_iter().filter(|p| p.is_dir()) {
                    self.set_bookmark(path, true);
                }
            }
            Msg::RemoveBookmark(path) => {
                self.bookmark_menu = None;
                self.set_bookmark(path, false);
            }
            Msg::BookmarkMenu(path, at) => self.bookmark_menu = Some((path, at)),
            Msg::CloseBookmarkMenu => self.bookmark_menu = None,
            Msg::Context(path, at) => {
                // Right-clicking an entry selects it, as file managers do,
                // unless it is already part of a selection to act on.
                if let Some(p) = &path
                    && !self.selected.contains(p)
                {
                    self.select_only(p.clone());
                }
                self.menu = Some((path, at));
            }
            Msg::CloseMenu => self.menu = None,
            Msg::Choose(choice) => {
                let Some((target, _)) = self.menu.take() else { return };
                match choice {
                    Choice::Open => self.update(Msg::Open),
                    Choice::Rename => self.update(Msg::Rename),
                    Choice::Trash => self.update(Msg::Trash),
                    Choice::NewFolder => self.update(Msg::NewFolder),
                    Choice::Hidden => self.update(Msg::Hidden),
                    Choice::Bookmark => {
                        let dir = target.filter(|p| p.is_dir()).unwrap_or_else(|| self.dir.clone());
                        let on = !self.bookmarks.contains(&dir);
                        self.set_bookmark(dir, on);
                    }
                    Choice::Terminal => {
                        let dir = target.filter(|p| p.is_dir()).unwrap_or_else(|| self.dir.clone());
                        if !neo_desktop::fs::open_in("neo-terminal", "NeoTerm", &dir) {
                            self.status = Some((Tone::Bad, "NeoTerm is not installed.".into()));
                        }
                    }
                }
            }
            Msg::Desktop(m) => {
                self.desktop.update(m);
            }
            Msg::Poll => {
                self.desktop.poll();
                if dir_stamp(&self.dir) != self.stamp {
                    let status = self.status.take();
                    self.reload();
                    self.status = self.status.take().or(status);
                }
            }
        }
    }

    fn view(&self) -> Element<Msg> {
        self.desktop.with_settings(self.content(), "Files Settings", Msg::Desktop, vec![])
    }
}

impl Files {
    /// The window's content, which the settings panel goes over.
    fn content(&self) -> Element<Msg> {
        let main = split(self.places(), column().width(Length::Fill).height(Length::Fill).push(self.toolbar()).push(Divider::horizontal()).push(self.list()).push(Divider::horizontal()).push(self.status_bar()));
        // The menu on a bookmark in the sidebar.
        if let Some((path, at)) = &self.bookmark_menu {
            let items = vec![MenuItem::new("Open", Msg::Go(path.clone())).icon(icons::FOLDER_OPEN), MenuItem::separator(), MenuItem::new("Remove from Sidebar", Msg::RemoveBookmark(path.clone())).icon(icons::BOOKMARK_MINUS)];
            return stack().width(Length::Fill).height(Length::Fill).push(main).push(popup_menu(*at, items, Msg::CloseBookmarkMenu)).into();
        }
        let Some((target, at)) = &self.menu else { return main };
        let entry = target.as_ref().and_then(|p| self.entries.iter().find(|e| &e.path == p));
        let items = match entry {
            // Several entries are selected: the menu acts on all of them.
            Some(_) if self.selected.len() > 1 => vec![
                MenuItem::new(format!("Open {} Items", self.selected.len()), Msg::Choose(Choice::Open)).icon(icons::EXTERNAL_LINK),
                MenuItem::separator(),
                MenuItem::disabled("Rename…").icon(icons::PENCIL),
                MenuItem::new(format!("Move {} Items to Trash", self.selected.len()), Msg::Choose(Choice::Trash)).icon(icons::TRASH_2).danger(),
            ],
            Some(e) => {
                let mut items = vec![MenuItem::new("Open", Msg::Choose(Choice::Open)).icon(if e.dir { icons::FOLDER_OPEN } else { icons::EXTERNAL_LINK })];
                if e.dir {
                    items.push(MenuItem::new("Open in Terminal", Msg::Choose(Choice::Terminal)).icon(icons::SQUARE_TERMINAL));
                    let marked = self.bookmarks.contains(&e.path);
                    items.push(MenuItem::new(if marked { "Remove from Sidebar" } else { "Add to Sidebar" }, Msg::Choose(Choice::Bookmark)).icon(if marked { icons::BOOKMARK_MINUS } else { icons::BOOKMARK_PLUS }));
                }
                items.extend([MenuItem::separator(), MenuItem::new("Rename…", Msg::Choose(Choice::Rename)).icon(icons::PENCIL), MenuItem::new("Move to Trash", Msg::Choose(Choice::Trash)).icon(icons::TRASH_2).danger()]);
                items
            }
            None => vec![
                MenuItem::new("New Folder", Msg::Choose(Choice::NewFolder)).icon(icons::FOLDER_PLUS),
                MenuItem::new("Open Terminal Here", Msg::Choose(Choice::Terminal)).icon(icons::SQUARE_TERMINAL),
                {
                    let marked = self.bookmarks.contains(&self.dir);
                    MenuItem::new(if marked { "Remove This Folder from Sidebar" } else { "Add This Folder to Sidebar" }, Msg::Choose(Choice::Bookmark)).icon(if marked { icons::BOOKMARK_MINUS } else { icons::BOOKMARK_PLUS })
                },
                MenuItem::separator(),
                MenuItem::new(if self.show_hidden { "Hide Hidden Files" } else { "Show Hidden Files" }, Msg::Choose(Choice::Hidden)).icon(if self.show_hidden { icons::EYE_OFF } else { icons::EYE }),
            ],
        };
        stack().width(Length::Fill).height(Length::Fill).push(main).push(popup_menu(*at, items, Msg::CloseMenu)).into()
    }
}

impl Files {
    fn places(&self) -> Element<Msg> {
        let home = home_dir();
        let mut places = vec![(icons::HOUSE, "Home".to_string(), home.clone())];
        for (glyph, key) in [(icons::MONITOR, "DESKTOP"), (icons::FILE_TEXT, "DOCUMENTS"), (icons::DOWNLOAD, "DOWNLOAD"), (icons::IMAGE, "PICTURES"), (icons::MUSIC, "MUSIC"), (icons::VIDEO, "VIDEOS")] {
            let p = user_dir(key);
            if p.is_dir() && p != home {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                places.push((glyph, name, p));
            }
        }
        let mut col = column().spacing(2.0).width(Length::Fill).push(section("Places"));
        for (glyph, name, path) in places {
            let here = self.dir == path;
            // Files can be dropped on a place to put them there.
            let target = path.clone();
            col = col.push(mouse_area(nav_item(glyph, name, here, Msg::Go(path))).on_drop(move |files| Msg::Drop(target.clone(), files)));
        }
        // Bookmarks: folders put here from the right-click menu, or by
        // dropping them on the heading.
        let heading = mouse_area(section("Bookmarks")).on_drop(Msg::BookmarkDrop);
        col = col.push(heading);
        for path in &self.bookmarks {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
            let (target, menu) = (path.clone(), path.clone());
            let glyph = if path.is_dir() { icons::FOLDER } else { icons::FOLDER_X };
            col = col.push(mouse_area(nav_item(glyph, name, self.dir == *path, Msg::Go(path.clone()))).on_drop(move |files| Msg::Drop(target.clone(), files)).on_secondary_press(move |at| Msg::BookmarkMenu(menu.clone(), at)));
        }
        if self.bookmarks.is_empty() {
            col = col.push(container(text("Right-click a folder and choose Add to Sidebar, or drop one here.").role(TextRole::Caption).tone(Tone::Faint)).padding([4.0, 10.0]));
        }
        col = col.push(section("Devices"));
        for (name, root) in roots() {
            let here = self.dir == root;
            col = col.push(nav_item(icons::HARD_DRIVE, name, here, Msg::Go(root)));
        }
        scrollable(col).into()
    }

    fn toolbar(&self) -> Element<Msg> {
        let nav = |glyph, enabled: bool, m: Msg| icon_button(glyph, 34.0).kind(ButtonKind::Ghost).on_press_maybe(enabled.then_some(m));
        // Breadcrumb: the last few ancestors of the current folder.
        let mut crumbs: Vec<(String, PathBuf)> = self.dir.ancestors().map(|a| (a.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "/".into()), a.to_path_buf())).collect();
        crumbs.reverse();
        let skip = crumbs.len().saturating_sub(3);
        let mut path = row().spacing(2.0).align(Align::Center);
        if skip > 0 {
            path = path.push(text("…").tone(Tone::Muted)).push(icon(icons::CHEVRON_RIGHT).size(14.0).tone(Tone::Faint));
        }
        let last = crumbs.len() - 1;
        for (i, (name, p)) in crumbs.into_iter().enumerate().skip(skip) {
            let current = i == last;
            let label = text(name).role(TextRole::Strong).no_wrap().tone(if current { Tone::Inherit } else { Tone::Muted });
            // A long folder name is cut short so it cannot crowd out the toolbar.
            path = path.push(Button::new(container(label).max_width(if current { 136.0 } else { 84.0 })).kind(ButtonKind::Ghost).padding([8.0, 6.0]).on_press(Msg::Go(p)));
            if !current {
                path = path.push(icon(icons::CHEVRON_RIGHT).size(14.0).tone(Tone::Faint));
            }
        }
        row()
            .spacing(4.0)
            .align(Align::Center)
            .width(Length::Fill)
            .padding([10.0, 12.0])
            .push(nav(icons::ARROW_LEFT, !self.back.is_empty(), Msg::Back))
            .push(nav(icons::ARROW_RIGHT, !self.forward.is_empty(), Msg::Forward))
            .push(nav(icons::ARROW_UP, self.dir.parent().is_some(), Msg::Up))
            .push(Space::new(6.0, 0.0))
            .push(path)
            .push(Space::fill_x())
            .push(container(text_input("Search this folder", self.query.clone()).on_input(Msg::Query).on_cancel(Msg::Query(String::new()))).width(170.0))
            .push(icon_button(if self.show_hidden { icons::EYE } else { icons::EYE_OFF }, 34.0).kind(ButtonKind::Ghost).selected(self.show_hidden).on_press(Msg::Hidden))
            .push(icon_button(icons::FOLDER_PLUS, 34.0).kind(ButtonKind::Ghost).on_press(Msg::NewFolder))
            .push(icon_button(icons::PENCIL, 34.0).kind(ButtonKind::Ghost).on_press_maybe((self.selected.len() == 1).then_some(Msg::Rename)))
            .push(icon_button(icons::TRASH_2, 34.0).kind(ButtonKind::Ghost).on_press_maybe((!self.selected.is_empty()).then_some(Msg::Trash)))
            .into()
    }

    fn header(&self) -> Element<Msg> {
        let col = |label: &str, by: SortBy, width: Length, align: Align| -> Element<Msg> {
            let mut r = row().spacing(4.0).align(Align::Center).push(text(label).role(TextRole::Label).tone(if self.sort == by { Tone::Accent } else { Tone::Muted }));
            if self.sort == by {
                r = r.push(icon(if self.descending { icons::CHEVRON_DOWN } else { icons::CHEVRON_UP }).size(12.0).tone(Tone::Accent));
            }
            Button::new(r).kind(ButtonKind::Ghost).padding([6.0, 6.0]).width(width).align_x(align).on_press(Msg::Sort(by)).into()
        };
        row()
            .spacing(12.0)
            .width(Length::Fill)
            .padding([4.0, 12.0, 4.0, 40.0])
            .push(col("Name", SortBy::Name, Length::Fill, Align::Start))
            .push(col("Size", SortBy::Size, Length::Fixed(90.0), Align::End))
            .push(col("Modified", SortBy::Modified, Length::Fixed(130.0), Align::Start))
            .push(col("Created", SortBy::Created, Length::Fixed(130.0), Align::Start))
            .into()
    }

    /// Wraps an entry's button with what every view shares: the context
    /// menu, dragging it away, and dropping onto it if it is a folder.
    fn entry_area(&self, e: &Entry, button: Button<Msg>) -> Element<Msg> {
        let path = e.path.clone();
        // Dragging a selected entry takes the whole selection with it.
        let dragged = if self.selected.contains(&e.path) { self.selected.clone() } else { vec![e.path.clone()] };
        let mut area = mouse_area(button).on_secondary_press(move |at| Msg::Context(Some(path.clone()), at)).drag_files(dragged);
        if e.dir {
            let folder = e.path.clone();
            area = area.on_drop(move |files| Msg::Drop(folder.clone(), files));
        }
        area.into()
    }

    fn entry_icon(e: &Entry) -> neo::theme::Icon {
        if e.link && e.dir {
            icons::FOLDER_SYMLINK
        } else if e.link {
            icons::FILE_SYMLINK
        } else {
            file_icon(&e.path, e.dir)
        }
    }

    fn row(&self, e: &Entry) -> Element<Msg> {
        let selected = self.selected.contains(&e.path);
        if let Some(Naming::Rename { from, name }) = &self.naming
            && from == &e.path
        {
            return self.name_field(file_icon(&e.path, e.dir), name.clone());
        }
        let compact = self.view == ViewMode::Compact;
        // A folder's size once added up; a pause while that happens.
        let size = match (e.dir, self.sizes.get(&e.path)) {
            (false, _) => human_size(e.size),
            (true, Some(FolderSize::Known(n))) => human_size(*n),
            (true, Some(FolderSize::Measuring)) => "…".to_string(),
            (true, _) => "—".to_string(),
        };
        let when = e.modified.map(friendly_time).unwrap_or_default();
        let created = e.created.map(friendly_time).unwrap_or_else(|| "—".into());
        let faded = if e.hidden() { Tone::Muted } else { Tone::Inherit };
        let name = text(e.name.clone()).no_wrap().tone(faded).width(Length::Fill);
        let content = row()
            .spacing(12.0)
            .align(Align::Center)
            .width(Length::Fill)
            .push(icon(Self::entry_icon(e)).size(if compact { 15.0 } else { 17.0 }).tone(if e.dir { Tone::Accent } else { Tone::Muted }))
            .push(if compact { name.role(TextRole::Caption) } else { name })
            .push(text(size).role(TextRole::Caption).tone(Tone::Muted).align(Align::End).width(90.0))
            .push(text(when).role(TextRole::Caption).tone(Tone::Muted).no_wrap().width(130.0))
            .push(text(created).role(TextRole::Caption).tone(Tone::Muted).no_wrap().width(130.0));
        let clicked = e.path.clone();
        let button = Button::new(content).kind(ButtonKind::Ghost).selected(selected).padding([10.0, if compact { 2.0 } else { 6.0 }]).width(Length::Fill).align_x(Align::Start).on_press_with(move |modifiers| Msg::Click(clicked.clone(), modifiers));
        self.entry_area(e, button)
    }

    /// One tile of the grid: a thumbnail or a large icon over the name.
    fn tile(&self, e: &Entry) -> Element<Msg> {
        let selected = self.selected.contains(&e.path);
        let preview: Element<Msg> = match self.thumbs.get(&e.path) {
            Some(Thumb::Ready(image)) => picture(image).width(108.0).height(84.0).into(),
            _ => icon(Self::entry_icon(e)).size(44.0).tone(if e.dir { Tone::Accent } else { Tone::Muted }).into(),
        };
        let faded = if e.hidden() { Tone::Muted } else { Tone::Inherit };
        let content = column()
            .spacing(6.0)
            .align(Align::Center)
            .width(Length::Fill)
            .push(container(preview).width(108.0).height(84.0).center())
            .push(text(e.name.clone()).role(TextRole::Caption).tone(faded).no_wrap().align(Align::Center).width(Length::Fill));
        let clicked = e.path.clone();
        let button = Button::new(content).kind(ButtonKind::Ghost).selected(selected).padding([8.0, 8.0]).width(TILE_W).on_press_with(move |modifiers| Msg::Click(clicked.clone(), modifiers));
        self.entry_area(e, button)
    }

    /// How many tiles fit across the folder view.
    fn columns(&self) -> usize {
        // The view is the window less the sidebar, its margins and the list's padding.
        let room = self.width - neo_desktop::ui::SIDEBAR_W - 12.0 - 20.0;
        (((room + TILE_GAP) / (TILE_W + TILE_GAP)).floor() as usize).max(1)
    }

    fn name_field(&self, glyph: neo::theme::Icon, value: String) -> Element<Msg> {
        row()
            .spacing(12.0)
            .align(Align::Center)
            .width(Length::Fill)
            .padding([10.0, 2.0])
            .push(icon(glyph).size(17.0).tone(Tone::Accent))
            .push(text_input("Name", value).on_input(Msg::NameInput).on_submit(Msg::NameSubmit).on_cancel(Msg::NameCancel).autofocus(true))
            .push(button("Cancel").on_press(Msg::NameCancel))
            .into()
    }

    fn list(&self) -> Element<Msg> {
        let v = self.visible();
        let grid = self.view == ViewMode::Grid;
        let mut rows = column().spacing(if grid { TILE_GAP } else { 1.0 }).width(Length::Fill).padding([if grid { 10.0 } else { 4.0 }, 10.0, 10.0, 10.0]);
        if let Some(Naming::NewFolder(name)) = &self.naming {
            rows = rows.push(self.name_field(icons::FOLDER_PLUS, name.clone()));
        }
        if v.is_empty() && self.naming.is_none() {
            let (glyph, msg) = if self.query.is_empty() { (icons::FOLDER_OPEN, "This folder is empty.".to_string()) } else { (icons::SEARCH, format!("Nothing here matches “{}”.", self.query)) };
            let empty = column().spacing(10.0).align(Align::Center).push(icon(glyph).size(36.0).tone(Tone::Faint)).push(text(msg).tone(Tone::Muted));
            return column().width(Length::Fill).height(Length::Fill).push(self.header()).push(container(empty).width(Length::Fill).height(Length::Fill).center()).into();
        }
        if self.view == ViewMode::Grid {
            // Renaming takes a full-width field above the tiles.
            if let Some(Naming::Rename { from, name }) = &self.naming {
                rows = rows.push(self.name_field(file_icon(from, from.is_dir()), name.clone()));
            }
            for chunk in v.iter().take(MAX_ROWS).collect::<Vec<_>>().chunks(self.columns()) {
                rows = rows.push(chunk.iter().fold(row().spacing(TILE_GAP).width(Length::Fill), |r, e| r.push(self.tile(e))));
            }
        } else {
            for e in v.iter().take(MAX_ROWS) {
                rows = rows.push(self.row(e));
            }
        }
        if v.len() > MAX_ROWS {
            rows = rows.push(container(text(format!("Showing {MAX_ROWS} of {} items. Search to find the rest.", v.len())).role(TextRole::Caption).tone(Tone::Muted)).padding(12.0));
        }
        // A right click on the empty part of the folder, below the rows.
        let here = self.dir.clone();
        let rows = mouse_area(scrollable(rows).height(Length::Fill)).on_secondary_press(|at| Msg::Context(None, at)).on_drop(move |files| Msg::Drop(here.clone(), files));
        // The grid has no columns to label.
        let mut view = column().width(Length::Fill).height(Length::Fill);
        if !grid {
            view = view.push(self.header());
        }
        view.push(rows).into()
    }

    fn status_bar(&self) -> Element<Msg> {
        let v = self.visible();
        let folders = v.iter().filter(|e| e.dir).count();
        let files = v.len() - folders;
        let plural = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let mut summary = format!("{}, {}", plural(folders, "folder", "folders"), plural(files, "file", "files"));
        let chosen: Vec<&Entry> = self.entries.iter().filter(|e| self.selected.contains(&e.path)).collect();
        match chosen.as_slice() {
            [] => {}
            [e] => summary = match self.size_of(e) {
                Some(n) => format!("“{}” selected ({})", e.name, human_size(n)),
                None => format!("“{}” selected", e.name),
            },
            many => {
                // Folders count once they have been added up.
                let sizes: Vec<Option<u64>> = many.iter().map(|e| self.size_of(e)).collect();
                summary = if sizes.iter().all(Option::is_some) { format!("{} items selected ({})", many.len(), human_size(sizes.iter().flatten().sum())) } else { format!("{} items selected", many.len()) };
            }
        }
        let mut r = row().spacing(12.0).align(Align::Center).width(Length::Fill).padding([16.0, 10.0]).push(text(summary).role(TextRole::Caption).tone(Tone::Muted)).push(Space::fill_x());
        if let Some((tone, msg)) = &self.status {
            r = r.push(notice(*tone, msg.clone()));
        }
        // How the folder is shown: list, compact list or thumbnails.
        let mode = |glyph, mode: ViewMode| icon_button(glyph, 28.0).kind(ButtonKind::Ghost).selected(self.view == mode).on_press(Msg::View(mode));
        r.push(row().spacing(2.0).align(Align::Center).push(mode(icons::LIST, ViewMode::List)).push(mode(icons::ALIGN_JUSTIFY, ViewMode::Compact)).push(mode(icons::LAYOUT_GRID, ViewMode::Grid))).into()
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--snapshot") {
        snapshots(PathBuf::from(args.get(i + 1).cloned().unwrap_or_else(|| "target/snapshots".into())));
        return;
    }
    let dir = args.first().map(PathBuf::from).unwrap_or_else(home_dir);
    let dir = std::path::absolute(&dir).unwrap_or(dir);
    // Given a file, show its folder with the file selected.
    let (dir, select) = match dir.parent() {
        Some(parent) if !dir.is_dir() && dir.exists() => (parent.to_path_buf(), Some(dir.clone())),
        _ => (dir, None),
    };
    let mut files = Files::new(dir);
    files.selected = select.into_iter().collect();
    if let Err(e) = neo::run(files) {
        eprintln!("neo-files: {e}");
        std::process::exit(1);
    }
}

fn snapshots(dir: PathBuf) {
    use neo::testing::Harness;
    std::fs::create_dir_all(&dir).expect("create snapshot dir");

    // The grid, on a folder of made-up pictures.
    let album = std::env::temp_dir().join(format!("neo-files-album-{}", std::process::id()));
    std::fs::create_dir_all(album.join("Holiday")).expect("create album");
    for (i, (name, w, h)) in [("Beach at dusk.png", 640, 400), ("Mountain path.png", 400, 640), ("Harbour.png", 800, 450), ("Old town, a street with a rather long name.png", 500, 500), ("Market.png", 640, 360)].into_iter().enumerate() {
        let hue = i as f32 * 1.3;
        image::RgbaImage::from_fn(w, h, |x, y| {
            let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
            let wave = 0.55 + 0.12 * (u * 8.0 + hue).sin();
            let c = if v > wave { [40.0 + 60.0 * (hue).sin().abs(), 110.0, 90.0 + 50.0 * (hue).cos().abs()] } else { [120.0 + 100.0 * v, 150.0 + 60.0 * (hue + v).cos().abs(), 215.0] };
            image::Rgba([c[0] as u8, c[1] as u8, c[2] as u8, 255])
        })
        .save(album.join(name))
        .expect("write sample");
    }
    std::fs::write(album.join("Notes.txt"), "sample").expect("write sample");
    let mut app = Files::new(album.clone());
    app.desktop.appearance.scheme = neo_desktop::SchemePref::Light;
    let mut h = Harness::new(app, Size::new(1040.0, 680.0)).expect("GPU");
    h.app_mut().update(Msg::View(ViewMode::Grid));
    h.app_mut().update(Msg::Click(album.join("Harbour.png"), Modifiers::default()));
    // The thumbnails arrive from the worker.
    for _ in 0..100 {
        if h.app().thumbs.values().all(|t| *t != Thumb::Loading) && !h.app().thumbs.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
        h.advance(Duration::from_millis(20));
    }
    let path = dir.join("files-grid.png");
    h.save_png(&path, 1.0).expect("write png");
    println!("wrote {}", path.display());
    let _ = std::fs::remove_dir_all(album);

    let root = std::path::absolute(".").expect("cwd");
    for (name, scheme) in [("files-light", neo_desktop::SchemePref::Light), ("files-dark", neo_desktop::SchemePref::Dark)] {
        let mut app = Files::new(root.clone());
        app.desktop.appearance.scheme = scheme;
        app.selected = vec![root.join("Cargo.toml")];
        // Two folders in the sidebar's Bookmarks.
        app.bookmarks = vec![root.join("crates"), root.join("apps")];
        let mut h = Harness::new(app, Size::new(1040.0, 680.0)).expect("GPU");
        if name == "files-dark" {
            h.app_mut().show_hidden = true;
            h.app_mut().update(Msg::NewFolder);
        } else {
            // The context menu, as a right click on a folder shows it.
            h.app_mut().update(Msg::Context(Some(root.join("crates")), Point::new(420.0, 212.0)));
        }
        let path = dir.join(format!("{name}.png"));
        h.save_png(&path, 1.0).expect("write png");
        println!("wrote {}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("neo-files-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("alpha")).unwrap();
        std::fs::write(d.join("notes.txt"), "hi").unwrap();
        d
    }

    #[test]
    fn folders_show_how_much_they_hold_and_sort_by_it() {
        let root = temp_dir("sizes");
        std::fs::create_dir_all(root.join("big/deeper")).unwrap();
        std::fs::write(root.join("big/deeper/data.bin"), vec![0u8; 5000]).unwrap();
        std::fs::write(root.join("big/more.bin"), vec![0u8; 1000]).unwrap();
        std::fs::write(root.join("alpha/a.txt"), vec![0u8; 300]).unwrap();
        let mut f = Files::new(root.clone());
        let size = |f: &Files, name: &str| f.entries.iter().find(|e| e.name == name).and_then(|e| f.size_of(e));
        assert_eq!((size(&f, "big"), size(&f, "alpha"), size(&f, "notes.txt")), (Some(6000), Some(300), Some(2)), "everything under a folder, however deep");
        // Sorting by size puts the bigger folder first, folders still before files.
        f.update(Msg::Sort(SortBy::Size));
        if !f.descending {
            f.update(Msg::Sort(SortBy::Size));
        }
        assert_eq!(f.visible().iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["big", "alpha", "notes.txt"]);
        // A selection's size counts the folders in it.
        f.update(Msg::Click(root.join("big"), Modifiers::default()));
        f.update(Msg::Click(root.join("notes.txt"), Modifiers { shift: true, ..Default::default() }));
        let mut h = neo::testing::Harness::new(f, Size::new(1040.0, 680.0)).unwrap();
        h.render(1.0);
        // While a worker is still counting, the folder says so.
        h.app_mut().sizes.insert(root.join("alpha"), FolderSize::Measuring);
        let alpha = h.app().entries.iter().find(|e| e.name == "alpha").unwrap().clone();
        assert_eq!(h.app().size_of(&alpha), None);
        // A late answer for a folder that could not be read keeps what was known.
        h.app_mut().update(Msg::Measured(root.join("big"), None));
        assert_eq!(h.app().sizes[&root.join("big")], FolderSize::Known(6000));
        h.app_mut().update(Msg::Measured(root.join("alpha"), None));
        assert_eq!(h.app().sizes[&root.join("alpha")], FolderSize::Unknown);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn folders_can_be_bookmarked_in_the_sidebar_and_kept() {
        let root = temp_dir("bookmarks");
        let file = root.join("kept/files-bookmarks");
        let mut f = Files::new(root.clone());
        assert!(f.bookmarks_file.is_none() && f.bookmarks.is_empty(), "tests keep nothing unless they say where");
        f.bookmarks_file = Some(file.clone());
        // From a folder's right-click menu.
        f.update(Msg::Context(Some(root.join("alpha")), Point::new(300.0, 200.0)));
        f.update(Msg::Choose(Choice::Bookmark));
        assert_eq!(f.bookmarks, [root.join("alpha")]);
        // From the folder's own background: the folder being shown.
        f.update(Msg::Context(None, Point::new(300.0, 400.0)));
        f.update(Msg::Choose(Choice::Bookmark));
        assert_eq!(f.bookmarks, [root.join("alpha"), root.clone()]);
        // Dropped on the heading: folders are bookmarked, files are not, and nothing twice.
        f.update(Msg::BookmarkDrop(vec![root.join("notes.txt"), root.join("alpha")]));
        assert_eq!(f.bookmarks.len(), 2);
        // Kept for next time, in order.
        assert_eq!(folders::load_bookmarks(&file), [root.join("alpha"), root.clone()]);
        // A bookmark goes where it points, and its own menu takes it out.
        f.update(Msg::Go(root.join("alpha")));
        assert_eq!(f.dir, root.join("alpha"));
        f.update(Msg::BookmarkMenu(root.join("alpha"), Point::new(40.0, 300.0)));
        assert!(f.bookmark_menu.is_some());
        f.update(Msg::RemoveBookmark(root.join("alpha")));
        assert!(f.bookmark_menu.is_none());
        assert_eq!(folders::load_bookmarks(&file), std::slice::from_ref(&root));
        // The same choice on a bookmarked folder takes it out too.
        f.update(Msg::Go(root.clone()));
        f.update(Msg::Context(None, Point::new(300.0, 400.0)));
        f.update(Msg::Choose(Choice::Bookmark));
        assert!(f.bookmarks.is_empty());
        // The sidebar shows them.
        f.update(Msg::BookmarkDrop(vec![root.join("alpha")]));
        let mut h = neo::testing::Harness::new(f, Size::new(1040.0, 680.0)).unwrap();
        let with = h.render(1.0);
        h.app_mut().update(Msg::RemoveBookmark(root.join("alpha")));
        assert!(h.render(1.0) != with);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn double_click_opens_folders_and_history_works() {
        let root = temp_dir("nav");
        let mut f = Files::new(root.clone());
        assert_eq!(f.visible().iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["alpha", "notes.txt"]);
        f.update(Msg::Click(root.join("alpha"), Modifiers::default()));
        assert_eq!(f.dir, root, "a single click only selects");
        f.update(Msg::Click(root.join("alpha"), Modifiers::default()));
        assert_eq!(f.dir, root.join("alpha"));
        f.update(Msg::Back);
        assert_eq!(f.dir, root);
        f.update(Msg::Forward);
        assert_eq!(f.dir, root.join("alpha"));
        f.update(Msg::Up);
        assert_eq!(f.dir, root);
        assert_eq!(f.selected, [root.join("alpha")], "going up selects the folder you came from");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn creates_and_renames() {
        let root = temp_dir("naming");
        let mut f = Files::new(root.clone());
        f.update(Msg::NewFolder);
        f.update(Msg::NameInput("beta".into()));
        f.update(Msg::NameSubmit);
        assert!(root.join("beta").is_dir());
        assert_eq!(f.selected, [root.join("beta")]);
        f.update(Msg::Rename);
        f.update(Msg::NameInput("notes.txt".into()));
        f.update(Msg::NameSubmit);
        assert!(f.naming.is_some(), "renaming onto an existing name is refused");
        assert!(f.status.as_ref().is_some_and(|(t, _)| *t == Tone::Bad));
        f.update(Msg::NameInput("gamma".into()));
        f.update(Msg::NameSubmit);
        assert!(root.join("gamma").is_dir() && !root.join("beta").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_right_click_selects_and_the_menu_acts_on_it() {
        let root = temp_dir("menu");
        let mut f = Files::new(root.clone());
        f.update(Msg::Context(Some(root.join("notes.txt")), Point::new(300.0, 200.0)));
        assert_eq!(f.selected, [root.join("notes.txt")]);
        assert!(f.menu.is_some());
        f.update(Msg::Choose(Choice::Rename));
        assert!(f.menu.is_none(), "choosing closes the menu");
        assert!(matches!(&f.naming, Some(Naming::Rename { name, .. }) if name == "notes.txt"));
        f.update(Msg::NameCancel);
        // On the folder's background the menu makes folders and shows hidden files.
        f.update(Msg::Context(None, Point::new(300.0, 400.0)));
        assert_eq!(f.selected, [root.join("notes.txt")], "the selection is left alone");
        f.update(Msg::Choose(Choice::Hidden));
        assert!(f.show_hidden);
        f.update(Msg::Context(None, Point::new(300.0, 400.0)));
        f.update(Msg::CloseMenu);
        assert!(f.menu.is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn shift_and_command_clicks_select_several() {
        let root = temp_dir("multi");
        for name in ["b.txt", "c.txt", "d.txt"] {
            std::fs::write(root.join(name), "x").unwrap();
        }
        let mut f = Files::new(root.clone());
        // Shown as: alpha, b.txt, c.txt, d.txt, notes.txt.
        let p = |n: &str| root.join(n);
        let plain = Modifiers::default();
        let shift = Modifiers { shift: true, ..plain };
        let toggle = if cfg!(target_os = "macos") { Modifiers { logo: true, ..plain } } else { Modifiers { ctrl: true, ..plain } };
        f.update(Msg::Click(p("b.txt"), plain));
        f.update(Msg::Click(p("d.txt"), shift));
        assert_eq!(f.selected, [p("b.txt"), p("c.txt"), p("d.txt")], "a range from the anchor");
        f.update(Msg::Click(p("alpha"), shift));
        assert_eq!(f.selected, [p("alpha"), p("b.txt")], "a new range from the same anchor, the other way");
        f.update(Msg::Click(p("notes.txt"), toggle));
        assert_eq!(f.selected, [p("alpha"), p("b.txt"), p("notes.txt")], "added one");
        f.update(Msg::Click(p("alpha"), toggle));
        assert_eq!(f.selected, [p("b.txt"), p("notes.txt")], "and took one away");
        f.update(Msg::Rename);
        assert!(f.naming.is_none(), "renaming needs exactly one entry");
        // A right click inside the selection keeps it; outside replaces it.
        f.update(Msg::Context(Some(p("b.txt")), Point::new(1.0, 1.0)));
        assert_eq!(f.selected.len(), 2);
        f.update(Msg::CloseMenu);
        f.update(Msg::Context(Some(p("c.txt")), Point::new(1.0, 1.0)));
        assert_eq!(f.selected, [p("c.txt")]);
        f.update(Msg::CloseMenu);
        f.update(Msg::SelectAll);
        assert_eq!(f.selected.len(), 5);
        f.update(Msg::Click(p("c.txt"), plain));
        assert_eq!(f.selected, [p("c.txt")], "a plain click goes back to one");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dropping_on_a_folder_moves_and_from_outside_copies() {
        let root = temp_dir("drop");
        let outside = temp_dir("drop-outside");
        std::fs::write(outside.join("photo.png"), "p").unwrap();
        let mut f = Files::new(root.clone());
        // Dragged within the folder onto a subfolder.
        f.update(Msg::Drop(root.join("alpha"), vec![root.join("notes.txt")]));
        assert!(root.join("alpha/notes.txt").exists() && !root.join("notes.txt").exists());
        assert!(f.status.as_ref().is_some_and(|(t, m)| *t == Tone::Good && m == "Moved 1 item."));
        assert!(f.entries.iter().all(|e| e.name != "notes.txt"), "the list is refreshed");
        // Dropped from another folder, as from another app.
        f.update(Msg::Drop(root.clone(), vec![outside.join("photo.png")]));
        assert!(root.join("photo.png").exists() && outside.join("photo.png").exists());
        assert!(f.status.as_ref().is_some_and(|(_, m)| m == "Copied 1 item."));
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn the_grid_fits_tiles_to_the_window_and_asks_for_thumbnails() {
        let root = temp_dir("grid");
        image::RgbaImage::from_pixel(40, 20, image::Rgba([10, 200, 90, 255])).save(root.join("pic.png")).unwrap();
        let mut f = Files::new(root.clone());
        // 1040 wide: 1040 − 212 − 12 − 20 leaves room for five 132-wide tiles and their gaps.
        assert_eq!(f.columns(), 5);
        f.update(Msg::Geometry(WindowGeometry { frame: neo::Rect::new(0.0, 0.0, 640.0, 480.0), screen: neo::Rect::ZERO, scale: 1.0 }));
        assert_eq!(f.columns(), 2);
        // Thumbnails are only wanted in the grid, and only for pictures.
        let (tx, rx) = std::sync::mpsc::channel();
        f.thumb_queue = Some(tx);
        f.request_thumbs();
        assert!(rx.try_recv().is_err(), "the list needs no thumbnails");
        f.update(Msg::View(ViewMode::Grid));
        assert_eq!(rx.try_iter().map(|(_, p)| p).collect::<Vec<_>>(), [root.join("pic.png")]);
        assert_eq!(f.thumbs.get(&root.join("pic.png")), Some(&Thumb::Loading));
        // The worker's answer fills it in, scaled down with its shape kept.
        let thumb = thumbnail(&root.join("pic.png")).unwrap();
        assert_eq!((thumb.width(), thumb.height()), (40, 20), "a small picture is not scaled up");
        f.update(Msg::Thumb(root.join("pic.png"), Some(thumb)));
        assert!(matches!(f.thumbs.get(&root.join("pic.png")), Some(Thumb::Ready(_))));
        assert!(thumbnail(&root.join("notes.txt")).is_none());
        // Leaving the folder drops its thumbnails.
        f.update(Msg::Go(root.join("alpha")));
        assert!(f.thumbs.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_bad_names() {
        assert!(valid_name("  ").is_err());
        assert!(valid_name("a/b").is_err());
        assert_eq!(valid_name(" notes ").unwrap(), "notes");
    }
}
