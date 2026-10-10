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
    /// The process it belongs to, where the system says; 0 where it does not.
    pub pid: u32,
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

/// A drag-to-select screenshot in progress: the system's own crosshair is
/// up, waiting for the user to drag out an area.
pub struct Picker {
    child: std::sync::Arc<std::sync::Mutex<Child>>,
}

impl Picker {
    /// Takes the crosshair down without a picture being taken.
    pub fn cancel(&self) {
        let _ = self.child.lock().unwrap_or_else(|e| e.into_inner()).kill();
    }
}

/// The command that lets the user drag out an area and saves a picture of it.
#[cfg(target_os = "macos")]
fn pick_command(path: &Path) -> Option<Command> {
    // -i is the same crosshair as the system's own shortcut; -x keeps it silent.
    let mut cmd = Command::new("screencapture");
    cmd.args(["-i", "-x"]).arg(path);
    Some(cmd)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn pick_command(path: &Path) -> Option<Command> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        // slurp draws the selection and grim takes the picture of it.
        let mut cmd = Command::new("sh");
        // The screen dimmed, and the area chosen left clear.
        cmd.args(["-c", "area=$(slurp -b 0000006b -s 00000000 -c ffffffe6) && grim -g \"$area\" \"$0\""]).arg(path);
        return Some(cmd);
    }
    let mut cmd = Command::new("maim");
    cmd.args(["-s", "-u"]).arg(path);
    Some(cmd)
}

#[cfg(windows)]
fn pick_command(_path: &Path) -> Option<Command> {
    None
}

/// Whether the system has a drag-to-select of its own to hand over to.
pub fn can_pick() -> bool {
    pick_command(Path::new("probe.png")).is_some()
}

/// Puts up the system's crosshair for the user to drag out an area, and
/// saves a picture of it at `path`. `done` is called from another thread
/// when that ends: with the picture, with an error, or with `None` if the
/// user backed out or [`Picker::cancel`] was called.
pub fn pick(path: &Path, done: impl FnOnce(Option<Result<Saved, String>>) + Send + 'static) -> Result<Picker, String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    }
    let mut cmd = pick_command(path).ok_or("Selecting an area with the mouse is not available here.")?;
    let tool = cmd.get_program().to_string_lossy().into_owned();
    let child = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(|e| format!("Selecting an area needs {tool}: {e}"))?;
    let child = std::sync::Arc::new(std::sync::Mutex::new(child));
    let (watched, path) = (child.clone(), path.to_path_buf());
    std::thread::spawn(move || {
        // Polled rather than waited on, so the lock is free for a cancel.
        loop {
            match watched.lock().unwrap_or_else(|e| e.into_inner()).try_wait() {
                Ok(None) => {}
                _ => break,
            }
            std::thread::sleep(Duration::from_millis(40));
        }
        // No file means no picture was taken: Escape, or a cancel.
        done(match std::fs::metadata(&path) {
            Ok(m) if m.len() > 0 => Some(Ok(Saved { path, bytes: m.len(), length: Duration::ZERO })),
            _ => None,
        });
    });
    Ok(Picker { child })
}

/// The screen as it stood at one moment: a picture of the whole of it,
/// kept in a file, with its pixels to show in its place while an area of
/// it is chosen.
#[derive(Clone, Debug, PartialEq)]
pub struct Still {
    pub file: PathBuf,
    pub width: u32,
    pub height: u32,
    /// Straight-alpha RGBA, top row first.
    pub rgba: Vec<u8>,
}

/// Takes a picture of the whole screen now, into `file`, to choose an
/// area of at leisure: whatever moves on the screen after this is not in
/// it. Blocks for a moment, so call it off the main thread.
pub fn still(file: &Path) -> Result<Still, String> {
    screenshot(&Target::Screen, Options::default(), file)?;
    let (width, height, rgba) = read_png(file).inspect_err(|_| {
        let _ = std::fs::remove_file(file);
    })?;
    Ok(Still { file: file.to_path_buf(), width: width as u32, height: height as u32, rgba })
}

/// Saves the part of a still at `x`, `y` that is `w` by `h`, all in the
/// still's own pixels, as a PNG at `path`. On macOS the system cuts it
/// out of the file, which keeps the screen's colours as they were taken.
pub fn save_part(still: &Still, (x, y, w, h): (u32, u32, u32, u32), path: &Path) -> Result<Saved, String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    }
    let (x, y) = (x.min(still.width.saturating_sub(1)), y.min(still.height.saturating_sub(1)));
    let (w, h) = (w.clamp(1, still.width - x), h.clamp(1, still.height - y));
    let cut = cfg!(target_os = "macos") && Command::new("/usr/bin/sips").args(["-c", &h.to_string(), &w.to_string(), "--cropOffset", &y.to_string(), &x.to_string()]).arg(&still.file).arg("--out").arg(path).stdin(Stdio::null()).output().is_ok_and(|o| o.status.success());
    if !cut || std::fs::metadata(path).map_or(true, |m| m.len() == 0) {
        // By hand, from the pixels.
        let mut part = Vec::with_capacity((w * h * 4) as usize);
        for row in y..y + h {
            let from = ((row * still.width + x) * 4) as usize;
            part.extend_from_slice(&still.rgba[from..from + (w * 4) as usize]);
        }
        let wrong = |e: &dyn std::fmt::Display| format!("Could not save the screenshot: {e}");
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(path).map_err(|e| wrong(&e))?), w, h);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header().and_then(|mut writer| writer.write_image_data(&part)).map_err(|e| wrong(&e))?;
    }
    match std::fs::metadata(path) {
        Ok(m) if m.len() > 0 => Ok(Saved { path: path.to_path_buf(), bytes: m.len(), length: Duration::ZERO }),
        _ => Err("No screenshot was saved.".into()),
    }
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
        out.push(WindowInfo { id: id.unwrap_or(0) as u64, pid: pid.unwrap_or(0) as u32, app, title, frame });
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
            out.push(WindowInfo { id: hwnd as usize as u64, pid, app: String::new(), title: String::from_utf16_lossy(&buf[..n]), frame: Rect::new(r.left as f32, r.top as f32, (r.right - r.left) as f32, (r.bottom - r.top) as f32) });
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
            (desktop != "-1" && pid != me && frame.w >= 80.0 && frame.h >= 60.0).then_some(WindowInfo { id, pid: pid.parse().unwrap_or(0), app: String::new(), title, frame })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    /// Puts the real crosshair up for a second and takes it down again,
    /// which no picture should come of. It shows on screen, so it runs only
    /// when asked for: `cargo test -p neo-recorder -- --ignored crosshair`.
    #[test]
    #[ignore]
    fn the_crosshair_comes_up_and_can_be_taken_down() {
        let path = std::env::temp_dir().join(format!("neo-pick-{}.png", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let (tx, rx) = std::sync::mpsc::channel();
        let picker = super::pick(&path, move |r| {
            let _ = tx.send(r);
        })
        .expect("the system's drag-to-select starts");
        assert!(rx.recv_timeout(std::time::Duration::from_millis(900)).is_err(), "it waits for the user");
        picker.cancel();
        let ended = rx.recv_timeout(std::time::Duration::from_secs(3)).expect("it ends when cancelled");
        assert!(ended.is_none(), "and no picture was taken");
        assert!(!path.exists());
    }

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

    #[test]
    fn a_part_of_a_still_is_saved_as_it_stood() {
        let dir = std::env::temp_dir().join(format!("neo-cap-still-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A picture four by three, each pixel saying where it is.
        let (w, h) = (4u32, 3u32);
        let rgba: Vec<u8> = (0..h).flat_map(|y| (0..w).flat_map(move |x| [x as u8 * 60, y as u8 * 80, 7, 255])).collect();
        let file = dir.join("still.png");
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(&file).unwrap()), w, h);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header().unwrap().write_image_data(&rgba).unwrap();
        assert_eq!(read_png(&file), Ok((w as usize, h as usize, rgba.clone())));
        let still = Still { file: file.clone(), width: w, height: h, rgba };
        // The two by two at its bottom right.
        let saved = save_part(&still, (2, 1, 2, 2), &dir.join("part.png")).unwrap();
        assert_eq!(read_png(&saved.path), Ok((2, 2, vec![120, 80, 7, 255, 180, 80, 7, 255, 120, 160, 7, 255, 180, 160, 7, 255])));
        // Asked for more than there is, it gives what there is.
        let edge = save_part(&still, (3, 2, 50, 50), &dir.join("edge.png")).unwrap();
        assert_eq!(read_png(&edge.path).map(|(w, h, _)| (w, h)), Ok((1, 1)));
        assert!(read_png(&dir.join("not there.png")).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
