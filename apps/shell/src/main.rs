//! NeoShell: the part of the Neo desktop that runs in the background.
//!
//! On a Neo session on Linux this is where the shell will grow: the panel,
//! the dock and the rest. For now it does one job everywhere, which is to
//! show notifications from other Neo apps. They appear at the top right,
//! stay for the time set in Settings, and go again. With motion reduced
//! in Settings they appear and vanish without sliding. One can be put aside
//! to be shown again later: in ten minutes, an hour, or tomorrow.
//!
//!     cargo run -p neo-shell
//!     cargo run -p neo-shell -- --snapshot target/snapshots

use std::path::PathBuf;
use std::time::{Duration, Instant};

use neo::prelude::*;
use neo::{Cx, DrawCx, Event, EventCx, Image, Limits, Point, Proxy, Rect, Size, Status, Widget, WindowGeometry};
use neo_desktop::Desktop;
use neo_desktop::notify::{Inbox, Notification};

mod reminders;
use reminders::{Later, Reminder};

const CARD_W: f32 = 400.0;
const CARD_H: f32 = 112.0;
const GAP: f32 = 10.0;
/// Room around the cards, which also holds their shadows.
const MARGIN: f32 = 22.0;
/// How many show at once; older ones make way for newer.
const MOST: usize = 4;
const SLIDE_IN: Duration = Duration::from_millis(260);
const SLIDE_OUT: Duration = Duration::from_millis(200);
/// The longest side of a notification's picture, in pixels.
const THUMB_SIDE: u32 = 256;

/// A notification on screen.
struct Card {
    id: u64,
    note: Notification,
    picture: Option<Image>,
    /// When it appeared, and when it started to leave.
    born: Instant,
    leaving: Option<Instant>,
    /// Being asked when to show it again, which keeps it on screen.
    choosing: bool,
    /// Shown again after being put aside.
    reminder: bool,
}

struct Shell {
    desktop: Desktop,
    cards: Vec<Card>,
    next_id: u64,
    /// The time the cards are drawn for.
    now: Instant,
    screen: Rect,
    /// Notifications put aside to show again, and the file they are kept in.
    reminders: Vec<Reminder>,
    kept_in: Option<PathBuf>,
    proxy: Option<Proxy<Msg>>,
}

/// A picture read and shrunk off the main thread: its size and pixels.
type Pixels = (u32, u32, Vec<u8>);

#[derive(Clone, Debug)]
enum Msg {
    Notify(Notification, Option<Pixels>),
    /// A notification that was put aside, shown again now its time has come.
    Remind(Notification, Option<Pixels>),
    /// Ask when to show a card again, or stop asking.
    Choose(u64),
    /// Put a card aside to show again later.
    Later(u64, Later),
    /// Time has passed: move the cards along.
    Tick,
    Dismiss(u64),
    /// Show the file a notification is about in Files.
    Reveal(u64),
    /// Open the file a notification is about: its card was clicked.
    Open(u64),
    Geometry(WindowGeometry),
    Poll,
}

/// Holds a card `by` pixels to the right of its place, at its full width.
/// A row would squeeze the card to fit instead; this lets it hang past the
/// window's edge, which hides the part that has not arrived.
struct Shifted<M> {
    card: [Element<M>; 1],
    by: f32,
}

impl<M: 'static> Widget<M> for Shifted<M> {
    fn children_mut(&mut self) -> &mut [Element<M>] {
        &mut self.card
    }

    fn layout(&mut self, cx: &mut Cx, limits: Limits) -> Size {
        let size = self.card[0].layout(cx, Limits::loose(Size::new(CARD_W, f32::INFINITY)));
        self.card[0].set_position(Point::new(self.by, 0.0));
        Size::new(limits.max.w.min(CARD_W), size.h)
    }

    fn draw(&self, cx: &mut DrawCx) {
        self.card[0].draw(cx);
    }

    fn event(&mut self, cx: &mut EventCx<M>, event: &Event) -> Status {
        self.card[0].event(cx, event)
    }
}

/// Reads a notification's picture and shrinks it to thumbnail size.
fn thumbnail(path: &std::path::Path) -> Option<Pixels> {
    let picture = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?.decode().ok()?;
    let small = if picture.width().max(picture.height()) > THUMB_SIDE { picture.thumbnail(THUMB_SIDE, THUMB_SIDE) } else { picture }.into_rgba8();
    let (w, h) = small.dimensions();
    Some((w, h, small.into_raw()))
}

/// Eases a 0 to 1 progress so movement starts fast and settles.
fn ease(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

impl Shell {
    fn new() -> Self {
        // Tests and snapshots keep clear of the reminders that are really set.
        let kept_in = (!cfg!(test)).then(reminders::file);
        Self { desktop: Desktop::load(), cards: vec![], next_id: 1, now: Instant::now(), screen: Rect::ZERO, reminders: vec![], kept_in, proxy: None }
    }

    /// Puts a card aside, to be shown again `later` after `wall`.
    fn remind(&mut self, id: u64, later: Later, wall: u64, now: Instant) {
        let Some(c) = self.cards.iter_mut().find(|c| c.id == id && c.leaving.is_none()) else { return };
        c.choosing = false;
        self.reminders.push(Reminder { due: wall + later.seconds(), note: c.note.clone() });
        self.keep();
        self.dismiss(id, now);
    }

    fn keep(&self) {
        if let Some(path) = &self.kept_in {
            reminders::save(path, &self.reminders);
        }
    }

    /// Takes out the reminders whose time has come by `wall`, soonest first.
    fn due(&mut self, wall: u64) -> Vec<Notification> {
        let (mut due, waiting): (Vec<Reminder>, Vec<Reminder>) = std::mem::take(&mut self.reminders).into_iter().partition(|r| r.due <= wall);
        self.reminders = waiting;
        if !due.is_empty() {
            self.keep();
        }
        due.sort_by_key(|r| r.due);
        due.into_iter().map(|r| r.note).collect()
    }

    /// Shows the reminders that are due. Their pictures are read off the
    /// main thread, as a new notification's is.
    fn wake(&mut self, wall: u64, now: Instant) {
        for note in self.due(wall) {
            match self.proxy.clone() {
                Some(proxy) if note.image.is_some() => {
                    std::thread::spawn(move || {
                        let pixels = note.image.as_deref().and_then(thumbnail);
                        proxy.send(Msg::Remind(note, pixels));
                    });
                }
                _ => self.show(note, None, true, now),
            }
        }
    }

    fn still(&self) -> bool {
        self.desktop.appearance.reduce_motion
    }

    /// How long a card stays before it starts to leave.
    fn stay(&self) -> Duration {
        Duration::from_secs_f32(self.desktop.appearance.notification_seconds)
    }

    fn add(&mut self, note: Notification, pixels: Option<Pixels>, now: Instant) {
        self.show(note, pixels, false, now);
    }

    fn show(&mut self, note: Notification, pixels: Option<Pixels>, reminder: bool, now: Instant) {
        let picture = pixels.map(|(w, h, rgba)| Image::new(w, h, rgba));
        self.cards.push(Card { id: self.next_id, note, picture, born: now, leaving: None, choosing: false, reminder });
        self.next_id += 1;
        // Too many to show: the oldest that is not already going goes.
        let staying: Vec<usize> = (0..self.cards.len()).filter(|i| self.cards[*i].leaving.is_none()).collect();
        for i in staying.iter().take(staying.len().saturating_sub(MOST)) {
            self.cards[*i].leaving = Some(now);
        }
        self.step(now);
    }

    fn dismiss(&mut self, id: u64, now: Instant) {
        if let Some(c) = self.cards.iter_mut().find(|c| c.id == id && c.leaving.is_none()) {
            c.leaving = Some(now);
        }
        self.step(now);
    }

    /// Moves time on to `now`: cards that have stayed their time start to
    /// leave, and those that have left are forgotten.
    fn step(&mut self, now: Instant) {
        self.now = now;
        let (stay, out) = (self.stay(), if self.still() { Duration::ZERO } else { SLIDE_OUT });
        for c in &mut self.cards {
            if c.leaving.is_none() && !c.choosing && now.saturating_duration_since(c.born) >= stay {
                c.leaving = Some(now);
            }
        }
        self.cards.retain(|c| c.leaving.is_none_or(|l| now.saturating_duration_since(l) < out));
    }

    /// How far a card sits to the right of its place, from fully out of
    /// the window (1) to in place (0).
    fn offset(&self, c: &Card) -> f32 {
        if self.still() {
            return 0.0;
        }
        let since = |t: Instant, over: Duration| self.now.saturating_duration_since(t).as_secs_f32() / over.as_secs_f32();
        match c.leaving {
            Some(left) => ease(since(left, SLIDE_OUT)),
            None => 1.0 - ease(since(c.born, SLIDE_IN)),
        }
    }

    /// Whether any card is part-way through sliding.
    fn moving(&self) -> bool {
        !self.still() && self.cards.iter().any(|c| c.leaving.is_some() || self.now.saturating_duration_since(c.born) < SLIDE_IN)
    }

    fn size(&self) -> Size {
        let n = self.cards.len().max(1) as f32;
        Size::new(CARD_W + MARGIN * 2.0, n * CARD_H + (n - 1.0) * GAP + MARGIN * 2.0)
    }

    fn card(&self, c: &Card) -> Element<Msg> {
        let title = text(c.note.title.clone()).role(TextRole::Strong).no_wrap();
        let mut words = column().spacing(2.0).width(Length::Fill);
        // One that was put aside says so, with a bell before its title.
        words = if c.reminder { words.push(row().spacing(6.0).align(Align::Center).push(icon(icons::BELL).size(13.0).tone(Tone::Accent)).push(title)) } else { words.push(title) };
        if c.choosing {
            // In place of what it says: when to say it again.
            let choices = Later::ALL.iter().fold(row().spacing(4.0), |r, l| r.push(Button::new(text(l.label()).role(TextRole::Caption)).kind(ButtonKind::Ghost).padding([8.0, 3.0]).radius(6.0).on_press(Msg::Later(c.id, *l))));
            words = words.push(text("Remind me in").role(TextRole::Caption).tone(Tone::Muted).no_wrap()).push(Space::new(0.0, 4.0)).push(choices);
        } else if !c.note.body.is_empty() {
            // Written as a link where clicking the card opens the file.
            words = words.push(text(c.note.body.clone()).role(TextRole::Caption).tone(if c.note.open.is_some() { Tone::Accent } else { Tone::Muted }).no_wrap());
        }
        if c.note.reveal.is_some() && !c.choosing {
            let link = row().spacing(5.0).align(Align::Center).push(icon(icons::FOLDER_OPEN).size(13.0)).push(text("Show in Files").role(TextRole::Caption));
            words = words.push(Space::new(0.0, 4.0)).push(Button::new(link).kind(ButtonKind::Ghost).padding([8.0, 3.0]).radius(6.0).on_press(Msg::Reveal(c.id)));
        }
        let mut body = row().spacing(12.0).align(Align::Center).width(Length::Fill);
        if let Some(p) = &c.picture {
            body = body.push(container(picture(p).fit(Fit::Cover).width(52.0).height(52.0)).width(52.0).height(52.0));
        }
        let later = icon_button(icons::ALARM_CLOCK, 24.0).kind(if c.choosing { ButtonKind::Raised } else { ButtonKind::Ghost }).on_press(Msg::Choose(c.id));
        body = body.push(words).push(row().spacing(2.0).push(later).push(icon_button(icons::X, 24.0).kind(ButtonKind::Ghost).on_press(Msg::Dismiss(c.id))));
        let inside: Element<Msg> = container(body).height(CARD_H - 40.0).align_y(Align::Center).into();
        // The card itself opens the file, anywhere its buttons are not.
        let id = c.id;
        let inside = if c.note.open.is_some() && !c.choosing { mouse_area(inside).on_press(move || Msg::Open(id)).into() } else { inside };
        container(neo_desktop::ui::sheet(inside, CARD_W)).height(CARD_H).into()
    }
}

impl App for Shell {
    type Message = Msg;

    fn title(&self) -> String {
        "NeoShell".into()
    }

    fn window(&self) -> WindowSettings {
        WindowSettings { size: self.size(), min_size: None, resizable: false, app_id: Some("org.neo.Shell".into()), ..Default::default() }
    }

    fn theme(&self, system: Scheme) -> Theme {
        let mut theme = self.desktop.theme(system);
        // Cards float over anything, so they stay solid and readable.
        theme.glass.enabled = false;
        theme
    }

    fn window_state(&self) -> WindowState {
        let size = self.size();
        // Top right, clear of a menu bar or panel.
        let position = (self.screen != Rect::ZERO).then(|| Point::new((self.screen.right() - size.w - 4.0).round(), (self.screen.y + 30.0).round()));
        // Shown only while there is something to say, and never taking the
        // keyboard from what the user is doing.
        WindowState { visible: !self.cards.is_empty(), always_on_top: true, bare: true, size: Some(size), position, hidden_from_capture: false, passive: true }
    }

    fn on_window_geometry(&self, geometry: WindowGeometry) -> Option<Msg> {
        Some(Msg::Geometry(geometry))
    }

    fn start(&mut self, proxy: Proxy<Msg>) {
        self.proxy = Some(proxy.clone());
        // Those set before, with any whose time came while this was not running.
        if let Some(path) = &self.kept_in {
            self.reminders = reminders::load(path);
        }
        self.wake(reminders::wall(), Instant::now());
        std::thread::spawn(move || {
            let inbox = match Inbox::open() {
                Ok(inbox) => inbox,
                Err(e) => return eprintln!("neo-shell: could not listen for notifications: {e}"),
            };
            while let Ok(note) = inbox.recv() {
                let pixels = note.image.as_deref().and_then(thumbnail);
                if !proxy.send(Msg::Notify(note, pixels)) {
                    break;
                }
            }
        });
    }

    fn subscriptions(&self) -> Vec<Subscription<Msg>> {
        let mut subs = vec![Desktop::subscription(Msg::Poll)];
        if self.moving() {
            subs.push(Subscription::every(Duration::from_millis(16), Msg::Tick));
        } else if !self.cards.is_empty() {
            // Nothing is sliding: only the wait for a card's time to be up.
            subs.push(Subscription::every(Duration::from_millis(100), Msg::Tick));
        } else if !self.reminders.is_empty() {
            // Nothing on screen: only the wait for a reminder to come due.
            subs.push(Subscription::every(Duration::from_secs(5), Msg::Tick));
        }
        subs
    }

    fn update(&mut self, m: Msg) {
        let now = Instant::now();
        match m {
            Msg::Notify(note, pixels) => self.add(note, pixels, now),
            Msg::Remind(note, pixels) => self.show(note, pixels, true, now),
            Msg::Choose(id) => {
                if let Some(c) = self.cards.iter_mut().find(|c| c.id == id && c.leaving.is_none()) {
                    c.choosing = !c.choosing;
                    // Changing its mind, it stays as long as a new one would.
                    c.born = now.checked_sub(SLIDE_IN).unwrap_or(now);
                }
            }
            Msg::Later(id, later) => self.remind(id, later, reminders::wall(), now),
            Msg::Tick => {
                self.step(now);
                self.wake(reminders::wall(), now);
            }
            Msg::Dismiss(id) => self.dismiss(id, now),
            Msg::Reveal(id) => {
                if let Some(path) = self.cards.iter().find(|c| c.id == id).and_then(|c| c.note.reveal.clone()) {
                    let _ = neo_desktop::fs::reveal(&path);
                }
                self.dismiss(id, now);
            }
            Msg::Open(id) => {
                if let Some(path) = self.cards.iter().find(|c| c.id == id).and_then(|c| c.note.open.clone()) {
                    // Tests must not start whatever viewers this computer has.
                    if !cfg!(test) {
                        let _ = neo_desktop::fs::open_file(&path);
                    }
                    self.dismiss(id, now);
                }
            }
            Msg::Geometry(g) => self.screen = g.screen,
            Msg::Poll => {
                self.desktop.poll();
            }
        }
    }

    fn view(&self) -> Element<Msg> {
        // Each card starts to the right of its place and slides in; the
        // window's edge hides the part that has not arrived.
        let slide = CARD_W + MARGIN;
        self.cards.iter().fold(column().spacing(GAP).padding(MARGIN).width(Length::Fill), |col, c| col.push(Element::new(Shifted { card: [self.card(c)], by: (self.offset(c) * slide).round() }))).into()
    }
}

/// Keeps NeoShell starting at login, for an installed copy, so that
/// notifications have somewhere to go.
fn keep_at_startup() {
    let installed = std::env::current_exe().is_ok_and(|p| !p.components().any(|c| c.as_os_str() == "target"));
    let Ok(program) = std::env::current_exe() else { return };
    let entry = neo_desktop::autostart::Entry { id: "org.neo.Shell", name: "NeoShell", program: &program, args: &[] };
    if installed && !neo_desktop::autostart::is_enabled(&entry) {
        let _ = neo_desktop::autostart::enable(&entry);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--snapshot") {
        snapshots(PathBuf::from(args.get(i + 1).cloned().unwrap_or_else(|| "target/snapshots".into())));
        return;
    }
    // `neo-shell --notify "Title" ["Body"] [--image file] [--reveal file] [--open file]`
    // sends a notification to the NeoShell that is running, for scripts.
    if let Some(i) = args.iter().position(|a| a == "--notify") {
        let value = |flag: &str| args.iter().position(|a| a == flag).and_then(|j| args.get(j + 1)).map(PathBuf::from);
        let words: Vec<&String> = args[i + 1..].iter().take_while(|a| !a.starts_with("--")).collect();
        let Some(title) = words.first() else {
            eprintln!("neo-shell: --notify needs a title");
            std::process::exit(2);
        };
        let note = Notification { title: (*title).clone(), body: words.get(1).map(|b| (*b).clone()).unwrap_or_default(), image: value("--image"), reveal: value("--reveal"), open: value("--open") };
        if !note.send() {
            eprintln!("neo-shell: NeoShell is not running, so there is nowhere to show that");
            std::process::exit(1);
        }
        return;
    }
    // `neo-shell --reminders` lists the notifications put aside for later.
    if args.iter().any(|a| a == "--reminders") {
        let (mut all, now) = (reminders::load(&reminders::file()), reminders::wall());
        all.sort_by_key(|r| r.due);
        for r in &all {
            println!("{}: {}{}", reminders::when(r.due, now), r.note.title, if r.note.body.is_empty() { String::new() } else { format!(" ({})", r.note.body) });
        }
        if all.is_empty() {
            println!("No reminders are set.");
        }
        return;
    }
    // One is enough: a second would take the notifications from the first.
    if neo_desktop::notify::shell_running() {
        return;
    }
    keep_at_startup();
    if let Err(e) = neo::run(Shell::new()) {
        eprintln!("neo-shell: {e}");
        std::process::exit(1);
    }
}

/// A small picture to stand in for a screenshot in snapshots and tests.
fn sample_pixels() -> Pixels {
    let (w, h) = (64u32, 40u32);
    let rgba = (0..w * h).flat_map(|i| [(i % w * 4) as u8, (i / w * 6) as u8, 180, 255]).collect();
    (w, h, rgba)
}

fn snapshots(dir: PathBuf) {
    use neo::testing::Harness;
    std::fs::create_dir_all(&dir).expect("create snapshot dir");
    for (name, scheme) in [("shell-notifications", neo_desktop::SchemePref::Light), ("shell-notifications-dark", neo_desktop::SchemePref::Dark)] {
        let mut app = Shell::new();
        app.desktop.appearance.scheme = scheme;
        let start = Instant::now();
        app.add(Notification::new("Screenshot copied to clipboard", "Neo Screenshot 2026-10-06 at 09.41.07.png").reveal("/tmp/shot.png"), Some(sample_pixels()), start);
        app.add(Notification::new("Recording saved", "Neo Recording 2026-10-06 at 09.42.30.mov").reveal("/tmp/rec.mov"), None, start);
        // Settled, with a third half-way in.
        app.cards[1].choosing = true;
        app.cards[0].reminder = true;
        app.add(Notification::new("Backup finished", ""), None, start + Duration::from_secs(1));
        app.step(start + Duration::from_secs(1) + SLIDE_IN / 3);
        let size = app.size();
        let mut h = Harness::new(app, size).expect("GPU");
        let path = dir.join(format!("{name}.png"));
        h.save_png(&path, 2.0).expect("write png");
        println!("wrote {}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell() -> (Shell, Instant) {
        let mut s = Shell::new();
        s.desktop.appearance.reduce_motion = false;
        s.desktop.appearance.notification_seconds = 5.0;
        (s, Instant::now())
    }

    fn note(title: &str) -> Notification {
        Notification::new(title, "body")
    }

    #[test]
    fn a_notification_shows_stays_its_time_and_goes() {
        let (mut s, t) = shell();
        assert!(!s.window_state().visible, "nothing to say, nothing on screen");
        s.add(note("One"), None, t);
        let w = s.window_state();
        assert!(w.visible && w.passive && w.always_on_top && w.bare, "shown without taking the keyboard");
        s.step(t + Duration::from_millis(4900));
        assert_eq!((s.cards.len(), s.cards[0].leaving), (1, None), "still there just before its time is up");
        s.step(t + Duration::from_secs(5));
        assert!(s.cards[0].leaving.is_some(), "then it starts to leave");
        s.step(t + Duration::from_secs(5) + SLIDE_OUT);
        assert!(s.cards.is_empty() && !s.window_state().visible, "and is gone once it has slid out");
    }

    #[test]
    fn the_stay_time_comes_from_settings() {
        let (mut s, t) = shell();
        s.desktop.appearance.notification_seconds = 2.0;
        s.add(note("One"), None, t);
        s.step(t + Duration::from_millis(2100));
        assert!(s.cards[0].leaving.is_some());
    }

    #[test]
    fn cards_slide_in_from_the_right_and_back_out() {
        let (mut s, t) = shell();
        s.add(note("One"), None, t);
        assert_eq!(s.offset(&s.cards[0]), 1.0, "starts out of the window");
        assert!(s.moving());
        s.step(t + SLIDE_IN / 2);
        let half = s.offset(&s.cards[0]);
        assert!(half > 0.0 && half < 0.5, "most of the way in by half time, then settling: {half}");
        s.step(t + SLIDE_IN);
        assert_eq!(s.offset(&s.cards[0]), 0.0);
        assert!(!s.moving(), "at rest, so no need to redraw every frame");
        s.dismiss(s.cards[0].id, t + Duration::from_secs(1));
        assert!(s.moving());
        s.step(t + Duration::from_secs(1) + SLIDE_OUT / 2);
        assert!(s.offset(&s.cards[0]) > 0.5, "on its way out");
    }

    #[test]
    fn with_motion_reduced_cards_appear_and_vanish_in_place() {
        let (mut s, t) = shell();
        s.desktop.appearance.reduce_motion = true;
        s.add(note("One"), None, t);
        assert_eq!(s.offset(&s.cards[0]), 0.0, "in place at once");
        assert!(!s.moving());
        s.dismiss(s.cards[0].id, t + Duration::from_secs(1));
        assert!(s.cards.is_empty(), "and gone at once");
    }

    #[test]
    fn closing_one_leaves_the_others_and_the_window_fits_what_is_left() {
        let (mut s, t) = shell();
        s.add(note("One"), None, t);
        s.add(note("Two"), None, t);
        let two = s.size();
        assert_eq!(two.h, CARD_H * 2.0 + GAP + MARGIN * 2.0);
        let first = s.cards[0].id;
        s.update(Msg::Dismiss(first));
        s.step(Instant::now() + SLIDE_OUT);
        assert_eq!(s.cards.iter().map(|c| c.note.title.as_str()).collect::<Vec<_>>(), ["Two"]);
        assert!(s.size().h < two.h);
        // Dismissing one that has gone does nothing.
        s.update(Msg::Dismiss(first));
        assert_eq!(s.cards.len(), 1);
    }

    #[test]
    fn a_flood_keeps_only_the_newest_few() {
        let (mut s, t) = shell();
        for i in 0..MOST + 2 {
            s.add(note(&format!("Note {i}")), None, t);
        }
        let staying: Vec<&str> = s.cards.iter().filter(|c| c.leaving.is_none()).map(|c| c.note.title.as_str()).collect();
        assert_eq!(staying, ["Note 2", "Note 3", "Note 4", "Note 5"]);
        s.step(t + SLIDE_OUT);
        assert_eq!(s.cards.len(), MOST);
    }

    #[test]
    fn the_window_sits_at_the_top_right_of_the_screen() {
        let (mut s, t) = shell();
        s.add(note("One"), None, t);
        assert_eq!(s.window_state().position, None, "not placed until the screen is known");
        s.update(Msg::Geometry(WindowGeometry { frame: Rect::ZERO, screen: Rect::new(0.0, 0.0, 1440.0, 900.0), scale: 2.0 }));
        let w = s.window_state();
        let (p, size) = (w.position.unwrap(), w.size.unwrap());
        assert_eq!(p.x + size.w, 1436.0);
        assert_eq!(p.y, 30.0);
    }

    #[test]
    fn a_card_draws_its_picture_text_and_link() {
        use neo::testing::Harness;
        let (mut s, t) = shell();
        s.desktop.appearance.reduce_motion = true;
        s.add(Notification::new("Screenshot copied to clipboard", "shot.png").reveal("/tmp/shot.png"), Some(sample_pixels()), t);
        assert!(s.cards[0].picture.is_some());
        let size = s.size();
        let mut h = Harness::new(s, size).unwrap();
        let px = h.render(1.0);
        let at = |x: usize, y: usize| px[(y * size.w as usize + x) * 4 + 3];
        assert_eq!(at(2, 2), 0, "the window is see-through around the card");
        assert_eq!(at(size.w as usize / 2, size.h as usize / 2), 255, "and the card is solid");
        // The close button, at the card's right edge.
        h.click(neo::Point::new(MARGIN + CARD_W - 32.0, MARGIN + CARD_H / 2.0));
        assert!(h.app().cards.is_empty(), "closing it removes it");
    }

    #[test]
    fn clicking_a_card_opens_its_file_and_its_buttons_still_do_their_own_thing() {
        use neo::testing::Harness;
        let (mut s, t) = shell();
        s.add(Notification::new("Recording saved", "clip.mov").reveal("/tmp/clip.mov").open("/tmp/clip.mov"), None, t);
        // Once it has slid into place.
        s.step(t + SLIDE_IN * 2);
        let size = s.size();
        let mut h = Harness::new(s, size).unwrap();
        h.render(1.0);
        // Over the words, the pointer says the card can be clicked.
        let words = neo::Point::new(MARGIN + 60.0, MARGIN + 34.0);
        h.move_to(words);
        assert_eq!(h.cursor(), neo::CursorIcon::Pointer);
        h.click(words);
        assert!(h.app().cards.iter().all(|c| c.leaving.is_some()), "opened, and on its way out");

        // One with nothing to open is not a button: clicking it leaves it be.
        let (mut s, t) = shell();
        s.add(Notification::new("Backup finished", "All files copied"), None, t);
        s.step(t + SLIDE_IN * 2);
        let size = s.size();
        let mut h = Harness::new(s, size).unwrap();
        h.render(1.0);
        h.move_to(words);
        assert_eq!(h.cursor(), neo::CursorIcon::Default);
        h.click(words);
        assert_eq!((h.app().cards.len(), h.app().cards[0].leaving), (1, None));
        // A message to open a card that has nothing to open does nothing.
        let id = h.app().cards[0].id;
        h.app_mut().update(Msg::Open(id));
        assert_eq!(h.app().cards[0].leaving, None);
        // The file travels with the notification.
        let sent = Notification::new("Screenshot saved", "shot.png").open("/tmp/a shot.png");
        assert_eq!(Notification::decode(&sent.encode()).and_then(|n| n.open), Some("/tmp/a shot.png".into()));
    }

    #[test]
    fn a_notification_put_aside_comes_back_when_its_time_is_up() {
        use neo::testing::Harness;
        let (mut s, t) = shell();
        let path = std::env::temp_dir().join(format!("neo-shell-later-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        s.kept_in = Some(path.clone());
        s.add(Notification::new("Recording saved", "clip.mov").reveal("/tmp/clip.mov").open("/tmp/clip.mov"), None, t);
        s.step(t + SLIDE_IN * 2);
        let size = s.size();
        let mut h = Harness::new(s, size).unwrap();
        h.render(1.0);
        // The clock, left of the close button.
        h.click(neo::Point::new(MARGIN + CARD_W - 58.0, MARGIN + CARD_H / 2.0));
        assert!(h.app().cards[0].choosing, "asked when");
        // While it asks, it does not go on its own, and clicking it opens nothing.
        h.app_mut().step(t + Duration::from_secs(60));
        assert_eq!((h.app().cards.len(), h.app().cards[0].leaving), (1, None));
        h.render(1.0);
        h.click(neo::Point::new(MARGIN + 200.0, MARGIN + 30.0));
        assert_eq!(h.app().cards[0].leaving, None);
        // The clock again takes the question away, and the card stays a while.
        let id = h.app().cards[0].id;
        h.app_mut().update(Msg::Choose(id));
        assert!(!h.app().cards[0].choosing);
        h.app_mut().step(Instant::now() + Duration::from_secs(4));
        assert_eq!(h.app().cards[0].leaving, None);
        h.app_mut().update(Msg::Choose(id));
        h.render(1.0);
        // The first of the choices, under the words.
        h.click(neo::Point::new(MARGIN + 60.0, MARGIN + 74.0));
        let s = h.app_mut();
        assert!(s.cards[0].leaving.is_some(), "put aside, and on its way out");
        assert_eq!(s.reminders.len(), 1);
        let due = s.reminders[0].due;
        assert!(due.abs_diff(reminders::wall() + 600) <= 2, "ten minutes from now");
        assert_eq!(reminders::load(&path), s.reminders, "and written down");

        // A NeoShell started later finds it, and shows it when it is due.
        let (mut s, t) = shell();
        s.kept_in = Some(path.clone());
        s.reminders = reminders::load(&path);
        s.wake(due - 1, t);
        assert!(s.cards.is_empty() && !s.window_state().visible, "not yet");
        assert!(!s.subscriptions().is_empty(), "but watching the time");
        s.wake(due, t);
        assert_eq!(s.cards.iter().map(|c| (c.note.title.as_str(), c.reminder, c.note.open.is_some())).collect::<Vec<_>>(), [("Recording saved", true, true)]);
        assert!(s.reminders.is_empty() && !path.exists(), "shown once, and no more kept");
        s.wake(due + 600, t);
        assert_eq!(s.cards.len(), 1);
    }

    #[test]
    fn reminders_come_back_soonest_first_and_can_be_put_aside_again() {
        let (mut s, t) = shell();
        s.add(note("Later"), None, t);
        s.add(note("Sooner"), None, t);
        let (later, sooner) = (s.cards[0].id, s.cards[1].id);
        s.remind(later, Later::Tomorrow, 1000, t);
        s.remind(sooner, Later::Hour, 1000, t);
        // Putting aside one that has gone does nothing.
        s.remind(sooner, Later::Hour, 1000, t);
        assert_eq!(s.reminders.iter().map(|r| r.due).collect::<Vec<_>>(), [1000 + 86_400, 1000 + 3600]);
        s.step(t + SLIDE_OUT);
        assert!(s.cards.is_empty());
        s.wake(1000 + 86_400, t);
        assert_eq!(s.cards.iter().map(|c| c.note.title.as_str()).collect::<Vec<_>>(), ["Sooner", "Later"]);
        let again = s.cards[0].id;
        s.remind(again, Later::TenMinutes, 90_000, t);
        assert_eq!(s.reminders.iter().map(|r| (r.note.title.as_str(), r.due)).collect::<Vec<_>>(), [("Sooner", 90_600)]);
    }

    #[test]
    fn a_picture_is_shrunk_to_a_thumbnail() {
        let path = std::env::temp_dir().join(format!("neo-shell-thumb-{}.png", std::process::id()));
        image::RgbaImage::from_pixel(1200, 400, image::Rgba([10, 200, 30, 255])).save(&path).unwrap();
        let (w, h, rgba) = thumbnail(&path).expect("a PNG is read");
        assert_eq!((w, h), (256, 85), "the longest side is brought down, keeping the shape");
        assert_eq!(rgba.len(), (w * h * 4) as usize);
        assert_eq!(&rgba[..4], [10, 200, 30, 255]);
        std::fs::remove_file(&path).unwrap();
        assert!(thumbnail(std::path::Path::new("/nowhere/at/all.png")).is_none());
    }
}
