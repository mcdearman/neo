//! Asking Apollo what a process is.
//!
//! The question is written here from what System Monitor knows of the
//! processes chosen: their names, where their programs are, who runs
//! them and what they are using. Apollo answers in its own window.

/// What is known of a process, for asking about it.
#[derive(Clone, Debug, PartialEq)]
pub struct About {
    pub name: String,
    pub pid: u32,
    pub user: String,
    /// Where its program is, if the system says.
    pub program: Option<String>,
    /// Percent of the whole machine.
    pub cpu: f32,
    pub memory: u64,
}

/// The most processes named in one question: more would be more than a
/// small model reads well, and more than anyone means to ask about.
pub const MOST: usize = 12;

fn described(p: &About) -> String {
    let program = p.program.as_deref().filter(|at| !at.is_empty()).map_or(String::new(), |at| format!(", the program at {at}"));
    format!("“{}” (PID {}{program}, run by {}, using {:.1}% of the CPU and {} of memory)", p.name, p.pid, if p.user.is_empty() { "an unknown user" } else { &p.user }, p.cpu, neo_desktop::fs::human_bytes_binary(p.memory))
}

/// The question to ask of these processes, or nothing if there are none.
/// Several of one name, as a browser's many helpers are, are asked about
/// once, with how many there are.
pub fn question(chosen: &[About]) -> Option<String> {
    match chosen {
        [] => None,
        [one] => Some(format!("What is the process {} on this computer? What does it do, is it part of the system or of an app, and is it safe to end it?", described(one))),
        many => {
            let mut kinds: Vec<(&About, usize)> = vec![];
            for p in many {
                match kinds.iter_mut().find(|(k, _)| k.name == p.name) {
                    Some((_, n)) => *n += 1,
                    None => kinds.push((p, 1)),
                }
            }
            let more = kinds.len().saturating_sub(MOST);
            let mut lines: Vec<String> = kinds.iter().take(MOST).map(|(p, n)| if *n > 1 { format!("- {}, one of {n} with that name", described(p)) } else { format!("- {}", described(p)) }).collect();
            if more > 0 {
                lines.push(format!("- and {more} more not named here"));
            }
            Some(format!("What are these processes on this computer? For each, say briefly what it does, whether it is part of the system or of an app, and whether it is safe to end.\n{}", lines.join("\n")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn about(name: &str, pid: u32) -> About {
        About { name: name.into(), pid, user: "chris".into(), program: Some(format!("/usr/libexec/{name}")), cpu: 12.34, memory: 300 * 1024 * 1024 }
    }

    #[test]
    fn one_process_is_asked_about_with_what_is_known_of_it() {
        assert_eq!(question(&[]), None);
        let q = question(&[about("mds_stores", 412)]).unwrap();
        assert_eq!(q, "What is the process “mds_stores” (PID 412, the program at /usr/libexec/mds_stores, run by chris, using 12.3% of the CPU and 300 MiB of memory) on this computer? What does it do, is it part of the system or of an app, and is it safe to end it?");
        // What is not known is left out, not made up.
        let bare = question(&[About { program: None, user: String::new(), ..about("kernel_task", 0) }]).unwrap();
        assert!(bare.contains("“kernel_task” (PID 0, run by an unknown user, using") && !bare.contains("program at"), "{bare}");
    }

    #[test]
    fn several_are_asked_about_together_and_those_of_one_name_once() {
        let q = question(&[about("Safari", 1), about("WebKit", 2), about("WebKit", 3), about("WebKit", 4), about("mds", 5)]).unwrap();
        let lines: Vec<&str> = q.lines().collect();
        assert_eq!(lines.len(), 4, "{q}");
        assert!(lines[0].starts_with("What are these processes") && lines[1].starts_with("- “Safari” (PID 1,") && lines[2].ends_with("one of 3 with that name") && lines[3].starts_with("- “mds”"), "{q}");
        // A great many: the first dozen, and how many more there were.
        let many: Vec<About> = (0..40).map(|i| about(&format!("helper{i}"), i)).collect();
        let q = question(&many).unwrap();
        assert_eq!((q.lines().count(), q.lines().last()), (MOST + 2, Some("- and 28 more not named here")));
    }
}
