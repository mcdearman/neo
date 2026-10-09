//! The Network page: how much is coming and going, over which
//! interfaces, and which programs are doing it.

use std::collections::HashMap;
use std::time::Instant;

use neo::prelude::*;
use neo_desktop::fs::human_size;
use sysinfo::Networks;

use crate::{Monitor, Msg, panel, rate};

/// A network interface, and what is passing over it.
#[derive(Clone, Debug, PartialEq)]
pub struct Interface {
    pub name: String,
    /// Its addresses, as they are written: "192.168.1.20/24".
    pub addresses: Vec<String>,
    pub mac: String,
    /// Bytes a second, coming in and going out.
    pub down: f32,
    pub up: f32,
    /// Bytes since the computer started.
    pub total_down: u64,
    pub total_up: u64,
}

/// What kind of interface a name is, where the name says.
pub fn kind(name: &str) -> &'static str {
    let starts = |prefixes: &[&str]| prefixes.iter().any(|p| name.starts_with(p));
    if starts(&["lo"]) {
        "This computer to itself"
    } else if starts(&["utun", "tun", "tap", "wg", "ppp", "ipsec"]) {
        "Tunnel or VPN"
    } else if starts(&["awdl", "llw"]) {
        "AirDrop and nearby devices"
    } else if starts(&["bridge", "br-", "docker", "virbr", "vmnet", "veth"]) {
        "Bridge to virtual machines"
    } else if starts(&["wl"]) {
        "Wi-Fi"
    } else if starts(&["eth", "enp", "eno", "ens"]) {
        "Ethernet"
    } else if starts(&["en"]) {
        // On a Mac both are called this.
        "Wi-Fi or Ethernet"
    } else {
        ""
    }
}

/// One with no address is shown only if it has carried this much: the
/// tunnels a Mac keeps ready each pass a few hundred bytes of nothing.
const WORTH_SHOWING: u64 = 1024 * 1024;

/// An address that is only good on the link itself, which every
/// interface has whether or not it is in use.
fn link_local(address: &std::net::IpAddr) -> bool {
    match address {
        std::net::IpAddr::V4(a) => a.is_link_local(),
        std::net::IpAddr::V6(a) => (a.segments()[0] & 0xffc0) == 0xfe80,
    }
}

/// The interfaces worth showing: those with an address to their name or
/// real traffic to show for themselves, the busiest first. A Mac has a
/// dozen that are neither. `secs` is how long it is since the last reading.
pub fn interfaces(networks: &Networks, secs: f32) -> Vec<Interface> {
    let mut out: Vec<Interface> = networks
        .list()
        .iter()
        .map(|(name, n)| Interface { name: name.clone(), addresses: n.ip_networks().iter().filter(|ip| !link_local(&ip.addr)).map(|ip| format!("{}/{}", ip.addr, ip.prefix)).collect(), mac: n.mac_address().to_string(), down: n.received() as f32 / secs.max(0.05), up: n.transmitted() as f32 / secs.max(0.05), total_down: n.total_received(), total_up: n.total_transmitted() })
        .filter(|i| !i.addresses.is_empty() || i.total_down + i.total_up >= WORTH_SHOWING)
        .collect();
    out.sort_by(|a, b| (b.total_down + b.total_up).cmp(&(a.total_down + a.total_up)).then_with(|| a.name.cmp(&b.name)));
    out
}

/// A program, and what it is sending and receiving.
#[derive(Clone, Debug, PartialEq)]
pub struct Using {
    pub name: String,
    pub pid: u32,
    pub down: f32,
    pub up: f32,
    pub total_down: u64,
    pub total_up: u64,
}

/// Reads `nettop -P -L 1 -x -J bytes_in,bytes_out`: each program with
/// its process number, and the bytes it has received and sent.
pub fn parse_nettop(text: &str) -> Vec<(String, u32, u64, u64)> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split(',');
            // "Google Chrome H.1234": the number after the last dot.
            let (name, pid) = fields.next()?.rsplit_once('.')?;
            let (down, up) = (fields.next()?.trim().parse().ok()?, fields.next()?.trim().parse().ok()?);
            Some((name.to_owned(), pid.parse().ok()?, down, up))
        })
        .collect()
}

/// Turns readings of what programs have sent and received in all into
/// what they are sending and receiving now.
#[derive(Default)]
pub struct Watcher {
    last: HashMap<u32, (u64, u64)>,
    at: Option<Instant>,
}

impl Watcher {
    /// Takes a reading, made at `now`. The first has nothing to be
    /// compared with, so its rates are nothing.
    pub fn take(&mut self, reading: Vec<(String, u32, u64, u64)>, now: Instant) -> Vec<Using> {
        let secs = self.at.map(|t| now.duration_since(t).as_secs_f32().max(0.05));
        let mut out: Vec<Using> = reading
            .iter()
            .map(|(name, pid, down, up)| {
                // A count that went down is a new process with an old number.
                let before = self.last.get(pid).filter(|(d, u)| d <= down && u <= up);
                let (rate_down, rate_up) = match (before, secs) {
                    (Some((d, u)), Some(secs)) => ((down - d) as f32 / secs, (up - u) as f32 / secs),
                    _ => (0.0, 0.0),
                };
                Using { name: name.clone(), pid: *pid, down: rate_down, up: rate_up, total_down: *down, total_up: *up }
            })
            .filter(|u| u.total_down + u.total_up > 0)
            .collect();
        self.last = reading.into_iter().map(|(_, pid, down, up)| (pid, (down, up))).collect();
        self.at = Some(now);
        // What is busy now first; among the idle, what has done most.
        out.sort_by(|a, b| (b.down + b.up).total_cmp(&(a.down + a.up)).then_with(|| (b.total_down + b.total_up).cmp(&(a.total_down + a.total_up))).then_with(|| a.name.cmp(&b.name)));
        out
    }

    /// Asks the system which programs are using the network. `None`
    /// where it does not say without being asked as its administrator:
    /// everywhere but macOS, so far.
    pub fn sample(&mut self) -> Option<Vec<Using>> {
        if !cfg!(target_os = "macos") {
            return None;
        }
        let out = std::process::Command::new("/usr/bin/nettop").args(["-P", "-L", "1", "-x", "-J", "bytes_in,bytes_out"]).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok()?;
        out.status.success().then(|| self.take(parse_nettop(&String::from_utf8_lossy(&out.stdout)), Instant::now()))
    }
}

/// How many programs the page lists.
const LISTED: usize = 25;

impl Monitor {
    pub(crate) fn network(&self) -> Element<Msg> {
        let peak = self.rx.iter().chain(self.tx.iter()).copied().fold(1024.0f32, f32::max) * 1.15;
        let (rx, tx) = (*self.rx.back().unwrap_or(&0.0), *self.tx.back().unwrap_or(&0.0));
        let (total_down, total_up) = self.interfaces.iter().filter(|i| !i.name.starts_with("lo")).fold((0u64, 0u64), |(d, u), i| (d + i.total_down, u + i.total_up));
        let flow = |title: &str, now: f32, total: u64, history: Vec<f32>, tone: Tone| -> Element<Msg> { panel(title, format!("{} since the computer started", human_size(total)), column().spacing(8.0).width(Length::Fill).push(text(rate(now)).role(TextRole::Heading)).push(sparkline(history, 0.0, peak).height(70.0).tone(tone))) };
        let flows = row().spacing(16.0).width(Length::Fill).push(flow("Download", rx, total_down, Vec::from(self.rx.clone()), Tone::Accent)).push(flow("Upload", tx, total_up, Vec::from(self.tx.clone()), Tone::Warn));

        // The interfaces.
        let cell = |words: String, width: f32| text(words).mono().role(TextRole::Caption).align(Align::End).width(width);
        let heading = |words: &str, width: Length, align: Align| text(words).role(TextRole::Label).tone(Tone::Muted).align(align).width(width);
        let mut table = column()
            .spacing(8.0)
            .width(Length::Fill)
            .push(row().spacing(12.0).width(Length::Fill).push(heading("Interface", Length::Fixed(190.0), Align::Start)).push(heading("Addresses", Length::Fill, Align::Start)).push(heading("Down", Length::Fixed(90.0), Align::End)).push(heading("Up", Length::Fixed(90.0), Align::End)).push(heading("Received", Length::Fixed(90.0), Align::End)).push(heading("Sent", Length::Fixed(90.0), Align::End)));
        for i in &self.interfaces {
            let what = kind(&i.name);
            let mut name = column().spacing(1.0).width(190.0).push(text(i.name.clone()).role(TextRole::Strong).no_wrap());
            if !what.is_empty() {
                name = name.push(text(what).role(TextRole::Caption).tone(Tone::Muted).no_wrap());
            }
            let addresses = if i.addresses.is_empty() { "No address".to_owned() } else { i.addresses.join("\n") };
            let mut where_it_is = column().spacing(1.0).width(Length::Fill).push(text(addresses).mono().role(TextRole::Caption).tone(if i.addresses.is_empty() { Tone::Faint } else { Tone::Inherit }));
            if i.mac != "00:00:00:00:00:00" && !i.mac.is_empty() {
                where_it_is = where_it_is.push(text(i.mac.clone()).mono().role(TextRole::Caption).tone(Tone::Faint));
            }
            table = table.push(Divider::horizontal()).push(row().spacing(12.0).align(Align::Center).width(Length::Fill).push(name).push(where_it_is).push(cell(rate(i.down), 90.0)).push(cell(rate(i.up), 90.0)).push(cell(human_size(i.total_down), 90.0)).push(cell(human_size(i.total_up), 90.0)));
        }
        if self.interfaces.is_empty() {
            table = table.push(text("No interface has an address or has carried anything.").tone(Tone::Muted));
        }
        let interfaces = panel("Interfaces", format!("{} in use", self.interfaces.len()), table);

        // The programs.
        let programs: Element<Msg> = match &self.net_using {
            _ if !cfg!(target_os = "macos") => text("Which programs are using the network is not something this system tells a program that is not its administrator.").tone(Tone::Muted).width(Length::Fill).into(),
            None => text("Taking the first reading…").tone(Tone::Muted).into(),
            Some(using) => {
                let mut list = column()
                    .spacing(6.0)
                    .width(Length::Fill)
                    .push(row().spacing(12.0).width(Length::Fill).push(heading("Program", Length::Fill, Align::Start)).push(heading("PID", Length::Fixed(70.0), Align::End)).push(heading("Down", Length::Fixed(90.0), Align::End)).push(heading("Up", Length::Fixed(90.0), Align::End)).push(heading("Received", Length::Fixed(90.0), Align::End)).push(heading("Sent", Length::Fixed(90.0), Align::End)));
                for u in using.iter().take(LISTED) {
                    let busy = u.down + u.up >= 1.0;
                    list = list.push(
                        row()
                            .spacing(12.0)
                            .align(Align::Center)
                            .width(Length::Fill)
                            .push(text(u.name.clone()).role(if busy { TextRole::Strong } else { TextRole::Body }).tone(if busy { Tone::Inherit } else { Tone::Muted }).no_wrap().width(Length::Fill))
                            .push(cell(u.pid.to_string(), 70.0).tone(Tone::Muted))
                            .push(cell(rate(u.down), 90.0).tone(if u.down >= 1.0 { Tone::Accent } else { Tone::Faint }))
                            .push(cell(rate(u.up), 90.0).tone(if u.up >= 1.0 { Tone::Warn } else { Tone::Faint }))
                            .push(cell(human_size(u.total_down), 90.0))
                            .push(cell(human_size(u.total_up), 90.0)),
                    );
                }
                if using.is_empty() {
                    list = list.push(text("Nothing has used the network.").tone(Tone::Muted));
                }
                list.into()
            }
        };
        let busy = self.net_using.as_ref().map_or(0, |u| u.iter().filter(|u| u.down + u.up >= 1.0).count());
        let programs = panel("Programs", if self.net_using.is_some() { format!("{busy} sending or receiving now") } else { String::new() }, programs);
        scrollable(column().spacing(16.0).width(Length::Fill).padding(18.0).push(flows).push(interfaces).push(programs)).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn what_programs_have_sent_and_received_is_read() {
        let text = ",bytes_in,bytes_out,\nsyslogd.352,0,71304,\nGoogle Chrome H.48213,3294641,772134,\nmDNSResponder.475,10,20,\nnot a line\nname.with.dots.9,1,2,\nbroken.x,1,2,\n";
        assert_eq!(parse_nettop(text), [("syslogd".to_owned(), 352, 0, 71304), ("Google Chrome H".to_owned(), 48213, 3294641, 772134), ("mDNSResponder".to_owned(), 475, 10, 20), ("name.with.dots".to_owned(), 9, 1, 2)]);
        assert_eq!(parse_nettop(""), vec![]);
    }

    #[test]
    fn totals_become_rates_between_one_reading_and_the_next() {
        let mut w = Watcher::default();
        let t = Instant::now();
        let first = w.take(vec![("curl".into(), 10, 1000, 100), ("idle".into(), 11, 50_000, 0), ("silent".into(), 12, 0, 0)], t);
        assert!(first.iter().all(|u| u.down == 0.0 && u.up == 0.0), "nothing to compare with the first time");
        assert_eq!(first.iter().map(|u| u.name.as_str()).collect::<Vec<_>>(), ["idle", "curl"], "what has done most first; what has done nothing is left out");
        let next = w.take(vec![("curl".into(), 10, 5000, 300), ("idle".into(), 11, 50_000, 0), ("new".into(), 13, 8, 8), ("reused".into(), 11_000, 5, 5)], t + Duration::from_secs(2));
        assert_eq!((next[0].name.as_str(), next[0].down, next[0].up, next[0].total_down), ("curl", 2000.0, 100.0, 5000), "busy now, it comes first");
        assert_eq!(next.iter().find(|u| u.name == "new").map(|u| u.down), Some(0.0), "one not seen before has no rate yet");
        // A process number given to a new process, whose count starts again, is not a rate of minus millions.
        let again = w.take(vec![("other".into(), 10, 10, 10)], t + Duration::from_secs(4));
        assert_eq!((again[0].down, again[0].up), (0.0, 0.0));
    }

    #[test]
    fn interfaces_are_named_for_what_they_are() {
        assert_eq!((kind("lo0"), kind("utun3"), kind("awdl0"), kind("bridge100"), kind("wlan0"), kind("enp3s0"), kind("en0"), kind("xyz")), ("This computer to itself", "Tunnel or VPN", "AirDrop and nearby devices", "Bridge to virtual machines", "Wi-Fi", "Ethernet", "Wi-Fi or Ethernet", ""));
        assert!(link_local(&"169.254.3.4".parse().unwrap()) && link_local(&"fe80::1".parse().unwrap()));
        assert!(!link_local(&"192.168.1.20".parse().unwrap()) && !link_local(&"2001:db8::1".parse().unwrap()));
    }

    #[test]
    fn this_computer_has_interfaces_to_show() {
        let networks = Networks::new_with_refreshed_list();
        let found = interfaces(&networks, 1.0);
        assert!(!found.is_empty(), "the one it talks to itself over, at least");
        assert!(found.iter().all(|i| !i.addresses.is_empty() || i.total_down + i.total_up >= WORTH_SHOWING));
        assert!(found.windows(2).all(|w| w[0].total_down + w[0].total_up >= w[1].total_down + w[1].total_up), "the busiest first");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn this_computer_says_which_programs_use_the_network() {
        let mut w = Watcher::default();
        let using = w.sample().expect("nettop answers");
        assert!(!using.is_empty() && using.iter().all(|u| !u.name.is_empty() && u.pid > 0));
    }
}
