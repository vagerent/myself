# 录音卡 7×24 无感同步 · 实施计划

> **执行方式：** Subagent-Driven（本会话逐任务派发，任务间代码评审）
> **设计定稿：** `docs/plans/2026-10-08-recorder-design.md`（先读它）
> **测试记录：** `录音卡/pnote_sync/测试记录与使用手册.md`（协议怪癖必守清单在文末）

**Goal:** 左侧「来源」新增「录音卡」页面，BLE 长连接常驻同步，录音保存后自动 OGG 归档 + 转写入库（source='recorder'）。

**Architecture:** 三层——(1) `pnote-sidecar`：PyInstaller 冻结的常驻 Python 进程（bleak 长连接，基于已验收 daemon.py 架构），stdin/stdout JSON-lines 控制协议，stderr 打 `MASAIFU_LOG`；(2) Rust `pnote.rs`：sidecar 进程管理 + 同步编排 + 转写触发（复用 meeting.rs 转写管线）；(3) 前端：来源菜单项 + 设备面板 + 设置 section。

**Tech Stack:** Python 3.12 + bleak 3.0.2 + PyInstaller / Rust + tokio + rusqlite / Svelte 4。

---

## 协议定义（全文约定）

**源码目录**：`src-tauri/pnote/`（从 `录音卡/pnote_sync/` 复制 pnote.py / opuswrap.py / sync.py 后改造）。

**改造点（复制后立即做）**：`pnote.py` 的 `log()`（pnote.py:32-39）目前 `print` 到 **stdout**，会污染 JSON-lines 通道 → 改为 `print(line, file=sys.stderr, flush=True)`，文件日志保留（`set_log_file`）。

**控制通道**：stdin 每行一个请求；stdout 每行一个响应或事件（按 `type` 区分）：

```json
// → sidecar（stdin）
{"id": 1, "cmd": "start"}                // 启动守护循环（连接→保活→轮询→下载）
{"id": 2, "cmd": "stop"}                 // 停循环并断开
{"id": 3, "cmd": "status"}               // 查询 {running, connected, battery, capacity, version}
{"id": 4, "cmd": "file_list"}            // 设备文件列表
{"id": 5, "cmd": "sync_now"}             // 立即跑一轮同步迭代
{"id": 6, "cmd": "gc_device", "names": ["note20261001-120000."]}  // 删除设备端指定文件
// ← sidecar（stdout）
{"type": "resp", "id": 1, "ok": true, "data": {...}}
{"type": "resp", "id": 2, "ok": false, "error": "原因"}
{"type": "event", "event": "state", "data": {"running": true, "connected": true, "battery": 66, "charging": false, "capacity": [30498848, 30525408], "version": "V1.0.0", "sleeping": false}}
{"type": "event", "event": "progress", "data": {"file": "note...", "received": 12345, "total": 99999}}
{"type": "event", "event": "file_synced", "data": {"name": "note20261008-173118.", "ogg": "C:\\...\\note....ogg", "seconds": 27, "size": 55000}}
```

- `state` 事件：连接状态变化时推；`sleeping=true` 表示找不到设备（等待唤醒）。
- `file_synced`：OGG 已校验落盘（.part→rename 后）才发。
- stderr 只打 `MASAIFU_LOG <msg>`（复用现有 Rust 解析）和普通诊断日志。
- stdin EOF = 退出（照 tts sidecar 惯例）。
- 设备端清理策略在 Rust 侧决策（它持有 recorder_files 表），sidecar 只执行 `gc_device`。

---

## Task 1: pnote-sidecar 源码与协议层

**Files:**
- Create: `src-tauri/pnote/pnote.py`（复制自 `录音卡/pnote_sync/pnote.py`，改 log 走 stderr）
- Create: `src-tauri/pnote/opuswrap.py`（复制自 `录音卡/pnote_sync/opuswrap.py`）
- Create: `src-tauri/pnote/sync.py`（复制自 `录音卡/pnote_sync/sync.py`）
- Create: `src-tauri/pnote/pnote_sidecar.py`

**Step 1: 复制并改造三个库文件**

```bash
mkdir -p src-tauri/pnote
cp "/d/SynologyDrive/Job/Project/AI/mycontext/录音卡/pnote_sync/pnote.py" \
   "/d/SynologyDrive/Job/Project/AI/mycontext/录音卡/pnote_sync/opuswrap.py" \
   "/d/SynologyDrive/Job/Project/AI/mycontext/录音卡/pnote_sync/sync.py" \
   src-tauri/pnote/
```

pnote.py 改 `log()`：`print(line, file=sys.stderr, flush=True)`（保留 RUN_LOG 文件追加）。
sync.py 若直接 print 也一并改 stderr（grep 检查）。

**Step 2: 写 pnote_sidecar.py 控制层**

骨架（关键结构，照此实现）：

```python
"""马赛福录音卡守护 sidecar。stdin JSON 行请求，stdout JSON 行响应/事件，stderr 打日志。
用法: pnote --out-dir <OGG归档目录> [--ffmpeg <路径>] [--device-name cb08]
退出: stdin EOF 或 {"cmd":"shutdown"}。"""
import sys, json, asyncio, argparse
# sys.stdout/stderr reconfigure utf-8（照 transcribe.py:119-121 惯例）

class Daemon:
    """移植 daemon.py:61-119 主循环 + wait_and_connect(daemon.py:37-58)。
    差异：日志改 stderr；每轮迭代后回报 state；下载用 sync.download_one；
    退出由 stop_event 驱动（替代 KeyboardInterrupt 独占）。"""
    # 方法：start() / stop() / status() / file_list() / sync_now() / gc_device(names)
    # 内部：_emit(event, data)  print json {"type":"event",...} 到 stdout flush
    #       _respond(id, ok, data/error)
    # 连接后必须 pen.start_event_router()（按键回执依赖，pnote.py:330）
    # 断开前 pen.stop_event_router()

async def main() -> int:
    # argparse: --out-dir(required) --ffmpeg(default "") --device-name(default cb08)
    # 启动行: {"type":"event","event":"ready","data":{"version": 1}}  —— Rust 等这行确认进程可用
    # stdin 逐行读：json.loads → 分发到 Daemon 方法（每个方法 try/except 包成 resp）
    # {"cmd":"shutdown"} 或 EOF → daemon.stop() → return 0
    # 所有从 pnote.py 抛出的 BleakError/TimeoutError/OSError 转成 ok:false 响应，进程不死

if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
```

要点：
- `status` 在无连接时也必须能应答（ok:true，connected:false）。
- 每轮同步迭代的文件列表、下载结果除了事件外，写入内存 latest_state 供 status 用。
- `gc_device`：逐个 `delete_file`（pnote.py:492，需 FileEntry 带 raw_entry），返回 `{"deleted": n, "failed": [...]}`。

**Step 3: 本地无设备冒烟**

```bash
cd src-tauri/pnote
python pnote_sidecar.py --out-dir /tmp/pnote_out <<< '{"id":1,"cmd":"status"}'
```

期望：stdout 出现 `ready` 事件 + `{"type":"resp","id":1,"ok":true,...,"connected":false}`，进程随 stdin EOF 退出。
再试 `{"id":1,"cmd":"start"}`：30 秒内应收到 `sleeping=true` 的 state 事件（找不到设备不 crash）。

**Step 4: Commit**

```bash
git add src-tauri/pnote/
git commit -m "feat(pnote): 录音卡 sidecar 源码与 JSON-lines 控制协议"
```

---

## Task 2: 冻结 pnote-sidecar 并接入打包

**Files:**
- Modify: `tools/build_sidecar.md`（追加 pnote 章节）
- Modify: `tauri.conf.json:35-37`（bundle.resources）
- Modify: `.gitignore`（pnote-sidecar 例外 + .venv-pnote）
- Create: `src-tauri/resources/pnote-sidecar/README.txt`（占位）

**Step 1: 建 venv 并冻结**

```bash
python -m venv .venv-pnote
.venv-pnote/Scripts/python.exe -m pip install bleak==3.0.2 pyinstaller
.venv-pnote/Scripts/python.exe -m PyInstaller --onedir --name pnote --noconfirm --clean \
  --collect-all bleak --hidden-import winrt.windows.foundation \
  --distpath src-tauri/resources \
  src-tauri/pnote/pnote_sidecar.py
mkdir -p src-tauri/resources/pnote-sidecar
mv src-tauri/resources/pnote/pnote.exe src-tauri/resources/pnote/_internal src-tauri/resources/pnote-sidecar/
rmdir src-tauri/resources/pnote
echo pnote-sidecar 由 PyInstaller 冻结生成，见 tools/build_sidecar.md > src-tauri/resources/pnote-sidecar/README.txt
```

若冻结后运行报 winrt 模块缺失，加 `--collect-all winrt` 重来（bleak Windows 后端）。

**Step 2: 冒烟冻结产物**

```bash
src-tauri/resources/pnote-sidecar/pnote.exe --out-dir /tmp/pnote_out <<< '{"id":1,"cmd":"status"}'
```
期望同 Task 1 Step 3。

**Step 3: 打包配置**

tauri.conf.json:
```json
"resources": {
  "resources/funasr-sidecar/": "funasr-sidecar/",
  "resources/tts-sidecar/": "tts-sidecar/",
  "resources/pnote-sidecar/": "pnote-sidecar/"
}
```
.gitignore 追加（照 funasr 惯例）：
```
src-tauri/resources/pnote-sidecar/*
!src-tauri/resources/pnote-sidecar/README.txt
.venv-pnote/
```
tools/build_sidecar.md 追加「构建录音卡 sidecar」章节（命令即 Step 1，注明 bleak==3.0.2 与 winrt 依赖坑）。

**Step 4: Commit** `feat(pnote): 冻结 pnote-sidecar 并接入打包配置`

---

## Task 3: recorder_files 表与数据层

**Files:**
- Modify: `src-tauri/src/db.rs`（建表 + CRUD + 单测）

**Step 1: 写失败测试**（db.rs tests 模块，照 `delete_entry_removes_row_and_fts` 风格）

```rust
#[test]
fn recorder_file_status_roundtrip() {
    let conn = Connection::open_in_memory().unwrap();
    init(&conn).unwrap();
    recorder_file_upsert(&conn, "note001.", 55000, 27, "downloading").unwrap();
    recorder_file_mark(&conn, "note001.", "done", Some(42), Some("/a/b.ogg")).unwrap();
    let f = recorder_file_get(&conn, "note001.").unwrap().unwrap();
    assert_eq!(f.status, "done");
    assert_eq!(f.entry_id, Some(42));
    // 重复 upsert 不报错（去重键为 name）
    recorder_file_upsert(&conn, "note001.", 55000, 27, "listed").unwrap();
    assert_eq!(recorder_file_get(&conn, "note001.").unwrap().unwrap().status, "done",
        "已存在的记录不能被回退成 listed");
}
```

**Step 2: 跑测试确认失败** `cd src-tauri && cargo test recorder_file` → 编译错误。

**Step 3: 实现**

- `init()` 里加建表（照 entries 表写法，db.rs:94 区）：
```sql
CREATE TABLE IF NOT EXISTS recorder_files (
  name TEXT PRIMARY KEY,
  size INTEGER NOT NULL,
  seconds INTEGER NOT NULL DEFAULT 0,
  status TEXT NOT NULL DEFAULT 'listed',
  ogg_path TEXT,
  entry_id INTEGER,
  created_at TEXT NOT NULL
);
```
- `RecorderFileRow` 结构体（serde camelCase，对齐 db.rs 现有 Row 风格）
- `recorder_file_upsert`（INSERT ... ON CONFLICT(name) DO NOTHING——已存在则不覆盖状态）
- `recorder_file_mark(conn, name, status, entry_id, ogg_path)`
- `recorder_file_get(conn, name) -> Result<Option<RecorderFileRow>>`
- `recorder_files_done_before(conn, days) -> Vec<String>`（device gc 候选：status='done' 且 ogg 文件 mtime 超过 days 天——时间判断在 Rust 侧做，SQL 只取 done 列表）
- `recorder_pending_count(conn) -> i64`（status IN ('listed','downloading','failed') 计数，给前端徽标）

**Step 4: 跑测试确认通过** `cargo test recorder_file` → PASS

**Step 5: Commit** `feat(db): recorder_files 同步状态表`

---

## Task 4: meeting.rs 抽出可复用转写函数

**Files:**
- Modify: `src-tauri/src/meeting.rs`（import_inner 拆层）

**Step 1: 重构**：把 import_inner 的「步骤 1+2」（准备音频 → run_sidecar → 解析 text）抽成：

```rust
/// 转写任意受支持音频文件为文本（ogg/opus 直通 sidecar，其余格式先解码 16k wav）。
/// 会议导入与录音卡同步共用此入口。上报 meeting-progress 同款进度事件。
pub async fn transcribe_audio(app: &AppHandle, audio: &Path) -> Result<String, String> {
    // 现 import_inner 的第 1~2 步原样搬入（含 is_ogg_opus 分支与 run_sidecar 调用），
    // 返回 transcript；「未识别到有效语音内容」错误也在这里返回
}
```

import_inner 改为调 `transcribe_audio(app, &src)`，后续入库/纪要逻辑不动。

**Step 2: 验证** `cd src-tauri && cargo test && cargo check` → 全过。

**Step 3: Commit** `refactor(meeting): 抽出 transcribe_audio 供录音卡复用`

---

## Task 5: Rust pnote 模块 —— sidecar 进程管理与命令

**Files:**
- Create: `src-tauri/src/pnote.rs`
- Modify: `src-tauri/src/lib.rs`（mod + AppState + 注册 command）
- Modify: `src-tauri/src/commands.rs`（无需，command 放 pnote.rs 里，照 meeting.rs 惯例）

**Step 1: AppState 加字段**（lib.rs:20-37）

```rust
pub pnote_child: Mutex<Option<tokio::process::Child>>,   // 常驻 sidecar 句柄
pub pnote_stdin: Mutex<Option<tokio::process::ChildStdin>>, // 控制通道写端
pub pnote_req_seq: AtomicU64,
pub pnote_latest: Mutex<PnoteStatus>,                    // 最近一次 state 事件快照
pub pnote_busy: AtomicBool,                              // 同步/转写重入守卫
```

`PnoteStatus`（serde camelCase）：`{running, connected, battery: Option<i64>, charging, charging 并入 battery==110, capacity: Option<[i64;2]>, version: Option<String>, sleeping, pending: i64}`。

**Step 2: pnote_sidecar_exe(app)**——照 meeting.rs:125-145 `sidecar_exe` 同构：`pnote-sidecar/pnote.exe`，resource_dir → dev `src-tauri/resources/` 回退。

**Step 3: ensure_sidecar(app) -> Result<()>**——pnote_child 为 None 或已退出时重新 spawn：
- 参数 `--out-dir <app_data_dir>/recorder-audio` `--ffmpeg <settings.pnote_ffmpeg_path 可空>`
- 三管道 piped + CREATE_NO_WINDOW（0x08000000，照 tts.rs:444-449）
- spawn 后登记 child+stdin 到 AppState
- stdout 读循环：BufReader lines → json → 区分 resp/event：
  - `event` → 更新 pnote_latest；`state`/`file_synced`/`progress` 用 `app.emit` 发前端（事件名 `pnote-state` / `pnote-synced` / `pnote-progress`，payload camelCase）
  - `resp` → 按 id 分发到挂起的 oneshot channel（HashMap<u64, oneshot::Sender>，Mutex 保护）
- stderr 循环：照 tts.rs:456-471（`MASAIFU_LOG` → log::info，tail 保 6 行）
- **子进程退出监控**：spawn 一个 task 等 child.wait()，退出时清 AppState 句柄并 emit `pnote-state`（connected:false, running:false）——崩溃后由下一次 ensure_sidecar 拉起（Task 6 的调用方负责）

**Step 4: 请求辅助**

```rust
async fn pnote_request<T: serde::de::DeserializeOwned>(app: &AppHandle, cmd: &str, extra: serde_json::Value) -> Result<T, String>
```
ensure_sidecar → 组 JSON 行（id = req_seq++）→ 写 stdin + flush → 等 oneshot（超时 30s，`tokio::time::timeout`）→ 解析 resp。

**Step 5: commands**（照 meeting.rs asr_model_status 风格）

```rust
#[tauri::command] pub async fn pnote_status(app: AppHandle) -> Result<PnoteStatus, String>      // 先 ensure（pnote_enabled 时），再发 status 请求合并本地 pending 数
#[tauri::command] pub async fn pnote_sync_now(app: AppHandle) -> Result<(), String>            // sync_now；busy 守卫
#[tauri::command] pub async fn pnote_file_list(app: AppHandle) -> Result<serde_json::Value, String>
#[tauri::command] pub async fn pnote_set_enabled(app: AppHandle, enabled: bool) -> Result<(), String> // 存 settings；true→ensure+start；false→stop+kill
```

lib.rs 注册 4 个 command。

**Step 6: 验证** `cargo check` 过；无设备环境 `cargo test` 过（进程管理逻辑不进单测）。

**Step 7: Commit** `feat(pnote): Rust 常驻 sidecar 管理与控制命令`

---

## Task 6: 同步编排 —— file_synced → 转写 → 入库

**Files:**
- Modify: `src-tauri/src/pnote.rs`

**Step 1: 事件处理**：stdout 循环里收到 `file_synced` 时：
1. `db::recorder_file_upsert(name, size, seconds, "done")` + `recorder_file_mark(..., ogg_path)`
2. spawn 转写任务（不阻塞事件循环）：
   - busy 守卫（pnote_busy，和 meeting_busy 独立，但转写内部仍要拿 funasr——meeting.rs 的转写有 meeting_busy 吗？import_meeting_audio 有。转写前检查 meeting 没在导入，在则 sleep 重试，复用「忙则排队」语义）
   - `let text = meeting::transcribe_audio(&app, Path::new(&ogg)).await?`
   - `db::insert_entry(&text-with-时长/文件名前缀, "recorder", dedup_daily)` → `mark_processed` 不需要 AI 整理？——**设计决策：不生成 AI 整理，直接以原文入库**（录音卡多为短条目，AI 整理留给后续统一处理；insert 后 processed 状态照 clipboard 流程看 db.rs 惯例，跟随 meeting 的 raw_fallback 路径：标题留空）
   - `recorder_file_mark(name, "done", entry_id, ogg)`
   - `app.emit("pnote-synced", {"newEntries": n})`（前端据此刷新列表+计数+toast）
3. 出错：mark `failed` + log::warn，下次 sync_now 重试（重试逻辑：sync_now 时把 failed 的 ogg 直接重转写，文件还在）。

**Step 2: sync_now 编排**：请求 sidecar `sync_now` 前，先比对一次 recorder_files 与 sidecar file_list？——不必，sidecar 内部已按本地存在性跳过（daemon 逻辑）。sync_now 返回后触发 device gc（见 Task 7）。

**Step 3: 验证** `cargo check && cargo test`；有设备时真机联调放 Task 9。

**Step 4: Commit** `feat(pnote): file_synced 转写入库流水线`

---

## Task 7: 清理任务（本地 30 天 + 设备容量 GC）

**Files:**
- Modify: `src-tauri/src/tasks.rs`（加 `pnote_cleanup_loop`）
- Modify: `src-tauri/src/lib.rs`（spawn 注册）

**Step 1: 本地音频清理循环**（照 clipboard_loop 模式，tasks.rs:34-68）

```rust
async fn pnote_cleanup_loop(app: AppHandle) {
    loop {
        tokio::time::sleep(StdDuration::from_secs(3600)).await;   // 每小时看一眼
        let s = settings_of(&app);
        if !s.pnote_enabled { continue; }
        let keep_days = s.pnote_keep_days.max(1);
        // 遍历 app_data_dir/recorder-audio/*.ogg，mtime 早于 keep_days 天的删除
        // 删完 log::info 汇总
    }
}
```

**Step 2: 设备 GC**：`pnote.rs` 里 `maybe_gc_device(app)`：
- 条件：`pnote_latest.capacity` 存在且 `remain * 2 < total`
- 候选：`db::recorder_files_done_before(&conn, 30)`（status='done' 且 ogg 落盘时间 >30 天——用 ogg_path 文件 mtime 判断）
- 调 `pnote_request("gc_device", {"names": candidates})`，返回 deleted 数 emit `pnote-state` 附注
- 触发点：每次收到带 capacity 的 state 事件后（debounce：一轮最多一次，记 Mutex<Option<String>> 上次日期）

**Step 3: 单测**：`recorder_files_done_before` 边界（db.rs tests，造数据验证 status 过滤）；文件 mtime 判断抽纯函数 `is_older_than(path_meta, days)` 好测——放 pnote.rs `#[cfg(test)]`。

**Step 4: Commit** `feat(pnote): 本地归档与设备容量清理`

---

## Task 8: 设置项

**Files:**
- Modify: `src-tauri/src/settings.rs`（struct + Default）
- Modify: `src/lib/types.ts`（Settings 接口）
- Modify: `src/views/SettingsView.svelte`（section）

**Step 1: settings.rs 加字段**（三处：struct / Default）

```rust
pub pnote_enabled: bool,          // default false
pub pnote_keep_days: i64,         // default 30
pub pnote_ffmpeg_path: String,    // default ""（为空则不校验时长，转写兜底）
```

**Step 2: types.ts Settings 接口加** `pnoteEnabled: boolean; pnoteKeepDays: number; pnoteFfmpegPath: string;`

**Step 3: SettingsView.svelte**
- `sections` 数组加 `{ id: "pnote", label: "录音卡" }`（SettingsView.svelte:38-44）
- `form` 初始对象加三个字段（SettingsView.svelte:8-28）
- 新增 `{:else if section === "pnote"}` 分支：启用开关（checkbox）、保留天数（number input）、ffmpeg 路径（text input + hint「留空则不校验时长；录音卡同步需要 Windows 蓝牙」）
- 启用开关 onChange 即时调 `api.pnoteSetEnabled(form.pnoteEnabled)`（不依赖底部保存按钮，照会议模型的即时生效风格）

**Step 4: api.ts 加** `pnoteSetEnabled: (enabled: boolean) => invoke<void>("pnote_set_enabled", { enabled }),` 及 types.ts `PnoteStatus` 接口。

**Step 5: 验证** `cargo check` + `npm run build` 过。

**Step 6: Commit** `feat(pnote): 设置项（启用/保留天数/ffmpeg 路径）`

---

## Task 9: 前端页面

**Files:**
- Modify: `src/views/BrowseView.svelte`
- Modify: `src/lib/types.ts`（SOURCE_LABEL）
- Modify: `src/lib/api.ts`（sourceLabel 已有 SOURCE_LABEL？api.ts:124-133 有自己的映射——两处都要加）

**Step 1: 来源菜单**（BrowseView.svelte:488-499 + :160）
- 数组 `["clipboard", "manual", "meeting", "recorder"]`
- `SOURCE_ICON` 加 `recorder: "🎙️"`
- 未同步计数徽标：`source:recorder` 的树项 count 显示 `stats.bySource` 计数 + pending（pending 从 `pnote_status` 拿，listen `pnote-state` 刷新）

**Step 2: 设备面板**（照 meeting 工具区模板，BrowseView.svelte:727-746 同位置加分支）

```svelte
{#if selectedKey === "source:recorder"}
  <div class="pnote-panel">
    {#if pnoteStatus?.connected}
      <span class="dot ok"></span> 已连接 · 电量 {pnoteStatus.battery}{pnoteStatus.charging ? "⚡" : ""}%
      · 剩余容量 {fmtKb(pnoteStatus.capacity[0])} · 固件 {pnoteStatus.version}
    {:else if pnoteStatus?.sleeping}
      <span class="dot warn"></span> 设备休眠，按录音卡按键唤醒后自动连接
    {:else}
      <span class="dot"></span> 未连接（在「设置 → 录音卡」启用）
    {/if}
    <button class="primary" on:click={pnoteSyncNow} disabled={pnoteSyncing || !pnoteStatus?.connected}>
      立即同步</button>
    {#if pnoteSyncing}<span class="spinner"></span> {pnoteStage}{/if}
  </div>
{/if}
```

- onMount 加 `listen("pnote-state", e => pnoteStatus = e.payload)` 与 `listen("pnote-synced", ...)`（toast「新增 N 条录音，已转写入库」+ refreshCurrent + refreshStats）
- 选中 `source:recorder` 时拉一次 `api.pnoteStatus()`
- listen 清理照现有 cleanup 模式

**Step 3: 验证** `npm run build` 过。

**Step 4: Commit** `feat(pnote): 录音卡来源页面与设备面板`

---

## Task 10: 端到端真机验收

按设计 §5 的 8 条清单逐条过（需要用户配合操作录音卡）。前置：
- 设置页启用录音卡 → 观察面板变绿（电量/版本正确——**电量必须与设备显示一致**，严禁信 GATT 0x2A19）
- `cargo test` 全绿、`npm run tauri dev` 无报错
- 验收中任何问题先记 `docs/plans/` 或修完再验

**最终 Commit** `feat(pnote): 录音卡 7x24 无感同步端到端验收通过`

---

## 协议怪癖必守清单（实现时逐条核对，来自测试记录）

1. 下载请求帧 36B **必须整帧单次 GATT 写**（拆写解析失败）
2. 下载中收到首个 ACK 帧**必须断连**（6/6 复现的固件 bug）
3. 电量只信协议 `0-3` 查询，GATT `0x2A19` 读数不可靠
4. 只连 5 秒内新鲜广播（`find_device` 已有）
5. 连接后必须 `start_event_router()`（按键回执）；断开前 `stop_event_router()`
6. 所有 GATT 订阅/写入带 10s 超时（防 Windows GATT 永久阻塞）
7. 事件路由必须经独立 data_events 队列（不能就地放回 events 队列——忙等死循环）
8. 边录边下拿的是半成品快照，时长校验必然拒绝——天然安全，保持校验
9. 笔反复连接 25+ 次后笔侧栈卡死 → 自动重连要有次数上限，超限提示用户重启录音笔
