pub mod audio;
pub mod clipboard;
pub mod commands;
pub mod config;
pub mod error;
pub mod hotkey;
pub mod llm;
pub mod speech;
pub mod state;
pub mod tray;

use audio::AudioCapture;
use commands::DownloadState;
use config::AppConfig;
use hotkey::HotkeyManager;
use hotkey::windows::WindowsHotkeyManager;
use state::StateMachine;
use std::sync::{Arc, Mutex};
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let state_machine = Arc::new(Mutex::new(StateMachine::new()));
    let audio_capture = Arc::new(Mutex::new(AudioCapture::new()));
    let clipboard_manager = Arc::new(Mutex::new(clipboard::ClipboardManager::new()));

    tauri::Builder::default()
        .setup(move |app| {
            tray::setup_tray(app)?;

            // Load config.
            let config = AppConfig::load().expect("failed to load config");

            // Shared state managed by Tauri.
            app.manage(state_machine.clone());
            app.manage(audio_capture.clone());
            app.manage(clipboard_manager.clone());

            // Register hotkey.
            let hotkey_name = config.hotkey.clone();
            let sm = state_machine.clone();
            let ac = audio_capture.clone();
            let mut hotkey_manager = WindowsHotkeyManager::new();
            let callback = commands::make_hotkey_callback(sm, ac);
            hotkey_manager
                .register(&hotkey_name, callback)
                .expect("failed to register hotkey");

            // Keep the hotkey manager alive for the lifetime of the app.
            app.manage(Mutex::new(hotkey_manager));

            // Download state for Whisper model downloads.
            app.manage(DownloadState::new());

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
            commands::save_settings,
            commands::test_llm_connection,
            commands::get_whisper_models,
            commands::download_whisper_model,
            commands::cancel_download,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
