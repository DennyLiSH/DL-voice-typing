use crate::error::AppError;
use std::time::{Duration, Instant};

const MAX_RETRIES: u32 = 3;
const RETRY_DELAY: Duration = Duration::from_millis(50);

/// Clipboard manager for save/restore + Ctrl+V simulation.
pub struct ClipboardManager {
    saved_content: Option<String>,
}

impl ClipboardManager {
    pub fn new() -> Self {
        Self {
            saved_content: None,
        }
    }

    /// Save current clipboard content.
    pub fn save(&mut self) -> Result<(), AppError> {
        self.saved_content = read_clipboard().ok();
        Ok(())
    }

    /// Write text to clipboard and simulate Ctrl+V to paste.
    /// **Blocking:** contains a sleep for paste processing.
    /// Must be called via `spawn_blocking` from async contexts.
    pub fn inject_text(&self, text: &str) -> Result<(), AppError> {
        write_clipboard_with_retry(text)?;
        simulate_paste()?;
        // Wait for target application to process the paste.
        // Initial 80ms covers most apps; then poll every 20ms up to 120ms more.
        std::thread::sleep(Duration::from_millis(80));
        let deadline = Instant::now() + Duration::from_millis(120);
        while Instant::now() < deadline {
            // If clipboard content changed, target app consumed the paste.
            if read_clipboard().map_or(true, |c| c != text) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        if let Some(ref saved) = self.saved_content {
            let _ = write_clipboard_with_retry(saved);
        }
        Ok(())
    }

    /// Restore saved clipboard content without injecting.
    pub fn restore(&mut self) -> Result<(), AppError> {
        if let Some(ref saved) = self.saved_content {
            write_clipboard_with_retry(saved)?;
        }
        self.saved_content = None;
        Ok(())
    }
}

impl Default for ClipboardManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Read text from the Windows clipboard using raw Win32 API.
fn read_clipboard() -> Result<String, AppError> {
    use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};

    unsafe {
        OpenClipboard(None).map_err(|e| AppError::Clipboard(format!("open failed: {e}")))?;

        let result = (|| -> Result<String, AppError> {
            // CF_UNICODETEXT = 13
            let handle = GetClipboardData(13u32)
                .map_err(|e| AppError::Clipboard(format!("get data failed: {e}")))?;

            let ptr = GlobalLock(windows::Win32::Foundation::HGLOBAL(handle.0));
            if ptr.is_null() {
                return Err(AppError::Clipboard("lock failed: null pointer".to_string()));
            }

            // Use GlobalSize for O(1) string length instead of O(n) null scan.
            let block_size = GlobalSize(windows::Win32::Foundation::HGLOBAL(handle.0));
            let len = if block_size > 0 {
                (block_size / 2).saturating_sub(1) as usize
            } else {
                // Fallback: scan for null terminator.
                let u16_ptr = ptr as *const u16;
                let mut l = 0usize;
                while *u16_ptr.add(l) != 0 {
                    l += 1;
                }
                l
            };

            let u16_ptr = ptr as *const u16;
            let slice = std::slice::from_raw_parts(u16_ptr, len);
            let text = String::from_utf16(slice)
                .map_err(|e| AppError::Clipboard(format!("utf16 decode failed: {e}")))?;

            let _ = GlobalUnlock(windows::Win32::Foundation::HGLOBAL(handle.0));
            Ok(text)
        })();

        let _ = CloseClipboard();
        result
    }
}

/// Write text to the clipboard, retrying on failure.
fn write_clipboard_with_retry(text: &str) -> Result<(), AppError> {
    for attempt in 0..MAX_RETRIES {
        match write_clipboard(text) {
            Ok(()) => return Ok(()),
            Err(_) if attempt < MAX_RETRIES - 1 => {
                std::thread::sleep(RETRY_DELAY);
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}

/// Write text to the Windows clipboard using raw Win32 API.
fn write_clipboard(text: &str) -> Result<(), AppError> {
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};

    unsafe {
        OpenClipboard(None).map_err(|e| AppError::Clipboard(format!("open failed: {e}")))?;
        EmptyClipboard().map_err(|e| AppError::Clipboard(format!("empty failed: {e}")))?;

        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0u16)).collect();
        let byte_len = wide.len() * 2;

        let hglobal = GlobalAlloc(GMEM_MOVEABLE, byte_len)
            .map_err(|e| AppError::Clipboard(format!("alloc failed: {e}")))?;

        let ptr = GlobalLock(hglobal);
        if ptr.is_null() {
            return Err(AppError::Clipboard("lock failed".to_string()));
        }

        std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr as *mut u16, wide.len());
        let _ = GlobalUnlock(hglobal);

        // SetClipboardData takes Param<HANDLE>, HGLOBAL contains HANDLE internally
        // We need to convert HGLOBAL -> HANDLE
        let handle = windows::Win32::Foundation::HANDLE(hglobal.0);
        SetClipboardData(13u32, handle)
            .map_err(|e| AppError::Clipboard(format!("set data failed: {e}")))?;

        let _ = CloseClipboard();
        Ok(())
    }
}

/// Simulate Ctrl+V keypress using Win32 SendInput.
fn simulate_paste() -> Result<(), AppError> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_TYPE, KEYEVENTF_KEYUP, SendInput, VK_CONTROL, VK_V,
    };

    unsafe {
        let mut inputs: [INPUT; 4] = std::mem::zeroed();

        // Ctrl down
        inputs[0].r#type = INPUT_TYPE(1);
        inputs[0].Anonymous.ki.wVk = VK_CONTROL;

        // V down
        inputs[1].r#type = INPUT_TYPE(1);
        inputs[1].Anonymous.ki.wVk = VK_V;

        // V up
        inputs[2].r#type = INPUT_TYPE(1);
        inputs[2].Anonymous.ki.wVk = VK_V;
        inputs[2].Anonymous.ki.dwFlags = KEYEVENTF_KEYUP;

        // Ctrl up
        inputs[3].r#type = INPUT_TYPE(1);
        inputs[3].Anonymous.ki.wVk = VK_CONTROL;
        inputs[3].Anonymous.ki.dwFlags = KEYEVENTF_KEYUP;

        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_clipboard_manager() {
        let manager = ClipboardManager::new();
        assert!(manager.saved_content.is_none());
    }

    #[test]
    fn test_save_and_restore_cycle() {
        let mut manager = ClipboardManager::new();
        assert!(manager.save().is_ok());
    }
}
