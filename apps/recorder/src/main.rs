//! Neo Recorder: record the screen, one window, or an area you frame.
//!
//!     cargo run -p neo-recorder
//!     cargo run -p neo-recorder -- --mode area --record --for 10
//!     cargo run -p neo-recorder -- --mode window --window Firefox --record
//!     cargo run -p neo-recorder -- --snapshot target/snapshots
//!
//! `--mode screen|window|area` picks what to record, `--window TEXT` chooses
//! the first window whose app or title contains TEXT, `--record` starts
//! straight away, and `--for SECONDS` stops by itself.
//!
//! Command+Shift+R on macOS, Ctrl+Shift+R elsewhere, shows the recorder from
//! anywhere and stops a recording. The window hides while recording so it
//! stays out of the picture. In Area mode the window becomes a hollow frame:
//! drag and resize it over what you want, then press Record.

// Release builds on Windows open no console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod capture;

use std::path::PathBuf;
use std::time::Duration;

use global_hotkey::hotkey::{Code, HotKey, Modifiers as HotMods};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use neo::prelude::*;
use neo::{Color, Cx, CursorIcon, DrawCx, Event, EventCx, Key, KeyEvent, Limits, Point, PointerButton, Proxy, Rect, ResizeEdge, Size, Status, TextLayout, TextStyle, Widget, WindowRequest, WindowState};
use neo_desktop::fs::{human_size, user_dir};
use neo_desktop::ui::notice;
use neo_desktop::Desktop;

use capture::{Options, Recording, Saved, Target, WindowInfo};

const POPUP: Size = Size { w: 460.0, h: 500.0 };
const FRAME: Size = Size { w: 760.0, h: 520.0 };
/// Thickness of the frame's outline, which is left out of the recording.
const BORDER: f32 = 3.0;
/// Size of the drag handles on the outline, and how close counts as a grab.
const HANDLE: f32 = 10.0;
const GRAB: f32 = 16.0;
/// Below this width the frame's controls shrink to icons.
const NARROW: f32 = 520.0;
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
    /// The window's content area on the screen, for Area mode.
    frame: Rect,
    scale: f32,
    recording: Option<Recording>,
    permitted: bool,
    proxy: Option<Proxy<Msg>>,
    /// Kept alive so the shortcut stays registered.
    hotkeys: Option<GlobalHotKeyManager>,
    hotkey_error: Option<String>,
    /// Off for screenshots and tests, which must not grab a system-wide shortcut.
    register_hotkey: bool,
    /// Start recording as soon as the window is up (`--record`).
    auto_record: bool,
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
    Record,
    Begin,
    Stop,
    Finished(Result<Saved, String>),
    Hotkey,
    Hide,
    Quit,
    Frame(Rect, f32),
    Open,
    Reveal,
    Reset,
    AskPermission,
    Tick,
    Poll,
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
            frame: Rect::ZERO,
            scale: 1.0,
            recording: None,
            permitted: capture::permitted(),
            proxy: None,
            hotkeys: None,
            hotkey_error: None,
            register_hotkey,
            auto_record: false,
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

    /// Whether the shortcut works, so the window can hide while recording.
    fn can_hide(&self) -> bool {
        self.hotkeys.is_some()
    }

    fn busy(&self) -> bool {
        matches!(self.phase, Phase::Starting(_) | Phase::Recording)
    }

    fn stop(&mut self) {
        let Some(rec) = self.recording.take() else { return };
        self.phase = Phase::Saving;
        self.shown = true;
        match self.proxy.clone() {
            // Finishing the file takes a moment; keep the window responsive.
            Some(proxy) => {
                std::thread::spawn(move || proxy.send(Msg::Finished(rec.stop())));
            }
            None => self.update(Msg::Finished(rec.stop())),
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
        let bare = self.mode == Mode::Area && self.phase == Phase::Idle;
        WindowState { visible: if self.busy() { !self.can_hide() } else { self.shown }, always_on_top: true, bare, size: Some(if bare { FRAME } else { POPUP }) }
    }

    fn on_close(&self) -> Option<Msg> {
        // Stay running for the shortcut when there is one.
        Some(if self.can_hide() { Msg::Hide } else { Msg::Quit })
    }

    fn on_window_frame(&self, frame: Rect, scale: f32) -> Option<Msg> {
        Some(Msg::Frame(frame, scale))
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
            Phase::Starting(_) => subs.push(Subscription::every(Duration::from_millis(350), Msg::Begin)),
            Phase::Recording => {
                subs.push(Subscription::every(Duration::from_secs(1), Msg::Tick));
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
            Key::Enter | Key::Space if self.phase == Phase::Recording => Some(Msg::Stop),
            Key::Escape if self.can_hide() && !self.busy() => Some(Msg::Hide),
            _ => None,
        }
    }

    fn update(&mut self, m: Msg) {
        match m {
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
            Msg::Record => match self.target() {
                Ok(target) => self.phase = Phase::Starting(target),
                Err(e) => self.phase = Phase::Failed(e),
            },
            Msg::Begin => {
                let Phase::Starting(target) = &self.phase else { return };
                let name = format!("Neo Recording {}.{}", chrono::Local::now().format("%Y-%m-%d at %H.%M.%S"), capture::EXTENSION);
                let options = Options { show_clicks: self.show_clicks, scale: self.scale };
                match capture::start(target, options, &recordings_dir().join(name)) {
                    Ok(rec) => {
                        self.recording = Some(rec);
                        self.phase = Phase::Recording;
                    }
                    Err(e) => {
                        self.phase = Phase::Failed(e);
                        self.shown = true;
                    }
                }
            }
            Msg::Stop => self.stop(),
            Msg::Finished(result) => {
                self.phase = match result {
                    Ok(saved) => Phase::Done(saved),
                    Err(e) => Phase::Failed(e),
                };
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
                        self.update(Msg::Refresh);
                    }
                }
            },
            Msg::Hide => self.shown = false,
            Msg::Quit => {
                // Finish the file rather than leave a broken one behind.
                if let Some(rec) = self.recording.take() {
                    let _ = rec.stop();
                }
                self.quit = true;
            }
            Msg::Frame(frame, scale) => {
                self.frame = frame;
                self.scale = scale;
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

    fn view(&self) -> Element<Msg> {
        if self.window_state().bare {
            return self.frame_view();
        }
        let body: Element<Msg> = match &self.phase {
            Phase::Idle => self.chooser(),
            Phase::Starting(_) => status(icons::VIDEO, Tone::Accent, "Starting…".into(), String::new(), row()),
            Phase::Recording => {
                let elapsed = self.recording.as_ref().map_or(Duration::ZERO, |r| r.started.elapsed());
                status(icons::CIRCLE_STOP, Tone::Bad, format!("Recording {}", clock(elapsed)), "This window is part of the picture while it is on screen.".into(), row().push(Button::new(text("Stop").role(TextRole::Strong)).kind(ButtonKind::Accent).on_press(Msg::Stop)))
            }
            Phase::Saving => status(icons::VIDEO, Tone::Accent, "Saving…".into(), String::new(), row()),
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
            None if self.can_hide() => format!("{SHORTCUT} shows this window and stops a recording."),
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
                    .push(text(if self.can_hide() { format!("This window hides while recording. Press {SHORTCUT} to stop.") } else { "Press Stop in this window to finish.".into() }).role(TextRole::Caption).tone(Tone::Muted).align(Align::Center));
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
        if cfg!(target_os = "macos") {
            col = col.push(row().spacing(12.0).align(Align::Center).width(Length::Fill).push(text("Show mouse clicks").width(Length::Fill)).push(toggle(self.show_clicks, Msg::ShowClicks)));
        }
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
            row().spacing(10.0).align(Align::Center).push(mode_switch(self.mode)).push(record_button(ready)).push(close)
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
        r.update(Msg::Frame(Rect::new(100.0, 50.0, 600.0, 400.0), 2.0));
        // The outline is left out.
        assert_eq!(r.target(), Ok(Target::Area(Rect::new(103.0, 53.0, 594.0, 394.0))));
        r.update(Msg::Frame(Rect::new(0.0, 0.0, 36.0, 80.0), 2.0));
        assert!(r.target().is_err(), "a sliver is refused");
    }

    #[test]
    fn the_window_is_a_bare_frame_only_while_choosing_an_area() {
        let mut r = recorder();
        assert_eq!(r.window_state(), WindowState { visible: true, always_on_top: true, bare: false, size: Some(POPUP) });
        r.update(Msg::Mode(Mode::Area));
        assert_eq!(r.window_state(), WindowState { visible: true, always_on_top: true, bare: true, size: Some(FRAME) });
        r.phase = Phase::Saving;
        assert!(!r.window_state().bare);
    }

    #[test]
    fn the_shortcut_shows_hides_and_cancels() {
        let mut r = recorder();
        r.update(Msg::Hotkey);
        assert!(!r.window_state().visible, "pressed while showing: hide");
        r.update(Msg::Hotkey);
        assert!(r.window_state().visible, "pressed while hidden: show");
        r.update(Msg::Record);
        assert!(matches!(r.phase, Phase::Starting(Target::Screen)));
        // Without the shortcut the window must stay up so Stop can be clicked.
        assert!(r.window_state().visible);
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
    fn formats_elapsed_time() {
        assert_eq!(clock(Duration::from_secs(83)), "1:23");
        assert_eq!(clock(Duration::from_secs(3725)), "1:02:05");
    }
}
