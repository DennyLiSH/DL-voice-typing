//! Record-only session orchestration.
//!
//! Owns the full hold-to-record lifecycle for record-only mode: hotkey
//! press opens a streaming recording session (state gate → policy snapshot
//! → capture → streaming writer), release finalizes it off the hook thread.
//! `RecordOnlySession::recover` is the single cleanup DAG shared by release, the
//! backpressure monitor, the watchdog, and the tray reset, so timeout and
//! error mapping live in exactly one place.

use crate::audio::{AudioCallback, TARGET_SAMPLE_RATE};
use crate::commands::pipeline_state::PipelineState;
use crate::config::{Language, WhisperModel};
use crate::error::AppError;
use crate::hotkey::{HotkeyCallback, HotkeyEvent};
use crate::state::StateTag;
use crate::streaming_recorder::{
    FinalizeInfo, PushHandle, STOP_TIMEOUT, StreamingRecorder, exceeds_drop_threshold,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{error, info, warn};

/// Backpressure monitor poll interval.
const MONITOR_POLL_MS: u64 = 250;

/// Session-level config snapshot taken at press time, so mid-session
/// settings changes cannot affect an in-flight recording.
#[derive(Clone)]
pub(crate) struct RecordOnlyPolicy {
    pub(crate) data_saving_path: String,
    pub(crate) language: Language,
    pub(crate) whisper_model: WhisperModel,
}

/// Per-session push cell shared with the cpal callback. The callback is
/// registered before the recorder exists (capture must start first to
/// learn the device sample rate), so the callback receives the cell and the
/// session installs the recorder's `PushHandle` right after construction.
/// `recover` takes the handle back out so late callbacks no-op — the same
/// semantics the former slot lock had.
pub(crate) type PushCell = Arc<Mutex<Option<PushHandle>>>;

/// Active record-only recording held in `PipelineState.record_only` between
/// press and release/reset. Fields are private; lifecycle goes through the
/// module's session functions.
pub(crate) struct ActiveRecordOnly {
    recorder: StreamingRecorder,
    policy: RecordOnlyPolicy,
    push_cell: PushCell,
}

impl ActiveRecordOnly {
    /// Shared dropped-block counter. Test-only: consumed by
    /// `PipelineState::test_record_only_dropped_counter` (the session-layer
    /// monitor holds its own independent `Arc` from the recorder).
    #[cfg(test)]
    pub(crate) fn dropped_counter(&self) -> Arc<AtomicU64> {
        self.recorder.dropped_counter()
    }

    /// Finalize the recording (bounded writer join). Consumes the session.
    /// Test-only until a non-test consumer needs direct finalization.
    #[cfg(test)]
    pub(crate) fn stop_and_wait(
        self,
        timeout: Duration,
    ) -> Result<crate::streaming_recorder::FinalizeInfo, AppError> {
        self.recorder.stop_and_wait(timeout)
    }

    /// Test-only constructor (fields are private to this module).
    #[cfg(test)]
    pub(crate) fn new_for_test(recorder: StreamingRecorder, policy: RecordOnlyPolicy) -> Self {
        Self {
            recorder,
            policy,
            push_cell: Arc::new(Mutex::new(None)),
        }
    }
}

/// Deep module owning the record-only hold-to-record lifecycle, symmetric
/// to `RecordingSession` (ADR-0001): a unit orchestration type. Resources
/// stay aggregated in `PipelineState` (ADR-0004) and are reached only via
/// its interface.
pub(crate) struct RecordOnlySession;

/// Build the record-only hotkey callback (hold to record, release to save).
pub(crate) fn make_record_only_callback(ps: PipelineState) -> HotkeyCallback {
    Box::new(move |event| match event {
        HotkeyEvent::Pressed => RecordOnlySession::on_press(&ps),
        HotkeyEvent::Released => RecordOnlySession::on_release(&ps),
    })
}

impl RecordOnlySession {
    /// Hotkey press: gate on the state machine (mutual exclusion with the
    /// classic pipeline), snapshot the policy, start capture + streaming writer.
    pub(crate) fn on_press(ps: &PipelineState) {
        let cfg = ps.config_cache().read_cached();
        if !cfg.record_only_enabled {
            // Hotkey should not be registered when disabled; defense in depth.
            return;
        }
        // Atomic gate: rejects when Recording/Transcribing/... or already RecordOnly.
        if !ps.sm_start_record_only() {
            return;
        }
        let policy = RecordOnlyPolicy {
            data_saving_path: cfg.data_saving_path.clone(),
            language: cfg.language,
            whisper_model: cfg.whisper_model.clone(),
        };
        drop(cfg);

        if let Err(e) = Self::start_session(ps, &policy) {
            error!("record-only: failed to start session: {e}");
            ps.emitter().emit(
                "record-only-error",
                serde_json::json!({"message": "录音启动失败，请检查数据保存路径"}),
            );
            Self::stop_capture(ps);
            ps.sm_reset();
        }
    }

    /// Hotkey release: finalize off the hook thread — the writer join is
    /// bounded (STOP_TIMEOUT) but must never stall keyboard input.
    pub(crate) fn on_release(ps: &PipelineState) {
        if ps.sm_state() != Some(StateTag::RecordOnly) {
            return;
        }
        let ps_owned = ps.clone();
        let spawned = std::thread::Builder::new()
            .name("record-only-release".to_string())
            .spawn(move || Self::recover(&ps_owned));
        if spawned.is_err() {
            // Thread spawn failed: run inline as a last resort so the session
            // is never orphaned.
            Self::recover(ps);
        }
    }

    /// Single cleanup DAG shared by release, backpressure monitor, watchdog,
    /// and tray reset. Order: (1) stop capture, (2) finalize the WAV (bounded
    /// writer join), (3) write JSON metadata atomically, (4) return the state
    /// machine to Idle — (4) runs regardless of (2)/(3) outcomes.
    ///
    /// Idempotent: the first caller takes the session and owns JSON writing;
    /// later callers only guarantee the state machine is not orphaned.
    pub(crate) fn recover(ps: &PipelineState) {
        // (1) Always stop capture first (idempotent).
        Self::stop_capture(ps);

        // (2)+(3) Take + finalize + JSON. Only the first caller wins the take.
        let Some(session) = ps.take_record_only_session() else {
            if ps.sm_state() == Some(StateTag::RecordOnly) {
                warn!("recover: RecordOnly state without an active session; resetting");
                ps.sm_reset();
            }
            return;
        };

        // Detach the callback's push handle so late callbacks no-op.
        if let Some(mut cell) = crate::util::lock_mutex(&session.push_cell, "record_only_push") {
            *cell = None;
        }

        let stem = session.recorder.stem().to_string();
        let outcome = session.recorder.stop_and_wait(STOP_TIMEOUT);
        if let Err(e) = &outcome {
            error!("record-only: finalize failed for {stem}: {e}");
        }

        if let Err(e) = Self::write_session_json(&session.policy, &stem, &outcome) {
            error!("record-only: failed to write metadata for {stem}: {e}");
            ps.emitter().emit(
                "record-only-error",
                serde_json::json!({"message": "录音元数据写入失败"}),
            );
        }

        let (status, dropped) = Self::session_outcome(&outcome);
        ps.emitter().emit(
            "record-only-finished",
            serde_json::json!({
                "stem": stem,
                "status": status,
                "dropped_blocks": dropped,
            }),
        );
        info!("record_only_finalized: {stem} status={status} dropped={dropped}");

        // (4) State machine must return to Idle regardless of earlier failures.
        if !ps.sm_finish_record_only() {
            warn!("recover: sm_finish_record_only failed; forcing reset");
            ps.sm_reset();
        }
    }

    fn start_session(ps: &PipelineState, policy: &RecordOnlyPolicy) -> Result<(), AppError> {
        if policy.data_saving_path.trim().is_empty() {
            return Err(AppError::Config(
                "record-only mode requires a data saving path".to_string(),
            ));
        }

        // The cpal callback holds the per-session push cell (not the
        // PipelineState slot). The recorder's push handle is installed into the
        // cell right after construction; recover takes it out first, so late
        // callbacks no-op instead of locking shared session state.
        let push_cell: PushCell = Arc::new(Mutex::new(None));
        let cell_for_cb = push_cell.clone();
        let on_data: AudioCallback = Box::new(move |data: &[f32]| {
            let handle = crate::util::lock_mutex(&cell_for_cb, "record_only_push")
                .and_then(|g| g.as_ref().cloned());
            if let Some(handle) = handle {
                handle.push(data);
            }
        });

        // Start capture first so the device sample rate is known before the
        // recorder (and its resampler) is constructed.
        let sample_rate = {
            let ac_handle = ps.audio_capture();
            let Some(mut ac) = crate::util::lock_mutex(&ac_handle, "audio_capture") else {
                return Err(AppError::Audio("audio capture lock poisoned".to_string()));
            };
            ac.start(on_data)?;
            ac.sample_rate()
                .ok_or_else(|| AppError::Audio("sample rate unavailable after start".to_string()))?
        };

        let recorder = StreamingRecorder::start(Path::new(&policy.data_saving_path), sample_rate)?;
        if let Some(mut cell) = crate::util::lock_mutex(&push_cell, "record_only_push") {
            *cell = Some(recorder.push_handle());
        }
        let dropped = recorder.dropped_counter();
        let stem = recorder.stem().to_string();
        ps.set_record_only_session(ActiveRecordOnly {
            recorder,
            policy: policy.clone(),
            push_cell,
        });
        Self::spawn_backpressure_monitor(ps.clone(), dropped);
        ps.emitter()
            .emit("record-only-started", serde_json::json!({"stem": stem}));
        info!("record_only_started: {stem}");
        Ok(())
    }

    /// Monitor thread: controlled stop when the writer-thread backpressure
    /// exceeds the drop threshold (disk too slow / stalled). Exits as soon as
    /// the session ends for any other reason.
    fn spawn_backpressure_monitor(ps: PipelineState, dropped: Arc<AtomicU64>) {
        let _ = std::thread::Builder::new()
            .name("record-only-monitor".to_string())
            .spawn(move || {
                loop {
                    std::thread::sleep(Duration::from_millis(MONITOR_POLL_MS));
                    if ps.sm_state() != Some(StateTag::RecordOnly) {
                        return;
                    }
                    if exceeds_drop_threshold(dropped.load(Ordering::Relaxed)) {
                        warn!("record-only: dropped-block threshold exceeded; controlled stop");
                        Self::recover(&ps);
                        return;
                    }
                }
            });
    }

    fn stop_capture(ps: &PipelineState) {
        if let Some(mut ac) = crate::util::lock_mutex(&ps.audio_capture(), "audio_capture") {
            ac.stop();
        }
    }

    /// (status, dropped_blocks) for events and logs. A finalized recording with
    /// drops beyond the threshold is marked failed (the audio has holes).
    fn session_outcome(outcome: &Result<FinalizeInfo, AppError>) -> (&'static str, u64) {
        match outcome {
            Ok(info) if !exceeds_drop_threshold(info.dropped_blocks) => {
                ("pending", info.dropped_blocks)
            }
            Ok(info) => ("failed", info.dropped_blocks),
            Err(_) => ("failed", 0),
        }
    }

    /// Write the session JSON metadata (atomic). `transcription_status` starts
    /// at "pending" (awaiting user-triggered transcription) or "failed" when
    /// the audio is known to be incomplete.
    fn write_session_json(
        policy: &RecordOnlyPolicy,
        stem: &str,
        outcome: &Result<FinalizeInfo, AppError>,
    ) -> Result<(), AppError> {
        let json_path = PathBuf::from(&policy.data_saving_path).join(format!("{stem}.json"));
        let (status, duration_seconds, dropped) = match outcome {
            Ok(info) => {
                let status = if exceeds_drop_threshold(info.dropped_blocks) {
                    "failed"
                } else {
                    "pending"
                };
                (
                    status,
                    (info.duration_ms() as f64 * 1000.0).round() / 1_000_000.0,
                    info.dropped_blocks,
                )
            }
            // Finalize failed: caller (recover) already error!-logged the cause.
            Err(_) => ("failed", 0.0, 0),
        };
        let metadata = crate::data_saving::RecordingMetadata {
            timestamp: Some(crate::data_saving::now_rfc3339()),
            language: Some(policy.language),
            whisper_model: Some(policy.whisper_model.clone()),
            sample_rate: Some(TARGET_SAMPLE_RATE),
            duration_seconds: Some(duration_seconds),
            transcription_status: Some(status.to_string()),
            source: Some("record_only".to_string()),
            dropped_blocks: dropped,
            ..Default::default()
        };
        crate::data_saving::write_metadata_atomic(&json_path, &metadata)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::MockAudioCapture;
    use crate::clipboard::MockClipboard;
    use crate::commands::review_provider::MockReviewProvider;
    use crate::commands::window_controller::NoopWindowController;
    use crate::commands::{EventEmitter, MockEmitter};
    use crate::config::{AppConfig, ConfigCache};
    use crate::llm::MockCorrector;
    use crate::perf::PerfHistory;
    use crate::speech::mock::MockEngine;
    use crate::state::StateMachine;
    use std::fs;
    use std::sync::Mutex;

    fn build_ps(config: AppConfig) -> (PipelineState, Arc<MockEmitter>) {
        let emitter = Arc::new(MockEmitter::new());
        let ps = PipelineState::new(
            Arc::new(Mutex::new(StateMachine::new())),
            Arc::new(Mutex::new(MockAudioCapture::new())),
            Arc::new(MockEngine::new("test")),
            Arc::new(MockClipboard::new()),
            Arc::new(PerfHistory::new()),
            ConfigCache::new(config),
            Arc::new(Mutex::new(Some(Box::new(MockCorrector::new("x"))))),
            Arc::new(Mutex::new(None)),
            Arc::new(NoopWindowController),
            emitter.clone() as Arc<dyn EventEmitter>,
            Arc::new(MockReviewProvider::new()),
        );
        (ps, emitter)
    }

    fn record_only_config(dir: &Path) -> AppConfig {
        AppConfig {
            record_only_enabled: true,
            data_saving_path: dir.to_string_lossy().to_string(),
            ..Default::default()
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dl-vt-ro-session-{name}"));
        let _ = fs::remove_dir_all(&dir);
        assert!(fs::create_dir_all(&dir).is_ok());
        dir
    }

    fn emitted(emitter: &MockEmitter, event: &str) -> bool {
        emitter.take_events().iter().any(|(e, _)| e == event)
    }

    /// Session stem recorded by the most recent record-only-started event.
    fn started_stem(emitter: &MockEmitter) -> Option<String> {
        let events = emitter.take_events();
        let mut stem = None;
        for (e, payload) in events {
            if e == "record-only-started" {
                stem = payload
                    .get("stem")
                    .and_then(|s| s.as_str())
                    .map(|s| s.to_string());
            }
        }
        stem
    }

    #[test]
    fn test_press_starts_session() {
        let dir = temp_dir("press-starts");
        let (ps, emitter) = build_ps(record_only_config(&dir));
        RecordOnlySession::on_press(&ps);
        assert_eq!(ps.sm_state(), Some(StateTag::RecordOnly));
        assert!(ps.take_record_only_session().is_some());
        assert!(started_stem(&emitter).is_some());
        ps.sm_reset();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_press_rejected_when_not_idle() {
        let dir = temp_dir("press-rejected");
        let (ps, _) = build_ps(record_only_config(&dir));
        ps.force_state_tag(StateTag::Recording);
        RecordOnlySession::on_press(&ps);
        assert_eq!(ps.sm_state(), Some(StateTag::Recording));
        assert!(ps.take_record_only_session().is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_press_ignored_when_disabled() {
        let dir = temp_dir("press-disabled");
        let mut cfg = record_only_config(&dir);
        cfg.record_only_enabled = false;
        let (ps, _) = build_ps(cfg);
        RecordOnlySession::on_press(&ps);
        assert_eq!(ps.sm_state(), Some(StateTag::Idle));
        assert!(ps.take_record_only_session().is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_press_empty_path_errors_and_resets() {
        let mut cfg = record_only_config(Path::new(""));
        cfg.data_saving_path = String::new();
        let (ps, emitter) = build_ps(cfg);
        RecordOnlySession::on_press(&ps);
        assert_eq!(ps.sm_state(), Some(StateTag::Idle));
        assert!(emitted(&emitter, "record-only-error"));
        assert!(ps.take_record_only_session().is_none());
    }

    #[test]
    fn test_recover_happy_path_writes_pending_json() {
        let dir = temp_dir("recover-happy");
        let (ps, emitter) = build_ps(record_only_config(&dir));
        RecordOnlySession::on_press(&ps);
        let stem = started_stem(&emitter);
        assert!(stem.is_some());
        let Some(stem) = stem else { return };

        RecordOnlySession::recover(&ps);
        assert_eq!(ps.sm_state(), Some(StateTag::Idle));
        assert!(ps.take_record_only_session().is_none());

        let json_path = dir.join(format!("{stem}.json"));
        let wav_path = dir.join(format!("{stem}.wav"));
        assert!(json_path.exists());
        assert!(wav_path.exists());
        let content = fs::read_to_string(&json_path);
        assert!(content.is_ok());
        let Ok(content) = content else { return };
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap_or_default();
        assert_eq!(parsed["source"], "record_only");
        assert_eq!(parsed["transcription_status"], "pending");
        assert_eq!(parsed["dropped_blocks"], 0);
        // Lenient shape: skip_serializing_if omits empty segments on serialize
        // (readers treat missing and empty identically).
        assert!(parsed.get("segments").is_none_or(|v| v.is_array()));
        assert!(parsed["transcription"].is_null());
        assert!(emitted(&emitter, "record-only-finished"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_recover_json_failure_still_returns_idle() {
        let dir = temp_dir("recover-json-fail");
        let (ps, emitter) = build_ps(record_only_config(&dir));
        RecordOnlySession::on_press(&ps);
        let stem = started_stem(&emitter);
        assert!(stem.is_some());
        let Some(stem) = stem else { return };
        // Force the atomic JSON write to fail: a directory named stem.json
        // cannot be removed by remove_file nor overwritten by rename.
        assert!(fs::create_dir(dir.join(format!("{stem}.json"))).is_ok());

        RecordOnlySession::recover(&ps);
        assert_eq!(ps.sm_state(), Some(StateTag::Idle));
        assert!(ps.take_record_only_session().is_none());
        let events: Vec<String> = emitter.take_events().into_iter().map(|(e, _)| e).collect();
        assert!(events.iter().any(|e| e == "record-only-error"));
        // The finished event still fires so the UI can refresh.
        assert!(events.iter().any(|e| e == "record-only-finished"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_recover_is_idempotent() {
        let dir = temp_dir("recover-idempotent");
        let (ps, _) = build_ps(record_only_config(&dir));
        RecordOnlySession::on_press(&ps);
        RecordOnlySession::recover(&ps);
        RecordOnlySession::recover(&ps);
        assert_eq!(ps.sm_state(), Some(StateTag::Idle));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_recover_orphaned_state_resets() {
        let dir = temp_dir("recover-orphan");
        let (ps, _) = build_ps(record_only_config(&dir));
        // RecordOnly tag with no active session (cross-session leak).
        ps.force_state_tag(StateTag::RecordOnly);
        RecordOnlySession::recover(&ps);
        assert_eq!(ps.sm_state(), Some(StateTag::Idle));
        let _ = fs::remove_dir_all(&dir);
    }

    /// Dropped audio never reaches the WAV: after a threshold stop the file
    /// duration reflects only written samples, while `dropped_blocks` records
    /// the lost wall-clock audio. The two numbers legitimately diverge.
    #[test]
    fn test_dropped_blocks_wav_shorter_than_wall_clock() {
        let dir = temp_dir("drop-duration");
        let (ps, _) = build_ps(record_only_config(&dir));
        RecordOnlySession::on_press(&ps);
        assert_eq!(ps.sm_state(), Some(StateTag::RecordOnly));

        // Push 1 second of real audio through the cpal callback path
        // (MockAudioCapture delivers straight into the registered callback).
        if let Some(mut ac) = crate::util::lock_mutex(&ps.audio_capture(), "audio_capture") {
            ac.deliver(&vec![0.3f32; 48_000]);
        }
        // Simulate 51 lost blocks (~3.2s of audio that never made it to disk).
        let counter = ps.test_record_only_dropped_counter();
        assert!(counter.is_some());
        let Some(counter) = counter else { return };
        counter.fetch_add(51, Ordering::Relaxed);

        let deadline = std::time::Instant::now() + Duration::from_secs(4);
        while ps.sm_state() == Some(StateTag::RecordOnly) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(ps.sm_state(), Some(StateTag::Idle));

        // WAV on disk: ~1s (written samples only), not ~4.2s (wall clock).
        let entries: Vec<_> = fs::read_dir(&dir)
            .map(|rd| rd.flatten().collect())
            .unwrap_or_default();
        let wav_entry = entries
            .iter()
            .find(|e| e.path().extension().and_then(|x| x.to_str()) == Some("wav"));
        assert!(wav_entry.is_some());
        let Some(wav_entry) = wav_entry else { return };
        let wav_len = wav_entry.metadata().map(|m| m.len()).unwrap_or(0);
        assert!(wav_len >= 44);
        let data_size = wav_len - 44;
        let duration_ms = data_size * 1000 / (16_000 * 2);
        // ~1s of written audio (resampler block granularity), not the ~4.2s
        // wall clock including the 51 dropped blocks.
        assert!((900..=1100).contains(&duration_ms));

        // JSON remembers the lost audio separately.
        let json_entry = entries
            .iter()
            .find(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"));
        assert!(json_entry.is_some());
        let Some(json_entry) = json_entry else { return };
        let content = fs::read_to_string(json_entry.path());
        assert!(content.is_ok());
        let Ok(content) = content else { return };
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap_or_default();
        assert_eq!(parsed["transcription_status"], "failed");
        assert!(parsed["dropped_blocks"].as_u64().unwrap_or(0) >= 51);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_backpressure_monitor_controlled_stop() {
        let dir = temp_dir("backpressure");
        let (ps, _) = build_ps(record_only_config(&dir));
        RecordOnlySession::on_press(&ps);
        assert_eq!(ps.sm_state(), Some(StateTag::RecordOnly));

        // Drive the dropped-block counter past the threshold; the monitor
        // (spawned at press, 250ms poll) must stop the session on its own.
        let counter = ps.test_record_only_dropped_counter();
        assert!(counter.is_some());
        let Some(counter) = counter else { return };
        counter.fetch_add(51, Ordering::Relaxed);

        // Wait (up to ~4s) for the monitor to run the cleanup DAG.
        let deadline = std::time::Instant::now() + Duration::from_secs(4);
        while ps.sm_state() == Some(StateTag::RecordOnly) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(ps.sm_state(), Some(StateTag::Idle));

        // JSON must carry the failed status and the dropped count.
        let entries: Vec<_> = fs::read_dir(&dir)
            .map(|rd| rd.flatten().collect())
            .unwrap_or_default();
        let json_entry = entries
            .iter()
            .find(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"));
        assert!(json_entry.is_some());
        let Some(json_entry) = json_entry else { return };
        let content = fs::read_to_string(json_entry.path());
        assert!(content.is_ok());
        let Ok(content) = content else { return };
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap_or_default();
        assert_eq!(parsed["transcription_status"], "failed");
        assert!(parsed["dropped_blocks"].as_u64().unwrap_or(0) >= 51);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_on_release_ignores_non_record_only_state() {
        let dir = temp_dir("release-ignore");
        let (ps, _) = build_ps(record_only_config(&dir));
        // Idle: release must be a no-op (no thread, no state change).
        RecordOnlySession::on_release(&ps);
        assert_eq!(ps.sm_state(), Some(StateTag::Idle));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_write_session_json_finalize_failure_records_failed() {
        let dir = temp_dir("json-finalize-fail");
        let policy = RecordOnlyPolicy {
            data_saving_path: dir.to_string_lossy().to_string(),
            language: crate::config::Language::Zh,
            whisper_model: crate::config::WhisperModel::default(),
        };
        let outcome = Err(AppError::Audio("simulated finalize failure".to_string()));
        RecordOnlySession::write_session_json(&policy, "2026-08-19_12-00-00", &outcome).unwrap();
        let json_path = dir.join("2026-08-19_12-00-00.json");
        let parsed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&json_path).unwrap()).unwrap();
        assert_eq!(parsed["transcription_status"], "failed");
        assert_eq!(parsed["source"], "record_only");
        assert_eq!(parsed["duration_seconds"], 0.0);
        assert_eq!(parsed["dropped_blocks"], 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
