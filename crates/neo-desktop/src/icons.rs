//! The icon the system shows for a file: an app's own, a shortcut's being
//! that of what it opens.

use std::path::Path;

use neo::Image;

/// The icon of the file at `path`, `side` pixels square. Only Windows is
/// asked here so far; elsewhere there is none to give.
pub fn of_file(path: &Path, side: usize) -> Option<Image> {
    #[cfg(windows)]
    {
        windows::read(path, side).map(|pixels| Image::frame(side as u32, side as u32, pixels))
    }
    #[cfg(not(windows))]
    {
        let _ = (path, side);
        None
    }
}

/// Reads the icon Windows shows for a file, as the shell gives it.
#[cfg(windows)]
mod windows {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Graphics::Gdi::{BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, SelectObject};
    use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    use windows_sys::Win32::UI::Controls::{ILD_TRANSPARENT, ImageList_GetIcon};
    use windows_sys::Win32::UI::Shell::{SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGFI_SYSICONINDEX, SHGetFileInfoW, SHGetImageList, SHIL_EXTRALARGE};
    use windows_sys::Win32::UI::WindowsAndMessaging::{DI_MASK, DI_NORMAL, DestroyIcon, DrawIconEx, HICON};
    use windows_sys::core::GUID;

    const IID_IIMAGELIST: GUID = GUID { data1: 0x46EB5926, data2: 0x582E, data3: 0x4017, data4: [0x9F, 0xDF, 0xE8, 0x99, 0x8D, 0xAA, 0x09, 0x50] };

    /// The icon of the file at `path`, a shortcut's being that of what it
    /// opens, as a `side` × `side` square of straight-alpha RGBA pixels.
    pub fn read(path: &Path, side: usize) -> Option<Vec<u8>> {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: the path ends in a nul, every handle made here is checked
        // before use and let go of after, and the pixels read are within the
        // bitmap made to hold them.
        unsafe {
            // The shell wants COM on the thread that asks; more than once does no harm.
            CoInitializeEx(null(), COINIT_APARTMENTTHREADED as u32);
            let icon = system_icon(&wide)?;
            let pixels = draw(icon, side);
            DestroyIcon(icon);
            pixels
        }
    }

    /// The icon at its 48 pixel size, from the system's list of them; the
    /// 32 pixel one if that list cannot be had.
    unsafe fn system_icon(wide: &[u16]) -> Option<HICON> {
        unsafe {
            let mut info: SHFILEINFOW = std::mem::zeroed();
            let size = size_of::<SHFILEINFOW>() as u32;
            if SHGetFileInfoW(wide.as_ptr(), 0, &mut info, size, SHGFI_SYSICONINDEX) != 0 {
                let mut list: *mut c_void = null_mut();
                if SHGetImageList(SHIL_EXTRALARGE as i32, &IID_IIMAGELIST, &mut list) >= 0 && !list.is_null() {
                    let icon = ImageList_GetIcon(list as _, info.iIcon, ILD_TRANSPARENT);
                    // The list is a COM object, given with a count taken on it: IUnknown's third method gives it back.
                    let release: unsafe extern "system" fn(*mut c_void) -> u32 = std::mem::transmute(*(*(list as *const *const usize)).add(2));
                    release(list);
                    if !icon.is_null() {
                        return Some(icon);
                    }
                }
            }
            let mut info: SHFILEINFOW = std::mem::zeroed();
            (SHGetFileInfoW(wide.as_ptr(), 0, &mut info, size, SHGFI_ICON | SHGFI_LARGEICON) != 0 && !info.hIcon.is_null()).then_some(info.hIcon)
        }
    }

    unsafe fn draw(icon: HICON, side: usize) -> Option<Vec<u8>> {
        unsafe {
            let dc = CreateCompatibleDC(null_mut());
            if dc.is_null() {
                return None;
            }
            let mut info: BITMAPINFO = std::mem::zeroed();
            // A negative height is a bitmap whose first row is its top one.
            info.bmiHeader = BITMAPINFOHEADER { biSize: size_of::<BITMAPINFOHEADER>() as u32, biWidth: side as i32, biHeight: -(side as i32), biPlanes: 1, biBitCount: 32, ..std::mem::zeroed() };
            let mut bits: *mut c_void = null_mut();
            let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
            if bitmap.is_null() || bits.is_null() {
                DeleteDC(dc);
                return None;
            }
            let old = SelectObject(dc, bitmap);
            let bytes = std::slice::from_raw_parts_mut(bits.cast::<u8>(), side * side * 4);
            let drawn = DrawIconEx(dc, 0, 0, icon, side as i32, side as i32, 0, null_mut(), DI_NORMAL) != 0;
            let mut pixels = bytes.to_vec();
            if drawn && pixels.chunks_exact(4).all(|p| p[3] == 0) {
                // An old icon, with a mask in place of an alpha channel: the
                // mask is black where the icon is.
                bytes.fill(0);
                DrawIconEx(dc, 0, 0, icon, side as i32, side as i32, 0, null_mut(), DI_MASK);
                for (p, m) in pixels.chunks_exact_mut(4).zip(bytes.chunks_exact(4)) {
                    p[3] = if m[0] == 0 { 255 } else { 0 };
                }
            }
            SelectObject(dc, old);
            DeleteObject(bitmap);
            DeleteDC(dc);
            if !drawn {
                return None;
            }
            // Windows draws blue first and premultiplied; Neo wants red first and straight alpha.
            for p in pixels.chunks_exact_mut(4) {
                p.swap(0, 2);
                let a = p[3] as u32;
                if a != 0 && a != 255 {
                    for c in &mut p[..3] {
                        *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
                    }
                }
            }
            Some(pixels)
        }
    }
}
