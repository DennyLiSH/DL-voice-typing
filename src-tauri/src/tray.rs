use tauri::{
    App, Manager, Runtime,
    menu::{Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::TrayIconBuilder,
};

/// Available languages for speech recognition.
const LANGUAGES: &[(&str, &str)] = &[
    ("zh", "简体中文"),
    ("en", "English"),
    ("zh-TW", "繁體中文"),
    ("ja", "日本語"),
    ("ko", "한국어"),
];

/// Setup the system tray.
pub fn setup_tray<R: Runtime>(app: &App<R>) -> Result<(), Box<dyn std::error::Error>> {
    let language_items: Vec<_> = LANGUAGES
        .iter()
        .map(|(code, name)| {
            MenuItem::with_id(app, format!("lang-{}", code), name, true, None::<&str>)
        })
        .collect::<Result<_, _>>()?;

    let language_refs: Vec<_> = language_items
        .iter()
        .map(|i| i as &dyn tauri::menu::IsMenuItem<R>)
        .collect();
    let language_menu = Submenu::with_items(app, "语言", true, &language_refs)?;

    let llm_enabled = MenuItem::with_id(app, "llm-enabled", "启用", true, None::<&str>)?;
    let llm_settings = MenuItem::with_id(app, "llm-settings", "设置...", true, None::<&str>)?;
    let llm_menu = Submenu::with_items(
        app,
        "LLM 纠错",
        true,
        &[
            &llm_enabled as &dyn tauri::menu::IsMenuItem<R>,
            &llm_settings,
        ],
    )?;

    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &language_menu as &dyn tauri::menu::IsMenuItem<R>,
            &llm_menu,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    TrayIconBuilder::new()
        .menu(&menu)
        .tooltip("DL 语音输入 - 就绪")
        .on_menu_event(move |app, event| {
            match event.id().as_ref() {
                "quit" => {
                    app.exit(0);
                }
                id if id.starts_with("lang-") => {
                    let code = id.strip_prefix("lang-").unwrap();
                    // TODO: update config and notify state machine
                    let _ = code;
                }
                "llm-enabled" => {
                    // TODO: toggle LLM enabled state
                }
                "llm-settings" => {
                    if let Some(window) = app.get_webview_window("settings") {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
                _ => {}
            }
        })
        .build(app)?;

    Ok(())
}
