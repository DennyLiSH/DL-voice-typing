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
    /// Error-message payload bound — see `crate::util::MAX_MESSAGE_CHARS`
    /// for the single definition.
    pub(crate) const MAX_MESSAGE_CHARS: usize = crate::util::MAX_MESSAGE_CHARS;

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
    crate::util::truncate_chars(raw, ErrorHistory::MAX_MESSAGE_CHARS)
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
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

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

    // ---- emit-literal contract (Round 6 P8 Task 3) ----
    //
    // RECORDED_EVENTS and the actual emit sites in `src/**/*.rs` must agree
    // in BOTH directions:
    // 1. every recorded event has ≥1 emit literal somewhere in the tree
    //    (renaming/removing a channel must consciously update the const —
    //    no dead history entry), and
    // 2. every `*-error` emit literal is a recorded channel (a new error
    //    emit cannot silently bypass the user-facing history).
    //
    // The scraper is hand-rolled (no new dependencies): walk
    // CARGO_MANIFEST_DIR/src recursively and recognize two call forms —
    // the dot-prefixed `emit` call (event name = FIRST string literal
    // after the paren; multi-line calls that open the paren and put the
    // name on the next line are the dominant form, so whitespace/newlines/
    // line comments between are skipped) and `emit_to` (event name =
    // SECOND literal; the first is the target label). A call
    // not followed by a string literal contributes nothing: variable event
    // names, trait `fn emit(` definitions (no dot prefix), and doc-comment
    // mentions like `AppHandle.emit()` all fall out naturally. Literals must
    // match the channel-name shape (`[A-Za-z0-9_-]+`) so payload fragments
    // are ignored.
    //
    // Comment discipline: do NOT write the exact dot-prefixed call shape
    // (`emit` preceded by a dot and followed by an open paren) in comments —
    // the frontend event-registry contract scrapes raw Rust text including
    // comments, and a comment-shaped call makes its paren-balancer ingest
    // unrelated literals as event names (2026-11-14 P8 Task 3 rehearsal
    // finding).

    /// Recursively collect `*.rs` files under `dir` (sorted for a stable
    /// scrape order). Unreadable directories/files panic loudly — the
    /// scraper must never silently shrink its view of the tree.
    fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
        let entries = std::fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("emit-literal scraper: read_dir {}: {e}", dir.display()));
        let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
        paths.sort();
        for path in paths {
            if path.is_dir() {
                collect_rs_files(&path, out);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
                out.push(path);
            }
        }
    }

    /// Skip whitespace and `//` line comments; return the position of the
    /// next token. Only line comments occur between emit-call arguments.
    fn skip_ws_and_comments(bytes: &[u8], mut pos: usize) -> usize {
        loop {
            while bytes.get(pos).is_some_and(|b| b.is_ascii_whitespace()) {
                pos += 1;
            }
            if bytes.get(pos) == Some(&b'/') && bytes.get(pos + 1) == Some(&b'/') {
                while bytes.get(pos).is_some_and(|b| *b != b'\n') {
                    pos += 1;
                }
                continue;
            }
            return pos;
        }
    }

    /// Read a `"…"` literal starting at `pos` (which must point at `"`):
    /// returns the content and the position after the closing quote. Escaped
    /// bytes (`\"`, `\\`) skip two; a trailing `\` before EOF is malformed
    /// and returns None. Non-UTF-8 bytes are lossy-replaced — the channel
    /// name shape filter rejects such literals anyway.
    fn read_string_literal(bytes: &[u8], pos: usize) -> Option<(String, usize)> {
        if bytes.get(pos) != Some(&b'"') {
            return None;
        }
        let start = pos + 1;
        let mut i = start;
        while i < bytes.len() {
            match bytes[i] {
                b'"' => {
                    return Some((
                        String::from_utf8_lossy(&bytes[start..i]).into_owned(),
                        i + 1,
                    ));
                }
                b'\\' => i += 2,
                _ => i += 1,
            }
        }
        None
    }

    /// Channel-name shape: non-empty ASCII `[A-Za-z0-9_-]` only. Anything
    /// else (payload fragments, format strings) is not a channel name.
    fn looks_like_event_name(s: &str) -> bool {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    }

    /// Scrape event-name literals out of one file's content (see the block
    /// comment above for the recognized call forms). `.emitter`/`.emitted`
    /// identifier continuations are not calls and contribute nothing.
    fn scrape_emit_literals(content: &str, out: &mut BTreeSet<String>) {
        let bytes = content.as_bytes();
        let mut i = 0;
        while let Some(rel) = content[i..].find(".emit") {
            let pos = i + rel;
            i = pos + ".emit".len();
            // `emit(` → literal 1 is the event name; `emit_to(` → literal
            // 2 (the first is the target label; zero call sites today, the
            // form is plan-mandated).
            let (skip_first, after_paren) = if bytes.get(i) == Some(&b'(') {
                (false, i + 1)
            } else if bytes[i..].starts_with(b"_to(") {
                (true, i + b"_to(".len())
            } else {
                continue;
            };
            let mut p = skip_ws_and_comments(bytes, after_paren);
            let mut event: Option<String> = None;
            if let Some((lit, next)) = read_string_literal(bytes, p) {
                p = skip_ws_and_comments(bytes, next);
                if bytes.get(p) == Some(&b',') {
                    p = skip_ws_and_comments(bytes, p + 1);
                }
                event = if skip_first {
                    read_string_literal(bytes, p).map(|(name, _)| name)
                } else {
                    Some(lit)
                };
            }
            if let Some(name) = event
                && looks_like_event_name(&name)
            {
                out.insert(name);
            }
        }
    }

    #[test]
    fn recorded_events_contract_scrapes_emit_literals() {
        let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        collect_rs_files(&src_dir, &mut files);
        assert!(
            !files.is_empty(),
            "emit-literal scraper collected no .rs files under {}",
            src_dir.display()
        );

        let mut emitted: BTreeSet<String> = BTreeSet::new();
        for path in &files {
            let content = std::fs::read_to_string(path)
                .unwrap_or_else(|e| panic!("emit-literal scraper: read {}: {e}", path.display()));
            scrape_emit_literals(&content, &mut emitted);
        }
        assert!(
            !emitted.is_empty(),
            "emit-literal scraper found zero emit literals in src/**/*.rs — \
             scraper broken or src tree unreadable"
        );

        // Direction 1: every recorded event has a real emit site.
        let missing: Vec<&str> = RECORDED_EVENTS
            .iter()
            .copied()
            .filter(|name| !emitted.contains(*name))
            .collect();
        assert!(
            missing.is_empty(),
            "RECORDED_EVENTS entries with no emit literal in src/**/*.rs \
             (the emit site was removed/renamed or moved to a non-literal \
             form the scraper cannot see — update RECORDED_EVENTS or restore \
             a literal emit): {missing:?}"
        );

        // Direction 2: every `*-error` emit literal is a conscious record
        // decision.
        let unrecorded: Vec<String> = emitted
            .iter()
            .filter(|name| name.ends_with("-error") && !RECORDED_EVENTS.contains(&name.as_str()))
            .cloned()
            .collect();
        assert!(
            unrecorded.is_empty(),
            "`*-error` emit literals missing from RECORDED_EVENTS (add the \
             channel to RECORDED_EVENTS so the history records it, or rename \
             the event): {unrecorded:?}"
        );
    }

    /// Scraper unit checks on synthetic sources: multi-line (dominant) form,
    /// `emit_to` second-literal form, variable event names, trait `fn emit(`
    /// definitions, doc-comment mentions, and identifier continuations.
    #[test]
    fn emit_literal_scraper_recognizes_call_forms() {
        let mut out = BTreeSet::new();
        scrape_emit_literals(
            "emitter.emit(\n        \"alpha-beta\",\n        payload,\n    );",
            &mut out,
        );
        scrape_emit_literals("emitter.emit(\"gamma\", v);", &mut out);
        scrape_emit_literals("app.emit_to(\"main\", \"delta-one\", v);", &mut out);
        scrape_emit_literals(
            "app.emit_to(\n    \"main\",\n    \"epsilon-two\",\n    v,\n);",
            &mut out,
        );
        // Non-call / non-literal shapes contribute nothing.
        scrape_emit_literals("fn emit(&self, event: &str) {}", &mut out);
        scrape_emit_literals("/// Abstracts `AppHandle.emit()` for tests.", &mut out);
        scrape_emit_literals("let e = ps.emitter();", &mut out);
        scrape_emit_literals("self.inner.emit(event, payload);", &mut out);
        scrape_emit_literals("app.emit_to(label, \"var-target\", v);", &mut out);
        // The payload's string literal never reaches the event slot (for
        // the dot-prefixed call only literal 1 is the event name).
        scrape_emit_literals(
            "e.emit(\n    \"zeta\",\n    json!({\"message\": \"x\"}),\n);",
            &mut out,
        );
        let names: Vec<String> = out.iter().cloned().collect();
        assert_eq!(
            names,
            ["alpha-beta", "delta-one", "epsilon-two", "gamma", "zeta"]
        );
    }
}
