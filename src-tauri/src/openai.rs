use crate::db::{clip, EntryRow, UnprocessedRow};
use serde_json::Value;
use std::collections::HashMap;

pub struct OpenAiClient {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
}

impl OpenAiClient {
    pub fn new(base_url: &str, api_key: &str, model: &str) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(180))
            .build()
            .unwrap_or_default();
        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            model: model.to_string(),
        }
    }

    pub async fn chat(&self, system: &str, user: &str) -> Result<String, String> {
        let url = format!("{}/chat/completions", self.base_url);
        let body = serde_json::json!({
            "model": self.model,
            "temperature": 0.3,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user}
            ]
        });
        let mut last_err = "未知错误".to_string();
        for attempt in 0..3 {
            match self.client.post(&url).bearer_auth(&self.api_key).json(&body).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        let v: Value = resp.json().await.map_err(|e| format!("响应解析失败: {e}"))?;
                        if let Some(content) = v["choices"][0]["message"]["content"].as_str() {
                            return Ok(content.to_string());
                        }
                        return Err("响应中没有 content 字段".into());
                    }
                    let text = resp.text().await.unwrap_or_default();
                    last_err = format!("HTTP {status}: {}", clip(&text, 300));
                    if status.as_u16() < 500 && status.as_u16() != 429 {
                        return Err(last_err);
                    }
                }
                Err(e) => {
                    last_err = format!("网络错误: {e}");
                }
            }
            let delay = 2u64 << attempt;
            tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
        }
        Err(last_err)
    }

    async fn chat_json(&self, system: &str, user: &str) -> Result<Value, String> {
        let text = self.chat(system, user).await?;
        let extracted = extract_json(&text);
        serde_json::from_str(extracted)
            .map_err(|e| format!("JSON 解析失败: {e}，原文：{}", clip(&text, 300)))
    }

    // 直接搜索未命中时，从用户输入中提取更可能命中的搜索词（1-4 个）
    pub async fn search_terms(&self, query: &str) -> Result<Vec<String>, String> {
        let system = "你是搜索词提取助手。用户在个人剪贴板/笔记库中搜索，直接搜索没有命中。请从用户输入中提取 1-4 个更可能命中的搜索词（关键词或短语，可包含同义词、近义词、相关概念、中英文互译）。只返回 JSON 对象，形如 {\"terms\":[\"词1\",\"词2\"]}，不要输出其他内容。";
        let out = self.chat_json(system, query).await?;
        let mut terms: Vec<String> = Vec::new();
        if let Some(arr) = out["terms"].as_array() {
            for t in arr {
                if let Some(s) = t.as_str() {
                    let s = s.trim().to_string();
                    if !s.is_empty() && !terms.contains(&s) {
                        terms.push(s);
                    }
                }
            }
        }
        terms.truncate(4);
        if terms.is_empty() {
            return Err("AI 未能提取出有效搜索词".into());
        }
        Ok(terms)
    }

    pub async fn preprocess_batch(&self, rows: &[UnprocessedRow]) -> Result<Vec<ProcessedRow>, String> {
        let items: Vec<Value> = rows
            .iter()
            .map(|r| serde_json::json!({"id": r.id, "text": clip(&r.raw_content, 3000)}))
            .collect();
        let system = "你是「马赛福」的剪贴板内容整理助手。对每条内容：1) 清理正文（去除多余空白、格式残留、复制自网页的杂乱文本，保留信息）；2) 提取 20 字以内的标题；3) 从固定分类中选一个：灵感、代码、文章、链接、待办、其他；4) 给出 1-3 个中文标签。只返回 JSON 对象，形如 {\"items\":[{\"id\":1,\"clean\":\"...\",\"title\":\"...\",\"category\":\"灵感\",\"tags\":[\"a\",\"b\"]}]}，不要输出其他内容。";
        let user = serde_json::to_string(&items).map_err(|e| e.to_string())?;
        let out = self.chat_json(system, &user).await?;
        let mut result: Vec<ProcessedRow> = Vec::new();
        if let Some(arr) = out["items"].as_array() {
            for item in arr {
                if let Some(id) = item["id"].as_i64() {
                    let original = rows.iter().find(|r| r.id == id).map(|r| r.raw_content.clone());
                    let clean = item["clean"].as_str().unwrap_or("").trim().to_string();
                    let title = item["title"].as_str().unwrap_or("").trim().to_string();
                    let category = item["category"].as_str().unwrap_or("其他").trim().to_string();
                    let tags: Vec<String> = item["tags"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|t| t.as_str().map(|s| s.trim().to_string()))
                                .filter(|s| !s.is_empty())
                                .collect()
                        })
                        .unwrap_or_default();
                    result.push(ProcessedRow {
                        id,
                        clean: if clean.is_empty() { original.unwrap_or_default() } else { clean },
                        title,
                        category: if category.is_empty() { "其他".into() } else { category },
                        tags,
                    });
                }
            }
        }
        Ok(result)
    }

    /// 根据会议转写文本生成结构化纪要，返回可复用的 ProcessedRow（category 固定为「会议」）。
    /// template 为用户可配置的纪要要求；转写文本过长时保留头尾截断。
    pub async fn meeting_minutes(
        &self,
        transcript: &str,
        template: &str,
    ) -> Result<ProcessedRow, String> {
        let truncated = truncate_middle(transcript, 15000);
        let system = "你是「马赛福」的会议纪要助手。根据用户给出的纪要要求和会议转写文本生成会议纪要。只返回 JSON 对象，形如 {\"minutes\":\"...\",\"title\":\"...\",\"tags\":[\"a\",\"b\"]}，不要输出其他内容。其中 minutes 为 Markdown 格式的会议纪要正文，title 为 20 字以内的会议主题，tags 为 1-3 个中文标签。";
        let user = format!("纪要要求：\n{template}\n\n会议转写文本：\n{truncated}");
        let out = self.chat_json(system, &user).await?;
        let minutes = out["minutes"].as_str().unwrap_or("").trim().to_string();
        if minutes.is_empty() {
            return Err("AI 未返回纪要内容".into());
        }
        let title = out["title"].as_str().unwrap_or("").trim().to_string();
        let tags: Vec<String> = out["tags"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|t| t.as_str().map(|s| s.trim().to_string()))
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        Ok(ProcessedRow {
            id: 0,
            clean: minutes,
            title,
            category: "会议".into(),
            tags,
        })
    }

    /// 会议深度解析：template 是完整的 system 提示词，转写文本作为用户消息。
    /// 直接返回 Markdown 文本（不走 JSON 结构化）。
    pub async fn meeting_deep_analysis(
        &self,
        transcript: &str,
        template: &str,
    ) -> Result<String, String> {
        let truncated = truncate_middle(transcript, 15000);
        let user = format!("会议转写文本：\n{truncated}");
        let out = self.chat(template, &user).await?;
        let out = out.trim().to_string();
        if out.is_empty() {
            return Err("AI 未返回深度解析内容".into());
        }
        Ok(out)
    }

    /// 根据已有内容（如深度解析结果）补一个 20 字以内的标题，返回纯文本。
    pub async fn generate_title(&self, content: &str) -> Result<String, String> {
        let truncated = truncate_middle(content, 8000);
        let system = "你是「马赛福」的标题助手。根据用户给出的内容提取一个 20 字以内的标题，只输出标题本身，不要引号、标点结尾或其他内容。";
        let out = self.chat(system, &truncated).await?;
        let title = out
            .trim()
            .trim_matches(|c| matches!(c, '"' | '\'' | '“' | '”'))
            .to_string();
        if title.is_empty() {
            return Err("AI 未返回标题".into());
        }
        Ok(title)
    }

    pub async fn entry_digests(&self, entries: &[EntryRow]) -> Result<HashMap<i64, String>, String> {
        let system = "为每条内容生成不超过 40 字的一句话摘要，保留关键信息。只返回 JSON 对象，形如 {\"1\":\"摘要\"}，不要输出其他内容。";
        let mut map = HashMap::new();
        for chunk in entries.chunks(25) {
            let items: Vec<Value> = chunk
                .iter()
                .map(|e| {
                    let title = e
                        .title
                        .clone()
                        .filter(|t| !t.is_empty())
                        .unwrap_or_else(|| clip(&e.raw_content, 30));
                    let content = e
                        .clean_content
                        .clone()
                        .filter(|c| !c.is_empty())
                        .unwrap_or_else(|| e.raw_content.clone());
                    serde_json::json!({
                        "id": e.id,
                        "text": format!(
                            "[{}] {}：{}",
                            e.category.clone().unwrap_or_else(|| "其他".into()),
                            title,
                            clip(&content, 500)
                        )
                    })
                })
                .collect();
            let user = serde_json::to_string(&items).map_err(|e| e.to_string())?;
            let out = self.chat_json(system, &user).await?;
            if let Some(obj) = out.as_object() {
                for (k, v) in obj {
                    if let (Ok(id), Some(s)) = (k.parse::<i64>(), v.as_str()) {
                        map.insert(id, s.to_string());
                    }
                }
            }
        }
        Ok(map)
    }
}

#[derive(Debug, Clone)]
pub struct ProcessedRow {
    pub id: i64,
    pub clean: String,
    pub title: String,
    pub category: String,
    pub tags: Vec<String>,
}

impl ProcessedRow {
    pub fn tags_json(&self) -> String {
        serde_json::to_string(&self.tags).unwrap_or_else(|_| "[]".into())
    }
}

/// 长文本保留头尾截断（转写全文可能超出模型上下文）。
fn truncate_middle(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let head: String = text.chars().take(max_chars / 2).collect();
    let tail: String = text
        .chars()
        .rev()
        .take(max_chars / 2)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{head}\n……（中间部分省略）……\n{tail}")
}

fn trim_json(s: &str) -> &str {
    let s = s.trim();
    if s.starts_with("```") {
        if let Some(start) = s.find('\n') {
            let rest = &s[start + 1..];
            if let Some(end) = rest.rfind("```") {
                return rest[..end].trim();
            }
        }
    }
    s
}

/// 从模型输出中提取第一个完整的 JSON 对象（按括号配对扫描，忽略字符串内的括号）。
/// 模型有时会在 JSON 前后输出多余文本，直接整体解析会报 trailing characters。
fn extract_json(s: &str) -> &str {
    let s = trim_json(s);
    let start = match s.find('{') {
        Some(i) => i,
        None => return s,
    };
    let bytes = s.as_bytes();
    let mut depth = 0;
    let mut in_str = false;
    let mut escape = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if in_str {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_str = false;
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return &s[start..=i];
                }
            }
            _ => {}
        }
    }
    // 没找到完整对象，原样返回，让 serde 给出原始解析错误
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_plain_object() {
        assert_eq!(extract_json("{\"1\":\"a\"}"), "{\"1\":\"a\"}");
    }

    #[test]
    fn extract_with_trailing_text() {
        assert_eq!(
            extract_json("{\"1\":\"a\"}\n以上是摘要。"),
            "{\"1\":\"a\"}"
        );
    }

    #[test]
    fn extract_with_fence_and_trailing() {
        assert_eq!(
            extract_json("```json\n{\"1\":\"a\"}\n```\n补充说明"),
            "{\"1\":\"a\"}"
        );
    }

    #[test]
    fn extract_ignores_braces_in_strings() {
        assert_eq!(
            extract_json("{\"1\":\"a}b{\\\"c\\\"}\"}多余"),
            "{\"1\":\"a}b{\\\"c\\\"}\"}"
        );
    }

    #[test]
    fn extract_without_object_returns_input() {
        assert_eq!(extract_json("没有 json"), "没有 json");
    }
}
