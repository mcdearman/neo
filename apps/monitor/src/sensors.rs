//! Hardware temperatures and fan speeds.
//!
//! - macOS: the System Management Controller, using the sensor keys known
//!   for each chip family, with IOKit's generic thermal sensors as the
//!   fallback. Fans come from the controller too.
//! - Linux: the kernel's hwmon sensors, for temperatures (through `sysinfo`)
//!   and fans (`/sys/class/hwmon`).
//! - Windows: whatever thermal zones the firmware reports. Fans are not
//!   available without a vendor driver.
//!
//! Reading takes tens of milliseconds, so it is done off the main thread.

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Cpu,
    Gpu,
    Memory,
    Storage,
    Battery,
    Other,
}

impl Kind {
    pub const SUMMARY: [Kind; 4] = [Kind::Cpu, Kind::Gpu, Kind::Memory, Kind::Storage];
    pub const ALL: [Kind; 6] = [Kind::Cpu, Kind::Gpu, Kind::Memory, Kind::Storage, Kind::Battery, Kind::Other];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Cpu => "CPU",
            Kind::Gpu => "GPU",
            Kind::Memory => "Memory",
            Kind::Storage => "Storage",
            Kind::Battery => "Battery",
            Kind::Other => "Other",
        }
    }

    /// Temperatures at which a part is warm, then hot, in °C. Processors
    /// run far hotter than batteries or drives are comfortable at.
    pub fn limits(self) -> (f32, f32) {
        match self {
            Kind::Cpu | Kind::Gpu => (85.0, 100.0),
            Kind::Memory => (70.0, 85.0),
            Kind::Storage => (60.0, 70.0),
            Kind::Battery => (40.0, 45.0),
            Kind::Other => (70.0, 85.0),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sensor {
    pub kind: Kind,
    pub name: String,
    /// The system's own name for the sensor, shown beside ours.
    pub id: String,
    pub celsius: f32,
    /// The sensor has nothing to report right now, and `celsius` means
    /// nothing. Apple Silicon's per-core sensors do this while the chip is
    /// idle, reading a few degrees above zero.
    pub idle: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Fan {
    pub name: String,
    pub rpm: f32,
    pub min: Option<f32>,
    pub max: Option<f32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reading {
    pub sensors: Vec<Sensor>,
    pub fans: Vec<Fan>,
}

impl Reading {
    /// Every sensor of a kind, idle or not.
    pub fn of(&self, kind: Kind) -> impl Iterator<Item = &Sensor> {
        self.sensors.iter().filter(move |s| s.kind == kind)
    }

    /// The sensors of a kind that have a reading.
    pub fn awake(&self, kind: Kind) -> impl Iterator<Item = &Sensor> {
        self.of(kind).filter(|s| !s.idle)
    }

    /// The hottest sensor of a kind, among those with a reading.
    pub fn hottest(&self, kind: Kind) -> Option<f32> {
        self.awake(kind).map(|s| s.celsius).reduce(f32::max)
    }

    /// A temperature to show for the processor when no core sensor is
    /// reporting: the power-management chips beside it, which always do.
    pub fn beside_cpu(&self) -> Option<f32> {
        self.sensors.iter().find(|s| s.id == POWER_CHIP_ID).map(|s| s.celsius)
    }
}

/// The ID of the summary sensor for Apple Silicon's power-management chips.
pub const POWER_CHIP_ID: &str = "PMU tdie";

/// Values outside this range are not real temperatures. Nothing in a
/// running computer is colder than the room it sits in. Sensors with
/// nothing to report, such as Apple Silicon's per-core sensors while the
/// chip is idle, read a few degrees above zero.
fn plausible(celsius: f32) -> bool {
    (15.0..=130.0).contains(&celsius)
}

/// Sorts a generic sensor label, from hwmon or IOKit, into a kind.
fn classify(label: &str) -> Kind {
    let l = label.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| l.contains(w));
    if has(&["gpu", "amdgpu", "radeon", "nouveau", "nvidia", "i915"]) {
        Kind::Gpu
    } else if has(&["nvme", "nand", "drivetemp", "ssd", "sata", "hdd"]) {
        Kind::Storage
    } else if has(&["spd5118", "jc42", "dimm", "memory"]) {
        Kind::Memory
    } else if has(&["battery", "bat0", "bat1"]) {
        Kind::Battery
    } else if has(&["coretemp", "k10temp", "zenpower", "cpu", "core ", "package", "tctl", "tdie", "pacc", "eacc", "soc"]) {
        Kind::Cpu
    } else {
        Kind::Other
    }
}

/// Temperatures from `sysinfo`: hwmon on Linux, IOKit on macOS, WMI on Windows.
fn generic(components: &mut sysinfo::Components) -> Vec<Sensor> {
    components.refresh(true);
    let mut out: Vec<Sensor> = components
        .iter()
        .filter_map(|c| {
            let celsius = c.temperature().filter(|t| plausible(*t))?;
            let label = c.label().trim().to_string();
            Some(Sensor { kind: classify(&label), name: label.clone(), id: label, celsius, idle: false })
        })
        .collect();
    out.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.name.cmp(&b.name)));
    out
}

pub struct Reader {
    components: sysinfo::Components,
    #[cfg(target_os = "macos")]
    smc: Option<crate::smc::Smc>,
    #[cfg(target_os = "macos")]
    keys: Vec<(Kind, String, &'static str)>,
}

#[cfg(target_os = "macos")]
mod mac {
    use super::Kind;

    /// Sensor keys by chip family: performance cores, efficiency cores, GPU,
    /// memory. Apple does not document them; these are the sets the
    /// community has mapped.
    type Family = (&'static [&'static str], &'static [&'static str], &'static [&'static str], &'static [&'static str]);

    fn family(brand: &str) -> Option<Family> {
        let b = brand.to_lowercase();
        Some(if b.contains("m1") {
            (&["Tp01", "Tp05", "Tp0D", "Tp0H", "Tp0L", "Tp0P", "Tp0X", "Tp0b"], &["Tp09", "Tp0T"], &["Tg05", "Tg0D", "Tg0L", "Tg0T"], &["Tm02", "Tm06", "Tm08", "Tm09"])
        } else if b.contains("m2") {
            (&["Tp01", "Tp05", "Tp09", "Tp0D", "Tp0X", "Tp0b", "Tp0f", "Tp0j"], &["Tp1h", "Tp1t", "Tp1p", "Tp1l"], &["Tg0f", "Tg0j"], &[])
        } else if b.contains("m3") {
            (&["Tf04", "Tf09", "Tf0A", "Tf0B", "Tf0D", "Tf0E", "Tf44", "Tf49", "Tf4A", "Tf4B", "Tf4D", "Tf4E"], &["Te05", "Te0L", "Te0P", "Te0S"], &["Tf14", "Tf18", "Tf19", "Tf1A", "Tf24", "Tf28", "Tf29", "Tf2A"], &[])
        } else if b.contains("m4") {
            (&["Tp01", "Tp05", "Tp09", "Tp0D", "Tp0V", "Tp0Y", "Tp0b", "Tp0e"], &["Te05", "Te0S", "Te09", "Te0H"], &["Tg0G", "Tg0H", "Tg1U", "Tg1k", "Tg0K", "Tg0L", "Tg0d", "Tg0e", "Tg0j", "Tg0k"], &[])
        } else if b.contains("intel") {
            (&["TC0P", "TC0D", "TC0E", "TC0F", "TC1C", "TC2C", "TC3C", "TC4C", "TC5C", "TC6C", "TC7C", "TC8C"], &[], &["TG0P", "TG0D", "TG1D"], &["TM0P", "Tm0P", "TM0S"])
        } else {
            return None;
        })
    }

    /// The keys to read on this Mac, with names, or nothing for a chip
    /// without a table.
    pub fn keys(brand: &str) -> Vec<(Kind, String, &'static str)> {
        let Some((performance, efficiency, gpu, memory)) = family(brand) else { return vec![] };
        let intel = brand.to_lowercase().contains("intel");
        let mut out = Vec::new();
        let mut group = |kind: Kind, label: &str, keys: &[&'static str]| {
            for (i, key) in keys.iter().enumerate() {
                out.push((kind, if keys.len() == 1 { label.to_string() } else { format!("{label} {}", i + 1) }, *key));
            }
        };
        group(Kind::Cpu, if intel { "CPU sensor" } else { "Performance core" }, performance);
        group(Kind::Cpu, "Efficiency core", efficiency);
        group(Kind::Gpu, "GPU", gpu);
        group(Kind::Memory, "Memory", memory);
        group(Kind::Storage, "SSD", if intel { &["TH0P", "TH0A", "TH0B"] } else { &["TH0x"] });
        group(Kind::Battery, "Battery", &["TB1T", "TB2T"]);
        group(Kind::Other, "Airflow left", &["TaLP"]);
        group(Kind::Other, "Airflow right", &["TaRF"]);
        group(Kind::Other, "Wi-Fi module", &["TW0P"]);
        group(Kind::Other, "Palm rest", &["Ts0P", "Ts1P"]);
        out
    }
}

impl Reader {
    /// `cpu_brand` is the processor's name, which picks the sensor table on macOS.
    #[cfg_attr(not(target_os = "macos"), allow(unused_variables))]
    pub fn new(cpu_brand: &str) -> Self {
        Self {
            components: sysinfo::Components::new_with_refreshed_list(),
            #[cfg(target_os = "macos")]
            smc: crate::smc::Smc::open(),
            #[cfg(target_os = "macos")]
            keys: mac::keys(cpu_brand),
        }
    }

    pub fn read(&mut self) -> Reading {
        Reading { sensors: self.temperatures(), fans: self.fans() }
    }

    fn temperatures(&mut self) -> Vec<Sensor> {
        #[cfg(target_os = "macos")]
        if let Some(smc) = &self.smc {
            // A core's key that reads a few degrees has nothing to report: the
            // chip is idle. It is kept, marked idle, so the list does not
            // change length with load.
            let mut named: Vec<Sensor> = self
                .keys
                .iter()
                .filter_map(|(kind, name, key)| {
                    let celsius = smc.number(key)?;
                    let idle = !plausible(celsius);
                    (!idle || matches!(kind, Kind::Cpu | Kind::Gpu)).then(|| Sensor { kind: *kind, name: name.clone(), id: key.to_string(), celsius, idle })
                })
                .collect();
            // A chip with no table, or a table that matched nothing, falls back.
            if named.iter().any(|s| s.kind == Kind::Cpu) {
                // The power-management chips beside the processor are always
                // reporting, so they give a reading even when no core sensor is.
                self.components.refresh(true);
                let dies: Vec<f32> = self.components.iter().filter(|c| c.label().starts_with(POWER_CHIP_ID)).filter_map(|c| c.temperature()).filter(|t| plausible(*t)).collect();
                if let Some(hottest) = dies.iter().copied().reduce(f32::max) {
                    named.push(Sensor { kind: Kind::Other, name: format!("Power chips beside the processor, hottest of {}", dies.len()), id: POWER_CHIP_ID.into(), celsius: hottest, idle: false });
                }
                return named;
            }
        }
        generic(&mut self.components)
    }

    #[cfg(target_os = "macos")]
    fn fans(&self) -> Vec<Fan> {
        let Some(smc) = &self.smc else { return vec![] };
        let count = smc.number("FNum").unwrap_or(0.0) as usize;
        (0..count.min(8))
            .filter_map(|i| {
                let rpm = smc.number(&format!("F{i}Ac"))?;
                Some(Fan { name: format!("Fan {}", i + 1), rpm, min: smc.number(&format!("F{i}Mn")), max: smc.number(&format!("F{i}Mx")) })
            })
            .collect()
    }

    /// Fans from hwmon: `fanN_input` in revolutions a minute, with optional
    /// label, minimum and maximum beside it.
    #[cfg(all(unix, not(target_os = "macos")))]
    fn fans(&self) -> Vec<Fan> {
        let mut out = Vec::new();
        let Ok(chips) = std::fs::read_dir("/sys/class/hwmon") else { return out };
        let mut chips: Vec<_> = chips.flatten().map(|e| e.path()).collect();
        chips.sort();
        for chip in chips {
            let read = |file: String| std::fs::read_to_string(chip.join(file)).ok().map(|s| s.trim().to_string());
            let chip_name = read("name".into()).unwrap_or_default();
            for n in 1..=12 {
                let Some(rpm) = read(format!("fan{n}_input")).and_then(|v| v.parse::<f32>().ok()) else { continue };
                let name = read(format!("fan{n}_label")).filter(|l| !l.is_empty()).unwrap_or_else(|| format!("{chip_name} fan {n}"));
                let number = |suffix: &str| read(format!("fan{n}_{suffix}")).and_then(|v| v.parse::<f32>().ok()).filter(|v| *v > 0.0);
                out.push(Fan { name, rpm, min: number("min"), max: number("max") });
            }
        }
        out
    }

    #[cfg(not(unix))]
    fn fans(&self) -> Vec<Fan> {
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_generic_labels_into_kinds() {
        for (label, kind) in [("coretemp Core 0", Kind::Cpu), ("k10temp Tctl", Kind::Cpu), ("PMU tdie3", Kind::Cpu), ("amdgpu edge", Kind::Gpu), ("GPU MTR Temp Sensor1", Kind::Gpu), ("nvme Composite", Kind::Storage), ("NAND CH0 temp", Kind::Storage), ("spd5118 temp1", Kind::Memory), ("gas gauge battery", Kind::Battery), ("acpitz temp1", Kind::Other)] {
            assert_eq!(classify(label), kind, "{label}");
        }
    }

    #[test]
    fn finds_the_hottest_of_a_kind() {
        let s = |kind, celsius| Sensor { kind, name: String::new(), id: String::new(), celsius, idle: false };
        let r = Reading { sensors: vec![s(Kind::Cpu, 61.0), s(Kind::Cpu, 84.5), s(Kind::Gpu, 70.0)], fans: vec![] };
        assert_eq!(r.hottest(Kind::Cpu), Some(84.5));
        // Idle sensors do not count, and all idle means no reading, not 7 °C.
        let idle = Reading { sensors: vec![Sensor { idle: true, ..s(Kind::Cpu, 6.7) }, Sensor { id: POWER_CHIP_ID.into(), ..s(Kind::Other, 44.0) }], fans: vec![] };
        assert_eq!(idle.hottest(Kind::Cpu), None);
        assert_eq!(idle.beside_cpu(), Some(44.0));
        assert_eq!(r.hottest(Kind::Memory), None, "no sensor is not zero degrees");
        assert!(!plausible(0.0) && !plausible(200.0) && plausible(45.0));
        assert!(!plausible(6.7), "a powered-down core's sensor is not a temperature");
    }

    /// Reads the real hardware, so it only checks what any computer has.
    #[test]
    fn reads_this_computer() {
        let brand = {
            let mut s = sysinfo::System::new();
            s.refresh_cpu_all();
            s.cpus().first().map(|c| c.brand().to_string()).unwrap_or_default()
        };
        let reading = Reader::new(&brand).read();
        assert!(reading.sensors.iter().all(|s| s.idle || plausible(s.celsius)));
        assert!(reading.fans.iter().all(|f| f.rpm >= 0.0 && f.rpm < 30_000.0));
        println!("{brand}: {} sensors, {} fans", reading.sensors.len(), reading.fans.len());
        for s in &reading.sensors {
            println!("  {:?} {} ({}) {}", s.kind, s.name, s.id, if s.idle { "idle".to_string() } else { format!("{:.1}", s.celsius) });
        }
        for f in &reading.fans {
            println!("  {} {:.0} rpm ({:?}–{:?})", f.name, f.rpm, f.min, f.max);
        }
    }
}
