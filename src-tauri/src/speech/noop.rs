#[cfg(not(feature = "whisper"))]
use crate::error::AppError;
use crate::speech::SpeechEngine;

/// No-op speech engine used when the `whisper` feature is disabled.
/// Always reports not-ready and returns Err on transcribe (semantic alignment
/// with `is_ready()=false`: callers must check readiness before invoking).
pub struct NoopEngine;

impl NoopEngine {
    pub fn new() -> Self {
        Self
    }
}

impl SpeechEngine for NoopEngine {
    fn transcribe_sync(&self, _samples: &[f32]) -> Result<String, AppError> {
        Err(AppError::Speech(
            "Speech engine not available in this build (whisper feature disabled)".to_string(),
        ))
    }

    fn is_ready(&self) -> bool {
        false
    }

    fn name(&self) -> &str {
        "noop"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_noop_returns_error_when_whisper_disabled() {
        let engine = NoopEngine::new();
        let result = engine.transcribe_sync(&[0.5f32; 100]);
        assert!(
            result.is_err(),
            "transcribe_sync should return Err when whisper feature disabled"
        );
    }

    #[test]
    fn test_noop_is_not_ready() {
        let engine = NoopEngine::new();
        assert!(!engine.is_ready());
    }

    #[test]
    fn test_noop_compute_mode_is_unloaded() {
        let engine = NoopEngine::new();
        assert_eq!(engine.compute_mode(), "unloaded");
    }
}
