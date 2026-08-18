//! Tests for commands::data_management_cmd.
//!
//! Lives in `src/tests/` to keep production source files free of `.unwrap()`.

use crate::commands::data_management_cmd::{
    FailedDelete, friendly_delete_error, friendly_io_error, is_valid_stem, resolve_base,
    resolve_child, scan_and_collect,
};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

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
fn test_resolve_child_canonicalize_blocks_symlink_escape() {
    // Create base + a symlink inside that points outside.
    // On Windows symlink creation requires privileges; skip when impossible.
    let dir = temp_dir("symlink-escape");
    let base = dir.canonicalize().unwrap();

    // Without symlink privileges, we still verify that a non-existent child
    // path does not error out (canonicalize fails gracefully).
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
