//! What Neo needs from outside itself, and getting it.
//!
//! Neo's apps are self-contained but for a few programs. `ffmpeg` reads
//! every kind of video there is: Files uses it for thumbnails of videos
//! the system cannot read, Apollo to look at videos, and on Linux and
//! Windows Videos plays with it and NeoCap records with it. `ollama` runs
//! Apollo's model on this computer, and `pdftotext` lets Apollo read
//! PDFs. A package of Neo should depend on them; an install from source
//! checks for them and gets them.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Something Neo runs that it does not build.
pub struct Dependency {
    /// The program's name.
    pub program: &'static str,
    /// What goes without it.
    pub why: &'static str,
    /// The package it comes in, where that is not called what it is: for
    /// Homebrew, for winget, and for a Linux package manager by name.
    /// `None` from the last where that manager has no package of it.
    pub brew: &'static str,
    pub winget: Option<&'static str>,
    pub linux: fn(&str) -> Option<&'static str>,
    /// Where Windows gets it when winget has no package of it: a zip of
    /// the program as its makers build it, to be unpacked beside the apps.
    pub windows_zip: Option<Zip>,
    /// And where a Windows computer with an NVIDIA graphics card gets it
    /// instead: a build that works on the card, which is several times
    /// quicker and several times the download.
    pub windows_zip_nvidia: Option<Zip>,
    /// macOS has something of its own for this, and does without.
    pub not_on_macos: bool,
}

/// A program to download for Windows, at a release that is known to work.
pub struct Zip {
    pub url: &'static str,
    /// The file's SHA-256, so that what is unpacked is what was meant.
    pub sha256: &'static str,
    /// The folder in the zip that holds the program and what it runs with.
    pub folder: &'static str,
    /// The folder it is unpacked into, under the apps' `tools`.
    pub into: &'static str,
}

pub const DEPENDENCIES: &[Dependency] = &[
    Dependency { program: "ffmpeg", why: "thumbnails of videos in Files, and Apollo's look at them; on Linux and Windows, playing in Videos and recording in NeoCap", brew: "ffmpeg", winget: Some("Gyan.FFmpeg"), linux: |_| Some("ffmpeg"), windows_zip: None, windows_zip_nvidia: None, not_on_macos: false },
    // Linux distributions do not package it; it has an installer of its own.
    Dependency { program: "ollama", why: "Apollo, whose model it runs on this computer (https://ollama.com/download)", brew: "ollama", winget: Some("Ollama.Ollama"), linux: |manager| (manager == "pacman").then_some("ollama"), windows_zip: None, windows_zip_nvidia: None, not_on_macos: false },
    // Packaged for Homebrew, and built for Windows by its makers; on Linux
    // it is built from whisper.cpp's source.
    Dependency {
        program: "whisper-cli",
        why: "Apollo's hearing of what is said in videos and recordings (https://github.com/ggml-org/whisper.cpp)",
        brew: "whisper-cpp",
        winget: None,
        linux: |_| None,
        windows_zip: Some(Zip { url: "https://github.com/ggml-org/whisper.cpp/releases/download/b5454/whisper-bin-x64.zip", sha256: "6ba69e3482d7826214f90a6a9c84ca07782aec1e1d0c6a7c30c994fd5d816ccb", folder: "Release", into: "whisper" }),
        // Built against CUDA 12, with the libraries it needs beside it: on an
        // RTX 5070 it heard three minutes of sound in 34 s where the other took 82.
        windows_zip_nvidia: Some(Zip { url: "https://github.com/ggml-org/whisper.cpp/releases/download/b5454/whisper-bin-win-cuda-12.4.0-x64.zip", sha256: "afef0b881c500958921c3f5523b50e59ee2ec9b6f5cbd25b324c51ed308a957a", folder: "Release", into: "whisper-cuda" }),
        not_on_macos: false,
    },
    // macOS reads word processors' files with a tool of its own.
    Dependency { program: "pandoc", why: "Apollo's reading of word processors' documents", brew: "pandoc", winget: Some("JohnMacFarlane.Pandoc"), linux: |_| Some("pandoc"), windows_zip: None, windows_zip_nvidia: None, not_on_macos: true },
    Dependency {
        program: "pdftotext",
        why: "Apollo's reading of PDFs",
        brew: "poppler",
        winget: Some("oschwartz10612.Poppler"),
        linux: |manager| {
            Some(match manager {
                "pacman" => "poppler",
                "zypper" => "poppler-tools",
                _ => "poppler-utils",
            })
        },
        windows_zip: None,
        windows_zip_nvidia: None,
        not_on_macos: false,
    },
];

/// Folders programs are installed in that an app started from a launcher
/// may not have on its `PATH`.
const USUAL: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin", "/snap/bin"];

/// Where the apps' own tools are kept on Windows: beside the apps, a folder to each.
pub fn tools_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_default().join(r"Programs\Neo\tools")
}

/// The folders Windows would give a program started now for its `PATH`.
/// This one's own is as old as the terminal it was started from, and knows
/// nothing of what winget installed a moment ago.
#[cfg(windows)]
fn fresh_path() -> Vec<PathBuf> {
    let mut dirs = vec![];
    for key in [r"HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment", r"HKCU\Environment"] {
        let Ok(out) = Command::new("reg").args(["query", key, "/v", "Path"]).output() else { continue };
        // "    Path    REG_EXPAND_SZ    C:\one;%USERPROFILE%\two"
        let text = String::from_utf8_lossy(&out.stdout);
        let Some(value) = text.lines().find_map(|l| l.split_once("_SZ").map(|(_, v)| v.trim().to_owned())) else { continue };
        for dir in value.split(';').filter(|d| !d.is_empty()) {
            // Names between percent signs stand for other variables.
            let mut whole = String::new();
            for (i, part) in dir.split('%').enumerate() {
                whole.push_str(&if i % 2 == 1 { std::env::var(part).unwrap_or_default() } else { part.to_owned() });
            }
            dirs.push(PathBuf::from(whole));
        }
    }
    dirs
}

#[cfg(not(windows))]
fn fresh_path() -> Vec<PathBuf> {
    vec![]
}

/// Where a program is, on the `PATH` or in the usual places.
pub fn find(program: &str) -> Option<PathBuf> {
    let name = format!("{program}{}", std::env::consts::EXE_SUFFIX);
    let on_path = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default();
    let ours = std::fs::read_dir(tools_dir()).into_iter().flatten().flatten().map(|e| e.path());
    on_path.into_iter().chain(USUAL.iter().map(PathBuf::from)).chain(fresh_path()).chain(ours).map(|dir| dir.join(&name)).find(|p| p.is_file())
}

/// The download of a dependency that suits this Windows computer, if it
/// is got by download at all: the NVIDIA one where there is such a card,
/// which is known by the tool its driver installs.
fn zip_for(dep: &Dependency) -> Option<&Zip> {
    if !cfg!(windows) || dep.winget.is_some() {
        return None;
    }
    let nvidia = std::env::var_os("SystemRoot").is_some_and(|root| PathBuf::from(root).join(r"System32\nvidia-smi.exe").is_file());
    dep.windows_zip_nvidia.as_ref().filter(|_| nvidia).or(dep.windows_zip.as_ref())
}

/// Downloads a program for Windows and unpacks it beside the apps.
fn fetch(zip: &Zip) -> Result<(), String> {
    let dir = tools_dir().join(zip.into);
    let file = std::env::temp_dir().join(format!("neo-{}.zip", zip.into));
    let run = |program: &str, args: &[&std::ffi::OsStr]| Command::new(program).args(args).output().map_err(|e| format!("could not run {program}: {e}"));
    // curl and tar come with Windows, and tar reads zips.
    let got = run("curl", &["-sSfL".as_ref(), "-o".as_ref(), file.as_os_str(), zip.url.as_ref()])?;
    if !got.status.success() {
        return Err(format!("{} could not be downloaded: {}", zip.url, String::from_utf8_lossy(&got.stderr).trim()));
    }
    let sum = run("certutil", &["-hashfile".as_ref(), file.as_os_str(), "SHA256".as_ref()])?;
    let sum = String::from_utf8_lossy(&sum.stdout).to_lowercase();
    if !sum.lines().any(|l| l.split_whitespace().collect::<String>() == zip.sha256) {
        let _ = std::fs::remove_file(&file);
        return Err(format!("{} is not the file that was expected, and was not unpacked", zip.url));
    }
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let unpacked = run("tar", &["-xf".as_ref(), file.as_os_str(), "-C".as_ref(), dir.as_os_str(), "--strip-components=1".as_ref(), zip.folder.as_ref()])?;
    let _ = std::fs::remove_file(&file);
    if !unpacked.status.success() {
        return Err(format!("{} could not be unpacked: {}", zip.url, String::from_utf8_lossy(&unpacked.stderr).trim()));
    }
    Ok(())
}

/// The command that installs a dependency with this system's own package
/// manager, if it has one that is known and that has it: the program to
/// run, then its arguments. Those that need the administrator's say-so
/// begin with `sudo`.
pub fn install_command(dep: &Dependency, have: &dyn Fn(&str) -> bool) -> Option<Vec<String>> {
    let words = |w: &[&str]| w.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    if cfg!(target_os = "macos") {
        // Homebrew, which does not want to be run as the administrator.
        return have("brew").then(|| words(&["brew", "install", dep.brew]));
    }
    if cfg!(windows) {
        return dep.winget.filter(|_| have("winget")).map(|id| words(&["winget", "install", "--id", id, "-e", "--accept-package-agreements", "--accept-source-agreements", "--disable-interactivity"]));
    }
    let managers: [(&str, &[&str]); 5] = [("apt-get", &["install", "-y"]), ("dnf", &["install", "-y"]), ("pacman", &["-S", "--noconfirm", "--needed"]), ("zypper", &["install", "-y"]), ("apk", &["add"])];
    let (manager, args) = managers.iter().find(|(manager, _)| have(manager))?;
    let package = (dep.linux)(manager)?;
    let mut command = words(&["sudo", manager]);
    command.extend(args.iter().map(|a| (*a).to_owned()));
    command.push(package.to_owned());
    Some(command)
}

/// Says what is here and what is not. With `install`, gets what is
/// missing. An error only if something is still missing afterwards and
/// was asked to be got.
pub fn ensure(install: bool) -> Result<(), String> {
    let mut missing = vec![];
    for dep in DEPENDENCIES.iter().filter(|d| !(d.not_on_macos && cfg!(target_os = "macos"))) {
        // One of ours that is not the build this computer should have counts as missing.
        let ours_but_another = |at: &PathBuf| zip_for(dep).is_some_and(|zip| at.starts_with(tools_dir()) && !at.starts_with(tools_dir().join(zip.into)));
        match find(dep.program).filter(|at| !ours_but_another(at)) {
            Some(at) => println!("found {} at {}", dep.program, at.display()),
            None => missing.push(dep),
        }
    }
    for dep in missing {
        // What winget does not have, Windows is given from where it is made.
        if let Some(zip) = zip_for(dep) {
            if !install {
                println!("missing {}: {}", dep.program, dep.why);
                println!("    to get it: cargo xtask deps --install    (from {})", zip.url);
                continue;
            }
            println!("installing {}: from {}", dep.program, zip.url);
            fetch(zip)?;
            // The other build of it, if it was here, would be found first.
            for other in [&dep.windows_zip, &dep.windows_zip_nvidia].into_iter().flatten().filter(|other| other.into != zip.into) {
                let _ = std::fs::remove_dir_all(tools_dir().join(other.into));
            }
            match find(dep.program) {
                Some(at) => println!("installed {} at {}", dep.program, at.display()),
                None => return Err(format!("{} was unpacked from {} but is not there", dep.program, zip.url)),
            }
            continue;
        }
        let command = install_command(dep, &|p| find(p).is_some());
        let how = command.as_ref().map(|c| c.join(" "));
        if !install {
            println!("missing {}: {}", dep.program, dep.why);
            match how {
                Some(how) => println!("    to get it: {how}    (or: cargo xtask deps --install)"),
                None => println!("    install it with your system's package manager"),
            }
            continue;
        }
        let Some(command) = command else {
            return Err(format!("{} is missing, and no package manager was found to install it with. It is needed for: {}", dep.program, dep.why));
        };
        println!("installing {}: {}", dep.program, command.join(" "));
        let program = find(&command[0]).unwrap_or_else(|| Path::new(&command[0]).to_path_buf());
        let status = Command::new(program).args(&command[1..]).status().map_err(|e| format!("could not run {}: {e}", command[0]))?;
        if !status.success() || find(dep.program).is_none() {
            return Err(format!("{} could not be installed with: {}", dep.program, command.join(" ")));
        }
        println!("installed {}", dep.program);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_is_this_systems_own() {
        let with = |present: &'static [&'static str]| move |p: &str| present.contains(&p);
        let dep = |program: &str| DEPENDENCIES.iter().find(|d| d.program == program).unwrap();
        let install_command = |program: &str, have: &dyn Fn(&str) -> bool| install_command(dep(program), have);
        if cfg!(target_os = "macos") {
            assert_eq!(install_command("ffmpeg", &with(&["brew"])), Some(vec!["brew".into(), "install".into(), "ffmpeg".into()]), "not as the administrator");
            assert_eq!(install_command("ffmpeg", &with(&[])), None, "with no Homebrew there is nothing to run");
            assert_eq!(install_command("pdftotext", &with(&["brew"])).map(|c| c.join(" ")), Some("brew install poppler".into()), "by the name of the package it comes in");
        } else if cfg!(windows) {
            assert!(install_command("ffmpeg", &with(&["winget"])).is_some_and(|c| c[0] == "winget"));
        } else {
            assert_eq!(install_command("ffmpeg", &with(&["apt-get"])), Some(vec!["sudo".into(), "apt-get".into(), "install".into(), "-y".into(), "ffmpeg".into()]));
            assert_eq!(install_command("ffmpeg", &with(&["pacman"])).map(|c| c.join(" ")), Some("sudo pacman -S --noconfirm --needed ffmpeg".into()));
            assert_eq!(install_command("ffmpeg", &with(&["dnf", "apt-get"])).map(|c| c[1].clone()), Some("apt-get".into()), "the first that is there");
            assert_eq!(install_command("ffmpeg", &with(&[])), None);
            assert_eq!(install_command("pdftotext", &with(&["apt-get"])).map(|c| c[4].clone()), Some("poppler-utils".into()), "by the name of the package it comes in");
            assert_eq!(install_command("ollama", &with(&["apt-get"])), None, "what a manager does not have is not asked of it");
        }
    }

    #[test]
    fn programs_are_found_where_they_are_and_not_where_they_are_not() {
        // Every system has a shell or a command interpreter on its path.
        assert!(find(if cfg!(windows) { "cmd" } else { "sh" }).is_some());
        assert!(find("neo-no-such-program-anywhere").is_none());
        assert!(DEPENDENCIES.iter().any(|d| d.program == "ffmpeg") && DEPENDENCIES.iter().all(|d| !d.why.is_empty()));
    }
}
