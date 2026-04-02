pub mod audio;
pub mod clipboard;
pub mod commands;
pub mod config;
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
use config::AppConfig;
use hotkey::HotkeyManager;
use hotkey::windows::WindowsHotkeyManager;
use perf::PerfHistory;
use speech::AnyEngine;
use state::StateMachine;
use std::sync::{Arc, Mutex};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let state_machine = Arc::new(Mutex::new(StateMachine::new()));
    let audio_capture = Arc::new(Mutex::new(AudioCapture::new()));
    let clipboard_manager = Arc::new(Mutex::new(clipboard::ClipboardManager::new()));
    let perf_history = Arc::new(PerfHistory::new());

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            tray::setup_tray(app)?;

            // Load config.
            let config = AppConfig::load().expect("failed to load config");

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
                eprintln!(
                    "Warning: model load failed: {}. Download a model from Settings.",
                    e
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
            .title("DL Voice Typing")
            .inner_size(120.0, 120.0)
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

            // Register hotkey.
            let hotkey_name = config.hotkey.clone();
            let sm = state_machine.clone();
            let ac = audio_capture.clone();
            let cb = clipboard_manager.clone();
            let ph = perf_history.clone();
            let app_handle = app.handle().clone();
            let mut hotkey_manager = WindowsHotkeyManager::new();
            let callback = commands::make_hotkey_callback(sm, ac, engine, cb, ph, app_handle);
            hotkey_manager
                .register(&hotkey_name, callback)
                .expect("failed to register hotkey");

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
            commands::get_config,
            commands::save_settings,
            commands::test_llm_connection,
            commands::get_whisper_models,
            commands::download_whisper_model,
            commands::cancel_download,
            commands::get_perf_history,
            commands::confirm_inject,
            commands::cancel_review,
            commands::get_review_text,
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
