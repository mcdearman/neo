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

    #[cfg(target_os = "macos")]
    #[test]
    fn this_mac_says_how_loud_it_is() {
        // Without changing it.
        assert!(read().is_none_or(|l| (0.0..=1.0).contains(&l.output)));
    }
}
