//! Which way the trackpad and the mouse scroll, where the system has one
//! setting for both and the user wants two: see `neo_desktop::pointing`.
//!
//! On macOS NeoShell turns round the scrolling of whichever is to differ
//! from the system's, for every app, by changing the events as they pass.
//! Elsewhere the desktop has the two settings itself, and this does nothing.

use std::time::Duration;

use neo_desktop::pointing::{Scrolling, Status};

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn neo_scroll_set(trackpad: std::ffi::c_int, mouse: std::ffi::c_int);
    fn neo_scroll_run() -> std::ffi::c_int;
    fn neo_scroll_working() -> std::ffi::c_int;
}

/// How often the settings, and the system's own, are looked at again.
const LOOK: Duration = Duration::from_millis(1500);

/// Keeps the scrolling as the settings say for as long as NeoShell runs.
#[cfg(target_os = "macos")]
pub fn run() {
    let (mut said, mut watching, mut tried) = (None, false, None::<std::time::Instant>);
    loop {
        let (trackpad, mouse) = Scrolling::load().turn(neo_desktop::pointing::system_reversed());
        // SAFETY: plain calls with numbers.
        unsafe { neo_scroll_set(i32::from(trackpad), i32::from(mouse)) };
        let needed = trackpad || mouse;
        // Watching starts when there is first something to turn round, so that
        // leave is not asked for by someone who wants what the system does.
        // Without leave it is tried again now and then, in case it has been given.
        if needed && !watching && tried.is_none_or(|t| t.elapsed() >= Duration::from_secs(5)) {
            tried = Some(std::time::Instant::now());
            let (started, heard) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                // Returns only if it could not begin.
                let _ = started.send(());
                unsafe { neo_scroll_run() };
            });
            let _ = heard.recv();
            std::thread::sleep(Duration::from_millis(200));
            watching = unsafe { neo_scroll_working() } != 0;
        }
        let status = Status { needed, working: needed && watching && unsafe { neo_scroll_working() } != 0 };
        if said != Some(status) {
            let _ = status.save();
            said = Some(status);
        }
        std::thread::sleep(LOOK);
    }
}

/// The desktop's own settings do this here.
#[cfg(not(target_os = "macos"))]
pub fn run() {
    let _ = (LOOK, Status { needed: false, working: false }.save(), Scrolling::default());
}
