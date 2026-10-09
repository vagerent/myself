use crate::db;
use crate::openai::OpenAiClient;
use crate::AppState;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

/// 待写回的纪要处理结果；无 API Key 或生成失败时退化为原始转写文本。
struct ProcessedContent {
    clean: String,
    title: String,
    category: String,
    tags_json: String,
}

impl ProcessedContent {
    fn raw_fallback(transcript: &str) -> Self {
        Self {
            clean: transcript.to_string(),
            title: String::new(),
            category: "会议".into(),
            tags_json: "[]".into(),
        }
    }
}

/// 模型必需文件清单：(ModelScope 仓库, 相对 models 根目录的路径)。
/// SenseVoiceSmall-onnx 仓库不含 sentencepiece 分词器，需从 torch 仓库补一个文件；
/// fsmn-vad 模型用于长音频按静音点切片（转写按切片逐块上报真实进度）。
const MODEL_FILES: &[(&str, &str)] = &[
    ("iic/SenseVoiceSmall-onnx", "sensevoice-small-onnx/model_quant.onnx"),
    ("iic/SenseVoiceSmall-onnx", "sensevoice-small-onnx/config.yaml"),
    ("iic/SenseVoiceSmall-onnx", "sensevoice-small-onnx/configuration.json"),
    ("iic/SenseVoiceSmall-onnx", "sensevoice-small-onnx/am.mvn"),
    ("iic/SenseVoiceSmall-onnx", "sensevoice-small-onnx/tokens.json"),
    (
        "iic/SenseVoiceSmall",
        "sensevoice-small-onnx/chn_jpn_yue_eng_ko_spectok.bpe.model",
    ),
    (
        "damo/speech_fsmn_vad_zh-cn-16k-common-onnx",
        "fsmn-vad/model_quant.onnx",
    ),
    ("damo/speech_fsmn_vad_zh-cn-16k-common-onnx", "fsmn-vad/config.yaml"),
    ("damo/speech_fsmn_vad_zh-cn-16k-common-onnx", "fsmn-vad/am.mvn"),
    (
        "damo/speech_fsmn_vad_zh-cn-16k-common-onnx",
        "fsmn-vad/configuration.json",
    ),
    // CAM++ 声纹模型（说话人分离），CosyVoice 仓库里是 onnx 单文件
    ("iic/CosyVoice-300M-SFT", "campplus/campplus.onnx"),
];
/// 转写子进程最长允许运行时间（长音频可能很慢）。
const TRANSCRIBE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AsrModelStatus {
    pub downloaded: bool,
    pub path: String,
    pub sidecar_ready: bool,
}

#[derive(Clone, Serialize)]
struct ProgressPayload {
    stage: &'static str,
    message: String,
    /// 0..1 的真实进度；None 表示该阶段无法量化（前端显示不确定动画）
    progress: Option<f32>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DownloadProgress {
    file: String,
    downloaded: u64,
    total: u64,
}

fn emit_progress(
    app: &AppHandle,
    stage: &'static str,
    message: impl Into<String>,
    progress: Option<f32>,
) {
    let _ = app.emit(
        "meeting-progress",
        ProgressPayload {
            stage,
            message: message.into(),
            progress,
        },
    );
}

/// 模型根目录：<app_data>/models（ASR 与 VAD 各占一个子目录）
fn models_root(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("models"))
}

fn asr_model_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(models_root(app)?.join("sensevoice-small-onnx"))
}

fn vad_model_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(models_root(app)?.join("fsmn-vad"))
}

fn spk_model_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(models_root(app)?.join("campplus"))
}

fn model_ready(root: &Path) -> bool {
    MODEL_FILES.iter().all(|(_, f)| root.join(f).is_file())
}

/// 转写组件（PyInstaller 冻结的 funasr-onnx sidecar）路径。
/// 打包后位于资源目录，开发时位于 src-tauri/resources/ 下。
fn sidecar_exe(app: &AppHandle) -> Result<PathBuf, String> {
    let name = if cfg!(windows) {
        "transcribe.exe"
    } else {
        "transcribe"
    };
    if let Ok(dir) = app.path().resource_dir() {
        let p = dir.join("funasr-sidecar").join(name);
        if p.is_file() {
            return Ok(p);
        }
    }
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("resources")
        .join("funasr-sidecar")
        .join(name);
    if dev.is_file() {
        return Ok(dev);
    }
    Err("未找到转写组件 funasr-sidecar，请先按 tools/build_sidecar.md 构建".into())
}

#[tauri::command]
pub fn asr_model_status(app: AppHandle) -> Result<AsrModelStatus, String> {
    let root = models_root(&app)?;
    Ok(AsrModelStatus {
        downloaded: model_ready(&root),
        path: root.to_string_lossy().to_string(),
        sidecar_ready: sidecar_exe(&app).is_ok(),
    })
}

#[tauri::command]
pub async fn download_asr_model(app: AppHandle) -> Result<(), String> {
    {
        let state = app.state::<AppState>();
        if state.asr_downloading.swap(true, Ordering::SeqCst) {
            return Err("模型正在下载中".into());
        }
    }
    let result = download_inner(&app).await;
    app.state::<AppState>()
        .asr_downloading
        .store(false, Ordering::SeqCst);
    result
}

async fn download_inner(app: &AppHandle) -> Result<(), String> {
    use futures_util::StreamExt;

    let root = models_root(app)?;
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3600))
        // ModelScope LFS CDN 会 403 没有 User-Agent 的请求（reqwest 默认不带 UA）
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36")
        .build()
        .map_err(|e| e.to_string())?;

    for (repo, name) in MODEL_FILES {
        let target = root.join(name);
        if target.is_file() {
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let file_name = name.rsplit('/').next().unwrap_or(name);
        let url =
            format!("https://modelscope.cn/api/v1/models/{repo}/repo?FilePath={file_name}");
        let resp = client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("下载 {name} 失败: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("下载 {name} 失败: HTTP {}", resp.status()));
        }
        let total = resp.content_length().unwrap_or(0);
        let tmp = target.with_extension("part");
        let mut file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        let mut downloaded: u64 = 0;
        let mut stream = resp.bytes_stream();
        use std::io::Write;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("下载 {name} 中断: {e}"))?;
            file.write_all(&chunk).map_err(|e| e.to_string())?;
            downloaded += chunk.len() as u64;
            let _ = app.emit(
                "asr-download-progress",
                DownloadProgress {
                    file: file_name.to_string(),
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

#[tauri::command]
pub async fn import_meeting_audio(app: AppHandle, path: String) -> Result<db::EntryRow, String> {
    {
        let state = app.state::<AppState>();
        if state.meeting_busy.swap(true, Ordering::SeqCst) {
            return Err("已有会议导入任务进行中".into());
        }
    }
    let result = import_inner(&app, &path).await;
    app.state::<AppState>()
        .meeting_busy
        .store(false, Ordering::SeqCst);
    match &result {
        Ok(_) => emit_progress(&app, "done", "导入完成", None),
        Err(e) => emit_progress(&app, "error", e.clone(), None),
    }
    result
}

/// 转写任意受支持音频文件为文本（ogg/opus 直通 sidecar，其余格式先解码 16k wav）。
/// 会议导入与录音卡同步共用此入口；进度仍走 meeting-progress 事件。
pub async fn transcribe_audio(app: &AppHandle, audio: &Path) -> Result<String, String> {
    let root = models_root(app)?;
    if !model_ready(&root) {
        return Err("语音识别模型未下载，请先到「设置 → 会议录音」下载模型".into());
    }
    let model_dir = asr_model_dir(app)?;
    let vad_dir = vad_model_dir(app)?;
    let spk_dir = spk_model_dir(app)?;
    let sidecar = sidecar_exe(app)?;

    // 1. 准备转写输入：ogg/opus 直接交给 sidecar 读取（libsndfile 原生支持，
    //    录音卡导出的 16kHz ogg 免转码）；其余格式经 symphonia 解码为 16kHz wav
    let wav_path: Option<PathBuf> = if is_ogg_opus(audio) {
        None
    } else {
        emit_progress(app, "decoding", "正在解码音频…", None);
        let app2 = app.clone();
        let src2 = audio.to_path_buf();
        let samples = tauri::async_runtime::spawn_blocking(move || {
            let mut last = 0.0f32;
            decode_file_to_16k(&src2, |frac| {
                let frac = frac.clamp(0.0, 1.0);
                if frac - last >= 0.02 || frac >= 1.0 {
                    last = frac;
                    emit_progress(
                        &app2,
                        "decoding",
                        format!("正在解码音频… {}%", (frac * 100.0) as u32),
                        Some(frac),
                    );
                }
            })
        })
        .await
        .map_err(|e| e.to_string())??;
        let wav_path = std::env::temp_dir().join(format!(
            "masaifu_meeting_{}_{}.wav",
            std::process::id(),
            chrono::Local::now().timestamp_millis()
        ));
        write_wav_16k(&wav_path, &samples)?;
        Some(wav_path)
    };
    let audio_path: &Path = match &wav_path {
        Some(p) => p,
        None => audio,
    };

    // 2. 调 sidecar 转写（stderr 流式读取，逐块上报真实进度）
    emit_progress(app, "transcribing", "正在启动语音识别…", Some(0.0));
    let transcribe_result = run_sidecar(app, &sidecar, &model_dir, &vad_dir, &spk_dir, audio_path).await;
    if let Some(p) = &wav_path {
        let _ = std::fs::remove_file(p);
    }
    let (status, stdout_bytes, stderr_tail) = transcribe_result?;
    if !status.success() {
        return Err(format!("转写失败: {}", db::clip(&stderr_tail, 300)));
    }
    let stdout = String::from_utf8_lossy(&stdout_bytes);
    let v: serde_json::Value = serde_json::from_str(stdout.trim())
        .map_err(|e| format!("转写结果解析失败: {e}"))?;
    let transcript = v["text"].as_str().unwrap_or("").trim().to_string();
    if transcript.is_empty() {
        return Err("未识别到有效语音内容".into());
    }
    Ok(transcript)
}

async fn import_inner(app: &AppHandle, path: &str) -> Result<db::EntryRow, String> {
    let src = PathBuf::from(path);
    if !src.is_file() {
        return Err("文件不存在".into());
    }
    let transcript = transcribe_audio(app, &src).await?;

    // 3. 入库（source = meeting）
    let (dedup_daily, settings) = {
        let state = app.state::<AppState>();
        let s = state.settings.lock().map_err(|e| e.to_string())?.clone();
        (s.dedup_scope != "all", s)
    };
    let entry_id = {
        let state = app.state::<AppState>();
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        db::insert_entry(&conn, &transcript, "meeting", dedup_daily)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| {
                if dedup_daily {
                    "相同内容的会议今天已导入过".to_string()
                } else {
                    "相同内容的会议已导入过".to_string()
                }
            })?
    };

    // 4. 生成会议纪要；无 API Key 或生成失败时退化为原始转写文本
    let mut minutes_warning: Option<String> = None;
    let processed = if settings.api_key_present() {
        emit_progress(app, "minutes", "正在生成会议纪要…", None);
        let client = OpenAiClient::new(&settings.base_url, &settings.api_key, &settings.model);
        match minutes_content(&client, &transcript, &settings.meeting_prompt_template).await {
            Ok(p) => p,
            Err(e) => {
                log::warn!("会议纪要生成失败，保留原始转写: {e}");
                minutes_warning = Some(e);
                ProcessedContent::raw_fallback(&transcript)
            }
        }
    } else {
        ProcessedContent::raw_fallback(&transcript)
    };
    {
        let state = app.state::<AppState>();
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        db::mark_processed(
            &conn,
            entry_id,
            &processed.clean,
            &processed.title,
            &processed.category,
            &processed.tags_json,
        )
        .map_err(|e| e.to_string())?;
        let row = db::get_entry(&conn, entry_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "读取失败".to_string())?;
        if let Some(w) = minutes_warning {
            log::warn!("会议 {entry_id} 已入库，但纪要生成失败: {w}");
        }
        Ok(row)
    }
}

/// 调模型生成会议纪要内容（不写库）；失败返回 Err。会议导入与录音卡自动分析共用。
async fn minutes_content(
    client: &OpenAiClient,
    transcript: &str,
    prompt: &str,
) -> Result<ProcessedContent, String> {
    let p = client.meeting_minutes(transcript, prompt).await?;
    let tags_json = p.tags_json();
    Ok(ProcessedContent {
        clean: p.clean,
        title: p.title,
        category: p.category,
        tags_json,
    })
}

/// 用指定提示词为已有条目生成会议纪要并 mark_processed。
/// 供录音卡转写后的自动静默分析复用：成功返回 true；无 API Key 或任何失败都
/// 只记日志返回 false，条目保持原文入库（非致命，不影响同步流程）。
pub async fn apply_minutes(app: &AppHandle, entry_id: i64, transcript: &str, prompt: &str) -> bool {
    let settings = {
        let state = app.state::<AppState>();
        let guard = state.settings.lock();
        match guard {
            Ok(s) => s.clone(),
            Err(e) => {
                log::warn!("条目 {entry_id} 读取设置失败，跳过纪要生成: {e}");
                return false;
            }
        }
    };
    if !settings.api_key_present() {
        log::info!("条目 {entry_id} 未配置 API Key，跳过纪要生成（保留原始转写）");
        return false;
    }
    let client = OpenAiClient::new(&settings.base_url, &settings.api_key, &settings.model);
    let processed = match minutes_content(&client, transcript, prompt).await {
        Ok(p) => p,
        Err(e) => {
            log::warn!("条目 {entry_id} 纪要生成失败，保留原始转写: {e}");
            return false;
        }
    };
    let result = {
        let state = app.state::<AppState>();
        let guard = state.db.lock();
        match guard {
            Ok(conn) => db::mark_processed(
                &conn,
                entry_id,
                &processed.clean,
                &processed.title,
                &processed.category,
                &processed.tags_json,
            )
            .map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        }
    };
    if let Err(e) = result {
        log::warn!("条目 {entry_id} 纪要写库失败: {e}");
        return false;
    }
    true
}

/// 生成会议深度解析（异步后台执行，不阻塞 UI；结果落库后返回）。
/// 同一时间只允许一个解析任务，避免并发打爆 API。
#[tauri::command]
pub async fn generate_deep_analysis(app: AppHandle, entry_id: i64) -> Result<String, String> {
    {
        let state = app.state::<AppState>();
        if state.deep_busy.swap(true, Ordering::SeqCst) {
            return Err("已有深度解析任务进行中".into());
        }
    }
    let prompt = {
        let state = app.state::<AppState>();
        state
            .settings
            .lock()
            .map(|s| s.meeting_deep_prompt_template.clone())
            .unwrap_or_default()
    };
    let result = deep_analysis_with_prompt(&app, entry_id, &prompt).await;
    app.state::<AppState>()
        .deep_busy
        .store(false, Ordering::SeqCst);
    result
}

/// 深度解析核心（不含 busy 门控，调用方负责串行）：读原文 → 按指定提示词调模型
/// → 结果落库 → 标题为空时按解析结果补题（补题失败仅记日志）。
/// 会议命令与录音卡自动分析共用。
pub async fn deep_analysis_with_prompt(
    app: &AppHandle,
    entry_id: i64,
    prompt: &str,
) -> Result<String, String> {
    let (transcript, title_missing, settings) = {
        let state = app.state::<AppState>();
        let s = state.settings.lock().map_err(|e| e.to_string())?.clone();
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        let row = db::get_entry(&conn, entry_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "条目不存在".to_string())?;
        let missing = row.title.as_deref().map(str::trim).unwrap_or("").is_empty();
        (row.raw_content, missing, s)
    };
    if !settings.api_key_present() {
        return Err("请先在「设置 → AI 服务」中配置 API Key".into());
    }
    let client = OpenAiClient::new(&settings.base_url, &settings.api_key, &settings.model);
    let analysis = client.meeting_deep_analysis(&transcript, prompt).await?;
    {
        let state = app.state::<AppState>();
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        db::save_deep_analysis(&conn, entry_id, &analysis).map_err(|e| e.to_string())?;
    }
    // 标题为空时（纪要未生成或生成失败），根据解析结果补一个标题；
    // 补题失败不影响解析结果，只记日志。
    if title_missing {
        match client.generate_title(&analysis).await {
            Ok(title) => {
                let state = app.state::<AppState>();
                let conn = state.db.lock().map_err(|e| e.to_string())?;
                db::update_title(&conn, entry_id, &title).map_err(|e| e.to_string())?;
            }
            Err(e) => log::warn!("会议 {entry_id} 深度解析后补标题失败: {e}"),
        }
    }
    Ok(analysis)
}

/// 启动 sidecar 并流式消费 stderr：协议行（MASAIFU_PROGRESS / MASAIFU_LOG）转成
/// 前端进度事件，其余行保留最后一行作错误诊断。stdout 并发排空，避免长转写撑满
/// 管道缓冲导致子进程阻塞。
async fn run_sidecar(
    app: &AppHandle,
    sidecar: &Path,
    model_dir: &Path,
    vad_dir: &Path,
    spk_dir: &Path,
    wav: &Path,
) -> Result<(std::process::ExitStatus, Vec<u8>, String), String> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

    let mut cmd = tokio::process::Command::new(sidecar);
    cmd.arg("--model-dir")
        .arg(model_dir)
        .arg("--vad-dir")
        .arg(vad_dir)
        .arg("--spk-dir")
        .arg(spk_dir)
        .arg("--audio")
        .arg(wav)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // Windows 下 GUI 程序拉起控制台子进程会闪出一个黑窗口，CREATE_NO_WINDOW 抑制
    #[cfg(windows)]
    cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("转写组件启动失败: {e}"))?;

    let mut stdout_pipe = child.stdout.take().ok_or("无法读取转写组件 stdout")?;
    let stderr_pipe = child.stderr.take().ok_or("无法读取转写组件 stderr")?;

    let stdout_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut buf).await;
        buf
    });

    let app2 = app.clone();
    let stderr_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr_pipe).lines();
        // 非协议行：保留最后几行做错误诊断（sidecar 报错的 traceback 在最后）
        let mut tail: Vec<String> = Vec::new();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(rest) = line.strip_prefix("MASAIFU_PROGRESS ") {
                if let Some((done, total)) = rest.split_once('/') {
                    if let (Ok(d), Ok(t)) =
                        (done.trim().parse::<f32>(), total.trim().parse::<f32>())
                    {
                        if t > 0.0 {
                            let frac = (d / t).clamp(0.0, 1.0);
                            emit_progress(
                                &app2,
                                "transcribing",
                                format!("正在语音识别… {}%", (frac * 100.0) as u32),
                                Some(frac),
                            );
                        }
                    }
                }
            } else if let Some(msg) = line.strip_prefix("MASAIFU_LOG ") {
                emit_progress(&app2, "transcribing", msg.to_string(), None);
            } else if !line.trim().is_empty() {
                tail.push(line);
                if tail.len() > 6 {
                    tail.remove(0);
                }
            }
        }
        tail.join(" | ")
    });

    let status = match tokio::time::timeout(TRANSCRIBE_TIMEOUT, child.wait()).await {
        Ok(s) => s.map_err(|e| format!("转写组件运行失败: {e}"))?,
        Err(_) => {
            let _ = child.kill().await;
            return Err("转写超时（超过 30 分钟）".into());
        }
    };
    let stdout_bytes = stdout_task.await.map_err(|e| e.to_string())?;
    let stderr_tail = stderr_task.await.map_err(|e| e.to_string())?;
    Ok((status, stdout_bytes, stderr_tail))
}

/// ogg/opus symphonia 解不了（无 opus 解码器），由 sidecar 的 libsndfile 直接读取
fn is_ogg_opus(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| matches!(e.to_ascii_lowercase().as_str(), "ogg" | "opus"))
        .unwrap_or(false)
}

/// 解码任意常见音频文件为 16kHz 单声道 f32 采样。
/// on_progress 在能确定总时长时按 0..1 回调（部分容器格式拿不到总帧数则不回调）。
pub fn decode_file_to_16k(
    path: &Path,
    mut on_progress: impl FnMut(f32),
) -> Result<Vec<f32>, String> {
    let (mono, rate) = decode_to_mono_f32(path, &mut on_progress)?;
    if rate == 16000 {
        Ok(mono)
    } else {
        resample_to_16k(&mono, rate)
    }
}

fn decode_to_mono_f32(
    path: &Path,
    on_progress: &mut dyn FnMut(f32),
) -> Result<(Vec<f32>, u32), String> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
    use symphonia::core::errors::Error as SymError;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let file = std::fs::File::open(path).map_err(|e| format!("打开音频失败: {e}"))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| format!("无法识别的音频格式: {e}"))?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| "文件中没有音频轨".to_string())?;
    let track_id = track.id;
    let rate = track
        .codec_params
        .sample_rate
        .ok_or_else(|| "未知采样率".to_string())?;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("不支持的音频编码: {e}"))?;
    // 总帧数（部分流式格式拿不到，此时不解码进度回调）
    let total_frames = track.codec_params.n_frames;

    let mut mono: Vec<f32> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymError::ResetRequired) => return Err("音频流需要重置，暂不支持".into()),
            Err(e) => return Err(format!("读取音频流失败: {e}")),
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = *decoded.spec();
                let channels = spec.channels.count().max(1);
                let mut buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
                buf.copy_interleaved_ref(decoded);
                for frame in buf.samples().chunks(channels) {
                    let sum: f32 = frame.iter().sum();
                    mono.push(sum / channels as f32);
                }
                if let Some(total) = total_frames {
                    if total > 0 {
                        on_progress(mono.len() as f32 / total as f32);
                    }
                }
            }
            // 单个坏包跳过，尽量解码完整文件
            Err(SymError::DecodeError(_)) => continue,
            Err(e) => return Err(format!("解码失败: {e}")),
        }
    }
    if mono.is_empty() {
        return Err("没有解码出任何音频数据".into());
    }
    Ok((mono, rate))
}

/// 线性域 FFT 重采样到 16kHz。
pub fn resample_to_16k(samples: &[f32], from_rate: u32) -> Result<Vec<f32>, String> {
    use rubato::{FftFixedIn, Resampler};

    const CHUNK: usize = 1024;
    let mut resampler = FftFixedIn::<f32>::new(from_rate as usize, 16000, CHUNK, 2, 1)
        .map_err(|e| format!("重采样初始化失败: {e}"))?;
    let mut out: Vec<f32> = Vec::new();
    let mut pos = 0;
    while pos + CHUNK <= samples.len() {
        let block = [samples[pos..pos + CHUNK].to_vec()];
        let mut res = resampler
            .process(&block, None)
            .map_err(|e| format!("重采样失败: {e}"))?;
        out.append(&mut res[0]);
        pos += CHUNK;
    }
    if pos < samples.len() {
        let block = [samples[pos..].to_vec()];
        let mut res = resampler
            .process_partial(Some(&block), None)
            .map_err(|e| format!("重采样失败: {e}"))?;
        out.append(&mut res[0]);
    }
    Ok(out)
}

fn write_wav_16k(path: &Path, samples: &[f32]) -> Result<(), String> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).map_err(|e| e.to_string())?;
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        writer.write_sample(v).map_err(|e| e.to_string())?;
    }
    writer.finalize().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, secs: f32) -> Vec<f32> {
        let n = (rate as f32 * secs) as usize;
        (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / rate as f32).sin() * 0.5)
            .collect()
    }

    #[test]
    fn resample_44100_to_16000_length() {
        let input = sine(44100, 1.0);
        let out = resample_to_16k(&input, 44100).unwrap();
        let expected = 16000.0_f32;
        let ratio = out.len() as f32 / expected;
        assert!(
            (0.95..=1.05).contains(&ratio),
            "重采样长度异常: {}（期望约 16000）",
            out.len()
        );
    }

    #[test]
    fn decode_wav_roundtrip() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("masaifu_test_{}.wav", std::process::id()));
        // 写一个 22050Hz 的 wav，再走完整解码 + 重采样管线
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 22050,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for s in sine(22050, 0.5) {
            writer
                .write_sample((s * i16::MAX as f32) as i16)
                .unwrap();
        }
        writer.finalize().unwrap();

        let out = decode_file_to_16k(&path, |_| {}).unwrap();
        let _ = std::fs::remove_file(&path);
        let expected = 16000.0 * 0.5;
        let ratio = out.len() as f32 / expected;
        assert!(
            (0.9..=1.1).contains(&ratio),
            "解码长度异常: {}（期望约 8000）",
            out.len()
        );
    }
}
