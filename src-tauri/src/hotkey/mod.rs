use crate::error::AppError;

pub mod windows;

/// Callback type for hotkey events.
pub type HotkeyCallback = Box<dyn Fn(HotkeyEvent) + Send>;

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

    /// Unregister the hotkey.
    fn unregister(&mut self) -> Result<(), AppError>;

    /// Check if hotkey is currently registered.
    fn is_registered(&self) -> bool;
}
