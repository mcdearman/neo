//! The shells on this computer that NeoTerm can run, and the one chosen.

use std::path::{Path, PathBuf};

use neo_desktop::config::File;

/// A shell to run: what it is called in the list, and how it is started.
#[derive(Clone, Debug, PartialEq)]
pub struct Kind {
    pub name: String,
    pub program: String,
    pub args: Vec<String>,
}

impl Kind {
    fn new(name: &str, program: &Path, args: &[&str]) -> Self {
        Self { name: name.to_owned(), program: program.to_string_lossy().into_owned(), args: args.iter().map(|a| (*a).to_owned()).collect() }
    }
}

/// The shell's icon, as the system has it: Git's from the program that
/// carries it, which its bash does not.
pub fn icon(kind: &Kind) -> Option<neo::Image> {
    let program = Path::new(&kind.program);
    let git = program.parent().and_then(Path::parent).map(|git| git.join("git-bash.exe")).filter(|p| kind.name == "Git Bash" && p.is_file());
    neo_desktop::icons::of_file(git.as_deref().unwrap_or(program), 36)
}

/// What the shell that runs when none has been chosen is called.
pub fn default_name() -> String {
    if cfg!(windows) {
        return "Windows PowerShell".into();
    }
    std::env::var_os("SHELL").map(PathBuf::from).and_then(|s| s.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_else(|| "Shell".into())
}

/// The shell chosen before, as NeoTerm's settings have it:
///
/// ```toml
/// [shell]
/// name = "Git Bash"
/// program = 'C:\Program Files\Git\bin\bash.exe'
/// args = ["--login", "-i"]
/// ```
pub fn chosen(file: &File) -> Option<Kind> {
    let program = file.text(&["shell", "program"])?.to_owned();
    let name = file.text(&["shell", "name"]).map(str::to_owned).unwrap_or_else(|| program.clone());
    Some(Kind { name, program, args: file.list(&["shell", "args"]).unwrap_or_default() })
}

/// Writes the choice down, to be the shell the next window starts with.
pub fn keep(kind: &Kind) -> std::io::Result<()> {
    let mut file = File::app("neo-terminal");
    file.set(&["shell", "name"], kind.name.as_str());
    file.set(&["shell", "program"], kind.program.as_str());
    file.set_list(&["shell", "args"], &kind.args);
    file.save()
}

/// A program by this name in one of the folders on the `PATH`.
fn on_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).map(|dir| dir.join(name)).find(|p| p.is_file())
}

/// The shells found on this computer. Asking Windows which Linux systems
/// it has can take a moment, so this is for a thread of its own.
#[cfg(windows)]
pub fn found() -> Vec<Kind> {
    let var = |name: &str| std::env::var_os(name).map(PathBuf::from);
    let system = var("SystemRoot").unwrap_or_else(|| PathBuf::from(r"C:\Windows")).join("System32");
    let mut kinds = Vec::new();
    if let Some(pwsh) = on_path("pwsh.exe") {
        kinds.push(Kind::new("PowerShell", &pwsh, &[]));
    }
    let powershell = system.join(r"WindowsPowerShell\v1.0\powershell.exe");
    if powershell.is_file() {
        kinds.push(Kind::new("Windows PowerShell", &powershell, &[]));
    }
    let cmd = var("COMSPEC").filter(|p| p.is_file()).unwrap_or_else(|| system.join("cmd.exe"));
    if cmd.is_file() {
        kinds.push(Kind::new("Command Prompt", &cmd, &[]));
    }
    // Git's own bash, which is not on the PATH: the one there is a helper.
    let git = ["ProgramFiles", "ProgramFiles(x86)"].iter().filter_map(|v| var(v)).map(|p| p.join("Git")).chain(var("LOCALAPPDATA").map(|p| p.join(r"Programs\Git"))).map(|git| git.join(r"bin\bash.exe")).find(|p| p.is_file());
    if let Some(bash) = git {
        kinds.push(Kind::new("Git Bash", &bash, &["--login", "-i"]));
    }
    let wsl = system.join("wsl.exe");
    for distribution in linux_systems(&wsl) {
        kinds.push(Kind::new(&format!("{distribution} (WSL)"), &wsl, &["--distribution", &distribution]));
    }
    kinds
}

/// The Linux systems Windows has, by name: none if it has no WSL.
#[cfg(windows)]
fn linux_systems(wsl: &Path) -> Vec<String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    if !wsl.is_file() {
        return vec![];
    }
    let Ok(out) = neo_desktop::command(wsl).args(["--list", "--quiet"]).creation_flags(CREATE_NO_WINDOW).stdin(std::process::Stdio::null()).output() else { return vec![] };
    if !out.status.success() {
        return vec![];
    }
    // It answers in UTF-16, a name to a line.
    let units: Vec<u16> = out.stdout.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect();
    String::from_utf16_lossy(&units).lines().map(|l| l.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}' || c == '\0').to_owned()).filter(|l| !l.is_empty()).collect()
}

/// The shells found on this computer: those `/etc/shells` lists, each
/// once however many places it is in.
#[cfg(not(windows))]
pub fn found() -> Vec<Kind> {
    let listed = std::fs::read_to_string("/etc/shells").unwrap_or_default();
    let own = std::env::var("SHELL").ok();
    let mut kinds: Vec<Kind> = Vec::new();
    for path in own.iter().map(String::as_str).chain(listed.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#'))) {
        let path = Path::new(path);
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
        if path.is_file() && !kinds.iter().any(|k| k.name == name) {
            // As a login shell on macOS, where that is what a terminal starts.
            kinds.push(Kind::new(&name, path, if cfg!(target_os = "macos") { &["-l"] } else { &[] }));
        }
    }
    kinds
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn there_is_a_shell_to_run_and_each_is_listed_once() {
        let kinds = found();
        assert!(!kinds.is_empty(), "every computer has a shell");
        assert!(kinds.iter().all(|k| Path::new(&k.program).is_file()), "{kinds:?}");
        let mut names: Vec<&str> = kinds.iter().map(|k| k.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), kinds.len(), "{kinds:?}");
        let _ = on_path("no-such-program");
    }

    #[test]
    fn the_choice_is_read_back_as_it_was_written() {
        let mut file = File::parse("");
        assert_eq!(chosen(&file), None, "nothing chosen is the user's own shell");
        file.set(&["shell", "name"], "Git Bash");
        file.set(&["shell", "program"], r"C:\Program Files\Git\bin\bash.exe");
        file.set_list(&["shell", "args"], &["--login".to_owned(), "-i".to_owned()]);
        let again = File::parse(&file.encode());
        assert_eq!(chosen(&again), Some(Kind { name: "Git Bash".into(), program: r"C:\Program Files\Git\bin\bash.exe".into(), args: vec!["--login".into(), "-i".into()] }));
    }
}
