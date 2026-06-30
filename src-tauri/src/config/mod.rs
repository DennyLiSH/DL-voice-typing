pub mod cache;
pub mod persistence;
pub mod presentation;
pub mod schema;

pub use cache::ConfigCache;
pub use persistence::{check_whisper_models, model_path_for_size, models_dir, scan_custom_models};
pub use presentation::{ApiKeyMask, MASKED_MARKER};
pub use schema::{
    AppConfig, DownloadMirror, Language, PipelineMode, Q8_MODELS_BASE_URL, WhisperModel,
};
