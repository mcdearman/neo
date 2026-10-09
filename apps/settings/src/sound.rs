//! How loud the computer is: what it plays, and what it hears.
//!
//! macOS is asked through AppleScript's own `volume` words, which need
//! no leave from the user. Linux is asked through PipeWire's `wpctl`.

use std::process::{Command, Stdio};

/// How loud things are, each from 0 to 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Levels {
    pub output: f32,
    pub muted: bool,
    /// How loud the microphone is taken, where the system says.
    pub input: Option<f32>,
}

/// Reads `osascript -e 'get volume settings'`:
/// `output volume:50, input volume:75, alert volume:100, output muted:false`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn parse_mac(text: &str) -> Option<Levels> {
    let field = |name: &str| text.split(',').find_map(|part| part.trim().strip_prefix(name)?.strip_prefix(':').map(str::trim));
    let percent = |name: &str| field(name)?.parse::<f32>().ok().map(|v| (v / 100.0).clamp(0.0, 1.0));
    // A device whose loudness is set on the device itself says "missing value".
    Some(Levels { output: percent("output volume")?, muted: field("output muted") == Some("true"), input: percent("input volume") })
}

/// Reads `wpctl get-volume @DEFAULT_AUDIO_SINK@`: `Volume: 0.40 [MUTED]`.
#[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]
pub fn parse_wpctl(text: &str) -> Option<(f32, bool)> {
    let rest = text.trim().strip_prefix("Volume:")?.trim();
    let level: f32 = rest.split_whitespace().next()?.parse().ok()?;
    Some((level.clamp(0.0, 1.0), rest.contains("[MUTED]")))
}

fn run(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(neo_desktop::fs::tool(program)).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// How loud things are now. `None` where the system cannot be asked.
pub fn read() -> Option<Levels> {
    if cfg!(target_os = "macos") {
        return run("osascript", &["-e", "get volume settings"]).and_then(|t| parse_mac(&t));
    }
    if cfg!(unix) {
        let (output, muted) = run("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"]).and_then(|t| parse_wpctl(&t))?;
        let input = run("wpctl", &["get-volume", "@DEFAULT_AUDIO_SOURCE@"]).and_then(|t| parse_wpctl(&t)).map(|(level, _)| level);
        return Some(Levels { output, muted, input });
    }
    None
}

/// Something sound comes out of or goes into: speakers, headphones, a
/// microphone, a display.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    /// What the system knows it by.
    pub id: u32,
    pub name: String,
    /// Sound goes into the computer through it.
    pub input: bool,
    /// Sound comes out of the computer through it.
    pub output: bool,
    /// It is the one used for sound in, unless an app chooses another.
    pub default_input: bool,
    pub default_output: bool,
}

/// Reads the helper's lines: number, takes sound in, gives sound out,
/// default for in, default for out, and name, with tabs between.
pub fn parse_devices(text: &str) -> Vec<Device> {
    text.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.splitn(6, '\t').collect();
            let [id, takes, gives, default_in, default_out, name] = fields.as_slice() else { return None };
            let yes = |f: &str| f == "1";
            Some(Device { id: id.parse().ok()?, name: (*name).to_owned(), input: yes(takes), output: yes(gives), default_input: yes(default_in), default_output: yes(default_out) }).filter(|d| !d.name.is_empty() && (d.input || d.output))
        })
        .collect()
}

/// Reads `wpctl status`: the sinks and sources it lists under Audio, the
/// default of each marked with a star.
#[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]
pub fn parse_wpctl_status(text: &str) -> Vec<Device> {
    let (mut out, mut audio, mut part): (Vec<Device>, bool, Option<bool>) = (vec![], false, None);
    for line in text.lines() {
        let plain = line.trim_start_matches([' ', '│', '├', '└', '─']).trim();
        if !line.starts_with([' ', '│', '├', '└']) && !plain.is_empty() {
            audio = plain == "Audio";
            part = None;
        } else if plain.ends_with(':') {
            // "Sinks:" give sound out; "Sources:" take it in; the rest are not devices to choose.
            part = match plain {
                "Sinks:" => Some(false),
                "Sources:" => Some(true),
                _ => None,
            };
        } else if let (true, Some(input)) = (audio, part) {
            let default = plain.starts_with('*');
            let Some((id, rest)) = plain.trim_start_matches('*').trim().split_once('.') else { continue };
            let Ok(id) = id.trim().parse::<u32>() else { continue };
            // The name ends where its volume begins.
            let name = rest.split(" [vol:").next().unwrap_or(rest).trim().to_owned();
            if !name.is_empty() {
                out.push(Device { id, name, input, output: !input, default_input: input && default, default_output: !input && default });
            }
        }
    }
    out
}

/// The devices there are, where the system says.
pub fn devices() -> Vec<Device> {
    imp::devices()
}

/// Makes a device the one used for sound in, or for sound out. False if
/// the system would not.
pub fn set_default(id: u32, input: bool) -> bool {
    imp::set_default(id, input)
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;

    unsafe extern "C" {
        fn neo_audio_devices(out: *mut std::ffi::c_char, length: std::ffi::c_int) -> std::ffi::c_int;
        fn neo_audio_set_default(device: std::ffi::c_uint, input: std::ffi::c_int) -> std::ffi::c_int;
    }

    pub fn devices() -> Vec<Device> {
        let mut buf = vec![0u8; 16 * 1024];
        // SAFETY: the buffer is as long as is said, and what is written to it ends with a zero.
        unsafe { neo_audio_devices(buf.as_mut_ptr().cast(), buf.len() as std::ffi::c_int) };
        let end = buf.iter().position(|b| *b == 0).unwrap_or(0);
        parse_devices(&String::from_utf8_lossy(&buf[..end]))
    }

    pub fn set_default(id: u32, input: bool) -> bool {
        // SAFETY: a plain call with two numbers.
        unsafe { neo_audio_set_default(id, i32::from(input)) != 0 }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use super::*;

    pub fn devices() -> Vec<Device> {
        run("wpctl", &["status"]).map(|t| parse_wpctl_status(&t)).unwrap_or_default()
    }

    pub fn set_default(id: u32, _input: bool) -> bool {
        run("wpctl", &["set-default", &id.to_string()]).is_some()
    }
}

#[cfg(not(unix))]
mod imp {
    use super::*;

    pub fn devices() -> Vec<Device> {
        vec![]
    }

    pub fn set_default(_id: u32, _input: bool) -> bool {
        false
    }
}

/// What to set, each to a level from 0 to 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Change {
    Output(f32),
    Muted(bool),
    Input(f32),
}

/// The command that makes a change on this system: the program, and what
/// it is given.
pub fn command(change: Change) -> Option<(&'static str, Vec<String>)> {
    let percent = |level: f32| (level.clamp(0.0, 1.0) * 100.0).round() as u32;
    if cfg!(target_os = "macos") {
        let script = match change {
            Change::Output(level) => format!("set volume output volume {}", percent(level)),
            Change::Muted(muted) => format!("set volume output muted {muted}"),
            Change::Input(level) => format!("set volume input volume {}", percent(level)),
        };
        return Some(("osascript", vec!["-e".into(), script]));
    }
    if cfg!(unix) {
        return Some((
            "wpctl",
            match change {
                Change::Output(level) => vec!["set-volume".into(), "@DEFAULT_AUDIO_SINK@".into(), format!("{:.2}", level.clamp(0.0, 1.0))],
                Change::Muted(muted) => vec!["set-mute".into(), "@DEFAULT_AUDIO_SINK@".into(), if muted { "1" } else { "0" }.into()],
                Change::Input(level) => vec!["set-volume".into(), "@DEFAULT_AUDIO_SOURCE@".into(), format!("{:.2}", level.clamp(0.0, 1.0))],
            },
        ));
    }
    None
}

/// Makes a change. False if the system would not, or cannot be asked.
pub fn set(change: Change) -> bool {
    command(change).is_some_and(|(program, args)| run(program, &args.iter().map(String::as_str).collect::<Vec<_>>()).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_the_system_says_of_loudness_is_read() {
        assert_eq!(parse_mac("output volume:50, input volume:75, alert volume:100, output muted:false\n"), Some(Levels { output: 0.5, muted: false, input: Some(0.75) }));
        assert_eq!(parse_mac("output volume:0, input volume:missing value, alert volume:100, output muted:true"), Some(Levels { output: 0.0, muted: true, input: None }));
        assert_eq!((parse_mac("output volume:missing value, output muted:false"), parse_mac("")), (None, None), "a device that sets its own loudness has none to show");
        assert_eq!((parse_wpctl("Volume: 0.40\n"), parse_wpctl("Volume: 1.25 [MUTED]"), parse_wpctl("nothing")), (Some((0.4, false)), Some((1.0, true)), None));
    }

    #[test]
    fn a_change_is_asked_for_in_this_systems_own_words() {
        let Some((program, args)) = command(Change::Output(0.426)) else { return };
        if cfg!(target_os = "macos") {
            assert_eq!((program, args.join(" ")), ("osascript", "-e set volume output volume 43".into()));
            assert_eq!(command(Change::Muted(true)).unwrap().1[1], "set volume output muted true");
            assert_eq!(command(Change::Input(2.0)).unwrap().1[1], "set volume input volume 100", "never past all the way up");
        } else {
            assert_eq!((program, args.join(" ")), ("wpctl", "set-volume @DEFAULT_AUDIO_SINK@ 0.43".into()));
            assert_eq!(command(Change::Muted(false)).unwrap().1.join(" "), "set-mute @DEFAULT_AUDIO_SINK@ 0");
        }
    }

    #[test]
    fn the_devices_there_are_are_read() {
        let text = "73\t0\t1\t0\t1\tMacBook Pro Speakers\n66\t1\t0\t1\t0\tMacBook Pro Microphone\n81\t1\t1\t0\t0\tStudio Display\twith a tab\n90\t0\t0\t0\t0\tNeither\nnonsense\n";
        let found = parse_devices(text);
        assert_eq!(found.len(), 3, "one that neither takes nor gives sound is not a device to choose");
        assert_eq!(found[0], Device { id: 73, name: "MacBook Pro Speakers".into(), input: false, output: true, default_input: false, default_output: true });
        assert!(found[1].input && found[1].default_input && !found[1].output);
        assert_eq!((found[2].name.as_str(), found[2].input, found[2].output), ("Studio Display\twith a tab", true, true));
        let status = "PipeWire 'pipewire-0' [1.0.5]\n └─ Clients:\n        31. pipewire\n\nAudio\n ├─ Devices:\n │      44. Built-in Audio                      [alsa]\n │  \n ├─ Sinks:\n │  *   52. Built-in Audio Analog Stereo        [vol: 0.40]\n │      60. HDMI Output                         [vol: 1.00]\n │  \n ├─ Sources:\n │  *   53. Built-in Audio Analog Stereo        [vol: 0.65]\n │  \n └─ Streams:\n        70. Firefox\n\nVideo\n ├─ Sources:\n │  *   80. Webcam\n";
        let found = parse_wpctl_status(status);
        assert_eq!(found.iter().map(|d| (d.id, d.name.as_str(), d.input, d.default_input || d.default_output)).collect::<Vec<_>>(), [(52, "Built-in Audio Analog Stereo", false, true), (60, "HDMI Output", false, false), (53, "Built-in Audio Analog Stereo", true, true)], "the sinks and the sources under Audio, and not the camera");
        assert_eq!(parse_wpctl_status(""), vec![]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn this_mac_says_which_devices_it_has() {
        let found = devices();
        assert!(found.iter().any(|d| d.output), "something to play through: {found:?}");
        assert!(found.iter().filter(|d| d.default_output).count() <= 1 && found.iter().filter(|d| d.default_input).count() <= 1);
        // Making the default the default changes nothing, and shows it can be set.
        if let Some(now) = found.iter().find(|d| d.default_output) {
            assert!(set_default(now.id, false));
            assert_eq!(devices().iter().find(|d| d.default_output).map(|d| d.id), Some(now.id));
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn this_mac_says_how_loud_it_is() {
        // Without changing it.
        assert!(read().is_none_or(|l| (0.0..=1.0).contains(&l.output)));
    }
}
