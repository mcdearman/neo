//! Reading a folder on macOS with `getattrlistbulk`, which returns the
//! names and sizes of many entries in one call. Asking about each file
//! separately costs a call per file, and on a disk with millions of them
//! that is most of the time a scan takes.

use std::cell::RefCell;
use std::ffi::{CString, c_void};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use crate::scan::{Item, Listing};

#[repr(C)]
struct AttrList {
    bitmapcount: u16,
    reserved: u16,
    commonattr: u32,
    volattr: u32,
    dirattr: u32,
    fileattr: u32,
    forkattr: u32,
}

unsafe extern "C" {
    fn getattrlistbulk(dirfd: i32, attrs: *mut AttrList, buf: *mut c_void, size: usize, options: u64) -> i32;
}

const ATTR_BIT_MAP_COUNT: u16 = 5;
const ATTR_CMN_NAME: u32 = 0x0000_0001;
const ATTR_CMN_OBJTYPE: u32 = 0x0000_0008;
const ATTR_CMN_FILEID: u32 = 0x0200_0000;
const ATTR_CMN_ERROR: u32 = 0x2000_0000;
const ATTR_CMN_RETURNED_ATTRS: u32 = 0x8000_0000;
const ATTR_FILE_LINKCOUNT: u32 = 0x0000_0001;
const ATTR_FILE_TOTALSIZE: u32 = 0x0000_0002;
const ATTR_FILE_ALLOCSIZE: u32 = 0x0000_0004;
/// The object type of a directory.
const VDIR: u32 = 2;

thread_local! {
    /// Where the system writes a batch of entries. One per thread, kept.
    static BUFFER: RefCell<Vec<u8>> = RefCell::new(vec![0; 128 * 1024]);
}

struct Fd(i32);

impl Drop for Fd {
    fn drop(&mut self) {
        unsafe { libc::close(self.0) };
    }
}

/// Reads fixed-size values out of a batch, never past its end.
struct Reader<'a> {
    buf: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn bytes<const N: usize>(&mut self) -> Option<[u8; N]> {
        let out = self.buf.get(self.at..self.at + N)?.try_into().ok()?;
        self.at += N;
        Some(out)
    }
    fn u32(&mut self) -> Option<u32> {
        self.bytes().map(u32::from_ne_bytes)
    }
    fn u64(&mut self) -> Option<u64> {
        self.bytes().map(u64::from_ne_bytes)
    }
}

/// Reads one entry, which starts at `start` and is `buf.len()` long at most.
/// Each attribute is present only if the entry says it was returned, and
/// they come in a fixed order.
fn entry(buf: &[u8], on_disk: bool) -> Option<Item> {
    let mut r = Reader { buf, at: 4 };
    let (common, _vol, _dir, file, _fork) = (r.u32()?, r.u32()?, r.u32()?, r.u32()?, r.u32()?);
    if common & ATTR_CMN_ERROR != 0 && r.u32()? != 0 {
        return None;
    }
    let mut name = String::new();
    if common & ATTR_CMN_NAME != 0 {
        // Where the name is, counted from this reference, and its length
        // with the zero that ends it.
        let at = r.at;
        let (offset, len) = (r.u32()? as i32, r.u32()? as usize);
        let from = at.checked_add_signed(offset as isize)?;
        name = String::from_utf8_lossy(buf.get(from..from + len.checked_sub(1)?)?).into_owned();
    }
    let kind = if common & ATTR_CMN_OBJTYPE != 0 { r.u32()? } else { 0 };
    let id = if common & ATTR_CMN_FILEID != 0 { r.u64()? } else { 0 };
    let links = if file & ATTR_FILE_LINKCOUNT != 0 { r.u32()? as u64 } else { 1 };
    let total = if file & ATTR_FILE_TOTALSIZE != 0 { r.u64()? } else { 0 };
    let alloc = if file & ATTR_FILE_ALLOCSIZE != 0 { r.u64()? } else { 0 };
    (!name.is_empty()).then_some(Item { name, dir: kind == VDIR, size: if on_disk { alloc } else { total }, links, id })
}

pub fn list(path: &Path, on_disk: bool) -> io::Result<Listing> {
    let c = CString::new(path.as_os_str().as_bytes()).map_err(|_| io::ErrorKind::InvalidInput)?;
    // Not followed: a link to a folder is not a folder to read.
    let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = Fd(fd);
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd.0, &mut st) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut attrs = AttrList { bitmapcount: ATTR_BIT_MAP_COUNT, reserved: 0, commonattr: ATTR_CMN_RETURNED_ATTRS | ATTR_CMN_ERROR | ATTR_CMN_NAME | ATTR_CMN_OBJTYPE | ATTR_CMN_FILEID, volattr: 0, dirattr: 0, fileattr: ATTR_FILE_LINKCOUNT | ATTR_FILE_TOTALSIZE | ATTR_FILE_ALLOCSIZE, forkattr: 0 };
    let mut items = vec![];
    BUFFER.with_borrow_mut(|buf| {
        loop {
            let count = unsafe { getattrlistbulk(fd.0, &mut attrs, buf.as_mut_ptr().cast(), buf.len(), 0) };
            if count < 0 {
                return Err(io::Error::last_os_error());
            }
            if count == 0 {
                return Ok(());
            }
            let mut at = 0;
            for _ in 0..count {
                // Each entry begins with its own length.
                let Some(len) = buf.get(at..at + 4).and_then(|b| b.try_into().ok()).map(u32::from_ne_bytes) else { break };
                let Some(one) = buf.get(at..at + len as usize).filter(|_| len >= 4) else { break };
                items.extend(entry(one, on_disk));
                at += len as usize;
            }
        }
    })?;
    Ok(Listing { mtime: st.st_mtime * 1_000_000_000 + st.st_mtime_nsec, dev: st.st_dev as u64, items })
}
