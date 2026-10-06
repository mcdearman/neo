//! Per-process details the `sysinfo` crate does not report: a process's
//! threads, and how much of its memory has been swapped out.
//!
//! Threads share their process's memory, so memory and swap are properties
//! of the process; no operating system accounts for them per thread.

use std::collections::HashMap;
use std::time::Duration;

// Windows does not report a thread's state.
#[cfg_attr(windows, allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadState {
    Running,
    /// Waiting for something: a lock, input, a timer.
    Waiting,
    /// Waiting and not interruptible, usually for disk.
    Blocked,
    Stopped,
    Unknown,
}

impl ThreadState {
    pub fn label(self) -> &'static str {
        match self {
            ThreadState::Running => "Running",
            ThreadState::Waiting => "Waiting",
            ThreadState::Blocked => "Blocked",
            ThreadState::Stopped => "Stopped",
            ThreadState::Unknown => "—",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ThreadInfo {
    pub id: u64,
    /// Empty when the program has not named the thread.
    pub name: String,
    pub state: ThreadState,
    /// Processor time used since the thread started, user plus system.
    pub cpu_time: Duration,
}

/// Why a process's threads could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Denied {
    /// The process belongs to another user or the system.
    Permission,
    /// The process has exited.
    Gone,
    /// This platform has no implementation.
    #[cfg_attr(any(target_os = "linux", target_os = "android", target_os = "macos", windows), allow(dead_code))]
    Unsupported,
}

/// Counts that are cheap enough to read for every process on each sample.
#[derive(Clone, Copy, Debug, Default)]
pub struct Quick {
    pub threads: Option<u32>,
    /// Swapped-out memory in bytes, where the system reports it cheaply.
    pub swap: Option<u64>,
    /// All the memory the process is charged with, in bytes, counting what
    /// has been compressed or swapped out as well as what is in RAM. Where
    /// the system keeps this figure it is the one to show as "memory":
    /// an idle process can have nearly everything outside RAM.
    pub footprint: Option<u64>,
}

pub use imp::{quick, swap, threads};

/// Whether [`quick`] reports swap, so the process table can show a column.
pub const QUICK_SWAP: bool = cfg!(any(target_os = "linux", target_os = "android"));

#[cfg(any(target_os = "linux", target_os = "android"))]
mod imp {
    use super::*;

    fn status_kb(status: &str, key: &str) -> Option<u64> {
        status.lines().find_map(|l| l.strip_prefix(key)).and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
    }

    pub fn quick(pids: &[u32]) -> HashMap<u32, Quick> {
        pids.iter()
            .filter_map(|&pid| {
                let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
                Some((pid, Quick { threads: status_kb(&status, "Threads:").map(|n| n as u32), swap: status_kb(&status, "VmSwap:").map(|kb| kb * 1024), footprint: None }))
            })
            .collect()
    }

    /// Swapped-out bytes of one process, from `VmSwap`.
    pub fn swap(pid: u32) -> Option<u64> {
        let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
        status_kb(&status, "VmSwap:").map(|kb| kb * 1024)
    }

    /// Parses `/proc/<pid>/task/<tid>/stat`: `tid (name) state ... utime stime ...`.
    /// The name can contain spaces and brackets, so split at the last `)`.
    pub(super) fn parse_stat(id: u64, stat: &str, ticks_per_sec: u64) -> Option<ThreadInfo> {
        let open = stat.find('(')?;
        let close = stat.rfind(')')?;
        let name = stat.get(open + 1..close)?.to_string();
        let rest: Vec<&str> = stat.get(close + 1..)?.split_whitespace().collect();
        let state = match *rest.first()? {
            "R" => ThreadState::Running,
            "S" | "I" => ThreadState::Waiting,
            "D" => ThreadState::Blocked,
            "T" | "t" => ThreadState::Stopped,
            _ => ThreadState::Unknown,
        };
        // Fields 14 and 15 of the whole line are utime and stime, in clock ticks.
        let ticks: u64 = rest.get(11)?.parse::<u64>().ok()? + rest.get(12)?.parse::<u64>().ok()?;
        Some(ThreadInfo { id, name, state, cpu_time: Duration::from_nanos(ticks * 1_000_000_000 / ticks_per_sec.max(1)) })
    }

    pub fn threads(pid: u32) -> Result<Vec<ThreadInfo>, Denied> {
        let dir = std::fs::read_dir(format!("/proc/{pid}/task")).map_err(|e| if e.kind() == std::io::ErrorKind::PermissionDenied { Denied::Permission } else { Denied::Gone })?;
        // SAFETY: sysconf has no preconditions.
        let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as u64;
        let mut out = Vec::new();
        for entry in dir.flatten() {
            let Some(id) = entry.file_name().to_str().and_then(|n| n.parse::<u64>().ok()) else { continue };
            // A thread can exit between listing and reading; skip it.
            if let Ok(stat) = std::fs::read_to_string(entry.path().join("stat"))
                && let Some(t) = parse_stat(id, &stat, ticks)
            {
                out.push(t);
            }
        }
        Ok(out)
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use std::ffi::c_void;

    /// `PROC_PIDLISTTHREADIDS`: the system-wide IDs of a process's threads.
    const LIST_THREAD_IDS: libc::c_int = 28;
    /// `PROC_PIDTHREADID64INFO`: a `proc_threadinfo` looked up by thread ID.
    const THREAD_ID_INFO: libc::c_int = 15;
    /// `PROC_PIDREGIONINFO`: the memory region at or after an address.
    const REGION_INFO: libc::c_int = 7;

    // Share modes from `<mach/vm_region.h>`.
    const SM_COW: u32 = 1;
    const SM_PRIVATE: u32 = 2;
    const SM_PRIVATE_ALIASED: u32 = 6;

    /// `struct proc_regioninfo` from `<sys/proc_info.h>`.
    #[repr(C)]
    #[derive(Default)]
    struct RegionInfo {
        protection: u32,
        max_protection: u32,
        inheritance: u32,
        flags: u32,
        offset: u64,
        behavior: u32,
        user_wired_count: u32,
        user_tag: u32,
        pages_resident: u32,
        pages_shared_now_private: u32,
        pages_swapped_out: u32,
        pages_dirtied: u32,
        ref_count: u32,
        shadow_depth: u32,
        share_mode: u32,
        private_pages_resident: u32,
        shared_pages_resident: u32,
        obj_id: u32,
        depth: u32,
        address: u64,
        size: u64,
    }

    fn denied() -> Denied {
        match std::io::Error::last_os_error().raw_os_error() {
            Some(libc::ESRCH) => Denied::Gone,
            _ => Denied::Permission,
        }
    }

    pub fn quick(pids: &[u32]) -> HashMap<u32, Quick> {
        let size = size_of::<libc::proc_taskinfo>() as libc::c_int;
        pids.iter()
            .filter_map(|&pid| {
                // SAFETY: the buffer is a zeroed proc_taskinfo of the size passed.
                let mut info: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
                let n = unsafe { libc::proc_pidinfo(pid as libc::c_int, libc::PROC_PIDTASKINFO, 0, (&raw mut info).cast::<c_void>(), size) };
                let threads = (n == size).then_some(info.pti_threadnum as u32);
                let footprint = footprint(pid);
                // Another user's process allows neither without permission.
                (threads.is_some() || footprint.is_some()).then_some((pid, Quick { threads, swap: None, footprint }))
            })
            .collect()
    }

    /// The process's physical footprint: the figure Activity Monitor shows
    /// as Memory. Unlike the resident size it includes memory the system
    /// has compressed or swapped out.
    pub fn footprint(pid: u32) -> Option<u64> {
        // SAFETY: the buffer is a zeroed rusage_info_v2, which is what this flavour fills.
        let mut info: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
        let ok = unsafe { libc::proc_pid_rusage(pid as libc::c_int, libc::RUSAGE_INFO_V2, (&raw mut info).cast::<libc::rusage_info_t>()) } == 0;
        ok.then_some(info.ri_phys_footprint)
    }

    /// Bytes of the process's private memory held in the compressor or in
    /// swap files, measured before compression.
    ///
    /// macOS compresses memory before swapping it and does not report the
    /// two separately per process. This walks every memory region, which
    /// takes up to a few hundred milliseconds for a large process, so call
    /// it off the main thread and for one process at a time.
    pub fn swap(pid: u32) -> Option<u64> {
        let size = size_of::<RegionInfo>() as libc::c_int;
        let mut address = 0u64;
        let mut pages = 0u64;
        let mut found = false;
        loop {
            let mut info = RegionInfo::default();
            // SAFETY: the buffer is a RegionInfo of the size passed.
            let n = unsafe { libc::proc_pidinfo(pid as libc::c_int, REGION_INFO, address, (&raw mut info).cast::<c_void>(), size) };
            if n != size {
                break;
            }
            found = true;
            // Count the process's own memory. Shared regions, such as system
            // libraries, would be counted again in every process that maps them.
            if matches!(info.share_mode, SM_COW | SM_PRIVATE | SM_PRIVATE_ALIASED) {
                pages += info.pages_swapped_out as u64;
            }
            let next = info.address.saturating_add(info.size);
            if next <= address {
                break;
            }
            address = next;
        }
        // SAFETY: sysconf has no preconditions.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(1) as u64;
        found.then_some(pages * page)
    }

    pub fn threads(pid: u32) -> Result<Vec<ThreadInfo>, Denied> {
        let pid = pid as libc::c_int;
        // Leave room for threads that start between sizing and listing.
        let mut ids = vec![0u64; 64];
        let listed = loop {
            let bytes = (ids.len() * size_of::<u64>()) as libc::c_int;
            // SAFETY: the buffer holds `bytes` bytes.
            let n = unsafe { libc::proc_pidinfo(pid, LIST_THREAD_IDS, 0, ids.as_mut_ptr().cast::<c_void>(), bytes) };
            if n <= 0 {
                return Err(denied());
            }
            if n < bytes {
                break n as usize / size_of::<u64>();
            }
            ids.resize(ids.len() * 2, 0);
        };
        let size = size_of::<libc::proc_threadinfo>() as libc::c_int;
        let mut out = Vec::with_capacity(listed);
        for &id in &ids[..listed] {
            // SAFETY: the buffer is a zeroed proc_threadinfo of the size passed.
            let mut info: libc::proc_threadinfo = unsafe { std::mem::zeroed() };
            let n = unsafe { libc::proc_pidinfo(pid, THREAD_ID_INFO, id, (&raw mut info).cast::<c_void>(), size) };
            if n != size {
                // The thread exited after it was listed.
                continue;
            }
            let name: Vec<u8> = info.pth_name.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
            let state = match info.pth_run_state {
                libc::TH_STATE_RUNNING => ThreadState::Running,
                libc::TH_STATE_WAITING => ThreadState::Waiting,
                libc::TH_STATE_UNINTERRUPTIBLE => ThreadState::Blocked,
                libc::TH_STATE_STOPPED | libc::TH_STATE_HALTED => ThreadState::Stopped,
                _ => ThreadState::Unknown,
            };
            out.push(ThreadInfo { id, name: String::from_utf8_lossy(&name).into_owned(), state, cpu_time: Duration::from_nanos(info.pth_user_time + info.pth_system_time) });
        }
        Ok(out)
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, FILETIME, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32};
    use windows_sys::Win32::System::Threading::{GetThreadDescription, GetThreadTimes, OpenThread, THREAD_QUERY_LIMITED_INFORMATION};

    /// Calls `f` with the owning process and ID of every thread on the system.
    fn each_thread(mut f: impl FnMut(u32, u32)) {
        // SAFETY: the snapshot handle is checked and closed; the entry's size is set as the API requires.
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snap == INVALID_HANDLE_VALUE {
                return;
            }
            let mut entry: THREADENTRY32 = std::mem::zeroed();
            entry.dwSize = size_of::<THREADENTRY32>() as u32;
            let mut more = Thread32First(snap, &mut entry);
            while more != 0 {
                f(entry.th32OwnerProcessID, entry.th32ThreadID);
                more = Thread32Next(snap, &mut entry);
            }
            CloseHandle(snap);
        }
    }

    pub fn quick(_pids: &[u32]) -> HashMap<u32, Quick> {
        let mut out: HashMap<u32, Quick> = HashMap::new();
        each_thread(|pid, _| *out.entry(pid).or_default().threads.get_or_insert(0) += 1);
        out
    }

    /// Windows does not report how much of one process is in the page file.
    pub fn swap(_pid: u32) -> Option<u64> {
        None
    }

    fn filetime(t: FILETIME) -> u64 {
        ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64
    }

    pub fn threads(pid: u32) -> Result<Vec<ThreadInfo>, Denied> {
        let mut ids = Vec::new();
        each_thread(|owner, id| {
            if owner == pid {
                ids.push(id);
            }
        });
        if ids.is_empty() {
            return Err(Denied::Gone);
        }
        let mut out = Vec::with_capacity(ids.len());
        let mut refused = 0;
        for id in ids {
            // SAFETY: the handle is checked and closed; out-pointers are valid for the calls.
            unsafe {
                let handle = OpenThread(THREAD_QUERY_LIMITED_INFORMATION, 0, id);
                if handle.is_null() {
                    refused += 1;
                    continue;
                }
                let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
                let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
                let timed = GetThreadTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) != 0;
                let mut name = String::new();
                let mut wide: *mut u16 = std::ptr::null_mut();
                if GetThreadDescription(handle, &mut wide) >= 0 && !wide.is_null() {
                    let len = (0..).take_while(|&i| *wide.add(i) != 0).count();
                    name = String::from_utf16_lossy(std::slice::from_raw_parts(wide, len));
                    LocalFree(wide.cast());
                }
                CloseHandle(handle);
                // FILETIME counts 100-nanosecond intervals.
                let cpu_time = if timed { Duration::from_nanos((filetime(kernel) + filetime(user)) * 100) } else { Duration::ZERO };
                out.push(ThreadInfo { id: id as u64, name, state: ThreadState::Unknown, cpu_time });
            }
        }
        if out.is_empty() && refused > 0 { Err(Denied::Permission) } else { Ok(out) }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos", windows)))]
mod imp {
    use super::*;

    pub fn quick(_pids: &[u32]) -> HashMap<u32, Quick> {
        HashMap::new()
    }

    pub fn swap(_pid: u32) -> Option<u64> {
        None
    }

    pub fn threads(_pid: u32) -> Result<Vec<ThreadInfo>, Denied> {
        Err(Denied::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn parses_stat_with_awkward_names() {
        let stat = "4242 (tokio) worker (1)) S 1 4242 4242 0 -1 4194560 100 0 0 0 250 50 0 0 20 0 8 0 100 1000 10";
        let t = imp::parse_stat(4242, stat, 100).unwrap();
        assert_eq!(t.name, "tokio) worker (1)");
        assert_eq!(t.state, ThreadState::Waiting);
        assert_eq!(t.cpu_time, Duration::from_secs(3));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_footprint_counts_memory_the_process_has_touched() {
        let pid = std::process::id();
        let before = imp::footprint(pid).expect("a process can read its own footprint");
        assert!(before > 1 << 20, "more than a megabyte, got {before}");
        // Touch every page of 64 MB so it is really the process's own.
        let mut block = vec![0u8; 64 << 20];
        for page in block.chunks_mut(4096) {
            page[0] = 1;
        }
        let after = imp::footprint(pid).unwrap();
        assert!(after >= before + (48 << 20), "grew by about the block: {} MB to {} MB", before >> 20, after >> 20);
        assert_eq!(quick(&[pid])[&pid].footprint.map(|f| f >> 24), Some(after >> 24), "and the per-sample reading carries it");
        std::hint::black_box(&block);
        // A process that does not exist has none.
        assert_eq!(imp::footprint(u32::MAX - 7), None);
    }

    /// For checking by hand against `footprint <pid>` or Activity Monitor:
    /// `NEO_PID=<pid> cargo test -p neo-monitor -- --ignored --nocapture prints_a_footprint`.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn prints_a_footprint() {
        let pid: u32 = std::env::var("NEO_PID").expect("set NEO_PID").parse().unwrap();
        println!("footprint of {pid}: {:?} MB", imp::footprint(pid).map(|f| f as f64 / 1048576.0));
    }

    #[cfg(any(target_os = "linux", target_os = "android", target_os = "macos", windows))]
    #[test]
    fn sees_its_own_threads_and_their_cpu_time() {
        let pid = std::process::id();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = stop.clone();
        let worker = std::thread::Builder::new()
            .name("neo-busy".into())
            .spawn(move || {
                while !flag.load(std::sync::atomic::Ordering::Relaxed) {
                    std::hint::spin_loop();
                }
            })
            .unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let list = threads(pid).expect("a process can read its own threads");
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        worker.join().unwrap();
        let busy = list.iter().find(|t| t.name == "neo-busy").expect("the named thread is listed");
        assert!(busy.cpu_time >= Duration::from_millis(150) && busy.cpu_time < Duration::from_millis(600), "busy thread used {:?}", busy.cpu_time);
        let count = quick(&[pid])[&pid].threads.unwrap();
        assert!(count >= 2, "counted {count} threads");
    }
}
