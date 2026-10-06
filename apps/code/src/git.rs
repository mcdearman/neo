//! Source control: what has changed in the folder's git repository, and
//! staging, committing and syncing it. The work is git's own; this reads
//! what it reports and runs its commands.

use std::path::{Path, PathBuf};

/// One file that differs from the last commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// Its path within the repository.
    pub path: String,
    /// How it differs in the index, staged to be committed: `M`, `A`,
    /// `D`, `R`. `None` if nothing of it is staged.
    pub staged: Option<char>,
    /// How the working copy differs from the index: `M`, `D`, or `?` for
    /// a file git has not been told about.
    pub unstaged: Option<char>,
    /// For a rename, what it was called.
    pub from: Option<String>,
}

impl Change {
    pub fn name(&self) -> &str {
        self.path.trim_end_matches('/').rsplit('/').next().unwrap_or(&self.path)
    }

    /// The folder it is in, within the repository, for telling apart two
    /// files of the same name.
    pub fn folder(&self) -> &str {
        self.path.trim_end_matches('/').rsplit_once('/').map_or("", |(dir, _)| dir)
    }
}

/// The state of a repository's working copy.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub branch: String,
    /// The branch it follows on a remote, if it follows one.
    pub upstream: Option<String>,
    /// Commits here that the upstream lacks, and the other way about.
    pub ahead: u32,
    pub behind: u32,
    pub changes: Vec<Change>,
}

impl Status {
    pub fn staged(&self) -> impl Iterator<Item = &Change> {
        self.changes.iter().filter(|c| c.staged.is_some())
    }

    pub fn unstaged(&self) -> impl Iterator<Item = &Change> {
        self.changes.iter().filter(|c| c.unstaged.is_some())
    }
}

/// What `git status` is asked for: the branch as well as the files, each
/// entry ended by a zero so that any file name can be told from the next.
pub const STATUS: [&str; 5] = ["status", "--porcelain=v1", "--branch", "-z", "--untracked-files=all"];

/// Reads the output of `git status` asked for as [`STATUS`].
pub fn parse_status(text: &str) -> Status {
    let mut status = Status::default();
    let mut entries = text.split('\0').filter(|e| !e.is_empty());
    while let Some(entry) = entries.next() {
        if let Some(head) = entry.strip_prefix("## ") {
            // "main...origin/main [ahead 1, behind 2]", or "main", or
            // "No commits yet on main", or "HEAD (no branch)".
            let (names, counts) = head.split_once(" [").map_or((head, ""), |(n, c)| (n, c.trim_end_matches(']')));
            let (branch, upstream) = names.split_once("...").map_or((names, None), |(b, u)| (b, Some(u.to_owned())));
            status.branch = branch.strip_prefix("No commits yet on ").unwrap_or(branch).to_owned();
            status.upstream = upstream;
            for part in counts.split(", ") {
                if let Some(n) = part.strip_prefix("ahead ") {
                    status.ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = part.strip_prefix("behind ") {
                    status.behind = n.parse().unwrap_or(0);
                }
            }
            continue;
        }
        let mut letters = entry.chars();
        let (Some(x), Some(y), Some(path)) = (letters.next(), letters.next(), entry.get(3..)) else { continue };
        // A rename or copy is followed by the name it had.
        let from = (x == 'R' || x == 'C').then(|| entries.next().map(str::to_owned)).flatten();
        let change = match (x, y) {
            ('?', '?') => Change { path: path.to_owned(), staged: None, unstaged: Some('?'), from },
            ('!', _) => continue,
            _ => Change { path: path.to_owned(), staged: (x != ' ').then_some(x), unstaged: (y != ' ').then_some(y), from },
        };
        status.changes.push(change);
    }
    status
}

/// Runs git in `root` and returns what it printed, or what it complained of.
/// `dirs` become its `PATH`, since an app started from the Dock has a bare one.
pub fn run(root: &Path, dirs: &[PathBuf], args: &[&str]) -> Result<String, String> {
    let mut command = std::process::Command::new("git");
    if let Ok(path) = std::env::join_paths(dirs)
        && !path.is_empty()
    {
        command.env("PATH", path);
    }
    // Never stop to ask for a password or an editor: there is nowhere to answer.
    let out = command.args(args).current_dir(root).env("GIT_TERMINAL_PROMPT", "0").env("GIT_EDITOR", "true").stdin(std::process::Stdio::null()).output().map_err(|e| format!("Couldn't run git: {e}"))?;
    let text = |b: &[u8]| String::from_utf8_lossy(b).trim_end().to_owned();
    if out.status.success() { Ok(String::from_utf8_lossy(&out.stdout).into_owned()) } else { Err(Some(text(&out.stderr)).filter(|e| !e.is_empty()).unwrap_or_else(|| text(&out.stdout))) }
}

/// The state of the repository `root` is in. An error if it is in none.
pub fn status(root: &Path, dirs: &[PathBuf]) -> Result<Status, String> {
    run(root, dirs, &STATUS).map(|text| parse_status(&text))
}

/// The top of the repository `dir` is in, if it is in one.
pub fn top(dir: &Path, dirs: &[PathBuf]) -> Option<PathBuf> {
    run(dir, dirs, &["rev-parse", "--show-toplevel"]).ok().map(|t| PathBuf::from(t.trim())).filter(|p| p.is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_is_read_with_its_branch_and_every_kind_of_change() {
        let text = "## main...origin/main [ahead 2, behind 1]\0 M src/lib.rs\0M  src/staged.rs\0MM both.rs\0A  new file.txt\0 D gone.rs\0R  now.rs\0was.rs\0?? notes/todo.md\0!! target/\0";
        let s = parse_status(text);
        assert_eq!((s.branch.as_str(), s.upstream.as_deref(), s.ahead, s.behind), ("main", Some("origin/main"), 2, 1));
        let seen: Vec<(&str, Option<char>, Option<char>)> = s.changes.iter().map(|c| (c.path.as_str(), c.staged, c.unstaged)).collect();
        assert_eq!(
            seen,
            [("src/lib.rs", None, Some('M')), ("src/staged.rs", Some('M'), None), ("both.rs", Some('M'), Some('M')), ("new file.txt", Some('A'), None), ("gone.rs", None, Some('D')), ("now.rs", Some('R'), None), ("notes/todo.md", None, Some('?'))],
            "a file with a space in its name, one changed both ways, a rename, and nothing that is ignored"
        );
        assert_eq!(s.changes[5].from.as_deref(), Some("was.rs"));
        assert_eq!((s.staged().count(), s.unstaged().count()), (4, 4));
        assert_eq!((s.changes[6].name(), s.changes[6].folder(), s.changes[2].folder()), ("todo.md", "notes", ""));
    }

    #[test]
    fn branches_come_in_several_shapes() {
        assert_eq!(parse_status("## feature/x\0").branch, "feature/x");
        let fresh = parse_status("## No commits yet on main\0?? a\0");
        assert_eq!((fresh.branch.as_str(), fresh.upstream, fresh.changes.len()), ("main", None, 1));
        assert_eq!(parse_status("## HEAD (no branch)\0").branch, "HEAD (no branch)");
        let behind = parse_status("## main...origin/main [behind 3]\0");
        assert_eq!((behind.ahead, behind.behind), (0, 3));
        assert_eq!(parse_status(""), Status::default());
    }

    #[test]
    fn a_real_repository_is_read_staged_and_committed() {
        let dir = std::env::temp_dir().join(format!("neo-code-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Not a repository yet: said so, not read as an empty one.
        assert!(status(&dir, &[]).is_err());
        assert_eq!(top(&dir, &[]), None);
        let git = |args: &[&str]| run(&dir, &[], args).unwrap_or_else(|e| panic!("git {args:?}: {e}"));
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "user.name", "Test"]);
        git(&["config", "commit.gpgsign", "false"]);
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        std::fs::write(dir.join("b c.txt"), "two\n").unwrap();
        let s = status(&dir, &[]).unwrap();
        assert_eq!(s.branch, "main");
        assert_eq!(s.changes.iter().map(|c| (c.path.as_str(), c.unstaged)).collect::<Vec<_>>(), [("a.txt", Some('?')), ("b c.txt", Some('?'))]);
        assert_eq!(top(&dir, &[]).map(|p| p.canonicalize().unwrap()), Some(dir.canonicalize().unwrap()));
        git(&["add", "--", "a.txt"]);
        let s = status(&dir, &[]).unwrap();
        assert_eq!((s.staged().map(|c| c.path.as_str()).collect::<Vec<_>>(), s.unstaged().count()), (vec!["a.txt"], 1));
        git(&["commit", "-q", "-m", "first"]);
        std::fs::write(dir.join("a.txt"), "one\nmore\n").unwrap();
        let s = status(&dir, &[]).unwrap();
        assert_eq!(s.changes.iter().map(|c| (c.path.as_str(), c.staged, c.unstaged)).collect::<Vec<_>>(), [("a.txt", None, Some('M')), ("b c.txt", None, Some('?'))]);
        // What git complains of is passed on.
        let why = run(&dir, &[], &["commit", "-m", "nothing staged"]).unwrap_err();
        assert!(!why.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
