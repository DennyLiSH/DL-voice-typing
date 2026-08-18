use crate::error::AppError;

pub mod windows;

/// Parse a key name string to a virtual key code.
/// Delegates to the platform implementation.
pub fn parse_key_code(key: &str) -> Option<u32> {
    windows::WindowsHotkeyManager::parse_key_code(key)
}

/// Callback type for hotkey events.
pub type HotkeyCallback = Box<dyn Fn(HotkeyEvent) + Send + Sync>;

/// Hotkey event types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    /// Hotkey was pressed (start recording).
    Pressed,
    /// Hotkey was released (stop recording).
    Released,
}

/// Trait for hotkey management (platform-agnostic).
pub trait HotkeyManager: Send {
    /// Register a global hotkey with the given key name.
    /// Calls the callback on press/release events.
    fn register(&mut self, key: &str, callback: HotkeyCallback) -> Result<(), AppError>;

    /// Register the record-only hotkey (second, independent slot).
    /// Shares the same low-level keyboard hook as the primary hotkey;
    /// events are dispatched by virtual key code.
    fn register_record_only(&mut self, key: &str, callback: HotkeyCallback)
    -> Result<(), AppError>;

    /// Unregister all hotkeys (primary + record-only) and remove the hook.
    fn unregister(&mut self) -> Result<(), AppError>;

    /// Unregister only the record-only hotkey slot, leaving the primary
    /// hotkey active. Used when settings change just the record-only key.
    fn unregister_record_only(&mut self) -> Result<(), AppError>;

    /// Check if hotkey is currently registered.
    fn is_registered(&self) -> bool;
}
