//! Memory pressure, and the energy each process uses.
//!
//! Memory pressure says how hard the system is working to find memory: not
//! how much is used, since a healthy system keeps most of it busy as cache,
//! but how much it is having to compress, swap and reclaim to keep going.
//! Energy is the power each process has drawn since the last sample: what
//! makes a battery run down, and the fans run.

use std::collections::HashMap;
use std::time::Instant;

/// How the system is coping for memory, as Activity Monitor colours it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// Green: memory is being found without trouble.
    Normal,
    /// Yellow: the system is compressing and reclaiming memory to keep up.
    Warning,
    /// Red: it is short, and swapping or ending processes.
    Critical,
}

impl Level {
    pub fn label(self) -> &'static str {
        match self {
            Level::Normal => "Normal",
            Level::Warning => "Elevated",
            Level::Critical => "Critical",
        }
    }
}

/// A reading of memory pressure.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pressure {
    /// How hard pressed, from 0 to 100.
    pub percent: f32,
    pub level: Level,
}

/// Reads PSI's `/proc/pressure/memory`: the share of time in which some
/// task, or every task, was held up waiting for memory, over the last ten
/// seconds. The levels are where waiting starts to be felt, and where
/// everything is waiting.
#[cfg_attr(not(any(target_os = "linux", target_os = "android")), allow(dead_code))]
pub fn parse_psi(text: &str) -> Option<Pressure> {
    let avg10 = |kind: &str| -> Option<f32> { text.lines().find(|l| l.starts_with(kind))?.split_whitespace().find_map(|f| f.strip_prefix("avg10="))?.parse().ok() };
    let (some, full) = (avg10("some")?, avg10("full").unwrap_or(0.0));
    let level = if full >= 10.0 || some >= 40.0 {
        Level::Critical
    } else if some >= 10.0 {
        Level::Warning
    } else {
        Level::Normal
    };
    Some(Pressure { percent: some.clamp(0.0, 100.0), level })
}

/// The system's memory pressure, where it reports one.
pub fn pressure() -> Option<Pressure> {
    imp::pressure()
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;

    fn sysctl(name: &str) -> Option<i32> {
        let name = std::ffi::CString::new(name).ok()?;
        let mut value: i32 = 0;
        let mut size = size_of::<i32>();
        // SAFETY: the buffer is an i32 and its size is passed.
        let ok = unsafe { libc::sysctlbyname(name.as_ptr(), (&raw mut value).cast(), &mut size, std::ptr::null_mut(), 0) } == 0;
        ok.then_some(value)
    }

    /// The same figures Activity Monitor's graph and `memory_pressure` use:
    /// the share of memory the kernel counts as free to give out, and the
    /// level it has declared.
    pub fn pressure() -> Option<Pressure> {
        let free = sysctl("kern.memorystatus_level")?;
        let level = match sysctl("kern.memorystatus_vm_pressure_level").unwrap_or(1) {
            4 => Level::Critical,
            2 => Level::Warning,
            _ => Level::Normal,
        };
        Some(Pressure { percent: (100 - free).clamp(0, 100) as f32, level })
    }

    /// `rusage_info_v6` as far as the energy count, then room to spare:
    /// the kernel fills only what the flavour asks for.
    #[repr(C)]
    struct Rusage {
        uuid: [u8; 16],
        counts: [u64; 64],
    }

    const RUSAGE_INFO_V6: libc::c_int = 6;
    /// Where `ri_energy_nj` falls among the counts: the energy the process
    /// has used since it started, in nanojoules.
    const ENERGY_NJ: usize = 40;

    /// The energy a process has used since it started, in nanojoules. Not
    /// for another user's processes, without permission.
    pub fn energy(pid: u32) -> Option<u64> {
        // SAFETY: the buffer is zeroed and larger than rusage_info_v6.
        let mut info: Rusage = unsafe { std::mem::zeroed() };
        let ok = unsafe { libc::proc_pid_rusage(pid as libc::c_int, RUSAGE_INFO_V6, (&raw mut info).cast::<libc::rusage_info_t>()) } == 0;
        ok.then_some(info.counts[ENERGY_NJ])
    }

    /// No package meter is needed: each process has its own count.
    pub fn package_energy() -> Option<u64> {
        None
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod imp {
    use super::*;

    pub fn pressure() -> Option<Pressure> {
        parse_psi(&std::fs::read_to_string("/proc/pressure/memory").ok()?)
    }

    /// Linux keeps no energy count per process.
    pub fn energy(_pid: u32) -> Option<u64> {
        None
    }

    /// The processor package's energy since some point, in nanojoules,
    /// from Intel's and AMD's RAPL meter. Most systems let only the
    /// administrator read it; then there is none.
    pub fn package_energy() -> Option<u64> {
        let text = std::fs::read_to_string("/sys/class/powercap/intel-rapl:0/energy_uj").ok()?;
        text.trim().parse::<u64>().ok().map(|uj| uj * 1000)
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "android")))]
mod imp {
    use super::*;

    pub fn pressure() -> Option<Pressure> {
        None
    }

    pub fn energy(_pid: u32) -> Option<u64> {
        None
    }

    pub fn package_energy() -> Option<u64> {
        None
    }
}

/// Turns running energy counts into power, between one sample and the next.
#[derive(Default)]
pub struct Meter {
    /// Each process's count at the last sample.
    last: HashMap<u32, u64>,
    /// The package's count at the last sample, where that is all there is.
    package: Option<u64>,
    at: Option<Instant>,
    /// What the last call found, given again when asked too soon after it.
    shown: HashMap<u32, f32>,
}

impl Meter {
    /// The power each process drew since the last call, in watts. `cpu`
    /// is each process's share of the processor over the same time, used
    /// where only the whole package's energy is known: then the figures
    /// are that energy shared out by processor time, an estimate. Empty
    /// the first time, and where neither is to be had.
    pub fn sample(&mut self, cpu: &HashMap<u32, f32>) -> HashMap<u32, f32> {
        let now = Instant::now();
        // Too soon after the last to measure anything: say the same again,
        // and keep counting from where that started.
        if self.at.is_some_and(|t| now.duration_since(t).as_secs_f64() < 0.05) {
            return self.shown.clone();
        }
        let secs = self.at.map(|t| now.duration_since(t).as_secs_f64());
        self.at = Some(now);
        let out = self.measure(cpu, secs);
        self.shown = out.clone();
        out
    }

    fn measure(&mut self, cpu: &HashMap<u32, f32>, secs: Option<f64>) -> HashMap<u32, f32> {
        let mut out = HashMap::new();
        let counts: HashMap<u32, u64> = cpu.keys().filter_map(|&pid| Some((pid, imp::energy(pid)?))).collect();
        if !counts.is_empty() {
            if let Some(secs) = secs {
                for (pid, now) in &counts {
                    // A count that went down is a new process with an old number.
                    if let Some(before) = self.last.get(pid).filter(|b| *b <= now) {
                        out.insert(*pid, ((now - before) as f64 / 1e9 / secs) as f32);
                    }
                }
            }
            self.last = counts;
            return out;
        }
        let package = imp::package_energy();
        if let (Some(now), Some(before), Some(secs)) = (package, self.package, secs) {
            let watts = (now.saturating_sub(before) as f64 / 1e9 / secs) as f32;
            let busy: f32 = cpu.values().sum();
            if busy > 0.0 {
                out = cpu.iter().map(|(pid, share)| (*pid, watts * share / busy)).collect();
            }
        }
        self.package = package;
        out
    }
}

/// Power for a column: milliwatts while small, watts above one.
pub fn watts(w: f32) -> String {
    if w >= 1.0 {
        format!("{w:.1} W")
    } else if w >= 0.0005 {
        format!("{:.0} mW", w * 1000.0)
    } else {
        "0".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressure_stall_figures_are_read_and_graded() {
        let calm = "some avg10=0.00 avg60=0.12 avg300=0.40 total=1234\nfull avg10=0.00 avg60=0.00 avg300=0.10 total=99\n";
        assert_eq!(parse_psi(calm), Some(Pressure { percent: 0.0, level: Level::Normal }));
        let busy = "some avg10=18.50 avg60=6.00 avg300=2.00 total=1\nfull avg10=2.10 avg60=0.40 avg300=0.10 total=1\n";
        assert_eq!(parse_psi(busy), Some(Pressure { percent: 18.5, level: Level::Warning }));
        let stuck = "some avg10=55.00 avg60=30.00 avg300=9.00 total=1\nfull avg10=12.00 avg60=4.00 avg300=1.00 total=1\n";
        assert_eq!(parse_psi(stuck).map(|p| p.level), Some(Level::Critical));
        // An older kernel reports only the `some` line.
        assert_eq!(parse_psi("some avg10=3.00 avg60=1.00 avg300=0.50 total=1\n").map(|p| p.level), Some(Level::Normal));
        assert_eq!(parse_psi(""), None);
        assert_eq!((Level::Normal.label(), Level::Warning.label(), Level::Critical.label()), ("Normal", "Elevated", "Critical"));
    }

    #[test]
    fn power_reads_in_the_units_that_suit_it() {
        assert_eq!((watts(3.346), watts(0.25), watts(0.0012), watts(0.0)), ("3.3 W".into(), "250 mW".into(), "1 mW".into(), "0".into()));
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn this_computer_reports_memory_pressure() {
        // Linux before 4.20, or without PSI built in, has none to report.
        if cfg!(target_os = "linux") && !std::path::Path::new("/proc/pressure/memory").exists() {
            return;
        }
        let p = pressure().expect("a pressure reading");
        assert!((0.0..=100.0).contains(&p.percent), "{p:?}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_busy_process_is_seen_drawing_power_and_an_idle_one_not() {
        let busy = std::process::Command::new("sh").args(["-c", "while :; do :; done"]).spawn().unwrap();
        let idle = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let pids: HashMap<u32, f32> = [(busy.id(), 1.0), (idle.id(), 0.0)].into();
        let mut meter = Meter::default();
        assert!(meter.sample(&pids).is_empty(), "nothing to compare with the first time");
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let power = meter.sample(&pids);
        for mut child in [busy, idle] {
            let _ = child.kill();
            let _ = child.wait();
        }
        let (b, i) = (power[&pids.keys().copied().find(|p| pids[p] == 1.0).unwrap()], power[&pids.keys().copied().find(|p| pids[p] == 0.0).unwrap()]);
        assert!(b > 0.2 && b < 50.0, "a core kept busy draws something like watts: {b}");
        assert!(i < 0.01, "a sleeping process next to nothing: {i}");
        // Another user's process is not counted, rather than counted as nothing.
        assert!(imp::energy(1).is_none());
    }
}
