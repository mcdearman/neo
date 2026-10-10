use armature::{wgpu, Chrome, Graphics, KeyEvent, Menu, MenuEntry, Proxy, Style, Subscription, WindowGeometry, WindowSettings, WindowState};
use neo_theme::{Scheme, TextRole, Theme};

use crate::core::Element;
use crate::widgets::frame::Frame;

/// A Neo application, in the Elm style: state, messages that change it,
/// and a view that describes the interface for the current state.
pub trait App: 'static {
    type Message: Clone + 'static;

    /// Shown in the title bar and task switcher.
    fn title(&self) -> String {
        "Neo".into()
    }

    /// Applies a message to the application state.
    fn update(&mut self, message: Self::Message);

    /// Describes the interface for the current state.
    fn view(&self) -> Element<Self::Message>;

    /// The theme to use. `system` is the desktop's light or dark preference.
    fn theme(&self, system: Scheme) -> Theme {
        Theme { scheme: system, ..Theme::default() }
    }

    /// The menu bar: File, Edit and so on. On macOS these go in the system's
    /// menu bar at the top of the screen; elsewhere Neo draws them at the
    /// top of the window. Asked after every update, so entries can follow
    /// app state. An entry's shortcut works whether or not its menu is open.
    fn menus(&self) -> Vec<Menu<Self::Message>> {
        vec![]
    }

    /// Entries about the app as a whole, such as Settings. On macOS they go
    /// in the application menu, under the app's name; elsewhere they follow
    /// the entries of the first menu.
    fn app_menu(&self) -> Vec<MenuEntry<Self::Message>> {
        vec![]
    }

    /// Key presses no widget handled, for app-wide shortcuts such as save.
    fn on_key(&self, _key: &KeyEvent) -> Option<Self::Message> {
        None
    }

    /// Called once before the first frame. Keep the proxy to send messages
    /// from other threads, such as a pty reader or a file watcher.
    fn start(&mut self, _proxy: Proxy<Self::Message>) {}

    /// The optional abilities of the graphics device this app would have
    /// turned on, out of those the computer's adapter has. Asked once,
    /// before the device is made; look in [`Graphics::features`] for what
    /// was given. The default asks for none.
    fn wanted_features(&self, _available: wgpu::Features) -> wgpu::Features {
        wgpu::Features::empty()
    }

    /// The limits the graphics device is to be made with, given the most
    /// the adapter allows. The default is wgpu's own.
    fn wanted_limits(&self, _available: &wgpu::Limits) -> wgpu::Limits {
        wgpu::Limits::default()
    }

    /// The graphics device the window is drawn with, for an app that draws
    /// with it too: a game rendering into a texture that is then shown in
    /// a [`viewport`](crate::widgets::viewport) as an `Image::from_texture`.
    /// Called once, before the first view; the device and queue are
    /// handles, to be cloned and kept.
    fn graphics(&mut self, _graphics: &Graphics) {}

    /// Called before each frame is drawn, after the input that came before
    /// it, with the time and how long it has been since the last: where a
    /// game moves on and draws its own frame, so that what the window then
    /// shows is this frame's. Return true if what the app shows may have
    /// changed. Frames are drawn only when something asks for one; a
    /// viewport that is playing asks for every one.
    fn step(&mut self, _now: std::time::Instant, _dt: std::time::Duration) -> bool {
        false
    }

    /// Whether [`step`](Self::step) goes on being called while the window
    /// cannot be seen (covered, minimised, hidden), ten times a second and
    /// with no frame drawn, for as long as something on its page wants
    /// frames. For an app whose work goes on unseen. Not, by default.
    fn steps_unseen(&self) -> bool {
        false
    }

    /// Timers that send messages periodically.
    fn subscriptions(&self) -> Vec<Subscription<Self::Message>> {
        vec![]
    }

    /// How the window presents itself right now: shown or hidden, on top,
    /// see-through. Read after every update, so it can follow app state.
    fn window_state(&self) -> WindowState {
        WindowState::default()
    }

    /// Called when the user asks to close the window. Return a message to
    /// handle it yourself, for example by hiding the window; `None` closes
    /// the window and ends the app.
    fn on_close(&self) -> Option<Self::Message> {
        None
    }

    /// Called when the window moves or changes size, with where it is on
    /// the screen.
    fn on_window_geometry(&self, _geometry: WindowGeometry) -> Option<Self::Message> {
        None
    }

    /// Called when the window gains or loses keyboard focus, as when the
    /// user clicks another app. A pop-up can use this to dismiss itself.
    fn on_window_focus(&self, _focused: bool) -> Option<Self::Message> {
        None
    }

    /// Called when the app is asked to open again while it is running: its
    /// Dock icon clicked, or opened again from the Finder or a launcher.
    /// An app that keeps running with its window out of sight shows it
    /// again here. Only macOS tells of this so far.
    fn on_reopen(&self) -> Option<Self::Message> {
        None
    }

    /// Called once as the app ends, however it ends: the window closed,
    /// Quit chosen from the menu, or the system shutting it down. The
    /// place to stop anything that would otherwise outlive it, such as a
    /// program it started.
    fn on_exit(&mut self) {}

    /// Return true to end the app. Checked after every update.
    fn should_exit(&self) -> bool {
        false
    }

    /// Initial window settings. Read once at startup.
    fn window(&self) -> WindowSettings {
        WindowSettings::default()
    }
}

/// Runs a Neo [`App`] on the framework underneath: supplies the theme as
/// the window's style, Neo's typefaces, and the Neo title bar.
pub struct Themed<A>(pub A);

fn scheme(s: armature::Scheme) -> Scheme {
    match s {
        armature::Scheme::Light => Scheme::Light,
        armature::Scheme::Dark => Scheme::Dark,
    }
}

impl<A: App> armature::App for Themed<A> {
    type Message = A::Message;

    fn title(&self) -> String {
        self.0.title()
    }

    fn update(&mut self, message: Self::Message) {
        self.0.update(message)
    }

    fn view(&self) -> Element<Self::Message> {
        self.0.view()
    }

    fn style(&self, system: armature::Scheme) -> Style {
        let theme = self.0.theme(scheme(system));
        Style::new(theme).content(theme.palette().text).text(theme.text(TextRole::Body).style()).window_radius(theme.window_radius()).backdrop_blur(theme.glass.enabled.then_some(theme.glass.blur))
    }

    fn fonts(&self) -> armature::Fonts {
        neo_theme::fonts::bundled()
    }

    fn frame(&self, view: Element<Self::Message>, chrome: Chrome) -> Element<Self::Message> {
        let menus = if chrome.menu_bar { armature::with_app_entries(self.0.menus(), self.0.app_menu()) } else { vec![] };
        Element::new(Frame::new(view, chrome.title, chrome.title_bar, chrome.rounded, chrome.background).menus(menus))
    }

    fn menus(&self) -> Vec<Menu<Self::Message>> {
        self.0.menus()
    }

    fn app_menu(&self) -> Vec<MenuEntry<Self::Message>> {
        self.0.app_menu()
    }

    fn on_key(&self, key: &KeyEvent) -> Option<Self::Message> {
        self.0.on_key(key)
    }

    fn wanted_features(&self, available: wgpu::Features) -> wgpu::Features {
        self.0.wanted_features(available)
    }

    fn wanted_limits(&self, available: &wgpu::Limits) -> wgpu::Limits {
        self.0.wanted_limits(available)
    }

    fn graphics(&mut self, graphics: &Graphics) {
        self.0.graphics(graphics);
    }

    fn step(&mut self, now: std::time::Instant, dt: std::time::Duration) -> bool {
        self.0.step(now, dt)
    }

    fn steps_unseen(&self) -> bool {
        self.0.steps_unseen()
    }

    fn start(&mut self, proxy: Proxy<Self::Message>) {
        self.0.start(proxy)
    }

    fn subscriptions(&self) -> Vec<Subscription<Self::Message>> {
        self.0.subscriptions()
    }

    fn window_state(&self) -> WindowState {
        self.0.window_state()
    }

    fn on_close(&self) -> Option<Self::Message> {
        self.0.on_close()
    }

    fn on_window_geometry(&self, geometry: WindowGeometry) -> Option<Self::Message> {
        self.0.on_window_geometry(geometry)
    }

    fn on_window_focus(&self, focused: bool) -> Option<Self::Message> {
        self.0.on_window_focus(focused)
    }

    fn on_reopen(&self) -> Option<Self::Message> {
        self.0.on_reopen()
    }

    fn on_exit(&mut self) {
        self.0.on_exit()
    }

    fn should_exit(&self) -> bool {
        self.0.should_exit()
    }

    fn window(&self) -> WindowSettings {
        self.0.window()
    }
}

/// Opens a window and runs `app` until it closes.
pub fn run<A: App>(app: A) -> Result<(), armature::Error> {
    armature::run(Themed(app))
}
