use super::hotkey_pipeline::make_hotkey_callback;
use crate::config::{ApiKeyMask, AppConfig};
use crate::error::CommandError;
use crate::hotkey::HotkeyManager;
use crate::hotkey::windows::WindowsHotkeyManager;
use crate::speech::SpeechEngine;
use std::sync::Arc;
use std::sync::{Mutex, mpsc};
use std::time::Duration;
use tauri::{Emitter, Manager};

/// Whether autostart is available in the current build.
/// - Release: always true.
/// - Debug: only when DL_AUTOSTART=1 env var is set.
#[tauri::command]
pub fn is_autostart_available() -> bool {
    if cfg!(debug_assertions) {
        std::env::var("DL_AUTOSTART").as_deref() == Ok("1")
    } else {
        true
    }
}

/// Return the current compute mode: "gpu", "cpu", or "unloaded".
#[tauri::command]
pub fn get_compute_mode(
    engine: tauri::State<'_, Arc<dyn SpeechEngine>>,
) -> Result<String, CommandError> {
    Ok(engine.compute_mode().to_string())
}

/// Return the current application config to the frontend.
/// The API key is replaced with a masked marker if set.
#[tauri::command]
pub fn get_config(
    config_cache: tauri::State<'_, crate::config::ConfigCache>,
) -> Result<AppConfig, CommandError> {
    let mut config: AppConfig = (*config_cache.read_cached()).clone();
    config.llm_api_key = ApiKeyMask::mask(&config.llm_api_key);
    Ok(config)
}

/// Save all settings. Handles hotkey re-registration on the main thread.
#[tauri::command]
pub fn save_settings(
    config: AppConfig,
    config_cache: tauri::State<'_, crate::config::ConfigCache>,
    _hotkey_manager: tauri::State<'_, Mutex<WindowsHotkeyManager>>,
    app: tauri::AppHandle,
) -> Result<(), CommandError> {
    // Validate first.
    config.validate().map_err(CommandError::from)?;

    // Load old config to detect hotkey change and preserve API key if masked.
    let old_config = config_cache.read_cached();
    let hotkey_changed = config.hotkey != old_config.hotkey;

    // If the frontend sent the masked marker, preserve the existing decrypted key.
    let mut config = config;
    config.llm_api_key = ApiKeyMask::unmask_or_keep(&config.llm_api_key, &old_config.llm_api_key);

    // Save new config to disk and update cache (save_cached encrypts the API key).
    config_cache
        .save_cached(&config)
        .map_err(CommandError::from)?;

    // Re-register hotkey if changed.
    if hotkey_changed {
        let (tx, rx) = mpsc::channel();
        let old_key = old_config.hotkey;
        let new_key = config.hotkey;
        let app_clone = app.clone();

        let _ = app.run_on_main_thread(move || {
            // Access hotkey_manager via app.state() inside the closure.
            let hm_state = app_clone.state::<Mutex<WindowsHotkeyManager>>();
            let mut hm = match hm_state.lock() {
                Ok(hm) => hm,
                Err(e) => {
                    let _ = tx.send(Err(format!("lock failed: {e}")));
                    return;
                }
            };

            // Unregister ONLY the primary slot (M2 fix — keeps record_only
            // and cancel_esc live across save_settings re-registration).
            // Full unregister() here would wipe the cancel_esc slot, which
            // the Esc-cancel dispatcher reads from to abort an in-flight
            // transcription. The hook itself is removed by unregister_primary
            // only when all three slots are clear.
            if let Err(e) = hm.unregister_primary() {
                let _ = tx.send(Err(format!("unregister failed: {e}")));
                return;
            }

            // M3 fix — pull the managed single PipelineState instance instead
            // of constructing a new one with PipelineState::from_app. The new
            // instance would have an independent DeliveryController context,
            // and the cancel_esc callback would target the wrong PS.
            let ps_managed = app_clone
                .state::<super::pipeline_state::PipelineState>()
                .inner()
                .clone();
            let callback = make_hotkey_callback(ps_managed.clone());

            // Try registering the new key.
            match hm.register(new_key, callback) {
                Ok(()) => {
                    let _ = tx.send(Ok(()));
                }
                Err(e) => {
                    // Fallback: re-register the old key.
                    let fallback_callback = make_hotkey_callback(ps_managed);
                    let _ = hm.register(old_key, fallback_callback);
                    let _ = tx.send(Err(format!(
                        "新热键注册失败({e})，已回退到旧热键: {}",
                        old_key.display()
                    )));
                }
            }
        });

        // Wait for the main thread callback to complete.
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => {
                // Hotkey error — config is saved but hotkey didn't change.
                // This emit bypasses the trait emitter, so record it into the
                // error history directly. try_state (not state): this point
                // runs after the config is already persisted, and a panic
                // here would tell the user "save failed" when it hadn't.
                if let Some(history) =
                    app.try_state::<Arc<crate::commands::error_history::ErrorHistory>>()
                {
                    history.record("hotkey-error", &serde_json::json!(e));
                } else {
                    tracing::warn!(
                        target: "error_history",
                        "hotkey-error not recorded: ErrorHistory not managed"
                    );
                }
                let _ = app.emit("hotkey-error", &e);
                Err(CommandError::new("HOTKEY", e))
            }
            Err(_) => Err(CommandError::new(
                "HOTKEY",
                "hotkey re-registration timed out",
            )),
        }
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::config::AppConfig;
    use crate::config::WhisperModel;
    use crate::hotkey::windows::HotkeySpec;

    #[test]
    fn test_read_cached_returns_default() {
        let cache = crate::config::ConfigCache::new(AppConfig::default());
        let result = cache.read_cached();
        assert_eq!(
            result.hotkey,
            HotkeySpec {
                ctrl: false,
                shift: false,
                alt: false,
                vk: 0xA3,
            }
        );
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
        // vk=0 is the simplest invalid object form — no name roundtrip.
        let config = AppConfig {
            hotkey: HotkeySpec {
                ctrl: false,
                shift: false,
                alt: false,
                vk: 0,
            },
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
