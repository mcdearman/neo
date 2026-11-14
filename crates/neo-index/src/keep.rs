//! Keeping the indexes of this computer's disks, and finding the one
//! that covers a folder.
//!
//! NeoShell does the keeping, since it runs from when the computer
//! starts: [`run`] goes round the disks now and then, bringing each one's
//! index up to date and saving it. Nothing is held in memory between
//! rounds: an index of a whole disk is tens of megabytes, and is on disk
//! for whoever wants it. An app asks [`known`] for the index that covers
//! a folder, or [`up_to_date`] to have one made if there is none.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::scan::{self, Index, Options, Progress};
use crate::store;

/// How long after starting the first round begins: once the computer has
/// finished starting, which is when reading whole disks is least in the way.
const FIRST_AFTER: Duration = Duration::from_secs(45);
/// How long from the end of one round to the start of the next.
const EVERY: Duration = Duration::from_secs(15 * 60);

/// The disks built into this computer, by their roots, on Windows: `C:\`
/// and the rest. One that is plugged in, a disc or a share on the network
/// is not counted: it may not be there tomorrow.
#[cfg(windows)]
pub fn fixed_drives() -> Vec<PathBuf> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetDriveTypeW(root: *const u16) -> u32;
    }
    const FIXED: u32 = 3;
    ('A'..='Z')
        .map(|letter| format!("{letter}:\\"))
        .filter(|root| {
            let wide: Vec<u16> = root.encode_utf16().chain(Some(0)).collect();
            // SAFETY: asked of Windows with a root's name ended by a nul, which it only reads.
            unsafe { GetDriveTypeW(wide.as_ptr()) == FIXED }
        })
        .map(PathBuf::from)
        .collect()
}

/// What is kept an index of without being asked: on Windows every disk
/// built in, where a person's things are spread over them; elsewhere the
/// home folder, where they are kept.
pub fn roots() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        fixed_drives()
    }
    #[cfg(not(windows))]
    {
        vec![neo_desktop::fs::home_dir()]
    }
}

/// A path as indexes name it: as the system resolves it.
pub fn resolved(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// The kept index that covers `path`: its own, or a disk's that it is on.
/// As it was last saved, which may be a quarter of an hour old.
pub fn known(dir: &Path, path: &Path) -> Option<Index> {
    store::covering(dir, &resolved(path))
}

/// How an index came to be as it is now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum How {
    /// Read from nothing.
    Made,
    /// A kept one, with this many folders read again.
    Refreshed(usize),
}

/// The index of `root`, up to date and saved: a kept one brought up to
/// date by reading what changed, or a new one where none is kept.
/// Measured as space on disk, which is what NeoDisk shows; whoever only
/// wants names is none the worse for it.
pub fn up_to_date(dir: &Path, root: &Path, progress: &Progress) -> (Index, How) {
    let root = resolved(root);
    let (index, how) = match store::load(dir, &root) {
        Some(mut index) => {
            let report = scan::refresh(&mut index, progress);
            (index, if report.full { How::Made } else { How::Refreshed(report.reread) })
        }
        None => (scan::scan(&root, Options::default(), progress), How::Made),
    };
    // One cut short by being told to stop is not kept as the whole of it.
    if !progress.cancel.load(std::sync::atomic::Ordering::Relaxed) {
        let _ = store::save(dir, &index);
    }
    (index, how)
}

/// Goes round the disks for as long as the program runs, keeping their
/// indexes up to date. For a thread of its own.
pub fn run() {
    let dir = store::folder();
    std::thread::sleep(FIRST_AFTER);
    loop {
        for root in roots() {
            // Made, saved, and let go of before the next is begun.
            drop(up_to_date(&dir, &root, &Progress::default()));
        }
        std::thread::sleep(EVERY);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_index_is_made_once_then_kept_and_brought_up_to_date() {
        let dir = std::env::temp_dir().join(format!("neo-index-keep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (root, cache) = (dir.join("disk"), dir.join("cache"));
        std::fs::create_dir_all(root.join("notes/old")).unwrap();
        std::fs::write(root.join("notes/a.md"), "one").unwrap();
        std::fs::write(root.join("notes/old/b.md"), "two!").unwrap();
        assert!(known(&cache, &root).is_none(), "nothing kept yet");

        let (index, how) = up_to_date(&cache, &root, &Progress::default());
        assert_eq!(how, How::Made);
        let notes = index.folder_at(&resolved(&root.join("notes"))).unwrap();
        let mut files: Vec<(&str, u64)> = notes.files().collect();
        files.sort_unstable();
        assert_eq!(files.iter().map(|f| f.0).collect::<Vec<_>>(), ["a.md"], "every file is named, however small");

        // Asked of any folder inside it, the same index answers.
        let found = known(&cache, &root.join("notes/old")).expect("the index of the folder above");
        assert_eq!(found.root, resolved(&root));
        assert_eq!(found.folder_at(&resolved(&root.join("notes/old"))).unwrap().files().map(|f| f.0).collect::<Vec<_>>(), ["b.md"]);

        // What is added is found by reading only the folder it was added to.
        std::thread::sleep(Duration::from_millis(30));
        std::fs::write(root.join("notes/old/c.md"), "three").unwrap();
        let (index, how) = up_to_date(&cache, &root, &Progress::default());
        assert!(matches!(how, How::Refreshed(n) if n >= 1), "{how:?}");
        let mut names: Vec<&str> = index.folder_at(&resolved(&root.join("notes/old"))).unwrap().files().map(|f| f.0).collect();
        names.sort_unstable();
        assert_eq!(names, ["b.md", "c.md"]);
        assert!(!roots().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// By hand: indexes this computer's own disks into a folder of its
    /// own, and says how long it took and what it weighs.
    /// `cargo test --release -p neo-index this_computer -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn this_computers_disks_are_indexed() {
        let cache = std::env::temp_dir().join(format!("neo-index-real-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&cache);
        for root in roots() {
            let began = std::time::Instant::now();
            let (index, how) = up_to_date(&cache, &root, &Progress::default());
            let made = began.elapsed();
            fn count(f: &crate::scan::Folder) -> (u64, u64) {
                f.folders.iter().map(count).fold((1, f.files().count() as u64), |a, b| (a.0 + b.0, a.1 + b.1))
            }
            let (folders, files) = count(&index.folder);
            let kept = std::fs::metadata(store::file(&cache, &index.root)).map_or(0, |m| m.len());
            let began = std::time::Instant::now();
            let (_, again) = up_to_date(&cache, &root, &Progress::default());
            println!("{}: {how:?} in {made:.1?}: {folders} folders, {files} files, {:.1} MB kept; again {again:?} in {:.1?}", root.display(), kept as f64 / 1e6, began.elapsed());
            assert!(files > 0 && known(&cache, &root).is_some());
        }
        let _ = std::fs::remove_dir_all(&cache);
    }
}