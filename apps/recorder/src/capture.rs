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
    /// Record sound from the default microphone.
    pub microphone: bool,
    /// Physical pixels per logical pixel, for recorders that work in physical pixels.
    pub scale: f32,
}

/// A recording in progress. Pausing ends the current part and resuming
/// starts another; `finish` joins them.
pub struct Session {
    target: Target,
    options: Options,
    /// Where the finished movie goes. A GIF takes the same name with `.gif`.
    final_path: PathBuf,
    parts: Vec<PathBuf>,
    /// The recorder running now. `None` while paused.
    child: Option<Child>,
    /// Recorders told to finish whose files may still be closing.
    stopping: Vec<Child>,
    recorded: Duration,
    since: Option<Instant>,
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
    if options.microphone {
        // -g records the default audio input.
        cmd.arg("-g");
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
        if options.microphone {
            cmd.arg("-a");
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
    cmd.arg("-i").arg(input);
    if options.microphone {
        cmd.args(["-f", "pulse", "-i", "default", "-c:a", "aac"]);
    }
    cmd.args(["-pix_fmt", "yuv420p"]).arg(path);
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

/// Takes one picture of `target` and saves it as a PNG at `path`. Blocks
/// for a moment, so call it off the main thread.
pub fn screenshot(target: &Target, options: Options, path: &Path) -> Result<Saved, String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    }
    let mut cmd = screenshot_command(target, options, path);
    let tool = cmd.get_program().to_string_lossy().into_owned();
    let out = cmd.stdin(Stdio::null()).output().map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { format!("Taking a screenshot here needs {tool}, which is not installed.") } else { format!("Could not run {tool}: {e}") })?;
    match std::fs::metadata(path) {
        Ok(m) if m.len() > 0 => Ok(Saved { path: path.to_path_buf(), bytes: m.len(), length: Duration::ZERO }),
        _ => Err(format!("No screenshot was saved ({}). {}", out.status, permission_hint())),
    }
}

#[cfg(target_os = "macos")]
fn screenshot_command(target: &Target, _options: Options, path: &Path) -> Command {
    let mut cmd = Command::new("screencapture");
    // -x keeps it silent.
    cmd.arg("-x");
    let rect = |r: &Rect| format!("-R{},{},{},{}", r.x.round(), r.y.round(), r.w.round(), r.h.round());
    match target {
        Target::Screen => {}
        Target::Window(w) => {
            // By ID, so the window is captured even if something overlaps it; -o leaves out its shadow.
            cmd.arg(format!("-l{}", w.id)).arg("-o");
        }
        Target::Area(r) => {
            cmd.arg(rect(r));
        }
    }
    cmd.arg(path);
    cmd
}

#[cfg(all(unix, not(target_os = "macos")))]
fn screenshot_command(target: &Target, options: Options, path: &Path) -> Command {
    let rect = match target {
        Target::Screen => None,
        Target::Window(w) => Some(physical(w.frame, options.scale)),
        Target::Area(r) => Some(physical(*r, options.scale)),
    };
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        // grim is the usual screenshot tool on wlroots compositors.
        let mut cmd = Command::new("grim");
        if let Some((x, y, w, h)) = rect {
            cmd.arg("-g").arg(format!("{x},{y} {w}x{h}"));
        }
        cmd.arg(path);
        return cmd;
    }
    let display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0".into());
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-y", "-loglevel", "error", "-f", "x11grab"]);
    let input = match rect {
        Some((x, y, w, h)) => {
            cmd.arg("-video_size").arg(format!("{w}x{h}"));
            format!("{display}+{x},{y}")
        }
        None => display,
    };
    cmd.arg("-i").arg(input).args(["-frames:v", "1"]).arg(path);
    cmd
}

#[cfg(windows)]
fn screenshot_command(target: &Target, options: Options, path: &Path) -> Command {
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-y", "-loglevel", "error", "-f", "gdigrab"]);
    let rect = match target {
        Target::Screen => None,
        Target::Window(w) => Some(physical(w.frame, options.scale)),
        Target::Area(r) => Some(physical(*r, options.scale)),
    };
    if let Some((x, y, w, h)) = rect {
        cmd.arg("-offset_x").arg(x.to_string()).arg("-offset_y").arg(y.to_string()).arg("-video_size").arg(format!("{w}x{h}"));
    }
    cmd.args(["-i", "desktop", "-frames:v", "1"]).arg(path);
    cmd
}

/// Reads a PNG file as straight-alpha RGBA pixels.
pub fn read_png(path: &Path) -> Result<(usize, usize, Vec<u8>), String> {
    let mut decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).map_err(|e| e.to_string())?));
    // Whatever the file holds, come out as 8-bit with an alpha channel.
    decoder.set_transformations(png::Transformations::normalize_to_color8() | png::Transformations::ALPHA);
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut pixels = vec![0; reader.output_buffer_size().ok_or("the picture is too large")?];
    let info = reader.next_frame(&mut pixels).map_err(|e| e.to_string())?;
    pixels.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => pixels,
        // Greyscale screens are rare, but a grey picture is still a picture.
        png::ColorType::GrayscaleAlpha => pixels.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        other => return Err(format!("unexpected colour type {other:?}")),
    };
    Ok((info.width as usize, info.height as usize, rgba))
}

/// Whether this platform's recorder can record the microphone.
pub const MICROPHONE: bool = cfg!(not(windows));

fn permission_hint() -> &'static str {
    if cfg!(target_os = "macos") { "Allow this app under System Settings › Privacy & Security › Screen & System Audio Recording, then try again." } else { "Check that screen recording is allowed for this session." }
}

/// Asks a recorder to finish its file.
fn interrupt(child: &mut Child) {
    #[cfg(unix)]
    // SAFETY: the child has not been waited for, so its ID still names it.
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGINT);
    }
    #[cfg(not(unix))]
    {
        // ffmpeg finishes the file when it reads `q`.
        use std::io::Write;
        if let Some(stdin) = child.stdin.as_mut() {
            let _ = stdin.write_all(b"q\n");
        }
    }
    drop(child.stdin.take());
}

/// Waits for a recorder to exit, ending it by force if it takes too long.
fn wait(mut child: Child) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(40)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
        }
    }
}

impl Session {
    /// Starts recording `target`. The result is saved at `final_path`, or
    /// beside it as a GIF.
    pub fn start(target: Target, options: Options, final_path: PathBuf) -> Result<Self, String> {
        if let Some(dir) = final_path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
        }
        let mut session = Self { target, options, final_path, parts: vec![], child: None, stopping: vec![], recorded: Duration::ZERO, since: None };
        session.begin_part()?;
        Ok(session)
    }

    /// A hidden file beside the final one, so joining never crosses disks.
    fn part_path(&self, label: &str) -> PathBuf {
        let stem = self.final_path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        self.final_path.with_file_name(format!(".{stem} {label}.{EXTENSION}"))
    }

    fn begin_part(&mut self) -> Result<(), String> {
        // Two recorders must not run at once.
        for child in self.stopping.drain(..) {
            wait(child);
        }
        let path = self.part_path(&format!("part {}", self.parts.len() + 1));
        let mut child = command(&self.target, self.options, &path)?;
        // A recorder that cannot start, for lack of permission say, exits at once.
        std::thread::sleep(Duration::from_millis(250));
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("The recorder stopped as soon as it started ({status}). {}", permission_hint()));
        }
        self.parts.push(path);
        self.child = Some(child);
        self.since = Some(Instant::now());
        Ok(())
    }

    pub fn paused(&self) -> bool {
        self.child.is_none()
    }

    /// How long has been recorded, not counting pauses.
    pub fn elapsed(&self) -> Duration {
        self.recorded + self.since.map_or(Duration::ZERO, |s| s.elapsed())
    }

    /// Stops recording until [`resume`](Self::resume). Returns at once; the
    /// part's file finishes closing in the background.
    pub fn pause(&mut self) {
        if let Some(mut child) = self.child.take() {
            self.recorded += self.since.take().map_or(Duration::ZERO, |s| s.elapsed());
            interrupt(&mut child);
            self.stopping.push(child);
        }
    }

    pub fn resume(&mut self) -> Result<(), String> {
        if self.paused() { self.begin_part() } else { Ok(()) }
    }

    /// Ends the recording and writes the result, joining the parts and
    /// converting to a GIF if asked. This takes a while, so call it off the
    /// main thread.
    pub fn finish(mut self, as_gif: bool) -> Result<Saved, String> {
        self.pause();
        for child in self.stopping.drain(..) {
            wait(child);
        }
        let parts: Vec<PathBuf> = self.parts.iter().filter(|p| std::fs::metadata(p).is_ok_and(|m| m.len() > 0)).cloned().collect();
        let cleanup = |paths: &[PathBuf]| {
            for p in paths {
                let _ = std::fs::remove_file(p);
            }
        };
        if parts.is_empty() {
            cleanup(&self.parts);
            return Err(format!("The recorder did not save a file. {}", permission_hint()));
        }
        // One movie, whether or not the recording was paused.
        let movie = if parts.len() == 1 {
            parts[0].clone()
        } else {
            let joined = self.part_path("joined");
            if let Err(e) = crate::media::concat(&parts, &joined) {
                return Err(format!("Could not join the {} recorded parts: {e}. They are kept as hidden files in {}.", parts.len(), joined.parent().map(|d| d.display().to_string()).unwrap_or_default()));
            }
            cleanup(&self.parts);
            joined
        };
        let path = if as_gif {
            let gif = self.final_path.with_extension("gif");
            if let Err(e) = crate::media::gif(&movie, &gif) {
                // Keep the movie rather than lose the recording.
                let _ = std::fs::rename(&movie, &self.final_path);
                return Err(format!("Could not make the GIF: {e}. The recording was saved as {} instead.", self.final_path.display()));
            }
            let _ = std::fs::remove_file(&movie);
            gif
        } else {
            std::fs::rename(&movie, &self.final_path).map_err(|e| format!("Could not save {}: {e}", self.final_path.display()))?;
            self.final_path.clone()
        };
        let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        Ok(Saved { path, bytes, length: self.recorded })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Never leave a recorder running after the session is gone.
        if let Some(mut child) = self.child.take() {
            interrupt(&mut child);
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

    /// Counts the frames of a GIF.
    #[cfg(target_os = "macos")]
    fn gif_frames(path: &Path) -> usize {
        let mut decoder = gif::DecodeOptions::new().read_info(std::fs::File::open(path).unwrap()).unwrap();
        let mut n = 0;
        while decoder.next_frame_info().unwrap().is_some() {
            n += 1;
        }
        n
    }

    /// Records the real screen, so it only runs when asked:
    /// `cargo test -p neo-recorder -- --ignored`.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "records the screen"]
    fn pauses_joins_and_makes_a_gif() {
        let dir = std::env::temp_dir().join(format!("neo-recorder-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let target = Target::Area(Rect::new(100.0, 100.0, 400.0, 300.0));
        let options = Options { scale: 2.0, ..Default::default() };
        let sleep = |ms| std::thread::sleep(Duration::from_millis(ms));

        // Two parts of about 1.5 s each, with a pause between that must not count.
        let mut s = Session::start(target.clone(), options, dir.join("paused.mov")).unwrap();
        sleep(1500);
        s.pause();
        assert!(s.paused());
        let at_pause = s.elapsed();
        sleep(1200);
        assert_eq!(s.elapsed(), at_pause, "time stands still while paused");
        s.resume().unwrap();
        sleep(1500);
        let saved = s.finish(false).unwrap();
        assert_eq!(saved.path, dir.join("paused.mov"));
        assert!(saved.bytes > 0);
        assert!((2.8..3.8).contains(&saved.length.as_secs_f32()), "recorded {:?}", saved.length);
        // The joined movie holds both parts: about 3 s at 10 frames a second.
        let probe = dir.join("probe.gif");
        crate::media::gif(&saved.path, &probe).unwrap();
        let frames = gif_frames(&probe);
        assert!((24..=40).contains(&frames), "joined movie has {frames} GIF frames");
        std::fs::remove_file(&probe).unwrap();

        // Straight to a GIF, with no movie left behind.
        let s = Session::start(target, options, dir.join("clip.mov")).unwrap();
        sleep(1500);
        let saved = s.finish(true).unwrap();
        assert_eq!(saved.path, dir.join("clip.gif"));
        assert!(std::fs::read(&saved.path).unwrap().starts_with(b"GIF89a"));
        assert!((10..=22).contains(&gif_frames(&saved.path)));
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!({ let mut l = left.clone(); l.sort(); l }, ["clip.gif", "paused.mov"], "no parts are left behind");
        println!("paused.mov: {} bytes, {:?}; clip.gif: {} bytes", std::fs::metadata(dir.join("paused.mov")).unwrap().len(), saved.length, saved.bytes);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reads_pngs_with_and_without_alpha() {
        let dir = std::env::temp_dir().join(format!("neo-recorder-png-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, color: png::ColorType, data: &[u8]| {
            let path = dir.join(name);
            let mut encoder = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(&path).unwrap()), 2, 1);
            encoder.set_color(color);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.write_header().unwrap().write_image_data(data).unwrap();
            path
        };
        // A screenshot with no alpha channel still comes out as opaque RGBA.
        let rgb = write("rgb.png", png::ColorType::Rgb, &[255, 0, 0, 0, 0, 255]);
        assert_eq!(read_png(&rgb).unwrap(), (2, 1, vec![255, 0, 0, 255, 0, 0, 255, 255]));
        let rgba = write("rgba.png", png::ColorType::Rgba, &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(read_png(&rgba).unwrap(), (2, 1, vec![1, 2, 3, 4, 5, 6, 7, 8]));
        let grey = write("grey.png", png::ColorType::Grayscale, &[10, 200]);
        assert_eq!(read_png(&grey).unwrap(), (2, 1, vec![10, 10, 10, 255, 200, 200, 200, 255]));
        assert!(read_png(&dir.join("missing.png")).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn physical_rects_are_even_and_scaled() {
        assert_eq!(physical(Rect::new(10.4, 20.0, 301.0, 199.0), 2.0), (21, 40, 602, 398));
        assert_eq!(physical(Rect::new(0.0, 0.0, 301.0, 199.0), 1.0), (0, 0, 300, 198));
    }
}
