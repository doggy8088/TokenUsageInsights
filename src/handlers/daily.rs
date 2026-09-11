use axum::{
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use std::{
    collections::HashMap,
    fs::File,
    io::BufReader,
    path::{Path as StdPath, PathBuf},
};

use super::*;
use crate::db::{self, TokenStats};
use crate::pricing::{load_prepared_pricing_rules, PreparedPricingRules};
use crate::timeline::{
    parse_antigravity_timeline, parse_claude_timeline, parse_codex_timeline,
    parse_copilot_timeline_filtered, parse_cursor_timeline, parse_grok_timeline,
    parse_muse_timeline, parse_omp_timeline, parse_pi_timeline, parse_vscode_timeline,
    TimelineItem,
};

fn add_usage_to_day_summary(summary: &mut DaySummary, usage: &UsageAggregation) {
    summary.total_tokens += usage.total_tokens;
    summary.total_input_tokens += usage.input_tokens;
    summary.total_output_tokens += usage.output_tokens;
    summary.total_cache_read_tokens += usage.cache_read_tokens;
    summary.total_cache_write_tokens += usage.cache_write_tokens;
    summary.total_reasoning_tokens += usage.reasoning_tokens;
    summary.total_cost_usd += usage.cost_usd;
}

fn aggregate_usage_details(
    entries_with_type: &[(crate::db::UsageDayExportRecord, String)],
    pricing_rules: &PreparedPricingRules,
) -> (DaySummary, Vec<SessionSummary>, Vec<UsageEntry>) {
    let mut summary = DaySummary::default();
    // Session identity = (source_kind, session_id, source_dir_key) so that rows from
    // different sources (copilot-cli, copilot-app, vscode-chat) with the same session_id
    // are not merged, and different COPILOT_APP_DIR values remain isolated.
    type SessionKey = (String, String, Option<String>);
    let mut sessions_map: HashMap<SessionKey, (Vec<UsageEntry>, String)> = HashMap::new();
    let mut entries = Vec::new();

    for (record, ast_type) in entries_with_type {
        let e = &record.entry;
        entries.push(e.clone());
        let source_kind = e
            .source_kind
            .clone()
            .unwrap_or_else(|| "legacy".to_string());
        let key = (source_kind, e.session_id.clone(), e.source_dir_key.clone());
        let (list, _stored_ast_type) = sessions_map
            .entry(key)
            .or_insert_with(|| (Vec::new(), ast_type.clone()));
        list.push(e.clone());
    }

    summary.total_sessions = sessions_map.len();
    let mut session_last_entries: HashMap<SessionKey, UsageEntry> = HashMap::new();
    for e in &entries {
        let source_kind = e
            .source_kind
            .clone()
            .unwrap_or_else(|| "legacy".to_string());
        let key = (source_kind, e.session_id.clone(), e.source_dir_key.clone());
        let last_e = session_last_entries.entry(key).or_insert_with(|| e.clone());
        if e.turn_no > last_e.turn_no {
            *last_e = e.clone();
        }
    }

    let mut sessions_summary = Vec::new();
    for ((source_kind, session_id, source_dir_key), (s_entries, ast_type)) in &sessions_map {
        let key = (
            source_kind.clone(),
            session_id.clone(),
            source_dir_key.clone(),
        );
        let last_entry = session_last_entries
            .get(&key)
            .cloned()
            .unwrap_or_else(|| s_entries[0].clone());
        let session_usage = summarize_session_usage(pricing_rules, s_entries);

        let session_duration = last_entry
            .cost
            .as_ref()
            .and_then(|c| c.total_api_duration_ms)
            .unwrap_or(0.0) as u64;
        let session_requests = last_entry
            .cost
            .as_ref()
            .and_then(|c| c.total_premium_requests)
            .unwrap_or(0.0) as u64;

        summary.total_duration_ms += session_duration;
        summary.total_requests += session_requests;

        add_usage_to_day_summary(&mut summary, &session_usage.usage);

        sessions_summary.push(SessionSummary {
            session_id: session_id.clone(),
            session_name: last_entry
                .session_name
                .unwrap_or_else(|| "Start Coding Session".to_string()),
            assistant_type: ast_type.clone(),
            source_kind: source_kind.clone(),
            source_dir_key: source_dir_key.clone(),
            cwd: last_entry.cwd.unwrap_or_default(),
            model: session_usage.display_model,
            total_tokens: session_usage.usage.total_tokens,
            total_input_tokens: session_usage.usage.input_tokens,
            total_output_tokens: session_usage.usage.output_tokens,
            total_cache_read_tokens: session_usage.usage.cache_read_tokens,
            total_cache_write_tokens: session_usage.usage.cache_write_tokens,
            total_reasoning_tokens: session_usage.usage.reasoning_tokens,
            max_turn_no: s_entries.iter().map(|e| e.turn_no).max().unwrap_or(1),
            timestamp: s_entries[0].timestamp.clone(),
            duration_ms: session_duration,
            total_requests: session_requests,
            cost_usd: session_usage.usage.cost_usd,
            parent_session_id: last_entry.parent_session_id.clone(),
            agent_nickname: last_entry.agent_nickname.clone(),
            agent_role: last_entry.agent_role.clone(),
            reasoning_effort: last_entry.reasoning_effort.clone(),
        });
    }

    sessions_summary.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    (summary, sessions_summary, entries)
}

fn is_safe_session_id(session_id: &str) -> bool {
    if session_id.is_empty() || session_id.len() > 128 {
        return false;
    }

    if session_id == "." || session_id == ".." {
        return false;
    }

    session_id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.')
}

fn resolve_claude_transcript_path(
    claude_dir: &StdPath,
    session_id: &str,
    transcript_path_db: &str,
) -> Result<PathBuf, String> {
    let mut path = PathBuf::from(transcript_path_db);
    if path.is_relative() {
        path = claude_dir.join(path);
    }

    if !path.exists() {
        return Err("找不到該會話的本地日誌檔案。".to_string());
    }

    let claude_root = claude_dir
        .canonicalize()
        .map_err(|_| "無法存取 Claude Code 根目錄。".to_string())?;
    let canonical_path = path
        .canonicalize()
        .map_err(|_| "無法解析會話日誌路徑。".to_string())?;

    if !canonical_path.starts_with(claude_root) {
        return Err("會話日誌路徑不在預期目錄內。".to_string());
    }

    let file_name = canonical_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if !file_name.contains(session_id) {
        return Err("會話日誌路徑與 session id 不一致。".to_string());
    }

    Ok(canonical_path)
}

fn resolve_codex_transcript_path(
    codex_dir: &StdPath,
    transcript_path_db: &str,
) -> Result<PathBuf, String> {
    let mut path = PathBuf::from(transcript_path_db);
    if path.is_relative() {
        path = codex_dir.join(path);
    }

    if !path.exists() {
        return Err("找不到該 Codex 會話的本地日誌檔案。".to_string());
    }

    let codex_root = codex_dir
        .canonicalize()
        .map_err(|_| "無法存取 Codex 根目錄。".to_string())?;
    let canonical_path = path
        .canonicalize()
        .map_err(|_| "無法解析 Codex 會話日誌路徑。".to_string())?;

    if !canonical_path.starts_with(codex_root) {
        return Err("Codex 會話日誌路徑不在預期目錄內。".to_string());
    }

    Ok(canonical_path)
}

fn resolve_pi_family_transcript_path(
    base_dir: &StdPath,
    assistant_label: &str,
    transcript_path_db: &str,
) -> Result<PathBuf, String> {
    let mut path = PathBuf::from(transcript_path_db);
    if path.is_relative() {
        path = base_dir.join(path);
    }

    if !path.exists() {
        return Err(format!(
            "找不到該 {assistant_label} session 的本地日誌檔案。"
        ));
    }

    let base_root = base_dir
        .canonicalize()
        .map_err(|_| format!("無法存取 {assistant_label} 根目錄。"))?;
    let canonical_path = path
        .canonicalize()
        .map_err(|_| format!("無法解析 {assistant_label} session 日誌路徑。"))?;

    if !canonical_path.starts_with(&base_root) {
        return Err(format!(
            "{assistant_label} session 日誌路徑不在預期目錄內。"
        ));
    }
    if canonical_path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
        return Err(format!("{assistant_label} session 日誌格式不受支援。"));
    }

    Ok(canonical_path)
}

fn resolve_cursor_transcript_path(
    cursor_dir: &StdPath,
    session_id: &str,
    transcript_path_db: &str,
) -> Result<PathBuf, String> {
    let mut path = PathBuf::from(transcript_path_db);
    if path.is_relative() {
        path = cursor_dir.join(path);
    }

    if !path.exists() {
        return Err("找不到該 Cursor 會話的本地日誌檔案。".to_string());
    }

    let cursor_root = cursor_dir
        .canonicalize()
        .map_err(|_| "無法存取 Cursor 根目錄。".to_string())?;
    let canonical_path = path
        .canonicalize()
        .map_err(|_| "無法解析 Cursor 會話日誌路徑。".to_string())?;

    if !canonical_path.starts_with(cursor_root) {
        return Err("Cursor 會話日誌路徑不在預期目錄內。".to_string());
    }

    let file_name = canonical_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if !file_name.contains(session_id) {
        return Err("會話日誌路徑與 session id 不一致。".to_string());
    }

    Ok(canonical_path)
}

fn resolve_grok_transcript_path(
    grok_dir: &StdPath,
    session_id: &str,
    transcript_path_db: &str,
) -> Result<PathBuf, String> {
    let mut path = PathBuf::from(transcript_path_db);
    if path.is_relative() {
        path = grok_dir.join(path);
    }

    if !path.exists() {
        return Err("找不到該 Grok Build session 的本地日誌檔案。".to_string());
    }

    let grok_root = grok_dir
        .canonicalize()
        .map_err(|_| "無法存取 Grok Build 根目錄。".to_string())?;
    let canonical_path = path
        .canonicalize()
        .map_err(|_| "無法解析 Grok Build session 日誌路徑。".to_string())?;

    if !canonical_path.starts_with(&grok_root) {
        return Err("Grok Build session 日誌路徑不在預期目錄內。".to_string());
    }
    if canonical_path.file_name().and_then(|name| name.to_str()) != Some("updates.jsonl") {
        return Err("Grok Build session 日誌格式不受支援。".to_string());
    }
    if canonical_path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        != Some(session_id)
    {
        return Err("Grok Build session 日誌路徑與 session id 不一致。".to_string());
    }

    Ok(canonical_path)
}

fn resolve_vscode_transcript_path(transcript_path_db: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(transcript_path_db);
    if !path.exists() {
        return Err("找不到該 VS Code Copilot 聊天檔案。".to_string());
    }

    let canonical_path = path
        .canonicalize()
        .map_err(|_| "無法解析 VS Code Copilot 聊天檔案路徑。".to_string())?;
    let is_allowed = crate::vscode::discover_workspace_storage_roots()
        .into_iter()
        .filter_map(|root| root.canonicalize().ok())
        .any(|root| {
            canonical_path.starts_with(&root)
                && canonical_path
                    .parent()
                    .and_then(|parent| parent.file_name())
                    .and_then(|name| name.to_str())
                    == Some("chatSessions")
        });
    if !is_allowed {
        return Err("VS Code Copilot 聊天檔案不在允許的 workspaceStorage 目錄內。".to_string());
    }

    let extension = canonical_path.extension().and_then(|value| value.to_str());
    if !matches!(extension, Some("json") | Some("jsonl")) {
        return Err("VS Code Copilot 聊天檔案格式不受支援。".to_string());
    }
    Ok(canonical_path)
}

/// Resolve the `events.jsonl` path for a Copilot App session drawer request.
///
/// `events_session_id` is the *parent* (original main) session id when the
/// request is for a subagent synthetic session (`<main>__<agent_id>`), or the
/// session id itself for a main agent request. Callers MUST obtain this from
/// the database `parent_session_id` column rather than splitting the synthetic
/// id, so a tampered id cannot escape the session-state root.
///
/// `agent_nickname` is only used to craft a precise `content_unavailable`
/// reason when the file resolves but parsing the agent-specific slice yields no
/// timeline items; it does not affect path resolution.
///
/// Security: the canonicalized path must remain within the
/// `<copilot_app_dir>/session-state` root. Any traversal attempt (synthetic id
/// with `..`, symlinks pointing outside, ...) is rejected with `file_missing`.
fn resolve_copilot_app_events_path(
    copilot_app_dir: &StdPath,
    events_session_id: &str,
    agent_nickname: Option<&str>,
) -> Result<PathBuf, SessionFileErrorExt> {
    if !is_safe_session_id(events_session_id) {
        return Err(SessionFileErrorExt::with_reason(
            StatusCode::NOT_FOUND,
            "Copilot App session id 格式不正確，無法定位 events.jsonl。",
            "file_missing",
        ));
    }

    let session_state_root = copilot_app_dir.join("session-state");
    let session_dir = session_state_root.join(events_session_id);
    let events_path = session_dir.join("events.jsonl");

    // Canonicalize defensively. If the directory/file is missing we still want
    // a precise reason, so handle missing paths before canonicalization (which
    // would error on non-existent paths).
    if !events_path.exists() {
        let reason = if session_dir.exists() {
            "no_events_yet"
        } else {
            "file_missing"
        };
        return Err(SessionFileErrorExt::with_reason(
            StatusCode::NOT_FOUND,
            if reason == "no_events_yet" {
                "找不到 Copilot App session 的 events.jsonl（session 目錄存在但尚未產生事件檔）。"
                    .to_string()
            } else if agent_nickname.is_some() {
                "找不到 Copilot App session 的 events.jsonl（subagent 對應的主 session 目錄不存在）。".to_string()
            } else {
                "找不到 Copilot App session 的 events.jsonl。".to_string()
            },
            reason,
        ));
    }

    let root_canonical = match session_state_root.canonicalize() {
        Ok(p) => p,
        Err(_) => {
            return Err(SessionFileErrorExt::with_reason(
                StatusCode::NOT_FOUND,
                "無法存取 Copilot App session-state 根目錄。",
                "file_missing",
            ));
        }
    };
    let canonical_path = match events_path.canonicalize() {
        Ok(p) => p,
        Err(_) => {
            return Err(SessionFileErrorExt::with_reason(
                StatusCode::NOT_FOUND,
                "無法解析 Copilot App events.jsonl 路徑。",
                "file_missing",
            ));
        }
    };

    if !canonical_path.starts_with(&root_canonical) {
        return Err(SessionFileErrorExt::with_reason(
            StatusCode::NOT_FOUND,
            "Copilot App events.jsonl 路徑不在允許的 session-state 目錄內。",
            "file_missing",
        ));
    }

    // The final path component must be events.jsonl and its parent directory
    // name must equal the requested session id, preventing a symlinked file
    // from impersonating another session.
    let parent_name = canonical_path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str());
    if parent_name != Some(events_session_id) {
        return Err(SessionFileErrorExt::with_reason(
            StatusCode::NOT_FOUND,
            "Copilot App events.jsonl 路徑與 session id 不一致。",
            "file_missing",
        ));
    }
    let file_name = canonical_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    if file_name != "events.jsonl" {
        return Err(SessionFileErrorExt::with_reason(
            StatusCode::NOT_FOUND,
            "Copilot App session 路徑未指向 events.jsonl。",
            "file_missing",
        ));
    }

    Ok(canonical_path)
}

/// Resolve the `events.jsonl` path for a Copilot CLI subagent drawer request.
///
/// CLI subagent rows use a synthetic session id (`<parent_session_id>__<agent_id>`)
/// and share the parent's `events.jsonl` under
/// `<copilot_dir>/session-state/<parent_session_id>/events.jsonl`. The caller
/// MUST pass the database-sourced `parent_session_id` (never a string-split
/// synthetic id) so a tampered id cannot escape the session-state root.
///
/// Mirrors [`resolve_copilot_app_events_path`] security checks but against the
/// Copilot CLI directory ([`db::get_copilot_dir`]).
fn resolve_copilot_cli_subagent_events_path(
    copilot_dir: &StdPath,
    parent_session_id: &str,
) -> Result<PathBuf, SessionFileErrorExt> {
    if !is_safe_session_id(parent_session_id) {
        return Err(SessionFileErrorExt::with_reason(
            StatusCode::NOT_FOUND,
            "Copilot CLI subagent 的 parent session id 格式不正確，無法定位 events.jsonl。",
            "file_missing",
        ));
    }

    let session_state_root = copilot_dir.join("session-state");
    let session_dir = session_state_root.join(parent_session_id);
    let events_path = session_dir.join("events.jsonl");

    if !events_path.exists() {
        let reason = if session_dir.exists() {
            "no_events_yet"
        } else {
            "file_missing"
        };
        return Err(SessionFileErrorExt::with_reason(
            StatusCode::NOT_FOUND,
            if reason == "no_events_yet" {
                "找不到 Copilot CLI subagent 對應的主 session events.jsonl（主 session 目錄存在但尚未產生事件檔）。".to_string()
            } else {
                "找不到 Copilot CLI subagent 對應的主 session events.jsonl（subagent 對應的主 session 目錄不存在）。".to_string()
            },
            reason,
        ));
    }

    let root_canonical = match session_state_root.canonicalize() {
        Ok(p) => p,
        Err(_) => {
            return Err(SessionFileErrorExt::with_reason(
                StatusCode::NOT_FOUND,
                "無法存取 Copilot CLI session-state 根目錄。",
                "file_missing",
            ));
        }
    };
    let canonical_path = match events_path.canonicalize() {
        Ok(p) => p,
        Err(_) => {
            return Err(SessionFileErrorExt::with_reason(
                StatusCode::NOT_FOUND,
                "無法解析 Copilot CLI subagent events.jsonl 路徑。",
                "file_missing",
            ));
        }
    };

    if !canonical_path.starts_with(&root_canonical) {
        return Err(SessionFileErrorExt::with_reason(
            StatusCode::NOT_FOUND,
            "Copilot CLI subagent events.jsonl 路徑不在允許的 session-state 目錄內。",
            "file_missing",
        ));
    }

    let parent_name = canonical_path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str());
    if parent_name != Some(parent_session_id) {
        return Err(SessionFileErrorExt::with_reason(
            StatusCode::NOT_FOUND,
            "Copilot CLI subagent events.jsonl 路徑與 parent session id 不一致。",
            "file_missing",
        ));
    }
    let file_name = canonical_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    if file_name != "events.jsonl" {
        return Err(SessionFileErrorExt::with_reason(
            StatusCode::NOT_FOUND,
            "Copilot CLI subagent 路徑未指向 events.jsonl。",
            "file_missing",
        ));
    }

    Ok(canonical_path)
}

type SessionFileError = (StatusCode, String);

/// Extended error carrying a machine-readable `reason` code for the frontend.
/// `reason` is `None` for generic errors (the frontend falls back to a generic
/// "load failed" message) and `Some("no_events_yet" | "file_missing" |
/// "content_unavailable")` for Copilot App session drawer cases.
#[derive(Debug)]
struct SessionFileErrorExt {
    status: StatusCode,
    error: String,
    reason: Option<String>,
}

impl SessionFileErrorExt {
    fn new(status: StatusCode, error: impl Into<String>) -> Self {
        Self {
            status,
            error: error.into(),
            reason: None,
        }
    }

    fn with_reason(
        status: StatusCode,
        error: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            status,
            error: error.into(),
            reason: Some(reason.into()),
        }
    }
}

fn resolve_session_file_path(
    assistant: &str,
    session_id: &str,
    transcript_path_db: Option<&str>,
    source_kind: &str,
    copilot_app_parent_session_id: Option<&str>,
    copilot_app_agent_nickname: Option<&str>,
) -> Result<PathBuf, SessionFileErrorExt> {
    match assistant {
        "antigravity" => Ok(db::get_antigravity_dir()
            .join("brain")
            .join(session_id)
            .join(".system_generated/logs/transcript_full.jsonl")),
        "copilot" if source_kind == crate::vscode::SOURCE_KIND => {
            let path = transcript_path_db.ok_or_else(|| {
                SessionFileErrorExt::new(
                    StatusCode::NOT_FOUND,
                    "找不到 VS Code Copilot 聊天檔案路徑。",
                )
            })?;
            resolve_vscode_transcript_path(path)
                .map_err(|error| SessionFileErrorExt::new(StatusCode::BAD_REQUEST, error))
        }
        "copilot" if source_kind == "copilot-app" => resolve_copilot_app_events_path(
            &crate::paths::copilot_app_dir(),
            copilot_app_parent_session_id.unwrap_or(session_id),
            copilot_app_agent_nickname,
        ),
        "copilot" if source_kind == "copilot-cli" && copilot_app_parent_session_id.is_some() => {
            // CLI subagent synthetic session: locate the shared events.jsonl
            // under the parent session's directory, not the synthetic id's.
            resolve_copilot_cli_subagent_events_path(
                &db::get_copilot_dir(),
                copilot_app_parent_session_id.unwrap(),
            )
        }
        "copilot" => {
            let copilot_dir = db::get_copilot_dir();
            let events_path = copilot_dir
                .join("session-state")
                .join(session_id)
                .join("events.jsonl");
            if events_path.exists() {
                Ok(events_path)
            } else {
                Ok(copilot_dir
                    .join("session-state")
                    .join(format!("{session_id}.jsonl")))
            }
        }
        "codex" => {
            let path = transcript_path_db.ok_or_else(|| {
                SessionFileErrorExt::new(
                    StatusCode::NOT_FOUND,
                    "找不到 Codex 會話日誌檔案路徑。".to_string(),
                )
            })?;
            resolve_codex_transcript_path(&db::get_codex_dir(), path)
                .map_err(|error| SessionFileErrorExt::new(StatusCode::BAD_REQUEST, error))
        }
        "claude" => {
            let path = transcript_path_db.ok_or_else(|| {
                SessionFileErrorExt::new(
                    StatusCode::NOT_FOUND,
                    "找不到 Claude Code 會話日誌檔案路徑。",
                )
            })?;
            resolve_claude_transcript_path(&db::get_claude_dir(), session_id, path)
                .map_err(|error| SessionFileErrorExt::new(StatusCode::BAD_REQUEST, error))
        }
        "cursor" => {
            let path = transcript_path_db.ok_or_else(|| {
                SessionFileErrorExt::new(StatusCode::NOT_FOUND, "找不到 Cursor 會話日誌檔案路徑。")
            })?;
            resolve_cursor_transcript_path(&db::get_cursor_dir(), session_id, path)
                .map_err(|error| SessionFileErrorExt::new(StatusCode::BAD_REQUEST, error))
        }
        "grok" => {
            let path = transcript_path_db.ok_or_else(|| {
                SessionFileErrorExt::new(
                    StatusCode::NOT_FOUND,
                    "找不到 Grok Build session 日誌檔案路徑。".to_string(),
                )
            })?;
            resolve_grok_transcript_path(&db::get_grok_dir(), session_id, path)
                .map_err(|error| SessionFileErrorExt::new(StatusCode::BAD_REQUEST, error))
        }
        "pi" => {
            let path = transcript_path_db.ok_or_else(|| {
                SessionFileErrorExt::new(
                    StatusCode::NOT_FOUND,
                    "找不到 Pi Coding Agent session 日誌檔案路徑。".to_string(),
                )
            })?;
            resolve_pi_family_transcript_path(&db::get_pi_dir(), "Pi Coding Agent", path)
                .map_err(|error| SessionFileErrorExt::new(StatusCode::BAD_REQUEST, error))
        }
        "omp" => {
            let path = transcript_path_db.ok_or_else(|| {
                SessionFileErrorExt::new(
                    StatusCode::NOT_FOUND,
                    "找不到 OMP session 日誌檔案路徑。".to_string(),
                )
            })?;
            resolve_pi_family_transcript_path(&db::get_omp_dir(), "OMP", path)
                .map_err(|error| SessionFileErrorExt::new(StatusCode::BAD_REQUEST, error))
        }
        "muse" => {
            let path = transcript_path_db.ok_or_else(|| {
                SessionFileErrorExt::new(
                    StatusCode::NOT_FOUND,
                    "找不到 Muse session 日誌檔案路徑。".to_string(),
                )
            })?;
            resolve_pi_family_transcript_path(&db::get_muse_dir(), "Muse", path)
                .map_err(|error| SessionFileErrorExt::new(StatusCode::BAD_REQUEST, error))
        }
        _ => Err(SessionFileErrorExt::new(
            StatusCode::BAD_REQUEST,
            "不支援的助理類型",
        )),
    }
}

/// Aggregated per-session DB data fetched before parsing the timeline file:
/// per-turn token stats, the session cwd, and the canonical session model.
/// The model is the child session's own model for subagent synthetic rows,
/// used to seed the Copilot timeline parser so the subagent drawer shows the
/// child model instead of the shared parent `session.start.selectedModel`.
#[derive(Default)]
struct SessionDbData {
    db_entries: HashMap<u32, (TokenStats, String)>,
    session_cwd: Option<String>,
    session_model: Option<String>,
}

fn parse_session_timeline_file(
    assistant: &str,
    source_kind: &str,
    filepath: &StdPath,
    db_entries: &HashMap<u32, (TokenStats, String)>,
    copilot_app_agent_filter: Option<&str>,
    copilot_session_model: Option<&str>,
) -> Result<(Vec<TimelineItem>, HashMap<String, serde_json::Value>), SessionFileError> {
    let mut timeline = Vec::new();
    let mut metadata = HashMap::new();

    if source_kind == crate::vscode::SOURCE_KIND {
        let session = crate::vscode::read_session_file(filepath)
            .map_err(|error| (StatusCode::BAD_REQUEST, error))?;
        parse_vscode_timeline(&session, db_entries, &mut timeline, &mut metadata);
        return Ok((timeline, metadata));
    }

    let file = File::open(filepath).map_err(|error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("開啟日誌檔案失敗: {error}"),
        )
    })?;
    let reader = BufReader::new(file);
    match assistant {
        "antigravity" => {
            parse_antigravity_timeline(reader, db_entries, &mut timeline, &mut metadata)
        }
        // Copilot App sessions share one events.jsonl across the main agent and
        // every subagent; the agent filter keeps each drawer's timeline scoped
        // to the right agent. Copilot CLI calls never carry an agentId, so
        // passing `None` here preserves the original CLI behavior. The
        // DB-sourced session model seeds the parser so a subagent drawer starts
        // from its own child model instead of the shared parent
        // `session.start.selectedModel`.
        "copilot" if source_kind == "copilot-app" => parse_copilot_timeline_filtered(
            reader,
            db_entries,
            &mut timeline,
            &mut metadata,
            copilot_app_agent_filter,
            copilot_session_model,
        ),
        "copilot" => parse_copilot_timeline_filtered(
            reader,
            db_entries,
            &mut timeline,
            &mut metadata,
            copilot_app_agent_filter,
            copilot_session_model,
        ),
        "codex" => parse_codex_timeline(reader, db_entries, &mut timeline, &mut metadata),
        "claude" => parse_claude_timeline(reader, db_entries, &mut timeline, &mut metadata),
        "cursor" => parse_cursor_timeline(reader, db_entries, &mut timeline, &mut metadata),
        "grok" => parse_grok_timeline(reader, db_entries, &mut timeline, &mut metadata),
        "pi" => parse_pi_timeline(reader, db_entries, &mut timeline, &mut metadata),
        "omp" => parse_omp_timeline(reader, db_entries, &mut timeline, &mut metadata),
        "muse" => parse_muse_timeline(reader, db_entries, &mut timeline, &mut metadata),
        _ => return Err((StatusCode::BAD_REQUEST, "不支援的助理類型".to_string())),
    }

    Ok((timeline, metadata))
}

fn timeline_matches_user_prompt(timeline: &[TimelineItem], normalized_query: &str) -> bool {
    timeline.iter().any(|item| {
        matches!(
            item,
            TimelineItem::UserPrompt { prompt, .. }
                if prompt.to_lowercase().contains(normalized_query)
        )
    })
}

#[derive(Deserialize)]
pub struct SessionSearchQuery {
    q: String,
}

#[derive(Deserialize, Default)]
pub struct SessionDetailsQuery {
    source_kind: Option<String>,
    source_dir_key: Option<String>,
}

#[derive(Serialize)]
struct SessionSearchMatch {
    session_id: String,
    assistant_type: String,
}

#[derive(Serialize)]
struct SessionSearchResponse {
    matches: Vec<SessionSearchMatch>,
    unavailable_sessions: usize,
}

struct SearchableSession {
    session_id: String,
    assistant_type: String,
    transcript_path: Option<String>,
    source_kind: String,
}

pub async fn get_available_dates(Path(assistant): Path<String>) -> impl IntoResponse {
    let assistant = normalize_assistant_name(&assistant);
    if !is_supported_assistant(&assistant) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "不支援的助理類型" })),
        )
            .into_response();
    }

    let res: Result<Vec<String>, String> = tokio::task::spawn_blocking(move || {
        let conn = db::get_db_conn()?;
        db::get_available_dates(&conn, &assistant)
    })
    .await
    .unwrap_or_else(|_| Err("執行緒執行失敗".to_string()));

    match res {
        Ok(date_list) => Json(DateListResponse { dates: date_list }).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        )
            .into_response(),
    }
}

/// API 2: 獲取當前環境配置與安裝狀況資訊
pub async fn get_setup_info(Path(assistant): Path<String>) -> impl IntoResponse {
    let assistant = normalize_assistant_name(&assistant);
    if !is_supported_assistant(&assistant) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "不支援的助理類型" })),
        )
            .into_response();
    }

    let workspace_dir = match std::env::current_dir() {
        Ok(dir) => dir.to_string_lossy().into_owned(),
        Err(_) => "".to_string(),
    };
    let home_dir_path = dirs::home_dir().unwrap_or_default();
    let home_dir = home_dir_path.to_string_lossy().into_owned();

    let script_name = if cfg!(windows) {
        "statusline-token.ps1"
    } else {
        "statusline-token.sh"
    };

    let anti_dir = db::get_antigravity_dir();
    let anti_script = anti_dir.join(script_name);
    let anti_source_relative = if cfg!(windows) {
        PathBuf::from("shell").join(script_name)
    } else {
        PathBuf::from("shell").join("antigravity").join(script_name)
    };
    let anti_source_script =
        crate::paths::find_resource(&anti_source_relative).unwrap_or(anti_source_relative);

    let copilot_dir = db::get_copilot_dir();
    let copilot_script = copilot_dir.join(script_name);
    let copilot_source_relative = if cfg!(windows) {
        PathBuf::from("shell").join(script_name)
    } else {
        PathBuf::from("shell").join("copilot").join(script_name)
    };
    let copilot_source_script =
        crate::paths::find_resource(&copilot_source_relative).unwrap_or(copilot_source_relative);

    let codex_dir = db::get_codex_dir();
    let codex_exists =
        codex_dir.join("sessions").exists() || codex_dir.join("archived_sessions").exists();

    let claude_dir = db::get_claude_dir();
    let claude_exists = claude_dir.join("projects").exists();

    let cursor_dir = db::get_cursor_dir();
    let cursor_exists = cursor_dir.join("projects").exists();

    let copilot_app_dir = crate::paths::copilot_app_dir();
    let copilot_app_data_db = copilot_app_dir.join("data.db");
    let copilot_app_session_db = copilot_app_dir.join("session-store.db");
    let copilot_app_exists = copilot_app_data_db.exists() || copilot_app_session_db.exists();

    let grok_dir = db::get_grok_dir();
    let grok_exists = grok_dir.join("sessions").exists();

    let pi_dir = db::get_pi_dir();
    let pi_exists = pi_dir.join("agent").join("sessions").exists();

    let omp_dir = db::get_omp_dir();
    let omp_exists = omp_dir.join("agent").join("sessions").exists();

    let muse_dir = db::get_muse_dir();
    let muse_exists = muse_dir.join("sessions").exists();

    Json(SetupInfoResponse {
        platform: std::env::consts::OS.to_string(),
        workspace_dir,
        home_dir,
        antigravity: AssistantSetupStatus {
            dir_path: anti_dir.to_string_lossy().into_owned(),
            data_path: anti_dir.join("usage").to_string_lossy().into_owned(),
            exists: anti_script.exists(),
            script_path: anti_script.to_string_lossy().into_owned(),
            source_script_path: anti_source_script.to_string_lossy().into_owned(),
            settings_path: anti_dir
                .join("settings.json")
                .to_string_lossy()
                .into_owned(),
        },
        copilot: AssistantSetupStatus {
            dir_path: copilot_dir.to_string_lossy().into_owned(),
            data_path: copilot_dir.join("usage").to_string_lossy().into_owned(),
            exists: copilot_script.exists(),
            script_path: copilot_script.to_string_lossy().into_owned(),
            source_script_path: copilot_source_script.to_string_lossy().into_owned(),
            settings_path: copilot_dir
                .join("settings.json")
                .to_string_lossy()
                .into_owned(),
        },
        copilot_app: AssistantSetupStatus {
            dir_path: copilot_app_dir.to_string_lossy().into_owned(),
            data_path: copilot_app_session_db.to_string_lossy().into_owned(),
            exists: copilot_app_exists,
            script_path: "".to_string(),
            source_script_path: "".to_string(),
            settings_path: "".to_string(),
        },
        codex: AssistantSetupStatus {
            dir_path: codex_dir.to_string_lossy().into_owned(),
            data_path: codex_dir.to_string_lossy().into_owned(),
            exists: codex_exists,
            script_path: "".to_string(),
            source_script_path: "".to_string(),
            settings_path: "".to_string(),
        },
        claude: AssistantSetupStatus {
            dir_path: claude_dir.to_string_lossy().into_owned(),
            data_path: claude_dir.join("projects").to_string_lossy().into_owned(),
            exists: claude_exists,
            script_path: "".to_string(),
            source_script_path: "".to_string(),
            settings_path: "".to_string(),
        },
        cursor: AssistantSetupStatus {
            dir_path: cursor_dir.to_string_lossy().into_owned(),
            data_path: cursor_dir.join("projects").to_string_lossy().into_owned(),
            exists: cursor_exists,
            script_path: "".to_string(),
            source_script_path: "".to_string(),
            settings_path: "".to_string(),
        },
        grok: AssistantSetupStatus {
            dir_path: grok_dir.to_string_lossy().into_owned(),
            data_path: grok_dir.join("sessions").to_string_lossy().into_owned(),
            exists: grok_exists,
            script_path: "".to_string(),
            source_script_path: "".to_string(),
            settings_path: "".to_string(),
        },
        pi: AssistantSetupStatus {
            dir_path: pi_dir.to_string_lossy().into_owned(),
            data_path: pi_dir
                .join("agent")
                .join("sessions")
                .to_string_lossy()
                .into_owned(),
            exists: pi_exists,
            script_path: "".to_string(),
            source_script_path: "".to_string(),
            settings_path: "".to_string(),
        },
        omp: AssistantSetupStatus {
            dir_path: omp_dir.to_string_lossy().into_owned(),
            data_path: omp_dir
                .join("agent")
                .join("sessions")
                .to_string_lossy()
                .into_owned(),
            exists: omp_exists,
            script_path: "".to_string(),
            source_script_path: "".to_string(),
            settings_path: "".to_string(),
        },
        muse: AssistantSetupStatus {
            dir_path: muse_dir.to_string_lossy().into_owned(),
            data_path: muse_dir.join("sessions").to_string_lossy().into_owned(),
            exists: muse_exists,
            script_path: "".to_string(),
            source_script_path: "".to_string(),
            settings_path: "".to_string(),
        },
    })
    .into_response()
}

/// API 3: 獲取指定日期的 Token 使用詳情與會話列表
pub async fn get_usage_details(
    Path((assistant, date)): Path<(String, String)>,
) -> impl IntoResponse {
    let assistant = normalize_assistant_name(&assistant);
    if !is_supported_assistant(&assistant) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "不支援的助理類型" })),
        )
            .into_response();
    }

    let assistant_clone = assistant.clone();
    let date_clone = date.clone();

    let entries_res: Result<Vec<(crate::db::UsageDayExportRecord, String)>, String> =
        tokio::task::spawn_blocking(move || {
            let conn = db::get_db_conn()?;
            db::get_usage_entries_by_date(&conn, &date_clone, &assistant_clone)
        })
        .await
        .unwrap_or_else(|_| Err("執行緒執行失敗".to_string()));

    let entries_with_type = match entries_res {
        Ok(e) => e,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": err })),
            )
                .into_response()
        }
    };

    if entries_with_type.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "找不到該日期的使用量資料。" })),
        )
            .into_response();
    }

    let pricing_rules = load_prepared_pricing_rules();
    let (summary, sessions_summary, entries) =
        aggregate_usage_details(&entries_with_type, &pricing_rules);

    Json(UsageDetailsResponse {
        date,
        home_dir: dirs::home_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        summary,
        sessions: sessions_summary,
        raw_entries: entries,
    })
    .into_response()
}

/// 搜尋指定日期各會話中的所有 USER 提示詞
pub async fn search_sessions_by_user_prompt(
    Path((assistant, date)): Path<(String, String)>,
    Query(params): Query<SessionSearchQuery>,
) -> impl IntoResponse {
    let assistant = normalize_assistant_name(&assistant);
    if !is_supported_assistant(&assistant) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "不支援的助理類型" })),
        )
            .into_response();
    }

    let query = params.q.trim();
    if query.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "搜尋關鍵字不可為空。" })),
        )
            .into_response();
    }
    if query.chars().count() > 256 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "搜尋關鍵字不可超過 256 個字元。" })),
        )
            .into_response();
    }

    let normalized_query = query.to_lowercase();
    let search_result: Result<SessionSearchResponse, String> = tokio::task::spawn_blocking({
        let assistant = assistant.clone();
        move || {
            let conn = db::get_db_conn()?;
            let entries = db::get_usage_entries_by_date(&conn, &date, &assistant)?;
            let mut sessions = HashMap::<(String, String), SearchableSession>::new();

            for (record, assistant_type) in entries {
                let entry = record.entry;
                let key = (assistant_type.clone(), entry.session_id.clone());
                let session = sessions.entry(key).or_insert_with(|| SearchableSession {
                    session_id: entry.session_id.clone(),
                    assistant_type,
                    transcript_path: entry.transcript_path.clone(),
                    source_kind: entry
                        .source_kind
                        .clone()
                        .unwrap_or_else(|| "legacy".to_string()),
                });
                if session.transcript_path.is_none() {
                    session.transcript_path = entry.transcript_path;
                }
            }

            let mut matches = Vec::new();
            let mut unavailable_sessions = 0;
            for session in sessions.into_values() {
                if !is_safe_session_id(&session.session_id) {
                    unavailable_sessions += 1;
                    continue;
                }

                let filepath = match resolve_session_file_path(
                    &session.assistant_type,
                    &session.session_id,
                    session.transcript_path.as_deref(),
                    &session.source_kind,
                    None,
                    None,
                ) {
                    Ok(path) if path.exists() => path,
                    _ => {
                        unavailable_sessions += 1;
                        continue;
                    }
                };
                let db_entries = HashMap::new();
                let (timeline, _) = match parse_session_timeline_file(
                    &session.assistant_type,
                    &session.source_kind,
                    &filepath,
                    &db_entries,
                    None,
                    None,
                ) {
                    Ok(result) => result,
                    Err(_) => {
                        unavailable_sessions += 1;
                        continue;
                    }
                };

                if timeline_matches_user_prompt(&timeline, &normalized_query) {
                    matches.push(SessionSearchMatch {
                        session_id: session.session_id,
                        assistant_type: session.assistant_type,
                    });
                }
            }

            matches.sort_by(|a, b| {
                a.assistant_type
                    .cmp(&b.assistant_type)
                    .then_with(|| a.session_id.cmp(&b.session_id))
            });
            Ok(SessionSearchResponse {
                matches,
                unavailable_sessions,
            })
        }
    })
    .await
    .unwrap_or_else(|_| Err("執行緒執行失敗".to_string()));

    match search_result {
        Ok(result) => Json(result).into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": error })),
        )
            .into_response(),
    }
}

/// API 4: 獲取特定會話的詳細對話歷史還原時間軸
fn get_git_info(cwd_str: &str) -> (Option<String>, Option<String>) {
    let path = std::path::Path::new(cwd_str);
    if !path.exists() {
        return (None, None);
    }

    let branch = std::process::Command::new("git")
        .args(["symbolic-ref", "--short", "HEAD"])
        .current_dir(path)
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
            } else {
                None
            }
        });

    let repo = std::process::Command::new("git")
        .args(["config", "--get", "remote.origin.url"])
        .current_dir(path)
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
            } else {
                None
            }
        });

    (branch, repo)
}

/// Query parameters for the session detail API.
/// `source_kind` and `source_dir_key` allow the client to disambiguate
/// sessions that share the same `session_id` across different sources
/// (e.g. Copilot CLI vs. Copilot App, or two different Copilot App
/// directories). Both are optional for backward compatibility; when omitted
/// the query falls back to non-App rows only.
/// Validates a `source_dir_key` value. The key is a hex-encoded canonical
/// path produced by the Copilot App collector; it must only contain
/// hex characters so it cannot be used for path injection.
fn is_safe_source_dir_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= 512 && key.chars().all(|c| c.is_ascii_hexdigit())
}

pub async fn get_session_details(
    Path((assistant, session_id)): Path<(String, String)>,
    Query(query): Query<SessionDetailsQuery>,
) -> impl IntoResponse {
    let assistant = normalize_assistant_name(&assistant);
    if !is_supported_assistant(&assistant) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "不支援的助理類型" })),
        )
            .into_response();
    }

    if !is_safe_session_id(&session_id) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "非法的 session_id 格式。" })),
        )
            .into_response();
    }

    let requested_source_kind = query
        .source_kind
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if requested_source_kind
        .as_ref()
        .is_some_and(|value| value.len() > 64)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "source_kind 格式不正確。" })),
        )
            .into_response();
    }

    // Validate source_dir_key if provided — must be hex-only to prevent
    // path injection.
    if let Some(ref sdk) = query.source_dir_key {
        if !is_safe_source_dir_key(sdk) {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "非法的 source_dir_key 格式。" })),
            )
                .into_response();
        }
    }

    let session_info: Result<db::SessionIdentity, String> = tokio::task::spawn_blocking({
        let sid = session_id.clone();
        let assistant_name = assistant.clone();
        let sk = requested_source_kind;
        let sdk = query.source_dir_key.clone();
        move || {
            let conn = db::get_db_conn()?;
            db::get_session_assistant_and_transcript(
                &conn,
                &assistant_name,
                &sid,
                sk.as_deref(),
                sdk.as_deref(),
            )
        }
    })
    .await
    .unwrap_or_else(|_| Err("執行緒執行失敗".to_string()));

    let (
        resolved_assistant,
        transcript_path_db,
        source_kind,
        source_dir_key,
        copilot_app_parent_session_id,
        copilot_app_agent_nickname,
    ) = match session_info {
        Ok(info) => info,
        Err(e) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": e })),
            )
                .into_response();
        }
    };

    if resolved_assistant != assistant {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "找不到該會話資料或助理類型不符" })),
        )
            .into_response();
    }

    // 2. 準備讀取檔案的完整路徑
    let filepath = match resolve_session_file_path(
        &resolved_assistant,
        &session_id,
        transcript_path_db.as_deref(),
        &source_kind,
        copilot_app_parent_session_id.as_deref(),
        copilot_app_agent_nickname.as_deref(),
    ) {
        Ok(path) => path,
        Err(err) => {
            let mut payload = serde_json::json!({ "error": err.error });
            if let Some(reason) = err.reason {
                payload["reason"] = serde_json::Value::String(reason);
            }
            return (err.status, Json(payload)).into_response();
        }
    };

    if !filepath.exists() {
        // 判斷是否為「尚未開始交談」（session 目錄存在但 events.jsonl 尚未產生）
        // For Copilot subagent synthetic sessions (App or CLI), the events file
        // lives under the parent session's directory, so check that directory.
        let is_session_dir_present = match resolved_assistant.as_str() {
            "copilot" => {
                // Copilot App and Copilot CLI subagent synthetic sessions store
                // their events.jsonl under the parent session's session-state
                // directory. App sessions live under `paths::copilot_app_dir()`
                // (honors COPILOT_APP_DIR), while CLI sessions live under
                // `db::get_copilot_dir()` (honors COPILOT_DIR). Using the wrong
                // base would misclassify missing App sessions when the two env
                // vars point to different directories.
                let cop_dir = if source_kind == "copilot-app" {
                    crate::paths::copilot_app_dir()
                } else {
                    db::get_copilot_dir()
                };
                let dir_id = if source_kind == "copilot-app" || source_kind == "copilot-cli" {
                    copilot_app_parent_session_id
                        .as_deref()
                        .unwrap_or(&session_id)
                } else {
                    &session_id
                };
                cop_dir.join("session-state").join(dir_id).exists()
            }
            _ => false,
        };
        let reason = if is_session_dir_present {
            "no_events_yet"
        } else {
            "file_missing"
        };
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "找不到該會話的本地日誌檔。", "reason": reason })),
        )
            .into_response();
    }

    // 3. 預先載入 SQLite 中的回合 (turn_no) 增量 token 數據
    let sid_clone = session_id.clone();
    let assistant_clone = resolved_assistant.clone();
    let source_kind_clone = source_kind.clone();
    let sdk_clone = source_dir_key.clone();
    let session_db_data: SessionDbData = tokio::task::spawn_blocking(move || {
        if let Ok(conn) = db::get_db_conn() {
            let session_cwd = db::get_session_cwd(
                &conn,
                &assistant_clone,
                &sid_clone,
                Some(&source_kind_clone),
                sdk_clone.as_deref(),
            )
            .unwrap_or(None);
            let session_model = db::get_session_model(
                &conn,
                &assistant_clone,
                &sid_clone,
                Some(&source_kind_clone),
                sdk_clone.as_deref(),
            )
            .unwrap_or(None);
            let map = db::get_session_turns_token_stats(
                &conn,
                &assistant_clone,
                &sid_clone,
                Some(&source_kind_clone),
                sdk_clone.as_deref(),
            )
            .unwrap_or_default();
            SessionDbData {
                db_entries: map,
                session_cwd,
                session_model,
            }
        } else {
            SessionDbData::default()
        }
    })
    .await
    .unwrap_or_default();
    let SessionDbData {
        db_entries,
        session_cwd,
        session_model,
    } = session_db_data;

    // Agent filter: Copilot App and Copilot CLI synthetic subagent rows both
    // share the parent session's events.jsonl and rely on a top-level
    // `agentId` field to keep subagent events out of the main agent view and
    // vice versa. The filter value is the database-sourced `agent_nickname`
    // (the real agent_id), never a string-split synthetic id.
    let copilot_agent_filter: Option<&str> = if resolved_assistant == "copilot"
        && matches!(source_kind.as_str(), "copilot-app" | "copilot-cli")
    {
        copilot_app_agent_nickname.as_deref()
    } else {
        None
    };
    let (timeline, mut metadata) = match parse_session_timeline_file(
        &resolved_assistant,
        &source_kind,
        &filepath,
        &db_entries,
        copilot_agent_filter,
        session_model.as_deref(),
    ) {
        Ok(result) => result,
        Err((status, error)) => {
            return (status, Json(serde_json::json!({ "error": error }))).into_response();
        }
    };

    // Copilot App / CLI subagent requests may resolve the shared events.jsonl
    // but find no agent-specific events carrying the requested agentId (e.g.
    // the subagent row was imported from a usage snapshot before its lifecycle
    // events were written, or the agent id drifted between DB and file).
    // Shared context (session start, user prompt) is intentionally kept for
    // readability, so the drawer is "unavailable" only when there are no
    // agent-specific items at all. Surface a recognizable reason instead of a
    // context-only drawer so the frontend can explain it.
    if matches!(source_kind.as_str(), "copilot-app" | "copilot-cli")
        && copilot_agent_filter.is_some()
    {
        let has_agent_specific = timeline.iter().any(|item| match item {
            TimelineItem::AgentReply { .. } => true,
            TimelineItem::ToolStep { .. } => true,
            TimelineItem::SystemStatus { status_type, .. } => {
                matches!(
                    status_type.as_str(),
                    "subagent_started" | "subagent_completed" | "subagent_failed"
                )
            }
            _ => false,
        });
        if !has_agent_specific {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": "Copilot subagent 的 events.jsonl 中找不到對應 agentId 的事件，可能該 subagent 尚未寫入事件或檔案已被置換。",
                    "reason": "content_unavailable",
                })),
            )
                .into_response();
        }
    }

    // 補充或覆寫 Git 與 CWD 相關資訊（若 metadata 未包含但資料庫中有紀錄）
    if let Some(ref cwd) = session_cwd {
        if !metadata.contains_key("cwd") {
            metadata.insert("cwd".to_string(), serde_json::Value::String(cwd.clone()));
        }

        let (branch, repo) = get_git_info(cwd);
        if !metadata.contains_key("git_branch") {
            if let Some(b) = branch {
                metadata.insert("git_branch".to_string(), serde_json::Value::String(b));
            }
        }
        if !metadata.contains_key("repository") {
            if let Some(r) = repo {
                metadata.insert("repository".to_string(), serde_json::Value::String(r));
            }
        }
    }

    // 計算該會話的加總 Token 資料，供 metadata 使用
    let mut total_tokens = 0;
    let mut total_cache_read_tokens = 0;
    let mut total_input_tokens = 0;
    let mut total_output_tokens = 0;
    let mut total_reasoning_tokens = 0;

    for (stats, _) in db_entries.values() {
        total_tokens += stats.total;
        total_cache_read_tokens += stats.cache_read.unwrap_or(0);
        total_input_tokens += stats.input;
        total_output_tokens += stats.output;
        total_reasoning_tokens += stats.reasoning.unwrap_or(0);
    }

    metadata.insert(
        "total_tokens".to_string(),
        serde_json::Value::Number(serde_json::Number::from(total_tokens)),
    );
    metadata.insert(
        "total_cache_read_tokens".to_string(),
        serde_json::Value::Number(serde_json::Number::from(total_cache_read_tokens)),
    );
    metadata.insert(
        "total_input_tokens".to_string(),
        serde_json::Value::Number(serde_json::Number::from(total_input_tokens)),
    );
    metadata.insert(
        "total_output_tokens".to_string(),
        serde_json::Value::Number(serde_json::Number::from(total_output_tokens)),
    );
    metadata.insert(
        "total_reasoning_tokens".to_string(),
        serde_json::Value::Number(serde_json::Number::from(total_reasoning_tokens)),
    );

    #[derive(Serialize)]
    struct LegacyEventWrapper {
        event_type: String,
        event_data: serde_json::Value,
    }

    let legacy_timeline: Vec<LegacyEventWrapper> = timeline.into_iter().map(|item| {
        match item {
            TimelineItem::UserPrompt { timestamp, prompt, context, turn_no } => {
                let mut attachments = Vec::new();
                if let Some(ctx) = context {
                    if let Some(atts) = ctx.get("attachments").and_then(|a| a.as_array()) {
                        attachments = atts.clone();
                    }
                }
                LegacyEventWrapper {
                    event_type: "UserPrompt".to_string(),
                    event_data: serde_json::json!({
                        "timestamp": timestamp,
                        "prompt": prompt,
                        "transformed_prompt": None::<String>,
                        "attachments": attachments,
                        "turn_no": turn_no,
                    }),
                }
            }
            TimelineItem::AgentReply { timestamp, reply, reasoning, turn_no, model, tokens, duration_ms: _, reasoning_effort } => {
                let reply_content = if let Some(r) = reasoning {
                    format!("<details><summary>🧠 LLM Reasoning Process</summary>\n{}\n</details>\n\n{}", r, reply)
                } else {
                    reply
                };
                LegacyEventWrapper {
                    event_type: "AssistantReply".to_string(),
                    event_data: serde_json::json!({
                        "timestamp": timestamp,
                        "reply": reply_content,
                        "model": model,
                        "reasoning_effort": reasoning_effort,
                        "input_tokens": tokens.as_ref().map(|t| t.input),
                        "output_tokens": tokens.as_ref().map(|t| t.output),
                        "cache_read_tokens": tokens.as_ref().and_then(|t| t.cache_read),
                        "cache_write_tokens": tokens.as_ref().and_then(|t| t.cache_write),
                        "reasoning_tokens": tokens.as_ref().and_then(|t| t.reasoning),
                        "total_tokens": tokens.as_ref().map(|t| t.total),
                        "tool_requests": Vec::<serde_json::Value>::new(),
                        "turn_no": turn_no,
                    }),
                }
            }
            TimelineItem::ToolStep { timestamp, tool_name, arguments, env: _, exit_code, stdout, stderr, tool_call_id: _, status } => {
                let content_str = if !stderr.is_empty() {
                    format!("Stdout:\n{}\n\nStderr:\n{}", stdout, stderr)
                } else {
                    stdout
                };
                LegacyEventWrapper {
                    event_type: "ToolStep".to_string(),
                    event_data: serde_json::json!({
                        "timestamp": timestamp,
                        "tool_name": tool_name,
                        "arguments": arguments,
                        "result": if status == "success" || status == "failed" {
                            Some(serde_json::json!({
                                "content": content_str,
                                "exitCode": exit_code,
                            }))
                        } else {
                            None
                        },
                        "turn_no": 1,
                    }),
                }
            }
            TimelineItem::SystemStatus { timestamp, status_type, message } => {
                LegacyEventWrapper {
                    event_type: "SystemStatus".to_string(),
                    event_data: serde_json::json!({
                        "timestamp": timestamp,
                        "status_type": status_type,
                        "message": message,
                    }),
                }
            }
        }
    }).collect();

    Json(serde_json::json!({
        "session_id": session_id,
        "metadata": metadata,
        "timeline": legacy_timeline,
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::UsageDayExportRecord;
    use crate::pricing::PricingRule;
    use rusqlite::Connection;
    use std::time::{SystemTime, UNIX_EPOCH};
    use std::{collections::HashMap, fs, path::PathBuf};

    fn legacy_usage_entry(turn_no: u32, model: &str, tokens: TokenStats) -> UsageEntry {
        UsageEntry {
            timestamp: format!("2026-07-10T10:{turn_no:02}:00Z"),
            session_id: "legacy-session".to_string(),
            session_name: None,
            transcript_path: None,
            cwd: None,
            version: None,
            turn_no,
            model: Some(model.to_string()),
            model_id: Some(model.to_string()),
            tokens: Some(tokens),
            delta_tokens: None,
            context: None,
            cost: None,
            source_kind: None,
            source_dir_key: None,
            parent_session_id: None,
            agent_nickname: None,
            agent_role: None,
            reasoning_effort: None,
        }
    }

    fn usage_day_record(entry: UsageEntry, assistant_type: &str) -> (UsageDayExportRecord, String) {
        (
            UsageDayExportRecord {
                entry,
                import_source_id: None,
            },
            assistant_type.to_string(),
        )
    }

    fn delta_usage_entry(
        session_id: &str,
        turn_no: u32,
        model: &str,
        tokens: TokenStats,
        delta_tokens: TokenStats,
    ) -> UsageEntry {
        UsageEntry {
            timestamp: format!("2026-08-07T10:{turn_no:02}:00Z"),
            session_id: session_id.to_string(),
            session_name: Some(format!("Session {session_id}")),
            transcript_path: None,
            cwd: Some("/repo".to_string()),
            version: None,
            turn_no,
            model: Some(model.to_string()),
            model_id: Some(model.to_string()),
            tokens: Some(tokens),
            delta_tokens: Some(delta_tokens),
            context: None,
            cost: None,
            source_kind: Some("copilot-cli".to_string()),
            source_dir_key: None,
            parent_session_id: None,
            agent_nickname: None,
            agent_role: None,
            reasoning_effort: None,
        }
    }

    fn assert_daily_summary_matches_session_totals(
        summary: &DaySummary,
        sessions: &[SessionSummary],
    ) {
        assert_eq!(
            summary.total_tokens,
            sessions
                .iter()
                .map(|session| session.total_tokens)
                .sum::<u64>()
        );
        assert_eq!(
            summary.total_input_tokens,
            sessions
                .iter()
                .map(|session| session.total_input_tokens)
                .sum::<u64>()
        );
        assert_eq!(
            summary.total_output_tokens,
            sessions
                .iter()
                .map(|session| session.total_output_tokens)
                .sum::<u64>()
        );
        assert_eq!(
            summary.total_cache_read_tokens,
            sessions
                .iter()
                .map(|session| session.total_cache_read_tokens)
                .sum::<u64>()
        );
        assert_eq!(
            summary.total_cache_write_tokens,
            sessions
                .iter()
                .map(|session| session.total_cache_write_tokens)
                .sum::<u64>()
        );
        assert_eq!(
            summary.total_reasoning_tokens,
            sessions
                .iter()
                .map(|session| session.total_reasoning_tokens)
                .sum::<u64>()
        );
        assert!(
            (summary.total_cost_usd - sessions.iter().map(|session| session.cost_usd).sum::<f64>())
                .abs()
                < 1e-9
        );
        assert_eq!(summary.total_sessions, sessions.len());
    }

    fn user_prompt(prompt: &str, turn_no: u32) -> TimelineItem {
        TimelineItem::UserPrompt {
            timestamp: "2026-07-16T00:00:00Z".to_string(),
            prompt: prompt.to_string(),
            context: None,
            turn_no,
        }
    }

    #[test]
    fn user_prompt_search_checks_every_turn_case_insensitively() {
        let timeline = vec![
            user_prompt("先建立專案", 1),
            user_prompt("Please FIX the payment callback", 2),
        ];

        assert!(timeline_matches_user_prompt(&timeline, "fix the payment"));
    }

    #[test]
    fn user_prompt_search_ignores_non_user_timeline_content() {
        let timeline = vec![
            user_prompt("整理今日工作", 1),
            TimelineItem::SystemStatus {
                timestamp: "2026-07-16T00:00:01Z".to_string(),
                status_type: "session_start".to_string(),
                message: "secret keyword".to_string(),
            },
        ];

        assert!(!timeline_matches_user_prompt(&timeline, "secret keyword"));
    }

    #[test]
    fn grok_multi_model_jsonl_survives_sqlite_and_timeline() {
        let root = std::env::temp_dir().join(format!(
            "token-usage-insights-grok-timeline-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let session_id = "grok-multi-model-timeline";
        let session_dir = root.join("sessions").join("work").join(session_id);
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
        db::init_db(&conn).unwrap();
        db::sync_grok_usage_logs(&mut conn, &root).unwrap();
        let db_entries = db::get_session_turns_token_stats(
            &conn,
            "grok",
            session_id,
            Some(crate::grok::USAGE_SOURCE_KIND),
            None,
        )
        .unwrap();

        let mut timeline = Vec::new();
        let mut metadata = HashMap::new();
        parse_grok_timeline(
            BufReader::new(File::open(&updates_path).unwrap()),
            &db_entries,
            &mut timeline,
            &mut metadata,
        );

        let (tokens, model) = timeline
            .iter()
            .find_map(|item| match item {
                TimelineItem::AgentReply { tokens, model, .. } => {
                    tokens.as_ref().map(|tokens| (tokens, model))
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(tokens.input, 300);
        assert_eq!(tokens.output, 60);
        assert_eq!(tokens.total, 360);
        assert!(model.contains("Grok 4.5"));
        assert!(model.contains("Grok Build 0.1"));

        let _ = fs::remove_dir_all(root);
    }

    /// Regression test: same session_id with copilot-cli and copilot-app rows
    /// must produce two separate session summaries with correct source_kind,
    /// not one merged session. This mirrors the aggregation logic in
    /// get_usage_details using get_usage_entries_by_date.
    #[test]
    fn daily_summary_separates_copilot_cli_and_app_with_same_session_id() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::init_db(&conn).unwrap();

        // Insert a copilot-cli row for session "shared-sess".
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, source_dir_key, timestamp, date,
                session_id, turn_no, session_name,
                tokens_input, tokens_output, tokens_total,
                delta_input, delta_output, delta_total
             ) VALUES (
                'copilot', 'copilot-cli', NULL, '2026-07-20T10:00:00Z', '2026-07-20',
                'shared-sess', 1, 'CLI Session',
                50, 5, 55,
                50, 5, 55
             )",
            [],
        )
        .unwrap();

        // Insert a copilot-app row for the SAME session_id (simulating what
        // sync_copilot_app_usage_logs would produce).
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, source_dir_key, timestamp, date,
                session_id, turn_no, session_name,
                tokens_input, tokens_output, tokens_total,
                delta_input, delta_output, delta_total,
                model
             ) VALUES (
                'copilot', 'copilot-app', 'abcdef00', '2026-07-20T10:00:00Z', '2026-07-20',
                'shared-sess', 1, 'App Session',
                100, 10, 110,
                100, 10, 110,
                'GLM5.2'
             )",
            [],
        )
        .unwrap();

        // Fetch entries for the date — should have copilot-cli and copilot-app.
        let entries = crate::db::get_usage_entries_by_date(&conn, "2026-07-20", "copilot").unwrap();

        // Group by (source_kind, session_id, source_dir_key) mirroring the handler.
        let mut sessions: HashMap<(String, String, Option<String>), Vec<&crate::db::UsageEntry>> =
            HashMap::new();
        for (record, _ast) in &entries {
            let e = &record.entry;
            let sk = e
                .source_kind
                .clone()
                .unwrap_or_else(|| "legacy".to_string());
            let key = (sk, e.session_id.clone(), e.source_dir_key.clone());
            sessions.entry(key).or_default().push(e);
        }

        // Must be 2 separate sessions.
        assert_eq!(
            sessions.len(),
            2,
            "copilot-cli and copilot-app with same session_id must be 2 separate sessions"
        );

        // Verify each session has the correct source_kind.
        let source_kinds: Vec<String> = sessions.keys().map(|(sk, _, _)| sk.clone()).collect();
        assert!(
            source_kinds.contains(&"copilot-cli".to_string()),
            "must have a copilot-cli session, got: {:?}",
            source_kinds
        );
        assert!(
            source_kinds.contains(&"copilot-app".to_string()),
            "must have a copilot-app session, got: {:?}",
            source_kinds
        );

        // Verify the copilot-app session has model GLM5.2 and source_kind copilot-app.
        let app_session = sessions
            .iter()
            .find(|(key, _)| key.0 == "copilot-app")
            .map(|(_, entries)| entries)
            .unwrap();
        assert_eq!(
            app_session[0].model.as_deref(),
            Some("GLM5.2"),
            "copilot-app session model should be GLM5.2"
        );
        assert_eq!(
            app_session[0].source_kind.as_deref(),
            Some("copilot-app"),
            "copilot-app session source_kind must be copilot-app"
        );
    }

    #[test]
    fn aggregate_usage_details_matches_delta_session_totals() {
        let entries_with_type = vec![
            usage_day_record(
                delta_usage_entry(
                    "delta-session-1",
                    1,
                    "gpt-5",
                    TokenStats {
                        input: 100,
                        output: 30,
                        cache_read: Some(10),
                        cache_write: Some(5),
                        cache_write_5m: None,
                        cache_write_1h: None,
                        reasoning: Some(2),
                        total: 147,
                    },
                    TokenStats {
                        input: 100,
                        output: 30,
                        cache_read: Some(10),
                        cache_write: Some(5),
                        cache_write_5m: None,
                        cache_write_1h: None,
                        reasoning: Some(2),
                        total: 147,
                    },
                ),
                "copilot",
            ),
            usage_day_record(
                delta_usage_entry(
                    "delta-session-1",
                    2,
                    "gpt-5",
                    TokenStats {
                        input: 160,
                        output: 50,
                        cache_read: Some(15),
                        cache_write: Some(7),
                        cache_write_5m: None,
                        cache_write_1h: None,
                        reasoning: Some(3),
                        total: 235,
                    },
                    TokenStats {
                        input: 60,
                        output: 20,
                        cache_read: Some(5),
                        cache_write: Some(2),
                        cache_write_5m: None,
                        cache_write_1h: None,
                        reasoning: Some(1),
                        total: 88,
                    },
                ),
                "copilot",
            ),
            usage_day_record(
                delta_usage_entry(
                    "delta-session-2",
                    3,
                    "gpt-5-mini",
                    TokenStats {
                        input: 40,
                        output: 10,
                        cache_read: Some(3),
                        cache_write: Some(1),
                        cache_write_5m: None,
                        cache_write_1h: None,
                        reasoning: Some(4),
                        total: 58,
                    },
                    TokenStats {
                        input: 40,
                        output: 10,
                        cache_read: Some(3),
                        cache_write: Some(1),
                        cache_write_5m: None,
                        cache_write_1h: None,
                        reasoning: Some(4),
                        total: 58,
                    },
                ),
                "copilot",
            ),
        ];
        let rules = [
            PricingRule {
                model_name: "gpt-5".to_string(),
                input_price: 1.0,
                cache_input_price: 0.1,
                output_price: 2.0,
            },
            PricingRule {
                model_name: "gpt-5-mini".to_string(),
                input_price: 0.5,
                cache_input_price: 0.05,
                output_price: 1.0,
            },
        ];

        let (summary, sessions, _) = aggregate_usage_details(
            &entries_with_type,
            &PreparedPricingRules::from_rules(rules.into()),
        );

        assert_daily_summary_matches_session_totals(&summary, &sessions);
    }

    #[test]
    fn aggregate_usage_details_matches_legacy_session_totals() {
        let entries_with_type = vec![
            usage_day_record(
                UsageEntry {
                    timestamp: "2026-08-05T09:00:00Z".to_string(),
                    session_id: "legacy-session-1".to_string(),
                    session_name: Some("Legacy Session 1".to_string()),
                    transcript_path: None,
                    cwd: Some("/repo".to_string()),
                    version: None,
                    turn_no: 1,
                    model: Some("gemini-2.5-pro".to_string()),
                    model_id: Some("gemini-2.5-pro".to_string()),
                    tokens: Some(TokenStats {
                        input: 120,
                        output: 45,
                        cache_read: Some(12),
                        cache_write: Some(6),
                        cache_write_5m: None,
                        cache_write_1h: None,
                        reasoning: Some(3),
                        total: 186,
                    }),
                    delta_tokens: None,
                    context: None,
                    cost: None,
                    source_kind: Some("antigravity".to_string()),
                    source_dir_key: None,
                    parent_session_id: None,
                    agent_nickname: None,
                    agent_role: None,
                    reasoning_effort: None,
                },
                "antigravity",
            ),
            usage_day_record(
                UsageEntry {
                    timestamp: "2026-08-05T10:00:00Z".to_string(),
                    session_id: "legacy-session-2".to_string(),
                    session_name: Some("Legacy Session 2".to_string()),
                    transcript_path: None,
                    cwd: Some("/repo".to_string()),
                    version: None,
                    turn_no: 1,
                    model: Some("gemini-2.5-flash".to_string()),
                    model_id: Some("gemini-2.5-flash".to_string()),
                    tokens: Some(TokenStats {
                        input: 80,
                        output: 20,
                        cache_read: Some(5),
                        cache_write: Some(2),
                        cache_write_5m: None,
                        cache_write_1h: None,
                        reasoning: Some(1),
                        total: 108,
                    }),
                    delta_tokens: None,
                    context: None,
                    cost: None,
                    source_kind: Some("antigravity".to_string()),
                    source_dir_key: None,
                    parent_session_id: None,
                    agent_nickname: None,
                    agent_role: None,
                    reasoning_effort: None,
                },
                "antigravity",
            ),
        ];
        let rules = [
            PricingRule {
                model_name: "gemini-2.5-pro".to_string(),
                input_price: 1.0,
                cache_input_price: 0.1,
                output_price: 2.0,
            },
            PricingRule {
                model_name: "gemini-2.5-flash".to_string(),
                input_price: 0.5,
                cache_input_price: 0.05,
                output_price: 1.0,
            },
        ];

        let (summary, sessions, _) = aggregate_usage_details(
            &entries_with_type,
            &PreparedPricingRules::from_rules(rules.into()),
        );

        assert_daily_summary_matches_session_totals(&summary, &sessions);
    }

    /// Shared helper: build a temp Copilot App directory layout with a single
    /// `session-state/<session_id>/events.jsonl` file containing the provided
    /// lines. Returns the app dir path. Tests clean it up by removing the
    /// returned base dir.
    fn copilot_app_fixture_dir(prefix: &str) -> PathBuf {
        let mut base = std::env::temp_dir();
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        base.push(format!(
            "token-insights-test-{}-{}-{}",
            prefix,
            std::process::id(),
            unique
        ));
        base
    }

    fn write_copilot_app_events(app_dir: &StdPath, session_id: &str, lines: &[&str]) {
        let session_dir = app_dir.join("session-state").join(session_id);
        std::fs::create_dir_all(&session_dir).unwrap();
        let content = lines.join("\n");
        std::fs::write(session_dir.join("events.jsonl"), content).unwrap();
    }

    #[test]
    fn copilot_app_main_session_resolves_events_jsonl_under_session_state() {
        let app_dir = copilot_app_fixture_dir("app-main-resolve");
        let session_id = "74b6d236-d311-4675-9855-fee91bc508e5";
        write_copilot_app_events(&app_dir, session_id, &["{}"]);

        let resolved = resolve_copilot_app_events_path(&app_dir, session_id, None).unwrap();
        assert!(resolved.ends_with("events.jsonl"));
        assert!(resolved.parent().unwrap().ends_with(session_id));

        let _ = std::fs::remove_dir_all(&app_dir);
    }

    #[test]
    fn copilot_app_subagent_uses_parent_session_id_for_path() {
        let app_dir = copilot_app_fixture_dir("app-sub-resolve");
        let parent = "74b6d236-d311-4675-9855-fee91bc508e5";
        let agent = "call_v4b32z66";
        write_copilot_app_events(&app_dir, parent, &["{}"]);

        // The synthetic id must NOT be combined into the path: the caller
        // resolves the parent from the DB and passes it in.
        let resolved = resolve_copilot_app_events_path(&app_dir, parent, Some(agent)).unwrap();
        assert!(resolved.parent().unwrap().ends_with(parent));
        assert!(!resolved.to_string_lossy().contains(&format!("__{agent}")));

        let _ = std::fs::remove_dir_all(&app_dir);
    }

    #[test]
    fn copilot_app_missing_session_dir_returns_file_missing_reason() {
        let app_dir = copilot_app_fixture_dir("app-missing-dir");
        let session_id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

        let err = resolve_copilot_app_events_path(&app_dir, session_id, None).unwrap_err();
        assert_eq!(err.status, StatusCode::NOT_FOUND);
        assert_eq!(err.reason.as_deref(), Some("file_missing"));

        let _ = std::fs::remove_dir_all(&app_dir);
    }

    #[test]
    fn copilot_app_session_dir_without_events_returns_no_events_yet_reason() {
        let app_dir = copilot_app_fixture_dir("app-no-events-yet");
        let session_id = "55555555-6666-7777-8888-999999999999";
        // Create the session directory but NOT events.jsonl.
        std::fs::create_dir_all(app_dir.join("session-state").join(session_id)).unwrap();

        let err = resolve_copilot_app_events_path(&app_dir, session_id, None).unwrap_err();
        assert_eq!(err.status, StatusCode::NOT_FOUND);
        assert_eq!(err.reason.as_deref(), Some("no_events_yet"));

        let _ = std::fs::remove_dir_all(&app_dir);
    }

    #[test]
    fn copilot_app_rejects_unsafe_session_id_before_path_lookup() {
        let app_dir = copilot_app_fixture_dir("app-unsafe-id");
        // A traversal attempt must be rejected without ever touching the FS.
        let err = resolve_copilot_app_events_path(&app_dir, "..", None).unwrap_err();
        assert_eq!(err.status, StatusCode::NOT_FOUND);
        assert_eq!(err.reason.as_deref(), Some("file_missing"));

        let _ = std::fs::remove_dir_all(&app_dir);
    }

    #[test]
    fn get_session_assistant_and_transcript_returns_parent_and_agent_for_subagent_row() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::init_db(&conn).unwrap();
        let parent = "74b6d236-d311-4675-9855-fee91bc508e5";
        let agent = "call_v4b32z66";
        let synthetic = format!("{parent}__{agent}");
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, source_dir_key, timestamp, date,
                session_id, turn_no, model,
                tokens_input, tokens_output, tokens_total,
                delta_input, delta_output, delta_total,
                parent_session_id, agent_nickname
             ) VALUES (
                'copilot', 'copilot-app', 'abcdef00', '2026-07-20T10:00:00Z', '2026-07-20',
                ?, 1, 'K2.7',
                100, 10, 110,
                100, 10, 110,
                ?, ?
             )",
            rusqlite::params![synthetic, parent, agent],
        )
        .unwrap();

        let (_ast, _path, source_kind, _sdk, parent_id, nickname) =
            crate::db::get_session_assistant_and_transcript(
                &conn,
                "copilot",
                &synthetic,
                Some("copilot-app"),
                Some("abcdef00"),
            )
            .unwrap();
        assert_eq!(source_kind, "copilot-app");
        assert_eq!(parent_id.as_deref(), Some(parent));
        assert_eq!(nickname.as_deref(), Some(agent));

        // Main agent row returns None for both.
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, source_dir_key, timestamp, date,
                session_id, turn_no, model,
                tokens_input, tokens_output, tokens_total,
                delta_input, delta_output, delta_total
             ) VALUES (
                'copilot', 'copilot-app', 'abcdef00', '2026-07-20T10:01:00Z', '2026-07-20',
                ?, 1, 'DP4F',
                50, 5, 55,
                50, 5, 55
             )",
            rusqlite::params![parent],
        )
        .unwrap();
        let (_ast, _path, _sk, _sdk, main_parent, main_nick) =
            crate::db::get_session_assistant_and_transcript(
                &conn,
                "copilot",
                parent,
                Some("copilot-app"),
                Some("abcdef00"),
            )
            .unwrap();
        assert!(main_parent.is_none());
        assert!(main_nick.is_none());
    }

    /// Regression test: a Copilot CLI subagent synthetic session row must
    /// resolve its drawer events.jsonl via the parent session's directory
    /// (not the synthetic id's), and the agent filter must keep only that
    /// subagent's events while preserving shared context.
    #[test]
    fn cli_subagent_drawer_resolves_via_parent_and_filters_by_agent_id() {
        let tmp = std::env::temp_dir().join(format!(
            "cli-drawer-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&tmp).unwrap();
        let parent = "drawer-parent-session";
        let agent = "call_drawer";
        let synthetic = format!("{parent}__{agent}");
        let session_dir = tmp.join("session-state").join(parent);
        fs::create_dir_all(&session_dir).unwrap();
        // Write events: shared context (no agentId), main agent reply (no
        // agentId), and the subagent's reply + tool call (tagged with agentId).
        let events = vec![
            serde_json::json!({
                "type": "session.start",
                "timestamp": "2026-07-22T10:00:00Z",
                "data": { "copilotVersion": "1.0.0", "context": { "cwd": "/tmp" } }
            }),
            serde_json::json!({
                "type": "user.message",
                "timestamp": "2026-07-22T10:00:05Z",
                "payload": { "content": "please run the subagent" }
            }),
            serde_json::json!({
                "type": "assistant.message",
                "timestamp": "2026-07-22T10:00:10Z",
                "payload": { "content": "main agent reply" }
            }),
            serde_json::json!({
                "type": "assistant.message",
                "timestamp": "2026-07-22T10:00:20Z",
                "agentId": agent,
                "payload": { "content": "subagent reply" }
            }),
            serde_json::json!({
                "type": "tool.execution_complete",
                "timestamp": "2026-07-22T10:00:25Z",
                "agentId": agent,
                "payload": { "callId": "tool-1" }
            }),
        ];
        let mut file_content = String::new();
        for ev in &events {
            file_content.push_str(&ev.to_string());
            file_content.push('\n');
        }
        fs::write(session_dir.join("events.jsonl"), file_content).unwrap();

        // Resolve the CLI subagent path directly against the temp copilot dir:
        // must point at the parent's events.jsonl, not the synthetic id's.
        let resolved = resolve_copilot_cli_subagent_events_path(&tmp, parent).unwrap();
        assert!(
            resolved.to_string_lossy().ends_with("events.jsonl"),
            "resolved path must end with events.jsonl: {:?}",
            resolved
        );
        assert!(
            resolved.to_string_lossy().contains(parent),
            "resolved path must be under the parent session dir: {:?}",
            resolved
        );
        assert!(
            !resolved.to_string_lossy().contains(&synthetic),
            "must NOT resolve under the synthetic id dir: {:?}",
            resolved
        );

        // Parse with the agent filter (simulating get_session_details'
        // copilot_agent_filter decision for source_kind = "copilot-cli").
        let file = std::fs::File::open(&resolved).unwrap();
        let reader = std::io::BufReader::new(file);
        let db_entries: HashMap<u32, (crate::db::TokenStats, String)> = HashMap::new();
        let mut timeline = Vec::new();
        let mut metadata = HashMap::new();
        crate::timeline::parse_copilot_timeline_filtered(
            reader,
            &db_entries,
            &mut timeline,
            &mut metadata,
            Some(agent),
            None,
        );

        // The subagent view must include shared context (session start, user
        // prompt) and the subagent's own reply + tool call, but NOT the main
        // agent's reply.
        let has_main_reply = timeline.iter().any(|item| match item {
            TimelineItem::AgentReply { reply, .. } => reply.contains("main agent reply"),
            _ => false,
        });
        assert!(
            !has_main_reply,
            "main agent reply must be filtered out of subagent view"
        );

        let has_subagent_reply = timeline.iter().any(|item| match item {
            TimelineItem::AgentReply { reply, .. } => reply.contains("subagent reply"),
            _ => false,
        });
        assert!(
            has_subagent_reply,
            "subagent reply must appear in its own view"
        );

        // Shared context preserved for readability.
        let has_user_prompt = timeline
            .iter()
            .any(|item| matches!(item, TimelineItem::UserPrompt { .. }));
        assert!(
            has_user_prompt,
            "shared user prompt must remain visible to subagent"
        );

        let _ = fs::remove_dir_all(tmp);
    }

    /// Regression: `parse_session_timeline_file` must thread the DB-sourced
    /// child session model into the Copilot timeline parser so a subagent
    /// drawer shows the child model, not the shared parent
    /// `session.start.selectedModel`. Covers both `copilot-app` and
    /// `copilot-cli` source kinds (they share `parse_copilot_timeline_filtered`).
    #[test]
    fn parse_session_timeline_file_threads_child_model_for_subagent_drawer() {
        let tmp = std::env::temp_dir().join(format!(
            "drawer-child-model-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&tmp).unwrap();
        let parent = "child-model-parent";
        let agent = "call_child_model";
        let session_dir = tmp.join("session-state").join(parent);
        fs::create_dir_all(&session_dir).unwrap();
        // Parent session.start carries GLM5.2-none, but the child DB model is
        // gpt-5.4-mini. The subagent drawer must show gpt-5.4-mini.
        let events = vec![
            serde_json::json!({
                "type": "session.start",
                "timestamp": "2026-07-22T10:00:00Z",
                "data": {
                    "copilotVersion": "1.0.0",
                    "context": { "cwd": "/tmp" },
                    "selectedModel": "GLM5.2-none"
                }
            }),
            serde_json::json!({
                "type": "user.message",
                "timestamp": "2026-07-22T10:00:01Z",
                "payload": { "content": "please run the subagent" }
            }),
            serde_json::json!({
                "type": "assistant.message",
                "timestamp": "2026-07-22T10:00:02Z",
                "payload": { "content": "main agent reply" }
            }),
            serde_json::json!({
                "type": "subagent.started",
                "timestamp": "2026-07-22T10:00:05Z",
                "agentId": agent,
                "data": { "agentDisplayName": "GPT", "agentName": "GPT" }
            }),
            serde_json::json!({
                "type": "assistant.message",
                "timestamp": "2026-07-22T10:00:06Z",
                "agentId": agent,
                "payload": { "content": "subagent reply" }
            }),
            serde_json::json!({
                "type": "subagent.completed",
                "timestamp": "2026-07-22T10:00:07Z",
                "agentId": agent
            }),
        ];
        let mut file_content = String::new();
        for ev in &events {
            file_content.push_str(&ev.to_string());
            file_content.push('\n');
        }
        fs::write(session_dir.join("events.jsonl"), file_content).unwrap();

        let resolved = resolve_copilot_cli_subagent_events_path(&tmp, parent).unwrap();
        let db_entries: HashMap<u32, (crate::db::TokenStats, String)> = HashMap::new();
        let (timeline, metadata) = parse_session_timeline_file(
            "copilot",
            "copilot-cli",
            &resolved,
            &db_entries,
            Some(agent),
            Some("gpt-5.4-mini"),
        )
        .unwrap();

        let selected_model = metadata
            .get("selected_model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        assert_eq!(
            selected_model.as_deref(),
            Some("gpt-5.4-mini"),
            "subagent drawer metadata.selected_model must be the child DB model"
        );

        let reply_models: Vec<(String, String)> = timeline
            .iter()
            .filter_map(|item| match item {
                TimelineItem::AgentReply { model, reply, .. } => {
                    Some((model.clone(), reply.clone()))
                }
                _ => None,
            })
            .collect();
        assert!(
            reply_models.iter().all(|(m, _)| m == "gpt-5.4-mini"),
            "every subagent AgentReply.model must be gpt-5.4-mini, got {:?}",
            reply_models
        );
        assert!(
            !reply_models.iter().any(|(m, _)| m == "GLM5.2-none"),
            "GLM5.2-none must not appear in subagent AgentReply models: {:?}",
            reply_models
        );
        // The main agent reply must be filtered out of the subagent view.
        let replies: Vec<String> = reply_models.into_iter().map(|(_, r)| r).collect();
        assert!(
            !replies.iter().any(|r| r == "main agent reply"),
            "main agent reply must not leak into the subagent drawer"
        );

        let _ = fs::remove_dir_all(tmp);
    }

    #[test]
    fn day_summary_uses_last_real_cumulative_legacy_entry() {
        let rules = [PricingRule {
            model_name: "test-model".to_string(),
            input_price: 1.0,
            cache_input_price: 0.1,
            output_price: 2.0,
        }];
        let entries = vec![
            legacy_usage_entry(
                1,
                "test-model",
                TokenStats {
                    input: 100,
                    output: 20,
                    cache_read: Some(10),
                    cache_write: Some(0),
                    cache_write_5m: None,
                    cache_write_1h: None,
                    reasoning: Some(5),
                    total: 135,
                },
            ),
            legacy_usage_entry(
                2,
                "test-model",
                TokenStats {
                    input: 200,
                    output: 40,
                    cache_read: Some(20),
                    cache_write: Some(0),
                    cache_write_5m: None,
                    cache_write_1h: None,
                    reasoning: Some(10),
                    total: 270,
                },
            ),
            legacy_usage_entry(
                3,
                "<synthetic>",
                TokenStats {
                    input: 0,
                    output: 0,
                    cache_read: Some(0),
                    cache_write: Some(0),
                    cache_write_5m: None,
                    cache_write_1h: None,
                    reasoning: Some(0),
                    total: 0,
                },
            ),
        ];
        let session_usage =
            summarize_session_usage(&PreparedPricingRules::from_rules(rules.into()), &entries);
        let mut summary = DaySummary::default();

        add_usage_to_day_summary(&mut summary, &session_usage.usage);

        assert_eq!(summary.total_tokens, 270);
        assert_eq!(summary.total_input_tokens, 200);
        assert_eq!(summary.total_output_tokens, 40);
        assert_eq!(summary.total_cache_read_tokens, 20);
        assert_eq!(summary.total_reasoning_tokens, 10);
    }
}
