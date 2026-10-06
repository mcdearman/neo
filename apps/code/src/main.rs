//! NeoCode: a small code editor built with Neo, with optional Vim or
//! Helix keys.
//!
//!     cargo run -p neo-code                  # sample project
//!     cargo run -p neo-code -- path/to/dir   # a real folder (Cmd/Ctrl+S saves)
//!     cargo run -p neo-code -- --snapshot target/snapshots

// Release builds on Windows open no console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

mod dap;
mod debug;
mod intel;
mod lsp;
mod runner;
mod settings;

use neo::prelude::*;
use neo::{Color, Key, KeyEvent, Proxy, Size};
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
    /// What the server offers to do to parts of the file.
    lenses: Vec<lsp::Lens>,
    /// The word the mouse is over, since when, and whether the server has
    /// been asked about it yet.
    pointed: Option<(Pos, Instant, bool)>,
    /// The word a hover still unanswered was asked about, if it was the
    /// mouse's and not the caret's.
    hover_for: Option<Pos>,
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
    /// What was last run from a lens, showing under the editor.
    run: Option<runner::Run>,
    runs: u64,
    /// Whether choosing a lens really starts its command.
    runs_commands: bool,
    settings: settings::Settings,
    /// NeoCode's own folder, where settings and what was open are kept.
    /// Tests keep nothing unless they say where.
    config: Option<PathBuf>,
    /// The folders opened lately, for the File menu.
    recent: Vec<PathBuf>,
    /// The program being debugged, if one is.
    debug: Option<debug::Session>,
    debugs: u64,
    /// Where the breakpoints are: by file, lines from zero, in order.
    breakpoints: std::collections::HashMap<PathBuf, Vec<usize>>,
    /// A function to debug that is waiting for its arguments.
    asking: Option<debug::Asking>,
    /// The arguments last given to each function debugged.
    debug_args: std::collections::HashMap<(PathBuf, String), String>,
    /// The Meadow program that last offered to debug something.
    meadow: Option<PathBuf>,
    /// A shell under the editor, once it has been asked for.
    terminal: Option<neo_term::Shell>,
    terminal_shown: bool,
    /// Raised to hand the keyboard back to the editor.
    editor_focus: u64,
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
    /// The mouse is over the word starting here, or over none.
    Point(Option<Pos>),
    /// A word was clicked with Command held: go to where it is defined.
    Jump,
    /// Open `settings.json` to edit.
    OpenSettings,
    OpenRecent(PathBuf),
    /// Show the terminal under the editor, or put it away.
    ToggleTerminal,
    /// Something that happened in the terminal.
    Term(neo_term::TermMsg),
    /// The margin beside this line was clicked: set or clear a breakpoint.
    Margin(usize),
    Debug(debug::Step),
    /// Word from the debugger with this number.
    Dap(u64, dap::Incoming),
    DebugBuilt(u64, Result<debug::DebugSpec, String>),
    DebugFrame(usize),
    DebugRow(usize),
    DebugAskTyped(String),
    DebugAskGo,
    DebugAskCancel,
    /// The lens on this line, and which of its labels, was clicked.
    Lens(usize, usize),
    /// A line printed by what is running, and how it ended.
    RunSaid(u64, String),
    RunEnded(u64, Option<i32>),
    RunStop,
    RunAgain,
    RunClose,
    /// See whether the mouse has rested on a word long enough.
    HoverTick,
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
        let mut app = Self { project: "aurora".into(), nodes, tabs: vec![], active: None, dark: None, toast: None, keymap: Keymap::Vim, root: None, desktop: Desktop::load(), servers: intel::Servers::default(), run: None, runs: 0, runs_commands: !cfg!(test), settings: Default::default(), config: None, recent: vec![], debug: None, debugs: 0, breakpoints: Default::default(), asking: None, debug_args: Default::default(), meadow: None, terminal: None, terminal_shown: false, editor_focus: 0 };
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
        Self { project, nodes, tabs: vec![], active: None, dark: None, toast: None, keymap: Keymap::Vim, root: Some(root), desktop: Desktop::load(), servers: intel::Servers::default(), run: None, runs: 0, runs_commands: !cfg!(test), settings: Default::default(), config: None, recent: vec![], debug: None, debugs: 0, breakpoints: Default::default(), asking: None, debug_args: Default::default(), meadow: None, terminal: None, terminal_shown: false, editor_focus: 0 }
    }

    /// The folder a file belongs to: the nearest one above it that looks
    /// like a project, or failing that the one it is in.
    fn project_of(file: &Path) -> PathBuf {
        let parent = file.parent().unwrap_or(file);
        const MARKERS: [&str; 8] = [".git", "Cargo.toml", "package.json", "go.mod", "pyproject.toml", "build.zig", "compile_commands.json", "Meadow.toml"];
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
        if let Some(mut run) = self.run.take() {
            run.stop();
        }
        self.end_debugging();
        // The shell was in the old folder.
        self.terminal = None;
        self.terminal_shown = false;
        self.load_settings();
        self.restore_workspace();
        // They may have been found already; the files just reopened are
        // waiting for them.
        if self.servers.dirs.is_some() {
            for i in 0..self.tabs.len() {
                self.lsp_open(i);
            }
        }
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
        Language::Json | Language::Yaml => icons::FILE_COG,
        // Every other language Neo can colour is code.
        _ => icons::FILE_CODE,
    }
}

impl NeoCode {
    /// Whether the window is dark: the override if set, else the desktop setting.
    fn is_dark(&self) -> bool {
        self.dark.unwrap_or(self.desktop.appearance.scheme == neo_desktop::SchemePref::Dark)
    }
}

impl NeoCode {
    fn apply(&mut self, m: Msg) {
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
                self.tabs.push(Tab { saved: doc.revision(), doc, language: Language::from_path(&node.name), name: node.name, path: node.path, disk, server: None, version: 0, synced: 0, diagnostics: vec![], tokens: std::rc::Rc::from([]), popup: None, lenses: vec![], pointed: None, hover_for: None });
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
            Msg::Point(word) => self.point(word),
            Msg::HoverTick => self.hover_rested(),
            Msg::Lens(line, index) => self.lens_chosen(line, index),
            Msg::Margin(line) => self.toggle_breakpoint(line),
            Msg::ToggleTerminal => self.toggle_terminal(),
            Msg::Term(m) => {
                if let Some(shell) = &mut self.terminal {
                    shell.update(m);
                }
            }
            Msg::Debug(step) => self.debug_step(step),
            Msg::Dap(id, incoming) => self.debug_incoming(id, incoming),
            Msg::DebugBuilt(id, result) => self.debug_built(id, result),
            Msg::DebugFrame(i) => self.debug_frame(i),
            Msg::DebugRow(i) => self.debug_row(i),
            Msg::DebugAskTyped(typed) => {
                if let Some(ask) = &mut self.asking {
                    ask.typed = typed;
                }
            }
            Msg::DebugAskGo => self.debug_asked(),
            Msg::DebugAskCancel => self.asking = None,
            Msg::RunSaid(id, line) => {
                if let Some(run) = self.run.as_mut().filter(|r| r.id == id) {
                    run.push(&line);
                }
            }
            Msg::RunEnded(id, code) => {
                // One that was stopped stays stopped, whatever it exited with.
                if let Some(run) = self.run.as_mut().filter(|r| r.id == id && r.status == runner::Status::Running) {
                    run.status = runner::Status::Ended(code);
                }
            }
            Msg::RunStop => {
                if let Some(run) = &mut self.run {
                    run.stop();
                }
            }
            Msg::RunAgain => {
                if let Some(spec) = self.run.as_ref().map(|r| r.spec.clone()) {
                    self.start_run(spec);
                }
            }
            Msg::RunClose => {
                if let Some(mut run) = self.run.take() {
                    run.stop();
                }
            }
            Msg::Jump => self.ask(intel::Ask::Definition),
            Msg::PopupKey(key) => self.popup_key(key),
            Msg::Keys(i) => {
                self.keymap = KEYMAPS.get(i).map_or(Keymap::Plain, |(k, _)| *k);
                for t in &mut self.tabs {
                    t.doc.set_keymap(self.keymap);
                }
                self.keep(settings::KEYMAP, settings::keymap_name(self.keymap).into());
            }
            Msg::OpenSettings => self.open_settings(),
            Msg::OpenRecent(dir) => self.open_folder(&dir),
            Msg::Save => {
                let Some(t) = self.active.and_then(|i| self.tabs.get_mut(i)) else { return };
                let result = match &t.disk {
                    Some(p) => std::fs::write(p, t.doc.text() + "\n").map(|_| format!("Saved {}", t.name)),
                    None => Ok(format!("Saved {} (sample project, kept in memory)", t.name)),
                };
                match result {
                    Ok(msg) => {
                        t.saved = t.doc.revision();
                        let settings = t.name == "settings.json";
                        self.toast = Some(msg);
                        // Saving the settings is how they are changed.
                        if settings {
                            self.load_settings();
                        }
                        self.save_workspace();
                        if let Some(i) = self.active {
                            self.lsp_saved(i);
                        }
                    }
                    Err(e) => self.toast = Some(format!("Couldn't save {}: {e}", t.name)),
                }
            }
            Msg::ToggleScheme => {
                self.dark = Some(!self.is_dark());
                self.keep(settings::THEME, if self.is_dark() { "dark" } else { "light" }.into());
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
        // Only while the mouse is resting on a word not yet asked about.
        if self.hover_waiting() {
            subs.push(Subscription::every(Duration::from_millis(60), Msg::HoverTick));
        }
        subs
    }

    fn menus(&self) -> Vec<Menu<Msg>> {
        let file_open = self.active_tab().is_some();
        // Whether a language server is looking after the file in front.
        let served = self.active_tab().and_then(|t| t.server).is_some_and(|s| self.servers.clients.contains_key(s));
        let keys = KEYMAPS.iter().enumerate().map(|(i, (k, name))| MenuEntry::new(format!("{}{name} Keys", if *k == self.keymap { "✓ " } else { "" }), Msg::Keys(i)));
        vec![
            {
                let mut file = Menu::new("File").push(MenuEntry::new("Open Folder…", Msg::OpenFolder).shortcut(Shortcut::command("o")));
                // The folders opened lately, other than this one.
                for dir in self.recent.iter().filter(|d| Some(*d) != self.root.as_ref()).take(5) {
                    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| dir.display().to_string());
                    file = file.push(MenuEntry::new(format!("Reopen {name}"), Msg::OpenRecent(dir.clone())));
                }
                file.separator()
                    .push(MenuEntry::new("Open Settings (JSON)", Msg::OpenSettings))
                    .separator()
                    .push(MenuEntry::new("Save", Msg::Save).shortcut(Shortcut::command("s")).enabled(file_open))
                    .push(MenuEntry::new("Close Tab", Msg::Close(self.active.unwrap_or(0))).shortcut(Shortcut::command("w")).enabled(file_open))
            },
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
            {
                let (going, stopped) = (self.debug.as_ref().is_some_and(|d| !d.over()), self.debug.as_ref().is_some_and(|d| d.stopped()));
                Menu::new("Debug")
                    .push(MenuEntry::new(if going { "Continue  (F5)" } else { "Start Debugging  (F5)" }, Msg::Debug(debug::Step::Go)).enabled(file_open && (!going || stopped)))
                    .push(MenuEntry::new("Pause  (F6)", Msg::Debug(debug::Step::Pause)).enabled(going && !stopped))
                    .push(MenuEntry::new("Stop  (Shift+F5)", Msg::Debug(debug::Step::Stop)).enabled(self.debug.is_some()))
                    .separator()
                    .push(MenuEntry::new("Step Over  (F10)", Msg::Debug(debug::Step::Over)).enabled(stopped))
                    .push(MenuEntry::new("Step Into  (F11)", Msg::Debug(debug::Step::In)).enabled(stopped))
                    .push(MenuEntry::new("Step Out  (Shift+F11)", Msg::Debug(debug::Step::Out)).enabled(stopped))
                    .separator()
                    .push(MenuEntry::new("Toggle Breakpoint  (F9)", Msg::Margin(self.active_tab().map_or(0, |t| t.doc.cursor().line))).enabled(file_open))
            },
            keys.fold(Menu::new("View").push(MenuEntry::new(if self.terminal_shown { "Hide Terminal  (Ctrl+Tab)" } else { "Terminal  (Ctrl+Tab)" }, Msg::ToggleTerminal)).push(MenuEntry::new(if self.is_dark() { "Light Appearance" } else { "Dark Appearance" }, Msg::ToggleScheme)).separator(), Menu::push),
        ]
    }

    fn on_key(&self, k: &KeyEvent) -> Option<Msg> {
        // Ctrl+Tab goes to the terminal and back, on every system.
        if k.key == Key::Tab && k.modifiers.ctrl {
            return Some(Msg::ToggleTerminal);
        }
        // The debugger's keys, as everywhere: F5 to go, F10 and F11 to step.
        if let Key::F(n) = k.key {
            let line = self.active_tab().map(|t| t.doc.cursor().line);
            return match (n, k.modifiers.shift) {
                (5, false) => Some(Msg::Debug(debug::Step::Go)),
                (5, true) => Some(Msg::Debug(debug::Step::Stop)),
                (6, _) => Some(Msg::Debug(debug::Step::Pause)),
                (9, _) => line.map(Msg::Margin),
                (10, _) => Some(Msg::Debug(debug::Step::Over)),
                (11, false) => Some(Msg::Debug(debug::Step::In)),
                (11, true) => Some(Msg::Debug(debug::Step::Out)),
                _ => None,
            };
        }
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
        let before = self.shape();
        self.apply(m);
        // What is open changed: remember it for next time.
        if self.shape() != before {
            self.save_workspace();
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
            Some(t) => container(text_editor(&t.doc).language(t.language).on_action(Msg::Edit).marks(self.editor_marks(t)).tokens(t.tokens.clone()).popup(self.editor_popup(t)).popup_at(self.editor_popup_at(t)).font_size(self.settings.font_size).lenses(self.editor_lenses(t), Msg::Lens).breakpoints(self.breakpoints_in(t), Msg::Margin).stopped_at(self.stopped_in(t)).focus(self.editor_focus).on_point(Msg::Point).on_jump(|_| Msg::Jump).on_popup_key(Msg::PopupKey).into_element_keyed(&t.path))
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
        let mut col = column().width(Length::Fill).height(Length::Fill).push(tabs).push(Divider::horizontal()).push(editor);
        // One panel under the editor: the terminal while it is up, else a
        // question before debugging, the debugger, or what was last run.
        if let Some(shell) = self.terminal.as_ref().filter(|_| self.terminal_shown) {
            col = col.push(Divider::horizontal()).push(self.terminal_panel(shell));
        } else if let Some(ask) = &self.asking {
            col = col.push(Divider::horizontal()).push(self.asking_panel(ask));
        } else if let Some(session) = &self.debug {
            col = col.push(Divider::horizontal()).push(self.debug_panel(session, surface));
        } else if let Some(run) = &self.run {
            col = col.push(Divider::horizontal()).push(self.run_panel(run, surface));
        }
        col.into()
    }

    /// Shows the terminal under the editor with the keyboard in it, or
    /// puts it away and hands the keyboard back. The shell is started the
    /// first time, in the folder that is open, and keeps running while
    /// out of sight.
    fn toggle_terminal(&mut self) {
        if self.terminal_shown {
            self.terminal_shown = false;
            self.editor_focus += 1;
            return;
        }
        self.terminal_shown = true;
        if self.terminal.is_none() {
            let mut shell = neo_term::Shell::new(self.root.clone().unwrap_or_else(neo_desktop::fs::home_dir), None);
            // Tests show the panel without a shell behind it.
            if let Some(proxy) = self.servers.proxy.clone().filter(|_| self.runs_commands) {
                shell.start(move |m| {
                    proxy.send(Msg::Term(m));
                });
            }
            self.terminal = Some(shell);
        }
        if let Some(shell) = &mut self.terminal {
            shell.focus();
        }
    }

    fn terminal_panel(&self, shell: &neo_term::Shell) -> Element<Msg> {
        let head = row()
            .spacing(8.0)
            .align(Align::Center)
            .width(Length::Fill)
            .push(icon(icons::SQUARE_TERMINAL).size(14.0).tone(Tone::Muted))
            .push(text("Terminal").role(TextRole::Strong).no_wrap())
            .push(container(text(shell.title.clone().filter(|t| !t.is_empty()).unwrap_or_else(|| shell.cwd.display().to_string())).role(TextRole::Caption).tone(Tone::Muted).no_wrap()).width(Length::Fill))
            .push(text("Ctrl+Tab").role(TextRole::Caption).tone(Tone::Faint).no_wrap())
            .push(icon_button(icons::X, 22.0).kind(ButtonKind::Ghost).on_press(Msg::ToggleTerminal));
        column().width(Length::Fill).height(280.0).push(container(head).padding([10.0, 4.0]).width(Length::Fill)).push(shell.view(Msg::Term)).into()
    }

    /// What was run from a lens: its name, how it stands, and what it
    /// has printed, under the editor.
    fn run_panel(&self, run: &runner::Run, surface: Color) -> Element<Msg> {
        let running = run.status == runner::Status::Running;
        let tone = match &run.status {
            runner::Status::Running => Tone::Accent,
            _ if run.ended_well() => Tone::Good,
            runner::Status::Stopped => Tone::Muted,
            _ => Tone::Bad,
        };
        let mut head = row()
            .spacing(8.0)
            .align(Align::Center)
            .width(Length::Fill)
            .push(icon(if running { icons::PLAY } else if run.ended_well() { icons::CIRCLE_CHECK } else { icons::CIRCLE_ALERT }).size(14.0).tone(tone))
            .push(text(run.spec.title.clone()).role(TextRole::Strong).no_wrap())
            .push(container(text(run.summary()).role(TextRole::Caption).tone(tone).no_wrap()).width(Length::Fill));
        head = if running { head.push(Button::new(text("Stop").role(TextRole::Caption)).padding([10.0, 3.0]).radius(6.0).on_press(Msg::RunStop)) } else { head.push(Button::new(text("Run Again").role(TextRole::Caption)).padding([10.0, 3.0]).radius(6.0).on_press(Msg::RunAgain)) };
        head = head.push(icon_button(icons::X, 22.0).kind(ButtonKind::Ghost).on_press(Msg::RunClose));
        column()
            .width(Length::Fill)
            .height(220.0)
            .push(container(head).padding([10.0, 4.0]).width(Length::Fill))
            .push(container(text_editor(&run.output).language(Language::Plain).into_element_keyed("run-output")).background(Background::Color(surface)).width(Length::Fill).height(Length::Fill))
            .into()
    }

    /// What is open and showing, to tell when that has changed.
    fn shape(&self) -> (Vec<String>, Option<usize>, Option<PathBuf>) {
        (self.tabs.iter().map(|t| t.path.clone()).collect(), self.active, self.root.clone())
    }

    /// Whether a tab is a file of the open folder, to come back to.
    fn in_folder(&self, t: &Tab) -> bool {
        matches!((&t.disk, &self.root), (Some(disk), Some(root)) if disk.starts_with(root))
    }

    /// Writes down which files are open in this folder, and where the
    /// caret is in each.
    fn save_workspace(&self) {
        let (Some(dir), Some(root)) = (&self.config, &self.root) else { return };
        let open = self.tabs.iter().filter(|t| self.in_folder(t)).map(|t| (t.path.clone(), t.doc.cursor().line, t.doc.cursor().col)).collect();
        let active = self.active_tab().filter(|t| self.in_folder(t)).map(|t| t.path.clone());
        let _ = settings::save_workspace(dir, root, &settings::Workspace { open, active });
    }

    /// Opens the files that were open in this folder last time, those of
    /// them that are still there.
    fn restore_workspace(&mut self) {
        let (Some(dir), Some(root)) = (self.config.clone(), self.root.clone()) else { return };
        let _ = settings::add_recent(&dir, &root);
        self.recent = settings::recent(&dir);
        if !self.settings.restore {
            return;
        }
        let Some(ws) = settings::load_workspace(&dir, &root) else { return };
        for (path, line, col) in &ws.open {
            let Some(i) = self.nodes.iter().position(|n| !n.dir && n.path == *path) else { continue };
            self.apply(Msg::Open(i));
            // Only if it opened: a file can have stopped being text.
            if let Some(t) = self.tabs.last_mut().filter(|t| t.path == *path) {
                let line = (*line).min(t.doc.line_count().saturating_sub(1));
                let col = (*col).min(t.doc.lines()[line].len());
                if t.doc.lines()[line].is_char_boundary(col) {
                    t.doc.apply(Action::Click { pos: Pos::new(line, col), select: false });
                }
            }
            // Show where it is in the tree.
            let mut depth = self.nodes[i].depth;
            for j in (0..i).rev() {
                if self.nodes[j].dir && self.nodes[j].depth < depth {
                    self.nodes[j].expanded = true;
                    depth = self.nodes[j].depth;
                }
            }
        }
        if let Some(active) = ws.active.and_then(|a| self.tabs.iter().position(|t| t.path == a)) {
            self.active = Some(active);
        }
        self.toast = None;
    }

    /// Reads the settings again and puts them into effect.
    fn load_settings(&mut self) {
        let (found, problems) = settings::load(self.config.as_deref(), self.root.as_deref());
        self.keymap = found.keymap;
        for t in &mut self.tabs {
            t.doc.set_keymap(self.keymap);
        }
        self.dark = found.dark;
        self.settings = found;
        if let Some(first) = problems.into_iter().next() {
            self.toast = Some(first);
        }
    }

    /// Writes one setting to the user's file, as when it is changed from
    /// a menu, so that it is still so next time.
    fn keep(&mut self, key: &str, value: serde_json::Value) {
        let Some(dir) = &self.config else { return };
        if let Err(why) = settings::set(&settings::user_file(dir), key, value) {
            self.toast = Some(why);
        }
    }

    /// Opens the user's `settings.json` in a tab, making it first if
    /// there is none. Saving it puts the changes into effect.
    fn open_settings(&mut self) {
        let Some(dir) = self.config.clone() else {
            self.toast = Some("There is no settings file to open here.".into());
            return;
        };
        let file = settings::user_file(&dir);
        if let Err(e) = settings::ensure(&file) {
            self.toast = Some(format!("Couldn't make {}: {e}", file.display()));
            return;
        }
        self.open_file(&file);
    }

    /// Opens a file that is not part of the folder's tree, in a tab of
    /// its own, named by its whole path.
    fn open_file(&mut self, file: &Path) {
        let key = file.to_string_lossy().into_owned();
        if let Some(t) = self.tabs.iter().position(|t| t.path == key) {
            self.active = Some(t);
            return;
        }
        let text = match std::fs::read_to_string(file) {
            Ok(text) => text,
            Err(e) => {
                self.toast = Some(format!("Couldn't open {}: {e}", file.display()));
                return;
            }
        };
        let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut doc = Document::new(text.trim_end_matches('\n'));
        doc.set_keymap(self.keymap);
        self.tabs.push(Tab { saved: doc.revision(), doc, language: Language::from_path(&name), name, path: key, disk: Some(file.to_path_buf()), server: None, version: 0, synced: 0, diagnostics: vec![], tokens: std::rc::Rc::from([]), popup: None, lenses: vec![], pointed: None, hover_for: None });
        self.active = Some(self.tabs.len() - 1);
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
                .push(text(format!("Spaces: {}", self.settings.tab_size)).role(TextRole::Caption).tone(Tone::Muted))
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
    let mut app = match args.first() {
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
    // Settings, and for a folder the files that were open in it. A file
    // named on the command line is what was asked for, and stays showing.
    app.config = Some(neo_desktop::config_dir().join("neo-code"));
    app.load_settings();
    let asked = app.active_tab().map(|t| t.path.clone());
    if app.root.is_some() {
        app.restore_workspace();
    }
    if let Some(i) = asked.and_then(|a| app.tabs.iter().position(|t| t.path == a)) {
        app.active = Some(i);
    }
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
        assert_eq!(titles, ["File", "Edit", "Go", "Debug", "View"]);
        let open = &menus[0].entries[0];
        assert!(matches!(open.message, Some(Msg::OpenFolder)) && open.shortcut.as_ref().unwrap().matches(&key));
        // The current keys are ticked in the View menu.
        assert!(menus[4].entries.iter().any(|e| e.label == "✓ Vim Keys"));

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
        // And by choosing the entry: File, then the last row, under Open
        // Folder, the settings and Save, with a rule after each of the first two.
        h.click(Point::new(24.0, 23.0));
        h.click(Point::new(60.0, 44.0 + 32.0 + 9.0 + 32.0 + 9.0 + 32.0 + 16.0));
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
            let caps = json!({ "positionEncoding": "utf-8", "documentFormattingProvider": true, "completionProvider": { "triggerCharacters": ["."] }, "semanticTokensProvider": { "legend": { "tokenTypes": ["function", "keyword", "type"] }, "full": true }, "codeLensProvider": {}, "executeCommandProvider": { "commands": ["tool.tidy"] } });
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
    fn resting_the_mouse_on_a_word_asks_about_it_and_shows_the_answer_there() {
        let mut s = Served::new("lsp-mouse-hover", "pub fn one() {}\nfn two() { one() }\n");
        s.sent();
        let rested = |s: &mut Served| {
            let i = s.h.app().active.unwrap();
            let t = &mut s.h.app_mut().tabs[i];
            t.pointed = t.pointed.map(|(p, _, asked)| (p, Instant::now() - NeoCode::REST * 2, asked));
            s.h.app_mut().update(Msg::HoverTick);
        };
        // Just arrived: nothing is asked until it has stayed a moment.
        s.h.app_mut().update(Msg::Point(Some(Pos::new(1, 11))));
        assert!(s.h.app().hover_waiting());
        s.h.app_mut().update(Msg::HoverTick);
        assert!(!s.methods().contains(&"textDocument/hover".to_owned()), "not at once");
        rested(&mut s);
        let ask = s.request("textDocument/hover");
        assert_eq!(ask["params"]["position"], json!({ "line": 1, "character": 11 }), "the word under the mouse, not the caret");
        assert!(!s.h.app().hover_waiting(), "asked once, not on every tick");
        let caret = s.tab().doc.cursor();
        s.says(json!({ "id": ask["id"], "result": { "contents": "fn one()" } }));
        assert_eq!(s.h.app().editor_popup(s.tab()), Some(EditorPopup::Text("fn one()".into())));
        assert_eq!(s.h.app().editor_popup_at(s.tab()), Some(Pos::new(1, 11)), "shown at the word");
        assert_eq!(s.tab().doc.cursor(), caret, "the caret stays where it was");
        // Moving off the word takes it away.
        s.h.app_mut().update(Msg::Point(None));
        assert!(s.h.app().editor_popup(s.tab()).is_none());

        // An answer that arrives after the mouse has moved on is dropped,
        // and a spot with nothing to say is passed over quietly.
        s.h.app_mut().update(Msg::Point(Some(Pos::new(0, 7))));
        rested(&mut s);
        let late = s.request("textDocument/hover");
        s.h.app_mut().update(Msg::Point(Some(Pos::new(1, 3))));
        s.says(json!({ "id": late["id"], "result": { "contents": "fn one()" } }));
        assert!(s.h.app().editor_popup(s.tab()).is_none());
        rested(&mut s);
        let empty = s.request("textDocument/hover");
        s.h.app_mut().toast = None;
        s.says(json!({ "id": empty["id"], "result": null }));
        assert!(s.h.app().editor_popup(s.tab()).is_none() && s.h.app().toast.is_none());

        // A problem on the word shows at once, with the answer under it.
        let uri = s.uri();
        s.says(json!({ "method": "textDocument/publishDiagnostics", "params": { "uri": uri, "diagnostics": [{ "range": { "start": { "line": 1, "character": 11 }, "end": { "line": 1, "character": 14 } }, "severity": 1, "message": "mismatched types" }] } }));
        s.h.app_mut().update(Msg::Point(Some(Pos::new(1, 11))));
        rested(&mut s);
        assert_eq!(s.h.app().editor_popup(s.tab()), Some(EditorPopup::Text("mismatched types".into())));
        let ask = s.request("textDocument/hover");
        s.says(json!({ "id": ask["id"], "result": { "contents": "fn one()" } }));
        assert_eq!(s.h.app().editor_popup(s.tab()), Some(EditorPopup::Text("mismatched types\n\nfn one()".into())));
        // Hover at the caret still works, and is shown at the caret.
        s.h.app_mut().update(Msg::Point(None));
        s.h.app_mut().update(Msg::Ask(intel::Ask::Hover));
        let ask = s.request("textDocument/hover");
        s.says(json!({ "id": ask["id"], "result": { "contents": "at the caret" } }));
        assert_eq!((s.h.app().editor_popup(s.tab()), s.h.app().editor_popup_at(s.tab())), (Some(EditorPopup::Text("at the caret".into())), None));
    }

    #[test]
    fn the_editor_says_which_word_the_mouse_is_over() {
        let mut s = Served::new("lsp-point", "pub fn one() {}\nfn two() { one() }\n");
        s.h.render(1.0);
        let pointed = |s: &Served| s.tab().pointed.map(|(p, _, _)| p);
        // Find the text by sweeping across the first line: the words come
        // up in order, each by where it starts, with gaps between them.
        let mut seen = vec![];
        let y = (60..200).step_by(2).map(|y| y as f32).find(|y| {
            s.h.move_to(Point::new(700.0, *y));
            s.h.move_to(Point::new(330.0, *y));
            pointed(&s).is_some()
        });
        let y = y.expect("a line of text somewhere near the top");
        for x in (250..700).step_by(2) {
            s.h.move_to(Point::new(x as f32, y));
            if seen.last() != Some(&pointed(&s)) {
                seen.push(pointed(&s));
            }
        }
        let at = |col| Some(Pos::new(0, col));
        assert_eq!(seen, [None, at(0), None, at(4), None, at(7), None], "pub, fn and one; not the margin, the gaps, the brackets or past the end");
        // Leaving the window is leaving the word.
        s.h.move_to(Point::new(330.0, y));
        assert!(pointed(&s).is_some());
        s.h.event(neo::Event::PointerLeft);
        assert_eq!(pointed(&s), None);
    }

    #[test]
    fn clicking_a_word_with_command_held_goes_to_its_definition() {
        let mut s = Served::new("lsp-jump", "pub fn one() {}\nfn two() { one() }\n");
        s.h.render(1.0);
        // Find `one` on the second line with the mouse, as the hover test does.
        let pointed = |s: &Served| s.tab().pointed.map(|(p, _, _)| p);
        let spot = (60..240).step_by(2).flat_map(|y| (250..700).step_by(4).map(move |x| Point::new(x as f32, y as f32))).find(|p| {
            s.h.move_to(*p);
            pointed(&s) == Some(Pos::new(1, 11))
        });
        let spot = spot.expect("the word on screen");
        // A plain click only moves the caret.
        s.sent();
        s.h.click(spot);
        assert_eq!(s.tab().doc.cursor().line, 1);
        assert!(!s.methods().contains(&"textDocument/definition".to_owned()));
        // With the key held it asks, about the place clicked.
        // Pressing the key, with the mouse where it is, marks the word:
        // darker and underlined, with a hand for a pointer.
        let plain = s.h.render(1.0);
        s.h.set_modifiers(Modifiers { logo: cfg!(target_os = "macos"), ctrl: !cfg!(target_os = "macos"), ..Default::default() });
        assert_eq!(s.h.cursor(), neo::CursorIcon::Pointer, "the word looks like somewhere to go");
        let marked = s.h.render(1.0);
        assert!(marked != plain, "and is drawn differently");
        // Only that word: the line above is drawn as it was.
        let changed: Vec<usize> = plain.chunks(4).zip(marked.chunks(4)).enumerate().filter(|(_, (a, b))| a != b).map(|(i, _)| i / 1280).collect();
        let (top, bottom) = (*changed.first().unwrap(), *changed.last().unwrap());
        assert!(bottom - top < 30 && (top as f32 - spot.y).abs() < 30.0, "rows {top} to {bottom}, around {}", spot.y);
        // Letting go of the key puts it back, as does moving off the word.
        s.h.set_modifiers(Modifiers::default());
        assert!(s.h.render(1.0) == plain);
        s.h.set_modifiers(Modifiers { logo: cfg!(target_os = "macos"), ctrl: !cfg!(target_os = "macos"), ..Default::default() });
        s.h.move_to(Point::new(spot.x, spot.y + 200.0));
        assert!(s.h.render(1.0) == plain);
        s.h.move_to(spot);
        assert!(s.h.render(1.0) == marked);
        s.h.click(spot);
        let ask = s.request("textDocument/definition");
        assert_eq!(ask["params"]["position"]["line"], 1);
        let col = ask["params"]["position"]["character"].as_u64().unwrap();
        assert!((11..=14).contains(&col), "inside `one`: {col}");
        let uri = s.uri();
        s.says(json!({ "id": ask["id"], "result": { "uri": uri, "range": { "start": { "line": 0, "character": 7 }, "end": { "line": 0, "character": 10 } } } }));
        assert_eq!(s.tab().doc.cursor(), Pos::new(0, 7), "the caret is at the definition");
    }

    #[test]
    fn a_folder_comes_back_with_the_files_that_were_open_in_it() {
        let dir = project("workspace");
        std::fs::write(dir.join("src/lib.rs"), "pub fn one() {}\npub fn two() {}\n").unwrap();
        std::fs::write(dir.join("notes.md"), "# Notes\n").unwrap();
        let config = dir.parent().unwrap().join(format!("{}-config", dir.file_name().unwrap().to_string_lossy()));
        let _ = std::fs::remove_dir_all(&config);
        let open = |app: &mut NeoCode, name: &str| {
            let i = app.nodes.iter().position(|n| n.name == name).unwrap();
            app.update(Msg::Open(i));
        };
        let mut first = NeoCode::folder(&dir);
        first.config = Some(config.clone());
        first.restore_workspace();
        assert!(first.tabs.is_empty(), "nothing to come back to yet");
        open(&mut first, "notes.md");
        open(&mut first, "lib.rs");
        first.update(Msg::Edit(Action::Click { pos: Pos::new(1, 7), select: false }));
        open(&mut first, "notes.md");
        first.update(Msg::Save);
        let root = first.root.clone().unwrap();

        // Another run: the same tabs in the same order, the same one
        // showing, and the caret where it was.
        let mut second = NeoCode::folder(&dir);
        second.config = Some(config.clone());
        second.load_settings();
        second.restore_workspace();
        assert_eq!(second.tabs.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["notes.md", "lib.rs"]);
        assert_eq!(second.active_tab().map(|t| t.name.as_str()), Some("notes.md"));
        assert_eq!(second.tabs[1].doc.cursor(), Pos::new(1, 7));
        assert!(second.nodes.iter().any(|n| n.name == "src" && n.expanded), "its folder is open in the tree");
        assert_eq!(second.recent, std::slice::from_ref(&root));
        // Closing a tab is remembered; a file that has gone is passed over.
        second.update(Msg::Close(0));
        std::fs::write(dir.join("extra.txt"), "x\n").unwrap();
        let mut third = NeoCode::folder(&dir);
        third.config = Some(config.clone());
        third.restore_workspace();
        assert_eq!(third.tabs.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["lib.rs"]);
        open(&mut third, "extra.txt");
        std::fs::remove_file(dir.join("extra.txt")).unwrap();
        let mut fourth = NeoCode::folder(&dir);
        fourth.config = Some(config.clone());
        fourth.restore_workspace();
        assert_eq!(fourth.tabs.len(), 1);
        // Told not to, it starts with nothing open.
        std::fs::write(settings::user_file(&config), "{ \"window.restoreWorkspace\": false }").unwrap();
        let mut fifth = NeoCode::folder(&dir);
        fifth.config = Some(config.clone());
        fifth.load_settings();
        fifth.restore_workspace();
        assert!(fifth.tabs.is_empty());
        std::fs::remove_dir_all(config).unwrap();
    }

    #[test]
    fn settings_json_is_read_edited_in_a_tab_and_kept_up_by_the_menus() {
        let dir = project("settings-json");
        let config = dir.parent().unwrap().join(format!("{}-config", dir.file_name().unwrap().to_string_lossy()));
        let _ = std::fs::remove_dir_all(&config);
        let mut app = NeoCode::folder(&dir);
        // With nowhere to keep them, as in other tests, nothing is written.
        app.update(Msg::Keys(2));
        app.update(Msg::OpenSettings);
        assert!(app.tabs.is_empty() && !config.exists());
        app.config = Some(config.clone());
        app.load_settings();
        assert_eq!(app.settings, settings::Settings::default());
        // Opening the settings makes the file, with every setting in it.
        app.toast = None;
        app.update(Msg::OpenSettings);
        let tab = app.active_tab().expect("the settings in a tab");
        assert_eq!((tab.name.as_str(), tab.language), ("settings.json", Language::Json));
        assert!(tab.doc.text().contains("\"editor.tabSize\": 4"));
        app.update(Msg::OpenSettings);
        assert_eq!(app.tabs.len(), 1, "not twice");
        // Edit and save: the changes are in force at once.
        std::fs::write(settings::user_file(&config), "{\n  // two spaces here\n  \"editor.tabSize\": 2,\n  \"editor.keymap\": \"helix\",\n  \"workbench.colorTheme\": \"dark\",\n  \"editor.codeLens\": false\n}\n").unwrap();
        app.update(Msg::Close(0));
        app.update(Msg::OpenSettings);
        app.update(Msg::Save);
        assert_eq!((app.settings.tab_size, app.keymap, app.is_dark(), app.settings.code_lens), (2, Keymap::Helix, true, false));
        assert_eq!(app.active_tab().unwrap().doc.keymap(), Keymap::Helix, "open files follow");
        // Changing the keys from the menu writes it down, keeping the rest.
        app.update(Msg::Keys(0));
        app.update(Msg::ToggleScheme);
        let kept = settings::read(&settings::user_file(&config)).unwrap();
        assert_eq!(kept[settings::KEYMAP], json!(settings::keymap_name(KEYMAPS[0].0)));
        assert_eq!((kept[settings::THEME].clone(), kept["editor.tabSize"].clone()), (json!("light"), json!(2)));
        // A mistake in the file is reported, and what was in force stays.
        std::fs::write(settings::user_file(&config), "{ nonsense").unwrap();
        app.toast = None;
        app.load_settings();
        assert!(app.toast.as_deref().is_some_and(|t| t.contains("mistake")), "{:?}", app.toast);
        // The project's own settings win for the project.
        std::fs::write(settings::user_file(&config), "{ \"editor.tabSize\": 8 }").unwrap();
        std::fs::create_dir_all(dir.join(".neocode")).unwrap();
        std::fs::write(settings::folder_file(app.root.as_ref().unwrap()), "{ \"editor.tabSize\": 3 }").unwrap();
        app.load_settings();
        assert_eq!(app.settings.tab_size, 3);
        std::fs::remove_dir_all(config).unwrap();
    }

    #[test]
    fn a_program_is_debugged_with_breakpoints_steps_and_a_look_at_its_variables() {
        let dir = project("debugging");
        std::fs::write(dir.join("src/lib.rs"), "fn one() {}\nfn two() {\n    one();\n    one();\n}\n").unwrap();
        std::fs::write(dir.join("src/other.rs"), "fn elsewhere() {}\n").unwrap();
        let mut app = NeoCode::folder(&dir);
        let root = app.root.clone().unwrap();
        let (file, other) = (root.join("src/lib.rs"), root.join("src/other.rs"));
        let lib = app.nodes.iter().position(|n| n.name == "lib.rs").unwrap();
        app.update(Msg::Open(lib));
        let mut h = Harness::new(app, Size::new(1280.0, 820.0)).unwrap();
        // Breakpoints are set and cleared from the margin, or with F9.
        h.app_mut().update(Msg::Margin(3));
        h.app_mut().update(Msg::Margin(1));
        h.app_mut().update(Msg::Margin(3));
        assert_eq!(h.app().breakpoints_in(h.app().active_tab().unwrap()), [1]);
        let plain = h.render(1.0);
        h.key(Key::F(9), Modifiers::default());
        assert_eq!(h.app().breakpoints[&file], [0, 1], "F9: the caret's line");
        assert!(h.render(1.0) != plain, "a breakpoint shows in the margin");

        // A session, with the test playing the adapter.
        let sink = Sink::default();
        let known: Vec<(PathBuf, Vec<usize>)> = h.app().breakpoints.iter().map(|(p, l)| (p.clone(), l.clone())).collect();
        h.app_mut().debug_with("debug two", crate::dap::Dap::new(Box::new(sink.clone()), json!({ "program": "x" }), known));
        let id = h.app().debug.as_ref().unwrap().id;
        let says = |h: &mut Harness<NeoCode>, message: Value| h.app_mut().update(Msg::Dap(id, crate::dap::Incoming::Message(message)));
        let answer = |sent: &Value, body: Value| json!({ "type": "response", "request_seq": sent["seq"], "command": sent["command"], "success": true, "body": body });
        let hello = sink.take();
        says(&mut h, answer(&hello[0], json!({})));
        says(&mut h, json!({ "type": "event", "event": "initialized" }));
        let setup = sink.take();
        assert_eq!(setup.iter().map(|m| m["command"].as_str().unwrap()).collect::<Vec<_>>(), ["launch", "setBreakpoints", "configurationDone"]);
        assert_eq!(setup[1]["arguments"]["breakpoints"], json!([{ "line": 1 }, { "line": 2 }]));
        // The adapter moves one to where it can go, and the margin follows.
        says(&mut h, answer(&setup[1], json!({ "breakpoints": [{ "line": 1, "verified": true }, { "line": 3, "verified": true }] })));
        assert_eq!(h.app().breakpoints[&file], [0, 2]);
        says(&mut h, answer(&setup[2], json!({})));
        assert_eq!(h.app().debug.as_ref().unwrap().state, debug::State::Running);
        let running = h.render(1.0);

        // It stops: the stack is asked for, the place shown, the locals fetched.
        says(&mut h, json!({ "type": "event", "event": "stopped", "body": { "reason": "breakpoint", "threadId": 1 } }));
        let ask = sink.take();
        says(&mut h, answer(&ask[0], json!({ "stackFrames": [{ "id": 5, "name": "two", "line": 3, "source": { "path": file } }, { "id": 6, "name": "elsewhere", "line": 1, "source": { "path": other } }, { "id": 7, "name": "<runtime>", "line": 0 }] })));
        let session = h.app().debug.as_ref().unwrap();
        assert_eq!((session.summary().as_str(), session.frames.len(), session.frame), ("stopped: breakpoint", 3, 0));
        assert_eq!(h.app().stopped_in(h.app().active_tab().unwrap()), Some(2));
        assert_eq!(h.app().active_tab().unwrap().doc.cursor().line, 2, "the caret goes to where it stopped");
        assert!(h.render(1.0) != running, "the line and the stack show");
        let ask = sink.take();
        assert_eq!((ask[0]["command"].as_str(), ask[0]["arguments"]["frameId"].as_i64()), (Some("scopes"), Some(5)));
        says(&mut h, answer(&ask[0], json!({ "scopes": [{ "name": "Locals", "variablesReference": 1, "expensive": false }, { "name": "Heap", "variablesReference": 2, "expensive": true }] })));
        let ask = sink.take();
        assert_eq!(ask.len(), 1, "only what is cheap is fetched unasked");
        says(&mut h, answer(&ask[0], json!({ "variables": [{ "name": "n", "value": "7", "type": "Int", "variablesReference": 0 }, { "name": "xs", "value": "[1; 2]", "variablesReference": 9 }] })));
        let names = |h: &Harness<NeoCode>| h.app().debug.as_ref().unwrap().rows.iter().map(|r| (r.depth, r.name.clone(), r.value.clone())).collect::<Vec<_>>();
        assert_eq!(names(&h), [(0, "Locals".into(), String::new()), (1, "n".into(), "7".into()), (1, "xs".into(), "[1; 2]".into()), (0, "Heap".into(), String::new())]);
        // Opening a variable with parts fetches them; closing takes them away.
        h.app_mut().update(Msg::DebugRow(2));
        let ask = sink.take();
        assert_eq!(ask[0]["arguments"]["variablesReference"], 9);
        says(&mut h, answer(&ask[0], json!({ "variables": [{ "name": "0", "value": "1", "variablesReference": 0 }, { "name": "1", "value": "2", "variablesReference": 0 }] })));
        assert_eq!(names(&h).iter().map(|r| (r.0, r.1.as_str())).collect::<Vec<_>>(), [(0, "Locals"), (1, "n"), (1, "xs"), (2, "0"), (2, "1"), (0, "Heap")]);
        h.app_mut().update(Msg::DebugRow(2));
        assert_eq!(names(&h).len(), 4);
        h.app_mut().update(Msg::DebugRow(1));
        assert!(sink.take().is_empty(), "nothing inside a plain value");

        // Another frame: its file opens at its line, and its variables are asked for.
        h.app_mut().update(Msg::DebugFrame(1));
        assert_eq!((h.app().active_tab().unwrap().name.as_str(), h.app().stopped_in(h.app().active_tab().unwrap())), ("other.rs", Some(0)));
        assert_eq!(sink.take()[0]["arguments"]["frameId"], 6);
        // The keys step, and a breakpoint set now is sent at once.
        for (key, shift, command) in [(10, false, "next"), (11, false, "stepIn"), (11, true, "stepOut"), (5, false, "continue")] {
            h.key(Key::F(key), Modifiers { shift, ..Default::default() });
            assert_eq!(sink.take()[0]["command"], command);
        }
        h.app_mut().update(Msg::Margin(0));
        let sent = sink.take();
        assert_eq!((sent[0]["command"].as_str(), &sent[0]["arguments"]["source"]["path"]), (Some("setBreakpoints"), &json!(other)));
        // Carrying on clears what was shown of the stopped program.
        says(&mut h, json!({ "type": "event", "event": "continued", "body": { "threadId": 1 } }));
        let session = h.app().debug.as_ref().unwrap();
        assert!(session.frames.is_empty() && session.rows.is_empty() && session.state == debug::State::Running);
        h.key(Key::F(10), Modifiers::default());
        assert!(sink.take().is_empty(), "no stepping while it runs");
        // What it prints is kept, and its end is reported.
        says(&mut h, json!({ "type": "event", "event": "output", "body": { "category": "stdout", "output": "hello\nworld\n" } }));
        says(&mut h, json!({ "type": "event", "event": "exited", "body": { "exitCode": 0 } }));
        let session = h.app().debug.as_ref().unwrap();
        assert_eq!((session.output.text().as_str(), session.summary().as_str()), ("hello\nworld", "finished"));
        assert!(h.app().stopped_in(h.app().active_tab().unwrap()).is_none());
        h.key(Key::F(5), Modifiers { shift: true, ..Default::default() });
        assert!(h.app().debug.is_none());
    }

    #[test]
    fn ctrl_tab_brings_up_a_terminal_and_puts_it_away_again() {
        let ctrl = Modifiers { ctrl: true, ..Default::default() };
        let mut h = editing(0);
        h.type_text("x");
        let typed = h.app().active_tab().unwrap().doc.text();
        let plain = h.render(1.0);
        // From the editor, whatever its keys are doing.
        h.key(Key::Tab, ctrl);
        assert!(h.app().terminal_shown && h.app().terminal.is_some());
        assert_eq!(h.app().active_tab().unwrap().doc.text(), typed, "the editor did not take it for a tab");
        assert!(h.render(1.0) != plain, "the panel shows");
        assert!(h.app().menus()[4].entries.iter().any(|e| e.label.starts_with("Hide Terminal")));
        // Again puts it away, keeping the shell, and typing goes to the
        // editor once more without a click.
        h.key(Key::Tab, ctrl);
        assert!(!h.app().terminal_shown && h.app().terminal.is_some());
        h.render(1.0);
        h.type_text("y");
        assert_ne!(h.app().active_tab().unwrap().doc.text(), typed, "the editor has the keyboard back");
        // The close button and the menu do the same.
        h.app_mut().update(Msg::ToggleTerminal);
        assert!(h.app().terminal_shown);
        h.app_mut().update(Msg::ToggleTerminal);
        assert!(!h.app().terminal_shown);
        // A plain Tab is still the editor's.
        assert!(h.app().on_key(&KeyEvent { key: Key::Tab, pressed: true, repeat: false, modifiers: Modifiers::default(), text: None }).is_none());
    }

    #[test]
    fn the_keys_chosen_are_still_chosen_next_time() {
        let dir = project("keys-kept");
        let config = dir.parent().unwrap().join(format!("{}-config", dir.file_name().unwrap().to_string_lossy()));
        let _ = std::fs::remove_dir_all(&config);
        // As the app starts: settings read, then the folder's files reopened.
        let start = || {
            let mut app = NeoCode::folder(&dir);
            app.config = Some(config.clone());
            app.load_settings();
            app.restore_workspace();
            app
        };
        let open = |app: &mut NeoCode| {
            let lib = app.nodes.iter().position(|n| n.name == "lib.rs").unwrap();
            app.update(Msg::Open(lib));
        };
        let mut app = start();
        assert_eq!(app.keymap, Keymap::Vim, "the usual keys, with nothing saved");
        open(&mut app);
        // Each way of choosing: the settings panel and the View menu send
        // the same message; the button in the status bar goes to the next.
        for (choose, want) in [(2, Keymap::Helix), (0, Keymap::Plain), (1, Keymap::Vim), (2, Keymap::Helix)] {
            app.update(Msg::Keys(choose));
            assert_eq!(app.keymap, want);
            let again = start();
            assert_eq!(again.keymap, want, "a fresh start comes up with the keys last chosen");
            assert_eq!(again.active_tab().map(|t| t.doc.keymap()), Some(want), "and so do the files it reopens");
            assert_eq!(again.settings.keymap, want);
        }
        // The file says so in a word a person can read and change.
        let text = std::fs::read_to_string(settings::user_file(&config)).unwrap();
        assert!(text.contains("\"editor.keymap\": \"helix\""), "{text}");
        // Chosen while a different folder is open, it holds for every folder.
        let other = project("keys-kept-other");
        let mut elsewhere = NeoCode::folder(&other);
        elsewhere.config = Some(config.clone());
        elsewhere.load_settings();
        assert_eq!(elsewhere.keymap, Keymap::Helix);
        elsewhere.update(Msg::Keys(0));
        assert_eq!(start().keymap, Keymap::Plain);
        std::fs::remove_dir_all(config).unwrap();
    }

    #[test]
    fn a_function_with_arguments_asks_for_them_before_debugging() {
        let mut s = Served::new("debug-ask", "pub fn one() {}\n");
        let file = s.file.clone();
        let lens = |name: &str, params: u64| crate::lsp::Lens { line: 0, title: "▶ Debug".into(), command: "meadow.debugFunction".into(), arguments: json!([{ "uri": crate::lsp::client::uri(&file), "name": name, "params": params, "signature": "Int -> Bool", "line": 0 }]) };
        let plain = s.h.render(1.0);
        s.h.app_mut().debug_lens(&lens("isPrime", 1), Some("/opt/meadow".into()));
        assert!(s.h.app().debug.is_none(), "not yet");
        assert_eq!(s.h.app().asking.as_ref().map(|a| (a.name.as_str(), a.typed.as_str())), Some(("isPrime", "")));
        assert!(s.h.render(1.0) != plain, "the question shows");
        // Typed and entered: it starts on that expression.
        s.h.type_text("7");
        assert_eq!(s.h.app().asking.as_ref().unwrap().typed, "7", "the field has the keyboard");
        s.h.key(Key::Enter, Modifiers::default());
        assert!(s.h.app().asking.is_none());
        assert_eq!(s.h.app().debug.as_ref().map(|d| d.title.as_str()), Some("debug isPrime 7"));
        // Asked again, the same arguments are offered; Escape backs out.
        s.h.app_mut().update(Msg::Debug(debug::Step::Stop));
        s.h.app_mut().debug_lens(&lens("isPrime", 1), None);
        assert_eq!(s.h.app().asking.as_ref().unwrap().typed, "7");
        s.h.app_mut().update(Msg::DebugAskCancel);
        assert!(s.h.app().asking.is_none() && s.h.app().debug.is_none());
        // One that takes none starts at once.
        s.h.app_mut().debug_lens(&lens("main", 0), None);
        assert_eq!(s.h.app().debug.as_ref().map(|d| d.title.as_str()), Some("debug main"));
    }

    #[test]
    fn lenses_show_above_their_lines_and_run_what_they_offer() {
        let mut s = Served::new("lsp-lens", "pub fn one() {}\n\n#[test]\nfn adds() {\n    assert_eq!(1 + 1, 2);\n}\n");
        let ask = s.request("textDocument/codeLens");
        let plain = s.h.render(1.0);
        let run = json!({ "label": "test adds", "kind": "cargo", "args": { "cargoArgs": ["test", "--lib"], "executableArgs": ["adds", "--exact"], "cwd": s.file.parent().unwrap().parent().unwrap() } });
        let lens = |title: &str, command: &str, arguments: Value| json!({ "range": { "start": { "line": 3, "character": 0 }, "end": { "line": 3, "character": 7 } }, "command": { "title": title, "command": command, "arguments": arguments } });
        s.says(json!({ "id": ask["id"], "result": [
            lens("▶\u{fe0e} Run Test", "rust-analyzer.runSingle", json!([run.clone()])),
            lens("⚙\u{fe0e} Debug", "rust-analyzer.debugSingle", json!([run])),
            lens("Tidy", "tool.tidy", json!(["x"])),
            lens("Odd", "tool.unheard_of", json!([])),
            { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 3 } } },
        ] }));
        assert_eq!(s.h.app().editor_lenses(s.tab()), [EditorLens { line: 3, labels: vec!["Run Test".into(), "Debug".into(), "Tidy".into(), "Odd".into()] }], "one row over the line, without the symbols; one with no command is left out");
        assert!(s.h.render(1.0) != plain, "they show");
        // The text below the row moves down by a line, and the mouse finds
        // the labels: sweep for the first, which is a button.
        // Below the tabs, which are buttons too.
        let spot = (92..300).step_by(2).flat_map(|y| (300..700).step_by(4).map(move |x| Point::new(x as f32, y as f32))).find(|p| {
            s.h.move_to(*p);
            s.h.cursor() == neo::CursorIcon::Pointer
        });
        let spot = spot.expect("a lens to click");
        let caret = s.tab().doc.cursor();
        s.h.click(spot);
        assert_eq!(s.tab().doc.cursor(), caret, "a lens is a button, not somewhere to put the caret");
        let run = s.h.app().run.as_ref().expect("something running");
        assert_eq!(run.spec.line(), "cargo test --lib -- adds --exact");
        assert_eq!((run.spec.title.as_str(), run.status.clone()), ("test adds", runner::Status::Running));
        // What it prints shows under the editor, and how it ended.
        let id = run.id;
        let before = s.h.render(1.0);
        s.h.app_mut().update(Msg::RunSaid(id, "\u{1b}[32mtest adds ... ok\u{1b}[0m\n".into()));
        s.h.app_mut().update(Msg::RunSaid(id + 7, "from a run long gone\n".into()));
        s.h.app_mut().update(Msg::RunEnded(id, Some(0)));
        let run = s.h.app().run.as_ref().unwrap();
        assert_eq!(run.output.text(), "$ cargo test --lib -- adds --exact\ntest adds ... ok");
        assert!(run.ended_well() && run.summary() == "finished");
        assert!(s.h.render(1.0) != before);
        // Debug says there is no debugger; a command the server runs is
        // sent to it; one nobody knows says so.
        s.sent();
        // Debug builds it first, to debug with lldb-dap, or says that is missing.
        s.h.app_mut().toast = None;
        s.h.app_mut().update(Msg::Lens(3, 1));
        match &s.h.app().debug {
            Some(session) => assert_eq!((session.title.as_str(), session.summary().as_str()), ("debug test adds", "building…")),
            None => assert!(s.h.app().toast.as_deref().is_some_and(|t| t.contains("lldb-dap"))),
        }
        s.h.app_mut().update(Msg::Debug(debug::Step::Stop));
        assert!(s.h.app().debug.is_none());
        s.h.app_mut().update(Msg::Lens(3, 2));
        let sent = s.request("workspace/executeCommand");
        assert_eq!(sent["params"], json!({ "command": "tool.tidy", "arguments": ["x"] }));
        s.h.app_mut().update(Msg::Lens(3, 3));
        assert!(s.h.app().toast.as_deref().is_some_and(|t| t.contains("Odd") && t.contains("tool.unheard_of")));
        // Running again replaces what was there; closing takes it away.
        s.h.app_mut().update(Msg::RunAgain);
        assert!(s.h.app().run.as_ref().is_some_and(|r| r.id != id && r.status == runner::Status::Running));
        s.h.app_mut().update(Msg::RunStop);
        let stopped = s.h.app().run.as_ref().unwrap().id;
        s.h.app_mut().update(Msg::RunEnded(stopped, Some(9)));
        assert_eq!(s.h.app().run.as_ref().unwrap().status, runner::Status::Stopped, "stopped, whatever it exited with");
        s.h.app_mut().update(Msg::RunClose);
        assert!(s.h.app().run.is_none());
        // Editing asks for the lenses again, since lines have moved.
        s.sent();
        s.h.type_text("x");
        assert!(s.methods().contains(&"textDocument/codeLens".to_owned()));
    }

    #[test]
    fn code_in_a_hover_is_coloured_as_code() {
        let mut s = Served::new("lsp-hover-colour", "pub fn one() {}\n");
        let hover = |s: &mut Served, contents: Value| {
            s.h.app_mut().update(Msg::Ask(intel::Ask::Hover));
            let ask = s.request("textDocument/hover");
            s.says(json!({ "id": ask["id"], "result": { "contents": contents } }));
            s.h.app().editor_popup(s.tab())
        };
        let line = |text: &str, code| PopupLine { text: text.into(), code };
        // A fence names its language; prose around it stays prose.
        let rich = hover(&mut s, json!({ "kind": "markdown", "value": "```python\ndef one(): pass\n```\n\nReturns **one**." }));
        assert_eq!(rich, Some(EditorPopup::Rich(vec![line("def one(): pass", Some(Language::Python)), line("", None), line("Returns one.", None)])));
        // No language named, an unknown one, or plain text: the file's own.
        for contents in [json!({ "kind": "markdown", "value": "```\nfn one()\n```" }), json!({ "kind": "markdown", "value": "```nonsense\nfn one()\n```" }), json!({ "kind": "plaintext", "value": "fn one()" }), json!({ "language": "rust", "value": "fn one()" })] {
            assert_eq!(hover(&mut s, contents.clone()), Some(EditorPopup::Rich(vec![line("fn one()", Some(Language::Rust))])), "{contents}");
        }
        // The colours show: the same words as prose look different.
        let coloured = s.h.render(1.0);
        hover(&mut s, json!("fn one()"));
        assert_eq!(s.h.app().editor_popup(s.tab()), Some(EditorPopup::Text("fn one()".into())));
        assert!(s.h.render(1.0) != coloured);
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

    /// Opens a real file with whatever server this computer has for it and
    /// reports how that went, for trying a project by hand:
    /// `NEO_LSP_FILE=/path/to/file cargo test -p neo-code -- --ignored --nocapture probe_a_real_file`.
    /// `NEO_LSP_WAIT` is how many seconds to give the server (default 60).
    #[test]
    #[ignore]
    fn probe_a_real_file() {
        let file = PathBuf::from(std::env::var("NEO_LSP_FILE").expect("set NEO_LSP_FILE"));
        let secs: u64 = std::env::var("NEO_LSP_WAIT").ok().and_then(|s| s.parse().ok()).unwrap_or(60);
        // Start empty, so the search is known before the file opens.
        let mut h = Harness::new(NeoCode::sample(), Size::new(1100.0, 560.0)).unwrap();
        h.app_mut().update(Msg::LspDirs(lsp::search_dirs()));
        h.app_mut().update(Msg::Dropped(vec![file.clone()]));
        println!("project: {}", h.app().root.as_ref().unwrap().display());
        let until = std::time::Instant::now() + Duration::from_secs(secs);
        let mut last = None;
        while std::time::Instant::now() < until {
            std::thread::sleep(Duration::from_millis(200));
            h.advance(Duration::from_millis(200));
            let Some(t) = h.app().active_tab() else { break };
            let now = (h.app().server_status(t), t.diagnostics.len(), t.tokens.len());
            if last.as_ref() != Some(&now) {
                println!("{:>5.1}s  status: {:?}  problems: {}  coloured stretches: {}", (secs as f32) - until.saturating_duration_since(std::time::Instant::now()).as_secs_f32(), now.0.as_ref().map(|s| s.0.as_str()), now.1, now.2);
                last = Some(now);
            }
            if h.app().toast.is_some() {
                println!("        message: {}", h.app_mut().toast.take().unwrap());
            }
        }
        for lens in h.app().active_tab().map(|t| t.lenses.clone()).unwrap_or_default().iter().take(6) {
            println!("lens on line {}: {:?} runs {} with {}", lens.line + 1, lens.title, lens.command, lens.arguments);
        }
        // `NEO_LSP_LENS=title` chooses the first lens so labelled, and
        // waits for what it runs.
        if let Ok(wanted) = std::env::var("NEO_LSP_LENS") {
            let found = h.app().active_tab().and_then(|t| {
                let lens = t.lenses.iter().find(|l| runner::label(&l.title) == wanted)?;
                Some((lens.line, t.lenses.iter().filter(|l| l.line == lens.line).position(|l| l == lens)?))
            });
            let (line, index) = found.expect("a lens with that label");
            h.app_mut().runs_commands = true;
            h.app_mut().update(Msg::Edit(Action::Click { pos: Pos::new(line, 0), select: false }));
            h.app_mut().update(Msg::Lens(line, index));
            let until = std::time::Instant::now() + Duration::from_secs(240);
            while std::time::Instant::now() < until && h.app().run.as_ref().is_some_and(|r| r.status == runner::Status::Running) {
                std::thread::sleep(Duration::from_millis(200));
                h.advance(Duration::from_millis(200));
            }
            match &h.app().run {
                Some(run) => println!("ran: {}\n{}\n-> {}", run.spec.line(), run.output.text(), run.summary()),
                None => println!("nothing ran; message: {:?}", h.app().toast),
            }
        }
        // `NEO_LSP_DEBUG=function|arguments|line` debugs that function with
        // a breakpoint on that line (from one), steps once, and carries on.
        if let Ok(wanted) = std::env::var("NEO_LSP_DEBUG") {
            let mut parts = wanted.split('|');
            let (function, arguments, line) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default(), parts.next().and_then(|l| l.parse::<usize>().ok()));
            let lens = h.app().active_tab().and_then(|t| t.lenses.iter().find(|l| l.command.contains("debug") && (l.arguments[0]["name"] == function || l.arguments[0]["label"].as_str().is_some_and(|label| label.contains(function)))).cloned()).expect("a Debug lens for that function");
            h.app_mut().runs_commands = true;
            if let Some(line) = line {
                h.app_mut().update(Msg::Margin(line - 1));
            }
            let server = h.app().active_tab().and_then(|t| t.server).and_then(|s| h.app().servers.clients.get(s)).and_then(|(_, c)| c.program.clone());
            h.app_mut().debug_lens(&lens, server);
            if h.app().asking.is_some() {
                h.app_mut().update(Msg::DebugAskTyped(arguments.into()));
                h.app_mut().update(Msg::DebugAskGo);
            }
            let wait = |h: &mut Harness<NeoCode>, what: &str, done: &dyn Fn(&NeoCode) -> bool| {
                let until = std::time::Instant::now() + Duration::from_secs(240);
                while std::time::Instant::now() < until && !done(h.app()) {
                    std::thread::sleep(Duration::from_millis(100));
                    h.advance(Duration::from_millis(100));
                }
                let d = h.app().debug.as_ref();
                println!("{what}: {:?}; message: {:?}", d.map(|d| d.summary()), h.app().toast);
                for f in d.map(|d| d.frames.clone()).unwrap_or_default().iter().take(4) {
                    println!("    frame {} at {:?}:{}", f.name, f.path.as_ref().and_then(|p| p.file_name()), f.line + 1);
                }
                for r in d.map(|d| d.rows.clone()).unwrap_or_default().iter().take(8) {
                    println!("    {}{} = {} {:?}", "  ".repeat(r.depth), r.name, r.value, r.kind);
                }
            };
            let stopped_with_rows = |a: &NeoCode| a.debug.as_ref().is_none_or(|d| d.over() || (d.stopped() && d.rows.iter().any(|r| r.depth > 0)));
            wait(&mut h, "first stop", &stopped_with_rows);
            if let Some(out) = std::env::var_os("NEO_SNAPSHOT_DIR") {
                h.app_mut().toast = None;
                h.save_png(PathBuf::from(out).join("code-debug.png"), 1.0).unwrap();
            }
            for (name, step) in [("after Step Over", debug::Step::Over), ("after Continue", debug::Step::Go), ("after another Continue", debug::Step::Go)] {
                if h.app().debug.as_ref().is_some_and(|d| d.stopped()) {
                    h.app_mut().update(Msg::Debug(step));
                    // Give the step a moment to be answered, then wait for
                    // where it comes to rest.
                    for _ in 0..15 {
                        std::thread::sleep(Duration::from_millis(100));
                        h.advance(Duration::from_millis(100));
                    }
                    wait(&mut h, name, &stopped_with_rows);
                }
            }
            println!("printed:\n{}", h.app().debug.as_ref().map(|d| d.output.text()).unwrap_or_default());
            h.app_mut().update(Msg::Debug(debug::Step::Stop));
        }
        // `NEO_TERMINAL=command` brings up the terminal and types that in.
        if let Ok(command) = std::env::var("NEO_TERMINAL") {
            h.app_mut().runs_commands = true;
            h.app_mut().update(Msg::ToggleTerminal);
            for i in 0..30 {
                std::thread::sleep(Duration::from_millis(100));
                h.advance(Duration::from_millis(100));
                h.render(1.0);
                if i == 12 {
                    h.app().terminal.as_ref().unwrap().write(format!("{command}\n"));
                }
            }
            let shell = h.app().terminal.as_ref().unwrap();
            println!("terminal: running {}, in {}, error {:?}", shell.running(), shell.cwd.display(), shell.error);
        }
        // `NEO_LSP_HOVER=line:col` (from zero) rests the mouse there first.
        if let Some((line, col)) = std::env::var("NEO_LSP_HOVER").ok().and_then(|at| at.split_once(':').and_then(|(l, c)| Some((l.parse().ok()?, c.parse().ok()?)))) {
            h.app_mut().update(Msg::Point(Some(Pos::new(line, col))));
            std::thread::sleep(NeoCode::REST);
            h.app_mut().update(Msg::HoverTick);
            for _ in 0..25 {
                std::thread::sleep(Duration::from_millis(200));
                h.advance(Duration::from_millis(200));
            }
            println!("hover: {:?}", h.app().active_tab().and_then(|t| h.app().editor_popup(t)));
        }
        if let Some(out) = std::env::var_os("NEO_SNAPSHOT_DIR") {
            h.app_mut().toast = None;
            h.save_png(PathBuf::from(out).join("code-probe.png"), 1.0).unwrap();
        }
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
