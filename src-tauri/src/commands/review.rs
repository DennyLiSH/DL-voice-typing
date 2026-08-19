use crate::commands::pipeline_state::PipelineState;
use crate::error::CommandError;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
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
    /// Whether the review window was already shown on hotkey press
    /// (used when realtime_transcription + review_before_paste are both enabled).
    pub shown_on_press: Mutex<bool>,
}

impl PendingReview {
    pub fn new() -> Self {
        Self {
            text: Arc::new(Mutex::new(None)),
            data_saving: Mutex::new(None),
            foreground_hwnd: Mutex::new(None),
            shown_on_press: Mutex::new(false),
        }
    }

    /// Save the current foreground window handle.
    pub fn save_foreground(&self) {
        let hwnd = crate::win32::get_foreground_hwnd();
        if let Some(mut guard) = crate::util::lock_mutex(&self.foreground_hwnd, "foreground_hwnd") {
            *guard = Some(hwnd);
        }
    }

    /// Take and return the saved foreground window handle.
    pub fn take_foreground(&self) -> Option<isize> {
        crate::util::lock_mutex(&self.foreground_hwnd, "foreground_hwnd").and_then(|mut g| g.take())
    }

    /// Consume the data-saving metadata and update the JSON file with the final text.
    pub fn consume_and_save(&self, final_text: Option<&str>) {
        if let Some(mut guard) = crate::util::lock_mutex(&self.data_saving, "pending_data") {
            if let Some(review_data) = guard.take() {
                let _ = crate::data_saving::set_transcription_result(
                    &review_data.json_path,
                    &review_data.raw_transcription,
                    review_data.llm_text.as_deref(),
                    final_text,
                );
            }
        }
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
    let mut guard = pending.text.lock().map_err(CommandError::lock)?;
    let result = guard.take();
    debug!(
        "Review: get_review_text called, text={}",
        if result.is_some() { "Some" } else { "None" }
    );
    Ok(result)
}

/// Confirm the reviewed text and inject it via clipboard paste.
///
/// Runs async on the Tokio runtime to avoid blocking the main thread.
#[tauri::command]
pub async fn confirm_inject(
    text: String,
    ps: tauri::State<'_, PipelineState>,
) -> Result<(), CommandError> {
    ps.delivery().confirm_review(&ps, text).await
}

/// Cancel the review and return to idle.
///
/// Works from `Reviewing` (normal path), `Recording`, or `Transcribing`
/// (user cancelled before pipeline finished in realtime+review mode).
///
/// Runs async on the Tokio runtime to avoid blocking the main thread.
#[tauri::command]
pub async fn cancel_review(ps: tauri::State<'_, PipelineState>) -> Result<(), CommandError> {
    ps.delivery().cancel_review(&ps).await
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
