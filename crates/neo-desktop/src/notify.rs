//! Notifications: a Neo app describes one and sends it to NeoShell, which
//! shows it.
//!
//! The two talk over a datagram on this computer's loopback address.
//! NeoShell writes the port it listens on to a file in Neo's settings
//! folder; a sender reads it, sends the notification, and waits a moment
//! for NeoShell to say it has it. No answer means NeoShell is not running,
//! and the sender can show the news itself instead.

use std::net::{SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::config_dir;

/// Something to tell the user, shown briefly at the edge of the screen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Notification {
    pub title: String,
    pub body: String,
    /// A picture to show small beside the text.
    pub image: Option<PathBuf>,
    /// A file the notification is about, to offer to show in Files.
    pub reveal: Option<PathBuf>,
    /// A file to open when the notification itself is clicked: a picture
    /// in Photos, a video in Videos, anything else in the system's choice.
    pub open: Option<PathBuf>,
}

const PING: &[u8] = b"neo-ping";
const OK: &[u8] = b"neo-ok";

/// Where NeoShell notes the port it is listening on.
pub fn port_file() -> PathBuf {
    config_dir().join("shell.port")
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\n', "\\n")
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

impl Notification {
    pub fn new(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self { title: title.into(), body: body.into(), image: None, reveal: None, open: None }
    }

    pub fn image(mut self, path: impl Into<PathBuf>) -> Self {
        self.image = Some(path.into());
        self
    }

    pub fn reveal(mut self, path: impl Into<PathBuf>) -> Self {
        self.reveal = Some(path.into());
        self
    }

    /// Clicking the notification opens this file.
    pub fn open(mut self, path: impl Into<PathBuf>) -> Self {
        self.open = Some(path.into());
        self
    }

    /// The notification as `key=value` lines, the form it is sent in.
    pub fn encode(&self) -> String {
        let mut out = format!("title={}\nbody={}\n", escape(&self.title), escape(&self.body));
        for (key, path) in [("image", &self.image), ("reveal", &self.reveal), ("open", &self.open)] {
            if let Some(p) = path {
                out.push_str(&format!("{key}={}\n", escape(&p.to_string_lossy())));
            }
        }
        out
    }

    /// Reads what [`encode`](Self::encode) wrote. `None` if it has no title.
    pub fn decode(src: &str) -> Option<Self> {
        let mut n = Self::default();
        for line in src.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            let value = unescape(value);
            match key {
                "title" => n.title = value,
                "body" => n.body = value,
                "image" => n.image = Some(value.into()),
                "reveal" => n.reveal = Some(value.into()),
                "open" => n.open = Some(value.into()),
                _ => {}
            }
        }
        (!n.title.is_empty()).then_some(n)
    }

    /// Sends the notification to NeoShell. Returns false if NeoShell is
    /// not there to show it.
    pub fn send(&self) -> bool {
        deliver(&port_file(), self.encode().as_bytes())
    }
}

/// Whether NeoShell is running and answering.
pub fn shell_running() -> bool {
    deliver(&port_file(), PING)
}

/// Sends `payload` to whoever noted its port in `port_file`, and waits
/// briefly to hear that it arrived.
fn deliver(port_file: &Path, payload: &[u8]) -> bool {
    let Some(port) = std::fs::read_to_string(port_file).ok().and_then(|s| s.trim().parse::<u16>().ok()) else { return false };
    let Ok(socket) = UdpSocket::bind("127.0.0.1:0") else { return false };
    let to = SocketAddr::from(([127, 0, 0, 1], port));
    if socket.set_read_timeout(Some(Duration::from_millis(300))).is_err() || socket.send_to(payload, to).is_err() {
        return false;
    }
    let mut reply = [0u8; 16];
    matches!(socket.recv_from(&mut reply), Ok((n, from)) if from == to && &reply[..n] == OK)
}

/// NeoShell's end: where notifications arrive.
pub struct Inbox {
    socket: UdpSocket,
    port_file: PathBuf,
}

impl Inbox {
    /// Starts listening, and notes where in [`port_file`].
    pub fn open() -> std::io::Result<Self> {
        Self::open_at(port_file())
    }

    /// As [`open`](Self::open), noting the port in a file of the caller's choosing.
    pub fn open_at(port_file: PathBuf) -> std::io::Result<Self> {
        let socket = UdpSocket::bind("127.0.0.1:0")?;
        if let Some(dir) = port_file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&port_file, socket.local_addr()?.port().to_string())?;
        Ok(Self { socket, port_file })
    }

    /// Waits for the next notification. Each sender is told it arrived.
    pub fn recv(&self) -> std::io::Result<Notification> {
        // Larger than any datagram the loopback will carry.
        let mut buf = vec![0u8; 65_536];
        loop {
            let (n, from) = self.socket.recv_from(&mut buf)?;
            let _ = self.socket.send_to(OK, from);
            if &buf[..n] != PING
                && let Some(note) = std::str::from_utf8(&buf[..n]).ok().and_then(Notification::decode)
            {
                return Ok(note);
            }
        }
    }
}

impl Drop for Inbox {
    fn drop(&mut self) {
        // Senders should not wait on a port nobody listens to any more.
        let _ = std::fs::remove_file(&self.port_file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("neo-notify-{}-{name}", std::process::id())).join("shell.port")
    }

    #[test]
    fn a_notification_survives_being_written_down() {
        let n = Notification::new("Screenshot copied", "Saved as a = b\\c\nsecond line").image("/tmp/a shot.png").reveal("/tmp/a shot.png").open("/tmp/a shot.png");
        assert_eq!(Notification::decode(&n.encode()), Some(n));
        let plain = Notification::new("Done", "");
        assert_eq!(Notification::decode(&plain.encode()), Some(plain));
        assert_eq!(Notification::decode("body=no title\n"), None);
        assert_eq!(Notification::decode(""), None);
    }

    #[test]
    fn notifications_reach_the_inbox_and_the_sender_hears_so() {
        let file = scratch("delivery");
        let inbox = Inbox::open_at(file.clone()).unwrap();
        let listener = std::thread::spawn(move || {
            let first = inbox.recv().unwrap();
            let second = inbox.recv().unwrap();
            (first, second)
        });
        let a = Notification::new("One", "first");
        let b = Notification::new("Two", "second").reveal("/tmp/x");
        assert!(deliver(&file, PING), "it answers a ping without treating it as a notification");
        assert!(deliver(&file, a.encode().as_bytes()));
        assert!(deliver(&file, b.encode().as_bytes()));
        assert_eq!(listener.join().unwrap(), (a, b));
        // With the inbox gone its port file is too, and sending says so at once.
        assert!(!file.exists());
        assert!(!deliver(&file, PING));
        std::fs::remove_dir_all(file.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_port_nobody_listens_on_is_not_a_delivery() {
        let file = scratch("stale");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        // A port that was free a moment ago, as a NeoShell that crashed would leave.
        let port = UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        std::fs::write(&file, port.to_string()).unwrap();
        assert!(!deliver(&file, PING));
        std::fs::write(&file, "not a port").unwrap();
        assert!(!deliver(&file, PING));
        std::fs::remove_dir_all(file.parent().unwrap()).unwrap();
    }
}
