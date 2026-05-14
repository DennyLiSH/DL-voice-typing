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

/// Mock implementation for testing.
pub struct MockPlatformProvider {
    caret_pos: std::sync::Mutex<(f64, f64)>,
    work_area: std::sync::Mutex<Option<(i32, i32, i32, i32)>>,
    foreground: std::sync::Mutex<isize>,
    restore_log: std::sync::Mutex<Vec<isize>>,
}

impl MockPlatformProvider {
    pub fn new() -> Self {
        Self {
            caret_pos: std::sync::Mutex::new((100.0, 200.0)),
            work_area: std::sync::Mutex::new(Some((0, 0, 1920, 1080))),
            foreground: std::sync::Mutex::new(42),
            restore_log: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn set_caret_pos(&self, x: f64, y: f64) {
        *self.caret_pos.lock().unwrap() = (x, y);
    }

    pub fn set_work_area(&self, area: Option<(i32, i32, i32, i32)>) {
        *self.work_area.lock().unwrap() = area;
    }

    pub fn restore_log(&self) -> Vec<isize> {
        self.restore_log.lock().unwrap().clone()
    }
}

impl PlatformProvider for MockPlatformProvider {
    fn caret_screen_pos(&self) -> (f64, f64) {
        *self.caret_pos.lock().unwrap()
    }

    fn monitor_work_area(&self, x: i32, y: i32) -> Option<(i32, i32, i32, i32)> {
        let _ = (x, y);
        *self.work_area.lock().unwrap()
    }

    fn foreground_hwnd(&self) -> isize {
        *self.foreground.lock().unwrap()
    }

    fn restore_foreground_hwnd(&self, hwnd: isize) {
        self.restore_log.lock().unwrap().push(hwnd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_caret_pos_default() {
        let p = MockPlatformProvider::new();
        assert_eq!(p.caret_screen_pos(), (100.0, 200.0));
    }

    #[test]
    fn test_mock_caret_pos_custom() {
        let p = MockPlatformProvider::new();
        p.set_caret_pos(500.0, 600.0);
        assert_eq!(p.caret_screen_pos(), (500.0, 600.0));
    }

    #[test]
    fn test_mock_work_area() {
        let p = MockPlatformProvider::new();
        assert_eq!(p.monitor_work_area(0, 0), Some((0, 0, 1920, 1080)));
        p.set_work_area(None);
        assert_eq!(p.monitor_work_area(0, 0), None);
    }

    #[test]
    fn test_mock_foreground_lifecycle() {
        let p = MockPlatformProvider::new();
        let hwnd = p.foreground_hwnd();
        p.restore_foreground_hwnd(hwnd);
        assert_eq!(p.restore_log(), vec![42]);
    }
}
