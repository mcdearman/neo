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

/// The question to ask of these processes, or nothing if there are none.
/// It is short: what was found out about them goes beside it, as facts.
pub fn question(chosen: &[About]) -> Option<String> {
    match chosen {
        [] => None,
        [one] => Some(format!("What is the process “{}”, where does it come from, and why is it using what it is?", one.name)),
        many => {
            let mut names: Vec<&str> = vec![];
            for p in many {
                if !names.contains(&p.name.as_str()) {
                    names.push(&p.name);
                }
            }
            let more = names.len().saturating_sub(MOST);
            let mut list = names.iter().take(MOST).map(|n| format!("“{n}”")).collect::<Vec<_>>().join(", ");
            if more > 0 {
                list.push_str(&format!(" and {more} more"));
            }
            Some(format!("What are these processes, and where does each come from? {list}"))
        }
    }
}

/// The facts found about several processes, set out one process to a
/// paragraph, and kept short enough to be sent: the first lines of each.
pub fn set_out(found: &[Vec<String>]) -> String {
    let each = (MOST_SENT / found.len().max(1)).max(200);
    let mut out = String::new();
    for facts in found {
        let mut part = String::new();
        for fact in facts {
            if part.len() + fact.len() + 1 > each && !part.is_empty() {
                break;
            }
            part.push_str(fact);
            part.push('\n');
        }
        out.push_str(part.trim_end());
        out.push_str("\n\n");
    }
    let mut out = out.trim_end().to_owned();
    while out.len() > MOST_SENT {
        out.pop();
    }
    out
}

/// The most that is sent to Apollo with a question, in bytes: it goes in
/// one packet, and the model that reads it is a small one.
pub const MOST_SENT: usize = 5000;

#[cfg(test)]
mod tests {
    use super::*;

    fn about(name: &str, pid: u32) -> About {
        About { name: name.into(), pid, user: "chris".into(), program: Some(format!("/usr/libexec/{name}")), cpu: 12.34, memory: 300 * 1024 * 1024 }
    }

    #[test]
    fn the_question_is_short_and_names_what_is_asked_about() {
        assert_eq!(question(&[]), None);
        assert_eq!(question(&[about("mds_stores", 412)]).as_deref(), Some("What is the process “mds_stores”, where does it come from, and why is it using what it is?"));
        // Several: those of one name once.
        assert_eq!(question(&[about("Safari", 1), about("WebKit", 2), about("WebKit", 3), about("mds", 5)]).as_deref(), Some("What are these processes, and where does each come from? “Safari”, “WebKit”, “mds”"));
        let many: Vec<About> = (0..40).map(|i| about(&format!("helper{i}"), i)).collect();
        assert!(question(&many).unwrap().ends_with("“helper11” and 28 more"));
    }

    #[test]
    fn what_was_found_is_set_out_a_process_to_a_paragraph_and_kept_short() {
        let one = vec!["A is run by chris.".to_owned(), "Its program is /a.".to_owned()];
        assert_eq!(set_out(&[one.clone(), vec!["B is run by root.".to_owned()]]), "A is run by chris.\nIts program is /a.\n\nB is run by root.");
        // A great deal found about a great many: each keeps its first lines, and the whole fits.
        let long: Vec<Vec<String>> = (0..30).map(|i| (0..20).map(|j| format!("Process {i} fact {j} {}", "x".repeat(60))).collect()).collect();
        let out = set_out(&long);
        assert!(out.len() <= MOST_SENT && out.contains("Process 0 fact 0") && out.contains("Process 20 fact 0") && !out.contains("Process 0 fact 9"), "{}", out.len());
    }
}
