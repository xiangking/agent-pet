use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, Runtime, WebviewUrl, WebviewWindowBuilder,
};

fn tray_label(language: &str, key: &str) -> String {
    match (language, key) {
        ("zh-CN", "quit") => "退出".to_string(),
        ("zh-CN", "settings") => "设置".to_string(),
        ("zh-CN", "reload") => "重新加载宠物".to_string(),
        (_, "quit") => "Quit".to_string(),
        (_, "settings") => "Settings".to_string(),
        (_, "reload") => "Reload Pet".to_string(),
        _ => key.to_string(),
    }
}

pub fn show_settings_window<R: Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(window) = app.get_webview_window("settings") {
        let _ = window.eval("window.location.reload()");
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }

    match WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("index.html".into()))
        .title("Agent Pet Settings")
        .inner_size(860.0, 720.0)
        .center()
        .visible(true)
        .build()
    {
        Ok(window) => {
            let _ = window.set_focus();
        }
        Err(error) => {
            log::error!("Failed to open settings window: {error}");
        }
    }
}

pub fn setup_tray<R: Runtime>(
    app: &tauri::App<R>,
    language: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    build_tray(app, app.handle(), language)
}

pub fn rebuild_tray<R: Runtime>(
    app_handle: &tauri::AppHandle<R>,
    language: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(tray) = app_handle.tray_by_id("main-tray") {
        let quit_i = MenuItem::with_id(
            app_handle,
            "quit",
            tray_label(language, "quit"),
            true,
            None::<&str>,
        )?;
        let settings_i = MenuItem::with_id(
            app_handle,
            "settings",
            tray_label(language, "settings"),
            true,
            None::<&str>,
        )?;
        let reload_i = MenuItem::with_id(
            app_handle,
            "reload",
            tray_label(language, "reload"),
            true,
            None::<&str>,
        )?;
        let separator = PredefinedMenuItem::separator(app_handle)?;
        let menu = Menu::with_items(app_handle, &[&settings_i, &reload_i, &separator, &quit_i])?;
        tray.set_menu(Some(menu))?;
    }
    Ok(())
}

fn build_tray<R: Runtime>(
    app: &tauri::App<R>,
    app_handle: &tauri::AppHandle<R>,
    language: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let quit_i = MenuItem::with_id(
        app,
        "quit",
        tray_label(language, "quit"),
        true,
        None::<&str>,
    )?;
    let settings_i = MenuItem::with_id(
        app,
        "settings",
        tray_label(language, "settings"),
        true,
        None::<&str>,
    )?;
    let reload_i = MenuItem::with_id(
        app,
        "reload",
        tray_label(language, "reload"),
        true,
        None::<&str>,
    )?;
    let separator = PredefinedMenuItem::separator(app)?;

    let menu = Menu::with_items(app, &[&settings_i, &reload_i, &separator, &quit_i])?;

    let _tray = TrayIconBuilder::with_id("main-tray")
        .icon(app_handle.default_window_icon().unwrap().clone())
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "quit" => {
                app.exit(0);
            }
            "settings" => {
                show_settings_window(app);
            }
            "reload" => {
                if let Some(window) = app.get_webview_window("pet") {
                    let _ = window.emit("reload-pet", ());
                }
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Some(window) = app.get_webview_window("pet") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
        })
        .build(app)?;

    Ok(())
}
