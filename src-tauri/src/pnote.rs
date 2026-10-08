//! 录音卡常驻 sidecar 管理：进程生命周期、JSON-lines 请求/事件协议、控制命令。
//!
//! sidecar（pnote-sidecar/pnote.exe，PyInstaller 冻结）通过 stdin 收请求、stdout 推
//! 响应与事件、stderr 打 `MASAIFU_LOG` 诊断日志。协议见
//! docs/plans/2026-10-08-recorder-implementation.md「协议定义」。

use crate::db;
use crate::meeting;
use crate::openai::OpenAiClient;
use crate::AppState;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::oneshot;

const READY_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
// 全量同步下载 12MB 约 11 分钟，给 20 分钟余量
const SYNC_TIMEOUT: Duration = Duration::from_secs(20 * 60);

/// 录音卡设备状态快照；字段与前端 types.ts 的 PnoteStatus 一致（camelCase）。
/// 注意：协议里 battery==110 表示充电中（展示为 100% + 充电标记）。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PnoteStatus {
    pub running: bool,
    pub connected: bool,
    pub battery: Option<i64>,
    pub charging: bool,
    pub capacity: Option<[i64; 2]>,
    pub version: Option<String>,
    pub sleeping: bool,
    // sidecar 的 status 响应只有设备字段、不含 pending（本地待转写计数由 pnote_status 补）
    #[serde(default)]
    pub pending: i64,
}

/// 录音卡组件（PyInstaller 冻结的 pnote-sidecar）路径。
/// 打包后位于资源目录，开发时位于 src-tauri/resources/ 下。
fn pnote_sidecar_exe(app: &AppHandle) -> Result<PathBuf, String> {
    let name = if cfg!(windows) { "pnote.exe" } else { "pnote" };
    if let Ok(dir) = app.path().resource_dir() {
        let p = dir.join("pnote-sidecar").join(name);
        if p.is_file() {
            return Ok(p);
        }
    }
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("resources")
        .join("pnote-sidecar")
        .join(name);
    if dev.is_file() {
        return Ok(dev);
    }
    Err("录音卡组件未构建，见 tools/build_sidecar.md".into())
}

/// 设备状态查询（合并本地待转写计数）。
#[tauri::command]
pub async fn pnote_status(app: AppHandle) -> Result<PnoteStatus, String> {
    let enabled = {
        let state = app.state::<AppState>();
        let settings = state.settings.lock().map_err(|e| e.to_string())?;
        settings.pnote_enabled
    };
    if !enabled {
        // 未启用时不拉起 sidecar，直接回默认状态
        return Ok(PnoteStatus::default());
    }
    let mut status: PnoteStatus =
        pnote_request(&app, "status", serde_json::json!({}), REQUEST_TIMEOUT).await?;
    let pending = {
        let state = app.state::<AppState>();
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        db::recorder_pending_count(&conn).map_err(|e| e.to_string())?
    };
    status.pending = pending;
    // 回写快照，让后续 pnote-state 事件带上较新的 pending
    if let Ok(mut latest) = app.state::<AppState>().pnote_latest.lock() {
        *latest = status.clone();
    }
    Ok(status)
}

/// 立即跑一轮同步。响应可能很慢（下载 12MB 约 11 分钟），超时给 20 分钟。
/// 响应返回后顺带重试此前转写失败且 ogg 仍在本地的记录。
#[tauri::command]
pub async fn pnote_sync_now(app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    if state.pnote_busy.swap(true, Ordering::SeqCst) {
        return Err("已有录音卡同步任务进行中".into());
    }
    let result: Result<serde_json::Value, String> =
        pnote_request(&app, "sync_now", serde_json::json!({}), SYNC_TIMEOUT).await;
    state.pnote_busy.store(false, Ordering::SeqCst);
    for (name, ogg, seconds, size) in failed_records(&app) {
        spawn_transcribe(app.clone(), name, ogg, seconds, size);
    }
    result.map(|_| ())
}

/// 设备端文件列表。
#[tauri::command]
pub async fn pnote_file_list(app: AppHandle) -> Result<serde_json::Value, String> {
    pnote_request(&app, "file_list", serde_json::json!({}), REQUEST_TIMEOUT).await
}

/// 启用/停用录音卡同步。启用即拉起 sidecar 并启动守护循环；停用发 stop 后 kill。
/// 照 commands.rs save_settings 的写法：锁 settings、改字段、调 .save。
#[tauri::command]
pub async fn pnote_set_enabled(app: AppHandle, enabled: bool) -> Result<(), String> {
    {
        let state = app.state::<AppState>();
        let path = state.settings_path.clone();
        let mut settings = state.settings.lock().map_err(|e| e.to_string())?;
        settings.pnote_enabled = enabled;
        let saved = settings.save(&path);
        saved?;
    }
    if enabled {
        ensure_sidecar(&app).await?;
        pnote_request::<serde_json::Value>(&app, "start", serde_json::json!({}), REQUEST_TIMEOUT)
            .await?;
    } else {
        stop_and_kill(&app).await;
    }
    Ok(())
}

/// 一键分析最近 7 天的录音转写，提取值得提醒的未来事件（承诺、待办、约定、
/// deadline、账单、预约、要跟进的人和事）并创建为单次提醒（09:00 触发）。
/// 返回创建的提醒条数；分析提示词在「设置 → 录音卡」中可修改。
#[tauri::command]
pub async fn analyze_recorder_events(app: AppHandle) -> Result<i64, String> {
    let settings = {
        let state = app.state::<AppState>();
        let s = state.settings.lock().map_err(|e| e.to_string())?.clone();
        s
    };
    if !settings.api_key_present() {
        return Err("请先在「设置 → AI 服务」中配置 API Key".into());
    }
    let entries = with_db(&app, |conn| db::recorder_entries_since(conn, 7))?;
    if entries.is_empty() {
        return Err("最近 7 天没有录音记录".into());
    }
    let tomorrow = (chrono::Local::now() + chrono::Duration::days(1))
        .format("%Y-%m-%d")
        .to_string();
    let user = entries
        .iter()
        .map(|e| {
            let text = e
                .clean_content
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(&e.raw_content);
            format!("[录制于 {}]\n{text}", &e.created_at[..10])
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let client = OpenAiClient::new(&settings.base_url, &settings.api_key, &settings.model);
    let raw = client
        .recorder_events(&user, &settings.recorder_event_prompt)
        .await
        .map_err(|e| format!("AI 分析失败：{e}"))?;
    // 模型可能在 JSON 前后输出多余文字：抠出第一个 '[' 到最后一个 ']' 的数组
    let start = raw
        .find('[')
        .ok_or_else(|| "AI 返回内容中没有找到 JSON 数组".to_string())?;
    let end = raw
        .rfind(']')
        .filter(|&i| i >= start)
        .ok_or_else(|| "AI 返回内容中没有找到完整的 JSON 数组".to_string())?;
    let value: serde_json::Value = serde_json::from_str(&raw[start..=end])
        .map_err(|e| format!("AI 返回的 JSON 解析失败：{e}"))?;
    let items = value
        .as_array()
        .ok_or_else(|| "AI 返回的 JSON 不是数组".to_string())?;
    let mut created = 0i64;
    for item in items {
        let Some(title) = item["title"].as_str().map(str::trim).filter(|s| !s.is_empty()) else {
            continue;
        };
        let note = item["note"].as_str().unwrap_or("").trim().to_string();
        let date = item["date"]
            .as_str()
            .map(str::trim)
            .filter(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok())
            .map(str::to_string)
            .unwrap_or_else(|| tomorrow.clone());
        let next_fire = db::compute_next_fire("once", "09:00", None, Some(&date))
            .ok_or_else(|| "计算提醒触发时间失败".to_string())?;
        with_db(&app, |conn| {
            db::insert_reminder(
                conn,
                title,
                &note,
                "once",
                "09:00",
                None,
                Some(&date),
                &db::fmt_fire_time(&next_fire),
            )
        })?;
        created += 1;
    }
    Ok(created)
}

/// sidecar 不存在或已退出时重新 spawn；本次新起的实例等 ready 事件（10s 超时）。
pub async fn ensure_sidecar(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    // 串行化 spawn，防并发 command 拉起两个实例
    let _spawn_guard = state.pnote_spawn_lock.lock().await;

    let needs_spawn = {
        let mut guard = state.pnote_child.lock().map_err(|e| e.to_string())?;
        match guard.as_mut() {
            Some(c) => matches!(c.try_wait(), Ok(Some(_)) | Err(_)),
            None => true,
        }
    };
    if needs_spawn {
        let ready_rx = spawn_inner(app).await?;
        tokio::time::timeout(READY_TIMEOUT, ready_rx)
            .await
            .map_err(|_| "录音卡组件启动超时".to_string())?
            .map_err(|_| "录音卡组件启动中断".to_string())?;
    }
    Ok(())
}

/// 启动 sidecar 并登记句柄；返回 ready 信号的接收端（本实例 stdout 出现
/// `{"type":"event","event":"ready",...}` 时触发）。崩溃后由下一次 ensure_sidecar 重新拉起。
async fn spawn_inner(app: &AppHandle) -> Result<oneshot::Receiver<()>, String> {
    let exe = pnote_sidecar_exe(app)?;
    let out_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("recorder-audio");
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    let ffmpeg = {
        let state = app.state::<AppState>();
        let settings = state.settings.lock().map_err(|e| e.to_string())?;
        settings.pnote_ffmpeg_path.clone()
    };

    let mut cmd = tokio::process::Command::new(&exe);
    cmd.arg("--out-dir")
        .arg(&out_dir)
        .arg("--ffmpeg")
        .arg(&ffmpeg)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // Windows 下 GUI 程序拉起控制台子进程会闪出一个黑窗口，CREATE_NO_WINDOW 抑制
    #[cfg(windows)]
    cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("录音卡组件启动失败: {e}"))?;

    let stdin = child.stdin.take().ok_or("无法写入录音卡组件 stdin")?;
    let stdout = child.stdout.take().ok_or("无法读取录音卡组件 stdout")?;
    let stderr = child.stderr.take().ok_or("无法读取录音卡组件 stderr")?;

    let state = app.state::<AppState>();
    // 代次 +1：旧监控任务（含残留子进程句柄、挂起请求）看到代次不符即退出
    let gen = state.pnote_gen.fetch_add(1, Ordering::SeqCst) + 1;

    // 回收上一个实例的残留：清挂起请求（发不出去响应了），句柄稍后覆盖
    if let Ok(mut pending) = state.pnote_pending.lock() {
        pending.clear();
    }

    let (ready_tx, ready_rx) = oneshot::channel::<()>();
    {
        let mut old = None;
        {
            let mut guard = state.pnote_child.lock().map_err(|e| e.to_string())?;
            std::mem::swap(&mut old, &mut *guard);
            *guard = Some(child);
        }
        *state.pnote_stdin.lock().await = Some(stdin);
        *state.pnote_ready.lock().map_err(|e| e.to_string())? = Some(ready_tx);
        // 上一个实例若有残留句柄（已被取代但还没被监控回收），后台 reap
        if let Some(mut old_child) = old {
            tokio::spawn(async move {
                let _ = old_child.wait().await;
            });
        }
    }

    let app_out = app.clone();
    tokio::spawn(async move {
        read_stdout_loop(app_out, stdout).await;
    });
    tokio::spawn(async move {
        read_stderr_loop(stderr).await;
    });

    // 崩溃监控：轮询子进程状态，退出时清句柄 + 通知前端断开；下次 ensure 重新拉起
    let app_mon = app.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let state = app_mon.state::<AppState>();
            if state.pnote_gen.load(Ordering::SeqCst) != gen {
                return; // 已被新实例取代（或被手动重启），本任务退场
            }
            let mut exited = None;
            {
                if let Ok(mut guard) = state.pnote_child.lock() {
                    if let Some(c) = guard.as_mut() {
                        if matches!(c.try_wait(), Ok(Some(_)) | Err(_)) {
                            exited = guard.take();
                        }
                    }
                }
            } // std MutexGuard 不能跨 await，回收 wait 前先 drop
            if let Some(mut child) = exited {
                let _ = child.wait().await;
                if state.pnote_gen.load(Ordering::SeqCst) != gen {
                    return;
                }
                *state.pnote_stdin.lock().await = None;
                let snapshot = {
                    if let Ok(mut latest) = state.pnote_latest.lock() {
                        latest.running = false;
                        latest.connected = false;
                        latest.sleeping = false;
                        latest.clone()
                    } else {
                        PnoteStatus::default()
                    }
                };
                log::warn!("录音卡组件已退出（崩溃或被终止），下次调用将自动重启");
                let _ = app_mon.emit("pnote-state", snapshot);
                return;
            }
        }
    });

    Ok(ready_rx)
}

/// stdout 逐行读 JSON：resp 按 id 分发给挂起的 oneshot；event 更新快照并发前端。
async fn read_stdout_loop(app: AppHandle, stdout: tokio::process::ChildStdout) {
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let v: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("resp") => {
                let id = v.get("id").and_then(|i| i.as_u64()).unwrap_or(0);
                let state = app.state::<AppState>();
                let tx = state
                    .pnote_pending
                    .lock()
                    .ok()
                    .and_then(|mut m| m.remove(&id));
                if let Some(tx) = tx {
                    let _ = tx.send(v); // 接收端已超时丢弃则忽略
                }
            }
            Some("event") => handle_event(&app, v).await,
            _ => {}
        }
    }
}

/// file_synced 事件的 data 字段（name/ogg 必填，seconds/size 缺省按 0 处理）。
#[derive(Deserialize)]
struct FileSyncedEvent {
    name: String,
    ogg: String,
    #[serde(default)]
    seconds: f64,
    #[serde(default)]
    size: i64,
}

async fn handle_event(app: &AppHandle, v: serde_json::Value) {
    let event = v.get("event").and_then(|e| e.as_str()).unwrap_or("");
    let data = v.get("data").cloned().unwrap_or(serde_json::Value::Null);
    match event {
        "ready" => {
            let state = app.state::<AppState>();
            let tx = {
                let mut g = match state.pnote_ready.lock() {
                    Ok(g) => g,
                    Err(_) => return,
                };
                g.take()
            };
            if let Some(tx) = tx {
                let _ = tx.send(());
            }
        }
        "state" => {
            let state = app.state::<AppState>();
            let snapshot = {
                let mut latest = match state.pnote_latest.lock() {
                    Ok(g) => g,
                    Err(_) => return,
                };
                merge_state_event(&mut latest, &data);
                latest.clone()
            };
            let _ = app.emit("pnote-state", snapshot);
            maybe_trigger_device_gc(app);
        }
        // file_synced：OGG 已落盘 → 先同步登记 done（ogg_path 阶段），再异步转写入库
        "file_synced" => {
            let ev: FileSyncedEvent = match serde_json::from_value(data) {
                Ok(e) => e,
                Err(e) => {
                    log::warn!("录音卡 file_synced 事件格式错误: {e}");
                    return;
                }
            };
            let seconds = ev.seconds.round() as i64;
            // 同步落库必须在 spawn 前完成：sidecar 按本地存在性跳过已收文件，
            // 这条记录是后续转写/重试的唯一凭据
            if let Err(e) = with_db(app, |conn| {
                db::recorder_file_upsert(conn, &ev.name, ev.size, seconds, "done")?;
                db::recorder_file_mark(conn, &ev.name, "done", None, Some(&ev.ogg))
            }) {
                log::warn!("录音卡文件登记失败（{}）: {e}", ev.name);
            }
            spawn_transcribe(app.clone(), ev.name, ev.ogg, seconds, ev.size);
        }
        "progress" => {
            let _ = app.emit("pnote-progress", data);
        }
        _ => {}
    }
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

/// status='failed' 且 ogg 文件仍在本地的记录，供 sync_now 后重试转写。
fn failed_records(app: &AppHandle) -> Vec<(String, String, i64, i64)> {
    let rows = with_db(app, |conn| {
        let mut stmt = conn.prepare(
            "SELECT name, ogg_path, seconds, size FROM recorder_files WHERE status = 'failed'",
        )?;
        let mapped = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })?;
        mapped.collect::<rusqlite::Result<Vec<_>>>()
    })
    .unwrap_or_default();
    rows.into_iter()
        .filter_map(|(name, ogg, seconds, size)| match ogg {
            Some(p) if Path::new(&p).is_file() => Some((name, p, seconds, size)),
            _ => None,
        })
        .collect()
}

/// 排队一个录音卡转写任务（file_synced 事件与 sync_now 失败重试共用）。
/// pnote_busy 拿不到（同步/转写进行中）则 5 秒重试、最多等 10 分钟，保证任一时刻
/// 只有一个转写任务在跑。size 仅在事件入口落库时使用，此处无需再登记。
fn spawn_transcribe(app: AppHandle, name: String, ogg_path: String, seconds: i64, size: i64) {
    let _ = size;
    tauri::async_runtime::spawn(async move {
        let result = async {
            {
                let state = app.state::<AppState>();
                let mut waited = 0u64;
                while state.pnote_busy.swap(true, Ordering::SeqCst) {
                    if waited >= 600 {
                        return Err("录音卡转写排队超时（同步/转写占用超过 10 分钟）".to_string());
                    }
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    waited += 5;
                }
            }
            let outcome = transcribe_recorder_file(&app, &name, &ogg_path, seconds).await;
            app.state::<AppState>()
                .pnote_busy
                .store(false, Ordering::SeqCst);
            outcome
        }
        .await;
        if let Err(e) = result {
            log::warn!("录音卡转写入库失败（{name}）: {e}");
            // ogg 与设备原件都保留，标 failed 后由下次 sync_now 重试
            let _ = with_db(&app, |conn| {
                db::recorder_file_mark(conn, &name, "failed", None, None)
            });
        }
    });
}

/// 转写单条录音并写入 entries（source = recorder）。不生成 AI 整理，直接原文入库
/// （录音卡多为短条目，格式前缀已带时长与文件名）。转写前等 meeting_busy 释放，
/// 避免与会议导入两个 funasr sidecar 并发（5 秒重试，最多 10 分钟）。
async fn transcribe_recorder_file(
    app: &AppHandle,
    name: &str,
    ogg_path: &str,
    seconds: i64,
) -> Result<(), String> {
    {
        let state = app.state::<AppState>();
        let mut waited = 0u64;
        while state.meeting_busy.load(Ordering::SeqCst) {
            if waited >= 600 {
                return Err("会议导入占用语音识别超过 10 分钟，本次转写放弃".into());
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
            waited += 5;
        }
    }
    let transcript = meeting::transcribe_audio(app, Path::new(ogg_path)).await?;
    let text = format!("（时长约 {seconds} 秒｜文件名 {name}）\n\n{transcript}");
    let dedup_daily = {
        let state = app.state::<AppState>();
        let s = state.settings.lock().map_err(|e| e.to_string())?;
        s.dedup_scope != "all"
    };
    let entry_id = with_db(app, |conn| {
        db::insert_entry(conn, &text, "recorder", dedup_daily)
    })?;
    // 重复内容（entry_id=None）也标 done，entry_id 留空即可
    with_db(app, |conn| {
        db::recorder_file_mark(conn, name, "done", entry_id, None)
    })?;
    let _ = app.emit(
        "pnote-synced",
        serde_json::json!({"newEntries": if entry_id.is_some() { 1 } else { 0 }, "name": name}),
    );
    Ok(())
}

/// 把 state 事件的 data 逐个字段合并进快照；battery==110 表示充电中。
/// 逐字段容错：单个字段类型不对只跳过该字段并记 warn（附原始数据），其余字段照常合并；
/// Option 字段收到 null 表示清除（sidecar 断连时推 battery/capacity/version=None）。
fn merge_state_event(latest: &mut PnoteStatus, data: &serde_json::Value) {
    let obj = match data.as_object() {
        Some(o) => o,
        None => {
            log::warn!("录音卡 state 事件 data 不是对象，已忽略: {data}");
            return;
        }
    };
    let mut bad: Vec<&str> = Vec::new();
    for (key, value) in [
        ("running", &mut latest.running),
        ("connected", &mut latest.connected),
        ("sleeping", &mut latest.sleeping),
    ] {
        match obj.get(key) {
            Some(v) => match v.as_bool() {
                Some(b) => *value = b,
                None => bad.push(key),
            },
            None => {}
        }
    }
    match obj.get("battery") {
        Some(v) if v.is_null() => latest.battery = None,
        Some(v) => match v.as_i64() {
            Some(b) if b >= 110 => {
                latest.charging = true;
                latest.battery = Some(100);
            }
            Some(b) => latest.battery = Some(b),
            None => bad.push("battery"),
        },
        None => {}
    }
    // 事件里显式的 charging 最后应用，可覆盖 battery>=110 的推断
    match obj.get("charging") {
        Some(v) => match v.as_bool() {
            Some(b) => latest.charging = b,
            None => bad.push("charging"),
        },
        None => {}
    }
    match obj.get("capacity") {
        Some(v) if v.is_null() => latest.capacity = None,
        Some(v) => {
            let pair = v.as_array().and_then(|a| match a.as_slice() {
                [x, y] => match (x.as_i64(), y.as_i64()) {
                    (Some(x), Some(y)) => Some([x, y]),
                    _ => None,
                },
                _ => None,
            });
            match pair {
                Some(c) => latest.capacity = Some(c),
                None => bad.push("capacity"),
            }
        }
        None => {}
    }
    match obj.get("version") {
        Some(v) if v.is_null() => latest.version = None,
        Some(v) => match v.as_str() {
            Some(s) => latest.version = Some(s.to_string()),
            None => bad.push("version"),
        },
        None => {}
    }
    if !bad.is_empty() {
        log::warn!(
            "录音卡 state 事件字段格式异常（已跳过 {}）: {data}",
            bad.join(", ")
        );
    }
}

/// 设备容量不足时向 sidecar 发 gc_device，清掉设备上早已同步完成的录音。
/// 候选取本地 status='done' 且 ogg 落盘超过 30 天的记录（设备端阈值，与保留天数设置无关）。
pub async fn maybe_gc_device(app: &AppHandle) {
    let capacity = {
        let state = app.state::<AppState>();
        state
            .pnote_latest
            .lock()
            .ok()
            .and_then(|latest| latest.capacity)
    };
    let Some([remain, total]) = capacity else { return };
    if total <= 0 || remain * 2 >= total {
        return;
    }
    let candidates = match with_db(app, |conn| db::recorder_files_done_before(conn, 30)) {
        Ok(c) => c,
        Err(e) => {
            log::warn!("录音卡设备清理候选查询失败: {e}");
            return;
        }
    };
    if candidates.is_empty() {
        return;
    }
    match pnote_request::<serde_json::Value>(
        app,
        "gc_device",
        serde_json::json!({ "names": candidates }),
        REQUEST_TIMEOUT,
    )
    .await
    {
        Ok(data) => {
            let deleted = data.get("deleted").and_then(|v| v.as_u64()).unwrap_or(0);
            log::info!("录音卡设备清理：删除 {deleted} 个已同步的旧录音");
        }
        Err(e) => log::warn!("录音卡设备清理失败: {e}"),
    }
}

/// state 事件触发点：容量低于一半时按天 debounce（同一天最多一次），
/// 后台 spawn 执行，不阻塞事件循环。
fn maybe_trigger_device_gc(app: &AppHandle) {
    let low = {
        let state = app.state::<AppState>();
        state
            .pnote_latest
            .lock()
            .ok()
            .and_then(|latest| latest.capacity)
            .map(|[remain, total]| total > 0 && remain * 2 < total)
            .unwrap_or(false)
    };
    if !low {
        return;
    }
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    {
        let state = app.state::<AppState>();
        let mut last = match state.pnote_last_gc.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        if last.as_deref() == Some(today.as_str()) {
            return;
        }
        *last = Some(today);
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        maybe_gc_device(&app).await;
    });
}

/// stderr 排空：协议行进日志，其余行保留尾部 6 行做崩溃诊断。
async fn read_stderr_loop(stderr: tokio::process::ChildStderr) {
    let mut lines = BufReader::new(stderr).lines();
    let mut tail: Vec<String> = Vec::new();
    while let Ok(Some(line)) = lines.next_line().await {
        if let Some(msg) = line.strip_prefix("MASAIFU_LOG ") {
            log::info!("[pnote] {msg}");
        } else if !line.starts_with("MASAIFU_") && !line.trim().is_empty() {
            tail.push(line);
            if tail.len() > 6 {
                tail.remove(0);
            }
        }
    }
    if !tail.is_empty() {
        log::warn!("录音卡组件 stderr 尾部: {}", tail.join(" | "));
    }
}

/// 发一个 JSON-lines 请求并等响应；extra 为除 id/cmd 外的字段（须为对象）。
async fn pnote_request<T: DeserializeOwned>(
    app: &AppHandle,
    cmd: &str,
    extra: serde_json::Value,
    timeout: Duration,
) -> Result<T, String> {
    ensure_sidecar(app).await?;
    let state = app.state::<AppState>();
    let id = state.pnote_req_seq.fetch_add(1, Ordering::SeqCst) + 1;

    let (tx, rx) = oneshot::channel();
    state
        .pnote_pending
        .lock()
        .map_err(|e| e.to_string())?
        .insert(id, tx);

    let mut req = match extra {
        serde_json::Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    req.insert("id".into(), serde_json::json!(id));
    req.insert("cmd".into(), serde_json::json!(cmd));
    let mut line = serde_json::Value::Object(req).to_string();
    line.push('\n');
    {
        // tokio Mutex 守卫可跨 await 持有；写期间 sidecar 句柄不会被并发清掉
        let mut guard = state.pnote_stdin.lock().await;
        let stdin = guard
            .as_mut()
            .ok_or_else(|| "录音卡组件未运行".to_string())?;
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("录音卡组件写入失败: {e}"))?;
        stdin.flush().await.map_err(|e| e.to_string())?;
    }

    let resp = tokio::time::timeout(timeout, rx)
        .await
        .map_err(|_| format!("录音卡组件响应超时（{cmd}）"))?
        .map_err(|_| "录音卡组件连接中断".to_string())?;
    if resp.get("ok").and_then(|v| v.as_bool()) == Some(true) {
        serde_json::from_value(resp.get("data").cloned().unwrap_or(serde_json::Value::Null))
            .map_err(|e| format!("录音卡组件响应格式错误: {e}"))
    } else {
        Err(resp
            .get("error")
            .and_then(|e| e.as_str())
            .unwrap_or("未知错误")
            .to_string())
    }
}

/// 发 stop 请求（尽力而为）并 kill 子进程；清空句柄由监控任务接力完成。
async fn stop_and_kill(app: &AppHandle) {
    let state = app.state::<AppState>();
    let _spawn_guard = state.pnote_spawn_lock.lock().await;
    {
        let mut guard = state.pnote_stdin.lock().await;
        if let Some(stdin) = guard.as_mut() {
            let id = state.pnote_req_seq.fetch_add(1, Ordering::SeqCst) + 1;
            let line = format!("{{\"id\":{id},\"cmd\":\"stop\"}}\n");
            let _ = stdin.write_all(line.as_bytes()).await;
            let _ = stdin.flush().await;
        }
    }
    // 稍等优雅退出，超时就地 kill
    tokio::time::sleep(Duration::from_millis(300)).await;
    let taken = state
        .pnote_child
        .lock()
        .ok()
        .and_then(|mut guard| guard.take());
    *state.pnote_stdin.lock().await = None;
    if let Some(mut child) = taken {
        let _ = child.start_kill();
        tokio::spawn(async move {
            let _ = child.wait().await;
        });
    }
    if let Ok(mut latest) = state.pnote_latest.lock() {
        latest.running = false;
        latest.connected = false;
        latest.sleeping = false;
    }
    let _ = app.emit("pnote-state", PnoteStatus::default());
}


#[cfg(test)]
mod tests {
    use super::*;

    /// 连接成功后 sidecar 实推的 state 事件形状（_refresh_device_info 全量 7 字段）
    #[test]
    fn merge_state_event_merges_real_shape() {
        let mut latest = PnoteStatus::default();
        let data = serde_json::json!({
            "running": true,
            "connected": true,
            "sleeping": false,
            "battery": 74,
            "charging": false,
            "capacity": [30498848, 30525408],
            "version": "V1.0.0",
        });
        merge_state_event(&mut latest, &data);
        assert!(latest.running);
        assert!(latest.connected, "connected=true 必须合并进去");
        assert!(!latest.sleeping);
        assert_eq!(latest.battery, Some(74));
        assert!(!latest.charging);
        assert_eq!(latest.capacity, Some([30498848, 30525408]));
        assert_eq!(latest.version.as_deref(), Some("V1.0.0"));
    }

    /// 单个字段畸形（capacity 长度不对、battery 是字符串）不影响其余字段合并
    #[test]
    fn merge_state_event_tolerates_bad_fields() {
        let mut latest = PnoteStatus::default();
        let data = serde_json::json!({
            "running": true,
            "connected": true,
            "sleeping": false,
            "battery": "74",
            "charging": false,
            "capacity": [30498848, 30525408, 3],
            "version": "V1.0.0",
        });
        merge_state_event(&mut latest, &data);
        assert!(latest.connected, "connected 仍应正确合并");
        assert!(latest.running);
        assert_eq!(latest.battery, None, "畸形 battery 应跳过");
        assert_eq!(latest.capacity, None, "长度!=2 的 capacity 应跳过");
        assert_eq!(latest.version.as_deref(), Some("V1.0.0"));
    }

    /// battery==110 表示充电中：置 charging 并展示 100%
    #[test]
    fn merge_state_event_battery_110_means_charging() {
        let mut latest = PnoteStatus::default();
        merge_state_event(&mut latest, &serde_json::json!({"battery": 110}));
        assert_eq!(latest.battery, Some(100));
        assert!(latest.charging);
    }

    /// 同一事件里显式 charging 最后应用，覆盖 battery>=110 的推断（沿用旧顺序）
    #[test]
    fn merge_state_event_explicit_charging_overrides_battery_inference() {
        let mut latest = PnoteStatus::default();
        merge_state_event(
            &mut latest,
            &serde_json::json!({"battery": 110, "charging": false}),
        );
        assert_eq!(latest.battery, Some(100));
        assert!(!latest.charging);
    }

    /// 断连时 sidecar 推 battery/capacity/version=null：清除旧值，其余字段照常
    #[test]
    fn merge_state_event_null_clears_option_fields() {
        let mut latest = PnoteStatus {
            connected: true,
            battery: Some(74),
            capacity: Some([30498848, 30525408]),
            version: Some("V1.0.0".into()),
            ..Default::default()
        };
        merge_state_event(
            &mut latest,
            &serde_json::json!({
                "connected": false, "battery": null, "capacity": null, "version": null,
            }),
        );
        assert!(!latest.connected);
        assert_eq!(latest.battery, None);
        assert_eq!(latest.capacity, None);
        assert_eq!(latest.version, None);
    }

    /// data 不是对象时不 panic、不改状态
    #[test]
    fn merge_state_event_ignores_non_object_data() {
        let mut latest = PnoteStatus {
            connected: true,
            ..Default::default()
        };
        merge_state_event(&mut latest, &serde_json::json!("connected"));
        assert!(latest.connected);
    }

    /// sidecar status 响应不含 pending 字段（pending 是本地计数），必须能反序列化——
    /// 这是面板拿不到已连接快照的根因之一：pnote_status 命令此前因此 100% 失败
    #[test]
    fn pnote_status_response_deserializes_without_pending() {
        let data = serde_json::json!({
            "running": true,
            "connected": true,
            "battery": 74,
            "charging": false,
            "capacity": [30498848, 30525408],
            "version": "V1.0.0",
            "sleeping": false,
        });
        let s: PnoteStatus = serde_json::from_value(data).unwrap();
        assert!(s.connected);
        assert_eq!(s.battery, Some(74));
        assert_eq!(s.capacity, Some([30498848, 30525408]));
        assert_eq!(s.pending, 0, "pending 缺省应为 0");
    }

    /// 发给前端的快照字段名与 types.ts PnoteStatus 逐字段一致（camelCase）
    #[test]
    fn pnote_status_serializes_frontend_fields() {
        let v = serde_json::to_value(PnoteStatus {
            pending: 3,
            ..Default::default()
        })
        .unwrap();
        let obj = v.as_object().unwrap();
        for key in [
            "running",
            "connected",
            "battery",
            "charging",
            "capacity",
            "version",
            "sleeping",
            "pending",
        ] {
            assert!(obj.contains_key(key), "快照缺少字段 {key}");
        }
        assert_eq!(obj.len(), 8, "快照字段数应与 types.ts PnoteStatus 一致");
    }
}
