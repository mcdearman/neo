//! Doing what a lens offers: running a test or a program and showing
//! what it prints, in a panel under the editor.
//!
//! A language server offers things to do above lines of a file, "Run
//! Test" and the like. Some it carries out itself when asked. Others it
//! only names, leaving the editor to know what they mean; the ones
//! NeoCode knows are turned into a command to run here.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use neo::prelude::{Action, Document, Motion};
use serde_json::Value;

use crate::lsp::Lens;

/// A command to run and what to call it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunSpec {
    pub title: String,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
}

impl RunSpec {
    /// The command as it would be typed.
    pub fn line(&self) -> String {
        let name = self.program.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        std::iter::once(name).chain(self.args.iter().map(|a| if a.contains(' ') { format!("\"{a}\"") } else { a.clone() })).collect::<Vec<_>>().join(" ")
    }
}

/// What choosing a lens comes to.
#[derive(Clone, Debug, PartialEq)]
pub enum LensAction {
    /// Run this and show what it prints.
    Run(RunSpec),
    /// The server carries it out when asked.
    AskServer,
    /// It starts a debugger, which NeoCode does not have.
    Debug,
    /// The server names a command NeoCode has not been taught.
    Unknown,
}

/// A lens's label as shown: servers put a symbol in front, often one the
/// interface font does not have.
pub fn label(title: &str) -> String {
    title.trim_start_matches(|c: char| !c.is_alphanumeric()).trim().to_owned()
}

/// The package a Meadow file belongs to: the nearest folder above it
/// with a manifest. A test is built as part of its whole package.
fn meadow_package(file: &Path) -> Option<PathBuf> {
    file.ancestors().skip(1).find(|d| d.join("Meadow.toml").exists() || d.join("meadow.toml").exists()).map(Path::to_path_buf)
}

/// Works out what a lens does. `commands` are the ones the server said it
/// carries out itself, `server` is the server's own program, and `root`
/// is the folder that is open.
pub fn lens_action(lens: &Lens, commands: &[String], server: Option<&Path>, root: &Path) -> LensAction {
    let first = &lens.arguments[0];
    let strings = |v: &Value| -> Vec<String> { v.as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect()).unwrap_or_default() };
    match lens.command.as_str() {
        // `meadow test <package> --exact <name>`, with the same program
        // that is serving the file, so the two agree about the language.
        "meadow.testFunction" => {
            let (Some(test), Some(file)) = (first["test"].as_str(), first["uri"].as_str().and_then(crate::lsp::client::path_of)) else { return LensAction::Unknown };
            let package = meadow_package(&file);
            let target = package.clone().unwrap_or_else(|| file.clone());
            let cwd = package.or_else(|| file.parent().map(Path::to_path_buf)).unwrap_or_else(|| root.to_path_buf());
            LensAction::Run(RunSpec { title: format!("test {test}"), program: server.map_or_else(|| PathBuf::from("meadow"), Path::to_path_buf), args: vec!["test".into(), target.to_string_lossy().into_owned(), "--exact".into(), test.to_owned()], cwd, env: vec![] })
        }
        // rust-analyzer spells the command out: cargo's arguments, then the
        // test binary's own after a `--`.
        "rust-analyzer.runSingle" => {
            let args = &first["args"];
            let mut argv = strings(&args["cargoArgs"]);
            if argv.is_empty() {
                return LensAction::Unknown;
            }
            let rest = strings(&args["executableArgs"]);
            if !rest.is_empty() {
                argv.push("--".into());
                argv.extend(rest);
            }
            let cwd = args["cwd"].as_str().or_else(|| args["workspaceRoot"].as_str()).map_or_else(|| root.to_path_buf(), PathBuf::from);
            let env = args["environment"].as_object().map(|o| o.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned()))).collect()).unwrap_or_default();
            let title = first["label"].as_str().map_or_else(|| label(&lens.title), str::to_owned);
            LensAction::Run(RunSpec { title, program: args["overrideCargo"].as_str().map_or_else(|| PathBuf::from("cargo"), PathBuf::from), args: argv, cwd, env })
        }
        "meadow.debugFunction" | "rust-analyzer.debugSingle" => LensAction::Debug,
        command if commands.iter().any(|c| c == command) => LensAction::AskServer,
        command if command.to_ascii_lowercase().contains("debug") => LensAction::Debug,
        _ => LensAction::Unknown,
    }
}

/// How a run stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Running,
    /// It ended, with this exit code if it had one.
    Ended(Option<i32>),
    /// It could not be started.
    Failed(String),
    Stopped,
}

/// Something run from a lens, and what it has printed so far.
pub struct Run {
    pub id: u64,
    pub spec: RunSpec,
    /// What it printed, kept as a document so it can be scrolled and read.
    pub output: Document,
    pub status: Status,
    lines: usize,
    child: Arc<Mutex<Option<Child>>>,
}

/// More than this is not kept: nobody reads it, and it all has to be laid out.
const MOST_LINES: usize = 5000;

/// Text as printed, without the codes that colour it or redraw a line.
pub fn plain(line: &str) -> String {
    // What follows the last carriage return is what would be left showing.
    let line = line.trim_end_matches(['\r', '\n']);
    let line = line.rsplit('\r').next().unwrap_or(line);
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            if c == '\t' || !c.is_control() {
                out.push(c);
            }
            continue;
        }
        // An escape: `[`, then parameters, then a letter that ends it.
        if chars.next_if_eq(&'[').is_some() {
            while chars.next_if(|c| !c.is_ascii_alphabetic()).is_some() {}
            chars.next();
        }
    }
    out
}

impl Run {
    pub fn new(id: u64, spec: RunSpec) -> Self {
        let mut run = Self { id, spec, output: Document::new(""), status: Status::Running, lines: 0, child: Arc::default() };
        let line = format!("$ {}", run.spec.line());
        run.push(&line);
        run
    }

    /// Adds a line to what was printed, leaving the view at the end.
    pub fn push(&mut self, line: &str) {
        self.lines += 1;
        if self.lines > MOST_LINES {
            return;
        }
        let text = if self.lines == MOST_LINES { "… more was printed than is kept here.".to_owned() } else { plain(line) };
        self.output.apply(Action::Move { motion: Motion::DocEnd, select: false });
        self.output.apply(Action::Insert(if self.lines == 1 { text } else { format!("\n{text}") }));
    }

    /// One line saying how it went.
    pub fn summary(&self) -> String {
        match &self.status {
            Status::Running => "running…".into(),
            Status::Ended(Some(0)) => "finished".into(),
            Status::Ended(Some(code)) => format!("failed, with exit code {code}"),
            Status::Ended(None) => "ended".into(),
            Status::Failed(why) => format!("could not start: {why}"),
            Status::Stopped => "stopped".into(),
        }
    }

    pub fn ended_well(&self) -> bool {
        self.status == Status::Ended(Some(0))
    }

    /// Stops it if it is still going.
    pub fn stop(&mut self) {
        if self.status == Status::Running {
            if let Some(child) = self.child.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                let _ = child.kill();
            }
            self.status = Status::Stopped;
        }
    }

    /// Starts the command. `dirs` become its `PATH`, since an app started
    /// from the Dock is given a bare one. Each line it prints is passed to
    /// `said`, and how it ended to `ended`, both from other threads.
    pub fn start(&mut self, dirs: &[PathBuf], said: impl Fn(String) + Send + Clone + 'static, ended: impl FnOnce(Option<i32>) + Send + 'static) {
        let mut command = Command::new(&self.spec.program);
        if let Ok(path) = std::env::join_paths(dirs).map(|p| p.to_os_string()).map(|p| if p.is_empty() { std::env::var_os("PATH").unwrap_or_default() } else { p }) {
            command.env("PATH", path);
        }
        // Colour codes would only be stripped again.
        command.env("NO_COLOR", "1").env("CLICOLOR", "0").env("TERM", "dumb").envs(self.spec.env.iter().cloned());
        let mut child = match command.args(&self.spec.args).current_dir(&self.spec.cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
            Ok(child) => child,
            Err(e) => {
                self.status = Status::Failed(e.to_string());
                return;
            }
        };
        let read = |from: Box<dyn Read + Send>, said: Box<dyn Fn(String) + Send>| {
            std::thread::spawn(move || {
                let mut reader = BufReader::new(from);
                let mut bytes = vec![];
                // By bytes, not as text: a program may print anything.
                while reader.read_until(b'\n', &mut bytes).is_ok_and(|n| n > 0) {
                    said(String::from_utf8_lossy(&bytes).into_owned());
                    bytes.clear();
                }
            })
        };
        let readers = [child.stdout.take().map(|o| read(Box::new(o), Box::new(said.clone()))), child.stderr.take().map(|e| read(Box::new(e), Box::new(said)))];
        *self.child.lock().unwrap_or_else(|e| e.into_inner()) = Some(child);
        let child = self.child.clone();
        std::thread::spawn(move || {
            // Everything it printed first, then how it ended. Polled, so
            // the lock is free for a stop.
            for reader in readers.into_iter().flatten() {
                let _ = reader.join();
            }
            let code = loop {
                match child.lock().unwrap_or_else(|e| e.into_inner()).as_mut().map(Child::try_wait) {
                    Some(Ok(None)) => {}
                    Some(Ok(Some(status))) => break status.code(),
                    _ => break None,
                }
                std::thread::sleep(Duration::from_millis(30));
            };
            ended(code);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn lens(command: &str, arguments: Value) -> Lens {
        Lens { line: 3, title: "▶\u{fe0e} Run Test".into(), command: command.into(), arguments }
    }

    #[test]
    fn labels_lose_the_symbols_servers_put_in_front() {
        assert_eq!(label("▶\u{fe0e} Run Test"), "Run Test");
        assert_eq!(label("⚙\u{fe0e} Debug"), "Debug");
        assert_eq!(label("▶ Debug"), "Debug");
        assert_eq!(label("3 references"), "3 references");
    }

    #[test]
    fn a_rust_test_is_run_with_cargo_as_the_server_spells_it_out() {
        let args = json!([{ "label": "test tests::adds", "kind": "cargo", "args": { "cargoArgs": ["test", "--package", "demo", "--lib"], "executableArgs": ["tests::adds", "--exact", "--nocapture"], "cwd": "/work/demo", "workspaceRoot": "/work", "environment": { "RUSTC_TOOLCHAIN": "/toolchains/stable" }, "overrideCargo": null } }]);
        let LensAction::Run(spec) = lens_action(&lens("rust-analyzer.runSingle", args.clone()), &[], None, Path::new("/elsewhere")) else { panic!("something to run") };
        assert_eq!(spec.program, PathBuf::from("cargo"));
        assert_eq!(spec.args, ["test", "--package", "demo", "--lib", "--", "tests::adds", "--exact", "--nocapture"]);
        assert_eq!((spec.cwd.clone(), spec.title.as_str()), (PathBuf::from("/work/demo"), "test tests::adds"));
        assert_eq!(spec.env, [("RUSTC_TOOLCHAIN".to_owned(), "/toolchains/stable".to_owned())]);
        assert_eq!(spec.line(), "cargo test --package demo --lib -- tests::adds --exact --nocapture");
        // The same with Debug in front needs a debugger.
        assert_eq!(lens_action(&lens("rust-analyzer.debugSingle", args), &[], None, Path::new("/")), LensAction::Debug);
        // Nothing to run is not run.
        assert_eq!(lens_action(&lens("rust-analyzer.runSingle", json!([{ "args": {} }])), &[], None, Path::new("/")), LensAction::Unknown);
    }

    #[test]
    fn a_meadow_test_is_run_by_the_program_serving_the_file() {
        let dir = std::env::temp_dir().join(format!("neo-code-meadow-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("pkg/src")).unwrap();
        std::fs::write(dir.join("pkg/Meadow.toml"), "").unwrap();
        let file = dir.join("pkg/src/Main.mw");
        let args = json!([{ "uri": crate::lsp::client::uri(&file), "test": "Main.adds" }]);
        let server = Path::new("/home/sam/.meadow/bin/meadow");
        let LensAction::Run(spec) = lens_action(&lens("meadow.testFunction", args.clone()), &[], Some(server), &dir) else { panic!("something to run") };
        assert_eq!(spec.program, server);
        assert_eq!(spec.args, ["test".to_owned(), dir.join("pkg").to_string_lossy().into_owned(), "--exact".into(), "Main.adds".into()], "the whole package, and only the test named");
        assert_eq!((spec.cwd, spec.title.as_str()), (dir.join("pkg"), "test Main.adds"));
        // With no server program known, the one on the PATH.
        let LensAction::Run(spec) = lens_action(&lens("meadow.testFunction", args), &[], None, &dir) else { panic!() };
        assert_eq!(spec.program, PathBuf::from("meadow"));
        assert_eq!(lens_action(&lens("meadow.debugFunction", json!([{ "name": "main" }])), &[], None, &dir), LensAction::Debug);
        assert_eq!(lens_action(&lens("meadow.testFunction", json!([{}])), &[], None, &dir), LensAction::Unknown);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn other_commands_go_to_the_server_if_it_said_it_runs_them() {
        let known = ["tool.organize".to_owned()];
        assert_eq!(lens_action(&lens("tool.organize", json!([])), &known, None, Path::new("/")), LensAction::AskServer);
        assert_eq!(lens_action(&lens("tool.showReferences", json!([])), &known, None, Path::new("/")), LensAction::Unknown);
        assert_eq!(lens_action(&lens("tool.startDebugging", json!([])), &known, None, Path::new("/")), LensAction::Debug);
    }

    #[test]
    fn output_is_shown_without_colour_codes_or_redrawn_lines() {
        assert_eq!(plain("\u{1b}[1m\u{1b}[32m   Compiling\u{1b}[0m demo v0.1.0\n"), "   Compiling demo v0.1.0");
        assert_eq!(plain("Building 1/9\rBuilding 9/9\r\n"), "Building 9/9");
        assert_eq!(plain("a\tb\u{7}"), "a\tb");
        let mut run = Run::new(1, RunSpec { title: "t".into(), program: "/bin/echo".into(), args: vec!["two words".into()], cwd: "/".into(), env: vec![] });
        run.push("first\n");
        run.push("second");
        assert_eq!(run.output.text(), "$ echo \"two words\"\nfirst\nsecond");
        assert_eq!(run.output.cursor().line, 2, "the view is left at the end");
        for i in 0..MOST_LINES + 50 {
            run.push(&format!("line {i}"));
        }
        assert_eq!(run.output.line_count(), MOST_LINES);
        assert!(run.output.lines().last().unwrap().contains("more was printed"));
    }

    #[cfg(unix)]
    #[test]
    fn a_command_runs_and_reports_what_it_printed_and_how_it_ended() {
        use std::sync::mpsc::channel;
        let go = |script: &str| {
            let mut run = Run::new(1, RunSpec { title: "t".into(), program: "/bin/sh".into(), args: vec!["-c".into(), script.into()], cwd: std::env::temp_dir(), env: vec![("NEO_RUN_TEST".into(), "set".into())] });
            let (lines_to, lines) = channel();
            let (ended_to, ended) = channel();
            run.start(
                &[PathBuf::from("/usr/bin"), PathBuf::from("/bin")],
                move |l| {
                    let _ = lines_to.send(l);
                },
                move |code| {
                    let _ = ended_to.send(code);
                },
            );
            (run, lines, ended)
        };
        let (run, lines, ended) = go("echo out; echo err >&2; echo $NEO_RUN_TEST; exit 3");
        assert_eq!(run.status, Status::Running);
        assert_eq!(ended.recv_timeout(Duration::from_secs(20)), Ok(Some(3)));
        let mut said: Vec<String> = lines.try_iter().map(|l| plain(&l)).collect();
        said.sort();
        assert_eq!(said, ["err", "out", "set"], "both what it printed and what it complained of, before it is said to have ended");
        // Stopping one that would go on.
        let (mut run, _lines, ended) = go("sleep 30");
        run.stop();
        assert_eq!(run.status, Status::Stopped);
        assert!(ended.recv_timeout(Duration::from_secs(20)).is_ok(), "it ended when stopped, not thirty seconds later");
        // One that cannot be started says why.
        let mut run = Run::new(1, RunSpec { title: "t".into(), program: "/no/such/program".into(), args: vec![], cwd: "/".into(), env: vec![] });
        run.start(&[], |_| {}, |_| {});
        assert!(matches!(run.status, Status::Failed(_)));
        assert!(run.summary().starts_with("could not start"));
    }
}
