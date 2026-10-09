# 录音洞察实施计划

日期：2026-10-09 ｜ 设计：`2026-10-09-recorder-insights-design.md`（先读）

## 任务拆分

### 任务1：db.rs 数据层
- 新表 `recorder_analysis_runs(id PK, started_at, trigger, analyzed_count, status, error)`
- 新表 `recorder_analysis_findings(id PK, run_id, kind, title, note, quote, entry_id, event_date, status, reminder_id, created_at)`，索引 (run_id)、(status)
- 新表 `recorder_project_state(id PK CHECK(id=1), state_json, updated_at)`
- `entries` 加列 `recorder_analyzed_at TEXT NULL`（照 `deep_analysis` 列的迁移模式，见 `init_adds_deep_analysis_column_to_old_db` 测试）
- 查询：`recorder_analysis_candidates(conn, limit)`（source='recorder' AND deep_analysis NOT NULL AND recorder_analyzed_at IS NULL，按 id 升序）、`mark_recorder_analyzed(conn, ids)`、`insert_run`、`list_runs(conn, limit)`、`insert_finding`、`finding_exists_dedup(conn, kind, normalized_title)`（非 dismissed 同 kind 同标题）、`list_findings(conn, run_id)`、`findings_unread_count`、`set_finding_status(conn, id, status)`、`link_finding_reminder(conn, id, reminder_id)`、`load_project_state -> Option<String>`、`save_project_state(conn, json)`
- 测试：旧库迁移加列、candidates+stamp、dedup、unread count、state 读写幂等

### 任务2：settings.rs + openai.rs
- 新常量 `DEFAULT_RECORDER_DEEP_PROMPT`（个人记录导向：时间线梳理做了什么/想到什么/待办/涉及的人和数字；作为 system 提示词，转写作为用户消息，直出 Markdown 结构化记录）
- `recorder_deep_prompt` 默认值改为该常量；`resolve_recorder_deep_prompt` 空值兜底 = 该常量（**不再跟随会议**）；`resolve_recorder_minutes_prompt` 不变
- 新常量 `DEFAULT_RECORDER_INSIGHT_PROMPT`（洞察分析提示词：输入为带日期的深度解析集合+项目状态+活跃发现，输出严格 JSON `{"findings":[{kind,title,note,quote,event_date,ref}],"state":{...}}`；kind 枚举 commitment_mine/commitment_other/idea/open_loop；只提取明确说出的内容、必须带原文引文、不编造、与活跃发现去重；state 含 projects[{name,thread[{date,note}],open_items[{item,origin_date,last_mention,reported}]}] 与 archived，超 60 天未提及归档）；替换 `DEFAULT_RECORDER_EVENT_PROMPT` 作为 `recorder_event_prompt` 的默认值，旧常量删除
- 新字段 `recorder_analysis_interval_hours: i64` 默认 3（0 = 手动）
- openai.rs 加 `recorder_insights(&self, user: &str, prompt: &str) -> Result<String>`（照 `recorder_events`：chat + 截断）

### 任务3：insights.rs 引擎 + 调度 + 接线
- 新模块 `src-tauri/src/insights.rs`：
  - `run_insights(app, trigger)`：busy 门控（AppState 加 `insights_busy: AtomicBool`）→ candidates(≤50)，空则直接返回不建 run → 载入 state + 活跃 findings → 拼 LLM 输入（每篇深度解析带 `[#序号] 录制于 YYYY-MM-DD（条目id）`，findings 的 ref=序号映射回 entry_id）→ `recorder_insights` → 抠第一个 `{` 至最后一个 `}` 解析 → 逐条校验（kind 合法、title 非空、event_date 非法置 NULL）→ dedup 后 insert（status='new'）→ commitment_mine 且 event_date 有效则建提醒（once 09:00，`compute_next_fire`+`insert_reminder`）并回链 reminder_id → state 修剪（last_mention 超 60 天移 archived）→ save → stamp entries → insert run(ok)。任一步 Err → insert run(failed, error) 且不 stamp（下轮重试）
  - commands：`insights_analyze_now`（busy 即报错）、`insights_runs`（最近 50 条 run+各自 findings 数）、`insights_findings(run_id?)`（默认最新 run）、`insights_unread`、`insights_dismiss(id)`、`insights_to_reminder(id)`（open_loop/commitment_other 转提醒回链）、`insights_threads`（解析 state_json 返回脉络）、`insights_mark_read`（new→done，查看即读）
  - 完成后 emit `insights-updated`
- tasks.rs：`insights_scheduler_loop`（照 summary_scheduler_loop 30s tick：interval>0 且距上次运行 ≥ interval 小时则 fire；settings 每轮现读，改设置即时生效）→ `spawn_background` 注册
- lib.rs：AppState 加字段、mod insights、注册 commands
- pnote.rs：删除旧 `analyze_recorder_events` 及其专用查询消费；lib.rs 注册表同步移除

### 任务4：前端
- types.ts：`InsightsRun`、`InsightsFinding`、thread 类型；Settings 加 `recorderAnalysisIntervalHours`
- api.ts：对应 7 个 command 绑定
- 新组件 `src/components/InsightsPanel.svelte`：发现页签（按 run 分组：时间、触发方式、分析条数、发现数；失败 run 红字显示错误；0 发现显示「本次没有发现值得提醒的事」；finding 卡片：kind 图标+标题+note+可展开引文+跳转来源条目+操作[转提醒/转条目/忽略]）+ 脉络页签（projects 时间线渲染，open_items 高亮未闭环⭐）+ 顶部「立即分析」按钮（busy 转圈）+ 监听 `insights-updated` 自动刷新
- BrowseView：汇总区加「录音洞察」tree-item（selectedKey="insights"，未读角标照 pnote-pending 样式）+ 主区 `{:else if selectedKey === "insights"} <InsightsPanel />`；**移除录音卡面板的「分析录音事件」按钮**及 analyzingEvents 相关 state
- SettingsView：录音卡区加「分析间隔」下拉（手动/1/2/3/6/12/24 小时）；「事件分析提示词」标签与 hint 改为「洞察分析提示词」

## 验证
- `cd src-tauri && cargo test --lib`（既有 39 + 新增全绿）
- 根目录 `npm run build`

## 提交纪律
- `types.ts`/`api.ts`/`SettingsView.svelte` 工作区混有未提交 tts 改动：提交这三个文件必须备份还原法（备份 → checkout HEAD → 重放本任务改动 → add → 还原备份 → commit → `git diff --cached` 确认无 tts）
- BrowseView.svelte、新文件、Rust 文件均干净，直接 add
- 旧 `analyze_recorder_events` 的删除与后端新引擎同批提交

## 明确不做（YAGNI）
- 历史深度解析回填（设计定稿 b）
- 情绪分析 / 人际雷达 / 用户反馈闭环（v2）
- findings 编辑、批量操作
