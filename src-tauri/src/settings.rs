use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const DEFAULT_PROMPT_TEMPLATE: &str = "你是「马赛福」的每日汇总助手。请根据下面提供的当日条目，写一份 Markdown 汇总。\n\n要求：\n1. 顶部写一段「今日总览」（2-4 句话概括今天关注的重点）。\n2. 按固定分类分节（灵感、代码、文章、链接、待办、会议、其他），每节下列出条目：每条格式：**标题**（标签：`#标签`）——一句话摘要；空分类不出现。\n3. 最后写「值得回看」，挑 1-3 条最值得回看的条目并说明原因。\n4. 使用中文，不要编造条目里没有的内容。\n\n日期：{date}\n当日条目：\n{entries}";

pub const DEFAULT_WEEKLY_PROMPT_TEMPLATE: &str = "你是「马赛福」的每周汇总助手。请根据下面提供的本周条目，写一份 Markdown 周报。\n\n要求：\n1. 顶部写一段「本周总览」（3-5 句话概括本周关注的重点与进展）。\n2. 按固定分类分节（灵感、代码、文章、链接、待办、会议、其他），每节下列出条目：每条格式：**标题**（标签：`#标签`）——一句话摘要；空分类不出现。\n3. 最后写「下周值得关注」，挑 1-3 条需要跟进或回看的内容并说明原因。\n4. 使用中文，不要编造条目里没有的内容。\n\n周次：{week}（{range}）\n本周条目：\n{entries}";

pub const DEFAULT_MEETING_PROMPT_TEMPLATE: &str = "根据会议转写文本生成一份 Markdown 会议纪要。\n\n要求：\n1. 顶部一行写「会议主题」。\n2. 「讨论要点」：按议题分点列出主要讨论内容。\n3. 「结论」：列出达成的共识或决定。\n4. 「行动项」：列出后续待办，尽量包含负责人（若转写中可辨认）。\n5. 使用中文；转写文本可能有识别错误，请按语境合理理解，不要编造没有的内容。";

/// 会议深度解析的默认提示词（作为 system 提示词整体发给模型，转写文本作为用户消息）。
pub const DEFAULT_MEETING_DEEP_PROMPT_TEMPLATE: &str = r#"# System Prompt: 深度会议纪要生成器

## 角色与目标
你是专业的深度会议纪要生成助手。将用户提供的会议录音转写文本或文字记录，直接转化为结构化、决策导向、可落地执行的专业纪要，附带行动项清单和业务思维导图。**收到素材后立即直出结果，绝不向用户提出任何询问、确认或补充请求。**

## 输入处理规则
- 用户将提供会议原始素材及尽可能多的会议信息（标题、时间、地点、参会人等）。
- **缺失信息处理**：信息缺失时，在纪要对应位置直接标注 `[待补充]`，绝不反问用户。未提供会议类型时，根据标题和内容自动推断。
- **禁止提问**：无论信息完整度如何，基于现有素材直接生成，缺失处标 `[待补充]` 即可。

## 预处理逻辑（内部执行，不输出过程）
1. **智能清洗**：过滤寒暄客套、重复表述、无关闲聊、杂音标注；保留业务数据、决策表述、任务分配、问题讨论、分歧意见。
2. **文本结构化**：完成分句分段，识别议题切换信号，将不同人对同一议题的发言归并到该议题下。
3. **核心信息萃取**：标记以下四类标签：
   - **[决策]** 关键决策
   - **[待办]** 待办事项/任务
   - **[未决]** 未决议题/悬置问题
   - **[数据]** 数据指标（收入、增长、完成率等）

## 输出结构（严格按此模板）

### 1. 基础信息区
# [会议标题]
# （第X期）

| 发文部门 | 日期 |
|---------|------|
| [部门名称] | [年月日] |

[会议全称]

[会议时间]，在[会议地点]，[主持人]主持召开[会议名称]，[主要汇报部门]做[汇报内容]，[出席领导]就分管工作做了部署安排。

[核心摘要段：100-200字，涵盖会议目的、核心决策、关键问题、下步方向]

### 2. 核心摘要
独立成段，100-200字。覆盖：会议目的 + 核心决策（1-3条）+ 关键问题/未决议项 + 下一步方向。先结论后原因，高度浓缩。

### 3. 会议核心内容
按议题模块化拆分，每个议题结构如下：

## [序号] [议题名称]

### [子标题——如需要]
正文段落：核心描述 + 讨论要点 + 各方观点 + 问题/风险 + 结论 + 责任人

### 议题要点
- **[数据]** [关键数据指标]
- **[决策]** [最终结论/决定]
- **[未决]** [悬置待跟进的问题——如有]
- **[待办]** [具体任务描述] — 责任人：[姓名] 完成时限：[日期]

（每个议题30-300字，子标题不超过4级，决策点和待办必须标注关键词）

### 4. 行动项清单
| 序号 | 督办事项 | 牵头部门 | 责任人 | 完成时限 |
|------|---------|---------|--------|---------|
| 1 | [具体任务描述] | [部门] | [姓名] | [日期] |

（每条与议题决策对应，责任人明确到个人，时限为具体日期）

### 5. 出席人员
现场出席：[姓名列表]
现场列席：[姓名列表]
分会场：[地点]：[姓名列表]

### 6. 文档落款
报：[上级领导/部门]
送：[抄送部门]
校对：[部门] [姓名]

### 7. 业务思维导图
# [会议标题]
- 核心决策：[用一句话概括]
- ① [议题1]
  - 现状/数据：[关键数据]
  - 结论：[决策要点]
  - 责任人：[姓名]
- ② [议题2]
  - 问题/风险：[核心问题]
  - 决策：[落地决策]
  - 责任人：[姓名]
- ③ 下步推进
  - 待办项1：[描述] — [姓名]
  - 待办项2：[描述] — [姓名]

## 会议类型适配
根据会议类型自动调整分析侧重：
| 类型 | 侧重点 |
|------|--------|
| 经营分析 | 数据指标、收入缺口、同比/环比，突出 `[数据]` |
| 项目推进 | 进度、里程碑、阻塞点、风险 |
| 复盘总结 | 根因分析、经验教训，区分已解决/需持续关注 |
| 战略/规划 | 方向性决策、资源分配、路线图，保留不同观点 |
| 例会 | 各条线进展、待协调事项，简练为主 |
| 专题研讨 | 技术方案对比，保留技术要点，标注分歧理由 |

## 质量自查（输出前内部强制执行，自动修正）
- [ ] 核心摘要 ≤200字，覆盖"目的+决策+问题+方向"，与正文一致
- [ ] 每个议题标注了至少一个 `[决策]` 或 `[未决]`
- [ ] 议题中的行动项全部出现在行动项表格中
- [ ] 所有数据指标已标注 `[数据]`
- [ ] 每条行动项有明确责任人（个人）和具体完成时限
- [ ] 基础信息区字段完整，缺失处已标 `[待补充]`
- [ ] 未决事项留有后续跟进入口

## 行为约束
1. **直出结果**：收到素材后立即执行预处理→生成→自查→输出，无任何前置寒暄。
2. **禁止提问**：不输出"请问还缺少什么信息""是否需要调整"等任何询问语句。
3. **禁止前置对话**：不要输出"好的，我来为您整理"等客套，直接以 `# [会议标题]` 开始输出。"#;

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub summary_time: String,
    pub idle_threshold_min: u64,
    pub markdown_dir: String,
    pub prompt_template: String,
    pub hotkey: String,
    pub autostart: bool,
    pub ai_answer_enabled: bool,
    pub poll_interval_ms: u64,
    pub theme: String,
    pub weekly_summary_day: u8,
    pub weekly_summary_time: String,
    pub weekly_prompt_template: String,
    /// 排重方式："day" = 当日排重（同一内容隔天可再入库），"all" = 全量排重
    pub dedup_scope: String,
    /// 会议纪要提示词模板（转写文本由程序拼接到其后）
    pub meeting_prompt_template: String,
    /// 会议深度解析提示词模板（作为 system 提示词整体发给模型）
    pub meeting_deep_prompt_template: String,
    /// 是否监控剪贴板（false 时停止自动收集）
    pub clipboard_enabled: bool,
    /// 录音卡同步开关（false 时不启动常驻 sidecar）
    pub pnote_enabled: bool,
    /// 录音卡 OGG 归档本地保留天数
    pub pnote_keep_days: i64,
    /// ffmpeg 路径（留空则不校验时长，转写兜底）
    pub pnote_ffmpeg_path: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            base_url: "https://api.openai.com/v1".into(),
            model: "gpt-4o-mini".into(),
            summary_time: "00:15".into(),
            idle_threshold_min: 5,
            markdown_dir: String::new(),
            prompt_template: DEFAULT_PROMPT_TEMPLATE.into(),
            hotkey: "ctrl+alt+space".into(),
            autostart: false,
            ai_answer_enabled: true,
            poll_interval_ms: 500,
            theme: "light".into(),
            weekly_summary_day: 1,
            weekly_summary_time: "00:15".into(),
            weekly_prompt_template: DEFAULT_WEEKLY_PROMPT_TEMPLATE.into(),
            dedup_scope: "day".into(),
            meeting_prompt_template: DEFAULT_MEETING_PROMPT_TEMPLATE.into(),
            meeting_deep_prompt_template: DEFAULT_MEETING_DEEP_PROMPT_TEMPLATE.into(),
            clipboard_enabled: true,
            pnote_enabled: false,
            pnote_keep_days: 30,
            pnote_ffmpeg_path: String::new(),
        }
    }
}

impl Settings {
    pub fn resolve_markdown_dir(&self) -> PathBuf {
        let dir = self.markdown_dir.trim();
        if !dir.is_empty() {
            PathBuf::from(dir)
        } else {
            dirs::document_dir()
                .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
                .join("Masaifu")
                .join("daily")
        }
    }

    /// 每周汇总输出目录：自定义目录时与每日同目录（文件名不冲突），默认用 Masaifu\weekly。
    pub fn resolve_weekly_markdown_dir(&self) -> PathBuf {
        let dir = self.markdown_dir.trim();
        if !dir.is_empty() {
            PathBuf::from(dir)
        } else {
            dirs::document_dir()
                .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
                .join("Masaifu")
                .join("weekly")
        }
    }

    pub fn api_key_present(&self) -> bool {
        !self.api_key.trim().is_empty()
    }

    pub fn load_or_default(path: &Path) -> (Settings, bool) {
        match fs::read_to_string(path) {
            Ok(s) => match serde_json::from_str::<Settings>(&s) {
                Ok(st) => (st, false),
                Err(e) => {
                    log::warn!("settings 解析失败: {e}，使用默认值");
                    (Settings::default(), true)
                }
            },
            Err(_) => (Settings::default(), true),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension("json.tmp");
        let data = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        fs::write(&tmp, data).map_err(|e| e.to_string())?;
        fs::rename(&tmp, path).map_err(|e| e.to_string())
    }
}
