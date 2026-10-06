//! Measuring a folder: walking everything under it and adding up sizes.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Files smaller than this are not listed one by one. A folder's small
/// files, and its subfolders that come to less, are added together as one
/// entry, which is what keeps a whole disk's tree in reasonable memory.
pub const SMALL: u64 = 256 * 1024;

/// A file, a folder, or a folder's small things taken together.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    /// Bytes on disk, for a folder including everything inside it.
    pub size: u64,
    pub dir: bool,
    /// How many files this stands for: one for a file, all of them inside
    /// for a folder.
    pub files: u64,
    /// A folder's contents, largest first.
    pub children: Vec<Entry>,
    /// Not one thing but a folder's small files and folders together.
    pub group: bool,
    /// A folder that could not be opened, so its size is not known.
    pub denied: bool,
}

impl Entry {
    /// The entry reached by taking, at each level, the child at that index.
    pub fn at(&self, path: &[usize]) -> Option<&Entry> {
        path.iter().try_fold(self, |e, i| e.children.get(*i))
    }
}

/// How a scan is going, shared with whoever is watching it.
#[derive(Default)]
pub struct Progress {
    pub files: AtomicU64,
    pub bytes: AtomicU64,
    /// Set to stop the scan early.
    pub cancel: AtomicBool,
}

/// What to count and where to stay.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Count the space files take up on disk rather than their length,
    /// which is what a full disk is short of. Lengths are exact, so tests
    /// use those.
    pub on_disk: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self { on_disk: true }
    }
}

struct Walk<'a> {
    options: Options,
    /// The devices to stay on, so a disk's scan does not wander onto
    /// others mounted inside it. Empty where devices cannot be told apart.
    devices: Vec<u64>,
    /// Folders to leave out.
    skip: Vec<PathBuf>,
    /// Files with several names, each counted once.
    linked: Mutex<HashSet<(u64, u64)>>,
    progress: &'a Progress,
}

#[cfg(unix)]
fn device(meta: &std::fs::Metadata) -> u64 {
    std::os::unix::fs::MetadataExt::dev(meta)
}

#[cfg(not(unix))]
fn device(_meta: &std::fs::Metadata) -> u64 {
    0
}

impl Walk<'_> {
    fn size(&self, meta: &std::fs::Metadata) -> u64 {
        #[cfg(unix)]
        if self.options.on_disk {
            return std::os::unix::fs::MetadataExt::blocks(meta) * 512;
        }
        meta.len()
    }

    /// Whether this file has already been counted under another name.
    fn seen_before(&self, meta: &std::fs::Metadata) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if meta.nlink() > 1 {
                return !self.linked.lock().unwrap_or_else(|e| e.into_inner()).insert((meta.dev(), meta.ino()));
            }
        }
        let _ = meta;
        false
    }

    fn file(&self, name: String, meta: &std::fs::Metadata) -> Entry {
        let size = if self.seen_before(meta) { 0 } else { self.size(meta) };
        self.progress.files.fetch_add(1, Ordering::Relaxed);
        self.progress.bytes.fetch_add(size, Ordering::Relaxed);
        Entry { name, size, files: 1, ..Default::default() }
    }

    fn folder(&self, path: &Path, name: String) -> Entry {
        let mut entry = Entry { name, dir: true, ..Default::default() };
        let Ok(read) = std::fs::read_dir(path) else {
            entry.denied = true;
            return entry;
        };
        let mut children = vec![];
        for item in read.flatten() {
            if self.progress.cancel.load(Ordering::Relaxed) {
                break;
            }
            // Not followed: a link is counted as itself, not as its target.
            let Ok(meta) = item.metadata() else { continue };
            let name = item.file_name().to_string_lossy().into_owned();
            if meta.is_dir() {
                let path = item.path();
                if (!self.devices.is_empty() && !self.devices.contains(&device(&meta))) || self.skip.contains(&path) {
                    continue;
                }
                children.push(self.folder(&path, name));
            } else {
                children.push(self.file(name, &meta));
            }
        }
        finish(&mut entry, children);
        entry
    }
}

/// Totals a folder from its contents, folds the small ones into one entry,
/// and puts the largest first.
fn finish(entry: &mut Entry, children: Vec<Entry>) {
    let mut small = Entry { group: true, ..Default::default() };
    let mut things = 0u64;
    for c in children {
        entry.size += c.size;
        entry.files += c.files;
        if c.size < SMALL && !c.denied {
            small.size += c.size;
            small.files += c.files;
            things += 1;
        } else {
            entry.children.push(c);
        }
    }
    if things > 0 {
        small.name = if things == 1 { "1 smaller item".into() } else { format!("{things} smaller items") };
        entry.children.push(small);
    }
    entry.children.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)));
}

/// Measures everything under `root`. The folders at the top are shared out
/// between a few threads, since reading a disk's directory tree is mostly
/// waiting. Stops early, with what it has, if `progress.cancel` is set.
pub fn scan(root: &Path, options: Options, progress: &Progress) -> Entry {
    let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| root.to_string_lossy().into_owned());
    let mut entry = Entry { name: name.clone(), dir: true, ..Default::default() };
    let mut devices: Vec<u64> = std::fs::metadata(root).map(|m| device(&m)).into_iter().filter(|_| cfg!(unix)).collect();
    let mut skip = vec![];
    // On macOS the startup disk is two volumes made to look like one: a
    // sealed system volume at `/`, and the data volume, which holds nearly
    // everything, mounted again under /System/Volumes. Count the data
    // volume where it appears in the tree, and not a second time there.
    if cfg!(target_os = "macos") && root == Path::new("/") {
        devices.extend(std::fs::metadata("/System/Volumes/Data").map(|m| device(&m)));
        skip.push(PathBuf::from("/System/Volumes"));
    }
    let walk = Walk { options, devices, skip, linked: Mutex::new(HashSet::new()), progress };
    let Ok(read) = std::fs::read_dir(root) else {
        entry.denied = true;
        return entry;
    };
    let mut children = vec![];
    let mut folders = vec![];
    for item in read.flatten() {
        let Ok(meta) = item.metadata() else { continue };
        let name = item.file_name().to_string_lossy().into_owned();
        if meta.is_dir() {
            let path = item.path();
            if (walk.devices.is_empty() || walk.devices.contains(&device(&meta))) && !walk.skip.contains(&path) {
                folders.push((path, name));
            }
        } else {
            children.push(walk.file(name, &meta));
        }
    }
    let queue = Mutex::new(folders);
    let done = Mutex::new(vec![]);
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(2, 8);
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                loop {
                    let Some((path, name)) = queue.lock().unwrap_or_else(|e| e.into_inner()).pop() else {
                        break;
                    };
                    let folder = walk.folder(&path, name);
                    done.lock().unwrap_or_else(|e| e.into_inner()).push(folder);
                }
            });
        }
    });
    children.extend(done.into_inner().unwrap_or_else(|e| e.into_inner()));
    finish(&mut entry, children);
    entry
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("neo-disk-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn file(path: &Path, bytes: u64) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![7u8; bytes as usize]).unwrap();
    }

    /// Lengths, which are exact, rather than space on disk, which rounds.
    fn measure(root: &Path) -> Entry {
        scan(root, Options { on_disk: false }, &Progress::default())
    }

    const MB: u64 = 1024 * 1024;

    #[test]
    fn folders_add_up_and_the_largest_come_first() {
        let root = scratch("sizes");
        file(&root.join("movies/long.mov"), 3 * MB);
        file(&root.join("movies/short.mov"), MB);
        file(&root.join("photos/2024/a.jpg"), 2 * MB);
        file(&root.join("notes.txt"), MB / 2);
        let e = measure(&root);
        assert_eq!((e.size, e.files, e.dir), (6 * MB + MB / 2, 4, true));
        let names: Vec<(&str, u64)> = e.children.iter().map(|c| (c.name.as_str(), c.size)).collect();
        assert_eq!(names, [("movies", 4 * MB), ("photos", 2 * MB), ("notes.txt", MB / 2)]);
        let movies = &e.children[0];
        assert_eq!(movies.children.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["long.mov", "short.mov"]);
        assert_eq!(e.at(&[1, 0, 0]).map(|c| c.name.as_str()), Some("a.jpg"), "photos, then 2024, then the picture");
        assert!(e.at(&[9]).is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn small_things_are_counted_together_not_listed() {
        let root = scratch("small");
        file(&root.join("big.bin"), 2 * MB);
        for i in 0..40 {
            file(&root.join(format!("tiny-{i}.txt")), 1000);
        }
        // A folder that comes to very little is one of the small things too.
        file(&root.join("crumbs/a"), 10);
        file(&root.join("crumbs/b"), 20);
        let e = measure(&root);
        assert_eq!(e.size, 2 * MB + 40_000 + 30, "nothing is left out of the total");
        assert_eq!(e.files, 43);
        assert_eq!(e.children.len(), 2);
        let group = &e.children[1];
        assert!(group.group && !group.dir);
        assert_eq!((group.name.as_str(), group.size, group.files), ("41 smaller items", 40_030, 42));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn links_are_not_followed_and_a_file_with_two_names_counts_once() {
        let root = scratch("links");
        file(&root.join("real/data.bin"), 2 * MB);
        std::os::unix::fs::symlink(root.join("real"), root.join("shortcut")).unwrap();
        std::fs::hard_link(root.join("real/data.bin"), root.join("same-file.bin")).unwrap();
        let e = measure(&root);
        assert_eq!(e.size - e.size % MB, 2 * MB, "two megabytes once, plus the link itself: {}", e.size);
        assert_eq!(e.files, 3, "the file, its second name, and the link: the link's target is not walked into");
        assert!(e.children.iter().all(|c| c.name != "shortcut" || !c.dir));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn space_on_disk_is_counted_in_blocks() {
        let root = scratch("blocks");
        file(&root.join("one-byte"), 1);
        file(&root.join("big.bin"), MB);
        let e = scan(&root, Options::default(), &Progress::default());
        assert!(e.size >= MB + 512, "a one-byte file still takes a block: {}", e.size);
        assert_eq!(e.size % 512, 0);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_that_cannot_be_read_is_marked_not_dropped() {
        use std::os::unix::fs::PermissionsExt;
        let root = scratch("denied");
        file(&root.join("open/a.bin"), MB);
        file(&root.join("locked/secret.bin"), MB);
        std::fs::set_permissions(root.join("locked"), std::fs::Permissions::from_mode(0o000)).unwrap();
        let e = measure(&root);
        std::fs::set_permissions(root.join("locked"), std::fs::Permissions::from_mode(0o755)).unwrap();
        // Root reads anything, so only check when the lock held.
        if let Some(locked) = e.children.iter().find(|c| c.name == "locked").filter(|c| c.denied) {
            assert_eq!((locked.size, e.size), (0, MB));
            assert!(e.children.iter().any(|c| c.name == "locked"), "still listed, small as it looks");
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn progress_is_reported_and_a_cancelled_scan_stops() {
        let root = scratch("progress");
        for i in 0..30 {
            file(&root.join(format!("dir-{i}/file.bin")), 1000);
        }
        let progress = Progress::default();
        scan(&root, Options { on_disk: false }, &progress);
        assert_eq!((progress.files.load(Ordering::Relaxed), progress.bytes.load(Ordering::Relaxed)), (30, 30_000));
        let stopped = Progress::default();
        stopped.cancel.store(true, Ordering::Relaxed);
        let e = scan(&root, Options { on_disk: false }, &stopped);
        assert_eq!(e.files, 0, "nothing inside the folders was read");
        // A folder that is not there is reported as unreadable.
        assert!(measure(&root.join("missing")).denied);
        std::fs::remove_dir_all(root).unwrap();
    }
}
