use crate::error::AppError;
use crate::hotkey::{HotkeyCallback, HotkeyEvent, HotkeyManager};
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, HHOOK, KBDLLHOOKSTRUCT, KBDLLHOOKSTRUCT_FLAGS, LLKHF_INJECTED,
    SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN,
    WM_SYSKEYUP,
};

type SlotCallback = Arc<dyn Fn(HotkeyEvent) + Send + Sync>;

/// Global state shared between WindowsHotkeyManager and the hook procedure.
/// Two independent slots (primary voice-input hotkey + record-only hotkey)
/// dispatched by virtual key code from a single low-level hook.
#[derive(Default)]
struct HookState {
    primary: Option<(u32, SlotCallback)>,
    record_only: Option<(u32, SlotCallback)>,
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
            .ok_or_else(|| AppError::Hotkey(format!("unknown key: {key}")))?;

        self.ensure_hook()?;
        let mut state = HOOK_STATE
            .lock()
            .map_err(|e| AppError::Hotkey(format!("global state lock poisoned: {e}")))?;
        let hook_state = state.get_or_insert_with(HookState::default);
        hook_state.primary = Some((vk_code, Arc::from(callback)));
        Ok(())
    }

    fn register_record_only(
        &mut self,
        key: &str,
        callback: HotkeyCallback,
    ) -> Result<(), AppError> {
        let vk_code = WindowsHotkeyManager::parse_key_code(key)
            .ok_or_else(|| AppError::Hotkey(format!("unknown key: {key}")))?;

        self.ensure_hook()?;
        let mut state = HOOK_STATE
            .lock()
            .map_err(|e| AppError::Hotkey(format!("global state lock poisoned: {e}")))?;
        let hook_state = state.get_or_insert_with(HookState::default);
        if let Some((primary_vk, _)) = &hook_state.primary {
            if *primary_vk == vk_code {
                return Err(AppError::Hotkey(
                    "record-only hotkey must differ from the primary hotkey".to_string(),
                ));
            }
        }
        hook_state.record_only = Some((vk_code, Arc::from(callback)));
        Ok(())
    }

    fn unregister(&mut self) -> Result<(), AppError> {
        self.remove_hook()?;
        // Clear global state (both slots).
        if let Ok(mut state) = HOOK_STATE.lock() {
            *state = None;
        }
        Ok(())
    }

    fn unregister_record_only(&mut self) -> Result<(), AppError> {
        if let Ok(mut state) = HOOK_STATE.lock() {
            if let Some(hook_state) = state.as_mut() {
                hook_state.record_only = None;
                // Remove the hook entirely when no slot remains.
                if hook_state.primary.is_none() {
                    *state = None;
                    drop(state);
                    self.remove_hook()?;
                }
            }
        }
        Ok(())
    }

    fn is_registered(&self) -> bool {
        self.hook.is_some()
    }
}

impl WindowsHotkeyManager {
    /// Install the low-level keyboard hook if not already installed.
    fn ensure_hook(&mut self) -> Result<(), AppError> {
        if self.hook.is_some() {
            return Ok(());
        }
        unsafe {
            let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_proc), None, 0)
                .map_err(|e| AppError::Hotkey(format!("failed to set hook: {e}")))?;
            self.hook = Some(hook);
        }
        Ok(())
    }

    /// Remove the low-level keyboard hook if installed.
    fn remove_hook(&mut self) -> Result<(), AppError> {
        if let Some(hook) = self.hook.take() {
            // SAFETY: UnhookWindowsHookEx removes a hook installed by SetWindowsHookExW.
            // `hook` is a valid HHOOK from a successful SetWindowsHookExW call.
            unsafe {
                UnhookWindowsHookEx(hook)
                    .map_err(|e| AppError::Hotkey(format!("failed to unhook: {e}")))?;
            }
        }
        Ok(())
    }
}

impl Default for WindowsHotkeyManager {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: HHOOK is a Windows hook handle (pointer) not Send/Sync by default.
// WindowsHotkeyManager is only accessed through `Mutex<WindowsHotkeyManager>`
// (see lib.rs). The Mutex guarantees exclusive access. The hook handle
// is created in register(), used in the static hook proc (via separate HOOK_STATE
// Mutex), and destroyed in unregister()/Drop — all under Mutex protection.
unsafe impl Send for WindowsHotkeyManager {}
unsafe impl Sync for WindowsHotkeyManager {}

impl Drop for WindowsHotkeyManager {
    fn drop(&mut self) {
        if let Some(hook) = self.hook.take() {
            // SAFETY: UnhookWindowsHookEx removes a hook installed by SetWindowsHookExW.
            // `hook` is a valid HHOOK. Best-effort cleanup in Drop — error is discarded.
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
///
/// # Safety
/// This function is called by Windows from the hook chain. `l_param` must point to a
/// valid `KBDLLHOOKSTRUCT`. The function reads global state through a Mutex and clones
/// any data before releasing the lock to avoid re-entrant deadlocks.
unsafe extern "system" fn keyboard_hook_proc(
    n_code: i32,
    w_param: WPARAM,
    l_param: LPARAM,
) -> LRESULT {
    if n_code >= 0 {
        // SAFETY: Windows guarantees l_param points to a valid KBDLLHOOKSTRUCT when
        // called from a WH_KEYBOARD_LL hook with n_code >= 0.
        let kb_struct = unsafe { *(l_param.0 as *const KBDLLHOOKSTRUCT) };
        let vk = kb_struct.vkCode;

        // Ignore synthetic (injected) key events from SendInput/keybd_event.
        // This prevents tools like Ditto from accidentally triggering the hotkey.
        if kb_struct.flags & LLKHF_INJECTED != KBDLLHOOKSTRUCT_FLAGS(0) {
            // SAFETY: CallNextHookEx passes the event to the next hook in the chain.
            // All parameters are forwarded unchanged from our hook proc.
            return unsafe { CallNextHookEx(None, n_code, w_param, l_param) };
        }

        // Determine event type from w_param.
        let event = match w_param.0 as u32 {
            WM_KEYDOWN | WM_SYSKEYDOWN => Some(HotkeyEvent::Pressed),
            WM_KEYUP | WM_SYSKEYUP => Some(HotkeyEvent::Released),
            _ => None,
        };

        if let Some(event) = event {
            // Find the matching slot and clone its callback out of the lock.
            // We must not hold the Mutex while calling the callback (deadlock risk
            // if the callback tries to unregister or triggers window operations
            // that re-enter the message loop, e.g., SetFocus, ShowWindow).
            // The callback still runs on the main thread (required for Win32
            // window operations and cpal audio capture).
            let callback = {
                let state = HOOK_STATE.lock();
                match state {
                    Ok(guard) => guard.as_ref().and_then(|hs| find_callback(hs, vk)),
                    Err(_) => None,
                }
            };
            if let Some(cb) = callback {
                cb(event);
            }
        }
    }

    // SAFETY: CallNextHookEx passes the event to the next hook in the chain.
    // All parameters are forwarded unchanged from our hook proc.
    unsafe { CallNextHookEx(None, n_code, w_param, l_param) }
}

/// Dispatch a virtual key code to the matching slot's callback.
/// Primary slot wins if both slots somehow hold the same vk (config
/// validation and register_record_only both reject that case).
fn find_callback(hs: &HookState, vk: u32) -> Option<SlotCallback> {
    if let Some((vk_code, cb)) = &hs.primary {
        if *vk_code == vk {
            return Some(cb.clone());
        }
    }
    if let Some((vk_code, cb)) = &hs.record_only {
        if *vk_code == vk {
            return Some(cb.clone());
        }
    }
    None
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

    #[test]
    fn test_hotkey_manager_is_send_sync_via_mutex() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<std::sync::Mutex<WindowsHotkeyManager>>();
    }

    fn dummy_callback() -> SlotCallback {
        Arc::new(|_event: HotkeyEvent| {})
    }

    #[test]
    fn test_find_callback_dispatches_by_vk() {
        let primary_cb = dummy_callback();
        let record_cb = dummy_callback();
        let hs = HookState {
            primary: Some((0xA3, primary_cb.clone())),
            record_only: Some((0xA5, record_cb.clone())),
        };
        let found_primary = find_callback(&hs, 0xA3);
        assert!(found_primary.is_some());
        let found_record = find_callback(&hs, 0xA5);
        assert!(found_record.is_some());
        // Distinct slots: the two callbacks are different allocations.
        if let (Some(p), Some(r)) = (found_primary, found_record) {
            assert!(!Arc::ptr_eq(&p, &r));
            assert!(Arc::ptr_eq(&p, &primary_cb));
            assert!(Arc::ptr_eq(&r, &record_cb));
        }
    }

    #[test]
    fn test_find_callback_returns_none_for_unregistered_vk() {
        let hs = HookState {
            primary: Some((0xA3, dummy_callback())),
            record_only: Some((0xA5, dummy_callback())),
        };
        assert!(find_callback(&hs, 0x70).is_none());
    }

    #[test]
    fn test_find_callback_handles_empty_slots() {
        let hs = HookState::default();
        assert!(find_callback(&hs, 0xA3).is_none());

        let only_record = HookState {
            primary: None,
            record_only: Some((0xA5, dummy_callback())),
        };
        assert!(only_record.primary.is_none());
        assert!(find_callback(&only_record, 0xA3).is_none());
        assert!(find_callback(&only_record, 0xA5).is_some());
    }
}
