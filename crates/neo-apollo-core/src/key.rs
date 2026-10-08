//! The key to Apollo's memory, and the check that looking through it asks for.
//!
//! The database is encrypted with a key made the first time it is needed
//! and kept in the system's keychain, which is itself locked by the user's
//! login: a copy of the disk, a backup, or another user's account cannot
//! read the memory. Apollo reads the key without asking, so that it can
//! go on remembering in the background. Looking *through* the memory is
//! another matter: the Apollo app asks for the user's login first, by
//! Touch ID or password, with [`authenticate`].

/// What the key is filed under.
const SERVICE: &str = "org.neo.Apollo";
const ACCOUNT: &str = "memory";

fn hex(key: &[u8; 32]) -> String {
    key.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<[u8; 32]> {
    let text = text.trim();
    if text.len() != 64 || !text.is_ascii() {
        return None;
    }
    let mut key = [0u8; 32];
    for (i, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(key)
}

fn fresh() -> Result<[u8; 32], String> {
    let mut key = [0u8; 32];
    getrandom::fill(&mut key).map_err(|e| format!("Couldn't make a key: {e}"))?;
    Ok(key)
}

/// The database's key: read from where it is kept, or made and put there.
pub fn database_key() -> Result<[u8; 32], String> {
    if let Some(key) = imp::read()? {
        return Ok(key);
    }
    let key = fresh()?;
    imp::write(&key)?;
    // Read back, so that a key that was not kept is found out now and not
    // after a database has been made that nothing can open.
    match imp::read()? {
        Some(kept) if kept == key => Ok(key),
        _ => Err("The key to Apollo's memory could not be kept.".into()),
    }
}

/// Asks the user to prove they are the computer's owner, by Touch ID or
/// their password, saying `reason`. Blocks until they have answered.
pub fn authenticate(reason: &str) -> Result<(), String> {
    imp::authenticate(reason)
}

/// A key kept in a file only its owner can read: for systems with no
/// keychain to ask.
#[cfg_attr(target_os = "macos", allow(dead_code))]
mod file {
    use super::*;

    fn path() -> std::path::PathBuf {
        crate::dir().join("key")
    }

    pub fn read() -> Result<Option<[u8; 32]>, String> {
        Ok(std::fs::read_to_string(path()).ok().and_then(|t| unhex(&t)))
    }

    pub fn write(key: &[u8; 32]) -> Result<(), String> {
        let path = path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        std::io::Write::write_all(&mut options.open(&path).map_err(|e| e.to_string())?, hex(key).as_bytes()).map_err(|e| e.to_string())
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use std::io::Write;
    use std::process::{Command, Stdio};

    /// The login keychain, through the system's own tool. Any app of the
    /// user's may read the item, so that a rebuilt Apollo is not stopped
    /// and asked for the login password every time it starts.
    pub fn read() -> Result<Option<[u8; 32]>, String> {
        let out = Command::new("/usr/bin/security").args(["find-generic-password", "-s", SERVICE, "-a", ACCOUNT, "-w"]).stdin(Stdio::null()).output().map_err(|e| format!("Couldn't ask the keychain: {e}"))?;
        Ok(out.status.success().then(|| unhex(&String::from_utf8_lossy(&out.stdout))).flatten())
    }

    pub fn write(key: &[u8; 32]) -> Result<(), String> {
        // Given on the tool's input, not its command line, where other
        // programs could see it.
        let mut tool = Command::new("/usr/bin/security").arg("-i").stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(|e| format!("Couldn't ask the keychain: {e}"))?;
        let line = format!("add-generic-password -U -A -s {SERVICE} -a {ACCOUNT} -l \"Apollo memory\" -w {}\n", hex(key));
        tool.stdin.take().ok_or("no input")?.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
        tool.wait().map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn authenticate(reason: &str) -> Result<(), String> {
        use block2::RcBlock;
        use objc2::runtime::Bool;
        use objc2_foundation::{NSError, NSString};
        use objc2_local_authentication::{LAContext, LAPolicy};

        // SAFETY: plain calls into LocalAuthentication; the reply block
        // owns what it captures and may be called on any thread.
        unsafe {
            let context = LAContext::new();
            // Touch ID or the watch where there is one, the password otherwise.
            let policy = LAPolicy::DeviceOwnerAuthentication;
            if let Err(e) = context.canEvaluatePolicy_error(policy) {
                return Err(format!("This computer cannot check who you are: {}", e.localizedDescription()));
            }
            let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
            let reply = RcBlock::new(move |ok: Bool, error: *mut NSError| {
                let outcome = if ok.as_bool() {
                    Ok(())
                } else if error.is_null() {
                    Err("Not unlocked.".to_owned())
                } else {
                    Err((*error).localizedDescription().to_string())
                };
                let _ = tx.send(outcome);
            });
            context.evaluatePolicy_localizedReason_reply(policy, &NSString::from_str(reason), &reply);
            rx.recv().unwrap_or_else(|_| Err("Not unlocked.".into()))
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use super::*;
    use std::io::Write;
    use std::process::{Command, Stdio};

    fn secret_tool() -> Option<std::path::PathBuf> {
        Some(neo_desktop::fs::tool("secret-tool")).filter(|p| p.is_file())
    }

    /// The desktop's keyring through `secret-tool`, where there is one,
    /// and a file only the user can read where there is not.
    pub fn read() -> Result<Option<[u8; 32]>, String> {
        if let Some(tool) = secret_tool()
            && let Ok(out) = Command::new(tool).args(["lookup", "service", SERVICE, "account", ACCOUNT]).stdin(Stdio::null()).output()
            && let Some(key) = unhex(&String::from_utf8_lossy(&out.stdout))
        {
            return Ok(Some(key));
        }
        file::read()
    }

    pub fn write(key: &[u8; 32]) -> Result<(), String> {
        if let Some(tool) = secret_tool()
            && let Ok(mut child) = Command::new(tool).args(["store", "--label", "Apollo memory", "service", SERVICE, "account", ACCOUNT]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
        {
            let wrote = child.stdin.take().is_some_and(|mut input| input.write_all(hex(key).as_bytes()).is_ok());
            if child.wait().is_ok_and(|s| s.success()) && wrote {
                return Ok(());
            }
        }
        file::write(key)
    }

    /// polkit asks for an administrator's password before running anything
    /// as one; asked to run nothing, that is the check on its own.
    pub fn authenticate(_reason: &str) -> Result<(), String> {
        let pkexec = neo_desktop::fs::tool("pkexec");
        if !pkexec.is_file() {
            return Err("This system has no polkit to check who you are with.".into());
        }
        match Command::new(pkexec).arg("true").stdin(Stdio::null()).status() {
            Ok(s) if s.success() => Ok(()),
            Ok(_) => Err("Not unlocked.".into()),
            Err(e) => Err(format!("Couldn't ask who you are: {e}")),
        }
    }
}

#[cfg(not(unix))]
mod imp {
    use super::*;

    pub fn read() -> Result<Option<[u8; 32]>, String> {
        file::read()
    }

    pub fn write(key: &[u8; 32]) -> Result<(), String> {
        file::write(key)
    }

    pub fn authenticate(_reason: &str) -> Result<(), String> {
        Err("Checking who you are is not there yet on this system.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_written_as_text_and_read_back() {
        let key: [u8; 32] = std::array::from_fn(|i| (i * 7) as u8);
        assert_eq!(hex(&key).len(), 64);
        assert_eq!(unhex(&format!(" {}\n", hex(&key))), Some(key));
        assert_eq!((unhex(""), unhex("zz"), unhex(&"g".repeat(64)), unhex(&"0".repeat(63))), (None, None, None, None));
        // Two made are not the same, and not nothing.
        let (a, b) = (fresh().unwrap(), fresh().unwrap());
        assert!(a != b && a != [0; 32]);
    }
}

#[cfg(test)]
mod probe {
    /// By hand: makes the real key in the keychain if it is not there, and
    /// reads it back. `cargo test -p neo-apollo-core real_key -- --ignored`
    #[test]
    #[ignore]
    fn the_real_key_is_kept_and_read_back() {
        let first = super::database_key().unwrap();
        assert_eq!(super::database_key().unwrap(), first, "the same key every time");
        assert_ne!(first, [0; 32]);
    }
}
