use crate::db;
use crate::openai::OpenAiClient;
use crate::settings::Settings;
use crate::AppState;
use chrono::{Datelike, Duration as ChronoDuration, Local, NaiveTime};
use std::sync::atomic::Ordering;
use std::time::Duration as StdDuration;
use tauri::{AppHandle, Emitter, Manager};

const SYSTEM_SUMMARY: &str = "你是「马赛福」的每日汇总助手。严格按用户模板生成 Markdown 汇总，输出中文，不要编造条目中不存在的内容。";
const SYSTEM_WEEKLY: &str = "你是「马赛福」的每周汇总助手。严格按用户模板生成 Markdown 周报，输出中文，不要编造条目中不存在的内容。";
pub const ANSWER_SYSTEM: &str = "你是「马赛福」的问答助手。根据用户提供的资料回答用户的问题：1) 优先使用资料中的信息；2) 在回答末尾用列表列出引用资料的标题和日期；3) 资料不足时明确说明；4) 使用中文。";

pub fn spawn_background(app: AppHandle) {
    tauri::async_runtime::spawn(clipboard_loop(app.clone()));
    tauri::async_runtime::spawn(idle_preprocess_loop(app.clone()));
    tauri::async_runtime::spawn(summary_scheduler_loop(app.clone()));
    tauri::async_runtime::spawn(reminder_loop(app.clone()));
    tauri::async_runtime::spawn(pnote_cleanup_loop(app));
}

fn settings_of(app: &AppHandle) -> Settings {
    app.state::<AppState>().settings.lock().unwrap().clone()
}

fn with_db<T>(
    app: &AppHandle,
    f: impl FnOnce(&rusqlite::Connection) -> rusqlite::Result<T>,
) -> Result<T, String> {
    let state = app.state::<AppState>();
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    f(&conn).map_err(|e| e.to_string())
}

async fn clipboard_loop(app: AppHandle) {
    let mut last_hash: Option<String> = None;
    loop {
        let s = settings_of(&app);
        let interval = s.poll_interval_ms.max(200);
        let dedup_daily = s.dedup_scope != "all";
        let enabled = s.clipboard_enabled;
        tokio::time::sleep(StdDuration::from_millis(interval)).await;
        if !enabled {
            continue;
        }
        let mut cb = match arboard::Clipboard::new() {
            Ok(c) => c,
            Err(_) => continue,
        };
        let text = match cb.get_text() {
            Ok(t) => t,
            Err(_) => continue,
        };
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() {
            continue;
        }
        let hash = db::sha256(&trimmed);
        if last_hash.as_deref() == Some(hash.as_str()) {
            continue;
        }
        match with_db(&app, |c| db::insert_entry(c, &trimmed, "clipboard", dedup_daily)) {
            Ok(_) => {
                last_hash = Some(hash);
            }
            Err(e) => log::warn!("剪贴板入库失败: {e}"),
        }
    }
}

/// 文件 mtime 是否早于 days 天前（严格早于边界；days 钳到 ≥0）。
fn is_older_than(mtime: std::time::SystemTime, days: i64) -> bool {
    let cutoff = std::time::SystemTime::now()
        .checked_sub(StdDuration::from_secs(days.max(0) as u64 * 86400))
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    mtime < cutoff
}

async fn pnote_cleanup_loop(app: AppHandle) {
    loop {
        tokio::time::sleep(StdDuration::from_secs(3600)).await;
        let s = settings_of(&app);
        if !s.pnote_enabled {
            continue;
        }
        let keep_days = s.pnote_keep_days.max(1);
        let dir = match app.path().app_data_dir() {
            Ok(d) => d.join("recorder-audio"),
            Err(e) => {
                log::warn!("录音归档目录定位失败: {e}");
                continue;
            }
        };
        let mut removed = 0usize;
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("ogg") {
                    continue;
                }
                let stale = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .map(|t| is_older_than(t, keep_days))
                    .unwrap_or(false);
                if stale && std::fs::remove_file(&path).is_ok() {
                    removed += 1;
                }
            }
        }
        if removed > 0 {
            log::info!("录音归档清理：删除 {removed} 个超过 {keep_days} 天的 ogg 文件");
        }
    }
}

fn idle_seconds() -> u64 {
    #[cfg(windows)]
    {
        use windows::Win32::System::SystemInformation::GetTickCount;
        use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
        let mut lii = LASTINPUTINFO {
            cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        unsafe {
            if GetLastInputInfo(&mut lii).as_bool() {
                return u64::from(GetTickCount().wrapping_sub(lii.dwTime)) / 1000;
            }
        }
        0
    }
    #[cfg(not(windows))]
    {
        0
    }
}

async fn idle_preprocess_loop(app: AppHandle) {
    loop {
        tokio::time::sleep(StdDuration::from_secs(10)).await;
        let s = settings_of(&app);
        if !s.api_key_present() {
            continue;
        }
        let threshold = s.idle_threshold_min.saturating_mul(60);
        if idle_seconds() < threshold {
            continue;
        }
        {
            let state = app.state::<AppState>();
            if state.preprocess_busy.swap(true, Ordering::SeqCst) {
                continue;
            }
        }
        let batch = with_db(&app, |c| db::unprocessed_batch(c, 20)).unwrap_or_default();
        if batch.is_empty() {
            app.state::<AppState>().preprocess_busy.store(false, Ordering::SeqCst);
            continue;
        }
        let client = OpenAiClient::new(&s.base_url, &s.api_key, &s.model);
        match client.preprocess_batch(&batch).await {
            Ok(processed) => {
                let mut ok = 0;
                for p in processed {
                    if with_db(&app, |c| {
                        db::mark_processed(c, p.id, &p.clean, &p.title, &p.category, &p.tags_json())
                    })
                    .is_ok()
                    {
                        ok += 1;
                    }
                }
                log::info!("空闲预处理完成 {ok}/{} 条", batch.len());
            }
            Err(e) => log::warn!("空闲预处理失败: {e}"),
        }
        app.state::<AppState>().preprocess_busy.store(false, Ordering::SeqCst);
    }
}

async fn summary_scheduler_loop(app: AppHandle) {
    loop {
        tokio::time::sleep(StdDuration::from_secs(30)).await;
        let s = settings_of(&app);
        if !s.api_key_present() {
            continue;
        }
        let now = Local::now();
        let hhmm = now.format("%H:%M").to_string();

        if hhmm == s.summary_time.trim() {
            let today = now.format("%Y-%m-%d").to_string();
            let should_run = {
                let state = app.state::<AppState>();
                let mut last = state.last_summary_attempt.lock().unwrap();
                if last.as_deref() == Some(today.as_str()) {
                    false
                } else {
                    let retry_until = state.next_retry.lock().unwrap().clone();
                    match retry_until {
                        Some(until) if now < until => false,
                        _ => {
                            *last = Some(today.clone());
                            true
                        }
                    }
                }
            };
            if should_run {
                let yesterday = (now - ChronoDuration::days(1)).format("%Y-%m-%d").to_string();
                match run_daily_summary(&app, &yesterday).await {
                    Ok(row) => log::info!("每日汇总完成: {}", row.date),
                    Err(e) => {
                        log::warn!("每日汇总失败: {e}");
                        let state = app.state::<AppState>();
                        if now.time() < NaiveTime::from_hms_opt(12, 0, 0).unwrap() {
                            *state.next_retry.lock().unwrap() =
                                Some(now + ChronoDuration::minutes(10));
                            *state.last_summary_attempt.lock().unwrap() = None;
                        }
                    }
                }
            }
        }

        let weekly_day = s.weekly_summary_day.clamp(1, 7);
        if now.weekday().num_days_from_monday() + 1 == u32::from(weekly_day)
            && hhmm == s.weekly_summary_time.trim()
        {
            let (week_key, start, end) = last_week_range(now);
            let should_run = {
                let state = app.state::<AppState>();
                let mut last = state.last_weekly_attempt.lock().unwrap();
                if last.as_deref() == Some(week_key.as_str()) {
                    false
                } else {
                    let retry_until = state.next_weekly_retry.lock().unwrap().clone();
                    match retry_until {
                        Some(until) if now < until => false,
                        _ => {
                            *last = Some(week_key.clone());
                            true
                        }
                    }
                }
            };
            if should_run {
                match run_weekly_summary(&app, &week_key, &start, &end).await {
                    Ok(row) => log::info!("每周汇总完成: {}", row.date),
                    Err(e) => {
                        log::warn!("每周汇总失败: {e}");
                        let state = app.state::<AppState>();
                        if now.time() < NaiveTime::from_hms_opt(12, 0, 0).unwrap() {
                            *state.next_weekly_retry.lock().unwrap() =
                                Some(now + ChronoDuration::minutes(10));
                            *state.last_weekly_attempt.lock().unwrap() = None;
                        }
                    }
                }
            }
        }
    }
}

/// 上一完整周（周一至周日）的 (周标识, 起始日期, 结束日期)。
pub fn last_week_range(now: chrono::DateTime<Local>) -> (String, String, String) {
    let today = now.date_naive();
    let this_monday = today - ChronoDuration::days(i64::from(today.weekday().num_days_from_monday()));
    let last_monday = this_monday - ChronoDuration::days(7);
    let last_sunday = this_monday - ChronoDuration::days(1);
    let iso = last_monday.iso_week();
    (
        format!("{}-W{:02}", iso.year(), iso.week()),
        last_monday.format("%Y-%m-%d").to_string(),
        last_sunday.format("%Y-%m-%d").to_string(),
    )
}

/// 从周标识（如 2026-W33）解析该周的起止日期。
pub fn week_range_from_key(key: &str) -> Option<(String, String)> {
    if !db::is_week_key(key) {
        return None;
    }
    let year: i32 = key[..4].parse().ok()?;
    let week: u32 = key[6..].parse().ok()?;
    let monday = chrono::NaiveDate::from_isoywd_opt(year, week, chrono::Weekday::Mon)?;
    let sunday = monday + ChronoDuration::days(6);
    Some((
        monday.format("%Y-%m-%d").to_string(),
        sunday.format("%Y-%m-%d").to_string(),
    ))
}

pub async fn run_weekly_summary(
    app: &AppHandle,
    week_key: &str,
    start_date: &str,
    end_date: &str,
) -> Result<db::SummaryRow, String> {
    let _ = app.emit("summary-started", week_key);
    let result = run_weekly_summary_inner(app, week_key, start_date, end_date).await;
    if let Err(e) = &result {
        push_alert(app, "每周汇总生成失败", week_key, e);
        let _ = app.emit("summary-failed", week_key);
    }
    result
}

async fn run_weekly_summary_inner(
    app: &AppHandle,
    week_key: &str,
    start_date: &str,
    end_date: &str,
) -> Result<db::SummaryRow, String> {
    let s = settings_of(app);
    if s.api_key_present() {
        let pending =
            with_db(app, |c| db::unprocessed_for_range(c, start_date, end_date, 200))
                .unwrap_or_default();
        if !pending.is_empty() {
            let client = OpenAiClient::new(&s.base_url, &s.api_key, &s.model);
            if let Ok(processed) = client.preprocess_batch(&pending).await {
                for p in processed {
                    let _ = with_db(app, |c| {
                        db::mark_processed(c, p.id, &p.clean, &p.title, &p.category, &p.tags_json())
                    });
                }
            }
        }
    }

    let entries = with_db(app, |c| db::entries_for_range(c, start_date, end_date))?;
    let range_label = format!("{start_date} ~ {end_date}");
    let content = if !s.api_key_present() {
        plain_listing(&format!("{week_key}（{range_label}）"), &entries)
    } else if entries.is_empty() {
        format!("# {week_key} 周汇总\n\n（本周没有记录）\n")
    } else {
        let client = OpenAiClient::new(&s.base_url, &s.api_key, &s.model);
        let digests = client.entry_digests(&entries).await?;
        let entries_text = entries
            .iter()
            .map(|e| {
                let title = e
                    .title
                    .clone()
                    .filter(|t| !t.is_empty())
                    .unwrap_or_else(|| db::clip(&e.raw_content, 30));
                let content = e
                    .clean_content
                    .clone()
                    .filter(|c| !c.is_empty())
                    .unwrap_or_else(|| e.raw_content.clone());
                let digest = digests.get(&e.id).cloned().unwrap_or_default();
                format!(
                    "日期：{}\n标题：{title}\n分类：{}\n标签：{}\n摘要：{digest}\n内容：{}\n",
                    e.created_at.get(..10).unwrap_or(&e.created_at),
                    e.category.clone().unwrap_or_else(|| "其他".into()),
                    e.tags.clone().unwrap_or_else(|| "[]".into()),
                    db::clip(&content, 400)
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let template = s.weekly_prompt_template.clone();
        let user = if template.contains("{week}") || template.contains("{entries}") {
            template
                .replace("{week}", week_key)
                .replace("{range}", &range_label)
                .replace("{entries}", &entries_text)
        } else {
            format!("{template}\n\n周次：{week_key}（{range_label}）\n本周条目：\n{entries_text}")
        };
        client.chat(SYSTEM_WEEKLY, &user).await?
    };

    let dir = s.resolve_weekly_markdown_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{week_key}.md"));
    std::fs::write(&path, &content).map_err(|e| e.to_string())?;
    let path_str = path.to_string_lossy().to_string();
    with_db(app, |c| db::insert_summary(c, week_key, &content, &path_str))?;
    let row = with_db(app, |c| db::get_summary(c, week_key))?
        .ok_or_else(|| "周汇总写入失败".to_string())?;
    push_alert(app, "每周汇总完成", &format!("{week_key} 的周报已生成"), "");
    let _ = app.emit("summary-done", &row);
    Ok(row)
}

pub async fn run_daily_summary(app: &AppHandle, date: &str) -> Result<db::SummaryRow, String> {
    let _ = app.emit("summary-started", date);
    let result = run_daily_summary_inner(app, date).await;
    if let Err(e) = &result {
        push_alert(app, "每日汇总生成失败", date, e);
        let _ = app.emit("summary-failed", date);
    }
    result
}

async fn run_daily_summary_inner(app: &AppHandle, date: &str) -> Result<db::SummaryRow, String> {
    let s = settings_of(app);
    if s.api_key_present() {
        let pending = with_db(app, |c| db::unprocessed_for_date(c, date, 100)).unwrap_or_default();
        if !pending.is_empty() {
            let client = OpenAiClient::new(&s.base_url, &s.api_key, &s.model);
            if let Ok(processed) = client.preprocess_batch(&pending).await {
                for p in processed {
                    let _ = with_db(app, |c| {
                        db::mark_processed(c, p.id, &p.clean, &p.title, &p.category, &p.tags_json())
                    });
                }
            }
        }
    }

    let entries = with_db(app, |c| db::entries_for_date(c, date))?;
    let content = if !s.api_key_present() {
        plain_listing(date, &entries)
    } else if entries.is_empty() {
        format!("# {date} 汇总\n\n（当天没有记录）\n")
    } else {
        let client = OpenAiClient::new(&s.base_url, &s.api_key, &s.model);
        let digests = client.entry_digests(&entries).await?;
        let entries_text = entries
            .iter()
            .map(|e| {
                let title = e
                    .title
                    .clone()
                    .filter(|t| !t.is_empty())
                    .unwrap_or_else(|| db::clip(&e.raw_content, 30));
                let content = e
                    .clean_content
                    .clone()
                    .filter(|c| !c.is_empty())
                    .unwrap_or_else(|| e.raw_content.clone());
                let digest = digests.get(&e.id).cloned().unwrap_or_default();
                format!(
                    "标题：{title}\n分类：{}\n标签：{}\n摘要：{digest}\n内容：{}\n",
                    e.category.clone().unwrap_or_else(|| "其他".into()),
                    e.tags.clone().unwrap_or_else(|| "[]".into()),
                    db::clip(&content, 400)
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let template = s.prompt_template.clone();
        let user = if template.contains("{date}") || template.contains("{entries}") {
            template.replace("{date}", date).replace("{entries}", &entries_text)
        } else {
            format!("{template}\n\n日期：{date}\n当日条目：\n{entries_text}")
        };
        client.chat(SYSTEM_SUMMARY, &user).await?
    };

    let dir = s.resolve_markdown_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{date}.md"));
    std::fs::write(&path, &content).map_err(|e| e.to_string())?;
    let path_str = path.to_string_lossy().to_string();
    with_db(app, |c| db::insert_summary(c, date, &content, &path_str))?;
    let row = with_db(app, |c| db::get_summary(c, date))?
        .ok_or_else(|| "汇总写入失败".to_string())?;
    push_alert(app, "每日汇总完成", &format!("{date} 的汇总已生成"), "");
    let _ = app.emit("summary-done", &row);
    Ok(row)
}

fn plain_listing(date: &str, entries: &[db::EntryRow]) -> String {
    let mut out = format!("# {date} 汇总\n\n> 未配置 API Key，以下为原始条目列表。\n");
    for e in entries {
        let title = e
            .title
            .clone()
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| db::clip(&e.raw_content, 30));
        out.push_str(&format!("\n## {title}\n\n{}\n", e.raw_content));
    }
    out
}

/// 触发宽限窗口：到期超过该时长视为「错过」，静默跳过不补发。
const REMINDER_GRACE_SECS: i64 = 90;

/// 应用内提醒弹窗的事件负载。
#[derive(Clone, serde::Serialize)]
struct AppAlert {
    heading: String,
    title: String,
    note: String,
}

/// 唤起主窗口并弹出应用内提醒，不走系统通知。
fn push_alert(app: &AppHandle, heading: &str, title: &str, note: &str) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
    let _ = app.emit(
        "app-alert",
        AppAlert {
            heading: heading.to_string(),
            title: title.to_string(),
            note: note.to_string(),
        },
    );
}

async fn reminder_loop(app: AppHandle) {
    loop {
        tokio::time::sleep(StdDuration::from_secs(30)).await;
        let now = Local::now().naive_local();
        let now_str = now.format("%Y-%m-%d %H:%M:%S").to_string();
        let due = with_db(&app, |c| db::due_reminders(c, &now_str)).unwrap_or_default();
        for r in due {
            let fire_at = chrono::NaiveDateTime::parse_from_str(&r.next_fire_at, "%Y-%m-%d %H:%M:%S")
                .unwrap_or(now);
            let overdue = (now - fire_at).num_seconds();
            let next = db::compute_next_fire(
                &r.kind,
                &r.time_of_day,
                r.weekday,
                r.fire_date.as_deref(),
            )
            .map(|dt| db::fmt_fire_time(&dt));
            if overdue > REMINDER_GRACE_SECS {
                // 错过（应用未运行等）：单次静默停用，重复静默推进
                let res = if r.kind == "once" {
                    with_db(&app, |c| db::mark_reminder_missed(c, r.id))
                } else if let Some(next) = next {
                    with_db(&app, |c| db::advance_reminder(c, r.id, &next))
                } else {
                    continue;
                };
                if let Err(e) = res {
                    log::warn!("提醒错过处理失败: {e}");
                }
                continue;
            }
            // 应用内弹窗提醒：唤起主窗口并推送事件，不走系统通知
            push_alert(&app, "任务提醒", &r.title, &r.note);
            let next = if r.kind == "once" { None } else { next };
            if let Err(e) = with_db(&app, |c| {
                db::mark_reminder_fired(c, r.id, &now_str, next.as_deref())
            }) {
                log::warn!("提醒触发回写失败: {e}");
            }
            let _ = app.emit("reminder-fired", r.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn is_older_than_boundary() {
        let days = 30;
        let cutoff = std::time::SystemTime::now()
            .checked_sub(std::time::Duration::from_secs(days as u64 * 86400))
            .unwrap();
        // 边界用 ±5s 裕量：函数内部取 now() 与这里计算存在亚秒级时间差，
        // 正好相等的断言会 flaky；语义是「严格早于边界才算过期」
        assert!(is_older_than(cutoff - std::time::Duration::from_secs(5), days));
        assert!(!is_older_than(cutoff + std::time::Duration::from_secs(5), days));
        // 远未过期
        assert!(!is_older_than(
            std::time::SystemTime::now(),
            days
        ));
    }

    #[test]
    fn last_week_range_from_mid_week() {
        // 2026-08-11 是周二；上周应为 08-03（周一）~ 08-09（周日），ISO W32
        let now = Local.with_ymd_and_hms(2026, 8, 11, 10, 0, 0).unwrap();
        let (key, start, end) = last_week_range(now);
        assert_eq!(start, "2026-08-03");
        assert_eq!(end, "2026-08-09");
        assert_eq!(key, "2026-W32");
    }

    #[test]
    fn last_week_range_from_monday() {
        // 周一触发时汇总刚过去的一周
        let now = Local.with_ymd_and_hms(2026, 8, 10, 0, 15, 0).unwrap();
        let (key, start, end) = last_week_range(now);
        assert_eq!(start, "2026-08-03");
        assert_eq!(end, "2026-08-09");
        assert_eq!(key, "2026-W32");
    }

    #[test]
    fn last_week_range_cross_year() {
        // 2026-01-05 是周一；上周跨年到 2026-W01（2025-12-29 ~ 2026-01-04）
        let now = Local.with_ymd_and_hms(2026, 1, 5, 0, 15, 0).unwrap();
        let (key, start, end) = last_week_range(now);
        assert_eq!(start, "2025-12-29");
        assert_eq!(end, "2026-01-04");
        assert_eq!(key, "2026-W01");
    }

    #[test]
    fn week_key_roundtrip() {
        let (start, end) = week_range_from_key("2026-W32").unwrap();
        assert_eq!(start, "2026-08-03");
        assert_eq!(end, "2026-08-09");
        assert!(week_range_from_key("2026-08-11").is_none());
        assert!(week_range_from_key("2026-W99").is_none());
    }
}
