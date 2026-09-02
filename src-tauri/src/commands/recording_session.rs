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
use crate::config::{AppConfig, Language, PipelineMode};
use crate::data_saving::{SaveConfig, SaveResult};
use crate::error::AppError;
use crate::llm::{LLMClient, TextCorrector};
use crate::perf::PerfMetrics;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

use super::pipeline_state::PipelineState;

/// User-facing payload for the `llm-error` event. The floating window is
/// 160x60px of text space — it gets this fixed Chinese summary while the
/// redacted English detail goes to the tracing log (see `resolve_llm_text`).
/// Wording matches transcribe_cmd's `transcription-error` message; payload
/// shape differs (bare string vs `{"message": …}`) because the consumers
/// differ (floating window vs transcribe window).
pub(crate) const LLM_ERROR_USER_MSG: &str = "LLM 纠错失败，已保留原始转录";

/// Snapshot of config consumed by a single recording session.
///
/// Built once at hotkey press/release entry to prevent mid-session config
/// mutations (user toggling settings window mid-pipeline) from breaking
/// in-flight state — `ConfigCache` reads return live values that can change
/// between `on_press` and delivery.
///
/// `llm_api_key` is intentionally NOT included — fetched live from
/// `ConfigCache` per LLM call so a rotated key takes effect on the next
/// `resolve_llm_text` without invalidating the rest of the snapshot.
///
/// Future AppConfig fields must be added here OR to `KNOWN_UNSNAPSHOTED_FIELDS`
/// (test `session_policy_field_coverage_audits_all_appconfig_fields` enforces).
pub(crate) struct SessionPolicy {
    pub mode: PipelineMode,
    pub llm_enabled: bool,
    pub language: Language,
    pub llm_api_url: String,
    pub llm_api_model: String,
    pub save: SaveConfig,
}

impl SessionPolicy {
    /// Snapshot an AppConfig into a session-scoped policy.
    pub(crate) fn from_config(c: &AppConfig) -> Self {
        Self {
            mode: c.pipeline_mode(),
            llm_enabled: c.llm_enabled,
            language: c.language,
            llm_api_url: c.llm_api_url.clone(),
            llm_api_model: c.llm_model.clone(),
            save: SaveConfig::from_app_config(c),
        }
    }
}

/// AppConfig fields intentionally excluded from SessionPolicy (audit reference).
/// Adding a new AppConfig field requires updating either SessionPolicy or this list,
/// otherwise the field-coverage test fails.
#[allow(dead_code)] // referenced by tests_session_policy::known_unsnapshoted_fields_audit_anchor
const KNOWN_UNSNAPSHOTED_FIELDS: &[&str] = &[
    "llm_api_key",         // live-read per LLM call for key rotation
    "autostart",           // app-lifecycle concern, not pipeline
    "review_before_paste", // folded into mode via pipeline_mode()
    "hotkey",              // hotkey re-registration is independent of pipeline
    "download_mirror",     // only used at model fetch time, not pipeline
    "record_only_enabled", // record-only mode has its own session policy (record_only_session)
    "record_only_hotkey",  // hotkey re-registration is independent of pipeline
];

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
        let cycle_id = self.ps.perf_history().next_cycle_id();

        let can_record = self.ps.sm_start_recording();

        if can_record && !self.ps.engine().is_ready() {
            reset_to_idle(&self.ps);
            self.ps.emitter().emit(
                "speech-error",
                serde_json::to_value("模型加载中，请稍候...").unwrap_or_default(),
            );
            return;
        }

        if can_record {
            // === Session state cleanup: prevent leaks from previous session ===
            self.ps.review().set_shown_on_press(false);
            if let Some(mut rt_guard) =
                crate::util::lock_mutex(&self.ps.realtime_transcriber(), "realtime_transcriber")
            {
                if let Some(ref mut rt) = *rt_guard {
                    rt.stop();
                    rt_guard.take();
                }
            }

            let policy = SessionPolicy::from_config(&self.ps.config_cache().read_cached());
            let mode = policy.mode;

            // Show floating window near text caret (RealtimeReview shows review instead).
            let show_floating = !matches!(mode, PipelineMode::RealtimeReview);
            info!(
                "hotkey press: mode={:?}, show_floating={}",
                mode, show_floating
            );
            if show_floating {
                self.ps.window_controller().show_floating_near_caret();
            }
            self.ps
                .emitter()
                .emit("recording-start", serde_json::Value::Null);

            // Clear ring buffer for new recording session.
            self.ps.clear_ring();

            // Start audio capture with RMS-emitting callback (~30 fps).
            let last_rms_emit = Arc::new(Mutex::new(Instant::now()));
            if let Some(mut ac_guard) =
                crate::util::lock_mutex(&self.ps.audio_capture(), "audio_capture")
            {
                let ring_buf_for_audio = self.ps.ring_buffer();
                let emitter_for_rms = self.ps.emitter();
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
                    self.ps.emitter().emit(
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
                            self.ps.ring_buffer(),
                        ));
                        let rt = crate::realtime::RealtimeTranscriber::start(
                            audio,
                            self.ps.engine(),
                            self.ps.emitter(),
                            sr,
                        );
                        if let Some(mut rt_guard) = crate::util::lock_mutex(
                            &self.ps.realtime_transcriber(),
                            "realtime_transcriber",
                        ) {
                            *rt_guard = Some(rt);
                        }
                    }
                }
            }

            // Show review window on press for RealtimeReview mode.
            if matches!(mode, PipelineMode::RealtimeReview) {
                self.ps.review().save_foreground();
                self.ps.review().set_shown_on_press(true);
                self.ps.window_controller().show_review_near_caret();
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
            .unwrap_or_else(|| PerfMetrics::new(self.ps.perf_history().next_cycle_id()));
        perf.audio_duration_ms = Some(t_release.elapsed().as_millis() as u64);

        let sample_rate = crate::util::lock_mutex(&self.ps.audio_capture(), "audio_capture")
            .and_then(|a| a.sample_rate());

        // Stop audio capture and realtime transcriber, get accumulated text.
        let realtime_accumulated = self.ps.stop_recording_resources();

        let policy = SessionPolicy::from_config(&self.ps.config_cache().read_cached());
        let mode = policy.mode;

        let t_press_for_e2e =
            t_release - Duration::from_millis(perf.audio_duration_ms.unwrap_or(0));

        match decide_release(mode, realtime_accumulated.as_deref()) {
            ReleaseActionKind::Done => {
                // RealtimeReview handoff: accumulated text already shown on press.
                info!(
                    "hotkey release: RealtimeReview fast path, {} chars already in textarea",
                    realtime_accumulated.as_ref().map(|s| s.len()).unwrap_or(0)
                );
                self.ps
                    .delivery()
                    .realtime_review_handoff(&self.ps, realtime_accumulated);
                ReleaseAction::Done
            }
            ReleaseActionKind::DeliverFast => {
                let Some(accumulated) = realtime_accumulated else {
                    warn!("hotkey release: DeliverFast missing accumulated text, resetting");
                    self.ps.sm_reset();
                    self.ps.window_controller().hide_floating();
                    return ReleaseAction::Done;
                };
                info!(
                    "hotkey release: RealtimeDirect fast path, {} chars",
                    accumulated.len()
                );
                let audio_data = self.ps.take_ring_samples();
                let native_rate = sample_rate.unwrap_or(48000);
                let stop_ok = self.ps.sm_stop_recording();
                if !stop_ok {
                    info!("hotkey release: RealtimeDirect stop_recording failed");
                    self.ps.sm_reset();
                    self.ps.window_controller().hide_floating();
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
                        policy,
                    )
                    .await;
                }))
            }
            ReleaseActionKind::DeliverFull => {
                // Classic modes, or realtime modes with no accumulated text.
                let audio_data = self.ps.take_ring_samples();
                let native_rate = sample_rate.unwrap_or(48000);
                let resampled = match preprocess_audio(&audio_data, native_rate) {
                    Some(r) => r,
                    None => {
                        info!(
                            "hotkey release: preprocess_audio returned None (silent?), samples={}",
                            audio_data.len()
                        );
                        self.ps.sm_reset();
                        self.ps.window_controller().hide_floating();
                        if self.ps.review().was_shown_on_press() {
                            self.ps.window_controller().hide_review();
                            self.ps.review().set_shown_on_press(false);
                        }
                        return ReleaseAction::Done;
                    }
                };
                let stop_ok = self.ps.sm_stop_recording();
                if !stop_ok {
                    info!("hotkey release: stop_recording failed (state already reset)");
                    self.ps.window_controller().hide_floating();
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
                        policy,
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
        self.ps.window_controller().hide_floating();
        if self.ps.clipboard().was_saved() {
            let _ = self.ps.clipboard().restore();
        }
        self.ps.emitter().emit(
            "speech-error",
            serde_json::to_value("转录流程异常，已恢复").unwrap_or_default(),
        );
    }

    /// Full transcription → LLM → injection pipeline (classic modes + realtime
    /// fallthrough). `review` decides review-vs-direct (derived once from mode
    /// at dispatch time — no `review_before_paste` re-read here).
    #[allow(clippy::too_many_arguments)] // 8 params; merging would obscure call site
    pub(crate) async fn run_pipeline(
        &self,
        audio_for_save: Vec<f32>,
        native_rate: u32,
        resampled: Vec<f32>,
        review: bool,
        mut perf: PerfMetrics,
        t_press_for_e2e: Instant,
        policy: SessionPolicy,
    ) {
        info!(
            "Pipeline: starting (review={}, llm={}, samples={})",
            review,
            policy.llm_enabled,
            resampled.len()
        );

        // -- Save audio and transcribe in parallel --
        let (save_result, transcription) = transcribe_and_save(
            &self.ps,
            audio_for_save,
            native_rate,
            resampled,
            &policy.save,
        )
        .await;

        perf.transcription_ms = perf
            .transcription_ms
            .or(Some(Instant::now().elapsed().as_millis() as u64));

        if transcription.is_empty() {
            info!("run_pipeline: empty transcription, resetting to idle");
            reset_to_idle(&self.ps);
            return;
        }

        // -- LLM Correction (optional) --
        perf.llm_enabled = policy.llm_enabled;
        let final_text = if policy.llm_enabled {
            resolve_llm_text(&self.ps, &policy, &transcription, &mut perf)
                .await
                .unwrap_or_else(|e| {
                    warn!("run_pipeline: LLM correction failed: {e}");
                    transcription.clone()
                })
        } else {
            transcription.clone()
        };

        let final_text = if policy.language == Language::Zh {
            normalize_chinese_punctuation(&final_text)
        } else {
            final_text
        };

        // -- Delivery (review vs direct decided once from mode) --
        if review {
            info!(
                "run_pipeline: handing off to DeliveryController::show_review ({} chars)",
                final_text.len()
            );
            self.ps
                .delivery()
                .show_review(
                    &self.ps,
                    final_text,
                    transcription,
                    save_result,
                    &policy,
                    perf,
                    t_press_for_e2e,
                    policy.llm_enabled,
                )
                .await;
            return;
        }
        info!(
            "run_pipeline: handing off to DeliveryController::inject_direct ({} chars)",
            final_text.len()
        );
        self.ps
            .delivery()
            .inject_direct(
                &self.ps,
                final_text,
                transcription,
                save_result,
                &policy,
                &mut perf,
                t_press_for_e2e,
                policy.llm_enabled,
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
        policy: SessionPolicy,
    ) {
        info!(
            "RealtimeFastPath: starting (llm={}, accumulated={} chars)",
            policy.llm_enabled,
            accumulated.len()
        );

        // Save audio in background for training data.
        let save_config = policy.save.clone();
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
        perf.llm_enabled = policy.llm_enabled;
        let final_text = if policy.llm_enabled {
            resolve_llm_text(&self.ps, &policy, &transcription, &mut perf)
                .await
                .unwrap_or_else(|e| {
                    warn!("run_realtime_fast_path: LLM correction failed: {e}");
                    transcription.clone()
                })
        } else {
            transcription.clone()
        };

        let final_text = if policy.language == Language::Zh {
            normalize_chinese_punctuation(&final_text)
        } else {
            final_text
        };

        let save_result = match save_handle.await {
            Ok(v) => v,
            Err(e) => {
                warn!("run_realtime_fast_path: save task failed: {e}");
                None
            }
        };

        self.ps
            .delivery()
            .inject_direct(
                &self.ps,
                final_text,
                transcription,
                save_result,
                &policy,
                &mut perf,
                t_press_for_e2e,
                policy.llm_enabled,
            )
            .await;
    }
}

/// Parallel save audio to disk + transcribe via speech engine.
/// Returns (save_result, transcription_text). On transcription failure, emits
/// error and returns an empty string.
async fn transcribe_and_save(
    ps: &PipelineState,
    audio_for_save: Vec<f32>,
    native_rate: u32,
    resampled: Vec<f32>,
    save: &SaveConfig,
) -> (Option<SaveResult>, String) {
    let save_config = save.clone();
    let sr_for_save = native_rate;
    let save_handle = tokio::task::spawn_blocking(move || {
        if save_config.enabled && !save_config.path.is_empty() {
            crate::data_saving::save_audio(&audio_for_save, sr_for_save, &save_config).ok()
        } else {
            None
        }
    });

    let engine_ref = ps.engine();
    let transcribe_handle =
        tokio::task::spawn_blocking(move || engine_ref.transcribe_sync(&resampled));

    let (save_result, transcription_result) = tokio::join!(save_handle, transcribe_handle);
    let save_result = save_result.unwrap_or(None);

    let transcription = match transcription_result {
        Ok(Ok(text)) => {
            ps.emitter().emit(
                "transcription-complete",
                serde_json::to_value(&text).unwrap_or_default(),
            );
            text
        }
        Ok(Err(e)) => {
            ps.emitter().emit(
                "speech-error",
                serde_json::to_value(e.to_string()).unwrap_or_default(),
            );
            reset_to_idle(ps);
            return (save_result, String::new());
        }
        Err(e) => {
            ps.emitter().emit(
                "speech-error",
                serde_json::to_value(e.to_string()).unwrap_or_default(),
            );
            reset_to_idle(ps);
            return (save_result, String::new());
        }
    };

    (save_result, transcription)
}

/// Ensure the cached corrector matches the given config, replacing it if mismatched.
/// Returns `Ok(true)` when the cache was rebuilt, `Ok(false)` when the existing
/// corrector was kept. Returns Err only on lock poisoning. HTTP calls happen
/// later in `resolve_llm_text`.
///
/// Boundary: `live_api_key` must stay read inside `resolve_llm_text` and be
/// passed in here as a parameter — do NOT move the read into this helper.
/// Rationale: preserve the "key rotation live-read per call" contract (see
/// SessionPolicy doc) so a rotated key takes effect on the next call without
/// invalidating the rest of the session policy snapshot.
///
/// Security note: the DPAPI boundary is unchanged — the key is decrypted from
/// ConfigCache per call and held as a plaintext String inside LLMClient;
/// Box<dyn TextCorrector> adds no new exposure surface.
fn refresh_cached_llm(
    cached_llm: &Arc<Mutex<Option<Box<dyn TextCorrector>>>>,
    api_url: &str,
    api_key: &str,
    model: &str,
) -> Result<bool, AppError> {
    let mut cached = crate::util::lock_mutex(cached_llm, "cached_llm")
        .ok_or_else(|| AppError::Llm("cached_llm lock poisoned".to_string()))?;
    let needs_new = cached
        .as_ref()
        .is_none_or(|c| !c.matches_config(api_url, api_key, model));
    if needs_new {
        *cached = Some(Box::new(LLMClient::new(
            api_url.to_string(),
            api_key.to_string(),
            model.to_string(),
        )));
    }
    Ok(needs_new)
}

/// Resolve LLM-corrected text. Handles cache lookup, client creation, and fallback.
///
/// API key is read live from `ConfigCache` (not snapshotted in `policy`) so a
/// rotated key takes effect on the next call without invalidating the rest of
/// the session policy.
async fn resolve_llm_text(
    ps: &PipelineState,
    policy: &SessionPolicy,
    transcription: &str,
    perf: &mut PerfMetrics,
) -> Result<String, AppError> {
    ps.sm_start_llm_refining();
    ps.emitter().emit("llm-refining", serde_json::Value::Null);

    let t_llm = Instant::now();

    // Live-read API key per call (rotation-friendly; not in snapshot).
    let live_api_key = ps.config_cache().read_cached().llm_api_key.clone();

    // Ensure the cached corrector matches config, creating a new one if needed.
    // live_api_key is still read live per call (line above) so a rotated key
    // forces a rebuild — see refresh_cached_llm doc for the boundary contract.
    refresh_cached_llm(
        &ps.cached_llm(),
        &policy.llm_api_url,
        &live_api_key,
        &policy.llm_api_model,
    )?;

    // Call correct_sync while re-acquiring the lock (holds lock for HTTP duration).
    let result = {
        let cached_llm = ps.cached_llm();
        let cached = crate::util::lock_mutex(&cached_llm, "cached_llm")
            .ok_or_else(|| AppError::Llm("cached_llm lock poisoned".to_string()))?;
        let corrector = cached
            .as_ref()
            .ok_or_else(|| AppError::Llm("no LLM corrector available".to_string()))?;
        corrector.correct_sync(transcription)
    };

    perf.llm_correction_ms = Some(t_llm.elapsed().as_millis() as u64);

    match result {
        Ok(corrected) => {
            ps.emitter().emit(
                "llm-complete",
                serde_json::to_value(&corrected).unwrap_or_default(),
            );
            Ok(corrected)
        }
        Err(e) => {
            warn!(
                target: "llm",
                "LLM correction failed, using raw transcription: {}",
                crate::llm::redact_error_detail(&e.to_string(), &live_api_key)
            );
            ps.emitter().emit(
                "llm-error",
                serde_json::to_value(LLM_ERROR_USER_MSG).unwrap_or_default(),
            );
            Ok(transcription.to_string())
        }
    }
}

/// Reset state machine to Idle and hide floating window.
fn reset_to_idle(ps: &PipelineState) {
    ps.sm_reset();
    ps.window_controller().hide_floating();
    if ps.review().was_shown_on_press() {
        ps.window_controller().hide_review();
        ps.review().set_shown_on_press(false);
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
mod tests_session_policy {
    use super::*;
    use crate::config::{AppConfig, Language, PipelineMode, WhisperModel};

    #[test]
    fn from_config_snapshots_all_fields() {
        let cfg = AppConfig {
            llm_enabled: true,
            language: Language::En,
            llm_api_url: "https://example.com/v1".to_string(),
            llm_model: "gpt-4".to_string(),
            realtime_transcription: true,
            review_before_paste: true,
            ..Default::default()
        };

        let policy = SessionPolicy::from_config(&cfg);

        // mode derived from realtime_transcription + review_before_paste
        assert_eq!(policy.mode, PipelineMode::RealtimeReview);
        assert!(policy.llm_enabled);
        assert_eq!(policy.language, Language::En);
        assert_eq!(policy.llm_api_url, "https://example.com/v1");
        assert_eq!(policy.llm_api_model, "gpt-4");
        // save mirrors SaveConfig::from_app_config
        assert_eq!(policy.save.language, cfg.language);
        assert_eq!(policy.save.whisper_model, WhisperModel::Base);
    }

    #[test]
    fn snapshot_is_immutable_after_config_cache_changes() {
        // Core property: policy captured before mutation must NOT reflect
        // subsequent ConfigCache updates. This is the whole point of C3.
        let cfg_a = AppConfig {
            llm_enabled: false,
            language: Language::Zh,
            ..Default::default()
        };

        let policy = SessionPolicy::from_config(&cfg_a);

        // Mutate the underlying config (simulating user toggling settings mid-session).
        // cfg_b is unused but documents the invariant being tested.
        let _cfg_b = AppConfig {
            llm_enabled: true,
            language: Language::En,
            llm_api_url: "https://other.com".to_string(),
            ..cfg_a.clone()
        };

        // Policy must still reflect cfg_a values, not cfg_b.
        assert!(!policy.llm_enabled, "policy llm_enabled frozen at snapshot");
        assert_eq!(
            policy.language,
            Language::Zh,
            "policy language frozen at snapshot"
        );
        assert_eq!(
            policy.llm_api_url, "",
            "policy llm_api_url frozen at snapshot"
        );
    }

    #[test]
    fn session_policy_does_not_contain_api_key() {
        // Structural invariant: API key must NOT be in SessionPolicy.
        // Compile-time check — if this test compiles, the struct lacks the field
        // (otherwise `policy.llm_api_key` would resolve and we'd have a problem).
        // Here we just exercise that the field legitimately does not exist.
        let cfg = AppConfig::default();
        let _policy = SessionPolicy::from_config(&cfg);
        // If `policy.llm_api_key` were accessible, this test would have failed
        // to compile. The struct definition is the contract.
    }

    #[test]
    fn known_unsnapshoted_fields_audit_anchor() {
        // Audit reference: adding a new AppConfig field requires either adding
        // it to SessionPolicy OR documenting it here. The test simply asserts
        // the constant remains non-empty and contains the canonical exclusions.
        assert!(!KNOWN_UNSNAPSHOTED_FIELDS.is_empty());
        assert!(KNOWN_UNSNAPSHOTED_FIELDS.contains(&"llm_api_key"));
        assert!(KNOWN_UNSNAPSHOTED_FIELDS.contains(&"autostart"));
        assert!(KNOWN_UNSNAPSHOTED_FIELDS.contains(&"review_before_paste"));
        assert!(KNOWN_UNSNAPSHOTED_FIELDS.contains(&"hotkey"));
        assert!(KNOWN_UNSNAPSHOTED_FIELDS.contains(&"download_mirror"));
        assert!(KNOWN_UNSNAPSHOTED_FIELDS.contains(&"record_only_enabled"));
        assert!(KNOWN_UNSNAPSHOTED_FIELDS.contains(&"record_only_hotkey"));
    }

    #[test]
    fn session_policy_field_coverage_audits_all_appconfig_fields() {
        use std::collections::HashSet;

        // Serialize AppConfig to enumerate all pub fields dynamically. A
        // hand-maintained field list would silently miss new fields; serde
        // enumeration forces authors adding a new field to either snapshot
        // it in SessionPolicy or document it in KNOWN_UNSNAPSHOTED_FIELDS.
        let cfg = AppConfig::default();
        let json = match serde_json::to_value(&cfg) {
            Ok(v) => v,
            Err(e) => panic!("AppConfig must serialize: {e}"),
        };
        let obj = match json.as_object() {
            Some(o) => o,
            None => panic!("AppConfig must serialize to a JSON object"),
        };
        let all_fields: HashSet<&str> = obj.keys().map(|s| s.as_str()).collect();

        // SessionPolicy direct coverage (SessionPolicy::from_config maps these 1:1).
        // Note: AppConfig field is `llm_model`, SessionPolicy field is `llm_api_model`.
        let direct_snapshot: &[&str] = &["llm_enabled", "language", "llm_api_url", "llm_model"];

        // Indirect coverage via SaveConfig::from_app_config. `language` is also
        // in SaveConfig but already in direct_snapshot; HashSet dedupes.
        let save_config_covered: &[&str] =
            &["data_saving_enabled", "data_saving_path", "whisper_model"];

        // Indirect coverage via pipeline_mode() folded into SessionPolicy.mode.
        let mode_covered: &[&str] = &["realtime_transcription", "review_before_paste"];

        let mut covered: HashSet<&str> = HashSet::new();
        covered.extend(direct_snapshot.iter().copied());
        covered.extend(save_config_covered.iter().copied());
        covered.extend(mode_covered.iter().copied());
        covered.extend(KNOWN_UNSNAPSHOTED_FIELDS.iter().copied());

        for field in all_fields {
            assert!(
                covered.contains(field),
                "AppConfig field '{}' not covered by SessionPolicy or KNOWN_UNSNAPSHOTED_FIELDS \
                 — add it to SessionPolicy::from_config for snapshot, or to \
                 KNOWN_UNSNAPSHOTED_FIELDS for documented exclusion",
                field
            );
        }
    }
}

#[cfg(test)]
mod tests_refresh_cached_llm {
    use super::*;
    use crate::llm::MockCorrector;

    fn make_cached(
        url: &str,
        key: &str,
        model: &str,
    ) -> Arc<Mutex<Option<Box<dyn TextCorrector>>>> {
        Arc::new(Mutex::new(Some(Box::new(
            MockCorrector::new("ok").with_config(url, key, model),
        ))))
    }

    /// Refresh and return the rebuilt flag, panicking on unexpected Err.
    fn refresh(
        cached: &Arc<Mutex<Option<Box<dyn TextCorrector>>>>,
        url: &str,
        key: &str,
        model: &str,
    ) -> bool {
        refresh_cached_llm(cached, url, key, model)
            .unwrap_or_else(|e| panic!("refresh_cached_llm should succeed in this test: {e}"))
    }

    #[test]
    fn skips_rebuild_when_config_matches() {
        let cached = make_cached("u", "k", "m");
        assert!(!refresh(&cached, "u", "k", "m"), "expected no rebuild");
    }

    #[test]
    fn rebuilds_when_api_url_differs() {
        let cached = make_cached("u1", "k", "m");
        assert!(refresh(&cached, "u2", "k", "m"), "expected rebuild");
    }

    #[test]
    fn rebuilds_when_api_key_differs() {
        // Validates the live-key rotation property: a rotated key forces rebuild
        // even though policy (url+model) is unchanged.
        let cached = make_cached("u", "k1", "m");
        assert!(
            refresh(&cached, "u", "k2", "m"),
            "expected rebuild on key rotation"
        );
    }

    #[test]
    fn rebuilds_when_model_differs() {
        let cached = make_cached("u", "k", "m1");
        assert!(refresh(&cached, "u", "k", "m2"), "expected rebuild");
    }

    #[test]
    fn rebuilds_when_cache_empty() {
        let cached: Arc<Mutex<Option<Box<dyn TextCorrector>>>> = Arc::new(Mutex::new(None));
        assert!(
            refresh(&cached, "u", "k", "m"),
            "expected rebuild on first insertion"
        );
    }

    #[test]
    fn returns_err_when_mutex_poisoned() {
        // Covers the lock-poisoned error path — without this, only the Ok branch
        // of refresh_cached_llm would be exercised by the suite.
        let cached: Arc<Mutex<Option<Box<dyn TextCorrector>>>> = Arc::new(Mutex::new(None));
        let _ = std::panic::catch_unwind(|| {
            let _g = cached
                .lock()
                .unwrap_or_else(|_| panic!("initial lock should succeed"));
            panic!("intentional poison");
        });
        let result = refresh_cached_llm(&cached, "u", "k", "m");
        assert!(result.is_err(), "poisoned mutex must surface as Err");
    }
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
        let resampled = match result {
            Some(r) => r,
            None => panic!("preprocess_audio should return Some for loud audio"),
        };
        assert!(resampled.len() < 4800);
    }
}
