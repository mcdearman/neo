//! NeoCode: a small code editor built with Neo, with optional Vim or
//! Helix keys.
//!
//!     cargo run -p neo-code                  # sample project
//!     cargo run -p neo-code -- path/to/dir   # a real folder (Cmd/Ctrl+S saves)
//!     cargo run -p neo-code -- --snapshot target/snapshots

// Release builds on Windows open no console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::time::Duration;

mod intel;
mod lsp;

use neo::prelude::*;
use neo::{Key, KeyEvent, Proxy, Size};
use neo_desktop::Desktop;

// ---------------------------------------------------------------------------
// Sample project, shown when no folder is given.

const SAMPLE: &[(&str, &str)] = &[
    ("Cargo.toml", r#"[package]
name = "aurora"
version = "0.3.0"
edition = "2024"
description = "Sunrise and daylight calculator"

[dependencies]
chrono = { version = "0.4", default-features = false, features = ["clock"] }
"#),
    ("README.md", r#"# Aurora

Prints sunrise, sunset and day length for any latitude.

## Usage

- `aurora 51.48` for Greenwich
- `aurora --json 64.13` for Reykjavík, as JSON

See [the NOAA solar calculator](https://gml.noaa.gov/grad/solcalc/) for the maths.
"#),
    ("src/main.rs", r##"//! Aurora: sunrise, sunset and day length for a latitude.

mod solar;

use std::env;
use std::process::ExitCode;

use solar::{Day, Latitude};

/// Output format chosen on the command line.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Format {
    Text,
    Json,
}

fn parse_args<'a>(args: &'a [String]) -> Result<(Format, &'a str), String> {
    match args {
        [flag, lat] if flag == "--json" => Ok((Format::Json, lat)),
        [lat] => Ok((Format::Text, lat)),
        _ => Err("usage: aurora [--json] <latitude>".to_string()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let (format, raw) = match parse_args(&args) {
        Ok(parsed) => parsed,
        Err(usage) => {
            eprintln!("{usage}");
            return ExitCode::from(2);
        }
    };

    let Some(lat) = raw.parse::<f64>().ok().and_then(Latitude::new) else {
        eprintln!("latitude must be a number between -90 and 90");
        return ExitCode::FAILURE;
    };

    /* Day 172 is the June solstice in a common year. */
    let day = Day::of_year(172);
    let hours = solar::day_length(lat, day);

    match format {
        Format::Text => println!("{:.1} hours of daylight at {lat}", hours),
        Format::Json => println!(r#"{{"latitude": {}, "hours": {:.2}}}"#, lat.degrees(), hours),
    }
    ExitCode::SUCCESS
}
"##),
    ("src/solar.rs", r#"use std::f64::consts::PI;
use std::fmt;

/// A latitude in degrees, north positive.
#[derive(Clone, Copy, Debug)]
pub struct Latitude(f64);

impl Latitude {
    pub fn new(degrees: f64) -> Option<Self> {
        (-90.0..=90.0).contains(&degrees).then_some(Self(degrees))
    }

    pub fn degrees(self) -> f64 {
        self.0
    }
}

impl fmt::Display for Latitude {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let hemisphere = if self.0 >= 0.0 { 'N' } else { 'S' };
        write!(f, "{:.2}°{hemisphere}", self.0.abs())
    }
}

/// Day of the year, 1 to 366.
#[derive(Clone, Copy, Debug)]
pub struct Day(u16);

impl Day {
    pub fn of_year(n: u16) -> Self {
        Self(n.clamp(1, 366))
    }
}

/// Hours between sunrise and sunset, using the solar declination.
pub fn day_length(lat: Latitude, day: Day) -> f64 {
    let declination = 23.44_f64.to_radians() * (2.0 * PI * (284.0 + day.0 as f64) / 365.0).sin();
    let phi = lat.degrees().to_radians();
    let cos_h = -phi.tan() * declination.tan();
    match cos_h {
        c if c <= -1.0 => 24.0, // midnight sun
        c if c >= 1.0 => 0.0,   // polar night
        c => 2.0 * c.acos().to_degrees() / 15.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equator_is_about_twelve_hours() {
        let h = day_length(Latitude::new(0.0).unwrap(), Day::of_year(80));
        assert!((h - 12.0).abs() < 0.1);
    }
}
"#),
];

// ---------------------------------------------------------------------------

/// The key sets on offer, in the order the status bar lists them.
const KEYMAPS: [(Keymap, &str); 3] = [(Keymap::Plain, "Standard"), (Keymap::Vim, "Vim"), (Keymap::Helix, "Helix")];

#[derive(Clone)]
enum Source {
    Memory(&'static str),
    Disk(PathBuf),
}

#[derive(Clone)]
struct Node {
    name: String,
    path: String,
    depth: usize,
    dir: bool,
    expanded: bool,
    source: Option<Source>,
}

struct Tab {
    path: String,
    name: String,
    doc: Document,
    saved: u64,
    language: Language,
    disk: Option<PathBuf>,
    /// The language server looking after this file, by name.
    server: Option<&'static str>,
    /// How many versions of the text the server has been sent, and the
    /// document revision of the last.
    version: i64,
    synced: u64,
    /// The problems the server has found.
    diagnostics: Vec<lsp::Diagnostic>,
    /// What the server says each stretch of the text is, for colouring.
    tokens: std::rc::Rc<[EditorToken]>,
    popup: Option<intel::Popup>,
}

impl Tab {
    fn dirty(&self) -> bool {
        self.doc.revision() != self.saved
    }
}

struct NeoCode {
    project: String,
    nodes: Vec<Node>,
    tabs: Vec<Tab>,
    active: Option<usize>,
    dark: Option<bool>,
    toast: Option<String>,
    keymap: Keymap,
    /// The folder that is open, when the project is on disk.
    root: Option<PathBuf>,
    desktop: Desktop,
    servers: intel::Servers,
}

#[derive(Clone, Debug)]
enum Msg {
    Open(usize),
    ToggleDir(usize),
    Select(usize),
    Close(usize),
    Edit(Action),
    Save,
    ToggleScheme,
    ClearToast,
    Poll,
    /// The Settings entry and panel every Neo app has.
    Desktop(neo_desktop::DesktopMsg),
    /// The keys to edit with: an index into [`KEYMAPS`].
    Keys(usize),
    /// Ask which folder to open.
    OpenFolder,
    Folder(PathBuf),
    /// Files or folders dropped on the window.
    Dropped(Vec<PathBuf>),
    /// The folders to look for language servers in have been worked out.
    LspDirs(Vec<PathBuf>),
    /// Word from a language server: its name, which launch of it, and what.
    Lsp(&'static str, u64, lsp::Incoming),
    /// Ask the language server about the caret's position.
    Ask(intel::Ask),
    PopupKey(PopupKey),
}

impl NeoCode {
    fn sample() -> Self {
        let mut nodes = vec![];
        let mut dirs_seen: Vec<String> = vec![];
        let mut files: Vec<&(&str, &str)> = SAMPLE.iter().collect();
        files.sort_by_key(|(p, _)| (!p.contains('/'), *p));
        for (path, text) in files {
            let parts: Vec<&str> = path.split('/').collect();
            for d in 0..parts.len() - 1 {
                let dir = parts[..=d].join("/");
                if !dirs_seen.contains(&dir) {
                    nodes.push(Node { name: parts[d].into(), path: dir.clone(), depth: d, dir: true, expanded: true, source: None });
                    dirs_seen.push(dir);
                }
            }
            nodes.push(Node { name: parts[parts.len() - 1].into(), path: path.to_string(), depth: parts.len() - 1, dir: false, expanded: false, source: Some(Source::Memory(text)) });
        }
        let mut app = Self { project: "aurora".into(), nodes, tabs: vec![], active: None, dark: None, toast: None, keymap: Keymap::Vim, root: None, desktop: Desktop::load(), servers: intel::Servers::default() };
        for p in ["src/main.rs", "src/solar.rs", "Cargo.toml"] {
            if let Some(i) = app.nodes.iter().position(|n| n.path == p) {
                app.update(Msg::Open(i));
            }
        }
        app.update(Msg::Select(0));
        app
    }

    /// The name and file tree of the folder at `root`.
    fn load(root: &Path) -> (String, Vec<Node>) {
        fn walk(dir: &Path, rel: &str, depth: usize, out: &mut Vec<Node>) {
            if depth > 6 || out.len() > 4000 {
                return;
            }
            let Ok(read) = std::fs::read_dir(dir) else { return };
            let mut entries: Vec<_> = read.flatten().collect();
            entries.sort_by_key(|e| (!e.path().is_dir(), e.file_name().to_string_lossy().to_lowercase()));
            for e in entries {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') || name == "target" || name == "node_modules" {
                    continue;
                }
                let path = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
                let is_dir = e.path().is_dir();
                out.push(Node { name, path: path.clone(), depth, dir: is_dir, expanded: depth == 0 && is_dir, source: (!is_dir).then(|| Source::Disk(e.path())) });
                if is_dir {
                    walk(&e.path(), &path, depth + 1, out);
                }
            }
        }
        let mut nodes = vec![];
        walk(root, "", 0, &mut nodes);
        let project = root.canonicalize().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_else(|| root.display().to_string());
        (project, nodes)
    }

    fn folder(root: &Path) -> Self {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let (project, nodes) = Self::load(&root);
        Self { project, nodes, tabs: vec![], active: None, dark: None, toast: None, keymap: Keymap::Vim, root: Some(root), desktop: Desktop::load(), servers: intel::Servers::default() }
    }

    /// The folder a file belongs to: the nearest one above it that looks
    /// like a project, or failing that the one it is in.
    fn project_of(file: &Path) -> PathBuf {
        let parent = file.parent().unwrap_or(file);
        const MARKERS: [&str; 7] = [".git", "Cargo.toml", "package.json", "go.mod", "pyproject.toml", "build.zig", "compile_commands.json"];
        // Stop short of the home folder and the root, which are not projects.
        let home = std::env::var_os("HOME").map(PathBuf::from);
        parent.ancestors().take_while(|d| Some(*d) != home.as_deref() && d.parent().is_some()).find(|d| MARKERS.iter().any(|m| d.join(m).exists())).unwrap_or(parent).to_path_buf()
    }

    /// Starts on a folder, or on a file with its project open around it.
    fn at(path: &Path) -> Self {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if path.is_dir() {
            return Self::folder(&path);
        }
        let mut app = Self::folder(&Self::project_of(&path));
        if let Some(i) = app.nodes.iter().position(|n| matches!(&n.source, Some(Source::Disk(p)) if *p == path)) {
            // Show where it is in the tree, then open it.
            let mut depth = app.nodes[i].depth;
            for j in (0..i).rev() {
                if app.nodes[j].dir && app.nodes[j].depth < depth {
                    app.nodes[j].expanded = true;
                    depth = app.nodes[j].depth;
                }
            }
            app.update(Msg::Open(i));
        }
        app
    }

    /// Where the folder last opened is noted, to come back to on the next launch.
    fn last_folder_file() -> PathBuf {
        neo_desktop::config_dir().join("apps").join("neo-code-folder")
    }

    fn remember(dir: &Path) {
        // Tests open folders too, and must not change what the app comes back to.
        if cfg!(test) {
            return;
        }
        let file = Self::last_folder_file();
        if let Some(parent) = file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(file, dir.to_string_lossy().as_bytes());
    }

    /// The folder open when NeoCode was last used, if it is still there.
    fn last_folder() -> Option<PathBuf> {
        let dir = PathBuf::from(std::fs::read_to_string(Self::last_folder_file()).ok()?.trim());
        dir.is_dir().then_some(dir)
    }

    /// Replaces the project with the folder at `dir`. Refused while files
    /// have unsaved changes, which would otherwise be lost.
    fn open_folder(&mut self, dir: &Path) {
        if self.tabs.iter().any(|t| t.dirty()) {
            self.toast = Some("Save or close your changed files before opening another folder.".into());
            return;
        }
        let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
        if !dir.is_dir() {
            self.toast = Some(format!("{} is not a folder.", dir.display()));
            return;
        }
        let (project, nodes) = Self::load(&dir);
        self.toast = Some(format!("Opened {project}"));
        self.project = project;
        self.nodes = nodes;
        self.tabs.clear();
        self.active = None;
        Self::remember(&dir);
        self.root = Some(dir);
        // The servers were started for the old folder.
        self.servers.reset();
    }

    /// Opens what was dropped on the window: a folder as the project, or a
    /// file in a tab, switching to the file's folder if it is not in this one.
    fn open_dropped(&mut self, path: &Path) {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if path.is_dir() {
            return self.open_folder(&path);
        }
        let node = |app: &Self| app.nodes.iter().position(|n| matches!(&n.source, Some(Source::Disk(p)) if *p == path));
        if node(self).is_none() {
            self.open_folder(&Self::project_of(&path));
        }
        if let Some(i) = node(self) {
            self.update(Msg::Open(i));
        }
    }

    fn visible_nodes(&self) -> Vec<usize> {
        let mut out = vec![];
        let mut hide_below: Option<usize> = None;
        for (i, n) in self.nodes.iter().enumerate() {
            if let Some(d) = hide_below {
                if n.depth > d {
                    continue;
                }
                hide_below = None;
            }
            out.push(i);
            if n.dir && !n.expanded {
                hide_below = Some(n.depth);
            }
        }
        out
    }

    fn active_tab(&self) -> Option<&Tab> {
        self.active.and_then(|i| self.tabs.get(i))
    }
}

fn file_icon(name: &str) -> neo::theme::Icon {
    match Language::from_path(name) {
        Language::Rust => icons::FILE_CODE,
        Language::Toml => icons::FILE_COG,
        Language::Markdown => icons::FILE_TEXT,
        Language::Plain => icons::FILE,
    }
}

impl NeoCode {
    /// Whether the window is dark: the override if set, else the desktop setting.
    fn is_dark(&self) -> bool {
        self.dark.unwrap_or(self.desktop.appearance.scheme == neo_desktop::SchemePref::Dark)
    }
}

impl App for NeoCode {
    type Message = Msg;

    fn title(&self) -> String {
        match self.active_tab() {
            Some(t) => format!("{}{} · {}", if t.dirty() { "• " } else { "" }, t.name, self.project),
            None => format!("NeoCode · {}", self.project),
        }
    }

    fn window(&self) -> WindowSettings {
        WindowSettings { size: Size::new(1280.0, 820.0), app_id: Some("org.neo.Code".into()), ..Default::default() }
    }

    fn app_menu(&self) -> Vec<MenuEntry<Msg>> {
        self.desktop.app_menu(Msg::Desktop)
    }

    fn theme(&self, system: Scheme) -> Theme {
        let mut theme = self.desktop.theme(system);
        // The sun and moon button overrides the desktop's scheme for this window.
        match self.dark {
            Some(true) => theme.scheme = Scheme::Dark,
            Some(false) => theme.scheme = Scheme::Light,
            None => {}
        }
        theme
    }

    fn start(&mut self, proxy: Proxy<Msg>) {
        self.find_servers(proxy);
    }

    fn subscriptions(&self) -> Vec<Subscription<Msg>> {
        let mut subs = vec![Desktop::subscription(Msg::Poll)];
        if self.toast.is_some() {
            subs.push(Subscription::every(Duration::from_secs(3), Msg::ClearToast));
        }
        subs
    }

    fn menus(&self) -> Vec<Menu<Msg>> {
        let file_open = self.active_tab().is_some();
        // Whether a language server is looking after the file in front.
        let served = self.active_tab().and_then(|t| t.server).is_some_and(|s| self.servers.clients.contains_key(s));
        let keys = KEYMAPS.iter().enumerate().map(|(i, (k, name))| MenuEntry::new(format!("{}{name} Keys", if *k == self.keymap { "✓ " } else { "" }), Msg::Keys(i)));
        vec![
            Menu::new("File")
                .push(MenuEntry::new("Open Folder…", Msg::OpenFolder).shortcut(Shortcut::command("o")))
                .separator()
                .push(MenuEntry::new("Save", Msg::Save).shortcut(Shortcut::command("s")).enabled(file_open))
                .push(MenuEntry::new("Close Tab", Msg::Close(self.active.unwrap_or(0))).shortcut(Shortcut::command("w")).enabled(file_open)),
            Menu::new("Edit")
                .push(MenuEntry::new("Undo", Msg::Edit(Action::Undo)).shortcut(Shortcut::command("z")).enabled(file_open))
                .push(MenuEntry::new("Redo", Msg::Edit(Action::Redo)).shortcut(Shortcut::command("z").shift()).enabled(file_open))
                .separator()
                .push(MenuEntry::new("Select All", Msg::Edit(Action::SelectAll)).shortcut(Shortcut::command("a")).enabled(file_open))
                .separator()
                .push(MenuEntry::new("Format Document", Msg::Ask(intel::Ask::Format)).shortcut(Shortcut::command("l").shift()).enabled(served)),
            Menu::new("Go")
                .push(MenuEntry::new("Show Hover", Msg::Ask(intel::Ask::Hover)).shortcut(Shortcut::command("i")).enabled(served))
                .push(MenuEntry::new("Go to Definition", Msg::Ask(intel::Ask::Definition)).shortcut(Shortcut::command("g")).enabled(served))
                .push(MenuEntry::new("Complete", Msg::Ask(intel::Ask::Complete)).shortcut(Shortcut::command(".")).enabled(served)),
            keys.fold(Menu::new("View").push(MenuEntry::new(if self.is_dark() { "Light Appearance" } else { "Dark Appearance" }, Msg::ToggleScheme)).separator(), Menu::push),
        ]
    }

    fn on_key(&self, k: &KeyEvent) -> Option<Msg> {
        if !k.modifiers.command() {
            return None;
        }
        // Save, close and open are menu entries; their shortcuts live there.
        match &k.key {
            Key::Character(c) => c.parse::<usize>().ok().filter(|n| (1..=self.tabs.len()).contains(n)).map(|n| Msg::Select(n - 1)),
            _ => None,
        }
    }

    fn update(&mut self, m: Msg) {
        match m {
            Msg::Open(i) => {
                let node = self.nodes[i].clone();
                if let Some(t) = self.tabs.iter().position(|t| t.path == node.path) {
                    self.active = Some(t);
                    return;
                }
                let (text, disk) = match &node.source {
                    Some(Source::Memory(t)) => (t.to_string(), None),
                    Some(Source::Disk(p)) => match std::fs::read(p).map(String::from_utf8) {
                        Ok(Ok(t)) if t.len() < 2_000_000 => (t, Some(p.clone())),
                        Ok(Ok(_)) => {
                            self.toast = Some(format!("{} is too large to open.", node.name));
                            return;
                        }
                        Ok(Err(_)) => {
                            self.toast = Some(format!("{} isn't a text file.", node.name));
                            return;
                        }
                        Err(e) => {
                            self.toast = Some(format!("Couldn't open {}: {e}", node.name));
                            return;
                        }
                    },
                    None => return,
                };
                let mut doc = Document::new(&text);
                doc.set_keymap(self.keymap);
                self.tabs.push(Tab { saved: doc.revision(), doc, language: Language::from_path(&node.name), name: node.name, path: node.path, disk, server: None, version: 0, synced: 0, diagnostics: vec![], tokens: std::rc::Rc::from([]), popup: None });
                self.active = Some(self.tabs.len() - 1);
                self.lsp_open(self.tabs.len() - 1);
            }
            Msg::ToggleDir(i) => self.nodes[i].expanded = !self.nodes[i].expanded,
            Msg::Select(i) => {
                if i < self.tabs.len() {
                    self.active = Some(i)
                }
            }
            Msg::Close(i) => {
                if i < self.tabs.len() {
                    self.lsp_closing(i);
                    self.tabs.remove(i);
                    self.active = if self.tabs.is_empty() {
                        None
                    } else {
                        self.active.map(|a| if a > i || a >= self.tabs.len() { a.saturating_sub(1) } else { a })
                    };
                }
            }
            Msg::Edit(a) => {
                let Some(i) = self.active else { return };
                let Some(t) = self.tabs.get_mut(i) else { return };
                // Text added on the caret's line is typing, which is when
                // completions are wanted.
                let shape = |t: &Tab| (t.doc.line_count(), t.doc.cursor().line, t.doc.lines()[t.doc.cursor().line].len());
                let (before, revision) = (shape(t), t.doc.revision());
                t.doc.apply(a);
                let after = shape(t);
                let typed = t.doc.revision() != revision && (before.0, before.1) == (after.0, after.1) && after.2 > before.2;
                if t.doc.revision() != revision && !t.tokens.is_empty() {
                    // The server's colours for the changed line are stale, and
                    // if lines came or went so are those below. The built-in
                    // highlighter covers for them until fresh ones arrive.
                    let (from, lines_moved) = (before.1.min(after.1), before.0 != after.0);
                    t.tokens = t.tokens.iter().filter(|k| k.line < from || (!lines_moved && k.line > from)).copied().collect();
                }
                self.after_edit(i, typed);
                let Some(t) = self.tabs.get_mut(i) else { return };
                for r in t.doc.take_vim_requests() {
                    match r {
                        VimRequest::Write => self.update(Msg::Save),
                        VimRequest::WriteQuit => {
                            self.update(Msg::Save);
                            if self.tabs.get(i).is_some_and(|t| !t.dirty()) {
                                self.update(Msg::Close(i));
                            }
                        }
                        VimRequest::Quit { force } => {
                            if !force && self.tabs.get(i).is_some_and(Tab::dirty) {
                                self.toast = Some("No write since last change. Use :q! to discard it.".into());
                            } else {
                                self.update(Msg::Close(i));
                            }
                        }
                    }
                }
            }
            Msg::OpenFolder => {
                if let Some(dir) = rfd::FileDialog::new().set_title("Open Folder").pick_folder() {
                    self.update(Msg::Folder(dir));
                }
            }
            Msg::Folder(dir) => self.open_folder(&dir),
            Msg::Dropped(paths) => {
                if let Some(first) = paths.first() {
                    self.open_dropped(first);
                }
            }
            Msg::LspDirs(dirs) => self.servers_found(dirs),
            Msg::Lsp(name, launch, incoming) => self.lsp_incoming(name, launch, incoming),
            Msg::Ask(what) => self.ask(what),
            Msg::PopupKey(key) => self.popup_key(key),
            Msg::Keys(i) => {
                self.keymap = KEYMAPS.get(i).map_or(Keymap::Plain, |(k, _)| *k);
                for t in &mut self.tabs {
                    t.doc.set_keymap(self.keymap);
                }
            }
            Msg::Save => {
                let Some(t) = self.active.and_then(|i| self.tabs.get_mut(i)) else { return };
                let result = match &t.disk {
                    Some(p) => std::fs::write(p, t.doc.text() + "\n").map(|_| format!("Saved {}", t.name)),
                    None => Ok(format!("Saved {} (sample project, kept in memory)", t.name)),
                };
                match result {
                    Ok(msg) => {
                        t.saved = t.doc.revision();
                        self.toast = Some(msg);
                        if let Some(i) = self.active {
                            self.lsp_saved(i);
                        }
                    }
                    Err(e) => self.toast = Some(format!("Couldn't save {}: {e}", t.name)),
                }
            }
            Msg::ToggleScheme => {
                self.dark = Some(!self.is_dark());
            }
            Msg::ClearToast => self.toast = None,
            Msg::Desktop(m) => {
                self.desktop.update(m);
            }
            Msg::Poll => {
                self.desktop.poll();
            }
        }
    }

    fn view(&self) -> Element<Msg> {
        self.desktop.with_settings(self.content(), "NeoCode Settings", Msg::Desktop, self.settings_rows())
    }
}

impl NeoCode {
    /// The window's content, which the settings panel goes over.
    fn content(&self) -> Element<Msg> {
        // Edge to edge, as in most code editors: the explorer, the editor
        // under a thin strip of tabs, and a thin status bar.
        let body = row().width(Length::Fill).height(Length::Fill).push(self.sidebar()).push(Divider::vertical()).push(self.main());
        let all = column().width(Length::Fill).height(Length::Fill).push(body).push(Divider::horizontal()).push(self.status_bar());
        mouse_area(all).on_drop(Msg::Dropped).into()
    }
}

impl NeoCode {
    fn sidebar(&self) -> Element<Msg> {
        let active_path = self.active_tab().map(|t| t.path.clone());
        let mut list = column().spacing(1.0).width(Length::Fill);
        for i in self.visible_nodes() {
            let n = &self.nodes[i];
            let glyph = if n.dir {
                if n.expanded { icons::CHEVRON_DOWN } else { icons::CHEVRON_RIGHT }
            } else {
                file_icon(&n.name)
            };
            let tone = if n.dir { Tone::Muted } else { Tone::Accent };
            let label = row()
                .spacing(8.0)
                .align(Align::Center)
                .push(Space::new(n.depth as f32 * 14.0, 0.0))
                .push(icon(glyph).size(14.0).tone(tone))
                .push(text(n.name.clone()).role(if n.dir { TextRole::Strong } else { TextRole::Body }).no_wrap());
            let msg = if n.dir { Msg::ToggleDir(i) } else { Msg::Open(i) };
            list = list.push(
                Button::new(label)
                    .kind(ButtonKind::Ghost)
                    .selected(!n.dir && active_path.as_deref() == Some(n.path.as_str()))
                    .align_x(Align::Start)
                    .width(Length::Fill)
                    .padding([8.0, 4.0])
                    .radius(6.0)
                    .on_press(msg)
                    .into_element_keyed(&n.path),
            );
        }
        let header = column()
            .spacing(6.0)
            .width(Length::Fill)
            .push(text("Explorer").role(TextRole::Label).tone(Tone::Muted))
            .push(text(self.project.clone()).role(TextRole::Strong).no_wrap());
        container(column().spacing(8.0).width(Length::Fill).height(Length::Fill).push(header).push(scrollable(list))).padding([12.0, 8.0, 0.0, 8.0]).width(240.0).height(Length::Fill).into()
    }

    fn main(&self) -> Element<Msg> {
        // The card fill, which is translucent on a glass window, so what is
        // behind the window still shows through the editor, blurred.
        let surface = self.theme(if self.is_dark() { Scheme::Dark } else { Scheme::Light }).paint(Surface::Card).fill;
        let mut tabs = row().spacing(2.0).align(Align::Center);
        for (i, t) in self.tabs.iter().enumerate() {
            let active = self.active == Some(i);
            let label = row()
                .spacing(6.0)
                .align(Align::Center)
                .push(icon(file_icon(&t.name)).size(13.0).tone(Tone::Accent))
                .push(text(t.name.clone()).role(TextRole::Body).no_wrap())
                .push_if(t.dirty(), || text("●").role(TextRole::Caption).tone(Tone::Accent).into());
            tabs = tabs.push(
                row()
                    .align(Align::Center)
                    .push(Button::new(label).kind(ButtonKind::Ghost).selected(active).padding([8.0, 4.0]).radius(6.0).on_press(Msg::Select(i)))
                    .push(icon_button(icons::X, 20.0).kind(ButtonKind::Ghost).on_press(Msg::Close(i))),
            );
        }
        let tabs = container(tabs).padding([6.0, 0.0]).height(34.0).width(Length::Fill).align_y(Align::Center);

        let editor: Element<Msg> = match self.active_tab() {
            Some(t) => container(text_editor(&t.doc).language(t.language).on_action(Msg::Edit).marks(self.editor_marks(t)).tokens(t.tokens.clone()).popup(self.editor_popup(t)).on_popup_key(Msg::PopupKey).into_element_keyed(&t.path))
                .background(Background::Color(surface))
                .width(Length::Fill)
                .height(Length::Fill)
                .into(),
            None => container(
                column()
                    .spacing(8.0)
                    .align(Align::Center)
                    .push(icon(icons::FILE_CODE).size(40.0).tone(Tone::Faint))
                    .push(text("Open a file from the Explorer").role(TextRole::Title).tone(Tone::Muted))
                    .push(text("File ▸ Open Folder… opens a folder, or drop one on the window. Language servers start for files in a folder.").role(TextRole::Caption).tone(Tone::Faint))
                    .push(text("Cmd/Ctrl+S saves · Cmd/Ctrl+W closes · Cmd/Ctrl+1–9 switches tabs · :w and :q work with Vim and Helix keys").role(TextRole::Caption).tone(Tone::Faint)),
            )
            .background(Background::Color(surface))
            .center()
            .width(Length::Fill)
            .height(Length::Fill)
            .into(),
        };
        column().width(Length::Fill).height(Length::Fill).push(tabs).push(Divider::horizontal()).push(editor).into()
    }

    /// Code's own rows for the settings panel.
    fn settings_rows(&self) -> Vec<Element<Msg>> {
        let keys = segmented(KEYMAPS.map(|(_, name)| name), KEYMAPS.iter().position(|(k, _)| *k == self.keymap), Msg::Keys);
        vec![
            neo_desktop::ui::setting("Keys", "Standard editing, or Vim or Helix modal keys.", keys),
            neo_desktop::ui::setting("Language servers", &self.servers_summary(), Space::new(0.0, 0.0)),
        ]
    }

    fn status_bar(&self) -> Element<Msg> {
        let mut left = row().spacing(18.0).align(Align::Center);
        if let Some(v) = self.active_tab().and_then(|t| t.doc.mode_status()) {
            let insert = v.insert;
            let badge = container(text(v.label).role(TextRole::Label))
                .padding([8.0, 2.0])
                .radius(5.0)
                .background(if insert { Background::Surface(Surface::Accent) } else { Background::Surface(Surface::Pressed) });
            left = left.push(badge);
            if let Some(cmd) = v.command_line {
                left = left.push(text(format!("{cmd}▏")).mono().role(TextRole::Body));
            } else if !v.pending.is_empty() {
                left = left.push(text(v.pending).mono().role(TextRole::Caption).tone(Tone::Accent));
            }
            if let Some(r) = v.recording {
                left = left.push(row().spacing(6.0).align(Align::Center).push(icon(icons::CIRCLE).size(10.0).tone(Tone::Bad)).push(text(format!("recording @{r}")).role(TextRole::Caption).tone(Tone::Bad)));
            }
            if v.selections > 1 {
                left = left.push(text(format!("{} selections", v.selections)).mono().role(TextRole::Caption).tone(Tone::Accent));
            }
            if let Some(m) = v.message.filter(|m| !m.starts_with("recording @")) {
                let tone = if m.starts_with("search hit") || m.ends_with("yanked") || m.ends_with("fewer lines") { Tone::Muted } else { Tone::Warn };
                left = left.push(text(m).role(TextRole::Caption).tone(tone));
            }
        }
        if let Some(t) = self.active_tab() {
            let c = t.doc.cursor();
            let col = t.doc.lines()[c.line][..c.col].chars().count() + 1;
            left = left.push(text(format!("Ln {}, Col {}", c.line + 1, col)).mono().role(TextRole::Caption).tone(Tone::Muted));
            let selected = t.doc.selected_text();
            if !selected.is_empty() {
                let n = selected.chars().filter(|c| *c != '\n').count();
                left = left.push(text(format!("{n} selected")).mono().role(TextRole::Caption).tone(Tone::Muted));
            }
            let count = |s: lsp::Severity| t.diagnostics.iter().filter(|d| d.severity == s).count();
            for (n, glyph, tone) in [(count(lsp::Severity::Error), icons::CIRCLE_X, Tone::Bad), (count(lsp::Severity::Warning), icons::TRIANGLE_ALERT, Tone::Warn)] {
                if n > 0 {
                    left = left.push(row().spacing(4.0).align(Align::Center).push(icon(glyph).size(13.0).tone(tone)).push(text(n.to_string()).mono().role(TextRole::Caption).tone(tone)));
                }
            }
            if let Some(d) = self.problem_here(t) {
                let first = d.message.lines().next().unwrap_or_default();
                let tone = if d.severity == lsp::Severity::Error { Tone::Bad } else { Tone::Warn };
                left = left.push(text(first.chars().take(90).collect::<String>()).role(TextRole::Caption).tone(tone).no_wrap());
            }
            left = left
                .push(text(t.language.name()).role(TextRole::Caption).tone(Tone::Muted))
                .push(text(format!("Spaces: {INDENT}")).role(TextRole::Caption).tone(Tone::Muted))
                .push(text("UTF-8").role(TextRole::Caption).tone(Tone::Muted))
                .push(text(format!("{} lines", t.doc.line_count())).role(TextRole::Caption).tone(Tone::Muted));
        }
        if let Some(msg) = &self.toast {
            left = left.push(row().spacing(6.0).align(Align::Center).push(icon(icons::CIRCLE_CHECK).size(14.0).tone(Tone::Good)).push(text(msg.clone()).role(TextRole::Caption).tone(Tone::Good)));
        }
        let dark = self.is_dark();
        let keys = KEYMAPS.iter().position(|(k, _)| *k == self.keymap).unwrap_or(0);
        let server = self.active_tab().and_then(|t| self.server_status(t));
        let bar = row()
            .width(Length::Fill)
            .align(Align::Center)
            .spacing(12.0)
            .push(left)
            .push(Space::fill_x())
            .push_if(server.is_some(), || {
                let (label, tone) = server.clone().unwrap_or_default();
                text(label).role(TextRole::Caption).tone(tone).no_wrap().into()
            })
            // Click to go on to the next set of keys.
            .push(Button::new(text(format!("{} keys", KEYMAPS[keys].1)).role(TextRole::Caption)).kind(ButtonKind::Ghost).padding([8.0, 3.0]).radius(5.0).on_press(Msg::Keys((keys + 1) % KEYMAPS.len())))
            .push(icon_button(if dark { icons::SUN } else { icons::MOON }, 24.0).kind(ButtonKind::Ghost).on_press(Msg::ToggleScheme));
        container(bar).padding([10.0, 0.0]).height(30.0).width(Length::Fill).align_y(Align::Center).into()
    }
}

trait Keyed<M> {
    fn into_element_keyed(self, key: &str) -> Element<M>;
}

impl<M: 'static, T: Into<Element<M>>> Keyed<M> for T {
    fn into_element_keyed(self, key: &str) -> Element<M> {
        self.into().key(key)
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--snapshot") {
        snapshots(PathBuf::from(args.get(i + 1).cloned().unwrap_or_else(|| "target/snapshots".into())));
        return;
    }
    let app = match args.first() {
        Some(path) => {
            let p = PathBuf::from(path);
            if !p.exists() {
                eprintln!("neo-code: {path} does not exist");
                std::process::exit(2);
            }
            let app = NeoCode::at(&p);
            if let Some(root) = &app.root {
                NeoCode::remember(root);
            }
            app
        }
        // Back to the folder from last time; the sample is for a first look.
        None => NeoCode::last_folder().map_or_else(NeoCode::sample, |dir| NeoCode::folder(&dir)),
    };
    if let Err(e) = neo::run(app) {
        eprintln!("neo-code: {e}");
        std::process::exit(1);
    }
}

fn snapshots(dir: PathBuf) {
    use neo::testing::Harness;
    use neo::{Modifiers, Point};
    std::fs::create_dir_all(&dir).expect("create snapshot dir");
    for (name, dark) in [("editor-light", false), ("editor-dark", true), ("editor-insert", false), ("editor-recording", true), ("editor-helix", true), ("editor-menu", false), ("editor-settings", false)] {
        let mut app = NeoCode::sample();
        app.dark = Some(dark);
        if name == "editor-helix" {
            app.update(Msg::Keys(2));
        }
        let mut h = Harness::new(app, Size::new(1280.0, 820.0)).expect("GPU");
        // Focus the editor, then use Vim keys: move down, select to the end of a word.
        h.click(Point::new(700.0, 300.0));
        match name {
            "editor-light" => {
                // Visual Block over three lines.
                h.type_text("7jw");
                h.key(Key::Character("v".into()), Modifiers { ctrl: true, ..Default::default() });
                h.type_text("2je");
            }
            "editor-dark" => h.type_text("jVj"),
            "editor-insert" => h.type_text("jA // edited"),
            // Helix: three lines selected, with a pending text-object command.
            // The File menu open, as Windows and Linux show it; macOS puts
            // the menus in its own bar at the top of the screen.
            "editor-menu" => {
                h.click(Point::new(24.0, 23.0));
                h.move_to(Point::new(60.0, 110.0));
            }
            "editor-settings" => h.app_mut().update(Msg::Desktop(neo_desktop::DesktopMsg::OpenSettings)),
            // Helix: a cursor on each of four lines, mid-way through typing.
            "editor-helix" => h.type_text("12GwCCCi"),
            _ => h.type_text("qad2"),
        }
        let path = dir.join(format!("{name}.png"));
        h.save_png(&path, 1.0).expect("write png");
        println!("wrote {}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use neo::testing::Harness;
    use neo::{Modifiers, Point};

    use super::*;

    /// The sample project with the editor focused.
    fn editing(keys: usize) -> Harness<NeoCode> {
        let mut app = NeoCode::sample();
        app.update(Msg::Keys(keys));
        let mut h = Harness::new(app, Size::new(1280.0, 820.0)).expect("a GPU adapter is required for these tests");
        h.click(Point::new(700.0, 300.0));
        h
    }

    fn lines(h: &Harness<NeoCode>) -> Vec<String> {
        h.app().active_tab().expect("the sample opens a file").doc.lines().to_vec()
    }

    fn label(h: &Harness<NeoCode>) -> Option<&'static str> {
        h.app().active_tab().and_then(|t| t.doc.mode_status()).map(|s| s.label)
    }

    #[test]
    fn helix_keys_select_then_act_in_the_editor() {
        let mut h = editing(2);
        assert_eq!(label(&h), Some("NOR"));
        let before = lines(&h);
        // Top of the file, select two lines, delete them.
        h.type_text("ggxxd");
        assert_eq!(lines(&h), before[2..]);
        h.type_text("u");
        assert_eq!(lines(&h), before);
        // Insert mode types text and Escape leaves it.
        h.type_text("ggi// ");
        assert_eq!(label(&h), Some("INS"));
        h.key(Key::Escape, Modifiers::default());
        assert_eq!(label(&h), Some("NOR"));
        assert_eq!(lines(&h)[0], format!("// {}", before[0]));
    }

    #[test]
    fn the_same_keys_mean_different_things_in_each_keymap() {
        // `x` deletes a character in Vim, selects the line in Helix, and is
        // typed as text with standard keys.
        let first = |h: &Harness<NeoCode>| lines(h)[0].clone();
        let original = first(&editing(0));

        let mut vim = editing(1);
        assert_eq!(label(&vim), Some("NORMAL"));
        vim.type_text("ggx");
        assert_eq!(first(&vim), original[1..]);

        let mut helix = editing(2);
        helix.type_text("ggx");
        assert_eq!(first(&helix), original);
        assert_eq!(helix.app().active_tab().unwrap().doc.selected_text(), format!("{original}\n"));

        let mut plain = editing(0);
        assert_eq!(label(&plain), None);
        plain.type_text("x");
        assert!(lines(&plain).iter().any(|l| l.contains('x')) && lines(&plain) != lines(&editing(0)));
    }

    #[test]
    fn helix_cursors_on_several_lines_type_together() {
        let mut h = editing(2);
        let before = lines(&h);
        h.type_text("ggCCi// ");
        h.key(Key::Escape, Modifiers::default());
        let now = lines(&h);
        for i in 0..3 {
            assert_eq!(now[i], format!("// {}", before[i]), "line {i}");
        }
        assert_eq!(now[3], before[3]);
        assert_eq!(h.app().active_tab().unwrap().doc.mode_status().unwrap().selections, 3);
        // And it draws: three carets is a different picture from one.
        let three = h.render(1.0);
        h.type_text(",");
        assert_ne!(three, h.render(1.0));
    }

    /// A folder in the scratch space of the test run, with two files.
    fn project(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("neo-code-test-{}-{name}", std::process::id())).join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/lib.rs"), "pub fn one() {}\n").unwrap();
        std::fs::write(dir.join("notes.md"), "# Notes\n").unwrap();
        dir
    }

    #[test]
    fn opening_a_folder_replaces_the_project() {
        let dir = project("open-folder");
        let mut app = NeoCode::sample();
        assert!(!app.tabs.is_empty());
        app.update(Msg::Folder(dir.clone()));
        assert_eq!(app.project, "open-folder");
        assert!(app.tabs.is_empty(), "the old project's tabs are closed");
        let names: Vec<&str> = app.nodes.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, ["src", "lib.rs", "notes.md"], "folders first, then files");
        let lib = app.nodes.iter().position(|n| n.name == "lib.rs").unwrap();
        app.update(Msg::Open(lib));
        assert_eq!(app.active_tab().unwrap().doc.text(), "pub fn one() {}\n");
        // Something that is not a folder is refused with a message.
        app.update(Msg::Folder(dir.join("notes.md")));
        assert_eq!(app.project, "open-folder");
        assert!(app.toast.as_deref().unwrap().ends_with("is not a folder."));
    }

    #[test]
    fn unsaved_changes_block_opening_another_folder() {
        let dir = project("unsaved-guard");
        let mut app = NeoCode::sample();
        app.update(Msg::Keys(0));
        app.update(Msg::Edit(Action::Insert("x".into())));
        assert!(app.active_tab().unwrap().dirty());
        app.update(Msg::Folder(dir));
        assert_eq!(app.project, "aurora", "still the sample project");
        assert!(app.toast.as_deref().unwrap().starts_with("Save or close"));
    }

    #[test]
    fn dropping_a_folder_or_a_file_opens_it() {
        let dir = project("dropped");
        let mut app = NeoCode::sample();
        app.update(Msg::Dropped(vec![dir.clone()]));
        assert_eq!(app.project, "dropped");
        assert!(app.tabs.is_empty());
        // A file inside the open folder just opens.
        app.update(Msg::Dropped(vec![dir.join("notes.md")]));
        assert_eq!((app.project.as_str(), app.active_tab().unwrap().name.as_str()), ("dropped", "notes.md"));
        // A file from elsewhere brings its folder with it.
        let other = project("dropped-elsewhere");
        app.update(Msg::Dropped(vec![other.join("src/lib.rs")]));
        assert_eq!((app.project.as_str(), app.active_tab().unwrap().name.as_str()), ("src", "lib.rs"));
    }

    #[test]
    fn the_shortcut_and_the_status_bar_reach_the_new_commands() {
        let app = NeoCode::sample();
        let cmd = Modifiers { logo: cfg!(target_os = "macos"), ctrl: !cfg!(target_os = "macos"), ..Default::default() };
        let key = KeyEvent { key: Key::Character("o".into()), pressed: true, repeat: false, modifiers: cmd, text: None };
        let menus = app.menus();
        let titles: Vec<&str> = menus.iter().map(|m| m.title.as_str()).collect();
        assert_eq!(titles, ["File", "Edit", "Go", "View"]);
        let open = &menus[0].entries[0];
        assert!(matches!(open.message, Some(Msg::OpenFolder)) && open.shortcut.as_ref().unwrap().matches(&key));
        // The current keys are ticked in the View menu.
        assert!(menus[3].entries.iter().any(|e| e.label == "✓ Vim Keys"));

        // Save and Close need a file; they grey out without one.
        let labelled = |app: &NeoCode, label: &str| app.menus()[0].entries.iter().find(|e| e.label == label).unwrap().message.is_some();
        assert!(labelled(&app, "Save") && labelled(&app, "Close Tab"));
        let mut empty = NeoCode::sample();
        while let Some(i) = empty.active {
            empty.update(Msg::Close(i));
        }
        assert!(!labelled(&empty, "Save") && !labelled(&empty, "Close Tab"));
    }

    #[test]
    fn menu_shortcuts_save_and_close_through_the_window() {
        let cmd = Modifiers { logo: cfg!(target_os = "macos"), ctrl: !cfg!(target_os = "macos"), ..Default::default() };
        let mut h = editing(0);
        h.type_text("x");
        assert!(h.app().active_tab().unwrap().dirty());
        h.key(Key::Character("s".into()), cmd);
        assert!(!h.app().active_tab().unwrap().dirty(), "saved by the File menu's shortcut");
        let tabs = h.app().tabs.len();
        h.key(Key::Character("w".into()), cmd);
        assert_eq!(h.app().tabs.len(), tabs - 1);
        // And by choosing the entry: File, then the fourth row down.
        h.click(Point::new(24.0, 23.0));
        h.click(Point::new(60.0, 44.0 + 32.0 + 9.0 + 32.0 + 16.0));
        assert_eq!(h.app().tabs.len(), tabs - 2);
    }

    /// The sample project with its own settings kept in a scratch file, so
    /// tests never touch the real ones.
    fn with_scratch_settings(name: &str) -> NeoCode {
        let mut app = NeoCode::sample();
        let path = std::env::temp_dir().join(format!("neo-code-test-{}-{name}", std::process::id())).join("app.conf");
        let _ = std::fs::remove_file(&path);
        app.desktop = Desktop::with_prefs_file(path);
        app
    }

    #[test]
    fn settings_open_from_the_menu_shortcut_and_block_the_editor() {
        let cmd = Modifiers { logo: cfg!(target_os = "macos"), ctrl: !cfg!(target_os = "macos"), ..Default::default() };
        let mut app = with_scratch_settings("panel");
        app.update(Msg::Keys(0));
        let mut h = Harness::new(app, Size::new(1280.0, 820.0)).unwrap();
        h.click(Point::new(700.0, 300.0));
        let before = lines(&h);
        h.key(Key::Character(",".into()), cmd);
        assert!(h.app().desktop.settings_open);
        h.type_text("zzz");
        assert_eq!(lines(&h), before, "typing does not reach the file behind the panel");
        h.click(Point::new(700.0, 700.0));
        assert!(!h.app().desktop.settings_open, "a click outside the panel closes it");
        h.key(Key::Character(",".into()), cmd);
        h.key(Key::Escape, Modifiers::default());
        assert!(!h.app().desktop.settings_open, "and so does Escape");
    }

    #[test]
    fn this_app_can_turn_glass_off_and_the_editor_follows() {
        let mut app = with_scratch_settings("glass");
        app.desktop.appearance.glass.enabled = true;
        app.desktop.appearance.glass.opacity = 0.6;
        let mut h = Harness::new(app, Size::new(1280.0, 820.0)).unwrap();
        // The editor area, right of the text: its opacity in the rendered frame.
        let alpha = |h: &mut Harness<NeoCode>| h.render(1.0)[(600 * 1280 + 1100) * 4 + 3];
        assert!(h.theme().glass.enabled);
        let glass = alpha(&mut h);
        assert!(glass < 250, "the editor area is see-through on a glass window, got {glass}");
        h.app_mut().update(Msg::Desktop(neo_desktop::DesktopMsg::Glass(false)));
        assert_eq!(alpha(&mut h), 255, "solid once this app opts out");
        assert!(!h.theme().glass.enabled);
        assert!(h.app().desktop.appearance.glass.enabled, "and only this app changed");
    }

    use crate::lsp::testing::Sink;
    use crate::lsp::{Client, Incoming};
    use serde_json::{json, Value};

    /// A project on disk with `src/lib.rs` open, looked after by a stand-in
    /// for rust-analyzer whose side of the conversation the test plays.
    struct Served {
        h: Harness<NeoCode>,
        sink: Sink,
        file: PathBuf,
    }

    impl Served {
        fn new(name: &str, source: &str) -> Self {
            let dir = project(name);
            std::fs::write(dir.join("src/lib.rs"), source).unwrap();
            let mut app = NeoCode::folder(&dir);
            app.update(Msg::Keys(0));
            let root = app.root.clone().unwrap();
            let sink = Sink::default();
            app.servers.clients.insert("rust-analyzer", (1, Client::new(Box::new(sink.clone()), &root)));
            let h = Harness::new(app, Size::new(1280.0, 820.0)).unwrap();
            let mut s = Self { h, sink, file: root.join("src/lib.rs") };
            // The handshake: answer `initialize`.
            let hello = s.sink.take();
            let caps = json!({ "positionEncoding": "utf-8", "documentFormattingProvider": true, "completionProvider": { "triggerCharacters": ["."] }, "semanticTokensProvider": { "legend": { "tokenTypes": ["function", "keyword", "type"] }, "full": true } });
            s.says(json!({ "id": hello[0]["id"], "result": { "capabilities": caps } }));
            let lib = s.h.app().nodes.iter().position(|n| n.name == "lib.rs").unwrap();
            s.h.app_mut().update(Msg::Open(lib));
            s.h.click(Point::new(700.0, 300.0));
            s
        }

        /// The server sends a message.
        fn says(&mut self, message: Value) {
            self.h.app_mut().update(Msg::Lsp("rust-analyzer", 1, Incoming::Message(message)));
        }

        /// What the app has sent the server since the last call, by method.
        fn sent(&self) -> Vec<Value> {
            self.sink.take()
        }

        fn methods(&self) -> Vec<String> {
            self.sent().iter().map(|m| m["method"].as_str().unwrap_or("reply").to_owned()).collect()
        }

        fn uri(&self) -> String {
            crate::lsp::client::uri(&self.file)
        }

        fn tab(&self) -> &Tab {
            self.h.app().active_tab().unwrap()
        }

        /// The request with this method among what was just sent.
        fn request(&self, method: &str) -> Value {
            self.sent().into_iter().rfind(|m| m["method"] == method).unwrap_or_else(|| panic!("no {method} was sent"))
        }
    }

    #[test]
    fn opening_and_editing_a_file_keeps_its_server_up_to_date() {
        let mut s = Served::new("lsp-sync", "pub fn one() {}\n");
        let opened = s.sent();
        let open = opened.iter().find(|m| m["method"] == "textDocument/didOpen").expect("the file is opened with the server");
        assert_eq!(open["params"]["textDocument"], json!({ "uri": s.uri(), "languageId": "rust", "version": 1, "text": "pub fn one() {}\n" }));
        assert!(opened.iter().any(|m| m["method"] == "textDocument/semanticTokens/full"), "and its colours are asked for");

        s.h.app_mut().update(Msg::Edit(Action::Click { pos: Pos::new(0, 0), select: false }));
        assert!(s.sent().is_empty(), "moving the caret changes nothing for the server");
        s.h.type_text("x");
        let sent = s.sent();
        let change = sent.iter().find(|m| m["method"] == "textDocument/didChange").unwrap();
        assert_eq!(change["params"]["textDocument"]["version"], 2);
        assert_eq!(change["params"]["contentChanges"][0]["text"], "xpub fn one() {}\n");
        assert!(sent.iter().any(|m| m["method"] == "textDocument/completion"), "typing a word asks for completions");

        let cmd = Modifiers { logo: cfg!(target_os = "macos"), ctrl: !cfg!(target_os = "macos"), ..Default::default() };
        s.h.key(Key::Character("s".into()), cmd);
        assert!(s.methods().contains(&"textDocument/didSave".to_owned()));
        s.h.key(Key::Character("w".into()), cmd);
        assert_eq!(s.methods(), ["textDocument/didClose"]);
    }

    #[test]
    fn problems_are_underlined_counted_and_described() {
        let mut s = Served::new("lsp-problems", "fn one() -> u32 {\n    \"x\"\n}\n");
        let clean = s.h.render(1.0);
        let uri = s.uri();
        s.says(json!({ "method": "textDocument/publishDiagnostics", "params": { "uri": uri, "diagnostics": [
            { "range": { "start": { "line": 1, "character": 4 }, "end": { "line": 1, "character": 7 } }, "severity": 1, "message": "mismatched types\nexpected u32" },
            { "range": { "start": { "line": 0, "character": 3 }, "end": { "line": 0, "character": 6 } }, "severity": 2, "message": "unused" },
        ] } }));
        assert_eq!(s.tab().diagnostics.len(), 2);
        let marks = s.h.app().editor_marks(s.tab());
        assert_eq!((marks[0].from, marks[0].to, marks[0].tone), (Pos::new(1, 4), Pos::new(1, 7), Tone::Bad));
        assert_eq!(marks[1].tone, Tone::Warn);
        assert!(s.h.render(1.0) != clean, "they show in the window");
        // The status bar describes the one on the caret's line.
        s.h.app_mut().update(Msg::Edit(Action::Click { pos: Pos::new(1, 5), select: false }));
        assert_eq!(s.h.app().problem_here(s.tab()).unwrap().message, "mismatched types\nexpected u32");
        s.h.app_mut().update(Msg::Edit(Action::Click { pos: Pos::new(2, 0), select: false }));
        assert!(s.h.app().problem_here(s.tab()).is_none());
        // An empty list clears them.
        s.says(json!({ "method": "textDocument/publishDiagnostics", "params": { "uri": uri, "diagnostics": [] } }));
        assert!(s.tab().diagnostics.is_empty());
    }

    #[test]
    fn completions_narrow_as_you_type_and_replace_the_word() {
        let mut s = Served::new("lsp-complete", "fn f() {\n    \n}\n");
        s.h.app_mut().update(Msg::Edit(Action::Click { pos: Pos::new(1, 4), select: false }));
        s.sent();
        s.h.type_text("pr");
        let ask = s.request("textDocument/completion");
        assert_eq!(ask["params"]["position"], json!({ "line": 1, "character": 6 }));
        s.says(json!({ "id": ask["id"], "result": [{ "label": "println!", "detail": "macro" }, { "label": "print!" }, { "label": "eprintln!" }, { "label": "format!" }] }));
        let Some(EditorPopup::List { items, selected }) = s.h.app().editor_popup(s.tab()) else { panic!("a list of completions") };
        let labels: Vec<&str> = items.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!((labels, selected), (vec!["print!", "println!", "eprintln!"], 0), "those starting with what was typed, in the server's order, then those containing it");
        assert_eq!(items[1].1.as_deref(), Some("macro"));

        // Down, then Enter: the arrow goes to the list, not the text.
        s.h.key(Key::Down, Modifiers::default());
        s.h.key(Key::Enter, Modifiers::default());
        assert_eq!(s.tab().doc.lines()[1], "    println!", "the word typed so far is replaced by the choice");
        assert!(s.h.app().editor_popup(s.tab()).is_none());

        // Escape dismisses without changing the text.
        s.h.type_text("f");
        let ask = s.request("textDocument/completion");
        s.says(json!({ "id": ask["id"], "result": [{ "label": "format!" }] }));
        assert!(s.h.app().editor_popup(s.tab()).is_some());
        s.h.key(Key::Escape, Modifiers::default());
        assert!(s.h.app().editor_popup(s.tab()).is_none());
        assert_eq!(s.tab().doc.lines()[1], "    println!f");
    }

    #[test]
    fn hover_shows_at_the_caret_until_the_next_key() {
        let mut s = Served::new("lsp-hover", "pub fn one() {}\n");
        s.h.app_mut().update(Msg::Edit(Action::Click { pos: Pos::new(0, 8), select: false }));
        s.sent();
        let cmd = Modifiers { logo: cfg!(target_os = "macos"), ctrl: !cfg!(target_os = "macos"), ..Default::default() };
        s.h.key(Key::Character("i".into()), cmd);
        let ask = s.request("textDocument/hover");
        assert_eq!(ask["params"]["position"], json!({ "line": 0, "character": 8 }));
        let plain = s.h.render(1.0);
        s.says(json!({ "id": ask["id"], "result": { "contents": { "kind": "markdown", "value": "fn one()" } } }));
        assert_eq!(s.h.app().editor_popup(s.tab()), Some(EditorPopup::Text("fn one()".into())));
        assert!(s.h.render(1.0) != plain);
        s.h.key(Key::Right, Modifiers::default());
        assert!(s.h.app().editor_popup(s.tab()).is_none(), "moving on dismisses it");
    }

    #[test]
    fn go_to_definition_opens_the_file_and_moves_the_caret() {
        let mut s = Served::new("lsp-definition", "pub fn one() {}\n");
        s.h.app_mut().update(Msg::Ask(intel::Ask::Definition));
        let ask = s.request("textDocument/definition");
        let notes = crate::lsp::client::uri(&s.file.parent().unwrap().parent().unwrap().join("notes.md"));
        s.says(json!({ "id": ask["id"], "result": [{ "uri": notes, "range": { "start": { "line": 0, "character": 2 }, "end": { "line": 0, "character": 7 } } }] }));
        assert_eq!((s.tab().name.as_str(), s.tab().doc.cursor()), ("notes.md", Pos::new(0, 2)));
        // Nothing found, and somewhere outside the folder, both say so.
        s.h.app_mut().update(Msg::Select(0));
        s.h.app_mut().update(Msg::Ask(intel::Ask::Definition));
        let ask = s.request("textDocument/definition");
        s.says(json!({ "id": ask["id"], "result": null }));
        assert_eq!(s.h.app().toast.as_deref(), Some("No definition found."));
        s.h.app_mut().update(Msg::Ask(intel::Ask::Definition));
        let ask = s.request("textDocument/definition");
        s.says(json!({ "id": ask["id"], "result": { "uri": "file:///elsewhere/std.rs", "range": { "start": { "line": 1, "character": 0 }, "end": { "line": 1, "character": 1 } } } }));
        assert!(s.h.app().toast.as_deref().unwrap().starts_with("That is defined outside this folder"));
    }

    #[test]
    fn formatting_is_one_step_to_undo() {
        let mut s = Served::new("lsp-format", "fn  one( ){}\n");
        s.h.app_mut().update(Msg::Keys(1));
        s.h.app_mut().update(Msg::Ask(intel::Ask::Format));
        let ask = s.request("textDocument/formatting");
        let edit = |l0: u32, c0: u32, c1: u32, text: &str| json!({ "range": { "start": { "line": l0, "character": c0 }, "end": { "line": l0, "character": c1 } }, "newText": text });
        s.says(json!({ "id": ask["id"], "result": [edit(0, 2, 4, " "), edit(0, 8, 9, ""), edit(0, 10, 10, " ")] }));
        assert_eq!(s.tab().doc.text(), "fn one() {}\n");
        assert_eq!(s.tab().doc.keymap(), Keymap::Vim, "whatever keys were in use still are");
        assert!(s.methods().contains(&"textDocument/didChange".to_owned()), "and the server hears the result");
        s.h.app_mut().update(Msg::Edit(Action::Undo));
        assert_eq!(s.tab().doc.text(), "fn  one( ){}\n");
        // Nothing to change is said, not done.
        s.h.app_mut().update(Msg::Ask(intel::Ask::Format));
        let ask = s.request("textDocument/formatting");
        s.says(json!({ "id": ask["id"], "result": [] }));
        assert_eq!(s.h.app().toast.as_deref(), Some("Already formatted."));
    }

    #[test]
    fn the_servers_colours_reach_the_editor_and_stale_ones_are_dropped() {
        let mut s = Served::new("lsp-colours", "pub fn one() {}\nfn two() {}\n");
        let before = s.h.render(1.0);
        let ask = s.request("textDocument/semanticTokens/full");
        // The server calls `one` a type, which the built-in highlighter would
        // not, and `two` a function; a kind it never named is skipped.
        s.says(json!({ "id": ask["id"], "result": { "data": [0, 7, 3, 2, 0, 1, 3, 3, 0, 0, 0, 4, 2, 9, 0] } }));
        let tokens: Vec<EditorToken> = s.tab().tokens.to_vec();
        assert_eq!(tokens, [EditorToken { line: 0, from: 7, to: 10, kind: SyntaxKind::Type }, EditorToken { line: 1, from: 3, to: 6, kind: SyntaxKind::Function }]);
        assert!(s.h.render(1.0) != before, "the names are coloured");
        // Typing on the first line drops its colours until new ones come,
        // and asks for them; the second line keeps its own.
        s.h.app_mut().update(Msg::Edit(Action::Click { pos: Pos::new(0, 15), select: false }));
        s.h.type_text(" ");
        assert_eq!(s.tab().tokens.to_vec(), [tokens[1]]);
        assert!(s.methods().contains(&"textDocument/semanticTokens/full".to_owned()));
        // A new line moves everything below, so those go too.
        s.h.key(Key::Enter, Modifiers::default());
        assert!(s.tab().tokens.is_empty());
        // The server asking for a refresh gets a new request.
        s.sent();
        s.says(json!({ "id": 9, "method": "workspace/semanticTokens/refresh" }));
        assert_eq!(s.methods(), ["reply", "textDocument/semanticTokens/full"]);
    }

    #[test]
    fn a_server_that_stops_is_reported_and_stale_word_is_ignored() {
        let mut s = Served::new("lsp-stopped", "pub fn one() {}\n");
        let uri = s.uri();
        let problem = json!({ "method": "textDocument/publishDiagnostics", "params": { "uri": uri, "diagnostics": [{ "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 1 } }, "message": "x" }] } });
        // From an earlier launch of the server: not ours to act on.
        s.h.app_mut().update(Msg::Lsp("rust-analyzer", 0, Incoming::Message(problem.clone())));
        assert!(s.tab().diagnostics.is_empty());
        s.says(problem);
        assert_eq!(s.tab().diagnostics.len(), 1);
        assert_eq!(s.h.app().server_status(s.tab()), Some(("rust-analyzer".into(), Tone::Muted)));
        s.h.app_mut().update(Msg::Lsp("rust-analyzer", 1, Incoming::Closed));
        assert!(s.tab().diagnostics.is_empty(), "its problems go with it");
        assert_eq!(s.h.app().server_status(s.tab()), Some(("rust-analyzer stopped".into(), Tone::Warn)));
        s.h.app_mut().update(Msg::Ask(intel::Ask::Hover));
        assert_eq!(s.h.app().toast.as_deref(), Some("rust-analyzer stopped."));
    }

    /// The whole app against a real rust-analyzer, if one is installed:
    /// it is found, started, and its problems, colours and hover show.
    /// Slow and machine-dependent, so it runs only when asked for:
    /// `cargo test -p neo-code -- --ignored whole_app`. Set `NEO_SNAPSHOT_DIR`
    /// to keep pictures of the result.
    #[test]
    #[ignore]
    fn whole_app_with_a_real_server() {
        let dir = project("lsp-real");
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
        std::fs::remove_file(dir.join("src/lib.rs")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "struct Point {\n    x: u32,\n}\n\nfn answer() -> u32 {\n    \"forty-two\"\n}\n\nfn main() {\n    let p = Point { x: answer() };\n    println!(\"{}\", p.x);\n}\n").unwrap();
        let mut app = NeoCode::folder(&dir);
        app.update(Msg::Keys(0));
        let mut h = Harness::new(app, Size::new(1100.0, 560.0)).unwrap();
        // The search the app does at startup, which tests otherwise skip.
        h.app_mut().update(Msg::LspDirs(lsp::search_dirs()));
        assert!(h.app().servers_summary().contains("rust-analyzer"), "{}", h.app().servers_summary());
        let main = h.app().nodes.iter().position(|n| n.name == "main.rs").unwrap();
        h.app_mut().update(Msg::Open(main));
        h.click(Point::new(700.0, 300.0));
        let wait = |h: &mut Harness<NeoCode>, what: &str, done: &dyn Fn(&NeoCode) -> bool| {
            let until = std::time::Instant::now() + Duration::from_secs(180);
            while !done(h.app()) {
                assert!(std::time::Instant::now() < until, "timed out waiting for {what}");
                std::thread::sleep(Duration::from_millis(100));
                h.advance(Duration::from_millis(100));
            }
        };
        wait(&mut h, "an error and colours", &|a| a.active_tab().is_some_and(|t| t.diagnostics.iter().any(|d| d.severity == lsp::Severity::Error) && !t.tokens.is_empty()));
        let tab = h.app().active_tab().unwrap();
        assert_eq!(h.app().server_status(tab), Some(("rust-analyzer".into(), Tone::Muted)));
        assert!(tab.diagnostics.iter().any(|d| d.from.0 == 5), "the mismatched string is on line 6: {:?}", tab.diagnostics);
        assert!(tab.tokens.iter().any(|k| k.line == 0 && k.kind == SyntaxKind::Type), "`Point` is known to be a type");
        // Hover over `answer` where it is called. Ask again while the server is still loading.
        h.app_mut().update(Msg::Edit(Action::Click { pos: Pos::new(9, 25), select: false }));
        for _ in 0..40 {
            h.app_mut().toast = None;
            h.app_mut().update(Msg::Ask(intel::Ask::Hover));
            wait(&mut h, "an answer to hover", &|a| a.toast.is_some() || a.active_tab().is_some_and(|t| t.popup.is_some()));
            if h.app().active_tab().unwrap().popup.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        let Some(EditorPopup::Text(text)) = h.app().editor_popup(h.app().active_tab().unwrap()) else { panic!("no hover") };
        assert!(text.contains("answer") && text.contains("u32"), "{text}");
        if let Some(out) = std::env::var_os("NEO_SNAPSHOT_DIR") {
            h.save_png(PathBuf::from(out).join("code-lsp.png"), 1.0).unwrap();
        }
    }

    #[test]
    fn the_sample_project_says_why_it_has_no_language_server() {
        let app = NeoCode::sample();
        let tab = app.active_tab().unwrap();
        assert!(tab.name.ends_with(".rs") && tab.disk.is_none());
        assert_eq!(app.server_status(tab), Some(("Sample project: open a folder to use language servers".into(), Tone::Muted)));
    }

    #[test]
    fn starting_on_a_file_opens_its_project_around_it() {
        let dir = project("start-on-file");
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        let app = NeoCode::at(&dir.join("src/lib.rs"));
        assert_eq!(app.project, "start-on-file", "the folder with Cargo.toml, not src");
        assert_eq!(app.active_tab().unwrap().name, "lib.rs");
        assert!(app.nodes.iter().find(|n| n.name == "src").unwrap().expanded, "and the file shows in the tree");
        // A folder opens as itself, and a loose file brings just its folder.
        assert_eq!(NeoCode::at(&dir.join("src")).project, "src");
        let loose = project("loose-file");
        assert_eq!(NeoCode::at(&loose.join("notes.md")).project, "loose-file");
    }

    #[test]
    fn a_file_with_no_server_installed_says_so() {
        let dir = project("lsp-none");
        let mut h = Harness::new(NeoCode::folder(&dir), Size::new(1280.0, 820.0)).unwrap();
        let lib = h.app().nodes.iter().position(|n| n.name == "lib.rs").unwrap();
        h.app_mut().update(Msg::Open(lib));
        let status = h.app().server_status(h.app().active_tab().unwrap());
        assert_eq!(status, Some(("No language server for Rust was found".into(), Tone::Warn)));
        assert!(h.app().servers_summary().starts_with("None found. NeoCode looks for: rust-analyzer, clangd"));
        // A plain text file has no server to look for, and says nothing.
        std::fs::write(dir.join("plain.txt"), "hello\n").unwrap();
        h.app_mut().update(Msg::Folder(dir));
        let txt = h.app().nodes.iter().position(|n| n.name == "plain.txt").unwrap();
        h.app_mut().update(Msg::Open(txt));
        assert_eq!(h.app().server_status(h.app().active_tab().unwrap()), None);
    }

    #[test]
    fn changing_keymap_applies_to_open_files() {
        let mut h = editing(1);
        assert_eq!(label(&h), Some("NORMAL"));
        h.app_mut().update(Msg::Keys(2));
        assert_eq!(label(&h), Some("NOR"));
        h.app_mut().update(Msg::Keys(0));
        assert_eq!(label(&h), None);
    }
}
