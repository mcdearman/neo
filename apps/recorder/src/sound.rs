//! Sound for a recording: what the computer is playing, and the microphone.
//!
//! Where ffmpeg can take sound from the system itself it is simply told
//! to (PulseAudio and PipeWire on Linux, AVFoundation on macOS; see
//! [`crate::capture`]). On Windows it cannot take what the computer is
//! playing at all, so that is taken here, from the sound card's own
//! output as it goes out (WASAPI loopback), and handed to ffmpeg over a
//! socket on this computer as plain samples; the microphone goes the
//! same way, so that the two are alike.

/// Where a sound comes from.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// What the computer is playing.
    Computer,
    Microphone,
}

/// A sound being handed to ffmpeg for as long as this is kept.
#[cfg_attr(not(windows), allow(dead_code))]
pub struct Feed {
    port: u16,
    rate: u32,
    channels: u16,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Feed {
    /// What tells ffmpeg to take this as one of its inputs.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn input(&self) -> Vec<String> {
        vec!["-thread_queue_size".into(), "1024".into(), "-f".into(), "f32le".into(), "-ar".into(), self.rate.to_string(), "-ac".into(), self.channels.to_string(), "-i".into(), format!("tcp://127.0.0.1:{}", self.port)]
    }
}

impl Drop for Feed {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Source {
    /// What its track in a movie is called.
    pub fn name(self) -> &'static str {
        match self {
            Source::Computer => "Computer sound",
            Source::Microphone => "Microphone",
        }
    }
}

/// What tells ffmpeg which of its inputs make the movie's sound, when the
/// picture is its first input and `sounds` follow it in that order. One is
/// taken as it is. Two make three tracks: the first both mixed, each as
/// loud as it was, which is what a player plays; then each on its own, for
/// an editor to set apart.
pub fn mapping(sounds: &[Source]) -> Vec<String> {
    let mut said: Vec<String> = match sounds.len() {
        0 => return vec![],
        1 => ["-map", "0:v", "-map", "1:a"].map(str::to_owned).to_vec(),
        _ => ["-filter_complex", "[1:a][2:a]amix=inputs=2:duration=longest:normalize=0[sound]", "-map", "0:v", "-map", "[sound]", "-map", "1:a", "-map", "2:a"].map(str::to_owned).to_vec(),
    };
    said.extend(["-c:a", "aac", "-b:a", "160k"].map(str::to_owned));
    said.extend(track_names(sounds));
    said
}

/// What names the sound tracks [`mapping`] makes, and marks the first as
/// the one to play: said again when pieces are joined, since the pieces of
/// a background recording cannot keep it.
pub fn track_names(sounds: &[Source]) -> Vec<String> {
    let names: Vec<&str> = match sounds {
        [] => vec![],
        [one] => vec![one.name()],
        _ => std::iter::once("Everything").chain(sounds.iter().map(|s| s.name())).collect(),
    };
    names
        .iter()
        .enumerate()
        // A movie keeps a track's title as its name; editors show its handler's name.
        .flat_map(|(i, name)| {
            let named = format!("-metadata:s:a:{i}");
            [named.clone(), format!("title={name}"), named, format!("handler_name={name}"), format!("-disposition:a:{i}"), (if i == 0 { "default" } else { "0" }).to_owned()]
        })
        .collect()
}

#[cfg(windows)]
impl Feed {
    /// Begins taking a sound. Nothing is sent until ffmpeg asks for it.
    pub fn open(source: Source) -> Result<Self, String> {
        use cpal::traits::{DeviceTrait, HostTrait};
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{Arc, Mutex};
        use std::time::{Duration, Instant};

        let wrong = |e: &dyn std::fmt::Display| match source {
            Source::Computer => format!("The computer's sound could not be recorded: {e}"),
            Source::Microphone => format!("The microphone could not be recorded: {e}"),
        };
        // How it comes, which ffmpeg has to be told before any of it does.
        let device = move |host: &cpal::Host| match source {
            Source::Computer => host.default_output_device(),
            Source::Microphone => host.default_input_device(),
        };
        let host = cpal::default_host();
        let found = device(&host).ok_or_else(|| wrong(&"there is no such device"))?;
        let config = match source {
            Source::Computer => found.default_output_config(),
            Source::Microphone => found.default_input_config(),
        }
        .map_err(|e| wrong(&e))?;
        let (rate, channels, format) = (config.sample_rate().0, config.channels(), config.sample_format());
        if !matches!(format, cpal::SampleFormat::F32 | cpal::SampleFormat::I16) {
            return Err(wrong(&format!("it comes as {format:?}, which is not understood")));
        }
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).map_err(|e| wrong(&e))?;
        let port = listener.local_addr().map_err(|e| wrong(&e))?.port();
        listener.set_nonblocking(true).map_err(|e| wrong(&e))?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        std::thread::spawn(move || {
            use cpal::traits::StreamTrait;
            use std::io::Write;
            // Until ffmpeg comes for it, or the recording is over before it has.
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        if stopped.load(Ordering::Relaxed) {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => return,
                }
            };
            let _ = socket.set_nonblocking(false);
            let _ = socket.set_nodelay(true);
            let Some(device) = device(&cpal::default_host()) else { return };
            let heard: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
            let (into, also) = (heard.clone(), heard.clone());
            // An input made on what plays sound is that sound as it goes out.
            let stream = match format {
                cpal::SampleFormat::F32 => device.build_input_stream(&config.config(), move |data: &[f32], _: &cpal::InputCallbackInfo| into.lock().unwrap_or_else(|e| e.into_inner()).extend_from_slice(data), |_| {}, None),
                _ => device.build_input_stream(&config.config(), move |data: &[i16], _: &cpal::InputCallbackInfo| also.lock().unwrap_or_else(|e| e.into_inner()).extend(data.iter().map(|s| *s as f32 / 32768.0)), |_| {}, None),
            };
            let Ok(stream) = stream else { return };
            if stream.play().is_err() {
                return;
            }
            let (began, per) = (Instant::now(), channels.max(1) as u64);
            let mut sent: u64 = 0;
            while !stopped.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(10));
                let mut samples = std::mem::take(&mut *heard.lock().unwrap_or_else(|e| e.into_inner()));
                // With nothing playing the system hands over nothing at all,
                // and ffmpeg would hold the picture back waiting for sound
                // that is not coming: silence is sent in its place, so the
                // sound keeps time with the clock.
                let (due, have) = ((began.elapsed().as_secs_f64() * rate as f64) as u64, sent + samples.len() as u64 / per);
                if have + rate as u64 / 10 < due {
                    let quiet = due - rate as u64 / 50 - have;
                    samples.resize(samples.len() + (quiet * per) as usize, 0.0);
                }
                if samples.is_empty() {
                    continue;
                }
                let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
                if socket.write_all(&bytes).is_err() {
                    break;
                }
                sent += samples.len() as u64 / per;
            }
        });
        Ok(Self { port, rate, channels, stop })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffmpeg_is_told_which_inputs_are_the_sound() {
        assert!(mapping(&[]).is_empty(), "no sound: nothing said, and the picture is all there is");
        let one = mapping(&[Source::Microphone]);
        assert_eq!(one[..4], ["-map", "0:v", "-map", "1:a"]);
        assert!(one.contains(&"title=Microphone".to_owned()) && !one.iter().any(|a| a.contains("a:1")), "one track, named");
        let two = mapping(&[Source::Computer, Source::Microphone]);
        assert!(two[1].contains("amix=inputs=2") && two.contains(&"[sound]".to_owned()) && two.contains(&"aac".to_owned()));
        let maps: Vec<&str> = two.windows(2).filter(|w| w[0] == "-map").map(|w| w[1].as_str()).collect();
        assert_eq!(maps, ["0:v", "[sound]", "1:a", "2:a"], "both mixed first, then each on its own");
        let titled = |i: usize, name: &str| two.windows(2).any(|w| w[0] == format!("-metadata:s:a:{i}") && w[1] == format!("title={name}"));
        assert!(titled(0, "Everything") && titled(1, "Computer sound") && titled(2, "Microphone"));
        assert!(two.windows(2).any(|w| w == ["-disposition:a:0", "default"]) && two.windows(2).any(|w| w == ["-disposition:a:2", "0"]), "the mix is the one played");
        let feed = Feed { port: 4321, rate: 48000, channels: 2, stop: Default::default() };
        assert_eq!(feed.input()[2..], ["-f", "f32le", "-ar", "48000", "-ac", "2", "-i", "tcp://127.0.0.1:4321"]);
    }
}