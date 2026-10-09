//! Choosing a colour: by eye from a plane of shades, by its hex code, or
//! straight off the screen with an eyedropper. The plane and the codes
//! are Neo's own, for any app; the eyedropper is the system's.

use neo::prelude::*;

pub use neo::widgets::{hex, hues, parse_hex, shades, Hsv, Plane};

/// Whether a colour can be picked off the screen on this system.
pub fn can_sample() -> bool {
    imp::can_sample()
}

/// Brings up the eyedropper, and gives `picked` the colour clicked on,
/// or nothing if it was dismissed. To be called on the main thread.
pub fn sample(picked: impl Fn(Option<Color>) + Send + Sync + 'static) {
    imp::sample(Box::new(picked));
}

type Picked = Box<dyn Fn(Option<Color>) + Send + Sync>;

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use std::sync::Mutex;

    /// Who to tell of the colour the eyedropper comes back with.
    static WAITING: Mutex<Option<Picked>> = Mutex::new(None);

    unsafe extern "C" {
        fn neo_sample_color(callback: extern "C" fn(std::ffi::c_int, f64, f64, f64));
    }

    extern "C" fn sampled(ok: std::ffi::c_int, r: f64, g: f64, b: f64) {
        if let Some(tell) = WAITING.lock().ok().and_then(|mut w| w.take()) {
            tell((ok != 0).then(|| Color::rgb(r as f32, g as f32, b as f32)));
        }
    }

    pub fn can_sample() -> bool {
        true
    }

    pub fn sample(picked: Picked) {
        if let Ok(mut waiting) = WAITING.lock() {
            *waiting = Some(picked);
        }
        // SAFETY: called on the main thread, with a function that lives as long as the program.
        unsafe { neo_sample_color(sampled) };
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::*;

    /// The pickers there are on Linux desktops: each prints the colour's code.
    const TOOLS: [(&str, &[&str]); 2] = [("hyprpicker", &["-n"]), ("xcolor", &[])];

    fn tool() -> Option<(std::path::PathBuf, &'static [&'static str])> {
        TOOLS.iter().map(|(name, args)| (neo_desktop::fs::tool(name), *args)).find(|(path, _)| path.is_file())
    }

    pub fn can_sample() -> bool {
        tool().is_some()
    }

    pub fn sample(picked: Picked) {
        let Some((program, args)) = tool() else { return picked(None) };
        std::thread::spawn(move || {
            let out = std::process::Command::new(program).args(args).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output();
            picked(out.ok().filter(|o| o.status.success()).and_then(|o| parse_hex(String::from_utf8_lossy(&o.stdout).trim())));
        });
    }
}
