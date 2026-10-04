use crate::commands::error_history::ErrorHistory;
use crate::error::CommandError;

/// Stack truncation bound: a JS stack's ~30 useful frames fit in 2000 chars;
/// the cap bounds pathological (deep-recursion) stacks entering the log.
const MAX_STACK_CHARS: usize = 2000;

/// Context truncation bound: contexts are kebab-case operation labels; the
/// cap exists for IPC-boundary completeness (reportError is a public
/// frontend API — a future call site may pass a dynamic string).
const MAX_CONTEXT_CHARS: usize = 200;

/// Silent char-boundary-safe truncation — same convention as
/// `ErrorHistory` message truncation and `redact_error_detail`'s tail cut.
fn truncate_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Receive non-critical errors from the frontend and forward to tracing.
///
/// Covers 4 user-initiated failure points in settings.js (save_settings,
/// test_llm_connection, download_whisper_model, delete_custom_model) where
/// UI feedback already informs the user; this command lets the same failure
/// be audited via the backend rolling log file after the fact.
///
/// Fields are truncated at the sink (message 500 / stack 2000 / context 200
/// chars, char-boundary safe) so an unbounded IPC payload cannot bloat the
/// tracing log; mirrors ErrorHistory's 500-char message bound.
///
/// Fire-and-forget: tracing uses `non_blocking::WorkerGuard` by design —
/// log write failures are not surfaced to the caller. UI feedback runs
/// through independent channels (showError/setStatus).
///
/// Covers 4 user-initiated failure points in settings.js (save_settings,
/// test_llm_connection, download_whisper_model, delete_custom_model) where
/// UI feedback already informs the user; this command lets the same failure
/// be audited via the backend rolling log file after the fact.
///
/// Fire-and-forget: tracing uses `non_blocking::WorkerGuard` by design —
/// log write failures are not surfaced to the caller. UI feedback runs
/// through independent channels (showError/setStatus).
#[tauri::command]
pub fn log_frontend_error(
    message: String,
    stack: Option<String>,
    context: Option<String>,
) -> Result<(), CommandError> {
    let message = truncate_chars(&message, ErrorHistory::MAX_MESSAGE_CHARS);
    let stack = truncate_chars(&stack.unwrap_or_default(), MAX_STACK_CHARS);
    let context = truncate_chars(&context.unwrap_or_default(), MAX_CONTEXT_CHARS);
    tracing::warn!(
        target: "frontend",
        message = %message,
        stack = %stack,
        context = %context,
        "frontend error"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lock the invariant: command always returns Ok regardless of input shape.
    /// Also verifies no panic on empty message / None stack / None context
    /// (covers the `unwrap_or_default()` branches).
    #[test]
    fn log_frontend_error_returns_ok_with_any_input() {
        assert!(log_frontend_error("msg".into(), None, None).is_ok());
        assert!(log_frontend_error("".into(), Some("stack".into()), Some("ctx".into())).is_ok());
        assert!(log_frontend_error("a".into(), Some("".into()), Some("".into())).is_ok());
    }

    /// --- Sink-side truncation contract (2026-10-04) ---
    /// Mirrors the silent chars().take() convention of ErrorHistory
    /// (error_history.rs) and redact_error_detail (llm/mod.rs).

    #[test]
    fn message_truncated_to_500_chars() {
        let out = truncate_chars(&"a".repeat(600), ErrorHistory::MAX_MESSAGE_CHARS);
        assert_eq!(out.chars().count(), ErrorHistory::MAX_MESSAGE_CHARS);
    }

    #[test]
    fn stack_truncated_to_2000_chars() {
        let out = truncate_chars(&"b".repeat(2500), MAX_STACK_CHARS);
        assert_eq!(out.chars().count(), MAX_STACK_CHARS);
    }

    #[test]
    fn context_truncated_to_200_chars() {
        let out = truncate_chars(&"c".repeat(300), MAX_CONTEXT_CHARS);
        assert_eq!(out.chars().count(), MAX_CONTEXT_CHARS);
    }

    #[test]
    fn at_limit_input_is_untouched() {
        let s = "x".repeat(ErrorHistory::MAX_MESSAGE_CHARS);
        assert_eq!(truncate_chars(&s, ErrorHistory::MAX_MESSAGE_CHARS), s);
    }

    #[test]
    fn multibyte_boundary_is_char_safe() {
        let out = truncate_chars(&"中".repeat(600), ErrorHistory::MAX_MESSAGE_CHARS);
        assert_eq!(out.chars().count(), ErrorHistory::MAX_MESSAGE_CHARS);
        assert!(out.chars().all(|c| c == '中'));
    }

    #[test]
    fn command_ok_with_over_limit_fields() {
        assert!(
            log_frontend_error(
                "x".repeat(600),
                Some("y".repeat(3000)),
                Some("z".repeat(400)),
            )
            .is_ok()
        );
    }
}
