//! Readings shared by HWiNFO, on Windows.
//!
//! The processor's own sensors, the motherboard's and the fans are read
//! from ring 0, which takes a kernel driver; Neo has none. HWiNFO has, and
//! while it runs it will share what it reads in two ways, either of which
//! is taken here:
//!
//! - Shared memory (`Global\HWiNFO_SENS_SM2`), with "Shared Memory Support"
//!   on in its settings: every sensor it shows. The free version turns
//!   this off again after twelve hours.
//! - The registry (`HKCU\Software\HWiNFO64\VSB`), for each sensor that has
//!   "Report value in Gadget" on: only those chosen, with no limit of time.
//!
//! With HWiNFO not running, or sharing nothing, there is nothing here and
//! the Sensors page shows what Windows gives without it.

use crate::sensors::{Fan, Kind, Sensor};

/// What sort of thing a reading is of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum What {
    Temperature,
    Fan,
    Other,
}

/// One reading as HWiNFO gives it: the sensor it is of, what it is called
/// there, and its value.
#[derive(Clone, Debug, PartialEq)]
pub struct Shared {
    pub sensor: String,
    pub label: String,
    pub what: What,
    pub value: f64,
}

/// Reads what HWiNFO lays out in its shared memory: a header that says
/// where the sensors and the readings are and how long each is, then the
/// two tables. `None` if it is not HWiNFO's, or says it has stopped.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn parse(bytes: &[u8]) -> Option<Vec<Shared>> {
    let number = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
    // "HWiS", as the bytes lie; "DEAD" once HWiNFO has stopped sharing.
    if bytes.get(..4)? != b"SiWH" {
        return None;
    }
    let (sensors_at, sensor_size, sensors) = (number(20)?, number(24)?, number(28)?);
    let (readings_at, reading_size, readings) = (number(32)?, number(36)?, number(40)?);
    if sensor_size < 264 || reading_size < 292 || sensors > 4096 || readings > 65_536 {
        return None;
    }
    // A name of at most 128 bytes, ended by a nul, in the system's own
    // code page: anything past plain letters is kept as best it can be.
    let name = |at: usize| -> Option<String> {
        let field = bytes.get(at..at + 128)?;
        let end = field.iter().position(|b| *b == 0).unwrap_or(field.len());
        Some(field[..end].iter().map(|b| if b.is_ascii() { *b as char } else { '?' }).collect::<String>().trim().to_owned())
    };
    // The name the user gave a thing, if any, and failing that HWiNFO's own.
    let named = |own: usize, given: usize| -> Option<String> { Some(name(given)?).filter(|n| !n.is_empty()).or_else(|| name(own)) };
    let sensor_names: Vec<String> = (0..sensors).map(|i| named(sensors_at + i * sensor_size + 8, sensors_at + i * sensor_size + 136)).collect::<Option<_>>()?;
    let mut out = Vec::with_capacity(readings);
    for i in 0..readings {
        let at = readings_at + i * reading_size;
        let what = match number(at)? {
            1 => What::Temperature,
            3 => What::Fan,
            _ => What::Other,
        };
        let value = f64::from_le_bytes(bytes.get(at + 284..at + 292)?.try_into().ok()?);
        out.push(Shared { sensor: sensor_names.get(number(at + 4)?).cloned().unwrap_or_default(), label: named(at + 12, at + 140)?, what, value });
    }
    Some(out)
}

/// Reads one of the values HWiNFO writes to the registry for a gadget:
/// "45 °C", "1,203 RPM", "62.5 %".
#[cfg_attr(not(windows), allow(dead_code))]
pub fn gadget(sensor: &str, label: &str, value: &str) -> Option<Shared> {
    let value = value.trim();
    let split = value.find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | ',' | '-' | '+'))).unwrap_or(value.len());
    let (number, unit) = (value[..split].replace(',', ""), value[split..].trim().to_lowercase());
    let what = if unit.ends_with("°c") || unit == "c" {
        What::Temperature
    } else if unit == "rpm" {
        What::Fan
    } else {
        What::Other
    };
    Some(Shared { sensor: sensor.trim().to_owned(), label: label.trim().to_owned(), what, value: number.parse().ok()? })
}

/// Which part a temperature is of, from the names HWiNFO gives the sensor
/// and the reading.
fn kind_of(sensor: &str, label: &str) -> Kind {
    let (s, l) = (sensor.to_lowercase(), label.to_lowercase());
    let either = |words: &[&str]| words.iter().any(|w| s.contains(w) || l.contains(w));
    if either(&["gpu"]) {
        Kind::Gpu
    } else if either(&["s.m.a.r.t", "nvme", "ssd", "hdd", "drive"]) {
        Kind::Storage
    } else if either(&["dimm", "spd hub", "memory"]) {
        Kind::Memory
    } else if either(&["battery"]) {
        Kind::Battery
    } else if s.contains("cpu") || l.starts_with("cpu") || l.starts_with("core") || l.starts_with("p-core") || l.starts_with("e-core") {
        Kind::Cpu
    } else {
        Kind::Other
    }
}

/// The temperatures and the fans among what HWiNFO shares, as the Sensors
/// page shows them.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn sorted(shared: Vec<Shared>) -> (Vec<Sensor>, Vec<Fan>) {
    let (mut sensors, mut fans) = (vec![], vec![]);
    for r in shared {
        let label = r.label.to_lowercase();
        match r.what {
            // Not the limits and the room left under them, which are in
            // degrees too and are not how warm anything is.
            What::Temperature if !(label.contains("tjmax") || label.contains("limit") || label.contains("critical")) => {
                let celsius = r.value as f32;
                if (15.0..=130.0).contains(&celsius) {
                    sensors.push(Sensor { kind: kind_of(&r.sensor, &r.label), name: r.label, id: r.sensor, celsius, idle: false });
                }
            }
            // A header with no fan on it reads nothing.
            What::Fan if r.value > 0.0 && r.value < 30_000.0 => fans.push(Fan { name: r.label, rpm: r.value as f32, min: None, max: None }),
            _ => {}
        }
    }
    // By kind, and within a kind as HWiNFO has them, which is core by core.
    sensors.sort_by_key(|s| s.kind);
    (sensors, fans)
}

/// What HWiNFO is sharing just now, if it is running and sharing anything.
#[cfg(windows)]
pub fn read() -> Option<(Vec<Sensor>, Vec<Fan>)> {
    let shared = win::memory().and_then(|bytes| parse(&bytes)).filter(|all| !all.is_empty()).or_else(win::registry)?;
    let found = sorted(shared);
    (!found.0.is_empty() || !found.1.is_empty()).then_some(found)
}

#[cfg(windows)]
mod win {
    use super::Shared;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS};
    use windows_sys::Win32::System::Memory::{FILE_MAP_READ, MEMORY_BASIC_INFORMATION, MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, VirtualQuery};
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// A copy of HWiNFO's shared memory, if there is any.
    pub fn memory() -> Option<Vec<u8>> {
        // SAFETY: the mapping is opened for reading and its view is read
        // only for as far as the system says it reaches, then let go.
        unsafe {
            let mapping = OpenFileMappingW(FILE_MAP_READ, 0, wide(r"Global\HWiNFO_SENS_SM2").as_ptr());
            if mapping.is_null() {
                return None;
            }
            let view = MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 0);
            let mut copy = None;
            if !view.Value.is_null() {
                let mut about: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
                if VirtualQuery(view.Value, &mut about, std::mem::size_of::<MEMORY_BASIC_INFORMATION>()) != 0 && about.RegionSize > 0 {
                    // Far more than HWiNFO ever shares; a size past it is not to be believed.
                    let len = about.RegionSize.min(64 << 20);
                    copy = Some(std::slice::from_raw_parts(view.Value as *const u8, len).to_vec());
                }
                UnmapViewOfFile(view);
            }
            CloseHandle(mapping);
            copy
        }
    }

    /// Whether HWiNFO is running: what it wrote to the registry stays
    /// there after it has gone, and is then of some time ago.
    fn running() -> bool {
        // SAFETY: a snapshot of the processes, walked and closed.
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return false;
            }
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut found = false;
            let mut more = Process32FirstW(snapshot, &mut entry) != 0;
            while more && !found {
                let end = entry.szExeFile.iter().position(|c| *c == 0).unwrap_or(entry.szExeFile.len());
                found = String::from_utf16_lossy(&entry.szExeFile[..end]).to_lowercase().starts_with("hwinfo");
                more = Process32NextW(snapshot, &mut entry) != 0;
            }
            CloseHandle(snapshot);
            found
        }
    }

    fn text(key: &[u16], name: &str) -> Option<String> {
        let mut buffer = [0u16; 512];
        let mut bytes = (buffer.len() * 2) as u32;
        // SAFETY: the buffer is as long as the system is told it is.
        let status = unsafe { RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), wide(name).as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buffer.as_mut_ptr().cast(), &mut bytes) };
        if status != 0 {
            return None;
        }
        let len = (bytes as usize / 2).min(buffer.len());
        let end = buffer[..len].iter().position(|c| *c == 0).unwrap_or(len);
        Some(String::from_utf16_lossy(&buffer[..end]))
    }

    /// The sensors set to report to a gadget, which HWiNFO numbers from nothing.
    pub fn registry() -> Option<Vec<Shared>> {
        if !running() {
            return None;
        }
        let mut out = vec![];
        for key in [r"Software\HWiNFO64\VSB", r"Software\HWiNFO32\VSB"] {
            let key = wide(key);
            for i in 0..512 {
                let (Some(sensor), Some(label), Some(value)) = (text(&key, &format!("Sensor{i}")), text(&key, &format!("Label{i}")), text(&key, &format!("Value{i}"))) else { break };
                out.extend(super::gadget(&sensor, &label, &value));
            }
        }
        (!out.is_empty()).then_some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shared memory as HWiNFO lays it out, with these sensors and readings.
    fn laid_out(sensors: &[&str], readings: &[(u32, u32, &str, f64)]) -> Vec<u8> {
        let (sensor_size, reading_size) = (264usize, 316usize);
        let mut bytes = vec![0u8; 44];
        bytes[..4].copy_from_slice(b"SiWH");
        for (at, n) in [(20, 44), (24, sensor_size), (28, sensors.len()), (32, 44 + sensors.len() * sensor_size), (36, reading_size), (40, readings.len())] {
            bytes[at..at + 4].copy_from_slice(&(n as u32).to_le_bytes());
        }
        for name in sensors {
            let mut one = vec![0u8; sensor_size];
            one[8..8 + name.len()].copy_from_slice(name.as_bytes());
            bytes.extend(one);
        }
        for (what, sensor, label, value) in readings {
            let mut one = vec![0u8; reading_size];
            one[..4].copy_from_slice(&what.to_le_bytes());
            one[4..8].copy_from_slice(&sensor.to_le_bytes());
            one[12..12 + label.len()].copy_from_slice(label.as_bytes());
            one[284..292].copy_from_slice(&value.to_le_bytes());
            bytes.extend(one);
        }
        bytes
    }

    #[test]
    fn what_hwinfo_shares_is_read_and_sorted_into_parts() {
        let bytes = laid_out(
            &["CPU [#0]: Intel Core i7-14700KF: DTS", "MSI PRO Z790-A MAX WIFI (Nuvoton NCT6687D)", "GPU [#0]: NVIDIA GeForce RTX 5070", "S.M.A.R.T.: Samsung SSD 990 PRO"],
            &[(1, 0, "Core 0", 41.0), (1, 0, "Core 0 Distance to TjMAX", 59.0), (1, 0, "CPU Package", 47.0), (1, 1, "System", 33.0), (1, 1, "CPU", 44.5), (3, 1, "CPU Fan", 812.0), (3, 1, "Pump Fan", 0.0), (2, 1, "Vcore", 1.21), (1, 2, "GPU Temperature", 43.0), (1, 3, "Drive Temperature", 38.0)],
        );
        let (sensors, fans) = sorted(parse(&bytes).unwrap());
        assert_eq!(
            sensors.iter().map(|s| (s.kind, s.name.as_str(), s.celsius)).collect::<Vec<_>>(),
            vec![(Kind::Cpu, "Core 0", 41.0), (Kind::Cpu, "CPU Package", 47.0), (Kind::Cpu, "CPU", 44.5), (Kind::Gpu, "GPU Temperature", 43.0), (Kind::Storage, "Drive Temperature", 38.0), (Kind::Other, "System", 33.0)],
            "the room left under a limit is not a temperature, and a voltage is not either"
        );
        assert_eq!(fans.iter().map(|f| (f.name.as_str(), f.rpm)).collect::<Vec<_>>(), vec![("CPU Fan", 812.0)], "a header with nothing on it is left out");
        // Stopped, or not HWiNFO's at all, or cut short: nothing, and no panic.
        let mut dead = bytes.clone();
        dead[..4].copy_from_slice(b"DAED");
        assert!(parse(&dead).is_none() && parse(&bytes[..300]).is_none() && parse(&[]).is_none());
    }

    #[test]
    fn a_gadget_value_is_read_with_its_unit() {
        let read = |value: &str| gadget("CPU [#0]: Intel Core i7-14700KF: DTS", "Core Max", value).map(|s| (s.what, s.value));
        assert_eq!(read("45 °C"), Some((What::Temperature, 45.0)));
        assert_eq!(read("1,203 RPM"), Some((What::Fan, 1203.0)));
        assert_eq!(read("62.5 %"), Some((What::Other, 62.5)));
        assert_eq!(read("Yes"), None);
    }
}
