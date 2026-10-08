//! Whether the computer is short of memory.
//!
//! Apollo's reading keeps a model of a few gigabytes busy in the
//! background. That is fine on a computer with room and one more push
//! over the edge on a computer without, so it is not started, and stops
//! between files, while memory is short.

/// The least share of memory, in percent, that must be free to give out
/// for Apollo to read.
pub const LEAST_FREE: u32 = 15;

/// Reads `/proc/meminfo`: the share of memory that is available, in percent.
#[cfg_attr(not(any(target_os = "linux", target_os = "android")), allow(dead_code))]
pub fn parse_meminfo(text: &str) -> Option<u32> {
    let kb = |name: &str| -> Option<u64> { text.lines().find_map(|l| l.strip_prefix(name))?.trim_start_matches(':').split_whitespace().next()?.parse().ok() };
    let (total, available) = (kb("MemTotal")?, kb("MemAvailable")?);
    (total > 0).then(|| (available * 100 / total) as u32)
}

/// The share of memory the system counts as free to give out, in percent,
/// where it says.
pub fn free_percent() -> Option<u32> {
    imp::free_percent()
}

/// How much memory this computer has, in bytes, where it says.
pub fn total_bytes() -> Option<u64> {
    imp::total_bytes()
}

/// Whether there is too little memory free for Apollo to be reading.
/// Where the system does not say, it is taken that there is enough.
pub fn short_of_memory() -> bool {
    free_percent().is_some_and(|free| free < LEAST_FREE)
}

#[cfg(target_os = "macos")]
mod imp {
    /// The figure behind Activity Monitor's memory pressure graph.
    pub fn free_percent() -> Option<u32> {
        let mut value: i32 = 0;
        let mut size = size_of::<i32>();
        // SAFETY: the buffer is an i32 and its size is passed.
        let ok = unsafe { libc::sysctlbyname(c"kern.memorystatus_level".as_ptr(), (&raw mut value).cast(), &mut size, std::ptr::null_mut(), 0) } == 0;
        ok.then_some(value.clamp(0, 100) as u32)
    }

    pub fn total_bytes() -> Option<u64> {
        let mut value: u64 = 0;
        let mut size = size_of::<u64>();
        // SAFETY: the buffer is a u64 and its size is passed.
        let ok = unsafe { libc::sysctlbyname(c"hw.memsize".as_ptr(), (&raw mut value).cast(), &mut size, std::ptr::null_mut(), 0) } == 0;
        ok.then_some(value)
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod imp {
    pub fn free_percent() -> Option<u32> {
        super::parse_meminfo(&std::fs::read_to_string("/proc/meminfo").ok()?)
    }

    pub fn total_bytes() -> Option<u64> {
        let text = std::fs::read_to_string("/proc/meminfo").ok()?;
        text.lines().find_map(|l| l.strip_prefix("MemTotal:"))?.split_whitespace().next()?.parse::<u64>().ok().map(|kb| kb * 1024)
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "android")))]
mod imp {
    pub fn free_percent() -> Option<u32> {
        None
    }

    pub fn total_bytes() -> Option<u64> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_share_of_memory_available_is_read() {
        assert_eq!(parse_meminfo("MemTotal:       16000000 kB\nMemFree:          200000 kB\nMemAvailable:    4000000 kB\n"), Some(25));
        assert_eq!(parse_meminfo("MemTotal:       16000000 kB\nMemAvailable:     800000 kB\n"), Some(5));
        assert_eq!((parse_meminfo("MemTotal: 0 kB\nMemAvailable: 0 kB\n"), parse_meminfo("")), (None, None));
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn this_computer_says_how_much_memory_is_free() {
        assert!(free_percent().is_some_and(|f| f <= 100));
        assert!(total_bytes().is_some_and(|t| t > 1 << 28));
    }
}
