use crate::error::AppError;
use crate::speech::SpeechEngine;
use std::path::PathBuf;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

/// If Whisper's no_speech probability exceeds this threshold, the segment is
/// treated as silence/hallucination and discarded.
const NO_SPEECH_PROB_THRESHOLD: f32 = 0.6;

/// Returns an initial prompt to anchor Whisper's output language.
/// Prevents drift to English for non-English languages.
fn initial_prompt_for_lang(lang: &str) -> Option<&'static str> {
    match lang {
        "zh" => Some("以下是普通话的句子。"),
        "ja" => Some("以下は日本語の文章です。"),
        "ko" => Some("다음은 한국어 문장입니다."),
        _ => None,
    }
}

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
        params.set_no_timestamps(true);
        params.set_single_segment(true);
        params.set_translate(false);
        if let Some(prompt) = initial_prompt_for_lang(&self.language) {
            params.set_initial_prompt(prompt);
        }

        let mut state = ctx
            .create_state()
            .map_err(|e| AppError::Speech(format!("failed to create state: {}", e)))?;

        state
            .full(params, samples)
            .map_err(|e| AppError::Speech(format!("transcription failed: {}", e)))?;

        let num_segments = state.full_n_segments();
        eprintln!(
            "Whisper: {} segments, {} samples",
            num_segments,
            samples.len()
        );

        let mut text = String::new();
        for i in 0..num_segments {
            let segment = match state.get_segment(i) {
                Some(s) => s,
                None => continue,
            };

            // Skip segments Whisper identifies as no-speech (hallucination guard).
            if segment.no_speech_probability() > NO_SPEECH_PROB_THRESHOLD {
                eprintln!(
                    "Whisper: skipping segment {} (no_speech_prob={:.3})",
                    i,
                    segment.no_speech_probability()
                );
                continue;
            }

            text.push_str(
                segment
                    .to_str()
                    .map_err(|e| AppError::Speech(format!("segment text failed: {}", e)))?,
            );
        }

        eprintln!("Whisper result: {:?} ({} chars)", text, text.len());
        Ok(text.trim().to_string())
    }

    fn is_ready(&self) -> bool {
        self.ctx.is_some()
    }

    fn name(&self) -> &str {
        "Whisper.cpp"
    }
}
