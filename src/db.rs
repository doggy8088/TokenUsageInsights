use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TokenStats {
    pub input: u64,
    pub output: u64,
    pub cache_read: Option<u64>,
    pub cache_write: Option<u64>,
    pub reasoning: Option<u64>,
    pub total: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ContextStats {
    pub current_context_tokens: Option<u64>,
    pub displayed_context_limit: Option<u64>,
    pub current_context_used_percentage: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CostStats {
    pub total_api_duration_ms: Option<f64>,
    pub total_duration_ms: Option<f64>,
    pub total_premium_requests: Option<f64>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct UsageEntry {
    pub timestamp: String,
    pub session_id: String,
    pub session_name: Option<String>,
    pub transcript_path: Option<String>,
    pub cwd: Option<String>,
    pub version: Option<String>,
    pub turn_no: u32,
    pub model: Option<String>,
    pub model_id: Option<String>,
    pub tokens: Option<TokenStats>,
    pub delta_tokens: Option<TokenStats>,
    pub context: Option<ContextStats>,
    pub cost: Option<CostStats>,

    // Codex-specific / Extended fields
    pub parent_session_id: Option<String>,
    pub agent_nickname: Option<String>,
    pub agent_role: Option<String>,
    pub reasoning_effort: Option<String>,
}

// Claude Code helper structs
#[derive(Debug, Clone, Default, Deserialize)]
struct ClaudeUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}

// Codex CLI helper structs
#[derive(Debug, Clone, Default, Deserialize)]
struct CodexTokenUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    cached_input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    reasoning_output_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
}

const CODEX_DELTA_PARSER_MIGRATION_KEY: &str = "migration:codex_delta_from_totals_v2";

/// Directory resolution helpers
pub fn get_insights_dir() -> PathBuf {
    if let Ok(val) = std::env::var("INSIGHTS_DIR") {
        let p = PathBuf::from(val);
        if p.exists() {
            return p;
        }
    }
    if let Some(home) = dirs::home_dir() {
        let p = home.join(".token-usage-insights");
        if !p.exists() {
            let _ = fs::create_dir_all(&p);
        }
        return p;
    }
    PathBuf::from(".")
}

pub fn get_antigravity_dir() -> PathBuf {
    if let Ok(val) = std::env::var("ANTIGRAVITY_DIR") {
        let p = PathBuf::from(val);
        if p.exists() {
            return p;
        }
    }
    dirs::home_dir()
        .map(|h| h.join(".gemini/antigravity-cli"))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn get_copilot_dir() -> PathBuf {
    if let Ok(val) = std::env::var("COPILOT_DIR") {
        let p = PathBuf::from(val);
        if p.exists() {
            return p;
        }
    }
    dirs::home_dir()
        .map(|h| h.join(".copilot"))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn get_codex_dir() -> PathBuf {
    if let Ok(val) = std::env::var("CODEX_DIR") {
        let p = PathBuf::from(val);
        if p.exists() {
            return p;
        }
    }
    dirs::home_dir()
        .map(|h| h.join(".codex"))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn get_claude_dir() -> PathBuf {
    if let Ok(val) = std::env::var("CLAUDE_DIR") {
        let p = PathBuf::from(val);
        if p.exists() {
            return p;
        }
    }
    dirs::home_dir()
        .map(|h| h.join(".claude"))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Get connection to centralized SQLite DB
pub fn get_db_conn() -> Result<Connection, String> {
    let dir = get_insights_dir();
    let db_path = dir.join("token_usage_insights.db");

    // Automatically move old centralized database if it exists in the legacy folder
    if !db_path.exists() {
        if let Some(home) = dirs::home_dir() {
            let old_unified_db = home.join(".gemini/antigravity-cli/token_usage_insights.db");
            if old_unified_db.exists() {
                println!(
                    "🔄 偵測到存在於舊位置的統一資料庫，正在移動至新位置：{:?} -> {:?}",
                    old_unified_db, db_path
                );
                if let Err(e) = fs::rename(&old_unified_db, &db_path) {
                    eprintln!("⚠️ 移動舊統一資料庫失敗: {}", e);
                } else {
                    println!("✅ 統一資料庫移動完成！");
                }
            }
        }
    }

    let conn = Connection::open(&db_path).map_err(|e| format!("無法開啟資料庫: {}", e))?;
    let _ = conn.busy_timeout(std::time::Duration::from_millis(15000));
    Ok(conn)
}

/// Initialize SQLite DB tables and indexes
pub fn init_db(conn: &Connection) -> Result<(), String> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS usage_entries (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            assistant_type TEXT NOT NULL, -- 'antigravity', 'copilot', 'codex', 'claude'
            timestamp TEXT NOT NULL,
            date TEXT NOT NULL,
            session_id TEXT NOT NULL,
            session_name TEXT,
            transcript_path TEXT,
            cwd TEXT,
            version TEXT,
            turn_no INTEGER NOT NULL,
            model TEXT,
            model_id TEXT,
            
            -- Token Statistics
            tokens_input INTEGER,
            tokens_output INTEGER,
            tokens_cache_read INTEGER,
            tokens_cache_write INTEGER,
            tokens_reasoning INTEGER,
            tokens_total INTEGER,
            
            -- Delta Token Statistics
            delta_input INTEGER,
            delta_output INTEGER,
            delta_cache_read INTEGER,
            delta_cache_write INTEGER,
            delta_reasoning INTEGER,
            delta_total INTEGER,
            
            -- Duration and Request Count
            duration_ms INTEGER,
            premium_requests INTEGER,

            -- Codex-specific fields
            parent_session_id TEXT,
            agent_nickname TEXT,
            agent_role TEXT,
            reasoning_effort TEXT
        )",
        [],
    )
    .map_err(|e| format!("建立 usage_entries 表失敗: {}", e))?;

    // Ensure reasoning_effort column is present in case database already exists
    let _ = conn.execute(
        "ALTER TABLE usage_entries ADD COLUMN reasoning_effort TEXT",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE usage_entries ADD COLUMN tokens_cache_write INTEGER",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE usage_entries ADD COLUMN delta_cache_write INTEGER",
        [],
    );

    // Unique index on assistant, session, and turn
    conn.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS uidx_assistant_session_turn 
         ON usage_entries(assistant_type, session_id, turn_no)",
        [],
    )
    .map_err(|e| format!("建立唯一索引 uidx_assistant_session_turn 失敗: {}", e))?;

    // Indexes for performance
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_usage_date ON usage_entries(date)",
        [],
    )
    .map_err(|e| format!("建立日期索引 idx_usage_date 失敗: {}", e))?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_assistant_type ON usage_entries(assistant_type)",
        [],
    )
    .map_err(|e| format!("建立助理類型索引 idx_assistant_type 失敗: {}", e))?;

    // Sync state tracking table
    conn.execute(
        "CREATE TABLE IF NOT EXISTS sync_state (
            filename TEXT PRIMARY KEY,
            last_synced_size INTEGER NOT NULL,
            last_synced_time INTEGER NOT NULL
        )",
        [],
    )
    .map_err(|e| format!("建立 sync_state 表失敗: {}", e))?;

    Ok(())
}

/// Helper to parse usage entries from jsonl files (Antigravity & Copilot)
fn parse_usage_entries(content: &str) -> Vec<UsageEntry> {
    let stream = serde_json::Deserializer::from_str(content).into_iter::<UsageEntry>();
    stream.filter_map(Result::ok).collect()
}

fn get_antigravity_session_name(session_id: &str) -> Option<String> {
    let path = get_antigravity_dir()
        .join("brain")
        .join(session_id)
        .join(".system_generated/logs/transcript_full.jsonl");
    if !path.exists() {
        return None;
    }
    let file = File::open(path).ok()?;
    let reader = BufReader::new(file);
    for line_res in reader.lines() {
        let line = line_res.ok()?;
        let event: serde_json::Value = serde_json::from_str(&line).ok()?;
        if event.get("type").and_then(|t| t.as_str()) == Some("USER_INPUT") {
            if let Some(content) = event.get("content").and_then(|c| c.as_str()) {
                let request_text = if let Some(start_idx) = content.find("<USER_REQUEST>") {
                    let actual_start = start_idx + "<USER_REQUEST>".len();
                    if let Some(end_idx) = content[actual_start..].find("</USER_REQUEST>") {
                        &content[actual_start..(actual_start + end_idx)]
                    } else {
                        content
                    }
                } else {
                    content
                };
                let trimmed = request_text.trim();
                let cleaned = trimmed.replace('\r', "").replace('\n', " ");
                let truncated = cleaned.chars().take(100).collect::<String>();
                return Some(truncated);
            }
            break;
        }
    }
    None
}

/// Sync usage logs for hooks-based assistant (Antigravity or Copilot)
fn sync_hook_usage_logs(
    conn: &mut Connection,
    assistant_type: &str,
    base_dir: &Path,
) -> Result<(), String> {
    if assistant_type == "antigravity" {
        // Perform migration if we haven't tracked it yet to update antigravity session names
        let migration_done: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_state WHERE filename = 'migration:antigravity_user_request_names')",
                [],
                |row| row.get(0),
            )
            .unwrap_or(false);

        if !migration_done {
            let _ = conn.execute(
                "DELETE FROM sync_state WHERE filename LIKE 'antigravity:%'",
                [],
            );
            let _ = conn.execute(
                "DELETE FROM usage_entries WHERE assistant_type = 'antigravity'",
                [],
            );
            let _ = conn.execute(
                "INSERT OR REPLACE INTO sync_state (filename, last_synced_size, last_synced_time) VALUES ('migration:antigravity_user_request_names', 1, 0)",
                [],
            );
        }
    }

    let usage_dir = base_dir.join("usage");
    if !usage_dir.exists() {
        return Ok(());
    }

    let entries = fs::read_dir(usage_dir).map_err(|e| format!("無法讀取 usage 目錄: {}", e))?;

    for entry in entries.flatten() {
        let file_type = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };

        if !file_type.is_file() {
            continue;
        }

        let filename = entry.file_name().to_string_lossy().into_owned();
        if !filename.starts_with("usage-") || !filename.ends_with(".jsonl") {
            continue;
        }

        let date_str = filename
            .trim_start_matches("usage-")
            .trim_end_matches(".jsonl")
            .to_string();

        let filepath = entry.path();

        // Scope the sync_state key with the assistant prefix to prevent key collision
        let state_key = format!("{}:{}", assistant_type, filename);

        let last_synced_size: u64 = conn
            .query_row(
                "SELECT last_synced_size FROM sync_state WHERE filename = ?",
                params![state_key],
                |row| row.get(0),
            )
            .unwrap_or(0u64);

        let mut file =
            File::open(&filepath).map_err(|e| format!("無法開啟日誌檔 {}: {}", filename, e))?;
        let metadata = file
            .metadata()
            .map_err(|e| format!("無法取得檔案資訊 {}: {}", filename, e))?;
        let current_size = metadata.len();

        let start_pos = if current_size < last_synced_size {
            0
        } else {
            last_synced_size
        };

        if current_size > start_pos {
            file.seek(SeekFrom::Start(start_pos))
                .map_err(|e| format!("Seek 失敗 {}: {}", filename, e))?;
            let mut buffer = Vec::new();
            file.read_to_end(&mut buffer)
                .map_err(|e| format!("讀取檔案失敗 {}: {}", filename, e))?;

            let mut read_len = buffer.len();
            while read_len > 0 && buffer[read_len - 1] != b'\n' {
                read_len -= 1;
            }

            if read_len > 0 {
                let new_content = String::from_utf8_lossy(&buffer[..read_len]);
                let parsed_entries = parse_usage_entries(&new_content);

                if parsed_entries.is_empty() {
                    continue;
                }

                let tx = conn
                    .transaction()
                    .map_err(|e| format!("Transaction BEGIN 失敗: {}", e))?;

                let mut success = true;
                for entry in &parsed_entries {
                    let tokens = entry.tokens.as_ref();
                    let delta = entry.delta_tokens.as_ref();
                    let cost = entry.cost.as_ref();

                    let mut resolved_name = entry.session_name.clone();
                    if assistant_type == "antigravity" {
                        if let Some(name) = get_antigravity_session_name(&entry.session_id) {
                            resolved_name = Some(name);
                        }
                    }

                    let insert_res = tx.execute(
                        "INSERT OR IGNORE INTO usage_entries (
                            assistant_type, timestamp, date, session_id, session_name, transcript_path, cwd, version, turn_no, model, model_id,
                            tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, tokens_reasoning, tokens_total,
                            delta_input, delta_output, delta_cache_read, delta_cache_write, delta_reasoning, delta_total,
                            duration_ms, premium_requests
                        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                        params![
                            assistant_type,
                            entry.timestamp,
                            date_str,
                            entry.session_id,
                            resolved_name.as_deref(),
                            entry.transcript_path.as_deref(),
                            entry.cwd.as_deref(),
                            entry.version.as_deref(),
                            entry.turn_no as i64,
                            entry.model.as_deref(),
                            entry.model_id.as_deref(),
                            tokens.map(|t| t.input as i64),
                            tokens.map(|t| t.output as i64),
                            tokens.and_then(|t| t.cache_read.map(|v| v as i64)),
                            tokens.and_then(|t| t.cache_write.map(|v| v as i64)),
                            tokens.and_then(|t| t.reasoning.map(|v| v as i64)),
                            tokens.map(|t| t.total as i64),
                            delta.map(|t| t.input as i64),
                            delta.map(|t| t.output as i64),
                            delta.and_then(|t| t.cache_read.map(|v| v as i64)),
                            delta.and_then(|t| t.cache_write.map(|v| v as i64)),
                            delta.and_then(|t| t.reasoning.map(|v| v as i64)),
                            delta.map(|t| t.total as i64),
                            cost.and_then(|c| c.total_api_duration_ms.map(|d| d as i64)),
                            cost.and_then(|c| c.total_premium_requests.map(|r| r as i64))
                        ],
                    );

                    if let Err(e) = insert_res {
                        eprintln!("[{}] 寫入資料庫失敗: {}", assistant_type, e);
                        success = false;
                        break;
                    }
                }

                if success {
                    let now = SystemTime::now()
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs() as i64;

                    let update_state_res = tx.execute(
                        "INSERT OR REPLACE INTO sync_state (filename, last_synced_size, last_synced_time) VALUES (?, ?, ?)",
                        params![state_key, (start_pos + read_len as u64) as i64, now],
                    );

                    if update_state_res.is_ok() {
                        if let Err(e) = tx.commit() {
                            eprintln!("Transaction COMMIT 失敗: {}", e);
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

fn find_codex_session_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(find_codex_session_files(&path));
            } else if path.is_file()
                && path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("jsonl"))
            {
                files.push(path);
            }
        }
    }
    files
}

fn codex_content_to_text(content: &serde_json::Value) -> String {
    if let Some(text) = content.as_str() {
        return text.replace('\r', "").replace('\n', " ");
    }

    let mut parts = Vec::new();
    if let Some(items) = content.as_array() {
        for item in items {
            match item.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                "input_text" | "output_text" | "text" => {
                    if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                        parts.push(text.replace('\r', "").replace('\n', " "));
                    }
                }
                _ => {}
            }
        }
    }
    parts.join(" ")
}

fn codex_usage_to_stats(usage: CodexTokenUsage) -> TokenStats {
    let cache_read = usage.cached_input_tokens;
    let input = usage.input_tokens.saturating_sub(cache_read);
    let output = usage.output_tokens;
    let total = if usage.total_tokens > 0 {
        usage.total_tokens
    } else {
        input.saturating_add(cache_read).saturating_add(output)
    };

    TokenStats {
        input,
        output,
        cache_read: Some(cache_read),
        cache_write: None,
        reasoning: Some(usage.reasoning_output_tokens),
        total,
    }
}

fn codex_usage_delta_to_stats(
    previous: Option<&CodexTokenUsage>,
    current: &CodexTokenUsage,
) -> TokenStats {
    let (input_tokens, cached_input_tokens, output_tokens, reasoning_output_tokens) = match previous
    {
        Some(previous)
            if current.input_tokens >= previous.input_tokens
                && current.cached_input_tokens >= previous.cached_input_tokens
                && current.output_tokens >= previous.output_tokens
                && current.reasoning_output_tokens >= previous.reasoning_output_tokens =>
        {
            (
                current.input_tokens - previous.input_tokens,
                current.cached_input_tokens - previous.cached_input_tokens,
                current.output_tokens - previous.output_tokens,
                current.reasoning_output_tokens - previous.reasoning_output_tokens,
            )
        }
        _ => (
            current.input_tokens,
            current.cached_input_tokens,
            current.output_tokens,
            current.reasoning_output_tokens,
        ),
    };

    let cache_read = cached_input_tokens;
    let input = input_tokens.saturating_sub(cache_read);
    let output = output_tokens;
    let total = input_tokens.saturating_add(output);

    TokenStats {
        input,
        output,
        cache_read: Some(cache_read),
        cache_write: None,
        reasoning: Some(reasoning_output_tokens),
        total,
    }
}

fn parse_codex_session_file(filepath: &Path) -> Result<Vec<UsageEntry>, String> {
    let file = File::open(filepath).map_err(|e| format!("無法開啟檔案: {}", e))?;
    let reader = BufReader::new(file);
    let fallback_session_id = filepath
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("unknown-session")
        .trim_start_matches("rollout-")
        .to_string();

    let mut events = Vec::new();
    for line_res in reader.lines() {
        let line = match line_res {
            Ok(line) => line,
            Err(_) => continue,
        };
        if let Ok(event) = serde_json::from_str::<serde_json::Value>(&line) {
            events.push(event);
        }
    }

    let mut session_id = fallback_session_id.clone();
    let mut session_name: Option<String> = None;
    let mut session_cwd: Option<String> = None;
    let mut session_version: Option<String> = None;
    let mut parent_session_id: Option<String> = None;
    let mut agent_nickname: Option<String> = None;
    let mut agent_role: Option<String> = None;
    let mut current_model = "Codex CLI".to_string();
    let mut reasoning_effort: Option<String> = None;

    for event in &events {
        let event_type = event.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let payload = match event.get("payload") {
            Some(payload) => payload,
            None => continue,
        };
        let payload_type = payload.get("type").and_then(|t| t.as_str()).unwrap_or("");

        if event_type == "session_meta" {
            if let Some(id) = payload
                .get("session_id")
                .or_else(|| payload.get("id"))
                .and_then(|id| id.as_str())
            {
                session_id = id.to_string();
            }
            session_cwd = payload
                .get("cwd")
                .and_then(|cwd| cwd.as_str())
                .map(|cwd| cwd.to_string())
                .or(session_cwd);
            session_version = payload
                .get("cli_version")
                .and_then(|version| version.as_str())
                .map(|version| version.to_string())
                .or(session_version);
            parent_session_id = payload
                .get("parent_thread_id")
                .and_then(|id| id.as_str())
                .map(|id| id.to_string())
                .or(parent_session_id);
            agent_nickname = payload
                .get("agent_nickname")
                .and_then(|name| name.as_str())
                .map(|name| name.to_string())
                .or(agent_nickname);
            agent_role = payload
                .get("agent_role")
                .and_then(|role| role.as_str())
                .map(|role| role.to_string())
                .or(agent_role);
            if let Some(model) = payload.get("model").and_then(|model| model.as_str()) {
                current_model = model.to_string();
            }
        } else if event_type == "turn_context" {
            session_cwd = payload
                .get("cwd")
                .and_then(|cwd| cwd.as_str())
                .map(|cwd| cwd.to_string())
                .or(session_cwd);
            if let Some(model) = payload.get("model").and_then(|model| model.as_str()) {
                current_model = model.to_string();
            }
            reasoning_effort = payload
                .get("effort")
                .or_else(|| payload.get("reasoning_effort"))
                .and_then(|effort| effort.as_str())
                .map(|effort| effort.to_string())
                .or(reasoning_effort);
        } else if session_name.is_none()
            && event_type == "event_msg"
            && payload_type == "user_message"
        {
            if let Some(message) = payload.get("message").and_then(|message| message.as_str()) {
                let cleaned = message.trim().replace('\r', "").replace('\n', " ");
                if !cleaned.is_empty() {
                    session_name = Some(cleaned.chars().take(100).collect());
                }
            }
        } else if session_name.is_none()
            && event_type == "response_item"
            && payload_type == "message"
            && payload.get("role").and_then(|role| role.as_str()) == Some("user")
        {
            if let Some(content) = payload.get("content") {
                let cleaned = codex_content_to_text(content);
                if !cleaned.trim().is_empty() {
                    session_name = Some(cleaned.trim().chars().take(100).collect());
                }
            }
        }
    }

    let mut results = Vec::new();
    let mut model_for_turn = current_model.clone();
    let mut effort_for_turn = reasoning_effort.clone();
    let mut previous_total_usage: Option<CodexTokenUsage> = None;

    for event in events {
        let event_type = event.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let timestamp = event
            .get("timestamp")
            .and_then(|timestamp| timestamp.as_str())
            .unwrap_or("")
            .to_string();
        let payload = match event.get("payload") {
            Some(payload) => payload,
            None => continue,
        };
        let payload_type = payload.get("type").and_then(|t| t.as_str()).unwrap_or("");

        if event_type == "turn_context" {
            if let Some(model) = payload.get("model").and_then(|model| model.as_str()) {
                model_for_turn = model.to_string();
            }
            effort_for_turn = payload
                .get("effort")
                .or_else(|| payload.get("reasoning_effort"))
                .and_then(|effort| effort.as_str())
                .map(|effort| effort.to_string())
                .or(effort_for_turn);
            continue;
        }

        if event_type != "event_msg" || payload_type != "token_count" {
            continue;
        }

        let info = match payload.get("info") {
            Some(info) => info,
            None => continue,
        };
        let total_usage = match info
            .get("total_token_usage")
            .cloned()
            .and_then(|value| serde_json::from_value::<CodexTokenUsage>(value).ok())
        {
            Some(usage) => usage,
            None => continue,
        };
        let delta_tokens = codex_usage_delta_to_stats(previous_total_usage.as_ref(), &total_usage);
        previous_total_usage = Some(total_usage.clone());

        let context = info
            .get("model_context_window")
            .and_then(|window| window.as_u64())
            .map(|window| ContextStats {
                current_context_tokens: None,
                displayed_context_limit: Some(window),
                current_context_used_percentage: None,
            });

        results.push(UsageEntry {
            timestamp,
            session_id: session_id.clone(),
            session_name: session_name
                .clone()
                .or_else(|| Some(fallback_session_id.clone())),
            transcript_path: Some(filepath.to_string_lossy().into_owned()),
            cwd: session_cwd.clone(),
            version: session_version.clone(),
            turn_no: (results.len() + 1) as u32,
            model: Some(model_for_turn.clone()),
            model_id: Some(model_for_turn.clone()),
            tokens: Some(codex_usage_to_stats(total_usage)),
            delta_tokens: Some(delta_tokens),
            context,
            cost: None,
            parent_session_id: parent_session_id.clone(),
            agent_nickname: agent_nickname.clone(),
            agent_role: agent_role.clone(),
            reasoning_effort: effort_for_turn.clone(),
        });
    }

    Ok(results)
}

fn sync_codex_usage_logs(conn: &mut Connection) -> Result<(), String> {
    let codex_dir = get_codex_dir();
    let sessions_dir = codex_dir.join("sessions");
    if !sessions_dir.exists() {
        return Ok(());
    }

    let delta_parser_migration_done: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_state WHERE filename = ?)",
            params![CODEX_DELTA_PARSER_MIGRATION_KEY],
            |row| row.get(0),
        )
        .unwrap_or(false);

    if !delta_parser_migration_done {
        let tx = conn
            .transaction()
            .map_err(|e| format!("Codex parser migration BEGIN 失敗: {}", e))?;
        tx.execute(
            "DELETE FROM sync_state WHERE filename LIKE 'codex:sessions/%'",
            [],
        )
        .map_err(|e| format!("清除 Codex 同步狀態失敗: {}", e))?;
        tx.execute(
            "INSERT OR REPLACE INTO sync_state (filename, last_synced_size, last_synced_time) VALUES (?, 1, 0)",
            params![CODEX_DELTA_PARSER_MIGRATION_KEY],
        )
        .map_err(|e| format!("寫入 Codex parser migration 狀態失敗: {}", e))?;
        tx.commit()
            .map_err(|e| format!("Codex parser migration COMMIT 失敗: {}", e))?;
    }

    let files = find_codex_session_files(&sessions_dir);

    for filepath in files {
        let state_path = filepath
            .strip_prefix(&codex_dir)
            .unwrap_or(&filepath)
            .to_string_lossy()
            .into_owned();
        let state_key = format!("codex:{}", state_path);

        let last_synced_size: u64 = conn
            .query_row(
                "SELECT last_synced_size FROM sync_state WHERE filename = ?",
                params![state_key],
                |row| row.get(0),
            )
            .unwrap_or(0u64);

        let metadata = match fs::metadata(&filepath) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        let current_size = metadata.len();

        if current_size != last_synced_size {
            let parsed_entries = match parse_codex_session_file(&filepath) {
                Ok(entries) => entries,
                Err(e) => {
                    eprintln!("解析 Codex CLI 會話檔案 {:?} 失敗: {}", filepath, e);
                    continue;
                }
            };

            let tx = conn
                .transaction()
                .map_err(|e| format!("Transaction BEGIN 失敗: {}", e))?;

            let session_ids: HashSet<String> = parsed_entries
                .iter()
                .map(|entry| entry.session_id.clone())
                .collect();
            for session_id in session_ids {
                let delete_res = tx.execute(
                    "DELETE FROM usage_entries WHERE assistant_type = 'codex' AND session_id = ?",
                    params![session_id],
                );

                if let Err(e) = delete_res {
                    eprintln!("清空舊 Codex CLI Session 資料失敗: {}", e);
                    continue;
                }
            }

            let mut success = true;
            for entry in &parsed_entries {
                let tokens = entry.tokens.as_ref();
                let delta = entry.delta_tokens.as_ref();
                let cost = entry.cost.as_ref();

                let insert_res = tx.execute(
                    "INSERT INTO usage_entries (
                        assistant_type, timestamp, date, session_id, session_name, transcript_path, cwd, version, turn_no, model, model_id,
                        tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, tokens_reasoning, tokens_total,
                        delta_input, delta_output, delta_cache_read, delta_cache_write, delta_reasoning, delta_total,
                        duration_ms, premium_requests, parent_session_id, agent_nickname, agent_role, reasoning_effort
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    params![
                        "codex",
                        entry.timestamp,
                        entry.timestamp.get(0..10).unwrap_or("unknown"),
                        entry.session_id,
                        entry.session_name.as_deref(),
                        entry.transcript_path.as_deref(),
                        entry.cwd.as_deref(),
                        entry.version.as_deref(),
                        entry.turn_no as i64,
                        entry.model.as_deref(),
                        entry.model_id.as_deref(),
                        tokens.map(|t| t.input as i64),
                        tokens.map(|t| t.output as i64),
                        tokens.and_then(|t| t.cache_read.map(|v| v as i64)),
                        tokens.and_then(|t| t.cache_write.map(|v| v as i64)),
                        tokens.and_then(|t| t.reasoning.map(|v| v as i64)),
                        tokens.map(|t| t.total as i64),
                        delta.map(|t| t.input as i64),
                        delta.map(|t| t.output as i64),
                        delta.and_then(|t| t.cache_read.map(|v| v as i64)),
                        delta.and_then(|t| t.cache_write.map(|v| v as i64)),
                        delta.and_then(|t| t.reasoning.map(|v| v as i64)),
                        delta.map(|t| t.total as i64),
                        cost.and_then(|c| c.total_api_duration_ms.map(|d| d as i64)),
                        cost.and_then(|c| c.total_premium_requests.map(|r| r as i64)),
                        entry.parent_session_id.as_deref(),
                        entry.agent_nickname.as_deref(),
                        entry.agent_role.as_deref(),
                        entry.reasoning_effort.as_deref()
                    ],
                );

                if let Err(e) = insert_res {
                    eprintln!(
                        "寫入 Codex CLI 資料庫失敗 (turn_no {}): {}",
                        entry.turn_no, e
                    );
                    success = false;
                    break;
                }
            }

            if success {
                let now = SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64;

                let update_state_res = tx.execute(
                    "INSERT OR REPLACE INTO sync_state (filename, last_synced_size, last_synced_time) VALUES (?, ?, ?)",
                    params![state_key, current_size as i64, now],
                );

                if update_state_res.is_ok() {
                    if let Err(e) = tx.commit() {
                        eprintln!("Transaction COMMIT 失敗: {}", e);
                    }
                }
            }
        }
    }

    Ok(())
}

fn find_claude_session_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(find_claude_session_files(&path));
            } else if path.is_file()
                && path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("jsonl"))
            {
                files.push(path);
            }
        }
    }
    files
}

fn claude_content_to_text(content: &serde_json::Value) -> String {
    if let Some(text) = content.as_str() {
        return text.replace('\r', "").replace('\n', " ");
    }

    let mut parts = Vec::new();
    if let Some(items) = content.as_array() {
        for item in items {
            match item.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                "text" => {
                    if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                        parts.push(text.replace('\r', "").replace('\n', " "));
                    }
                }
                "tool_result" => {
                    if let Some(text) = item.get("content").and_then(|c| c.as_str()) {
                        parts.push(text.replace('\r', "").replace('\n', " "));
                    }
                }
                _ => {}
            }
        }
    }
    parts.join(" ")
}

fn parse_claude_session_file(filepath: &Path) -> Result<Vec<UsageEntry>, String> {
    let file = File::open(filepath).map_err(|e| format!("無法開啟檔案: {}", e))?;
    let reader = BufReader::new(file);
    let fallback_session_id = filepath
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("unknown-session")
        .to_string();

    let mut session_name: Option<String> = None;
    let mut session_cwd: Option<String> = None;
    let mut session_version: Option<String> = None;
    let mut seen_response_keys = HashSet::new();
    let mut results = Vec::new();

    for line_res in reader.lines() {
        let line = match line_res {
            Ok(line) => line,
            Err(_) => continue,
        };
        let event: serde_json::Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => continue,
        };

        if session_cwd.is_none() {
            session_cwd = event
                .get("cwd")
                .and_then(|cwd| cwd.as_str())
                .map(|cwd| cwd.to_string());
        }
        if session_version.is_none() {
            session_version = event
                .get("version")
                .and_then(|version| version.as_str())
                .map(|version| version.to_string());
        }

        let message = match event.get("message") {
            Some(message) => message,
            None => continue,
        };
        let role = message
            .get("role")
            .and_then(|role| role.as_str())
            .unwrap_or("");

        if role == "user" && session_name.is_none() {
            if let Some(content) = message.get("content") {
                let first_message = claude_content_to_text(content);
                if !first_message.trim().is_empty() {
                    session_name = Some(first_message.chars().take(100).collect());
                }
            }
        }

        if role != "assistant" {
            continue;
        }

        let usage_value = match message.get("usage") {
            Some(usage) => usage.clone(),
            None => continue,
        };
        let usage = match serde_json::from_value::<ClaudeUsage>(usage_value) {
            Ok(usage) => usage,
            Err(_) => continue,
        };

        let response_key = event
            .get("requestId")
            .and_then(|id| id.as_str())
            .or_else(|| message.get("id").and_then(|id| id.as_str()))
            .or_else(|| event.get("uuid").and_then(|id| id.as_str()))
            .unwrap_or("");
        if response_key.is_empty() || !seen_response_keys.insert(response_key.to_string()) {
            continue;
        }

        let timestamp = event
            .get("timestamp")
            .and_then(|timestamp| timestamp.as_str())
            .unwrap_or("")
            .to_string();
        let session_id = event
            .get("sessionId")
            .and_then(|id| id.as_str())
            .unwrap_or(&fallback_session_id)
            .to_string();
        let cwd = event
            .get("cwd")
            .and_then(|cwd| cwd.as_str())
            .map(|cwd| cwd.to_string())
            .or_else(|| session_cwd.clone());
        let version = event
            .get("version")
            .and_then(|version| version.as_str())
            .map(|version| version.to_string())
            .or_else(|| session_version.clone());
        let model = message
            .get("model")
            .and_then(|model| model.as_str())
            .map(|model| model.to_string());

        let input = usage
            .input_tokens
            .saturating_add(usage.cache_creation_input_tokens);
        let cache_read = usage.cache_read_input_tokens;
        let output = usage.output_tokens;
        let total = input.saturating_add(cache_read).saturating_add(output);
        let tokens = TokenStats {
            input,
            output,
            cache_read: Some(cache_read),
            cache_write: Some(usage.cache_creation_input_tokens),
            reasoning: None,
            total,
        };

        results.push(UsageEntry {
            timestamp,
            session_id,
            session_name: session_name
                .clone()
                .or_else(|| Some(fallback_session_id.clone())),
            transcript_path: Some(filepath.to_string_lossy().into_owned()),
            cwd,
            version,
            turn_no: (results.len() + 1) as u32,
            model: model.clone(),
            model_id: model,
            tokens: Some(tokens.clone()),
            delta_tokens: Some(tokens),
            context: None,
            cost: None,
            parent_session_id: None,
            agent_nickname: None,
            agent_role: None,
            reasoning_effort: None,
        });
    }

    Ok(results)
}

/// Sync Claude Code local transcripts into the dashboard's Claude Code assistant slot.
fn sync_claude_usage_logs(conn: &mut Connection) -> Result<(), String> {
    // Move Claude Code data that was previously written into the Codex slot.
    let migration_done: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_state WHERE filename = 'migration:claude_code_source_v2')",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);

    if !migration_done {
        let _ = conn.execute(
            "UPDATE usage_entries SET assistant_type = 'claude'
             WHERE assistant_type = 'codex'
               AND transcript_path IS NOT NULL
               AND (transcript_path LIKE '%.claude/%' OR transcript_path LIKE '%/claude/%')",
            [],
        );
        let mut migrated_states = Vec::new();
        if let Ok(mut stmt) = conn.prepare(
            "SELECT filename, last_synced_size, last_synced_time FROM sync_state WHERE filename LIKE 'codex:claude:%'",
        ) {
            if let Ok(mut rows) = stmt.query([]) {
                while let Ok(Some(row)) = rows.next() {
                    let filename = row.get::<_, String>(0).unwrap_or_default();
                    let size = row.get::<_, i64>(1).unwrap_or_default();
                    let time = row.get::<_, i64>(2).unwrap_or_default();
                    migrated_states.push((
                        filename.replacen("codex:claude:", "claude:", 1),
                        size,
                        time,
                    ));
                }
            }
        }
        for (filename, size, time) in migrated_states {
            let _ = conn.execute(
                "INSERT OR REPLACE INTO sync_state (filename, last_synced_size, last_synced_time) VALUES (?, ?, ?)",
                params![filename, size, time],
            );
        }
        let _ = conn.execute(
            "DELETE FROM sync_state WHERE filename LIKE 'codex:claude:%'",
            [],
        );
        let _ = conn.execute(
            "INSERT OR REPLACE INTO sync_state (filename, last_synced_size, last_synced_time) VALUES ('migration:claude_code_source_v2', 1, 0)",
            [],
        );
    }

    let claude_dir = get_claude_dir();
    let projects_dir = claude_dir.join("projects");
    if !projects_dir.exists() {
        return Ok(());
    }

    let files = find_claude_session_files(&projects_dir);

    for filepath in files {
        let state_path = filepath
            .strip_prefix(&claude_dir)
            .unwrap_or(&filepath)
            .to_string_lossy()
            .into_owned();
        let state_key = format!("claude:{}", state_path);

        let last_synced_size: u64 = conn
            .query_row(
                "SELECT last_synced_size FROM sync_state WHERE filename = ?",
                params![state_key],
                |row| row.get(0),
            )
            .unwrap_or(0u64);

        let metadata = match fs::metadata(&filepath) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let current_size = metadata.len();

        if current_size != last_synced_size {
            let parsed_entries = match parse_claude_session_file(&filepath) {
                Ok(entries) => entries,
                Err(e) => {
                    eprintln!("解析 Claude Code 會話檔案 {:?} 失敗: {}", filepath, e);
                    continue;
                }
            };

            let tx = conn
                .transaction()
                .map_err(|e| format!("Transaction BEGIN 失敗: {}", e))?;

            // First delete old entries for this session
            let session_ids: HashSet<String> = parsed_entries
                .iter()
                .map(|entry| entry.session_id.clone())
                .collect();
            for session_id in session_ids {
                let delete_res = tx.execute(
                    "DELETE FROM usage_entries WHERE assistant_type = 'claude' AND session_id = ?",
                    params![session_id],
                );

                if let Err(e) = delete_res {
                    eprintln!("清空舊 Claude Code Session 資料失敗: {}", e);
                    continue;
                }
            }

            let mut success = true;
            for entry in &parsed_entries {
                let tokens = entry.tokens.as_ref();
                let delta = entry.delta_tokens.as_ref();
                let cost = entry.cost.as_ref();

                let insert_res = tx.execute(
                    "INSERT INTO usage_entries (
                        assistant_type, timestamp, date, session_id, session_name, transcript_path, cwd, version, turn_no, model, model_id,
                        tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, tokens_reasoning, tokens_total,
                        delta_input, delta_output, delta_cache_read, delta_cache_write, delta_reasoning, delta_total,
                        duration_ms, premium_requests, parent_session_id, agent_nickname, agent_role, reasoning_effort
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    params![
                        "claude",
                        entry.timestamp,
                        entry.timestamp.get(0..10).unwrap_or("unknown"),
                        entry.session_id,
                        entry.session_name.as_deref(),
                        entry.transcript_path.as_deref(),
                        entry.cwd.as_deref(),
                        entry.version.as_deref(),
                        entry.turn_no as i64,
                        entry.model.as_deref(),
                        entry.model_id.as_deref(),
                        tokens.map(|t| t.input as i64),
                        tokens.map(|t| t.output as i64),
                        tokens.and_then(|t| t.cache_read.map(|v| v as i64)),
                        tokens.and_then(|t| t.cache_write.map(|v| v as i64)),
                        tokens.and_then(|t| t.reasoning.map(|v| v as i64)),
                        tokens.map(|t| t.total as i64),
                        delta.map(|t| t.input as i64),
                        delta.map(|t| t.output as i64),
                        delta.and_then(|t| t.cache_read.map(|v| v as i64)),
                        delta.and_then(|t| t.cache_write.map(|v| v as i64)),
                        delta.and_then(|t| t.reasoning.map(|v| v as i64)),
                        delta.map(|t| t.total as i64),
                        cost.and_then(|c| c.total_api_duration_ms.map(|d| d as i64)),
                        cost.and_then(|c| c.total_premium_requests.map(|r| r as i64)),
                        entry.parent_session_id.as_deref(),
                        entry.agent_nickname.as_deref(),
                        entry.agent_role.as_deref(),
                        entry.reasoning_effort.as_deref()
                    ],
                );

                if let Err(e) = insert_res {
                    eprintln!(
                        "寫入 Claude Code 資料庫失敗 (turn_no {}): {}",
                        entry.turn_no, e
                    );
                    success = false;
                    break;
                }
            }

            if success {
                let now = SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64;

                let update_state_res = tx.execute(
                    "INSERT OR REPLACE INTO sync_state (filename, last_synced_size, last_synced_time) VALUES (?, ?, ?)",
                    params![state_key, current_size as i64, now],
                );

                if update_state_res.is_ok() {
                    if let Err(e) = tx.commit() {
                        eprintln!("Transaction COMMIT 失敗: {}", e);
                    }
                }
            }
        }
    }

    Ok(())
}

/// Unified sync function triggering sync for all supported assistants
pub fn sync_usage_logs(conn: &mut Connection) -> Result<(), String> {
    // 1. Sync Google Antigravity CLI
    let antigravity_dir = get_antigravity_dir();
    if let Err(e) = sync_hook_usage_logs(conn, "antigravity", &antigravity_dir) {
        eprintln!("❌ 同步 Antigravity 失敗: {}", e);
    }

    // 2. Sync GitHub Copilot CLI
    let copilot_dir = get_copilot_dir();
    if let Err(e) = sync_hook_usage_logs(conn, "copilot", &copilot_dir) {
        eprintln!("❌ 同步 Copilot 失敗: {}", e);
    }

    // 3. Sync Codex CLI
    if let Err(e) = sync_codex_usage_logs(conn) {
        eprintln!("❌ 同步 Codex CLI 失敗: {}", e);
    }

    // 4. Sync Claude Code
    if let Err(e) = sync_claude_usage_logs(conn) {
        eprintln!("❌ 同步 Claude Code 失敗: {}", e);
    }

    Ok(())
}

/// Migrate data from legacy standalone databases into the centralized DB
pub fn migrate_old_databases(dest_conn: &mut Connection) -> Result<(), String> {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return Err("無法讀取家目錄以進行資料庫遷移。".to_string()),
    };

    // 1. Migrate Antigravity
    let old_antigravity_db = home.join(".gemini/antigravity-cli/antigravity_cli_token_insights.db");
    if old_antigravity_db.exists() {
        println!("🔄 偵測到舊的 Antigravity SQLite 資料庫，正在進行數據遷移...");
        if let Ok(src_conn) = Connection::open(&old_antigravity_db) {
            if let Err(e) = migrate_records(&src_conn, dest_conn, "antigravity") {
                eprintln!("❌ 遷移 Antigravity 數據失敗: {}", e);
            } else {
                println!("✅ Antigravity 數據遷移完成！");
                let backup_path =
                    home.join(".gemini/antigravity-cli/antigravity_cli_token_insights.db.bak");
                let _ = fs::rename(&old_antigravity_db, &backup_path);
            }
        }
    }

    // 2. Migrate Copilot
    let old_copilot_db = home.join(".copilot/copilot_cli_token_insights.db");
    if old_copilot_db.exists() {
        println!("🔄 偵測到舊的 Copilot SQLite 資料庫，正在進行數據遷移...");
        if let Ok(src_conn) = Connection::open(&old_copilot_db) {
            if let Err(e) = migrate_records(&src_conn, dest_conn, "copilot") {
                eprintln!("❌ 遷移 Copilot 數據失敗: {}", e);
            } else {
                println!("✅ Copilot 數據遷移完成！");
                let backup_path = home.join(".copilot/copilot_cli_token_insights.db.bak");
                let _ = fs::rename(&old_copilot_db, &backup_path);
            }
        }
    }

    // 3. Migrate Codex
    let old_codex_db = home.join(".codex/codex_cli_token_insights.db");
    if old_codex_db.exists() {
        println!("🔄 偵測到舊的 Codex SQLite 資料庫，正在進行數據遷移...");
        if let Ok(src_conn) = Connection::open(&old_codex_db) {
            if let Err(e) = migrate_records(&src_conn, dest_conn, "codex") {
                eprintln!("❌ 遷移 Codex 數據失敗: {}", e);
            } else {
                println!("✅ Codex 數據遷移完成！");
                let backup_path = home.join(".codex/codex_cli_token_insights.db.bak");
                let _ = fs::rename(&old_codex_db, &backup_path);
            }
        }
    }

    Ok(())
}

fn migrate_records(
    src_conn: &Connection,
    dest_conn: &mut Connection,
    assistant: &str,
) -> Result<(), rusqlite::Error> {
    let table_exists: bool = src_conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='usage_entries'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0)
        > 0;

    if !table_exists {
        return Ok(());
    }

    let mut stmt = src_conn.prepare(
        "SELECT 
            timestamp, date, session_id, session_name, transcript_path, cwd, version, turn_no, model, model_id,
            tokens_input, tokens_output, tokens_cache_read, tokens_reasoning, tokens_total,
            delta_input, delta_output, delta_cache_read, delta_reasoning, delta_total,
            duration_ms, premium_requests
         FROM usage_entries"
    )?;

    let mut rows = stmt.query([])?;

    let tx = dest_conn.transaction()?;

    while let Ok(Some(row)) = rows.next() {
        let session_id = row.get::<_, String>(2)?;
        let turn_no = row.get::<_, i64>(7)?;

        let mut parent_sid: Option<String> = None;
        let mut nickname: Option<String> = None;
        let mut role: Option<String> = None;

        if assistant == "codex" {
            if let Ok(mut c_stmt) = src_conn.prepare(
                "SELECT parent_session_id, agent_nickname, agent_role FROM usage_entries WHERE session_id = ? AND turn_no = ? LIMIT 1"
            ) {
                if let Ok(mut c_rows) = c_stmt.query(params![session_id, turn_no]) {
                    if let Ok(Some(r)) = c_rows.next() {
                        parent_sid = r.get(0).ok();
                        nickname = r.get(1).ok();
                        role = r.get(2).ok();
                    }
                }
            }
        }

        let insert_res = tx.execute(
            "INSERT OR IGNORE INTO usage_entries (
                assistant_type, timestamp, date, session_id, session_name, transcript_path, cwd, version, turn_no, model, model_id,
                tokens_input, tokens_output, tokens_cache_read, tokens_reasoning, tokens_total,
                delta_input, delta_output, delta_cache_read, delta_reasoning, delta_total,
                duration_ms, premium_requests, parent_session_id, agent_nickname, agent_role
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                assistant,
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                session_id,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                turn_no,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<i64>>(10)?,
                row.get::<_, Option<i64>>(11)?,
                row.get::<_, Option<i64>>(12)?,
                row.get::<_, Option<i64>>(13)?,
                row.get::<_, Option<i64>>(14)?,
                row.get::<_, Option<i64>>(15)?,
                row.get::<_, Option<i64>>(16)?,
                row.get::<_, Option<i64>>(17)?,
                row.get::<_, Option<i64>>(18)?,
                row.get::<_, Option<i64>>(19)?,
                row.get::<_, Option<i64>>(20)?,
                row.get::<_, Option<i64>>(21)?,
                parent_sid,
                nickname,
                role
            ],
        );

        if let Err(e) = insert_res {
            eprintln!(
                "遷移單筆紀錄失敗 ({} - session_id: {}, turn_no: {}): {}",
                assistant, session_id, turn_no, e
            );
        }
    }

    // Migrate sync_state
    let sync_table_exists: bool = src_conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='sync_state'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0)
        > 0;

    if sync_table_exists {
        if let Ok(mut sync_stmt) =
            src_conn.prepare("SELECT filename, last_synced_size, last_synced_time FROM sync_state")
        {
            if let Ok(mut sync_rows) = sync_stmt.query([]) {
                while let Ok(Some(row)) = sync_rows.next() {
                    let filename = row.get::<_, String>(0)?;
                    let size = row.get::<_, i64>(1)?;
                    let time = row.get::<_, i64>(2)?;
                    let state_key = format!("{}:{}", assistant, filename);
                    let _ = tx.execute(
                        "INSERT OR REPLACE INTO sync_state (filename, last_synced_size, last_synced_time) VALUES (?, ?, ?)",
                        params![state_key, size, time],
                    );
                }
            }
        }
    }

    tx.commit()?;
    Ok(())
}

pub fn get_latest_codex_rate_limit() -> Option<serde_json::Value> {
    None
}

// =========================================================================
// Encapsulated SQL Queries (Phase 2 Refactoring)
// =========================================================================

pub fn get_available_dates(
    conn: &rusqlite::Connection,
    assistant: &str,
) -> Result<Vec<String>, String> {
    let mut dates = Vec::new();
    if assistant == "all" {
        let mut stmt = conn
            .prepare("SELECT DISTINCT date FROM usage_entries ORDER BY date DESC")
            .map_err(|e| e.to_string())?;
        let date_iter = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        for d in date_iter {
            dates.push(d.map_err(|e| e.to_string())?);
        }
    } else {
        let assistants: Vec<&str> = assistant.split(',').collect();
        let mut placeholders = Vec::new();
        let mut params_vec = Vec::new();
        for a in assistants {
            placeholders.push("?");
            params_vec.push(rusqlite::types::Value::Text(a.to_string()));
        }
        let query = format!(
            "SELECT DISTINCT date FROM usage_entries WHERE assistant_type IN ({}) ORDER BY date DESC",
            placeholders.join(",")
        );
        let mut stmt = conn.prepare(&query).map_err(|e| e.to_string())?;
        let date_iter = stmt
            .query_map(rusqlite::params_from_iter(params_vec), |row| {
                row.get::<_, String>(0)
            })
            .map_err(|e| e.to_string())?;
        for d in date_iter {
            dates.push(d.map_err(|e| e.to_string())?);
        }
    }
    Ok(dates)
}

pub fn get_usage_entries_by_date(
    conn: &rusqlite::Connection,
    date: &str,
    assistant: &str,
) -> Result<Vec<(UsageEntry, String)>, String> {
    let mut query = "SELECT 
            timestamp, session_id, session_name, transcript_path, cwd, version, turn_no, model, model_id,
            tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, tokens_reasoning, tokens_total,
            delta_input, delta_output, delta_cache_read, delta_cache_write, delta_reasoning, delta_total,
            duration_ms, premium_requests, parent_session_id, agent_nickname, agent_role, assistant_type, reasoning_effort
         FROM usage_entries WHERE date = ?".to_string();
    let mut params_vec = Vec::new();
    params_vec.push(rusqlite::types::Value::Text(date.to_string()));

    if assistant != "all" {
        let assistants: Vec<&str> = assistant.split(',').collect();
        let mut placeholders = Vec::new();
        for a in assistants {
            placeholders.push("?");
            params_vec.push(rusqlite::types::Value::Text(a.to_string()));
        }
        query.push_str(&format!(
            " AND assistant_type IN ({})",
            placeholders.join(",")
        ));
    }
    query.push_str(" ORDER BY timestamp ASC");

    let mut stmt = conn.prepare(&query).map_err(|e| e.to_string())?;
    let mut rows = stmt
        .query(rusqlite::params_from_iter(params_vec))
        .map_err(|e| e.to_string())?;

    let mut entries = Vec::new();
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let ast_type = row.get::<_, String>(26).map_err(|e| e.to_string())?;
        let tokens_input: Option<u64> = row
            .get::<_, Option<i64>>(9)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let tokens_output: Option<u64> = row
            .get::<_, Option<i64>>(10)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let tokens_cache_read: Option<u64> = row
            .get::<_, Option<i64>>(11)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let tokens_cache_write: Option<u64> = row
            .get::<_, Option<i64>>(12)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let tokens_reasoning: Option<u64> = row
            .get::<_, Option<i64>>(13)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let tokens_total: Option<u64> = row
            .get::<_, Option<i64>>(14)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);

        let tokens = if let (Some(input), Some(output), Some(total)) =
            (tokens_input, tokens_output, tokens_total)
        {
            Some(TokenStats {
                input,
                output,
                cache_read: tokens_cache_read,
                cache_write: tokens_cache_write,
                reasoning: tokens_reasoning,
                total,
            })
        } else {
            None
        };

        let delta_input: Option<u64> = row
            .get::<_, Option<i64>>(15)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let delta_output: Option<u64> = row
            .get::<_, Option<i64>>(16)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let delta_cache_read: Option<u64> = row
            .get::<_, Option<i64>>(17)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let delta_cache_write: Option<u64> = row
            .get::<_, Option<i64>>(18)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let delta_reasoning: Option<u64> = row
            .get::<_, Option<i64>>(19)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let delta_total: Option<u64> = row
            .get::<_, Option<i64>>(20)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);

        let delta_tokens = if let (Some(input), Some(output), Some(total)) =
            (delta_input, delta_output, delta_total)
        {
            Some(TokenStats {
                input,
                output,
                cache_read: delta_cache_read,
                cache_write: delta_cache_write,
                reasoning: delta_reasoning,
                total,
            })
        } else {
            None
        };

        let duration_ms: Option<f64> = row
            .get::<_, Option<i64>>(21)
            .map_err(|e| e.to_string())?
            .map(|v| v as f64);
        let premium_requests: Option<f64> = row
            .get::<_, Option<i64>>(22)
            .map_err(|e| e.to_string())?
            .map(|v| v as f64);

        let cost = if duration_ms.is_some() || premium_requests.is_some() {
            Some(CostStats {
                total_api_duration_ms: duration_ms,
                total_duration_ms: None,
                total_premium_requests: premium_requests,
            })
        } else {
            None
        };

        entries.push((
            UsageEntry {
                timestamp: row.get(0).map_err(|e| e.to_string())?,
                session_id: row.get(1).map_err(|e| e.to_string())?,
                session_name: row.get(2).ok(),
                transcript_path: row.get(3).ok(),
                cwd: row.get(4).ok(),
                version: row.get(5).ok(),
                turn_no: row.get::<_, i64>(6).map_err(|e| e.to_string())? as u32,
                model: row.get(7).ok(),
                model_id: row.get(8).ok(),
                tokens,
                delta_tokens,
                context: None,
                cost,
                parent_session_id: row.get(23).ok(),
                agent_nickname: row.get(24).ok(),
                agent_role: row.get(25).ok(),
                reasoning_effort: row.get(27).ok(),
            },
            ast_type,
        ));
    }
    Ok(entries)
}

pub fn get_session_assistant_and_transcript(
    conn: &rusqlite::Connection,
    assistant: &str,
    session_id: &str,
) -> Result<(String, Option<String>), String> {
    let mut stmt = conn
        .prepare(
            "SELECT assistant_type, transcript_path FROM usage_entries WHERE session_id = ? AND assistant_type = ? LIMIT 1",
        )
        .map_err(|e| e.to_string())?;
    let mut rows = stmt
        .query(params![session_id, assistant])
        .map_err(|e| e.to_string())?;
    if let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let ast: String = row.get(0).map_err(|e| e.to_string())?;
        let path: Option<String> = row.get(1).ok();
        Ok((ast, path))
    } else {
        Err("Session not found".to_string())
    }
}

pub fn get_session_cwd(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<Option<String>, String> {
    let mut stmt = conn
        .prepare("SELECT cwd FROM usage_entries WHERE session_id = ? AND cwd IS NOT NULL LIMIT 1")
        .map_err(|e| e.to_string())?;
    let mut rows = stmt.query(params![session_id]).map_err(|e| e.to_string())?;
    if let Some(row) = rows.next().map_err(|e| e.to_string())? {
        Ok(row.get::<_, String>(0).ok())
    } else {
        Ok(None)
    }
}

pub fn get_session_turns_token_stats(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<HashMap<u32, (TokenStats, String)>, String> {
    let mut map = HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT turn_no, delta_input, delta_output, delta_cache_read, delta_cache_write, delta_reasoning, delta_total, model
         FROM usage_entries WHERE session_id = ? ORDER BY turn_no ASC"
    ).map_err(|e| e.to_string())?;
    let mut rows = stmt.query(params![session_id]).map_err(|e| e.to_string())?;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        if let (Ok(turn_no), Ok(delta_input), Ok(delta_output), Ok(delta_total)) = (
            row.get::<_, i64>(0),
            row.get::<_, Option<i64>>(1),
            row.get::<_, Option<i64>>(2),
            row.get::<_, Option<i64>>(6),
        ) {
            if let (Some(input), Some(output), Some(total)) =
                (delta_input, delta_output, delta_total)
            {
                let cache_read = row
                    .get::<_, Option<i64>>(3)
                    .ok()
                    .flatten()
                    .map(|v| v as u64);
                let cache_write = row
                    .get::<_, Option<i64>>(4)
                    .ok()
                    .flatten()
                    .map(|v| v as u64);
                let reasoning = row
                    .get::<_, Option<i64>>(5)
                    .ok()
                    .flatten()
                    .map(|v| v as u64);
                let model = row
                    .get::<_, Option<String>>(7)
                    .unwrap_or(None)
                    .unwrap_or_else(|| "Gemini".to_string());
                map.insert(
                    turn_no as u32,
                    (
                        TokenStats {
                            input: input as u64,
                            output: output as u64,
                            cache_read,
                            cache_write,
                            reasoning,
                            total: total as u64,
                        },
                        model,
                    ),
                );
            }
        }
    }
    Ok(map)
}

pub fn get_available_months(
    conn: &rusqlite::Connection,
    assistant: &str,
) -> Result<Vec<String>, String> {
    let mut months = Vec::new();
    if assistant == "all" {
        let mut stmt = conn
            .prepare("SELECT DISTINCT substr(date, 1, 7) FROM usage_entries ORDER BY date DESC")
            .map_err(|e| e.to_string())?;
        let month_iter = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        for m in month_iter {
            months.push(m.map_err(|e| e.to_string())?);
        }
    } else {
        let assistants: Vec<&str> = assistant.split(',').collect();
        let mut placeholders = Vec::new();
        let mut params_vec = Vec::new();
        for a in assistants {
            placeholders.push("?");
            params_vec.push(rusqlite::types::Value::Text(a.to_string()));
        }
        let query = format!(
            "SELECT DISTINCT substr(date, 1, 7) FROM usage_entries WHERE assistant_type IN ({}) ORDER BY date DESC",
            placeholders.join(",")
        );
        let mut stmt = conn.prepare(&query).map_err(|e| e.to_string())?;
        let month_iter = stmt
            .query_map(rusqlite::params_from_iter(params_vec), |row| {
                row.get::<_, String>(0)
            })
            .map_err(|e| e.to_string())?;
        for m in month_iter {
            months.push(m.map_err(|e| e.to_string())?);
        }
    }
    Ok(months)
}

pub fn get_usage_entries_by_month(
    conn: &rusqlite::Connection,
    year_month: &str,
    assistant: &str,
) -> Result<Vec<(UsageEntry, String, String)>, String> {
    let query_month = format!("{}-%", year_month);
    let mut query = "SELECT 
            timestamp, session_id, session_name, transcript_path, cwd, version, turn_no, model, model_id,
            tokens_input, tokens_output, tokens_cache_read, tokens_reasoning, tokens_total,
            delta_input, delta_output, delta_cache_read, delta_reasoning, delta_total,
            duration_ms, premium_requests, parent_session_id, agent_nickname, agent_role, assistant_type, reasoning_effort,
            date
         FROM usage_entries WHERE date LIKE ?".to_string();
    let mut params_vec = Vec::new();
    params_vec.push(rusqlite::types::Value::Text(query_month));

    if assistant != "all" {
        let assistants: Vec<&str> = assistant.split(',').collect();
        let mut placeholders = Vec::new();
        for a in assistants {
            placeholders.push("?");
            params_vec.push(rusqlite::types::Value::Text(a.to_string()));
        }
        query.push_str(&format!(
            " AND assistant_type IN ({})",
            placeholders.join(",")
        ));
    }
    query.push_str(" ORDER BY timestamp ASC");

    let mut stmt = conn.prepare(&query).map_err(|e| e.to_string())?;
    let mut rows = stmt
        .query(rusqlite::params_from_iter(params_vec))
        .map_err(|e| e.to_string())?;

    let mut entries = Vec::new();
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let ast_type = row.get::<_, String>(24).map_err(|e| e.to_string())?;
        let tokens_input: Option<u64> = row
            .get::<_, Option<i64>>(9)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let tokens_output: Option<u64> = row
            .get::<_, Option<i64>>(10)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let tokens_cache_read: Option<u64> = row
            .get::<_, Option<i64>>(11)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let tokens_reasoning: Option<u64> = row
            .get::<_, Option<i64>>(12)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let tokens_total: Option<u64> = row
            .get::<_, Option<i64>>(13)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);

        let tokens = if let (Some(input), Some(output), Some(total)) =
            (tokens_input, tokens_output, tokens_total)
        {
            Some(TokenStats {
                input,
                output,
                cache_read: tokens_cache_read,
                cache_write: None,
                reasoning: tokens_reasoning,
                total,
            })
        } else {
            None
        };

        let delta_input: Option<u64> = row
            .get::<_, Option<i64>>(14)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let delta_output: Option<u64> = row
            .get::<_, Option<i64>>(15)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let delta_cache_read: Option<u64> = row
            .get::<_, Option<i64>>(16)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let delta_reasoning: Option<u64> = row
            .get::<_, Option<i64>>(17)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let delta_total: Option<u64> = row
            .get::<_, Option<i64>>(18)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);

        let delta_tokens = if let (Some(input), Some(output), Some(total)) =
            (delta_input, delta_output, delta_total)
        {
            Some(TokenStats {
                input,
                output,
                cache_read: delta_cache_read,
                cache_write: None,
                reasoning: delta_reasoning,
                total,
            })
        } else {
            None
        };

        let duration_ms: Option<f64> = row
            .get::<_, Option<i64>>(19)
            .map_err(|e| e.to_string())?
            .map(|v| v as f64);
        let premium_requests: Option<f64> = row
            .get::<_, Option<i64>>(20)
            .map_err(|e| e.to_string())?
            .map(|v| v as f64);

        let cost = if duration_ms.is_some() || premium_requests.is_some() {
            Some(CostStats {
                total_api_duration_ms: duration_ms,
                total_duration_ms: None,
                total_premium_requests: premium_requests,
            })
        } else {
            None
        };

        let entry_date = row.get::<_, String>(26).map_err(|e| e.to_string())?;

        entries.push((
            UsageEntry {
                timestamp: row.get(0).map_err(|e| e.to_string())?,
                session_id: row.get(1).map_err(|e| e.to_string())?,
                session_name: row.get(2).ok(),
                transcript_path: row.get(3).ok(),
                cwd: row.get(4).ok(),
                version: row.get(5).ok(),
                turn_no: row.get::<_, i64>(6).map_err(|e| e.to_string())? as u32,
                model: row.get(7).ok(),
                model_id: row.get(8).ok(),
                tokens,
                delta_tokens,
                context: None,
                cost,
                parent_session_id: row.get(21).ok(),
                agent_nickname: row.get(22).ok(),
                agent_role: row.get(23).ok(),
                reasoning_effort: row.get(25).ok(),
            },
            ast_type,
            entry_date,
        ));
    }
    Ok(entries)
}

pub fn get_available_years(
    conn: &rusqlite::Connection,
    assistant: &str,
) -> Result<Vec<String>, String> {
    let mut years = Vec::new();
    if assistant == "all" {
        let mut stmt = conn
            .prepare("SELECT DISTINCT substr(date, 1, 4) FROM usage_entries ORDER BY date DESC")
            .map_err(|e| e.to_string())?;
        let year_iter = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        for y in year_iter {
            years.push(y.map_err(|e| e.to_string())?);
        }
    } else {
        let assistants: Vec<&str> = assistant.split(',').collect();
        let mut placeholders = Vec::new();
        let mut params_vec = Vec::new();
        for a in assistants {
            placeholders.push("?");
            params_vec.push(rusqlite::types::Value::Text(a.to_string()));
        }
        let query = format!(
            "SELECT DISTINCT substr(date, 1, 4) FROM usage_entries WHERE assistant_type IN ({}) ORDER BY date DESC",
            placeholders.join(",")
        );
        let mut stmt = conn.prepare(&query).map_err(|e| e.to_string())?;
        let year_iter = stmt
            .query_map(rusqlite::params_from_iter(params_vec), |row| {
                row.get::<_, String>(0)
            })
            .map_err(|e| e.to_string())?;
        for y in year_iter {
            years.push(y.map_err(|e| e.to_string())?);
        }
    }
    Ok(years)
}

pub fn get_usage_entries_by_year(
    conn: &rusqlite::Connection,
    year: &str,
    assistant: &str,
) -> Result<Vec<(UsageEntry, String, String)>, String> {
    let query_year = format!("{}-%", year);
    let mut query = "SELECT 
            timestamp, session_id, session_name, transcript_path, cwd, version, turn_no, model, model_id,
            tokens_input, tokens_output, tokens_cache_read, tokens_reasoning, tokens_total,
            delta_input, delta_output, delta_cache_read, delta_reasoning, delta_total,
            duration_ms, premium_requests, parent_session_id, agent_nickname, agent_role, assistant_type, reasoning_effort,
            date
         FROM usage_entries WHERE date LIKE ?".to_string();
    let mut params_vec = Vec::new();
    params_vec.push(rusqlite::types::Value::Text(query_year));

    if assistant != "all" {
        let assistants: Vec<&str> = assistant.split(',').collect();
        let mut placeholders = Vec::new();
        for a in assistants {
            placeholders.push("?");
            params_vec.push(rusqlite::types::Value::Text(a.to_string()));
        }
        query.push_str(&format!(
            " AND assistant_type IN ({})",
            placeholders.join(",")
        ));
    }
    query.push_str(" ORDER BY timestamp ASC");

    let mut stmt = conn.prepare(&query).map_err(|e| e.to_string())?;
    let mut rows = stmt
        .query(rusqlite::params_from_iter(params_vec))
        .map_err(|e| e.to_string())?;

    let mut entries = Vec::new();
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let ast_type = row.get::<_, String>(24).map_err(|e| e.to_string())?;
        let tokens_input: Option<u64> = row
            .get::<_, Option<i64>>(9)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let tokens_output: Option<u64> = row
            .get::<_, Option<i64>>(10)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let tokens_cache_read: Option<u64> = row
            .get::<_, Option<i64>>(11)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let tokens_reasoning: Option<u64> = row
            .get::<_, Option<i64>>(12)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let tokens_total: Option<u64> = row
            .get::<_, Option<i64>>(13)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);

        let tokens = if let (Some(input), Some(output), Some(total)) =
            (tokens_input, tokens_output, tokens_total)
        {
            Some(TokenStats {
                input,
                output,
                cache_read: tokens_cache_read,
                cache_write: None,
                reasoning: tokens_reasoning,
                total,
            })
        } else {
            None
        };

        let delta_input: Option<u64> = row
            .get::<_, Option<i64>>(14)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let delta_output: Option<u64> = row
            .get::<_, Option<i64>>(15)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let delta_cache_read: Option<u64> = row
            .get::<_, Option<i64>>(16)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let delta_reasoning: Option<u64> = row
            .get::<_, Option<i64>>(17)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let delta_total: Option<u64> = row
            .get::<_, Option<i64>>(18)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);

        let delta_tokens = if let (Some(input), Some(output), Some(total)) =
            (delta_input, delta_output, delta_total)
        {
            Some(TokenStats {
                input,
                output,
                cache_read: delta_cache_read,
                cache_write: None,
                reasoning: delta_reasoning,
                total,
            })
        } else {
            None
        };

        let duration_ms: Option<f64> = row
            .get::<_, Option<i64>>(19)
            .map_err(|e| e.to_string())?
            .map(|v| v as f64);
        let premium_requests: Option<f64> = row
            .get::<_, Option<i64>>(20)
            .map_err(|e| e.to_string())?
            .map(|v| v as f64);

        let cost = if duration_ms.is_some() || premium_requests.is_some() {
            Some(CostStats {
                total_api_duration_ms: duration_ms,
                total_duration_ms: None,
                total_premium_requests: premium_requests,
            })
        } else {
            None
        };

        let entry_date = row.get::<_, String>(26).map_err(|e| e.to_string())?;

        entries.push((
            UsageEntry {
                timestamp: row.get(0).map_err(|e| e.to_string())?,
                session_id: row.get(1).map_err(|e| e.to_string())?,
                session_name: row.get(2).ok(),
                transcript_path: row.get(3).ok(),
                cwd: row.get(4).ok(),
                version: row.get(5).ok(),
                turn_no: row.get::<_, i64>(6).map_err(|e| e.to_string())? as u32,
                model: row.get(7).ok(),
                model_id: row.get(8).ok(),
                tokens,
                delta_tokens,
                context: None,
                cost,
                parent_session_id: row.get(21).ok(),
                agent_nickname: row.get(22).ok(),
                agent_role: row.get(23).ok(),
                reasoning_effort: row.get(25).ok(),
            },
            ast_type,
            entry_date,
        ));
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    };

    static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn temp_jsonl_path(prefix: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        path.push(format!(
            "{}-{}-{}-{}.jsonl",
            prefix,
            std::process::id(),
            counter,
            unique
        ));
        path
    }

    #[test]
    fn parse_codex_session_file_derives_delta_from_cumulative_usage() {
        let path = temp_jsonl_path("codex-parser");

        let content = r#"{"timestamp":"2026-07-07T10:58:17.474Z","type":"session_meta","payload":{"session_id":"session-1","cwd":"/tmp/project","cli_version":"0.142.5","model":"gpt-5.5"}}
{"timestamp":"2026-07-07T10:58:26.197Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":10,"reasoning_output_tokens":4,"total_tokens":110},"last_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":10,"reasoning_output_tokens":4,"total_tokens":110},"model_context_window":258400}}}
{"timestamp":"2026-07-07T10:59:26.197Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":10,"reasoning_output_tokens":4,"total_tokens":110},"last_token_usage":{"input_tokens":0,"cached_input_tokens":0,"output_tokens":0,"reasoning_output_tokens":0,"total_tokens":19347},"model_context_window":258400}}}
{"timestamp":"2026-07-07T11:00:26.197Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":130,"cached_input_tokens":30,"output_tokens":15,"reasoning_output_tokens":7,"total_tokens":145},"last_token_usage":{"input_tokens":30,"cached_input_tokens":10,"output_tokens":5,"reasoning_output_tokens":3,"total_tokens":35},"model_context_window":258400}}}
"#;

        fs::write(&path, content).unwrap();
        let entries = parse_codex_session_file(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(entries.len(), 3);

        let first = entries[0].delta_tokens.as_ref().unwrap();
        assert_eq!(first.input, 80);
        assert_eq!(first.cache_read, Some(20));
        assert_eq!(first.output, 10);
        assert_eq!(first.reasoning, Some(4));
        assert_eq!(first.total, 110);

        let anomalous = entries[1].delta_tokens.as_ref().unwrap();
        assert_eq!(anomalous.input, 0);
        assert_eq!(anomalous.cache_read, Some(0));
        assert_eq!(anomalous.output, 0);
        assert_eq!(anomalous.reasoning, Some(0));
        assert_eq!(anomalous.total, 0);

        let third = entries[2].delta_tokens.as_ref().unwrap();
        assert_eq!(third.input, 20);
        assert_eq!(third.cache_read, Some(10));
        assert_eq!(third.output, 5);
        assert_eq!(third.reasoning, Some(3));
        assert_eq!(third.total, 35);

        let total = entries
            .iter()
            .map(|entry| entry.delta_tokens.as_ref().unwrap().total)
            .sum::<u64>();
        assert_eq!(total, 145);
    }

    #[test]
    fn parse_codex_session_file_ignores_repeats_and_handles_resets() {
        let path = temp_jsonl_path("codex-parser");

        let content = r#"{"timestamp":"2026-06-17T13:50:00.000Z","type":"session_meta","payload":{"session_id":"session-2","cwd":"/tmp/project","cli_version":"0.142.5","model":"gpt-5.5"}}
{"timestamp":"2026-06-17T13:50:51.243Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1000,"cached_input_tokens":200,"output_tokens":100,"reasoning_output_tokens":40,"total_tokens":1100},"last_token_usage":{"input_tokens":1000,"cached_input_tokens":200,"output_tokens":100,"reasoning_output_tokens":40,"total_tokens":1100},"model_context_window":121600}}}
{"timestamp":"2026-06-17T13:50:54.339Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1000,"cached_input_tokens":200,"output_tokens":100,"reasoning_output_tokens":40,"total_tokens":1100},"last_token_usage":{"input_tokens":1000,"cached_input_tokens":200,"output_tokens":100,"reasoning_output_tokens":40,"total_tokens":1100},"model_context_window":121600}}}
{"timestamp":"2026-06-17T13:53:01.169Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":0,"cached_input_tokens":0,"output_tokens":0,"reasoning_output_tokens":0,"total_tokens":121600},"last_token_usage":{"input_tokens":0,"cached_input_tokens":0,"output_tokens":0,"reasoning_output_tokens":0,"total_tokens":0},"model_context_window":121600}}}
{"timestamp":"2026-06-17T14:43:08.185Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":200,"cached_input_tokens":50,"output_tokens":20,"reasoning_output_tokens":8,"total_tokens":121820},"last_token_usage":{"input_tokens":200,"cached_input_tokens":50,"output_tokens":20,"reasoning_output_tokens":8,"total_tokens":220},"model_context_window":258400}}}
"#;

        fs::write(&path, content).unwrap();
        let entries = parse_codex_session_file(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].delta_tokens.as_ref().unwrap().total, 1100);
        assert_eq!(entries[1].delta_tokens.as_ref().unwrap().total, 0);
        assert_eq!(entries[2].delta_tokens.as_ref().unwrap().total, 0);

        let after_reset = entries[3].delta_tokens.as_ref().unwrap();
        assert_eq!(after_reset.input, 150);
        assert_eq!(after_reset.cache_read, Some(50));
        assert_eq!(after_reset.output, 20);
        assert_eq!(after_reset.reasoning, Some(8));
        assert_eq!(after_reset.total, 220);
    }

    #[test]
    fn sync_codex_usage_logs_writes_recomputed_delta_totals() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_codex_dir = std::env::var("CODEX_DIR").ok();
        let mut codex_dir = std::env::temp_dir();
        let unique = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        codex_dir.push(format!("codex-sync-{}-{}", std::process::id(), unique));

        let sessions_dir = codex_dir.join("sessions/2026/07/07");
        fs::create_dir_all(&sessions_dir).unwrap();
        let session_path = sessions_dir.join("rollout-2026-07-07T10-58-17-session-sync.jsonl");

        let content = r#"{"timestamp":"2026-07-07T10:58:17.474Z","type":"session_meta","payload":{"session_id":"session-sync","cwd":"/tmp/project","cli_version":"0.142.5","model":"gpt-5.5"}}
{"timestamp":"2026-07-07T10:58:26.197Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":10,"reasoning_output_tokens":4,"total_tokens":110},"last_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":10,"reasoning_output_tokens":4,"total_tokens":110},"model_context_window":258400}}}
{"timestamp":"2026-07-07T10:59:26.197Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":10,"reasoning_output_tokens":4,"total_tokens":110},"last_token_usage":{"input_tokens":0,"cached_input_tokens":0,"output_tokens":0,"reasoning_output_tokens":0,"total_tokens":19347},"model_context_window":258400}}}
"#;

        fs::write(&session_path, content).unwrap();
        std::env::set_var("CODEX_DIR", &codex_dir);

        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        sync_codex_usage_logs(&mut conn).unwrap();

        let total: u64 = conn
            .query_row(
                "SELECT SUM(delta_total) FROM usage_entries WHERE assistant_type = 'codex' AND session_id = 'session-sync'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(total, 110);

        if let Some(value) = old_codex_dir {
            std::env::set_var("CODEX_DIR", value);
        } else {
            std::env::remove_var("CODEX_DIR");
        }
        let _ = fs::remove_dir_all(&codex_dir);
    }

    #[test]
    fn parse_claude_session_file_deduplicates_request_usage() {
        let path = temp_jsonl_path("claude-parser");

        let content = r#"{"type":"user","sessionId":"session-1","cwd":"/tmp/project","version":"2.1.201","timestamp":"2026-07-04T19:28:48.190Z","uuid":"u1","message":{"role":"user","content":"Build the report"}}
{"type":"assistant","sessionId":"session-1","cwd":"/tmp/project","version":"2.1.201","timestamp":"2026-07-04T19:28:51.753Z","uuid":"a1","requestId":"req_1","message":{"id":"msg_1","role":"assistant","model":"claude-haiku-4-5-20251001","content":[{"type":"thinking","thinking":"working"}],"usage":{"input_tokens":10,"cache_creation_input_tokens":3,"cache_read_input_tokens":7,"output_tokens":5}}}
{"type":"assistant","sessionId":"session-1","cwd":"/tmp/project","version":"2.1.201","timestamp":"2026-07-04T19:28:51.948Z","uuid":"a2","requestId":"req_1","message":{"id":"msg_1","role":"assistant","model":"claude-haiku-4-5-20251001","content":[{"type":"text","text":"Done"}],"usage":{"input_tokens":10,"cache_creation_input_tokens":3,"cache_read_input_tokens":7,"output_tokens":5}}}
"#;

        fs::write(&path, content).unwrap();
        let entries = parse_claude_session_file(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.session_id, "session-1");
        assert_eq!(entry.session_name.as_deref(), Some("Build the report"));
        assert_eq!(entry.cwd.as_deref(), Some("/tmp/project"));
        assert_eq!(entry.version.as_deref(), Some("2.1.201"));
        assert_eq!(entry.model.as_deref(), Some("claude-haiku-4-5-20251001"));

        let tokens = entry.tokens.as_ref().unwrap();
        assert_eq!(tokens.input, 13);
        assert_eq!(tokens.cache_write, Some(3));
        assert_eq!(tokens.cache_read, Some(7));
        assert_eq!(tokens.output, 5);
        assert_eq!(tokens.total, 25);
    }
}
