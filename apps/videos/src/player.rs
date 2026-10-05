//! Video playback behind one interface.
//!
//! - macOS: AVFoundation, through `mac_player.m`. It decodes with hardware
//!   help, plays the sound, and seeks exactly.
//! - Linux and Windows: `ffmpeg` decodes frames into a pipe and `ffplay`
//!   plays the sound. Pausing and seeking restart both at the new position.
//!   A Neo session would use PipeWire and GStreamer instead.

use std::path::Path;

use neo::Image;

/// Frames wider than this are scaled down as they are decoded.
const MAX_WIDTH: i32 = 1920;

/// Whether the file is ready to play.
// ffmpeg reports problems when the file is opened, so only macOS loads later.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Loading,
    Ready,
    Failed(String),
}

pub use imp::Player;

/// Lets a headless run make progress. Windowed apps never need it.
pub fn pump(seconds: f64) {
    #[cfg(target_os = "macos")]
    imp::pump(seconds);
    #[cfg(not(target_os = "macos"))]
    std::thread::sleep(std::time::Duration::from_secs_f64(seconds));
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use std::ffi::{c_char, c_int, c_void, CString};

    unsafe extern "C" {
        fn neo_player_open(path: *const c_char, max_width: c_int) -> *mut c_void;
        fn neo_player_close(handle: *mut c_void);
        fn neo_player_status(handle: *mut c_void) -> c_int;
        fn neo_player_error(handle: *mut c_void, out: *mut c_char, len: c_int);
        fn neo_player_set_playing(handle: *mut c_void, playing: c_int);
        fn neo_player_playing(handle: *mut c_void) -> c_int;
        fn neo_player_seek(handle: *mut c_void, seconds: f64);
        fn neo_player_time(handle: *mut c_void) -> f64;
        fn neo_player_duration(handle: *mut c_void) -> f64;
        fn neo_player_set_volume(handle: *mut c_void, volume: f32);
        fn neo_player_turns(handle: *mut c_void) -> c_int;
        fn neo_player_next_frame(handle: *mut c_void, width: *mut c_int, height: *mut c_int) -> c_int;
        fn neo_player_copy_frame(handle: *mut c_void, rgba: *mut u8);
        fn neo_player_pump(seconds: f64);
    }

    /// Lets the system player make progress when no window is running the
    /// main loop, as in headless screenshots.
    pub fn pump(seconds: f64) {
        // SAFETY: runs the main run loop from the main thread.
        unsafe { neo_player_pump(seconds) }
    }

    /// An open video. Use it from the main thread only.
    pub struct Player(*mut c_void);

    // SAFETY (all calls below): the handle came from neo_player_open, is
    // closed only in Drop, and is used from the thread that owns the Player.
    impl Player {
        pub fn open(path: &Path) -> Result<Self, String> {
            let c = CString::new(path.to_string_lossy().as_bytes()).map_err(|_| "The file's name cannot be used.".to_string())?;
            let handle = unsafe { neo_player_open(c.as_ptr(), MAX_WIDTH) };
            if handle.is_null() { Err("The video could not be opened.".into()) } else { Ok(Self(handle)) }
        }

        pub fn status(&self) -> Status {
            match unsafe { neo_player_status(self.0) } {
                1 => Status::Ready,
                2 => {
                    let mut buf = [0u8; 512];
                    unsafe { neo_player_error(self.0, buf.as_mut_ptr().cast(), buf.len() as c_int) };
                    let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
                    Status::Failed(String::from_utf8_lossy(&buf[..end]).into_owned())
                }
                _ => Status::Loading,
            }
        }

        pub fn set_playing(&mut self, playing: bool) {
            unsafe { neo_player_set_playing(self.0, playing as c_int) }
        }

        pub fn playing(&self) -> bool {
            unsafe { neo_player_playing(self.0) != 0 }
        }

        pub fn seek(&mut self, seconds: f64) {
            unsafe { neo_player_seek(self.0, seconds.max(0.0)) }
        }

        pub fn position(&self) -> f64 {
            unsafe { neo_player_time(self.0) }
        }

        pub fn duration(&self) -> Option<f64> {
            let d = unsafe { neo_player_duration(self.0) };
            (d > 0.0).then_some(d)
        }

        pub fn set_volume(&mut self, volume: f32) {
            unsafe { neo_player_set_volume(self.0, volume.clamp(0.0, 1.0)) }
        }

        /// Quarter turns clockwise to draw frames with, for video recorded sideways.
        pub fn turns(&self) -> u8 {
            (unsafe { neo_player_turns(self.0) }).rem_euclid(4) as u8
        }

        /// The frame for this moment, if it differs from the last one given.
        /// Pass the previous frame so its texture is reused.
        pub fn frame(&mut self, previous: Option<&Image>) -> Option<Image> {
            let (mut w, mut h) = (0, 0);
            if unsafe { neo_player_next_frame(self.0, &mut w, &mut h) } == 0 || w <= 0 || h <= 0 {
                return None;
            }
            let mut pixels = vec![0u8; w as usize * h as usize * 4];
            unsafe { neo_player_copy_frame(self.0, pixels.as_mut_ptr()) };
            Some(match previous {
                Some(p) => p.next_frame(w as u32, h as u32, pixels),
                None => Image::frame(w as u32, h as u32, pixels),
            })
        }
    }

    impl Drop for Player {
        fn drop(&mut self) {
            unsafe { neo_player_close(self.0) }
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::*;
    use std::io::Read;
    use std::path::PathBuf;
    use std::process::{Child, Command, Stdio};
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    /// What `ffprobe` says about the file.
    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    pub(super) struct Info {
        pub width: u32,
        pub height: u32,
        pub fps: f64,
        pub duration: f64,
        /// Quarter turns clockwise stored by the camera.
        pub turns: u8,
    }

    /// Parses `key=value` lines from `ffprobe -of default=noprint_wrappers=1`.
    pub(super) fn parse_info(text: &str) -> Option<Info> {
        let mut info = Info { fps: 30.0, ..Default::default() };
        for line in text.lines() {
            let Some((key, value)) = line.trim().split_once('=') else { continue };
            match key {
                "width" => info.width = value.parse().ok()?,
                "height" => info.height = value.parse().ok()?,
                "r_frame_rate" => {
                    // A fraction such as 30000/1001.
                    let (n, d) = value.split_once('/').unwrap_or((value, "1"));
                    let (n, d): (f64, f64) = (n.parse().ok()?, d.parse().ok()?);
                    if n > 0.0 && d > 0.0 {
                        info.fps = (n / d).clamp(1.0, 120.0);
                    }
                }
                "duration" => info.duration = value.parse().unwrap_or(0.0),
                // The rotation that shows the video upright, anticlockwise.
                "rotation" => info.turns = (-(value.parse::<f64>().unwrap_or(0.0)) / 90.0).round().rem_euclid(4.0) as u8,
                _ => {}
            }
        }
        (info.width > 0 && info.height > 0).then_some(info)
    }

    /// The size frames are decoded at: no wider than the limit, and even.
    pub(super) fn output_size(info: &Info) -> (u32, u32) {
        let w = info.width.min(MAX_WIDTH as u32);
        let h = (info.height as f64 * w as f64 / info.width as f64).round() as u32;
        ((w & !1).max(2), (h & !1).max(2))
    }

    fn missing(e: std::io::Error, tool: &str) -> String {
        if e.kind() == std::io::ErrorKind::NotFound { format!("Playing video needs {tool}, which is not installed.") } else { format!("Could not run {tool}: {e}") }
    }

    pub struct Player {
        path: PathBuf,
        info: Info,
        size: (u32, u32),
        /// Where playback last started or stopped, in seconds.
        base: f64,
        /// When playback last started. `None` while paused.
        started: Option<Instant>,
        video: Option<Child>,
        audio: Option<Child>,
        /// The newest decoded frame, written by the reader thread.
        latest: Arc<Mutex<Option<Vec<u8>>>>,
        volume: f32,
    }

    impl Player {
        pub fn open(path: &Path) -> Result<Self, String> {
            let out = Command::new("ffprobe")
                .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height,r_frame_rate:stream_side_data=rotation:format=duration", "-of", "default=noprint_wrappers=1"])
                .arg(path)
                .output()
                .map_err(|e| missing(e, "ffmpeg"))?;
            let info = parse_info(&String::from_utf8_lossy(&out.stdout)).ok_or_else(|| "This file has no video that ffmpeg can read.".to_string())?;
            let mut player = Self { path: path.to_path_buf(), info, size: output_size(&info), base: 0.0, started: None, video: None, audio: None, latest: Default::default(), volume: 1.0 };
            // Show the first frame before anything is played.
            player.decode_from(0.0, false);
            Ok(player)
        }

        fn stop_children(&mut self) {
            for mut child in [self.video.take(), self.audio.take()].into_iter().flatten() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }

        /// Starts `ffmpeg` writing frames from `at`. When playing it runs at
        /// the video's own speed; otherwise it writes one frame and stops.
        fn decode_from(&mut self, at: f64, playing: bool) {
            let (w, h) = self.size;
            let mut cmd = Command::new("ffmpeg");
            cmd.args(["-loglevel", "error", "-noautorotate"]);
            if playing {
                // -re reads the input in real time, so frames arrive when due.
                cmd.arg("-re");
            }
            cmd.arg("-ss").arg(format!("{at:.3}")).arg("-i").arg(&self.path).args(["-an", "-vf"]).arg(format!("scale={w}:{h}"));
            if !playing {
                cmd.args(["-frames:v", "1"]);
            }
            cmd.args(["-f", "rawvideo", "-pix_fmt", "rgba", "-"]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
            let Ok(mut child) = cmd.spawn() else { return };
            if let Some(mut stdout) = child.stdout.take() {
                let latest = self.latest.clone();
                let frame_len = w as usize * h as usize * 4;
                std::thread::spawn(move || {
                    let mut buf = vec![0u8; frame_len];
                    // The pipe closes when ffmpeg exits or is stopped.
                    while stdout.read_exact(&mut buf).is_ok() {
                        if let Ok(mut slot) = latest.lock() {
                            *slot = Some(buf.clone());
                        }
                    }
                });
            }
            self.video = Some(child);
        }

        fn play_audio_from(&mut self, at: f64) {
            let volume = (self.volume * 100.0).round() as u32;
            self.audio = Command::new("ffplay")
                .args(["-nodisp", "-autoexit", "-loglevel", "quiet", "-vn"])
                .arg("-ss")
                .arg(format!("{at:.3}"))
                .arg("-volume")
                .arg(volume.to_string())
                .arg(&self.path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .ok();
        }

        pub fn status(&self) -> Status {
            Status::Ready
        }

        pub fn set_playing(&mut self, playing: bool) {
            if playing == self.playing() {
                return;
            }
            let at = self.position();
            self.stop_children();
            self.base = at;
            if playing {
                self.started = Some(Instant::now());
                self.decode_from(at, true);
                self.play_audio_from(at);
            } else {
                self.started = None;
            }
        }

        pub fn playing(&self) -> bool {
            self.started.is_some()
        }

        pub fn seek(&mut self, seconds: f64) {
            let at = seconds.clamp(0.0, self.info.duration.max(0.0));
            let playing = self.playing();
            self.stop_children();
            self.base = at;
            self.started = playing.then(Instant::now);
            self.decode_from(at, playing);
            if playing {
                self.play_audio_from(at);
            }
        }

        pub fn position(&self) -> f64 {
            let at = self.base + self.started.map_or(0.0, |s| s.elapsed().as_secs_f64());
            if self.info.duration > 0.0 { at.min(self.info.duration) } else { at }
        }

        pub fn duration(&self) -> Option<f64> {
            (self.info.duration > 0.0).then_some(self.info.duration)
        }

        pub fn set_volume(&mut self, volume: f32) {
            let volume = volume.clamp(0.0, 1.0);
            if (volume - self.volume).abs() < 0.005 {
                return;
            }
            self.volume = volume;
            // ffplay takes its volume when it starts, so start it again.
            if self.playing() {
                if let Some(mut audio) = self.audio.take() {
                    let _ = audio.kill();
                    let _ = audio.wait();
                }
                self.play_audio_from(self.position());
            }
        }

        pub fn turns(&self) -> u8 {
            self.info.turns
        }

        pub fn frame(&mut self, previous: Option<&Image>) -> Option<Image> {
            // Reaching the end stops the clock.
            if self.playing() && self.info.duration > 0.0 && self.position() >= self.info.duration {
                self.stop_children();
                self.base = self.info.duration;
                self.started = None;
            }
            let pixels = self.latest.lock().ok()?.take()?;
            let (w, h) = self.size;
            Some(match previous {
                Some(p) => p.next_frame(w, h, pixels),
                None => Image::frame(w, h, pixels),
            })
        }
    }

    impl Drop for Player {
        fn drop(&mut self) {
            self.stop_children();
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn reads_ffprobe_output() {
            let info = parse_info("width=3840\nheight=2160\nr_frame_rate=30000/1001\nrotation=-90\nduration=12.512000\n").unwrap();
            assert_eq!((info.width, info.height, info.turns), (3840, 2160, 1));
            assert!((info.fps - 29.97).abs() < 0.01 && (info.duration - 12.512).abs() < 1e-6);
            assert_eq!(output_size(&info), (1920, 1080), "4K is decoded at 1080p");
            assert_eq!(output_size(&Info { width: 853, height: 481, ..info }), (852, 480), "sizes are made even");
            assert!(parse_info("duration=3.0\n").is_none(), "no video stream");
        }
    }
}
