use crate::db;
use crate::AppState;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

/// TTS 模型文件清单：(ModelScope 仓库内相对路径, 相对 models/kokoro 根目录的路径)。
/// 仓库统一为 onnx-community/Kokoro-82M-v1.1-zh-ONNX。
/// 注意：必须用 q8（model_quantized.onnx），fp16 版本在 CPU 上推理输出全 NaN。
const TTS_REPO: &str = "onnx-community/Kokoro-82M-v1.1-zh-ONNX";
const TTS_MODEL_FILES: &[(&str, &str)] = &[
    ("onnx/model_quantized.onnx", "model_quantized.onnx"),
    ("tokenizer.json", "tokenizer.json"),
    ("voices/zf_001.bin", "voices/zf_001.bin"),
    ("voices/zf_002.bin", "voices/zf_002.bin"),
    ("voices/zf_003.bin", "voices/zf_003.bin"),
    ("voices/zf_004.bin", "voices/zf_004.bin"),
    ("voices/zm_009.bin", "voices/zm_009.bin"),
    ("voices/zm_010.bin", "voices/zm_010.bin"),
    ("voices/zm_011.bin", "voices/zm_011.bin"),
    ("voices/zm_012.bin", "voices/zm_012.bin"),
];
/// 可选音色（不带 .bin 后缀），与 TTS_MODEL_FILES 里的 voices/* 一一对应
pub const TTS_VOICES: &[&str] = &[
    "zf_001", "zf_002", "zf_003", "zf_004", "zm_009", "zm_010", "zm_011", "zm_012",
];
/// 合成总超时（长纪要逐句流式，预留充足余量）
const TTS_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// 分句目标长度：短句拼到该字数以上，单句最长不超过两倍
const SENT_TARGET: usize = 40;
const SENT_MAX: usize = 100;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TtsModelStatus {
    pub downloaded: bool,
    pub path: String,
    pub sidecar_ready: bool,
    pub voices: Vec<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TtsDownloadProgress {
    file: String,
    downloaded: u64,
    total: u64,
}

/// 每合成好一句发给前端的事件
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TtsChunk {
    entry_id: i64,
    index: usize,
    total: usize,
    wav: String,
    duration: f32,
    /// 该句对应的原文（前端当前句高亮用）
    text: String,
}

/// 朗读开始时一次性发给前端的分句计划（前端展示持续更新的朗读列表）
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TtsPlan {
    entry_id: i64,
    sentences: Vec<String>,
}

fn tts_model_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("models")
        .join("kokoro"))
}

fn tts_model_ready(dir: &Path) -> bool {
    TTS_MODEL_FILES.iter().all(|(_, f)| dir.join(f).is_file())
}

/// TTS 组件（PyInstaller 冻结的 kokoro sidecar）路径。
/// 打包后位于资源目录，开发时位于 src-tauri/resources/ 下。
fn tts_sidecar_exe(app: &AppHandle) -> Result<PathBuf, String> {
    let name = if cfg!(windows) { "tts.exe" } else { "tts" };
    if let Ok(dir) = app.path().resource_dir() {
        let p = dir.join("tts-sidecar").join(name);
        if p.is_file() {
            return Ok(p);
        }
    }
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("resources")
        .join("tts-sidecar")
        .join(name);
    if dev.is_file() {
        return Ok(dev);
    }
    Err("未找到 TTS 组件 tts-sidecar，请先按 tools/build_sidecar.md 构建".into())
}

#[tauri::command]
pub fn tts_model_status(app: AppHandle) -> Result<TtsModelStatus, String> {
    let dir = tts_model_dir(&app)?;
    Ok(TtsModelStatus {
        downloaded: tts_model_ready(&dir),
        path: dir.to_string_lossy().to_string(),
        sidecar_ready: tts_sidecar_exe(&app).is_ok(),
        voices: TTS_VOICES.iter().map(|s| s.to_string()).collect(),
    })
}

#[tauri::command]
pub async fn download_tts_model(app: AppHandle) -> Result<(), String> {
    {
        let state = app.state::<AppState>();
        if state.tts_downloading.swap(true, Ordering::SeqCst) {
            return Err("TTS 模型正在下载中".into());
        }
    }
    let result = download_tts_inner(&app).await;
    app.state::<AppState>()
        .tts_downloading
        .store(false, Ordering::SeqCst);
    result
}

async fn download_tts_inner(app: &AppHandle) -> Result<(), String> {
    use futures_util::StreamExt;

    let root = tts_model_dir(app)?;
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3600))
        // ModelScope LFS CDN 会 403 没有 User-Agent 的请求（reqwest 默认不带 UA）
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36")
        .build()
        .map_err(|e| e.to_string())?;

    for (repo_path, local) in TTS_MODEL_FILES {
        let target = root.join(local);
        if target.is_file() {
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let url =
            format!("https://modelscope.cn/api/v1/models/{TTS_REPO}/repo?FilePath={repo_path}");
        let resp = client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("下载 {local} 失败: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("下载 {local} 失败: HTTP {}", resp.status()));
        }
        let total = resp.content_length().unwrap_or(0);
        let tmp = target.with_extension("part");
        let mut file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        let mut downloaded: u64 = 0;
        let mut stream = resp.bytes_stream();
        use std::io::Write;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("下载 {local} 中断: {e}"))?;
            file.write_all(&chunk).map_err(|e| e.to_string())?;
            downloaded += chunk.len() as u64;
            let _ = app.emit(
                "tts-download-progress",
                TtsDownloadProgress {
                    file: local.to_string(),
                    downloaded,
                    total,
                },
            );
        }
        drop(file);
        std::fs::rename(&tmp, &target).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 轻量 markdown → 朗读文本：去代码块/图片/链接语法/标记符号，保留文字本身
fn strip_markdown(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    let mut in_code = false;
    for line in md.lines() {
        let t = line.trim();
        if t.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code || t.is_empty() {
            continue;
        }
        let mut s = t
            .trim_start_matches('#')
            .trim_start_matches(['>', ' '])
            .trim_start_matches(['-', '*', '+'])
            .trim_start()
            // 有序列表前缀 "1. " / "1、"
            .to_string();
        if let Some(pos) = s.find(['.', '、']) {
            if s[..pos].chars().all(|c| c.is_ascii_digit()) && !s[..pos].is_empty() {
                s = s[pos + 1..].trim_start().to_string();
            }
        }
        // 图片整体丢弃，链接保留显示文字
        s = s.replace("![", "\u{1}["); // 先标记图片，后面整段删掉
        let mut cleaned = String::with_capacity(s.len());
        let mut rest = s.as_str();
        while let Some(i) = rest.find('\u{1}') {
            cleaned.push_str(&rest[..i]);
            rest = &rest[i + 1..];
            if let Some(j) = rest.find(')') {
                rest = &rest[j + 1..];
            } else {
                rest = "";
            }
        }
        cleaned.push_str(rest);
        // 链接 [文字](url) → 文字；行内代码 `x` → x；粗斜体标记去掉
        let mut final_s = String::with_capacity(cleaned.len());
        let mut r = cleaned.as_str();
        while let Some(i) = r.find('[') {
            if let Some(j) = r[i..].find("](") {
                final_s.push_str(&r[..i]);
                final_s.push_str(&r[i + 1..i + j]);
                r = match r[i + j..].find(')') {
                    Some(k) => &r[i + j + k + 1..],
                    None => &r[i + j + 2..],
                };
            } else {
                final_s.push_str(&r[..i + 1]);
                r = &r[i + 1..];
            }
        }
        final_s.push_str(r);
        let final_s: String = final_s
            .chars()
            .filter(|c| !matches!(c, '*' | '`' | '_' | '~'))
            .collect();
        let final_s = final_s.trim();
        if !final_s.is_empty() {
            out.push_str(final_s);
            out.push('\n');
        }
    }
    out
}

/// 转写原文的 [说话人N mm:ss] / [mm:ss] 前缀转成可读形式
fn humanize_transcript(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if t.starts_with('[') {
            if let Some(end) = t.find(']') {
                let head = &t[1..end];
                let body = t[end + 1..].trim();
                // head 形如 "说话人1 03:25" 或 "03:25" / "1:02:33"
                let speaker = head.split_whitespace().next().unwrap_or("");
                if speaker.starts_with("说话人") {
                    out.push_str(speaker);
                    out.push('，');
                }
                out.push_str(body);
                out.push('\n');
                continue;
            }
        }
        out.push_str(t);
        out.push('\n');
    }
    out
}

/// 按标点分句并归并：短句拼到 SENT_TARGET 字以上，超过 SENT_MAX 强制切
fn split_sentences(text: &str) -> Vec<String> {
    let mut frags: Vec<String> = Vec::new();
    let mut cur = String::new();
    for ch in text.chars() {
        if ch == '\n' {
            if !cur.trim().is_empty() {
                frags.push(cur.trim().to_string());
            }
            cur.clear();
            continue;
        }
        cur.push(ch);
        if matches!(ch, '。' | '！' | '？' | '；' | '!' | '?' | ';') {
            frags.push(cur.trim().to_string());
            cur.clear();
        }
    }
    if !cur.trim().is_empty() {
        frags.push(cur.trim().to_string());
    }
    let mut out: Vec<String> = Vec::new();
    let mut buf = String::new();
    for f in frags {
        if f.is_empty() {
            continue;
        }
        let f_len = f.chars().count();
        let buf_len = buf.chars().count();
        if buf_len > 0 && buf_len + f_len > SENT_MAX {
            out.push(std::mem::take(&mut buf));
        }
        buf.push_str(&f);
        if buf.chars().count() >= SENT_TARGET {
            out.push(std::mem::take(&mut buf));
        }
    }
    if !buf.is_empty() {
        out.push(buf);
    }
    out
}

fn cache_dir(app: &AppHandle, key: u64) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("tts-cache")
        .join(format!("{key:016x}"));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

/// 朗读会议条目的纪要或原文。逐句合成，每句就绪即向前端发 tts-chunk 事件；
/// 全部完成发 tts-done，被停止发 tts-stopped。同一时间只允许一个朗读任务。
#[tauri::command]
pub async fn meeting_speak(app: AppHandle, entry_id: i64, voice: String, tab: String) -> Result<(), String> {
    {
        let state = app.state::<AppState>();
        if state.tts_speaking.swap(true, Ordering::SeqCst) {
            return Err("已有朗读任务进行中".into());
        }
        state.tts_cancel.store(false, Ordering::SeqCst);
    }
    let result = speak_inner(&app, entry_id, &voice, &tab).await;
    {
        let state = app.state::<AppState>();
        state.tts_speaking.store(false, Ordering::SeqCst);
        let mut child = state.tts_child.lock().map_err(|e| e.to_string())?;
        *child = None;
    }
    match &result {
        Ok(_) => emit_speak_event(&app, "tts-done", entry_id),
        Err(e) if e == "__cancelled__" => emit_speak_event(&app, "tts-stopped", entry_id),
        Err(e) => {
            let _ = app.emit("tts-error", e.clone());
        }
    }
    result.map_err(|e| if e == "__cancelled__" { "已停止".into() } else { e })
}

fn emit_speak_event(app: &AppHandle, name: &str, entry_id: i64) {
    let _ = app.emit(name, entry_id);
}

async fn speak_inner(app: &AppHandle, entry_id: i64, voice: &str, tab: &str) -> Result<(), String> {
    if !TTS_VOICES.contains(&voice) {
        return Err(format!("未知音色 {voice}"));
    }
    let model_dir = tts_model_dir(app)?;
    if !tts_model_ready(&model_dir) {
        return Err("TTS 模型未下载，请先到「设置 → 会议录音」下载".into());
    }
    let sidecar = tts_sidecar_exe(app)?;

    let entry = {
        let state = app.state::<AppState>();
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        db::get_entry(&conn, entry_id)
            .map_err(|e| e.to_string())?
            .ok_or("条目不存在")?
    };
    // 纪要/深度解析是 markdown，朗读前清洗；原文把 [说话人N mm:ss] 前缀转成可读形式
    let text = if tab == "raw" {
        humanize_transcript(&entry.raw_content)
    } else if tab == "deep" {
        let deep = entry.deep_analysis.as_deref().unwrap_or("").trim();
        if deep.is_empty() {
            return Err("还没有深度解析，请先生成".into());
        }
        strip_markdown(deep)
    } else {
        strip_markdown(entry.clean_content.as_deref().unwrap_or(&entry.raw_content))
    };
    let sentences = split_sentences(&text);
    if sentences.is_empty() {
        return Err("没有可朗读的内容".into());
    }
    // 一次性把分句计划发给前端：朗读列表立即完整展示，随后逐句更新状态
    let _ = app.emit(
        "tts-plan",
        TtsPlan {
            entry_id,
            sentences: sentences.clone(),
        },
    );

    // 缓存键：文本 + 音色；合成过的整句 wav 直接复用
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    voice.hash(&mut hasher);
    let out_dir = cache_dir(app, hasher.finish())?;

    let total = sentences.len();
    let mut pending: Vec<(usize, &str)> = Vec::new();
    for (i, s) in sentences.iter().enumerate() {
        let wav = out_dir.join(format!("{i:04}.wav"));
        if wav.is_file() {
            emit_chunk(app, entry_id, i, total, &wav, 0.0, s);
        } else {
            pending.push((i, s));
        }
    }
    if pending.is_empty() {
        return Ok(());
    }

    // 启动 sidecar：未缓存的句子全部写进 stdin，逐行读 stdout 结果
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let mut cmd = tokio::process::Command::new(&sidecar);
    cmd.arg("--model-dir")
        .arg(&model_dir)
        .arg("--voice")
        .arg(voice)
        .arg("--out-dir")
        .arg(&out_dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // Windows 下 GUI 程序拉起控制台子进程会闪出一个黑窗口，CREATE_NO_WINDOW 抑制
    #[cfg(windows)]
    cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    let mut child = cmd.spawn().map_err(|e| format!("TTS 组件启动失败: {e}"))?;

    let mut stdin = child.stdin.take().ok_or("无法写入 TTS 组件 stdin")?;
    let stdout = child.stdout.take().ok_or("无法读取 TTS 组件 stdout")?;
    let stderr = child.stderr.take().ok_or("无法读取 TTS 组件 stderr")?;

    // stderr 并发排空：日志行转发给前端做进度提示，非协议行留作错误诊断
    let app2 = app.clone();
    let stderr_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        let mut tail: Vec<String> = Vec::new();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(msg) = line.strip_prefix("MASAIFU_LOG ") {
                let _ = app2.emit("tts-progress", msg.to_string());
            } else if !line.starts_with("MASAIFU_") && !line.trim().is_empty() {
                tail.push(line);
                if tail.len() > 6 {
                    tail.remove(0);
                }
            }
        }
        tail.join(" | ")
    });

    let mut input = String::new();
    for (i, s) in &pending {
        input.push_str(&serde_json::json!({"index": i, "text": s}).to_string());
        input.push('\n');
    }
    tokio::spawn(async move {
        let _ = stdin.write_all(input.as_bytes()).await;
        // 显式关闭 stdin，sidecar 收完 EOF 自行退出
        drop(stdin);
    });

    // 供停止命令 kill 用；正常路径下循环结束后从 AppState 取回并 wait 回收
    {
        let state = app.state::<AppState>();
        let mut guard = state.tts_child.lock().map_err(|e| e.to_string())?;
        *guard = Some(child);
    }
    // child 已移入 AppState，下面通过轮询取消标志 + 逐句读 stdout 推进
    let cancelled = |app: &AppHandle| app.state::<AppState>().tts_cancel.load(Ordering::SeqCst);

    let mut lines = BufReader::new(stdout).lines();
    let deadline = tokio::time::Instant::now() + TTS_TIMEOUT;
    let mut received = 0usize;
    while received < pending.len() {
        if cancelled(app) {
            kill_tts_child(app);
            return Err("__cancelled__".into());
        }
        let line = match tokio::time::timeout_at(deadline, lines.next_line()).await {
            Ok(Ok(Some(line))) => line,
            Ok(Ok(None)) => break, // sidecar 提前退出
            Ok(Err(e)) => return Err(format!("读取 TTS 组件输出失败: {e}")),
            Err(_) => {
                kill_tts_child(app);
                return Err("TTS 合成超时".into());
            }
        };
        let v: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let index = v["index"].as_u64().unwrap_or(usize::MAX as u64) as usize;
        let wav = v["wav"].as_str().unwrap_or("").to_string();
        let duration = v["duration"].as_f64().unwrap_or(0.0) as f32;
        if index < total && !wav.is_empty() {
            emit_chunk(app, entry_id, index, total, Path::new(&wav), duration, &sentences[index]);
        }
        received += 1;
    }
    let stderr_tail = stderr_task.await.map_err(|e| e.to_string())?;
    // 回收子进程（sidecar 在 stdin EOF 后自行退出；被 kill 时 wait 直接返回）
    {
        let state = app.state::<AppState>();
        let taken = {
            let mut guard = state.tts_child.lock().map_err(|e| e.to_string())?;
            guard.take()
        };
        // std MutexGuard 不是 Send，必须在 await 前释放
        if let Some(mut child) = taken {
            let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
        }
    }
    if cancelled(app) {
        return Err("__cancelled__".into());
    }
    if received == 0 {
        return Err(format!("TTS 组件没有产出任何音频: {}", db::clip(&stderr_tail, 300)));
    }
    if received < pending.len() {
        // 个别句子合成失败不拖垮整个朗读，跳过即可
        log::warn!("TTS 有 {} 句未合成: {}", pending.len() - received, db::clip(&stderr_tail, 200));
    }
    Ok(())
}

fn emit_chunk(app: &AppHandle, entry_id: i64, index: usize, total: usize, wav: &Path, duration: f32, text: &str) {
    let _ = app.emit(
        "tts-chunk",
        TtsChunk {
            entry_id,
            index,
            total,
            wav: wav.to_string_lossy().to_string(),
            duration,
            text: text.to_string(),
        },
    );
}

fn kill_tts_child(app: &AppHandle) {
    let state = app.state::<AppState>();
    let taken = state
        .tts_child
        .lock()
        .ok()
        .and_then(|mut guard| guard.take());
    if let Some(mut child) = taken {
        let _ = child.start_kill();
    }
}

/// 停止当前朗读（kill sidecar，前端清空播放队列）
#[tauri::command]
pub fn meeting_stop_speak(app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    state.tts_cancel.store(true, Ordering::SeqCst);
    let taken = state
        .tts_child
        .lock()
        .ok()
        .and_then(|mut guard| guard.take());
    if let Some(mut child) = taken {
        let _ = child.start_kill();
    }
    Ok(())
}

/// 读取合成好的 wav 供前端播放（避免为缓存目录开 asset protocol scope）
#[tauri::command]
pub fn tts_read_wav(path: String) -> Result<Vec<u8>, String> {
    // 只允许读 tts-cache 目录下的文件，防路径穿越
    let p = Path::new(&path);
    if !path.contains("tts-cache") || p.extension().and_then(|e| e.to_str()) != Some("wav") {
        return Err("非法的音频路径".into());
    }
    std::fs::read(p).map_err(|e| format!("读取音频失败: {e}"))
}
