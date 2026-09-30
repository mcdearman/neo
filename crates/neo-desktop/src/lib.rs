//! Shared pieces of the Neo desktop apps.
//!
//! Every Neo app reads the same [`Appearance`] file, so changing the
//! accent or colour scheme in Settings restyles every open window. Apps
//! embed a [`Desktop`], return [`Desktop::theme`] from `App::theme`, and
//! poll it from a subscription:
//!
//! ```no_run
//! use neo::prelude::*;
//! use neo_desktop::Desktop;
//!
//! struct Hello { desktop: Desktop }
//!
//! #[derive(Clone)]
//! enum Msg { Poll }
//!
//! impl App for Hello {
//!     type Message = Msg;
//!     fn theme(&self, system: Scheme) -> Theme { self.desktop.theme(system) }
//!     fn subscriptions(&self) -> Vec<Subscription<Msg>> { vec![Desktop::subscription(Msg::Poll)] }
//!     fn update(&mut self, m: Msg) { match m { Msg::Poll => { self.desktop.poll(); } } }
//!     fn view(&self) -> Element<Msg> { text("Hello").into() }
//! }
//! ```

mod appearance;
pub mod fs;
pub mod ui;

pub use appearance::{config_dir, Appearance, SchemePref};

use std::time::{Duration, SystemTime};

use neo::{Scheme, Subscription, Theme};

/// The desktop-wide settings an app follows, reloaded when the file changes.
pub struct Desktop {
    pub appearance: Appearance,
    stamp: Option<SystemTime>,
}

impl Default for Desktop {
    fn default() -> Self {
        Self::load()
    }
}

impl Desktop {
    pub fn load() -> Self {
        Self { appearance: Appearance::load(), stamp: Appearance::modified() }
    }

    /// The theme for this app, following the user's appearance settings.
    pub fn theme(&self, system: Scheme) -> Theme {
        self.appearance.theme(system)
    }

    /// Reloads the settings if another app saved them. Returns true if they changed.
    pub fn poll(&mut self) -> bool {
        let stamp = Appearance::modified();
        if stamp == self.stamp {
            return false;
        }
        self.stamp = stamp;
        let next = Appearance::load();
        let changed = next != self.appearance;
        self.appearance = next;
        changed
    }

    /// Saves new settings for every app, including this one.
    pub fn save(&mut self, appearance: Appearance) -> std::io::Result<()> {
        self.appearance = appearance;
        let result = appearance.save();
        self.stamp = Appearance::modified();
        result
    }

    /// How often apps check for new settings.
    pub fn subscription<M>(message: M) -> Subscription<M> {
        Subscription::every(Duration::from_millis(750), message)
    }
}
