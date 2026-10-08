//! Reading folders into memories.
//!
//! A picture is shown to the model, which says what is in it. A video is
//! a few pictures taken along its length. A document is its text, cut
//! into passages. Each becomes a memory with an embedding, and a file
//! that has not changed since it was read is not read again.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::UNIX_EPOCH;

use crate::model::{Model, Seen};
use crate::store::{Kind, New, Store};
use crate::words;

/// What sort of file something is, to Apollo.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum What {
    // In the order they are read: what is quick first.
    Document,
    Photo,
    Video,
}

const PHOTOS: &[&str] = &["jpg", "jpeg", "png", "gif", "webp", "bmp", "tif", "tiff", "heic", "heif", "avif"];
const VIDEOS: &[&str] = &["mp4", "mov", "m4v", "mkv", "webm", "avi", "wmv", "flv", "mpg", "mpeg", "3gp", "ts"];
/// Text as it stands.
const PLAIN: &[&str] = &["txt", "md", "markdown", "org", "rst", "tex", "text"];
/// Text that a tool has to get out.
const RICH: &[&str] = &["rtf", "doc", "docx", "odt", "html", "htm"];

/// Folders that are not the user's own things.
const SKIPPED: &[&str] = &["node_modules", "target", "Library", "__pycache__", "venv"];
/// A picture smaller than this is an icon, not a photo.
const SMALLEST_PHOTO: u64 = 8 * 1024;
/// More text than this in one file is data, not a document.
const MOST_TEXT: u64 = 8 * 1024 * 1024;
/// The longest side of a picture as the model is shown it.
const LOOK_SIDE: u32 = 768;
/// About how long a passage is, in characters, and the most taken from one file.
const PASSAGE: usize = 1400;
const MOST_PASSAGES: usize = 60;

fn extension(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

pub fn classify(path: &Path) -> Option<What> {
    let ext = extension(path);
    let ext = ext.as_str();
    if PHOTOS.contains(&ext) {
        Some(What::Photo)
    } else if VIDEOS.contains(&ext) {
        Some(What::Video)
    } else if PLAIN.contains(&ext) || RICH.contains(&ext) || ext == "pdf" {
        Some(What::Document)
    } else {
        None
    }
}

/// What tells one version of a file from the next: its size and when it
/// was last changed.
pub fn stamp(meta: &std::fs::Metadata) -> String {
    format!("{}-{}", meta.len(), modified(meta))
}

/// Counts up when pictures come to be described differently, so that
/// those described the old way are read again.
const DESCRIBED: u32 = 2;

/// A file's stamp, with how it was read: a photo or a video described an
/// older way does not count as read.
fn stamp_of(path: &Path, meta: &std::fs::Metadata) -> String {
    match classify(path) {
        Some(What::Photo | What::Video) => format!("{}-d{DESCRIBED}", stamp(meta)),
        _ => stamp(meta),
    }
}

fn modified(meta: &std::fs::Metadata) -> u64 {
    meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs())
}

/// Whether a folder of this name is passed over with all that is in it:
/// hidden ones, those that are not the user's own things, and packages,
/// such as an app or a photo library, which are one thing and not a
/// folder of files.
fn passed_over(name: &str) -> bool {
    name.starts_with('.') || SKIPPED.contains(&name) || ["app", "photoslibrary", "bundle", "framework"].contains(&extension(Path::new(name)).as_str())
}

/// What a file is to Apollo, if it is one Apollo reads: of a kind it
/// knows, and neither an icon nor a heap of data.
fn fits(path: &Path, meta: &std::fs::Metadata) -> Option<What> {
    let what = classify(path)?;
    let too_small = what == What::Photo && meta.len() < SMALLEST_PHOTO;
    let too_large = what == What::Document && meta.len() > MOST_TEXT && extension(path) != "pdf";
    (meta.is_file() && meta.len() > 0 && !too_small && !too_large && !name(path).starts_with('.')).then_some(what)
}

/// Every file under `roots` that Apollo reads: documents, which are quick,
/// and then photos and videos together, the newest first, so that what
/// was just made is remembered soonest.
pub fn walk(roots: &[PathBuf]) -> Vec<PathBuf> {
    fn into(dir: &Path, out: &mut Vec<(What, u64, PathBuf)>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let (path, name) = (entry.path(), entry.file_name().to_string_lossy().into_owned());
            // Links are not followed: what they point at is read where it is.
            let Ok(kind) = entry.file_type() else { continue };
            if name.starts_with('.') || kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                if !passed_over(&name) {
                    into(&path, out);
                }
            } else if let Ok(meta) = entry.metadata()
                && let Some(what) = fits(&path, &meta)
            {
                out.push((what, modified(&meta), path));
            }
        }
    }
    let mut found = vec![];
    for root in roots {
        into(root, &mut found);
    }
    // Photos and videos take their turn together by age: read by kind, a
    // large folder of photos would keep every video waiting for days.
    let turn = |what: What| what != What::Document;
    found.sort_by(|a, b| turn(a.0).cmp(&turn(b.0)).then(b.1.cmp(&a.1)).then_with(|| a.2.cmp(&b.2)));
    found.dedup_by(|a, b| a.2 == b.2);
    found.into_iter().map(|f| f.2).collect()
}

/// Cuts text into passages of about [`PASSAGE`] characters, at the ends of
/// paragraphs where it can and of sentences or words where it must.
pub fn passages(text: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let mut now = String::new();
    let put = |now: &mut String, out: &mut Vec<String>| {
        let done = now.trim();
        if !done.is_empty() {
            out.push(done.to_owned());
        }
        now.clear();
    };
    for paragraph in text.split("\n\n").map(str::trim).filter(|p| !p.is_empty()) {
        if !now.is_empty() && now.len() + paragraph.len() > PASSAGE {
            put(&mut now, &mut out);
        }
        // One paragraph too long for a passage is cut where a sentence or a word ends.
        let mut rest = paragraph;
        while rest.len() > PASSAGE {
            let mut cut = PASSAGE;
            while !rest.is_char_boundary(cut) {
                cut -= 1;
            }
            let head = &rest[..cut];
            let at = head.rfind(". ").map(|i| i + 1).or_else(|| head.rfind(char::is_whitespace)).filter(|i| *i > PASSAGE / 2).unwrap_or(cut);
            out.push(rest[..at].trim().to_owned());
            rest = rest[at..].trim_start();
        }
        if !now.is_empty() {
            now.push_str("\n\n");
        }
        now.push_str(rest);
    }
    put(&mut now, &mut out);
    out.truncate(MOST_PASSAGES);
    out
}

fn run_tool(program: &str, args: &[&std::ffi::OsStr]) -> Option<Vec<u8>> {
    let tool = neo_desktop::fs::tool(program);
    let out = Command::new(tool).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then_some(out.stdout)
}

fn have(program: &str) -> bool {
    neo_desktop::fs::tool(program).is_file()
}

/// Why a file was left unread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Read, and this many memories made of it.
    Read(u32),
    /// Not read, because the program that could read it is not installed.
    /// It will be tried again.
    Needs(&'static str),
}

/// A document's text.
fn text_of(path: &Path) -> Result<Result<String, &'static str>, String> {
    let ext = extension(path);
    if PLAIN.contains(&ext.as_str()) {
        return std::fs::read(path).map(|b| Ok(String::from_utf8_lossy(&b).into_owned())).map_err(|e| e.to_string());
    }
    if ext == "pdf" {
        if !have("pdftotext") {
            return Ok(Err("pdftotext"));
        }
        let text = run_tool("pdftotext", &["-q".as_ref(), "-enc".as_ref(), "UTF-8".as_ref(), path.as_os_str(), "-".as_ref()]).ok_or("pdftotext could not read it")?;
        return Ok(Ok(String::from_utf8_lossy(&text).replace('\u{c}', "\n\n")));
    }
    // Word processors' files: macOS has a tool for these, others pandoc.
    if cfg!(target_os = "macos") {
        let text = run_tool("textutil", &["-convert".as_ref(), "txt".as_ref(), "-stdout".as_ref(), path.as_os_str()]).ok_or("textutil could not read it")?;
        return Ok(Ok(String::from_utf8_lossy(&text).into_owned()));
    }
    if !have("pandoc") {
        return Ok(Err("pandoc"));
    }
    let text = run_tool("pandoc", &["-t".as_ref(), "plain".as_ref(), path.as_os_str()]).ok_or("pandoc could not read it")?;
    Ok(Ok(String::from_utf8_lossy(&text).into_owned()))
}

fn shrink(picture: image::DynamicImage) -> Result<Vec<u8>, String> {
    let small = if picture.width().max(picture.height()) > LOOK_SIDE { picture.thumbnail(LOOK_SIDE, LOOK_SIDE) } else { picture }.to_rgb8();
    let mut jpeg = vec![];
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 82).encode_image(&small).map_err(|e| e.to_string())?;
    Ok(jpeg)
}

/// A picture as the model is shown it: a small JPEG.
fn look_at(path: &Path) -> Result<Vec<u8>, String> {
    if let Ok(picture) = image::ImageReader::open(path).map_err(|e| e.to_string()).and_then(|r| r.with_guessed_format().map_err(|e| e.to_string())).and_then(|r| r.decode().map_err(|e| e.to_string())) {
        return shrink(picture);
    }
    // A kind this cannot read itself, such as a phone's HEIC: the system's
    // own converter on macOS, ffmpeg anywhere.
    let out = std::env::temp_dir().join(format!("neo-apollo-look-{}-{}.jpg", std::process::id(), std::thread::current().name().unwrap_or("t").len()));
    let side = LOOK_SIDE.to_string();
    let made = (cfg!(target_os = "macos") && run_tool("sips", &["-s".as_ref(), "format".as_ref(), "jpeg".as_ref(), "-Z".as_ref(), side.as_ref(), path.as_os_str(), "--out".as_ref(), out.as_os_str()]).is_some())
        || run_tool("ffmpeg", &["-y".as_ref(), "-v".as_ref(), "error".as_ref(), "-i".as_ref(), path.as_os_str(), "-frames:v".as_ref(), "1".as_ref(), "-vf".as_ref(), format!("scale='min({side},iw)':-2").as_ref(), out.as_os_str()]).is_some();
    let jpeg = std::fs::read(&out);
    let _ = std::fs::remove_file(&out);
    match jpeg {
        Ok(jpeg) if made && !jpeg.is_empty() => Ok(jpeg),
        _ => Err("It is not a picture that can be read.".into()),
    }
}

/// A small picture of a photo, or of a moment early in a video, for a
/// list: its width, its height, and its pixels as RGBA, the longest side
/// no more than `side`. `None` for anything else, or if it cannot be read.
pub fn thumbnail(path: &Path, side: u32) -> Option<(u32, u32, Vec<u8>)> {
    if !path.is_file() {
        return None;
    }
    let jpeg = match classify(path)? {
        What::Photo => look_at(path).ok()?,
        // Not the very start, which is so often black.
        What::Video => frame(path, duration(path).map_or(0.0, |d| (d * 0.15).min(10.0)))?,
        What::Document => return None,
    };
    let small = image::load_from_memory(&jpeg).ok()?.thumbnail(side, side).into_rgba8();
    let (w, h) = small.dimensions();
    Some((w, h, small.into_raw()))
}

/// How long a video is, in seconds.
fn duration(path: &Path) -> Option<f64> {
    let out = run_tool("ffprobe", &["-v".as_ref(), "error".as_ref(), "-show_entries".as_ref(), "format=duration".as_ref(), "-of".as_ref(), "csv=p=0".as_ref(), path.as_os_str()])?;
    String::from_utf8_lossy(&out).trim().parse().ok().filter(|d: &f64| d.is_finite() && *d > 0.0)
}

/// The moments of a video that are looked at: more of a long one.
pub fn moments(seconds: f64) -> Vec<f64> {
    let shares: &[f64] = if seconds < 4.0 {
        &[0.5]
    } else if seconds < 600.0 {
        &[0.15, 0.5, 0.85]
    } else {
        &[0.08, 0.3, 0.5, 0.7, 0.92]
    };
    shares.iter().map(|s| (seconds * s * 10.0).round() / 10.0).collect()
}

fn frame(path: &Path, at: f64) -> Option<Vec<u8>> {
    let jpeg = run_tool("ffmpeg", &["-v".as_ref(), "error".as_ref(), "-ss".as_ref(), format!("{at:.1}").as_ref(), "-i".as_ref(), path.as_os_str(), "-frames:v".as_ref(), "1".as_ref(), "-vf".as_ref(), format!("scale='min({LOOK_SIDE},iw)':-2").as_ref(), "-f".as_ref(), "image2pipe".as_ref(), "-vcodec".as_ref(), "mjpeg".as_ref(), "-".as_ref()])?;
    (!jpeg.is_empty()).then_some(jpeg)
}

/// A length of time as a clock shows it: "1:05", "1:02:03".
pub fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

/// What was seen along a video, as one piece of text and its words.
pub fn video_text(seconds: f64, seen: &[(f64, Seen)]) -> Seen {
    let mut text = format!("A video {} long.", clock(seconds));
    for (at, s) in seen {
        text.push_str(&format!(" At {}: {}", clock(*at), s.text));
    }
    Seen { text, words: words::clean_all(seen.iter().flat_map(|(_, s)| s.words.iter()), 10) }
}

fn name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Remembers one thing, giving any of its words that are new their embeddings.
pub fn remember(store: &mut Store, model: &dyn Model, new: &New) -> Result<i64, String> {
    // The name is part of what is searched for: "the Neo recording", "invoice".
    let said = format!("{}\n{}", new.title, new.text);
    let embedding = model.embed(&[said], false)?.pop().ok_or("no embedding")?;
    let id = store.add(new, &embedding)?;
    learn_words(store, model, new.words)?;
    Ok(id)
}

fn learn_words(store: &mut Store, model: &dyn Model, words: &[String]) -> Result<(), String> {
    let fresh = store.words_without_vectors(words)?;
    for (word, vector) in fresh.iter().zip(model.embed(&fresh, false)?) {
        store.set_word_vector(word, &vector)?;
    }
    Ok(())
}

/// What a listed-only file's stamp ends with.
const LISTED: &str = "-l";

/// Lists a photo or a video without looking at it: a memory of its name,
/// its folder and its date, which is enough to find it by those and to
/// count it among the photos from last month. It costs one embedding,
/// where looking costs a model of several gigabytes some seconds; so a
/// folder of thousands is listed in the time a dozen would be looked at.
/// Looking comes later: when something asks after it, or for everything
/// at once if that was chosen.
pub fn list_file(store: &mut Store, model: &dyn Model, path: &Path) -> Result<Outcome, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    let kind = match classify(path) {
        Some(What::Photo) => Kind::Photo,
        Some(What::Video) => Kind::Video,
        _ => return index_file(store, model, path),
    };
    let (title, folder) = (name(path), path.parent().and_then(Path::file_name).map(|f| f.to_string_lossy().into_owned()).unwrap_or_default());
    let what = if kind == Kind::Photo { "A photo" } else { "A video" };
    let text = format!("{what} that has not been looked at yet, called {title}, in the folder {folder}.");
    // What its name and folder say of it: "Neo Recording", "Trips".
    let about = words::clean_all(words::terms(&format!("{} {folder}", path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default())), 6);
    store.forget_source(path)?;
    let id = remember(store, model, &New { kind, source: Some(path), title: &title, text: &text, part: 0, created: modified(&meta), words: &about })?;
    store.mark_light(id)?;
    store.set_file(path, &format!("{}{LISTED}", stamp(&meta)), 1)?;
    Ok(Outcome::Read(1))
}

/// Reads one file into the memory, in place of whatever was remembered of
/// it before.
pub fn index_file(store: &mut Store, model: &dyn Model, path: &Path) -> Result<Outcome, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    let (title, created) = (name(path), modified(&meta));
    let what = classify(path).ok_or("Not a kind of file Apollo reads.")?;
    let made = match what {
        What::Photo => {
            let seen = model.describe(&look_at(path)?)?;
            store.forget_source(path)?;
            remember(store, model, &New { kind: Kind::Photo, source: Some(path), title: &title, text: &seen.text, part: 0, created, words: &seen.words })?;
            1
        }
        What::Video => {
            if !have("ffmpeg") || !have("ffprobe") {
                return Ok(Outcome::Needs("ffmpeg"));
            }
            let seconds = duration(path).ok_or("ffmpeg could not read it.")?;
            let mut seen = vec![];
            for at in moments(seconds) {
                if let Some(jpeg) = frame(path, at) {
                    seen.push((at, model.describe(&jpeg)?));
                }
            }
            if seen.is_empty() {
                return Err("No picture could be taken from it.".into());
            }
            let all = video_text(seconds, &seen);
            store.forget_source(path)?;
            remember(store, model, &New { kind: Kind::Video, source: Some(path), title: &title, text: &all.text, part: 0, created, words: &all.words })?;
            1
        }
        What::Document => {
            let text = match text_of(path)? {
                Ok(text) => text,
                Err(tool) => return Ok(Outcome::Needs(tool)),
            };
            let parts = passages(&text);
            store.forget_source(path)?;
            // Embedded a few at a time: one request for each would crawl.
            for (n, group) in parts.chunks(16).enumerate() {
                let said: Vec<String> = group.iter().map(|p| format!("{title}\n{p}")).collect();
                for (i, (passage, embedding)) in group.iter().zip(model.embed(&said, false)?).enumerate() {
                    let about = words::keywords(passage, 5);
                    store.add(&New { kind: Kind::Document, source: Some(path), title: &title, text: passage, part: (n * 16 + i) as u32, created, words: &about }, &embedding)?;
                    learn_words(store, model, &about)?;
                }
            }
            parts.len() as u32
        }
    };
    store.set_file(path, &stamp_of(path, &meta), made)?;
    Ok(Outcome::Read(made))
}

/// How a run through the folders is going.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    /// Files looked at so far, of those there are.
    pub done: usize,
    pub total: usize,
    /// The file being read.
    pub now: Option<PathBuf>,
    /// Files read this run (documents read, pictures and videos looked
    /// at), and those that could not be.
    pub read: usize,
    /// Pictures and videos that were listed this run without being looked at.
    pub listed: usize,
    pub failed: usize,
    /// Files forgotten because they are gone, or their folder is no longer read.
    pub forgotten: usize,
    /// Programs that would let more be read, if they were installed.
    pub needs: Vec<&'static str>,
    /// The last thing to go wrong, and with what.
    pub trouble: Option<String>,
    /// Pictures and videos are being looked at, which is the slow part,
    /// and not merely listed.
    pub looking: bool,
}

/// Forgets files that are gone, or are not under any of `roots` any more.
pub fn sweep(store: &mut Store, roots: &[PathBuf]) -> Result<usize, String> {
    let mut gone = 0;
    for file in store.files()? {
        if !file.is_file() || !roots.iter().any(|r| file.starts_with(r)) {
            store.forget_source(&file)?;
            gone += 1;
        }
    }
    if gone > 0 {
        store.tidy_words()?;
    }
    Ok(gone)
}

/// Brings the memory up to date with `roots`: forgets what is gone and
/// takes in what is new or changed. Everything is listed first, which is
/// quick; with `look`, every photo and video is then looked at as well,
/// which is not. `report` hears how it is going before each file and at
/// the end; returning false from it stops the run there, to be taken up
/// again later.
pub fn run(store: &mut Store, model: &dyn Model, roots: &[PathBuf], look: bool, report: &mut dyn FnMut(&Progress) -> bool) -> Progress {
    let mut p = Progress::default();
    match sweep(store, roots) {
        Ok(n) => p.forgotten = n,
        Err(e) => p.trouble = Some(e),
    }
    let files = walk(roots);
    let mut stopped = false;
    let p = read_files(store, model, files.clone(), p, false, &mut |p| {
        stopped = !report(p);
        !stopped
    });
    if !look || stopped {
        return p;
    }
    // Then the slow part, newest first, as the files were found.
    let waiting: Vec<PathBuf> = files.into_iter().filter(|f| matches!(classify(f), Some(What::Photo | What::Video))).collect();
    read_files(store, model, waiting.clone(), Progress { done: p.total - waiting.len(), ..p }, true, report)
}

/// Brings the memory up to date with what has changed at `changed`, and
/// nothing else: a file there is taken in if it is new or different, what
/// was remembered of one that has gone is forgotten, and a folder is
/// looked through. Paths outside `roots`, or in folders that are passed
/// over, are left alone. For when the system says which files changed, so
/// that one new screenshot does not mean looking through everything
/// again. With `look`, photos and videos are looked at and not only listed.
pub fn update(store: &mut Store, model: &dyn Model, roots: &[PathBuf], changed: &[PathBuf], look: bool, report: &mut dyn FnMut(&Progress) -> bool) -> Progress {
    let mut p = Progress::default();
    let mut files: Vec<PathBuf> = vec![];
    for path in changed {
        let Some(root) = roots.iter().find(|r| path.starts_with(r)) else { continue };
        let inside = path.strip_prefix(root).unwrap_or(path);
        let folders = inside.parent().into_iter().flat_map(Path::components);
        if folders.into_iter().any(|c| passed_over(&c.as_os_str().to_string_lossy())) {
            continue;
        }
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.is_dir() => {
                if !passed_over(&name(path)) || path == root {
                    files.extend(walk(std::slice::from_ref(path)));
                }
            }
            Ok(meta) => {
                if fits(path, &meta).is_some() {
                    files.push(path.clone());
                }
            }
            // Gone: the file itself, or a folder with files in it.
            Err(_) => {
                let was: Vec<PathBuf> = store.files().unwrap_or_default().into_iter().filter(|f| f.starts_with(path)).collect();
                for file in was {
                    match store.forget_source(&file) {
                        Ok(_) => p.forgotten += 1,
                        Err(e) => p.trouble = Some(e),
                    }
                }
            }
        }
    }
    if p.forgotten > 0 {
        let _ = store.tidy_words();
    }
    files.sort();
    files.dedup();
    read_files(store, model, files, p, look, report)
}

/// Looks at these files now, whatever is known of them: for the photos
/// and videos that a question or a search has turned up only listed.
pub fn look_at_files(store: &mut Store, model: &dyn Model, files: Vec<PathBuf>, report: &mut dyn FnMut(&Progress) -> bool) -> Progress {
    read_files(store, model, files, Progress::default(), true, report)
}

fn read_files(store: &mut Store, model: &dyn Model, files: Vec<PathBuf>, mut p: Progress, look: bool, report: &mut dyn FnMut(&Progress) -> bool) -> Progress {
    p.total = p.done + files.len();
    p.looking = look;
    for file in files {
        let Ok(meta) = std::fs::metadata(&file) else {
            p.done += 1;
            continue;
        };
        let had = store.file_stamp(&file).ok().flatten();
        let seen = matches!(classify(&file), Some(What::Photo | What::Video));
        // Looked at already, or listed already and that is all that is asked.
        let unchanged = had.as_deref() == Some(stamp_of(&file, &meta).as_str()) || (seen && !look && had.as_deref() == Some(format!("{}{LISTED}", stamp(&meta)).as_str()));
        if !unchanged {
            p.now = Some(file.clone());
            if !report(&p) {
                p.now = None;
                return p;
            }
            match if look { index_file(store, model, &file) } else { list_file(store, model, &file) } {
                Ok(Outcome::Read(_)) if seen && !look => p.listed += 1,
                Ok(Outcome::Read(_)) => p.read += 1,
                Ok(Outcome::Needs(tool)) => {
                    if !p.needs.contains(&tool) {
                        p.needs.push(tool);
                    }
                }
                Err(e) => {
                    p.failed += 1;
                    p.trouble = Some(format!("{}: {e}", name(&file)));
                    // One that is listed and cannot be looked at stays listed,
                    // and is not tried again until it changes.
                    if look && seen && had.is_some() {
                        let _ = store.set_file(&file, &stamp_of(&file, &meta), 1);
                    }
                }
            }
        }
        p.done += 1;
    }
    p.now = None;
    report(&p);
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::fake::Fake;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("neo-apollo-index-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn picture(path: &Path, colour: [u8; 3]) {
        // Noise over the colour, so that the file is not too small to be a photo.
        let img = image::RgbImage::from_fn(240, 160, |x, y| if x < 8 && y < 8 { image::Rgb(colour) } else { image::Rgb([colour[0] ^ ((x * 7 + y * 13) % 31) as u8, colour[1] ^ ((x * 3 + y * 5) % 29) as u8, colour[2] ^ ((x + y * 11) % 23) as u8]) });
        img.save(path).unwrap();
    }

    #[test]
    fn files_are_sorted_into_kinds_and_the_rest_left() {
        assert_eq!(classify(Path::new("/a/IMG_1.HEIC")), Some(What::Photo));
        assert_eq!(classify(Path::new("clip.MKV")), Some(What::Video));
        assert_eq!((classify(Path::new("notes.md")), classify(Path::new("paper.pdf")), classify(Path::new("letter.docx"))), (Some(What::Document), Some(What::Document), Some(What::Document)));
        assert_eq!((classify(Path::new("main.rs")), classify(Path::new("data.bin")), classify(Path::new("README"))), (None, None, None));
    }

    #[test]
    fn text_is_cut_into_passages_at_natural_places() {
        assert_eq!(passages("One.\n\nTwo.\n\n\n\nThree."), ["One.\n\nTwo.\n\nThree."], "short paragraphs go together");
        assert_eq!(passages("  \n\n "), Vec::<String>::new());
        let long = format!("{}\n\n{}", "A sentence here. ".repeat(60), "Another one there. ".repeat(60));
        let parts = passages(&long);
        assert_eq!(parts.len(), 2, "each paragraph fits a passage of its own");
        let endless = "word ".repeat(1000);
        let parts = passages(&endless);
        assert!(parts.len() >= 3 && parts.iter().all(|p| p.len() <= PASSAGE && !p.starts_with(' ') && p.ends_with("word")), "cut between words");
        assert_eq!(parts.join(" ").split_whitespace().count(), 1000, "and nothing lost");
        assert!(passages(&"é".repeat(4000)).iter().all(|p| p.chars().all(|c| c == 'é')), "never in the middle of a letter");
        assert_eq!(passages(&"Para.\n\n".repeat(20_000)).len(), MOST_PASSAGES);
    }

    #[test]
    fn a_video_is_looked_at_along_its_length() {
        assert_eq!(moments(2.0), [1.0]);
        assert_eq!(moments(100.0), [15.0, 50.0, 85.0]);
        assert_eq!(moments(1200.0).len(), 5);
        assert_eq!((clock(5.4), clock(65.0), clock(3723.0)), ("0:05".into(), "1:05".into(), "1:02:03".into()));
        let seen = [(15.0, Seen { text: "A dog.".into(), words: vec!["dog".into()] }), (50.0, Seen { text: "The sea.".into(), words: vec!["sea".into(), "dog".into()] })];
        assert_eq!(video_text(100.0, &seen), Seen { text: "A video 1:40 long. At 0:15: A dog. At 0:50: The sea.".into(), words: vec!["dog".into(), "sea".into()] });
    }

    #[test]
    fn folders_are_read_into_memory_and_only_what_changed_is_read_again() {
        let dir = scratch("run");
        let model = Fake::default();
        let mut store = Store::open(&dir.join("db/memory.db"), &[1; 32], model.dims().unwrap()).unwrap();
        let root = dir.join("things");
        std::fs::create_dir_all(root.join("trip/.hidden")).unwrap();
        std::fs::create_dir_all(root.join("node_modules")).unwrap();
        picture(&root.join("trip/red.png"), [220, 40, 40]);
        picture(&root.join("trip/green.png"), [40, 220, 40]);
        picture(&root.join("trip/.hidden/blue.png"), [40, 40, 220]);
        picture(&root.join("node_modules/blue.png"), [40, 40, 220]);
        std::fs::write(root.join("bill.md"), "The invoice from the supplier.\n\nPayment of the invoice is due to the supplier's bank in March.").unwrap();
        std::fs::write(root.join("tiny.png"), b"not a photo").unwrap();
        std::fs::write(root.join("program.rs"), "fn main() {}").unwrap();
        let roots = vec![root.clone()];
        assert_eq!(walk(&roots).iter().map(|p| name(p)).collect::<Vec<_>>().len(), 3, "hidden folders, dependencies, icons and code are left");
        assert_eq!(name(&walk(&roots)[0]), "bill.md", "documents first");

        let mut seen = vec![];
        let p = run(&mut store, &model, &roots, true, &mut |p| {
            seen.extend(p.now.as_deref().map(name));
            true
        });
        assert_eq!((p.done, p.total, p.read, p.failed, p.trouble.clone()), (3, 3, 3, 0, None));
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), 3, "each said as it is listed, and the pictures again as they are looked at");
        let stats = store.stats().unwrap();
        assert_eq!((stats.of(Kind::Photo), stats.of(Kind::Document), stats.files), (2, 1, 3));
        // What a picture shows finds it.
        let ask = |q: &str| model.embed(&[q.to_owned()], true).unwrap().pop().unwrap();
        let found = store.search(&ask("a dog at the beach"), 1, None).unwrap();
        assert_eq!((found[0].memory.title.as_str(), found[0].memory.kind, found[0].memory.text.as_str()), ("red.png", Kind::Photo, "A dog running on a beach."));
        assert_eq!(store.search(&ask("invoice payment"), 1, None).unwrap()[0].memory.title, "bill.md");
        assert_eq!(store.words_of(found[0].memory.id).unwrap(), ["beach", "dog"]);
        assert!(store.cloud(20, None).unwrap().iter().any(|w| w.word == "invoice" || w.also.iter().any(|a| a == "invoice")), "a document's own words are in the cloud");
        assert!(store.word_vector("dog").unwrap().is_some());

        // Nothing has changed: nothing is read.
        let p = run(&mut store, &model, &roots, true, &mut |p| {
            assert_eq!(p.now, None, "no file needed reading");
            true
        });
        assert_eq!((p.done, p.read), (3, 0));
        // One changes and one goes.
        picture(&root.join("trip/red.png"), [40, 40, 220]);
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options().write(true).open(root.join("trip/red.png")).unwrap().set_modified(later).unwrap();
        std::fs::remove_file(root.join("trip/green.png")).unwrap();
        let p = run(&mut store, &model, &roots, true, &mut |_| true);
        assert_eq!((p.read, p.forgotten, p.total), (1, 1, 2));
        assert_eq!(store.stats().unwrap().of(Kind::Photo), 1);
        assert_eq!(store.recent(1, Some(Kind::Photo)).unwrap()[0].text, "Waves on the sea.", "read afresh, not added to");
        assert!(store.cloud(50, None).unwrap().iter().all(|w| w.word != "mountain"), "the words of what is gone go with it");
        // A folder no longer read is forgotten.
        let p = run(&mut store, &model, &[], true, &mut |_| true);
        assert_eq!((p.forgotten, store.stats().unwrap().total()), (2, 0));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn photos_and_videos_have_small_pictures_for_lists() {
        let dir = scratch("thumb");
        picture(&dir.join("wide.png"), [220, 40, 40]);
        let (w, h, rgba) = thumbnail(&dir.join("wide.png"), 120).expect("a photo has one");
        assert_eq!((w, h, rgba.len()), (120, 80, 120 * 80 * 4), "the shape kept, the longest side brought down");
        std::fs::write(dir.join("notes.txt"), "words").unwrap();
        std::fs::write(dir.join("broken.png"), vec![0u8; 9000]).unwrap();
        assert!(thumbnail(&dir.join("notes.txt"), 120).is_none() && thumbnail(&dir.join("gone.png"), 120).is_none() && thumbnail(&dir.join("broken.png"), 120).is_none());
        if have("ffmpeg") && have("ffprobe") {
            let clip = dir.join("clip.mp4");
            assert!(Command::new(neo_desktop::fs::tool("ffmpeg")).args(["-v", "error", "-f", "lavfi", "-i", "color=c=0x2828DC:s=320x180:d=3", "-pix_fmt", "yuv420p"]).arg(&clip).status().unwrap().success());
            let (w, h, rgba) = thumbnail(&clip, 160).expect("a video has one");
            assert_eq!((w, h), (160, 90));
            assert!(rgba[2] > 150 && rgba[0] < 110, "a frame of the video itself: {:?}", &rgba[..4]);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn only_what_changed_is_looked_at_when_the_changes_are_known() {
        let dir = scratch("update");
        let model = Fake::default();
        let mut store = Store::open(&dir.join("db/memory.db"), &[1; 32], model.dims().unwrap()).unwrap();
        let root = dir.join("things");
        std::fs::create_dir_all(root.join("trip")).unwrap();
        std::fs::create_dir_all(root.join(".cache")).unwrap();
        std::fs::write(root.join("old.txt"), "An old note about an invoice.").unwrap();
        let roots = vec![root.clone()];
        assert_eq!(run(&mut store, &model, &roots, true, &mut |_| true).read, 1);
        // Another file is there that no change was told of: it is not gone looking for.
        std::fs::write(root.join("unseen.txt"), "Nobody said this was here.").unwrap();
        picture(&root.join("trip/red.png"), [220, 40, 40]);
        picture(&root.join(".cache/blue.png"), [40, 40, 220]);
        std::fs::write(root.join("program.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.join("outside.txt"), "Not in a folder that is read.").unwrap();
        let told = [root.join("trip/red.png"), root.join(".cache/blue.png"), root.join("program.rs"), dir.join("outside.txt"), root.join("old.txt")];
        let mut seen = vec![];
        let p = update(&mut store, &model, &roots, &told, true, &mut |p| {
            seen.extend(p.now.as_deref().map(name));
            true
        });
        assert_eq!((p.read, p.total, seen), (1, 2, vec!["red.png".to_owned()]), "the new picture is read; the note is looked at and found the same; the rest are not Apollo's to read");
        assert_eq!((store.stats().unwrap().total(), store.file_stamp(&root.join("unseen.txt")).unwrap()), (2, None));
        // A folder that appears is looked through; one that goes takes its memories with it.
        std::fs::create_dir_all(root.join("more")).unwrap();
        picture(&root.join("more/green.png"), [40, 220, 40]);
        std::fs::write(root.join("more/note.md"), "Hiking the mountain in the snow.").unwrap();
        assert_eq!(update(&mut store, &model, &roots, &[root.join("more")], true, &mut |_| true).read, 2);
        std::fs::remove_dir_all(root.join("more")).unwrap();
        std::fs::remove_file(root.join("trip/red.png")).unwrap();
        let p = update(&mut store, &model, &roots, &[root.join("more"), root.join("trip/red.png"), root.join("never-was.png")], true, &mut |_| true);
        assert_eq!((p.forgotten, p.read, store.stats().unwrap().total()), (3, 0, 1));
        // A full look afterwards finds what no change told of.
        assert_eq!(run(&mut store, &model, &roots, true, &mut |_| true).read, 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn photos_and_videos_are_listed_at_once_and_looked_at_when_asked_after() {
        let dir = scratch("listed");
        let model = Fake::default();
        let mut store = Store::open(&dir.join("db/memory.db"), &[1; 32], model.dims().unwrap()).unwrap();
        let root = dir.join("Trips");
        std::fs::create_dir_all(&root).unwrap();
        picture(&root.join("beach dog.png"), [220, 40, 40]);
        picture(&root.join("IMG_0002.png"), [40, 220, 40]);
        std::fs::write(root.join("plan.txt"), "Hiking the mountain in the snow.").unwrap();
        let roots = vec![root.clone()];
        // Listing: every file is known, the pictures by name and place only, and nothing was looked at.
        let p = run(&mut store, &model, &roots, false, &mut |p| {
            assert!(!p.looking);
            true
        });
        assert_eq!((p.read, p.listed, p.total, p.looking), (1, 2, 3, false));
        let stats = store.stats().unwrap();
        assert_eq!((stats.of(Kind::Photo), stats.of(Kind::Document), stats.light), (2, 1, 2), "a document is read in full either way");
        let listed = &store.recent(5, Some(Kind::Photo)).unwrap();
        let dog = listed.iter().find(|m| m.title == "beach dog.png").unwrap();
        assert_eq!((dog.light, dog.text.as_str()), (true, "A photo that has not been looked at yet, called beach dog.png, in the folder Trips."));
        assert_eq!(store.words_of(dog.id).unwrap(), ["beach", "dog", "trips"], "what its name and folder say of it");
        let ask = |q: &str| model.embed(&[q.to_owned()], true).unwrap().pop().unwrap();
        assert_eq!(store.search(&ask("dog on a beach"), 1, None).unwrap()[0].memory.title, "beach dog.png", "found by its name");
        // Listed again, nothing is done.
        assert_eq!(run(&mut store, &model, &roots, false, &mut |_| true).read, 0);
        // Asked after, one is looked at: it is described, and no longer only listed.
        let p = look_at_files(&mut store, &model, vec![root.join("IMG_0002.png")], &mut |p| {
            assert!(p.looking);
            true
        });
        assert_eq!(p.read, 1);
        let seen = store.recent(5, Some(Kind::Photo)).unwrap().into_iter().find(|m| m.title == "IMG_0002.png").unwrap();
        assert_eq!((seen.light, seen.text.as_str(), store.stats().unwrap().light, store.stats().unwrap().of(Kind::Photo)), (false, "A snowy mountain peak.", 1, 2));
        assert_eq!(store.light_files(5).unwrap(), [root.join("beach dog.png")]);
        // Listing afterwards leaves what was looked at alone; looking at everything does the rest, once.
        assert_eq!(run(&mut store, &model, &roots, false, &mut |_| true).read, 0);
        let p = run(&mut store, &model, &roots, true, &mut |_| true);
        assert_eq!((p.read, p.done, p.total, p.looking, store.stats().unwrap().light), (1, 3, 3, true, 0));
        assert_eq!(run(&mut store, &model, &roots, true, &mut |_| true).read, 0);
        // A new picture told of while only listing is listed, not looked at.
        picture(&root.join("new.png"), [40, 40, 220]);
        assert_eq!(update(&mut store, &model, &roots, &[root.join("new.png")], false, &mut |_| true).listed, 1);
        assert_eq!(store.stats().unwrap().light, 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_run_can_be_stopped_and_taken_up_again() {
        let dir = scratch("stop");
        let model = Fake::default();
        let mut store = Store::open(&dir.join("db/memory.db"), &[1; 32], model.dims().unwrap()).unwrap();
        for i in 0..4 {
            std::fs::write(dir.join(format!("note{i}.txt")), format!("Note number {i} about hiking a mountain.")).unwrap();
        }
        // A file that is not what its name says counts as failed, and the rest go on.
        std::fs::write(dir.join("broken.png"), vec![0u8; 20_000]).unwrap();
        let roots = vec![dir.clone()];
        let mut asked = 0;
        let p = run(&mut store, &model, &roots, true, &mut |_| {
            asked += 1;
            asked <= 2
        });
        assert_eq!((p.read, p.done, p.total), (2, 2, 5));
        let p = run(&mut store, &model, &roots, true, &mut |_| true);
        assert_eq!((p.read, p.failed, p.done), (2, 1, 5));
        assert!(p.trouble.unwrap().starts_with("broken.png: "));
        assert_eq!((store.stats().unwrap().total(), store.stats().unwrap().light), (5, 1), "the one that could not be looked at stays listed by its name");
        let p = run(&mut store, &model, &roots, true, &mut |_| true);
        assert_eq!((p.read, p.failed), (0, 0), "and is not tried again until it changes");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_video_is_remembered_by_what_its_frames_show() {
        if !have("ffmpeg") || !have("ffprobe") {
            return;
        }
        let dir = scratch("video");
        let clip = dir.join("clip.mp4");
        let made = Command::new(neo_desktop::fs::tool("ffmpeg")).args(["-v", "error", "-f", "lavfi", "-i", "color=c=0xDC2828:s=160x120:d=6", "-pix_fmt", "yuv420p"]).arg(&clip).status().unwrap();
        assert!(made.success());
        let model = Fake::default();
        let mut store = Store::open(&dir.join("db/memory.db"), &[1; 32], model.dims().unwrap()).unwrap();
        assert_eq!(index_file(&mut store, &model, &clip), Ok(Outcome::Read(1)));
        let m = &store.recent(1, None).unwrap()[0];
        assert_eq!(m.kind, Kind::Video);
        assert!(m.text.starts_with("A video 0:06 long. At 0:01: A dog running on a beach. At 0:03: "), "{}", m.text);
        assert_eq!(store.words_of(m.id).unwrap(), ["beach", "dog"]);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
