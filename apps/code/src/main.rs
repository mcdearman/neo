//! Neo Code: a small code editor built with Neo, with optional Vim or
//! Helix keys.
//!
//!     cargo run -p neo-code                  # sample project
//!     cargo run -p neo-code -- path/to/dir   # a real folder (Cmd/Ctrl+S saves)
//!     cargo run -p neo-code -- --snapshot target/snapshots

// Release builds on Windows open no console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::time::Duration;

use neo::prelude::*;
use neo::{Key, KeyEvent, Size};
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
        let mut app = Self { project: "aurora".into(), nodes, tabs: vec![], active: None, dark: None, toast: None, keymap: Keymap::Vim, root: None, desktop: Desktop::load() };
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
        Self { project, nodes, tabs: vec![], active: None, dark: None, toast: None, keymap: Keymap::Vim, root: Some(root), desktop: Desktop::load() }
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
        self.root = Some(dir);
    }

    /// Opens what was dropped on the window: a folder as the project, or a
    /// file in a tab, switching to the file's folder if it is not in this one.
    fn open_dropped(&mut self, path: &Path) {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if path.is_dir() {
            return self.open_folder(&path);
        }
        let node = |app: &Self| app.nodes.iter().position(|n| matches!(&n.source, Some(Source::Disk(p)) if *p == path));
        if node(self).is_none()
            && let Some(parent) = path.parent()
        {
            self.open_folder(parent);
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
            None => format!("Neo Code · {}", self.project),
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

    fn subscriptions(&self) -> Vec<Subscription<Msg>> {
        let mut subs = vec![Desktop::subscription(Msg::Poll)];
        if self.toast.is_some() {
            subs.push(Subscription::every(Duration::from_secs(3), Msg::ClearToast));
        }
        subs
    }

    fn menus(&self) -> Vec<Menu<Msg>> {
        let file_open = self.active_tab().is_some();
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
                .push(MenuEntry::new("Select All", Msg::Edit(Action::SelectAll)).shortcut(Shortcut::command("a")).enabled(file_open)),
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
                self.tabs.push(Tab { saved: doc.revision(), doc, language: Language::from_path(&node.name), name: node.name, path: node.path, disk });
                self.active = Some(self.tabs.len() - 1);
            }
            Msg::ToggleDir(i) => self.nodes[i].expanded = !self.nodes[i].expanded,
            Msg::Select(i) => {
                if i < self.tabs.len() {
                    self.active = Some(i)
                }
            }
            Msg::Close(i) => {
                if i < self.tabs.len() {
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
                t.doc.apply(a);
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
        self.desktop.with_settings(self.content(), "Code Settings", Msg::Desktop, self.settings_rows())
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
            Some(t) => container(text_editor(&t.doc).language(t.language).on_action(Msg::Edit).into_element_keyed(&t.path))
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
                    .push(text("File ▸ Open Folder… opens a folder, or drop one on the window").role(TextRole::Caption).tone(Tone::Faint))
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
        vec![neo_desktop::ui::setting("Keys", "Standard editing, or Vim or Helix modal keys.", keys)]
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
        let bar = row()
            .width(Length::Fill)
            .align(Align::Center)
            .spacing(12.0)
            .push(left)
            .push(Space::fill_x())
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
        Some(dir) => {
            let p = PathBuf::from(dir);
            if !p.is_dir() {
                eprintln!("neo-code: {dir} is not a folder");
                std::process::exit(2);
            }
            NeoCode::folder(&p)
        }
        None => NeoCode::sample(),
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
        assert_eq!(titles, ["File", "Edit", "View"]);
        let open = &menus[0].entries[0];
        assert!(matches!(open.message, Some(Msg::OpenFolder)) && open.shortcut.as_ref().unwrap().matches(&key));
        // The current keys are ticked in the View menu.
        assert!(menus[2].entries.iter().any(|e| e.label == "✓ Vim Keys"));

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
