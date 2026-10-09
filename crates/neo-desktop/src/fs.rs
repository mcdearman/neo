//! File-system helpers shared by the desktop apps.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Local};
use neo::icons;
use neo::theme::Icon;

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

/// A standard folder such as `DOCUMENTS`, read from `~/.config/user-dirs.dirs`
/// on Linux and falling back to `~/Documents` style names elsewhere.
pub fn user_dir(xdg_name: &str) -> PathBuf {
    let home = home_dir();
    let fallback = || {
        let mut name = xdg_name.to_lowercase();
        if let Some(first) = name.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        home.join(match xdg_name {
            "DOWNLOAD" => "Downloads".into(),
            // macOS calls its video folder Movies.
            "VIDEOS" if cfg!(target_os = "macos") => "Movies".into(),
            _ => name,
        })
    };
    let config = std::env::var_os("XDG_CONFIG_HOME").filter(|d| !d.is_empty()).map(PathBuf::from).unwrap_or_else(|| home.join(".config"));
    let dirs = config.join("user-dirs.dirs");
    let Ok(src) = std::fs::read_to_string(dirs) else { return fallback() };
    let key = format!("XDG_{xdg_name}_DIR=");
    src.lines()
        .find_map(|l| l.trim().strip_prefix(&key))
        .map(|v| PathBuf::from(v.trim_matches('"').replace("$HOME", &home.to_string_lossy())))
        .unwrap_or_else(fallback)
}

/// Compares names so that "file2" sorts before "file10".
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, _) => return std::cmp::Ordering::Less,
            (_, None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let num = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut s = String::new();
                    while let Some(c) = it.peek().filter(|c| c.is_ascii_digit()) {
                        s.push(*c);
                        it.next();
                    }
                    s
                };
                let (na, nb) = (num(&mut a), num(&mut b));
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let o = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if o != std::cmp::Ordering::Equal {
                    return o;
                }
            }
            (Some(x), Some(y)) => {
                let o = x.to_lowercase().cmp(y.to_lowercase());
                if o != std::cmp::Ordering::Equal {
                    return o;
                }
                a.next();
                b.next();
            }
        }
    }
}

/// A name for `name` in `dir` that nothing has yet: the name itself, then
/// "name copy", "name copy 2" and so on, keeping the extension.
pub fn free_name(dir: &Path, name: &str) -> PathBuf {
    let direct = dir.join(name);
    if direct.symlink_metadata().is_err() {
        return direct;
    }
    let as_path = Path::new(name);
    let stem = as_path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| name.to_string());
    let ext = as_path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (1..)
        .map(|n| dir.join(if n == 1 { format!("{stem} copy{ext}") } else { format!("{stem} copy {n}{ext}") }))
        .find(|p| p.symlink_metadata().is_err())
        .expect("some number is free")
}

/// Copies a file, or a folder and everything in it, to `to`.
pub fn copy_all(from: &Path, to: &Path) -> std::io::Result<()> {
    let meta = from.symlink_metadata()?;
    if meta.is_dir() {
        std::fs::create_dir(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_all(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

/// How files dropped on a folder got there.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Transferred {
    pub moved: usize,
    pub copied: usize,
}

/// Puts `sources` into the folder `target`. Entries that already live in
/// `move_from` are moved, as when dragging within one folder; anything else
/// is copied, so a drop from another app never takes the original away.
/// Sources already in `target`, and a folder dropped into itself, are skipped.
pub fn transfer(sources: &[PathBuf], target: &Path, move_from: Option<&Path>) -> std::io::Result<Transferred> {
    let mut done = Transferred::default();
    for src in sources {
        let Some(name) = src.file_name() else { continue };
        if src.parent() == Some(target) || target.starts_with(src) {
            continue;
        }
        if move_from.is_some() && src.parent() == move_from {
            let dest = target.join(name);
            if dest.symlink_metadata().is_ok() {
                return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, format!("{} already exists in {}", name.to_string_lossy(), target.display())));
            }
            std::fs::rename(src, &dest)?;
            done.moved += 1;
        } else {
            copy_all(src, &free_name(target, &name.to_string_lossy()))?;
            done.copied += 1;
        }
    }
    Ok(done)
}

/// Picture formats Photos opens, by lower-case extension.
pub const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff", "ico", "heic", "heif", "avif"];

/// Video formats Videos opens, by lower-case extension.
pub const VIDEO_EXTENSIONS: &[&str] = &["mp4", "m4v", "mov", "mkv", "webm", "avi", "mpg", "mpeg", "wmv", "flv", "3gp", "ts"];

/// Whether `path` has one of `extensions`, ignoring case.
pub fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| extensions.contains(&e.to_ascii_lowercase().as_str()))
}

/// The tops of the file system: `/` on Unix, each drive on Windows.
pub fn roots() -> Vec<(String, PathBuf)> {
    if cfg!(windows) {
        (b'A'..=b'Z')
            .map(|l| PathBuf::from(format!("{}:\\", l as char)))
            .filter(|p| p.exists())
            .map(|p| (format!("Local Disk ({})", p.to_string_lossy().trim_end_matches('\\')), p))
            .collect()
    } else {
        vec![("Computer".into(), PathBuf::from("/"))]
    }
}

/// `1.4 MB` style sizes, in powers of 1000 like most desktop file managers.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["kB", "MB", "GB", "TB", "PB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut v = bytes as f64 / 1000.0;
    let mut unit = 0;
    while v >= 1000.0 && unit < UNITS.len() - 1 {
        v /= 1000.0;
        unit += 1;
    }
    if v < 10.0 { format!("{v:.1} {}", UNITS[unit]) } else { format!("{v:.0} {}", UNITS[unit]) }
}

/// Binary sizes (`7.8 GiB`) for memory, which is counted in powers of two.
pub fn human_bytes_binary(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["KiB", "MiB", "GiB", "TiB", "PiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut v = bytes as f64 / 1024.0;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    if v < 10.0 { format!("{v:.1} {}", UNITS[unit]) } else { format!("{v:.0} {}", UNITS[unit]) }
}

/// `Today 14:32`, `Yesterday 09:10`, `Mon 12:00` within a week, else `3 Mar 2026`.
pub fn friendly_time(t: SystemTime) -> String {
    let t: DateTime<Local> = t.into();
    let now = Local::now();
    let days = (now.date_naive() - t.date_naive()).num_days();
    match days {
        0 => t.format("Today %H:%M").to_string(),
        1 => t.format("Yesterday %H:%M").to_string(),
        2..=6 => t.format("%a %H:%M").to_string(),
        _ => t.format("%-d %b %Y").to_string(),
    }
}

/// Where a program Neo relies on is: on the `PATH`, or in the places
/// programs are usually installed, which an app started from the Dock or
/// a launcher is not always told about. Just the name if it is nowhere to
/// be found, so that running it says so.
pub fn tool(program: &str) -> PathBuf {
    let name = format!("{program}{}", std::env::consts::EXE_SUFFIX);
    let on_path = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default();
    on_path.into_iter().chain(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin", "/snap/bin"].map(PathBuf::from)).map(|dir| dir.join(&name)).find(|p| p.is_file()).unwrap_or_else(|| PathBuf::from(program))
}

/// A time in full, for where the exact moment matters: "8 Oct 2026, 09:41:07".
pub fn full_time(t: SystemTime) -> String {
    let t: DateTime<Local> = t.into();
    t.format("%-d %b %Y, %H:%M:%S").to_string()
}

/// An icon for a file, chosen by extension.
pub fn file_icon(path: &Path, is_dir: bool) -> Icon {
    if is_dir {
        return icons::FOLDER;
    }
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "heic" | "avif" => icons::FILE_IMAGE,
        "mp3" | "flac" | "ogg" | "wav" | "m4a" | "opus" => icons::FILE_AUDIO,
        "mp4" | "mkv" | "webm" | "mov" | "avi" => icons::FILE_VIDEO,
        "zip" | "tar" | "gz" | "xz" | "zst" | "bz2" | "7z" | "rar" | "deb" | "rpm" => icons::FILE_ARCHIVE,
        "rs" | "c" | "h" | "cpp" | "py" | "js" | "ts" | "go" | "java" | "rb" | "lua" | "zig" | "hs" | "ml" | "html" | "css" | "wgsl" => icons::FILE_CODE,
        "json" => icons::FILE_JSON,
        "csv" | "ods" | "xlsx" => icons::FILE_SPREADSHEET,
        "sh" | "bash" | "zsh" | "fish" => icons::FILE_TERMINAL,
        "txt" | "md" | "toml" | "yaml" | "yml" | "conf" | "ini" | "log" | "pdf" | "odt" | "docx" => icons::FILE_TEXT,
        _ => icons::FILE,
    }
}

/// Opens a file or folder with the desktop's default application.
pub fn open(path: &Path) -> std::io::Result<()> {
    let mut cmd = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    } else {
        std::process::Command::new("xdg-open")
    };
    cmd.arg(path).spawn().map(|_| ())
}

/// Where another Neo app's program might be: beside this program, in a
/// macOS app bundle next to this one or in `~/Applications`, then on the
/// `PATH`. `program` is the binary's name, such as `neo-files`, and
/// `bundle` its macOS app name, such as `Files`.
fn neo_app_candidates(program: &str, bundle: &str) -> Vec<PathBuf> {
    let name = format!("{program}{}", std::env::consts::EXE_SUFFIX);
    let mut out = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        out.push(dir.join(&name));
        // Inside `Some.app/Contents/MacOS`, look for a sibling bundle.
        if let Some(apps) = dir.ancestors().nth(3).filter(|_| dir.ends_with("Contents/MacOS")) {
            out.push(apps.join(format!("{bundle}.app/Contents/MacOS")).join(&name));
        }
    }
    if cfg!(target_os = "macos") {
        out.push(home_dir().join(format!("Applications/{bundle}.app/Contents/MacOS")).join(&name));
    }
    out.retain(|p| p.is_file());
    out.push(PathBuf::from(name));
    out
}

/// Shows a file in Files, selected in its folder. Falls back to the
/// system's file manager, opened on the folder, when Files is not installed.
pub fn reveal(path: &Path) -> std::io::Result<()> {
    if open_in("neo-files", "Files", path) {
        return Ok(());
    }
    open(path.parent().unwrap_or(path))
}

/// Where another Neo app's program is, if it is installed: beside this
/// one, in a bundle beside this one's, or in the user's Applications.
pub fn neo_app(program: &str, bundle: &str) -> Option<PathBuf> {
    neo_app_candidates(program, bundle).into_iter().find(|p| p.is_file())
}

/// Starts another Neo app with these arguments. Returns false if that
/// app is not installed.
pub fn open_with(program: &str, bundle: &str, args: &[&str]) -> bool {
    neo_app_candidates(program, bundle).into_iter().any(|app| std::process::Command::new(app).args(args).stdin(std::process::Stdio::null()).spawn().is_ok())
}

/// Opens `path` in another Neo app. Returns false if that app is not installed.
pub fn open_in(program: &str, bundle: &str, path: &Path) -> bool {
    neo_app_candidates(program, bundle).into_iter().any(|app| std::process::Command::new(app).arg(path).spawn().is_ok())
}

/// Opens a file the way Files does: pictures in Photos, videos in
/// Videos, and everything else with the system's default app. Falls
/// back to the system's choice when the Neo app is not installed.
pub fn open_file(path: &Path) -> std::io::Result<()> {
    let viewer = if has_extension(path, IMAGE_EXTENSIONS) {
        Some(("neo-photos", "Photos"))
    } else if has_extension(path, VIDEO_EXTENSIONS) {
        Some(("neo-videos", "Videos"))
    } else {
        None
    };
    match viewer {
        Some((program, bundle)) if open_in(program, bundle, path) => Ok(()),
        _ => open(path),
    }
}

/// Moves a file or folder to the Trash, so it can be restored later.
///
/// Follows the freedesktop.org Trash specification on Linux and uses
/// `~/.Trash` on macOS. Items on another file system than the home folder
/// are refused rather than copied.
pub fn move_to_trash(path: &Path) -> std::io::Result<PathBuf> {
    let path = std::path::absolute(path)?;
    let name = path.file_name().ok_or_else(|| std::io::Error::other("cannot trash a root folder"))?.to_os_string();
    let (files, info) = if cfg!(target_os = "macos") {
        (home_dir().join(".Trash"), None)
    } else if cfg!(unix) {
        let data = std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()).map(PathBuf::from).unwrap_or_else(|| home_dir().join(".local/share"));
        let trash = data.join("Trash");
        (trash.join("files"), Some(trash.join("info")))
    } else {
        return Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "the Trash is not supported on this platform yet"));
    };
    std::fs::create_dir_all(&files)?;
    if let Some(info) = &info {
        std::fs::create_dir_all(info)?;
    }
    // Pick a free name: "notes.txt", then "notes.2.txt" and so on.
    let stem = Path::new(&name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = Path::new(&name).extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let mut n = 1;
    let (dest, info_file) = loop {
        let candidate = if n == 1 { name.to_string_lossy().into_owned() } else { format!("{stem}.{n}{ext}") };
        let dest = files.join(&candidate);
        let info_file = info.as_ref().map(|i| i.join(format!("{candidate}.trashinfo")));
        let taken = dest.symlink_metadata().is_ok() || info_file.as_ref().is_some_and(|f| f.exists());
        if !taken {
            break (dest, info_file);
        }
        n += 1;
    };
    if let Some(info_file) = &info_file {
        let when = Local::now().format("%Y-%m-%dT%H:%M:%S");
        let encoded = percent_encode_path(&path.to_string_lossy());
        std::fs::write(info_file, format!("[Trash Info]\nPath={encoded}\nDeletionDate={when}\n"))?;
    }
    if let Err(e) = std::fs::rename(&path, &dest) {
        if let Some(info_file) = &info_file {
            let _ = std::fs::remove_file(info_file);
        }
        return Err(if e.kind() == std::io::ErrorKind::CrossesDevices { std::io::Error::other("this item is on another drive, which has no Trash here") } else { e });
    }
    Ok(dest)
}

fn percent_encode_path(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(human_size(999), "999 B");
        assert_eq!(human_size(1_400_000), "1.4 MB");
        assert_eq!(human_size(52_000_000_000), "52 GB");
        assert_eq!(human_bytes_binary(8 * 1024 * 1024 * 1024), "8.0 GiB");
    }

    #[test]
    fn natural_order() {
        let mut v = vec!["file10", "File2", "file1", "a"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["a", "file1", "File2", "file10"]);
    }

    #[test]
    fn knows_pictures_and_videos_by_extension() {
        assert!(has_extension(Path::new("/a/Holiday.JPG"), IMAGE_EXTENSIONS));
        assert!(has_extension(Path::new("clip.mov"), VIDEO_EXTENSIONS));
        assert!(!has_extension(Path::new("notes.txt"), IMAGE_EXTENSIONS));
        assert!(!has_extension(Path::new("png"), IMAGE_EXTENSIONS), "a name is not an extension");
    }

    #[test]
    fn drops_move_within_a_folder_and_copy_from_outside() {
        let root = std::env::temp_dir().join(format!("neo-transfer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("here/sub")).unwrap();
        std::fs::create_dir_all(root.join("elsewhere/album")).unwrap();
        std::fs::write(root.join("here/a.txt"), "a").unwrap();
        std::fs::write(root.join("elsewhere/b.txt"), "b").unwrap();
        std::fs::write(root.join("elsewhere/album/c.txt"), "c").unwrap();
        let here = root.join("here");

        // Dragged from this folder onto its subfolder: moved.
        let done = transfer(&[here.join("a.txt")], &here.join("sub"), Some(&here)).unwrap();
        assert_eq!(done, Transferred { moved: 1, copied: 0 });
        assert!(here.join("sub/a.txt").exists() && !here.join("a.txt").exists());

        // From somewhere else, a file and a folder: copied, originals kept.
        let from = root.join("elsewhere");
        let done = transfer(&[from.join("b.txt"), from.join("album")], &here, Some(&here)).unwrap();
        assert_eq!(done, Transferred { moved: 0, copied: 2 });
        assert!(here.join("b.txt").exists() && from.join("b.txt").exists() && here.join("album/c.txt").exists());

        // Again: the copies get new names instead of replacing anything.
        transfer(&[from.join("b.txt")], &here, Some(&here)).unwrap();
        assert!(here.join("b copy.txt").exists());
        assert_eq!(free_name(&here, "b.txt"), here.join("b copy 2.txt"));

        // Dropping where it already is, or a folder into itself, does nothing.
        assert_eq!(transfer(&[here.join("b.txt")], &here, Some(&here)).unwrap(), Transferred::default());
        assert_eq!(transfer(&[here.join("sub")], &here.join("sub"), Some(&here)).unwrap(), Transferred::default());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn encodes_trash_paths() {
        assert_eq!(percent_encode_path("/home/a b/ä.txt"), "/home/a%20b/%C3%A4.txt");
    }
}
