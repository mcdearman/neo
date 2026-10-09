//! Finding out about a process: where it came from, and what it is busy with.
//!
//! Apollo is asked what a process is, and the model that answers is a
//! small one that knows little of any one program. So what can be found
//! out is found out here, from the system, and sent along: where the
//! program is and whose it is, who signed it, what started it, the files
//! it has open and the code it is spending its time in. The answer is
//! then a matter of saying what these show, and they are shown with it.
//!
//! On macOS this asks `codesign`, `launchctl`, `lsof` and `sample`. A
//! process that is another user's, or the system's, may refuse the last
//! two, and is then described by what the rest say.

use std::process::Command;

/// Where a program comes from, going by where it is kept.
pub fn origin(path: &str) -> Option<String> {
    let home = neo_desktop::fs::home_dir();
    let home = home.to_string_lossy();
    // Inside an app, it is that app's: the outermost one, as helpers nest.
    if let Some(at) = path.find(".app/") {
        let app = path[..at].rsplit('/').next().unwrap_or_default();
        let whose = if path.starts_with("/System/") { "an app that comes with macOS" } else { "an app" };
        return Some(format!("It is part of {app}, {whose}."));
    }
    let known: [(&[&str], &str); 5] = [
        (&["/System/", "/usr/libexec/", "/usr/sbin/", "/usr/bin/", "/sbin/", "/bin/", "/Library/Apple/"], "It is part of macOS itself."),
        (&["/opt/homebrew/", "/usr/local/Cellar/", "/usr/local/opt/", "/home/linuxbrew/"], "It was installed with Homebrew."),
        (&["/nix/"], "It was installed with Nix."),
        (&["/Library/"], "It was installed for every user of this computer, by an app or an installer."),
        (&["/usr/local/"], "It was installed by hand or by an installer, outside the system."),
    ];
    if let Some((_, said)) = known.iter().find(|(under, _)| under.iter().any(|u| path.starts_with(u))) {
        return Some((*said).to_owned());
    }
    if !home.is_empty() && path.starts_with(&*home) {
        let inside = path[home.len()..].trim_start_matches('/');
        let said = match inside.split('/').next().unwrap_or_default() {
            ".cargo" | ".rustup" => "It is part of the Rust tools in your home folder.",
            ".meadow" => "It is part of the Meadow tools in your home folder.",
            "Applications" => "It is an app installed for you alone.",
            "Library" => "It was put in your Library folder by an app.",
            _ => "It is a program in your home folder, built or downloaded by you.",
        };
        return Some(said.to_owned());
    }
    None
}

/// Who signed a program, from what `codesign -dv --verbose=2` says of it:
/// the name it is signed under, and by whom.
pub fn signer(codesign: &str) -> Option<String> {
    let said = |key: &str| codesign.lines().find_map(|l| l.strip_prefix(key)).map(str::trim).filter(|v| !v.is_empty());
    let id = said("Identifier=");
    let by = match (said("Authority="), said("TeamIdentifier=")) {
        (Some("Software Signing"), _) => Some("Apple".to_owned()),
        (Some(who), _) => Some(who.strip_prefix("Developer ID Application: ").or_else(|| who.strip_prefix("Apple Development: ")).unwrap_or(who).to_owned()),
        // Signed on this computer, by whoever built it, and by nobody known.
        (None, _) if codesign.contains("Signature=adhoc") => Some("no developer: it was built on this computer, or by a package manager".to_owned()),
        (None, _) => None,
    };
    match (id, by) {
        (Some(id), Some(by)) => Some(format!("It is signed as {id}, by {by}.")),
        (None, Some(by)) => Some(format!("It is signed by {by}.")),
        (Some(id), None) => Some(format!("It is signed as {id}.")),
        (None, None) => None,
    }
}

/// The name launchd runs process `pid` under, from what `launchctl list` says.
pub fn service(launchctl: &str, pid: u32) -> Option<String> {
    launchctl.lines().find_map(|l| {
        let mut parts = l.split_whitespace();
        (parts.next()?.parse::<u32>().ok()? == pid).then_some(())?;
        parts.nth(1).map(str::to_owned)
    })
}

/// What some of the system's libraries are for, so that time spent in one
/// says what kind of work is being done.
const WORK: [(&[&str], &str); 14] = [
    (&["libBNNS", "Espresso", "CoreML", "MLCompute", "ANECompiler", "MPSNeuralNetwork", "libllama", "libggml", "onnxruntime"], "running a machine-learning model"),
    (&["Metal", "AGX", "MetalPerformanceShaders", "libGPUSupport", "OpenGL", "wgpu", "Vulkan"], "drawing or computing on the graphics processor"),
    (&["ImageIO", "CoreImage", "libJPEG", "libPng", "AppleJPEG", "libTIFF", "vImage", "CoreGraphics"], "decoding or processing pictures"),
    (&["VideoToolbox", "AVFCore", "AVFoundation", "CoreMedia", "libavcodec", "MediaToolbox"], "decoding or encoding video"),
    (&["libsqlite3", "CoreData"], "reading or writing a database"),
    (&["JavaScriptCore", "libv8", "WebCore", "WebKit"], "running a web page or its JavaScript"),
    (&["libcompression", "libz", "libbz2", "liblzma", "libzstd", "AppleArchive"], "compressing or unpacking data"),
    (&["CoreSpotlight", "SpotlightIndex", "mdworker", "SearchKit", "Metadata"], "indexing files for search"),
    (&["Security", "libcommonCrypto", "corecrypto", "libcrypto", "libssl"], "encrypting, decrypting or checking signatures"),
    (&["CFNetwork", "libnetwork", "Network"], "talking over the network"),
    (&["CoreAudio", "AudioToolbox", "libAudioToolboxUtility"], "playing or processing sound"),
    (&["CoreText", "libFontParser", "HIToolbox", "AppKit", "SwiftUI", "QuartzCore"], "laying out and drawing its windows"),
    (&["libclang", "libLLVM", "librustc", "libswiftCore", "rustc_driver"], "compiling code"),
    (&["libsystem_malloc", "libgmalloc"], "allocating and freeing memory"),
];

/// Where a process is spending its time, from what `sample` says: the
/// libraries its running code is in, busiest first, with what each is for
/// where that is known. Time spent waiting is not time spent.
pub fn busy_in(sample: &str) -> Vec<String> {
    const WAITING: [&str; 12] = ["__workq_kernreturn", "mach_msg2_trap", "mach_msg_trap", "__psynch_cvwait", "__psynch_mutexwait", "kevent", "__ulock_wait", "semaphore_wait", "__semwait_signal", "__select", "start_wqthread", "__sigsuspend"];
    let Some(at) = sample.find("Sort by top of stack") else { return vec![] };
    let mut by_library: Vec<(String, u64)> = vec![];
    for line in sample[at..].lines().skip(1).take_while(|l| !l.trim().is_empty()) {
        let line = line.trim();
        let Some(count) = line.rsplit(char::is_whitespace).next().and_then(|n| n.parse::<u64>().ok()) else { continue };
        // In the kernel's own library a thread is nearly always waiting to be woken.
        if WAITING.iter().any(|w| line.starts_with(w)) || line.contains("(in libsystem_kernel.dylib)") {
            continue;
        }
        let Some(library) = line.split_once("(in ").and_then(|(_, rest)| rest.split_once(')')).map(|(l, _)| l.trim().to_owned()) else { continue };
        match by_library.iter_mut().find(|(l, _)| *l == library) {
            Some((_, n)) => *n += count,
            None => by_library.push((library, count)),
        }
    }
    by_library.sort_by_key(|l| std::cmp::Reverse(l.1));
    let total: u64 = by_library.iter().map(|(_, n)| n).sum();
    by_library
        .into_iter()
        .take(4)
        .filter(|(_, n)| total > 0 && n * 20 >= total)
        .map(|(library, n)| {
            let share = (n * 100).div_ceil(total);
            let name = library.trim_end_matches(".dylib");
            match WORK.iter().find(|(names, _)| names.iter().any(|w| name == *w || name.starts_with(&format!("{w}.")) || name.starts_with(&format!("{w}_")))) {
                Some((_, what)) => format!("{share}% in {library}: {what}"),
                None => format!("{share}% in {library}"),
            }
        })
        .collect()
}

/// The files a process has open that say what it is working on, from
/// what `lsof -Fn` says: its own, not the system's libraries and devices.
pub fn working_on(lsof: &str) -> Vec<String> {
    const DULL: [&str; 9] = ["/System/", "/usr/", "/dev", "/Library/Apple/", "/private/var/db/", "/private/var/folders/", "/Library/Preferences/Logging/", "/Applications/", "/opt/homebrew/"];
    let mut seen: Vec<String> = vec![];
    for path in lsof.lines().filter_map(|l| l.strip_prefix("n/")).map(|p| format!("/{p}")) {
        // A database's side files are the database.
        let path = path.trim_end_matches("-wal").trim_end_matches("-shm").to_owned();
        let dull = path == "/" || DULL.iter().any(|d| path.starts_with(d)) || path.ends_with(".dylib") || path.contains(".app/Contents/");
        // A folder that a file already listed is in adds nothing.
        if !dull && !seen.contains(&path) {
            seen.retain(|s| !path.starts_with(&format!("{s}/")));
            if !seen.iter().any(|s| s.starts_with(&format!("{path}/"))) {
                seen.push(path);
            }
        }
    }
    // Cache files in their hundreds say one thing between them.
    let mut out: Vec<String> = vec![];
    for path in seen {
        let shown = match path.find("/Caches/") {
            Some(at) => format!("{} (cache files)", &path[..at + 7]),
            None => path,
        };
        if !out.contains(&shown) {
            out.push(shown);
        }
    }
    out.truncate(8);
    let home = neo_desktop::fs::home_dir();
    out.into_iter().map(|p| p.strip_prefix(&*home.to_string_lossy()).filter(|rest| rest.starts_with('/') && !home.as_os_str().is_empty()).map_or(p.clone(), |rest| format!("~{rest}"))).collect()
}

fn said(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
    // `codesign` says what it has to say on the other stream.
    let text = if out.stdout.is_empty() { out.stderr } else { out.stdout };
    Some(String::from_utf8_lossy(&text).into_owned()).filter(|t| !t.trim().is_empty())
}

/// What was asked of one process, to find out about it.
pub struct Subject<'a> {
    pub about: &'a crate::ask::About,
    /// What started it, and what started that: names, nearest first.
    pub parents: Vec<String>,
    /// How it was started: its command line.
    pub command: Vec<String>,
    /// How long it has been running, in seconds.
    pub running: u64,
    pub threads: Option<u32>,
}

fn ago(seconds: u64) -> String {
    match seconds {
        0..120 => "under two minutes".into(),
        120..7200 => format!("{} minutes", seconds / 60),
        7200..172_800 => format!("{} hours", seconds / 3600),
        _ => format!("{} days", seconds / 86_400),
    }
}

/// Everything found out about a process, a fact to a line. `closely`
/// also looks at what it is doing just now, which takes a second or two.
pub fn gather(s: &Subject, closely: bool) -> Vec<String> {
    let a = s.about;
    let mut facts = vec![format!("{} (PID {}) is run by {}, has been running for {}, and is using {:.1}% of one processor core and {} of memory{}.", a.name, a.pid, if a.user.is_empty() { "an unknown user" } else { &a.user }, ago(s.running), a.cpu, neo_desktop::fs::human_bytes_binary(a.memory), s.threads.map_or(String::new(), |n| format!(", in {n} threads")))];
    if let Some(path) = a.program.as_deref().filter(|p| !p.is_empty()) {
        facts.push(format!("Its program is {path}."));
        facts.extend(origin(path));
        if cfg!(target_os = "macos") {
            facts.extend(said("/usr/bin/codesign", &["-dv", "--verbose=2", path]).as_deref().and_then(signer));
        }
    }
    let arguments: Vec<&str> = s.command.iter().skip(1).map(String::as_str).collect();
    if !arguments.is_empty() {
        let line = arguments.join(" ");
        facts.push(format!("It was started with: {}", if line.chars().count() > 240 { format!("{}…", line.chars().take(240).collect::<String>()) } else { line }));
    }
    let service = if cfg!(target_os = "macos") { said("/bin/launchctl", &["list"]).and_then(|l| service(&l, a.pid)) } else { None };
    match (&service, s.parents.first().map(String::as_str)) {
        // An app opened in the ordinary way is known to launchd too, and is not a service.
        (Some(label), _) if label.starts_with("application.") => facts.push("It was opened as an app, by you or at login, and not from a terminal. Ended, it stays closed until it is opened again.".into()),
        (Some(label), _) => facts.push(format!("launchd, which starts the system's background services, runs it as {label}. Ended, it is started again when it is next wanted, so ending it stops what it is doing for now and no more.")),
        (None, Some("launchd" | "systemd")) => facts.push(format!("It was started by {}, the system's own starter of services, and not from an app or a terminal.", s.parents[0])),
        (None, Some(_)) => facts.push(format!("It was started by {}.", s.parents.iter().take(4).cloned().collect::<Vec<_>>().join(", which was started by "))),
        (None, None) => {}
    }
    if closely && cfg!(target_os = "macos") {
        let pid = a.pid.to_string();
        if let Some(files) = said("/usr/sbin/lsof", &["-p", &pid, "-Fn"]).map(|l| working_on(&l)).filter(|f| !f.is_empty()) {
            facts.push(format!("It has these open: {}.", files.join("; ")));
        }
        // A second's look at what its threads are running.
        let file = std::env::temp_dir().join(format!("neo-monitor-sample-{pid}.txt"));
        let _ = Command::new("/usr/bin/sample").args([&pid, "1", "-mayDie", "-file"]).arg(&file).output();
        let busy = std::fs::read_to_string(&file).map(|s| busy_in(&s)).unwrap_or_default();
        let _ = std::fs::remove_file(&file);
        if a.cpu < 2.0 {
            facts.push("Watched for a second, it was waiting, and doing next to nothing.".into());
        } else if !busy.is_empty() {
            facts.push(format!("Watched for a second, its processor time went: {}.", busy.join("; ")));
        }
    }
    facts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn where_a_program_is_kept_says_where_it_came_from() {
        let home = neo_desktop::fs::home_dir().to_string_lossy().into_owned();
        assert_eq!(origin("/System/Library/PrivateFrameworks/MediaAnalysis.framework/Versions/A/mediaanalysisd").as_deref(), Some("It is part of macOS itself."));
        assert_eq!(origin("/usr/libexec/trustd").as_deref(), Some("It is part of macOS itself."));
        assert_eq!(origin("/Applications/Safari.app/Contents/MacOS/Safari").as_deref(), Some("It is part of Safari, an app."));
        assert_eq!(origin("/System/Applications/Music.app/Contents/MacOS/Music").as_deref(), Some("It is part of Music, an app that comes with macOS."));
        assert_eq!(origin("/Applications/Slack.app/Contents/Frameworks/Slack Helper (GPU).app/Contents/MacOS/Slack Helper (GPU)").as_deref(), Some("It is part of Slack, an app."), "a helper inside an app is that app's");
        assert_eq!(origin("/opt/homebrew/Cellar/ollama/0.9/bin/ollama").as_deref(), Some("It was installed with Homebrew."));
        assert_eq!(origin(&format!("{home}/.cargo/bin/rust-analyzer")).as_deref(), Some("It is part of the Rust tools in your home folder."));
        assert_eq!(origin(&format!("{home}/Repositories/meadow/target/release/meadow")).as_deref(), Some("It is a program in your home folder, built or downloaded by you."));
        assert_eq!(origin(&format!("{home}/Applications/Apollo.app/Contents/MacOS/neo-apollo")).as_deref(), Some("It is part of Apollo, an app."));
        assert_eq!(origin("/somewhere/else/entirely"), None);
    }

    #[test]
    fn who_signed_a_program_and_what_runs_it() {
        let apple = "Executable=/System/Library/x\nIdentifier=com.apple.mediaanalysisd\nFormat=Mach-O universal\nAuthority=Software Signing\nAuthority=Apple Code Signing Certification Authority\nAuthority=Apple Root CA\n";
        assert_eq!(signer(apple).as_deref(), Some("It is signed as com.apple.mediaanalysisd, by Apple."));
        let other = "Identifier=com.tinyspeck.slackmacgap\nAuthority=Developer ID Application: Slack Technologies, Inc. (BQR82RBBHL)\nAuthority=Developer ID Certification Authority\nTeamIdentifier=BQR82RBBHL\n";
        assert_eq!(signer(other).as_deref(), Some("It is signed as com.tinyspeck.slackmacgap, by Slack Technologies, Inc. (BQR82RBBHL)."));
        assert_eq!(signer("Identifier=meadow-abc\nSignature=adhoc\nTeamIdentifier=not set\n").as_deref(), Some("It is signed as meadow-abc, by no developer: it was built on this computer, or by a package manager."));
        assert_eq!(signer("/tmp/x: code object is not signed at all\n"), None);
        let list = "PID\tStatus\tLabel\n-\t0\tcom.apple.SafariHistoryServiceAgent\n64740\t-9\tcom.apple.mediaanalysisd\n811\t0\tapplication.com.apple.Terminal.1152921500311879778\n";
        assert_eq!((service(list, 64740).as_deref(), service(list, 811).as_deref(), service(list, 7)), (Some("com.apple.mediaanalysisd"), Some("application.com.apple.Terminal.1152921500311879778"), None));
    }

    #[test]
    fn what_a_process_is_busy_with_is_read_from_a_sample_of_it() {
        let sample = "Call graph:\n    804 Thread_1\n\nTotal number in stack (recursive counted multiple, when >=5):\n\nSort by top of stack, same collapsed (when >= 5):\n        __workq_kernreturn  (in libsystem_kernel.dylib)        6772\n        mach_msg2_trap  (in libsystem_kernel.dylib)        804\n        ???  (in libBNNS.dylib)  load address 0x1a137a000 + 0x65d74c  [0x1a19d774c]        319\n        ???  (in libBNNS.dylib)  load address 0x1a137a000 + 0x65d76c  [0x1a19d776c]        122\n        sqlite3VdbeExec  (in libsqlite3.dylib) + 3004  [0x19d9b2c48]        49\n        _platform_memmove  (in libsystem_platform.dylib) + 96  [0x1]        20\n        tiny  (in libodd.dylib)        3\n        start_wqthread  (in libsystem_pthread.dylib)        30\n\nBinary Images:\n    0x100 - 0x200 +mediaanalysisd\n";
        assert_eq!(busy_in(sample), ["86% in libBNNS.dylib: running a machine-learning model", "10% in libsqlite3.dylib: reading or writing a database"], "waiting is left out, and so is what is too little to matter");
        // Nothing but waiting, or no sample at all: nothing to say.
        assert!(busy_in("Sort by top of stack, same collapsed (when >= 5):\n        mach_msg2_trap  (in libsystem_kernel.dylib)        804\n").is_empty());
        assert!(busy_in("sample cannot examine process 1 for unknown reasons").is_empty());
    }

    #[test]
    fn the_files_a_process_has_open_say_what_it_is_working_on() {
        let home = neo_desktop::fs::home_dir().to_string_lossy().into_owned();
        let lsof = format!("p64740\nfcwd\nn/\nftxt\nn/System/Library/PrivateFrameworks/MediaAnalysis.framework/Versions/A/mediaanalysisd\nn/usr/lib/dyld\nn/dev/null\nn{home}/Library/Containers/com.apple.mediaanalysisd/Data\nn{home}/Library/Containers/com.apple.mediaanalysisd/Data/Library/Caches/com.apple.mediaanalysisd/a/1\nn{home}/Library/Containers/com.apple.mediaanalysisd/Data/Library/Caches/com.apple.mediaanalysisd/a/2\nn{home}/Pictures/Photos Library.photoslibrary/database/Photos.sqlite\nn{home}/Pictures/Photos Library.photoslibrary/database/Photos.sqlite-shm\nn{home}/Pictures/Photos Library.photoslibrary/database/Photos.sqlite-wal\nn/Volumes/Backup/film.mov\nnnot a path\n");
        assert_eq!(working_on(&lsof), ["~/Library/Containers/com.apple.mediaanalysisd/Data/Library/Caches (cache files)", "~/Pictures/Photos Library.photoslibrary/database/Photos.sqlite", "/Volumes/Backup/film.mov"]);
        assert!(working_on("p1\nn/usr/lib/dyld\n").is_empty());
    }

    #[test]
    fn what_is_known_without_asking_the_system_is_set_out() {
        let about = crate::ask::About { name: "ollama".into(), pid: 4242, user: "chris".into(), program: None, cpu: 0.2, memory: 64 * 1024 * 1024 };
        let facts = gather(&Subject { about: &about, parents: vec!["zsh".into(), "Terminal".into(), "launchd".into()], command: vec!["ollama".into(), "serve".into()], running: 3 * 3600, threads: Some(12) }, false);
        assert_eq!(facts[0], "ollama (PID 4242) is run by chris, has been running for 3 hours, and is using 0.2% of one processor core and 64 MiB of memory, in 12 threads.");
        assert!(facts.contains(&"It was started with: serve".to_owned()), "{facts:?}");
        // On a Mac `launchctl` is asked too, and does not know a process there is none of.
        assert_eq!(facts.last().map(String::as_str), Some("It was started by zsh, which was started by Terminal, which was started by launchd."));
        assert_eq!((ago(30), ago(600), ago(86_400 * 3)), ("under two minutes".into(), "10 minutes".into(), "3 days".into()));
    }
}
