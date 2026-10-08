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
}

impl Kind {
    pub const ALL: [Kind; 5] = [Kind::Photo, Kind::Video, Kind::Document, Kind::Conversation, Kind::Note];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Photo => "photo",
            Kind::Video => "video",
            Kind::Document => "document",
            Kind::Conversation => "conversation",
            Kind::Note => "note",
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
    pub memories: [u32; 5],
    pub words: u32,
    pub files: u32,
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
}

/// Words closer than this mean the same for the cloud's purposes: "dog"
/// and "dogs", "beach" and "seaside".
const SAME_WORD: f32 = 0.1;
/// Words closer than this are near enough in meaning to go together.
const NEAR_WORD: f32 = 0.3;
/// How many of the commonest words are weighed for the cloud.
const CLOUD_POOL: usize = 400;

fn blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn floats(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
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

const MEMORY: &str = "id, kind, source, title, text, part, created";

fn memory(row: &rusqlite::Row) -> rusqlite::Result<Memory> {
    Ok(Memory { id: row.get(0)?, kind: Kind::from_name(&row.get::<_, String>(1)?).unwrap_or(Kind::Note), source: row.get::<_, Option<String>>(2)?.map(PathBuf::from), title: row.get(3)?, text: row.get(4)?, part: row.get(5)?, created: row.get::<_, i64>(6)? as u64 })
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
        db.busy_timeout(std::time::Duration::from_secs(10)).map_err(err)?;
        db.execute_batch("create table if not exists meta(key text primary key, value text not null);").map_err(err)?;
        let made_for: Option<usize> = db.query_row("select value from meta where key = 'dims'", [], |r| r.get::<_, String>(0)).optional().map_err(err)?.and_then(|v| v.parse().ok());
        match made_for {
            Some(d) if d != dims => return Err(format!("The memory was made with a model whose embeddings are {d} long, and this one's are {dims}.")),
            Some(_) => {}
            None => {
                db.execute("insert into meta(key, value) values ('dims', ?1)", [dims.to_string()]).map_err(err)?;
            }
        }
        db.execute_batch(&format!(
            "create table if not exists memories(id integer primary key, kind text not null, source text, title text not null, text text not null, part integer not null default 0, created integer not null);
             create index if not exists memories_source on memories(source);
             create virtual table if not exists memory_vectors using vec0(embedding float[{dims}] distance_metric=cosine);
             create table if not exists words(id integer primary key, word text unique not null);
             create virtual table if not exists word_vectors using vec0(embedding float[{dims}] distance_metric=cosine);
             create table if not exists memory_words(memory integer not null, word integer not null, primary key(memory, word)) without rowid;
             create index if not exists memory_words_word on memory_words(word);
             create table if not exists files(path text primary key, stamp text not null, memories integer not null);"
        ))
        .map_err(err)?;
        Ok(Self { db, dims })
    }

    pub fn dims(&self) -> usize {
        self.dims
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
        let tx = self.db.transaction().map_err(err)?;
        tx.execute("insert into memories(kind, source, title, text, part, created) values (?1, ?2, ?3, ?4, ?5, ?6)", params![new.kind.name(), new.source.map(|p| p.to_string_lossy().into_owned()), new.title, new.text, new.part, new.created as i64]).map_err(err)?;
        let id = tx.last_insert_rowid();
        tx.execute("insert into memory_vectors(rowid, embedding) values (?1, ?2)", params![id, blob(embedding)]).map_err(err)?;
        for word in new.words {
            tx.execute("insert or ignore into words(word) values (?1)", [word]).map_err(err)?;
            tx.execute("insert or ignore into memory_words(memory, word) select ?1, id from words where word = ?2", params![id, word]).map_err(err)?;
        }
        tx.commit().map_err(err)?;
        Ok(id)
    }

    /// Those of `words` that have no embedding yet.
    pub fn words_without_vectors(&self, words: &[String]) -> Result<Vec<String>, String> {
        let mut known = self.db.prepare("select 1 from words w join word_vectors v on v.rowid = w.id where w.word = ?1").map_err(err)?;
        let mut out: Vec<String> = vec![];
        for word in words {
            if !out.contains(word) && !known.exists([word]).map_err(err)? {
                out.push(word.clone());
            }
        }
        Ok(out)
    }

    /// Gives a word its embedding, which is what lets the cloud tell that
    /// two words mean the same, and a click on one find what is near it.
    pub fn set_word_vector(&mut self, word: &str, embedding: &[f32]) -> Result<(), String> {
        if embedding.len() != self.dims {
            return Err(format!("An embedding {} long was given to a memory of {}.", embedding.len(), self.dims));
        }
        let tx = self.db.transaction().map_err(err)?;
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
        // The nearest are found first and the kind picked out after, so
        // more are asked for when only some will do.
        let reach = if kind.is_some() { most * 8 } else { most }.clamp(1, 4000);
        let mut near = self.db.prepare(&format!("select {}, v.distance from memory_vectors v join memories m on m.id = v.rowid where v.embedding match ?1 and k = ?2 order by v.distance", MEMORY.split(", ").map(|c| format!("m.{c}")).collect::<Vec<_>>().join(", "))).map_err(err)?;
        let hits = near.query_map(params![blob(embedding), reach as i64], |r| Ok(Hit { memory: memory(r)?, distance: r.get::<_, f64>(7)? as f32 })).map_err(err)?;
        let mut out = vec![];
        for hit in hits {
            let hit = hit.map_err(err)?;
            if kind.is_none_or(|k| k == hit.memory.kind) && out.len() < most {
                out.push(hit);
            }
        }
        Ok(out)
    }

    /// The memories that are about `word`, newest first.
    pub fn about(&self, word: &str, most: usize) -> Result<Vec<Memory>, String> {
        let mut st = self.db.prepare(&format!("select {} from memories m join memory_words mw on mw.memory = m.id join words w on w.id = mw.word where w.word = ?1 order by m.created desc, m.id desc limit ?2", MEMORY.split(", ").map(|c| format!("m.{c}")).collect::<Vec<_>>().join(", "))).map_err(err)?;
        st.query_map(params![word, most as i64], memory).map_err(err)?.collect::<Result<_, _>>().map_err(err)
    }

    /// The newest memories, of one kind if `kind` is given.
    pub fn recent(&self, most: usize, kind: Option<Kind>) -> Result<Vec<Memory>, String> {
        let mut st = self.db.prepare(&format!("select {MEMORY} from memories where ?1 is null or kind = ?1 order by created desc, id desc limit ?2")).map_err(err)?;
        st.query_map(params![kind.map(Kind::name), most as i64], memory).map_err(err)?.collect::<Result<_, _>>().map_err(err)
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
    pub fn cloud(&self, most: usize) -> Result<Vec<Word>, String> {
        let total: f32 = self.db.query_row("select count(*) from memories", [], |r| r.get::<_, i64>(0)).map_err(err)? as f32;
        let mut st = self.db.prepare("select w.word, count(*) as n, (select embedding from word_vectors v where v.rowid = w.id) from words w join memory_words mw on mw.word = w.id group by w.id order by n desc, w.word limit ?1").map_err(err)?;
        let pool: Vec<(String, u32, Option<Vec<f32>>)> = st.query_map([CLOUD_POOL as i64], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u32, r.get::<_, Option<Vec<u8>>>(2)?.map(|b| floats(&b))))).map_err(err)?.collect::<Result<_, _>>().map_err(err)?;
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
    pub fn related(&self, word: &str, also: &[String], most: usize) -> Result<Vec<Related>, String> {
        let mut names: Vec<&str> = vec![word];
        names.extend(also.iter().map(String::as_str));
        let marks = vec!["?"; names.len()].join(", ");
        let mut st = self
            .db
            .prepare(&format!(
                "select w2.word, count(distinct mw2.memory) as n from words w1 join memory_words mw1 on mw1.word = w1.id join memory_words mw2 on mw2.memory = mw1.memory join words w2 on w2.id = mw2.word
                 where w1.word in ({marks}) and w2.word not in ({marks}) group by w2.id order by n desc, w2.word limit {most}"
            ))
            .map_err(err)?;
        let twice: Vec<&str> = names.iter().chain(names.iter()).copied().collect();
        let mut out: Vec<Related> = st.query_map(rusqlite::params_from_iter(twice), |r| Ok(Related { word: r.get(0)?, shared: r.get::<_, i64>(1)? as u32, distance: None })).map_err(err)?.collect::<Result<_, _>>().map_err(err)?;
        if out.len() < most
            && let Some(vector) = self.word_vector(word)?
        {
            let mut near = self.db.prepare("select w.word, v.distance from word_vectors v join words w on w.id = v.rowid where v.embedding match ?1 and k = ?2 order by v.distance").map_err(err)?;
            let found: Vec<(String, f32)> = near.query_map(params![blob(&vector), (most * 2 + names.len()) as i64], |r| Ok((r.get(0)?, r.get::<_, f64>(1)? as f32))).map_err(err)?.collect::<Result<_, _>>().map_err(err)?;
            for (w, distance) in found {
                if out.len() < most && distance < NEAR_WORD && !names.contains(&w.as_str()) && !out.iter().any(|r| r.word == w) {
                    out.push(Related { word: w, shared: 0, distance: Some(distance) });
                }
            }
        }
        Ok(out)
    }

    /// Forgets everything remembered of a file.
    pub fn forget_source(&mut self, source: &Path) -> Result<u32, String> {
        let source = source.to_string_lossy();
        let tx = self.db.transaction().map_err(err)?;
        tx.execute("delete from memory_vectors where rowid in (select id from memories where source = ?1)", [&*source]).map_err(err)?;
        tx.execute("delete from memory_words where memory in (select id from memories where source = ?1)", [&*source]).map_err(err)?;
        let n = tx.execute("delete from memories where source = ?1", [&*source]).map_err(err)?;
        tx.execute("delete from files where path = ?1", [&*source]).map_err(err)?;
        tx.commit().map_err(err)?;
        Ok(n as u32)
    }

    /// Forgets one memory.
    pub fn forget(&mut self, id: i64) -> Result<(), String> {
        let tx = self.db.transaction().map_err(err)?;
        tx.execute("delete from memory_vectors where rowid = ?1", [id]).map_err(err)?;
        tx.execute("delete from memory_words where memory = ?1", [id]).map_err(err)?;
        tx.execute("delete from memories where id = ?1", [id]).map_err(err)?;
        tx.commit().map_err(err)
    }

    /// Drops words no memory is about any more.
    pub fn tidy_words(&mut self) -> Result<(), String> {
        let tx = self.db.transaction().map_err(err)?;
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
        let mut st = self.db.prepare("select kind, count(*) from memories group by kind").map_err(err)?;
        for row in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))).map_err(err)? {
            let (kind, n) = row.map_err(err)?;
            if let Some(i) = Kind::ALL.iter().position(|k| k.name() == kind) {
                stats.memories[i] = n as u32;
            }
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
        assert_eq!(found[0].memory, Memory { id: a, kind: Kind::Photo, source: Some("/p/dog.jpg".into()), title: "dog.jpg".into(), text: "A dog running on a beach".into(), part: 0, created: 100 });
        assert_eq!(s.search(&[0.9, 0.1, 0.0], 3, Some(Kind::Video)).unwrap().iter().map(|h| h.memory.id).collect::<Vec<_>>(), [b]);
        assert_eq!(s.search(&[0.9, 0.1, 0.0], 1, None).unwrap().len(), 1);
        assert_eq!(s.about("beach", 10).unwrap().iter().map(|m| m.id).collect::<Vec<_>>(), [b, a], "newest first");
        assert_eq!(s.words_of(a).unwrap(), ["beach", "dog"]);
        assert_eq!(s.recent(2, None).unwrap().iter().map(|m| m.id).collect::<Vec<_>>(), [c, b]);
        assert_eq!(s.recent(5, Some(Kind::Photo)).unwrap().len(), 1);
        let stats = s.stats().unwrap();
        assert_eq!((stats.total(), stats.of(Kind::Photo), stats.of(Kind::Note), stats.words), (3, 1, 0, 2));
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
        assert_eq!(s.cloud(10).unwrap().len(), 5);
        s.set_word_vector("dog", &[1.0, 0.0, 0.0]).unwrap();
        s.set_word_vector("dogs", &[0.98, 0.1, 0.0]).unwrap();
        s.set_word_vector("beach", &[0.0, 1.0, 0.0]).unwrap();
        s.set_word_vector("invoice", &[0.0, 0.0, 1.0]).unwrap();
        s.set_word_vector("photo", &[0.5, 0.5, 0.7]).unwrap();
        assert_eq!(s.words_without_vectors(&words(&["dog", "beach"])).unwrap(), Vec::<String>::new());
        assert_eq!(s.word_vector("beach").unwrap(), Some(vec![0.0, 1.0, 0.0]));
        assert_eq!(s.word_vector("nothing").unwrap(), None);
        let cloud = s.cloud(10).unwrap();
        let dog = cloud.iter().find(|w| w.word == "dog").unwrap();
        assert_eq!((dog.count, dog.also.as_slice()), (6, ["dogs".to_owned()].as_slice()), "dogs are counted with dog");
        assert!(cloud.iter().all(|w| w.word != "dogs"));
        assert_eq!((cloud[0].word.as_str(), cloud[0].weight), ("dog", 1.0), "the word in every memory does not come first");
        let weight = |word: &str| cloud.iter().find(|w| w.word == word).unwrap().weight;
        assert!(weight("photo") < weight("beach") && weight("beach") > weight("invoice"), "being in every memory counts against a word");
        assert_eq!(s.cloud(2).unwrap().len(), 2);
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
        assert_eq!(names(s.related("dog", &[], 10).unwrap()), [("beach".into(), 2), ("sea".into(), 1)], "those that share most memories first");
        assert_eq!(names(s.related("dog", &words(&["dogs"]), 10).unwrap()), [("beach".into(), 2), ("park".into(), 1), ("sea".into(), 1)], "with the words folded into it");
        assert_eq!(s.related("dog", &[], 1).unwrap().len(), 1);
        assert_eq!(s.related("nothing", &[], 5).unwrap(), vec![]);
        // With room left, words near in meaning that share no memory come after.
        s.set_word_vector("dog", &[1.0, 0.0, 0.0]).unwrap();
        s.set_word_vector("park", &[0.9, 0.3, 0.0]).unwrap();
        s.set_word_vector("invoice", &[0.0, 0.0, 1.0]).unwrap();
        let with_near = s.related("dog", &[], 10).unwrap();
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
