//! Apollo's memory: an encrypted database searched by meaning.
//!
//! The file is SQLite encrypted whole with SQLCipher, so without the key it
//! is noise: no table names, no text, no file paths. Each memory is a piece
//! of text (what a picture shows, a passage of a document, something said)
//! with an embedding of it, a list of numbers placing it near other text
//! that means much the same. sqlite-vec keeps the embeddings and finds the
//! nearest to a question's.
//!
//! Each memory also carries a few words for what it is about. Those, with
//! embeddings of their own, are what the word cloud is drawn from.

use std::path::{Path, PathBuf};
use std::sync::Once;

use rusqlite::{Connection, OptionalExtension, params};

/// What a memory is of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Photo,
    Video,
    Document,
    /// Something asked of Apollo and what it answered.
    Conversation,
    /// Something Apollo was told to remember.
    Note,
    /// A folder, and what it is: an Obsidian vault, a project, or only a
    /// folder with a name. Known by where it is, for "where is my …".
    Folder,
    /// A sound recording: a voice memo, a podcast, a song. Known by what
    /// is said in it.
    Audio,
}

impl Kind {
    pub const ALL: [Kind; 7] = [Kind::Photo, Kind::Video, Kind::Document, Kind::Conversation, Kind::Note, Kind::Folder, Kind::Audio];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Photo => "photo",
            Kind::Video => "video",
            Kind::Document => "document",
            Kind::Conversation => "conversation",
            Kind::Note => "note",
            Kind::Folder => "folder",
            Kind::Audio => "audio",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }

    /// As a heading: "Photos".
    pub fn plural(self) -> &'static str {
        match self {
            Kind::Photo => "Photos",
            Kind::Video => "Videos",
            Kind::Document => "Documents",
            Kind::Conversation => "Conversations",
            Kind::Note => "Notes",
            Kind::Folder => "Folders",
            Kind::Audio => "Audio",
        }
    }
}

/// Something to remember.
#[derive(Clone, Debug, PartialEq)]
pub struct New<'a> {
    pub kind: Kind,
    /// The file it is of, if it is of one.
    pub source: Option<&'a Path>,
    pub title: &'a str,
    pub text: &'a str,
    /// Which passage of the file, for a document in several.
    pub part: u32,
    /// When, in seconds since 1970.
    pub created: u64,
    /// What it is about, in a few words.
    pub words: &'a [String],
}

/// Something remembered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Memory {
    pub id: i64,
    pub kind: Kind,
    pub source: Option<PathBuf>,
    pub title: String,
    pub text: String,
    pub part: u32,
    pub created: u64,
    /// Only listed so far: known by its name and place, and not yet looked
    /// at. A photo or a video stays so until something asks after it.
    pub light: bool,
}

/// A memory found by a search, and how far its meaning is from what was
/// asked: 0 the same, 1 unrelated.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub memory: Memory,
    pub distance: f32,
}

/// A word for the cloud.
#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    pub word: String,
    /// How many memories are about it, counting those about words that
    /// mean nearly the same.
    pub count: u32,
    /// How large to draw it, from 0 to 1.
    pub weight: f32,
    /// Words that mean nearly the same, folded into this one.
    pub also: Vec<String>,
}

/// A word that goes with another.
#[derive(Clone, Debug, PartialEq)]
pub struct Related {
    pub word: String,
    /// How many memories are about both.
    pub shared: u32,
    /// For one that shares none and is only near in meaning, how near.
    pub distance: Option<f32>,
}

/// How much is remembered.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Memories of each kind, in [`Kind::ALL`]'s order.
    pub memories: [u32; 7],
    pub words: u32,
    pub files: u32,
    /// Photos and videos that are only listed, not yet looked at.
    pub light: u32,
}

impl Stats {
    pub fn total(&self) -> u32 {
        self.memories.iter().sum()
    }

    pub fn of(&self, kind: Kind) -> u32 {
        self.memories[Kind::ALL.iter().position(|k| *k == kind).unwrap_or(0)]
    }
}

pub struct Store {
    db: Connection,
    dims: usize,
    /// The middles of the neighbourhoods embeddings are sorted into, once
    /// read: see [`Store::index_meanings`].
    middles: std::sync::Mutex<Option<std::sync::Arc<Vec<Middle>>>>,
}

/// The middle of a neighbourhood: its number, an embedding that stands
/// for those in it, and that embedding's length.
struct Middle {
    id: i64,
    vector: Vec<f32>,
    length: f32,
}

/// With fewer memories than this, a search compares the question with
/// every one, which is quick enough and exact.
const NEAR_FROM: usize = 20_000;
/// How many neighbourhoods a search looks in, and how many of what it
/// finds there it compares in full.
const PROBES: usize = 48;
const SHORTLIST: usize = 120;
/// A kind with no more memories than this is picked out by their numbers
/// before a search by meaning ranks anything.
const KIND_KNOWN_BY_NUMBER: i64 = 250_000;
/// How many of the memories last added that are about a word are looked
/// at for the newest of them, and for the words that go with it.
const ABOUT_AMONG: usize = 2000;
const RELATED_AMONG: usize = 2000;
/// How many of a kind's newest memories its cloud is counted from.
const CLOUD_AMONG: usize = 50_000;

/// An embedding cut down to a bit a number: whether it is above nothing.
/// Ninety-six bytes where there were three thousand, and two of them are
/// near in meaning roughly as they agree bit for bit; enough to pick out
/// the few hundred worth comparing in full.
fn bits(v: &[f32]) -> Vec<u8> {
    let mut out = vec![0u8; v.len().div_ceil(8)];
    for (i, x) in v.iter().enumerate() {
        if *x > 0.0 {
            out[i / 8] |= 1 << (i % 8);
        }
    }
    out
}

/// In how many bits two differ.
#[cfg(test)]
fn apart(a: &[u8], b: &[u8]) -> u32 {
    let wide = |c: &[u8]| u64::from_le_bytes(c.try_into().unwrap_or([0; 8]));
    let (a8, b8) = (a.chunks_exact(8), b.chunks_exact(8));
    let rest: u32 = a8.remainder().iter().zip(b8.remainder()).map(|(x, y)| (x ^ y).count_ones()).sum();
    a8.zip(b8).map(|(x, y)| (wide(x) ^ wide(y)).count_ones()).sum::<u32>() + rest
}

/// For a question, what each byte of another embedding's bits says of how
/// alike the two are: the question's own numbers added where the bit is
/// set and taken away where it is not. The question is kept whole and
/// only what it is held against is cut to bits, which tells near from far
/// a good deal better than counting the bits in which two differ, and so
/// far fewer need comparing in full afterwards.
fn weigh(v: &[f32]) -> Vec<[f32; 256]> {
    v.chunks(8)
        .map(|eight| {
            let mut table = [0.0f32; 256];
            for (byte, sum) in table.iter_mut().enumerate() {
                *sum = eight.iter().enumerate().map(|(i, x)| if byte >> i & 1 == 1 { *x } else { -*x }).sum();
            }
            table
        })
        .collect()
}

/// How unlike a question an embedding's bits are: the less, the nearer.
fn unlike(weighed: &[[f32; 256]], theirs: &[u8]) -> f32 {
    -weighed.iter().zip(theirs).map(|(table, byte)| table[*byte as usize]).sum::<f32>()
}

fn length(v: &[f32]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

/// The neighbourhoods nearest an embedding, nearest first.
fn nearest(middles: &[Middle], v: &[f32], most: usize) -> Vec<i64> {
    let own = length(v).max(f32::MIN_POSITIVE);
    let mut by: Vec<(f32, i64)> = middles.iter().map(|m| (-m.vector.iter().zip(v).map(|(a, b)| a * b).sum::<f32>() / (m.length.max(f32::MIN_POSITIVE) * own), m.id)).collect();
    let most = most.min(by.len());
    if most > 0 && most < by.len() {
        by.select_nth_unstable_by(most - 1, |a, b| a.0.total_cmp(&b.0));
    }
    by.truncate(most);
    by.sort_by(|a, b| a.0.total_cmp(&b.0));
    by.into_iter().map(|(_, id)| id).collect()
}

/// How an error begins that says the memory holds another model's embeddings.
pub const OTHER_MODEL: &str = "The memory was made with";

/// Words closer than this mean the same for the cloud's purposes: "dog"
/// and "dogs", "beach" and "seaside".
const SAME_WORD: f32 = 0.1;
/// How much nearer a memory counts as being for saying every word of the
/// question. Distances between a question and what answers it run from
/// about 0.25 to 0.55, so this is enough to lift one that says the words
/// over those that only sound alike, and not enough to lift one that is
/// about something else altogether.
pub const SAID: f32 = 0.12;

/// Words closer than this are near enough in meaning to go together.
const NEAR_WORD: f32 = 0.3;
/// How many of the commonest words are weighed for the cloud.
const CLOUD_POOL: usize = 400;

/// A file that looks like a picture that was asked about.
#[derive(Clone, Debug, PartialEq)]
pub struct SightMatch {
    pub source: PathBuf,
    /// How alike the whole picture is to the nearest moment of it, of 1.
    pub scene: f32,
    /// How alike the nearest face in it is to one in the picture.
    pub face: f32,
}

/// Whole pictures less alike than this are not the same scene: CLIP puts
/// any two photographs at 0.3 to 0.6, and two moments of one video, or
/// one place from another side, at 0.8 and up.
pub const SCENE_NEAR: f32 = 0.78;
/// Faces less alike than this are not taken for one person's. The model's
/// makers divide at 0.36; tried on real pictures, two different drawn
/// faces came out at 0.41 and 0.43 and the same one twice at 0.94, and
/// with a hundred thousand faces to be wrong about, the bar is set high.
pub const FACE_NEAR: f32 = 0.50;

impl SightMatch {
    /// How good a match it is, of 1: the better of the two ways of being
    /// alike, each counted from where it begins to mean anything. The very
    /// video a frame was taken from comes out near 1 by the whole picture;
    /// the same person somewhere else, from a half up by the face.
    pub fn score(&self) -> f32 {
        let scene = if self.scene >= SCENE_NEAR { 0.4 + 0.6 * (self.scene - SCENE_NEAR) / (1.0 - SCENE_NEAR) } else { 0.0 };
        let face = if self.face >= FACE_NEAR { 0.5 + 0.5 * (self.face - FACE_NEAR) / (1.0 - FACE_NEAR) } else { 0.0 };
        scene.max(face).clamp(0.0, 1.0)
    }
}

/// An embedding of length one, a byte a number.
fn bytes_of(v: &[f32]) -> Vec<u8> {
    v.iter().map(|x| ((x * 127.0).round().clamp(-127.0, 127.0) as i8) as u8).collect()
}

/// How alike an embedding of length one is to one kept a byte a number.
fn alike_bytes(ours: &[f32], theirs: &[u8]) -> f32 {
    if ours.len() != theirs.len() {
        return 0.0;
    }
    ours.iter().zip(theirs).map(|(a, b)| a * (*b as i8) as f32).sum::<f32>() / 127.0
}

fn blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn floats(b: &[u8]) -> Vec<f32> {
    b.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect()
}

/// How far apart two embeddings point: 0 the same way, 1 at right angles.
pub fn distance(a: &[f32], b: &[f32]) -> f32 {
    let (mut dot, mut aa, mut bb) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        aa += x * x;
        bb += y * y;
    }
    if aa == 0.0 || bb == 0.0 { 1.0 } else { 1.0 - dot / (aa.sqrt() * bb.sqrt()) }
}

fn err(e: rusqlite::Error) -> String {
    e.to_string()
}

const MEMORY: &str = "id, kind, source, title, text, part, created, light";

fn memory(row: &rusqlite::Row) -> rusqlite::Result<Memory> {
    Ok(Memory { id: row.get(0)?, kind: Kind::from_name(&row.get::<_, String>(1)?).unwrap_or(Kind::Note), source: row.get::<_, Option<String>>(2)?.map(PathBuf::from), title: row.get(3)?, text: row.get(4)?, part: row.get(5)?, created: row.get::<_, i64>(6)? as u64, light: row.get::<_, i64>(7)? != 0 })
}

impl Store {
    /// Opens the database at `path` with `key`, making it if it is not
    /// there. `dims` is the length of the embeddings it will hold. An error
    /// if the key is not the one it was made with, or if it was made for
    /// embeddings of another length (by another model).
    pub fn open(path: &Path, key: &[u8; 32], dims: usize) -> Result<Self, String> {
        static VECTORS: Once = Once::new();
        // SAFETY: sqlite-vec's entry point has the signature SQLite calls
        // extensions with; registering it is how the crate is meant to be used.
        VECTORS.call_once(|| unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<*const (), unsafe extern "C" fn(*mut rusqlite::ffi::sqlite3, *mut *mut std::ffi::c_char, *const rusqlite::ffi::sqlite3_api_routines) -> i32>(sqlite_vec::sqlite3_vec_init as *const ())));
        });
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("Couldn't make {}: {e}", dir.display()))?;
        }
        let db = Connection::open(path).map_err(err)?;
        let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
        db.pragma_update(None, "key", format!("x'{hex}'")).map_err(err)?;
        // The first read is where a wrong key shows.
        db.query_row("select count(*) from sqlite_master", [], |r| r.get::<_, i64>(0)).map_err(|_| "The memory could not be opened with this key.".to_owned())?;
        // The indexer writes while the app reads.
        let _ = db.pragma_update(None, "journal_mode", "wal");
        // With a log, this keeps the memory whole through a power cut and
        // costs no wait on the disk at every change; at worst the last few
        // files read are read again.
        let _ = db.pragma_update(None, "synchronous", "normal");
        // Embeddings are kept a thousand to a block of some megabytes, and
        // reaching one means walking the block's pages from its start. With
        // room for only a few hundred pages, each walk read and decrypted
        // them afresh, some milliseconds a time; with room for the blocks
        // in use, it is a walk through memory. Sixty-four megabytes.
        let _ = db.pragma_update(None, "cache_size", -65536);
        db.busy_timeout(std::time::Duration::from_secs(10)).map_err(err)?;
        db.execute_batch("create table if not exists meta(key text primary key, value text not null);").map_err(err)?;
        let made_for: Option<usize> = db.query_row("select value from meta where key = 'dims'", [], |r| r.get::<_, String>(0)).optional().map_err(err)?.and_then(|v| v.parse().ok());
        match made_for {
            Some(d) if d != dims => return Err(format!("{OTHER_MODEL} a model whose embeddings are {d} long, and this one's are {dims}.")),
            Some(_) => {}
            None => {
                db.execute("insert into meta(key, value) values ('dims', ?1)", [dims.to_string()]).map_err(err)?;
            }
        }
        db.execute_batch(&format!(
            "create table if not exists memories(id integer primary key, kind text not null, source text, title text not null, text text not null, part integer not null default 0, created integer not null, light integer not null default 0);
             create index if not exists memories_source on memories(source);
             create index if not exists memories_kind on memories(kind, light);
             create index if not exists memories_created on memories(created, id);
             create index if not exists memories_kind_created on memories(kind, created, id);
             create virtual table if not exists memory_vectors using vec0(embedding float[{dims}] distance_metric=cosine);
             create table if not exists words(id integer primary key, word text unique not null);
             create virtual table if not exists word_vectors using vec0(embedding float[{dims}] distance_metric=cosine);
             create table if not exists memory_words(memory integer not null, word integer not null, primary key(memory, word)) without rowid;
             create index if not exists memory_words_word on memory_words(word);
             create table if not exists unheard(path text primary key, heard integer not null);
             create table if not exists files(path text primary key, stamp text not null, memories integer not null);
             create table if not exists middles(id integer primary key, vector blob not null);
             create table if not exists nearby(place integer not null, memory integer not null, bits blob not null, primary key(place, memory)) without rowid;
             create index if not exists nearby_memory on nearby(memory);
create table if not exists sights(id integer primary key, source text not null, at real not null default 0, face integer not null default 0, vector blob not null);
create index if not exists sights_source on sights(source);
create table if not exists sighted(path text primary key, stamp text not null);"
        ))
        .map_err(err)?;
        // A memory made before there were listed-only ones has no place to say so.
        let has_light = db.prepare("select 1 from pragma_table_info('memories') where name = 'light'").and_then(|mut st| st.exists([])).map_err(err)?;
        if !has_light {
            db.execute_batch("alter table memories add column light integer not null default 0;").map_err(err)?;
        }
        Ok(Self { db, dims, middles: std::sync::Mutex::new(None) })
    }

    /// Notes that a recording has been listened to as far as `heard`
    /// seconds of its sound, and that there is more of it.
    pub fn set_unheard(&mut self, path: &Path, heard: u32) -> Result<(), String> {
        self.db.execute("insert into unheard(path, heard) values (?1, ?2) on conflict(path) do update set heard = ?2", params![path.to_string_lossy(), heard]).map(|_| ()).map_err(err)
    }

    /// Notes that a recording has been listened to to its end.
    pub fn all_heard(&mut self, path: &Path) -> Result<(), String> {
        self.db.execute("delete from unheard where path = ?1", [path.to_string_lossy()]).map(|_| ()).map_err(err)
    }

    /// The recordings with more to listen to, and how far each has been
    /// listened to, at most `most`: the ones nearest done first.
    pub fn unheard(&self, most: usize) -> Result<Vec<(PathBuf, u32)>, String> {
        let mut st = self.db.prepare("select path, heard from unheard order by heard desc, path limit ?1").map_err(err)?;
        st.query_map([most as i64], |r| Ok((PathBuf::from(r.get::<_, String>(0)?), r.get::<_, i64>(1)? as u32))).map_err(err)?.collect::<Result<_, _>>().map_err(err)
    }

    /// How many recordings have more to listen to.
    pub fn unheard_count(&self) -> Result<u32, String> {
        self.db.query_row("select count(*) from unheard", [], |r| r.get::<_, i64>(0)).map(|n| n as u32).map_err(err)
    }

    /// The memory of a file itself, as against of its later passages, and
    /// the number its next passage would have.
    pub fn first_of(&self, path: &Path) -> Result<Option<(Memory, u32)>, String> {
        let source = path.to_string_lossy();
        let first = self.db.query_row(&format!("select {MEMORY} from memories where source = ?1 order by part limit 1"), [&*source], memory).optional().map_err(err)?;
        let next: i64 = self.db.query_row("select coalesce(max(part), -1) + 1 from memories where source = ?1", [&*source], |r| r.get(0)).map_err(err)?;
        Ok(first.map(|m| (m, next as u32)))
    }

    /// Marks a memory as only listed: known by name and place, not looked at.
    /// Begins a run of changes that are written down together by
    /// [Store::commit]: one wait on the disk for many files, where each
    /// change on its own waits every time. The changes each method makes
    /// are still all or nothing within it.
    pub fn begin(&mut self) -> Result<(), String> {
        self.db.execute_batch("begin").map_err(err)
    }

    /// Writes down what was changed since [Store::begin]. Nothing, if
    /// nothing was begun.
    pub fn commit(&mut self) -> Result<(), String> {
        if self.db.is_autocommit() { Ok(()) } else { self.db.execute_batch("commit").map_err(err) }
    }

    pub fn mark_light(&mut self, id: i64) -> Result<(), String> {
        self.db.execute("update memories set light = 1 where id = ?1", [id]).map(|_| ()).map_err(err)
    }

    /// The files that are only listed, newest first, at most `most`.
    pub fn light_files(&self, most: usize) -> Result<Vec<PathBuf>, String> {
        let mut st = self.db.prepare("select source from memories where light != 0 and source is not null order by created desc, id desc limit ?1").map_err(err)?;
        st.query_map([most as i64], |r| r.get::<_, String>(0).map(PathBuf::from)).map_err(err)?.collect::<Result<_, _>>().map_err(err)
    }

    pub fn dims(&self) -> usize {
        self.dims
    }

    /// Opens the database for use with `model`: made for the length of its
    /// embeddings, and marked as its own. An error beginning with
    /// [`OTHER_MODEL`] if it already holds another model's embeddings,
    /// which this one's cannot be compared with.
    pub fn open_for(path: &Path, key: &[u8; 32], model: &dyn crate::Model) -> Result<Self, String> {
        let store = Self::open(path, key, model.dims()?)?;
        let name = model.name();
        match store.db.query_row("select value from meta where key = 'embedder'", [], |r| r.get::<_, String>(0)).optional().map_err(err)? {
            Some(made_with) if made_with != name => Err(format!("{OTHER_MODEL} {made_with}, and {name} is chosen now.")),
            Some(_) => Ok(store),
            None => {
                store.db.execute("insert into meta(key, value) values ('embedder', ?1)", [name]).map_err(err)?;
                Ok(store)
            }
        }
    }

    /// Removes the database at `path`, with the files SQLite keeps beside it.
    pub fn delete(path: &Path) -> std::io::Result<()> {
        for suffix in ["", "-wal", "-shm"] {
            let mut name = path.as_os_str().to_owned();
            name.push(suffix);
            match std::fs::remove_file(&name) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
        }
        Ok(())
    }

    /// The length of the embeddings the database at `path` was made for,
    /// if it is there and `key` opens it.
    pub fn made_for(path: &Path, key: &[u8; 32]) -> Option<usize> {
        if !path.is_file() {
            return None;
        }
        let db = Connection::open(path).ok()?;
        let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
        db.pragma_update(None, "key", format!("x'{hex}'")).ok()?;
        db.query_row("select value from meta where key = 'dims'", [], |r| r.get::<_, String>(0)).ok()?.parse().ok()
    }

    /// Remembers something, with the embedding of it. Returns its id.
    pub fn add(&mut self, new: &New, embedding: &[f32]) -> Result<i64, String> {
        if embedding.len() != self.dims {
            return Err(format!("An embedding {} long was given to a memory of {}.", embedding.len(), self.dims));
        }
        let tx = self.db.savepoint().map_err(err)?;
        tx.execute("insert into memories(kind, source, title, text, part, created) values (?1, ?2, ?3, ?4, ?5, ?6)", params![new.kind.name(), new.source.map(|p| p.to_string_lossy().into_owned()), new.title, new.text, new.part, new.created as i64]).map_err(err)?;
        let id = tx.last_insert_rowid();
        tx.execute("insert into memory_vectors(rowid, embedding) values (?1, ?2)", params![id, blob(embedding)]).map_err(err)?;
        for word in new.words {
            tx.execute("insert or ignore into words(word) values (?1)", [word]).map_err(err)?;
            tx.execute("insert or ignore into memory_words(memory, word) select ?1, id from words where word = ?2", params![id, word]).map_err(err)?;
        }
        tx.commit().map_err(err)?;
        self.place(id, embedding)?;
        Ok(id)
    }

    /// Those of `words` that have no embedding yet.
    pub fn words_without_vectors(&self, words: &[String]) -> Result<Vec<String>, String> {
        // In two steps, the word and then its embedding by number: asked
        // for both at once, the table of embeddings is looked through from
        // end to end for every word, which was most of the time it took to
        // keep a file once some thousands of words were known.
        let mut named = self.db.prepare_cached("select id from words where word = ?1").map_err(err)?;
        let mut embedded = self.db.prepare_cached("select 1 from word_vectors where rowid = ?1").map_err(err)?;
        let mut out: Vec<String> = vec![];
        for word in words {
            if out.contains(word) {
                continue;
            }
            let id: Option<i64> = named.query_row([word], |r| r.get(0)).optional().map_err(err)?;
            if !match id {
                Some(id) => embedded.exists([id]).map_err(err)?,
                None => false,
            } {
                out.push(word.clone());
            }
        }
        Ok(out)
    }

    /// Every word that has its embedding.
    pub fn words_with_vectors(&self) -> Result<Vec<String>, String> {
        let mut st = self.db.prepare("select word from words where id in (select rowid from word_vectors)").map_err(err)?;
        st.query_map([], |r| r.get::<_, String>(0)).map_err(err)?.collect::<Result<_, _>>().map_err(err)
    }

    /// Gives a word its embedding, which is what lets the cloud tell that
    /// two words mean the same, and a click on one find what is near it.
    pub fn set_word_vector(&mut self, word: &str, embedding: &[f32]) -> Result<(), String> {
        if embedding.len() != self.dims {
            return Err(format!("An embedding {} long was given to a memory of {}.", embedding.len(), self.dims));
        }
        let tx = self.db.savepoint().map_err(err)?;
        tx.execute("insert or ignore into words(word) values (?1)", [word]).map_err(err)?;
        let id: i64 = tx.query_row("select id from words where word = ?1", [word], |r| r.get(0)).map_err(err)?;
        tx.execute("delete from word_vectors where rowid = ?1", [id]).map_err(err)?;
        tx.execute("insert into word_vectors(rowid, embedding) values (?1, ?2)", params![id, blob(embedding)]).map_err(err)?;
        tx.commit().map_err(err)
    }

    pub fn word_vector(&self, word: &str) -> Result<Option<Vec<f32>>, String> {
        self.db.query_row("select v.embedding from words w join word_vectors v on v.rowid = w.id where w.word = ?1", [word], |r| r.get::<_, Vec<u8>>(0)).optional().map(|b| b.map(|b| floats(&b))).map_err(err)
    }

    /// The `most` memories nearest in meaning to `embedding`, nearest
    /// first, of one kind if `kind` is given.
    pub fn search(&self, embedding: &[f32], most: usize, kind: Option<Kind>) -> Result<Vec<Hit>, String> {
        if embedding.len() != self.dims {
            return Err(format!("A question's embedding is {} long, and the memory's are {}.", embedding.len(), self.dims));
        }
        // By way of the neighbourhoods where there are any: a few steps,
        // and a few megabytes read, in place of every embedding there is.
        if let Some(middles) = self.middles()? {
            return self.search_near(&middles, embedding, most, kind, SHORTLIST);
        }
        self.search_every(embedding, most, kind)
    }

    /// The nearest by comparing with every embedding there is.
    fn search_every(&self, embedding: &[f32], most: usize, kind: Option<Kind>) -> Result<Vec<Hit>, String> {
        // The nearest are found first and the kind picked out after, so
        // more are asked for when only some will do.
        let reach = if kind.is_some() { most * 8 } else { most }.clamp(1, 4000);
        let mut near = self.db.prepare(&format!("select {}, v.distance from memory_vectors v join memories m on m.id = v.rowid where v.embedding match ?1 and k = ?2 order by v.distance", MEMORY.split(", ").map(|c| format!("m.{c}")).collect::<Vec<_>>().join(", "))).map_err(err)?;
        let hits = near.query_map(params![blob(embedding), reach as i64], |r| Ok(Hit { memory: memory(r)?, distance: r.get::<_, f64>(8)? as f32 })).map_err(err)?;
        let mut out = vec![];
        for hit in hits {
            let hit = hit.map_err(err)?;
            if kind.is_none_or(|k| k == hit.memory.kind) && out.len() < most {
                out.push(hit);
            }
        }
        Ok(out)
    }

    /// As [`search`](Self::search), for a question whose own words are
    /// known as well as its embedding. Nearness in meaning alone ranks a
    /// picture described as "a woman in a bright pink wig" hardly above any
    /// other picture of a woman; one that says the very words asked for is
    /// moved up, by up to [`SAID`] for saying all of them. Memories that
    /// say every word but are not among the nearest are brought in too.
    pub fn search_for(&self, embedding: &[f32], terms: &[String], most: usize, kind: Option<Kind>) -> Result<Vec<Hit>, String> {
        if terms.is_empty() {
            return self.search(embedding, most, kind);
        }
        let mut hits = self.search(embedding, (most * 2).max(60), kind)?;
        // With a great many memories, those that say every word are found
        // through the words themselves, each a short walk in an index:
        // looking for the words in the text of every memory there is
        // takes seconds, which is no way to answer what is being typed.
        if self.middles()?.is_some() {
            let mut all: Option<Vec<i64>> = None;
            let mut of_word = self.db.prepare_cached("select mw.memory from words w join memory_words mw on mw.word = w.id where w.word >= ?1 and w.word < ?2 order by mw.memory desc limit 4000").map_err(err)?;
            for term in terms {
                // Words that begin as the term does: "garden" finds "gardens".
                let after = format!("{term}\u{10ffff}");
                let have: Vec<i64> = of_word.query_map(params![term, after], |r| r.get(0)).map_err(err)?.collect::<Result<_, _>>().map_err(err)?;
                all = Some(match all {
                    None => have,
                    Some(so_far) => so_far.into_iter().filter(|id| have.contains(id)).collect(),
                });
            }
            let mut whole = self.db.prepare_cached("select embedding from memory_vectors where rowid = ?1").map_err(err)?;
            let mut one = self.db.prepare_cached(&format!("select {MEMORY} from memories where id = ?1")).map_err(err)?;
            for id in all.unwrap_or_default().into_iter().take(24) {
                if hits.iter().any(|h| h.memory.id == id) {
                    continue;
                }
                let Some(m) = one.query_row([id], memory).optional().map_err(err)? else { continue };
                if kind.is_none_or(|k| k == m.kind) {
                    let far = whole.query_row([id], |r| r.get::<_, Vec<u8>>(0)).optional().map_err(err)?.map_or(1.0, |v| distance(embedding, &floats(&v)));
                    hits.push(Hit { memory: m, distance: far });
                }
            }
            return self.rank_by_words(embedding, terms, hits, most);
        }
        // Those that say every word, wherever they stand by meaning.
        let says_all = (1..=terms.len()).map(|i| format!("(lower(m.title || ' ' || m.text) like ?{i} or exists (select 1 from memory_words mw join words w on w.id = mw.word where mw.memory = m.id and w.word like ?{i}))")).collect::<Vec<_>>().join(" and ");
        let mut st = self.db.prepare(&format!("select {}, (select embedding from memory_vectors v where v.rowid = m.id) from memories m where {says_all} order by m.created desc limit 60", MEMORY.split(", ").map(|c| format!("m.{c}")).collect::<Vec<_>>().join(", "))).map_err(err)?;
        let like: Vec<String> = terms.iter().map(|t| format!("%{}%", t.replace(['%', '_'], ""))).collect();
        let said = st.query_map(rusqlite::params_from_iter(&like), |r| Ok((memory(r)?, r.get::<_, Option<Vec<u8>>>(8)?))).map_err(err)?;
        for row in said {
            let (m, vector) = row.map_err(err)?;
            if kind.is_none_or(|k| k == m.kind) && !hits.iter().any(|h| h.memory.id == m.id) {
                let far = vector.map_or(1.0, |v| distance(embedding, &floats(&v)));
                hits.push(Hit { memory: m, distance: far });
            }
        }
        let _ = embedding;
        self.rank_by_words(embedding, terms, hits, most)
    }

    /// Puts what was found in order: by nearness in meaning, moved up for
    /// saying the words asked for.
    fn rank_by_words(&self, _embedding: &[f32], terms: &[String], hits: Vec<Hit>, most: usize) -> Result<Vec<Hit>, String> {
        let mut scored = Vec::with_capacity(hits.len());
        for hit in hits {
            let mut said = format!("{} {} ", hit.memory.title, hit.memory.text).to_lowercase();
            said.push_str(&self.words_of(hit.memory.id)?.join(" "));
            let share = terms.iter().filter(|t| said.contains(t.as_str())).count() as f32 / terms.len() as f32;
            scored.push((hit.distance - SAID * share, hit));
        }
        scored.sort_by(|a, b| a.0.total_cmp(&b.0));
        Ok(scored.into_iter().take(most).map(|(_, hit)| hit).collect())
    }

    /// The memories that say every one of `terms`, or a word beginning as
    /// it does, the newest first: found through the words alone, which
    /// asks nothing of a model and is answered at once.
    pub fn saying(&self, terms: &[String], most: usize, kind: Option<Kind>) -> Result<Vec<Memory>, String> {
        let mut all: Option<Vec<i64>> = None;
        let mut of_word = self.db.prepare_cached("select mw.memory from words w join memory_words mw on mw.word = w.id where w.word >= ?1 and w.word < ?2 order by mw.memory desc limit 4000").map_err(err)?;
        for term in terms {
            let after = format!("{term}\u{10ffff}");
            let mut have: Vec<i64> = of_word.query_map(params![term, after], |r| r.get(0)).map_err(err)?.collect::<Result<_, _>>().map_err(err)?;
            have.sort_unstable();
            all = Some(match all {
                None => have,
                Some(so_far) => so_far.into_iter().filter(|id| have.binary_search(id).is_ok()).collect(),
            });
        }
        let mut ids = all.unwrap_or_default();
        ids.sort_unstable_by(|a, b| b.cmp(a));
        ids.dedup();
        let mut one = self.db.prepare_cached(&format!("select {MEMORY} from memories where id = ?1")).map_err(err)?;
        let mut out = vec![];
        for id in ids {
            if let Some(m) = one.query_row([id], memory).optional().map_err(err)?.filter(|m: &Memory| kind.is_none_or(|k| k == m.kind)) {
                out.push(m);
                if out.len() >= most {
                    break;
                }
            }
        }
        Ok(out)
    }

    /// The memories that are about `word`, newest first.
    ///
    /// Of one kind if `kind` is given. With a word that a great many are
    /// about, they are the newest of the [`ABOUT_AMONG`] last remembered:
    /// putting every memory about "music" in order of age to keep forty
    /// took seconds, where the last so many are a short walk in an index.
    pub fn about(&self, word: &str, most: usize, kind: Option<Kind>) -> Result<Vec<Memory>, String> {
        let ids: Vec<i64> = {
            let mut st = self.db.prepare_cached("select mw.memory from words w join memory_words mw on mw.word = w.id where w.word = ?1 order by mw.memory desc limit ?2").map_err(err)?;
            st.query_map(params![word, ABOUT_AMONG as i64], |r| r.get(0)).map_err(err)?.collect::<Result<_, _>>().map_err(err)?
        };
        let mut one = self.db.prepare_cached(&format!("select {MEMORY} from memories where id = ?1")).map_err(err)?;
        let mut out: Vec<Memory> = vec![];
        for id in ids {
            if let Some(m) = one.query_row([id], memory).optional().map_err(err)?.filter(|m: &Memory| kind.is_none_or(|k| k == m.kind)) {
                out.push(m);
            }
        }
        out.sort_by(|a, b| b.created.cmp(&a.created).then(b.id.cmp(&a.id)));
        out.truncate(most);
        Ok(out)
    }

    /// The newest memories, of one kind if `kind` is given.
    pub fn recent(&self, most: usize, kind: Option<Kind>) -> Result<Vec<Memory>, String> {
        // Asked in two ways, each of which an index answers from its end:
        // "of any kind, or of this one" in a single question is answered
        // by sorting every memory there is.
        match kind {
            None => {
                let mut st = self.db.prepare(&format!("select {MEMORY} from memories order by created desc, id desc limit ?1")).map_err(err)?;
                st.query_map(params![most as i64], memory).map_err(err)?.collect::<Result<_, _>>().map_err(err)
            }
            Some(kind) => {
                let mut st = self.db.prepare(&format!("select {MEMORY} from memories where kind = ?1 order by created desc, id desc limit ?2")).map_err(err)?;
                st.query_map(params![kind.name(), most as i64], memory).map_err(err)?.collect::<Result<_, _>>().map_err(err)
            }
        }
    }

    /// The things remembered from a stretch of time, newest first: each
    /// file once, however many passages of it there are. Of one kind if
    /// `kind` is given; from `since` up to but not including `until`,
    /// where they are. With them, how many there are in all, which may be
    /// more than the `most` returned.
    pub fn between(&self, kind: Option<Kind>, since: Option<u64>, until: Option<u64>, most: usize) -> Result<(Vec<Memory>, u32), String> {
        let which = "part = 0 and (?1 is null or kind = ?1) and (?2 is null or created >= ?2) and (?3 is null or created < ?3)";
        let (kind, since, until) = (kind.map(Kind::name), since.map(|t| t as i64), until.map(|t| t as i64));
        let total: i64 = self.db.query_row(&format!("select count(*) from memories where {which}"), params![kind, since, until], |r| r.get(0)).map_err(err)?;
        let mut st = self.db.prepare(&format!("select {MEMORY} from memories where {which} order by created desc, id desc limit ?4")).map_err(err)?;
        let found = st.query_map(params![kind, since, until, most as i64], memory).map_err(err)?.collect::<Result<_, _>>().map_err(err)?;
        Ok((found, total as u32))
    }

    /// The words a memory is about.
    pub fn words_of(&self, id: i64) -> Result<Vec<String>, String> {
        let mut st = self.db.prepare("select w.word from words w join memory_words mw on mw.word = w.id where mw.memory = ?1 order by w.word").map_err(err)?;
        st.query_map([id], |r| r.get(0)).map_err(err)?.collect::<Result<_, _>>().map_err(err)
    }

    /// The words for the cloud, weightiest first, at most `most`.
    ///
    /// A word's weight is how many memories are about it, tempered so that
    /// one in nearly every memory ("photo") does not drown the rest. Words
    /// whose embeddings are close are one word, under the commonest of them.
    ///
    /// With a `kind`, only that kind's memories are counted: the cloud of
    /// the videos, say, and not of everything.
    pub fn cloud(&self, most: usize, kind: Option<Kind>) -> Result<Vec<Word>, String> {
        let kind = kind.map(Kind::name);
        // Asked in two ways, as an index can answer each.
        let total: f32 = match kind {
            None => self.db.query_row("select count(*) from memories", [], |r| r.get::<_, i64>(0)),
            // A kind's cloud is of its newest memories, and so is its count.
            Some(kind) => self.db.query_row("select count(*) from memories where kind = ?1", [kind], |r| r.get::<_, i64>(0)).map(|n| n.min(CLOUD_AMONG as i64)),
        }
        .map_err(err)? as f32;
        // The commonest words first, and only then their embeddings, one
        // by one: asked for in the same breath, an embedding was fetched
        // for every word there is before any was dropped for being rare,
        // which took twenty seconds with sixty thousand words. And words
        // of every kind are counted without looking at the memories at
        // all, since nothing about a memory is wanted then.
        let counted: Vec<(i64, String, u32)> = {
            let row = |r: &rusqlite::Row| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? as u32));
            match kind {
                None => {
                    let mut st = self.db.prepare("select w.id, w.word, c.n from (select word, count(*) as n from memory_words group by word order by n desc limit ?1) c join words w on w.id = c.word order by c.n desc, w.word").map_err(err)?;
                    st.query_map(params![CLOUD_POOL as i64], row).map_err(err)?.collect::<Result<_, _>>().map_err(err)?
                }
                Some(kind) => {
                    // Counted from the newest of that kind and not from all
                    // of them: with half a million documents every one of
                    // them was looked up for its words, five seconds of it,
                    // for a picture that the newest fifty thousand give.
                    let mut st = self.db.prepare("select w.id, w.word, c.n from (select mw.word as word, count(*) as n from (select id from memories where kind = ?2 order by created desc, id desc limit ?3) m join memory_words mw on mw.memory = m.id group by mw.word order by n desc limit ?1) c join words w on w.id = c.word order by c.n desc, w.word").map_err(err)?;
                    st.query_map(params![CLOUD_POOL as i64, kind, CLOUD_AMONG as i64], row).map_err(err)?.collect::<Result<_, _>>().map_err(err)?
                }
            }
        };
        let mut embedding = self.db.prepare_cached("select embedding from word_vectors where rowid = ?1").map_err(err)?;
        let mut pool: Vec<(String, u32, Option<Vec<f32>>)> = Vec::with_capacity(counted.len());
        for (id, word, count) in counted {
            let vector = embedding.query_row([id], |r| r.get::<_, Vec<u8>>(0)).optional().map_err(err)?.map(|b| floats(&b));
            pool.push((word, count, vector));
        }
        // Commonest first, each either joining one already kept or kept itself.
        let mut kept: Vec<(Word, Option<Vec<f32>>)> = vec![];
        for (word, count, vector) in pool {
            let same = vector.as_ref().and_then(|v| kept.iter().position(|(_, k)| k.as_ref().is_some_and(|k| distance(v, k) < SAME_WORD)));
            match same {
                Some(i) => {
                    kept[i].0.count += count;
                    kept[i].0.also.push(word);
                }
                None => kept.push((Word { word, count, weight: 0.0, also: vec![] }, vector)),
            }
        }
        let mut words: Vec<Word> = kept.into_iter().map(|(w, _)| w).collect();
        for w in &mut words {
            // Popular, but less so for being everywhere.
            let share = if total > 0.0 { (w.count as f32 / total).min(1.0) } else { 0.0 };
            w.weight = (w.count as f32).sqrt() * (1.0 - 0.6 * share * share);
        }
        words.sort_by(|a, b| b.weight.total_cmp(&a.weight).then_with(|| a.word.cmp(&b.word)));
        words.truncate(most);
        let top = words.first().map_or(1.0, |w| w.weight).max(f32::MIN_POSITIVE);
        for w in &mut words {
            w.weight /= top;
        }
        Ok(words)
    }

    /// The words that go with `word`: those that the memories about it
    /// (or about `also`, words folded into it) are about as well, the ones
    /// that share most memories first; and then, if there is room, words
    /// that are simply near it in meaning. At most `most`.
    /// With a `kind`, only among memories of that kind.
    pub fn related(&self, word: &str, also: &[String], most: usize, kind: Option<Kind>) -> Result<Vec<Related>, String> {
        // Among the memories about it that were last remembered, and not
        // among all of them: every word of every memory about a word that
        // a hundred thousand are about is a second and more of counting.
        let about = format!("select distinct mw1.memory as memory from words w1 join memory_words mw1 on mw1.word = w1.id where w1.word in ({{marks}}) order by mw1.memory desc limit {}", if kind.is_some() { RELATED_AMONG * 4 } else { RELATED_AMONG });
        let about = match kind {
            Some(k) => format!("select a0.memory as memory from ({about}) a0 cross join memories m on m.id = a0.memory where m.kind = '{}' limit {RELATED_AMONG}", k.name()),
            None => about,
        };
        let mut names: Vec<&str> = vec![word];
        names.extend(also.iter().map(String::as_str));
        let marks = vec!["?"; names.len()].join(", ");
        let mut st = self
            .db
            .prepare(&format!(
                "select w2.word, count(*) as n from ({about}) a join memory_words mw2 on mw2.memory = a.memory join words w2 on w2.id = mw2.word
                 where w2.word not in ({marks}) group by w2.id order by n desc, w2.word limit {most}",
                about = about.replace("{marks}", &marks)
            ))
            .map_err(err)?;
        let twice: Vec<&str> = names.iter().chain(names.iter()).copied().collect();
        let mut out: Vec<Related> = st.query_map(rusqlite::params_from_iter(twice), |r| Ok(Related { word: r.get(0)?, shared: r.get::<_, i64>(1)? as u32, distance: None })).map_err(err)?.collect::<Result<_, _>>().map_err(err)?;
        // Words near in meaning are looked for by holding the word against
        // every other there is, which is a second and more once there are
        // tens of thousands; with that many, the words that share memories
        // are what is shown, and there are seldom too few of them.
        let few_words = self.db.query_row("select coalesce(max(id), 0) from words", [], |r| r.get::<_, i64>(0)).map_err(err)? < NEAR_FROM as i64;
        if out.len() < most
            && few_words
            && let Some(vector) = self.word_vector(word)?
        {
            let mut near = self.db.prepare("select w.word, v.distance from word_vectors v join words w on w.id = v.rowid where v.embedding match ?1 and k = ?2 order by v.distance").map_err(err)?;
            let found: Vec<(String, f32)> = near.query_map(params![blob(&vector), (most * 2 + names.len()) as i64], |r| Ok((r.get(0)?, r.get::<_, f64>(1)? as f32))).map_err(err)?.collect::<Result<_, _>>().map_err(err)?;
            // Near in meaning, and with a kind chosen, a word that kind's memories use.
            let used = |w: &str| -> Result<bool, String> {
                match kind {
                    None => Ok(true),
                    Some(k) => self.db.prepare("select 1 from memory_words mw join words w on w.id = mw.word join memories m on m.id = mw.memory where w.word = ?1 and m.kind = ?2").and_then(|mut st| st.exists(params![w, k.name()])).map_err(err),
                }
            };
            for (w, distance) in found {
                if out.len() < most && distance < NEAR_WORD && used(&w)? && !names.contains(&w.as_str()) && !out.iter().any(|r| r.word == w) {
                    out.push(Related { word: w, shared: 0, distance: Some(distance) });
                }
            }
        }
        Ok(out)
    }

    /// Forgets everything remembered of a file.
    pub fn forget_source(&mut self, source: &Path) -> Result<u32, String> {
        let source = source.to_string_lossy();
        let tx = self.db.savepoint().map_err(err)?;
        // The memories of it are found first, and their embeddings taken
        // out one by one: asked for "those whose memory is of this file",
        // the table of embeddings looks through every one it holds, which
        // took longer for each file the more had been read.
        let ids: Vec<i64> = {
            let mut of_it = tx.prepare_cached("select id from memories where source = ?1").map_err(err)?;
            of_it.query_map([&*source], |r| r.get(0)).map_err(err)?.collect::<Result<_, _>>().map_err(err)?
        };
        for id in &ids {
            tx.execute("delete from memory_vectors where rowid = ?1", [id]).map_err(err)?;
            tx.execute("delete from memory_words where memory = ?1", [id]).map_err(err)?;
            tx.execute("delete from nearby where memory = ?1", [id]).map_err(err)?;
        }
        let n = tx.execute("delete from memories where source = ?1", [&*source]).map_err(err)?;
        tx.execute("delete from files where path = ?1", [&*source]).map_err(err)?;
        tx.execute("delete from unheard where path = ?1", [&*source]).map_err(err)?;
        tx.execute("delete from sights where source = ?1", [&*source]).map_err(err)?;
        tx.execute("delete from sighted where path = ?1", [&*source]).map_err(err)?;
        tx.commit().map_err(err)?;
        Ok(n as u32)
    }

    /// Forgets one memory.
    pub fn forget(&mut self, id: i64) -> Result<(), String> {
        let tx = self.db.savepoint().map_err(err)?;
        tx.execute("delete from memory_vectors where rowid = ?1", [id]).map_err(err)?;
        tx.execute("delete from memory_words where memory = ?1", [id]).map_err(err)?;
        tx.execute("delete from nearby where memory = ?1", [id]).map_err(err)?;
        tx.execute("delete from memories where id = ?1", [id]).map_err(err)?;
        tx.commit().map_err(err)
    }

    /// The middles of the neighbourhoods, if there are any yet.
    fn middles(&self) -> Result<Option<std::sync::Arc<Vec<Middle>>>, String> {
        let mut kept = self.middles.lock().map_err(|_| "The memory's index is in a bad way.".to_owned())?;
        if kept.is_none() {
            let mut st = self.db.prepare("select id, vector from middles order by id").map_err(err)?;
            let read: Vec<Middle> = st
                .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?)))
                .map_err(err)?
                .map(|row| {
                    row.map(|(id, blob)| {
                        let vector = floats(&blob);
                        Middle { id, length: length(&vector), vector }
                    })
                })
                .collect::<Result<_, _>>()
                .map_err(err)?;
            *kept = Some(std::sync::Arc::new(read));
        }
        Ok(kept.clone().filter(|m| !m.is_empty()))
    }

    /// Puts a memory in the neighbourhood its embedding is nearest, once
    /// there are neighbourhoods; before that there is nothing to do.
    fn place(&self, id: i64, embedding: &[f32]) -> Result<(), String> {
        let Some(middles) = self.middles()? else { return Ok(()) };
        let Some(place) = nearest(&middles, embedding, 1).pop() else { return Ok(()) };
        self.db.execute("insert or replace into nearby(place, memory, bits) values (?1, ?2, ?3)", params![place, id, bits(embedding)]).map(|_| ()).map_err(err)
    }

    /// Makes, or brings up to date, what lets a search by meaning find
    /// its answers in a few steps where it would otherwise compare the
    /// question with every memory there is: with hundreds of thousands,
    /// gigabytes of embeddings read for each search.
    ///
    /// It is an index as a table has an index. Embeddings are sorted into
    /// neighbourhoods by which of some hundreds of them, taken as middles,
    /// each is nearest; and those of a neighbourhood are kept together,
    /// cut down to a bit a number, so that reading one neighbourhood is
    /// reading one stretch of the file. A search finds the few middles
    /// nearest the question, reads those neighbourhoods and no others,
    /// and compares in full only the few hundred that come out nearest.
    ///
    /// `go_on` is told how many are placed, of how many, and stops it by
    /// returning false; it takes up where it left off. Returns whether
    /// there is an index now. Below [`NEAR_FROM`] memories there is none,
    /// and none is wanted.
    pub fn index_meanings(&mut self, go_on: &mut dyn FnMut(usize, usize) -> bool) -> Result<bool, String> {
        let count = |db: &Connection, sql: &str| db.query_row(sql, [], |r| r.get::<_, i64>(0)).map(|n| n as usize).map_err(err);
        let total = count(&self.db, "select count(*) from memories")?;
        if total < NEAR_FROM {
            return Ok(false);
        }
        let made_for: usize = self.db.query_row("select value from meta where key = 'middles-for'", [], |r| r.get::<_, String>(0)).optional().map_err(err)?.and_then(|v| v.parse().ok()).unwrap_or(0);
        // None yet, or made for a memory a quarter the size: begun again.
        if count(&self.db, "select count(*) from middles")? == 0 || total > made_for.saturating_mul(4) {
            let many = ((total as f64).sqrt() as usize).clamp(64, 2048);
            let chosen: Vec<i64> = {
                let mut st = self.db.prepare("select id from memories order by random() limit ?1").map_err(err)?;
                st.query_map([many as i64], |r| r.get(0)).map_err(err)?.collect::<Result<_, _>>().map_err(err)?
            };
            self.db.execute_batch("begin; delete from nearby; delete from middles;").map_err(err)?;
            for (n, id) in chosen.iter().enumerate() {
                if let Some(vector) = self.db.query_row("select embedding from memory_vectors where rowid = ?1", [id], |r| r.get::<_, Vec<u8>>(0)).optional().map_err(err)? {
                    self.db.execute("insert into middles(id, vector) values (?1, ?2)", params![n as i64 + 1, vector]).map_err(err)?;
                }
            }
            self.db.execute("insert into meta(key, value) values ('middles-for', ?1) on conflict(key) do update set value = ?1", [total.to_string()]).map_err(err)?;
            self.db.execute_batch("commit").map_err(err)?;
            if let Ok(mut kept) = self.middles.lock() {
                *kept = None;
            }
        }
        let Some(middles) = self.middles()? else { return Ok(false) };
        let mut placed = count(&self.db, "select count(*) from nearby")?;
        if placed >= total {
            return Ok(true);
        }
        // Every embedding in turn; those placed already are passed by.
        let mut is_placed = self.db.prepare("select 1 from nearby where memory = ?1 limit 1").map_err(err)?;
        let mut put = self.db.prepare("insert or replace into nearby(place, memory, bits) values (?1, ?2, ?3)").map_err(err)?;
        let mut all = self.db.prepare("select rowid, embedding from memory_vectors").map_err(err)?;
        let mut rows = all.query([]).map_err(err)?;
        let mut waiting: Vec<(i64, Vec<f32>)> = vec![];
        let mut done = false;
        while !done {
            match rows.next().map_err(err)? {
                Some(row) => {
                    let id: i64 = row.get(0).map_err(err)?;
                    if !is_placed.exists([id]).map_err(err)? {
                        waiting.push((id, floats(&row.get::<_, Vec<u8>>(1).map_err(err)?)));
                    }
                }
                None => done = true,
            }
            if waiting.len() >= 4096 || (done && !waiting.is_empty()) {
                // Which neighbourhood each belongs in is worked out on every core; then they are written down together.
                let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 12);
                let share = waiting.len().div_ceil(threads).max(1);
                let places: Vec<i64> = std::thread::scope(|scope| {
                    let running: Vec<_> = waiting.chunks(share).map(|some| scope.spawn(|| some.iter().map(|(_, v)| nearest(&middles, v, 1).pop().unwrap_or(1)).collect::<Vec<i64>>())).collect();
                    running.into_iter().flat_map(|r| r.join().unwrap_or_default()).collect()
                });
                self.db.execute_batch("begin").map_err(err)?;
                for ((id, vector), place) in waiting.iter().zip(places) {
                    put.execute(params![place, id, bits(vector)]).map_err(err)?;
                }
                self.db.execute_batch("commit").map_err(err)?;
                placed += waiting.len();
                waiting.clear();
                if !go_on(placed.min(total), total) {
                    return Ok(true);
                }
            }
        }
        Ok(true)
    }

    /// The memories nearest an embedding, by way of the neighbourhoods.
    fn search_near(&self, middles: &[Middle], embedding: &[f32], most: usize, kind: Option<Kind>, shortlist: usize) -> Result<Vec<Hit>, String> {
        let asked = weigh(embedding);
        let mut found: Vec<(f32, i64)> = vec![];
        // A kind there is little of, beside everything else, is picked out
        // before anything is ranked: with a million documents and forty
        // thousand photos, the nearest three thousand of every kind held
        // too few photos to fill a page, however many photos were near.
        // Its memories are few enough to know by number; and more
        // neighbourhoods are looked in, since each holds few of them.
        let of_kind: Option<std::collections::HashSet<i64>> = match kind {
            Some(k) if self.db.query_row("select count(*) from memories where kind = ?1", [k.name()], |r| r.get::<_, i64>(0)).map_err(err)? <= KIND_KNOWN_BY_NUMBER => {
                let mut st = self.db.prepare_cached("select id from memories where kind = ?1").map_err(err)?;
                Some(st.query_map([k.name()], |r| r.get(0)).map_err(err)?.collect::<Result<_, _>>().map_err(err)?)
            }
            _ => None,
        };
        let mut of_place = self.db.prepare_cached("select memory, bits from nearby where place = ?1").map_err(err)?;
        for place in nearest(middles, embedding, if of_kind.is_some() { PROBES * 4 } else { PROBES }) {
            let mut rows = of_place.query([place]).map_err(err)?;
            while let Some(row) = rows.next().map_err(err)? {
                let id: i64 = row.get(0).map_err(err)?;
                if of_kind.as_ref().is_some_and(|ids| !ids.contains(&id)) {
                    continue;
                }
                let theirs = row.get_ref(1).map_err(err)?.as_blob().map_err(|e| e.to_string())?;
                found.push((unlike(&asked, theirs), id));
            }
        }
        // More are kept when only one kind will do and it was not picked out, since most will be of another.
        let keep = if kind.is_some() && of_kind.is_none() { shortlist * 25 } else { shortlist }.max(most + most / 2).min(found.len());
        if keep > 0 && keep < found.len() {
            found.select_nth_unstable_by(keep - 1, |a, b| a.0.total_cmp(&b.0));
        }
        found.truncate(keep);
        found.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        let mut whole = self.db.prepare_cached("select embedding from memory_vectors where rowid = ?1").map_err(err)?;
        let mut one = self.db.prepare_cached(&format!("select {MEMORY} from memories where id = ?1")).map_err(err)?;
        let mut hits: Vec<Hit> = vec![];
        for (_, id) in found {
            let Some(m) = one.query_row([id], memory).optional().map_err(err)? else { continue };
            if kind.is_some_and(|k| k != m.kind) {
                continue;
            }
            let Some(vector) = whole.query_row([id], |r| r.get::<_, Vec<u8>>(0)).optional().map_err(err)? else { continue };
            hits.push(Hit { memory: m, distance: distance(embedding, &floats(&vector)) });
            if kind.is_some() && hits.len() >= shortlist.max(most * 2) {
                break;
            }
        }
        hits.sort_by(|a, b| a.distance.total_cmp(&b.distance));
        hits.truncate(most);
        Ok(hits)
    }

    /// What a picture or a video looked like when sight last took it in:
    /// see [`crate::sight`].
    pub fn sighted_stamp(&self, path: &Path) -> Result<Option<String>, String> {
        self.db.query_row("select stamp from sighted where path = ?1", [path.to_string_lossy()], |r| r.get(0)).optional().map_err(err)
    }

    /// Keeps what sight made of a file, in place of whatever it made of
    /// it before: for each moment looked at (0 for a photo), the whole
    /// picture's embedding and each face's. They are kept a byte a number,
    /// which is all the telling apart needs and a quarter the room.
    pub fn set_sights(&mut self, source: &Path, stamp: &str, seen: &[(f32, Vec<f32>, Vec<Vec<f32>>)]) -> Result<(), String> {
        let name = source.to_string_lossy();
        let tx = self.db.savepoint().map_err(err)?;
        tx.execute("delete from sights where source = ?1", [&*name]).map_err(err)?;
        {
            let mut put = tx.prepare_cached("insert into sights(source, at, face, vector) values (?1, ?2, ?3, ?4)").map_err(err)?;
            for (at, scene, faces) in seen {
                put.execute(params![&*name, *at as f64, 0, bytes_of(scene)]).map_err(err)?;
                for face in faces {
                    put.execute(params![&*name, *at as f64, 1, bytes_of(face)]).map_err(err)?;
                }
            }
        }
        tx.execute("insert into sighted(path, stamp) values (?1, ?2) on conflict(path) do update set stamp = ?2", params![&*name, stamp]).map_err(err)?;
        tx.commit().map_err(err)
    }

    /// How many files sight has taken in, and how many faces it has found in them.
    pub fn sight_counts(&self) -> Result<(u32, u32), String> {
        let count = |sql: &str| self.db.query_row(sql, [], |r| r.get::<_, i64>(0)).map(|n| n as u32).map_err(err);
        Ok((count("select count(*) from sighted")?, count("select count(*) from sights where face = 1")?))
    }

    /// The files that look most like a picture: by the whole of it
    /// (`scene`), and by the faces in it. Every embedding sight has kept
    /// is held against it, which for a couple of hundred thousand of them
    /// a byte a number is a fraction of a second. The best first, at most
    /// `most`; each with how alike the whole picture was and how alike the
    /// nearest face, of 1 (nothing where there was none to compare).
    pub fn search_sights(&self, scene: Option<&[f32]>, faces: &[Vec<f32>], most: usize) -> Result<Vec<SightMatch>, String> {
        let mut best: std::collections::HashMap<String, (f32, f32)> = std::collections::HashMap::new();
        let mut st = self.db.prepare_cached("select source, face, vector from sights").map_err(err)?;
        let mut rows = st.query([]).map_err(err)?;
        while let Some(row) = rows.next().map_err(err)? {
            let is_face = row.get::<_, i64>(1).map_err(err)? != 0;
            let theirs = row.get_ref(2).map_err(err)?.as_blob().map_err(|e| e.to_string())?;
            let how = if is_face {
                faces.iter().map(|f| alike_bytes(f, theirs)).fold(0.0f32, f32::max)
            } else {
                scene.map_or(0.0, |s| alike_bytes(s, theirs))
            };
            if how < if is_face { FACE_NEAR } else { SCENE_NEAR } {
                continue;
            }
            let source = row.get_ref(0).map_err(err)?.as_str().map_err(|e| e.to_string())?;
            let kept = match best.get_mut(source) {
                Some(kept) => kept,
                None => best.entry(source.to_owned()).or_insert((0.0, 0.0)),
            };
            if is_face { kept.1 = kept.1.max(how) } else { kept.0 = kept.0.max(how) }
        }
        let mut found: Vec<SightMatch> = best.into_iter().map(|(source, (scene, face))| SightMatch { source: PathBuf::from(source), scene, face }).collect();
        found.sort_by(|a, b| b.score().total_cmp(&a.score()).then_with(|| a.source.cmp(&b.source)));
        found.truncate(most);
        Ok(found)
    }

    /// What is remembered of a file: the first memory of it.
    pub fn memory_of(&self, source: &Path) -> Result<Option<Memory>, String> {
        let mut st = self.db.prepare_cached(&format!("select {MEMORY} from memories where source = ?1 order by part, id limit 1")).map_err(err)?;
        st.query_row([source.to_string_lossy()], memory).optional().map_err(err)
    }

    /// Drops words no memory is about any more.
    pub fn tidy_words(&mut self) -> Result<(), String> {
        let tx = self.db.savepoint().map_err(err)?;
        tx.execute("delete from word_vectors where rowid in (select id from words where id not in (select word from memory_words))", []).map_err(err)?;
        tx.execute("delete from words where id not in (select word from memory_words)", []).map_err(err)?;
        tx.commit().map_err(err)
    }

    /// What a file looked like when it was last read: see [`crate::index::stamp`].
    pub fn file_stamp(&self, path: &Path) -> Result<Option<String>, String> {
        self.db.query_row("select stamp from files where path = ?1", [path.to_string_lossy()], |r| r.get(0)).optional().map_err(err)
    }

    /// Notes that a file has been read, and how many memories came of it.
    pub fn set_file(&mut self, path: &Path, stamp: &str, memories: u32) -> Result<(), String> {
        self.db.execute("insert into files(path, stamp, memories) values (?1, ?2, ?3) on conflict(path) do update set stamp = ?2, memories = ?3", params![path.to_string_lossy(), stamp, memories]).map(|_| ()).map_err(err)
    }

    /// Every file that has been read.
    pub fn files(&self) -> Result<Vec<PathBuf>, String> {
        let mut st = self.db.prepare("select path from files order by path").map_err(err)?;
        st.query_map([], |r| r.get::<_, String>(0).map(PathBuf::from)).map_err(err)?.collect::<Result<_, _>>().map_err(err)
    }

    pub fn stats(&self) -> Result<Stats, String> {
        let mut stats = Stats::default();
        // Both counts in one pass over the index of kinds.
        let mut st = self.db.prepare("select kind, count(*), sum(light != 0) from memories group by kind").map_err(err)?;
        for row in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))).map_err(err)? {
            let (kind, n, light) = row.map_err(err)?;
            if let Some(i) = Kind::ALL.iter().position(|k| k.name() == kind) {
                stats.memories[i] = n as u32;
            }
            stats.light += light as u32;
        }
        stats.words = self.db.query_row("select count(*) from words", [], |r| r.get::<_, i64>(0)).map_err(err)? as u32;
        stats.files = self.db.query_row("select count(*) from files", [], |r| r.get::<_, i64>(0)).map_err(err)? as u32;
        Ok(stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("neo-apollo-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("memory.db")
    }

    const KEY: [u8; 32] = [7; 32];

    fn words(w: &[&str]) -> Vec<String> {
        w.iter().map(|s| (*s).to_owned()).collect()
    }

    fn new<'a>(kind: Kind, source: Option<&'a Path>, title: &'a str, text: &'a str, words: &'a [String]) -> New<'a> {
        New { kind, source, title, text, part: 0, created: 100, words }
    }

    #[test]
    fn what_is_remembered_is_found_by_meaning_and_by_kind() {
        let path = scratch("search");
        let mut s = Store::open(&path, &KEY, 3).unwrap();
        let (dog, beach) = (words(&["dog", "beach"]), words(&["beach"]));
        let a = s.add(&new(Kind::Photo, Some(Path::new("/p/dog.jpg")), "dog.jpg", "A dog running on a beach", &dog), &[1.0, 0.0, 0.0]).unwrap();
        let b = s.add(&new(Kind::Video, Some(Path::new("/v/sea.mov")), "sea.mov", "Waves on a beach", &beach), &[0.8, 0.6, 0.0]).unwrap();
        let c = s.add(&new(Kind::Document, None, "Invoice", "Payment due in March", &[]), &[0.0, 0.0, 1.0]).unwrap();
        let found = s.search(&[0.9, 0.1, 0.0], 3, None).unwrap();
        assert_eq!(found.iter().map(|h| h.memory.id).collect::<Vec<_>>(), [a, b, c], "nearest in meaning first");
        assert!(found[0].distance < 0.05 && found[2].distance > 0.9);
        assert_eq!(found[0].memory, Memory { id: a, kind: Kind::Photo, source: Some("/p/dog.jpg".into()), title: "dog.jpg".into(), text: "A dog running on a beach".into(), part: 0, created: 100, light: false });
        assert_eq!(s.search(&[0.9, 0.1, 0.0], 3, Some(Kind::Video)).unwrap().iter().map(|h| h.memory.id).collect::<Vec<_>>(), [b]);
        assert_eq!(s.search(&[0.9, 0.1, 0.0], 1, None).unwrap().len(), 1);
        assert_eq!(s.about("beach", 10, None).unwrap().iter().map(|m| m.id).collect::<Vec<_>>(), [b, a], "newest first");
        assert_eq!(s.words_of(a).unwrap(), ["beach", "dog"]);
        assert_eq!(s.recent(2, None).unwrap().iter().map(|m| m.id).collect::<Vec<_>>(), [c, b]);
        assert_eq!(s.recent(5, Some(Kind::Photo)).unwrap().len(), 1);
        // By when, and of what kind.
        let old = s.add(&New { created: 50, ..new(Kind::Video, Some(Path::new("/v/old.mov")), "old.mov", "An old video", &[]) }, &[0.0, 1.0, 0.0]).unwrap();
        s.add(&New { created: 100, part: 1, ..new(Kind::Video, Some(Path::new("/v/sea.mov")), "sea.mov", "A second passage", &[]) }, &[0.0, 1.0, 0.0]).unwrap();
        let ids = |found: (Vec<Memory>, u32)| (found.0.iter().map(|m| m.id).collect::<Vec<_>>(), found.1);
        assert_eq!(ids(s.between(Some(Kind::Video), None, None, 10).unwrap()), (vec![b, old], 2), "newest first, each file once");
        assert_eq!(ids(s.between(Some(Kind::Video), Some(60), None, 10).unwrap()), (vec![b], 1));
        assert_eq!(ids(s.between(None, None, Some(100), 10).unwrap()), (vec![old], 1), "up to, not including");
        assert_eq!(ids(s.between(None, Some(100), Some(101), 2).unwrap()), (vec![c, b], 3), "no more than asked for, but all counted");
        s.forget(old).unwrap();
        s.forget_source(Path::new("/v/sea.mov")).unwrap();
        let b = s.add(&new(Kind::Video, Some(Path::new("/v/sea.mov")), "sea.mov", "Waves on a beach", &beach), &[0.8, 0.6, 0.0]).unwrap();
        let c2 = s.recent(2, None).unwrap();
        assert_eq!(c2[0].id, b);
        let stats = s.stats().unwrap();
        assert_eq!((stats.total(), stats.of(Kind::Photo), stats.of(Kind::Note), stats.words), (3, 1, 0, 2));
        // One only listed is marked so, counted, and found among those waiting to be looked at.
        s.mark_light(a).unwrap();
        assert_eq!((s.stats().unwrap().light, s.light_files(5).unwrap(), s.recent(5, Some(Kind::Photo)).unwrap()[0].light), (1, vec![PathBuf::from("/p/dog.jpg")], true));
        // A recording with more to listen to is kept note of, and how far it has got.
        s.set_unheard(Path::new("/v/sea.mov"), 1200).unwrap();
        s.set_unheard(Path::new("/v/long.mov"), 2400).unwrap();
        s.set_unheard(Path::new("/v/sea.mov"), 3600).unwrap();
        assert_eq!((s.unheard(5).unwrap(), s.unheard(1).unwrap().len(), s.unheard_count().unwrap()), (vec![(PathBuf::from("/v/sea.mov"), 3600), (PathBuf::from("/v/long.mov"), 2400)], 1, 2));
        assert_eq!(s.first_of(Path::new("/v/sea.mov")).unwrap().map(|(m, next)| (m.title, next)), Some(("sea.mov".to_owned(), 1)));
        assert_eq!(s.first_of(Path::new("/nowhere")).unwrap(), None);
        s.all_heard(Path::new("/v/long.mov")).unwrap();
        s.all_heard(Path::new("/v/sea.mov")).unwrap();
        assert_eq!(s.unheard_count().unwrap(), 0);
        // An embedding of the wrong length is refused, not stored askew.
        assert!(s.add(&new(Kind::Note, None, "x", "y", &[]), &[1.0]).is_err());
        assert!(s.search(&[1.0], 1, None).is_err());
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn the_file_is_unreadable_without_its_key() {
        let path = scratch("key");
        {
            let mut s = Store::open(&path, &KEY, 3).unwrap();
            s.add(&new(Kind::Note, Some(Path::new("/secret/place.txt")), "A secret title", "the secret text of the note", &words(&["secretword"])), &[1.0, 0.0, 0.0]).unwrap();
        }
        // Nothing that was written can be read in the file, in any of its parts.
        for entry in std::fs::read_dir(path.parent().unwrap()).unwrap() {
            let raw = std::fs::read(entry.unwrap().path()).unwrap();
            let has = |needle: &[u8]| raw.windows(needle.len()).any(|w| w == needle);
            assert!(!has(b"SQLite format 3") && !has(b"secret") && !has(b"memories") && !has(b"place.txt"));
        }
        assert!(Store::open(&path, &[8; 32], 3).is_err(), "another key does not open it");
        assert_eq!(Store::made_for(&path, &[8; 32]), None);
        assert_eq!(Store::made_for(&path, &KEY), Some(3));
        let s = Store::open(&path, &KEY, 3).unwrap();
        assert_eq!(s.recent(1, None).unwrap()[0].title, "A secret title");
        // Made for one model's embeddings, it says so when given another's.
        drop(s);
        assert!(Store::open(&path, &KEY, 4).err().unwrap().contains("3 long"));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn the_cloud_weighs_words_and_folds_those_that_mean_the_same() {
        let path = scratch("cloud");
        let mut s = Store::open(&path, &KEY, 3).unwrap();
        let mut add = |ws: &[&str]| {
            let ws = words(ws);
            s.add(&new(Kind::Photo, None, "t", "x", &ws), &[1.0, 0.0, 0.0]).unwrap();
        };
        for _ in 0..4 {
            add(&["dog", "photo"]);
        }
        add(&["dogs", "photo"]);
        add(&["dogs", "photo", "beach"]);
        add(&["beach", "photo"]);
        add(&["invoice", "photo"]);
        // Before the words have embeddings, each stands for itself.
        assert_eq!(s.words_without_vectors(&words(&["dog", "dogs", "dog", "nothing"])).unwrap(), ["dog", "dogs", "nothing"]);
        assert_eq!(s.cloud(10, None).unwrap().len(), 5);
        s.set_word_vector("dog", &[1.0, 0.0, 0.0]).unwrap();
        s.set_word_vector("dogs", &[0.98, 0.1, 0.0]).unwrap();
        s.set_word_vector("beach", &[0.0, 1.0, 0.0]).unwrap();
        s.set_word_vector("invoice", &[0.0, 0.0, 1.0]).unwrap();
        s.set_word_vector("photo", &[0.5, 0.5, 0.7]).unwrap();
        assert_eq!(s.words_without_vectors(&words(&["dog", "beach"])).unwrap(), Vec::<String>::new());
        assert_eq!(s.word_vector("beach").unwrap(), Some(vec![0.0, 1.0, 0.0]));
        assert_eq!(s.word_vector("nothing").unwrap(), None);
        let cloud = s.cloud(10, None).unwrap();
        let dog = cloud.iter().find(|w| w.word == "dog").unwrap();
        assert_eq!((dog.count, dog.also.as_slice()), (6, ["dogs".to_owned()].as_slice()), "dogs are counted with dog");
        assert!(cloud.iter().all(|w| w.word != "dogs"));
        assert_eq!((cloud[0].word.as_str(), cloud[0].weight), ("dog", 1.0), "the word in every memory does not come first");
        let weight = |word: &str| cloud.iter().find(|w| w.word == word).unwrap().weight;
        assert!(weight("photo") < weight("beach") && weight("beach") > weight("invoice"), "being in every memory counts against a word");
        assert_eq!(s.cloud(2, None).unwrap().len(), 2);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_memory_that_says_the_words_asked_for_comes_first() {
        let path = scratch("said");
        let mut s = Store::open(&path, &KEY, 3).unwrap();
        let none: Vec<String> = vec![];
        // Three pictures of women, all much the same by meaning; one a little nearer.
        let jinx = s.add(&new(Kind::Photo, None, "jinx.png", "An illustration of a character with a fierce expression.", &none), &[1.0, 0.05, 0.0]).unwrap();
        let wig = s.add(&new(Kind::Photo, None, "giphy.gif", "A woman singing into a microphone in a bright pink wig.", &words(&["pink hair", "singer"])), &[1.0, 0.25, 0.0]).unwrap();
        let hat = s.add(&new(Kind::Photo, None, "hat.jpg", "A girl in a pink hat.", &none), &[1.0, 0.3, 0.0]).unwrap();
        // Far off by meaning, but it says every word.
        let note = s.add(&new(Kind::Note, None, "A note", "The girl next door dyed her hair pink.", &none), &[0.0, 0.2, 1.0]).unwrap();
        let ask = [1.0, 0.0, 0.0];
        let order = |terms: &[&str], most, kind| s.search_for(&ask, &words(terms), most, kind).unwrap().iter().map(|h| h.memory.id).collect::<Vec<_>>();
        assert_eq!(s.search(&ask, 3, None).unwrap()[0].memory.id, jinx, "by meaning alone, the nearest");
        assert_eq!(order(&["girl", "pink", "hair"], 3, None), [wig, hat, jinx], "saying pink and hair (in its words) beats saying girl and pink, which beats saying none");
        assert_eq!(order(&[], 1, None), [jinx], "with no words to go on, meaning alone");
        // One that says them all is found though far off, yet not put above what is both near and says most.
        let all = order(&["girl", "pink", "hair"], 10, None);
        assert!(all.contains(&note) && all[0] == wig, "{all:?}");
        assert_eq!(order(&["girl", "pink", "hair"], 10, Some(Kind::Note)), [note]);
        assert!(!order(&["pink"], 10, Some(Kind::Photo)).contains(&note));
        // The distance given is still the true one.
        let hits = s.search_for(&ask, &words(&["pink"]), 4, None).unwrap();
        assert!(hits.iter().all(|h| (0.0..=1.0).contains(&h.distance)) && hits.iter().any(|h| h.memory.id == note && h.distance > 0.9));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_memory_keeps_to_the_model_it_began_with() {
        use crate::model::fake::Fake;
        struct Other;
        impl crate::Model for Other {
            fn name(&self) -> String {
                "other".into()
            }
            fn dims(&self) -> Result<usize, String> {
                Fake::default().dims()
            }
            fn embed(&self, texts: &[String], question: bool) -> Result<Vec<Vec<f32>>, String> {
                Fake::default().embed(texts, question)
            }
            fn describe(&self, jpeg: &[u8]) -> Result<crate::model::Seen, String> {
                Fake::default().describe(jpeg)
            }
            fn chat(&self, turns: &[crate::model::Turn], piece: &mut dyn FnMut(&str) -> bool) -> Result<String, String> {
                Fake::default().chat(turns, piece)
            }
        }
        let path = scratch("model");
        drop(Store::open_for(&path, &KEY, &Fake::default()).unwrap());
        assert!(Store::open_for(&path, &KEY, &Fake::default()).is_ok(), "the same model again");
        let refused = Store::open_for(&path, &KEY, &Other).err().unwrap();
        assert_eq!(refused, "The memory was made with fake, and other is chosen now.", "same length, but not the same embeddings");
        assert!(refused.starts_with(OTHER_MODEL) && Store::open(&path, &KEY, 3).err().unwrap().starts_with(OTHER_MODEL));
        // Removed, it can be begun afresh with the other.
        Store::delete(&path).unwrap();
        Store::delete(&path).unwrap();
        assert!(!path.exists() && Store::open_for(&path, &KEY, &Other).is_ok());
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_word_has_the_words_that_go_with_it() {
        let path = scratch("related");
        let mut s = Store::open(&path, &KEY, 3).unwrap();
        for ws in [&["dog", "beach", "sea"][..], &["dog", "beach"], &["dogs", "park"], &["invoice", "bank"], &["dog"]] {
            s.add(&new(Kind::Photo, None, "t", "x", &words(ws)), &[1.0, 0.0, 0.0]).unwrap();
        }
        let names = |r: Vec<Related>| r.into_iter().map(|r| (r.word, r.shared)).collect::<Vec<_>>();
        assert_eq!(names(s.related("dog", &[], 10, None).unwrap()), [("beach".into(), 2), ("sea".into(), 1)], "those that share most memories first");
        assert_eq!(names(s.related("dog", &words(&["dogs"]), 10, None).unwrap()), [("beach".into(), 2), ("park".into(), 1), ("sea".into(), 1)], "with the words folded into it");
        assert_eq!(s.related("dog", &[], 1, None).unwrap().len(), 1);
        // Among one kind's memories only.
        s.add(&new(Kind::Video, None, "t", "x", &words(&["dog", "trail"])), &[1.0, 0.0, 0.0]).unwrap();
        assert_eq!(names(s.related("dog", &[], 10, Some(Kind::Video)).unwrap()), [("trail".into(), 1)]);
        assert_eq!(s.cloud(10, Some(Kind::Video)).unwrap().iter().map(|w| w.word.as_str()).collect::<Vec<_>>(), ["dog", "trail"], "and the cloud of one kind is of its words");
        assert!(s.cloud(10, Some(Kind::Note)).unwrap().is_empty());
        assert_eq!(names(s.related("dog", &[], 10, None).unwrap())[0], ("beach".into(), 2));
        let video = s.recent(1, Some(Kind::Video)).unwrap()[0].id;
        s.forget(video).unwrap();
        s.tidy_words().unwrap();
        assert_eq!(s.related("nothing", &[], 5, None).unwrap(), vec![]);
        // With room left, words near in meaning that share no memory come after.
        s.set_word_vector("dog", &[1.0, 0.0, 0.0]).unwrap();
        s.set_word_vector("park", &[0.9, 0.3, 0.0]).unwrap();
        s.set_word_vector("invoice", &[0.0, 0.0, 1.0]).unwrap();
        let with_near = s.related("dog", &[], 10, None).unwrap();
        assert_eq!(with_near.iter().map(|r| r.word.as_str()).collect::<Vec<_>>(), ["beach", "sea", "park"]);
        assert!(with_near[2].shared == 0 && with_near[2].distance.is_some_and(|d| d < 0.1) && with_near[0].distance.is_none());
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_file_can_be_forgotten_and_read_again() {
        let path = scratch("forget");
        let mut s = Store::open(&path, &KEY, 3).unwrap();
        let file = Path::new("/docs/plan.md");
        let ws = words(&["plan"]);
        for part in 0..3 {
            s.add(&New { part, ..new(Kind::Document, Some(file), "plan.md", "text", &ws) }, &[1.0, 0.0, 0.0]).unwrap();
        }
        let other = s.add(&new(Kind::Note, None, "kept", "kept", &words(&["kept"])), &[0.0, 1.0, 0.0]).unwrap();
        assert_eq!(s.file_stamp(file).unwrap(), None);
        s.set_file(file, "10-20", 3).unwrap();
        s.set_file(file, "11-21", 3).unwrap();
        assert_eq!((s.file_stamp(file).unwrap().as_deref(), s.files().unwrap()), (Some("11-21"), vec![file.to_path_buf()]));
        assert_eq!(s.forget_source(file).unwrap(), 3);
        assert_eq!((s.stats().unwrap().total(), s.file_stamp(file).unwrap(), s.search(&[1.0, 0.0, 0.0], 5, None).unwrap().len()), (1, None, 1));
        s.tidy_words().unwrap();
        assert_eq!(s.stats().unwrap().words, 1, "words nothing is about any more go too");
        s.forget(other).unwrap();
        assert!(s.search(&[0.0, 1.0, 0.0], 5, None).unwrap().is_empty());
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}

#[cfg(test)]
mod scale_probe {
    use super::*;

    #[test]
    #[ignore]
    fn forgetting_does_not_slow_as_the_memory_fills() {
        let dir = std::env::temp_dir().join(format!("neo-apollo-scale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dims = 768;
        let mut s = Store::open(&dir.join("memory.db"), &[1; 32], dims).unwrap();
        let vector = |i: usize| -> Vec<f32> { (0..dims).map(|d| ((i * 31 + d * 7) % 97) as f32 / 97.0).collect() };
        for round in 0..3 {
            let base = round * 20_000;
            s.begin().unwrap();
            for i in base..base + 20_000 {
                let path = PathBuf::from(format!("/files/file number {i}.mp3"));
                s.add(&New { kind: Kind::Audio, source: Some(&path), title: "t", text: "An audio recording.", part: 0, created: i as u64, words: &["sound".to_owned()] }, &vector(i)).unwrap();
            }
            s.commit().unwrap();
            s.begin().unwrap();
            let began = std::time::Instant::now();
            for i in 0..200 {
                assert_eq!(s.forget_source(Path::new(&format!("/nowhere/{i}"))).unwrap(), 0);
                assert_eq!(s.forget_source(Path::new(&format!("/files/file number {}.mp3", base + i))).unwrap(), 1);
            }
            println!("with {} memories: forgetting {:.3} ms each", base + 20_000, began.elapsed().as_secs_f64() * 1000.0 / 400.0);
            s.commit().unwrap();
        }
        drop(s);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod near_tests {
    use super::*;

    #[test]
    fn bits_tell_near_from_far_and_the_nearest_middles_come_first() {
        let a = [0.5, -0.2, 0.9, -0.7, 0.1, 0.3, -0.4, 0.8, 0.6];
        let near = [0.4, -0.3, 0.8, -0.6, 0.2, 0.2, -0.5, 0.9, 0.5];
        let far: Vec<f32> = a.iter().map(|x| -x).collect();
        assert_eq!(bits(&a).len(), 2, "nine numbers, nine bits, two bytes");
        assert_eq!((apart(&bits(&a), &bits(&near)), apart(&bits(&a), &bits(&far)), apart(&bits(&a), &bits(&a))), (0, 9, 0));
        // Held whole against another's bits, a question is nearest what points its own way.
        let asked = weigh(&a);
        assert!(unlike(&asked, &bits(&near)) < unlike(&asked, &bits(&far)));
        assert_eq!(unlike(&asked, &bits(&a)), -a.iter().map(|x| x.abs()).sum::<f32>());
        let middles: Vec<Middle> = [(1, far.clone()), (2, near.to_vec()), (3, vec![0.0; 9])].into_iter().map(|(id, vector)| Middle { id, length: length(&vector), vector }).collect();
        assert_eq!(nearest(&middles, &a, 2), [2, 3], "the one like it, then the one that is like nothing");
        assert_eq!(nearest(&middles, &a, 9).len(), 3);
    }
}

#[cfg(test)]
mod real_probe {
    use super::*;

    /// By hand, on this computer's own memory, which it copies and does
    /// not change: what each step of keeping a file costs once the memory
    /// is full. `cargo test --release -p neo-apollo-core real_memory -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn what_each_step_costs_in_the_real_memory() {
        let dir = std::env::temp_dir().join(format!("neo-apollo-real-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["memory.db", "memory.db-wal"] {
            let _ = std::fs::copy(crate::dir().join(name), dir.join(name));
        }
        let key = crate::key::database_key().unwrap();
        let dims = 768;
        let mut s = Store::open(&dir.join("memory.db"), &key, dims).unwrap();
        let count = |s: &Store, table: &str| s.db.query_row(&format!("select count(*) from {table}"), [], |r| r.get::<_, i64>(0)).unwrap();
        println!("memories {}, words {}, memory_words {}, files {}", count(&s, "memories"), count(&s, "words"), count(&s, "memory_words"), count(&s, "files"));
        let vector = |i: usize| -> Vec<f32> { (0..dims).map(|d| ((i * 31 + d * 7) % 97) as f32 / 97.0).collect() };
        let time = |what: &str, s: &mut Store, f: &mut dyn FnMut(&mut Store, usize)| {
            s.begin().unwrap();
            let began = std::time::Instant::now();
            for i in 0..100 {
                f(s, i);
            }
            let took = began.elapsed();
            s.commit().unwrap();
            println!("  {what:<26} {:>8.3} ms each", took.as_secs_f64() * 1000.0 / 100.0);
        };
        time("forget (not there)", &mut s, &mut |s, i| {
            s.forget_source(Path::new(&format!("/nowhere/{i}"))).unwrap();
        });
        time("file_stamp", &mut s, &mut |s, i| {
            s.file_stamp(Path::new(&format!("/nowhere/{i}"))).unwrap();
        });
        time("words_without_vectors", &mut s, &mut |s, i| {
            s.words_without_vectors(&[format!("zzword{i}"), "music".to_owned(), "sound".to_owned(), "album".to_owned()]).unwrap();
        });
        time("add (4 words)", &mut s, &mut |s, i| {
            s.add(&New { kind: Kind::Audio, source: Some(Path::new(&format!("/probe/{i}.mp3"))), title: "t", text: "x", part: 0, created: 1, words: &[format!("zzword{i}"), "music".to_owned(), "sound".to_owned(), "album".to_owned()] }, &vector(i)).unwrap();
        });
        time("set_word_vector (new)", &mut s, &mut |s, i| s.set_word_vector(&format!("zzword{i}"), &vector(i)).unwrap());
        time("set_file", &mut s, &mut |s, i| s.set_file(Path::new(&format!("/probe/{i}.mp3")), "s", 1).unwrap());
        time("mark_light", &mut s, &mut |s, i| s.mark_light(i as i64 + 1).unwrap());
        time("stats", &mut s, &mut |s, _| {
            s.stats().unwrap();
        });
        // What the Memory page asks for when the memory is unlocked: once each.
        let like = s.word_vector("music").unwrap().unwrap_or_else(|| vector(3));
        // What an exact search finds, to hold the index's answers against.
        let began = std::time::Instant::now();
        let exact: Vec<i64> = s.search(&like, 40, None).unwrap().into_iter().map(|h| h.memory.id).collect();
        println!("  exact search, no index     {:>8.1} ms once", began.elapsed().as_secs_f64() * 1000.0);
        let began = std::time::Instant::now();
        let mut said = std::time::Instant::now();
        let indexed = s
            .index_meanings(&mut |done, total| {
                if said.elapsed().as_secs() >= 20 {
                    println!("    placed {done} of {total} after {:.0?}", began.elapsed());
                    said = std::time::Instant::now();
                }
                true
            })
            .unwrap();
        println!("  index of meanings made: {indexed}, in {:.1?}; {} neighbourhoods, {} placed", began.elapsed(), count(&s, "middles"), count(&s, "nearby"));
        let near: Vec<i64> = s.search(&like, 40, None).unwrap().into_iter().map(|h| h.memory.id).collect();
        println!("  of the 40 an exact search finds, the index finds {}; of its first 10, {}", exact.iter().filter(|id| near.contains(id)).count(), exact.iter().take(10).filter(|id| near.contains(id)).count());
        let terms = vec!["music".to_owned()];
        for (what, f) in [
            ("cloud", (&|s: &Store| drop(s.cloud(60, None).unwrap())) as &dyn Fn(&Store)),
            ("recent", &|s: &Store| drop(s.recent(40, None).unwrap())),
            ("related", &|s: &Store| drop(s.related("music", &[], 8, None).unwrap())),
            ("about a word", &|s: &Store| drop(s.about("music", 40, None).unwrap())),
            ("search by meaning", &|s: &Store| drop(s.search(&like, 40, None).unwrap())),
            ("search by meaning, again", &|s: &Store| drop(s.search(&like, 40, None).unwrap())),
            ("search_for (meaning+words)", &|s: &Store| drop(s.search_for(&like, &terms, 40, None).unwrap())),
            ("search of one kind", &|s: &Store| drop(s.search(&like, 40, Some(Kind::Photo)).unwrap())),
        ] {
            let began = std::time::Instant::now();
            f(&s);
            println!("  {what:<26} {:>8.1} ms once", began.elapsed().as_secs_f64() * 1000.0);
        }
        drop(s);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// By hand, on this computer's own memory, which it only reads: how
    /// far sight has got.
    /// `cargo test --release -p neo-apollo-core how_much_sight -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn how_much_sight_has_taken_in() {
        let key = crate::key::database_key().unwrap();
        let s = Store::open(&crate::dir().join("memory.db"), &key, 768).unwrap();
        let (files, faces) = s.sight_counts().unwrap();
        let count = |sql: &str| s.db.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
        println!("sighted {files} files; {} whole pictures and {faces} faces kept; photos and videos remembered: {}", count("select count(*) from sights where face = 0"), count("select count(distinct source) from memories where kind in ('photo', 'video')"));
    }

    /// By hand, on this computer's own memory: what picking a
    /// word or a kind on the Memory page asks of it, step by step.
    /// `cargo test --release -p neo-apollo-core what_a_click_costs -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn what_a_click_costs() {
        // The memory itself, which this only reads: a copy taken while
        // Apollo is writing to it is as likely as not to be a broken one.
        let key = crate::key::database_key().unwrap();
        let s = Store::open(&crate::dir().join("memory.db"), &key, 768).unwrap();
        let timed = |what: &str, f: &dyn Fn()| {
            let began = std::time::Instant::now();
            f();
            println!("  {what:<34} {:>9.1} ms", began.elapsed().as_secs_f64() * 1000.0);
        };
        timed("stats", &|| drop(s.stats().unwrap()));
        timed("cloud, every kind", &|| drop(s.cloud(60, None).unwrap()));
        timed("cloud, every kind, again", &|| drop(s.cloud(60, None).unwrap()));
        for kind in Kind::ALL {
            timed(&format!("cloud of {}", kind.name()), &|| drop(s.cloud(60, Some(kind)).unwrap()));
            timed(&format!("recent of {}", kind.name()), &|| drop(s.recent(40, Some(kind)).unwrap()));
        }
        let cloud = s.cloud(60, None).unwrap();
        for w in cloud.iter().take(4).chain(cloud.iter().skip(30).take(2)) {
            println!("{} ({} memories, with {} folded in)", w.word, w.count, w.also.len());
            timed("about", &|| {
                for word in std::iter::once(&w.word).chain(&w.also) {
                    drop(s.about(word, 40, None).unwrap());
                }
            });
            timed("related", &|| drop(s.related(&w.word, &w.also, 8, None).unwrap()));
            timed("related, of photos", &|| drop(s.related(&w.word, &w.also, 8, Some(Kind::Photo)).unwrap()));
            timed("near it in meaning", &|| {
                if let Some(v) = s.word_vector(&w.word).unwrap() {
                    drop(s.search(&v, 40, None).unwrap());
                }
            });
            timed("near it, of photos", &|| {
                if let Some(v) = s.word_vector(&w.word).unwrap() {
                    let found = s.search(&v, 81, Some(Kind::Photo)).unwrap();
                    let videos = s.search(&v, 81, Some(Kind::Video)).unwrap();
                    print!("  (81 asked for: {} photos, {} videos)", found.len(), videos.len());
                }
            });
            if let (Some(v), Some(middles)) = (s.word_vector(&w.word).unwrap(), s.middles().unwrap()) {
                let exact: Vec<i64> = s.search_every(&v, 40, None).unwrap().into_iter().map(|h| h.memory.id).collect();
                for shortlist in [60, 120, 250, 500] {
                    let began = std::time::Instant::now();
                    let near: Vec<i64> = s.search_near(&middles, &v, 40, None, shortlist).unwrap().into_iter().map(|h| h.memory.id).collect();
                    println!("  {shortlist:>4} compared in full: {:>7.1} ms, {} of the exact 40, {} of its first 10", began.elapsed().as_secs_f64() * 1000.0, exact.iter().filter(|id| near.contains(id)).count(), exact.iter().take(10).filter(|id| near.contains(id)).count());
                }
            }
            let terms = vec![w.word.clone()];
            timed("saying", &|| drop(s.saying(&terms, 40, None).unwrap()));
            timed("saying, of photos", &|| drop(s.saying(&terms, 40, Some(Kind::Photo)).unwrap()));
            if let Some(v) = s.word_vector(&w.word).unwrap() {
                timed("search_for", &|| drop(s.search_for(&v, &terms, 40, None).unwrap()));
            }
        }
    }
}
