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
        home.join(if xdg_name == "DOWNLOAD" { "Downloads".into() } else { name })
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

/// Where the Neo Files program might be: beside this program, in a macOS
/// app bundle next to this one or in `~/Applications`, then on the `PATH`.
fn neo_files_candidates() -> Vec<PathBuf> {
    let name = format!("neo-files{}", std::env::consts::EXE_SUFFIX);
    let mut out = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        out.push(dir.join(&name));
        // Inside `Some.app/Contents/MacOS`, look for a sibling bundle.
        if let Some(apps) = dir.ancestors().nth(3).filter(|_| dir.ends_with("Contents/MacOS")) {
            out.push(apps.join("Neo Files.app/Contents/MacOS").join(&name));
        }
    }
    if cfg!(target_os = "macos") {
        out.push(home_dir().join("Applications/Neo Files.app/Contents/MacOS").join(&name));
    }
    out.retain(|p| p.is_file());
    out.push(PathBuf::from(name));
    out
}

/// Shows a file in Neo Files, selected in its folder. Falls back to the
/// system's file manager, opened on the folder, when Neo Files is not installed.
pub fn reveal(path: &Path) -> std::io::Result<()> {
    for files in neo_files_candidates() {
        if std::process::Command::new(files).arg(path).spawn().is_ok() {
            return Ok(());
        }
    }
    open(path.parent().unwrap_or(path))
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
    fn encodes_trash_paths() {
        assert_eq!(percent_encode_path("/home/a b/ä.txt"), "/home/a%20b/%C3%A4.txt");
    }
}
