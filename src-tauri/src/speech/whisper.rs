use crate::config::Language;
use crate::error::AppError;
use crate::speech::{CANCELLED_MESSAGE, Segment, SpeechEngine};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
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

/// Maximum characters from confirmed text to include in initial_prompt context.
/// Conservative: 50 Chinese chars ≈ 50-150 tokens, well under Whisper's ~224-token prompt budget.
const MAX_CONTEXT_CHARS: usize = 50;

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
    pub(crate) fn new(model_path: PathBuf, language: Language) -> Self {
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
        // SAFETY: WhisperEngine is a single-owner struct; its internal Mutex is never
        // shared across panic-capable boundaries. Lock poisoning is impossible here.
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

        // SAFETY: WhisperEngine is a single-owner struct; its internal Mutex is never
        // shared across panic-capable boundaries. Lock poisoning is impossible here.
        *self.ctx.lock().unwrap() = Some(ctx);
        Ok(())
    }

    fn get_ctx(&self) -> Result<Arc<WhisperContext>, AppError> {
        // SAFETY: WhisperEngine is a single-owner struct; its internal Mutex is never
        // shared across panic-capable boundaries. Lock poisoning is impossible here.
        self.ctx
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| AppError::Speech("model not loaded".to_string()))
    }

    fn pop_state(&self, ctx: &Arc<WhisperContext>) -> whisper_rs::WhisperState {
        // SAFETY: WhisperEngine is a single-owner struct; its internal Mutex is never
        // shared across panic-capable boundaries. Lock poisoning is impossible here.
        self.state_pool.lock().unwrap().pop().unwrap_or_else(|| {
            // Pool exhausted: create a new state (slow path).
            ctx.create_state()
                .unwrap_or_else(|e| panic!("Whisper: failed to create state and pool empty: {e}"))
        })
    }

    fn push_state(&self, state: whisper_rs::WhisperState) {
        // SAFETY: WhisperEngine is a single-owner struct; its internal Mutex is never
        // shared across panic-capable boundaries. Lock poisoning is impossible here.
        let mut pool = self.state_pool.lock().unwrap();
        if pool.len() < STATE_POOL_SIZE {
            pool.push(state);
        }
        // If pool is full, drop the state (it will be cleaned up naturally).
    }

    /// Build base transcription params shared by all callers.
    /// Does NOT set `initial_prompt` — each caller sets its own.
    ///
    /// `cancel` wires whisper.cpp's abort callback so a long inference can be
    /// stopped mid-flight (Esc cancel); `None` keeps the historical
    /// no-abort behaviour byte-for-byte.
    fn build_base_params(&self, cancel: Option<Arc<AtomicBool>>) -> FullParams<'_, '_> {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some(self.language.code()));
        params.set_print_progress(false);
        params.set_print_timestamps(false);
        params.set_no_timestamps(true);
        params.set_single_segment(true);
        params.set_translate(false);
        if let Some(cancel) = cancel {
            // whisper-rs 0.16 callback setters take O: Into<Option<F>>, which
            // defeats closure type inference — spell out both type parameters.
            type AbortCb = Box<dyn FnMut() -> bool>;
            let abort_cb: AbortCb = Box::new(move || cancel.load(Ordering::Relaxed));
            params.set_abort_callback_safe::<Option<AbortCb>, AbortCb>(Some(abort_cb));
        }
        params
    }

    /// Build transcription params for long-form segmented transcription.
    ///
    /// MUST NOT reuse `build_base_params`: that one enables `no_timestamps`
    /// and `single_segment`, which would erase segment boundaries and
    /// timestamps. This builder keeps timestamps enabled and multi-segment
    /// output, and wires cancellation + progress callbacks.
    fn build_segment_params(
        &self,
        cancel: Arc<AtomicBool>,
        progress: Box<dyn Fn(u8) + Send + Sync>,
    ) -> FullParams<'_, '_> {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some(self.language.code()));
        params.set_print_progress(false);
        params.set_print_timestamps(false);
        params.set_no_timestamps(false);
        params.set_single_segment(false);
        params.set_translate(false);
        if let Some(prompt) = initial_prompt_for_lang(self.language) {
            params.set_initial_prompt(prompt);
        }
        // whisper-rs 0.16 callback setters take O: Into<Option<F>>, which
        // defeats closure type inference — spell out both type parameters.
        type AbortCb = Box<dyn FnMut() -> bool>;
        type ProgressCb = Box<dyn FnMut(i32)>;
        let abort_cb: AbortCb = Box::new(move || cancel.load(Ordering::Relaxed));
        params.set_abort_callback_safe::<Option<AbortCb>, AbortCb>(Some(abort_cb));
        let progress_cb: ProgressCb = Box::new(move |p: i32| {
            progress(p.clamp(0, 100) as u8);
        });
        params.set_progress_callback_safe::<Option<ProgressCb>, ProgressCb>(Some(progress_cb));
        params
    }

    /// Core transcription logic shared by `transcribe_sync` and `transcribe_with_context`.
    fn transcribe_with_params(
        &self,
        samples: &[f32],
        params: FullParams,
    ) -> Result<String, AppError> {
        let ctx = self.get_ctx()?;
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

    /// Transcribe with prior context appended to the initial_prompt.
    /// This stabilizes Whisper's output for overlapping regions by conditioning
    /// on previously confirmed transcription, reducing homophone drift.
    pub fn transcribe_with_context(
        &self,
        samples: &[f32],
        context: Option<&str>,
    ) -> Result<String, AppError> {
        let mut params = self.build_base_params(None);
        let lang_anchor = initial_prompt_for_lang(self.language);
        let prompt = match (lang_anchor, context) {
            (Some(anchor), Some(ctx_text)) => {
                let tail = Self::last_n_chars(ctx_text, MAX_CONTEXT_CHARS);
                format!("{anchor}{tail}")
            }
            (Some(anchor), None) => anchor.to_string(),
            (None, Some(ctx_text)) => Self::last_n_chars(ctx_text, MAX_CONTEXT_CHARS),
            (None, None) => String::new(),
        };
        if !prompt.is_empty() {
            params.set_initial_prompt(&prompt);
        }
        self.transcribe_with_params(samples, params)
    }

    /// Take the last `n` characters from `text`.
    pub(crate) fn last_n_chars(text: &str, n: usize) -> String {
        if text.chars().count() <= n {
            return text.to_string();
        }
        text.chars()
            .rev()
            .take(n)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }

    /// Return whether the engine is currently running on GPU.
    /// This is a Whisper-specific capability and not part of the generic `SpeechEngine` trait.
    pub fn is_gpu_mode(&self) -> bool {
        self.gpu_mode.load(std::sync::atomic::Ordering::Relaxed)
            && self.ctx.lock().is_ok_and(|guard| guard.is_some())
    }
}

impl SpeechEngine for WhisperEngine {
    fn transcribe_sync(&self, samples: &[f32]) -> Result<String, AppError> {
        let mut params = self.build_base_params(None);
        if let Some(prompt) = initial_prompt_for_lang(self.language) {
            params.set_initial_prompt(prompt);
        }
        self.transcribe_with_params(samples, params)
    }

    fn transcribe_sync_cancelable(
        &self,
        samples: &[f32],
        cancel: Arc<AtomicBool>,
    ) -> Result<String, AppError> {
        let mut params = self.build_base_params(Some(cancel.clone()));
        if let Some(prompt) = initial_prompt_for_lang(self.language) {
            params.set_initial_prompt(prompt);
        }
        let result = self.transcribe_with_params(samples, params);
        // Whisper's return value after an abort callback fires is not reliable
        // (some paths return Ok with partial segments). The token is the single
        // source of truth — if the user pressed Esc, surface as cancelled.
        if cancel.load(Ordering::Relaxed) {
            return Err(AppError::Speech(CANCELLED_MESSAGE.to_string()));
        }
        result
    }

    fn is_ready(&self) -> bool {
        self.ctx.lock().is_ok_and(|guard| guard.is_some())
    }

    fn compute_mode(&self) -> &'static str {
        if !self.is_ready() {
            "unloaded"
        } else if self.is_gpu_mode() {
            "gpu"
        } else {
            "cpu"
        }
    }

    fn transcribe_sync_with_context(
        &self,
        samples: &[f32],
        context: Option<&str>,
    ) -> Result<String, AppError> {
        self.transcribe_with_context(samples, context)
    }

    fn transcribe_with_segments_sync(
        &self,
        samples: &[f32],
        cancel: Arc<AtomicBool>,
        progress: Box<dyn Fn(u8) + Send + Sync>,
    ) -> Result<Vec<Segment>, AppError> {
        let ctx = self.get_ctx()?;
        let mut state = self.pop_state(&ctx);
        let params = self.build_segment_params(cancel.clone(), progress);

        let run = state
            .full(params, samples)
            .map_err(|e| AppError::Speech(format!("transcription failed: {e}")));

        let result = match run {
            Err(e) => Err(e),
            Ok(()) => {
                if cancel.load(Ordering::Relaxed) {
                    Err(AppError::Speech(CANCELLED_MESSAGE.to_string()))
                } else {
                    let num_segments = state.full_n_segments();
                    debug!("Whisper segments: {num_segments}");
                    let mut segments = Vec::new();
                    for i in 0..num_segments {
                        let Some(segment) = state.get_segment(i) else {
                            continue;
                        };
                        // Skip segments Whisper identifies as no-speech
                        // (hallucination guard).
                        if segment.no_speech_probability() > NO_SPEECH_PROB_THRESHOLD {
                            continue;
                        }
                        let text = segment
                            .to_str()
                            .map_err(|e| AppError::Speech(format!("segment text failed: {e}")))?
                            .trim()
                            .to_string();
                        if text.is_empty() {
                            continue;
                        }
                        segments.push(Segment::from_whisper(
                            text,
                            segment.start_timestamp(),
                            segment.end_timestamp(),
                        ));
                    }
                    info!("Whisper segment transcription: {} segments", segments.len());
                    Ok(segments)
                }
            }
        };

        self.push_state(state);
        result
    }

    fn name(&self) -> &str {
        "Whisper.cpp"
    }
}
