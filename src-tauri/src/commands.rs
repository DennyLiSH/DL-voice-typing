use crate::audio::AudioCapture;
use crate::audio::rms;
use crate::config::{
    AppConfig, DOWNLOAD_MIRRORS, WHISPER_MODELS, check_whisper_models, models_dir,
};
use crate::hotkey::windows::WindowsHotkeyManager;
use crate::hotkey::{HotkeyCallback, HotkeyEvent, HotkeyManager};
use crate::llm::LLMClient;
use crate::perf::{PerfHistory, PerfMetrics};
use crate::speech::AnyEngine;
use crate::speech::SpeechEngine;
use crate::state::StateMachine;
use futures_util::StreamExt;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};
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
    perf_history: tauri::State<'_, Arc<PerfHistory>>,
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
            let callback = make_hotkey_callback(sm, ac, engine, cb, ph_clone, app_clone.clone());

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
                    let fallback_callback =
                        make_hotkey_callback(sm2, ac2, engine2, cb2, ph2, app_clone.clone());
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

/// Build the hotkey callback that starts/stops recording and runs the full pipeline.
pub fn make_hotkey_callback(
    sm: Arc<Mutex<StateMachine>>,
    ac: Arc<Mutex<AudioCapture>>,
    engine: Arc<AnyEngine>,
    clipboard: Arc<Mutex<crate::clipboard::ClipboardManager>>,
    perf_history: Arc<PerfHistory>,
    app: tauri::AppHandle,
) -> HotkeyCallback {
    // Shared perf state between Pressed and Released invocations of the same cycle.
    // Pressed creates it, Released takes it and moves into the async task.
    let perf_slot: Arc<Mutex<Option<PerfMetrics>>> = Arc::new(Mutex::new(None));

    Box::new(move |event| {
        match event {
            HotkeyEvent::Pressed => {
                let t_press = Instant::now();
                let cycle_id = perf_history.next_cycle_id();

                let can_record = sm
                    .lock()
                    .map(|mut s| s.start_recording().is_ok())
                    .unwrap_or(false);
                if can_record {
                    // Show floating window.
                    if let Some(win) = app.get_webview_window("floating") {
                        let _ = win.show();
                    }
                    let _ = app.emit("recording-start", ());

                    // Start audio capture with RMS-emitting callback.
                    if let Ok(mut ac_guard) = ac.lock() {
                        let sm_for_audio = Arc::clone(&sm);
                        let app_for_rms = app.clone();
                        let _ = ac_guard.start(Box::new(move |data: &[f32]| {
                            if let Ok(mut s) = sm_for_audio.lock() {
                                let _ = s.append_audio(data);
                            }
                            let rms_val = rms::calculate_rms(data);
                            let _ = app_for_rms.emit("audio-rms", rms_val);
                        }));
                    }

                    // Store perf metrics with press timing.
                    let press_latency = t_press.elapsed().as_millis() as u64;
                    let mut perf = PerfMetrics::new(cycle_id);
                    perf.press_latency_ms = Some(press_latency);
                    if let Ok(mut slot) = perf_slot.lock() {
                        *slot = Some(perf);
                    }
                }
            }
            HotkeyEvent::Released => {
                let t_release = Instant::now();

                // Take the perf metrics created by Pressed.
                let mut perf = perf_slot
                    .lock()
                    .ok()
                    .and_then(|mut s| s.take())
                    .unwrap_or_else(|| PerfMetrics::new(perf_history.next_cycle_id()));

                perf.audio_duration_ms = Some(t_release.elapsed().as_millis() as u64);

                // Read sample rate BEFORE stop (stop clears config).
                let sample_rate = ac.lock().ok().and_then(|a| a.sample_rate());
                if let Ok(mut ac_guard) = ac.lock() {
                    ac_guard.stop();
                }
                if let Ok(mut sm_guard) = sm.lock() {
                    if let Ok(audio_data) = sm_guard.stop_recording() {
                        perf.release_latency_ms = Some(t_release.elapsed().as_millis() as u64);
                        perf.audio_samples = audio_data.len();
                        perf.audio_sample_rate = sample_rate.unwrap_or(48000);

                        // Save training data if enabled (audio first, text later).
                        let save_result = if let (Some(sr), Ok(config)) =
                            (sample_rate, AppConfig::load())
                        {
                            if config.data_saving_enabled && !config.data_saving_path.is_empty() {
                                crate::data_saving::save_audio(&audio_data, sr, &config).ok()
                            } else {
                                None
                            }
                        } else {
                            None
                        };

                        // Resample to 16kHz for Whisper.
                        let native_rate = sample_rate.unwrap_or(48000);
                        let resampled = crate::data_saving::resample(
                            &audio_data,
                            native_rate,
                            crate::data_saving::TARGET_SAMPLE_RATE,
                        );

                        // Clone references for the async task.
                        let engine_clone = engine.clone();
                        let sm_clone = sm.clone();
                        let cb_clone = clipboard.clone();
                        let app_clone = app.clone();
                        let ph_clone = perf_history.clone();
                        let t_press_for_e2e = t_release
                            - Duration::from_millis(perf.audio_duration_ms.unwrap_or(0));

                        tauri::async_runtime::spawn(async move {
                            // -- Transcription --
                            let t_transcribe = Instant::now();
                            let transcription = match engine_clone.transcribe(&resampled).await {
                                Ok(text) => {
                                    let _ = app_clone.emit("transcription-complete", &text);
                                    {
                                        if let Ok(mut s) = sm_clone.lock() {
                                            let _ = s.add_partial_result(text.clone());
                                        }
                                    }
                                    text
                                }
                                Err(e) => {
                                    let _ = app_clone.emit("speech-error", e.to_string());
                                    if let Ok(mut s) = sm_clone.lock() {
                                        s.reset();
                                    }
                                    if let Some(win) = app_clone.get_webview_window("floating") {
                                        let _ = win.hide();
                                    }
                                    return;
                                }
                            };
                            perf.transcription_ms =
                                Some(t_transcribe.elapsed().as_millis() as u64);

                            // -- LLM Correction (optional) --
                            let config = AppConfig::load().unwrap_or_default();
                            perf.llm_enabled = config.llm_enabled;
                            let final_text = if config.llm_enabled {
                                if let Ok(mut s) = sm_clone.lock() {
                                    let _ = s.start_llm_refining(transcription.clone());
                                }
                                let _ = app_clone.emit("llm-refining", ());

                                let t_llm = Instant::now();
                                let llm = LLMClient::new(
                                    config.llm_api_url,
                                    config.llm_api_key,
                                    config.llm_model,
                                );
                                let result = match llm.correct(&transcription).await {
                                    Ok(corrected) => {
                                        let _ = app_clone.emit("llm-complete", &corrected);
                                        corrected
                                    }
                                    Err(e) => {
                                        let _ = app_clone.emit("llm-error", e.to_string());
                                        transcription.clone()
                                    }
                                };
                                perf.llm_correction_ms =
                                    Some(t_llm.elapsed().as_millis() as u64);
                                result
                            } else {
                                transcription.clone()
                            };

                            // -- Injection --
                            if let Ok(mut s) = sm_clone.lock() {
                                if config.llm_enabled {
                                    let _ = s.llm_to_injecting(final_text.clone());
                                } else {
                                    let _ = s.transcribing_to_injecting(final_text.clone());
                                }
                            }

                            let t_inject = Instant::now();
                            {
                                if let Ok(mut cb) = cb_clone.lock() {
                                    let _ = cb.save();
                                    if let Err(e) = cb.inject_text(&final_text) {
                                        let _ = app_clone.emit("injection-error", e.to_string());
                                    }
                                }
                            }
                            perf.injection_ms = Some(t_inject.elapsed().as_millis() as u64);
                            perf.end_to_end_ms =
                                Some(t_press_for_e2e.elapsed().as_millis() as u64);
                            perf.text_length = final_text.len();

                            if let Ok(mut s) = sm_clone.lock() {
                                let _ = s.finish_injecting();
                            }
                            let _ = app_clone.emit("injection-complete", ());

                            // Hide floating window.
                            if let Some(win) = app_clone.get_webview_window("floating") {
                                let _ = win.hide();
                            }

                            // Update data_saving JSON with transcription.
                            if let Some(sr) = save_result {
                                let llm_text = if config.llm_enabled {
                                    Some(final_text.as_str())
                                } else {
                                    None
                                };
                                let _ = crate::data_saving::update_json_with_text(
                                    &sr.json_path,
                                    &transcription,
                                    llm_text,
                                );
                            }

                            // Record and report performance metrics.
                            ph_clone.record(perf.clone());
                            let _ = app_clone.emit("perf-metrics", &perf);
                            eprintln!("{}", perf.summary());
                        });
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

/// Return recent performance metrics history.
#[tauri::command]
pub fn get_perf_history(
    perf: tauri::State<'_, Arc<PerfHistory>>,
    n: Option<usize>,
) -> Result<Vec<PerfMetrics>, String> {
    Ok(perf.recent(n.unwrap_or(10)))
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
