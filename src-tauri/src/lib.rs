pub mod audio;
pub mod clipboard;
pub mod config;
pub mod error;
pub mod hotkey;
pub mod llm;
pub mod speech;
pub mod state;
pub mod tray;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            tray::setup_tray(app)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
