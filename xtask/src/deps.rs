//! What Neo needs from outside itself, and getting it.
//!
//! Neo's apps are self-contained but for one thing: `ffmpeg`, which reads
//! every kind of video there is. Files uses it for thumbnails of videos
//! the system cannot read, and on Linux and Windows Videos plays with it
//! and NeoCap records with it. A package of Neo should depend on it; an
//! install from source checks for it and gets it.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Something Neo runs that it does not build.
pub struct Dependency {
    /// The program's name.
    pub program: &'static str,
    /// What goes without it.
    pub why: &'static str,
}

pub const DEPENDENCIES: &[Dependency] = &[Dependency { program: "ffmpeg", why: "thumbnails of videos in Files; on Linux and Windows, playing in Videos and recording in NeoCap" }];

/// Folders programs are installed in that an app started from a launcher
/// may not have on its `PATH`.
const USUAL: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin", "/snap/bin"];

/// Where a program is, on the `PATH` or in the usual places.
pub fn find(program: &str) -> Option<PathBuf> {
    let name = format!("{program}{}", std::env::consts::EXE_SUFFIX);
    let on_path = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default();
    on_path.into_iter().chain(USUAL.iter().map(PathBuf::from)).map(|dir| dir.join(&name)).find(|p| p.is_file())
}

/// The command that installs `program` with this system's own package
/// manager, if it has one that is known: the program to run, then its
/// arguments. Those that need the administrator's say-so begin with `sudo`.
pub fn install_command(program: &str, have: &dyn Fn(&str) -> bool) -> Option<Vec<String>> {
    let words = |w: &[&str]| w.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    if cfg!(target_os = "macos") {
        // Homebrew, which does not want to be run as the administrator.
        return have("brew").then(|| words(&["brew", "install", program]));
    }
    if cfg!(windows) {
        return have("winget").then(|| words(&["winget", "install", "--id", "Gyan.FFmpeg", "-e"]));
    }
    let managers: [(&str, &[&str]); 5] = [("apt-get", &["install", "-y"]), ("dnf", &["install", "-y"]), ("pacman", &["-S", "--noconfirm", "--needed"]), ("zypper", &["install", "-y"]), ("apk", &["add"])];
    managers.iter().find(|(manager, _)| have(manager)).map(|(manager, args)| {
        let mut command = words(&["sudo", manager]);
        command.extend(args.iter().map(|a| (*a).to_owned()));
        command.push(program.to_owned());
        command
    })
}

/// Says what is here and what is not. With `install`, gets what is
/// missing. An error only if something is still missing afterwards and
/// was asked to be got.
pub fn ensure(install: bool) -> Result<(), String> {
    let mut missing = vec![];
    for dep in DEPENDENCIES {
        match find(dep.program) {
            Some(at) => println!("found {} at {}", dep.program, at.display()),
            None => missing.push(dep),
        }
    }
    for dep in missing {
        let command = install_command(dep.program, &|p| find(p).is_some());
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
        if cfg!(target_os = "macos") {
            assert_eq!(install_command("ffmpeg", &with(&["brew"])), Some(vec!["brew".into(), "install".into(), "ffmpeg".into()]), "not as the administrator");
            assert_eq!(install_command("ffmpeg", &with(&[])), None, "with no Homebrew there is nothing to run");
        } else if cfg!(windows) {
            assert!(install_command("ffmpeg", &with(&["winget"])).is_some_and(|c| c[0] == "winget"));
        } else {
            assert_eq!(install_command("ffmpeg", &with(&["apt-get"])), Some(vec!["sudo".into(), "apt-get".into(), "install".into(), "-y".into(), "ffmpeg".into()]));
            assert_eq!(install_command("ffmpeg", &with(&["pacman"])).map(|c| c.join(" ")), Some("sudo pacman -S --noconfirm --needed ffmpeg".into()));
            assert_eq!(install_command("ffmpeg", &with(&["dnf", "apt-get"])).map(|c| c[1].clone()), Some("apt-get".into()), "the first that is there");
            assert_eq!(install_command("ffmpeg", &with(&[])), None);
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
