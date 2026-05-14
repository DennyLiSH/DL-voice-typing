use crate::commands::review::ReviewData;
use std::sync::{Arc, Mutex};
use tauri::Manager;

/// Abstract access to review-before-paste state.
/// Decouples the pipeline from Tauri's managed state (PendingReview).
pub(crate) trait ReviewProvider: Send + Sync {
    /// Store the text for the review window to display.
    fn store_text(&self, text: String);
    /// Save the current foreground window handle.
    fn save_foreground(&self);
    /// Take and return the saved foreground window handle.
    #[allow(dead_code)]
    fn take_foreground(&self) -> Option<isize>;
    /// Read the shown_on_press flag.
    fn was_shown_on_press(&self) -> bool;
    /// Set the shown_on_press flag.
    fn set_shown_on_press(&self, value: bool);
    /// Store data-saving metadata for confirm/cancel to consume later.
    fn store_review_data(&self, data: ReviewData);
}

/// Production implementation that delegates to Tauri's managed `PendingReview`.
pub(crate) struct TauriReviewProvider {
    app: tauri::AppHandle,
}

impl TauriReviewProvider {
    pub fn new(app: tauri::AppHandle) -> Self {
        Self { app }
    }
}

impl ReviewProvider for TauriReviewProvider {
    fn store_text(&self, text: String) {
        if let Some(pending) = self.app.try_state::<super::review::PendingReview>() {
            if let Some(mut guard) = crate::util::lock_mutex(&pending.text, "pending_text") {
                *guard = Some(text);
            }
        }
    }

    fn save_foreground(&self) {
        if let Some(pending) = self.app.try_state::<super::review::PendingReview>() {
            let pending: &super::review::PendingReview = &pending;
            pending.save_foreground();
        }
    }

    fn take_foreground(&self) -> Option<isize> {
        self.app
            .try_state::<super::review::PendingReview>()
            .and_then(|p: tauri::State<'_, super::review::PendingReview>| p.take_foreground())
    }

    fn was_shown_on_press(&self) -> bool {
        self.app
            .try_state::<super::review::PendingReview>()
            .map(|p| {
                crate::util::lock_mutex(&p.shown_on_press, "shown_on_press")
                    .map(|g| *g)
                    .unwrap_or(false)
            })
            .unwrap_or(false)
    }

    fn set_shown_on_press(&self, value: bool) {
        if let Some(pending) = self.app.try_state::<super::review::PendingReview>() {
            if let Some(mut guard) =
                crate::util::lock_mutex(&pending.shown_on_press, "shown_on_press")
            {
                *guard = value;
            }
        }
    }

    fn store_review_data(&self, data: ReviewData) {
        if let Some(pending) = self.app.try_state::<super::review::PendingReview>() {
            if let Some(mut guard) = crate::util::lock_mutex(&pending.data_saving, "pending_data") {
                *guard = Some(data);
            }
        }
    }
}

/// Mock implementation for testing.
#[cfg(test)]
pub(crate) struct MockReviewProvider {
    text: Arc<Mutex<Option<String>>>,
    foreground: Arc<Mutex<Option<isize>>>,
    shown_on_press: Arc<Mutex<bool>>,
    review_data: Arc<Mutex<Option<ReviewData>>>,
}

#[cfg(test)]
impl MockReviewProvider {
    pub fn new() -> Self {
        Self {
            text: Arc::new(Mutex::new(None)),
            foreground: Arc::new(Mutex::new(None)),
            shown_on_press: Arc::new(Mutex::new(false)),
            review_data: Arc::new(Mutex::new(None)),
        }
    }

    pub fn get_text(&self) -> Option<String> {
        self.text
            .lock()
            .ok()
            .and_then(|g: std::sync::MutexGuard<'_, Option<String>>| g.clone())
    }

    pub fn get_foreground(&self) -> Option<isize> {
        self.foreground
            .lock()
            .ok()
            .and_then(|mut g: std::sync::MutexGuard<'_, Option<isize>>| g.take())
    }
}

#[cfg(test)]
impl ReviewProvider for MockReviewProvider {
    fn store_text(&self, text: String) {
        if let Some(mut guard) = crate::util::lock_mutex(&self.text, "mock_text") {
            *guard = Some(text);
        }
    }

    fn save_foreground(&self) {
        if let Some(mut guard) = crate::util::lock_mutex(&self.foreground, "mock_foreground") {
            *guard = Some(42); // sentinel value
        }
    }

    fn take_foreground(&self) -> Option<isize> {
        crate::util::lock_mutex(&self.foreground, "mock_foreground")
            .and_then(|mut g| g.take())
    }

    fn was_shown_on_press(&self) -> bool {
        crate::util::lock_mutex(&self.shown_on_press, "mock_shown_on_press")
            .map(|g| *g)
            .unwrap_or(false)
    }

    fn set_shown_on_press(&self, value: bool) {
        if let Some(mut guard) =
            crate::util::lock_mutex(&self.shown_on_press, "mock_shown_on_press")
        {
            *guard = value;
        }
    }

    fn store_review_data(&self, data: ReviewData) {
        if let Some(mut guard) = crate::util::lock_mutex(&self.review_data, "mock_review_data") {
            *guard = Some(data);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_store_and_get_text() {
        let provider = MockReviewProvider::new();
        provider.store_text("hello".to_string());
        assert_eq!(provider.get_text(), Some("hello".to_string()));
    }

    #[test]
    fn test_foreground_lifecycle() {
        let provider = MockReviewProvider::new();
        assert!(provider.take_foreground().is_none());
        provider.save_foreground();
        assert!(provider.take_foreground().is_some());
        assert!(provider.take_foreground().is_none());
    }

    #[test]
    fn test_shown_on_press_flag() {
        let provider = MockReviewProvider::new();
        assert!(!provider.was_shown_on_press());
        provider.set_shown_on_press(true);
        assert!(provider.was_shown_on_press());
        provider.set_shown_on_press(false);
        assert!(!provider.was_shown_on_press());
    }

    #[test]
    fn test_store_review_data() {
        let provider = MockReviewProvider::new();
        let data = ReviewData {
            json_path: PathBuf::from("test.json"),
            raw_transcription: "raw".to_string(),
            llm_text: Some("corrected".to_string()),
        };
        provider.store_review_data(data);
        let stored = crate::util::lock_mutex(&provider.review_data, "mock_review_data")
            .and_then(|g| g.as_ref().map(|d| d.raw_transcription.clone()));
        assert_eq!(stored, Some("raw".to_string()));
    }
}
