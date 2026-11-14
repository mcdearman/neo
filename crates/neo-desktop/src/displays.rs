//! The displays, as far as which one the user is working on.

/// The display the user is working on: the one holding the window in front,
/// or failing that the one under the pointer. Left, top, width and height
/// in the system's pixels. For a window called up by a key, such as the
/// launcher, to come up there and not where it was last. `None` where the
/// system cannot say here: so far everywhere but Windows.
pub fn active() -> Option<(i32, i32, i32, i32)> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTONULL};
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetForegroundWindow};
        // SAFETY: plain calls; the structures are the sizes they say.
        unsafe {
            let front = GetForegroundWindow();
            let mut monitor = if front.is_null() { std::ptr::null_mut() } else { MonitorFromWindow(front, MONITOR_DEFAULTTONULL) };
            if monitor.is_null() {
                let mut p = POINT { x: 0, y: 0 };
                if GetCursorPos(&mut p) == 0 {
                    return None;
                }
                monitor = MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST);
            }
            let mut info: MONITORINFO = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(monitor, &mut info) == 0 {
                return None;
            }
            let r = info.rcMonitor;
            Some((r.left, r.top, r.right - r.left, r.bottom - r.top))
        }
    }
    #[cfg(not(windows))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[cfg(windows)]
    fn a_display_is_found_where_there_is_one() {
        let (_, _, w, h) = super::active().expect("a display");
        assert!(w > 0 && h > 0);
    }
}
