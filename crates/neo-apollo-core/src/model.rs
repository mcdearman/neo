//! The model that describes, embeds and answers.
//!
//! It runs on this computer, served by Ollama: one model that can look at
//! pictures and hold a conversation, and a small one that turns text into
//! embeddings. Nothing is sent anywhere else.

use std::io::{BufRead, BufReader};
use std::sync::OnceLock;
use std::time::Duration;

use base64::Engine;
use serde_json::{Value, json};

/// Who said something in a conversation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Instructions to the model, not shown.
    System,
    User,
    Assistant,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Turn {
    pub role: Role,
    pub text: String,
}

impl Turn {
    pub fn new(role: Role, text: impl Into<String>) -> Self {
        Self { role, text: text.into() }
    }
}

/// What the model made of a picture.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Seen {
    /// A sentence or two on what it shows.
    pub text: String,
    /// What it is of, in a few words each.
    pub words: Vec<String>,
}

/// What Apollo needs of a model. The real one is [`Ollama`]; tests have
/// their own, which answers at once and the same way every time.
pub trait Model: Send + Sync {
    /// What makes its embeddings, by name: embeddings from two different
    /// makers cannot be compared, so a memory keeps to the one it began with.
    fn name(&self) -> String;

    /// The length of the embeddings it makes.
    fn dims(&self) -> Result<usize, String>;

    /// An embedding of each text. `question` says they are things asked,
    /// to be matched against things stored, which some models want to know.
    fn embed(&self, texts: &[String], question: bool) -> Result<Vec<Vec<f32>>, String>;

    /// Says what a picture, given as a JPEG, shows.
    fn describe(&self, jpeg: &[u8]) -> Result<Seen, String>;

    /// Lets go of what reading needed and waiting to be asked does not:
    /// the model that sees, which is the large one. A model that holds
    /// nothing need do nothing.
    fn rest(&self) {}

    /// Lets go of everything it holds in memory, for when Apollo closes.
    fn rest_all(&self) {
        self.rest();
    }

    /// Answers a conversation. `piece` is given the answer as it comes;
    /// returning false from it stops the answer there.
    fn chat(&self, turns: &[Turn], piece: &mut dyn FnMut(&str) -> bool) -> Result<String, String>;
}

/// Models served by Ollama: on this computer, and, for any of the three
/// jobs that is given to it, on another computer of the user's that has
/// room for models this one has not.
pub struct Ollama {
    /// The server on this computer.
    server: String,
    /// The server on the other computer, if one is used.
    remote: Option<String>,
    chat_model: String,
    vision_model: String,
    embed_model: String,
    /// Whether each job (answering, seeing, embedding) is done on the other computer.
    away: [bool; 3],
    dims: OnceLock<usize>,
}

/// The three jobs a model is chosen for.
const ANSWER: usize = 0;
const SEE: usize = 1;
const EMBED: usize = 2;

/// How far a model's download has got.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pulling {
    pub status: String,
    pub done: u64,
    pub total: u64,
}

/// How long a model stays in memory after it was last asked something.
/// The one that sees is the large one, and is let go soon after a reading.
const KEEP_SEEING: &str = "2m";
/// The model that answers is small and back in a second or two, so it
/// need not sit in memory for long between questions.
const KEEP_ANSWERING: &str = "5m";
/// The embedding model is a few hundred megabytes and every search needs
/// it, so it stays: this is what Apollo holds while it waits to be asked.
const KEEP_EMBEDDING: &str = "30m";

const DESCRIBE: &str = "Describe this image for someone who will search for it later by what is in it. In `description`, say in two to four plain sentences what it shows: the people, animals, objects, place, activity, and any text that can be read. For each person or character, say whether they look like a man, woman, boy or girl, the colour and style of their hair, what they are wearing and its colours, and their expression, since searches are often for such details. Name a well-known person or character only if you are sure who it is. If it is a screenshot, say what app or page it shows and what is on it. In `tags`, give six to ten single words or short phrases for what is actually in this image, most important first, lower case; where there are people, include their telling details, such as the colour of their hair. Describe only what you can see.";

fn agent(wait: Option<Duration>) -> ureq::Agent {
    ureq::Agent::config_builder().timeout_connect(Some(Duration::from_secs(3))).timeout_recv_response(wait).http_status_as_error(false).build().into()
}

impl Ollama {
    pub fn new(server: &str, chat_model: &str, vision_model: &str, embed_model: &str) -> Self {
        Self { server: server.trim_end_matches('/').to_owned(), remote: None, chat_model: chat_model.to_owned(), vision_model: vision_model.to_owned(), embed_model: embed_model.to_owned(), away: [false; 3], dims: OnceLock::new() }
    }

    /// Has the jobs marked in `away` (answering, seeing, embedding) done by
    /// the server at `remote`, on another computer. With no address, all
    /// stay here.
    pub fn with_remote(mut self, remote: &str, away: [bool; 3]) -> Self {
        let remote = remote.trim().trim_end_matches('/');
        if !remote.is_empty() {
            // An address given bare is taken to be plain HTTP on Ollama's port.
            let with_scheme = if remote.contains("://") { remote.to_owned() } else { format!("http://{remote}") };
            let has_port = with_scheme.rsplit_once("://").is_some_and(|(_, rest)| rest.rsplit_once(':').is_some_and(|(_, port)| port.chars().all(|c| c.is_ascii_digit()) && !port.is_empty()));
            self.remote = Some(if has_port || with_scheme.starts_with("https://") { with_scheme } else { format!("{with_scheme}:11434") });
            self.away = away;
        }
        self
    }

    pub fn from_settings(s: &crate::settings::Settings) -> Self {
        Self::new(&s.server, &s.chat_model, &s.vision_model, &s.embed_model).with_remote(&s.remote, [s.chat_remote, s.vision_remote, s.embed_remote])
    }

    /// The other computer's address as it is used, if one is.
    pub fn remote(&self) -> Option<&str> {
        self.remote.as_deref()
    }

    /// The server that does a job.
    fn at(&self, job: usize) -> &str {
        self.place(self.away[job])
    }

    fn place(&self, away: bool) -> &str {
        match &self.remote {
            Some(remote) if away => remote,
            _ => &self.server,
        }
    }

    /// Each job's model and whether it is done on the other computer.
    fn jobs(&self) -> [(&String, bool); 3] {
        [(&self.chat_model, self.away[ANSWER] && self.remote.is_some()), (&self.vision_model, self.away[SEE] && self.remote.is_some()), (&self.embed_model, self.away[EMBED] && self.remote.is_some())]
    }

    pub fn chat_model(&self) -> &str {
        &self.chat_model
    }

    pub fn embed_model(&self) -> &str {
        &self.embed_model
    }

    fn post(&self, server: &str, path: &str, body: &Value, wait: Option<Duration>) -> Result<ureq::http::Response<ureq::Body>, String> {
        let res = agent(wait).post(&format!("{server}{path}")).send_json(body).map_err(|e| self.silent(server, &e.to_string()))?;
        if res.status().is_success() {
            return Ok(res);
        }
        let status = res.status();
        let mut res = res;
        let said = res.body_mut().read_json::<Value>().ok().and_then(|v| v["error"].as_str().map(str::to_owned));
        Err(said.unwrap_or_else(|| format!("The model server said {status}.")))
    }

    /// What to say of a server that did not answer.
    fn silent(&self, server: &str, why: &str) -> String {
        if self.remote.as_deref() == Some(server) { format!("The other computer, at {server}, did not answer: {why}") } else { format!("The model server did not answer: {why}") }
    }

    /// Has the servers unload the models of these jobs now, which gives
    /// their memory back, and not when they have sat idle for a while.
    fn unload(&self, jobs: &[usize]) {
        let mut done: Vec<(&String, bool)> = vec![];
        for job in jobs {
            let (model, away) = self.jobs()[*job];
            if !done.contains(&(model, away)) {
                // Asking for nothing, to be kept for no time, is how the server is told.
                let _ = self.post(self.place(away), if *job == EMBED { "/api/embed" } else { "/api/generate" }, &json!({ "model": model, "keep_alive": 0 }), Some(Duration::from_secs(5)));
                done.push((model, away));
            }
        }
    }

    fn answers(&self, server: &str) -> bool {
        agent(Some(Duration::from_secs(3))).get(&format!("{server}/api/version")).call().is_ok_and(|r| r.status().is_success())
    }

    /// Whether the server on this computer is there.
    pub fn running(&self) -> bool {
        self.answers(&self.server)
    }

    /// Makes sure the servers that are used are there: the one on this
    /// computer is started if it is not running; the other computer's can
    /// only be asked.
    pub fn start(&self) -> Result<(), String> {
        if let Some(remote) = self.remote.as_deref().filter(|_| self.jobs().iter().any(|(_, away)| *away))
            && !self.answers(remote)
        {
            return Err(format!("The other computer, at {remote}, is not answering. Ollama must be running there and listening on the network: start it with OLLAMA_HOST=0.0.0.0 ollama serve."));
        }
        // With every job given away, nothing is needed here.
        if self.running() || self.jobs().iter().all(|(_, away)| *away) {
            return Ok(());
        }
        let program = neo_desktop::fs::tool("ollama");
        if !program.is_file() {
            return Err("Ollama, which runs Apollo's model, is not installed. Get it with: cargo xtask deps --install".into());
        }
        std::process::Command::new(&program).arg("serve").stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().map_err(|e| format!("Couldn't start Ollama: {e}"))?;
        for _ in 0..50 {
            std::thread::sleep(Duration::from_millis(200));
            if self.running() {
                return Ok(());
            }
        }
        Err("Ollama was started but is not answering.".into())
    }

    /// The models the server on this computer has, by name.
    pub fn models(&self) -> Result<Vec<String>, String> {
        self.models_at(false)
    }

    /// The models a server has: this computer's, or the other's.
    pub fn models_at(&self, away: bool) -> Result<Vec<String>, String> {
        let server = self.place(away);
        let mut res = agent(Some(Duration::from_secs(5))).get(&format!("{server}/api/tags")).call().map_err(|e| self.silent(server, &e.to_string()))?;
        let v: Value = res.body_mut().read_json().map_err(|e| e.to_string())?;
        Ok(v["models"].as_array().map(|m| m.iter().filter_map(|m| m["name"].as_str().map(str::to_owned)).collect()).unwrap_or_default())
    }

    /// Those of the models Apollo uses that the server meant to run them
    /// does not have yet.
    pub fn missing(&self) -> Result<Vec<String>, String> {
        // "nomic-embed-text" is had as "nomic-embed-text:latest".
        let has = |have: &[String], want: &str| have.iter().any(|h| h == want || h.strip_suffix(":latest") == Some(want) || want.strip_suffix(":latest") == Some(h));
        let jobs = self.jobs();
        let here = if jobs.iter().any(|(_, away)| !away) { self.models_at(false)? } else { vec![] };
        let there = if jobs.iter().any(|(_, away)| *away) { self.models_at(true)? } else { vec![] };
        let mut missing: Vec<String> = vec![];
        for (model, away) in jobs {
            if !has(if away { &there } else { &here }, model) && !missing.contains(model) {
                missing.push(model.clone());
            }
        }
        Ok(missing)
    }

    /// What a model the server has can do: `completion`, `vision`,
    /// `embedding`, `tools` and the like.
    pub fn capabilities(&self, model: &str) -> Result<Vec<String>, String> {
        self.capabilities_at(model, false)
    }

    /// As [`capabilities`](Self::capabilities), of a model on this
    /// computer's server or the other's.
    pub fn capabilities_at(&self, model: &str, away: bool) -> Result<Vec<String>, String> {
        let mut res = self.post(self.place(away), "/api/show", &json!({ "model": model }), Some(Duration::from_secs(10)))?;
        let v: Value = res.body_mut().read_json().map_err(|e| e.to_string())?;
        Ok(v["capabilities"].as_array().map(|c| c.iter().filter_map(|c| c.as_str().map(str::to_owned)).collect()).unwrap_or_default())
    }

    /// Why the models chosen cannot do the jobs they were chosen for, if
    /// any cannot: a model to describe pictures that cannot see, or one
    /// to place text by meaning that makes no embeddings. A server too old
    /// to say what its models can do is taken at its word.
    pub fn unfit(&self) -> Option<String> {
        let lacks = |job: usize, need: &str| self.capabilities_at(self.jobs()[job].0, self.jobs()[job].1).is_ok_and(|c| !c.is_empty() && !c.iter().any(|c| c == need));
        if lacks(SEE, "vision") {
            Some(format!("{} cannot look at pictures. Choose a model that can see to describe them, such as gemma3 or one of the qwen vision models.", self.vision_model))
        } else if lacks(EMBED, "embedding") {
            Some(format!("{} does not make embeddings. Choose an embedding model, such as nomic-embed-text.", self.embed_model))
        } else if lacks(ANSWER, "completion") {
            Some(format!("{} cannot hold a conversation. Choose a chat model to answer with.", self.chat_model))
        } else {
            None
        }
    }

    /// Downloads a model to wherever it is to run (onto the other computer,
    /// for a job done there), telling `progress` how far it has got.
    /// Returning false from `progress` gives up; what has come down is
    /// kept for next time.
    pub fn pull(&self, model: &str, progress: &mut dyn FnMut(Pulling) -> bool) -> Result<(), String> {
        let mut places: Vec<bool> = vec![];
        for (wanted, away) in self.jobs() {
            if wanted == model && !places.contains(&away) {
                places.push(away);
            }
        }
        if places.is_empty() {
            places.push(false);
        }
        for away in places {
            let mut res = self.post(self.place(away), "/api/pull", &json!({ "model": model, "stream": true }), None)?;
            for line in BufReader::new(res.body_mut().as_reader()).lines() {
                let line = line.map_err(|e| format!("The download stopped: {e}"))?;
                let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                if let Some(e) = v["error"].as_str() {
                    return Err(e.to_owned());
                }
                if !progress(Pulling { status: v["status"].as_str().unwrap_or_default().to_owned(), done: v["completed"].as_u64().unwrap_or(0), total: v["total"].as_u64().unwrap_or(0) }) {
                    return Err("Stopped.".into());
                }
            }
        }
        Ok(())
    }
}

/// Reads the model's answer about a picture, which was asked for as JSON.
pub fn read_seen(answer: &str) -> Seen {
    match serde_json::from_str::<Value>(answer.trim()) {
        Ok(v) => Seen { text: v["description"].as_str().unwrap_or_default().trim().to_owned(), words: crate::words::clean_all(v["tags"].as_array().into_iter().flatten().filter_map(Value::as_str), 10) },
        // Not what was asked for: what it said is still a description.
        Err(_) => Seen { text: answer.trim().to_owned(), words: vec![] },
    }
}

impl Model for Ollama {
    fn name(&self) -> String {
        self.embed_model.clone()
    }

    fn rest(&self) {
        self.unload(&[SEE]);
    }

    fn rest_all(&self) {
        self.unload(&[ANSWER, SEE, EMBED]);
    }

    fn dims(&self) -> Result<usize, String> {
        if let Some(d) = self.dims.get() {
            return Ok(*d);
        }
        let d = self.embed(&["dimensions".to_owned()], false)?.first().map_or(0, Vec::len);
        if d == 0 {
            return Err(format!("{} gave no embedding.", self.embed_model));
        }
        Ok(*self.dims.get_or_init(|| d))
    }

    fn embed(&self, texts: &[String], question: bool) -> Result<Vec<Vec<f32>>, String> {
        if texts.is_empty() {
            return Ok(vec![]);
        }
        // Nomic's models are trained with a word in front saying which side
        // of a search a text is on.
        let prefix = if !self.embed_model.starts_with("nomic-embed") {
            ""
        } else if question {
            "search_query: "
        } else {
            "search_document: "
        };
        let input: Vec<String> = texts.iter().map(|t| format!("{prefix}{t}")).collect();
        let mut res = self.post(self.at(EMBED), "/api/embed", &json!({ "model": self.embed_model, "input": input, "keep_alive": KEEP_EMBEDDING, "truncate": true }), Some(Duration::from_secs(300)))?;
        let v: Value = res.body_mut().read_json().map_err(|e| e.to_string())?;
        let out: Vec<Vec<f32>> = v["embeddings"].as_array().map(|all| all.iter().map(|e| e.as_array().map(|e| e.iter().map(|f| f.as_f64().unwrap_or(0.0) as f32).collect()).unwrap_or_default()).collect()).unwrap_or_default();
        if out.len() != texts.len() {
            return Err(format!("{} embeddings came back for {} texts.", out.len(), texts.len()));
        }
        Ok(out)
    }

    fn describe(&self, jpeg: &[u8]) -> Result<Seen, String> {
        let picture = base64::engine::general_purpose::STANDARD.encode(jpeg);
        let format = json!({ "type": "object", "properties": { "description": { "type": "string" }, "tags": { "type": "array", "items": { "type": "string" } } }, "required": ["description", "tags"] });
        let body = json!({ "model": self.vision_model, "stream": false, "keep_alive": KEEP_SEEING, "format": format, "options": { "temperature": 0.2 }, "messages": [{ "role": "user", "content": DESCRIBE, "images": [picture] }] });
        let mut res = self.post(self.at(SEE), "/api/chat", &body, Some(Duration::from_secs(600)))?;
        let v: Value = res.body_mut().read_json().map_err(|e| e.to_string())?;
        let seen = read_seen(v["message"]["content"].as_str().unwrap_or_default());
        if seen.text.is_empty() { Err("The model said nothing about the picture.".into()) } else { Ok(seen) }
    }

    fn chat(&self, turns: &[Turn], piece: &mut dyn FnMut(&str) -> bool) -> Result<String, String> {
        let messages: Vec<Value> = turns
            .iter()
            .map(|t| {
                let role = match t.role {
                    Role::System => "system",
                    Role::User => "user",
                    Role::Assistant => "assistant",
                };
                json!({ "role": role, "content": t.text })
            })
            .collect();
        let mut res = self.post(self.at(ANSWER), "/api/chat", &json!({ "model": self.chat_model, "stream": true, "keep_alive": KEEP_ANSWERING, "messages": messages }), Some(Duration::from_secs(600)))?;
        let mut said = String::new();
        for line in BufReader::new(res.body_mut().as_reader()).lines() {
            let line = line.map_err(|e| format!("The answer stopped: {e}"))?;
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            if let Some(e) = v["error"].as_str() {
                return Err(e.to_owned());
            }
            if let Some(text) = v["message"]["content"].as_str().filter(|t| !t.is_empty()) {
                said.push_str(text);
                if !piece(text) {
                    break;
                }
            }
            if v["done"].as_bool() == Some(true) {
                break;
            }
        }
        Ok(said)
    }
}

/// A model for tests: it answers at once and the same way every time.
/// Its embeddings have one place for each of a few subjects, so texts on
/// the same subject come out near each other.
pub mod fake {
    use super::*;

    pub const SUBJECTS: [&[&str]; 6] = [&["dog", "dogs", "puppy", "retriever"], &["beach", "sea", "sand", "waves", "seaside"], &["invoice", "payment", "supplier", "bank"], &["mountain", "snow", "hiking", "peak"], &["cat", "cats", "kitten"], &["code", "editor", "terminal", "screenshot"]];

    #[derive(Default)]
    pub struct Fake {
        /// What it says of every picture, if not what the picture's name suggests.
        pub sees: Option<Seen>,
    }

    pub fn vector(text: &str) -> Vec<f32> {
        let lower = text.to_lowercase();
        let mut v: Vec<f32> = SUBJECTS.iter().map(|words| words.iter().filter(|w| lower.split(|c: char| !c.is_alphanumeric()).any(|t| t == **w)).count() as f32).collect();
        // A little of the text itself, so that no two are quite the same
        // and none is nothing at all.
        v.push(0.05 + (lower.len() % 7) as f32 * 0.01);
        v.push(0.05 + (lower.bytes().map(u32::from).sum::<u32>() % 11) as f32 * 0.01);
        v
    }

    impl Model for Fake {
        fn name(&self) -> String {
            "fake".into()
        }

        fn dims(&self) -> Result<usize, String> {
            Ok(SUBJECTS.len() + 2)
        }

        fn embed(&self, texts: &[String], _question: bool) -> Result<Vec<Vec<f32>>, String> {
            Ok(texts.iter().map(|t| vector(t)).collect())
        }

        fn describe(&self, jpeg: &[u8]) -> Result<Seen, String> {
            if let Some(seen) = &self.sees {
                return Ok(seen.clone());
            }
            // Tests' pictures are plain colours: say which.
            let colour = image::load_from_memory(jpeg).map_err(|e| e.to_string())?.to_rgb8().get_pixel(0, 0).0;
            let (name, words): (&str, &[&str]) = match colour {
                [r, g, b] if r > 150 && g < 110 && b < 110 => ("A dog running on a beach.", &["dog", "beach"]),
                [r, g, b] if g > 150 && r < 110 && b < 110 => ("A snowy mountain peak.", &["mountain", "snow"]),
                [r, g, b] if b > 150 && r < 110 && g < 110 => ("Waves on the sea.", &["sea", "waves"]),
                _ => ("A cat asleep.", &["cat"]),
            };
            Ok(Seen { text: name.into(), words: words.iter().map(|w| (*w).to_owned()).collect() })
        }

        fn chat(&self, turns: &[Turn], piece: &mut dyn FnMut(&str) -> bool) -> Result<String, String> {
            // The question is put with what bears on it set out before it, numbered.
            let put = turns.iter().rev().find(|t| t.role == Role::User).map(|t| t.text.as_str()).unwrap_or_default();
            let asked = put.split("\nQuestion: ").nth(1).map_or(put, |rest| rest.lines().next().unwrap_or_default()).lines().next().unwrap_or_default();
            let knows = put.lines().filter(|l| l.split_once(". ").is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))).count();
            let answer = format!("You asked: {asked} I looked at {knows} memories.");
            let mut said = String::new();
            for word in answer.split_inclusive(' ') {
                said.push_str(word);
                if !piece(word) {
                    break;
                }
            }
            Ok(said)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_the_model_says_of_a_picture_is_read() {
        let seen = read_seen(r#" {"description": " A dog on a beach. ", "tags": ["Dog", "Beach", "dog", "the", "golden retriever"]} "#);
        assert_eq!(seen, Seen { text: "A dog on a beach.".into(), words: vec!["dog".into(), "beach".into(), "golden retriever".into()] });
        assert_eq!(read_seen("Just a sentence."), Seen { text: "Just a sentence.".into(), words: vec![] });
        assert_eq!(read_seen(r#"{"tags": []}"#), Seen::default());
    }

    #[test]
    fn no_server_is_said_plainly_and_not_waited_on() {
        // Nothing listens on the discard port.
        let m = Ollama::new("http://127.0.0.1:9/", "chat", "see", "embed");
        assert!(!m.running());
        assert!(m.embed(&["x".into()], false).unwrap_err().contains("did not answer"));
        assert!(m.models().is_err() && m.missing().is_err() && m.dims().is_err() && m.capabilities("chat").is_err());
        assert_eq!((m.unfit(), m.name()), (None, "embed".to_owned()), "a server that cannot be asked is not held against the models");
        assert_eq!(m.embed(&[], false), Ok(vec![]), "nothing to embed asks nothing");
    }

    /// A stand-in for an Ollama server: it has the models it is given, and
    /// notes what it was asked and for which model.
    fn server(has: &'static [&'static str]) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let asked = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
        let log = asked.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut raw = vec![];
                let mut buf = [0u8; 4096];
                // The head, and then as much body as it says there is.
                let (head, body) = loop {
                    let Ok(n) = stream.read(&mut buf) else { break (String::new(), String::new()) };
                    raw.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&raw).into_owned();
                    if let Some((head, body)) = text.split_once("\r\n\r\n") {
                        let lower = head.to_lowercase();
                        let length = lower.lines().find_map(|l| l.strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)));
                        // Sent whole, with its length said, or in pieces that end with an empty one.
                        let all = match length {
                            Some(length) => body.len() >= length,
                            None if lower.contains("transfer-encoding: chunked") => body.ends_with("0\r\n\r\n"),
                            None => true,
                        };
                        if all || n == 0 {
                            // What is between the first brace and the last is the JSON, pieces or not.
                            let json = body.find('{').zip(body.rfind('}')).map_or(String::new(), |(from, to)| body[from..=to].to_owned());
                            break (head.to_owned(), json);
                        }
                    } else if n == 0 {
                        break (text, String::new());
                    }
                };
                let path = head.split_whitespace().nth(1).unwrap_or_default().to_owned();
                let model = serde_json::from_str::<Value>(&body).ok().and_then(|v| v["model"].as_str().map(str::to_owned)).unwrap_or_default();
                log.lock().unwrap().push(format!("{path} {model}").trim().to_owned());
                let answer = match path.as_str() {
                    "/api/version" => json!({ "version": "test" }).to_string(),
                    "/api/tags" => json!({ "models": has.iter().map(|m| json!({ "name": m })).collect::<Vec<_>>() }).to_string(),
                    "/api/show" => json!({ "capabilities": if model.contains("embed") { vec!["embedding"] } else { vec!["completion", "vision"] } }).to_string(),
                    "/api/embed" => json!({ "embeddings": [[0.5, 0.5]] }).to_string(),
                    "/api/chat" if serde_json::from_str::<Value>(&body).is_ok_and(|v| v["stream"] == true) => format!("{}\n", json!({ "message": { "content": "From afar." }, "done": true })),
                    "/api/chat" => json!({ "message": { "content": "{\"description\": \"A hill.\", \"tags\": [\"hill\"]}" } }).to_string(),
                    _ => "{}".to_owned(),
                };
                let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}", answer.len());
            }
        });
        (address, asked)
    }

    #[test]
    fn a_job_given_to_another_computer_is_asked_of_it_and_the_rest_stay_here() {
        let (here, asked_here) = server(&["small:latest", "embed"]);
        let (there, asked_there) = server(&["large", "sees"]);
        // Answering and seeing go to the other computer; embedding stays.
        let m = Ollama::new(&here, "large", "sees", "embed").with_remote(&there, [true, true, false]);
        assert_eq!(m.remote(), Some(there.as_str()));
        m.start().unwrap();
        assert_eq!(m.missing().unwrap(), Vec::<String>::new(), "each looked for where it is to run");
        assert_eq!(m.unfit(), None);
        assert_eq!(m.chat(&[Turn::new(Role::User, "hello")], &mut |_| true).unwrap(), "From afar.");
        assert_eq!(m.describe(b"jpeg").unwrap(), Seen { text: "A hill.".into(), words: vec!["hill".into()] });
        assert_eq!(m.embed(&["x".into()], false).unwrap(), vec![vec![0.5, 0.5]]);
        m.rest_all();
        let (here_log, there_log) = (asked_here.lock().unwrap().clone(), asked_there.lock().unwrap().clone());
        assert!(there_log.contains(&"/api/chat large".to_owned()) && there_log.contains(&"/api/chat sees".to_owned()) && there_log.contains(&"/api/generate large".to_owned()), "{there_log:?}");
        assert!(here_log.contains(&"/api/embed embed".to_owned()) && here_log.iter().all(|a| !a.contains("large") && !a.contains("sees")), "nothing of what was given away is asked here: {here_log:?}");
        assert!(there_log.iter().all(|a| !a.contains("/api/embed")), "and the embedding never leaves: {there_log:?}");
        assert_eq!((m.models_at(true).unwrap(), m.models().unwrap()), (vec!["large".to_owned(), "sees".to_owned()], vec!["small:latest".to_owned(), "embed".to_owned()]));
        // A model the other computer lacks is missing though this one has it, and the other way about.
        let m = Ollama::new(&here, "small", "sees", "embed").with_remote(&there, [true, false, false]);
        assert_eq!(m.missing().unwrap(), ["small".to_owned(), "sees".to_owned()]);
        // The other computer not answering is said as that, and nothing is asked of it.
        let gone = Ollama::new(&here, "large", "sees", "embed").with_remote("127.0.0.1:9", [true, false, false]);
        assert_eq!(gone.remote(), Some("http://127.0.0.1:9"));
        assert!(gone.start().unwrap_err().starts_with("The other computer, at http://127.0.0.1:9, is not answering."));
        assert!(gone.chat(&[Turn::new(Role::User, "hello")], &mut |_| true).unwrap_err().starts_with("The other computer, at http://127.0.0.1:9, did not answer"));
        // An address is filled out; none means everything stays here.
        assert_eq!(Ollama::new(&here, "a", "b", "c").with_remote(" studio.local ", [true; 3]).remote(), Some("http://studio.local:11434"));
        assert_eq!(Ollama::new(&here, "a", "b", "c").with_remote("https://models.example.com/", [true; 3]).remote(), Some("https://models.example.com"));
        let none = Ollama::new(&here, "small", "small", "embed").with_remote("  ", [true; 3]);
        assert_eq!((none.remote(), none.missing().unwrap()), (None, vec![]));
    }

    #[test]
    fn the_test_model_puts_like_with_like() {
        use crate::store::distance;
        let v = |t: &str| fake::vector(t);
        assert!(distance(&v("a dog and a puppy"), &v("dogs")) < distance(&v("a dog and a puppy"), &v("an invoice")));
        assert!(distance(&v("dog"), &v("dogs")) < 0.2);
        assert_eq!(fake::Fake::default().dims().unwrap(), v("anything").len());
    }
}

#[cfg(test)]
mod probe {
    use super::*;

    /// By hand, against the real server: `NEO_APOLLO_PICTURE=file cargo test -p neo-apollo real_model -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn the_real_model_sees_embeds_and_answers() {
        // With NEO_APOLLO_REMOTE, answering and seeing are asked of the server there.
        let m = Ollama::from_settings(&crate::settings::Settings::default()).with_remote(&std::env::var("NEO_APOLLO_REMOTE").unwrap_or_default(), [true, true, false]);
        m.start().unwrap();
        println!("remote: {:?}; there: {:?}", m.remote(), m.remote().map(|_| m.models_at(true)));
        println!("missing: {:?}; dims {}", m.missing().unwrap(), m.dims().unwrap());
        let began = std::time::Instant::now();
        let v = m.embed(&["dog".into(), "dogs".into(), "puppy".into(), "beach".into(), "seaside".into(), "invoice".into(), "screenshot".into(), "screen capture".into()], false).unwrap();
        println!("embedded in {:?}", began.elapsed());
        let d = |a: usize, b: usize| crate::store::distance(&v[a], &v[b]);
        println!("dog-dogs {:.3} dog-puppy {:.3} beach-seaside {:.3} dog-invoice {:.3} dog-beach {:.3} screenshot-screen capture {:.3}", d(0, 1), d(0, 2), d(3, 4), d(0, 5), d(0, 3), d(6, 7));
        if let Ok(file) = std::env::var("NEO_APOLLO_PICTURE") {
            let jpeg = {
                let p = image::open(&file).unwrap().thumbnail(768, 768).to_rgb8();
                let mut out = vec![];
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 82).encode_image(&p).unwrap();
                out
            };
            let began = std::time::Instant::now();
            let seen = m.describe(&jpeg).unwrap();
            println!("seen in {:?}: {seen:#?}", began.elapsed());
            let q = m.embed(&["a file manager window".into(), "a recipe for soup".into()], true).unwrap();
            let s = m.embed(std::slice::from_ref(&seen.text), false).unwrap();
            println!("near question {:.3}, far question {:.3}", crate::store::distance(&q[0], &s[0]), crate::store::distance(&q[1], &s[0]));
        }
        let began = std::time::Instant::now();
        let said = m.chat(&[Turn::new(Role::System, "Answer in one short sentence."), Turn::new(Role::User, "What is the capital of France?")], &mut |_| true).unwrap();
        println!("answered in {:?}: {said}", began.elapsed());
    }
}
