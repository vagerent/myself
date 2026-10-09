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

/// 录音卡深度解析的默认提示词（作为 system 提示词整体发给模型，录音转写作为用户消息）。
/// 个人记录导向：按时间线梳理这段录音里这个人做了什么、想到了什么、说了要做什么。
pub const DEFAULT_RECORDER_DEEP_PROMPT: &str = r#"# 角色
你是我的个人录音整理助手。我把随手录下的语音转写交给你，你帮我整理成一份属于「我」的个人记录。收到转写后立即直接输出 Markdown，绝不向我提问，绝不寒暄。

# 整理方式
按时间线梳理这段录音，讲清楚四件事：
1. **做了什么**：我在这段时间实际做了什么事、推进了什么工作。
2. **想到了什么**：我冒出的想法、灵感、判断、顾虑、纠结。
3. **说了要做什么**：我说出口的待办、承诺、约定（对谁、做什么、什么时候），逐条列出。
4. **涉及的人和数字**：提到的人名或称呼、具体数字、金额、日期、时间点。

# 输出结构（Markdown）
## 这段录音做了什么
按时间顺序分点，每点一两句话。

## 想到的事
想法、灵感、顾虑；没有则写「无」。

## 行动项
- [ ] 我要做的：……（有明确时间的标注日期）
- [ ] 别人答应我的：……（谁、答应什么）

## 人和数字
- 人：……
- 数字/时间：……

# 规则
- 只整理转写里真实存在的内容，禁止编造，禁止合理想象补全。
- 缺失但关键的信息标 `[待补充]`，不要反问。
- 过滤寒暄、客套、口头禅、语气词；保留行动和想法。
- 全程用第一人称「我」的视角叙述。
- 直接以 Markdown 正文开始，不要输出「好的」「以下是」等任何前置话语。"#;

/// 录音洞察分析的默认提示词（作为 system 提示词整体发给模型）。
/// 用户消息由程序拼装：带日期的深度解析集合 + 上次项目状态 JSON + 已有活跃发现清单。
pub const DEFAULT_RECORDER_INSIGHT_PROMPT: &str = r#"你是我的个人录音洞察分析助手。用户消息里有三部分输入：
1. 【深度解析】：若干段我的录音深度解析，每段开头有编号 [N] 和录制日期。
2. 【上次状态】：上次的项目脉络 JSON（projects + archived），你要在它基础上滚动更新。
3. 【已有活跃发现】：之前已报告、仍未关闭的发现清单，用于去重。

# 任务
从这些深度解析里提取值得我记住的发现，并维护跨天的项目脉络状态。

# findings 规则
每个发现是 JSON 对象：{"kind","title","note","quote","event_date","ref"}
- kind 只能是以下四种：
  - "commitment_mine"：我要做的事（我说过我要做什么）
  - "commitment_other"：别人答应我的事
  - "idea"：我说过的想法、灵感
  - "open_loop"：说过要做什么、之后再没被提起的未闭环事项
- title：不超过 20 字的一句话。
- note：不超过 100 字，带上下文（什么场景说的、涉及谁）。
- quote：必须从输入深度解析里逐字摘录的原文短句，让我能回听核验，不许改写。
- event_date：YYYY-MM-DD，推断不出填 null。
- ref：该发现出自的输入块编号 [N]。
- 只提取输入里明确说出的内容，禁止编造、禁止过度推断。
- 与【已有活跃发现】重复（同一件事）的不要再报。

# open_loop 规则
【上次状态】里 open_items 中 last_mention 距今超过 7 天、本次输入也未再提及、且没有闭环证据的，产出一条 kind="open_loop" 的发现（只报一次），并在 state 中把该 open_item 的 reported 置为 true。

# state 规则
state 是滚动更新的项目脉络，结构：
{"projects":[{"name":"...","thread":[{"date":"YYYY-MM-DD","note":"..."}],"open_items":[{"item":"...","origin_date":"YYYY-MM-DD","last_mention":"YYYY-MM-DD","reported":false}]}],"archived":["..."]}
- thread：该项目每次被提及追加一条（date + 一句 note），跨天滚动。
- open_items：未闭环事项台账，last_mention 更新为最近一次提及日期。
- 距今超过 60 天没有被提及的项目，从 projects 移入 archived（只保留名字）。

# 输出
只输出一个严格 JSON 对象，不要输出任何其他文字、解释或 Markdown 代码块标记：
{"findings":[...],"state":{...}}
没有新发现时 findings 输出 []，state 照常输出。"#;

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
    /// 同步窗口（天）：只同步最近 N 天内开始的录音；0 = 全部
    pub pnote_sync_window_days: i64,
    /// 录音洞察分析提示词（作为 system 提示词整体发给模型）
    pub recorder_event_prompt: String,
    /// 录音卡会议纪要提示词（转写文本拼接到其后）；留空跟随 meeting_prompt_template
    pub recorder_minutes_prompt: String,
    /// 录音卡深度解析提示词（作为 system 提示词整体发给模型）；留空使用内置个人记录导向提示词
    pub recorder_deep_prompt: String,
    /// 洞察分析间隔小时数；0=仅手动
    pub recorder_analysis_interval_hours: i64,
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
            pnote_sync_window_days: 2,
            recorder_event_prompt: DEFAULT_RECORDER_INSIGHT_PROMPT.into(),
            recorder_minutes_prompt: String::new(),
            recorder_deep_prompt: DEFAULT_RECORDER_DEEP_PROMPT.into(),
            recorder_analysis_interval_hours: 3,
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

    /// 录音卡纪要提示词：留空跟随「会议录音」的提示词
    pub fn resolve_recorder_minutes_prompt(&self) -> &str {
        let p = self.recorder_minutes_prompt.trim();
        if p.is_empty() {
            self.meeting_prompt_template.trim()
        } else {
            p
        }
    }

    /// 录音卡深度解析提示词：留空使用内置个人记录导向提示词
    pub fn resolve_recorder_deep_prompt(&self) -> &str {
        let p = self.recorder_deep_prompt.trim();
        if p.is_empty() {
            DEFAULT_RECORDER_DEEP_PROMPT
        } else {
            p
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorder_prompts_fall_back_when_empty() {
        let mut s = Settings::default();
        s.recorder_minutes_prompt = "  ".into();
        s.recorder_deep_prompt = String::new();
        assert_eq!(
            s.resolve_recorder_minutes_prompt(),
            s.meeting_prompt_template.trim()
        );
        assert_eq!(s.resolve_recorder_deep_prompt(), DEFAULT_RECORDER_DEEP_PROMPT);
    }

    #[test]
    fn recorder_prompts_override_when_set() {
        let mut s = Settings::default();
        s.recorder_minutes_prompt = "  录音专用纪要  ".into();
        s.recorder_deep_prompt = "录音专用深度".into();
        assert_eq!(s.resolve_recorder_minutes_prompt(), "录音专用纪要");
        assert_eq!(s.resolve_recorder_deep_prompt(), "录音专用深度");
    }

    /// 旧 settings.json 没有新字段时必须能反序列化（struct 级 default 兜底）
    #[test]
    fn settings_deserializes_without_recorder_prompt_fields() {
        let s = Settings::default();
        let mut v = serde_json::to_value(&s).unwrap();
        let obj = v.as_object_mut().unwrap();
        obj.remove("recorderMinutesPrompt");
        obj.remove("recorderDeepPrompt");
        obj.remove("recorderAnalysisIntervalHours");
        let parsed: Settings = serde_json::from_value(v).unwrap();
        assert_eq!(parsed.recorder_minutes_prompt, "");
        assert_eq!(parsed.resolve_recorder_deep_prompt(), DEFAULT_RECORDER_DEEP_PROMPT);
        assert_eq!(parsed.recorder_analysis_interval_hours, 3);
        assert_eq!(
            parsed.resolve_recorder_minutes_prompt(),
            parsed.meeting_prompt_template.trim()
        );
    }

    /// recorder_event_prompt 复用为录音洞察分析提示词，默认值必须非空
    #[test]
    fn recorder_event_prompt_default_is_insight_prompt() {
        let s = Settings::default();
        assert_eq!(s.recorder_event_prompt, DEFAULT_RECORDER_INSIGHT_PROMPT);
        assert!(!s.recorder_event_prompt.trim().is_empty());
    }
}
