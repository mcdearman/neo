//! Asking Apollo something from outside its window: the search bar the
//! Launcher brings up, or a script.
//!
//! A running Apollo listens the way NeoShell listens for notifications,
//! on a port it notes in a file. A question is sent there and Apollo is
//! brought to the front; with no Apollo running, it is started with the
//! question.

use std::path::{Path, PathBuf};

use crate::notify::{Inbox, Notification};

/// Where a running Apollo notes the port it listens for questions on.
pub fn port_file() -> PathBuf {
    crate::config_dir().join("apollo").join("ask.port")
}

/// Apollo's end: where questions arrive.
pub struct Questions(Inbox);

impl Questions {
    /// Starts listening for questions, and notes where.
    pub fn open() -> std::io::Result<Self> {
        Self::open_at(port_file())
    }

    pub fn open_at(port_file: PathBuf) -> std::io::Result<Self> {
        Inbox::open_at(port_file).map(Self)
    }

    /// Waits for the next thing asked of it.
    pub fn next(&self) -> std::io::Result<Asked> {
        self.0.recv().map(|note| match note {
            note if note.title == SHOW => Asked::Show,
            note if note.body.trim().is_empty() => Asked::Question(note.title),
            note => Asked::About(note.title, note.body),
        })
    }
}

/// What a running Apollo is asked from outside.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Asked {
    Question(String),
    /// A question, and what the asker found out that bears on it: to be
    /// answered from that, not from Apollo's memory of the user's files.
    About(String, String),
    /// Only to show its window: it was opened again while running out of sight.
    Show,
}

/// What stands for "show yourself", which no question is.
const SHOW: &str = "\u{1}show";

/// Has the running Apollo show its window. False if none is running.
pub fn show() -> bool {
    let shown = send_to(&port_file(), SHOW);
    if shown && cfg!(target_os = "macos") {
        let _ = std::process::Command::new("open").args(["-b", "org.neo.Apollo"]).spawn();
    }
    shown
}

/// Sends a question to the Apollo that noted its port in `port_file`.
/// False if none is there to hear it.
pub fn send_to(port_file: &Path, question: &str) -> bool {
    crate::notify::send_to(port_file, &Notification::new(question, ""))
}

/// Whether an Apollo is running and listening.
pub fn running() -> bool {
    crate::notify::answers(&port_file())
}

/// Asks Apollo `question` and brings its window up: the running Apollo if
/// there is one, or else a new one started with the question.
pub fn ask(question: &str) -> std::io::Result<()> {
    if send_to(&port_file(), question) {
        // It has the question; all that is left is to bring it forward.
        if cfg!(target_os = "macos") {
            std::process::Command::new("open").args(["-b", "org.neo.Apollo"]).spawn()?;
        }
        return Ok(());
    }
    if crate::fs::open_with("neo-apollo", "Apollo", &["--ask", question]) { Ok(()) } else { Err(std::io::Error::other("Apollo is not installed")) }
}

/// Asks Apollo `question` with `facts` the asker has found out, which it
/// answers from and shows with its answer: System Monitor asking what a
/// process is, with what the system says of it.
pub fn ask_about(question: &str, facts: &str) -> std::io::Result<()> {
    if crate::notify::send_to(&port_file(), &Notification::new(question, facts)) {
        if cfg!(target_os = "macos") {
            std::process::Command::new("open").args(["-b", "org.neo.Apollo"]).spawn()?;
        }
        return Ok(());
    }
    if crate::fs::open_with("neo-apollo", "Apollo", &["--ask", question, "--facts", facts]) { Ok(()) } else { Err(std::io::Error::other("Apollo is not installed")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_question_reaches_the_apollo_that_is_listening_and_no_other() {
        let file = std::env::temp_dir().join(format!("neo-apollo-ask-{}", std::process::id())).join("ask.port");
        assert!(!send_to(&file, "anyone?"), "with none listening, it is said so");
        let questions = Questions::open_at(file.clone()).unwrap();
        let heard = std::thread::spawn(move || (questions.next().unwrap(), questions.next().unwrap()));
        assert!(send_to(&file, "Which photos show a dog?"));
        assert!(send_to(&file, "Two lines\nand an = sign"));
        assert_eq!(heard.join().unwrap(), (Asked::Question("Which photos show a dog?".to_owned()), Asked::Question("Two lines\nand an = sign".to_owned())));
        // Asked only to show itself, it hears that and not a question.
        let questions = Questions::open_at(file.clone()).unwrap();
        let heard = std::thread::spawn(move || questions.next().unwrap());
        assert!(send_to(&file, SHOW));
        assert_eq!(heard.join().unwrap(), Asked::Show);
        assert!(!send_to(&file, "gone now"), "once it has stopped listening, too");
        std::fs::remove_dir_all(file.parent().unwrap()).unwrap();
    }
}
