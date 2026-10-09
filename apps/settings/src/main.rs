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
use neo::{Image, Point, Proxy, Rect, Size};
use neo_desktop::fs::human_bytes_binary;
use neo_desktop::ui::{nav_item, notice, section, setting, split};
use neo_desktop::{Appearance, Desktop, SchemePref};

mod bluetooth;
mod color;
mod network;
mod sound;
mod startup;
mod wallpaper;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Appearance,
    Sound,
    Network,
    Bluetooth,
    Notifications,
    Startup,
    Shortcuts,
    Accessibility,
    About,
}

impl Page {
    const ALL: [Page; 9] = [Page::Appearance, Page::Sound, Page::Network, Page::Bluetooth, Page::Notifications, Page::Startup, Page::Shortcuts, Page::Accessibility, Page::About];

    fn name(self) -> &'static str {
        match self {
            Page::Appearance => "Appearance",
            Page::Sound => "Sound",
            Page::Network => "Network",
            Page::Bluetooth => "Bluetooth",
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
            Page::Sound => icons::VOLUME_2,
            Page::Network => icons::WIFI,
            Page::Bluetooth => icons::BLUETOOTH,
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

/// The colour being picked for the accent.
struct Picker {
    hsv: color::Hsv,
    /// Its code as typed, which is not a colour until it is finished.
    code: String,
    /// Every shade of its hue, and every hue, to pick from.
    shades: Image,
    hues: Image,
}

impl Picker {
    fn of(c: Color) -> Self {
        let hsv = color::Hsv::of(c);
        Self { hsv, code: color::hex(c), shades: color::shades(hsv.h), hues: color::hues() }
    }
}

struct Settings {
    desktop: Desktop,
    page: Page,
    about: About,
    error: Option<String>,
    proxy: Option<Proxy<Msg>>,
    /// The picker for an accent of one's own colour, while it is open.
    picker: Option<Picker>,
    /// The pictures there are to put on the desktop, once looked for, with
    /// a small copy of each as it is made, and the one that is there now.
    wallpapers: Option<Vec<PathBuf>>,
    thumbs: HashMap<PathBuf, Image>,
    wallpaper: Option<PathBuf>,
    /// How loud things are, once asked. `None` where the system does not say.
    levels: Option<sound::Levels>,
    /// What sound can come out of and go into.
    devices: Vec<sound::Device>,
    /// When a level was last changed here: what the system says just after
    /// is of a moment before, and is not to pull the slider back.
    sound_changed: Option<std::time::Instant>,
    /// Which list of devices is open, for sound in or out, and where it hangs from.
    choosing: Option<(bool, Point)>,
    /// How this computer is connected, and Bluetooth, once each has been asked.
    network: Option<network::Network>,
    bluetooth: Option<bluetooth::Bluetooth>,
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
    /// Open the picker for an accent of one's own colour, or put it away.
    PickAccent,
    /// A shade was picked: how much colour, and how far down from light.
    Shade(f32, f32),
    Hue(f32),
    /// The colour's code, as it is being typed.
    Code(String),
    /// Pick a colour off the screen.
    Sample,
    Sampled(Option<Color>),
    /// Put this picture on the desktop.
    Wallpaper(PathBuf),
    /// Choose a picture of one's own for the desktop.
    ChoosePicture,
    /// A small copy of a picture has been made.
    Thumb(PathBuf, (u32, u32, Vec<u8>)),
    Volume(f32),
    Mute(bool),
    Input(f32),
    /// Time to see whether the loudness was changed elsewhere: by the
    /// keyboard's volume keys, or the system's own settings.
    SoundTick,
    /// How loud things are, just asked, and the devices there are.
    Levels(Option<sound::Levels>, Vec<sound::Device>),
    /// Open the list of devices for sound in (true) or out, under its control.
    ChooseDevice(bool, Rect),
    CloseChoice,
    /// Make a device the one used for sound in, or out.
    PickDevice(u32, bool),
    /// What the system says of the network, and of Bluetooth.
    Network(network::Network),
    Bluetooth(bluetooth::Bluetooth),
    Wifi(bool),
    BluetoothPower(bool),
    /// Open System Monitor, where what is passing over the network is shown.
    OpenMonitor,
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

/// How often the Sound page asks the system how loud things are, and how
/// long after a change made here it leaves the system to catch up.
const SOUND_EVERY: std::time::Duration = std::time::Duration::from_millis(500);
const SOUND_SETTLES: std::time::Duration = std::time::Duration::from_millis(900);

/// How large the plane of shades is in the colour picker.
const PLANE: (f32, f32) = (300.0, 150.0);

const TEXT_SCALES: [(f32, &str); 3] = [(1.0, "Default"), (1.15, "Large"), (1.3, "Larger")];

impl Settings {
    fn new() -> Self {
        Self { desktop: Desktop::load(), page: Page::Appearance, about: About::gather(), error: None, proxy: None, picker: None, wallpapers: None, thumbs: HashMap::new(), wallpaper: None, levels: None, devices: vec![], sound_changed: None, choosing: None, network: None, bluetooth: None, starting: [false; 4], tried: None }
    }

    /// Looks up what a page shows, on coming to it: these are things that
    /// can have changed behind Settings' back.
    fn arrive(&mut self) {
        self.choosing = None;
        match self.page {
            Page::Appearance => {
                self.wallpaper = wallpaper::current();
                if self.wallpapers.is_none() {
                    // Where the system keeps its own, and wherever the one that is set came
                    // from: a folder of one's own pictures is then all there to choose from.
                    // One's own come first: the folder the picture now set is in.
                    let mut folders: Vec<PathBuf> = self.wallpaper.as_deref().and_then(Path::parent).map(Path::to_path_buf).into_iter().collect();
                    folders.extend(wallpaper::folders());
                    let mut all = wallpaper::pictures_in(&folders);
                    // The one that is set, if it is from somewhere else.
                    if let Some(now) = self.wallpaper.clone().filter(|now| !all.contains(now)) {
                        all.insert(0, now);
                    }
                    self.picture(all.clone());
                    self.wallpapers = Some(all);
                }
            }
            Page::Sound => {
                if !cfg!(test) {
                    (self.levels, self.devices) = (sound::read(), sound::devices());
                }
            }
            Page::Network | Page::Bluetooth => self.ask(self.page, 0),
            Page::Startup => self.starting = if cfg!(test) { self.starting } else { startup::ITEMS.map(|item| item.on()) },
            Page::Notifications => self.tried = None,
            _ => {}
        }
    }

    /// Asks the system about the network or Bluetooth, off the main
    /// thread as it takes a second or so, after `wait` milliseconds: a
    /// change just made takes that long to show.
    fn ask(&mut self, page: Page, wait: u64) {
        let Some(proxy) = self.proxy.clone() else { return };
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(wait));
            match page {
                Page::Network => proxy.send(Msg::Network(network::read())),
                _ => proxy.send(Msg::Bluetooth(bluetooth::read())),
            };
        });
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
        self.sound_changed = Some(std::time::Instant::now());
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

    /// Makes the colour in the picker the accent, and writes its code.
    fn use_picked(&mut self) {
        let Some(p) = &mut self.picker else { return };
        let c = p.hsv.color();
        p.code = color::hex(c);
        self.change(|a| a.accent = Accent::from_color(c));
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
        // The page it opens on has things to look up too.
        self.arrive();
    }

    fn subscriptions(&self) -> Vec<Subscription<Msg>> {
        let mut subs = vec![Desktop::subscription(Msg::Poll)];
        // While the Sound page shows, it follows the volume keys and devices coming and going.
        if self.page == Page::Sound {
            subs.push(Subscription::every(SOUND_EVERY, Msg::SoundTick));
        }
        subs
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
            Msg::SoundTick => {
                // Asked off the main thread, as it takes a moment to answer.
                if let Some(proxy) = self.proxy.clone() {
                    std::thread::spawn(move || {
                        proxy.send(Msg::Levels(sound::read(), sound::devices()));
                    });
                }
            }
            Msg::Levels(levels, devices) => {
                // Not over a change just made here, which it may not have caught up with.
                if self.page == Page::Sound && self.sound_changed.is_none_or(|t| t.elapsed() >= SOUND_SETTLES) {
                    (self.levels, self.devices) = (levels, devices);
                }
            }
            Msg::ChooseDevice(input, under) => self.choosing = Some((input, Point::new(under.x, under.bottom() + 4.0))),
            Msg::CloseChoice => self.choosing = None,
            Msg::PickDevice(id, input) => {
                self.choosing = None;
                self.sound_changed = Some(std::time::Instant::now());
                // Tests must not change what this computer plays through.
                if cfg!(test) || sound::set_default(id, input) {
                    for d in &mut self.devices {
                        if input {
                            d.default_input = d.id == id;
                        } else {
                            d.default_output = d.id == id;
                        }
                    }
                    self.error = None;
                    // Another device has its own loudness.
                    if !cfg!(test) {
                        self.levels = sound::read();
                    }
                } else {
                    self.error = Some("The system would not use that device.".into());
                }
            }
            Msg::Network(found) => self.network = Some(found),
            Msg::Bluetooth(found) => self.bluetooth = Some(found),
            Msg::Wifi(on) => {
                let device = self.network.as_ref().and_then(|n| n.connections.iter().find(|c| c.wifi)).map(|c| c.device.clone());
                match device.map(|d| if cfg!(test) { Ok(()) } else { network::set_wifi(&d, on) }) {
                    Some(Ok(())) => {
                        if let Some(n) = &mut self.network {
                            n.wifi_on = Some(on);
                        }
                        self.error = None;
                        // It takes a few seconds to join a network, or to leave one.
                        self.ask(Page::Network, 4000);
                    }
                    Some(Err(e)) => self.error = Some(e),
                    None => {}
                }
            }
            Msg::BluetoothPower(on) => {
                if cfg!(test) || bluetooth::set_power(on) {
                    if let Some(b) = &mut self.bluetooth {
                        b.on = Some(on);
                    }
                    self.ask(Page::Bluetooth, 2500);
                } else {
                    self.error = Some("Bluetooth could not be changed.".into());
                }
            }
            Msg::OpenMonitor => {
                if !cfg!(test) {
                    neo_desktop::fs::open_with("neo-monitor", "System Monitor", &[]);
                }
            }
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
            Msg::Accent(c) => {
                self.picker = None;
                self.change(|a| a.accent = c);
            }
            Msg::PickAccent => {
                self.picker = match self.picker.take() {
                    Some(_) => None,
                    // From the accent there is, so that it starts where things stand.
                    None => Some(Picker::of(self.desktop.appearance.accent.swatch())),
                };
            }
            Msg::Shade(s, down) => {
                if let Some(p) = &mut self.picker {
                    (p.hsv.s, p.hsv.v) = (s.clamp(0.0, 1.0), (1.0 - down).clamp(0.0, 1.0));
                    self.use_picked();
                }
            }
            Msg::Hue(h) => {
                if let Some(p) = &mut self.picker {
                    // A whole turn is the start again, which the strip does not have twice.
                    p.hsv.h = h.clamp(0.0, 0.999);
                    p.shades = color::shades(p.hsv.h);
                    self.use_picked();
                }
            }
            Msg::Code(code) => {
                if let Some(p) = &mut self.picker {
                    p.code = code;
                    // Not a colour until it is finished: nothing changes while it is being typed.
                    if let Some(c) = color::parse_hex(&p.code) {
                        let typed = std::mem::take(&mut p.code);
                        *p = Picker::of(c);
                        p.code = typed;
                        self.change(|a| a.accent = Accent::from_color(c));
                    }
                }
            }
            Msg::Sample => {
                if let Some(proxy) = self.proxy.clone() {
                    color::sample(move |picked| {
                        proxy.send(Msg::Sampled(picked));
                    });
                }
            }
            Msg::Sampled(picked) => {
                // Dismissed, it changes nothing.
                if let Some(c) = picked {
                    self.picker = Some(Picker::of(c));
                    self.change(|a| a.accent = Accent::from_color(c));
                }
            }
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
            Page::Sound => self.sound_page(),
            Page::Network => self.network_page(),
            Page::Bluetooth => self.bluetooth_page(),
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
        let page = split(side, scrollable(container(main).padding([30.0, 26.0]).max_width(720.0)));
        // The list of sound devices hangs over everything, from the control that opened it.
        match self.choosing {
            Some((input, at)) => {
                let mut items: Vec<MenuItem<Msg>> = self.devices.iter().filter(|d| if input { d.input } else { d.output }).map(|d| if (input && d.default_input) || (!input && d.default_output) { MenuItem::new(d.name.clone(), Msg::CloseChoice).icon(icons::CHECK) } else { MenuItem::new(d.name.clone(), Msg::PickDevice(d.id, input)) }).collect();
                if items.is_empty() {
                    items.push(MenuItem::disabled("None"));
                }
                stack().width(Length::Fill).height(Length::Fill).push(page).push(popup_menu(at, items, Msg::CloseChoice)).into()
            }
            None => page,
        }
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
        // And one of one's own colour: the swatch of it, or the way to choosing one.
        let own = matches!(a.accent, Accent::Custom { .. });
        let own_face: Element<Msg> = if own { container(Space::new(18.0, 18.0)).background(Background::Color(a.accent.swatch())).radius(9.0).into() } else { container(icon(icons::PIPETTE).size(14.0).tone(Tone::Muted)).width(18.0).height(18.0).center().into() };
        let swatches = swatches.push(Button::new(own_face).round().padding(7.0).selected(own || self.picker.is_some()).on_press(Msg::PickAccent));
        let mut look =
            vec![setting("Colour scheme", "Auto follows your system's light or dark setting.", segmented(["Auto", "Light", "Dark"], scheme_idx, |i| Msg::Scheme(SchemePref::ALL[i]))), setting("Accent", &if own { format!("Your own, from {}.", self.picker.as_ref().map_or_else(|| color::hex(a.accent.swatch()), |p| color::hex(p.hsv.color()))) } else { a.accent.name().to_owned() }, swatches)];
        if let Some(p) = &self.picker {
            look.push(self.picker_row(p));
        }
        look.push(setting("Corner radius", &format!("{} px. Applies to windows and controls.", a.radius), container(slider(4.0..=28.0, a.radius, Msg::Radius).step(1.0)).width(220.0)));
        let mut glass = vec![setting("Glass windows", "Makes windows translucent and blurs what is behind them.", toggle(a.glass.enabled, Msg::Glass))];
        if a.glass.enabled {
            glass.push(setting("Opacity", &format!("{:.0}%. Lower shows more of the desktop.", a.glass.opacity * 100.0), container(slider(0.3..=0.95, a.glass.opacity, Msg::GlassOpacity).step(0.05)).width(220.0)));
            glass.push(setting("Blur", &format!("{:.0} px.", a.glass.blur), container(slider(0.0..=40.0, a.glass.blur, Msg::GlassBlur).step(1.0)).width(220.0)));
        }
        column().spacing(18.0).width(Length::Fill).push(group(look)).push(group(glass)).push(section("Wallpaper")).push(self.wallpaper_section()).push(section("Preview")).push(container(preview).surface(Surface::Well).padding(18.0).width(Length::Fill)).into()
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

    /// The picker: a plane of the hue's shades and a strip of hues to drag
    /// in, the colour's code to type, and the eyedropper.
    fn picker_row(&self, p: &Picker) -> Element<Msg> {
        let chosen = p.hsv.color();
        let accent = Accent::from_color(chosen);
        let planes = column().spacing(8.0).push(Element::new(color::Plane::new(p.shades.clone(), (p.hsv.s, 1.0 - p.hsv.v), Size::new(PLANE.0, PLANE.1), Msg::Shade))).push(Element::new(color::Plane::new(p.hues.clone(), (p.hsv.h, 0.5), Size::new(PLANE.0, 16.0), |h, _| Msg::Hue(h))));
        // How it comes out in each scheme, which is the colour itself only where that can be read.
        let shown = |label: &str, scheme: Scheme| -> Element<Msg> {
            let pill = container(text(label).role(TextRole::Caption).tone(Tone::Custom(accent.on_tone(scheme)))).background(Background::Color(accent.tone(scheme))).radius(10.0).padding([12.0, 5.0]);
            pill.into()
        };
        let mut side = column()
            .spacing(10.0)
            .width(Length::Fill)
            .push(row().spacing(8.0).align(Align::Center).push(container(Space::new(26.0, 26.0)).background(Background::Color(chosen)).radius(13.0)).push(text_input("#3F5BC4", p.code.clone()).on_input(Msg::Code).width(120.0)))
            .push(row().spacing(6.0).push(shown("On light", Scheme::Light)).push(shown("On dark", Scheme::Dark)))
            .push(text("Drag in the colours, type a code, or pick one off the screen. A colour too pale or too dark to read is shifted just far enough for each scheme.").role(TextRole::Caption).tone(Tone::Muted).width(Length::Fill));
        if color::can_sample() {
            side = side.push(row().push(Button::new(row().spacing(8.0).align(Align::Center).push(icon(icons::PIPETTE).size(14.0)).push(text("Pick from the Screen"))).on_press(Msg::Sample)));
        }
        row().spacing(18.0).width(Length::Fill).push(planes).push(side).into()
    }

    /// The desktop picture: those there are to choose from, and the one that is set.
    fn wallpaper_section(&self) -> Element<Msg> {
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
        column().spacing(14.0).width(Length::Fill).push(top).push(grid).push(text(note).role(TextRole::Caption).tone(Tone::Muted)).into()
    }

    fn sound_page(&self) -> Element<Msg> {
        let percent = |level: f32| format!("{:.0}%", level * 100.0);
        // The device in use for sound in or out, as a control that opens the list of them.
        let chooser = |input: bool| -> Element<Msg> {
            let now = self.devices.iter().find(|d| if input { d.default_input } else { d.default_output }).map_or("None".to_owned(), |d| d.name.clone());
            let face = row().spacing(8.0).align(Align::Center).width(Length::Fill).push(text(now).no_wrap().width(Length::Fill)).push(icon(icons::CHEVRONS_UP_DOWN).size(14.0).tone(Tone::Muted));
            mouse_area(container(face).surface(Surface::Raised).radius(8.0).padding([12.0, 7.0]).width(260.0)).on_press_in(move |at| Msg::ChooseDevice(input, at)).into()
        };
        let has = |input: bool| self.devices.iter().any(|d| if input { d.input } else { d.output });
        let mut output = vec![];
        if has(false) {
            output.push(setting("Play through", "Where sound comes out, unless an app chooses otherwise.", chooser(false)));
        }
        match self.levels {
            Some(levels) => {
                output.push(setting("Output volume", &if levels.muted { "Muted.".to_owned() } else { format!("{}. How loud everything plays.", percent(levels.output)) }, container(slider(0.0..=1.0, levels.output, Msg::Volume).step(0.05)).width(260.0)));
                output.push(setting("Mute", "Silences everything without losing where the volume was.", toggle(levels.muted, Msg::Mute)));
            }
            None => output.push(text("How loud this device plays is set on the device itself, such as a display or an audio interface.").tone(Tone::Muted).width(Length::Fill).into()),
        }
        let mut input = vec![];
        if has(true) {
            input.push(setting("Listen through", "The microphone used for recordings and calls, unless an app chooses otherwise.", chooser(true)));
        }
        if let Some(level) = self.levels.and_then(|l| l.input) {
            input.push(setting("Input volume", &format!("{}. How loud the microphone is taken.", percent(level)), container(slider(0.0..=1.0, level, Msg::Input).step(0.05)).width(260.0)));
        }
        let mut page = column().spacing(18.0).width(Length::Fill).push(section("Output")).push(group(output));
        if !input.is_empty() {
            page = page.push(section("Input")).push(group(input));
        }
        page.push(text("These are the system's own: changing them here is the same as changing them in its sound settings or with the keyboard's volume keys.").role(TextRole::Caption).tone(Tone::Muted)).into()
    }

    fn network_page(&self) -> Element<Msg> {
        let Some(net) = &self.network else {
            return text("Asking how this computer is connected…").tone(Tone::Muted).into();
        };
        let mut page = column().spacing(18.0).width(Length::Fill);
        if let Some(on) = net.wifi_on {
            page = page.push(group(vec![setting("Wi-Fi", if on { "On. Turning it off leaves every Wi-Fi network." } else { "Off." }, toggle(on, Msg::Wifi))]));
        }
        let fact = |k: &str, v: String| -> Element<Msg> { row().spacing(16.0).width(Length::Fill).push(text(k).tone(Tone::Muted).width(140.0)).push(text(v).mono().role(TextRole::Caption).width(Length::Fill)).into() };
        for c in &net.connections {
            let state = if c.connected() {
                ("Connected", Tone::Good)
            } else if c.wifi && net.wifi_on == Some(false) {
                ("Off", Tone::Muted)
            } else {
                ("Not connected", Tone::Muted)
            };
            let head = row().spacing(10.0).align(Align::Center).width(Length::Fill).push(icon(if c.wifi { icons::WIFI } else { icons::NETWORK }).size(18.0).tone(Tone::Accent)).push(text(c.name.clone()).role(TextRole::Strong).width(Length::Fill)).push(text(state.0).role(TextRole::Caption).tone(state.1));
            let mut rows: Vec<Element<Msg>> = vec![head.into()];
            if let Some(address) = &c.address {
                rows.push(fact("Address", format!("{address}{}", if c.automatic { "  ·  given by the network" } else { "  ·  set by hand" })));
                if let Some(router) = &c.router {
                    rows.push(fact("Router", router.clone()));
                }
                rows.push(fact("Name servers", if c.dns.is_empty() { "The network's own".to_owned() } else { c.dns.join(", ") }));
            }
            if !c.mac.is_empty() {
                rows.push(fact("Hardware address", c.mac.clone()));
            }
            rows.push(fact("Interface", c.device.clone()));
            page = page.push(group(rows));
        }
        if net.connections.is_empty() {
            page = page.push(group(vec![text("This computer is not connected to a network.").tone(Tone::Muted).into()]));
        }
        page.push(row().spacing(12.0).align(Align::Center).width(Length::Fill).push(text("What is passing over the network, and which programs are using it, is in System Monitor.").role(TextRole::Caption).tone(Tone::Muted).width(Length::Fill)).push(button("Open System Monitor").on_press(Msg::OpenMonitor))).into()
    }

    fn bluetooth_page(&self) -> Element<Msg> {
        let Some(bt) = &self.bluetooth else {
            return text("Asking about Bluetooth…").tone(Tone::Muted).into();
        };
        let Some(on) = bt.on else {
            return group(vec![text("This computer has no Bluetooth, or does not say whether it is on.").tone(Tone::Muted).width(Length::Fill).into()]);
        };
        let mut page = column().spacing(18.0).width(Length::Fill).push(group(vec![setting("Bluetooth", if on { "On." } else { "Off. Devices that connect by it will not." }, toggle(on, Msg::BluetoothPower))]));
        let mut rows: Vec<Element<Msg>> = vec![];
        for d in &bt.devices {
            let about = [d.kind.clone(), d.battery.clone().map(|b| format!("battery {b}")).unwrap_or_default()].into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join("  ·  ");
            let mut words = column().spacing(1.0).width(Length::Fill).push(text(d.name.clone()).role(TextRole::Strong).no_wrap());
            if !about.is_empty() {
                words = words.push(text(about).role(TextRole::Caption).tone(Tone::Muted));
            }
            rows.push(row().spacing(12.0).align(Align::Center).width(Length::Fill).push(icon(icons::BLUETOOTH).size(16.0).tone(if d.connected && on { Tone::Accent } else { Tone::Faint })).push(words).push(text(if d.connected && on { "Connected" } else { "Not connected" }).role(TextRole::Caption).tone(if d.connected && on { Tone::Good } else { Tone::Muted })).into());
        }
        if rows.is_empty() {
            rows.push(text("No devices have been paired with this computer.").tone(Tone::Muted).into());
        }
        page = page.push(section("Devices")).push(group(rows));
        page.push(text("Pairing a new device, and connecting or disconnecting one, are done in the system's own Bluetooth settings for now.").role(TextRole::Caption).tone(Tone::Muted).width(Length::Fill)).into()
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
    for (name, page, scheme) in [
        ("settings-appearance", Page::Appearance, SchemePref::Light),
        ("settings-about", Page::About, SchemePref::Dark),
        ("settings-picker", Page::Appearance, SchemePref::Dark),
        ("settings-sound", Page::Sound, SchemePref::Dark),
        ("settings-network", Page::Network, SchemePref::Light),
        ("settings-bluetooth", Page::Bluetooth, SchemePref::Dark),
        ("settings-startup", Page::Startup, SchemePref::Dark),
        ("settings-shortcuts", Page::Shortcuts, SchemePref::Light),
    ] {
        let mut app = Settings::new();
        app.page = page;
        app.desktop.appearance = Appearance { scheme, ..Appearance::default() };
        app.arrive();
        // With no thread to ask on here, what takes a moment is asked in place.
        match page {
            Page::Network => app.network = Some(network::read()),
            Page::Bluetooth => app.bluetooth = Some(bluetooth::read()),
            _ => {}
        }
        // The picker, open on a colour of one's own. (Shown, not saved: this is a picture of it.)
        if name == "settings-picker" {
            app.picker = Some(Picker::of(Color::hex(0xE0569B)));
            app.desktop.appearance.accent = Accent::from_color(Color::hex(0xE0569B));
        }
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
        // An accent of one's own colour: picked by dragging, by its code, or off the screen.
        settings.update(Msg::PickAccent);
        assert!(settings.picker.is_some() && settings.desktop.appearance.accent == Accent::Teal, "opening it changes nothing yet");
        settings.update(Msg::Hue(0.9));
        settings.update(Msg::Shade(0.82, 0.0));
        let picked = settings.picker.as_ref().unwrap().hsv.color();
        assert_eq!(settings.desktop.appearance.accent, Accent::from_color(picked));
        assert_eq!(settings.picker.as_ref().unwrap().code, color::hex(picked), "its code is written as it is dragged");
        // Kept as its codes, so what another app reads is the same to the eye.
        let kept = Desktop::load().appearance.accent;
        let codes = |a: Accent| (color::hex(a.tone(Scheme::Light)), color::hex(a.tone(Scheme::Dark)));
        assert_eq!(codes(kept), codes(Accent::from_color(picked)), "and it is written down for other apps to take up");
        // A code half typed changes nothing; finished, it is the accent, and the mark moves to it.
        settings.update(Msg::Code("#3F5B".into()));
        assert_eq!(settings.desktop.appearance.accent, Accent::from_color(picked));
        settings.update(Msg::Code("#3f5bc4".into()));
        assert_eq!(settings.desktop.appearance.accent, Accent::from_color(Color::hex(0x3F5BC4)));
        assert_eq!((settings.picker.as_ref().unwrap().code.as_str(), color::hex(settings.picker.as_ref().unwrap().hsv.color())), ("#3f5bc4", "#3F5BC4".to_owned()), "what was typed is left as typed");
        // Off the screen: a colour comes back, or none if it was dismissed.
        settings.update(Msg::Sampled(None));
        assert_eq!(settings.desktop.appearance.accent, Accent::from_color(Color::hex(0x3F5BC4)));
        settings.update(Msg::Sampled(Some(Color::hex(0xFF2D95))));
        assert_eq!((settings.desktop.appearance.accent, settings.picker.as_ref().unwrap().code.as_str()), (Accent::from_color(Color::hex(0xFF2D95)), "#FF2D95"));
        // It draws, and dragging in the plane of shades picks one.
        let mut h = neo::testing::Harness::new(settings, Size::new(920.0, 640.0)).unwrap();
        h.render(1.0);
        let before = h.app().picker.as_ref().unwrap().hsv;
        let plane = (0..60).map(|i| neo::Point::new(300.0, 120.0 + i as f32 * 8.0)).find(|p| {
            h.move_to(*p);
            h.cursor() == neo::CursorIcon::Crosshair
        });
        h.click(plane.expect("the plane of shades is on the page"));
        let after = h.app().picker.as_ref().unwrap().hsv;
        assert!(after != before && after.h == before.h, "a shade of the same hue: {before:?} then {after:?}");
        let mut settings = std::mem::replace(h.app_mut(), Settings::new());
        // A preset puts the picker away.
        settings.update(Msg::Accent(Accent::Teal));
        assert!(settings.picker.is_none() && settings.desktop.appearance.accent == Accent::Teal);
        settings.update(Msg::Scheme(SchemePref::Dark));
        other.poll();

        // Every page draws, with whatever this computer has or has not.
        use neo::testing::Harness;
        let mut h = Harness::new(settings, Size::new(920.0, 640.0)).unwrap();
        for page in Page::ALL {
            h.app_mut().update(Msg::Page(page));
            h.render(1.0);
        }
        let mut settings = std::mem::replace(h.app_mut(), Settings::new());
        assert_eq!(Page::ALL.len(), 9);

        // The desktop picture: one chosen is the one shown as set.
        settings.update(Msg::Page(Page::Appearance));
        assert!(settings.wallpapers.is_some(), "looked for on coming to the page it is on");
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

        // Changed elsewhere, by the volume keys say, the page follows; but what the system says
        // just after a change made here does not pull the slider back.
        settings.levels = Some(sound::Levels { output: 0.4, muted: false, input: Some(0.5) });
        settings.update(Msg::Page(Page::Sound));
        settings.sound_changed = None;
        assert_eq!(settings.subscriptions().len(), 2, "the page asks again and again while it shows");
        settings.update(Msg::Levels(Some(sound::Levels { output: 0.7, muted: false, input: Some(0.5) }), vec![]));
        assert_eq!(settings.levels.map(|l| l.output), Some(0.7));
        settings.update(Msg::Volume(0.2));
        settings.update(Msg::Levels(Some(sound::Levels { output: 0.7, muted: false, input: Some(0.5) }), vec![]));
        assert_eq!(settings.levels.map(|l| l.output), Some(0.2), "a stale answer just after the slider moved");
        settings.sound_changed = std::time::Instant::now().checked_sub(SOUND_SETTLES);
        settings.update(Msg::Levels(Some(sound::Levels { output: 0.25, muted: true, input: None }), vec![]));
        assert_eq!(settings.levels, Some(sound::Levels { output: 0.25, muted: true, input: None }));
        // Away from the page nothing is asked, and an answer that comes late is not taken.
        settings.update(Msg::Page(Page::About));
        assert_eq!(settings.subscriptions().len(), 1);
        settings.update(Msg::Levels(None, vec![]));
        assert!(settings.levels.is_some());

        // The devices sound goes through: the list opens under its control, and one picked is the one in use.
        let device = |id, name: &str, input: bool, default: bool| sound::Device { id, name: name.into(), input, output: !input, default_input: input && default, default_output: !input && default };
        settings.devices = vec![device(1, "Speakers", false, true), device(2, "Headphones", false, false), device(3, "Microphone", true, true), device(4, "Headset Mic", true, false)];
        settings.levels = Some(sound::Levels { output: 0.4, muted: false, input: Some(0.5) });
        settings.update(Msg::ChooseDevice(false, Rect::new(500.0, 200.0, 260.0, 32.0)));
        assert_eq!(settings.choosing, Some((false, Point::new(500.0, 236.0))));
        let mut h = Harness::new(settings, Size::new(920.0, 640.0)).unwrap();
        h.render(1.0);
        h.key(neo::Key::Escape, neo::Modifiers::default());
        let mut settings = std::mem::replace(h.app_mut(), Settings::new());
        assert_eq!(settings.choosing, None, "Escape closes the list");
        settings.update(Msg::PickDevice(2, false));
        settings.update(Msg::PickDevice(4, true));
        assert_eq!(settings.devices.iter().filter(|d| d.default_output || d.default_input).map(|d| d.name.as_str()).collect::<Vec<_>>(), ["Headphones", "Headset Mic"], "one of each, and only one");

        // The network and Bluetooth: what the system said is shown, and a switch shows at once.
        settings.update(Msg::Page(Page::Network));
        assert_eq!(settings.network, None, "asked for, and not there yet");
        let wifi = network::Connection { name: "Wi-Fi".into(), device: "en0".into(), mac: "7a:41".into(), address: Some("192.168.1.20".into()), router: Some("192.168.1.1".into()), automatic: true, dns: vec![], wifi: true };
        settings.update(Msg::Network(network::Network { connections: vec![wifi], wifi_on: Some(true) }));
        settings.update(Msg::Wifi(false));
        assert_eq!(settings.network.as_ref().and_then(|n| n.wifi_on), Some(false));
        settings.update(Msg::Page(Page::Bluetooth));
        settings.update(Msg::Bluetooth(bluetooth::Bluetooth { on: Some(true), devices: vec![bluetooth::Device { name: "Magic Keyboard".into(), kind: "Keyboard".into(), connected: true, battery: Some("80%".into()) }] }));
        settings.update(Msg::BluetoothPower(false));
        assert_eq!(settings.bluetooth.as_ref().and_then(|b| b.on), Some(false));
        settings.update(Msg::OpenMonitor);

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
