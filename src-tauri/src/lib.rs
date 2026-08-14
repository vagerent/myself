mod commands;
mod db;
mod hotkey;
mod openai;
mod settings;
mod tasks;
mod tray;
mod winstate;

use chrono::{DateTime, Local};
use db::Db;
use settings::Settings;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use tauri::Manager;

pub struct AppState {
    pub db: Db,
    pub settings: Mutex<Settings>,
    pub settings_path: PathBuf,
    pub preprocess_busy: AtomicBool,
    pub last_summary_attempt: Mutex<Option<String>>,
    pub next_retry: Mutex<Option<DateTime<Local>>>,
    pub last_weekly_attempt: Mutex<Option<String>>,
    pub next_weekly_retry: Mutex<Option<DateTime<Local>>>,
}

pub fn run() {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .try_init();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tray::show_main_window(app, "focus-search");
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let db_path = data_dir.join("masaifu.db");
            let settings_path = data_dir.join("settings.json");

            let conn = rusqlite::Connection::open(&db_path)?;
            db::init(&conn)?;

            let (settings, is_first_run) = Settings::load_or_default(&settings_path);
            if is_first_run {
                settings.save(&settings_path)?;
            }

            let app_handle = app.handle();
            if settings.autostart {
                use tauri_plugin_autostart::ManagerExt;
                let _ = app_handle.autolaunch().enable();
            }

            app.manage(AppState {
                db: Arc::new(Mutex::new(conn)),
                settings: Mutex::new(settings.clone()),
                settings_path,
                preprocess_busy: AtomicBool::new(false),
                last_summary_attempt: Mutex::new(None),
                next_retry: Mutex::new(None),
                last_weekly_attempt: Mutex::new(None),
                next_weekly_retry: Mutex::new(None),
            });

            if let Err(e) = hotkey::apply_hotkey(app_handle, &settings.hotkey) {
                log::warn!("全局快捷键注册失败（不影响使用）: {e}");
            }
            tray::build_tray(app_handle)?;
            tasks::spawn_background(app_handle.clone());
            winstate::apply_on_startup(app_handle);

            if is_first_run {
                tray::show_main_window(app_handle, "focus-search");
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::save_settings,
            commands::search,
            commands::add_entry,
            commands::update_entry,
            commands::get_entry,
            commands::list_summaries,
            commands::get_tree_stats,
            commands::list_entries_filtered,
            commands::get_summary,
            commands::rerun_summary,
            commands::ai_answer,
            commands::ai_search_terms,
            commands::copy_text,
            commands::open_url,
            commands::count_today_entries,
            commands::list_reminders,
            commands::add_reminder,
            commands::update_reminder,
            commands::delete_reminder,
            commands::set_reminder_enabled,
            commands::get_window_state,
            commands::save_window_state
        ])
        .run(tauri::generate_context!())
        .expect("运行马赛福失败");
}
