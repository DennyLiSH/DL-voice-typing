use crate::config::Language;
use crate::error::AppError;
use crate::speech::SpeechEngine;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
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

/// Number of pre-created states in the pool.
const STATE_POOL_SIZE: usize = 2;

/// Whisper.cpp speech engine with internal state pool.
///
/// `WhisperContext` (model weights) is `Arc`-shared so multiple concurrent
/// transcriptions can run without creating a new state each time.
pub struct WhisperEngine {
    ctx: Mutex<Option<Arc<WhisperContext>>>,
    state_pool: Mutex<Vec<whisper_rs::WhisperState>>,
    model_path: PathBuf,
    language: Language,
    gpu_mode: std::sync::atomic::AtomicBool,
}

impl WhisperEngine {
    /// Create a new WhisperEngine with the given model path and language.
    pub fn new(model_path: PathBuf, language: Language) -> Self {
        Self {
            ctx: Mutex::new(None),
            state_pool: Mutex::new(Vec::new()),
            model_path,
            language,
            gpu_mode: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Load the Whisper model. Must be called before transcribe.
    /// Tries GPU first, falls back to CPU if GPU initialization fails.
    pub fn load_model(&self) -> Result<(), AppError> {
        if !self.model_path.exists() {
            let path = self.model_path.display();
            return Err(AppError::Speech(format!("model file not found: {path}")));
        }

        let path = self.model_path.to_string_lossy().to_string();

        // Phase 1: Try GPU (use_gpu defaults to true when vulkan feature is enabled).
        let params = WhisperContextParameters::default();
        let ctx = match WhisperContext::new_with_params(&path, params) {
            Ok(ctx) => {
                self.gpu_mode
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                info!("Whisper: model loaded on GPU");
                Arc::new(ctx)
            }
            Err(e) => {
                warn!("Whisper: GPU init failed ({e}), falling back to CPU...");
                // Phase 2: CPU fallback.
                let params = WhisperContextParameters {
                    use_gpu: false,
                    ..Default::default()
                };
                let ctx = WhisperContext::new_with_params(&path, params).map_err(|e| {
                    AppError::Speech(format!("failed to load model (GPU and CPU): {e}"))
                })?;
                self.gpu_mode
                    .store(false, std::sync::atomic::Ordering::Relaxed);
                info!("Whisper: model loaded on CPU (no GPU acceleration)");
                Arc::new(ctx)
            }
        };

        // Pre-warm the state pool.
        let mut pool = self.state_pool.lock().unwrap();
        pool.clear();
        for _ in 0..STATE_POOL_SIZE {
            match ctx.create_state() {
                Ok(state) => pool.push(state),
                Err(e) => {
                    warn!("Whisper: failed to pre-create state: {e}");
                    break;
                }
            }
        }
        info!("Whisper: state pool warmed with {} states", pool.len());

        *self.ctx.lock().unwrap() = Some(ctx);
        Ok(())
    }

    fn get_ctx(&self) -> Result<Arc<WhisperContext>, AppError> {
        self.ctx
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| AppError::Speech("model not loaded".to_string()))
    }

    fn pop_state(&self, ctx: &Arc<WhisperContext>) -> whisper_rs::WhisperState {
        self.state_pool.lock().unwrap().pop().unwrap_or_else(|| {
            // Pool exhausted: create a new state (slow path).
            ctx.create_state()
                .unwrap_or_else(|e| panic!("Whisper: failed to create state and pool empty: {e}"))
        })
    }

    fn push_state(&self, state: whisper_rs::WhisperState) {
        let mut pool = self.state_pool.lock().unwrap();
        if pool.len() < STATE_POOL_SIZE {
            pool.push(state);
        }
        // If pool is full, drop the state (it will be cleaned up naturally).
    }
}

impl SpeechEngine for WhisperEngine {
    async fn transcribe(&self, samples: &[f32]) -> Result<String, AppError> {
        self.transcribe_sync(samples)
    }

    fn transcribe_sync(&self, samples: &[f32]) -> Result<String, AppError> {
        let ctx = self.get_ctx()?;

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

        let mut state = self.pop_state(&ctx);

        let result = state
            .full(params, samples)
            .map_err(|e| AppError::Speech(format!("transcription failed: {e}")))
            .and_then(|_| {
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
            });

        self.push_state(state);
        result
    }

    fn is_ready(&self) -> bool {
        self.ctx.lock().unwrap().is_some()
    }

    fn is_gpu_mode(&self) -> bool {
        self.gpu_mode.load(std::sync::atomic::Ordering::Relaxed)
            && self.ctx.lock().unwrap().is_some()
    }

    fn name(&self) -> &str {
        "Whisper.cpp"
    }
}
