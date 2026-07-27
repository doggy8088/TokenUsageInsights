use axum::{extract::Path, http::StatusCode, response::IntoResponse, Json};
use std::collections::{HashMap, HashSet};

use super::*;
use crate::db;
use crate::pricing::load_pricing_rules;

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

    let assistant_clone = assistant.clone();
    let year_clone = year.clone();

    let entries_res: Result<Vec<(UsageEntry, String, String)>, String> =
        tokio::task::spawn_blocking(move || {
            let conn = db::get_db_conn()?;
            db::get_usage_entries_by_year(&conn, &year_clone, &assistant_clone)
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
            Json(serde_json::json!({ "error": "找不到該年份的使用量資料。" })),
        )
            .into_response();
    }

    let pricing_rules = load_pricing_rules();
    let mut monthly_map: HashMap<String, Vec<(UsageEntry, String)>> = HashMap::new();
    let mut sessions_map: HashMap<String, (Vec<UsageEntry>, String)> = HashMap::new();

    for (e, ast_type, entry_date) in &entries_with_type {
        let month_str = if entry_date.len() >= 7 {
            &entry_date[0..7]
        } else {
            "Unknown"
        };
        monthly_map
            .entry(month_str.to_string())
            .or_default()
            .push((e.clone(), ast_type.clone()));
        let (list, _) = sessions_map
            .entry(e.session_id.clone())
            .or_insert_with(|| (Vec::new(), ast_type.clone()));
        list.push(e.clone());
    }

    let mut monthly_breakdown = Vec::new();
    let mut yearly_summary = DaySummary {
        total_sessions: sessions_map.len(),
        ..Default::default()
    };

    let mut session_last_entries: HashMap<String, UsageEntry> = HashMap::new();
    for (e, _, _) in &entries_with_type {
        let sid = e.session_id.clone();
        let last_e = session_last_entries.entry(sid).or_insert_with(|| e.clone());
        if e.turn_no > last_e.turn_no {
            *last_e = e.clone();
        }
    }

    // 計算每月彙整與年彙整
    let mut sorted_months: Vec<String> = monthly_map.keys().cloned().collect();
    sorted_months.sort();

    for month_str in sorted_months {
        let month_entries_with_type = monthly_map.get(&month_str).unwrap();
        let mut m_tokens = 0;
        let mut m_input = 0;
        let mut m_output = 0;
        let mut m_reasoning = 0;
        let mut m_cache_read = 0;
        let mut m_cache_write = 0;
        let mut m_cost_usd = 0.0;
        let mut m_sessions = HashSet::new();

        let mut month_sessions_map: HashMap<String, Vec<UsageEntry>> = HashMap::new();
        for (e, _) in month_entries_with_type {
            m_sessions.insert(e.session_id.clone());
            month_sessions_map
                .entry(e.session_id.clone())
                .or_default()
                .push(e.clone());
        }

        for s_entries in month_sessions_map.values() {
            let session_usage = summarize_session_usage(&pricing_rules, s_entries);
            m_tokens += session_usage.usage.total_tokens;
            m_input += session_usage.usage.input_tokens;
            m_output += session_usage.usage.output_tokens;
            m_cache_read += session_usage.usage.cache_read_tokens;
            m_cache_write += session_usage.usage.cache_write_tokens;
            m_reasoning += session_usage.usage.reasoning_tokens;
            m_cost_usd += session_usage.usage.cost_usd;
        }

        yearly_summary.total_tokens += m_tokens;
        yearly_summary.total_input_tokens += m_input;
        yearly_summary.total_output_tokens += m_output;
        yearly_summary.total_cache_read_tokens += m_cache_read;
        yearly_summary.total_cache_write_tokens += m_cache_write;
        yearly_summary.total_reasoning_tokens += m_reasoning;
        yearly_summary.total_cost_usd += m_cost_usd;

        monthly_breakdown.push(YearlyMonthlyBreakdown {
            month: month_str,
            total_tokens: m_tokens,
            total_input_tokens: m_input,
            total_output_tokens: m_output,
            total_cache_read_tokens: m_cache_read,
            total_reasoning_tokens: m_reasoning,
            sessions_count: m_sessions.len(),
            cost_usd: m_cost_usd,
        });
    }

    // 按專案統計 (CWD)
    let mut project_map_stats: HashMap<String, (usize, u64, f64)> = HashMap::new();
    // 按模型統計 (Model)
    let mut model_map_stats: HashMap<String, (usize, u64, u64, u64, u64, f64)> = HashMap::new();
    // 按 Agent 類型統計
    let mut agent_map_stats: HashMap<String, AgentBreakdown> = HashMap::new();

    for (session_id, (s_entries, ast_type)) in &sessions_map {
        let last_entry = session_last_entries
            .get(session_id)
            .cloned()
            .unwrap_or_else(|| s_entries[0].clone());
        let session_usage = summarize_session_usage(&pricing_rules, s_entries);

        let cwd = last_entry.cwd.unwrap_or_else(|| "Unknown CWD".to_string());
        let project_stat = project_map_stats.entry(cwd).or_insert((0, 0, 0.0));
        project_stat.0 += 1;
        project_stat.1 += session_usage.usage.total_tokens;
        project_stat.2 += session_usage.usage.cost_usd;

        for model_usage in &session_usage.models {
            let model_stat = model_map_stats
                .entry(model_usage.model.clone())
                .or_insert((0, 0, 0, 0, 0, 0.0));
            model_stat.0 += 1;
            model_stat.1 += model_usage.usage.total_tokens;
            model_stat.2 += model_usage.usage.input_tokens;
            model_stat.3 += model_usage.usage.output_tokens;
            model_stat.4 += model_usage.usage.cache_read_tokens;
            model_stat.5 += model_usage.usage.cost_usd;
        }

        let agent_stat = agent_map_stats.entry(ast_type.clone()).or_default();
        agent_stat.total_tokens += session_usage.usage.total_tokens;
        agent_stat.total_input_tokens += session_usage.usage.input_tokens;
        agent_stat.total_output_tokens += session_usage.usage.output_tokens;
        agent_stat.total_cache_read_tokens += session_usage.usage.cache_read_tokens;
        agent_stat.total_reasoning_tokens += session_usage.usage.reasoning_tokens;
        agent_stat.total_cost_usd += session_usage.usage.cost_usd;
        agent_stat.total_sessions += 1;
    }

    let mut project_summaries = Vec::new();
    for (cwd, (sessions_count, total_tokens, cost_usd)) in project_map_stats {
        project_summaries.push(MonthlyProjectSummary {
            cwd,
            sessions_count,
            total_tokens,
            cost_usd,
        });
    }
    project_summaries.sort_by_key(|item| std::cmp::Reverse(item.total_tokens));

    let mut model_summaries = Vec::new();
    for (
        model,
        (
            sessions_count,
            total_tokens,
            total_input_tokens,
            total_output_tokens,
            total_cache_read_tokens,
            cost_usd,
        ),
    ) in model_map_stats
    {
        model_summaries.push(MonthlyModelSummary {
            model,
            sessions_count,
            total_tokens,
            total_input_tokens,
            total_output_tokens,
            total_cache_read_tokens,
            cost_usd,
        });
    }
    model_summaries.sort_by_key(|item| std::cmp::Reverse(item.total_tokens));

    Json(YearlyDetailsResponse {
        year,
        summary: yearly_summary,
        monthly_breakdown,
        projects: project_summaries,
        models: model_summaries,
        agent_breakdown: agent_map_stats,
    })
    .into_response()
}
