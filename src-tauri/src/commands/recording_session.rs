//! Recording session orchestration.
//!
//! Deep module owning the full hotkey pipeline: mode → behaviour mapping,
//! recording resource lifecycle, and delivery (transcribe → LLM → inject).
//! The hotkey callback ([`super::hotkey_pipeline::make_hotkey_callback`]) is a
//! thin adapter that delegates `on_press` / `handle_release` to this module.
//!
//! Testability: `decide_release` is a pure function; `run_pipeline` /
//! `run_realtime_fast_path` are `pub(crate)` async methods callable with
//! constructed inputs; `on_release` returns a [`ReleaseAction`] whose
//! `Deliver` future can be awaited directly under a test runtime.

use crate::audio::{TARGET_SAMPLE_RATE, resample, rms};
use crate::clipboard::ClipboardProvider;
use crate::config::{AppConfig, Language, PipelineMode};
use crate::data_saving::{SaveConfig, SaveResult};
use crate::error::AppError;
use crate::llm::{AnyCorrector, LLMClient, TextCorrector};
use crate::perf::PerfMetrics;
use crate::speech::SpeechEngine;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

use super::pipeline_state::PipelineState;
use super::review::ReviewData;

/// Outcome of a hotkey release: either fully handled inline, or an async
/// delivery future for the caller (prod: spawn; tests: await) to schedule.
pub(crate) enum ReleaseAction {
    /// Release handled synchronously (silent audio, or RealtimeReview handoff).
    Done,
    /// Async delivery work (transcribe → LLM → inject). The caller decides
    /// *where* it runs; the module owns *what* it does.
    Deliver(Pin<Box<dyn Future<Output = ()> + Send>>),
}

/// Pure decision: `PipelineMode × accumulated realtime text → delivery kind`.
/// Zero dependencies — the core testable surface for mode routing.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ReleaseActionKind {
    Done,
    DeliverFast,
    DeliverFull,
}

/// Decide the release delivery path from mode and accumulated realtime text.
///
/// Silence is NOT modelled here — it is a runtime data check (`preprocess_audio`)
/// performed in `on_release` before this function is consulted.
pub(crate) fn decide_release(mode: PipelineMode, accumulated: Option<&str>) -> ReleaseActionKind {
    use PipelineMode as M;
    match (mode, accumulated) {
        // Classic modes always run the full pipeline (no realtime accumulator).
        (M::ClassicDirect | M::ClassicReview, _) => ReleaseActionKind::DeliverFull,
        (M::RealtimeReview, Some(_)) => ReleaseActionKind::Done,
        (M::RealtimeDirect, Some(_)) => ReleaseActionKind::DeliverFast,
        // Realtime modes with no accumulated text fall through to the full pipeline.
        (M::RealtimeDirect | M::RealtimeReview, None) => ReleaseActionKind::DeliverFull,
    }
}

/// Adapter: bridges `commands::EventEmitter` → `realtime::EventEmitter`.
struct RealtimeEmitterAdapter(Arc<dyn crate::commands::EventEmitter>);

impl crate::realtime::EventEmitter for RealtimeEmitterAdapter {
    fn emit_partial(&self, text: &str) {
        self.0.emit(
            "transcription-partial",
            serde_json::to_value(text).unwrap_or_default(),
        );
    }
}

/// Check silence and resample audio to 16kHz for Whisper. Returns `None` if
/// audio is near-silent (hallucination guard).
fn preprocess_audio(audio: &[f32], native_rate: u32) -> Option<Vec<f32>> {
    if audio.is_empty() {
        warn!("preprocess_audio: empty audio buffer, skipping transcription");
        return None;
    }
    let rms_val = rms::calculate_rms(audio);
    if rms_val < 0.01 {
        debug!("Silent audio (rms={rms_val:.4}), skipping transcription");
        return None;
    }
    Some(resample(audio, native_rate, TARGET_SAMPLE_RATE))
}

/// Owns the recording session: shared pipeline state + per-cycle perf slot.
/// Clone is cheap (all fields are `Arc`).
#[derive(Clone)]
pub(crate) struct RecordingSession {
    ps: PipelineState,
    perf_slot: Arc<Mutex<Option<PerfMetrics>>>,
}

impl RecordingSession {
    pub(crate) fn new(ps: PipelineState) -> Self {
        Self {
            ps,
            perf_slot: Arc::new(Mutex::new(None)),
        }
    }

    /// Hotkey press: state guard, window policy, resource lifecycle.
    /// Runs on the Win32 hook thread — must be synchronous and non-blocking.
    pub(crate) fn on_press(&self) {
        let t_press = Instant::now();
        let cycle_id = self.ps.perf_history.next_cycle_id();

        let can_record = self.ps.sm_start_recording();

        if can_record && !self.ps.engine.is_ready() {
            reset_to_idle(&self.ps);
            self.ps.emitter.emit(
                "speech-error",
                serde_json::to_value("模型加载中，请稍候...").unwrap_or_default(),
            );
            return;
        }

        if can_record {
            // === Session state cleanup: prevent leaks from previous session ===
            self.ps.review.set_shown_on_press(false);
            if let Some(mut rt_guard) =
                crate::util::lock_mutex(&self.ps.realtime_transcriber, "realtime_transcriber")
            {
                if let Some(ref mut rt) = *rt_guard {
                    rt.stop();
                    rt_guard.take();
                }
            }

            let config = self.ps.config_cache.read_cached();
            let mode = config.pipeline_mode();

            // Show floating window near text caret (RealtimeReview shows review instead).
            let show_floating = !matches!(mode, PipelineMode::RealtimeReview);
            info!(
                "hotkey press: mode={:?}, show_floating={}",
                mode, show_floating
            );
            if show_floating {
                self.ps.window_controller.show_floating_near_caret();
            }
            self.ps
                .emitter
                .emit("recording-start", serde_json::Value::Null);

            // Clear ring buffer for new recording session.
            if let Some(mut buf) =
                crate::util::lock_mutex(&self.ps.audio_ring_buffer, "audio_ring_buffer")
            {
                buf.clear();
            }

            // Start audio capture with RMS-emitting callback (~30 fps).
            let last_rms_emit = Arc::new(Mutex::new(Instant::now()));
            if let Some(mut ac_guard) = crate::util::lock_mutex(&self.ps.ac, "audio_capture") {
                let ring_buf_for_audio = Arc::clone(&self.ps.audio_ring_buffer);
                let emitter_for_rms = self.ps.emitter.clone();
                let last_rms_for_cb = Arc::clone(&last_rms_emit);
                let start_result = ac_guard.start(Box::new(move |data: &[f32]| {
                    if let Some(mut buf) =
                        crate::util::lock_mutex(&ring_buf_for_audio, "audio_ring_buffer")
                    {
                        buf.push(data);
                    }
                    let rms_val = rms::calculate_rms(data);
                    if let Some(mut last) =
                        crate::util::lock_mutex(&last_rms_for_cb, "last_rms_emit")
                    {
                        if last.elapsed() >= Duration::from_millis(33) {
                            *last = Instant::now();
                            emitter_for_rms.emit(
                                "audio-rms",
                                serde_json::to_value(rms_val).unwrap_or_default(),
                            );
                        }
                    }
                }));

                if let Err(e) = start_result {
                    warn!("audio capture start failed: {e}");
                    reset_to_idle(&self.ps);
                    self.ps.emitter.emit(
                        "speech-error",
                        serde_json::to_value(format!("录音启动失败: {e}")).unwrap_or_default(),
                    );
                    return;
                }

                // Start real-time transcription for realtime modes.
                if matches!(
                    mode,
                    PipelineMode::RealtimeDirect | PipelineMode::RealtimeReview
                ) {
                    if let Some(sr) = ac_guard.sample_rate() {
                        let audio = Arc::new(crate::realtime::AudioRingBufferSource::new(
                            self.ps.audio_ring_buffer.clone(),
                        ));
                        let emitter = Arc::new(RealtimeEmitterAdapter(self.ps.emitter.clone()));
                        let rt = crate::realtime::RealtimeTranscriber::start(
                            audio,
                            self.ps.engine.clone(),
                            emitter,
                            sr,
                        );
                        if let Some(mut rt_guard) = crate::util::lock_mutex(
                            &self.ps.realtime_transcriber,
                            "realtime_transcriber",
                        ) {
                            *rt_guard = Some(rt);
                        }
                    }
                }
            }

            // Show review window on press for RealtimeReview mode.
            if matches!(mode, PipelineMode::RealtimeReview) {
                self.ps.review.save_foreground();
                self.ps.review.set_shown_on_press(true);
                self.ps.window_controller.show_review_near_caret();
            }

            let press_latency = t_press.elapsed().as_millis() as u64;
            let mut perf = PerfMetrics::new(cycle_id);
            perf.press_latency_ms = Some(press_latency);
            if let Some(mut slot) = crate::util::lock_mutex(&self.perf_slot, "perf") {
                *slot = Some(perf);
            }
        }
    }

    /// Hotkey release: synchronous prefix (stop resources, state guard) +
    /// decides the delivery path via [`decide_release`]. Returns a
    /// [`ReleaseAction`] whose `Deliver` future the caller schedules.
    pub(crate) fn on_release(&self) -> ReleaseAction {
        info!("hotkey release: event received");
        let t_release = Instant::now();

        let mut perf = crate::util::lock_mutex(&self.perf_slot, "perf")
            .and_then(|mut s| s.take())
            .unwrap_or_else(|| PerfMetrics::new(self.ps.perf_history.next_cycle_id()));
        perf.audio_duration_ms = Some(t_release.elapsed().as_millis() as u64);

        let sample_rate =
            crate::util::lock_mutex(&self.ps.ac, "audio_capture").and_then(|a| a.sample_rate());

        // Stop audio capture and realtime transcriber, get accumulated text.
        let realtime_accumulated = self.ps.stop_recording_resources();

        let config = self.ps.config_cache.read_cached();
        let mode = config.pipeline_mode();

        let t_press_for_e2e =
            t_release - Duration::from_millis(perf.audio_duration_ms.unwrap_or(0));

        match decide_release(mode, realtime_accumulated.as_deref()) {
            ReleaseActionKind::Done => {
                // RealtimeReview handoff: accumulated text already shown on press.
                info!(
                    "hotkey release: RealtimeReview fast path, {} chars already in textarea",
                    realtime_accumulated.map(|s| s.len()).unwrap_or(0)
                );
                if let Some(mut cb) = crate::util::lock_mutex(&self.ps.clipboard, "clipboard") {
                    let _ = cb.save();
                }
                self.ps.sm_stop_recording();
                self.ps.sm_transcribing_to_reviewing();
                self.ps.window_controller.hide_floating();
                ReleaseAction::Done
            }
            ReleaseActionKind::DeliverFast => {
                let accumulated = realtime_accumulated.expect("DeliverFast requires accumulated");
                info!(
                    "hotkey release: RealtimeDirect fast path, {} chars",
                    accumulated.len()
                );
                let audio_data = take_audio(&self.ps);
                let native_rate = sample_rate.unwrap_or(48000);
                let stop_ok = self.ps.sm_stop_recording();
                if !stop_ok {
                    info!("hotkey release: RealtimeDirect stop_recording failed");
                    self.ps.sm_reset();
                    self.ps.window_controller.hide_floating();
                    return ReleaseAction::Done;
                }
                perf.audio_samples = audio_data.len();
                perf.audio_sample_rate = native_rate;
                perf.release_latency_ms = Some(t_release.elapsed().as_millis() as u64);
                let this = self.clone();
                ReleaseAction::Deliver(Box::pin(async move {
                    this.run_realtime_fast_path(
                        accumulated,
                        audio_data,
                        native_rate,
                        perf,
                        t_press_for_e2e,
                    )
                    .await;
                }))
            }
            ReleaseActionKind::DeliverFull => {
                // Classic modes, or realtime modes with no accumulated text.
                let audio_data = take_audio(&self.ps);
                let native_rate = sample_rate.unwrap_or(48000);
                let resampled = match preprocess_audio(&audio_data, native_rate) {
                    Some(r) => r,
                    None => {
                        info!(
                            "hotkey release: preprocess_audio returned None (silent?), samples={}",
                            audio_data.len()
                        );
                        self.ps.sm_reset();
                        self.ps.window_controller.hide_floating();
                        if self.ps.review.was_shown_on_press() {
                            self.ps.window_controller.hide_review();
                            self.ps.review.set_shown_on_press(false);
                        }
                        return ReleaseAction::Done;
                    }
                };
                let stop_ok = self.ps.sm_stop_recording();
                if !stop_ok {
                    info!("hotkey release: stop_recording failed (state already reset)");
                    self.ps.window_controller.hide_floating();
                    return ReleaseAction::Done;
                }
                perf.audio_samples = audio_data.len();
                perf.audio_sample_rate = native_rate;
                perf.release_latency_ms = Some(t_release.elapsed().as_millis() as u64);
                let review = matches!(
                    mode,
                    PipelineMode::ClassicReview | PipelineMode::RealtimeReview
                );
                let this = self.clone();
                ReleaseAction::Deliver(Box::pin(async move {
                    this.run_pipeline(
                        audio_data,
                        native_rate,
                        resampled,
                        review,
                        perf,
                        t_press_for_e2e,
                    )
                    .await;
                }))
            }
        }
    }

    /// Production release entry point: runs the synchronous prefix, then for a
    /// `Deliver` outcome spawns the future and supervises its `JoinHandle`. On
    /// panic (only observable under `panic=unwind`: dev/test/CI), runs
    /// [`recover`]. Under release `panic=abort` a panic aborts the process
    /// (identical to today) before the supervisor can act.
    pub(crate) fn handle_release(&self) {
        match self.on_release() {
            ReleaseAction::Done => {}
            ReleaseAction::Deliver(fut) => {
                let session = self.clone();
                let handle = tauri::async_runtime::spawn(fut);
                tauri::async_runtime::spawn(async move {
                    // `tauri::async_runtime` JoinHandle yields `tauri::Error` on
                    // failure (no `is_panic`). Recover on any error: panic
                    // surfaces here only under `panic=unwind` (dev/test/CI); under
                    // release `panic=abort` the process dies before this runs.
                    if handle.await.is_err() {
                        warn!("delivery future failed; running session recovery");
                        session.recover();
                    }
                });
            }
        }
    }

    /// Panic recovery: reset state to idle, conditionally restore the
    /// clipboard, and surface a `speech-error`. Each step is tolerant so a
    /// secondary panic does not escape the supervisor task.
    ///
    /// Conditional restore (Maj-γ): `restore()` is called only when the
    /// clipboard was saved this cycle. `ClipboardManager::inject_text` already
    /// self-restores after a successful paste, so this is a harmless no-op in
    /// the post-inject case and cleans up leaked text in the pre-inject case.
    pub(crate) fn recover(&self) {
        self.ps.sm_reset();
        self.ps.window_controller.hide_floating();
        if let Some(mut cb) = crate::util::lock_mutex(&self.ps.clipboard, "clipboard") {
            if cb.was_saved() {
                let _ = cb.restore();
            }
        }
        self.ps.emitter.emit(
            "speech-error",
            serde_json::to_value("转录流程异常，已恢复").unwrap_or_default(),
        );
    }

    /// Full transcription → LLM → injection pipeline (classic modes + realtime
    /// fallthrough). `review` decides review-vs-direct (derived once from mode
    /// at dispatch time — no `review_before_paste` re-read here).
    pub(crate) async fn run_pipeline(
        &self,
        audio_for_save: Vec<f32>,
        native_rate: u32,
        resampled: Vec<f32>,
        review: bool,
        mut perf: PerfMetrics,
        t_press_for_e2e: Instant,
    ) {
        let config = self.ps.config_cache.read_cached();
        info!(
            "Pipeline: starting (review={}, llm={}, samples={})",
            review,
            config.llm_enabled,
            resampled.len()
        );

        // -- Save audio and transcribe in parallel --
        let (save_result, transcription) =
            transcribe_and_save(&self.ps, audio_for_save, native_rate, resampled, &config).await;

        perf.transcription_ms = perf
            .transcription_ms
            .or(Some(Instant::now().elapsed().as_millis() as u64));

        if transcription.is_empty() {
            info!("run_pipeline: empty transcription, resetting to idle");
            reset_to_idle(&self.ps);
            return;
        }

        // -- LLM Correction (optional) --
        perf.llm_enabled = config.llm_enabled;
        let final_text = if config.llm_enabled {
            resolve_llm_text(&self.ps, &config, &transcription, &mut perf)
                .await
                .unwrap_or_else(|e| {
                    warn!("run_pipeline: LLM correction failed: {e}");
                    transcription.clone()
                })
        } else {
            transcription.clone()
        };

        let final_text = if config.language == Language::Zh {
            normalize_chinese_punctuation(&final_text)
        } else {
            final_text
        };

        // -- Delivery (review vs direct decided once from mode) --
        if review {
            info!(
                "run_pipeline: handing off to deliver_review ({} chars)",
                final_text.len()
            );
            deliver_review(&self.ps, final_text, transcription, save_result, &config).await;
            return;
        }
        info!(
            "run_pipeline: handing off to deliver_direct ({} chars)",
            final_text.len()
        );
        deliver_direct(
            &self.ps,
            final_text,
            transcription,
            save_result,
            &config,
            &mut perf,
            t_press_for_e2e,
        )
        .await;
    }

    /// Fast path for RealtimeDirect mode: uses accumulated realtime text
    /// directly, optionally runs LLM, then injects. Skips Whisper entirely.
    pub(crate) async fn run_realtime_fast_path(
        &self,
        accumulated: String,
        audio_data: Vec<f32>,
        native_rate: u32,
        mut perf: PerfMetrics,
        t_press_for_e2e: Instant,
    ) {
        let config = self.ps.config_cache.read_cached();
        info!(
            "RealtimeFastPath: starting (llm={}, accumulated={} chars)",
            config.llm_enabled,
            accumulated.len()
        );

        // Save audio in background for training data.
        let save_config = SaveConfig::from_app_config(&config);
        let sr_for_save = native_rate;
        let audio_for_save = audio_data.clone();
        let save_handle = tokio::task::spawn_blocking(move || {
            if save_config.enabled && !save_config.path.is_empty() {
                crate::data_saving::save_audio(&audio_for_save, sr_for_save, &save_config).ok()
            } else {
                None
            }
        });

        let transcription = accumulated.clone();
        perf.llm_enabled = config.llm_enabled;
        let final_text = if config.llm_enabled {
            resolve_llm_text(&self.ps, &config, &transcription, &mut perf)
                .await
                .unwrap_or_else(|e| {
                    warn!("run_realtime_fast_path: LLM correction failed: {e}");
                    transcription.clone()
                })
        } else {
            transcription.clone()
        };

        let final_text = if config.language == Language::Zh {
            normalize_chinese_punctuation(&final_text)
        } else {
            final_text
        };

        // State: Transcribing to Injecting.
        if config.llm_enabled {
            self.ps.sm_llm_to_injecting();
        } else {
            self.ps.sm_transcribing_to_injecting();
        }

        let save_result = save_handle.await.unwrap_or(None);
        let mut ctx = super::text_injector::InjectionContext {
            text: final_text,
            transcription,
            save_result,
            config: &config,
            perf: &mut perf,
            t_press_for_e2e,
        };
        super::text_injector::inject_text(&self.ps, &mut ctx).await;
    }
}

/// Take all accumulated audio samples from the ring buffer.
fn take_audio(ps: &PipelineState) -> Vec<f32> {
    crate::util::lock_mutex(&ps.audio_ring_buffer, "audio_ring_buffer")
        .map(|mut buf| buf.take_all())
        .unwrap_or_default()
}

/// Parallel save audio to disk + transcribe via speech engine.
/// Returns (save_result, transcription_text). On transcription failure, emits
/// error and returns an empty string.
async fn transcribe_and_save(
    ps: &PipelineState,
    audio_for_save: Vec<f32>,
    native_rate: u32,
    resampled: Vec<f32>,
    config: &AppConfig,
) -> (Option<SaveResult>, String) {
    let save_config = SaveConfig::from_app_config(config);
    let sr_for_save = native_rate;
    let save_handle = tokio::task::spawn_blocking(move || {
        if save_config.enabled && !save_config.path.is_empty() {
            crate::data_saving::save_audio(&audio_for_save, sr_for_save, &save_config).ok()
        } else {
            None
        }
    });

    let engine_ref = ps.engine.clone();
    let transcribe_handle =
        tokio::task::spawn_blocking(move || engine_ref.transcribe_sync(&resampled));

    let (save_result, transcription_result) = tokio::join!(save_handle, transcribe_handle);
    let save_result = save_result.unwrap_or(None);

    let transcription = match transcription_result {
        Ok(Ok(text)) => {
            ps.emitter.emit(
                "transcription-complete",
                serde_json::to_value(&text).unwrap_or_default(),
            );
            text
        }
        Ok(Err(e)) => {
            ps.emitter.emit(
                "speech-error",
                serde_json::to_value(e.to_string()).unwrap_or_default(),
            );
            reset_to_idle(ps);
            return (save_result, String::new());
        }
        Err(e) => {
            ps.emitter.emit(
                "speech-error",
                serde_json::to_value(e.to_string()).unwrap_or_default(),
            );
            reset_to_idle(ps);
            return (save_result, String::new());
        }
    };

    (save_result, transcription)
}

/// Resolve LLM-corrected text. Handles cache lookup, client creation, and fallback.
async fn resolve_llm_text(
    ps: &PipelineState,
    config: &AppConfig,
    transcription: &str,
    perf: &mut PerfMetrics,
) -> Result<String, AppError> {
    ps.sm_start_llm_refining();
    ps.emitter.emit("llm-refining", serde_json::Value::Null);

    let t_llm = Instant::now();

    // Ensure the cached corrector matches config, creating a new one if needed.
    {
        let mut cached = crate::util::lock_mutex(&ps.cached_llm, "cached_llm")
            .ok_or_else(|| AppError::Llm("cached_llm lock poisoned".to_string()))?;
        let needs_new = cached.as_ref().is_none_or(|c| {
            !c.matches_config(&config.llm_api_url, &config.llm_api_key, &config.llm_model)
        });
        if needs_new {
            *cached = Some(AnyCorrector::Live(LLMClient::new(
                config.llm_api_url.clone(),
                config.llm_api_key.clone(),
                config.llm_model.clone(),
            )));
        }
    }

    // Call correct_sync while re-acquiring the lock (holds lock for HTTP duration).
    let result = {
        let cached = crate::util::lock_mutex(&ps.cached_llm, "cached_llm")
            .ok_or_else(|| AppError::Llm("cached_llm lock poisoned".to_string()))?;
        let corrector = cached
            .as_ref()
            .ok_or_else(|| AppError::Llm("no LLM corrector available".to_string()))?;
        corrector.correct_sync(transcription)
    };

    perf.llm_correction_ms = Some(t_llm.elapsed().as_millis() as u64);

    match result {
        Ok(corrected) => {
            ps.emitter.emit(
                "llm-complete",
                serde_json::to_value(&corrected).unwrap_or_default(),
            );
            Ok(corrected)
        }
        Err(e) => {
            ps.emitter.emit(
                "llm-error",
                serde_json::to_value(e.to_string()).unwrap_or_default(),
            );
            Ok(transcription.to_string())
        }
    }
}

/// Show review window with transcribed text for user editing.
async fn deliver_review(
    ps: &PipelineState,
    final_text: String,
    transcription: String,
    save_result: Option<SaveResult>,
    config: &AppConfig,
) {
    info!(
        "deliver_review: ENTER ({} chars, llm={})",
        final_text.len(),
        config.llm_enabled
    );

    // Save clipboard before entering review state.
    if let Some(mut cb) = crate::util::lock_mutex(&ps.clipboard, "clipboard") {
        let _ = cb.save();
    } else {
        warn!("deliver_review: clipboard lock returned None (poisoned?)");
    }
    // Transition to Reviewing.
    if config.llm_enabled {
        ps.sm_llm_to_reviewing();
    } else {
        ps.sm_transcribing_to_reviewing();
    }
    // Store text for the review window to fetch on load.
    ps.review.store_text(final_text.clone());
    debug!("Review: stored pending text ({} chars)", final_text.len());

    let was_shown_on_press = ps.review.was_shown_on_press();

    if was_shown_on_press {
        info!(
            "Review: deliver_review() called, was_shown_on_press=true, final_text={} chars",
            final_text.len()
        );
        let json_text = serde_json::to_string(&final_text).unwrap_or_default();
        let js = format!(
            "(function(){{\
                 var t=document.getElementById('review-text');\
                 if(t){{t.value={json_text};t.selectionStart=t.selectionEnd=t.value.length;t.scrollTop=t.scrollHeight;}}\
                 var p=document.getElementById('preview');\
                 if(p){{p.textContent='';p.classList.remove('visible');}}\
                 var b=document.getElementById('btn-confirm');\
                 if(b){{b.disabled=false;}}\
             }})()"
        );
        if ps.window_controller.eval_review_js(&js) {
            info!(
                "Review: set final text via eval OK ({} chars)",
                final_text.len()
            );
        } else {
            warn!("Review: get_webview_window('review') returned None");
        }

        ps.window_controller.emit_review_final_text(&final_text);
        info!(
            "Review: emitted review-final-text OK ({} chars)",
            final_text.len()
        );

        if let Some(sr) = save_result.as_ref() {
            ps.review.store_review_data(ReviewData {
                json_path: sr.json_path.clone(),
                raw_transcription: transcription,
                llm_text: if config.llm_enabled {
                    Some(final_text)
                } else {
                    None
                },
            });
        }
        return;
    }

    ps.review.save_foreground();
    if ps.window_controller.show_review_near_caret() {
        debug!("Review: window shown, review-show emitted");

        if let Some(sr) = save_result.as_ref() {
            ps.review.store_review_data(ReviewData {
                json_path: sr.json_path.clone(),
                raw_transcription: transcription,
                llm_text: if config.llm_enabled {
                    Some(final_text)
                } else {
                    None
                },
            });
        }
    } else {
        warn!("review window not found. Falling back to direct injection.");
        let cb = ps.clipboard.clone();
        let text = final_text.clone();
        let _ = tokio::task::spawn_blocking(move || {
            if let Some(mut cb) = crate::util::lock_mutex(&cb, "clipboard") {
                let _ = cb.save();
                let _ = cb.inject_text(&text);
            }
        })
        .await;
        ps.sm_finish_injecting();
        ps.emitter
            .emit("injection-complete", serde_json::Value::Null);
        ps.window_controller.hide_floating();
    }
}

/// Direct clipboard injection path (review disabled).
async fn deliver_direct(
    ps: &PipelineState,
    final_text: String,
    transcription: String,
    save_result: Option<SaveResult>,
    config: &AppConfig,
    perf: &mut PerfMetrics,
    t_press_for_e2e: Instant,
) {
    if config.llm_enabled {
        ps.sm_llm_to_injecting();
    } else {
        ps.sm_transcribing_to_injecting();
    }

    let mut ctx = super::text_injector::InjectionContext {
        text: final_text,
        transcription,
        save_result,
        config,
        perf,
        t_press_for_e2e,
    };
    super::text_injector::inject_text(ps, &mut ctx).await;
}

/// Reset state machine to Idle and hide floating window.
fn reset_to_idle(ps: &PipelineState) {
    ps.sm_reset();
    ps.window_controller.hide_floating();
    if ps.review.was_shown_on_press() {
        ps.window_controller.hide_review();
        ps.review.set_shown_on_press(false);
    }
}

/// Replace ASCII comma/period with full-width Chinese equivalents when
/// surrounded by CJK Unified Ideographs, or at end-of-string preceded by CJK.
fn normalize_chinese_punctuation(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut result = String::with_capacity(text.len());

    fn is_cjk(ch: char) -> bool {
        matches!(ch, '\u{4E00}'..='\u{9FFF}')
    }

    for (i, &ch) in chars.iter().enumerate() {
        if ch == ',' || ch == '.' {
            let prev_cjk = i > 0 && is_cjk(chars[i - 1]);
            let next_cjk = i + 1 < chars.len() && is_cjk(chars[i + 1]);
            let at_end = i + 1 == chars.len();
            if prev_cjk && (next_cjk || at_end) {
                result.push(if ch == ',' { '，' } else { '。' });
                continue;
            }
        }
        result.push(ch);
    }
    result
}

#[cfg(test)]
mod tests_normalize_chinese_punctuation {
    use super::*;

    #[test]
    fn test_between_chinese() {
        assert_eq!(normalize_chinese_punctuation("你好,世界"), "你好，世界");
        assert_eq!(normalize_chinese_punctuation("今天.明天"), "今天。明天");
    }

    #[test]
    fn test_mixed_language_unchanged() {
        assert_eq!(normalize_chinese_punctuation("Hello,世界"), "Hello,世界");
        assert_eq!(normalize_chinese_punctuation("你好, world"), "你好, world");
    }

    #[test]
    fn test_decimal_unchanged() {
        assert_eq!(normalize_chinese_punctuation("版本3.5"), "版本3.5");
        assert_eq!(normalize_chinese_punctuation("3.14"), "3.14");
    }

    #[test]
    fn test_already_fullwidth_unchanged() {
        assert_eq!(normalize_chinese_punctuation("你好，世界"), "你好，世界");
        assert_eq!(normalize_chinese_punctuation("今天。明天"), "今天。明天");
    }

    #[test]
    fn test_multiple_replacements() {
        assert_eq!(
            normalize_chinese_punctuation("你好,今天天气不错.我们去玩吧"),
            "你好，今天天气不错。我们去玩吧"
        );
    }

    #[test]
    fn test_boundary_cases() {
        assert_eq!(normalize_chinese_punctuation(","), ",");
        assert_eq!(normalize_chinese_punctuation("你好."), "你好。");
        assert_eq!(normalize_chinese_punctuation("你好,"), "你好，");
        assert_eq!(normalize_chinese_punctuation(",你好"), ",你好");
    }

    #[test]
    fn test_end_of_string_not_cjk() {
        assert_eq!(normalize_chinese_punctuation("3."), "3.");
        assert_eq!(normalize_chinese_punctuation("hello."), "hello.");
    }

    #[test]
    fn test_preprocess_audio_silent() {
        let silent = vec![0.0f32; 4800];
        assert!(preprocess_audio(&silent, 48000).is_none());
    }

    #[test]
    fn test_preprocess_audio_loud() {
        let loud = vec![0.5f32; 4800];
        let result = preprocess_audio(&loud, 48000);
        assert!(result.is_some());
        // Resampled from 48000 to 16000 = 1/3 the samples.
        let resampled = result.unwrap();
        assert!(resampled.len() < 4800);
    }
}
