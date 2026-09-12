//! Tests for commands::data_management_cmd.
//!
//! Lives in `src/tests/` to keep production source files free of `.unwrap()`.

use crate::commands::data_management_cmd::{
    FailedDelete, PendingDeletes, SoftDeleteOutcome, ensure_inside, friendly_delete_error,
    friendly_io_error, is_valid_stem, resolve_base, resolve_child, scan_and_collect,
    soft_delete_files, sweep_pending_dir,
};
use crate::config::AppConfig;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

fn write_recording(base: &Path, stem: &str, transcription: Option<&str>) {
    fs::create_dir_all(base).expect("mkdir");
    let wav = base.join(format!("{stem}.wav"));
    let json = base.join(format!("{stem}.json"));
    let mut f = fs::File::create(&wav).expect("create wav");
    f.write_all(
        b"RIFF\x24\x00\x00\x00WAVEfmt \x10\x00\x00\x00\x01\x00\x01\x00\x80\xbb\x00\x00\x00\x77\x01\x00\x02\x00\x10\x00data\x00\x00\x00\x00",
    )
    .expect("write wav");
    // Stem format is YYYY-MM-DD_HH-MM-SS — extract both date and time.
    let yyyy = stem.get(0..4).unwrap_or("2026");
    let mo = stem.get(5..7).unwrap_or("01");
    let dd = stem.get(8..10).unwrap_or("01");
    let hh = stem.get(11..13).unwrap_or("00");
    let mm = stem.get(14..16).unwrap_or("00");
    let ss = stem.get(17..19).unwrap_or("00");
    let timestamp = format!("{yyyy}-{mo}-{dd}T{hh}:{mm}:{ss}+08:00");
    let metadata = serde_json::json!({
        "timestamp": timestamp,
        "language": "zh",
        "whisper_model": "base",
        "duration_seconds": 1.5_f32,
        "transcription": transcription.unwrap_or(""),
        "llm_corrected": serde_json::Value::Null,
        "final_text": serde_json::Value::Null,
    });
    let body = serde_json::to_string(&metadata).expect("serialize");
    fs::write(&json, body).expect("write json");
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dl-voice-typing-test-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("mkdir temp");
    dir
}

fn cleanup(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn test_is_valid_stem_accepts_well_formed() {
    assert!(is_valid_stem("2026-06-24_14-30-25"));
}

#[test]
fn test_is_valid_stem_rejects_path_traversal() {
    assert!(!is_valid_stem(".."));
    assert!(!is_valid_stem("../etc/passwd"));
    assert!(!is_valid_stem("2026-06-24_14-30-25.exe"));
    assert!(!is_valid_stem(""));
    assert!(!is_valid_stem("2026-06-24_14-30-25 "));
}

#[test]
fn test_is_valid_stem_rejects_separator_chars() {
    assert!(!is_valid_stem("2026/06/24_14-30-25"));
    assert!(!is_valid_stem("2026-06-24 14-30-25"));
    assert!(!is_valid_stem("2026-06-24\\14-30-25"));
}

#[test]
fn test_is_valid_stem_rejects_wrong_length() {
    assert!(!is_valid_stem("2026-06-24"));
    assert!(!is_valid_stem("2026-06-24_14-30-25-extra"));
}

#[test]
fn test_resolve_base_empty_returns_none() {
    assert!(resolve_base("").unwrap().is_none());
}

#[test]
fn test_resolve_base_missing_returns_none() {
    let dir = std::env::temp_dir().join("dl-voice-typing-nonexistent-xyz-abc");
    let _ = fs::remove_dir_all(&dir);
    assert!(
        resolve_base(dir.to_string_lossy().as_ref())
            .unwrap()
            .is_none()
    );
}

#[test]
fn test_resolve_base_existing_canonicalizes() {
    let dir = temp_dir("resolve-base");
    let resolved = resolve_base(dir.to_string_lossy().as_ref())
        .unwrap()
        .expect("some base");
    assert!(resolved.is_absolute());
    cleanup(&dir);
}

#[test]
fn test_resolve_child_rejects_invalid_stem() {
    let base = PathBuf::from("/tmp");
    let err = resolve_child(&base, "..", "wav").expect_err("should reject");
    assert_eq!(err.code, "VALIDATION");
}

#[test]
fn test_scan_and_collect_skips_corrupt_json() {
    let dir = temp_dir("scan-corrupt");
    write_recording(&dir, "2026-06-24_14-30-25", Some("hello"));

    let bad = dir.join("2026-06-24_14-31-00.json");
    fs::write(&bad, "{not valid json").unwrap();

    let entries = scan_and_collect(&dir, None).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].filename, "2026-06-24_14-30-25");

    cleanup(&dir);
}

#[test]
fn test_scan_and_collect_handles_missing_wav() {
    let dir = temp_dir("scan-missing-wav");
    let json = serde_json::json!({
        "timestamp": "2026-06-24T14:30:25+08:00",
        "language": "zh",
        "transcription": "你好",
    });
    let body = serde_json::to_string(&json).unwrap();
    fs::write(dir.join("2026-06-24_14-30-25.json"), &body).unwrap();

    let entries = scan_and_collect(&dir, None).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].wav_size, 0);
    assert_eq!(entries[0].json_size, body.len() as u64);

    cleanup(&dir);
}

#[test]
fn test_scan_and_collect_ignores_orphan_wav() {
    let dir = temp_dir("scan-orphan-wav");
    fs::write(dir.join("2026-06-24_14-30-25.wav"), b"fake wav").unwrap();

    let entries = scan_and_collect(&dir, None).unwrap();
    assert_eq!(entries.len(), 0);

    cleanup(&dir);
}

#[test]
fn test_scan_and_collect_text_search() {
    let dir = temp_dir("scan-search");
    write_recording(&dir, "2026-06-24_14-30-25", Some("今天去开会"));
    write_recording(&dir, "2026-06-24_14-31-00", Some("明天去超市"));

    let all = scan_and_collect(&dir, None).unwrap();
    assert_eq!(all.len(), 2);

    let filtered = scan_and_collect(&dir, Some("开会")).unwrap();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].filename, "2026-06-24_14-30-25");

    cleanup(&dir);
}

#[test]
fn test_scan_and_collect_case_insensitive() {
    let dir = temp_dir("scan-case");
    write_recording(&dir, "2026-06-24_14-30-25", Some("Hello World"));
    write_recording(&dir, "2026-06-24_14-31-00", Some("Bye bye"));

    // Lowercase query matches mixed-case text.
    let filtered = scan_and_collect(&dir, Some("hello")).unwrap();
    assert_eq!(filtered.len(), 1);

    cleanup(&dir);
}

#[test]
fn test_scan_and_collect_sorts_newest_first() {
    let dir = temp_dir("scan-sort");
    write_recording(&dir, "2026-06-24_10-00-00", Some("old"));
    write_recording(&dir, "2026-06-25_15-30-00", Some("new"));
    write_recording(&dir, "2026-06-24_20-00-00", Some("mid"));

    let entries = scan_and_collect(&dir, None).unwrap();
    assert_eq!(entries[0].filename, "2026-06-25_15-30-00");
    assert_eq!(entries[1].filename, "2026-06-24_20-00-00");
    assert_eq!(entries[2].filename, "2026-06-24_10-00-00");

    cleanup(&dir);
}

#[test]
fn test_scan_and_collect_nonexistent_dir_returns_empty() {
    let dir = std::env::temp_dir().join("dl-voice-typing-test-no-such-dir");
    let _ = fs::remove_dir_all(&dir);
    let entries = scan_and_collect(&dir, None).unwrap();
    assert!(entries.is_empty());
}

#[test]
fn test_scan_and_collect_empty_query_returns_all() {
    let dir = temp_dir("scan-empty-query");
    write_recording(&dir, "2026-06-24_14-30-25", Some("alpha"));
    write_recording(&dir, "2026-06-24_14-31-00", Some("beta"));

    // `None` and `Some("")` both match all (empty substring is in everything).
    // `Some("   ")` is a non-empty literal search — only the command layer
    // trims and normalizes whitespace-only queries (out of scope here).
    let entries_none = scan_and_collect(&dir, None).unwrap();
    let entries_empty = scan_and_collect(&dir, Some("")).unwrap();
    assert_eq!(entries_none.len(), 2);
    assert_eq!(entries_empty.len(), 2);

    let entries_ws = scan_and_collect(&dir, Some("   ")).unwrap();
    assert_eq!(entries_ws.len(), 0); // literal 3-space substring matches nothing

    cleanup(&dir);
}

#[test]
fn test_resolve_child_no_traversal_for_valid_stem() {
    let dir = temp_dir("valid-stem");
    let base = dir.canonicalize().unwrap();
    let wav = resolve_child(&base, "2026-06-24_14-30-25", "wav").unwrap();
    assert!(wav.starts_with(&base));

    cleanup(&dir);
}

#[test]
fn test_resolve_child_missing_child_returns_plain_join() {
    // Despite the historical name (this fn never tested the
    // directory-junction-rejection path — that is covered by the
    // dedicated `pending_junction_*` / `sweep_junction_*` tests
    // below — it only pins down the canonicalize-failure contract).
    // resolve_child (`src/commands/data_management_cmd.rs:593-597`)
    // canonicalizes the candidate and, on failure, returns the
    // unvalidated `base.join(stem, ext)` concatenation. A non-existent
    // child does NOT error out and is returned as a plain joined path.
    let dir = temp_dir("resolve-child-missing");
    let base = dir.canonicalize().unwrap();

    let child = resolve_child(&base, "2026-06-24_14-30-25", "wav").unwrap();
    assert!(child.starts_with(&base));

    cleanup(&dir);
}

#[test]
fn test_friendly_io_error_permission() {
    let e = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
    let msg = friendly_io_error(&e);
    assert!(msg.contains("占用") || msg.contains("权限"));
}

#[test]
fn test_friendly_io_error_not_found() {
    let e = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
    let msg = friendly_io_error(&e);
    assert!(msg.contains("不存在"));
}

#[test]
fn test_friendly_delete_error_validation() {
    let e = crate::error::CommandError {
        code: "VALIDATION".to_string(),
        message: "bad stem".to_string(),
    };
    assert_eq!(friendly_delete_error(&e), "文件名不合法");
}

#[test]
fn test_failed_delete_serialize() {
    let fd = FailedDelete {
        filename: "2026-06-24_14-30-25".to_string(),
        error: "文件被占用".to_string(),
    };
    let json = serde_json::to_string(&fd).unwrap();
    assert!(json.contains("2026-06-24_14-30-25"));
    assert!(json.contains("文件被占用"));
}

#[test]
fn test_scan_maps_record_only_fields() {
    let dir = temp_dir("scan-record-only");
    let metadata = serde_json::json!({
        "timestamp": "2026-08-17T10:00:00+08:00",
        "language": "zh",
        "source": "record_only",
        "transcription_status": "failed",
        "dropped_blocks": 12_u64,
        "transcription": serde_json::Value::Null,
    });
    fs::write(
        dir.join("2026-08-17_10-00-00.json"),
        serde_json::to_string(&metadata).unwrap(),
    )
    .unwrap();

    let entries = scan_and_collect(&dir, None).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].source, "record_only");
    assert_eq!(entries[0].transcription_status.as_deref(), Some("failed"));
    assert_eq!(entries[0].dropped_blocks, 12);

    cleanup(&dir);
}

#[test]
fn test_scan_defaults_classic_source_when_missing() {
    let dir = temp_dir("scan-classic-default");
    write_recording(&dir, "2026-06-24_14-30-25", Some("hello"));

    let entries = scan_and_collect(&dir, None).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].source, "classic");
    assert!(entries[0].transcription_status.is_none());
    assert_eq!(entries[0].dropped_blocks, 0);

    cleanup(&dir);
}

// ===========================================================================
// Soft-delete (P2 用户控制权专项) tests — moved-to-pending rename + 5s undo + sweep
// ===========================================================================

/// Per-test tempdir unique by pid + atomic counter to avoid parallel test
/// collisions. `line!()` from inside `fresh_tempdir` is the same for every
/// call — using an AtomicUsize counter instead keeps each test isolated.
use std::sync::atomic::{AtomicUsize, Ordering};
static TEMPDIR_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn fresh_tempdir() -> PathBuf {
    let n = TEMPDIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "dl-test-{}-{}-{}",
        std::process::id(),
        n,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("mkdir");
    dir
}

/// Create a Windows directory junction (reparse point) at `link` pointing
/// to `target`. Used by the security-gate tests below to simulate an
/// attacker who plants a junction inside the data-save base that escapes
/// to an external directory.
///
/// # Constraints
///
/// - **No privileges required** — `mklink /J` does not need Developer Mode
///   or `SeCreateSymbolicLinkPrivilege`, so CI runners (windows-latest)
///   can run it without any setup.
/// - **Absolute cmd.exe path** — the child uses
///   `%SystemRoot%\System32\cmd.exe` so it cannot resolve through PATH/CWD,
///   which could otherwise route an attacker-controlled link to a different
///   binary (defense-in-depth for a security-test helper).
/// - **Plain-form paths** — `link` and `target` MUST be in plain form.
///   `\\?\` verbatim prefixes (produced by `Path::canonicalize`) are
///   REJECTED by `mklink /J`, so this helper takes the plain form
///   returned by `fresh_tempdir()` rather than re-canonicalizing its
///   arguments.
#[cfg(windows)]
fn make_junction(link: &Path, target: &Path) {
    let cmd = format!(
        "{}\\System32\\cmd.exe",
        std::env::var("SystemRoot").expect("SystemRoot set")
    );
    let out = std::process::Command::new(cmd)
        .args(["/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .expect("spawn cmd");
    let success = out.status.success();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        success,
        "mklink /J {} -> {} failed: {stdout} {stderr}",
        link.display(),
        target.display(),
    );
}

#[test]
fn soft_delete_moves_pair_to_pending_and_restores() {
    let base = fresh_tempdir();
    let stem = "2026-09-05_10-00-00";
    write_recording(&base, stem, Some("hello"));

    // Soft-delete via the testable core (returns pairs for restore).
    let SoftDeleteOutcome {
        moved,
        failed,
        pairs,
    } = soft_delete_files(&base, &[stem.to_string()]);
    assert_eq!(moved, 1, "one stem moved (stem-counted, not file-counted)");
    assert!(failed.is_empty());
    assert_eq!(pairs.len(), 2, "wav + json pair");

    // Original files gone, pending files present.
    assert!(!base.join(format!("{stem}.wav")).exists());
    assert!(!base.join(format!("{stem}.json")).exists());
    let pending = base.join(".dl_pending");
    assert!(pending.join(format!("{stem}.wav")).exists());
    assert!(pending.join(format!("{stem}.json")).exists());

    // Restore via PendingDeletes.take_entry — same shape as the command path.
    let pd = Arc::new(PendingDeletes::default());
    let id = pd.schedule_with_finalize(pairs, None);
    let entry = pd.take_entry(id).expect("entry exists pre-window");
    let mut restored = 0;
    for (orig, src) in &entry.pairs {
        if fs::rename(src, orig).is_ok() {
            restored += 1;
        }
    }
    assert_eq!(restored, 2);
    assert!(base.join(format!("{stem}.wav")).exists());
    assert!(base.join(format!("{stem}.json")).exists());
    assert!(!pending.join(format!("{stem}.wav")).exists());

    let _ = fs::remove_dir_all(&base);
}

#[test]
fn soft_delete_rejects_invalid_stem_zero_io() {
    let base = fresh_tempdir();
    let bad = vec![
        "../evil".to_string(),
        "a\\b".to_string(),
        "short".to_string(),
        "".to_string(),
    ];
    let SoftDeleteOutcome {
        moved,
        failed,
        pairs,
    } = soft_delete_files(&base, &bad);
    assert_eq!(moved, 0);
    assert_eq!(failed.len(), bad.len(), "all rejected");
    for f in &failed {
        assert_eq!(f.error, "文件名不合法");
    }
    assert!(pairs.is_empty(), "zero filesystem side effects");
    assert!(!base.join(".dl_pending").exists());
    let _ = fs::remove_dir_all(&base);
}

#[test]
fn soft_delete_missing_files_count_as_moved_without_io() {
    let base = fresh_tempdir();
    let stem = "2026-09-05_10-00-01";
    // No write_recording — files absent on disk.
    let SoftDeleteOutcome {
        moved,
        failed,
        pairs,
    } = soft_delete_files(&base, &[stem.to_string()]);
    assert_eq!(moved, 1, "target state already reached");
    assert!(failed.is_empty());
    assert!(pairs.is_empty(), "no pairs to track");
    let _ = fs::remove_dir_all(&base);
}

#[test]
fn finalize_after_window_removes_pending_and_clears_entry() {
    let base = fresh_tempdir();
    let stem = "2026-09-05_10-00-02";
    write_recording(&base, stem, Some("t"));

    let SoftDeleteOutcome { moved, pairs, .. } = soft_delete_files(&base, &[stem.to_string()]);
    assert_eq!(moved, 1);
    let pd = Arc::new(PendingDeletes::default());
    let id = pd.schedule_with_finalize(pairs, None);

    // Simulate "5s passed, user did not undo" by driving finalize_by_id directly.
    pd.finalize_by_id(id);

    // Pending file should be gone.
    let pending = base.join(".dl_pending");
    assert!(!pending.join(format!("{stem}.wav")).exists());
    assert!(!pending.join(format!("{stem}.json")).exists());
    // Entry consumed — take_entry returns None (the take-once invariant).
    assert!(
        pd.take_entry(id).is_none(),
        "take-once consumed by finalize"
    );

    let _ = fs::remove_dir_all(&base);
}

/// Verify the finalize callback fires ONLY when the entry was actually
/// consumed by the finalize path (not when restore won the take-once race).
/// This is the contract for the `pending-deletes-finalized` event that the
/// frontend toast listens to: a restored batch must never be reported as
/// permanently deleted.
#[test]
fn finalize_callback_fires_only_when_entry_existed() {
    let pd = Arc::new(PendingDeletes::default());
    let fired = Arc::new(std::sync::Mutex::new(Vec::<u64>::new()));
    let f2 = fired.clone();
    let id = pd.schedule_with_finalize(
        vec![(PathBuf::from("a.wav"), PathBuf::from("b.wav"))],
        Some(Arc::new(move |id| f2.lock().unwrap().push(id))),
    );
    // Restore wins the race → finalize path sees no entry → no callback.
    let _ = pd.restore_with_id(id).unwrap();
    pd.finalize_by_id(id);
    assert!(fired.lock().unwrap().is_empty());

    // Second batch: finalize wins → callback fires with the id.
    let f3 = fired.clone();
    let id2 = pd.schedule_with_finalize(
        vec![(PathBuf::from("c.wav"), PathBuf::from("d.wav"))],
        Some(Arc::new(move |id| f3.lock().unwrap().push(id))),
    );
    pd.finalize_by_id(id2);
    assert_eq!(*fired.lock().unwrap(), vec![id2]);
}

#[test]
fn restore_unknown_id_is_error() {
    let base = fresh_tempdir();
    let pd = PendingDeletes::default();
    let result = pd.restore_with_id(99);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.code, "VALIDATION");
    let _ = fs::remove_dir_all(&base);
}

/// The restore report counts RECORDINGS (stems), not files: the frontend
/// renders "已撤销删除，恢复 N 条" and `SoftDeleteBatch::moved` on the delete
/// side is already stem-counted — the two sides must share one unit, or
/// undoing 1 recording displays "恢复 2 条" (wav + json) (P2 E2E finding).
#[test]
fn restore_with_id_counts_stems_not_files() {
    let base = fresh_tempdir();
    let stem_a = "2026-09-12_10-00-00";
    let stem_b = "2026-09-12_10-00-01";
    write_recording(&base, stem_a, Some("a"));
    write_recording(&base, stem_b, Some("b"));
    let SoftDeleteOutcome { pairs, .. } =
        soft_delete_files(&base, &[stem_a.to_string(), stem_b.to_string()]);
    let pd = Arc::new(PendingDeletes::default());
    let id = pd.schedule_with_finalize(pairs, None);

    let report = pd.restore_with_id(id).expect("entry exists");
    assert_eq!(report.restored, 2, "2 recordings, not 4 files");
    assert_eq!(report.failed, 0);
    assert!(base.join(format!("{stem_a}.wav")).exists());
    assert!(base.join(format!("{stem_b}.json")).exists());

    let _ = fs::remove_dir_all(&base);
}

#[test]
fn restore_partial_failure_leaves_failed_pairs_for_sweep() {
    let base = fresh_tempdir();
    let stem = "2026-09-05_10-00-03";
    write_recording(&base, stem, Some("x"));

    let SoftDeleteOutcome { moved, pairs, .. } = soft_delete_files(&base, &[stem.to_string()]);
    assert_eq!(moved, 1);
    let pd = Arc::new(PendingDeletes::default());
    let id = pd.schedule_with_finalize(pairs, None);

    // Yank one of the pending files before the user hits Undo — simulates
    // a process holding the file open or a manual edit.
    let pending = base.join(".dl_pending");
    let wav_pending = pending.join(format!("{stem}.wav"));
    assert!(wav_pending.exists());
    fs::remove_file(&wav_pending).unwrap();

    // restore_pending_delete: entry is consumed (take-once), but the missing
    // wav stays gone (no orphan to restore) — the recording counts as a
    // FAILED restore (stem-level semantics: all files must come back), so
    // the report reads 0/1 even though the json did return.
    let report = pd.restore_with_id(id).expect("entry exists");
    assert_eq!(
        report.restored, 0,
        "stem counts failed: wav was already missing"
    );
    assert_eq!(report.failed, 1);
    assert!(!pd.has_entry(id), "entry taken");

    // Original wav: still missing (rename src was gone — no-op); json back.
    assert!(!base.join(format!("{stem}.wav")).exists());
    assert!(base.join(format!("{stem}.json")).exists());

    let _ = fs::remove_dir_all(&base);
}

#[test]
fn sweep_removes_residual_pending_files() {
    let base = fresh_tempdir();
    let pending = base.join(".dl_pending");
    fs::create_dir_all(&pending).unwrap();
    // Two legitimate files
    fs::write(pending.join("2026-09-05_10-00-00.wav"), b"fake").unwrap();
    fs::write(pending.join("2026-09-05_10-00-00.json"), b"{}").unwrap();
    // One ill-named (should NOT be swept — stem invalid)
    fs::write(pending.join("attacker.txt"), b"x").unwrap();

    let cfg = AppConfig {
        data_saving_path: base.to_string_lossy().to_string(),
        ..AppConfig::default()
    };
    sweep_pending_dir(&cfg);

    assert!(!pending.join("2026-09-05_10-00-00.wav").exists());
    assert!(!pending.join("2026-09-05_10-00-00.json").exists());
    assert!(pending.join("attacker.txt").exists(), "ill-named file kept");

    // Empty/missing config path is a no-op (does not panic).
    let cfg_empty = AppConfig {
        data_saving_path: String::new(),
        ..AppConfig::default()
    };
    sweep_pending_dir(&cfg_empty); // no panic

    let cfg_missing = AppConfig {
        data_saving_path: "/no/such/dir/xyz/abc".to_string(),
        ..AppConfig::default()
    };
    sweep_pending_dir(&cfg_missing); // no panic

    let _ = fs::remove_dir_all(&base);
}

#[test]
fn restore_with_id_returns_validation_when_id_unknown() {
    let base = fresh_tempdir();
    let pd = PendingDeletes::default();
    let err = pd.restore_with_id(12345).unwrap_err();
    assert_eq!(err.code, "VALIDATION");
    assert!(err.message.contains("撤销") || err.message.contains("过期"));
    let _ = fs::remove_dir_all(&base);
}

#[test]
fn schedule_stores_entry_and_timer_runs_finalize() {
    // End-to-end: schedule a timer, sleep >5s, verify entry is gone and file removed.
    let base = fresh_tempdir();
    let stem = "2026-09-05_10-00-04";
    write_recording(&base, stem, Some("z"));
    let SoftDeleteOutcome { moved, pairs, .. } = soft_delete_files(&base, &[stem.to_string()]);
    assert_eq!(moved, 1);

    let pd: Arc<PendingDeletes> = Arc::new(PendingDeletes::default());
    let id = pd.schedule_with_finalize(pairs, None);
    assert!(pd.has_entry(id));

    // Wait for the 5s std::thread timer to fire and finalize.
    std::thread::sleep(Duration::from_secs(6));
    assert!(!pd.has_entry(id), "timer consumed the entry");
    let pending = base.join(".dl_pending");
    assert!(!pending.join(format!("{stem}.wav")).exists());

    let _ = fs::remove_dir_all(&base);
}

/// M4 security gate regression: when `.dl_pending` is replaced with a
/// When `.dl_pending` inside `base` is a Windows **directory junction**
/// pointing OUTSIDE the base, `soft_delete_files` must reject the whole
/// batch (no files moved, no data written to the attacker-chosen
/// external target).
///
/// The guard is `ensure_inside` inside `soft_delete_files`
/// (`src/commands/data_management_cmd.rs:337-346`): the junction target
/// is canonicalized, and `Path::starts_with(base_canonical)` fails for
/// any path that resolves outside the base — so the rename is aborted.
///
/// `mklink /J` is **unprivileged** on Windows (no Developer Mode, no
/// `SeCreateSymbolicLinkPrivilege`), so this test runs in CI on
/// windows-latest without any setup. The CI blind spot it closes is
/// the only remaining coverage gap in the security-gate suite — the
/// pre-existing `#[ignore]` annotation was a historical leftover from
/// the previous implementation, which DID need those privileges.
#[cfg(windows)]
#[test]
fn pending_junction_rejected_by_ensure_inside_guard() {
    let base = fresh_tempdir();
    let stem = "2026-09-05_10-00-05";
    write_recording(&base, stem, Some("secret"));

    // External target the attacker chooses (outside `base`).
    let external = fresh_tempdir();
    let external_pending = external.join(".dl_pending");
    fs::create_dir_all(&external_pending).expect("mkdir external");

    // Remove the legitimate `.dl_pending` (created lazily on first delete)
    // and replace it with a junction pointing to the external dir.
    // `soft_delete_files` calls `create_dir_all` before canonicalize, so
    // the junction must exist BEFORE the call (an attacker would
    // pre-plant it; here we simulate that by removing then mklink /J).
    let pending_link = base.join(".dl_pending");
    let _ = fs::remove_dir(&pending_link);
    make_junction(&pending_link, &external_pending);

    // soft_delete_files: must reject the stem (junction escapes base).
    let SoftDeleteOutcome {
        moved,
        failed,
        pairs,
    } = soft_delete_files(&base, &[stem.to_string()]);
    assert_eq!(moved, 0, "junction must block all renames");
    assert_eq!(failed.len(), 1, "junction reports one failure for the stem");
    assert_eq!(failed[0].filename, stem);
    assert!(pairs.is_empty(), "no pairs to track");

    // Originals must still be in place (no partial moves).
    assert!(
        base.join(format!("{stem}.wav")).exists(),
        "original wav intact"
    );
    assert!(
        base.join(format!("{stem}.json")).exists(),
        "original json intact"
    );

    // External target must be untouched — zero files leaked across.
    let external_entries: Vec<_> = fs::read_dir(&external_pending)
        .expect("read external dir")
        .flatten()
        .collect();
    assert!(
        external_entries.is_empty(),
        "no files leaked into the external target"
    );

    // Explicit three-step cleanup: drop the junction entry itself
    // (remove_dir on a junction only unlinks the reparse point, never
    // deletes the target — defense-in-depth on top of the
    // CVE-2022-21658 stdlib fix), then tear down both tempdirs.
    let _ = fs::remove_dir(&pending_link);
    let _ = fs::remove_dir_all(&base);
    let _ = fs::remove_dir_all(&external);
}

/// Sister test to `pending_junction_rejected_by_ensure_inside_guard`:
/// covers the `sweep_pending_dir` startup-cleanup path
/// (`src/commands/data_management_cmd.rs:424-430`). When `.dl_pending`
/// inside `base` is a junction pointing OUTSIDE the base, sweep must
/// abort (warn + early return) and NOT delete anything from the
/// attacker-chosen external target.
///
/// The victim file MUST be a legal stem + `.wav` extension: if the
/// guard ever regresses (sweep deletes regardless of ensure_inside),
/// the deletion loop would only remove files matching the legal-stem
/// predicate, so an ill-named victim would make the test silent. A
/// legal stem turns any "sweep deleted it" outcome into an
/// immediately-visible regression.
#[cfg(windows)]
#[test]
fn sweep_junction_rejected_by_ensure_inside_guard() {
    let base = fresh_tempdir();
    let external = fresh_tempdir();
    let external_pending = external.join(".dl_pending");
    fs::create_dir_all(&external_pending).expect("mkdir external");

    // Pre-plant a legal-stem victim inside the external target. If the
    // guard regresses and sweep iterates the junction as if it were a
    // real pending dir, this file gets deleted — the regression would
    // be visible as "external wav disappeared".
    let victim = external_pending.join("2026-09-05_10-00-06.wav");
    fs::write(&victim, b"x").expect("write victim");

    // Plant the junction inside `base`.
    let pending_link = base.join(".dl_pending");
    make_junction(&pending_link, &external_pending);

    // sweep_pending_dir: must abort (warn + return) on ensure_inside
    // failure. The victim file MUST survive untouched.
    let cfg = AppConfig {
        data_saving_path: base.to_string_lossy().to_string(),
        ..AppConfig::default()
    };
    sweep_pending_dir(&cfg);

    assert!(
        victim.exists(),
        "external victim file must survive: sweep must not delete across the junction boundary"
    );

    // Explicit three-step cleanup (same convention as the soft-delete
    // sister test above).
    let _ = fs::remove_dir(&pending_link);
    let _ = fs::remove_dir_all(&base);
    let _ = fs::remove_dir_all(&external);
}

// ===========================================================================
// ensure_inside sentinel (P3 候选 8) — single junction-defense variant
// ===========================================================================

#[test]
fn ensure_inside_accepts_child_and_rejects_escape() {
    // ensure_inside requires an ALREADY-CANONICALIZED base (see doc):
    // on Windows canonicalize() yields `\\?\` verbatim prefixes, and
    // Path::starts_with compares by component — a verbatim child never
    // starts_with a plain-form base. fresh_tempdir() returns the plain
    // form, so canonicalize here first (same as the canonicalize precedents
    // in test_resolve_child_no_traversal_for_valid_stem /
    // test_resolve_child_missing_child_returns_plain_join).
    let base = fresh_tempdir().canonicalize().unwrap();
    let inside = base.join("a.wav");
    std::fs::write(&inside, b"x").unwrap();
    assert!(ensure_inside(&base, &inside).is_ok());

    let other = fresh_tempdir().canonicalize().unwrap(); // unrelated dir
    let r = ensure_inside(&base, &other);
    assert!(r.is_err(), "a path outside base must be rejected");
    // Missing candidate surfaces as the canonicalize error (NotFound),
    // NOT as an escape.
    let missing = ensure_inside(&base, &base.join("missing.wav"));
    assert!(missing.unwrap_err().kind() == std::io::ErrorKind::NotFound);

    let _ = fs::remove_dir_all(&base);
    let _ = fs::remove_dir_all(&other);
}
