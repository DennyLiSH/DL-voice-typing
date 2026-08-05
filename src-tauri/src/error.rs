/// Unified error type for the application.
#[derive(thiserror::Error, Debug)]
pub enum AppError {
    #[error("audio error: {0}")]
    Audio(String),

    #[error("speech recognition error: {0}")]
    Speech(String),

    #[error("clipboard error: {0}")]
    Clipboard(String),

    #[error("LLM error: {0}")]
    Llm(String),

    #[error("config error: {0}")]
    Config(String),

    #[error("hotkey error: {0}")]
    Hotkey(String),

    #[error("crypto error: {0}")]
    Crypto(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
}

/// Serializable error for Tauri commands.
/// Preserves error category for frontend programmatic handling.
#[derive(Debug, serde::Serialize)]
pub struct CommandError {
    pub code: String,
    pub message: String,
}

impl CommandError {
    /// Generic constructor for arbitrary error codes.
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
        }
    }

    /// State-machine violation (e.g., confirm_review from non-Reviewing state).
    pub fn state(message: impl Into<String>) -> Self {
        Self::new("STATE", message)
    }

    /// Mutex/RwLock poisoning or contention.
    pub fn lock<E: std::fmt::Display>(e: E) -> Self {
        Self::new("LOCK", e.to_string())
    }

    /// I/O failure with context prefix (template: "<ctx>: <e>").
    pub fn io<E: std::fmt::Display>(e: E, context: &str) -> Self {
        Self::new("IO", format!("{context}: {e}"))
    }

    /// Input validation failure.
    pub fn validation(message: impl Into<String>) -> Self {
        Self::new("VALIDATION", message)
    }
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl From<AppError> for CommandError {
    fn from(e: AppError) -> Self {
        let (code, message) = match &e {
            AppError::Audio(msg) => ("AUDIO", msg.clone()),
            AppError::Speech(msg) => ("SPEECH", msg.clone()),
            AppError::Clipboard(msg) => ("CLIPBOARD", msg.clone()),
            AppError::Llm(msg) => ("LLM", msg.clone()),
            AppError::Config(msg) => ("CONFIG", msg.clone()),
            AppError::Hotkey(msg) => ("HOTKEY", msg.clone()),
            AppError::Crypto(msg) => ("CRYPTO", msg.clone()),
            AppError::Io(io_err) => ("IO", io_err.to_string()),
            AppError::Json(json_err) => ("JSON", json_err.to_string()),
            AppError::Http(http_err) => ("HTTP", http_err.to_string()),
        };
        CommandError {
            code: code.to_string(),
            message,
        }
    }
}

impl From<cpal::BuildStreamError> for AppError {
    fn from(e: cpal::BuildStreamError) -> Self {
        AppError::Audio(e.to_string())
    }
}

impl From<cpal::PlayStreamError> for AppError {
    fn from(e: cpal::PlayStreamError) -> Self {
        AppError::Audio(e.to_string())
    }
}

impl From<cpal::SupportedStreamConfigsError> for AppError {
    fn from(e: cpal::SupportedStreamConfigsError) -> Self {
        AppError::Audio(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_io_error_conversion() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let app_err: AppError = io_err.into();
        assert!(matches!(app_err, AppError::Io(_)));
        assert!(app_err.to_string().contains("file not found"));
    }

    #[test]
    fn test_json_error_conversion() {
        let json_err = serde_json::from_str::<serde_json::Value>("invalid json");
        let app_err: AppError = json_err.unwrap_err().into();
        assert!(matches!(app_err, AppError::Json(_)));
    }

    #[test]
    fn test_error_display() {
        let err = AppError::Audio("no microphone".to_string());
        assert_eq!(err.to_string(), "audio error: no microphone");
    }

    #[test]
    fn test_command_error_from_app_error() {
        let err = AppError::Speech("model not loaded".to_string());
        let cmd_err: CommandError = err.into();
        assert_eq!(cmd_err.code, "SPEECH");
        assert_eq!(cmd_err.message, "model not loaded");
    }

    #[test]
    fn test_command_error_display() {
        let err = CommandError {
            code: "CONFIG".to_string(),
            message: "bad value".to_string(),
        };
        assert_eq!(err.to_string(), "CONFIG: bad value");
    }

    #[test]
    fn test_command_error_new_generic() {
        let err = CommandError::new("CUSTOM", "detail");
        assert_eq!(err.code, "CUSTOM");
        assert_eq!(err.message, "detail");
    }

    #[test]
    fn test_command_error_state_helper() {
        let err = CommandError::state("cannot confirm from current state");
        assert_eq!(err.code, "STATE");
        assert_eq!(err.message, "cannot confirm from current state");
    }

    #[test]
    fn test_command_error_lock_helper() {
        let err = CommandError::lock(std::io::Error::other("poisoned"));
        assert_eq!(err.code, "LOCK");
        assert!(err.message.contains("poisoned"));
    }

    #[test]
    fn test_command_error_io_helper() {
        let err = CommandError::io(std::io::Error::other("disk full"), "save_settings");
        assert_eq!(err.code, "IO");
        assert_eq!(err.message, "save_settings: disk full");
    }

    #[test]
    fn test_command_error_validation_helper() {
        let err = CommandError::validation("invalid filename");
        assert_eq!(err.code, "VALIDATION");
        assert_eq!(err.message, "invalid filename");
    }

    #[test]
    fn test_state_helper_byte_identical_to_struct_literal() {
        let helper = CommandError::state("cannot confirm from current state");
        let literal = CommandError {
            code: "STATE".to_string(),
            message: "cannot confirm from current state".to_string(),
        };
        assert_eq!(helper.code, literal.code);
        assert_eq!(helper.message, literal.message);

        let helper2 = CommandError::state("cancel_reviewing failed");
        let literal2 = CommandError {
            code: "STATE".to_string(),
            message: "cancel_reviewing failed".to_string(),
        };
        assert_eq!(helper2.code, literal2.code);
        assert_eq!(helper2.message, literal2.message);

        let helper3 = CommandError::state("cannot cancel from current state");
        let literal3 = CommandError {
            code: "STATE".to_string(),
            message: "cannot cancel from current state".to_string(),
        };
        assert_eq!(helper3.code, literal3.code);
        assert_eq!(helper3.message, literal3.message);
    }

    #[test]
    fn test_download_cancelled_message_byte_identical() {
        let err = CommandError::new("CANCELLED", "download cancelled");
        assert_eq!(err.message, "download cancelled");
        assert_eq!(err.message.len(), "download cancelled".len());
    }
}
