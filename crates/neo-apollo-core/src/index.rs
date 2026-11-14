//! Reading folders into memories.
//!
//! A picture is shown to the model, which says what is in it. A video is
//! a few pictures taken along its length. A document is its text, cut
//! into passages. Each becomes a memory with an embedding, and a file
//! that has not changed since it was read is not read again.

use std::path::{Path, PathBuf};
use std::process::Stdio;
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
    Audio,
}

const PHOTOS: &[&str] = &["jpg", "jpeg", "png", "gif", "webp", "bmp", "tif", "tiff", "heic", "heif", "avif"];
const AUDIO: &[&str] = &["mp3", "m4a", "wav", "flac", "ogg", "oga", "opus", "aac", "aiff", "aif", "wma", "caf"];
const VIDEOS: &[&str] = &["mp4", "mov", "m4v", "mkv", "webm", "avi", "wmv", "flv", "mpg", "mpeg", "3gp", "ts"];
/// Text as it stands.
const PLAIN: &[&str] = &["txt", "md", "markdown", "org", "rst", "tex", "text"];
/// Text that a tool has to get out.
const RICH: &[&str] = &["rtf", "doc", "docx", "odt", "html", "htm"];

/// Folders that are not the user's own things.
const SKIPPED: &[&str] = &["node_modules", "target", "Library", "__pycache__", "venv"];
/// And Windows' own, met when a whole drive is read: the system, the
/// programs installed on it and what they keep for themselves.
const WINDOWS_OWN: &[&str] = &["windows", "program files", "program files (x86)", "programdata", "appdata", "system volume information", "recovery", "perflogs", "msocache", "windowsapps"];
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
    } else if AUDIO.contains(&ext) {
        Some(What::Audio)
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

/// What a stamp carries for a file whose sound has been listened to.
const HEARD: &str = "-h1";

/// A file's stamp, with how it was read: a photo or a video described an
/// older way does not count as read.
fn stamp_of(path: &Path, meta: &std::fs::Metadata) -> String {
    match classify(path) {
        Some(What::Photo) => format!("{}-d{DESCRIBED}", stamp(meta)),
        // A video looked at before there was hearing has not been listened
        // to: once there is, it counts as not yet read.
        Some(What::Video) => format!("{}-d{DESCRIBED}{}", stamp(meta), if crate::hear::missing().is_none() { HEARD } else { "" }),
        Some(What::Audio) => format!("{}{HEARD}", stamp(meta)),
        _ => stamp(meta),
    }
}

pub(crate) fn modified(meta: &std::fs::Metadata) -> u64 {
    meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs())
}

/// Whether a folder of this name is passed over with all that is in it:
/// hidden ones, those that are not the user's own things, and packages,
/// such as an app or a photo library, which are one thing and not a
/// folder of files.
pub(crate) fn passed_over(name: &str) -> bool {
    // $Recycle.Bin and its like, and the folders above, whatever their capitals.
    let windows_own = cfg!(windows) && (name.starts_with('$') || WINDOWS_OWN.contains(&name.to_lowercase().as_str()));
    windows_own || name.starts_with('.') || SKIPPED.contains(&name) || ["app", "photoslibrary", "bundle", "framework"].contains(&extension(Path::new(name)).as_str())
}

/// Whether a file of this name and size is one Apollo reads, of the kind
/// it has been found to be: for a file known only from a disk's index.
pub(crate) fn fits_by(what: What, name: &str, size: u64) -> bool {
    let too_small = what == What::Photo && size < SMALLEST_PHOTO;
    let too_large = what == What::Document && size > MOST_TEXT && extension(Path::new(name)) != "pdf";
    size > 0 && !too_small && !too_large && !name.starts_with('.')
}

/// Whether Windows hides a folder, or keeps it as the system's: what is
/// in those is not the user's own things either. Elsewhere a hidden
/// folder is one whose name begins with a dot, which is seen to already.
fn kept_out_of_sight(entry: &std::fs::DirEntry) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const HIDDEN_OR_SYSTEM: u32 = 0x2 | 0x4;
        entry.metadata().is_ok_and(|m| m.file_attributes() & HIDDEN_OR_SYSTEM != 0)
    }
    #[cfg(not(windows))]
    {
        let _ = entry;
        false
    }
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
                if !passed_over(&name) && !kept_out_of_sight(&entry) {
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
    let out = neo_desktop::command(tool).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
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
    /// Read, and found to hold something that looks like a key or a token
    /// (what kind is said), which was left out of what is remembered.
    Holds(u32, &'static str),
    /// Not read, because the program that could read it is not installed.
    /// It will be tried again.
    Needs(&'static str),
}

/// The words of a web page: what is between its tags, without its
/// scripts and styles, a line to each block, with the commonest of the
/// characters that are written by name put back.
fn html_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut rest = html;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        rest = &rest[open..];
        // A comment, or a script or a style with all that is in it, is passed over whole.
        let lower = rest.get(..9).unwrap_or(rest).to_ascii_lowercase();
        let until = if lower.starts_with("<!--") {
            Some("-->")
        } else if lower.starts_with("<script") {
            Some("</script")
        } else if lower.starts_with("<style") {
            Some("</style")
        } else {
            None
        };
        if let Some(until) = until {
            let end = rest.to_ascii_lowercase().find(until).map_or(rest.len(), |i| i + until.len());
            rest = &rest[end..];
            // And the rest of the tag that closed it.
            if until != "-->" {
                rest = rest.find('>').map_or("", |i| &rest[i + 1..]);
            }
            out.push(' ');
            continue;
        }
        let Some(close) = rest.find('>') else { break };
        // What ends a block ends a line; anything else is at most a space.
        let tag = rest[1..close].trim_start_matches('/').split(|c: char| !c.is_ascii_alphanumeric()).next().unwrap_or_default().to_ascii_lowercase();
        out.push_str(if matches!(tag.as_str(), "p" | "div" | "br" | "li" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "section" | "article" | "table" | "ul" | "ol" | "pre" | "blockquote" | "title") { "\n\n" } else { " " });
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    let mut text = out;
    for (name, is) in [("&nbsp;", " "), ("&lt;", "<"), ("&gt;", ">"), ("&quot;", "\""), ("&#39;", "'"), ("&apos;", "'"), ("&mdash;", "—"), ("&ndash;", "–"), ("&hellip;", "…"), ("&amp;", "&")] {
        if text.contains(name) {
            text = text.replace(name, is);
        }
    }
    // Lines trimmed, and no more than one empty line between them.
    let mut tidy = String::with_capacity(text.len());
    let mut empty = 0;
    for line in text.lines().map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")) {
        empty = if line.is_empty() { empty + 1 } else { 0 };
        if empty <= 1 {
            tidy.push_str(&line);
            tidy.push('\n');
        }
    }
    tidy.trim().to_owned()
}

/// Whether a change at `path` could be anything to Apollo: it is not in
/// a folder that is passed over, and is of a kind that is read or has no
/// kind, as a folder has none. For sorting what the system says has
/// changed, which on a whole drive is everything every program writes.
pub fn could_matter(path: &Path) -> bool {
    let named = |c: std::path::Component| match c {
        std::path::Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
        _ => None,
    };
    path.components().filter_map(named).all(|name| !passed_over(&name)) && (path.extension().is_none() || classify(path).is_some())
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
    // A web page is read here: a program started for each one took about
    // a second, and a drive can hold tens of thousands of them.
    if ext == "html" || ext == "htm" {
        return std::fs::read(path).map(|b| Ok(html_text(&String::from_utf8_lossy(&b)))).map_err(|e| e.to_string());
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

/// A picture given as its pixels (RGBA, top row first), as the model is
/// shown it: for one that is in no file, such as one copied from a page.
pub fn shown(width: u32, height: u32, rgba: Vec<u8>) -> Result<Vec<u8>, String> {
    let picture = image::RgbaImage::from_raw(width, height, rgba).ok_or("The picture is not whole.")?;
    shrink(image::DynamicImage::ImageRgba8(picture))
}

/// A small copy of a picture given as its pixels, to show beside what is
/// being looked for: its width, its height and its pixels as RGBA, the
/// longest side no more than `side`.
pub fn small(width: u32, height: u32, rgba: &[u8], side: u32) -> Option<(u32, u32, Vec<u8>)> {
    let picture = image::RgbaImage::from_raw(width, height, rgba.to_vec())?;
    let small = image::DynamicImage::ImageRgba8(picture).thumbnail(side, side).to_rgba8();
    Some((small.width(), small.height(), small.into_raw()))
}

/// The picture in a file, as the model is shown it.
pub fn shown_from(path: &Path) -> Result<Vec<u8>, String> {
    look_at(path)
}

/// A picture as the model is shown it: a small JPEG.
fn look_at(path: &Path) -> Result<Vec<u8>, String> {
    if let Ok(picture) = image::ImageReader::open(path).map_err(|e| e.to_string()).and_then(|r| r.with_guessed_format().map_err(|e| e.to_string())).and_then(|r| r.decode().map_err(|e| e.to_string())) {
        return shrink(picture);
    }
    // A kind this cannot read itself, such as a phone's HEIC: the system's
    // own converter on macOS, ffmpeg anywhere.
    // Several are made at once, so each has a name of its own.
        static MADE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let out = std::env::temp_dir().join(format!("neo-apollo-look-{}-{}.jpg", std::process::id(), MADE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
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
        What::Document | What::Audio => return None,
    };
    let small = image::load_from_memory(&jpeg).ok()?.thumbnail(side, side).into_rgba8();
    let (w, h) = small.dimensions();
    Some((w, h, small.into_raw()))
}

/// How long a video is, in seconds.
pub(crate) fn duration(path: &Path) -> Option<f64> {
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

pub(crate) fn frame(path: &Path, at: f64) -> Option<Vec<u8>> {
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
    remember_known(store, model, &Known::default(), new)
}

/// Embeddings made ahead of time, by what they are of: so that the one
/// hand that writes the memory need not stop to ask the model for each.
type Known = std::collections::HashMap<String, Vec<f32>>;

/// The embeddings of 	exts: from those made ahead where every one is
/// there, and from the model otherwise.
fn embed_known(model: &dyn Model, known: &Known, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
    if texts.iter().all(|t| known.contains_key(t)) {
        return Ok(texts.iter().map(|t| known[t].clone()).collect());
    }
    model.embed(texts, false)
}

fn remember_known(store: &mut Store, model: &dyn Model, known: &Known, new: &New) -> Result<i64, String> {
    // The name is part of what is searched for: "the Neo recording", "invoice".
    let said = format!("{}\n{}", new.title, new.text);
    let embedding = embed_known(model, known, &[said])?.pop().ok_or("no embedding")?;
    let id = store.add(new, &embedding)?;
    learn_words(store, model, known, new.words)?;
    Ok(id)
}

fn learn_words(store: &mut Store, model: &dyn Model, known: &Known, words: &[String]) -> Result<(), String> {
    let fresh = store.words_without_vectors(words)?;
    if fresh.is_empty() {
        return Ok(());
    }
    for (word, vector) in fresh.iter().zip(embed_known(model, known, &fresh)?) {
        store.set_word_vector(word, &vector)?;
    }
    Ok(())
}

/// A folder worth knowing of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub path: PathBuf,
    /// What it is, if it can be told: "an Obsidian vault", "a Rust project".
    pub what: Option<&'static str>,
    /// How many things are in it, not counting what is in its folders.
    pub holds: usize,
}

/// What a folder is, from what is found in it.
fn what_folder(dir: &Path) -> Option<&'static str> {
    let has = |name: &str| dir.join(name).exists();
    if has(".obsidian") {
        Some("an Obsidian vault")
    } else if has("Cargo.toml") {
        Some("a Rust project")
    } else if has("package.json") {
        Some("a JavaScript project")
    } else if has("pyproject.toml") || has("requirements.txt") {
        Some("a Python project")
    } else if has("go.mod") {
        Some("a Go project")
    } else if has(".git") {
        Some("a git repository")
    } else {
        None
    }
}

/// How far below a folder that is read plain folders are remembered.
/// One that is something in particular is remembered however deep.
const PLAIN_DEPTH: usize = 2;
/// And how deep anything is looked for at all.
const DEEPEST: usize = 6;

/// The folders under `roots` worth knowing of: those near the top, by
/// name, and those anywhere that are something in particular, such as a
/// vault of notes or a project. What is inside a project is the
/// project's business and is not gone into.
pub fn places(roots: &[PathBuf]) -> Vec<Place> {
    fn into(dir: &Path, depth: usize, out: &mut Vec<Place>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let (path, name) = (entry.path(), entry.file_name().to_string_lossy().into_owned());
            if !entry.file_type().is_ok_and(|t| t.is_dir()) || passed_over(&name) {
                continue;
            }
            let what = what_folder(&path);
            if what.is_some() || depth < PLAIN_DEPTH {
                let holds = std::fs::read_dir(&path).map(|d| d.flatten().filter(|e| !e.file_name().to_string_lossy().starts_with('.')).count()).unwrap_or(0);
                out.push(Place { path: path.clone(), what, holds });
            }
            // A vault of notes has folders of its own worth knowing; a project's are its workings.
            if depth + 1 < DEEPEST && what.is_none_or(|w| w == "an Obsidian vault") {
                into(&path, depth + 1, out);
            }
        }
    }
    let mut out = vec![];
    for root in roots {
        into(root, 0, &mut out);
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out.dedup_by(|a, b| a.path == b.path);
    out
}

/// A path with the home folder written as `~`.
fn from_home(path: &Path) -> String {
    match path.strip_prefix(neo_desktop::fs::home_dir()) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// Remembers the folders worth knowing of, so that "where is my vault"
/// has an answer: one memory each, of its name, what it is and where.
/// Those already remembered as they are now are left alone.
pub fn list_places(store: &mut Store, model: &dyn Model, roots: &[PathBuf]) -> Result<usize, String> {
    let mut listed = 0;
    for place in places(roots) {
        let stamp = format!("folder-{}-{}", place.what.unwrap_or("plain"), place.holds);
        if store.file_stamp(&place.path)?.as_deref() == Some(stamp.as_str()) {
            continue;
        }
        let title = name(&place.path);
        let within = place.path.parent().map(from_home).unwrap_or_default();
        let text = match place.what {
            Some(what) => format!("A folder that is {what}, called {title}, in {within}. It holds {} things.", place.holds),
            None => format!("A folder called {title}, in {within}. It holds {} things.", place.holds),
        };
        let about = words::clean_all(words::terms(&format!("{title} {}", place.what.unwrap_or_default())), 6);
        let created = std::fs::metadata(&place.path).map(|m| modified(&m)).unwrap_or(0);
        store.forget_source(&place.path)?;
        remember(store, model, &New { kind: Kind::Folder, source: Some(&place.path), title: &title, text: &text, part: 0, created, words: &about })?;
        store.set_file(&place.path, &stamp, 1)?;
        listed += 1;
    }
    Ok(listed)
}

/// A number with its thousands marked off.
fn counted(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, d) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(d);
    }
    out
}

/// Remembers the folders that hold a program or a game, each as one
/// thing: what it is called, where it is, how much is in it and of what
/// kinds. Its files are not read one by one, so this is what there is to
/// answer from when one is asked after. Those already remembered as they
/// are now are left alone. Returns how many were remembered.
pub fn list_programs(store: &mut Store, model: &dyn Model, programs: &[crate::survey::Program]) -> Result<usize, String> {
    let mut listed = 0;
    for program in programs {
        let stamp = format!("program-{}-{}", program.files, program.bytes >> 20);
        if store.file_stamp(&program.path)?.as_deref() == Some(stamp.as_str()) {
            continue;
        }
        let title = name(&program.path);
        let within = program.path.parent().map(from_home).unwrap_or_default();
        let kinds: Vec<String> = [(program.sounds, "sound files"), (program.pictures, "pictures"), (program.videos, "videos"), (program.documents, "documents")].into_iter().filter(|(n, _)| *n > 0).map(|(n, what)| format!("{} {what}", counted(n))).collect();
        let among = if kinds.is_empty() { String::new() } else { format!(", among them {}", kinds.join(", ")) };
        let (is, hint) = match program.what {
            crate::survey::Whole::Program => ("A folder that holds a program or a game", "program game"),
            crate::survey::Whole::Sounds => ("A folder that is a heap of sound files, as a game's sounds or recovered files are kept", "sounds"),
        };
        let text = format!("{is}, called {title}, in {within}. There are {} files in it, {}{among}. It is known of as a whole: its files are not read one by one, unless it is chosen in Sources as a folder to read.", counted(program.files), neo_desktop::fs::human_size(program.bytes));
        let about = words::clean_all(words::terms(&format!("{title} {hint}")), 6);
        let created = std::fs::metadata(&program.path).map(|m| modified(&m)).unwrap_or(0);
        store.forget_source(&program.path)?;
        remember(store, model, &New { kind: Kind::Folder, source: Some(&program.path), title: &title, text: &text, part: 0, created, words: &about })?;
        store.set_file(&program.path, &stamp, 1)?;
        listed += 1;
    }
    Ok(listed)
}

/// What a listed-only file's stamp ends with. It was "-l" before a
/// listing said what the file says of itself; one listed then is listed
/// again, and is not taken for one that has been looked at.
const LISTED: &str = "-l2";
const LISTED_BEFORE: &str = "-l";

/// Lists a photo or a video without looking at it: a memory of its name,
/// its folder and its date, which is enough to find it by those and to
/// count it among the photos from last month. It costs one embedding,
/// where looking costs a model of several gigabytes some seconds; so a
/// folder of thousands is listed in the time a dozen would be looked at.
/// Looking comes later: when something asks after it, or for everything
/// at once if that was chosen.
pub fn list_file(store: &mut Store, model: &dyn Model, path: &Path) -> Result<Outcome, String> {
    if !matches!(classify(path), Some(What::Photo | What::Video | What::Audio)) {
        return index_file(store, model, path);
    }
    let (taken, known) = make(model, &Embedded::default(), path, false)?;
    keep(store, model, &known, path, taken)
}

/// What a photo, a video or a recording is listed as, before it has been
/// looked at or listened to: its kind, what is said of it, the words it is
/// about, and what of it is embedded.
fn listed_as(path: &Path) -> (Kind, String, Vec<String>, String) {
    let kind = match classify(path) {
        Some(What::Photo) => Kind::Photo,
        Some(What::Video) => Kind::Video,
        _ => Kind::Audio,
    };
    let (title, folder) = (name(path), path.parent().and_then(Path::file_name).map(|f| f.to_string_lossy().into_owned()).unwrap_or_default());
    let text = match kind {
        Kind::Audio => format!("An audio recording that has not been listened to yet, called {title}, in the folder {folder}."),
        Kind::Photo => format!("A photo that has not been looked at yet, called {title}, in the folder {folder}."),
        _ => format!("A video that has not been looked at yet, called {title}, in the folder {folder}."),
    };
    // And what the file says of itself, which is read from its first
    // bytes and asks nothing of a model: how large a picture is and when
    // and with what it was taken, how long a recording runs and what it
    // is called.
    let (itself, named) = says_of_itself(path, kind);
    let text = if itself.is_empty() { text } else { format!("{text} {itself}") };
    // What its name and folder say of it: "Neo Recording", "Trips".
    let about = words::clean_all(words::terms(&format!("{} {folder} {named}", path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default())), 8);
    let said = format!("{title}\n{text}");
    (kind, text, about, said)
}

/// What a photo, a video or a recording says of itself in its own
/// header, as a sentence or two, and the names in it worth finding it by
/// (a camera, a title, an artist). Nothing where it says nothing.
fn says_of_itself(path: &Path, kind: Kind) -> (String, String) {
    if kind == Kind::Photo {
        let mut said = String::new();
        if let Ok((w, h)) = image::image_dimensions(path) {
            said = format!("It is {w} by {h} pixels.");
        }
        let written = std::fs::File::open(path).ok().and_then(|f| exif::Reader::new().read_from_container(&mut std::io::BufReader::new(f)).ok());
        let field = |tag| written.as_ref().and_then(|e| e.get_field(tag, exif::In::PRIMARY)).map(|f| f.display_value().to_string().trim_matches('"').trim().to_owned()).filter(|v| !v.is_empty());
        let camera = [field(exif::Tag::Make), field(exif::Tag::Model)].into_iter().flatten().collect::<Vec<_>>().join(" ");
        match (field(exif::Tag::DateTimeOriginal), camera.is_empty()) {
            (Some(when), false) => said.push_str(&format!(" It was taken on {when} with a {camera}.")),
            (Some(when), true) => said.push_str(&format!(" It was taken on {when}.")),
            (None, false) => said.push_str(&format!(" It was taken with a {camera}.")),
            (None, true) => {}
        }
        return (said.trim().to_owned(), camera);
    }
    // A recording or a video: its length and what it is tagged as, from ffprobe in one asking.
    let Some(out) = have("ffprobe").then(|| run_tool("ffprobe", &["-v".as_ref(), "error".as_ref(), "-show_entries".as_ref(), "format=duration:format_tags=title,artist,album".as_ref(), "-of".as_ref(), "default=noprint_wrappers=1".as_ref(), path.as_os_str()])).flatten() else {
        return Default::default();
    };
    let out = String::from_utf8_lossy(&out).into_owned();
    let value = |key: &str| out.lines().find_map(|l| l.strip_prefix(key)).map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
    let mut said = String::new();
    if let Some(seconds) = value("duration=").and_then(|d| d.parse::<f64>().ok()).filter(|d| d.is_finite() && *d > 0.0) {
        said = format!("It is {} long.", clock(seconds));
    }
    let (title, artist, album) = (value("TAG:title="), value("TAG:artist="), value("TAG:album="));
    if let Some(title) = &title {
        said.push_str(&match &artist {
            Some(artist) => format!(" It is called {title}, by {artist}."),
            None => format!(" It is called {title}."),
        });
    }
    if let Some(album) = &album {
        said.push_str(&format!(" It is from {album}."));
    }
    (said.trim().to_owned(), [title, artist, album].into_iter().flatten().collect::<Vec<_>>().join(" "))
}

/// What a photo, a video or a recording is listed as, with the embedding
/// that finds it.
fn listing(model: &dyn Model, path: &Path) -> Result<Taken, String> {
    let (kind, text, about, said) = listed_as(path);
    let embedding = model.embed(&[said], false)?.pop().ok_or("no embedding")?;
    Ok(Taken::Listed { kind, text, about, embedding })
}

/// The words that have their embeddings already, so far as the readers
/// know: one of these is not asked of the model again for every file it
/// turns up in, which for names and folders is most of them.
type Embedded = std::sync::RwLock<std::collections::HashSet<String>>;

/// What is made of several files together. Documents and listings want
/// nothing of the model but embeddings, and a request to it costs much
/// the same whether it carries one text or sixty: so what all of them
/// want goes in as few requests as will hold it, where a request to each
/// file kept a drive of hundreds of thousands to some fifteen a second.
/// Anything else, a picture to be looked at, is made on its own.
fn many(model: &dyn Model, embedded: &Embedded, jobs: &[(&Path, bool)]) -> Vec<Result<(Taken, Known), String>> {
    enum Draft {
        Listed(Kind, String, Vec<String>, String),
        Document { passages: Vec<String>, said: Vec<String>, about: Vec<Vec<String>>, secret: Option<&'static str>, skimmed: bool },
        Needs(&'static str),
        Alone,
    }
    let drafts: Vec<Result<Draft, String>> = jobs
        .iter()
        .map(|(path, look)| match classify(path) {
            Some(What::Document) => {
                let text = match text_of(path)? {
                    Ok(text) => text,
                    Err(tool) => return Ok(Draft::Needs(tool)),
                };
                // A key or a token lying in it is left out of what is remembered.
                let (text, secret) = crate::warden::without_secrets(&text);
                let (title, mut passages) = (name(path), passages(&text));
                // Skimmed, only how it opens is taken: one passage, where a long one has sixty.
                let skimmed = !*look && passages.len() > 1;
                if skimmed {
                    passages.truncate(1);
                }
                Ok(Draft::Document { said: passages.iter().map(|p| format!("{title}\n{p}")).collect(), about: passages.iter().map(|p| words::keywords(p, 5)).collect(), passages, secret, skimmed })
            }
            Some(_) if !*look => {
                let (kind, text, about, said) = listed_as(path);
                Ok(Draft::Listed(kind, text, about, said))
            }
            _ => Ok(Draft::Alone),
        })
        .collect();
    // Everything they want embedded, each thing once, and no word that is known already.
    let mut wanted: Vec<String> = vec![];
    if let Ok(have) = embedded.read() {
        let mut word = |w: &String| (!have.contains(w)).then(|| w.clone());
        for draft in drafts.iter().flatten() {
            match draft {
                Draft::Listed(_, _, about, said) => wanted.extend(std::iter::once(said.clone()).chain(about.iter().filter_map(&mut word))),
                Draft::Document { said, about, .. } => wanted.extend(said.iter().cloned().chain(about.iter().flatten().filter_map(&mut word))),
                Draft::Needs(_) | Draft::Alone => {}
            }
        }
    }
    wanted.sort_unstable();
    wanted.dedup();
    let mut made = Known::new();
    for group in wanted.chunks(AT_ONCE) {
        // What cannot be made together is asked for file by file below.
        let Ok(vectors) = model.embed(group, false) else { break };
        made.extend(group.iter().cloned().zip(vectors));
    }
    let of = |words: &[String]| -> Known { words.iter().filter_map(|w| made.get(w).map(|v| (w.clone(), v.clone()))).collect() };
    drafts
        .into_iter()
        .zip(jobs)
        .map(|(draft, (path, look))| match draft? {
            Draft::Needs(tool) => Ok((Taken::Needs(tool), Known::new())),
            Draft::Listed(kind, text, about, said) => match made.get(&said) {
                Some(embedding) => Ok((Taken::Listed { kind, text, embedding: embedding.clone(), about: about.clone() }, of(&about))),
                None => make(model, embedded, path, *look),
            },
            Draft::Document { passages, said, about, secret, skimmed } => {
                if !said.iter().all(|s| made.contains_key(s)) {
                    return make(model, embedded, path, *look);
                }
                let known = of(&about.concat());
                Ok((Taken::Document { parts: passages.into_iter().zip(said.iter().map(|s| made[s].clone())).collect(), secret, skimmed }, known))
            }
            Draft::Alone => make(model, embedded, path, *look),
        })
        .collect()
}

/// How many texts go to the model in one request. More than this at
/// once has been seen to bring Ollama's embedding runner down.
const AT_ONCE: usize = 64;
/// Remembers more of what is said in a video or a recording, a passage
/// to a memory, numbered from `from`. Returns how many it made.
fn remember_said(store: &mut Store, model: &dyn Model, known: &Known, of: &Recording, from: u32, passages: &[String]) -> Result<u32, String> {
    for (i, passage) in passages.iter().enumerate() {
        let text = format!("Said further on: {passage}");
        let about = words::keywords(passage, 5);
        remember_known(store, model, known, &New { kind: of.kind, source: Some(of.path), title: of.title, text: &text, part: from + i as u32, created: of.created, words: &about })?;
    }
    Ok(passages.len() as u32)
}

/// The video or recording that passages of speech are of.
struct Recording<'a> {
    kind: Kind,
    path: &'a Path,
    title: &'a str,
    created: u64,
}

/// Listens to the rest of the recordings that go on past their first
/// stretch, a stretch at a time, `runners` of them at once, until there
/// is no more or `report` says to stop. Listening is the slow part and
/// needs nothing from the memory, so each runner does only that; what
/// they heard is then remembered here, one after another.
pub fn hear_more(store: &mut Store, model: &dyn Model, runners: usize, report: &mut dyn FnMut(&Progress) -> bool) -> Progress {
    let mut p = Progress { listening: true, ..Progress::default() };
    loop {
        let waiting = store.unheard(runners.max(1)).unwrap_or_default();
        p.total = p.done + store.unheard_count().unwrap_or(0) as usize;
        p.now = waiting.first().map(|(path, _)| path.clone());
        if waiting.is_empty() || crate::hear::missing().is_some() || !report(&p) {
            break;
        }
        let stretch = crate::hear::stretch();
        let heard: Vec<(PathBuf, u32, Result<crate::hear::Heard, String>)> = std::thread::scope(|scope| {
            let listening: Vec<_> = waiting.iter().map(|(path, from)| scope.spawn(move || crate::hear::transcribe_from(path, *from))).collect();
            waiting.iter().zip(listening).map(|((path, from), runner)| (path.clone(), *from, runner.join().unwrap_or_else(|_| Err("The listener stopped.".into())))).collect()
        });
        for (path, from, heard) in heard {
            let kept = (|| -> Result<bool, String> {
                let heard = heard?;
                // Gone, or forgotten, since it was begun: nothing to add to.
                let Some((first, next)) = store.first_of(&path)? else { return Ok(false) };
                remember_said(store, model, &Known::default(), &Recording { kind: first.kind, path: &path, title: &first.title, created: first.created }, next, &passages(&heard.text))?;
                Ok(heard.more(stretch))
            })();
            match kept {
                Ok(true) => drop(store.set_unheard(&path, from + stretch)),
                Ok(false) => {
                    let _ = store.all_heard(&path);
                    p.done += 1;
                }
                // One that cannot be listened to further is left at what was heard.
                Err(e) => {
                    let _ = store.all_heard(&path);
                    p.done += 1;
                    p.failed += 1;
                    p.trouble = Some(format!("{}: {e}", name(&path)));
                }
            }
            p.read += 1;
        }
    }
    p.now = None;
    report(&p);
    p
}

/// What the slow part of reading a file made of it: all that a model, a
/// tool or a listener has to say. Making it needs nothing of the memory,
/// so several can be made at once; [`keep`] then puts each in the memory,
/// one after another.
enum Taken {
    /// A photo, a video or a recording that is only listed for now.
    Listed { kind: Kind, text: String, about: Vec<String>, embedding: Vec<f32> },
    Photo(Seen),
    /// What is seen in a video with the start of what is said in it; the
    /// passages said after that; and whether it goes on past the first stretch.
    Video { all: Seen, said: Vec<String>, more: bool },
    Audio { text: String, about: Vec<String>, said: Vec<String>, more: bool },
    /// A document's passages, each with its embedding, and what looks like
    /// a key or a token in it, if anything does. Skimmed, it is its
    /// opening only, and the rest is still to be read.
    Document { parts: Vec<(String, Vec<f32>)>, secret: Option<&'static str>, skimmed: bool },
    /// A program that would have to be installed to read it.
    Needs(&'static str),
}

/// The slow part of reading a file. With `look`, a photo or a video is
/// looked at and a recording listened to; without, they are only listed.
fn take_in(model: &dyn Model, path: &Path, look: bool) -> Result<Taken, String> {
    let title = name(path);
    let what = classify(path).ok_or("Not a kind of file Apollo reads.")?;
    if !look && what != What::Document {
        return listing(model, path);
    }
    // A document not being looked into is skimmed: see `many`, which is
    // where that is done when there are several, and does the same.
    if !look {
        return many(model, &Embedded::default(), &[(path, false)]).pop().unwrap_or(Err("Nothing was made of it.".into())).map(|(taken, _)| taken);
    }
    Ok(match what {
        What::Photo => Taken::Photo(model.describe(&look_at(path)?)?),
        What::Video => {
            if !have("ffmpeg") || !have("ffprobe") {
                return Ok(Taken::Needs("ffmpeg"));
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
            let mut all = video_text(seconds, &seen);
            // And what is said in it, where there is hearing: the start of
            // it with what is seen, and the rest in passages after.
            // The first stretch now; the rest, if it goes on, is listened to behind.
            let heard = if crate::hear::missing().is_none() { crate::hear::transcribe(path).unwrap_or_default() } else { crate::hear::Heard::default() };
            let mut said = passages(&heard.text);
            if let Some(first) = said.first() {
                all.text.push_str(&format!(" What is said: {first}"));
                all.words = words::clean_all(all.words.iter().cloned().chain(words::keywords(&said.join(" "), 4)), 12);
                said.remove(0);
            }
            Taken::Video { all, said, more: heard.more(crate::hear::stretch()) }
        }
        What::Audio => {
            match crate::hear::missing() {
                Some(crate::hear::Missing::Tool) => return Ok(Taken::Needs(crate::hear::TOOL)),
                Some(crate::hear::Missing::Ffmpeg) => return Ok(Taken::Needs("ffmpeg")),
                Some(crate::hear::Missing::Model) => return Ok(Taken::Needs("whisper-model")),
                None => {}
            }
            let heard = crate::hear::transcribe(path)?;
            let mut said = passages(&heard.text);
            let long = duration(path).map_or(String::new(), |d| format!(" {} long", clock(d)));
            let text = match said.first() {
                Some(first) => format!("An audio recording{long}. What is said: {first}"),
                None => format!("An audio recording{long}, in which no speech could be made out."),
            };
            let about = words::keywords(&said.join(" "), 6);
            if !said.is_empty() {
                said.remove(0);
            }
            Taken::Audio { text, about, said, more: heard.more(crate::hear::stretch()) }
        }
        What::Document => {
            let text = match text_of(path)? {
                Ok(text) => text,
                Err(tool) => return Ok(Taken::Needs(tool)),
            };
            // A key or a token lying in it is left out of what is remembered.
            let (text, secret) = crate::warden::without_secrets(&text);
            let passages = passages(&text);
            let mut parts = Vec::with_capacity(passages.len());
            // Embedded a few at a time: one request for each would crawl.
            for group in passages.chunks(16) {
                let said: Vec<String> = group.iter().map(|p| format!("{title}\n{p}")).collect();
                parts.extend(group.iter().cloned().zip(model.embed(&said, false)?));
            }
            Taken::Document { parts, secret, skimmed: false }
        }
    })
}

/// What is made of a file, with the embeddings that keeping it will ask
/// for: of what is to be remembered, and of the words it is about. Both
/// are the slow part, and done here they are done several files at once.
fn make(model: &dyn Model, embedded: &Embedded, path: &Path, look: bool) -> Result<(Taken, Known), String> {
    let taken = take_in(model, path, look)?;
    let title = name(path);
    let said = |text: &str| format!("{title}\n{text}");
    let further = |passages: &[String]| -> Vec<String> { passages.iter().flat_map(|p| std::iter::once(said(&format!("Said further on: {p}"))).chain(words::keywords(p, 5))).collect() };
    let mut wanted: Vec<String> = match &taken {
        Taken::Needs(_) => vec![],
        Taken::Listed { about, .. } => about.clone(),
        Taken::Photo(seen) => std::iter::once(said(&seen.text)).chain(seen.words.iter().cloned()).collect(),
        Taken::Video { all, said: after, .. } => std::iter::once(said(&all.text)).chain(all.words.iter().cloned()).chain(further(after)).collect(),
        Taken::Audio { text, about, said: after, .. } => std::iter::once(said(text)).chain(about.iter().cloned()).chain(further(after)).collect(),
        Taken::Document { parts, .. } => parts.iter().flat_map(|(p, _)| words::keywords(p, 5)).collect(),
    };
    // Not the words that have theirs already.
    if let Ok(have) = embedded.read() {
        wanted.retain(|w| !have.contains(w));
    }
    wanted.sort_unstable();
    wanted.dedup();
    let mut known = Known::new();
    for group in wanted.chunks(AT_ONCE) {
        // One that cannot be made now is asked for again when it is wanted.
        let Ok(made) = model.embed(group, false) else { break };
        known.extend(group.iter().cloned().zip(made));
    }
    Ok((taken, known))
}

/// Puts what was made of a file into the memory, in place of whatever was
/// remembered of it before.
fn keep(store: &mut Store, model: &dyn Model, known: &Known, path: &Path, taken: Taken) -> Result<Outcome, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    let (title, created) = (name(path), modified(&meta));
    let mut secret = None;
    let made = match taken {
        Taken::Needs(tool) => return Ok(Outcome::Needs(tool)),
        Taken::Listed { kind, text, about, embedding } => {
            store.forget_source(path)?;
            let id = store.add(&New { kind, source: Some(path), title: &title, text: &text, part: 0, created, words: &about }, &embedding)?;
            learn_words(store, model, known, &about)?;
            store.mark_light(id)?;
            store.set_file(path, &format!("{}{LISTED}", stamp(&meta)), 1)?;
            return Ok(Outcome::Read(1));
        }
        Taken::Photo(seen) => {
            store.forget_source(path)?;
            remember_known(store, model, known, &New { kind: Kind::Photo, source: Some(path), title: &title, text: &seen.text, part: 0, created, words: &seen.words })?;
            1
        }
        Taken::Video { all, said, more } => {
            store.forget_source(path)?;
            remember_known(store, model, known, &New { kind: Kind::Video, source: Some(path), title: &title, text: &all.text, part: 0, created, words: &all.words })?;
            let made = remember_said(store, model, known, &Recording { kind: Kind::Video, path, title: &title, created }, 1, &said)?;
            if more {
                store.set_unheard(path, crate::hear::stretch())?;
            }
            made + 1
        }
        Taken::Audio { text, about, said, more } => {
            store.forget_source(path)?;
            remember_known(store, model, known, &New { kind: Kind::Audio, source: Some(path), title: &title, text: &text, part: 0, created, words: &about })?;
            let made = remember_said(store, model, known, &Recording { kind: Kind::Audio, path, title: &title, created }, 1, &said)?;
            if more {
                store.set_unheard(path, crate::hear::stretch())?;
            }
            made + 1
        }
        Taken::Document { parts, secret: found, skimmed } => {
            secret = found;
            store.forget_source(path)?;
            for (i, (passage, embedding)) in parts.iter().enumerate() {
                let about = words::keywords(passage, 5);
                store.add(&New { kind: Kind::Document, source: Some(path), title: &title, text: passage, part: i as u32, created, words: &about }, embedding)?;
                learn_words(store, model, known, &about)?;
            }
            // Its opening is known and the rest is not: marked as a file
            // only listed is, so that it is read in full when its turn comes.
            if skimmed {
                store.set_file(path, &format!("{}{LISTED}", stamp(&meta)), parts.len() as u32)?;
                return Ok(match secret {
                    Some(what) => Outcome::Holds(parts.len() as u32, what),
                    None => Outcome::Read(parts.len() as u32),
                });
            }
            parts.len() as u32
        }
    };
    store.set_file(path, &stamp_of(path, &meta), made)?;
    Ok(match secret {
        Some(what) => Outcome::Holds(made, what),
        None => Outcome::Read(made),
    })
}

/// Reads one file into the memory, in place of whatever was remembered of
/// it before.
pub fn index_file(store: &mut Store, model: &dyn Model, path: &Path) -> Result<Outcome, String> {
    let (taken, known) = make(model, &Embedded::default(), path, true)?;
    keep(store, model, &known, path, taken)
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
    /// Documents whose opening was read this run, the rest of them still to come.
    pub skimmed: usize,
    pub failed: usize,
    /// Files forgotten because they are gone, or their folder is no longer read.
    pub forgotten: usize,
    /// Programs that would let more be read, if they were installed.
    pub needs: Vec<&'static str>,
    /// The last thing to go wrong, and with what.
    pub trouble: Option<String>,
    /// Files read this run that hold what looks like a key or a token, and
    /// what kind.
    pub secrets: Vec<(PathBuf, &'static str)>,
    /// The rest of long recordings is being listened to: `done` and
    /// `total` are then of recordings, and `read` of stretches heard.
    pub listening: bool,
    /// Pictures and videos are being looked at, which is the slow part,
    /// and not merely listed.
    pub looking: bool,
    /// Pictures and videos are being taken in by sight, for telling
    /// places and people apart: `done` and `total` are then of those.
    pub sighting: bool,
}

/// Forgets files that are gone, or are not under any of `roots` any more.
pub fn sweep(store: &mut Store, roots: &[PathBuf]) -> Result<usize, String> {
    let mut gone = 0;
    for file in store.files()? {
        // (A folder that is remembered is among them, which is why not `is_file`.)
        if !file.exists() || !roots.iter().any(|r| file.starts_with(r)) {
            store.forget_source(&file)?;
            gone += 1;
        }
    }
    if gone > 0 {
        store.tidy_words()?;
    }
    Ok(gone)
}

/// How many questions are being answered just now.
static ASKING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Held while a question is being answered: the readers take nothing
/// more to the model until it is let go, so that what someone is waiting
/// for does not stand in a queue behind what nobody is.
pub struct Asking(());

/// Whether a question is being answered just now.
pub(crate) fn being_asked() -> bool {
    ASKING.load(std::sync::atomic::Ordering::Relaxed) > 0
}

/// Says a question is being answered, for as long as what is returned is kept.
pub fn asking() -> Asking {
    ASKING.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    Asking(())
}

impl Drop for Asking {
    fn drop(&mut self) {
        ASKING.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Where the indexes of disks are kept, once it has been said.
static INDEXES: std::sync::RwLock<Option<PathBuf>> = std::sync::RwLock::new(None);

/// Has [`run`] find what there is to read in the indexes of disks kept
/// in `dir`, making and keeping any that are not there. The app says
/// this once as it starts; a test does not, and its folders are looked
/// through as they are, with nothing written outside them.
pub fn use_indexes(dir: PathBuf) {
    if let Ok(mut at) = INDEXES.write() {
        *at = Some(dir);
    }
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
    // What lets a search by meaning be answered in a moment however much
    // is remembered: made here the first time there is enough to want it,
    // which takes a minute for a million memories, and kept up after that
    // as each memory is added.
    if let Err(e) = store.index_meanings(&mut |_, _| report(&p)) {
        p.trouble = Some(e);
    }
    // The folders themselves, which is quick: a memory each of the few worth knowing.
    if let Err(e) = list_places(store, model, roots) {
        p.trouble = Some(e);
    }
    // What there is to read: from the kept index of each disk, which
    // also says which folders are a program's and are to be known of
    // whole. Where no place for indexes has been named, the folders are
    // looked through themselves and nothing is kept.
    let files = match INDEXES.read().ok().and_then(|dir| dir.clone()) {
        Some(cache) => {
            let found = crate::survey::survey(roots, &cache);
            if let Err(e) = list_programs(store, model, &found.programs) {
                p.trouble = Some(e);
            }
            found.files
        }
        None => walk(roots),
    };
    let mut stopped = false;
    let p = read_files(store, model, files.clone(), p, false, &mut |p| {
        stopped = !report(p);
        !stopped
    });
    if stopped {
        return p;
    }
    // Then every picture and video by how it looks, which is quick beside
    // describing them and is what a pasted picture is held against.
    let p = crate::sight::pass(store, &files, p, &mut |p| {
        stopped = !report(p);
        !stopped
    });
    if stopped {
        return p;
    }
    // That was the quick pass over everything: every document's opening,
    // and every picture, video and recording by what it says of itself.
    // Now the documents in full, which is a matter of embeddings.
    let (documents, waiting): (Vec<PathBuf>, Vec<PathBuf>) = files.into_iter().partition(|f| classify(f) == Some(What::Document));
    let p = read_files(store, model, documents.clone(), Progress { done: p.total.saturating_sub(documents.len()), looking: false, ..p }, true, &mut |p| {
        stopped = !report(&Progress { looking: false, ..p.clone() });
        !stopped
    });
    if !look || stopped {
        return Progress { looking: false, ..p };
    }
    // Then the slow part: looking at the pictures and listening.
    read_files(store, model, waiting.clone(), Progress { done: p.total.saturating_sub(waiting.len()), ..p }, true, report)
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

/// How many files of each kind are read at once. Each kind waits on
/// something of its own, so the kinds do not wait on each other: text
/// and listings on the model that embeds, pictures and videos on the one
/// that sees, sound on the listener. A video is several pictures and a
/// stretch of sound, so they are taken one at a time.
const LANES: [(Lane, usize); 4] = [(Lane::Words, 6), (Lane::Pictures, 2), (Lane::Videos, 1), (Lane::Sound, 2)];

/// How many files one reader takes at a time and asks of the model
/// together: of those only being listed, and of documents.
const LISTED_TOGETHER: usize = 24;
const DOCUMENTS_TOGETHER: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lane {
    /// Documents, and whatever is only being listed.
    Words,
    Pictures,
    Videos,
    Sound,
}

/// Puts things in the order they are to be read: round by round, each
/// round taking something from every folder. The first takes one from
/// each, its newest; the next two more, then four, and so on doubling,
/// so that a folder of ten thousand does not hold up a folder of ten,
/// and neither waits for the other to be finished. Folders are visited
/// with the one touched most lately first. `of` says which folder a
/// thing is in and when it was last changed.
pub(crate) fn spread<T>(things: Vec<T>, of: impl Fn(&T) -> (PathBuf, u64)) -> Vec<T> {
    let total = things.len();
    let mut folders: std::collections::HashMap<PathBuf, Vec<(u64, T)>> = std::collections::HashMap::new();
    for thing in things {
        let (folder, changed) = of(&thing);
        folders.entry(folder).or_default().push((changed, thing));
    }
    // Each folder's own, oldest first, so that the newest is taken from the end.
    let mut folders: Vec<(PathBuf, Vec<(u64, T)>)> = folders.into_iter().collect();
    for (_, own) in &mut folders {
        own.sort_by_key(|(changed, _)| *changed);
    }
    let newest = |own: &Vec<(u64, T)>| own.last().map_or(0, |(changed, _)| *changed);
    folders.sort_by(|a, b| newest(&b.1).cmp(&newest(&a.1)).then_with(|| a.0.cmp(&b.0)));
    let mut out = Vec::with_capacity(total);
    let mut take = 1;
    while out.len() < total {
        for (_, own) in &mut folders {
            for _ in 0..take.min(own.len()) {
                out.extend(own.pop().map(|(_, thing)| thing));
            }
        }
        folders.retain(|(_, own)| !own.is_empty());
        take = (take * 2).min(64);
    }
    out
}

/// A file waiting to be read, with what was known of it.
struct Waiting {
    file: PathBuf,
    meta: std::fs::Metadata,
    had: Option<String>,
    look: bool,
    seen: bool,
}

fn read_files(store: &mut Store, model: &dyn Model, files: Vec<PathBuf>, mut p: Progress, look: bool, report: &mut dyn FnMut(&Progress) -> bool) -> Progress {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Mutex, mpsc};

    p.total = p.done + files.len();
    p.looking = look;
    // What there is to do is settled first, which is quick: most files
    // are as they were.
    let mut lanes: Vec<VecDeque<Waiting>> = LANES.iter().map(|_| VecDeque::new()).collect();
    let mut waiting = 0;
    for file in files {
        let Ok(meta) = std::fs::metadata(&file) else {
            p.done += 1;
            continue;
        };
        let had = store.file_stamp(&file).ok().flatten();
        let what = classify(&file);
        let seen = matches!(what, Some(What::Photo | What::Video | What::Audio));
        // Looked at already, or listed already and that is all that is asked.
        let (base, listed_stamp) = (stamp(&meta), format!("{}{LISTED}", stamp(&meta)));
        // (A document is listed when its opening has been skimmed.)
        let unchanged = had.as_deref() == Some(stamp_of(&file, &meta).as_str()) || (!look && had.as_deref() == Some(listed_stamp.as_str()));
        if unchanged {
            p.done += 1;
            continue;
        }
        // The file is as it was, and was looked at, but in an older way
        // (before pictures were described as they are now, or before there
        // was hearing). It is looked at again, not put back to being only
        // listed: what was known of it is not to be lost for being improved on.
        let listed_before = format!("{base}{LISTED_BEFORE}");
        let look = look || (seen && had.as_deref().is_some_and(|h| h.starts_with(&base) && h != listed_stamp && h != listed_before));
        let lane = match what {
            Some(What::Photo) if look => Lane::Pictures,
            Some(What::Video) if look => Lane::Videos,
            Some(What::Audio) if look => Lane::Sound,
            _ => Lane::Words,
        };
        lanes[LANES.iter().position(|(l, _)| *l == lane).unwrap_or(0)].push_back(Waiting { file, meta, had, look, seen });
        waiting += 1;
    }
    if waiting == 0 || !report(&p) {
        p.now = None;
        report(&p);
        return p;
    }
    // Not from one end of a disk to the other: a little of every folder
    // first, then more of each, so that what is known fills in evenly
    // and every folder has something said of it early. Documents keep
    // their place ahead of what is only being listed.
    for queue in lanes.iter_mut() {
        let (documents, rest): (Vec<Waiting>, Vec<Waiting>) = std::mem::take(queue).into_iter().partition(|w| !w.seen);
        let order = |of: Vec<Waiting>| spread(of, |w| (w.file.parent().map(Path::to_path_buf).unwrap_or_default(), modified(&w.meta)));
        *queue = order(documents).into_iter().chain(order(rest)).collect();
    }
    let lanes: Vec<Mutex<VecDeque<Waiting>>> = lanes.into_iter().map(Mutex::new).collect();
    let stop = AtomicBool::new(false);
    let embedded: Embedded = std::sync::RwLock::new(store.words_with_vectors().unwrap_or_default().into_iter().collect());
    let embedded = &embedded;
    // The readers make what they can of each file and hand it here, where
    // it is put in the memory: the memory is written by one hand only.
    std::thread::scope(|scope| {
        let (made, taken) = mpsc::sync_channel::<(Waiting, Result<(Taken, Known), String>)>(128);
        for (queue, (_, readers)) in lanes.iter().zip(LANES) {
            for _ in 0..readers {
                let (made, stop) = (made.clone(), &stop);
                scope.spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        // A question comes first: reading waits while one is answered.
                        if ASKING.load(Ordering::Relaxed) > 0 {
                            std::thread::sleep(std::time::Duration::from_millis(25));
                            continue;
                        }
                        // Documents, and files only being listed, are taken
                        // several at a time and made together; the rest one by one.
                        let lists = |w: &Waiting| w.seen && !w.look;
                        let mut next: Vec<Waiting> = {
                            let Ok(mut q) = queue.lock() else { break };
                            let many = match q.front() {
                                Some(first) if lists(first) => q.iter().take(LISTED_TOGETHER).take_while(|w| lists(w)).count(),
                                Some(first) if !first.seen => q.iter().take(DOCUMENTS_TOGETHER).take_while(|w| !w.seen).count(),
                                _ => 1,
                            };
                            let many = many.min(q.len());
                            q.drain(..many).collect()
                        };
                        if next.is_empty() {
                            break;
                        }
                        let taken = many(model, embedded, &next.iter().map(|w| (w.file.as_path(), w.look)).collect::<Vec<_>>());
                        let mut gone = false;
                        for pair in next.drain(..).zip(taken) {
                            gone |= made.send(pair).is_err();
                        }
                        if gone {
                            break;
                        }
                    }
                });
            }
        }
        drop(made);
        // What is kept is written down a good many files at a time, and at
        // least every so often, so that the disk is waited on rarely and
        // what the app shows of the memory is never far behind.
        // Nothing is left open while the readers are slow about the next
        // one: the app writes to the memory too, and would wait on it.
        let (mut unwritten, mut written) = (0, std::time::Instant::now());
        loop {
            let (was, taken) = match taken.recv_timeout(std::time::Duration::from_millis(500)) {
                Ok(next) => next,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let _ = store.commit();
                    (unwritten, written) = (0, std::time::Instant::now());
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            // Asked to stop: what is already made is let go, and the readers finish what they hold.
            if stop.load(Ordering::Relaxed) {
                continue;
            }
            let Waiting { file, meta, had, look, seen } = was;
            p.now = Some(file.clone());
            // Begun if it is not already: an error says that it is.
            let _ = store.begin();
            // The words this one brings embeddings for are known from here on.
            if let (Ok((_, known)), Ok(mut have)) = (&taken, embedded.write()) {
                have.extend(known.keys().filter(|k| !k.contains('\n')).cloned());
            }
            match taken.and_then(|(taken, known)| keep(store, model, &known, &file, taken)) {
                Ok(Outcome::Read(_)) if seen && !look => p.listed += 1,
                // A document's opening, with more of it to come; one short enough to be all opening is read.
                Ok(Outcome::Read(_)) if !look && store.file_stamp(&file).ok().flatten().is_some_and(|s| s.ends_with(LISTED)) => p.skimmed += 1,
                Ok(Outcome::Read(_)) => p.read += 1,
                Ok(Outcome::Holds(_, what)) => {
                    p.read += 1;
                    p.secrets.push((file.clone(), what));
                }
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
            p.done += 1;
            unwritten += 1;
            if unwritten >= 200 || written.elapsed() >= std::time::Duration::from_millis(500) {
                let _ = store.commit();
                (unwritten, written) = (0, std::time::Instant::now());
            }
            if !report(&p) {
                stop.store(true, Ordering::Relaxed);
            }
        }
        let _ = store.commit();
    });
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
    fn a_long_document_is_skimmed_first_and_read_in_full_after() {
        let dir = scratch("skim");
        let model = Fake::default();
        let mut store = Store::open(&dir.join("db/memory.db"), &[1; 32], model.dims().unwrap()).unwrap();
        let long: String = (0..12).map(|i| format!("Chapter {i} is about gardens and trains and the weather on the coast. {}\n\n", "More is said of it here at some length. ".repeat(12))).collect();
        std::fs::create_dir_all(dir.join("files")).unwrap();
        std::fs::write(dir.join("files/book.md"), &long).unwrap();
        std::fs::write(dir.join("files/note.md"), "A short note about a boat.").unwrap();
        let whole = passages(&long).len();
        assert!(whole > 3, "long enough to be worth skimming");
        // Skimmed: the opening of the long one, and all of the short one, which is all opening.
        // Spelt as the reading spells it, so that it is the same file to the memory.
        let book = dir.join("files").join("book.md");
        let mut made = many(&model, &Embedded::default(), &[(&book, false), (&dir.join("files").join("note.md"), false)]);
        let note = made.pop().unwrap().unwrap().0;
        assert!(matches!(note, Taken::Document { ref parts, skimmed: false, .. } if parts.len() == 1));
        let skim = made.pop().unwrap().unwrap().0;
        assert!(matches!(skim, Taken::Document { ref parts, skimmed: true, .. } if parts.len() == 1 && parts[0].0.starts_with("Chapter 0")));
        keep(&mut store, &model, &Known::default(), &book, skim).unwrap();
        assert_eq!(store.stats().unwrap().of(Kind::Document), 1, "one memory of it so far");
        assert!(store.file_stamp(&book).unwrap().unwrap().ends_with(LISTED), "and it is marked as having more to come");
        // A run goes over everything quickly and then reads the documents through.
        let mut seen = vec![];
        let p = run(&mut store, &model, &[dir.join("files")], false, &mut |p| {
            seen.push((p.skimmed, p.read));
            true
        });
        assert_eq!((p.read, p.skimmed, p.failed), (2, 0, 0), "the note in the quick pass, the book in full after; its skim was already there");
        assert_eq!(store.stats().unwrap().of(Kind::Document) as usize, whole + 1);
        assert!(!store.file_stamp(&book).unwrap().unwrap().ends_with(LISTED));
        // And again, nothing is left to do.
        let again = run(&mut store, &model, &[dir.join("files")], false, &mut |_| true);
        assert_eq!((again.read, again.skimmed), (0, 0));
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn reading_takes_a_little_of_every_folder_before_more_of_any() {
        // (folder, name, when changed): a big folder, a small one, and one with a single old thing.
        let mut things: Vec<(&str, String, u64)> = (0..10).map(|i| ("big", format!("b{i}"), 100 + i)).collect();
        things.extend((0..3).map(|i| ("small", format!("s{i}"), 500 + i)));
        things.push(("lone", "l0".to_owned(), 1));
        let order: Vec<String> = spread(things, |t| (PathBuf::from(t.0), t.2)).into_iter().map(|t| t.1).collect();
        // One from each, the newest of it, the folder touched most lately first; then two; then four.
        assert_eq!(&order[..3], ["s2", "b9", "l0"]);
        assert_eq!(&order[3..7], ["s1", "s0", "b8", "b7"]);
        assert_eq!(&order[7..11], ["b6", "b5", "b4", "b3"]);
        assert_eq!(order.len(), 14, "and all of it, each thing once");
        assert!(spread(Vec::<(&str, String, u64)>::new(), |t| (PathBuf::from(t.0), t.2)).is_empty());
    }

    #[test]
    fn a_web_page_is_read_for_its_words() {
        let page = "<!DOCTYPE html><html><head><title>Trip notes</title><style>p { color: red }</style><script>var x = '<p>not this</p>';</script></head><body><!-- nor this --><h1>The&nbsp;coast</h1><p>We walked <b>ten</b> miles &amp; saw seals.</p><ul><li>Bread</li><li>Cheese &lt;soft&gt;</li></ul></body></html>";
        assert_eq!(html_text(page), "Trip notes\n\nThe coast\n\nWe walked ten miles & saw seals.\n\nBread\n\nCheese <soft>");
        assert_eq!((html_text("plain words"), html_text("<p>unclosed"), html_text("a < b")), ("plain words".into(), "unclosed".into(), "a < b".into()), "and what is not a tag is left as it is");
    }

    #[test]
    fn only_changes_that_could_matter_are_heard() {
        for path in ["/home/me/Documents/notes.md", "/home/me/Pictures/trip", "/home/me/Pictures/trip/IMG_1.jpg"] {
            assert!(could_matter(Path::new(path)), "{path}");
        }
        for path in ["/home/me/.cache/x.md", "/home/me/code/node_modules/readme.md", "/home/me/app/build.log", "/home/me/db/memory.db-wal"] {
            assert!(!could_matter(Path::new(path)), "{path}");
        }
        if cfg!(windows) {
            assert!(!could_matter(Path::new(r"C:\Users\me\AppData\Roaming\Neo\apollo\notes.md")) && !could_matter(Path::new(r"C:\Windows\Temp\a.txt")));
        }
    }

    #[test]
    fn files_are_sorted_into_kinds_and_the_rest_left() {
        assert_eq!(classify(Path::new("/a/IMG_1.HEIC")), Some(What::Photo));
        assert_eq!(classify(Path::new("clip.MKV")), Some(What::Video));
        assert_eq!((classify(Path::new("notes.md")), classify(Path::new("paper.pdf")), classify(Path::new("letter.docx"))), (Some(What::Document), Some(What::Document), Some(What::Document)));
        assert_eq!((classify(Path::new("memo.M4A")), classify(Path::new("talk.mp3"))), (Some(What::Audio), Some(What::Audio)));
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
        assert_eq!((stats.of(Kind::Photo), stats.of(Kind::Document), stats.files - stats.of(Kind::Folder)), (2, 1, 3));
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
        assert_eq!((p.forgotten, store.stats().unwrap().total()), (3, 0), "its two files, and the folder in it that was remembered");
        // Windows will not remove a folder with an open file in it; the next run clears it away.
        let _ = std::fs::remove_dir_all(dir);
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
            assert!(neo_desktop::command(neo_desktop::fs::tool("ffmpeg")).args(["-v", "error", "-f", "lavfi", "-i", "color=c=0x2828DC:s=320x180:d=3", "-pix_fmt", "yuv420p"]).arg(&clip).status().unwrap().success());
            let (w, h, rgba) = thumbnail(&clip, 160).expect("a video has one");
            assert_eq!((w, h), (160, 90));
            assert!(rgba[2] > 150 && rgba[0] < 110, "a frame of the video itself: {:?}", &rgba[..4]);
        }
        // Windows will not remove a folder with an open file in it; the next run clears it away.
        let _ = std::fs::remove_dir_all(dir);
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
        let known = |store: &Store| store.stats().unwrap().total() - store.stats().unwrap().of(Kind::Folder);
        assert_eq!((known(&store), store.file_stamp(&root.join("unseen.txt")).unwrap()), (2, None));
        // A folder that appears is looked through; one that goes takes its memories with it.
        std::fs::create_dir_all(root.join("more")).unwrap();
        picture(&root.join("more/green.png"), [40, 220, 40]);
        std::fs::write(root.join("more/note.md"), "Hiking the mountain in the snow.").unwrap();
        assert_eq!(update(&mut store, &model, &roots, &[root.join("more")], true, &mut |_| true).read, 2);
        std::fs::remove_dir_all(root.join("more")).unwrap();
        std::fs::remove_file(root.join("trip/red.png")).unwrap();
        let p = update(&mut store, &model, &roots, &[root.join("more"), root.join("trip/red.png"), root.join("never-was.png")], true, &mut |_| true);
        assert_eq!((p.forgotten, p.read, known(&store)), (3, 0, 1));
        // A full look afterwards finds what no change told of.
        assert_eq!(run(&mut store, &model, &roots, true, &mut |_| true).read, 1);
        // Windows will not remove a folder with an open file in it; the next run clears it away.
        let _ = std::fs::remove_dir_all(dir);
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
        assert_eq!((dog.light, dog.text.as_str()), (true, "A photo that has not been looked at yet, called beach dog.png, in the folder Trips. It is 240 by 160 pixels."), "and what the file says of itself");
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
        // Windows will not remove a folder with an open file in it; the next run clears it away.
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_key_lying_in_a_document_is_left_out_of_the_memory_and_said() {
        let dir = scratch("secret");
        let model = Fake::default();
        let mut store = Store::open(&dir.join("db/memory.db"), &[1; 32], model.dims().unwrap()).unwrap();
        let root = dir.join("docs");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("server.txt"), "Notes on the server.\n\nexport AWS_KEY=AKIAIOSFODNN7EXAMPLE\n\nThe invoice is paid by the bank.").unwrap();
        std::fs::write(root.join("plain.txt"), "Nothing to hide here.").unwrap();
        let p = run(&mut store, &model, std::slice::from_ref(&root), false, &mut |_| true);
        assert_eq!((p.read, p.secrets.clone()), (2, vec![(root.join("server.txt"), "an Amazon access key")]));
        let kept = store.recent(5, Some(Kind::Document)).unwrap().into_iter().find(|m| m.title == "server.txt").unwrap();
        assert!(kept.text.contains("[an Amazon access key, left out]") && !kept.text.contains("AKIA") && kept.text.contains("The invoice is paid"), "{}", kept.text);
        // Not said again while the file stays as it is.
        assert_eq!(run(&mut store, &model, std::slice::from_ref(&root), false, &mut |_| true).secrets, vec![]);
        // Windows will not remove a folder with an open file in it; the next run clears it away.
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn folders_worth_knowing_of_are_remembered_by_what_and_where_they_are() {
        let dir = scratch("places");
        let model = Fake::default();
        let mut store = Store::open(&dir.join("db/memory.db"), &[1; 32], model.dims().unwrap()).unwrap();
        let root = dir.join("Documents");
        for sub in ["vaultobs/.obsidian", "vaultobs/Daily", "Invoices/2026", "code/tool/src", "code/tool/.git", "a/b/c/d", "a/b/deep-vault/.obsidian", ".hidden/x", "node_modules/y"] {
            std::fs::create_dir_all(root.join(sub)).unwrap();
        }
        std::fs::write(root.join("vaultobs/Welcome.md"), "Notes begin here.").unwrap();
        std::fs::write(root.join("code/tool/Cargo.toml"), "[package]").unwrap();
        let roots = vec![root.clone()];
        let found = places(&roots);
        let named = |name: &str| found.iter().find(|p| p.path.ends_with(name)).map(|p| p.what);
        assert_eq!(named("vaultobs"), Some(Some("an Obsidian vault")), "told by what is in it, not by its name");
        assert_eq!((named("tool"), named("Invoices"), named("2026"), named("Daily")), (Some(Some("a Rust project")), Some(None), Some(None), Some(None)));
        assert_eq!((named("deep-vault"), named("c"), named("src"), named(".hidden"), named("node_modules")), (Some(Some("an Obsidian vault")), None, None, None, None), "one that is something is known however deep; plain ones only near the top; a project's workings and hidden folders not at all");
        assert_eq!(found.iter().find(|p| p.path.ends_with("vaultobs")).unwrap().holds, 2);
        // Read, each is a memory that says what and where; asked after by what it is, it is found.
        run(&mut store, &model, &roots, false, &mut |_| true);
        let folders = store.recent(50, Some(Kind::Folder)).unwrap();
        let vault = folders.iter().find(|m| m.title == "vaultobs").unwrap();
        assert!(vault.text.starts_with("A folder that is an Obsidian vault, called vaultobs, in ") && vault.text.ends_with("/Documents. It holds 2 things."), "{}", vault.text);
        assert_eq!((vault.source.clone(), vault.light), (Some(root.join("vaultobs")), false));
        assert!(store.words_of(vault.id).unwrap().contains(&"obsidian".to_owned()));
        let asked = model.embed(&["where is my obsidian vault".to_owned()], true).unwrap().pop().unwrap();
        let first = &store.search_for(&asked, &words::terms("where is my obsidian vault"), 3, Some(Kind::Folder)).unwrap()[0];
        assert!(first.memory.text.contains("Obsidian vault"), "{}", first.memory.text);
        // Unchanged, they are not remembered again; one that goes is forgotten.
        let before = store.stats().unwrap().of(Kind::Folder);
        assert_eq!(list_places(&mut store, &model, &roots).unwrap(), 0);
        std::fs::remove_dir_all(root.join("Invoices")).unwrap();
        run(&mut store, &model, &roots, false, &mut |_| true);
        assert_eq!(store.stats().unwrap().of(Kind::Folder), before - 2);
        // Windows will not remove a folder with an open file in it; the next run clears it away.
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn what_is_said_in_a_video_and_in_a_recording_is_remembered() {
        // Needs the real hearing, and a voice to hear: macOS has one to hand.
        let voice = Path::new("/usr/bin/say");
        if crate::hear::missing().is_some() || !voice.is_file() || !have("ffprobe") {
            return;
        }
        let dir = scratch("heard");
        let spoken = dir.join("memo.aiff");
        assert!(neo_desktop::command(voice).arg("-o").arg(&spoken).arg("We take the ferry to the island on Friday morning. The ferry back leaves at four on Sunday.").status().unwrap().success());
        let clip = dir.join("trip.mp4");
        assert!(neo_desktop::command(neo_desktop::fs::tool("ffmpeg")).args(["-v", "error", "-f", "lavfi", "-i", "color=c=0x2828DC:s=160x120:d=7", "-i"]).arg(&spoken).args(["-shortest", "-pix_fmt", "yuv420p"]).arg(&clip).status().unwrap().success());
        let model = Fake::default();
        let mut store = Store::open(&dir.join("db/memory.db"), &[1; 32], model.dims().unwrap()).unwrap();
        // Listed first, as a photo is; then listened to.
        assert_eq!(list_file(&mut store, &model, &spoken), Ok(Outcome::Read(1)));
        let listed = &store.recent(1, Some(Kind::Audio)).unwrap()[0];
        assert!(listed.light && listed.text.starts_with("An audio recording that has not been listened to yet, called memo.aiff"));
        assert_eq!(index_file(&mut store, &model, &spoken), Ok(Outcome::Read(1)));
        let heard = &store.recent(1, Some(Kind::Audio)).unwrap()[0];
        assert!(!heard.light && heard.text.starts_with("An audio recording 0:0") && heard.text.to_lowercase().contains("what is said: we take the ferry to the island"), "{}", heard.text);
        assert!(store.words_of(heard.id).unwrap().contains(&"ferry".to_owned()));
        // A video is what is seen in it and what is said in it.
        assert_eq!(index_file(&mut store, &model, &clip), Ok(Outcome::Read(1)));
        let seen = &store.recent(1, Some(Kind::Video)).unwrap()[0];
        assert!(seen.text.starts_with("A video 0:0") && seen.text.contains("Waves on the sea.") && seen.text.to_lowercase().contains("what is said: we take the ferry"), "{}", seen.text);
        assert!(store.file_stamp(&clip).unwrap().unwrap().ends_with("-d2-h1") && store.file_stamp(&spoken).unwrap().unwrap().ends_with("-h1"));
        // A recording longer than a stretch: its first stretch is heard when it is read, with the
        // silences left out, and the rest behind, by runners, until all of it is in the memory.
        let long = dir.join("long.aiff");
        let part = |name: &str, words: &str| {
            let out = dir.join(name);
            assert!(neo_desktop::command(voice).arg("-o").arg(&out).arg(words).status().unwrap().success());
            out
        };
        let (a, b, c) = (part("a.aiff", "The ferry to the island leaves on Friday morning."), part("b.aiff", "We stay two nights at the harbour inn."), part("c.aiff", "On Saturday we walk the coast path to the lighthouse."));
        // Twelve seconds of nothing between each.
        let made = neo_desktop::command(neo_desktop::fs::tool("ffmpeg")).args(["-v", "error", "-i"]).arg(&a).args(["-f", "lavfi", "-t", "12", "-i", "anullsrc=r=22050:cl=mono", "-i"]).arg(&b).args(["-f", "lavfi", "-t", "12", "-i", "anullsrc=r=22050:cl=mono", "-i"]).arg(&c).args(["-filter_complex", "[0:a][1:a][2:a][3:a][4:a]concat=n=5:v=0:a=1"]).arg(&long).status().unwrap();
        assert!(made.success());
        for scrap in [a, b, c] {
            std::fs::remove_file(scrap).unwrap();
        }
        assert!(duration(&long).unwrap() > 30.0, "more than half of it silence");
        crate::hear::set_stretch(4);
        assert!(matches!(index_file(&mut store, &model, &long), Ok(Outcome::Read(_))));
        let first = store.first_of(&long).unwrap().unwrap().0;
        assert!(first.text.to_lowercase().contains("ferry") && !first.text.to_lowercase().contains("lighthouse"), "{}", first.text);
        assert_eq!(store.unheard(5).unwrap(), [(long.clone(), 4)], "and there is more of it");
        let mut steps = 0;
        let p = hear_more(&mut store, &model, 2, &mut |p| {
            assert!(p.listening);
            steps += 1;
            true
        });
        crate::hear::set_stretch(20 * 60);
        let all: String = store.recent(50, Some(Kind::Audio)).unwrap().into_iter().filter(|m| m.source.as_deref() == Some(long.as_path())).map(|m| m.text.to_lowercase()).collect::<Vec<_>>().join(" | ");
        assert!(all.contains("ferry") && all.contains("harbo") && all.contains("lighthouse"), "all of it is heard in the end: {all}");
        assert_eq!((store.unheard_count().unwrap(), p.done, p.failed), (0, 1, 0));
        assert!(p.read >= 1 && p.read <= 4, "the silence was not sat through: {} stretches of four seconds for forty of recording", p.read);
        // Nothing left: the runners have nothing to do.
        assert_eq!(hear_more(&mut store, &model, 2, &mut |_| true).read, 0);
        // One looked at before there was hearing is listened to when there is, even where
        // files are only being listed: it is not put back to being a name and a place.
        let meta = std::fs::metadata(&clip).unwrap();
        store.set_file(&clip, &format!("{}-d2", stamp(&meta)), 1).unwrap();
        let p = run(&mut store, &model, std::slice::from_ref(&dir), false, &mut |_| true);
        let again = store.recent(5, Some(Kind::Video)).unwrap().into_iter().find(|m| m.title == "trip.mp4").unwrap();
        assert!(!again.light && again.text.contains("What is said:") && p.read >= 1, "{}", again.text);
        // Windows will not remove a folder with an open file in it; the next run clears it away.
        let _ = std::fs::remove_dir_all(dir);
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
        // Several are read at once, so which two came first is not fixed:
        // two notes, or a note and the picture's listing.
        assert_eq!((p.read + p.listed, p.done, p.total), (2, 2, 5));
        let before = p.read;
        let p = run(&mut store, &model, &roots, true, &mut |_| true);
        assert_eq!((before + p.read, p.failed, p.done), (4, 1, 5), "the rest is read, and nothing twice");
        assert!(p.trouble.unwrap().starts_with("broken.png: "));
        assert_eq!((store.stats().unwrap().total() - store.stats().unwrap().of(Kind::Folder), store.stats().unwrap().light), (5, 1), "the one that could not be looked at stays listed by its name");
        let p = run(&mut store, &model, &roots, true, &mut |_| true);
        assert_eq!((p.read, p.failed), (0, 0), "and is not tried again until it changes");
        // Windows will not remove a folder with an open file in it; the next run clears it away.
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_video_is_remembered_by_what_its_frames_show() {
        if !have("ffmpeg") || !have("ffprobe") {
            return;
        }
        let dir = scratch("video");
        let clip = dir.join("clip.mp4");
        let made = neo_desktop::command(neo_desktop::fs::tool("ffmpeg")).args(["-v", "error", "-f", "lavfi", "-i", "color=c=0xDC2828:s=160x120:d=6", "-pix_fmt", "yuv420p"]).arg(&clip).status().unwrap();
        assert!(made.success());
        let model = Fake::default();
        let mut store = Store::open(&dir.join("db/memory.db"), &[1; 32], model.dims().unwrap()).unwrap();
        assert_eq!(index_file(&mut store, &model, &clip), Ok(Outcome::Read(1)));
        let m = &store.recent(1, None).unwrap()[0];
        assert_eq!(m.kind, Kind::Video);
        assert!(m.text.starts_with("A video 0:06 long. At 0:01: A dog running on a beach. At 0:03: "), "{}", m.text);
        assert_eq!(store.words_of(m.id).unwrap(), ["beach", "dog"]);
        // Windows will not remove a folder with an open file in it; the next run clears it away.
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod bench {
    use super::*;
    use crate::model::fake::Fake;

    /// By hand: how long the memory takes to be written, with a model that
    /// answers at once, so that what is timed is the keeping and not the
    /// model. `cargo test --release -p neo-apollo-core bench -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn how_long_keeping_takes() {
        let dir = std::env::temp_dir().join(format!("neo-apollo-bench-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("files")).unwrap();
        for i in 0..1500 {
            std::fs::write(dir.join(format!("files/song number {i} of the album.mp3")), b"not really sound").unwrap();
            std::fs::write(dir.join(format!("files/notes about thing {i}.txt")), format!("Some notes about thing number {i}, which is a thing among things.\n\nIt has a second paragraph too, about gardens and trains {i}.")).unwrap();
        }
        let model = Fake::default();
        let mut store = Store::open(&dir.join("db/memory.db"), &[1; 32], model.dims().unwrap()).unwrap();
        let began = std::time::Instant::now();
        let p = run(&mut store, &model, &[dir.join("files")], false, &mut |_| true);
        let took = began.elapsed();
        println!("{} files kept in {:.2?}: {:.2} ms each ({} read, {} listed, {} failed)", p.total, took, took.as_secs_f64() * 1000.0 / p.total as f64, p.read, p.listed, p.failed);
        assert_eq!((p.read, p.listed, p.failed), (1500, 1500, 0));
        let began = std::time::Instant::now();
        let again = run(&mut store, &model, &[dir.join("files")], false, &mut |_| true);
        println!("looked over again, nothing changed, in {:.2?}", began.elapsed());
        assert_eq!(again.read + again.listed, 0, "all of it was written down");
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }
}