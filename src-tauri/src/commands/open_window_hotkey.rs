//! M6-a open-window hotkey callback factory.
//!
//! `make_open_window_callback` returns a `HotkeyCallback` that opens the
//! configured window (`settings` or `transcribe`) on press, delegating to
//! the tray show-or-build helpers so a window that has never been created
//! this session still opens (the app ships with no pre-created windows).
//! The callback is intentionally decoupled from `PipelineState` — it only
//! needs an `AppHandle`.

use tauri::{AppHandle, Runtime};

use crate::hotkey::{HotkeyCallback, HotkeyEvent};

/// Window targets that can be opened by a global hotkey (M6-a).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WindowKind {
    Settings,
    Transcribe,
}

/// Build a `HotkeyCallback` that opens the given window on press.
pub(crate) fn make_open_window_callback<R: Runtime>(
    app: AppHandle<R>,
    kind: WindowKind,
) -> HotkeyCallback {
    Box::new(move |event: HotkeyEvent| match event {
        // Window opens on press; release is a no-op (aligns with the
        // hotkey_pipeline press/release dispatch — the former |_event|
        // form fired twice per tap, the second set_focus stole focus).
        HotkeyEvent::Pressed => {
            let handle = app.clone();
            // Window creation must happen on the main thread.
            let _ = app.run_on_main_thread(move || match kind {
                WindowKind::Settings => crate::tray::open_settings_window(&handle),
                WindowKind::Transcribe => crate::tray::open_transcribe_window(&handle),
            });
        }
        HotkeyEvent::Released => {}
    })
}
