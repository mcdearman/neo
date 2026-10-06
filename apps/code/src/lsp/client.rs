//! Talking to a language server: the wire format, and a client that keeps
//! track of what it asked.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use neo::widgets::Pos;
use serde_json::{json, Value};

use super::servers::Found;
#[cfg(test)]
use std::time::Duration;

/// A message as it goes down the pipe: a length header, then the JSON.
pub fn encode(message: &Value) -> Vec<u8> {
    let body = message.to_string();
    format!("Content-Length: {}\r\n\r\n{body}", body.len()).into_bytes()
}

/// Reads one message. `None` when the server has closed its end.
pub fn read_message(r: &mut impl BufRead) -> std::io::Result<Option<Value>> {
    let mut length = None;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let length = length.ok_or_else(|| std::io::Error::other("a message without a length"))?;
    let mut body = vec![0; length];
    r.read_exact(&mut body)?;
    serde_json::from_slice(&body).map(Some).map_err(std::io::Error::other)
}

/// A file's address in the form the protocol uses.
pub fn uri(path: &Path) -> String {
    let mut out = String::from("file://");
    let text = path.to_string_lossy().replace('\\', "/");
    if !text.starts_with('/') {
        out.push('/');
    }
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/:".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The file a `file://` address names.
pub fn path_of(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let mut bytes = Vec::with_capacity(rest.len());
    let mut it = rest.bytes();
    while let Some(b) = it.next() {
        if b == b'%' {
            let hex = [it.next()?, it.next()?];
            bytes.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
        } else {
            bytes.push(b);
        }
    }
    let path = String::from_utf8(bytes).ok()?;
    // Windows addresses look like `/C:/folder`.
    let path = if cfg!(windows) { path.trim_start_matches('/').to_owned() } else { path };
    Some(PathBuf::from(path))
}

/// A position as the protocol counts it: a line, and a distance along it
/// in UTF-16 units or, if the server agreed to it, bytes.
pub type Place = (u32, u32);

/// Where a protocol position is in the document.
pub fn to_pos(lines: &[String], place: Place, utf8: bool) -> Pos {
    let line = (place.0 as usize).min(lines.len().saturating_sub(1));
    let text = lines.get(line).map_or("", String::as_str);
    let want = place.1 as usize;
    let col = if utf8 {
        let mut col = want.min(text.len());
        while !text.is_char_boundary(col) {
            col -= 1;
        }
        col
    } else {
        let mut units = 0;
        text.char_indices().find(|(_, c)| {
            let before = units;
            units += c.len_utf16();
            before >= want
        }).map_or(text.len(), |(i, _)| i)
    };
    Pos::new(line, col)
}

/// A document position as the protocol counts it.
pub fn to_place(lines: &[String], pos: Pos, utf8: bool) -> Place {
    let text = lines.get(pos.line).map_or("", String::as_str);
    let col = pos.col.min(text.len());
    let along = if utf8 { col } else { text[..col].encode_utf16().count() };
    (pos.line as u32, along as u32)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

/// A problem the server found, where the protocol says it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Diagnostic {
    pub from: Place,
    pub to: Place,
    pub severity: Severity,
    pub message: String,
    pub source: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Completion {
    pub label: String,
    /// A type or signature to show beside the label.
    pub detail: Option<String>,
    /// What to put in the text.
    pub insert: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextEdit {
    pub from: Place,
    pub to: Place,
    pub text: String,
}

/// A stretch of a line and what the server says it is, for colouring.
#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub line: u32,
    /// Where it starts along the line, and how long it is, counted the way
    /// the server counts positions.
    pub start: u32,
    pub length: u32,
    /// The server's name for the kind of thing, such as `function`.
    pub kind: String,
}

/// The kinds of token NeoCode can colour, which it tells servers about.
const TOKEN_TYPES: &[&str] = &[
    "namespace", "type", "class", "enum", "interface", "struct", "typeParameter", "parameter", "variable", "property", "enumMember", "event", "function", "method", "macro", "keyword", "modifier", "comment", "string",
    "number", "regexp", "operator", "decorator",
];

/// Something a server told the client.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The handshake is done and the server is taking requests.
    Ready,
    Diagnostics { path: PathBuf, items: Vec<Diagnostic> },
    Hover(Option<String>),
    Definition(Option<(PathBuf, Place)>),
    Completions(Vec<Completion>),
    /// Changes to make to a file, from a request to format it.
    Edits { path: PathBuf, edits: Vec<TextEdit> },
    /// What each stretch of a file is, for colouring it.
    Tokens { path: PathBuf, tokens: Vec<Token> },
    /// The server's view of the code changed: ask for tokens again.
    RefreshTokens,
    /// Something for the user to read.
    Message(String),
    Failed(String),
}

/// What arrives from the thread that reads a server's output.
#[derive(Clone, Debug)]
pub enum Incoming {
    Message(Value),
    /// The server's output ended: it exited or crashed.
    Closed,
}

enum Pending {
    Initialize,
    Hover,
    Definition,
    Completion,
    Formatting(PathBuf),
    Tokens(PathBuf),
}

/// One running language server.
pub struct Client {
    out: Box<dyn Write + Send>,
    child: Option<std::process::Child>,
    next_id: u64,
    pending: HashMap<u64, Pending>,
    ready: bool,
    /// Messages waiting for the handshake to finish.
    queued: Vec<Value>,
    /// Positions are counted in bytes rather than UTF-16 units.
    pub utf8: bool,
    /// Characters after which the server wants to be asked for completions.
    pub triggers: Vec<char>,
    pub can_format: bool,
    /// The server can say what each stretch of a file is.
    pub can_colour: bool,
    /// The server's names for token kinds, by the numbers it sends.
    legend: Vec<String>,
}

fn place(v: &Value) -> Place {
    (v["line"].as_u64().unwrap_or(0) as u32, v["character"].as_u64().unwrap_or(0) as u32)
}

/// Snippet text with its placeholders reduced to plain text: `${1:name}`
/// becomes `name`, and bare tab stops such as `$0` vanish.
pub fn plain_snippet(s: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '\\' if i + 1 < chars.len() => {
                out.push(chars[i + 1]);
                i += 2;
            }
            '$' if chars.get(i + 1).is_some_and(char::is_ascii_digit) => {
                i += 1;
                while chars.get(i).is_some_and(char::is_ascii_digit) {
                    i += 1;
                }
            }
            '$' if chars.get(i + 1) == Some(&'{') => {
                let Some(close) = chars[i..].iter().position(|c| *c == '}') else {
                    out.push('$');
                    i += 1;
                    continue;
                };
                let inner: String = chars[i + 2..i + close].iter().collect();
                if let Some((_, default)) = inner.split_once(':') {
                    out.push_str(default);
                }
                i += close + 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// Hover contents in any of the shapes servers send, as text.
fn hover_text(contents: &Value) -> Option<String> {
    let one = |v: &Value| v.as_str().map(str::to_owned).or_else(|| v["value"].as_str().map(str::to_owned));
    let text = match contents {
        Value::Array(parts) => parts.iter().filter_map(one).collect::<Vec<_>>().join("\n\n"),
        other => one(other)?,
    };
    let text = plain_markdown(&text);
    (!text.is_empty()).then_some(text)
}

/// Markdown as plain text, for a popup that shows no formatting: code
/// fences and rules go, emphasis marks and backticks are removed, and runs
/// of blank lines become one.
pub fn plain_markdown(text: &str) -> String {
    let mut out: Vec<String> = vec![];
    let mut in_code = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if !in_code && (trimmed == "---" || trimmed == "***") {
            continue;
        }
        // Inside a fence the text is code, and is kept as written.
        let line = if in_code { line.to_owned() } else { line.replace("**", "").replace("__", "").replace('`', "").trim_start_matches('#').trim_start().to_owned() };
        if line.trim().is_empty() && out.last().is_none_or(|l| l.trim().is_empty()) {
            continue;
        }
        out.push(line);
    }
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    out.join("\n")
}

impl Client {
    /// A client writing to `out`. It starts the handshake at once; other
    /// messages wait until the server answers it.
    pub fn new(out: Box<dyn Write + Send>, root: &Path) -> Self {
        let mut client = Self { out, child: None, next_id: 1, pending: HashMap::new(), ready: false, queued: vec![], utf8: false, triggers: vec![], can_format: false, can_colour: false, legend: vec![] };
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let params = json!({
            "processId": std::process::id(),
            "clientInfo": { "name": "NeoCode" },
            "rootUri": uri(root),
            "workspaceFolders": [{ "uri": uri(root), "name": name }],
            "capabilities": {
                "general": { "positionEncodings": ["utf-8", "utf-16"] },
                "textDocument": {
                    "synchronization": { "didSave": true },
                    "publishDiagnostics": {},
                    "hover": { "contentFormat": ["plaintext", "markdown"] },
                    "definition": {},
                    "completion": { "completionItem": { "snippetSupport": false } },
                    "formatting": {},
                    "semanticTokens": { "requests": { "full": true }, "tokenTypes": TOKEN_TYPES, "tokenModifiers": [], "formats": ["relative"] },
                },
                "workspace": { "semanticTokens": { "refreshSupport": true } },
            },
        });
        let id = client.take_id(Pending::Initialize);
        client.send(&json!({ "jsonrpc": "2.0", "id": id, "method": "initialize", "params": params }));
        client
    }

    /// Starts a server program and reads what it says on another thread,
    /// which hands each message to `on_message`.
    ///
    /// `dirs` becomes the server's `PATH`. Servers lean on other tools,
    /// such as Node, Cargo or Go, and an app started from a dock has a
    /// bare `PATH` that would hide them.
    pub fn spawn(found: &Found, root: &Path, dirs: &[PathBuf], on_message: impl Fn(Incoming) + Send + 'static) -> std::io::Result<Self> {
        use std::process::Stdio;
        let mut command = std::process::Command::new(&found.program);
        if let Ok(path) = std::env::join_paths(dirs) {
            command.env("PATH", path);
        }
        let mut child = command.args(&found.args).current_dir(root).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
        let stdin = child.stdin.take().ok_or_else(|| std::io::Error::other("no pipe to the server"))?;
        let stdout = child.stdout.take().ok_or_else(|| std::io::Error::other("no pipe from the server"))?;
        std::thread::spawn(move || {
            let mut reader = std::io::BufReader::new(stdout);
            while let Ok(Some(message)) = read_message(&mut reader) {
                on_message(Incoming::Message(message));
            }
            on_message(Incoming::Closed);
        });
        let mut client = Self::new(Box::new(stdin), root);
        client.child = Some(child);
        Ok(client)
    }

    pub fn ready(&self) -> bool {
        self.ready
    }

    fn take_id(&mut self, what: Pending) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.pending.insert(id, what);
        id
    }

    fn send(&mut self, message: &Value) {
        // A server that has gone away shows up as a closed output instead.
        let _ = self.out.write_all(&encode(message)).and_then(|_| self.out.flush());
    }

    /// Sends now if the handshake is done, otherwise once it is.
    fn post(&mut self, message: Value) {
        if self.ready {
            self.send(&message);
        } else {
            self.queued.push(message);
        }
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.post(json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    fn request(&mut self, method: &str, params: Value, what: Pending) {
        let id = self.take_id(what);
        self.post(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
    }

    pub fn did_open(&mut self, path: &Path, language: &str, version: i64, text: &str) {
        self.notify("textDocument/didOpen", json!({ "textDocument": { "uri": uri(path), "languageId": language, "version": version, "text": text } }));
    }

    /// Tells the server the file's new text, whole.
    pub fn did_change(&mut self, path: &Path, version: i64, text: &str) {
        self.notify("textDocument/didChange", json!({ "textDocument": { "uri": uri(path), "version": version }, "contentChanges": [{ "text": text }] }));
    }

    pub fn did_save(&mut self, path: &Path) {
        self.notify("textDocument/didSave", json!({ "textDocument": { "uri": uri(path) } }));
    }

    pub fn did_close(&mut self, path: &Path) {
        self.notify("textDocument/didClose", json!({ "textDocument": { "uri": uri(path) } }));
    }

    fn at(path: &Path, place: Place) -> Value {
        json!({ "textDocument": { "uri": uri(path) }, "position": { "line": place.0, "character": place.1 } })
    }

    pub fn hover(&mut self, path: &Path, place: Place) {
        self.request("textDocument/hover", Self::at(path, place), Pending::Hover);
    }

    pub fn definition(&mut self, path: &Path, place: Place) {
        self.request("textDocument/definition", Self::at(path, place), Pending::Definition);
    }

    pub fn completion(&mut self, path: &Path, place: Place) {
        // Only the latest answer is of any use.
        self.pending.retain(|_, p| !matches!(p, Pending::Completion));
        self.request("textDocument/completion", Self::at(path, place), Pending::Completion);
    }

    pub fn format(&mut self, path: &Path, indent: usize) {
        let params = json!({ "textDocument": { "uri": uri(path) }, "options": { "tabSize": indent, "insertSpaces": true } });
        self.request("textDocument/formatting", params, Pending::Formatting(path.to_path_buf()));
    }

    /// Asks what each stretch of the file is. Only the answer about the
    /// file's latest text is wanted, so earlier requests are dropped.
    pub fn tokens(&mut self, path: &Path) {
        if self.ready && !self.can_colour {
            return;
        }
        self.pending.retain(|_, p| !matches!(p, Pending::Tokens(other) if other == path));
        self.request("textDocument/semanticTokens/full", json!({ "textDocument": { "uri": uri(path) } }), Pending::Tokens(path.to_path_buf()));
    }

    /// Takes in a message from the server and says what, if anything, the
    /// app should do about it.
    pub fn handle(&mut self, message: &Value) -> Vec<Event> {
        let id = message.get("id");
        let method = message["method"].as_str();
        match (id, method) {
            // The server asking the client something. Answer, or it waits.
            (Some(id), Some(method)) => {
                let result = match method {
                    "workspace/configuration" => Value::Array(vec![Value::Null; message["params"]["items"].as_array().map_or(0, Vec::len)]),
                    _ => Value::Null,
                };
                self.send(&json!({ "jsonrpc": "2.0", "id": id, "result": result }));
                if method == "workspace/semanticTokens/refresh" { vec![Event::RefreshTokens] } else { vec![] }
            }
            (None, Some("textDocument/publishDiagnostics")) => {
                let params = &message["params"];
                let Some(path) = params["uri"].as_str().and_then(path_of) else { return vec![] };
                let items = params["diagnostics"]
                    .as_array()
                    .map(|list| {
                        list.iter()
                            .map(|d| Diagnostic {
                                from: place(&d["range"]["start"]),
                                to: place(&d["range"]["end"]),
                                severity: match d["severity"].as_u64() {
                                    Some(2) => Severity::Warning,
                                    Some(3) => Severity::Info,
                                    Some(4) => Severity::Hint,
                                    _ => Severity::Error,
                                },
                                message: d["message"].as_str().unwrap_or_default().to_owned(),
                                source: d["source"].as_str().map(str::to_owned),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                vec![Event::Diagnostics { path, items }]
            }
            // Errors and warnings are worth showing; chatter is not.
            (None, Some("window/showMessage")) if message["params"]["type"].as_u64().is_some_and(|t| t <= 2) => {
                message["params"]["message"].as_str().map(|m| vec![Event::Message(m.to_owned())]).unwrap_or_default()
            }
            (None, Some(_)) => vec![],
            (Some(id), None) => {
                let Some(what) = id.as_u64().and_then(|id| self.pending.remove(&id)) else { return vec![] };
                self.answered(what, message)
            }
            (None, None) => vec![],
        }
    }

    fn answered(&mut self, what: Pending, message: &Value) -> Vec<Event> {
        let error = message["error"]["message"].as_str();
        let result = &message["result"];
        match what {
            Pending::Initialize => {
                if let Some(e) = error {
                    return vec![Event::Failed(e.to_owned())];
                }
                let caps = &result["capabilities"];
                self.utf8 = caps["positionEncoding"].as_str() == Some("utf-8");
                self.can_format = caps["documentFormattingProvider"].as_bool() == Some(true) || caps["documentFormattingProvider"].is_object();
                let colours = &caps["semanticTokensProvider"];
                self.legend = colours["legend"]["tokenTypes"].as_array().map(|a| a.iter().map(|t| t.as_str().unwrap_or_default().to_owned()).collect()).unwrap_or_default();
                self.can_colour = !self.legend.is_empty() && (colours["full"].as_bool() == Some(true) || colours["full"].is_object());
                self.triggers = caps["completionProvider"]["triggerCharacters"].as_array().map(|a| a.iter().filter_map(|c| c.as_str().and_then(|s| s.chars().next())).collect()).unwrap_or_default();
                self.send(&json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }));
                self.ready = true;
                for message in std::mem::take(&mut self.queued) {
                    self.send(&message);
                }
                vec![Event::Ready]
            }
            Pending::Hover => vec![Event::Hover(hover_text(&result["contents"]))],
            Pending::Definition => {
                let first = match result {
                    Value::Array(list) => list.first(),
                    Value::Null => None,
                    one => Some(one),
                };
                let found = first.and_then(|l| {
                    // A plain location, or a link with a target.
                    let uri = l["uri"].as_str().or_else(|| l["targetUri"].as_str())?;
                    let range = if l["targetSelectionRange"].is_object() { &l["targetSelectionRange"] } else { &l["range"] };
                    Some((path_of(uri)?, place(&range["start"])))
                });
                vec![Event::Definition(found)]
            }
            Pending::Completion => {
                let items = if result.is_array() { result } else { &result["items"] };
                let mut list: Vec<(String, Completion)> = items
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|c| {
                                let label = c["label"].as_str()?.to_owned();
                                let raw = c["textEdit"]["newText"].as_str().or_else(|| c["insertText"].as_str()).unwrap_or(&label);
                                // Format 2 is a snippet; the client asked for plain text, but be forgiving.
                                let insert = if c["insertTextFormat"].as_u64() == Some(2) { plain_snippet(raw) } else { raw.to_owned() };
                                let order = c["sortText"].as_str().unwrap_or(&label).to_owned();
                                Some((order, Completion { detail: c["detail"].as_str().map(str::to_owned), insert, label }))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                list.sort_by(|a, b| a.0.cmp(&b.0));
                vec![Event::Completions(list.into_iter().map(|(_, c)| c).collect())]
            }
            Pending::Tokens(path) => {
                // Five numbers a token: lines and columns relative to the
                // token before, a length, a kind, and modifiers.
                let data: Vec<u32> = result["data"].as_array().map(|a| a.iter().map(|n| n.as_u64().unwrap_or(0) as u32).collect()).unwrap_or_default();
                let (mut line, mut start) = (0, 0);
                let mut tokens = Vec::with_capacity(data.len() / 5);
                for t in data.chunks_exact(5) {
                    line += t[0];
                    start = if t[0] == 0 { start + t[1] } else { t[1] };
                    if let Some(kind) = self.legend.get(t[3] as usize) {
                        tokens.push(Token { line, start, length: t[2], kind: kind.clone() });
                    }
                }
                vec![Event::Tokens { path, tokens }]
            }
            Pending::Formatting(path) => {
                if let Some(e) = error {
                    return vec![Event::Message(format!("Could not format: {e}"))];
                }
                let edits = result
                    .as_array()
                    .map(|a| a.iter().map(|e| TextEdit { from: place(&e["range"]["start"]), to: place(&e["range"]["end"]), text: e["newText"].as_str().unwrap_or_default().to_owned() }).collect())
                    .unwrap_or_default();
                vec![Event::Edits { path, edits }]
            }
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if self.ready {
            let id = self.next_id;
            self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": "shutdown" }));
            self.send(&json!({ "jsonrpc": "2.0", "method": "exit" }));
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// The text after making `edits`, which are positions in `lines`.
pub fn apply_edits(lines: &[String], edits: &[(Pos, Pos, String)]) -> String {
    let mut starts = Vec::with_capacity(lines.len());
    let mut total = 0;
    for l in lines {
        starts.push(total);
        total += l.len() + 1;
    }
    let offset = |p: Pos| starts.get(p.line).map_or(total.saturating_sub(1), |s| s + p.col.min(lines[p.line].len()));
    let mut text = lines.join("\n");
    let mut spans: Vec<(usize, usize, &str)> = edits.iter().map(|(a, b, t)| (offset(*a), offset(*b), t.as_str())).collect();
    // Last first, so the earlier offsets stay right.
    spans.sort_by_key(|span| std::cmp::Reverse(span.0));
    for (from, to, new) in spans {
        let (from, to) = (from.min(text.len()), to.clamp(from, text.len()));
        text.replace_range(from..to, new);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    use crate::lsp::testing::Sink;

    fn client() -> (Client, Sink) {
        let sink = Sink::default();
        (Client::new(Box::new(sink.clone()), Path::new("/work/demo")), sink)
    }

    /// A client that has finished its handshake.
    fn ready(capabilities: Value) -> (Client, Sink) {
        let (mut c, sink) = client();
        let id = sink.take()[0]["id"].clone();
        c.handle(&json!({ "jsonrpc": "2.0", "id": id, "result": { "capabilities": capabilities } }));
        sink.take();
        (c, sink)
    }

    fn lines(text: &str) -> Vec<String> {
        text.split('\n').map(str::to_owned).collect()
    }

    #[test]
    fn messages_survive_the_wire_however_they_are_cut_up() {
        let a = json!({ "jsonrpc": "2.0", "method": "x", "params": { "text": "héllo\n" } });
        let b = json!({ "id": 7 });
        let mut bytes = encode(&a);
        assert!(bytes.starts_with(b"Content-Length: "));
        bytes.extend(encode(&b));
        // One byte at a time, as a slow pipe might deliver them.
        struct Drip<'a>(&'a [u8]);
        impl Read for Drip<'_> {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let n = self.0.len().min(1).min(buf.len());
                buf[..n].copy_from_slice(&self.0[..n]);
                self.0 = &self.0[n..];
                Ok(n)
            }
        }
        let mut reader = std::io::BufReader::new(Drip(&bytes));
        assert_eq!(read_message(&mut reader).unwrap(), Some(a));
        assert_eq!(read_message(&mut reader).unwrap(), Some(b));
        assert_eq!(read_message(&mut reader).unwrap(), None, "the end of the output");
        let mut broken = std::io::BufReader::new(&b"X-Other: 1\r\n\r\n{}"[..]);
        assert!(read_message(&mut broken).is_err(), "no length, no message");
    }

    #[test]
    fn file_addresses_round_trip() {
        let path = Path::new("/home/me/my project/naïve#1.rs");
        let address = uri(path);
        assert_eq!(address, "file:///home/me/my%20project/na%C3%AFve%231.rs");
        assert_eq!(path_of(&address).as_deref(), Some(path));
        assert_eq!(path_of("https://example.com/x"), None);
    }

    #[test]
    fn positions_convert_between_bytes_and_utf16_units() {
        // é is two bytes and one unit; the emoji is four bytes and two units.
        let doc = lines("aé😀b\nxy");
        let b = Pos::new(0, 7);
        assert_eq!(to_place(&doc, b, false), (0, 4));
        assert_eq!(to_place(&doc, b, true), (0, 7));
        assert_eq!(to_pos(&doc, (0, 4), false), b);
        assert_eq!(to_pos(&doc, (0, 7), true), b);
        assert_eq!(to_pos(&doc, (0, 2), false), Pos::new(0, 3), "after the é");
        assert_eq!(to_pos(&doc, (0, 99), false), Pos::new(0, 8), "past the end is the end");
        assert_eq!(to_pos(&doc, (9, 0), false), Pos::new(1, 0), "past the last line is the last line");
        assert_eq!(to_pos(&doc, (0, 2), true), Pos::new(0, 1), "never inside a character");
    }

    #[test]
    fn the_handshake_comes_first_and_holds_everything_else_back() {
        let (mut c, sink) = client();
        let hello = sink.take();
        assert_eq!(hello.len(), 1);
        assert_eq!(hello[0]["method"], "initialize");
        assert_eq!(hello[0]["params"]["rootUri"], "file:///work/demo");
        assert_eq!(hello[0]["params"]["capabilities"]["general"]["positionEncodings"][0], "utf-8");

        c.did_open(Path::new("/work/demo/a.rs"), "rust", 1, "fn main() {}");
        c.hover(Path::new("/work/demo/a.rs"), (0, 3));
        assert!(sink.take().is_empty(), "nothing is sent before the server answers");
        assert!(!c.ready());

        let caps = json!({ "positionEncoding": "utf-8", "documentFormattingProvider": true, "completionProvider": { "triggerCharacters": [".", ":"] } });
        let events = c.handle(&json!({ "jsonrpc": "2.0", "id": hello[0]["id"], "result": { "capabilities": caps } }));
        assert_eq!(events, [Event::Ready]);
        assert!(c.ready() && c.utf8 && c.can_format);
        assert_eq!(c.triggers, ['.', ':']);
        let sent: Vec<String> = sink.take().iter().map(|m| m["method"].as_str().unwrap().to_owned()).collect();
        assert_eq!(sent, ["initialized", "textDocument/didOpen", "textDocument/hover"], "then what was waiting, in order");
    }

    #[test]
    fn a_failed_handshake_is_reported() {
        let (mut c, sink) = client();
        let id = sink.take()[0]["id"].clone();
        let events = c.handle(&json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -1, "message": "no toolchain" } }));
        assert_eq!(events, [Event::Failed("no toolchain".into())]);
        assert!(!c.ready());
    }

    #[test]
    fn diagnostics_arrive_with_their_place_and_severity() {
        let (mut c, _) = ready(json!({}));
        let events = c.handle(&json!({ "jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": { "uri": "file:///work/demo/a.rs", "diagnostics": [
            { "range": { "start": { "line": 2, "character": 4 }, "end": { "line": 2, "character": 9 } }, "severity": 2, "message": "unused variable", "source": "rustc" },
            { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 1 } }, "message": "no severity means an error" },
        ] } }));
        let Event::Diagnostics { path, items } = &events[0] else { panic!("{events:?}") };
        assert_eq!(path, Path::new("/work/demo/a.rs"));
        assert_eq!(items[0], Diagnostic { from: (2, 4), to: (2, 9), severity: Severity::Warning, message: "unused variable".into(), source: Some("rustc".into()) });
        assert_eq!(items[1].severity, Severity::Error);
        // An empty list clears them.
        let cleared = c.handle(&json!({ "method": "textDocument/publishDiagnostics", "params": { "uri": "file:///work/demo/a.rs", "diagnostics": [] } }));
        assert_eq!(cleared, [Event::Diagnostics { path: "/work/demo/a.rs".into(), items: vec![] }]);
    }

    #[test]
    fn hover_text_comes_in_several_shapes() {
        let ask = |contents: Value| {
            let (mut c, sink) = ready(json!({}));
            c.hover(Path::new("/work/demo/a.rs"), (1, 2));
            let sent = sink.take();
            assert_eq!(sent[0]["params"]["position"], json!({ "line": 1, "character": 2 }));
            c.handle(&json!({ "id": sent[0]["id"], "result": { "contents": contents } }))
        };
        assert_eq!(ask(json!("plain")), [Event::Hover(Some("plain".into()))]);
        assert_eq!(ask(json!({ "kind": "markdown", "value": "  **bold**\n" })), [Event::Hover(Some("bold".into()))]);
        assert_eq!(ask(json!([{ "language": "rust", "value": "fn f()" }, "docs"])), [Event::Hover(Some("fn f()\n\ndocs".into()))]);
        assert_eq!(ask(json!("")), [Event::Hover(None)]);
        let (mut c, sink) = ready(json!({}));
        c.hover(Path::new("/work/demo/a.rs"), (0, 0));
        let id = sink.take()[0]["id"].clone();
        assert_eq!(c.handle(&json!({ "id": id, "result": null })), [Event::Hover(None)], "nothing to say about that spot");
    }

    #[test]
    fn markdown_is_shown_as_plain_text() {
        // What rust-analyzer sends for a function: its crate, a rule, its signature, its docs.
        let hover = "```rust\nprobe\n```\n\n```rust\nfn answer() -> u32\n```\n\n---\n\nThe **answer**, as a `u32`.\n\n\n# Examples\n";
        assert_eq!(plain_markdown(hover), "probe\n\nfn answer() -> u32\n\nThe answer, as a u32.\n\nExamples");
        assert_eq!(plain_markdown("```\nlet a = `b` ** 2;\n```"), "let a = `b` ** 2;", "code is kept as written");
        assert_eq!(plain_markdown("\n\n"), "");
    }

    #[test]
    fn a_definition_is_the_first_place_offered() {
        let ask = |result: Value| {
            let (mut c, sink) = ready(json!({}));
            c.definition(Path::new("/work/demo/a.rs"), (0, 0));
            let id = sink.take()[0]["id"].clone();
            c.handle(&json!({ "id": id, "result": result }))
        };
        let range = json!({ "start": { "line": 5, "character": 3 }, "end": { "line": 5, "character": 8 } });
        let there = [Event::Definition(Some(("/work/demo/b.rs".into(), (5, 3))))];
        assert_eq!(ask(json!({ "uri": "file:///work/demo/b.rs", "range": range })), there);
        assert_eq!(ask(json!([{ "uri": "file:///work/demo/b.rs", "range": range }, { "uri": "file:///other", "range": range }])), there);
        assert_eq!(ask(json!([{ "targetUri": "file:///work/demo/b.rs", "targetRange": { "start": { "line": 4, "character": 0 }, "end": { "line": 9, "character": 0 } }, "targetSelectionRange": range }])), there, "a link points at the name, not the whole item");
        assert_eq!(ask(json!(null)), [Event::Definition(None)]);
        assert_eq!(ask(json!([])), [Event::Definition(None)]);
    }

    #[test]
    fn completions_are_sorted_and_made_plain() {
        let (mut c, sink) = ready(json!({}));
        c.completion(Path::new("/work/demo/a.rs"), (0, 3));
        let id = sink.take()[0]["id"].clone();
        let events = c.handle(&json!({ "id": id, "result": { "isIncomplete": false, "items": [
            { "label": "zeta", "sortText": "b" },
            { "label": "push(…)", "sortText": "a", "detail": "fn(&mut self, T)", "insertText": "push(${1:value})$0", "insertTextFormat": 2 },
            { "label": "alpha", "sortText": "c", "textEdit": { "newText": "alpha_beta", "range": {} } },
        ] } }));
        let Event::Completions(items) = &events[0] else { panic!("{events:?}") };
        let got: Vec<(&str, &str)> = items.iter().map(|c| (c.label.as_str(), c.insert.as_str())).collect();
        assert_eq!(got, [("push(…)", "push(value)"), ("zeta", "zeta"), ("alpha", "alpha_beta")]);
        assert_eq!(items[0].detail.as_deref(), Some("fn(&mut self, T)"));
        // A bare list works too.
        c.completion(Path::new("/work/demo/a.rs"), (0, 3));
        let id = sink.take()[0]["id"].clone();
        assert_eq!(c.handle(&json!({ "id": id, "result": [{ "label": "one" }] })), [Event::Completions(vec![Completion { label: "one".into(), detail: None, insert: "one".into() }])]);
    }

    #[test]
    fn only_the_latest_completion_request_is_answered() {
        let (mut c, sink) = ready(json!({}));
        c.completion(Path::new("/work/demo/a.rs"), (0, 1));
        let first = sink.take()[0]["id"].clone();
        c.completion(Path::new("/work/demo/a.rs"), (0, 2));
        let second = sink.take()[0]["id"].clone();
        assert!(c.handle(&json!({ "id": first, "result": [{ "label": "stale" }] })).is_empty());
        assert_eq!(c.handle(&json!({ "id": second, "result": [] })), [Event::Completions(vec![])]);
    }

    #[test]
    fn snippets_become_plain_text() {
        assert_eq!(plain_snippet("for ${1:item} in ${2:list} {\n\t$0\n}"), "for item in list {\n\t\n}");
        assert_eq!(plain_snippet("new($1, ${2})"), "new(, )");
        assert_eq!(plain_snippet("cost \\$5"), "cost $5");
        assert_eq!(plain_snippet("plain"), "plain");
        assert_eq!(plain_snippet("odd ${1:unclosed"), "odd ${1:unclosed");
    }

    #[test]
    fn the_server_gets_an_answer_to_whatever_it_asks() {
        let (mut c, sink) = ready(json!({}));
        assert!(c.handle(&json!({ "id": 31, "method": "workspace/configuration", "params": { "items": [{}, {}] } })).is_empty());
        assert!(c.handle(&json!({ "id": "abc", "method": "client/registerCapability", "params": {} })).is_empty());
        let replies = sink.take();
        assert_eq!(replies[0], json!({ "jsonrpc": "2.0", "id": 31, "result": [null, null] }));
        assert_eq!(replies[1], json!({ "jsonrpc": "2.0", "id": "abc", "result": null }));
        // Notifications it does not know are ignored, and warnings are passed on.
        assert!(c.handle(&json!({ "method": "$/progress", "params": {} })).is_empty());
        assert_eq!(c.handle(&json!({ "method": "window/showMessage", "params": { "type": 1, "message": "broken" } })), [Event::Message("broken".into())]);
        assert!(c.handle(&json!({ "method": "window/showMessage", "params": { "type": 3, "message": "fyi" } })).is_empty());
    }

    #[test]
    fn formatting_returns_edits_for_the_file_asked_about() {
        let (mut c, sink) = ready(json!({}));
        c.format(Path::new("/work/demo/a.rs"), 4);
        let sent = sink.take();
        assert_eq!(sent[0]["params"]["options"], json!({ "tabSize": 4, "insertSpaces": true }));
        let events = c.handle(&json!({ "id": sent[0]["id"], "result": [{ "range": { "start": { "line": 0, "character": 2 }, "end": { "line": 0, "character": 5 } }, "newText": " " }] }));
        assert_eq!(events, [Event::Edits { path: "/work/demo/a.rs".into(), edits: vec![TextEdit { from: (0, 2), to: (0, 5), text: " ".into() }] }]);
        c.format(Path::new("/work/demo/a.rs"), 4);
        let id = sink.take()[0]["id"].clone();
        assert_eq!(c.handle(&json!({ "id": id, "error": { "message": "syntax error" } })), [Event::Message("Could not format: syntax error".into())]);
    }

    #[test]
    fn tokens_are_decoded_against_the_servers_own_names() {
        let (mut c, sink) = ready(json!({ "semanticTokensProvider": { "legend": { "tokenTypes": ["keyword", "function", "variable"], "tokenModifiers": [] }, "full": { "delta": false } } }));
        assert!(c.can_colour);
        let path = Path::new("/work/demo/a.rs");
        c.tokens(path);
        c.tokens(path);
        let sent = sink.take();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[1]["method"], "textDocument/semanticTokens/full");
        // `fn main` on line 0, then `x` two lines down, then `main` again on that line.
        let data = json!([0, 0, 2, 0, 0, 0, 3, 4, 1, 0, 2, 4, 1, 2, 0, 0, 6, 4, 1, 0, 0, 9, 1, 7, 0]);
        assert!(c.handle(&json!({ "id": sent[0]["id"], "result": { "data": data } })).is_empty(), "the earlier request was dropped");
        let events = c.handle(&json!({ "id": sent[1]["id"], "result": { "data": data } }));
        let Event::Tokens { tokens, .. } = &events[0] else { panic!("{events:?}") };
        let got: Vec<(u32, u32, u32, &str)> = tokens.iter().map(|t| (t.line, t.start, t.length, t.kind.as_str())).collect();
        assert_eq!(got, [(0, 0, 2, "keyword"), (0, 3, 4, "function"), (2, 4, 1, "variable"), (2, 10, 4, "function")], "a kind the server never named is left out");
        // When the server's view changes it says so, and gets its answer.
        assert_eq!(c.handle(&json!({ "id": 5, "method": "workspace/semanticTokens/refresh" })), [Event::RefreshTokens]);
        assert_eq!(sink.take()[0]["id"], 5);
        // A server that cannot do this is not asked.
        let (mut plain, sink) = ready(json!({}));
        plain.tokens(path);
        assert!(sink.take().is_empty());
    }

    #[test]
    fn edits_apply_together_wherever_they_are() {
        let doc = lines("fn  main( ){\nlet x=1;\n}");
        let edits = vec![
            (Pos::new(0, 2), Pos::new(0, 4), " ".to_owned()),
            (Pos::new(0, 9), Pos::new(0, 10), String::new()),
            (Pos::new(0, 11), Pos::new(0, 11), " ".to_owned()),
            (Pos::new(1, 0), Pos::new(1, 0), "    ".to_owned()),
            (Pos::new(1, 5), Pos::new(1, 6), " = ".to_owned()),
        ];
        assert_eq!(apply_edits(&doc, &edits), "fn main() {\n    let x = 1;\n}");
        // An edit can span lines, and none leaves the text alone.
        assert_eq!(apply_edits(&doc, &[(Pos::new(0, 12), Pos::new(2, 0), String::new())]), "fn  main( ){}");
        assert_eq!(apply_edits(&doc, &[]), doc.join("\n"));
    }

    /// Against a real rust-analyzer, if one is installed. Slow, and needs a
    /// Rust toolchain, so it runs only when asked for:
    /// `cargo test -p neo-code -- --ignored real_server`.
    #[test]
    #[ignore]
    fn real_server_reports_a_type_error_and_answers_hover() {
        use crate::lsp::{find_in, search_dirs, SERVERS};
        let spec = SERVERS.iter().find(|s| s.name == "rust-analyzer").unwrap();
        let dirs = search_dirs();
        let found = find_in(&dirs, spec);
        assert!(!found.is_empty(), "rust-analyzer is not installed");
        let root = std::env::temp_dir().join(format!("neo-code-ra-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
        let source = "fn answer() -> u32 {\n    \"forty-two\"\n}\n\nfn main() {\n    let _ = answer();\n}\n";
        let file = root.join("src/main.rs");
        std::fs::write(&file, source).unwrap();
        let root = root.canonicalize().unwrap();
        let file = root.join("src/main.rs");

        // Try each copy found, as the app does: a rustup proxy without the
        // component installed exits at once.
        let deadline = std::time::Instant::now() + Duration::from_secs(240);
        let mut outcome = None;
        for candidate in &found {
            let (tx, rx) = std::sync::mpsc::channel();
            let Ok(mut client) = Client::spawn(candidate, &root, &dirs, move |m| {
                let _ = tx.send(m);
            }) else { continue };
            client.did_open(&file, "rust", 1, source);
            client.tokens(&file);
            let (mut error, mut hover, mut asked) = (None, None, false);
            let mut kinds: Vec<(u32, String)> = vec![];
            while std::time::Instant::now() < deadline && (error.is_none() || hover.is_none() || kinds.is_empty()) {
                let Ok(Incoming::Message(m)) = rx.recv_timeout(Duration::from_secs(120)) else { break };
                for event in client.handle(&m) {
                    match event {
                        Event::Diagnostics { path, items } if path == file => error = items.into_iter().find(|d| d.severity == Severity::Error).or(error),
                        Event::Hover(Some(text)) => hover = Some(text),
                        Event::Tokens { tokens, .. } => kinds = tokens.into_iter().map(|t| (t.line, t.kind)).collect(),
                        Event::RefreshTokens => client.tokens(&file),
                        // Still indexing: ask again shortly, as someone would.
                        Event::Hover(None) => {
                            std::thread::sleep(Duration::from_secs(1));
                            client.hover(&file, (5, 13));
                        }
                        _ => {}
                    }
                }
                // Hover on `answer` in main, once the server has analysed enough to report problems.
                if error.is_some() && !asked {
                    asked = true;
                    client.hover(&file, (5, 13));
                }
            }
            if client.ready() {
                outcome = Some((candidate.program.clone(), error, hover, kinds));
                break;
            }
        }
        let _ = std::fs::remove_dir_all(&root);
        let (program, error, hover, kinds) = outcome.expect("no copy of rust-analyzer would start");
        assert!(kinds.contains(&(0, "function".into())) && kinds.contains(&(1, "string".into())), "the function on the first line and the string on the second are named: {kinds:?}");
        let error = error.unwrap_or_else(|| panic!("{} reported no error", program.display()));
        assert_eq!(error.from.0, 1, "on the line with the string: {error:?}");
        assert!(error.message.contains("mismatched") || error.message.contains("expected"), "{}", error.message);
        let hover = hover.expect("an answer to hover");
        assert!(hover.contains("answer") && hover.contains("u32"), "{hover}");
    }
}
