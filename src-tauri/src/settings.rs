use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const DEFAULT_PROMPT_TEMPLATE: &str = "你是「马赛福」的每日汇总助手。请根据下面提供的当日条目，写一份 Markdown 汇总。\n\n要求：\n1. 顶部写一段「今日总览」（2-4 句话概括今天关注的重点）。\n2. 按固定分类分节（灵感、代码、文章、链接、待办、其他），每节下列出条目：每条格式：**标题**（标签：`#标签`）——一句话摘要；空分类不出现。\n3. 最后写「值得回看」，挑 1-3 条最值得回看的条目并说明原因。\n4. 使用中文，不要编造条目里没有的内容。\n\n日期：{date}\n当日条目：\n{entries}";

pub const DEFAULT_WEEKLY_PROMPT_TEMPLATE: &str = "你是「马赛福」的每周汇总助手。请根据下面提供的本周条目，写一份 Markdown 周报。\n\n要求：\n1. 顶部写一段「本周总览」（3-5 句话概括本周关注的重点与进展）。\n2. 按固定分类分节（灵感、代码、文章、链接、待办、其他），每节下列出条目：每条格式：**标题**（标签：`#标签`）——一句话摘要；空分类不出现。\n3. 最后写「下周值得关注」，挑 1-3 条需要跟进或回看的内容并说明原因。\n4. 使用中文，不要编造条目里没有的内容。\n\n周次：{week}（{range}）\n本周条目：\n{entries}";

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
