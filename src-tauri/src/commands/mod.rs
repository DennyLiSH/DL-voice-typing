pub mod config_cmd;
pub mod data_management_cmd;
pub mod delivery_controller;
pub mod download;
pub mod hotkey_pipeline;
pub mod llm_cmd;
pub mod log_cmd;
pub mod perf_cmd;
pub(crate) mod pipeline_state;
pub(crate) mod record_only_session;
pub(crate) mod recording_session;
pub mod review;
pub(crate) mod review_provider;
pub mod window_controller;

use tauri::Emitter as TauriEmitter;

/// Trait for emitting events to the frontend.
/// Abstracts `tauri::AppHandle.emit()` for testability.
/// Uses `serde_json::Value` for trait-object safety.
pub trait EventEmitter: Send + Sync {
    fn emit(&self, event: &str, payload: serde_json::Value);
}

/// Tauri-based event emitter for production use.
pub(crate) struct TauriEventEmitter {
    app: tauri::AppHandle,
}

impl TauriEventEmitter {
    pub fn new(app: tauri::AppHandle) -> Self {
        Self { app }
    }
}

impl EventEmitter for TauriEventEmitter {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        let _ = TauriEmitter::emit(&self.app, event, payload);
    }
}

/// Mock event emitter for testing. Records all emitted events.
pub struct MockEmitter {
    events: std::sync::Mutex<Vec<(String, serde_json::Value)>>,
}

impl Default for MockEmitter {
    fn default() -> Self {
        Self::new()
    }
}

impl MockEmitter {
    /// Create a new mock emitter with an empty event log.
    pub fn new() -> Self {
        Self {
            events: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Drain and return all recorded (event, payload) pairs.
    pub fn take_events(&self) -> Vec<(String, serde_json::Value)> {
        crate::util::lock_mutex(&self.events, "MockEmitter::take_events")
            .map(|mut guard| guard.drain(..).collect())
            .unwrap_or_default()
    }
}

impl EventEmitter for MockEmitter {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        if let Some(mut guard) = crate::util::lock_mutex(&self.events, "MockEmitter::emit") {
            guard.push((event.to_string(), payload));
        }
    }
}

// Re-export all public items so `lib.rs` requires no changes.
pub use config_cmd::{get_compute_mode, get_config, is_autostart_available, save_settings};
pub use download::{
    DownloadState, ModelsResponse, cancel_download, delete_custom_model, download_whisper_model,
    get_whisper_models,
};
pub(crate) use hotkey_pipeline::make_hotkey_callback;
pub use llm_cmd::test_llm_connection;
pub use log_cmd::log_frontend_error;
pub use perf_cmd::get_perf_history;
pub use review::{PendingReview, cancel_review, confirm_inject, get_review_text};
pub use window_controller::WindowController;
