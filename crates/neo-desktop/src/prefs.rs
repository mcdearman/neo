use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::config_dir;

/// Settings that belong to one app rather than the whole desktop.
///
/// Stored as `key = value` lines in `apps/<app>.conf` under [`config_dir`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppPrefs {
    /// Whether this app's window is glass when the desktop's windows are.
    /// Off makes this one app solid.
    pub glass: bool,
}

impl Default for AppPrefs {
    fn default() -> Self {
        Self { glass: true }
    }
}

impl AppPrefs {
    /// The name this app's settings are kept under: its program's name.
    pub fn app_name() -> String {
        std::env::current_exe().ok().and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned())).unwrap_or_else(|| "app".into())
    }

    /// Where the settings of the app called `app` are kept.
    pub fn path_for(app: &str) -> PathBuf {
        config_dir().join("apps").join(format!("{app}.conf"))
    }

    pub fn parse(src: &str) -> Self {
        let mut p = Self::default();
        for line in src.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            if k.trim() == "glass" {
                match v.trim() {
                    "true" | "yes" | "on" => p.glass = true,
                    "false" | "no" | "off" => p.glass = false,
                    _ => {}
                }
            }
        }
        p
    }

    pub fn serialize(&self) -> String {
        format!("# Settings for this Neo app alone.\nglass = {}\n", self.glass)
    }

    /// Reads the settings file. A missing or unreadable file gives the defaults.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path).map(|s| Self::parse(&s)).unwrap_or_default()
    }

    pub(crate) fn modified(path: &Path) -> Option<SystemTime> {
        std::fs::metadata(path).and_then(|m| m.modified()).ok()
    }

    /// Writes the settings atomically, so readers never see half a file.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("conf.tmp");
        std::fs::write(&tmp, self.serialize())?;
        std::fs::rename(tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_defaults_to_glass() {
        assert!(AppPrefs::default().glass);
        assert_eq!(AppPrefs::parse(""), AppPrefs::default());
        assert_eq!(AppPrefs::parse("glass = maybe\nother = 1"), AppPrefs::default(), "junk keeps the default");
        let solid = AppPrefs { glass: false };
        assert_eq!(AppPrefs::parse(&solid.serialize()), solid);
    }

    #[test]
    fn saves_to_and_loads_from_its_file() {
        let path = std::env::temp_dir().join(format!("neo-prefs-{}", std::process::id())).join("apps").join("demo.conf");
        assert_eq!(AppPrefs::load(&path), AppPrefs::default(), "no file yet");
        AppPrefs { glass: false }.save(&path).unwrap();
        assert!(!AppPrefs::load(&path).glass);
        std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap()).unwrap();
    }
}
