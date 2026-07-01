use crate::error::AppError;
use crate::speech::SpeechEngine;

/// Mock speech engine for testing.
pub struct MockEngine {
    response: String,
    ready: bool,
}

impl MockEngine {
    /// Create a mock engine that always returns the given fixed response.
    pub fn new(response: impl Into<String>) -> Self {
        Self {
            response: response.into(),
            ready: true,
        }
    }

    /// Change the fixed response returned by `transcribe_sync`.
    pub fn set_response(&mut self, response: impl Into<String>) {
        self.response = response.into();
    }

    /// Set whether the engine reports itself as ready.
    pub fn set_ready(&mut self, ready: bool) {
        self.ready = ready;
    }
}

impl SpeechEngine for MockEngine {
    fn transcribe_sync(&self, _samples: &[f32]) -> Result<String, AppError> {
        if !self.ready {
            return Err(AppError::Speech("mock engine not ready".to_string()));
        }
        Ok(self.response.clone())
    }

    fn is_ready(&self) -> bool {
        self.ready
    }

    fn name(&self) -> &str {
        "Mock"
    }
}

mod tests {
    use super::*;

    #[test]
    fn test_mock_transcribe_sync() {
        let engine = MockEngine::new("hello world");
        let result = engine.transcribe_sync(&[0.0]);
        assert!(result.is_ok(), "transcribe_sync should succeed");
        assert_eq!(result.unwrap_or_default(), "hello world");
    }

    #[test]
    fn test_mock_not_ready() {
        let mut engine = MockEngine::new("test");
        engine.set_ready(false);
        let result = engine.transcribe_sync(&[0.0]);
        assert!(
            result.is_err(),
            "transcribe_sync should fail when not ready"
        );
    }

    #[test]
    fn test_mock_name() {
        let engine = MockEngine::new("test");
        assert_eq!(engine.name(), "Mock");
    }

    #[test]
    fn test_mock_transcribe_sync_with_context_ignores_context() {
        let engine = MockEngine::new("hello");
        let with_ctx = engine.transcribe_sync_with_context(&[0.5f32; 100], Some("ignored context"));
        let without_ctx = engine.transcribe_sync(&[0.5f32; 100]);
        assert!(
            with_ctx.is_ok(),
            "transcribe_sync_with_context should succeed"
        );
        assert_eq!(with_ctx.unwrap_or_default(), "hello");
        assert_eq!(without_ctx.unwrap_or_default(), "hello");
    }
}
