//! Screen capture: listing windows and running the platform's recorder.
//!
//! Recording is done by a helper program so this example needs no video
//! encoder of its own:
//!
//! - macOS: `screencapture`, which ships with the system.
//! - Linux: `wf-recorder` on wlroots Wayland compositors, `ffmpeg` on X11.
//! - Windows: `ffmpeg`.
//!
//! A Neo session would capture through PipeWire and the desktop portal instead.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use neo::Rect;

/// A window that can be recorded.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowInfo {
    pub id: u64,
    pub app: String,
    pub title: String,
    /// Position on the screen in logical pixels.
    pub frame: Rect,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Screen,
    Window(WindowInfo),
    /// A rectangle on the screen in logical pixels.
    Area(Rect),
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Options {
    /// Draw a ring around mouse clicks, where the recorder supports it.
    pub show_clicks: bool,
    /// Physical pixels per logical pixel, for recorders that work in physical pixels.
    pub scale: f32,
}

/// A recording in progress.
pub struct Recording {
    child: Child,
    pub path: PathBuf,
    pub started: Instant,
}

/// A finished recording.
#[derive(Clone, Debug, PartialEq)]
pub struct Saved {
    pub path: PathBuf,
    pub bytes: u64,
    pub length: Duration,
}

/// The file extension the platform's recorder writes.
pub const EXTENSION: &str = if cfg!(target_os = "macos") { "mov" } else { "mp4" };

fn spawn(mut cmd: Command, what: &str, install: &str) -> Result<Child, String> {
    cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound { format!("Recording here needs {what}. {install}") } else { format!("Could not start {what}: {e}") }
    })
}

/// H.264 needs even dimensions, and recorders want whole pixels.
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn physical(r: Rect, scale: f32) -> (i32, i32, i32, i32) {
    let s = if scale > 0.0 { scale } else { 1.0 };
    let even = |v: f32| ((v * s).round() as i32 / 2 * 2).max(2);
    ((r.x * s).round() as i32, (r.y * s).round() as i32, even(r.w), even(r.h))
}

#[cfg(target_os = "macos")]
fn command(target: &Target, options: Options, path: &Path) -> Result<Child, String> {
    let mut cmd = Command::new("screencapture");
    // -v records video, -x keeps it silent.
    cmd.args(["-v", "-x"]);
    if options.show_clicks {
        cmd.arg("-k");
    }
    let rect = |r: &Rect| format!("-R{},{},{},{}", r.x.round(), r.y.round(), r.w.round(), r.h.round());
    match target {
        Target::Screen => {}
        Target::Window(w) => {
            // Follows the window if it moves; -o leaves out its shadow.
            cmd.arg(format!("-l{}", w.id)).arg("-o");
        }
        Target::Area(r) => {
            cmd.arg(rect(r));
        }
    }
    cmd.arg(path);
    spawn(cmd, "screencapture", "It is part of macOS.")
}

#[cfg(all(unix, not(target_os = "macos")))]
fn command(target: &Target, options: Options, path: &Path) -> Result<Child, String> {
    let rect = match target {
        Target::Screen => None,
        Target::Window(w) => Some(physical(w.frame, options.scale)),
        Target::Area(r) => Some(physical(*r, options.scale)),
    };
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        let mut cmd = Command::new("wf-recorder");
        if let Some((x, y, w, h)) = rect {
            cmd.arg("-g").arg(format!("{x},{y} {w}x{h}"));
        }
        cmd.arg("-f").arg(path);
        return spawn(cmd, "wf-recorder", "Install it with your package manager. It works on wlroots compositors such as Sway and labwc; GNOME and KDE Wayland sessions are not supported yet.");
    }
    let display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0".into());
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-y", "-loglevel", "error", "-f", "x11grab", "-framerate", "30"]);
    let input = match rect {
        Some((x, y, w, h)) => {
            cmd.arg("-video_size").arg(format!("{w}x{h}"));
            format!("{display}+{x},{y}")
        }
        None => display,
    };
    cmd.arg("-i").arg(input).args(["-pix_fmt", "yuv420p"]).arg(path);
    spawn(cmd, "ffmpeg", "Install it with your package manager.")
}

#[cfg(windows)]
fn command(target: &Target, options: Options, path: &Path) -> Result<Child, String> {
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-y", "-loglevel", "error", "-f", "gdigrab", "-framerate", "30"]);
    let rect = match target {
        Target::Screen => None,
        Target::Window(w) => Some(physical(w.frame, options.scale)),
        Target::Area(r) => Some(physical(*r, options.scale)),
    };
    if let Some((x, y, w, h)) = rect {
        cmd.arg("-offset_x").arg(x.to_string()).arg("-offset_y").arg(y.to_string()).arg("-video_size").arg(format!("{w}x{h}"));
    }
    cmd.args(["-i", "desktop", "-pix_fmt", "yuv420p"]).arg(path);
    spawn(cmd, "ffmpeg", "Install it, for example with `winget install ffmpeg`, and make sure it is on your PATH.")
}

/// Starts recording `target` into `path`.
pub fn start(target: &Target, options: Options, path: &Path) -> Result<Recording, String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    }
    let mut child = command(target, options, path)?;
    // A recorder that cannot start, for lack of permission say, exits at once.
    std::thread::sleep(Duration::from_millis(250));
    if let Ok(Some(status)) = child.try_wait() {
        return Err(format!("The recorder stopped as soon as it started ({status}). {}", permission_hint()));
    }
    Ok(Recording { child, path: path.to_path_buf(), started: Instant::now() })
}

fn permission_hint() -> &'static str {
    if cfg!(target_os = "macos") { "Allow this app under System Settings › Privacy & Security › Screen & System Audio Recording, then try again." } else { "Check that screen recording is allowed for this session." }
}

impl Recording {
    /// Asks the recorder to finish and waits for the file. This can take a
    /// second or two, so call it off the main thread.
    pub fn stop(mut self) -> Result<Saved, String> {
        let length = self.started.elapsed();
        #[cfg(unix)]
        // SAFETY: the child has not been waited for, so its ID still names it.
        unsafe {
            libc::kill(self.child.id() as libc::pid_t, libc::SIGINT);
        }
        #[cfg(not(unix))]
        {
            // ffmpeg finishes the file when it reads `q`.
            use std::io::Write;
            if let Some(stdin) = self.child.stdin.as_mut() {
                let _ = stdin.write_all(b"q\n");
            }
        }
        drop(self.child.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
                _ => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    break;
                }
            }
        }
        match std::fs::metadata(&self.path) {
            Ok(m) if m.len() > 0 => Ok(Saved { path: self.path.clone(), bytes: m.len(), length }),
            _ => Err(format!("The recorder did not save a file. {}", permission_hint())),
        }
    }
}

/// Whether this app may record the screen. Always true where the system has
/// no such permission.
pub fn permitted() -> bool {
    #[cfg(target_os = "macos")]
    // SAFETY: a CoreGraphics query with no arguments.
    unsafe {
        mac::CGPreflightScreenCaptureAccess()
    }
    #[cfg(not(target_os = "macos"))]
    true
}

/// Asks the system to show its permission prompt, where it has one.
pub fn request_permission() {
    #[cfg(target_os = "macos")]
    // SAFETY: a CoreGraphics call with no arguments.
    unsafe {
        mac::CGRequestScreenCaptureAccess();
    }
}

/// Windows on screen, front to back, without this app's own.
pub fn windows() -> Vec<WindowInfo> {
    imp_windows()
}

#[cfg(target_os = "macos")]
mod mac {
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        pub fn CGPreflightScreenCaptureAccess() -> bool;
        pub fn CGRequestScreenCaptureAccess() -> bool;
    }
}

#[cfg(target_os = "macos")]
fn imp_windows() -> Vec<WindowInfo> {
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
    use core_foundation::number::CFNumber;
    use core_foundation::string::{CFString, CFStringRef};
    use core_graphics::geometry::CGRect;
    use core_graphics::window::{copy_window_info, kCGNullWindowID, kCGWindowBounds, kCGWindowLayer, kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly, kCGWindowName, kCGWindowNumber, kCGWindowOwnerName, kCGWindowOwnerPID};

    let Some(list) = copy_window_info(kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements, kCGNullWindowID) else { return vec![] };
    let me = std::process::id() as i64;
    let mut out = Vec::new();
    for item in list.iter() {
        // SAFETY: CGWindowListCopyWindowInfo returns an array of dictionaries
        // keyed by the string constants used below.
        let dict: CFDictionary<CFString, CFType> = unsafe { CFDictionary::wrap_under_get_rule(*item as CFDictionaryRef) };
        let get = |key: CFStringRef| dict.find(unsafe { CFString::wrap_under_get_rule(key) }).map(|v| v.clone());
        let number = |key: CFStringRef| get(key).and_then(|v| v.downcast::<CFNumber>()).and_then(|n| n.to_i64());
        let string = |key: CFStringRef| get(key).and_then(|v| v.downcast::<CFString>()).map(|s| s.to_string()).unwrap_or_default();
        // SAFETY: the statics are valid for the life of the process.
        let (layer, pid, id) = unsafe { (number(kCGWindowLayer), number(kCGWindowOwnerPID), number(kCGWindowNumber)) };
        // Layer 0 holds ordinary app windows; menus, the Dock and overlays sit above it.
        if layer != Some(0) || pid == Some(me) {
            continue;
        }
        let Some(bounds) = get(unsafe { kCGWindowBounds }).and_then(|v| v.downcast::<CFDictionary>()).and_then(|d| CGRect::from_dict_representation(&d)) else { continue };
        let frame = Rect::new(bounds.origin.x as f32, bounds.origin.y as f32, bounds.size.width as f32, bounds.size.height as f32);
        // Skip slivers such as status items and hidden helper windows.
        if frame.w < 80.0 || frame.h < 60.0 {
            continue;
        }
        let (app, title) = unsafe { (string(kCGWindowOwnerName), string(kCGWindowName)) };
        out.push(WindowInfo { id: id.unwrap_or(0) as u64, app, title, frame });
    }
    out
}

#[cfg(windows)]
fn imp_windows() -> Vec<WindowInfo> {
    use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowRect, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible};

    unsafe extern "system" fn each(hwnd: HWND, out: LPARAM) -> BOOL {
        // SAFETY: `out` is the Vec passed to EnumWindows below, alive for the call.
        let out = unsafe { &mut *(out as *mut Vec<WindowInfo>) };
        unsafe {
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, &mut pid);
            let len = GetWindowTextLengthW(hwnd);
            if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 || len == 0 || pid == std::process::id() {
                return 1;
            }
            let mut buf = vec![0u16; len as usize + 1];
            let n = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32).max(0) as usize;
            let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            if GetWindowRect(hwnd, &mut r) == 0 || r.right - r.left < 80 || r.bottom - r.top < 60 {
                return 1;
            }
            // Window rectangles are physical pixels; `physical` is told a scale of 1 for them.
            out.push(WindowInfo { id: hwnd as usize as u64, app: String::new(), title: String::from_utf16_lossy(&buf[..n]), frame: Rect::new(r.left as f32, r.top as f32, (r.right - r.left) as f32, (r.bottom - r.top) as f32) });
        }
        1
    }
    let mut out: Vec<WindowInfo> = Vec::new();
    // SAFETY: the callback only runs during this call, while `out` is alive.
    unsafe {
        EnumWindows(Some(each), &mut out as *mut _ as LPARAM);
    }
    out
}

/// X11 sessions list windows through `wmctrl` when it is installed. Wayland
/// does not let apps see each other's windows.
#[cfg(all(unix, not(target_os = "macos")))]
fn imp_windows() -> Vec<WindowInfo> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        return vec![];
    }
    let Ok(out) = Command::new("wmctrl").args(["-l", "-G", "-p"]).output() else { return vec![] };
    let me = std::process::id().to_string();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            // id desktop pid x y w h host title...
            let mut it = line.split_whitespace();
            let id = u64::from_str_radix(it.next()?.trim_start_matches("0x"), 16).ok()?;
            let desktop = it.next()?;
            let pid = it.next()?;
            let mut num = || it.next()?.parse::<f32>().ok();
            let frame = Rect::new(num()?, num()?, num()?, num()?);
            let _host = it.next()?;
            let title = it.collect::<Vec<_>>().join(" ");
            (desktop != "-1" && pid != me && frame.w >= 80.0 && frame.h >= 60.0).then_some(WindowInfo { id, app: String::new(), title, frame })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_rects_are_even_and_scaled() {
        assert_eq!(physical(Rect::new(10.4, 20.0, 301.0, 199.0), 2.0), (21, 40, 602, 398));
        assert_eq!(physical(Rect::new(0.0, 0.0, 301.0, 199.0), 1.0), (0, 0, 300, 198));
    }
}
