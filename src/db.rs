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
    #[serde(default)]
    pub source_kind: Option<String>,
    /// Source directory key (hex-encoded canonical path) for Copilot App rows.
    /// `None` for all other collectors. Used to isolate sessions from different
    /// COPILOT_APP_DIR values that may share the same session_id.
    #[serde(default)]
    pub source_dir_key: Option<String>,

    // Codex-specific / Extended fields
    pub parent_session_id: Option<String>,
    pub agent_nickname: Option<String>,
    pub agent_role: Option<String>,
    pub reasoning_effort: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct UsageDayExportRecord {
    #[serde(flatten)]
    pub entry: UsageEntry,
    pub import_source_id: Option<String>,
}

#[derive(Serialize, Debug)]
pub struct UsageDayImportSummary {
    pub date: String,
    pub total: usize,
    pub imported: usize,
    pub skipped_duplicates: usize,
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

const CODEX_PARSER_MIGRATION_KEY: &str = "migration:codex_session_identity_v6";
const COPILOT_SOURCE_KIND_MIGRATION_KEY: &str = "migration:copilot_source_kind_v1";

/// Source kind written for usage entries originating from the Copilot App
/// (Tauri desktop application). Distinguishes them from `copilot-cli` and
/// VS Code Copilot Chat sessions within the shared `copilot` assistant type.
const COPILOT_APP_SOURCE_KIND: &str = "copilot-app";

/// `sync_state.filename` key prefix storing the maximum
/// The full cursor filename is
/// `sync:copilot_app:cursor:<hex(canonical_source_path)>::<created_at>::<id>`.
/// The cursor is scoped per source directory and switching
/// `COPILOT_APP_DIR`/`COPILOT_DIR` starts a fresh cursor instead of reusing the
/// previous directory's.
const COPILOT_APP_CURSOR_PREFIX: &str = "sync:copilot_app:cursor:";
const VSCODE_EMPTY_SESSION_MIGRATION_KEY: &str = "migration:vscode_empty_sessions_v1";
const COPILOT_CACHED_INPUT_MIGRATION_KEY: &str = "migration:copilot_cached_input_v1";
const SESSION_NAME_SELECTION_MIGRATION_KEY: &str = "migration:session_name_selection_v1";

#[derive(Default)]
enum InitialUserPromptState {
    #[default]
    Waiting,
    Collecting,
    WaitingForFallback,
    Complete,
}

#[derive(Default)]
pub(crate) struct InitialUserPromptSelector {
    state: InitialUserPromptState,
    name: Option<String>,
}

impl InitialUserPromptSelector {
    pub(crate) fn observe_user_prompt(&mut self, prompt: &str) {
        let normalized = prompt.trim().replace('\r', "").replace('\n', " ");
        if normalized.is_empty() || matches!(self.state, InitialUserPromptState::Complete) {
            return;
        }

        let name = normalized.chars().take(100).collect();
        match self.state {
            InitialUserPromptState::Waiting => {
                self.name = Some(name);
                self.state = InitialUserPromptState::Collecting;
            }
            InitialUserPromptState::Collecting => {
                self.name = Some(name);
            }
            InitialUserPromptState::WaitingForFallback => {
                self.name = Some(name);
                self.state = InitialUserPromptState::Complete;
            }
            InitialUserPromptState::Complete => {}
        }
    }

    pub(crate) fn observe_non_user_message(&mut self) {
        self.state = match self.state {
            InitialUserPromptState::Waiting => InitialUserPromptState::WaitingForFallback,
            InitialUserPromptState::Collecting => InitialUserPromptState::Complete,
            InitialUserPromptState::WaitingForFallback => {
                InitialUserPromptState::WaitingForFallback
            }
            InitialUserPromptState::Complete => InitialUserPromptState::Complete,
        };
    }

    fn is_complete(&self) -> bool {
        matches!(self.state, InitialUserPromptState::Complete)
    }

    fn selected_name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub(crate) fn into_name(self) -> Option<String> {
        self.name
    }
}

fn hash_fnv1a_64(input: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in input.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn normalize_import_source_id(raw: Option<&str>) -> Option<String> {
    let value = raw?.trim();
    if value.is_empty() {
        return None;
    }
    Some(value.to_string())
}

fn build_import_token_signature(tokens: &Option<TokenStats>) -> String {
    if let Some(t) = tokens {
        format!(
            "{}|{}|{}|{}|{}|{}",
            t.input,
            t.output,
            t.cache_read.unwrap_or(0),
            t.cache_write.unwrap_or(0),
            t.reasoning.unwrap_or(0),
            t.total
        )
    } else {
        "null".to_string()
    }
}

fn build_usage_entry_import_source_id(assistant: &str, date: &str, entry: &UsageEntry) -> String {
    let signature = format!(
        "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
        assistant,
        date,
        entry.timestamp,
        entry.session_id,
        entry.turn_no,
        entry.model.clone().unwrap_or_default(),
        entry.model_id.clone().unwrap_or_default(),
        entry.version.clone().unwrap_or_default(),
        entry.cwd.clone().unwrap_or_default(),
        entry.transcript_path.clone().unwrap_or_default(),
        entry.parent_session_id.clone().unwrap_or_default(),
        entry.agent_nickname.clone().unwrap_or_default(),
        entry.agent_role.clone().unwrap_or_default(),
        build_import_token_signature(&entry.tokens),
        build_import_token_signature(&entry.delta_tokens)
    );
    format!("{:016x}", hash_fnv1a_64(&signature))
}

/// Directory resolution helpers
pub fn get_insights_dir() -> PathBuf {
    if let Some(path) = crate::paths::env_path("INSIGHTS_DIR") {
        return path;
    }

    #[cfg(windows)]
    if let Some(data_dir) = dirs::data_local_dir() {
        return data_dir.join("TokenUsageInsights");
    }

    if let Some(home) = dirs::home_dir() {
        return home.join(".token-usage-insights");
    }
    PathBuf::from(".")
}

pub fn get_antigravity_dir() -> PathBuf {
    if let Some(path) = crate::paths::env_path("ANTIGRAVITY_DIR") {
        return path;
    }
    dirs::home_dir()
        .map(|h| h.join(".gemini").join("antigravity-cli"))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn get_copilot_dir() -> PathBuf {
    if let Some(path) = crate::paths::env_path("COPILOT_DIR") {
        return path;
    }
    dirs::home_dir()
        .map(|h| h.join(".copilot"))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn get_codex_dir() -> PathBuf {
    if let Some(path) = crate::paths::env_path("CODEX_DIR") {
        return path;
    }
    dirs::home_dir()
        .map(|h| h.join(".codex"))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn get_claude_dir() -> PathBuf {
    if let Some(path) = crate::paths::env_path("CLAUDE_DIR") {
        return path;
    }
    dirs::home_dir()
        .map(|h| h.join(".claude"))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn get_cursor_dir() -> PathBuf {
    if let Some(path) = crate::paths::env_path("CURSOR_DIR") {
        return path;
    }
    dirs::home_dir()
        .map(|h| h.join(".cursor"))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn move_file_with_copy_fallback(source: &Path, destination: &Path) -> Result<(), String> {
    if let Err(rename_error) = fs::rename(source, destination) {
        let copied = fs::copy(source, destination).map_err(|copy_error| {
            format!("重新命名失敗 ({rename_error})，跨磁碟複製也失敗: {copy_error}")
        })?;
        let source_size = fs::metadata(source)
            .map_err(|error| format!("讀取來源資料庫大小失敗: {error}"))?
            .len();
        if copied != source_size {
            let _ = fs::remove_file(destination);
            return Err(format!(
                "跨磁碟複製大小不符: source={source_size}, destination={copied}"
            ));
        }
        File::open(destination)
            .and_then(|file| file.sync_all())
            .map_err(|error| format!("同步目標資料庫失敗: {error}"))?;
        fs::remove_file(source).map_err(|error| format!("移除舊資料庫失敗: {error}"))?;
    }
    Ok(())
}

fn legacy_unified_database_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = dirs::home_dir() {
        #[cfg(windows)]
        paths.push(
            home.join(".token-usage-insights")
                .join("token_usage_insights.db"),
        );
        paths.push(
            home.join(".gemini")
                .join("antigravity-cli")
                .join("token_usage_insights.db"),
        );
    }
    paths
}

/// Get connection to centralized SQLite DB
pub fn get_db_conn() -> Result<Connection, String> {
    let dir = get_insights_dir();
    fs::create_dir_all(&dir).map_err(|error| format!("無法建立資料庫目錄 {:?}: {}", dir, error))?;
    let db_path = dir.join("token_usage_insights.db");

    // Automatically move old centralized database if it exists in the legacy folder
    if !db_path.exists() {
        if let Some(old_unified_db) = legacy_unified_database_paths()
            .into_iter()
            .find(|path| path != &db_path && path.exists())
        {
            println!(
                "🔄 偵測到存在於舊位置的統一資料庫，正在移動至新位置：{:?} -> {:?}",
                old_unified_db, db_path
            );
            if let Err(e) = move_file_with_copy_fallback(&old_unified_db, &db_path) {
                eprintln!("⚠️ 移動舊統一資料庫失敗: {}", e);
            } else {
                println!("✅ 統一資料庫移動完成！");
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
            assistant_type TEXT NOT NULL, -- 'antigravity', 'copilot', 'codex', 'claude', 'cursor'
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
            source_kind TEXT NOT NULL DEFAULT 'legacy',

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
    let _ = conn.execute(
        "ALTER TABLE usage_entries ADD COLUMN import_source_id TEXT",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE usage_entries ADD COLUMN source_kind TEXT NOT NULL DEFAULT 'legacy'",
        [],
    );
    // source_dir_key isolates Copilot App rows by source directory so that
    // identical (session_id, turn_no) from different COPILOT_APP_DIR values
    // do not REPLACE each other via the unique index. NULL for all other
    // collectors.
    let _ = conn.execute(
        "ALTER TABLE usage_entries ADD COLUMN source_dir_key TEXT",
        [],
    );

    // Migration: delete legacy copilot-app rows that predate source_dir_key.
    // These rows have source_kind = 'copilot-app', source_dir_key IS NULL, and
    // the old import_source_id format (copilot-app:<session>:<turn>, without
    // the hex source key segment). They cannot be attributed to a specific
    // source directory, so they must be removed to avoid double-counting when
    // the new collector re-syncs the same turns from the actual directory.
    let legacy_deleted: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM usage_entries
             WHERE source_kind = 'copilot-app'
               AND source_dir_key IS NULL
               AND import_source_id LIKE 'copilot-app:%:%'
               AND import_source_id NOT LIKE 'copilot-app:%:%:%'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    if legacy_deleted > 0 {
        let _ = conn.execute(
            "DELETE FROM usage_entries
             WHERE source_kind = 'copilot-app'
               AND source_dir_key IS NULL
               AND import_source_id LIKE 'copilot-app:%:%'
               AND import_source_id NOT LIKE 'copilot-app:%:%:%'",
            [],
        );
        println!(
            "✅ 遷移：清除 {} 筆舊版 Copilot App 資料（無 source_dir_key）",
            legacy_deleted
        );
    }

    // Use two partial unique indexes to preserve original uniqueness semantics
    // for non-copilot-app collectors while isolating copilot-app rows by source
    // directory. A single nullable-column index would treat NULLs as distinct,
    // breaking uniqueness for codex/claude/cursor/copilot-cli/vscode.
    let _ = conn.execute("DROP INDEX IF EXISTS uidx_assistant_session_turn", []);
    let _ = conn.execute(
        "DROP INDEX IF EXISTS uidx_assistant_source_session_turn",
        [],
    );
    let _ = conn.execute(
        "DROP INDEX IF EXISTS uidx_assistant_source_dir_session_turn",
        [],
    );

    // Partial index for collectors without source_dir_key (NULL): preserves
    // the original (assistant_type, source_kind, session_id, turn_no) uniqueness.
    conn.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS uidx_assistant_source_session_turn
         ON usage_entries(assistant_type, source_kind, session_id, turn_no)
         WHERE source_dir_key IS NULL",
        [],
    )
    .map_err(|e| {
        format!(
            "建立唯一索引 uidx_assistant_source_session_turn 失敗: {}",
            e
        )
    })?;

    // Partial index for copilot-app rows (source_dir_key IS NOT NULL): includes
    // source_dir_key so different COPILOT_APP_DIR values are isolated.
    conn.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS uidx_assistant_source_dir_session_turn
         ON usage_entries(assistant_type, source_kind, source_dir_key, session_id, turn_no)
         WHERE source_dir_key IS NOT NULL",
        [],
    )
    .map_err(|e| {
        format!(
            "建立唯一索引 uidx_assistant_source_dir_session_turn 失敗: {}",
            e
        )
    })?;

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

    let _ = conn.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS uidx_assistant_import_source_id ON usage_entries(assistant_type, import_source_id) WHERE import_source_id IS NOT NULL",
        [],
    );

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

    // Before source_kind existed, every Copilot record came from the CLI
    // collector. Classify those historical rows once so the new source-scoped
    // unique index does not duplicate them on the first synchronization.
    let source_kind_migration_done: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_state WHERE filename = ?)",
            params![COPILOT_SOURCE_KIND_MIGRATION_KEY],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if !source_kind_migration_done {
        let _ = conn.execute(
            "UPDATE usage_entries
             SET source_kind = 'copilot-cli'
             WHERE assistant_type = 'copilot' AND source_kind = 'legacy'",
            [],
        );
        let _ = conn.execute(
            "INSERT OR REPLACE INTO sync_state (filename, last_synced_size, last_synced_time)
             VALUES (?, 1, 0)",
            params![COPILOT_SOURCE_KIND_MIGRATION_KEY],
        );
    }

    let empty_vscode_migration_done: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_state WHERE filename = ?)",
            params![VSCODE_EMPTY_SESSION_MIGRATION_KEY],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if !empty_vscode_migration_done {
        conn.execute(
            "DELETE FROM usage_entries
             WHERE assistant_type = 'copilot'
               AND source_kind = 'vscode-chat'
               AND model IS NULL
               AND model_id IS NULL
               AND tokens_input IS NULL
               AND tokens_output IS NULL
               AND tokens_cache_read IS NULL
               AND tokens_cache_write IS NULL
               AND tokens_reasoning IS NULL
               AND tokens_total IS NULL
               AND delta_input IS NULL
               AND delta_output IS NULL
               AND delta_cache_read IS NULL
               AND delta_cache_write IS NULL
               AND delta_reasoning IS NULL
               AND delta_total IS NULL
               AND duration_ms IS NULL
               AND premium_requests IS NULL",
            [],
        )
        .map_err(|error| format!("清除空白 VS Code Copilot 工作階段失敗: {error}"))?;
        conn.execute(
            "INSERT OR REPLACE INTO sync_state (filename, last_synced_size, last_synced_time)
             VALUES (?, 1, 0)",
            params![VSCODE_EMPTY_SESSION_MIGRATION_KEY],
        )
        .map_err(|error| format!("記錄空白 VS Code Copilot 工作階段遷移失敗: {error}"))?;
    }

    let copilot_cached_input_migration_done: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_state WHERE filename = ?)",
            params![COPILOT_CACHED_INPUT_MIGRATION_KEY],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if !copilot_cached_input_migration_done {
        conn.execute(
            "UPDATE usage_entries
             SET tokens_input = CASE
                    WHEN tokens_input IS NOT NULL
                     AND tokens_output IS NOT NULL
                     AND tokens_cache_read > 0
                     AND tokens_input >= tokens_cache_read
                     AND tokens_total = tokens_input + tokens_output
                    THEN tokens_input - tokens_cache_read
                    ELSE tokens_input
                 END,
                 delta_input = CASE
                    WHEN delta_input IS NOT NULL
                     AND delta_output IS NOT NULL
                     AND delta_cache_read > 0
                     AND delta_input >= delta_cache_read
                     AND delta_total = delta_input + delta_output
                    THEN delta_input - delta_cache_read
                    ELSE delta_input
                 END
             WHERE assistant_type = 'copilot'
               AND source_kind = 'copilot-cli'",
            [],
        )
        .map_err(|error| format!("正規化 Copilot CLI 快取輸入失敗: {error}"))?;
        conn.execute(
            "INSERT OR REPLACE INTO sync_state (filename, last_synced_size, last_synced_time)
             VALUES (?, 1, 0)",
            params![COPILOT_CACHED_INPUT_MIGRATION_KEY],
        )
        .map_err(|error| format!("記錄 Copilot CLI 快取輸入遷移失敗: {error}"))?;
    }

    let session_name_migration_done: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_state WHERE filename = ?)",
            params![SESSION_NAME_SELECTION_MIGRATION_KEY],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if !session_name_migration_done {
        conn.execute(
            "DELETE FROM sync_state
             WHERE filename LIKE 'antigravity:%'
                OR filename LIKE 'copilot:%'
                OR filename LIKE 'vscode:%'
                OR filename LIKE 'codex:sessions/%'
                OR filename LIKE 'codex:sessions\\%'
                OR filename LIKE 'claude:%'
                OR filename LIKE 'cursor:%'",
            [],
        )
        .map_err(|error| format!("清除會話名稱同步狀態失敗: {error}"))?;
        conn.execute(
            "INSERT OR REPLACE INTO sync_state (filename, last_synced_size, last_synced_time)
             VALUES (?, 1, 0)",
            params![SESSION_NAME_SELECTION_MIGRATION_KEY],
        )
        .map_err(|error| format!("記錄會話名稱遷移失敗: {error}"))?;
    }

    Ok(())
}

/// Helper to parse usage entries from jsonl files (Antigravity & Copilot)
fn parse_usage_entries(content: &str) -> Vec<UsageEntry> {
    let stream = serde_json::Deserializer::from_str(content).into_iter::<UsageEntry>();
    stream.filter_map(Result::ok).collect()
}

fn separate_copilot_cli_cached_input(input: u64, output: u64, cache_read: u64, total: u64) -> u64 {
    if cache_read > 0 && input >= cache_read && total == input.saturating_add(output) {
        input - cache_read
    } else {
        input
    }
}

fn normalize_copilot_cli_token_stats(tokens: &mut Option<TokenStats>) {
    let Some(tokens) = tokens else {
        return;
    };
    let cache_read = tokens.cache_read.unwrap_or(0);
    tokens.input =
        separate_copilot_cli_cached_input(tokens.input, tokens.output, cache_read, tokens.total);
}

fn normalize_copilot_cli_usage_entry(entry: &mut UsageEntry) {
    normalize_copilot_cli_token_stats(&mut entry.tokens);
    normalize_copilot_cli_token_stats(&mut entry.delta_tokens);
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
    let mut selector = InitialUserPromptSelector::default();
    for line_res in reader.lines() {
        let Ok(line) = line_res else {
            continue;
        };
        let Ok(event) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        match event.get("type").and_then(|event_type| event_type.as_str()) {
            Some("USER_INPUT") => {
                if let Some(content) = event.get("content").and_then(|content| content.as_str()) {
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
                    selector.observe_user_prompt(request_text);
                }
            }
            Some("PLANNER_RESPONSE" | "RUN_COMMAND" | "GREP_SEARCH" | "LIST_DIRECTORY")
            | Some("VIEW_FILE" | "CODE_ACTION" | "GENERIC" | "ERROR_MESSAGE" | "TOOL_CALL") => {
                selector.observe_non_user_message();
            }
            _ => {}
        }
        if selector.is_complete() {
            break;
        }
    }
    selector.into_name()
}

fn get_copilot_session_name(session_id: &str) -> Option<String> {
    let copilot_dir = get_copilot_dir();
    let events_path = copilot_dir
        .join("session-state")
        .join(session_id)
        .join("events.jsonl");
    let path = if events_path.exists() {
        events_path
    } else {
        copilot_dir
            .join("session-state")
            .join(format!("{session_id}.jsonl"))
    };
    let file = File::open(path).ok()?;
    let reader = BufReader::new(file);
    let mut selector = InitialUserPromptSelector::default();

    for line_res in reader.lines() {
        let Ok(line) = line_res else {
            continue;
        };
        let Ok(event) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let event_type = event
            .get("type")
            .and_then(|event_type| event_type.as_str())
            .unwrap_or("");
        match event_type {
            "user.message" | "USER_PROMPT" => {
                let payload = event.get("payload").or_else(|| event.get("data"));
                if let Some(content) = payload
                    .and_then(|payload| payload.get("content"))
                    .and_then(|content| content.as_str())
                {
                    selector.observe_user_prompt(content);
                }
            }
            "assistant.message"
            | "ASSISTANT_REPLY"
            | "tool.call"
            | "TOOL_CALL"
            | "tool.response"
            | "TOOL_RESPONSE"
            | "tool.execution_start"
            | "tool.execution_complete" => selector.observe_non_user_message(),
            _ => {}
        }
        if selector.is_complete() {
            break;
        }
    }

    selector.into_name()
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
    let source_kind = if assistant_type == "copilot" {
        "copilot-cli"
    } else {
        "legacy"
    };

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
                let mut parsed_entries = parse_usage_entries(&new_content);

                if assistant_type == "copilot" {
                    for entry in &mut parsed_entries {
                        normalize_copilot_cli_usage_entry(entry);
                    }
                }

                if parsed_entries.is_empty() {
                    continue;
                }

                let tx = conn
                    .transaction()
                    .map_err(|e| format!("Transaction BEGIN 失敗: {}", e))?;

                let mut success = true;
                let mut resolved_names = HashMap::<String, Option<String>>::new();
                for entry in &parsed_entries {
                    let tokens = entry.tokens.as_ref();
                    let delta = entry.delta_tokens.as_ref();
                    let cost = entry.cost.as_ref();

                    let resolved_name = resolved_names
                        .entry(entry.session_id.clone())
                        .or_insert_with(|| match assistant_type {
                            "antigravity" => get_antigravity_session_name(&entry.session_id),
                            "copilot" => get_copilot_session_name(&entry.session_id),
                            _ => None,
                        })
                        .clone()
                        .or_else(|| entry.session_name.clone());

                    let insert_res = tx.execute(
                        "INSERT OR IGNORE INTO usage_entries (
                            assistant_type, source_kind, timestamp, date, session_id, session_name, transcript_path, cwd, version, turn_no, model, model_id,
                            tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, tokens_reasoning, tokens_total,
                            delta_input, delta_output, delta_cache_read, delta_cache_write, delta_reasoning, delta_total,
                            duration_ms, premium_requests
                        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                        params![
                            assistant_type,
                            source_kind,
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

                    if let Some(name) = resolved_name.as_deref() {
                        if let Err(error) = tx.execute(
                            "UPDATE usage_entries
                             SET session_name = ?
                             WHERE assistant_type = ?
                               AND source_kind = ?
                               AND session_id = ?",
                            params![name, assistant_type, source_kind, entry.session_id],
                        ) {
                            eprintln!("[{}] 更新會話名稱失敗: {}", assistant_type, error);
                            success = false;
                            break;
                        }
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

fn insert_vscode_usage_entry(
    tx: &rusqlite::Transaction<'_>,
    entry: &UsageEntry,
) -> rusqlite::Result<usize> {
    let tokens = entry.tokens.as_ref();
    let delta = entry.delta_tokens.as_ref();
    let cost = entry.cost.as_ref();
    tx.execute(
        "INSERT OR REPLACE INTO usage_entries (
            assistant_type, source_kind, timestamp, date, session_id, session_name, transcript_path, cwd, version, turn_no, model, model_id,
            tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, tokens_reasoning, tokens_total,
            delta_input, delta_output, delta_cache_read, delta_cache_write, delta_reasoning, delta_total,
            duration_ms, premium_requests, parent_session_id, agent_nickname, agent_role, reasoning_effort
        ) VALUES (
            ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
            ?, ?, ?, ?, ?, ?,
            ?, ?, ?, ?, ?, ?,
            ?, ?, ?, ?, ?, ?
        )",
        params![
            "copilot",
            entry.source_kind.as_deref().unwrap_or(crate::vscode::SOURCE_KIND),
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
            tokens.map(|value| value.input as i64),
            tokens.map(|value| value.output as i64),
            tokens.and_then(|value| value.cache_read.map(|v| v as i64)),
            tokens.and_then(|value| value.cache_write.map(|v| v as i64)),
            tokens.and_then(|value| value.reasoning.map(|v| v as i64)),
            tokens.map(|value| value.total as i64),
            delta.map(|value| value.input as i64),
            delta.map(|value| value.output as i64),
            delta.and_then(|value| value.cache_read.map(|v| v as i64)),
            delta.and_then(|value| value.cache_write.map(|v| v as i64)),
            delta.and_then(|value| value.reasoning.map(|v| v as i64)),
            delta.map(|value| value.total as i64),
            cost.and_then(|value| value.total_duration_ms.or(value.total_api_duration_ms))
                .map(|value| value as i64),
            cost.and_then(|value| value.total_premium_requests)
                .map(|value| value as i64),
            entry.parent_session_id.as_deref(),
            entry.agent_nickname.as_deref(),
            entry.agent_role.as_deref(),
            entry.reasoning_effort.as_deref(),
        ],
    )
}

fn sync_vscode_chat_sessions(conn: &mut Connection) -> Result<(), String> {
    let mut seen_sessions = HashSet::new();

    for filepath in crate::vscode::discover_session_files() {
        let metadata = match fs::metadata(&filepath) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        let current_size = metadata.len();
        let modified_time = metadata
            .modified()
            .ok()
            .and_then(|value| value.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|value| value.as_nanos() as i64)
            .unwrap_or(0);
        let state_key = format!("vscode:{}", filepath.to_string_lossy());
        let previous_state: Option<(u64, i64)> = conn
            .query_row(
                "SELECT last_synced_size, last_synced_time FROM sync_state WHERE filename = ?",
                params![state_key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok();

        if previous_state == Some((current_size, modified_time)) {
            continue;
        }

        let session = match crate::vscode::read_session_file(&filepath) {
            Ok(session) => session,
            Err(error) => {
                eprintln!("解析 VS Code Copilot 檔案 {:?} 失敗: {}", filepath, error);
                continue;
            }
        };
        let session_key = session.session_id.clone();
        if !crate::vscode::is_github_copilot(&session) || !seen_sessions.insert(session_key.clone())
        {
            let tx = conn
                .transaction()
                .map_err(|error| format!("建立 VS Code 狀態交易失敗: {error}"))?;
            tx.execute(
                "INSERT OR REPLACE INTO sync_state (filename, last_synced_size, last_synced_time)
                 VALUES (?, ?, ?)",
                params![state_key, current_size as i64, modified_time],
            )
            .map_err(|error| format!("更新 VS Code 狀態失敗: {error}"))?;
            tx.commit()
                .map_err(|error| format!("提交 VS Code 狀態交易失敗: {error}"))?;
            continue;
        }
        let entries = crate::vscode::to_usage_entries(&session, &filepath);

        let tx = conn
            .transaction()
            .map_err(|error| format!("建立 VS Code 同步交易失敗: {error}"))?;
        let db_session_id = format!("vscode-{session_key}");
        tx.execute(
            "DELETE FROM usage_entries
             WHERE assistant_type = 'copilot'
               AND source_kind = ?
               AND session_id = ?",
            params![crate::vscode::SOURCE_KIND, db_session_id],
        )
        .map_err(|error| format!("清除舊 VS Code 工作階段失敗: {error}"))?;

        for entry in &entries {
            insert_vscode_usage_entry(&tx, entry)
                .map_err(|error| format!("寫入 VS Code Copilot 資料失敗: {error}"))?;
        }

        tx.execute(
            "INSERT OR REPLACE INTO sync_state (filename, last_synced_size, last_synced_time)
             VALUES (?, ?, ?)",
            params![state_key, current_size as i64, modified_time],
        )
        .map_err(|error| format!("更新 VS Code 同步狀態失敗: {error}"))?;
        tx.commit()
            .map_err(|error| format!("提交 VS Code 同步交易失敗: {error}"))?;
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
    let mut session_name_selector = InitialUserPromptSelector::default();
    let mut session_cwd: Option<String> = None;
    let mut session_version: Option<String> = None;
    let mut parent_session_id: Option<String> = None;
    let mut agent_nickname: Option<String> = None;
    let mut agent_role: Option<String> = None;
    let mut current_model = "GPT-5.3-Codex".to_string();
    let mut reasoning_effort: Option<String> = None;
    let mut session_identity_locked = false;

    for event in &events {
        let event_type = event.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let payload = match event.get("payload") {
            Some(payload) => payload,
            None => continue,
        };
        let payload_type = payload.get("type").and_then(|t| t.as_str()).unwrap_or("");

        if event_type == "session_meta" {
            if !session_identity_locked {
                if let Some(id) = payload
                    .get("id")
                    .and_then(|id| id.as_str())
                    .filter(|id| !id.is_empty())
                    .or_else(|| {
                        payload
                            .get("session_id")
                            .and_then(|id| id.as_str())
                            .filter(|id| !id.is_empty())
                    })
                {
                    session_id = id.to_string();
                    session_identity_locked = true;
                }
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
        }

        match (event_type, payload_type) {
            ("event_msg", "user_message") => {
                if let Some(message) = payload.get("message").and_then(|message| message.as_str()) {
                    session_name_selector.observe_user_prompt(message);
                }
            }
            ("response_item", "message")
                if payload.get("role").and_then(|role| role.as_str()) == Some("user") =>
            {
                if let Some(content) = payload.get("content") {
                    session_name_selector.observe_user_prompt(&codex_content_to_text(content));
                }
            }
            ("event_msg", "agent_message")
            | ("response_item", "function_call" | "function_call_output") => {
                session_name_selector.observe_non_user_message();
            }
            ("response_item", "message")
                if payload.get("role").and_then(|role| role.as_str()) == Some("assistant") =>
            {
                session_name_selector.observe_non_user_message();
            }
            _ => {}
        }
    }

    let session_name = session_name_selector.into_name();

    if parent_session_id.as_deref() == Some(session_id.as_str()) {
        parent_session_id = None;
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
            source_kind: None,
            source_dir_key: None,
            parent_session_id: parent_session_id.clone(),
            agent_nickname: agent_nickname.clone(),
            agent_role: agent_role.clone(),
            reasoning_effort: effort_for_turn.clone(),
        });
    }

    Ok(results)
}

fn run_codex_parser_migration(conn: &mut Connection) -> Result<(), String> {
    let parser_migration_done: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_state WHERE filename = ?)",
            params![CODEX_PARSER_MIGRATION_KEY],
            |row| row.get(0),
        )
        .unwrap_or(false);

    if !parser_migration_done {
        let tx = conn
            .transaction()
            .map_err(|e| format!("Codex parser migration BEGIN 失敗: {}", e))?;
        tx.execute(
            "UPDATE usage_entries
             SET parent_session_id = NULL
             WHERE assistant_type = 'codex' AND parent_session_id = session_id",
            [],
        )
        .map_err(|e| format!("修正 Codex self-parent 資料失敗: {}", e))?;
        tx.execute(
            "DELETE FROM sync_state
             WHERE filename LIKE 'codex:sessions/%'
                OR filename LIKE 'codex:sessions\\%'",
            [],
        )
        .map_err(|e| format!("清除 Codex 同步狀態失敗: {}", e))?;
        tx.execute(
            "INSERT OR REPLACE INTO sync_state (filename, last_synced_size, last_synced_time) VALUES (?, 1, 0)",
            params![CODEX_PARSER_MIGRATION_KEY],
        )
        .map_err(|e| format!("寫入 Codex parser migration 狀態失敗: {}", e))?;
        tx.commit()
            .map_err(|e| format!("Codex parser migration COMMIT 失敗: {}", e))?;
    }
    Ok(())
}

fn portable_relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Sync token usage from the Copilot App (Tauri desktop application).
///
/// The Copilot App writes per-API-call usage into `~/.copilot/session-store.db`
/// (`assistant_usage_events`) and per-session aggregates into `~/.copilot/data.db`
/// (`sessions`). This collector groups API calls by `(session_id, turn_index)`
/// into per-turn `UsageEntry` rows with `source_kind = "copilot-app"`, and
/// deduplicates via the `import_source_id` unique index
/// (`copilot-app:<session_id>:<turn_index>`).
///
/// Incremental sync is tracked by storing the maximum `(created_at, id)` seen
/// in `sync_state`, scoped by the canonical source directory so switching
/// `COPILOT_APP_DIR`/`COPILOT_DIR` starts a fresh cursor.
///
/// Because `assistant_usage_events` records per-API-call usage (not cumulative
/// session totals), `delta_*` columns are set equal to the per-turn SUM; no
/// differencing against a previous turn is performed. To handle turns that
/// receive additional API calls after the first sync, affected turns are
/// re-aggregated from the full event history (not just `created_at > cursor`)
/// and upserted via `INSERT OR REPLACE` keyed on `import_source_id`.
fn sync_copilot_app_usage_logs(conn: &mut Connection) -> Result<(), String> {
    let app_dir = crate::paths::copilot_app_dir();
    let session_store_path = app_dir.join("session-store.db");
    if !session_store_path.exists() {
        return Ok(());
    }

    // Canonicalize the source directory so the cursor is stable across trailing
    // slashes / symlinks and isolated per COPILOT_APP_DIR / COPILOT_DIR value.
    // Hex-encode the canonical path's raw OS-encoded bytes so the cursor key is
    // injective (no two distinct paths map to the same key) and free of LIKE
    // wildcard characters (`%`, `_`). Encoding raw bytes (not lossy UTF-8) avoids
    // collisions from Unicode replacement chars and from `\\` vs `/` normalization.
    let canonical_app_dir = app_dir.canonicalize().unwrap_or_else(|_| app_dir.clone());
    let source_key = encode_hex(canonical_app_dir.as_os_str().as_encoded_bytes());
    let cursor_key_prefix = format!("{}{}::", COPILOT_APP_CURSOR_PREFIX, source_key);

    // Open the Copilot App session-store in read-only mode with a busy timeout
    // so concurrent writes from the app do not block us.
    let session_store = match Connection::open_with_flags(
        &session_store_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "⚠️ 無法開啟 Copilot App session-store.db ({}): {}",
                session_store_path.display(),
                e
            );
            return Ok(());
        }
    };
    let _ = session_store.busy_timeout(std::time::Duration::from_secs(2));

    // Confirm the expected table exists; older or future schemas may differ.
    let table_exists: bool = session_store
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='assistant_usage_events'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);
    if !table_exists {
        return Ok(());
    }

    // Optional join source for session title / workspace.
    let data_db_path = app_dir.join("data.db");
    let data_db = if data_db_path.exists() {
        Connection::open_with_flags(
            &data_db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .ok()
    } else {
        None
    };
    if let Some(ref c) = data_db {
        let _ = c.busy_timeout(std::time::Duration::from_secs(2));
    }

    // Load last sync cursor (scoped by canonical source path). New cursors
    // store `created_at` and the INTEGER event id. A legacy timestamp-only
    // cursor is read as `(timestamp, i64::MIN)` so all events at that
    // timestamp are safely re-processed once before it is upgraded.
    let stored_cursor: Option<String> = conn
        .query_row(
            "SELECT filename FROM sync_state WHERE filename LIKE ? ESCAPE '\\' LIMIT 1",
            params![format!("{}%", cursor_key_prefix)],
            |row| row.get::<_, String>(0),
        )
        .ok()
        .and_then(|f| f.strip_prefix(&cursor_key_prefix).map(|s| s.to_string()));
    let (last_cursor, legacy_cursor) = stored_cursor
        .map(|suffix| parse_copilot_app_cursor(&suffix))
        .unwrap_or((None, false));

    // Scan new events in stable high-water-mark order. The legacy path uses
    // the same strict tuple predicate, with the minimum INTEGER id as its
    // one-time compatibility baseline.
    let touched_query = if last_cursor.is_some() {
        "SELECT session_id, turn_index, created_at, id
         FROM assistant_usage_events
         WHERE created_at > ?
            OR (created_at = ? AND id > ?)
         ORDER BY created_at ASC, id ASC"
    } else {
        "SELECT session_id, turn_index, created_at, id
         FROM assistant_usage_events
         ORDER BY created_at ASC, id ASC"
    };

    let mut touched_stmt = session_store
        .prepare(touched_query)
        .map_err(|e| format!("準備 Copilot App touched-turns 查詢失敗: {}", e))?;
    let map_touched = |row: &rusqlite::Row| -> rusqlite::Result<(String, i64, String, i64)> {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
        ))
    };
    let touched_iter = if let Some(ref cursor) = last_cursor {
        touched_stmt
            .query_map(
                params![cursor.0.as_str(), cursor.0.as_str(), cursor.1],
                map_touched,
            )
            .map_err(|e| format!("執行 Copilot App touched-turns 查詢失敗: {}", e))?
    } else {
        touched_stmt
            .query_map([], map_touched)
            .map_err(|e| format!("執行 Copilot App touched-turns 查詢失敗: {}", e))?
    };

    // Deduplicate touched turns while preserving the stable event order, and
    // retain the final event tuple as the source high-water mark.
    let mut touched_turns: Vec<(String, i64)> = Vec::new();
    let mut touched_set: HashSet<(String, i64)> = HashSet::new();
    let mut max_event_cursor: Option<(String, i64)> = None;
    let mut scan_failed = false;
    for row_res in touched_iter {
        match row_res {
            Ok((session_id, turn_index, created_at, id)) => {
                max_event_cursor = Some((created_at, id));
                if touched_set.insert((session_id.clone(), turn_index)) {
                    touched_turns.push((session_id, turn_index));
                }
            }
            Err(e) => {
                eprintln!("⚠️ 讀取 Copilot App touched-turn 失敗: {}", e);
                scan_failed = true;
            }
        }
    }

    if scan_failed {
        return Ok(());
    }

    // Upgrade a legacy timestamp-only cursor even when there are no events
    // after it. The maximum id at the legacy timestamp is the safest tuple
    // boundary and prevents the old timestamp from causing repeated scans.
    if touched_turns.is_empty() {
        if legacy_cursor {
            let legacy_timestamp = last_cursor
                .as_ref()
                .map(|cursor| cursor.0.as_str())
                .ok_or_else(|| "Copilot App legacy cursor 遺失 timestamp".to_string())?;
            let max_id: Option<i64> = session_store
                .query_row(
                    "SELECT MAX(id) FROM assistant_usage_events WHERE created_at = ?",
                    params![legacy_timestamp],
                    |row| row.get(0),
                )
                .map_err(|e| format!("讀取 Copilot App legacy cursor id 失敗: {}", e))?;
            let tx = conn.transaction().map_err(|e| {
                format!("開啟 Copilot App cursor migration transaction 失敗: {}", e)
            })?;
            write_copilot_app_cursor(
                &tx,
                &cursor_key_prefix,
                legacy_timestamp,
                max_id.unwrap_or(0),
            )?;
            tx.commit()
                .map_err(|e| format!("Copilot App cursor migration COMMIT 失敗: {}", e))?;
        }
        return Ok(());
    }

    // Re-aggregate each touched turn from the FULL event history for that
    // (session_id, turn_index), regardless of cursor. This guarantees that
    // turns which straddle the cursor boundary are written with their complete
    // token totals rather than only the post-cursor subset.
    //
    // `assistant_usage_events.input_tokens` already INCLUDES cache reads
    // (cache retrievals are a subset of the input the model processed). To
    // avoid double-counting cache-read tokens in both `tokens_input` and
    // `tokens_cache_read` (and again in `tokens_total` / pricing), we store
    // the net non-cached input as `tokens_input = SUM(input_tokens) -
    // SUM(cache_read_tokens)`, mirroring the Copilot CLI normalization
    // (`separate_copilot_cli_cached_input`). `tokens_cache_read` keeps the
    // raw cache-read total; `tokens_total` sums net input + output +
    // cache_read + cache_write + reasoning, so cache read is counted once.
    let aggregate_query = "SELECT MIN(created_at) AS ts,
                SUM(input_tokens), SUM(output_tokens),
                SUM(cache_read_tokens), SUM(cache_write_tokens),
                SUM(reasoning_tokens), SUM(duration_ms),
                MIN(model), MIN(reasoning_effort)
         FROM assistant_usage_events
         WHERE session_id = ? AND turn_index = ?
         GROUP BY session_id, turn_index";

    let mut agg_stmt = session_store
        .prepare(aggregate_query)
        .map_err(|e| format!("準備 Copilot App 聚合查詢失敗: {}", e))?;

    let mut turn_rows: Vec<CopilotAppTurnRow> = Vec::new();
    for (session_id, turn_index) in &touched_turns {
        let row_res = agg_stmt.query_row(params![session_id, turn_index], |row| {
            let raw_input: i64 = row.get::<_, Option<i64>>(1)?.unwrap_or(0).max(0);
            let cache_read: i64 = row.get::<_, Option<i64>>(3)?.unwrap_or(0).max(0);
            // Net non-cached input; clamp at 0 in case of schema drift.
            let net_input = (raw_input - cache_read).max(0) as u64;
            Ok(CopilotAppTurnRow {
                session_id: session_id.clone(),
                turn_index: *turn_index,
                ts: row.get::<_, String>(0)?,
                input_tokens: net_input,
                output_tokens: row.get::<_, Option<i64>>(2)?.unwrap_or(0).max(0) as u64,
                cache_read: cache_read as u64,
                cache_write: row.get::<_, Option<i64>>(4)?.unwrap_or(0).max(0) as u64,
                reasoning: row.get::<_, Option<i64>>(5)?.unwrap_or(0).max(0) as u64,
                duration_ms: row.get::<_, Option<i64>>(6)?.unwrap_or(0).max(0) as u64,
                model: row.get::<_, Option<String>>(7)?,
                reasoning_effort: row.get::<_, Option<String>>(8)?,
            })
        });
        match row_res {
            Ok(r) => turn_rows.push(r),
            Err(e) => {
                eprintln!(
                    "⚠️ 聚合 Copilot App turn (session {} turn {}) 失敗: {}",
                    session_id, turn_index, e
                );
                return Ok(());
            }
        }
    }

    if turn_rows.is_empty() {
        return Ok(());
    }

    let tx = conn
        .transaction()
        .map_err(|e| format!("開啟 Copilot App transaction 失敗: {}", e))?;

    // Cache session title/workspace lookups.
    let mut session_meta_cache: HashMap<String, CopilotAppSessionMeta> = HashMap::new();

    let mut upserted = 0usize;

    for row in turn_rows {
        // Resolve session metadata (title + cwd).
        let meta = session_meta_cache
            .entry(row.session_id.clone())
            .or_insert_with(|| resolve_copilot_app_session_meta(&data_db, &row.session_id))
            .clone();

        // Normalize timestamp: Copilot App uses `YYYY-MM-DD HH:MM:SS` UTC.
        // Convert to ISO 8601 with `Z` to match other collectors.
        let timestamp = normalize_copilot_app_timestamp(&row.ts);
        let date_str = timestamp.get(..10).unwrap_or(&row.ts).to_string();
        let turn_no = (row.turn_index.max(0) + 1) as u32;

        // tokens_total counts cache_read once (as its own component), since
        // tokens_input has already been normalized to the non-cached portion.
        let total =
            row.input_tokens + row.output_tokens + row.cache_read + row.cache_write + row.reasoning;

        // Delta tokens: the source records per-API-call usage (not cumulative
        // session totals), so the per-turn SUM already represents the delta for
        // this turn. Set delta_* equal to the per-turn totals directly; do NOT
        // subtract the previous turn's totals.
        let delta_input = row.input_tokens;
        let delta_output = row.output_tokens;
        let delta_cache_read = row.cache_read;
        let delta_cache_write = row.cache_write;
        let delta_reasoning = row.reasoning;
        let delta_total = total;

        // Include the source directory key in import_source_id so turns from
        // different COPILOT_APP_DIR with the same (session_id, turn_index) do
        // not upsert-overwrite each other.
        let import_source_id = format!(
            "copilot-app:{}:{}:{}",
            source_key, row.session_id, row.turn_index
        );

        // Use INSERT OR REPLACE so turns that received additional API calls
        // after the first sync are updated with the complete re-aggregated
        // totals instead of being silently dropped by INSERT OR IGNORE.
        // source_dir_key isolates rows by source directory in the unique index.
        let insert_res = tx.execute(
            "INSERT OR REPLACE INTO usage_entries (
                assistant_type, source_kind, source_dir_key, timestamp, date, session_id, session_name, transcript_path, cwd, version, turn_no, model, model_id,
                tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, tokens_reasoning, tokens_total,
                delta_input, delta_output, delta_cache_read, delta_cache_write, delta_reasoning, delta_total,
                duration_ms, premium_requests, import_source_id, reasoning_effort
            ) VALUES (?, ?, ?, ?, ?, ?, ?, NULL, ?, NULL, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, ?, ?)",
            params![
                "copilot",
                COPILOT_APP_SOURCE_KIND,
                source_key,
                timestamp,
                date_str,
                row.session_id,
                meta.title,
                meta.cwd,
                turn_no as i64,
                row.model,
                row.model,
                row.input_tokens as i64,
                row.output_tokens as i64,
                row.cache_read as i64,
                row.cache_write as i64,
                row.reasoning as i64,
                total as i64,
                delta_input as i64,
                delta_output as i64,
                delta_cache_read as i64,
                delta_cache_write as i64,
                delta_reasoning as i64,
                delta_total as i64,
                row.duration_ms as i64,
                import_source_id,
                row.reasoning_effort,
            ],
        );

        match insert_res {
            Ok(_) => upserted += 1,
            Err(e) => {
                eprintln!(
                    "⚠️ 寫入 Copilot App usage 失敗 (session {} turn {}): {}",
                    row.session_id, row.turn_index, e
                );
                let _ = tx.rollback();
                return Ok(());
            }
        }
    }

    // Store the maximum raw event tuple for this source directory.
    // Use the max raw event `created_at` (not per-turn MIN) so a turn whose
    // events straddle the cursor does not pin the cursor at its earliest event
    // and get re-aggregated on every subsequent sync.
    //
    // Only advance the cursor when every touched turn was aggregated and
    // written successfully. If any turn failed (aggregation or upsert error),
    // keep the cursor at its previous value so the failed turns are retried on
    // the next sync instead of being permanently skipped.
    if let Some((created_at, id)) = max_event_cursor {
        if let Err(e) = write_copilot_app_cursor(&tx, &cursor_key_prefix, &created_at, id) {
            eprintln!("⚠️ 寫入 Copilot App cursor 失敗: {}", e);
            let _ = tx.rollback();
            return Ok(());
        }
    }

    tx.commit()
        .map_err(|e| format!("Copilot App transaction COMMIT 失敗: {}", e))?;

    if upserted > 0 {
        println!("✅ 同步 Copilot App：{} 筆 turn（upsert）", upserted);
    }
    Ok(())
}

fn parse_copilot_app_cursor(suffix: &str) -> (Option<(String, i64)>, bool) {
    if let Some((created_at, id)) = suffix.rsplit_once("::") {
        if !created_at.is_empty() {
            if let Ok(id) = id.parse::<i64>() {
                return (Some((created_at.to_string(), id)), false);
            }
        }
    }

    if suffix.is_empty() {
        (None, false)
    } else {
        (Some((suffix.to_string(), i64::MIN)), true)
    }
}

fn write_copilot_app_cursor(
    tx: &rusqlite::Transaction<'_>,
    cursor_key_prefix: &str,
    created_at: &str,
    id: i64,
) -> Result<(), String> {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    tx.execute(
        "DELETE FROM sync_state WHERE filename LIKE ? ESCAPE '\\'",
        params![format!("{}%", cursor_key_prefix)],
    )
    .map_err(|e| format!("刪除舊 Copilot App cursor 失敗: {}", e))?;
    let cursor_sentinel = format!("{}{}::{}", cursor_key_prefix, created_at, id);
    tx.execute(
        "INSERT OR REPLACE INTO sync_state (filename, last_synced_size, last_synced_time) VALUES (?, ?, ?)",
        params![cursor_sentinel, 0i64, now],
    )
    .map_err(|e| format!("寫入 Copilot App cursor 失敗: {}", e))?;
    Ok(())
}

struct CopilotAppTurnRow {
    session_id: String,
    turn_index: i64,
    ts: String,
    input_tokens: u64,
    output_tokens: u64,
    cache_read: u64,
    cache_write: u64,
    reasoning: u64,
    duration_ms: u64,
    model: Option<String>,
    reasoning_effort: Option<String>,
}

#[derive(Clone, Default)]
struct CopilotAppSessionMeta {
    title: Option<String>,
    cwd: Option<String>,
}

fn resolve_copilot_app_session_meta(
    data_db: &Option<Connection>,
    session_id: &str,
) -> CopilotAppSessionMeta {
    let Some(db) = data_db else {
        return CopilotAppSessionMeta::default();
    };

    let title: Option<String> = db
        .query_row(
            "SELECT title FROM sessions WHERE id = ?",
            params![session_id],
            |row| row.get(0),
        )
        .ok()
        .flatten();

    // Workspace/checkout path resolution is non-trivial across schema versions;
    // leave cwd empty for now. The frontend can fall back to session-state.
    CopilotAppSessionMeta { title, cwd: None }
}

/// Convert Copilot App `created_at` (`YYYY-MM-DD HH:MM:SS` UTC) to ISO 8601.
fn normalize_copilot_app_timestamp(raw: &str) -> String {
    // Already ISO-ish; ensure `T` separator and `Z` suffix.
    if raw.len() >= 19 {
        format!("{}T{}Z", &raw[..10], &raw[11..19])
    } else {
        raw.to_string()
    }
}

/// Hex-encode bytes into a lowercase hex string (no external dependency).
/// Used to build an injective, LIKE-wildcard-free cursor key from a canonical
/// source directory path.
fn encode_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{:02x}", byte));
    }
    out
}

fn sync_codex_usage_logs(conn: &mut Connection) -> Result<(), String> {
    let codex_dir = get_codex_dir();
    let sessions_dir = codex_dir.join("sessions");

    run_codex_parser_migration(conn)?;

    if !sessions_dir.exists() {
        return Ok(());
    }

    let files = find_codex_session_files(&sessions_dir);

    for filepath in files {
        let state_path = portable_relative_path(&codex_dir, &filepath);
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

            if parsed_entries.is_empty() {
                continue;
            }

            let tx = conn
                .transaction()
                .map_err(|e| format!("Transaction BEGIN 失敗: {}", e))?;

            let transcript_path = filepath.to_string_lossy().into_owned();
            #[cfg(windows)]
            let transcript_delete_result = tx.execute(
                "DELETE FROM usage_entries
                 WHERE assistant_type = 'codex'
                   AND (transcript_path = ? COLLATE NOCASE
                        OR transcript_path = ? COLLATE NOCASE)",
                params![transcript_path, transcript_path.replace('\\', "/")],
            );
            #[cfg(not(windows))]
            let transcript_delete_result = tx.execute(
                "DELETE FROM usage_entries
                 WHERE assistant_type = 'codex' AND transcript_path = ?",
                params![transcript_path],
            );
            transcript_delete_result
                .map_err(|e| format!("清空舊 Codex CLI transcript 資料失敗: {}", e))?;

            let session_ids: HashSet<String> = parsed_entries
                .iter()
                .map(|entry| entry.session_id.clone())
                .collect();
            for session_id in session_ids {
                tx.execute(
                    "DELETE FROM usage_entries WHERE assistant_type = 'codex' AND session_id = ?",
                    params![session_id],
                )
                .map_err(|e| format!("清空舊 Codex CLI Session 資料失敗: {}", e))?;
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

    let mut session_name_selector = InitialUserPromptSelector::default();
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

        if role == "user" {
            if let Some(content) = message.get("content") {
                let has_tool_result = content.as_array().is_some_and(|items| {
                    items.iter().any(|item| {
                        item.get("type").and_then(|item_type| item_type.as_str())
                            == Some("tool_result")
                    })
                });
                if has_tool_result {
                    session_name_selector.observe_non_user_message();
                } else {
                    session_name_selector.observe_user_prompt(&claude_content_to_text(content));
                }
            }
            continue;
        }

        if role != "assistant" {
            continue;
        }
        session_name_selector.observe_non_user_message();

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
            session_name: session_name_selector
                .selected_name()
                .map(str::to_string)
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
            source_kind: None,
            source_dir_key: None,
            parent_session_id: None,
            agent_nickname: None,
            agent_role: None,
            reasoning_effort: None,
        });
    }

    Ok(results)
}

fn migrate_legacy_claude_usage_entries(conn: &Connection) -> Result<usize, String> {
    conn.execute(
        "UPDATE usage_entries SET assistant_type = 'claude'
         WHERE assistant_type = 'codex'
           AND transcript_path IS NOT NULL
           AND (
                transcript_path LIKE '%.claude/%'
             OR transcript_path LIKE '%/claude/%'
             OR transcript_path LIKE '%.claude\\%'
             OR transcript_path LIKE '%\\claude\\%'
           )",
        [],
    )
    .map_err(|error| format!("遷移 Claude Code 舊資料失敗: {error}"))
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
        let _ = migrate_legacy_claude_usage_entries(conn);
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

pub fn parse_cursor_timestamp(s: &str) -> String {
    let parts: Vec<&str> = s.split(" (UTC").collect();
    if parts.is_empty() {
        return s.to_string();
    }
    let dt_part = parts[0].trim();
    let dt_str = if let Some(comma_idx) = dt_part.find(',') {
        dt_part[comma_idx + 1..].trim()
    } else {
        dt_part
    };

    let formats = [
        "%b %e, %Y, %l:%M %p",
        "%b %d, %Y, %I:%M %p",
        "%b %d, %Y, %l:%M %p",
        "%b %e, %Y, %I:%M %p",
        "%Y-%m-%d %H:%M:%S",
    ];

    for fmt in &formats {
        if let Ok(naive_dt) = chrono::NaiveDateTime::parse_from_str(dt_str, fmt) {
            if parts.len() > 1 {
                let tz_str = parts[1].trim_end_matches(')');
                let hours_str = if tz_str.contains(':') {
                    tz_str.split(':').next().unwrap_or("0")
                } else {
                    tz_str
                };
                if let Ok(hours) = hours_str.parse::<i32>() {
                    if let Some(offset) = chrono::FixedOffset::east_opt(hours * 3600) {
                        use chrono::TimeZone;
                        let local_dt = offset.from_local_datetime(&naive_dt);
                        if let chrono::LocalResult::Single(dt_tz) = local_dt {
                            return dt_tz.to_rfc3339();
                        }
                    }
                }
            }
            return naive_dt.format("%Y-%m-%d %H:%M:%S").to_string();
        }
    }

    s.to_string()
}

fn find_cursor_session_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(find_cursor_session_files(&path));
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

fn cursor_content_to_text(content: &serde_json::Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_string();
    }
    let mut parts = Vec::new();
    if let Some(items) = content.as_array() {
        for item in items {
            let itype = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if itype == "text" {
                if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                    parts.push(text.to_string());
                }
            }
        }
    }
    parts.join(" ")
}

fn parse_cursor_session_file(filepath: &Path) -> Result<Vec<UsageEntry>, String> {
    let file = File::open(filepath).map_err(|e| format!("無法開啟檔案: {}", e))?;
    let reader = BufReader::new(file);
    let fallback_session_id = filepath
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("unknown-session")
        .to_string();

    let mut session_name_selector = InitialUserPromptSelector::default();
    let mut results = Vec::new();

    let mut current_timestamp = String::new();
    let mut current_prompt = String::new();

    for line_res in reader.lines() {
        let line = match line_res {
            Ok(line) => line,
            Err(_) => continue,
        };
        let event: serde_json::Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => continue,
        };

        let role = event.get("role").and_then(|r| r.as_str()).unwrap_or("");

        if role == "user" {
            let content_val = event.get("message").and_then(|m| m.get("content"));
            let text = cursor_content_to_text(content_val.unwrap_or(&serde_json::Value::Null));

            let mut extracted_ts = String::new();
            if let Some(start_idx) = text.find("<timestamp>") {
                let actual_start = start_idx + "<timestamp>".len();
                if let Some(end_idx) = text[actual_start..].find("</timestamp>") {
                    extracted_ts = text[actual_start..(actual_start + end_idx)].to_string();
                }
            }

            if !extracted_ts.is_empty() {
                current_timestamp = parse_cursor_timestamp(&extracted_ts);
            }

            let mut clean_prompt = text.clone();
            if let Some(start_idx) = clean_prompt.find("<user_query>") {
                let actual_start = start_idx + "<user_query>".len();
                if let Some(end_idx) = clean_prompt[actual_start..].find("</user_query>") {
                    clean_prompt = clean_prompt[actual_start..(actual_start + end_idx)].to_string();
                }
            }

            current_prompt = clean_prompt.trim().to_string();
            session_name_selector.observe_user_prompt(&current_prompt);
        } else if role == "assistant" {
            session_name_selector.observe_non_user_message();
            let content_val = event.get("message").and_then(|m| m.get("content"));
            let reply_text =
                cursor_content_to_text(content_val.unwrap_or(&serde_json::Value::Null));

            if current_timestamp.is_empty() {
                if let Ok(metadata) = filepath.metadata() {
                    if let Ok(modified) = metadata.modified() {
                        let datetime: chrono::DateTime<chrono::Utc> = modified.into();
                        current_timestamp = datetime.format("%Y-%m-%d %H:%M:%S").to_string();
                    }
                }
            }
            if current_timestamp.is_empty() {
                current_timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
            }

            let input_tokens = (current_prompt.len() / 4).max(10) as u64;
            let output_tokens = (reply_text.len() / 4).max(10) as u64;
            let total_tokens = input_tokens + output_tokens;

            let tokens = TokenStats {
                input: input_tokens,
                output: output_tokens,
                cache_read: Some(0),
                cache_write: Some(0),
                reasoning: None,
                total: total_tokens,
            };

            results.push(UsageEntry {
                timestamp: current_timestamp.clone(),
                session_id: fallback_session_id.clone(),
                session_name: session_name_selector
                    .selected_name()
                    .map(str::to_string)
                    .or_else(|| Some(fallback_session_id.clone())),
                transcript_path: Some(filepath.to_string_lossy().into_owned()),
                cwd: None,
                version: None,
                turn_no: (results.len() + 1) as u32,
                model: Some("Cursor Agent".to_string()),
                model_id: Some("Cursor Agent".to_string()),
                tokens: Some(tokens.clone()),
                delta_tokens: Some(tokens),
                context: None,
                cost: None,
                source_kind: None,
                source_dir_key: None,
                parent_session_id: None,
                agent_nickname: None,
                agent_role: None,
                reasoning_effort: None,
            });
        }
    }

    Ok(results)
}

fn sync_cursor_usage_logs(conn: &mut Connection, cursor_dir: &Path) -> Result<(), String> {
    let projects_dir = cursor_dir.join("projects");
    if !projects_dir.exists() {
        return Ok(());
    }

    let files = find_cursor_session_files(&projects_dir);

    for filepath in files {
        let state_path = filepath
            .strip_prefix(cursor_dir)
            .unwrap_or(&filepath)
            .to_string_lossy()
            .into_owned();
        let state_key = format!("cursor:{}", state_path);

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
            let parsed_entries = match parse_cursor_session_file(&filepath) {
                Ok(entries) => entries,
                Err(e) => {
                    eprintln!("解析 Cursor 會話檔案 {:?} 失敗: {}", filepath, e);
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
                    "DELETE FROM usage_entries WHERE assistant_type = 'cursor' AND session_id = ?",
                    params![session_id],
                );

                if let Err(e) = delete_res {
                    eprintln!("清空舊 Cursor Session 資料失敗: {}", e);
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
                        "cursor",
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
                    eprintln!("寫入 Cursor 資料庫失敗 (turn_no {}): {}", entry.turn_no, e);
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

    // 2b. Sync GitHub Copilot sessions created in VS Code
    if let Err(e) = sync_vscode_chat_sessions(conn) {
        eprintln!("❌ 同步 VS Code Copilot 失敗: {}", e);
    }

    // 2c. Sync GitHub Copilot App (Tauri desktop) usage
    if let Err(e) = sync_copilot_app_usage_logs(conn) {
        eprintln!("❌ 同步 Copilot App 失敗: {}", e);
    }

    // 3. Sync Codex CLI
    if let Err(e) = sync_codex_usage_logs(conn) {
        eprintln!("❌ 同步 Codex CLI 失敗: {}", e);
    }

    // 4. Sync Claude Code
    if let Err(e) = sync_claude_usage_logs(conn) {
        eprintln!("❌ 同步 Claude Code 失敗: {}", e);
    }

    // 5. Sync Cursor
    let cursor_dir = get_cursor_dir();
    if let Err(e) = sync_cursor_usage_logs(conn, &cursor_dir) {
        eprintln!("❌ 同步 Cursor 失敗: {}", e);
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
        let mut tokens_input = row.get::<_, Option<i64>>(10)?;
        let tokens_output = row.get::<_, Option<i64>>(11)?;
        let tokens_cache_read = row.get::<_, Option<i64>>(12)?;
        let tokens_reasoning = row.get::<_, Option<i64>>(13)?;
        let tokens_total = row.get::<_, Option<i64>>(14)?;
        let mut delta_input = row.get::<_, Option<i64>>(15)?;
        let delta_output = row.get::<_, Option<i64>>(16)?;
        let delta_cache_read = row.get::<_, Option<i64>>(17)?;
        let delta_reasoning = row.get::<_, Option<i64>>(18)?;
        let delta_total = row.get::<_, Option<i64>>(19)?;

        if assistant == "copilot" {
            let normalize_input = |input: Option<i64>,
                                   output: Option<i64>,
                                   cache_read: Option<i64>,
                                   total: Option<i64>| {
                let (Some(input), Some(output), Some(cache_read), Some(total)) =
                    (input, output, cache_read, total)
                else {
                    return input;
                };
                let Ok(input_unsigned) = u64::try_from(input) else {
                    return Some(input);
                };
                let Ok(output_unsigned) = u64::try_from(output) else {
                    return Some(input);
                };
                let Ok(cache_read_unsigned) = u64::try_from(cache_read) else {
                    return Some(input);
                };
                let Ok(total_unsigned) = u64::try_from(total) else {
                    return Some(input);
                };
                Some(separate_copilot_cli_cached_input(
                    input_unsigned,
                    output_unsigned,
                    cache_read_unsigned,
                    total_unsigned,
                ) as i64)
            };
            tokens_input =
                normalize_input(tokens_input, tokens_output, tokens_cache_read, tokens_total);
            delta_input = normalize_input(delta_input, delta_output, delta_cache_read, delta_total);
        }

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
                assistant_type, source_kind, timestamp, date, session_id, session_name, transcript_path, cwd, version, turn_no, model, model_id,
                tokens_input, tokens_output, tokens_cache_read, tokens_reasoning, tokens_total,
                delta_input, delta_output, delta_cache_read, delta_reasoning, delta_total,
                duration_ms, premium_requests, parent_session_id, agent_nickname, agent_role
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                assistant,
                if assistant == "copilot" {
                    "copilot-cli"
                } else {
                    "legacy"
                },
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
                tokens_input,
                tokens_output,
                tokens_cache_read,
                tokens_reasoning,
                tokens_total,
                delta_input,
                delta_output,
                delta_cache_read,
                delta_reasoning,
                delta_total,
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
) -> Result<Vec<(UsageDayExportRecord, String)>, String> {
    let mut query = "SELECT 
            timestamp, session_id, session_name, transcript_path, cwd, version, turn_no, model, model_id,
            tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, tokens_reasoning, tokens_total,
            delta_input, delta_output, delta_cache_read, delta_cache_write, delta_reasoning, delta_total,
            duration_ms, premium_requests, parent_session_id, agent_nickname, agent_role, assistant_type, reasoning_effort, import_source_id, source_kind, source_dir_key
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
        let import_source_id = normalize_import_source_id(
            row.get::<_, Option<String>>(28)
                .map_err(|e| e.to_string())?
                .as_deref(),
        );

        let mut record = UsageDayExportRecord {
            entry: UsageEntry {
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
                source_kind: row.get(29).ok(),
                source_dir_key: row.get(30).ok(),
                parent_session_id: row.get(23).ok(),
                agent_nickname: row.get(24).ok(),
                agent_role: row.get(25).ok(),
                reasoning_effort: row.get(27).ok(),
            },
            import_source_id,
        };

        if record.import_source_id.is_none() {
            record.import_source_id = Some(build_usage_entry_import_source_id(
                assistant,
                date,
                &record.entry,
            ));
        }

        entries.push((record, ast_type));
    }
    Ok(entries)
}

fn entry_date_from_timestamp(timestamp: &str) -> Option<&str> {
    let trimmed = timestamp.trim();
    trimmed
        .split(['T', ' '])
        .next()
        .filter(|date_part| date_part.len() == 10)
}

pub fn export_usage_day_entries(
    conn: &rusqlite::Connection,
    assistant: &str,
    date: &str,
) -> Result<Vec<UsageDayExportRecord>, String> {
    let rows = get_usage_entries_by_date(conn, date, assistant)?;
    let mut records = Vec::with_capacity(rows.len());

    for (mut record, _assistant_type) in rows {
        if record.import_source_id.is_none() {
            record.import_source_id = Some(build_usage_entry_import_source_id(
                assistant,
                date,
                &record.entry,
            ));
        }
        records.push(record);
    }

    Ok(records)
}

pub fn import_usage_day_entries(
    conn: &mut Connection,
    assistant: &str,
    date: &str,
    records: Vec<UsageDayExportRecord>,
) -> Result<UsageDayImportSummary, String> {
    let total = records.len();
    if total == 0 {
        return Err("匯入資料為空".to_string());
    }

    let mut inserted = 0usize;
    let mut skipped_duplicates = 0usize;

    let tx = conn
        .transaction()
        .map_err(|e| format!("建立匯入交易失敗: {}", e))?;

    for record in records {
        let mut entry = record.entry;
        let normalized_id = normalize_import_source_id(record.import_source_id.as_deref());
        let file_date = entry_date_from_timestamp(&entry.timestamp)
            .ok_or_else(|| "無效的 timestamp 格式，無法取得日期".to_string())?;
        if file_date != date {
            return Err(format!(
                "匯入資料日期不一致：預期 {date}，但資料為 {file_date}"
            ));
        }

        let source_kind = entry
            .source_kind
            .clone()
            .unwrap_or_else(|| "legacy".to_string());
        if assistant == "copilot" && matches!(source_kind.as_str(), "copilot-cli" | "legacy") {
            normalize_copilot_cli_usage_entry(&mut entry);
        }
        let source_id = normalized_id
            .unwrap_or_else(|| build_usage_entry_import_source_id(assistant, date, &entry));

        let imported = tx
            .execute(
                "INSERT OR IGNORE INTO usage_entries (
                    assistant_type, source_kind, timestamp, date, session_id, session_name, transcript_path, cwd, version, turn_no,
                    model, model_id, tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, tokens_reasoning, tokens_total,
                    delta_input, delta_output, delta_cache_read, delta_cache_write, delta_reasoning, delta_total,
                    duration_ms, premium_requests,
                    parent_session_id, agent_nickname, agent_role, reasoning_effort, import_source_id
                ) VALUES (
                    ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                    ?, ?, ?, ?, ?, ?, ?, ?,
                    ?, ?, ?, ?, ?, ?,
                    ?, ?, ?, ?, ?, ?, ?
                )",
                rusqlite::params![
                    assistant,
                    source_kind,
                    entry.timestamp,
                    date,
                    entry.session_id,
                    entry.session_name,
                    entry.transcript_path,
                    entry.cwd,
                    entry.version,
                    entry.turn_no as i64,
                    entry.model,
                    entry.model_id,
                    entry.tokens.as_ref().map(|t| t.input as i64),
                    entry.tokens.as_ref().map(|t| t.output as i64),
                    entry.tokens.as_ref().and_then(|t| t.cache_read.map(|v| v as i64)),
                    entry.tokens.as_ref().and_then(|t| t.cache_write.map(|v| v as i64)),
                    entry.tokens.as_ref().and_then(|t| t.reasoning.map(|v| v as i64)),
                    entry.tokens.as_ref().map(|t| t.total as i64),
                    entry.delta_tokens.as_ref().map(|t| t.input as i64),
                    entry.delta_tokens.as_ref().map(|t| t.output as i64),
                    entry.delta_tokens.as_ref().and_then(|t| t.cache_read.map(|v| v as i64)),
                    entry.delta_tokens.as_ref().and_then(|t| t.cache_write.map(|v| v as i64)),
                    entry.delta_tokens.as_ref().and_then(|t| t.reasoning.map(|v| v as i64)),
                    entry.delta_tokens.as_ref().map(|t| t.total as i64),
                    entry.cost.as_ref().and_then(|c| c.total_api_duration_ms).map(|v| v as i64),
                    entry.cost.as_ref().and_then(|c| c.total_premium_requests).map(|v| v as i64),
                    entry.parent_session_id,
                    entry.agent_nickname,
                    entry.agent_role,
                    entry.reasoning_effort,
                    source_id,
                ],
            )
            .map_err(|e| format!("匯入資料寫入失敗: {}", e))?;

        if imported > 0 {
            inserted += 1;
        } else {
            skipped_duplicates += 1;
        }
    }

    tx.commit()
        .map_err(|e| format!("提交匯入結果失敗: {}", e))?;

    Ok(UsageDayImportSummary {
        date: date.to_string(),
        total,
        imported: inserted,
        skipped_duplicates,
    })
}

pub fn get_session_assistant_and_transcript(
    conn: &rusqlite::Connection,
    assistant: &str,
    session_id: &str,
) -> Result<(String, Option<String>, String, Option<String>), String> {
    let mut stmt = conn
        .prepare(
            "SELECT assistant_type, transcript_path, source_kind, source_dir_key
             FROM usage_entries WHERE session_id = ? AND assistant_type = ? LIMIT 1",
        )
        .map_err(|e| e.to_string())?;
    let mut rows = stmt
        .query(params![session_id, assistant])
        .map_err(|e| e.to_string())?;
    if let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let ast: String = row.get(0).map_err(|e| e.to_string())?;
        let path: Option<String> = row.get(1).ok();
        let source_kind = row
            .get::<_, Option<String>>(2)
            .ok()
            .flatten()
            .unwrap_or_else(|| "legacy".to_string());
        let source_dir_key: Option<String> = row.get(3).ok().flatten();
        Ok((ast, path, source_kind, source_dir_key))
    } else {
        Err("Session not found".to_string())
    }
}

pub fn get_session_cwd(
    conn: &rusqlite::Connection,
    session_id: &str,
    source_dir_key: Option<&str>,
) -> Result<Option<String>, String> {
    let mut stmt = conn
        .prepare("SELECT cwd FROM usage_entries WHERE session_id = ? AND cwd IS NOT NULL AND (? IS NULL OR source_dir_key = ?) LIMIT 1")
        .map_err(|e| e.to_string())?;
    let mut rows = stmt
        .query(params![session_id, source_dir_key, source_dir_key])
        .map_err(|e| e.to_string())?;
    if let Some(row) = rows.next().map_err(|e| e.to_string())? {
        Ok(row.get::<_, String>(0).ok())
    } else {
        Ok(None)
    }
}

pub fn get_session_turns_token_stats(
    conn: &rusqlite::Connection,
    session_id: &str,
    source_dir_key: Option<&str>,
) -> Result<HashMap<u32, (TokenStats, String)>, String> {
    let mut map = HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT turn_no, delta_input, delta_output, delta_cache_read, delta_cache_write, delta_reasoning, delta_total, model
         FROM usage_entries
         WHERE session_id = ? AND (? IS NULL OR source_dir_key = ?)
         ORDER BY turn_no ASC"
    ).map_err(|e| e.to_string())?;
    let mut rows = stmt
        .query(params![session_id, source_dir_key, source_dir_key])
        .map_err(|e| e.to_string())?;
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
            date, source_kind, source_dir_key
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
                source_kind: row.get(27).ok(),
                source_dir_key: row.get(28).ok(),
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
            date, source_kind, source_dir_key
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
                source_kind: row.get(27).ok(),
                source_dir_key: row.get(28).ok(),
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

    fn sample_import_record() -> UsageDayExportRecord {
        UsageDayExportRecord {
            entry: UsageEntry {
                timestamp: "2026-07-10T12:34:56Z".to_string(),
                session_id: "import-session".to_string(),
                session_name: Some("匯入測試".to_string()),
                transcript_path: Some("/tmp/import.json".to_string()),
                cwd: Some("/tmp".to_string()),
                version: Some("0.1.4".to_string()),
                turn_no: 1,
                model: Some("gpt-5".to_string()),
                model_id: Some("gpt-5".to_string()),
                tokens: Some(TokenStats {
                    input: 100,
                    output: 20,
                    cache_read: Some(30),
                    cache_write: Some(10),
                    reasoning: Some(5),
                    total: 120,
                }),
                delta_tokens: Some(TokenStats {
                    input: 10,
                    output: 2,
                    cache_read: Some(3),
                    cache_write: Some(1),
                    reasoning: Some(1),
                    total: 12,
                }),
                context: None,
                cost: Some(CostStats {
                    total_api_duration_ms: Some(125.0),
                    total_duration_ms: None,
                    total_premium_requests: Some(1.0),
                }),
                source_kind: None,
                source_dir_key: None,
                parent_session_id: Some("parent-session".to_string()),
                agent_nickname: Some("worker".to_string()),
                agent_role: Some("analysis".to_string()),
                reasoning_effort: Some("high".to_string()),
            },
            import_source_id: Some("import-test-record".to_string()),
        }
    }

    #[test]
    fn session_name_uses_last_prompt_from_initial_consecutive_run() {
        let mut selector = InitialUserPromptSelector::default();
        selector.observe_user_prompt("第一條提示");
        selector.observe_user_prompt("第二條提示");
        selector.observe_non_user_message();
        selector.observe_user_prompt("後續提示");

        assert_eq!(selector.into_name().as_deref(), Some("第二條提示"));
    }

    #[test]
    fn session_name_uses_first_prompt_when_initial_run_has_one_prompt() {
        let mut selector = InitialUserPromptSelector::default();
        selector.observe_user_prompt("第一條提示");
        selector.observe_non_user_message();
        selector.observe_user_prompt("後續提示");

        assert_eq!(selector.into_name().as_deref(), Some("第一條提示"));
    }

    #[test]
    fn session_name_falls_back_to_first_user_prompt_after_non_user_message() {
        let mut selector = InitialUserPromptSelector::default();
        selector.observe_non_user_message();
        selector.observe_user_prompt("第一條使用者提示");
        selector.observe_user_prompt("不應取代名稱");

        assert_eq!(selector.into_name().as_deref(), Some("第一條使用者提示"));
    }

    #[test]
    fn hook_session_name_readers_use_last_initial_consecutive_prompt() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_antigravity_dir = std::env::var("ANTIGRAVITY_DIR").ok();
        let old_copilot_dir = std::env::var("COPILOT_DIR").ok();
        let base_dir = temp_jsonl_path("hook-session-names").with_extension("");
        let antigravity_dir = base_dir.join("antigravity");
        let copilot_dir = base_dir.join("copilot");
        let antigravity_log = antigravity_dir
            .join("brain")
            .join("antigravity-session")
            .join(".system_generated/logs/transcript_full.jsonl");
        let copilot_log = copilot_dir
            .join("session-state")
            .join("copilot-session")
            .join("events.jsonl");
        fs::create_dir_all(antigravity_log.parent().unwrap()).unwrap();
        fs::create_dir_all(copilot_log.parent().unwrap()).unwrap();
        fs::write(
            &antigravity_log,
            r#"{"type":"USER_INPUT","content":"第一條提示"}
{"type":"USER_INPUT","content":"<USER_REQUEST>第二條提示</USER_REQUEST>"}
{"type":"PLANNER_RESPONSE","content":"收到"}
{"type":"USER_INPUT","content":"後續提示"}
"#,
        )
        .unwrap();
        fs::write(
            &copilot_log,
            r#"{"type":"session.start","data":{}}
{"type":"user.message","data":{"content":"First prompt"}}
{"type":"user.message","data":{"content":"Second prompt"}}
{"type":"assistant.message","data":{"content":"Reply"}}
{"type":"user.message","data":{"content":"Later prompt"}}
"#,
        )
        .unwrap();
        std::env::set_var("ANTIGRAVITY_DIR", &antigravity_dir);
        std::env::set_var("COPILOT_DIR", &copilot_dir);

        assert_eq!(
            get_antigravity_session_name("antigravity-session").as_deref(),
            Some("第二條提示")
        );
        assert_eq!(
            get_copilot_session_name("copilot-session").as_deref(),
            Some("Second prompt")
        );

        if let Some(value) = old_antigravity_dir {
            std::env::set_var("ANTIGRAVITY_DIR", value);
        } else {
            std::env::remove_var("ANTIGRAVITY_DIR");
        }
        if let Some(value) = old_copilot_dir {
            std::env::set_var("COPILOT_DIR", value);
        } else {
            std::env::remove_var("COPILOT_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    #[test]
    fn normalize_copilot_cli_usage_entry_separates_cached_input() {
        let mut entry = sample_import_record().entry;
        entry.model = Some("mai-code-1-flash-picker · medium".to_string());
        entry.tokens = Some(TokenStats {
            input: 443_554,
            output: 1_370,
            cache_read: Some(401_024),
            cache_write: Some(0),
            reasoning: Some(384),
            total: 444_924,
        });
        entry.delta_tokens = entry.tokens.clone();

        normalize_copilot_cli_usage_entry(&mut entry);

        assert_eq!(
            entry.tokens.as_ref().map(|tokens| tokens.input),
            Some(42_530)
        );
        assert_eq!(
            entry.delta_tokens.as_ref().map(|tokens| tokens.input),
            Some(42_530)
        );
        assert_eq!(
            entry.tokens.as_ref().map(|tokens| tokens.total),
            Some(444_924)
        );
    }

    #[test]
    fn normalize_copilot_cli_usage_entry_preserves_net_input() {
        let mut entry = sample_import_record().entry;
        entry.tokens = Some(TokenStats {
            input: 42_530,
            output: 1_370,
            cache_read: Some(401_024),
            cache_write: Some(0),
            reasoning: Some(384),
            total: 444_924,
        });
        entry.delta_tokens = entry.tokens.clone();

        normalize_copilot_cli_usage_entry(&mut entry);

        assert_eq!(
            entry.tokens.as_ref().map(|tokens| tokens.input),
            Some(42_530)
        );
        assert_eq!(
            entry.delta_tokens.as_ref().map(|tokens| tokens.input),
            Some(42_530)
        );
    }

    #[test]
    fn sync_antigravity_usage_log_writes_all_columns() {
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        let usage_file = temp_jsonl_path("antigravity-sync");
        let base_dir = usage_file.with_extension("");
        let usage_dir = base_dir.join("usage");
        fs::create_dir_all(&usage_dir).unwrap();
        let log_path = usage_dir.join("usage-2026-07-12.jsonl");
        let record = sample_import_record().entry;
        fs::write(
            &log_path,
            format!("{}\n", serde_json::to_string(&record).unwrap()),
        )
        .unwrap();

        sync_hook_usage_logs(&mut conn, "antigravity", &base_dir).unwrap();

        let inserted: (u64, String, Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT COUNT(*), source_kind, tokens_cache_write, delta_cache_write
                 FROM usage_entries WHERE assistant_type = 'antigravity'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(inserted, (1, "legacy".to_string(), Some(10), Some(1)));

        fs::remove_dir_all(base_dir).unwrap();
    }

    #[test]
    fn sync_copilot_usage_log_separates_cached_input() {
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        let usage_file = temp_jsonl_path("copilot-sync");
        let base_dir = usage_file.with_extension("");
        let usage_dir = base_dir.join("usage");
        fs::create_dir_all(&usage_dir).unwrap();
        let log_path = usage_dir.join("usage-2026-07-15.jsonl");
        let mut record = sample_import_record().entry;
        record.session_id = "copilot-cache-session".to_string();
        record.tokens = Some(TokenStats {
            input: 443_554,
            output: 1_370,
            cache_read: Some(401_024),
            cache_write: Some(0),
            reasoning: Some(384),
            total: 444_924,
        });
        record.delta_tokens = record.tokens.clone();
        fs::write(
            &log_path,
            format!("{}\n", serde_json::to_string(&record).unwrap()),
        )
        .unwrap();

        sync_hook_usage_logs(&mut conn, "copilot", &base_dir).unwrap();

        let inserted: (u64, u64) = conn
            .query_row(
                "SELECT tokens_input, delta_input
                 FROM usage_entries
                 WHERE assistant_type = 'copilot' AND session_id = 'copilot-cache-session'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(inserted, (42_530, 42_530));

        fs::remove_dir_all(base_dir).unwrap();
    }

    #[test]
    fn import_usage_day_entries_writes_and_deduplicates_records() {
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        let record = sample_import_record();

        let first =
            import_usage_day_entries(&mut conn, "codex", "2026-07-10", vec![record.clone()])
                .unwrap();
        assert_eq!(first.imported, 1);
        assert_eq!(first.skipped_duplicates, 0);

        let second =
            import_usage_day_entries(&mut conn, "codex", "2026-07-10", vec![record]).unwrap();
        assert_eq!(second.imported, 0);
        assert_eq!(second.skipped_duplicates, 1);

        let imported_rows: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries WHERE assistant_type = ? AND import_source_id = ?",
                params!["codex", "import-test-record"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(imported_rows, 1);
    }

    #[test]
    fn import_usage_day_entries_normalizes_copilot_cached_input() {
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        let mut record = sample_import_record();
        record.entry.session_id = "imported-copilot-cache".to_string();
        record.entry.source_kind = Some("copilot-cli".to_string());
        record.entry.tokens = Some(TokenStats {
            input: 443_554,
            output: 1_370,
            cache_read: Some(401_024),
            cache_write: Some(0),
            reasoning: Some(384),
            total: 444_924,
        });
        record.entry.delta_tokens = record.entry.tokens.clone();
        record.import_source_id = Some("imported-copilot-cache".to_string());

        import_usage_day_entries(&mut conn, "copilot", "2026-07-10", vec![record]).unwrap();

        let inserted: (u64, u64) = conn
            .query_row(
                "SELECT tokens_input, delta_input
                 FROM usage_entries
                 WHERE assistant_type = 'copilot' AND session_id = 'imported-copilot-cache'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(inserted, (42_530, 42_530));
    }

    #[test]
    fn init_db_migrates_legacy_copilot_source_kind() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, timestamp, date, session_id, turn_no
             ) VALUES ('copilot', '2026-07-10T00:00:00Z', '2026-07-10', 'legacy-copilot', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "DELETE FROM sync_state WHERE filename = ?",
            params![COPILOT_SOURCE_KIND_MIGRATION_KEY],
        )
        .unwrap();

        init_db(&conn).unwrap();

        let source_kind: String = conn
            .query_row(
                "SELECT source_kind FROM usage_entries WHERE session_id = 'legacy-copilot'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(source_kind, "copilot-cli");
    }

    #[test]
    fn init_db_removes_empty_vscode_session_placeholders() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, timestamp, date, session_id, turn_no,
                tokens_input, tokens_output, tokens_total
             ) VALUES
                ('copilot', 'vscode-chat', '2026-07-10T00:00:00Z', '2026-07-10', 'empty-vscode', 1, NULL, NULL, NULL),
                ('copilot', 'vscode-chat', '2026-07-10T00:01:00Z', '2026-07-10', 'unresolved-vscode', 1, 8, 2, 10)",
            [],
        )
        .unwrap();
        conn.execute(
            "DELETE FROM sync_state WHERE filename = 'migration:vscode_empty_sessions_v1'",
            [],
        )
        .unwrap();

        init_db(&conn).unwrap();

        let empty_count: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries WHERE session_id = 'empty-vscode'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let unresolved_count: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries WHERE session_id = 'unresolved-vscode'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(empty_count, 0);
        assert_eq!(unresolved_count, 1);
    }

    #[test]
    fn init_db_normalizes_legacy_copilot_cached_input() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, timestamp, date, session_id, turn_no,
                tokens_input, tokens_output, tokens_cache_read, tokens_total,
                delta_input, delta_output, delta_cache_read, delta_total
             ) VALUES
                ('copilot', 'copilot-cli', '2026-07-15T20:40:35Z', '2026-07-15', 'raw-copilot', 1,
                 443554, 1370, 401024, 444924, 443554, 1370, 401024, 444924),
                ('copilot', 'copilot-cli', '2026-07-15T20:40:36Z', '2026-07-15', 'net-copilot', 1,
                 42530, 1370, 401024, 444924, 42530, 1370, 401024, 444924),
                ('antigravity', 'legacy', '2026-07-15T20:40:37Z', '2026-07-15', 'other-assistant', 1,
                 443554, 1370, 401024, 444924, 443554, 1370, 401024, 444924)",
            [],
        )
        .unwrap();
        conn.execute(
            "DELETE FROM sync_state WHERE filename = 'migration:copilot_cached_input_v1'",
            [],
        )
        .unwrap();

        init_db(&conn).unwrap();

        let raw_copilot: (u64, u64) = conn
            .query_row(
                "SELECT tokens_input, delta_input FROM usage_entries WHERE session_id = 'raw-copilot'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let net_copilot: (u64, u64) = conn
            .query_row(
                "SELECT tokens_input, delta_input FROM usage_entries WHERE session_id = 'net-copilot'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let other_assistant: (u64, u64) = conn
            .query_row(
                "SELECT tokens_input, delta_input FROM usage_entries WHERE session_id = 'other-assistant'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();

        assert_eq!(raw_copilot, (42_530, 42_530));
        assert_eq!(net_copilot, (42_530, 42_530));
        assert_eq!(other_assistant, (443_554, 443_554));
    }

    #[test]
    fn session_name_migration_resets_source_sync_state_without_deleting_usage() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, timestamp, date, session_id, session_name, turn_no
             ) VALUES (
                'codex', '2026-07-16T00:00:00Z', '2026-07-16',
                'preserved-session', '舊名稱', 1
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "DELETE FROM sync_state WHERE filename = ?",
            params![SESSION_NAME_SELECTION_MIGRATION_KEY],
        )
        .unwrap();
        for state_key in [
            "antigravity:usage-2026-07-16.jsonl",
            "copilot:usage-2026-07-16.jsonl",
            "vscode:session.jsonl",
            "codex:sessions/2026/07/session.jsonl",
            "claude:projects/session.jsonl",
            "cursor:projects/session.jsonl",
        ] {
            conn.execute(
                "INSERT INTO sync_state (filename, last_synced_size, last_synced_time)
                 VALUES (?, 10, 0)",
                params![state_key],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO sync_state (filename, last_synced_size, last_synced_time)
             VALUES ('migration:unrelated', 1, 0)",
            [],
        )
        .unwrap();

        init_db(&conn).unwrap();

        let source_state_count: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_state
                 WHERE filename LIKE 'antigravity:%'
                    OR filename LIKE 'copilot:%'
                    OR filename LIKE 'vscode:%'
                    OR filename LIKE 'codex:sessions/%'
                    OR filename LIKE 'claude:%'
                    OR filename LIKE 'cursor:%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let preserved_usage_count: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries WHERE session_id = 'preserved-session'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let unrelated_state_count: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_state WHERE filename = 'migration:unrelated'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(source_state_count, 0);
        assert_eq!(preserved_usage_count, 1);
        assert_eq!(unrelated_state_count, 1);
    }

    #[test]
    fn migrate_records_normalizes_copilot_cached_input() {
        let src_conn = Connection::open_in_memory().unwrap();
        src_conn
            .execute_batch(
                "CREATE TABLE usage_entries (
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
                    tokens_input INTEGER,
                    tokens_output INTEGER,
                    tokens_cache_read INTEGER,
                    tokens_reasoning INTEGER,
                    tokens_total INTEGER,
                    delta_input INTEGER,
                    delta_output INTEGER,
                    delta_cache_read INTEGER,
                    delta_reasoning INTEGER,
                    delta_total INTEGER,
                    duration_ms INTEGER,
                    premium_requests INTEGER
                );
                INSERT INTO usage_entries (
                    timestamp, date, session_id, turn_no, model,
                    tokens_input, tokens_output, tokens_cache_read, tokens_total,
                    delta_input, delta_output, delta_cache_read, delta_total
                ) VALUES (
                    '2026-07-15T20:40:35Z', '2026-07-15', 'legacy-copilot-cache', 1,
                    'mai-code-1-flash-picker · medium',
                    443554, 1370, 401024, 444924,
                    443554, 1370, 401024, 444924
                );",
            )
            .unwrap();
        let mut dest_conn = Connection::open_in_memory().unwrap();
        init_db(&dest_conn).unwrap();

        migrate_records(&src_conn, &mut dest_conn, "copilot").unwrap();

        let inserted: (String, u64, u64) = dest_conn
            .query_row(
                "SELECT source_kind, tokens_input, delta_input
                 FROM usage_entries
                 WHERE session_id = 'legacy-copilot-cache'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(inserted, ("copilot-cli".to_string(), 42_530, 42_530));
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
    fn parse_codex_session_file_uses_last_initial_consecutive_user_prompt_as_name() {
        let path = temp_jsonl_path("codex-session-name");
        let content = r#"{"timestamp":"2026-07-16T00:00:00Z","type":"session_meta","payload":{"session_id":"session-name","model":"gpt-5.5"}}
{"timestamp":"2026-07-16T00:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"第一條提示"}}
{"timestamp":"2026-07-16T00:00:02Z","type":"event_msg","payload":{"type":"user_message","message":"第二條提示"}}
{"timestamp":"2026-07-16T00:00:03Z","type":"event_msg","payload":{"type":"agent_message","message":"收到"}}
{"timestamp":"2026-07-16T00:00:04Z","type":"event_msg","payload":{"type":"user_message","message":"後續提示"}}
{"timestamp":"2026-07-16T00:00:05Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":10,"reasoning_output_tokens":4,"total_tokens":110},"model_context_window":258400}}}
"#;

        fs::write(&path, content).unwrap();
        let entries = parse_codex_session_file(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_name.as_deref(), Some("第二條提示"));
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
    fn parse_codex_session_file_keeps_subagent_identity_separate_from_parent() {
        let path = temp_jsonl_path("codex-subagent");
        let content = r#"{"timestamp":"2026-07-10T03:45:00.000Z","type":"session_meta","payload":{"session_id":"parent-session","id":"child-session","forked_from_id":"parent-session","parent_thread_id":"parent-session","cwd":"/tmp/project","cli_version":"0.142.5","model":"gpt-5.5","agent_nickname":"reviewer","agent_role":"review","source":{"subagent":{"thread_spawn":{"parent_thread_id":"parent-session","depth":1,"agent_nickname":"reviewer","agent_role":"review"}}}}}
{"timestamp":"2026-07-10T03:45:00.500Z","type":"session_meta","payload":{"session_id":"parent-session","id":"parent-session","cwd":"/tmp/project","cli_version":"0.142.5","model":"gpt-5.5","source":"cli"}}
{"timestamp":"2026-07-10T03:45:01.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":10,"reasoning_output_tokens":4,"total_tokens":110},"last_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":10,"reasoning_output_tokens":4,"total_tokens":110},"model_context_window":258400}}}
"#;

        fs::write(&path, content).unwrap();
        let entries = parse_codex_session_file(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_id, "child-session");
        assert_eq!(
            entries[0].parent_session_id.as_deref(),
            Some("parent-session")
        );
        assert_ne!(entries[0].session_id, "parent-session");
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
    fn sync_codex_usage_logs_preserves_parent_and_subagent_sessions() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_codex_dir = std::env::var("CODEX_DIR").ok();
        let mut codex_dir = std::env::temp_dir();
        let unique = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        codex_dir.push(format!(
            "codex-parent-child-sync-{}-{}",
            std::process::id(),
            unique
        ));

        let sessions_dir = codex_dir.join("sessions/2026/07/10");
        fs::create_dir_all(&sessions_dir).unwrap();
        let parent_path = sessions_dir.join("rollout-2026-07-10T03-43-00-parent-session.jsonl");
        let child_path = sessions_dir.join("rollout-2026-07-10T03-45-00-child-session.jsonl");

        let parent_content = r#"{"timestamp":"2026-07-10T03:43:00.000Z","type":"session_meta","payload":{"session_id":"parent-session","id":"parent-session","cwd":"/tmp/project","cli_version":"0.142.5","model":"gpt-5.5"}}
{"timestamp":"2026-07-10T03:43:01.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":10,"reasoning_output_tokens":4,"total_tokens":110},"last_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":10,"reasoning_output_tokens":4,"total_tokens":110},"model_context_window":258400}}}
"#;
        let child_content = r#"{"timestamp":"2026-07-10T03:45:00.000Z","type":"session_meta","payload":{"session_id":"parent-session","id":"child-session","forked_from_id":"parent-session","parent_thread_id":"parent-session","cwd":"/tmp/project","cli_version":"0.142.5","model":"gpt-5.5","source":{"subagent":{"thread_spawn":{"parent_thread_id":"parent-session","depth":1}}}}}
{"timestamp":"2026-07-10T03:45:00.500Z","type":"session_meta","payload":{"session_id":"parent-session","id":"parent-session","cwd":"/tmp/project","cli_version":"0.142.5","model":"gpt-5.5","source":"cli"}}
{"timestamp":"2026-07-10T03:45:01.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":50,"cached_input_tokens":10,"output_tokens":5,"reasoning_output_tokens":2,"total_tokens":55},"last_token_usage":{"input_tokens":50,"cached_input_tokens":10,"output_tokens":5,"reasoning_output_tokens":2,"total_tokens":55},"model_context_window":258400}}}
"#;

        fs::write(&parent_path, parent_content).unwrap();
        fs::write(&child_path, child_content).unwrap();
        std::env::set_var("CODEX_DIR", &codex_dir);

        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        conn.execute(
            "INSERT INTO sync_state (filename, last_synced_size, last_synced_time) VALUES ('migration:codex_delta_from_totals_v2', 1, 0)",
            [],
        )
        .unwrap();
        let parent_state_key =
            format!("codex:{}", portable_relative_path(&codex_dir, &parent_path));
        let child_state_key = format!("codex:{}", portable_relative_path(&codex_dir, &child_path));
        for (path, state_key) in [
            (&parent_path, parent_state_key.as_str()),
            (&child_path, child_state_key.as_str()),
        ] {
            let size = fs::metadata(path).unwrap().len() as i64;
            conn.execute(
                "INSERT INTO sync_state (filename, last_synced_size, last_synced_time) VALUES (?, ?, 0)",
                params![state_key, size],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO sync_state (filename, last_synced_size, last_synced_time) VALUES (?, 10, 0)",
            params![r"codex:sessions\2026\07\10\legacy.jsonl"],
        )
        .unwrap();
        #[cfg(windows)]
        let stale_transcript_path = child_path
            .to_string_lossy()
            .replace('\\', "/")
            .to_uppercase();
        #[cfg(not(windows))]
        let stale_transcript_path = child_path.to_string_lossy().into_owned();
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, timestamp, date, session_id, transcript_path, turn_no,
                parent_session_id
             ) VALUES ('codex', '2026-07-10T00:00:00Z', '2026-07-10',
                'legacy-shared', ?, 1, 'legacy-shared')",
            params![stale_transcript_path],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, timestamp, date, session_id, turn_no
             ) VALUES ('antigravity', '2026-07-10T00:00:00Z', '2026-07-10',
                'unrelated-session', 1)",
            [],
        )
        .unwrap();

        sync_codex_usage_logs(&mut conn).unwrap();

        let session_count: u64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT session_id) FROM usage_entries WHERE assistant_type = 'codex'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(session_count, 2);

        let child_parent: Option<String> = conn
            .query_row(
                "SELECT parent_session_id FROM usage_entries WHERE assistant_type = 'codex' AND session_id = 'child-session' LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(child_parent.as_deref(), Some("parent-session"));

        let self_parent_count: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries WHERE assistant_type = 'codex' AND parent_session_id = session_id",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let transcript_count: u64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT transcript_path) FROM usage_entries WHERE assistant_type = 'codex'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let unrelated_count: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries WHERE assistant_type = 'antigravity' AND session_id = 'unrelated-session'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let legacy_count: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries WHERE assistant_type = 'codex' AND session_id = 'legacy-shared'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let migration_marker_count: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_state WHERE filename = ?",
                params![CODEX_PARSER_MIGRATION_KEY],
                |row| row.get(0),
            )
            .unwrap();
        let legacy_state_count: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_state WHERE filename LIKE 'codex:sessions\\%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(self_parent_count, 0);
        assert_eq!(transcript_count, 2);
        assert_eq!(unrelated_count, 1);
        assert_eq!(legacy_count, 0);
        assert_eq!(migration_marker_count, 1);
        assert_eq!(legacy_state_count, 0);

        sync_codex_usage_logs(&mut conn).unwrap();
        let codex_rows_after_second_sync: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries WHERE assistant_type = 'codex'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(codex_rows_after_second_sync, 2);

        let synced_child_size: u64 = conn
            .query_row(
                "SELECT last_synced_size FROM sync_state WHERE filename = ?",
                params![child_state_key],
                |row| row.get(0),
            )
            .unwrap();
        let empty_child_content = format!(
            "{{\"timestamp\":\"2026-07-10T03:45:00.000Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"child-session\"}}}}\n{}",
            " ".repeat(1000)
        );
        fs::write(&child_path, empty_child_content).unwrap();
        assert_ne!(fs::metadata(&child_path).unwrap().len(), synced_child_size);
        sync_codex_usage_logs(&mut conn).unwrap();
        let preserved_child_rows: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries WHERE assistant_type = 'codex' AND session_id = 'child-session'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let state_size_after_empty_parse: u64 = conn
            .query_row(
                "SELECT last_synced_size FROM sync_state WHERE filename = ?",
                params![child_state_key],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(preserved_child_rows, 1);
        assert_eq!(state_size_after_empty_parse, synced_child_size);

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
{"type":"user","sessionId":"session-1","cwd":"/tmp/project","version":"2.1.201","timestamp":"2026-07-04T19:28:49.190Z","uuid":"u2","message":{"role":"user","content":"Use monthly grouping"}}
{"type":"assistant","sessionId":"session-1","cwd":"/tmp/project","version":"2.1.201","timestamp":"2026-07-04T19:28:51.753Z","uuid":"a1","requestId":"req_1","message":{"id":"msg_1","role":"assistant","model":"claude-haiku-4-5-20251001","content":[{"type":"thinking","thinking":"working"}],"usage":{"input_tokens":10,"cache_creation_input_tokens":3,"cache_read_input_tokens":7,"output_tokens":5}}}
{"type":"assistant","sessionId":"session-1","cwd":"/tmp/project","version":"2.1.201","timestamp":"2026-07-04T19:28:51.948Z","uuid":"a2","requestId":"req_1","message":{"id":"msg_1","role":"assistant","model":"claude-haiku-4-5-20251001","content":[{"type":"text","text":"Done"}],"usage":{"input_tokens":10,"cache_creation_input_tokens":3,"cache_read_input_tokens":7,"output_tokens":5}}}
"#;

        fs::write(&path, content).unwrap();
        let entries = parse_claude_session_file(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.session_id, "session-1");
        assert_eq!(entry.session_name.as_deref(), Some("Use monthly grouping"));
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

    #[test]
    fn parse_cursor_session_file_uses_last_initial_consecutive_user_prompt_as_name() {
        let path = temp_jsonl_path("cursor-session-name");
        let content = r#"{"role":"user","message":{"content":"第一條提示"}}
{"role":"user","message":{"content":"第二條提示"}}
{"role":"assistant","message":{"content":"收到"}}
{"role":"user","message":{"content":"後續提示"}}
{"role":"assistant","message":{"content":"完成"}}
"#;

        fs::write(&path, content).unwrap();
        let entries = parse_cursor_session_file(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(entries.len(), 2);
        assert!(entries
            .iter()
            .all(|entry| entry.session_name.as_deref() == Some("第二條提示")));
    }

    #[test]
    fn test_parse_cursor_timestamp() {
        let ts = "Wednesday, Jul 8, 2026, 2:24 AM (UTC+8)";
        let parsed = parse_cursor_timestamp(ts);
        assert_eq!(parsed, "2026-07-08T02:24:00+08:00");
    }

    #[test]
    fn codex_parser_migration_clears_all_codex_file_state_once() {
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        conn.execute(
            "INSERT INTO sync_state (filename, last_synced_size, last_synced_time) VALUES ('migration:codex_delta_from_totals_v2', 1, 0)",
            [],
        )
        .unwrap();
        for key in [
            "codex:sessions/2026/07/session.jsonl",
            r"codex:sessions\2026\07\session.jsonl",
        ] {
            conn.execute(
                "INSERT INTO sync_state (filename, last_synced_size, last_synced_time) VALUES (?, 10, 0)",
                params![key],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO sync_state (filename, last_synced_size, last_synced_time) VALUES ('codex:claude:legacy.jsonl', 10, 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, timestamp, date, session_id, turn_no, parent_session_id
             ) VALUES ('codex', '2026-07-10T00:00:00Z', '2026-07-10',
                'codex-session', 1, 'codex-session')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, timestamp, date, session_id, turn_no
             ) VALUES ('antigravity', '2026-07-10T00:00:00Z', '2026-07-10',
                'antigravity-session', 1)",
            [],
        )
        .unwrap();

        run_codex_parser_migration(&mut conn).unwrap();

        let remaining: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_state WHERE filename LIKE 'codex:sessions%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0);

        let codex_entries: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries WHERE assistant_type = 'codex'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let antigravity_entries: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries WHERE assistant_type = 'antigravity'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let codex_parent: Option<String> = conn
            .query_row(
                "SELECT parent_session_id FROM usage_entries WHERE assistant_type = 'codex'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let legacy_claude_state: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_state WHERE filename = 'codex:claude:legacy.jsonl'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(codex_entries, 1);
        assert_eq!(antigravity_entries, 1);
        assert_eq!(codex_parent, None);
        assert_eq!(legacy_claude_state, 1);

        conn.execute(
            "INSERT INTO sync_state (filename, last_synced_size, last_synced_time) VALUES ('codex:sessions/new.jsonl', 10, 0)",
            [],
        )
        .unwrap();
        run_codex_parser_migration(&mut conn).unwrap();
        let state_after_second_run: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_state WHERE filename = 'codex:sessions/new.jsonl'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state_after_second_run, 1);
    }

    #[test]
    fn portable_state_paths_use_forward_slashes() {
        let root = PathBuf::from("root");
        let path = root
            .join("sessions")
            .join("2026")
            .join("07")
            .join("session.jsonl");

        assert_eq!(
            portable_relative_path(&root, &path),
            "sessions/2026/07/session.jsonl"
        );
    }

    #[test]
    fn claude_migration_recognizes_windows_and_unix_transcript_paths() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        for (session_id, transcript_path) in [
            ("windows", r"C:\Users\name\.claude\projects\session.jsonl"),
            ("unix", "/home/name/.claude/projects/session.jsonl"),
        ] {
            conn.execute(
                "INSERT INTO usage_entries (
                    assistant_type, timestamp, date, session_id, turn_no, transcript_path
                 ) VALUES ('codex', '2026-07-10T00:00:00Z', '2026-07-10', ?, 1, ?)",
                params![session_id, transcript_path],
            )
            .unwrap();
        }

        assert_eq!(migrate_legacy_claude_usage_entries(&conn).unwrap(), 2);
        let migrated: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries WHERE assistant_type = 'claude'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(migrated, 2);
    }

    #[test]
    fn sync_copilot_app_usage_logs_inserts_per_turn_rows() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_app_dir = std::env::var("COPILOT_APP_DIR").ok();

        let base_dir = temp_jsonl_path("copilot-app-sync").with_extension("");
        let app_dir = base_dir.join("copilot-app");
        fs::create_dir_all(&app_dir).unwrap();

        // Build session-store.db with two sessions, 3 turns each.
        let session_store = Connection::open(app_dir.join("session-store.db")).unwrap();
        session_store
            .execute(
                "CREATE TABLE assistant_usage_events (
                    id INTEGER PRIMARY KEY,
                    session_id TEXT,
                    turn_index INTEGER,
                    model TEXT,
                    input_tokens INTEGER,
                    output_tokens INTEGER,
                    cache_read_tokens INTEGER,
                    cache_write_tokens INTEGER,
                    reasoning_tokens INTEGER,
                    duration_ms INTEGER,
                    reasoning_effort TEXT,
                    created_at TEXT
                 )",
                [],
            )
            .unwrap();

        let session_a = "app-session-a";
        let session_b = "app-session-b";
        for turn in 0..3i64 {
            for session_id in [session_a, session_b] {
                let id = turn * 2 + if session_id == session_a { 1 } else { 2 };
                let ts = format!("2026-07-20 10:0{}:00", turn);
                session_store
                    .execute(
                        "INSERT INTO assistant_usage_events
                            (id, session_id, turn_index, model,
                             input_tokens, output_tokens,
                             cache_read_tokens, cache_write_tokens,
                             reasoning_tokens, duration_ms,
                             reasoning_effort, created_at)
                         VALUES (?, ?, ?, 'gpt-5', ?, ?, 0, 0, 0, 100, 'medium', ?)",
                        params![id, session_id, turn, (turn + 1) * 100, (turn + 1) * 10, ts,],
                    )
                    .unwrap();
            }
        }

        // Build data.db with session titles.
        let data_db = Connection::open(app_dir.join("data.db")).unwrap();
        data_db
            .execute(
                "CREATE TABLE sessions (
                    id TEXT PRIMARY KEY,
                    title TEXT
                 )",
                [],
            )
            .unwrap();
        data_db
            .execute(
                "INSERT INTO sessions (id, title) VALUES (?, 'Session A'), (?, 'Session B')",
                params![session_a, session_b],
            )
            .unwrap();

        std::env::set_var("COPILOT_APP_DIR", &app_dir);

        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        sync_copilot_app_usage_logs(&mut conn).unwrap();

        let total: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-app'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(total, 6, "expected 6 per-turn rows (2 sessions x 3 turns)");

        // Delta tokens equal per-turn totals (source is per-API-call usage,
        // not cumulative session totals, so no differencing is performed).
        let turn0: (i64, i64) = conn
            .query_row(
                "SELECT tokens_input, delta_input
                 FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-app'
                   AND session_id = ? AND turn_no = 1",
                params![session_a],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            turn0,
            (100, 100),
            "turn 0 delta should equal per-turn total"
        );

        let turn1: (i64, i64) = conn
            .query_row(
                "SELECT tokens_input, delta_input
                 FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-app'
                   AND session_id = ? AND turn_no = 2",
                params![session_a],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            turn1,
            (200, 200),
            "turn 1 delta should equal per-turn total"
        );

        let turn2: (i64, i64) = conn
            .query_row(
                "SELECT tokens_input, delta_input
                 FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-app'
                   AND session_id = ? AND turn_no = 3",
                params![session_a],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            turn2,
            (300, 300),
            "turn 2 delta should equal per-turn total"
        );

        // Verify session title resolved from data.db.
        let title: Option<String> = conn
            .query_row(
                "SELECT session_name FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-app'
                   AND session_id = ? LIMIT 1",
                params![session_b],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(title.as_deref(), Some("Session B"));

        // Verify the cursor was written and is scoped by the canonical source path.
        let cursor_count: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_state
                 WHERE filename LIKE 'sync:copilot_app:cursor:%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(cursor_count, 1);

        let snapshot_before_second: Vec<(String, i64, i64, i64)> = conn
            .prepare(
                "SELECT session_id, turn_no, tokens_input, tokens_total
                 FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-app'
                 ORDER BY session_id, turn_no",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .map(|row| row.unwrap())
            .collect();

        // Second run has no new events: it must be quiet and perform zero
        // upserts, leaving all persisted turn data unchanged.
        sync_copilot_app_usage_logs(&mut conn).unwrap();
        let total_after: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-app'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(total_after, 6, "second sync should not duplicate rows");
        let snapshot_after_second: Vec<(String, i64, i64, i64)> = conn
            .prepare(
                "SELECT session_id, turn_no, tokens_input, tokens_total
                 FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-app'
                 ORDER BY session_id, turn_no",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        assert_eq!(snapshot_after_second, snapshot_before_second);

        if let Some(value) = old_app_dir {
            std::env::set_var("COPILOT_APP_DIR", value);
        } else {
            std::env::remove_var("COPILOT_APP_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    /// Verify that a turn which receives additional API calls after the first
    /// sync is re-aggregated from the full event history and upserted, rather
    /// than being silently dropped by INSERT OR IGNORE.
    #[test]
    fn sync_copilot_app_usage_logs_upserts_turns_with_new_events() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_app_dir = std::env::var("COPILOT_APP_DIR").ok();

        let base_dir = temp_jsonl_path("copilot-app-upsert").with_extension("");
        let app_dir = base_dir.join("copilot-app");
        fs::create_dir_all(&app_dir).unwrap();

        let session_store = Connection::open(app_dir.join("session-store.db")).unwrap();
        session_store
            .execute(
                "CREATE TABLE assistant_usage_events (
                    id INTEGER PRIMARY KEY,
                    session_id TEXT,
                    turn_index INTEGER,
                    model TEXT,
                    input_tokens INTEGER,
                    output_tokens INTEGER,
                    cache_read_tokens INTEGER,
                    cache_write_tokens INTEGER,
                    reasoning_tokens INTEGER,
                    duration_ms INTEGER,
                    reasoning_effort TEXT,
                    created_at TEXT
                 )",
                [],
            )
            .unwrap();

        let session_a = "app-session-a";
        // First API call for turn 0, early timestamp.
        session_store
            .execute(
                "INSERT INTO assistant_usage_events
                    (id, session_id, turn_index, model,
                     input_tokens, output_tokens,
                     cache_read_tokens, cache_write_tokens,
                     reasoning_tokens, duration_ms,
                     reasoning_effort, created_at)
                 VALUES (1, ?, 0, 'gpt-5', 100, 10, 0, 0, 0, 100, 'medium', '2026-07-20 10:00:00')",
                params![session_a],
            )
            .unwrap();

        std::env::set_var("COPILOT_APP_DIR", &app_dir);

        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        sync_copilot_app_usage_logs(&mut conn).unwrap();

        let turn0_total: i64 = conn
            .query_row(
                "SELECT tokens_input FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-app'
                   AND session_id = ? AND turn_no = 1",
                params![session_a],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(turn0_total, 100, "initial turn 0 total should be 100");

        // Second API call for the SAME turn 0, later timestamp.
        session_store
            .execute(
                "INSERT INTO assistant_usage_events
                    (id, session_id, turn_index, model,
                     input_tokens, output_tokens,
                     cache_read_tokens, cache_write_tokens,
                     reasoning_tokens, duration_ms,
                     reasoning_effort, created_at)
                 VALUES (2, ?, 0, 'gpt-5', 250, 20, 0, 0, 0, 100, 'medium', '2026-07-20 10:00:05')",
                params![session_a],
            )
            .unwrap();

        sync_copilot_app_usage_logs(&mut conn).unwrap();

        let turn0_total_after: i64 = conn
            .query_row(
                "SELECT tokens_input FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-app'
                   AND session_id = ? AND turn_no = 1",
                params![session_a],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            turn0_total_after, 350,
            "turn 0 must be re-aggregated to 100 + 250 after upsert"
        );

        let row_count: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-app'
                   AND session_id = ?",
                params![session_a],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(row_count, 1, "no duplicate rows should be created");

        if let Some(value) = old_app_dir {
            std::env::set_var("COPILOT_APP_DIR", value);
        } else {
            std::env::remove_var("COPILOT_APP_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    /// Verify that switching COPILOT_APP_DIR uses an independent cursor and
    /// does not skip earlier events in the new source directory.
    #[test]
    fn sync_copilot_app_usage_logs_cursor_is_scoped_by_source_dir() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_app_dir = std::env::var("COPILOT_APP_DIR").ok();

        let base_dir = temp_jsonl_path("copilot-app-cursor-scope").with_extension("");
        let app_dir_a = base_dir.join("app-a");
        let app_dir_b = base_dir.join("app-b");
        fs::create_dir_all(&app_dir_a).unwrap();
        fs::create_dir_all(&app_dir_b).unwrap();

        let build_store = |dir: &Path| {
            let store = Connection::open(dir.join("session-store.db")).unwrap();
            store
                .execute(
                    "CREATE TABLE assistant_usage_events (
                        id INTEGER PRIMARY KEY,
                        session_id TEXT,
                        turn_index INTEGER,
                        model TEXT,
                        input_tokens INTEGER,
                        output_tokens INTEGER,
                        cache_read_tokens INTEGER,
                        cache_write_tokens INTEGER,
                        reasoning_tokens INTEGER,
                        duration_ms INTEGER,
                        reasoning_effort TEXT,
                        created_at TEXT
                     )",
                    [],
                )
                .unwrap();
            store
        };

        let store_a = build_store(&app_dir_a);
        let store_b = build_store(&app_dir_b);

        // Directory A: one turn at 2026-07-20 10:00:00.
        store_a
            .execute(
                "INSERT INTO assistant_usage_events
                    (id, session_id, turn_index, model,
                     input_tokens, output_tokens,
                     cache_read_tokens, cache_write_tokens,
                     reasoning_tokens, duration_ms,
                     reasoning_effort, created_at)
                 VALUES (1, 'sess-a', 0, 'gpt-5', 100, 10, 0, 0, 0, 100, 'medium', '2026-07-20 10:00:00')",
                [],
            )
            .unwrap();

        // Directory B: one turn at an EARLIER timestamp than A's cursor would be.
        store_b
            .execute(
                "INSERT INTO assistant_usage_events
                    (id, session_id, turn_index, model,
                     input_tokens, output_tokens,
                     cache_read_tokens, cache_write_tokens,
                     reasoning_tokens, duration_ms,
                     reasoning_effort, created_at)
                 VALUES (1, 'sess-b', 0, 'gpt-5', 50, 5, 0, 0, 0, 100, 'medium', '2026-07-19 09:00:00')",
                [],
            )
            .unwrap();

        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        // Sync from A first; this establishes a cursor at 2026-07-20 10:00:00.
        std::env::set_var("COPILOT_APP_DIR", &app_dir_a);
        sync_copilot_app_usage_logs(&mut conn).unwrap();

        // Switch to B. A correct scoped cursor must NOT reuse A's cursor; B's
        // earlier event must still be ingested.
        std::env::set_var("COPILOT_APP_DIR", &app_dir_b);
        sync_copilot_app_usage_logs(&mut conn).unwrap();

        let b_total: i64 = conn
            .query_row(
                "SELECT tokens_input FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-app'
                   AND session_id = 'sess-b'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(b_total, 50, "directory B's earlier event must be ingested");

        // Both cursors should coexist (one per source directory).
        let cursor_count: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_state
                 WHERE filename LIKE 'sync:copilot_app:cursor:%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            cursor_count, 2,
            "each source directory must have its own cursor"
        );

        if let Some(value) = old_app_dir {
            std::env::set_var("COPILOT_APP_DIR", value);
        } else {
            std::env::remove_var("COPILOT_APP_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    /// Events can share a timestamp, so the event id must be part of both the
    /// ordering and the high-water mark.
    #[test]
    fn sync_copilot_app_usage_logs_imports_same_timestamp_events_once() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_app_dir = std::env::var("COPILOT_APP_DIR").ok();

        let base_dir = temp_jsonl_path("copilot-app-same-timestamp").with_extension("");
        let app_dir = base_dir.join("copilot-app");
        fs::create_dir_all(&app_dir).unwrap();
        let session_store = Connection::open(app_dir.join("session-store.db")).unwrap();
        session_store
            .execute(
                "CREATE TABLE assistant_usage_events (
                    id INTEGER PRIMARY KEY,
                    session_id TEXT,
                    turn_index INTEGER,
                    model TEXT,
                    input_tokens INTEGER,
                    output_tokens INTEGER,
                    cache_read_tokens INTEGER,
                    cache_write_tokens INTEGER,
                    reasoning_tokens INTEGER,
                    duration_ms INTEGER,
                    reasoning_effort TEXT,
                    created_at TEXT
                 )",
                [],
            )
            .unwrap();

        for (id, session_id, turn_index, input) in [
            (1i64, "same-ts", 0i64, 10i64),
            (2, "same-ts", 1, 20),
            (3, "same-ts", 0, 30),
        ] {
            session_store
                .execute(
                    "INSERT INTO assistant_usage_events
                        (id, session_id, turn_index, model, input_tokens,
                         output_tokens, cache_read_tokens, cache_write_tokens,
                         reasoning_tokens, duration_ms, reasoning_effort, created_at)
                     VALUES (?, ?, ?, 'gpt-5', ?, 1, 0, 0, 0, 100, 'medium',
                             '2026-07-20 10:00:00')",
                    params![id, session_id, turn_index, input],
                )
                .unwrap();
        }

        std::env::set_var("COPILOT_APP_DIR", &app_dir);
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        sync_copilot_app_usage_logs(&mut conn).unwrap();
        let cursor: String = conn
            .query_row(
                "SELECT filename FROM sync_state
                 WHERE filename LIKE 'sync:copilot_app:cursor:%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(cursor.ends_with("::2026-07-20 10:00:00::3"));

        let first_snapshot: Vec<(i64, i64)> = conn
            .prepare(
                "SELECT turn_no, tokens_input FROM usage_entries
                 WHERE source_kind = 'copilot-app' ORDER BY turn_no",
            )
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        assert_eq!(first_snapshot, vec![(1, 40), (2, 20)]);

        sync_copilot_app_usage_logs(&mut conn).unwrap();
        let second_snapshot: Vec<(i64, i64)> = conn
            .prepare(
                "SELECT turn_no, tokens_input FROM usage_entries
                 WHERE source_kind = 'copilot-app' ORDER BY turn_no",
            )
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        assert_eq!(second_snapshot, first_snapshot);

        if let Some(value) = old_app_dir {
            std::env::set_var("COPILOT_APP_DIR", value);
        } else {
            std::env::remove_var("COPILOT_APP_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    /// A timestamp-only cursor must re-scan its timestamp boundary once and
    /// then persist the upgraded tuple cursor without recurring re-syncs.
    #[test]
    fn sync_copilot_app_usage_logs_upgrades_legacy_timestamp_cursor() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_app_dir = std::env::var("COPILOT_APP_DIR").ok();

        let base_dir = temp_jsonl_path("copilot-app-legacy-cursor").with_extension("");
        let app_dir = base_dir.join("copilot-app");
        fs::create_dir_all(&app_dir).unwrap();
        let session_store = Connection::open(app_dir.join("session-store.db")).unwrap();
        session_store
            .execute(
                "CREATE TABLE assistant_usage_events (
                    id INTEGER PRIMARY KEY,
                    session_id TEXT,
                    turn_index INTEGER,
                    model TEXT,
                    input_tokens INTEGER,
                    output_tokens INTEGER,
                    cache_read_tokens INTEGER,
                    cache_write_tokens INTEGER,
                    reasoning_tokens INTEGER,
                    duration_ms INTEGER,
                    reasoning_effort TEXT,
                    created_at TEXT
                 )",
                [],
            )
            .unwrap();
        session_store
            .execute(
                "INSERT INTO assistant_usage_events
                    (id, session_id, turn_index, model, input_tokens,
                     output_tokens, cache_read_tokens, cache_write_tokens,
                     reasoning_tokens, duration_ms, reasoning_effort, created_at)
                 VALUES (1, 'legacy-sess', 0, 'gpt-5', 10, 1, 0, 0, 0, 100,
                         'medium', '2026-07-20 10:00:00'),
                        (2, 'legacy-sess', 1, 'gpt-5', 20, 2, 0, 0, 0, 100,
                         'medium', '2026-07-20 10:00:00'),
                        (3, 'legacy-sess', 0, 'gpt-5', 30, 3, 0, 0, 0, 100,
                         'medium', '2026-07-20 10:05:00')",
                [],
            )
            .unwrap();

        std::env::set_var("COPILOT_APP_DIR", &app_dir);
        let canonical_app_dir = app_dir.canonicalize().unwrap();
        let source_key = encode_hex(canonical_app_dir.as_os_str().as_encoded_bytes());
        let cursor_prefix = format!("{}{}::", COPILOT_APP_CURSOR_PREFIX, source_key);
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        conn.execute(
            "INSERT INTO sync_state (filename, last_synced_size, last_synced_time)
             VALUES (?, 0, 0)",
            params![format!("{}2026-07-20 10:00:00", cursor_prefix)],
        )
        .unwrap();

        sync_copilot_app_usage_logs(&mut conn).unwrap();
        let cursor: String = conn
            .query_row(
                "SELECT filename FROM sync_state WHERE filename LIKE 'sync:copilot_app:cursor:%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(cursor.ends_with("::2026-07-20 10:05:00::3"));

        let totals: (i64, i64, i64) = conn
            .query_row(
                "SELECT COUNT(*),
                        (SELECT tokens_input FROM usage_entries WHERE turn_no = 1),
                        (SELECT tokens_input FROM usage_entries WHERE turn_no = 2)
                 FROM usage_entries WHERE source_kind = 'copilot-app'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(totals, (2, 40, 20));

        sync_copilot_app_usage_logs(&mut conn).unwrap();
        let count_after_second: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries WHERE source_kind = 'copilot-app'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count_after_second, 2);

        if let Some(value) = old_app_dir {
            std::env::set_var("COPILOT_APP_DIR", value);
        } else {
            std::env::remove_var("COPILOT_APP_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    /// Verify that cache-read tokens are not double-counted.
    /// `assistant_usage_events.input_tokens` already includes cache reads, so
    /// `tokens_input` must be normalized to `input - cache_read`, and
    /// `tokens_total` must count `cache_read` only once (via its own column).
    #[test]
    fn sync_copilot_app_usage_logs_separates_cached_input() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_app_dir = std::env::var("COPILOT_APP_DIR").ok();

        let base_dir = temp_jsonl_path("copilot-app-cache").with_extension("");
        let app_dir = base_dir.join("copilot-app");
        fs::create_dir_all(&app_dir).unwrap();

        let session_store = Connection::open(app_dir.join("session-store.db")).unwrap();
        session_store
            .execute(
                "CREATE TABLE assistant_usage_events (
                    id INTEGER PRIMARY KEY,
                    session_id TEXT,
                    turn_index INTEGER,
                    model TEXT,
                    input_tokens INTEGER,
                    output_tokens INTEGER,
                    cache_read_tokens INTEGER,
                    cache_write_tokens INTEGER,
                    reasoning_tokens INTEGER,
                    duration_ms INTEGER,
                    reasoning_effort TEXT,
                    created_at TEXT
                 )",
                [],
            )
            .unwrap();

        // One turn with input=443_554 (includes 401_024 cache reads),
        // output=1_370, reasoning=384. Mirror the Copilot CLI normalization
        // fixture: net input should be 42_530, total should be 444_924.
        session_store
            .execute(
                "INSERT INTO assistant_usage_events
                    (id, session_id, turn_index, model,
                     input_tokens, output_tokens,
                     cache_read_tokens, cache_write_tokens,
                     reasoning_tokens, duration_ms,
                     reasoning_effort, created_at)
                 VALUES (1, 'sess-c', 0, 'gpt-5', 443554, 1370, 401024, 0, 384, 100, 'medium', '2026-07-20 10:00:00')",
                [],
            )
            .unwrap();

        std::env::set_var("COPILOT_APP_DIR", &app_dir);

        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        sync_copilot_app_usage_logs(&mut conn).unwrap();

        let row: (i64, i64, i64, i64, i64, i64, i64) = conn
            .query_row(
                "SELECT tokens_input, tokens_cache_read, tokens_output, tokens_reasoning,
                        tokens_total, delta_input, delta_total
                 FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-app'
                   AND session_id = 'sess-c' AND turn_no = 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )
            .unwrap();

        assert_eq!(row.0, 42_530, "tokens_input must exclude cache_read");
        assert_eq!(
            row.1, 401_024,
            "tokens_cache_read keeps the raw cache total"
        );
        assert_eq!(row.2, 1_370, "tokens_output");
        assert_eq!(row.3, 384, "tokens_reasoning");
        // total = net_input + cache_read + output + reasoning = 42_530 + 401_024 + 1_370 + 384
        assert_eq!(row.4, 445_308, "tokens_total counts cache_read once");
        assert_eq!(row.5, 42_530, "delta_input must also exclude cache_read");
        assert_eq!(row.6, 445_308, "delta_total must match tokens_total");

        if let Some(value) = old_app_dir {
            std::env::set_var("COPILOT_APP_DIR", value);
        } else {
            std::env::remove_var("COPILOT_APP_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    /// Verify the cursor advances to the max raw event `(created_at, id)`, not
    /// the per-turn MIN, so a turn whose events straddle the cursor does not
    /// get re-aggregated forever on subsequent syncs.
    #[test]
    fn sync_copilot_app_usage_logs_cursor_advances_to_max_event_ts() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_app_dir = std::env::var("COPILOT_APP_DIR").ok();

        let base_dir = temp_jsonl_path("copilot-app-cursor-max").with_extension("");
        let app_dir = base_dir.join("copilot-app");
        fs::create_dir_all(&app_dir).unwrap();

        let session_store = Connection::open(app_dir.join("session-store.db")).unwrap();
        session_store
            .execute(
                "CREATE TABLE assistant_usage_events (
                    id INTEGER PRIMARY KEY,
                    session_id TEXT,
                    turn_index INTEGER,
                    model TEXT,
                    input_tokens INTEGER,
                    output_tokens INTEGER,
                    cache_read_tokens INTEGER,
                    cache_write_tokens INTEGER,
                    reasoning_tokens INTEGER,
                    duration_ms INTEGER,
                    reasoning_effort TEXT,
                    created_at TEXT
                 )",
                [],
            )
            .unwrap();

        let session_a = "sess-a";
        // Turn 0 has two events at 10:00 and 10:05.
        session_store
            .execute(
                "INSERT INTO assistant_usage_events
                    (id, session_id, turn_index, model,
                     input_tokens, output_tokens,
                     cache_read_tokens, cache_write_tokens,
                     reasoning_tokens, duration_ms,
                     reasoning_effort, created_at)
                 VALUES (1, ?, 0, 'gpt-5', 100, 10, 0, 0, 0, 100, 'medium', '2026-07-20 10:00:00')",
                params![session_a],
            )
            .unwrap();
        session_store
            .execute(
                "INSERT INTO assistant_usage_events
                    (id, session_id, turn_index, model,
                     input_tokens, output_tokens,
                     cache_read_tokens, cache_write_tokens,
                     reasoning_tokens, duration_ms,
                     reasoning_effort, created_at)
                 VALUES (2, ?, 0, 'gpt-5', 200, 20, 0, 0, 0, 100, 'medium', '2026-07-20 10:05:00')",
                params![session_a],
            )
            .unwrap();

        std::env::set_var("COPILOT_APP_DIR", &app_dir);

        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        sync_copilot_app_usage_logs(&mut conn).unwrap();

        // The cursor must be at the max raw event tuple, not the per-turn MIN.
        let cursor: String = conn
            .query_row(
                "SELECT filename FROM sync_state
                 WHERE filename LIKE 'sync:copilot_app:cursor:%' LIMIT 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        assert!(
            cursor.ends_with("::2026-07-20 10:05:00::2"),
            "cursor must advance to max raw event tuple, got: {}",
            cursor
        );

        // A second sync with NO new events is quiet: the turn straddling the
        // old timestamp is not re-aggregated, so the total stays at 300.
        sync_copilot_app_usage_logs(&mut conn).unwrap();
        let row_count: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-app'
                   AND session_id = ?",
                params![session_a],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(row_count, 1, "no duplicate rows after idempotent re-sync");
        let total_input: i64 = conn
            .query_row(
                "SELECT tokens_input FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-app'
                   AND session_id = ? AND turn_no = 1",
                params![session_a],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            total_input, 300,
            "turn 0 total should remain 300 after idempotent re-sync"
        );

        if let Some(value) = old_app_dir {
            std::env::set_var("COPILOT_APP_DIR", value);
        } else {
            std::env::remove_var("COPILOT_APP_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    /// Verify the cursor and usage rows do NOT change when a turn fails to write, so the
    /// failed turn is retried on the next sync instead of being permanently
    /// skipped. We simulate a write failure by installing a trigger on
    /// `usage_entries` that rejects inserts for `copilot-app` source_kind.
    #[test]
    fn sync_copilot_app_usage_logs_cursor_rollback_on_write_failure() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_app_dir = std::env::var("COPILOT_APP_DIR").ok();

        let base_dir = temp_jsonl_path("copilot-app-cursor-rollback").with_extension("");
        let app_dir = base_dir.join("copilot-app");
        fs::create_dir_all(&app_dir).unwrap();

        let session_store = Connection::open(app_dir.join("session-store.db")).unwrap();
        session_store
            .execute(
                "CREATE TABLE assistant_usage_events (
                    id INTEGER PRIMARY KEY,
                    session_id TEXT,
                    turn_index INTEGER,
                    model TEXT,
                    input_tokens INTEGER,
                    output_tokens INTEGER,
                    cache_read_tokens INTEGER,
                    cache_write_tokens INTEGER,
                    reasoning_tokens INTEGER,
                    duration_ms INTEGER,
                    reasoning_effort TEXT,
                    created_at TEXT
                 )",
                [],
            )
            .unwrap();

        let session_a = "sess-a";
        session_store
            .execute(
                "INSERT INTO assistant_usage_events
                    (id, session_id, turn_index, model,
                     input_tokens, output_tokens,
                     cache_read_tokens, cache_write_tokens,
                     reasoning_tokens, duration_ms,
                     reasoning_effort, created_at)
                 VALUES (1, ?, 0, 'gpt-5', 100, 10, 0, 0, 0, 100, 'medium', '2026-07-20 10:00:00')",
                params![session_a],
            )
            .unwrap();

        std::env::set_var("COPILOT_APP_DIR", &app_dir);

        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        // First sync succeeds and establishes a cursor at 10:00:00.
        sync_copilot_app_usage_logs(&mut conn).unwrap();
        let cursor_after_first: String = conn
            .query_row(
                "SELECT filename FROM sync_state
                 WHERE filename LIKE 'sync:copilot_app:cursor:%' LIMIT 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        assert!(
            cursor_after_first.ends_with("::2026-07-20 10:00:00::1"),
            "cursor should be at 10:00:00 after first sync, got: {}",
            cursor_after_first
        );

        // Install a trigger that rejects new inserts for copilot-app rows,
        // simulating a persistent write failure (e.g. schema drift, disk).
        conn.execute(
            "CREATE TRIGGER reject_copilot_app_insert
             BEFORE INSERT ON usage_entries
             WHEN NEW.source_kind = 'copilot-app'
             BEGIN
                 SELECT RAFAIL('simulated write failure');
             END",
            [],
        )
        .unwrap();

        // Add a new event at 10:05 so the touched-turns query returns a row that
        // will fail to upsert.
        session_store
            .execute(
                "INSERT INTO assistant_usage_events
                    (id, session_id, turn_index, model,
                     input_tokens, output_tokens,
                     cache_read_tokens, cache_write_tokens,
                     reasoning_tokens, duration_ms,
                     reasoning_effort, created_at)
                 VALUES (2, ?, 1, 'gpt-5', 200, 20, 0, 0, 0, 100, 'medium', '2026-07-20 10:05:00')",
                params![session_a],
            )
            .unwrap();

        sync_copilot_app_usage_logs(&mut conn).unwrap();

        // Cursor must NOT have advanced to 10:05 because the upsert failed; it
        // must remain at 10:00:00 so the turn is retried next sync.
        let usage_count_after_failure: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-app'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(usage_count_after_failure, 1, "failed upsert must rollback");
        let cursor_after_failure: String = conn
            .query_row(
                "SELECT filename FROM sync_state
                 WHERE filename LIKE 'sync:copilot_app:cursor:%' LIMIT 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        assert!(
            cursor_after_failure.ends_with("::2026-07-20 10:00:00::1"),
            "cursor must not advance on write failure, got: {}",
            cursor_after_failure
        );

        if let Some(value) = old_app_dir {
            std::env::set_var("COPILOT_APP_DIR", value);
        } else {
            std::env::remove_var("COPILOT_APP_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    /// Verify import_source_id includes the source directory key so turns from
    /// different COPILOT_APP_DIR with the same (session_id, turn_index) do not
    /// share a dedup key.
    #[test]
    fn sync_copilot_app_usage_logs_import_source_id_includes_source_dir() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_app_dir = std::env::var("COPILOT_APP_DIR").ok();

        let base_dir = temp_jsonl_path("copilot-app-import-src").with_extension("");
        let app_dir = base_dir.join("copilot-app");
        fs::create_dir_all(&app_dir).unwrap();

        let session_store = Connection::open(app_dir.join("session-store.db")).unwrap();
        session_store
            .execute(
                "CREATE TABLE assistant_usage_events (
                    id INTEGER PRIMARY KEY,
                    session_id TEXT,
                    turn_index INTEGER,
                    model TEXT,
                    input_tokens INTEGER,
                    output_tokens INTEGER,
                    cache_read_tokens INTEGER,
                    cache_write_tokens INTEGER,
                    reasoning_tokens INTEGER,
                    duration_ms INTEGER,
                    reasoning_effort TEXT,
                    created_at TEXT
                 )",
                [],
            )
            .unwrap();

        session_store
            .execute(
                "INSERT INTO assistant_usage_events
                    (id, session_id, turn_index, model,
                     input_tokens, output_tokens,
                     cache_read_tokens, cache_write_tokens,
                     reasoning_tokens, duration_ms,
                     reasoning_effort, created_at)
                 VALUES (1, 'sess-x', 0, 'gpt-5', 100, 10, 0, 0, 0, 100, 'medium', '2026-07-20 10:00:00')",
                [],
            )
            .unwrap();

        std::env::set_var("COPILOT_APP_DIR", &app_dir);

        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        sync_copilot_app_usage_logs(&mut conn).unwrap();

        let import_source_id: String = conn
            .query_row(
                "SELECT import_source_id FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-app'
                   AND session_id = 'sess-x' AND turn_no = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();

        // import_source_id must be copilot-app:<hex_source_key>:sess-x:0.
        assert!(
            import_source_id.starts_with("copilot-app:"),
            "import_source_id must start with copilot-app: prefix, got: {}",
            import_source_id
        );
        let rest = &import_source_id["copilot-app:".len()..];
        // The remainder is <hex>:sess-x:0; the hex segment is the first colon-
        // delimited component and must be non-empty hex.
        let hex_segment = rest.split(':').next().unwrap_or("");
        assert!(
            !hex_segment.is_empty() && hex_segment.chars().all(|c| c.is_ascii_hexdigit()),
            "import_source_id must include a non-empty hex source key, got: {}",
            import_source_id
        );
        assert!(
            import_source_id.ends_with(":sess-x:0"),
            "import_source_id must end with :sess-x:0, got: {}",
            import_source_id
        );

        if let Some(value) = old_app_dir {
            std::env::set_var("COPILOT_APP_DIR", value);
        } else {
            std::env::remove_var("COPILOT_APP_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    /// Verify that two different COPILOT_APP_DIR with the same (session_id,
    /// turn_index) do not overwrite each other. The unique index now includes
    /// source_dir_key, so each directory keeps its own row.
    #[test]
    fn sync_copilot_app_usage_logs_isolates_rows_by_source_dir() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_app_dir = std::env::var("COPILOT_APP_DIR").ok();

        let base_dir = temp_jsonl_path("copilot-app-isolate").with_extension("");
        let app_dir_a = base_dir.join("app-a");
        let app_dir_b = base_dir.join("app-b");
        fs::create_dir_all(&app_dir_a).unwrap();
        fs::create_dir_all(&app_dir_b).unwrap();

        let build_store = |dir: &Path| {
            let store = Connection::open(dir.join("session-store.db")).unwrap();
            store
                .execute(
                    "CREATE TABLE assistant_usage_events (
                        id INTEGER PRIMARY KEY,
                        session_id TEXT,
                        turn_index INTEGER,
                        model TEXT,
                        input_tokens INTEGER,
                        output_tokens INTEGER,
                        cache_read_tokens INTEGER,
                        cache_write_tokens INTEGER,
                        reasoning_tokens INTEGER,
                        duration_ms INTEGER,
                        reasoning_effort TEXT,
                        created_at TEXT
                     )",
                    [],
                )
                .unwrap();
            store
        };

        let store_a = build_store(&app_dir_a);
        let store_b = build_store(&app_dir_b);

        // Both directories have the SAME session_id and turn_index, but
        // different token counts so we can tell them apart.
        store_a
            .execute(
                "INSERT INTO assistant_usage_events
                    (id, session_id, turn_index, model,
                     input_tokens, output_tokens,
                     cache_read_tokens, cache_write_tokens,
                     reasoning_tokens, duration_ms,
                     reasoning_effort, created_at)
                 VALUES (1, 'shared-sess', 0, 'gpt-5', 100, 10, 0, 0, 0, 100, 'medium', '2026-07-20 10:00:00')",
                [],
            )
            .unwrap();
        store_b
            .execute(
                "INSERT INTO assistant_usage_events
                    (id, session_id, turn_index, model,
                     input_tokens, output_tokens,
                     cache_read_tokens, cache_write_tokens,
                     reasoning_tokens, duration_ms,
                     reasoning_effort, created_at)
                 VALUES (1, 'shared-sess', 0, 'gpt-5', 200, 20, 0, 0, 0, 100, 'medium', '2026-07-20 09:00:00')",
                [],
            )
            .unwrap();

        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        // Sync A first.
        std::env::set_var("COPILOT_APP_DIR", &app_dir_a);
        sync_copilot_app_usage_logs(&mut conn).unwrap();

        // Sync B (same session_id, same turn). Must NOT overwrite A's row.
        std::env::set_var("COPILOT_APP_DIR", &app_dir_b);
        sync_copilot_app_usage_logs(&mut conn).unwrap();

        // Both rows must coexist.
        let row_count: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-app'
                   AND session_id = 'shared-sess' AND turn_no = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            row_count, 2,
            "two source dirs with same session/turn must each keep their own row"
        );

        // Verify token totals are distinct (A=100, B=200) and not overwritten.
        let totals: Vec<i64> = conn
            .prepare(
                "SELECT tokens_input FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-app'
                   AND session_id = 'shared-sess' AND turn_no = 1
                 ORDER BY tokens_input",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(
            totals,
            vec![100, 200],
            "both directories' rows must be present with their own totals"
        );

        if let Some(value) = old_app_dir {
            std::env::set_var("COPILOT_APP_DIR", value);
        } else {
            std::env::remove_var("COPILOT_APP_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    /// Verify init_db migrates legacy copilot-app rows (old import_source_id
    /// format, NULL source_dir_key) by deleting them so they do not coexist
    /// with new rows and cause double-counting.
    #[test]
    fn init_db_migrates_legacy_copilot_app_rows() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        // Insert a legacy copilot-app row (old import_source_id format, no
        // source_dir_key).
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, source_dir_key, timestamp, date,
                session_id, turn_no, import_source_id
             ) VALUES (
                'copilot', 'copilot-app', NULL, '2026-07-01T10:00:00Z', '2026-07-01',
                'legacy-sess', 1, 'copilot-app:legacy-sess:0'
             )",
            [],
        )
        .unwrap();

        // Insert a non-copilot-app row to ensure it is NOT deleted.
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, source_dir_key, timestamp, date,
                session_id, turn_no, import_source_id
             ) VALUES (
                'codex', 'legacy', NULL, '2026-07-01T10:00:00Z', '2026-07-01',
                'codex-sess', 1, 'codex-import-1'
             )",
            [],
        )
        .unwrap();

        // Re-run init_db to trigger migration.
        init_db(&conn).unwrap();

        let legacy_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries
                 WHERE source_kind = 'copilot-app' AND source_dir_key IS NULL
                   AND import_source_id = 'copilot-app:legacy-sess:0'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(legacy_count, 0, "legacy copilot-app row must be deleted");

        let codex_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries WHERE session_id = 'codex-sess'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(codex_count, 1, "non-copilot-app row must be preserved");
    }

    /// Verify that non-copilot-app collectors (codex, claude, cursor) retain
    /// their uniqueness after the partial index change. Two identical
    /// (assistant_type, source_kind, session_id, turn_no) rows with NULL
    /// source_dir_key must not coexist.
    #[test]
    fn init_db_partial_index_preserves_non_copilot_uniqueness() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        // Insert a codex row.
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, source_dir_key, timestamp, date,
                session_id, turn_no
             ) VALUES ('codex', 'legacy', NULL, '2026-07-01T10:00:00Z', '2026-07-01', 'codex-sess', 1)",
            [],
        )
        .unwrap();

        // Attempt to insert a duplicate codex row with the same identity. This
        // should fail (or be a no-op via INSERT OR IGNORE) because the partial
        // unique index WHERE source_dir_key IS NULL enforces uniqueness.
        let dup_result = conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, source_dir_key, timestamp, date,
                session_id, turn_no
             ) VALUES ('codex', 'legacy', NULL, '2026-07-01T11:00:00Z', '2026-07-01', 'codex-sess', 1)",
            [],
        );

        assert!(
            dup_result.is_err(),
            "duplicate non-copilot-app row must be rejected by partial unique index"
        );

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries WHERE session_id = 'codex-sess'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "only one codex row should exist");
    }

    /// Verify that two copilot-app sources with the same session_id are not
    /// merged in the daily summary aggregation. Each source should appear as
    /// a separate session.
    #[test]
    fn daily_summary_separates_same_session_id_across_source_dirs() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_app_dir = std::env::var("COPILOT_APP_DIR").ok();

        let base_dir = temp_jsonl_path("copilot-app-daily-merge").with_extension("");
        let app_dir_a = base_dir.join("app-a");
        let app_dir_b = base_dir.join("app-b");
        fs::create_dir_all(&app_dir_a).unwrap();
        fs::create_dir_all(&app_dir_b).unwrap();

        let build_store = |dir: &Path| {
            let store = Connection::open(dir.join("session-store.db")).unwrap();
            store
                .execute(
                    "CREATE TABLE assistant_usage_events (
                        id INTEGER PRIMARY KEY,
                        session_id TEXT,
                        turn_index INTEGER,
                        model TEXT,
                        input_tokens INTEGER,
                        output_tokens INTEGER,
                        cache_read_tokens INTEGER,
                        cache_write_tokens INTEGER,
                        reasoning_tokens INTEGER,
                        duration_ms INTEGER,
                        reasoning_effort TEXT,
                        created_at TEXT
                     )",
                    [],
                )
                .unwrap();
            store
        };

        let store_a = build_store(&app_dir_a);
        let store_b = build_store(&app_dir_b);

        // Both directories use the SAME session_id but different token counts.
        store_a
            .execute(
                "INSERT INTO assistant_usage_events
                    (id, session_id, turn_index, model,
                     input_tokens, output_tokens,
                     cache_read_tokens, cache_write_tokens,
                     reasoning_tokens, duration_ms,
                     reasoning_effort, created_at)
                 VALUES (1, 'shared-sess', 0, 'gpt-5', 100, 10, 0, 0, 0, 100, 'medium', '2026-07-20 10:00:00')",
                [],
            )
            .unwrap();
        store_b
            .execute(
                "INSERT INTO assistant_usage_events
                    (id, session_id, turn_index, model,
                     input_tokens, output_tokens,
                     cache_read_tokens, cache_write_tokens,
                     reasoning_tokens, duration_ms,
                     reasoning_effort, created_at)
                 VALUES (1, 'shared-sess', 0, 'gpt-5', 200, 20, 0, 0, 0, 100, 'medium', '2026-07-20 09:00:00')",
                [],
            )
            .unwrap();

        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        std::env::set_var("COPILOT_APP_DIR", &app_dir_a);
        sync_copilot_app_usage_logs(&mut conn).unwrap();
        std::env::set_var("COPILOT_APP_DIR", &app_dir_b);
        sync_copilot_app_usage_logs(&mut conn).unwrap();

        // Fetch entries for the date and verify the two sources are NOT merged.
        let entries = get_usage_entries_by_date(&conn, "2026-07-20", "copilot").unwrap();

        // Group by (session_id, source_dir_key) to simulate daily summary logic.
        let mut sessions: HashMap<(String, Option<String>), Vec<i64>> = HashMap::new();
        for (record, _ast) in &entries {
            let e = &record.entry;
            let key = (e.session_id.clone(), e.source_dir_key.clone());
            sessions
                .entry(key)
                .or_default()
                .push(e.tokens.as_ref().map(|t| t.input as i64).unwrap_or(0));
        }

        // There must be 2 separate sessions (one per source dir), not 1 merged.
        assert_eq!(
            sessions.len(),
            2,
            "two source dirs with same session_id must be 2 separate sessions, not merged"
        );

        // Verify the token totals are distinct (100 and 200, not 300 merged).
        let mut all_totals: Vec<i64> = sessions.values().map(|v| v[0]).collect();
        all_totals.sort();
        assert_eq!(
            all_totals,
            vec![100, 200],
            "each session keeps its own tokens"
        );

        // Verify source_kind is "copilot-app" for all entries so the frontend
        // renders the App badge, not the CLI fallback.
        for (record, _ast) in &entries {
            let e = &record.entry;
            assert_eq!(
                e.source_kind.as_deref(),
                Some("copilot-app"),
                "copilot-app entries must have source_kind = 'copilot-app' for correct frontend badge"
            );
        }

        if let Some(value) = old_app_dir {
            std::env::set_var("COPILOT_APP_DIR", value);
        } else {
            std::env::remove_var("COPILOT_APP_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }
}
