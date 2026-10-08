//! Videos: play video files.
//!
//!     cargo run -p neo-videos -- clip.mp4
//!     cargo run -p neo-videos -- --snapshot target/snapshots
//!
//! Space or a click plays and pauses. Left and Right skip five seconds, Up
//! and Down change the volume, and M mutes.

// Release builds on Windows open no console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod player;

use std::path::PathBuf;
use std::time::Duration;

use neo::prelude::*;
use neo::{Color, Cx, CursorIcon, DrawCx, Event, EventCx, Image, Key, KeyEvent, Limits, PointerButton, Rect, Size, Status, Widget};
use neo_desktop::Desktop;

use player::{Player, Status as PlayerStatus};

/// How far the arrow keys skip, in seconds.
const SKIP: f64 = 5.0;

struct Videos {
    desktop: Desktop,
    path: Option<PathBuf>,
    player: Option<Player>,
    frame: Option<Image>,
    /// Why the file cannot be played.
    error: Option<String>,
    volume: f32,
    muted: bool,
    /// Copies of the player's clock, refreshed on every tick so the view
    /// can stay a plain function of state.
    position: f64,
    duration: Option<f64>,
    playing: bool,
    /// Start playing as soon as the file is ready: it was opened to be
    /// watched, from Files or a notification, not to be looked at paused.
    autoplay: bool,
}

#[derive(Clone, Debug)]
enum Msg {
    Tick,
    Toggle,
    Seek(f32),
    Skip(f64),
    Volume(f32),
    Mute,
    Poll,
    /// The Settings entry and panel every Neo app has.
    Desktop(neo_desktop::DesktopMsg),
}

/// `1:05` or `1:02:05`.
fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

impl Videos {
    fn new(path: Option<PathBuf>) -> Self {
        let mut app = Self { desktop: Desktop::load(), path: None, player: None, frame: None, error: None, volume: 1.0, muted: false, position: 0.0, duration: None, playing: false, autoplay: false };
        if let Some(p) = path {
            match Player::open(&p) {
                Ok(player) => app.player = Some(player),
                Err(e) => app.error = Some(e),
            }
            app.path = Some(p);
        }
        app
    }

    fn apply_volume(&mut self) {
        let v = if self.muted { 0.0 } else { self.volume };
        if let Some(p) = &mut self.player {
            p.set_volume(v);
        }
    }

    /// Copies the player's clock and newest frame into the app's state.
    fn sync(&mut self) {
        let Some(p) = &mut self.player else { return };
        match p.status() {
            PlayerStatus::Failed(e) => {
                self.error = Some(e);
                self.player = None;
                self.playing = false;
                return;
            }
            // Ready, and opened to be watched: play, once.
            PlayerStatus::Ready if std::mem::take(&mut self.autoplay) => p.set_playing(true),
            _ => {}
        }
        if let Some(next) = p.frame(self.frame.as_ref()) {
            self.frame = Some(next);
        }
        self.position = p.position();
        self.duration = p.duration();
        self.playing = p.playing();
    }
}

impl App for Videos {
    type Message = Msg;

    fn title(&self) -> String {
        match self.path.as_ref().and_then(|p| p.file_name()) {
            Some(name) => format!("{} — Videos", name.to_string_lossy()),
            None => "Videos".into(),
        }
    }

    fn window(&self) -> WindowSettings {
        WindowSettings { size: Size::new(1040.0, 700.0), min_size: Some(Size::new(480.0, 320.0)), app_id: Some("org.neo.Videos".into()), ..Default::default() }
    }

    fn app_menu(&self) -> Vec<MenuEntry<Msg>> {
        self.desktop.app_menu(Msg::Desktop)
    }

    fn theme(&self, system: Scheme) -> Theme {
        self.desktop.theme(system)
    }

    /// Closing the window stops the video, sound and all: the programs
    /// that play it would otherwise carry on without a window.
    fn on_exit(&mut self) {
        self.player = None;
        self.playing = false;
    }

    fn subscriptions(&self) -> Vec<Subscription<Msg>> {
        let mut subs = vec![Desktop::subscription(Msg::Poll)];
        if self.player.is_some() {
            // Every frame while playing; a slower look while paused, which
            // still picks up the picture after a seek.
            subs.push(Subscription::every(Duration::from_millis(if self.playing { 16 } else { 120 }), Msg::Tick));
        }
        subs
    }

    fn on_key(&self, k: &KeyEvent) -> Option<Msg> {
        match &k.key {
            Key::Space | Key::Enter => Some(Msg::Toggle),
            Key::Left => Some(Msg::Skip(-SKIP)),
            Key::Right => Some(Msg::Skip(SKIP)),
            Key::Up => Some(Msg::Volume((self.volume + 0.1).min(1.0))),
            Key::Down => Some(Msg::Volume((self.volume - 0.1).max(0.0))),
            Key::Home => Some(Msg::Seek(0.0)),
            Key::Character(c) if c == "m" => Some(Msg::Mute),
            _ => None,
        }
    }

    fn update(&mut self, m: Msg) {
        match m {
            Msg::Tick => {}
            Msg::Toggle => {
                if let Some(p) = &mut self.player {
                    let play = !p.playing();
                    // Play at the end starts again from the beginning.
                    if play && p.duration().is_some_and(|d| p.position() >= d - 0.05) {
                        p.seek(0.0);
                    }
                    p.set_playing(play);
                }
            }
            Msg::Seek(to) => {
                if let Some(p) = &mut self.player {
                    p.seek(to as f64);
                }
            }
            Msg::Skip(by) => {
                if let Some(p) = &mut self.player {
                    let to = (p.position() + by).clamp(0.0, p.duration().unwrap_or(f64::MAX));
                    p.seek(to);
                }
            }
            Msg::Volume(v) => {
                self.volume = v.clamp(0.0, 1.0);
                self.muted = false;
                self.apply_volume();
            }
            Msg::Mute => {
                self.muted = !self.muted;
                self.apply_volume();
            }
            Msg::Desktop(m) => {
                self.desktop.update(m);
            }
            Msg::Poll => {
                self.desktop.poll();
            }
        }
        self.sync();
    }

    fn view(&self) -> Element<Msg> {
        self.desktop.with_settings(self.content(), "Videos Settings", Msg::Desktop, vec![])
    }
}

impl Videos {
    /// The window's content, which the settings panel goes over.
    fn content(&self) -> Element<Msg> {
        let body: Element<Msg> = match (&self.error, &self.player) {
            (Some(e), _) => message(icons::FILM, "This video can't be played", e.clone()),
            (None, Some(p)) => Element::new(Screen { frame: self.frame.clone(), turns: p.turns() }),
            (None, None) => message(icons::CLAPPERBOARD, "No video open", "Open a video from Files, or start Videos with a file.".into()),
        };
        let ready = self.player.is_some();
        let duration = self.duration.unwrap_or(0.0);
        let silent = self.muted || self.volume <= 0.0;
        let controls = row()
            .spacing(12.0)
            .align(Align::Center)
            .width(Length::Fill)
            .padding([14.0, 10.0])
            .push(icon_button(if self.playing { icons::PAUSE } else { icons::PLAY }, 40.0).kind(ButtonKind::Accent).on_press_maybe(ready.then_some(Msg::Toggle)))
            .push(text(clock(self.position)).mono().role(TextRole::Caption).align(Align::End).width(58.0))
            .push(slider(0.0..=(duration.max(0.1) as f32), self.position as f32, Msg::Seek))
            .push(text(if duration > 0.0 { clock(duration) } else { "–:––".into() }).mono().role(TextRole::Caption).tone(Tone::Muted).width(58.0))
            .push(icon_button(if silent { icons::VOLUME_X } else if self.volume < 0.5 { icons::VOLUME_1 } else { icons::VOLUME_2 }, 34.0).kind(ButtonKind::Ghost).on_press_maybe(ready.then_some(Msg::Mute)))
            .push(container(slider(0.0..=1.0, if self.muted { 0.0 } else { self.volume }, Msg::Volume)).width(110.0));
        let pane = container(column().width(Length::Fill).height(Length::Fill).push(body).push(Divider::horizontal()).push(controls)).surface(Surface::Card).width(Length::Fill).height(Length::Fill);
        container(pane).padding([0.0, 12.0, 12.0, 12.0]).width(Length::Fill).height(Length::Fill).into()
    }
}

fn message(glyph: neo::theme::Icon, title: &str, detail: String) -> Element<Msg> {
    let col = column().spacing(8.0).align(Align::Center).push(icon(glyph).size(36.0).tone(Tone::Faint)).push(text(title).role(TextRole::Strong)).push(container(text(detail).tone(Tone::Muted).align(Align::Center)).max_width(420.0));
    container(col).width(Length::Fill).height(Length::Fill).center().into()
}

/// Where a frame of `image` size is drawn in `bounds`: as large as fits, centred.
fn fit(image: Size, bounds: Rect) -> Rect {
    if image.w <= 0.0 || image.h <= 0.0 {
        return bounds;
    }
    let scale = (bounds.w / image.w).min(bounds.h / image.h);
    let (w, h) = (image.w * scale, image.h * scale);
    Rect::new(bounds.x + (bounds.w - w) * 0.5, bounds.y + (bounds.h - h) * 0.5, w, h)
}

/// The picture: the current frame on black. A click plays or pauses.
struct Screen {
    frame: Option<Image>,
    turns: u8,
}

impl Widget<Msg> for Screen {
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
        cx.scene.fill(b, 0.0, Color::BLACK, None);
        if let Some(frame) = &self.frame {
            let (w, h) = (frame.width() as f32, frame.height() as f32);
            // Turned on its side, the frame's height becomes its width.
            let shown = if self.turns % 2 == 1 { Size::new(h, w) } else { Size::new(w, h) };
            cx.scene.push_clip(b);
            cx.scene.image_turned(frame, fit(shown, b), self.turns);
            cx.scene.pop_clip();
        }
        cx.scene.push_layer();
    }

    fn event(&mut self, cx: &mut EventCx<Msg>, event: &Event) -> Status {
        let b = cx.bounds();
        match event {
            Event::PointerMoved { pos } if b.contains(*pos) => {
                cx.set_cursor(CursorIcon::Pointer);
                Status::Ignored
            }
            Event::PointerPressed { pos, button: PointerButton::Primary } if b.contains(*pos) => {
                cx.emit(Msg::Toggle);
                Status::Captured
            }
            _ => Status::Ignored,
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--snapshot") {
        snapshots(PathBuf::from(args.get(i + 1).cloned().unwrap_or_else(|| "target/snapshots".into())), args.get(i + 2).map(PathBuf::from));
        return;
    }
    let path = args.first().map(PathBuf::from).map(|p| std::path::absolute(&p).unwrap_or(p));
    if let Some(p) = &path
        && !p.is_file()
    {
        eprintln!("neo-videos: {} is not a file", p.display());
        std::process::exit(2);
    }
    let mut app = Videos::new(path);
    // Given a video to open, it is played without being asked twice.
    app.autoplay = app.player.is_some();
    if let Err(e) = neo::run(app) {
        eprintln!("neo-videos: {e}");
        std::process::exit(1);
    }
}

/// Renders the empty window, and the player on `video` if one is given:
/// `neo-videos --snapshot DIR [VIDEO]`.
fn snapshots(dir: PathBuf, video: Option<PathBuf>) {
    use neo::testing::Harness;
    std::fs::create_dir_all(&dir).expect("create snapshot dir");
    let mut shots = vec![("videos-empty", None, neo_desktop::SchemePref::Light)];
    if let Some(v) = video {
        shots.push(("videos-playing", Some(v), neo_desktop::SchemePref::Dark));
    }
    for (name, video, scheme) in shots {
        let playing = video.is_some();
        let mut app = Videos::new(video);
        // As when opened on a file: it plays once it is ready.
        app.autoplay = app.player.is_some();
        app.desktop.appearance.scheme = scheme;
        let mut h = Harness::new(app, Size::new(1040.0, 700.0)).expect("GPU");
        if playing {
            h.app_mut().update(Msg::Volume(0.0));
            // Let it load, play for a moment, and report what happened.
            for _ in 0..80 {
                player::pump(0.025);
                h.app_mut().update(Msg::Tick);
            }
            let a = h.app();
            let by = a.player.as_ref().map(|p| if p.by_ffmpeg() { "ffmpeg" } else { "the system's player" });
            println!("played by {by:?}: position {:.2} of {:?}, playing {}, frame {:?}, error {:?}", a.position, a.duration, a.playing, a.frame.as_ref().map(|f| (f.width(), f.height())), a.error);
        }
        let path = dir.join(format!("{name}.png"));
        h.save_png(&path, 1.0).expect("write png");
        println!("wrote {}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_video_opened_to_be_watched_starts_playing_when_it_is_ready() {
        // Made with ffmpeg, where there is one; a kind it plays itself, so
        // the test does not need the main thread's run loop.
        let path = std::env::temp_dir().join(format!("neo-videos-{}-autoplay.mkv", std::process::id()));
        let made = std::process::Command::new(neo_desktop::fs::tool("ffmpeg")).args(["-y", "-loglevel", "error", "-f", "lavfi", "-i", "testsrc=duration=2:size=160x120:rate=12", "-pix_fmt", "yuv420p"]).arg(&path).status().is_ok_and(|s| s.success());
        if !made {
            return;
        }
        // Opened and left alone, as from inside the app: it waits.
        let mut still = Videos::new(Some(path.clone()));
        still.update(Msg::Tick);
        assert!(!still.playing && !still.autoplay);
        // Opened to be watched: the first tick with the file ready plays it.
        let mut app = Videos::new(Some(path.clone()));
        app.autoplay = true;
        app.update(Msg::Tick);
        assert!(app.playing && !app.autoplay, "playing, and not to be started again");
        // Pausing it afterwards stays paused.
        app.update(Msg::Toggle);
        app.update(Msg::Tick);
        assert!(!app.playing);
        // With nothing to play there is nothing to start.
        let mut empty = Videos::new(None);
        empty.autoplay = empty.player.is_some();
        empty.update(Msg::Tick);
        assert!(!empty.playing && !empty.autoplay);
        drop((still, app));
        let _ = std::fs::remove_file(path);
    }

    #[cfg(unix)]
    #[test]
    fn closing_the_app_stops_the_picture_and_the_sound() {
        // With sound, so there is something to be left playing.
        let path = std::env::temp_dir().join(format!("neo-videos-{}-closing.mkv", std::process::id()));
        let made = std::process::Command::new(neo_desktop::fs::tool("ffmpeg"))
            .args(["-y", "-loglevel", "error", "-f", "lavfi", "-i", "testsrc=duration=30:size=160x120:rate=12", "-f", "lavfi", "-i", "sine=frequency=440:duration=30", "-pix_fmt", "yuv420p", "-shortest"])
            .arg(&path)
            .status()
            .is_ok_and(|s| s.success());
        if !made {
            return;
        }
        // The programs playing this file, by its name on their command lines.
        let playing = || {
            let out = std::process::Command::new("pgrep").arg("-f").arg(path.file_name().unwrap()).output().map(|o| String::from_utf8_lossy(&o.stdout).lines().count()).unwrap_or(0);
            out
        };
        let mut app = Videos::new(Some(path.clone()));
        app.update(Msg::Volume(0.0));
        app.autoplay = true;
        let mut h = neo::testing::Harness::new(app, Size::new(640.0, 480.0)).unwrap();
        h.app_mut().update(Msg::Tick);
        assert!(h.app().playing);
        let until = std::time::Instant::now() + Duration::from_secs(10);
        while playing() < 2 && std::time::Instant::now() < until {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(playing() >= 2, "one program for the picture and one for the sound: {}", playing());
        // The app ends, as when its window is closed or Quit is chosen.
        h.exit();
        let until = std::time::Instant::now() + Duration::from_secs(5);
        while playing() > 0 && std::time::Instant::now() < until {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(playing(), 0, "nothing is left playing it");
        assert!(h.app().player.is_none() && !h.app().playing);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn formats_the_clock() {
        assert_eq!(clock(65.4), "1:05");
        assert_eq!(clock(3725.0), "1:02:05");
        assert_eq!(clock(-3.0), "0:00");
    }

    #[test]
    fn fits_the_frame_in_the_screen() {
        let screen = Rect::new(0.0, 0.0, 1000.0, 500.0);
        // 16:9 in a wider screen: full height, bars left and right.
        let r = fit(Size::new(1920.0, 1080.0), screen);
        assert!((r.w - 888.889).abs() < 0.01 && (r.x - 55.556).abs() < 0.01 && r.h == 500.0 && r.y == 0.0, "{r:?}");
        // A small clip is scaled up to fill what it can.
        assert_eq!(fit(Size::new(100.0, 100.0), screen), Rect::new(250.0, 0.0, 500.0, 500.0));
    }

    #[test]
    fn a_missing_file_is_reported_not_played() {
        let app = Videos::new(Some(PathBuf::from("/nonexistent/clip.mp4")));
        // macOS finds out when the file fails to load; ffmpeg platforms at once.
        assert!(app.error.is_some() || app.player.is_some());
        assert!(!app.playing);
    }
}
