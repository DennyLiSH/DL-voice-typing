use crate::config::{AppConfig, WhisperModel, check_whisper_models, models_dir};
use crate::error::{AppError, CommandError};
use futures_util::StreamExt;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use tauri::Emitter;
use tokio::io::AsyncWriteExt;

/// Shared state for download management.
pub struct DownloadState {
    pub cancel: Arc<AtomicBool>,
    pub active: Arc<Mutex<Option<String>>>,
}

impl DownloadState {
    pub fn new() -> Self {
        Self {
            cancel: Arc::new(AtomicBool::new(false)),
            active: Arc::new(Mutex::new(None)),
        }
    }
}

impl Default for DownloadState {
    fn default() -> Self {
        Self::new()
    }
}

/// RAII guard that clears the active download on drop (including panic).
struct DownloadGuard {
    active: Arc<Mutex<Option<String>>>,
}

impl DownloadGuard {
    fn new(active: Arc<Mutex<Option<String>>>, model: String) -> Self {
        if let Some(mut g) = crate::util::lock_mutex(&active, "download_active") {
            *g = Some(model);
        }
        Self { active }
    }
}

impl Drop for DownloadGuard {
    fn drop(&mut self) {
        if let Some(mut g) = crate::util::lock_mutex(&self.active, "download_active") {
            *g = None;
        }
    }
}

/// Check which Whisper models are downloaded.
#[tauri::command]
pub fn get_whisper_models() -> Result<std::collections::HashMap<String, bool>, CommandError> {
    Ok(check_whisper_models())
}

/// Download a Whisper model with progress events.
#[tauri::command]
pub async fn download_whisper_model(
    size: String,
    download_state: tauri::State<'_, DownloadState>,
    config_cache: tauri::State<'_, crate::config::ConfigCache>,
    app: tauri::AppHandle,
) -> Result<(), CommandError> {
    // Validate size.
    let model = WhisperModel::all()
        .iter()
        .find(|m| m.size_str() == size)
        .copied()
        .ok_or_else(|| CommandError {
            code: "VALIDATION".to_string(),
            message: format!("unknown model size: {}", size),
        })?;
    let filename = model.filename();

    // Check not already downloading.
    {
        let active = download_state.active.lock().map_err(|e| CommandError {
            code: "LOCK".to_string(),
            message: e.to_string(),
        })?;
        if active.is_some() {
            return Err(CommandError {
                code: "CONFLICT".to_string(),
                message: "a download is already in progress".to_string(),
            });
        }
    }

    // Reset cancel flag.
    download_state
        .cancel
        .store(false, std::sync::atomic::Ordering::SeqCst);

    // Set active with RAII guard.
    let _guard = DownloadGuard::new(download_state.active.clone(), size.clone());

    // Ensure models directory exists.
    let dir = models_dir();
    std::fs::create_dir_all(&dir).map_err(|e| CommandError::from(AppError::from(e)))?;

    let url = {
        let config = AppConfig::read_cached(&config_cache).map_err(CommandError::from)?;
        let base_url = config.download_mirror.base_url();
        format!("{}/{}", base_url, filename)
    };
    let temp_path = dir.join(format!("{}.tmp", filename));
    let final_path = dir.join(filename);

    // Stream download.
    let response = reqwest::get(&url)
        .await
        .map_err(|e| CommandError::from(AppError::from(e)))?;

    let total_size = response.content_length();
    let mut file = tokio::fs::File::create(&temp_path)
        .await
        .map_err(|e| CommandError::from(AppError::from(e)))?;

    let mut stream = response.bytes_stream();
    let mut downloaded: u64 = 0;
    let mut last_percent: u8 = 0;

    while let Some(chunk) = stream.next().await {
        // Check cancel.
        if download_state
            .cancel
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            let _ = file.shutdown().await;
            let _ = std::fs::remove_file(&temp_path);
            return Err(CommandError {
                code: "CANCELLED".to_string(),
                message: "download cancelled".to_string(),
            });
        }

        let chunk = chunk.map_err(|e| {
            let _ = std::fs::remove_file(&temp_path);
            CommandError {
                code: "NETWORK".to_string(),
                message: format!("download stream error: {}", e),
            }
        })?;

        file.write_all(&chunk).await.map_err(|e| {
            let _ = std::fs::remove_file(&temp_path);
            CommandError {
                code: "IO".to_string(),
                message: format!("write error: {}", e),
            }
        })?;

        downloaded += chunk.len() as u64;

        // Emit progress (throttled to 1% changes).
        if let Some(total) = total_size {
            let percent = ((downloaded as f64 / total as f64) * 100.0) as u8;
            if percent != last_percent {
                last_percent = percent;
                let _ = app.emit(
                    "download-progress",
                    serde_json::json!({
                        "size": size,
                        "percent": percent,
                        "downloaded": downloaded,
                        "total": total,
                    }),
                );
            }
        }
    }

    file.flush()
        .await
        .map_err(|e| CommandError::from(AppError::from(e)))?;

    // Rename temp to final.
    std::fs::rename(&temp_path, &final_path).map_err(|e| {
        let _ = std::fs::remove_file(&temp_path);
        CommandError {
            code: "IO".to_string(),
            message: format!("rename failed: {}", e),
        }
    })?;

    Ok(())
}

/// Cancel an active download.
#[tauri::command]
pub fn cancel_download(
    download_state: tauri::State<'_, DownloadState>,
) -> Result<(), CommandError> {
    download_state
        .cancel
        .store(true, std::sync::atomic::Ordering::SeqCst);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{WhisperModel, check_whisper_models};

    #[test]
    fn test_check_whisper_models() {
        let models = check_whisper_models();
        assert_eq!(models.len(), 4);
        assert!(models.contains_key("tiny"));
        assert!(models.contains_key("base"));
        assert!(models.contains_key("small"));
        assert!(models.contains_key("medium"));
    }

    #[test]
    fn test_download_rejects_invalid_size() {
        let valid = WhisperModel::all().iter().find(|m| m.size_str() == "huge");
        assert!(valid.is_none());
    }

    #[test]
    fn test_download_guard_clears_active() {
        let active: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        {
            let _guard = DownloadGuard::new(active.clone(), "base".to_string());
            assert_eq!(*active.lock().unwrap(), Some("base".to_string()));
        }
        // Guard dropped — active should be cleared.
        assert_eq!(*active.lock().unwrap(), None);
    }

    #[test]
    fn test_cancel_download_sets_flag() {
        let ds = DownloadState::new();
        assert!(!ds.cancel.load(std::sync::atomic::Ordering::SeqCst));
        ds.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(ds.cancel.load(std::sync::atomic::Ordering::SeqCst));
    }
}
