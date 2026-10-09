//! The trackpad and the mouse: which way each scrolls.
//!
//! Scrolling is "reversed" when what is on screen moves the way the
//! fingers or the wheel do, as paper under them would, which is how a
//! trackpad is usually wanted and a wheel usually is not. The two are set
//! apart here: `[trackpad]` and `[mouse]` in `neo.toml`, each with
//! `reverse-scrolling`.
//!
//! macOS has one setting for both, so there NeoShell turns round the
//! scrolling of whichever is to differ from it, for every app, which it
//! needs the same leave for as tiling does. On a GNOME desktop the two are
//! the desktop's own settings, and are set through it.

use std::path::PathBuf;
use std::process::Command;

use crate::config::File;

/// Which way the trackpad and the mouse scroll.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scrolling {
    pub trackpad_reversed: bool,
    pub mouse_reversed: bool,
}

impl Default for Scrolling {
    /// A trackpad moves what is under the fingers; a wheel does not.
    fn default() -> Self {
        Self { trackpad_reversed: true, mouse_reversed: false }
    }
}

impl Scrolling {
    pub fn load() -> Self {
        Self::read(&File::desktop())
    }

    pub fn read(file: &File) -> Self {
        let d = Self::default();
        Self { trackpad_reversed: file.flag(&["trackpad", "reverse-scrolling"]).unwrap_or(d.trackpad_reversed), mouse_reversed: file.flag(&["mouse", "reverse-scrolling"]).unwrap_or(d.mouse_reversed) }
    }

    pub fn write(&self, file: &mut File) {
        file.set(&["trackpad", "reverse-scrolling"], self.trackpad_reversed);
        file.set(&["mouse", "reverse-scrolling"], self.mouse_reversed);
    }

    pub fn save(&self) -> std::io::Result<()> {
        let mut file = File::desktop();
        self.write(&mut file);
        file.save()
    }

    /// Which of the two has to be turned round, where the system scrolls
    /// both one way: the trackpad, and the mouse. `system_reversed` is the
    /// way the system has them.
    pub fn turn(&self, system_reversed: bool) -> (bool, bool) {
        (self.trackpad_reversed != system_reversed, self.mouse_reversed != system_reversed)
    }
}

/// Whether this computer has a trackpad, as far as can be told.
pub fn has_trackpad() -> bool {
    if cfg!(target_os = "macos") {
        // Its own, or one beside it: both are multitouch devices that are not a mouse.
        let said = |class: &str| Command::new("/usr/sbin/ioreg").args(["-r", "-c", class, "-d", "1"]).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
        trackpad_in(&said("AppleMultitouchDevice")) || trackpad_in(&said("BNBTrackpadDevice"))
    } else {
        std::fs::read_to_string("/proc/bus/input/devices").is_ok_and(|d| d.lines().any(|l| l.starts_with("N: Name=") && (l.contains("Touchpad") || l.contains("TouchPad") || l.contains("Trackpad"))))
    }
}

/// Whether what `ioreg` says of the multitouch devices names a trackpad:
/// a Magic Mouse is one of them too, and is a mouse.
fn trackpad_in(ioreg: &str) -> bool {
    ioreg.lines().any(|l| l.contains("\"Product\"") && l.contains("Trackpad")) || ioreg.contains("class BNBTrackpadDevice")
}

/// Whether the system scrolls the way the fingers move, on macOS, where
/// one setting stands for the trackpad and the mouse both. It does unless
/// it has been turned off.
pub fn system_reversed() -> bool {
    Command::new("/usr/bin/defaults").args(["read", "-g", "com.apple.swipescrolldirection"]).output().map(|o| String::from_utf8_lossy(&o.stdout).trim() != "0").unwrap_or(true)
}

/// Sets the desktop's own settings to these, where it has them: GNOME's,
/// through `gsettings`. Elsewhere it is NeoShell's to do, or nobody's.
pub fn apply(s: Scrolling) {
    if cfg!(all(unix, not(target_os = "macos"))) {
        for (device, reversed) in [("touchpad", s.trackpad_reversed), ("mouse", s.mouse_reversed)] {
            let _ = Command::new("gsettings").args(["set", &format!("org.gnome.desktop.peripherals.{device}"), "natural-scroll", if reversed { "true" } else { "false" }]).output();
        }
    }
}

/// What NeoShell says of turning the scrolling round, for Settings to show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// Something is to be turned round: the system does not already scroll both as wanted.
    pub needed: bool,
    /// And it is being: NeoShell has leave to, and is doing it.
    pub working: bool,
}

impl Status {
    pub fn path() -> PathBuf {
        crate::config_dir().join("scrolling.status")
    }

    pub fn load() -> Option<Self> {
        let text = std::fs::read_to_string(Self::path()).ok()?;
        let said = |key: &str| text.lines().any(|l| l.trim() == format!("{key} = true"));
        Some(Self { needed: said("needed"), working: said("working") })
    }

    pub fn save(&self) -> std::io::Result<()> {
        std::fs::write(Self::path(), format!("needed = {}\nworking = {}\n", self.needed, self.working))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trackpad_follows_the_fingers_and_a_wheel_does_not_until_set_otherwise() {
        let d = Scrolling::default();
        assert_eq!((d.trackpad_reversed, d.mouse_reversed), (true, false));
        assert_eq!(Scrolling::read(&File::parse("")), d);
        assert_eq!(Scrolling::read(&File::parse("[trackpad]\nreverse-scrolling = false\n[mouse]\nreverse-scrolling = true\n")), Scrolling { trackpad_reversed: false, mouse_reversed: true });
        assert_eq!(Scrolling::read(&File::parse("[mouse]\nreverse-scrolling = \"yes\"\n")), d, "what makes no sense keeps the default");
        let mut file = File::parse("# mine\n[mouse]\nspeed = 3 # kept\n");
        Scrolling { trackpad_reversed: true, mouse_reversed: true }.write(&mut file);
        assert_eq!(file.encode(), "# mine\n[mouse]\nspeed = 3 # kept\nreverse-scrolling = true\n\n[trackpad]\nreverse-scrolling = true\n");
    }

    #[test]
    fn only_what_differs_from_the_system_is_turned_round() {
        let d = Scrolling::default();
        // The system follows the fingers for both: the mouse is the one to turn.
        assert_eq!(d.turn(true), (false, true));
        // It follows neither: the trackpad is.
        assert_eq!(d.turn(false), (true, false));
        assert_eq!(Scrolling { trackpad_reversed: true, mouse_reversed: true }.turn(true), (false, false), "as the system has them: nothing to do");
        assert_eq!(Scrolling { trackpad_reversed: false, mouse_reversed: false }.turn(true), (true, true));
    }

    #[test]
    fn a_trackpad_is_told_from_a_mouse_that_is_also_touched() {
        assert!(trackpad_in("+-o AppleMultitouchDevice  <class AppleMultitouchDevice, id 0x1>\n      \"Product\" = \"Apple Internal Keyboard / Trackpad\"\n"));
        assert!(trackpad_in("+-o BNBTrackpadDevice  <class BNBTrackpadDevice, id 0x2>\n      \"Product\" = \"Magic Trackpad\"\n"));
        assert!(!trackpad_in("+-o AppleMultitouchDevice  <class AppleMultitouchDevice, id 0x3>\n      \"Product\" = \"Magic Mouse\"\n"));
        assert!(!trackpad_in(""));
    }
}
