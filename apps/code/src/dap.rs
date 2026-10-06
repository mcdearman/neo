//! Talking to a debugger over the Debug Adapter Protocol.
//!
//! A debug adapter is a program that runs the thing being debugged and
//! answers questions about it: where it has stopped, what called what,
//! what the variables hold. Languages ship their own (`meadow dap`), and
//! `lldb-dap` does it for anything compiled. This is the other end of
//! that conversation; what to show for it is the app's business.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::lsp::client::{encode, read_message};

/// One call on the stack of a stopped program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub id: i64,
    pub name: String,
    /// The file it is in, if the adapter knows of one.
    pub path: Option<PathBuf>,
    /// From zero.
    pub line: usize,
}

/// A variable, or something else with a name and a value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Variable {
    pub name: String,
    pub value: String,
    pub kind: Option<String>,
    /// What to ask for to see what is inside it; zero if nothing is.
    pub reference: i64,
}

/// A group of variables: the locals, the globals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scope {
    pub name: String,
    pub reference: i64,
    /// Costly to fetch, so not fetched until asked for.
    pub expensive: bool,
}

/// Something the adapter said.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The program is running, or about to.
    Running,
    /// It has stopped, and why: a breakpoint, a step, an exception.
    Stopped { reason: String, thread: i64 },
    Stack(Vec<Frame>),
    Scopes { frame: i64, scopes: Vec<Scope> },
    Variables { reference: i64, variables: Vec<Variable> },
    /// The lines the adapter put a file's breakpoints on, which may be
    /// further down than where they were asked for. From zero.
    Breakpoints { path: PathBuf, lines: Vec<usize> },
    /// Something the program or the adapter printed.
    Output(String),
    /// The program is over.
    Ended(Option<i64>),
    /// The adapter refused something that cannot be carried on from.
    Failed(String),
    /// It refused something that can.
    Message(String),
}

/// What arrives from the thread that reads an adapter's output.
#[derive(Clone, Debug)]
pub enum Incoming {
    Message(Value),
    Closed,
}

enum Pending {
    Initialize,
    Launch,
    Configured,
    Stack,
    Scopes(i64),
    Variables(i64),
    Breakpoints(PathBuf),
    /// Continue and the steps: only a refusal is of interest.
    Move,
    Other,
}

/// A running debug adapter.
pub struct Dap {
    out: Box<dyn Write + Send>,
    child: Option<std::process::Child>,
    seq: u64,
    pending: HashMap<u64, Pending>,
    /// What to launch, sent once the adapter has said what it can do.
    launch: Value,
    /// Breakpoints to set before the program starts: by file, lines from zero.
    breakpoints: Vec<(PathBuf, Vec<usize>)>,
    /// The adapter is ready to be told about breakpoints.
    configurable: bool,
    /// The thread last heard to have stopped, which steps apply to.
    pub thread: i64,
}

impl Dap {
    /// A client writing to `out`, which starts the conversation at once.
    /// `launch` is the adapter's own description of what to debug.
    pub fn new(out: Box<dyn Write + Send>, launch: Value, breakpoints: Vec<(PathBuf, Vec<usize>)>) -> Self {
        let mut dap = Self { out, child: None, seq: 0, pending: HashMap::new(), launch, breakpoints, configurable: false, thread: 1 };
        dap.request("initialize", json!({ "clientID": "neocode", "clientName": "NeoCode", "adapterID": "neocode", "linesStartAt1": true, "columnsStartAt1": true, "pathFormat": "path", "supportsVariableType": true }), Pending::Initialize);
        dap
    }

    /// Starts the adapter `program` with `args`, in `cwd`, with `dirs` as
    /// its `PATH`. Its messages are passed to `on_message` from another thread.
    pub fn spawn(program: &Path, args: &[String], cwd: &Path, dirs: &[PathBuf], launch: Value, breakpoints: Vec<(PathBuf, Vec<usize>)>, on_message: impl Fn(Incoming) + Send + 'static) -> std::io::Result<Self> {
        use std::process::Stdio;
        let mut command = std::process::Command::new(program);
        if let Ok(path) = std::env::join_paths(dirs)
            && !path.is_empty()
        {
            command.env("PATH", path);
        }
        let mut child = command.args(args).current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
        let stdin = child.stdin.take().ok_or_else(|| std::io::Error::other("no pipe to the debugger"))?;
        let stdout = child.stdout.take().ok_or_else(|| std::io::Error::other("no pipe from the debugger"))?;
        std::thread::spawn(move || {
            let mut reader = std::io::BufReader::new(stdout);
            while let Ok(Some(message)) = read_message(&mut reader) {
                on_message(Incoming::Message(message));
            }
            on_message(Incoming::Closed);
        });
        let mut dap = Self::new(Box::new(stdin), launch, breakpoints);
        dap.child = Some(child);
        Ok(dap)
    }

    fn request(&mut self, command: &str, arguments: Value, what: Pending) {
        self.seq += 1;
        self.pending.insert(self.seq, what);
        let message = json!({ "seq": self.seq, "type": "request", "command": command, "arguments": arguments });
        let _ = self.out.write_all(&encode(&message)).and_then(|_| self.out.flush());
    }

    /// Tells the adapter where one file's breakpoints are, replacing any
    /// it had for that file. Lines are from zero.
    pub fn set_breakpoints(&mut self, path: &Path, lines: &[usize]) {
        if !self.configurable {
            // Kept until it can be told.
            self.breakpoints.retain(|(p, _)| p != path);
            self.breakpoints.push((path.to_path_buf(), lines.to_vec()));
            return;
        }
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let breakpoints: Vec<Value> = lines.iter().map(|l| json!({ "line": l + 1 })).collect();
        self.request("setBreakpoints", json!({ "source": { "name": name, "path": path }, "breakpoints": breakpoints, "lines": lines.iter().map(|l| l + 1).collect::<Vec<_>>() }), Pending::Breakpoints(path.to_path_buf()));
    }

    pub fn resume(&mut self) {
        self.request("continue", json!({ "threadId": self.thread }), Pending::Move);
    }

    /// Runs to the next line, without going into what it calls.
    pub fn step_over(&mut self) {
        self.request("next", json!({ "threadId": self.thread }), Pending::Move);
    }

    pub fn step_in(&mut self) {
        self.request("stepIn", json!({ "threadId": self.thread }), Pending::Move);
    }

    pub fn step_out(&mut self) {
        self.request("stepOut", json!({ "threadId": self.thread }), Pending::Move);
    }

    pub fn pause(&mut self) {
        self.request("pause", json!({ "threadId": self.thread }), Pending::Other);
    }

    /// Asks what the variables in a frame are.
    pub fn scopes(&mut self, frame: i64) {
        self.request("scopes", json!({ "frameId": frame }), Pending::Scopes(frame));
    }

    /// Asks what is inside a scope, or inside a variable that has parts.
    pub fn variables(&mut self, reference: i64) {
        self.request("variables", json!({ "variablesReference": reference }), Pending::Variables(reference));
    }

    /// Ends the session and the program with it.
    pub fn stop(&mut self) {
        self.request("disconnect", json!({ "terminateDebuggee": true }), Pending::Other);
    }

    /// Takes in a message from the adapter and says what the app should
    /// do about it.
    pub fn handle(&mut self, message: &Value) -> Vec<Event> {
        match message["type"].as_str() {
            Some("event") => self.event(message["event"].as_str().unwrap_or_default(), &message["body"]),
            Some("response") => {
                let Some(what) = message["request_seq"].as_u64().and_then(|seq| self.pending.remove(&seq)) else { return vec![] };
                let refused = (message["success"].as_bool() == Some(false)).then(|| {
                    let body = &message["body"]["error"]["format"];
                    body.as_str().or_else(|| message["message"].as_str()).unwrap_or("the debugger refused").to_owned()
                });
                self.answered(what, refused, &message["body"])
            }
            // The adapter asking the editor to do something, such as run
            // the program in a terminal. Nothing of the kind is offered.
            Some("request") => {
                self.seq += 1;
                let answer = json!({ "seq": self.seq, "type": "response", "request_seq": message["seq"], "command": message["command"], "success": false, "message": "not supported" });
                let _ = self.out.write_all(&encode(&answer)).and_then(|_| self.out.flush());
                vec![]
            }
            _ => vec![],
        }
    }

    fn event(&mut self, name: &str, body: &Value) -> Vec<Event> {
        match name {
            // Ready for breakpoints; when they are in, the program may go.
            "initialized" => {
                self.configurable = true;
                for (path, lines) in std::mem::take(&mut self.breakpoints) {
                    self.set_breakpoints(&path, &lines);
                }
                self.request("configurationDone", json!({}), Pending::Configured);
                vec![]
            }
            "stopped" => {
                self.thread = body["threadId"].as_i64().unwrap_or(self.thread);
                self.request("stackTrace", json!({ "threadId": self.thread, "startFrame": 0, "levels": 64 }), Pending::Stack);
                let said = body["description"].as_str().or_else(|| body["reason"].as_str()).unwrap_or("stopped");
                vec![Event::Stopped { reason: said.to_owned(), thread: self.thread }]
            }
            "continued" => vec![Event::Running],
            "output" => match (body["category"].as_str(), body["output"].as_str()) {
                // Not what the adapter says about itself for its makers.
                (Some("telemetry"), _) | (_, None) => vec![],
                (_, Some(text)) => vec![Event::Output(text.to_owned())],
            },
            "exited" => vec![Event::Ended(body["exitCode"].as_i64())],
            "terminated" => vec![Event::Ended(None)],
            _ => vec![],
        }
    }

    fn answered(&mut self, what: Pending, refused: Option<String>, body: &Value) -> Vec<Event> {
        match (what, refused) {
            (Pending::Initialize, Some(why)) | (Pending::Launch, Some(why)) | (Pending::Configured, Some(why)) => vec![Event::Failed(why)],
            (Pending::Initialize, None) => {
                let launch = self.launch.clone();
                self.request("launch", launch, Pending::Launch);
                vec![]
            }
            (Pending::Configured, None) => vec![Event::Running],
            (Pending::Stack, None) => {
                let frames = body["stackFrames"].as_array().map(|a| a.iter().filter_map(|f| Some(Frame { id: f["id"].as_i64()?, name: f["name"].as_str().unwrap_or("?").to_owned(), path: f["source"]["path"].as_str().map(PathBuf::from), line: (f["line"].as_u64().unwrap_or(1) as usize).saturating_sub(1) })).collect()).unwrap_or_default();
                vec![Event::Stack(frames)]
            }
            (Pending::Scopes(frame), None) => {
                let scopes = body["scopes"].as_array().map(|a| a.iter().filter_map(|s| Some(Scope { name: s["name"].as_str()?.to_owned(), reference: s["variablesReference"].as_i64()?, expensive: s["expensive"].as_bool() == Some(true) })).collect()).unwrap_or_default();
                vec![Event::Scopes { frame, scopes }]
            }
            (Pending::Variables(reference), None) => {
                let variables = body["variables"].as_array().map(|a| a.iter().filter_map(|v| Some(Variable { name: v["name"].as_str()?.to_owned(), value: v["value"].as_str().unwrap_or_default().to_owned(), kind: v["type"].as_str().filter(|t| !t.is_empty()).map(str::to_owned), reference: v["variablesReference"].as_i64().unwrap_or(0) })).collect()).unwrap_or_default();
                vec![Event::Variables { reference, variables }]
            }
            (Pending::Breakpoints(path), None) => {
                // Only those it could place, each on the line it chose.
                let lines = body["breakpoints"].as_array().map(|a| a.iter().filter(|b| b["verified"].as_bool() != Some(false)).filter_map(|b| Some((b["line"].as_u64()? as usize).saturating_sub(1))).collect()).unwrap_or_default();
                vec![Event::Breakpoints { path, lines }]
            }
            (Pending::Move, None) => vec![Event::Running],
            (Pending::Launch, None) | (Pending::Other, _) => vec![],
            (_, Some(why)) => vec![Event::Message(why)],
        }
    }
}

impl Drop for Dap {
    fn drop(&mut self) {
        // An adapter left running would keep the program going too.
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::testing::Sink;

    fn started(breakpoints: Vec<(PathBuf, Vec<usize>)>) -> (Dap, Sink) {
        let sink = Sink::default();
        let dap = Dap::new(Box::new(sink.clone()), json!({ "program": "/work/demo" }), breakpoints);
        (dap, sink)
    }

    fn answer(sent: &Value, body: Value) -> Value {
        json!({ "type": "response", "request_seq": sent["seq"], "command": sent["command"], "success": true, "body": body })
    }

    #[test]
    fn a_session_starts_in_the_order_the_protocol_wants() {
        let file = PathBuf::from("/work/demo/src/Main.mw");
        let (mut dap, sink) = started(vec![(file.clone(), vec![4, 9])]);
        let hello = sink.take();
        assert_eq!((hello.len(), hello[0]["command"].as_str()), (1, Some("initialize")));
        assert_eq!(hello[0]["arguments"]["linesStartAt1"], true);
        // Breakpoints asked for before it is ready are kept, not sent.
        dap.set_breakpoints(Path::new("/work/demo/src/Other.mw"), &[0]);
        assert!(sink.take().is_empty());
        // Its abilities known, the launch goes with what was asked for.
        assert!(dap.handle(&answer(&hello[0], json!({ "supportsConfigurationDoneRequest": true }))).is_empty());
        let launch = sink.take();
        assert_eq!((launch[0]["command"].as_str(), &launch[0]["arguments"]), (Some("launch"), &json!({ "program": "/work/demo" })));
        // Ready for breakpoints: each file's, then the word to go.
        assert!(dap.handle(&json!({ "type": "event", "event": "initialized" })).is_empty());
        let setup = sink.take();
        assert_eq!(setup.iter().map(|m| m["command"].as_str().unwrap()).collect::<Vec<_>>(), ["setBreakpoints", "setBreakpoints", "configurationDone"]);
        assert_eq!(setup[0]["arguments"]["source"]["path"], "/work/demo/src/Main.mw");
        assert_eq!(setup[0]["arguments"]["breakpoints"], json!([{ "line": 5 }, { "line": 10 }]), "lines are counted from one on the wire");
        // Where it put them, which may not be where they were asked for.
        let placed = dap.handle(&answer(&setup[0], json!({ "breakpoints": [{ "line": 7, "verified": true }, { "line": 10, "verified": false }] })));
        assert_eq!(placed, [Event::Breakpoints { path: file, lines: vec![6] }]);
        assert_eq!(dap.handle(&answer(&setup[2], json!({}))), [Event::Running]);
        assert!(dap.handle(&answer(&launch[0], json!({}))).is_empty());
    }

    #[test]
    fn stopping_asks_for_the_stack_and_then_what_is_in_it() {
        let (mut dap, sink) = started(vec![]);
        sink.take();
        let stopped = dap.handle(&json!({ "type": "event", "event": "stopped", "body": { "reason": "breakpoint", "threadId": 3, "allThreadsStopped": true } }));
        assert_eq!(stopped, [Event::Stopped { reason: "breakpoint".into(), thread: 3 }]);
        let ask = sink.take();
        assert_eq!((ask[0]["command"].as_str(), ask[0]["arguments"]["threadId"].as_i64()), (Some("stackTrace"), Some(3)));
        let frames = json!({ "stackFrames": [{ "id": 10, "name": "isPrime", "line": 32, "column": 6, "source": { "name": "Main.mw", "path": "/work/demo/src/Main.mw" } }, { "id": 11, "name": "<native>", "line": 0 }] });
        let stack = dap.handle(&answer(&ask[0], frames));
        assert_eq!(stack, [Event::Stack(vec![Frame { id: 10, name: "isPrime".into(), path: Some("/work/demo/src/Main.mw".into()), line: 31 }, Frame { id: 11, name: "<native>".into(), path: None, line: 0 }])]);
        dap.scopes(10);
        let ask = sink.take();
        let scopes = dap.handle(&answer(&ask[0], json!({ "scopes": [{ "name": "Locals", "variablesReference": 1, "expensive": false }, { "name": "Heap", "variablesReference": 4, "expensive": true }] })));
        assert_eq!(scopes, [Event::Scopes { frame: 10, scopes: vec![Scope { name: "Locals".into(), reference: 1, expensive: false }, Scope { name: "Heap".into(), reference: 4, expensive: true }] }]);
        dap.variables(1);
        let ask = sink.take();
        let vars = dap.handle(&answer(&ask[0], json!({ "variables": [{ "name": "n", "value": "7", "type": "Int", "variablesReference": 0 }, { "name": "xs", "value": "[1; 2]", "variablesReference": 9 }] })));
        assert_eq!(vars, [Event::Variables { reference: 1, variables: vec![Variable { name: "n".into(), value: "7".into(), kind: Some("Int".into()), reference: 0 }, Variable { name: "xs".into(), value: "[1; 2]".into(), kind: None, reference: 9 }] }]);
        // Steps go to the thread that stopped.
        dap.step_over();
        dap.step_in();
        dap.step_out();
        dap.resume();
        let moves = sink.take();
        assert_eq!(moves.iter().map(|m| m["command"].as_str().unwrap()).collect::<Vec<_>>(), ["next", "stepIn", "stepOut", "continue"]);
        assert!(moves.iter().all(|m| m["arguments"]["threadId"] == 3));
        assert_eq!(dap.handle(&answer(&moves[0], json!({}))), [Event::Running]);
    }

    #[test]
    fn output_endings_and_refusals_are_passed_on() {
        let (mut dap, sink) = started(vec![]);
        let hello = sink.take();
        let event = |name: &str, body: Value| json!({ "type": "event", "event": name, "body": body });
        assert_eq!(dap.handle(&event("output", json!({ "category": "stdout", "output": "hello\n" }))), [Event::Output("hello\n".into())]);
        assert!(dap.handle(&event("output", json!({ "category": "telemetry", "output": "x" }))).is_empty());
        assert_eq!(dap.handle(&event("exited", json!({ "exitCode": 2 }))), [Event::Ended(Some(2))]);
        assert_eq!(dap.handle(&event("terminated", json!({}))), [Event::Ended(None)]);
        assert_eq!(dap.handle(&event("continued", json!({ "threadId": 1 }))), [Event::Running]);
        // A launch that is refused ends things; a step that is, does not.
        let refused = json!({ "type": "response", "request_seq": hello[0]["seq"], "command": "initialize", "success": false, "message": "no such package", "body": { "error": { "format": "no meadow.toml here" } } });
        assert_eq!(dap.handle(&refused), [Event::Failed("no meadow.toml here".into())]);
        dap.step_over();
        let step = sink.take();
        let refused = json!({ "type": "response", "request_seq": step[0]["seq"], "command": "next", "success": false, "message": "not stopped" });
        assert_eq!(dap.handle(&refused), [Event::Message("not stopped".into())]);
        // Asked to do something itself, it says it cannot, so the adapter
        // is not left waiting.
        dap.handle(&json!({ "type": "request", "seq": 40, "command": "runInTerminal", "arguments": {} }));
        let said = sink.take();
        assert_eq!((said[0]["type"].as_str(), said[0]["request_seq"].as_i64(), said[0]["success"].as_bool()), (Some("response"), Some(40), Some(false)));
        dap.stop();
        assert_eq!(sink.take()[0]["command"], "disconnect");
        // An answer to nothing that was asked is nothing.
        assert!(dap.handle(&json!({ "type": "response", "request_seq": 999, "success": true })).is_empty());
    }
}
