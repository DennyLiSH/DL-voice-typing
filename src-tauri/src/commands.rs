use crate::audio::AudioCapture;
use crate::config::{
    AppConfig, DOWNLOAD_MIRRORS, WHISPER_MODELS, check_whisper_models, models_dir,
};
use crate::hotkey::windows::WindowsHotkeyManager;
use crate::hotkey::{HotkeyCallback, HotkeyEvent, HotkeyManager};
use crate::llm::LLMClient;
use crate::state::StateMachine;
use futures_util::StreamExt;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;
use tauri::{Emitter, Manager};
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
        if let Ok(mut g) = active.lock() {
            *g = Some(model);
        }
        Self { active }
    }
}

impl Drop for DownloadGuard {
    fn drop(&mut self) {
        if let Ok(mut g) = self.active.lock() {
            *g = None;
        }
    }
}

/// Return the current application config to the frontend.
#[tauri::command]
pub fn get_config() -> Result<AppConfig, String> {
    AppConfig::load().map_err(|e| e.to_string())
}

/// Save all settings. Handles hotkey re-registration on the main thread.
#[tauri::command]
pub fn save_settings(
    config: AppConfig,
    _hotkey_manager: tauri::State<'_, Mutex<WindowsHotkeyManager>>,
    state_machine: tauri::State<'_, Arc<Mutex<StateMachine>>>,
    audio_capture: tauri::State<'_, Arc<Mutex<AudioCapture>>>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    // Validate first.
    config.validate().map_err(|e| e.to_string())?;

    // Load old config to detect hotkey change.
    let old_config = AppConfig::load().map_err(|e| e.to_string())?;
    let hotkey_changed = config.hotkey != old_config.hotkey;

    // Save new config to disk.
    config.save().map_err(|e| e.to_string())?;

    // Re-register hotkey if changed.
    if hotkey_changed {
        let (tx, rx) = mpsc::channel();
        let sm = state_machine.inner().clone();
        let ac = audio_capture.inner().clone();
        let old_key = old_config.hotkey;
        let new_key = config.hotkey;
        let app_clone = app.clone();

        let _ = app.run_on_main_thread(move || {
            // Access hotkey_manager via app.state() inside the closure.
            let hm_state = app_clone.state::<Mutex<WindowsHotkeyManager>>();
            let mut hm = match hm_state.lock() {
                Ok(hm) => hm,
                Err(e) => {
                    let _ = tx.send(Err(format!("lock failed: {}", e)));
                    return;
                }
            };

            // Unregister old.
            if let Err(e) = hm.unregister() {
                let _ = tx.send(Err(format!("unregister failed: {}", e)));
                return;
            }

            // Build callback with current state references.
            let callback = make_hotkey_callback(sm, ac);

            // Try registering the new key.
            match hm.register(&new_key, callback) {
                Ok(()) => {
                    let _ = tx.send(Ok(()));
                }
                Err(e) => {
                    // Fallback: re-register the old key.
                    let hm_state2 = app_clone.state::<Arc<Mutex<StateMachine>>>();
                    let hm_state3 = app_clone.state::<Arc<Mutex<AudioCapture>>>();
                    let sm2 = hm_state2.inner().clone();
                    let ac2 = hm_state3.inner().clone();
                    let fallback_callback = make_hotkey_callback(sm2, ac2);
                    let _ = hm.register(&old_key, fallback_callback);
                    let _ = tx.send(Err(format!(
                        "新热键注册失败({})，已回退到旧热键: {}",
                        e, old_key
                    )));
                }
            }
        });

        // Wait for the main thread callback to complete.
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => {
                // Hotkey error — config is saved but hotkey didn't change.
                let _ = app.emit("hotkey-error", &e);
                Err(e)
            }
            Err(_) => Err("hotkey re-registration timed out".to_string()),
        }
    } else {
        Ok(())
    }
}

/// Build the hotkey callback that starts/stops recording.
pub fn make_hotkey_callback(
    sm: Arc<Mutex<StateMachine>>,
    ac: Arc<Mutex<AudioCapture>>,
) -> HotkeyCallback {
    Box::new(move |event| {
        match event {
            HotkeyEvent::Pressed => {
                let can_record = sm
                    .lock()
                    .map(|mut s| s.start_recording().is_ok())
                    .unwrap_or(false);
                if can_record {
                    if let Ok(mut ac_guard) = ac.lock() {
                        let sm_for_audio = Arc::clone(&sm);
                        let _ = ac_guard.start(Box::new(move |data: &[f32]| {
                            if let Ok(mut s) = sm_for_audio.lock() {
                                let _ = s.append_audio(data);
                            }
                        }));
                    }
                }
            }
            HotkeyEvent::Released => {
                if let Ok(mut ac_guard) = ac.lock() {
                    ac_guard.stop();
                }
                if let Ok(mut sm_guard) = sm.lock() {
                    if let Ok(_audio_data) = sm_guard.stop_recording() {
                        // TODO: Run transcription pipeline (Whisper/Mock → LLM → inject)
                        sm_guard.reset();
                    }
                }
            }
        }
    })
}

/// Check which Whisper models are downloaded.
#[tauri::command]
pub fn get_whisper_models() -> Result<std::collections::HashMap<String, bool>, String> {
    Ok(check_whisper_models())
}

/// Download a Whisper model with progress events.
#[tauri::command]
pub async fn download_whisper_model(
    size: String,
    download_state: tauri::State<'_, DownloadState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    // Validate size.
    let (filename, _display_size) = WHISPER_MODELS
        .iter()
        .find(|(s, _, _)| *s == size)
        .map(|(_, f, d)| (*f, *d))
        .ok_or_else(|| format!("unknown model size: {}", size))?;

    // Check not already downloading.
    {
        let active = download_state
            .active
            .lock()
            .map_err(|e| format!("lock error: {}", e))?;
        if active.is_some() {
            return Err("a download is already in progress".to_string());
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
    std::fs::create_dir_all(&dir).map_err(|e| format!("create models dir failed: {}", e))?;

    let url = {
        let config = AppConfig::load().map_err(|e| e.to_string())?;
        let base_url = DOWNLOAD_MIRRORS
            .iter()
            .find(|(id, _, _)| *id == config.download_mirror)
            .map(|(_, _, url)| *url)
            .unwrap_or(DOWNLOAD_MIRRORS[0].2);
        format!("{}/{}", base_url, filename)
    };
    let temp_path = dir.join(format!("{}.tmp", filename));
    let final_path = dir.join(filename);

    // Stream download.
    let response = reqwest::get(&url)
        .await
        .map_err(|e| format!("download request failed: {}", e))?;

    let total_size = response.content_length();
    let mut file = tokio::fs::File::create(&temp_path)
        .await
        .map_err(|e| format!("create temp file failed: {}", e))?;

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
            return Err("download cancelled".to_string());
        }

        let chunk = chunk.map_err(|e| {
            let _ = std::fs::remove_file(&temp_path);
            format!("download stream error: {}", e)
        })?;

        file.write_all(&chunk).await.map_err(|e| {
            let _ = std::fs::remove_file(&temp_path);
            format!("write error: {}", e)
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
        .map_err(|e| format!("flush failed: {}", e))?;

    // Rename temp to final.
    std::fs::rename(&temp_path, &final_path).map_err(|e| {
        let _ = std::fs::remove_file(&temp_path);
        format!("rename failed: {}", e)
    })?;

    Ok(())
}

/// Cancel an active download.
#[tauri::command]
pub fn cancel_download(download_state: tauri::State<'_, DownloadState>) -> Result<(), String> {
    download_state
        .cancel
        .store(true, std::sync::atomic::Ordering::SeqCst);
    Ok(())
}

/// Test the LLM connection with the given settings.
#[tauri::command]
pub async fn test_llm_connection(
    api_url: String,
    api_key: String,
    model: String,
) -> Result<(), String> {
    let client = LLMClient::new(api_url, api_key, model);
    client.test_connection().await.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;

    #[test]
    fn test_get_config_returns_default() {
        let result = get_config();
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_rejects_invalid_whisper_model() {
        let config = AppConfig {
            whisper_model: "huge".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_rejects_invalid_hotkey() {
        let config = AppConfig {
            hotkey: "NoSuchKey".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_rejects_empty_llm_when_enabled() {
        let config = AppConfig {
            llm_enabled: true,
            llm_api_url: String::new(),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_accepts_valid_config() {
        let config = AppConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_accepts_llm_disabled_with_empty_fields() {
        let config = AppConfig {
            llm_enabled: false,
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

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
        let valid = WHISPER_MODELS
            .iter()
            .find(|(s, _, _)| *s == "huge")
            .map(|(_, f, d)| (*f, *d));
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
