use crate::error::AppError;
use crate::speech::SpeechEngine;
use std::path::PathBuf;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

/// Whisper.cpp speech engine.
pub struct WhisperEngine {
    ctx: Option<WhisperContext>,
    model_path: PathBuf,
    language: String,
}

impl WhisperEngine {
    /// Create a new WhisperEngine with the given model path and language.
    pub fn new(model_path: PathBuf, language: String) -> Self {
        Self {
            ctx: None,
            model_path,
            language,
        }
    }

    /// Load the Whisper model. Must be called before transcribe.
    pub fn load_model(&mut self) -> Result<(), AppError> {
        if !self.model_path.exists() {
            return Err(AppError::Speech(format!(
                "model file not found: {}",
                self.model_path.display()
            )));
        }

        let params = WhisperContextParameters::default();
        let ctx =
            WhisperContext::new_with_params(self.model_path.to_string_lossy().as_ref(), params)
                .map_err(|e| AppError::Speech(format!("failed to load model: {}", e)))?;

        self.ctx = Some(ctx);
        Ok(())
    }

    /// Get the expected model file path for a given model size.
    pub fn model_path_for_size(model_size: &str) -> PathBuf {
        let filename = match model_size {
            "tiny" => "ggml-tiny.bin",
            "base" => "ggml-base.bin",
            "small" => "ggml-small.bin",
            _ => "ggml-base.bin",
        };
        Self::models_dir().join(filename)
    }

    /// Get the models directory (%APPDATA%/dl-voice-typing/models).
    pub fn models_dir() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("dl-voice-typing")
            .join("models")
    }
}

impl SpeechEngine for WhisperEngine {
    async fn transcribe(&self, samples: &[f32]) -> Result<String, AppError> {
        let ctx = self
            .ctx
            .as_ref()
            .ok_or_else(|| AppError::Speech("model not loaded".to_string()))?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some(&self.language));
        params.set_print_progress(false);
        params.set_print_timestamps(false);

        let mut state = ctx
            .create_state()
            .map_err(|e| AppError::Speech(format!("failed to create state: {}", e)))?;

        state
            .full(params, samples)
            .map_err(|e| AppError::Speech(format!("transcription failed: {}", e)))?;

        let num_segments = state
            .full_n_segments()
            .map_err(|e| AppError::Speech(format!("segment count failed: {}", e)))?;

        let mut text = String::new();
        for i in 0..num_segments {
            let segment = state
                .full_get_segment_text(i)
                .map_err(|e| AppError::Speech(format!("segment text failed: {}", e)))?;
            text.push_str(&segment);
        }

        Ok(text.trim().to_string())
    }

    fn is_ready(&self) -> bool {
        self.ctx.is_some()
    }

    fn name(&self) -> &str {
        "Whisper.cpp"
    }
}
