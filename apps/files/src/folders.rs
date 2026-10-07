//! What Files keeps beside a folder listing: how big each folder is, which
//! takes reading everything inside it, and the folders bookmarked in the
//! sidebar.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// How much a folder holds: the length of every file under it, each file
/// with several names counted once. Links are not followed, and other
/// disks mounted inside it are left out. `keep_going` is asked now and
/// then, and stops the count with `None` when it says no.
pub fn measure(dir: &Path, keep_going: &dyn Fn() -> bool) -> Option<u64> {
    let start = std::fs::symlink_metadata(dir).ok()?;
    let home = device(&start);
    let mut seen = HashSet::new();
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    let mut counted = 0u32;
    while let Some(here) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&here) else { continue };
        for item in read.flatten() {
            counted += 1;
            if counted % 512 == 0 && !keep_going() {
                return None;
            }
            // Not followed: a link counts as itself.
            let Ok(meta) = item.metadata() else { continue };
            if meta.is_dir() {
                if device(&meta) == home {
                    stack.push(item.path());
                }
            } else if !linked_again(&meta, &mut seen) {
                total += meta.len();
            }
        }
    }
    Some(total)
}

#[cfg(unix)]
fn device(meta: &std::fs::Metadata) -> u64 {
    std::os::unix::fs::MetadataExt::dev(meta)
}

#[cfg(not(unix))]
fn device(_meta: &std::fs::Metadata) -> u64 {
    0
}

/// Whether this file has already been counted under another name.
fn linked_again(meta: &std::fs::Metadata, seen: &mut HashSet<(u64, u64)>) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.nlink() > 1 {
            return !seen.insert((meta.dev(), meta.ino()));
        }
    }
    let _ = (meta, seen);
    false
}

/// Where the bookmarks are kept: one folder a line, in the order shown.
pub fn bookmarks_file() -> PathBuf {
    neo_desktop::config_dir().join("apps").join("files-bookmarks")
}

pub fn load_bookmarks(file: &Path) -> Vec<PathBuf> {
    std::fs::read_to_string(file).map(|text| text.lines().map(str::trim).filter(|l| !l.is_empty()).map(PathBuf::from).collect()).unwrap_or_default()
}

pub fn save_bookmarks(file: &Path, bookmarks: &[PathBuf]) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text: String = bookmarks.iter().map(|b| format!("{}\n", b.display())).collect();
    std::fs::write(file, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("neo-files-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_folder_is_the_sum_of_everything_under_it() {
        let dir = scratch("measure");
        std::fs::create_dir_all(dir.join("a/b/c")).unwrap();
        std::fs::write(dir.join("one"), vec![0u8; 1000]).unwrap();
        std::fs::write(dir.join("a/two"), vec![0u8; 2000]).unwrap();
        std::fs::write(dir.join("a/b/c/three"), vec![0u8; 3000]).unwrap();
        assert_eq!(measure(&dir, &|| true), Some(6000));
        assert_eq!(measure(&dir.join("a"), &|| true), Some(5000));
        // An empty folder holds nothing; a missing one cannot be measured.
        std::fs::create_dir_all(dir.join("empty")).unwrap();
        assert_eq!(measure(&dir.join("empty"), &|| true), Some(0));
        assert_eq!(measure(&dir.join("missing"), &|| true), None);
        #[cfg(unix)]
        {
            // A link is not followed into, and a file with two names counts once.
            std::os::unix::fs::symlink(dir.join("a"), dir.join("shortcut")).unwrap();
            std::fs::hard_link(dir.join("one"), dir.join("again")).unwrap();
            let total = measure(&dir, &|| true).unwrap();
            assert!((6000..6200).contains(&total), "{total}");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_count_can_be_given_up() {
        let dir = scratch("give-up");
        for i in 0..1200 {
            std::fs::write(dir.join(format!("f{i}")), b"x").unwrap();
        }
        assert_eq!(measure(&dir, &|| false), None, "asked to stop, it stops");
        assert_eq!(measure(&dir, &|| true), Some(1200));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn bookmarks_are_kept_in_order() {
        let dir = scratch("bookmarks");
        let file = dir.join("apps/files-bookmarks");
        assert!(load_bookmarks(&file).is_empty());
        let marks = vec![PathBuf::from("/work/neo"), PathBuf::from("/work/a folder with spaces")];
        save_bookmarks(&file, &marks).unwrap();
        assert_eq!(load_bookmarks(&file), marks);
        std::fs::write(&file, "\n/work/neo\n\n  /tmp  \n").unwrap();
        assert_eq!(load_bookmarks(&file), [PathBuf::from("/work/neo"), PathBuf::from("/tmp")], "blank lines and spaces around are passed over");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
