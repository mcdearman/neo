//! Neo Code: a small code editor built with Neo, with optional Vim keys.
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
    vim: bool,
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
    Vim(bool),
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
        let mut app = Self { project: "aurora".into(), nodes, tabs: vec![], active: None, dark: None, toast: None, vim: true, desktop: Desktop::load() };
        for p in ["src/main.rs", "src/solar.rs", "Cargo.toml"] {
            if let Some(i) = app.nodes.iter().position(|n| n.path == p) {
                app.update(Msg::Open(i));
            }
        }
        app.update(Msg::Select(0));
        app
    }

    fn folder(root: &Path) -> Self {
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
        Self { project, nodes, tabs: vec![], active: None, dark: None, toast: None, vim: true, desktop: Desktop::load() }
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

    fn on_key(&self, k: &KeyEvent) -> Option<Msg> {
        if !k.modifiers.command() {
            return None;
        }
        match &k.key {
            Key::Character(c) if c == "s" => Some(Msg::Save),
            Key::Character(c) if c == "w" => self.active.map(Msg::Close),
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
                doc.set_vim(self.vim);
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
            Msg::Vim(on) => {
                self.vim = on;
                for t in &mut self.tabs {
                    t.doc.set_vim(on);
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
            Msg::Poll => {
                self.desktop.poll();
            }
        }
    }

    fn view(&self) -> Element<Msg> {
        let body = row().spacing(14.0).width(Length::Fill).height(Length::Fill).push(self.sidebar()).push(self.main());
        column().width(Length::Fill).height(Length::Fill).padding([4.0, 14.0, 12.0, 14.0]).spacing(10.0).push(body).push(self.status_bar()).into()
    }
}

impl NeoCode {
    fn sidebar(&self) -> Element<Msg> {
        let active_path = self.active_tab().map(|t| t.path.clone());
        let mut list = column().spacing(2.0).width(Length::Fill);
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
                .push(icon(glyph).size(15.0).tone(tone))
                .push(text(n.name.clone()).role(if n.dir { TextRole::Strong } else { TextRole::Body }).no_wrap());
            let msg = if n.dir { Msg::ToggleDir(i) } else { Msg::Open(i) };
            list = list.push(
                Button::new(label)
                    .kind(ButtonKind::Ghost)
                    .selected(!n.dir && active_path.as_deref() == Some(n.path.as_str()))
                    .align_x(Align::Start)
                    .width(Length::Fill)
                    .padding([6.0, 8.0])
                    .radius(8.0)
                    .on_press(msg)
                    .into_element_keyed(&n.path),
            );
        }
        let header = column()
            .spacing(10.0)
            .width(Length::Fill)
            .push(text("Explorer").role(TextRole::Label).tone(Tone::Muted))
            .push(row().spacing(8.0).align(Align::Center).push(icon(icons::FOLDER_OPEN).size(17.0).tone(Tone::Accent)).push(text(self.project.clone()).role(TextRole::Title).no_wrap()));
        container(column().spacing(12.0).width(Length::Fill).height(Length::Fill).push(header).push(scrollable(list)))
            .surface(Surface::Card)
            .padding([16.0, 10.0])
            .width(250.0)
            .height(Length::Fill)
            .into()
    }

    fn main(&self) -> Element<Msg> {
        let mut tabs = row().spacing(6.0).align(Align::Center);
        for (i, t) in self.tabs.iter().enumerate() {
            let active = self.active == Some(i);
            let label = row()
                .spacing(8.0)
                .align(Align::Center)
                .push(icon(file_icon(&t.name)).size(14.0).tone(Tone::Accent))
                .push(text(t.name.clone()).role(TextRole::Strong).no_wrap())
                .push_if(t.dirty(), || text("●").role(TextRole::Caption).tone(Tone::Accent).into());
            tabs = tabs.push(
                row()
                    .spacing(2.0)
                    .align(Align::Center)
                    .push(Button::new(label).kind(ButtonKind::Ghost).selected(active).padding([7.0, 12.0]).radius(9.0).on_press(Msg::Select(i)))
                    .push(icon_button(icons::X, 24.0).kind(ButtonKind::Ghost).on_press(Msg::Close(i))),
            );
        }

        let editor: Element<Msg> = match self.active_tab() {
            Some(t) => container(text_editor(&t.doc).language(t.language).on_action(Msg::Edit).into_element_keyed(&t.path))
                .surface(Surface::Card)
                .padding(4.0)
                .width(Length::Fill)
                .height(Length::Fill)
                .into(),
            None => container(
                column()
                    .spacing(8.0)
                    .align(Align::Center)
                    .push(icon(icons::FILE_CODE).size(40.0).tone(Tone::Faint))
                    .push(text("Open a file from the Explorer").role(TextRole::Title).tone(Tone::Muted))
                    .push(text("Cmd/Ctrl+S saves · Cmd/Ctrl+W closes · Cmd/Ctrl+1–9 switches tabs · :w and :q work in Vim mode").role(TextRole::Caption).tone(Tone::Faint)),
            )
            .surface(Surface::Card)
            .center()
            .width(Length::Fill)
            .height(Length::Fill)
            .into(),
        };
        column().spacing(10.0).width(Length::Fill).height(Length::Fill).push(tabs).push(editor).into()
    }

    fn status_bar(&self) -> Element<Msg> {
        let mut left = row().spacing(18.0).align(Align::Center);
        if let Some(v) = self.active_tab().and_then(|t| t.doc.vim()) {
            let insert = v.mode == VimMode::Insert;
            let badge = container(text(v.mode.label()).role(TextRole::Label))
                .padding([4.0, 10.0])
                .radius(6.0)
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
        row()
            .width(Length::Fill)
            .align(Align::Center)
            .spacing(12.0)
            .padding([0.0, 6.0])
            .push(left)
            .push(Space::fill_x())
            .push(row().spacing(8.0).align(Align::Center).push(text("Vim").role(TextRole::Caption).tone(Tone::Muted)).push(toggle(self.vim, Msg::Vim)))
            .push(icon_button(if dark { icons::SUN } else { icons::MOON }, 34.0).on_press(Msg::ToggleScheme))
            .into()
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
    for (name, dark) in [("editor-light", false), ("editor-dark", true), ("editor-insert", false), ("editor-recording", true)] {
        let mut app = NeoCode::sample();
        app.dark = Some(dark);
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
            _ => h.type_text("qad2"),
        }
        let path = dir.join(format!("{name}.png"));
        h.save_png(&path, 1.0).expect("write png");
        println!("wrote {}", path.display());
    }
}
