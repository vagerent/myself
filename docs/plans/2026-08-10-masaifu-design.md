# 马赛福（Masaifu）设计方案

> 状态：已实现（v0.1.0）　日期：2026-08-10　作者：vager

## 概述

一个 Tauri v2 桌面应用，常驻系统托盘：剪贴板文本自动入库 SQLite，空闲时由 AI 预处理（清洗、提取标题、分类、打标签）；每日定时生成 AI 汇总（Markdown 文件 + 库内记录 + 托盘提醒）；托盘点击弹出紧凑搜索面板，输入即得关键词结果，同时异步给出 AI 综合回答。支持展开为完整窗口浏览与编辑。

## 关键决策（与用户确认）

| 主题 | 决策 |
| --- | --- |
| 技术栈 | Tauri 2（Rust 后端 + Svelte/TS 前端） |
| AI 服务 | OpenAI API（可配置 base_url / 模型） |
| 捕获范围 | 仅文本，全量入库（用户选择不过滤敏感内容） |
| 去重 | 内容 SHA-256 哈希，重复内容不重复入库 |
| 预处理 | 机器空闲（默认 >5 分钟）时批处理当天未处理条目，带 `processed` 标记；夜间汇总前兜底补跑 |
| 预处理产出 | 清洗后正文 + 标题 + 固定分类（默认 灵感/代码/文章/链接/待办/其他，可配置）+ 自由标签 |
| 汇总时间 | 可配置，默认次日 00:15 汇总前一天 |
| 汇总结构 | 用户可编辑提示词模板（占位符 `{date}`、`{entries}`），默认模板为“总览 + 分类分节 + 值得回看” |
| 汇总交付 | SQLite 记录 + 本地 Markdown 文件（默认 `文档\Masaifu\daily\YYYY-MM-DD.md`）+ 系统通知 |
| 搜索 | FTS5 trigram 全文检索（中文子串），条目与汇总混合结果，不设 Tab 与筛选；命中前 10 条异步喂给 AI 生成回答（可关闭） |
| 弹窗形态 | 默认紧凑面板（760×560），可展开完整窗口（1080×720） |
| 数据存放 | 全部本机：数据库与设置 `%APPDATA%\com.masaifu.desktop\` |
| 二期规划 | 语义搜索（向量检索）、图片/文件捕获 + OCR |

## 实现要点

### 技术栈

- Tauri 2 + Rust（tray-icon、single-instance、autostart、global-shortcut、notification 插件）
- Svelte 4 + TypeScript + Vite 前端，无 UI 框架依赖
- SQLite（rusqlite bundled，FTS5 trigram 分词）
- OpenAI Chat Completions API（reqwest + tokio）
- 剪贴板轮询 arboard（默认 500ms）
- 空闲检测 Windows `GetLastInputInfo`
- 打包：NSIS 安装包（`Masaifu_0.1.0_x64-setup.exe`）

### 数据流

1. **捕获**：剪贴板文本每 500ms 轮询，去空、SHA-256 去重后写入 `entries`（`source=clipboard`）；手动输入走同一张表（`source=manual`）。
2. **预处理**：空闲检测通过后，取未处理条目（每批 20 条）调用 OpenAI 批量清洗，产出 `clean_content / title / category / tags`，标记 `processed=1`；失败记日志，下一空闲周期重试。
3. **每日汇总**：到设定时间（默认 00:15）取前一天全部条目 → 先补跑未处理条目 → 分块（每 25 条）生成一句话摘要 → 按用户提示词模板组装最终 Markdown → 写文件 + 写 `summaries` 表 + 系统通知；失败每 10 分钟重试至当日中午，完整窗口可手动重跑任一天。
4. **搜索**：FTS5 trigram 索引（raw + clean + title + category + tags + 汇总正文），3 字符及以上走 MATCH，短查询回退 LIKE；结果条目/汇总混合，点击复制原文，双击进入详情编辑；「AI 回答」将前 10 条命中异步提交 OpenAI 生成带引用回答。

### 数据模型

- `entries(id, raw_content, content_hash UNIQUE, clean_content, title, category, tags, source, processed, created_at, processed_at)`
- `summaries(date PK, content, markdown_path, status, created_at)`
- `entries_fts`（FTS5 虚表：`doc_type, doc_id, raw_content, clean_content, title, category, tags, summary_text`）
- 设置存 JSON 文件 `settings.json`（API Key、base_url、模型、汇总时间、空闲阈值、快捷键、Markdown 目录、提示词模板、自启、AI 回答开关、轮询间隔）

### 交互

- 托盘左键点击切换窗口；右键菜单：打开/搜索、查看今日汇总、设置、退出
- 全局快捷键默认 `Ctrl+Alt+Space`（设置中可改，修改即时生效）
- 重新唤起时回到精简搜索状态；「查看今日汇总」「设置」以完整窗口打开
- 精简面板右上角为「最大化」「关闭」；最大化进入分类浏览页，左下角为设置入口
- 紧凑面板：搜索框 + 结果列表 + AI 回答区 + 底部灵感输入（Ctrl+Enter 保存）；空查询不列出剪贴板数据，保证轻快
- 完整窗口：左侧分类树（全部条目 / 分类 / 来源 / 每日汇总，带计数）+ 右侧条目列表与详情编辑、汇总 Markdown 渲染；列表顶部支持在当前节点内按标题 / 内容 / 标签搜索；设置表单
- 主题：亮色 / 暗色 / 跟随系统，设置页可选，CSS 变量切换；界面为 Notion 风格（细边框、低饱和、琥珀橙强调色、淡灰侧栏），默认亮色
- Esc 隐藏窗口

## 测试计划

- 捕获：复制文本入库、重复复制不重复、超长文本、连续快速复制、无内容稳定运行
- 预处理：空闲触发、忙碌暂停、断网重试、`processed` 标记正确、批处理不阻塞界面
- 汇总：定时触发、自定义提示词生效、Markdown 结构与内容正确、API 失败重试、手动重跑
- 搜索：中英文与子串命中、条目/汇总混合结果、AI 回答异步出现、无结果提示、点击复制、开关生效
- 系统：全局快捷键唤起、托盘退出、开机自启、单实例运行、打包后正常启动

## 验证记录

- `cargo check`：零错误零警告
- `vite build`：通过（45 模块）
- `npm run tauri build`：release 编译 + NSIS 打包成功
- 冒烟测试：exe 启动 8 秒进程稳定，数据库（WAL）与默认设置正常创建

## 假设与默认值

- UI 全中文；应用名“马赛福”，工程标识 `masaifu`，bundle identifier `com.masaifu.desktop`
- API Key 存本地 `settings.json`（仅当前用户可读），v1 不接系统凭据库
- 剪贴板全量入库不做敏感内容过滤（用户明确选择）
- 单机单实例，无多设备同步

## 运行与构建

```bash
npm install
npm run tauri dev      # 开发调试
npm run tauri build    # 构建安装包
```

产物：`src-tauri/target/release/masaifu.exe` 与 `src-tauri/target/release/bundle/nsis/Masaifu_0.1.0_x64-setup.exe`
