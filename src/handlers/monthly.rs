use axum::{extract::Path, http::StatusCode, response::IntoResponse, Json};
use std::collections::{HashMap, HashSet};

use super::*;
use crate::db;
use crate::pricing::{calculate_entries_cost, load_pricing_rules};

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

    let assistant_clone = assistant.clone();
    let year_month_clone = year_month.clone();

    let entries_res: Result<Vec<(UsageEntry, String, String)>, String> =
        tokio::task::spawn_blocking(move || {
            let conn = db::get_db_conn()?;
            db::get_usage_entries_by_month(&conn, &year_month_clone, &assistant_clone)
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
            Json(serde_json::json!({ "error": "找不到該月份的使用量資料。" })),
        )
            .into_response();
    }

    let pricing_rules = load_pricing_rules();
    let mut daily_map: HashMap<String, Vec<(UsageEntry, String)>> = HashMap::new();
    let mut sessions_map: HashMap<String, (Vec<UsageEntry>, String)> = HashMap::new();

    for (e, ast_type, entry_date) in &entries_with_type {
        daily_map
            .entry(entry_date.clone())
            .or_default()
            .push((e.clone(), ast_type.clone()));
        let (list, _) = sessions_map
            .entry(e.session_id.clone())
            .or_insert_with(|| (Vec::new(), ast_type.clone()));
        list.push(e.clone());
    }

    let mut daily_breakdown = Vec::new();
    let mut monthly_summary = DaySummary {
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

    // 計算每日彙整與月彙整
    let mut sorted_dates: Vec<String> = daily_map.keys().cloned().collect();
    sorted_dates.sort();

    for date_str in sorted_dates {
        let day_entries_with_type = daily_map.get(&date_str).unwrap();
        let mut day_tokens = 0;
        let mut day_input = 0;
        let mut day_output = 0;
        let mut day_reasoning = 0;
        let mut day_cache_read = 0;
        let mut day_cost_usd = 0.0;
        let mut day_sessions = HashSet::new();

        let mut day_sessions_map: HashMap<String, Vec<UsageEntry>> = HashMap::new();
        for (e, _) in day_entries_with_type {
            day_sessions.insert(e.session_id.clone());
            day_sessions_map
                .entry(e.session_id.clone())
                .or_default()
                .push(e.clone());
        }

        for (sid, s_entries) in &day_sessions_map {
            let s_tokens = s_entries
                .iter()
                .map(|e| e.delta_tokens.as_ref().map(|t| t.total).unwrap_or(0))
                .sum::<u64>();
            let s_input = s_entries
                .iter()
                .map(|e| e.delta_tokens.as_ref().map(|t| t.input).unwrap_or(0))
                .sum::<u64>();
            let s_output = s_entries
                .iter()
                .map(|e| e.delta_tokens.as_ref().map(|t| t.output).unwrap_or(0))
                .sum::<u64>();
            let s_cache = s_entries
                .iter()
                .map(|e| {
                    e.delta_tokens
                        .as_ref()
                        .and_then(|t| t.cache_read)
                        .unwrap_or(0)
                })
                .sum::<u64>();
            let s_reasoning = s_entries
                .iter()
                .map(|e| {
                    e.delta_tokens
                        .as_ref()
                        .and_then(|t| t.reasoning)
                        .unwrap_or(0)
                })
                .sum::<u64>();

            let last_entry = session_last_entries
                .get(sid)
                .cloned()
                .unwrap_or_else(|| s_entries[0].clone());
            let final_input = if s_tokens > 0 {
                s_input
            } else {
                last_entry.tokens.as_ref().map(|t| t.input).unwrap_or(0)
            };
            let final_output = if s_tokens > 0 {
                s_output
            } else {
                last_entry.tokens.as_ref().map(|t| t.output).unwrap_or(0)
            };
            let final_cache = if s_tokens > 0 {
                s_cache
            } else {
                last_entry
                    .tokens
                    .as_ref()
                    .and_then(|t| t.cache_read)
                    .unwrap_or(0)
            };
            let final_reasoning = if s_tokens > 0 {
                s_reasoning
            } else {
                last_entry
                    .tokens
                    .as_ref()
                    .and_then(|t| t.reasoning)
                    .unwrap_or(0)
            };
            let final_total = if s_tokens > 0 {
                s_tokens
            } else {
                last_entry.tokens.as_ref().map(|t| t.total).unwrap_or(0)
            };

            let cost_usd = match calculate_entries_cost(
                &pricing_rules,
                s_entries,
                last_entry.model.as_deref(),
                final_input,
                final_output,
                final_cache,
            ) {
                Ok(v) => v,
                Err(err) => {
                    eprintln!("⚠️ 計算成本失敗: {}", err);
                    0.0
                }
            };

            day_tokens += final_total;
            day_input += final_input;
            day_output += final_output;
            day_cache_read += final_cache;
            day_reasoning += final_reasoning;
            day_cost_usd += cost_usd;
        }

        monthly_summary.total_tokens += day_tokens;
        monthly_summary.total_input_tokens += day_input;
        monthly_summary.total_output_tokens += day_output;
        monthly_summary.total_cache_read_tokens += day_cache_read;
        monthly_summary.total_reasoning_tokens += day_reasoning;
        monthly_summary.total_cost_usd += day_cost_usd;

        daily_breakdown.push(MonthlyDailyBreakdown {
            date: date_str,
            total_tokens: day_tokens,
            total_input_tokens: day_input,
            total_output_tokens: day_output,
            total_cache_read_tokens: day_cache_read,
            total_reasoning_tokens: day_reasoning,
            sessions_count: day_sessions.len(),
            cost_usd: day_cost_usd,
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

        let s_tokens = s_entries
            .iter()
            .map(|e| e.delta_tokens.as_ref().map(|t| t.total).unwrap_or(0))
            .sum::<u64>();
        let s_input = s_entries
            .iter()
            .map(|e| e.delta_tokens.as_ref().map(|t| t.input).unwrap_or(0))
            .sum::<u64>();
        let s_output = s_entries
            .iter()
            .map(|e| e.delta_tokens.as_ref().map(|t| t.output).unwrap_or(0))
            .sum::<u64>();
        let s_cache = s_entries
            .iter()
            .map(|e| {
                e.delta_tokens
                    .as_ref()
                    .and_then(|t| t.cache_read)
                    .unwrap_or(0)
            })
            .sum::<u64>();
        let s_reasoning = s_entries
            .iter()
            .map(|e| {
                e.delta_tokens
                    .as_ref()
                    .and_then(|t| t.reasoning)
                    .unwrap_or(0)
            })
            .sum::<u64>();

        let final_input = if s_tokens > 0 {
            s_input
        } else {
            last_entry.tokens.as_ref().map(|t| t.input).unwrap_or(0)
        };
        let final_output = if s_tokens > 0 {
            s_output
        } else {
            last_entry.tokens.as_ref().map(|t| t.output).unwrap_or(0)
        };
        let final_cache = if s_tokens > 0 {
            s_cache
        } else {
            last_entry
                .tokens
                .as_ref()
                .and_then(|t| t.cache_read)
                .unwrap_or(0)
        };
        let final_reasoning = if s_tokens > 0 {
            s_reasoning
        } else {
            last_entry
                .tokens
                .as_ref()
                .and_then(|t| t.reasoning)
                .unwrap_or(0)
        };
        let final_total = if s_tokens > 0 {
            s_tokens
        } else {
            last_entry.tokens.as_ref().map(|t| t.total).unwrap_or(0)
        };

        let cost_usd = match calculate_entries_cost(
            &pricing_rules,
            s_entries,
            last_entry.model.as_deref(),
            final_input,
            final_output,
            final_cache,
        ) {
            Ok(v) => v,
            Err(err) => {
                eprintln!("⚠️ 計算成本失敗: {}", err);
                0.0
            }
        };

        let cwd = last_entry.cwd.unwrap_or_else(|| "Unknown CWD".to_string());
        let project_stat = project_map_stats.entry(cwd).or_insert((0, 0, 0.0));
        project_stat.0 += 1;
        project_stat.1 += final_total;
        project_stat.2 += cost_usd;

        let model = last_entry
            .model
            .unwrap_or_else(|| "Unknown Model".to_string());
        let model_stat = model_map_stats.entry(model).or_insert((0, 0, 0, 0, 0, 0.0));
        model_stat.0 += 1;
        model_stat.1 += final_total;
        model_stat.2 += final_input;
        model_stat.3 += final_output;
        model_stat.4 += final_cache;
        model_stat.5 += cost_usd;

        let agent_stat = agent_map_stats.entry(ast_type.clone()).or_default();
        agent_stat.total_tokens += final_total;
        agent_stat.total_input_tokens += final_input;
        agent_stat.total_output_tokens += final_output;
        agent_stat.total_cache_read_tokens += final_cache;
        agent_stat.total_reasoning_tokens += final_reasoning;
        agent_stat.total_cost_usd += cost_usd;
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

    Json(MonthlyDetailsResponse {
        year_month,
        summary: monthly_summary,
        daily_breakdown,
        projects: project_summaries,
        models: model_summaries,
        agent_breakdown: agent_map_stats,
    })
    .into_response()
}
