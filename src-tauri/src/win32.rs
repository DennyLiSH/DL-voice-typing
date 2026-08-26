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
    // SAFETY: GetGUIThreadInfo with threadId=0 queries the foreground thread.
    // `gui` is properly initialized with cbSize. Output is written to our stack variable.
    if unsafe { GetGUIThreadInfo(0, &mut gui) }.is_ok() && !gui.hwndCaret.is_invalid() {
        let mut pt = POINT {
            x: gui.rcCaret.left,
            y: gui.rcCaret.top,
        };
        // SAFETY: ClientToScreen converts client coordinates to screen coordinates.
        // hwndCaret is a valid window handle (checked non-invalid above). pt is stack-allocated.
        let _ = unsafe { ClientToScreen(gui.hwndCaret, &mut pt) };
        return (pt.x as f64, pt.y as f64);
    }

    // Strategy 2: UI Automation — works for Chrome, Edge, VS Code, etc.
    // SAFETY: CoCreateInstance creates a COM object for IUIAutomation. CUIAutomation
    // is a valid class identifier, CLSCTX_ALL is the standard server context. The call
    // is safe on any thread as UI Automation handles apartment initialization internally.
    let automation: Result<IUIAutomation, _> =
        unsafe { CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_ALL) };
    if let Ok(automation) = automation {
        // SAFETY: All UI Automation calls below operate on valid COM interface pointers
        // obtained from CoCreateInstance and its chain of method calls. Each call returns
        // a Result that we check before proceeding. The SAFEARRAY from GetBoundingRectangles
        // is passed to extract_first_rect_from_safearray which documents its safety requirements.
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
    // SAFETY: GetCursorPos writes to our stack-allocated POINT. No preconditions beyond
    // a valid output pointer.
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
    // SAFETY: MonitorFromPoint receives a valid POINT. MONITOR_DEFAULTTONEAREST always
    // returns a valid monitor handle (fallback to nearest). No preconditions beyond valid input.
    let monitor = unsafe { MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_invalid() {
        return None;
    }

    // SAFETY: std::mem::zeroed is valid for MONITORINFOEXW — it's a POD struct with
    // no invalid bit patterns. We set cbSize immediately after.
    let mut info: MONITORINFOEXW = unsafe { std::mem::zeroed() };
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    // SAFETY: monitor is a valid handle from MonitorFromPoint. info.monitorInfo is
    // properly initialized with cbSize. GetMonitorInfoW writes to our stack variable.
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
    // SAFETY: GetForegroundWindow has no preconditions. Returns a valid HWND (or null
    // if no foreground window exists, which is rare but handled by callers).
    let hwnd = unsafe { GetForegroundWindow() };
    hwnd.0 as isize
}

/// Check whether a window handle still refers to a live window.
pub fn is_window_valid(hwnd_val: isize) -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::IsWindow;
    let hwnd = HWND(hwnd_val as *mut _);
    // SAFETY: IsWindow has no preconditions; an invalid/dangling HWND value
    // simply returns FALSE.
    unsafe { IsWindow(hwnd).as_bool() }
}

/// Read the title of a window handle. Returns None when the handle is
/// invalid or the title is empty.
pub fn get_window_title(hwnd_val: isize) -> Option<String> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowTextLengthW, GetWindowTextW};
    // SAFETY: GetWindowTextLengthW/GetWindowTextW have no preconditions;
    // an invalid HWND simply returns 0. windows 0.58 takes the buffer as a
    // &mut [u16] slice — the slice length IS nMaxCount (NUL room included).
    unsafe {
        let hwnd = HWND(hwnd_val as *mut _);
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return None;
        }
        let mut buf = vec![0u16; len as usize + 1];
        let copied = GetWindowTextW(hwnd, &mut buf);
        if copied <= 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..copied as usize]))
    }
}

/// Restore focus to a saved window handle, reporting whether
/// `SetForegroundWindow` actually succeeded.
///
/// Uses `AttachThreadInput` to share input state with the foreground window's
/// thread, then calls `SetForegroundWindow` + `BringWindowToTop`. This is
/// necessary because `SetForegroundWindow` may fail when called from a
/// non-UI thread (e.g., Tokio runtime) even if the process is the foreground
/// process — Windows restricts which threads can change the foreground window.
pub fn restore_foreground_hwnd_checked(hwnd_val: isize) -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, IsIconic, SW_RESTORE,
        SetForegroundWindow, ShowWindow,
    };
    // SAFETY: All Win32 calls below operate on valid handles and thread IDs obtained
    // from their respective APIs. AttachThreadInput attaches/detaches the current thread's
    // input processing to the foreground window's thread — both TIDs are from valid API
    // results. SetForegroundWindow, ShowWindow, BringWindowToTop receive a valid HWND
    // cast from the stored isize. All return values are checked or discarded intentionally.
    unsafe {
        let hwnd = HWND(hwnd_val as *mut _);

        // Attach our thread's input to the foreground window's thread.
        // This grants us foreground-lock permission from that thread.
        let fg_hwnd = GetForegroundWindow();
        let fg_tid = GetWindowThreadProcessId(fg_hwnd, None);
        let cur_tid = GetCurrentThreadId();
        let attached = if fg_tid != cur_tid && fg_tid != 0 {
            AttachThreadInput(cur_tid, fg_tid, true).as_bool()
        } else {
            false
        };

        // Restore window if minimized.
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let ok = SetForegroundWindow(hwnd).as_bool();
        let _ = BringWindowToTop(hwnd);

        // Detach input processing.
        if attached {
            let _ = AttachThreadInput(cur_tid, fg_tid, false);
        }
        ok
    }
}

/// Restore focus to a saved window handle.
///
/// Uses `AttachThreadInput` to share input state with the foreground window's
/// thread, then calls `SetForegroundWindow` + `BringWindowToTop`. This is
/// necessary because `SetForegroundWindow` may fail when called from a
/// non-UI thread (e.g., Tokio runtime) even if the process is the foreground
/// process — Windows restricts which threads can change the foreground window.
pub fn restore_foreground_hwnd(hwnd_val: isize) {
    let _ = restore_foreground_hwnd_checked(hwnd_val);
}
