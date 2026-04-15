pub mod audio;
pub mod clipboard;
pub mod commands;
pub mod config;
pub mod crypto;
pub mod data_saving;
pub mod error;
pub mod hotkey;
pub mod llm;
pub mod perf;
pub mod speech;
pub mod state;
pub mod tray;

use audio::AudioCapture;
use commands::DownloadState;
use config::{AppConfig, ConfigCache};
use hotkey::HotkeyManager;
use hotkey::windows::WindowsHotkeyManager;
use perf::PerfHistory;
use speech::AnyEngine;
use state::StateMachine;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tracing::warn;
use tracing_subscriber::EnvFilter;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Initialize structured logging to file (%APPDATA%\dl-voice-typing\logs\).
    let log_dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("dl-voice-typing")
        .join("logs");

    let file_appender = tracing_appender::rolling::daily(log_dir, "dl-voice-typing.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(non_blocking)
        .with_ansi(false)
        .init();

    let state_machine = Arc::new(Mutex::new(StateMachine::new()));
    let audio_capture = Arc::new(Mutex::new(AudioCapture::new()));
    let clipboard_manager = Arc::new(Mutex::new(clipboard::ClipboardManager::new()));
    let perf_history = Arc::new(PerfHistory::new());
    let shutting_down = Arc::new(AtomicBool::new(false));

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            tray::setup_tray(app)?;

            #[cfg(desktop)]
            app.handle().plugin(tauri_plugin_autostart::init(
                tauri_plugin_autostart::MacosLauncher::LaunchAgent,
                None,
            ))?;

            // Load config.
            let config = match AppConfig::load() {
                Ok(c) => c,
                Err(e) => {
                    warn!("failed to load config, using defaults: {e}");
                    AppConfig::default()
                }
            };

            let config_cache = ConfigCache::new(std::sync::RwLock::new(config.clone()));
            app.manage(config_cache);

            // Sync autostart registry with config preference.
            #[cfg(desktop)]
            {
                use tauri_plugin_autostart::ManagerExt;
                let manager = app.autolaunch();
                if config.autostart {
                    if let Err(e) = manager.enable() {
                        warn!("failed to enable autostart: {e}");
                    }
                } else if let Err(e) = manager.disable() {
                    warn!("failed to disable autostart: {e}");
                }
            }

            // Initialize speech engine.
            let mut engine = {
                #[cfg(feature = "whisper")]
                {
                    let model_path = config::model_path_for_size(&config.whisper_model);
                    AnyEngine::new_whisper(model_path, config.language.clone())
                }
                #[cfg(not(feature = "whisper"))]
                {
                    AnyEngine::new_mock("[mock transcription]")
                }
            };
            if let Err(e) = engine.load_model() {
                warn!(
                    "model load failed: {e}. This may be due to missing GPU drivers or a corrupted model file."
                );
            }
            let engine = Arc::new(engine);
            app.manage(engine.clone());

            // Create floating window (hidden by default).
            let _floating = tauri::webview::WebviewWindowBuilder::new(
                app,
                "floating",
                tauri::WebviewUrl::App("floating.html".into()),
            )
            .title("语文兔语音输入法")
            .inner_size(180.0, 180.0)
            .decorations(false)
            .transparent(true)
            .shadow(false)
            .always_on_top(true)
            .focusable(false)
            .skip_taskbar(true)
            .visible(false)
            .center()
            .build()?;

            // Pre-create review window (hidden) for fast show on demand.
            let _review = tauri::webview::WebviewWindowBuilder::new(
                app,
                "review",
                tauri::WebviewUrl::App("review.html".into()),
            )
            .title("Review")
            .inner_size(420.0, 220.0)
            .decorations(false)
            .transparent(true)
            .shadow(false)
            .always_on_top(true)
            .focusable(true)
            .skip_taskbar(true)
            .visible(false)
            .center()
            .build()?;

            // Shared state managed by Tauri.
            app.manage(state_machine.clone());
            app.manage(audio_capture.clone());
            app.manage(clipboard_manager.clone());
            app.manage(perf_history.clone());
            app.manage(shutting_down.clone());

            // Register hotkey.
            let hotkey_name = config.hotkey.clone();
            let sm = state_machine.clone();
            let ac = audio_capture.clone();
            let cb = clipboard_manager.clone();
            let ph = perf_history.clone();
            let app_handle = app.handle().clone();
            let mut hotkey_manager = WindowsHotkeyManager::new();
            let cc = app.state::<ConfigCache>().inner().clone();
            let callback = commands::make_hotkey_callback(sm, ac, engine, cb, ph, app_handle, cc);
            if let Err(e) = hotkey_manager.register(&hotkey_name, callback) {
                warn!("failed to register hotkey '{hotkey_name}': {e}");
            }

            // Keep the hotkey manager alive for the lifetime of the app.
            use tauri::Manager;
            app.manage(Mutex::new(hotkey_manager));

            // Download state for Whisper model downloads.
            app.manage(DownloadState::new());

            // Pending review text for the review window to fetch on load.
            app.manage(commands::PendingReview::new());

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::config_cmd::get_config,
            commands::config_cmd::save_settings,
            commands::misc_cmd::test_llm_connection,
            commands::download::get_whisper_models,
            commands::download::download_whisper_model,
            commands::download::cancel_download,
            commands::misc_cmd::get_perf_history,
            commands::misc_cmd::get_compute_mode,
            commands::review::confirm_inject,
            commands::review::cancel_review,
            commands::review::get_review_text,
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                use tauri::Manager;
                let shutting_down = window.state::<Arc<AtomicBool>>();
                if shutting_down.load(Ordering::SeqCst) {
                    return; // allow close during shutdown
                }
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .inspect_err(|e| tracing::error!("fatal: error running application: {e}"))
        .ok();
}
