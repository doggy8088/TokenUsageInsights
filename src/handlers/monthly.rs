use axum::{
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::collections::HashMap;

use super::*;
use crate::db;
use crate::pricing::{load_prepared_pricing_rules, PreparedPricingRules};
use crate::reporting::{build_period_report, cursor_session_mode, group_sessions};
use crate::session_identity::SessionIdentity;

#[derive(serde::Deserialize)]
pub struct ModelSessionsQuery {
    period: String,
    model: String,
    mode: Option<String>,
}

#[derive(Clone, Copy)]
enum ModelSessionPeriod {
    Month,
    Year,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ModelSessionMode {
    Agent,
    Ide,
    Unclassified,
}

impl ModelSessionMode {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "agent" => Some(Self::Agent),
            "ide" => Some(Self::Ide),
            "unclassified" => Some(Self::Unclassified),
            _ => None,
        }
    }

    fn label(self) -> Option<&'static str> {
        match self {
            Self::Agent => Some("agent"),
            Self::Ide => Some("ide"),
            Self::Unclassified => None,
        }
    }

    fn matches(self, mode: Option<&str>) -> bool {
        match self {
            Self::Agent => mode == Some("agent"),
            Self::Ide => mode == Some("ide"),
            Self::Unclassified => mode.is_none(),
        }
    }
}

fn parse_model_session_period(value: &str) -> Option<ModelSessionPeriod> {
    match value.len() {
        4 => chrono::NaiveDate::parse_from_str(&format!("{value}-01-01"), "%Y-%m-%d")
            .ok()
            .map(|_| ModelSessionPeriod::Year),
        7 => chrono::NaiveDate::parse_from_str(&format!("{value}-01"), "%Y-%m-%d")
            .ok()
            .map(|_| ModelSessionPeriod::Month),
        _ => None,
    }
}

fn valid_model_session_date(value: &str) -> Option<String> {
    if value.len() != 10 {
        return None;
    }
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .ok()
        .map(|_| value.to_string())
}

fn collect_model_session_details(
    entries_with_type: &[(UsageEntry, String, String)],
    requested_model: &str,
    requested_mode: Option<ModelSessionMode>,
    pricing_rules: &PreparedPricingRules,
) -> Vec<ModelSessionDetail> {
    let sessions_map = group_sessions(
        entries_with_type
            .iter()
            .map(|(entry, assistant_type, _)| (entry, assistant_type.as_str())),
    );
    let mut session_first_entries: HashMap<SessionIdentity, (UsageEntry, String)> = HashMap::new();

    for (entry, assistant_type, entry_date) in entries_with_type {
        let identity = SessionIdentity::from_entry(assistant_type, entry);
        let first = session_first_entries
            .entry(identity)
            .or_insert_with(|| (entry.clone(), entry_date.clone()));
        if entry.turn_no < first.0.turn_no
            || (entry.turn_no == first.0.turn_no && entry.timestamp < first.0.timestamp)
        {
            *first = (entry.clone(), entry_date.clone());
        }
    }

    let mut details = Vec::new();
    for (identity, group) in sessions_map {
        let entries = group.entries;
        let mode = cursor_session_mode(&identity.assistant_type, &entries);
        if requested_mode.is_some_and(|requested| !requested.matches(mode)) {
            continue;
        }
        let session_usage = summarize_session_usage(pricing_rules, &entries);
        let Some(model_usage) = session_usage
            .models
            .iter()
            .find(|usage| usage.model == requested_model)
        else {
            continue;
        };
        let Some((first_entry, first_date)) = session_first_entries.get(&identity) else {
            continue;
        };
        let last_entry = entries
            .iter()
            .max_by(|left, right| {
                left.turn_no
                    .cmp(&right.turn_no)
                    .then_with(|| left.timestamp.cmp(&right.timestamp))
            })
            .unwrap_or(first_entry);
        let duration_ms = last_entry
            .cost
            .as_ref()
            .and_then(|cost| cost.total_api_duration_ms)
            .unwrap_or(0.0) as u64;
        let total_requests = last_entry
            .cost
            .as_ref()
            .and_then(|cost| cost.total_premium_requests)
            .unwrap_or(0.0) as u64;

        details.push(ModelSessionDetail {
            session_id: identity.session_id.clone(),
            session_name: last_entry
                .session_name
                .clone()
                .unwrap_or_else(|| identity.session_id.clone()),
            assistant_type: identity.assistant_type,
            source_kind: identity.source_kind,
            source_dir_key: identity.source_dir_key,
            date: valid_model_session_date(first_date),
            timestamp: first_entry.timestamp.clone(),
            cwd: last_entry.cwd.clone().unwrap_or_default(),
            model: requested_model.to_string(),
            total_tokens: model_usage.usage.total_tokens,
            total_input_tokens: model_usage.usage.input_tokens,
            total_output_tokens: model_usage.usage.output_tokens,
            total_cache_read_tokens: model_usage.usage.cache_read_tokens,
            total_cache_write_tokens: model_usage.usage.cache_write_tokens,
            total_reasoning_tokens: model_usage.usage.reasoning_tokens,
            max_turn_no: entries.iter().map(|entry| entry.turn_no).max().unwrap_or(1),
            duration_ms,
            total_requests,
            cost_usd: model_usage.usage.cost_usd,
            session_model: session_usage.display_model.clone(),
            session_total_tokens: session_usage.usage.total_tokens,
            session_total_input_tokens: session_usage.usage.input_tokens,
            session_total_output_tokens: session_usage.usage.output_tokens,
            session_total_cache_read_tokens: session_usage.usage.cache_read_tokens,
            session_total_cache_write_tokens: session_usage.usage.cache_write_tokens,
            session_total_reasoning_tokens: session_usage.usage.reasoning_tokens,
            session_cost_usd: session_usage.usage.cost_usd,
            parent_session_id: last_entry.parent_session_id.clone(),
            agent_nickname: last_entry.agent_nickname.clone(),
            agent_role: last_entry.agent_role.clone(),
            reasoning_effort: last_entry.reasoning_effort.clone(),
        });
    }

    details.sort_by(|left, right| {
        right
            .timestamp
            .cmp(&left.timestamp)
            .then_with(|| left.session_id.cmp(&right.session_id))
            .then_with(|| left.assistant_type.cmp(&right.assistant_type))
            .then_with(|| left.source_kind.cmp(&right.source_kind))
            .then_with(|| left.source_dir_key.cmp(&right.source_dir_key))
    });
    details
}

pub async fn get_model_sessions(
    Path(assistant): Path<String>,
    Query(query): Query<ModelSessionsQuery>,
) -> impl IntoResponse {
    let assistant = normalize_assistant_name(&assistant);
    if !is_supported_assistant(&assistant) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "不支援的助理類型" })),
        )
            .into_response();
    }

    let period = query.period.trim().to_string();
    let model = query.model.trim().to_string();
    let mode =
        match query.mode.as_deref().map(str::trim) {
            Some(value) => match ModelSessionMode::parse(value) {
                Some(mode) => Some(mode),
                None => return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": "Mode 必須是 agent、ide 或 unclassified" })),
                )
                    .into_response(),
            },
            None => None,
        };
    let Some(period_kind) = parse_model_session_period(&period) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "期間必須是 YYYY 或 YYYY-MM" })),
        )
            .into_response();
    };
    if model.is_empty() || model.len() > 200 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "模型名稱無效" })),
        )
            .into_response();
    }

    let assistant_for_query = assistant.clone();
    let period_for_query = period.clone();
    let model_for_query = model.clone();
    let mode_for_query = mode;
    let result = tokio::task::spawn_blocking(move || {
        let conn = db::get_db_conn()?;
        let entries = match period_kind {
            ModelSessionPeriod::Month => {
                db::get_usage_entries_by_month(&conn, &period_for_query, &assistant_for_query)?
            }
            ModelSessionPeriod::Year => {
                db::get_usage_entries_by_year(&conn, &period_for_query, &assistant_for_query)?
            }
        };
        Ok::<_, String>(collect_model_session_details(
            &entries,
            &model_for_query,
            mode_for_query,
            &load_prepared_pricing_rules(),
        ))
    })
    .await
    .unwrap_or_else(|_| Err("執行緒執行失敗".to_string()));

    match result {
        Ok(sessions) => Json(ModelSessionsResponse {
            period,
            model,
            mode: mode.and_then(ModelSessionMode::label).map(str::to_string),
            sessions,
        })
        .into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": error })),
        )
            .into_response(),
    }
}

/// API 5: 獲取可用的有使用記錄月份
pub async fn get_available_months(Path(assistant): Path<String>) -> impl IntoResponse {
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
        db::get_available_months(&conn, &assistant)
    })
    .await
    .unwrap_or_else(|_| Err("執行緒執行失敗".to_string()));

    match res {
        Ok(month_list) => Json(MonthListResponse { months: month_list }).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        )
            .into_response(),
    }
}

/// API 6: 獲取指定月份的統計摘要數據
pub async fn get_monthly_details(
    Path((assistant, year_month)): Path<(String, String)>,
) -> impl IntoResponse {
    let assistant = normalize_assistant_name(&assistant);
    if !is_supported_assistant(&assistant) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "不支援的助理類型" })),
        )
            .into_response();
    }

    let report = tokio::task::spawn_blocking(move || {
        let conn = db::get_db_conn()?;
        let entries = db::get_usage_entries_by_month(&conn, &year_month, &assistant)?;
        if entries.is_empty() {
            return Ok(None);
        }
        let pricing_rules = load_prepared_pricing_rules();
        Ok::<_, String>(Some((
            year_month,
            build_period_report(&entries, str::to_string, &pricing_rules),
        )))
    })
    .await
    .unwrap_or_else(|_| Err("執行緒執行失敗".to_string()));

    match report {
        Ok(Some((year_month, report))) => Json(MonthlyDetailsResponse {
            year_month,
            summary: report.summary,
            daily_breakdown: report
                .breakdown
                .into_iter()
                .map(|item| MonthlyDailyBreakdown {
                    date: item.label,
                    total_tokens: item.usage.total_tokens,
                    total_input_tokens: item.usage.input_tokens,
                    total_output_tokens: item.usage.output_tokens,
                    total_cache_read_tokens: item.usage.cache_read_tokens,
                    total_reasoning_tokens: item.usage.reasoning_tokens,
                    sessions_count: item.sessions_count,
                    cost_usd: item.usage.cost_usd,
                })
                .collect(),
            projects: report.projects,
            models: report.models,
            agent_breakdown: report.agent_breakdown,
        })
        .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "找不到該月份的使用量資料。" })),
        )
            .into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": error })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::TokenStats;
    use crate::pricing::PricingRule;

    fn model_entry(
        session_id: &str,
        turn_no: u32,
        timestamp: &str,
        model: &str,
        source_kind: &str,
        input: u64,
        output: u64,
    ) -> UsageEntry {
        let tokens = TokenStats {
            input,
            output,
            cache_read: Some(0),
            cache_write: Some(0),
            cache_write_5m: None,
            cache_write_1h: None,
            reasoning: None,
            total: input + output,
        };
        UsageEntry {
            timestamp: timestamp.to_string(),
            session_id: session_id.to_string(),
            session_name: Some(format!("Session {session_id}")),
            transcript_path: None,
            cwd: Some("/workspace/project".to_string()),
            version: None,
            turn_no,
            model: Some(model.to_string()),
            model_id: Some(model.to_string()),
            tokens: Some(tokens.clone()),
            delta_tokens: Some(tokens),
            context: None,
            cost: None,
            source_kind: Some(source_kind.to_string()),
            source_dir_key: None,
            parent_session_id: None,
            agent_nickname: None,
            agent_role: None,
            reasoning_effort: None,
        }
    }

    #[test]
    fn model_session_period_accepts_only_valid_months_and_years() {
        assert!(matches!(
            parse_model_session_period("2026"),
            Some(ModelSessionPeriod::Year)
        ));
        assert!(matches!(
            parse_model_session_period("2026-07"),
            Some(ModelSessionPeriod::Month)
        ));
        assert!(parse_model_session_period("2026-13").is_none());
        assert!(parse_model_session_period("20x6").is_none());
        assert!(parse_model_session_period("2026-07-10").is_none());
    }

    #[test]
    fn model_session_details_are_model_specific_and_keep_invalid_dates_null() {
        let entries = vec![
            (
                model_entry(
                    "mixed",
                    1,
                    "2026-07-10T10:00:00Z",
                    "composer-2.5",
                    "cursor-agent",
                    100,
                    50,
                ),
                "cursor".to_string(),
                "2026-07-10".to_string(),
            ),
            (
                model_entry(
                    "mixed",
                    2,
                    "2026-07-10T10:05:00Z",
                    "cursor-grok-4.5",
                    "cursor-agent",
                    200,
                    100,
                ),
                "cursor".to_string(),
                "2026-07-10".to_string(),
            ),
            (
                model_entry(
                    "invalid-date",
                    1,
                    "2026-07-11T09:00:00Z",
                    "composer-2.5",
                    "cursor-agent",
                    20,
                    10,
                ),
                "cursor".to_string(),
                "not-a-date".to_string(),
            ),
        ];
        let rules = [
            PricingRule {
                model_name: "composer-2.5".to_string(),
                input_price: 1.0,
                cache_input_price: 0.0,
                output_price: 2.0,
            },
            PricingRule {
                model_name: "cursor-grok-4.5".to_string(),
                input_price: 2.0,
                cache_input_price: 0.0,
                output_price: 3.0,
            },
        ];

        let details = collect_model_session_details(
            &entries,
            "composer-2.5",
            Some(ModelSessionMode::Agent),
            &PreparedPricingRules::from_rules(rules.into()),
        );

        assert_eq!(details.len(), 2);
        let mixed = details
            .iter()
            .find(|detail| detail.session_id == "mixed")
            .unwrap();
        assert_eq!(mixed.model, "composer-2.5");
        assert_eq!(mixed.total_tokens, 150);
        assert_eq!(mixed.total_input_tokens, 100);
        assert_eq!(mixed.total_output_tokens, 50);
        assert_eq!(mixed.max_turn_no, 2);
        assert_eq!(mixed.date.as_deref(), Some("2026-07-10"));
        assert_eq!(mixed.session_model, "cursor-grok-4.5");
        assert_eq!(mixed.session_total_tokens, 450);
        assert_eq!(mixed.session_total_input_tokens, 300);
        assert_eq!(mixed.session_total_output_tokens, 150);
        assert!((mixed.cost_usd - 0.0002).abs() < 1e-12);
        assert!((mixed.session_cost_usd - 0.0009).abs() < 1e-12);

        let invalid_date = details
            .iter()
            .find(|detail| detail.session_id == "invalid-date")
            .unwrap();
        assert!(invalid_date.date.is_none());
    }

    #[test]
    fn model_summaries_separate_cursor_agent_and_ide_modes() {
        let entries = [
            model_entry(
                "agent",
                1,
                "2026-07-10T10:00:00Z",
                "composer-2.5",
                "cursor-agent",
                100,
                50,
            ),
            model_entry(
                "ide",
                1,
                "2026-07-10T11:00:00Z",
                "composer-2.5",
                "cursor-ide",
                80,
                20,
            ),
        ];
        let sessions =
            crate::reporting::group_sessions(entries.iter().map(|entry| (entry, "cursor")));
        let summaries = crate::reporting::summarize_models_by_mode(
            &sessions,
            &PreparedPricingRules::from_rules(Vec::new()),
        );

        assert_eq!(summaries.len(), 2);
        assert!(summaries.iter().any(|summary| {
            summary.model == "composer-2.5"
                && summary.mode.as_deref() == Some("agent")
                && summary.sessions_count == 1
                && summary.total_tokens == 150
        }));
        assert!(summaries.iter().any(|summary| {
            summary.model == "composer-2.5"
                && summary.mode.as_deref() == Some("ide")
                && summary.sessions_count == 1
                && summary.total_tokens == 100
        }));
    }

    #[test]
    fn model_session_details_filter_cursor_mode() {
        let entries = vec![
            (
                model_entry(
                    "agent",
                    1,
                    "2026-07-10T10:00:00Z",
                    "composer-2.5",
                    "cursor-agent",
                    100,
                    50,
                ),
                "cursor".to_string(),
                "2026-07-10".to_string(),
            ),
            (
                model_entry(
                    "ide",
                    1,
                    "2026-07-10T11:00:00Z",
                    "composer-2.5",
                    "cursor-ide",
                    80,
                    20,
                ),
                "cursor".to_string(),
                "2026-07-10".to_string(),
            ),
        ];

        let details = collect_model_session_details(
            &entries,
            "composer-2.5",
            Some(ModelSessionMode::Ide),
            &PreparedPricingRules::from_rules(Vec::new()),
        );

        assert_eq!(details.len(), 1);
        assert_eq!(details[0].session_id, "ide");
    }

    #[test]
    fn model_session_details_keep_same_id_sources_separate() {
        let entries = vec![
            (
                model_entry(
                    "shared",
                    1,
                    "2026-07-10T10:00:00Z",
                    "gpt-5",
                    "copilot-cli",
                    100,
                    10,
                ),
                "copilot".to_string(),
                "2026-07-10".to_string(),
            ),
            (
                model_entry(
                    "shared",
                    1,
                    "2026-07-10T11:00:00Z",
                    "gpt-5",
                    "vscode-chat",
                    200,
                    20,
                ),
                "copilot".to_string(),
                "2026-07-10".to_string(),
            ),
        ];

        let details = collect_model_session_details(
            &entries,
            "gpt-5",
            None,
            &PreparedPricingRules::from_rules(Vec::new()),
        );

        assert_eq!(details.len(), 2);
        assert!(details
            .iter()
            .any(|detail| { detail.source_kind == "copilot-cli" && detail.total_tokens == 110 }));
        assert!(details
            .iter()
            .any(|detail| { detail.source_kind == "vscode-chat" && detail.total_tokens == 220 }));
    }

    #[test]
    fn model_session_details_keep_same_id_source_directories_separate() {
        let mut first = model_entry(
            "shared",
            1,
            "2026-07-10T10:00:00Z",
            "gpt-5",
            "copilot-app",
            100,
            10,
        );
        first.source_dir_key = Some("aa".to_string());
        let mut second = model_entry(
            "shared",
            1,
            "2026-07-10T11:00:00Z",
            "gpt-5",
            "copilot-app",
            200,
            20,
        );
        second.source_dir_key = Some("bb".to_string());
        let entries = vec![
            (first, "copilot".to_string(), "2026-07-10".to_string()),
            (second, "copilot".to_string(), "2026-07-10".to_string()),
        ];

        let details = collect_model_session_details(
            &entries,
            "gpt-5",
            None,
            &PreparedPricingRules::from_rules(Vec::new()),
        );

        assert_eq!(details.len(), 2);
        assert_eq!(
            details
                .iter()
                .map(|detail| detail.total_tokens)
                .sum::<u64>(),
            330
        );
    }
}
