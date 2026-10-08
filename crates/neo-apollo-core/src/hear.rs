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
/// A model that writes speech down: Whisper, in one of its sizes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Listener {
    /// What it is called in settings, and in the name of its file.
    pub id: &'static str,
    /// What it is called to the user.
    pub name: &'static str,
    /// What is to be said for it.
    pub note: &'static str,
    /// Its file among whisper.cpp's models.
    file: &'static str,
    /// About how large that is, for saying before it is fetched.
    pub bytes: u64,
}

/// The models there are to choose from, smallest first. Larger ones get
/// more of the words right, names and accents and speech over noise above
/// all, and take longer over it and more memory while they listen.
pub const LISTENERS: [Listener; 4] = [
    Listener { id: "base", name: "Base", note: "Quick and small; gets plain speech, and stumbles on names and noise.", file: "ggml-base.bin", bytes: 148_000_000 },
    Listener { id: "small", name: "Small", note: "Noticeably better than Base, at three times the size.", file: "ggml-small.bin", bytes: 488_000_000 },
    Listener { id: "turbo", name: "Large v3 Turbo", note: "Close to the best there is and still quick, packed to about the size of Small. The one to choose if the words matter.", file: "ggml-large-v3-turbo-q5_0.bin", bytes: 574_000_000 },
    Listener { id: "turbo-full", name: "Large v3 Turbo, unpacked", note: "The same model at full size; a shade more exact, and three times the memory.", file: "ggml-large-v3-turbo.bin", bytes: 1_625_000_000 },
];

/// Which of them is in use: its place in [`LISTENERS`].
static LISTENER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The model in use.
pub fn listener() -> Listener {
    LISTENERS[LISTENER.load(std::sync::atomic::Ordering::Relaxed).min(LISTENERS.len() - 1)]
}

/// Puts another model to use, by its id. One that is not known leaves
/// things as they are.
pub fn use_listener(id: &str) {
    if let Some(i) = LISTENERS.iter().position(|l| l.id == id) {
        LISTENER.store(i, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Listener {
    /// Where it is fetched from.
    pub fn url(&self) -> String {
        format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{}", self.file)
    }

    /// Where it is kept once fetched.
    pub fn kept_at(&self) -> PathBuf {
        crate::dir().join(format!("whisper-{}.bin", self.id))
    }

    /// Whether it has been fetched.
    pub fn here(&self) -> bool {
        self.kept_at().is_file()
    }

    /// How many recordings can be listened to at once with it without
    /// the listening itself crowding the computer.
    pub fn runners(&self) -> usize {
        if self.bytes < 300_000_000 { 2 } else { 1 }
    }
}

/// How much of a recording is listened to at a time, in seconds of sound
/// with the silences taken out. The first stretch is listened to when the
/// file is read, so that it is known what it is; the rest is listened to
/// behind, a stretch at a time.
const STRETCH: u32 = 20 * 60;

static STRETCH_NOW: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(STRETCH);

/// How long a stretch is.
pub fn stretch() -> u32 {
    STRETCH_NOW.load(std::sync::atomic::Ordering::Relaxed)
}

/// Makes a stretch another length: for trying this out on recordings of
/// seconds, not hours.
pub fn set_stretch(seconds: u32) {
    STRETCH_NOW.store(seconds.max(1), std::sync::atomic::Ordering::Relaxed);
}

/// Takes the dead space out of a recording: silence at its start, and
/// any stretch of silence longer than a breath anywhere in it. What is
/// left is the sound worth listening to, end to end.
const WITHOUT_SILENCE: &str = "silenceremove=start_periods=1:start_threshold=-45dB:stop_periods=-1:stop_duration=1.2:stop_threshold=-45dB";

/// Where the model in use is kept.
pub fn model_file() -> PathBuf {
    listener().kept_at()
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
    let model = listener();
    download_to(&model.url(), &model.kept_at(), model.bytes, progress)
}

pub fn download_to(url: &str, to: &Path, about: u64, progress: &mut dyn FnMut(u64, u64) -> bool) -> Result<(), String> {
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
        if !progress(done, about.max(done)) {
            let _ = curl.kill();
            let _ = curl.wait();
            let _ = std::fs::remove_file(&part);
            return Err("Stopped.".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    };
    let done = std::fs::metadata(&part).map_or(0, |m| m.len());
    // Much smaller than it should be is an error page, not a model.
    if !fetched.success() || done < about / 4 {
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

/// A stretch of a recording, listened to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Heard {
    /// What was said in it.
    pub text: String,
    /// How many seconds of sound there were in the stretch. Less than a
    /// whole stretch means the recording ended in it.
    pub seconds: f32,
}

impl Heard {
    /// Whether the recording goes on past this stretch.
    pub fn more(&self, stretch: u32) -> bool {
        self.seconds + 1.0 >= stretch as f32
    }
}

/// Writes down what is said in the first stretch of a video or a
/// recording, silences left out. Nothing, for one with no sound or no
/// speech. An error if it could not be listened to.
pub fn transcribe(path: &Path) -> Result<Heard, String> {
    transcribe_from(path, 0)
}

/// As [`transcribe`], of the stretch that begins `from` seconds into the
/// recording's sound, counted with its silences left out.
pub fn transcribe_from(path: &Path, from: u32) -> Result<Heard, String> {
    transcribe_with(path, &model_file(), from, stretch())
}

pub fn transcribe_with(path: &Path, model: &Path, from: u32, stretch: u32) -> Result<Heard, String> {
    // Whisper wants its sound a particular way: one channel, 16 kHz. Each
    // listener has a file of its own, for several go at once.
    let wav = std::env::temp_dir().join(format!("neo-apollo-hear-{}-{:x}-{from}.wav", std::process::id(), path.to_string_lossy().bytes().fold(0u64, |h, b| h.wrapping_mul(31).wrapping_add(u64::from(b)))));
    let stretch_of_it = format!("{WITHOUT_SILENCE},atrim=start={from}:duration={stretch},asetpts=PTS-STARTPTS");
    let taken = Command::new(neo_desktop::fs::tool("ffmpeg")).args(["-y", "-v", "error", "-i"]).arg(path).args(["-vn", "-ac", "1", "-ar", "16000", "-af", &stretch_of_it, "-f", "wav"]).arg(&wav).stdin(Stdio::null()).stderr(Stdio::null()).status();
    // 16-bit, one channel, 16 kHz: 32,000 bytes a second after the header.
    let seconds = std::fs::metadata(&wav).map_or(0.0, |m| m.len().saturating_sub(44) as f32 / 32_000.0);
    // No sound in it at all, or none left, is not a failure: there is nothing to hear.
    if !taken.is_ok_and(|s| s.success()) || seconds < 0.2 {
        let _ = std::fs::remove_file(&wav);
        return Ok(Heard::default());
    }
    let heard = Command::new(neo_desktop::fs::tool(TOOL)).arg("-m").arg(model).arg("-f").arg(&wav).args(["-nt", "-np", "-l", "auto", "-sns"]).stdin(Stdio::null()).stderr(Stdio::null()).output();
    let _ = std::fs::remove_file(&wav);
    let heard = heard.map_err(|e| format!("Couldn't run {TOOL}: {e}"))?;
    if !heard.status.success() {
        return Err(format!("{TOOL} could not make anything of it."));
    }
    Ok(Heard { text: tidy(&String::from_utf8_lossy(&heard.stdout)), seconds })
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
        assert!(Heard { text: String::new(), seconds: 1199.6 }.more(1200) && !Heard { text: String::new(), seconds: 640.0 }.more(1200));
    }

    #[test]
    fn there_are_models_to_choose_from_and_one_is_in_use() {
        assert_eq!(LISTENERS.map(|l| l.id), ["base", "small", "turbo", "turbo-full"]);
        assert!(LISTENERS.windows(2).all(|w| w[0].bytes < w[1].bytes), "smallest first");
        let turbo = LISTENERS[2];
        assert_eq!(turbo.url(), "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin");
        assert!(turbo.kept_at().ends_with("whisper-turbo.bin") && LISTENERS[0].kept_at().ends_with("whisper-base.bin"));
        assert_eq!((LISTENERS[0].runners(), turbo.runners()), (2, 1), "a large one listens to one thing at a time");
        // One not known changes nothing. (The one in use is not changed here: other tests listen with it.)
        let before = listener();
        use_listener("no-such-model");
        assert_eq!(listener(), before);
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
        let failed = download_to("http://127.0.0.1:9/model.bin", &to, 1000, &mut |_, _| true);
        assert!(failed.unwrap_err().starts_with("The model could not be fetched"));
        assert!(!to.exists() && !to.with_extension("part").exists());
        let _ = std::fs::remove_dir_all(to.parent().unwrap());
    }

    /// By hand, with the real tool and model: `NEO_APOLLO_HEAR=file cargo test -p neo-apollo-core real_hearing -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_hearing() {
        if let Ok(id) = std::env::var("NEO_APOLLO_LISTENER") {
            use_listener(&id);
        }
        println!("{} missing: {:?}", listener().name, missing());
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
            let heard = transcribe(Path::new(&file));
            println!("heard in {:?}: {:?}", began.elapsed(), heard);
        }
    }
}
