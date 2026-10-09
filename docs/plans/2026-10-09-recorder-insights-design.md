# 录音洞察设计（录音卡事件分析独立化）

日期：2026-10-09 ｜ 状态：已与用户对核定稿 ｜ 前置依赖：录音卡自动深度解析（`35bfedd`）

## 目标

把「分析录音事件」从录音卡面板独立为「录音洞察」：汇总区独立菜单，展示每次分析运行与发现的洞察流 + 跨天项目脉络；支持间隔自动分析与手动分析；让系统捡起用户「说过就忘」的事，制造 wow 时刻。

## 已确认的用户决策

1. 范围做全量四类洞察（原讨论 C 方案）：双向承诺、灵感回收、未闭环、项目脉络
2. **只通过深度解析内容分析**，不看转写原文
3. 窗口分层：承诺/灵感走增量；未闭环与脉络由跨次运行的滚动 state 推导（60 天修剪）
4. 触发用**间隔调度**（如每 3 小时），不做「同步后触发 / 每天定时」
5. 录音卡深度解析默认提示词改为**内置个人记录导向**（原 A 案），不再跟随会议纪要提示词
6. **不做历史回填**（原 b 案），脉络从今天开始自然生长

## 数据模型（SQLite）

- `recorder_analysis_runs(id, started_at, trigger, analyzed_count, status, error)`：每次运行留痕，失败也记录（可见性 = 信任）
- `recorder_analysis_findings(id, run_id, kind, title, note, quote, entry_id, event_date, status, reminder_id, created_at)`
  - kind：`commitment_mine` / `commitment_other` / `idea` / `open_loop` / `project_thread`
  - status：`new → done | dismissed`；**dismissed 是墓碑，以后永不再报**；删除提醒 ≠ 忽略
- `recorder_project_state(id=1 单行, state_json, updated_at)`：滚动「项目脉络 + 未闭环台账」，每个项目/待办带最后提及日期；超 60 天未提及自动归档（修剪规则即 60 天窗口）
- `entries` 新增列 `recorder_analyzed_at`：候选集 = `source='recorder' AND deep_analysis IS NOT NULL AND recorder_analyzed_at IS NULL`，自愈合、无需水位线

## 运行机制

每次运行一次 LLM 调用：

- 输入：新增深度解析（每篇带录制日期）+ 上次 state + 已有活跃 findings（去重比对）
- 输出：`{ findings: [...], state: {...} }`，state 整体替换
- 每 run 最多消化 50 篇深度解析，超出下个周期继续（成本与延迟天然受限）
- 成功后才 stamp `recorder_analyzed_at`；失败不 stamp，下个周期整体重试
- 空跑（无候选）直接跳过，不产生 run 记录、不调 API

## 发现类型与推送路由

| 类型 | 自动动作 | 说明 |
|---|---|---|
| commitment_mine（有日期） | 自动建提醒并回链 reminder_id | 我要做的 |
| commitment_other | 仅进洞察流，一键转提醒 | 别人答应我的 |
| idea | 进洞察流，一键转灵感条目 | 「我有个想法…」回收 |
| open_loop | 进洞察流，建议日期可转提醒 | 「N 天前说要 X，之后再没提」⭐ |
| project_thread | 脉络页签 | 跨天时间线，越滚越厚 ⭐ |

wow 来源： forgotten open loops + 跨天脉络；每条 finding 必须带**原文引文 + 来源条目**（可回听核验，信任地基）。

## 界面

- 汇总区新增「录音洞察」菜单：未读角标 = `status='new'` 计数
- 发现页签：按 run 分组，0 结果也展示（敢报零才证明不瞎编）；每条可展开引文、跳转来源条目、转提醒/转条目/忽略
- 脉络页签：project_state 渲染的跨天项目时间线
- 录音卡面板的旧「分析录音事件」按钮移除（独立化）
- 手动「立即分析」按钮放洞察页

## 设置

- 分析间隔：1/2/3/6/12/24 小时或纯手动，默认 3 小时
- 「事件分析提示词」原地升级为「洞察分析提示词」（settings 字段 `recorder_event_prompt` 复用）
- 录音卡深度解析默认提示词（新常量 `DEFAULT_RECORDER_DEEP_PROMPT`）：个人记录导向——时间线梳理做了什么、想到什么、待办、涉及的人和数字；`recorder_deep_prompt` 默认值从「留空跟随会议」改为该常量，仍可自定义；解析方法 `resolve_recorder_deep_prompt` 空值兜底改为新常量（纪要提示词维持跟随会议不变）

## 触发实现

App 启动后 tokio interval 循环（重启时钟从 0 起算，偏差 ≤ 一个间隔，可接受）；间隔设置变更即时重建。分析引擎与调度解耦，调度只负责 fire。

## 不做 / YAGNI

- 历史深度解析回填（用户选 b：脉络从今天自然生长）
- 情绪/健康信号、人际雷达、时间模式分析（v2 再议）
- 用户反馈「有用/没用」闭环（v2）
