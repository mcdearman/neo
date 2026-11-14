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
#[cfg_attr(windows, allow(dead_code))]
const SERVICE: &str = "org.neo.Apollo";
#[cfg_attr(windows, allow(dead_code))]
const ACCOUNT: &str = "memory";

#[cfg_attr(windows, allow(dead_code))]
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
#[cfg_attr(any(target_os = "macos", windows), allow(dead_code))]
mod file {
    use super::*;

    pub fn path() -> std::path::PathBuf {
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
    use std::process::Stdio;

    /// The login keychain, through the system's own tool. Any app of the
    /// user's may read the item, so that a rebuilt Apollo is not stopped
    /// and asked for the login password every time it starts.
    pub fn read() -> Result<Option<[u8; 32]>, String> {
        let out = neo_desktop::command("/usr/bin/security").args(["find-generic-password", "-s", SERVICE, "-a", ACCOUNT, "-w"]).stdin(Stdio::null()).output().map_err(|e| format!("Couldn't ask the keychain: {e}"))?;
        Ok(out.status.success().then(|| unhex(&String::from_utf8_lossy(&out.stdout))).flatten())
    }

    pub fn write(key: &[u8; 32]) -> Result<(), String> {
        // Given on the tool's input, not its command line, where other
        // programs could see it.
        let mut tool = neo_desktop::command("/usr/bin/security").arg("-i").stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(|e| format!("Couldn't ask the keychain: {e}"))?;
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
    use std::process::Stdio;

    fn secret_tool() -> Option<std::path::PathBuf> {
        Some(neo_desktop::fs::tool("secret-tool")).filter(|p| p.is_file())
    }

    /// The desktop's keyring through `secret-tool`, where there is one,
    /// and a file only the user can read where there is not.
    pub fn read() -> Result<Option<[u8; 32]>, String> {
        if let Some(tool) = secret_tool()
            && let Ok(out) = neo_desktop::command(tool).args(["lookup", "service", SERVICE, "account", ACCOUNT]).stdin(Stdio::null()).output()
            && let Some(key) = unhex(&String::from_utf8_lossy(&out.stdout))
        {
            return Ok(Some(key));
        }
        file::read()
    }

    pub fn write(key: &[u8; 32]) -> Result<(), String> {
        if let Some(tool) = secret_tool()
            && let Ok(mut child) = neo_desktop::command(tool).args(["store", "--label", "Apollo memory", "service", SERVICE, "account", ACCOUNT]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
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
        match neo_desktop::command(pkexec).arg("true").stdin(Stdio::null()).status() {
            Ok(s) if s.success() => Ok(()),
            Ok(_) => Err("Not unlocked.".into()),
            Err(e) => Err(format!("Couldn't ask who you are: {e}")),
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use windows::Security::Credentials::UI::{UserConsentVerificationResult, UserConsentVerifier, UserConsentVerifierAvailability};
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData};
    use windows::Win32::System::WinRT::{IUserConsentVerifierInterop, RO_INIT_MULTITHREADED, RoInitialize};
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    use windows::core::{HSTRING, PCWSTR, factory};
    use windows_future::IAsyncOperation;

    /// What a key locked by the user's login begins with in its file.
    const LOCKED: &str = "dpapi:";

    /// Locks or unlocks `data` with the user's Windows login: what comes
    /// out locked can be read by this user on this computer and no other.
    fn protect(data: &[u8], lock: bool) -> Result<Vec<u8>, String> {
        let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr().cast_mut() };
        let mut out = CRYPT_INTEGER_BLOB::default();
        // SAFETY: the input is `data`, which outlives the call, and what
        // Windows gives back is copied and then freed as it asks.
        unsafe {
            let done = if lock {
                CryptProtectData(&input, PCWSTR::null(), None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out)
            } else {
                CryptUnprotectData(&input, None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out)
            };
            done.map_err(|e| format!("Windows could not {} the key to Apollo's memory: {e}", if lock { "lock" } else { "unlock" }))?;
            let bytes = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
            LocalFree(Some(HLOCAL(out.pbData.cast())));
            Ok(bytes)
        }
    }

    /// The key from its file, where it is kept locked by the user's login.
    /// One left there in the open by an earlier Neo is locked when found.
    pub fn read() -> Result<Option<[u8; 32]>, String> {
        let Ok(text) = std::fs::read_to_string(file::path()) else { return Ok(None) };
        let Some(locked) = text.trim().strip_prefix(LOCKED) else {
            let key = unhex(&text);
            if let Some(key) = &key {
                let _ = write(key);
            }
            return Ok(key);
        };
        if locked.len() % 2 != 0 || !locked.is_ascii() {
            return Ok(None);
        }
        let Some(bytes) = (0..locked.len() / 2).map(|i| u8::from_str_radix(&locked[i * 2..i * 2 + 2], 16).ok()).collect::<Option<Vec<u8>>>() else { return Ok(None) };
        Ok(protect(&bytes, false)?.try_into().ok())
    }

    pub fn write(key: &[u8; 32]) -> Result<(), String> {
        let locked: String = protect(key, true)?.iter().map(|b| format!("{b:02x}")).collect();
        let path = file::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(path, format!("{LOCKED}{locked}")).map_err(|e| e.to_string())
    }

    /// Windows Hello: a face, a finger or the PIN, asked for by Windows
    /// itself over the window in front, which is Apollo's.
    pub fn authenticate(reason: &str) -> Result<(), String> {
        let asked = || -> windows::core::Result<Result<(), String>> {
            // SAFETY: plain calls into Windows, on a thread it has been told of.
            unsafe {
                // More than once on a thread does no harm.
                let _ = RoInitialize(RO_INIT_MULTITHREADED);
                if UserConsentVerifier::CheckAvailabilityAsync()?.join()? != UserConsentVerifierAvailability::Available {
                    return Ok(Err("Windows Hello is not set up. Set a PIN in Windows Settings, under Accounts and Sign-in options, and Apollo can check who you are.".into()));
                }
                let interop = factory::<UserConsentVerifier, IUserConsentVerifierInterop>()?;
                let answer: IAsyncOperation<UserConsentVerificationResult> = interop.RequestVerificationForWindowAsync(GetForegroundWindow(), &HSTRING::from(reason))?;
                Ok(if answer.join()? == UserConsentVerificationResult::Verified { Ok(()) } else { Err("Not unlocked.".into()) })
            }
        };
        asked().unwrap_or_else(|e| Err(format!("Couldn't ask who you are: {e}")))
    }

    #[test]
    fn what_the_login_locks_it_unlocks_and_nothing_else_does() {
        let key: [u8; 32] = std::array::from_fn(|i| (i * 5) as u8);
        let locked = protect(&key, true).unwrap();
        assert!(locked.len() > key.len() && !locked.windows(32).any(|w| w == key), "not the key in the open");
        assert_eq!(protect(&locked, false).unwrap(), key);
        assert!(protect(&key, false).is_err(), "what was never locked is not unlocked");
    }
}

#[cfg(not(any(unix, windows)))]
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
