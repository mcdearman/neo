//! Finding the apps installed on this computer, and opening them.

use std::path::{Path, PathBuf};

use neo::Image;

/// An app that can be opened.
#[derive(Clone, Debug, PartialEq)]
pub struct AppEntry {
    pub name: String,
    /// What to open: an `.app` bundle, a `.desktop` file or a shortcut.
    pub path: PathBuf,
    /// Where it lives, shown beside the name: "Applications", "Utilities"…
    pub place: String,
    /// The command to run on Linux, from the desktop entry's `Exec` line.
    pub exec: Option<String>,
}

fn by_name(apps: &mut Vec<AppEntry>) {
    apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.path.cmp(&b.path)));
    // The same app can be listed twice, as a system copy and a user copy.
    apps.dedup_by(|a, b| a.name == b.name && a.place == b.place);
}

/// App bundles in the usual folders and one level of subfolders, such as Utilities.
#[cfg(target_os = "macos")]
pub fn discover() -> Vec<AppEntry> {
    let home = neo_desktop::fs::home_dir();
    let roots = [PathBuf::from("/Applications"), PathBuf::from("/System/Applications"), PathBuf::from("/System/Library/CoreServices/Applications"), home.join("Applications"), PathBuf::from("/System/Library/CoreServices/Finder.app")];
    let mut apps = Vec::new();
    fn add(path: &Path, apps: &mut Vec<AppEntry>) {
        let Some(name) = path.file_stem().map(|n| n.to_string_lossy().into_owned()) else { return };
        let place = path.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let place = if place == "CoreServices" || place == "Applications" && path.starts_with("/System") { "System".to_string() } else { place };
        apps.push(AppEntry { name, path: path.to_path_buf(), place, exec: None });
    }
    let is_app = |p: &Path| p.extension().is_some_and(|e| e == "app");
    for root in roots {
        if is_app(&root) {
            add(&root, &mut apps);
            continue;
        }
        for entry in std::fs::read_dir(&root).into_iter().flatten().flatten() {
            let path = entry.path();
            if is_app(&path) {
                add(&path, &mut apps);
            } else if path.is_dir() {
                // One level down: Utilities, and folders people make themselves.
                for inner in std::fs::read_dir(&path).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| is_app(p)) {
                    add(&inner, &mut apps);
                }
            }
        }
    }
    by_name(&mut apps);
    apps
}

/// Reads the fields the launcher needs from a desktop entry. `None` for
/// entries that are hidden or are not apps.
#[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]
pub fn parse_desktop_entry(text: &str, path: &Path) -> Option<AppEntry> {
    let (mut name, mut exec, mut kind, mut hidden, mut main) = (None, None, None, false, false);
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            // Only the main group describes the app; actions come after it.
            main = line == "[Desktop Entry]";
            continue;
        }
        if !main {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        match key.trim() {
            "Name" => name = Some(value.trim().to_string()),
            "Exec" => exec = Some(value.trim().to_string()),
            "Type" => kind = Some(value.trim().to_string()),
            "NoDisplay" | "Hidden" if value.trim() == "true" => hidden = true,
            _ => {}
        }
    }
    if hidden || kind.as_deref() != Some("Application") {
        return None;
    }
    Some(AppEntry { name: name?, path: path.to_path_buf(), place: "Applications".into(), exec: Some(exec?) })
}

/// Desktop entries from every `applications` folder the session lists.
#[cfg(all(unix, not(target_os = "macos")))]
pub fn discover() -> Vec<AppEntry> {
    let home = neo_desktop::fs::home_dir();
    let data_home = std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()).map(PathBuf::from).unwrap_or_else(|| home.join(".local/share"));
    let data_dirs = std::env::var("XDG_DATA_DIRS").ok().filter(|d| !d.is_empty()).unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    let mut dirs = vec![data_home];
    dirs.extend(data_dirs.split(':').map(PathBuf::from));
    dirs.push(PathBuf::from("/var/lib/flatpak/exports/share"));
    dirs.push(home.join(".local/share/flatpak/exports/share"));
    let mut apps = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for dir in dirs {
        for entry in std::fs::read_dir(dir.join("applications")).into_iter().flatten().flatten() {
            let path = entry.path();
            // An entry earlier in the list overrides one of the same file name later.
            if path.extension().is_some_and(|e| e == "desktop")
                && seen.insert(entry.file_name())
                && let Some(app) = std::fs::read_to_string(&path).ok().and_then(|t| parse_desktop_entry(&t, &path))
            {
                apps.push(app);
            }
        }
    }
    by_name(&mut apps);
    apps
}

/// Shortcuts in the Start menu, for this user and for everyone.
#[cfg(windows)]
pub fn discover() -> Vec<AppEntry> {
    let mut roots = Vec::new();
    for var in ["APPDATA", "ProgramData"] {
        if let Some(base) = std::env::var_os(var) {
            roots.push(PathBuf::from(base).join(r"Microsoft\Windows\Start Menu\Programs"));
        }
    }
    let mut apps = Vec::new();
    fn walk(dir: &Path, depth: usize, apps: &mut Vec<AppEntry>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() && depth < 3 {
                walk(&path, depth + 1, apps);
            } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("lnk")) {
                let name = path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                // Skip the uninstallers and read-me files that live beside apps.
                let lower = name.to_lowercase();
                if lower.contains("uninstall") || lower.contains("readme") {
                    continue;
                }
                let place = path.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                apps.push(AppEntry { name, path, place: if place == "Programs" { "Start menu".into() } else { place }, exec: None });
            }
        }
    }
    for root in roots {
        walk(&root, 0, &mut apps);
    }
    by_name(&mut apps);
    apps
}

/// The command in a desktop entry's `Exec` line without its `%f`-style
/// placeholders, which stand for files the launcher is not passing.
#[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]
pub fn exec_command(exec: &str) -> String {
    let mut out = String::new();
    let mut chars = exec.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            // "%%" is a literal percent sign; any other code is dropped.
            if let Some('%') = chars.next() {
                out.push('%');
            }
        } else {
            out.push(c);
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Opens the app.
pub fn launch(app: &AppEntry) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(&app.path).spawn().map(|_| ())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let command = exec_command(app.exec.as_deref().unwrap_or_default());
        if command.is_empty() {
            return Err(std::io::Error::other("this entry has no command to run"));
        }
        // The Exec line is a shell-like command; let the shell split it.
        std::process::Command::new("sh").arg("-c").arg(format!("exec {command}")).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().map(|_| ())
    }
    #[cfg(windows)]
    {
        std::process::Command::new("cmd").args(["/C", "start", ""]).arg(&app.path).spawn().map(|_| ())
    }
}

/// The app's own icon, where the platform makes it easy to read.
pub fn icon(app: &AppEntry) -> Option<Image> {
    #[cfg(target_os = "macos")]
    {
        use std::ffi::{c_char, c_int, CString};
        unsafe extern "C" {
            fn neo_app_icon(path: *const c_char, side: c_int, rgba: *mut u8) -> c_int;
        }
        // Twice the size it is shown at, for high-density screens.
        const SIDE: usize = 64;
        let path = CString::new(app.path.to_string_lossy().as_bytes()).ok()?;
        let mut pixels = vec![0u8; SIDE * SIDE * 4];
        // SAFETY: the path is a valid C string and the buffer holds SIDE × SIDE × 4 bytes.
        let ok = unsafe { neo_app_icon(path.as_ptr(), SIDE as c_int, pixels.as_mut_ptr()) } != 0;
        ok.then(|| Image::frame(SIDE as u32, SIDE as u32, pixels))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_desktop_entries() {
        let text = "[Desktop Entry]\nType=Application\nName=Files\nExec=neo-files %f\nIcon=org.neo.Files\n\n[Desktop Action new]\nName=New Window\nExec=neo-files --new\n";
        let app = parse_desktop_entry(text, Path::new("/usr/share/applications/org.neo.Files.desktop")).unwrap();
        assert_eq!((app.name.as_str(), app.exec.as_deref()), ("Files", Some("neo-files %f")), "actions do not override the app's own name");
        assert!(parse_desktop_entry("[Desktop Entry]\nType=Application\nName=Hidden\nExec=x\nNoDisplay=true\n", Path::new("x")).is_none());
        assert!(parse_desktop_entry("[Desktop Entry]\nType=Link\nName=Site\nURL=https://example.org\n", Path::new("x")).is_none());
    }

    #[test]
    fn strips_field_codes_from_exec() {
        assert_eq!(exec_command("neo-files %f"), "neo-files");
        assert_eq!(exec_command("firefox --name=%c %U"), "firefox --name=");
        assert_eq!(exec_command("sh -c \"echo 100%%\""), "sh -c \"echo 100%\"");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn finds_the_apps_every_mac_has() {
        let apps = discover();
        for name in ["Safari", "Finder", "System Settings", "Terminal"] {
            assert!(apps.iter().any(|a| a.name == name), "{name} is listed");
        }
        let safari = apps.iter().find(|a| a.name == "Safari").unwrap();
        let icon = icon(safari).expect("Safari has an icon");
        assert_eq!((icon.width(), icon.height()), (64, 64));
    }
}
