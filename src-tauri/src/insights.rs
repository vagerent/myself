//! 录音洞察分析引擎：未分析的录音深度解析 → 一次 LLM 调用
//! （新发现 findings + 滚动项目脉络 state）→ 发现去重落库、我的承诺自动建提醒、
//! 项目状态按 60 天窗口修剪归档。手动触发与间隔调度共用 `run_insights`。

use crate::db;
use crate::openai::OpenAiClient;
use crate::AppState;
use rusqlite::OptionalExtension;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Emitter, Manager};

/// 单次运行最多消化的深度解析篇数（超出留待下个周期）。
const MAX_CANDIDATES: i64 = 50;
/// 项目脉络修剪窗口：距今超过该天数未被提及的项目移入 archived。
const ARCHIVE_DAYS: i64 = 60;

const VALID_KINDS: [&str; 4] = ["commitment_mine", "commitment_other", "idea", "open_loop"];

/// 校验后的待落库发现。
struct NewFinding {
    kind: String,
    title: String,
    note: String,
    quote: String,
    event_date: Option<String>,
    entry_id: Option<i64>,
}

/// 手动触发一次洞察分析（busy 即报错，由前端按钮调用）。
#[tauri::command]
pub async fn insights_analyze_now(app: AppHandle) -> Result<(), String> {
    run_insights(&app, "manual").await
}

/// 最近 50 次运行记录（含每次的发现数）。
#[tauri::command]
pub async fn insights_runs(app: AppHandle) -> Result<Vec<db::InsightsRunRow>, String> {
    with_db(&app, |conn| db::list_runs(conn, 50))
}

/// 某次运行的发现清单；run_id 缺省取最近一条 run，无 run 返回空。
#[tauri::command]
pub async fn insights_findings(
    app: AppHandle,
    run_id: Option<i64>,
) -> Result<Vec<db::InsightsFindingRow>, String> {
    let rid = match run_id {
        Some(id) => id,
        None => match with_db(&app, |conn| db::list_runs(conn, 1))?.first() {
            Some(run) => run.id,
            None => return Ok(Vec::new()),
        },
    };
    with_db(&app, |conn| db::list_findings(conn, rid))
}

/// status='new' 的发现数（前端徽标）。
#[tauri::command]
pub async fn insights_unread(app: AppHandle) -> Result<i64, String> {
    with_db(&app, db::findings_unread_count)
}

/// 忽略一条发现（dismissed 是墓碑，此后永不重复报告）。
#[tauri::command]
pub async fn insights_dismiss(app: AppHandle, id: i64) -> Result<(), String> {
    with_db(&app, |conn| db::set_finding_status(conn, id, "dismissed")).map(|_| ())
}

/// 查看即读：全部 status='new' 置为 'done'。
#[tauri::command]
pub async fn insights_mark_read(app: AppHandle) -> Result<(), String> {
    with_db(&app, |conn| {
        conn.execute(
            "UPDATE recorder_analysis_findings SET status = 'done' WHERE status = 'new'",
            [],
        )
    })
    .map(|_| ())
}

/// 把一条发现转为单次提醒（09:00 触发）并回链 reminder_id。
/// 已有提醒的发现报错；event_date 缺省用明天。
#[tauri::command]
pub async fn insights_to_reminder(app: AppHandle, id: i64) -> Result<(), String> {
    let finding = with_db(&app, |conn| get_finding(conn, id))?
        .ok_or_else(|| "发现不存在".to_string())?;
    if finding.reminder_id.is_some() {
        return Err("该发现已创建过提醒".into());
    }
    let tomorrow = (chrono::Local::now() + chrono::Duration::days(1))
        .format("%Y-%m-%d")
        .to_string();
    let date = finding.event_date.clone().unwrap_or(tomorrow);
    let note = if finding.quote.is_empty() {
        finding.note.clone()
    } else {
        format!("{}\n\n> {}", finding.note, finding.quote)
    };
    let Some(next_fire) = db::compute_next_fire("once", "09:00", None, Some(&date)) else {
        return Err("计算提醒触发时间失败".into());
    };
    with_db(&app, |conn| {
        let rid = db::insert_reminder(
            conn,
            &finding.title,
            &note,
            "once",
            "09:00",
            None,
            Some(&date),
            &db::fmt_fire_time(&next_fire),
        )?;
        db::link_finding_reminder(conn, id, rid)
    })?;
    Ok(())
}

/// 把一条发现转为知识库条目（source='manual'）：标题 + 备注 + 引文，
/// 分类 idea→灵感、其余→待办；成功后标记 done。
/// 返回新条目 id；相同内容当日已存在时返回 null 与提示。
#[tauri::command]
pub async fn insights_to_entry(app: AppHandle, id: i64) -> Result<serde_json::Value, String> {
    let finding = with_db(&app, |conn| get_finding(conn, id))?
        .ok_or_else(|| "发现不存在".to_string())?;
    let mut text = format!("{}\n\n{}", finding.title, finding.note);
    if !finding.quote.is_empty() {
        text.push_str("\n\n> ");
        text.push_str(&finding.quote);
    }
    let category = if finding.kind == "idea" { "灵感" } else { "待办" };
    let dedup_daily = {
        let state = app.state::<AppState>();
        let s = state.settings.lock().map_err(|e| e.to_string())?;
        s.dedup_scope != "all"
    };
    let entry_id = with_db(&app, |conn| db::insert_entry(conn, &text, "manual", dedup_daily))?;
    match entry_id {
        Some(eid) => {
            with_db(&app, |conn| {
                db::update_entry_meta(conn, eid, &finding.title, category, "[]")
            })?;
            with_db(&app, |conn| db::set_finding_status(conn, id, "done"))?;
            Ok(serde_json::json!({ "entryId": eid }))
        }
        None => Ok(serde_json::json!({
            "entryId": serde_json::Value::Null,
            "message": "相同内容今日已存在",
        })),
    }
}

/// 跨天项目脉络（state JSON 原样解析返回；无则 Null）。
#[tauri::command]
pub async fn insights_threads(app: AppHandle) -> Result<serde_json::Value, String> {
    let json = with_db(&app, db::load_project_state)?;
    match json {
        Some(s) => Ok(serde_json::from_str(&s).unwrap_or(serde_json::Value::Null)),
        None => Ok(serde_json::Value::Null),
    }
}

/// 一次完整的洞察分析。busy 门控在调度跳过与手动报错之间共用；
/// 任一步失败：不 stamp 候选（下轮重试），并留一条 failed run 记录。
pub async fn run_insights(app: &AppHandle, trigger: &str) -> Result<(), String> {
    let state = app.state::<AppState>();
    if state.insights_busy.swap(true, Ordering::SeqCst) {
        return Err("已有洞察分析进行中".into());
    }
    let result = run_insights_inner(app, trigger).await;
    app.state::<AppState>()
        .insights_busy
        .store(false, Ordering::SeqCst);
    if let Err(e) = &result {
        log::warn!("录音洞察分析失败（{trigger}）: {e}");
        if let Err(db_err) = with_db(app, |conn| db::insert_run(conn, trigger, 0, "failed", Some(e)))
        {
            log::warn!("录音洞察失败运行记录写入失败: {db_err}");
        }
    }
    result
}

async fn run_insights_inner(app: &AppHandle, trigger: &str) -> Result<(), String> {
    let settings = {
        let state = app.state::<AppState>();
        let guard = state.settings.lock().map_err(|e| e.to_string())?;
        guard.clone()
    };
    let candidates =
        with_db(app, |conn| db::recorder_analysis_candidates(conn, MAX_CANDIDATES))?;
    if candidates.is_empty() {
        // 空跑：不调 API、不建 run 记录
        log::info!("录音洞察（{trigger}）：没有待分析的深度解析，跳过");
        return Ok(());
    }
    if !settings.api_key_present() {
        return Err("未配置 API Key，请在「设置 → AI 服务」中配置".into());
    }
    let state_json = with_db(app, db::load_project_state)?
        .unwrap_or_else(|| "{}".to_string());
    // 已有活跃发现 = 最近一条 run 的 findings 中未 dismissed 的部分
    let active = match with_db(app, |conn| db::list_runs(conn, 1))?.first() {
        Some(run) => with_db(app, |conn| db::list_findings(conn, run.id))?,
        None => Vec::new(),
    };
    let (user, ref_map) = build_user_message(&state_json, &active, &candidates);
    let client = OpenAiClient::new(&settings.base_url, &settings.api_key, &settings.model);
    let raw = client
        .recorder_insights(&user, &settings.recorder_event_prompt)
        .await
        .map_err(|e| format!("AI 分析失败：{e}"))?;
    // 模型可能在 JSON 前后输出多余文字：抠出第一个括号配平的 JSON 对象
    // （跳过字符串内部的括号）。比「首 { 到尾 }」稳——模型在 JSON 后再输出
    // 带括号的多余文字（或第二个 JSON）不会被卷进来。
    let json_str = extract_json_object(&raw)
        .ok_or_else(|| "AI 返回内容中没有找到完整的 JSON 对象".to_string())?;
    let parsed: serde_json::Value = match serde_json::from_str(json_str) {
        Ok(v) => v,
        Err(e) => {
            log::warn!(
                "洞察分析 JSON 解析失败，原始响应前 500 字：{}",
                db::clip(&raw, 500)
            );
            return Err(format!("AI 返回的 JSON 解析失败：{e}"));
        }
    };
    let findings_arr = parsed
        .get("findings")
        .and_then(|f| f.as_array())
        .ok_or_else(|| "AI 返回的 JSON 缺少 findings 数组".to_string())?;
    let findings = parse_findings(findings_arr, &ref_map);
    let mut state = parsed
        .get("state")
        .cloned()
        .filter(|s| s.is_object())
        .ok_or_else(|| "AI 返回的 JSON 缺少 state 对象".to_string())?;
    prune_state(&mut state);

    // 校验全部通过才落库：run → findings（+我的承诺自动提醒）→ state → stamp 候选
    let run_id = with_db(app, |conn| {
        db::insert_run(conn, trigger, candidates.len() as i64, "ok", None)
    })?;
    let mut inserted = 0usize;
    let mut reminders = 0usize;
    for f in &findings {
        let normalized = f.title.trim().to_lowercase();
        if with_db(app, |conn| db::finding_exists_dedup(conn, &f.kind, &normalized))? {
            continue;
        }
        let finding_id = with_db(app, |conn| {
            db::insert_finding(
                conn,
                run_id,
                &f.kind,
                &f.title,
                &f.note,
                &f.quote,
                f.entry_id,
                f.event_date.as_deref(),
            )
        })?;
        inserted += 1;
        if f.kind == "commitment_mine" {
            if let Some(date) = &f.event_date {
                match create_commitment_reminder(app, finding_id, &f.title, &f.note, date).await {
                    Ok(true) => reminders += 1,
                    Ok(false) => log::warn!("洞察发现 {finding_id} 提醒日期无效，跳过自动提醒"),
                    Err(e) => log::warn!("洞察发现 {finding_id} 自动创建提醒失败（非致命）: {e}"),
                }
            }
        }
    }
    with_db(app, |conn| db::save_project_state(conn, &state.to_string()))?;
    let ids: Vec<i64> = candidates.iter().map(|c| c.id).collect();
    with_db(app, |conn| db::mark_recorder_analyzed(conn, &ids))?;
    log::info!(
        "录音洞察（{trigger}）：分析 {} 篇深度解析，新发现 {inserted} 条，自动提醒 {reminders} 条",
        candidates.len()
    );
    let _ = app.emit("insights-updated", run_id);
    Ok(())
}

/// 为「我的承诺」创建单次提醒（09:00）并回链 finding。
/// 返回是否创建成功；任何失败都不影响 run 本身。
async fn create_commitment_reminder(
    app: &AppHandle,
    finding_id: i64,
    title: &str,
    note: &str,
    date: &str,
) -> Result<bool, String> {
    let Some(next_fire) = db::compute_next_fire("once", "09:00", None, Some(date)) else {
        return Ok(false);
    };
    with_db(app, |conn| {
        let rid = db::insert_reminder(
            conn,
            title,
            note,
            "once",
            "09:00",
            None,
            Some(date),
            &db::fmt_fire_time(&next_fire),
        )?;
        db::link_finding_reminder(conn, finding_id, rid)
    })?;
    Ok(true)
}

/// 拼装 LLM 用户消息三段：上次 state JSON、已有活跃发现、本次新深度解析；
/// 同时返回 ref 序号 → entry_id 的映射。
fn build_user_message(
    state_json: &str,
    active_findings: &[db::InsightsFindingRow],
    candidates: &[db::InsightsCandidate],
) -> (String, HashMap<i64, i64>) {
    let mut ref_map = HashMap::new();
    let sections: Vec<String> = candidates
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let n = i as i64 + 1;
            ref_map.insert(n, c.id);
            let date = c.created_at.get(..10).unwrap_or(&c.created_at);
            format!("[#{n}] 录制于 {date}（条目 {}）\n{}", c.id, c.deep_analysis)
        })
        .collect();
    let user = format!(
        "【上次状态】\n{state_json}\n\n【已有活跃发现】\n{}\n\n【深度解析】\n{}",
        active_findings_text(active_findings),
        sections.join("\n\n")
    );
    (user, ref_map)
}

/// 活跃发现清单文本（- [kind] title（date））；空则「无」。dismissed 不列入。
fn active_findings_text(rows: &[db::InsightsFindingRow]) -> String {
    let lines: Vec<String> = rows
        .iter()
        .filter(|r| r.status != "dismissed")
        .map(|r| {
            let date = r
                .event_date
                .as_deref()
                .or(r.created_at.get(..10))
                .unwrap_or("");
            format!("- [{}] {}（{}）", r.kind, r.title, date)
        })
        .collect();
    if lines.is_empty() {
        "无".to_string()
    } else {
        lines.join("\n")
    }
}

/// 从模型输出中抠出第一个括号配平的 JSON 对象（跳过字符串内部的括号与转义），
/// 找不到完整对象返回 None。
fn extract_json_object(raw: &str) -> Option<&str> {
    let start = raw.find('{')?;
    let mut depth = 0usize;
    let mut in_str = false;
    let mut escape = false;
    for (i, ch) in raw[start..].char_indices() {
        if in_str {
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                in_str = false;
            }
            continue;
        }
        match ch {
            '"' => in_str = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(&raw[start..=start + i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// 逐条校验模型输出的 findings：kind 合法、title 非空、event_date 必须是
/// YYYY-MM-DD（否则置 None）、ref 能映射则带 entry_id。不合法的整条跳过。
fn parse_findings(items: &[serde_json::Value], ref_map: &HashMap<i64, i64>) -> Vec<NewFinding> {
    let mut out = Vec::new();
    for item in items {
        let Some(kind) = item.get("kind").and_then(|k| k.as_str()) else {
            continue;
        };
        if !VALID_KINDS.contains(&kind) {
            continue;
        }
        let Some(title) = item
            .get("title")
            .and_then(|t| t.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let note = item
            .get("note")
            .and_then(|n| n.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let quote = item
            .get("quote")
            .and_then(|q| q.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let event_date = item
            .get("event_date")
            .and_then(|d| d.as_str())
            .map(str::trim)
            .filter(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok())
            .map(str::to_string);
        let entry_id = item
            .get("ref")
            .and_then(|r| r.as_i64())
            .and_then(|n| ref_map.get(&n).copied());
        out.push(NewFinding {
            kind: kind.to_string(),
            title: title.to_string(),
            note,
            quote,
            event_date,
            entry_id,
        });
    }
    out
}

/// 项目脉络兜底修剪：projects 里所有日期（thread[].date、open_items[].last_mention）
/// 的最大值距今超过 60 天的项目移入 archived（只保留名字）。
fn prune_state(state: &mut serde_json::Value) {
    let today = chrono::Local::now().date_naive();
    let mut stale_names: Vec<String> = Vec::new();
    if let Some(projects) = state.get_mut("projects").and_then(|v| v.as_array_mut()) {
        let taken = std::mem::take(projects);
        let mut kept = Vec::with_capacity(taken.len());
        for project in taken {
            let stale = project_last_date(&project)
                .map(|d| (today - d).num_days() > ARCHIVE_DAYS)
                .unwrap_or(false);
            if stale {
                if let Some(name) = project.get("name").and_then(|n| n.as_str()) {
                    stale_names.push(name.to_string());
                }
            } else {
                kept.push(project);
            }
        }
        *projects = kept;
    }
    if !stale_names.is_empty() {
        if let Some(obj) = state.as_object_mut() {
            let archived = obj
                .entry("archived".to_string())
                .or_insert_with(|| serde_json::json!([]));
            if let Some(arr) = archived.as_array_mut() {
                for name in stale_names {
                    arr.push(serde_json::Value::String(name));
                }
            }
        }
    }
}

/// 项目内所有提及日期的最大值；没有任何合法日期时返回 None（视为不修剪）。
fn project_last_date(project: &serde_json::Value) -> Option<chrono::NaiveDate> {
    let mut max: Option<chrono::NaiveDate> = None;
    let mut scan = |v: &serde_json::Value| {
        if let Some(s) = v.as_str() {
            if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
                let next = max.map_or(d, |m| m.max(d));
                max = Some(next);
            }
        }
    };
    if let Some(thread) = project.get("thread").and_then(|t| t.as_array()) {
        for item in thread {
            scan(&item["date"]);
        }
    }
    if let Some(items) = project.get("open_items").and_then(|t| t.as_array()) {
        for item in items {
            scan(&item["last_mention"]);
        }
    }
    max
}

/// 按 id 取一条发现（insights_to_reminder / insights_to_entry 共用）。
fn get_finding(
    conn: &rusqlite::Connection,
    id: i64,
) -> rusqlite::Result<Option<db::InsightsFindingRow>> {
    conn.query_row(
        "SELECT id, run_id, kind, title, note, quote, entry_id, event_date, status, reminder_id, created_at
         FROM recorder_analysis_findings WHERE id = ?1",
        rusqlite::params![id],
        |r| {
            Ok(db::InsightsFindingRow {
                id: r.get(0)?,
                run_id: r.get(1)?,
                kind: r.get(2)?,
                title: r.get(3)?,
                note: r.get(4)?,
                quote: r.get(5)?,
                entry_id: r.get(6)?,
                event_date: r.get(7)?,
                status: r.get(8)?,
                reminder_id: r.get(9)?,
                created_at: r.get(10)?,
            })
        },
    )
    .optional()
}

/// 同步执行一条数据库语句（照 tasks.rs 的 with_db 模式）。
fn with_db<T>(
    app: &AppHandle,
    f: impl FnOnce(&rusqlite::Connection) -> rusqlite::Result<T>,
) -> Result<T, String> {
    let state = app.state::<AppState>();
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    f(&conn).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: i64, text: &str) -> db::InsightsCandidate {
        db::InsightsCandidate {
            id,
            created_at: "2026-10-09 12:00:00".to_string(),
            deep_analysis: text.to_string(),
        }
    }

    fn finding(kind: &str, title: &str, status: &str, event_date: Option<&str>) -> db::InsightsFindingRow {
        db::InsightsFindingRow {
            id: 1,
            run_id: 1,
            kind: kind.to_string(),
            title: title.to_string(),
            note: String::new(),
            quote: String::new(),
            entry_id: None,
            event_date: event_date.map(str::to_string),
            status: status.to_string(),
            reminder_id: None,
            created_at: "2026-10-09 08:00:00".to_string(),
        }
    }

    #[test]
    fn build_user_message_has_three_sections_and_ref_map() {
        let cands = [candidate(101, "深度解析A"), candidate(102, "深度解析B")];
        let active = [finding("idea", "旧灵感", "new", Some("2026-10-01"))];
        let (user, ref_map) = build_user_message("{\"projects\":[]}", &active, &cands);
        assert!(user.contains("【上次状态】\n{\"projects\":[]}"));
        assert!(user.contains("【已有活跃发现】\n- [idea] 旧灵感（2026-10-01）"));
        assert!(user.contains("[#1] 录制于 2026-10-09（条目 101）\n深度解析A"));
        assert!(user.contains("[#2] 录制于 2026-10-09（条目 102）\n深度解析B"));
        assert_eq!(ref_map.get(&1), Some(&101));
        assert_eq!(ref_map.get(&2), Some(&102));
    }

    #[test]
    fn active_findings_text_empty_is_none_word_and_skips_dismissed() {
        assert_eq!(active_findings_text(&[]), "无");
        let rows = [
            finding("idea", "活跃", "new", Some("2026-10-02")),
            finding("idea", "已完成", "done", None),
            finding("idea", "已忽略", "dismissed", Some("2026-10-03")),
        ];
        let text = active_findings_text(&rows);
        assert!(text.contains("- [idea] 活跃（2026-10-02）"));
        assert!(text.contains("- [idea] 已完成（2026-10-09）"), "无 event_date 用 created_at");
        assert!(!text.contains("已忽略"), "dismissed 不列入活跃发现");
    }

    #[test]
    fn parse_findings_maps_ref_and_validates() {
        let mut ref_map = HashMap::new();
        ref_map.insert(1, 101i64);
        let items = serde_json::json!([
            {"kind": "commitment_mine", "title": " 写周报 ", "note": "n", "quote": "q",
             "event_date": "2026-10-10", "ref": 1},
            {"kind": "idea", "title": "无日期灵感", "event_date": "不是日期", "ref": 9},
            {"kind": "bad_kind", "title": "坏类型"},
            {"kind": "idea", "title": "   "},
            {"kind": "open_loop", "title": "旧事项", "ref": 1}
        ]);
        let arr = items.as_array().unwrap();
        let out = parse_findings(arr, &ref_map);
        assert_eq!(out.len(), 3, "坏 kind 与空 title 被跳过");
        assert_eq!(out[0].title, "写周报");
        assert_eq!(out[0].event_date.as_deref(), Some("2026-10-10"));
        assert_eq!(out[0].entry_id, Some(101));
        assert_eq!(out[1].event_date, None, "非法日期置 None");
        assert_eq!(out[1].entry_id, None, "ref 序号无映射则不带 entry_id");
        assert_eq!(out[2].kind, "open_loop");
        assert_eq!(out[2].entry_id, Some(101));
    }

    fn project(name: &str, thread_date: &str, last_mention: &str) -> serde_json::Value {
        serde_json::json!({
            "name": name,
            "thread": [{"date": thread_date, "note": "提及"}],
            "open_items": [{"item": "x", "origin_date": thread_date, "last_mention": last_mention, "reported": false}]
        })
    }

    #[test]
    fn prune_state_archives_projects_stale_over_60_days() {
        let today = chrono::Local::now().date_naive();
        let old = (today - chrono::Duration::days(61)).format("%Y-%m-%d").to_string();
        let recent = (today - chrono::Duration::days(30)).format("%Y-%m-%d").to_string();
        let mut state = serde_json::json!({
            "projects": [project("旧项目", &old, &old), project("活跃项目", &recent, &recent)],
            "archived": []
        });
        prune_state(&mut state);
        let projects = state["projects"].as_array().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0]["name"], "活跃项目");
        let archived = state["archived"].as_array().unwrap();
        assert_eq!(archived.len(), 1);
        assert_eq!(archived[0], "旧项目", "archived 只保留名字");
    }

    #[test]
    fn prune_state_uses_latest_of_all_dates() {
        let today = chrono::Local::now().date_naive();
        let old_thread = (today - chrono::Duration::days(90)).format("%Y-%m-%d").to_string();
        let recent_mention = (today - chrono::Duration::days(5)).format("%Y-%m-%d").to_string();
        // thread 很旧但 open_items 最近提及 → 不归档
        let mut state = serde_json::json!({
            "projects": [project("项目X", &old_thread, &recent_mention)],
            "archived": []
        });
        prune_state(&mut state);
        assert_eq!(state["projects"].as_array().unwrap().len(), 1);
        assert!(state["archived"].as_array().unwrap().is_empty());
    }

    #[test]
    fn prune_state_no_projects_is_noop() {
        let mut state = serde_json::json!({"archived": ["旧"]});
        prune_state(&mut state);
        assert_eq!(state["archived"].as_array().unwrap().len(), 1);
        assert!(state.get("projects").is_none());
    }

    #[test]
    fn extract_json_object_takes_first_balanced_object() {
        // 前后有多余文字 + JSON 后跟着带括号的散文（旧「首{到尾}」会把它卷进来）
        let raw = r#"好的，结果如下：
{"findings":[{"kind":"idea","title":"想法"}],"state":{"projects":[]}}
以上是基于录音的洞察。补充说明：{"备用":"对象"} 不必理会。"#;
        let out = extract_json_object(raw).unwrap();
        let v: serde_json::Value = serde_json::from_str(out).unwrap();
        assert_eq!(v["findings"][0]["title"], "想法");
        assert!(!out.contains("备用"), "第二个对象和散文不能被卷进来");
    }

    #[test]
    fn extract_json_object_skips_braces_in_strings() {
        let raw = r#"{"note":"他说 {明天} 再来，引号 \" 转义","n":1}"#;
        let out = extract_json_object(raw).unwrap();
        let v: serde_json::Value = serde_json::from_str(out).unwrap();
        assert_eq!(v["n"], 1);
    }

    #[test]
    fn extract_json_object_handles_truncated_and_empty() {
        assert!(extract_json_object("没有对象").is_none());
        assert!(extract_json_object("").is_none());
        assert!(extract_json_object("{\"a\":1").is_none(), "缺右括号返回 None");
    }
}
