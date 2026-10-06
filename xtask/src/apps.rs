//! The apps that get installed, and how their icons look.

use neo_theme::icons;
use neo_theme::{Color, Icon};

pub struct AppInfo {
    /// Cargo package and binary name.
    pub bin: &'static str,
    /// Reverse-DNS ID: the Wayland app ID, `.desktop` file name and macOS bundle ID.
    pub id: &'static str,
    /// The app's name, everywhere it is shown.
    pub name: &'static str,
    /// What it was called on macOS and Windows before, so an install can
    /// clear away the copy under the old name.
    #[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
    pub former: &'static str,
    pub glyph: Icon,
    /// Icon background, top and bottom of the gradient.
    pub top: Color,
    pub bottom: Color,
    pub ink: Color,
    /// Lives in the background, behind a shortcut or a tray icon, so it has
    /// no Dock icon on macOS.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub background: bool,
}

pub const APPS: &[AppInfo] = &[
    AppInfo { bin: "neo-files", id: "org.neo.Files", name: "Files", former: "Neo Files", glyph: icons::FOLDER, top: Color::hex(0x6E88EC), bottom: Color::hex(0x3F5BC4), ink: Color::WHITE, background: false },
    AppInfo { bin: "neo-terminal", id: "org.neo.Terminal", name: "NeoTerm", former: "Neo Terminal", glyph: icons::SQUARE_TERMINAL, top: Color::hex(0x3A383B), bottom: Color::hex(0x161516), ink: Color::hex(0xA9DC76), background: false },
    AppInfo { bin: "neo-settings", id: "org.neo.Settings", name: "Settings", former: "Neo Settings", glyph: icons::SETTINGS, top: Color::hex(0x9AA3B2), bottom: Color::hex(0x5C6576), ink: Color::WHITE, background: false },
    AppInfo { bin: "neo-monitor", id: "org.neo.Monitor", name: "System Monitor", former: "Neo System Monitor", glyph: icons::ACTIVITY, top: Color::hex(0x35B3A2), bottom: Color::hex(0x16706A), ink: Color::WHITE, background: false },
    AppInfo { bin: "neo-calculator", id: "org.neo.Calculator", name: "NeoCal", former: "Neo Calculator", glyph: icons::CALCULATOR, top: Color::hex(0xF79A6C), bottom: Color::hex(0xC0533B), ink: Color::WHITE, background: false },
    AppInfo { bin: "neo-photos", id: "org.neo.Photos", name: "Photos", former: "Neo Photos", glyph: icons::IMAGE, top: Color::hex(0xFFD866), bottom: Color::hex(0xE08A2E), ink: Color::WHITE, background: false },
    AppInfo { bin: "neo-videos", id: "org.neo.Videos", name: "Videos", former: "Neo Videos", glyph: icons::CLAPPERBOARD, top: Color::hex(0xAB9DF2), bottom: Color::hex(0x6A55C9), ink: Color::WHITE, background: false },
    AppInfo { bin: "neo-recorder", id: "org.neo.Recorder", name: "NeoCap", former: "Neo Recorder", glyph: icons::VIDEO, top: Color::hex(0xFF7A9C), bottom: Color::hex(0xC0443A), ink: Color::WHITE, background: true },
    AppInfo { bin: "neo-launcher", id: "org.neo.Launcher", name: "Launcher", former: "Neo Launcher", glyph: icons::SEARCH, top: Color::hex(0x5B78E6), bottom: Color::hex(0x2B3A8C), ink: Color::WHITE, background: true },
    AppInfo { bin: "neo-shell", id: "org.neo.Shell", name: "NeoShell", former: "NeoShell", glyph: icons::PANEL_TOP, top: Color::hex(0x4A5568), bottom: Color::hex(0x1F2733), ink: Color::WHITE, background: true },
    AppInfo { bin: "neo-disk", id: "org.neo.Disk", name: "NeoDisk", former: "NeoDisk", glyph: icons::CHART_PIE, top: Color::hex(0x7BC96F), bottom: Color::hex(0x2E7D4F), ink: Color::WHITE, background: false },
    AppInfo { bin: "neo-code", id: "org.neo.Code", name: "NeoCode", former: "Neo Code", glyph: icons::CODE, top: Color::hex(0x2F3542), bottom: Color::hex(0x14171C), ink: Color::hex(0x889FEC), background: false },
];
