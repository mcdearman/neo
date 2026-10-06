//! NeoDisk: how full each disk is, and what is using the space.
//!
//! Pick a disk or a folder and NeoDisk measures everything in it, then
//! shows the result two ways at once: a list of what is inside, largest
//! first, and a treemap in which each block's area is its size. Click
//! either to go into a folder.
//!
//! What is found is kept, here and between runs, and brought up to date
//! afterwards by reading only the folders that changed.
//!
//! A second tab lists the drives themselves, to mount, unmount, eject or
//! format them. Formatting is offered for external drives only.
//!
//!     cargo run -p neo-disk
//!     cargo run -p neo-disk -- ~/Downloads
//!     cargo run -p neo-disk -- --snapshot target/snapshots
//!     cargo run --release -p neo-disk -- --measure ~

#[cfg(target_os = "macos")]
mod bulk;
mod changes;
mod drives;
mod scan;
mod store;
mod treemap;

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use neo::prelude::*;
use neo::{Key, Proxy, Size};
use neo_desktop::fs::human_size;
use neo_desktop::ui::{nav_item, notice, section, split};
use neo_desktop::{Desktop, DesktopMsg};

use drives::{Action, Drive, Fs};
use scan::{Entry, Progress};
use treemap::{Item, Treemap};

/// A disk, as the system reports it.
#[derive(Clone, Debug, PartialEq)]
struct Disk {
    name: String,
    mount: PathBuf,
    total: u64,
    free: u64,
    removable: bool,
}

impl Disk {
    fn used(&self) -> u64 {
        self.total.saturating_sub(self.free)
    }
}

/// Whether a mounted volume is one a person thinks of as a disk. Systems
/// mount a good many more for their own purposes.
fn is_a_disk(mount: &Path) -> bool {
    let m = mount.to_string_lossy();
    if cfg!(target_os = "macos") {
        // The startup disk, and whatever is plugged in. The rest under
        // /System/Volumes are parts of the startup disk mounted again.
        m == "/" || m.starts_with("/Volumes/")
    } else if cfg!(windows) {
        true
    } else {
        !["/boot", "/snap", "/run", "/sys", "/proc", "/dev", "/var/lib/docker", "/var/snap"].iter().any(|p| m == *p || m.starts_with(&format!("{p}/")))
    }
}

fn disks() -> Vec<Disk> {
    let mut seen = std::collections::HashSet::new();
    let mut list: Vec<Disk> = sysinfo::Disks::new_with_refreshed_list()
        .iter()
        .filter(|d| d.total_space() > 0 && is_a_disk(d.mount_point()) && seen.insert(d.mount_point().to_path_buf()))
        .map(|d| {
            let name = d.name().to_string_lossy().into_owned();
            let mount = d.mount_point().to_path_buf();
            Disk { name: if name.is_empty() { mount.to_string_lossy().into_owned() } else { name }, mount, total: d.total_space(), free: d.available_space(), removable: d.is_removable() }
        })
        .collect();
    // The startup disk first, then by name.
    list.sort_by(|a, b| (a.mount.parent().is_some(), &a.name).cmp(&(b.mount.parent().is_some(), &b.name)));
    list
}

/// The two things NeoDisk does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    /// What is using the space.
    Usage,
    /// Mounting and formatting.
    Drives,
}

/// The form filled in before a drive is formatted.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Form {
    fs: Fs,
    name: String,
    /// The drive's present name, typed out to show it is the one meant.
    confirm: String,
}

/// What carries out commands on drives. Tests put a recorder here.
type Runner = Arc<dyn Fn(&[Vec<String>]) -> Result<String, String> + Send + Sync>;
type Lister = Arc<dyn Fn() -> Result<Vec<Drive>, String> + Send + Sync>;

struct NeoDisk {
    desktop: Desktop,
    tab: Tab,
    /// The drives found, once looked for.
    drives: Option<Result<Vec<Drive>, String>>,
    listing: bool,
    /// The drive showing in the Drives tab, by what the system calls it.
    chosen: Option<String>,
    /// What is being done to a drive just now.
    busy: Option<String>,
    /// How the last thing done to a drive went.
    outcome: Option<Result<String, String>>,
    form: Option<Form>,
    runner: Runner,
    lister: Lister,
    disks: Vec<Disk>,
    /// The disk or folder being looked at.
    target: Option<PathBuf>,
    /// What was found there.
    tree: Option<Arc<Entry>>,
    /// The way down from the top to the folder showing, as child indices.
    path: Vec<usize>,
    /// The showing folder's contents, laid out for the treemap.
    map: Rc<Vec<Item>>,
    progress: Arc<Progress>,
    running: bool,
    /// Which scan this is, so one that was abandoned is ignored when it ends.
    scan_no: u64,
    started: Instant,
    took: Option<Duration>,
    /// How the result showing was come by.
    how: How,
    /// The index behind the result showing, which the thread bringing it
    /// up to date takes and puts back.
    index: Arc<Mutex<Option<scan::Index>>>,
    /// Where indexes are saved between runs, if they are.
    cache: Option<PathBuf>,
    /// The scan showing was stopped before it finished.
    stopped: bool,
    proxy: Option<Proxy<Msg>>,
}

/// How a result was come by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum How {
    /// Everything was read.
    Measured,
    /// What was known was brought up to date by reading this many folders.
    Updated(usize),
    /// Saved from before, this many seconds ago, and not yet checked.
    Saved(u64),
}

/// One piece of measuring, done away from the window.
struct Job {
    no: u64,
    target: PathBuf,
    index: Arc<Mutex<Option<scan::Index>>>,
    cache: Option<PathBuf>,
    progress: Arc<Progress>,
    /// Read everything, whatever is known already.
    fully: bool,
}

impl Job {
    fn run(self, send: &mut dyn FnMut(Msg)) {
        let Job { no, target, index: slot, cache, progress, fully } = self;
        let take = || slot.lock().unwrap_or_else(|e| e.into_inner()).take();
        // What is known: from this run, or saved by an earlier one, which
        // is worth showing at once while it is checked.
        let known = if fully {
            take();
            None
        } else {
            take().or_else(|| {
                let saved = store::load(cache.as_deref()?, &target.canonicalize().unwrap_or_else(|_| target.clone()))?;
                let age = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()).saturating_sub(saved.taken);
                send(Msg::Scanned(no, Arc::new(saved.entry()), How::Saved(age)));
                Some(saved)
            })
        };
        let (index, how) = match known {
            Some(mut index) => {
                let report = scan::refresh(&mut index, &progress);
                (index, if report.full { How::Measured } else { How::Updated(report.reread) })
            }
            None => (scan::scan(&target, scan::Options::default(), &progress), How::Measured),
        };
        let entry = Arc::new(index.entry());
        // Stopped part-way, it is good to look at but not to build on.
        if !progress.cancel.load(Ordering::Relaxed) {
            if let Some(dir) = &cache {
                let _ = store::save(dir, &index);
            }
            *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(index);
        }
        send(Msg::Scanned(no, entry, how));
    }
}

#[derive(Clone, Debug)]
enum Msg {
    Scan(PathBuf),
    ChooseFolder,
    Scanned(u64, Arc<Entry>, How),
    /// Measure from nothing, setting aside what is known.
    RescanFully,
    /// Redraw the progress of a scan.
    Tick,
    /// Go into the showing folder's child at this index.
    Enter(usize),
    Up,
    /// Go back to this many levels down from the top.
    Jump(usize),
    /// Show the folder on screen in Files.
    Reveal,
    Rescan,
    Cancel,
    Tab(Tab),
    FindDrives,
    Found(Result<Vec<Drive>, String>),
    Pick(String),
    /// Mount, unmount or eject the drive showing.
    Do(Action),
    Done(Result<String, String>),
    AskFormat,
    FormFs(usize),
    FormName(String),
    FormConfirm(String),
    FormCancel,
    /// Format the drive showing as the form says.
    FormGo,
    Poll,
    /// The Settings entry and panel every Neo app has.
    Desktop(DesktopMsg),
}

/// How many rows the list shows; a folder with more says how many more.
const ROWS: usize = 200;

/// How long ago, roughly.
fn ago(secs: u64) -> String {
    let (n, unit) = match secs {
        0..60 => return "a moment ago".into(),
        60..3600 => (secs / 60, "minute"),
        3600..86_400 => (secs / 3600, "hour"),
        _ => (secs / 86_400, "day"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

impl NeoDisk {
    fn new() -> Self {
        Self {
            desktop: Desktop::load(),
            tab: Tab::Usage,
            drives: None,
            listing: false,
            chosen: None,
            busy: None,
            outcome: None,
            form: None,
            // Nothing a test does may reach a real drive.
            runner: if cfg!(test) { Arc::new(|steps| panic!("a test tried to run {steps:?}")) } else { Arc::new(drives::run) },
            lister: Arc::new(drives::list),
            disks: disks(),
            target: None,
            tree: None,
            path: vec![],
            map: Rc::new(vec![]),
            progress: Arc::default(),
            running: false,
            scan_no: 0,
            started: Instant::now(),
            took: None,
            how: How::Measured,
            index: Arc::default(),
            // Tests keep nothing on this computer unless they say where.
            cache: (!cfg!(test)).then(store::folder),
            stopped: false,
            proxy: None,
        }
    }

    /// The folder showing.
    fn current(&self) -> Option<&Entry> {
        self.tree.as_ref()?.at(&self.path)
    }

    /// Where the showing folder is on disk.
    fn folder_path(&self) -> Option<PathBuf> {
        let mut path = self.target.clone()?;
        let mut entry = self.tree.as_deref()?;
        for i in &self.path {
            entry = entry.children.get(*i)?;
            path.push(&entry.name);
        }
        Some(path)
    }

    fn show(&mut self, path: Vec<usize>) {
        self.path = path;
        self.map = Rc::new(self.current().map(treemap::items).unwrap_or_default());
    }

    /// The names of the folders gone into, from the top down.
    fn names(&self) -> Vec<String> {
        let mut names = vec![];
        let mut at = self.tree.as_deref();
        for i in &self.path {
            at = at.and_then(|e| e.children.get(*i));
            names.extend(at.map(|e| e.name.clone()));
        }
        names
    }

    /// Measures `target`. If it is the folder showing, what is known of it
    /// is brought up to date, unless `fully` asks for everything again.
    fn start(&mut self, target: PathBuf, fully: bool) {
        self.progress.cancel.store(true, Ordering::Relaxed);
        self.progress = Arc::default();
        self.scan_no += 1;
        self.stopped = false;
        self.started = Instant::now();
        if self.target.as_ref() != Some(&target) {
            self.index = Arc::default();
            self.tree = None;
            self.took = None;
            self.show(vec![]);
        }
        self.target = Some(target.clone());
        let job = Job { no: self.scan_no, target, index: self.index.clone(), cache: self.cache.clone(), progress: self.progress.clone(), fully };
        match self.proxy.clone() {
            Some(proxy) => {
                self.running = true;
                std::thread::spawn(move || {
                    job.run(&mut |m| {
                        proxy.send(m);
                    })
                });
            }
            // No event loop to report back to, as in tests: measure now.
            None => {
                let mut said = vec![];
                job.run(&mut |m| said.push(m));
                said.into_iter().for_each(|m| self.update(m));
            }
        }
    }

    fn sidebar(&self) -> Element<Msg> {
        let tabs = segmented(["Usage", "Drives"], Some(self.tab as usize), |i| Msg::Tab(if i == 0 { Tab::Usage } else { Tab::Drives })).width(Length::Fill);
        let mut side = column().spacing(2.0).width(Length::Fill).push(container(tabs).padding([2.0, 4.0]).width(Length::Fill));
        if self.tab == Tab::Drives {
            return scrollable(self.drive_list(side)).into();
        }
        side = side.push(section("Disks"));
        for d in &self.disks {
            let share = if d.total > 0 { d.used() as f32 / d.total as f32 } else { 0.0 };
            let label = column()
                .spacing(5.0)
                .width(Length::Fill)
                .push(row().spacing(8.0).align(Align::Center).push(icon(if d.removable { icons::USB } else { icons::HARD_DRIVE }).size(16.0)).push(text(d.name.clone()).role(TextRole::Strong).no_wrap()))
                .push(progress_bar(share).height(6.0).tone(if share > 0.9 { Tone::Bad } else { Tone::Accent }))
                .push(text(format!("{} used of {}", human_size(d.used()), human_size(d.total))).role(TextRole::Caption).tone(Tone::Muted))
                .push(text(format!("{} free", human_size(d.free))).role(TextRole::Caption).tone(Tone::Muted));
            side = side.push(Button::new(label).kind(ButtonKind::Ghost).selected(self.target.as_deref() == Some(d.mount.as_path())).width(Length::Fill).align_x(Align::Start).padding([10.0, 9.0]).on_press(Msg::Scan(d.mount.clone())));
        }
        let home = neo_desktop::fs::home_dir();
        side = side.push(section("Folders")).push(nav_item(icons::HOUSE, "Home", self.target.as_deref() == Some(home.as_path()), Msg::Scan(home))).push(nav_item(icons::FOLDER_SEARCH, "Choose Folder…", false, Msg::ChooseFolder));
        scrollable(side).into()
    }

    fn welcome(&self) -> Element<Msg> {
        container(column().spacing(8.0).align(Align::Center).push(icon(icons::CHART_PIE).size(42.0).tone(Tone::Faint)).push(text("See what is using the space").role(TextRole::Title).tone(Tone::Muted)).push(text("Choose a disk or a folder on the left, and NeoDisk measures everything in it.").role(TextRole::Caption).tone(Tone::Faint))).center().width(Length::Fill).height(Length::Fill).into()
    }

    fn scanning(&self) -> Element<Msg> {
        let (files, bytes) = (self.progress.files.load(Ordering::Relaxed), self.progress.bytes.load(Ordering::Relaxed));
        let name = self.target.as_deref().map(|t| t.display().to_string()).unwrap_or_default();
        let mut col = column().spacing(10.0).align(Align::Center).push(icon(icons::FOLDER_SEARCH).size(40.0).tone(Tone::Accent)).push(text(format!("Measuring {name}")).role(TextRole::Title).no_wrap()).push(text(format!("{files} files, {} so far", human_size(bytes))).mono().role(TextRole::Caption).tone(Tone::Muted));
        // For a whole disk the amount in use says how far there is to go.
        if let Some(d) = self.disks.iter().find(|d| Some(d.mount.as_path()) == self.target.as_deref()).filter(|d| d.used() > 0) {
            col = col.push(container(progress_bar((bytes as f32 / d.used() as f32).min(1.0)).height(6.0)).width(320.0));
        }
        col = col.push(Space::new(0.0, 4.0)).push(button("Stop").on_press(Msg::Cancel));
        container(col).center().width(Length::Fill).height(Length::Fill).into()
    }

    fn breakdown(&self, here: &Entry) -> Element<Msg> {
        let tree = self.tree.as_deref().expect("there is a folder showing");
        // The way here, each step a place to go back to.
        let top = self.target.as_deref().map(|t| t.display().to_string()).unwrap_or_default();
        let mut crumbs = row().spacing(2.0).align(Align::Center).push(Button::new(text(top).role(TextRole::Strong).no_wrap()).kind(ButtonKind::Ghost).padding([8.0, 4.0]).radius(6.0).on_press(Msg::Jump(0)));
        let mut at = tree;
        for (depth, i) in self.path.iter().enumerate() {
            at = &at.children[*i];
            crumbs = crumbs.push(icon(icons::CHEVRON_RIGHT).size(13.0).tone(Tone::Faint)).push(Button::new(text(at.name.clone()).role(TextRole::Strong).no_wrap()).kind(ButtonKind::Ghost).padding([8.0, 4.0]).radius(6.0).on_press(Msg::Jump(depth + 1)));
        }
        let header = row()
            .spacing(8.0)
            .align(Align::Center)
            .width(Length::Fill)
            .push(icon_button(icons::ARROW_UP, 30.0).kind(ButtonKind::Ghost).on_press_maybe((!self.path.is_empty()).then_some(Msg::Up)))
            .push(container(crumbs).width(Length::Fill))
            .push(text(human_size(here.size)).role(TextRole::Title).mono())
            .push(button("Show in Files").on_press(Msg::Reveal))
            .push(icon_button(icons::REFRESH_CW, 30.0).kind(ButtonKind::Ghost).on_press(Msg::Rescan));
        let mut note = format!("{} files", here.files);
        let took = self.took.map_or(0.0, |t| t.as_secs_f32());
        match self.how {
            How::Saved(age) => note.push_str(&format!("  ·  as it was {}", ago(age))),
            How::Measured if self.took.is_some() => note.push_str(&format!("  ·  measured in {took:.1} s")),
            How::Updated(0) => note.push_str(&format!("  ·  nothing has changed, checked in {took:.1} s")),
            How::Updated(n) => note.push_str(&format!("  ·  brought up to date in {took:.1} s by reading {n} folder{} again", if n == 1 { "" } else { "s" })),
            How::Measured => {}
        }
        if self.running {
            note.push_str(&format!("  ·  checking for changes… {} files read", self.progress.files.load(Ordering::Relaxed)));
        }
        if self.stopped {
            note.push_str("  ·  stopped early, so this is not everything");
        }

        let mut list = column().spacing(1.0).width(Length::Fill);
        for (i, c) in here.children.iter().take(ROWS).enumerate() {
            let share = if here.size > 0 { c.size as f32 / here.size as f32 } else { 0.0 };
            let glyph = if c.group {
                icons::LAYERS
            } else if c.dir {
                icons::FOLDER
            } else {
                icons::FILE
            };
            let size = if c.denied { "can't be read".to_owned() } else { human_size(c.size) };
            let line = row()
                .spacing(8.0)
                .align(Align::Center)
                .width(Length::Fill)
                .push(icon(glyph).size(15.0).tone(if c.dir { Tone::Accent } else { Tone::Muted }))
                .push(text(c.name.clone()).no_wrap().width(Length::Fill))
                .push(text(size).mono().role(TextRole::Caption).tone(if c.denied { Tone::Warn } else { Tone::Muted }))
                .push(text(format!("{:>5.1}%", share * 100.0)).mono().role(TextRole::Caption).tone(Tone::Faint));
            let opens = c.dir && !c.children.is_empty();
            list = list.push(Button::new(column().spacing(5.0).width(Length::Fill).push(line).push(progress_bar(share).height(3.0))).kind(ButtonKind::Ghost).width(Length::Fill).padding([8.0, 7.0]).radius(6.0).on_press_maybe(opens.then_some(Msg::Enter(i))));
        }
        if here.children.len() > ROWS {
            let rest: u64 = here.children[ROWS..].iter().map(|c| c.size).sum();
            list = list.push(container(text(format!("and {} more, {} together", here.children.len() - ROWS, human_size(rest))).role(TextRole::Caption).tone(Tone::Muted)).padding([8.0, 8.0]));
        }
        if here.children.is_empty() {
            list = list.push(container(text(if here.denied { "This folder can't be read." } else { "This folder is empty." }).role(TextRole::Caption).tone(Tone::Muted)).padding([8.0, 8.0]));
        }
        let map = Element::new(Treemap::new(self.map.clone(), here.size).on_open(Msg::Enter));
        column().spacing(8.0).padding(14.0).width(Length::Fill).height(Length::Fill).push(header).push(text(note).role(TextRole::Caption).tone(Tone::Muted)).push(row().spacing(14.0).width(Length::Fill).height(Length::Fill).push(container(scrollable(list)).width(370.0).height(Length::Fill)).push(map)).into()
    }
}

/// A drive's size, file system and whether it is mounted, in a line.
fn summary(d: &Drive) -> String {
    let fs = if d.fs.is_empty() { "not formatted" } else { &d.fs };
    format!("{} · {fs}", human_size(d.size))
}

impl NeoDisk {
    fn drive(&self) -> Option<&Drive> {
        let id = self.chosen.as_deref()?;
        self.drives.as_ref()?.as_ref().ok()?.iter().find(|d| d.id == id)
    }

    /// Looks for drives again, off the main thread where there is one.
    fn find_drives(&mut self) {
        if self.listing {
            return;
        }
        let lister = self.lister.clone();
        match self.proxy.clone() {
            Some(proxy) => {
                self.listing = true;
                std::thread::spawn(move || proxy.send(Msg::Found(lister())));
            }
            None => self.update(Msg::Found(lister())),
        }
    }

    /// Carries out `action` on the drive showing.
    fn act(&mut self, action: Action) {
        let Some(d) = self.drive().cloned() else {
            return;
        };
        if self.busy.is_some() {
            return;
        }
        let title = d.title().to_owned();
        let steps = match drives::commands(&d, &action) {
            Ok(steps) => steps,
            Err(why) => {
                self.outcome = Some(Err(why));
                return;
            }
        };
        let (doing, done, failed) = match &action {
            Action::Mount => (format!("Mounting {title}…"), format!("{title} is mounted."), "mount"),
            Action::Unmount => (format!("Unmounting {title}…"), format!("{title} is unmounted."), "unmount"),
            Action::Eject => (format!("Ejecting {title}…"), format!("{title} can be unplugged now."), "eject"),
            Action::Format { fs, name } => (format!("Formatting {title}…"), format!("{title} was erased. It is now {}, formatted as {}.", fs.label(name).unwrap_or_default(), fs.name()), "format"),
        };
        self.busy = Some(doing);
        self.outcome = None;
        self.form = None;
        let runner = self.runner.clone();
        let work = move || runner(&steps).map(|_| done).map_err(|why| format!("Could not {failed} {title}: {why}"));
        match self.proxy.clone() {
            Some(proxy) => {
                std::thread::spawn(move || proxy.send(Msg::Done(work())));
            }
            None => self.update(Msg::Done(work())),
        }
    }

    /// Whether the form is filled in well enough to go ahead, and if not, why.
    fn form_problem(&self) -> Option<String> {
        let (d, form) = (self.drive()?, self.form.as_ref()?);
        if let Err(why) = form.fs.label(&form.name) {
            return Some(why);
        }
        (form.confirm.trim() != d.title()).then(|| format!("Type {} to confirm.", d.title()))
    }

    fn drive_list(&self, mut side: Column<Msg>) -> Column<Msg> {
        side = side.push(section("Drives"));
        match &self.drives {
            Some(Ok(list)) => {
                for d in list {
                    let state = if d.mount.is_some() { "mounted" } else { "not mounted" };
                    let label = row()
                        .spacing(9.0)
                        .align(Align::Center)
                        .width(Length::Fill)
                        .push(icon(if d.external { icons::USB } else { icons::HARD_DRIVE }).size(16.0))
                        .push(column().spacing(3.0).width(Length::Fill).push(text(d.title().to_owned()).role(TextRole::Strong).no_wrap()).push(text(summary(d)).role(TextRole::Caption).tone(Tone::Muted).no_wrap()).push(text(state).role(TextRole::Caption).tone(if d.mount.is_some() { Tone::Good } else { Tone::Faint })));
                    side = side.push(Button::new(label).kind(ButtonKind::Ghost).selected(self.chosen.as_deref() == Some(d.id.as_str())).width(Length::Fill).align_x(Align::Start).padding([10.0, 9.0]).on_press(Msg::Pick(d.id.clone())));
                }
                if list.is_empty() {
                    side = side.push(container(text("No drives found.").role(TextRole::Caption).tone(Tone::Muted)).padding([10.0, 6.0]));
                }
            }
            Some(Err(_)) => {}
            None => side = side.push(container(text("Looking for drives…").role(TextRole::Caption).tone(Tone::Muted)).padding([10.0, 6.0])),
        }
        side.push(Space::new(0.0, 6.0)).push(nav_item(icons::REFRESH_CW, "Look Again", false, Msg::FindDrives))
    }

    fn drives_view(&self) -> Element<Msg> {
        let message = |glyph, title: &str, more: String| -> Element<Msg> { container(column().spacing(8.0).align(Align::Center).push(icon(glyph).size(42.0).tone(Tone::Faint)).push(text(title.to_owned()).role(TextRole::Title).tone(Tone::Muted)).push(text(more).role(TextRole::Caption).tone(Tone::Faint))).center().width(Length::Fill).height(Length::Fill).into() };
        let d = match (&self.drives, self.drive()) {
            (Some(Err(why)), _) => {
                return message(icons::TRIANGLE_ALERT, "The drives could not be listed", why.clone());
            }
            (None, _) => return message(icons::HARD_DRIVE, "Looking for drives…", String::new()),
            (_, None) => {
                return message(icons::USB, "Choose a drive", "Plug one in, then Look Again, if it is not on the left.".into());
            }
            (_, Some(d)) => d,
        };
        let idle = self.busy.is_none();
        let place = if d.startup {
            "Startup disk"
        } else if d.external {
            "External"
        } else {
            "Built in"
        };
        let header = row().spacing(12.0).align(Align::Center).width(Length::Fill).push(icon(if d.external { icons::USB } else { icons::HARD_DRIVE }).size(34.0).tone(Tone::Accent)).push(column().spacing(3.0).width(Length::Fill).push(text(d.title().to_owned()).role(TextRole::Title).no_wrap()).push(text(format!("{}  ·  {place}", summary(d))).role(TextRole::Caption).tone(Tone::Muted)));
        let fact = |name: &str, value: String| row().spacing(10.0).width(Length::Fill).push(container(text(name.to_owned()).role(TextRole::Caption).tone(Tone::Muted)).width(110.0)).push(text(value).mono().role(TextRole::Caption));
        let mut facts = column().spacing(7.0).width(Length::Fill).push(fact("Identifier", d.id.clone())).push(fact("File system", if d.fs.is_empty() { "None".into() } else { d.fs.clone() })).push(fact("Size", human_size(d.size))).push(fact("Mounted at", d.mount.as_deref().map_or("Not mounted".into(), |m| m.display().to_string())));
        if let Some(disk) = d.mount.as_deref().and_then(|m| self.disks.iter().find(|k| k.mount == m)).filter(|k| k.total > 0) {
            let share = disk.used() as f32 / disk.total as f32;
            facts = facts.push(fact("In use", format!("{} of {}, {} free", human_size(disk.used()), human_size(disk.total), human_size(disk.free)))).push(container(progress_bar(share).height(6.0).tone(if share > 0.9 { Tone::Bad } else { Tone::Accent })).width(420.0));
        }

        let mut actions = row().spacing(8.0).align(Align::Center);
        if d.startup {
            actions = actions.push(text("The disk your computer started from stays mounted.").role(TextRole::Caption).tone(Tone::Muted));
        } else {
            if d.mount.is_some() {
                actions = actions.push(button("Unmount").on_press_maybe(idle.then_some(Msg::Do(Action::Unmount))));
            } else if !d.fs.is_empty() {
                actions = actions.push(button("Mount").kind(ButtonKind::Accent).on_press_maybe(idle.then_some(Msg::Do(Action::Mount))));
            }
            if d.external {
                actions = actions.push(button("Eject").on_press_maybe(idle.then_some(Msg::Do(Action::Eject))));
            }
        }
        if let Some(mount) = d.mount.clone() {
            actions = actions.push(button("See What Is Using It").on_press_maybe(idle.then_some(Msg::Scan(mount))));
        }

        let mut col = column().spacing(16.0).padding(22.0).width(Length::Fill).height(Length::Fill).push(header).push(facts).push(actions);
        if let Some(doing) = &self.busy {
            col = col.push(notice(Tone::Accent, doing.clone()));
        }
        match &self.outcome {
            Some(Ok(said)) => col = col.push(notice(Tone::Good, said.clone())),
            Some(Err(why)) => col = col.push(notice(Tone::Bad, why.clone())),
            None => {}
        }

        // Formatting, apart from the rest and never one click away.
        let mut erase = column().spacing(10.0).width(Length::Fill).push(text("Format").role(TextRole::Strong));
        if let Some(why) = drives::format_refusal(d) {
            erase = erase.push(row().spacing(6.0).align(Align::Center).push(icon(icons::LOCK).size(14.0).tone(Tone::Muted)).push(text(why).role(TextRole::Caption).tone(Tone::Muted)));
        } else if Fs::available().is_empty() {
            erase = erase.push(text("Formatting is not available on this system yet.").role(TextRole::Caption).tone(Tone::Muted));
        } else if let Some(form) = &self.form {
            let kinds = Fs::available();
            let field = |name: String, input: Element<Msg>| row().spacing(10.0).align(Align::Center).push(container(text(name).role(TextRole::Caption).tone(Tone::Muted)).width(110.0)).push(input);
            erase = erase
                .push(notice(Tone::Bad, format!("Everything on {} will be erased. This can't be undone.", d.title())))
                .push(field("File system".into(), segmented(kinds.iter().map(|k| k.name()), kinds.iter().position(|k| *k == form.fs), Msg::FormFs).into()))
                .push(row().spacing(10.0).push(Space::new(110.0, 0.0)).push(container(text(form.fs.note()).role(TextRole::Caption).tone(Tone::Muted)).width(420.0)))
                .push(field("New name".into(), text_input("Untitled", form.name.clone()).on_input(Msg::FormName).width(260.0).into()))
                .push(field(format!("Type {}", d.title()), text_input(d.title().to_owned(), form.confirm.clone()).on_input(Msg::FormConfirm).on_submit(Msg::FormGo).on_cancel(Msg::FormCancel).width(260.0).into()));
            let problem = self.form_problem();
            if let Some(why) = problem.as_ref().filter(|_| !form.confirm.is_empty() || form.fs.label(&form.name).is_err()) {
                erase = erase.push(row().spacing(10.0).push(Space::new(110.0, 0.0)).push(text(why.clone()).role(TextRole::Caption).tone(Tone::Warn)));
            }
            erase = erase.push(row().spacing(8.0).push(Space::new(110.0, 0.0)).push(button("Cancel").on_press(Msg::FormCancel)).push(button("Erase and Format").kind(ButtonKind::Accent).on_press_maybe((problem.is_none() && idle).then_some(Msg::FormGo))));
        } else {
            erase = erase.push(text("Erases the drive and gives it a new, empty file system.").role(TextRole::Caption).tone(Tone::Muted)).push(row().push(button("Format…").on_press_maybe(idle.then_some(Msg::AskFormat))));
        }
        scrollable(col.push(Space::new(0.0, 4.0)).push(erase)).into()
    }
}

impl App for NeoDisk {
    type Message = Msg;

    fn title(&self) -> String {
        match &self.target {
            _ if self.tab == Tab::Drives => "Drives · NeoDisk".into(),
            Some(t) => format!("{} · NeoDisk", t.display()),
            None => "NeoDisk".into(),
        }
    }

    fn window(&self) -> WindowSettings {
        WindowSettings { size: Size::new(1180.0, 740.0), min_size: Some(Size::new(820.0, 480.0)), app_id: Some("org.neo.Disk".into()), ..Default::default() }
    }

    fn app_menu(&self) -> Vec<MenuEntry<Msg>> {
        self.desktop.app_menu(Msg::Desktop)
    }

    fn theme(&self, system: Scheme) -> Theme {
        self.desktop.theme(system)
    }

    fn menus(&self) -> Vec<Menu<Msg>> {
        let showing = self.tree.is_some();
        vec![
            Menu::new("File")
                .push(MenuEntry::new("Measure Folder…", Msg::ChooseFolder).shortcut(Shortcut::command("o")))
                .push(MenuEntry::new("Measure Again", Msg::Rescan).shortcut(Shortcut::command("r")).enabled(self.target.is_some() && !self.running))
                .push(MenuEntry::new("Measure from Scratch", Msg::RescanFully).shortcut(Shortcut::command("r").shift()).enabled(self.target.is_some() && !self.running))
                .separator()
                .push(MenuEntry::new("Show in Files", Msg::Reveal).enabled(showing)),
            Menu::new("View").push(MenuEntry::new("Usage", Msg::Tab(Tab::Usage)).shortcut(Shortcut::command("1"))).push(MenuEntry::new("Drives", Msg::Tab(Tab::Drives)).shortcut(Shortcut::command("2"))).separator().push(MenuEntry::new("Look for Drives Again", Msg::FindDrives).enabled(self.tab == Tab::Drives && !self.listing)),
            Menu::new("Go").push(MenuEntry::new("Up", Msg::Up).shortcut(Shortcut { key: Key::Up, shift: false, alt: false }).enabled(showing && !self.path.is_empty())),
        ]
    }

    fn start(&mut self, proxy: Proxy<Msg>) {
        self.proxy = Some(proxy);
        // A folder given on the command line was waiting for this.
        if let Some(target) = self.target.clone().filter(|_| self.tree.is_none()) {
            self.start(target, false);
        }
    }

    fn subscriptions(&self) -> Vec<Subscription<Msg>> {
        let mut subs = vec![Desktop::subscription(Msg::Poll)];
        if self.running {
            subs.push(Subscription::every(Duration::from_millis(120), Msg::Tick));
        }
        subs
    }

    fn update(&mut self, m: Msg) {
        match m {
            Msg::Scan(target) => {
                self.tab = Tab::Usage;
                self.start(target, false);
            }
            Msg::Tab(tab) => {
                self.tab = tab;
                if tab == Tab::Drives && self.drives.is_none() {
                    self.find_drives();
                }
            }
            Msg::FindDrives => self.find_drives(),
            Msg::Found(found) => {
                self.listing = false;
                // Keep the drive showing if it is still there; otherwise
                // the first that is plugged in, or failing that the first.
                if let Ok(list) = &found
                    && !list.iter().any(|d| Some(d.id.as_str()) == self.chosen.as_deref())
                {
                    self.chosen = list.iter().find(|d| d.external).or(list.first()).map(|d| d.id.clone());
                    self.form = None;
                }
                self.drives = Some(found);
            }
            Msg::Pick(id) => {
                if self.chosen.as_deref() != Some(id.as_str()) {
                    self.chosen = Some(id);
                    self.form = None;
                    self.outcome = None;
                }
            }
            Msg::Do(action) => {
                // Formatting goes through the form and its confirmation only.
                if !matches!(action, Action::Format { .. }) {
                    self.act(action);
                }
            }
            Msg::Done(outcome) => {
                self.busy = None;
                self.outcome = Some(outcome);
                self.disks = disks();
                self.find_drives();
            }
            Msg::AskFormat => {
                if let Some(d) = self.drive().filter(|d| drives::format_refusal(d).is_none())
                    && let Some(fs) = Fs::available().first()
                {
                    self.form = Some(Form { fs: *fs, name: if d.name.is_empty() { "Untitled".into() } else { d.name.clone() }, confirm: String::new() });
                }
            }
            Msg::FormFs(i) => {
                if let (Some(form), Some(fs)) = (&mut self.form, Fs::available().get(i)) {
                    form.fs = *fs;
                }
            }
            Msg::FormName(name) => {
                if let Some(form) = &mut self.form {
                    form.name = name;
                }
            }
            Msg::FormConfirm(typed) => {
                if let Some(form) = &mut self.form {
                    form.confirm = typed;
                }
            }
            Msg::FormCancel => self.form = None,
            Msg::FormGo => {
                if self.form_problem().is_none()
                    && let Some(form) = self.form.clone()
                {
                    self.act(Action::Format { fs: form.fs, name: form.name });
                }
            }
            Msg::ChooseFolder => {
                if let Some(dir) = rfd::FileDialog::new().set_title("Measure Folder").pick_folder() {
                    self.start(dir, false);
                }
            }
            Msg::Scanned(no, tree, how) => {
                // A scan that was replaced by another has nothing to show.
                if no == self.scan_no {
                    // A saved result is shown while the checking goes on.
                    if !matches!(how, How::Saved(_)) {
                        self.stopped = self.progress.cancel.load(Ordering::Relaxed);
                        self.running = false;
                        self.took = Some(self.started.elapsed());
                    }
                    self.how = how;
                    // Stay in the folder showing, as far down as it still goes.
                    let names = self.names();
                    self.tree = Some(tree);
                    let mut path = vec![];
                    let mut at = self.tree.as_deref();
                    for name in names {
                        let Some(i) = at.and_then(|e| e.children.iter().position(|c| c.dir && !c.group && c.name == name && !c.children.is_empty())) else { break };
                        path.push(i);
                        at = at.and_then(|e| e.children.get(i));
                    }
                    self.show(path);
                    // The disk may have changed while it was read.
                    self.disks = disks();
                }
            }
            Msg::Tick => {}
            Msg::Enter(i) => {
                if self.current().and_then(|c| c.children.get(i)).is_some_and(|c| c.dir && !c.children.is_empty()) {
                    let mut path = self.path.clone();
                    path.push(i);
                    self.show(path);
                }
            }
            Msg::Up => {
                let mut path = self.path.clone();
                if path.pop().is_some() {
                    self.show(path);
                }
            }
            Msg::Jump(depth) => {
                let mut path = self.path.clone();
                path.truncate(depth);
                self.show(path);
            }
            Msg::Reveal => {
                if let Some(path) = self.folder_path() {
                    let _ = neo_desktop::fs::reveal(&path);
                }
            }
            Msg::Rescan | Msg::RescanFully => {
                if let Some(target) = self.target.clone().filter(|_| !self.running) {
                    if matches!(m, Msg::RescanFully) {
                        self.tree = None;
                        self.took = None;
                        self.show(vec![]);
                    }
                    self.start(target, matches!(m, Msg::RescanFully));
                }
            }
            Msg::Cancel => self.progress.cancel.store(true, Ordering::Relaxed),
            Msg::Poll => {
                self.desktop.poll();
            }
            Msg::Desktop(m) => {
                self.desktop.update(m);
            }
        }
    }

    fn view(&self) -> Element<Msg> {
        self.desktop.with_settings(self.content(), "NeoDisk Settings", Msg::Desktop, vec![])
    }
}

impl NeoDisk {
    /// The window's content, which the settings panel goes over.
    fn content(&self) -> Element<Msg> {
        let main = if self.tab == Tab::Drives {
            self.drives_view()
        } else if self.running && self.tree.is_none() {
            self.scanning()
        } else {
            match self.current() {
                Some(here) => self.breakdown(here),
                None => self.welcome(),
            }
        };
        split(self.sidebar(), main)
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--snapshot") {
        snapshots(PathBuf::from(args.get(i + 1).cloned().unwrap_or_else(|| "target/snapshots".into())));
        return;
    }
    // Time a scan without opening a window: `neo-disk --measure ~/Downloads`.
    if let Some(i) = args.iter().position(|a| a == "--measure") {
        let root = PathBuf::from(args.get(i + 1).cloned().unwrap_or_else(|| ".".into()));
        let (progress, began) = (Progress::default(), Instant::now());
        let mut index = scan::scan(&root, scan::Options::default(), &progress);
        let tree = index.entry();
        println!("{} files in {} folders, {}, in {:.2} s", tree.files, index.folders(), human_size(tree.size), began.elapsed().as_secs_f32());
        // And again, reading only what has changed since.
        let began = Instant::now();
        let report = scan::refresh(&mut index, &Progress::default());
        println!("brought up to date in {:.2} s: {}", began.elapsed().as_secs_f32(), if report.full { "everything read again".into() } else { format!("{} folders read again", report.reread) });
        // And what keeping it between runs costs.
        let dir = std::env::temp_dir().join(format!("neo-disk-measure-{}", std::process::id()));
        let began = Instant::now();
        if store::save(&dir, &index).is_ok() {
            let size = std::fs::metadata(store::file(&dir, &index.root)).map_or(0, |m| m.len());
            let saved = began.elapsed();
            let began = Instant::now();
            let back = store::load(&dir, &index.root);
            println!("saved as {} in {:.2} s, read back in {:.2} s{}", human_size(size), saved.as_secs_f32(), began.elapsed().as_secs_f32(), if back.as_ref() == Some(&index) { "" } else { ", but not as it was saved" });
        }
        let _ = std::fs::remove_dir_all(dir);
        return;
    }
    let mut app = NeoDisk::new();
    if let Some(path) = args.first() {
        let p = PathBuf::from(path);
        if !p.is_dir() {
            eprintln!("neo-disk: {path} is not a folder");
            std::process::exit(2);
        }
        app.target = Some(p.canonicalize().unwrap_or(p));
    }
    if let Err(e) = neo::run(app) {
        eprintln!("neo-disk: {e}");
        std::process::exit(1);
    }
}

/// A made-up disk to show, so snapshots and tests do not depend on what
/// is on this computer.
fn sample() -> Entry {
    const GB: u64 = 1_000_000_000;
    let file = |name: &str, size: u64| Entry { name: name.into(), size, files: 1, ..Default::default() };
    fn folder(name: &str, children: Vec<Entry>) -> Entry {
        let mut children = children;
        children.sort_by_key(|c| std::cmp::Reverse(c.size));
        Entry { name: name.into(), size: children.iter().map(|c| c.size).sum(), dir: true, files: children.iter().map(|c| c.files).sum(), children, ..Default::default() }
    }
    let small = |n: u64, size: u64| Entry { name: format!("{n} smaller items"), size, files: n, group: true, ..Default::default() };
    folder(
        "home",
        vec![
            folder("Movies", vec![file("Holiday 2025.mov", 38 * GB), file("Wedding.mov", 21 * GB), folder("Clips", vec![file("drone-1.mp4", 6 * GB), file("drone-2.mp4", 5 * GB), file("beach.mp4", 3 * GB), small(48, GB)])]),
            folder("Repositories", vec![folder("neo", vec![folder("target", vec![folder("debug", vec![file("deps", 14 * GB), file("incremental", 6 * GB)]), folder("release", vec![file("deps", 5 * GB)])]), folder("crates", vec![small(310, GB / 4)])]), folder("koka", vec![file("dist-newstyle", 4 * GB), small(2100, GB / 2)])]),
            folder("Pictures", vec![folder("2025", vec![file("raw", 11 * GB), small(3200, 4 * GB)]), folder("2024", vec![small(5400, 9 * GB)])]),
            folder("Library", vec![file("Caches", 9 * GB), file("Mail", 5 * GB), small(41000, 3 * GB)]),
            folder("Downloads", vec![file("ubuntu-26.04.iso", 6 * GB), file("xcode.xip", 8 * GB), small(120, 2 * GB)]),
            file("swapfile", 4 * GB),
            small(900, GB / 5),
        ],
    )
}

/// The app showing the made-up disk.
fn showing_sample() -> NeoDisk {
    let mut app = NeoDisk::new();
    const GB: u64 = 1_000_000_000;
    app.target = Some(PathBuf::from("/Users/sam"));
    app.scan_no = 1;
    app.update(Msg::Scanned(1, Arc::new(sample()), How::Measured));
    app.took = Some(Duration::from_millis(8400));
    // After the result, which re-reads this computer's own disks.
    app.disks = vec![Disk { name: "Macintosh HD".into(), mount: "/".into(), total: 494 * GB, free: 59 * GB, removable: false }, Disk { name: "Backup".into(), mount: "/Volumes/Backup".into(), total: 2000 * GB, free: 1310 * GB, removable: true }];
    app
}

/// Made-up drives, and a record of every command asked for in place of
/// running any.
fn showing_drives() -> (NeoDisk, Arc<std::sync::Mutex<Vec<Vec<String>>>>) {
    const GB: u64 = 1_000_000_000;
    let mac = cfg!(target_os = "macos");
    let id = |n: &str, linux: &str| if mac { n.to_owned() } else { linux.to_owned() };
    let list = vec![
        Drive { id: id("disk3s1s1", "/dev/nvme0n1p2"), parent: id("disk3", "/dev/nvme0n1"), name: "Macintosh HD".into(), size: 494 * GB, fs: "APFS".into(), mount: Some("/".into()), startup: true, ..Default::default() },
        Drive { id: id("disk6s2", "/dev/sdb1"), parent: id("disk6", "/dev/sdb"), name: "HOLIDAY".into(), size: 64 * GB, fs: "ExFAT".into(), mount: Some("/Volumes/HOLIDAY".into()), external: true, ..Default::default() },
        Drive { id: id("disk7s1", "/dev/sdc1"), parent: id("disk7", "/dev/sdc"), name: "Backup".into(), size: 2000 * GB, fs: "APFS".into(), mount: None, external: true, ..Default::default() },
    ];
    let mut app = showing_sample();
    app.disks.push(Disk { name: "HOLIDAY".into(), mount: "/Volumes/HOLIDAY".into(), total: 64 * GB, free: 23 * GB, removable: true });
    let ran = Arc::new(std::sync::Mutex::new(vec![]));
    let record = ran.clone();
    app.runner = Arc::new(move |steps| {
        record.lock().unwrap().extend(steps.iter().cloned());
        Ok(String::new())
    });
    app.lister = Arc::new(move || Ok(list.clone()));
    app.update(Msg::Tab(Tab::Drives));
    (app, ran)
}

fn snapshots(dir: PathBuf) {
    use neo::testing::Harness;
    std::fs::create_dir_all(&dir).expect("create snapshot dir");
    for (name, scheme, into) in [("disk-overview", neo_desktop::SchemePref::Light, None), ("disk-folder", neo_desktop::SchemePref::Dark, Some(1))] {
        let mut app = showing_sample();
        app.desktop.appearance.scheme = scheme;
        if let Some(i) = into {
            app.update(Msg::Enter(i));
        }
        let mut h = Harness::new(app, Size::new(1180.0, 740.0)).expect("GPU");
        h.move_to(neo::Point::new(820.0, 420.0));
        let path = dir.join(format!("{name}.png"));
        h.save_png(&path, 1.0).expect("write png");
        println!("wrote {}", path.display());
    }
    for (name, scheme, form) in [("disk-drives", neo_desktop::SchemePref::Light, false), ("disk-format", neo_desktop::SchemePref::Dark, true)] {
        let (mut app, _) = showing_drives();
        app.desktop.appearance.scheme = scheme;
        if form {
            app.update(Msg::AskFormat);
            app.update(Msg::FormName("Trip 2026".into()));
            app.update(Msg::FormConfirm("HOLI".into()));
        }
        let mut h = Harness::new(app, Size::new(1180.0, 740.0)).expect("GPU");
        let path = dir.join(format!("{name}.png"));
        h.save_png(&path, 1.0).expect("write png");
        println!("wrote {}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neo::Point;
    use neo::testing::Harness;

    const GB: u64 = 1_000_000_000;

    #[test]
    fn disks_are_the_ones_a_person_would_name() {
        if cfg!(target_os = "macos") {
            assert!(is_a_disk(Path::new("/")) && is_a_disk(Path::new("/Volumes/Backup")));
            assert!(!is_a_disk(Path::new("/System/Volumes/Data")), "part of the startup disk, mounted again");
            assert!(!is_a_disk(Path::new("/System/Volumes/VM")));
        } else if cfg!(unix) {
            assert!(is_a_disk(Path::new("/")) && is_a_disk(Path::new("/home")) && is_a_disk(Path::new("/mnt/data")));
            assert!(!is_a_disk(Path::new("/boot/efi")) && !is_a_disk(Path::new("/snap/core/1")) && !is_a_disk(Path::new("/run/user/1000")));
            assert!(is_a_disk(Path::new("/bootstrap")), "only /boot itself and what is under it");
        }
        // This computer has at least one, with room accounted for.
        let found = disks();
        assert!(!found.is_empty());
        assert!(found.iter().all(|d| d.total > 0 && d.used() <= d.total));
    }

    #[test]
    fn going_into_folders_and_back() {
        let mut app = showing_sample();
        assert_eq!(app.current().unwrap().name, "home");
        assert_eq!(app.current().unwrap().children[0].name, "Movies", "largest first");
        app.update(Msg::Enter(0));
        assert_eq!((app.current().unwrap().name.as_str(), app.folder_path().unwrap()), ("Movies", PathBuf::from("/Users/sam/Movies")));
        // A file is not somewhere to go.
        app.update(Msg::Enter(0));
        assert_eq!(app.current().unwrap().name, "Movies");
        app.update(Msg::Enter(2));
        assert_eq!(app.folder_path().unwrap(), PathBuf::from("/Users/sam/Movies/Clips"));
        assert_eq!(app.map.len(), 4, "the map follows: three clips and the small things");
        app.update(Msg::Up);
        assert_eq!(app.current().unwrap().name, "Movies");
        app.update(Msg::Enter(2));
        app.update(Msg::Jump(0));
        assert_eq!((app.current().unwrap().name.as_str(), app.path.len()), ("home", 0));
        app.update(Msg::Up);
        assert_eq!(app.current().unwrap().name, "home", "nowhere above the top");
        // An index that is not there does nothing.
        app.update(Msg::Enter(99));
        assert!(app.path.is_empty());
    }

    #[test]
    fn a_real_folder_is_measured_and_shown() {
        let root = std::env::temp_dir().join(format!("neo-disk-app-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("big")).unwrap();
        std::fs::write(root.join("big/data.bin"), vec![1u8; 3_000_000]).unwrap();
        std::fs::write(root.join("note.txt"), vec![1u8; 400_000]).unwrap();
        let mut app = NeoDisk::new();
        app.update(Msg::Scan(root.clone()));
        assert!(!app.running, "with no event loop the measuring is done at once");
        let here = app.current().expect("a result");
        assert_eq!(here.files, 2);
        assert!(here.size >= 3_400_000);
        assert_eq!(here.children.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["big", "note.txt"]);
        assert_eq!(app.title(), format!("{} · NeoDisk", root.display()));
        // Measuring again stays in the folder showing, and reads little.
        app.update(Msg::Enter(0));
        app.update(Msg::Rescan);
        assert_eq!(app.current().unwrap().name, "big");
        assert!(matches!(app.how, How::Updated(_)), "{:?}", app.how);
        // From scratch reads everything, and starts over from the top.
        app.update(Msg::RescanFully);
        assert!(app.path.is_empty() && app.current().is_some());
        assert_eq!(app.how, How::Measured);
        // A scan that another replaced is ignored when it reports.
        let stale = app.scan_no - 1;
        app.update(Msg::Scanned(stale, Arc::new(Entry { name: "old".into(), ..Default::default() }), How::Measured));
        assert_ne!(app.current().unwrap().name, "old");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_result_is_saved_and_shown_again_next_time() {
        let root = std::env::temp_dir().join(format!("neo-disk-saved-{}", std::process::id()));
        let cache = std::env::temp_dir().join(format!("neo-disk-saved-cache-{}", std::process::id()));
        for dir in [&root, &cache] {
            let _ = std::fs::remove_dir_all(dir);
        }
        std::fs::create_dir_all(root.join("big")).unwrap();
        std::fs::write(root.join("big/data.bin"), vec![1u8; 3_000_000]).unwrap();
        let mut first = NeoDisk::new();
        assert!(first.cache.is_none(), "tests keep nothing unless they say where");
        first.cache = Some(cache.clone());
        first.update(Msg::Scan(root.clone()));
        assert_eq!(first.how, How::Measured);
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 1, "the index was saved");
        let size = first.current().unwrap().size;

        // Another run: what was saved is found, and only checked.
        let mut second = NeoDisk::new();
        second.cache = Some(cache.clone());
        let saved = store::load(&cache, &root.canonicalize().unwrap()).expect("an index to start from");
        assert_eq!(saved.entry().size, size);
        second.update(Msg::Scan(root.clone()));
        assert!(matches!(second.how, How::Updated(_)), "brought up to date, not measured again: {:?}", second.how);
        assert_eq!(second.current().unwrap().size, size);
        // A saved result arriving is shown, with the checking still going on.
        second.running = true;
        let no = second.scan_no;
        second.update(Msg::Scanned(no, Arc::new(saved.entry()), How::Saved(7200)));
        assert!(second.running && second.current().is_some());
        assert_eq!(ago(7200), "2 hours ago");
        assert_eq!((ago(5), ago(60), ago(200_000)), ("a moment ago".into(), "1 minute ago".into(), "2 days ago".into()));
        // A scan that was stopped is not saved over a whole one.
        let before = std::fs::read(store::file(&cache, &root.canonicalize().unwrap())).unwrap();
        second.running = false;
        second.progress.cancel.store(true, Ordering::Relaxed);
        let job = Job { no: 99, target: root.clone(), index: second.index.clone(), cache: Some(cache.clone()), progress: second.progress.clone(), fully: true };
        job.run(&mut |_| {});
        assert_eq!(std::fs::read(store::file(&cache, &root.canonicalize().unwrap())).unwrap(), before);
        for dir in [&root, &cache] {
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn stopping_a_scan_keeps_what_was_found_and_says_so() {
        let mut app = showing_sample();
        app.running = true;
        app.update(Msg::Cancel);
        assert!(app.progress.cancel.load(Ordering::Relaxed));
        let no = app.scan_no;
        app.update(Msg::Scanned(no, Arc::new(sample()), How::Measured));
        assert!(app.stopped && !app.running && app.current().is_some());
    }

    #[test]
    fn the_list_and_the_map_both_open_folders() {
        let mut h = Harness::new(showing_sample(), Size::new(1180.0, 740.0)).unwrap();
        h.render(1.0);
        // The first row of the list: Movies.
        h.click(Point::new(360.0, 152.0));
        assert_eq!(h.app().current().unwrap().name, "Movies");
        h.app_mut().update(Msg::Jump(0));
        h.render(1.0);
        // The top-left block of the map is the largest thing: Movies again.
        h.click(Point::new(660.0, 200.0));
        assert_eq!(h.app().current().unwrap().name, "Movies");
        // Pointing at a block names it under the map.
        h.app_mut().update(Msg::Jump(0));
        let before = h.render(1.0);
        h.move_to(Point::new(900.0, 400.0));
        assert!(h.render(1.0) != before);
    }

    fn stick() -> &'static str {
        if cfg!(target_os = "macos") { "disk6s2" } else { "/dev/sdb1" }
    }

    #[test]
    fn the_drives_tab_lists_drives_and_shows_the_one_plugged_in() {
        let mut app = NeoDisk::new();
        assert_eq!(app.tab, Tab::Usage);
        assert!(app.drives.is_none(), "drives are not looked for until the tab is opened");
        let (mut app2, ran) = showing_drives();
        std::mem::swap(&mut app, &mut app2);
        assert_eq!(app.tab, Tab::Drives);
        assert_eq!(app.drive().unwrap().name, "HOLIDAY", "the first external drive, not the startup disk");
        app.update(Msg::Pick(app.drives.clone().unwrap().unwrap()[2].id.clone()));
        assert_eq!(app.drive().unwrap().name, "Backup");
        // A drive that has gone is not left showing.
        let rest = vec![app.drives.clone().unwrap().unwrap()[0].clone()];
        app.lister = Arc::new(move || Ok(rest.clone()));
        app.update(Msg::FindDrives);
        assert!(app.drive().unwrap().startup);
        // Looking failed: say so, and nothing is chosen.
        app.lister = Arc::new(|| Err("no lsblk".into()));
        app.update(Msg::FindDrives);
        assert!(app.drive().is_none() && app.drives == Some(Err("no lsblk".into())));
        // Measuring a drive goes back to the usage tab.
        app.update(Msg::Scan(std::env::temp_dir()));
        assert_eq!(app.tab, Tab::Usage);
        assert!(ran.lock().unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn mounting_unmounting_and_ejecting_run_the_systems_commands() {
        let (mut app, ran) = showing_drives();
        app.update(Msg::Do(Action::Unmount));
        assert_eq!(ran.lock().unwrap().last().unwrap()[..2], if cfg!(target_os = "macos") { ["diskutil", "unmount"] } else { ["udisksctl", "unmount"] });
        assert_eq!(app.outcome, Some(Ok("HOLIDAY is unmounted.".into())));
        assert!(app.busy.is_none());
        app.update(Msg::Do(Action::Eject));
        assert_eq!(app.outcome, Some(Ok("HOLIDAY can be unplugged now.".into())));
        // A failure is reported as what the command said.
        app.runner = Arc::new(|_| Err("the volume is in use".into()));
        app.update(Msg::Do(Action::Unmount));
        assert_eq!(app.outcome, Some(Err("Could not unmount HOLIDAY: the volume is in use".into())));
        // The startup disk is left alone whatever is asked.
        let before = ran.lock().unwrap().len();
        let start = app.drives.clone().unwrap().unwrap()[0].id.clone();
        app.update(Msg::Pick(start));
        app.update(Msg::AskFormat);
        assert!(app.form.is_none(), "no form for the startup disk");
        app.update(Msg::FormGo);
        app.update(Msg::Do(Action::Format { fs: Fs::ExFat, name: "x".into() }));
        assert_eq!(ran.lock().unwrap().len(), before);
    }

    #[cfg(unix)]
    #[test]
    fn formatting_needs_the_drives_name_typed_out() {
        let (mut app, ran) = showing_drives();
        // Never straight from a message, only through the form.
        app.update(Msg::Do(Action::Format { fs: Fs::ExFat, name: "x".into() }));
        app.update(Msg::FormGo);
        assert!(ran.lock().unwrap().is_empty());
        app.update(Msg::AskFormat);
        assert_eq!(app.form.as_ref().unwrap().name, "HOLIDAY", "it starts with the name it has");
        app.update(Msg::FormName("Trip 2026".into()));
        for typed in ["", "holiday", "HOLIDA", "Backup"] {
            app.update(Msg::FormConfirm(typed.into()));
            app.update(Msg::FormGo);
            assert!(ran.lock().unwrap().is_empty(), "{typed:?} is not the drive's name");
            assert!(app.form.is_some());
        }
        // The right name, but a new name the file system can't take.
        app.update(Msg::FormConfirm("HOLIDAY".into()));
        app.update(Msg::FormName("a/b".into()));
        assert!(app.form_problem().is_some());
        app.update(Msg::FormGo);
        assert!(ran.lock().unwrap().is_empty());
        // Choosing another drive throws the form away, typed name and all.
        let other = app.drives.clone().unwrap().unwrap()[2].id.clone();
        app.update(Msg::Pick(other));
        assert!(app.form.is_none());
        app.update(Msg::Pick(stick().into()));
        app.update(Msg::FormGo);
        assert!(ran.lock().unwrap().is_empty());
        // Filled in properly, it goes ahead, on the drive showing.
        app.update(Msg::AskFormat);
        app.update(Msg::FormFs(1));
        app.update(Msg::FormName("camera".into()));
        app.update(Msg::FormConfirm("HOLIDAY".into()));
        assert_eq!(app.form_problem(), None);
        app.update(Msg::FormGo);
        let ran = ran.lock().unwrap();
        let last = ran.last().unwrap();
        assert_eq!(last.last().unwrap(), stick());
        assert!(last.contains(&"CAMERA".to_owned()), "{last:?}");
        assert!(app.form.is_none());
        assert_eq!(app.outcome, Some(Ok("HOLIDAY was erased. It is now CAMERA, formatted as FAT32.".into())));
    }

    #[test]
    fn the_drives_tab_is_reached_and_worked_by_clicking() {
        let (mut app, ran) = showing_drives();
        app.update(Msg::Tab(Tab::Usage));
        let mut h = Harness::new(app, Size::new(1180.0, 740.0)).unwrap();
        let usage = h.render(1.0);
        // The right half of the switch at the top of the sidebar.
        h.click(Point::new(190.0, 66.0));
        assert_eq!(h.app().tab, Tab::Drives);
        assert!(h.render(1.0) != usage);
        // The form shows once asked for, and changes the picture.
        let plain = h.render(1.0);
        h.app_mut().update(Msg::AskFormat);
        assert!(h.render(1.0) != plain);
        assert!(ran.lock().unwrap().is_empty(), "looking and asking run nothing");
    }

    #[test]
    fn the_sidebar_starts_a_scan_of_a_disk_and_menus_follow_the_state() {
        let app = NeoDisk::new();
        let entry = |app: &NeoDisk, menu: usize, label: &str| app.menus()[menu].entries.iter().find(|e| e.label == label).unwrap().message.is_some();
        assert!(!entry(&app, 0, "Measure Again") && !entry(&app, 0, "Show in Files") && !entry(&app, 2, "Up"));
        let mut app = showing_sample();
        assert!(entry(&app, 0, "Measure Again") && entry(&app, 0, "Show in Files"));
        assert!(!entry(&app, 2, "Up"), "already at the top");
        app.update(Msg::Enter(0));
        assert!(entry(&app, 2, "Up"));
        let used = app.disks[0].used();
        assert_eq!(used, 435 * GB);
    }
}
