use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::collections::HashMap;
use std::time::SystemTime;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

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

// Codex helper structs
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TokenCountUsage {
    input_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
    reasoning_output_tokens: u64,
    total_tokens: u64,
}

struct ParsedTurnData {
    turn_no: u32,
    timestamp: String,
    model: Option<String>,
    cwd: Option<String>,
    duration_ms: Option<u64>,
    total_token_usage: Option<TokenCountUsage>,
    reasoning_effort: Option<String>,
}

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

/// Get connection to centralized SQLite DB
pub fn get_db_conn() -> Result<Connection, String> {
    let dir = get_insights_dir();
    let db_path = dir.join("token_usage_insights.db");
    
    // Automatically move old centralized database if it exists in the legacy folder
    if !db_path.exists() {
        if let Some(home) = dirs::home_dir() {
            let old_unified_db = home.join(".gemini/antigravity-cli/token_usage_insights.db");
            if old_unified_db.exists() {
                println!("🔄 偵測到存在於舊位置的統一資料庫，正在移動至新位置：{:?} -> {:?}", old_unified_db, db_path);
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
            assistant_type TEXT NOT NULL, -- 'antigravity', 'copilot', 'codex'
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
            tokens_reasoning INTEGER,
            tokens_total INTEGER,
            
            -- Delta Token Statistics
            delta_input INTEGER,
            delta_output INTEGER,
            delta_cache_read INTEGER,
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
    ).map_err(|e| format!("建立 usage_entries 表失敗: {}", e))?;

    // Ensure reasoning_effort column is present in case database already exists
    let _ = conn.execute("ALTER TABLE usage_entries ADD COLUMN reasoning_effort TEXT", []);

    // Unique index on assistant, session, and turn
    conn.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS uidx_assistant_session_turn 
         ON usage_entries(assistant_type, session_id, turn_no)",
        [],
    ).map_err(|e| format!("建立唯一索引 uidx_assistant_session_turn 失敗: {}", e))?;

    // Indexes for performance
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_usage_date ON usage_entries(date)",
        [],
    ).map_err(|e| format!("建立日期索引 idx_usage_date 失敗: {}", e))?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_assistant_type ON usage_entries(assistant_type)",
        [],
    ).map_err(|e| format!("建立助理類型索引 idx_assistant_type 失敗: {}", e))?;

    // Sync state tracking table
    conn.execute(
        "CREATE TABLE IF NOT EXISTS sync_state (
            filename TEXT PRIMARY KEY,
            last_synced_size INTEGER NOT NULL,
            last_synced_time INTEGER NOT NULL
        )",
        [],
    ).map_err(|e| format!("建立 sync_state 表失敗: {}", e))?;

    Ok(())
}

/// Helper to parse usage entries from jsonl files (Antigravity & Copilot)
fn parse_usage_entries(content: &str) -> Vec<UsageEntry> {
    let mut entries = Vec::new();
    let mut current_obj = String::new();
    let mut in_object = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if !in_object && trimmed.starts_with('{') && trimmed.ends_with('}') {
            if let Ok(entry) = serde_json::from_str::<UsageEntry>(trimmed) {
                entries.push(entry);
                continue;
            }
        }

        if !in_object {
            if trimmed.starts_with('{') {
                in_object = true;
                current_obj.clear();
                current_obj.push_str(line);
                current_obj.push('\n');
            }
        } else {
            current_obj.push_str(line);
            current_obj.push('\n');

            let is_root_close = line.trim_end() == "}" && !line.starts_with(' ') && !line.starts_with('\t');
            if is_root_close {
                if let Ok(entry) = serde_json::from_str::<UsageEntry>(&current_obj) {
                    entries.push(entry);
                }
                in_object = false;
                current_obj.clear();
            }
        }
    }
    entries
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
fn sync_hook_usage_logs(conn: &Connection, assistant_type: &str, base_dir: &Path) -> Result<(), String> {
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
            let _ = conn.execute("DELETE FROM sync_state WHERE filename LIKE 'antigravity:%'", []);
            let _ = conn.execute("DELETE FROM usage_entries WHERE assistant_type = 'antigravity'", []);
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

        let mut file = File::open(&filepath).map_err(|e| format!("無法開啟日誌檔 {}: {}", filename, e))?;
        let metadata = file.metadata().map_err(|e| format!("無法取得檔案資訊 {}: {}", filename, e))?;
        let current_size = metadata.len();

        let start_pos = if current_size < last_synced_size { 0 } else { last_synced_size };

        if current_size > start_pos {
            file.seek(SeekFrom::Start(start_pos)).map_err(|e| format!("Seek 失敗 {}: {}", filename, e))?;
            let mut buffer = Vec::new();
            file.read_to_end(&mut buffer).map_err(|e| format!("讀取檔案失敗 {}: {}", filename, e))?;

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

                conn.execute("BEGIN TRANSACTION", []).map_err(|e| format!("Transaction BEGIN 失敗: {}", e))?;

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

                    let insert_res = conn.execute(
                        "INSERT OR IGNORE INTO usage_entries (
                            assistant_type, timestamp, date, session_id, session_name, transcript_path, cwd, version, turn_no, model, model_id,
                            tokens_input, tokens_output, tokens_cache_read, tokens_reasoning, tokens_total,
                            delta_input, delta_output, delta_cache_read, delta_reasoning, delta_total,
                            duration_ms, premium_requests
                        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
                            tokens.and_then(|t| t.reasoning.map(|v| v as i64)),
                            tokens.map(|t| t.total as i64),
                            delta.map(|t| t.input as i64),
                            delta.map(|t| t.output as i64),
                            delta.and_then(|t| t.cache_read.map(|v| v as i64)),
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

                    let update_state_res = conn.execute(
                        "INSERT OR REPLACE INTO sync_state (filename, last_synced_size, last_synced_time) VALUES (?, ?, ?)",
                        params![state_key, (start_pos + read_len as u64) as i64, now],
                    );

                    if update_state_res.is_ok() {
                        if let Err(e) = conn.execute("COMMIT TRANSACTION", []) {
                            eprintln!("Transaction COMMIT 失敗: {}", e);
                            let _ = conn.execute("ROLLBACK TRANSACTION", []);
                        }
                    } else {
                        let _ = conn.execute("ROLLBACK TRANSACTION", []);
                    }
                } else {
                    let _ = conn.execute("ROLLBACK TRANSACTION", []);
                }
            }
        }
    }

    Ok(())
}

/// Codex sync helper: parse filename
fn parse_codex_filename(filename: &str) -> Option<(String, String, String)> {
    if !filename.starts_with("rollout-") || !filename.ends_with(".jsonl") {
        return None;
    }
    let core = filename.strip_prefix("rollout-")?.strip_suffix(".jsonl")?;
    if core.len() < 20 {
        return None;
    }
    let date_part = &core[0..10]; // YYYY-MM-DD
    let time_part = &core[11..19]; // HH-mm-ss
    let uuid_part = &core[20..]; // UUID
    
    let date = date_part.to_string();
    let time_formatted = time_part.replace('-', ":");
    let session_name = format!("Rollout {} {}", date, time_formatted);
    let session_id = uuid_part.to_string();
    
    Some((session_id, date, session_name))
}

/// Codex sync helper: recursively find session rollout jsonl files
fn find_codex_session_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(find_codex_session_files(&path));
            } else if path.is_file() {
                if let Some(filename) = path.file_name().and_then(|f| f.to_str()) {
                    if filename.starts_with("rollout-") && filename.ends_with(".jsonl") {
                        files.push(path);
                    }
                }
            }
        }
    }
    files
}

/// Codex sync helper: parse rollout session events
fn parse_codex_session_file(
    filepath: &Path,
    session_id: &str,
    session_name: &str,
    _session_date: &str,
) -> Result<Vec<UsageEntry>, String> {
    let file = File::open(filepath).map_err(|e| format!("無法開啟檔案: {}", e))?;
    let reader = BufReader::new(file);

    let mut cli_version = None;
    let mut session_meta_cwd = None;
    let mut parent_session_id = None;
    let mut agent_nickname = None;
    let mut agent_role = None;
    let mut first_user_message = None;

    let mut seen_turn_ids = Vec::new();
    let mut active_turn_id: Option<String> = None;
    let mut turn_data_map: HashMap<String, ParsedTurnData> = HashMap::new();

    for line_res in reader.lines() {
        let line = match line_res {
            Ok(l) => l,
            Err(_) => continue,
        };
        let event: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let event_type = event.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let timestamp = event.get("timestamp").and_then(|t| t.as_str()).unwrap_or("").to_string();
        let payload = event.get("payload");

        if event_type == "event_msg" && first_user_message.is_none() {
            if let Some(p) = payload {
                let is_user_msg = if let Some(t) = p.get("type") {
                    if let Some(s) = t.as_str() {
                        s == "user_message"
                    } else if let Some(arr) = t.as_array() {
                        arr.first().and_then(|v| v.as_str()) == Some("user_message")
                    } else {
                        false
                    }
                } else {
                    false
                };
                if is_user_msg {
                    if let Some(msg) = p.get("message").and_then(|m| m.as_str()) {
                        let cleaned = msg.replace('\r', "").replace('\n', " ");
                        let truncated = cleaned.chars().take(100).collect::<String>();
                        first_user_message = Some(truncated);
                    }
                }
            }
        }

        if event_type == "session_meta" {
            if let Some(p) = payload {
                if cli_version.is_none() {
                    cli_version = p.get("cli_version").and_then(|v| v.as_str()).map(|s| s.to_string());
                }
                if session_meta_cwd.is_none() {
                    session_meta_cwd = p.get("cwd").and_then(|c| c.as_str()).map(|s| s.to_string());
                }
                let p_sid = p.get("session_id").and_then(|v| v.as_str());
                let p_id = p.get("id").and_then(|v| v.as_str());
                if let (Some(psid), Some(pid)) = (p_sid, p_id) {
                    if psid != pid {
                        parent_session_id = Some(psid.to_string());
                    }
                }
                if agent_nickname.is_none() {
                    agent_nickname = p.get("agent_nickname")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| {
                            p.get("source")
                                .and_then(|s| s.get("subagent"))
                                .and_then(|s| s.get("thread_spawn"))
                                .and_then(|t| t.get("agent_nickname"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string())
                        });
                }
                if agent_role.is_none() {
                    agent_role = p.get("agent_role")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| {
                            p.get("source")
                                .and_then(|s| s.get("subagent"))
                                .and_then(|s| s.get("thread_spawn"))
                                .and_then(|t| t.get("agent_role"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string())
                        });
                }
            }
            continue;
        }

        let mut turn_id = None;
        if event_type == "turn_context" {
            if let Some(p) = payload {
                turn_id = p.get("turn_id").and_then(|id| id.as_str()).map(|s| s.to_string());
            }
        } else if event_type == "event_msg" {
            if let Some(p) = payload {
                turn_id = p.get("turn_id").and_then(|id| id.as_str()).map(|s| s.to_string());
            }
        } else if event_type == "response_item" {
            if let Some(meta) = event.get("metadata") {
                turn_id = meta.get("turn_id").and_then(|id| id.as_str()).map(|s| s.to_string());
            }
            if turn_id.is_none() {
                turn_id = event.get("internal_chat_message_metadata_passthrough")
                    .and_then(|m| m.get("turn_id"))
                    .and_then(|id| id.as_str())
                    .map(|s| s.to_string());
            }
        }

        if let Some(tid) = turn_id {
            active_turn_id = Some(tid.clone());
            if !seen_turn_ids.contains(&tid) {
                seen_turn_ids.push(tid.clone());
                let turn_no = seen_turn_ids.len() as u32;
                turn_data_map.insert(tid.clone(), ParsedTurnData {
                    turn_no,
                    timestamp: timestamp.clone(),
                    model: None,
                    cwd: None,
                    duration_ms: None,
                    total_token_usage: None,
                    reasoning_effort: None,
                });
            }
        }

        if let Some(ref active_tid) = active_turn_id {
            if let Some(td) = turn_data_map.get_mut(active_tid) {
                if event_type == "turn_context" {
                    if let Some(p) = payload {
                        if td.model.is_none() {
                            td.model = p.get("model").and_then(|m| m.as_str()).map(|s| s.to_string());
                        }
                        if td.cwd.is_none() {
                            td.cwd = p.get("cwd").and_then(|c| c.as_str()).map(|s| s.to_string());
                        }
                        if td.reasoning_effort.is_none() {
                            td.reasoning_effort = p.get("effort")
                                .or_else(|| p.get("collaboration_mode").and_then(|cm| cm.get("settings")).and_then(|s| s.get("reasoning_effort")))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                        }
                    }
                } else if event_type == "response_item" {
                    if let Some(p) = payload {
                        if p.get("role").and_then(|r| r.as_str()) == Some("assistant") && td.model.is_none() {
                            td.model = p.get("model").and_then(|m| m.as_str()).map(|s| s.to_string());
                        }
                    }
                } else if event_type == "event_msg" {
                    if let Some(p) = payload {
                        let sub_type = p.get("type").and_then(|t| t.as_str()).unwrap_or("");
                        if sub_type == "token_count" {
                            if let Some(info) = p.get("info") {
                                if let Some(usage_val) = info.get("total_token_usage") {
                                    if let Ok(usage) = serde_json::from_value::<TokenCountUsage>(usage_val.clone()) {
                                        td.total_token_usage = Some(usage);
                                    }
                                }
                            }
                        } else if sub_type == "task_complete" || sub_type == "turn_aborted" {
                            if let Some(dur) = p.get("duration_ms").and_then(|d| d.as_u64()) {
                                td.duration_ms = Some(dur);
                            }
                        }
                    }
                }
            }
        }
    }

    let mut results = Vec::new();
    let mut cumulative_usage = TokenCountUsage {
        input_tokens: 0,
        cached_input_tokens: 0,
        output_tokens: 0,
        reasoning_output_tokens: 0,
        total_tokens: 0,
    };

    let mut prev_cli_input = 0u64;
    let mut prev_cli_cached = 0u64;
    let mut prev_cli_output = 0u64;
    let mut prev_cli_reasoning = 0u64;
    let mut prev_cli_total = 0u64;

    for tid in &seen_turn_ids {
        if let Some(td) = turn_data_map.get(tid) {
            if let Some(ref usage) = td.total_token_usage {
                cumulative_usage = usage.clone();
            }

            let cli_input = cumulative_usage.input_tokens.saturating_sub(cumulative_usage.cached_input_tokens);
            let cli_cached = cumulative_usage.cached_input_tokens;
            let cli_output = cumulative_usage.output_tokens;
            let cli_reasoning = cumulative_usage.reasoning_output_tokens;
            let cli_total = cli_input + cli_output;

            let delta_input = cli_input.saturating_sub(prev_cli_input);
            let delta_cached = cli_cached.saturating_sub(prev_cli_cached);
            let delta_output = cli_output.saturating_sub(prev_cli_output);
            let delta_reasoning = cli_reasoning.saturating_sub(prev_cli_reasoning);
            let delta_total = cli_total.saturating_sub(prev_cli_total);

            prev_cli_input = cli_input;
            prev_cli_cached = cli_cached;
            prev_cli_output = cli_output;
            prev_cli_reasoning = cli_reasoning;
            prev_cli_total = cli_total;

            let turn_tokens = TokenStats {
                input: cli_input,
                output: cli_output,
                cache_read: Some(cli_cached),
                cache_write: None,
                reasoning: Some(cli_reasoning),
                total: cli_total,
            };

            let turn_delta = TokenStats {
                input: delta_input,
                output: delta_output,
                cache_read: Some(delta_cached),
                cache_write: None,
                reasoning: Some(delta_reasoning),
                total: delta_total,
            };

            let cost = if td.duration_ms.is_some() {
                Some(CostStats {
                    total_api_duration_ms: td.duration_ms.map(|d| d as f64),
                    total_duration_ms: None,
                    total_premium_requests: Some(0.0),
                })
            } else {
                None
            };

            let actual_session_name = first_user_message.clone().unwrap_or_else(|| session_name.to_string());
            results.push(UsageEntry {
                timestamp: td.timestamp.clone(),
                session_id: session_id.to_string(),
                session_name: Some(actual_session_name),
                transcript_path: Some(filepath.to_string_lossy().into_owned()),
                cwd: td.cwd.clone().or_else(|| session_meta_cwd.clone()),
                version: cli_version.clone(),
                turn_no: td.turn_no,
                model: td.model.clone(),
                model_id: td.model.clone(),
                tokens: Some(turn_tokens),
                delta_tokens: Some(turn_delta),
                context: None,
                cost,
                parent_session_id: parent_session_id.clone(),
                agent_nickname: agent_nickname.clone(),
                agent_role: agent_role.clone(),
                reasoning_effort: td.reasoning_effort.clone(),
            });
        }
    }

    Ok(results)
}

/// Sync usage logs for directory-based assistant (Codex)
fn sync_codex_usage_logs(conn: &Connection) -> Result<(), String> {
    // Perform migration if we haven't tracked it yet to update session names with user messages
    let migration_done: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_state WHERE filename = 'migration:codex_user_message_names')",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);

    if !migration_done {
        let _ = conn.execute("DELETE FROM sync_state WHERE filename LIKE 'codex:%'", []);
        let _ = conn.execute("DELETE FROM usage_entries WHERE assistant_type = 'codex'", []);
        let _ = conn.execute(
            "INSERT OR REPLACE INTO sync_state (filename, last_synced_size, last_synced_time) VALUES ('migration:codex_user_message_names', 1, 0)",
            [],
        );
    }

    let codex_dir = get_codex_dir();
    let sessions_dir = codex_dir.join("sessions");
    if !sessions_dir.exists() {
        return Ok(());
    }

    let files = find_codex_session_files(&sessions_dir);

    for filepath in files {
        let filename = match filepath.file_name().and_then(|f| f.to_str()) {
            Some(name) => name.to_string(),
            None => continue,
        };

        let (session_id, session_date, session_name) = match parse_codex_filename(&filename) {
            Some(res) => res,
            None => continue,
        };

        let state_key = format!("codex:{}", filename);

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
            let parsed_entries = match parse_codex_session_file(&filepath, &session_id, &session_name, &session_date) {
                Ok(entries) => entries,
                Err(e) => {
                    eprintln!("解析 Codex 會話檔案 {} 失敗: {}", filename, e);
                    continue;
                }
            };

            conn.execute("BEGIN TRANSACTION", []).map_err(|e| format!("Transaction BEGIN 失敗: {}", e))?;

            // First delete old entries for this session
            let delete_res = conn.execute(
                "DELETE FROM usage_entries WHERE assistant_type = 'codex' AND session_id = ?",
                params![session_id],
            );

            if let Err(e) = delete_res {
                eprintln!("清空舊 Codex Session 資料失敗: {}", e);
                let _ = conn.execute("ROLLBACK TRANSACTION", []);
                continue;
            }

            let mut success = true;
            for entry in &parsed_entries {
                let tokens = entry.tokens.as_ref();
                let delta = entry.delta_tokens.as_ref();
                let cost = entry.cost.as_ref();

                let insert_res = conn.execute(
                    "INSERT INTO usage_entries (
                        assistant_type, timestamp, date, session_id, session_name, transcript_path, cwd, version, turn_no, model, model_id,
                        tokens_input, tokens_output, tokens_cache_read, tokens_reasoning, tokens_total,
                        delta_input, delta_output, delta_cache_read, delta_reasoning, delta_total,
                        duration_ms, premium_requests, parent_session_id, agent_nickname, agent_role, reasoning_effort
                    ) VALUES ('codex', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    params![
                        entry.timestamp,
                        session_date,
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
                        tokens.and_then(|t| t.reasoning.map(|v| v as i64)),
                        tokens.map(|t| t.total as i64),
                        delta.map(|t| t.input as i64),
                        delta.map(|t| t.output as i64),
                        delta.and_then(|t| t.cache_read.map(|v| v as i64)),
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
                    eprintln!("寫入 Codex 資料庫失敗 (turn_no {}): {}", entry.turn_no, e);
                    success = false;
                    break;
                }
            }

            if success {
                let now = SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64;

                let update_state_res = conn.execute(
                    "INSERT OR REPLACE INTO sync_state (filename, last_synced_size, last_synced_time) VALUES (?, ?, ?)",
                    params![state_key, current_size as i64, now],
                );

                if update_state_res.is_ok() {
                    if let Err(e) = conn.execute("COMMIT TRANSACTION", []) {
                        eprintln!("Transaction COMMIT 失敗: {}", e);
                        let _ = conn.execute("ROLLBACK TRANSACTION", []);
                    }
                } else {
                    let _ = conn.execute("ROLLBACK TRANSACTION", []);
                }
            } else {
                let _ = conn.execute("ROLLBACK TRANSACTION", []);
            }
        }
    }

    Ok(())
}

/// Unified sync function triggering sync for all three assistants
pub fn sync_usage_logs(conn: &Connection) -> Result<(), String> {
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
        eprintln!("❌ 同步 Codex 失敗: {}", e);
    }

    Ok(())
}

/// Migrate data from legacy standalone databases into the centralized DB
pub fn migrate_old_databases(dest_conn: &Connection) -> Result<(), String> {
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
                let backup_path = home.join(".gemini/antigravity-cli/antigravity_cli_token_insights.db.bak");
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

fn migrate_records(src_conn: &Connection, dest_conn: &Connection, assistant: &str) -> Result<(), rusqlite::Error> {
    let table_exists: bool = src_conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='usage_entries'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0) > 0;

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

    dest_conn.execute("BEGIN TRANSACTION", [])?;

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

        let insert_res = dest_conn.execute(
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
            eprintln!("遷移單筆紀錄失敗 ({} - session_id: {}, turn_no: {}): {}", assistant, session_id, turn_no, e);
        }
    }

    // Migrate sync_state
    let sync_table_exists: bool = src_conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='sync_state'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0) > 0;

    if sync_table_exists {
        if let Ok(mut sync_stmt) = src_conn.prepare("SELECT filename, last_synced_size, last_synced_time FROM sync_state") {
            if let Ok(mut sync_rows) = sync_stmt.query([]) {
                while let Ok(Some(row)) = sync_rows.next() {
                    let filename = row.get::<_, String>(0)?;
                    let size = row.get::<_, i64>(1)?;
                    let time = row.get::<_, i64>(2)?;
                    let state_key = format!("{}:{}", assistant, filename);
                    let _ = dest_conn.execute(
                        "INSERT OR REPLACE INTO sync_state (filename, last_synced_size, last_synced_time) VALUES (?, ?, ?)",
                        params![state_key, size, time],
                    );
                }
            }
        }
    }

    let _ = dest_conn.execute("COMMIT TRANSACTION", []);
    Ok(())
}

