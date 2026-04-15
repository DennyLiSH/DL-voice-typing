//! Win32 platform helpers for window positioning, caret detection, and focus management.

use windows::Win32::Foundation::POINT;
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, SAFEARRAY};
use windows::Win32::System::Ole::{
    SafeArrayAccessData, SafeArrayGetLBound, SafeArrayGetUBound, SafeArrayUnaccessData,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationTextPattern, UIA_TextPatternId,
};
use windows::Win32::UI::WindowsAndMessaging::{GUITHREADINFO, GetCursorPos, GetGUIThreadInfo};

/// Returns the text caret (cursor) position in screen coordinates.
/// Falls back through three strategies:
///   1. GetGUIThreadInfo (Win32 apps: Notepad, Word, etc.)
///   2. UI Automation TextPattern (Chrome, Edge, VS Code, Electron, etc.)
///   3. Mouse cursor position (last resort)
pub fn get_caret_screen_pos() -> (f64, f64) {
    // Strategy 1: GetGUIThreadInfo — works for classic Win32 apps.
    let mut gui: GUITHREADINFO = GUITHREADINFO {
        cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetGUIThreadInfo(0, &mut gui) }.is_ok() && !gui.hwndCaret.is_invalid() {
        let mut pt = POINT {
            x: gui.rcCaret.left,
            y: gui.rcCaret.top,
        };
        let _ = unsafe { ClientToScreen(gui.hwndCaret, &mut pt) };
        return (pt.x as f64, pt.y as f64);
    }

    // Strategy 2: UI Automation — works for Chrome, Edge, VS Code, etc.
    let automation: Result<IUIAutomation, _> =
        unsafe { CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_ALL) };
    if let Ok(automation) = automation {
        if let Ok(element) = unsafe { automation.GetFocusedElement() } {
            if let Ok(text_pattern) = unsafe {
                element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
            } {
                if let Ok(ranges) = unsafe { text_pattern.GetSelection() } {
                    if let Ok(count) = unsafe { ranges.Length() } {
                        if count > 0 {
                            if let Ok(range) = unsafe { ranges.GetElement(0) } {
                                if let Ok(sa) = unsafe { range.GetBoundingRectangles() } {
                                    if let Some((x, y)) =
                                        unsafe { extract_first_rect_from_safearray(sa) }
                                    {
                                        return (x, y);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Strategy 3: Fallback to mouse cursor.
    let mut pt = POINT { x: 0, y: 0 };
    let _ = unsafe { GetCursorPos(&mut pt) };
    (pt.x as f64, pt.y as f64)
}

/// Returns the work area (excluding taskbar) of the monitor containing the given point.
/// Returns `None` if the Win32 calls fail.
pub fn get_monitor_work_area(x: i32, y: i32) -> Option<(i32, i32, i32, i32)> {
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFOEXW, MonitorFromPoint,
    };

    let pt = POINT { x, y };
    let monitor = unsafe { MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_invalid() {
        return None;
    }

    let mut info: MONITORINFOEXW = unsafe { std::mem::zeroed() };
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    if !unsafe { GetMonitorInfoW(monitor, &mut info.monitorInfo) }.as_bool() {
        return None;
    }

    let rc = info.monitorInfo.rcWork;
    Some((rc.left, rc.top, rc.right, rc.bottom))
}

/// Extracts the first bounding rectangle (x, y) from a SAFEARRAY of f64
/// returned by IUIAutomationTextRange::GetBoundingRectangles.
///
/// # Safety
/// `sa` must point to a valid SAFEARRAY containing f64 values.
pub unsafe fn extract_first_rect_from_safearray(sa: *mut SAFEARRAY) -> Option<(f64, f64)> {
    // SAFETY: caller guarantees `sa` points to a valid SAFEARRAY of f64.
    let lower = unsafe { SafeArrayGetLBound(sa, 1).ok()? };
    let upper = unsafe { SafeArrayGetUBound(sa, 1).ok()? };
    let count = (upper - lower + 1) as usize;
    if count < 4 {
        return None; // Need at least x, y, w, h.
    }
    let mut data_ptr: *mut f64 = std::ptr::null_mut();
    unsafe { SafeArrayAccessData(sa, &mut data_ptr as *mut _ as *mut _).ok()? };
    let x = unsafe { *data_ptr };
    let y = unsafe { *data_ptr.add(1) };
    let _ = unsafe { SafeArrayUnaccessData(sa) };
    Some((x, y))
}

/// Save the current foreground window handle.
pub fn get_foreground_hwnd() -> isize {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    let hwnd = unsafe { GetForegroundWindow() };
    hwnd.0 as isize
}

/// Restore focus to a saved window handle.
pub fn restore_foreground_hwnd(hwnd_val: isize) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
    unsafe {
        let _ = SetForegroundWindow(HWND(hwnd_val as *mut _));
    }
}
