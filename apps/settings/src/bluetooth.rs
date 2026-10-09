//! Bluetooth: on or off, and the devices this computer knows.
//!
//! macOS says what it knows through `system_profiler`; turning Bluetooth
//! on and off goes through the same calls its own settings use. Linux is
//! asked through BlueZ's `bluetoothctl`.

use std::process::{Command, Stdio};

/// A device this computer has been paired with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub name: String,
    /// What kind of thing it is, where it says: "Headphones", "Keyboard".
    pub kind: String,
    pub connected: bool,
    /// How full its battery is, where it says: "80%".
    pub battery: Option<String>,
}

/// The state of Bluetooth.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bluetooth {
    /// Whether it is turned on. `None` where the computer has none, or
    /// does not say.
    pub on: Option<bool>,
    /// Connected ones first, then by name.
    pub devices: Vec<Device>,
}

/// Reads `system_profiler SPBluetoothDataType -json`.
pub fn parse_profiler(text: &str) -> Bluetooth {
    let Ok(all) = serde_json::from_str::<serde_json::Value>(text) else { return Bluetooth::default() };
    let Some(here) = all["SPBluetoothDataType"].get(0) else { return Bluetooth::default() };
    let on = here["controller_properties"]["controller_state"].as_str().map(|s| s == "attrib_on");
    let mut devices = vec![];
    for (key, connected) in [("device_connected", true), ("device_not_connected", false)] {
        // A list of objects, each with one device under its name.
        for entry in here[key].as_array().into_iter().flatten() {
            for (name, about) in entry.as_object().into_iter().flatten() {
                // The fullest account of its battery that there is: one figure, or each earbud's and the case's.
                let battery = ["device_batteryLevelMain", "device_batteryLevel"].iter().find_map(|k| about[*k].as_str()).map(str::to_owned).or_else(|| {
                    let parts: Vec<String> = [("device_batteryLevelLeft", "left"), ("device_batteryLevelRight", "right"), ("device_batteryLevelCase", "case")].iter().filter_map(|(k, what)| about[*k].as_str().map(|v| format!("{what} {v}"))).collect();
                    (!parts.is_empty()).then(|| parts.join(", "))
                });
                devices.push(Device { name: name.clone(), kind: about["device_minorType"].as_str().unwrap_or_default().to_owned(), connected, battery });
            }
        }
    }
    devices.sort_by(|a, b| b.connected.cmp(&a.connected).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Bluetooth { on, devices }
}

/// Reads `bluetoothctl devices`: `Device AA:BB:CC:DD:EE:FF Name of It`.
#[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]
pub fn parse_bluetoothctl(devices: &str, connected: &str) -> Vec<Device> {
    let named = |text: &str| -> Vec<(String, String)> { text.lines().filter_map(|l| l.strip_prefix("Device ")).filter_map(|l| l.split_once(' ')).map(|(address, name)| (address.to_owned(), name.trim().to_owned())).collect() };
    let on: Vec<String> = named(connected).into_iter().map(|(address, _)| address).collect();
    let mut out: Vec<Device> = named(devices).into_iter().map(|(address, name)| Device { name, kind: String::new(), connected: on.contains(&address), battery: None }).collect();
    out.sort_by(|a, b| b.connected.cmp(&a.connected).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    out
}

fn run(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(neo_desktop::fs::tool(program)).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Asks the system about Bluetooth. It takes a second or two: to be
/// called off the main thread.
pub fn read() -> Bluetooth {
    if cfg!(target_os = "macos") {
        return run("/usr/sbin/system_profiler", &["SPBluetoothDataType", "-json"]).map(|t| parse_profiler(&t)).unwrap_or_default();
    }
    if cfg!(unix) {
        let Some(shown) = run("bluetoothctl", &["show"]) else { return Bluetooth::default() };
        let on = shown.lines().find_map(|l| l.trim().strip_prefix("Powered: ")).map(|v| v.trim() == "yes");
        let devices = parse_bluetoothctl(&run("bluetoothctl", &["devices"]).unwrap_or_default(), &run("bluetoothctl", &["devices", "Connected"]).unwrap_or_default());
        return Bluetooth { on, devices };
    }
    Bluetooth::default()
}

/// Turns Bluetooth on or off. False if it could not be asked.
pub fn set_power(on: bool) -> bool {
    imp::set_power(on)
}

#[cfg(target_os = "macos")]
mod imp {
    unsafe extern "C" {
        fn neo_bluetooth_power() -> std::ffi::c_int;
        fn neo_bluetooth_set_power(on: std::ffi::c_int);
    }

    pub fn set_power(on: bool) -> bool {
        // SAFETY: plain calls with a number. The system takes a moment to do
        // as it is told, so what it then says is not waited on here.
        unsafe { neo_bluetooth_set_power(i32::from(on)) };
        true
    }

    /// Whether Bluetooth is on, asked of the system directly.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn power() -> bool {
        // SAFETY: a plain call.
        unsafe { neo_bluetooth_power() != 0 }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    pub fn set_power(on: bool) -> bool {
        super::run("bluetoothctl", &["power", if on { "on" } else { "off" }]).is_some()
    }
}

#[cfg(not(unix))]
mod imp {
    pub fn set_power(_on: bool) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_the_system_knows_of_bluetooth_is_read() {
        let text = r#"{"SPBluetoothDataType":[{"controller_properties":{"controller_state":"attrib_on","controller_address":"AA:BB"},
            "device_connected":[{"Magic Keyboard":{"device_address":"1","device_minorType":"Keyboard","device_batteryLevelMain":"80%"}},{"AirPods Pro":{"device_minorType":"Headphones","device_batteryLevelLeft":"90%","device_batteryLevelRight":"85%","device_batteryLevelCase":"40%"}}],
            "device_not_connected":[{"Zed Speaker":{"device_address":"3"}},{"an old mouse":{"device_minorType":"Mouse"}}]}]}"#;
        let found = parse_profiler(text);
        assert_eq!(found.on, Some(true));
        assert_eq!(found.devices.iter().map(|d| (d.name.as_str(), d.connected)).collect::<Vec<_>>(), [("AirPods Pro", true), ("Magic Keyboard", true), ("an old mouse", false), ("Zed Speaker", false)], "connected first, then by name");
        assert_eq!((found.devices[0].battery.as_deref(), found.devices[1].battery.as_deref(), found.devices[1].kind.as_str(), found.devices[3].kind.as_str()), (Some("left 90%, right 85%, case 40%"), Some("80%"), "Keyboard", ""));
        // Off, with nothing known; and a computer with none.
        assert_eq!(parse_profiler(r#"{"SPBluetoothDataType":[{"controller_properties":{"controller_state":"attrib_off"}}]}"#), Bluetooth { on: Some(false), devices: vec![] });
        assert_eq!((parse_profiler(r#"{"SPBluetoothDataType":[]}"#), parse_profiler("not json")), (Bluetooth::default(), Bluetooth::default()));
        let linux = parse_bluetoothctl("Device AA:BB:CC:DD:EE:01 Sony WH-1000XM4\nDevice AA:BB:CC:DD:EE:02 Keychron K2\n", "Device AA:BB:CC:DD:EE:02 Keychron K2\n");
        assert_eq!(linux.iter().map(|d| (d.name.as_str(), d.connected)).collect::<Vec<_>>(), [("Keychron K2", true), ("Sony WH-1000XM4", false)]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn this_mac_says_whether_bluetooth_is_on_both_ways_alike() {
        // What the system's own report says and what it says when asked directly agree;
        // and setting it to what it is changes nothing.
        let reported = read();
        let asked = imp::power();
        if let Some(on) = reported.on {
            assert_eq!(on, asked);
            assert!(set_power(asked));
            assert_eq!(imp::power(), asked);
        }
    }
}
