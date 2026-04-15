use crate::error::CommandError;
use crate::state::StateMachine;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Manager};
use tracing::debug;

/// Metadata from the transcription pipeline needed for data-saving JSON update.
pub(crate) struct ReviewData {
    pub json_path: PathBuf,
    pub raw_transcription: String,
    pub llm_text: Option<String>,
}

/// Shared state for passing review text to the review window.
/// The async task stores text here, and the review window fetches it on load.
pub struct PendingReview {
    /// Text for the review window to display. Consumed by `get_review_text`.
    pub text: Arc<Mutex<Option<String>>>,
    /// Data-saving metadata. Consumed by `confirm_inject` or `cancel_review`.
    pub(crate) data_saving: Mutex<Option<ReviewData>>,
    /// Foreground window HWND before review window appeared. Used to restore focus.
    foreground_hwnd: Mutex<Option<isize>>,
}

impl PendingReview {
    pub fn new() -> Self {
        Self {
            text: Arc::new(Mutex::new(None)),
            data_saving: Mutex::new(None),
            foreground_hwnd: Mutex::new(None),
        }
    }

    /// Save the current foreground window handle.
    pub fn save_foreground(&self) {
        use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
        let hwnd = unsafe { GetForegroundWindow() };
        if let Ok(mut guard) = self.foreground_hwnd.lock() {
            *guard = Some(hwnd.0 as isize);
        }
    }

    /// Take and return the saved foreground window handle.
    pub fn take_foreground(&self) -> Option<isize> {
        self.foreground_hwnd.lock().ok().and_then(|mut g| g.take())
    }
}

impl Default for PendingReview {
    fn default() -> Self {
        Self::new()
    }
}

/// Fetch the pending review text (called by review window on load).
#[tauri::command]
pub fn get_review_text(
    pending: tauri::State<'_, PendingReview>,
) -> Result<Option<String>, CommandError> {
    let mut guard = pending.text.lock().map_err(|e| CommandError {
        code: "LOCK".to_string(),
        message: e.to_string(),
    })?;
    let result = guard.take();
    debug!(
        "Review: get_review_text called, text={}",
        if result.is_some() { "Some" } else { "None" }
    );
    Ok(result)
}

/// Confirm the reviewed text and inject it via clipboard paste.
#[tauri::command]
pub fn confirm_inject(
    text: String,
    state_machine: tauri::State<'_, Arc<Mutex<StateMachine>>>,
    clipboard: tauri::State<'_, Arc<Mutex<crate::clipboard::ClipboardManager>>>,
    app: tauri::AppHandle,
) -> Result<(), CommandError> {
    // 1. Transition Reviewing → Injecting
    {
        let mut sm = state_machine.lock().map_err(|e| CommandError {
            code: "LOCK".to_string(),
            message: e.to_string(),
        })?;
        sm.reviewing_to_injecting(text.clone())
            .map_err(|e| CommandError {
                code: "STATE".to_string(),
                message: e.to_string(),
            })?;
    }

    // 2. Restore focus to target app, then hide review window.
    let saved_hwnd = app
        .try_state::<PendingReview>()
        .and_then(|p| p.take_foreground());
    if let Some(hwnd_val) = saved_hwnd {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
        unsafe {
            let _ = SetForegroundWindow(HWND(hwnd_val as *mut _));
        }
    }
    if let Some(win) = app.get_webview_window("review") {
        let _ = win.hide();
    }

    // 3. Inject text (clipboard write + Ctrl+V + restore saved clipboard)
    {
        let cb = clipboard.lock().map_err(|e| CommandError {
            code: "LOCK".to_string(),
            message: e.to_string(),
        })?;
        if let Err(e) = cb.inject_text(&text) {
            let _ = app.emit("injection-error", e.to_string());
        }
    }

    // 4. Transition → Idle
    {
        let mut sm = state_machine.lock().map_err(|e| CommandError {
            code: "LOCK".to_string(),
            message: e.to_string(),
        })?;
        let _ = sm.finish_injecting();
    }

    // 5. Emit injection-complete + hide floating indicator
    let _ = app.emit("injection-complete", ());
    if let Some(win) = app.get_webview_window("floating") {
        let _ = win.hide();
    }

    // 6. Update data-saving JSON with the final reviewed text.
    if let Some(pending) = app.try_state::<PendingReview>() {
        if let Ok(mut guard) = pending.data_saving.lock() {
            if let Some(review_data) = guard.take() {
                let _ = crate::data_saving::update_json_with_text(
                    &review_data.json_path,
                    &review_data.raw_transcription,
                    review_data.llm_text.as_deref(),
                    Some(&text),
                );
            }
        }
    }

    Ok(())
}

/// Cancel the review and return to idle.
#[tauri::command]
pub fn cancel_review(
    state_machine: tauri::State<'_, Arc<Mutex<StateMachine>>>,
    clipboard: tauri::State<'_, Arc<Mutex<crate::clipboard::ClipboardManager>>>,
    app: tauri::AppHandle,
) -> Result<(), CommandError> {
    // 1. Cancel Reviewing → Idle
    {
        let mut sm = state_machine.lock().map_err(|e| CommandError {
            code: "LOCK".to_string(),
            message: e.to_string(),
        })?;
        sm.cancel_reviewing().map_err(|e| CommandError {
            code: "STATE".to_string(),
            message: e.to_string(),
        })?;
    }

    // 2. Restore clipboard
    {
        let mut cb = clipboard.lock().map_err(|e| CommandError {
            code: "LOCK".to_string(),
            message: e.to_string(),
        })?;
        let _ = cb.restore();
    }

    // 3. Restore focus to target app, then hide windows.
    let saved_hwnd = app
        .try_state::<PendingReview>()
        .and_then(|p| p.take_foreground());
    if let Some(hwnd_val) = saved_hwnd {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
        unsafe {
            let _ = SetForegroundWindow(HWND(hwnd_val as *mut _));
        }
    }
    if let Some(win) = app.get_webview_window("floating") {
        let _ = win.hide();
    }
    if let Some(win) = app.get_webview_window("review") {
        let _ = win.hide();
    }

    // 4. Update data-saving JSON: preserve raw transcription, mark no final text.
    if let Some(pending) = app.try_state::<PendingReview>() {
        if let Ok(mut guard) = pending.data_saving.lock() {
            if let Some(review_data) = guard.take() {
                let _ = crate::data_saving::update_json_with_text(
                    &review_data.json_path,
                    &review_data.raw_transcription,
                    review_data.llm_text.as_deref(),
                    None,
                );
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_foreground_save_and_take() {
        let review = PendingReview::new();
        review.save_foreground();
        let hwnd = review.take_foreground();
        assert!(hwnd.is_some());
        let hwnd2 = review.take_foreground();
        assert!(hwnd2.is_none());
    }

    #[test]
    fn test_take_foreground_empty() {
        let review = PendingReview::new();
        assert!(review.take_foreground().is_none());
    }

    #[test]
    fn test_take_foreground_idempotent() {
        let review = PendingReview::new();
        review.save_foreground();
        assert!(review.take_foreground().is_some());
        assert!(review.take_foreground().is_none());
        assert!(review.take_foreground().is_none());
    }
}
