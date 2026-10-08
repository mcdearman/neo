//! What there is to know about one file or folder, for its Properties.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A file's or folder's details, read once when asked for.
#[derive(Clone, Debug, PartialEq)]
pub struct Properties {
    pub path: PathBuf,
    pub name: String,
    pub dir: bool,
    /// What kind of thing it is, in words.
    pub kind: String,
    /// A file's length in bytes. A folder's is added up separately.
    pub size: u64,
    /// The room it takes on disk, where the system says.
    pub on_disk: Option<u64>,
    /// How many things are directly inside a folder.
    pub items: Option<usize>,
    /// Where a link points.
    pub link: Option<PathBuf>,
    pub hidden: bool,
    pub created: Option<SystemTime>,
    pub modified: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
    /// Who may do what, as `rwxr-xr-x`, and as the number that stands for it.
    pub permissions: Option<(String, u32)>,
    /// The user and group it belongs to.
    pub owner: Option<(String, String)>,
    pub read_only: bool,
    /// A picture's width and height in pixels.
    pub pixels: Option<(u32, u32)>,
}

/// Permission bits as `ls` writes them: read, write and execute, for the
/// owner, the group and everyone else.
#[cfg_attr(not(unix), allow(dead_code))]
pub fn mode_string(mode: u32) -> String {
    (0..9).map(|i| if mode & (0o400 >> i) != 0 { ['r', 'w', 'x'][i % 3] } else { '-' }).collect()
}

/// What kind of file a name suggests.
pub fn kind_of(path: &Path, dir: bool, link: bool) -> String {
    if link {
        return if dir { "Link to a folder".into() } else { "Link".into() };
    }
    if dir {
        return if path.extension().is_some_and(|e| e == "app") { "Application".into() } else { "Folder".into() };
    }
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let known = match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "heic" | "avif" | "ico" => "image",
        "svg" => "vector image",
        "mp4" | "mov" | "mkv" | "webm" | "avi" | "m4v" => "video",
        "mp3" | "wav" | "flac" | "ogg" | "m4a" | "aac" => "audio",
        "pdf" => return "PDF document".into(),
        "txt" => return "Plain text".into(),
        "md" => return "Markdown document".into(),
        "rs" => return "Rust source".into(),
        "mw" => return "Meadow source".into(),
        "py" => return "Python script".into(),
        "js" | "mjs" => return "JavaScript source".into(),
        "ts" | "tsx" => return "TypeScript source".into(),
        "c" | "h" => return "C source".into(),
        "cpp" | "cc" | "hpp" => return "C++ source".into(),
        "go" => return "Go source".into(),
        "hs" => return "Haskell source".into(),
        "sh" | "zsh" | "bash" => return "Shell script".into(),
        "json" => return "JSON document".into(),
        "toml" | "yaml" | "yml" | "ini" | "conf" => return "Configuration file".into(),
        "html" | "htm" => return "Web page".into(),
        "css" => return "Style sheet".into(),
        "zip" | "tar" | "gz" | "xz" | "bz2" | "7z" | "zst" => "archive",
        "dmg" | "iso" => "disk image",
        "lock" => return "Lock file".into(),
        "" => return "Document".into(),
        _ => return format!("{} file", ext.to_uppercase()),
    };
    format!("{} {known}", ext.to_uppercase())
}

#[cfg(unix)]
fn names(uid: u32, gid: u32) -> (String, String) {
    // SAFETY: the pointers come from the C library and are read at once,
    // before anything else could ask it for another.
    let user = unsafe {
        let p = libc::getpwuid(uid);
        (!p.is_null() && !(*p).pw_name.is_null()).then(|| std::ffi::CStr::from_ptr((*p).pw_name).to_string_lossy().into_owned())
    };
    let group = unsafe {
        let g = libc::getgrgid(gid);
        (!g.is_null() && !(*g).gr_name.is_null()).then(|| std::ffi::CStr::from_ptr((*g).gr_name).to_string_lossy().into_owned())
    };
    (user.unwrap_or_else(|| uid.to_string()), group.unwrap_or_else(|| gid.to_string()))
}

impl Properties {
    /// Reads what the system knows about `path`. `None` if it has gone.
    pub fn read(path: &Path) -> Option<Self> {
        let own = std::fs::symlink_metadata(path).ok()?;
        let link = own.file_type().is_symlink();
        // A link is described by what it points to, where that is still there.
        let meta = std::fs::metadata(path).unwrap_or_else(|_| own.clone());
        let dir = meta.is_dir();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
        #[cfg(unix)]
        let (permissions, owner, on_disk) = {
            use std::os::unix::fs::MetadataExt;
            (Some((mode_string(meta.mode()), meta.mode() & 0o7777)), Some(names(meta.uid(), meta.gid())), (!dir).then(|| meta.blocks() * 512))
        };
        #[cfg(not(unix))]
        let (permissions, owner, on_disk) = (None, None, None);
        Some(Self {
            hidden: name.starts_with('.'),
            kind: kind_of(path, dir, link),
            size: if dir { 0 } else { meta.len() },
            on_disk,
            items: dir.then(|| std::fs::read_dir(path).map(|r| r.count()).ok()).flatten(),
            link: link.then(|| std::fs::read_link(path).ok()).flatten(),
            created: meta.created().ok(),
            modified: meta.modified().ok(),
            accessed: meta.accessed().ok(),
            permissions,
            owner,
            read_only: meta.permissions().readonly(),
            pixels: (!dir && neo_desktop::fs::has_extension(path, neo_desktop::fs::IMAGE_EXTENSIONS)).then(|| image::image_dimensions(path).ok()).flatten(),
            path: path.to_path_buf(),
            name,
            dir,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permissions_read_as_ls_writes_them() {
        assert_eq!(mode_string(0o755), "rwxr-xr-x");
        assert_eq!(mode_string(0o640), "rw-r-----");
        assert_eq!(mode_string(0o100644), "rw-r--r--", "the bits that say what kind of file it is are not permissions");
        assert_eq!(mode_string(0), "---------");
    }

    #[test]
    fn kinds_are_named_in_words() {
        let kind = |name: &str| kind_of(Path::new(name), false, false);
        assert_eq!((kind("a.PNG"), kind("clip.mov"), kind("lib.rs"), kind("Main.mw")), ("PNG image".into(), "MOV video".into(), "Rust source".into(), "Meadow source".into()));
        assert_eq!((kind("notes"), kind("data.xyz"), kind("pack.tar")), ("Document".into(), "XYZ file".into(), "TAR archive".into()));
        assert_eq!(kind_of(Path::new("src"), true, false), "Folder");
        assert_eq!(kind_of(Path::new("Files.app"), true, false), "Application");
        assert_eq!((kind_of(Path::new("x"), true, true), kind_of(Path::new("x"), false, true)), ("Link to a folder".into(), "Link".into()));
    }

    #[test]
    fn a_files_and_a_folders_details_are_read() {
        let dir = std::env::temp_dir().join(format!("neo-files-props-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("inside")).unwrap();
        std::fs::write(dir.join("notes.txt"), vec![b'x'; 1500]).unwrap();
        std::fs::write(dir.join(".secret"), "x").unwrap();
        image::RgbaImage::new(40, 30).save(dir.join("pic.png")).unwrap();
        let file = Properties::read(&dir.join("notes.txt")).unwrap();
        assert_eq!((file.name.as_str(), file.kind.as_str(), file.size, file.dir, file.items, file.hidden), ("notes.txt", "Plain text", 1500, false, None, false));
        assert!(file.modified.is_some() && !file.read_only && file.link.is_none());
        let folder = Properties::read(&dir).unwrap();
        assert_eq!((folder.dir, folder.items, folder.kind.as_str()), (true, Some(4), "Folder"));
        assert_eq!(Properties::read(&dir.join("pic.png")).unwrap().pixels, Some((40, 30)));
        assert!(Properties::read(&dir.join(".secret")).unwrap().hidden);
        assert_eq!(Properties::read(&dir.join("missing")), None);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.join("notes.txt"), std::fs::Permissions::from_mode(0o640)).unwrap();
            let p = Properties::read(&dir.join("notes.txt")).unwrap();
            assert_eq!(p.permissions, Some(("rw-r-----".into(), 0o640)));
            assert!(p.on_disk.is_some_and(|d| d >= 1500) && p.owner.is_some_and(|(user, group)| !user.is_empty() && !group.is_empty()));
            std::os::unix::fs::symlink(dir.join("inside"), dir.join("shortcut")).unwrap();
            let link = Properties::read(&dir.join("shortcut")).unwrap();
            assert_eq!((link.kind.as_str(), link.link, link.dir), ("Link to a folder", Some(dir.join("inside")), true));
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
