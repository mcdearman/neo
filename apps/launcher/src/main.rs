//! Launcher: type a few letters to find and open an app.
//!
//!     cargo run -p neo-launcher            # opens the search window
//!     cargo run -p neo-launcher -- --hidden
//!     cargo run -p neo-launcher -- --snapshot target/snapshots
//!
//! Command+' on macOS and Ctrl+' on Windows bring it up from anywhere while
//! it runs in the background; the installed app starts at login for that.
//! Enter opens the top match, Up and Down choose another, and Escape, or a
//! click elsewhere, puts it away. The search is fuzzy and always has an
//! answer: letters match in order with gaps allowed, and when nothing
//! matches the closest names are shown. Apps opened often rank higher.
//!
//! On Linux no shortcut is taken yet: bind `neo-launcher` to a key in the
//! window manager, and it opens, launches and exits.

// Release builds on Windows open no console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod apps;
mod fuzzy;

use std::collections::HashMap;
use std::path::PathBuf;

use global_hotkey::hotkey::{Code, HotKey, Modifiers as HotMods};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use neo::prelude::*;
use neo::{Image, Key, KeyEvent, Point, Proxy, Rect, Size, WindowGeometry, WindowState};
use neo_desktop::Desktop;

use apps::AppEntry;

/// How many matches are listed.
const ROWS: usize = 8;
const ROW_H: f32 = 46.0;
const WIDTH: f32 = 640.0;
/// The search field, the rows and the hint under them.
const HEIGHT: f32 = 66.0 + ROWS as f32 * ROW_H + 44.0;
const SHORTCUT: &str = if cfg!(target_os = "macos") { "⌘'" } else { "Ctrl+'" };
/// The bar for asking Apollo is the search field and a line under it.
const ASK_HEIGHT: f32 = 66.0 + 44.0;
const ASK_SHORTCUT: &str = if cfg!(target_os = "macos") { "⌘⇧A" } else { "Ctrl+Shift+A" };

struct Launcher {
    desktop: Desktop,
    apps: Vec<AppEntry>,
    icons: HashMap<PathBuf, Image>,
    /// How many times each app has been opened from here.
    uses: HashMap<PathBuf, u32>,
    query: String,
    /// The indices into `apps` of the best matches, best first.
    results: Vec<usize>,
    selected: usize,
    shown: bool,
    /// Whether the window has had keyboard focus since it was last shown.
    /// Losing focus only means "the user went elsewhere" after that.
    focused: bool,
    /// Counts each time the window is shown, to give the search field
    /// focus afresh.
    showings: u64,
    screen: Rect,
    error: Option<String>,
    /// The window is the bar for asking Apollo, and not the list of apps.
    asking: bool,
    /// Kept alive so the shortcut stays registered.
    hotkeys: Option<GlobalHotKeyManager>,
    /// With no shortcut to bring it back, putting the window away quits.
    register_hotkey: bool,
    quit: bool,
}

#[derive(Clone, Debug)]
enum Msg {
    Query(String),
    Move(isize),
    Launch,
    LaunchAt(usize),
    Toggle,
    /// Show the bar for asking Apollo, or put it away.
    ToggleAsk,
    Hide,
    Focus(bool),
    Geometry(WindowGeometry),
    Icons(Vec<(PathBuf, Image)>),
    Poll,
}

fn history_path() -> PathBuf {
    neo_desktop::config_dir().join("launcher-history")
}

fn load_uses() -> HashMap<PathBuf, u32> {
    std::fs::read_to_string(history_path())
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let (count, path) = l.split_once('\t')?;
            Some((PathBuf::from(path), count.parse().ok()?))
        })
        .collect()
}

fn save_uses(uses: &HashMap<PathBuf, u32>) {
    let path = history_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut lines: Vec<String> = uses.iter().map(|(p, n)| format!("{n}\t{}", p.display())).collect();
    lines.sort();
    let _ = std::fs::write(path, lines.join("\n") + "\n");
}

impl Launcher {
    fn new(register_hotkey: bool, apps: Vec<AppEntry>, uses: HashMap<PathBuf, u32>) -> Self {
        let mut l = Self { desktop: Desktop::load(), apps, icons: HashMap::new(), uses, query: String::new(), results: vec![], selected: 0, asking: false, shown: true, focused: false, showings: 0, screen: Rect::ZERO, error: None, hotkeys: None, register_hotkey, quit: false };
        l.search();
        l
    }

    /// Ranks every app against the query and keeps the best.
    fn search(&mut self) {
        let mut ranked: Vec<(i64, usize)> = self
            .apps
            .iter()
            .enumerate()
            .map(|(i, app)| {
                // Apps opened often get a nudge, never enough to beat a much better match.
                let habit = self.uses.get(&app.path).copied().unwrap_or(0).min(20) as i64;
                (fuzzy::score(&self.query, &app.name).0 as i64 + habit * 4, i)
            })
            .collect();
        // With nothing typed every score is zero: most used first, then by name.
        ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| self.apps[a.1].name.to_lowercase().cmp(&self.apps[b.1].name.to_lowercase())));
        self.results = ranked.into_iter().take(ROWS).map(|(_, i)| i).collect();
        self.selected = 0;
    }

    fn show(&mut self) {
        self.shown = true;
        self.focused = false;
        self.showings += 1;
        self.query.clear();
        self.error = None;
        self.search();
    }

    fn hide(&mut self) {
        self.shown = false;
        self.focused = false;
        // Nothing would bring a hidden window back without the shortcut.
        if self.hotkeys.is_none() {
            self.quit = true;
        }
    }

    /// Hands what was typed to Apollo, which comes up with it, and puts the bar away.
    fn ask(&mut self) {
        let question = self.query.trim().to_owned();
        if question.is_empty() {
            return;
        }
        // Tests must not start Apollo.
        match if cfg!(test) { Ok(()) } else { neo_desktop::apollo::ask(&question) } {
            Ok(()) => self.hide(),
            Err(e) => self.error = Some(format!("Could not ask Apollo: {e}")),
        }
    }

    fn launch(&mut self, row: usize) {
        let Some(app) = self.results.get(row).and_then(|i| self.apps.get(*i)).cloned() else { return };
        match apps::launch(&app) {
            Ok(()) => {
                *self.uses.entry(app.path.clone()).or_insert(0) += 1;
                if self.register_hotkey {
                    save_uses(&self.uses);
                }
                self.hide();
            }
            Err(e) => self.error = Some(format!("Could not open {}: {e}", app.name)),
        }
    }
}

impl App for Launcher {
    type Message = Msg;

    fn title(&self) -> String {
        "Launcher".into()
    }

    fn window(&self) -> WindowSettings {
        WindowSettings { size: Size::new(WIDTH, HEIGHT), min_size: Some(Size::new(WIDTH, ASK_HEIGHT)), resizable: false, app_id: Some("org.neo.Launcher".into()), ..Default::default() }
    }

    fn theme(&self, system: Scheme) -> Theme {
        let mut theme = self.desktop.theme(system);
        // The launcher floats over anything, so it stays opaque and readable.
        theme.glass.enabled = false;
        theme
    }

    fn window_state(&self) -> WindowState {
        // Centred, a little above the middle, where the eye goes first.
        let position = (self.screen != Rect::ZERO).then(|| Point::new((self.screen.x + (self.screen.w - WIDTH) * 0.5).round(), (self.screen.y + self.screen.h * 0.22).round()));
        WindowState { visible: self.shown, always_on_top: true, bare: true, size: Some(Size::new(WIDTH, if self.asking { ASK_HEIGHT } else { HEIGHT })), position, hidden_from_capture: false, passive: false }
    }

    fn on_window_geometry(&self, geometry: WindowGeometry) -> Option<Msg> {
        (geometry.screen != self.screen).then_some(Msg::Geometry(geometry))
    }

    fn on_window_focus(&self, focused: bool) -> Option<Msg> {
        Some(Msg::Focus(focused))
    }

    fn on_close(&self) -> Option<Msg> {
        Some(Msg::Hide)
    }

    fn should_exit(&self) -> bool {
        self.quit
    }

    fn start(&mut self, proxy: Proxy<Msg>) {
        // Icons are read in the background; the list works without them.
        let list = self.apps.clone();
        let icons_to = proxy.clone();
        std::thread::spawn(move || {
            for chunk in list.chunks(24) {
                let batch: Vec<(PathBuf, Image)> = chunk.iter().filter_map(|a| apps::icon(a).map(|i| (a.path.clone(), i))).collect();
                if !batch.is_empty() && !icons_to.send(Msg::Icons(batch)) {
                    return;
                }
            }
        });
        // Linux window managers bind the key themselves, for now.
        if !self.register_hotkey || cfg!(all(unix, not(target_os = "macos"))) {
            return;
        }
        let modifier = if cfg!(target_os = "macos") { HotMods::SUPER } else { HotMods::CONTROL };
        let (apps, ask) = (HotKey::new(Some(modifier), Code::Quote), HotKey::new(Some(modifier | HotMods::SHIFT), Code::KeyA));
        if let Ok(manager) = GlobalHotKeyManager::new().and_then(|m| m.register(apps).map(|_| m)) {
            // The bar for Apollo is a second key; without it the first still works.
            let _ = manager.register(ask);
            let ask = ask.id();
            GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
                if e.state() == HotKeyState::Pressed {
                    proxy.send(if e.id() == ask { Msg::ToggleAsk } else { Msg::Toggle });
                }
            }));
            self.hotkeys = Some(manager);
        }
    }

    fn subscriptions(&self) -> Vec<Subscription<Msg>> {
        vec![Desktop::subscription(Msg::Poll)]
    }

    fn on_key(&self, k: &KeyEvent) -> Option<Msg> {
        match k.key {
            Key::Up => Some(Msg::Move(-1)),
            Key::Down | Key::Tab => Some(Msg::Move(1)),
            Key::Enter => Some(Msg::Launch),
            Key::Escape => Some(Msg::Hide),
            _ => None,
        }
    }

    fn update(&mut self, m: Msg) {
        match m {
            Msg::Query(q) => {
                self.query = q;
                self.error = None;
                self.search();
            }
            Msg::Move(by) => {
                if !self.results.is_empty() && !self.asking {
                    self.selected = (self.selected as isize + by).rem_euclid(self.results.len() as isize) as usize;
                }
            }
            Msg::Launch if self.asking => self.ask(),
            Msg::Launch => self.launch(self.selected),
            Msg::LaunchAt(row) => self.launch(row),
            Msg::ToggleAsk => {
                // The same key puts it away; from the list of apps, it turns the window into the bar.
                if self.shown && self.asking {
                    self.hide();
                } else {
                    self.show();
                    self.asking = true;
                }
            }
            Msg::Toggle => {
                if self.shown && !self.asking {
                    self.hide();
                } else {
                    // Pick up apps installed since the last time.
                    self.apps = apps::discover();
                    self.show();
                    self.asking = false;
                }
            }
            Msg::Hide => self.hide(),
            Msg::Focus(true) => self.focused = true,
            Msg::Focus(false) => {
                // A click on another app puts the launcher away. A window
                // that never had focus, as when it first opens behind
                // another app, is told "not focused" too and must stay.
                if self.shown && std::mem::take(&mut self.focused) {
                    self.hide();
                }
            }
            Msg::Geometry(g) => self.screen = g.screen,
            Msg::Icons(batch) => self.icons.extend(batch),
            Msg::Poll => {
                self.desktop.poll();
            }
        }
    }

    fn view(&self) -> Element<Msg> {
        // A new key each showing gives the field focus again, with its text selected away.
        let field = text_input(if self.asking { "Ask Apollo" } else { "Search apps" }, self.query.clone()).on_input(Msg::Query).on_submit(Msg::Launch).on_cancel(Msg::Hide).on_arrow(|by| Msg::Move(by as isize)).autofocus(true);
        let glyph = if self.asking { icon(icons::SPARKLES).size(20.0).tone(Tone::Accent) } else { icon(icons::SEARCH).size(20.0).tone(Tone::Muted) };
        let search = row().spacing(12.0).align(Align::Center).width(Length::Fill).padding([18.0, 12.0]).push(glyph).push(Element::from(field).key(self.showings));
        // Asking Apollo, the window is the field and a line under it; the
        // answer comes in Apollo's own window.
        if self.asking {
            let hint = match &self.error {
                Some(e) => text(e.clone()).role(TextRole::Caption).tone(Tone::Bad),
                None => text(format!("↵ ask, and Apollo opens with the answer   esc close   {ASK_SHORTCUT} show or hide")).role(TextRole::Caption).tone(Tone::Faint),
            };
            return container(column().width(Length::Fill).height(Length::Fill).push(search).push(Divider::horizontal()).push(container(hint).padding([18.0, 10.0]).width(Length::Fill))).surface(Surface::Card).radius(16.0).width(Length::Fill).height(Length::Fill).into();
        }

        let loose = !self.query.trim().is_empty() && self.results.first().is_some_and(|i| fuzzy::score(&self.query, &self.apps[*i].name).0 <= fuzzy::LOOSE + 10_000);
        let mut list = column().spacing(2.0).width(Length::Fill).padding([8.0, 6.0]);
        for (row_index, app_index) in self.results.iter().enumerate() {
            let app = &self.apps[*app_index];
            let picture_or_glyph: Element<Msg> = match self.icons.get(&app.path) {
                Some(image) => picture(image).width(30.0).height(30.0).into(),
                None => container(icon(icons::APP_WINDOW).size(20.0).tone(Tone::Muted)).width(30.0).height(30.0).center().into(),
            };
            let content = row().spacing(12.0).align(Align::Center).width(Length::Fill).push(picture_or_glyph).push(text(app.name.clone()).role(TextRole::Strong).no_wrap().width(Length::Fill)).push(text(app.place.clone()).role(TextRole::Caption).tone(Tone::Muted).no_wrap());
            list = list.push(Button::new(content).kind(ButtonKind::Ghost).selected(row_index == self.selected).padding([12.0, 0.0]).width(Length::Fill).height(ROW_H - 2.0).align_x(Align::Start).on_press(Msg::LaunchAt(row_index)));
        }
        if self.results.is_empty() {
            list = list.push(container(text("No apps were found on this computer.").tone(Tone::Muted)).padding(16.0));
        }

        let hint = if let Some(e) = &self.error {
            text(e.clone()).role(TextRole::Caption).tone(Tone::Bad)
        } else if loose {
            text("Nothing matches exactly. These are the closest.").role(TextRole::Caption).tone(Tone::Muted)
        } else if self.hotkeys.is_some() {
            text(format!("↵ open   ↑↓ choose   esc close   {SHORTCUT} show or hide")).role(TextRole::Caption).tone(Tone::Faint)
        } else {
            text("↵ open   ↑↓ choose   esc close").role(TextRole::Caption).tone(Tone::Faint)
        };
        let footer = container(hint).padding([18.0, 10.0]).width(Length::Fill);

        container(column().width(Length::Fill).height(Length::Fill).push(search).push(Divider::horizontal()).push(container(list).width(Length::Fill).height(Length::Fill)).push(Divider::horizontal()).push(footer)).surface(Surface::Card).radius(16.0).width(Length::Fill).height(Length::Fill).into()
    }
}

/// Keeps the launcher starting at login, for an installed copy, so the
/// shortcut works after a restart. `launcher.conf` can turn it off with
/// `launch-at-startup = false`.
fn keep_at_startup() {
    let installed = std::env::current_exe().is_ok_and(|p| !p.components().any(|c| c.as_os_str() == "target"));
    if !installed {
        return;
    }
    let wanted = !std::fs::read_to_string(neo_desktop::config_dir().join("launcher.conf")).unwrap_or_default().lines().any(|l| l.replace(' ', "") == "launch-at-startup=false");
    let Ok(program) = std::env::current_exe() else { return };
    let entry = neo_desktop::autostart::Entry { id: "org.neo.Launcher", name: "Launcher", program: &program, args: &["--hidden"] };
    let _ = if !wanted {
        neo_desktop::autostart::disable(&entry)
    } else if !neo_desktop::autostart::is_enabled(&entry) {
        neo_desktop::autostart::enable(&entry)
    } else {
        Ok(())
    };
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--snapshot") {
        snapshots(PathBuf::from(args.get(i + 1).cloned().unwrap_or_else(|| "target/snapshots".into())));
        return;
    }
    keep_at_startup();
    let mut app = Launcher::new(true, apps::discover(), load_uses());
    app.shown = !args.iter().any(|a| a == "--hidden");
    // `neo-launcher --ask` opens as the bar for asking Apollo: for a window
    // manager's own key binding, where this cannot bind one itself.
    app.asking = args.iter().any(|a| a == "--ask");
    if let Err(e) = neo::run(app) {
        eprintln!("neo-launcher: {e}");
        std::process::exit(1);
    }
}

fn snapshots(dir: PathBuf) {
    use neo::testing::Harness;
    std::fs::create_dir_all(&dir).expect("create snapshot dir");
    for (name, query, scheme) in [("launcher-empty", "", neo_desktop::SchemePref::Light), ("launcher-search", "sys", neo_desktop::SchemePref::Dark), ("launcher-closest", "fierfx", neo_desktop::SchemePref::Dark)] {
        let mut app = Launcher::new(false, apps::discover(), HashMap::new());
        app.desktop.appearance.scheme = scheme;
        let mut h = Harness::new(app, Size::new(WIDTH, HEIGHT)).expect("GPU");
        h.app_mut().update(Msg::Query(query.into()));
        // The icons arrive from a worker.
        for _ in 0..60 {
            std::thread::sleep(std::time::Duration::from_millis(25));
            h.advance(std::time::Duration::from_millis(25));
        }
        let path = dir.join(format!("{name}.png"));
        h.save_png(&path, 1.0).expect("write png");
        println!("wrote {}", path.display());
    }
    // The bar for asking Apollo.
    let mut app = Launcher::new(false, vec![], HashMap::new());
    app.desktop.appearance.scheme = neo_desktop::SchemePref::Dark;
    app.asking = true;
    app.update(Msg::Query("which photos show a girl with pink hair?".into()));
    let mut h = Harness::new(app, Size::new(WIDTH, ASK_HEIGHT)).expect("GPU");
    let path = dir.join("launcher-ask.png");
    h.save_png(&path, 2.0).expect("write png");
    println!("wrote {}", path.display());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str) -> AppEntry {
        AppEntry { name: name.into(), path: PathBuf::from(format!("/Applications/{name}.app")), place: "Applications".into(), exec: None }
    }

    fn launcher() -> Launcher {
        Launcher::new(false, ["Safari", "Firefox", "Finder", "Notes", "Calendar", "Calculator"].map(app).to_vec(), HashMap::new())
    }

    fn names(l: &Launcher) -> Vec<&str> {
        l.results.iter().map(|i| l.apps[*i].name.as_str()).collect()
    }

    #[test]
    fn the_other_key_turns_the_window_into_a_bar_for_asking_apollo() {
        let mut l = launcher();
        l.update(Msg::Hide);
        l.quit = false;
        l.update(Msg::ToggleAsk);
        assert!(l.shown && l.asking);
        assert_eq!(l.window_state().size, Some(Size::new(WIDTH, ASK_HEIGHT)), "only the field and a line, not the list of apps");
        // What is typed is not a search for apps, and nothing is asked with nothing typed.
        l.update(Msg::Query("which photos show a dog?".into()));
        l.update(Msg::Move(1));
        assert_eq!(l.selected, 0);
        let mut h = neo::testing::Harness::new(l, Size::new(WIDTH, ASK_HEIGHT)).unwrap();
        h.render(1.0);
        let mut l = std::mem::replace(h.app_mut(), launcher());
        l.update(Msg::Query("   ".into()));
        l.update(Msg::Launch);
        assert!(l.shown, "nothing to ask: it stays");
        l.update(Msg::Query("which photos show a dog?".into()));
        l.update(Msg::Launch);
        assert!(!l.shown && l.error.is_none(), "asked, it is put away and Apollo takes over");
        // Its key again puts it away; the apps' key from it brings the list, and back.
        l.quit = false;
        l.update(Msg::ToggleAsk);
        l.update(Msg::ToggleAsk);
        assert!(!l.shown);
        l.quit = false;
        l.update(Msg::ToggleAsk);
        l.update(Msg::Toggle);
        assert!(l.shown && !l.asking && l.window_state().size == Some(Size::new(WIDTH, HEIGHT)));
        l.update(Msg::ToggleAsk);
        assert!(l.shown && l.asking && l.query.is_empty());
    }

    #[test]
    fn typing_narrows_and_there_is_always_a_first_answer() {
        let mut l = launcher();
        assert_eq!(names(&l), ["Calculator", "Calendar", "Finder", "Firefox", "Notes", "Safari"], "nothing typed: by name");
        l.update(Msg::Query("fi".into()));
        assert_eq!(&names(&l)[..2], ["Finder", "Firefox"]);
        l.update(Msg::Query("qqfirefoxx".into()));
        assert_eq!(names(&l)[0], "Firefox", "no exact match, so the closest");
        assert_eq!(l.selected, 0);
    }

    #[test]
    fn arrow_keys_move_the_selection_while_the_search_field_has_focus() {
        use neo::testing::Harness;
        let mut h = Harness::new(launcher(), neo::Size::new(640.0, 420.0)).unwrap();
        h.render(1.0);
        let none = neo::Modifiers::default();
        h.type_text("c");
        assert_eq!(h.app().query, "c", "the field has focus, so typing searches");
        h.key(neo::Key::Down, none);
        assert_eq!(h.app().selected, 1);
        h.key(neo::Key::Down, none);
        assert_eq!(h.app().selected, 2);
        h.key(neo::Key::Up, none);
        assert_eq!(h.app().selected, 1);
        h.key(neo::Key::Tab, none);
        assert_eq!(h.app().selected, 2, "Tab moves down too");
        assert_eq!(h.app().query, "c", "and none of them edit the text");
    }

    #[test]
    fn the_arrows_wrap_and_habits_break_ties() {
        let mut l = launcher();
        l.update(Msg::Move(-1));
        assert_eq!(l.selected, 5, "up from the top goes to the bottom");
        l.update(Msg::Move(1));
        assert_eq!(l.selected, 0);
        // "cal" fits Calendar and Calculator about as well; the one used more wins.
        l.update(Msg::Query("cal".into()));
        assert_eq!(names(&l)[0], "Calendar");
        l.uses.insert(app("Calculator").path, 9);
        l.update(Msg::Query("cal".into()));
        assert_eq!(names(&l)[0], "Calculator");
        // But a habit does not beat typing a different app's name.
        l.update(Msg::Query("calendar".into()));
        assert_eq!(names(&l)[0], "Calendar");
    }

    #[test]
    fn hiding_without_a_shortcut_quits_and_showing_starts_fresh() {
        let mut l = launcher();
        l.update(Msg::Query("saf".into()));
        l.show();
        assert!(l.query.is_empty() && l.window_state().visible && l.showings == 1);
        l.update(Msg::Hide);
        assert!(!l.window_state().visible);
        assert!(l.should_exit(), "nothing could bring it back");
        // Losing focus puts the launcher away, but only once it has had focus:
        // a window opening behind another app is told "not focused" at once.
        let mut l = launcher();
        l.update(Msg::Focus(false));
        assert!(l.window_state().visible, "never focused, so it stays");
        l.update(Msg::Focus(true));
        l.update(Msg::Focus(false));
        assert!(!l.window_state().visible, "the user clicked elsewhere");
    }

    #[test]
    fn sits_centred_in_the_upper_part_of_the_screen() {
        let mut l = launcher();
        assert_eq!(l.window_state().position, None, "until the screen is known");
        l.update(Msg::Geometry(WindowGeometry { frame: Rect::ZERO, screen: Rect::new(0.0, 0.0, 1512.0, 982.0), scale: 2.0 }));
        assert_eq!(l.window_state().position, Some(Point::new(436.0, 216.0)));
    }
}
