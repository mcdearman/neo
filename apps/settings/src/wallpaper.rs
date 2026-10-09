//! The desktop picture: which there are to choose from, which is set,
//! and setting another.
//!
//! On macOS the system is asked directly. On Linux each desktop keeps
//! its picture its own way; GNOME's and its relations' is set through
//! `gsettings`, and others by `swww` or `feh` where those are there.
//! What was chosen is also noted in Neo's settings, for Neo's own shell
//! to draw when it has a desktop of its own.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Kinds of picture that can be a desktop picture.
const PICTURES: &[&str] = &["jpg", "jpeg", "png", "heic", "heif", "webp", "tif", "tiff", "bmp"];
/// How many pictures are offered at most.
const MOST: usize = 80;
/// The longest side of the small pictures shown to choose from.
pub const THUMB_SIDE: u32 = 360;

/// Where the picture last chosen in Settings is noted.
pub fn noted_file() -> PathBuf {
    neo_desktop::config_dir().join("wallpaper")
}

/// The picture last chosen in Settings, if one was.
pub fn noted() -> Option<PathBuf> {
    std::fs::read_to_string(noted_file()).ok().map(|s| PathBuf::from(s.trim())).filter(|p| p.is_file())
}

/// The folders pictures come with the system in, and the user's own.
pub fn folders() -> Vec<PathBuf> {
    let home = neo_desktop::fs::home_dir();
    let mut out: Vec<PathBuf> = if cfg!(target_os = "macos") {
        vec!["/System/Library/Desktop Pictures".into(), "/Library/Desktop Pictures".into()]
    } else if cfg!(windows) {
        vec![r"C:\Windows\Web\Wallpaper".into()]
    } else {
        vec!["/usr/share/backgrounds".into(), "/usr/share/wallpapers".into(), home.join(".local/share/backgrounds")]
    };
    out.push(neo_desktop::fs::user_dir("PICTURES").join("Wallpapers"));
    out
}

fn is_picture(path: &Path) -> bool {
    path.extension().is_some_and(|e| PICTURES.contains(&e.to_string_lossy().to_lowercase().as_str()))
}

/// The pictures in `folders` and the folders in them, by name, each
/// once: no more than [`MOST`], and nothing hidden.
pub fn pictures_in(folders: &[PathBuf]) -> Vec<PathBuf> {
    fn into(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'))).collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                // Not into what the system keeps small copies of its own pictures in.
                if depth < 2 && !path.ends_with(".thumbnails") {
                    into(&path, depth + 1, out);
                }
            } else if is_picture(&path) && !out.contains(&path) {
                out.push(path);
            }
        }
    }
    let mut out = vec![];
    for folder in folders {
        into(folder, 0, &mut out);
    }
    out.truncate(MOST);
    out
}

/// A picture's name as it is shown: the file's, without what kind it is.
pub fn name(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// A small copy of a picture to choose it by: its size and its pixels as
/// RGBA. Kinds this cannot read itself, such as the HEIC pictures macOS
/// comes with, are made smaller by the system's own converter first.
pub fn thumbnail(path: &Path) -> Option<(u32, u32, Vec<u8>)> {
    let small = |picture: image::DynamicImage| {
        let small = picture.thumbnail(THUMB_SIDE, THUMB_SIDE).into_rgba8();
        let (w, h) = small.dimensions();
        (w, h, small.into_raw())
    };
    if let Ok(picture) = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?.decode() {
        return Some(small(picture));
    }
    if !cfg!(target_os = "macos") {
        return None;
    }
    let out = std::env::temp_dir().join(format!("neo-settings-thumb-{}-{:x}.jpg", std::process::id(), path.to_string_lossy().bytes().fold(0u64, |h, b| h.wrapping_mul(31).wrapping_add(u64::from(b)))));
    let made = Command::new("/usr/bin/sips").args(["-s", "format", "jpeg", "-Z", &THUMB_SIDE.to_string()]).arg(path).arg("--out").arg(&out).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
    let picture = made.ok().filter(|s| s.success()).and_then(|_| image::open(&out).ok());
    let _ = std::fs::remove_file(&out);
    picture.map(small)
}

/// Reads `gsettings get org.gnome.desktop.background picture-uri`:
/// `'file:///usr/share/backgrounds/a%20b.jpg'`.
#[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]
pub fn parse_uri(text: &str) -> Option<PathBuf> {
    let uri = text.trim().trim_matches('\'').trim_matches('"');
    let path = uri.strip_prefix("file://")?;
    // Spaces and the like are written as %20 and so on.
    let mut out = Vec::with_capacity(path.len());
    let bytes = path.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match (bytes[i], bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok().filter(|p| !p.is_empty()).map(PathBuf::from)
}

/// The picture on the desktop now, where the system says.
pub fn current() -> Option<PathBuf> {
    imp::current().or_else(noted)
}

/// Puts a picture on the desktop, and notes it. An error says why it
/// could not be done.
pub fn set(path: &Path) -> Result<(), String> {
    if !path.is_file() {
        return Err(format!("{} is not there any more.", path.display()));
    }
    imp::set(path)?;
    let noted = noted_file();
    if let Some(dir) = noted.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(noted, path.to_string_lossy().as_bytes()).map_err(|e| format!("The picture was set, but could not be noted: {e}"))
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;

    unsafe extern "C" {
        fn neo_set_wallpaper(path: *const std::ffi::c_char) -> std::ffi::c_int;
        fn neo_get_wallpaper(out: *mut std::ffi::c_char, length: std::ffi::c_int) -> std::ffi::c_int;
    }

    pub fn current() -> Option<PathBuf> {
        let mut buf = vec![0u8; 4096];
        // SAFETY: the buffer is as long as is said, and is ended with a zero if anything is written.
        let found = unsafe { neo_get_wallpaper(buf.as_mut_ptr().cast(), buf.len() as std::ffi::c_int) } != 0;
        let end = buf.iter().position(|b| *b == 0)?;
        found.then(|| PathBuf::from(String::from_utf8_lossy(&buf[..end]).into_owned())).filter(|p| p.is_file())
    }

    pub fn set(path: &Path) -> Result<(), String> {
        let c = std::ffi::CString::new(path.to_string_lossy().as_bytes()).map_err(|_| "The picture's name cannot be given to the system.".to_owned())?;
        // SAFETY: the string outlives the call, which is made on the main thread.
        if unsafe { neo_set_wallpaper(c.as_ptr()) } != 0 { Ok(()) } else { Err("The system would not take it as the desktop picture.".into()) }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use super::*;

    fn run(program: &str, args: &[&str]) -> Option<String> {
        let tool = neo_desktop::fs::tool(program);
        let out = Command::new(tool).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    }

    pub fn current() -> Option<PathBuf> {
        run("gsettings", &["get", "org.gnome.desktop.background", "picture-uri"]).and_then(|t| parse_uri(&t)).filter(|p| p.is_file())
    }

    /// Each desktop has its own way; the first that takes it will do.
    pub fn set(path: &Path) -> Result<(), String> {
        let (plain, uri) = (path.to_string_lossy().into_owned(), format!("file://{}", path.display()));
        let gnome = run("gsettings", &["set", "org.gnome.desktop.background", "picture-uri", &uri]).is_some();
        if gnome {
            // The dark style has a picture of its own, which is to be the same.
            let _ = run("gsettings", &["set", "org.gnome.desktop.background", "picture-uri-dark", &uri]);
            return Ok(());
        }
        if run("swww", &["img", &plain]).is_some() || run("feh", &["--bg-fill", &plain]).is_some() {
            return Ok(());
        }
        Err("This desktop keeps its picture in a way Settings does not know. It is noted for Neo's own shell.".into())
    }
}

#[cfg(not(unix))]
mod imp {
    use super::*;

    pub fn current() -> Option<PathBuf> {
        None
    }

    pub fn set(_path: &Path) -> Result<(), String> {
        Err("Setting the desktop picture is not there yet on this system.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_are_found_in_folders_by_name_and_nothing_else_is() {
        let dir = std::env::temp_dir().join(format!("neo-settings-wall-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Nature/Deep/Deeper/Deepest")).unwrap();
        std::fs::create_dir_all(dir.join(".thumbnails")).unwrap();
        for file in ["b.JPG", "a.png", "notes.txt", ".hidden.png", "Nature/hill.heic", "Nature/Deep/lake.webp", "Nature/Deep/Deeper/far.png", "Nature/Deep/Deeper/Deepest/too far.png", ".thumbnails/small.png"] {
            std::fs::write(dir.join(file), b"x").unwrap();
        }
        let found: Vec<String> = pictures_in(&[dir.clone(), dir.clone(), dir.join("nowhere")]).iter().map(|p| p.strip_prefix(&dir).unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(found, ["Nature/Deep/lake.webp", "Nature/hill.heic", "a.png", "b.JPG"], "by name, folders gone into two deep and no deeper, each once");
        assert_eq!((name(Path::new("/x/Big Sur Coast.heic")), name(Path::new("/x/plain"))), ("Big Sur Coast".to_owned(), "plain".to_owned()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_picture_is_made_small_to_choose_it_by() {
        let path = std::env::temp_dir().join(format!("neo-settings-wall-{}.png", std::process::id()));
        image::RgbImage::from_pixel(1600, 900, image::Rgb([20, 120, 220])).save(&path).unwrap();
        let (w, h, rgba) = thumbnail(&path).expect("a PNG is read");
        assert_eq!((w, &rgba[..4]), (THUMB_SIDE, &[20u8, 120, 220, 255][..]));
        assert!(h.abs_diff(THUMB_SIDE * 9 / 16) <= 1, "its shape kept: {h}");
        std::fs::remove_file(&path).unwrap();
        assert!(thumbnail(Path::new("/nowhere/at/all.png")).is_none());
        // One that is gone cannot be set, and it is said why.
        assert!(set(Path::new("/nowhere/at/all.png")).unwrap_err().ends_with("is not there any more."));
    }

    #[test]
    fn a_picture_named_as_an_address_is_read() {
        assert_eq!(parse_uri("'file:///usr/share/backgrounds/Big%20Sur%20Coast.jpg'\n"), Some("/usr/share/backgrounds/Big Sur Coast.jpg".into()));
        assert_eq!(parse_uri("\"file:///a/b.png\""), Some("/a/b.png".into()));
        assert_eq!((parse_uri("''"), parse_uri("'none'"), parse_uri("")), (None, None, None));
        assert_eq!(parse_uri("'file:///100%.png'"), Some("/100%.png".into()), "a percent sign that is only that");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn this_mac_has_pictures_and_says_which_is_set() {
        assert!(!pictures_in(&folders()).is_empty(), "the system comes with some");
        // Whatever is on the desktop is a file, where the system names one.
        assert!(imp::current().is_none_or(|p| p.is_file()));
    }
}

#[cfg(test)]
mod probe {
    /// By hand: puts the picture that is on the desktop on the desktop,
    /// which changes nothing to look at and shows that setting one works.
    /// `cargo test -p neo-settings same_picture -- --ignored --nocapture`
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn the_same_picture_can_be_set_again() {
        let now = super::imp::current().expect("a picture is set");
        println!("on the desktop: {}", now.display());
        super::imp::set(&now).expect("the system takes it");
        assert_eq!(super::imp::current(), Some(now));
    }
}
