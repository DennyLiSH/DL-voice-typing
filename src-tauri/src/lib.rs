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
pub mod realtime;
pub mod speech;
pub mod state;
pub mod tray;
pub mod util;
pub mod watchdog;
pub mod win32;

use audio::AudioCapture;
use clipboard::AnyClipboard;
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
use tauri::{Emitter, Manager};
use time::UtcOffset;
use time::format_description::well_known::Rfc3339;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::time::OffsetTime;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_logging();

    let state_machine = Arc::new(Mutex::new(StateMachine::new()));
    let audio_capture = Arc::new(Mutex::new(AudioCapture::new()));
    let clipboard_manager = Arc::new(Mutex::new(clipboard::AnyClipboard::Windows(
        clipboard::ClipboardManager::new(),
    )));
    let perf_history = Arc::new(PerfHistory::new());
    let cached_llm: Arc<Mutex<Option<crate::llm::AnyCorrector>>> = Arc::new(Mutex::new(None));
    let shutting_down = Arc::new(AtomicBool::new(false));

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            setup_tray_and_plugins(app)?;
            let config = load_and_manage_config(app.handle());
            let engine = init_and_manage_engine(app.handle(), &config);
            spawn_model_loading(engine, app.handle().clone());
            create_overlay_windows(app)?;
            manage_pipeline_state(
                app.handle(),
                state_machine.clone(),
                audio_capture.clone(),
                clipboard_manager.clone(),
                perf_history.clone(),
                shutting_down.clone(),
                cached_llm.clone(),
            );
            let hotkey_manager = register_hotkey(app.handle(), &config);
            app.manage(Mutex::new(hotkey_manager));
            app.manage(DownloadState::new());
            app.manage(commands::PendingReview::new());
            start_watchdog(app.handle(), state_machine.clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::config_cmd::get_config,
            commands::config_cmd::save_settings,
            commands::misc_cmd::test_llm_connection,
            commands::download::get_whisper_models,
            commands::download::download_whisper_model,
            commands::download::cancel_download,
            commands::download::delete_custom_model,
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

// ---------------------------------------------------------------------------
// Setup stage functions — each handles one logical concern of app bootstrap.
// ---------------------------------------------------------------------------

/// Initialize structured logging to file (%APPDATA%\dl-voice-typing\logs\).
fn init_logging() {
    let log_dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("dl-voice-typing")
        .join("logs");

    let file_appender = tracing_appender::rolling::RollingFileAppender::builder()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("dl-voice-typing")
        .filename_suffix("log")
        .build(&log_dir)
        .expect("failed to initialize log file appender");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_timer(OffsetTime::new(
            UtcOffset::current_local_offset().expect("failed to get local time offset"),
            Rfc3339,
        ))
        .init();
}

/// Setup tray icon and register autostart plugin.
fn setup_tray_and_plugins(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    tray::setup_tray(app)?;

    #[cfg(desktop)]
    app.handle().plugin(tauri_plugin_autostart::init(
        tauri_plugin_autostart::MacosLauncher::LaunchAgent,
        None,
    ))?;

    Ok(())
}

/// Load config from disk, manage the in-memory cache, and sync autostart.
fn load_and_manage_config(app: &tauri::AppHandle) -> AppConfig {
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

    config
}

/// Create the speech engine, manage it in Tauri state, and return it.
fn init_and_manage_engine(app: &tauri::AppHandle, config: &AppConfig) -> Arc<Mutex<AnyEngine>> {
    let engine = {
        #[cfg(feature = "whisper")]
        {
            let model_path = config::model_path_for_size(&config.whisper_model);
            AnyEngine::new_whisper(model_path, config.language)
        }
        #[cfg(not(feature = "whisper"))]
        {
            AnyEngine::new_mock("[mock transcription]")
        }
    };
    let engine = Arc::new(Mutex::new(engine));
    app.manage(engine.clone());
    engine
}

/// Load the Whisper model in a background thread so the UI stays responsive.
fn spawn_model_loading(engine: Arc<Mutex<AnyEngine>>, app_handle: tauri::AppHandle) {
    tauri::async_runtime::spawn_blocking(move || {
        info!("background model loading started");
        if let Some(mut e) = util::lock_mutex(&engine, "engine") {
            if let Err(e) = e.load_model() {
                warn!(
                    "model load failed: {e}. This may be due to missing GPU drivers or a corrupted model file."
                );
            }
        }
        let _ = app_handle.emit("model-loaded", ());
        info!("background model loading finished");
    });
}

/// Create the floating indicator and review windows (both hidden by default).
fn create_overlay_windows(app: &mut tauri::App) -> Result<(), tauri::Error> {
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

    Ok(())
}

/// Manage all shared state that the pipeline and commands depend on.
fn manage_pipeline_state(
    app: &tauri::AppHandle,
    state_machine: Arc<Mutex<StateMachine>>,
    audio_capture: Arc<Mutex<AudioCapture>>,
    clipboard_manager: Arc<Mutex<AnyClipboard>>,
    perf_history: Arc<PerfHistory>,
    shutting_down: Arc<AtomicBool>,
    cached_llm: Arc<Mutex<Option<crate::llm::AnyCorrector>>>,
) {
    app.manage(state_machine);
    app.manage(audio_capture);
    app.manage(clipboard_manager);
    app.manage(perf_history);
    app.manage(shutting_down);
    app.manage(cached_llm);
    app.manage(Arc::new(Mutex::new(None::<realtime::RealtimeTranscriber>)));
}

/// Register the global hotkey from config. Warns but does not fail on error.
fn register_hotkey(app: &tauri::AppHandle, config: &AppConfig) -> WindowsHotkeyManager {
    let hotkey_name = config.hotkey.clone();
    let mut hotkey_manager = WindowsHotkeyManager::new();
    let callback =
        commands::make_hotkey_callback(commands::pipeline_state::PipelineState::from_app(app));
    if let Err(e) = hotkey_manager.register(&hotkey_name, callback) {
        warn!("failed to register hotkey '{hotkey_name}': {e}");
    }
    hotkey_manager
}

/// Start the background watchdog thread that monitors state machine health.
fn start_watchdog(app: &tauri::AppHandle, state_machine: Arc<Mutex<StateMachine>>) {
    let watchdog_recovery = Arc::new(crate::watchdog::TauriRecoveryActions::new(app.clone()));
    std::thread::spawn(move || {
        let wd = crate::watchdog::Watchdog::new(
            state_machine,
            watchdog_recovery,
            std::time::Duration::from_secs(10),
            std::time::Duration::from_secs(30),
        );
        wd.run();
    });
}

#[cfg(test)]
mod tests {
    mod pipeline_test;
}
