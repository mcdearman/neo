//! Keeping watch: what Apollo warns of.
//!
//! Two kinds of thing. How the computer is doing for memory and disk:
//! when memory runs short it says what is using it, since by the time
//! that is noticed otherwise the machine may be past helping. And what
//! stands open: a disk that is not encrypted, a firewall that is off, a
//! model server anyone on the network can use, questions that cross the
//! network unprotected, a key or a password lying in a file among the
//! documents.
//!
//! Everything here looks and reports; nothing is changed.

use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Something to tell the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Warning {
    /// What it is of, so that the same thing is not said over and over:
    /// "memory", "disk", "secret:/path/to/file".
    pub key: String,
    pub title: String,
    pub body: String,
    /// About what stands open, as against what is running out.
    pub security: bool,
    /// A file it is about, to show in Files.
    pub file: Option<PathBuf>,
}

/// A program and what it holds in memory, counting every process of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Using {
    pub name: String,
    pub processes: u32,
    pub bytes: u64,
}

/// Memory is short when less than this share of it is free, in percent.
const SHORT: u32 = 12;
/// One program holding more than this share of the memory there is, in
/// percent, is worth saying even before memory is short.
const GREEDY: u64 = 70;
/// The disk is nearly full with less than this free.
const DISK_LEAST: u64 = 5 * 1024 * 1024 * 1024;

/// Reads `ps -axo rss=,comm=`: what each program holds, the most first,
/// with all the processes of one program counted together.
pub fn parse_ps(text: &str) -> Vec<Using> {
    let mut out: Vec<Using> = vec![];
    for line in text.lines() {
        let line = line.trim_start();
        let Some((kb, command)) = line.split_once(char::is_whitespace) else { continue };
        let Ok(kb) = kb.parse::<u64>() else { continue };
        // By the program's own name, not the folder it is in.
        let name = command.trim().rsplit('/').next().unwrap_or(command).trim().to_owned();
        if name.is_empty() {
            continue;
        }
        match out.iter_mut().find(|u| u.name == name) {
            Some(u) => (u.processes, u.bytes) = (u.processes + 1, u.bytes + kb * 1024),
            None => out.push(Using { name, processes: 1, bytes: kb * 1024 }),
        }
    }
    out.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.name.cmp(&b.name)));
    out
}

/// Reads `df -k <path>`: the bytes free and the bytes there are in all.
pub fn parse_df(text: &str) -> Option<(u64, u64)> {
    let fields: Vec<&str> = text.lines().nth(1)?.split_whitespace().collect();
    let (total, free): (u64, u64) = (fields.get(1)?.parse().ok()?, fields.get(3)?.parse().ok()?);
    Some((free * 1024, total * 1024))
}

fn gigabytes(bytes: u64) -> String {
    let gb = bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    if gb >= 10.0 { format!("{gb:.0} GB") } else { format!("{gb:.1} GB") }
}

fn told(u: &Using) -> String {
    if u.processes > 1 { format!("{} is using {} across {} processes", u.name, gigabytes(u.bytes), u.processes) } else { format!("{} is using {}", u.name, gigabytes(u.bytes)) }
}

/// What to say of memory: `free` percent of it is free, of `total` bytes,
/// held as `using` says.
pub fn memory_warnings(free: u32, total: u64, using: &[Using]) -> Vec<Warning> {
    let mut out = vec![];
    let top = using.first();
    if free < SHORT {
        let who = top.map_or(String::new(), |u| format!("{}. ", told(u)));
        let next = using.get(1).map_or(String::new(), |u| format!("After it, {}. ", told(u)));
        out.push(Warning { key: "memory".into(), title: "Memory is running short".into(), body: format!("{who}{next}Only {free}% of memory is free; closing what is not needed keeps the computer from seizing up."), security: false, file: None });
    } else if let Some(u) = top.filter(|u| total > 0 && u.bytes * 100 / total > GREEDY) {
        out.push(Warning { key: format!("greedy:{}", u.name), title: format!("{} is using a great deal of memory", u.name), body: format!("{}, of the {} this computer has. Memory is not short yet.", told(u), gigabytes(total)), security: false, file: None });
    }
    out
}

/// What to say of the disk, with `free` bytes left of `total`.
pub fn disk_warnings(free: u64, total: u64) -> Vec<Warning> {
    if total > 0 && (free < DISK_LEAST || free * 100 / total < 3) {
        return vec![Warning { key: "disk".into(), title: "The disk is nearly full".into(), body: format!("{} is free of {}. NeoDisk shows what is taking the room.", gigabytes(free), gigabytes(total)), security: false, file: None }];
    }
    vec![]
}

fn said(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// How the computer is doing for memory and disk, now.
pub fn resources() -> Vec<Warning> {
    let mut out = vec![];
    if let Some(free) = crate::pressure::free_percent() {
        let using = if cfg!(unix) { said("ps", &["-axo", "rss=,comm="]).map(|t| parse_ps(&t)).unwrap_or_default() } else { vec![] };
        out.extend(memory_warnings(free, crate::pressure::total_bytes().unwrap_or(0), &using));
    }
    let home = neo_desktop::fs::home_dir();
    if let Some((free, total)) = cfg!(unix).then(|| said("df", &["-k", &home.to_string_lossy()])).flatten().and_then(|t| parse_df(&t)) {
        out.extend(disk_warnings(free, total));
    }
    out
}

/// Whether an address is of this computer itself.
fn is_here(address: &str) -> bool {
    let host = address.split_once("://").map_or(address, |(_, rest)| rest);
    let host = host.split(['/', ':']).next().unwrap_or(host);
    host == "localhost" || host == "127.0.0.1" || host == "::1" || host.starts_with("127.")
}

/// What to say of where Apollo's questions go: to `remote`, if any job is
/// done there.
pub fn remote_warnings(remote: &str, any_job_there: bool) -> Vec<Warning> {
    if any_job_there && remote.starts_with("http://") && !is_here(remote) {
        return vec![Warning { key: format!("remote:{remote}"), title: "Apollo's questions cross the network unprotected".into(), body: format!("What is asked of the model at {remote}, and the pictures it is shown, are sent as they are: anyone on the network between could read them. An https address, or a tunnel to that computer, would keep them private."), security: true, file: None }];
    }
    vec![]
}

/// Reads `lsof -nP -iTCP:<port> -sTCP:LISTEN`: whether something listens
/// there on every address, and so to the whole network.
pub fn listens_to_all(text: &str, port: u16) -> bool {
    text.lines().any(|l| l.contains(&format!("*:{port}")) || l.contains(&format!("0.0.0.0:{port}")) || l.contains(&format!("[::]:{port}")))
}

/// What stands open on this computer, as far as can be told without
/// being its administrator.
pub fn security(remote: &str, any_job_there: bool) -> Vec<Warning> {
    let mut out = remote_warnings(remote, any_job_there);
    if cfg!(unix) && said("lsof", &["-nP", "-iTCP:11434", "-sTCP:LISTEN"]).is_some_and(|t| listens_to_all(&t, 11434)) {
        out.push(Warning { key: "ollama-open".into(), title: "Apollo's model server is open to the network".into(), body: "Ollama is listening on every address, so any device on this network can use the models here and see what they are asked. Unless another computer is meant to use them, start it without OLLAMA_HOST set.".into(), security: true, file: None });
    }
    if cfg!(target_os = "macos") {
        if said("/usr/bin/fdesetup", &["status"]).is_some_and(|t| t.contains("FileVault is Off")) {
            out.push(Warning { key: "disk-open".into(), title: "The disk is not encrypted".into(), body: "FileVault is off, so whoever has this computer in their hands can read its files, and Apollo's memory is only as safe as the login keychain. It is turned on in System Settings, under Privacy & Security.".into(), security: true, file: None });
        }
        if said("/usr/libexec/ApplicationFirewall/socketfilterfw", &["--getglobalstate"]).is_some_and(|t| t.to_lowercase().contains("disabled")) {
            out.push(Warning { key: "firewall".into(), title: "The firewall is off".into(), body: "Programs on this computer can be reached from the network without being asked about. It is turned on in System Settings, under Network.".into(), security: true, file: None });
        }
    }
    out
}

/// What kinds of secret are known, by how they begin, with how many
/// letters and digits must follow for it to be one and not a word that
/// merely starts alike.
const SECRETS: &[(&str, usize, &str)] = &[("AKIA", 16, "an Amazon access key"), ("ghp_", 30, "a GitHub token"), ("github_pat_", 30, "a GitHub token"), ("sk-ant-", 30, "an Anthropic API key"), ("sk-proj-", 30, "an OpenAI API key"), ("xoxb-", 20, "a Slack token"), ("xoxp-", 20, "a Slack token"), ("AIza", 30, "a Google API key")];

/// Finds the first thing in a text that looks like a key or a token that
/// should not be lying about: where it is, how long, and what it is.
pub fn find_secret(text: &str) -> Option<(std::ops::Range<usize>, &'static str)> {
    // A private key, from its first line to its last.
    if let Some(from) = text.find("-----BEGIN ")
        && let Some(head) = text[from..].find("PRIVATE KEY-----").filter(|i| *i < 40)
    {
        let to = text[from..].find("-----END ").and_then(|end| text[from + end..].find('\n').map(|nl| from + end + nl)).unwrap_or(text.len());
        let _ = head;
        return Some((from..to, "a private key"));
    }
    let mut found: Option<(std::ops::Range<usize>, &'static str)> = None;
    for (start, least, what) in SECRETS {
        let mut at = 0;
        while let Some(i) = text[at..].find(start).map(|i| i + at) {
            let body = &text[i + start.len()..];
            let length = body.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-')).unwrap_or(body.len());
            // Not the tail of a longer word.
            let begins = text[..i].chars().next_back().is_none_or(|c| !c.is_ascii_alphanumeric());
            if begins && length >= *least && found.as_ref().is_none_or(|(r, _)| i < r.start) {
                found = Some((i..i + start.len() + length, what));
                break;
            }
            at = i + start.len();
        }
    }
    found
}

/// A text with whatever looks like a key or token taken out, and what the
/// first of them was. The memory keeps what a document says, not the
/// keys that happen to be in it.
pub fn without_secrets(text: &str) -> (String, Option<&'static str>) {
    let (mut out, mut rest, mut first) = (String::new(), text, None);
    while let Some((range, what)) = find_secret(rest) {
        out.push_str(&rest[..range.start]);
        out.push_str(&format!("[{what}, left out]"));
        first.get_or_insert(what);
        rest = &rest[range.end..];
    }
    out.push_str(rest);
    (out, first)
}

/// What to say of a file that holds a secret.
pub fn secret_warning(file: &std::path::Path, what: &str) -> Warning {
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Warning { key: format!("secret:{}", file.display()), title: format!("{name} holds what looks like {what}"), body: "It is in a folder Apollo reads, as plain text. Apollo has left it out of its memory; anything else that reads the folder, or a backup of it, has not. A password manager is a better place for it.".into(), security: true, file: Some(file.to_path_buf()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn what_programs_hold_is_added_up_by_program() {
        let ps = "  9437184 /Users/x/meadow/target/release/MeadowBoot\n 8388608 /Users/x/meadow/target/release/MeadowBoot\n   3774873 /opt/homebrew/Cellar/ollama/0.40.1/libexec/lib/ollama/llama-server\n  512000 /Applications/Firefox.app/Contents/MacOS/firefox\n not a line\n\n 1024 kernel_task\n";
        let using = parse_ps(ps);
        assert_eq!(using[0], Using { name: "MeadowBoot".into(), processes: 2, bytes: (9437184 + 8388608) * 1024 });
        assert_eq!(using.iter().map(|u| u.name.as_str()).collect::<Vec<_>>(), ["MeadowBoot", "llama-server", "firefox", "kernel_task"]);
        assert_eq!(parse_ps(""), vec![]);
    }

    #[test]
    fn short_memory_is_said_with_what_is_using_it() {
        let using = vec![Using { name: "MeadowBoot".into(), processes: 8, bytes: 73 * GB }, Using { name: "llama-server".into(), processes: 1, bytes: 2 * GB + GB / 5 }];
        let w = memory_warnings(4, 16 * GB, &using);
        assert_eq!((w.len(), w[0].key.as_str(), w[0].title.as_str(), w[0].security), (1, "memory", "Memory is running short", false));
        assert_eq!(w[0].body, "MeadowBoot is using 73 GB across 8 processes. After it, llama-server is using 2.2 GB. Only 4% of memory is free; closing what is not needed keeps the computer from seizing up.");
        // Not short yet, but one program has most of it: said, once for that program.
        let w = memory_warnings(30, 16 * GB, &[Using { name: "Chrome".into(), processes: 40, bytes: 12 * GB }]);
        assert_eq!((w[0].key.as_str(), w[0].title.as_str()), ("greedy:Chrome", "Chrome is using a great deal of memory"));
        // All well: nothing.
        assert_eq!(memory_warnings(40, 16 * GB, &[Using { name: "Firefox".into(), processes: 9, bytes: 3 * GB }]), vec![]);
        assert_eq!(memory_warnings(40, 0, &using), vec![], "with no total to compare with, nothing is made of it");
        assert_eq!(memory_warnings(5, 16 * GB, &[])[0].body, "Only 5% of memory is free; closing what is not needed keeps the computer from seizing up.");
    }

    #[test]
    fn a_nearly_full_disk_is_said() {
        let df = "Filesystem   1024-blocks      Used Available Capacity iused ifree %iused  Mounted on\n/dev/disk3s5   482797652 470000000   3145728    99% 2000000 31457280    6%   /System/Volumes/Data\n";
        assert_eq!(parse_df(df), Some((3145728 * 1024, 482797652 * 1024)));
        assert_eq!((parse_df(""), parse_df("Filesystem\nnonsense here")), (None, None));
        let w = disk_warnings(3 * GB, 460 * GB);
        assert_eq!((w[0].key.as_str(), w[0].body.as_str()), ("disk", "3.0 GB is free of 460 GB. NeoDisk shows what is taking the room."));
        assert_eq!(disk_warnings(100 * GB, 460 * GB), vec![]);
        assert_eq!(disk_warnings(8 * GB, 460 * GB).len(), 1, "under 3% free, though more than a few gigabytes");
        assert_eq!(disk_warnings(0, 0), vec![]);
    }

    #[test]
    fn what_stands_open_is_told_apart_from_what_does_not() {
        // Questions sent in the clear to another computer, when a job is done there.
        let w = remote_warnings("http://studio.local:11434", true);
        assert!(w.len() == 1 && w[0].security && w[0].key == "remote:http://studio.local:11434" && w[0].body.contains("studio.local"));
        assert_eq!(remote_warnings("http://studio.local:11434", false), vec![], "named but not used");
        assert_eq!(remote_warnings("https://models.example.com", true), vec![]);
        assert_eq!(remote_warnings("http://127.0.0.1:11500", true), vec![], "this computer itself");
        assert_eq!(remote_warnings("http://localhost:11500", true), vec![]);
        assert_eq!(remote_warnings("", true), vec![]);
        // A server listening to everyone, as against to this computer only.
        assert!(listens_to_all("COMMAND PID USER FD TYPE DEVICE SIZE/OFF NODE NAME\nollama 123 chris 3u IPv6 0x1 0t0 TCP *:11434 (LISTEN)\n", 11434));
        assert!(!listens_to_all("COMMAND PID USER FD TYPE DEVICE SIZE/OFF NODE NAME\nollama 123 chris 3u IPv4 0x1 0t0 TCP 127.0.0.1:11434 (LISTEN)\n", 11434));
        assert!(!listens_to_all("", 11434));
    }

    #[test]
    fn keys_lying_in_a_text_are_found_and_left_out() {
        let key = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\nAAAAB3NzaC1yc2E\n-----END OPENSSH PRIVATE KEY-----\n";
        let text = format!("Server notes.\n\n{key}\nThe port is 2222.");
        let (clean, what) = without_secrets(&text);
        assert_eq!((clean.as_str(), what), ("Server notes.\n\n[a private key, left out]\n\nThe port is 2222.", Some("a private key")));
        let (clean, what) = without_secrets("aws key AKIAIOSFODNN7EXAMPLE and token ghp_abcdefghijklmnopqrstuvwxyz0123456789 here");
        assert_eq!((clean.as_str(), what), ("aws key [an Amazon access key, left out] and token [a GitHub token, left out] here", Some("an Amazon access key")));
        assert_eq!(find_secret("export ANTHROPIC_API_KEY=sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789ABCD").map(|(_, what)| what), Some("an Anthropic API key"));
        // Words that only begin alike, and ordinary text, are let be.
        for plain in ["AKIA is how Amazon's keys begin.", "The task-ant-farm project", "xoxb- and nothing after", "MAKIAIOSFODNN7EXAMPLE1", "Ordinary notes about a trip.", ""] {
            assert_eq!(without_secrets(plain), (plain.to_owned(), None), "{plain}");
        }
        let w = secret_warning(std::path::Path::new("/Users/x/Documents/server notes.txt"), "a private key");
        assert_eq!((w.title.as_str(), w.key.as_str(), w.file.as_deref(), w.security), ("server notes.txt holds what looks like a private key", "secret:/Users/x/Documents/server notes.txt", Some(std::path::Path::new("/Users/x/Documents/server notes.txt")), true));
    }

    #[cfg(unix)]
    #[test]
    fn this_computer_can_be_looked_over() {
        // Whatever it finds, it finds without failing, and says who is using memory.
        let _ = resources();
        let using = parse_ps(&said("ps", &["-axo", "rss=,comm="]).unwrap());
        assert!(using.len() > 3 && using[0].bytes >= using[1].bytes);
        assert!(parse_df(&said("df", &["-k", "/"]).unwrap()).is_some_and(|(free, total)| free <= total && total > 0));
    }
}

#[cfg(test)]
mod probe {
    /// By hand: what would be warned of on this computer now.
    /// `cargo test -p neo-apollo-core what_this_computer -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn what_this_computer_would_be_warned_of() {
        for w in super::resources().into_iter().chain(super::security("", false)) {
            println!("[{}] {} :: {}", if w.security { "security" } else { "resources" }, w.title, w.body);
        }
        println!("free {:?}%, of {:?} bytes", crate::pressure::free_percent(), crate::pressure::total_bytes());
    }
}
