//! Debugging: breakpoints, stepping, and looking at a stopped program.
//!
//! The work is done by a debug adapter ([`crate::dap`]). This is what
//! NeoCode keeps and shows for a session: where the program is, what
//! called what, what the variables hold, and what it has printed.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::*;
use crate::dap::{Dap, Event, Frame, Incoming};
use crate::lsp::Lens;
use crate::runner::RunSpec;

/// How a session stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// Being built, or the adapter is getting ready.
    Starting(String),
    Running,
    /// Stopped, and why.
    Stopped(String),
    Ended(String),
}

/// A line in the variables list: a scope, a variable, or a part of one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub depth: usize,
    pub name: String,
    pub value: String,
    pub kind: Option<String>,
    /// What to ask the adapter for to see inside; zero if nothing is.
    pub reference: i64,
    pub open: bool,
}

/// What to start debugging, once anything it needs has been asked for
/// or built.
#[derive(Clone, Debug, PartialEq)]
pub struct DebugSpec {
    pub title: String,
    pub adapter: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// The adapter's own description of what to run.
    pub launch: Value,
}

/// A function that takes arguments, waiting for them to be typed.
#[derive(Clone, Debug, PartialEq)]
pub struct Asking {
    pub file: PathBuf,
    pub name: String,
    pub signature: String,
    pub params: u64,
    pub typed: String,
}

pub struct Session {
    pub id: u64,
    pub title: String,
    dap: Option<Dap>,
    pub state: State,
    pub frames: Vec<Frame>,
    /// Which frame is being looked at.
    pub frame: usize,
    pub rows: Vec<Row>,
    /// What the program and the adapter have printed.
    pub output: Document,
    printed: usize,
}

impl Session {
    fn new(id: u64, title: String, dap: Option<Dap>, state: State) -> Self {
        Self { id, title, dap, state, frames: vec![], frame: 0, rows: vec![], output: Document::new(""), printed: 0 }
    }

    pub fn stopped(&self) -> bool {
        matches!(self.state, State::Stopped(_))
    }

    pub fn over(&self) -> bool {
        matches!(self.state, State::Ended(_))
    }

    /// Where the frame being looked at is.
    pub fn place(&self) -> Option<(&Path, usize)> {
        let frame = self.frames.get(self.frame).filter(|_| self.stopped())?;
        Some((frame.path.as_deref()?, frame.line))
    }

    fn print(&mut self, text: &str) {
        for line in text.trim_end_matches('\n').split('\n') {
            self.printed += 1;
            if self.printed > 5000 {
                return;
            }
            let line = crate::runner::plain(line);
            self.output.apply(Action::Move { motion: Motion::DocEnd, select: false });
            self.output.apply(Action::Insert(if self.printed == 1 { line } else { format!("\n{line}") }));
        }
    }

    /// One line saying how it stands.
    pub fn summary(&self) -> String {
        match &self.state {
            State::Starting(what) => what.clone(),
            State::Running => "running…".into(),
            State::Stopped(why) => format!("stopped: {why}"),
            State::Ended(how) => how.clone(),
        }
    }
}

/// Something to do to the program being debugged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Start, or carry on from where it is stopped.
    Go,
    Pause,
    Over,
    In,
    Out,
    Stop,
}

/// The package a Meadow file belongs to, as the adapter wants it named.
fn meadow_program(file: &Path) -> PathBuf {
    file.ancestors().skip(1).find(|d| d.join("Meadow.toml").exists() || d.join("meadow.toml").exists()).map_or_else(|| file.to_path_buf(), Path::to_path_buf)
}

/// What to tell Meadow's adapter to run: the whole program, or one
/// expression in a module, stopping as its function is entered.
pub fn meadow_spec(meadow: &Path, file: &Path, entry: Option<(&str, &str)>) -> DebugSpec {
    let program = meadow_program(file);
    let cwd = if program.is_dir() { program.clone() } else { file.parent().map_or_else(|| program.clone(), Path::to_path_buf) };
    let mut launch = json!({ "type": "meadow", "request": "launch", "name": "Debug Meadow", "program": program });
    let title = match entry {
        Some((function, expression)) => {
            launch["entry"] = json!({ "module": file, "expression": expression, "function": function });
            format!("debug {expression}")
        }
        None => format!("debug {}", program.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
    };
    DebugSpec { title, adapter: meadow.to_path_buf(), args: vec!["dap".into()], cwd, launch }
}

/// The command that builds what rust-analyzer's Debug lens names without
/// running it, and reports where the result is. The first word of its
/// cargo arguments says what kind of thing it is.
pub fn cargo_build(arguments: &Value, root: &Path) -> Option<(RunSpec, Vec<String>)> {
    let args = &arguments[0]["args"];
    let strings = |v: &Value| -> Vec<String> { v.as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect()).unwrap_or_default() };
    let mut cargo = strings(&args["cargoArgs"]);
    match cargo.first().map(String::as_str) {
        Some("test" | "bench") => cargo.push("--no-run".into()),
        Some("run") => cargo[0] = "build".into(),
        _ => return None,
    }
    cargo.push("--message-format=json".into());
    let cwd = args["cwd"].as_str().or_else(|| args["workspaceRoot"].as_str()).map_or_else(|| root.to_path_buf(), PathBuf::from);
    let env = args["environment"].as_object().map(|o| o.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned()))).collect()).unwrap_or_default();
    let title = arguments[0]["label"].as_str().unwrap_or("program").to_owned();
    Some((RunSpec { title, program: args["overrideCargo"].as_str().map_or_else(|| PathBuf::from("cargo"), PathBuf::from), args: cargo, cwd, env }, strings(&args["executableArgs"])))
}

/// The program cargo built, from what it printed with
/// `--message-format=json`: the last thing it made that can be run.
pub fn built_program(output: &str) -> Option<PathBuf> {
    output.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(|m| m["reason"] == "compiler-artifact").filter_map(|m| m["executable"].as_str().map(PathBuf::from)).next_back()
}

/// What the compiler said was wrong, from the same output, for when
/// nothing was built.
pub fn build_errors(output: &str) -> String {
    output.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(|m| m["reason"] == "compiler-message" && m["message"]["level"] == "error").filter_map(|m| m["message"]["rendered"].as_str().map(str::to_owned)).collect::<Vec<_>>().join("\n")
}

/// What to tell `lldb-dap` to run.
pub fn lldb_spec(adapter: &Path, title: &str, program: &Path, args: &[String], cwd: &Path) -> DebugSpec {
    DebugSpec { title: format!("debug {title}"), adapter: adapter.to_path_buf(), args: vec![], cwd: cwd.to_path_buf(), launch: json!({ "type": "lldb-dap", "request": "launch", "name": title, "program": program, "args": args, "cwd": cwd, "stopOnEntry": false }) }
}

impl NeoCode {
    /// The lines with breakpoints in a file.
    pub(crate) fn breakpoints_in(&self, t: &Tab) -> Vec<usize> {
        t.disk.as_ref().and_then(|d| self.breakpoints.get(d)).cloned().unwrap_or_default()
    }

    /// The line the program is stopped at, if it is in this file.
    pub(crate) fn stopped_in(&self, t: &Tab) -> Option<usize> {
        let (path, line) = self.debug.as_ref()?.place()?;
        (t.disk.as_deref() == Some(path)).then_some(line)
    }

    /// Sets or clears the breakpoint on a line of the file showing.
    pub(crate) fn toggle_breakpoint(&mut self, line: usize) {
        let Some(disk) = self.active_tab().and_then(|t| t.disk.clone()) else {
            self.toast = Some("Breakpoints go in files that are on disk.".into());
            return;
        };
        let lines = self.breakpoints.entry(disk.clone()).or_default();
        match lines.iter().position(|l| *l == line) {
            Some(i) => {
                lines.remove(i);
            }
            None => {
                lines.push(line);
                lines.sort_unstable();
            }
        }
        let lines = lines.clone();
        if let Some(dap) = self.debug.as_mut().and_then(|s| s.dap.as_mut()) {
            dap.set_breakpoints(&disk, &lines);
        }
    }

    /// A Debug lens was chosen.
    pub(crate) fn debug_lens(&mut self, lens: &Lens, server: Option<PathBuf>) {
        let first = &lens.arguments[0];
        match lens.command.as_str() {
            "meadow.debugFunction" => {
                let (Some(name), Some(file)) = (first["name"].as_str(), first["uri"].as_str().and_then(crate::lsp::client::path_of)) else { return };
                let params = first["params"].as_u64().unwrap_or(0);
                let meadow = server.unwrap_or_else(|| PathBuf::from("meadow"));
                self.meadow = Some(meadow.clone());
                if params == 0 {
                    self.start_debugging(meadow_spec(&meadow, &file, Some((name, name))));
                } else {
                    // It needs arguments: ask, offering the ones used last.
                    let typed = self.debug_args.get(&(file.clone(), name.to_owned())).cloned().unwrap_or_default();
                    self.asking = Some(Asking { file, name: name.to_owned(), signature: first["signature"].as_str().unwrap_or_default().to_owned(), params, typed });
                }
            }
            "rust-analyzer.debugSingle" => {
                let root = self.root.clone().unwrap_or_default();
                let Some((build, args)) = cargo_build(&lens.arguments, &root) else {
                    self.toast = Some("NeoCode doesn't know how to build that to debug it.".into());
                    return;
                };
                // Apple's own first on a Mac: it is the one allowed to
                // take hold of another process there.
                let apple = [PathBuf::from("/Library/Developer/CommandLineTools/usr/bin/lldb-dap"), PathBuf::from("/Applications/Xcode.app/Contents/Developer/usr/bin/lldb-dap")];
                let Some(adapter) = apple.into_iter().find(|p| cfg!(target_os = "macos") && p.is_file()).or_else(|| self.find_program(&["lldb-dap", "lldb-vscode"])) else {
                    self.toast = Some("Debugging Rust needs lldb-dap, which was not found. It comes with Xcode's command line tools and with LLVM.".into());
                    return;
                };
                self.build_then_debug(build, args, adapter);
            }
            _ => self.toast = Some(format!("NeoCode doesn't know how to start \"{}\" ({}).", crate::runner::label(&lens.title), lens.command)),
        }
    }

    /// A program by any of these names, in the folders servers are looked for in.
    fn find_program(&self, names: &[&str]) -> Option<PathBuf> {
        let dirs = self.servers.dirs.as_deref().unwrap_or_default();
        names.iter().flat_map(|n| dirs.iter().map(move |d| d.join(n))).find(|p| p.is_file())
    }

    /// The arguments have been typed: start, and remember them.
    pub(crate) fn debug_asked(&mut self) {
        let Some(ask) = self.asking.take() else { return };
        self.debug_args.insert((ask.file.clone(), ask.name.clone()), ask.typed.clone());
        let expression = format!("{} {}", ask.name, ask.typed.trim());
        let meadow = self.meadow.clone().unwrap_or_else(|| PathBuf::from("meadow"));
        self.start_debugging(meadow_spec(&meadow, &ask.file, Some((&ask.name, expression.trim()))));
    }

    /// Builds with cargo, off to one side, then debugs what it made.
    fn build_then_debug(&mut self, build: RunSpec, args: Vec<String>, adapter: PathBuf) {
        self.end_debugging();
        self.debugs += 1;
        let id = self.debugs;
        let mut session = Session::new(id, format!("debug {}", build.title), None, State::Starting("building…".into()));
        session.print(&format!("$ {}", build.line()));
        self.debug = Some(session);
        let (Some(proxy), true) = (self.servers.proxy.clone(), self.runs_commands) else { return };
        let dirs = self.servers.dirs.clone().unwrap_or_default();
        std::thread::spawn(move || {
            let mut command = std::process::Command::new(&build.program);
            if let Ok(path) = std::env::join_paths(&dirs)
                && !path.is_empty()
            {
                command.env("PATH", path);
            }
            let built = command.args(&build.args).current_dir(&build.cwd).envs(build.env.iter().cloned()).stdin(std::process::Stdio::null()).output();
            let result = match built {
                Err(e) => Err(format!("Couldn't run cargo: {e}")),
                Ok(out) => {
                    let text = String::from_utf8_lossy(&out.stdout);
                    match built_program(&text).filter(|_| out.status.success()) {
                        Some(program) => Ok(lldb_spec(&adapter, &build.title, &program, &args, &build.cwd)),
                        None => Err(Some(build_errors(&text)).filter(|e| !e.is_empty()).unwrap_or_else(|| String::from_utf8_lossy(&out.stderr).into_owned())),
                    }
                }
            };
            proxy.send(Msg::DebugBuilt(id, result));
        });
    }

    /// The build is done: debug it, or say what went wrong.
    pub(crate) fn debug_built(&mut self, id: u64, result: Result<DebugSpec, String>) {
        if self.debug.as_ref().is_none_or(|s| s.id != id) {
            return;
        }
        match result {
            Ok(spec) => self.start_debugging(spec),
            Err(why) => {
                let session = self.debug.as_mut().expect("checked above");
                session.print(&why);
                session.state = State::Ended("it did not build".into());
            }
        }
    }

    /// Starts a session, in place of any there was.
    pub(crate) fn start_debugging(&mut self, spec: DebugSpec) {
        // The adapter builds from what is on disk.
        for i in 0..self.tabs.len() {
            if self.tabs[i].dirty() && self.tabs[i].disk.is_some() {
                let was = self.active;
                self.active = Some(i);
                self.apply(Msg::Save);
                self.active = was;
            }
        }
        self.end_debugging();
        self.asking = None;
        self.debugs += 1;
        let id = self.debugs;
        let breakpoints: Vec<(PathBuf, Vec<usize>)> = self.breakpoints.iter().filter(|(_, lines)| !lines.is_empty()).map(|(p, l)| (p.clone(), l.clone())).collect();
        let mut session = Session::new(id, spec.title.clone(), None, State::Starting("starting…".into()));
        // With no event loop to report back to, as in tests, nothing is started.
        if let Some(proxy) = self.servers.proxy.clone().filter(|_| self.runs_commands) {
            let dirs = self.servers.dirs.clone().unwrap_or_default();
            match Dap::spawn(&spec.adapter, &spec.args, &spec.cwd, &dirs, spec.launch.clone(), breakpoints, move |m| {
                proxy.send(Msg::Dap(id, m));
            }) {
                Ok(dap) => session.dap = Some(dap),
                Err(e) => {
                    session.print(&format!("Couldn't start {}: {e}", spec.adapter.display()));
                    session.state = State::Ended("the debugger would not start".into());
                }
            }
        }
        self.debug = Some(session);
    }

    /// A session with an adapter already to hand, for tests to play the
    /// other side of.
    #[cfg(test)]
    pub(crate) fn debug_with(&mut self, title: &str, dap: Dap) {
        self.debugs += 1;
        self.debug = Some(Session::new(self.debugs, title.into(), Some(dap), State::Starting("starting…".into())));
    }

    /// Ends the session, and the program with it.
    pub(crate) fn end_debugging(&mut self) {
        if let Some(mut session) = self.debug.take()
            && let Some(dap) = &mut session.dap
        {
            dap.stop();
        }
    }

    /// Starts debugging the file showing from the top: for a language
    /// with a debugger of its own that needs nothing asked.
    fn debug_this_file(&mut self) {
        let Some((file, language, server)) = self.active_tab().and_then(|t| Some((t.disk.clone()?, t.language, t.server))) else {
            self.toast = Some("Open a file to debug it.".into());
            return;
        };
        match language {
            Language::Meadow => {
                let meadow = server.and_then(|s| self.servers.clients.get(s)).and_then(|(_, c)| c.program.clone()).or_else(|| self.find_program(&["meadow"])).unwrap_or_else(|| PathBuf::from("meadow"));
                self.start_debugging(meadow_spec(&meadow, &file, None));
            }
            Language::Rust => self.toast = Some("Use a Debug button above a test or main to debug Rust.".into()),
            other => self.toast = Some(format!("NeoCode has no debugger for {} yet.", other.name())),
        }
    }

    pub(crate) fn debug_step(&mut self, step: Step) {
        let Some(session) = &mut self.debug else {
            if step == Step::Go {
                self.debug_this_file();
            }
            return;
        };
        if step == Step::Stop || (step == Step::Go && session.over()) {
            let again = step == Step::Go;
            self.end_debugging();
            if again {
                self.debug_this_file();
            }
            return;
        }
        let stopped = session.stopped();
        let Some(dap) = &mut session.dap else { return };
        match step {
            Step::Pause if !stopped => dap.pause(),
            Step::Go if stopped => dap.resume(),
            Step::Over if stopped => dap.step_over(),
            Step::In if stopped => dap.step_in(),
            Step::Out if stopped => dap.step_out(),
            _ => {}
        }
    }

    /// Looks at another frame of the stack.
    pub(crate) fn debug_frame(&mut self, index: usize) {
        let Some(session) = self.debug.as_mut().filter(|s| index < s.frames.len()) else { return };
        session.frame = index;
        session.rows.clear();
        let frame = session.frames[index].clone();
        if let Some(dap) = &mut session.dap {
            dap.scopes(frame.id);
        }
        if let Some(path) = &frame.path {
            self.show_place(path, frame.line);
        }
    }

    /// Opens a file with the caret on a line, wherever the file is.
    fn show_place(&mut self, path: &Path, line: usize) {
        if !self.show_file(path) {
            return;
        }
        if let Some(t) = self.active.and_then(|i| self.tabs.get_mut(i)) {
            let line = line.min(t.doc.line_count().saturating_sub(1));
            t.doc.apply(Action::Click { pos: Pos::new(line, 0), select: false });
        }
    }

    /// Opens or closes a row of the variables list.
    pub(crate) fn debug_row(&mut self, index: usize) {
        let Some(session) = &mut self.debug else { return };
        let Some(row) = session.rows.get_mut(index).filter(|r| r.reference > 0) else { return };
        row.open = !row.open;
        let (open, reference, depth) = (row.open, row.reference, row.depth);
        // What was inside goes either way: closed, or about to be asked for again.
        let inside = session.rows[index + 1..].iter().take_while(|r| r.depth > depth).count();
        session.rows.drain(index + 1..index + 1 + inside);
        if let (true, Some(dap)) = (open, &mut session.dap) {
            dap.variables(reference);
        }
    }

    /// Word from the adapter, or that it has gone.
    pub(crate) fn debug_incoming(&mut self, id: u64, incoming: Incoming) {
        let Some(session) = self.debug.as_mut().filter(|s| s.id == id) else { return };
        let events = match incoming {
            Incoming::Message(message) => session.dap.as_mut().map(|d| d.handle(&message)).unwrap_or_default(),
            Incoming::Closed => {
                if !session.over() {
                    session.state = State::Ended("ended".into());
                }
                session.dap = None;
                return;
            }
        };
        for event in events {
            self.debug_event(event);
        }
    }

    fn debug_event(&mut self, event: Event) {
        let Some(session) = &mut self.debug else { return };
        match event {
            Event::Running => {
                if !session.over() {
                    session.state = State::Running;
                    session.frames.clear();
                    session.rows.clear();
                }
            }
            Event::Stopped { reason, .. } => session.state = State::Stopped(reason),
            Event::Stack(frames) => {
                session.frames = frames;
                // The first frame with a file to show is the one to look at.
                let first = session.frames.iter().position(|f| f.path.as_ref().is_some_and(|p| p.is_file())).unwrap_or(0);
                self.debug_frame(first);
            }
            Event::Scopes { frame, scopes } => {
                if session.frames.get(session.frame).is_none_or(|f| f.id != frame) {
                    return;
                }
                // Only the first that is cheap to fetch is opened without
                // being asked: the locals, with every adapter. The rest,
                // globals and registers and the like, wait to be asked for.
                let first = scopes.iter().position(|s| !s.expensive);
                session.rows = scopes.iter().enumerate().map(|(i, s)| Row { depth: 0, name: s.name.clone(), value: String::new(), kind: None, reference: s.reference, open: Some(i) == first }).collect();
                if let (Some(dap), Some(first)) = (&mut session.dap, first) {
                    dap.variables(scopes[first].reference);
                }
            }
            Event::Variables { reference, variables } => {
                let Some(at) = session.rows.iter().position(|r| r.reference == reference && r.open) else { return };
                let depth = session.rows[at].depth;
                let inside = session.rows[at + 1..].iter().take_while(|r| r.depth > depth).count();
                let rows = variables.into_iter().map(|v| Row { depth: depth + 1, name: v.name, value: v.value, kind: v.kind, reference: v.reference, open: false });
                session.rows.splice(at + 1..at + 1 + inside, rows);
            }
            Event::Breakpoints { path, lines } => {
                self.breakpoints.insert(path, lines);
            }
            Event::Output(text) => session.print(&text),
            Event::Ended(code) => {
                session.state = State::Ended(match code {
                    Some(0) | None => "finished".into(),
                    Some(code) => format!("ended with exit code {code}"),
                });
                session.frames.clear();
                session.rows.clear();
            }
            Event::Failed(why) => {
                session.print(&why);
                session.state = State::Ended("could not start".into());
                session.dap = None;
            }
            Event::Message(why) => self.toast = Some(why),
        }
    }

    /// The question asked before debugging a function that takes arguments.
    pub(crate) fn asking_panel(&self, ask: &Asking) -> Element<Msg> {
        let hint = if ask.params == 1 { "one argument".to_owned() } else { format!("{} arguments", ask.params) };
        let head = row()
            .spacing(8.0)
            .align(Align::Center)
            .width(Length::Fill)
            .push(icon(icons::BUG).size(14.0).tone(Tone::Accent))
            .push(text(format!("Debug {}", ask.name)).role(TextRole::Strong).no_wrap())
            .push(container(text(format!("{} : {}", ask.name, ask.signature)).mono().role(TextRole::Caption).tone(Tone::Muted).no_wrap()).width(Length::Fill))
            .push(icon_button(icons::X, 22.0).kind(ButtonKind::Ghost).on_press(Msg::DebugAskCancel));
        let field = row()
            .spacing(8.0)
            .align(Align::Center)
            .push(text_input(format!("{hint}, as you would write them after {}", ask.name), ask.typed.clone()).on_input(Msg::DebugAskTyped).on_submit(Msg::DebugAskGo).on_cancel(Msg::DebugAskCancel).autofocus(true).width(Length::Fill))
            .push(button("Debug").kind(ButtonKind::Accent).on_press(Msg::DebugAskGo));
        column().spacing(8.0).padding([12.0, 8.0, 12.0, 12.0]).width(Length::Fill).push(head).push(field).into()
    }

    /// The session: the controls, the stack, the variables, and what the
    /// program has printed.
    pub(crate) fn debug_panel(&self, session: &Session, surface: Color) -> Element<Msg> {
        let stopped = session.stopped();
        let tone = match &session.state {
            State::Stopped(_) => Tone::Warn,
            State::Ended(_) => Tone::Muted,
            _ => Tone::Accent,
        };
        let step = |glyph, label: &str, step: Step, on: bool| {
            Button::new(row().spacing(5.0).align(Align::Center).push(icon(glyph).size(13.0)).push(text(label.to_owned()).role(TextRole::Caption).no_wrap())).kind(ButtonKind::Ghost).padding([8.0, 3.0]).radius(6.0).on_press_maybe(on.then_some(Msg::Debug(step)))
        };
        let mut head = row()
            .spacing(4.0)
            .align(Align::Center)
            .width(Length::Fill)
            .push(icon(icons::BUG).size(14.0).tone(tone))
            .push(Space::new(4.0, 0.0))
            .push(text(session.title.clone()).role(TextRole::Strong).no_wrap())
            .push(Space::new(4.0, 0.0))
            .push(container(text(session.summary()).role(TextRole::Caption).tone(tone).no_wrap()).width(Length::Fill));
        if !session.over() {
            head = if stopped { head.push(step(icons::PLAY, "Continue", Step::Go, true)) } else { head.push(step(icons::PAUSE, "Pause", Step::Pause, session.state == State::Running)) };
            head = head.push(step(icons::REDO_DOT, "Step Over", Step::Over, stopped)).push(step(icons::ARROW_DOWN_TO_LINE, "Step Into", Step::In, stopped)).push(step(icons::ARROW_UP_FROM_LINE, "Step Out", Step::Out, stopped));
        }
        head = head.push(step(icons::SQUARE, if session.over() { "Close" } else { "Stop" }, Step::Stop, true));

        // What called what, the innermost first.
        let mut stack = column().spacing(1.0).width(Length::Fill).push(section_label("Call stack"));
        for (i, f) in session.frames.iter().enumerate() {
            let place = f.path.as_ref().and_then(|p| p.file_name()).map(|n| format!("{}:{}", n.to_string_lossy(), f.line + 1)).unwrap_or_default();
            let line = row().spacing(8.0).align(Align::Center).width(Length::Fill).push(text(f.name.clone()).mono().role(TextRole::Caption).no_wrap().width(Length::Fill)).push(text(place).role(TextRole::Caption).tone(Tone::Faint).no_wrap());
            stack = stack.push(Button::new(line).kind(ButtonKind::Ghost).selected(i == session.frame).width(Length::Fill).padding([8.0, 3.0]).radius(5.0).on_press(Msg::DebugFrame(i)));
        }
        if session.frames.is_empty() {
            stack = stack.push(container(text(if session.over() { "The program has ended." } else { "Shown when the program stops." }).role(TextRole::Caption).tone(Tone::Faint)).padding([8.0, 3.0]));
        }

        let mut vars = column().spacing(1.0).width(Length::Fill).push(section_label("Variables"));
        for (i, r) in session.rows.iter().enumerate() {
            let twist: Element<Msg> = if r.reference > 0 { icon(if r.open { icons::CHEVRON_DOWN } else { icons::CHEVRON_RIGHT }).size(12.0).tone(Tone::Muted).into() } else { Space::new(12.0, 0.0).into() };
            let mut line = row().spacing(6.0).align(Align::Center).width(Length::Fill).push(Space::new(r.depth as f32 * 14.0, 0.0)).push(twist);
            line = if r.depth == 0 { line.push(text(r.name.clone()).role(TextRole::Caption).tone(Tone::Muted).no_wrap()) } else { line.push(text(r.name.clone()).mono().role(TextRole::Caption).tone(Tone::Accent).no_wrap()).push(text(format!("= {}", r.value)).mono().role(TextRole::Caption).no_wrap().width(Length::Fill)) };
            if let Some(kind) = &r.kind {
                line = line.push(text(kind.clone()).mono().role(TextRole::Caption).tone(Tone::Faint).no_wrap());
            }
            // Only what has something inside is a button; a button that
            // does nothing would be drawn faded.
            vars = if r.reference > 0 { vars.push(Button::new(line).kind(ButtonKind::Ghost).width(Length::Fill).padding([8.0, 2.0]).radius(5.0).on_press(Msg::DebugRow(i))) } else { vars.push(container(line).width(Length::Fill).padding([8.0, 2.0])) };
        }

        let body = row()
            .width(Length::Fill)
            .height(Length::Fill)
            .push(container(scrollable(stack)).width(250.0).height(Length::Fill).padding([4.0, 0.0]))
            .push(Divider::vertical())
            .push(container(scrollable(vars)).width(Length::Fill).height(Length::Fill).padding([4.0, 0.0]))
            .push(Divider::vertical())
            .push(container(text_editor(&session.output).language(Language::Plain).into_element_keyed("debug-output")).background(Background::Color(surface)).width(340.0).height(Length::Fill));
        column().width(Length::Fill).height(250.0).push(container(head).padding([10.0, 4.0]).width(Length::Fill)).push(Divider::horizontal()).push(body).into()
    }
}

fn section_label(label: &str) -> Element<Msg> {
    container(text(label.to_uppercase()).role(TextRole::Label).tone(Tone::Faint)).padding([8.0, 4.0]).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meadow_is_told_what_to_run_and_where_to_stop() {
        let dir = std::env::temp_dir().join(format!("neo-code-debug-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("pkg/src")).unwrap();
        std::fs::write(dir.join("pkg/Meadow.toml"), "").unwrap();
        let file = dir.join("pkg/src/Main.mw");
        let meadow = Path::new("/home/sam/.meadow/bin/meadow");
        let spec = meadow_spec(meadow, &file, Some(("isPrime", "isPrime 7")));
        assert_eq!((spec.adapter.as_path(), spec.args.as_slice(), spec.cwd.clone(), spec.title.as_str()), (meadow, &["dap".to_owned()][..], dir.join("pkg"), "debug isPrime 7"));
        assert_eq!(spec.launch, json!({ "type": "meadow", "request": "launch", "name": "Debug Meadow", "program": dir.join("pkg"), "entry": { "module": file, "expression": "isPrime 7", "function": "isPrime" } }));
        // The whole program: no entry, so it starts at main.
        let whole = meadow_spec(meadow, &file, None);
        assert!(whole.launch.get("entry").is_none());
        assert_eq!(whole.title, "debug pkg");
        // A file on its own is its own program.
        let loose = dir.join("loose.mw");
        assert_eq!(meadow_spec(meadow, &loose, None).launch["program"], json!(loose));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_rust_test_is_built_without_running_it_and_the_result_found() {
        let lens = json!([{ "label": "test tests::adds", "args": { "cargoArgs": ["test", "--package", "demo", "--lib"], "executableArgs": ["tests::adds", "--exact"], "cwd": "/work/demo", "environment": { "A": "b" } } }]);
        let (build, args) = cargo_build(&lens, Path::new("/")).unwrap();
        assert_eq!(build.args, ["test", "--package", "demo", "--lib", "--no-run", "--message-format=json"]);
        assert_eq!((build.cwd.clone(), build.env.clone(), args), (PathBuf::from("/work/demo"), vec![("A".to_owned(), "b".to_owned())], vec!["tests::adds".to_owned(), "--exact".to_owned()]));
        let run = json!([{ "args": { "cargoArgs": ["run", "--package", "demo"], "executableArgs": [] } }]);
        assert_eq!(cargo_build(&run, Path::new("/work")).unwrap().0.args, ["build", "--package", "demo", "--message-format=json"]);
        assert!(cargo_build(&json!([{ "args": { "cargoArgs": ["check"] } }]), Path::new("/")).is_none());
        // What cargo prints: the last thing that can be run is the one.
        let output = "{\"reason\":\"compiler-artifact\",\"executable\":null}\nnot json\n{\"reason\":\"compiler-artifact\",\"executable\":\"/t/debug/deps/dep-1\"}\n{\"reason\":\"compiler-artifact\",\"executable\":\"/t/debug/deps/demo-9\"}\n{\"reason\":\"build-finished\",\"success\":true}\n";
        assert_eq!(built_program(output), Some(PathBuf::from("/t/debug/deps/demo-9")));
        assert_eq!(built_program("{\"reason\":\"build-finished\",\"success\":false}"), None);
        let failed = "{\"reason\":\"compiler-message\",\"message\":{\"level\":\"warning\",\"rendered\":\"warn\"}}\n{\"reason\":\"compiler-message\",\"message\":{\"level\":\"error\",\"rendered\":\"error: mismatched types\"}}\n";
        assert_eq!(build_errors(failed), "error: mismatched types");
        let spec = lldb_spec(Path::new("/usr/bin/lldb-dap"), "test adds", Path::new("/t/demo-9"), &["adds".into()], Path::new("/work/demo"));
        assert_eq!((spec.launch["program"].clone(), spec.launch["args"].clone(), spec.title.as_str()), (json!("/t/demo-9"), json!(["adds"]), "debug test adds"));
    }
}
