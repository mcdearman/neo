//! Neo Recorder: record the screen, one window, or an area you frame.
//!
//!     cargo run -p neo-recorder
//!     cargo run -p neo-recorder -- --mode area --record --for 10
//!     cargo run -p neo-recorder -- --mode window --window Firefox --record
//!     cargo run -p neo-recorder -- --snapshot target/snapshots
//!
//! `--mode screen|window|area` picks what to record, `--window TEXT` chooses
//! the first window whose app or title contains TEXT, `--record` starts
//! straight away, `--for SECONDS` stops by itself, `--mic` records the
//! microphone, `--gif` saves a GIF instead of a movie, and `--include-bar`
//! lets the recording bar appear in the recording. `--hidden` starts in the
//! tray without showing the window, which is how it starts at login.
//!
//! The Recorder keeps an icon in the menu bar or system tray, and by default
//! the installed app starts at login so the shortcut is always ready. A
//! build run from Cargo's `target` folder does not add itself to startup
//! unless you turn that on. `--dir FOLDER` saves
//! somewhere other than the Movies or Videos folder.
//!
//! While recording, the window shrinks to a small bar with the time, Pause
//! and Stop, kept out of the recording where the system allows. Command+
//! Shift+R on macOS, Ctrl+Shift+R elsewhere, brings up the area frame from
//! anywhere and stops a recording. In Area mode the window becomes a hollow
//! frame: drag and resize it over what you want, then press Record.

// Release builds on Windows open no console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod capture;
mod media;
mod tray;

use std::path::PathBuf;
use std::time::Duration;

use global_hotkey::hotkey::{Code, HotKey, Modifiers as HotMods};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use neo::prelude::*;
use neo::{Color, Cx, CursorIcon, DrawCx, Event, EventCx, Key, KeyEvent, Limits, Point, PointerButton, Proxy, Rect, ResizeEdge, Size, Status, TextLayout, TextStyle, Widget, WindowGeometry, WindowRequest, WindowState};
use neo_desktop::fs::{human_size, user_dir};
use neo_desktop::ui::notice;
use neo_desktop::Desktop;

use capture::{Options, Saved, Session, Target, WindowInfo};
use tray::{Tray, TrayAction, TrayState};

const POPUP: Size = Size { w: 460.0, h: 650.0 };
/// The bar shown while recording.
const BAR: Size = Size { w: 300.0, h: 60.0 };
const FRAME: Size = Size { w: 760.0, h: 520.0 };
/// Thickness of the frame's outline, which is left out of the recording.
const BORDER: f32 = 3.0;
/// Size of the drag handles on the outline, and how close counts as a grab.
const HANDLE: f32 = 10.0;
const GRAB: f32 = 16.0;
/// Below this width the frame's controls shrink to icons.
const NARROW: f32 = 640.0;
const SHORTCUT: &str = if cfg!(target_os = "macos") { "⌘⇧R" } else { "Ctrl+Shift+R" };

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Screen,
    Window,
    Area,
}

const MODES: [Mode; 3] = [Mode::Screen, Mode::Window, Mode::Area];

#[derive(Clone, Debug, PartialEq)]
enum Phase {
    Idle,
    /// The window has been told to hide; capture starts once it is gone.
    Starting(Target),
    Recording,
    Saving,
    Done(Saved),
    Failed(String),
}

struct Recorder {
    desktop: Desktop,
    mode: Mode,
    phase: Phase,
    /// Whether the window is wanted on screen when nothing is being recorded.
    shown: bool,
    windows: Vec<WindowInfo>,
    selected: Option<u64>,
    show_clicks: bool,
    microphone: bool,
    /// Save a GIF instead of a movie.
    gif: bool,
    /// The window's content area on the screen, for Area mode.
    frame: Rect,
    screen: Rect,
    scale: f32,
    /// Where the window was when recording began, to put it back afterwards.
    home: Option<Point>,
    /// What is being recorded, to keep the recording bar clear of it.
    subject: Option<Rect>,
    session: Option<Session>,
    permitted: bool,
    proxy: Option<Proxy<Msg>>,
    /// Kept alive so the shortcut stays registered.
    hotkeys: Option<GlobalHotKeyManager>,
    hotkey_error: Option<String>,
    tray: Option<Tray>,
    tray_state: TrayState,
    /// Start at login. Saved in `recorder.conf`.
    autostart: bool,
    autostart_error: Option<String>,
    /// A problem to report once the recording made so far has been saved.
    status_after_stop: Option<String>,
    /// Off for screenshots and tests, which must not grab a system-wide shortcut.
    register_hotkey: bool,
    /// Start recording as soon as the window is up (`--record`).
    auto_record: bool,
    /// Where recordings are saved (`--dir`); the Movies or Videos folder by default.
    dir: PathBuf,
    /// Leave the recording bar visible in recordings and screenshots (`--include-bar`).
    include_bar: bool,
    /// Stop by itself after this long (`--for`).
    stop_after: Option<Duration>,
    quit: bool,
}

#[derive(Clone, Debug)]
enum Msg {
    Mode(Mode),
    Select(u64),
    Refresh,
    ShowClicks(bool),
    Microphone(bool),
    Gif(bool),
    Record,
    Begin,
    Pause,
    Resume,
    Stop,
    Finished(Result<Saved, String>),
    Hotkey,
    Tray(TrayAction),
    Autostart(bool),
    Hide,
    Quit,
    Geometry(WindowGeometry),
    Open,
    Reveal,
    Reset,
    AskPermission,
    Tick,
    Poll,
}

/// The Recorder's own settings, in `recorder.conf` beside the appearance file.
mod settings {
    use std::path::PathBuf;

    fn path() -> PathBuf {
        neo_desktop::config_dir().join("recorder.conf")
    }

    /// Whether to start at login, if the user has ever chosen.
    pub fn load() -> Option<bool> {
        parse(&std::fs::read_to_string(path()).ok()?)
    }

    pub fn parse(src: &str) -> Option<bool> {
        src.lines().find_map(|l| l.trim().strip_prefix("launch-at-startup")).and_then(|v| match v.trim_start_matches([' ', '=']).trim() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        })
    }

    pub fn save(launch_at_startup: bool) {
        let path = path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, format!("# Neo Recorder settings.\nlaunch-at-startup = {launch_at_startup}\n"));
    }

    /// Whether this is an installed copy and not one run from Cargo's build
    /// folder, which should not add itself to startup uninvited.
    pub fn installed() -> bool {
        std::env::current_exe().is_ok_and(|p| !p.components().any(|c| c.as_os_str() == "target"))
    }
}

fn recordings_dir() -> PathBuf {
    if cfg!(target_os = "macos") { neo_desktop::fs::home_dir().join("Movies") } else { user_dir("VIDEOS") }
}

fn clock(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

impl Recorder {
    fn new(register_hotkey: bool) -> Self {
        Self {
            desktop: Desktop::load(),
            mode: Mode::Screen,
            phase: Phase::Idle,
            shown: true,
            windows: capture::windows(),
            selected: None,
            show_clicks: false,
            microphone: false,
            gif: false,
            frame: Rect::ZERO,
            screen: Rect::ZERO,
            scale: 1.0,
            home: None,
            subject: None,
            session: None,
            permitted: capture::permitted(),
            proxy: None,
            hotkeys: None,
            hotkey_error: None,
            tray: None,
            tray_state: TrayState::default(),
            autostart: false,
            autostart_error: None,
            status_after_stop: None,
            register_hotkey,
            auto_record: false,
            dir: recordings_dir(),
            include_bar: false,
            stop_after: None,
            quit: false,
        }
    }

    /// The part of the screen inside the frame, in Area mode.
    fn area(&self) -> Rect {
        Rect::new(self.frame.x + BORDER, self.frame.y + BORDER, (self.frame.w - BORDER * 2.0).max(0.0), (self.frame.h - BORDER * 2.0).max(0.0))
    }

    fn target(&self) -> Result<Target, String> {
        match self.mode {
            Mode::Screen => Ok(Target::Screen),
            Mode::Window => self.selected.and_then(|id| self.windows.iter().find(|w| w.id == id)).cloned().map(Target::Window).ok_or_else(|| "Choose a window to record first.".into()),
            Mode::Area => {
                let a = self.area();
                if a.w < 32.0 || a.h < 32.0 { Err("Make the frame a little bigger.".into()) } else { Ok(Target::Area(a)) }
            }
        }
    }

    /// Whether a hidden window can be brought back: by the shortcut or the tray.
    fn can_hide(&self) -> bool {
        self.hotkeys.is_some() || self.tray.is_some()
    }

    /// Adds the tray icon. Called once the event loop is running, which
    /// macOS needs.
    fn add_tray(&mut self) {
        let Some(proxy) = self.proxy.clone() else { return };
        self.tray_state = TrayState { recording: self.busy(), autostart: self.autostart };
        // No tray is not fatal: the window and shortcut still work.
        match Tray::new(self.tray_state, move |action| {
            proxy.send(Msg::Tray(action));
        }) {
            Ok(tray) => self.tray = Some(tray),
            Err(e) => eprintln!("neo-recorder: no tray icon: {e}"),
        }
    }

    /// Makes startup match the setting, and points it at this copy of the app.
    fn apply_autostart(&mut self) {
        let Ok(program) = std::env::current_exe() else { return };
        let entry = neo_desktop::autostart::Entry { id: "org.neo.Recorder", name: "Neo Recorder", program: &program, args: &["--hidden"] };
        let result = if self.autostart {
            if neo_desktop::autostart::is_enabled(&entry) { Ok(()) } else { neo_desktop::autostart::enable(&entry) }
        } else {
            neo_desktop::autostart::disable(&entry)
        };
        self.autostart_error = result.err().map(|e| format!("Could not change the startup setting: {e}"));
    }

    /// Where the recording bar goes: just below what is being recorded, or
    /// above it, so the bar is out of the picture even where the system
    /// cannot hide it. Full-screen recordings put it at the bottom centre.
    fn bar_origin(&self) -> Point {
        let s = self.screen;
        let gap = 10.0;
        let (cx, y) = match self.subject {
            Some(r) if r.bottom() + gap + BAR.h <= s.bottom() => (r.center().x, r.bottom() + gap),
            Some(r) if r.y - gap - BAR.h >= s.y => (r.center().x, r.y - gap - BAR.h),
            Some(r) => (r.center().x, r.bottom() - gap - BAR.h),
            None => (s.center().x, s.bottom() - BAR.h - 96.0),
        };
        let x = (cx - BAR.w * 0.5).clamp(s.x + 8.0, (s.right() - BAR.w - 8.0).max(s.x + 8.0));
        Point::new(x.round(), y.round())
    }

    fn busy(&self) -> bool {
        matches!(self.phase, Phase::Starting(_) | Phase::Recording)
    }

    fn stop(&mut self) {
        let Some(session) = self.session.take() else { return };
        self.phase = Phase::Saving;
        self.shown = true;
        let gif = self.gif;
        match self.proxy.clone() {
            // Joining parts and making a GIF take a while; keep the window responsive.
            Some(proxy) => {
                std::thread::spawn(move || proxy.send(Msg::Finished(session.finish(gif))));
            }
            None => self.update(Msg::Finished(session.finish(gif))),
        }
    }
}

impl App for Recorder {
    type Message = Msg;

    fn title(&self) -> String {
        "Recorder".into()
    }

    fn window(&self) -> WindowSettings {
        WindowSettings { size: POPUP, min_size: Some(Size::new(240.0, 150.0)), app_id: Some("org.neo.Recorder".into()), ..Default::default() }
    }

    fn theme(&self, system: Scheme) -> Theme {
        self.desktop.theme(system)
    }

    fn window_state(&self) -> WindowState {
        let recording = self.busy();
        let framing = self.mode == Mode::Area && self.phase == Phase::Idle;
        WindowState {
            visible: recording || self.shown,
            // Only the overlays float: the area frame, which has to sit over
            // what it frames, and the recording bar. The ordinary window
            // goes behind whatever the user clicks, like any other.
            always_on_top: recording || framing,
            bare: recording || framing,
            size: Some(if recording { BAR } else if framing { FRAME } else { POPUP }),
            position: if recording { Some(self.bar_origin()) } else { self.home },
            hidden_from_capture: recording && !self.include_bar,
        }
    }

    fn on_close(&self) -> Option<Msg> {
        // Stay running for the shortcut when there is one.
        Some(if self.can_hide() { Msg::Hide } else { Msg::Quit })
    }

    fn on_window_geometry(&self, geometry: WindowGeometry) -> Option<Msg> {
        Some(Msg::Geometry(geometry))
    }

    fn should_exit(&self) -> bool {
        self.quit
    }

    fn start(&mut self, proxy: Proxy<Msg>) {
        self.proxy = Some(proxy.clone());
        if self.auto_record {
            proxy.send(Msg::Record);
        }
        if !self.register_hotkey {
            return;
        }
        let primary = if cfg!(target_os = "macos") { HotMods::SUPER } else { HotMods::CONTROL };
        let hotkey = HotKey::new(Some(primary | HotMods::SHIFT), Code::KeyR);
        match GlobalHotKeyManager::new().and_then(|m| m.register(hotkey).map(|_| m)) {
            Ok(manager) => {
                GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
                    if e.state() == HotKeyState::Pressed {
                        proxy.send(Msg::Hotkey);
                    }
                }));
                self.hotkeys = Some(manager);
            }
            Err(e) => self.hotkey_error = Some(e.to_string()),
        }
    }

    fn subscriptions(&self) -> Vec<Subscription<Msg>> {
        let mut subs = vec![Desktop::subscription(Msg::Poll)];
        match self.phase {
            // Long enough for the window to leave the screen.
            Phase::Starting(_) => subs.push(Subscription::every(Duration::from_millis(400), Msg::Begin)),
            Phase::Recording => {
                subs.push(Subscription::every(Duration::from_millis(500), Msg::Tick));
                if let Some(limit) = self.stop_after {
                    subs.push(Subscription::every(limit, Msg::Stop));
                }
            }
            _ => {}
        }
        subs
    }

    fn on_key(&self, k: &KeyEvent) -> Option<Msg> {
        match k.key {
            Key::Enter if self.phase == Phase::Idle => Some(Msg::Record),
            Key::Enter if self.phase == Phase::Recording => Some(Msg::Stop),
            Key::Space if self.phase == Phase::Recording => Some(if self.session.as_ref().is_some_and(|s| s.paused()) { Msg::Resume } else { Msg::Pause }),
            Key::Escape if self.can_hide() && !self.busy() => Some(Msg::Hide),
            _ => None,
        }
    }

    fn update(&mut self, m: Msg) {
        self.apply(m);
        // Keep the tray's icon and menu in step with what just happened.
        let state = TrayState { recording: self.busy(), autostart: self.autostart };
        if state != self.tray_state {
            self.tray_state = state;
            if let Some(tray) = &self.tray {
                tray.update(state);
            }
        }
    }

    fn view(&self) -> Element<Msg> {
        self.window_view()
    }
}

impl Recorder {
    fn apply(&mut self, m: Msg) {
        match m {
            Msg::Tray(action) => match action {
                TrayAction::Show => {
                    self.shown = true;
                    self.apply(Msg::Refresh);
                }
                TrayAction::ToggleRecording => match self.phase {
                    Phase::Recording => self.stop(),
                    Phase::Idle | Phase::Done(_) | Phase::Failed(_) => {
                        self.phase = Phase::Idle;
                        self.apply(Msg::Record);
                        // A problem, such as no window chosen, needs the window to say so.
                        if matches!(self.phase, Phase::Failed(_)) {
                            self.shown = true;
                        }
                    }
                    Phase::Starting(_) | Phase::Saving => {}
                },
                TrayAction::ToggleAutostart => self.apply(Msg::Autostart(!self.autostart)),
                TrayAction::Quit => self.apply(Msg::Quit),
            },
            Msg::Autostart(on) => {
                self.autostart = on;
                settings::save(on);
                self.apply_autostart();
            }
            Msg::Mode(mode) => {
                self.mode = mode;
                if mode == Mode::Window {
                    self.windows = capture::windows();
                }
            }
            Msg::Select(id) => self.selected = Some(id),
            Msg::Refresh => {
                self.windows = capture::windows();
                self.permitted = capture::permitted();
            }
            Msg::ShowClicks(on) => self.show_clicks = on,
            Msg::Microphone(on) => self.microphone = on,
            Msg::Gif(on) => self.gif = on,
            Msg::Record => match self.target() {
                Ok(target) => {
                    self.home = Some(Point::new(self.frame.x, self.frame.y));
                    self.subject = match &target {
                        Target::Screen => None,
                        Target::Window(w) => Some(w.frame),
                        Target::Area(r) => Some(*r),
                    };
                    self.phase = Phase::Starting(target);
                }
                Err(e) => self.phase = Phase::Failed(e),
            },
            Msg::Begin => {
                let Phase::Starting(target) = &self.phase else { return };
                let name = format!("Neo Recording {}.{}", chrono::Local::now().format("%Y-%m-%d at %H.%M.%S"), capture::EXTENSION);
                // A GIF has no sound, so there is no point recording any.
                let options = Options { show_clicks: self.show_clicks, microphone: self.microphone && !self.gif, scale: self.scale };
                match Session::start(target.clone(), options, self.dir.join(name)) {
                    Ok(session) => {
                        self.session = Some(session);
                        self.phase = Phase::Recording;
                    }
                    Err(e) => {
                        self.phase = Phase::Failed(e);
                        self.shown = true;
                    }
                }
            }
            Msg::Pause => {
                if let Some(s) = &mut self.session {
                    s.pause();
                }
            }
            Msg::Resume => {
                if let Some(Err(e)) = self.session.as_mut().map(|s| s.resume()) {
                    // Keep what was recorded so far.
                    self.status_after_stop = Some(e);
                    self.stop();
                }
            }
            Msg::Stop => self.stop(),
            Msg::Finished(result) => {
                self.phase = match result {
                    Ok(saved) => Phase::Done(saved),
                    Err(e) => Phase::Failed(e),
                };
                if let (Phase::Done(_), Some(e)) = (&self.phase, self.status_after_stop.take()) {
                    self.phase = Phase::Failed(format!("Recording could not resume, so it stopped early: {e}"));
                }
                self.shown = true;
            }
            Msg::Hotkey => match self.phase {
                Phase::Recording => self.stop(),
                Phase::Starting(_) => {
                    self.phase = Phase::Idle;
                    self.shown = true;
                }
                Phase::Saving => {}
                _ => {
                    self.shown = !self.shown;
                    if self.shown {
                        // The shortcut goes straight to framing an area, the
                        // quickest way to grab part of the screen. The other
                        // modes are one click away in the frame's controls.
                        self.phase = Phase::Idle;
                        self.mode = Mode::Area;
                        self.apply(Msg::Refresh);
                    }
                }
            },
            Msg::Hide => self.shown = false,
            Msg::Quit => {
                // Finish the file rather than leave a broken one behind.
                if let Some(session) = self.session.take() {
                    let _ = session.finish(self.gif);
                }
                self.quit = true;
            }
            Msg::Geometry(g) => {
                // The first report means the event loop is up, so the tray can go in.
                if self.tray.is_none() && self.register_hotkey && self.screen == Rect::ZERO {
                    self.add_tray();
                }
                self.screen = g.screen;
                self.scale = g.scale;
                // The recording bar's own frame is not the area to record.
                if !self.busy() {
                    self.frame = g.frame;
                }
            }
            Msg::Open => {
                if let Phase::Done(saved) = &self.phase {
                    let _ = neo_desktop::fs::open(&saved.path);
                }
            }
            Msg::Reveal => {
                if let Phase::Done(saved) = &self.phase {
                    let _ = neo_desktop::fs::reveal(&saved.path);
                }
            }
            Msg::Reset => self.phase = Phase::Idle,
            Msg::AskPermission => {
                capture::request_permission();
                self.permitted = capture::permitted();
            }
            Msg::Tick => {}
            Msg::Poll => {
                self.desktop.poll();
            }
        }
    }

    fn window_view(&self) -> Element<Msg> {
        if self.busy() {
            return self.recording_bar();
        }
        if self.window_state().bare {
            return self.frame_view();
        }
        let body: Element<Msg> = match &self.phase {
            Phase::Idle => self.chooser(),
            // Shown as the recording bar instead; see `recording_bar`.
            Phase::Starting(_) | Phase::Recording => Space::fill_y().into(),
            Phase::Saving => status(icons::VIDEO, Tone::Accent, "Saving…".into(), if self.gif { "Making the GIF. Long recordings take a while.".into() } else { String::new() }, row()),
            Phase::Done(saved) => {
                let name = saved.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let dir = saved.path.parent().map(|d| d.display().to_string()).unwrap_or_default();
                status(
                    icons::CIRCLE_CHECK,
                    Tone::Good,
                    name,
                    format!("{} · {} · saved in {dir}", clock(saved.length), human_size(saved.bytes)),
                    row().spacing(10.0).push(button("Play").on_press(Msg::Open)).push(button("Show in folder").on_press(Msg::Reveal)).push(Button::new(text("Record again").role(TextRole::Strong)).kind(ButtonKind::Accent).on_press(Msg::Reset)),
                )
            }
            Phase::Failed(e) => status(icons::TRIANGLE_ALERT, Tone::Warn, "That didn't record".into(), e.clone(), row().push(button("Back").on_press(Msg::Reset))),
        };
        column().width(Length::Fill).height(Length::Fill).padding([0.0, 16.0, 16.0, 16.0]).push(body).into()
    }
}

/// A centred message with an icon and actions, for every state but choosing.
fn status(glyph: neo::theme::Icon, tone: Tone, title: String, detail: String, actions: Row<Msg>) -> Element<Msg> {
    let mut col = column().spacing(10.0).align(Align::Center).push(icon(glyph).size(40.0).tone(tone)).push(text(title).role(TextRole::Title).align(Align::Center));
    if !detail.is_empty() {
        col = col.push(container(text(detail).role(TextRole::Caption).tone(Tone::Muted).align(Align::Center)).max_width(380.0));
    }
    container(col.push(Space::new(0.0, 6.0)).push(actions)).width(Length::Fill).height(Length::Fill).center().into()
}

fn mode_switch(mode: Mode) -> Element<Msg> {
    segmented(["Full screen", "Window", "Area"], MODES.iter().position(|m| *m == mode), |i| Msg::Mode(MODES[i])).into()
}

impl Recorder {
    fn shortcut_hint(&self) -> String {
        match &self.hotkey_error {
            None if self.hotkeys.is_some() => format!("{SHORTCUT} brings up the area frame and stops a recording."),
            Some(e) => format!("The {SHORTCUT} shortcut is unavailable: {e}"),
            None => String::new(),
        }
    }

    fn chooser(&self) -> Element<Msg> {
        let detail: Element<Msg> = match self.mode {
            Mode::Window => self.window_list(),
            _ => {
                let col = column()
                    .spacing(10.0)
                    .align(Align::Center)
                    .push(icon(icons::MONITOR).size(40.0).tone(Tone::Accent))
                    .push(text("Everything on your main display").role(TextRole::Strong))
                    .push(text("A small bar with Pause and Stop stays on screen while recording.").role(TextRole::Caption).tone(Tone::Muted).align(Align::Center));
                container(col).width(Length::Fill).height(Length::Fill).center().into()
            }
        };
        let mut col = column().spacing(14.0).width(Length::Fill).height(Length::Fill);
        if !self.permitted {
            col = col.push(
                container(row().spacing(10.0).align(Align::Center).width(Length::Fill).push(container(notice(Tone::Warn, "This app is not allowed to record the screen yet.")).width(Length::Fill)).push(button("Allow…").on_press(Msg::AskPermission)))
                    .surface(Surface::Well)
                    .padding([14.0, 10.0])
                    .width(Length::Fill),
            );
        }
        col = col.push(container(mode_switch(self.mode)).width(Length::Fill).align_x(Align::Center)).push(container(detail).surface(Surface::Well).width(Length::Fill).height(Length::Fill));
        let option = |title: &str, note: String, on: bool, f: fn(bool) -> Msg| -> Element<Msg> {
            let mut label = column().spacing(1.0).width(Length::Fill).push(text(title));
            if !note.is_empty() {
                label = label.push(text(note).role(TextRole::Caption).tone(Tone::Muted));
            }
            row().spacing(12.0).align(Align::Center).width(Length::Fill).push(label).push(toggle(on, f)).into()
        };
        let mut options = column().spacing(10.0).width(Length::Fill);
        if capture::MICROPHONE {
            let note = if self.gif { "GIFs have no sound." } else { "Sound from the default input." };
            options = options.push(option("Record microphone", note.into(), self.microphone && !self.gif, Msg::Microphone));
        }
        options = options.push(option("Launch at startup", format!("Starts in the tray, so {SHORTCUT} is always ready."), self.autostart, Msg::Autostart));
        options = options.push(option("Save as GIF", format!("{} frames a second, up to {} pixels wide.", media::GIF_FPS, media::GIF_MAX_WIDTH), self.gif, Msg::Gif));
        if cfg!(target_os = "macos") {
            options = options.push(option("Show mouse clicks", String::new(), self.show_clicks, Msg::ShowClicks));
        }
        col = col.push(options);
        let ready = self.mode != Mode::Window || self.selected.is_some_and(|id| self.windows.iter().any(|w| w.id == id));
        col.push(
            row()
                .spacing(12.0)
                .align(Align::Center)
                .width(Length::Fill)
                .push(text(self.shortcut_hint()).role(TextRole::Caption).tone(Tone::Muted).width(Length::Fill))
                .push(record_button(ready)),
        )
        .into()
    }

    fn window_list(&self) -> Element<Msg> {
        if self.windows.is_empty() {
            let why = if cfg!(all(unix, not(target_os = "macos"))) { "Listing windows needs an X11 session with wmctrl installed. Use Area to frame a window instead." } else { "No windows are open." };
            let col = column().spacing(10.0).align(Align::Center).push(icon(icons::APP_WINDOW).size(36.0).tone(Tone::Faint)).push(container(text(why).tone(Tone::Muted).align(Align::Center)).max_width(320.0)).push(button("Look again").on_press(Msg::Refresh));
            return container(col).width(Length::Fill).height(Length::Fill).center().into();
        }
        let mut list = column().spacing(1.0).width(Length::Fill).padding(6.0);
        for w in &self.windows {
            let (name, sub) = match (w.app.is_empty(), w.title.is_empty()) {
                (false, false) => (w.app.clone(), w.title.clone()),
                (false, true) => (w.app.clone(), String::new()),
                _ => (w.title.clone(), String::new()),
            };
            let mut label = column().spacing(1.0).width(Length::Fill).push(text(name).role(TextRole::Strong).no_wrap());
            if !sub.is_empty() {
                label = label.push(text(sub).role(TextRole::Caption).tone(Tone::Muted).no_wrap());
            }
            let content = row()
                .spacing(10.0)
                .align(Align::Center)
                .width(Length::Fill)
                .push(icon(icons::APP_WINDOW).size(17.0).tone(Tone::Muted))
                .push(label)
                .push(text(format!("{} × {}", w.frame.w.round(), w.frame.h.round())).mono().role(TextRole::Caption).tone(Tone::Faint));
            list = list.push(Button::new(content).kind(ButtonKind::Ghost).selected(self.selected == Some(w.id)).padding([10.0, 7.0]).width(Length::Fill).align_x(Align::Start).on_press(Msg::Select(w.id)));
        }
        scrollable(list).height(Length::Fill).into()
    }

    /// Area mode: the whole window is the frame, with its controls floating
    /// inside. The window hides while recording, so they are never captured.
    fn frame_view(&self) -> Element<Msg> {
        let a = self.area();
        let ready = a.w >= 32.0 && a.h >= 32.0;
        let close = icon_button(icons::X, 34.0).kind(ButtonKind::Ghost).on_press(if self.can_hide() { Msg::Hide } else { Msg::Quit });
        let controls = if self.frame.w >= NARROW {
            let mut r = row().spacing(8.0).align(Align::Center).push(mode_switch(self.mode));
            if capture::MICROPHONE {
                let on = self.microphone && !self.gif;
                r = r.push(icon_button(if on { icons::MIC } else { icons::MIC_OFF }, 34.0).kind(ButtonKind::Ghost).selected(on).on_press_maybe((!self.gif).then_some(Msg::Microphone(!self.microphone))));
            }
            r.push(Button::new(text("GIF").role(TextRole::Strong)).kind(ButtonKind::Ghost).selected(self.gif).padding([10.0, 7.0]).on_press(Msg::Gif(!self.gif))).push(record_button(ready)).push(close)
        } else {
            // Too narrow for the mode switch: a way back, record, and close.
            row()
                .spacing(6.0)
                .align(Align::Center)
                .push(icon_button(icons::ARROW_LEFT, 34.0).kind(ButtonKind::Ghost).on_press(Msg::Mode(Mode::Screen)))
                .push(icon_button(icons::VIDEO, 34.0).kind(ButtonKind::Accent).on_press_maybe(ready.then_some(Msg::Record)))
                .push(close)
        };
        let bar = container(container(controls).surface(Surface::Card).padding([10.0, 8.0])).padding([0.0, 0.0, 14.0, 0.0]);
        stack().width(Length::Fill).height(Length::Fill).align_x(Align::Center).align_y(Align::End).push(Element::new(Viewfinder { label: format!("{} × {}", a.w.round(), a.h.round()), layout: None })).push(bar).into()
    }
}

impl Recorder {
    /// The bar shown while recording: time, Pause or Resume, and Stop.
    fn recording_bar(&self) -> Element<Msg> {
        let paused = self.session.as_ref().is_some_and(|s| s.paused());
        let starting = self.session.is_none();
        let elapsed = self.session.as_ref().map_or(Duration::ZERO, |s| s.elapsed());
        let (dot, label) = if starting { (Tone::Muted, "Starting…".to_string()) } else if paused { (Tone::Warn, format!("Paused {}", clock(elapsed))) } else { (Tone::Bad, clock(elapsed)) };
        let pause = if paused { icon_button(icons::PLAY, 36.0).on_press_maybe((!starting).then_some(Msg::Resume)) } else { icon_button(icons::PAUSE, 36.0).on_press_maybe((!starting).then_some(Msg::Pause)) };
        let controls = row()
            .spacing(10.0)
            .align(Align::Center)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding([16.0, 0.0])
            .push(icon(icons::CIRCLE).size(12.0).tone(dot))
            .push(text(label).mono().role(TextRole::Strong).no_wrap().width(Length::Fill))
            .push(pause)
            .push(Button::new(row().spacing(6.0).align(Align::Center).push(icon(icons::SQUARE).size(13.0)).push(text("Stop").role(TextRole::Strong))).kind(ButtonKind::Accent).padding([14.0, 8.0]).on_press_maybe((!starting).then_some(Msg::Stop)));
        stack().width(Length::Fill).height(Length::Fill).push(Element::new(Grip)).push(controls).into()
    }
}

/// The recording bar's background, which also drags the window.
struct Grip;

impl<M> Widget<M> for Grip {
    fn width(&self) -> Length {
        Length::Fill
    }

    fn height(&self) -> Length {
        Length::Fill
    }

    fn layout(&mut self, _cx: &mut Cx, limits: Limits) -> Size {
        limits.max
    }

    fn draw(&self, cx: &mut DrawCx) {
        let b = cx.bounds();
        let p = cx.theme().palette();
        // Opaque, whatever the glass setting: it sits over anything.
        cx.scene.fill(b.inset(1.0), (b.h * 0.5).min(18.0), p.surface, Some((1.0, p.line)));
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        match event {
            Event::PointerPressed { pos, button: PointerButton::Primary } if cx.bounds().contains(*pos) => {
                cx.window_request(WindowRequest::Drag);
                Status::Captured
            }
            _ => Status::Ignored,
        }
    }
}

fn record_button(enabled: bool) -> Element<Msg> {
    Button::new(row().spacing(8.0).align(Align::Center).push(icon(icons::VIDEO).size(16.0)).push(text("Record").role(TextRole::Strong))).kind(ButtonKind::Accent).on_press_maybe(enabled.then_some(Msg::Record)).into()
}

/// The frame around what will be recorded: an outline with drag handles.
/// Dragging a handle or an edge resizes the window; dragging inside moves it.
struct Viewfinder {
    label: String,
    layout: Option<TextLayout>,
}

impl Viewfinder {
    /// The eight handles: corners and the middle of each side.
    fn handles(b: Rect) -> [(ResizeEdge, Point); 8] {
        let (l, r, t, m) = (b.x, b.right(), b.y, b.bottom());
        let c = b.center();
        [
            (ResizeEdge::NorthWest, Point::new(l, t)),
            (ResizeEdge::North, Point::new(c.x, t)),
            (ResizeEdge::NorthEast, Point::new(r, t)),
            (ResizeEdge::East, Point::new(r, c.y)),
            (ResizeEdge::SouthEast, Point::new(r, m)),
            (ResizeEdge::South, Point::new(c.x, m)),
            (ResizeEdge::SouthWest, Point::new(l, m)),
            (ResizeEdge::West, Point::new(l, c.y)),
        ]
    }

    /// The edge or corner a pointer position would resize, if it is on the outline.
    fn edge_at(b: Rect, p: Point) -> Option<ResizeEdge> {
        // Corners reach further than sides, so they are easy to catch.
        if let Some((edge, _)) = Self::handles(b).iter().step_by(2).find(|(_, h)| (p.x - h.x).abs() <= GRAB && (p.y - h.y).abs() <= GRAB) {
            return Some(*edge);
        }
        let near = GRAB * 0.5;
        if p.y - b.y <= near {
            Some(ResizeEdge::North)
        } else if b.bottom() - p.y <= near {
            Some(ResizeEdge::South)
        } else if p.x - b.x <= near {
            Some(ResizeEdge::West)
        } else if b.right() - p.x <= near {
            Some(ResizeEdge::East)
        } else {
            None
        }
    }
}

impl<M> Widget<M> for Viewfinder {
    fn width(&self) -> Length {
        Length::Fill
    }

    fn height(&self) -> Length {
        Length::Fill
    }

    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        self.layout = Some(cx.text().layout(&self.label, &TextStyle { size: 13.0, weight: 700, family: neo::FontFamily::Mono, ..Default::default() }, None));
        limits.max
    }

    fn draw(&self, cx: &mut DrawCx) {
        let b = cx.bounds();
        let p = cx.theme().palette();
        // A faint wash shows the frame is there without hiding what is under
        // it. A thin dark line inside the accent outline keeps the outline
        // visible over backgrounds of the same colour.
        cx.scene.fill(b, 0.0, p.accent.with_alpha(0.06), Some((BORDER, p.accent)));
        cx.scene.fill(b.inset(BORDER), 0.0, Color::TRANSPARENT, Some((1.0, Color::BLACK.with_alpha(0.35))));
        for (_, at) in Self::handles(b) {
            // Keep handles inside the window, where they can be drawn.
            let x = (at.x - HANDLE * 0.5).clamp(b.x, b.right() - HANDLE);
            let y = (at.y - HANDLE * 0.5).clamp(b.y, b.bottom() - HANDLE);
            cx.scene.fill(Rect::new(x, y, HANDLE, HANDLE), 2.0, Color::WHITE, Some((2.0, p.accent)));
        }
        if let Some(l) = &self.layout {
            let s = l.size();
            let pill = Rect::new((b.x + BORDER + 10.0).round(), (b.y + BORDER + 10.0).round(), s.w + 20.0, s.h + 10.0);
            if pill.right() < b.right() - 16.0 && pill.bottom() < b.bottom() - 16.0 {
                cx.scene.fill(pill, pill.h * 0.5, p.surface, Some((1.0, p.line)));
                cx.scene.text(l, Point::new(pill.x + 10.0, pill.y + 5.0), p.text);
            }
        }
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        let b = cx.bounds();
        match event {
            Event::PointerMoved { pos } if b.contains(*pos) => {
                cx.set_cursor(Self::edge_at(b, *pos).map_or(CursorIcon::Grab, CursorIcon::Resize));
                Status::Ignored
            }
            Event::PointerPressed { pos, button: PointerButton::Primary } if b.contains(*pos) => {
                cx.window_request(Self::edge_at(b, *pos).map_or(WindowRequest::Drag, WindowRequest::Resize));
                Status::Captured
            }
            _ => Status::Ignored,
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--snapshot") {
        snapshots(PathBuf::from(args.get(i + 1).cloned().unwrap_or_else(|| "target/snapshots".into())));
        return;
    }
    let value = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1));
    let mut app = Recorder::new(true);
    match value("--mode").map(String::as_str) {
        None => {}
        Some("screen") => app.mode = Mode::Screen,
        Some("window") => app.mode = Mode::Window,
        Some("area") => app.mode = Mode::Area,
        Some(other) => {
            eprintln!("neo-recorder: unknown mode {other}; use screen, window or area");
            std::process::exit(2);
        }
    }
    if let Some(want) = value("--window") {
        let want = want.to_lowercase();
        app.mode = Mode::Window;
        app.selected = app.windows.iter().find(|w| w.app.to_lowercase().contains(&want) || w.title.to_lowercase().contains(&want)).map(|w| w.id);
        if app.selected.is_none() {
            eprintln!("neo-recorder: no window matches {want}");
            std::process::exit(2);
        }
    }
    app.auto_record = args.iter().any(|a| a == "--record");
    app.shown = !args.iter().any(|a| a == "--hidden");
    // On unless it has been turned off. Only an installed copy sets startup
    // up by itself; a development build changes it only when asked, so it
    // never replaces the installed app's startup item with its own path.
    app.autostart = settings::load().unwrap_or_else(settings::installed);
    if settings::installed() {
        app.apply_autostart();
    }
    if let Some(dir) = value("--dir") {
        app.dir = PathBuf::from(dir);
    }
    app.include_bar = args.iter().any(|a| a == "--include-bar");
    app.gif = args.iter().any(|a| a == "--gif");
    app.microphone = args.iter().any(|a| a == "--mic");
    app.stop_after = value("--for").and_then(|s| s.parse::<f32>().ok()).filter(|s| *s > 0.0).map(Duration::from_secs_f32);
    if let Err(e) = neo::run(app) {
        eprintln!("neo-recorder: {e}");
        std::process::exit(1);
    }
}

fn snapshots(dir: PathBuf) {
    use neo::testing::Harness;
    std::fs::create_dir_all(&dir).expect("create snapshot dir");
    let shots: [(&str, Mode, Size, neo_desktop::SchemePref); 4] = [
        ("recorder-screen", Mode::Screen, POPUP, neo_desktop::SchemePref::Light),
        ("recorder-window", Mode::Window, POPUP, neo_desktop::SchemePref::Dark),
        ("recorder-area", Mode::Area, FRAME, neo_desktop::SchemePref::Dark),
        ("recorder-done", Mode::Screen, POPUP, neo_desktop::SchemePref::Light),
    ];
    for (name, mode, size, scheme) in shots {
        let mut app = Recorder::new(false);
        app.desktop.appearance.scheme = scheme;
        app.mode = mode;
        app.frame = Rect::new(200.0, 160.0, size.w, size.h);
        app.selected = app.windows.first().map(|w| w.id);
        if name == "recorder-done" {
            app.phase = Phase::Done(Saved { path: recordings_dir().join(format!("Neo Recording 2026-10-04 at 14.32.05.{}", capture::EXTENSION)), bytes: 4_812_000, length: Duration::from_secs(83) });
        }
        let mut h = Harness::new(app, size).expect("GPU");
        let path = dir.join(format!("{name}.png"));
        h.save_png(&path, 1.0).expect("write png");
        println!("wrote {}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recorder() -> Recorder {
        let mut r = Recorder::new(false);
        r.windows = vec![WindowInfo { id: 7, app: "Files".into(), title: "Home".into(), frame: Rect::new(10.0, 20.0, 800.0, 600.0) }];
        r
    }

    fn geometry(frame: Rect) -> WindowGeometry {
        WindowGeometry { frame, screen: Rect::new(0.0, 0.0, 1512.0, 982.0), scale: 2.0 }
    }

    #[test]
    fn targets_follow_the_mode() {
        let mut r = recorder();
        assert_eq!(r.target(), Ok(Target::Screen));
        r.update(Msg::Mode(Mode::Window));
        r.windows = recorder().windows;
        assert!(r.target().is_err(), "a window must be chosen first");
        r.update(Msg::Select(7));
        assert!(matches!(r.target(), Ok(Target::Window(w)) if w.id == 7));
        r.update(Msg::Mode(Mode::Area));
        r.update(Msg::Geometry(geometry(Rect::new(100.0, 50.0, 600.0, 400.0))));
        // The outline is left out.
        assert_eq!(r.target(), Ok(Target::Area(Rect::new(103.0, 53.0, 594.0, 394.0))));
        r.update(Msg::Geometry(geometry(Rect::new(0.0, 0.0, 36.0, 80.0))));
        assert!(r.target().is_err(), "a sliver is refused");
    }

    #[test]
    fn the_window_changes_shape_with_the_state() {
        let mut r = recorder();
        let idle = r.window_state();
        assert!(idle.visible && !idle.always_on_top && !idle.bare && !idle.hidden_from_capture);
        assert_eq!(idle.size, Some(POPUP));
        r.update(Msg::Mode(Mode::Area));
        assert!(r.window_state().bare && r.window_state().always_on_top, "the area frame floats over what it frames");
        assert_eq!(r.window_state().size, Some(FRAME));
        r.phase = Phase::Saving;
        assert!(!r.window_state().bare);
    }

    #[test]
    fn recording_shows_a_bar_clear_of_the_picture() {
        let mut r = recorder();
        r.update(Msg::Mode(Mode::Area));
        r.update(Msg::Geometry(geometry(Rect::new(300.0, 200.0, 600.0, 400.0))));
        r.update(Msg::Record);
        let s = r.window_state();
        assert!(s.visible && s.bare && s.hidden_from_capture && s.always_on_top);
        assert_eq!(s.size, Some(BAR));
        // Centred under the area, just below its bottom edge.
        assert_eq!(s.position, Some(Point::new(450.0, 607.0)));
        // An area at the bottom of the screen pushes the bar above it.
        r.subject = Some(Rect::new(300.0, 500.0, 600.0, 470.0));
        assert_eq!(r.bar_origin(), Point::new(450.0, 430.0));
        // Full screen: bottom centre.
        r.subject = None;
        assert_eq!(r.bar_origin(), Point::new(606.0, 826.0));
        // The bar's own geometry must not replace the framed area.
        r.update(Msg::Geometry(geometry(Rect::new(450.0, 607.0, 300.0, 60.0))));
        assert_eq!(r.frame, Rect::new(300.0, 200.0, 600.0, 400.0));
        assert_eq!(r.home, Some(Point::new(300.0, 200.0)), "the window returns here afterwards");
    }

    #[test]
    fn the_shortcut_shows_hides_and_cancels() {
        let mut r = recorder();
        r.update(Msg::Hotkey);
        assert!(!r.window_state().visible, "pressed while showing: hide");
        r.update(Msg::Hotkey);
        let s = r.window_state();
        assert!(s.visible && s.bare && r.mode == Mode::Area, "pressed while hidden: show the area frame");
        r.update(Msg::Mode(Mode::Screen));
        r.update(Msg::Record);
        assert!(matches!(r.phase, Phase::Starting(Target::Screen)));
        r.update(Msg::Hotkey);
        assert_eq!(r.phase, Phase::Idle, "pressed before capture began: cancel");
        assert!(r.on_close().is_some_and(|m| matches!(m, Msg::Quit)), "with no shortcut, closing quits");
    }

    #[test]
    fn the_outline_resizes_and_the_inside_moves() {
        let b = Rect::new(0.0, 0.0, 400.0, 300.0);
        assert_eq!(Viewfinder::edge_at(b, Point::new(3.0, 4.0)), Some(ResizeEdge::NorthWest));
        assert_eq!(Viewfinder::edge_at(b, Point::new(390.0, 292.0)), Some(ResizeEdge::SouthEast));
        assert_eq!(Viewfinder::edge_at(b, Point::new(200.0, 2.0)), Some(ResizeEdge::North));
        assert_eq!(Viewfinder::edge_at(b, Point::new(397.0, 150.0)), Some(ResizeEdge::East));
        assert_eq!(Viewfinder::edge_at(b, Point::new(200.0, 150.0)), None, "the middle drags the window");
    }

    #[test]
    fn the_tray_menu_drives_the_recorder() {
        let mut r = recorder();
        r.shown = false;
        r.update(Msg::Tray(TrayAction::Show));
        assert!(r.window_state().visible);
        r.update(Msg::Tray(TrayAction::ToggleRecording));
        assert!(matches!(r.phase, Phase::Starting(Target::Screen)), "Start Recording begins with the current mode");
        assert!(r.tray_state.recording, "the tray shows the recording state");
        // With no window chosen, starting from the tray brings the window up to explain.
        let mut r = recorder();
        r.mode = Mode::Window;
        r.shown = false;
        r.update(Msg::Tray(TrayAction::ToggleRecording));
        assert!(matches!(r.phase, Phase::Failed(_)) && r.window_state().visible);
        r.update(Msg::Tray(TrayAction::Quit));
        assert!(r.should_exit());
    }

    #[test]
    fn reads_the_startup_setting() {
        assert_eq!(settings::parse("# Neo Recorder settings.\nlaunch-at-startup = false\n"), Some(false));
        assert_eq!(settings::parse("launch-at-startup = true"), Some(true));
        assert_eq!(settings::parse(""), None, "never chosen: the default applies");
        assert!(!settings::installed(), "a test binary lives in Cargo's target folder");
    }

    #[test]
    fn formats_elapsed_time() {
        assert_eq!(clock(Duration::from_secs(83)), "1:23");
        assert_eq!(clock(Duration::from_secs(3725)), "1:02:05");
    }
}
