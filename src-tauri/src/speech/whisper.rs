use crate::config::Language;
use crate::error::AppError;
use crate::speech::SpeechEngine;
use std::path::PathBuf;
use tracing::{debug, info, warn};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

/// If Whisper's no_speech probability exceeds this threshold, the segment is
/// treated as silence/hallucination and discarded.
const NO_SPEECH_PROB_THRESHOLD: f32 = 0.6;

/// Returns an initial prompt to anchor Whisper's output language.
/// Prevents drift to English for non-English languages.
fn initial_prompt_for_lang(lang: Language) -> Option<&'static str> {
    match lang {
        Language::Zh => Some("以下是普通话的句子。"),
        Language::Ja => Some("以下は日本語の文章です。"),
        Language::Ko => Some("다음은 한국어 문장입니다."),
        Language::En => None,
    }
}

/// Whisper.cpp speech engine.
pub struct WhisperEngine {
    ctx: Option<WhisperContext>,
    model_path: PathBuf,
    language: Language,
    gpu_mode: bool,
}

impl WhisperEngine {
    /// Create a new WhisperEngine with the given model path and language.
    pub fn new(model_path: PathBuf, language: Language) -> Self {
        Self {
            ctx: None,
            model_path,
            language,
            gpu_mode: false,
        }
    }

    /// Load the Whisper model. Must be called before transcribe.
    /// Tries GPU first, falls back to CPU if GPU initialization fails.
    pub fn load_model(&mut self) -> Result<(), AppError> {
        if !self.model_path.exists() {
            return Err(AppError::Speech(format!(
                "model file not found: {}",
                self.model_path.display()
            )));
        }

        let path = self.model_path.to_string_lossy().to_string();

        // Phase 1: Try GPU (use_gpu defaults to true when vulkan feature is enabled).
        let params = WhisperContextParameters::default();
        match WhisperContext::new_with_params(&path, params) {
            Ok(ctx) => {
                self.gpu_mode = true;
                self.ctx = Some(ctx);
                info!("Whisper: model loaded on GPU");
                return Ok(());
            }
            Err(e) => {
                warn!("Whisper: GPU init failed ({e}), falling back to CPU...");
            }
        }

        // Phase 2: CPU fallback.
        let params = WhisperContextParameters {
            use_gpu: false,
            ..Default::default()
        };
        let ctx = WhisperContext::new_with_params(&path, params)
            .map_err(|e| AppError::Speech(format!("failed to load model (GPU and CPU): {e}")))?;
        self.gpu_mode = false;
        self.ctx = Some(ctx);
        info!("Whisper: model loaded on CPU (no GPU acceleration)");
        Ok(())
    }
}

impl SpeechEngine for WhisperEngine {
    async fn transcribe(&self, samples: &[f32]) -> Result<String, AppError> {
        self.transcribe_sync(samples)
    }

    fn transcribe_sync(&self, samples: &[f32]) -> Result<String, AppError> {
        let ctx = self
            .ctx
            .as_ref()
            .ok_or_else(|| AppError::Speech("model not loaded".to_string()))?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some(self.language.code()));
        params.set_print_progress(false);
        params.set_print_timestamps(false);
        params.set_no_timestamps(true);
        params.set_single_segment(true);
        params.set_translate(false);
        if let Some(prompt) = initial_prompt_for_lang(self.language) {
            params.set_initial_prompt(prompt);
        }

        let mut state = ctx
            .create_state()
            .map_err(|e| AppError::Speech(format!("failed to create state: {e}")))?;

        state
            .full(params, samples)
            .map_err(|e| AppError::Speech(format!("transcription failed: {e}")))?;

        let num_segments = state.full_n_segments();
        debug!(
            "Whisper: {num_segments} segments, {} samples",
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
                debug!(
                    "Whisper: skipping segment {i} (no_speech_prob={:.3})",
                    segment.no_speech_probability()
                );
                continue;
            }

            text.push_str(
                segment
                    .to_str()
                    .map_err(|e| AppError::Speech(format!("segment text failed: {e}")))?,
            );
        }

        info!("Whisper result: {text:?} ({} chars)", text.len());
        Ok(text.trim().to_string())
    }

    fn is_ready(&self) -> bool {
        self.ctx.is_some()
    }

    fn is_gpu_mode(&self) -> bool {
        self.gpu_mode && self.ctx.is_some()
    }

    fn name(&self) -> &str {
        "Whisper.cpp"
    }
}
