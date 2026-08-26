use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager};

pub fn show_main_window(app: &AppHandle, event: &str) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
    }
    let _ = app.emit(event, ());
}

pub fn toggle_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        if w.is_visible().unwrap_or(false) {
            let _ = w.hide();
        } else {
            let _ = w.show();
            let _ = w.set_focus();
            let _ = app.emit("focus-search", ());
        }
    }
}

pub fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let icon = app
        .default_window_icon()
        .cloned()
        .expect("缺少默认窗口图标");

    let show = MenuItem::with_id(app, "show", "打开 / 搜索", true, None::<&str>)?;
    let today = MenuItem::with_id(app, "today", "查看今日汇总", true, None::<&str>)?;
    let reminders = MenuItem::with_id(app, "reminders", "任务提醒", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "设置", true, None::<&str>)?;
    let about = MenuItem::with_id(app, "about", "关于", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;

    let menu = Menu::with_items(
        app,
        &[&show, &today, &reminders, &settings, &about, &separator, &quit],
    )?;

    TrayIconBuilder::with_id("main")
        .icon(icon)
        .tooltip("马赛福 - 个人数字分身 v1.2")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main_window(app, "focus-search"),
            "today" => show_main_window(app, "show-summary"),
            "reminders" => show_main_window(app, "show-reminders"),
            "settings" => show_main_window(app, "show-settings"),
            "about" => show_main_window(app, "show-about"),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle_main_window(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(())
}
