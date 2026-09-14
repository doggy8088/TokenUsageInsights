use super::*;
use crate::db;
use crate::pricing::load_prepared_pricing_rules;
use crate::reporting::build_period_report;
use axum::{extract::Path, http::StatusCode, response::IntoResponse, Json};

/// API 12: 獲取可用的有使用記錄年份
pub async fn get_available_years(Path(assistant): Path<String>) -> impl IntoResponse {
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
        db::get_available_years(&conn, &assistant)
    })
    .await
    .unwrap_or_else(|_| Err("執行緒執行失敗".to_string()));

    match res {
        Ok(year_list) => Json(YearListResponse { years: year_list }).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        )
            .into_response(),
    }
}

/// API 13: 獲取指定年份的統計摘要數據
pub async fn get_yearly_details(
    Path((assistant, year)): Path<(String, String)>,
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
        let entries = db::get_usage_entries_by_year(&conn, &year, &assistant)?;
        if entries.is_empty() {
            return Ok(None);
        }
        let pricing_rules = load_prepared_pricing_rules();
        let report = build_period_report(
            &entries,
            |date| date.get(..7).unwrap_or("Unknown").to_string(),
            &pricing_rules,
        );
        Ok::<_, String>(Some((year, report)))
    })
    .await
    .unwrap_or_else(|_| Err("執行緒執行失敗".to_string()));

    match report {
        Ok(Some((year, report))) => Json(YearlyDetailsResponse {
            year,
            summary: report.summary,
            monthly_breakdown: report
                .breakdown
                .into_iter()
                .map(|item| YearlyMonthlyBreakdown {
                    month: item.label,
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
            Json(serde_json::json!({ "error": "找不到該年份的使用量資料。" })),
        )
            .into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": error })),
        )
            .into_response(),
    }
}
