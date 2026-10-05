use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Duration;

use neo_render::{Point, Rect, Size};
use neo_theme::{Scheme, Theme};

use crate::core::Element;
use crate::event::KeyEvent;

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

    /// Key presses no widget handled, for app-wide shortcuts such as save.
    fn on_key(&self, _key: &KeyEvent) -> Option<Self::Message> {
        None
    }

    /// Called once before the first frame. Keep the proxy to send messages
    /// from other threads, such as a pty reader or a file watcher.
    fn start(&mut self, _proxy: Proxy<Self::Message>) {}

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

    /// Return true to end the app. Checked after every update.
    fn should_exit(&self) -> bool {
        false
    }

    /// Initial window settings. Read once at startup.
    fn window(&self) -> WindowSettings {
        WindowSettings::default()
    }
}

/// Sends messages to a running app from any thread and wakes its event loop.
pub struct Proxy<M> {
    pub(crate) tx: Sender<M>,
    pub(crate) wake: Arc<dyn Fn() + Send + Sync>,
}

impl<M> Clone for Proxy<M> {
    fn clone(&self) -> Self {
        Self { tx: self.tx.clone(), wake: self.wake.clone() }
    }
}

impl<M> Proxy<M> {
    /// Queues `message` for `App::update`. Returns false once the app has exited.
    pub fn send(&self, message: M) -> bool {
        let ok = self.tx.send(message).is_ok();
        if ok {
            (self.wake)();
        }
        ok
    }
}

/// Sends `message` every `period`.
#[derive(Clone, Debug)]
pub struct Subscription<M> {
    pub period: Duration,
    pub message: M,
}

impl<M> Subscription<M> {
    pub fn every(period: Duration, message: M) -> Self {
        Self { period, message }
    }
}

/// How a window presents itself. See [`App::window_state`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowState {
    /// Hidden windows keep running, for example to wait for a global shortcut.
    pub visible: bool,
    /// Stay above other apps' windows.
    pub always_on_top: bool,
    /// Draw no window background or title bar. Only what the view paints
    /// shows; the rest of the window is see-through.
    pub bare: bool,
    /// Resize the window's content area to this whenever the value changes.
    pub size: Option<Size>,
    /// Move the window's top-left corner here, in logical screen pixels,
    /// whenever the value changes. Ignored where the compositor places
    /// windows itself, as on Wayland.
    pub position: Option<Point>,
    /// Leave the window out of screenshots and screen recordings, where the
    /// platform can.
    pub hidden_from_capture: bool,
}

impl Default for WindowState {
    fn default() -> Self {
        Self { visible: true, always_on_top: false, bare: false, size: None, position: None, hidden_from_capture: false }
    }
}

/// Where a window is. See [`App::on_window_geometry`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowGeometry {
    /// The window's content area on the screen, in logical pixels.
    /// Compositors that hide window positions, such as Wayland ones, report
    /// the origin as zero.
    pub frame: Rect,
    /// The screen the window is on, in the same coordinates.
    pub screen: Rect,
    /// Physical pixels per logical pixel.
    pub scale: f32,
}

/// Who draws the title bar and window border.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Decorations {
    /// Neo draws the title bar, window controls and rounded corners, so
    /// windows look the same on every platform. The default.
    #[default]
    Neo,
    /// The platform's native title bar.
    System,
}

#[derive(Clone, Debug)]
pub struct WindowSettings {
    /// Initial content size in logical pixels.
    pub size: Size,
    pub min_size: Option<Size>,
    pub decorations: Decorations,
    pub resizable: bool,
    /// Identifies the app to the desktop, such as `org.neo.Files`. On Linux
    /// this is the Wayland app ID and X11 class, which must match the name
    /// of the app's `.desktop` file for docks and menus to show its icon.
    pub app_id: Option<String>,
}

impl Default for WindowSettings {
    fn default() -> Self {
        Self { size: Size::new(960.0, 640.0), min_size: Some(Size::new(360.0, 240.0)), decorations: Decorations::Neo, resizable: true, app_id: None }
    }
}
