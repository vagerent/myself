# 每周汇总设计

日期：2026-08-11
状态：已与用户确认

## 需求

- 内容来源：**汇总原始条目**（上周全部条目，不依赖每日汇总）
- 触发时间：**可配置**（设置页：星期几 + 时间）
- 提示词模板：**独立模板**，占位符 `{week}` `{range}` `{entries}`

## 设计

### 调度与配置

- `Settings` 新增：`weekly_summary_day`（u8，1=周一..7=周日，默认 1）、`weekly_summary_time`（HH:MM，默认 00:15）、`weekly_prompt_template`
- 复用 `summary_scheduler_loop`：到点汇总刚过去的完整一周（周一至周日），周标识 ISO 周（`2026-W33`）
- 失败重试与每日一致（中午前每 10 分钟重试），`AppState` 增加 `last_weekly_attempt` / `next_weekly_retry`

### 生成

- `entries_for_range(conn, start, end)` 范围查询（字符串比较）；`unprocessed_for_range` 补齐预处理
- `run_weekly_summary(app, week_key, start, end)`：无 API Key 退化为原始条目列表；无条目写占位
- Markdown：默认 `文档\Masaifu\weekly\2026-W33.md`；自定义目录时直接写该目录

### 存储与展示

- 复用 `summaries` 表，主键存 `2026-W33`；FTS 同步（标题「每周汇总 2026-W33」）
- `rerun_summary` 按 key 是否匹配 `-W\d{2}` 分发到日/周生成函数
- BrowseView 树：「每日汇总」后新增「每周汇总」节点（📆），点击右侧查看/重新生成

### 前端

- `types.ts` Settings 增加 3 个字段；SettingsView 增加星期下拉 + 时间 + 模板 textarea
- BrowseView：summaries 按 `-W\d{2}$` 拆分为日/周两个列表

## 验证

- 单测：周 key/范围计算、entries_for_range
- `cargo test`、`npm run build`
- 手工：设置周一 00:15 或调时间触发；树中查看周报；重新生成
