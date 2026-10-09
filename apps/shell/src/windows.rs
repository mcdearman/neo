//! Tiling: laying other apps' windows out to share the screen.
//!
//! The layouts and the settings are `neo_desktop::tiling`'s. This is what
//! follows the windows as they come and go and puts each where its
//! layout says: a dynamic tiling window manager, in the small.
//!
//! - The windows on each screen are kept in an order: the first is the
//!   main one. A window that opens joins at the front or the back, as
//!   the settings say; one that closes leaves, and the rest close up.
//! - Dragged onto another tiled window and let go, a window changes
//!   places with it. Dragged anywhere else, it goes back to its place.
//! - A window can be let out of the tiling to float, and put back.
//!
//! On macOS the windows are found and moved through the accessibility
//! interface, which the user has to allow. Elsewhere the desktop's own
//! window manager places windows, and this does nothing.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

use neo::{Point, Rect};
use neo_desktop::tiling::{MOST_MASTERS, Mode, NewWindow, RATIOS, Status, Windowing};

/// A window that can be tiled.
#[derive(Clone, Debug, PartialEq)]
pub struct Win {
    /// The system's number for it.
    pub id: u32,
    pub pid: i32,
    /// The name of the app it belongs to.
    pub app: String,
    /// Where it is, measured down from the top left of the main screen.
    pub frame: Rect,
}

/// What finds the windows and moves them.
pub trait Backend {
    /// Whether windows can be tiled on this system at all.
    fn possible(&self) -> bool;
    /// Whether leave has been given to move other apps' windows. With
    /// `ask`, the system is to ask the user for it if not.
    fn allowed(&self, ask: bool) -> bool;
    /// The part of each screen windows may use.
    fn screens(&self) -> Vec<Rect>;
    /// The windows on screen that can be tiled, the frontmost first.
    fn windows(&self) -> Vec<Win>;
    fn place(&self, window: &Win, frame: Rect);
    fn focus(&self, window: &Win);
    /// The window that has the keyboard.
    fn focused(&self) -> Option<u32>;
    /// Whether the mouse's button is down, as while a window is dragged.
    fn button_down(&self) -> bool;
}

/// Something asked of the tiling from the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Give the keyboard to the next window in the order, or the one before.
    FocusNext,
    FocusPrevious,
    /// Make the window that has the keyboard the main one; or, if it is, the next one.
    Promote,
    /// Give the main windows more of the screen, or less.
    Grow,
    Shrink,
    MoreMasters,
    FewerMasters,
    /// Go on to the next layout.
    NextLayout,
    /// Let the window that has the keyboard out of the tiling, or put it back.
    ToggleFloat,
    /// Put every window back where its layout says.
    Retile,
}

/// How near a window must be to where it was put to count as there, in points.
const NEAR: f32 = 3.0;

fn near(a: Rect, b: Rect) -> bool {
    (a.x - b.x).abs() <= NEAR && (a.y - b.y).abs() <= NEAR && (a.w - b.w).abs() <= NEAR && (a.h - b.h).abs() <= NEAR
}

fn middle(r: Rect) -> Point {
    Point::new(r.x + r.w / 2.0, r.y + r.h / 2.0)
}

pub struct Manager<B> {
    backend: B,
    pub config: Windowing,
    /// The tiled windows, in their order: the first on each screen is its main one.
    order: Vec<u32>,
    /// Windows the user has let out of the tiling.
    floating: Vec<u32>,
    /// Where each tiled window was last asked to be.
    asked: HashMap<u32, Rect>,
    /// Where each came to rest after being asked, which for an app that
    /// will not be made as small as asked is not where it was asked to be.
    rested: HashMap<u32, Rect>,
    /// The windows as last seen.
    seen: Vec<Win>,
    /// Leave has been asked for since tiling was turned on, so as not to ask over and over.
    asked_leave: bool,
    status: Status,
    /// Where each window was before it was tiled, with its process, to
    /// put it back when tiling is turned off; and the file that is kept
    /// in, so that it is still known after NeoShell is started again.
    before: HashMap<u32, (i32, Rect)>,
    kept: Option<PathBuf>,
}

impl<B: Backend> Manager<B> {
    pub fn new(backend: B, config: Windowing) -> Self {
        Self { backend, config, order: vec![], floating: vec![], asked: HashMap::new(), rested: HashMap::new(), seen: vec![], asked_leave: false, status: Status::default(), before: HashMap::new(), kept: None }
    }

    /// Keeps where windows were before tiling in a file, and takes up
    /// what an earlier run left there.
    pub fn keeping(mut self, path: PathBuf) -> Self {
        self.before = std::fs::read_to_string(&path).map(|text| parse_windows(&text).into_iter().map(|w| (w.id, (w.pid, w.frame))).collect()).unwrap_or_default();
        self.kept = Some(path);
        self
    }

    fn keep(&self) {
        let Some(path) = &self.kept else { return };
        if self.before.is_empty() {
            let _ = std::fs::remove_file(path);
            return;
        }
        // As the system's own lines of windows are written, less the name.
        let lines: String = self.before.iter().map(|(id, (pid, r))| format!("{id}\t{pid}\t{}\t{}\t{}\t{}\t\n", r.x, r.y, r.w, r.h)).collect();
        let _ = std::fs::write(path, lines);
    }

    /// Puts the windows back where they were before they were tiled:
    /// those that are still there, on the screen now. Then it is forgotten.
    fn put_back(&mut self) {
        for w in self.backend.windows() {
            if let Some((_, was)) = self.before.get(&w.id).filter(|(pid, was)| *pid == w.pid && !near(*was, w.frame)) {
                self.backend.place(&w, *was);
            }
        }
        self.before.clear();
        self.keep();
    }

    pub fn tiling(&self) -> bool {
        self.config.mode == Mode::Tiling
    }

    /// Has the system ask the user to let windows be moved, if they have
    /// not. Asked for from Settings. Returns whether they have.
    pub fn ask(&mut self) -> bool {
        self.asked_leave = true;
        self.backend.possible() && self.backend.allowed(true)
    }

    /// How it is going, for Settings to show.
    pub fn status(&self) -> &Status {
        &self.status
    }

    /// Takes up new settings. What changes the layout lays the windows out again.
    pub fn configure(&mut self, config: Windowing) {
        if config == self.config {
            return;
        }
        let was_tiling = self.tiling();
        self.config = config;
        if self.tiling() && !was_tiling {
            // Begun afresh: the windows as they stand now, in the order they are in.
            (self.order, self.asked_leave) = (vec![], false);
            self.floating.clear();
        }
        if !self.tiling() {
            self.forget();
        }
        self.tick(true);
    }

    fn forget(&mut self) {
        self.order.clear();
        self.asked.clear();
        self.rested.clear();
    }

    /// Whether a window is tiled: not one of an app that floats, and not
    /// one the user has let out.
    fn tiled(&self, w: &Win) -> bool {
        !self.config.floats(&w.app) && !self.floating.contains(&w.id)
    }

    /// The screen a window is on: the one its middle is in, or the nearest.
    fn screen_of(screens: &[Rect], frame: Rect) -> usize {
        let at = middle(frame);
        screens.iter().position(|s| s.contains(at)).unwrap_or_else(|| {
            let far = |s: &Rect| {
                let m = middle(*s);
                (m.x - at.x).powi(2) + (m.y - at.y).powi(2)
            };
            (0..screens.len()).min_by(|a, b| far(&screens[*a]).total_cmp(&far(&screens[*b]))).unwrap_or(0)
        })
    }

    /// Looks at the windows there are, and lays them out if anything has
    /// changed: one has opened or closed, one has been dragged, or
    /// `all` says to whatever has. Returns whether any window was moved.
    pub fn tick(&mut self, all: bool) -> bool {
        let possible = self.backend.possible();
        if !self.tiling() || !possible {
            let allowed = possible && self.backend.allowed(false);
            self.status = Status { allowed, windows: 0, possible };
            // Tiling has been turned off, now or while this was not running:
            // the windows go back where they were.
            if allowed && !self.tiling() && !self.before.is_empty() {
                self.put_back();
                return true;
            }
            return false;
        }
        // Asked for once when tiling is turned on; after that it is the user's to give.
        let allowed = self.backend.allowed(!self.asked_leave);
        self.asked_leave = true;
        if !allowed {
            self.status = Status { allowed: false, windows: 0, possible };
            self.forget();
            return false;
        }
        let windows = self.backend.windows();
        let screens = self.backend.screens();
        if screens.is_empty() {
            return false;
        }
        // Those that have gone leave the order, and those that are new join it.
        let before = self.order.clone();
        let tiled: Vec<u32> = windows.iter().filter(|w| self.tiled(w)).map(|w| w.id).collect();
        self.order.retain(|id| tiled.contains(id));
        self.floating.retain(|id| windows.iter().any(|w| w.id == *id));
        let fresh: Vec<u32> = windows.iter().filter(|w| self.tiled(w) && !self.order.contains(&w.id)).map(|w| w.id).collect();
        // Where each was before it is moved, to be put back there. One seen
        // before is one coming back from another desktop, and keeps what it had.
        let known = self.before.len();
        for w in windows.iter().filter(|w| fresh.contains(&w.id)) {
            if self.before.get(&w.id).is_none_or(|(pid, _)| *pid != w.pid) {
                self.before.insert(w.id, (w.pid, w.frame));
            }
        }
        if self.before.len() != known {
            self.keep();
        }
        match self.config.new_window {
            // The frontmost of several ends up first.
            NewWindow::Master => fresh.iter().rev().for_each(|id| self.order.insert(0, *id)),
            NewWindow::Stack => self.order.extend(fresh),
        }
        let mut changed = all || self.order != before;
        // A window dragged and let go: onto another, it changes places with
        // it; anywhere else, it goes back. While the button is down it is
        // left alone, or it would be pulled from under the pointer.
        if !changed && !self.backend.button_down() {
            let moved = windows.iter().find(|w| self.order.contains(&w.id) && self.rested.get(&w.id).is_some_and(|r| !near(*r, w.frame)));
            if let Some(moved) = moved {
                let onto = self.order.iter().copied().find(|id| *id != moved.id && self.asked.get(id).is_some_and(|r| r.contains(middle(moved.frame))));
                if let (Some(onto), Some(a)) = (onto, self.order.iter().position(|id| *id == moved.id)) {
                    let b = self.order.iter().position(|id| *id == onto).unwrap_or(a);
                    self.order.swap(a, b);
                }
                changed = true;
            }
        }
        // Where a window came to rest after it was last asked to move.
        for w in &windows {
            if self.asked.contains_key(&w.id) && !self.rested.contains_key(&w.id) {
                self.rested.insert(w.id, w.frame);
            }
        }
        self.status = Status { allowed: true, windows: self.order.len() as u32, possible };
        self.seen = windows;
        if changed {
            self.lay_out(&screens);
        }
        changed
    }

    /// Puts each tiled window where the layout says, screen by screen.
    fn lay_out(&mut self, screens: &[Rect]) {
        for (n, screen) in screens.iter().enumerate() {
            let here: Vec<&Win> = self.order.iter().filter_map(|id| self.seen.iter().find(|w| w.id == *id)).filter(|w| Self::screen_of(screens, w.frame) == n).collect();
            let places = self.config.arrange(here.len(), *screen);
            for (w, place) in here.into_iter().zip(places) {
                // One that will not be made the size asked is not asked again and again.
                let stubborn = self.asked.get(&w.id) == Some(&place) && self.rested.get(&w.id).is_some_and(|r| near(*r, w.frame));
                if !near(w.frame, place) && !stubborn {
                    self.backend.place(w, place);
                    self.rested.remove(&w.id);
                } else {
                    // Where it is, is where it stays: a move from here is the user's.
                    self.rested.insert(w.id, w.frame);
                }
                self.asked.insert(w.id, place);
            }
        }
        self.asked.retain(|id, _| self.order.contains(id));
        self.rested.retain(|id, _| self.order.contains(id));
    }

    /// Does something asked from the keyboard. Returns the settings if it
    /// changed them, to be kept.
    pub fn act(&mut self, action: Action) -> Option<Windowing> {
        if !self.tiling() || self.order.is_empty() && action != Action::ToggleFloat {
            return None;
        }
        let focused = self.backend.focused();
        let at = focused.and_then(|id| self.order.iter().position(|o| *o == id));
        let before = self.config.clone();
        match action {
            Action::FocusNext | Action::FocusPrevious => {
                let n = self.order.len();
                let to = match (at, action) {
                    (Some(i), Action::FocusNext) => (i + 1) % n,
                    (Some(i), _) => (i + n - 1) % n,
                    (None, _) => 0,
                };
                if let Some(w) = self.seen.iter().find(|w| w.id == self.order[to]) {
                    self.backend.focus(w);
                }
            }
            Action::Promote => {
                // The main window promoted changes places with the one after it.
                let from = at.unwrap_or(0);
                let with = if from == 0 { 1.min(self.order.len() - 1) } else { 0 };
                self.order.swap(from, with);
                self.tick(true);
            }
            Action::Grow => self.config.ratio = (self.config.ratio + 0.05).min(RATIOS.1),
            Action::Shrink => self.config.ratio = (self.config.ratio - 0.05).max(RATIOS.0),
            Action::MoreMasters => self.config.masters = (self.config.masters + 1).min(MOST_MASTERS),
            Action::FewerMasters => self.config.masters = self.config.masters.saturating_sub(1).max(1),
            Action::NextLayout => self.config.layout = self.config.layout.next(),
            Action::ToggleFloat => {
                if let Some(id) = focused {
                    match self.floating.iter().position(|f| *f == id) {
                        Some(i) => drop(self.floating.remove(i)),
                        None => {
                            self.floating.push(id);
                            // Let out, it goes back where it was, and is the user's to place from there.
                            if let (Some((_, was)), Some(w)) = (self.before.remove(&id), self.seen.iter().find(|w| w.id == id)) {
                                self.backend.place(w, was);
                                self.keep();
                            }
                        }
                    }
                    self.tick(true);
                }
            }
            Action::Retile => {
                // Whatever would not move before is asked again.
                self.rested.clear();
                self.asked.clear();
                self.tick(true);
            }
        }
        if self.config != before {
            // The ratio is rounded as it is written, so it comes back the same.
            self.config.ratio = (self.config.ratio * 100.0).round() / 100.0;
            self.tick(true);
            return Some(self.config.clone());
        }
        None
    }

    #[cfg(test)]
    pub fn order(&self) -> &[u32] {
        &self.order
    }
}

/// The system's own windows, on macOS.
pub struct System;

#[cfg(target_os = "macos")]
mod imp {
    use super::*;

    unsafe extern "C" {
        fn neo_wm_allowed(ask: std::ffi::c_int) -> std::ffi::c_int;
        fn neo_wm_screens(out: *mut std::ffi::c_char, length: std::ffi::c_int) -> std::ffi::c_int;
        fn neo_wm_windows(out: *mut std::ffi::c_char, length: std::ffi::c_int) -> std::ffi::c_int;
        fn neo_wm_place(pid: std::ffi::c_int, number: std::ffi::c_uint, x: f64, y: f64, w: f64, h: f64) -> std::ffi::c_int;
        fn neo_wm_focus(pid: std::ffi::c_int, number: std::ffi::c_uint) -> std::ffi::c_int;
        fn neo_wm_focused() -> std::ffi::c_uint;
        fn neo_wm_button_down() -> std::ffi::c_int;
    }

    fn text(fill: unsafe extern "C" fn(*mut std::ffi::c_char, std::ffi::c_int) -> std::ffi::c_int) -> String {
        let mut buf = vec![0u8; 64 * 1024];
        // SAFETY: the buffer is as long as is said, and what is written to it ends with a zero.
        unsafe { fill(buf.as_mut_ptr().cast(), buf.len() as std::ffi::c_int) };
        let end = buf.iter().position(|b| *b == 0).unwrap_or(0);
        String::from_utf8_lossy(&buf[..end]).into_owned()
    }

    impl Backend for System {
        fn possible(&self) -> bool {
            true
        }

        fn allowed(&self, ask: bool) -> bool {
            // SAFETY: a plain call with a number.
            unsafe { neo_wm_allowed(i32::from(ask)) != 0 }
        }

        fn screens(&self) -> Vec<Rect> {
            parse_screens(&text(neo_wm_screens))
        }

        fn windows(&self) -> Vec<Win> {
            parse_windows(&text(neo_wm_windows))
        }

        fn place(&self, window: &Win, frame: Rect) {
            // SAFETY: plain calls with numbers.
            unsafe { neo_wm_place(window.pid, window.id, f64::from(frame.x), f64::from(frame.y), f64::from(frame.w), f64::from(frame.h)) };
        }

        fn focus(&self, window: &Win) {
            unsafe { neo_wm_focus(window.pid, window.id) };
        }

        fn focused(&self) -> Option<u32> {
            Some(unsafe { neo_wm_focused() }).filter(|id| *id != 0)
        }

        fn button_down(&self) -> bool {
            unsafe { neo_wm_button_down() != 0 }
        }
    }
}

#[cfg(not(target_os = "macos"))]
impl Backend for System {
    /// The desktop's own window manager places windows here.
    fn possible(&self) -> bool {
        false
    }

    fn allowed(&self, _ask: bool) -> bool {
        false
    }

    fn screens(&self) -> Vec<Rect> {
        vec![]
    }

    fn windows(&self) -> Vec<Win> {
        vec![]
    }

    fn place(&self, _window: &Win, _frame: Rect) {}

    fn focus(&self, _window: &Win) {}

    fn focused(&self) -> Option<u32> {
        None
    }

    fn button_down(&self) -> bool {
        false
    }
}

/// How often the windows are looked at while tiling, and how often the
/// settings are while not.
const LOOK: Duration = Duration::from_millis(350);
const IDLE: Duration = Duration::from_millis(1500);

fn written(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Keeps the windows tiled for as long as NeoShell runs, on a thread of
/// its own so that an app slow to answer holds up nothing else. The
/// settings are read again whenever Settings writes them, `actions` are
/// what the keys ask for, and `turned` is told when tiling goes on or off.
pub fn run(actions: Receiver<Action>, turned: impl Fn(bool)) {
    let mut manager = Manager::new(System, Windowing::load()).keeping(neo_desktop::config_dir().join("windowing.before"));
    let mut read = written(&Windowing::path());
    let (mut said, mut on): (Option<Status>, Option<bool>) = (None, None);
    loop {
        match actions.recv_timeout(if manager.tiling() { LOOK } else { IDLE }) {
            // A key that changed the settings: kept, so Settings shows the same.
            Ok(action) => {
                if let Some(config) = manager.act(action) {
                    let _ = config.save();
                    read = written(&Windowing::path());
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        // Settings has asked for leave to be asked for.
        if std::fs::remove_file(neo_desktop::tiling::ask_path()).is_ok() {
            manager.ask();
        }
        let now = written(&Windowing::path());
        if now != read {
            read = now;
            manager.configure(Windowing::load());
        }
        manager.tick(false);
        if said.as_ref() != Some(manager.status()) {
            let _ = manager.status().save();
            said = Some(manager.status().clone());
        }
        let tiling = manager.tiling() && manager.status().possible;
        if on != Some(tiling) {
            on = Some(tiling);
            turned(tiling);
        }
    }
}

/// Reads the helper's lines of screens: x, y, width and height.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn parse_screens(text: &str) -> Vec<Rect> {
    text.lines()
        .filter_map(|line| {
            let n: Vec<f32> = line.split('\t').filter_map(|f| f.parse().ok()).collect();
            (n.len() == 4 && n[2] > 0.0 && n[3] > 0.0).then(|| Rect::new(n[0], n[1], n[2], n[3]))
        })
        .collect()
}

/// Reads the helper's lines of windows: number, process, x, y, width,
/// height and the app's name.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn parse_windows(text: &str) -> Vec<Win> {
    text.lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.splitn(7, '\t').collect();
            let [id, pid, x, y, w, h, app] = f.as_slice() else { return None };
            Some(Win { id: id.parse().ok()?, pid: pid.parse().ok()?, app: (*app).to_owned(), frame: Rect::new(x.parse().ok()?, y.parse().ok()?, w.parse().ok()?, h.parse().ok()?) })
        })
        .collect()
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use neo_desktop::tiling::Layout;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// A desktop for tests: windows that go where they are put.
    #[derive(Default)]
    pub struct Desk {
        pub windows: Vec<Win>,
        pub screens: Vec<Rect>,
        pub allowed: bool,
        pub asked: u32,
        pub focused: Option<u32>,
        pub button_down: bool,
        /// How many times a window was moved.
        pub moves: u32,
        /// Windows that will not be made narrower than this.
        pub least_width: HashMap<u32, f32>,
    }

    #[derive(Clone, Default)]
    pub struct Fake(pub Rc<RefCell<Desk>>);

    impl Fake {
        pub fn open(&self, id: u32, app: &str) {
            // Opened in the middle of the first screen, at the front.
            self.0.borrow_mut().windows.insert(0, Win { id, pid: id as i32 + 100, app: app.into(), frame: Rect::new(300.0, 200.0, 600.0, 400.0) });
        }

        pub fn close(&self, id: u32) {
            self.0.borrow_mut().windows.retain(|w| w.id != id);
        }

        pub fn frame(&self, id: u32) -> Rect {
            self.0.borrow().windows.iter().find(|w| w.id == id).unwrap().frame
        }

        pub fn drag(&self, id: u32, to: Rect) {
            self.0.borrow_mut().windows.iter_mut().find(|w| w.id == id).unwrap().frame = to;
        }
    }

    impl Backend for Fake {
        fn possible(&self) -> bool {
            true
        }

        fn allowed(&self, ask: bool) -> bool {
            let mut d = self.0.borrow_mut();
            d.asked += u32::from(ask && !d.allowed);
            d.allowed
        }

        fn screens(&self) -> Vec<Rect> {
            self.0.borrow().screens.clone()
        }

        fn windows(&self) -> Vec<Win> {
            self.0.borrow().windows.clone()
        }

        fn place(&self, window: &Win, frame: Rect) {
            let mut d = self.0.borrow_mut();
            d.moves += 1;
            let least = d.least_width.get(&window.id).copied().unwrap_or(0.0);
            if let Some(w) = d.windows.iter_mut().find(|w| w.id == window.id) {
                w.frame = Rect::new(frame.x, frame.y, frame.w.max(least), frame.h);
            }
        }

        fn focus(&self, window: &Win) {
            self.0.borrow_mut().focused = Some(window.id);
        }

        fn focused(&self) -> Option<u32> {
            self.0.borrow().focused
        }

        fn button_down(&self) -> bool {
            self.0.borrow().button_down
        }
    }

    const SCREEN: Rect = Rect { x: 0.0, y: 25.0, w: 1440.0, h: 875.0 };

    fn tiling() -> Windowing {
        Windowing { mode: Mode::Tiling, gap: 10.0, margin: 10.0, ..Windowing::default() }
    }

    fn desk(apps: &[(u32, &str)]) -> (Manager<Fake>, Fake) {
        let fake = Fake::default();
        (fake.0.borrow_mut().screens, fake.0.borrow_mut().allowed) = (vec![SCREEN], true);
        // Opened one after another: the last is the frontmost.
        for (id, app) in apps {
            fake.open(*id, app);
        }
        (Manager::new(fake.clone(), tiling()), fake)
    }

    #[test]
    fn windows_share_the_screen_and_close_up_as_they_come_and_go() {
        let (mut m, desk) = desk(&[(1, "Notes"), (2, "Safari"), (3, "NeoTerm")]);
        assert!(m.tick(false), "laid out the first time it looks");
        assert_eq!(m.order(), [3, 2, 1], "as they stood, the frontmost the main one");
        let places = tiling().arrange(3, SCREEN);
        assert_eq!((desk.frame(3), desk.frame(2), desk.frame(1)), (places[0], places[1], places[2]));
        assert_eq!((m.status().windows, m.status().allowed), (3, true));
        // Nothing has changed: nothing is moved.
        let moves = desk.0.borrow().moves;
        assert!(!m.tick(false) && !m.tick(false));
        assert_eq!(desk.0.borrow().moves, moves);
        // One opens and goes to the end of the stack; the main window keeps its place.
        desk.open(4, "Files");
        assert!(m.tick(false));
        let places = tiling().arrange(4, SCREEN);
        assert_eq!((m.order(), desk.frame(3), desk.frame(4)), (&[3, 2, 1, 4][..], places[0], places[3]));
        // One closes and the rest close up.
        desk.close(2);
        assert!(m.tick(false));
        let places = tiling().arrange(3, SCREEN);
        assert_eq!((m.order(), desk.frame(1), desk.frame(4)), (&[3, 1, 4][..], places[1], places[2]));
        // Set so, a new window becomes the main one.
        m.configure(Windowing { new_window: NewWindow::Master, ..tiling() });
        desk.open(5, "Photos");
        m.tick(false);
        assert_eq!((m.order(), desk.frame(5)), (&[5, 3, 1, 4][..], tiling().arrange(4, SCREEN)[0]));
    }

    #[test]
    fn floating_leaves_windows_where_they_are() {
        let (mut m, desk) = desk(&[(1, "Notes"), (2, "Safari")]);
        m.configure(Windowing::default());
        assert!(!m.tick(false) && !m.tick(true));
        assert_eq!((desk.0.borrow().moves, desk.frame(1)), (0, Rect::new(300.0, 200.0, 600.0, 400.0)), "nothing is moved until tiling is asked for");
        // Turned on, they are laid out; and with it off, one that opens is left alone.
        m.configure(tiling());
        assert_ne!(desk.frame(1), Rect::new(300.0, 200.0, 600.0, 400.0));
        m.configure(Windowing::default());
        desk.open(3, "Files");
        m.tick(false);
        assert_eq!(desk.frame(3), Rect::new(300.0, 200.0, 600.0, 400.0));
        // An app set to float is left out, and so is a window let out by hand.
        m.configure(Windowing { floating: vec!["files".into()], ..tiling() });
        assert_eq!((m.order(), desk.frame(3)), (&[2, 1][..], Rect::new(300.0, 200.0, 600.0, 400.0)));
        desk.0.borrow_mut().focused = Some(2);
        m.act(Action::ToggleFloat);
        assert_eq!((m.order(), desk.frame(1)), (&[1][..], tiling().arrange(1, SCREEN)[0]), "the one left has the screen");
        m.act(Action::ToggleFloat);
        assert_eq!(m.order(), [1, 2], "put back, it joins as a new window does");
    }

    #[test]
    fn turning_tiling_off_puts_windows_back_where_they_were() {
        let (mut m, desk) = desk(&[(1, "Notes"), (2, "Safari"), (3, "NeoTerm")]);
        let were = [Rect::new(40.0, 60.0, 500.0, 300.0), Rect::new(700.0, 90.0, 640.0, 480.0), Rect::new(200.0, 400.0, 800.0, 450.0)];
        for (id, at) in [1, 2, 3].into_iter().zip(were) {
            desk.drag(id, at);
        }
        let file = std::env::temp_dir().join(format!("neo-shell-before-{}", std::process::id()));
        let _ = std::fs::remove_file(&file);
        m = m.keeping(file.clone());
        m.tick(false);
        assert!(file.exists() && [1, 2, 3].into_iter().zip(were).all(|(id, at)| desk.frame(id) != at), "tiled, and where they were is kept");
        // One opened while tiling goes back to where it opened; one closed is done without.
        desk.open(4, "Files");
        m.tick(false);
        desk.close(2);
        m.act(Action::NextLayout);
        m.configure(Windowing::default());
        assert_eq!((desk.frame(1), desk.frame(3), desk.frame(4)), (were[0], were[2], Rect::new(300.0, 200.0, 600.0, 400.0)));
        assert!(!file.exists(), "and then it is forgotten");
        // Moved about while floating, they stay where they are put; tiling again starts from there.
        desk.drag(1, Rect::new(10.0, 40.0, 300.0, 300.0));
        assert!(!m.tick(false));
        m.configure(tiling());
        m.configure(Windowing::default());
        assert_eq!(desk.frame(1), Rect::new(10.0, 40.0, 300.0, 300.0));

        // NeoShell started again while tiling still knows where they were,
        // and one started after tiling was turned off puts them back.
        m.configure(tiling());
        let again = Manager::new(desk.clone(), tiling()).keeping(file.clone());
        drop(again);
        let mut later = Manager::new(desk.clone(), Windowing::default()).keeping(file.clone());
        assert!(later.tick(false));
        assert_eq!((desk.frame(1), desk.frame(3), file.exists()), (Rect::new(10.0, 40.0, 300.0, 300.0), were[2], false));
        // A window let out by hand goes back at once, and is not moved again later.
        let mut m = Manager::new(desk.clone(), tiling()).keeping(file.clone());
        m.tick(false);
        desk.0.borrow_mut().focused = Some(3);
        m.act(Action::ToggleFloat);
        assert_eq!(desk.frame(3), were[2]);
        desk.drag(3, Rect::new(900.0, 500.0, 400.0, 300.0));
        m.configure(Windowing::default());
        assert_eq!((desk.frame(3), desk.frame(1)), (Rect::new(900.0, 500.0, 400.0, 300.0), Rect::new(10.0, 40.0, 300.0, 300.0)));
        // Without leave nothing can be put back, and it is kept until there is.
        m.configure(tiling());
        desk.0.borrow_mut().allowed = false;
        let moves = desk.0.borrow().moves;
        m.configure(Windowing::default());
        assert_eq!((desk.0.borrow().moves, file.exists()), (moves, true));
        desk.0.borrow_mut().allowed = true;
        assert!(m.tick(false) && !file.exists());
    }

    #[test]
    fn leave_is_asked_for_once_and_nothing_moves_without_it() {
        let (mut m, desk) = desk(&[(1, "Notes"), (2, "Safari")]);
        desk.0.borrow_mut().allowed = false;
        assert!(!m.tick(false) && !m.tick(false) && !m.tick(true));
        assert_eq!((desk.0.borrow().asked, desk.0.borrow().moves, m.status().allowed), (1, 0, false), "asked the once, and not again");
        // Given, the windows are laid out at the next look.
        desk.0.borrow_mut().allowed = true;
        assert!(m.tick(false));
        assert_eq!(m.status(), &Status { allowed: true, windows: 2, possible: true });
        // Asked for from Settings, it is asked again, tiling or not; and not once it is given.
        let (mut m, desk) = super::tests::desk(&[(1, "Notes")]);
        m.configure(Windowing::default());
        desk.0.borrow_mut().allowed = false;
        assert!(!m.ask() && !m.ask());
        assert_eq!(desk.0.borrow().asked, 2);
        desk.0.borrow_mut().allowed = true;
        assert!(m.ask());
        assert_eq!(desk.0.borrow().asked, 2);
    }

    #[test]
    fn a_window_dragged_onto_another_changes_places_and_anywhere_else_goes_back() {
        let (mut m, desk) = desk(&[(1, "Notes"), (2, "Safari"), (3, "NeoTerm")]);
        m.tick(false);
        m.tick(false);
        let places = tiling().arrange(3, SCREEN);
        // Being dragged: left alone while the button is down.
        desk.0.borrow_mut().button_down = true;
        desk.drag(1, Rect::new(100.0, 100.0, places[2].w, places[2].h));
        assert!(!m.tick(false));
        assert_eq!(desk.frame(1).x, 100.0);
        // Let go over the main window: they change places.
        desk.0.borrow_mut().button_down = false;
        assert!(m.tick(false));
        assert_eq!((m.order(), desk.frame(1), desk.frame(3)), (&[1, 2, 3][..], places[0], places[2]));
        // Let go over nothing: back it goes.
        m.tick(false);
        desk.drag(2, Rect::new(5000.0, 5000.0, 300.0, 300.0));
        m.tick(false);
        assert_eq!((m.order(), desk.frame(2)), (&[1, 2, 3][..], places[1]));
    }

    #[test]
    fn a_window_that_will_not_be_made_small_is_not_fought_with() {
        let (mut m, desk) = desk(&[(1, "Notes"), (2, "Wide")]);
        desk.0.borrow_mut().least_width.insert(1, 900.0);
        m.tick(false);
        assert_eq!(desk.frame(1).w, 900.0, "wider than the stack's share, and it stays so");
        let moves = desk.0.borrow().moves;
        for _ in 0..5 {
            m.tick(false);
        }
        assert_eq!(desk.0.borrow().moves, moves, "asked once, not at every look");
        // Asked to lay everything out again, it is tried once more.
        m.act(Action::Retile);
        assert_eq!(desk.0.borrow().moves, moves + 1);
    }

    #[test]
    fn the_keyboard_moves_between_windows_and_changes_the_layout() {
        let (mut m, desk) = desk(&[(1, "Notes"), (2, "Safari"), (3, "NeoTerm")]);
        m.tick(false);
        desk.0.borrow_mut().focused = Some(3);
        assert_eq!(m.act(Action::FocusNext), None);
        assert_eq!(desk.0.borrow().focused, Some(2));
        m.act(Action::FocusNext);
        m.act(Action::FocusNext);
        assert_eq!(desk.0.borrow().focused, Some(3), "round again to the first");
        m.act(Action::FocusPrevious);
        assert_eq!(desk.0.borrow().focused, Some(1));
        // The window with the keyboard becomes the main one; the main one promoted changes with the next.
        m.act(Action::Promote);
        assert_eq!((m.order(), desk.frame(1)), (&[1, 2, 3][..], tiling().arrange(3, SCREEN)[0]));
        m.act(Action::Promote);
        assert_eq!(m.order(), [2, 1, 3]);
        // More of the screen for the main window, and more of them: the settings come back to be kept.
        let grown = m.act(Action::Grow).expect("the settings changed");
        assert_eq!((grown.ratio, desk.frame(2).w), (0.6, grown.arrange(3, SCREEN)[0].w));
        for _ in 0..20 {
            m.act(Action::Grow);
        }
        assert_eq!(m.config.ratio, RATIOS.1, "no further than it may go");
        assert_eq!(m.act(Action::Grow), None, "and at the end nothing changes");
        assert_eq!(m.act(Action::MoreMasters).map(|c| c.masters), Some(2));
        assert_eq!((desk.frame(2).x, desk.frame(1).x), (10.0, 10.0), "two on the left now");
        m.act(Action::FewerMasters);
        assert_eq!(m.act(Action::FewerMasters), None, "never fewer than one");
        // Through the layouts and back.
        assert_eq!(m.act(Action::NextLayout).map(|c| c.layout), Some(Layout::Columns));
        assert_eq!(desk.frame(3).h, SCREEN.h - 20.0);
        m.act(Action::NextLayout);
        m.act(Action::NextLayout);
        assert_eq!((m.config.layout, desk.frame(1), desk.frame(2)), (Layout::Monocle, desk.frame(3), desk.frame(3)));
        assert_eq!(m.act(Action::NextLayout).map(|c| c.layout), Some(Layout::MasterStack));
        // With tiling off, the keys do nothing.
        m.configure(Windowing::default());
        assert_eq!((m.act(Action::NextLayout), m.act(Action::Grow)), (None, None));
    }

    #[test]
    fn each_screen_is_laid_out_on_its_own() {
        let (mut m, desk) = desk(&[(1, "Notes"), (2, "Safari"), (3, "NeoTerm")]);
        let second = Rect::new(1440.0, 0.0, 1920.0, 1080.0);
        desk.0.borrow_mut().screens.push(second);
        desk.drag(3, Rect::new(2000.0, 300.0, 500.0, 400.0));
        m.tick(false);
        assert_eq!(desk.frame(3), tiling().arrange(1, second)[0], "alone on the second screen, it has it");
        let first = tiling().arrange(2, SCREEN);
        assert_eq!((desk.frame(2), desk.frame(1)), (first[0], first[1]));
        // Dragged across, it joins the other screen's windows.
        m.tick(false);
        desk.drag(1, Rect::new(2100.0, 200.0, 500.0, 400.0));
        m.tick(false);
        assert_eq!(desk.frame(2), tiling().arrange(1, SCREEN)[0]);
        assert!(second.contains(middle(desk.frame(1))) && second.contains(middle(desk.frame(3))));
    }

    /// What this computer says of itself, to look at by hand:
    /// `cargo test -p neo-shell this_computer -- --ignored --nocapture`.
    /// Nothing is moved.
    #[test]
    #[ignore]
    fn this_computer() {
        let system = System;
        println!("possible {}, allowed {}, button down {}", system.possible(), system.allowed(false), system.button_down());
        println!("screens {:?}", system.screens());
        for w in system.windows() {
            println!("{} {:?} {:?}", w.id, w.app, w.frame);
        }
        println!("focused {:?}", system.focused());
    }

    #[test]
    fn what_the_system_says_of_screens_and_windows_is_read() {
        assert_eq!(parse_screens("0\t25\t1512\t916\n1512\t0\t1920\t1080\nnonsense\n0\t0\t0\t0\n"), [Rect::new(0.0, 25.0, 1512.0, 916.0), Rect::new(1512.0, 0.0, 1920.0, 1080.0)]);
        let found = parse_windows("412\t8801\t20\t50\t800\t600\tSafari\n77\t12\t-300\t0\t400\t300\tA name\twith a tab\nbroken\n");
        assert_eq!(found[0], Win { id: 412, pid: 8801, app: "Safari".into(), frame: Rect::new(20.0, 50.0, 800.0, 600.0) });
        assert_eq!((found.len(), found[1].app.as_str(), found[1].frame.x), (2, "A name\twith a tab", -300.0));
    }
}
