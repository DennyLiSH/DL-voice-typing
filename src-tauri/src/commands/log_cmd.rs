use crate::error::CommandError;

/// Receive non-critical errors from the frontend and forward to tracing.
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
    tracing::warn!(
        target: "frontend",
        message = %message,
        stack = %stack.unwrap_or_default(),
        context = %context.unwrap_or_default(),
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
}
