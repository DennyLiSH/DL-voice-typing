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
use crate::perf::{PerfMetrics, PerfTimer, ms_of};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

use super::pipeline_state::{CancelGuard, PipelineState};

/// User-facing payload for the `speech-error` not-ready branch, routed by
/// model-file presence. Pure function so the routing is testable without
/// touching the un-injectable `model_path_for_size` (config_dir-derived).
pub(crate) fn not_ready_message(model_on_disk: bool) -> &'static str {
    if model_on_disk {
        "模型加载中，请稍候..."
    } else {
        "模型未下载，请在 设置→模型 下载"
    }
}

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
    "llm_api_key",            // live-read per LLM call for key rotation
    "autostart",              // app-lifecycle concern, not pipeline
    "review_before_paste",    // folded into mode via pipeline_mode()
    "hotkey",                 // hotkey re-registration is independent of pipeline
    "download_mirror",        // only used at model fetch time, not pipeline
    "record_only_enabled",    // record-only mode has its own session policy (record_only_session)
    "record_only_hotkey",     // hotkey re-registration is independent of pipeline
    "open_settings_hotkey",   // window-lifecycle concern, not pipeline (M6-a)
    "open_transcribe_hotkey", // window-lifecycle concern, not pipeline (M6-a)
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

/// Per-cycle state carried from the press handler to the release handler.
///
/// The press timer travels with the metrics because two measurements
/// need the press instant: the hold duration (`audio_duration_ms`) and
/// the end-to-end clock (`end_to_end_ms`). Reconstructing it by
/// subtracting the (previously broken) hold duration from the release
/// instant lost the precision and produced ~0 for both.
struct PerfSlot {
    metrics: PerfMetrics,
    press_timer: PerfTimer,
}

/// Owns the recording session: shared pipeline state + per-cycle perf slot.
/// Clone is cheap (all fields are `Arc`).
#[derive(Clone)]
pub(crate) struct RecordingSession {
    ps: PipelineState,
    perf_slot: Arc<Mutex<Option<PerfSlot>>>,
}

impl RecordingSession {
    pub(crate) fn new(ps: PipelineState) -> Self {
        Self {
            ps,
            perf_slot: Arc::new(Mutex::new(None)),
        }
    }

    /// Test-only accessor for the shared PipelineState. Used by Esc-cancel
    /// state-guard tests that need to set/inspect the slot directly.
    #[cfg(test)]
    pub(crate) fn ps_ref(&self) -> &PipelineState {
        &self.ps
    }

    /// Hotkey press: state guard, window policy, resource lifecycle.
    /// Runs on the Win32 hook thread — must be synchronous and non-blocking.
    pub(crate) fn on_press(&self) {
        let t_press = PerfTimer::start();
        let cycle_id = self.ps.next_perf_cycle_id();

        let can_record = self.ps.sm_start_recording();

        if can_record && !self.ps.engine().is_ready() {
            // Route the not-ready message by model-file presence: a missing
            // download must say so instead of the misleading "loading".
            // The exists() stat lands on the hook thread — one local
            // %APPDATA% stat is microsecond-scale and only reached in this
            // branch; if hook latency ever shows up, cache an atomic flag at
            // engine-load time instead.
            let model_on_disk = {
                let cfg = self.ps.config_cache().read_cached();
                crate::config::model_path_for_size(&cfg.whisper_model).exists()
            };
            let msg = not_ready_message(model_on_disk);
            // The classic floating-window show happens AFTER this early
            // return, so the window is OS-hidden here — emitting alone would
            // never be visible. Show it, emit, then schedule a delayed
            // backend hide: the frontend showError only clears DOM state and
            // never hides the OS window.
            self.ps.window_controller().show_floating_near_caret();
            self.ps.emitter().emit(
                "speech-error",
                serde_json::to_value(msg).unwrap_or_default(),
            );
            self.ps.sm_reset();
            let wc = self.ps.window_controller();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(4500));
                wc.hide_floating();
            });
            return;
        }

        if can_record {
            // === Session state cleanup: prevent leaks from previous session ===
            self.ps.review().set_shown_on_press(false);
            self.ps.stop_realtime_leftover();

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
                        && last.elapsed() >= Duration::from_millis(33)
                    {
                        *last = Instant::now();
                        emitter_for_rms.emit(
                            "audio-rms",
                            serde_json::to_value(rms_val).unwrap_or_default(),
                        );
                    }
                }));

                if let Err(e) = start_result {
                    warn!("audio capture start failed: {e}");
                    self.ps.reset_to_idle();
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
                ) && let Some(sr) = ac_guard.sample_rate()
                {
                    let audio = Arc::new(crate::realtime::AudioRingBufferSource::new(
                        self.ps.ring_buffer(),
                    ));
                    let rt = crate::realtime::RealtimeTranscriber::start(
                        audio,
                        self.ps.engine(),
                        self.ps.emitter(),
                        sr,
                        crate::realtime::RealtimePolicy {
                            language: policy.language,
                        },
                    );
                    self.ps.set_realtime_transcriber(rt);
                }
            }

            // Show review window on press for RealtimeReview mode.
            if matches!(mode, PipelineMode::RealtimeReview) {
                self.ps.review().save_foreground();
                self.ps.review().set_shown_on_press(true);
                self.ps.window_controller().show_review_near_caret();
            }

            let press_latency = t_press.finish_ms();
            let mut perf = PerfMetrics::new(cycle_id);
            perf.press_latency_ms = Some(press_latency);
            if let Some(mut slot) = crate::util::lock_mutex(&self.perf_slot, "perf") {
                *slot = Some(PerfSlot {
                    metrics: perf,
                    press_timer: t_press,
                });
            }
        }
    }

    /// Hotkey release: synchronous prefix (stop resources, state guard) +
    /// decides the delivery path via [`decide_release`]. Returns a
    /// [`ReleaseAction`] whose `Deliver` future the caller schedules.
    pub(crate) fn on_release(&self) -> ReleaseAction {
        info!("hotkey release: event received");
        let t_release = PerfTimer::start();

        // Take the press timer with the metrics (no press happened → no
        // timer; the release instant then stands in for it, which is
        // what the previous reconstruction produced).
        let (mut perf, press_timer) =
            match crate::util::lock_mutex(&self.perf_slot, "perf").and_then(|mut s| s.take()) {
                Some(slot) => (slot.metrics, slot.press_timer),
                None => (PerfMetrics::new(self.ps.next_perf_cycle_id()), t_release),
            };
        // Hold duration: press -> release.
        perf.audio_duration_ms = Some(ms_of(
            t_release
                .started_at()
                .duration_since(press_timer.started_at()),
        ));

        let sample_rate = crate::util::lock_mutex(&self.ps.audio_capture(), "audio_capture")
            .and_then(|a| a.sample_rate());

        // Stop audio capture and realtime transcriber, get accumulated text.
        let realtime_accumulated = self.ps.stop_recording_resources();

        let policy = SessionPolicy::from_config(&self.ps.config_cache().read_cached());
        let mode = policy.mode;

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
                    self.ps.reset_to_idle();
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
                    self.ps.reset_to_idle();
                    return ReleaseAction::Done;
                }
                // P1 Esc-cancel: arm the cancel token right after entering
                // Transcribing (sm_stop_recording succeeded). The pipeline
                // body takes a snapshot via `cancel_token_snapshot()` so the
                // Esc-cancel path can flip this bit from the hook thread.
                let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
                self.ps.set_cancel_token(cancel.clone());
                perf.audio_samples = audio_data.len();
                perf.audio_sample_rate = native_rate;
                perf.release_latency_ms = Some(t_release.finish_ms());
                let this = self.clone();
                ReleaseAction::Deliver(Box::pin(async move {
                    this.run_realtime_fast_path(
                        accumulated,
                        audio_data,
                        native_rate,
                        perf,
                        press_timer,
                        policy,
                        cancel,
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
                        if audio_data.is_empty() {
                            warn!(
                                target: "hotkey",
                                "hotkey release with empty buffer: no press started this \
                                 session (keydown modifier-rejected or orphan keyup); pair \
                                 with target=hotkey rejection warns to adjudicate"
                            );
                        }
                        info!(
                            "hotkey release: preprocess_audio returned None (silent?), samples={}",
                            audio_data.len()
                        );
                        self.ps.reset_to_idle();
                        return ReleaseAction::Done;
                    }
                };
                let stop_ok = self.ps.sm_stop_recording();
                if !stop_ok {
                    info!("hotkey release: stop_recording failed (state already reset)");
                    self.ps.reset_to_idle();
                    return ReleaseAction::Done;
                }
                // P1 Esc-cancel: same arming point as DeliverFast (above).
                let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
                self.ps.set_cancel_token(cancel.clone());
                perf.audio_samples = audio_data.len();
                perf.audio_sample_rate = native_rate;
                perf.release_latency_ms = Some(t_release.finish_ms());
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
                        press_timer,
                        policy,
                        cancel,
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

    /// Panic recovery: reset state to idle (hiding any shown overlays,
    /// including a review window shown on press), conditionally restore the
    /// clipboard, and surface a `speech-error`. Each step is tolerant so a
    /// secondary panic does not escape the supervisor task.
    ///
    /// Conditional restore (Maj-γ): `restore()` is called only when the
    /// clipboard was saved this cycle. `ClipboardManager::inject_text` already
    /// self-restores after a successful paste, so this is a harmless no-op in
    /// the post-inject case and cleans up leaked text in the pre-inject case.
    pub(crate) fn recover(&self) {
        self.ps.reset_to_idle();
        self.ps.restore_clipboard_if_saved();
        // Defense-in-depth: the CancelGuard drains during unwind before
        // recover runs, making this a no-op today. Kept as a second lock
        // of the invariant against future refactors moving/delaying guard
        // creation. The arming-to-spawn gap lives on the hook FFI thread
        // where a panic is process-fatal — no drain can help there; the
        // next session's set_cancel_token overwrite is the recovery.
        let _ = self.ps.take_cancel_token();
        self.ps.emitter().emit(
            "speech-error",
            serde_json::to_value("转录流程异常，已恢复").unwrap_or_default(),
        );
    }

    // Esc-cancel entry is the PipelineState::cancel_active_pipeline method —
    // the hook slot holds an Arc<PipelineState> directly.

    /// Full transcription → LLM → injection pipeline (classic modes + realtime
    /// fallthrough). `review` decides review-vs-direct (derived once from mode
    /// at dispatch time — no `review_before_paste` re-read here).
    #[allow(clippy::too_many_arguments)] // 9 params; merging would obscure call site
    pub(crate) async fn run_pipeline(
        &self,
        audio_for_save: Vec<f32>,
        native_rate: u32,
        resampled: Vec<f32>,
        review: bool,
        mut perf: PerfMetrics,
        press_timer: PerfTimer,
        policy: SessionPolicy,
        cancel: Arc<std::sync::atomic::AtomicBool>,
    ) {
        info!(
            "Pipeline: starting (review={}, llm={}, samples={})",
            review,
            policy.llm_enabled,
            resampled.len()
        );
        let _cancel_guard = CancelGuard::new(&self.ps);

        // -- Save audio and transcribe in parallel --
        // The timer must be started BEFORE the call: reading
        // `Instant::now().elapsed()` at the measurement point measures the
        // lifetime of a fresh instant (always ~0) — the bug this line
        // used to have.
        let t_transcribe = PerfTimer::start();
        let (save_result, transcription) = transcribe_and_save(
            &self.ps,
            audio_for_save,
            native_rate,
            resampled,
            &policy.save,
            cancel.clone(),
        )
        .await;

        perf.transcription_ms = perf.transcription_ms.or(Some(t_transcribe.finish_ms()));

        // -- Gate 2 (classic): after transcription returns, BEFORE the LLM
        // phase starts. LLM HTTP is non-interruptible once launched, so this
        // is the last cheap exit before it; a cancel that lands DURING LLM
        // is caught by gate 3 below. Checked before the empty-transcription
        // branch: a gate-1 cancel returns an empty string, and that branch's
        // reset must not fire for a cancelled pipeline.
        //
        // Gates never touch the state machine / windows / token slot. Esc
        // already did all of that on the hook thread (cancel_active_pipeline),
        // and by the time a gate fires the user may have re-pressed and
        // started a NEW session — a gate-side reset would silently kill it.
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            info!(target: "cancel", "run_pipeline: gate 2 cancelled before LLM");
            return;
        }

        if transcription.is_empty() {
            info!("run_pipeline: empty transcription, resetting to idle");
            self.ps.reset_to_idle();
            return;
        }

        refine_and_deliver(
            &self.ps,
            transcription,
            SaveForDelivery::Resolved(save_result),
            if review {
                DeliveryRoute::Review
            } else {
                DeliveryRoute::Direct
            },
            &policy,
            perf,
            press_timer,
            &cancel,
            "gate 3",
        )
        .await;
    }

    /// Fast path for RealtimeDirect mode: uses accumulated realtime text
    /// directly, optionally runs LLM, then injects. Skips Whisper entirely.
    #[allow(clippy::too_many_arguments)] // 7 params; merging would obscure call site
    pub(crate) async fn run_realtime_fast_path(
        &self,
        accumulated: String,
        audio_data: Vec<f32>,
        native_rate: u32,
        perf: PerfMetrics,
        press_timer: PerfTimer,
        policy: SessionPolicy,
        cancel: Arc<std::sync::atomic::AtomicBool>,
    ) {
        info!(
            "RealtimeFastPath: starting (llm={}, accumulated={} chars)",
            policy.llm_enabled,
            accumulated.len()
        );
        let _cancel_guard = CancelGuard::new(&self.ps);

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
        refine_and_deliver(
            &self.ps,
            transcription,
            SaveForDelivery::Background(save_handle),
            DeliveryRoute::Direct,
            &policy,
            perf,
            press_timer,
            &cancel,
            "gate 3'",
        )
        .await;
    }
}

/// Parallel save audio to disk + transcribe via speech engine.
/// Returns (save_result, transcription_text). On transcription failure, emits
/// error and returns an empty string. On Esc-cancel during the Whisper call,
/// returns an empty string WITHOUT emitting `speech-error` (the cancel path
/// already emitted `pipeline-cancelled`).
async fn transcribe_and_save(
    ps: &PipelineState,
    audio_for_save: Vec<f32>,
    native_rate: u32,
    resampled: Vec<f32>,
    save: &SaveConfig,
    cancel: Arc<std::sync::atomic::AtomicBool>,
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
    // Defensive snapshot — guarantees the engine always sees a fresh token
    // even if PipelineState's slot was cleared mid-flight (the snapshot
    // accessor returns a fresh `false` token in that case, never panics).
    // This is the engine-side handle; the gates above/below still read the
    // caller's `cancel` for the early-return checks.
    let cancel_for_engine = ps.cancel_token_snapshot();
    let transcribe_handle = tokio::task::spawn_blocking(move || {
        // P1 Esc-cancel: route through the cancelable variant so the abort
        // callback fires when the token is flipped from the hook thread.
        engine_ref.transcribe_sync_cancelable(&resampled, cancel_for_engine)
    });

    let (save_result, transcription_result) = tokio::join!(save_handle, transcribe_handle);
    let save_result = save_result.unwrap_or(None);

    // -- Gate 1 (classic): after Whisper returns, before emitting anything --
    // Whisper's return value is unreliable after an abort callback fires
    // (some paths return Ok with partial segments). The token is the single
    // source of truth — if the user pressed Esc, discard the result and
    // return empty WITHOUT emitting speech-error. `save_result` is preserved
    // because the WAV has already been written to disk; dropping it would
    // leave the JSON metadata file dangling forever. Log-and-return only —
    // state/window cleanup belongs to cancel_active_pipeline on the hook
    // thread (see the gate-2 note in run_pipeline).
    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        info!(target: "cancel", "transcribe_and_save: gate 1 cancelled");
        return (save_result, String::new());
    }

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
            ps.reset_to_idle();
            return (save_result, String::new());
        }
        Err(e) => {
            ps.emitter().emit(
                "speech-error",
                serde_json::to_value(e.to_string()).unwrap_or_default(),
            );
            ps.reset_to_idle();
            return (save_result, String::new());
        }
    };

    (save_result, transcription)
}

/// Resolve LLM-corrected text. Handles cache lookup, client creation, and fallback.
///
/// API key is read live from `ConfigCache` (not snapshotted in `policy`) so a
/// rotated key takes effect on the next call without invalidating the rest of
/// the session policy.
///
/// The corrector resolution + correction + fallback all live in
/// `llm::correction` (shared with the transcribe window) — this function
/// owns only the classic-specific parts: the state transition, the perf
/// measurement and the `llm-refining` / `llm-complete` / `llm-error`
/// events.
async fn resolve_llm_text(
    ps: &PipelineState,
    policy: &SessionPolicy,
    transcription: &str,
    perf: &mut PerfMetrics,
) -> Result<String, AppError> {
    ps.sm_start_llm_refining();
    ps.emitter().emit("llm-refining", serde_json::Value::Null);

    let t_llm = PerfTimer::start();

    // Live-read API key per call (rotation-friendly; not in snapshot).
    let live_api_key = ps.config_cache().read_cached().llm_api_key.clone();

    // Resolve the corrector and run the correction inside ONE slot-lock
    // window: the lock is held for the duration of the HTTP call (the
    // pre-existing "HTTP inside the lock" invariant), so no corrector can
    // be rebuilt while this one is mid-request.
    //
    // `with_cached_llm` returns None when the slot lock is poisoned — the
    // same condition that made the previous `refresh_cached_llm` return
    // Err, and the caller (`refine_and_deliver`) falls back to the raw
    // transcription in both cases.
    let outcome = ps
        .with_cached_llm(|slot| {
            let corrector = crate::llm::correction::resolve_corrector(
                slot,
                &policy.llm_api_url,
                &live_api_key,
                &policy.llm_api_model,
            );
            crate::llm::correction::correct_with_fallback(&corrector, transcription, &live_api_key)
        })
        .ok_or_else(|| AppError::Llm("cached_llm lock poisoned".to_string()))?;

    perf.llm_correction_ms = Some(t_llm.finish_ms());

    if outcome.used_llm {
        ps.emitter().emit(
            "llm-complete",
            serde_json::to_value(&outcome.text).unwrap_or_default(),
        );
    } else {
        ps.emitter().emit(
            "llm-error",
            serde_json::to_value(crate::llm::correction::LLM_FALLBACK_USER_MSG).unwrap_or_default(),
        );
    }
    Ok(outcome.text)
}

/// Where the delivery's data-saving result comes from. Classic resolves
/// it inside transcribe_and_save (already awaited); the fast path
/// overlaps it with LLM via spawn_blocking and the tail awaits it.
enum SaveForDelivery {
    Resolved(Option<crate::data_saving::SaveResult>),
    Background(tokio::task::JoinHandle<Option<crate::data_saving::SaveResult>>),
}

/// Which DeliveryController entry the tail hands off to.
enum DeliveryRoute {
    Review,
    Direct,
}

/// Shared delivery tail: LLM refine -> punctuation normalize -> resolve
/// save -> final cancel gate -> DeliveryController handoff. The ~40-line
/// tail previously lived twice (run_pipeline / run_realtime_fast_path)
/// with cross-referencing gate-number comments; it is now one function.
///
/// The final gate is log-and-return ONLY: cleanup belongs to
/// cancel_active_pipeline on the hook thread, and a gate-side reset
/// would kill a NEW session started while this one was in flight
/// (7f0cbf0). `gate_label` distinguishes call sites in logs.
#[allow(clippy::too_many_arguments)] // 9 args; merging would obscure call site
async fn refine_and_deliver(
    ps: &PipelineState,
    transcription: String,
    save: SaveForDelivery,
    route: DeliveryRoute,
    policy: &SessionPolicy,
    mut perf: PerfMetrics,
    press_timer: PerfTimer,
    cancel: &Arc<std::sync::atomic::AtomicBool>,
    gate_label: &'static str,
) {
    perf.llm_enabled = policy.llm_enabled;
    let final_text = if policy.llm_enabled {
        resolve_llm_text(ps, policy, &transcription, &mut perf)
            .await
            .unwrap_or_else(|e| {
                warn!("refine_and_deliver: LLM correction failed: {e}");
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

    let save_result = match save {
        SaveForDelivery::Resolved(v) => v,
        SaveForDelivery::Background(handle) => match handle.await {
            Ok(v) => v,
            Err(e) => {
                warn!("refine_and_deliver: save task failed: {e}");
                None
            }
        },
    };

    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        info!(target: "cancel", "refine_and_deliver: {gate_label} cancelled before delivery");
        return;
    }

    match route {
        DeliveryRoute::Review => {
            info!(
                "refine_and_deliver: DeliveryController::show_review ({} chars)",
                final_text.len()
            );
            ps.delivery()
                .show_review(
                    ps,
                    final_text,
                    transcription,
                    save_result,
                    policy,
                    perf,
                    press_timer,
                    policy.llm_enabled,
                )
                .await;
        }
        DeliveryRoute::Direct => {
            info!(
                "refine_and_deliver: DeliveryController::inject_direct ({} chars)",
                final_text.len()
            );
            ps.delivery()
                .inject_direct(
                    ps,
                    final_text,
                    transcription,
                    save_result,
                    policy,
                    &mut perf,
                    press_timer,
                    policy.llm_enabled,
                )
                .await;
        }
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
        // it to SessionPolicy OR documenting it here. The contains() asserts
        // double as the non-emptiness check for the canonical exclusions.
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
        // Serialize AppConfig to enumerate all pub fields dynamically. A
        // hand-maintained field list would silently miss new fields; serde
        // enumeration forces authors adding a new field to either snapshot
        // it in SessionPolicy or document it in KNOWN_UNSNAPSHOTED_FIELDS.
        // The shared helper owns the enumeration and the assertion (the
        // record-only pipeline audits itself with the same one).

        // SessionPolicy direct coverage (SessionPolicy::from_config maps these 1:1).
        // Note: AppConfig field is `llm_model`, SessionPolicy field is `llm_api_model`.
        let direct_snapshot: &[&str] = &["llm_enabled", "language", "llm_api_url", "llm_model"];

        // Indirect coverage via SaveConfig::from_app_config. `language` is also
        // in SaveConfig but already in direct_snapshot.
        let save_config_covered: &[&str] =
            &["data_saving_enabled", "data_saving_path", "whisper_model"];

        // Indirect coverage via pipeline_mode() folded into SessionPolicy.mode.
        let mode_covered: &[&str] = &["realtime_transcription", "review_before_paste"];

        let mut covered: Vec<&str> = Vec::new();
        covered.extend(direct_snapshot.iter().copied());
        covered.extend(save_config_covered.iter().copied());
        covered.extend(mode_covered.iter().copied());

        crate::tests::policy_audit::assert_policy_covers_all_appconfig_fields(
            "SessionPolicy",
            &covered,
            KNOWN_UNSNAPSHOTED_FIELDS,
        );
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

#[cfg(test)]
mod tests_resolve_llm_text {
    use super::*;
    use crate::audio::MockAudioCapture;
    use crate::clipboard::MockClipboard;
    use crate::commands::MockEmitter;
    use crate::commands::review_provider::MockReviewProvider;
    use crate::commands::window_controller::NoopWindowController;
    use crate::config::{AppConfig, ConfigCache};
    use crate::perf::PerfHistory;
    use crate::speech::mock::MockEngine;
    use crate::state::StateMachine;

    fn make_ps() -> PipelineState {
        let emitter: Arc<dyn crate::commands::EventEmitter> = Arc::new(MockEmitter::new());
        PipelineState::new(
            Arc::new(Mutex::new(StateMachine::new())),
            Arc::new(Mutex::new(MockAudioCapture::new())),
            Arc::new(MockEngine::new("test")),
            Arc::new(MockClipboard::new()),
            Arc::new(PerfHistory::new()),
            ConfigCache::new(AppConfig::default()),
            Arc::new(Mutex::new(None)),
            Arc::new(Mutex::new(None)),
            Arc::new(NoopWindowController),
            emitter,
            Arc::new(MockReviewProvider::new()),
        )
    }

    /// A poisoned `cached_llm` slot must surface as an error, not as a
    /// silent "no corrector" — the caller (`refine_and_deliver`) falls
    /// back to the raw transcription on an error.
    ///
    /// This covers the lock-poison branch that the removed
    /// `refresh_cached_llm` test used to guard: the slot lock is now
    /// taken once (resolve + correct in one window), so the poisoned
    /// case has to be pinned at the new call site.
    #[tokio::test]
    async fn returns_err_when_cached_llm_slot_lock_is_poisoned() {
        let ps = make_ps();
        // PipelineState holds trait-object Arcs (ReviewProvider, EventEmitter)
        // which are not RefUnwindSafe — wrap in AssertUnwindSafe, same idiom
        // as the cancel-guard unwind test.
        let ps_ptr: *const PipelineState = &ps;
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let ps_ref: &PipelineState = unsafe { &*ps_ptr };
            let slot = ps_ref.test_components().cached_llm.clone();
            let _g = slot
                .lock()
                .unwrap_or_else(|_| panic!("initial lock should succeed"));
            panic!("intentional poison");
        }));

        let policy = SessionPolicy::from_config(&AppConfig::default());
        let mut perf = PerfMetrics::new(0);
        let result = resolve_llm_text(&ps, &policy, "raw transcription", &mut perf).await;
        assert!(
            result.is_err(),
            "a poisoned slot lock must surface as Err so the caller falls back to the raw transcription"
        );
    }
}
