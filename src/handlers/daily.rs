use axum::{
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use std::{collections::HashMap, path::PathBuf};

use super::*;
use crate::db;
use crate::pricing::{load_prepared_pricing_rules, PreparedPricingRules};
use crate::session_details::{load_session_details, parse_session_timeline_file};
use crate::session_files::{is_safe_session_id, resolve_session_file_path};
use crate::timeline::TimelineItem;

#[cfg(test)]
use crate::db::TokenStats;
#[cfg(test)]
use crate::session_files::{
    resolve_copilot_app_events_path, resolve_copilot_cli_subagent_events_path,
};
#[cfg(test)]
use crate::timeline::parse_grok_timeline;
#[cfg(test)]
use std::{fs::File, io::BufReader};

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
///
/// Session 查詢、日誌解析與 Git 子程序都由 `session_details` 服務在
/// blocking thread 執行；HTTP handler 僅負責輸入驗證與回應轉換。
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

    let source_kind = query
        .source_kind
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if source_kind.as_ref().is_some_and(|value| value.len() > 64) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "source_kind 格式不正確。" })),
        )
            .into_response();
    }
    if query
        .source_dir_key
        .as_deref()
        .is_some_and(|key| !is_safe_source_dir_key(key))
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "非法的 source_dir_key 格式。" })),
        )
            .into_response();
    }

    match tokio::task::spawn_blocking(move || {
        load_session_details(assistant, session_id, source_kind, query.source_dir_key)
    })
    .await
    {
        Ok(Ok(payload)) => Json(payload).into_response(),
        Ok(Err(error)) => (error.status, Json(error.payload)).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "執行緒執行失敗" })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::UsageDayExportRecord;
    use crate::pricing::PricingRule;
    use rusqlite::Connection;
    use std::time::{SystemTime, UNIX_EPOCH};
    use std::{
        collections::HashMap,
        fs,
        path::{Path as StdPath, PathBuf},
    };

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
                usage_identity: None,
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
