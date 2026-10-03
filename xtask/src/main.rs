//! Project tasks.
//!
//!     cargo xtask install      # build the apps and add them to this computer's app launcher
//!     cargo xtask uninstall    # remove them again
//!     cargo xtask icons        # redraw dist/icons
//!
//! Where things go:
//!
//! - macOS: `~/Applications/Neo Files.app` and so on, which Launchpad,
//!   Spotlight and the Dock pick up.
//! - Linux: binaries in `~/.local/bin`, menu entries in
//!   `~/.local/share/applications` and icons in `~/.local/share/icons`.
//!   Set `PREFIX` to install elsewhere, such as `/usr/local`.
//! - Windows: `%LOCALAPPDATA%\Programs\Neo`, with shortcuts in the
//!   Start menu's Neo folder.

mod apps;
mod icons;
mod platform;

use std::path::PathBuf;
use std::process::{Command, ExitCode};

use apps::APPS;

pub fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().expect("xtask lives in the workspace").to_path_buf()
}

fn target_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| root().join("target"))
}

/// Path of a release binary, with `.exe` on Windows.
pub fn built(bin: &str) -> PathBuf {
    target_dir().join("release").join(format!("{bin}{}", std::env::consts::EXE_SUFFIX))
}

fn build() -> Result<(), String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.arg("build").arg("--release").current_dir(root());
    for app in APPS {
        cmd.args(["-p", app.bin]);
    }
    let status = cmd.status().map_err(|e| format!("could not run cargo: {e}"))?;
    if status.success() { Ok(()) } else { Err("the build failed".into()) }
}

fn usage() -> ExitCode {
    eprintln!("usage: cargo xtask <install [--no-build] | uninstall | icons>");
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("install") => {
            let skip_build = args.iter().any(|a| a == "--no-build");
            (if skip_build { Ok(()) } else { build() }).and_then(|_| platform::install())
        }
        Some("uninstall") => platform::uninstall(),
        Some("icons") => icons::render_all(&root().join("dist/icons")),
        _ => return usage(),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
    }
}
