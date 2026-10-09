//! The network: what this computer is connected by, and Wi-Fi on and off.
//!
//! macOS is asked through `networksetup`, which answers without leave
//! and changes Wi-Fi's power for the user who is logged in. Linux is
//! asked through NetworkManager's `nmcli`. What is passing over the
//! network is System Monitor's to show.

use std::process::{Command, Stdio};

/// One way this computer can be on a network: Wi-Fi, Ethernet, a bridge.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Connection {
    /// What the system calls it: "Wi-Fi", "Ethernet Adapter (en4)".
    pub name: String,
    /// The interface under it: "en0".
    pub device: String,
    pub mac: String,
    /// Its address, if it is connected.
    pub address: Option<String>,
    pub router: Option<String>,
    /// Whether the address was given by the network (DHCP) or set by hand.
    pub automatic: bool,
    /// The name servers set for it by hand; none means the network's own.
    pub dns: Vec<String>,
    pub wifi: bool,
}

impl Connection {
    pub fn connected(&self) -> bool {
        self.address.is_some()
    }
}

/// The state of the network, as far as it is told.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Network {
    pub connections: Vec<Connection>,
    /// Whether Wi-Fi is turned on, where there is Wi-Fi and it says.
    pub wifi_on: Option<bool>,
}

/// Reads `networksetup -listallhardwareports`: each port's name, its
/// interface and its hardware address.
pub fn parse_ports(text: &str) -> Vec<Connection> {
    let mut out = vec![];
    let mut now: Option<Connection> = None;
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("Hardware Port: ") {
            out.extend(now.take());
            now = Some(Connection { name: name.trim().to_owned(), wifi: name.contains("Wi-Fi") || name.contains("AirPort"), ..Connection::default() });
        } else if let (Some(c), Some(device)) = (&mut now, line.strip_prefix("Device: ")) {
            c.device = device.trim().to_owned();
        } else if let (Some(c), Some(mac)) = (&mut now, line.strip_prefix("Ethernet Address: ")) {
            c.mac = mac.trim().to_owned();
        }
    }
    out.extend(now);
    out.retain(|c| !c.device.is_empty());
    out
}

/// Reads `networksetup -getinfo <port>` into a connection: its address,
/// its router, and whether the network gave them.
pub fn read_info(c: &mut Connection, text: &str) {
    let field = |name: &str| text.lines().find_map(|l| l.strip_prefix(name)).map(str::trim).filter(|v| !v.is_empty() && *v != "none").map(str::to_owned);
    c.address = field("IP address: ");
    c.router = field("Router: ");
    c.automatic = text.lines().next().is_some_and(|l| l.contains("DHCP") || l.contains("BOOTP"));
}

/// Reads `networksetup -getdnsservers <port>`: the servers, one a line,
/// or a sentence saying there are none.
pub fn parse_dns(text: &str) -> Vec<String> {
    text.lines().map(str::trim).filter(|l| !l.is_empty() && l.chars().all(|c| c.is_ascii_hexdigit() || c == '.' || c == ':')).map(str::to_owned).collect()
}

/// Reads `networksetup -getairportpower <device>`: `Wi-Fi Power (en0): On`.
pub fn parse_power(text: &str) -> Option<bool> {
    match text.trim().rsplit(':').next()?.trim() {
        "On" => Some(true),
        "Off" => Some(false),
        _ => None,
    }
}

/// Reads `nmcli -t -f DEVICE,TYPE,STATE,CONNECTION device`: each device
/// with its kind, its state and the network it is on.
#[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]
pub fn parse_nmcli(text: &str) -> Vec<Connection> {
    text.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.splitn(4, ':').collect();
            let [device, kind, state, name] = fields.as_slice() else { return None };
            if !matches!(*kind, "wifi" | "ethernet") {
                return None;
            }
            let label = if *kind == "wifi" { "Wi-Fi" } else { "Ethernet" };
            Some(Connection { name: if name.is_empty() || *state != "connected" { label.to_owned() } else { format!("{label}: {name}") }, device: (*device).to_owned(), wifi: *kind == "wifi", automatic: true, ..Connection::default() })
        })
        .collect()
}

fn run(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(neo_desktop::fs::tool(program)).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Asks the system how this computer is connected. It takes a moment:
/// to be called off the main thread.
pub fn read() -> Network {
    if cfg!(target_os = "macos") {
        let mut connections = run("/usr/sbin/networksetup", &["-listallhardwareports"]).map(|t| parse_ports(&t)).unwrap_or_default();
        for c in &mut connections {
            if let Some(info) = run("/usr/sbin/networksetup", &["-getinfo", &c.name]) {
                read_info(c, &info);
            }
            if c.connected() {
                c.dns = run("/usr/sbin/networksetup", &["-getdnsservers", &c.name]).map(|t| parse_dns(&t)).unwrap_or_default();
            }
        }
        let wifi_on = connections.iter().find(|c| c.wifi).and_then(|c| run("/usr/sbin/networksetup", &["-getairportpower", &c.device])).and_then(|t| parse_power(&t));
        // What is connected, and Wi-Fi whether or not it is; a Mac lists a dozen ports it has nothing on.
        connections.retain(|c| c.connected() || c.wifi);
        connections.sort_by_key(|c| (!c.connected(), !c.wifi));
        return Network { connections, wifi_on };
    }
    if cfg!(unix) {
        let mut connections = run("nmcli", &["-t", "-f", "DEVICE,TYPE,STATE,CONNECTION", "device"]).map(|t| parse_nmcli(&t)).unwrap_or_default();
        for c in &mut connections {
            // "IP4.ADDRESS[1]:192.168.1.20/24", "IP4.GATEWAY:192.168.1.1", "IP4.DNS[1]:…".
            let Some(shown) = run("nmcli", &["-t", "-f", "GENERAL.HWADDR,IP4.ADDRESS,IP4.GATEWAY,IP4.DNS", "device", "show", &c.device]) else { continue };
            let value = |key: &str| shown.lines().find(|l| l.starts_with(key)).and_then(|l| l.split_once(':')).map(|(_, v)| v.trim().to_owned()).filter(|v| !v.is_empty() && v != "--");
            c.mac = value("GENERAL.HWADDR").unwrap_or_default().replace("\\:", ":");
            c.address = value("IP4.ADDRESS").map(|a| a.split('/').next().unwrap_or(&a).to_owned());
            c.router = value("IP4.GATEWAY");
        }
        let wifi_on = run("nmcli", &["radio", "wifi"]).map(|t| t.trim() == "enabled");
        return Network { connections, wifi_on };
    }
    Network::default()
}

/// Turns Wi-Fi on or off. An error says why it could not be.
pub fn set_wifi(device: &str, on: bool) -> Result<(), String> {
    let done = if cfg!(target_os = "macos") {
        run("/usr/sbin/networksetup", &["-setairportpower", device, if on { "on" } else { "off" }])
    } else if cfg!(unix) {
        run("nmcli", &["radio", "wifi", if on { "on" } else { "off" }])
    } else {
        None
    };
    done.map(|_| ()).ok_or_else(|| "The system would not change Wi-Fi. It may need an administrator.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_the_system_says_of_its_ports_is_read() {
        let ports = "\nHardware Port: Ethernet Adapter (en4)\nDevice: en4\nEthernet Address: 1e:b2:a0:c0:d8:b2\n\nHardware Port: Thunderbolt Bridge\nDevice: bridge0\nEthernet Address: 36:aa:bb:cc:dd:ee\n\nHardware Port: Wi-Fi\nDevice: en0\nEthernet Address: 7a:41:64:30:df:45\n\nVLAN Configurations\n===================\n";
        let mut found = parse_ports(ports);
        assert_eq!(found.iter().map(|c| (c.name.as_str(), c.device.as_str(), c.wifi)).collect::<Vec<_>>(), [("Ethernet Adapter (en4)", "en4", false), ("Thunderbolt Bridge", "bridge0", false), ("Wi-Fi", "en0", true)]);
        assert_eq!(found[2].mac, "7a:41:64:30:df:45");
        read_info(&mut found[2], "DHCP Configuration\nIP address: 192.168.200.139\nSubnet mask: 255.255.255.0\nRouter: 192.168.200.1\nClient ID: \nIPv6: Automatic\nIPv6 IP address: none\nIPv6 Router: none\nWi-Fi ID: 7a:41:64:30:df:45\n");
        assert_eq!((found[2].address.as_deref(), found[2].router.as_deref(), found[2].automatic, found[2].connected()), (Some("192.168.200.139"), Some("192.168.200.1"), true, true));
        read_info(&mut found[0], "Manual Configuration\nIP address: \nSubnet mask: \nRouter: \nEthernet Address: 1e:b2:a0:c0:d8:b2\n");
        assert_eq!((found[0].address.clone(), found[0].automatic, found[0].connected()), (None, false, false));
        assert_eq!(parse_ports(""), vec![]);
    }

    #[test]
    fn name_servers_and_wifi_power_are_read() {
        assert_eq!(parse_dns("1.1.1.1\n2606:4700:4700::1111\n"), ["1.1.1.1", "2606:4700:4700::1111"]);
        assert_eq!(parse_dns("There aren't any DNS Servers set on Wi-Fi.\n"), Vec::<String>::new());
        assert_eq!((parse_power("Wi-Fi Power (en0): On\n"), parse_power("Wi-Fi Power (en0): Off"), parse_power("en4 is not a Wi-Fi interface.")), (Some(true), Some(false), None));
        let found = parse_nmcli("wlp3s0:wifi:connected:Home Network\nenp2s0:ethernet:unavailable:\nlo:loopback:unmanaged:\ndocker0:bridge:unmanaged:\n");
        assert_eq!(found.iter().map(|c| (c.name.as_str(), c.device.as_str(), c.wifi)).collect::<Vec<_>>(), [("Wi-Fi: Home Network", "wlp3s0", true), ("Ethernet", "enp2s0", false)]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn this_mac_says_how_it_is_connected() {
        let net = read();
        // Wi-Fi is listed where there is any, connected or not, and nothing unconnected besides.
        assert!(net.connections.iter().all(|c| c.connected() || c.wifi), "{net:?}");
        assert!(net.connections.iter().all(|c| !c.device.is_empty()));
    }
}
