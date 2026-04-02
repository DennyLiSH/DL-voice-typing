use crate::error::AppError;
use crate::hotkey::{HotkeyCallback, HotkeyEvent, HotkeyManager};
use std::sync::Mutex;
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, HHOOK, KBDLLHOOKSTRUCT, KBDLLHOOKSTRUCT_FLAGS, LLKHF_INJECTED,
    SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN,
    WM_SYSKEYUP,
};

/// Global state shared between WindowsHotkeyManager and the hook procedure.
struct HookState {
    key_code: u32,
    callback: Option<Box<dyn Fn(HotkeyEvent) + Send>>,
}

static HOOK_STATE: Mutex<Option<HookState>> = Mutex::new(None);

/// Windows global keyboard hook implementation.
pub struct WindowsHotkeyManager {
    hook: Option<HHOOK>,
}

impl WindowsHotkeyManager {
    pub fn new() -> Self {
        Self { hook: None }
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
        let vk_code = WindowsHotkeyManager::parse_key_code(key)
            .ok_or_else(|| AppError::Hotkey(format!("unknown key: {}", key)))?;

        // Store callback + key_code in global state for the hook proc to access.
        {
            let mut state = HOOK_STATE
                .lock()
                .map_err(|e| AppError::Hotkey(format!("global state lock poisoned: {}", e)))?;
            *state = Some(HookState {
                key_code: vk_code,
                callback: Some(callback),
            });
        }

        unsafe {
            let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_proc), None, 0)
                .map_err(|e| AppError::Hotkey(format!("failed to set hook: {}", e)))?;

            self.hook = Some(hook);
        }

        Ok(())
    }

    fn unregister(&mut self) -> Result<(), AppError> {
        if let Some(hook) = self.hook.take() {
            unsafe {
                UnhookWindowsHookEx(hook)
                    .map_err(|e| AppError::Hotkey(format!("failed to unhook: {}", e)))?;
            }
        }
        // Clear global state.
        if let Ok(mut state) = HOOK_STATE.lock() {
            *state = None;
        }
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

// HHOOK contains *mut c_void which is not Send/Sync by default,
// but the Windows hook handle is safe to send across threads on Windows.
unsafe impl Send for WindowsHotkeyManager {}
unsafe impl Sync for WindowsHotkeyManager {}

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
///
/// Reads the registered key_code and callback from global state,
/// detects press/release, and invokes the callback.
unsafe extern "system" fn keyboard_hook_proc(
    n_code: i32,
    w_param: WPARAM,
    l_param: LPARAM,
) -> LRESULT {
    if n_code >= 0 {
        let kb_struct = unsafe { *(l_param.0 as *const KBDLLHOOKSTRUCT) };
        let vk = kb_struct.vkCode;

        // Ignore synthetic (injected) key events from SendInput/keybd_event.
        // This prevents tools like Ditto from accidentally triggering the hotkey.
        if kb_struct.flags & LLKHF_INJECTED != KBDLLHOOKSTRUCT_FLAGS(0) {
            return unsafe { CallNextHookEx(None, n_code, w_param, l_param) };
        }

        // Determine event type from w_param.
        let event = match w_param.0 as u32 {
            WM_KEYDOWN | WM_SYSKEYDOWN => Some(HotkeyEvent::Pressed),
            WM_KEYUP | WM_SYSKEYUP => Some(HotkeyEvent::Released),
            _ => None,
        };

        if let Some(event) = event {
            // Access global state — match key and invoke callback.
            // We must not hold the Mutex while calling the callback (deadlock risk
            // if the callback tries to unregister), so clone what we need.
            let matched = {
                let state = HOOK_STATE.lock();
                match state {
                    Ok(guard) => {
                        if let Some(ref hook_state) = *guard {
                            hook_state.key_code == vk
                        } else {
                            false
                        }
                    }
                    Err(_) => false,
                }
            };

            if matched {
                // Re-lock only to get a reference to the callback.
                // The callback itself must not call register/unregister.
                let state = HOOK_STATE.lock();
                if let Ok(guard) = state {
                    if let Some(ref hook_state) = *guard {
                        if let Some(ref callback) = hook_state.callback {
                            callback(event);
                        }
                    }
                }
            }
        }
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
