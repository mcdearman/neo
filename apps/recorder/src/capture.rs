//! Screen capture: listing windows and running the platform's recorder.
//!
//! Recording is done by a helper program so this example needs no video
//! encoder of its own:
//!
//! - macOS: `screencapture`, which ships with the system.
//! - Linux: `wf-recorder` on wlroots Wayland compositors, `ffmpeg` on X11.
//! - Windows: `ffmpeg`, taking the screen from the graphics card as it
//!   has it (Desktop Duplication) and encoding it there too, with
//!   whichever maker's encoder the card has; see [`Encoder`].
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
    /// Record what the computer is playing.
    pub system_sound: bool,
    /// Physical pixels per logical pixel, for recorders that work in physical pixels.
    pub scale: f32,
    /// A point on the display to record, in physical pixels, where there
    /// is more than one and the whole of one is wanted.
    pub on: Option<(i32, i32)>,
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
    /// The sounds being handed to it, where NeoCap takes them itself.
    sound: Vec<crate::sound::Feed>,
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

/// What a recorder is started with: the program running, and the sounds
/// NeoCap is taking itself to hand to it, kept for as long as it runs.
type Started = (Child, Vec<crate::sound::Feed>);

/// Has ffmpeg take the sounds asked for as its next inputs, after the
/// picture, and returns how many there are and whatever is to be kept
/// while it runs. On Windows NeoCap takes them itself and hands them
/// over; elsewhere ffmpeg takes them from the system.
#[cfg_attr(target_os = "macos", allow(unused_variables, unused_mut))]
fn add_sound(cmd: &mut Command, options: Options) -> (Vec<crate::sound::Source>, Vec<crate::sound::Feed>) {
    use crate::sound::Source;
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut feeds: Vec<crate::sound::Feed> = vec![];
    let mut sounds = vec![];
    #[cfg(windows)]
    {
        use crate::sound::Feed;
        for source in [Source::Computer, Source::Microphone] {
            let wanted = if source == Source::Computer { options.system_sound } else { options.microphone };
            // One that cannot be had is done without; the recording is made all the same.
            if wanted && let Ok(feed) = Feed::open(source) {
                cmd.args(feed.input());
                feeds.push(feed);
                sounds.push(source);
            }
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // PulseAudio, or PipeWire answering as it: what is playing is the
        // "monitor" of where it plays, and the microphone the default source.
        if options.system_sound {
            cmd.args(["-thread_queue_size", "1024", "-f", "pulse", "-i", "@DEFAULT_MONITOR@"]);
            sounds.push(Source::Computer);
        }
        if options.microphone {
            cmd.args(["-thread_queue_size", "1024", "-f", "pulse", "-i", "default"]);
            sounds.push(Source::Microphone);
        }
    }
    #[cfg(target_os = "macos")]
    if options.microphone {
        cmd.args(["-thread_queue_size", "1024", "-f", "avfoundation", "-i", ":default"]);
        sounds.push(Source::Microphone);
    }
    (sounds, feeds)
}

#[cfg(target_os = "macos")]
fn command(target: &Target, options: Options, path: &Path) -> Result<Started, String> {
    let mut cmd = neo_desktop::command("screencapture");
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
    Ok((spawn(cmd, "screencapture", "It is part of macOS.")?, vec![]))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn command(target: &Target, options: Options, path: &Path) -> Result<Started, String> {
    let rect = match target {
        Target::Screen => None,
        Target::Window(w) => Some(physical(w.frame, options.scale)),
        Target::Area(r) => Some(physical(*r, options.scale)),
    };
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        let mut cmd = neo_desktop::command("wf-recorder");
        if let Some((x, y, w, h)) = rect {
            cmd.arg("-g").arg(format!("{x},{y} {w}x{h}"));
        }
        // It takes one sound: what is playing if that is asked for, else the microphone.
        if options.system_sound {
            cmd.arg("--audio=@DEFAULT_MONITOR@");
        } else if options.microphone {
            cmd.arg("-a");
        }
        cmd.arg("-f").arg(path);
        return Ok((spawn(cmd, "wf-recorder", "Install it with your package manager. It works on wlroots compositors such as Sway and labwc; GNOME and KDE Wayland sessions are not supported yet.")?, vec![]));
    }
    let display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0".into());
    let mut cmd = neo_desktop::command("ffmpeg");
    cmd.args(["-y", "-loglevel", "error", "-f", "x11grab", "-framerate", "30"]);
    let input = match rect {
        Some((x, y, w, h)) => {
            cmd.arg("-video_size").arg(format!("{w}x{h}"));
            format!("{display}+{x},{y}")
        }
        None => display,
    };
    cmd.arg("-i").arg(input);
    let (sounds, feeds) = add_sound(&mut cmd, options);
    cmd.args(["-pix_fmt", "yuv420p"]).args(crate::sound::mapping(&sounds)).arg(path);
    Ok((spawn(cmd, "ffmpeg", "Install it with your package manager.")?, feeds))
}

/// What turns the screen into a movie on Windows. The graphics card does
/// it where it can, as the makers' own recorders do: the screen never
/// leaves the card, and the processor is left alone.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoder {
    /// NVIDIA's (NVENC).
    Nvidia,
    /// AMD's (AMF).
    Amd,
    /// Intel's (Quick Sync).
    Intel,
    /// Whatever hardware encoder the system itself offers (Media
    /// Foundation): any card with one, by a slower road, since the
    /// picture is brought off the card to be handed over.
    System,
    /// The processor (x264), with the screen still taken from the card.
    Software,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl Encoder {
    /// In the order they are tried.
    pub const ALL: [Encoder; 5] = [Encoder::Nvidia, Encoder::Amd, Encoder::Intel, Encoder::System, Encoder::Software];

    pub fn name(self) -> &'static str {
        match self {
            Encoder::Nvidia => "the NVIDIA graphics card",
            Encoder::Amd => "the AMD graphics card",
            Encoder::Intel => "Intel graphics",
            Encoder::System => "the graphics card",
            Encoder::Software => "the processor",
        }
    }

    /// What follows the screen grab in ffmpeg's filter, to hand the
    /// picture to this encoder as it wants it.
    fn handed_over(self) -> &'static str {
        match self {
            // These take it as it lies on the card.
            Encoder::Nvidia | Encoder::Amd => "",
            Encoder::Intel => ",hwmap=derive_device=qsv,format=qsv",
            Encoder::System | Encoder::Software => ",hwdownload,format=bgra",
        }
    }

    /// ffmpeg's arguments for it: good enough to read small text in, at
    /// whatever rate that takes, with a whole picture every two seconds.
    fn arguments(self) -> &'static [&'static str] {
        match self {
            Encoder::Nvidia => &["-c:v", "h264_nvenc", "-preset", "p4", "-tune", "hq", "-rc", "vbr", "-cq", "21", "-b:v", "0", "-maxrate", "80M", "-g", "120"],
            Encoder::Amd => &["-c:v", "h264_amf", "-usage", "transcoding", "-quality", "balanced", "-rc", "cqp", "-qp_i", "21", "-qp_p", "23", "-g", "120"],
            Encoder::Intel => &["-c:v", "h264_qsv", "-preset", "medium", "-global_quality", "22", "-g", "120"],
            Encoder::System => &["-c:v", "h264_mf", "-hw_encoding", "true", "-rate_control", "quality", "-quality", "70", "-pix_fmt", "nv12", "-g", "120"],
            Encoder::Software => &["-c:v", "libx264", "-preset", "veryfast", "-crf", "22", "-pix_fmt", "yuv420p", "-g", "120"],
        }
    }
}

/// Frames a second when the graphics card does the work.
#[cfg_attr(not(windows), allow(dead_code))]
const SMOOTH: u32 = 60;

/// Which encoder works on this computer, found by trying each on a few
/// frames of the real screen: every one is built into ffmpeg whatever
/// card is fitted, so there is no telling but by trying. `None` if the
/// screen cannot be taken from the card at all (over a remote desktop,
/// say), and then it is recorded the old way. Tried once and remembered;
/// the first asking takes a second or two, so [`warm`] asks early.
#[cfg(windows)]
static FOUND: std::sync::OnceLock<Option<Encoder>> = std::sync::OnceLock::new();

#[cfg(windows)]
pub fn encoder() -> Option<Encoder> {
    *FOUND.get_or_init(|| {
        Encoder::ALL.into_iter().find(|encoder| {
            let grab = format!("ddagrab=framerate=30{}", encoder.handed_over());
            let mut cmd = neo_desktop::command("ffmpeg");
            cmd.args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", &grab, "-frames:v", "3"]).args(encoder.arguments()).args(["-f", "null", "-"]);
            cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
        })
    })
}

/// Finds out which encoder works, on a thread of its own, so that the
/// first recording does not wait for it.
pub fn warm() {
    #[cfg(windows)]
    std::thread::spawn(|| {
        let _ = encoder();
    });
}

/// How recording is done here, to be said in the window; nothing where
/// there is nothing to choose, or it has not been found out yet (this
/// never waits to find out).
pub fn how_recorded() -> Option<String> {
    #[cfg(windows)]
    {
        return Some(match *FOUND.get()? {
            Some(Encoder::Software) => format!("Recorded by the processor, {SMOOTH} frames a second: no graphics card's encoder was found."),
            Some(encoder) => format!("Recorded by {}, {SMOOTH} frames a second.", encoder.name()),
            None => "Recorded by the processor: the screen could not be taken from the graphics card.".into(),
        });
    }
    #[cfg(not(windows))]
    None
}

/// How many minutes can be kept ready in the background.
pub const REPLAY_MINUTES: [u32; 4] = [1, 5, 10, 20];
/// The background recording is written in pieces this many seconds long,
/// the oldest written over as each new one begins. Short, and each
/// written out as it is made: what has only just happened is what is
/// most wanted, and with pieces of ten seconds kept back until whole, up
/// to the last ten seconds were missing from what was saved.
#[cfg_attr(not(windows), allow(dead_code))]
const PIECE: u32 = 4;

/// Whether the screen can be recorded in the background here, the last
/// minutes of it kept ready to save. Wherever ffmpeg can take the screen
/// itself: Windows, macOS, and Linux under X11. Not yet under Wayland,
/// where the screen is taken by another program that cannot write pieces.
pub fn can_replay() -> bool {
    if cfg!(any(windows, target_os = "macos")) { true } else { std::env::var_os("WAYLAND_DISPLAY").is_none() }
}

/// ffmpeg's arguments for taking one display as its first input, and for
/// encoding the picture, for a recording that goes on in the background.
/// `display` is the display's number as [`displays`] gives it.
#[cfg(windows)]
fn taken_in_the_background(display: u32) -> Result<(Vec<String>, Vec<String>), String> {
    let encoder = encoder().ok_or("Recording in the background needs the screen taken from the graphics card, which is not possible here.")?;
    let grab = format!("ddagrab=output_idx={display}:framerate={SMOOTH}:draw_mouse=1{}", encoder.handed_over());
    let mut encode: Vec<String> = encoder.arguments().iter().map(|a| (*a).to_owned()).collect();
    // Kept within bounds, since it is never looked at unless saved: at
    // most some 225 MB a minute.
    if encoder == Encoder::Nvidia {
        encode.extend(["-cq", "24", "-maxrate", "30M", "-bufsize", "60M"].map(str::to_owned));
    }
    Ok((["-f", "lavfi", "-i", grab.as_str()].map(str::to_owned).to_vec(), encode))
}

/// On macOS the screen is taken by AVFoundation and encoded by the
/// system's own encoder, which is the graphics chip's.
#[cfg(target_os = "macos")]
fn taken_in_the_background(display: u32) -> Result<(Vec<String>, Vec<String>), String> {
    let screen = format!("Capture screen {display}:none");
    Ok((["-f", "avfoundation", "-framerate", "60", "-capture_cursor", "1", "-i", screen.as_str()].map(str::to_owned).to_vec(), ["-c:v", "h264_videotoolbox", "-b:v", "20M", "-pix_fmt", "yuv420p", "-g", "120"].map(str::to_owned).to_vec()))
}

/// Under X11 the whole desktop is taken, and encoded by the processor.
#[cfg(all(unix, not(target_os = "macos")))]
fn taken_in_the_background(_display: u32) -> Result<(Vec<String>, Vec<String>), String> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        return Err("Recording in the background is not there yet under Wayland.".into());
    }
    let display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0".into());
    Ok((["-f", "x11grab", "-framerate", "60", "-i", display.as_str()].map(str::to_owned).to_vec(), ["-c:v", "libx264", "-preset", "veryfast", "-crf", "23", "-pix_fmt", "yuv420p", "-g", "120"].map(str::to_owned).to_vec()))
}

/// The screen being recorded all the time, quietly, with only its last
/// minutes kept: when something worth keeping has just happened, they are
/// saved. Where there is a graphics card's encoder it does the work, so
/// it costs the processor next to nothing. What has been encoded is kept
/// in short pieces in a folder of the system's temporary files, each new
/// piece taking the place of the oldest, and goes when this does. One
/// display, or several, each as a recording of its own.
pub struct Replay {
    each: Vec<Kept>,
    minutes: u32,
    began: Instant,
}

/// One display's background recording.
struct Kept {
    display: u32,
    child: Option<Child>,
    dir: PathBuf,
    /// The sounds it records, in the order of their tracks.
    sounds: Vec<crate::sound::Source>,
    /// The sounds being handed to it, where NeoCap takes them itself.
    _sound: Vec<crate::sound::Feed>,
}

impl Replay {
    /// Begins keeping the last `minutes` of each of `displays`, numbered
    /// as [`displays`] gives them; with none named, of the display that
    /// `on` is a point of, or the one the desktop begins on. With the
    /// sounds `options` asks for.
    pub fn start(minutes: u32, displays: &[u32], on: Option<(i32, i32)>, options: Options) -> Result<Self, String> {
        let displays: Vec<u32> = if displays.is_empty() { vec![placed(&self::displays(), None, on).0] } else { displays.to_vec() };
        let mut each = vec![];
        for display in displays {
            let (input, encode) = taken_in_the_background(display)?;
            let dir = std::env::temp_dir().join(format!("neo-cap-replay-{}-{display}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).map_err(|e| format!("Could not make room for the background recording: {e}"))?;
            let mut cmd = neo_desktop::command("ffmpeg");
            cmd.args(["-y", "-loglevel", "error"]).args(input);
            let (sounds, feeds) = add_sound(&mut cmd, options);
            cmd.args(encode).args(crate::sound::mapping(&sounds));
            // One piece more than is kept, and the one being written.
            let pieces = minutes.max(1) * 60 / PIECE + 2;
            cmd.args(["-f", "segment", "-segment_time", &PIECE.to_string(), "-segment_wrap", &pieces.to_string(), "-segment_format", "mpegts", "-reset_timestamps", "1", "-flush_packets", "1"]).arg(dir.join("piece-%04d.ts"));
            let mut child = spawn(cmd, "ffmpeg", "It is what Neo records with; `cargo xtask deps` installs it.")?;
            std::thread::sleep(Duration::from_millis(250));
            if let Ok(Some(status)) = child.try_wait() {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(format!("The background recording stopped as soon as it started ({status}). {}", permission_hint()));
            }
            each.push(Kept { display, child: Some(child), dir, sounds, _sound: feeds });
        }
        Ok(Self { each, minutes, began: Instant::now() })
    }

    /// Whether it is still going: the recorder ends itself when the
    /// screen is locked or the displays change, and is then begun again.
    pub fn running(&mut self) -> bool {
        self.each.iter_mut().all(|k| k.child.as_mut().is_some_and(|c| matches!(c.try_wait(), Ok(None))))
    }

    pub fn minutes(&self) -> u32 {
        self.minutes
    }

    /// For each display being kept: its number, the pieces that hold its
    /// last minutes, oldest first, the one being written among them; about
    /// how long they are together; and the sounds they hold.
    pub fn pieces(&self) -> Vec<(u32, Vec<PathBuf>, Duration, Vec<crate::sound::Source>)> {
        self.each
            .iter()
            .map(|k| {
                let mut found: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(&k.dir)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .filter_map(|e| {
                        let meta = e.metadata().ok()?;
                        (meta.len() > 0 && e.path().extension().is_some_and(|x| x == "ts")).then_some((meta.modified().ok()?, e.path()))
                    })
                    .collect();
                found.sort();
                // The oldest of a full round is about to be written over.
                let kept = (self.minutes.max(1) * 60 / PIECE + 1) as usize;
                let from = found.len().saturating_sub(kept);
                let pieces: Vec<PathBuf> = found.into_iter().skip(from).map(|(_, p)| p).collect();
                let about = self.began.elapsed().min(Duration::from_secs(pieces.len() as u64 * PIECE as u64));
                (k.display, pieces, about, k.sounds.clone())
            })
            .collect()
    }
}

impl Drop for Kept {
    fn drop(&mut self) {
        // Told to finish, and waited for and tidied up behind the window.
        if let Some(mut child) = self.child.take() {
            interrupt(&mut child);
            let dir = self.dir.clone();
            std::thread::spawn(move || {
                wait(child);
                let _ = std::fs::remove_dir_all(dir);
            });
        }
    }
}
/// Saves what [`Replay::pieces`] gave as one movie at `to`, without
/// encoding anything again. Takes a moment, so call it off the main thread.
pub fn save_replay(pieces: &[PathBuf], about: Duration, sounds: &[crate::sound::Source], to: &Path) -> Result<Saved, String> {
    if pieces.is_empty() {
        return Err("Nothing has been recorded in the background yet.".into());
    }
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    }
    crate::media::join(pieces, &crate::sound::track_names(sounds), to).map_err(|e| format!("Could not save the background recording: {e}"))?;
    match std::fs::metadata(to) {
        Ok(m) if m.len() > 0 => Ok(Saved { path: to.to_path_buf(), bytes: m.len(), length: about.max(Duration::from_secs(1)) }),
        _ => Err("The background recording was not saved.".into()),
    }
}

/// The displays as the graphics card numbers them, each with where it is
/// on the desktop in physical pixels: left, top, width, height.
#[cfg(windows)]
pub fn displays() -> Vec<(u32, (i32, i32, i32, i32))> {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
    let mut found = vec![];
    // SAFETY: plain calls into DXGI, each checked; what they return is let go when dropped.
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else { return found };
        // The first card is the one the desktop is drawn by, and the one ffmpeg takes it from.
        let Ok(adapter) = factory.EnumAdapters1(0) else { return found };
        let mut index = 0;
        while let Ok(output) = adapter.EnumOutputs(index) {
            if let Ok(about) = output.GetDesc() {
                let r = about.DesktopCoordinates;
                found.push((index, (r.left, r.top, r.right - r.left, r.bottom - r.top)));
            }
            index += 1;
        }
    }
    found
}

/// Elsewhere the displays are not told apart yet: there is the one.
#[cfg(not(windows))]
pub fn displays() -> Vec<(u32, (i32, i32, i32, i32))> {
    vec![]
}

/// Which display to record and what part of it: the display's number,
/// and the part as left, top, width, height within it, if not all of it.
/// `rect` is the part wanted on the whole desktop, `on` a point on the
/// display wanted when all of one is; `displays` is what [`displays`] gave.
#[cfg_attr(not(windows), allow(dead_code))]
fn placed(displays: &[(u32, (i32, i32, i32, i32))], rect: Option<(i32, i32, i32, i32)>, on: Option<(i32, i32)>) -> (u32, Option<(i32, i32, i32, i32)>) {
    let holds = |d: &(u32, (i32, i32, i32, i32)), (px, py): (i32, i32)| px >= d.1.0 && py >= d.1.1 && px < d.1.0 + d.1.2 && py < d.1.1 + d.1.3;
    // The display its middle is on; failing that the one the desktop begins on; failing that the first.
    let point = rect.map(|(x, y, w, h)| (x + w / 2, y + h / 2)).or(on).unwrap_or((0, 0));
    let Some(display) = displays.iter().find(|d| holds(d, point)).or_else(|| displays.iter().find(|d| holds(d, (0, 0)))).or(displays.first()) else { return (0, None) };
    let (dx, dy, dw, dh) = display.1;
    let part = rect.map(|(x, y, w, h)| {
        // What of it lies on the display, and even both ways, which H.264 wants.
        let (left, top) = ((x.max(dx) - dx).min((dw - 2).max(0)), (y.max(dy) - dy).min((dh - 2).max(0)));
        let (right, bottom) = (((x + w).min(dx + dw) - dx).max(left + 2), ((y + h).min(dy + dh) - dy).max(top + 2));
        (left, top, ((right - left) / 2 * 2).max(2), ((bottom - top) / 2 * 2).max(2))
    });
    (display.0, part)
}

#[cfg(windows)]
fn command(target: &Target, options: Options, path: &Path) -> Result<Started, String> {
    let rect = match target {
        Target::Screen => None,
        Target::Window(w) => Some(physical(w.frame, options.scale)),
        Target::Area(r) => Some(physical(*r, options.scale)),
    };
    // From the graphics card, and encoded by it, where that works.
    if let Some(encoder) = encoder() {
        let (display, part) = placed(&displays(), rect, options.on);
        let mut grab = format!("ddagrab=output_idx={display}:framerate={SMOOTH}:draw_mouse=1");
        if let Some((x, y, w, h)) = part {
            grab.push_str(&format!(":offset_x={x}:offset_y={y}:video_size={w}x{h}"));
        }
        grab.push_str(encoder.handed_over());
        let mut cmd = neo_desktop::command("ffmpeg");
        cmd.args(["-y", "-loglevel", "error", "-f", "lavfi", "-i", &grab]);
        let (sounds, feeds) = add_sound(&mut cmd, options);
        cmd.args(encoder.arguments()).args(crate::sound::mapping(&sounds)).arg(path);
        return Ok((spawn(cmd, "ffmpeg", "Install it, for example with `winget install ffmpeg`, and make sure it is on your PATH.")?, feeds));
    }
    let mut cmd = neo_desktop::command("ffmpeg");
    cmd.args(["-y", "-loglevel", "error", "-f", "gdigrab", "-framerate", "30"]);
    if let Some((x, y, w, h)) = rect {
        cmd.arg("-offset_x").arg(x.to_string()).arg("-offset_y").arg(y.to_string()).arg("-video_size").arg(format!("{w}x{h}"));
    }
    cmd.args(["-i", "desktop"]);
    let (sounds, feeds) = add_sound(&mut cmd, options);
    cmd.args(["-pix_fmt", "yuv420p"]).args(crate::sound::mapping(&sounds)).arg(path);
    Ok((spawn(cmd, "ffmpeg", "Install it, for example with `winget install ffmpeg`, and make sure it is on your PATH.")?, feeds))
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
    let mut cmd = neo_desktop::command("screencapture");
    cmd.args(["-i", "-x"]).arg(path);
    Some(cmd)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn pick_command(path: &Path) -> Option<Command> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        // slurp draws the selection and grim takes the picture of it.
        let mut cmd = neo_desktop::command("sh");
        // The screen dimmed, and the area chosen left clear.
        cmd.args(["-c", "area=$(slurp -b 0000006b -s 00000000 -c ffffffe6) && grim -g \"$area\" \"$0\""]).arg(path);
        return Some(cmd);
    }
    let mut cmd = neo_desktop::command("maim");
    cmd.args(["-s", "-u"]).arg(path);
    Some(cmd)
}

#[cfg(windows)]
fn pick_command(_path: &Path) -> Option<Command> {
    None
}

/// Whether an area can be dragged out with the mouse: with the system's
/// own drag-to-select where it has one, and on Windows, which has none to
/// hand over to, with NeoCap's own over a picture of the screen.
pub fn can_pick() -> bool {
    cfg!(windows) || pick_command(Path::new("probe.png")).is_some()
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
    /// Where on the desktop its corner is, in points: nothing where it is
    /// of the main display from its very corner; on Windows the corner of
    /// the display it was taken of, which with two is not always that.
    pub corner: (f32, f32),
}

/// Takes a picture of the whole screen now, into `file`, to choose an
/// area of at leisure: whatever moves on the screen after this is not in
/// it. Blocks for a moment, so call it off the main thread.
///
/// `screen` is the display the area will be chosen on, in points, and
/// `scale` its pixels to a point. On Windows the picture is of that
/// display alone, read straight from the screen and kept only in memory:
/// with two displays the system's "whole screen" is both side by side.
/// Elsewhere it is of the main display, in the file.
#[cfg_attr(not(windows), allow(unused_variables))]
pub fn still(file: &Path, screen: Rect, scale: f32) -> Result<Still, String> {
    #[cfg(windows)]
    if screen.w > 0.0 && screen.h > 0.0 {
        let by = scale.max(1.0);
        let (x, y, w, h) = ((screen.x * by).round() as i32, (screen.y * by).round() as i32, (screen.w * by).round() as i32, (screen.h * by).round() as i32);
        let rgba = grab(x, y, w, h)?;
        return Ok(Still { file: file.to_path_buf(), width: w as u32, height: h as u32, rgba, corner: (screen.x, screen.y) });
    }
    screenshot(&Target::Screen, Options::default(), file)?;
    let (width, height, rgba) = read_png(file).inspect_err(|_| {
        let _ = std::fs::remove_file(file);
    })?;
    Ok(Still { file: file.to_path_buf(), width: width as u32, height: height as u32, rgba, corner: (0.0, 0.0) })
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
    let cut = cfg!(target_os = "macos") && neo_desktop::command("/usr/bin/sips").args(["-c", &h.to_string(), &w.to_string(), "--cropOffset", &y.to_string(), &x.to_string()]).arg(&still.file).arg("--out").arg(path).stdin(Stdio::null()).output().is_ok_and(|o| o.status.success());
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
    let mut cmd = neo_desktop::command("screencapture");
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
        let mut cmd = neo_desktop::command("grim");
        if let Some((x, y, w, h)) = rect {
            cmd.arg("-g").arg(format!("{x},{y} {w}x{h}"));
        }
        cmd.arg(path);
        return cmd;
    }
    let display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0".into());
    let mut cmd = neo_desktop::command("ffmpeg");
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
    let mut cmd = neo_desktop::command("ffmpeg");
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

/// The pixels of a part of the screen, as RGBA, top row first: copied
/// from the screen as the system draws it, which takes some tens of
/// milliseconds and needs no other program.
#[cfg(windows)]
fn grab(x: i32, y: i32, w: i32, h: i32) -> Result<Vec<u8>, String> {
    use windows_sys::Win32::Graphics::Gdi::{BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, ReleaseDC, SRCCOPY, SelectObject};
    if w <= 0 || h <= 0 {
        return Err("There is no screen to take a picture of.".into());
    }
    // SAFETY: each thing made here is let go before returning, and the
    // pixels are read only for the length the bitmap was made with.
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        if screen.is_null() {
            return Err("The screen could not be read.".into());
        }
        let memory = CreateCompatibleDC(screen);
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader = BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, ..std::mem::zeroed() };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let bitmap = CreateDIBSection(memory, &info, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        let mut out = Err("The screen could not be read.".to_owned());
        if !bitmap.is_null() && !bits.is_null() {
            let before = SelectObject(memory, bitmap);
            if BitBlt(memory, 0, 0, w, h, screen, x, y, SRCCOPY | CAPTUREBLT) != 0 {
                // Blue, green, red as the system keeps them; and wholly opaque.
                let taken = std::slice::from_raw_parts(bits as *const u8, w as usize * h as usize * 4);
                out = Ok(taken.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], 255]).collect());
            }
            SelectObject(memory, before);
        }
        if !bitmap.is_null() {
            DeleteObject(bitmap);
        }
        DeleteDC(memory);
        ReleaseDC(std::ptr::null_mut(), screen);
        out
    }
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
pub const MICROPHONE: bool = true;

/// Whether what the computer is playing can be recorded. Not on macOS
/// yet: the system gives it only through ScreenCaptureKit, which NeoCap
/// does not use.
pub const SYSTEM_SOUND: bool = cfg!(not(target_os = "macos"));

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
        let mut session = Self { target, options, final_path, parts: vec![], child: None, sound: vec![], stopping: vec![], recorded: Duration::ZERO, since: None };
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
        let (mut child, sound) = command(&self.target, self.options, &path)?;
        // A recorder that cannot start, for lack of permission say, exits at once.
        std::thread::sleep(Duration::from_millis(250));
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("The recorder stopped as soon as it started ({status}). {}", permission_hint()));
        }
        self.parts.push(path);
        self.child = Some(child);
        self.sound = sound;
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
            self.sound.clear();
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
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GWL_EXSTYLE, GetWindowLongW, GetWindowRect, GetWindowThreadProcessId, InternalGetWindowText, IsIconic, IsWindowVisible, WS_EX_TOOLWINDOW};

    unsafe extern "system" fn each(hwnd: HWND, out: LPARAM) -> BOOL {
        // SAFETY: `out` is the Vec passed to EnumWindows below, alive for the call.
        let out = unsafe { &mut *(out as *mut Vec<WindowInfo>) };
        unsafe {
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, &mut pid);
            // This app's own are passed by before anything is asked of
            // them. Nothing below sends a window a message, either: asking
            // one for its title that way waits on the thread it belongs
            // to, and a window of another thread of this app, or of an
            // app that has stopped answering, would never reply.
            if pid == std::process::id() || IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 {
                return 1;
            }
            // Tool windows, and those Windows keeps out of sight while it
            // calls them visible: overlays, and the apps of other desktops.
            let mut cloaked = 0u32;
            DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED as u32, (&raw mut cloaked).cast(), size_of::<u32>() as u32);
            if cloaked != 0 || GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW != 0 {
                return 1;
            }
            let mut buf = [0u16; 512];
            let n = InternalGetWindowText(hwnd, buf.as_mut_ptr(), buf.len() as i32).max(0) as usize;
            let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            if n == 0 || GetWindowRect(hwnd, &mut r) == 0 || r.right - r.left < 80 || r.bottom - r.top < 60 {
                return 1;
            }
            // The app is known by its program's name, as far as Windows will say.
            let app = program_of(pid).and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned())).unwrap_or_default();
            // Window rectangles are physical pixels; `physical` is told a scale of 1 for them.
            out.push(WindowInfo { id: hwnd as usize as u64, pid, app, title: String::from_utf16_lossy(&buf[..n]), frame: Rect::new(r.left as f32, r.top as f32, (r.right - r.left) as f32, (r.bottom - r.top) as f32) });
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

/// The program a process is running, where the system will say: for its
/// name, and for its icon. Only Windows is asked so far.
pub fn program_of(pid: u32) -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
        // SAFETY: the handle is checked, used for the one question and closed.
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if process.is_null() {
                return None;
            }
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            let named = QueryFullProcessImageNameW(process, 0, buf.as_mut_ptr(), &mut len) != 0;
            CloseHandle(process);
            named.then(|| std::path::PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
        }
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        None
    }
}

/// X11 sessions list windows through `wmctrl` when it is installed. Wayland
/// does not let apps see each other's windows.
#[cfg(all(unix, not(target_os = "macos")))]
fn imp_windows() -> Vec<WindowInfo> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        return vec![];
    }
    let Ok(out) = neo_desktop::command("wmctrl").args(["-l", "-G", "-p"]).output() else { return vec![] };
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

    /// By hand, on Windows: takes the corner of the real screen and saves
    /// it where `NEO_CAP_PROBE` says, to be looked at.
    /// `NEO_CAP_PROBE=out.png cargo test -p neo-recorder the_real_screen -- --ignored`
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn the_real_screen_is_read_without_another_program() {
        let out = PathBuf::from(std::env::var_os("NEO_CAP_PROBE").expect("say where to save it"));
        let began = std::time::Instant::now();
        let still = still(Path::new("never-written.png"), Rect::new(0.0, 0.0, 960.0, 540.0), 1.0).unwrap();
        println!("read {} by {} in {:.0?}", still.width, still.height, began.elapsed());
        assert_eq!((still.width, still.height, still.rgba.len()), (960, 540, 960 * 540 * 4));
        assert!(still.rgba.chunks_exact(4).any(|p| p[0] != still.rgba[0] || p[1] != still.rgba[1]), "not one flat colour");
        save_part(&still, (0, 0, 960, 540), &out).unwrap();
    }

    #[test]
    fn the_display_and_the_part_of_it_to_record_are_found() {
        // Two displays side by side, the second numbered first by the card.
        let two = [(0, (3840, 0, 3840, 2160)), (1, (0, 0, 3840, 2160))];
        assert_eq!(placed(&two, None, None), (1, None), "all of the one the desktop begins on");
        assert_eq!(placed(&two, None, Some((5000, 900))), (0, None), "all of the one a point is on");
        assert_eq!(placed(&two, Some((4000, 100, 801, 601)), None), (0, Some((160, 100, 800, 600))), "a part, counted from its own display's corner, and made even");
        assert_eq!(placed(&two, Some((3700, 1900, 400, 400)), None), (0, Some((0, 1900, 260, 260))), "what hangs over an edge is cut to the display its middle is on");
        assert_eq!(placed(&[], Some((10, 10, 100, 100)), None), (0, None), "with nothing known of the displays, the first, whole");
        // Every encoder has arguments, and the ones the card takes as they lie need nothing done first.
        assert!(Encoder::ALL.iter().all(|e| e.arguments().len() >= 4) && Encoder::Nvidia.handed_over().is_empty() && Encoder::Software.handed_over().contains("hwdownload"));
    }

    /// By hand, on Windows: which encoder this computer has, and its displays.
    /// `cargo test -p neo-recorder this_computers_encoder -- --ignored --nocapture`
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn this_computers_encoder_and_displays() {
        let began = std::time::Instant::now();
        println!("encoder: {:?}, found in {:.0?}; {:?}", encoder(), began.elapsed(), how_recorded());
        println!("displays: {:?}", displays());
    }

    /// By hand, on Windows: records the real screen in the background for
    /// `NEO_CAP_REPLAY_SECONDS`, saves what is kept to `NEO_CAP_PROBE`,
    /// and then a recording of a few seconds beside it.
    /// `NEO_CAP_PROBE=out.mp4 cargo test -p neo-recorder the_last_minute -- --ignored --nocapture`
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn the_last_minute_is_kept_and_saved() {
        let out = PathBuf::from(std::env::var_os("NEO_CAP_PROBE").expect("say where to save it"));
        let seconds: u64 = std::env::var("NEO_CAP_REPLAY_SECONDS").ok().and_then(|s| s.parse().ok()).unwrap_or(35);
        let sounds = Options { system_sound: true, microphone: std::env::var_os("NEO_CAP_MIC").is_some(), scale: 1.0, ..Default::default() };
        let all: Vec<u32> = if std::env::var_os("NEO_CAP_ALL").is_some() { displays().iter().map(|d| d.0).collect() } else { vec![] };
        let mut replay = Replay::start(1, &all, None, sounds).unwrap();
        std::thread::sleep(Duration::from_secs(seconds));
        assert!(replay.running(), "still going after {seconds} s");
        for (display, pieces, about, sounds) in replay.pieces() {
            let began = Instant::now();
            let saved = save_replay(&pieces, about, &sounds, &out.with_file_name(format!("replay-{display}.mp4"))).unwrap();
            println!("display {display}: {} pieces, about {:.0?}; saved {} KB in {:.0?}", pieces.len(), about, saved.bytes / 1024, began.elapsed());
        }
        drop(replay);
        // And an ordinary recording, by the same road.
        let session = Session::start(Target::Area(Rect::new(100.0, 100.0, 801.0, 601.0)), sounds, out.with_file_name("recording.mp4")).unwrap();
        std::thread::sleep(Duration::from_secs(3));
        let saved = session.finish(false).unwrap();
        println!("recorded {} KB, {:.1?}", saved.bytes / 1024, saved.length);
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
        let still = Still { file: file.clone(), width: w, height: h, rgba, corner: (0.0, 0.0) };
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
