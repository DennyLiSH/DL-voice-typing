use crate::config::Language;
use crate::speech::whisper::WhisperEngine;
#[cfg(feature = "whisper")]
use std::path::PathBuf;
use std::sync::Arc;

/// Factory for creating production `WhisperEngine` instances.
///
/// This is the only non-test entry point for constructing `WhisperEngine`;
/// direct `WhisperEngine::new` calls outside tests are discouraged.
pub struct WhisperEngineFactory;

impl WhisperEngineFactory {
    /// Create a new `WhisperEngine` configured for the given model and language.
    pub fn create(model_path: PathBuf, language: Language) -> Arc<WhisperEngine> {
        Arc::new(WhisperEngine::new(model_path, language))
    }
}
