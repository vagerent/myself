# 马赛福（Masaifu）

> 常驻系统托盘的个人信息搜索与蒸馏工具：自动收集剪贴板与手动记录的灵感，经 AI 蒸馏整理，支持全文搜索、AI 问答、每日/每周汇总与任务提醒。数据全部存本机。

**官网**：https://myself.mavenzero.com/

## 截图

| AI 搜索与问答 | 每日汇总 |
| --- | --- |
| ![AI 搜索与问答](docs/screenshot-search-ai.png) | ![每日汇总](docs/screenshot-daily-summary.png) |

| 每周汇总 | 任务提醒 |
| --- | --- |
| ![每周汇总](docs/screenshot-weekly-summary.png) | ![任务提醒](docs/screenshot-reminder.png) |

## 功能特性

### 收集与蒸馏

- **剪贴板自动收集**：后台轮询剪贴板，文本自动入库 SQLite，无需任何操作
- **智能排重**：支持「当日排重」（同一天相同内容只记一次）或「全量排重」（相同内容永远只记一次），设置页可切换
- **AI 预处理**：空闲时自动清洗内容、提取标题、分类（灵感/代码/文章/链接/待办/其他）并打标签
- **手动记录**：随手记下灵感，立即入库

### 搜索与问答

- **全文搜索**：SQLite FTS5（trigram）全文检索，输入即出结果，条目与汇总混合命中，关键词高亮
- **AI 兜底搜索**：直接搜索无结果时，AI 自动从输入中提取多个搜索词分别重搜，各取前 5 条排重后呈现
- **AI 回答**：基于命中的资料生成回答，末尾附引用来源；搜索结果页内一键触发

### 汇总与提醒

- **每日汇总**：定时生成当日 Markdown 汇总（本地文件 + 库内记录 + 托盘提醒），提示词模板可自定义
- **每周汇总**：按配置的星期与时间自动生成上周（周一至周日）AI 周报，独立提示词模板
- **任务提醒**：单次 / 每天 / 每周提醒，到点弹出系统通知；错过的提醒不补发，循环提醒自动推进

### 窗口与体验

- **双窗口模式**：精简窗口（快速搜索 + 记录）⇄ 全量窗口（分类树浏览 + 管理），一键切换
- **窗口状态记忆**：两种模式各自记住窗口大小，位置共用，下次启动自动恢复
- **主题**：亮色 / 暗色 / 跟随系统，Notion 风格界面
- **状态栏**：侧栏底部实时显示当前日期时间与今日收集条数

## 安装

从 [Releases](https://github.com/vagerent/myself/releases) 下载 Windows 安装包（NSIS），或自行构建：

```bash
npm install
npm run tauri build
```

安装包输出在 `src-tauri/target/release/bundle/`。

## 开发

前置要求：Node.js 18+、Rust 工具链（Tauri 2 依赖，Windows 需 MSVC Build Tools）。

```bash
npm install
npm run tauri dev
```

## 使用

- **唤起窗口**：全局快捷键（默认 `Ctrl+Alt+Space`）或左键点击托盘图标；`Esc` 隐藏窗口
- **托盘菜单**（右键）：打开搜索、今日汇总、任务提醒、设置、退出
- **精简窗口**：搜索框输入即出关键词结果；按 Enter 或点「AI 回答」基于命中结果生成回答；底部输入框手动记录灵感；右上角「最大化」进入全量窗口
- **全量窗口**：左侧分类树（全部 / 分类 / 来源 / 每日汇总 / 每周汇总 / 定时任务），右侧条目列表与详情；搜索在当前选中节点内过滤，无结果时自动进入 AI 兜底搜索
- **设置页**：OpenAI API Key、模型、base_url、汇总时间、每周汇总（星期几 + 时间）、空闲阈值、快捷键、Markdown 输出目录、每日/每周提示词模板、排重方式、主题、开机自启

> AI 功能（预处理、汇总、问答、兜底搜索）依赖 OpenAI 兼容的 Chat Completions API，未配置 API Key 时自动跳过，不影响收集与搜索。

## 技术栈

- **后端**：Tauri 2（Rust）、rusqlite（SQLite + FTS5 trigram）、reqwest（OpenAI 兼容 API）
- **前端**：Svelte 4 + TypeScript + Vite

## 数据位置

| 数据 | 位置 |
| --- | --- |
| SQLite 数据库 / 设置 / 窗口状态 | `%APPDATA%\com.masaifu.desktop\` |
| 每日汇总 Markdown | `文档\Masaifu\daily\YYYY-MM-DD.md`（可修改） |
| 每周汇总 Markdown | `文档\Masaifu\weekly\YYYY-Www.md` |

## 隐私说明

- 所有数据（剪贴板记录、灵感、汇总）仅保存在本机，不上传任何服务器
- 仅在你配置 API Key 后，条目内容会发送到你自配置的 AI 接口用于预处理/汇总/问答
- 剪贴板默认全量收集，不做敏感内容过滤，请自行评估使用场景

## Roadmap

- 语义搜索（向量检索）
- 图片 / 文件捕获
