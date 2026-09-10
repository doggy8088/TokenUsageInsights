use rusqlite::{params, Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TokenStats {
    pub input: u64,
    pub output: u64,
    pub cache_read: Option<u64>,
    pub cache_write: Option<u64>,
    #[serde(default)]
    pub cache_write_5m: Option<u64>,
    #[serde(default)]
    pub cache_write_1h: Option<u64>,
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
    #[serde(default)]
    pub reported_cost_usd: Option<f64>,
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
    pub batch_id: String,
}

#[derive(Debug, Clone, Default)]
pub struct UsageImportMetadata {
    pub source_assistant: Option<String>,
    pub source_file_name: Option<String>,
}

#[derive(Serialize, Debug, Clone)]
pub struct UsageImportBatch {
    pub id: String,
    pub assistant: String,
    pub source_assistant: Option<String>,
    pub source_file_name: Option<String>,
    pub date: String,
    pub total: usize,
    pub imported: usize,
    pub skipped_duplicates: usize,
    pub created_at: i64,
    pub rolled_back_at: Option<i64>,
    pub removed_records: usize,
}

#[derive(Serialize, Debug, Clone)]
pub struct UsageImportRollbackSummary {
    pub batch_id: String,
    pub removed_records: usize,
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
    #[serde(default)]
    cache_creation: ClaudeCacheCreation,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ClaudeCacheCreation {
    #[serde(default)]
    ephemeral_5m_input_tokens: u64,
    #[serde(default)]
    ephemeral_1h_input_tokens: u64,
}

// Codex helper structs shared by the CLI and Desktop session formats.
#[derive(Debug, Clone, Default, Deserialize)]
struct CodexTokenUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    cached_input_tokens: u64,
    #[serde(default)]
    cache_write_input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    reasoning_output_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
}

const CODEX_PARSER_MIGRATION_KEY: &str = "migration:codex_session_identity_v7";
const CODEX_SOURCE_KIND_MIGRATION_KEY: &str = "migration:codex_source_kind_v1";
const CODEX_CLI_SOURCE_KIND: &str = "codex-cli";
const CODEX_DESKTOP_SOURCE_KIND: &str = "codex-desktop";
const CODEX_OTHER_SOURCE_KIND: &str = "codex-other";
const CODEX_EMPTY_TRANSCRIPT_SYNC_TIME: i64 = -1;
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
const CLAUDE_CACHE_WRITE_PRICING_MIGRATION_KEY: &str = "migration:claude_cache_write_pricing_v1";
const SESSION_NAME_SELECTION_MIGRATION_KEY: &str = "migration:session_name_selection_v1";
static IMPORT_BATCH_COUNTER: AtomicU64 = AtomicU64::new(0);
const CURSOR_MODEL_ATTRIBUTION_MIGRATION_KEY: &str = "migration:cursor_model_attribution_v2";
const CURSOR_CACHE_TOKENS_UNKNOWN_MIGRATION_KEY: &str = "migration:cursor_cache_tokens_unknown_v1";
const CURSOR_AGENT_SOURCE_KIND: &str = "cursor-agent";
const CURSOR_IDE_SOURCE_KIND: &str = "cursor-ide";
const GROK_PARSER_MIGRATION_KEY: &str = "migration:grok_parser_v6";
const LEGACY_GROK_PARSER_MIGRATION_KEYS: &[&str] = &[
    "migration:grok_parser_v1",
    "migration:grok_model_normalization_v2",
    "migration:grok_parser_v3",
    "migration:grok_parser_v4",
    "migration:grok_parser_v5",
];

/// Source kind written for usage entries originating from the Copilot CLI
/// status-line hook (`~/.copilot/usage/usage-YYYY-MM-DD.jsonl`). The hook
/// records session-cumulative totals with no `agent_id`, so subagent usage is
/// folded into the main session. The CLI agent reconciler
/// ([`sync_copilot_cli_agent_usage_logs`]) replaces these merged rows with
/// per-agent split rows (same `source_kind`, distinct `import_source_id`
/// namespace) when `session-store.db` provides per-agent attribution.
const COPILOT_CLI_SOURCE_KIND: &str = "copilot-cli";

/// `sync_state.filename` key prefix for the Copilot CLI agent reconciliation
/// cursor. The full cursor filename is
/// `sync:copilot_cli_agents:<hex(canonical_copilot_dir)>::<created_at>::<id>`.
/// This namespace is intentionally separate from
/// [`COPILOT_APP_CURSOR_PREFIX`] so CLI reconciliation and the Copilot App
/// collector advance independently and never overwrite each other's
/// high-water mark. Scoped by the canonical Copilot directory so switching
/// `COPILOT_DIR` starts a fresh cursor.
const COPILOT_CLI_AGENT_CURSOR_PREFIX: &str = "sync:copilot_cli_agents:";

/// `sync_state.filename` prefix for CLI sessions whose hook rows are ahead of
/// `assistant_usage_events`. These sessions are retried even when no new agent
/// event arrives, without forcing already-valid sessions to roll back.
const COPILOT_CLI_AGENT_PENDING_PREFIX: &str = "sync:copilot_cli_agents:pending:";

/// Versioned migration key recording that the first CLI agent reconciliation
/// backfill has completed. The migration scans every CLI-classified session
/// (transcript at `session-state/<id>/events.jsonl`) and replaces its merged
/// `copilot-cli` hook rows with per-agent split rows. Idempotent and safe to
/// retry; only affects `copilot-cli` rows, never `copilot-app` or others.
const COPILOT_CLI_AGENT_MIGRATION_KEY: &str = "migration:copilot_cli_agent_split_v2";

/// One-time backfill of `cwd` for existing Copilot rows (`copilot-cli` and
/// `copilot-app`) that were written before CWD was populated from
/// `session-store.db.sessions`. Idempotent: the marker is set on success so
/// re-runs only cover sessions synced after the marker.
const COPILOT_CWD_BACKFILL_MIGRATION_KEY: &str = "migration:copilot_cwd_backfill_v1";

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

fn unix_timestamp_secs() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn new_import_batch_id() -> String {
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let counter = IMPORT_BATCH_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("import-{timestamp:x}-{counter:x}")
}

fn normalize_import_metadata_value(raw: Option<String>, max_chars: usize) -> Option<String> {
    let value = raw?.trim().chars().take(max_chars).collect::<String>();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
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
        if t.cache_write_5m.is_none() && t.cache_write_1h.is_none() {
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
            format!(
                "{}|{}|{}|{}|{}|{}|{}|{}",
                t.input,
                t.output,
                t.cache_read.unwrap_or(0),
                t.cache_write.unwrap_or(0),
                t.cache_write_5m.unwrap_or(0),
                t.cache_write_1h.unwrap_or(0),
                t.reasoning.unwrap_or(0),
                t.total
            )
        }
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

pub fn get_cursor_state_db_path() -> PathBuf {
    if let Some(path) = crate::paths::env_path("CURSOR_STATE_DB") {
        return path;
    }
    dirs::config_dir()
        .map(|dir| {
            dir.join("Cursor")
                .join("User")
                .join("globalStorage")
                .join("state.vscdb")
        })
        .unwrap_or_else(|| get_cursor_dir().join("state.vscdb"))
}

pub fn get_grok_dir() -> PathBuf {
    if let Some(path) = crate::paths::env_path("GROK_DIR") {
        return path;
    }
    dirs::home_dir()
        .map(|home| home.join(".grok"))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn get_pi_dir() -> PathBuf {
    if let Some(path) = crate::paths::env_path("PI_DIR") {
        return path;
    }
    dirs::home_dir()
        .map(|home| home.join(".pi"))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn get_omp_dir() -> PathBuf {
    if let Some(path) = crate::paths::env_path("OMP_DIR") {
        return path;
    }
    dirs::home_dir()
        .map(|home| home.join(".omp"))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn get_muse_dir() -> PathBuf {
    if let Some(path) = crate::paths::env_path("MUSE_DIR") {
        return path;
    }
    // Muse stores sessions under ~/.local/share/muse on both Linux and macOS,
    // while `dirs::data_local_dir()` on macOS points to ~/Library/Application Support.
    // Prefer the XDG location if it exists to avoid missing data on macOS.
    if let Some(home) = dirs::home_dir() {
        let xdg_path = home.join(".local").join("share").join("muse");
        if xdg_path.join("sessions").exists() || xdg_path.exists() {
            return xdg_path;
        }
    }
    if let Some(data_dir) = dirs::data_local_dir() {
        let candidate = data_dir.join("muse");
        if candidate.join("sessions").exists() || candidate.exists() {
            return candidate;
        }
        // Return XDG fallback for consistency even if not yet created
        if let Some(home) = dirs::home_dir() {
            return home.join(".local").join("share").join("muse");
        }
        return candidate;
    }
    dirs::home_dir()
        .map(|home| home.join(".local").join("share").join("muse"))
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
            assistant_type TEXT NOT NULL, -- 'antigravity', 'copilot', 'codex', 'claude', 'cursor', 'grok', 'pi', 'omp'
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
            model_signature TEXT,
            
            -- Token Statistics
            tokens_input INTEGER,
            tokens_output INTEGER,
            tokens_cache_read INTEGER,
            tokens_cache_write INTEGER,
            tokens_cache_write_5m INTEGER,
            tokens_cache_write_1h INTEGER,
            tokens_reasoning INTEGER,
            tokens_total INTEGER,
            
            -- Delta Token Statistics
            delta_input INTEGER,
            delta_output INTEGER,
            delta_cache_read INTEGER,
            delta_cache_write INTEGER,
            delta_cache_write_5m INTEGER,
            delta_cache_write_1h INTEGER,
            delta_reasoning INTEGER,
            delta_total INTEGER,
            
            -- Duration and Request Count
            duration_ms INTEGER,
            premium_requests INTEGER,
            reported_cost_usd REAL,
            source_kind TEXT NOT NULL DEFAULT 'legacy',
            usage_identity TEXT NOT NULL DEFAULT '',

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
        "ALTER TABLE usage_entries ADD COLUMN tokens_cache_write_5m INTEGER",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE usage_entries ADD COLUMN tokens_cache_write_1h INTEGER",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE usage_entries ADD COLUMN delta_cache_write_5m INTEGER",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE usage_entries ADD COLUMN delta_cache_write_1h INTEGER",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE usage_entries ADD COLUMN import_source_id TEXT",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE usage_entries ADD COLUMN import_batch_id TEXT",
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
    // Most collectors write one row per (assistant, source, session, turn)
    // and keep the default empty identity. Collectors that legitimately emit
    // multiple rows for the same turn, such as Copilot per-model attribution,
    // provide a stable non-empty identity so each row remains independently
    // upsertable without weakening uniqueness for existing collectors.
    let _ = conn.execute(
        "ALTER TABLE usage_entries ADD COLUMN usage_identity TEXT NOT NULL DEFAULT ''",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE usage_entries ADD COLUMN model_signature TEXT",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE usage_entries ADD COLUMN reported_cost_usd REAL",
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
    // the original uniqueness because existing collectors use the default
    // empty usage_identity, while explicitly keyed collectors may retain
    // multiple independently upsertable rows for one logical turn.
    conn.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS uidx_assistant_source_session_turn
         ON usage_entries(assistant_type, source_kind, session_id, turn_no, usage_identity)
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
         ON usage_entries(
            assistant_type, source_kind, source_dir_key, session_id, turn_no, usage_identity
         )
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

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_assistant_transcript_path
         ON usage_entries(assistant_type, transcript_path)",
        [],
    )
    .map_err(|e| {
        format!(
            "建立 transcript 路徑索引 idx_assistant_transcript_path 失敗: {}",
            e
        )
    })?;

    let _ = conn.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS uidx_assistant_import_source_id ON usage_entries(assistant_type, import_source_id) WHERE import_source_id IS NOT NULL",
        [],
    );
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_usage_import_batch
         ON usage_entries(assistant_type, import_batch_id)
         WHERE import_batch_id IS NOT NULL",
        [],
    )
    .map_err(|e| format!("建立匯入批次索引 idx_usage_import_batch 失敗: {e}"))?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS import_batches (
            id TEXT PRIMARY KEY,
            assistant_type TEXT NOT NULL,
            source_assistant TEXT,
            source_file_name TEXT,
            import_date TEXT NOT NULL,
            total_records INTEGER NOT NULL,
            imported_records INTEGER NOT NULL,
            skipped_duplicates INTEGER NOT NULL,
            created_at INTEGER NOT NULL,
            rolled_back_at INTEGER,
            removed_records INTEGER NOT NULL DEFAULT 0
        )",
        [],
    )
    .map_err(|e| format!("建立 import_batches 表失敗: {e}"))?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_import_batches_assistant_created
         ON import_batches(assistant_type, created_at DESC)",
        [],
    )
    .map_err(|e| format!("建立匯入批次查詢索引失敗: {e}"))?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_cursor_model_signature
         ON usage_entries(model_signature)
         WHERE assistant_type = 'cursor' AND model_signature IS NOT NULL",
        [],
    )
    .map_err(|error| format!("建立 Cursor 模型簽章索引失敗: {error}"))?;

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
    conn.execute(
        "CREATE TABLE IF NOT EXISTS cursor_model_signatures (
            source_id TEXT NOT NULL,
            signature TEXT NOT NULL,
            model TEXT NOT NULL,
            is_ambiguous INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (source_id, signature)
        )",
        [],
    )
    .map_err(|error| format!("建立 Cursor 模型簽章表失敗: {error}"))?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS cursor_session_metadata (
            source_id TEXT NOT NULL,
            session_id TEXT NOT NULL,
            cwd TEXT,
            mode TEXT,
            model TEXT,
            PRIMARY KEY (source_id, session_id)
        )",
        [],
    )
    .map_err(|error| format!("建立 Cursor Session 中繼資料表失敗: {error}"))?;
    let _ = conn.execute(
        "ALTER TABLE cursor_session_metadata ADD COLUMN model TEXT",
        [],
    );

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

    let claude_cache_write_migration_done: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_state WHERE filename = ?)",
            params![CLAUDE_CACHE_WRITE_PRICING_MIGRATION_KEY],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if !claude_cache_write_migration_done {
        conn.execute(
            "UPDATE usage_entries
             SET tokens_input = CASE
                    WHEN tokens_input IS NULL THEN NULL
                    WHEN tokens_input >= COALESCE(tokens_cache_write, 0)
                    THEN tokens_input - COALESCE(tokens_cache_write, 0)
                    ELSE 0
                 END,
                 tokens_cache_write_5m = COALESCE(tokens_cache_write, 0),
                 tokens_cache_write_1h = 0,
                 delta_input = CASE
                    WHEN delta_input IS NULL THEN NULL
                    WHEN delta_input >= COALESCE(delta_cache_write, 0)
                    THEN delta_input - COALESCE(delta_cache_write, 0)
                    ELSE 0
                 END,
                 delta_cache_write_5m = COALESCE(delta_cache_write, 0),
                 delta_cache_write_1h = 0
             WHERE (
                    assistant_type = 'claude'
                    OR (
                        assistant_type = 'codex'
                        AND transcript_path IS NOT NULL
                        AND (
                               transcript_path LIKE '%.claude/%'
                            OR transcript_path LIKE '%/claude/%'
                            OR transcript_path LIKE '%.claude\\%'
                            OR transcript_path LIKE '%\\claude\\%'
                        )
                    )
               )
               AND tokens_cache_write_5m IS NULL
               AND tokens_cache_write_1h IS NULL",
            [],
        )
        .map_err(|error| format!("遷移 Claude 快取寫入費用欄位失敗: {error}"))?;
        conn.execute(
            "DELETE FROM sync_state
             WHERE filename LIKE 'claude:%'
                OR filename LIKE 'codex:claude:%'",
            [],
        )
        .map_err(|error| format!("重設 Claude 同步狀態失敗: {error}"))?;
        conn.execute(
            "INSERT OR REPLACE INTO sync_state (filename, last_synced_size, last_synced_time)
             VALUES (?, 1, 0)",
            params![CLAUDE_CACHE_WRITE_PRICING_MIGRATION_KEY],
        )
        .map_err(|error| format!("記錄 Claude 快取寫入費用遷移失敗: {error}"))?;
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
                OR filename LIKE 'codex:archived_sessions/%'
                OR filename LIKE 'codex:archived_sessions\\%'
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

    let grok_parser_migration_done: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_state WHERE filename = ?)",
            params![GROK_PARSER_MIGRATION_KEY],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if !grok_parser_migration_done {
        // Reparse Grok sessions so model aliases, cached reads, context deltas,
        // and provider-reported costs use the current parser semantics.
        conn.execute("DELETE FROM sync_state WHERE filename LIKE 'grok:%'", [])
            .map_err(|error| format!("清除 Grok Build 同步狀態失敗: {error}"))?;
        for legacy_key in LEGACY_GROK_PARSER_MIGRATION_KEYS {
            conn.execute(
                "DELETE FROM sync_state WHERE filename = ?",
                params![legacy_key],
            )
            .map_err(|error| {
                format!("清理舊 Grok Build migration 狀態失敗 ({legacy_key}): {error}")
            })?;
        }
        conn.execute(
            "INSERT OR REPLACE INTO sync_state (filename, last_synced_size, last_synced_time)
             VALUES (?, 1, 0)",
            params![GROK_PARSER_MIGRATION_KEY],
        )
        .map_err(|error| format!("記錄 Grok Build parser migration 失敗: {error}"))?;
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

fn normalize_legacy_claude_token_stats(tokens: &mut Option<TokenStats>) {
    let Some(tokens) = tokens else {
        return;
    };
    if tokens.cache_write_5m.is_some() || tokens.cache_write_1h.is_some() {
        return;
    }

    let cache_write = tokens.cache_write.unwrap_or(0);
    tokens.input = tokens.input.saturating_sub(cache_write);
    tokens.cache_write_5m = Some(cache_write);
    tokens.cache_write_1h = Some(0);
}

fn normalize_legacy_claude_usage_entry(entry: &mut UsageEntry) {
    normalize_legacy_claude_token_stats(&mut entry.tokens);
    normalize_legacy_claude_token_stats(&mut entry.delta_tokens);
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
        COPILOT_CLI_SOURCE_KIND
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
                            tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, tokens_cache_write_5m, tokens_cache_write_1h, tokens_reasoning, tokens_total,
                            delta_input, delta_output, delta_cache_read, delta_cache_write, delta_cache_write_5m, delta_cache_write_1h, delta_reasoning, delta_total,
                            duration_ms, premium_requests
                        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
                            tokens.and_then(|t| t.cache_write_5m.map(|v| v as i64)),
                            tokens.and_then(|t| t.cache_write_1h.map(|v| v as i64)),
                            tokens.and_then(|t| t.reasoning.map(|v| v as i64)),
                            tokens.map(|t| t.total as i64),
                            delta.map(|t| t.input as i64),
                            delta.map(|t| t.output as i64),
                            delta.and_then(|t| t.cache_read.map(|v| v as i64)),
                            delta.and_then(|t| t.cache_write.map(|v| v as i64)),
                            delta.and_then(|t| t.cache_write_5m.map(|v| v as i64)),
                            delta.and_then(|t| t.cache_write_1h.map(|v| v as i64)),
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
            tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, tokens_cache_write_5m, tokens_cache_write_1h, tokens_reasoning, tokens_total,
            delta_input, delta_output, delta_cache_read, delta_cache_write, delta_cache_write_5m, delta_cache_write_1h, delta_reasoning, delta_total,
            duration_ms, premium_requests, parent_session_id, agent_nickname, agent_role, reasoning_effort
        ) VALUES (
            ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
            ?, ?, ?, ?, ?, ?, ?, ?,
            ?, ?, ?, ?, ?, ?, ?, ?,
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
            tokens.and_then(|value| value.cache_write_5m.map(|v| v as i64)),
            tokens.and_then(|value| value.cache_write_1h.map(|v| v as i64)),
            tokens.and_then(|value| value.reasoning.map(|v| v as i64)),
            tokens.map(|value| value.total as i64),
            delta.map(|value| value.input as i64),
            delta.map(|value| value.output as i64),
            delta.and_then(|value| value.cache_read.map(|v| v as i64)),
            delta.and_then(|value| value.cache_write.map(|v| v as i64)),
            delta.and_then(|value| value.cache_write_5m.map(|v| v as i64)),
            delta.and_then(|value| value.cache_write_1h.map(|v| v as i64)),
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

fn codex_source_kind_from_metadata(payload: &serde_json::Value) -> &'static str {
    let originator = payload
        .get("originator")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    if originator.contains("desktop") {
        return CODEX_DESKTOP_SOURCE_KIND;
    }
    if matches!(
        originator.as_str(),
        "codex-tui" | "codex_cli_rs" | "codex_exec"
    ) {
        return CODEX_CLI_SOURCE_KIND;
    }

    match payload.get("source").and_then(|value| value.as_str()) {
        Some("cli" | "exec") => CODEX_CLI_SOURCE_KIND,
        _ => CODEX_OTHER_SOURCE_KIND,
    }
}

fn codex_usage_to_stats(usage: CodexTokenUsage) -> TokenStats {
    let cache_read = usage.cached_input_tokens;
    let cache_write = usage.cache_write_input_tokens;
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
        cache_write: Some(cache_write),
        cache_write_5m: None,
        cache_write_1h: None,
        reasoning: Some(usage.reasoning_output_tokens),
        total,
    }
}

fn codex_usage_delta_to_stats(
    previous: Option<&CodexTokenUsage>,
    current: &CodexTokenUsage,
) -> TokenStats {
    let (
        input_tokens,
        cached_input_tokens,
        cache_write_input_tokens,
        output_tokens,
        reasoning_output_tokens,
    ) = match previous {
        Some(previous)
            if current.input_tokens >= previous.input_tokens
                && current.cached_input_tokens >= previous.cached_input_tokens
                && current.cache_write_input_tokens >= previous.cache_write_input_tokens
                && current.output_tokens >= previous.output_tokens
                && current.reasoning_output_tokens >= previous.reasoning_output_tokens =>
        {
            (
                current.input_tokens - previous.input_tokens,
                current.cached_input_tokens - previous.cached_input_tokens,
                current.cache_write_input_tokens - previous.cache_write_input_tokens,
                current.output_tokens - previous.output_tokens,
                current.reasoning_output_tokens - previous.reasoning_output_tokens,
            )
        }
        _ => (
            current.input_tokens,
            current.cached_input_tokens,
            current.cache_write_input_tokens,
            current.output_tokens,
            current.reasoning_output_tokens,
        ),
    };

    let cache_read = cached_input_tokens;
    let cache_write = cache_write_input_tokens;
    let input = input_tokens.saturating_sub(cache_read);
    let output = output_tokens;
    let total = input_tokens.saturating_add(output);

    TokenStats {
        input,
        output,
        cache_read: Some(cache_read),
        cache_write: Some(cache_write),
        cache_write_5m: None,
        cache_write_1h: None,
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
    let mut source_kind = CODEX_OTHER_SOURCE_KIND.to_string();
    let mut session_identity_locked = false;

    for event in &events {
        let event_type = event.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let payload = match event.get("payload") {
            Some(payload) => payload,
            None => continue,
        };
        let payload_type = payload.get("type").and_then(|t| t.as_str()).unwrap_or("");

        if event_type == "session_meta" {
            let detected_source_kind = codex_source_kind_from_metadata(payload);
            if source_kind == CODEX_OTHER_SOURCE_KIND
                || detected_source_kind == CODEX_DESKTOP_SOURCE_KIND
            {
                source_kind = detected_source_kind.to_string();
            }
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
    let completed_task_duration_ms = events
        .iter()
        .filter_map(|event| {
            if event.get("type").and_then(|value| value.as_str()) != Some("event_msg") {
                return None;
            }
            let payload = event.get("payload")?;
            if payload.get("type").and_then(|value| value.as_str()) != Some("task_complete") {
                return None;
            }
            payload.get("duration_ms").and_then(|value| value.as_u64())
        })
        .fold(None::<u64>, |total, duration_ms| {
            Some(total.unwrap_or_default().saturating_add(duration_ms))
        });

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
            cost: completed_task_duration_ms.map(|duration_ms| CostStats {
                total_api_duration_ms: Some(duration_ms as f64),
                total_duration_ms: None,
                total_premium_requests: None,
                reported_cost_usd: None,
            }),
            source_kind: Some(source_kind.clone()),
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
                OR filename LIKE 'codex:sessions\\%'
                OR filename LIKE 'codex:archived_sessions/%'
                OR filename LIKE 'codex:archived_sessions\\%'",
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

fn run_codex_source_kind_migration(conn: &mut Connection) -> Result<(), String> {
    let migration_done: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_state WHERE filename = ?)",
            params![CODEX_SOURCE_KIND_MIGRATION_KEY],
            |row| row.get(0),
        )
        .unwrap_or(false);

    if migration_done {
        return Ok(());
    }

    let tx = conn
        .transaction()
        .map_err(|error| format!("Codex 來源分類遷移 BEGIN 失敗: {error}"))?;
    tx.execute(
        "DELETE FROM sync_state
         WHERE filename LIKE 'codex:sessions/%'
            OR filename LIKE 'codex:sessions\\%'
            OR filename LIKE 'codex:archived_sessions/%'
            OR filename LIKE 'codex:archived_sessions\\%'",
        [],
    )
    .map_err(|error| format!("清除 Codex 來源分類同步狀態失敗: {error}"))?;
    tx.execute(
        "INSERT OR REPLACE INTO sync_state (filename, last_synced_size, last_synced_time)
         VALUES (?, 1, 0)",
        params![CODEX_SOURCE_KIND_MIGRATION_KEY],
    )
    .map_err(|error| format!("記錄 Codex 來源分類遷移失敗: {error}"))?;
    tx.commit()
        .map_err(|error| format!("Codex 來源分類遷移 COMMIT 失敗: {error}"))
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

    // Canonicalize the source directory so the cursor is stable across trailing
    // slashes / symlinks and isolated per COPILOT_APP_DIR / COPILOT_DIR value.
    // Hex-encode the canonical path's raw OS-encoded bytes so the cursor key is
    // injective (no two distinct paths map to the same key) and free of LIKE
    // wildcard characters (`%`, `_`). Encoding raw bytes (not lossy UTF-8) avoids
    // collisions from Unicode replacement chars and from `\\` vs `/` normalization.
    let canonical_app_dir = app_dir.canonicalize().unwrap_or_else(|_| app_dir.clone());
    let source_key = encode_hex(canonical_app_dir.as_os_str().as_encoded_bytes());
    let cursor_key_prefix = format!("{}{}::", COPILOT_APP_CURSOR_PREFIX, source_key);

    // `data.db.sessions` is the authoritative registry for Copilot App
    // sessions. The session-store is shared with Copilot CLI, so without a
    // validated registry it is unsafe to classify any usage event as App.
    let data_db_path = app_dir.join("data.db");
    if !data_db_path.exists() {
        // data.db is created by the Copilot App, not the CLI, so its absence
        // is a normal state for CLI-only users. The sync loop runs every few
        // seconds — log once per process instead of flooding the console.
        static MISSING_DATA_DB_NOTICE: std::sync::Once = std::sync::Once::new();
        MISSING_DATA_DB_NOTICE.call_once(|| {
            eprintln!(
                "ℹ️ Copilot App 同步跳過：找不到 data.db ({})，未安裝 Copilot App 時屬正常狀態",
                data_db_path.display()
            );
        });
        return Ok(());
    }
    let data_db = match Connection::open_with_flags(
        &data_db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "⚠️ Copilot App 同步跳過：無法開啟 data.db ({}): {}",
                data_db_path.display(),
                e
            );
            return Ok(());
        }
    };
    let _ = data_db.busy_timeout(std::time::Duration::from_secs(2));

    let sessions_table_exists: bool = data_db
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='sessions'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);
    if !sessions_table_exists {
        eprintln!(
            "⚠️ Copilot App 同步跳過：data.db 缺少 sessions table ({})",
            data_db_path.display()
        );
        return Ok(());
    }

    let app_session_ids_result: rusqlite::Result<HashSet<String>> = (|| {
        let mut stmt = data_db.prepare("SELECT id FROM sessions")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect()
    })();
    let app_session_ids: HashSet<String> = match app_session_ids_result {
        Ok(ids) => ids,
        Err(e) => {
            eprintln!(
                "⚠️ Copilot App 同步跳過：讀取 data.db.sessions 失敗 ({}): {}",
                data_db_path.display(),
                e
            );
            return Ok(());
        }
    };

    let existing_app_session_ids = query_copilot_app_session_ids(conn, &source_key)?;
    let reconciliation_session_ids: Vec<String> = app_session_ids
        .difference(&existing_app_session_ids)
        .cloned()
        .collect();
    let stale_app_session_ids: Vec<String> = existing_app_session_ids
        .difference(&app_session_ids)
        .cloned()
        .collect();

    if !session_store_path.exists() {
        cleanup_stale_copilot_app_rows(conn, &source_key, &stale_app_session_ids)?;
        return Ok(());
    }

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
            cleanup_stale_copilot_app_rows(conn, &source_key, &stale_app_session_ids)?;
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
        cleanup_stale_copilot_app_rows(conn, &source_key, &stale_app_session_ids)?;
        return Ok(());
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
                if matches!(
                    classify_copilot_app_session(&app_dir, &app_session_ids, &session_id),
                    CopilotAppSessionKind::App
                ) && touched_set.insert((session_id.clone(), turn_index))
                {
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
    // Add all turns for registry sessions which do not yet have an App row.
    // This is the history-based reconciliation path for events whose cursor
    // was already advanced before data.db.sessions contained the session.
    if !reconciliation_session_ids.is_empty() {
        let mut reconciliation_stmt = session_store
            .prepare(
                "SELECT DISTINCT session_id, turn_index
                 FROM assistant_usage_events
                 WHERE session_id = ?
                 ORDER BY turn_index ASC",
            )
            .map_err(|e| format!("準備 Copilot App reconciliation 查詢失敗: {}", e))?;
        for session_id in &reconciliation_session_ids {
            let rows = reconciliation_stmt
                .query_map(params![session_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })
                .map_err(|e| format!("執行 Copilot App reconciliation 查詢失敗: {}", e))?;
            for row in rows {
                let turn =
                    row.map_err(|e| format!("讀取 Copilot App reconciliation turn 失敗: {}", e))?;
                if touched_set.insert(turn.clone()) {
                    touched_turns.push(turn);
                }
            }
        }
    }

    if touched_turns.is_empty() && stale_app_session_ids.is_empty() {
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
        if max_event_cursor.is_none() {
            return Ok(());
        }
    }

    // Re-aggregate each touched turn from the FULL event history for that
    // (session_id, turn_index), regardless of cursor. This guarantees that
    // turns which straddle the cursor boundary are written with their complete
    // token totals rather than only the post-cursor subset.
    //
    // Subagents (non-null `agent_id`) share the main session's `session_id` and
    // `turn_index` but use a different model, so the aggregation key is
    // (session_id, turn_index, agent_id, model). `agent_id IS NULL` identifies
    // the main agent; a non-null `agent_id` identifies a subagent. Grouping by
    // both keeps the main agent and each subagent as separate usage rows so
    // their token totals are not merged and the subagent model is preserved.
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
                model, MIN(reasoning_effort), agent_id, MIN(initiator)
         FROM assistant_usage_events
         WHERE session_id = ? AND turn_index = ?
         GROUP BY session_id, turn_index, agent_id, model";

    let mut agg_stmt = session_store
        .prepare(aggregate_query)
        .map_err(|e| format!("準備 Copilot App 聚合查詢失敗: {}", e))?;

    let mut turn_rows: Vec<CopilotAppTurnRow> = Vec::new();
    for (session_id, turn_index) in &touched_turns {
        let rows_res = agg_stmt
            .query_map(params![session_id, turn_index], |row| {
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
                    agent_id: row.get::<_, Option<String>>(9)?,
                    initiator: row.get::<_, Option<String>>(10)?,
                })
            })
            .map_err(|e| format!("執行 Copilot App 聚合查詢失敗: {}", e));
        match rows_res {
            Ok(rows) => {
                for row in rows {
                    match row {
                        Ok(r) => turn_rows.push(r),
                        Err(e) => {
                            eprintln!(
                                "⚠️ 讀取 Copilot App 聚合結果 (session {} turn {}) 失敗: {}",
                                session_id, turn_index, e
                            );
                            return Ok(());
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!(
                    "⚠️ 聚合 Copilot App turn (session {} turn {}) 失敗: {}",
                    session_id, turn_index, e
                );
                return Ok(());
            }
        }
    }

    if turn_rows.is_empty() && stale_app_session_ids.is_empty() && max_event_cursor.is_none() {
        return Ok(());
    }

    let tx = conn
        .transaction()
        .map_err(|e| format!("開啟 Copilot App transaction 失敗: {}", e))?;

    // Cache session title/workspace lookups.
    let mut session_meta_cache: HashMap<String, CopilotAppSessionMeta> = HashMap::new();

    let mut upserted = 0usize;
    let deleted = delete_stale_copilot_app_rows(&tx, &source_key, &stale_app_session_ids)?;

    // Reconcile legacy merged Copilot App rows: the old collector aggregated
    // by (session_id, turn_index) and used `MIN(model)`, producing a single row
    // per turn with a 3-segment `import_source_id`
    // (`copilot-app:<source_key>:<session_id>:<turn_index>`) that merged
    // main-agent and subagent events. The new collector writes one 4-segment
    // row per (session_id, turn_index, agent_id, model) group. Delete the
    // legacy 3-segment rows for sessions being synced so the new split rows
    // replace them instead of coexisting and causing double-counting. Only
    // rows with this source_kind and a non-null source_dir_key are affected;
    // CLI/VS Code rows never match.
    let touched_session_ids: HashSet<&String> = turn_rows.iter().map(|r| &r.session_id).collect();
    let mut legacy_cleaned = 0usize;
    for session_id in &touched_session_ids {
        legacy_cleaned += tx
            .execute(
                "DELETE FROM usage_entries
                 WHERE source_kind = ?
                   AND source_dir_key = ?
                   AND session_id = ?
                   AND import_source_id LIKE 'copilot-app:%:%:%'
                   AND import_source_id NOT LIKE 'copilot-app:%:%:%:%'",
                params![COPILOT_APP_SOURCE_KIND, source_key, session_id],
            )
            .map_err(|e| format!("清除舊版合併 Copilot App 資料失敗: {}", e))?;
    }
    if legacy_cleaned > 0 {
        println!(
            "✅ Copilot App reconciliation：清除 {} 筆舊版合併資料（主從 Agent 未分列）",
            legacy_cleaned
        );
    }

    for row in turn_rows {
        // Resolve session metadata (title + cwd) from the original session.
        let meta = session_meta_cache
            .entry(row.session_id.clone())
            .or_insert_with(|| {
                resolve_copilot_app_session_meta(&data_db, &session_store, &row.session_id)
            })
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

        // Subagents share the main session's `session_id` and `turn_index`
        // but use a different model and a non-null `agent_id`. To keep them as
        // distinct usage rows (the daily handler and UI tree key sessions by
        // `session_id`), subagents get a synthetic session id
        // `<session_id>__<agent_id>` and `parent_session_id = <session_id>` so
        // the existing UI tree renders them under the main session. The `__`
        // separator keeps the synthetic id within the `is_safe_session_id`
        // charset. The main agent keeps its original session id.
        let (row_session_id, parent_session_id, agent_nickname, agent_id_segment, agent_role) =
            match &row.agent_id {
                Some(agent) if !agent.is_empty() => {
                    let synthetic = format!("{}__{}", row.session_id, agent);
                    let segment = agent.clone();
                    // Only surface agent_role when the source explicitly marked
                    // the agent as a sub-agent; never guess. Any other
                    // initiator value (including NULL) leaves agent_role NULL.
                    let role = if row.initiator.as_deref() == Some("sub-agent") {
                        Some("sub-agent".to_string())
                    } else {
                        None
                    };
                    (
                        synthetic,
                        Some(row.session_id.clone()),
                        Some(agent.clone()),
                        segment,
                        role,
                    )
                }
                _ => (row.session_id.clone(), None, None, "main".to_string(), None),
            };

        // Build a session name for subagents that surfaces their agent id.
        let session_name = match (&row.agent_id, &meta.title) {
            (Some(agent), Some(title)) if !agent.is_empty() => {
                Some(format!("{} (subagent {})", title, agent))
            }
            (Some(agent), None) if !agent.is_empty() => Some(format!("Subagent {}", agent)),
            _ => meta.title.clone(),
        };

        // Include the source directory key and an agent segment in
        // import_source_id so turns from different COPILOT_APP_DIR and from
        // different agents (main vs subagent) with the same (session_id,
        // turn_index) do not upsert-overwrite each other.
        let model_identity = encode_hex(row.model.as_deref().unwrap_or("").as_bytes());
        let usage_identity = format!(
            "agent={};model={}",
            encode_hex(agent_id_segment.as_bytes()),
            model_identity
        );
        let import_source_id = format!(
            "copilot-app:{}:{}:{}:{}:{}",
            source_key, row.session_id, row.turn_index, agent_id_segment, model_identity
        );

        // Rows written by the original collector predate usage_identity and
        // may have merged several models into the default identity. Remove
        // only the matching logical agent/turn row before inserting the
        // independently keyed model rows; untouched turns remain intact.
        tx.execute(
            "DELETE FROM usage_entries
             WHERE assistant_type = 'copilot'
               AND source_kind = ?
               AND source_dir_key = ?
               AND session_id = ?
               AND turn_no = ?
               AND usage_identity = ''",
            params![
                COPILOT_APP_SOURCE_KIND,
                source_key,
                row_session_id,
                turn_no as i64
            ],
        )
        .map_err(|e| format!("清除舊版 Copilot App 單列模型資料失敗: {}", e))?;

        // Use INSERT OR REPLACE so turns that received additional API calls
        // after the first sync are updated with the complete re-aggregated
        // totals instead of being silently dropped by INSERT OR IGNORE.
        // source_dir_key isolates rows by source directory in the unique index.
        let insert_res = tx.execute(
            "INSERT OR REPLACE INTO usage_entries (
                assistant_type, source_kind, source_dir_key, usage_identity, timestamp, date, session_id, session_name, transcript_path, cwd, version, turn_no, model, model_id,
                tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, tokens_reasoning, tokens_total,
                delta_input, delta_output, delta_cache_read, delta_cache_write, delta_reasoning, delta_total,
                duration_ms, premium_requests, import_source_id, reasoning_effort,
                parent_session_id, agent_nickname, agent_role
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, NULL, ?, NULL, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, ?, ?, ?, ?, ?)",
            params![
                "copilot",
                COPILOT_APP_SOURCE_KIND,
                source_key,
                usage_identity,
                timestamp,
                date_str,
                row_session_id,
                session_name,
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
                parent_session_id,
                agent_nickname,
                // `agent_role` reuses `initiator` semantics: only
                // `initiator = 'sub-agent'` produces `agent_role = 'sub-agent'`
                // for subagent rows; any other initiator (or the main agent)
                // leaves agent_role NULL so the frontend Subagent badge is the
                // sole role marker and never duplicates.
                agent_role,
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
    if deleted > 0 {
        println!("✅ Copilot App reconciliation：清除 {} 筆錯誤資料", deleted);
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum CopilotAppSessionKind {
    App,
    Cli,
    Unknown,
}

fn classify_copilot_app_session(
    app_dir: &Path,
    app_session_ids: &HashSet<String>,
    session_id: &str,
) -> CopilotAppSessionKind {
    if app_session_ids.contains(session_id) {
        return CopilotAppSessionKind::App;
    }

    let cli_transcript = app_dir
        .join("session-state")
        .join(session_id)
        .join("events.jsonl");
    if cli_transcript.is_file() {
        CopilotAppSessionKind::Cli
    } else {
        CopilotAppSessionKind::Unknown
    }
}

fn query_copilot_app_session_ids(
    conn: &Connection,
    source_key: &str,
) -> Result<HashSet<String>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT session_id
             FROM usage_entries
             WHERE assistant_type = 'copilot'
               AND source_kind = ?
               AND source_dir_key = ?",
        )
        .map_err(|e| format!("查詢 Copilot App session 失敗: {}", e))?;
    let rows = stmt
        .query_map(params![COPILOT_APP_SOURCE_KIND, source_key], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|e| format!("讀取 Copilot App session 失敗: {}", e))?;
    let mut ids = HashSet::new();
    for row in rows {
        let raw = row.map_err(|e| format!("讀取 Copilot App session 失敗: {}", e))?;
        // Subagent rows use a synthetic `<session_id>__<agent_id>` id. Strip
        // the agent suffix so reconciliation compares the original session id
        // against `data.db.sessions`, otherwise subagent rows would always be
        // classified as stale and deleted on every sync.
        let base = raw.split("__").next().unwrap_or(&raw).to_string();
        ids.insert(base);
    }
    Ok(ids)
}

fn delete_stale_copilot_app_rows(
    tx: &rusqlite::Transaction<'_>,
    source_key: &str,
    session_ids: &[String],
) -> Result<usize, String> {
    let mut deleted = 0usize;
    for session_id in session_ids {
        // Delete the main session row plus any subagent synthetic rows
        // (`<session_id>__<agent_id>`). Use a prefix match (`session_id` + '__')
        // so stale subagent rows are removed together with their parent.
        deleted += tx
            .execute(
                "DELETE FROM usage_entries
                 WHERE assistant_type = 'copilot'
                   AND source_kind = ?
                   AND source_dir_key = ?
                   AND (session_id = ? OR session_id LIKE ? ESCAPE '\\')",
                params![
                    COPILOT_APP_SOURCE_KIND,
                    source_key,
                    session_id,
                    format!("{}\\__%", session_id)
                ],
            )
            .map_err(|e| format!("刪除錯誤 Copilot App 資料失敗: {}", e))?;
    }
    Ok(deleted)
}

fn cleanup_stale_copilot_app_rows(
    conn: &mut Connection,
    source_key: &str,
    session_ids: &[String],
) -> Result<(), String> {
    if session_ids.is_empty() {
        return Ok(());
    }
    let tx = conn
        .transaction()
        .map_err(|e| format!("開啟 Copilot App cleanup transaction 失敗: {}", e))?;
    let deleted = delete_stale_copilot_app_rows(&tx, source_key, session_ids)?;
    tx.commit()
        .map_err(|e| format!("Copilot App cleanup COMMIT 失敗: {}", e))?;
    if deleted > 0 {
        println!("✅ Copilot App reconciliation：清除 {} 筆錯誤資料", deleted);
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
    /// `agent_id` from `assistant_usage_events`. `None` (NULL) identifies the
    /// main agent; a non-null value identifies a subagent (e.g. `call_v4b32z66`).
    agent_id: Option<String>,
    /// `initiator` of the first event for this agent group, used only to label
    /// subagent `agent_role` when it is `'sub-agent'`. Never guessed; any other
    /// value (including NULL) leaves `agent_role` NULL.
    initiator: Option<String>,
}

#[derive(Clone, Default)]
struct CopilotAppSessionMeta {
    title: Option<String>,
    cwd: Option<String>,
}

/// Resolve the CWD for a Copilot session from `session-store.db.sessions.cwd`.
///
/// The `sessions` table in the shared session-store records the working
/// directory for every session (both CLI and App). Filter out empty strings
/// and bare `/` (the root directory, which is usually a meaningless default).
fn resolve_session_store_cwd(session_store: &Connection, session_id: &str) -> Option<String> {
    let cwd: Option<String> = session_store
        .query_row(
            "SELECT cwd FROM sessions WHERE id = ?",
            params![session_id],
            |row| row.get(0),
        )
        .ok()
        .flatten();
    cwd.filter(|c| !c.is_empty() && c != "/")
}

fn resolve_copilot_app_session_meta(
    data_db: &Connection,
    session_store: &Connection,
    session_id: &str,
) -> CopilotAppSessionMeta {
    let title: Option<String> = data_db
        .query_row(
            "SELECT title FROM sessions WHERE id = ?",
            params![session_id],
            |row| row.get(0),
        )
        .ok()
        .flatten();

    // data.db.sessions has no cwd column; resolve it from the shared
    // session-store.db.sessions table instead.
    let cwd = resolve_session_store_cwd(session_store, session_id);
    CopilotAppSessionMeta { title, cwd }
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

/// Aggregated per-agent token row for a CLI session, produced by grouping
/// `assistant_usage_events` by `(session_id, agent_id, model)`.
///
/// `agent_id = None` identifies the main agent; a non-null `agent_id`
/// identifies a subagent (e.g. `call_f14xiouf`). `model` is taken verbatim
/// from the events so a subagent never inherits the main agent's model.
struct CopilotCliAgentRow {
    session_id: String,
    /// Earliest event timestamp for this agent group (ISO 8601 normalized).
    ts: String,
    model: Option<String>,
    /// `None` for the main agent, `Some(agent_id)` for a subagent.
    agent_id: Option<String>,
    /// `initiator` of the first event for this agent, used only to label
    /// subagent `agent_role` when it is `'sub-agent'`. Never guessed.
    initiator: Option<String>,
    /// Working directory from `session-store.db.sessions.cwd`.
    cwd: Option<String>,
    input_tokens: u64,
    output_tokens: u64,
    cache_read: u64,
    cache_write: u64,
    reasoning: u64,
    duration_ms: u64,
}

/// Reconcile Copilot CLI session usage against the per-API-call attribution in
/// `~/.copilot/session-store.db.assistant_usage_events`.
///
/// The status-line hook writes session-cumulative totals with no `agent_id`,
/// so subagent usage is folded into the main session row. This collector reads
/// the shared session-store (the same database the Copilot App collector uses,
/// but restricted to CLI-classified sessions), aggregates events by
/// `(session_id, agent_id, model)`, and replaces the merged `copilot-cli` hook
/// rows for each reconciled session with one main-agent row plus one row per
/// subagent — preserving the original session total exactly.
///
/// ## Session classification
/// Only sessions classified as [`CopilotAppSessionKind::Cli`] (transcript at
/// `session-state/<id>/events.jsonl` and NOT in the authoritative App registry
/// `data.db.sessions`) are processed. App sessions and unclassifiable sessions
/// are left untouched so the App collector and hook fallback remain
/// authoritative for them.
///
/// ## Token accounting
/// `assistant_usage_events.input_tokens` already includes cache reads, so to
/// avoid double-counting cache-read tokens in both `tokens_input` and
/// `tokens_cache_read` (and again in `tokens_total`), `tokens_input` is stored
/// as `SUM(input_tokens) - SUM(cache_read_tokens)` (clamped at 0), mirroring
/// [`normalize_copilot_cli_usage_entry`]. `tokens_total` follows the Copilot
/// hook's accounting semantics: net input + cache read + output. Cache read is
/// therefore counted once, while reasoning remains a separate breakdown.
///
/// ## Replacing hook rows
/// For each reconciled session, the merged `copilot-cli` hook rows for that
/// session (the main session id and any `__`-suffixed synthetic subagent ids)
/// are deleted and the new split rows are inserted in the same transaction.
/// Hook deltas form a lower bound because the status-line hook may not run for
/// every API call. If hook usage is ahead of the agent events, that session is
/// preserved and marked for retry; other valid sessions still commit.
///
/// ## Cursor & backfill
/// Incremental sync is tracked by a `(created_at, id)` high-water mark scoped
/// by the canonical Copilot directory, stored under
/// [`COPILOT_CLI_AGENT_CURSOR_PREFIX`]. Touched sessions are re-aggregated from
/// their FULL event history (not just post-cursor) so turns straddling the
/// cursor are written with complete totals. A versioned migration
/// ([`COPILOT_CLI_AGENT_MIGRATION_KEY`]) performs the first backfill of all
/// existing CLI sessions. Both the cursor and the migration key are
/// independent of the Copilot App collector's state.
fn sync_copilot_cli_agent_usage_logs(conn: &mut Connection) -> Result<(), String> {
    let copilot_dir = get_copilot_dir();
    let session_store_path = copilot_dir.join("session-store.db");

    // Canonicalize for a stable, per-COPILOT_DIR cursor key (mirrors the App
    // collector). Hex-encode raw OS bytes so the key is injective and free of
    // LIKE wildcards.
    let canonical_copilot_dir = copilot_dir
        .canonicalize()
        .unwrap_or_else(|_| copilot_dir.clone());
    let source_key = encode_hex(canonical_copilot_dir.as_os_str().as_encoded_bytes());
    let cursor_key_prefix = format!("{}{}::", COPILOT_CLI_AGENT_CURSOR_PREFIX, source_key);
    let pending_key_prefix = format!("{}{}::", COPILOT_CLI_AGENT_PENDING_PREFIX, source_key);

    // CLI reconciliation must not touch App sessions. A missing data.db /
    // sessions table yields an empty registry (normal for CLI-only users) —
    // safe because a session only classifies as CLI when its transcript
    // exists, so App sessions stay Unknown even with an empty registry.
    // Genuine I/O or schema failures skip reconciliation, keeping hook rows.
    let data_db_path = copilot_dir.join("data.db");
    let app_session_ids: HashSet<String> = match load_copilot_app_session_registry(&data_db_path) {
        Ok(ids) => ids,
        Err(message) => {
            eprintln!("⚠️ Copilot CLI agent reconciliation 跳過：{}", message);
            return Ok(());
        }
    };

    if !session_store_path.exists() {
        // No session-store: keep hook rows for all CLI sessions.
        return Ok(());
    }

    let session_store = match Connection::open_with_flags(
        &session_store_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "⚠️ 無法開啟 Copilot CLI session-store.db ({}): {}",
                session_store_path.display(),
                e
            );
            return Ok(());
        }
    };
    let _ = session_store.busy_timeout(std::time::Duration::from_secs(2));

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

    // Determine the set of CLI sessions to process. On the first run (no
    // migration marker) scan every session in the store that classifies as
    // CLI; subsequently use the cursor to find newly-touched sessions and
    // re-aggregate them from full history.
    let migration_done: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_state WHERE filename = ?)",
            params![COPILOT_CLI_AGENT_MIGRATION_KEY],
            |row| row.get(0),
        )
        .unwrap_or(false);

    let stored_cursor: Option<String> = conn
        .query_row(
            "SELECT filename FROM sync_state WHERE filename LIKE ? ESCAPE '\\' LIMIT 1",
            params![format!("{}%", cursor_key_prefix)],
            |row| row.get::<_, String>(0),
        )
        .ok()
        .and_then(|f| f.strip_prefix(&cursor_key_prefix).map(|s| s.to_string()));
    let (last_cursor, _legacy_cursor) = stored_cursor
        .map(|suffix| parse_copilot_app_cursor(&suffix))
        .unwrap_or((None, false));

    // Collect candidate session ids and the max event (created_at, id).
    let touched_query = if last_cursor.is_some() {
        "SELECT session_id, created_at, id
         FROM assistant_usage_events
         WHERE created_at > ?
            OR (created_at = ? AND id > ?)
         ORDER BY created_at ASC, id ASC"
    } else {
        "SELECT session_id, created_at, id
         FROM assistant_usage_events
         ORDER BY created_at ASC, id ASC"
    };
    let mut touched_stmt = session_store
        .prepare(touched_query)
        .map_err(|e| format!("準備 Copilot CLI touched-sessions 查詢失敗: {}", e))?;
    let map_touched = |row: &rusqlite::Row| -> rusqlite::Result<(String, String, i64)> {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    };
    let touched_iter = if let Some(ref cursor) = last_cursor {
        touched_stmt
            .query_map(
                params![cursor.0.as_str(), cursor.0.as_str(), cursor.1],
                map_touched,
            )
            .map_err(|e| format!("執行 Copilot CLI touched-sessions 查詢失敗: {}", e))?
    } else {
        touched_stmt
            .query_map([], map_touched)
            .map_err(|e| format!("執行 Copilot CLI touched-sessions 查詢失敗: {}", e))?
    };

    let mut touched_cli_sessions: HashSet<String> = HashSet::new();
    let mut max_event_cursor: Option<(String, i64)> = None;
    let mut scan_failed = false;
    for row_res in touched_iter {
        match row_res {
            Ok((session_id, created_at, id)) => {
                max_event_cursor = Some((created_at, id));
                if matches!(
                    classify_copilot_app_session(&copilot_dir, &app_session_ids, &session_id),
                    CopilotAppSessionKind::Cli
                ) {
                    touched_cli_sessions.insert(session_id);
                }
            }
            Err(e) => {
                eprintln!("⚠️ 讀取 Copilot CLI touched-session 失敗: {}", e);
                scan_failed = true;
            }
        }
    }
    if scan_failed {
        return Ok(());
    }

    // Retry sessions whose hook totals were previously ahead of the agent
    // event store. The marker also records the compared totals so an unchanged
    // mismatch is not logged repeatedly on every application sync.
    let mut pending_totals: HashMap<String, (i64, i64)> = HashMap::new();
    let mut pending_stmt = conn
        .prepare(
            "SELECT filename, last_synced_size, last_synced_time
             FROM sync_state
             WHERE filename LIKE ? ESCAPE '\\'",
        )
        .map_err(|e| format!("準備 Copilot CLI pending-session 查詢失敗: {}", e))?;
    let pending_rows = pending_stmt
        .query_map(params![format!("{}%", pending_key_prefix)], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(|e| format!("執行 Copilot CLI pending-session 查詢失敗: {}", e))?;
    for row in pending_rows {
        let (filename, hook_total, agent_total) =
            row.map_err(|e| format!("讀取 Copilot CLI pending-session 失敗: {}", e))?;
        let Some(session_id) = filename.strip_prefix(&pending_key_prefix) else {
            continue;
        };
        if matches!(
            classify_copilot_app_session(&copilot_dir, &app_session_ids, session_id),
            CopilotAppSessionKind::Cli
        ) {
            touched_cli_sessions.insert(session_id.to_string());
            pending_totals.insert(session_id.to_string(), (hook_total, agent_total));
        }
    }
    drop(pending_stmt);

    // First-run backfill: scan every CLI-classified session in the store, not
    // just those after the cursor. This converts existing hook merged rows
    // into per-agent rows. Idempotent: the migration marker is set only after
    // a successful commit, and the cursor prevents re-scanning on retry.
    if !migration_done {
        let mut all_sessions_stmt = session_store
            .prepare("SELECT DISTINCT session_id FROM assistant_usage_events")
            .map_err(|e| format!("準備 Copilot CLI backfill 查詢失敗: {}", e))?;
        let all_sessions = all_sessions_stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| format!("執行 Copilot CLI backfill 查詢失敗: {}", e))?;
        for sid_res in all_sessions {
            match sid_res {
                Ok(sid) => {
                    if matches!(
                        classify_copilot_app_session(&copilot_dir, &app_session_ids, &sid),
                        CopilotAppSessionKind::Cli
                    ) {
                        touched_cli_sessions.insert(sid);
                    }
                }
                Err(e) => {
                    eprintln!("⚠️ 讀取 Copilot CLI backfill session 失敗: {}", e);
                    return Ok(());
                }
            }
        }
    }

    if touched_cli_sessions.is_empty() && max_event_cursor.is_none() {
        // Nothing to do. Still record the migration marker on first run so the
        // backfill scan is not repeated.
        if !migration_done {
            let tx = conn.transaction().map_err(|e| {
                format!("開啟 Copilot CLI migration marker transaction 失敗: {}", e)
            })?;
            tx.execute(
                "INSERT OR REPLACE INTO sync_state (filename, last_synced_size, last_synced_time)
                 VALUES (?, 1, 0)",
                params![COPILOT_CLI_AGENT_MIGRATION_KEY],
            )
            .map_err(|e| format!("寫入 Copilot CLI migration marker 失敗: {}", e))?;
            if let Some((created_at, id)) = max_event_cursor {
                write_copilot_cli_agent_cursor(&tx, &cursor_key_prefix, &created_at, id)?;
            }
            tx.commit()
                .map_err(|e| format!("Copilot CLI migration marker COMMIT 失敗: {}", e))?;
        }
        return Ok(());
    }

    // Aggregate each touched CLI session from its FULL event history, grouped
    // by (agent_id, model). turn_index is unreliable for CLI (often all 0), so
    // we aggregate across all turns for a stable, non-duplicated per-agent row.
    // `MIN(model)` is safe because the group key already includes model.
    let aggregate_query = "SELECT MIN(created_at) AS ts,
                MIN(model) AS model,
                agent_id,
                MIN(initiator) AS initiator,
                SUM(input_tokens), SUM(output_tokens),
                SUM(cache_read_tokens), SUM(cache_write_tokens),
                SUM(reasoning_tokens), SUM(duration_ms)
         FROM assistant_usage_events
         WHERE session_id = ?
         GROUP BY agent_id, model";
    let mut agg_stmt = session_store
        .prepare(aggregate_query)
        .map_err(|e| format!("準備 Copilot CLI 聚合查詢失敗: {}", e))?;

    let mut all_session_rows: HashMap<String, Vec<CopilotCliAgentRow>> = HashMap::new();
    for session_id in &touched_cli_sessions {
        // Resolve CWD once per session from session-store.db.sessions.
        let session_cwd = resolve_session_store_cwd(&session_store, session_id);

        let rows_res = agg_stmt.query_map(params![session_id], |row| {
            let raw_input: i64 = row.get::<_, Option<i64>>(4)?.unwrap_or(0).max(0);
            let cache_read: i64 = row.get::<_, Option<i64>>(6)?.unwrap_or(0).max(0);
            // Net non-cached input; clamp at 0 in case of schema drift.
            let net_input = (raw_input - cache_read).max(0) as u64;
            Ok(CopilotCliAgentRow {
                session_id: session_id.clone(),
                ts: row.get::<_, String>(0)?,
                model: row.get::<_, Option<String>>(1)?,
                agent_id: row.get::<_, Option<String>>(2)?,
                initiator: row.get::<_, Option<String>>(3)?,
                cwd: session_cwd.clone(),
                input_tokens: net_input,
                output_tokens: row.get::<_, Option<i64>>(5)?.unwrap_or(0).max(0) as u64,
                cache_read: cache_read as u64,
                cache_write: row.get::<_, Option<i64>>(7)?.unwrap_or(0).max(0) as u64,
                reasoning: row.get::<_, Option<i64>>(8)?.unwrap_or(0).max(0) as u64,
                duration_ms: row.get::<_, Option<i64>>(9)?.unwrap_or(0).max(0) as u64,
            })
        });
        match rows_res {
            Ok(rows) => {
                let mut agent_rows = Vec::new();
                for row in rows {
                    match row {
                        Ok(r) => agent_rows.push(r),
                        Err(e) => {
                            eprintln!(
                                "⚠️ 讀取 Copilot CLI 聚合結果 (session {}) 失敗: {}",
                                session_id, e
                            );
                            return Ok(());
                        }
                    }
                }
                if agent_rows.is_empty() {
                    // No agent events for this session: keep hook rows.
                    continue;
                }
                all_session_rows.insert(session_id.clone(), agent_rows);
            }
            Err(e) => {
                eprintln!("⚠️ 聚合 Copilot CLI session {} 失敗: {}", session_id, e);
                return Ok(());
            }
        }
    }

    if all_session_rows.is_empty() && max_event_cursor.is_none() {
        return Ok(());
    }

    let tx = conn.transaction().map_err(|e| {
        format!(
            "開啟 Copilot CLI agent reconciliation transaction 失敗: {}",
            e
        )
    })?;

    let mut upserted = 0usize;
    let mut hook_replaced = 0usize;

    for (session_id, agent_rows) in &all_session_rows {
        // Compare the per-agent total against the raw status-line hook rows
        // BEFORE deleting anything. Copilot's raw input includes cache reads,
        // while the normalized database input excludes them. Add cache read
        // back exactly once so this total matches `tokens.total`.
        let agent_total: u64 = agent_rows
            .iter()
            .map(|r| r.input_tokens + r.cache_read + r.output_tokens)
            .sum();
        let agent_total = i64::try_from(agent_total).unwrap_or(i64::MAX);

        let hook_total: Option<i64> = tx
            .query_row(
                "SELECT COALESCE(SUM(delta_total), MAX(tokens_total))
                 FROM usage_entries
                 WHERE assistant_type = 'copilot'
                   AND source_kind = ?
                   AND (session_id = ? OR session_id LIKE ? ESCAPE '\\')
                   AND parent_session_id IS NULL
                   AND import_source_id IS NULL",
                params![
                    COPILOT_CLI_SOURCE_KIND,
                    session_id,
                    format!("{}\\__%", session_id)
                ],
                |row| row.get::<_, Option<i64>>(0),
            )
            .ok()
            .flatten();

        // assistant_usage_events records every API call, while the status-line
        // hook runs only when Copilot redraws it. Agent usage may therefore be
        // greater than hook usage and is authoritative in that direction. If
        // the hook is greater, keep this session intact and retry it later
        // without rolling back other sessions in the same batch.
        if hook_total.is_some_and(|hook_total| hook_total > agent_total) {
            let hook_total = hook_total.unwrap_or_default();
            let compared_totals = (hook_total, agent_total);
            if pending_totals.get(session_id) != Some(&compared_totals) {
                eprintln!(
                    "⚠️ Copilot CLI session {} 尚待 agent events 補齊（hook={} agent={}），保留 hook rows",
                    session_id, hook_total, agent_total
                );
            }
            tx.execute(
                "INSERT OR REPLACE INTO sync_state (
                    filename, last_synced_size, last_synced_time
                 ) VALUES (?, ?, ?)",
                params![
                    format!("{}{}", pending_key_prefix, session_id),
                    hook_total,
                    agent_total
                ],
            )
            .map_err(|e| {
                format!(
                    "寫入 Copilot CLI pending-session 失敗 (session {}): {}",
                    session_id, e
                )
            })?;
            continue;
        }

        tx.execute(
            "DELETE FROM sync_state WHERE filename = ?",
            params![format!("{}{}", pending_key_prefix, session_id)],
        )
        .map_err(|e| {
            format!(
                "清除 Copilot CLI pending-session 失敗 (session {}): {}",
                session_id, e
            )
        })?;

        // Delete the merged hook rows for this session: the main session id
        // and any synthetic subagent ids. Scoped precisely to copilot-cli so
        // copilot-app, vscode-chat, codex, claude, cursor and antigravity rows
        // are never touched.
        let deleted = tx
            .execute(
                "DELETE FROM usage_entries
                 WHERE assistant_type = 'copilot'
                   AND source_kind = ?
                   AND (session_id = ? OR session_id LIKE ? ESCAPE '\\')",
                params![
                    COPILOT_CLI_SOURCE_KIND,
                    session_id,
                    format!("{}\\__%", session_id)
                ],
            )
            .map_err(|e| {
                format!(
                    "刪除 Copilot CLI hook rows 失敗 (session {}): {}",
                    session_id, e
                )
            })?;
        hook_replaced += deleted;

        // Insert the split per-agent rows.
        for row in agent_rows {
            let timestamp = normalize_copilot_app_timestamp(&row.ts);
            let date_str = timestamp.get(..10).unwrap_or(&row.ts).to_string();
            // CLI accounting mirrors the hook: raw input + output. Since
            // `input_tokens` was normalized to exclude cache reads, add
            // `cache_read` back once. Reasoning and cache write remain separate
            // breakdowns and are not added to tokens_total.
            let total = row.input_tokens + row.cache_read + row.output_tokens;

            let (row_session_id, parent_session_id, agent_nickname, agent_id_segment, agent_role) =
                match &row.agent_id {
                    Some(agent) if !agent.is_empty() => {
                        let synthetic = format!("{}__{}", row.session_id, agent);
                        // Only surface agent_role when the source explicitly
                        // marked the agent as a sub-agent; never guess.
                        let role = if row.initiator.as_deref() == Some("sub-agent") {
                            Some("sub-agent".to_string())
                        } else {
                            None
                        };
                        (
                            synthetic,
                            Some(row.session_id.clone()),
                            Some(agent.clone()),
                            agent.clone(),
                            role,
                        )
                    }
                    _ => (row.session_id.clone(), None, None, "main".to_string(), None),
                };

            let session_name = get_copilot_session_name(&row.session_id);
            let session_name = match (&row.agent_id, &session_name) {
                (Some(agent), Some(name)) if !agent.is_empty() => {
                    Some(format!("{} (subagent {})", name, agent))
                }
                (Some(agent), None) if !agent.is_empty() => Some(format!("Subagent {}", agent)),
                _ => session_name,
            };

            // import_source_id namespace is distinct from copilot-app, the
            // hook's FNV hash, and vscode-chat. Includes the canonical source
            // directory, session, agent and model so re-runs upsert the same
            // row instead of duplicating.
            let model_identity = encode_hex(row.model.as_deref().unwrap_or("").as_bytes());
            let usage_identity = format!(
                "agent={};model={}",
                encode_hex(agent_id_segment.as_bytes()),
                model_identity
            );
            let import_source_id = format!(
                "copilot-cli-agents:{}:{}:{}:{}",
                source_key, row.session_id, agent_id_segment, model_identity
            );

            let insert_res = tx.execute(
                "INSERT OR REPLACE INTO usage_entries (
                    assistant_type, source_kind, usage_identity, timestamp, date, session_id, session_name,
                    transcript_path, cwd, version, turn_no, model, model_id,
                    tokens_input, tokens_output, tokens_cache_read, tokens_cache_write,
                    tokens_reasoning, tokens_total,
                    delta_input, delta_output, delta_cache_read, delta_cache_write,
                    delta_reasoning, delta_total,
                    duration_ms, premium_requests, import_source_id, reasoning_effort,
                    parent_session_id, agent_nickname, agent_role
                ) VALUES (
                    ?, ?, ?, ?, ?, ?, ?,
                    NULL, ?, NULL, ?, ?, ?,
                    ?, ?, ?, ?, ?, ?,
                    ?, ?, ?, ?, ?, ?,
                    ?, NULL, ?, NULL,
                    ?, ?, ?
                )",
                params![
                    "copilot",
                    COPILOT_CLI_SOURCE_KIND,
                    usage_identity,
                    timestamp,
                    date_str,
                    row_session_id,
                    session_name,
                    row.cwd.as_deref(),
                    // CLI sessions aggregate across all turns into a single
                    // row per agent, so turn_no is fixed at 1.
                    1i64,
                    row.model,
                    row.model,
                    row.input_tokens as i64,
                    row.output_tokens as i64,
                    row.cache_read as i64,
                    row.cache_write as i64,
                    row.reasoning as i64,
                    total as i64,
                    row.input_tokens as i64,
                    row.output_tokens as i64,
                    row.cache_read as i64,
                    row.cache_write as i64,
                    row.reasoning as i64,
                    total as i64,
                    row.duration_ms as i64,
                    import_source_id,
                    parent_session_id,
                    agent_nickname,
                    agent_role,
                ],
            );
            match insert_res {
                Ok(_) => upserted += 1,
                Err(e) => {
                    eprintln!(
                        "⚠️ 寫入 Copilot CLI agent usage 失敗 (session {}): {}",
                        row.session_id, e
                    );
                    let _ = tx.rollback();
                    return Ok(());
                }
            }
        }
    }

    // Advance the cursor to the max event tuple seen. Only commit after all
    // sessions are written successfully; on any earlier rollback the cursor
    // stays put so failed sessions are retried.
    if let Some((created_at, id)) = max_event_cursor {
        if let Err(e) = write_copilot_cli_agent_cursor(&tx, &cursor_key_prefix, &created_at, id) {
            eprintln!("⚠️ 寫入 Copilot CLI agent cursor 失敗: {}", e);
            let _ = tx.rollback();
            return Ok(());
        }
    }

    // Record the migration marker now that the backfill has committed.
    if !migration_done {
        tx.execute(
            "INSERT OR REPLACE INTO sync_state (filename, last_synced_size, last_synced_time)
             VALUES (?, 1, 0)",
            params![COPILOT_CLI_AGENT_MIGRATION_KEY],
        )
        .map_err(|e| format!("寫入 Copilot CLI migration marker 失敗: {}", e))?;
    }

    tx.commit()
        .map_err(|e| format!("Copilot CLI agent reconciliation COMMIT 失敗: {}", e))?;

    if upserted > 0 {
        println!(
            "✅ 同步 Copilot CLI agent：{} 筆 per-agent row（upsert）",
            upserted
        );
    }
    if hook_replaced > 0 {
        println!(
            "✅ Copilot CLI agent reconciliation：替換 {} 筆 hook merged row",
            hook_replaced
        );
    }
    Ok(())
}

/// One-time backfill: populate `cwd` for Copilot rows that were written before
/// CWD was resolved from `session-store.db.sessions`. Covers both `copilot-cli`
/// and `copilot-app` source kinds. Only fills rows where `cwd IS NULL` and the
/// session-store has a non-trivial CWD for the session.
fn backfill_copilot_cwd(conn: &mut Connection) -> Result<(), String> {
    let migration_done: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_state WHERE filename = ?)",
            params![COPILOT_CWD_BACKFILL_MIGRATION_KEY],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if migration_done {
        return Ok(());
    }

    let copilot_dir = get_copilot_dir();
    let session_store_path = copilot_dir.join("session-store.db");
    if !session_store_path.exists() {
        // Nothing to backfill from; mark as done so we don't retry every sync.
        return mark_copilot_cwd_backfill_done(conn);
    }

    let session_store = match Connection::open_with_flags(
        &session_store_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) {
        Ok(c) => c,
        Err(_) => return mark_copilot_cwd_backfill_done(conn),
    };
    let _ = session_store.busy_timeout(std::time::Duration::from_secs(2));

    let sessions_table_exists: bool = session_store
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='sessions'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);
    if !sessions_table_exists {
        return mark_copilot_cwd_backfill_done(conn);
    }

    // Collect distinct session_ids that need a CWD backfill.
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT session_id FROM usage_entries
             WHERE assistant_type = 'copilot' AND cwd IS NULL",
        )
        .map_err(|e| format!("準備 Copilot CWD backfill 查詢失敗: {}", e))?;
    let session_ids: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| format!("執行 Copilot CWD backfill 查詢失敗: {}", e))?
        .filter_map(Result::ok)
        .collect();
    drop(stmt);

    if session_ids.is_empty() {
        return mark_copilot_cwd_backfill_done(conn);
    }

    let tx = conn
        .transaction()
        .map_err(|e| format!("開啟 Copilot CWD backfill transaction 失敗: {}", e))?;

    let mut updated = 0usize;
    for session_id in &session_ids {
        let cwd = match resolve_session_store_cwd(&session_store, session_id) {
            Some(cwd) => cwd,
            None => continue,
        };
        updated += tx
            .execute(
                "UPDATE usage_entries
                 SET cwd = ?
                 WHERE assistant_type = 'copilot'
                   AND session_id = ?
                   AND cwd IS NULL",
                params![cwd, session_id],
            )
            .map_err(|e| format!("更新 Copilot CWD 失敗 (session {}): {}", session_id, e))?;
    }

    tx.execute(
        "INSERT OR REPLACE INTO sync_state (filename, last_synced_size, last_synced_time)
         VALUES (?, 1, 0)",
        params![COPILOT_CWD_BACKFILL_MIGRATION_KEY],
    )
    .map_err(|e| format!("寫入 Copilot CWD backfill marker 失敗: {}", e))?;

    tx.commit()
        .map_err(|e| format!("Copilot CWD backfill COMMIT 失敗: {}", e))?;

    if updated > 0 {
        println!("✅ 補填 Copilot CWD：{} 筆", updated);
    }
    Ok(())
}

fn mark_copilot_cwd_backfill_done(conn: &mut Connection) -> Result<(), String> {
    conn.execute(
        "INSERT OR REPLACE INTO sync_state (filename, last_synced_size, last_synced_time)
         VALUES (?, 1, 0)",
        params![COPILOT_CWD_BACKFILL_MIGRATION_KEY],
    )
    .map_err(|e| format!("寫入 Copilot CWD backfill marker 失敗: {}", e))?;
    Ok(())
}

/// Load the authoritative Copilot App session registry (`data.db.sessions`).
/// Returns an empty set if `data.db` or the `sessions` table is missing (a
/// normal state for CLI-only users); returns an error string for genuine I/O
/// or schema failures so the caller can decide whether to fall back.
fn load_copilot_app_session_registry(data_db_path: &Path) -> Result<HashSet<String>, String> {
    if !data_db_path.exists() {
        return Ok(HashSet::new());
    }
    let data_db = Connection::open_with_flags(
        data_db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("無法開啟 data.db ({}): {}", data_db_path.display(), e))?;
    let _ = data_db.busy_timeout(std::time::Duration::from_secs(2));

    let table_exists: bool = data_db
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='sessions'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);
    if !table_exists {
        return Ok(HashSet::new());
    }

    let mut stmt = data_db
        .prepare("SELECT id FROM sessions")
        .map_err(|e| format!("讀取 data.db.sessions 失敗: {}", e))?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| format!("讀取 data.db.sessions 失敗: {}", e))?;
    let mut ids = HashSet::new();
    for row in rows {
        ids.insert(row.map_err(|e| format!("讀取 data.db.sessions 失敗: {}", e))?);
    }
    Ok(ids)
}

fn write_copilot_cli_agent_cursor(
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
    .map_err(|e| format!("刪除舊 Copilot CLI agent cursor 失敗: {}", e))?;
    let cursor_sentinel = format!("{}{}::{}", cursor_key_prefix, created_at, id);
    tx.execute(
        "INSERT OR REPLACE INTO sync_state (filename, last_synced_size, last_synced_time) VALUES (?, ?, ?)",
        params![cursor_sentinel, 0i64, now],
    )
    .map_err(|e| format!("寫入 Copilot CLI agent cursor 失敗: {}", e))?;
    Ok(())
}

fn codex_transcript_path_key_for_platform(path: &str, is_windows: bool) -> String {
    if is_windows {
        path.replace('\\', "/").to_ascii_lowercase()
    } else {
        path.to_string()
    }
}

fn codex_transcript_path_key(path: &str) -> String {
    codex_transcript_path_key_for_platform(path, cfg!(windows))
}

fn group_codex_transcript_paths(
    paths: impl IntoIterator<Item = String>,
    is_windows: bool,
) -> HashMap<String, Vec<String>> {
    let mut grouped_paths: HashMap<String, Vec<String>> = HashMap::new();
    for path in paths {
        grouped_paths
            .entry(codex_transcript_path_key_for_platform(&path, is_windows))
            .or_default()
            .push(path);
    }
    grouped_paths
}

fn load_codex_transcript_paths(conn: &Connection) -> Result<HashMap<String, Vec<String>>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT transcript_path
             FROM usage_entries
             WHERE assistant_type = 'codex' AND transcript_path IS NOT NULL",
        )
        .map_err(|error| format!("準備讀取 Codex transcript 路徑失敗: {error}"))?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| format!("讀取 Codex transcript 路徑失敗: {error}"))?;
    let mut paths = Vec::new();
    for row in rows {
        let path = row.map_err(|error| format!("解析 Codex transcript 路徑失敗: {error}"))?;
        paths.push(path);
    }
    Ok(group_codex_transcript_paths(paths, cfg!(windows)))
}

fn codex_transcript_needs_sync(
    current_size: u64,
    last_synced_state: Option<(u64, i64)>,
    transcript_is_current: bool,
) -> bool {
    match last_synced_state {
        None => true,
        Some((last_synced_size, _)) if last_synced_size != current_size => true,
        Some((_, last_synced_time)) => {
            !transcript_is_current && last_synced_time != CODEX_EMPTY_TRANSCRIPT_SYNC_TIME
        }
    }
}

fn sync_codex_usage_logs(conn: &mut Connection) -> Result<(), String> {
    let codex_dir = get_codex_dir();

    run_codex_parser_migration(conn)?;
    run_codex_source_kind_migration(conn)?;

    let mut files = Vec::new();
    for directory in [
        codex_dir.join("sessions"),
        codex_dir.join("archived_sessions"),
    ] {
        files.extend(find_codex_session_files(&directory));
    }
    files.sort();

    if files.is_empty() {
        return Ok(());
    }

    let transcript_paths = load_codex_transcript_paths(conn)?;

    for filepath in files {
        let state_path = portable_relative_path(&codex_dir, &filepath);
        let state_key = format!("codex:{}", state_path);

        let last_synced_state: Option<(u64, i64)> = conn
            .query_row(
                "SELECT last_synced_size, last_synced_time
                 FROM sync_state
                 WHERE filename = ?",
                params![state_key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok();

        let metadata = match fs::metadata(&filepath) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        let current_size = metadata.len();
        let transcript_path = filepath.to_string_lossy().into_owned();
        let transcript_path_key = codex_transcript_path_key(&transcript_path);
        let known_transcript_paths = transcript_paths.get(&transcript_path_key);
        let transcript_is_current = known_transcript_paths.is_some();

        if codex_transcript_needs_sync(current_size, last_synced_state, transcript_is_current) {
            let parsed_entries = match parse_codex_session_file(&filepath) {
                Ok(entries) => entries,
                Err(e) => {
                    eprintln!("解析 Codex 會話檔案 {:?} 失敗: {}", filepath, e);
                    continue;
                }
            };

            if parsed_entries.is_empty() {
                conn.execute(
                    "INSERT OR REPLACE INTO sync_state
                     (filename, last_synced_size, last_synced_time)
                     VALUES (?, ?, ?)",
                    params![
                        state_key,
                        current_size as i64,
                        CODEX_EMPTY_TRANSCRIPT_SYNC_TIME
                    ],
                )
                .map_err(|error| format!("記錄空白 Codex transcript 同步狀態失敗: {error}"))?;
                continue;
            }

            let tx = conn
                .transaction()
                .map_err(|e| format!("Transaction BEGIN 失敗: {}", e))?;

            if let Some(existing_paths) = known_transcript_paths {
                for existing_path in existing_paths {
                    tx.execute(
                        "DELETE FROM usage_entries
                         WHERE assistant_type = 'codex' AND transcript_path = ?",
                        params![existing_path],
                    )
                    .map_err(|e| format!("清空舊 Codex transcript 資料失敗: {}", e))?;
                }
            } else {
                tx.execute(
                    "DELETE FROM usage_entries
                     WHERE assistant_type = 'codex' AND transcript_path = ?",
                    params![transcript_path],
                )
                .map_err(|e| format!("清空舊 Codex transcript 資料失敗: {}", e))?;
            }

            let session_ids: HashSet<String> = parsed_entries
                .iter()
                .map(|entry| entry.session_id.clone())
                .collect();
            for session_id in session_ids {
                tx.execute(
                    "DELETE FROM usage_entries WHERE assistant_type = 'codex' AND session_id = ?",
                    params![session_id],
                )
                .map_err(|e| format!("清空舊 Codex Session 資料失敗: {}", e))?;
            }

            let mut success = true;
            for entry in &parsed_entries {
                let tokens = entry.tokens.as_ref();
                let delta = entry.delta_tokens.as_ref();
                let cost = entry.cost.as_ref();

                let insert_res = tx.execute(
                    "INSERT INTO usage_entries (
                        assistant_type, source_kind, timestamp, date, session_id, session_name, transcript_path, cwd, version, turn_no, model, model_id,
                        tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, tokens_cache_write_5m, tokens_cache_write_1h, tokens_reasoning, tokens_total,
                        delta_input, delta_output, delta_cache_read, delta_cache_write, delta_cache_write_5m, delta_cache_write_1h, delta_reasoning, delta_total,
                        duration_ms, premium_requests, parent_session_id, agent_nickname, agent_role, reasoning_effort
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    params![
                        "codex",
                        entry.source_kind.as_deref().unwrap_or(CODEX_OTHER_SOURCE_KIND),
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
                        tokens.and_then(|t| t.cache_write_5m.map(|v| v as i64)),
                        tokens.and_then(|t| t.cache_write_1h.map(|v| v as i64)),
                        tokens.and_then(|t| t.reasoning.map(|v| v as i64)),
                        tokens.map(|t| t.total as i64),
                        delta.map(|t| t.input as i64),
                        delta.map(|t| t.output as i64),
                        delta.and_then(|t| t.cache_read.map(|v| v as i64)),
                        delta.and_then(|t| t.cache_write.map(|v| v as i64)),
                        delta.and_then(|t| t.cache_write_5m.map(|v| v as i64)),
                        delta.and_then(|t| t.cache_write_1h.map(|v| v as i64)),
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

        let input = usage.input_tokens;
        let cache_read = usage.cache_read_input_tokens;
        let reported_cache_write = usage.cache_creation_input_tokens;
        let explicit_cache_write_5m = usage.cache_creation.ephemeral_5m_input_tokens;
        let cache_write_1h = usage.cache_creation.ephemeral_1h_input_tokens;
        let explicit_cache_write = explicit_cache_write_5m.saturating_add(cache_write_1h);
        let cache_write = reported_cache_write.max(explicit_cache_write);
        let cache_write_5m = explicit_cache_write_5m
            .saturating_add(reported_cache_write.saturating_sub(explicit_cache_write));
        let output = usage.output_tokens;
        let total = input
            .saturating_add(cache_read)
            .saturating_add(cache_write)
            .saturating_add(output);
        let tokens = TokenStats {
            input,
            output,
            cache_read: Some(cache_read),
            cache_write: Some(cache_write),
            cache_write_5m: Some(cache_write_5m),
            cache_write_1h: Some(cache_write_1h),
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
                        tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, tokens_cache_write_5m, tokens_cache_write_1h, tokens_reasoning, tokens_total,
                        delta_input, delta_output, delta_cache_read, delta_cache_write, delta_cache_write_5m, delta_cache_write_1h, delta_reasoning, delta_total,
                        duration_ms, premium_requests, parent_session_id, agent_nickname, agent_role, reasoning_effort
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
                        tokens.and_then(|t| t.cache_write_5m.map(|v| v as i64)),
                        tokens.and_then(|t| t.cache_write_1h.map(|v| v as i64)),
                        tokens.and_then(|t| t.reasoning.map(|v| v as i64)),
                        tokens.map(|t| t.total as i64),
                        delta.map(|t| t.input as i64),
                        delta.map(|t| t.output as i64),
                        delta.and_then(|t| t.cache_read.map(|v| v as i64)),
                        delta.and_then(|t| t.cache_write.map(|v| v as i64)),
                        delta.and_then(|t| t.cache_write_5m.map(|v| v as i64)),
                        delta.and_then(|t| t.cache_write_1h.map(|v| v as i64)),
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

fn cursor_response_signature(content: &serde_json::Value) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(text) = content.as_str() {
        if !text.is_empty() {
            parts.push(serde_json::json!(["text", text]));
        }
    } else {
        for item in content.as_array()? {
            match item.get("type").and_then(|value| value.as_str()) {
                Some("text") => {
                    if let Some(text) = item
                        .get("text")
                        .or_else(|| item.get("data"))
                        .and_then(|value| value.as_str())
                        .filter(|value| !value.is_empty())
                    {
                        parts.push(serde_json::json!(["text", text]));
                    }
                }
                Some("tool_use") => {
                    let Some(name) = item.get("name").and_then(|value| value.as_str()) else {
                        continue;
                    };
                    parts.push(serde_json::json!([
                        "tool",
                        name,
                        item.get("input")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null)
                    ]));
                }
                Some("tool-call") => {
                    let Some(name) = item.get("toolName").and_then(|value| value.as_str()) else {
                        continue;
                    };
                    parts.push(serde_json::json!([
                        "tool",
                        name,
                        item.get("args").cloned().unwrap_or(serde_json::Value::Null)
                    ]));
                }
                _ => {}
            }
        }
    }
    if parts.is_empty() {
        return None;
    }
    let serialized = serde_json::to_string(&parts).ok()?;
    Some(format!(
        "{:016x}",
        hash_fnv1a_64(&format!("cursor-response-v2:{serialized}"))
    ))
}

fn cursor_model_from_provider_options(value: &serde_json::Value) -> Option<String> {
    value
        .get("providerOptions")
        .and_then(|provider_options| provider_options.get("cursor"))
        .and_then(|cursor| cursor.get("modelName"))
        .and_then(|model| model.as_str())
        .map(str::trim)
        .filter(|model| !model.is_empty() && model.len() <= 200)
        .map(str::to_string)
}

fn parse_cursor_agent_kv_model_signature(raw: &[u8]) -> Option<(String, String)> {
    let event: serde_json::Value = serde_json::from_slice(raw).ok()?;
    if event.get("role").and_then(|value| value.as_str()) != Some("assistant") {
        return None;
    }
    let content = event
        .get("content")
        .or_else(|| event.pointer("/message/content"))?;
    let mut models = HashSet::new();
    if let Some(model) = cursor_model_from_provider_options(&event) {
        models.insert(model);
    }
    if let Some(items) = content.as_array() {
        for item in items {
            if let Some(model) = cursor_model_from_provider_options(item) {
                models.insert(model);
            }
        }
    }
    if models.len() != 1 {
        return None;
    }
    Some((
        cursor_response_signature(content)?,
        models.into_iter().next()?,
    ))
}

fn cursor_model_source_id(path: &Path) -> String {
    let resolved = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let normalized = resolved.to_string_lossy().replace('\\', "/");
    #[cfg(windows)]
    let normalized = normalized.to_lowercase();
    format!("{:016x}", hash_fnv1a_64(&normalized))
}

fn cursor_mode_source_kind(mode: Option<&str>) -> Option<String> {
    match mode {
        Some("agent") => Some(CURSOR_AGENT_SOURCE_KIND.to_string()),
        Some("ide") => Some(CURSOR_IDE_SOURCE_KIND.to_string()),
        _ => None,
    }
}

fn cursor_date_from_timestamp(timestamp: &str) -> Option<&str> {
    let date = timestamp.get(..10)?;
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    Some(date)
}

fn run_cursor_model_attribution_migration(conn: &mut Connection) -> Result<(), String> {
    let already_applied: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_state WHERE filename = ?)",
            params![CURSOR_MODEL_ATTRIBUTION_MIGRATION_KEY],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if already_applied {
        return Ok(());
    }

    let tx = conn
        .transaction()
        .map_err(|error| format!("啟動 Cursor 模型歸因遷移失敗: {error}"))?;
    tx.execute(
        "UPDATE usage_entries
         SET model = 'Unknown Model', model_id = 'Unknown Model'
         WHERE assistant_type = 'cursor'
           AND (model IS NULL OR model = '' OR model = 'Cursor Agent')",
        [],
    )
    .map_err(|error| format!("重設 Cursor 籠統模型名稱失敗: {error}"))?;
    tx.execute(
        "DELETE FROM sync_state
         WHERE filename LIKE 'cursor:%'
            OR filename LIKE 'cursor-agent-kv:%'
            OR filename LIKE 'cursor-composer-data:%'",
        [],
    )
    .map_err(|error| format!("重設 Cursor 同步狀態失敗: {error}"))?;
    tx.execute(
        "INSERT OR REPLACE INTO sync_state (filename, last_synced_size, last_synced_time)
         VALUES (?, 1, 0)",
        params![CURSOR_MODEL_ATTRIBUTION_MIGRATION_KEY],
    )
    .map_err(|error| format!("記錄 Cursor 模型歸因遷移失敗: {error}"))?;
    tx.commit()
        .map_err(|error| format!("提交 Cursor 模型歸因遷移失敗: {error}"))
}

fn run_cursor_cache_tokens_unknown_migration(conn: &mut Connection) -> Result<(), String> {
    let already_applied: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_state WHERE filename = ?)",
            params![CURSOR_CACHE_TOKENS_UNKNOWN_MIGRATION_KEY],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if already_applied {
        return Ok(());
    }

    let tx = conn
        .transaction()
        .map_err(|error| format!("啟動 Cursor 快取 Token 遷移失敗: {error}"))?;
    tx.execute(
        "UPDATE usage_entries
         SET tokens_cache_read = NULL,
             tokens_cache_write = NULL,
             tokens_cache_write_5m = NULL,
             tokens_cache_write_1h = NULL,
             delta_cache_read = NULL,
             delta_cache_write = NULL,
             delta_cache_write_5m = NULL,
             delta_cache_write_1h = NULL
         WHERE assistant_type = 'cursor'
           AND COALESCE(tokens_cache_read, 0) = 0
           AND COALESCE(tokens_cache_write, 0) = 0
           AND COALESCE(tokens_cache_write_5m, 0) = 0
           AND COALESCE(tokens_cache_write_1h, 0) = 0
           AND COALESCE(delta_cache_read, 0) = 0
           AND COALESCE(delta_cache_write, 0) = 0
           AND COALESCE(delta_cache_write_5m, 0) = 0
           AND COALESCE(delta_cache_write_1h, 0) = 0",
        [],
    )
    .map_err(|error| format!("將 Cursor 快取 Token 標記為未知失敗: {error}"))?;
    tx.execute(
        "INSERT OR REPLACE INTO sync_state (filename, last_synced_size, last_synced_time)
         VALUES (?, 1, 0)",
        params![CURSOR_CACHE_TOKENS_UNKNOWN_MIGRATION_KEY],
    )
    .map_err(|error| format!("記錄 Cursor 快取 Token 遷移失敗: {error}"))?;
    tx.commit()
        .map_err(|error| format!("提交 Cursor 快取 Token 遷移失敗: {error}"))
}

fn open_cursor_state_db(state_db_path: &Path) -> Result<Connection, String> {
    let conn = Connection::open_with_flags(
        state_db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("無法唯讀開啟 Cursor state.vscdb: {error}"))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|error| format!("設定 Cursor state.vscdb busy timeout 失敗: {error}"))?;
    let has_cursor_disk_kv: bool = conn
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM sqlite_master
                WHERE type = 'table' AND name = 'cursorDiskKV'
            )",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("檢查 Cursor cursorDiskKV 表失敗: {error}"))?;
    if !has_cursor_disk_kv {
        return Err("Cursor state.vscdb 缺少 cursorDiskKV 表".to_string());
    }
    Ok(conn)
}

fn cursor_state_max_rowid(conn: &Connection) -> Result<i64, String> {
    conn.query_row(
        "SELECT COALESCE(MAX(rowid), 0) FROM cursorDiskKV",
        [],
        |row| row.get(0),
    )
    .map_err(|error| format!("讀取 Cursor cursorDiskKV 最大 rowid 失敗: {error}"))
}

fn sync_cursor_model_signatures(
    conn: &mut Connection,
    state_db_path: &Path,
) -> Result<String, String> {
    let source_id = cursor_model_source_id(state_db_path);
    let state_key = format!("cursor-agent-kv:v2:{source_id}");
    let source_conn = open_cursor_state_db(state_db_path)?;
    let max_rowid = cursor_state_max_rowid(&source_conn)?;
    let stored_rowid: i64 = conn
        .query_row(
            "SELECT last_synced_size FROM sync_state WHERE filename = ?",
            params![state_key],
            |row| row.get(0),
        )
        .unwrap_or(0);
    let reset_cache = max_rowid < stored_rowid;
    let start_rowid = if reset_cache { 0 } else { stored_rowid };

    let mut mappings = Vec::new();
    if max_rowid > start_rowid {
        let mut statement = source_conn
            .prepare(
                "SELECT CAST(value AS BLOB)
                 FROM cursorDiskKV INDEXED BY sqlite_autoindex_cursorDiskKV_1
                 WHERE rowid > ? AND rowid <= ?
                   AND key >= 'agentKv:blob:' AND key < 'agentKv:blob;'
                   AND instr(CAST(value AS TEXT), '\"modelName\"') > 0",
            )
            .map_err(|error| format!("準備 Cursor agentKv 查詢失敗: {error}"))?;
        let mut rows = statement
            .query(params![start_rowid, max_rowid])
            .map_err(|error| format!("查詢 Cursor agentKv 失敗: {error}"))?;
        while let Some(row) = rows
            .next()
            .map_err(|error| format!("讀取 Cursor agentKv 記錄失敗: {error}"))?
        {
            let raw: Vec<u8> = match row.get(0) {
                Ok(raw) => raw,
                Err(_) => continue,
            };
            if let Some(mapping) = parse_cursor_agent_kv_model_signature(&raw) {
                mappings.push(mapping);
            }
        }
    }
    drop(source_conn);

    if reset_cache || max_rowid > start_rowid {
        let has_mapping_changes = !mappings.is_empty();
        let tx = conn
            .transaction()
            .map_err(|error| format!("啟動 Cursor 模型簽章同步失敗: {error}"))?;
        if reset_cache {
            tx.execute(
                "DELETE FROM cursor_model_signatures WHERE source_id = ?",
                params![source_id],
            )
            .map_err(|error| format!("重設 Cursor 模型簽章快取失敗: {error}"))?;
            tx.execute(
                "UPDATE usage_entries
                 SET model = 'Unknown Model', model_id = 'Unknown Model'
                 WHERE assistant_type = 'cursor'
                   AND model_signature IS NOT NULL",
                [],
            )
            .map_err(|error| format!("清除過期 Cursor 模型歸因失敗: {error}"))?;
            tx.execute("DELETE FROM sync_state WHERE filename LIKE 'cursor:%'", [])
                .map_err(|error| format!("重設 Cursor 逐字稿同步狀態失敗: {error}"))?;
        }
        for (signature, model) in mappings {
            tx.execute(
                "INSERT INTO cursor_model_signatures (
                    source_id, signature, model, is_ambiguous
                 ) VALUES (?, ?, ?, 0)
                 ON CONFLICT(source_id, signature) DO UPDATE SET
                    is_ambiguous = CASE
                        WHEN cursor_model_signatures.model = excluded.model
                        THEN cursor_model_signatures.is_ambiguous
                        ELSE 1
                    END",
                params![source_id, signature, model],
            )
            .map_err(|error| format!("寫入 Cursor 模型簽章快取失敗: {error}"))?;
        }
        if reset_cache || has_mapping_changes {
            tx.execute(
                "UPDATE usage_entries
                 SET model = 'Unknown Model', model_id = 'Unknown Model'
                 WHERE assistant_type = 'cursor'
                   AND model_signature IS NOT NULL
                   AND EXISTS (
                        SELECT 1 FROM cursor_model_signatures signatures
                        WHERE signatures.source_id = ?
                          AND signatures.signature = usage_entries.model_signature
                          AND signatures.is_ambiguous = 1
                   )",
                params![source_id],
            )
            .map_err(|error| format!("清除歧義 Cursor 模型歸因失敗: {error}"))?;
            tx.execute(
                "UPDATE usage_entries
                 SET model = (
                        SELECT signatures.model FROM cursor_model_signatures signatures
                        WHERE signatures.source_id = ?
                          AND signatures.signature = usage_entries.model_signature
                          AND signatures.is_ambiguous = 0
                     ),
                     model_id = (
                        SELECT signatures.model FROM cursor_model_signatures signatures
                        WHERE signatures.source_id = ?
                          AND signatures.signature = usage_entries.model_signature
                          AND signatures.is_ambiguous = 0
                     )
                 WHERE assistant_type = 'cursor'
                   AND model_signature IS NOT NULL
                   AND EXISTS (
                        SELECT 1 FROM cursor_model_signatures signatures
                        WHERE signatures.source_id = ?
                          AND signatures.signature = usage_entries.model_signature
                          AND signatures.is_ambiguous = 0
                   )",
                params![source_id, source_id, source_id],
            )
            .map_err(|error| format!("回填 Cursor 模型歸因失敗: {error}"))?;
        }

        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        tx.execute(
            "INSERT OR REPLACE INTO sync_state (
                filename, last_synced_size, last_synced_time
             ) VALUES (?, ?, ?)",
            params![state_key, max_rowid, now],
        )
        .map_err(|error| format!("更新 Cursor agentKv 同步狀態失敗: {error}"))?;
        tx.commit()
            .map_err(|error| format!("提交 Cursor 模型簽章同步失敗: {error}"))?;
    }

    Ok(source_id)
}

fn load_cursor_model_signatures(
    conn: &Connection,
    source_id: &str,
) -> Result<HashMap<String, String>, String> {
    let mut statement = conn
        .prepare(
            "SELECT signature, model
             FROM cursor_model_signatures
             WHERE source_id = ? AND is_ambiguous = 0",
        )
        .map_err(|error| format!("準備讀取 Cursor 模型簽章快取失敗: {error}"))?;
    let rows = statement
        .query_map(params![source_id], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|error| format!("讀取 Cursor 模型簽章快取失敗: {error}"))?;
    let mut mappings = HashMap::new();
    for row in rows {
        let (signature, model) =
            row.map_err(|error| format!("解析 Cursor 模型簽章快取失敗: {error}"))?;
        mappings.insert(signature, model);
    }
    Ok(mappings)
}

fn load_cursor_ambiguous_model_signatures(
    conn: &Connection,
    source_id: &str,
) -> Result<HashSet<String>, String> {
    let mut statement = conn
        .prepare(
            "SELECT signature
             FROM cursor_model_signatures
             WHERE source_id = ? AND is_ambiguous = 1",
        )
        .map_err(|error| format!("準備讀取 Cursor 歧義模型簽章失敗: {error}"))?;
    let rows = statement
        .query_map(params![source_id], |row| row.get(0))
        .map_err(|error| format!("讀取 Cursor 歧義模型簽章失敗: {error}"))?;
    let mut signatures = HashSet::new();
    for row in rows {
        signatures.insert(row.map_err(|error| format!("解析 Cursor 歧義模型簽章失敗: {error}"))?);
    }
    Ok(signatures)
}

#[derive(Clone, Debug, Default)]
struct CursorSessionMetadata {
    cwd: Option<String>,
    mode: Option<String>,
    model: Option<String>,
}

fn parse_cursor_session_metadata(key: &str, raw: &[u8]) -> Option<(String, CursorSessionMetadata)> {
    let value: serde_json::Value = serde_json::from_slice(raw).ok()?;
    let session_id = value
        .get("composerId")
        .and_then(|item| item.as_str())
        .or_else(|| key.strip_prefix("composerData:"))
        .map(str::trim)
        .filter(|item| !item.is_empty() && item.len() <= 200)?
        .to_string();
    let cwd = value
        .pointer("/workspaceIdentifier/uri/fsPath")
        .or_else(|| value.pointer("/workspaceIdentifier/fsPath"))
        .or_else(|| value.pointer("/workspaceIdentifier/uri/path"))
        .and_then(|item| item.as_str())
        .map(str::trim)
        .filter(|item| !item.is_empty() && item.len() <= 4096)
        .map(str::to_string);
    let unified_mode = value
        .get("unifiedMode")
        .and_then(|item| item.as_str())
        .map(str::trim)
        .filter(|item| !item.is_empty());
    let is_agentic = value.get("isAgentic").and_then(|item| item.as_bool());
    let mode = if is_agentic == Some(false)
        || unified_mode.is_some_and(|item| !item.eq_ignore_ascii_case("agent"))
    {
        Some("ide".to_string())
    } else if is_agentic == Some(true)
        || unified_mode.is_some_and(|item| item.eq_ignore_ascii_case("agent"))
    {
        Some("agent".to_string())
    } else {
        None
    };
    let model = value
        .pointer("/modelConfig/modelName")
        .and_then(|item| item.as_str())
        .map(str::trim)
        .filter(|item| {
            !item.is_empty()
                && item.len() <= 200
                && !item.eq_ignore_ascii_case("default")
                && !item.eq_ignore_ascii_case("auto")
                && !item.eq_ignore_ascii_case("unknown model")
        })
        .map(str::to_string);

    if cwd.is_none() && mode.is_none() && model.is_none() {
        return None;
    }
    Some((session_id, CursorSessionMetadata { cwd, mode, model }))
}

fn sync_cursor_session_metadata(
    conn: &mut Connection,
    state_db_path: &Path,
) -> Result<String, String> {
    let source_id = cursor_model_source_id(state_db_path);
    let state_key = format!("cursor-composer-data:v3:{source_id}");
    let source_conn = open_cursor_state_db(state_db_path)?;
    let max_rowid = cursor_state_max_rowid(&source_conn)?;
    let stored_rowid: i64 = conn
        .query_row(
            "SELECT last_synced_size FROM sync_state WHERE filename = ?",
            params![state_key],
            |row| row.get(0),
        )
        .unwrap_or(0);
    let reset_cache = max_rowid < stored_rowid;
    let start_rowid = if reset_cache { 0 } else { stored_rowid };

    let mut metadata_rows = Vec::new();
    if max_rowid > start_rowid {
        let mut statement = source_conn
            .prepare(
                "SELECT key, CAST(value AS BLOB)
                 FROM cursorDiskKV INDEXED BY sqlite_autoindex_cursorDiskKV_1
                 WHERE rowid > ? AND rowid <= ?
                   AND key >= 'composerData:' AND key < 'composerData;'",
            )
            .map_err(|error| format!("準備 Cursor composerData 查詢失敗: {error}"))?;
        let mut rows = statement
            .query(params![start_rowid, max_rowid])
            .map_err(|error| format!("查詢 Cursor composerData 失敗: {error}"))?;
        while let Some(row) = rows
            .next()
            .map_err(|error| format!("讀取 Cursor composerData 記錄失敗: {error}"))?
        {
            let key: String = match row.get(0) {
                Ok(key) => key,
                Err(_) => continue,
            };
            let raw: Vec<u8> = match row.get(1) {
                Ok(raw) => raw,
                Err(_) => continue,
            };
            if let Some(metadata) = parse_cursor_session_metadata(&key, &raw) {
                metadata_rows.push(metadata);
            }
        }
    }
    drop(source_conn);

    if reset_cache || max_rowid > start_rowid {
        let has_metadata_changes = !metadata_rows.is_empty();
        let tx = conn
            .transaction()
            .map_err(|error| format!("啟動 Cursor Session 中繼資料同步失敗: {error}"))?;
        if reset_cache {
            tx.execute(
                "DELETE FROM cursor_session_metadata WHERE source_id = ?",
                params![source_id],
            )
            .map_err(|error| format!("重設 Cursor Session 中繼資料快取失敗: {error}"))?;
            tx.execute("DELETE FROM sync_state WHERE filename LIKE 'cursor:%'", [])
                .map_err(|error| format!("重設 Cursor 逐字稿同步狀態失敗: {error}"))?;
        }
        for (session_id, metadata) in metadata_rows {
            tx.execute(
                "INSERT INTO cursor_session_metadata (
                    source_id, session_id, cwd, mode, model
                 ) VALUES (?, ?, ?, ?, ?)
                 ON CONFLICT(source_id, session_id) DO UPDATE SET
                    cwd = COALESCE(excluded.cwd, cursor_session_metadata.cwd),
                    mode = COALESCE(excluded.mode, cursor_session_metadata.mode),
                    model = COALESCE(excluded.model, cursor_session_metadata.model)",
                params![
                    source_id,
                    session_id,
                    metadata.cwd,
                    metadata.mode,
                    metadata.model
                ],
            )
            .map_err(|error| format!("寫入 Cursor Session 中繼資料快取失敗: {error}"))?;
        }
        if reset_cache || has_metadata_changes {
            tx.execute(
                "UPDATE usage_entries
                 SET cwd = (
                        SELECT metadata.cwd FROM cursor_session_metadata metadata
                        WHERE metadata.source_id = ?
                          AND metadata.session_id = usage_entries.session_id
                     )
                 WHERE assistant_type = 'cursor'
                   AND EXISTS (
                        SELECT 1 FROM cursor_session_metadata metadata
                        WHERE metadata.source_id = ?
                          AND metadata.session_id = usage_entries.session_id
                          AND metadata.cwd IS NOT NULL
                          AND metadata.cwd != ''
                   )",
                params![source_id, source_id],
            )
            .map_err(|error| format!("回填 Cursor 工作路徑失敗: {error}"))?;
            tx.execute(
                "DELETE FROM usage_entries
                 WHERE rowid IN (
                    SELECT legacy.rowid
                    FROM usage_entries legacy
                    JOIN cursor_session_metadata metadata
                      ON metadata.source_id = ?
                     AND metadata.session_id = legacy.session_id
                    JOIN usage_entries classified
                      ON classified.assistant_type = legacy.assistant_type
                     AND classified.session_id = legacy.session_id
                     AND classified.turn_no = legacy.turn_no
                     AND classified.source_kind = CASE metadata.mode
                        WHEN 'agent' THEN ?
                        WHEN 'ide' THEN ?
                     END
                    WHERE legacy.assistant_type = 'cursor'
                      AND legacy.source_kind = 'legacy'
                 )",
                params![source_id, CURSOR_AGENT_SOURCE_KIND, CURSOR_IDE_SOURCE_KIND],
            )
            .map_err(|error| format!("清除 Cursor legacy 重複記錄失敗: {error}"))?;
            tx.execute(
                "UPDATE usage_entries
                 SET source_kind = CASE (
                        SELECT metadata.mode FROM cursor_session_metadata metadata
                        WHERE metadata.source_id = ?
                          AND metadata.session_id = usage_entries.session_id
                     )
                        WHEN 'agent' THEN ?
                        WHEN 'ide' THEN ?
                        ELSE source_kind
                     END
                 WHERE assistant_type = 'cursor'
                   AND EXISTS (
                        SELECT 1 FROM cursor_session_metadata metadata
                        WHERE metadata.source_id = ?
                          AND metadata.session_id = usage_entries.session_id
                          AND metadata.mode IN ('agent', 'ide')
                   )",
                params![
                    source_id,
                    CURSOR_AGENT_SOURCE_KIND,
                    CURSOR_IDE_SOURCE_KIND,
                    source_id
                ],
            )
            .map_err(|error| format!("回填 Cursor Session 模式失敗: {error}"))?;
            tx.execute(
                "UPDATE usage_entries
                 SET model = (
                        SELECT metadata.model FROM cursor_session_metadata metadata
                        WHERE metadata.source_id = ?
                          AND metadata.session_id = usage_entries.session_id
                     ),
                     model_id = (
                        SELECT metadata.model FROM cursor_session_metadata metadata
                        WHERE metadata.source_id = ?
                          AND metadata.session_id = usage_entries.session_id
                     )
                 WHERE assistant_type = 'cursor'
                   AND EXISTS (
                        SELECT 1 FROM cursor_session_metadata metadata
                        WHERE metadata.source_id = ?
                          AND metadata.session_id = usage_entries.session_id
                          AND metadata.model IS NOT NULL
                          AND metadata.model != ''
                   )
                   AND NOT EXISTS (
                        SELECT 1 FROM cursor_model_signatures signatures
                        WHERE signatures.source_id = ?
                          AND signatures.signature = usage_entries.model_signature
                   )",
                params![source_id, source_id, source_id, source_id],
            )
            .map_err(|error| format!("回填 Cursor Session 模型 fallback 失敗: {error}"))?;
        }

        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        tx.execute(
            "INSERT OR REPLACE INTO sync_state (
                filename, last_synced_size, last_synced_time
             ) VALUES (?, ?, ?)",
            params![state_key, max_rowid, now],
        )
        .map_err(|error| format!("更新 Cursor composerData 同步狀態失敗: {error}"))?;
        tx.commit()
            .map_err(|error| format!("提交 Cursor Session 中繼資料同步失敗: {error}"))?;
    }

    Ok(source_id)
}

fn load_cursor_session_metadata(
    conn: &Connection,
    source_id: &str,
) -> Result<HashMap<String, CursorSessionMetadata>, String> {
    let mut statement = conn
        .prepare(
            "SELECT session_id, cwd, mode, model
             FROM cursor_session_metadata
             WHERE source_id = ?",
        )
        .map_err(|error| format!("準備讀取 Cursor Session 中繼資料失敗: {error}"))?;
    let rows = statement
        .query_map(params![source_id], |row| {
            Ok((
                row.get(0)?,
                CursorSessionMetadata {
                    cwd: row.get(1)?,
                    mode: row.get(2)?,
                    model: row.get(3)?,
                },
            ))
        })
        .map_err(|error| format!("讀取 Cursor Session 中繼資料失敗: {error}"))?;
    let mut mappings = HashMap::new();
    for row in rows {
        let (session_id, metadata) =
            row.map_err(|error| format!("解析 Cursor Session 中繼資料失敗: {error}"))?;
        mappings.insert(session_id, metadata);
    }
    Ok(mappings)
}

struct CursorParsedEntry {
    entry: UsageEntry,
    model_signature: Option<String>,
}

fn parse_cursor_session_file(
    filepath: &Path,
    model_mappings: &HashMap<String, String>,
    ambiguous_model_signatures: &HashSet<String>,
    session_metadata: &HashMap<String, CursorSessionMetadata>,
) -> Result<Vec<CursorParsedEntry>, String> {
    let file = File::open(filepath).map_err(|e| format!("無法開啟檔案: {}", e))?;
    let reader = BufReader::new(file);
    let fallback_session_id = filepath
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("unknown-session")
        .to_string();
    let metadata = session_metadata.get(&fallback_session_id);
    let session_cwd = metadata.and_then(|value| value.cwd.clone());
    let source_kind = cursor_mode_source_kind(metadata.and_then(|value| value.mode.as_deref()));

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
                let parsed_timestamp = parse_cursor_timestamp(&extracted_ts);
                if cursor_date_from_timestamp(&parsed_timestamp).is_some() {
                    current_timestamp = parsed_timestamp;
                }
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
            let current_model_signature =
                cursor_response_signature(content_val.unwrap_or(&serde_json::Value::Null));
            let current_model = match current_model_signature.as_ref() {
                Some(signature) if ambiguous_model_signatures.contains(signature) => {
                    "Unknown Model".to_string()
                }
                Some(signature) => model_mappings
                    .get(signature)
                    .cloned()
                    .or_else(|| metadata.and_then(|value| value.model.clone()))
                    .unwrap_or_else(|| "Unknown Model".to_string()),
                None => metadata
                    .and_then(|value| value.model.clone())
                    .unwrap_or_else(|| "Unknown Model".to_string()),
            };

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
                // Cursor transcripts do not expose cache token counts.
                cache_read: None,
                cache_write: None,
                cache_write_5m: None,
                cache_write_1h: None,
                reasoning: None,
                total: total_tokens,
            };

            results.push(CursorParsedEntry {
                entry: UsageEntry {
                    timestamp: current_timestamp.clone(),
                    session_id: fallback_session_id.clone(),
                    session_name: session_name_selector
                        .selected_name()
                        .map(str::to_string)
                        .or_else(|| Some(fallback_session_id.clone())),
                    transcript_path: Some(filepath.to_string_lossy().into_owned()),
                    cwd: session_cwd.clone(),
                    version: None,
                    turn_no: (results.len() + 1) as u32,
                    model: Some(current_model.clone()),
                    model_id: Some(current_model.clone()),
                    tokens: Some(tokens.clone()),
                    delta_tokens: Some(tokens),
                    context: None,
                    cost: None,
                    source_kind: source_kind.clone(),
                    source_dir_key: None,
                    parent_session_id: None,
                    agent_nickname: None,
                    agent_role: None,
                    reasoning_effort: None,
                },
                model_signature: current_model_signature,
            });
        }
    }

    Ok(results)
}

fn sync_cursor_usage_logs(conn: &mut Connection, cursor_dir: &Path) -> Result<(), String> {
    run_cursor_model_attribution_migration(conn)?;
    run_cursor_cache_tokens_unknown_migration(conn)?;

    let state_db_path = get_cursor_state_db_path();
    let source_id = if state_db_path.exists() {
        let source_id = cursor_model_source_id(&state_db_path);
        if let Err(error) = sync_cursor_session_metadata(conn, &state_db_path) {
            eprintln!("同步 Cursor composerData Session 中繼資料失敗: {error}");
        }
        if let Err(error) = sync_cursor_model_signatures(conn, &state_db_path) {
            eprintln!("同步 Cursor agentKv 模型資訊失敗: {error}");
        }
        Some(source_id)
    } else {
        None
    };
    let model_mappings = if let Some(source_id) = source_id.as_deref() {
        load_cursor_model_signatures(conn, source_id)?
    } else {
        HashMap::new()
    };
    let ambiguous_model_signatures = if let Some(source_id) = source_id.as_deref() {
        load_cursor_ambiguous_model_signatures(conn, source_id)?
    } else {
        HashSet::new()
    };
    let session_metadata = if let Some(source_id) = source_id.as_deref() {
        load_cursor_session_metadata(conn, source_id)?
    } else {
        HashMap::new()
    };

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
            let parsed_entries = match parse_cursor_session_file(
                &filepath,
                &model_mappings,
                &ambiguous_model_signatures,
                &session_metadata,
            ) {
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
                .map(|parsed| parsed.entry.session_id.clone())
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
            for parsed in &parsed_entries {
                let entry = &parsed.entry;
                let tokens = entry.tokens.as_ref();
                let delta = entry.delta_tokens.as_ref();
                let cost = entry.cost.as_ref();
                let entry_date = cursor_date_from_timestamp(&entry.timestamp).ok_or_else(|| {
                    format!(
                        "Cursor Session {} 的時間戳記無有效日期: {}",
                        entry.session_id, entry.timestamp
                    )
                })?;

                let insert_res = tx.execute(
                    "INSERT INTO usage_entries (
                        assistant_type, timestamp, date, session_id, session_name, transcript_path, cwd, version, turn_no, model, model_id, model_signature,
                        tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, tokens_cache_write_5m, tokens_cache_write_1h, tokens_reasoning, tokens_total,
                        delta_input, delta_output, delta_cache_read, delta_cache_write, delta_cache_write_5m, delta_cache_write_1h, delta_reasoning, delta_total,
                        duration_ms, premium_requests, parent_session_id, agent_nickname, agent_role, reasoning_effort, source_kind
                    ) VALUES (
                        ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                        ?, ?, ?, ?, ?, ?, ?, ?,
                        ?, ?, ?, ?, ?, ?, ?, ?,
                        ?, ?, ?, ?, ?, ?, ?
                    )",
                    params![
                        "cursor",
                        entry.timestamp,
                        entry_date,
                        entry.session_id,
                        entry.session_name.as_deref(),
                        entry.transcript_path.as_deref(),
                        entry.cwd.as_deref(),
                        entry.version.as_deref(),
                        entry.turn_no as i64,
                        entry.model.as_deref(),
                        entry.model_id.as_deref(),
                        parsed.model_signature.as_deref(),
                        tokens.map(|t| t.input as i64),
                        tokens.map(|t| t.output as i64),
                        tokens.and_then(|t| t.cache_read.map(|v| v as i64)),
                        tokens.and_then(|t| t.cache_write.map(|v| v as i64)),
                        tokens.and_then(|t| t.cache_write_5m.map(|v| v as i64)),
                        tokens.and_then(|t| t.cache_write_1h.map(|v| v as i64)),
                        tokens.and_then(|t| t.reasoning.map(|v| v as i64)),
                        tokens.map(|t| t.total as i64),
                        delta.map(|t| t.input as i64),
                        delta.map(|t| t.output as i64),
                        delta.and_then(|t| t.cache_read.map(|v| v as i64)),
                        delta.and_then(|t| t.cache_write.map(|v| v as i64)),
                        delta.and_then(|t| t.cache_write_5m.map(|v| v as i64)),
                        delta.and_then(|t| t.cache_write_1h.map(|v| v as i64)),
                        delta.and_then(|t| t.reasoning.map(|v| v as i64)),
                        delta.map(|t| t.total as i64),
                        cost.and_then(|c| c.total_api_duration_ms.map(|d| d as i64)),
                        cost.and_then(|c| c.total_premium_requests.map(|r| r as i64)),
                        entry.parent_session_id.as_deref(),
                        entry.agent_nickname.as_deref(),
                        entry.agent_role.as_deref(),
                        entry.reasoning_effort.as_deref(),
                        entry.source_kind.as_deref().unwrap_or("cursor")
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

pub(crate) fn sync_grok_usage_logs(conn: &mut Connection, grok_dir: &Path) -> Result<(), String> {
    for filepath in crate::grok::find_session_update_files(grok_dir) {
        let metadata = match fs::metadata(&filepath) {
            Ok(metadata) => metadata,
            Err(error) => {
                eprintln!(
                    "讀取 Grok Build session 檔案 {:?} 失敗: {}",
                    filepath, error
                );
                continue;
            }
        };
        let current_size = metadata.len();
        let state_name = portable_relative_path(grok_dir, &filepath);
        let state_key = format!("grok:{state_name}");
        let last_synced_size: u64 = conn
            .query_row(
                "SELECT last_synced_size FROM sync_state WHERE filename = ?",
                params![state_key],
                |row| row.get(0),
            )
            .unwrap_or(0);

        if current_size == last_synced_size {
            continue;
        }

        // Grok appends JSONL records. Wait for the current record to be
        // complete before advancing sync_state, otherwise a partial final
        // line could be skipped permanently on the next incremental pass.
        if current_size > 0 {
            let mut file = match File::open(&filepath) {
                Ok(file) => file,
                Err(error) => {
                    eprintln!(
                        "開啟 Grok Build session 檔案 {:?} 失敗: {}",
                        filepath, error
                    );
                    continue;
                }
            };
            if file.seek(SeekFrom::End(-1)).is_err() {
                continue;
            }
            let mut last_byte = [0u8; 1];
            if file.read_exact(&mut last_byte).is_err() || last_byte[0] != b'\n' {
                continue;
            }
        }

        let parsed_entries = match crate::grok::parse_session_usage_file(&filepath) {
            Ok(entries) => entries,
            Err(error) => {
                eprintln!(
                    "解析 Grok Build session 檔案 {:?} 失敗: {}",
                    filepath, error
                );
                continue;
            }
        };

        let transcript_path = filepath.to_string_lossy().into_owned();
        let tx = conn
            .transaction()
            .map_err(|error| format!("Grok Build transaction BEGIN 失敗: {error}"))?;
        tx.execute(
            "DELETE FROM usage_entries
             WHERE assistant_type = 'grok' AND transcript_path = ?",
            params![transcript_path],
        )
        .map_err(|error| format!("清除舊 Grok Build session 資料失敗: {error}"))?;

        for entry in &parsed_entries {
            let tokens = entry.tokens.as_ref();
            let delta = entry.delta_tokens.as_ref();
            let cost = entry.cost.as_ref();
            let source_kind = entry
                .source_kind
                .as_deref()
                .unwrap_or(crate::grok::CONTEXT_SOURCE_KIND);
            let usage_identity = entry
                .model_id
                .as_deref()
                .filter(|model| !model.trim().is_empty())
                .map(|model| format!("model:{model}"))
                .unwrap_or_default();
            tx.execute(
                "INSERT INTO usage_entries (
                    assistant_type, source_kind, usage_identity, timestamp, date, session_id, session_name, transcript_path, cwd, version, turn_no, model, model_id,
                    tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, tokens_reasoning, tokens_total,
                    delta_input, delta_output, delta_cache_read, delta_cache_write, delta_reasoning, delta_total,
                    duration_ms, premium_requests, reported_cost_usd, reasoning_effort
                ) VALUES (
                    ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                    ?, ?, ?, ?, ?, ?,
                    ?, ?, ?, ?, ?, ?,
                    ?, ?, ?, ?
                )",
                params![
                    "grok",
                    source_kind,
                    usage_identity,
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
                    cost.and_then(|value| value.total_api_duration_ms.map(|v| v as i64)),
                    cost.and_then(|value| value.total_premium_requests.map(|v| v as i64)),
                    cost.and_then(|value| value.reported_cost_usd),
                    entry.reasoning_effort.as_deref(),
                ],
            )
            .map_err(|error| format!("寫入 Grok Build 資料庫失敗: {error}"))?;
        }

        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        tx.execute(
            "INSERT OR REPLACE INTO sync_state (filename, last_synced_size, last_synced_time)
             VALUES (?, ?, ?)",
            params![state_key, current_size as i64, now],
        )
        .map_err(|error| format!("更新 Grok Build sync state 失敗: {error}"))?;
        tx.commit()
            .map_err(|error| format!("提交 Grok Build transaction 失敗: {error}"))?;
    }

    Ok(())
}

/// Shared incremental sync routine for the Pi Coding Agent and its fork OMP,
/// both of which persist sessions as append-only JSONL files under
/// `<dir>/agent/sessions/` using the identical tree-structured format. Each
/// assistant message entry already carries a complete, self-contained
/// token/cost snapshot for its turn, so (unlike Grok) no cross-line delta
/// accumulation is required; `tokens` and `delta_tokens` are therefore equal.
fn sync_pi_family_usage_logs(
    conn: &mut Connection,
    assistant_type: &str,
    assistant_label: &str,
    session_files: Vec<PathBuf>,
    dir: &Path,
    parse_file: impl Fn(&Path) -> Result<Vec<UsageEntry>, String>,
) -> Result<(), String> {
    for filepath in session_files {
        let metadata = match fs::metadata(&filepath) {
            Ok(metadata) => metadata,
            Err(error) => {
                eprintln!(
                    "讀取 {assistant_label} session 檔案 {:?} 失敗: {}",
                    filepath, error
                );
                continue;
            }
        };
        let current_size = metadata.len();
        let state_name = portable_relative_path(dir, &filepath);
        let state_key = format!("{assistant_type}:{state_name}");
        let last_synced_size: u64 = conn
            .query_row(
                "SELECT last_synced_size FROM sync_state WHERE filename = ?",
                params![state_key],
                |row| row.get(0),
            )
            .unwrap_or(0);

        if current_size == last_synced_size {
            continue;
        }

        // Sessions append JSONL records. Wait for the current record to be
        // complete before advancing sync_state, otherwise a partial final
        // line could be skipped permanently on the next incremental pass.
        if current_size > 0 {
            let mut file = match File::open(&filepath) {
                Ok(file) => file,
                Err(error) => {
                    eprintln!(
                        "開啟 {assistant_label} session 檔案 {:?} 失敗: {}",
                        filepath, error
                    );
                    continue;
                }
            };
            if file.seek(SeekFrom::End(-1)).is_err() {
                continue;
            }
            let mut last_byte = [0u8; 1];
            if file.read_exact(&mut last_byte).is_err() || last_byte[0] != b'\n' {
                continue;
            }
        }

        let parsed_entries = match parse_file(&filepath) {
            Ok(entries) => entries,
            Err(error) => {
                eprintln!(
                    "解析 {assistant_label} session 檔案 {:?} 失敗: {}",
                    filepath, error
                );
                continue;
            }
        };

        let transcript_path = filepath.to_string_lossy().into_owned();
        let tx = conn
            .transaction()
            .map_err(|error| format!("{assistant_label} transaction BEGIN 失敗: {error}"))?;
        tx.execute(
            "DELETE FROM usage_entries
             WHERE assistant_type = ?1 AND transcript_path = ?2",
            params![assistant_type, transcript_path],
        )
        .map_err(|error| format!("清除舊 {assistant_label} session 資料失敗: {error}"))?;

        for entry in &parsed_entries {
            let tokens = entry.tokens.as_ref();
            let delta = entry.delta_tokens.as_ref();
            let cost = entry.cost.as_ref();
            let source_kind = entry.source_kind.as_deref().unwrap_or(assistant_type);
            let usage_identity = entry
                .model_id
                .as_deref()
                .filter(|model| !model.trim().is_empty())
                .map(|model| format!("model:{model}"))
                .unwrap_or_default();
            tx.execute(
                "INSERT INTO usage_entries (
                    assistant_type, source_kind, usage_identity, timestamp, date, session_id, session_name, transcript_path, cwd, version, turn_no, model, model_id,
                    tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, tokens_reasoning, tokens_total,
                    delta_input, delta_output, delta_cache_read, delta_cache_write, delta_reasoning, delta_total,
                    duration_ms, premium_requests, reported_cost_usd, reasoning_effort
                ) VALUES (
                    ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                    ?, ?, ?, ?, ?, ?,
                    ?, ?, ?, ?, ?, ?,
                    ?, ?, ?, ?
                )",
                params![
                    assistant_type,
                    source_kind,
                    usage_identity,
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
                    cost.and_then(|value| value.total_api_duration_ms.map(|v| v as i64)),
                    cost.and_then(|value| value.total_premium_requests.map(|v| v as i64)),
                    cost.and_then(|value| value.reported_cost_usd),
                    entry.reasoning_effort.as_deref(),
                ],
            )
            .map_err(|error| format!("寫入 {assistant_label} 資料庫失敗: {error}"))?;
        }

        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        tx.execute(
            "INSERT OR REPLACE INTO sync_state (filename, last_synced_size, last_synced_time)
             VALUES (?, ?, ?)",
            params![state_key, current_size as i64, now],
        )
        .map_err(|error| format!("更新 {assistant_label} sync state 失敗: {error}"))?;
        tx.commit()
            .map_err(|error| format!("提交 {assistant_label} transaction 失敗: {error}"))?;
    }

    Ok(())
}

pub(crate) fn sync_pi_usage_logs(conn: &mut Connection, pi_dir: &Path) -> Result<(), String> {
    let session_files = crate::pi::find_session_files(pi_dir);
    sync_pi_family_usage_logs(
        conn,
        "pi",
        "Pi Coding Agent",
        session_files,
        pi_dir,
        |path| crate::pi::parse_session_usage_file(path, crate::pi::SOURCE_KIND),
    )
}

pub(crate) fn sync_omp_usage_logs(conn: &mut Connection, omp_dir: &Path) -> Result<(), String> {
    let session_files = crate::omp::find_session_files(omp_dir);
    sync_pi_family_usage_logs(
        conn,
        "omp",
        "OMP",
        session_files,
        omp_dir,
        crate::omp::parse_session_usage_file,
    )
}

pub(crate) fn sync_muse_usage_logs(conn: &mut Connection, muse_dir: &Path) -> Result<(), String> {
    let session_files = crate::muse::find_session_files(muse_dir);
    sync_pi_family_usage_logs(
        conn,
        "muse",
        "Muse",
        session_files,
        muse_dir,
        crate::muse::parse_session_usage_file,
    )
}

/// Unified sync function triggering sync for all supported assistants
pub fn sync_usage_logs(conn: &mut Connection) -> Result<(), String> {
    // 1. Sync Cursor metadata first so model and mode attribution is available
    // before the potentially slower transcript collectors finish.
    let cursor_dir = get_cursor_dir();
    if let Err(e) = sync_cursor_usage_logs(conn, &cursor_dir) {
        eprintln!("❌ 同步 Cursor 失敗: {}", e);
    }

    // 2. Sync Google Antigravity CLI
    let antigravity_dir = get_antigravity_dir();
    if let Err(e) = sync_hook_usage_logs(conn, "antigravity", &antigravity_dir) {
        eprintln!("❌ 同步 Antigravity 失敗: {}", e);
    }

    // 3. Sync GitHub Copilot CLI
    let copilot_dir = get_copilot_dir();
    if let Err(e) = sync_hook_usage_logs(conn, "copilot", &copilot_dir) {
        eprintln!("❌ 同步 Copilot 失敗: {}", e);
    }

    // 3b. Sync GitHub Copilot sessions created in VS Code
    if let Err(e) = sync_vscode_chat_sessions(conn) {
        eprintln!("❌ 同步 VS Code Copilot 失敗: {}", e);
    }

    // 4. Sync GitHub Copilot App (Tauri desktop) usage
    if let Err(e) = sync_copilot_app_usage_logs(conn) {
        eprintln!("❌ 同步 Copilot App 失敗: {}", e);
    }

    // 5. Reconcile Copilot CLI subagent usage against session-store.db. Runs
    // after the hook and App collectors so CLI sessions are classified against
    // the authoritative App registry and the hook merged rows are available
    // for total validation. Falls back to hook rows when session-store is
    // missing, unclassifiable, or fails total validation.
    if let Err(e) = sync_copilot_cli_agent_usage_logs(conn) {
        eprintln!("❌ 同步 Copilot CLI agent reconciliation 失敗: {}", e);
    }

    // 5b. Backfill CWD for Copilot rows written before CWD was resolved from
    // session-store.db.sessions.
    if let Err(e) = backfill_copilot_cwd(conn) {
        eprintln!("❌ 補填 Copilot CWD 失敗: {}", e);
    }

    // 6. Sync Codex CLI and Desktop
    if let Err(e) = sync_codex_usage_logs(conn) {
        eprintln!("❌ 同步 Codex 失敗: {}", e);
    }

    // 5. Sync Claude Code
    if let Err(e) = sync_claude_usage_logs(conn) {
        eprintln!("❌ 同步 Claude Code 失敗: {}", e);
    }

    // 8. Sync Grok Build sessions
    let grok_dir = get_grok_dir();
    if let Err(e) = sync_grok_usage_logs(conn, &grok_dir) {
        eprintln!("❌ 同步 Grok Build 失敗: {}", e);
    }

    // 9. Sync Pi Coding Agent sessions
    let pi_dir = get_pi_dir();
    if let Err(e) = sync_pi_usage_logs(conn, &pi_dir) {
        eprintln!("❌ 同步 Pi Coding Agent 失敗: {}", e);
    }

    // 10. Sync OMP sessions (Pi fork)
    let omp_dir = get_omp_dir();
    if let Err(e) = sync_omp_usage_logs(conn, &omp_dir) {
        eprintln!("❌ 同步 OMP 失敗: {}", e);
    }

    // 11. Sync Muse sessions
    let muse_dir = get_muse_dir();
    if let Err(e) = sync_muse_usage_logs(conn, &muse_dir) {
        eprintln!("❌ 同步 Muse 失敗: {}", e);
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
            tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, tokens_cache_write_5m, tokens_cache_write_1h, tokens_reasoning, tokens_total,
            delta_input, delta_output, delta_cache_read, delta_cache_write, delta_cache_write_5m, delta_cache_write_1h, delta_reasoning, delta_total,
            duration_ms, premium_requests, parent_session_id, agent_nickname, agent_role, assistant_type, reasoning_effort, import_source_id, source_kind, source_dir_key, reported_cost_usd
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
        let ast_type = row.get::<_, String>(30).map_err(|e| e.to_string())?;
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
        let tokens_cache_write_5m: Option<u64> = row
            .get::<_, Option<i64>>(13)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let tokens_cache_write_1h: Option<u64> = row
            .get::<_, Option<i64>>(14)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let tokens_reasoning: Option<u64> = row
            .get::<_, Option<i64>>(15)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let tokens_total: Option<u64> = row
            .get::<_, Option<i64>>(16)
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
                cache_write_5m: tokens_cache_write_5m,
                cache_write_1h: tokens_cache_write_1h,
                reasoning: tokens_reasoning,
                total,
            })
        } else {
            None
        };

        let delta_input: Option<u64> = row
            .get::<_, Option<i64>>(17)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let delta_output: Option<u64> = row
            .get::<_, Option<i64>>(18)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let delta_cache_read: Option<u64> = row
            .get::<_, Option<i64>>(19)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let delta_cache_write: Option<u64> = row
            .get::<_, Option<i64>>(20)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let delta_cache_write_5m: Option<u64> = row
            .get::<_, Option<i64>>(21)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let delta_cache_write_1h: Option<u64> = row
            .get::<_, Option<i64>>(22)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let delta_reasoning: Option<u64> = row
            .get::<_, Option<i64>>(23)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let delta_total: Option<u64> = row
            .get::<_, Option<i64>>(24)
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
                cache_write_5m: delta_cache_write_5m,
                cache_write_1h: delta_cache_write_1h,
                reasoning: delta_reasoning,
                total,
            })
        } else {
            None
        };

        let duration_ms: Option<f64> = row
            .get::<_, Option<i64>>(25)
            .map_err(|e| e.to_string())?
            .map(|v| v as f64);
        let premium_requests: Option<f64> = row
            .get::<_, Option<i64>>(26)
            .map_err(|e| e.to_string())?
            .map(|v| v as f64);

        let reported_cost_usd: Option<f64> =
            row.get::<_, Option<f64>>(35).map_err(|e| e.to_string())?;
        let cost =
            if duration_ms.is_some() || premium_requests.is_some() || reported_cost_usd.is_some() {
                Some(CostStats {
                    total_api_duration_ms: duration_ms,
                    total_duration_ms: None,
                    total_premium_requests: premium_requests,
                    reported_cost_usd,
                })
            } else {
                None
            };
        let import_source_id = normalize_import_source_id(
            row.get::<_, Option<String>>(32)
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
                source_kind: row.get(33).ok(),
                source_dir_key: row.get(34).ok(),
                parent_session_id: row.get(27).ok(),
                agent_nickname: row.get(28).ok(),
                agent_role: row.get(29).ok(),
                reasoning_effort: row.get(31).ok(),
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

pub fn export_usage_period_entries(
    conn: &rusqlite::Connection,
    assistant: &str,
    period: &str,
) -> Result<Vec<UsageDayExportRecord>, String> {
    if period.len() == 10 {
        return export_usage_day_entries(conn, assistant, period);
    }

    let mut dates = get_available_dates(conn, assistant)?
        .into_iter()
        .filter(|date| date.starts_with(period))
        .collect::<Vec<_>>();
    dates.sort();

    let mut records = Vec::new();
    for date in dates {
        records.extend(export_usage_day_entries(conn, assistant, &date)?);
    }
    Ok(records)
}

pub fn import_usage_day_entries(
    conn: &mut Connection,
    assistant: &str,
    batch_date: &str,
    records: Vec<UsageDayExportRecord>,
    metadata: UsageImportMetadata,
) -> Result<UsageDayImportSummary, String> {
    let total = records.len();
    if total == 0 {
        return Err("匯入資料為空".to_string());
    }

    let batch_id = new_import_batch_id();
    let created_at = unix_timestamp_secs();
    let source_assistant = normalize_import_metadata_value(metadata.source_assistant, 64);
    let source_file_name = normalize_import_metadata_value(metadata.source_file_name, 255);
    let mut inserted = 0usize;
    let mut skipped_duplicates = 0usize;

    let tx = conn
        .transaction()
        .map_err(|e| format!("建立匯入交易失敗: {}", e))?;
    tx.execute(
        "INSERT INTO import_batches (
            id, assistant_type, source_assistant, source_file_name, import_date,
            total_records, imported_records, skipped_duplicates, created_at
         ) VALUES (?, ?, ?, ?, ?, ?, 0, 0, ?)",
        params![
            batch_id,
            assistant,
            source_assistant,
            source_file_name,
            batch_date,
            total as i64,
            created_at,
        ],
    )
    .map_err(|e| format!("建立匯入批次失敗: {e}"))?;

    for record in records {
        let mut entry = record.entry;
        let normalized_id = normalize_import_source_id(record.import_source_id.as_deref());
        let record_date = entry_date_from_timestamp(&entry.timestamp)
            .ok_or_else(|| "無效的 timestamp 格式，無法取得日期".to_string())?
            .to_string();
        let generated_source_id =
            build_usage_entry_import_source_id(assistant, &record_date, &entry);

        let source_kind = entry
            .source_kind
            .clone()
            .unwrap_or_else(|| "legacy".to_string());
        let usage_identity = if assistant == "grok" && source_kind == crate::grok::USAGE_SOURCE_KIND
        {
            entry
                .model_id
                .as_deref()
                .filter(|model| !model.trim().is_empty())
                .map(|model| format!("model:{model}"))
                .unwrap_or_default()
        } else {
            String::new()
        };
        if assistant == "copilot" && matches!(source_kind.as_str(), "copilot-cli" | "legacy") {
            normalize_copilot_cli_usage_entry(&mut entry);
        } else if assistant == "claude" {
            normalize_legacy_claude_usage_entry(&mut entry);
        }
        let source_id = normalized_id.unwrap_or(generated_source_id);

        let imported = tx
            .execute(
                "INSERT OR IGNORE INTO usage_entries (
                    assistant_type, source_kind, usage_identity, timestamp, date, session_id, session_name, transcript_path, cwd, version, turn_no,
                    model, model_id, tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, tokens_cache_write_5m, tokens_cache_write_1h, tokens_reasoning, tokens_total,
                    delta_input, delta_output, delta_cache_read, delta_cache_write, delta_cache_write_5m, delta_cache_write_1h, delta_reasoning, delta_total,
                    duration_ms, premium_requests, reported_cost_usd,
                    parent_session_id, agent_nickname, agent_role, reasoning_effort,
                    import_source_id, import_batch_id
                ) VALUES (
                    ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                    ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                    ?, ?, ?, ?, ?, ?, ?, ?,
                    ?, ?, ?, ?, ?, ?, ?, ?, ?
                )",
                rusqlite::params![
                    assistant,
                    source_kind,
                    usage_identity,
                    entry.timestamp,
                    record_date,
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
                    entry.tokens.as_ref().and_then(|t| t.cache_write_5m.map(|v| v as i64)),
                    entry.tokens.as_ref().and_then(|t| t.cache_write_1h.map(|v| v as i64)),
                    entry.tokens.as_ref().and_then(|t| t.reasoning.map(|v| v as i64)),
                    entry.tokens.as_ref().map(|t| t.total as i64),
                    entry.delta_tokens.as_ref().map(|t| t.input as i64),
                    entry.delta_tokens.as_ref().map(|t| t.output as i64),
                    entry.delta_tokens.as_ref().and_then(|t| t.cache_read.map(|v| v as i64)),
                    entry.delta_tokens.as_ref().and_then(|t| t.cache_write.map(|v| v as i64)),
                    entry.delta_tokens.as_ref().and_then(|t| t.cache_write_5m.map(|v| v as i64)),
                    entry.delta_tokens.as_ref().and_then(|t| t.cache_write_1h.map(|v| v as i64)),
                    entry.delta_tokens.as_ref().and_then(|t| t.reasoning.map(|v| v as i64)),
                    entry.delta_tokens.as_ref().map(|t| t.total as i64),
                    entry.cost.as_ref().and_then(|c| c.total_api_duration_ms).map(|v| v as i64),
                    entry.cost.as_ref().and_then(|c| c.total_premium_requests).map(|v| v as i64),
                    entry.cost.as_ref().and_then(|c| c.reported_cost_usd),
                    entry.parent_session_id,
                    entry.agent_nickname,
                    entry.agent_role,
                    entry.reasoning_effort,
                    source_id,
                    batch_id,
                ],
            )
            .map_err(|e| format!("匯入資料寫入失敗: {}", e))?;

        if imported > 0 {
            inserted += 1;
        } else {
            skipped_duplicates += 1;
        }
    }

    tx.execute(
        "UPDATE import_batches
         SET imported_records = ?, skipped_duplicates = ?
         WHERE id = ?",
        params![inserted as i64, skipped_duplicates as i64, batch_id],
    )
    .map_err(|e| format!("更新匯入批次結果失敗: {e}"))?;
    tx.commit()
        .map_err(|e| format!("提交匯入結果失敗: {}", e))?;

    Ok(UsageDayImportSummary {
        date: batch_date.to_string(),
        total,
        imported: inserted,
        skipped_duplicates,
        batch_id,
    })
}

pub fn list_usage_import_batches(
    conn: &Connection,
    assistant: &str,
    limit: usize,
) -> Result<Vec<UsageImportBatch>, String> {
    let limit = limit.clamp(1, 100) as i64;
    let mut stmt = conn
        .prepare(
            "SELECT id, assistant_type, source_assistant, source_file_name, import_date,
                    total_records, imported_records, skipped_duplicates, created_at,
                    rolled_back_at, removed_records
             FROM import_batches
             WHERE assistant_type = ?
             ORDER BY created_at DESC, id DESC
             LIMIT ?",
        )
        .map_err(|e| format!("準備匯入批次查詢失敗: {e}"))?;
    let rows = stmt
        .query_map(params![assistant, limit], |row| {
            Ok(UsageImportBatch {
                id: row.get(0)?,
                assistant: row.get(1)?,
                source_assistant: row.get(2)?,
                source_file_name: row.get(3)?,
                date: row.get(4)?,
                total: row.get::<_, i64>(5)? as usize,
                imported: row.get::<_, i64>(6)? as usize,
                skipped_duplicates: row.get::<_, i64>(7)? as usize,
                created_at: row.get(8)?,
                rolled_back_at: row.get(9)?,
                removed_records: row.get::<_, i64>(10)? as usize,
            })
        })
        .map_err(|e| format!("查詢匯入批次失敗: {e}"))?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("讀取匯入批次失敗: {e}"))
}

pub fn rollback_usage_import_batch(
    conn: &mut Connection,
    assistant: &str,
    batch_id: &str,
) -> Result<UsageImportRollbackSummary, String> {
    let tx = conn
        .transaction()
        .map_err(|e| format!("建立撤銷交易失敗: {e}"))?;
    let rolled_back_at = match tx.query_row(
        "SELECT rolled_back_at
         FROM import_batches
         WHERE id = ? AND assistant_type = ?",
        params![batch_id, assistant],
        |row| row.get::<_, Option<i64>>(0),
    ) {
        Ok(value) => value,
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            return Err("找不到指定的匯入批次".to_string());
        }
        Err(error) => return Err(format!("查詢匯入批次失敗: {error}")),
    };
    if rolled_back_at.is_some() {
        return Err("指定的匯入批次已撤銷".to_string());
    }

    let removed_records = tx
        .execute(
            "DELETE FROM usage_entries
             WHERE assistant_type = ? AND import_batch_id = ?",
            params![assistant, batch_id],
        )
        .map_err(|e| format!("刪除匯入資料失敗: {e}"))?;
    tx.execute(
        "UPDATE import_batches
         SET rolled_back_at = ?, removed_records = ?
         WHERE id = ? AND assistant_type = ?",
        params![
            unix_timestamp_secs(),
            removed_records as i64,
            batch_id,
            assistant
        ],
    )
    .map_err(|e| format!("更新匯入批次狀態失敗: {e}"))?;
    tx.commit().map_err(|e| format!("提交撤銷結果失敗: {e}"))?;

    Ok(UsageImportRollbackSummary {
        batch_id: batch_id.to_string(),
        removed_records,
    })
}

/// Session identity tuple returned by [`get_session_assistant_and_transcript`]:
/// `(assistant_type, transcript_path, source_kind, source_dir_key,
/// parent_session_id, agent_nickname)`.
pub type SessionIdentity = (
    String,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// Resolves the single `source_kind` used by the legacy `source_kind = None`
/// fallback. All four session-lookup helpers
/// ([`get_session_assistant_and_transcript`], [`get_session_cwd`],
/// [`get_session_model`], [`get_session_turns_token_stats`]) MUST call this
/// helper when invoked with `source_kind = None` so that the identity, CWD,
/// model, and turn-stats lookups always pick rows from the same source — even
/// when a single session id has rows from multiple collectors
/// (e.g. `copilot-cli` turn 1 + `vscode-chat` turn 2, both with
/// `source_dir_key IS NULL`).
///
/// Behaviour:
///
/// * `source_kind = Some(kind)` — returns `Some(kind.to_string())` directly,
///   without touching the database. Mirrors the explicit production-handler
///   call sites that always pass a concrete `source_kind`.
/// * `source_kind = None` — runs a single, parameter-bound query against
///   `usage_entries` matching `(assistant_type, session_id, source_dir_key
///   predicate)`, ordered by the canonical tie-break:
///   1. main agent rows first — `(parent_session_id IS NULL) DESC`,
///   2. `source_kind ASC`,
///   3. `turn_no ASC`,
///   4. `id ASC` (final stable tie-breaker).
///
/// Returns the `source_kind` of the first row, or `None` if the query produced
/// no rows. Never panics on empty result sets — it just returns `None`, so the
/// downstream helper can still surface a clean "session not found" / empty
/// result to the caller.
///
/// `source_dir_key` follows the same semantics as the four helpers: `Some(k)`
/// filters by `source_dir_key = k`, `None` filters by `source_dir_key IS NULL`.
/// `None` therefore means "non-App rows only" and never "any source".
fn resolve_session_source_kind(
    conn: &rusqlite::Connection,
    assistant: &str,
    session_id: &str,
    source_kind: Option<&str>,
    source_dir_key: Option<&str>,
) -> Result<Option<String>, String> {
    if let Some(kind) = source_kind {
        return Ok(Some(kind.to_string()));
    }

    let mut sql = String::from(
        "SELECT source_kind FROM usage_entries
         WHERE assistant_type = ? AND session_id = ?",
    );
    let mut params_vec: Vec<rusqlite::types::Value> = vec![
        rusqlite::types::Value::Text(assistant.to_string()),
        rusqlite::types::Value::Text(session_id.to_string()),
    ];
    if let Some(key) = source_dir_key {
        sql.push_str(" AND source_dir_key = ?");
        params_vec.push(rusqlite::types::Value::Text(key.to_string()));
    } else {
        sql.push_str(" AND source_dir_key IS NULL");
    }
    // Main agent rows first (parent_session_id IS NULL → 1; DESC puts them
    // ahead of subagent rows which evaluate to 0). Then `source_kind ASC`
    // for a deterministic tie-break between equally-scoped sources, then
    // `turn_no ASC`, then `id ASC` as the final stable tie-breaker.
    sql.push_str(
        " ORDER BY (parent_session_id IS NULL) DESC, source_kind ASC, turn_no ASC, id ASC LIMIT 1",
    );

    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let mut rows = stmt
        .query(rusqlite::params_from_iter(params_vec))
        .map_err(|e| e.to_string())?;
    if let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let resolved: Option<String> = row.get(0).ok().flatten();
        // The column is TEXT NOT NULL DEFAULT 'legacy' so this should always
        // be Some, but fall back to the column default if a NULL sneaks in
        // (e.g. via a future migration). This keeps the helper panic-free.
        Ok(resolved.or_else(|| Some("legacy".to_string())))
    } else {
        Ok(None)
    }
}

/// Returns `(assistant_type, transcript_path, source_kind, source_dir_key,
/// parent_session_id, agent_nickname)` for a given session id.
///
/// `source_kind` and `source_dir_key` narrow the query so that sessions with the
/// same `session_id` from different sources (e.g. Copilot CLI vs. Copilot App,
/// or two different Copilot App directories) are unambiguously identified.
/// When either is `None`, the corresponding predicate is still applied
/// explicitly (`source_dir_key IS NULL` when `source_dir_key` is `None`, no
/// `source_kind` filter when `source_kind` is `None`) so the result is never
/// derived from an arbitrary row.
///
/// For the legacy `source_kind = None` fallback the deterministic tie-break
/// is: rows with `parent_session_id IS NULL` first (main agent rows before
/// subagent synthetic rows), then `source_kind ASC`, then `turn_no ASC`, and
/// finally the smallest `id` as a final stable tie-breaker. Callers that
/// also query [`get_session_cwd`], [`get_session_model`], and
/// [`get_session_turns_token_stats`] with the same `None` arguments MUST
/// observe the same tie-break so identity, CWD, model, and turn stats
/// always select the same source row (otherwise a request could pick
/// identity from Copilot CLI while CWD/model/turn stats come from VS Code
/// Chat for the same session id).
///
/// `parent_session_id` and `agent_nickname` are populated for Copilot App
/// subagent synthetic rows (`<main>__<agent_id>`); they are `None` for main
/// agent rows and for non-Copilot-App sessions. The session detail handler uses
/// them to locate the shared `events.jsonl` under the parent session
/// directory and to filter events by agent id, so it must NOT derive the
/// parent/agent from string-splitting the synthetic id.
pub fn get_session_assistant_and_transcript(
    conn: &rusqlite::Connection,
    assistant: &str,
    session_id: &str,
    source_kind: Option<&str>,
    source_dir_key: Option<&str>,
) -> Result<SessionIdentity, String> {
    // Build a deterministic query:
    // - When source_kind is provided, filter by it exactly.
    // - When source_dir_key is Some, filter by source_dir_key = ?.
    // - When source_dir_key is None, filter by source_dir_key IS NULL.
    //   This ensures None means "no source directory" (non-App), not "any".
    // - When source_kind is None (legacy), resolve a single source_kind via
    //   `resolve_session_source_kind` and then filter by it. This keeps the
    //   legacy `None` fallback consistent with the other three session
    //   helpers (`get_session_cwd`, `get_session_model`,
    //   `get_session_turns_token_stats`) so identity, CWD, model, and turn
    //   stats always observe the same source row even when the same
    //   session_id has rows from multiple collectors (e.g. `copilot-cli`
    //   turn 1 + `vscode-chat` turn 2, both with `source_dir_key IS NULL`).
    let resolved_source_kind =
        resolve_session_source_kind(conn, assistant, session_id, source_kind, source_dir_key)?;
    // No matching row exists; mirror the previous "Session not found" error
    // so callers see the same behaviour when the legacy lookup has nothing
    // to fall back on.
    let resolved_source_kind = match resolved_source_kind {
        Some(k) => k,
        None => return Err("Session not found".to_string()),
    };

    let mut sql = String::from(
        "SELECT assistant_type, transcript_path, source_kind, source_dir_key,
                parent_session_id, agent_nickname
         FROM usage_entries
         WHERE session_id = ? AND assistant_type = ? AND source_kind = ?",
    );
    let mut params_vec: Vec<rusqlite::types::Value> = vec![
        rusqlite::types::Value::Text(session_id.to_string()),
        rusqlite::types::Value::Text(assistant.to_string()),
        rusqlite::types::Value::Text(resolved_source_kind),
    ];

    if let Some(key) = source_dir_key {
        sql.push_str(" AND source_dir_key = ?");
        params_vec.push(rusqlite::types::Value::Text(key.to_string()));
    } else {
        // None means source_dir_key IS NULL (non-App rows only).
        sql.push_str(" AND source_dir_key IS NULL");
    }

    // Deterministic ordering: main agent rows first
    // (`(parent_session_id IS NULL) DESC` puts NULL/main rows ahead of
    // NOT NULL/subagent rows), then `source_kind ASC`, then the earliest
    // turn, then the smallest row `id` as a final tie-breaker so the choice
    // is stable even when multiple rows share the same (source_kind,
    // turn_no).
    sql.push_str(
        " ORDER BY (parent_session_id IS NULL) DESC, source_kind ASC, turn_no ASC, id ASC LIMIT 1",
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let mut rows = stmt
        .query(rusqlite::params_from_iter(params_vec))
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
        let parent_session_id = row
            .get::<_, Option<String>>(4)
            .ok()
            .flatten()
            .filter(|s| !s.is_empty());
        let agent_nickname = row
            .get::<_, Option<String>>(5)
            .ok()
            .flatten()
            .filter(|s| !s.is_empty());
        Ok((
            ast,
            path,
            source_kind,
            source_dir_key,
            parent_session_id,
            agent_nickname,
        ))
    } else {
        Err("Session not found".to_string())
    }
}

pub fn get_session_cwd(
    conn: &rusqlite::Connection,
    assistant: &str,
    session_id: &str,
    source_kind: Option<&str>,
    source_dir_key: Option<&str>,
) -> Result<Option<String>, String> {
    // None means source_dir_key IS NULL (non-App rows only), not "any source".
    // The legacy `source_kind = None` path MUST resolve a single
    // `source_kind` via `resolve_session_source_kind` so the CWD lookup
    // always observes the same source row as identity / model / turn
    // stats; otherwise a session_id with rows from multiple collectors
    // (e.g. `copilot-cli` turn 1 + `vscode-chat` turn 2, both
    // `source_dir_key IS NULL`) could leak CWD from a different source.
    let resolved_source_kind =
        resolve_session_source_kind(conn, assistant, session_id, source_kind, source_dir_key)?;
    let resolved_source_kind = match resolved_source_kind {
        Some(k) => k,
        None => return Ok(None),
    };

    let mut sql = String::from(
        "SELECT cwd FROM usage_entries
         WHERE assistant_type = ? AND session_id = ? AND source_kind = ?
           AND cwd IS NOT NULL",
    );
    let mut params_vec: Vec<rusqlite::types::Value> = vec![
        rusqlite::types::Value::Text(assistant.to_string()),
        rusqlite::types::Value::Text(session_id.to_string()),
        rusqlite::types::Value::Text(resolved_source_kind),
    ];
    if let Some(key) = source_dir_key {
        sql.push_str(" AND source_dir_key = ?");
        params_vec.push(rusqlite::types::Value::Text(key.to_string()));
    } else {
        sql.push_str(" AND source_dir_key IS NULL");
    }
    sql.push_str(
        " ORDER BY (parent_session_id IS NULL) DESC, source_kind ASC, turn_no ASC, id ASC LIMIT 1",
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let mut rows = stmt
        .query(rusqlite::params_from_iter(params_vec))
        .map_err(|e| e.to_string())?;
    if let Some(row) = rows.next().map_err(|e| e.to_string())? {
        Ok(row.get::<_, String>(0).ok())
    } else {
        Ok(None)
    }
}

/// Returns the canonical model for a session row from the database.
///
/// For Copilot App / CLI subagent synthetic rows (`<main>__<agent_id>`), this
/// returns the child session's own model (the one written by the collector for
/// the subagent's usage), NOT the parent session's model. The session detail
/// handler uses this to seed the timeline parser so the subagent drawer's
/// `metadata.selected_model` and `AgentReply.model` reflect the child model
/// even when the shared `events.jsonl` only carries the parent's
/// `session.start.selectedModel`.
///
/// `source_dir_key` mirrors the same scoping as [`get_session_cwd`] and
/// [`get_session_turns_token_stats`]. Returns `None` when the session has no
/// model column populated (caller then falls back to the parser default).
pub fn get_session_model(
    conn: &rusqlite::Connection,
    assistant: &str,
    session_id: &str,
    source_kind: Option<&str>,
    source_dir_key: Option<&str>,
) -> Result<Option<String>, String> {
    // None means source_dir_key IS NULL (non-App rows only), not "any source".
    // The legacy `source_kind = None` path MUST resolve a single
    // `source_kind` via `resolve_session_source_kind` so the model lookup
    // always observes the same source row as identity / CWD / turn stats;
    // otherwise a session_id with rows from multiple collectors could leak
    // the model from a different source.
    let resolved_source_kind =
        resolve_session_source_kind(conn, assistant, session_id, source_kind, source_dir_key)?;
    let resolved_source_kind = match resolved_source_kind {
        Some(k) => k,
        None => return Ok(None),
    };

    let mut sql = String::from(
        "SELECT model FROM usage_entries
         WHERE assistant_type = ? AND session_id = ? AND source_kind = ?
           AND model IS NOT NULL AND model != ''",
    );
    let mut params_vec: Vec<rusqlite::types::Value> = vec![
        rusqlite::types::Value::Text(assistant.to_string()),
        rusqlite::types::Value::Text(session_id.to_string()),
        rusqlite::types::Value::Text(resolved_source_kind),
    ];
    if let Some(key) = source_dir_key {
        sql.push_str(" AND source_dir_key = ?");
        params_vec.push(rusqlite::types::Value::Text(key.to_string()));
    } else {
        sql.push_str(" AND source_dir_key IS NULL");
    }
    sql.push_str(
        " ORDER BY (parent_session_id IS NULL) DESC, source_kind ASC, turn_no ASC, id ASC LIMIT 1",
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let mut rows = stmt
        .query(rusqlite::params_from_iter(params_vec))
        .map_err(|e| e.to_string())?;
    if let Some(row) = rows.next().map_err(|e| e.to_string())? {
        Ok(row.get::<_, Option<String>>(0).ok().flatten())
    } else {
        Ok(None)
    }
}

fn add_optional_token_total(current: &mut Option<u64>, incoming: Option<u64>) {
    if let Some(value) = incoming {
        *current = Some(current.unwrap_or(0).saturating_add(value));
    }
}

pub fn get_session_turns_token_stats(
    conn: &rusqlite::Connection,
    assistant: &str,
    session_id: &str,
    source_kind: Option<&str>,
    source_dir_key: Option<&str>,
) -> Result<HashMap<u32, (TokenStats, String)>, String> {
    let mut map: HashMap<u32, (TokenStats, String)> = HashMap::new();
    // None means source_dir_key IS NULL (non-App rows only), not "any source".
    // The legacy `source_kind = None` path MUST resolve a single
    // `source_kind` via `resolve_session_source_kind` so the turn-stats
    // lookup always observes the same source row as identity / CWD / model
    // AND so all returned turns come from a single collector. Without
    // resolving first, the previous per-turn "first row encountered" rule
    // could mix rows from different sources (e.g. `copilot-cli` turn 1 +
    // `vscode-chat` turn 2, both `source_dir_key IS NULL`) into a single
    // map, violating the "legacy fallback must pick a single source"
    // contract.
    let resolved_source_kind =
        resolve_session_source_kind(conn, assistant, session_id, source_kind, source_dir_key)?;
    let resolved_source_kind = match resolved_source_kind {
        Some(k) => k,
        None => return Ok(map),
    };

    let mut sql = String::from(
        "SELECT turn_no, delta_input, delta_output, delta_cache_read, delta_cache_write, delta_cache_write_5m, delta_cache_write_1h, delta_reasoning, delta_total, model
         FROM usage_entries
         WHERE assistant_type = ? AND session_id = ? AND source_kind = ?",
    );
    let mut params_vec: Vec<rusqlite::types::Value> = vec![
        rusqlite::types::Value::Text(assistant.to_string()),
        rusqlite::types::Value::Text(session_id.to_string()),
        rusqlite::types::Value::Text(resolved_source_kind),
    ];
    if let Some(key) = source_dir_key {
        sql.push_str(" AND source_dir_key = ?");
        params_vec.push(rusqlite::types::Value::Text(key.to_string()));
    } else {
        sql.push_str(" AND source_dir_key IS NULL");
    }
    // Order by turn_no first so the per-turn map mirrors natural turn order.
    // Within the same turn_no, prefer main agent rows
    // (`(parent_session_id IS NULL) DESC` puts NULL/main rows ahead of
    // NOT NULL/subagent rows) so turn stats never mix main and subagent
    // synthetic rows; then `source_kind ASC` (deterministic) and finally
    // `id ASC` as the stable final tie-breaker. After resolving the
    // source_kind above, every row in the result set already comes from
    // the same source, so this ORDER BY only matters for tie-breaks
    // within that source.
    sql.push_str(
        " ORDER BY turn_no ASC, (parent_session_id IS NULL) DESC, source_kind ASC, id ASC",
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let mut rows = stmt
        .query(rusqlite::params_from_iter(params_vec))
        .map_err(|e| e.to_string())?;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        if let (Ok(turn_no), Ok(delta_input), Ok(delta_output), Ok(delta_total)) = (
            row.get::<_, i64>(0),
            row.get::<_, Option<i64>>(1),
            row.get::<_, Option<i64>>(2),
            row.get::<_, Option<i64>>(8),
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
                let cache_write_5m = row
                    .get::<_, Option<i64>>(5)
                    .ok()
                    .flatten()
                    .map(|v| v as u64);
                let cache_write_1h = row
                    .get::<_, Option<i64>>(6)
                    .ok()
                    .flatten()
                    .map(|v| v as u64);
                let reasoning = row
                    .get::<_, Option<i64>>(7)
                    .ok()
                    .flatten()
                    .map(|v| v as u64);
                let model = row
                    .get::<_, Option<String>>(9)
                    .unwrap_or(None)
                    .unwrap_or_else(|| "Gemini".to_string());
                let turn_no = turn_no as u32;
                if let Some((existing, existing_models)) = map.get_mut(&turn_no) {
                    existing.input = existing.input.saturating_add(input as u64);
                    existing.output = existing.output.saturating_add(output as u64);
                    add_optional_token_total(&mut existing.cache_read, cache_read);
                    add_optional_token_total(&mut existing.cache_write, cache_write);
                    add_optional_token_total(&mut existing.cache_write_5m, cache_write_5m);
                    add_optional_token_total(&mut existing.cache_write_1h, cache_write_1h);
                    add_optional_token_total(&mut existing.reasoning, reasoning);
                    existing.total = existing.total.saturating_add(total as u64);
                    if !existing_models
                        .split(" + ")
                        .any(|existing_model| existing_model == model)
                    {
                        existing_models.push_str(" + ");
                        existing_models.push_str(&model);
                    }
                } else {
                    map.insert(
                        turn_no,
                        (
                            TokenStats {
                                input: input as u64,
                                output: output as u64,
                                cache_read,
                                cache_write,
                                cache_write_5m,
                                cache_write_1h,
                                reasoning,
                                total: total as u64,
                            },
                            model,
                        ),
                    );
                }
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
            tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, tokens_cache_write_5m, tokens_cache_write_1h, tokens_reasoning, tokens_total,
            delta_input, delta_output, delta_cache_read, delta_cache_write, delta_cache_write_5m, delta_cache_write_1h, delta_reasoning, delta_total,
            duration_ms, premium_requests, parent_session_id, agent_nickname, agent_role, assistant_type, reasoning_effort,
            date, source_kind, source_dir_key, reported_cost_usd
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
        let ast_type = row.get::<_, String>(30).map_err(|e| e.to_string())?;
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
        let tokens_cache_write_5m: Option<u64> = row
            .get::<_, Option<i64>>(13)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let tokens_cache_write_1h: Option<u64> = row
            .get::<_, Option<i64>>(14)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let tokens_reasoning: Option<u64> = row
            .get::<_, Option<i64>>(15)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let tokens_total: Option<u64> = row
            .get::<_, Option<i64>>(16)
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
                cache_write_5m: tokens_cache_write_5m,
                cache_write_1h: tokens_cache_write_1h,
                reasoning: tokens_reasoning,
                total,
            })
        } else {
            None
        };

        let delta_input: Option<u64> = row
            .get::<_, Option<i64>>(17)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let delta_output: Option<u64> = row
            .get::<_, Option<i64>>(18)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let delta_cache_read: Option<u64> = row
            .get::<_, Option<i64>>(19)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let delta_cache_write: Option<u64> = row
            .get::<_, Option<i64>>(20)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let delta_cache_write_5m: Option<u64> = row
            .get::<_, Option<i64>>(21)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let delta_cache_write_1h: Option<u64> = row
            .get::<_, Option<i64>>(22)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let delta_reasoning: Option<u64> = row
            .get::<_, Option<i64>>(23)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let delta_total: Option<u64> = row
            .get::<_, Option<i64>>(24)
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
                cache_write_5m: delta_cache_write_5m,
                cache_write_1h: delta_cache_write_1h,
                reasoning: delta_reasoning,
                total,
            })
        } else {
            None
        };

        let duration_ms: Option<f64> = row
            .get::<_, Option<i64>>(25)
            .map_err(|e| e.to_string())?
            .map(|v| v as f64);
        let premium_requests: Option<f64> = row
            .get::<_, Option<i64>>(26)
            .map_err(|e| e.to_string())?
            .map(|v| v as f64);

        let reported_cost_usd: Option<f64> =
            row.get::<_, Option<f64>>(35).map_err(|e| e.to_string())?;
        let cost =
            if duration_ms.is_some() || premium_requests.is_some() || reported_cost_usd.is_some() {
                Some(CostStats {
                    total_api_duration_ms: duration_ms,
                    total_duration_ms: None,
                    total_premium_requests: premium_requests,
                    reported_cost_usd,
                })
            } else {
                None
            };

        let entry_date = row.get::<_, String>(32).map_err(|e| e.to_string())?;

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
                source_kind: row.get(33).ok(),
                source_dir_key: row.get(34).ok(),
                parent_session_id: row.get(27).ok(),
                agent_nickname: row.get(28).ok(),
                agent_role: row.get(29).ok(),
                reasoning_effort: row.get(31).ok(),
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
            tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, tokens_cache_write_5m, tokens_cache_write_1h, tokens_reasoning, tokens_total,
            delta_input, delta_output, delta_cache_read, delta_cache_write, delta_cache_write_5m, delta_cache_write_1h, delta_reasoning, delta_total,
            duration_ms, premium_requests, parent_session_id, agent_nickname, agent_role, assistant_type, reasoning_effort,
            date, source_kind, source_dir_key, reported_cost_usd
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
        let ast_type = row.get::<_, String>(30).map_err(|e| e.to_string())?;
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
        let tokens_cache_write_5m: Option<u64> = row
            .get::<_, Option<i64>>(13)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let tokens_cache_write_1h: Option<u64> = row
            .get::<_, Option<i64>>(14)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let tokens_reasoning: Option<u64> = row
            .get::<_, Option<i64>>(15)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let tokens_total: Option<u64> = row
            .get::<_, Option<i64>>(16)
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
                cache_write_5m: tokens_cache_write_5m,
                cache_write_1h: tokens_cache_write_1h,
                reasoning: tokens_reasoning,
                total,
            })
        } else {
            None
        };

        let delta_input: Option<u64> = row
            .get::<_, Option<i64>>(17)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let delta_output: Option<u64> = row
            .get::<_, Option<i64>>(18)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let delta_cache_read: Option<u64> = row
            .get::<_, Option<i64>>(19)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let delta_cache_write: Option<u64> = row
            .get::<_, Option<i64>>(20)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let delta_cache_write_5m: Option<u64> = row
            .get::<_, Option<i64>>(21)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let delta_cache_write_1h: Option<u64> = row
            .get::<_, Option<i64>>(22)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let delta_reasoning: Option<u64> = row
            .get::<_, Option<i64>>(23)
            .map_err(|e| e.to_string())?
            .map(|v| v as u64);
        let delta_total: Option<u64> = row
            .get::<_, Option<i64>>(24)
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
                cache_write_5m: delta_cache_write_5m,
                cache_write_1h: delta_cache_write_1h,
                reasoning: delta_reasoning,
                total,
            })
        } else {
            None
        };

        let duration_ms: Option<f64> = row
            .get::<_, Option<i64>>(25)
            .map_err(|e| e.to_string())?
            .map(|v| v as f64);
        let premium_requests: Option<f64> = row
            .get::<_, Option<i64>>(26)
            .map_err(|e| e.to_string())?
            .map(|v| v as f64);

        let reported_cost_usd: Option<f64> =
            row.get::<_, Option<f64>>(35).map_err(|e| e.to_string())?;
        let cost =
            if duration_ms.is_some() || premium_requests.is_some() || reported_cost_usd.is_some() {
                Some(CostStats {
                    total_api_duration_ms: duration_ms,
                    total_duration_ms: None,
                    total_premium_requests: premium_requests,
                    reported_cost_usd,
                })
            } else {
                None
            };

        let entry_date = row.get::<_, String>(32).map_err(|e| e.to_string())?;

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
                source_kind: row.get(33).ok(),
                source_dir_key: row.get(34).ok(),
                parent_session_id: row.get(27).ok(),
                agent_nickname: row.get(28).ok(),
                agent_role: row.get(29).ok(),
                reasoning_effort: row.get(31).ok(),
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

    fn create_copilot_app_registry_from_events(app_dir: &Path) {
        let store = Connection::open(app_dir.join("session-store.db")).unwrap();
        let session_ids: Vec<String> = store
            .prepare("SELECT DISTINCT session_id FROM assistant_usage_events")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        drop(store);

        let data_db = Connection::open(app_dir.join("data.db")).unwrap();
        data_db
            .execute(
                "CREATE TABLE IF NOT EXISTS sessions (
                    id TEXT PRIMARY KEY,
                    title TEXT
                 )",
                [],
            )
            .unwrap();
        for session_id in session_ids {
            data_db
                .execute(
                    "INSERT OR IGNORE INTO sessions (id, title) VALUES (?, NULL)",
                    params![session_id],
                )
                .unwrap();
        }
    }

    fn create_copilot_app_registry(app_dir: &Path, session_ids: &[&str]) {
        let data_db = Connection::open(app_dir.join("data.db")).unwrap();
        data_db
            .execute(
                "CREATE TABLE IF NOT EXISTS sessions (
                    id TEXT PRIMARY KEY,
                    title TEXT
                 )",
                [],
            )
            .unwrap();
        for session_id in session_ids {
            data_db
                .execute(
                    "INSERT OR IGNORE INTO sessions (id, title) VALUES (?, NULL)",
                    params![session_id],
                )
                .unwrap();
        }
    }

    fn create_test_copilot_session_store(app_dir: &Path) -> Connection {
        let store = Connection::open(app_dir.join("session-store.db")).unwrap();
        store
            .execute(
                "CREATE TABLE assistant_usage_events (
                    id INTEGER PRIMARY KEY,
                    session_id TEXT,
                    turn_index INTEGER,
                    model TEXT,
                    agent_id TEXT,
                    initiator TEXT,
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
            .execute(
                "CREATE TABLE sessions (
                    id TEXT PRIMARY KEY,
                    cwd TEXT,
                    repository TEXT,
                    host_type TEXT,
                    branch TEXT,
                    summary TEXT,
                    created_at TEXT,
                    updated_at TEXT
                 )",
                [],
            )
            .unwrap();
        store
    }

    fn insert_test_session_cwd(store: &Connection, session_id: &str, cwd: &str) {
        store
            .execute(
                "INSERT INTO sessions (id, cwd) VALUES (?, ?)",
                params![session_id, cwd],
            )
            .unwrap();
    }

    fn insert_test_copilot_event(
        store: &Connection,
        id: i64,
        session_id: &str,
        turn_index: i64,
        created_at: &str,
    ) {
        store
            .execute(
                "INSERT INTO assistant_usage_events
                    (id, session_id, turn_index, model,
                     input_tokens, output_tokens,
                     cache_read_tokens, cache_write_tokens,
                     reasoning_tokens, duration_ms,
                     reasoning_effort, created_at)
                 VALUES (?, ?, ?, 'gpt-5', 100, 10, 0, 0, 0, 100, 'medium', ?)",
                params![id, session_id, turn_index, created_at],
            )
            .unwrap();
    }

    fn test_copilot_source_key(app_dir: &Path) -> String {
        let canonical = app_dir.canonicalize().unwrap();
        encode_hex(canonical.as_os_str().as_encoded_bytes())
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
                    cache_write_5m: None,
                    cache_write_1h: None,
                    reasoning: Some(5),
                    total: 120,
                }),
                delta_tokens: Some(TokenStats {
                    input: 10,
                    output: 2,
                    cache_read: Some(3),
                    cache_write: Some(1),
                    cache_write_5m: None,
                    cache_write_1h: None,
                    reasoning: Some(1),
                    total: 12,
                }),
                context: None,
                cost: Some(CostStats {
                    total_api_duration_ms: Some(125.0),
                    total_duration_ms: None,
                    total_premium_requests: Some(1.0),
                    reported_cost_usd: None,
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
            cache_write_5m: None,
            cache_write_1h: None,
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
            cache_write_5m: None,
            cache_write_1h: None,
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
            cache_write_5m: None,
            cache_write_1h: None,
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

        let first = import_usage_day_entries(
            &mut conn,
            "codex",
            "2026-07-10",
            vec![record.clone()],
            UsageImportMetadata::default(),
        )
        .unwrap();
        assert_eq!(first.imported, 1);
        assert_eq!(first.skipped_duplicates, 0);

        let second = import_usage_day_entries(
            &mut conn,
            "codex",
            "2026-07-10",
            vec![record],
            UsageImportMetadata::default(),
        )
        .unwrap();
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
    fn import_uses_each_record_timestamp_date_and_period_export_includes_all_dates() {
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        let first = sample_import_record();
        let mut second = sample_import_record();
        second.entry.timestamp = "2026-07-11T01:02:03Z".to_string();
        second.entry.session_id = "import-session-next-day".to_string();
        second.import_source_id = Some("import-test-record-next-day".to_string());

        let summary = import_usage_day_entries(
            &mut conn,
            "codex",
            "2026-07",
            vec![first, second],
            UsageImportMetadata::default(),
        )
        .unwrap();

        assert_eq!(summary.imported, 2);
        assert_eq!(
            get_available_dates(&conn, "codex").unwrap(),
            vec!["2026-07-11", "2026-07-10"]
        );
        assert_eq!(
            export_usage_period_entries(&conn, "codex", "2026-07")
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            export_usage_period_entries(&conn, "codex", "2026")
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn import_batches_track_source_and_rollback_only_imported_rows() {
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, timestamp, date, session_id, turn_no
             ) VALUES (
                'codex', '2026-07-10T00:00:00Z', '2026-07-10', 'native-session', 1
             )",
            [],
        )
        .unwrap();
        let record = sample_import_record();
        let summary = import_usage_day_entries(
            &mut conn,
            "codex",
            "2026-07-10",
            vec![record],
            UsageImportMetadata {
                source_assistant: Some("codex".to_string()),
                source_file_name: Some("token-usage-codex.json".to_string()),
            },
        )
        .unwrap();

        let batches = list_usage_import_batches(&conn, "codex", 50).unwrap();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].id, summary.batch_id);
        assert_eq!(batches[0].source_assistant.as_deref(), Some("codex"));
        assert_eq!(
            batches[0].source_file_name.as_deref(),
            Some("token-usage-codex.json")
        );
        assert_eq!(batches[0].imported, 1);
        assert_eq!(batches[0].rolled_back_at, None);

        let rollback = rollback_usage_import_batch(&mut conn, "codex", &summary.batch_id).unwrap();
        assert_eq!(rollback.removed_records, 1);
        let remaining_native_rows: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries WHERE session_id = 'native-session'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let remaining_imported_rows: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries WHERE import_batch_id = ?",
                params![summary.batch_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining_native_rows, 1);
        assert_eq!(remaining_imported_rows, 0);

        let batches = list_usage_import_batches(&conn, "codex", 50).unwrap();
        assert!(batches[0].rolled_back_at.is_some());
        assert_eq!(batches[0].removed_records, 1);
        let error = rollback_usage_import_batch(&mut conn, "codex", &summary.batch_id).unwrap_err();
        assert_eq!(error, "指定的匯入批次已撤銷");
    }

    #[test]
    fn legacy_import_without_ttl_fields_keeps_previous_source_id() {
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        let mut record = sample_import_record();
        record.entry.session_id = "legacy-source-id".to_string();
        record.entry.model = Some("claude-fable-5".to_string());
        record.entry.model_id = Some("claude-fable-5".to_string());
        record.import_source_id = None;

        let entry = &record.entry;
        let tokens = entry.tokens.as_ref().unwrap();
        let delta = entry.delta_tokens.as_ref().unwrap();
        let legacy_tokens_signature = format!(
            "{}|{}|{}|{}|{}|{}",
            tokens.input,
            tokens.output,
            tokens.cache_read.unwrap_or(0),
            tokens.cache_write.unwrap_or(0),
            tokens.reasoning.unwrap_or(0),
            tokens.total
        );
        let legacy_delta_signature = format!(
            "{}|{}|{}|{}|{}|{}",
            delta.input,
            delta.output,
            delta.cache_read.unwrap_or(0),
            delta.cache_write.unwrap_or(0),
            delta.reasoning.unwrap_or(0),
            delta.total
        );
        let legacy_signature = format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
            "claude",
            "2026-07-10",
            entry.timestamp,
            entry.session_id,
            entry.turn_no,
            entry.model.as_deref().unwrap_or_default(),
            entry.model_id.as_deref().unwrap_or_default(),
            entry.version.as_deref().unwrap_or_default(),
            entry.cwd.as_deref().unwrap_or_default(),
            entry.transcript_path.as_deref().unwrap_or_default(),
            entry.parent_session_id.as_deref().unwrap_or_default(),
            entry.agent_nickname.as_deref().unwrap_or_default(),
            entry.agent_role.as_deref().unwrap_or_default(),
            legacy_tokens_signature,
            legacy_delta_signature
        );
        let legacy_source_id = format!("{:016x}", hash_fnv1a_64(&legacy_signature));
        assert_eq!(
            build_usage_entry_import_source_id("claude", "2026-07-10", entry),
            legacy_source_id
        );

        let mut existing_record = record.clone();
        existing_record.import_source_id = Some(legacy_source_id);
        let first = import_usage_day_entries(
            &mut conn,
            "claude",
            "2026-07-10",
            vec![existing_record],
            UsageImportMetadata::default(),
        )
        .unwrap();
        assert_eq!(first.imported, 1);

        let second = import_usage_day_entries(
            &mut conn,
            "claude",
            "2026-07-10",
            vec![record],
            UsageImportMetadata::default(),
        )
        .unwrap();
        assert_eq!(second.imported, 0);
        assert_eq!(second.skipped_duplicates, 1);
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
            cache_write_5m: None,
            cache_write_1h: None,
            reasoning: Some(384),
            total: 444_924,
        });
        record.entry.delta_tokens = record.entry.tokens.clone();
        record.import_source_id = Some("imported-copilot-cache".to_string());

        import_usage_day_entries(
            &mut conn,
            "copilot",
            "2026-07-10",
            vec![record],
            UsageImportMetadata::default(),
        )
        .unwrap();

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
    fn import_usage_day_entries_normalizes_legacy_claude_cache_writes() {
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        let mut record = sample_import_record();
        record.entry.session_id = "imported-legacy-claude-cache".to_string();
        record.entry.model = Some("claude-fable-5".to_string());
        record.entry.model_id = Some("claude-fable-5".to_string());
        record.entry.tokens = Some(TokenStats {
            input: 110,
            output: 20,
            cache_read: Some(30),
            cache_write: Some(10),
            cache_write_5m: None,
            cache_write_1h: None,
            reasoning: None,
            total: 160,
        });
        record.entry.delta_tokens = Some(TokenStats {
            input: 11,
            output: 2,
            cache_read: Some(3),
            cache_write: Some(1),
            cache_write_5m: None,
            cache_write_1h: None,
            reasoning: None,
            total: 16,
        });
        record.import_source_id = Some("imported-legacy-claude-cache".to_string());

        import_usage_day_entries(
            &mut conn,
            "claude",
            "2026-07-10",
            vec![record],
            UsageImportMetadata::default(),
        )
        .unwrap();

        let inserted: (u64, u64, u64, u64, u64, u64) = conn
            .query_row(
                "SELECT tokens_input, tokens_cache_write_5m, tokens_cache_write_1h,
                        delta_input, delta_cache_write_5m, delta_cache_write_1h
                 FROM usage_entries
                 WHERE assistant_type = 'claude'
                   AND session_id = 'imported-legacy-claude-cache'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(inserted, (100, 10, 0, 10, 1, 0));
    }

    #[test]
    fn usage_queries_round_trip_claude_cache_write_ttls() {
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        let mut record = sample_import_record();
        record.entry.session_id = "claude-cache-write-ttl".to_string();
        record.entry.model = Some("claude-fable-5".to_string());
        record.entry.model_id = Some("claude-fable-5".to_string());
        record.entry.tokens = Some(TokenStats {
            input: 100,
            output: 20,
            cache_read: Some(30),
            cache_write: Some(10),
            cache_write_5m: Some(3),
            cache_write_1h: Some(7),
            reasoning: None,
            total: 160,
        });
        record.entry.delta_tokens = Some(TokenStats {
            input: 10,
            output: 2,
            cache_read: Some(3),
            cache_write: Some(4),
            cache_write_5m: Some(1),
            cache_write_1h: Some(3),
            reasoning: None,
            total: 19,
        });
        record.import_source_id = Some("claude-cache-write-ttl".to_string());

        import_usage_day_entries(
            &mut conn,
            "claude",
            "2026-07-10",
            vec![record],
            UsageImportMetadata::default(),
        )
        .unwrap();

        let day_entries = get_usage_entries_by_date(&conn, "2026-07-10", "claude").unwrap();
        let month_entries = get_usage_entries_by_month(&conn, "2026-07", "claude").unwrap();
        let year_entries = get_usage_entries_by_year(&conn, "2026", "claude").unwrap();
        let turn_entries =
            get_session_turns_token_stats(&conn, "claude", "claude-cache-write-ttl", None, None)
                .unwrap();
        let entries = [
            &day_entries[0].0.entry,
            &month_entries[0].0,
            &year_entries[0].0,
        ];

        for entry in entries {
            let tokens = entry.tokens.as_ref().unwrap();
            assert_eq!(tokens.input, 100);
            assert_eq!(tokens.cache_write, Some(10));
            assert_eq!(tokens.cache_write_5m, Some(3));
            assert_eq!(tokens.cache_write_1h, Some(7));

            let delta = entry.delta_tokens.as_ref().unwrap();
            assert_eq!(delta.input, 10);
            assert_eq!(delta.cache_write, Some(4));
            assert_eq!(delta.cache_write_5m, Some(1));
            assert_eq!(delta.cache_write_1h, Some(3));
        }

        let turn = &turn_entries.get(&1).unwrap().0;
        assert_eq!(turn.cache_write, Some(4));
        assert_eq!(turn.cache_write_5m, Some(1));
        assert_eq!(turn.cache_write_1h, Some(3));
    }

    #[test]
    fn session_detail_queries_are_scoped_by_assistant_and_source() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        for (source_kind, transcript_path, cwd, model, input) in [
            (
                "copilot-cli",
                "/tmp/cli/events.jsonl",
                "/tmp/cli",
                "gpt-5",
                10i64,
            ),
            (
                "vscode-chat",
                "/tmp/vscode/session.json",
                "/tmp/vscode",
                "gpt-4.1",
                20i64,
            ),
        ] {
            conn.execute(
                "INSERT INTO usage_entries (
                    assistant_type, source_kind, timestamp, date, session_id,
                    transcript_path, cwd, turn_no, model,
                    delta_input, delta_output, delta_total
                 ) VALUES (
                    'copilot', ?, '2026-07-10T10:00:00Z', '2026-07-10', 'shared',
                    ?, ?, 1, ?, ?, 1, ?
                 )",
                params![source_kind, transcript_path, cwd, model, input, input + 1],
            )
            .unwrap();
        }

        let (_, transcript_path, source_kind, _, _, _) = get_session_assistant_and_transcript(
            &conn,
            "copilot",
            "shared",
            Some("vscode-chat"),
            None,
        )
        .unwrap();
        assert_eq!(source_kind, "vscode-chat");
        assert_eq!(transcript_path.as_deref(), Some("/tmp/vscode/session.json"));

        let cwd = get_session_cwd(&conn, "copilot", "shared", Some("vscode-chat"), None).unwrap();
        assert_eq!(cwd.as_deref(), Some("/tmp/vscode"));

        let turns =
            get_session_turns_token_stats(&conn, "copilot", "shared", Some("vscode-chat"), None)
                .unwrap();
        let (tokens, model) = turns.get(&1).unwrap();
        assert_eq!(tokens.input, 20);
        assert_eq!(tokens.output, 1);
        assert_eq!(tokens.total, 21);
        assert_eq!(model, "gpt-4.1");
    }

    #[test]
    fn init_db_migrates_legacy_claude_cache_write_pricing_once() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, timestamp, date, session_id, turn_no,
                tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, tokens_total,
                delta_input, delta_output, delta_cache_read, delta_cache_write, delta_total
             ) VALUES (
                'claude', '2026-07-10T00:00:00Z', '2026-07-10', 'legacy-claude-cache', 1,
                110, 20, 30, 10, 160,
                11, 2, 3, 1, 16
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, timestamp, date, session_id, transcript_path, turn_no,
                tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, tokens_total,
                delta_input, delta_output, delta_cache_read, delta_cache_write, delta_total
             ) VALUES (
                'codex', '2026-07-10T00:01:00Z', '2026-07-10',
                'legacy-misclassified-claude-cache', '/home/user/.claude/projects/session.jsonl', 1,
                220, 40, 60, 20, 320,
                22, 4, 6, 2, 32
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sync_state (filename, last_synced_size, last_synced_time)
             VALUES ('claude:projects/session.jsonl', 100, 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sync_state (filename, last_synced_size, last_synced_time)
             VALUES ('codex:claude:projects/session.jsonl', 100, 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "DELETE FROM sync_state WHERE filename = ?",
            params![CLAUDE_CACHE_WRITE_PRICING_MIGRATION_KEY],
        )
        .unwrap();

        init_db(&conn).unwrap();
        init_db(&conn).unwrap();

        let migrated: (u64, u64, u64, u64, u64, u64) = conn
            .query_row(
                "SELECT tokens_input, tokens_cache_write_5m, tokens_cache_write_1h,
                        delta_input, delta_cache_write_5m, delta_cache_write_1h
                 FROM usage_entries
                 WHERE session_id = 'legacy-claude-cache'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .unwrap();
        let claude_sync_state_count: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_state
                 WHERE filename LIKE 'claude:%'
                    OR filename LIKE 'codex:claude:%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let misclassified_migrated: (u64, u64, u64, u64, u64, u64) = conn
            .query_row(
                "SELECT tokens_input, tokens_cache_write_5m, tokens_cache_write_1h,
                        delta_input, delta_cache_write_5m, delta_cache_write_1h
                 FROM usage_entries
                 WHERE session_id = 'legacy-misclassified-claude-cache'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .unwrap();
        let migration_marker_count: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_state WHERE filename = ?",
                params![CLAUDE_CACHE_WRITE_PRICING_MIGRATION_KEY],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(migrated, (100, 10, 0, 10, 1, 0));
        assert_eq!(misclassified_migrated, (200, 20, 0, 20, 2, 0));
        assert_eq!(claude_sync_state_count, 0);
        assert_eq!(migration_marker_count, 1);
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
    fn parse_codex_session_file_sums_completed_task_durations() {
        let path = temp_jsonl_path("codex-task-duration");

        let content = r#"{"timestamp":"2026-07-07T10:58:17.474Z","type":"session_meta","payload":{"session_id":"session-duration","model":"gpt-5.5"}}
{"timestamp":"2026-07-07T10:58:18.000Z","type":"event_msg","payload":{"type":"task_started"}}
{"timestamp":"2026-07-07T10:58:19.000Z","type":"event_msg","payload":{"type":"task_complete","duration_ms":1200}}
{"timestamp":"2026-07-07T10:58:20.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":10,"reasoning_output_tokens":4,"total_tokens":110},"model_context_window":258400}}}
{"timestamp":"2026-07-07T10:58:21.000Z","type":"event_msg","payload":{"type":"task_started"}}
{"timestamp":"2026-07-07T10:58:22.000Z","type":"event_msg","payload":{"type":"task_complete","duration_ms":2300}}
{"timestamp":"2026-07-07T10:58:23.000Z","type":"event_msg","payload":{"type":"task_started","duration_ms":9000}}
{"timestamp":"2026-07-07T10:58:24.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":130,"cached_input_tokens":30,"output_tokens":15,"reasoning_output_tokens":7,"total_tokens":145},"model_context_window":258400}}}
"#;

        fs::write(&path, content).unwrap();
        let entries = parse_codex_session_file(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|entry| {
            entry
                .cost
                .as_ref()
                .and_then(|cost| cost.total_api_duration_ms)
                == Some(3500.0)
        }));
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
    fn parse_codex_desktop_session_preserves_source_and_cache_write_tokens() {
        let path = temp_jsonl_path("codex-desktop");
        let content = r#"{"timestamp":"2026-07-26T10:00:00Z","type":"session_meta","payload":{"id":"desktop-session","session_id":"desktop-session","originator":"Codex Desktop","source":"vscode","cwd":"/tmp/project","cli_version":"0.145.0-alpha.30"}}
{"timestamp":"2026-07-26T10:00:00.500Z","type":"session_meta","payload":{"id":"desktop-session","session_id":"desktop-session","source":"cli","cwd":"/tmp/project","cli_version":"0.145.0-alpha.30"}}
{"timestamp":"2026-07-26T10:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"cache_write_input_tokens":5,"output_tokens":10,"reasoning_output_tokens":4,"total_tokens":110},"model_context_window":258400}}}
{"timestamp":"2026-07-26T10:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":150,"cached_input_tokens":30,"cache_write_input_tokens":8,"output_tokens":15,"reasoning_output_tokens":7,"total_tokens":165},"model_context_window":258400}}}
"#;

        fs::write(&path, content).unwrap();
        let entries = parse_codex_session_file(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(entries.len(), 2);
        assert!(entries
            .iter()
            .all(|entry| entry.source_kind.as_deref() == Some(CODEX_DESKTOP_SOURCE_KIND)));

        let first = entries[0].delta_tokens.as_ref().unwrap();
        assert_eq!(first.cache_write, Some(5));

        let second = entries[1].delta_tokens.as_ref().unwrap();
        assert_eq!(second.input, 40);
        assert_eq!(second.cache_read, Some(10));
        assert_eq!(second.cache_write, Some(3));
        assert_eq!(second.output, 5);
        assert_eq!(second.reasoning, Some(3));
        assert_eq!(second.total, 55);
    }

    #[test]
    fn sync_codex_usage_logs_tracks_archived_and_unarchived_desktop_sessions() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_codex_dir = std::env::var("CODEX_DIR").ok();
        let mut codex_dir = std::env::temp_dir();
        let unique = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        codex_dir.push(format!(
            "codex-archive-sync-{}-{}",
            std::process::id(),
            unique
        ));

        let sessions_dir = codex_dir.join("sessions/2026/07/26");
        let archived_dir = codex_dir.join("archived_sessions");
        fs::create_dir_all(&sessions_dir).unwrap();
        fs::create_dir_all(&archived_dir).unwrap();
        let active_path = sessions_dir.join("rollout-2026-07-26T10-00-00-desktop-session.jsonl");
        let archived_path = archived_dir.join("rollout-2026-07-26T10-00-00-desktop-session.jsonl");
        let content = r#"{"timestamp":"2026-07-26T10:00:00Z","type":"session_meta","payload":{"id":"desktop-session","session_id":"desktop-session","originator":"Codex Desktop","source":"vscode","cwd":"/tmp/project","cli_version":"0.145.0-alpha.30"}}
{"timestamp":"2026-07-26T10:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"cache_write_input_tokens":5,"output_tokens":10,"reasoning_output_tokens":4,"total_tokens":110},"model_context_window":258400}}}
"#;

        fs::write(&active_path, content).unwrap();
        std::env::set_var("CODEX_DIR", &codex_dir);

        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        sync_codex_usage_logs(&mut conn).unwrap();

        let first_sync: (String, String, u64) = conn
            .query_row(
                "SELECT source_kind, transcript_path, tokens_cache_write
                 FROM usage_entries
                 WHERE assistant_type = 'codex' AND session_id = 'desktop-session'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(first_sync.0, CODEX_DESKTOP_SOURCE_KIND);
        assert_eq!(PathBuf::from(first_sync.1), active_path);
        assert_eq!(first_sync.2, 5);

        fs::rename(&active_path, &archived_path).unwrap();
        sync_codex_usage_logs(&mut conn).unwrap();
        let archived_transcript: String = conn
            .query_row(
                "SELECT transcript_path FROM usage_entries
                 WHERE assistant_type = 'codex' AND session_id = 'desktop-session'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(PathBuf::from(archived_transcript), archived_path);

        fs::rename(&archived_path, &active_path).unwrap();
        sync_codex_usage_logs(&mut conn).unwrap();
        let restored_transcript: String = conn
            .query_row(
                "SELECT transcript_path FROM usage_entries
                 WHERE assistant_type = 'codex' AND session_id = 'desktop-session'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(PathBuf::from(restored_transcript), active_path);

        if let Some(value) = old_codex_dir {
            std::env::set_var("CODEX_DIR", value);
        } else {
            std::env::remove_var("CODEX_DIR");
        }
        let _ = fs::remove_dir_all(&codex_dir);
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
{"timestamp":"2026-07-07T10:58:30.197Z","type":"event_msg","payload":{"type":"task_complete","duration_ms":1200}}
{"timestamp":"2026-07-07T10:59:26.197Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":10,"reasoning_output_tokens":4,"total_tokens":110},"last_token_usage":{"input_tokens":0,"cached_input_tokens":0,"output_tokens":0,"reasoning_output_tokens":0,"total_tokens":19347},"model_context_window":258400}}}
{"timestamp":"2026-07-07T10:59:30.197Z","type":"event_msg","payload":{"type":"task_complete","duration_ms":2300}}
{"timestamp":"2026-07-07T10:59:31.197Z","type":"event_msg","payload":{"type":"task_started"}}
"#;

        fs::write(&session_path, content).unwrap();
        std::env::set_var("CODEX_DIR", &codex_dir);

        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        conn.execute(
            "INSERT INTO sync_state (filename, last_synced_size, last_synced_time)
             VALUES ('migration:codex_session_identity_v6', 1, 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sync_state (filename, last_synced_size, last_synced_time)
             VALUES (?, 1, 0)",
            params![CODEX_SOURCE_KIND_MIGRATION_KEY],
        )
        .unwrap();
        let state_key = format!(
            "codex:{}",
            portable_relative_path(&codex_dir, &session_path)
        );
        conn.execute(
            "INSERT INTO sync_state (filename, last_synced_size, last_synced_time)
             VALUES (?, ?, 0)",
            params![state_key, fs::metadata(&session_path).unwrap().len()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO usage_entries (
                 assistant_type, timestamp, date, session_id, transcript_path, turn_no
             ) VALUES ('codex', '2026-07-07T10:58:26.197Z', '2026-07-07',
                 'session-sync', ?, 1)",
            params![session_path.to_string_lossy().as_ref()],
        )
        .unwrap();
        sync_codex_usage_logs(&mut conn).unwrap();

        let total: u64 = conn
            .query_row(
                "SELECT SUM(delta_total) FROM usage_entries WHERE assistant_type = 'codex' AND session_id = 'session-sync'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(total, 110);
        let durations: Vec<Option<i64>> = conn
            .prepare(
                "SELECT duration_ms
                 FROM usage_entries
                 WHERE assistant_type = 'codex' AND session_id = 'session-sync'
                 ORDER BY turn_no",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(durations, vec![Some(3500), Some(3500)]);

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
        let empty_child_size = empty_child_content.len() as u64;
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
        let state_after_empty_parse: (u64, i64) = conn
            .query_row(
                "SELECT last_synced_size, last_synced_time
                 FROM sync_state
                 WHERE filename = ?",
                params![child_state_key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(preserved_child_rows, 1);
        assert_eq!(state_after_empty_parse.0, empty_child_size);
        assert_eq!(state_after_empty_parse.1, CODEX_EMPTY_TRANSCRIPT_SYNC_TIME);

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
{"type":"assistant","sessionId":"session-1","cwd":"/tmp/project","version":"2.1.201","timestamp":"2026-07-04T19:28:51.753Z","uuid":"a1","requestId":"req_1","message":{"id":"msg_1","role":"assistant","model":"claude-haiku-4-5-20251001","content":[{"type":"thinking","thinking":"working"}],"usage":{"input_tokens":10,"cache_creation_input_tokens":3,"cache_read_input_tokens":7,"output_tokens":5,"cache_creation":{"ephemeral_5m_input_tokens":1,"ephemeral_1h_input_tokens":2}}}}
{"type":"assistant","sessionId":"session-1","cwd":"/tmp/project","version":"2.1.201","timestamp":"2026-07-04T19:28:51.948Z","uuid":"a2","requestId":"req_1","message":{"id":"msg_1","role":"assistant","model":"claude-haiku-4-5-20251001","content":[{"type":"text","text":"Done"}],"usage":{"input_tokens":10,"cache_creation_input_tokens":3,"cache_read_input_tokens":7,"output_tokens":5,"cache_creation":{"ephemeral_5m_input_tokens":1,"ephemeral_1h_input_tokens":2}}}}
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
        assert_eq!(tokens.input, 10);
        assert_eq!(tokens.cache_write, Some(3));
        assert_eq!(tokens.cache_write_5m, Some(1));
        assert_eq!(tokens.cache_write_1h, Some(2));
        assert_eq!(tokens.cache_read, Some(7));
        assert_eq!(tokens.output, 5);
        assert_eq!(tokens.total, 25);
    }

    #[test]
    fn parse_claude_session_file_defaults_unclassified_cache_writes_to_5m() {
        let path = temp_jsonl_path("claude-cache-default");
        let content = r#"{"type":"assistant","sessionId":"session-cache-default","timestamp":"2026-07-04T19:28:51.753Z","uuid":"a1","requestId":"req_1","message":{"id":"msg_1","role":"assistant","model":"claude-haiku-4-5-20251001","content":[{"type":"text","text":"Done"}],"usage":{"input_tokens":10,"cache_creation_input_tokens":3,"cache_read_input_tokens":7,"output_tokens":5}}}
"#;

        fs::write(&path, content).unwrap();
        let entries = parse_claude_session_file(&path).unwrap();
        let _ = fs::remove_file(&path);

        let tokens = entries[0].tokens.as_ref().unwrap();
        assert_eq!(tokens.input, 10);
        assert_eq!(tokens.cache_write, Some(3));
        assert_eq!(tokens.cache_write_5m, Some(3));
        assert_eq!(tokens.cache_write_1h, Some(0));
        assert_eq!(tokens.total, 25);
    }

    #[test]
    fn sync_claude_usage_logs_writes_cache_write_ttls() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_claude_dir = std::env::var("CLAUDE_DIR").ok();
        let claude_dir = temp_jsonl_path("claude-cache-sync").with_extension("");
        let projects_dir = claude_dir.join("projects/test-project");
        fs::create_dir_all(&projects_dir).unwrap();
        let session_path = projects_dir.join("session-cache-sync.jsonl");
        let content = r#"{"type":"assistant","sessionId":"session-cache-sync","timestamp":"2026-07-04T19:28:51.753Z","uuid":"a1","requestId":"req_1","message":{"id":"msg_1","role":"assistant","model":"claude-haiku-4-5-20251001","content":[{"type":"text","text":"Done"}],"usage":{"input_tokens":10,"cache_creation_input_tokens":3,"cache_read_input_tokens":7,"output_tokens":5,"cache_creation":{"ephemeral_5m_input_tokens":1,"ephemeral_1h_input_tokens":2}}}}
"#;
        fs::write(&session_path, content).unwrap();
        std::env::set_var("CLAUDE_DIR", &claude_dir);

        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        sync_claude_usage_logs(&mut conn).unwrap();

        let stored: (u64, u64, u64, u64, u64, u64) = conn
            .query_row(
                "SELECT tokens_input, tokens_cache_write_5m, tokens_cache_write_1h,
                        delta_input, delta_cache_write_5m, delta_cache_write_1h
                 FROM usage_entries
                 WHERE assistant_type = 'claude'
                   AND session_id = 'session-cache-sync'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(stored, (10, 1, 2, 10, 1, 2));

        if let Some(value) = old_claude_dir {
            std::env::set_var("CLAUDE_DIR", value);
        } else {
            std::env::remove_var("CLAUDE_DIR");
        }
        fs::remove_dir_all(claude_dir).unwrap();
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
        let entries =
            parse_cursor_session_file(&path, &HashMap::new(), &HashSet::new(), &HashMap::new())
                .unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(entries.len(), 2);
        assert!(entries
            .iter()
            .all(|entry| entry.entry.session_name.as_deref() == Some("第二條提示")));
    }

    #[test]
    fn cursor_response_signature_matches_plain_text_agent_kv_content() {
        let transcript_content = serde_json::json!([
            {
                "type": "text",
                "text": "Plain answer"
            }
        ]);
        let agent_kv_content = serde_json::json!([
            {
                "type": "text",
                "data": "Plain answer",
                "providerOptions": {
                    "cursor": {
                        "modelName": "composer-2.5"
                    }
                }
            }
        ]);

        assert_eq!(
            cursor_response_signature(&transcript_content),
            cursor_response_signature(&agent_kv_content)
        );
    }

    #[test]
    fn cursor_response_signature_matches_agent_kv_tool_calls() {
        let transcript_content = serde_json::json!([
            {
                "type": "text",
                "text": "Running"
            },
            {
                "type": "tool_use",
                "name": "Shell",
                "input": {
                    "command": "echo hi",
                    "block_until_ms": 120_000
                }
            }
        ]);
        let agent_kv_event = serde_json::json!({
            "role": "assistant",
            "content": [
                {
                    "type": "text",
                    "data": "Running",
                    "providerOptions": {
                        "cursor": {
                            "modelName": "composer-2.5"
                        }
                    }
                },
                {
                    "type": "tool-call",
                    "toolName": "Shell",
                    "args": {
                        "block_until_ms": 120_000,
                        "command": "echo hi"
                    }
                }
            ]
        });
        let raw = serde_json::to_vec(&agent_kv_event).unwrap();
        let (signature, model) = parse_cursor_agent_kv_model_signature(&raw).unwrap();

        assert_eq!(
            Some(signature),
            cursor_response_signature(&transcript_content)
        );
        assert_eq!(model, "composer-2.5");
    }

    #[test]
    fn cursor_parser_does_not_reuse_a_previous_reply_model() {
        let path = temp_jsonl_path("cursor-model-reset");
        let content = r#"{"role":"user","message":{"content":"Prompt"}}
{"role":"assistant","message":{"content":[{"type":"text","text":"Known reply"}]}}
{"role":"assistant","message":{"content":[{"type":"image","data":"omitted"}]}}
"#;
        fs::write(&path, content).unwrap();
        let signature = cursor_response_signature(
            &serde_json::json!([{"type": "text", "text": "Known reply"}]),
        )
        .unwrap();
        let model_mappings = HashMap::from([(signature, "composer-2.5".to_string())]);

        let entries =
            parse_cursor_session_file(&path, &model_mappings, &HashSet::new(), &HashMap::new())
                .unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].entry.model.as_deref(), Some("composer-2.5"));
        assert!(entries[0].model_signature.is_some());
        assert_eq!(entries[1].entry.model.as_deref(), Some("Unknown Model"));
        assert!(entries[1].model_signature.is_none());
    }

    #[test]
    fn cursor_session_metadata_treats_non_agent_modes_as_ide() {
        let (_, false_overrides_agent_mode) = parse_cursor_session_metadata(
            "composerData:false-overrides-agent",
            br#"{
                "composerId": "false-overrides-agent",
                "unifiedMode": "agent",
                "isAgentic": false
            }"#,
        )
        .unwrap();
        let (_, non_agent_mode_overrides_true) = parse_cursor_session_metadata(
            "composerData:chat-overrides-true",
            br#"{
                "composerId": "chat-overrides-true",
                "unifiedMode": "chat",
                "isAgentic": true
            }"#,
        )
        .unwrap();
        let (_, agent_mode) = parse_cursor_session_metadata(
            "composerData:agent",
            br#"{
                "composerId": "agent",
                "unifiedMode": "agent",
                "isAgentic": true
            }"#,
        )
        .unwrap();

        assert_eq!(false_overrides_agent_mode.mode.as_deref(), Some("ide"));
        assert_eq!(non_agent_mode_overrides_true.mode.as_deref(), Some("ide"));
        assert_eq!(agent_mode.mode.as_deref(), Some("agent"));
    }

    #[test]
    fn cursor_session_metadata_uses_only_concrete_model_configs() {
        let (_, concrete_model) = parse_cursor_session_metadata(
            "composerData:concrete-model",
            br#"{
                "composerId": "concrete-model",
                "unifiedMode": "agent",
                "modelConfig": { "modelName": "composer-2.5" }
            }"#,
        )
        .unwrap();
        let (_, default_model) = parse_cursor_session_metadata(
            "composerData:default-model",
            br#"{
                "composerId": "default-model",
                "unifiedMode": "agent",
                "modelConfig": { "modelName": "default" }
            }"#,
        )
        .unwrap();

        assert_eq!(concrete_model.model.as_deref(), Some("composer-2.5"));
        assert!(default_model.model.is_none());
    }

    #[test]
    fn cursor_state_db_reader_observes_uncheckpointed_wal() {
        let state_db_path = temp_jsonl_path("cursor-state-wal").with_extension("vscdb");
        let writer = Connection::open(&state_db_path).unwrap();
        let journal_mode: String = writer
            .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
            .unwrap();
        assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
        writer
            .execute_batch(
                "PRAGMA wal_autocheckpoint = 0;
                 CREATE TABLE cursorDiskKV (
                    key TEXT PRIMARY KEY,
                    value BLOB
                 );
                 INSERT INTO cursorDiskKV (key, value)
                 VALUES ('agentKv:blob:test', X'7B7D');",
            )
            .unwrap();

        let wal_path = PathBuf::from(format!("{}-wal", state_db_path.to_string_lossy()));
        assert!(wal_path.exists(), "fixture must keep committed data in WAL");

        let reader = open_cursor_state_db(&state_db_path).unwrap();
        let row_count: i64 = reader
            .query_row("SELECT COUNT(*) FROM cursorDiskKV", [], |row| row.get(0))
            .unwrap();
        assert_eq!(row_count, 1, "read-only connection must observe WAL data");

        drop(reader);
        drop(writer);
        let _ = fs::remove_file(&state_db_path);
        let _ = fs::remove_file(wal_path);
        let _ = fs::remove_file(format!("{}-shm", state_db_path.to_string_lossy()));
    }

    #[test]
    fn cursor_cache_token_migration_marks_legacy_values_unknown() {
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, timestamp, date, session_id, turn_no,
                tokens_cache_read, tokens_cache_write,
                tokens_cache_write_5m, tokens_cache_write_1h,
                delta_cache_read, delta_cache_write,
                delta_cache_write_5m, delta_cache_write_1h
             ) VALUES (
                'cursor', '2026-07-24T00:00:00Z', '2026-07-24',
                'cursor-cache-session', 1, 0, 0, 0, 0, 0, 0, 0, 0
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, timestamp, date, session_id, turn_no,
                tokens_cache_read, tokens_cache_write,
                tokens_cache_write_5m, tokens_cache_write_1h,
                delta_cache_read, delta_cache_write,
                delta_cache_write_5m, delta_cache_write_1h
             ) VALUES (
                'cursor', '2026-07-24T00:01:00Z', '2026-07-24',
                'cursor-cache-measured', 1, 10, 20, 30, 40, 1, 2, 3, 4
             )",
            [],
        )
        .unwrap();

        run_cursor_cache_tokens_unknown_migration(&mut conn).unwrap();
        run_cursor_cache_tokens_unknown_migration(&mut conn).unwrap();

        let values: [Option<u64>; 8] = conn
            .query_row(
                "SELECT
                    tokens_cache_read, tokens_cache_write,
                    tokens_cache_write_5m, tokens_cache_write_1h,
                    delta_cache_read, delta_cache_write,
                    delta_cache_write_5m, delta_cache_write_1h
                 FROM usage_entries
                 WHERE session_id = 'cursor-cache-session'",
                [],
                |row| {
                    Ok([
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ])
                },
            )
            .unwrap();
        let migration_count: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_state WHERE filename = ?",
                params![CURSOR_CACHE_TOKENS_UNKNOWN_MIGRATION_KEY],
                |row| row.get(0),
            )
            .unwrap();
        let measured_values: [u64; 8] = conn
            .query_row(
                "SELECT
                    tokens_cache_read, tokens_cache_write,
                    tokens_cache_write_5m, tokens_cache_write_1h,
                    delta_cache_read, delta_cache_write,
                    delta_cache_write_5m, delta_cache_write_1h
                 FROM usage_entries
                 WHERE session_id = 'cursor-cache-measured'",
                [],
                |row| {
                    Ok([
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ])
                },
            )
            .unwrap();

        assert_eq!(values, [None; 8]);
        assert_eq!(measured_values, [10, 20, 30, 40, 1, 2, 3, 4]);
        assert_eq!(migration_count, 1);
    }

    #[test]
    fn cursor_sync_backfills_plain_text_model_and_rejects_ambiguous_mapping() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_state_db = std::env::var("CURSOR_STATE_DB").ok();
        let unique = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "cursor-agent-kv-sync-{}-{}",
            std::process::id(),
            unique
        ));
        let transcript_dir = root
            .join("projects")
            .join("workspace")
            .join("agent-transcripts")
            .join("session-model");
        fs::create_dir_all(&transcript_dir).unwrap();
        let transcript_path = transcript_dir.join("session-model.jsonl");
        fs::write(
            &transcript_path,
            concat!(
                "{\"role\":\"user\",\"message\":{\"content\":[",
                "{\"type\":\"text\",\"text\":\"",
                "<timestamp>Friday, Jul 24, 2026, 9:00 AM (UTC+8)</timestamp>",
                "<user_query>Inspect the project</user_query>\"}]}}\n",
                "{\"role\":\"assistant\",\"message\":{\"content\":[",
                "{\"type\":\"text\",\"text\":\"Plain answer\"}]}}\n"
            ),
        )
        .unwrap();

        let state_db_path = root.join("state.vscdb");
        let state_conn = Connection::open(&state_db_path).unwrap();
        state_conn
            .execute(
                "CREATE TABLE cursorDiskKV (
                    key TEXT UNIQUE ON CONFLICT REPLACE,
                    value BLOB
                )",
                [],
            )
            .unwrap();
        state_conn
            .execute(
                "INSERT INTO cursorDiskKV (key, value) VALUES (?, ?)",
                params![
                    "composerData:session-model",
                    r#"{
                        "composerId": "session-model",
                        "workspaceIdentifier": {
                            "uri": {
                                "fsPath": "/tmp/project"
                            }
                        },
                        "unifiedMode": "agent",
                        "isAgentic": false,
                        "modelConfig": {
                            "modelName": "composer-2"
                        }
                    }"#
                ],
            )
            .unwrap();
        drop(state_conn);
        std::env::set_var("CURSOR_STATE_DB", &state_db_path);

        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        sync_cursor_usage_logs(&mut conn, &root).unwrap();
        let initial: (
            String,
            String,
            String,
            String,
            Option<u64>,
            Option<u64>,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT model, cwd, source_kind, date,
                        tokens_cache_read, tokens_cache_write, model_signature
                 FROM usage_entries
                 WHERE assistant_type = 'cursor'
                   AND session_id = 'session-model'",
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

        let state_conn = Connection::open(&state_db_path).unwrap();
        state_conn
            .execute(
                "INSERT INTO cursorDiskKV (key, value) VALUES (?, ?)",
                params![
                    "agentKv:blob:plain-text-model",
                    r#"{
                        "role": "assistant",
                        "content": [
                            {
                                "type": "text",
                                "data": "Plain answer",
                                "providerOptions": {
                                    "cursor": {
                                        "modelName": "composer-2.5"
                                    }
                                }
                            }
                        ]
                    }"#
                ],
            )
            .unwrap();
        drop(state_conn);
        sync_cursor_usage_logs(&mut conn, &root).unwrap();
        let matched_model: String = conn
            .query_row(
                "SELECT model FROM usage_entries
                 WHERE assistant_type = 'cursor'
                   AND session_id = 'session-model'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        let state_conn = Connection::open(&state_db_path).unwrap();
        state_conn
            .execute(
                "INSERT INTO cursorDiskKV (key, value) VALUES (?, ?)",
                params![
                    "composerData:session-model",
                    r#"{
                        "composerId": "session-model",
                        "unifiedMode": "agent",
                        "isAgentic": true
                    }"#
                ],
            )
            .unwrap();
        drop(state_conn);
        sync_cursor_usage_logs(&mut conn, &root).unwrap();
        let mode_only_update: (String, String) = conn
            .query_row(
                "SELECT cwd, source_kind
                 FROM usage_entries
                 WHERE assistant_type = 'cursor'
                   AND session_id = 'session-model'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();

        let state_conn = Connection::open(&state_db_path).unwrap();
        state_conn
            .execute(
                "INSERT INTO cursorDiskKV (key, value) VALUES (?, ?)",
                params![
                    "composerData:session-model",
                    r#"{
                        "composerId": "session-model",
                        "workspaceIdentifier": {
                            "uri": {
                                "fsPath": "/tmp/updated-project"
                            }
                        }
                    }"#
                ],
            )
            .unwrap();
        drop(state_conn);
        sync_cursor_usage_logs(&mut conn, &root).unwrap();
        let cwd_only_update: (String, String) = conn
            .query_row(
                "SELECT cwd, source_kind
                 FROM usage_entries
                 WHERE assistant_type = 'cursor'
                   AND session_id = 'session-model'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();

        let state_conn = Connection::open(&state_db_path).unwrap();
        state_conn
            .execute(
                "INSERT INTO cursorDiskKV (key, value) VALUES (?, ?)",
                params![
                    "agentKv:blob:ambiguous-plain-text-model",
                    r#"{
                        "role": "assistant",
                        "content": [
                            {
                                "type": "text",
                                "data": "Plain answer",
                                "providerOptions": {
                                    "cursor": {
                                        "modelName": "cursor-grok-4.5-high-fast"
                                    }
                                }
                            }
                        ]
                    }"#
                ],
            )
            .unwrap();
        drop(state_conn);
        sync_cursor_usage_logs(&mut conn, &root).unwrap();
        let (ambiguous_model, is_ambiguous): (String, bool) = conn
            .query_row(
                "SELECT usage.model, signatures.is_ambiguous
                 FROM usage_entries usage
                 JOIN cursor_model_signatures signatures
                   ON signatures.signature = usage.model_signature
                 WHERE usage.assistant_type = 'cursor'
                   AND usage.session_id = 'session-model'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();

        let state_conn = Connection::open(&state_db_path).unwrap();
        state_conn.execute("DELETE FROM cursorDiskKV", []).unwrap();
        state_conn
            .execute(
                "INSERT INTO cursorDiskKV (key, value) VALUES (?, ?)",
                params![
                    "composerData:session-model",
                    r#"{
                        "composerId": "session-model",
                        "workspaceIdentifier": {
                            "uri": {
                                "fsPath": "/tmp/replaced-project"
                            }
                        },
                        "unifiedMode": "agent",
                        "isAgentic": true
                    }"#
                ],
            )
            .unwrap();
        drop(state_conn);
        sync_cursor_usage_logs(&mut conn, &root).unwrap();
        let reset_state: (String, String, String, u64) = conn
            .query_row(
                "SELECT usage.model, usage.cwd, usage.source_kind,
                        (SELECT COUNT(*) FROM cursor_model_signatures)
                 FROM usage_entries usage
                 WHERE usage.assistant_type = 'cursor'
                   AND usage.session_id = 'session-model'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();

        if let Some(value) = old_state_db {
            std::env::set_var("CURSOR_STATE_DB", value);
        } else {
            std::env::remove_var("CURSOR_STATE_DB");
        }
        let _ = fs::remove_dir_all(&root);

        assert_eq!(initial.0, "composer-2");
        assert_eq!(initial.1, "/tmp/project");
        assert_eq!(initial.2, CURSOR_IDE_SOURCE_KIND);
        assert_eq!(initial.3, "2026-07-24");
        assert_eq!(initial.4, None);
        assert_eq!(initial.5, None);
        assert!(initial.6.is_some());
        assert_eq!(matched_model, "composer-2.5");
        assert_eq!(ambiguous_model, "Unknown Model");
        assert!(is_ambiguous);
        assert_eq!(
            mode_only_update,
            (
                "/tmp/project".to_string(),
                CURSOR_AGENT_SOURCE_KIND.to_string()
            )
        );
        assert_eq!(
            cwd_only_update,
            (
                "/tmp/updated-project".to_string(),
                CURSOR_AGENT_SOURCE_KIND.to_string()
            )
        );
        assert_eq!(
            reset_state,
            (
                "Unknown Model".to_string(),
                "/tmp/replaced-project".to_string(),
                CURSOR_AGENT_SOURCE_KIND.to_string(),
                0
            )
        );
    }

    #[test]
    fn test_parse_cursor_timestamp() {
        let ts = "Wednesday, Jul 8, 2026, 2:24 AM (UTC+8)";
        let parsed = parse_cursor_timestamp(ts);
        assert_eq!(parsed, "2026-07-08T02:24:00+08:00");
        assert_eq!(cursor_date_from_timestamp(&parsed), Some("2026-07-08"));
        assert_eq!(cursor_date_from_timestamp("unknown"), None);
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
    fn codex_source_kind_migration_resets_active_and_archived_state_once() {
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        for key in [
            "codex:sessions/2026/07/active.jsonl",
            r"codex:sessions\2026\07\active.jsonl",
            "codex:archived_sessions/archived.jsonl",
            r"codex:archived_sessions\archived.jsonl",
            "codex:claude:legacy.jsonl",
        ] {
            conn.execute(
                "INSERT INTO sync_state (filename, last_synced_size, last_synced_time)
                 VALUES (?, 10, 0)",
                params![key],
            )
            .unwrap();
        }

        run_codex_source_kind_migration(&mut conn).unwrap();

        let codex_transcript_states: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_state
                 WHERE filename LIKE 'codex:sessions%'
                    OR filename LIKE 'codex:archived_sessions%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let unrelated_state: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_state
                 WHERE filename = 'codex:claude:legacy.jsonl'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(codex_transcript_states, 0);
        assert_eq!(unrelated_state, 1);

        conn.execute(
            "INSERT INTO sync_state (filename, last_synced_size, last_synced_time)
             VALUES ('codex:archived_sessions/new.jsonl', 10, 0)",
            [],
        )
        .unwrap();
        run_codex_source_kind_migration(&mut conn).unwrap();
        let state_after_second_run: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_state
                 WHERE filename = 'codex:archived_sessions/new.jsonl'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state_after_second_run, 1);
    }

    #[test]
    fn init_db_indexes_assistant_transcript_path_lookups() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        let mut index_columns = conn
            .prepare("PRAGMA index_info('idx_assistant_transcript_path')")
            .unwrap();
        let columns: Vec<String> = index_columns
            .query_map([], |row| row.get(2))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(columns, ["assistant_type", "transcript_path"]);

        let mut query_plan = conn
            .prepare(
                "EXPLAIN QUERY PLAN
                 SELECT EXISTS(
                    SELECT 1 FROM usage_entries
                    WHERE assistant_type = 'codex' AND transcript_path = ?
                 )",
            )
            .unwrap();
        let details: Vec<String> = query_plan
            .query_map(["/tmp/session.jsonl"], |row| row.get(3))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(
            details
                .iter()
                .any(|detail| detail.contains("idx_assistant_transcript_path")),
            "查詢計畫未使用 transcript 路徑索引：{details:?}"
        );
    }

    #[test]
    fn codex_sync_skips_unchanged_empty_transcripts() {
        assert!(codex_transcript_needs_sync(10, None, false));
        assert!(codex_transcript_needs_sync(10, Some((9, 0)), true));
        assert!(codex_transcript_needs_sync(10, Some((10, 0)), false));
        assert!(!codex_transcript_needs_sync(10, Some((10, 0)), true));
        assert!(!codex_transcript_needs_sync(
            10,
            Some((10, CODEX_EMPTY_TRANSCRIPT_SYNC_TIME)),
            false
        ));
    }

    #[test]
    fn windows_codex_transcript_paths_keep_original_values_for_indexed_deletion() {
        let stored_path =
            "C:/USERS/RUNNER/APPDATA/LOCAL/TEMP/CODEX/SESSIONS/SESSION.JSONL".to_string();
        let current_path = r"c:\users\runner\appdata\local\temp\codex\sessions\session.jsonl";

        let grouped_paths = group_codex_transcript_paths([stored_path.clone()], true);
        let current_key = codex_transcript_path_key_for_platform(current_path, true);

        assert_eq!(grouped_paths.get(&current_key), Some(&vec![stored_path]));
    }

    #[test]
    fn sync_codex_usage_logs_records_empty_transcript_state() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_codex_dir = std::env::var("CODEX_DIR").ok();
        let mut codex_dir = std::env::temp_dir();
        let unique = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        codex_dir.push(format!(
            "codex-empty-sync-{}-{}",
            std::process::id(),
            unique
        ));

        let sessions_dir = codex_dir.join("sessions/2026/07/26");
        fs::create_dir_all(&sessions_dir).unwrap();
        let transcript_path = sessions_dir.join("empty-session.jsonl");
        let content = r#"{"timestamp":"2026-07-26T10:00:00Z","type":"session_meta","payload":{"id":"empty-session","session_id":"empty-session","originator":"Codex Desktop"}}"#;
        fs::write(&transcript_path, content).unwrap();
        std::env::set_var("CODEX_DIR", &codex_dir);

        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        sync_codex_usage_logs(&mut conn).unwrap();

        let state_key = format!(
            "codex:{}",
            portable_relative_path(&codex_dir, &transcript_path)
        );
        let state: (u64, i64) = conn
            .query_row(
                "SELECT last_synced_size, last_synced_time
                 FROM sync_state
                 WHERE filename = ?",
                params![state_key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();

        if let Some(value) = old_codex_dir {
            std::env::set_var("CODEX_DIR", value);
        } else {
            std::env::remove_var("CODEX_DIR");
        }
        let _ = fs::remove_dir_all(&codex_dir);

        assert_eq!(state.0, content.len() as u64);
        assert_eq!(state.1, CODEX_EMPTY_TRANSCRIPT_SYNC_TIME);
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
                    agent_id TEXT,
                    initiator TEXT,
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

    #[test]
    fn sync_copilot_app_usage_logs_populates_cwd_from_session_store() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_app_dir = std::env::var("COPILOT_APP_DIR").ok();

        let base_dir = temp_jsonl_path("copilot-app-cwd").with_extension("");
        let app_dir = base_dir.join("copilot-app");
        fs::create_dir_all(&app_dir).unwrap();

        let session_store = Connection::open(app_dir.join("session-store.db")).unwrap();
        session_store
            .execute(
                "CREATE TABLE assistant_usage_events (
                    id INTEGER PRIMARY KEY, session_id TEXT, turn_index INTEGER,
                    model TEXT, agent_id TEXT, initiator TEXT,
                    input_tokens INTEGER, output_tokens INTEGER,
                    cache_read_tokens INTEGER, cache_write_tokens INTEGER,
                    reasoning_tokens INTEGER, duration_ms INTEGER,
                    reasoning_effort TEXT, created_at TEXT
                 )",
                [],
            )
            .unwrap();
        session_store
            .execute(
                "CREATE TABLE sessions (
                    id TEXT PRIMARY KEY, cwd TEXT, repository TEXT,
                    host_type TEXT, branch TEXT, summary TEXT,
                    created_at TEXT, updated_at TEXT
                 )",
                [],
            )
            .unwrap();
        let session_id = "app-cwd-session";
        session_store
            .execute(
                "INSERT INTO sessions (id, cwd) VALUES (?, '/Users/test/app-project')",
                params![session_id],
            )
            .unwrap();
        session_store
            .execute(
                "INSERT INTO assistant_usage_events
                    (id, session_id, turn_index, model, agent_id, initiator,
                     input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
                     reasoning_tokens, duration_ms, reasoning_effort, created_at)
                 VALUES (1, ?, 0, 'gpt-5', NULL, NULL, 100, 10, 0, 0, 0, 100, 'medium', '2026-07-20 10:00:00')",
                params![session_id],
            )
            .unwrap();

        let data_db = Connection::open(app_dir.join("data.db")).unwrap();
        data_db
            .execute(
                "CREATE TABLE sessions (id TEXT PRIMARY KEY, title TEXT)",
                [],
            )
            .unwrap();
        data_db
            .execute(
                "INSERT INTO sessions (id, title) VALUES (?, 'App CWD Test')",
                params![session_id],
            )
            .unwrap();

        std::env::set_var("COPILOT_APP_DIR", &app_dir);
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        sync_copilot_app_usage_logs(&mut conn).unwrap();

        let cwd: Option<String> = conn
            .query_row(
                "SELECT cwd FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-app'
                   AND session_id = ?",
                params![session_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            cwd.as_deref(),
            Some("/Users/test/app-project"),
            "App row must have CWD from session-store.db.sessions"
        );

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
                    agent_id TEXT,
                    initiator TEXT,
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

        create_copilot_app_registry_from_events(&app_dir);
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
                        agent_id TEXT,
                        initiator TEXT,
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

        create_copilot_app_registry_from_events(&app_dir_a);
        // Sync from A first; this establishes a cursor at 2026-07-20 10:00:00.
        std::env::set_var("COPILOT_APP_DIR", &app_dir_a);
        sync_copilot_app_usage_logs(&mut conn).unwrap();

        // Switch to B. A correct scoped cursor must NOT reuse A's cursor; B's
        // earlier event must still be ingested.
        create_copilot_app_registry_from_events(&app_dir_b);
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
                    agent_id TEXT,
                    initiator TEXT,
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

        create_copilot_app_registry_from_events(&app_dir);
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
                    agent_id TEXT,
                    initiator TEXT,
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
        create_copilot_app_registry_from_events(&app_dir);
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
                    agent_id TEXT,
                    initiator TEXT,
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

        create_copilot_app_registry_from_events(&app_dir);
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
                    agent_id TEXT,
                    initiator TEXT,
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

        create_copilot_app_registry_from_events(&app_dir);
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
                    agent_id TEXT,
                    initiator TEXT,
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

        create_copilot_app_registry_from_events(&app_dir);
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
                    agent_id TEXT,
                    initiator TEXT,
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

        create_copilot_app_registry_from_events(&app_dir);
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

        // import_source_id must include source, session, turn, agent, and model.
        assert!(
            import_source_id.starts_with("copilot-app:"),
            "import_source_id must start with copilot-app: prefix, got: {}",
            import_source_id
        );
        let rest = &import_source_id["copilot-app:".len()..];
        // The source key is the first colon-delimited component after the
        // prefix and must be non-empty hex.
        let hex_segment = rest.split(':').next().unwrap_or("");
        assert!(
            !hex_segment.is_empty() && hex_segment.chars().all(|c| c.is_ascii_hexdigit()),
            "import_source_id must include a non-empty hex source key, got: {}",
            import_source_id
        );
        let expected_suffix = format!(":sess-x:0:main:{}", encode_hex(b"gpt-5"));
        assert!(
            import_source_id.ends_with(&expected_suffix),
            "import_source_id must end with session, turn, agent and model identity, got: {}",
            import_source_id
        );

        if let Some(value) = old_app_dir {
            std::env::set_var("COPILOT_APP_DIR", value);
        } else {
            std::env::remove_var("COPILOT_APP_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    /// Verify that a Copilot App turn with both a main agent (NULL agent_id,
    /// DP4F) and a subagent (non-null agent_id, K2.7) produces two distinct
    /// usage rows, each with its own model and token totals, and that their
    /// token sums equal the raw event totals.
    #[test]
    fn sync_copilot_app_usage_logs_splits_main_agent_and_subagent() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_app_dir = std::env::var("COPILOT_APP_DIR").ok();

        let base_dir = temp_jsonl_path("copilot-app-subagent-split").with_extension("");
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
                    agent_id TEXT,
                    initiator TEXT,
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

        let session_id = "74b6d236-d311-4675-9855-fee91bc508e5";
        let agent_id = "call_v4b32z66";
        // Main agent: 2 events, DP4F.
        session_store
            .execute(
                "INSERT INTO assistant_usage_events
                    (id, session_id, turn_index, model, agent_id, initiator,
                     input_tokens, output_tokens,
                     cache_read_tokens, cache_write_tokens,
                     reasoning_tokens, duration_ms,
                     reasoning_effort, created_at)
                 VALUES
                    (1, ?, 0, 'DP4F', NULL, NULL, 100000, 800, 0, 0, 1500, 500, 'medium', '2026-07-21 02:59:16'),
                    (2, ?, 0, 'DP4F', NULL, NULL, 1261894, 17622, 0, 0, 88, 1500, 'medium', '2026-07-21 03:01:00')",
                params![session_id, session_id],
            )
            .unwrap();
        // Subagent: 2 events, K2.7.
        session_store
            .execute(
                "INSERT INTO assistant_usage_events
                    (id, session_id, turn_index, model, agent_id, initiator,
                     input_tokens, output_tokens,
                     cache_read_tokens, cache_write_tokens,
                     reasoning_tokens, duration_ms,
                     reasoning_effort, created_at)
                 VALUES
                    (3, ?, 0, 'K2.7', ?, 'sub-agent', 2000000, 20000, 0, 0, 100, 2000, NULL, '2026-07-21 02:59:20'),
                    (4, ?, 0, 'K2.7', ?, 'sub-agent', 1156615, 8069, 0, 0, 97, 2000, NULL, '2026-07-21 03:05:00')",
                params![session_id, agent_id, session_id, agent_id],
            )
            .unwrap();

        std::env::set_var("COPILOT_APP_DIR", &app_dir);

        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        create_copilot_app_registry(&app_dir, &[session_id]);
        sync_copilot_app_usage_logs(&mut conn).unwrap();

        let main_row: (i64, i64, Option<String>, Option<String>, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT tokens_total, delta_total, model, parent_session_id, agent_nickname, agent_role
                 FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-app'
                   AND session_id = ? AND turn_no = 1",
                params![session_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            main_row.2.as_deref(),
            Some("DP4F"),
            "main agent model must be DP4F"
        );
        assert!(
            main_row.3.is_none(),
            "main agent must have no parent_session_id"
        );
        assert!(
            main_row.4.is_none(),
            "main agent must have no agent_nickname"
        );
        assert!(
            main_row.5.is_none(),
            "main agent agent_role must be NULL (initiator is NULL)"
        );
        // DP4F events: input 100000+1261894=1361894, output 800+17622=18422, reasoning 1500+88=1588
        let dp4f_total = 1361894 + 18422 + 1588;
        assert_eq!(
            main_row.0, dp4f_total,
            "main agent total must match DP4F event sum"
        );
        assert_eq!(
            main_row.1, dp4f_total,
            "main agent delta_total must match DP4F event sum"
        );

        let synthetic_id = format!("{}__{}", session_id, agent_id);
        let sub_row: (i64, i64, Option<String>, Option<String>, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT tokens_total, delta_total, model, parent_session_id, agent_nickname, agent_role
                 FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-app'
                   AND session_id = ? AND turn_no = 1",
                params![synthetic_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            sub_row.2.as_deref(),
            Some("K2.7"),
            "subagent model must be K2.7"
        );
        assert_eq!(
            sub_row.3.as_deref(),
            Some(session_id),
            "subagent parent_session_id must be the main session id"
        );
        assert_eq!(
            sub_row.4.as_deref(),
            Some(agent_id),
            "subagent agent_nickname must be the agent_id"
        );
        assert_eq!(
            sub_row.5.as_deref(),
            Some("sub-agent"),
            "subagent agent_role must be 'sub-agent' when initiator is 'sub-agent'"
        );
        // K2.7 events: input 2000000+1156615=3156615, output 20000+8069=28069, reasoning 100+97=197
        let k27_total = 3156615 + 28069 + 197;
        assert_eq!(
            sub_row.0, k27_total,
            "subagent total must match K2.7 event sum"
        );
        assert_eq!(
            sub_row.1, k27_total,
            "subagent delta_total must match K2.7 event sum"
        );

        // Sum of the two rows equals the raw event total (no double counting).
        assert_eq!(
            main_row.0 + sub_row.0,
            dp4f_total + k27_total,
            "split row totals must sum to raw event totals"
        );

        if let Some(value) = old_app_dir {
            std::env::set_var("COPILOT_APP_DIR", value);
        } else {
            std::env::remove_var("COPILOT_APP_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    #[test]
    fn sync_copilot_app_usage_logs_preserves_multiple_models_for_same_agent_turn() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_app_dir = std::env::var("COPILOT_APP_DIR").ok();

        let base_dir = temp_jsonl_path("copilot-app-multi-model").with_extension("");
        let app_dir = base_dir.join("copilot-app");
        fs::create_dir_all(&app_dir).unwrap();
        let session_id = "app-multi-model-session";
        let store = create_test_copilot_session_store(&app_dir);
        for (id, model, input, output) in [(1i64, "gpt-5", 100i64, 10i64), (2, "claude-4", 200, 20)]
        {
            store
                .execute(
                    "INSERT INTO assistant_usage_events (
                        id, session_id, turn_index, model, agent_id, initiator,
                        input_tokens, output_tokens, cache_read_tokens,
                        cache_write_tokens, reasoning_tokens, duration_ms,
                        reasoning_effort, created_at
                     ) VALUES (?, ?, 0, ?, 'call_same', 'sub-agent', ?, ?, 0, 0, 0, 10, NULL, ?)",
                    params![
                        id,
                        session_id,
                        model,
                        input,
                        output,
                        format!("2026-07-22T10:00:0{id}")
                    ],
                )
                .unwrap();
        }
        create_copilot_app_registry(&app_dir, &[session_id]);
        drop(store);

        std::env::set_var("COPILOT_APP_DIR", &app_dir);
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        sync_copilot_app_usage_logs(&mut conn).unwrap();

        let rows: Vec<(String, i64)> = conn
            .prepare(
                "SELECT model, tokens_total
                 FROM usage_entries
                 WHERE assistant_type = 'copilot'
                   AND source_kind = 'copilot-app'
                   AND session_id = ?
                 ORDER BY model",
            )
            .unwrap()
            .query_map([format!("{session_id}__call_same")], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        assert_eq!(
            rows,
            vec![("claude-4".to_string(), 220), ("gpt-5".to_string(), 110)],
            "同一 App Agent／Turn 的每個模型都必須保留獨立用量"
        );
        let source_key = test_copilot_source_key(&app_dir);
        let timeline_rows = get_session_turns_token_stats(
            &conn,
            "copilot",
            &format!("{session_id}__call_same"),
            Some("copilot-app"),
            Some(&source_key),
        )
        .unwrap();
        let (timeline_tokens, timeline_models) = timeline_rows.get(&1).unwrap();
        assert_eq!(
            timeline_tokens.total, 330,
            "timeline 必須彙總同回合的所有模型"
        );
        assert!(timeline_models.contains("gpt-5"));
        assert!(timeline_models.contains("claude-4"));

        if let Some(value) = old_app_dir {
            std::env::set_var("COPILOT_APP_DIR", value);
        } else {
            std::env::remove_var("COPILOT_APP_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    /// Verify re-syncing the same events does not duplicate rows (idempotency)
    /// and that adding new events for an existing turn upserts correctly.
    #[test]
    fn sync_copilot_app_usage_logs_subagent_idempotent_and_upserts() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_app_dir = std::env::var("COPILOT_APP_DIR").ok();

        let base_dir = temp_jsonl_path("copilot-app-subagent-idem").with_extension("");
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
                    agent_id TEXT,
                    initiator TEXT,
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

        let session_id = "split-sess";
        let agent_id = "call_abc";
        session_store
            .execute(
                "INSERT INTO assistant_usage_events
                    (id, session_id, turn_index, model, agent_id, initiator,
                     input_tokens, output_tokens,
                     cache_read_tokens, cache_write_tokens,
                     reasoning_tokens, duration_ms,
                     reasoning_effort, created_at)
                 VALUES
                    (1, ?, 0, 'DP4F', NULL, NULL, 100, 10, 0, 0, 0, 100, 'medium', '2026-07-20 10:00:00'),
                    (2, ?, 0, 'K2.7', ?, 'sub-agent', 200, 20, 0, 0, 0, 200, NULL, '2026-07-20 10:00:05')",
                params![session_id, session_id, agent_id],
            )
            .unwrap();

        std::env::set_var("COPILOT_APP_DIR", &app_dir);

        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        create_copilot_app_registry(&app_dir, &[session_id]);

        sync_copilot_app_usage_logs(&mut conn).unwrap();
        let count_after_first: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-app'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            count_after_first, 2,
            "first sync must produce 2 rows (main + subagent)"
        );

        // Re-sync: no new events → no new rows.
        sync_copilot_app_usage_logs(&mut conn).unwrap();
        let count_after_second: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-app'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            count_after_second, 2,
            "re-sync without new events must not duplicate rows"
        );

        // Add a new event to the subagent (same turn) → upsert should update
        // the subagent row's totals, not create a third row.
        session_store
            .execute(
                "INSERT INTO assistant_usage_events
                    (id, session_id, turn_index, model, agent_id, initiator,
                     input_tokens, output_tokens,
                     cache_read_tokens, cache_write_tokens,
                     reasoning_tokens, duration_ms,
                     reasoning_effort, created_at)
                 VALUES (3, ?, 0, 'K2.7', ?, 'sub-agent', 50, 5, 0, 0, 0, 50, NULL, '2026-07-20 10:00:10')",
                params![session_id, agent_id],
            )
            .unwrap();
        sync_copilot_app_usage_logs(&mut conn).unwrap();
        let count_after_add: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-app'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            count_after_add, 2,
            "adding subagent events must upsert, not add a row"
        );

        let synthetic_id = format!("{}__{}", session_id, agent_id);
        let sub_total: i64 = conn
            .query_row(
                "SELECT tokens_total FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-app'
                   AND session_id = ?",
                params![synthetic_id],
                |row| row.get(0),
            )
            .unwrap();
        // 200+20 + 50+5 = 275
        assert_eq!(sub_total, 275, "subagent totals must reflect added events");

        if let Some(value) = old_app_dir {
            std::env::set_var("COPILOT_APP_DIR", value);
        } else {
            std::env::remove_var("COPILOT_APP_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    /// Verify that Copilot App subagent rows whose `initiator` is NULL or an
    /// unknown value get `agent_role = NULL` (never guessed), while a subagent
    /// with `initiator = 'sub-agent'` gets `agent_role = 'sub-agent'`. The main
    /// agent row always has `agent_role = NULL`. This matches the Copilot CLI
    /// collector semantics so the same subagent produces identical metadata
    /// across App and CLI.
    #[test]
    fn sync_copilot_app_usage_logs_agent_role_follows_initiator() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_app_dir = std::env::var("COPILOT_APP_DIR").ok();

        let base_dir = temp_jsonl_path("copilot-app-agent-role").with_extension("");
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
                    agent_id TEXT,
                    initiator TEXT,
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

        let session_id = "role-sess";
        // Main agent: initiator NULL → agent_role NULL.
        // Subagent A: initiator 'sub-agent' → agent_role 'sub-agent'.
        // Subagent B: initiator NULL → agent_role NULL (do not guess).
        // Subagent C: initiator 'worker' (unknown) → agent_role NULL (do not guess).
        session_store
            .execute(
                "INSERT INTO assistant_usage_events
                    (id, session_id, turn_index, model, agent_id, initiator,
                     input_tokens, output_tokens,
                     cache_read_tokens, cache_write_tokens,
                     reasoning_tokens, duration_ms,
                     reasoning_effort, created_at)
                 VALUES
                    (1, ?, 0, 'DP4F', NULL, NULL, 100, 10, 0, 0, 0, 100, 'medium', '2026-07-22 10:00:00'),
                    (2, ?, 0, 'K2.7', ?, 'sub-agent', 200, 20, 0, 0, 0, 200, NULL, '2026-07-22 10:00:01'),
                    (3, ?, 0, 'K2.7', ?, NULL, 300, 30, 0, 0, 0, 300, NULL, '2026-07-22 10:00:02'),
                    (4, ?, 0, 'K2.7', ?, 'worker', 400, 40, 0, 0, 0, 400, NULL, '2026-07-22 10:00:03')",
                params![
                    session_id,
                    session_id, "call_sub",
                    session_id, "call_null",
                    session_id, "call_worker"
                ],
            )
            .unwrap();

        std::env::set_var("COPILOT_APP_DIR", &app_dir);

        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        create_copilot_app_registry(&app_dir, &[session_id]);
        sync_copilot_app_usage_logs(&mut conn).unwrap();

        let role_of = |agent: &str| -> Option<String> {
            let synthetic = format!("{}__{}", session_id, agent);
            conn.query_row(
                "SELECT agent_role FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-app'
                   AND session_id = ? AND turn_no = 1",
                params![synthetic],
                |row| row.get(0),
            )
            .unwrap()
        };

        let main_role: Option<String> = conn
            .query_row(
                "SELECT agent_role FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-app'
                   AND session_id = ? AND turn_no = 1 AND parent_session_id IS NULL",
                params![session_id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(main_role.is_none(), "main agent agent_role must be NULL");

        assert_eq!(
            role_of("call_sub").as_deref(),
            Some("sub-agent"),
            "subagent with initiator='sub-agent' must get agent_role='sub-agent'"
        );
        assert_eq!(
            role_of("call_null"),
            None::<String>,
            "subagent with NULL initiator must keep agent_role NULL (no guessing)"
        );
        assert_eq!(
            role_of("call_worker"),
            None::<String>,
            "subagent with unknown initiator='worker' must keep agent_role NULL (no guessing)"
        );

        if let Some(value) = old_app_dir {
            std::env::set_var("COPILOT_APP_DIR", value);
        } else {
            std::env::remove_var("COPILOT_APP_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    /// Verify that Copilot App and Copilot CLI produce identical subagent
    /// metadata semantics: parent_session_id = main session id,
    /// agent_nickname = agent_id, agent_role = 'sub-agent' only when the source
    /// explicitly provides initiator='sub-agent'. Both collectors must not
    /// guess a role (e.g. 'worker') for Copilot subagents.
    #[test]
    fn copilot_app_and_cli_subagent_metadata_consistent() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_dir = std::env::var("COPILOT_DIR").ok();
        let old_app_dir = std::env::var("COPILOT_APP_DIR").ok();

        let base_dir = temp_jsonl_path("copilot-app-cli-consistency").with_extension("");
        let app_dir = base_dir.join("copilot");
        fs::create_dir_all(&app_dir).unwrap();

        // CLI fixture: a CLI-classified session with one main + one subagent.
        let cli_session = "consistency-cli-sess";
        let cli_agent = "call_cli_sub";
        let store = build_cli_reconciler_fixture(&app_dir, &[], &[cli_session]);
        insert_cli_event(
            &store,
            CliEventIdentity {
                id: 1,
                session_id: cli_session,
                model: "DP4P",
                agent_id: None,
                initiator: None,
            },
            CliEventTokens {
                input: 100,
                output: 10,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
                duration_ms: 50,
            },
            "2026-07-22T10:00:00",
        );
        insert_cli_event(
            &store,
            CliEventIdentity {
                id: 2,
                session_id: cli_session,
                model: "DP4P",
                agent_id: Some(cli_agent),
                initiator: Some("sub-agent"),
            },
            CliEventTokens {
                input: 200,
                output: 20,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
                duration_ms: 60,
            },
            "2026-07-22T10:00:01",
        );

        // App fixture: an App-registry session with one main + one subagent.
        let app_session = "consistency-app-sess";
        let app_agent = "call_app_sub";
        create_copilot_app_registry(&app_dir, &[app_session]);
        store
            .execute(
                "INSERT INTO assistant_usage_events
                    (id, session_id, turn_index, model, agent_id, initiator,
                     input_tokens, output_tokens,
                     cache_read_tokens, cache_write_tokens,
                     reasoning_tokens, duration_ms,
                     reasoning_effort, created_at)
                 VALUES
                    (3, ?, 0, 'DP4F', NULL, NULL, 100, 10, 0, 0, 0, 100, 'medium', '2026-07-22 10:00:00'),
                    (4, ?, 0, 'K2.7', ?, 'sub-agent', 200, 20, 0, 0, 0, 200, NULL, '2026-07-22 10:00:01')",
                params![app_session, app_session, app_agent],
            )
            .unwrap();

        std::env::set_var("COPILOT_DIR", &app_dir);
        std::env::set_var("COPILOT_APP_DIR", &app_dir);

        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        seed_cli_hook_row(&conn, cli_session, 330);

        sync_copilot_cli_agent_usage_logs(&mut conn).unwrap();
        sync_copilot_app_usage_logs(&mut conn).unwrap();

        let read_subagent = |session: &str,
                             agent: &str,
                             source_kind: &str|
         -> (String, Option<String>, Option<String>, Option<String>) {
            let synthetic = format!("{}__{}", session, agent);
            conn.query_row(
                "SELECT session_id, parent_session_id, agent_nickname, agent_role
                 FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = ?
                   AND session_id = ? AND turn_no = 1",
                params![source_kind, synthetic],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap()
        };

        let cli_sub = read_subagent(cli_session, cli_agent, "copilot-cli");
        let app_sub = read_subagent(app_session, app_agent, "copilot-app");

        // Synthetic session id format must match across collectors.
        assert_eq!(
            cli_sub.0,
            format!("{}__{}", cli_session, cli_agent),
            "CLI synthetic session id format"
        );
        assert_eq!(
            app_sub.0,
            format!("{}__{}", app_session, app_agent),
            "App synthetic session id format"
        );
        // parent_session_id = main session id.
        assert_eq!(cli_sub.1.as_deref(), Some(cli_session));
        assert_eq!(app_sub.1.as_deref(), Some(app_session));
        // agent_nickname = agent_id.
        assert_eq!(cli_sub.2.as_deref(), Some(cli_agent));
        assert_eq!(app_sub.2.as_deref(), Some(app_agent));
        // agent_role = 'sub-agent' only when initiator='sub-agent'; never 'worker'.
        assert_eq!(cli_sub.3.as_deref(), Some("sub-agent"));
        assert_eq!(app_sub.3.as_deref(), Some("sub-agent"));

        if let Some(value) = old_dir {
            std::env::set_var("COPILOT_DIR", value);
        } else {
            std::env::remove_var("COPILOT_DIR");
        }
        if let Some(value) = old_app_dir {
            std::env::set_var("COPILOT_APP_DIR", value);
        } else {
            std::env::remove_var("COPILOT_APP_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    /// Verify that re-syncing an existing Copilot App session whose subagent
    /// rows were previously written with agent_role=NULL upgrades them to
    /// agent_role='sub-agent' when the source events carry
    /// initiator='sub-agent'. This is the one-time backfill path for existing
    /// App rows, exercised by the normal INSERT OR REPLACE upsert: no duplicate
    /// rows are created.
    #[test]
    fn sync_copilot_app_usage_logs_backfills_null_agent_role_on_resync() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_app_dir = std::env::var("COPILOT_APP_DIR").ok();

        let base_dir = temp_jsonl_path("copilot-app-role-backfill").with_extension("");
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
                    agent_id TEXT,
                    initiator TEXT,
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

        let session_id = "backfill-sess";
        let agent_id = "call_backfill";
        session_store
            .execute(
                "INSERT INTO assistant_usage_events
                    (id, session_id, turn_index, model, agent_id, initiator,
                     input_tokens, output_tokens,
                     cache_read_tokens, cache_write_tokens,
                     reasoning_tokens, duration_ms,
                     reasoning_effort, created_at)
                 VALUES
                    (1, ?, 0, 'DP4F', NULL, NULL, 100, 10, 0, 0, 0, 100, 'medium', '2026-07-22 10:00:00'),
                    (2, ?, 0, 'K2.7', ?, 'sub-agent', 200, 20, 0, 0, 0, 200, NULL, '2026-07-22 10:00:01')",
                params![session_id, session_id, agent_id],
            )
            .unwrap();

        std::env::set_var("COPILOT_APP_DIR", &app_dir);

        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        let source_key = test_copilot_source_key(&app_dir);
        create_copilot_app_registry(&app_dir, &[session_id]);

        // Simulate a pre-existing App subagent row written by the old collector
        // with agent_role=NULL (the regression we are fixing). Same
        // import_source_id as the new collector would produce so the upsert
        // targets the same row instead of creating a duplicate.
        let synthetic_id = format!("{}__{}", session_id, agent_id);
        let import_source_id = format!("copilot-app:{}:{}:0:{}", source_key, session_id, agent_id);
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, source_dir_key, timestamp, date,
                session_id, turn_no, import_source_id, model,
                tokens_total, tokens_input, tokens_output, delta_total, delta_input, delta_output,
                parent_session_id, agent_nickname, agent_role
             ) VALUES (
                'copilot', 'copilot-app', ?, '2026-07-22T10:00:01Z', '2026-07-22',
                ?, 1, ?, 'K2.7',
                220, 200, 20, 220, 200, 20,
                ?, ?, NULL
             )",
            params![
                source_key,
                synthetic_id,
                import_source_id,
                session_id,
                agent_id
            ],
        )
        .unwrap();

        sync_copilot_app_usage_logs(&mut conn).unwrap();

        // No duplicate rows: exactly one subagent row for this synthetic id.
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-app'
                   AND session_id = ?",
                params![synthetic_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 1,
            "resync must upsert, not duplicate the subagent row"
        );

        // The upserted row now carries agent_role='sub-agent'.
        let role: Option<String> = conn
            .query_row(
                "SELECT agent_role FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-app'
                   AND session_id = ?",
                params![synthetic_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            role.as_deref(),
            Some("sub-agent"),
            "backfilled subagent row must carry agent_role='sub-agent'"
        );

        if let Some(value) = old_app_dir {
            std::env::set_var("COPILOT_APP_DIR", value);
        } else {
            std::env::remove_var("COPILOT_APP_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    /// Verify that a legacy merged Copilot App row (old 3-segment
    /// import_source_id) is removed when the same session is re-synced by the
    /// new collector, and that keyed rows for other sessions are not
    /// touched. The cleanup happens during `sync_copilot_app_usage_logs`, not
    /// in `init_db`, so it runs whenever a session is re-aggregated.
    #[test]
    fn sync_copilot_app_usage_logs_cleans_legacy_merged_rows_on_resync() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_app_dir = std::env::var("COPILOT_APP_DIR").ok();

        let base_dir = temp_jsonl_path("copilot-app-legacy-cleanup").with_extension("");
        let app_dir = base_dir.join("copilot-app");
        fs::create_dir_all(&app_dir).unwrap();

        // Build a session-store with one event for `reconcile-sess`.
        let session_store = Connection::open(app_dir.join("session-store.db")).unwrap();
        session_store
            .execute(
                "CREATE TABLE assistant_usage_events (
                    id INTEGER PRIMARY KEY,
                    session_id TEXT,
                    turn_index INTEGER,
                    model TEXT,
                    agent_id TEXT,
                    initiator TEXT,
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
                    (id, session_id, turn_index, model, agent_id, initiator,
                     input_tokens, output_tokens,
                     cache_read_tokens, cache_write_tokens,
                     reasoning_tokens, duration_ms,
                     reasoning_effort, created_at)
                 VALUES (1, 'reconcile-sess', 0, 'DP4F', NULL, NULL, 100, 10, 0, 0, 0, 100, 'medium', '2026-07-20 10:00:00')",
                [],
            )
            .unwrap();

        std::env::set_var("COPILOT_APP_DIR", &app_dir);

        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        let source_key = test_copilot_source_key(&app_dir);

        // Pre-seed a legacy merged row (3-segment import_source_id) for the
        // session that will be re-synced, plus a keyed row for a
        // different session that must NOT be touched.
        let legacy_id = format!("copilot-app:{}:reconcile-sess:0", source_key);
        let other_id = format!("copilot-app:{}:other-sess:0:main", source_key);
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, source_dir_key, timestamp, date,
                session_id, turn_no, import_source_id, model, tokens_total, tokens_input, tokens_output, delta_total, delta_input, delta_output
             ) VALUES (
                'copilot', 'copilot-app', ?, '2026-07-20T10:00:00Z', '2026-07-20',
                'reconcile-sess', 1, ?, 'DP4F', 110, 100, 10, 110, 100, 10
             )",
            params![source_key, legacy_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, source_dir_key, timestamp, date,
                session_id, turn_no, import_source_id, model, tokens_total, tokens_input, tokens_output, delta_total, delta_input, delta_output
             ) VALUES (
                'copilot', 'copilot-app', ?, '2026-07-20T10:00:00Z', '2026-07-20',
                'other-sess', 1, ?, 'DP4F', 110, 100, 10, 110, 100, 10
             )",
            params![source_key, other_id],
        )
        .unwrap();

        create_copilot_app_registry(&app_dir, &["reconcile-sess", "other-sess"]);
        sync_copilot_app_usage_logs(&mut conn).unwrap();

        // Legacy merged row for the synced session must be gone.
        let legacy_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries
                 WHERE source_kind = 'copilot-app' AND source_dir_key = ?
                   AND session_id = 'reconcile-sess'
                   AND import_source_id = ?",
                params![source_key, legacy_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            legacy_count, 0,
            "legacy merged 3-segment row must be deleted on re-sync"
        );

        // The new agent-and-model keyed row for the synced session must exist.
        let expected_new_id = format!(
            "copilot-app:{}:reconcile-sess:0:main:{}",
            source_key,
            encode_hex(b"DP4F")
        );
        let new_row_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries
                 WHERE source_kind = 'copilot-app' AND source_dir_key = ?
                   AND session_id = 'reconcile-sess'
                   AND import_source_id = ?",
                params![source_key, expected_new_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            new_row_count, 1,
            "new agent-and-model keyed row must be written for the synced session"
        );

        // The other session's keyed row must NOT be deleted.
        let other_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries
                 WHERE source_kind = 'copilot-app' AND source_dir_key = ?
                   AND session_id = 'other-sess'",
                params![source_key],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(other_count, 1, "other session's new row must be untouched");

        if let Some(value) = old_app_dir {
            std::env::set_var("COPILOT_APP_DIR", value);
        } else {
            std::env::remove_var("COPILOT_APP_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    /// Verify that Copilot CLI rows and other source kinds are not affected by
    /// the subagent split cleanup (which only targets copilot-app rows with a
    /// non-null source_dir_key and a 3-segment import_source_id during sync).
    #[test]
    fn sync_copilot_app_usage_logs_subagent_cleanup_preserves_other_sources() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_app_dir = std::env::var("COPILOT_APP_DIR").ok();

        let base_dir = temp_jsonl_path("copilot-app-preserve").with_extension("");
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
                    agent_id TEXT,
                    initiator TEXT,
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
                    (id, session_id, turn_index, model, agent_id, initiator,
                     input_tokens, output_tokens,
                     cache_read_tokens, cache_write_tokens,
                     reasoning_tokens, duration_ms,
                     reasoning_effort, created_at)
                 VALUES (1, 'app-sess', 0, 'DP4F', NULL, NULL, 100, 10, 0, 0, 0, 100, 'medium', '2026-07-20 10:00:00')",
                [],
            )
            .unwrap();

        std::env::set_var("COPILOT_APP_DIR", &app_dir);

        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        // Pre-seed cross-source rows that must survive the Copilot App sync.
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, source_dir_key, timestamp, date,
                session_id, turn_no, import_source_id, model, tokens_total, tokens_input, tokens_output, delta_total, delta_input, delta_output
             ) VALUES (
                'copilot', 'copilot-cli', NULL, '2026-07-01T10:00:00Z', '2026-07-01',
                'cli-sess', 1, 'copilot-cli:cli-sess:0', 'gpt-5', 110, 100, 10, 110, 100, 10
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, source_dir_key, timestamp, date,
                session_id, turn_no, import_source_id, model, tokens_total, tokens_input, tokens_output, delta_total, delta_input, delta_output
             ) VALUES (
                'codex', 'legacy', 'abc123', '2026-07-01T10:00:00Z', '2026-07-01',
                'codex-sess', 1, 'copilot-app:abc123:codex-sess:0', 'gpt-5', 110, 100, 10, 110, 100, 10
             )",
            [],
        )
        .unwrap();

        create_copilot_app_registry(&app_dir, &["app-sess"]);
        sync_copilot_app_usage_logs(&mut conn).unwrap();

        let cli_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries WHERE session_id = 'cli-sess'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(cli_count, 1, "Copilot CLI row must survive");

        let codex_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries WHERE session_id = 'codex-sess'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            codex_count, 1,
            "Codex row must survive even with a copilot-app-looking id"
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
                        agent_id TEXT,
                        initiator TEXT,
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

        create_copilot_app_registry_from_events(&app_dir_a);
        // Sync A first.
        std::env::set_var("COPILOT_APP_DIR", &app_dir_a);
        sync_copilot_app_usage_logs(&mut conn).unwrap();

        // Sync B (same session_id, same turn). Must NOT overwrite A's row.
        create_copilot_app_registry_from_events(&app_dir_b);
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

    #[test]
    fn copilot_app_registry_excludes_cli_and_unknown_events_without_stalling_cursor() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_app_dir = std::env::var("COPILOT_APP_DIR").ok();
        let base_dir = temp_jsonl_path("copilot-app-registry-classification").with_extension("");
        let app_dir = base_dir.join("copilot-app");
        fs::create_dir_all(app_dir.join("session-state").join("cli-session")).unwrap();
        let store = create_test_copilot_session_store(&app_dir);
        insert_test_copilot_event(&store, 1, "app-session", 0, "2026-07-20 10:00:00");
        insert_test_copilot_event(&store, 2, "cli-session", 0, "2026-07-20 10:00:01");
        insert_test_copilot_event(&store, 3, "unknown-session", 0, "2026-07-20 10:00:02");
        fs::write(
            app_dir
                .join("session-state")
                .join("cli-session")
                .join("events.jsonl"),
            "{}\n",
        )
        .unwrap();
        create_copilot_app_registry(&app_dir, &["app-session"]);
        std::env::set_var("COPILOT_APP_DIR", &app_dir);

        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, timestamp, date, session_id, turn_no,
                tokens_input, tokens_output, tokens_total
             ) VALUES ('copilot', 'copilot-cli', '2026-07-20T10:00:01Z', '2026-07-20',
                       'cli-session', 1, 100, 10, 110)",
            [],
        )
        .unwrap();

        sync_copilot_app_usage_logs(&mut conn).unwrap();

        let app_sessions: Vec<String> = conn
            .prepare(
                "SELECT session_id FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-app'",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        assert_eq!(app_sessions, vec!["app-session"]);
        let cli_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-cli'
                   AND session_id = 'cli-session'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(cli_rows, 1);
        let cursor: String = conn
            .query_row(
                "SELECT filename FROM sync_state
                 WHERE filename LIKE 'sync:copilot_app:cursor:%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(cursor.ends_with("::2026-07-20 10:00:02::3"));

        if let Some(value) = old_app_dir {
            std::env::set_var("COPILOT_APP_DIR", value);
        } else {
            std::env::remove_var("COPILOT_APP_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    #[test]
    fn copilot_app_reconciliation_removes_stale_rows_but_keeps_cli_rows() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_app_dir = std::env::var("COPILOT_APP_DIR").ok();
        let base_dir = temp_jsonl_path("copilot-app-reconciliation-cleanup").with_extension("");
        let app_dir = base_dir.join("copilot-app");
        fs::create_dir_all(&app_dir).unwrap();
        let store = create_test_copilot_session_store(&app_dir);
        insert_test_copilot_event(&store, 1, "valid-app", 0, "2026-07-20 10:00:00");
        create_copilot_app_registry(&app_dir, &["valid-app"]);
        std::env::set_var("COPILOT_APP_DIR", &app_dir);

        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        let source_key = test_copilot_source_key(&app_dir);
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, source_dir_key, timestamp, date,
                session_id, turn_no, tokens_input, tokens_output, tokens_total,
                import_source_id
             ) VALUES
                ('copilot', 'copilot-app', ?, '2026-07-20T09:00:00Z', '2026-07-20',
                 'stale-session', 1, 20, 2, 22, 'stale-app-row'),
                ('copilot', 'copilot-cli', NULL, '2026-07-20T09:00:00Z', '2026-07-20',
                 'stale-session', 1, 30, 3, 33, 'stale-cli-row')",
            params![source_key],
        )
        .unwrap();

        sync_copilot_app_usage_logs(&mut conn).unwrap();

        let stale_app_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries
                 WHERE source_kind = 'copilot-app' AND source_dir_key = ?
                   AND session_id = 'stale-session'",
                params![source_key],
                |row| row.get(0),
            )
            .unwrap();
        let stale_cli_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries
                 WHERE source_kind = 'copilot-cli' AND session_id = 'stale-session'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let valid_app_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries
                 WHERE source_kind = 'copilot-app' AND session_id = 'valid-app'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stale_app_rows, 0);
        assert_eq!(stale_cli_rows, 1);
        assert_eq!(valid_app_rows, 1);

        if let Some(value) = old_app_dir {
            std::env::set_var("COPILOT_APP_DIR", value);
        } else {
            std::env::remove_var("COPILOT_APP_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    #[test]
    fn copilot_app_reconciliation_imports_history_after_registry_arrives() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_app_dir = std::env::var("COPILOT_APP_DIR").ok();
        let base_dir = temp_jsonl_path("copilot-app-registry-late").with_extension("");
        let app_dir = base_dir.join("copilot-app");
        fs::create_dir_all(&app_dir).unwrap();
        let store = create_test_copilot_session_store(&app_dir);
        insert_test_copilot_event(&store, 1, "late-app", 0, "2026-07-20 10:00:00");
        create_copilot_app_registry(&app_dir, &[]);
        std::env::set_var("COPILOT_APP_DIR", &app_dir);

        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        sync_copilot_app_usage_logs(&mut conn).unwrap();
        let cursor_before: String = conn
            .query_row(
                "SELECT filename FROM sync_state
                 WHERE filename LIKE 'sync:copilot_app:cursor:%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM usage_entries WHERE source_kind = 'copilot-app'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0
        );

        create_copilot_app_registry(&app_dir, &["late-app"]);
        sync_copilot_app_usage_logs(&mut conn).unwrap();

        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM usage_entries
                 WHERE source_kind = 'copilot-app' AND session_id = 'late-app'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            1
        );
        let cursor_after: String = conn
            .query_row(
                "SELECT filename FROM sync_state
                 WHERE filename LIKE 'sync:copilot_app:cursor:%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(cursor_after, cursor_before);

        if let Some(value) = old_app_dir {
            std::env::set_var("COPILOT_APP_DIR", value);
        } else {
            std::env::remove_var("COPILOT_APP_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    #[test]
    fn copilot_app_sync_skips_when_data_db_is_invalid() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_app_dir = std::env::var("COPILOT_APP_DIR").ok();
        let base_dir = temp_jsonl_path("copilot-app-invalid-data-db").with_extension("");
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        for (name, data_db_kind) in [
            ("missing", 0u8),
            ("missing-table", 1u8),
            ("unopenable", 2u8),
        ] {
            let app_dir = base_dir.join(name);
            fs::create_dir_all(&app_dir).unwrap();
            let store = create_test_copilot_session_store(&app_dir);
            insert_test_copilot_event(&store, 1, "unsafe-session", 0, "2026-07-20 10:00:00");
            drop(store);
            let data_db_path = app_dir.join("data.db");
            match data_db_kind {
                0 => {}
                1 => {
                    Connection::open(&data_db_path).unwrap();
                }
                2 => {
                    fs::create_dir(&data_db_path).unwrap();
                }
                _ => unreachable!(),
            }
            std::env::set_var("COPILOT_APP_DIR", &app_dir);
            sync_copilot_app_usage_logs(&mut conn).unwrap();
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM usage_entries WHERE source_kind = 'copilot-app'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
                0
            );
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM sync_state
                     WHERE filename LIKE 'sync:copilot_app:cursor:%'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
                0
            );
        }

        if let Some(value) = old_app_dir {
            std::env::set_var("COPILOT_APP_DIR", value);
        } else {
            std::env::remove_var("COPILOT_APP_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    #[test]
    fn copilot_app_noop_sync_preserves_row_identity_content_and_cursor() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_app_dir = std::env::var("COPILOT_APP_DIR").ok();
        let base_dir = temp_jsonl_path("copilot-app-noop").with_extension("");
        let app_dir = base_dir.join("copilot-app");
        fs::create_dir_all(&app_dir).unwrap();
        let store = create_test_copilot_session_store(&app_dir);
        insert_test_copilot_event(&store, 1, "stable-session", 0, "2026-07-20 10:00:00");
        create_copilot_app_registry(&app_dir, &["stable-session"]);
        std::env::set_var("COPILOT_APP_DIR", &app_dir);

        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        sync_copilot_app_usage_logs(&mut conn).unwrap();
        let first_row: (i64, i64, i64) = conn
            .query_row(
                "SELECT id, tokens_input, tokens_total FROM usage_entries
                 WHERE source_kind = 'copilot-app' AND session_id = 'stable-session'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        let first_cursor: String = conn
            .query_row(
                "SELECT filename FROM sync_state
                 WHERE filename LIKE 'sync:copilot_app:cursor:%'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        sync_copilot_app_usage_logs(&mut conn).unwrap();
        let second_row: (i64, i64, i64) = conn
            .query_row(
                "SELECT id, tokens_input, tokens_total FROM usage_entries
                 WHERE source_kind = 'copilot-app' AND session_id = 'stable-session'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        let second_cursor: String = conn
            .query_row(
                "SELECT filename FROM sync_state
                 WHERE filename LIKE 'sync:copilot_app:cursor:%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(second_row, first_row);
        assert_eq!(second_cursor, first_cursor);

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
                        agent_id TEXT,
                        initiator TEXT,
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

        create_copilot_app_registry_from_events(&app_dir_a);
        create_copilot_app_registry_from_events(&app_dir_b);
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

    // =========================================================================
    // Copilot CLI agent reconciliation tests
    // =========================================================================
    //
    // The CLI reconciler reads `~/.copilot/session-store.db` (resolved via
    // `get_copilot_dir()` / `COPILOT_DIR`) and splits merged `copilot-cli`
    // hook rows into per-agent rows. These fixtures build a temp copilot dir,
    // a session-store.db with `assistant_usage_events`, a `data.db` App
    // registry (to classify sessions as CLI), an `events.jsonl` transcript (to
    // satisfy `CopilotAppSessionKind::Cli`), and seed hook rows for total
    // validation. They never touch the real `~/.copilot`.

    /// Build a CLI reconciliation fixture: a temp copilot dir with
    /// `session-store.db` (assistant_usage_events), `data.db` (sessions
    /// registry listing only `app_session_ids`), and `events.jsonl`
    /// transcripts for each `cli_session_id` so they classify as CLI.
    fn build_cli_reconciler_fixture(
        app_dir: &Path,
        app_session_ids: &[&str],
        cli_session_ids: &[&str],
    ) -> Connection {
        let session_store = create_test_copilot_session_store(app_dir);
        // App registry: only App sessions are listed. CLI sessions are absent,
        // so they classify as CLI (transcript exists) — not App.
        create_copilot_app_registry(app_dir, app_session_ids);
        // Create CLI transcripts so classify_copilot_app_session returns Cli.
        for sid in cli_session_ids {
            let dir = app_dir.join("session-state").join(sid);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("events.jsonl"), "").unwrap();
        }
        session_store
    }

    /// Token + duration parameters for a CLI assistant_usage_event fixture row.
    struct CliEventTokens {
        input: i64,
        output: i64,
        cache_read: i64,
        cache_write: i64,
        reasoning: i64,
        duration_ms: i64,
    }

    /// Identity parameters for a CLI assistant_usage_event fixture row.
    struct CliEventIdentity<'a> {
        id: i64,
        session_id: &'a str,
        model: &'a str,
        agent_id: Option<&'a str>,
        initiator: Option<&'a str>,
    }

    /// Insert a CLI assistant_usage_event row.
    fn insert_cli_event(
        store: &Connection,
        identity: CliEventIdentity,
        tokens: CliEventTokens,
        created_at: &str,
    ) {
        store
            .execute(
                "INSERT INTO assistant_usage_events
                    (id, session_id, turn_index, model, agent_id, initiator,
                     input_tokens, output_tokens,
                     cache_read_tokens, cache_write_tokens,
                     reasoning_tokens, duration_ms,
                     reasoning_effort, created_at)
                 VALUES (?, ?, 0, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, ?)",
                params![
                    identity.id,
                    identity.session_id,
                    identity.model,
                    identity.agent_id,
                    identity.initiator,
                    tokens.input,
                    tokens.output,
                    tokens.cache_read,
                    tokens.cache_write,
                    tokens.reasoning,
                    tokens.duration_ms,
                    created_at
                ],
            )
            .unwrap();
    }

    /// Seed a merged `copilot-cli` hook row for a session with a cumulative
    /// total, mirroring what `sync_hook_usage_logs` would have written.
    fn seed_cli_hook_row(conn: &Connection, session_id: &str, tokens_total: i64) {
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, timestamp, date, session_id, session_name,
                turn_no, model, tokens_input, tokens_output, tokens_total,
                delta_input, delta_output, delta_total
             ) VALUES (
                'copilot', 'copilot-cli', '2026-07-22T10:00:00Z', '2026-07-22', ?,
                'Test Session', 1, 'DP4P', 0, 0, ?, 0, 0, ?
             )",
            params![session_id, tokens_total, tokens_total],
        )
        .unwrap();
    }

    /// Per-agent CLI row read from `usage_entries` for assertion.
    struct CliAgentRow {
        session_id: String,
        model: Option<String>,
        parent_session_id: Option<String>,
        tokens_total: i64,
        agent_nickname: Option<String>,
        agent_role: Option<String>,
        tokens_input: i64,
        tokens_output: i64,
        tokens_reasoning: i64,
        tokens_cache_read: i64,
        cwd: Option<String>,
    }

    /// Read the per-agent rows for a CLI session.
    fn read_cli_agent_rows(conn: &Connection, session_id: &str) -> Vec<CliAgentRow> {
        let mut stmt = conn
            .prepare(
                "SELECT session_id, model, parent_session_id, tokens_total,
                        agent_nickname, agent_role,
                        tokens_input, tokens_output, tokens_reasoning,
                        tokens_cache_read, cwd
                 FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-cli'
                   AND (session_id = ? OR session_id LIKE ? ESCAPE '\\')
                 ORDER BY session_id ASC",
            )
            .unwrap();
        let rows = stmt
            .query_map(params![session_id, format!("{}\\__%", session_id)], |row| {
                Ok(CliAgentRow {
                    session_id: row.get(0)?,
                    model: row.get(1)?,
                    parent_session_id: row.get(2)?,
                    tokens_total: row.get(3)?,
                    agent_nickname: row.get(4)?,
                    agent_role: row.get(5)?,
                    tokens_input: row.get(6)?,
                    tokens_output: row.get(7)?,
                    tokens_reasoning: row.get(8)?,
                    tokens_cache_read: row.get(9)?,
                    cwd: row.get(10)?,
                })
            })
            .unwrap();
        rows.map(|r| r.unwrap()).collect()
    }

    /// Returns (main_total, sub1_total, sub2_total) for the validation session
    /// fixture described in the task: main DP4P total 281490, subagent
    /// call_f14xiouf DP4P total 398911, subagent call_2y5obibr DP4P total
    /// 314861, combined 995262. `with_reasoning` controls whether reasoning
    /// tokens are included (they must NOT alter the totals).
    fn build_validation_fixture_events(store: &Connection, session_id: &str) {
        // Main agent: 11 calls, DP4P. input 271753, output 9737, reasoning
        // 1272 -> total 281490 (input includes cache read; use cache_read 0 so
        // net input = raw input, total invariant preserved).
        insert_cli_event(
            store,
            CliEventIdentity {
                id: 1,
                session_id,
                model: "DP4P",
                agent_id: None,
                initiator: None,
            },
            CliEventTokens {
                input: 271753,
                output: 9737,
                cache_read: 0,
                cache_write: 0,
                reasoning: 1272,
                duration_ms: 100,
            },
            "2026-07-22T10:00:00",
        );
        // Subagent call_f14xiouf: 13 calls, DP4P. input 383928, output 14983,
        // reasoning 11060 -> total 398911.
        insert_cli_event(
            store,
            CliEventIdentity {
                id: 2,
                session_id,
                model: "DP4P",
                agent_id: Some("call_f14xiouf"),
                initiator: Some("sub-agent"),
            },
            CliEventTokens {
                input: 383928,
                output: 14983,
                cache_read: 0,
                cache_write: 0,
                reasoning: 11060,
                duration_ms: 200,
            },
            "2026-07-22T10:00:01",
        );
        // Subagent call_2y5obibr: 11 calls, DP4P. input 305638, output 9223,
        // reasoning 660 -> total 314861.
        insert_cli_event(
            store,
            CliEventIdentity {
                id: 3,
                session_id,
                model: "DP4P",
                agent_id: Some("call_2y5obibr"),
                initiator: Some("sub-agent"),
            },
            CliEventTokens {
                input: 305638,
                output: 9223,
                cache_read: 0,
                cache_write: 0,
                reasoning: 660,
                duration_ms: 150,
            },
            "2026-07-22T10:00:02",
        );
    }

    #[test]
    fn cli_reconciler_splits_main_and_two_subagents() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_dir = std::env::var("COPILOT_DIR").ok();

        let base_dir = temp_jsonl_path("cli-reconcile-split").with_extension("");
        let app_dir = base_dir.join("copilot");
        fs::create_dir_all(&app_dir).unwrap();
        let session_id = "f33b0404-e2dc-48ff-aa55-25a700b8fa7e";
        let store = build_cli_reconciler_fixture(&app_dir, &[], &[session_id]);
        build_validation_fixture_events(&store, session_id);

        std::env::set_var("COPILOT_DIR", &app_dir);
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        // Seed hook row with the combined total so validation passes.
        seed_cli_hook_row(&conn, session_id, 995262);

        sync_copilot_cli_agent_usage_logs(&mut conn).unwrap();

        let rows = read_cli_agent_rows(&conn, session_id);
        // Exactly 3 rows: main + 2 subagents.
        assert_eq!(rows.len(), 3, "must produce 1 main + 2 subagent rows");

        let main = rows
            .iter()
            .find(|r| r.model.as_deref() == Some("DP4P") && r.agent_nickname.is_none())
            .expect("main agent row exists");
        assert_eq!(main.session_id, session_id);
        assert!(
            main.parent_session_id.is_none(),
            "main parent_session_id must be NULL"
        );
        assert!(
            main.agent_nickname.is_none(),
            "main agent_nickname must be NULL"
        );
        assert_eq!(main.tokens_total, 281490, "main agent total must be 281490");

        let sub1 = rows
            .iter()
            .find(|r| r.agent_nickname.as_deref() == Some("call_f14xiouf"))
            .expect("subagent call_f14xiouf row exists");
        assert_eq!(sub1.session_id, format!("{}__call_f14xiouf", session_id));
        assert_eq!(
            sub1.parent_session_id.as_deref(),
            Some(session_id),
            "parent_session_id"
        );
        assert_eq!(
            sub1.agent_nickname.as_deref(),
            Some("call_f14xiouf"),
            "agent_nickname"
        );
        assert_eq!(sub1.agent_role.as_deref(), Some("sub-agent"), "agent_role");
        assert_eq!(sub1.model.as_deref(), Some("DP4P"), "model from event");
        assert_eq!(sub1.tokens_total, 398911, "subagent total 398911");

        let sub2 = rows
            .iter()
            .find(|r| r.agent_nickname.as_deref() == Some("call_2y5obibr"))
            .expect("subagent call_2y5obibr row exists");
        assert_eq!(sub2.session_id, format!("{}__call_2y5obibr", session_id));
        assert_eq!(sub2.parent_session_id.as_deref(), Some(session_id));
        assert_eq!(sub2.agent_nickname.as_deref(), Some("call_2y5obibr"));
        assert_eq!(sub2.tokens_total, 314861, "subagent total 314861");

        // Combined total invariant.
        assert_eq!(
            main.tokens_total + sub1.tokens_total + sub2.tokens_total,
            995262,
            "combined per-agent total must equal hook session total"
        );

        // Per-agent token breakdown: input, output, reasoning, cache_read,
        // cache_write are preserved in their own columns. cache_read is netted
        // out of input (CLI normalization), and reasoning is stored but NOT
        // included in tokens_total.
        assert_eq!(main.tokens_input, 271753);
        assert_eq!(main.tokens_output, 9737);
        assert_eq!(main.tokens_reasoning, 1272);
        assert_eq!(main.tokens_cache_read, 0);

        assert_eq!(sub1.tokens_input, 383928);
        assert_eq!(sub1.tokens_output, 14983);
        assert_eq!(sub1.tokens_reasoning, 11060);
        assert_eq!(sub1.tokens_cache_read, 0);

        assert_eq!(sub2.tokens_input, 305638);
        assert_eq!(sub2.tokens_output, 9223);
        assert_eq!(sub2.tokens_reasoning, 660);
        assert_eq!(sub2.tokens_cache_read, 0);

        if let Some(value) = old_dir {
            std::env::set_var("COPILOT_DIR", value);
        } else {
            std::env::remove_var("COPILOT_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    #[test]
    fn cli_reconciler_proceeds_when_data_db_missing() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_dir = std::env::var("COPILOT_DIR").ok();

        let base_dir = temp_jsonl_path("cli-reconcile-no-data-db").with_extension("");
        let app_dir = base_dir.join("copilot");
        fs::create_dir_all(&app_dir).unwrap();
        let session_id = "f33b0404-e2dc-48ff-aa55-25a700b8fa7e";
        // CLI-only install: no data.db registry. The transcript still
        // classifies the session as CLI, so reconciliation must proceed with
        // an empty App registry instead of skipping.
        let store = create_test_copilot_session_store(&app_dir);
        let transcript_dir = app_dir.join("session-state").join(session_id);
        fs::create_dir_all(&transcript_dir).unwrap();
        fs::write(transcript_dir.join("events.jsonl"), "").unwrap();
        build_validation_fixture_events(&store, session_id);

        std::env::set_var("COPILOT_DIR", &app_dir);
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        seed_cli_hook_row(&conn, session_id, 995262);

        sync_copilot_cli_agent_usage_logs(&mut conn).unwrap();

        let rows = read_cli_agent_rows(&conn, session_id);
        assert_eq!(
            rows.len(),
            3,
            "reconciliation must run without data.db (1 main + 2 subagent rows)"
        );
        assert_eq!(
            rows.iter().map(|r| r.tokens_total).sum::<i64>(),
            995262,
            "combined per-agent total must equal hook session total"
        );

        if let Some(value) = old_dir {
            std::env::set_var("COPILOT_DIR", value);
        } else {
            std::env::remove_var("COPILOT_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    #[test]
    fn cli_reconciler_populates_cwd_from_sessions_table() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_dir = std::env::var("COPILOT_DIR").ok();

        let base_dir = temp_jsonl_path("cli-reconcile-cwd").with_extension("");
        let app_dir = base_dir.join("copilot");
        fs::create_dir_all(&app_dir).unwrap();
        let session_id = "cwd-test-session";
        let store = build_cli_reconciler_fixture(&app_dir, &[], &[session_id]);
        build_validation_fixture_events(&store, session_id);
        // Seed CWD in the sessions table.
        insert_test_session_cwd(&store, session_id, "/Users/test/project");

        std::env::set_var("COPILOT_DIR", &app_dir);
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        seed_cli_hook_row(&conn, session_id, 995262);

        sync_copilot_cli_agent_usage_logs(&mut conn).unwrap();

        let rows = read_cli_agent_rows(&conn, session_id);
        assert_eq!(rows.len(), 3, "must produce 1 main + 2 subagent rows");
        // Every per-agent row — main and subagents alike — must carry the
        // session's CWD resolved from session-store.db.sessions.
        for row in &rows {
            assert_eq!(
                row.cwd.as_deref(),
                Some("/Users/test/project"),
                "row {:?} must have CWD",
                row.session_id
            );
        }

        if let Some(value) = old_dir {
            std::env::set_var("COPILOT_DIR", value);
        } else {
            std::env::remove_var("COPILOT_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    #[test]
    fn cli_reconciler_cwd_null_when_sessions_table_empty() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_dir = std::env::var("COPILOT_DIR").ok();

        let base_dir = temp_jsonl_path("cli-reconcile-cwd-empty").with_extension("");
        let app_dir = base_dir.join("copilot");
        fs::create_dir_all(&app_dir).unwrap();
        let session_id = "cwd-null-session";
        let store = build_cli_reconciler_fixture(&app_dir, &[], &[session_id]);
        build_validation_fixture_events(&store, session_id);
        // No session row in the sessions table → CWD should remain NULL.

        std::env::set_var("COPILOT_DIR", &app_dir);
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        seed_cli_hook_row(&conn, session_id, 995262);

        sync_copilot_cli_agent_usage_logs(&mut conn).unwrap();

        let rows = read_cli_agent_rows(&conn, session_id);
        assert_eq!(rows.len(), 3);
        for row in &rows {
            assert!(
                row.cwd.is_none(),
                "CWD must be NULL when sessions table has no entry"
            );
        }

        if let Some(value) = old_dir {
            std::env::set_var("COPILOT_DIR", value);
        } else {
            std::env::remove_var("COPILOT_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    #[test]
    fn backfill_copilot_cwd_updates_existing_null_rows() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_dir = std::env::var("COPILOT_DIR").ok();

        let base_dir = temp_jsonl_path("backfill-cwd").with_extension("");
        let app_dir = base_dir.join("copilot");
        fs::create_dir_all(&app_dir).unwrap();

        let store = create_test_copilot_session_store(&app_dir);
        let session_a = "backfill-session-a";
        let session_b = "backfill-session-b";
        insert_test_session_cwd(&store, session_a, "/Users/test/projectA");
        insert_test_session_cwd(&store, session_b, "/Users/test/projectB");

        std::env::set_var("COPILOT_DIR", &app_dir);
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        // Insert existing copilot rows with NULL cwd (simulating pre-fix state).
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, timestamp, date, session_id, session_name,
                turn_no, model, tokens_total, cwd
             ) VALUES
                ('copilot', 'copilot-cli', '2026-07-22T10:00:00Z', '2026-07-22', ?, 'A', 1, 'gpt-5', 100, NULL),
                ('copilot', 'copilot-cli', '2026-07-22T10:01:00Z', '2026-07-22', ?, 'B', 1, 'gpt-5', 200, NULL),
                ('copilot', 'copilot-app', '2026-07-22T10:02:00Z', '2026-07-22', ?, 'A2', 1, 'gpt-5', 300, NULL)",
            params![session_a, session_b, session_a],
        )
        .unwrap();

        // Also insert a row that already has a CWD — must not be overwritten.
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, timestamp, date, session_id, session_name,
                turn_no, model, tokens_total, cwd
             ) VALUES
                ('copilot', 'copilot-cli', '2026-07-22T10:03:00Z', '2026-07-22', ?, 'C', 2, 'gpt-5', 400, '/existing')",
            params![session_a],
        )
        .unwrap();

        backfill_copilot_cwd(&mut conn).unwrap();

        let cwd_a: Vec<Option<String>> = conn
            .prepare("SELECT cwd FROM usage_entries WHERE session_id = ? ORDER BY id")
            .unwrap()
            .query_map(params![session_a], |row| row.get::<_, Option<String>>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(cwd_a.len(), 3);
        // Two NULL rows for session_a are backfilled.
        assert_eq!(cwd_a[0].as_deref(), Some("/Users/test/projectA"));
        assert_eq!(cwd_a[1].as_deref(), Some("/Users/test/projectA"));
        // The pre-existing CWD is preserved.
        assert_eq!(cwd_a[2].as_deref(), Some("/existing"));

        let cwd_b: Option<String> = conn
            .query_row(
                "SELECT cwd FROM usage_entries WHERE session_id = ?",
                params![session_b],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(cwd_b.as_deref(), Some("/Users/test/projectB"));

        // Second run is a no-op (migration marker set).
        backfill_copilot_cwd(&mut conn).unwrap();

        if let Some(value) = old_dir {
            std::env::set_var("COPILOT_DIR", value);
        } else {
            std::env::remove_var("COPILOT_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    #[test]
    fn cli_reconciler_reasoning_not_double_counted() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_dir = std::env::var("COPILOT_DIR").ok();

        let base_dir = temp_jsonl_path("cli-reconcile-reasoning").with_extension("");
        let app_dir = base_dir.join("copilot");
        fs::create_dir_all(&app_dir).unwrap();
        let session_id = "reasoning-session";
        let store = build_cli_reconciler_fixture(&app_dir, &[], &[session_id]);
        // input 1000, output 200, reasoning 100, cache_read 0. CLI accounting
        // total = net_input + output = 1200. reasoning is stored in its own
        // column but NOT added to tokens_total (matching the hook semantics),
        // so it is never double-counted.
        insert_cli_event(
            &store,
            CliEventIdentity {
                id: 1,
                session_id,
                model: "DP4P",
                agent_id: None,
                initiator: None,
            },
            CliEventTokens {
                input: 1000,
                output: 200,
                cache_read: 0,
                cache_write: 0,
                reasoning: 100,
                duration_ms: 50,
            },
            "2026-07-22T10:00:00",
        );

        std::env::set_var("COPILOT_DIR", &app_dir);
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        seed_cli_hook_row(&conn, session_id, 1200);

        sync_copilot_cli_agent_usage_logs(&mut conn).unwrap();

        let row: (i64, i64, i64, i64) = conn
            .query_row(
                "SELECT tokens_total, tokens_input, tokens_output, tokens_reasoning
                 FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-cli'
                   AND session_id = ? AND parent_session_id IS NULL",
                params![session_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            row.0, 1200,
            "total must be net_input + output, reasoning not added"
        );
        assert_eq!(row.1, 1000, "net input preserved");
        assert_eq!(row.2, 200);
        assert_eq!(row.3, 100, "reasoning stored separately");

        if let Some(value) = old_dir {
            std::env::set_var("COPILOT_DIR", value);
        } else {
            std::env::remove_var("COPILOT_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    #[test]
    fn cli_reconciler_counts_cache_read_once_in_total() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_dir = std::env::var("COPILOT_DIR").ok();

        let base_dir = temp_jsonl_path("cli-reconcile-cache-read").with_extension("");
        let app_dir = base_dir.join("copilot");
        fs::create_dir_all(&app_dir).unwrap();
        let session_id = "cache-read-session";
        let store = build_cli_reconciler_fixture(&app_dir, &[], &[session_id]);
        insert_cli_event(
            &store,
            CliEventIdentity {
                id: 1,
                session_id,
                model: "mai-code-1-flash-picker",
                agent_id: None,
                initiator: None,
            },
            CliEventTokens {
                input: 443_554,
                output: 1_370,
                cache_read: 401_024,
                cache_write: 0,
                reasoning: 384,
                duration_ms: 50,
            },
            "2026-07-15T12:39:57",
        );

        std::env::set_var("COPILOT_DIR", &app_dir);
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        seed_cli_hook_row(&conn, session_id, 444_924);

        sync_copilot_cli_agent_usage_logs(&mut conn).unwrap();

        let row: (i64, Option<i64>, Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT tokens_total, tokens_input, tokens_cache_read, import_source_id
                 FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-cli'
                   AND session_id = ? AND parent_session_id IS NULL",
                params![session_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(row.0, 444_924, "total must match raw input plus output");
        assert_eq!(row.1, Some(42_530), "stored input must exclude cache read");
        assert_eq!(row.2, Some(401_024), "cache read must remain explicit");
        assert!(
            row.3
                .as_deref()
                .is_some_and(|id| id.starts_with("copilot-cli-agents:")),
            "hook row must be replaced by the reconciled agent row"
        );

        if let Some(value) = old_dir {
            std::env::set_var("COPILOT_DIR", value);
        } else {
            std::env::remove_var("COPILOT_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    #[test]
    fn cli_reconciler_sums_hook_deltas_across_cumulative_reset() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_dir = std::env::var("COPILOT_DIR").ok();

        let base_dir = temp_jsonl_path("cli-reconcile-hook-reset").with_extension("");
        let app_dir = base_dir.join("copilot");
        fs::create_dir_all(&app_dir).unwrap();
        let session_id = "hook-reset-session";
        let store = build_cli_reconciler_fixture(&app_dir, &[], &[session_id]);
        insert_cli_event(
            &store,
            CliEventIdentity {
                id: 1,
                session_id,
                model: "DP4P",
                agent_id: None,
                initiator: None,
            },
            CliEventTokens {
                input: 200,
                output: 20,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
                duration_ms: 50,
            },
            "2026-07-22T10:00:00",
        );
        insert_cli_event(
            &store,
            CliEventIdentity {
                id: 2,
                session_id,
                model: "DP4P",
                agent_id: None,
                initiator: None,
            },
            CliEventTokens {
                input: 100,
                output: 10,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
                duration_ms: 50,
            },
            "2026-07-22T10:01:00",
        );

        std::env::set_var("COPILOT_DIR", &app_dir);
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        seed_cli_hook_row(&conn, session_id, 220);
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, timestamp, date, session_id, session_name,
                turn_no, model, tokens_input, tokens_output, tokens_total,
                delta_input, delta_output, delta_total
             ) VALUES (
                'copilot', 'copilot-cli', '2026-07-22T10:01:00Z', '2026-07-22', ?,
                'Test Session', 2, 'DP4P', 100, 10, 110, 100, 10, 110
             )",
            [session_id],
        )
        .unwrap();

        sync_copilot_cli_agent_usage_logs(&mut conn).unwrap();

        let row: (i64, i64, Option<String>) = conn
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(tokens_total), 0), MAX(import_source_id)
                 FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-cli'
                   AND (session_id = ? OR session_id LIKE ? ESCAPE '\\')",
                params![session_id, format!("{}\\__%", session_id)],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(row.0, 1, "cumulative hook rows must be replaced");
        assert_eq!(row.1, 330, "hook delta sum must cover both reset segments");
        assert!(row
            .2
            .as_deref()
            .is_some_and(|id| id.starts_with("copilot-cli-agents:")));

        if let Some(value) = old_dir {
            std::env::set_var("COPILOT_DIR", value);
        } else {
            std::env::remove_var("COPILOT_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    #[test]
    fn cli_reconciler_accepts_agent_events_missing_from_hook() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_dir = std::env::var("COPILOT_DIR").ok();

        let base_dir = temp_jsonl_path("cli-reconcile-hook-gap").with_extension("");
        let app_dir = base_dir.join("copilot");
        fs::create_dir_all(&app_dir).unwrap();
        let session_id = "hook-gap-session";
        let store = build_cli_reconciler_fixture(&app_dir, &[], &[session_id]);
        for (id, input, output) in [(1, 100, 10), (2, 200, 20)] {
            insert_cli_event(
                &store,
                CliEventIdentity {
                    id,
                    session_id,
                    model: "DP4P",
                    agent_id: None,
                    initiator: None,
                },
                CliEventTokens {
                    input,
                    output,
                    cache_read: 0,
                    cache_write: 0,
                    reasoning: 0,
                    duration_ms: 50,
                },
                if id == 1 {
                    "2026-07-22T10:00:00"
                } else {
                    "2026-07-22T10:01:00"
                },
            );
        }

        std::env::set_var("COPILOT_DIR", &app_dir);
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        // The hook did not run for the second API call.
        seed_cli_hook_row(&conn, session_id, 110);

        sync_copilot_cli_agent_usage_logs(&mut conn).unwrap();

        let row: (i64, Option<String>) = conn
            .query_row(
                "SELECT tokens_total, import_source_id
                 FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-cli'
                   AND session_id = ? AND parent_session_id IS NULL",
                params![session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(row.0, 330, "agent events are the complete API-call source");
        assert!(row
            .1
            .as_deref()
            .is_some_and(|id| id.starts_with("copilot-cli-agents:")));

        if let Some(value) = old_dir {
            std::env::set_var("COPILOT_DIR", value);
        } else {
            std::env::remove_var("COPILOT_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    #[test]
    fn cli_reconciler_different_models_not_merged() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_dir = std::env::var("COPILOT_DIR").ok();

        let base_dir = temp_jsonl_path("cli-reconcile-models").with_extension("");
        let app_dir = base_dir.join("copilot");
        fs::create_dir_all(&app_dir).unwrap();
        let session_id = "multi-model-session";
        let store = build_cli_reconciler_fixture(&app_dir, &[], &[session_id]);
        // Main agent DP4P; subagent K2.7 (different model). Even though the
        // prompt might have asked for K2.7, the subagent actually used DP4P per
        // the event — but here we test that distinct event models stay distinct
        // rows and the subagent does NOT inherit the main model.
        insert_cli_event(
            &store,
            CliEventIdentity {
                id: 1,
                session_id,
                model: "DP4P",
                agent_id: None,
                initiator: None,
            },
            CliEventTokens {
                input: 100,
                output: 10,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
                duration_ms: 50,
            },
            "2026-07-22T10:00:00",
        );
        insert_cli_event(
            &store,
            CliEventIdentity {
                id: 2,
                session_id,
                model: "K2.7",
                agent_id: Some("call_diff"),
                initiator: Some("sub-agent"),
            },
            CliEventTokens {
                input: 200,
                output: 20,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
                duration_ms: 60,
            },
            "2026-07-22T10:00:01",
        );

        std::env::set_var("COPILOT_DIR", &app_dir);
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        seed_cli_hook_row(&conn, session_id, 330);

        sync_copilot_cli_agent_usage_logs(&mut conn).unwrap();

        let models: Vec<(Option<String>, Option<String>)> = conn
            .prepare(
                "SELECT model, agent_nickname FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-cli'
                   AND (session_id = ? OR session_id LIKE ? ESCAPE '\\')",
            )
            .unwrap()
            .query_map(params![session_id, format!("{}\\__%", session_id)], |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                ))
            })
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        let main_model = models
            .iter()
            .find(|m| m.1.is_none())
            .map(|m| m.0.clone().unwrap())
            .unwrap();
        let sub_model = models
            .iter()
            .find(|m| m.1.as_deref() == Some("call_diff"))
            .map(|m| m.0.clone().unwrap())
            .unwrap();
        assert_eq!(main_model, "DP4P");
        assert_eq!(
            sub_model, "K2.7",
            "subagent model must come from its event, not inherited"
        );

        if let Some(value) = old_dir {
            std::env::set_var("COPILOT_DIR", value);
        } else {
            std::env::remove_var("COPILOT_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    #[test]
    fn cli_reconciler_preserves_multiple_models_for_same_agent() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_dir = std::env::var("COPILOT_DIR").ok();

        let base_dir = temp_jsonl_path("cli-reconcile-same-agent-models").with_extension("");
        let app_dir = base_dir.join("copilot");
        fs::create_dir_all(&app_dir).unwrap();
        let session_id = "cli-same-agent-multi-model";
        let store = build_cli_reconciler_fixture(&app_dir, &[], &[session_id]);
        insert_cli_event(
            &store,
            CliEventIdentity {
                id: 1,
                session_id,
                model: "DP4P",
                agent_id: Some("call_same"),
                initiator: Some("sub-agent"),
            },
            CliEventTokens {
                input: 100,
                output: 10,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
                duration_ms: 50,
            },
            "2026-07-22T10:00:00",
        );
        insert_cli_event(
            &store,
            CliEventIdentity {
                id: 2,
                session_id,
                model: "K2.7",
                agent_id: Some("call_same"),
                initiator: Some("sub-agent"),
            },
            CliEventTokens {
                input: 200,
                output: 20,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
                duration_ms: 60,
            },
            "2026-07-22T10:00:01",
        );

        std::env::set_var("COPILOT_DIR", &app_dir);
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        seed_cli_hook_row(&conn, session_id, 330);
        sync_copilot_cli_agent_usage_logs(&mut conn).unwrap();

        let rows = read_cli_agent_rows(&conn, session_id);
        assert_eq!(rows.len(), 2, "同一 CLI Agent 的兩個模型都必須保留");
        assert_eq!(
            rows.iter().map(|row| row.tokens_total).sum::<i64>(),
            330,
            "模型拆分後的 Token 總量必須維持不變"
        );
        let models: HashSet<&str> = rows.iter().filter_map(|row| row.model.as_deref()).collect();
        assert_eq!(models, HashSet::from(["DP4P", "K2.7"]));
        let timeline_rows = get_session_turns_token_stats(
            &conn,
            "copilot",
            &format!("{session_id}__call_same"),
            Some("copilot-cli"),
            None,
        )
        .unwrap();
        let (timeline_tokens, timeline_models) = timeline_rows.get(&1).unwrap();
        assert_eq!(
            timeline_tokens.total, 330,
            "timeline 必須彙總同一 CLI Agent 的多模型用量"
        );
        assert!(timeline_models.contains("DP4P"));
        assert!(timeline_models.contains("K2.7"));

        if let Some(value) = old_dir {
            std::env::set_var("COPILOT_DIR", value);
        } else {
            std::env::remove_var("COPILOT_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    #[test]
    fn cli_reconciler_retries_total_mismatch_without_new_event() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_dir = std::env::var("COPILOT_DIR").ok();

        let base_dir = temp_jsonl_path("cli-reconcile-mismatch-retry").with_extension("");
        let app_dir = base_dir.join("copilot");
        fs::create_dir_all(&app_dir).unwrap();
        let session_id = "cli-total-mismatch-retry";
        let store = build_cli_reconciler_fixture(&app_dir, &[], &[session_id]);
        insert_cli_event(
            &store,
            CliEventIdentity {
                id: 1,
                session_id,
                model: "DP4P",
                agent_id: None,
                initiator: None,
            },
            CliEventTokens {
                input: 100,
                output: 10,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
                duration_ms: 50,
            },
            "2026-07-22T10:00:00",
        );
        insert_cli_event(
            &store,
            CliEventIdentity {
                id: 2,
                session_id,
                model: "K2.7",
                agent_id: Some("call_retry"),
                initiator: Some("sub-agent"),
            },
            CliEventTokens {
                input: 200,
                output: 20,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
                duration_ms: 60,
            },
            "2026-07-22T10:00:01",
        );

        std::env::set_var("COPILOT_DIR", &app_dir);
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        seed_cli_hook_row(&conn, session_id, 999);

        sync_copilot_cli_agent_usage_logs(&mut conn).unwrap();
        let row_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries
                 WHERE assistant_type = 'copilot'
                   AND source_kind = 'copilot-cli'
                   AND (session_id = ? OR session_id LIKE ? ESCAPE '\\')",
                params![session_id, format!("{}\\__%", session_id)],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(row_count, 1, "第一次總量不符時必須保留原 hook row");

        conn.execute(
            "UPDATE usage_entries
             SET tokens_total = 330, delta_total = 330
             WHERE assistant_type = 'copilot'
               AND source_kind = 'copilot-cli'
               AND session_id = ?",
            [session_id],
        )
        .unwrap();
        sync_copilot_cli_agent_usage_logs(&mut conn).unwrap();

        let (rows, total): (i64, i64) = conn
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(tokens_total), 0)
                 FROM usage_entries
                 WHERE assistant_type = 'copilot'
                   AND source_kind = 'copilot-cli'
                   AND (session_id = ? OR session_id LIKE ? ESCAPE '\\')",
                params![session_id, format!("{}\\__%", session_id)],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(rows, 2, "只修正 hook、沒有新增 event 時仍必須重新處理");
        assert_eq!(total, 330);

        if let Some(value) = old_dir {
            std::env::set_var("COPILOT_DIR", value);
        } else {
            std::env::remove_var("COPILOT_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    #[test]
    fn cli_reconciler_commits_valid_sessions_while_another_is_pending() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_dir = std::env::var("COPILOT_DIR").ok();

        let base_dir = temp_jsonl_path("cli-reconcile-partial-batch").with_extension("");
        let app_dir = base_dir.join("copilot");
        fs::create_dir_all(&app_dir).unwrap();
        let valid_session = "valid-batch-session";
        let pending_session = "pending-batch-session";
        let store = build_cli_reconciler_fixture(&app_dir, &[], &[valid_session, pending_session]);
        insert_cli_event(
            &store,
            CliEventIdentity {
                id: 1,
                session_id: valid_session,
                model: "DP4P",
                agent_id: None,
                initiator: None,
            },
            CliEventTokens {
                input: 100,
                output: 10,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
                duration_ms: 50,
            },
            "2026-07-22T10:00:00",
        );
        insert_cli_event(
            &store,
            CliEventIdentity {
                id: 2,
                session_id: pending_session,
                model: "DP4P",
                agent_id: None,
                initiator: None,
            },
            CliEventTokens {
                input: 200,
                output: 20,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
                duration_ms: 50,
            },
            "2026-07-22T10:01:00",
        );

        std::env::set_var("COPILOT_DIR", &app_dir);
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        seed_cli_hook_row(&conn, valid_session, 110);
        seed_cli_hook_row(&conn, pending_session, 999);

        sync_copilot_cli_agent_usage_logs(&mut conn).unwrap();

        let valid_import_source: Option<String> = conn
            .query_row(
                "SELECT import_source_id
                 FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-cli'
                   AND session_id = ?",
                [valid_session],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            valid_import_source
                .as_deref()
                .is_some_and(|id| id.starts_with("copilot-cli-agents:")),
            "valid session must commit even when another session is pending"
        );

        let pending_row: (i64, Option<String>) = conn
            .query_row(
                "SELECT tokens_total, import_source_id
                 FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-cli'
                   AND session_id = ?",
                [pending_session],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(pending_row, (999, None), "pending hook row must remain");

        let pending_marker_count: i64 = conn
            .query_row(
                "SELECT COUNT(*)
                 FROM sync_state
                 WHERE filename LIKE ?",
                [format!(
                    "{}%{}",
                    COPILOT_CLI_AGENT_PENDING_PREFIX, pending_session
                )],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            pending_marker_count, 1,
            "pending session must be retained for a later retry"
        );

        if let Some(value) = old_dir {
            std::env::set_var("COPILOT_DIR", value);
        } else {
            std::env::remove_var("COPILOT_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    #[test]
    fn cli_reconciler_idempotent() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_dir = std::env::var("COPILOT_DIR").ok();

        let base_dir = temp_jsonl_path("cli-reconcile-idem").with_extension("");
        let app_dir = base_dir.join("copilot");
        fs::create_dir_all(&app_dir).unwrap();
        let session_id = "idempotent-session";
        let store = build_cli_reconciler_fixture(&app_dir, &[], &[session_id]);
        insert_cli_event(
            &store,
            CliEventIdentity {
                id: 1,
                session_id,
                model: "DP4P",
                agent_id: None,
                initiator: None,
            },
            CliEventTokens {
                input: 100,
                output: 10,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
                duration_ms: 50,
            },
            "2026-07-22T10:00:00",
        );
        insert_cli_event(
            &store,
            CliEventIdentity {
                id: 2,
                session_id,
                model: "DP4P",
                agent_id: Some("call_idem"),
                initiator: Some("sub-agent"),
            },
            CliEventTokens {
                input: 200,
                output: 20,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
                duration_ms: 60,
            },
            "2026-07-22T10:00:01",
        );

        std::env::set_var("COPILOT_DIR", &app_dir);
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        seed_cli_hook_row(&conn, session_id, 330);

        sync_copilot_cli_agent_usage_logs(&mut conn).unwrap();
        let count_after_first: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-cli'
                   AND (session_id = ? OR session_id LIKE ? ESCAPE '\\')",
                params![session_id, format!("{}\\__%", session_id)],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count_after_first, 2);

        sync_copilot_cli_agent_usage_logs(&mut conn).unwrap();
        let count_after_second: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-cli'
                   AND (session_id = ? OR session_id LIKE ? ESCAPE '\\')",
                params![session_id, format!("{}\\__%", session_id)],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count_after_second, 2, "second sync must not duplicate rows");

        if let Some(value) = old_dir {
            std::env::set_var("COPILOT_DIR", value);
        } else {
            std::env::remove_var("COPILOT_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    #[test]
    fn cli_reconciler_upserts_on_new_events() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_dir = std::env::var("COPILOT_DIR").ok();

        let base_dir = temp_jsonl_path("cli-reconcile-upsert").with_extension("");
        let app_dir = base_dir.join("copilot");
        fs::create_dir_all(&app_dir).unwrap();
        let session_id = "upsert-session";
        let store = build_cli_reconciler_fixture(&app_dir, &[], &[session_id]);
        insert_cli_event(
            &store,
            CliEventIdentity {
                id: 1,
                session_id,
                model: "DP4P",
                agent_id: None,
                initiator: None,
            },
            CliEventTokens {
                input: 100,
                output: 10,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
                duration_ms: 50,
            },
            "2026-07-22T10:00:00",
        );

        std::env::set_var("COPILOT_DIR", &app_dir);
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        seed_cli_hook_row(&conn, session_id, 110);

        sync_copilot_cli_agent_usage_logs(&mut conn).unwrap();
        let main_total: i64 = conn
            .query_row(
                "SELECT tokens_total FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-cli'
                   AND session_id = ? AND parent_session_id IS NULL",
                params![session_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(main_total, 110);

        // Add a new event for the same session (new API call).
        insert_cli_event(
            &store,
            CliEventIdentity {
                id: 2,
                session_id,
                model: "DP4P",
                agent_id: None,
                initiator: None,
            },
            CliEventTokens {
                input: 500,
                output: 50,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
                duration_ms: 80,
            },
            "2026-07-22T11:00:00",
        );
        // Update the hook row total to match the new combined total so
        // validation passes on the next sync.
        conn.execute(
            "UPDATE usage_entries SET tokens_total = ?, delta_total = ?
             WHERE assistant_type = 'copilot' AND source_kind = 'copilot-cli'
               AND session_id = ? AND parent_session_id IS NULL",
            params![660, 660, session_id],
        )
        .unwrap();

        sync_copilot_cli_agent_usage_logs(&mut conn).unwrap();
        let new_main_total: i64 = conn
            .query_row(
                "SELECT tokens_total FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-cli'
                   AND session_id = ? AND parent_session_id IS NULL",
                params![session_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            new_main_total, 660,
            "must re-aggregate from full history and upsert"
        );

        if let Some(value) = old_dir {
            std::env::set_var("COPILOT_DIR", value);
        } else {
            std::env::remove_var("COPILOT_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    #[test]
    fn cli_reconciler_replaces_hook_rows_no_double_count() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_dir = std::env::var("COPILOT_DIR").ok();

        let base_dir = temp_jsonl_path("cli-reconcile-replace").with_extension("");
        let app_dir = base_dir.join("copilot");
        fs::create_dir_all(&app_dir).unwrap();
        let session_id = "replace-session";
        let store = build_cli_reconciler_fixture(&app_dir, &[], &[session_id]);
        insert_cli_event(
            &store,
            CliEventIdentity {
                id: 1,
                session_id,
                model: "DP4P",
                agent_id: None,
                initiator: None,
            },
            CliEventTokens {
                input: 100,
                output: 10,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
                duration_ms: 50,
            },
            "2026-07-22T10:00:00",
        );
        insert_cli_event(
            &store,
            CliEventIdentity {
                id: 2,
                session_id,
                model: "DP4P",
                agent_id: Some("call_replace"),
                initiator: Some("sub-agent"),
            },
            CliEventTokens {
                input: 200,
                output: 20,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
                duration_ms: 60,
            },
            "2026-07-22T10:00:01",
        );

        std::env::set_var("COPILOT_DIR", &app_dir);
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        seed_cli_hook_row(&conn, session_id, 330);

        sync_copilot_cli_agent_usage_logs(&mut conn).unwrap();

        // Only the 2 split rows remain; the original merged hook row is gone.
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-cli'
                   AND session_id = ? AND parent_session_id IS NULL",
                params![session_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "exactly one main row (hook merged row replaced)");

        let total_sum: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(tokens_total), 0) FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-cli'
                   AND (session_id = ? OR session_id LIKE ? ESCAPE '\\')",
                params![session_id, format!("{}\\__%", session_id)],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            total_sum, 330,
            "split rows sum to original hook total, no double count"
        );

        if let Some(value) = old_dir {
            std::env::set_var("COPILOT_DIR", value);
        } else {
            std::env::remove_var("COPILOT_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    #[test]
    fn cli_reconciler_preserves_hook_when_agent_events_lag() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_dir = std::env::var("COPILOT_DIR").ok();

        let base_dir = temp_jsonl_path("cli-reconcile-rollback").with_extension("");
        let app_dir = base_dir.join("copilot");
        fs::create_dir_all(&app_dir).unwrap();
        let session_id = "mismatch-session";
        let store = build_cli_reconciler_fixture(&app_dir, &[], &[session_id]);
        // Agent events sum to 330.
        insert_cli_event(
            &store,
            CliEventIdentity {
                id: 1,
                session_id,
                model: "DP4P",
                agent_id: None,
                initiator: None,
            },
            CliEventTokens {
                input: 100,
                output: 10,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
                duration_ms: 50,
            },
            "2026-07-22T10:00:00",
        );
        insert_cli_event(
            &store,
            CliEventIdentity {
                id: 2,
                session_id,
                model: "DP4P",
                agent_id: Some("call_mismatch"),
                initiator: Some("sub-agent"),
            },
            CliEventTokens {
                input: 200,
                output: 20,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
                duration_ms: 60,
            },
            "2026-07-22T10:00:01",
        );

        std::env::set_var("COPILOT_DIR", &app_dir);
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        // Hook usage is ahead of the available agent events. Reconciliation
        // must preserve this session until the agent event store catches up.
        seed_cli_hook_row(&conn, session_id, 999);

        sync_copilot_cli_agent_usage_logs(&mut conn).unwrap();

        let hook_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-cli'
                   AND session_id = ? AND parent_session_id IS NULL",
                params![session_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            hook_rows, 1,
            "hook row must be preserved on validation failure"
        );

        let hook_total: i64 = conn
            .query_row(
                "SELECT tokens_total FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-cli'
                   AND session_id = ? AND parent_session_id IS NULL",
                params![session_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(hook_total, 999, "original hook total preserved");

        let subagent_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-cli'
                   AND session_id LIKE ? ESCAPE '\\'",
                params![format!("{}\\__%", session_id)],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            subagent_rows, 0,
            "pending session must not contain partially split subagent rows"
        );

        if let Some(value) = old_dir {
            std::env::set_var("COPILOT_DIR", value);
        } else {
            std::env::remove_var("COPILOT_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    #[test]
    fn cli_reconciler_falls_back_when_session_store_missing() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_dir = std::env::var("COPILOT_DIR").ok();

        let base_dir = temp_jsonl_path("cli-reconcile-no-store").with_extension("");
        let app_dir = base_dir.join("copilot");
        fs::create_dir_all(&app_dir).unwrap();
        // No session-store.db created; only a data.db registry + transcript.
        create_copilot_app_registry(&app_dir, &[]);
        let dir = app_dir.join("session-state").join("no-store-session");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("events.jsonl"), "").unwrap();

        std::env::set_var("COPILOT_DIR", &app_dir);
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        seed_cli_hook_row(&conn, "no-store-session", 500);

        sync_copilot_cli_agent_usage_logs(&mut conn).unwrap();

        // Hook row untouched.
        let total: i64 = conn
            .query_row(
                "SELECT tokens_total FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-cli'
                   AND session_id = 'no-store-session'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            total, 500,
            "hook fallback preserved when session-store missing"
        );

        if let Some(value) = old_dir {
            std::env::set_var("COPILOT_DIR", value);
        } else {
            std::env::remove_var("COPILOT_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    #[test]
    fn cli_reconciler_skips_app_registry_sessions() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_dir = std::env::var("COPILOT_DIR").ok();

        let base_dir = temp_jsonl_path("cli-reconcile-app-skip").with_extension("");
        let app_dir = base_dir.join("copilot");
        fs::create_dir_all(&app_dir).unwrap();
        let app_session = "app-owned-session";
        // Register the session in the App registry so it classifies as App,
        // not CLI — even though a transcript exists.
        let store = build_cli_reconciler_fixture(&app_dir, &[app_session], &[]);
        insert_cli_event(
            &store,
            CliEventIdentity {
                id: 1,
                session_id: app_session,
                model: "DP4P",
                agent_id: None,
                initiator: None,
            },
            CliEventTokens {
                input: 100,
                output: 10,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
                duration_ms: 50,
            },
            "2026-07-22T10:00:00",
        );

        std::env::set_var("COPILOT_DIR", &app_dir);
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        seed_cli_hook_row(&conn, app_session, 110);

        sync_copilot_cli_agent_usage_logs(&mut conn).unwrap();

        // App session not processed by CLI reconciler: hook row preserved, no
        // split rows.
        let rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-cli'
                   AND session_id = ?",
                params![app_session],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            rows, 1,
            "App registry session must not be touched by CLI reconciler"
        );

        if let Some(value) = old_dir {
            std::env::set_var("COPILOT_DIR", value);
        } else {
            std::env::remove_var("COPILOT_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    #[test]
    fn cli_reconciler_preserves_other_collectors() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_dir = std::env::var("COPILOT_DIR").ok();

        let base_dir = temp_jsonl_path("cli-reconcile-preserve").with_extension("");
        let app_dir = base_dir.join("copilot");
        fs::create_dir_all(&app_dir).unwrap();
        let cli_session = "cli-preserve-sess";
        let store = build_cli_reconciler_fixture(&app_dir, &[], &[cli_session]);
        insert_cli_event(
            &store,
            CliEventIdentity {
                id: 1,
                session_id: cli_session,
                model: "DP4P",
                agent_id: None,
                initiator: None,
            },
            CliEventTokens {
                input: 100,
                output: 10,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
                duration_ms: 50,
            },
            "2026-07-22T10:00:00",
        );

        std::env::set_var("COPILOT_DIR", &app_dir);
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        seed_cli_hook_row(&conn, cli_session, 110);

        // Pre-seed cross-source rows that must survive CLI reconciliation.
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, source_dir_key, timestamp, date,
                session_id, turn_no, import_source_id, model, tokens_total, tokens_input, tokens_output, delta_total, delta_input, delta_output
             ) VALUES (
                'copilot', 'copilot-app', 'abc123', '2026-07-01T10:00:00Z', '2026-07-01',
                'app-sess', 1, 'copilot-app:abc123:app-sess:0', 'DP4P', 110, 100, 10, 110, 100, 10
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, timestamp, date,
                session_id, turn_no, import_source_id, model, tokens_total, tokens_input, tokens_output, delta_total, delta_input, delta_output
             ) VALUES (
                'codex', 'legacy', '2026-07-01T10:00:00Z', '2026-07-01',
                'codex-sess', 1, 'codex-import', 'gpt-5', 110, 100, 10, 110, 100, 10
             )",
            [],
        )
        .unwrap();

        sync_copilot_cli_agent_usage_logs(&mut conn).unwrap();

        let app_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries WHERE session_id = 'app-sess'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(app_count, 1, "copilot-app row must survive");
        let codex_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries WHERE session_id = 'codex-sess'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(codex_count, 1, "codex row must survive");

        if let Some(value) = old_dir {
            std::env::set_var("COPILOT_DIR", value);
        } else {
            std::env::remove_var("COPILOT_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    #[test]
    fn cli_reconciler_cursor_isolated_from_app_cursor() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_dir = std::env::var("COPILOT_DIR").ok();
        let old_app_dir = std::env::var("COPILOT_APP_DIR").ok();

        let base_dir = temp_jsonl_path("cli-reconcile-cursor").with_extension("");
        let app_dir = base_dir.join("copilot");
        fs::create_dir_all(&app_dir).unwrap();
        let cli_session = "cursor-cli-sess";
        let app_session = "cursor-app-sess";
        let store = build_cli_reconciler_fixture(&app_dir, &[app_session], &[cli_session]);
        insert_cli_event(
            &store,
            CliEventIdentity {
                id: 1,
                session_id: cli_session,
                model: "DP4P",
                agent_id: None,
                initiator: None,
            },
            CliEventTokens {
                input: 100,
                output: 10,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
                duration_ms: 50,
            },
            "2026-07-22T10:00:00",
        );
        insert_cli_event(
            &store,
            CliEventIdentity {
                id: 2,
                session_id: app_session,
                model: "DP4P",
                agent_id: None,
                initiator: None,
            },
            CliEventTokens {
                input: 100,
                output: 10,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
                duration_ms: 50,
            },
            "2026-07-22T10:00:01",
        );

        std::env::set_var("COPILOT_DIR", &app_dir);
        std::env::set_var("COPILOT_APP_DIR", &app_dir);
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        seed_cli_hook_row(&conn, cli_session, 110);

        sync_copilot_cli_agent_usage_logs(&mut conn).unwrap();

        // Both cursor namespaces must coexist independently.
        let cli_cursor_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_state WHERE filename LIKE 'sync:copilot_cli_agents:%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(cli_cursor_count >= 1, "CLI agent cursor written");

        // Run the App collector too; it must not overwrite the CLI cursor.
        sync_copilot_app_usage_logs(&mut conn).unwrap();
        let cli_cursor_after_app: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_state WHERE filename LIKE 'sync:copilot_cli_agents:%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            cli_cursor_after_app, cli_cursor_count,
            "App collector must not touch CLI cursor"
        );

        let app_cursor_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_state WHERE filename LIKE 'sync:copilot_app:cursor:%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(app_cursor_count >= 1, "App cursor written");

        if let Some(value) = old_dir {
            std::env::set_var("COPILOT_DIR", value);
        } else {
            std::env::remove_var("COPILOT_DIR");
        }
        if let Some(value) = old_app_dir {
            std::env::set_var("COPILOT_APP_DIR", value);
        } else {
            std::env::remove_var("COPILOT_APP_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    #[test]
    fn cli_reconciler_backfills_historical_hook_session() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_dir = std::env::var("COPILOT_DIR").ok();

        let base_dir = temp_jsonl_path("cli-reconcile-backfill").with_extension("");
        let app_dir = base_dir.join("copilot");
        fs::create_dir_all(&app_dir).unwrap();
        let session_id = "historical-cli-sess";
        let store = build_cli_reconciler_fixture(&app_dir, &[], &[session_id]);
        insert_cli_event(
            &store,
            CliEventIdentity {
                id: 1,
                session_id,
                model: "DP4P",
                agent_id: None,
                initiator: None,
            },
            CliEventTokens {
                input: 100,
                output: 10,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
                duration_ms: 50,
            },
            "2026-07-22T10:00:00",
        );
        insert_cli_event(
            &store,
            CliEventIdentity {
                id: 2,
                session_id,
                model: "DP4P",
                agent_id: Some("call_hist"),
                initiator: Some("sub-agent"),
            },
            CliEventTokens {
                input: 200,
                output: 20,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
                duration_ms: 60,
            },
            "2026-07-22T10:00:01",
        );

        std::env::set_var("COPILOT_DIR", &app_dir);
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        // Simulate a historical hook row that predates the reconciler.
        seed_cli_hook_row(&conn, session_id, 330);

        // First sync performs the backfill migration.
        sync_copilot_cli_agent_usage_logs(&mut conn).unwrap();
        let migration_done: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_state WHERE filename = ?)",
                params![COPILOT_CLI_AGENT_MIGRATION_KEY],
                |row| row.get(0),
            )
            .unwrap();
        assert!(migration_done, "migration marker recorded after backfill");

        let split_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries
                 WHERE assistant_type = 'copilot' AND source_kind = 'copilot-cli'
                   AND (session_id = ? OR session_id LIKE ? ESCAPE '\\')",
                params![session_id, format!("{}\\__%", session_id)],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            split_count, 2,
            "historical session backfilled into split rows"
        );

        if let Some(value) = old_dir {
            std::env::set_var("COPILOT_DIR", value);
        } else {
            std::env::remove_var("COPILOT_DIR");
        }
        let _ = fs::remove_dir_all(base_dir);
    }

    /// Regression: `get_session_model` must return the child session's own
    /// model for a Copilot App subagent synthetic row (`<main>__<agent_id>`),
    /// NOT the parent's model. The drawer parser relies on this so the
    /// subagent drawer shows the child model and never the parent's
    /// `session.start.selectedModel`.
    #[test]
    fn get_session_model_returns_child_model_for_subagent_synthetic_row() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        let parent = "74b6d236-d311-4675-9855-fee91bc508e5";
        let agent = "call_v4b32z66";
        let synthetic = format!("{parent}__{agent}");

        // Parent (main agent) row uses a different model.
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, source_dir_key, timestamp, date,
                session_id, turn_no, model,
                tokens_input, tokens_output, tokens_total,
                delta_input, delta_output, delta_total
             ) VALUES (
                'copilot', 'copilot-app', 'abcdef00', '2026-07-20T10:00:00Z', '2026-07-20',
                ?, 1, 'GLM5.2-medium',
                50, 5, 55,
                50, 5, 55
             )",
            params![parent],
        )
        .unwrap();

        // Subagent synthetic row uses K2.7.
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, source_dir_key, timestamp, date,
                session_id, turn_no, model,
                tokens_input, tokens_output, tokens_total,
                delta_input, delta_output, delta_total,
                parent_session_id, agent_nickname
             ) VALUES (
                'copilot', 'copilot-app', 'abcdef00', '2026-07-20T10:00:30Z', '2026-07-20',
                ?, 1, 'K2.7',
                100, 10, 110,
                100, 10, 110,
                ?, ?
             )",
            params![synthetic, parent, agent],
        )
        .unwrap();

        // The subagent synthetic row must resolve to K2.7 (its own model),
        // not the parent's GLM5.2-medium.
        let child_model = get_session_model(
            &conn,
            "copilot",
            &synthetic,
            Some("copilot-app"),
            Some("abcdef00"),
        )
        .unwrap();
        assert_eq!(child_model.as_deref(), Some("K2.7"));

        // The main session must resolve to its own model.
        let main_model = get_session_model(
            &conn,
            "copilot",
            parent,
            Some("copilot-app"),
            Some("abcdef00"),
        )
        .unwrap();
        assert_eq!(main_model.as_deref(), Some("GLM5.2-medium"));

        // No source_dir_key (None) means source_dir_key IS NULL — App rows
        // are excluded, so the synthetic App session is not found.
        let unscoped = get_session_model(&conn, "copilot", &synthetic, None, None).unwrap();
        assert_eq!(unscoped, None);
    }

    /// Regression: `get_session_model` returns `None` when the session has no
    /// populated model column (caller falls back to the parser default).
    #[test]
    fn get_session_model_returns_none_when_model_column_is_empty() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, source_dir_key, timestamp, date,
                session_id, turn_no,
                tokens_input, tokens_output, tokens_total,
                delta_input, delta_output, delta_total
             ) VALUES (
                'copilot', 'copilot-cli', NULL, '2026-07-20T10:00:00Z', '2026-07-20',
                'no-model-sess', 1,
                10, 1, 11,
                10, 1, 11
             )",
            [],
        )
        .unwrap();
        let model = get_session_model(&conn, "copilot", "no-model-sess", Some("copilot-cli"), None)
            .unwrap();
        assert!(
            model.is_none(),
            "expected None for model-less session, got {:?}",
            model
        );
    }

    /// Regression: two Copilot App sessions with the same `session_id` but
    /// different `source_dir_key` values must be isolated by
    /// `get_session_assistant_and_transcript`, `get_session_cwd`,
    /// `get_session_model`, and `get_session_turns_token_stats`.
    #[test]
    fn session_queries_isolate_copilot_app_by_source_dir_key() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        let session_id = "shared-app-session";
        let dir_a = "aaaa00";
        let dir_b = "bbbb00";

        // Insert App row from directory A.
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, source_dir_key, timestamp, date,
                session_id, turn_no, model, cwd,
                tokens_input, tokens_output, tokens_total,
                delta_input, delta_output, delta_total
             ) VALUES (
                'copilot', 'copilot-app', ?, '2026-07-20T10:00:00Z', '2026-07-20',
                ?, 1, 'GLM5.2-high', '/home/dirA',
                100, 10, 110,
                100, 10, 110
             )",
            params![dir_a, session_id],
        )
        .unwrap();

        // Insert App row from directory B with same session_id.
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, source_dir_key, timestamp, date,
                session_id, turn_no, model, cwd,
                tokens_input, tokens_output, tokens_total,
                delta_input, delta_output, delta_total
             ) VALUES (
                'copilot', 'copilot-app', ?, '2026-07-20T10:00:00Z', '2026-07-20',
                ?, 1, 'K2.7-code', '/home/dirB',
                200, 20, 220,
                200, 20, 220
             )",
            params![dir_b, session_id],
        )
        .unwrap();

        // get_session_assistant_and_transcript must return the correct
        // source_dir_key when queried with explicit source_kind + source_dir_key.
        let (_, _, sk_a, sdk_a, _, _) = get_session_assistant_and_transcript(
            &conn,
            "copilot",
            session_id,
            Some("copilot-app"),
            Some(dir_a),
        )
        .unwrap();
        assert_eq!(sk_a, "copilot-app");
        assert_eq!(sdk_a.as_deref(), Some(dir_a));

        let (_, _, sk_b, sdk_b, _, _) = get_session_assistant_and_transcript(
            &conn,
            "copilot",
            session_id,
            Some("copilot-app"),
            Some(dir_b),
        )
        .unwrap();
        assert_eq!(sk_b, "copilot-app");
        assert_eq!(sdk_b.as_deref(), Some(dir_b));

        // get_session_cwd must return the correct CWD per source_dir_key.
        let cwd_a = get_session_cwd(
            &conn,
            "copilot",
            session_id,
            Some("copilot-app"),
            Some(dir_a),
        )
        .unwrap();
        assert_eq!(cwd_a.as_deref(), Some("/home/dirA"));
        let cwd_b = get_session_cwd(
            &conn,
            "copilot",
            session_id,
            Some("copilot-app"),
            Some(dir_b),
        )
        .unwrap();
        assert_eq!(cwd_b.as_deref(), Some("/home/dirB"));

        // get_session_model must return the correct model per source_dir_key.
        let model_a = get_session_model(
            &conn,
            "copilot",
            session_id,
            Some("copilot-app"),
            Some(dir_a),
        )
        .unwrap();
        assert_eq!(model_a.as_deref(), Some("GLM5.2-high"));
        let model_b = get_session_model(
            &conn,
            "copilot",
            session_id,
            Some("copilot-app"),
            Some(dir_b),
        )
        .unwrap();
        assert_eq!(model_b.as_deref(), Some("K2.7-code"));

        // get_session_turns_token_stats must return the correct tokens.
        let turns_a = get_session_turns_token_stats(
            &conn,
            "copilot",
            session_id,
            Some("copilot-app"),
            Some(dir_a),
        )
        .unwrap();
        let turns_b = get_session_turns_token_stats(
            &conn,
            "copilot",
            session_id,
            Some("copilot-app"),
            Some(dir_b),
        )
        .unwrap();
        assert_eq!(turns_a.len(), 1);
        assert_eq!(turns_b.len(), 1);
        assert_eq!(turns_a[&1].0.total, 110);
        assert_eq!(turns_b[&1].0.total, 220);
    }

    /// Regression: Copilot CLI and Copilot App sessions with the same
    /// `session_id` must be isolated. CLI rows have `source_dir_key IS NULL`,
    /// App rows have `source_dir_key = <hex>`. Querying with `None` must
    /// return only the CLI row, not the App row.
    #[test]
    fn session_queries_isolate_copilot_cli_from_app_with_same_session_id() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        let session_id = "shared-cli-app-session";
        let dir_app = "dead00";

        // CLI row (source_dir_key IS NULL).
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, source_dir_key, timestamp, date,
                session_id, turn_no, model, cwd,
                tokens_input, tokens_output, tokens_total,
                delta_input, delta_output, delta_total
             ) VALUES (
                'copilot', 'copilot-cli', NULL, '2026-07-20T10:00:00Z', '2026-07-20',
                ?, 1, 'GPT-5.4', '/home/cli',
                50, 5, 55,
                50, 5, 55
             )",
            params![session_id],
        )
        .unwrap();

        // App row (source_dir_key = hex).
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, source_dir_key, timestamp, date,
                session_id, turn_no, model, cwd,
                tokens_input, tokens_output, tokens_total,
                delta_input, delta_output, delta_total
             ) VALUES (
                'copilot', 'copilot-app', ?, '2026-07-20T10:00:00Z', '2026-07-20',
                ?, 1, 'GLM5.2-high', '/home/app',
                100, 10, 110,
                100, 10, 110
             )",
            params![dir_app, session_id],
        )
        .unwrap();

        // Query with source_kind=copilot-cli, source_dir_key=None must find
        // the CLI row only.
        let (_, _, sk, _sdk, _, _) = get_session_assistant_and_transcript(
            &conn,
            "copilot",
            session_id,
            Some("copilot-cli"),
            None,
        )
        .unwrap();
        assert_eq!(sk, "copilot-cli");

        // Query with source_kind=copilot-app, source_dir_key=hex must find
        // the App row only.
        let (_, _, sk_app, sdk_app, _, _) = get_session_assistant_and_transcript(
            &conn,
            "copilot",
            session_id,
            Some("copilot-app"),
            Some(dir_app),
        )
        .unwrap();
        assert_eq!(sk_app, "copilot-app");
        assert_eq!(sdk_app.as_deref(), Some(dir_app));

        // CWD isolation: None -> CLI row; Some(dir) -> App row.
        let cwd_cli =
            get_session_cwd(&conn, "copilot", session_id, Some("copilot-cli"), None).unwrap();
        assert_eq!(cwd_cli.as_deref(), Some("/home/cli"));
        let cwd_app = get_session_cwd(
            &conn,
            "copilot",
            session_id,
            Some("copilot-app"),
            Some(dir_app),
        )
        .unwrap();
        assert_eq!(cwd_app.as_deref(), Some("/home/app"));

        // Model isolation: None -> CLI model; Some(dir) -> App model.
        let model_cli =
            get_session_model(&conn, "copilot", session_id, Some("copilot-cli"), None).unwrap();
        assert_eq!(model_cli.as_deref(), Some("GPT-5.4"));
        let model_app = get_session_model(
            &conn,
            "copilot",
            session_id,
            Some("copilot-app"),
            Some(dir_app),
        )
        .unwrap();
        assert_eq!(model_app.as_deref(), Some("GLM5.2-high"));

        // Turn stats isolation: None -> CLI totals; Some(dir) -> App totals.
        let turns_cli =
            get_session_turns_token_stats(&conn, "copilot", session_id, Some("copilot-cli"), None)
                .unwrap();
        let turns_app = get_session_turns_token_stats(
            &conn,
            "copilot",
            session_id,
            Some("copilot-app"),
            Some(dir_app),
        )
        .unwrap();
        assert_eq!(turns_cli[&1].0.total, 55);
        assert_eq!(turns_app[&1].0.total, 110);
    }

    /// Regression: Copilot CLI and VS Code Chat rows both have a NULL
    /// source_dir_key, so downstream session queries must also filter by
    /// source_kind to avoid mixing their CWD, model, or turn token stats.
    #[test]
    fn session_queries_isolate_null_source_kinds_with_same_session_id() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        let session_id = "shared-cli-vscode-session";

        for (source_kind, model, cwd, total) in [
            ("copilot-cli", "GPT-5.4", "/home/cli", 55i64),
            ("vscode-chat", "GPT-4.1", "/home/vscode", 110i64),
        ] {
            conn.execute(
                "INSERT INTO usage_entries (
                    assistant_type, source_kind, source_dir_key, timestamp, date,
                    session_id, turn_no, model, cwd,
                    tokens_input, tokens_output, tokens_total,
                    delta_input, delta_output, delta_total
                 ) VALUES (
                    'copilot', ?, NULL, '2026-07-20T10:00:00Z', '2026-07-20',
                    ?, 1, ?, ?,
                    ?, 5, ?,
                    ?, 5, ?
                 )",
                params![
                    source_kind,
                    session_id,
                    model,
                    cwd,
                    total - 5,
                    total,
                    total - 5,
                    total,
                ],
            )
            .unwrap();
        }

        // A different assistant may also reuse the same session_id; the
        // assistant_type predicate must keep it out of Copilot queries.
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, source_dir_key, timestamp, date,
                session_id, turn_no, model, cwd,
                tokens_input, tokens_output, tokens_total,
                delta_input, delta_output, delta_total
             ) VALUES (
                'codex', 'codex-cli', NULL, '2026-07-20T10:00:00Z', '2026-07-20',
                ?, 1, 'o3', '/home/codex',
                28, 5, 33,
                28, 5, 33
             )",
            params![session_id],
        )
        .unwrap();

        let (_, _, cli_kind, cli_dir, _, _) = get_session_assistant_and_transcript(
            &conn,
            "copilot",
            session_id,
            Some("copilot-cli"),
            None,
        )
        .unwrap();
        assert_eq!(cli_kind, "copilot-cli");
        assert_eq!(cli_dir, None);

        let cli_cwd =
            get_session_cwd(&conn, "copilot", session_id, Some("copilot-cli"), None).unwrap();
        let vscode_cwd =
            get_session_cwd(&conn, "copilot", session_id, Some("vscode-chat"), None).unwrap();
        assert_eq!(cli_cwd.as_deref(), Some("/home/cli"));
        assert_eq!(vscode_cwd.as_deref(), Some("/home/vscode"));

        let cli_model =
            get_session_model(&conn, "copilot", session_id, Some("copilot-cli"), None).unwrap();
        let vscode_model =
            get_session_model(&conn, "copilot", session_id, Some("vscode-chat"), None).unwrap();
        assert_eq!(cli_model.as_deref(), Some("GPT-5.4"));
        assert_eq!(vscode_model.as_deref(), Some("GPT-4.1"));

        let cli_turns =
            get_session_turns_token_stats(&conn, "copilot", session_id, Some("copilot-cli"), None)
                .unwrap();
        let vscode_turns =
            get_session_turns_token_stats(&conn, "copilot", session_id, Some("vscode-chat"), None)
                .unwrap();
        assert_eq!(cli_turns[&1].0.total, 55);
        assert_eq!(vscode_turns[&1].0.total, 110);

        let codex_cwd =
            get_session_cwd(&conn, "codex", session_id, Some("codex-cli"), None).unwrap();
        let codex_model =
            get_session_model(&conn, "codex", session_id, Some("codex-cli"), None).unwrap();
        let codex_turns =
            get_session_turns_token_stats(&conn, "codex", session_id, Some("codex-cli"), None)
                .unwrap();
        assert_eq!(codex_cwd.as_deref(), Some("/home/codex"));
        assert_eq!(codex_model.as_deref(), Some("o3"));
        assert_eq!(codex_turns[&1].0.total, 33);
    }

    /// Regression: when no source_kind or source_dir_key is provided (legacy
    /// caller), the query must deterministically return the non-App row
    /// (source_dir_key IS NULL), not an arbitrary row.
    #[test]
    fn session_queries_legacy_none_is_deterministic_and_prefers_non_app() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        let session_id = "ambiguous-session";

        // Insert an App row first (lower turn_no to test ordering is not
        // by insertion or turn_no).
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, source_dir_key, timestamp, date,
                session_id, turn_no, model, cwd,
                tokens_input, tokens_output, tokens_total,
                delta_input, delta_output, delta_total
             ) VALUES (
                'copilot', 'copilot-app', 'dead00', '2026-07-20T10:00:00Z', '2026-07-20',
                ?, 1, 'GLM5.2-high', '/home/app',
                100, 10, 110,
                100, 10, 110
             )",
            params![session_id],
        )
        .unwrap();

        // Insert a CLI row (source_dir_key IS NULL) after the App row.
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, source_dir_key, timestamp, date,
                session_id, turn_no, model, cwd,
                tokens_input, tokens_output, tokens_total,
                delta_input, delta_output, delta_total
             ) VALUES (
                'copilot', 'copilot-cli', NULL, '2026-07-20T10:00:00Z', '2026-07-20',
                ?, 2, 'GPT-5.4', '/home/cli',
                50, 5, 55,
                50, 5, 55
             )",
            params![session_id],
        )
        .unwrap();

        // Legacy query (both source_kind and source_dir_key are None) must
        // return the CLI row because source_dir_key IS NULL is the filter.
        let (_, _, sk, _sdk, _, _) =
            get_session_assistant_and_transcript(&conn, "copilot", session_id, None, None).unwrap();
        assert_eq!(sk, "copilot-cli");

        // CWD with None must be the CLI CWD.
        let cwd = get_session_cwd(&conn, "copilot", session_id, None, None).unwrap();
        assert_eq!(cwd.as_deref(), Some("/home/cli"));

        // Model with None must be the CLI model.
        let model = get_session_model(&conn, "copilot", session_id, None, None).unwrap();
        assert_eq!(model.as_deref(), Some("GPT-5.4"));

        // Turn stats with None must be the CLI totals.
        let turns =
            get_session_turns_token_stats(&conn, "copilot", session_id, None, None).unwrap();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[&2].0.total, 55);
    }

    /// Regression: Copilot CLI and VS Code Chat sessions share a NULL
    /// `source_dir_key` (they are non-App collectors) and may collide on the
    /// same `session_id`. Querying with an explicit `source_kind` must keep
    /// identity, CWD, model, and turn stats strictly isolated.
    ///
    /// Coverage:
    /// - same `assistant_type`
    /// - same `session_id`
    /// - same `turn_no` (so `get_session_turns_token_stats` cannot rely on
    ///   the turn number to distinguish sources)
    /// - both `source_dir_key IS NULL`
    /// - different `source_kind` / `cwd` / `model` / token counts
    /// - explicit `source_kind` → identity, CWD, model, turn stats never
    ///   mix between CLI and VS Code Chat.
    #[test]
    fn session_queries_isolate_copilot_cli_vs_vscode_chat_with_same_session_id() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        let session_id = "iso-cli-vscode-chat";

        // Same assistant, same session_id, same turn_no, both source_dir_key
        // IS NULL, different source_kind / CWD / model / token totals.
        for (source_kind, model, cwd, total) in [
            ("copilot-cli", "GPT-5.4", "/home/cli", 55i64),
            ("vscode-chat", "GPT-4.1", "/home/vscode", 110i64),
        ] {
            conn.execute(
                "INSERT INTO usage_entries (
                    assistant_type, source_kind, source_dir_key, timestamp, date,
                    session_id, turn_no, model, cwd,
                    tokens_input, tokens_output, tokens_total,
                    delta_input, delta_output, delta_total
                 ) VALUES (
                    'copilot', ?, NULL, '2026-07-20T10:00:00Z', '2026-07-20',
                    ?, 1, ?, ?,
                    ?, 5, ?,
                    ?, 5, ?
                 )",
                params![
                    source_kind,
                    session_id,
                    model,
                    cwd,
                    total - 5,
                    total,
                    total - 5,
                    total,
                ],
            )
            .unwrap();
        }

        // Identity must select the matching source_kind and report
        // source_dir_key as None for both.
        let (_, _, sk_cli, sdk_cli, _, _) = get_session_assistant_and_transcript(
            &conn,
            "copilot",
            session_id,
            Some("copilot-cli"),
            None,
        )
        .unwrap();
        assert_eq!(sk_cli, "copilot-cli");
        assert_eq!(sdk_cli, None);

        let (_, _, sk_vscode, sdk_vscode, _, _) = get_session_assistant_and_transcript(
            &conn,
            "copilot",
            session_id,
            Some("vscode-chat"),
            None,
        )
        .unwrap();
        assert_eq!(sk_vscode, "vscode-chat");
        assert_eq!(sdk_vscode, None);

        // CWD isolation per source_kind.
        let cwd_cli =
            get_session_cwd(&conn, "copilot", session_id, Some("copilot-cli"), None).unwrap();
        let cwd_vscode =
            get_session_cwd(&conn, "copilot", session_id, Some("vscode-chat"), None).unwrap();
        assert_eq!(cwd_cli.as_deref(), Some("/home/cli"));
        assert_eq!(cwd_vscode.as_deref(), Some("/home/vscode"));

        // Model isolation per source_kind.
        let model_cli =
            get_session_model(&conn, "copilot", session_id, Some("copilot-cli"), None).unwrap();
        let model_vscode =
            get_session_model(&conn, "copilot", session_id, Some("vscode-chat"), None).unwrap();
        assert_eq!(model_cli.as_deref(), Some("GPT-5.4"));
        assert_eq!(model_vscode.as_deref(), Some("GPT-4.1"));

        // Turn stats isolation per source_kind. The map must contain exactly
        // one entry per source_kind (same turn_no on both sides) and the
        // totals must not cross-pollinate.
        let turns_cli =
            get_session_turns_token_stats(&conn, "copilot", session_id, Some("copilot-cli"), None)
                .unwrap();
        let turns_vscode =
            get_session_turns_token_stats(&conn, "copilot", session_id, Some("vscode-chat"), None)
                .unwrap();
        assert_eq!(turns_cli.len(), 1);
        assert_eq!(turns_vscode.len(), 1);
        assert_eq!(turns_cli[&1].0.total, 55);
        assert_eq!(turns_vscode[&1].0.total, 110);
    }

    /// Regression: different assistant types may share a `session_id` and
    /// both have `source_dir_key IS NULL` (e.g. Copilot CLI and Codex CLI
    /// can both use the same hex-looking id). The assistant_type predicate
    /// must keep downstream CWD / model / turn stats isolated per assistant.
    ///
    /// Coverage:
    /// - different `assistant_type`
    /// - same `session_id`
    /// - same `turn_no`
    /// - both `source_dir_key IS NULL`
    /// - different CWD / model / token totals per assistant
    /// - downstream queries (CWD / model / turn stats) must be isolated
    ///   by assistant_type.
    #[test]
    fn session_queries_isolate_cross_assistant_with_same_session_id_and_null_key() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        let session_id = "iso-cross-assistant";

        // Copilot row.
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, source_dir_key, timestamp, date,
                session_id, turn_no, model, cwd,
                tokens_input, tokens_output, tokens_total,
                delta_input, delta_output, delta_total
             ) VALUES (
                'copilot', 'copilot-cli', NULL, '2026-07-20T10:00:00Z', '2026-07-20',
                ?, 1, 'GPT-5.4', '/home/copilot',
                50, 5, 55,
                50, 5, 55
             )",
            params![session_id],
        )
        .unwrap();

        // Codex row with the same session_id, same turn_no, both NULL key.
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, source_dir_key, timestamp, date,
                session_id, turn_no, model, cwd,
                tokens_input, tokens_output, tokens_total,
                delta_input, delta_output, delta_total
             ) VALUES (
                'codex', 'codex-cli', NULL, '2026-07-20T10:00:00Z', '2026-07-20',
                ?, 1, 'o3', '/home/codex',
                28, 5, 33,
                28, 5, 33
             )",
            params![session_id],
        )
        .unwrap();

        // Downstream queries with assistant='copilot' must NOT see codex data.
        let cwd_copilot =
            get_session_cwd(&conn, "copilot", session_id, Some("copilot-cli"), None).unwrap();
        let model_copilot =
            get_session_model(&conn, "copilot", session_id, Some("copilot-cli"), None).unwrap();
        let turns_copilot =
            get_session_turns_token_stats(&conn, "copilot", session_id, Some("copilot-cli"), None)
                .unwrap();
        assert_eq!(cwd_copilot.as_deref(), Some("/home/copilot"));
        assert_eq!(model_copilot.as_deref(), Some("GPT-5.4"));
        assert_eq!(turns_copilot[&1].0.total, 55);

        // Downstream queries with assistant='codex' must NOT see copilot data.
        let cwd_codex =
            get_session_cwd(&conn, "codex", session_id, Some("codex-cli"), None).unwrap();
        let model_codex =
            get_session_model(&conn, "codex", session_id, Some("codex-cli"), None).unwrap();
        let turns_codex =
            get_session_turns_token_stats(&conn, "codex", session_id, Some("codex-cli"), None)
                .unwrap();
        assert_eq!(cwd_codex.as_deref(), Some("/home/codex"));
        assert_eq!(model_codex.as_deref(), Some("o3"));
        assert_eq!(turns_codex[&1].0.total, 33);
    }

    /// Regression: the legacy `source_kind = None` fallback must pick a
    /// deterministic source even when there are multiple non-App,
    /// `source_dir_key IS NULL` rows for the same `(assistant, session_id,
    /// turn_no)`. The test inserts TWO non-App NULL-key sources (Copilot
    /// CLI and VS Code Chat) sharing the same turn number so the choice
    /// cannot be justified by a different turn_no alone.
    ///
    /// Coverage:
    /// - at least two non-App, `source_dir_key IS NULL` sources
    /// - same `turn_no` (so the tie-break is not happening on turn_no)
    /// - multiple `source_kind = None` calls must all pick the same source
    /// - identity, CWD, model, and turn stats must all pick the same source
    ///   for the same `source_kind = None` legacy call.
    #[test]
    fn session_queries_legacy_none_is_deterministic_with_multiple_null_key_sources() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        let session_id = "iso-legacy-multi-null";

        // Insert two non-App, source_dir_key = NULL rows with the same
        // turn_no=1. The deterministic tie-break must pick one consistently.
        // Insertion order: VS Code Chat first (alphabetically later), then
        // Copilot CLI (alphabetically earlier). With the deterministic
        // ORDER BY (source_kind ASC), Copilot CLI must win because
        // "copilot-cli" sorts before "vscode-chat".
        for (source_kind, model, cwd, total) in [
            ("vscode-chat", "GPT-4.1", "/home/vscode", 110i64),
            ("copilot-cli", "GPT-5.4", "/home/cli", 55i64),
        ] {
            conn.execute(
                "INSERT INTO usage_entries (
                    assistant_type, source_kind, source_dir_key, timestamp, date,
                    session_id, turn_no, model, cwd,
                    tokens_input, tokens_output, tokens_total,
                    delta_input, delta_output, delta_total
                 ) VALUES (
                    'copilot', ?, NULL, '2026-07-20T10:00:00Z', '2026-07-20',
                    ?, 1, ?, ?,
                    ?, 5, ?,
                    ?, 5, ?
                 )",
                params![
                    source_kind,
                    session_id,
                    model,
                    cwd,
                    total - 5,
                    total,
                    total - 5,
                    total,
                ],
            )
            .unwrap();
        }

        // Helper closures so we can call each lookup many times and assert
        // they all observe the same source row.
        let pick_identity = || {
            get_session_assistant_and_transcript(&conn, "copilot", session_id, None, None)
                .map(|(_, _, sk, sdk, _, _)| (sk, sdk))
        };
        let pick_cwd = || get_session_cwd(&conn, "copilot", session_id, None, None);
        let pick_model = || get_session_model(&conn, "copilot", session_id, None, None);
        let pick_turns = || get_session_turns_token_stats(&conn, "copilot", session_id, None, None);

        // All four lookups must agree on the same source row.
        let (id_sk_1, id_sdk_1) = pick_identity().unwrap();
        let cwd_1 = pick_cwd().unwrap();
        let model_1 = pick_model().unwrap();
        let turns_1 = pick_turns().unwrap();

        // The tie-break is `source_kind ASC` → "copilot-cli" wins.
        assert_eq!(id_sk_1, "copilot-cli");
        assert_eq!(id_sdk_1, None);
        assert_eq!(cwd_1.as_deref(), Some("/home/cli"));
        assert_eq!(model_1.as_deref(), Some("GPT-5.4"));
        assert_eq!(turns_1.len(), 1);
        assert_eq!(turns_1[&1].0.total, 55);

        // Calling the same legacy lookup several more times must yield the
        // exact same result. This is the core determinism guarantee: the
        // choice is never dependent on arbitrary row order, even with two
        // matching non-App NULL-key sources.
        for _ in 0..5 {
            let (id_sk, id_sdk) = pick_identity().unwrap();
            let cwd = pick_cwd().unwrap();
            let model = pick_model().unwrap();
            let turns = pick_turns().unwrap();

            assert_eq!(id_sk, "copilot-cli");
            assert_eq!(id_sdk, None);
            assert_eq!(cwd.as_deref(), Some("/home/cli"));
            assert_eq!(model.as_deref(), Some("GPT-5.4"));
            assert_eq!(turns.len(), 1);
            assert_eq!(turns[&1].0.total, 55);
        }
    }

    /// Regression: when an App row (`source_dir_key IS NOT NULL`) and a
    /// non-App row (`source_dir_key IS NULL`) both exist for the same
    /// `(assistant, session_id, turn_no)`, the legacy `source_kind = None`
    /// fallback must prefer the non-App row because the WHERE clause
    /// explicitly filters by `source_dir_key IS NULL`. The tie-break is
    /// therefore `source_kind ASC` within the non-App NULL-key rows; the
    /// App row is invisible to the legacy caller.
    ///
    /// This complements the test above by ensuring the App row never leaks
    /// into a legacy `None` lookup even when the same turn_no exists on
    /// both sides.
    #[test]
    fn session_queries_legacy_none_excludes_app_row_when_null_key_alternative_exists() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        let session_id = "iso-legacy-app-vs-null";

        // App row with same turn_no=1 and a different model / cwd.
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, source_dir_key, timestamp, date,
                session_id, turn_no, model, cwd,
                tokens_input, tokens_output, tokens_total,
                delta_input, delta_output, delta_total
             ) VALUES (
                'copilot', 'copilot-app', 'dead00', '2026-07-20T10:00:00Z', '2026-07-20',
                ?, 1, 'GLM5.2-high', '/home/app',
                100, 10, 110,
                100, 10, 110
             )",
            params![session_id],
        )
        .unwrap();

        // Non-App NULL-key row (Copilot CLI) with same turn_no=1.
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, source_dir_key, timestamp, date,
                session_id, turn_no, model, cwd,
                tokens_input, tokens_output, tokens_total,
                delta_input, delta_output, delta_total
             ) VALUES (
                'copilot', 'copilot-cli', NULL, '2026-07-20T10:00:00Z', '2026-07-20',
                ?, 1, 'GPT-5.4', '/home/cli',
                50, 5, 55,
                50, 5, 55
             )",
            params![session_id],
        )
        .unwrap();

        // Legacy `None` lookup must see only the CLI row, never the App row.
        let (_, _, sk, sdk, _, _) =
            get_session_assistant_and_transcript(&conn, "copilot", session_id, None, None).unwrap();
        assert_eq!(sk, "copilot-cli");
        assert_eq!(sdk, None);

        let cwd = get_session_cwd(&conn, "copilot", session_id, None, None).unwrap();
        assert_eq!(cwd.as_deref(), Some("/home/cli"));

        let model = get_session_model(&conn, "copilot", session_id, None, None).unwrap();
        assert_eq!(model.as_deref(), Some("GPT-5.4"));

        let turns =
            get_session_turns_token_stats(&conn, "copilot", session_id, None, None).unwrap();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[&1].0.total, 55);
    }

    /// Regression: when a single `session_id` carries rows from two different
    /// non-App collectors — `copilot-cli` turn 1 and `vscode-chat` turn 2,
    /// both with `source_dir_key IS NULL` — the legacy `source_kind = None`
    /// fallback must NOT mix turns from different sources into a single
    /// turn-stats map. The fallback must first resolve a single
    /// `source_kind` (deterministically via `source_kind ASC` → `copilot-cli`)
    /// and then return only the rows for that resolved source. Otherwise
    /// `get_session_turns_token_stats` would return a map that contains
    /// CLI turn 1 *and* VS Code Chat turn 2, violating the "legacy
    /// fallback must pick a single source" contract even though the
    /// helper would have reported the identity as CLI.
    ///
    /// Coverage:
    /// - same `assistant_type` (`copilot`)
    /// - same `session_id`
    /// - two different `source_kind` (`copilot-cli`, `vscode-chat`)
    /// - both `source_dir_key IS NULL`
    /// - different `turn_no` (CLI=1, VS Code=2) — the per-turn "first row
    ///   encountered" rule would otherwise keep BOTH turns when ordered
    ///   by `turn_no ASC`
    /// - different `model` / `cwd` / token totals
    /// - legacy `source_kind = None` must report CLI as the resolved
    ///   source for identity, CWD, model, and turn stats
    /// - turn stats must contain only the CLI turn 1 row, never the VS
    ///   Code Chat turn 2 row
    #[test]
    fn session_queries_legacy_none_picks_single_source_for_interleaved_turns() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        let session_id = "iso-legacy-interleaved";

        // Copilot CLI turn 1 (NULL source_dir_key). Inserted first; its
        // turn_no is the only one that should survive in the turn-stats
        // map after the legacy fallback.
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, source_dir_key, timestamp, date,
                session_id, turn_no, model, cwd,
                tokens_input, tokens_output, tokens_total,
                delta_input, delta_output, delta_total
             ) VALUES (
                'copilot', 'copilot-cli', NULL, '2026-07-20T10:00:00Z', '2026-07-20',
                ?, 1, 'GPT-5.4', '/home/cli',
                50, 5, 55,
                50, 5, 55
             )",
            params![session_id],
        )
        .unwrap();

        // VS Code Chat turn 2 (also NULL source_dir_key) for the same
        // session_id. This row must NEVER appear in the legacy
        // turn-stats map once the resolver has picked `copilot-cli`.
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, source_dir_key, timestamp, date,
                session_id, turn_no, model, cwd,
                tokens_input, tokens_output, tokens_total,
                delta_input, delta_output, delta_total
             ) VALUES (
                'copilot', 'vscode-chat', NULL, '2026-07-20T10:01:00Z', '2026-07-20',
                ?, 2, 'GPT-4.1', '/home/vscode',
                100, 10, 110,
                100, 10, 110
             )",
            params![session_id],
        )
        .unwrap();

        // Identity, CWD, model all must come from the CLI source.
        let (_, _, sk, sdk, _, _) =
            get_session_assistant_and_transcript(&conn, "copilot", session_id, None, None).unwrap();
        assert_eq!(sk, "copilot-cli");
        assert_eq!(sdk, None);

        let cwd = get_session_cwd(&conn, "copilot", session_id, None, None).unwrap();
        assert_eq!(cwd.as_deref(), Some("/home/cli"));

        let model = get_session_model(&conn, "copilot", session_id, None, None).unwrap();
        assert_eq!(model.as_deref(), Some("GPT-5.4"));

        // Turn stats must contain ONLY the CLI turn 1 row. The VS Code
        // Chat turn 2 row must not appear in the map at all because
        // its source_kind differs from the resolved `copilot-cli`.
        let turns =
            get_session_turns_token_stats(&conn, "copilot", session_id, None, None).unwrap();
        assert_eq!(
            turns.len(),
            1,
            "legacy turn stats must not mix turns from different sources"
        );
        assert!(turns.contains_key(&1), "CLI turn 1 must be present");
        assert!(
            !turns.contains_key(&2),
            "VS Code Chat turn 2 must be excluded by the resolved source_kind filter"
        );
        assert_eq!(turns[&1].0.total, 55);
        assert_eq!(turns[&1].1, "GPT-5.4");
    }

    /// Regression: the legacy `source_kind = None` fallback must prefer a
    /// main agent row (`parent_session_id IS NULL`) over a subagent
    /// synthetic row (`parent_session_id IS NOT NULL`) for the same
    /// `(assistant_type, source_kind, source_dir_key, session_id)`, even
    /// when the subagent row has an earlier `turn_no`. The old
    /// `ORDER BY (parent_session_id IS NULL) ASC` actually returned
    /// subagent rows first (because `IS NULL` evaluates to `1` while
    /// `IS NOT NULL` evaluates to `0` and `ASC` puts `0` first), so the
    /// subagent row would be picked even when a main row existed.
    ///
    /// Coverage:
    /// - same `assistant_type`, same `session_id`, same `source_kind`,
    ///   same `source_dir_key`
    /// - subagent row: `parent_session_id = Some("parent-id")`,
    ///   `turn_no = 1` (earlier turn)
    /// - main row: `parent_session_id = NULL`, `turn_no = 2` (later turn)
    /// - different model / cwd / token totals so the assertions can
    ///   distinguish which row was selected
    /// - different `turn_no` keeps the partial unique index
    ///   `uidx_assistant_source_session_turn` happy (the index is
    ///   `assistant_type, source_kind, session_id, turn_no`).
    #[test]
    fn session_queries_legacy_none_prefers_main_over_subagent_row() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        let session_id = "iso-legacy-main-vs-subagent";

        // Subagent synthetic row: `parent_session_id` is set, turn_no = 1
        // (earlier turn), with a distinct model and cwd. If the helper
        // ever picks this row, the assertions below will fail.
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, source_dir_key, parent_session_id,
                agent_nickname, timestamp, date,
                session_id, turn_no, model, cwd,
                tokens_input, tokens_output, tokens_total,
                delta_input, delta_output, delta_total
             ) VALUES (
                'copilot', 'copilot-app', 'dead00', 'parent-id',
                'call_sub', '2026-07-20T10:00:00Z', '2026-07-20',
                ?, 1, 'K2.7-code', '/home/subagent',
                30, 3, 33,
                30, 3, 33
             )",
            params![session_id],
        )
        .unwrap();

        // Main agent row: `parent_session_id` is NULL, turn_no = 2
        // (later turn), with a distinct model and cwd. This is the row
        // the helper MUST select.
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, source_dir_key, parent_session_id,
                agent_nickname, timestamp, date,
                session_id, turn_no, model, cwd,
                tokens_input, tokens_output, tokens_total,
                delta_input, delta_output, delta_total
             ) VALUES (
                'copilot', 'copilot-app', 'dead00', NULL,
                NULL, '2026-07-20T10:01:00Z', '2026-07-20',
                ?, 2, 'GLM5.2-high', '/home/main',
                100, 10, 110,
                100, 10, 110
             )",
            params![session_id],
        )
        .unwrap();

        // Use the legacy `source_kind = None` fallback while scoping the
        // query to the App directory. The resolver must still prefer the
        // main row over the subagent row.
        let (_, path, sk, sdk, parent, agent) = get_session_assistant_and_transcript(
            &conn,
            "copilot",
            session_id,
            None,
            Some("dead00"),
        )
        .unwrap();
        assert_eq!(sk, "copilot-app");
        assert_eq!(sdk.as_deref(), Some("dead00"));
        assert_eq!(
            parent, None,
            "main row must win: parent_session_id must be NULL"
        );
        assert_eq!(
            agent, None,
            "main row must win: agent_nickname must be None"
        );
        // transcript_path can be NULL in this fixture; we only assert
        // that we got *some* row back (None is acceptable here).
        let _ = path;

        // CWD must come from the main row (`/home/main`), not the
        // subagent row (`/home/subagent`).
        let cwd = get_session_cwd(&conn, "copilot", session_id, None, Some("dead00")).unwrap();
        assert_eq!(cwd.as_deref(), Some("/home/main"));

        // Model must come from the main row (`GLM5.2-high`), not the
        // subagent row (`K2.7-code`).
        let model = get_session_model(&conn, "copilot", session_id, None, Some("dead00")).unwrap();
        assert_eq!(model.as_deref(), Some("GLM5.2-high"));

        // Turn stats must contain both turns, but the per-turn first-row
        // rule must pick the main row (`turn_no = 2`, total 110) for
        // turn 2 and the subagent row (`turn_no = 1`, total 33) for
        // turn 1 — the only row that exists for turn 1. The critical
        // assertion is turn 2: it must reflect the main row's totals,
        // not be missing because of an off-by-one ordering bug.
        let turns =
            get_session_turns_token_stats(&conn, "copilot", session_id, None, Some("dead00"))
                .unwrap();
        assert_eq!(turns.len(), 2, "must contain both turn 1 and turn 2");
        assert_eq!(turns[&1].0.total, 33);
        assert_eq!(turns[&1].1, "K2.7-code");
        assert_eq!(
            turns[&2].0.total, 110,
            "turn 2 must come from the main row, not the subagent row"
        );
        assert_eq!(turns[&2].1, "GLM5.2-high");
    }

    #[test]
    fn grok_parser_migration_resets_existing_sync_state() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        conn.execute(
            "DELETE FROM sync_state WHERE filename = ?",
            params![GROK_PARSER_MIGRATION_KEY],
        )
        .unwrap();
        for legacy_key in LEGACY_GROK_PARSER_MIGRATION_KEYS {
            conn.execute(
                "INSERT INTO sync_state (filename, last_synced_size, last_synced_time)
                 VALUES (?, 1, 0)",
                params![legacy_key],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO sync_state (filename, last_synced_size, last_synced_time)
             VALUES ('grok:sessions/work/updates.jsonl', 123, 456)",
            [],
        )
        .unwrap();

        init_db(&conn).unwrap();

        let stale_state_count: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_state WHERE filename LIKE 'grok:%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stale_state_count, 0);

        let migration_done: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_state WHERE filename = ?)",
                params![GROK_PARSER_MIGRATION_KEY],
                |row| row.get(0),
            )
            .unwrap();
        assert!(migration_done);

        for legacy_key in LEGACY_GROK_PARSER_MIGRATION_KEYS {
            let legacy_marker_count: u64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sync_state WHERE filename = ?",
                    params![legacy_key],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                legacy_marker_count, 0,
                "legacy marker should be removed: {legacy_key}"
            );
        }
    }

    #[test]
    fn sync_grok_usage_logs_rebuilds_session_and_keeps_reported_cost() {
        let root = temp_jsonl_path("grok-sync");
        let session_dir = root.join("sessions/work/grok-session");
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(
            session_dir.join("summary.json"),
            r#"{"info":{"cwd":"/tmp/grok-project"},"current_model_id":"grok-4.5","reasoning_effort":"high","generated_title":"Grok sync test"}"#,
        )
        .unwrap();
        fs::write(
            session_dir.join("updates.jsonl"),
            concat!(
                r#"{"timestamp":1710000000,"params":{"update":{"sessionUpdate":"turn_started","turn_number":0}}}"#, "\n",
                r#"{"timestamp":1710000001,"params":{"update":{"sessionUpdate":"user_message_chunk","content":{"text":"sync"}}}}"#, "\n",
                r#"{"timestamp":1710000002,"params":{"update":{"sessionUpdate":"turn_completed","usage":{"input_tokens":80,"cache_read_input_tokens":20,"output_tokens":20,"total_tokens":120},"total_cost_usd":0.00024}}}"#, "\n"
            ),
        )
        .unwrap();

        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        sync_grok_usage_logs(&mut conn, &root).unwrap();

        let (
            count,
            source_kind,
            delta_input,
            delta_cache_read,
            delta_total,
            reported_cost,
            reasoning_effort,
        ): (u64, String, u64, u64, u64, f64, Option<String>) = conn
            .query_row(
                "SELECT COUNT(*), source_kind, delta_input, delta_cache_read,
                        delta_total, reported_cost_usd, reasoning_effort
                 FROM usage_entries WHERE assistant_type = 'grok'",
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
        assert_eq!(count, 1);
        assert_eq!(source_kind, crate::grok::USAGE_SOURCE_KIND);
        assert_eq!(delta_input, 80);
        assert_eq!(delta_cache_read, 20);
        assert_eq!(delta_total, 120);
        assert!((reported_cost - 0.00024).abs() < f64::EPSILON);
        assert_eq!(reasoning_effort.as_deref(), Some("High"));

        let date_rows = get_usage_entries_by_date(&conn, "2024-03-09", "grok").unwrap();
        assert_eq!(date_rows.len(), 1);
        assert_eq!(
            date_rows[0]
                .0
                .entry
                .cost
                .as_ref()
                .and_then(|cost| cost.reported_cost_usd),
            Some(0.00024)
        );

        let month_rows = get_usage_entries_by_month(&conn, "2024-03", "grok").unwrap();
        assert_eq!(
            month_rows[0]
                .0
                .cost
                .as_ref()
                .and_then(|cost| cost.reported_cost_usd),
            Some(0.00024)
        );

        let year_rows = get_usage_entries_by_year(&conn, "2024", "grok").unwrap();
        assert_eq!(
            year_rows[0]
                .0
                .cost
                .as_ref()
                .and_then(|cost| cost.reported_cost_usd),
            Some(0.00024)
        );

        let exported = export_usage_day_entries(&conn, "grok", "2024-03-09").unwrap();
        assert_eq!(exported.len(), 1);
        let mut imported_conn = Connection::open_in_memory().unwrap();
        init_db(&imported_conn).unwrap();
        import_usage_day_entries(
            &mut imported_conn,
            "grok",
            "2024-03-09",
            exported,
            UsageImportMetadata::default(),
        )
        .unwrap();
        let imported_cost: f64 = imported_conn
            .query_row(
                "SELECT reported_cost_usd FROM usage_entries WHERE assistant_type = 'grok'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!((imported_cost - 0.00024).abs() < f64::EPSILON);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sync_pi_usage_logs_persists_turn_and_reported_cost() {
        let root = temp_jsonl_path("pi-sync");
        let session_dir = root
            .join("agent")
            .join("sessions")
            .join("--tmp--pi-project");
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(
            session_dir.join("2024-12-03T14-00-00_abc.jsonl"),
            concat!(
                r#"{"type":"session","version":3,"id":"pi-sess-1","timestamp":"2024-12-03T14:00:00.000Z","cwd":"/tmp/pi-project"}"#, "\n",
                r#"{"type":"message","id":"m1","parentId":null,"timestamp":"2024-12-03T14:00:01.000Z","message":{"role":"user","content":"Hello"}}"#, "\n",
                r#"{"type":"message","id":"m2","parentId":"m1","timestamp":"2024-12-03T14:00:02.000Z","message":{"role":"assistant","content":[{"type":"text","text":"Hi!"}],"provider":"anthropic","model":"claude-sonnet-4-5","usage":{"input":100,"output":50,"cacheRead":10,"totalTokens":150,"cost":{"total":0.0031}},"stopReason":"stop"}}"#, "\n"
            ),
        )
        .unwrap();

        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        sync_pi_usage_logs(&mut conn, &root).unwrap();

        let (count, source_kind, delta_input, delta_total, reported_cost, model): (
            u64,
            String,
            u64,
            u64,
            f64,
            String,
        ) = conn
            .query_row(
                "SELECT COUNT(*), source_kind, delta_input, delta_total, reported_cost_usd, model
                 FROM usage_entries WHERE assistant_type = 'pi'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(source_kind, crate::pi::SOURCE_KIND);
        assert_eq!(delta_input, 100);
        assert_eq!(delta_total, 150);
        assert!((reported_cost - 0.0031).abs() < f64::EPSILON);
        assert_eq!(model, "claude-sonnet-4-5");

        let date_rows = get_usage_entries_by_date(&conn, "2024-12-03", "pi").unwrap();
        assert_eq!(date_rows.len(), 1);

        // Re-syncing an unchanged file must not duplicate rows.
        sync_pi_usage_logs(&mut conn, &root).unwrap();
        let count_after_resync: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_entries WHERE assistant_type = 'pi'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count_after_resync, 1);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sync_omp_usage_logs_persists_turn_with_omp_source_kind() {
        let root = temp_jsonl_path("omp-sync");
        let session_dir = root
            .join("agent")
            .join("sessions")
            .join("--tmp--omp-project");
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(
            session_dir.join("2024-12-03T14-00-00_def.jsonl"),
            concat!(
                r#"{"type":"session","version":3,"id":"omp-sess-1","timestamp":"2024-12-03T14:00:00.000Z","cwd":"/tmp/omp-project"}"#, "\n",
                r#"{"type":"message","id":"m2","parentId":null,"timestamp":"2024-12-03T14:00:02.000Z","message":{"role":"assistant","content":[{"type":"text","text":"Hi!"}],"provider":"anthropic","model":"claude-sonnet-4-5","usage":{"input":10,"output":5,"totalTokens":15,"cost":{"total":0.0005}},"stopReason":"stop"}}"#, "\n"
            ),
        )
        .unwrap();

        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        sync_omp_usage_logs(&mut conn, &root).unwrap();

        let (count, source_kind, reported_cost): (u64, String, f64) = conn
            .query_row(
                "SELECT COUNT(*), source_kind, reported_cost_usd
                 FROM usage_entries WHERE assistant_type = 'omp'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(source_kind, crate::omp::SOURCE_KIND);
        assert!((reported_cost - 0.0005).abs() < f64::EPSILON);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sync_grok_multi_model_turn_survives_database_and_timeline() {
        let root = temp_jsonl_path("grok-multi-model-sync").with_extension("");
        let session_id = "grok-multi-model";
        let session_dir = root.join("sessions/work").join(session_id);
        fs::create_dir_all(&session_dir).unwrap();
        let updates_path = session_dir.join("updates.jsonl");
        fs::write(
            &updates_path,
            concat!(
                r#"{"timestamp":1710000000,"params":{"update":{"sessionUpdate":"turn_started","turn_number":0}}}"#, "\n",
                r#"{"timestamp":1710000001,"params":{"update":{"sessionUpdate":"user_message_chunk","content":{"text":"multi model"}}}}"#, "\n",
                r#"{"timestamp":1710000002,"params":{"update":{"sessionUpdate":"agent_message_chunk","content":{"text":"done"}}}}"#, "\n",
                r#"{"timestamp":1710000003,"params":{"update":{"sessionUpdate":"turn_completed","usage":{"inputTokens":300,"outputTokens":60,"totalTokens":360,"modelUsage":{"grok-4.5":{"inputTokens":100,"outputTokens":20,"totalTokens":120,"costUSD":0.01},"grok-build-0.1":{"inputTokens":200,"outputTokens":40,"totalTokens":240,"costUSD":0.02}}}}}}"#, "\n"
            ),
        )
        .unwrap();

        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        sync_grok_usage_logs(&mut conn, &root).unwrap();

        let mut stmt = conn
            .prepare(
                "SELECT model_id, delta_total, reported_cost_usd, usage_identity
                 FROM usage_entries
                 WHERE assistant_type = 'grok' AND session_id = ?
                 ORDER BY model_id",
            )
            .unwrap();
        let rows: Vec<(String, u64, f64, String)> = stmt
            .query_map(params![session_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            rows,
            [
                (
                    "grok-4.5".to_string(),
                    120,
                    0.01,
                    "model:grok-4.5".to_string()
                ),
                (
                    "grok-build-0.1".to_string(),
                    240,
                    0.02,
                    "model:grok-build-0.1".to_string()
                )
            ]
        );

        let turn_entries = get_session_turns_token_stats(
            &conn,
            "grok",
            session_id,
            Some(crate::grok::USAGE_SOURCE_KIND),
            None,
        )
        .unwrap();
        let (turn_stats, turn_models) = &turn_entries[&1];
        assert_eq!(turn_stats.input, 300);
        assert_eq!(turn_stats.output, 60);
        assert_eq!(turn_stats.total, 360);
        assert!(turn_models.contains("Grok 4.5"));
        assert!(turn_models.contains("Grok Build 0.1"));

        let exported = export_usage_day_entries(&conn, "grok", "2024-03-09").unwrap();
        assert_eq!(exported.len(), 2);
        let mut imported_conn = Connection::open_in_memory().unwrap();
        init_db(&imported_conn).unwrap();
        let import_summary = import_usage_day_entries(
            &mut imported_conn,
            "grok",
            "2024-03-09",
            exported,
            UsageImportMetadata::default(),
        )
        .unwrap();
        assert_eq!(import_summary.imported, 2);
        let imported: (u64, f64) = imported_conn
            .query_row(
                "SELECT COUNT(*), SUM(reported_cost_usd)
                 FROM usage_entries WHERE assistant_type = 'grok'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(imported.0, 2);
        assert!((imported.1 - 0.03).abs() < 1e-12);

        let _ = fs::remove_dir_all(root);
    }
}
