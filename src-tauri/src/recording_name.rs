//! Owner of the recording-filename invariant.
//!
//! A recording on disk is a pair of sidecars sharing one stem:
//! `<stem>.wav` (audio) + `<stem>.json` (metadata — the truth source).
//! The stem layout is `YYYY-MM-DD_HH-MM-SS` (19 chars).
//!
//! This module is the single owner of that knowledge:
//!
//! - [`is_valid_stem`] — the reader's check (does this stem look like one
//!   of ours?),
//! - [`generate_timestamp_filename`] — the writer's producer (does it
//!   always produce a stem the reader accepts?),
//! - [`resolve_recording`] / [`recording_json_path`] — the path builders
//!   with the sandbox defense (canonicalize + `starts_with`).
//!
//! Before this module the writer and the reader each carried their own
//! copy of the rules, and the writer could produce stems the reader
//! rejected (a 10-char date-only fallback, or an RFC3339 string whose
//! `:` is illegal in a Windows filename) — such files were silently
//! invisible to list / usage / delete / undo.
//!
//! Dependency direction: the data layer must not import the command
//! layer. `streaming_recorder` used to import
//! `commands::data_management_cmd::is_valid_stem`; that edge is gone.

use crate::error::AppError;
use std::path::{Path, PathBuf};

/// Pattern: `YYYY-MM-DD_HH-MM-SS` (19 chars total). Validated without regex
/// to avoid pulling in the regex crate for one fixed pattern.
pub(crate) const FILENAME_LEN: usize = 19;

/// Extension of the audio sidecar.
pub(crate) const WAV_EXT: &str = "wav";

/// Extension of the metadata sidecar — the truth source of a recording
/// (an orphan WAV without its JSON is not a recording).
pub(crate) const JSON_EXT: &str = "json";

/// Every extension a recording may have. The pair is always moved,
/// counted and swept together — a third sidecar (e.g. `.srt`) would
/// have to be added here and would then be covered by all of those
/// call sites at once.
pub(crate) const EXTENSIONS: [&str; 2] = [WAV_EXT, JSON_EXT];

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

/// Assemble the canonical stem from the fields of a timestamp.
///
/// Used by the last-resort tier of [`generate_timestamp_filename`]: it
/// does not go through `time`'s format-description machinery at all, so
/// it survives a regression in the format parser.
fn assemble_stem(at: time::OffsetDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02}_{:02}-{:02}-{:02}",
        at.year(),
        u8::from(at.month()),
        at.day(),
        at.hour(),
        at.minute(),
        at.second()
    )
}

/// Generate a timestamp-based filename stem (e.g. "2026-04-02_14-30-25").
///
/// Every fallback tier produces a stem that passes [`is_valid_stem`] —
/// the writer may never hand the reader something it will reject:
///
/// 1. the canonical layout, formatted from the local time;
/// 2. the same layout, formatted from UTC (covers a local time the
///    formatter rejects);
/// 3. manual field assembly (covers a `time`-crate regression in the
///    format-description parser — the reason this chain exists).
///
/// The previous chain ended in RFC3339 (`2026-04-02T14:30:25+08:00`),
/// which contains `:` — illegal in a Windows filename, so the file
/// could never be written — and its middle tier produced a 10-char
/// date-only stem that [`is_valid_stem`] rejects. Both were silently
/// invisible to the readers.
pub(crate) fn generate_timestamp_filename() -> String {
    let local =
        time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    // Tier 1 — canonical layout, local time.
    if let Ok(fmt) = time::format_description::parse_borrowed::<2>(
        "[year]-[month]-[day]_[hour]-[minute]-[second]",
    ) && let Ok(text) = local.format(&fmt)
    {
        return text;
    }
    // Tier 2 — canonical layout, UTC time.
    let utc = time::OffsetDateTime::now_utc();
    if let Ok(fmt) = time::format_description::parse_borrowed::<2>(
        "[year]-[month]-[day]_[hour]-[minute]-[second]",
    ) && let Ok(text) = utc.format(&fmt)
    {
        return text;
    }
    // Tier 3 — no format description at all.
    tracing::warn!(
        target: "recording_name",
        "time format literals failed; assembling the recording stem manually"
    );
    assemble_stem(utc)
}

/// Path of one sidecar under `base` — pure naming, no filesystem access
/// and no sandbox check.
///
/// The stem MUST be validated with [`is_valid_stem`] first; this helper
/// only owns the `<stem>.<ext>` naming rule. Use
/// [`resolve_recording`] when the path is used to read or write a file
/// whose location must be verified.
pub(crate) fn sidecar_path(base: &Path, stem: &str, ext: &str) -> PathBuf {
    base.join(format!("{stem}.{ext}"))
}

/// The sidecar paths of one recording, resolved against a base directory.
#[derive(Debug)]
pub(crate) struct RecordingFiles {
    pub stem: String,
    pub wav: PathBuf,
    pub json: PathBuf,
}

/// Resolve both sidecars of `stem` under `base`.
///
/// Defense (moved here from `commands::data_management_cmd::resolve_child`
/// so it travels with the naming knowledge):
///
/// 1. the stem must pass [`is_valid_stem`] (no separators, no escapes,
///    exact layout);
/// 2. the resolved path is canonicalized and must still be inside
///    `base` — a symlink pointing outside the data directory is
///    rejected.
///
/// A sidecar that does not exist yet canonicalizes to an error; the
/// plain path is returned in that case (the caller decides whether a
/// missing file is an error).
pub(crate) fn resolve_recording(base: &Path, stem: &str) -> Result<RecordingFiles, AppError> {
    let wav = resolve_sidecar(base, stem, WAV_EXT)?;
    let json = resolve_sidecar(base, stem, JSON_EXT)?;
    Ok(RecordingFiles {
        stem: stem.to_string(),
        wav,
        json,
    })
}

/// Resolve one sidecar path under `base` with the sandbox defense.
fn resolve_sidecar(base: &Path, stem: &str, ext: &str) -> Result<PathBuf, AppError> {
    if !is_valid_stem(stem) {
        return Err(AppError::Validation(format!(
            "invalid recording filename: {stem}"
        )));
    }
    let child = sidecar_path(base, stem, ext);
    match child.canonicalize() {
        Ok(canon) => {
            // Compare against the CANONICAL base: on Windows a canonical
            // child carries the `\\?\` prefix while the caller's base may
            // still be a plain path — comparing the two directly would
            // reject every existing file under a non-canonical base.
            let base_canon = base.canonicalize().unwrap_or_else(|_| base.to_path_buf());
            if !canon.starts_with(&base_canon) {
                return Err(AppError::Validation(
                    "path escapes data_saving_path".to_string(),
                ));
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

/// Resolved path of the metadata sidecar of `stem` under `base`.
pub(crate) fn recording_json_path(base: &Path, stem: &str) -> Result<PathBuf, AppError> {
    Ok(resolve_recording(base, stem)?.json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Temp dir for the resolution tests. The helper mirrors the pattern
    /// used by the other inline test modules in this crate (no `unwrap()`
    /// in production files).
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dl-vt-recording-name-{name}"));
        let _ = fs::remove_dir_all(&dir);
        assert!(fs::create_dir_all(&dir).is_ok());
        dir
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

    /// The writer's contract: whatever it produces, the reader accepts.
    #[test]
    fn test_generated_stem_passes_the_reader_check() {
        let stem = generate_timestamp_filename();
        assert!(
            is_valid_stem(&stem),
            "generated stem must pass is_valid_stem: {stem}"
        );
        assert_eq!(stem.len(), FILENAME_LEN);
    }

    /// The manual-assembly tier (the one that survives a `time`-crate
    /// regression) must also produce a valid stem.
    #[test]
    fn test_manual_assembly_produces_a_valid_stem() {
        let stem = assemble_stem(time::OffsetDateTime::now_utc());
        assert!(
            is_valid_stem(&stem),
            "manually assembled stem must pass is_valid_stem: {stem}"
        );
    }

    #[test]
    fn test_extensions_are_pinned() {
        assert_eq!(EXTENSIONS, ["wav", "json"]);
        assert_eq!(EXTENSIONS.len(), 2);
    }

    #[test]
    fn test_resolve_recording_rejects_invalid_stem() {
        let base = PathBuf::from("/tmp");
        let err = match resolve_recording(&base, "..") {
            Ok(_) => panic!("an invalid stem must be rejected"),
            Err(e) => e,
        };
        // The command-error code contract is preserved across the
        // migration (AppError::Validation maps to "VALIDATION" — the
        // frontend's "文件名不合法" mapping keys on it).
        let err = crate::error::CommandError::from(err);
        assert_eq!(err.code, "VALIDATION");
    }

    #[test]
    fn test_resolve_recording_no_traversal_for_valid_stem() {
        let dir = temp_dir("valid-stem");
        let base = dir.canonicalize().unwrap_or_else(|_| dir.clone());
        let wav = match resolve_recording(&base, "2026-06-24_14-30-25") {
            Ok(f) => f.wav,
            Err(e) => panic!("a valid stem must resolve: {e}"),
        };
        assert!(wav.starts_with(&base));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_resolve_recording_missing_child_returns_plain_join() {
        // The resolution canonicalizes the candidate and, on failure,
        // returns the unvalidated `base.join(stem, ext)` concatenation. A
        // non-existent child does NOT error out.
        let dir = temp_dir("resolve-missing");
        let base = dir.canonicalize().unwrap_or_else(|_| dir.clone());

        let child = match resolve_recording(&base, "2026-06-24_14-30-25") {
            Ok(f) => f.wav,
            Err(e) => panic!("a valid stem must resolve: {e}"),
        };
        assert!(child.starts_with(&base));

        let _ = fs::remove_dir_all(&dir);
    }

    /// A sidecar that passes the stem check but resolves OUTSIDE the base
    /// (a symlink pointing elsewhere) must be rejected.
    #[test]
    fn test_resolve_recording_rejects_path_escape() {
        let dir = temp_dir("escape");
        let outside = temp_dir("escape-outside");
        let stem = "2026-06-24_14-30-25";
        // A real file outside the base, linked in under a valid stem.
        let secret = outside.join("secret.wav");
        assert!(fs::write(&secret, b"x").is_ok());
        let link = dir.join(format!("{stem}.wav"));
        let made = std::os::windows::fs::symlink_file(&secret, &link);
        if let Err(e) = made {
            // Creating a file symlink needs Developer Mode (or
            // SeCreateSymbolicLinkPrivilege) — not available on every
            // machine. The escape case is then not observable here; the
            // junction cases in data_management_test.rs cover the
            // equivalent directory-side defense. Revisit when CI gains a
            // Developer-Mode runner or a privileged test identity
            // (limitation tracked in _Project/TODO.md). Known residual:
            // the test binary has NO tracing subscriber (the sole init
            // lives in the production run()), so this warn is never
            // emitted in test runs — the skip still reports as PASS.
            // Production-path callers do reach the structured log.
            tracing::warn!("skipping the path-escape case: symlink_file failed: {e}");
            let _ = fs::remove_dir_all(&dir);
            let _ = fs::remove_dir_all(&outside);
            return;
        }
        let base = dir.canonicalize().unwrap_or_else(|_| dir.clone());
        let result = resolve_recording(&base, stem);
        assert!(
            result.is_err(),
            "a sidecar resolving outside the base must be rejected"
        );

        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&outside);
    }

    #[test]
    fn test_resolve_recording_resolves_both_sidecars() {
        let base = std::env::temp_dir();
        let stem = "2026-06-24_14-30-25";
        let files = match resolve_recording(&base, stem) {
            Ok(f) => f,
            Err(e) => panic!("a valid stem must resolve: {e}"),
        };
        assert_eq!(files.stem, stem);
        assert!(files.wav.ends_with(format!("{stem}.wav")));
        assert!(files.json.ends_with(format!("{stem}.json")));
    }

    /// Surface guard: the naming rule has ONE owner. A bare
    /// `join(format!("{…}"))` in a production file means a second place
    /// re-implements the `<stem>.<ext>` rule — the exact drift this
    /// module was created to remove (the writer could then produce a
    /// name the reader rejects).
    ///
    /// Only the production part of each file is scraped (everything
    /// before its `#[cfg(test)]` marker): test fixtures build their own
    /// paths on purpose.
    #[test]
    fn test_no_bare_sidecar_path_join_outside_this_module() {
        const PATTERN: &str = "join(format!";
        let files: [(&str, &str); 5] = [
            (
                "commands/data_management_cmd.rs",
                include_str!("commands/data_management_cmd.rs"),
            ),
            (
                "commands/record_only_session.rs",
                include_str!("commands/record_only_session.rs"),
            ),
            (
                "commands/transcribe_cmd.rs",
                include_str!("commands/transcribe_cmd.rs"),
            ),
            (
                "streaming_recorder.rs",
                include_str!("streaming_recorder.rs"),
            ),
            ("data_saving.rs", include_str!("data_saving.rs")),
        ];
        for (name, src) in files {
            let production = match src.find("#[cfg(test)]") {
                Some(idx) => &src[..idx],
                None => src,
            };
            // Check-face evidence: the scraped production slice must
            // actually contain code (an empty slice would make the
            // assertion below vacuously true).
            assert!(
                production.len() > 200,
                "{name}: scraped production slice is suspiciously small"
            );
            assert!(
                !production.contains(PATTERN),
                "{name} builds a recording path by hand - use recording_name::sidecar_path"
            );
        }
    }

    #[test]
    fn test_sidecar_path_is_pure_naming() {
        let base = Path::new("/data");
        assert_eq!(
            sidecar_path(base, "2026-06-24_14-30-25", WAV_EXT),
            Path::new("/data/2026-06-24_14-30-25.wav")
        );
        assert_eq!(
            sidecar_path(base, "2026-06-24_14-30-25", JSON_EXT),
            Path::new("/data/2026-06-24_14-30-25.json")
        );
    }
}
