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
pub mod recording_name;
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
///
/// # 启动拓扑序 / startup topological order
///
/// setup 闭包内的初始化步骤存在严格先后依赖：部分是硬约束（乱序会 panic
/// 或破坏首建语义），部分是刻意的惯例位置。规范顺序与各步位置的理由如下；
/// `startup_order_ratchet` 测试以源码刮取钉住其中的关键先后——改动顺序须
/// 同步改契约块、本测试，以及下列各点的一行指引：
///
/// 1. **config load** — ConfigCache 是后续每步的输入源（salvage/sweep 读
///    data_saving_path，engine init 读模型/语言，hotkey 读键位）。
/// 2. **salvage/sweep**（data_saving_path 为空则跳过）— 崩溃恢复必须先于
///    engine init：截断的 record-only WAV 要在引擎开始使用录音目录前修复归位。
/// 3. **engine init** — 依赖 1；后台模型加载线程此刻拿不到 PipelineState
///    （尚未 manage，见 spawn_model_loading 一行注），失败经裸 AppHandle
///    落托盘 tooltip。
/// 4. **overlay windows** — floating/review/transcribe 创建（默认隐藏）。
///    与 manage 步骤无硬依赖，刻意先行：之后 watchdog 复位 / 错误收尾的
///    hide 不会碰到窗体缺失。
/// 5. **ErrorHistory manage** — 硬约束：必须先于 manage_pipeline_state，
///    from_app 首建 RecordingEmitter 装饰器时 try_state 必须能找到它。
/// 6. **PendingDeletes manage** — 软删除 5s 撤销窗口的注册表；先于任何
///    可能软删除/撤销的组件（mistouch 调度器闭包运行时取它）。
/// 7. **mistouch scheduler** — 依赖 6；先于 hotkey register：热键注册后
///    record-only 会话才可能派发 mistouch。
/// 8. **manage_pipeline_state** — 命令面唯一聚合根。硬约束：5 先行；
///    hotkey register / watchdog 的 state 提取依赖它，乱序即 panic。
/// 9. **hotkey register** — 回调提取 PipelineState，必须在 8 之后。
/// 10. **cancel-esc register** — dispatcher 必须抵达已安装的 hook proc，
///     故必须在 9 之后。
/// 11. **watchdog** — 提取 PipelineState（8 之后）；最后启动监控，只面对
///     初始化完成后的状态机。
/// 12. **first-run guard** — 模型缺失时开设置窗直达模型页；兜底性质，
///     放最后：要求其余步骤全部完成。
///
/// 其余 manage（hotkey manager / DownloadState / PendingReview /
/// PendingTranscribe）穿插于 9–11 之间，彼此无先后约束。
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _log_guard = init_logging();

    let state_machine = Arc::new(Mutex::new(StateMachine::new()));
    let audio_capture: Arc<Mutex<dyn AudioCaptureProvider>> =
        Arc::new(Mutex::new(AudioCapture::new()));
    let clipboard_manager: Arc<dyn ClipboardProvider> = Arc::new(ClipboardManager::new());
    let perf_history = Arc::new(PerfHistory::new());
    let cached_llm: Arc<Mutex<Option<Arc<dyn crate::llm::TextCorrector>>>> =
        Arc::new(Mutex::new(None));
    let shutting_down = Arc::new(AtomicBool::new(false));

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(move |app| {
            setup_tray_and_plugins(app)?;
            let config = load_and_manage_config(app.handle());
            // Crash salvage + pending sweep, before engine init — ordering contract: run() doc「启动拓扑序」.
            if !config.data_saving_path.trim().is_empty() {
                streaming_recorder::salvage_incomplete_recordings(std::path::Path::new(
                    &config.data_saving_path,
                ));
                // Sweep residual .dl_pending/ files from a previous crashed
                // or killed process. The 5s undo window is gone — these are
                // either successfully-restored (file not present) or stale.
                commands::data_management_cmd::sweep_pending_dir(&config);
            }
            let _engine = init_and_manage_engine(app.handle(), &config);
            create_overlay_windows(app)?;
            // Managed BEFORE manage_pipeline_state (RecordingEmitter decorator, first build) — ordering contract: run() doc「启动拓扑序」.
            app.manage(Arc::new(commands::error_history::ErrorHistory::new()));
            // PendingDeletes registry for the soft-delete 5s undo window
            // (P2 用户控制权专项). Process-lifetime; the timer threads hold
            // Arc clones.
            app.manage(std::sync::Arc::new(
                commands::data_management_cmd::PendingDeletes::default(),
            ));
            // M2-a mistouch scheduling (non-IPC; see record_only_session.rs
            // MISTOUCH_SCHEDULER doc for why this is not an event listener).
            commands::record_only_session::set_mistouch_scheduler(make_mistouch_scheduler(
                app.handle().clone(),
            ));
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
            start_watchdog(app.handle());
            maybe_open_settings_on_missing_model(app.handle(), &config);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::config_cmd::get_config,
            commands::config_cmd::get_default_config,
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
            commands::error_history::get_last_errors,
            commands::data_management_cmd::list_saved_recordings,
            commands::data_management_cmd::soft_delete_recordings,
            commands::data_management_cmd::restore_pending_delete,
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
            // Surface the failure on the tray via the raw app handle (not PipelineState — managed later per run() doc「启动拓扑序」): state access here could panic, and panic=abort kills the whole process.
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
    match crate::tray::build_settings_window(
        app,
        tauri::WebviewUrl::App("settings.html?model_missing=1".into()),
        Some("import('./app-shell.js').then(m => m.switchPage('model')).catch(() => {})"),
    ) {
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

/// M2-a mistouch scheduler factory (non-IPC; see record_only_session.rs
/// MISTOUCH_SCHEDULER doc for why this is not an event listener).
/// Soft-deletes the mistouch WAV into the pending dir and wires the tray
/// undo item; the finalize callback clears the tray entry when the 5s
/// undo window ends. Stays in lib.rs because it assembles tray +
/// PendingDeletes + ConfigCache — pure composition, no logic of its own
/// beyond the dir-mismatch defense.
fn make_mistouch_scheduler(
    app_h: tauri::AppHandle,
) -> commands::record_only_session::MistouchScheduler {
    std::sync::Arc::new(move |stem: &str, dir: &str| {
        let pending = app_h
            .state::<std::sync::Arc<commands::data_management_cmd::PendingDeletes>>()
            .inner()
            .clone();
        // Defense-in-depth: the scheduler is process-internal (no IPC
        // surface), but still pin the dir to the CURRENT configured data
        // dir so no future caller can point it elsewhere.
        let cfg_dir = {
            let cfg = app_h.state::<crate::config::ConfigCache>();
            cfg.read_cached().data_saving_path.clone()
        };
        if dir != cfg_dir {
            tracing::warn!(
                "mistouch dispatch: dir mismatch (payload != config data dir); ignoring"
            );
            return;
        }
        let base = std::path::PathBuf::from(dir);
        let filenames = vec![stem.to_string()];
        let app_for_cb = app_h.clone();
        let outcome = commands::data_management_cmd::soft_delete_files(&base, &filenames);
        if outcome.moved == 0 {
            // Known limitation (registered below): the floating copy already
            // said "已自动丢弃" — a failed move leaves an orphan WAV in the
            // data dir with no JSON. Rare disk-failure path; log-visible only.
            tracing::warn!(
                "mistouch discard: no file moved for {stem} dir={dir} (failed_count={})",
                outcome.failed.len()
            );
            return;
        }
        let id = pending.schedule_with_finalize(
            outcome.pairs,
            Some(std::sync::Arc::new(move |_batch_id| {
                crate::tray::set_mistouch_undo(&app_for_cb, None);
            })),
        );
        crate::tray::set_mistouch_undo(&app_h, Some(id));
    })
}

/// Manage all shared state that the pipeline and commands depend on.
fn manage_pipeline_state(
    app: &tauri::AppHandle,
    state_machine: Arc<Mutex<StateMachine>>,
    audio_capture: Arc<Mutex<dyn AudioCaptureProvider>>,
    clipboard_manager: Arc<dyn ClipboardProvider>,
    perf_history: Arc<PerfHistory>,
    shutting_down: Arc<AtomicBool>,
    cached_llm: Arc<Mutex<Option<Arc<dyn crate::llm::TextCorrector>>>>,
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
/// Must be called after manage_pipeline_state — ordering contract: run() doc「启动拓扑序」.
fn register_hotkey(app: &tauri::AppHandle, config: &AppConfig) -> WindowsHotkeyManager {
    let mut hotkey_manager = WindowsHotkeyManager::new();
    let ps = app
        .state::<commands::pipeline_state::PipelineState>()
        .inner()
        .clone();
    let callback = commands::make_hotkey_callback(ps.clone());
    // Stage 6 fix #2 — preserve load-path observability. The object-form
    // HotkeySpec accepts any u32 vk on load; warn here if vk_to_key_name
    // round-trip fails so the spec is a known "dead" hotkey.
    if !crate::hotkey::is_resolvable_vk(config.hotkey.vk) {
        warn!(
            "config hotkey vk={:#x} has no name representation; keydown will not match",
            config.hotkey.vk
        );
    }
    if let Err(e) = hotkey_manager.register(config.hotkey, callback) {
        warn!(
            "failed to register hotkey '{}': {e}",
            config.hotkey.display()
        );
    }
    // P1 Esc-cancel: the hook must already be installed (above) so the dispatcher reaches the hook proc — ordering contract: run() doc「启动拓扑序」.
    {
        let ps_cancel = ps.clone();
        if let Err(e) =
            hotkey_manager.register_cancel_esc(Box::new(move || ps_cancel.cancel_active_pipeline()))
        {
            warn!("failed to register cancel-esc slot: {e}");
        }
    }
    if config.record_only_enabled {
        let callback = commands::record_only_session::make_record_only_callback(ps);
        if let Err(e) = hotkey_manager.register_record_only(config.record_only_hotkey, callback) {
            warn!(
                "failed to register record-only hotkey '{}': {e}",
                config.record_only_hotkey.display()
            );
        }
    }

    // M6-a optional open-windows slots (default: not registered).
    if let Some(spec) = config.open_settings_hotkey {
        let cb = commands::make_open_window_callback(app.clone(), commands::WindowKind::Settings);
        if let Err(e) = hotkey_manager.register_open_settings(spec, cb) {
            warn!(
                "failed to register open-settings hotkey '{}': {e}",
                spec.display()
            );
        }
    }
    if let Some(spec) = config.open_transcribe_hotkey {
        let cb = commands::make_open_window_callback(app.clone(), commands::WindowKind::Transcribe);
        if let Err(e) = hotkey_manager.register_open_transcribe(spec, cb) {
            warn!(
                "failed to register open-transcribe hotkey '{}': {e}",
                spec.display()
            );
        }
    }
    hotkey_manager
}

/// Start the background watchdog thread that monitors pipeline-state health.
///
/// The watchdog holds an `Arc<PipelineState>` clone (all fields are shared
/// Arcs, so the clone observes the same components — same `.inner().clone()`
/// extraction shape as config_cmd's rebuild path). The managed PipelineState
/// itself is not re-managed or rewrapped; the three existing
/// `app.state::<PipelineState>()` extraction sites stay untouched.
fn start_watchdog(app: &tauri::AppHandle) {
    let ps = Arc::new(
        app.state::<commands::pipeline_state::PipelineState>()
            .inner()
            .clone(),
    );
    std::thread::spawn(move || {
        let wd = crate::watchdog::Watchdog::new(
            ps,
            std::time::Duration::from_secs(10),
            std::time::Duration::from_secs(30),
        );
        wd.run();
    });
}

#[cfg(test)]
mod tests {
    pub(crate) mod policy_audit;

    mod data_management_test;
    mod delivery_controller_test;
    mod pipeline_integration_test;
    mod pipeline_lifecycle_test;
    mod pipeline_test;
    mod recording_session_test;

    /// 启动拓扑序棘轮（Round 6 P8 Task 2）：include_str! 刮取 lib.rs 自身
    /// 源码，断言 setup 启动调用锚点的字节偏移满足规范先后。锚点 = 唯一
    /// 语句标记（调用点，非行号；`fn` 定义与测试模块自身不参与匹配），
    /// rustfmt 重排语句内部布局不敏感，真正的步骤重排会在此变红。
    /// 各步位置的理由见 run() doc「启动拓扑序」契约块——改顺序须同步改
    /// 契约块与本测试的锚点/配对。
    #[test]
    fn startup_order_ratchet() {
        const RUN_SRC: &str = include_str!("lib.rs");

        // Unique statement markers (call sites, not `fn` definitions). If a
        // marker ever goes stale (rustfmt reflow / refactor), update it
        // together with the contract block on run().
        const ERR_HISTORY: &str = "commands::error_history::ErrorHistory::new";
        const MPS_CALL: &str = "manage_pipeline_state(\n                app.handle(),";
        const HOTKEY_CALL: &str = "register_hotkey(app.handle()";
        const CANCEL_ESC: &str = "register_cancel_esc(";
        const SALVAGE: &str = "salvage_incomplete_recordings(";
        const SWEEP: &str = "sweep_pending_dir(";
        const ENGINE_INIT: &str = "init_and_manage_engine(app.handle()";

        let locate = |marker: &str| RUN_SRC.find(marker).map_or(-1, |o| o as i64);

        let err_history = locate(ERR_HISTORY);
        let mps_call = locate(MPS_CALL);
        let hotkey_call = locate(HOTKEY_CALL);
        let cancel_esc = locate(CANCEL_ESC);
        let salvage = locate(SALVAGE);
        let sweep = locate(SWEEP);
        let engine_init = locate(ENGINE_INIT);

        // Ordered pairs (a must appear before b). Failure messages name both
        // anchors and their found offsets (-1 = anchor missing) so a future
        // reordering is diagnosable from the output alone.
        let pairs: [(&str, i64, &str, &str, i64, &str); 6] = [
            ("salvage", salvage, SALVAGE, "sweep", sweep, SWEEP),
            (
                "salvage",
                salvage,
                SALVAGE,
                "engine init",
                engine_init,
                ENGINE_INIT,
            ),
            (
                "sweep",
                sweep,
                SWEEP,
                "engine init",
                engine_init,
                ENGINE_INIT,
            ),
            (
                "error_history manage",
                err_history,
                ERR_HISTORY,
                "manage_pipeline_state call",
                mps_call,
                MPS_CALL,
            ),
            (
                "manage_pipeline_state call",
                mps_call,
                MPS_CALL,
                "register_hotkey call",
                hotkey_call,
                HOTKEY_CALL,
            ),
            (
                "register_hotkey call",
                hotkey_call,
                HOTKEY_CALL,
                "register_cancel_esc call",
                cancel_esc,
                CANCEL_ESC,
            ),
        ];
        for (a_name, a_off, a_marker, b_name, b_off, b_marker) in pairs {
            assert!(
                a_off >= 0 && b_off >= 0 && a_off < b_off,
                "startup order violated: {a_name} (marker {a_marker:?}, offset {a_off}) must come before \
                 {b_name} (marker {b_marker:?}, offset {b_off}) — see the startup topological order \
                 contract block on run() in lib.rs"
            );
        }
    }
}
