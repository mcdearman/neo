//! Installing into each platform's app launcher.

use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(target_os = "macos")]
use crate::apps::AppInfo;
use crate::apps::APPS;
use crate::{built, root};

fn io(context: impl std::fmt::Display) -> impl FnOnce(std::io::Error) -> String {
    move |e| format!("{context}: {e}")
}

fn home() -> PathBuf {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).expect("no home folder")
}

fn copy(from: &Path, to: &Path) -> Result<(), String> {
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir).map_err(io(dir.display()))?;
    }
    // Replace rather than overwrite in place, so a running copy keeps working.
    let _ = std::fs::remove_file(to);
    std::fs::copy(from, to).map_err(io(format!("copy {} to {}", from.display(), to.display())))?;
    Ok(())
}

fn icons_dir() -> PathBuf {
    root().join("dist/icons")
}

/// Runs a helper tool and ignores failure, for optional cache refreshes.
#[cfg(unix)]
fn try_run(program: &str, args: &[&str]) {
    let _ = Command::new(program).args(args).output();
}

// ---------------------------------------------------------------------------
// macOS

#[cfg(target_os = "macos")]
fn app_dir() -> PathBuf {
    home().join("Applications")
}

#[cfg(target_os = "macos")]
fn bundle(app: &AppInfo) -> PathBuf {
    app_dir().join(format!("{}.app", app.name))
}

/// The bundle this app was installed as under its earlier name, if that
/// is still there and really is this app.
#[cfg(target_os = "macos")]
fn former_bundle(app: &AppInfo) -> Option<PathBuf> {
    let old = app_dir().join(format!("{}.app", app.former));
    let ours = std::fs::read_to_string(old.join("Contents/Info.plist")).is_ok_and(|plist| plist.contains(&format!("<string>{}</string>", app.id)));
    (app.former != app.name && ours).then_some(old)
}

#[cfg(target_os = "macos")]
fn info_plist(app: &AppInfo) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key><string>en</string>
    <key>CFBundleDisplayName</key><string>{name}</string>
    <key>CFBundleExecutable</key><string>{bin}</string>
    <key>CFBundleIconFile</key><string>AppIcon</string>
    <key>CFBundleIdentifier</key><string>{id}</string>
    <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
    <key>CFBundleName</key><string>{name}</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>{version}</string>
    <key>CFBundleVersion</key><string>{version}</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.utilities</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>NSHighResolutionCapable</key><true/>{usage}
    <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict>
</plist>
"#,
        name = app.name,
        bin = app.bin,
        id = app.id,
        version = env!("CARGO_PKG_VERSION"),
        usage = {
            let mut extra = String::new();
            if app.background {
                // No Dock icon for an app that waits behind a shortcut.
                extra.push_str("\n    <key>LSUIElement</key><true/>");
            }
            if app.bin == "neo-recorder" {
                // macOS refuses microphone access to apps that do not say why they want it.
                extra.push_str("\n    <key>NSMicrophoneUsageDescription</key><string>NeoCap records sound from the microphone when you turn that option on.</string>");
            }
            if app.bin == "neo-settings" {
                // And Bluetooth, which Settings turns on and off.
                extra.push_str("\n    <key>NSBluetoothAlwaysUsageDescription</key><string>Settings turns Bluetooth on and off and shows the devices it knows.</string>");
            }
            extra
        },
    )
}

#[cfg(target_os = "macos")]
pub fn install() -> Result<(), String> {
    for app in APPS {
        // Update in place, so Spotlight keeps the bundle it already indexed.
        let b = bundle(app);
        if let Some(old) = former_bundle(app) {
            std::fs::remove_dir_all(&old).map_err(io(old.display()))?;
            println!("removed {}, now called {}", old.display(), app.name);
        }
        let contents = b.join("Contents");
        copy(&built(app.bin), &contents.join("MacOS").join(app.bin))?;
        std::fs::write(contents.join("Info.plist"), info_plist(app)).map_err(io("write Info.plist"))?;
        let resources = contents.join("Resources");
        std::fs::create_dir_all(&resources).map_err(io(resources.display()))?;
        let iconset = icons_dir().join(format!("macos/{}.iconset", app.id));
        let icns = resources.join("AppIcon.icns");
        let ok = Command::new("iconutil").arg("-c").arg("icns").arg(&iconset).arg("-o").arg(&icns).status().is_ok_and(|s| s.success());
        if !ok {
            eprintln!("warning: could not make {}; {} will have a generic icon", icns.display(), app.name);
        }
        // An ad-hoc signature, so macOS treats the bundle as one signed app.
        // By default such a signature is identified by a hash of the binary,
        // so every rebuild would look like a new app and lose its privacy
        // permissions (screen recording, microphone). Naming the app ID as
        // the requirement keeps them across reinstalls.
        let requirement = format!("=designated => identifier \"{}\"", app.id);
        try_run("codesign", &["--force", "--deep", "--sign", "-", "--identifier", app.id, "-r", &requirement, &b.to_string_lossy()]);
        try_run("/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister", &["-f", &b.to_string_lossy()]);
        println!("installed {}", b.display());
    }
    println!("\nOpen them from Launchpad, or from {} in the Finder. Spotlight finds them within a minute or so.", app_dir().display());
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn uninstall() -> Result<(), String> {
    for app in APPS {
        // A removed app must not be started at login.
        let _ = std::fs::remove_file(home().join("Library/LaunchAgents").join(format!("{}.plist", app.id)));
        for b in [Some(bundle(app)), former_bundle(app)].into_iter().flatten() {
            if b.exists() {
                std::fs::remove_dir_all(&b).map_err(io(b.display()))?;
                println!("removed {}", b.display());
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Linux and other freedesktop.org systems

#[cfg(all(unix, not(target_os = "macos")))]
fn prefix() -> PathBuf {
    std::env::var_os("PREFIX").map(PathBuf::from).unwrap_or_else(|| home().join(".local"))
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn install() -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let prefix = prefix();
    let bin = prefix.join("bin");
    let share = prefix.join("share");
    for app in APPS {
        let exe = bin.join(app.bin);
        copy(&built(app.bin), &exe)?;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).map_err(io(exe.display()))?;
        let entry = format!("{}.desktop", app.id);
        copy(&root().join("dist/applications").join(&entry), &share.join("applications").join(&entry))?;
        for px in crate::icons::SIZES {
            let rel = format!("{px}x{px}/apps/{}.png", app.id);
            copy(&icons_dir().join("hicolor").join(&rel), &share.join("icons/hicolor").join(&rel))?;
        }
        println!("installed {}", app.name);
    }
    try_run("update-desktop-database", &[&share.join("applications").to_string_lossy()]);
    try_run("gtk-update-icon-cache", &["--force", "--ignore-theme-index", &share.join("icons/hicolor").to_string_lossy()]);
    println!("\nThe apps are in your desktop's app menu. If they don't appear yet, log out and back in.");
    let on_path = std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d == bin));
    if !on_path {
        println!("To start them from a terminal, add {} to your PATH.", bin.display());
    }
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn uninstall() -> Result<(), String> {
    let prefix = prefix();
    for app in APPS {
        let _ = std::fs::remove_file(prefix.join("bin").join(app.bin));
        let _ = std::fs::remove_file(prefix.join("share/applications").join(format!("{}.desktop", app.id)));
        // A removed app must not be started at login.
        let config = std::env::var_os("XDG_CONFIG_HOME").filter(|d| !d.is_empty()).map(PathBuf::from).unwrap_or_else(|| home().join(".config"));
        let _ = std::fs::remove_file(config.join("autostart").join(format!("{}.desktop", app.id)));
        for px in crate::icons::SIZES {
            let _ = std::fs::remove_file(prefix.join(format!("share/icons/hicolor/{px}x{px}/apps/{}.png", app.id)));
        }
        println!("removed {}", app.name);
    }
    try_run("update-desktop-database", &[&prefix.join("share/applications").to_string_lossy()]);
    Ok(())
}

// ---------------------------------------------------------------------------
// Windows

#[cfg(windows)]
fn install_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| home().join("AppData/Local")).join("Programs").join("Neo")
}

#[cfg(windows)]
fn start_menu() -> PathBuf {
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| home().join("AppData/Roaming")).join(r"Microsoft\Windows\Start Menu\Programs\Neo")
}

/// Packs PNG images into an .ico file. Windows reads PNG-compressed entries
/// since Vista.
#[cfg(windows)]
fn write_ico(pngs: &[(u32, Vec<u8>)], path: &Path) -> std::io::Result<()> {
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(pngs.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * pngs.len() as u32;
    for (px, data) in pngs {
        let dim = if *px >= 256 { 0 } else { *px as u8 };
        out.extend_from_slice(&[dim, dim, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += data.len() as u32;
    }
    for (_, data) in pngs {
        out.extend_from_slice(data);
    }
    std::fs::write(path, out)
}

#[cfg(windows)]
fn ps_quote(p: &Path) -> String {
    format!("'{}'", p.to_string_lossy().replace('\'', "''"))
}

#[cfg(windows)]
pub fn install() -> Result<(), String> {
    let dir = install_dir();
    let menu = start_menu();
    std::fs::create_dir_all(&menu).map_err(io(menu.display()))?;
    for app in APPS {
        let exe = dir.join(format!("{}.exe", app.bin));
        copy(&built(app.bin), &exe)?;
        let mut pngs = Vec::new();
        for px in [16, 24, 32, 48, 64, 256] {
            let png = icons_dir().join(format!("hicolor/{px}x{px}/apps/{}.png", app.id));
            pngs.push((px, std::fs::read(&png).map_err(io(png.display()))?));
        }
        let ico = dir.join(format!("{}.ico", app.bin));
        write_ico(&pngs, &ico).map_err(io(ico.display()))?;
        let _ = std::fs::remove_file(menu.join(format!("{}.lnk", app.former)));
        let lnk = menu.join(format!("{}.lnk", app.name));
        let script = format!(
            "$s = (New-Object -ComObject WScript.Shell).CreateShortcut({lnk}); $s.TargetPath = {exe}; $s.IconLocation = {ico}; $s.WorkingDirectory = $env:USERPROFILE; $s.Save()",
            lnk = ps_quote(&lnk),
            exe = ps_quote(&exe),
            ico = ps_quote(&ico),
        );
        let ok = Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", &script]).status().is_ok_and(|s| s.success());
        if !ok {
            return Err(format!("could not create the Start menu shortcut {}", lnk.display()));
        }
        println!("installed {}", app.name);
    }
    println!("\nThe apps are in the Start menu's Neo folder.");
    Ok(())
}

#[cfg(windows)]
pub fn uninstall() -> Result<(), String> {
    let _ = std::fs::remove_dir_all(start_menu());
    let dir = install_dir();
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(io(dir.display()))?;
    }
    println!("removed the Neo apps");
    Ok(())
}
