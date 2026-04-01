use crate::error::AppError;
use crate::hotkey::{HotkeyCallback, HotkeyEvent, HotkeyManager};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, SetWindowsHookExW, UnhookWindowsHookEx, HHOOK, KBDLLHOOKSTRUCT,
    WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::core::PCWSTR;

/// Windows global keyboard hook implementation.
pub struct WindowsHotkeyManager {
    hook: Option<HHOOK>,
    key_code: u32,
    is_pressed: Arc<AtomicBool>,
    callback: Option<HotkeyCallback>,
}

impl WindowsHotkeyManager {
    pub fn new() -> Self {
        Self {
            hook: None,
            key_code: 0,
            is_pressed: Arc::new(AtomicBool::new(false)),
            callback: None,
        }
    }

    /// Parse a key name string to a virtual key code.
    pub fn parse_key_code(key: &str) -> Option<u32> {
        match key.to_lowercase().as_str() {
            "rightalt" | "ralt" => Some(0xA5), // VK_RMENU
            "leftalt" | "lalt" => Some(0xA4),  // VK_LMENU
            "rightctrl" | "rctrl" => Some(0xA3),
            "leftctrl" | "lctrl" => Some(0xA2),
            "rightshift" | "rshift" => Some(0xA1),
            "leftshift" | "lshift" => Some(0xA0),
            "f1" => Some(0x70),
            "f2" => Some(0x71),
            "f3" => Some(0x72),
            "f4" => Some(0x73),
            "f5" => Some(0x74),
            "f6" => Some(0x75),
            "f7" => Some(0x76),
            "f8" => Some(0x77),
            "f9" => Some(0x78),
            "f10" => Some(0x79),
            "f11" => Some(0x7A),
            "f12" => Some(0x7B),
            "escape" | "esc" => Some(0x1B),
            _ => None,
        }
    }
}

impl HotkeyManager for WindowsHotkeyManager {
    fn register(&mut self, key: &str, callback: HotkeyCallback) -> Result<(), AppError> {
        let vk_code = WindowsHotkeyManager::parse_key_code(key).ok_or_else(|| {
            AppError::Hotkey(format!("unknown key: {}", key))
        })?;

        self.key_code = vk_code;
        self.callback = Some(callback);

        // The hook procedure needs to access the callback.
        // We use a thread-local or global approach.
        // For safety, we store the callback in a global and reference it from the hook.
        unsafe {
            let hook = SetWindowsHookExW(
                WH_KEYBOARD_LL,
                Some(keyboard_hook_proc),
                None,
                0,
            ).map_err(|e| AppError::Hotkey(format!("failed to set hook: {}", e)))?;

            self.hook = Some(hook);
        }

        // Store self reference for the hook callback
        // This is a simplified version - in production, use proper thread-local storage
        Ok(())
    }

    fn unregister(&mut self) -> Result<(), AppError> {
        if let Some(hook) = self.hook.take() {
            unsafe {
                UnhookWindowsHookEx(hook)
                    .map_err(|e| AppError::Hotkey(format!("failed to unhook: {}", e)))?;
            }
        }
        self.callback = None;
        Ok(())
    }

    fn is_registered(&self) -> bool {
        self.hook.is_some()
    }
}

impl Default for WindowsHotkeyManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for WindowsHotkeyManager {
    fn drop(&mut self) {
        if let Some(hook) = self.hook.take() {
            unsafe {
                let _ = UnhookWindowsHookEx(hook);
            }
        }
    }
}

/// Low-level keyboard hook procedure.
unsafe extern "system" fn keyboard_hook_proc(
    n_code: i32,
    w_param: WPARAM,
    l_param: LPARAM,
) -> LRESULT {
    if n_code >= 0 {
        let kb_struct = *(l_param.0 as *const KBDLLHOOKSTRUCT);
        let vk = kb_struct.vkCode;

        // TODO: dispatch to registered callback via global state
        // For now, just pass through
        let _ = (vk, w_param);
    }

    unsafe { CallNextHookEx(None, n_code, w_param, l_param) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_key_code() {
        assert_eq!(WindowsHotkeyManager::parse_key_code("RightAlt"), Some(0xA5));
        assert_eq!(WindowsHotkeyManager::parse_key_code("ralt"), Some(0xA5));
        assert_eq!(WindowsHotkeyManager::parse_key_code("F9"), Some(0x78));
        assert_eq!(WindowsHotkeyManager::parse_key_code("escape"), Some(0x1B));
        assert_eq!(WindowsHotkeyManager::parse_key_code("unknown"), None);
    }

    #[test]
    fn test_new_manager() {
        let manager = WindowsHotkeyManager::new();
        assert!(!manager.is_registered());
    }
}
