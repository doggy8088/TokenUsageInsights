use axum::{
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use std::path::PathBuf;

use super::*;
use crate::db;
use crate::pricing::{load_prepared_pricing_rules, PreparedPricingRules};
use crate::reporting::{group_sessions, latest_usage_entry};
use crate::session_details::load_session_details;
use crate::session_files::is_safe_session_id;
use crate::session_search;

#[cfg(test)]
use crate::db::TokenStats;
#[cfg(test)]
use crate::timeline::{parse_grok_timeline, TimelineItem};
#[cfg(test)]
use std::{fs::File, io::BufReader};

fn aggregate_usage_details(
    entries_with_type: &[crate::db::UsageDayRecordWithAssistant],
    pricing_rules: &PreparedPricingRules,
) -> (DaySummary, Vec<SessionSummary>, Vec<RawUsageEntry>) {
    let mut summary = DaySummary::default();
    let sessions_map = group_sessions(
        entries_with_type
            .iter()
            .map(|row| (&row.record.entry, row.assistant_type.as_str())),
    );
    let entries = entries_with_type
        .iter()
        .map(|row| RawUsageEntry {
            assistant_type: row.assistant_type.clone(),
            entry: row.record.entry.clone(),
        })
        .collect::<Vec<_>>();

    summary.total_sessions = sessions_map.len();

    let mut sessions_summary = Vec::new();
    for (identity, group) in &sessions_map {
        let s_entries = &group.entries;
        let Some(last_entry) = latest_usage_entry(s_entries) else {
            continue;
        };
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

        summary.add_usage(&session_usage.usage);

        sessions_summary.push(SessionSummary {
            session_id: identity.session_id.clone(),
            session_name: last_entry
                .session_name
                .clone()
                .unwrap_or_else(|| "Start Coding Session".to_string()),
            assistant_type: identity.assistant_type.clone(),
            source_kind: identity.source_kind.clone(),
            source_dir_key: identity.source_dir_key.clone(),
            cwd: last_entry.cwd.clone().unwrap_or_default(),
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

#[derive(Deserialize)]
pub struct SessionSearchQuery {
    q: String,
}

#[derive(Deserialize, Default)]
pub struct SessionDetailsQuery {
    source_kind: Option<String>,
    source_dir_key: Option<String>,
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

    let mcode_dir = db::get_mcode_dir();
    let mcode_exists = mcode_dir.join("sessions").exists();

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
        mcode: AssistantSetupStatus {
            dir_path: mcode_dir.to_string_lossy().into_owned(),
            data_path: mcode_dir.join("sessions").to_string_lossy().into_owned(),
            exists: mcode_exists,
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

    let entries_res: Result<Vec<crate::db::UsageDayRecordWithAssistant>, String> =
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
    let search_result = tokio::task::spawn_blocking(move || {
        session_search::search_user_prompts(&assistant, &date, &normalized_query)
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
    use crate::db::{UsageDayExportRecord, UsageDayRecordWithAssistant};
    use crate::pricing::PricingRule;
    use rusqlite::Connection;
    use std::time::{SystemTime, UNIX_EPOCH};
    use std::{collections::HashMap, fs};

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

    fn usage_day_record(entry: UsageEntry, assistant_type: &str) -> UsageDayRecordWithAssistant {
        UsageDayRecordWithAssistant {
            record: UsageDayExportRecord {
                entry,
                import_source_id: None,
                usage_identity: None,
            },
            assistant_type: assistant_type.to_string(),
            date: "2026-07-10".to_string(),
        }
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

        let entries = crate::db::get_usage_entries_by_date(&conn, "2026-07-20", "copilot").unwrap();
        let (_, sessions, _) =
            aggregate_usage_details(&entries, &PreparedPricingRules::from_rules(Vec::new()));

        assert_eq!(
            sessions.len(),
            2,
            "copilot-cli and copilot-app with same session_id must be 2 separate sessions"
        );

        let source_kinds: Vec<&str> = sessions
            .iter()
            .map(|session| session.source_kind.as_str())
            .collect();
        assert!(
            source_kinds.contains(&"copilot-cli"),
            "must have a copilot-cli session, got: {:?}",
            source_kinds
        );
        assert!(
            source_kinds.contains(&"copilot-app"),
            "must have a copilot-app session, got: {:?}",
            source_kinds
        );

        let app_session = sessions
            .iter()
            .find(|session| session.source_kind == "copilot-app")
            .unwrap();
        assert_eq!(app_session.model, "GLM5.2");
    }

    #[test]
    fn daily_summary_separates_same_session_identity_across_assistants() {
        let tokens = TokenStats {
            input: 10,
            output: 5,
            cache_read: Some(0),
            cache_write: Some(0),
            cache_write_5m: None,
            cache_write_1h: None,
            reasoning: None,
            total: 15,
        };
        let mut first = legacy_usage_entry(1, "shared-model", tokens.clone());
        first.session_id = "shared-session".to_string();
        first.source_kind = Some("legacy".to_string());
        let second = first.clone();
        let entries = vec![
            usage_day_record(first, "claude"),
            usage_day_record(second, "codex"),
        ];

        let (summary, sessions, raw_entries) =
            aggregate_usage_details(&entries, &PreparedPricingRules::from_rules(Vec::new()));

        assert_eq!(summary.total_sessions, 2);
        assert_eq!(sessions.len(), 2);
        assert_eq!(raw_entries.len(), 2);
        assert!(sessions
            .iter()
            .any(|session| session.assistant_type == "claude"));
        assert!(sessions
            .iter()
            .any(|session| session.assistant_type == "codex"));
        assert!(raw_entries
            .iter()
            .any(|entry| entry.assistant_type == "claude"));
        assert!(raw_entries
            .iter()
            .any(|entry| entry.assistant_type == "codex"));
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

        summary.add_usage(&session_usage.usage);

        assert_eq!(summary.total_tokens, 270);
        assert_eq!(summary.total_input_tokens, 200);
        assert_eq!(summary.total_output_tokens, 40);
        assert_eq!(summary.total_cache_read_tokens, 20);
        assert_eq!(summary.total_reasoning_tokens, 10);
    }
}
