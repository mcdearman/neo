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
/// What is nearer is shown to the model, and shown to the user beneath
/// the answer, so it is set where what does bear on a question mostly
/// falls inside and what merely reads a little like it mostly falls out.
pub const NEAR: f32 = 0.42;
/// How many things are listed for a question about a kind or a time.
const LISTED: usize = 15;
/// How much of the conversation so far the model is shown.
const EARLIER: usize = 8;

const WHO: &str = "You are Apollo, the assistant built into the Neo desktop, running on this computer. Answer plainly and briefly, in full sentences. You keep a memory of the user's pictures, videos and documents, made by reading the folders chosen in the Sources page; you cannot read a file on request in this conversation. Any files you draw on are shown to the user as pictures under your answer, so do not write the names of files or the numbers of entries. Say which folder something is in when asked where it is.";

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
    /// The question also says what the things should be of: "photos of a
    /// girl with pink hair" does, "what photos do I have" does not.
    pub about: bool,
}

impl Wanted {
    /// Whether the question asks for a list of things: everything from a
    /// stretch of time, the newest of a kind, or all of a kind. One that
    /// says what the things should be of is a search among that kind
    /// instead, not a list of every one of them.
    pub fn any(&self) -> bool {
        self.span.is_some() || (self.kind.is_some() && (self.newest || !self.about))
    }

    /// What is asked for, in words: "videos from this month", "photos".
    pub fn what(&self) -> String {
        let things = self.kind.map_or("things", |k| match k {
            Kind::Photo => "photos",
            Kind::Video => "videos",
            Kind::Document => "documents",
            Kind::Conversation => "conversations",
            Kind::Note => "notes",
            Kind::Folder => "folders",
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
    let kind = if has(&["video", "videos", "movie", "movies", "recording", "recordings", "clip", "clips", "footage"]) {
        Some(Kind::Video)
    } else if has(&["photo", "photos", "picture", "pictures", "image", "images", "screenshot", "screenshots", "pics"]) {
        Some(Kind::Photo)
    } else if has(&["document", "documents", "docs", "pdf", "pdfs", "papers"]) {
        Some(Kind::Document)
    } else if has(&["folder", "folders", "directory", "directories", "vault", "vaults", "repo", "repos", "repository", "repositories", "project", "projects"]) {
        Some(Kind::Folder)
    } else {
        None
    };
    let mut w = Wanted { kind, ..Wanted::default() };
    // What is left when the words for kinds, times and asking are taken out.
    const ASKING: &[&str] = &[
        "video",
        "videos",
        "movie",
        "movies",
        "recording",
        "recordings",
        "clip",
        "clips",
        "footage",
        "photo",
        "photos",
        "picture",
        "pictures",
        "image",
        "images",
        "screenshot",
        "screenshots",
        "pics",
        "document",
        "documents",
        "docs",
        "pdf",
        "pdfs",
        "papers",
        "folder",
        "folders",
        "directory",
        "directories",
        "file",
        "files",
        "show",
        "shows",
        "showing",
        "list",
        "find",
        "tell",
        "give",
        "look",
        "count",
        "number",
        "got",
        "mine",
        "please",
        "apollo",
        "taken",
        "saved",
        "today",
        "yesterday",
        "week",
        "month",
        "year",
        "latest",
        "newest",
        "recent",
        "recently",
        "last",
        "ago",
        "since",
        "ones",
        "anything",
        "everything",
        "something",
        "things",
        "stuff",
    ];
    w.about = crate::words::terms(question).iter().any(|t| !ASKING.contains(&t.as_str()) && !MONTHS.contains(&t.as_str()) && t.parse::<u32>().is_err());
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
    /// How many photos and videos that are only listed may be looked at
    /// to answer one question.
    pub look: usize,
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
    // One that says the question's own words is let in from a little further off.
    let terms = crate::words::terms(question);
    let said = |h: &Hit| {
        let text = format!("{} {}", h.memory.title, h.memory.text).to_lowercase();
        terms.iter().filter(|t| text.contains(t.as_str())).count()
    };
    // What was asked before reads more like this question than anything
    // that answers it, and says only what was known then: it is left out
    // unless the question is about what was said.
    let about_talk = ["ask", "asked", "said", "told", "talked", "conversation", "conversations", "earlier", "before"].iter().any(|w| terms.iter().any(|t| t == w)) || question.to_lowercase().contains("last time");
    let mut found: Vec<Hit> = store.search_for(&asked, &terms, RECALLED + 4, kind)?.into_iter().filter(|h| about_talk || kind.is_some() || h.memory.kind != Kind::Conversation).take(RECALLED).filter(|h| h.distance < NEAR || (!terms.is_empty() && said(h) == terms.len() && h.distance < NEAR + crate::store::SAID)).collect();
    // With something found that says every word asked for, what says none
    // of them is only near by the shape of its words, as one folder's
    // description is near any other's: it is left out.
    if !terms.is_empty() && found.iter().any(|h| said(h) == terms.len()) {
        found.retain(|h| said(h) > 0);
    }
    Ok(found)
}

/// What is in the memory and how the reading stands, for the model to know.
fn overview(stats: &Stats, aware: &Aware) -> String {
    let counts: Vec<String> = Kind::ALL.iter().filter(|k| stats.of(**k) > 0).map(|k| format!("{} {}", stats.of(*k), k.plural().to_lowercase())).collect();
    let mut out = if counts.is_empty() { " Your memory is empty so far.".to_owned() } else { format!(" Your memory holds {} (a document counts once for each passage).", counts.join(", ")) };
    if stats.light > 0 {
        out.push_str(&format!(" {} of the photos and videos are only listed by name so far and have not been looked at; you know what those show only once they are.", stats.light));
    }
    if let Some((done, total)) = aware.reading {
        out.push_str(&format!(" You are part-way through reading the user's folders, {done} of {total} files so far, documents first and then photos and videos from the newest back, so some files are not in your memory yet; say so if what is asked for may be among them."));
    }
    out
}

/// A memory as the model is shown it: numbered, with what it is and when
/// it is from. A photo or a video goes without its file's name, which
/// says little and which the model is not to repeat; a document's name
/// is what it is known by.
fn entry(n: usize, m: &Memory) -> String {
    let when = neo_desktop::fs::full_time(UNIX_EPOCH + std::time::Duration::from_secs(m.created));
    let what = match m.kind {
        Kind::Photo => "A photo".to_owned(),
        Kind::Video => "A video".to_owned(),
        Kind::Document => format!("A document called \"{}\"", m.title),
        Kind::Conversation => "An earlier conversation".to_owned(),
        Kind::Note => "A note the user asked you to keep".to_owned(),
        // A folder says what and where it is itself.
        Kind::Folder => return format!("{n}. {}\n", m.text),
    };
    // Where it is, for when that is what is asked: the folder, not the file.
    let place = m.source.as_deref().and_then(std::path::Path::parent).map(|dir| match dir.strip_prefix(neo_desktop::fs::home_dir()) {
        Ok(rest) => format!(", in the folder ~/{}", rest.display()),
        Err(_) => format!(", in the folder {}", dir.display()),
    });
    format!("{n}. {what}{}, from {when}: {}\n", place.unwrap_or_default(), m.text)
}

/// What the model is told before the conversation: who it is, and how
/// its memory stands. `memory` is `None` while the memory is locked.
pub fn briefing(memory: Option<(&Stats, &Aware)>) -> String {
    let today = neo_desktop::fs::full_time(SystemTime::now());
    let mut out = format!("{WHO} It is {today}.");
    match memory {
        Some((stats, aware)) => out.push_str(&overview(stats, aware)),
        None => out.push_str(" Your memory of the user's files is locked just now, so you cannot look anything up in it; if asked about their files, say that it needs unlocking in the Memory page."),
    }
    out
}

/// The question as it is put to the model: with what the memory holds
/// that bears on it set out first, and what to make of that after.
///
/// It is put this way, in the question's own turn and in few words,
/// because the model that answers is a small one: it is held in memory
/// only briefly and is back in a second or two, which is what lets Apollo
/// wait to be asked while holding next to nothing. The finding is done by
/// the index, not by the model; the model only has to say what was found.
pub fn put(question: &str, recalled: &[Hit], listing: Option<(&Wanted, &[Memory], u32)>) -> String {
    let more: Vec<&Memory> = recalled.iter().map(|h| &h.memory).filter(|m| !listing.is_some_and(|(_, listed, _)| listed.iter().any(|l| l.source.is_some() && l.source == m.source))).collect();
    let mut out = String::new();
    let mut n = 0;
    match listing {
        Some((wanted, [], _)) => return format!("{question}\n\n(There are no {} in your memory. Say so in one sentence, and do not offer other files in their place.)", wanted.what()),
        Some((wanted, listed, total)) => {
            let how_many = if total as usize > listed.len() {
                format!("There are {total}; the newest {} are below.", listed.len())
            } else if total == 1 {
                "There is 1, below.".to_owned()
            } else {
                format!("There are {total}, below.")
            };
            out.push_str(&format!("The user asks about {}. {how_many}\n\n", wanted.what()));
            for m in listed {
                n += 1;
                out.push_str(&entry(n, m));
            }
        }
        None if more.is_empty() => return question.to_owned(),
        None => out.push_str("From your memory of the user's files:\n\n"),
    }
    for m in more {
        n += 1;
        out.push_str(&entry(n, m));
    }
    out.push_str(&format!("\nQuestion: {question}\n\nAnswer in one to three sentences, using only what is above. Say what was found and what it shows, in your own words; do not copy the entries out, and do not give their numbers or dates unless asked. If what is above does not answer the question, say that you have nothing on it."));
    out
}

/// Answers `question`, which follows `earlier` in the conversation. With
/// a `store`, from memory; without, the model is told its memory is
/// locked. `piece` hears the answer as it comes.
pub fn ask(mut store: Option<&mut Store>, model: &dyn Model, earlier: &[Turn], question: &str, aware: &Aware, looking: &mut dyn FnMut(usize, usize) -> bool, piece: &mut dyn FnMut(&str) -> bool) -> Result<Answer, String> {
    let asks = wanted(question, Local::now());
    // A question about a kind of thing or a stretch of time is answered
    // from a list of just those; meaning alone would bring back whatever
    // reads most like the question.
    let find = |store: &Store| -> Result<(Vec<Hit>, Vec<Memory>, u32), String> {
        let (listed, total) = if asks.any() { store.between(asks.kind, asks.since, asks.until, LISTED)? } else { (vec![], 0) };
        Ok((recall_of(store, model, question, asks.kind)?, listed, total))
    };
    let (mut recalled, mut listed, mut total, mut stats) = (vec![], vec![], 0, None);
    if let Some(store) = store.as_deref_mut() {
        (recalled, listed, total) = find(store)?;
        // What it turned up that is only listed by name is looked at now,
        // so that the answer can say what is in it; then it is found again,
        // by what it shows this time.
        let waiting: Vec<std::path::PathBuf> = sources(&listed, &recalled).into_iter().filter(|m| m.light).filter_map(|m| m.source.clone()).take(aware.look).collect();
        if !waiting.is_empty() {
            for (i, file) in waiting.iter().enumerate() {
                if !looking(i, waiting.len()) {
                    break;
                }
                // One that cannot be looked at stays as it was listed.
                let _ = crate::index::look_at_files(store, model, vec![file.clone()], &mut |_| true);
            }
            model.rest();
            (recalled, listed, total) = find(store)?;
        }
        stats = Some(store.stats()?);
    }
    let mut turns = vec![Turn::new(Role::System, briefing(stats.as_ref().map(|s| (s, aware))))];
    let said: Vec<&Turn> = earlier.iter().filter(|t| t.role != Role::System).collect();
    turns.extend(said[said.len().saturating_sub(EARLIER)..].iter().map(|t| (*t).clone()));
    turns.push(Turn::new(Role::User, put(question, &recalled, (store.is_some() && asks.any()).then_some((&asks, listed.as_slice(), total)))));
    let text = model.chat(&turns, piece)?;
    // A small model now and then gives back a stray mark and nothing else.
    // What was found is still found: it is said plainly in the model's place.
    let text = if says_something(&text) { text.trim().to_owned() } else { said_plainly(&listed, &recalled) };
    Ok(Answer { text, recalled, listed })
}

/// Whether an answer has a word in it, and is not only marks or nothing.
/// One word is an answer: "Paris", or one that was stopped as it began.
fn says_something(answer: &str) -> bool {
    answer.split_whitespace().any(|w| w.chars().filter(|c| c.is_alphabetic()).count() >= 2)
}

/// What was found, in its own words, for when the model gave no answer.
fn said_plainly(listed: &[Memory], recalled: &[Hit]) -> String {
    let found: Vec<&Memory> = listed.iter().chain(recalled.iter().map(|h| &h.memory)).take(3).collect();
    match found.as_slice() {
        [] => "I could not put an answer together that time. Ask again.".to_owned(),
        [one] => format!("Here is what I found. {}", one.text.lines().next().unwrap_or_default()),
        several => format!("Here is what I found. {}", several.iter().map(|m| m.text.lines().next().unwrap_or_default()).collect::<Vec<_>>().join(" ")),
    }
}

/// The numbers an answer marks its sources with: "[2]", "[1, 3]".
fn marks(answer: &str) -> Vec<(std::ops::Range<usize>, Vec<usize>)> {
    let mut out = vec![];
    let mut from = 0;
    while let Some(open) = answer[from..].find('[').map(|i| i + from) {
        let Some(close) = answer[open..].find(']').map(|i| i + open) else { break };
        let inside = &answer[open + 1..close];
        let numbers: Vec<usize> = inside.split(',').filter_map(|n| n.trim().parse().ok()).collect();
        if !numbers.is_empty() && inside.chars().all(|c| c.is_ascii_digit() || c == ',' || c == ' ') {
            out.push((open..close + 1, numbers));
        }
        from = close + 1;
    }
    out
}

/// An answer as it is shown: without the numbers it marks its sources
/// with, which are for [`cited`] and mean nothing to the reader.
pub fn plain(answer: &str) -> String {
    let mut out = String::with_capacity(answer.len());
    let mut from = 0;
    for (range, _) in marks(answer) {
        // The space before a mark goes with it, so none is left before a full stop.
        out.push_str(answer[from..range.start].trim_end_matches(' '));
        from = range.end;
    }
    out.push_str(&answer[from..]);
    out.trim().to_owned()
}

/// The files an answer was given to draw on, to show beneath it: those
/// listed for a question about a kind or a time, then what else bore on
/// it, each once. Which they are is the index's finding, not the model's
/// say-so, so they are right whatever the model makes of them.
pub fn sources<'a>(listed: &'a [Memory], recalled: &'a [Hit]) -> Vec<&'a Memory> {
    let mut out: Vec<&Memory> = vec![];
    for m in listed.iter().chain(recalled.iter().map(|h| &h.memory)) {
        if m.source.is_some() && !out.iter().any(|have| have.source == m.source) {
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
        let answer = ask(Some(&mut s), &model, &[], "Where is my dog?", &Aware::default(), &mut |_, _| true, &mut |p| {
            heard.push_str(p);
            true
        })
        .unwrap();
        assert_eq!(answer.recalled.iter().map(|h| h.memory.title.as_str()).collect::<Vec<_>>(), ["rex.jpg"], "the invoice is about something else");
        assert_eq!(answer.text, "You asked: Where is my dog? I looked at 1 memories.");
        assert_eq!(sources(&answer.listed, &answer.recalled).iter().map(|m| m.title.as_str()).collect::<Vec<_>>(), ["rex.jpg"], "and that is what is shown beneath the answer");
        assert_eq!(heard.trim(), answer.text, "heard as it came");
        let told = put("Where is my dog?", &answer.recalled, None);
        assert!(told.starts_with("From your memory of the user's files:\n\n1. A photo, in the folder /p, from ") && !told.contains("rex.jpg") && told.contains(": A dog, a retriever puppy.\n\nQuestion: Where is my dog?\n\nAnswer in"), "{told}");
        assert!(briefing(Some((&s.stats().unwrap(), &Aware::default()))).contains("Your memory holds 1 photos, 1 documents"));
        // A document goes by its name, and a question nothing bears on is put as it was asked.
        let bill = recall(&s, &model, "the invoice payment").unwrap();
        assert!(put("How much?", &bill, None).contains("1. A document called \"bill.md\", in the folder /d, from "));
        assert_eq!(put("What is the capital of France?", &[], None), "What is the capital of France?");
        // Stopped part-way, what was said so far is the answer.
        let cut = ask(Some(&mut s), &model, &[], "dog", &Aware::default(), &mut |_, _| true, &mut |_| false).unwrap();
        assert_eq!(cut.text, "You");
        // Locked, it is told so and shown nothing.
        let locked = ask(None, &model, &[], "Where is my dog?", &Aware::default(), &mut |_, _| true, &mut |_| true).unwrap();
        assert!(locked.recalled.is_empty() && locked.text.ends_with("0 memories."));
        assert!(briefing(None).contains("locked") && briefing(Some((&Stats::default(), &Aware::default()))).contains("Your memory is empty so far"));
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
        // Saying what they should be of makes it a search among that kind, not a list of them all.
        let w = wanted("Which pictures show a girl with pink hair?", now);
        assert!(w.kind == Some(Kind::Photo) && w.about && !w.any());
        assert!(!wanted("What photos do I have?", now).about && wanted("What photos do I have?", now).any());
        assert!(wanted("photos of the dog from last month", now).any(), "a stretch of time is still a list");
        assert!(!wanted("show me my videos from March 2026 please", now).about);
        // An ordinary question asks for no list.
        for q in ["Where is the spare key?", "What may I do about it?", "How do I record the screen?"] {
            assert!(!wanted(q, now).any() || q.contains("record"), "{q}");
        }
        assert!(!wanted("What may I do about it?", now).any());
    }

    #[test]
    fn a_model_that_answers_with_nothing_is_stood_in_for() {
        assert!(says_something("It is in Documents.") && says_something("Paris, France"));
        assert!(says_something("Ok") && says_something("You"));
        assert!(!says_something("```") && !says_something("") && !says_something(" . \n ") && !says_something("[1]") && !says_something("a"));
        let m = |text: &str| Memory { id: 1, kind: Kind::Folder, source: Some("/d/v".into()), title: "v".into(), text: text.into(), part: 0, created: 0, light: false };
        assert_eq!(said_plainly(&[], &[Hit { memory: m("A folder that is an Obsidian vault, called v, in ~/Documents."), distance: 0.3 }]), "Here is what I found. A folder that is an Obsidian vault, called v, in ~/Documents.");
        assert_eq!(said_plainly(&[m("One."), m("Two.\nMore.")], &[]), "Here is what I found. One. Two.");
        assert_eq!(said_plainly(&[], &[]), "I could not put an answer together that time. Ask again.");
    }

    #[test]
    fn where_a_thing_is_can_be_asked() {
        let now = Local.with_ymd_and_hms(2026, 10, 8, 11, 30, 0).unwrap();
        let w = wanted("where is my obsidian vault", now);
        assert_eq!((w.kind, w.about, w.any()), (Some(Kind::Folder), true, false), "a search among the folders, for the one that is that");
        assert!(wanted("what folders do I have?", now).any() && wanted("list my projects", now).kind == Some(Kind::Folder));
        let (mut s, dir) = store("where");
        let model = Fake::default();
        let about = ["obsidian".to_owned(), "vault".to_owned()];
        let home = neo_desktop::fs::home_dir();
        crate::index::remember(&mut s, &model, &New { kind: Kind::Folder, source: Some(&home.join("Documents/vaultobs")), title: "vaultobs", text: "A folder that is an Obsidian vault, called vaultobs, in ~/Documents. It holds 40 things.", part: 0, created: 1_790_000_000, words: &about }).unwrap();
        crate::index::remember(&mut s, &model, &New { kind: Kind::Folder, source: Some(&home.join("Documents/Invoices")), title: "Invoices", text: "A folder called Invoices, in ~/Documents. It holds 3 things.", part: 0, created: 1_790_000_000, words: &[] }).unwrap();
        crate::index::remember(&mut s, &model, &New { kind: Kind::Document, source: Some(&home.join("Documents/vaultobs/Daily/today.md")), title: "today.md", text: "The invoice from the supplier.", part: 0, created: 1_790_000_000, words: &[] }).unwrap();
        let answer = ask(Some(&mut s), &model, &[], "where is my obsidian vault", &Aware::default(), &mut |_, _| true, &mut |_| true).unwrap();
        assert_eq!(answer.recalled.iter().map(|h| h.memory.title.as_str()).collect::<Vec<_>>(), ["vaultobs"], "the folder that is one, and not the other, nor a file in it");
        let told = put("where is my obsidian vault", &answer.recalled, None);
        assert!(told.contains("1. A folder that is an Obsidian vault, called vaultobs, in ~/Documents. It holds 40 things.\n"), "{told}");
        // And a file says which folder it is in.
        let bill = recall(&s, &model, "the invoice from the supplier").unwrap();
        assert!(put("where is the invoice?", &bill, None).contains("A document called \"today.md\", in the folder ~/Documents/vaultobs/Daily, from "));
        std::fs::remove_dir_all(dir).unwrap();
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
        let answer = ask(Some(&mut s), &model, &[], "What videos do I have from this year?", &Aware { reading: Some((40, 900)), look: 0 }, &mut |_, _| true, &mut |_| true).unwrap();
        assert_eq!(answer.listed.iter().map(|m| m.title.as_str()).collect::<Vec<_>>(), ["hike.mp4"], "the video from this year, not the old one, and not the document that talks of videos");
        assert!(answer.recalled.iter().all(|h| h.memory.kind == Kind::Video));
        let wants = wanted("What videos do I have from this year?", Local::now());
        let told = put("What videos do I have from this year?", &answer.recalled, Some((&wants, &answer.listed, 1)));
        assert!(told.starts_with("The user asks about videos from this year. There is 1, below.\n\n1. A video, in the folder /m, from ") && !told.contains("hike.mp4"), "{told}");
        assert_eq!(told.matches("A video of a mountain hiking trail.").count(), 1, "not listed twice for being near in meaning as well");
        assert!(put("q", &[], Some((&wants, &answer.listed, 40))).contains("There are 40; the newest 1 are below."));
        let knows = briefing(Some((&s.stats().unwrap(), &Aware { reading: Some((40, 900)), look: 0 })));
        assert!(knows.contains("40 of 900 files so far") && knows.contains("Your memory holds 2 videos, 1 documents"), "{knows}");
        assert_eq!(sources(&answer.listed, &answer.recalled).len(), 1);
        // None of that kind and time: said, and nothing offered in its place.
        let none = ask(Some(&mut s), &model, &[], "photos from yesterday", &Aware::default(), &mut |_, _| true, &mut |_| true).unwrap();
        assert!(none.listed.is_empty() && none.recalled.is_empty());
        assert_eq!(put("photos from yesterday", &[], Some((&wanted("photos from yesterday", Local::now()), &[], 0))), "photos from yesterday\n\n(There are no photos from yesterday in your memory. Say so in one sentence, and do not offer other files in their place.)");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_answer_is_shown_without_marks_and_with_the_files_it_was_given() {
        assert_eq!(plain("A dog on a beach [1], and a hike [2, 3]. Nothing else [x] costs 3 [pounds]."), "A dog on a beach, and a hike. Nothing else [x] costs 3 [pounds].");
        assert_eq!(plain("[1] A dog.  "), "A dog.");
        assert_eq!((plain("No marks here."), plain("An open [ bracket")), ("No marks here.".to_owned(), "An open [ bracket".to_owned()));
        let m = |id, file: &str| Memory { id, kind: Kind::Video, source: Some(file.into()), title: Path::new(file).file_name().unwrap().to_string_lossy().into_owned(), text: String::new(), part: 0, created: 0, light: false };
        let listed = [m(1, "/v/a.mp4"), m(2, "/v/b.mp4")];
        let note = Memory { source: None, ..m(4, "/none") };
        let recalled = [Hit { memory: m(3, "/v/c.mp4"), distance: 0.3 }, Hit { memory: m(1, "/v/a.mp4"), distance: 0.35 }, Hit { memory: note, distance: 0.36 }];
        assert_eq!(sources(&listed, &recalled).iter().map(|m| m.title.as_str()).collect::<Vec<_>>(), ["a.mp4", "b.mp4", "c.mp4"], "the list, then what else bore on it, each file once, and nothing that is of no file");
        assert!(sources(&[], &[]).is_empty());
    }

    #[test]
    fn what_a_question_turns_up_only_listed_is_looked_at_before_it_is_answered() {
        let (mut s, dir) = store("look");
        let model = Fake::default();
        let folder = dir.join("Trips");
        std::fs::create_dir_all(&folder).unwrap();
        // Red is a dog on a beach to the test model, green a mountain, blue the sea.
        for (file, colour) in [("a.png", [220u8, 40, 40]), ("b.png", [40, 220, 40]), ("c.png", [40, 40, 220])] {
            image::RgbImage::from_fn(240, 160, |x, y| if x < 8 && y < 8 { image::Rgb(colour) } else { image::Rgb([colour[0] ^ ((x * 7 + y * 13) % 31) as u8, colour[1] ^ ((x * 3 + y * 5) % 29) as u8, colour[2] ^ ((x + y * 11) % 23) as u8]) }).save(folder.join(file)).unwrap();
        }
        crate::index::run(&mut s, &model, std::slice::from_ref(&folder), false, &mut |_| true);
        assert_eq!(s.stats().unwrap().light, 3, "listed, none looked at");
        // Asked for the photos, two may be looked at: they are, the newest first, and the answer is of what they show.
        let mut steps = vec![];
        let answer = ask(
            Some(&mut s),
            &model,
            &[],
            "What photos do I have?",
            &Aware { reading: None, look: 2 },
            &mut |i, n| {
                steps.push((i, n));
                true
            },
            &mut |_| true,
        )
        .unwrap();
        assert_eq!(steps, [(0, 2), (1, 2)]);
        assert_eq!((answer.listed.len(), answer.listed.iter().filter(|m| m.light).count(), s.stats().unwrap().light), (3, 1, 1));
        assert!(answer.listed.iter().any(|m| !m.light && (m.text == "A dog running on a beach." || m.text == "A snowy mountain peak." || m.text == "Waves on the sea.")));
        assert!(briefing(Some((&s.stats().unwrap(), &Aware::default()))).contains("1 of the photos and videos are only listed by name so far"));
        // With none allowed, or the looking stopped, it answers from what it has.
        let before = s.stats().unwrap().light;
        ask(Some(&mut s), &model, &[], "What photos do I have?", &Aware::default(), &mut |_, _| panic!("none may be looked at"), &mut |_| true).unwrap();
        ask(Some(&mut s), &model, &[], "What photos do I have?", &Aware { reading: None, look: 5 }, &mut |_, _| false, &mut |_| true).unwrap();
        assert_eq!(s.stats().unwrap().light, before);
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
        // What was asked before is not what answers a question, unless the question is about that.
        keep_exchange(&mut s, &model, "what breed is my dog", "I have nothing on that.").unwrap();
        assert!(recall(&s, &model, "what breed is my dog").unwrap().iter().all(|h| h.memory.kind != Kind::Conversation));
        assert!(recall(&s, &model, "what did I ask about my dog last time?").unwrap().iter().any(|h| h.memory.kind == Kind::Conversation));
        let recalled = recall(&s, &model, "what breed is my dog").unwrap();
        assert_eq!((recalled[0].memory.kind, recalled[0].memory.text.as_str()), (Kind::Note, "My dog Rex is a retriever"));
        let talk = &s.recent(5, Some(Kind::Conversation)).unwrap().into_iter().find(|m| m.title.starts_with("How much")).unwrap();
        assert_eq!((talk.title.as_str(), talk.text.as_str()), ("How much was the invoice from the supplier?", "Asked: How much was the invoice from the supplier?\nAnswered: It was 40 pounds."));
        assert_eq!(headline(&"long ".repeat(40)).chars().count(), 80);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod probe {
    use super::*;

    /// By hand, against the real memory, to see why a search ranks as it
    /// does: `NEO_APOLLO_QUERY="…" NEO_APOLLO_TITLES="a.gif|b.png" cargo test -p neo-apollo-core real_search -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn the_real_search_is_explained() {
        let settings = crate::settings::Settings::load();
        let model = crate::model::Ollama::from_settings(&settings);
        let store = Store::open_for(&crate::dir().join("memory.db"), &crate::key::database_key().unwrap(), &model).unwrap();
        let query = std::env::var("NEO_APOLLO_QUERY").unwrap();
        let asked = model.embed(std::slice::from_ref(&query), true).unwrap().pop().unwrap();
        let hits = store.search_for(&asked, &crate::words::terms(&query), 400, None).unwrap();
        println!("{:?}", store.stats().unwrap());
        for (i, h) in hits.iter().take(6).enumerate() {
            println!("#{} {:.3} {} :: {}", i + 1, h.distance, h.memory.title, h.memory.text);
        }
        if std::env::var("NEO_APOLLO_ANSWER").is_ok() {
            let mut store = store;
            let answer = ask(Some(&mut store), &model, &[], &query, &Aware::default(), &mut |_, _| true, &mut |_| true).unwrap();
            println!("ANSWER: {}\n  shown beneath: {:?}", plain(&answer.text), sources(&answer.listed, &answer.recalled).iter().map(|m| m.title.as_str()).collect::<Vec<_>>());
            return;
        }
        for title in std::env::var("NEO_APOLLO_TITLES").unwrap_or_default().split('|').filter(|t| !t.is_empty()) {
            match hits.iter().position(|h| h.memory.title == title) {
                Some(i) => println!("{title}: #{} {:.3} :: {} :: {:?}", i + 1, hits[i].distance, hits[i].memory.text, store.words_of(hits[i].memory.id).unwrap()),
                None => println!("{title}: not among the nearest {}", hits.len()),
            }
        }
    }
}
