pub mod config_cmd;
pub mod download;
pub mod hotkey_pipeline;
pub mod misc_cmd;
pub(crate) mod pipeline_state;
pub mod review;
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

/// Sentinel value returned to frontend when an API key exists but should not be exposed.
pub const MASKED_MARKER: &str = "__MASKED__";

// Re-export all public items so `lib.rs` requires no changes.
pub use config_cmd::{get_config, save_settings};
pub use download::{
    DownloadState, ModelsResponse, cancel_download, delete_custom_model, download_whisper_model,
    get_whisper_models,
};
pub(crate) use hotkey_pipeline::make_hotkey_callback;
pub use misc_cmd::{get_compute_mode, get_perf_history, test_llm_connection};
pub use review::{PendingReview, cancel_review, confirm_inject, get_review_text};
pub use window_controller::WindowController;
