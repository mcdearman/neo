use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Duration;

use neo_render::Size;
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
