//! Joining recorded parts and converting a recording to a GIF.
//!
//! macOS uses AVFoundation through the small Objective-C helper in
//! `mac_media.m`, and writes GIFs with the `gif` crate. Other platforms use
//! `ffmpeg`, which they already need for recording.

use std::path::{Path, PathBuf};

/// Frames a second in exported GIFs.
pub const GIF_FPS: f64 = 10.0;
/// GIFs are scaled down to at most this many pixels wide.
pub const GIF_MAX_WIDTH: i32 = 800;

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::{c_char, c_int, c_void, CString};
    use std::path::Path;

    type FrameFn = extern "C" fn(context: *mut c_void, rgba: *const u8, width: c_int, height: c_int) -> c_int;

    unsafe extern "C" {
        fn neo_media_concat(paths: *const *const c_char, count: c_int, out: *const c_char, err: *mut c_char, errlen: c_int) -> c_int;
        fn neo_media_frames(path: *const c_char, fps: f64, max_width: c_int, on_frame: FrameFn, context: *mut c_void, err: *mut c_char, errlen: c_int) -> c_int;
    }

    fn c_path(p: &Path) -> Result<CString, String> {
        CString::new(p.to_string_lossy().as_bytes()).map_err(|_| format!("{} is not a usable path", p.display()))
    }

    fn message(buf: &[u8]) -> String {
        let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
        String::from_utf8_lossy(&buf[..end]).into_owned()
    }

    pub fn concat(parts: &[std::path::PathBuf], out: &Path) -> Result<(), String> {
        let paths: Vec<CString> = parts.iter().map(|p| c_path(p)).collect::<Result<_, _>>()?;
        let pointers: Vec<*const c_char> = paths.iter().map(|p| p.as_ptr()).collect();
        let out = c_path(out)?;
        let mut err = [0u8; 512];
        // SAFETY: the pointers are valid C strings for the call, and `err` is writable for its length.
        let status = unsafe { neo_media_concat(pointers.as_ptr(), pointers.len() as c_int, out.as_ptr(), err.as_mut_ptr().cast(), err.len() as c_int) };
        if status == 0 { Ok(()) } else { Err(message(&err)) }
    }

    struct GifWriter {
        path: std::path::PathBuf,
        encoder: Option<gif::Encoder<std::io::BufWriter<std::fs::File>>>,
        delay: u16,
        /// The picture so far, to store only what changes in each frame.
        shown: Vec<u8>,
        error: Option<String>,
    }

    /// Clears the pixels of `frame` that match `shown`, and updates `shown`.
    /// Returns how many pixels changed. Video compression makes still areas
    /// flicker by a level or two, so small differences count as no change.
    pub(super) fn keep_changes(frame: &mut [u8], shown: &mut [u8]) -> usize {
        let mut changed = 0;
        for (new, old) in frame.chunks_exact_mut(4).zip(shown.chunks_exact_mut(4)) {
            let same = (0..3).all(|c| new[c].abs_diff(old[c]) <= 3);
            if same {
                new.copy_from_slice(&[0, 0, 0, 0]);
            } else {
                new[3] = 255;
                old.copy_from_slice(new);
                changed += 1;
            }
        }
        changed
    }

    extern "C" fn on_frame(context: *mut c_void, rgba: *const u8, width: c_int, height: c_int) -> c_int {
        // SAFETY: `context` is the GifWriter passed to neo_media_frames below, and
        // `rgba` holds width × height × 4 bytes for the length of this call.
        let (w, pixels) = unsafe { (&mut *context.cast::<GifWriter>(), std::slice::from_raw_parts(rgba, width as usize * height as usize * 4)) };
        let result = (|| -> Result<(), String> {
            if w.encoder.is_none() {
                let file = std::fs::File::create(&w.path).map_err(|e| e.to_string())?;
                let mut encoder = gif::Encoder::new(std::io::BufWriter::new(file), width as u16, height as u16, &[]).map_err(|e| e.to_string())?;
                encoder.set_repeat(gif::Repeat::Infinite).map_err(|e| e.to_string())?;
                w.encoder = Some(encoder);
            }
            let mut owned = pixels.to_vec();
            let mut frame = if w.shown.is_empty() {
                // The first frame is stored whole.
                owned.chunks_exact_mut(4).for_each(|p| p[3] = 255);
                w.shown = owned.clone();
                gif::Frame::from_rgba_speed(width as u16, height as u16, &mut owned, 10)
            } else if keep_changes(&mut owned, &mut w.shown) == 0 {
                // Nothing moved: one clear pixel holds the picture for another beat.
                gif::Frame { width: 1, height: 1, buffer: vec![0].into(), palette: Some(vec![0; 6]), transparent: Some(0), ..Default::default() }
            } else {
                // Speed 10 trades a little palette quality for much faster export.
                gif::Frame::from_rgba_speed(width as u16, height as u16, &mut owned, 10)
            };
            frame.dispose = gif::DisposalMethod::Keep;
            frame.delay = w.delay;
            w.encoder.as_mut().expect("created above").write_frame(&frame).map_err(|e| e.to_string())
        })();
        match result {
            Ok(()) => 0,
            Err(e) => {
                w.error = Some(e);
                1
            }
        }
    }

    pub fn gif(movie: &Path, out: &Path, fps: f64, max_width: i32) -> Result<(), String> {
        let movie = c_path(movie)?;
        // GIF delays are hundredths of a second.
        let mut writer = GifWriter { path: out.to_path_buf(), encoder: None, delay: (100.0 / fps).round() as u16, shown: vec![], error: None };
        let mut err = [0u8; 512];
        // SAFETY: `writer` outlives the call, which runs `on_frame` only before it returns.
        let status = unsafe { neo_media_frames(movie.as_ptr(), fps, max_width, on_frame, (&raw mut writer).cast(), err.as_mut_ptr().cast(), err.len() as c_int) };
        if let Some(e) = writer.error {
            return Err(e);
        }
        if status != 0 {
            return Err(message(&err));
        }
        // Dropping the encoder writes the GIF trailer.
        writer.encoder.take().map(drop).ok_or_else(|| "the recording had no frames".to_string())
    }
}

#[cfg(not(target_os = "macos"))]
fn ffmpeg(args: &[&std::ffi::OsStr]) -> Result<(), String> {
    let out = std::process::Command::new("ffmpeg").args(["-y", "-loglevel", "error"]).args(args).output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound { "This needs ffmpeg, which is not installed.".to_string() } else { format!("Could not run ffmpeg: {e}") }
    })?;
    if out.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&out.stderr).trim().to_string()) }
}

/// Joins `parts` end to end into `out` without re-encoding.
pub fn concat(parts: &[PathBuf], out: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        mac::concat(parts, out)
    }
    #[cfg(not(target_os = "macos"))]
    {
        // The concat demuxer reads a list file of `file '<path>'` lines.
        let list = out.with_extension("parts.txt");
        let body: String = parts.iter().map(|p| format!("file '{}'\n", p.to_string_lossy().replace('\'', "'\\''"))).collect();
        std::fs::write(&list, body).map_err(|e| e.to_string())?;
        let result = ffmpeg(&["-f".as_ref(), "concat".as_ref(), "-safe".as_ref(), "0".as_ref(), "-i".as_ref(), list.as_os_str(), "-c".as_ref(), "copy".as_ref(), out.as_os_str()]);
        let _ = std::fs::remove_file(list);
        result
    }
}

/// Converts `movie` to an animated GIF at `out`.
pub fn gif(movie: &Path, out: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        mac::gif(movie, out, GIF_FPS, GIF_MAX_WIDTH)
    }
    #[cfg(not(target_os = "macos"))]
    {
        // One palette for the whole clip keeps colours steady between frames.
        let filter = format!("fps={GIF_FPS},scale='min({GIF_MAX_WIDTH},iw)':-1:flags=lanczos,split[a][b];[a]palettegen[p];[b][p]paletteuse");
        ffmpeg(&["-i".as_ref(), movie.as_os_str(), "-vf".as_ref(), filter.as_ref(), out.as_os_str()])
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    #[test]
    fn only_changed_pixels_are_kept() {
        let mut shown = vec![10, 10, 10, 255, 200, 200, 200, 255];
        // The first pixel flickers by two levels; the second really changes.
        let mut frame = vec![12, 9, 10, 255, 90, 200, 200, 255];
        assert_eq!(super::mac::keep_changes(&mut frame, &mut shown), 1);
        assert_eq!(frame, [0, 0, 0, 0, 90, 200, 200, 255]);
        assert_eq!(shown, [10, 10, 10, 255, 90, 200, 200, 255], "the picture so far takes only the real change");
    }
}
