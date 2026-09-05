//! User-side error history: a bounded ring buffer recording every
//! user-facing error event, plus a query command for the settings help page.
//!
//! Rationale (2026-09-05 recoverability audit): all error channels were
//! evaporating (floating window 4.5s, toast 4s, save status 1.5s) — the
//! user's recovery path broke at "what did that error just say?". The
//! history is the persistent counterpart; the tracing log remains the
//! authoritative audit trail (the two are complementary, not redundant).
//!
//! Recording is fail-safe by construction: `record` never panics (timestamp
//! formatting degrades to UTC/empty per the data_saving.rs:240 precedent,
//! mutex access goes through `util::lock_mutex`), and the `RecordingEmitter`
//! passthrough never depends on the record outcome.

use crate::commands::EventEmitter;
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// One recorded user-facing error.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ErrorRecord {
    /// RFC 3339 local time (UTC fallback when the local offset is
    /// indeterminate).
    pub timestamp: String,
    /// Event channel name (e.g. "speech-error").
    pub event: String,
    /// User-facing message extracted from the payload (bare string or
    /// `{message: …}` object), truncated to 500 chars.
    pub message: String,
}

/// Bounded ring buffer of the most recent user-facing errors.
pub struct ErrorHistory {
    buffer: Mutex<VecDeque<ErrorRecord>>,
}

impl ErrorHistory {
    /// Maximum number of records retained (help page shows 3; headroom for
    /// future UIs).
    pub const CAPACITY: usize = 20;
    /// Message truncation bound (mirrors `redact_error_detail`'s 500-char
    /// convention — whisper error strings can be long).
    const MAX_MESSAGE_CHARS: usize = 500;

    pub fn new() -> Self {
        Self {
            buffer: Mutex::new(VecDeque::with_capacity(Self::CAPACITY)),
        }
    }

    /// Append a record. Infallible: timestamp formatting degrades instead of
    /// panicking (data_saving.rs:240 precedent — deliberately NOT the
    /// lib.rs expect-panic variant, which would abort the whole process),
    /// and a poisoned lock is skipped, never propagated.
    pub fn record(&self, event: &str, payload: &serde_json::Value) {
        let timestamp = now_rfc3339_local();
        let message = extract_message(payload);
        let record = ErrorRecord {
            timestamp,
            event: event.to_string(),
            message,
        };
        if let Some(mut buf) = crate::util::lock_mutex(&self.buffer, "error_history") {
            if buf.len() >= Self::CAPACITY {
                buf.pop_front();
            }
            buf.push_back(record);
        }
    }

    /// Return the last `n` records (most recent last). `None` = default 3.
    pub fn recent(&self, n: Option<usize>) -> Vec<ErrorRecord> {
        let n = n.unwrap_or(3);
        match crate::util::lock_mutex(&self.buffer, "error_history") {
            Some(buf) => buf
                .iter()
                .rev()
                .take(n)
                .cloned()
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect(),
            None => Vec::new(),
        }
    }
}

impl Default for ErrorHistory {
    fn default() -> Self {
        Self::new()
    }
}

fn now_rfc3339_local() -> String {
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    now.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

/// Payload message extraction: bare string payloads first (speech/llm/
/// injection-error), then `{message: "…"}` objects (record-only-error,
/// transcription-error); truncated to MAX_MESSAGE_CHARS (char-boundary safe).
fn extract_message(payload: &serde_json::Value) -> String {
    let raw = payload
        .as_str()
        .or_else(|| payload.get("message").and_then(|m| m.as_str()))
        .unwrap_or("");
    raw.chars().take(ErrorHistory::MAX_MESSAGE_CHARS).collect()
}

/// Events recorded into the history. `hotkey-error` is defensive: its only
/// current path is the config_cmd bypass (recorded directly there), but if
/// that site ever migrates to the trait emitter it picks up automatically —
/// no double-recording today.
const RECORDED_EVENTS: [&str; 6] = [
    "speech-error",
    "llm-error",
    "injection-error",
    "record-only-error",
    "transcription-error",
    "hotkey-error",
];

/// EventEmitter decorator: records user-facing error events into the shared
/// [`ErrorHistory`] and passes EVERY event through to the inner emitter
/// unconditionally. Assembled in `PipelineState::from_app` so config_cmd's
/// rebuild path is covered too.
pub struct RecordingEmitter {
    inner: Arc<dyn EventEmitter>,
    history: Arc<ErrorHistory>,
}

impl RecordingEmitter {
    pub fn new(inner: Arc<dyn EventEmitter>, history: Arc<ErrorHistory>) -> Self {
        Self { inner, history }
    }
}

impl EventEmitter for RecordingEmitter {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        if RECORDED_EVENTS.contains(&event) {
            self.history.record(event, &payload);
        }
        // Passthrough never depends on the record outcome (record is
        // structurally infallible), and a non-recorded event is only
        // forwarded.
        self.inner.emit(event, payload);
    }
}

/// Return the most recent user-facing errors (`n` defaults to 3) for the
/// settings help page. Read-only; the history is in-memory and resets on
/// restart (the tracing log is the persistent audit trail).
#[tauri::command]
pub fn get_last_errors(
    errors: tauri::State<'_, Arc<ErrorHistory>>,
    n: Option<usize>,
) -> Result<Vec<ErrorRecord>, crate::error::CommandError> {
    Ok(errors.recent(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_truncates_to_20_keeping_newest() {
        let history = ErrorHistory::new();
        for i in 0..25 {
            history.record("speech-error", &serde_json::json!(format!("err-{i}")));
        }
        let all = history.recent(Some(25));
        assert_eq!(all.len(), 20);
        assert_eq!(all.first().unwrap().message, "err-5");
        assert_eq!(all.last().unwrap().message, "err-24");
    }

    #[test]
    fn extracts_bare_string_and_object_message_payloads() {
        let history = ErrorHistory::new();
        history.record("llm-error", &serde_json::json!("裸串文案"));
        history.record(
            "record-only-error",
            &serde_json::json!({"message": "对象文案"}),
        );
        history.record("injection-error", &serde_json::Value::Null);
        let all = history.recent(Some(10));
        assert_eq!(all[0].message, "裸串文案");
        assert_eq!(all[1].message, "对象文案");
        assert_eq!(all[2].message, "");
    }

    #[test]
    fn truncates_long_messages_at_500_chars() {
        let history = ErrorHistory::new();
        history.record("speech-error", &serde_json::json!("x".repeat(600)));
        let all = history.recent(Some(1));
        assert_eq!(all[0].message.chars().count(), 500);
    }

    #[test]
    fn recent_defaults_to_3() {
        let history = ErrorHistory::new();
        for i in 0..5 {
            history.record("speech-error", &serde_json::json!(format!("e{i}")));
        }
        let recent = history.recent(None);
        assert_eq!(
            recent
                .iter()
                .map(|r| r.message.as_str())
                .collect::<Vec<_>>(),
            vec!["e2", "e3", "e4"]
        );
        assert_eq!(history.recent(Some(2)).len(), 2);
    }

    /// MockEmitter as inner: error events recorded, non-error events not,
    /// and everything forwarded.
    #[test]
    fn recording_emitter_records_errors_and_forwards_all() {
        let inner = Arc::new(crate::commands::MockEmitter::new());
        let history = Arc::new(ErrorHistory::new());
        let emitter = RecordingEmitter::new(inner.clone(), history.clone());

        emitter.emit("speech-error", serde_json::json!("boom"));
        emitter.emit("llm-error", serde_json::json!("bad"));
        emitter.emit("transcription-done", serde_json::json!({}));
        emitter.emit("perf-metrics", serde_json::json!([]));

        let recorded = history.recent(Some(10));
        assert_eq!(recorded.len(), 2);
        assert_eq!(recorded[0].event, "speech-error");
        assert_eq!(recorded[1].event, "llm-error");

        let forwarded = inner.take_events();
        let names: Vec<&str> = forwarded.iter().map(|(e, _)| e.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "speech-error",
                "llm-error",
                "transcription-done",
                "perf-metrics"
            ]
        );
    }

    /// A poisoned history mutex must never block the passthrough — the
    /// floating window's error display depends on the inner emit.
    #[test]
    fn poisoned_history_mutex_does_not_block_passthrough() {
        let inner = Arc::new(crate::commands::MockEmitter::new());
        let history = Arc::new(ErrorHistory::new());
        // Poison the history's mutex by panicking while holding it.
        let h2 = history.clone();
        let _ = std::thread::spawn(move || {
            let _g = h2.buffer.lock().unwrap();
            panic!("poison");
        })
        .join();

        let emitter = RecordingEmitter::new(inner.clone(), history);
        emitter.emit("speech-error", serde_json::json!("still forwarded"));
        let forwarded = inner.take_events();
        assert_eq!(forwarded.len(), 1);
        assert_eq!(forwarded[0].0, "speech-error");
    }
}
