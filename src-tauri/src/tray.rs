use std::sync::{Arc, Mutex};
use tracing::{info, warn};

use tauri::{
    App, AppHandle, Emitter, Manager, Runtime,
    image::Image,
    menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    webview::WebviewWindowBuilder,
};

/// Tray id used by both `TrayIconBuilder::with_id` (when present) and
/// `tray_by_id` lookups. **Must** be explicitly assigned via
/// `TrayIconBuilder::with_id` — `TrayIconBuilder::new()` defaults the id
/// to a global incrementing counter (tray-icon crate, see
/// `COUNTER.next().to_string()`), so `tray_by_id("main")` /
/// `tray_by_id("default")` both return None. The pre-P4 tray.rs worked
/// only by coincidence (the tooltip update was wrapped in `if let Some`,
/// silently no-op'd). M2-a unified the lookup under one constant.
const TRAY_ID: &str = "main-tray";

/// Build the tray menu. `mistouch_undo = Some(batch_id)` prepends the
/// M2-a 「撤销误触丢弃」item (id carries the batch id; the 5s finalize
/// window is the only time it exists — restoring or finalizing rebuilds
/// without it). MVP: only the most recent mistouch batch is offered
/// (rapid double-mistouch within the window keeps only the latest —
/// registered deviation, design review 2026-09-29 finding B).
fn build_tray_menu<R: Runtime>(
    app: &AppHandle<R>,
    mistouch_undo: Option<u64>,
) -> Result<Menu<R>, Box<dyn std::error::Error>> {
    let mut items: Vec<Box<dyn IsMenuItem<R>>> = Vec::new();
    if let Some(id) = mistouch_undo {
        items.push(Box::new(MenuItem::with_id(
            app,
            format!("undo-mistouch:{id}"),
            "撤销误触丢弃",
            true,
            None::<&str>,
        )?));
        items.push(Box::new(PredefinedMenuItem::separator(app)?));
    }
    items.push(Box::new(MenuItem::with_id(
        app,
        "reset",
        "重置状态",
        true,
        None::<&str>,
    )?));
    items.push(Box::new(MenuItem::with_id(
        app,
        "transcribe",
        "录音转录...",
        true,
        None::<&str>,
    )?));
    items.push(Box::new(MenuItem::with_id(
        app,
        "settings",
        "设置...",
        true,
        None::<&str>,
    )?));
    items.push(Box::new(PredefinedMenuItem::separator(app)?));
    items.push(Box::new(MenuItem::with_id(
        app,
        "quit",
        "退出",
        true,
        None::<&str>,
    )?));
    let refs: Vec<&dyn IsMenuItem<R>> = items.iter().map(|b| b.as_ref()).collect();
    Ok(Menu::with_items(app, &refs)?)
}

/// Show/hide the tray mistouch-undo item by rebuilding the menu (the
/// `on_menu_event` handler set at setup persists across `set_menu` calls).
pub(crate) fn set_mistouch_undo<R: Runtime>(app: &AppHandle<R>, mistouch_undo: Option<u64>) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        warn!("set_mistouch_undo: tray not found");
        return;
    };
    match build_tray_menu(app, mistouch_undo) {
        Ok(menu) => {
            if let Err(e) = tray.set_menu(Some(menu)) {
                warn!("set_mistouch_undo: set_menu failed: {e}");
            }
        }
        Err(e) => warn!("set_mistouch_undo: build menu failed: {e}"),
    }
}

/// Setup the system tray.
pub fn setup_tray<R: Runtime>(app: &App<R>) -> Result<(), Box<dyn std::error::Error>> {
    let menu = build_tray_menu(app.handle(), None)?;

    let icon_bytes = include_bytes!("../icons/32x32.png");
    let icon = image::load_from_memory(icon_bytes)
        .expect("embedded icon should be valid")
        .to_rgba8();
    let (w, h) = icon.dimensions();

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(Image::new_owned(icon.into_raw(), w, h))
        .menu(&menu)
        .tooltip("语文兔 - 就绪")
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "reset" => {
                info!("Tray: user triggered manual reset");
                // Reset state machine
                if let Some(sm) = app.try_state::<Arc<Mutex<crate::state::StateMachine>>>()
                    && let Some(mut guard) =
                        crate::util::lock_mutex(&sm, "state_machine_tray_reset")
                {
                    guard.reset();
                    info!("Tray: state machine reset to Idle");
                }
                // Stop audio capture if recording
                if let Some(ac) =
                    app.try_state::<Arc<Mutex<dyn crate::audio::AudioCaptureProvider>>>()
                    && let Some(mut guard) =
                        crate::util::lock_mutex(&ac, "audio_capture_tray_reset")
                {
                    guard.stop();
                }
                // Hide all windows
                if let Some(win) = app.get_webview_window("floating") {
                    let _ = win.hide();
                }
                if let Some(win) = app.get_webview_window("review") {
                    let _ = win.hide();
                }
                // Restore clipboard if needed
                if let Some(cb) = app.try_state::<Arc<dyn crate::clipboard::ClipboardProvider>>() {
                    let _ = cb.restore();
                }
                // Stop any active record-only recording (bounded wait; the
                // WAV stays playable via finalize or next-startup salvage).
                if let Some(ps) = app.try_state::<crate::commands::pipeline_state::PipelineState>()
                {
                    crate::commands::record_only_session::RecordOnlySession::recover(&ps);
                }
                // Emit event
                let _ = app.emit("tray-reset", ());
                // Update tooltip
                if let Some(tray) = app.tray_by_id(TRAY_ID) {
                    let _ = tray.set_tooltip(Some("语文兔 - 就绪"));
                }
            }
            "transcribe" => {
                if let Some(pt) =
                    app.try_state::<crate::commands::transcribe_cmd::PendingTranscribe>()
                    && let Err(e) = crate::commands::transcribe_cmd::open_window_impl(app, &pt)
                {
                    info!("Tray: open transcribe window failed: {e}");
                }
            }
            "quit" => {
                use std::sync::atomic::Ordering;
                if let Some(flag) = app.try_state::<std::sync::Arc<std::sync::atomic::AtomicBool>>()
                {
                    flag.store(true, Ordering::SeqCst);
                }
                app.exit(0);
            }
            "settings" => {
                if let Some(window) = app.get_webview_window("settings") {
                    let _ = window.show();
                    let _ = window.set_focus();
                } else if let Ok(window) = WebviewWindowBuilder::new(
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
                .build()
                {
                    let _ = window.show();
                    #[cfg(feature = "devtools")]
                    {
                        if let Some(w) = app.get_webview_window("settings") {
                            w.open_devtools();
                        }
                    }
                }
            }
            id if id.starts_with("undo-mistouch:") => {
                let batch_id: u64 = id
                    .strip_prefix("undo-mistouch:")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                info!("Tray: undo mistouch discard batch {batch_id}");
                let pending = app
                    .state::<Arc<crate::commands::data_management_cmd::PendingDeletes>>()
                    .inner()
                    .clone();
                if let Err(e) = pending.restore_with_id(batch_id) {
                    // Restore failure must not be silent to the user (data-page
                    // restore surfaces via error bar; tray path has only tooltip).
                    warn!("undo mistouch restore failed: {e:?}");
                    if let Some(tray) = app.tray_by_id(TRAY_ID) {
                        let _ = tray.set_tooltip(Some(
                            "撤销失败：录音未能恢复，可在数据目录 .dl_pending 手动找回",
                        ));
                    }
                }
                set_mistouch_undo(app, None);
            }
            _ => {}
        })
        .build(app)?;

    Ok(())
}
