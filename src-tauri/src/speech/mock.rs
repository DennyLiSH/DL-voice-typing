use crate::error::AppError;
use crate::speech::SpeechEngine;

/// Mock speech engine for testing.
pub struct MockEngine {
    response: String,
    ready: bool,
}

impl MockEngine {
    pub fn new(response: impl Into<String>) -> Self {
        Self {
            response: response.into(),
            ready: true,
        }
    }

    pub fn set_response(&mut self, response: impl Into<String>) {
        self.response = response.into();
    }

    pub fn set_ready(&mut self, ready: bool) {
        self.ready = ready;
    }
}

impl SpeechEngine for MockEngine {
    async fn transcribe(&self, _samples: &[f32]) -> Result<String, AppError> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_transcribe() {
        let engine = MockEngine::new("hello world");
        let result = engine.transcribe(&[0.0]).await.unwrap();
        assert_eq!(result, "hello world");
    }

    #[tokio::test]
    async fn test_mock_not_ready() {
        let mut engine = MockEngine::new("test");
        engine.set_ready(false);
        assert!(engine.transcribe(&[0.0]).await.is_err());
    }

    #[test]
    fn test_mock_name() {
        let engine = MockEngine::new("test");
        assert_eq!(engine.name(), "Mock");
    }
}
