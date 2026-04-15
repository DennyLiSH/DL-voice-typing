pub mod config_cmd;
pub mod download;
pub mod hotkey_pipeline;
pub mod misc_cmd;
pub mod review;

/// Sentinel value returned to frontend when an API key exists but should not be exposed.
pub const MASKED_MARKER: &str = "__MASKED__";

// Re-export all public items so `lib.rs` requires no changes.
pub use config_cmd::{get_config, save_settings};
pub use download::{DownloadState, cancel_download, download_whisper_model, get_whisper_models};
pub use hotkey_pipeline::make_hotkey_callback;
pub use misc_cmd::{get_compute_mode, get_perf_history, test_llm_connection};
pub use review::{PendingReview, cancel_review, confirm_inject, get_review_text};
