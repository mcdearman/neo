//! Hearing: what is said in a video or a recording.
//!
//! The sound is taken out with ffmpeg and written down by Whisper, which
//! runs on this computer as the rest does (`whisper-cli`, from
//! whisper.cpp), with a model of its own of about 150 megabytes that is
//! downloaded once. What comes of it is text, remembered and searched as
//! a document's is: "the video where we talk about the ferry".

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The program that writes speech down.
pub const TOOL: &str = "whisper-cli";
/// Where its model comes from: Whisper's "base" model, which knows many
/// languages, in whisper.cpp's form.
pub const MODEL_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin";
/// About how large it is, for saying before it is fetched.
pub const MODEL_BYTES: u64 = 148_000_000;
/// No more than this much of a recording is listened to, in seconds: the
/// start of a long one says what it is.
const LONGEST: u32 = 20 * 60;

/// Where the model is kept.
pub fn model_file() -> PathBuf {
    crate::dir().join("whisper-base.bin")
}

/// What stands in the way of hearing, if anything does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Missing {
    /// `whisper-cli` is not installed.
    Tool,
    /// Its model has not been downloaded.
    Model,
    /// `ffmpeg`, which takes the sound out of a file, is not installed.
    Ffmpeg,
}

/// Whether sound can be listened to, and if not, what is missing.
pub fn missing() -> Option<Missing> {
    missing_with(&model_file())
}

fn missing_with(model: &Path) -> Option<Missing> {
    if !neo_desktop::fs::tool(TOOL).is_file() {
        Some(Missing::Tool)
    } else if !neo_desktop::fs::tool("ffmpeg").is_file() {
        Some(Missing::Ffmpeg)
    } else if !model.is_file() {
        Some(Missing::Model)
    } else {
        None
    }
}

/// Downloads the model, telling `progress` how many bytes have come of
/// how many. Returning false from it gives up. It is written beside its
/// place and moved there whole, so a download cut short leaves nothing
/// that looks like a model.
pub fn download(progress: &mut dyn FnMut(u64, u64) -> bool) -> Result<(), String> {
    download_to(MODEL_URL, &model_file(), progress)
}

pub fn download_to(url: &str, to: &Path, progress: &mut dyn FnMut(u64, u64) -> bool) -> Result<(), String> {
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let part = to.with_extension("part");
    let _ = std::fs::remove_file(&part);
    // The system's own downloader, which every system has and which knows
    // how to follow where the file has moved to.
    let mut curl = Command::new(neo_desktop::fs::tool("curl")).args(["--location", "--fail", "--silent", "--show-error", "--connect-timeout", "15", "--output"]).arg(&part).arg(url).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().map_err(|e| format!("The model could not be fetched: {e}"))?;
    // How far it has got is how much of the file is there.
    let fetched = loop {
        match curl.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => return Err(format!("The model could not be fetched: {e}")),
        }
        let done = std::fs::metadata(&part).map_or(0, |m| m.len());
        if !progress(done, MODEL_BYTES.max(done)) {
            let _ = curl.kill();
            let _ = curl.wait();
            let _ = std::fs::remove_file(&part);
            return Err("Stopped.".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    };
    let done = std::fs::metadata(&part).map_or(0, |m| m.len());
    // Much smaller than it should be is an error page, not a model.
    if !fetched.success() || (done < 1_000_000 && url == MODEL_URL) {
        let mut why = String::new();
        if let Some(mut err) = curl.stderr.take() {
            let _ = std::io::Read::read_to_string(&mut err, &mut why);
        }
        let _ = std::fs::remove_file(&part);
        return Err(format!("The model could not be fetched: {}", if why.trim().is_empty() { "what came was not the model." } else { why.trim() }));
    }
    progress(done, done);
    std::fs::rename(&part, to).map_err(|e| e.to_string())
}

/// What Whisper writes for sound that is not speech, which is not what
/// was said: "[MUSIC]", "(applause)", "[BLANK_AUDIO]".
fn is_noise(piece: &str) -> bool {
    let piece = piece.trim();
    (piece.starts_with('[') && piece.ends_with(']')) || (piece.starts_with('(') && piece.ends_with(')')) || (piece.starts_with('*') && piece.ends_with('*'))
}

/// Tidies what Whisper wrote: one run of text, without its marks for
/// music and silence, and without a line said over and over, which is
/// what it does with sound it can make nothing of.
pub fn tidy(written: &str) -> String {
    let mut out: Vec<&str> = vec![];
    let mut repeats = 0;
    for line in written.lines().map(str::trim).filter(|l| !l.is_empty() && !is_noise(l)) {
        if out.last() == Some(&line) {
            repeats += 1;
            // Twice may be said; more is the model going round.
            if repeats >= 2 {
                continue;
            }
        } else {
            repeats = 0;
        }
        out.push(line);
    }
    out.join(" ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Writes down what is said in a video or a recording. Nothing, for one
/// with no sound or no speech. An error if it could not be listened to.
pub fn transcribe(path: &Path) -> Result<String, String> {
    transcribe_with(path, &model_file())
}

pub fn transcribe_with(path: &Path, model: &Path) -> Result<String, String> {
    // Whisper wants its sound a particular way: one channel, 16 kHz.
    let wav = std::env::temp_dir().join(format!("neo-apollo-hear-{}-{:x}.wav", std::process::id(), path.to_string_lossy().bytes().fold(0u64, |h, b| h.wrapping_mul(31).wrapping_add(u64::from(b)))));
    let taken = Command::new(neo_desktop::fs::tool("ffmpeg")).args(["-y", "-v", "error", "-i"]).arg(path).args(["-vn", "-ac", "1", "-ar", "16000", "-t", &LONGEST.to_string(), "-f", "wav"]).arg(&wav).stdin(Stdio::null()).stderr(Stdio::null()).status();
    // No sound in it at all is not a failure: there is nothing to hear.
    if !taken.is_ok_and(|s| s.success()) || std::fs::metadata(&wav).map_or(0, |m| m.len()) < 2000 {
        let _ = std::fs::remove_file(&wav);
        return Ok(String::new());
    }
    let heard = Command::new(neo_desktop::fs::tool(TOOL)).arg("-m").arg(model).arg("-f").arg(&wav).args(["-nt", "-np", "-l", "auto", "-sns"]).stdin(Stdio::null()).stderr(Stdio::null()).output();
    let _ = std::fs::remove_file(&wav);
    let heard = heard.map_err(|e| format!("Couldn't run {TOOL}: {e}"))?;
    if !heard.status.success() {
        return Err(format!("{TOOL} could not make anything of it."));
    }
    Ok(tidy(&String::from_utf8_lossy(&heard.stdout)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_whisper_writes_is_tidied() {
        let written = " [MUSIC]\n We take the ferry on Friday.\n\n (applause)\n The ferry back\n leaves at four.\n [BLANK_AUDIO]\n";
        assert_eq!(tidy(written), "We take the ferry on Friday. The ferry back leaves at four.");
        assert_eq!(tidy(" [MUSIC]\n *sighs*\n"), "", "no speech is nothing said");
        assert_eq!(tidy("Thank you.\nThank you.\nThank you.\nThank you.\nThank you.\nGoodbye."), "Thank you. Thank you. Goodbye.", "a line it goes round on is cut short");
        assert_eq!(tidy(""), "");
    }

    #[test]
    fn what_is_missing_is_said_in_order() {
        let nowhere = std::env::temp_dir().join("neo-apollo-no-such-model.bin");
        // Whatever is installed here, a model that is not there is missing at the latest.
        assert!(missing_with(&nowhere).is_some());
    }

    #[test]
    fn a_download_cut_short_leaves_nothing_behind() {
        let to = std::env::temp_dir().join(format!("neo-apollo-hear-{}/model.bin", std::process::id()));
        // Nothing listens on the discard port.
        let failed = download_to("http://127.0.0.1:9/model.bin", &to, &mut |_, _| true);
        assert!(failed.unwrap_err().starts_with("The model could not be fetched"));
        assert!(!to.exists() && !to.with_extension("part").exists());
        let _ = std::fs::remove_dir_all(to.parent().unwrap());
    }

    /// By hand, with the real tool and model: `NEO_APOLLO_HEAR=file cargo test -p neo-apollo-core real_hearing -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_hearing() {
        println!("missing: {:?}", missing());
        if missing() == Some(Missing::Model) {
            let mut last = 0;
            download(&mut |done, total| {
                if done * 10 / total != last {
                    last = done * 10 / total;
                    println!("{}%", last * 10);
                }
                true
            })
            .unwrap();
        }
        if let Ok(file) = std::env::var("NEO_APOLLO_HEAR") {
            let began = std::time::Instant::now();
            println!("heard in {:?}: {:?}", began.elapsed(), transcribe(Path::new(&file)));
        }
    }
}
