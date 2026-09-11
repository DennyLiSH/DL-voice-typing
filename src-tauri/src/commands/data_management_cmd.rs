//! Data management commands for the saved training data feature.
//!
//! Provides listing, deletion (with 5s undo window via `PendingDeletes`),
//! disk usage stats, and audio read access for the WAV + JSON pairs written
//! by `crate::data_saving`.

use crate::config::ConfigCache;
use crate::error::CommandError;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::ipc::Response;
use tracing::{info, warn};

/// Pattern: `YYYY-MM-DD_HH-MM-SS` (19 chars total). Validated without regex
/// to avoid pulling in the regex crate for one fixed pattern.
const FILENAME_LEN: usize = 19;

/// One recording entry returned to the frontend.
#[derive(Serialize)]
pub struct RecordingEntry {
    /// Filename stem without extension, e.g. `2026-06-24_14-30-25`.
    pub filename: String,
    /// RFC3339 timestamp from JSON, or `None` if missing.
    pub timestamp: Option<String>,
    pub language: Option<String>,
    pub whisper_model: Option<String>,
    pub duration_seconds: Option<f32>,
    pub transcription: Option<String>,
    pub llm_corrected: Option<String>,
    pub final_text: Option<String>,
    /// "classic" | "record_only"; defaults to "classic" when JSON predates the field.
    pub source: String,
    /// "pending" | "done" | "failed"; `None` for classic recordings.
    pub transcription_status: Option<String>,
    /// Number of audio blocks dropped by backpressure (record-only only).
    pub dropped_blocks: u64,
    /// 0 indicates the `.wav` file is missing.
    pub wav_size: u64,
    pub json_size: u64,
}

/// Paginated list response.
#[derive(Serialize)]
pub struct RecordingListResponse {
    pub items: Vec<RecordingEntry>,
    /// Total matching count (before pagination).
    pub total: u32,
    /// Sum of WAV + JSON sizes for matching entries (bytes).
    pub total_bytes: u64,
    pub offset: u32,
    pub limit: u32,
    /// Whether `data_saving_path` is non-empty. Distinct from "path exists with files".
    pub path_configured: bool,
}

#[derive(Serialize)]
pub struct DataUsage {
    pub total_bytes: u64,
    pub recording_count: u32,
}

#[derive(Serialize)]
pub struct FailedDelete {
    pub filename: String,
    pub error: String,
}

// ---------------------------------------------------------------------------
// Soft-delete + 5s undo (P2 用户控制权专项)
// ---------------------------------------------------------------------------

/// Directory under `data_saving_path` where pending (soft-deleted, undoable)
/// files live until the 5-second window expires or the user clicks Undo.
pub(crate) const PENDING_DIR: &str = ".dl_pending";

/// How long the undo window stays open (seconds). User-facing copy is
/// "5 秒内可撤销" — keep these two numbers aligned.
pub(crate) const UNDO_WINDOW_SECS: u64 = 5;

/// Result of a soft-delete batch. `id` is the undo handle returned to the
/// frontend; `moved` counts successful stems (录音条数), not files.
#[derive(Serialize)]
pub struct SoftDeleteBatch {
    pub id: u64,
    pub moved: u32,
    pub failed: Vec<FailedDelete>,
    /// Undo window length in seconds — single source for the frontend
    /// toast countdown (the confirm-copy number flows through the frontend
    /// mirror constant + contract test instead).
    pub undo_window_secs: u64,
}

/// Per-batch undo state. One entry per soft-delete batch.
pub(crate) struct PendingEntry {
    /// `(orig_path, pending_path)` pairs — wav + json for each stem.
    /// All paths were validated through `is_valid_stem` + `resolve_child`
    /// before insertion, so they are guaranteed to live under the user's
    /// `data_saving_path`.
    pub(crate) pairs: Vec<(PathBuf, PathBuf)>,
    /// Fires (from the finalize path ONLY, on whichever thread finalizes)
    /// when this batch is permanently deleted. A restore consumes the
    /// entry first and never fires it — a restored batch must not be
    /// reported as permanently deleted.
    pub(crate) on_finalize: Option<Arc<dyn Fn(u64) + Send + Sync>>,
}

/// Process-wide registry of pending soft-delete batches. The 5-second
/// timer is a detached `std::thread` per batch (no `tokio::time` feature
/// dependency) — the thread holds an `Arc<Self>` clone and uses
/// `take_entry` to ensure the timer vs. restore paths are serialized
/// through the inner mutex. `take-once` means the entry is consumed
/// exactly once, by whichever path fires first.
pub struct PendingDeletes {
    inner: Mutex<HashMap<u64, PendingEntry>>,
    next_id: AtomicU64,
}

impl Default for PendingDeletes {
    fn default() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        }
    }
}

/// Report returned from `restore_with_id`. Serialized to the frontend so
/// the undo toast can tell the user when a restore was PARTIAL (failed > 0
/// means some pairs could not be moved back and stay in pending until the
/// startup sweep removes them).
#[derive(Debug, Clone, Copy, Serialize)]
pub struct RestoreReport {
    pub restored: u32,
    pub failed: u32,
}

impl PendingDeletes {
    /// Schedule a soft-delete batch: insert the entry, spawn a detached
    /// `std::thread` that sleeps `UNDO_WINDOW_SECS` and then finalizes.
    /// The returned id is the undo handle.
    ///
    /// `on_finalize` fires (from whichever thread finalizes) only when the
    /// finalize path actually consumed the entry — restore wins the take-once
    /// race and never fires it.
    pub(crate) fn schedule_with_finalize(
        self: &Arc<Self>,
        pairs: Vec<(PathBuf, PathBuf)>,
        on_finalize: Option<Arc<dyn Fn(u64) + Send + Sync>>,
    ) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        match crate::util::lock_mutex(&self.inner, "PendingDeletes::schedule") {
            Some(mut guard) => {
                guard.insert(
                    id,
                    PendingEntry {
                        pairs: pairs.clone(),
                        on_finalize: on_finalize.clone(),
                    },
                );
            }
            // Lock poisoned: the entry is NOT registered, so the undo toast's
            // 撤销 click will surface an unknown-id error and the pending
            // files are left for the startup sweep. Log loudly rather than
            // silently returning an unusable undo handle.
            None => {
                warn!(
                    target: "data",
                    id,
                    files = pairs.len(),
                    "PendingDeletes lock poisoned; batch registered WITHOUT undo (sweep will clean up)"
                );
            }
        }
        // Spawn the timer thread (detach — process exit kills it, which is
        // fine: startup sweep handles residuals).
        let pd = Arc::clone(self);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(UNDO_WINDOW_SECS));
            pd.finalize_by_id(id);
        });
        id
    }

    /// Take-once entry extraction. Used by both `restore_pending_delete`
    /// (the Undo path) and the timer thread (the finalize path). Whichever
    /// caller wins the race gets the entry; the loser sees `None` and
    /// does nothing.
    pub(crate) fn take_entry(&self, id: u64) -> Option<PendingEntry> {
        crate::util::lock_mutex(&self.inner, "PendingDeletes::take_entry")
            .and_then(|mut guard| guard.remove(&id))
    }

    /// Test-only / internal finalize: take-once remove the entry; if we
    /// got it, remove the pending files (best-effort), fire the
    /// `on_finalize` callback if registered, and return `true`. Returns
    /// `false` when the entry was already consumed by restore (the
    /// take-once invariant). The unlock-before-IO pattern keeps the lock
    /// window microseconds; subsequent restores can race through
    /// `take_entry` returning `None`.
    pub(crate) fn finalize_by_id(&self, id: u64) -> bool {
        let Some(entry) = self.take_entry(id) else {
            return false;
        };
        for (_orig, pending) in &entry.pairs {
            if let Err(e) = std::fs::remove_file(pending) {
                warn!(
                    target: "data",
                    id,
                    path = %pending.display(),
                    "pending finalize remove failed (startup sweep will retry): {e}"
                );
            }
        }
        info!(target: "data", id, files = entry.pairs.len(), "pending finalize");
        if let Some(cb) = &entry.on_finalize {
            cb(id);
        }
        true
    }

    /// Test-only entry check (used by `schedule_stores_entry_and_timer_runs_finalize`).
    #[cfg(test)]
    pub(crate) fn has_entry(&self, id: u64) -> bool {
        crate::util::lock_mutex(&self.inner, "PendingDeletes::has_entry")
            .map(|guard| guard.contains_key(&id))
            .unwrap_or(false)
    }

    /// Restore helper used by the command path and tests. Returns
    /// `Err(VALIDATION)` when the id is unknown (already restored or
    /// window expired). On success the entry is consumed (take-once).
    pub(crate) fn restore_with_id(&self, id: u64) -> Result<RestoreReport, CommandError> {
        let Some(entry) = self.take_entry(id) else {
            return Err(CommandError::validation("撤销机会已过期或不存在"));
        };
        let mut restored = 0u32;
        let mut failed = 0u32;
        for (orig, pending_path) in &entry.pairs {
            match std::fs::rename(pending_path, orig) {
                Ok(()) => restored += 1,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // Source already gone (e.g. user yanked it manually).
                    // Iteration 2 reversal (deviation #14): leave it gone —
                    // we do NOT finalize the rest of the batch. The failed
                    // pair stays missing; the next startup sweep sees no
                    // orphan, and successful pairs stay restored.
                    failed += 1;
                }
                Err(_e) => {
                    failed += 1;
                }
            }
        }
        info!(
            target: "data",
            id, restored, failed, "undo restore"
        );
        Ok(RestoreReport { restored, failed })
    }
}

/// Outcome of a soft-delete batch: fully transitioned stem count, per-file
/// failures, and the `(orig, pending)` pairs to register for undo.
pub(crate) struct SoftDeleteOutcome {
    pub(crate) moved: u32,
    pub(crate) failed: Vec<FailedDelete>,
    pub(crate) pairs: Vec<(PathBuf, PathBuf)>,
}

/// Move WAV+JSON pairs for each stem in `filenames` into `data_saving_path/.dl_pending/`.
/// Per-stem atomicity: a failed rename
/// in the middle of a pair is rolled back so the stem ends up either fully
/// moved or fully not-moved.
///
/// **Defense layers (M4):**
/// 1. `is_valid_stem` per filename (reject path separators, length, format).
/// 2. `resolve_child` (canonicalize + starts_with) for orig path.
/// 3. After `create_dir_all(pending)`, canonicalize pending and assert it
///    still starts with `base_canonical` — `.dl_pending` swapped for a
///    junction (mklink /J, no admin) is rejected wholesale.
pub(crate) fn soft_delete_files(base: &Path, filenames: &[String]) -> SoftDeleteOutcome {
    let pending = base.join(PENDING_DIR);
    let base_canon = match base.canonicalize() {
        Ok(b) => b,
        Err(e) => {
            // The command path pre-canonicalizes via resolve_base_existing,
            // so this is normally unreachable there — but direct callers
            // (tests, future code) deserve an honest per-file failure rather
            // than a silent (0 moved, 0 failed) result that reads as success.
            return SoftDeleteOutcome {
                moved: 0,
                failed: vec![FailedDelete {
                    filename: "<data dir>".to_string(),
                    error: format!("cannot canonicalize data dir: {e}"),
                }],
                pairs: Vec::new(),
            };
        }
    };
    let mut moved: u32 = 0;
    let mut failed: Vec<FailedDelete> = Vec::new();
    let mut pairs: Vec<(PathBuf, PathBuf)> = Vec::new();

    for filename in filenames {
        if !is_valid_stem(filename) {
            failed.push(FailedDelete {
                filename: filename.clone(),
                error: "文件名不合法".to_string(),
            });
            continue;
        }
        // Create + canonicalize the pending dir once per stem. The
        // `starts_with(base_canon)` check rejects `.dl_pending` swapped
        // for a junction (mklink /J). Per-ext canonicalize removed (the
        // rename is the only file-system call that follows; a concurrent
        // attacker swapping the dir mid-loop would only move the
        // already-renamed file into an attacker-chosen target, which
        // gives no privilege over the source — both sides are user-
        // owned).
        if let Err(e) = std::fs::create_dir_all(&pending)
            .and_then(|_| ensure_inside(&base_canon, &pending))
            .map(|_| ())
        {
            failed.push(FailedDelete {
                filename: filename.clone(),
                error: friendly_io_error(&e),
            });
            continue;
        }
        let mut ok = true;
        let mut pair_moved: Vec<(PathBuf, PathBuf)> = Vec::new();
        for ext in ["wav", "json"] {
            // `resolve_child` already canonicalizes the child and verifies
            // it stays under `base`; we then re-check against the canonical
            // base (matters on Windows where canonical paths use the `\\?\`
            // UNC prefix while the caller passes a plain path).
            let orig = match resolve_child(&base_canon, filename, ext) {
                Ok(p) => p,
                Err(e) => {
                    failed.push(FailedDelete {
                        filename: filename.clone(),
                        error: friendly_delete_error(&e),
                    });
                    ok = false;
                    break;
                }
            };
            if !orig.exists() {
                // Target state already reached (file was already moved or
                // never existed). Skip without counting — `moved` is the
                // number of successfully *transitioned* stems, not files.
                continue;
            }
            let dest = pending.join(format!("{filename}.{ext}"));
            let move_res = std::fs::rename(&orig, &dest);
            if let Err(e) = move_res {
                failed.push(FailedDelete {
                    filename: filename.clone(),
                    error: friendly_io_error(&e),
                });
                // Roll back any files of this pair we already moved.
                for (o, p) in &pair_moved {
                    let _ = std::fs::rename(p, o);
                }
                ok = false;
                break;
            }
            pair_moved.push((orig, dest));
        }
        if ok {
            moved += 1;
            pairs.extend(pair_moved);
        }
    }
    SoftDeleteOutcome {
        moved,
        failed,
        pairs,
    }
}

/// Startup sweep: any files left in `.dl_pending/` are from a crashed or
/// killed process — the undo window is gone. Per M4, defense layers are:
///
/// 1. `data_saving_path` non-empty AND canonicalize OK.
/// 2. `pending` canonicalize + `starts_with(base)` — reject junction.
/// 3. Per-entry stem + ext validation — skip ill-named files so a planted
///    file in `.dl_pending/` cannot trigger arbitrary deletion.
///
/// **User-data destruction logging**: every removed file is listed in the
/// `info!` log so the sweep is fully traceable.
pub fn sweep_pending_dir(config: &crate::config::schema::AppConfig) {
    if config.data_saving_path.is_empty() {
        return;
    }
    let base = match Path::new(&config.data_saving_path).canonicalize() {
        Ok(b) => b,
        Err(e) => {
            warn!(target: "data", "sweep aborted: cannot canonicalize data_saving_path: {e}");
            return;
        }
    };
    let pending = base.join(PENDING_DIR);
    if !pending.exists() {
        return;
    }
    let pc = match ensure_inside(&base, &pending) {
        Ok(p) => p,
        Err(e) => {
            warn!(target: "data", "sweep aborted: pending dir check failed: {e}");
            return;
        }
    };
    let read_dir = match std::fs::read_dir(&pc) {
        Ok(rd) => rd,
        Err(e) => {
            warn!(target: "data", "sweep aborted: cannot read pending dir: {e}");
            return;
        }
    };
    let mut removed: Vec<String> = Vec::new();
    for entry in read_dir.flatten() {
        let path = entry.path();
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        if !is_valid_stem(stem) {
            continue;
        }
        if ext != "wav" && ext != "json" {
            continue;
        }
        if std::fs::remove_file(&path).is_ok() {
            removed.push(path.to_string_lossy().into_owned());
        }
    }
    if !removed.is_empty() {
        info!(
            target: "data",
            files = ?removed,
            count = removed.len(),
            "pending sweep"
        );
    }
}

// ---------------------------------------------------------------------------
// Filename validation + path resolution
// ---------------------------------------------------------------------------

/// Serialize a typed metadata enum field back to its on-disk string form
/// (language / whisper_model are enums at rest as strings). The helper
/// deliberately routes through `to_value` + `as_str` to lock the string
/// shape contract, so a future change to the enum's `Serialize` form
/// cannot silently alter the IPC payload. A missing field (`None`)
/// short-circuits as legitimately absent; non-string shapes are logged
/// (schema drift must be observable, not silent).
fn enum_field<T: serde::Serialize>(v: &Option<T>) -> Option<String> {
    let v = v.as_ref()?;
    let value = serde_json::to_value(v).ok()?;
    match value.as_str() {
        Some(s) => Some(s.to_string()),
        None => {
            tracing::debug!("recording meta enum field serialized to non-string shape");
            None
        }
    }
}

/// Canonicalize `candidate` and verify it still lives under `base`.
/// **Contract: `base` must ALREADY be canonicalized** — on Windows,
/// canonicalize() yields `\\?\` verbatim prefixes and Path::starts_with
/// compares by component, so a verbatim child never starts_with a
/// plain-form base. Rejects directory swaps (junction/symlink) that would
/// let a resolved path escape the data dir. Single sentinel for the two
/// former inline variants (soft_delete_files pending check,
/// sweep_pending_dir). Escapes surface as `io::ErrorKind::Other`; a
/// missing candidate surfaces as the canonicalize error itself (usually
/// `NotFound`) so callers can distinguish "not there" from "escaped".
pub(crate) fn ensure_inside(base: &Path, candidate: &Path) -> Result<PathBuf, std::io::Error> {
    let canon = candidate.canonicalize()?;
    if !canon.starts_with(base) {
        return Err(std::io::Error::other(format!(
            "{} escapes base (junction?)",
            candidate.display()
        )));
    }
    Ok(canon)
}

/// Validate that `filename` is a `YYYY-MM-DD_HH-MM-SS` stem and contains no
/// path separators or other escape characters. Combined with canonicalize-based
/// checks at resolve time, this is defense-in-depth against path traversal.
pub(crate) fn is_valid_stem(filename: &str) -> bool {
    if filename.len() != FILENAME_LEN {
        return false;
    }
    let bytes = filename.as_bytes();
    let is_dash_at = |i: usize| bytes[i] == b'-';
    let is_under_at = |i: usize| bytes[i] == b'_';
    let is_digit_at = |i: usize| bytes[i].is_ascii_digit();

    // Layout: YYYY-MM-DD_HH-MM-SS
    // Indices: 0-3 digits, 4 dash, 5-6 digits, 7 dash, 8-9 digits,
    //          10 underscore, 11-12 digits, 13 dash, 14-15 digits,
    //          16 dash, 17-18 digits.
    [
        is_digit_at(0),
        is_digit_at(1),
        is_digit_at(2),
        is_digit_at(3),
        is_dash_at(4),
        is_digit_at(5),
        is_digit_at(6),
        is_dash_at(7),
        is_digit_at(8),
        is_digit_at(9),
        is_under_at(10),
        is_digit_at(11),
        is_digit_at(12),
        is_dash_at(13),
        is_digit_at(14),
        is_digit_at(15),
        is_dash_at(16),
        is_digit_at(17),
        is_digit_at(18),
    ]
    .iter()
    .all(|&ok| ok)
}

/// Resolve `data_saving_path` and return the canonicalized base directory.
/// Returns Ok(None) when path is empty (treated as "feature not configured").
/// Returns Ok(Some(dir)) when the path exists and is canonicalizable.
/// Returns Err when canonicalize fails.
pub(crate) fn resolve_base(path_str: &str) -> Result<Option<PathBuf>, CommandError> {
    if path_str.is_empty() {
        return Ok(None);
    }
    let raw = PathBuf::from(path_str);
    if !raw.exists() {
        return Ok(None);
    }
    match raw.canonicalize() {
        Ok(p) => Ok(Some(p)),
        Err(e) => Err(CommandError::new(
            "IO",
            format!("failed to canonicalize data_saving_path: {e}"),
        )),
    }
}

/// Build the full path for a recording file (.wav or .json) under `base`,
/// verifying the canonicalized result is still inside `base`.
pub(crate) fn resolve_child(
    base: &Path,
    filename: &str,
    ext: &str,
) -> Result<PathBuf, CommandError> {
    if !is_valid_stem(filename) {
        return Err(CommandError::validation(format!(
            "invalid recording filename: {filename}"
        )));
    }
    let child = base.join(format!("{filename}.{ext}"));
    match child.canonicalize() {
        Ok(canon) => {
            if !canon.starts_with(base) {
                return Err(CommandError::validation("path escapes data_saving_path"));
            }
            Ok(canon)
        }
        Err(_) => {
            // File does not exist (canonicalize fails on missing paths).
            // Caller decides whether that is an error.
            Ok(child)
        }
    }
}

// ---------------------------------------------------------------------------
// List command
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn list_saved_recordings(
    config_cache: tauri::State<'_, ConfigCache>,
    offset: u32,
    limit: u32,
    query: Option<String>,
) -> Result<RecordingListResponse, CommandError> {
    if limit == 0 {
        return Err(CommandError::validation("limit must be > 0"));
    }

    let config = config_cache.read_cached();
    let path_str = config.data_saving_path.clone();
    let query_lower = query
        .map(|q| q.trim().to_lowercase())
        .filter(|q| !q.is_empty());

    // Snapshot the inputs we need inside spawn_blocking (config Arc is Send).
    let config_path = path_str.clone();
    let result =
        tokio::task::spawn_blocking(move || -> Result<RecordingListResponse, CommandError> {
            let base = match resolve_base(&config_path)? {
                Some(p) => p,
                None => {
                    return Ok(RecordingListResponse {
                        items: Vec::new(),
                        total: 0,
                        total_bytes: 0,
                        offset,
                        limit,
                        path_configured: !config_path.is_empty(),
                    });
                }
            };

            let entries = scan_and_collect(&base, query_lower.as_deref())?;
            let total = entries.len() as u32;
            let total_bytes = entries.iter().map(|e| e.wav_size + e.json_size).sum();

            let offset_us = offset as usize;
            let limit_us = limit as usize;
            let paged: Vec<RecordingEntry> = if offset_us >= entries.len() {
                Vec::new()
            } else {
                entries[offset_us..]
                    .iter()
                    .take(limit_us)
                    .cloned()
                    .collect()
            };

            Ok(RecordingListResponse {
                items: paged,
                total,
                total_bytes,
                offset,
                limit,
                path_configured: true,
            })
        })
        .await;

    match result {
        Ok(inner) => inner,
        Err(join_err) => Err(CommandError::new(
            "TASK",
            format!("list task panicked: {join_err}"),
        )),
    }
}

/// Scan `base`, parse each matching `.json` (silently skipping corrupt ones),
/// filter by query, sort newest-first by timestamp (stem fallback), and return.
pub(crate) fn scan_and_collect(
    base: &Path,
    query_lower: Option<&str>,
) -> Result<Vec<RecordingEntry>, CommandError> {
    let read_dir = match std::fs::read_dir(base) {
        Ok(rd) => rd,
        Err(e) => {
            // Treat as empty directory on NotFound / PermissionDenied.
            if e.kind() == std::io::ErrorKind::NotFound
                || e.kind() == std::io::ErrorKind::PermissionDenied
            {
                return Ok(Vec::new());
            }
            return Err(CommandError::new(
                "IO",
                format!("failed to read data_saving_path: {e}"),
            ));
        }
    };

    let mut entries: Vec<RecordingEntry> = Vec::new();
    for entry in read_dir.flatten() {
        let path = entry.path();
        let Some(ext) = path.extension() else {
            continue;
        };
        if ext != "json" {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if !is_valid_stem(stem) {
            continue;
        }

        let json_size = entry.metadata().map(|m| m.len()).unwrap_or(0);

        // Parse JSON; skip on error (single corrupt file must not break the list).
        let meta = match crate::data_saving::load_metadata(&path) {
            Ok(m) => m,
            Err(e) => {
                warn!(filename = stem, error = %e, "skip unreadable recording json");
                continue;
            }
        };

        // Optional text query filter (case-insensitive substring across 3 fields).
        if let Some(q) = query_lower {
            let matches = meta
                .transcription
                .as_deref()
                .map(|t| t.to_lowercase().contains(q))
                .unwrap_or(false)
                || meta
                    .llm_corrected
                    .as_deref()
                    .map(|t| t.to_lowercase().contains(q))
                    .unwrap_or(false)
                || meta
                    .final_text
                    .as_deref()
                    .map(|t| t.to_lowercase().contains(q))
                    .unwrap_or(false);
            if !matches {
                continue;
            }
        }

        // WAV size (0 if missing). Do not fail the scan when WAV is unreadable.
        let wav_path = base.join(format!("{stem}.wav"));
        let wav_size = std::fs::metadata(&wav_path).map(|m| m.len()).unwrap_or(0);

        entries.push(RecordingEntry {
            filename: stem.to_string(),
            timestamp: meta.timestamp,
            language: enum_field(&meta.language),
            whisper_model: enum_field(&meta.whisper_model),
            duration_seconds: meta.duration_seconds.map(|d| d as f32),
            transcription: meta.transcription,
            llm_corrected: meta.llm_corrected,
            final_text: meta.final_text,
            source: meta.source.unwrap_or_else(|| "classic".to_string()),
            transcription_status: meta.transcription_status,
            dropped_blocks: meta.dropped_blocks,
            wav_size,
            json_size,
        });
    }

    // Sort newest first. RFC3339 strings sort lexicographically by time.
    // For entries without a real timestamp, fall back to filename stem
    // (which is itself time-ordered at second granularity).
    entries.sort_by(|a, b| {
        let ka = a.timestamp.as_deref().unwrap_or(&a.filename);
        let kb = b.timestamp.as_deref().unwrap_or(&b.filename);
        kb.cmp(ka)
    });

    Ok(entries)
}

// ---------------------------------------------------------------------------
// Soft-delete + restore commands (replaces the legacy hard-delete commands
// `delete_recording` / `delete_recordings`; see plan §用户控制权专项 commit 2).
// ---------------------------------------------------------------------------

/// Soft-delete: move each stem's WAV+JSON into `.dl_pending/` and schedule
/// the 5-second undo window. The frontend receives the batch id and shows
/// the undo toast. Process exit / crash = undo window lost; startup sweep
/// (`sweep_pending_dir`) handles residual files.
#[tauri::command]
pub async fn soft_delete_recordings(
    config_cache: tauri::State<'_, ConfigCache>,
    pending: tauri::State<'_, Arc<PendingDeletes>>,
    pipeline: tauri::State<'_, crate::commands::pipeline_state::PipelineState>,
    filenames: Vec<String>,
) -> Result<SoftDeleteBatch, CommandError> {
    let config = config_cache.read_cached();
    let path_str = config.data_saving_path.clone();
    // Clone the Arc out of the State reference before crossing into
    // spawn_blocking (State's borrow cannot move into a 'static closure).
    let pending_arc: Arc<PendingDeletes> = pending.inner().clone();
    // Snapshot the emitter in command scope so the spawn_blocking closure
    // captures it by move (pipeline State's borrow cannot cross into a
    // 'static closure). The emitter is an owned Arc<dyn EventEmitter>
    // (Send + Sync), safe to ship into the detached finalize thread via
    // the `on_finalize` closure.
    let emitter = pipeline.emitter();

    let result = tokio::task::spawn_blocking(move || -> Result<SoftDeleteBatch, CommandError> {
        let base = resolve_base_existing(&path_str)?;
        let SoftDeleteOutcome {
            moved,
            failed,
            pairs,
        } = soft_delete_files(&base, &filenames);
        // Only schedule a timer when there is something to undo.
        let id = if !pairs.is_empty() {
            // The finalize thread emits `pending-deletes-finalized` so the
            // toast can render backend truth instead of guessing via its
            // own setInterval (which drifts under throttled background
            // tabs). The callback fires only when finalize actually
            // consumed the entry — restore wins the race and skips.
            let emitter_for_cb = emitter.clone();
            pending_arc.schedule_with_finalize(
                pairs,
                Some(Arc::new(move |batch_id| {
                    emitter_for_cb.emit(
                        "pending-deletes-finalized",
                        serde_json::json!({ "id": batch_id }),
                    );
                })),
            )
        } else {
            0
        };
        info!(
            target: "data",
            id, moved, failed_count = failed.len(),
            "soft_delete_recordings completed"
        );
        Ok(SoftDeleteBatch {
            id,
            moved,
            failed,
            undo_window_secs: UNDO_WINDOW_SECS,
        })
    })
    .await;

    match result {
        Ok(inner) => inner,
        Err(join_err) => Err(CommandError::new(
            "TASK",
            format!("soft delete task panicked: {join_err}"),
        )),
    }
}

/// Restore a soft-delete batch by id. The frontend calls this from the
/// undo-toast click. Returns the full report (restored + failed) so a
/// partial restore is user-visible: failed pairs stay in pending until
/// the startup sweep removes them (deviation #14 — do NOT destroy them
/// eagerly when the restore intent itself failed).
#[tauri::command]
pub async fn restore_pending_delete(
    pending: tauri::State<'_, Arc<PendingDeletes>>,
    id: u64,
) -> Result<RestoreReport, CommandError> {
    pending.inner().restore_with_id(id)
}

#[tauri::command]
pub async fn get_data_usage(
    config_cache: tauri::State<'_, ConfigCache>,
) -> Result<DataUsage, CommandError> {
    let config = config_cache.read_cached();
    let path_str = config.data_saving_path.clone();

    let result = tokio::task::spawn_blocking(move || -> Result<DataUsage, CommandError> {
        let base = match resolve_base(&path_str)? {
            Some(p) => p,
            None => {
                return Ok(DataUsage {
                    total_bytes: 0,
                    recording_count: 0,
                });
            }
        };

        let read_dir = match std::fs::read_dir(&base) {
            Ok(rd) => rd,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound
                    || e.kind() == std::io::ErrorKind::PermissionDenied
                {
                    return Ok(DataUsage {
                        total_bytes: 0,
                        recording_count: 0,
                    });
                }
                return Err(CommandError::new(
                    "IO",
                    format!("failed to read data_saving_path: {e}"),
                ));
            }
        };

        let mut total_bytes: u64 = 0;
        let mut recording_count: u32 = 0;
        for entry in read_dir.flatten() {
            let path = entry.path();
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if !is_valid_stem(stem) {
                continue;
            }
            let Some(ext) = path.extension() else {
                continue;
            };
            // Only count WAV+JSON pairs that match the timestamp pattern.
            // Orphan WAV (no JSON) is excluded — the JSON is the truth source.
            if ext != "wav" && ext != "json" {
                continue;
            }
            if ext == "wav" {
                let json_sibling = base.join(format!("{stem}.json"));
                if !json_sibling.exists() {
                    continue; // orphan WAV, skip
                }
            }
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            total_bytes = total_bytes.saturating_add(size);
            if ext == "json" {
                recording_count = recording_count.saturating_add(1);
            }
        }

        Ok(DataUsage {
            total_bytes,
            recording_count,
        })
    })
    .await;

    match result {
        Ok(inner) => inner,
        Err(join_err) => Err(CommandError::new(
            "TASK",
            format!("usage task panicked: {join_err}"),
        )),
    }
}

// ---------------------------------------------------------------------------
// Audio read command (replaces asset protocol — see plan review decisions)
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn read_recording_audio(
    config_cache: tauri::State<'_, ConfigCache>,
    filename: String,
) -> Result<Response, CommandError> {
    let config = config_cache.read_cached();
    let path_str = config.data_saving_path.clone();

    let result = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, CommandError> {
        let base = resolve_base_existing(&path_str)?;
        let wav = resolve_child(&base, &filename, "wav")?;
        if !wav.exists() {
            return Err(CommandError::new(
                "NOT_FOUND",
                format!("audio file not found: {filename}.wav"),
            ));
        }
        std::fs::read(&wav)
            .map_err(|e| CommandError::new("IO", format!("failed to read {filename}.wav: {e}")))
    })
    .await;

    match result {
        Ok(Ok(bytes)) => Ok(Response::new(bytes)),
        Ok(Err(e)) => Err(e),
        Err(join_err) => Err(CommandError::new(
            "TASK",
            format!("read audio task panicked: {join_err}"),
        )),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve base path and require it to exist (returns NOT_FOUND when missing).
/// Used by commands where an empty result would be misleading (delete, read).
fn resolve_base_existing(path_str: &str) -> Result<PathBuf, CommandError> {
    if path_str.is_empty() {
        return Err(CommandError::validation(
            "data_saving_path is not configured",
        ));
    }
    let raw = PathBuf::from(path_str);
    if !raw.exists() {
        return Err(CommandError::new(
            "NOT_FOUND",
            "data_saving_path does not exist on disk",
        ));
    }
    raw.canonicalize()
        .map_err(|e| CommandError::io(e, "failed to canonicalize data_saving_path"))
}

/// Translate a raw OS io::Error into a user-friendly Chinese message
/// for the `FailedDelete.error` field.
pub(crate) fn friendly_io_error(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::PermissionDenied => "文件被占用或无权限".to_string(),
        std::io::ErrorKind::NotFound => "文件不存在".to_string(),
        std::io::ErrorKind::Other if e.to_string().contains("used by another process") => {
            "文件被其他程序占用".to_string()
        }
        _ => format!("删除失败：{e}"),
    }
}

/// Friendly message for non-IO errors during delete (e.g. validation).
pub(crate) fn friendly_delete_error(e: &CommandError) -> String {
    match e.code.as_str() {
        "VALIDATION" => "文件名不合法".to_string(),
        _ => e.message.clone(),
    }
}

impl Clone for RecordingEntry {
    fn clone(&self) -> Self {
        Self {
            filename: self.filename.clone(),
            timestamp: self.timestamp.clone(),
            language: self.language.clone(),
            whisper_model: self.whisper_model.clone(),
            duration_seconds: self.duration_seconds,
            transcription: self.transcription.clone(),
            llm_corrected: self.llm_corrected.clone(),
            final_text: self.final_text.clone(),
            source: self.source.clone(),
            transcription_status: self.transcription_status.clone(),
            dropped_blocks: self.dropped_blocks,
            wav_size: self.wav_size,
            json_size: self.json_size,
        }
    }
}
