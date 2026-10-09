//! Settings: how Neo looks and behaves, and a few things of the
//! computer's own.
//!
//! Appearance and accessibility are saved to the shared appearance file
//! straight away, and every running Neo app restyles itself to match. The
//! desktop picture and the volume are the system's, changed through it.
//! Startup says which of Neo's background apps start at login.
//!
//!     cargo run -p neo-settings
//!     cargo run -p neo-settings -- --snapshot target/snapshots

// Release builds on Windows open no console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use neo::prelude::*;
use neo::{Image, Proxy, Size};
use neo_desktop::fs::human_bytes_binary;
use neo_desktop::ui::{nav_item, notice, section, setting, split};
use neo_desktop::{Appearance, Desktop, SchemePref};

mod sound;
mod startup;
mod wallpaper;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Appearance,
    Wallpaper,
    Sound,
    Notifications,
    Startup,
    Shortcuts,
    Accessibility,
    About,
}

impl Page {
    const ALL: [Page; 8] = [Page::Appearance, Page::Wallpaper, Page::Sound, Page::Notifications, Page::Startup, Page::Shortcuts, Page::Accessibility, Page::About];

    fn name(self) -> &'static str {
        match self {
            Page::Appearance => "Appearance",
            Page::Wallpaper => "Wallpaper",
            Page::Sound => "Sound",
            Page::Notifications => "Notifications",
            Page::Startup => "Startup",
            Page::Shortcuts => "Shortcuts",
            Page::Accessibility => "Accessibility",
            Page::About => "About",
        }
    }

    fn icon(self) -> neo::theme::Icon {
        match self {
            Page::Appearance => icons::PALETTE,
            Page::Wallpaper => icons::WALLPAPER,
            Page::Sound => icons::VOLUME_2,
            Page::Notifications => icons::BELL,
            Page::Startup => icons::ROCKET,
            Page::Shortcuts => icons::KEYBOARD,
            Page::Accessibility => icons::ACCESSIBILITY,
            Page::About => icons::LAPTOP,
        }
    }
}

/// Facts about this computer, gathered once at startup.
struct About {
    host: String,
    os: String,
    kernel: String,
    cpu: String,
    cores: usize,
    memory: u64,
    uptime: u64,
}

impl About {
    fn gather() -> Self {
        use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};
        let sys = System::new_with_specifics(RefreshKind::nothing().with_cpu(CpuRefreshKind::nothing()).with_memory(MemoryRefreshKind::nothing().with_ram()));
        let unknown = || "Unknown".to_string();
        Self { host: System::host_name().unwrap_or_else(unknown), os: System::long_os_version().unwrap_or_else(unknown), kernel: System::kernel_version().unwrap_or_else(unknown), cpu: sys.cpus().first().map(|c| c.brand().trim().to_string()).filter(|b| !b.is_empty()).unwrap_or_else(unknown), cores: sys.cpus().len(), memory: sys.total_memory(), uptime: System::uptime() }
    }
}

struct Settings {
    desktop: Desktop,
    page: Page,
    about: About,
    error: Option<String>,
    proxy: Option<Proxy<Msg>>,
    /// The pictures there are to put on the desktop, once looked for, with
    /// a small copy of each as it is made, and the one that is there now.
    wallpapers: Option<Vec<PathBuf>>,
    thumbs: HashMap<PathBuf, Image>,
    wallpaper: Option<PathBuf>,
    /// How loud things are, once asked. `None` where the system does not say.
    levels: Option<sound::Levels>,
    /// Which of the background apps start at login, as last looked.
    starting: [bool; 4],
    /// What came of sending a notification to try them.
    tried: Option<(Tone, String)>,
}

#[derive(Clone, Debug)]
enum Msg {
    Page(Page),
    Scheme(SchemePref),
    Accent(Accent),
    Radius(f32),
    Glass(bool),
    GlassOpacity(f32),
    GlassBlur(f32),
    TextScale(f32),
    ReduceMotion(bool),
    NotificationSeconds(f32),
    Poll,
    /// The Settings entry and panel every Neo app has.
    Desktop(neo_desktop::DesktopMsg),
    /// The preview controls do nothing.
    Preview,
    /// Put this picture on the desktop.
    Wallpaper(PathBuf),
    /// Choose a picture of one's own for the desktop.
    ChoosePicture,
    /// A small copy of a picture has been made.
    Thumb(PathBuf, (u32, u32, Vec<u8>)),
    Volume(f32),
    Mute(bool),
    Input(f32),
    /// Whether one of the background apps starts at login.
    Starting(usize, bool),
    /// Send a notification, to see how they look.
    TryNotification,
    StartShell,
}

/// How large a picture is shown to choose it by, how many go across, and
/// the room between them.
const TILE: (f32, f32) = (146.0, 91.0);
const TILES_ACROSS: usize = 4;
const TILE_GAP: f32 = 6.0;

const TEXT_SCALES: [(f32, &str); 3] = [(1.0, "Default"), (1.15, "Large"), (1.3, "Larger")];

impl Settings {
    fn new() -> Self {
        Self { desktop: Desktop::load(), page: Page::Appearance, about: About::gather(), error: None, proxy: None, wallpapers: None, thumbs: HashMap::new(), wallpaper: None, levels: None, starting: [false; 4], tried: None }
    }

    /// Looks up what a page shows, on coming to it: these are things that
    /// can have changed behind Settings' back.
    fn arrive(&mut self) {
        match self.page {
            Page::Wallpaper => {
                self.wallpaper = wallpaper::current();
                if self.wallpapers.is_none() {
                    // Where the system keeps its own, and wherever the one that is set came
                    // from: a folder of one's own pictures is then all there to choose from.
                    let mut folders = wallpaper::folders();
                    folders.extend(self.wallpaper.as_deref().and_then(Path::parent).map(Path::to_path_buf));
                    let mut all = wallpaper::pictures_in(&folders);
                    // The one that is set, if it is from somewhere else.
                    if let Some(now) = self.wallpaper.clone().filter(|now| !all.contains(now)) {
                        all.insert(0, now);
                    }
                    self.picture(all.clone());
                    self.wallpapers = Some(all);
                }
            }
            Page::Sound => self.levels = if cfg!(test) { self.levels } else { sound::read() },
            Page::Startup => self.starting = if cfg!(test) { self.starting } else { startup::ITEMS.map(|item| item.on()) },
            Page::Notifications => self.tried = None,
            _ => {}
        }
    }

    /// Has small copies made of these pictures, off the main thread, each
    /// shown as it comes.
    fn picture(&mut self, pictures: Vec<PathBuf>) {
        let Some(proxy) = self.proxy.clone() else { return };
        std::thread::spawn(move || {
            for path in pictures {
                if let Some(small) = wallpaper::thumbnail(&path)
                    && !proxy.send(Msg::Thumb(path, small))
                {
                    return;
                }
            }
        });
    }

    fn set_wallpaper(&self, path: &Path) -> Result<(), String> {
        // Tests must not change this computer's desktop.
        if cfg!(test) { Ok(()) } else { wallpaper::set(path) }
    }

    /// Makes a change to how loud things are, and shows it at once: the
    /// system is told on a thread, as it takes a moment to answer.
    fn change_sound(&mut self, change: sound::Change) {
        if let Some(levels) = &mut self.levels {
            match change {
                sound::Change::Output(level) => (levels.output, levels.muted) = (level, levels.muted && level <= 0.0),
                sound::Change::Muted(muted) => levels.muted = muted,
                sound::Change::Input(level) => levels.input = Some(level),
            }
        }
        if !cfg!(test) {
            std::thread::spawn(move || {
                // Turning it up is also to be heard: a muted output is unmuted.
                if let sound::Change::Output(level) = change
                    && level > 0.0
                {
                    sound::set(sound::Change::Muted(false));
                }
                sound::set(change);
            });
        }
    }

    fn change(&mut self, f: impl FnOnce(&mut Appearance)) {
        let mut a = self.desktop.appearance;
        f(&mut a);
        self.error = self.desktop.save(a).err().map(|e| format!("Could not save {}: {e}", Appearance::path().display()));
    }
}

impl App for Settings {
    type Message = Msg;

    fn title(&self) -> String {
        "Settings".into()
    }

    fn window(&self) -> WindowSettings {
        WindowSettings { size: Size::new(920.0, 640.0), min_size: Some(Size::new(720.0, 480.0)), app_id: Some("org.neo.Settings".into()), ..Default::default() }
    }

    fn app_menu(&self) -> Vec<MenuEntry<Msg>> {
        self.desktop.app_menu(Msg::Desktop)
    }

    fn theme(&self, system: Scheme) -> Theme {
        self.desktop.theme(system)
    }

    fn start(&mut self, proxy: Proxy<Msg>) {
        self.proxy = Some(proxy);
    }

    fn subscriptions(&self) -> Vec<Subscription<Msg>> {
        vec![Desktop::subscription(Msg::Poll)]
    }

    fn update(&mut self, m: Msg) {
        match m {
            Msg::Page(p) => {
                self.page = p;
                self.error = None;
                self.arrive();
            }
            Msg::Wallpaper(path) => match self.set_wallpaper(&path) {
                Ok(()) => (self.wallpaper, self.error) = (Some(path), None),
                Err(e) => self.error = Some(e),
            },
            Msg::ChoosePicture => {
                let picked = rfd::FileDialog::new().set_title("Choose a Desktop Picture").add_filter("Pictures", &["jpg", "jpeg", "png", "heic", "webp", "tif", "tiff", "bmp"]).pick_file();
                if let Some(path) = picked {
                    // Among those to choose from, so that it can be gone back to.
                    if let Some(all) = &mut self.wallpapers
                        && !all.contains(&path)
                    {
                        all.insert(0, path.clone());
                        self.picture(vec![path.clone()]);
                    }
                    self.update(Msg::Wallpaper(path));
                }
            }
            Msg::Thumb(path, (w, h, rgba)) => {
                self.thumbs.insert(path, Image::new(w, h, rgba));
            }
            Msg::Volume(level) => self.change_sound(sound::Change::Output(level)),
            Msg::Mute(muted) => self.change_sound(sound::Change::Muted(muted)),
            Msg::Input(level) => self.change_sound(sound::Change::Input(level)),
            Msg::Starting(i, on) => {
                if let Some(item) = startup::ITEMS.get(i) {
                    // Tests must not change what starts when this computer does.
                    match if cfg!(test) { Ok(()) } else { item.set(on) } {
                        Ok(()) => (self.starting[i], self.error) = (on, None),
                        Err(e) => self.error = Some(e),
                    }
                }
            }
            Msg::TryNotification => {
                let seconds = self.desktop.appearance.notification_seconds;
                let sent = cfg!(test) || neo_desktop::notify::Notification::new("This is a notification", format!("It stays for {seconds:.0} seconds, then goes on its own.")).send();
                self.tried = Some(if sent { (Tone::Good, "Sent. It is at the top right of the screen.".into()) } else { (Tone::Warn, "NeoShell, which shows notifications, is not running.".into()) });
            }
            Msg::StartShell => {
                if !cfg!(test) {
                    neo_desktop::fs::open_with("neo-shell", "NeoShell", &[]);
                }
                self.tried = None;
            }
            Msg::Scheme(s) => self.change(|a| a.scheme = s),
            Msg::Accent(c) => self.change(|a| a.accent = c),
            Msg::Radius(r) => self.change(|a| a.radius = r),
            Msg::Glass(g) => self.change(|a| a.glass.enabled = g),
            Msg::GlassOpacity(o) => self.change(|a| a.glass.opacity = o),
            Msg::GlassBlur(b) => self.change(|a| a.glass.blur = b),
            Msg::TextScale(s) => self.change(|a| a.text_scale = s),
            Msg::ReduceMotion(r) => self.change(|a| a.reduce_motion = r),
            Msg::NotificationSeconds(n) => self.change(|a| a.notification_seconds = n),
            Msg::Preview => {}
            Msg::Desktop(m) => {
                self.desktop.update(m);
            }
            Msg::Poll => {
                self.desktop.poll();
            }
        }
    }

    fn view(&self) -> Element<Msg> {
        self.desktop.with_settings(self.content(), "This Window", Msg::Desktop, vec![])
    }
}

impl Settings {
    /// The window's content, which the settings panel goes over.
    fn content(&self) -> Element<Msg> {
        let mut side = column().spacing(2.0).width(Length::Fill).push(section("Settings"));
        for p in Page::ALL {
            side = side.push(nav_item(p.icon(), p.name(), p == self.page, Msg::Page(p)));
        }
        let body = match self.page {
            Page::Appearance => self.appearance(),
            Page::Wallpaper => self.wallpaper_page(),
            Page::Sound => self.sound_page(),
            Page::Notifications => self.notifications(),
            Page::Startup => self.startup_page(),
            Page::Shortcuts => self.shortcuts(),
            Page::Accessibility => self.accessibility(),
            Page::About => self.about(),
        };
        let mut main = column().spacing(20.0).width(Length::Fill).push(text(self.page.name()).role(TextRole::Heading)).push(body);
        if let Some(e) = &self.error {
            main = main.push(notice(Tone::Bad, e.clone()));
        }
        split(side, scrollable(container(main).padding([30.0, 26.0]).max_width(720.0)))
    }
}

fn group<M: 'static>(rows: Vec<Element<M>>) -> Element<M> {
    let mut col = column().spacing(16.0).width(Length::Fill);
    let n = rows.len();
    for (i, r) in rows.into_iter().enumerate() {
        col = col.push(r);
        if i + 1 < n {
            col = col.push(Divider::horizontal());
        }
    }
    container(col).surface(Surface::Well).padding([20.0, 18.0]).width(Length::Fill).into()
}

impl Settings {
    fn appearance(&self) -> Element<Msg> {
        let a = &self.desktop.appearance;
        let scheme_idx = SchemePref::ALL.iter().position(|s| *s == a.scheme);
        let swatches = Accent::PRESETS.iter().fold(row().spacing(8.0), |r, c| r.push(Button::new(container(Space::new(18.0, 18.0)).background(Background::Color(c.swatch())).radius(9.0)).round().padding(7.0).selected(*c == a.accent).on_press(Msg::Accent(*c))));
        let preview = row().spacing(12.0).align(Align::Center).push(button("Cancel").on_press(Msg::Preview)).push(Button::new(text("Save changes").role(TextRole::Strong)).kind(ButtonKind::Accent).on_press(Msg::Preview)).push(toggle(true, |_| Msg::Preview)).push(container(progress_bar(0.62)).width(120.0));
        let mut glass = vec![setting("Glass windows", "Makes windows translucent and blurs what is behind them.", toggle(a.glass.enabled, Msg::Glass))];
        if a.glass.enabled {
            glass.push(setting("Opacity", &format!("{:.0}%. Lower shows more of the desktop.", a.glass.opacity * 100.0), container(slider(0.3..=0.95, a.glass.opacity, Msg::GlassOpacity).step(0.05)).width(220.0)));
            glass.push(setting("Blur", &format!("{:.0} px.", a.glass.blur), container(slider(0.0..=40.0, a.glass.blur, Msg::GlassBlur).step(1.0)).width(220.0)));
        }
        column()
            .spacing(18.0)
            .width(Length::Fill)
            .push(group(vec![
                setting("Colour scheme", "Auto follows your system's light or dark setting.", segmented(["Auto", "Light", "Dark"], scheme_idx, |i| Msg::Scheme(SchemePref::ALL[i]))),
                setting("Accent", a.accent.name(), swatches),
                setting("Corner radius", &format!("{} px. Applies to windows and controls.", a.radius), container(slider(4.0..=28.0, a.radius, Msg::Radius).step(1.0)).width(220.0)),
            ]))
            .push(group(glass))
            .push(section("Preview"))
            .push(container(preview).surface(Surface::Well).padding(18.0).width(Length::Fill))
            .into()
    }

    fn accessibility(&self) -> Element<Msg> {
        let a = &self.desktop.appearance;
        let scale_idx = TEXT_SCALES.iter().position(|(s, _)| (s - a.text_scale).abs() < 0.01);
        column()
            .spacing(18.0)
            .width(Length::Fill)
            .push(group(vec![setting("Text size", "Scales text in every Neo app.", segmented(TEXT_SCALES.map(|(_, n)| n), scale_idx, |i| Msg::TextScale(TEXT_SCALES[i].0))), setting("Reduce motion", "Turns off transitions and animation, including notifications sliding in and the flash after a screenshot.", toggle(a.reduce_motion, Msg::ReduceMotion))]))
            .push(text("Text and controls meet WCAG AA contrast in both colour schemes with the royal blue accent.").role(TextRole::Caption).tone(Tone::Muted))
            .into()
    }

    fn wallpaper_page(&self) -> Element<Msg> {
        let now = self.wallpaper.as_deref();
        let said = match now {
            Some(path) => format!("{} is on the desktop.", wallpaper::name(path)),
            None => "Choose a picture for the desktop.".to_owned(),
        };
        let top = row().spacing(12.0).align(Align::Center).width(Length::Fill).push(text(said).tone(Tone::Muted).width(Length::Fill)).push(Button::new(row().spacing(8.0).align(Align::Center).push(icon(icons::IMAGE).size(14.0)).push(text("Choose a Picture…"))).on_press(Msg::ChoosePicture));
        let mut grid = column().spacing(TILE_GAP).width(Length::Fill);
        let all = self.wallpapers.as_deref().unwrap_or_default();
        for chunk in all.chunks(TILES_ACROSS) {
            let mut line = row().spacing(TILE_GAP).width(Length::Fill);
            for path in chunk {
                // The picture itself, or its place while it is being made small.
                let face: Element<Msg> = match self.thumbs.get(path) {
                    Some(small) => container(picture(small).fit(Fit::Cover).width(TILE.0).height(TILE.1)).width(TILE.0).height(TILE.1).into(),
                    None => container(icon(icons::IMAGE).size(20.0).tone(Tone::Faint)).surface(Surface::Pressed).radius(6.0).width(TILE.0).height(TILE.1).center().into(),
                };
                let tile = column().spacing(6.0).align(Align::Center).push(face).push(text(wallpaper::name(path)).role(TextRole::Caption).tone(if now == Some(path.as_path()) { Tone::Accent } else { Tone::Muted }).no_wrap().width(TILE.0).align(Align::Center));
                line = line.push(Button::new(tile).kind(ButtonKind::Ghost).selected(now == Some(path.as_path())).padding(6.0).radius(10.0).on_press(Msg::Wallpaper(path.clone())));
            }
            grid = grid.push(line);
        }
        if all.is_empty() {
            grid = grid.push(text("No pictures were found on this computer. Choose one of your own.").tone(Tone::Muted));
        }
        let note = if cfg!(target_os = "macos") { "The picture goes on every screen, for the desktop that is showing now. Pictures of your own in Pictures/Wallpapers are listed here too." } else { "Pictures of your own in Pictures/Wallpapers are listed here too." };
        column().spacing(18.0).width(Length::Fill).push(top).push(grid).push(text(note).role(TextRole::Caption).tone(Tone::Muted)).into()
    }

    fn sound_page(&self) -> Element<Msg> {
        let Some(levels) = self.levels else {
            return group(vec![text("How loud this computer is cannot be read here: its sound is set somewhere Settings cannot ask, such as on a display or an audio interface.").tone(Tone::Muted).width(Length::Fill).into()]);
        };
        let percent = |level: f32| format!("{:.0}%", level * 100.0);
        let mut rows = vec![setting("Output volume", &if levels.muted { "Muted.".to_owned() } else { format!("{}. How loud everything plays.", percent(levels.output)) }, container(slider(0.0..=1.0, levels.output, Msg::Volume).step(0.05)).width(240.0)), setting("Mute", "Silences everything without losing where the volume was.", toggle(levels.muted, Msg::Mute))];
        if let Some(input) = levels.input {
            rows.push(setting("Input volume", &format!("{}. How loud the microphone is taken, in recordings and calls.", percent(input)), container(slider(0.0..=1.0, input, Msg::Input).step(0.05)).width(240.0)));
        }
        column().spacing(18.0).width(Length::Fill).push(group(rows)).push(text("These are the system's own levels: changing them here is the same as changing them with the keyboard's volume keys.").role(TextRole::Caption).tone(Tone::Muted)).into()
    }

    fn notifications(&self) -> Element<Msg> {
        let a = &self.desktop.appearance;
        let mut col = column()
            .spacing(18.0)
            .width(Length::Fill)
            .push(group(vec![setting("Stay for", &format!("{:.0} seconds, then they go on their own.", a.notification_seconds), container(slider(2.0..=15.0, a.notification_seconds, Msg::NotificationSeconds).step(1.0)).width(220.0)), setting("Try one", "Sends a notification, to see where they appear and how long they stay.", button("Send a Notification").on_press(Msg::TryNotification))]));
        if let Some((tone, said)) = &self.tried {
            let mut line = row().spacing(12.0).align(Align::Center).width(Length::Fill).push(container(notice(*tone, said.clone())).width(Length::Fill));
            if *tone == Tone::Warn {
                line = line.push(button("Start NeoShell").on_press(Msg::StartShell));
            }
            col = col.push(line);
        }
        col.push(text("Notifications come from Neo's own apps and are shown by NeoShell at the top right. The clock on one puts it aside to be shown again later. Whether they slide or simply appear follows Reduce motion, in Accessibility.").role(TextRole::Caption).tone(Tone::Muted).width(Length::Fill)).into()
    }

    fn startup_page(&self) -> Element<Msg> {
        let rows = startup::ITEMS
            .iter()
            .enumerate()
            .map(|(i, item)| {
                // In tests every app is taken to be there, so the page does not turn on what is installed here.
                let installed = cfg!(test) || item.installed().is_some();
                let control: Element<Msg> = if installed { toggle(self.starting[i], move |on| Msg::Starting(i, on)).into() } else { text("Not installed").role(TextRole::Caption).tone(Tone::Faint).into() };
                setting(item.name, item.what, control)
            })
            .collect();
        column().spacing(18.0).width(Length::Fill).push(group(rows)).push(text("These are the apps that work from the background, so each is of use only while it is running. One turned off here stays off; it can still be opened by hand.").role(TextRole::Caption).tone(Tone::Muted).width(Length::Fill)).into()
    }

    fn shortcuts(&self) -> Element<Msg> {
        let mac = cfg!(target_os = "macos");
        let key = |mac_keys: &str, other: &str| if mac { mac_keys.to_owned() } else { other.to_owned() };
        let line = |keys: String, what: &str| -> Element<Msg> { row().spacing(16.0).align(Align::Center).width(Length::Fill).push(text(what).width(Length::Fill)).push(container(text(keys).mono().role(TextRole::Strong)).surface(Surface::Pressed).radius(6.0).padding([10.0, 4.0])).into() };
        column()
            .spacing(18.0)
            .width(Length::Fill)
            .push(section("From anywhere"))
            .push(group(vec![line(key("⌘ '", "Ctrl + '"), "Search for an app and open it"), line(key("⌘ ⇧ A", "Ctrl + Shift + A"), "Ask Apollo"), line(key("⌘ ⇧ S", "Ctrl + Shift + S"), "Take a screenshot"), line(key("⌘ ⇧ R", "Ctrl + Shift + R"), "Record the screen, or stop recording")]))
            .push(section("In every Neo app"))
            .push(group(vec![line(key("⌘ ,", "Ctrl + ,"), "This window's settings"), line(key("⌘ Q", "Ctrl + Q"), "Quit the app"), line(key("⌘ W", "Ctrl + W"), "Close the window")]))
            .push(text(if mac { "The first four work while the Launcher and NeoCap are running, which is what Startup is for. They cannot be changed here yet." } else { "On Linux the desktop's own settings bind keys: bind them to neo-launcher, neo-launcher --ask and neo-recorder. They cannot be changed here yet." }).role(TextRole::Caption).tone(Tone::Muted).width(Length::Fill))
            .into()
    }

    fn about(&self) -> Element<Msg> {
        let a = &self.about;
        let fact = |k: &str, v: String| -> Element<Msg> { row().spacing(16.0).width(Length::Fill).push(text(k).tone(Tone::Muted).width(140.0)).push(text(v).role(TextRole::Strong).width(Length::Fill)).into() };
        let up = a.uptime;
        let uptime = match (up / 86_400, up / 3600 % 24, up / 60 % 60) {
            (0, 0, m) => format!("{m} min"),
            (0, h, m) => format!("{h} h {m} min"),
            (d, h, _) => format!("{d} d {h} h"),
        };
        let badge = row().spacing(16.0).align(Align::Center).push(container(icon(icons::SPARKLES).size(30.0).tone(Tone::Accent)).surface(Surface::Pressed).padding(16.0).radius(18.0)).push(column().spacing(2.0).push(text("Neo").role(TextRole::Heading)).push(text(format!("Desktop {}", env!("CARGO_PKG_VERSION"))).tone(Tone::Muted)));
        column()
            .spacing(18.0)
            .width(Length::Fill)
            .push(badge)
            .push(group(vec![fact("Device name", a.host.clone()), fact("Operating system", a.os.clone()), fact("Kernel", a.kernel.clone()), fact("Processor", format!("{} × {}", a.cores, a.cpu)), fact("Memory", human_bytes_binary(a.memory)), fact("Uptime", uptime)]))
            .push(text(format!("Settings file: {}", Appearance::path().display())).role(TextRole::Caption).tone(Tone::Muted))
            .into()
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--snapshot") {
        snapshots(std::path::PathBuf::from(args.get(i + 1).cloned().unwrap_or_else(|| "target/snapshots".into())));
        return;
    }
    if let Err(e) = neo::run(Settings::new()) {
        eprintln!("neo-settings: {e}");
        std::process::exit(1);
    }
}

fn snapshots(dir: std::path::PathBuf) {
    use neo::testing::Harness;
    std::fs::create_dir_all(&dir).expect("create snapshot dir");
    for (name, page, scheme) in [("settings-appearance", Page::Appearance, SchemePref::Light), ("settings-about", Page::About, SchemePref::Dark), ("settings-wallpaper", Page::Wallpaper, SchemePref::Dark), ("settings-sound", Page::Sound, SchemePref::Light), ("settings-startup", Page::Startup, SchemePref::Dark), ("settings-shortcuts", Page::Shortcuts, SchemePref::Light)] {
        let mut app = Settings::new();
        app.page = page;
        app.desktop.appearance = Appearance { scheme, ..Appearance::default() };
        app.arrive();
        // With no thread to make them on here, the first pictures are made small in place.
        for path in app.wallpapers.clone().unwrap_or_default().into_iter().take(12) {
            if let Some((w, h, rgba)) = wallpaper::thumbnail(&path) {
                app.thumbs.insert(path, Image::new(w, h, rgba));
            }
        }
        let mut h = Harness::new(app, Size::new(920.0, 640.0)).expect("GPU");
        let path = dir.join(format!("{name}.png"));
        h.save_png(&path, 1.0).expect("write png");
        println!("wrote {}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_reach_other_apps() {
        let dir = std::env::temp_dir().join(format!("neo-settings-{}", std::process::id()));
        // SAFETY: this crate's only test sets the variable before reading it.
        unsafe { std::env::set_var("NEO_CONFIG_DIR", &dir) };
        let mut settings = Settings::new();
        let mut other = Desktop::load();
        settings.update(Msg::Accent(Accent::Teal));
        settings.update(Msg::Scheme(SchemePref::Dark));
        assert!(settings.error.is_none(), "{:?}", settings.error);
        assert!(other.poll(), "another app sees the change");
        assert_eq!(other.appearance.accent, Accent::Teal);
        assert_eq!(other.theme(Scheme::Light).scheme, Scheme::Dark);
        // Every page draws, with whatever this computer has or has not.
        use neo::testing::Harness;
        let mut h = Harness::new(settings, Size::new(920.0, 640.0)).unwrap();
        for page in Page::ALL {
            h.app_mut().update(Msg::Page(page));
            h.render(1.0);
        }
        let mut settings = std::mem::replace(h.app_mut(), Settings::new());
        assert_eq!(Page::ALL.len(), 8);

        // The desktop picture: one chosen is the one shown as set.
        settings.update(Msg::Page(Page::Wallpaper));
        assert!(settings.wallpapers.is_some(), "looked for on coming to the page");
        let picture = dir.join("mine.png");
        image::RgbImage::from_pixel(64, 40, image::Rgb([200, 60, 60])).save(&picture).unwrap();
        settings.update(Msg::Wallpaper(picture.clone()));
        assert_eq!((settings.wallpaper.clone(), settings.error.clone()), (Some(picture.clone()), None));
        settings.update(Msg::Thumb(picture.clone(), wallpaper::thumbnail(&picture).unwrap()));
        assert!(settings.thumbs.contains_key(&picture));

        // Sound: a change shows at once; turning the volume up from nothing unmutes it.
        settings.levels = Some(sound::Levels { output: 0.0, muted: true, input: Some(0.5) });
        settings.update(Msg::Page(Page::Sound));
        settings.update(Msg::Volume(0.4));
        assert_eq!(settings.levels, Some(sound::Levels { output: 0.4, muted: false, input: Some(0.5) }));
        settings.update(Msg::Mute(true));
        settings.update(Msg::Input(0.9));
        assert_eq!(settings.levels, Some(sound::Levels { output: 0.4, muted: true, input: Some(0.9) }));
        // With nothing to read, the page says so and the controls do nothing.
        settings.levels = None;
        settings.update(Msg::Volume(0.7));
        assert_eq!(settings.levels, None);

        // Startup, and trying a notification.
        settings.update(Msg::Page(Page::Startup));
        settings.update(Msg::Starting(1, true));
        settings.update(Msg::Starting(9, true));
        assert_eq!(settings.starting, [false, true, false, false]);
        settings.update(Msg::Page(Page::Notifications));
        settings.update(Msg::NotificationSeconds(9.0));
        settings.update(Msg::TryNotification);
        assert_eq!((settings.desktop.appearance.notification_seconds, settings.tried.as_ref().map(|t| t.0)), (9.0, Some(Tone::Good)));
        settings.update(Msg::Page(Page::Shortcuts));
        assert_eq!(settings.tried.as_ref().map(|t| t.0), Some(Tone::Good), "kept until the page is come to again");
        settings.update(Msg::Page(Page::Notifications));
        assert_eq!(settings.tried, None);
        let mut h = Harness::new(settings, Size::new(920.0, 640.0)).unwrap();
        for page in Page::ALL {
            h.app_mut().update(Msg::Page(page));
            h.render(1.0);
        }
        drop(h);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
