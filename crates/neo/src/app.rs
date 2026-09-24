use std::time::Duration;

use neo_render::Size;
use neo_theme::{Scheme, Theme};

use crate::core::Element;

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

    /// Timers that send messages periodically.
    fn subscriptions(&self) -> Vec<Subscription<Self::Message>> {
        vec![]
    }

    /// Initial window settings. Read once at startup.
    fn window(&self) -> WindowSettings {
        WindowSettings::default()
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
}

impl Default for WindowSettings {
    fn default() -> Self {
        Self { size: Size::new(960.0, 640.0), min_size: Some(Size::new(360.0, 240.0)), decorations: Decorations::Neo, resizable: true }
    }
}
