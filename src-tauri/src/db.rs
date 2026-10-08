use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};

pub type Db = Arc<Mutex<Connection>>;

pub const CATEGORIES: [&str; 7] = ["灵感", "代码", "文章", "链接", "待办", "会议", "其他"];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryRow {
    pub id: i64,
    pub raw_content: String,
    pub clean_content: Option<String>,
    /// 会议深度解析结果（AI 生成，Markdown），仅会议类条目使用
    pub deep_analysis: Option<String>,
    pub title: Option<String>,
    pub category: Option<String>,
    pub tags: Option<String>,
    pub source: String,
    pub processed: bool,
    pub created_at: String,
    pub processed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryRow {
    pub date: String,
    pub content: String,
    pub markdown_path: String,
    pub status: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub doc_type: String,
    pub doc_id: String,
    pub title: String,
    pub content: String,
    pub category: String,
    pub tags: String,
    pub source: String,
    pub created_at: String,
    pub rank: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CountRow {
    pub key: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TreeStats {
    pub total_entries: i64,
    pub by_category: Vec<CountRow>,
    pub by_source: Vec<CountRow>,
    pub summary_dates: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReminderRow {
    pub id: i64,
    pub title: String,
    pub note: String,
    pub kind: String,
    pub time_of_day: String,
    pub weekday: Option<i64>,
    pub fire_date: Option<String>,
    pub next_fire_at: String,
    pub enabled: bool,
    pub last_fired_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct UnprocessedRow {
    pub id: i64,
    pub raw_content: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecorderFileRow {
    pub name: String,
    pub size: i64,
    pub seconds: i64,
    pub status: String,
    pub ogg_path: Option<String>,
    pub entry_id: Option<i64>,
    pub created_at: String,
}

pub fn init(conn: &Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS entries (
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          raw_content TEXT NOT NULL,
          content_hash TEXT NOT NULL UNIQUE,
          clean_content TEXT,
          title TEXT,
          category TEXT,
          tags TEXT,
          source TEXT NOT NULL DEFAULT 'clipboard',
          processed INTEGER NOT NULL DEFAULT 0,
          created_at TEXT NOT NULL,
          processed_at TEXT,
          deep_analysis TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_entries_created ON entries(created_at);
        CREATE INDEX IF NOT EXISTS idx_entries_processed ON entries(processed);
        CREATE INDEX IF NOT EXISTS idx_entries_source ON entries(source);

        CREATE TABLE IF NOT EXISTS summaries (
          date TEXT PRIMARY KEY,
          content TEXT NOT NULL,
          markdown_path TEXT,
          status TEXT NOT NULL DEFAULT 'done',
          created_at TEXT NOT NULL
        );

        CREATE VIRTUAL TABLE IF NOT EXISTS entries_fts USING fts5(
          doc_type,
          doc_id UNINDEXED,
          raw_content,
          clean_content,
          title,
          category,
          tags,
          summary_text,
          tokenize = 'trigram'
        );

        CREATE TABLE IF NOT EXISTS reminders (
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          title TEXT NOT NULL,
          note TEXT NOT NULL DEFAULT '',
          kind TEXT NOT NULL,
          time_of_day TEXT NOT NULL,
          weekday INTEGER,
          fire_date TEXT,
          next_fire_at TEXT NOT NULL,
          enabled INTEGER NOT NULL DEFAULT 1,
          last_fired_at TEXT,
          created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_reminders_next_fire ON reminders(enabled, next_fire_at);

        CREATE TABLE IF NOT EXISTS recorder_files (
          name TEXT PRIMARY KEY,
          size INTEGER NOT NULL,
          seconds INTEGER NOT NULL DEFAULT 0,
          status TEXT NOT NULL DEFAULT 'listed',
          ogg_path TEXT,
          entry_id INTEGER,
          created_at TEXT NOT NULL
        );
        "#,
    )?;
    ensure_column(conn, "entries", "deep_analysis", "TEXT")?;
    Ok(())
}

/// 老库缺列时补列（CREATE TABLE IF NOT EXISTS 不会修改已有表）。
fn ensure_column(
    conn: &Connection,
    table: &str,
    column: &str,
    decl: &str,
) -> rusqlite::Result<()> {
    let exists = conn
        .prepare(&format!("PRAGMA table_info({table})"))?
        .query_map([], |r| r.get::<_, String>(1))?
        .any(|name| name.as_deref() == Ok(column));
    if !exists {
        conn.execute(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl}"), [])?;
    }
    Ok(())
}

pub fn sha256(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn now_str() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

// 当天收集的条目数量（按本地日期匹配 created_at 前缀）
pub fn count_entries_today(conn: &Connection) -> rusqlite::Result<i64> {
    let today = now_str()[..10].to_string();
    conn.query_row(
        "SELECT COUNT(*) FROM entries WHERE substr(created_at, 1, 10) = ?1",
        params![today],
        |r| r.get(0),
    )
}

fn fts_replace(
    conn: &Connection,
    doc_type: &str,
    doc_id: &str,
    raw: &str,
    clean: &str,
    title: &str,
    category: &str,
    tags: &str,
    summary_text: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM entries_fts WHERE doc_type = ?1 AND doc_id = ?2",
        params![doc_type, doc_id],
    )?;
    conn.execute(
        "INSERT INTO entries_fts (doc_type, doc_id, raw_content, clean_content, title, category, tags, summary_text)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![doc_type, doc_id, raw, clean, title, category, tags, summary_text],
    )?;
    Ok(())
}

/// 入库；dedup_daily 为 true 时按当天去重（同一内容隔天可再次记录），否则全量去重。
/// 内容已存在（按所选排重范围）时返回 None。
pub fn insert_entry(
    conn: &Connection,
    content: &str,
    source: &str,
    dedup_daily: bool,
) -> rusqlite::Result<Option<i64>> {
    let ts = now_str();
    // 当日排重：哈希混入日期，去重范围限定为当天
    let hash = if dedup_daily {
        sha256(&format!("{}|{}", &ts[..10], content))
    } else {
        sha256(content)
    };
    let n = conn.execute(
        "INSERT OR IGNORE INTO entries (raw_content, content_hash, source, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![content, hash, source, ts],
    )?;
    if n == 0 {
        return Ok(None);
    }
    let id = conn.last_insert_rowid();
    fts_replace(conn, "entry", &id.to_string(), content, "", "", "", "", "")?;
    Ok(Some(id))
}

pub fn unprocessed_batch(conn: &Connection, limit: usize) -> rusqlite::Result<Vec<UnprocessedRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, raw_content FROM entries WHERE processed = 0 AND source != 'meeting' ORDER BY created_at ASC LIMIT ?1",
    )?;
    let rows = stmt
        .query_map(params![limit as i64], |r| {
            Ok(UnprocessedRow {
                id: r.get(0)?,
                raw_content: r.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn unprocessed_for_date(
    conn: &Connection,
    date: &str,
    limit: usize,
) -> rusqlite::Result<Vec<UnprocessedRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, raw_content FROM entries WHERE processed = 0 AND source != 'meeting' AND created_at LIKE ?1 ORDER BY created_at ASC LIMIT ?2",
    )?;
    let like = format!("{date}%");
    let rows = stmt
        .query_map(params![like, limit as i64], |r| {
            Ok(UnprocessedRow {
                id: r.get(0)?,
                raw_content: r.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn mark_processed(
    conn: &Connection,
    id: i64,
    clean: &str,
    title: &str,
    category: &str,
    tags_json: &str,
) -> rusqlite::Result<()> {
    let raw: Option<String> = conn
        .query_row("SELECT raw_content FROM entries WHERE id = ?1", params![id], |r| r.get(0))
        .optional()?;
    let raw = raw.unwrap_or_default();
    conn.execute(
        "UPDATE entries SET clean_content = ?1, title = ?2, category = ?3, tags = ?4, processed = 1, processed_at = ?5 WHERE id = ?6",
        params![clean, title, category, tags_json, now_str(), id],
    )?;
    fts_replace(conn, "entry", &id.to_string(), &raw, clean, title, category, tags_json, "")?;
    Ok(())
}

pub fn update_entry_meta(
    conn: &Connection,
    id: i64,
    title: &str,
    category: &str,
    tags_json: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE entries SET title = ?1, category = ?2, tags = ?3 WHERE id = ?4",
        params![title, category, tags_json, id],
    )?;
    let (raw, clean, tags): (String, String, String) = conn.query_row(
        "SELECT COALESCE(raw_content,''), COALESCE(clean_content,''), COALESCE(tags,'') FROM entries WHERE id = ?1",
        params![id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    fts_replace(conn, "entry", &id.to_string(), &raw, &clean, title, category, &tags, "")?;
    Ok(())
}

fn row_to_entry(r: &rusqlite::Row<'_>) -> rusqlite::Result<EntryRow> {
    Ok(EntryRow {
        id: r.get(0)?,
        raw_content: r.get(1)?,
        clean_content: r.get(2)?,
        deep_analysis: r.get(3)?,
        title: r.get(4)?,
        category: r.get(5)?,
        tags: r.get(6)?,
        source: r.get(7)?,
        processed: r.get::<_, i64>(8)? != 0,
        created_at: r.get(9)?,
        processed_at: r.get(10)?,
    })
}

const ENTRY_COLS: &str = "id, raw_content, clean_content, deep_analysis, title, category, tags, source, processed, created_at, processed_at";

/// 保存条目的会议深度解析结果。
pub fn save_deep_analysis(conn: &Connection, id: i64, analysis: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE entries SET deep_analysis = ?1 WHERE id = ?2",
        params![analysis, id],
    )?;
    Ok(())
}

/// 仅更新条目标题（深度解析后为空标题补题用，不影响其他字段）。
pub fn update_title(conn: &Connection, id: i64, title: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE entries SET title = ?1 WHERE id = ?2",
        params![title, id],
    )?;
    Ok(())
}

pub fn get_entry(conn: &Connection, id: i64) -> rusqlite::Result<Option<EntryRow>> {
    conn.query_row(
        &format!("SELECT {ENTRY_COLS} FROM entries WHERE id = ?1"),
        params![id],
        row_to_entry,
    )
    .optional()
}

pub fn entries_for_date(conn: &Connection, date: &str) -> rusqlite::Result<Vec<EntryRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {ENTRY_COLS} FROM entries WHERE created_at LIKE ?1 ORDER BY created_at ASC"
    ))?;
    let like = format!("{date}%");
    let rows = stmt
        .query_map(params![like], row_to_entry)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// 按日期范围取条目（闭区间，YYYY-MM-DD）。
pub fn entries_for_range(
    conn: &Connection,
    start_date: &str,
    end_date: &str,
) -> rusqlite::Result<Vec<EntryRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {ENTRY_COLS} FROM entries WHERE created_at >= ?1 AND created_at <= ?2 ORDER BY created_at ASC"
    ))?;
    let rows = stmt
        .query_map(
            params![
                format!("{start_date} 00:00:00"),
                format!("{end_date} 23:59:59")
            ],
            row_to_entry,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// 最近 N 天内录音卡来源（source='recorder'）的条目，按时间升序。
pub fn recorder_entries_since(conn: &Connection, days: i64) -> rusqlite::Result<Vec<EntryRow>> {
    use chrono::{Duration, Local};
    let since = (Local::now() - Duration::days(days))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let mut stmt = conn.prepare(&format!(
        "SELECT {ENTRY_COLS} FROM entries WHERE source = 'recorder' AND created_at >= ?1 ORDER BY created_at ASC"
    ))?;
    let rows = stmt
        .query_map(params![since], row_to_entry)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// 范围内未预处理条目。
pub fn unprocessed_for_range(
    conn: &Connection,
    start_date: &str,
    end_date: &str,
    limit: usize,
) -> rusqlite::Result<Vec<UnprocessedRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, raw_content FROM entries WHERE processed = 0 AND source != 'meeting' AND created_at >= ?1 AND created_at <= ?2 ORDER BY created_at ASC LIMIT ?3",
    )?;
    let rows = stmt
        .query_map(
            params![
                format!("{start_date} 00:00:00"),
                format!("{end_date} 23:59:59"),
                limit as i64
            ],
            |r| {
                Ok(UnprocessedRow {
                    id: r.get(0)?,
                    raw_content: r.get(1)?,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn insert_summary(
    conn: &Connection,
    date: &str,
    content: &str,
    markdown_path: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO summaries (date, content, markdown_path, status, created_at) VALUES (?1, ?2, ?3, 'done', ?4)",
        params![date, content, markdown_path, now_str()],
    )?;
    fts_replace(conn, "summary", date, "", "", "", "", "", content)?;
    Ok(())
}

pub fn get_summary(conn: &Connection, date: &str) -> rusqlite::Result<Option<SummaryRow>> {
    conn.query_row(
        "SELECT date, content, markdown_path, status, created_at FROM summaries WHERE date = ?1",
        params![date],
        |r| {
            Ok(SummaryRow {
                date: r.get(0)?,
                content: r.get(1)?,
                markdown_path: r.get(2)?,
                status: r.get(3)?,
                created_at: r.get(4)?,
            })
        },
    )
    .optional()
}

pub fn list_summaries(conn: &Connection) -> rusqlite::Result<Vec<SummaryRow>> {
    let mut stmt = conn.prepare(
        "SELECT date, content, markdown_path, status, created_at FROM summaries ORDER BY date DESC",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(SummaryRow {
                date: r.get(0)?,
                content: r.get(1)?,
                markdown_path: r.get(2)?,
                status: r.get(3)?,
                created_at: r.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn tree_stats(conn: &Connection) -> rusqlite::Result<TreeStats> {
    let total_entries: i64 = conn.query_row("SELECT COUNT(*) FROM entries", [], |r| r.get(0))?;

    let mut by_category = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT COALESCE(category, '') AS c, COUNT(*) AS cnt FROM entries GROUP BY c ORDER BY cnt DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(CountRow {
                key: r.get(0)?,
                count: r.get(1)?,
            })
        })?;
        for r in rows {
            by_category.push(r?);
        }
    }

    let mut by_source = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT source, COUNT(*) AS cnt FROM entries GROUP BY source ORDER BY cnt DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(CountRow {
                key: r.get(0)?,
                count: r.get(1)?,
            })
        })?;
        for r in rows {
            by_source.push(r?);
        }
    }

    let mut summary_dates = Vec::new();
    {
        let mut stmt = conn.prepare("SELECT date FROM summaries ORDER BY date DESC LIMIT 60")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        for r in rows {
            summary_dates.push(r?);
        }
    }

    Ok(TreeStats {
        total_entries,
        by_category,
        by_source,
        summary_dates,
    })
}

#[derive(Serialize)]
pub struct EntryPage {
    pub entries: Vec<EntryRow>,
    pub total: usize,
}

pub fn list_entries_filtered(
    conn: &Connection,
    category: Option<&str>,
    source: Option<&str>,
    query: Option<&str>,
    limit: usize,
    offset: usize,
) -> rusqlite::Result<EntryPage> {
    let mut where_sql = String::from(" WHERE 1 = 1");
    let mut params: Vec<String> = Vec::new();
    if let Some(c) = category {
        where_sql.push_str(" AND category = ?");
        params.push(c.to_string());
    }
    if let Some(s) = source {
        where_sql.push_str(" AND source = ?");
        params.push(s.to_string());
    }
    if let Some(q) = query {
        let q = q.trim();
        if !q.is_empty() {
            let pattern = format!("%{}%", escape_like(q));
            where_sql.push_str(
                " AND (raw_content LIKE ? ESCAPE '\\' OR clean_content LIKE ? ESCAPE '\\' \
                 OR title LIKE ? ESCAPE '\\' OR tags LIKE ? ESCAPE '\\')",
            );
            for _ in 0..4 {
                params.push(pattern.clone());
            }
        }
    }

    let total: usize = conn.query_row(
        &format!("SELECT COUNT(*) FROM entries{where_sql}"),
        rusqlite::params_from_iter(params.iter()),
        |r| r.get(0),
    )?;

    let sql = format!(
        "SELECT {ENTRY_COLS} FROM entries{where_sql} ORDER BY created_at DESC LIMIT ? OFFSET ?"
    );
    let mut page_params = params.clone();
    page_params.push(limit.to_string());
    page_params.push(offset.to_string());

    let mut stmt = conn.prepare(&sql)?;
    let entries = stmt
        .query_map(rusqlite::params_from_iter(page_params.iter()), row_to_entry)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(EntryPage { entries, total })
}

fn escape_like(q: &str) -> String {
    q.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn entry_hit(e: &EntryRow, rank: f64) -> SearchHit {
    let clean = e.clean_content.clone().unwrap_or_default();
    let content = if clean.is_empty() {
        e.raw_content.clone()
    } else {
        clean
    };
    let title = e
        .title
        .clone()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| clip(&e.raw_content, 40));
    SearchHit {
        doc_type: "entry".into(),
        doc_id: e.id.to_string(),
        title,
        content,
        category: e.category.clone().unwrap_or_default(),
        tags: e.tags.clone().unwrap_or_default(),
        source: e.source.clone(),
        created_at: e.created_at.clone(),
        rank,
    }
}

/// 汇总主键是否为 ISO 周标识（如 2026-W33）。
pub fn is_week_key(key: &str) -> bool {
    let bytes = key.as_bytes();
    bytes.len() == 8
        && bytes[4] == b'-'
        && bytes[5] == b'W'
        && bytes[..4].iter().all(|b| b.is_ascii_digit())
        && bytes[6..].iter().all(|b| b.is_ascii_digit())
}

pub fn summary_title(date: &str) -> String {
    if is_week_key(date) {
        format!("每周汇总 {date}")
    } else {
        format!("每日汇总 {date}")
    }
}

fn summary_hit(s: &SummaryRow, rank: f64) -> SearchHit {
    SearchHit {
        doc_type: "summary".into(),
        doc_id: s.date.clone(),
        title: summary_title(&s.date),
        content: s.content.clone(),
        category: String::new(),
        tags: String::new(),
        source: "summary".into(),
        created_at: format!("{} 00:00:00", s.date),
        rank,
    }
}

pub fn build_fts_query(q: &str) -> Option<String> {
    let tokens: Vec<String> = q
        .split_whitespace()
        .filter(|t| t.chars().count() >= 3)
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect();
    if tokens.is_empty() {
        None
    } else {
        Some(tokens.join(" AND "))
    }
}

fn search_fts(conn: &Connection, fts_query: &str, limit: usize) -> rusqlite::Result<Vec<SearchHit>> {
    let mut stmt = conn.prepare(
        "SELECT doc_type, CAST(doc_id AS TEXT), raw_content, clean_content, title, category, tags, summary_text, rank
         FROM entries_fts WHERE entries_fts MATCH ?1 ORDER BY rank LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![fts_query, limit as i64], |r| {
            let doc_type: String = r.get(0)?;
            let doc_id: String = r.get(1)?;
            let raw: String = r.get(2)?;
            let clean: String = r.get(3)?;
            let title: String = r.get(4)?;
            let category: String = r.get(5)?;
            let tags: String = r.get(6)?;
            let summary_text: String = r.get(7)?;
            let rank: f64 = r.get(8)?;
            let content = if summary_text.is_empty() {
                if clean.is_empty() {
                    raw
                } else {
                    clean
                }
            } else {
                summary_text
            };
            let title = if doc_type == "summary" {
                summary_title(&doc_id)
            } else if title.is_empty() {
                clip(&content, 40)
            } else {
                title
            };
            Ok(SearchHit {
                doc_type,
                doc_id,
                title,
                content,
                category,
                tags,
                source: String::new(),
                created_at: String::new(),
                rank,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn search_like(conn: &Connection, q: &str, limit: usize) -> rusqlite::Result<Vec<SearchHit>> {
    let pattern = format!("%{}%", escape_like(q));
    let mut hits = Vec::new();
    let mut stmt = conn.prepare(&format!(
        "SELECT {ENTRY_COLS} FROM entries
         WHERE raw_content LIKE ?1 ESCAPE '\\' OR clean_content LIKE ?1 ESCAPE '\\'
            OR title LIKE ?1 ESCAPE '\\' OR category LIKE ?1 ESCAPE '\\' OR tags LIKE ?1 ESCAPE '\\'
         ORDER BY created_at DESC LIMIT ?2"
    ))?;
    let entries = stmt
        .query_map(params![pattern, limit as i64], row_to_entry)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for e in entries {
        hits.push(entry_hit(&e, 0.0));
    }
    if hits.len() < limit {
        let remain = (limit - hits.len()) as i64;
        let mut stmt2 = conn.prepare(
            "SELECT date, content, markdown_path, status, created_at FROM summaries
             WHERE content LIKE ?1 ESCAPE '\\' OR date LIKE ?1 ESCAPE '\\'
             ORDER BY date DESC LIMIT ?2",
        )?;
        let summaries = stmt2
            .query_map(params![pattern, remain], |r| {
                Ok(SummaryRow {
                    date: r.get(0)?,
                    content: r.get(1)?,
                    markdown_path: r.get(2)?,
                    status: r.get(3)?,
                    created_at: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for s in summaries {
            hits.push(summary_hit(&s, 0.0));
        }
    }
    Ok(hits)
}

pub fn recent(conn: &Connection, limit: usize) -> rusqlite::Result<Vec<SearchHit>> {
    let mut hits = Vec::new();
    let mut stmt = conn.prepare(&format!(
        "SELECT {ENTRY_COLS} FROM entries ORDER BY created_at DESC LIMIT ?1"
    ))?;
    let entries = stmt
        .query_map(params![limit as i64], row_to_entry)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for e in entries {
        hits.push(entry_hit(&e, 0.0));
    }
    let mut stmt2 = conn.prepare(
        "SELECT date, content, markdown_path, status, created_at FROM summaries ORDER BY date DESC LIMIT 5",
    )?;
    let summaries = stmt2
        .query_map([], |r| {
            Ok(SummaryRow {
                date: r.get(0)?,
                content: r.get(1)?,
                markdown_path: r.get(2)?,
                status: r.get(3)?,
                created_at: r.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for s in summaries {
        hits.push(summary_hit(&s, 0.0));
    }
    Ok(hits)
}

pub fn search(conn: &Connection, query: &str, limit: usize) -> rusqlite::Result<Vec<SearchHit>> {
    let q = query.trim();
    if q.is_empty() {
        return recent(conn, limit);
    }
    let mut hits = Vec::new();
    if let Some(fq) = build_fts_query(q) {
        let fts = search_fts(conn, &fq, limit)?;
        for mut h in fts {
            if h.doc_type == "entry" {
                if let Ok(Some(e)) = get_entry(conn, h.doc_id.parse().unwrap_or(0)) {
                    h.source = e.source.clone();
                    h.created_at = e.created_at.clone();
                    h.category = e.category.clone().unwrap_or_default();
                    h.tags = e.tags.clone().unwrap_or_default();
                }
            } else {
                h.source = "summary".into();
                h.created_at = format!("{} 00:00:00", h.doc_id);
            }
            hits.push(h);
        }
    }
    if hits.len() < limit {
        let remain = limit.saturating_sub(hits.len());
        let more = search_like(conn, q, remain)?;
        let seen = hits
            .iter()
            .map(|h| (h.doc_type.clone(), h.doc_id.clone()))
            .collect::<std::collections::HashSet<_>>();
        for h in more {
            if !seen.contains(&(h.doc_type.clone(), h.doc_id.clone())) {
                hits.push(h);
            }
        }
    }
    Ok(hits)
}

pub fn clip(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        s.to_string()
    } else {
        let cut: String = chars[..max].iter().collect();
        format!("{cut}…")
    }
}

// ---------- 任务提醒 ----------

const REMINDER_COLS: &str =
    "id, title, note, kind, time_of_day, weekday, fire_date, next_fire_at, enabled, last_fired_at, created_at";

fn row_to_reminder(r: &rusqlite::Row<'_>) -> rusqlite::Result<ReminderRow> {
    Ok(ReminderRow {
        id: r.get(0)?,
        title: r.get(1)?,
        note: r.get(2)?,
        kind: r.get(3)?,
        time_of_day: r.get(4)?,
        weekday: r.get(5)?,
        fire_date: r.get(6)?,
        next_fire_at: r.get(7)?,
        enabled: r.get::<_, i64>(8)? != 0,
        last_fired_at: r.get(9)?,
        created_at: r.get(10)?,
    })
}

/// 计算下一次触发时间（本地时间）。weekday 约定：0=周日 .. 6=周六。
pub fn compute_next_fire(
    kind: &str,
    time_of_day: &str,
    weekday: Option<i64>,
    fire_date: Option<&str>,
) -> Option<chrono::NaiveDateTime> {
    use chrono::{Datelike, Duration, Local, NaiveDate, NaiveTime};
    let time = NaiveTime::parse_from_str(time_of_day, "%H:%M").ok()?;
    let now = Local::now().naive_local();
    match kind {
        "once" => {
            let date = NaiveDate::parse_from_str(fire_date?, "%Y-%m-%d").ok()?;
            Some(date.and_time(time))
        }
        "daily" => {
            let mut dt = now.date().and_time(time);
            if dt <= now {
                dt += Duration::days(1);
            }
            Some(dt)
        }
        "weekly" => {
            let target = weekday?.rem_euclid(7);
            let cur = now.date().weekday().num_days_from_sunday() as i64;
            let delta = (target - cur).rem_euclid(7);
            let mut dt = (now.date() + Duration::days(delta)).and_time(time);
            if dt <= now {
                dt += Duration::days(7);
            }
            Some(dt)
        }
        _ => None,
    }
}

pub fn fmt_fire_time(dt: &chrono::NaiveDateTime) -> String {
    dt.format("%Y-%m-%d %H:%M:%S").to_string()
}

#[allow(clippy::too_many_arguments)]
pub fn insert_reminder(
    conn: &Connection,
    title: &str,
    note: &str,
    kind: &str,
    time_of_day: &str,
    weekday: Option<i64>,
    fire_date: Option<&str>,
    next_fire_at: &str,
) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO reminders (title, note, kind, time_of_day, weekday, fire_date, next_fire_at, enabled, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8)",
        params![title, note, kind, time_of_day, weekday, fire_date, next_fire_at, now_str()],
    )?;
    Ok(conn.last_insert_rowid())
}

#[allow(clippy::too_many_arguments)]
pub fn update_reminder(
    conn: &Connection,
    id: i64,
    title: &str,
    note: &str,
    kind: &str,
    time_of_day: &str,
    weekday: Option<i64>,
    fire_date: Option<&str>,
    next_fire_at: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE reminders SET title = ?1, note = ?2, kind = ?3, time_of_day = ?4, weekday = ?5,
         fire_date = ?6, next_fire_at = ?7, enabled = 1 WHERE id = ?8",
        params![title, note, kind, time_of_day, weekday, fire_date, next_fire_at, id],
    )?;
    Ok(())
}

pub fn delete_reminder(conn: &Connection, id: i64) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM reminders WHERE id = ?1", params![id])?;
    Ok(())
}

/// 删除条目及其全文索引（FTS 行由 doc_type='entry' + doc_id 定位，无外键级联）
pub fn delete_entry(conn: &Connection, id: i64) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM entries_fts WHERE doc_type = 'entry' AND doc_id = ?1",
        params![id.to_string()],
    )?;
    conn.execute("DELETE FROM entries WHERE id = ?1", params![id])?;
    Ok(())
}

pub fn set_reminder_enabled(conn: &Connection, id: i64, enabled: bool) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE reminders SET enabled = ?1 WHERE id = ?2",
        params![if enabled { 1 } else { 0 }, id],
    )?;
    Ok(())
}

pub fn get_reminder(conn: &Connection, id: i64) -> rusqlite::Result<Option<ReminderRow>> {
    conn.query_row(
        &format!("SELECT {REMINDER_COLS} FROM reminders WHERE id = ?1"),
        params![id],
        row_to_reminder,
    )
    .optional()
}

pub fn list_reminders(conn: &Connection) -> rusqlite::Result<Vec<ReminderRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {REMINDER_COLS} FROM reminders ORDER BY enabled DESC, next_fire_at ASC"
    ))?;
    let rows = stmt
        .query_map([], row_to_reminder)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// 到期（含已过点）的启用中提醒。
pub fn due_reminders(conn: &Connection, now: &str) -> rusqlite::Result<Vec<ReminderRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {REMINDER_COLS} FROM reminders WHERE enabled = 1 AND next_fire_at <= ?1 ORDER BY next_fire_at ASC"
    ))?;
    let rows = stmt
        .query_map(params![now], row_to_reminder)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// 触发后回写：单次 → 停用；重复 → 推进到下次时间。
pub fn mark_reminder_fired(
    conn: &Connection,
    id: i64,
    fired_at: &str,
    next_fire_at: Option<&str>,
) -> rusqlite::Result<()> {
    match next_fire_at {
        Some(next) => conn.execute(
            "UPDATE reminders SET last_fired_at = ?1, next_fire_at = ?2 WHERE id = ?3",
            params![fired_at, next, id],
        )?,
        None => conn.execute(
            "UPDATE reminders SET last_fired_at = ?1, enabled = 0 WHERE id = ?2",
            params![fired_at, id],
        )?,
    };
    Ok(())
}

/// 错过的重复提醒：静默推进到下次时间。
pub fn advance_reminder(conn: &Connection, id: i64, next_fire_at: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE reminders SET next_fire_at = ?1 WHERE id = ?2",
        params![next_fire_at, id],
    )?;
    Ok(())
}

/// 错过的单次提醒：静默停用（last_fired_at 保持 NULL，前端据此显示「已错过」）。
pub fn mark_reminder_missed(conn: &Connection, id: i64) -> rusqlite::Result<()> {
    conn.execute("UPDATE reminders SET enabled = 0 WHERE id = ?1", params![id])?;
    Ok(())
}

// ---------- 录音卡同步 ----------

const RECORDER_FILE_COLS: &str = "name, size, seconds, status, ogg_path, entry_id, created_at";

fn row_to_recorder_file(r: &rusqlite::Row<'_>) -> rusqlite::Result<RecorderFileRow> {
    Ok(RecorderFileRow {
        name: r.get(0)?,
        size: r.get(1)?,
        seconds: r.get(2)?,
        status: r.get(3)?,
        ogg_path: r.get(4)?,
        entry_id: r.get(5)?,
        created_at: r.get(6)?,
    })
}

/// 登记设备端文件；已存在（按 name 去重）则不覆盖，避免回退同步状态。
pub fn recorder_file_upsert(
    conn: &Connection,
    name: &str,
    size: i64,
    seconds: i64,
    status: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO recorder_files (name, size, seconds, status, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(name) DO NOTHING",
        params![name, size, seconds, status, now_str()],
    )?;
    Ok(())
}

/// 更新同步状态；entry_id / ogg_path 传 None 时保留原值（分阶段回填）。
pub fn recorder_file_mark(
    conn: &Connection,
    name: &str,
    status: &str,
    entry_id: Option<i64>,
    ogg_path: Option<&str>,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE recorder_files SET status = ?2,
             entry_id = COALESCE(?3, entry_id),
             ogg_path = COALESCE(?4, ogg_path)
         WHERE name = ?1",
        params![name, status, entry_id, ogg_path],
    )?;
    Ok(())
}

pub fn recorder_file_get(
    conn: &Connection,
    name: &str,
) -> rusqlite::Result<Option<RecorderFileRow>> {
    conn.query_row(
        &format!("SELECT {RECORDER_FILE_COLS} FROM recorder_files WHERE name = ?1"),
        params![name],
        row_to_recorder_file,
    )
    .optional()
}

/// 设备 GC 候选：status='done' 且 ogg 文件 mtime 早于 days 天。
/// mtime 判断在 Rust 侧做（SQL 只取 done 行）；ogg 文件已不存在的不列入。
pub fn recorder_files_done_before(conn: &Connection, days: i64) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT name, ogg_path FROM recorder_files WHERE status = 'done'")?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(days.max(0) as u64 * 86400))
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    Ok(rows
        .into_iter()
        .filter(|(_, ogg)| {
            ogg.as_deref()
                .map(|p| {
                    std::fs::metadata(p)
                        .and_then(|m| m.modified())
                        .map(|t| t < cutoff)
                        .unwrap_or(false)
                })
                .unwrap_or(false)
        })
        .map(|(name, _)| name)
        .collect())
}

/// 未完成的同步计数（listed/downloading/failed），给前端徽标。
pub fn recorder_pending_count(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM recorder_files WHERE status IN ('listed','downloading','failed')",
        [],
        |r| r.get(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, Local};

    #[test]
    fn next_fire_once_uses_given_date() {
        let dt = compute_next_fire("once", "15:30", None, Some("2099-01-02")).unwrap();
        assert_eq!(dt.format("%Y-%m-%d %H:%M").to_string(), "2099-01-02 15:30");
    }

    #[test]
    fn insert_entry_dedupes_within_same_day() {
        let conn = Connection::open_in_memory().unwrap();
        init(&conn).unwrap();
        let first = insert_entry(&conn, "同一段内容", "clipboard", true).unwrap();
        let second = insert_entry(&conn, "同一段内容", "clipboard", true).unwrap();
        assert!(first.is_some());
        assert!(second.is_none(), "当天相同内容不应重复入库");
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM entries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn init_adds_deep_analysis_column_to_old_db() {
        let conn = Connection::open_in_memory().unwrap();
        // 模拟 v1.0 老库：entries 表没有 deep_analysis 列
        conn.execute_batch(
            "CREATE TABLE entries (
              id INTEGER PRIMARY KEY AUTOINCREMENT,
              raw_content TEXT NOT NULL,
              content_hash TEXT NOT NULL UNIQUE,
              clean_content TEXT,
              title TEXT,
              category TEXT,
              tags TEXT,
              source TEXT NOT NULL DEFAULT 'clipboard',
              processed INTEGER NOT NULL DEFAULT 0,
              created_at TEXT NOT NULL,
              processed_at TEXT
            );",
        )
        .unwrap();
        init(&conn).unwrap();
        // init 幂等：再跑一次不应报错
        init(&conn).unwrap();
        let id = insert_entry(&conn, "会议内容", "meeting", true).unwrap().unwrap();
        save_deep_analysis(&conn, id, "# 深度解析").unwrap();
        let row = get_entry(&conn, id).unwrap().unwrap();
        assert_eq!(row.deep_analysis.as_deref(), Some("# 深度解析"));
    }

    #[test]
    fn insert_entry_daily_allows_same_content_next_day() {
        let conn = Connection::open_in_memory().unwrap();
        init(&conn).unwrap();
        // 模拟昨天入过库的同内容（当日排重的哈希带昨天日期）
        let yesterday = (Local::now() - chrono::Duration::days(1)).format("%Y-%m-%d").to_string();
        let hash = sha256(&format!("{yesterday}|同一段内容"));
        conn.execute(
            "INSERT INTO entries (raw_content, content_hash, source, created_at) VALUES (?1, ?2, 'clipboard', ?3)",
            params!["同一段内容", hash, format!("{yesterday} 10:00:00")],
        )
        .unwrap();
        let again = insert_entry(&conn, "同一段内容", "clipboard", true).unwrap();
        assert!(again.is_some(), "当日排重时，隔天同内容应可再次入库");
    }

    #[test]
    fn insert_entry_global_dedupes_across_days() {
        let conn = Connection::open_in_memory().unwrap();
        init(&conn).unwrap();
        // 模拟昨天入过库的同内容（全量排重的哈希不带日期）
        let yesterday = (Local::now() - chrono::Duration::days(1)).format("%Y-%m-%d").to_string();
        let hash = sha256("同一段内容");
        conn.execute(
            "INSERT INTO entries (raw_content, content_hash, source, created_at) VALUES (?1, ?2, 'clipboard', ?3)",
            params!["同一段内容", hash, format!("{yesterday} 10:00:00")],
        )
        .unwrap();
        let again = insert_entry(&conn, "同一段内容", "clipboard", false).unwrap();
        assert!(again.is_none(), "全量排重时，隔天同内容不应再入库");
    }

    #[test]
    fn recorder_entries_since_filters_source_and_window() {
        let conn = Connection::open_in_memory().unwrap();
        init(&conn).unwrap();
        let old = (Local::now() - chrono::Duration::days(10))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        conn.execute(
            "INSERT INTO entries (raw_content, content_hash, source, created_at) VALUES (?1, ?2, 'recorder', ?3)",
            params!["十天前的录音", sha256("十天前的录音"), old],
        )
        .unwrap();
        let yesterday = (Local::now() - chrono::Duration::days(1))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        conn.execute(
            "INSERT INTO entries (raw_content, content_hash, source, created_at) VALUES (?1, ?2, 'recorder', ?3)",
            params!["昨天的录音", sha256("昨天的录音"), yesterday.clone()],
        )
        .unwrap();
        // 窗口内的非录音卡来源不应命中
        conn.execute(
            "INSERT INTO entries (raw_content, content_hash, source, created_at) VALUES (?1, ?2, 'clipboard', ?3)",
            params!["昨天的剪贴板", sha256("昨天的剪贴板"), yesterday],
        )
        .unwrap();
        let rows = recorder_entries_since(&conn, 7).unwrap();
        assert_eq!(rows.len(), 1, "只应命中窗口内的录音卡条目");
        assert_eq!(rows[0].raw_content, "昨天的录音");
        assert_eq!(rows[0].source, "recorder");
        // 再补一条今天的录音，结果应保持时间升序
        insert_entry(&conn, "今天的录音", "recorder", true).unwrap();
        let rows = recorder_entries_since(&conn, 7).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].created_at <= rows[1].created_at);
        assert_eq!(rows[1].raw_content, "今天的录音");
    }

    #[test]
    fn next_fire_daily_always_future() {
        for t in ["00:00", "12:00", "23:59"] {
            let dt = compute_next_fire("daily", t, None, None).unwrap();
            assert!(dt > Local::now().naive_local(), "daily {t} 应在未来");
        }
    }

    #[test]
    fn next_fire_weekly_lands_on_target_weekday() {
        for target in 0..7 {
            let dt = compute_next_fire("weekly", "10:00", Some(target), None).unwrap();
            assert!(dt > Local::now().naive_local());
            assert_eq!(dt.weekday().num_days_from_sunday(), target as u32);
        }
    }

    #[test]
    fn next_fire_rejects_bad_input() {
        assert!(compute_next_fire("once", "25:00", None, Some("2030-01-01")).is_none());
        assert!(compute_next_fire("once", "10:00", None, None).is_none());
        assert!(compute_next_fire("weekly", "10:00", None, None).is_none());
        assert!(compute_next_fire("nope", "10:00", None, None).is_none());
    }

    #[test]
    fn reminder_crud_roundtrip() {
        let conn = Connection::open_in_memory().unwrap();
        init(&conn).unwrap();
        let id = insert_reminder(
            &conn, "测试", "", "daily", "09:00", None, None, "2099-01-01 09:00:00",
        )
        .unwrap();
        let r = get_reminder(&conn, id).unwrap().unwrap();
        assert!(r.enabled && r.kind == "daily");
        mark_reminder_fired(&conn, id, "2026-01-01 09:00:00", Some("2026-01-02 09:00:00")).unwrap();
        let r = get_reminder(&conn, id).unwrap().unwrap();
        assert!(r.enabled && r.next_fire_at == "2026-01-02 09:00:00");
        mark_reminder_missed(&conn, id).unwrap();
        let r = get_reminder(&conn, id).unwrap().unwrap();
        assert!(!r.enabled);
        let due = due_reminders(&conn, "2099-12-31 00:00:00").unwrap();
        assert!(due.is_empty());
        delete_reminder(&conn, id).unwrap();
        assert!(get_reminder(&conn, id).unwrap().is_none());
    }

    #[test]
    fn delete_entry_removes_row_and_fts() {
        let conn = Connection::open_in_memory().unwrap();
        init(&conn).unwrap();
        let id = insert_entry(&conn, "待删除的内容", "manual", true).unwrap().unwrap();
        delete_entry(&conn, id).unwrap();
        assert!(get_entry(&conn, id).unwrap().is_none());
        let fts: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM entries_fts WHERE doc_type = 'entry' AND doc_id = ?1",
                params![id.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(fts, 0, "删除条目后其全文索引也应清除");
    }

    #[test]
    fn entries_range_query() {
        let conn = Connection::open_in_memory().unwrap();
        init(&conn).unwrap();
        for (content, ts) in [
            ("周一", "2026-08-03 10:00:00"),
            ("周日", "2026-08-09 23:59:59"),
            ("下周一", "2026-08-10 00:00:00"),
        ] {
            let hash = sha256(content);
            conn.execute(
                "INSERT INTO entries (raw_content, content_hash, source, created_at) VALUES (?1, ?2, 'manual', ?3)",
                params![content, hash, ts],
            )
            .unwrap();
        }
        let rows = entries_for_range(&conn, "2026-08-03", "2026-08-09").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].raw_content, "周一");
        assert_eq!(rows[1].raw_content, "周日");
    }

    #[test]
    fn week_key_detection() {
        assert!(is_week_key("2026-W33"));
        assert!(is_week_key("2026-W01"));
        assert!(!is_week_key("2026-08-11"));
        assert!(!is_week_key("2026-W3"));
        assert_eq!(summary_title("2026-W33"), "每周汇总 2026-W33");
        assert_eq!(summary_title("2026-08-11"), "每日汇总 2026-08-11");
    }

    #[test]
    fn recorder_file_status_roundtrip() {
        let conn = Connection::open_in_memory().unwrap();
        init(&conn).unwrap();
        recorder_file_upsert(&conn, "note001.", 55000, 27, "downloading").unwrap();
        recorder_file_mark(&conn, "note001.", "done", Some(42), Some("/a/b.ogg")).unwrap();
        let f = recorder_file_get(&conn, "note001.").unwrap().unwrap();
        assert_eq!(f.status, "done");
        assert_eq!(f.entry_id, Some(42));
        // 重复 upsert 不报错（去重键为 name）
        recorder_file_upsert(&conn, "note001.", 55000, 27, "listed").unwrap();
        assert_eq!(recorder_file_get(&conn, "note001.").unwrap().unwrap().status, "done",
            "已存在的记录不能被回退成 listed");
    }

    #[test]
    fn recorder_files_done_before_lists_only_old_ogg() {
        use std::time::{Duration, SystemTime};
        let conn = Connection::open_in_memory().unwrap();
        init(&conn).unwrap();
        let dir = std::env::temp_dir().join(format!(
            "masaifu-done-before-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        // 旧文件：mtime 拨回 40 天前
        let old_ogg = dir.join("old.ogg");
        std::fs::write(&old_ogg, b"ogg").unwrap();
        let f = std::fs::OpenOptions::new().write(true).open(&old_ogg).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(40 * 86400))
            .unwrap();
        drop(f);
        // 新文件：mtime 即现在
        let new_ogg = dir.join("new.ogg");
        std::fs::write(&new_ogg, b"ogg").unwrap();

        recorder_file_upsert(&conn, "old.", 1, 1, "done").unwrap();
        recorder_file_mark(&conn, "old.", "done", None, Some(old_ogg.to_str().unwrap())).unwrap();
        recorder_file_upsert(&conn, "new.", 1, 1, "done").unwrap();
        recorder_file_mark(&conn, "new.", "done", None, Some(new_ogg.to_str().unwrap())).unwrap();
        // ogg 已被本地清理（文件不存在）：无法判断年龄，不应列入
        recorder_file_upsert(&conn, "missing.", 1, 1, "done").unwrap();
        recorder_file_mark(
            &conn,
            "missing.",
            "done",
            None,
            Some(dir.join("gone.ogg").to_str().unwrap()),
        )
        .unwrap();
        // 非 done 状态：即使文件很旧也不列入，但计入 pending
        recorder_file_upsert(&conn, "failed.", 1, 1, "failed").unwrap();
        recorder_file_mark(&conn, "failed.", "failed", None, Some(old_ogg.to_str().unwrap()))
            .unwrap();
        assert_eq!(recorder_pending_count(&conn).unwrap(), 1);

        let names = recorder_files_done_before(&conn, 30).unwrap();
        assert_eq!(names, vec!["old.".to_string()], "只有 ogg mtime 早于 30 天的 done 行才应列入");

        std::fs::remove_dir_all(&dir).ok();
    }
}
