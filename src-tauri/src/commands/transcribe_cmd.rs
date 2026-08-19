//! Transcribe window commands (record-only mode).
//!
//! Five commands per the API contract: `open_transcribe_window`,
//! `transcribe_recording`, `cancel_transcription`, `get_recording_segments`,
//! `inject_transcript_text`. `PendingTranscribe` owns the take-once target
//! HWND and the in-flight cancellation token. Long-running work (Whisper,
//! LLM, clipboard injection) runs in `spawn_blocking`; user-facing outcomes
//! are delivered as `transcription-*` events, while setup-time failures
//! (validation, paths, re-entry) return `CommandError`.

use crate::commands::data_management_cmd::resolve_child;
use crate::commands::delivery_controller::{InjectError, WIN32_FOCUS_OPS};
use crate::commands::pipeline_state::PipelineState;
use crate::error::CommandError;
use crate::speech::Segment;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::Manager;
use tracing::{error, info, warn};

/// Shared state for the transcribe window: take-once target HWND and the
/// in-flight transcription cancel token.
pub struct PendingTranscribe {
    /// Foreground HWND captured when the transcribe window opens. Taken by
    /// `inject_transcript_text` (success or failure) and cleared on window
    /// close — a second inject without reopening finds None and errors.
    hwnd: Mutex<Option<isize>>,
    /// Cancel token for the in-flight transcription; replaced per run.
    cancel_token: Mutex<Option<Arc<AtomicBool>>>,
}

impl PendingTranscribe {
    pub fn new() -> Self {
        Self {
            hwnd: Mutex::new(None),
            cancel_token: Mutex::new(None),
        }
    }
}

impl Default for PendingTranscribe {
    fn default() -> Self {
        Self::new()
    }
}

/// Response payload for `get_recording_segments`.
#[derive(Serialize)]
pub struct RecordingSegments {
    segments: Vec<Segment>,
    duration_ms: u32,
    transcription_status: String,
    dropped_blocks: u64,
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Open the transcribe window: reject while a transcription is in flight,
/// capture the current foreground HWND (take-once semantics), then show.
#[tauri::command]
pub fn open_transcribe_window(
    app: tauri::AppHandle,
    pt: tauri::State<'_, PendingTranscribe>,
) -> Result<(), CommandError> {
    open_window_impl(&app, &pt)
}

/// Start a transcription for the given recording. Returns immediately after
/// validation; progress and results arrive via `transcription-*` events.
#[tauri::command]
pub async fn transcribe_recording(
    pt: tauri::State<'_, PendingTranscribe>,
    ps: tauri::State<'_, PipelineState>,
    filename: String,
    use_llm: bool,
) -> Result<(), CommandError> {
    let base = recordings_base_dir(&ps)?;
    let wav_path = resolve_child(&base, &filename, "wav")?;
    let json_path = resolve_child(&base, &filename, "json")?;
    // Read + validate the WAV header before claiming the in-flight slot.
    let (samples, _duration_ms) = read_wav_samples(&wav_path)?;

    let token = begin_transcription(&pt)?;
    let ps_owned = ps.inner().clone();
    let token_for_task = token.clone();
    let join = tokio::task::spawn_blocking(move || {
        run_transcription(
            &ps_owned,
            &json_path,
            &filename,
            &samples,
            use_llm,
            token_for_task,
        );
    })
    .await;
    end_transcription(&pt);
    join.map_err(|e| CommandError::new("INTERNAL", format!("transcription task failed: {e}")))
}

/// Cancel the in-flight transcription. Errors when nothing is running.
#[tauri::command]
pub fn cancel_transcription(pt: tauri::State<'_, PendingTranscribe>) -> Result<(), CommandError> {
    let Some(guard) = crate::util::lock_mutex(&pt.cancel_token, "pt_cancel_token") else {
        return Err(CommandError::lock("cancel token lock poisoned"));
    };
    match guard.as_ref() {
        Some(token) => {
            token.store(true, Ordering::SeqCst);
            Ok(())
        }
        None => Err(CommandError::state("没有进行中的转录任务")),
    }
}

/// Return the stored segments and metadata for a recording.
#[tauri::command]
pub fn get_recording_segments(
    ps: tauri::State<'_, PipelineState>,
    filename: String,
) -> Result<RecordingSegments, CommandError> {
    let base = recordings_base_dir(&ps)?;
    let json_path = resolve_child(&base, &filename, "json")?;
    let content = std::fs::read_to_string(&json_path)
        .map_err(|e| CommandError::io(e, "failed to read recording metadata"))?;
    let metadata: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| CommandError::io(e, "failed to parse recording metadata"))?;
    Ok(segments_from_metadata(&metadata))
}

/// Inject text into the window that was foreground when the transcribe
/// window opened. Fallback: leave the text in the clipboard with a warning
/// when the target window is gone or cannot be focused. After the delivery
/// attempt (success or clipboard fallback), the recording JSON `final_text`
/// is updated with the injected text — write failure is logged, not fatal,
/// because the delivery already happened and cannot be rolled back.
#[tauri::command]
pub async fn inject_transcript_text(
    pt: tauri::State<'_, PendingTranscribe>,
    ps: tauri::State<'_, PipelineState>,
    filename: String,
    text: String,
) -> Result<(), CommandError> {
    let trimmed = text.trim().to_string();
    if trimmed.is_empty() {
        return Err(CommandError::validation("注入文本不能为空"));
    }
    // Validate the filename BEFORE consuming the take-once HWND.
    let base = recordings_base_dir(&ps)?;
    let json_path = resolve_child(&base, &filename, "json")?;
    let hwnd = crate::util::lock_mutex(&pt.hwnd, "pt_hwnd").and_then(|mut g| g.take());
    let Some(hwnd) = hwnd else {
        return Err(CommandError::state(
            "目标窗口句柄已失效，请重新打开转录窗口",
        ));
    };
    let ps_owned = ps.inner().clone();
    let text_for_task = trimmed.clone();
    let inject_result = ps_owned
        .delivery
        .inject_to_hwnd(hwnd, &text_for_task, &WIN32_FOCUS_OPS)
        .await
        .map_err(inject_error_to_command);
    if let Err(e) = write_final_text(&json_path, &trimmed) {
        warn!("transcribe inject: final_text write failed for {filename}: {e}");
    }
    inject_result
}

/// Persist the injected text as `final_text` in the recording JSON,
/// preserving the existing transcription / llm_corrected fields.
fn write_final_text(json_path: &Path, text: &str) -> Result<(), CommandError> {
    let content = std::fs::read_to_string(json_path)
        .map_err(|e| CommandError::io(e, "failed to read recording metadata"))?;
    let metadata: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| CommandError::io(e, "failed to parse recording metadata"))?;
    let transcription = metadata
        .get("transcription")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let llm_corrected = metadata.get("llm_corrected").and_then(|v| v.as_str());
    crate::data_saving::update_json_with_text(json_path, transcription, llm_corrected, Some(text))
        .map_err(CommandError::from)
}

// ---------------------------------------------------------------------------
// Window lifecycle helpers
// ---------------------------------------------------------------------------

/// Shared open logic for the command and the tray menu entry.
pub(crate) fn open_window_impl<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    pt: &PendingTranscribe,
) -> Result<(), CommandError> {
    // Reject re-entry while a transcription is in flight.
    if let Some(guard) = crate::util::lock_mutex(&pt.cancel_token, "pt_cancel_token") {
        if guard.is_some() {
            return Err(CommandError::state("转录进行中，请等待完成或取消"));
        }
    }
    // Capture the target HWND BEFORE showing our window (which would steal
    // the foreground). Overwrites any stale handle from a previous session.
    let hwnd = crate::win32::get_foreground_hwnd();
    if let Some(mut guard) = crate::util::lock_mutex(&pt.hwnd, "pt_hwnd") {
        *guard = Some(hwnd);
    }
    let win = app
        .get_webview_window("transcribe")
        .ok_or_else(|| CommandError::state("转录窗口未初始化"))?;
    win.show()
        .map_err(|e| CommandError::state(format!("显示转录窗口失败: {e}")))?;
    let _ = win.set_focus();
    Ok(())
}

/// Window closed: cancel any in-flight transcription, clear the HWND so
/// later injects are rejected until the window is reopened.
pub(crate) fn on_transcribe_window_closed(pt: &PendingTranscribe) {
    if let Some(mut guard) = crate::util::lock_mutex(&pt.hwnd, "pt_hwnd") {
        *guard = None;
    }
    if let Some(mut guard) = crate::util::lock_mutex(&pt.cancel_token, "pt_cancel_token") {
        if let Some(token) = guard.take() {
            token.store(true, Ordering::SeqCst);
        }
    }
}

// ---------------------------------------------------------------------------
// Transcription task
// ---------------------------------------------------------------------------

/// Claim the in-flight slot, returning the fresh cancel token.
fn begin_transcription(pt: &PendingTranscribe) -> Result<Arc<AtomicBool>, CommandError> {
    let Some(mut guard) = crate::util::lock_mutex(&pt.cancel_token, "pt_cancel_token") else {
        return Err(CommandError::lock("cancel token lock poisoned"));
    };
    if guard.is_some() {
        return Err(CommandError::state("已有转录任务进行中，请等待完成或取消"));
    }
    let token = Arc::new(AtomicBool::new(false));
    *guard = Some(token.clone());
    Ok(token)
}

/// Release the in-flight slot (called after the task finishes for any reason).
fn end_transcription(pt: &PendingTranscribe) {
    if let Some(mut guard) = crate::util::lock_mutex(&pt.cancel_token, "pt_cancel_token") {
        guard.take();
    }
}

/// The blocking transcription body. All user-facing outcomes are emitted as
/// events; this function never returns an error to the command layer.
fn run_transcription(
    ps: &PipelineState,
    json_path: &Path,
    filename: &str,
    samples: &[f32],
    use_llm: bool,
    cancel: Arc<AtomicBool>,
) {
    info!("transcription_requested: {filename} use_llm={use_llm}");
    let emitter = ps.emitter.clone();
    let progress: Box<dyn Fn(u8) + Send + Sync> = Box::new(move |p: u8| {
        emitter.emit(
            "transcription-progress",
            serde_json::json!({"percent": p, "stage": "whisper"}),
        );
    });

    let result = ps
        .engine
        .transcribe_with_segments_sync(samples, cancel.clone(), progress);

    if cancel.load(Ordering::Relaxed) {
        info!("transcription_cancelled: {filename}");
        ps.emitter.emit(
            "transcription-cancelled",
            serde_json::json!({"filename": filename}),
        );
        return;
    }

    let segments = match result {
        Ok(s) => s,
        Err(e) => {
            error!("transcription failed for {filename}: {e}");
            ps.emitter.emit(
                "transcription-error",
                serde_json::json!({"message": "转录失败，请查看日志"}),
            );
            return;
        }
    };

    let transcription: String = segments.iter().map(|s| s.text.as_str()).collect();
    let llm_corrected = if use_llm {
        match run_llm_correction(ps, &transcription) {
            Some(Ok(corrected)) => Some(corrected),
            Some(Err(e)) => {
                warn!("transcribe: LLM correction failed for {filename}: {e}");
                ps.emitter.emit(
                    "transcription-error",
                    serde_json::json!({"message": "LLM 纠错失败，已保留原始转录"}),
                );
                None
            }
            None => None,
        }
    } else {
        None
    };

    if let Err(e) = crate::data_saving::update_json_with_segments(
        json_path,
        &segments,
        &transcription,
        llm_corrected.as_deref(),
    ) {
        error!("transcribe: failed to write results for {filename}: {e}");
        ps.emitter.emit(
            "transcription-error",
            serde_json::json!({"message": "转录结果写入失败"}),
        );
        return;
    }
    ps.emitter.emit(
        "transcription-done",
        serde_json::json!({"filename": filename}),
    );
}

/// Run LLM correction on the merged transcription.
/// `None` = LLM not attempted (disabled or incomplete config);
/// `Some(Err)` = attempted and failed (caller keeps the raw transcription).
fn run_llm_correction(
    ps: &PipelineState,
    text: &str,
) -> Option<Result<String, crate::error::AppError>> {
    let cfg = ps.config_cache.read_cached();
    if !cfg.llm_enabled
        || cfg.llm_api_url.is_empty()
        || cfg.llm_api_key.is_empty()
        || cfg.llm_model.is_empty()
    {
        return None;
    }
    ps.emitter.emit(
        "transcription-progress",
        serde_json::json!({"percent": 100, "stage": "llm"}),
    );
    // Prefer the cached corrector when it matches the current config (tests
    // inject a mock there); otherwise build a fresh client. The LLM API key
    // stays in-process — never logged, never sent to the frontend.
    if let Some(guard) = crate::util::lock_mutex(&ps.cached_llm, "cached_llm") {
        if let Some(corrector) = guard.as_ref() {
            if corrector.matches_config(&cfg.llm_api_url, &cfg.llm_api_key, &cfg.llm_model) {
                return Some(corrector.correct_sync(text));
            }
        }
    }
    let client = crate::llm::LLMClient::new(
        cfg.llm_api_url.clone(),
        cfg.llm_api_key.clone(),
        cfg.llm_model.clone(),
    );
    Some(client.correct_sync(text))
}

// ---------------------------------------------------------------------------
// Injection
// ---------------------------------------------------------------------------

/// Map the detached-delivery domain error to command-layer `CommandError`.
/// The two Chinese messages are a byte-identical frontend contract
/// (toast display) — guarded by test_inject_error_messages_byte_identical.
fn inject_error_to_command(e: InjectError) -> CommandError {
    match e {
        InjectError::WindowGone => {
            CommandError::state("目标窗口已关闭，文本已复制到剪贴板，请尽快粘贴并覆盖")
        }
        InjectError::FocusFailed => {
            CommandError::state("无法聚焦目标窗口，文本已复制到剪贴板，请尽快粘贴并覆盖")
        }
        InjectError::Clipboard(e) => CommandError::from(e),
    }
}

// ---------------------------------------------------------------------------
// WAV + path helpers
// ---------------------------------------------------------------------------

/// Base directory for recordings (config data_saving_path).
fn recordings_base_dir(ps: &PipelineState) -> Result<PathBuf, CommandError> {
    let cfg = ps.config_cache.read_cached();
    if cfg.data_saving_path.trim().is_empty() {
        return Err(CommandError::validation("未设置数据保存路径"));
    }
    Ok(PathBuf::from(&cfg.data_saving_path))
}

/// Read a 16kHz mono 16-bit PCM WAV into f32 samples, validating the header.
/// Returns (samples, duration_ms).
fn read_wav_samples(path: &Path) -> Result<(Vec<f32>, u32), CommandError> {
    let bytes = std::fs::read(path).map_err(|e| CommandError::io(e, "failed to read recording"))?;
    if bytes.len() < 44 {
        return Err(CommandError::validation("录音文件损坏（头部不完整）"));
    }
    let u16_at = |off: usize| u16::from_le_bytes([bytes[off], bytes[off + 1]]);
    let u32_at = |off: usize| {
        u32::from_le_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]])
    };
    if &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(CommandError::validation("录音文件损坏（非 WAV 格式）"));
    }
    if u16_at(20) != 1 || u16_at(22) != 1 || u32_at(24) != 16_000 || u16_at(34) != 16 {
        return Err(CommandError::validation(
            "录音格式不支持（需要 16kHz 单声道 16-bit PCM）",
        ));
    }
    if &bytes[36..40] != b"data" {
        return Err(CommandError::validation("录音文件损坏（缺少 data 块）"));
    }
    let payload = &bytes[44..];
    if payload.len() % 2 != 0 {
        return Err(CommandError::validation("录音文件损坏（数据长度为奇数）"));
    }
    let samples: Vec<f32> = payload
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
        .collect();
    let duration_ms = (samples.len() as u64 * 1000 / 16_000).min(u32::MAX as u64) as u32;
    Ok((samples, duration_ms))
}

/// Lenient metadata → RecordingSegments mapping: missing fields get
/// defaults (classic recordings lack segments/status/dropped_blocks).
fn segments_from_metadata(metadata: &serde_json::Value) -> RecordingSegments {
    let segments: Vec<Segment> = metadata
        .get("segments")
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    let duration_ms = metadata
        .get("duration_seconds")
        .and_then(|v| v.as_f64())
        .map(|s| (s * 1000.0).round().min(u32::MAX as f64) as u32)
        .unwrap_or(0);
    let transcription_status = metadata
        .get("transcription_status")
        .and_then(|v| v.as_str())
        .unwrap_or("pending")
        .to_string();
    let dropped_blocks = metadata
        .get("dropped_blocks")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    RecordingSegments {
        segments,
        duration_ms,
        transcription_status,
        dropped_blocks,
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

    fn build_ps(
        config: AppConfig,
        corrector: Option<Box<dyn crate::llm::TextCorrector>>,
    ) -> (PipelineState, Arc<MockEmitter>, Arc<MockClipboard>) {
        let emitter = Arc::new(MockEmitter::new());
        let clipboard = Arc::new(MockClipboard::new());
        let ps = PipelineState::new(
            Arc::new(Mutex::new(StateMachine::new())),
            Arc::new(Mutex::new(MockAudioCapture::new())),
            Arc::new(MockEngine::new("测试转录文本")),
            clipboard.clone(),
            Arc::new(PerfHistory::new()),
            ConfigCache::new(config),
            Arc::new(Mutex::new(corrector)),
            Arc::new(Mutex::new(None)),
            Arc::new(NoopWindowController),
            emitter.clone() as Arc<dyn EventEmitter>,
            Arc::new(MockReviewProvider::new()),
        );
        (ps, emitter, clipboard)
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dl-vt-transcribe-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(std::fs::create_dir_all(&dir).is_ok());
        dir
    }

    fn event_names(emitter: &MockEmitter) -> Vec<String> {
        emitter.take_events().into_iter().map(|(e, _)| e).collect()
    }

    // --- WAV reading ---

    #[test]
    fn test_read_wav_samples_valid() {
        let dir = temp_dir("wav-valid");
        let rec = crate::streaming_recorder::StreamingRecorder::start(&dir, 16_000);
        assert!(rec.is_ok());
        let Some(mut rec) = rec.ok() else { return };
        rec.push_samples(&vec![0.5f32; 16000]);
        let info = rec.finalize();
        assert!(info.is_ok());
        let Some(info) = info.ok() else { return };

        let read = read_wav_samples(&info.wav_path);
        assert!(read.is_ok());
        let Ok((samples, duration_ms)) = read else {
            return;
        };
        assert_eq!(samples.len(), 16000);
        assert_eq!(duration_ms, 1000);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_read_wav_samples_rejects_bad_magic() {
        let dir = temp_dir("wav-magic");
        let path = dir.join("bad.wav");
        assert!(std::fs::write(&path, [0u8; 100]).is_ok());
        assert!(read_wav_samples(&path).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_read_wav_samples_rejects_too_small() {
        let dir = temp_dir("wav-small");
        let path = dir.join("small.wav");
        assert!(std::fs::write(&path, [0u8; 10]).is_ok());
        assert!(read_wav_samples(&path).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_read_wav_samples_rejects_wrong_rate() {
        let dir = temp_dir("wav-rate");
        let path = dir.join("rate.wav");
        let mut bytes = vec![0u8; 44];
        bytes[0..4].copy_from_slice(b"RIFF");
        bytes[8..12].copy_from_slice(b"WAVE");
        bytes[20..22].copy_from_slice(&1u16.to_le_bytes()); // PCM
        bytes[22..24].copy_from_slice(&1u16.to_le_bytes()); // mono
        bytes[24..28].copy_from_slice(&8_000u32.to_le_bytes()); // wrong rate
        bytes[34..36].copy_from_slice(&16u16.to_le_bytes());
        bytes[36..40].copy_from_slice(b"data");
        assert!(std::fs::write(&path, &bytes).is_ok());
        assert!(read_wav_samples(&path).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- Cancel token lifecycle ---

    #[test]
    fn test_begin_transcription_rejects_reentry() {
        let pt = PendingTranscribe::new();
        let first = begin_transcription(&pt);
        assert!(first.is_ok());
        let second = begin_transcription(&pt);
        assert!(second.is_err());
        end_transcription(&pt);
        let third = begin_transcription(&pt);
        assert!(third.is_ok());
    }

    #[test]
    fn test_cancel_token_set_on_window_close() {
        let pt = PendingTranscribe::new();
        let token = begin_transcription(&pt);
        assert!(token.is_ok());
        let Some(token) = token.ok() else { return };
        assert!(!token.load(Ordering::Relaxed));
        on_transcribe_window_closed(&pt);
        assert!(token.load(Ordering::Relaxed));
        // After close, cancel reports nothing in flight.
        assert!(
            crate::util::lock_mutex(&pt.cancel_token, "t")
                .map(|g| g.is_none())
                .unwrap_or(false)
        );
    }

    #[test]
    fn test_window_close_clears_hwnd() {
        let pt = PendingTranscribe::new();
        if let Some(mut g) = crate::util::lock_mutex(&pt.hwnd, "t") {
            *g = Some(12345);
        }
        on_transcribe_window_closed(&pt);
        let cleared = crate::util::lock_mutex(&pt.hwnd, "t")
            .map(|g| g.is_none())
            .unwrap_or(false);
        assert!(cleared);
    }

    // --- run_transcription ---

    fn write_pending_json(dir: &Path, stem: &str) -> PathBuf {
        let json_path = dir.join(format!("{stem}.json"));
        let metadata = serde_json::json!({
            "transcription": serde_json::Value::Null,
            "llm_corrected": serde_json::Value::Null,
            "segments": [],
            "transcription_status": "pending",
            "source": "record_only",
            "dropped_blocks": 0,
        });
        assert!(crate::data_saving::atomic_write_json(&json_path, &metadata).is_ok());
        json_path
    }

    #[test]
    fn test_run_transcription_happy_path() {
        let dir = temp_dir("run-happy");
        let (ps, emitter, _) = build_ps(AppConfig::default(), None);
        let json_path = write_pending_json(&dir, "2026-08-18_10-00-00");
        let samples = vec![0.0f32; 16000];

        run_transcription(
            &ps,
            &json_path,
            "2026-08-18_10-00-00",
            &samples,
            false,
            Arc::new(AtomicBool::new(false)),
        );

        let events = event_names(&emitter);
        assert!(events.iter().any(|e| e == "transcription-done"));
        assert!(!events.iter().any(|e| e == "transcription-error"));

        let content = std::fs::read_to_string(&json_path);
        assert!(content.is_ok());
        let Ok(content) = content else { return };
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap_or_default();
        assert_eq!(parsed["transcription_status"], "done");
        assert_eq!(parsed["transcription"], "测试转录文本");
        assert!(parsed["segments"].is_array());
        assert_eq!(parsed["segments"].as_array().map(|a| a.len()), Some(1));
        assert!(parsed["llm_corrected"].is_null());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_run_transcription_cancelled_keeps_pending() {
        let dir = temp_dir("run-cancel");
        let (ps, emitter, _) = build_ps(AppConfig::default(), None);
        let json_path = write_pending_json(&dir, "2026-08-18_10-00-01");
        let samples = vec![0.0f32; 16000];
        // Pre-set cancel token: engine aborts immediately.
        let cancel = Arc::new(AtomicBool::new(true));

        run_transcription(
            &ps,
            &json_path,
            "2026-08-18_10-00-01",
            &samples,
            false,
            cancel,
        );

        let events = event_names(&emitter);
        assert!(events.iter().any(|e| e == "transcription-cancelled"));
        assert!(!events.iter().any(|e| e == "transcription-done"));
        // JSON untouched: status stays pending, no segments written.
        let content = std::fs::read_to_string(&json_path);
        assert!(content.is_ok());
        let Ok(content) = content else { return };
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap_or_default();
        assert_eq!(parsed["transcription_status"], "pending");
        assert!(parsed["transcription"].is_null());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_run_transcription_with_llm_correction() {
        let dir = temp_dir("run-llm");
        let cfg = AppConfig {
            llm_enabled: true,
            llm_api_url: "http://llm.local".to_string(),
            llm_api_key: "k".to_string(),
            llm_model: "m".to_string(),
            ..Default::default()
        };
        let corrector =
            MockCorrector::new("纠正后的文本").with_config("http://llm.local", "k", "m");
        let (ps, emitter, _) = build_ps(cfg, Some(Box::new(corrector)));
        let json_path = write_pending_json(&dir, "2026-08-18_10-00-02");
        let samples = vec![0.0f32; 16000];

        run_transcription(
            &ps,
            &json_path,
            "2026-08-18_10-00-02",
            &samples,
            true,
            Arc::new(AtomicBool::new(false)),
        );

        let events = event_names(&emitter);
        assert!(events.iter().any(|e| e == "transcription-done"));
        let content = std::fs::read_to_string(&json_path);
        assert!(content.is_ok());
        let Ok(content) = content else { return };
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap_or_default();
        assert_eq!(parsed["llm_corrected"], "纠正后的文本");
        assert_eq!(parsed["transcription_status"], "done");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_run_transcription_zero_segments() {
        let dir = temp_dir("run-zero-seg");
        let emitter = Arc::new(MockEmitter::new());
        let clipboard = Arc::new(MockClipboard::new());
        let ps = PipelineState::new(
            Arc::new(Mutex::new(StateMachine::new())),
            Arc::new(Mutex::new(MockAudioCapture::new())),
            // Whisper produced zero segments (all filtered / silence).
            Arc::new(MockEngine::new("").with_segments(vec![])),
            clipboard,
            Arc::new(PerfHistory::new()),
            ConfigCache::new(AppConfig::default()),
            Arc::new(Mutex::new(None)),
            Arc::new(Mutex::new(None)),
            Arc::new(NoopWindowController),
            emitter.clone() as Arc<dyn EventEmitter>,
            Arc::new(MockReviewProvider::new()),
        );
        let json_path = write_pending_json(&dir, "2026-08-18_10-00-03");
        let samples = vec![0.0f32; 16000];

        run_transcription(
            &ps,
            &json_path,
            "2026-08-18_10-00-03",
            &samples,
            false,
            Arc::new(AtomicBool::new(false)),
        );

        // Zero segments is not an error: done, empty segments, empty text.
        let events = event_names(&emitter);
        assert!(events.iter().any(|e| e == "transcription-done"));
        assert!(!events.iter().any(|e| e == "transcription-error"));
        let content = std::fs::read_to_string(&json_path);
        assert!(content.is_ok());
        let Ok(content) = content else { return };
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap_or_default();
        assert_eq!(parsed["transcription_status"], "done");
        assert_eq!(parsed["transcription"], "");
        assert_eq!(parsed["segments"].as_array().map(|a| a.len()), Some(0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// LLM regression: a 5xx from the LLM endpoint must keep the raw
    /// whisper transcription (status=done, llm_corrected=null) and surface
    /// a user-friendly error event.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_run_transcription_llm_http_5xx_keeps_raw() {
        use std::io::{Read, Write};
        let listener = match std::net::TcpListener::bind("127.0.0.1:0") {
            Ok(l) => l,
            Err(_) => return, // sandbox without network: skip gracefully
        };
        let addr = match listener.local_addr() {
            Ok(a) => a,
            Err(_) => return,
        };
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 8192];
                let _ = stream.read(&mut buf);
                let resp = "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            }
        });

        let dir = temp_dir("run-llm-5xx");
        let cfg = AppConfig {
            llm_enabled: true,
            llm_api_url: format!("http://{addr}/v1/chat/completions"),
            llm_api_key: "k".to_string(),
            llm_model: "m".to_string(),
            ..Default::default()
        };
        // No cached corrector → run_llm_correction builds a fresh LLMClient
        // that hits the 5xx mock server.
        let (ps, emitter, _) = build_ps(cfg, None);
        let json_path = write_pending_json(&dir, "2026-08-18_10-00-04");
        let samples = vec![0.0f32; 16000];

        run_transcription(
            &ps,
            &json_path,
            "2026-08-18_10-00-04",
            &samples,
            true,
            Arc::new(AtomicBool::new(false)),
        );

        let events = emitter.take_events();
        let names: Vec<&str> = events.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"transcription-done"));
        let llm_err = events.iter().find(|(n, _)| n == "transcription-error");
        assert!(llm_err.is_some());
        if let Some((_, payload)) = llm_err {
            assert_eq!(payload["message"], "LLM 纠错失败，已保留原始转录");
        }

        let content = std::fs::read_to_string(&json_path);
        assert!(content.is_ok());
        let Ok(content) = content else { return };
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap_or_default();
        assert_eq!(parsed["transcription_status"], "done");
        assert_eq!(parsed["transcription"], "测试转录文本");
        assert!(parsed["llm_corrected"].is_null());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- segments_from_metadata ---

    #[test]
    fn test_segments_from_metadata_defaults_for_classic() {
        let metadata = serde_json::json!({
            "duration_seconds": 2.5,
            "transcription": "你好",
        });
        let out = segments_from_metadata(&metadata);
        assert!(out.segments.is_empty());
        assert_eq!(out.duration_ms, 2500);
        assert_eq!(out.transcription_status, "pending");
        assert_eq!(out.dropped_blocks, 0);
    }

    #[test]
    fn test_segments_from_metadata_full() {
        let metadata = serde_json::json!({
            "duration_seconds": 1.0,
            "segments": [{"text": "你好", "start_ms": 0, "end_ms": 1000}],
            "transcription_status": "done",
            "dropped_blocks": 3,
        });
        let out = segments_from_metadata(&metadata);
        assert_eq!(out.segments.len(), 1);
        assert_eq!(out.segments[0].text, "你好");
        assert_eq!(out.transcription_status, "done");
        assert_eq!(out.dropped_blocks, 3);
    }

    // --- path traversal ---

    #[test]
    fn test_resolve_child_rejects_traversal() {
        let dir = temp_dir("traversal");
        let result = resolve_child(&dir, "../../etc/passwd", "wav");
        assert!(result.is_err());
        let result = resolve_child(&dir, "not-a-stem", "wav");
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- write_final_text ---

    #[test]
    fn test_write_final_text_preserves_transcription() {
        let dir = temp_dir("final-text");
        let json_path = write_pending_json(&dir, "2026-08-18_11-00-00");
        // Simulate a completed transcription.
        let segments = [crate::speech::Segment {
            text: "原始转录".to_string(),
            start_ms: 0,
            end_ms: 1000,
        }];
        assert!(
            crate::data_saving::update_json_with_segments(
                &json_path,
                &segments,
                "原始转录",
                Some("LLM纠正"),
            )
            .is_ok()
        );

        let result = write_final_text(&json_path, "编辑后注入文本");
        assert!(result.is_ok());

        let content = std::fs::read_to_string(&json_path);
        assert!(content.is_ok());
        let Ok(content) = content else { return };
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap_or_default();
        assert_eq!(parsed["final_text"], "编辑后注入文本");
        assert_eq!(parsed["transcription"], "原始转录");
        assert_eq!(parsed["llm_corrected"], "LLM纠正");
        assert_eq!(parsed["transcription_status"], "done");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_write_final_text_missing_json_errors() {
        let dir = temp_dir("final-text-missing");
        let json_path = dir.join("2026-08-18_11-00-01.json");
        assert!(write_final_text(&json_path, "x").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    use crate::commands::delivery_controller::{FocusOps, InjectError};

    // --- inject error mapping (byte-identical frontend contract) ---

    #[test]
    fn test_inject_error_messages_byte_identical() {
        assert_eq!(
            inject_error_to_command(InjectError::WindowGone).message,
            "目标窗口已关闭，文本已复制到剪贴板，请尽快粘贴并覆盖"
        );
        assert_eq!(
            inject_error_to_command(InjectError::FocusFailed).message,
            "无法聚焦目标窗口，文本已复制到剪贴板，请尽快粘贴并覆盖"
        );
        // Clipboard variant: only `.code` asserted — the message comes from
        // the unchanged legacy From<AppError> conversion, not a fixed contract.
        assert_eq!(
            inject_error_to_command(InjectError::Clipboard(crate::error::AppError::Clipboard(
                "x".to_string()
            )))
            .code,
            CommandError::from(crate::error::AppError::Clipboard("x".to_string())).code
        );
    }

    /// Mock E2E chain: record-only press → push audio → release (cleanup DAG)
    /// → on-demand transcribe → inject with a dead target window (clipboard
    /// fallback) → final_text persisted.
    const DEAD_WINDOW_OPS: FocusOps = FocusOps {
        is_valid: |_| false,
        focus: |_| false,
    };

    #[tokio::test]
    async fn test_record_only_to_inject_chain() {
        let dir = temp_dir("e2e-chain");
        let cfg = AppConfig {
            record_only_enabled: true,
            data_saving_path: dir.to_string_lossy().to_string(),
            ..Default::default()
        };
        let (ps, emitter, clipboard) = build_ps(cfg, None);

        // 1. Record: press → 1s of audio → cleanup DAG (release path).
        // MockAudioCapture runs at 48kHz; the recorder resamples to 16kHz.
        crate::commands::record_only_session::on_press(&ps);
        assert_eq!(ps.sm_state(), Some(crate::state::StateTag::RecordOnly));
        // Deliver 1s of audio through the real cpal callback path.
        if let Some(mut ac) = crate::util::lock_mutex(&ps.ac, "audio_capture") {
            ac.deliver(&vec![0.4f32; 48_000]);
        }
        crate::commands::record_only_session::recover_session(&ps);
        assert_eq!(ps.sm_state(), Some(crate::state::StateTag::Idle));

        let stem = emitter
            .take_events()
            .into_iter()
            .find(|(e, _)| e == "record-only-finished")
            .and_then(|(_, p)| p["stem"].as_str().map(str::to_string));
        assert!(stem.is_some());
        let Some(stem) = stem else { return };
        let json_path = dir.join(format!("{stem}.json"));
        let read_json = |path: &Path| -> serde_json::Value {
            std::fs::read_to_string(path)
                .ok()
                .and_then(|c| serde_json::from_str(&c).ok())
                .unwrap_or_default()
        };
        assert_eq!(read_json(&json_path)["transcription_status"], "pending");

        // 2. Transcribe on demand.
        let wav_path = dir.join(format!("{stem}.wav"));
        let read = read_wav_samples(&wav_path);
        assert!(read.is_ok());
        let Ok((samples, _)) = read else { return };
        // 48kHz → 16kHz resampling: ~1s of audio (block-granularity tolerance).
        assert!(samples.len() > 16000 - 2048 && samples.len() <= 16000 + 1024);
        run_transcription(
            &ps,
            &json_path,
            &stem,
            &samples,
            false,
            Arc::new(AtomicBool::new(false)),
        );
        let after = read_json(&json_path);
        assert_eq!(after["transcription_status"], "done");
        assert_eq!(after["transcription"], "测试转录文本");

        // 3. Inject with a dead target window: clipboard fallback holds the text.
        let result = ps
            .delivery
            .inject_to_hwnd(0, "测试转录文本", &DEAD_WINDOW_OPS)
            .await;
        assert!(result.is_err());
        assert_eq!(clipboard.set_texts(), vec!["测试转录文本".to_string()]);
        assert!(clipboard.injected().is_empty());

        // 4. The delivered text is persisted as final_text.
        assert!(write_final_text(&json_path, "测试转录文本").is_ok());
        assert_eq!(read_json(&json_path)["final_text"], "测试转录文本");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
