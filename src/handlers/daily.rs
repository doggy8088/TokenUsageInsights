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
use crate::pricing::{calculate_usage_cost, load_pricing_rules};
use crate::timeline::{
    parse_antigravity_timeline, parse_claude_timeline, parse_codex_timeline,
    parse_copilot_timeline, parse_cursor_timeline, parse_vscode_timeline, TimelineItem,
};

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
        return Err("找不到該 Codex CLI 會話的本地日誌檔案。".to_string());
    }

    let codex_root = codex_dir
        .canonicalize()
        .map_err(|_| "無法存取 Codex CLI 根目錄。".to_string())?;
    let canonical_path = path
        .canonicalize()
        .map_err(|_| "無法解析 Codex CLI 會話日誌路徑。".to_string())?;

    if !canonical_path.starts_with(codex_root) {
        return Err("Codex CLI 會話日誌路徑不在預期目錄內。".to_string());
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

type SessionFileError = (StatusCode, String);

fn resolve_session_file_path(
    assistant: &str,
    session_id: &str,
    transcript_path_db: Option<&str>,
    source_kind: &str,
) -> Result<PathBuf, SessionFileError> {
    match assistant {
        "antigravity" => Ok(db::get_antigravity_dir()
            .join("brain")
            .join(session_id)
            .join(".system_generated/logs/transcript_full.jsonl")),
        "copilot" if source_kind == crate::vscode::SOURCE_KIND => {
            let path = transcript_path_db.ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    "找不到 VS Code Copilot 聊天檔案路徑。".to_string(),
                )
            })?;
            resolve_vscode_transcript_path(path).map_err(|error| (StatusCode::BAD_REQUEST, error))
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
                (
                    StatusCode::NOT_FOUND,
                    "找不到 Codex CLI 會話日誌檔案路徑。".to_string(),
                )
            })?;
            resolve_codex_transcript_path(&db::get_codex_dir(), path)
                .map_err(|error| (StatusCode::BAD_REQUEST, error))
        }
        "claude" => {
            let path = transcript_path_db.ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    "找不到 Claude Code 會話日誌檔案路徑。".to_string(),
                )
            })?;
            resolve_claude_transcript_path(&db::get_claude_dir(), session_id, path)
                .map_err(|error| (StatusCode::BAD_REQUEST, error))
        }
        "cursor" => {
            let path = transcript_path_db.ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    "找不到 Cursor 會話日誌檔案路徑。".to_string(),
                )
            })?;
            resolve_cursor_transcript_path(&db::get_cursor_dir(), session_id, path)
                .map_err(|error| (StatusCode::BAD_REQUEST, error))
        }
        _ => Err((StatusCode::BAD_REQUEST, "不支援的助理類型".to_string())),
    }
}

fn parse_session_timeline_file(
    assistant: &str,
    source_kind: &str,
    filepath: &StdPath,
    db_entries: &HashMap<u32, (TokenStats, String)>,
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
        "copilot" => parse_copilot_timeline(reader, db_entries, &mut timeline, &mut metadata),
        "codex" => parse_codex_timeline(reader, db_entries, &mut timeline, &mut metadata),
        "claude" => parse_claude_timeline(reader, db_entries, &mut timeline, &mut metadata),
        "cursor" => parse_cursor_timeline(reader, db_entries, &mut timeline, &mut metadata),
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
    let codex_exists = codex_dir.join("sessions").exists();

    let claude_dir = db::get_claude_dir();
    let claude_exists = claude_dir.join("projects").exists();

    let cursor_dir = db::get_cursor_dir();
    let cursor_exists = cursor_dir.join("projects").exists();

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
        codex: AssistantSetupStatus {
            dir_path: codex_dir.to_string_lossy().into_owned(),
            data_path: codex_dir.join("sessions").to_string_lossy().into_owned(),
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

    let mut summary = DaySummary::default();
    let mut sessions_map: HashMap<String, (Vec<UsageEntry>, String)> = HashMap::new();
    let mut entries = Vec::new();

    for (record, ast_type) in &entries_with_type {
        let e = &record.entry;
        entries.push(e.clone());
        let (list, _) = sessions_map
            .entry(e.session_id.clone())
            .or_insert_with(|| (Vec::new(), ast_type.clone()));
        list.push(e.clone());
    }

    summary.total_sessions = sessions_map.len();
    let mut session_last_entries: HashMap<String, UsageEntry> = HashMap::new();

    for e in &entries {
        if let Some(ref tokens) = e.delta_tokens {
            summary.total_tokens += tokens.total;
            summary.total_input_tokens += tokens.input;
            summary.total_output_tokens += tokens.output;
            summary.total_cache_read_tokens += tokens.cache_read.unwrap_or(0);
            summary.total_cache_write_tokens += tokens.cache_write.unwrap_or(0);
            summary.total_reasoning_tokens += tokens.reasoning.unwrap_or(0);
        } else if let Some(ref tokens) = e.tokens {
            if e.turn_no == 1 {
                summary.total_tokens += tokens.total;
                summary.total_input_tokens += tokens.input;
                summary.total_output_tokens += tokens.output;
                summary.total_cache_read_tokens += tokens.cache_read.unwrap_or(0);
                summary.total_cache_write_tokens += tokens.cache_write.unwrap_or(0);
                summary.total_reasoning_tokens += tokens.reasoning.unwrap_or(0);
            }
        }

        let sid = e.session_id.clone();
        let last_e = session_last_entries.entry(sid).or_insert_with(|| e.clone());
        if e.turn_no > last_e.turn_no {
            *last_e = e.clone();
        }
    }

    let pricing_rules = load_pricing_rules();
    let mut sessions_summary = Vec::new();

    for (session_id, (s_entries, ast_type)) in &sessions_map {
        let last_entry = session_last_entries
            .get(session_id)
            .cloned()
            .unwrap_or_else(|| s_entries[0].clone());

        let session_tokens = s_entries
            .iter()
            .map(|e| e.delta_tokens.as_ref().map(|t| t.total).unwrap_or(0))
            .sum::<u64>();
        let session_input_tokens = s_entries
            .iter()
            .map(|e| e.delta_tokens.as_ref().map(|t| t.input).unwrap_or(0))
            .sum::<u64>();
        let session_output_tokens = s_entries
            .iter()
            .map(|e| e.delta_tokens.as_ref().map(|t| t.output).unwrap_or(0))
            .sum::<u64>();
        let session_cache_read = s_entries
            .iter()
            .map(|e| {
                e.delta_tokens
                    .as_ref()
                    .and_then(|t| t.cache_read)
                    .unwrap_or(0)
            })
            .sum::<u64>();
        let session_cache_write = s_entries
            .iter()
            .map(|e| {
                e.delta_tokens
                    .as_ref()
                    .and_then(|t| t.cache_write)
                    .unwrap_or(0)
            })
            .sum::<u64>();
        let session_reasoning = s_entries
            .iter()
            .map(|e| {
                e.delta_tokens
                    .as_ref()
                    .and_then(|t| t.reasoning)
                    .unwrap_or(0)
            })
            .sum::<u64>();

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

        let total_cache_read_tokens = if session_tokens > 0 {
            session_cache_read
        } else {
            last_entry
                .tokens
                .as_ref()
                .and_then(|t| t.cache_read)
                .unwrap_or(0)
        };
        let total_cache_write_tokens = if session_tokens > 0 {
            session_cache_write
        } else {
            last_entry
                .tokens
                .as_ref()
                .and_then(|t| t.cache_write)
                .unwrap_or(0)
        };
        let total_reasoning_tokens = if session_tokens > 0 {
            session_reasoning
        } else {
            last_entry
                .tokens
                .as_ref()
                .and_then(|t| t.reasoning)
                .unwrap_or(0)
        };
        let total_input_tokens = if session_tokens > 0 {
            session_input_tokens
        } else {
            last_entry.tokens.as_ref().map(|t| t.input).unwrap_or(0)
        };
        let total_output_tokens = if session_tokens > 0 {
            session_output_tokens
        } else {
            last_entry.tokens.as_ref().map(|t| t.output).unwrap_or(0)
        };

        let cost_usd = match calculate_usage_cost(
            &pricing_rules,
            last_entry.model.as_deref(),
            total_input_tokens,
            total_output_tokens,
            total_cache_read_tokens,
        ) {
            Ok(v) => v,
            Err(err) => {
                eprintln!("⚠️ 計算成本失敗: {}", err);
                0.0
            }
        };
        summary.total_cost_usd += cost_usd;

        sessions_summary.push(SessionSummary {
            session_id: session_id.clone(),
            session_name: last_entry
                .session_name
                .unwrap_or_else(|| "Start Coding Session".to_string()),
            assistant_type: ast_type.clone(),
            source_kind: last_entry
                .source_kind
                .clone()
                .unwrap_or_else(|| "legacy".to_string()),
            cwd: last_entry.cwd.unwrap_or_default(),
            model: last_entry
                .model
                .unwrap_or_else(|| "Unknown Model".to_string()),
            total_tokens: if session_tokens > 0 {
                session_tokens
            } else {
                last_entry.tokens.as_ref().map(|t| t.total).unwrap_or(0)
            },
            total_input_tokens,
            total_output_tokens,
            total_cache_read_tokens,
            total_cache_write_tokens,
            total_reasoning_tokens,
            max_turn_no: s_entries.iter().map(|e| e.turn_no).max().unwrap_or(1),
            timestamp: s_entries[0].timestamp.clone(),
            duration_ms: session_duration,
            cost_usd,
            parent_session_id: last_entry.parent_session_id.clone(),
            agent_nickname: last_entry.agent_nickname.clone(),
            agent_role: last_entry.agent_role.clone(),
            reasoning_effort: last_entry.reasoning_effort.clone(),
        });
    }

    sessions_summary.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

    Json(UsageDetailsResponse {
        date,
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

pub async fn get_session_details(
    Path((assistant, session_id)): Path<(String, String)>,
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

    let session_info: Result<(String, Option<String>, String), String> =
        tokio::task::spawn_blocking({
            let sid = session_id.clone();
            let assistant_name = assistant.clone();
            move || {
                let conn = db::get_db_conn()?;
                db::get_session_assistant_and_transcript(&conn, &assistant_name, &sid)
            }
        })
        .await
        .unwrap_or_else(|_| Err("執行緒執行失敗".to_string()));

    let (resolved_assistant, transcript_path_db, source_kind) = match session_info {
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
    ) {
        Ok(path) => path,
        Err((status, error)) => {
            return (status, Json(serde_json::json!({ "error": error }))).into_response();
        }
    };

    if !filepath.exists() {
        // 判斷是否為「尚未開始交談」（session 目錄存在但 events.jsonl 尚未產生）
        let is_session_dir_present = match resolved_assistant.as_str() {
            "copilot" => {
                let cop_dir = db::get_copilot_dir();
                cop_dir.join("session-state").join(&session_id).exists()
            }
            _ => false,
        };
        let reason = if is_session_dir_present {
            "no_events_yet"
        } else {
            "file_missing"
        };
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": format!("找不到該會話的本地日誌檔: {:?}", filepath), "reason": reason }))).into_response();
    }

    // 3. 預先載入 SQLite 中的回合 (turn_no) 增量 token 數據
    let sid_clone = session_id.clone();
    let (db_entries, session_cwd): (HashMap<u32, (TokenStats, String)>, Option<String>) =
        tokio::task::spawn_blocking(move || {
            if let Ok(conn) = db::get_db_conn() {
                let session_cwd = db::get_session_cwd(&conn, &sid_clone).unwrap_or(None);
                let map = db::get_session_turns_token_stats(&conn, &sid_clone).unwrap_or_default();
                (map, session_cwd)
            } else {
                (HashMap::new(), None)
            }
        })
        .await
        .unwrap_or_else(|_| (HashMap::new(), None));

    // 依據不同助理格式解析日誌
    let (timeline, mut metadata) = match parse_session_timeline_file(
        &resolved_assistant,
        &source_kind,
        &filepath,
        &db_entries,
    ) {
        Ok(result) => result,
        Err((status, error)) => {
            return (status, Json(serde_json::json!({ "error": error }))).into_response();
        }
    };

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
}
