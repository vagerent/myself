mod commands;
mod db;
mod hotkey;
mod insights;
mod meeting;
mod openai;
mod pnote;
mod settings;
mod tasks;
mod tray;
mod tts;
mod winstate;

use chrono::{DateTime, Local};
use db::Db;
use settings::Settings;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::Manager;
use tokio::sync::oneshot;

pub struct AppState {
    pub db: Db,
    pub settings: Mutex<Settings>,
    pub settings_path: PathBuf,
    pub preprocess_busy: AtomicBool,
    pub last_summary_attempt: Mutex<Option<String>>,
    pub next_retry: Mutex<Option<DateTime<Local>>>,
    pub last_weekly_attempt: Mutex<Option<String>>,
    pub next_weekly_retry: Mutex<Option<DateTime<Local>>>,
    pub meeting_busy: AtomicBool,
    pub asr_downloading: AtomicBool,
    pub deep_busy: AtomicBool,
    pub tts_downloading: AtomicBool,
    pub tts_speaking: AtomicBool,
    pub tts_cancel: AtomicBool,
    /// 当前朗读任务的 sidecar 子进程句柄，供停止命令 kill
    pub tts_child: Mutex<Option<tokio::process::Child>>,
    /// 录音卡常驻 sidecar 子进程句柄 / 控制通道写端（tokio Mutex：写 stdin 需跨 await 持有）
    pub pnote_child: Mutex<Option<tokio::process::Child>>,
    pub pnote_stdin: tokio::sync::Mutex<Option<tokio::process::ChildStdin>>,
    /// 录音卡请求序号（JSON-lines 协议按 id 关联响应）
    pub pnote_req_seq: AtomicU64,
    /// 最近一次 state 事件快照
    pub pnote_latest: Mutex<pnote::PnoteStatus>,
    /// 上次设备容量 GC 触发日期（yyyy-mm-dd），同一天最多触发一次
    pub pnote_last_gc: Mutex<Option<String>>,
    /// 挂起的录音卡请求：id → oneshot 发送端
    pub pnote_pending: Mutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>,
    /// 本次 spawn 的 ready 信号（stdout 出现 ready 事件时触发）
    pub pnote_ready: Mutex<Option<oneshot::Sender<()>>>,
    /// 同步/转写重入守卫
    pub pnote_busy: AtomicBool,
    /// sync_now 请求重入守卫（下载阶段不持 pnote_busy，单独防重复点击）
    pub pnote_sync_req: AtomicBool,
    /// sidecar 快速崩溃计数（存活 <60s 即崩溃），连续 5 次后停止自动拉起
    pub pnote_fast_crashes: AtomicU32,
    /// 崩溃自动重启通知：监控任务发消息，常驻重启任务（spawn_inner 懒建）负责拉起，
    /// 避免监控块直接 await ensure_sidecar 造成 Send 自引用环
    pub pnote_restart_tx: Mutex<Option<tokio::sync::mpsc::UnboundedSender<()>>>,
    /// 洞察分析重入守卫（手动触发与间隔调度共用）
    pub insights_busy: AtomicBool,
    /// 上次洞察分析尝试时间（成功/失败都记，调度节流用）
    pub insights_last_run: Mutex<Option<DateTime<Local>>>,
    /// spawn 串行锁，防并发 command 拉起两个实例
    pub pnote_spawn_lock: tokio::sync::Mutex<()>,
    /// sidecar 代次：重新 spawn 时 +1，旧监控任务看到代次不符即退场
    pub pnote_gen: AtomicU64,
}

pub fn run() {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .try_init();

    // WebView2 默认的自动播放策略会拦截没有用户手势的音频播放（朗读是合成完一句
    // 播一句，后续句子没有手势上下文），通过追加浏览器参数放开
    #[cfg(windows)]
    unsafe {
        std::env::set_var(
            "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
            "--autoplay-policy=no-user-gesture-required",
        );
    }

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
        .plugin(tauri_plugin_dialog::init())
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
                meeting_busy: AtomicBool::new(false),
                asr_downloading: AtomicBool::new(false),
                deep_busy: AtomicBool::new(false),
                tts_downloading: AtomicBool::new(false),
                tts_speaking: AtomicBool::new(false),
                tts_cancel: AtomicBool::new(false),
                tts_child: Mutex::new(None),
                pnote_child: Mutex::new(None),
                pnote_stdin: tokio::sync::Mutex::new(None),
                pnote_req_seq: AtomicU64::new(0),
                pnote_latest: Mutex::new(pnote::PnoteStatus::default()),
                pnote_last_gc: Mutex::new(None),
                pnote_pending: Mutex::new(HashMap::new()),
                pnote_ready: Mutex::new(None),
                pnote_busy: AtomicBool::new(false),
                pnote_sync_req: AtomicBool::new(false),
                pnote_fast_crashes: AtomicU32::new(0),
                pnote_restart_tx: Mutex::new(None),
                insights_busy: AtomicBool::new(false),
                insights_last_run: Mutex::new(None),
                pnote_spawn_lock: tokio::sync::Mutex::new(()),
                pnote_gen: AtomicU64::new(0),
            });

            // 崩溃自动重启任务：监控任务只发通知（同步 send，不 await 拉起链路），
    // 由这里统一拉起并补发 start。独立顶层任务而非在 spawn_inner 里创建——
    // 否则「监控块 → ensure_sidecar → spawn_inner」构成 Send 自引用环，编译不过。
    {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
        if let Ok(mut guard) = app_handle.state::<AppState>().pnote_restart_tx.lock() {
            *guard = Some(tx);
        }
        let app_restart = app_handle.clone();
        tauri::async_runtime::spawn(async move {
            while rx.recv().await.is_some() {
                tokio::time::sleep(Duration::from_secs(3)).await;
                let enabled = {
                    let st = app_restart.state::<AppState>();
                    st.settings.lock().map(|s| s.pnote_enabled).unwrap_or(false)
                };
                if !enabled {
                    continue; // 通知发出后被停用，不拉
                }
                log::info!("录音卡组件自动重启");
                if let Err(e) = pnote::ensure_sidecar(&app_restart).await {
                    log::warn!("录音卡组件自动重启失败: {e}");
                    continue;
                }
                if let Err(e) = pnote::start_daemon(&app_restart).await {
                    log::warn!("录音卡守护循环自动重启失败: {e}");
                    continue;
                }
                if let Err(e) = pnote::push_sync_window(&app_restart).await {
                    log::warn!("录音卡同步窗口重推失败: {e}");
                }
            }
        });
    }

    if let Err(e) = hotkey::apply_hotkey(app_handle, &settings.hotkey) {
                log::warn!("全局快捷键注册失败（不影响使用）: {e}");
            }
            // 录音卡常驻 sidecar：fire-and-forget，不阻塞启动；失败仅记日志，
            // 下次 pnote_status 等命令调用时会经 ensure_sidecar 重试
            if settings.pnote_enabled {
                let app2 = app_handle.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = pnote::ensure_sidecar(&app2).await {
                        log::warn!("录音卡组件启动失败（可在设置中重新启用）: {e}");
                        return;
                    }
                    // 只拉起进程还不够：守护循环（定时扫描+自动同步）需要显式 start
                    if let Err(e) = pnote::start_daemon(&app2).await {
                        log::warn!("录音卡守护循环启动失败（可在设置中重新启用）: {e}");
                        return;
                    }
                    // 把同步时间窗口推给 sidecar（无状态过滤：窗口外直接跳过）
                    if let Err(e) = pnote::push_sync_window(&app2).await {
                        log::warn!("录音卡同步窗口推送失败: {e}");
                    }
                    // 对账：下载过但登记丢失的录音（App 重启瞬态丢事件）补登记补转写
                    let rescued = pnote::reconcile_recorder_files(&app2).await;
                    if rescued > 0 {
                        log::info!("录音卡对账：补登记 {rescued} 个磁盘录音");
                    }
                });
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
            commands::delete_entry,
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
            commands::save_window_state,
            meeting::import_meeting_audio,
            meeting::asr_model_status,
            meeting::download_asr_model,
            meeting::generate_deep_analysis,
            tts::tts_model_status,
            tts::download_tts_model,
            tts::meeting_speak,
            tts::meeting_stop_speak,
            tts::tts_read_wav,
            pnote::pnote_status,
            pnote::pnote_sync_now,
            pnote::pnote_file_list,
            pnote::pnote_connect_now,
            pnote::pnote_apply_settings,
            pnote::pnote_bind_device,
            pnote::pnote_unbind_device,
            pnote::pnote_set_enabled,
            insights::insights_analyze_now,
            insights::insights_runs,
            insights::insights_findings,
            insights::insights_unread,
            insights::insights_dismiss,
            insights::insights_mark_read,
            insights::insights_to_reminder,
            insights::insights_to_entry,
            insights::insights_threads
        ])
        .run(tauri::generate_context!())
        .expect("运行马赛福失败");
}
