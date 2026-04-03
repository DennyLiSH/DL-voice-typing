pub mod mock;
#[cfg(feature = "whisper")]
pub mod whisper;

use crate::error::AppError;

/// Trait for speech-to-text engines.
///
/// Allows swapping backends (Whisper.cpp, cloud APIs) without changing consumers.
/// Uses native async fn in traits (Rust 1.75+).
#[allow(async_fn_in_trait)]
pub trait SpeechEngine: Send + Sync {
    /// Transcribe audio samples to text.
    /// `samples` is 16kHz mono f32 audio.
    async fn transcribe(&self, samples: &[f32]) -> Result<String, AppError>;

    /// Check if the engine's model is loaded and ready.
    fn is_ready(&self) -> bool;

    /// Check if the engine is running on GPU (vs CPU fallback).
    fn is_gpu_mode(&self) -> bool {
        false
    }

    /// Get the engine name for display.
    fn name(&self) -> &str;
}

/// Enum-based dispatch for speech engines (dyn-safe alternative).
///
/// Native `async fn` in traits is not dyn-compatible, so we use an enum
/// to dispatch between concrete engine types.
pub enum AnyEngine {
    #[cfg(feature = "whisper")]
    Whisper(whisper::WhisperEngine),
    Mock(mock::MockEngine),
}

impl AnyEngine {
    /// Create the appropriate engine based on the active feature flag.
    #[cfg(feature = "whisper")]
    pub fn new_whisper(model_path: std::path::PathBuf, language: String) -> Self {
        Self::Whisper(whisper::WhisperEngine::new(model_path, language))
    }

    pub fn new_mock(response: &str) -> Self {
        Self::Mock(mock::MockEngine::new(response))
    }

    /// Load the model (only meaningful for Whisper).
    pub fn load_model(&mut self) -> Result<(), AppError> {
        match self {
            #[cfg(feature = "whisper")]
            Self::Whisper(e) => e.load_model(),
            Self::Mock(_) => Ok(()),
        }
    }
}

impl SpeechEngine for AnyEngine {
    async fn transcribe(&self, samples: &[f32]) -> Result<String, AppError> {
        match self {
            #[cfg(feature = "whisper")]
            Self::Whisper(e) => e.transcribe(samples).await,
            Self::Mock(e) => e.transcribe(samples).await,
        }
    }

    fn is_ready(&self) -> bool {
        match self {
            #[cfg(feature = "whisper")]
            Self::Whisper(e) => e.is_ready(),
            Self::Mock(e) => e.is_ready(),
        }
    }

    fn is_gpu_mode(&self) -> bool {
        match self {
            #[cfg(feature = "whisper")]
            Self::Whisper(e) => e.is_gpu_mode(),
            Self::Mock(_) => false,
        }
    }

    fn name(&self) -> &str {
        match self {
            #[cfg(feature = "whisper")]
            Self::Whisper(e) => e.name(),
            Self::Mock(e) => e.name(),
        }
    }
}
