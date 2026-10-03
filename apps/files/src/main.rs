//! Neo Files: browse, open and organise files.
//!
//!     cargo run -p neo-files [folder]
//!     cargo run -p neo-files -- --snapshot target/snapshots
//!
//! Double-click or press Enter to open. Backspace goes back, Delete moves
//! the selection to the Trash, and typing in the search field filters the
//! current folder.

// Release builds on Windows open no console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use neo::prelude::*;
use neo::{KeyEvent, Key, Size};
use neo_desktop::fs::{file_icon, friendly_time, home_dir, human_size, move_to_trash, open, roots, user_dir};
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
    selected: Option<PathBuf>,
    last_click: Option<(PathBuf, Instant)>,
    query: String,
    show_hidden: bool,
    sort: SortBy,
    descending: bool,
    naming: Option<Naming>,
    status: Option<(Tone, String)>,
}

#[derive(Clone, Debug)]
enum Msg {
    Go(PathBuf),
    Back,
    Forward,
    Up,
    Click(PathBuf),
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
    Poll,
}

fn read_dir(dir: &Path) -> std::io::Result<Vec<Entry>> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir)?.flatten() {
        let path = e.path();
        let link = e.file_type().is_ok_and(|t| t.is_symlink());
        // Follow links for size and kind, but keep broken ones.
        let meta = std::fs::metadata(&path).or_else(|_| e.metadata());
        let (dir, size, modified) = match &meta {
            Ok(m) => (m.is_dir(), if m.is_dir() { 0 } else { m.len() }, m.modified().ok()),
            Err(_) => (false, 0, None),
        };
        out.push(Entry { name: e.file_name().to_string_lossy().into_owned(), path, dir, link, size, modified });
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
            selected: None,
            last_click: None,
            query: String::new(),
            show_hidden: false,
            sort: SortBy::Name,
            descending: false,
            naming: None,
            status: None,
        };
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
                    self.selected = None;
                    self.query.clear();
                    self.naming = None;
                }
                self.dir = dir;
                self.status = None;
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
                SortBy::Size => a.size.cmp(&b.size).then_with(|| natural_cmp(&a.name, &b.name)),
                SortBy::Modified => a.modified.cmp(&b.modified),
            };
            dirs_first.then(if self.descending { by.reverse() } else { by })
        });
        v
    }

    fn open_entry(&mut self, path: PathBuf) {
        let Some(e) = self.entries.iter().find(|e| e.path == path) else { return };
        if e.dir {
            self.navigate(path);
        } else if let Err(err) = open(&path) {
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
                self.selected = Some(path);
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

/// Compares names so that "file2" sorts before "file10".
fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let num = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut s = String::new();
                    while let Some(c) = it.peek().filter(|c| c.is_ascii_digit()) {
                        s.push(*c);
                        it.next();
                    }
                    s
                };
                let (na, nb) = (num(&mut a), num(&mut b));
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let o = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(x), Some(y)) => {
                let o = x.to_lowercase().cmp(y.to_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
                a.next();
                b.next();
            }
        }
    }
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

    fn theme(&self, system: Scheme) -> Theme {
        self.desktop.theme(system)
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
                    self.selected = Some(child);
                }
            }
            Msg::Click(p) => {
                let now = Instant::now();
                let double = self.last_click.as_ref().is_some_and(|(q, t)| *q == p && now.duration_since(*t) < DOUBLE_CLICK);
                self.selected = Some(p.clone());
                if double {
                    self.last_click = None;
                    self.open_entry(p);
                } else {
                    self.last_click = Some((p, now));
                }
            }
            Msg::Open => {
                if let Some(p) = self.selected.clone() {
                    self.open_entry(p);
                }
            }
            Msg::Query(q) => self.query = q,
            Msg::Hidden => self.show_hidden = !self.show_hidden,
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
                if let Some(p) = &self.selected {
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
                if let Some(p) = self.selected.take() {
                    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    match move_to_trash(&p) {
                        Ok(_) => {
                            self.reload();
                            self.status = Some((Tone::Good, format!("Moved {name} to the Trash.")));
                        }
                        Err(e) => {
                            self.status = Some((Tone::Bad, format!("Could not move {name} to the Trash: {e}")));
                            self.selected = Some(p);
                        }
                    }
                }
            }
            Msg::Select(d) => {
                let v = self.visible();
                if v.is_empty() {
                    return;
                }
                let cur = self.selected.as_ref().and_then(|s| v.iter().position(|e| &e.path == s));
                let next = match cur {
                    None if d > 0 => 0,
                    None => v.len() - 1,
                    Some(i) => (i as isize + d).clamp(0, v.len() as isize - 1) as usize,
                };
                self.selected = Some(v[next].path.clone());
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
        split(self.places(), column().width(Length::Fill).height(Length::Fill).push(self.toolbar()).push(Divider::horizontal()).push(self.list()).push(Divider::horizontal()).push(self.status_bar()))
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
            col = col.push(nav_item(glyph, name, here, Msg::Go(path)));
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
            path = path.push(Button::new(label).kind(ButtonKind::Ghost).padding([8.0, 6.0]).on_press(Msg::Go(p)));
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
            .push(container(text_input("Search this folder", self.query.clone()).on_input(Msg::Query).on_cancel(Msg::Query(String::new()))).width(190.0))
            .push(icon_button(if self.show_hidden { icons::EYE } else { icons::EYE_OFF }, 34.0).kind(ButtonKind::Ghost).selected(self.show_hidden).on_press(Msg::Hidden))
            .push(icon_button(icons::FOLDER_PLUS, 34.0).kind(ButtonKind::Ghost).on_press(Msg::NewFolder))
            .push(icon_button(icons::PENCIL, 34.0).kind(ButtonKind::Ghost).on_press_maybe(self.selected.as_ref().map(|_| Msg::Rename)))
            .push(icon_button(icons::TRASH_2, 34.0).kind(ButtonKind::Ghost).on_press_maybe(self.selected.as_ref().map(|_| Msg::Trash)))
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
            .push(col("Modified", SortBy::Modified, Length::Fixed(150.0), Align::Start))
            .into()
    }

    fn row(&self, e: &Entry) -> Element<Msg> {
        let selected = self.selected.as_ref() == Some(&e.path);
        if let Some(Naming::Rename { from, name }) = &self.naming
            && from == &e.path
        {
            return self.name_field(file_icon(&e.path, e.dir), name.clone());
        }
        let glyph = if e.link && e.dir { icons::FOLDER_SYMLINK } else if e.link { icons::FILE_SYMLINK } else { file_icon(&e.path, e.dir) };
        let size = if e.dir { "—".to_string() } else { human_size(e.size) };
        let when = e.modified.map(friendly_time).unwrap_or_default();
        let faded = if e.hidden() { Tone::Muted } else { Tone::Inherit };
        let content = row()
            .spacing(12.0)
            .align(Align::Center)
            .width(Length::Fill)
            .push(icon(glyph).size(17.0).tone(if e.dir { Tone::Accent } else { Tone::Muted }))
            .push(text(e.name.clone()).no_wrap().tone(faded).width(Length::Fill))
            .push(text(size).role(TextRole::Caption).tone(Tone::Muted).align(Align::End).width(90.0))
            .push(text(when).role(TextRole::Caption).tone(Tone::Muted).width(150.0));
        Button::new(content).kind(ButtonKind::Ghost).selected(selected).padding([10.0, 6.0]).width(Length::Fill).align_x(Align::Start).on_press(Msg::Click(e.path.clone())).into()
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
        let mut rows = column().spacing(1.0).width(Length::Fill).padding([4.0, 10.0, 10.0, 10.0]);
        if let Some(Naming::NewFolder(name)) = &self.naming {
            rows = rows.push(self.name_field(icons::FOLDER_PLUS, name.clone()));
        }
        if v.is_empty() && self.naming.is_none() {
            let (glyph, msg) = if self.query.is_empty() { (icons::FOLDER_OPEN, "This folder is empty.".to_string()) } else { (icons::SEARCH, format!("Nothing here matches “{}”.", self.query)) };
            let empty = column().spacing(10.0).align(Align::Center).push(icon(glyph).size(36.0).tone(Tone::Faint)).push(text(msg).tone(Tone::Muted));
            return column().width(Length::Fill).height(Length::Fill).push(self.header()).push(container(empty).width(Length::Fill).height(Length::Fill).center()).into();
        }
        for e in v.iter().take(MAX_ROWS) {
            rows = rows.push(self.row(e));
        }
        if v.len() > MAX_ROWS {
            rows = rows.push(container(text(format!("Showing {MAX_ROWS} of {} items. Search to find the rest.", v.len())).role(TextRole::Caption).tone(Tone::Muted)).padding(12.0));
        }
        column().width(Length::Fill).height(Length::Fill).push(self.header()).push(scrollable(rows).height(Length::Fill)).into()
    }

    fn status_bar(&self) -> Element<Msg> {
        let v = self.visible();
        let folders = v.iter().filter(|e| e.dir).count();
        let files = v.len() - folders;
        let plural = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let mut summary = format!("{}, {}", plural(folders, "folder", "folders"), plural(files, "file", "files"));
        if let Some(e) = self.selected.as_ref().and_then(|s| self.entries.iter().find(|e| &e.path == s)) {
            summary = if e.dir { format!("“{}” selected", e.name) } else { format!("“{}” selected ({})", e.name, human_size(e.size)) };
        }
        let mut r = row().spacing(12.0).align(Align::Center).width(Length::Fill).padding([16.0, 10.0]).push(text(summary).role(TextRole::Caption).tone(Tone::Muted)).push(Space::fill_x());
        if let Some((tone, msg)) = &self.status {
            r = r.push(notice(*tone, msg.clone()));
        }
        r.into()
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
    files.selected = select;
    if let Err(e) = neo::run(files) {
        eprintln!("neo-files: {e}");
        std::process::exit(1);
    }
}

fn snapshots(dir: PathBuf) {
    use neo::testing::Harness;
    std::fs::create_dir_all(&dir).expect("create snapshot dir");
    let root = std::path::absolute(".").expect("cwd");
    for (name, scheme) in [("files-light", neo_desktop::SchemePref::Light), ("files-dark", neo_desktop::SchemePref::Dark)] {
        let mut app = Files::new(root.clone());
        app.desktop.appearance.scheme = scheme;
        app.selected = Some(root.join("Cargo.toml"));
        let mut h = Harness::new(app, Size::new(1040.0, 680.0)).expect("GPU");
        if name == "files-dark" {
            h.app_mut().show_hidden = true;
            h.app_mut().update(Msg::NewFolder);
        }
        let path = dir.join(format!("{name}.png"));
        h.save_png(&path, 1.0).expect("write png");
        println!("wrote {}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order() {
        let mut v = vec!["file10", "File2", "file1", "a"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["a", "file1", "File2", "file10"]);
    }

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("neo-files-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("alpha")).unwrap();
        std::fs::write(d.join("notes.txt"), "hi").unwrap();
        d
    }

    #[test]
    fn double_click_opens_folders_and_history_works() {
        let root = temp_dir("nav");
        let mut f = Files::new(root.clone());
        assert_eq!(f.visible().iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["alpha", "notes.txt"]);
        f.update(Msg::Click(root.join("alpha")));
        assert_eq!(f.dir, root, "a single click only selects");
        f.update(Msg::Click(root.join("alpha")));
        assert_eq!(f.dir, root.join("alpha"));
        f.update(Msg::Back);
        assert_eq!(f.dir, root);
        f.update(Msg::Forward);
        assert_eq!(f.dir, root.join("alpha"));
        f.update(Msg::Up);
        assert_eq!(f.dir, root);
        assert_eq!(f.selected, Some(root.join("alpha")), "going up selects the folder you came from");
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
        assert_eq!(f.selected, Some(root.join("beta")));
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
    fn rejects_bad_names() {
        assert!(valid_name("  ").is_err());
        assert!(valid_name("a/b").is_err());
        assert_eq!(valid_name(" notes ").unwrap(), "notes");
    }
}
