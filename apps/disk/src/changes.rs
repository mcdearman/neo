//! What changed under a folder since it was last measured.
//!
//! macOS keeps a record, for each volume, of every folder in which
//! something changed, and will play it back from any point. That lets an
//! index be brought up to date by reading only those folders again. Other
//! systems keep no such record for a program that was not running, and
//! there each folder's own time is checked instead.

use std::path::{Path, PathBuf};

/// A point in the system's record of changes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mark {
    /// The number of the last change before this point.
    pub event: u64,
    /// Which record the number belongs to. A volume's record is replaced,
    /// and its numbers start over, when it is erased or the record is lost.
    pub volume: String,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub enum Changes {
    /// There is no telling, so everything has to be read.
    Unknown,
    /// These folders changed. The flag says everything under one did.
    Folders(Vec<(PathBuf, bool)>),
    /// There is no record: compare each folder's time with the one noted.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    CheckTimes,
}

#[cfg(not(target_os = "macos"))]
pub fn mark(_root: &Path) -> Mark {
    Mark::default()
}

#[cfg(not(target_os = "macos"))]
pub fn since(_root: &Path, _mark: &Mark) -> Changes {
    Changes::CheckTimes
}

#[cfg(target_os = "macos")]
pub use mac::{mark, since};

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use std::ffi::{CStr, c_char, c_void};
    use std::sync::Mutex;
    use std::sync::mpsc::{Sender, channel};
    use std::time::{Duration, Instant};

    #[repr(C)]
    struct Context {
        version: isize,
        info: *mut c_void,
        retain: *const c_void,
        release: *const c_void,
        describe: *const c_void,
    }

    type Callback = extern "C" fn(stream: *const c_void, info: *mut c_void, count: usize, paths: *mut c_void, flags: *const u32, ids: *const u64);

    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        fn FSEventsGetCurrentEventId() -> u64;
        fn FSEventsCopyUUIDForDevice(dev: i32) -> *const c_void;
        fn FSEventStreamCreate(alloc: *const c_void, callback: Callback, context: *const Context, paths: *const c_void, since: u64, latency: f64, flags: u32) -> *mut c_void;
        fn FSEventStreamSetDispatchQueue(stream: *mut c_void, queue: *mut c_void);
        fn FSEventStreamStart(stream: *mut c_void) -> u8;
        fn FSEventStreamStop(stream: *mut c_void);
        fn FSEventStreamInvalidate(stream: *mut c_void);
        fn FSEventStreamRelease(stream: *mut c_void);
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFTypeArrayCallBacks: c_void;
        fn CFUUIDCreateString(alloc: *const c_void, uuid: *const c_void) -> *const c_void;
        fn CFStringGetCString(string: *const c_void, buf: *mut c_char, size: isize, encoding: u32) -> u8;
        fn CFStringCreateWithBytes(alloc: *const c_void, bytes: *const u8, len: isize, encoding: u32, external: u8) -> *const c_void;
        fn CFArrayCreate(alloc: *const c_void, values: *const *const c_void, count: isize, callbacks: *const c_void) -> *const c_void;
        fn CFRelease(object: *const c_void);
    }

    unsafe extern "C" {
        fn dispatch_queue_create(label: *const c_char, attr: *const c_void) -> *mut c_void;
        fn dispatch_release(object: *mut c_void);
    }

    const UTF8: u32 = 0x0800_0100;
    /// Deliver events as they come, without waiting to gather more.
    const NO_DEFER: u32 = 0x2;
    const MUST_SCAN_SUBDIRS: u32 = 0x1;
    const USER_DROPPED: u32 = 0x2;
    const KERNEL_DROPPED: u32 = 0x4;
    const IDS_WRAPPED: u32 = 0x8;
    /// Everything from the past has been sent.
    const HISTORY_DONE: u32 = 0x10;
    const ROOT_CHANGED: u32 = 0x20;
    /// How long to wait for the record to be played back before giving up
    /// on it and reading everything.
    const PATIENCE: Duration = Duration::from_secs(60);

    /// The volume whose record covers `root`. The startup disk's files are
    /// on its data volume, whatever `/` itself is on.
    fn volume(root: &Path) -> String {
        use std::os::unix::fs::MetadataExt;
        let on = if root == Path::new("/") { Path::new("/System/Volumes/Data") } else { root };
        let Ok(meta) = std::fs::metadata(on) else { return String::new() };
        unsafe {
            let uuid = FSEventsCopyUUIDForDevice(meta.dev() as i32);
            if uuid.is_null() {
                return String::new();
            }
            let text = CFUUIDCreateString(std::ptr::null(), uuid);
            CFRelease(uuid);
            if text.is_null() {
                return String::new();
            }
            let mut buf = [0 as c_char; 64];
            let ok = CFStringGetCString(text, buf.as_mut_ptr(), buf.len() as isize, UTF8);
            CFRelease(text);
            if ok == 0 { String::new() } else { CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned() }
        }
    }

    pub fn mark(root: &Path) -> Mark {
        Mark { event: unsafe { FSEventsGetCurrentEventId() }, volume: volume(root) }
    }

    enum Told {
        Changed(String, u32),
        Done,
    }

    extern "C" fn told(_stream: *const c_void, info: *mut c_void, count: usize, paths: *mut c_void, flags: *const u32, _ids: *const u64) {
        // `info` is the sender, which is never freed while a stream lives.
        let to = unsafe { &*(info as *const Mutex<Sender<Told>>) };
        let Ok(to) = to.lock() else { return };
        let paths = paths as *const *const c_char;
        for i in 0..count {
            let (path, flag) = unsafe { (CStr::from_ptr(*paths.add(i)).to_string_lossy().into_owned(), *flags.add(i)) };
            let _ = to.send(if flag & HISTORY_DONE != 0 { Told::Done } else { Told::Changed(path, flag) });
        }
    }

    pub fn since(root: &Path, mark: &Mark) -> Changes {
        // Numbers from another record, or none at all, say nothing.
        if mark.event == 0 || mark.volume.is_empty() || volume(root) != mark.volume {
            return Changes::Unknown;
        }
        let (tx, rx) = channel();
        // Left alive for good: a late callback may still reach for it.
        let sender: &'static Mutex<Sender<Told>> = Box::leak(Box::new(Mutex::new(tx)));
        let context = Context { version: 0, info: sender as *const _ as *mut c_void, retain: std::ptr::null(), release: std::ptr::null(), describe: std::ptr::null() };
        let bytes = root.as_os_str().as_encoded_bytes();
        let mut folders = vec![];
        let mut whole = false;
        unsafe {
            let path = CFStringCreateWithBytes(std::ptr::null(), bytes.as_ptr(), bytes.len() as isize, UTF8, 0);
            if path.is_null() {
                return Changes::Unknown;
            }
            let paths = CFArrayCreate(std::ptr::null(), &path, 1, &raw const kCFTypeArrayCallBacks);
            let stream = FSEventStreamCreate(std::ptr::null(), told, &context, paths, mark.event, 0.0, NO_DEFER);
            CFRelease(paths);
            CFRelease(path);
            if stream.is_null() {
                return Changes::Unknown;
            }
            let queue = dispatch_queue_create(c"org.neo.Disk.changes".as_ptr(), std::ptr::null());
            FSEventStreamSetDispatchQueue(stream, queue);
            let mut finished = false;
            if FSEventStreamStart(stream) != 0 {
                let until = Instant::now() + PATIENCE;
                while let Ok(told) = rx.recv_timeout(until.saturating_duration_since(Instant::now())) {
                    match told {
                        Told::Done => {
                            finished = true;
                            break;
                        }
                        Told::Changed(_, flag) if flag & (USER_DROPPED | KERNEL_DROPPED | IDS_WRAPPED | ROOT_CHANGED) != 0 => whole = true,
                        Told::Changed(path, flag) => folders.push((path, flag & MUST_SCAN_SUBDIRS != 0)),
                    }
                }
                FSEventStreamStop(stream);
            }
            FSEventStreamInvalidate(stream);
            FSEventStreamRelease(stream);
            dispatch_release(queue);
            if !finished {
                return Changes::Unknown;
            }
        }
        let top = root == Path::new("/");
        let mut list = vec![];
        for (path, under) in folders {
            // The data volume's folders are reported where they really are.
            let path = match path.strip_prefix("/System/Volumes/Data") {
                Some(rest) if top && (rest.is_empty() || rest.starts_with('/')) => PathBuf::from(if rest.is_empty() { "/" } else { rest }),
                _ => PathBuf::from(path),
            };
            if !path.starts_with(root) {
                continue;
            }
            // Everything under the top is everything.
            whole |= under && path == root;
            list.push((path, under));
        }
        if whole { Changes::Unknown } else { Changes::Folders(list) }
    }
}
