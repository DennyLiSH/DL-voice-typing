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
pub mod streaming_recorder;
pub mod tray;
pub mod util;
pub mod watchdog;
pub mod win32;

use audio::{AudioCapture, AudioCaptureProvider};
use clipboard::{ClipboardManager, ClipboardProvider};
use commands::DownloadState;
use config::{AppConfig, ConfigCache};
use hotkey::HotkeyManager;
use hotkey::windows::WindowsHotkeyManager;
use perf::PerfHistory;
use speech::SpeechEngine;
#[cfg(feature = "whisper")]
use speech::whisper_factory::WhisperEngineFactory;
use state::StateMachine;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
#[cfg(feature = "whisper")]
use tauri::Emitter;
use tauri::Manager;
use time::UtcOffset;
use time::format_description::well_known::Rfc3339;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::time::OffsetTime;

/// Application entry point: initializes logging, state, engine, windows, hotkey, and watchdog,
/// then runs the Tauri event loop.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _log_guard = init_logging();

    let state_machine = Arc::new(Mutex::new(StateMachine::new()));
    let audio_capture: Arc<Mutex<dyn AudioCaptureProvider>> =
        Arc::new(Mutex::new(AudioCapture::new()));
    let clipboard_manager: Arc<dyn ClipboardProvider> = Arc::new(ClipboardManager::new());
    let perf_history = Arc::new(PerfHistory::new());
    let cached_llm: Arc<Mutex<Option<Box<dyn crate::llm::TextCorrector>>>> =
        Arc::new(Mutex::new(None));
    let shutting_down = Arc::new(AtomicBool::new(false));

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            setup_tray_and_plugins(app)?;
            let config = load_and_manage_config(app.handle());
            // Salvage crash-truncated record-only WAVs (rewrite headers from
            // actual file lengths). Runs before engine init; skipped when the
            // recording directory is unset or missing.
            if !config.data_saving_path.trim().is_empty() {
                streaming_recorder::salvage_incomplete_recordings(std::path::Path::new(
                    &config.data_saving_path,
                ));
            }
            let _engine = init_and_manage_engine(app.handle(), &config);
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
            app.manage(commands::transcribe_cmd::PendingTranscribe::new());
            start_watchdog(app.handle(), state_machine.clone());
            maybe_open_settings_on_missing_model(app.handle(), &config);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::config_cmd::get_config,
            commands::config_cmd::save_settings,
            commands::config_cmd::is_autostart_available,
            commands::config_cmd::get_compute_mode,
            commands::download::get_whisper_models,
            commands::download::download_whisper_model,
            commands::download::cancel_download,
            commands::download::delete_custom_model,
            commands::llm_cmd::test_llm_connection,
            commands::log_cmd::log_frontend_error,
            commands::perf_cmd::get_perf_history,
            commands::data_management_cmd::list_saved_recordings,
            commands::data_management_cmd::delete_recording,
            commands::data_management_cmd::delete_recordings,
            commands::data_management_cmd::get_data_usage,
            commands::data_management_cmd::read_recording_audio,
            commands::review::confirm_inject,
            commands::review::cancel_review,
            commands::review::get_review_text,
            commands::transcribe_cmd::open_transcribe_window,
            commands::transcribe_cmd::transcribe_recording,
            commands::transcribe_cmd::cancel_transcription,
            commands::transcribe_cmd::get_recording_segments,
            commands::transcribe_cmd::get_inject_target,
            commands::transcribe_cmd::inject_transcript_text,
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                use tauri::Manager;
                let shutting_down = window.state::<Arc<AtomicBool>>();
                if shutting_down.load(Ordering::SeqCst) {
                    return; // allow close during shutdown
                }
                api.prevent_close();
                if window.label() == "transcribe" {
                    // Cancel any in-flight transcription and clear the
                    // take-once HWND so later injects are rejected until the
                    // window is reopened.
                    let pt = window.state::<commands::transcribe_cmd::PendingTranscribe>();
                    commands::transcribe_cmd::on_transcribe_window_closed(&pt);
                }
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

/// Whether autostart registry operations should run.
/// - Release builds: always true.
/// - Debug builds: only when DL_AUTOSTART=1 env var is set.
fn should_manage_autostart() -> bool {
    if cfg!(debug_assertions) {
        std::env::var("DL_AUTOSTART").as_deref() == Ok("1")
    } else {
        true
    }
}

/// Initialize structured logging to file (%APPDATA%\dl-voice-typing\logs\).
/// Returns the WorkerGuard which MUST be held for the app lifetime — dropping it
/// kills the background writer thread and silently discards all log output.
fn init_logging() -> tracing_appender::non_blocking::WorkerGuard {
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
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

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

    guard
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

    let config_cache = ConfigCache::new(config.clone());
    app.manage(config_cache);

    // Sync autostart registry with config preference.
    #[cfg(desktop)]
    {
        if should_manage_autostart() {
            use tauri_plugin_autostart::ManagerExt;
            let manager = app.autolaunch();
            if config.autostart {
                if let Err(e) = manager.enable() {
                    warn!("failed to enable autostart: {e}");
                }
            } else if let Err(e) = manager.disable() {
                warn!("failed to disable autostart: {e}");
            }
        } else {
            info!("autostart management skipped (debug build without DL_AUTOSTART=1)");
        }
    }

    config
}

/// Create the speech engine, manage it in Tauri state, and return it.
fn init_and_manage_engine(app: &tauri::AppHandle, _config: &AppConfig) -> Arc<dyn SpeechEngine> {
    #[cfg(feature = "whisper")]
    {
        let config = _config;
        let model_path = config::model_path_for_size(&config.whisper_model);
        let whisper_engine = WhisperEngineFactory::create(model_path, config.language);
        let engine: Arc<dyn SpeechEngine> = whisper_engine.clone();
        app.manage(engine.clone());
        spawn_model_loading(whisper_engine, app.clone());
        engine
    }
    #[cfg(not(feature = "whisper"))]
    {
        let engine: Arc<dyn SpeechEngine> = Arc::new(speech::noop::NoopEngine::new());
        app.manage(engine.clone());
        engine
    }
}

/// Load the Whisper model in a background thread so the UI stays responsive.
#[cfg(feature = "whisper")]
fn spawn_model_loading(
    engine: Arc<crate::speech::whisper::WhisperEngine>,
    app_handle: tauri::AppHandle,
) {
    tauri::async_runtime::spawn_blocking(move || {
        info!("background model loading started");
        if let Err(e) = engine.load_model() {
            warn!(
                "model load failed: {e}. This may be due to missing GPU drivers or a corrupted model file."
            );
            // Surface the failure on the tray. Deliberately via the raw
            // app handle (not PipelineState): manage_pipeline_state runs
            // later in setup, so state access here could panic — and
            // panic=abort would kill the whole process.
            if let Some(tray) = app_handle.tray_by_id("default") {
                let _ = tray.set_tooltip(Some("语文兔 - 模型加载失败"));
            }
        }
        let _ = app_handle.emit("model-loaded", ());
        info!("background model loading finished");
    });
}

/// First-run dead-end guard: when the selected Whisper model file is missing,
/// open the settings window straight at the model page so a fresh install
/// lands on the download UI instead of hitting the misleading "loading"
/// error on the first hotkey press. Runs off the setup thread so window
/// creation cannot block app startup.
fn maybe_open_settings_on_missing_model(app: &tauri::AppHandle, config: &AppConfig) {
    #[cfg(feature = "whisper")]
    {
        if config::model_path_for_size(&config.whisper_model).exists() {
            return;
        }
        info!("whisper model file missing; opening settings at the model page");
        let app = app.clone();
        std::thread::spawn(move || open_settings_at_model_page(&app));
    }
    #[cfg(not(feature = "whisper"))]
    {
        let _ = (app, config);
    }
}

/// Show the settings window (creating it on first open, mirroring the tray
/// menu handler) and navigate to the model sub-page. The navigation eval is
/// attached via `on_page_load(Finished)` on the fresh window — evaluating
/// during setup would race the page load and silently no-op. `.catch` keeps
/// genuine failures degraded to the default general page, never a crash.
fn open_settings_at_model_page(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("settings") {
        // Already opened once (and navigated to the model page then) — keep
        // whatever page the user is on now.
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }
    let built = tauri::webview::WebviewWindowBuilder::new(
        app,
        "settings",
        tauri::WebviewUrl::App("settings.html".into()),
    )
    .title("语文兔语音输入法 - 设置")
    .inner_size(560.0, 620.0)
    .resizable(true)
    .center()
    .visible(false)
    .background_color(tauri::webview::Color(0xFA, 0xFA, 0xF8, 0xFF))
    .on_page_load(|window, payload| {
        if payload.event() == tauri::webview::PageLoadEvent::Finished {
            let _ = window
                .eval("import('./app-shell.js').then(m => m.switchPage('model')).catch(() => {})");
        }
    })
    .build();
    match built {
        Ok(window) => {
            let _ = window.show();
        }
        Err(e) => warn!("failed to open settings for missing model: {e}"),
    }
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
    .title("确认粘贴")
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

    let _transcribe = tauri::webview::WebviewWindowBuilder::new(
        app,
        "transcribe",
        tauri::WebviewUrl::App("transcribe.html".into()),
    )
    .title("录音转录")
    .inner_size(720.0, 560.0)
    .resizable(true)
    .visible(false)
    .center()
    .build()?;

    Ok(())
}

/// Manage all shared state that the pipeline and commands depend on.
fn manage_pipeline_state(
    app: &tauri::AppHandle,
    state_machine: Arc<Mutex<StateMachine>>,
    audio_capture: Arc<Mutex<dyn AudioCaptureProvider>>,
    clipboard_manager: Arc<dyn ClipboardProvider>,
    perf_history: Arc<PerfHistory>,
    shutting_down: Arc<AtomicBool>,
    cached_llm: Arc<Mutex<Option<Box<dyn crate::llm::TextCorrector>>>>,
) {
    app.manage(state_machine);
    app.manage(audio_capture);
    app.manage(clipboard_manager);
    app.manage(perf_history);
    app.manage(shutting_down);
    app.manage(cached_llm);
    app.manage(Arc::new(Mutex::new(None::<realtime::RealtimeTranscriber>)));
    // Register PipelineState as the canonical command-facing aggregate so that
    // review commands and future commands no longer lock StateMachine directly.
    app.manage(commands::pipeline_state::PipelineState::from_app(app));
}

/// Register the global hotkey from config. Warns but does not fail on error.
///
/// Uses the SAME managed PipelineState instance as review commands — calling
/// `PipelineState::from_app` here would construct a second instance with an
/// independent `DeliveryController.context`, breaking the show_review →
/// confirm/cancel context flow (stored on one instance, taken on the other).
/// Must be called after `manage_pipeline_state` (which manages PipelineState).
fn register_hotkey(app: &tauri::AppHandle, config: &AppConfig) -> WindowsHotkeyManager {
    let hotkey_name = config.hotkey.clone();
    let mut hotkey_manager = WindowsHotkeyManager::new();
    let ps = app
        .state::<commands::pipeline_state::PipelineState>()
        .inner()
        .clone();
    let callback = commands::make_hotkey_callback(ps.clone());
    if let Err(e) = hotkey_manager.register(&hotkey_name, callback) {
        warn!("failed to register hotkey '{hotkey_name}': {e}");
    }
    if config.record_only_enabled {
        let callback = commands::record_only_session::make_record_only_callback(ps);
        if let Err(e) = hotkey_manager.register_record_only(&config.record_only_hotkey, callback) {
            warn!(
                "failed to register record-only hotkey '{}': {e}",
                config.record_only_hotkey
            );
        }
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
    mod data_management_test;
    mod delivery_controller_test;
    mod pipeline_integration_test;
    mod pipeline_lifecycle_test;
    mod pipeline_test;
    mod recording_session_test;
}
