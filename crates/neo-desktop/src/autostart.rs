//! Starting an app when the user logs in.
//!
//! - macOS: a LaunchAgent in `~/Library/LaunchAgents`.
//! - Linux: a desktop entry in `~/.config/autostart`, which GNOME, KDE and
//!   other freedesktop.org sessions run.
//! - Windows: a value under the user's `Run` registry key.

use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;

/// What to start at login.
pub struct Entry<'a> {
    /// Reverse-DNS app ID, such as `org.neo.Recorder`.
    pub id: &'a str,
    /// Name shown in the system's list of startup items.
    pub name: &'a str,
    pub program: &'a Path,
    pub args: &'a [&'a str],
}

#[cfg(unix)]
fn home() -> PathBuf {
    crate::fs::home_dir()
}

#[cfg(target_os = "macos")]
fn file(id: &str) -> PathBuf {
    home().join("Library/LaunchAgents").join(format!("{id}.plist"))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn file(id: &str) -> PathBuf {
    let config = std::env::var_os("XDG_CONFIG_HOME").filter(|d| !d.is_empty()).map(PathBuf::from).unwrap_or_else(|| home().join(".config"));
    config.join("autostart").join(format!("{id}.desktop"))
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The file that starts the app, for the platforms that use one.
#[cfg_attr(windows, allow(dead_code))]
fn contents(e: &Entry) -> String {
    let program = e.program.to_string_lossy();
    if cfg!(target_os = "macos") {
        let args: String = std::iter::once(program.as_ref()).chain(e.args.iter().copied()).map(|a| format!("        <string>{}</string>\n", xml(a))).collect();
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n    <key>Label</key><string>{}</string>\n    <key>ProgramArguments</key>\n    <array>\n{args}    </array>\n    <key>RunAtLoad</key><true/>\n    <key>ProcessType</key><string>Interactive</string>\n</dict>\n</plist>\n",
            xml(e.id)
        )
    } else {
        // Desktop entries quote arguments with double quotes.
        let quote = |a: &str| format!("\"{}\"", a.replace('\\', "\\\\").replace('"', "\\\""));
        let exec: Vec<String> = std::iter::once(program.as_ref()).chain(e.args.iter().copied()).map(quote).collect();
        format!("[Desktop Entry]\nType=Application\nName={}\nExec={}\nX-GNOME-Autostart-enabled=true\nNoDisplay=true\n", e.name, exec.join(" "))
    }
}

#[cfg(windows)]
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

#[cfg(windows)]
fn command_line(e: &Entry) -> String {
    std::iter::once(e.program.to_string_lossy().into_owned()).chain(e.args.iter().map(|a| a.to_string())).map(|a| format!("\"{a}\"")).collect::<Vec<_>>().join(" ")
}

/// Where it is noted which apps the user has said are not to start at login.
fn declined_file() -> std::path::PathBuf {
    crate::config_dir().join("startup.conf")
}

/// The apps noted in `text` as not to start at login, by their IDs.
fn declined_in(text: &str) -> Vec<String> {
    text.lines().filter_map(|l| l.split_once('=')).filter(|(_, v)| v.trim() == "false").map(|(id, _)| id.trim().to_owned()).filter(|id| !id.is_empty()).collect()
}

/// Whether the user has said this app is not to start at login. Neo's
/// background apps set themselves to start when they are first run, so
/// that their shortcuts work after a restart; one turned off in Settings
/// is to stay off, and this is how it knows.
pub fn declined(id: &str) -> bool {
    declined_in(&std::fs::read_to_string(declined_file()).unwrap_or_default()).iter().any(|d| d == id)
}

/// Notes that the user does not want this app started at login, or that
/// they do again.
pub fn set_declined(id: &str, declined: bool) -> std::io::Result<()> {
    let path = declined_file();
    let mut ids = declined_in(&std::fs::read_to_string(&path).unwrap_or_default());
    ids.retain(|d| d != id);
    if declined {
        ids.push(id.to_owned());
    }
    ids.sort();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, ids.iter().map(|id| format!("{id} = false\n")).collect::<String>())
}

/// Whether the app is set to start at login exactly as `e` describes. A
/// startup item that points at an older copy of the program counts as not set.
pub fn is_enabled(e: &Entry) -> bool {
    #[cfg(unix)]
    {
        std::fs::read_to_string(file(e.id)).is_ok_and(|s| s == contents(e))
    }
    #[cfg(windows)]
    {
        let out = std::process::Command::new("reg").args(["query", RUN_KEY, "/v", e.name]).output();
        out.is_ok_and(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).contains(&command_line(e)))
    }
}

/// Makes the app start at login, replacing any earlier startup item for it.
pub fn enable(e: &Entry) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let path = file(e.id);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, contents(e))
    }
    #[cfg(windows)]
    {
        let status = std::process::Command::new("reg").args(["add", RUN_KEY, "/v", e.name, "/t", "REG_SZ", "/d", &command_line(e), "/f"]).output()?.status;
        if status.success() { Ok(()) } else { Err(std::io::Error::other("could not write the startup registry value")) }
    }
}

/// Stops the app starting at login. Succeeds if it was not set to.
pub fn disable(e: &Entry) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        match std::fs::remove_file(file(e.id)) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err),
            _ => Ok(()),
        }
    }
    #[cfg(windows)]
    {
        // Deleting a value that is not there fails, which is fine.
        let _ = std::process::Command::new("reg").args(["delete", RUN_KEY, "/v", e.name, "/f"]).output()?;
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn apps_turned_off_at_login_are_noted() {
        assert_eq!(declined_in("org.neo.Shell = false\norg.neo.Launcher=true\n\n org.neo.Apollo = false \nnonsense\n"), ["org.neo.Shell", "org.neo.Apollo"]);
        assert_eq!(declined_in(""), Vec::<String>::new());
    }

    #[test]
    fn describes_the_program_and_its_arguments() {
        let e = Entry { id: "org.neo.Test", name: "Neo Test", program: Path::new("/Apps/Neo Test.app/Contents/MacOS/neo-test"), args: &["--hidden"] };
        let c = contents(&e);
        assert!(c.contains("/Apps/Neo Test.app/Contents/MacOS/neo-test"));
        assert!(c.contains("--hidden"));
        if cfg!(target_os = "macos") {
            assert!(c.contains("<key>Label</key><string>org.neo.Test</string>") && c.contains("<key>RunAtLoad</key><true/>"));
        } else {
            assert!(c.contains("Exec=\"/Apps/Neo Test.app/Contents/MacOS/neo-test\" \"--hidden\""));
        }
    }
}
