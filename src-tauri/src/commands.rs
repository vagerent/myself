use crate::db::{self, SearchHit};
use crate::hotkey;
use crate::openai::OpenAiClient;
use crate::settings::Settings;
use crate::tasks;
use crate::winstate;
use crate::AppState;
use tauri::{AppHandle, State};
use tauri_plugin_autostart::ManagerExt;

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Settings {
    state.settings.lock().unwrap().clone()
}

#[tauri::command]
pub fn save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: Settings,
) -> Result<(), String> {
    if settings.summary_time.trim().len() != 5 || !settings.summary_time.contains(':') {
        return Err("汇总时间格式应为 HH:MM".into());
    }
    if settings.weekly_summary_time.trim().len() != 5
        || !settings.weekly_summary_time.contains(':')
    {
        return Err("每周汇总时间格式应为 HH:MM".into());
    }
    if !(1..=7).contains(&settings.weekly_summary_day) {
        return Err("每周汇总星期应为 1（周一）至 7（周日）".into());
    }
    if !["dark", "light", "system"].contains(&settings.theme.as_str()) {
        return Err("主题必须是 dark / light / system".into());
    }
    hotkey::apply_hotkey(&app, &settings.hotkey)?;
    let path = state.settings_path.clone();
    settings.save(&path)?;
    let autostart = settings.autostart;
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = if autostart {
            app2.autolaunch().enable()
        } else {
            app2.autolaunch().disable()
        };
        if let Err(e) = result {
            log::warn!("开机自启设置失败: {e}");
        }
    });
    *state.settings.lock().map_err(|e| e.to_string())? = settings;
    Ok(())
}

#[tauri::command]
pub fn search(
    state: State<'_, AppState>,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<SearchHit>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    db::search(&conn, &query, limit.unwrap_or(50)).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn add_entry(state: State<'_, AppState>, content: String) -> Result<db::EntryRow, String> {
    let content = content.trim().to_string();
    if content.is_empty() {
        return Err("内容为空".into());
    }
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let dedup_daily = state.settings.lock().map_err(|e| e.to_string())?.dedup_scope != "all";
    let id = db::insert_entry(&conn, &content, "manual", dedup_daily)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| {
            if dedup_daily {
                "重复内容，今天已记录过".to_string()
            } else {
                "重复内容，已存在".to_string()
            }
        })?;
    db::get_entry(&conn, id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "读取失败".into())
}

#[tauri::command]
pub fn update_entry(
    state: State<'_, AppState>,
    id: i64,
    title: String,
    category: String,
    tags: String,
) -> Result<(), String> {
    if !category.is_empty() && !db::CATEGORIES.contains(&category.as_str()) {
        return Err(format!("未知分类：{category}"));
    }
    let tags_vec: Vec<String> = tags
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let tags_json =
        serde_json::to_string(&tags_vec).map_err(|e| e.to_string())?;
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    db::update_entry_meta(&conn, id, title.trim(), category.trim(), &tags_json)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_entry(state: State<'_, AppState>, id: i64) -> Result<db::EntryRow, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    db::get_entry(&conn, id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "条目不存在".into())
}

#[tauri::command]
pub fn list_summaries(state: State<'_, AppState>) -> Result<Vec<db::SummaryRow>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    db::list_summaries(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_tree_stats(state: State<'_, AppState>) -> Result<db::TreeStats, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    db::tree_stats(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_entries_filtered(
    state: State<'_, AppState>,
    category: Option<String>,
    source: Option<String>,
    query: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<db::EntryPage, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    db::list_entries_filtered(
        &conn,
        category.as_deref(),
        source.as_deref(),
        query.as_deref(),
        limit.unwrap_or(20),
        offset.unwrap_or(0),
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_summary(state: State<'_, AppState>, date: String) -> Result<db::SummaryRow, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    db::get_summary(&conn, &date)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "该日期还没有汇总".into())
}

#[tauri::command]
pub async fn rerun_summary(app: AppHandle, date: String) -> Result<db::SummaryRow, String> {
    if let Some((start, end)) = tasks::week_range_from_key(&date) {
        tasks::run_weekly_summary(&app, &date, &start, &end).await
    } else {
        tasks::run_daily_summary(&app, &date).await
    }
}

#[tauri::command]
pub async fn ai_answer(
    state: State<'_, AppState>,
    query: String,
    hits: Vec<SearchHit>,
) -> Result<String, String> {
    let s = state.settings.lock().map_err(|e| e.to_string())?.clone();
    if !s.ai_answer_enabled {
        return Err("AI 回答已在设置中关闭".into());
    }
    if !s.api_key_present() {
        return Err("未配置 API Key，无法使用 AI 回答".into());
    }
    let client = OpenAiClient::new(&s.base_url, &s.api_key, &s.model);
    let mut parts = Vec::new();
    for (i, h) in hits.iter().take(10).enumerate() {
        parts.push(format!(
            "{}. [{}]（日期：{}，来源：{}）\n{}",
            i + 1,
            h.title,
            h.created_at,
            h.source,
            db::clip(&h.content, 800)
        ));
    }
    let user = format!("问题：{}\n\n相关资料：\n{}", query, parts.join("\n\n"));
    client.chat(tasks::ANSWER_SYSTEM, &user).await
}

#[tauri::command]
pub async fn ai_search_terms(
    state: State<'_, AppState>,
    query: String,
) -> Result<Vec<String>, String> {
    let s = state.settings.lock().map_err(|e| e.to_string())?.clone();
    if !s.ai_answer_enabled {
        return Err("AI 回答已在设置中关闭".into());
    }
    if !s.api_key_present() {
        return Err("未配置 API Key，无法使用 AI 搜索".into());
    }
    let client = OpenAiClient::new(&s.base_url, &s.api_key, &s.model);
    client.search_terms(&query).await
}

#[tauri::command]
pub fn copy_text(text: String) -> Result<(), String> {
    let mut cb = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    cb.set_text(text).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn count_today_entries(state: State<'_, AppState>) -> Result<i64, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    db::count_entries_today(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_window_state(app: AppHandle) -> winstate::WindowState {
    winstate::load(&app)
}

#[tauri::command]
pub fn save_window_state(app: AppHandle, state: winstate::WindowState) -> Result<(), String> {
    winstate::save(&app, &state)
}

#[tauri::command]
pub fn open_url(url: String) -> Result<(), String> {
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err("仅支持 http/https 链接".into());
    }
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("cmd")
        .args(["/C", "start", "", &url])
        .spawn();
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(&url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(&url).spawn();
    result.map(|_| ()).map_err(|e| e.to_string())
}

// ---------- 任务提醒 ----------

fn validate_reminder(
    title: &str,
    kind: &str,
    time_of_day: &str,
    weekday: Option<i64>,
    fire_date: Option<&str>,
) -> Result<String, String> {
    if title.trim().is_empty() {
        return Err("标题不能为空".into());
    }
    if !["once", "daily", "weekly"].contains(&kind) {
        return Err("未知提醒类型".into());
    }
    if chrono::NaiveTime::parse_from_str(time_of_day, "%H:%M").is_err() {
        return Err("时间格式应为 HH:MM".into());
    }
    if kind == "weekly" && !matches!(weekday, Some(0..=6)) {
        return Err("每周提醒需指定星期（0=周日 .. 6=周六）".into());
    }
    if kind == "once" {
        let date = fire_date.ok_or_else(|| "单次提醒需指定日期".to_string())?;
        if chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_err() {
            return Err("日期格式应为 YYYY-MM-DD".into());
        }
    }
    let next = db::compute_next_fire(kind, time_of_day, weekday, fire_date)
        .ok_or_else(|| "无法计算下次触发时间".to_string())?;
    if next <= chrono::Local::now().naive_local() {
        return Err("触发时间必须晚于当前时间".into());
    }
    Ok(db::fmt_fire_time(&next))
}

#[tauri::command]
pub fn list_reminders(state: State<'_, AppState>) -> Result<Vec<db::ReminderRow>, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    db::list_reminders(&conn).map_err(|e| e.to_string())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn add_reminder(
    state: State<'_, AppState>,
    title: String,
    note: Option<String>,
    kind: String,
    time_of_day: String,
    weekday: Option<i64>,
    fire_date: Option<String>,
) -> Result<db::ReminderRow, String> {
    let next = validate_reminder(&title, &kind, &time_of_day, weekday, fire_date.as_deref())?;
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let id = db::insert_reminder(
        &conn,
        title.trim(),
        note.unwrap_or_default().trim(),
        &kind,
        &time_of_day,
        weekday,
        fire_date.as_deref(),
        &next,
    )
    .map_err(|e| e.to_string())?;
    db::get_reminder(&conn, id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "读取失败".into())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn update_reminder(
    state: State<'_, AppState>,
    id: i64,
    title: String,
    note: Option<String>,
    kind: String,
    time_of_day: String,
    weekday: Option<i64>,
    fire_date: Option<String>,
) -> Result<(), String> {
    let next = validate_reminder(&title, &kind, &time_of_day, weekday, fire_date.as_deref())?;
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    db::update_reminder(
        &conn,
        id,
        title.trim(),
        note.unwrap_or_default().trim(),
        &kind,
        &time_of_day,
        weekday,
        fire_date.as_deref(),
        &next,
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_reminder(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    db::delete_reminder(&conn, id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_reminder_enabled(
    state: State<'_, AppState>,
    id: i64,
    enabled: bool,
) -> Result<(), String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    if enabled {
        // 重新启用时重算下次触发时间，避免立即被判定为错过
        let r = db::get_reminder(&conn, id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "提醒不存在".to_string())?;
        let next = db::compute_next_fire(&r.kind, &r.time_of_day, r.weekday, r.fire_date.as_deref())
            .ok_or_else(|| "无法计算下次触发时间".to_string())?;
        if next <= chrono::Local::now().naive_local() {
            return Err("触发时间已过，请先编辑时间再启用".into());
        }
        db::advance_reminder(&conn, id, &db::fmt_fire_time(&next)).map_err(|e| e.to_string())?;
    }
    db::set_reminder_enabled(&conn, id, enabled).map_err(|e| e.to_string())
}
