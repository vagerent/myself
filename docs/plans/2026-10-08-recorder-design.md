# 录音卡 7×24 无感同步 · 设计定稿

> 日期：2026-10-08
> 状态：已确认（§1–§5 与需求方逐节对齐）
> 依赖：录音卡长连接测试已全部验证（见 `录音卡/pnote_sync/测试记录与使用手册.md`）

## 背景与目标

左侧「来源」菜单新增「录音卡」页面。录音卡（CB08 录音笔）通过蓝牙 BLE 与桌面 App
保持长连接，录音保存后 6 秒内自动同步到电脑、转写为可搜索的条目，全程无感。

已实测的硬约束（来自录音卡侧测试记录）：

- 设备休眠后停止广播、必须按键唤醒；**连接状态下不休眠**，空闲连接 90+ 分钟零掉线
- 保活方式：每 30 秒一次协议电量查询；挂线功耗约 2%/小时（满电约 2 天）
- BLE 单主机：App 挂线期间手机官方 App 连不上设备
- 传输速率约 18KB/s：60 分钟音频 ≈ 12MB ≈ 11 分钟下载；协议原生 offset 断点续传
- 长录音在整 60 分钟处自动拆段，每段独立文件、独立命名（段开始时间戳）
- 设备文件为私有容器（40B 头 + 20ms Opus 包），需无损重封装为 OGG（16kHz 单声道）
- 守护程序底座 `daemon.py` 已验收：按键事件自动回执、事件路由、GATT 订阅 10s 超时、
  笔侧蓝牙栈卡死自愈（重启设备）

## 已确认的决策

| 决策点 | 结论 |
|---|---|
| 数据形态 | 转写为条目（source='recorder'）+ OGG 音频归档本地 |
| 归档音频清理 | 本地音频 >30 天自动删除（文本条目保留） |
| 同步触发 | 保活长连接（非轮询）：AE23 事件驱动 + 每 30s 保活查询顺带发现新文件 |
| 拆段处理 | 每段一个 entry（不做分段合并） |
| 设备端清理 | 设备剩余容量 <50% 时，自动删除设备上 30 天前的录音文件（仅删本地已确认转写成功且 OGG 校验通过的） |
| v1 明确不做 | TYPE=1 实时音频流、TYPE-3 远程开始/停止录音 |

## §1 总体架构

```
masaifu (Tauri 2 + Svelte 4)
├─ 前端：BrowseView 左侧「来源」新增「🎙️ 录音卡」
│    选中后右侧设备面板：连接状态、电量、容量、固件版本、最近同步时间、
│    未同步计数、设备文件列表（折叠）、立即同步按钮
├─ Rust 新增 recorder.rs
│    · 常驻管理 pnote-sidecar 子进程（App 启动拉起，退出回收）
│    · command：pnote_status / pnote_sync_now / pnote_file_list / pnote_set_enabled …
│    · sidecar 事件驱动：录音停止 → 增量同步；断线 → 退避重连 → 重连后补同步
│    · 同步完成的 OGG 走「会议」同款转写流水线入库（source='recorder'）
│    · 归档目录 + 每日清理任务（>30 天音频）
└─ pnote-sidecar（PyInstaller 冻结，基于已验收 daemon.py 架构）
     · pnote.py / opuswrap.py / daemon.py 原样 import，不重写协议层
     · stdin/stdout 逐行 JSON 控制协议；stderr 推事件（复用 MASAIFU_PROGRESS 约定）
     · 内部维持 bleak 长连接、30s 保活对时/电量、订阅 AE23 录音状态
```

技术选型：不走 Rust 重写 BLE（bleak 侧 Windows 蓝牙栈坑已踩完），不走 Web
Bluetooth（WebView2 不支持）。复用 funasr/tts 的既有 sidecar  toolchain
（Python 3.12 + PyInstaller one-dir → `src-tauri/resources/pnote-sidecar/`）。

## §2 数据流

```
① AE23「录音已停止」事件（或保活轮询发现新文件）
        ↓
② 请求文件列表 → 与本地 recorder_files 表比对 → 增量下载新文件
   （协议原生 offset 续传，断线重连自动续）
        ↓
③ opuswrap 封装 OGG → ffmpeg/时长校验 → 落盘归档目录（.part 临时文件 + 改名）
        ↓
④ OGG 直接交给 funasr sidecar（2026-10-08 已验证：libsndfile 原生支持，
   16kHz ogg 零转码；非 16k 输入 sidecar 内已加重采样兜底）
        ↓
⑤ 写入 entries（source='recorder'，正文=转写文本+时长/文件名备注）
   recorder_files 记录：文件名、大小、时长、状态机、ogg 路径、entry_id
        ↓
⑥ 每日清理任务：归档目录 mtime >30 天的 OGG 删除（条目与 recorder_files 保留）
```

配套：

- **新表 `recorder_files`**：状态机 `listed → downloading → done / failed`；
  重启后按状态续传，重复同步靠它去重
- **归档目录**：`app_data_dir/recorder-audio/`（内部目录，30 天自动删）
- **转写输入**：复用 2026-10-08 刚落地的 ogg 直通能力（meeting.rs `is_ogg_opus` 分支）
- **失败处理**：OGG 校验/转写失败标 `failed`，不删设备原件，下次同步重试；
  与会议导入并发时复用 AtomicBool 忙标志排队
- **正在录制的当前文件不同步**（AE23 当前文件名排除），防止下载到增长中的半成品

## §3 前端页面与交互

- 左侧「来源」树形项：🎙️ 录音卡 + 未同步文件计数徽标（沿用 tree-count）
- 设备面板四态：已连接（绿，电量/容量/版本/上次同步）/ 同步中（进度条，
  recorder-progress 事件）/ 设备休眠待唤醒（黄，提示按键）/ 未找到设备（灰）
- 同步完成 toast：「新增 N 条录音，已转写入库」
- 条目展示：source='recorder' 徽标「录音卡」，复用现有 EntryDetail
  （编辑分类/标签、参与每日/每周汇总、SpeakBar 朗读）
- 设置页新增「录音卡」section：启用/关闭、归档保留天数（默认 30）、
  ffmpeg 路径（校验用）、保活间隔
- v1 不做：实时音频流、远程录音控制

## §4 错误处理与边界

- 设备不在/休眠：扫描超时静默，重试退避 1min → 5min → 15min 封顶；UI 黄态
- BLE 被占用/系统蓝牙关闭：sidecar 回具体错误码，toast 提示
- 传输中断：offset 续传 + 12s 无数据判超时重连；App 重启按 recorder_files 恢复
- 删除保护（设备端）：仅当 本地 done + OGG 校验通过 + 记录时间>30 天 + 容量<50%
  四条件同时满足才自动删
- 删除保护（本地）：只删 OGG 文件，entries 文本与 recorder_files 记录保留
- 进程生命周期：App 启动拉起、退出回收（复用 tts_child 模式）；
  sidecar 崩溃检测退出码自动重启并续同步
- 固件怪癖（来自测试记录，实现时严格遵守）：下载请求帧 36B 必须整帧单写、
  收到首个 ACK 帧必须断连、电量只信协议 0-3 查询（勿读 GATT 0x2A19）

## §5 测试与验证

- sidecar：协议层不 mock，真机冒烟；控制层做 JSON 协议干跑自测
- Rust：recorder_files 状态机单测、清理任务边界单测（30 天/容量 50%）、并发排队
- 真机端到端验收清单：
  1. 首次配对（扫描→连接→对时→面板信息）
  2. 录一段→6 秒内入库→自动转写→时间线出现条目
  3. 60 分钟拆段→两段各成一条 entry
  4. 断链→重连→offset 续传→OGG 校验通过
  5. 休眠→黄态提示；唤醒→自动重连补同步
  6. 容量 <50%→清理 30 天前已同步设备文件
  7. 本地归档满 30 天自动删（文本保留）
  8. App 重启→守护恢复→未完成续传
- 回归：`cargo test` + sidecar 冒烟（ogg 直通已验证）
