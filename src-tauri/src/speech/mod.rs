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

    /// Get the engine name for display.
    fn name(&self) -> &str;
}
