//! Keeping an index between runs, so a disk measured once does not have
//! to be measured from nothing again.
//!
//! One file per folder measured, in the user's cache folder. It lists the
//! name of every folder under it and of every large file, so it is
//! readable by the user alone.

use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use crate::changes::Mark;
use crate::scan::{Big, Folder, Index, Linked};

/// Changed whenever the layout below does, so an old file is not misread.
const MAGIC: &[u8; 8] = b"NEODISK2";

/// Where indexes are kept.
pub fn folder() -> PathBuf {
    if cfg!(target_os = "macos") {
        neo_desktop::fs::home_dir().join("Library/Caches/org.neo.Disk")
    } else if let Some(dir) = std::env::var_os("XDG_CACHE_HOME").filter(|d| !d.is_empty()) {
        PathBuf::from(dir).join("neo-disk")
    } else if let Some(dir) = std::env::var_os("LOCALAPPDATA").filter(|_| cfg!(windows)) {
        PathBuf::from(dir).join("NeoDisk")
    } else {
        neo_desktop::fs::home_dir().join(".cache/neo-disk")
    }
}

/// The file for the index of `root`: named by a hash of the path, the
/// same on every run.
pub fn file(dir: &Path, root: &Path) -> PathBuf {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in root.as_os_str().as_encoded_bytes() {
        hash = (hash ^ *b as u64).wrapping_mul(0x0000_0100_0000_01b3);
    }
    dir.join(format!("{hash:016x}.index"))
}

fn put_str(w: &mut impl Write, s: &str) -> std::io::Result<()> {
    w.write_all(&(s.len() as u32).to_le_bytes())?;
    w.write_all(s.as_bytes())
}

fn put_folder(w: &mut impl Write, f: &Folder) -> std::io::Result<()> {
    put_str(w, &f.name)?;
    w.write_all(&f.mtime.to_le_bytes())?;
    w.write_all(&[f.denied as u8])?;
    w.write_all(&f.small_size.to_le_bytes())?;
    w.write_all(&f.small_count.to_le_bytes())?;
    w.write_all(&(f.big.len() as u32).to_le_bytes())?;
    for b in &f.big {
        put_str(w, &b.name)?;
        w.write_all(&b.size.to_le_bytes())?;
    }
    w.write_all(&(f.linked.len() as u32).to_le_bytes())?;
    for l in &f.linked {
        w.write_all(&l.id.to_le_bytes())?;
        w.write_all(&l.size.to_le_bytes())?;
        put_str(w, &l.name)?;
    }
    w.write_all(&(f.folders.len() as u32).to_le_bytes())?;
    f.folders.iter().try_for_each(|c| put_folder(w, c))
}

/// Saves an index, replacing the last one for the same folder only once
/// the new one is written whole.
pub fn save(dir: &Path, index: &Index) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = file(dir, &index.root);
    let part = path.with_extension("part");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut w = BufWriter::with_capacity(1 << 20, options.open(&part)?);
    w.write_all(MAGIC)?;
    put_str(&mut w, &index.root.to_string_lossy())?;
    w.write_all(&[index.on_disk as u8])?;
    w.write_all(&index.taken.to_le_bytes())?;
    w.write_all(&index.mark.event.to_le_bytes())?;
    put_str(&mut w, &index.mark.volume)?;
    put_folder(&mut w, &index.folder)?;
    w.into_inner().map_err(|e| e.into_error())?.sync_all()?;
    std::fs::rename(part, path)
}

/// Reads values back, giving up at anything that does not fit.
struct In<R: Read>(R);

impl<R: Read> In<R> {
    fn bytes<const N: usize>(&mut self) -> Option<[u8; N]> {
        let mut b = [0; N];
        self.0.read_exact(&mut b).ok()?;
        Some(b)
    }
    fn u32(&mut self) -> Option<u32> {
        self.bytes().map(u32::from_le_bytes)
    }
    fn u64(&mut self) -> Option<u64> {
        self.bytes().map(u64::from_le_bytes)
    }
    fn str(&mut self) -> Option<String> {
        // No name or path is longer than this; a length that is means the
        // file is not what it should be.
        let len = self.u32().filter(|n| *n <= 1 << 16)? as usize;
        let mut b = vec![0; len];
        self.0.read_exact(&mut b).ok()?;
        String::from_utf8(b).ok()
    }
    fn folder(&mut self, depth: usize) -> Option<Folder> {
        if depth > 4096 {
            return None;
        }
        let mut f = Folder { name: self.str()?.into(), mtime: self.u64()? as i64, denied: self.bytes::<1>()?[0] != 0, small_size: self.u64()?, small_count: self.u32()?, ..Default::default() };
        // Counts are not trusted with memory before the items are read.
        for _ in 0..self.u32()? {
            f.big.push(Big { name: self.str()?.into(), size: self.u64()? });
        }
        for _ in 0..self.u32()? {
            f.linked.push(Linked { id: self.u64()?, size: self.u64()?, name: self.str()?.into() });
        }
        for _ in 0..self.u32()? {
            f.folders.push(self.folder(depth + 1)?);
        }
        Some(f)
    }
}

/// The saved index for `root`, if there is one that can be read.
pub fn load(dir: &Path, root: &Path) -> Option<Index> {
    let mut r = In(BufReader::with_capacity(1 << 20, std::fs::File::open(file(dir, root)).ok()?));
    if &r.bytes::<8>()? != MAGIC {
        return None;
    }
    let saved = PathBuf::from(r.str()?);
    let index = Index { root: saved, on_disk: r.bytes::<1>()?[0] != 0, taken: r.u64()?, mark: Mark { event: r.u64()?, volume: r.str()? }, folder: r.folder(0)? };
    // Two paths could share a file name; only the right one will do.
    (index.root == root).then_some(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Index {
        let leaf = Folder { name: "2025".into(), mtime: -5, big: vec![Big { name: "raw.dng".into(), size: 40_000_000 }], small_size: 900, small_count: 3, ..Default::default() };
        let locked = Folder { name: "privé".into(), denied: true, ..Default::default() };
        Index { root: "/Users/sam".into(), on_disk: true, taken: 1_790_000_000, mark: Mark { event: 987_654_321, volume: "A1B2".into() }, folder: Folder { name: "sam".into(), mtime: 1_790_000_000_123_456_789, linked: vec![Linked { id: 77, size: 5_000_000, name: "twice".into() }], folders: vec![leaf, locked], ..Default::default() } }
    }

    #[test]
    fn an_index_comes_back_as_it_was_saved() {
        let dir = std::env::temp_dir().join(format!("neo-disk-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let index = sample();
        assert_eq!(load(&dir, &index.root), None, "nothing saved yet");
        save(&dir, &index).unwrap();
        assert_eq!(load(&dir, &index.root), Some(index.clone()));
        assert_eq!(load(&dir, Path::new("/Users/alex")), None, "another folder's index is not this one");
        // Saving again replaces it, and leaves nothing half-written behind.
        let mut later = index.clone();
        later.taken += 60;
        later.folder.folders.pop();
        save(&dir, &later).unwrap();
        assert_eq!(load(&dir, &index.root), Some(later));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(file(&dir, &index.root)).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0, "it names the user's files, so only they may read it");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_damaged_or_foreign_file_is_ignored() {
        let dir = std::env::temp_dir().join(format!("neo-disk-damaged-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let index = sample();
        save(&dir, &index).unwrap();
        let path = file(&dir, &index.root);
        let whole = std::fs::read(&path).unwrap();
        // Cut short anywhere, it is no index.
        for keep in [0, 7, 20, whole.len() / 2, whole.len() - 1] {
            std::fs::write(&path, &whole[..keep]).unwrap();
            assert_eq!(load(&dir, &index.root), None, "cut to {keep} bytes");
        }
        // Another version's layout, and a length that makes no sense.
        let mut other = whole.clone();
        other[7] = b'9';
        std::fs::write(&path, &other).unwrap();
        assert_eq!(load(&dir, &index.root), None);
        let mut huge = whole.clone();
        huge[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        std::fs::write(&path, &huge).unwrap();
        assert_eq!(load(&dir, &index.root), None);
        assert_ne!(file(&dir, Path::new("/a")), file(&dir, Path::new("/b")));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
