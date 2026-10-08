//! Answering a question from what is remembered.
//!
//! The question is embedded and the memories nearest to it in meaning are
//! put before the model with the question, so that it answers from what
//! is on this computer and says which file it means.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::model::{Model, Role, Turn};
use chrono::{DateTime, Datelike, Days, Local, Months, NaiveDate, TimeZone};

use crate::store::{Hit, Kind, Memory, New, Stats, Store};

/// How many memories are put before the model, and how near they must be.
const RECALLED: usize = 6;
/// Further than this from the question, a memory is about something else.
pub const NEAR: f32 = 0.48;
/// How many things are listed for a question about a kind or a time.
const LISTED: usize = 15;
/// How much of the conversation so far the model is shown.
const EARLIER: usize = 8;

const WHO: &str = "You are Apollo, the assistant built into the Neo desktop. You run on this computer and nothing said to you leaves it. Answer plainly and briefly, in ordinary sentences. You keep a memory of the user's pictures, videos and documents: you read the folders listed in the Sources page by yourself, describing each picture, looking at each video in a few places along its length, and reading each document. You cannot read a file on request in this conversation; a folder is added in Sources, and Read Now there starts a reading.";

/// An answer, and the memories it was given to draw on.
#[derive(Clone, Debug, PartialEq)]
pub struct Answer {
    pub text: String,
    pub recalled: Vec<Hit>,
    /// The things listed for a question about a kind or a time.
    pub listed: Vec<Memory>,
}

/// What a question asks for besides its meaning: things of a kind, or
/// from a stretch of time. "What videos do I have from this month?" asks
/// for videos, since the first of the month.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Wanted {
    pub kind: Option<Kind>,
    /// From when, and up to when, in seconds since 1970.
    pub since: Option<u64>,
    pub until: Option<u64>,
    /// The stretch of time in the question's own words: "this month".
    pub span: Option<String>,
    /// The newest are asked for, whenever they are from.
    pub newest: bool,
}

impl Wanted {
    /// Whether the question asks for a list of things at all.
    pub fn any(&self) -> bool {
        self.kind.is_some() || self.span.is_some()
    }

    /// What is asked for, in words: "videos from this month", "photos".
    pub fn what(&self) -> String {
        let things = self.kind.map_or("things", |k| match k {
            Kind::Photo => "photos",
            Kind::Video => "videos",
            Kind::Document => "documents",
            Kind::Conversation => "conversations",
            Kind::Note => "notes",
        });
        match &self.span {
            Some(span) => format!("{things} from {span}"),
            None => things.to_owned(),
        }
    }
}

const MONTHS: [&str; 12] = ["january", "february", "march", "april", "may", "june", "july", "august", "september", "october", "november", "december"];

fn midnight(day: NaiveDate) -> u64 {
    day.and_hms_opt(0, 0, 0).and_then(|t| Local.from_local_datetime(&t).earliest()).map_or(0, |t| t.timestamp().max(0) as u64)
}

/// Works out what a question asks for, at the time `now`.
pub fn wanted(question: &str, now: DateTime<Local>) -> Wanted {
    let lower = question.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let has = |any: &[&str]| words.iter().any(|w| any.contains(w));
    let phrase = |p: &str| lower.contains(p);
    let mut w = Wanted::default();
    w.kind = if has(&["video", "videos", "movie", "movies", "recording", "recordings", "clip", "clips", "footage"]) {
        Some(Kind::Video)
    } else if has(&["photo", "photos", "picture", "pictures", "image", "images", "screenshot", "screenshots", "pics"]) {
        Some(Kind::Photo)
    } else if has(&["document", "documents", "docs", "pdf", "pdfs", "papers"]) {
        Some(Kind::Document)
    } else {
        None
    };
    w.newest = has(&["latest", "newest", "recent", "recently", "last"]) && w.kind.is_some();
    let today = now.date_naive();
    let first = |d: NaiveDate| d.with_day(1).unwrap_or(d);
    let monday = today - Days::new(u64::from(today.weekday().num_days_from_monday()));
    let year = |y: i32| NaiveDate::from_ymd_opt(y, 1, 1).unwrap_or(today);
    let mut span = |name: &str, from: NaiveDate, to: Option<NaiveDate>| {
        (w.since, w.until, w.span) = (Some(midnight(from)), to.map(midnight), Some(name.to_owned()));
    };
    if phrase("today") {
        span("today", today, None);
    } else if phrase("yesterday") {
        span("yesterday", today - Days::new(1), Some(today));
    } else if phrase("this week") {
        span("this week", monday, None);
    } else if phrase("last week") {
        span("last week", monday - Days::new(7), Some(monday));
    } else if phrase("this month") {
        span("this month", first(today), None);
    } else if phrase("last month") {
        span("last month", first(today) - Months::new(1), Some(first(today)));
    } else if phrase("this year") {
        span("this year", year(today.year()), None);
    } else if phrase("last year") {
        span("last year", year(today.year() - 1), Some(year(today.year())));
    } else if let Some(m) = MONTHS.iter().position(|m| words.iter().any(|w| w == m) && (*m != "may" || phrase("in may") || phrase("from may"))) {
        // A month by name: of the year said, or else the last one there was.
        let said = words.iter().filter_map(|w| w.parse::<i32>().ok()).find(|y| (1990..=today.year()).contains(y));
        let y = said.unwrap_or(if (m as u32) < today.month() { today.year() } else { today.year() - 1 + i32::from(m as u32 + 1 == today.month()) });
        if let Some(from) = NaiveDate::from_ymd_opt(y, m as u32 + 1, 1) {
            let mut name = MONTHS[m].to_owned();
            name[..1].make_ascii_uppercase();
            span(&format!("{name} {y}"), from, Some(from + Months::new(1)));
        }
    } else if let Some(y) = words.iter().filter_map(|w| w.parse::<i32>().ok()).find(|y| (1990..=today.year()).contains(y)).filter(|_| phrase("from 19") || phrase("from 20") || phrase("in 19") || phrase("in 20")) {
        span(&y.to_string(), year(y), Some(year(y + 1)));
    }
    w
}

/// What Apollo knows of its own state while it answers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Aware {
    /// A reading of the folders is under way: files done, of how many.
    pub reading: Option<(usize, usize)>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The memories that bear on a question, nearest first.
pub fn recall(store: &Store, model: &dyn Model, question: &str) -> Result<Vec<Hit>, String> {
    recall_of(store, model, question, None)
}

fn recall_of(store: &Store, model: &dyn Model, question: &str, kind: Option<Kind>) -> Result<Vec<Hit>, String> {
    let asked = model.embed(&[question.to_owned()], true)?.pop().ok_or("no embedding")?;
    Ok(store.search(&asked, RECALLED, kind)?.into_iter().filter(|h| h.distance < NEAR).collect())
}

/// What is in the memory and how the reading stands, for the model to know.
fn overview(stats: &Stats, aware: &Aware) -> String {
    let counts: Vec<String> = Kind::ALL.iter().filter(|k| stats.of(**k) > 0).map(|k| format!("{} {}", stats.of(*k), k.plural().to_lowercase())).collect();
    let mut out = if counts.is_empty() { " Your memory is empty so far.".to_owned() } else { format!(" Your memory holds {} (a document counts once for each passage).", counts.join(", ")) };
    if let Some((done, total)) = aware.reading {
        out.push_str(&format!(" You are part-way through reading the user's folders, {done} of {total} files so far, documents first and then photos and videos from the newest back, so some files are not in your memory yet; say so if what is asked for may be among them."));
    }
    out
}

fn entry(n: usize, m: &Memory) -> String {
    let of = m.source.as_ref().map(|p| format!(" ({})", p.display())).unwrap_or_default();
    let when = neo_desktop::fs::full_time(UNIX_EPOCH + std::time::Duration::from_secs(m.created));
    format!("\n[{n}] {} · {}{of} · {when}\n{}\n", m.kind.name(), m.title, m.text)
}

/// What the model is told before the conversation: who it is, what its
/// memory holds, the list the question asked for if it asked for one, and
/// what else it remembers that may bear on the question. `memory` is
/// `None` while the memory is locked.
pub fn briefing(recalled: &[Hit], listing: Option<(&Wanted, &[Memory], u32)>, memory: Option<(&Stats, &Aware)>) -> String {
    let today = neo_desktop::fs::full_time(SystemTime::now());
    let mut out = format!("{WHO} It is {today}.");
    let Some((stats, aware)) = memory else {
        out.push_str(" Your memory of the user's files is locked just now, so you cannot look anything up in it; if asked about their files, say that it needs unlocking in the Memory page.");
        return out;
    };
    out.push_str(&overview(stats, aware));
    let mut n = 0;
    if let Some((wanted, listed, total)) = listing {
        let what = wanted.what();
        if listed.is_empty() {
            out.push_str(&format!(" The user asks about {what}: there are none in your memory. Say so plainly, and do not offer other files in their place."));
        } else {
            let all = if total as usize > listed.len() { format!("the newest {} of {total}", listed.len()) } else { format!("all {total}") };
            out.push_str(&format!(" The user asks about {what}. Here are {all} in your memory, newest first; answer from this list, naming the files and saying briefly what each shows.\n"));
            for m in listed {
                n += 1;
                out.push_str(&entry(n, m));
            }
        }
    }
    let more: Vec<&Hit> = recalled.iter().filter(|h| !listing.is_some_and(|(_, listed, _)| listed.iter().any(|m| m.source.is_some() && m.source == h.memory.source))).collect();
    if more.is_empty() {
        if listing.is_none() {
            out.push_str(" Nothing in your memory bears on this question. If it is about the user's files, say you have nothing on it rather than guessing.");
        }
        return out;
    }
    out.push_str(if listing.is_some() { " These may bear on the question too:\n" } else { " Below is what you remember that may bear on the question, each with the file it is of. Use what helps and name the file; if none of it answers the question, say so rather than guessing.\n" });
    for hit in more {
        n += 1;
        out.push_str(&entry(n, &hit.memory));
    }
    out
}

/// Answers `question`, which follows `earlier` in the conversation. With
/// a `store`, from memory; without, the model is told its memory is
/// locked. `piece` hears the answer as it comes.
pub fn ask(store: Option<&Store>, model: &dyn Model, earlier: &[Turn], question: &str, aware: &Aware, piece: &mut dyn FnMut(&str) -> bool) -> Result<Answer, String> {
    let (mut recalled, mut listed, mut total, mut stats) = (vec![], vec![], 0, None);
    let asks = wanted(question, Local::now());
    if let Some(store) = store {
        // A question about a kind of thing or a stretch of time is answered
        // from a list of just those; meaning alone would bring back whatever
        // reads most like the question.
        if asks.any() {
            (listed, total) = store.between(asks.kind, asks.since, asks.until, LISTED)?;
        }
        recalled = recall_of(store, model, question, asks.kind)?;
        stats = Some(store.stats()?);
    }
    let told = briefing(&recalled, asks.any().then_some((&asks, listed.as_slice(), total)), stats.as_ref().map(|s| (s, aware)));
    let mut turns = vec![Turn::new(Role::System, told)];
    let said: Vec<&Turn> = earlier.iter().filter(|t| t.role != Role::System).collect();
    turns.extend(said[said.len().saturating_sub(EARLIER)..].iter().map(|t| (*t).clone()));
    turns.push(Turn::new(Role::User, question));
    let text = model.chat(&turns, piece)?;
    Ok(Answer { text: text.trim().to_owned(), recalled, listed })
}

/// The files an answer names, of those it was given to draw on: each
/// once, in the order given. What it was shown and did not use is left out.
pub fn named<'a>(answer: &str, listed: &'a [Memory], recalled: &'a [Hit]) -> Vec<&'a Memory> {
    let lower = answer.to_lowercase();
    let mut out: Vec<&Memory> = vec![];
    for m in listed.iter().chain(recalled.iter().map(|h| &h.memory)) {
        let stem = std::path::Path::new(&m.title).file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
        if m.source.is_some() && stem.chars().count() >= 3 && lower.contains(&stem) && !out.iter().any(|have| have.source == m.source) {
            out.push(m);
        }
    }
    out
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
        let answer = ask(Some(&s), &model, &[], "Where is my dog?", &Aware::default(), &mut |p| {
            heard.push_str(p);
            true
        })
        .unwrap();
        assert_eq!(answer.recalled.iter().map(|h| h.memory.title.as_str()).collect::<Vec<_>>(), ["rex.jpg"], "the invoice is about something else");
        assert_eq!(answer.text, "You asked: Where is my dog? I looked at 1 memories.");
        assert_eq!(heard.trim(), answer.text, "heard as it came");
        let told = briefing(&answer.recalled, None, Some((&s.stats().unwrap(), &Aware::default())));
        assert!(told.contains("[1] photo · rex.jpg (/p/rex.jpg) · ") && told.contains("A dog, a retriever puppy.") && told.contains("Your memory holds 1 photos, 1 documents"), "{told}");
        // Stopped part-way, what was said so far is the answer.
        let cut = ask(Some(&s), &model, &[], "dog", &Aware::default(), &mut |_| false).unwrap();
        assert_eq!(cut.text, "You");
        // Locked, it is told so and shown nothing.
        let locked = ask(None, &model, &[], "Where is my dog?", &Aware::default(), &mut |_| true).unwrap();
        assert!(locked.recalled.is_empty() && locked.text.ends_with("0 memories."));
        assert!(briefing(&[], None, None).contains("locked") && briefing(&[], None, Some((&Stats::default(), &Aware::default()))).contains("Nothing in your memory bears"));
        // Only what an answer names is offered with it.
        assert_eq!(named("It is in rex.jpg.", &[], &answer.recalled).iter().map(|m| m.title.as_str()).collect::<Vec<_>>(), ["rex.jpg"]);
        assert!(named("I have nothing on that.", &[], &answer.recalled).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_question_about_a_kind_or_a_time_is_read_as_one() {
        // A Thursday.
        let now = Local.with_ymd_and_hms(2026, 10, 8, 11, 30, 0).unwrap();
        let day = |y, m, d| Some(midnight(NaiveDate::from_ymd_opt(y, m, d).unwrap()));
        let w = wanted("What videos do I have from this month?", now);
        assert_eq!((w.kind, w.since, w.until, w.span.as_deref(), w.what().as_str()), (Some(Kind::Video), day(2026, 10, 1), None, Some("this month"), "videos from this month"));
        let w = wanted("Show me screenshots from last week", now);
        assert_eq!((w.kind, w.since, w.until), (Some(Kind::Photo), day(2026, 9, 28), day(2026, 10, 5)));
        let w = wanted("anything from yesterday?", now);
        assert_eq!((w.kind, w.since, w.until, w.what().as_str()), (None, day(2026, 10, 7), day(2026, 10, 8), "things from yesterday"));
        assert_eq!(wanted("documents from last month", now).since, day(2026, 9, 1));
        assert_eq!((wanted("pictures from last year", now).since, wanted("pictures from last year", now).until), (day(2025, 1, 1), day(2026, 1, 1)));
        // A month by name is the last one there was, or the one of the year said.
        let w = wanted("photos from March", now);
        assert_eq!((w.since, w.until, w.span.as_deref()), (day(2026, 3, 1), day(2026, 4, 1), Some("March 2026")));
        assert_eq!(wanted("photos from December", now).span.as_deref(), Some("December 2025"));
        assert_eq!(wanted("videos in october", now).span.as_deref(), Some("October 2026"));
        assert_eq!(wanted("recordings from June 2024", now).since, day(2024, 6, 1));
        assert_eq!(wanted("photos from 2023", now).since, day(2023, 1, 1));
        let w = wanted("my latest videos", now);
        assert!(w.newest && w.since.is_none() && w.any());
        // An ordinary question asks for no list.
        for q in ["Where is the spare key?", "What may I do about it?", "How do I record the screen?"] {
            assert!(!wanted(q, now).any() || q.contains("record"), "{q}");
        }
        assert!(!wanted("What may I do about it?", now).any());
    }

    #[test]
    fn a_list_is_answered_from_what_is_of_that_kind_and_time() {
        let (mut s, dir) = store("list");
        let model = Fake::default();
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let mut add = |kind, file: &str, text: &str, created| crate::index::remember(&mut s, &model, &New { kind, source: Some(Path::new(file)), title: Path::new(file).file_name().unwrap().to_str().unwrap(), text, part: 0, created, words: &[] }).unwrap();
        add(Kind::Video, "/m/hike.mp4", "A video of a mountain hiking trail.", now - 60);
        add(Kind::Video, "/m/old.mov", "A video of a cat.", 1_000_000);
        add(Kind::Document, "/d/videos.md", "Notes about videos and films to watch this month.", now - 60);
        let answer = ask(Some(&s), &model, &[], "What videos do I have from this year?", &Aware { reading: Some((40, 900)) }, &mut |_| true).unwrap();
        assert_eq!(answer.listed.iter().map(|m| m.title.as_str()).collect::<Vec<_>>(), ["hike.mp4"], "the video from this year, not the old one, and not the document that talks of videos");
        assert!(answer.recalled.iter().all(|h| h.memory.kind == Kind::Video));
        let wants = wanted("What videos do I have from this year?", Local::now());
        let told = briefing(&answer.recalled, Some((&wants, &answer.listed, 1)), Some((&s.stats().unwrap(), &Aware { reading: Some((40, 900)) })));
        assert!(told.contains("The user asks about videos from this year. Here are all 1 in your memory") && told.contains("[1] video · hike.mp4 (/m/hike.mp4)"), "{told}");
        assert!(told.contains("40 of 900 files so far") && told.contains("Your memory holds 2 videos, 1 documents"), "{told}");
        assert_eq!(told.matches("hike.mp4 (").count(), 1, "not listed twice for being near in meaning as well");
        // None of that kind and time: said, and nothing offered in its place.
        let none = ask(Some(&s), &model, &[], "photos from yesterday", &Aware::default(), &mut |_| true).unwrap();
        assert!(none.listed.is_empty() && none.recalled.is_empty());
        assert!(briefing(&[], Some((&wanted("photos from yesterday", Local::now()), &[], 0)), Some((&s.stats().unwrap(), &Aware::default()))).contains("there are none in your memory"));
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
