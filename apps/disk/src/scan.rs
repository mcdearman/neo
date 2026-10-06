//! Measuring a folder: reading everything under it and adding up sizes.
//!
//! A scan produces an [`Index`]: every folder under the root, with what
//! is in it. The index is what is kept, saved and brought up to date
//! later by re-reading only the folders that changed. What the window
//! shows, an [`Entry`] tree with the small things folded together, is
//! worked out from it.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use rayon::prelude::*;

use crate::changes::{self, Changes, Mark};

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

/// A file large enough to list by name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Big {
    pub name: Box<str>,
    pub size: u64,
}

/// A file with more than one name. Which of its names gets the size is
/// settled when the totals are worked out, so that it is counted once
/// however the index was put together.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Linked {
    /// The same for every name of one file.
    pub id: u64,
    pub size: u64,
    pub name: Box<str>,
}

/// One folder in the index: what is directly in it, and its subfolders.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Folder {
    pub name: Box<str>,
    /// When the folder itself last changed, in nanoseconds. Where there is
    /// no record of changes to consult, this says whether to read it again.
    pub mtime: i64,
    pub denied: bool,
    pub big: Vec<Big>,
    /// The files too small to list: their size together, and how many.
    pub small_size: u64,
    pub small_count: u32,
    pub linked: Vec<Linked>,
    pub folders: Vec<Folder>,
}

/// Everything found under a folder, and enough to bring it up to date.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Index {
    pub root: PathBuf,
    pub on_disk: bool,
    /// When it was last brought up to date, in seconds since 1970.
    pub taken: u64,
    /// Where the system's record of changes stood at that moment.
    pub mark: Mark,
    pub folder: Folder,
}

impl Index {
    /// What to show: sizes added up, small things folded together, the
    /// largest first.
    pub fn entry(&self) -> Entry {
        entry_of(&self.folder, &mut HashSet::new())
    }

    /// How many folders it holds.
    pub fn folders(&self) -> usize {
        fn count(f: &Folder) -> usize {
            1 + f.folders.iter().map(count).sum::<usize>()
        }
        count(&self.folder)
    }
}

fn entry_of(f: &Folder, seen: &mut HashSet<u64>) -> Entry {
    let mut entry = Entry { name: f.name.to_string(), dir: true, denied: f.denied, ..Default::default() };
    let mut children: Vec<Entry> = f.big.iter().map(|b| Entry { name: b.name.to_string(), size: b.size, files: 1, ..Default::default() }).collect();
    // A file's size goes to the first of its names met; the rest are empty.
    children.extend(f.linked.iter().map(|l| Entry { name: l.name.to_string(), size: if seen.insert(l.id) { l.size } else { 0 }, files: 1, ..Default::default() }));
    children.extend(f.folders.iter().map(|c| entry_of(c, seen)));
    let mut small = Entry { size: f.small_size, files: f.small_count as u64, group: true, ..Default::default() };
    let mut things = f.small_count as u64;
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
    entry.size += f.small_size;
    entry.files += f.small_count as u64;
    if things > 0 {
        small.name = if things == 1 { "1 smaller item".into() } else { format!("{things} smaller items") };
        entry.children.push(small);
    }
    entry.children.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)));
    entry
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

/// One thing in a folder, as the system lists it.
pub struct Item {
    pub name: String,
    /// A real folder, not a link to one.
    pub dir: bool,
    pub size: u64,
    /// How many names the file has.
    pub links: u64,
    pub id: u64,
}

/// A folder's own details and what is directly in it.
pub struct Listing {
    pub mtime: i64,
    /// The device the folder is on.
    pub dev: u64,
    pub items: Vec<Item>,
}

/// Reads one folder. On macOS the system hands back names and sizes for a
/// whole folder at a time; elsewhere each file is asked about in turn.
#[cfg(target_os = "macos")]
fn list(path: &Path, on_disk: bool) -> std::io::Result<Listing> {
    crate::bulk::list(path, on_disk)
}

#[cfg(not(target_os = "macos"))]
fn list(path: &Path, on_disk: bool) -> std::io::Result<Listing> {
    let own = std::fs::symlink_metadata(path)?;
    let mut items = vec![];
    for item in std::fs::read_dir(path)?.flatten() {
        // Not followed: a link is counted as itself, not as its target.
        let Ok(meta) = item.metadata() else { continue };
        #[cfg(unix)]
        let (size, links, id) = {
            use std::os::unix::fs::MetadataExt;
            (if on_disk { meta.blocks() * 512 } else { meta.len() }, meta.nlink(), meta.ino())
        };
        #[cfg(not(unix))]
        let (size, links, id) = (meta.len(), 1, 0);
        items.push(Item { name: item.file_name().to_string_lossy().into_owned(), dir: meta.is_dir(), size, links, id });
    }
    let _ = on_disk;
    Ok(Listing { mtime: mtime(&own), dev: device(&own), items })
}

#[cfg(unix)]
fn device(meta: &std::fs::Metadata) -> u64 {
    std::os::unix::fs::MetadataExt::dev(meta)
}

#[cfg(not(unix))]
fn device(_meta: &std::fs::Metadata) -> u64 {
    0
}

#[cfg(not(target_os = "macos"))]
fn mtime(meta: &std::fs::Metadata) -> i64 {
    meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos() as i64)
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The threads that read folders. Reading a directory tree is mostly
/// waiting on the system, and any thread with nothing to do takes
/// whichever folder is waiting, at whatever depth.
fn pool() -> &'static rayon::ThreadPool {
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| {
        let threads = std::env::var("NEO_DISK_THREADS").ok().and_then(|n| n.parse().ok()).unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(2, 16));
        rayon::ThreadPoolBuilder::new().num_threads(threads).thread_name(|i| format!("neo-disk-{i}")).build().expect("threads to read folders with")
    })
}

struct Walk<'a> {
    options: Options,
    /// The devices to stay on, so a disk's scan does not wander onto
    /// others mounted inside it. Empty where devices cannot be told apart.
    devices: Vec<u64>,
    /// Folders to leave out.
    skip: Vec<PathBuf>,
    progress: &'a Progress,
}

impl<'a> Walk<'a> {
    fn new(root: &Path, options: Options, progress: &'a Progress) -> Self {
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
        Self { options, devices, skip, progress }
    }

    fn cancelled(&self) -> bool {
        self.progress.cancel.load(Ordering::Relaxed)
    }

    fn elsewhere(&self, dev: u64) -> bool {
        !self.devices.is_empty() && !self.devices.contains(&dev)
    }

    /// Sorts a listing into a folder's own record, and the names of the
    /// folders inside it, which are still to be read.
    fn own(&self, name: Box<str>, listing: Listing) -> (Folder, Vec<String>) {
        let mut f = Folder { name, mtime: listing.mtime, ..Default::default() };
        let (mut dirs, mut files, mut bytes) = (vec![], 0, 0);
        for item in listing.items {
            if item.dir {
                dirs.push(item.name);
                continue;
            }
            files += 1;
            bytes += item.size;
            if item.links > 1 {
                f.linked.push(Linked { id: item.id, size: item.size, name: item.name.into() });
            } else if item.size >= SMALL {
                f.big.push(Big { name: item.name.into(), size: item.size });
            } else {
                f.small_size += item.size;
                f.small_count += 1;
            }
        }
        self.progress.files.fetch_add(files, Ordering::Relaxed);
        self.progress.bytes.fetch_add(bytes, Ordering::Relaxed);
        (f, dirs)
    }

    /// Reads the folders with these names inside `path`, each with
    /// everything under it, sharing them out between the threads.
    fn inside(&self, path: &Path, names: Vec<String>) -> Vec<Folder> {
        names
            .into_par_iter()
            .filter_map(|name| {
                let path = path.join(&name);
                if self.skip.contains(&path) { None } else { self.folder(&path, name.into()) }
            })
            .collect()
    }

    /// Reads a folder and everything under it. `None` if it turns out to
    /// be on another disk.
    fn folder(&self, path: &Path, name: Box<str>) -> Option<Folder> {
        if self.cancelled() {
            return Some(Folder { name, ..Default::default() });
        }
        let listing = match list(path, self.options.on_disk) {
            Ok(listing) => listing,
            Err(_) => return Some(Folder { name, denied: true, ..Default::default() }),
        };
        if self.elsewhere(listing.dev) {
            return None;
        }
        let (mut f, dirs) = self.own(name, listing);
        f.folders = self.inside(path, dirs);
        Some(f)
    }

    /// Reads one folder again, keeping what is known of the folders inside
    /// it that are still there, and reading in full those that are new.
    fn reread(&self, f: &mut Folder, path: &Path) {
        if self.cancelled() {
            return;
        }
        let name = std::mem::take(&mut f.name);
        let listing = match list(path, self.options.on_disk) {
            Ok(listing) if !self.elsewhere(listing.dev) => listing,
            // Gone, or closed to us now: nothing in it can be counted.
            _ => {
                *f = Folder { name, denied: true, ..Default::default() };
                return;
            }
        };
        let mut before: HashMap<Box<str>, Folder> = std::mem::take(&mut f.folders).into_iter().map(|c| (c.name.clone(), c)).collect();
        let (mut now, dirs) = self.own(name, listing);
        let mut new = vec![];
        for dir in dirs {
            match before.remove(dir.as_str()) {
                Some(kept) => now.folders.push(kept),
                None => new.push(dir),
            }
        }
        now.folders.extend(self.inside(path, new));
        *f = now;
    }

    /// Brings the index up to date for a change reported at `path`: reads
    /// that folder again, or the nearest one above it that is known.
    fn update(&self, top: &mut Folder, root: &Path, path: &Path, everything_under: bool) {
        let Ok(rest) = path.strip_prefix(root) else { return };
        if self.skip.iter().any(|s| path.starts_with(s)) {
            return;
        }
        let (mut at, mut here, mut found) = (top, root.to_path_buf(), true);
        for part in rest.components() {
            let part = part.as_os_str().to_string_lossy();
            match at.folders.iter().position(|c| *c.name == *part) {
                Some(i) => {
                    at = &mut at.folders[i];
                    here.push(&*part);
                }
                None => {
                    found = false;
                    break;
                }
            }
        }
        if found && everything_under {
            let name = std::mem::take(&mut at.name);
            *at = self.folder(&here, name.clone()).unwrap_or(Folder { name, denied: true, ..Default::default() });
        } else {
            self.reread(at, &here);
        }
    }

    /// Where there is no record of changes: reads again every folder whose
    /// own time has moved, which it does when something is added to it,
    /// removed or renamed.
    #[cfg(not(target_os = "macos"))]
    fn check(&self, f: &mut Folder, path: &Path, reread: &AtomicUsize) {
        if self.cancelled() {
            return;
        }
        if f.denied || std::fs::symlink_metadata(path).map(|m| mtime(&m)).ok() != Some(f.mtime) {
            self.reread(f, path);
            reread.fetch_add(1, Ordering::Relaxed);
        }
        f.folders.par_iter_mut().for_each(|c| self.check(c, &path.join(&*c.name), reread));
    }
}

/// Measures everything under `root`. Stops early, with what it has, if
/// `progress.cancel` is set.
pub fn scan(root: &Path, options: Options, progress: &Progress) -> Index {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let name: Box<str> = root.file_name().map(|n| n.to_string_lossy().into()).unwrap_or_else(|| root.to_string_lossy().into());
    // Before reading, so that what changes meanwhile is caught next time.
    let mark = changes::mark(&root);
    let walk = Walk::new(&root, options, progress);
    let folder = pool().install(|| walk.folder(&root, name.clone())).unwrap_or(Folder { name, denied: true, ..Default::default() });
    Index { root, on_disk: options.on_disk, taken: now(), mark, folder }
}

/// What bringing an index up to date came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Report {
    /// Folders read again.
    pub reread: usize,
    /// Nothing could be told about what changed, so all of it was read.
    pub full: bool,
}

/// Brings an index up to date by reading again only what changed since
/// it was made.
///
/// On macOS the system keeps a record of every folder in which something
/// changed, and those are the ones read. Elsewhere each folder's own time
/// is checked, which catches files added, removed and renamed but not one
/// that only grew where it was; measuring from scratch catches those.
pub fn refresh(index: &mut Index, progress: &Progress) -> Report {
    let options = Options { on_disk: index.on_disk };
    let mark = changes::mark(&index.root);
    let root = index.root.clone();
    let walk = Walk::new(&root, options, progress);
    let reread = AtomicUsize::new(0);
    match changes::since(&root, &index.mark) {
        Changes::Unknown => {
            *index = scan(&root, options, progress);
            return Report { reread: 0, full: true };
        }
        Changes::Folders(list) => {
            let mut done = HashSet::new();
            pool().install(|| {
                for (path, under) in list {
                    if done.insert((path.clone(), under)) {
                        walk.update(&mut index.folder, &root, &path, under);
                        reread.fetch_add(1, Ordering::Relaxed);
                    }
                }
            });
        }
        #[cfg(not(target_os = "macos"))]
        Changes::CheckTimes => pool().install(|| walk.check(&mut index.folder, &root, &reread)),
        #[cfg(target_os = "macos")]
        Changes::CheckTimes => unreachable!("macOS has a record of changes"),
    }
    index.mark = mark;
    index.taken = now();
    Report { reread: reread.into_inner(), full: false }
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
        scan(root, Options { on_disk: false }, &Progress::default()).entry()
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
        let e = scan(&root, Options::default(), &Progress::default()).entry();
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

    /// Brings `index` up to date, giving the system a little while to
    /// write down what just happened, and returns the last report.
    fn refreshed(index: &mut Index, root: &Path) -> Report {
        let want = measure(root);
        let mut report = refresh(index, &Progress::default());
        for _ in 0..100 {
            if index.entry() == want {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
            report = refresh(index, &Progress::default());
        }
        assert_eq!(index.entry(), want, "brought up to date, it is what measuring from nothing finds");
        report
    }

    #[test]
    fn an_index_is_brought_up_to_date_by_reading_only_what_changed() {
        let root = scratch("refresh");
        for i in 0..20 {
            file(&root.join(format!("steady-{i}/data.bin")), MB);
        }
        file(&root.join("busy/old.bin"), 2 * MB);
        file(&root.join("busy/deep/keep.bin"), MB);
        file(&root.join("doomed/gone.bin"), 3 * MB);
        let mut index = scan(&root, Options { on_disk: false }, &Progress::default());
        assert_eq!(index.entry().size, 26 * MB);
        assert_eq!(index.folders(), 24);
        // Nothing has happened since: at most the folders just made are
        // looked at again, and nothing is read in full.
        let quiet = refresh(&mut index, &Progress::default());
        assert!(!quiet.full && quiet.reread <= index.folders(), "{quiet:?}");
        assert_eq!(index.entry().size, 26 * MB);

        // A file added, one removed, a folder made with folders in it, and
        // a folder deleted.
        std::thread::sleep(std::time::Duration::from_millis(20));
        file(&root.join("busy/new.bin"), 5 * MB);
        std::fs::remove_file(root.join("busy/old.bin")).unwrap();
        file(&root.join("fresh/a/b/c.bin"), 4 * MB);
        std::fs::remove_dir_all(root.join("doomed")).unwrap();
        let report = refreshed(&mut index, &root);
        assert!(!report.full, "the folders that changed were found without reading them all");
        assert_eq!(index.entry().size, 30 * MB);
        assert_eq!(index.folders(), 26);
        let busy = index.folder.folders.iter().find(|f| &*f.name == "busy").unwrap();
        assert_eq!(busy.big.iter().map(|b| &*b.name).collect::<Vec<_>>(), ["new.bin"]);
        assert_eq!(busy.folders.len(), 1, "the folder inside it that did not change is kept");

        // A folder that goes and comes back with other things in it.
        std::fs::remove_dir_all(root.join("fresh")).unwrap();
        file(&root.join("fresh/other.bin"), MB);
        refreshed(&mut index, &root);
        assert_eq!(index.entry().size, 27 * MB);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_file_with_two_names_counts_once_however_the_index_was_built() {
        let root = scratch("relinked");
        file(&root.join("a/data.bin"), 2 * MB);
        file(&root.join("b/other.bin"), MB);
        std::fs::hard_link(root.join("a/data.bin"), root.join("b/again.bin")).unwrap();
        let mut index = scan(&root, Options { on_disk: false }, &Progress::default());
        assert_eq!(index.entry().size, 3 * MB);
        // Reading again the folder with the second name must not add it in.
        std::thread::sleep(std::time::Duration::from_millis(20));
        file(&root.join("b/more.bin"), MB);
        refreshed(&mut index, &root);
        assert_eq!(index.entry().size, 4 * MB);
        // Nor reading again the one with the first.
        file(&root.join("a/more.bin"), MB);
        refreshed(&mut index, &root);
        assert_eq!(index.entry().size, 5 * MB);
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
        let e = scan(&root, Options { on_disk: false }, &stopped).entry();
        assert_eq!(e.files, 0, "nothing inside the folders was read");
        // A folder that is not there is reported as unreadable.
        assert!(measure(&root.join("missing")).denied);
        std::fs::remove_dir_all(root).unwrap();
    }
}
