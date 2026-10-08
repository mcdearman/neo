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
            if counted.is_multiple_of(512) && !keep_going() {
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

/// Where pictures made of videos are kept, so each is made once.
pub fn frames_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        neo_desktop::fs::home_dir().join("Library/Caches/org.neo.Files/frames")
    } else if let Some(dir) = std::env::var_os("XDG_CACHE_HOME").filter(|d| !d.is_empty()) {
        PathBuf::from(dir).join("neo-files/frames")
    } else if let Some(dir) = std::env::var_os("LOCALAPPDATA").filter(|_| cfg!(windows)) {
        PathBuf::from(dir).join("NeoFiles/frames")
    } else {
        neo_desktop::fs::home_dir().join(".cache/neo-files/frames")
    }
}

/// The name a video's picture is kept under: one for this file as it is
/// now, so that a file changed or replaced gets a new one.
pub fn frame_name(video: &Path, meta: &std::fs::Metadata) -> String {
    let stamp = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in video.as_os_str().as_encoded_bytes().iter().copied().chain(stamp.to_le_bytes()).chain(meta.len().to_le_bytes()) {
        hash = (hash ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}.png")
}

/// The commands that would write a small picture of `video` to `out`, in
/// the order to try them: the system's own on macOS, and elsewhere the
/// usual thumbnailer and then ffmpeg itself.
pub fn frame_commands(video: &Path, out_dir: &Path, out: &Path, side: u32) -> Vec<std::process::Command> {
    let mut commands = vec![];
    if cfg!(target_os = "macos") {
        // Quick Look writes `<name>.png` into the folder it is given. It
        // is asked only for the kinds macOS can read: given any other, it
        // does not say no, it just never answers.
        if neo_desktop::fs::has_extension(video, &["mov", "mp4", "m4v", "3gp"]) {
            let mut c = std::process::Command::new("qlmanage");
            c.args(["-t", "-s", &side.to_string(), "-o"]).arg(out_dir).arg(video);
            commands.push(c);
        }
    } else {
        let mut c = std::process::Command::new("ffmpegthumbnailer");
        c.arg("-i").arg(video).arg("-o").arg(out).args(["-s", &side.to_string()]);
        commands.push(c);
    }
    // A second in, past any black first frame; scaled to fit the side.
    // ffmpeg reads everything, and Neo counts on its being installed.
    let mut c = std::process::Command::new(neo_desktop::fs::tool("ffmpeg"));
    c.args(["-y", "-loglevel", "error", "-ss", "1", "-i"]).arg(video).args(["-frames:v", "1", "-vf", &format!("scale={side}:{side}:force_original_aspect_ratio=decrease")]).arg(out);
    commands.push(c);
    commands
}

/// How long a tool gets to make one picture. The system's own never
/// answers at all for a file that is not the video its name says it is.
const FRAME_PATIENCE: std::time::Duration = std::time::Duration::from_secs(6);

/// Runs a command, and says whether it ended well within `patience`. One
/// that is still going then is stopped. `None` if there is no such
/// program here to run.
fn finished_in_time(command: &mut std::process::Command, patience: std::time::Duration) -> Option<bool> {
    let mut child = command.spawn().ok()?;
    let until = std::time::Instant::now() + patience;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status.success()),
            Ok(None) if std::time::Instant::now() < until => std::thread::sleep(std::time::Duration::from_millis(25)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Some(false);
            }
        }
    }
}

/// Clears away what was left half-made when the app was closed while a
/// picture was being made.
pub fn tidy_frames(cache: &Path) {
    for left in std::fs::read_dir(cache).into_iter().flatten().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("making-")) {
        let _ = std::fs::remove_dir_all(left.path());
    }
}

/// How each folder is shown, kept between runs: a line a folder, the view
/// and then the path.
pub fn views_file() -> PathBuf {
    neo_desktop::config_dir().join("apps").join("files-views")
}

pub fn load_views(file: &Path) -> Vec<(PathBuf, String)> {
    std::fs::read_to_string(file).map(|text| text.lines().filter_map(|l| l.split_once('\t')).map(|(view, path)| (PathBuf::from(path), view.to_owned())).collect()).unwrap_or_default()
}

pub fn save_views(file: &Path, views: &[(PathBuf, String)]) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text: String = views.iter().map(|(path, view)| format!("{view}\t{}\n", path.display())).collect();
    std::fs::write(file, text)
}

/// A picture of a video, as a file: from the cache if it has been made
/// for this file as it is now, otherwise made and kept. `None` where
/// nothing on this computer can make one.
pub fn video_frame(video: &Path, cache: &Path, side: u32) -> Option<PathBuf> {
    let meta = std::fs::metadata(video).ok()?;
    let kept = cache.join(frame_name(video, &meta));
    if kept.is_file() {
        return Some(kept);
    }
    // Tried before and nothing could make one: not tried again each visit.
    let none = kept.with_extension("none");
    if none.is_file() {
        return None;
    }
    // Made in a folder of its own, since one tool names the file itself.
    let work = cache.join(format!("making-{}-{}", std::process::id(), frame_name(video, &meta)));
    std::fs::create_dir_all(&work).ok()?;
    let out = work.join("frame.png");
    // Whether anything here could even try: if not, it is worth asking
    // again another day, when something that can may have been installed.
    let mut tried = false;
    let made = frame_commands(video, &work, &out, side).into_iter().any(|mut c| {
        let ran = finished_in_time(c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()), FRAME_PATIENCE);
        tried |= ran.is_some();
        let ran = ran == Some(true);
        // Whatever picture it left, under whatever name.
        ran && std::fs::read_dir(&work).ok().and_then(|mut d| d.find_map(|e| e.ok().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "png")))).is_some_and(|p| std::fs::rename(p, &kept).is_ok())
    });
    let _ = std::fs::remove_dir_all(&work);
    if !made && tried {
        let _ = std::fs::write(&none, b"");
    }
    made.then_some(kept)
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
    fn a_videos_picture_is_named_for_the_file_as_it_is_now() {
        let dir = scratch("frames");
        let (a, b) = (dir.join("a.mov"), dir.join("b.mov"));
        std::fs::write(&a, b"one").unwrap();
        std::fs::write(&b, b"one").unwrap();
        let name = |p: &Path| frame_name(p, &std::fs::metadata(p).unwrap());
        assert_ne!(name(&a), name(&b), "two files, two pictures");
        assert_eq!(name(&a), name(&a));
        let before = name(&a);
        std::fs::write(&a, b"one and more").unwrap();
        assert_ne!(name(&a), before, "changed, it gets a new one");
        // Each way of making one is asked for the size wanted, and where to put it.
        let commands = frame_commands(&a, &dir, &dir.join("out.png"), 256);
        assert!(!commands.is_empty());
        for c in &commands {
            let args: Vec<String> = c.get_args().map(|x| x.to_string_lossy().into_owned()).collect();
            assert!(args.iter().any(|x| x.contains("256")) && args.iter().any(|x| x.ends_with("a.mov")), "{args:?}");
        }
        // A file that is not there gives nothing.
        let cache = dir.join("cache");
        assert_eq!(video_frame(&dir.join("missing.mov"), &cache, 256), None);
        // A kind the system's tool cannot read is not given to it, since it
        // would never answer; and what could not be made is not tried twice.
        let mkv = dir.join("clip.mkv");
        std::fs::write(&mkv, b"not really").unwrap();
        if cfg!(target_os = "macos") {
            assert!(frame_commands(&mkv, &dir, &dir.join("o.png"), 256).iter().all(|c| c.get_program() != "qlmanage"));
            assert!(frame_commands(&a, &dir, &dir.join("o.png"), 256).iter().any(|c| c.get_program() == "qlmanage"));
        }
        let began = std::time::Instant::now();
        assert_eq!(video_frame(&mkv, &cache, 256), None);
        assert!(began.elapsed() < std::time::Duration::from_secs(4), "said no quickly: {:?}", began.elapsed());
        // Noted as not to be tried again only if something did try: with
        // nothing here that could, it is worth asking another day.
        let kept: Vec<String> = std::fs::read_dir(&cache).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert!(kept.iter().all(|k| k.ends_with(".none")) && kept.len() <= 1, "{kept:?}");
        // What a closed app left half-made is cleared away.
        std::fs::create_dir_all(cache.join("making-1-x.png")).unwrap();
        tidy_frames(&cache);
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), kept.len());
        // Views are kept a line each, paths with spaces and all.
        let file = dir.join("apps/files-views");
        let views = vec![(PathBuf::from("/Users/sam/My Pictures"), "grid".to_owned()), (PathBuf::from("/work"), "compact".to_owned())];
        save_views(&file, &views).unwrap();
        assert_eq!(load_views(&file), views);
        assert!(load_views(&dir.join("none")).is_empty());
        // A tool that never answers is stopped, not waited on for ever.
        #[cfg(unix)]
        {
            let began = std::time::Instant::now();
            assert_eq!(finished_in_time(std::process::Command::new("sleep").arg("30"), std::time::Duration::from_millis(200)), Some(false));
            assert!(began.elapsed() < std::time::Duration::from_secs(5));
            assert_eq!(finished_in_time(&mut std::process::Command::new("true"), std::time::Duration::from_secs(5)), Some(true));
            assert_eq!(finished_in_time(&mut std::process::Command::new("false"), std::time::Duration::from_secs(5)), Some(false));
            assert_eq!(finished_in_time(&mut std::process::Command::new("neo-files-no-such-tool"), std::time::Duration::from_secs(5)), None);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Makes a picture of a real video with whatever this computer has
    /// for it, by hand: `NEO_VIDEO=/path/to/clip.mov cargo test -p neo-files
    /// -- --ignored --nocapture a_real_video`.
    #[test]
    #[ignore]
    fn a_real_video_gets_a_picture_and_keeps_it() {
        let video = PathBuf::from(std::env::var("NEO_VIDEO").expect("set NEO_VIDEO"));
        let cache = scratch("real-frames");
        let began = std::time::Instant::now();
        let made = video_frame(&video, &cache, 448).expect("a picture of it");
        let first = began.elapsed();
        let (w, h) = image::image_dimensions(&made).expect("a picture that can be read");
        let began = std::time::Instant::now();
        assert_eq!(video_frame(&video, &cache, 448), Some(made.clone()), "the second time it is the one kept");
        println!("{w} x {h}, made in {first:?}, found again in {:?}", began.elapsed());
        assert!(w.max(h) >= 64 && began.elapsed() < first);
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 1, "nothing left over from making it");
        std::fs::remove_dir_all(cache).unwrap();
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
