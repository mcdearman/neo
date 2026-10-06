//! Shared pieces of the Neo desktop apps.
//!
//! Every Neo app reads the same [`Appearance`] file, so changing the
//! accent or colour scheme in Settings restyles every open window. Apps
//! embed a [`Desktop`], return [`Desktop::theme`] from `App::theme`, and
//! pass its messages back to it. That also gives the app a Settings entry
//! in the menu bar and a settings panel, where this one app can opt out of
//! glass windows:
//!
//! ```no_run
//! use neo::prelude::*;
//! use neo_desktop::Desktop;
//!
//! struct Hello { desktop: Desktop }
//!
//! #[derive(Clone)]
//! enum Msg { Desktop(neo_desktop::DesktopMsg) }
//!
//! impl App for Hello {
//!     type Message = Msg;
//!     fn theme(&self, system: Scheme) -> Theme { self.desktop.theme(system) }
//!     fn subscriptions(&self) -> Vec<Subscription<Msg>> { vec![Desktop::subscription(Msg::Desktop(neo_desktop::DesktopMsg::Poll))] }
//!     fn app_menu(&self) -> Vec<MenuEntry<Msg>> { self.desktop.app_menu(Msg::Desktop) }
//!     fn update(&mut self, m: Msg) { match m { Msg::Desktop(d) => { self.desktop.update(d); } } }
//!     fn view(&self) -> Element<Msg> { self.desktop.with_settings(text("Hello"), "Hello Settings", Msg::Desktop, vec![]) }
//! }
//! ```

mod appearance;
pub mod autostart;
pub mod fs;
pub mod notify;
mod prefs;
pub mod ui;

pub use appearance::{config_dir, Appearance, SchemePref};
pub use prefs::AppPrefs;

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use neo::{Element, MenuEntry, Scheme, Shortcut, Subscription, Theme};

/// What the shared parts of an app ask of it. An app wraps these in one of
/// its own messages and hands them back to [`Desktop::update`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DesktopMsg {
    /// Check whether the settings files changed.
    Poll,
    OpenSettings,
    CloseSettings,
    /// Whether this app's window is glass when the desktop's windows are.
    Glass(bool),
}

/// The settings an app follows: the desktop-wide appearance, reloaded when
/// another app changes it, and this app's own preferences.
pub struct Desktop {
    pub appearance: Appearance,
    stamp: Option<SystemTime>,
    /// This app's own settings.
    pub prefs: AppPrefs,
    prefs_path: PathBuf,
    prefs_stamp: Option<SystemTime>,
    /// Whether the settings panel is showing.
    pub settings_open: bool,
    /// Why the last change could not be saved, if it could not.
    pub error: Option<String>,
}

impl Default for Desktop {
    fn default() -> Self {
        Self::load()
    }
}

impl Desktop {
    pub fn load() -> Self {
        Self::with_prefs_file(AppPrefs::path_for(&AppPrefs::app_name()))
    }

    /// As [`load`](Self::load), keeping this app's own settings in `path`.
    pub fn with_prefs_file(path: PathBuf) -> Self {
        Self { appearance: Appearance::load(), stamp: Appearance::modified(), prefs: AppPrefs::load(&path), prefs_stamp: AppPrefs::modified(&path), prefs_path: path, settings_open: false, error: None }
    }

    /// The theme for this app: the user's appearance settings, without
    /// glass if this app has opted out of it.
    pub fn theme(&self, system: Scheme) -> Theme {
        let mut theme = self.appearance.theme(system);
        theme.glass.enabled &= self.prefs.glass;
        theme
    }

    /// Reloads the settings if another app, or another window of this one,
    /// saved them. Returns true if they changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        let stamp = Appearance::modified();
        if stamp != self.stamp {
            self.stamp = stamp;
            let next = Appearance::load();
            changed |= next != self.appearance;
            self.appearance = next;
        }
        let stamp = AppPrefs::modified(&self.prefs_path);
        if stamp != self.prefs_stamp {
            self.prefs_stamp = stamp;
            let next = AppPrefs::load(&self.prefs_path);
            changed |= next != self.prefs;
            self.prefs = next;
        }
        changed
    }

    /// Carries out a [`DesktopMsg`]. Returns true if how the app looks changed.
    pub fn update(&mut self, message: DesktopMsg) -> bool {
        match message {
            DesktopMsg::Poll => return self.poll(),
            DesktopMsg::OpenSettings => self.settings_open = true,
            DesktopMsg::CloseSettings => self.settings_open = false,
            DesktopMsg::Glass(on) => {
                self.prefs.glass = on;
                self.error = self.prefs.save(&self.prefs_path).err().map(|e| format!("Could not save this setting: {e}"));
                self.prefs_stamp = AppPrefs::modified(&self.prefs_path);
                return true;
            }
        }
        false
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

    /// The Settings entry for [`App::app_menu`](neo::App::app_menu): under
    /// the app's name on macOS, in the first menu elsewhere.
    pub fn app_menu<M>(&self, wrap: impl Fn(DesktopMsg) -> M) -> Vec<MenuEntry<M>> {
        vec![MenuEntry::new("Settings…", wrap(DesktopMsg::OpenSettings)).shortcut(Shortcut::command(","))]
    }

    /// `view`, with this app's settings panel over it while that is open.
    /// `heading` titles the panel, and `extra` is the app's own settings
    /// rows, shown above the ones every app has.
    pub fn with_settings<M: Clone + 'static>(&self, view: impl Into<Element<M>>, heading: &str, wrap: impl Fn(DesktopMsg) -> M + Clone + 'static, extra: Vec<Element<M>>) -> Element<M> {
        if self.settings_open { ui::settings_panel(self, view.into(), heading, wrap, extra) } else { view.into() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desktop(name: &str) -> (Desktop, PathBuf) {
        let path = std::env::temp_dir().join(format!("neo-desktop-{}-{name}", std::process::id())).join("app.conf");
        let _ = std::fs::remove_file(&path);
        (Desktop::with_prefs_file(path.clone()), path)
    }

    #[test]
    fn an_app_can_opt_out_of_glass_on_its_own() {
        let (mut d, path) = desktop("opt-out");
        d.appearance.glass.enabled = true;
        assert!(d.theme(Scheme::Light).glass.enabled, "glass by default when the desktop has it on");
        assert!(d.update(DesktopMsg::Glass(false)));
        assert!(!d.theme(Scheme::Light).glass.enabled);
        assert!(d.appearance.glass.enabled, "the desktop's own setting is untouched");
        assert_eq!(d.error, None);
        // It is remembered, and another window of the same app picks it up.
        assert!(!Desktop::with_prefs_file(path.clone()).prefs.glass);
        let (mut other, _) = (Desktop::with_prefs_file(path.clone()), ());
        other.appearance.glass.enabled = true;
        d.update(DesktopMsg::Glass(true));
        other.prefs_stamp = None;
        assert!(other.poll() && other.prefs.glass);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn opting_in_cannot_turn_glass_on_when_the_desktop_has_it_off() {
        let (mut d, _) = desktop("desktop-off");
        d.appearance.glass.enabled = false;
        assert!(d.prefs.glass);
        assert!(!d.theme(Scheme::Dark).glass.enabled);
    }

    #[test]
    fn settings_open_and_close_and_have_a_menu_entry() {
        let (mut d, _) = desktop("panel");
        let entries = d.app_menu(|m| m);
        assert_eq!(entries.len(), 1);
        assert_eq!((entries[0].label.as_str(), entries[0].message), ("Settings…", Some(DesktopMsg::OpenSettings)));
        assert_eq!(entries[0].shortcut, Some(Shortcut::command(",")));
        assert!(!d.settings_open);
        assert!(!d.update(DesktopMsg::OpenSettings), "opening the panel does not change how the app looks");
        assert!(d.settings_open);
        d.update(DesktopMsg::CloseSettings);
        assert!(!d.settings_open);
    }
}
