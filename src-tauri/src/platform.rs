//! Platform abstraction for Win32 operations.
//! Enables testing of window positioning and focus management without Windows.

/// Abstract platform operations for window positioning and focus management.
pub trait PlatformProvider: Send + Sync {
    /// Get the text caret position in screen coordinates.
    fn caret_screen_pos(&self) -> (f64, f64);
    /// Get the work area (excluding taskbar) of the monitor containing the point.
    fn monitor_work_area(&self, x: i32, y: i32) -> Option<(i32, i32, i32, i32)>;
    /// Get the current foreground window handle.
    fn foreground_hwnd(&self) -> isize;
    /// Restore focus to a saved window handle.
    fn restore_foreground_hwnd(&self, hwnd: isize);
}

/// Production implementation using Win32 APIs.
pub struct Win32PlatformProvider;

impl PlatformProvider for Win32PlatformProvider {
    fn caret_screen_pos(&self) -> (f64, f64) {
        crate::win32::get_caret_screen_pos()
    }

    fn monitor_work_area(&self, x: i32, y: i32) -> Option<(i32, i32, i32, i32)> {
        crate::win32::get_monitor_work_area(x, y)
    }

    fn foreground_hwnd(&self) -> isize {
        crate::win32::get_foreground_hwnd()
    }

    fn restore_foreground_hwnd(&self, hwnd: isize) {
        crate::win32::restore_foreground_hwnd(hwnd);
    }
}

