//! Data management commands for the saved training data feature.
//!
//! Provides listing, deletion, disk usage stats, and audio read access
//! for the WAV + JSON pairs written by `crate::data_saving`.

use crate::config::ConfigCache;
use crate::error::CommandError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::ipc::Response;
use tracing::warn;

/// Pattern: `YYYY-MM-DD_HH-MM-SS` (19 chars total). Validated without regex
/// to avoid pulling in the regex crate for one fixed pattern.
const FILENAME_LEN: usize = 19;

/// Metadata subset extracted from each saved JSON file.
/// Missing fields stay `None` — corrupted or partial JSON does not fail the whole list.
#[derive(Deserialize)]
struct RecordingMeta {
    timestamp: Option<String>,
    language: Option<String>,
    whisper_model: Option<String>,
    duration_seconds: Option<f32>,
    transcription: Option<String>,
    llm_corrected: Option<String>,
    final_text: Option<String>,
}

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
pub struct DeleteBatchResult {
    pub deleted: u32,
    pub failed: Vec<FailedDelete>,
}

#[derive(Serialize)]
pub struct FailedDelete {
    pub filename: String,
    pub error: String,
}

// ---------------------------------------------------------------------------
// Filename validation + path resolution
// ---------------------------------------------------------------------------

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
        let meta = match std::fs::read_to_string(&path).and_then(|content| {
            serde_json::from_str::<RecordingMeta>(&content)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
        }) {
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
            language: meta.language,
            whisper_model: meta.whisper_model,
            duration_seconds: meta.duration_seconds,
            transcription: meta.transcription,
            llm_corrected: meta.llm_corrected,
            final_text: meta.final_text,
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
// Delete commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn delete_recording(
    config_cache: tauri::State<'_, ConfigCache>,
    filename: String,
) -> Result<(), CommandError> {
    let config = config_cache.read_cached();
    let path_str = config.data_saving_path.clone();

    let result = tokio::task::spawn_blocking(move || -> Result<(), CommandError> {
        let base = resolve_base_existing(&path_str)?;
        let wav = resolve_child(&base, &filename, "wav")?;
        let json = resolve_child(&base, &filename, "json")?;
        // Best-effort delete of the pair. Missing files are silently OK
        // (target state already reached).
        if wav.exists() {
            if let Err(e) = std::fs::remove_file(&wav) {
                return Err(CommandError::new(
                    "IO",
                    format!("failed to delete {filename}.wav: {e}"),
                ));
            }
        }
        if json.exists() {
            if let Err(e) = std::fs::remove_file(&json) {
                return Err(CommandError::new(
                    "IO",
                    format!("failed to delete {filename}.json: {e}"),
                ));
            }
        }
        tracing::info!(deleted_count = 1, "delete_recording completed");
        Ok(())
    })
    .await;

    match result {
        Ok(inner) => inner,
        Err(join_err) => Err(CommandError::new(
            "TASK",
            format!("delete task panicked: {join_err}"),
        )),
    }
}

#[tauri::command]
pub async fn delete_recordings(
    config_cache: tauri::State<'_, ConfigCache>,
    filenames: Vec<String>,
) -> Result<DeleteBatchResult, CommandError> {
    let config = config_cache.read_cached();
    let path_str = config.data_saving_path.clone();

    let result = tokio::task::spawn_blocking(move || -> Result<DeleteBatchResult, CommandError> {
        let base = resolve_base_existing(&path_str)?;
        let mut deleted: u32 = 0;
        let mut failed: Vec<FailedDelete> = Vec::new();

        for filename in filenames {
            let wav = match resolve_child(&base, &filename, "wav") {
                Ok(p) => p,
                Err(e) => {
                    failed.push(FailedDelete {
                        filename: filename.clone(),
                        error: friendly_delete_error(&e),
                    });
                    continue;
                }
            };
            let json = match resolve_child(&base, &filename, "json") {
                Ok(p) => p,
                Err(e) => {
                    failed.push(FailedDelete {
                        filename: filename.clone(),
                        error: friendly_delete_error(&e),
                    });
                    continue;
                }
            };

            let mut local_err: Option<std::io::Error> = None;
            if wav.exists() {
                if let Err(e) = std::fs::remove_file(&wav) {
                    local_err = Some(e);
                }
            }
            if local_err.is_none() && json.exists() {
                if let Err(e) = std::fs::remove_file(&json) {
                    local_err = Some(e);
                }
            }
            if let Some(e) = local_err {
                failed.push(FailedDelete {
                    filename: filename.clone(),
                    error: friendly_io_error(&e),
                });
                tracing::warn!(filename = %filename, error = %e, "delete failed");
                continue;
            }
            deleted += 1;
        }

        tracing::info!(
            deleted_count = deleted,
            failed_count = failed.len() as u32,
            "delete_recordings completed"
        );
        Ok(DeleteBatchResult { deleted, failed })
    })
    .await;

    match result {
        Ok(inner) => inner,
        Err(join_err) => Err(CommandError::new(
            "TASK",
            format!("delete batch task panicked: {join_err}"),
        )),
    }
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
            wav_size: self.wav_size,
            json_size: self.json_size,
        }
    }
}
