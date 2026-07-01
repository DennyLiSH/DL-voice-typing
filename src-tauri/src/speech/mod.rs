#[cfg(test)]
pub mod mock;
#[cfg(not(feature = "whisper"))]
pub mod noop;
#[cfg(feature = "whisper")]
pub mod whisper;
#[cfg(feature = "whisper")]
pub mod whisper_factory;

use crate::error::AppError;

/// Trait for speech-to-text engines.
///
/// Allows swapping backends (Whisper.cpp, cloud APIs) without changing consumers.
/// Dyn-compatible: uses only `&self` methods, no generic parameters, no async methods.
pub trait SpeechEngine: Send + Sync + 'static {
    /// Synchronous transcription (blocking). Use via `spawn_blocking` from async contexts.
    fn transcribe_sync(&self, samples: &[f32]) -> Result<String, AppError>;

    /// Check if the engine's model is loaded and ready.
    fn is_ready(&self) -> bool;

    /// Get the engine name for display.
    fn name(&self) -> &str;

    /// Return the compute mode badge string for the UI.
    /// Default is "unloaded"; WhisperEngine returns "gpu", "cpu", or "unloaded".
    fn compute_mode(&self) -> &'static str {
        "unloaded"
    }

    /// Transcribe with prior context for stabilizing overlapping regions.
    /// Default implementation ignores context and delegates to `transcribe_sync`.
    fn transcribe_sync_with_context(
        &self,
        samples: &[f32],
        _context: Option<&str>,
    ) -> Result<String, AppError> {
        self.transcribe_sync(samples)
    }
}
