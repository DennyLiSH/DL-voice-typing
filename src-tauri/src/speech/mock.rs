use crate::error::AppError;
use crate::speech::SpeechEngine;

/// Mock speech engine for testing.
pub struct MockEngine {
    response: String,
    ready: bool,
    segments: Option<Vec<crate::speech::Segment>>,
}

impl MockEngine {
    /// Create a mock engine that always returns the given fixed response.
    pub fn new(response: impl Into<String>) -> Self {
        Self {
            response: response.into(),
            ready: true,
            segments: None,
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

    /// Override `transcribe_with_segments_sync` to return the given segments
    /// (e.g., an empty vec to simulate whisper producing zero segments).
    pub fn with_segments(mut self, segments: Vec<crate::speech::Segment>) -> Self {
        self.segments = Some(segments);
        self
    }
}

impl SpeechEngine for MockEngine {
    fn transcribe_sync(&self, _samples: &[f32]) -> Result<String, AppError> {
        if !self.ready {
            return Err(AppError::Speech("mock engine not ready".to_string()));
        }
        Ok(self.response.clone())
    }

    fn transcribe_with_segments_sync(
        &self,
        samples: &[f32],
        cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
        progress: Box<dyn Fn(u8) + Send + Sync>,
    ) -> Result<Vec<crate::speech::Segment>, AppError> {
        if let Some(segments) = &self.segments {
            use std::sync::atomic::Ordering;
            if cancel.load(Ordering::Relaxed) {
                return Err(AppError::Speech(
                    crate::speech::CANCELLED_MESSAGE.to_string(),
                ));
            }
            progress(100);
            return Ok(segments.clone());
        }
        // Mirror the trait default: single segment spanning the whole audio.
        use std::sync::atomic::Ordering;
        if cancel.load(Ordering::Relaxed) {
            return Err(AppError::Speech(
                crate::speech::CANCELLED_MESSAGE.to_string(),
            ));
        }
        let text = self.transcribe_sync(samples)?;
        progress(100);
        let duration_ms = (samples.len() as u64 * 1000 / crate::audio::TARGET_SAMPLE_RATE as u64)
            .min(u32::MAX as u64) as u32;
        Ok(vec![crate::speech::Segment {
            text,
            start_ms: 0,
            end_ms: duration_ms,
        }])
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

    #[test]
    fn test_mock_with_segments_override() {
        use std::sync::Arc;
        use std::sync::atomic::AtomicBool;
        let engine = MockEngine::new("ignored").with_segments(vec![]);
        let result = engine.transcribe_with_segments_sync(
            &[0.0f32; 100],
            Arc::new(AtomicBool::new(false)),
            Box::new(|_| {}),
        );
        assert!(result.is_ok());
        assert_eq!(result.unwrap_or_default().len(), 0);
    }
}
