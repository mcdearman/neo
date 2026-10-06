//! Which language server goes with which files, and where to find it.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// A language server NeoCode knows how to start.
pub struct ServerSpec {
    /// The server's name, as shown in the status bar.
    pub name: &'static str,
    /// What the language is called, for messages.
    pub language: &'static str,
    /// File extensions it handles, lower case, each with the language ID
    /// the protocol wants for it.
    pub extensions: &'static [(&'static str, &'static str)],
    /// Programs to look for, most preferred first, with their arguments.
    pub commands: &'static [(&'static str, &'static [&'static str])],
}

pub const SERVERS: &[ServerSpec] = &[
    ServerSpec { name: "rust-analyzer", language: "Rust", extensions: &[("rs", "rust")], commands: &[("rust-analyzer", &[])] },
    ServerSpec { name: "clangd", language: "C and C++", extensions: &[("c", "c"), ("h", "c"), ("cc", "cpp"), ("cpp", "cpp"), ("cxx", "cpp"), ("hpp", "cpp"), ("m", "objective-c"), ("mm", "objective-cpp")], commands: &[("clangd", &[])] },
    ServerSpec { name: "gopls", language: "Go", extensions: &[("go", "go")], commands: &[("gopls", &[])] },
    ServerSpec {
        name: "Python",
        language: "Python",
        extensions: &[("py", "python"), ("pyi", "python")],
        commands: &[("basedpyright-langserver", &["--stdio"]), ("pyright-langserver", &["--stdio"]), ("pylsp", &[]), ("ruff", &["server"])],
    },
    ServerSpec {
        name: "TypeScript",
        language: "TypeScript and JavaScript",
        extensions: &[("ts", "typescript"), ("tsx", "typescriptreact"), ("js", "javascript"), ("jsx", "javascriptreact"), ("mjs", "javascript"), ("cjs", "javascript")],
        commands: &[("typescript-language-server", &["--stdio"]), ("vtsls", &["--stdio"]), ("deno", &["lsp"])],
    },
    ServerSpec { name: "taplo", language: "TOML", extensions: &[("toml", "toml")], commands: &[("taplo", &["lsp", "stdio"])] },
    ServerSpec { name: "marksman", language: "Markdown", extensions: &[("md", "markdown"), ("markdown", "markdown")], commands: &[("marksman", &["server"])] },
    ServerSpec { name: "JSON", language: "JSON", extensions: &[("json", "json"), ("jsonc", "jsonc")], commands: &[("vscode-json-language-server", &["--stdio"])] },
    ServerSpec { name: "YAML", language: "YAML", extensions: &[("yaml", "yaml"), ("yml", "yaml")], commands: &[("yaml-language-server", &["--stdio"])] },
    ServerSpec { name: "HTML", language: "HTML", extensions: &[("html", "html"), ("htm", "html")], commands: &[("vscode-html-language-server", &["--stdio"])] },
    ServerSpec { name: "CSS", language: "CSS", extensions: &[("css", "css"), ("scss", "scss"), ("less", "less")], commands: &[("vscode-css-language-server", &["--stdio"])] },
    ServerSpec { name: "lua-language-server", language: "Lua", extensions: &[("lua", "lua")], commands: &[("lua-language-server", &[])] },
    ServerSpec { name: "zls", language: "Zig", extensions: &[("zig", "zig")], commands: &[("zls", &[])] },
    ServerSpec { name: "bash-language-server", language: "shell scripts", extensions: &[("sh", "shellscript"), ("bash", "shellscript"), ("zsh", "shellscript")], commands: &[("bash-language-server", &["start"])] },
];

/// The server for a file, and the language ID to open the file with.
pub fn spec_for(path: &Path) -> Option<(&'static ServerSpec, &'static str)> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    SERVERS.iter().find_map(|s| s.extensions.iter().find(|(e, _)| *e == ext).map(|(_, id)| (s, *id)))
}

/// A server program found on this computer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub program: PathBuf,
    pub args: Vec<String>,
}

/// Folders where language servers get installed, beyond those on the
/// `PATH`. An app started from the Dock or a launcher gets a bare `PATH`
/// that leaves most of these out.
fn usual_dirs(home: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = [".cargo/bin", ".local/bin", "go/bin", ".npm-global/bin", ".bun/bin", ".deno/bin", ".volta/bin", ".nix-profile/bin", ".local/share/mise/shims", ".asdf/shims", ".local/share/nvim/mason/bin"]
        .iter()
        .map(|d| home.join(d))
        .collect();
    dirs.extend(["/opt/homebrew/bin", "/opt/homebrew/opt/llvm/bin", "/usr/local/bin", "/usr/local/opt/llvm/bin", "/usr/bin"].map(PathBuf::from));
    dirs
}

/// The `PATH` the user's shell sets up when they log in, which is where
/// their own tools usually are. `None` if the shell does not answer soon.
fn login_shell_path() -> Option<String> {
    if cfg!(windows) {
        return None;
    }
    let shell = std::env::var_os("SHELL")?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let out = std::process::Command::new(shell).args(["-l", "-c", "printf '%s' \"$PATH\""]).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output();
        let _ = tx.send(out.ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).into_owned()));
    });
    rx.recv_timeout(Duration::from_secs(3)).ok().flatten().filter(|p| !p.is_empty())
}

/// Every folder to look for servers in, most trusted first: this
/// program's `PATH`, the login shell's, then the usual places.
pub fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if let Some(p) = login_shell_path() {
        dirs.extend(std::env::split_paths(&p));
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_default();
    dirs.extend(usual_dirs(&home));
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|d| !d.as_os_str().is_empty() && seen.insert(d.clone()));
    dirs
}

fn executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// Every copy of the server's programs in `dirs`, in the order to try them:
/// by how much the command is preferred, then by folder.
pub fn find_in(dirs: &[PathBuf], spec: &ServerSpec) -> Vec<Found> {
    let suffixes: &[&str] = if cfg!(windows) { &[".exe", ".cmd", ".bat", ""] } else { &[""] };
    let mut found = Vec::new();
    for (program, args) in spec.commands {
        for dir in dirs {
            for suffix in suffixes {
                let path = dir.join(format!("{program}{suffix}"));
                if executable(&path) {
                    found.push(Found { program: path, args: args.iter().map(|a| a.to_string()).collect() });
                    break;
                }
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("neo-code-lsp-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn program(dir: &Path, name: &str, runnable: bool) {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(if runnable { 0o755 } else { 0o644 })).unwrap();
        }
        let _ = runnable;
    }

    #[test]
    fn files_map_to_a_server_and_a_language_id() {
        let (spec, id) = spec_for(Path::new("src/main.rs")).unwrap();
        assert_eq!((spec.name, id), ("rust-analyzer", "rust"));
        assert_eq!(spec_for(Path::new("App.TSX")).map(|(s, id)| (s.name, id)), Some(("TypeScript", "typescriptreact")), "whatever the case");
        assert_eq!(spec_for(Path::new("Cargo.toml")).map(|(s, _)| s.name), Some("taplo"));
        assert!(spec_for(Path::new("notes.txt")).is_none());
        assert!(spec_for(Path::new("Makefile")).is_none());
    }

    #[test]
    fn no_extension_is_claimed_by_two_servers() {
        let mut seen = std::collections::HashSet::new();
        for s in SERVERS {
            assert!(!s.commands.is_empty(), "{} has nothing to run", s.name);
            for (ext, _) in s.extensions {
                assert!(seen.insert(*ext), "{ext} is listed twice");
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn servers_are_found_in_the_folders_searched() {
        let root = scratch("find");
        let (a, b) = (root.join("a"), root.join("b"));
        let python = SERVERS.iter().find(|s| s.name == "Python").unwrap();
        assert!(find_in(&[a.clone(), b.clone()], python).is_empty(), "nothing installed");

        program(&b, "pylsp", true);
        program(&a, "pyright-langserver", true);
        program(&b, "pyright-langserver", true);
        program(&a, "ruff", false);
        let found = find_in(&[a.clone(), b.clone()], python);
        let names: Vec<_> = found.iter().map(|f| f.program.strip_prefix(&root).unwrap().to_str().unwrap()).collect();
        assert_eq!(names, ["a/pyright-langserver", "b/pyright-langserver", "b/pylsp"], "the preferred command first, then by folder; a file that cannot be run is skipped");
        assert_eq!(found[0].args, ["--stdio"]);
        assert!(found[2].args.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_search_covers_the_path_and_the_usual_places_once_each() {
        let dirs = search_dirs();
        let home = PathBuf::from(std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).unwrap_or_default());
        assert!(dirs.contains(&home.join(".cargo/bin")), "where rustup puts rust-analyzer");
        for d in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).filter(|d| !d.as_os_str().is_empty()) {
            assert!(dirs.contains(&d));
        }
        let unique: std::collections::HashSet<_> = dirs.iter().collect();
        assert_eq!(unique.len(), dirs.len());
    }
}
