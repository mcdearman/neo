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
    /// The length of the embeddings it makes.
    fn dims(&self) -> Result<usize, String>;

    /// An embedding of each text. `question` says they are things asked,
    /// to be matched against things stored, which some models want to know.
    fn embed(&self, texts: &[String], question: bool) -> Result<Vec<Vec<f32>>, String>;

    /// Says what a picture, given as a JPEG, shows.
    fn describe(&self, jpeg: &[u8]) -> Result<Seen, String>;

    /// Answers a conversation. `piece` is given the answer as it comes;
    /// returning false from it stops the answer there.
    fn chat(&self, turns: &[Turn], piece: &mut dyn FnMut(&str) -> bool) -> Result<String, String>;
}

/// Models served by Ollama on this computer.
pub struct Ollama {
    server: String,
    chat_model: String,
    embed_model: String,
    dims: OnceLock<usize>,
}

/// How far a model's download has got.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pulling {
    pub status: String,
    pub done: u64,
    pub total: u64,
}

/// How long a model stays in memory after it was last asked something.
/// Short, so that a few gigabytes are given back soon after Apollo is done.
const KEEP: &str = "2m";

const DESCRIBE: &str =
    "Describe this image for someone who will search for it later. In `description`, say in two or three plain sentences what it shows: the people, animals, objects, place, activity, and any text that can be read. If it is a screenshot, say what app or page it shows and what is on it. In `tags`, give five to eight single words or short phrases for what it is of, most important first, lower case.";

fn agent(wait: Option<Duration>) -> ureq::Agent {
    ureq::Agent::config_builder().timeout_connect(Some(Duration::from_secs(3))).timeout_recv_response(wait).http_status_as_error(false).build().into()
}

impl Ollama {
    pub fn new(server: &str, chat_model: &str, embed_model: &str) -> Self {
        Self { server: server.trim_end_matches('/').to_owned(), chat_model: chat_model.to_owned(), embed_model: embed_model.to_owned(), dims: OnceLock::new() }
    }

    pub fn from_settings(s: &crate::settings::Settings) -> Self {
        Self::new(&s.server, &s.chat_model, &s.embed_model)
    }

    pub fn chat_model(&self) -> &str {
        &self.chat_model
    }

    pub fn embed_model(&self) -> &str {
        &self.embed_model
    }

    fn post(&self, path: &str, body: &Value, wait: Option<Duration>) -> Result<ureq::http::Response<ureq::Body>, String> {
        let res = agent(wait).post(&format!("{}{path}", self.server)).send_json(body).map_err(|e| format!("The model server did not answer: {e}"))?;
        if res.status().is_success() {
            return Ok(res);
        }
        let status = res.status();
        let mut res = res;
        let said = res.body_mut().read_json::<Value>().ok().and_then(|v| v["error"].as_str().map(str::to_owned));
        Err(said.unwrap_or_else(|| format!("The model server said {status}.")))
    }

    /// Whether the server is there.
    pub fn running(&self) -> bool {
        agent(Some(Duration::from_secs(2))).get(&format!("{}/api/version", self.server)).call().is_ok_and(|r| r.status().is_success())
    }

    /// Starts the server if it is not running, and waits for it to answer.
    pub fn start(&self) -> Result<(), String> {
        if self.running() {
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

    /// The models the server has, by name.
    pub fn models(&self) -> Result<Vec<String>, String> {
        let mut res = agent(Some(Duration::from_secs(5))).get(&format!("{}/api/tags", self.server)).call().map_err(|e| format!("The model server did not answer: {e}"))?;
        let v: Value = res.body_mut().read_json().map_err(|e| e.to_string())?;
        Ok(v["models"].as_array().map(|m| m.iter().filter_map(|m| m["name"].as_str().map(str::to_owned)).collect()).unwrap_or_default())
    }

    /// Those of the models Apollo uses that the server does not have yet.
    pub fn missing(&self) -> Result<Vec<String>, String> {
        let have = self.models()?;
        // "nomic-embed-text" is had as "nomic-embed-text:latest".
        let has = |want: &str| have.iter().any(|h| h == want || h.strip_suffix(":latest") == Some(want) || want.strip_suffix(":latest") == Some(h));
        Ok([&self.chat_model, &self.embed_model].into_iter().filter(|m| !has(m)).cloned().collect())
    }

    /// Downloads a model, telling `progress` how far it has got. Returning
    /// false from `progress` gives up; what has come down is kept for next time.
    pub fn pull(&self, model: &str, progress: &mut dyn FnMut(Pulling) -> bool) -> Result<(), String> {
        let mut res = self.post("/api/pull", &json!({ "model": model, "stream": true }), None)?;
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
        Ok(())
    }
}

/// Reads the model's answer about a picture, which was asked for as JSON.
pub fn read_seen(answer: &str) -> Seen {
    match serde_json::from_str::<Value>(answer.trim()) {
        Ok(v) => Seen { text: v["description"].as_str().unwrap_or_default().trim().to_owned(), words: crate::words::clean_all(v["tags"].as_array().into_iter().flatten().filter_map(Value::as_str), 8) },
        // Not what was asked for: what it said is still a description.
        Err(_) => Seen { text: answer.trim().to_owned(), words: vec![] },
    }
}

impl Model for Ollama {
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
        let mut res = self.post("/api/embed", &json!({ "model": self.embed_model, "input": input, "keep_alive": KEEP, "truncate": true }), Some(Duration::from_secs(300)))?;
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
        let body = json!({ "model": self.chat_model, "stream": false, "keep_alive": KEEP, "format": format, "options": { "temperature": 0.2 }, "messages": [{ "role": "user", "content": DESCRIBE, "images": [picture] }] });
        let mut res = self.post("/api/chat", &body, Some(Duration::from_secs(600)))?;
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
        let mut res = self.post("/api/chat", &json!({ "model": self.chat_model, "stream": true, "keep_alive": KEEP, "messages": messages }), Some(Duration::from_secs(600)))?;
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
            let asked = turns.iter().rev().find(|t| t.role == Role::User).map(|t| t.text.as_str()).unwrap_or_default();
            let knows = turns.iter().filter(|t| t.role == Role::System).map(|t| t.text.matches("\n[").count()).sum::<usize>();
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
        let m = Ollama::new("http://127.0.0.1:9/", "chat", "embed");
        assert!(!m.running());
        assert!(m.embed(&["x".into()], false).unwrap_err().contains("did not answer"));
        assert!(m.models().is_err() && m.missing().is_err() && m.dims().is_err());
        assert_eq!(m.embed(&[], false), Ok(vec![]), "nothing to embed asks nothing");
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
        let m = Ollama::from_settings(&crate::settings::Settings::default());
        m.start().unwrap();
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
