//! Notifications put aside to be shown again later.
//!
//! They are kept in a file, so that one set for tomorrow is still there
//! after a restart, and one whose time came while NeoShell was not running
//! is shown when it next starts.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use neo_desktop::notify::Notification;

/// How long to put a notification aside for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Later {
    TenMinutes,
    Hour,
    Tomorrow,
}

impl Later {
    pub const ALL: [Later; 3] = [Later::TenMinutes, Later::Hour, Later::Tomorrow];

    pub fn label(self) -> &'static str {
        match self {
            Later::TenMinutes => "10 minutes",
            Later::Hour => "1 hour",
            Later::Tomorrow => "Tomorrow",
        }
    }

    pub fn seconds(self) -> u64 {
        match self {
            Later::TenMinutes => 10 * 60,
            Later::Hour => 60 * 60,
            Later::Tomorrow => 24 * 60 * 60,
        }
    }
}

/// A notification and when to show it again, in seconds since 1970.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reminder {
    pub due: u64,
    pub note: Notification,
}

/// The time now, in seconds since 1970.
pub fn wall() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Where reminders are kept.
pub fn file() -> PathBuf {
    neo_desktop::config_dir().join("reminders")
}

/// Each reminder as its time and then the notification's own lines, with
/// an empty line between one and the next.
pub fn encode(reminders: &[Reminder]) -> String {
    reminders.iter().map(|r| format!("due={}\n{}", r.due, r.note.encode())).collect::<Vec<_>>().join("\n")
}

/// Reads what [`encode`] wrote, leaving out anything it cannot make sense of.
pub fn decode(text: &str) -> Vec<Reminder> {
    text.split("\n\n")
        .filter_map(|part| {
            let due = part.lines().find_map(|l| l.strip_prefix("due="))?.trim().parse().ok()?;
            Some(Reminder { due, note: Notification::decode(part)? })
        })
        .collect()
}

pub fn load(path: &Path) -> Vec<Reminder> {
    std::fs::read_to_string(path).map(|text| decode(&text)).unwrap_or_default()
}

/// Writes the reminders, or removes the file when there are none. Written
/// beside the file and moved over it, so a crash leaves the old ones.
pub fn save(path: &Path, reminders: &[Reminder]) {
    if reminders.is_empty() {
        let _ = std::fs::remove_file(path);
        return;
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let part = path.with_extension("part");
    if std::fs::write(&part, encode(reminders)).is_ok() {
        let _ = std::fs::rename(&part, path);
    }
}

/// When a reminder is due, said from `now`: "in 10 minutes", "in 3 hours".
pub fn when(due: u64, now: u64) -> String {
    let left = due.saturating_sub(now);
    let count = |n: u64, unit: &str| format!("in {n} {unit}{}", if n == 1 { "" } else { "s" });
    match left {
        0 => "now".into(),
        1..60 => "in under a minute".into(),
        60..3600 => count(left / 60, "minute"),
        3600..86_400 => count(left / 3600, "hour"),
        _ => count(left / 86_400, "day"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reminders_are_written_and_read_back() {
        let all = vec![Reminder { due: 1_800_000_000, note: Notification::new("Recording saved", "clip.mov").reveal("/tmp/a clip.mov").open("/tmp/a clip.mov") }, Reminder { due: 1_800_000_600, note: Notification::new("Two\nlines", "") }, Reminder { due: 5, note: Notification::new("due=7", "a title that looks like a time").image("/tmp/p.png") }];
        assert_eq!(decode(&encode(&all)), all);
        assert_eq!(decode(""), vec![]);
        // Something without a time, or without a title, is left out.
        assert_eq!(decode("title=No time\nbody=\n\ndue=12\nbody=no title\n\ndue=9\ntitle=Kept\nbody=\n").iter().map(|r| r.note.title.as_str()).collect::<Vec<_>>(), ["Kept"]);
    }

    #[test]
    fn the_file_is_kept_only_while_there_is_something_in_it() {
        let path = std::env::temp_dir().join(format!("neo-shell-reminders-{}", std::process::id()));
        let one = vec![Reminder { due: 10, note: Notification::new("One", "") }];
        save(&path, &one);
        assert_eq!(load(&path), one);
        save(&path, &[]);
        assert!(!path.exists());
        assert_eq!(load(&path), vec![]);
    }

    #[test]
    fn the_wait_is_said_in_words() {
        assert_eq!((when(700, 100), when(160, 100), when(100 + 3 * 3600, 100), when(100 + 86_400, 100), when(130, 100), when(50, 100)), ("in 10 minutes".into(), "in 1 minute".into(), "in 3 hours".into(), "in 1 day".into(), "in under a minute".into(), "now".into()));
        assert_eq!(Later::ALL.map(Later::seconds), [600, 3600, 86_400]);
    }
}
