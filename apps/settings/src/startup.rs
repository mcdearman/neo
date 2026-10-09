//! Which of Neo's apps start when the user logs in.
//!
//! These are the ones that live in the background: a shortcut or a menu
//! bar icon is no use until its app is running. Each sets itself to start
//! the first time it is run; turned off here, it stays off.

use std::path::PathBuf;

/// An app that can start at login.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Item {
    pub id: &'static str,
    pub name: &'static str,
    program: &'static str,
    bundle: &'static str,
    args: &'static [&'static str],
    /// What is lost without it.
    pub what: &'static str,
}

pub const ITEMS: [Item; 4] = [
    Item { id: "org.neo.Launcher", name: "Launcher", program: "neo-launcher", bundle: "Launcher", args: &["--hidden"], what: "The shortcuts that bring up the app search and the Ask Apollo bar." },
    Item { id: "org.neo.Shell", name: "NeoShell", program: "neo-shell", bundle: "NeoShell", args: &[], what: "Notifications from Neo's apps, and reminders of them." },
    Item { id: "org.neo.Recorder", name: "NeoCap", program: "neo-recorder", bundle: "NeoCap", args: &["--hidden"], what: "The shortcuts that take a screenshot and record the screen." },
    Item { id: "org.neo.Apollo", name: "Apollo", program: "neo-apollo", bundle: "Apollo", args: &["--hidden"], what: "Apollo waiting in the menu bar, ready to answer at once, and reading what changes." },
];

impl Item {
    /// Where the app is, if it is installed.
    pub fn installed(&self) -> Option<PathBuf> {
        neo_desktop::fs::neo_app(self.program, self.bundle)
    }

    /// Whether it is set to start at login.
    pub fn on(&self) -> bool {
        let Some(program) = self.installed() else { return false };
        neo_desktop::autostart::is_enabled(&neo_desktop::autostart::Entry { id: self.id, name: self.name, program: &program, args: self.args })
    }

    /// Sets whether it starts at login, and notes the choice so that the
    /// app does not undo it the next time it runs.
    pub fn set(&self, on: bool) -> Result<(), String> {
        let program = self.installed().ok_or_else(|| format!("{} is not installed.", self.name))?;
        let entry = neo_desktop::autostart::Entry { id: self.id, name: self.name, program: &program, args: self.args };
        let done = neo_desktop::autostart::set_declined(self.id, !on).and_then(|()| if on { neo_desktop::autostart::enable(&entry) } else { neo_desktop::autostart::disable(&entry) });
        done.map_err(|e| format!("Could not change whether {} starts at login: {e}", self.name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_background_apps_are_each_there_once() {
        let mut ids: Vec<&str> = ITEMS.iter().map(|i| i.id).collect();
        ids.dedup();
        assert_eq!(ids.len(), 4);
        assert!(ITEMS.iter().all(|i| !i.what.is_empty() && i.program.starts_with("neo-")));
        // One that is not installed is off, and cannot be turned on.
        let ghost = Item { id: "org.neo.Ghost", name: "Ghost", program: "neo-no-such-app", bundle: "Ghost", args: &[], what: "x" };
        assert!(ghost.installed().is_none() && !ghost.on());
        assert_eq!(ghost.set(true), Err("Ghost is not installed.".into()));
    }
}
