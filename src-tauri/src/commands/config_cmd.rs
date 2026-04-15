use super::MASKED_MARKER;
use super::hotkey_pipeline::make_hotkey_callback;
use crate::audio::AudioCapture;
use crate::config::AppConfig;
use crate::error::CommandError;
use crate::hotkey::HotkeyManager;
use crate::hotkey::windows::WindowsHotkeyManager;
use crate::perf::PerfHistory;
use crate::speech::AnyEngine;
use crate::state::StateMachine;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;
use tauri::{Emitter, Manager};

/// Return the current application config to the frontend.
/// The API key is replaced with a masked marker if set.
#[tauri::command]
pub fn get_config(
    config_cache: tauri::State<'_, crate::config::ConfigCache>,
) -> Result<AppConfig, CommandError> {
    let mut config = AppConfig::read_cached(&config_cache).map_err(CommandError::from)?;
    if !config.llm_api_key.is_empty() {
        config.llm_api_key = MASKED_MARKER.to_string();
    }
    Ok(config)
}

/// Save all settings. Handles hotkey re-registration on the main thread.
#[tauri::command]
pub fn save_settings(
    config: AppConfig,
    config_cache: tauri::State<'_, crate::config::ConfigCache>,
    _hotkey_manager: tauri::State<'_, Mutex<WindowsHotkeyManager>>,
    state_machine: tauri::State<'_, Arc<Mutex<StateMachine>>>,
    audio_capture: tauri::State<'_, Arc<Mutex<AudioCapture>>>,
    perf_history: tauri::State<'_, Arc<PerfHistory>>,
    app: tauri::AppHandle,
) -> Result<(), CommandError> {
    // Validate first.
    config.validate().map_err(CommandError::from)?;

    // Load old config to detect hotkey change and preserve API key if masked.
    let old_config = AppConfig::read_cached(&config_cache).map_err(CommandError::from)?;
    let hotkey_changed = config.hotkey != old_config.hotkey;

    // If the frontend sent the masked marker, preserve the existing decrypted key.
    let mut config = config;
    if config.llm_api_key == MASKED_MARKER
        || (config.llm_api_key.is_empty() && !old_config.llm_api_key.is_empty())
    {
        config.llm_api_key = old_config.llm_api_key;
    }

    // Save new config to disk and update cache (save_cached encrypts the API key).
    config
        .save_cached(&config_cache)
        .map_err(CommandError::from)?;

    // Re-register hotkey if changed.
    if hotkey_changed {
        let (tx, rx) = mpsc::channel();
        let sm = state_machine.inner().clone();
        let ac = audio_capture.inner().clone();
        let ph = perf_history.inner().clone();
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
            let engine = app_clone.state::<Arc<AnyEngine>>().inner().clone();
            let cb = app_clone
                .state::<Arc<Mutex<crate::clipboard::ClipboardManager>>>()
                .inner()
                .clone();
            let ph_clone = ph.clone();
            let cc = app_clone
                .state::<crate::config::ConfigCache>()
                .inner()
                .clone();
            let callback =
                make_hotkey_callback(sm, ac, engine, cb, ph_clone, app_clone.clone(), cc);

            // Try registering the new key.
            match hm.register(&new_key, callback) {
                Ok(()) => {
                    let _ = tx.send(Ok(()));
                }
                Err(e) => {
                    // Fallback: re-register the old key.
                    let sm2 = app_clone
                        .state::<Arc<Mutex<StateMachine>>>()
                        .inner()
                        .clone();
                    let ac2 = app_clone
                        .state::<Arc<Mutex<AudioCapture>>>()
                        .inner()
                        .clone();
                    let engine2 = app_clone.state::<Arc<AnyEngine>>().inner().clone();
                    let cb2 = app_clone
                        .state::<Arc<Mutex<crate::clipboard::ClipboardManager>>>()
                        .inner()
                        .clone();
                    let ph2 = ph.clone();
                    let cc2 = app_clone
                        .state::<crate::config::ConfigCache>()
                        .inner()
                        .clone();
                    let fallback_callback =
                        make_hotkey_callback(sm2, ac2, engine2, cb2, ph2, app_clone.clone(), cc2);
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
                Err(CommandError {
                    code: "HOTKEY".to_string(),
                    message: e,
                })
            }
            Err(_) => Err(CommandError {
                code: "HOTKEY".to_string(),
                message: "hotkey re-registration timed out".to_string(),
            }),
        }
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::config::AppConfig;
    use crate::config::WhisperModel;

    #[test]
    fn test_read_cached_returns_default() {
        let cache = crate::config::ConfigCache::new(std::sync::RwLock::new(AppConfig::default()));
        let result = AppConfig::read_cached(&cache);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().hotkey, "RightCtrl");
    }

    #[test]
    fn test_default_config_validates() {
        let config = AppConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_enum_serialization_roundtrip() {
        let config = AppConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        // WhisperModel::Base should serialize as "base".
        assert!(json.contains("\"whisper_model\":\"base\""));
        // Language::Zh should serialize as "zh".
        assert!(json.contains("\"language\":\"zh\""));
        // DownloadMirror::HfMirror should serialize as "hf-mirror".
        assert!(json.contains("\"download_mirror\":\"hf-mirror\""));
        let parsed: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.whisper_model, WhisperModel::Base);
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
}
