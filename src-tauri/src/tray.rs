use crate::clipboard::ClipboardProvider;
use std::sync::{Arc, Mutex};
use tracing::info;

use tauri::{
    App, Emitter, Manager, Runtime,
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    webview::WebviewWindowBuilder,
};

/// Setup the system tray.
pub fn setup_tray<R: Runtime>(app: &App<R>) -> Result<(), Box<dyn std::error::Error>> {
    let reset = MenuItem::with_id(app, "reset", "重置状态", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "设置...", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &reset as &dyn tauri::menu::IsMenuItem<R>,
            &settings as &dyn tauri::menu::IsMenuItem<R>,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    let icon_bytes = include_bytes!("../icons/32x32.png");
    let icon = image::load_from_memory(icon_bytes)
        .expect("embedded icon should be valid")
        .to_rgba8();
    let (w, h) = icon.dimensions();

    TrayIconBuilder::new()
        .icon(Image::new_owned(icon.into_raw(), w, h))
        .menu(&menu)
        .tooltip("语文兔 - 就绪")
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "reset" => {
                info!("Tray: user triggered manual reset");
                // Reset state machine
                if let Some(sm) = app.try_state::<Arc<Mutex<crate::state::StateMachine>>>() {
                    if let Some(mut guard) =
                        crate::util::lock_mutex(&sm, "state_machine_tray_reset")
                    {
                        guard.reset();
                        info!("Tray: state machine reset to Idle");
                    }
                }
                // Stop audio capture if recording
                if let Some(ac) = app.try_state::<Arc<Mutex<crate::audio::AudioCapture>>>() {
                    if let Some(mut guard) =
                        crate::util::lock_mutex(&ac, "audio_capture_tray_reset")
                    {
                        guard.stop();
                    }
                }
                // Hide all windows
                if let Some(win) = app.get_webview_window("floating") {
                    let _ = win.hide();
                }
                if let Some(win) = app.get_webview_window("review") {
                    let _ = win.hide();
                }
                // Restore clipboard if needed
                if let Some(cb) = app.try_state::<Arc<Mutex<crate::clipboard::AnyClipboard>>>() {
                    if let Some(mut guard) = crate::util::lock_mutex(&cb, "clipboard_tray_reset") {
                        let _ = guard.restore();
                    }
                }
                // Emit event
                let _ = app.emit("tray-reset", ());
                // Update tooltip
                if let Some(tray) = app.tray_by_id("default") {
                    let _ = tray.set_tooltip(Some("语文兔 - 就绪"));
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
            _ => {}
        })
        .build(app)?;

    Ok(())
}
