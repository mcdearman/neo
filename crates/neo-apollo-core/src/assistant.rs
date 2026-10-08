//! Answering a question from what is remembered.
//!
//! The question is embedded and the memories nearest to it in meaning are
//! put before the model with the question, so that it answers from what
//! is on this computer and says which file it means.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::model::{Model, Role, Turn};
use crate::store::{Hit, Kind, New, Store};

/// How many memories are put before the model, and how near they must be.
const RECALLED: usize = 6;
/// Further than this from the question, a memory is about something else.
pub const NEAR: f32 = 0.48;
/// How much of the conversation so far the model is shown.
const EARLIER: usize = 8;

const WHO: &str = "You are Apollo, the assistant built into the Neo desktop. You run on this computer and nothing said to you leaves it. Answer plainly and briefly, in ordinary sentences.";

/// An answer, and the memories it was given to draw on.
#[derive(Clone, Debug, PartialEq)]
pub struct Answer {
    pub text: String,
    pub recalled: Vec<Hit>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The memories that bear on a question, nearest first.
pub fn recall(store: &Store, model: &dyn Model, question: &str) -> Result<Vec<Hit>, String> {
    let asked = model.embed(&[question.to_owned()], true)?.pop().ok_or("no embedding")?;
    Ok(store.search(&asked, RECALLED, None)?.into_iter().filter(|h| h.distance < NEAR).collect())
}

/// What the model is told before the conversation: who it is, and what it
/// remembers that may bear on the question.
pub fn briefing(recalled: &[Hit], memory_open: bool) -> String {
    let today = neo_desktop::fs::full_time(SystemTime::now());
    let mut out = format!("{WHO} It is {today}.");
    if !memory_open {
        out.push_str(" Your memory of the user's files is locked just now, so you cannot look anything up in it; if asked about their files, say that it needs unlocking in the Memory page.");
        return out;
    }
    if recalled.is_empty() {
        out.push_str(" Nothing in your memory of the user's files bears on this question. If it is about their files, say you have nothing on it rather than guessing.");
        return out;
    }
    out.push_str(" Below is what you remember that may bear on the question, each with the file it is of. Use what helps and name the file; if none of it answers the question, say so rather than guessing.\n");
    for (i, hit) in recalled.iter().enumerate() {
        let m = &hit.memory;
        let of = m.source.as_ref().map(|p| format!(" ({})", p.display())).unwrap_or_default();
        let when = neo_desktop::fs::full_time(UNIX_EPOCH + std::time::Duration::from_secs(m.created));
        out.push_str(&format!("\n[{}] {} · {}{of} · {when}\n{}\n", i + 1, m.kind.name(), m.title, m.text));
    }
    out
}

/// Answers `question`, which follows `earlier` in the conversation. With
/// a `store`, from memory; without, the model is told its memory is
/// locked. `piece` hears the answer as it comes.
pub fn ask(store: Option<&Store>, model: &dyn Model, earlier: &[Turn], question: &str, piece: &mut dyn FnMut(&str) -> bool) -> Result<Answer, String> {
    let recalled = match store {
        Some(store) => recall(store, model, question)?,
        None => vec![],
    };
    let mut turns = vec![Turn::new(Role::System, briefing(&recalled, store.is_some()))];
    let said: Vec<&Turn> = earlier.iter().filter(|t| t.role != Role::System).collect();
    turns.extend(said[said.len().saturating_sub(EARLIER)..].iter().map(|t| (*t).clone()));
    turns.push(Turn::new(Role::User, question));
    let text = model.chat(&turns, piece)?;
    Ok(Answer { text: text.trim().to_owned(), recalled })
}

/// What Apollo was told to keep in mind, if the message is that: "remember
/// that the spare key is under the mat" gives "the spare key is under the mat".
pub fn told_to_remember(message: &str) -> Option<&str> {
    let trimmed = message.trim();
    let lower = trimmed.to_lowercase();
    let rest = ["remember that ", "remember: ", "remember, ", "note that ", "make a note that ", "make a note: "].iter().find(|p| lower.starts_with(**p)).map(|p| trimmed[p.len()..].trim())?;
    (!rest.is_empty()).then_some(rest)
}

fn headline(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    if line.chars().count() <= 80 { line.to_owned() } else { format!("{}…", line.chars().take(79).collect::<String>().trim_end()) }
}

/// Keeps something Apollo was told.
pub fn keep_note(store: &mut Store, model: &dyn Model, note: &str) -> Result<i64, String> {
    let words = crate::words::keywords(note, 5);
    crate::index::remember(store, model, &New { kind: Kind::Note, source: None, title: &headline(note), text: note, part: 0, created: now(), words: &words })
}

/// Keeps a question and its answer, so that it can be brought up later.
pub fn keep_exchange(store: &mut Store, model: &dyn Model, question: &str, answer: &str) -> Result<i64, String> {
    let text = format!("Asked: {}\nAnswered: {}", question.trim(), answer.trim());
    let words = crate::words::keywords(question, 4);
    crate::index::remember(store, model, &New { kind: Kind::Conversation, source: None, title: &headline(question), text: &text, part: 0, created: now(), words: &words })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::fake::Fake;
    use std::path::Path;

    fn store(name: &str) -> (Store, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("neo-apollo-ask-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        (Store::open(&dir.join("memory.db"), &[3; 32], Fake::default().dims().unwrap()).unwrap(), dir)
    }

    #[test]
    fn a_question_is_answered_from_the_memories_that_bear_on_it() {
        let (mut s, dir) = store("ask");
        let model = Fake::default();
        let dog = ["dog".to_owned()];
        crate::index::remember(&mut s, &model, &New { kind: Kind::Photo, source: Some(Path::new("/p/rex.jpg")), title: "rex.jpg", text: "A dog, a retriever puppy.", part: 0, created: 1_790_000_000, words: &dog }).unwrap();
        crate::index::remember(&mut s, &model, &New { kind: Kind::Document, source: Some(Path::new("/d/bill.md")), title: "bill.md", text: "The invoice and the payment to the supplier.", part: 0, created: 1_790_000_000, words: &[] }).unwrap();
        let mut heard = String::new();
        let answer = ask(Some(&s), &model, &[], "Where is the photo of my dog?", &mut |p| {
            heard.push_str(p);
            true
        })
        .unwrap();
        assert_eq!(answer.recalled.iter().map(|h| h.memory.title.as_str()).collect::<Vec<_>>(), ["rex.jpg"], "the invoice is about something else");
        assert_eq!(answer.text, "You asked: Where is the photo of my dog? I looked at 1 memories.");
        assert_eq!(heard.trim(), answer.text, "heard as it came");
        let told = briefing(&answer.recalled, true);
        assert!(told.contains("[1] photo · rex.jpg (/p/rex.jpg) · ") && told.contains("A dog, a retriever puppy."), "{told}");
        // Stopped part-way, what was said so far is the answer.
        let cut = ask(Some(&s), &model, &[], "dog", &mut |_| false).unwrap();
        assert_eq!(cut.text, "You");
        // Locked, it is told so and shown nothing.
        let locked = ask(None, &model, &[], "Where is the photo of my dog?", &mut |_| true).unwrap();
        assert!(locked.recalled.is_empty() && locked.text.ends_with("0 memories."));
        assert!(briefing(&[], false).contains("locked") && briefing(&[], true).contains("Nothing in your memory"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn what_it_is_told_and_asked_is_kept() {
        assert_eq!(told_to_remember("Remember that the spare key is under the mat "), Some("the spare key is under the mat"));
        assert_eq!(told_to_remember("note that Sam's birthday is in May"), Some("Sam's birthday is in May"));
        assert_eq!((told_to_remember("Do you remember that film?"), told_to_remember("remember that "), told_to_remember("remember when")), (None, None, None));
        let (mut s, dir) = store("keep");
        let model = Fake::default();
        keep_note(&mut s, &model, "My dog Rex is a retriever").unwrap();
        keep_exchange(&mut s, &model, "How much was the invoice from the supplier?", "It was 40 pounds.").unwrap();
        let recalled = recall(&s, &model, "what breed is my dog").unwrap();
        assert_eq!((recalled[0].memory.kind, recalled[0].memory.text.as_str()), (Kind::Note, "My dog Rex is a retriever"));
        let talk = &s.recent(1, Some(Kind::Conversation)).unwrap()[0];
        assert_eq!((talk.title.as_str(), talk.text.as_str()), ("How much was the invoice from the supplier?", "Asked: How much was the invoice from the supplier?\nAnswered: It was 40 pounds."));
        assert_eq!(headline(&"long ".repeat(40)).chars().count(), 80);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
