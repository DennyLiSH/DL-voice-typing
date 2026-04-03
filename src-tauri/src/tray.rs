use tauri::{
    App, Manager, Runtime,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    webview::WebviewWindowBuilder,
};

/// Setup the system tray.
pub fn setup_tray<R: Runtime>(app: &App<R>) -> Result<(), Box<dyn std::error::Error>> {
    let settings = MenuItem::with_id(app, "settings", "设置...", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &settings as &dyn tauri::menu::IsMenuItem<R>,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    TrayIconBuilder::new()
        .menu(&menu)
        .tooltip("语文兔 - 就绪")
        .on_menu_event(move |app, event| match event.id().as_ref() {
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
                } else {
                    let _ = WebviewWindowBuilder::new(
                        app,
                        "settings",
                        tauri::WebviewUrl::App("settings.html".into()),
                    )
                    .title("语文兔语音输入法 - 设置")
                    .inner_size(480.0, 620.0)
                    .resizable(true)
                    .center()
                    .build();
                }
            }
            _ => {}
        })
        .build(app)?;

    Ok(())
}
