//! M6-a open-window hotkey callback factory.
//!
//! `make_open_window_callback` returns a `HotkeyCallback` that opens the
//! configured window (`settings` or `transcribe`) when invoked. The callback
//! is intentionally decoupled from `PipelineState` — it only needs an
//! `AppHandle` to call `show + set_focus` on the named webview window.

use tauri::{AppHandle, Manager, Runtime};

use crate::hotkey::{HotkeyCallback, HotkeyEvent};

/// Window targets that can be opened by a global hotkey (M6-a).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WindowKind {
    Settings,
    Transcribe,
}

impl WindowKind {
    fn window_label(self) -> &'static str {
        match self {
            WindowKind::Settings => "settings",
            WindowKind::Transcribe => "transcribe",
        }
    }
}

/// Build a `HotkeyCallback` that opens the given window on press.
pub(crate) fn make_open_window_callback<R: Runtime>(
    app: AppHandle<R>,
    kind: WindowKind,
) -> HotkeyCallback {
    let label = kind.window_label();
    Box::new(move |_event: HotkeyEvent| {
        if let Some(window) = app.get_webview_window(label) {
            let _ = window.show();
            let _ = window.set_focus();
        }
    })
}
