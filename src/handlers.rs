use axum::{extract::Path, http::StatusCode, response::IntoResponse, Json};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{BufRead, BufReader},
    path::PathBuf,
};

use crate::db::{self, CostStats, TokenStats, UsageEntry};
use crate::pricing::{calculate_cost, load_pricing_rules, PricingEntry};
use crate::timeline::{
    parse_antigravity_timeline, parse_codex_timeline, parse_copilot_timeline, TimelineItem,
};

#[derive(Serialize)]
pub struct DateListResponse {
    pub dates: Vec<String>,
}

#[derive(Serialize)]
pub struct MonthListResponse {
    pub months: Vec<String>,
}

#[derive(Serialize)]
pub struct SetupInfoResponse {
    pub workspace_dir: String,
    pub home_dir: String,
    pub antigravity: AssistantSetupStatus,
    pub copilot: AssistantSetupStatus,
    pub codex: AssistantSetupStatus,
}

#[derive(Serialize)]
pub struct AssistantSetupStatus {
    pub dir_path: String,
    pub exists: bool,
    pub script_path: String,
}

#[derive(Serialize, Default, Clone)]
pub struct DaySummary {
    pub total_sessions: usize,
    pub total_tokens: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_read_tokens: u64,
    pub total_reasoning_tokens: u64,
    pub total_duration_ms: u64,
    pub total_requests: u64,
    pub total_cost_usd: f64,
}

#[derive(Serialize, Clone)]
pub struct SessionSummary {
    pub session_id: String,
    pub session_name: String,
    pub assistant_type: String,
    pub cwd: String,
    pub model: String,
    pub total_tokens: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_read_tokens: u64,
    pub total_reasoning_tokens: u64,
    pub max_turn_no: u32,
    pub timestamp: String,
    pub duration_ms: u64,
    pub cost_usd: f64,
    pub parent_session_id: Option<String>,
    pub agent_nickname: Option<String>,
    pub agent_role: Option<String>,
    pub reasoning_effort: Option<String>,
}

#[derive(Serialize)]
pub struct UsageDetailsResponse {
    pub date: String,
    pub summary: DaySummary,
    pub sessions: Vec<SessionSummary>,
    pub raw_entries: Vec<UsageEntry>,
}

#[derive(Serialize)]
pub struct MonthlyDailyBreakdown {
    pub date: String,
    pub total_tokens: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_read_tokens: u64,
    pub total_reasoning_tokens: u64,
    pub sessions_count: usize,
    pub cost_usd: f64,
}

#[derive(Serialize)]
pub struct MonthlyProjectSummary {
    pub cwd: String,
    pub sessions_count: usize,
    pub total_tokens: u64,
    pub cost_usd: f64,
}

#[derive(Serialize)]
pub struct MonthlyModelSummary {
    pub model: String,
    pub sessions_count: usize,
    pub total_tokens: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_read_tokens: u64,
    pub cost_usd: f64,
}

#[derive(Serialize, Default, Clone)]
pub struct AgentBreakdown {
    pub total_tokens: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_read_tokens: u64,
    pub total_reasoning_tokens: u64,
    pub total_cost_usd: f64,
    pub total_sessions: usize,
}

#[derive(Serialize)]
pub struct MonthlyDetailsResponse {
    pub year_month: String,
    pub summary: DaySummary,
    pub daily_breakdown: Vec<MonthlyDailyBreakdown>,
    pub projects: Vec<MonthlyProjectSummary>,
    pub models: Vec<MonthlyModelSummary>,
    pub agent_breakdown: HashMap<String, AgentBreakdown>,
}

#[derive(Serialize)]
pub struct YearlyMonthlyBreakdown {
    pub month: String,
    pub total_tokens: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_read_tokens: u64,
    pub total_reasoning_tokens: u64,
    pub sessions_count: usize,
    pub cost_usd: f64,
}

#[derive(Serialize)]
pub struct YearlyDetailsResponse {
    pub year: String,
    pub summary: DaySummary,
    pub monthly_breakdown: Vec<YearlyMonthlyBreakdown>,
    pub projects: Vec<MonthlyProjectSummary>,
    pub models: Vec<MonthlyModelSummary>,
    pub agent_breakdown: HashMap<String, AgentBreakdown>,
}

#[derive(Serialize)]
pub struct YearListResponse {
    pub years: Vec<String>,
}

/// API 1: 獲取可用的有使用記錄日期
pub async fn get_available_dates(Path(assistant): Path<String>) -> impl IntoResponse {
    let res: Result<Vec<String>, String> = tokio::task::spawn_blocking(move || {
        let conn = db::get_db_conn()?;
        let mut dates = Vec::new();
        if assistant == "all" {
            let mut stmt = conn.prepare("SELECT DISTINCT date FROM usage_entries ORDER BY date DESC").map_err(|e| e.to_string())?;
            let date_iter = stmt.query_map([], |row| row.get::<_, String>(0)).map_err(|e| e.to_string())?;
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
            let date_iter = stmt.query_map(rusqlite::params_from_iter(params_vec), |row| row.get::<_, String>(0)).map_err(|e| e.to_string())?;
            for d in date_iter {
                dates.push(d.map_err(|e| e.to_string())?);
            }
        }
        Ok(dates)
    }).await.unwrap_or_else(|_| Err("執行緒執行失敗".to_string()));

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
pub async fn get_setup_info(Path(_assistant): Path<String>) -> impl IntoResponse {
    let workspace_dir = match std::env::current_dir() {
        Ok(dir) => dir.to_string_lossy().into_owned(),
        Err(_) => "".to_string(),
    };
    let home_dir_path = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/home/user"));
    let home_dir = home_dir_path.to_string_lossy().into_owned();

    let anti_dir = db::get_antigravity_dir();
    let anti_script = anti_dir
        .join("statusline-token.sh")
        .to_string_lossy()
        .into_owned();
    let anti_exists = anti_dir.exists();

    let copilot_dir = db::get_copilot_dir();
    let copilot_script = copilot_dir
        .join("statusline-token.sh")
        .to_string_lossy()
        .into_owned();
    let copilot_exists = copilot_dir.exists();

    let codex_dir = db::get_codex_dir();
    let codex_exists = codex_dir.exists();

    Json(SetupInfoResponse {
        workspace_dir,
        home_dir,
        antigravity: AssistantSetupStatus {
            dir_path: anti_dir.to_string_lossy().into_owned(),
            exists: anti_exists,
            script_path: anti_script,
        },
        copilot: AssistantSetupStatus {
            dir_path: copilot_dir.to_string_lossy().into_owned(),
            exists: copilot_exists,
            script_path: copilot_script,
        },
        codex: AssistantSetupStatus {
            dir_path: codex_dir.to_string_lossy().into_owned(),
            exists: codex_exists,
            script_path: "".to_string(),
        },
    })
}

/// API 3: 獲取指定日期的 Token 使用詳情與會話列表
pub async fn get_usage_details(
    Path((assistant, date)): Path<(String, String)>,
) -> impl IntoResponse {
    let assistant_clone = assistant.clone();
    let date_clone = date.clone();

    let entries_res: Result<Vec<(UsageEntry, String)>, String> = tokio::task::spawn_blocking(move || {
        let conn = db::get_db_conn()?;
        let mut query = "SELECT 
                timestamp, session_id, session_name, transcript_path, cwd, version, turn_no, model, model_id,
                tokens_input, tokens_output, tokens_cache_read, tokens_reasoning, tokens_total,
                delta_input, delta_output, delta_cache_read, delta_reasoning, delta_total,
                duration_ms, premium_requests, parent_session_id, agent_nickname, agent_role, assistant_type, reasoning_effort
             FROM usage_entries WHERE date = ?".to_string();
        let mut params_vec = Vec::new();
        params_vec.push(rusqlite::types::Value::Text(date_clone));

        if assistant_clone != "all" {
            let assistants: Vec<&str> = assistant_clone.split(',').collect();
            let mut placeholders = Vec::new();
            for a in assistants {
                placeholders.push("?");
                params_vec.push(rusqlite::types::Value::Text(a.to_string()));
            }
            query.push_str(&format!(" AND assistant_type IN ({})", placeholders.join(",")));
        }
        query.push_str(" ORDER BY timestamp ASC");

        let mut stmt = conn.prepare(&query).map_err(|e| e.to_string())?;
        let mut rows = stmt.query(rusqlite::params_from_iter(params_vec)).map_err(|e| e.to_string())?;

        let mut entries = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let ast_type = row.get::<_, String>(24).map_err(|e| e.to_string())?;
            let tokens_input: Option<u64> = row.get::<_, Option<i64>>(9).map_err(|e| e.to_string())?.map(|v| v as u64);
            let tokens_output: Option<u64> = row.get::<_, Option<i64>>(10).map_err(|e| e.to_string())?.map(|v| v as u64);
            let tokens_cache_read: Option<u64> = row.get::<_, Option<i64>>(11).map_err(|e| e.to_string())?.map(|v| v as u64);
            let tokens_reasoning: Option<u64> = row.get::<_, Option<i64>>(12).map_err(|e| e.to_string())?.map(|v| v as u64);
            let tokens_total: Option<u64> = row.get::<_, Option<i64>>(13).map_err(|e| e.to_string())?.map(|v| v as u64);

            let tokens = if let (Some(input), Some(output), Some(total)) = (tokens_input, tokens_output, tokens_total) {
                Some(TokenStats { input, output, cache_read: tokens_cache_read, cache_write: None, reasoning: tokens_reasoning, total })
            } else {
                None
            };

            let delta_input: Option<u64> = row.get::<_, Option<i64>>(14).map_err(|e| e.to_string())?.map(|v| v as u64);
            let delta_output: Option<u64> = row.get::<_, Option<i64>>(15).map_err(|e| e.to_string())?.map(|v| v as u64);
            let delta_cache_read: Option<u64> = row.get::<_, Option<i64>>(16).map_err(|e| e.to_string())?.map(|v| v as u64);
            let delta_reasoning: Option<u64> = row.get::<_, Option<i64>>(17).map_err(|e| e.to_string())?.map(|v| v as u64);
            let delta_total: Option<u64> = row.get::<_, Option<i64>>(18).map_err(|e| e.to_string())?.map(|v| v as u64);

            let delta_tokens = if let (Some(input), Some(output), Some(total)) = (delta_input, delta_output, delta_total) {
                Some(TokenStats { input, output, cache_read: delta_cache_read, cache_write: None, reasoning: delta_reasoning, total })
            } else {
                None
            };

            let duration_ms: Option<f64> = row.get::<_, Option<i64>>(19).map_err(|e| e.to_string())?.map(|v| v as f64);
            let premium_requests: Option<f64> = row.get::<_, Option<i64>>(20).map_err(|e| e.to_string())?.map(|v| v as f64);

            let cost = if duration_ms.is_some() || premium_requests.is_some() {
                Some(CostStats { total_api_duration_ms: duration_ms, total_duration_ms: None, total_premium_requests: premium_requests })
            } else {
                None
            };

            entries.push((UsageEntry {
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
                parent_session_id: row.get(21).ok(),
                agent_nickname: row.get(22).ok(),
                agent_role: row.get(23).ok(),
                reasoning_effort: row.get(25).ok(),
            }, ast_type));
        }
        Ok(entries)
    }).await.unwrap_or_else(|_| Err("執行緒執行失敗".to_string()));

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

    for (e, ast_type) in &entries_with_type {
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
            summary.total_reasoning_tokens += tokens.reasoning.unwrap_or(0);
        } else if let Some(ref tokens) = e.tokens {
            if e.turn_no == 1 {
                summary.total_tokens += tokens.total;
                summary.total_input_tokens += tokens.input;
                summary.total_output_tokens += tokens.output;
                summary.total_cache_read_tokens += tokens.cache_read.unwrap_or(0);
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

        let cost_usd = calculate_cost(
            &pricing_rules,
            &last_entry
                .model
                .clone()
                .unwrap_or_else(|| "Unknown Model".to_string()),
            total_input_tokens,
            total_output_tokens,
            total_cache_read_tokens,
        );
        summary.total_cost_usd += cost_usd;

        sessions_summary.push(SessionSummary {
            session_id: session_id.clone(),
            session_name: last_entry
                .session_name
                .unwrap_or_else(|| "Start Coding Session".to_string()),
            assistant_type: ast_type.clone(),
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

/// API 4: 獲取特定會話的詳細對話歷史還原時間軸
fn get_git_info(cwd_str: &str) -> (Option<String>, Option<String>) {
    let path = std::path::Path::new(cwd_str);
    if !path.exists() {
        return (None, None);
    }

    let branch = std::process::Command::new("git")
        .args(&["symbolic-ref", "--short", "HEAD"])
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
        .args(&["config", "--get", "remote.origin.url"])
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
    // 1. 若 assistant == "all"，需要先在資料庫尋找此 session 屬於哪一個 assistant，並獲取 transcript 路徑
    let session_info: Result<(String, Option<String>), String> = tokio::task::spawn_blocking({
        let sid = session_id.clone();
        move || {
            let conn = db::get_db_conn()?;
            let mut stmt = conn.prepare("SELECT assistant_type, transcript_path FROM usage_entries WHERE session_id = ? LIMIT 1").map_err(|e| e.to_string())?;
            let mut rows = stmt.query(params![sid]).map_err(|e| e.to_string())?;
            if let Some(row) = rows.next().map_err(|e| e.to_string())? {
                let ast: String = row.get(0).map_err(|e| e.to_string())?;
                let path: Option<String> = row.get(1).ok();
                Ok((ast, path))
            } else {
                Err("Session not found".to_string())
            }
        }
    }).await.unwrap_or_else(|_| Err("執行緒執行失敗".to_string()));

    let (resolved_assistant, transcript_path_db) = match session_info {
        Ok(info) => info,
        Err(_) => (assistant.clone(), None), // 若查不到則採用路徑參數
    };

    // 2. 準備讀取檔案的完整路徑
    let filepath = match resolved_assistant.as_str() {
        "antigravity" => {
            let anti_dir = db::get_antigravity_dir();
            anti_dir
                .join("brain")
                .join(&session_id)
                .join(".system_generated/logs/transcript_full.jsonl")
        }
        "copilot" => {
            let cop_dir = db::get_copilot_dir();
            let mut path = cop_dir
                .join("session-state")
                .join(&session_id)
                .join("events.jsonl");
            if !path.exists() {
                path = cop_dir
                    .join("session-state")
                    .join(format!("{}.jsonl", session_id));
            }
            path
        }
        "codex" => {
            if let Some(ref p_str) = transcript_path_db {
                PathBuf::from(p_str)
            } else {
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({ "error": "找不到 Codex 會話日誌檔案路徑。" })),
                )
                    .into_response();
            }
        }
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "不支援的助理類型" })),
            )
                .into_response()
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

    let file = match File::open(&filepath) {
        Ok(f) => f,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("開啟日誌檔案失敗: {}", e) })),
            )
                .into_response()
        }
    };

    // 3. 預先載入 SQLite 中的回合 (turn_no) 增量 token 數據
    let sid_clone = session_id.clone();
    let (db_entries, session_cwd): (HashMap<u32, (TokenStats, String)>, Option<String>) = tokio::task::spawn_blocking(move || {
        let mut map = HashMap::new();
        let mut session_cwd = None;
        if let Ok(conn) = db::get_db_conn() {
            // 查詢會話的 cwd
            if let Ok(mut stmt) = conn.prepare(
                "SELECT cwd FROM usage_entries WHERE session_id = ? AND cwd IS NOT NULL LIMIT 1"
            ) {
                if let Ok(mut rows) = stmt.query(params![sid_clone]) {
                    if let Some(row) = rows.next().ok().flatten() {
                        session_cwd = row.get::<_, String>(0).ok();
                    }
                }
            }

            if let Ok(mut stmt) = conn.prepare(
                "SELECT turn_no, delta_input, delta_output, delta_cache_read, delta_reasoning, delta_total, model 
                 FROM usage_entries WHERE session_id = ? ORDER BY turn_no ASC"
            ) {
                if let Ok(mut rows) = stmt.query(params![sid_clone]) {
                    while let Ok(Some(row)) = rows.next() {
                        if let (Ok(turn_no), Ok(delta_input), Ok(delta_output), Ok(delta_total)) = (
                            row.get::<_, i64>(0),
                            row.get::<_, Option<i64>>(1),
                            row.get::<_, Option<i64>>(2),
                            row.get::<_, Option<i64>>(5)
                        ) {
                            if let (Some(input), Some(output), Some(total)) = (delta_input, delta_output, delta_total) {
                                let cache_read = row.get::<_, Option<i64>>(3).ok().flatten().map(|v| v as u64);
                                let reasoning = row.get::<_, Option<i64>>(4).ok().flatten().map(|v| v as u64);
                                let model = row.get::<_, Option<String>>(6).unwrap_or(None).unwrap_or_else(|| "Gemini".to_string());
                                map.insert(turn_no as u32, (TokenStats { input: input as u64, output: output as u64, cache_read, cache_write: None, reasoning, total: total as u64 }, model));
                            }
                        }
                    }
                }
            }
        }
        (map, session_cwd)
    }).await.unwrap_or_else(|_| (HashMap::new(), None));

    let reader = BufReader::new(file);
    let mut timeline = Vec::new();
    let mut metadata = HashMap::new();

    // 依據不同助理格式解析日誌
    match resolved_assistant.as_str() {
        "antigravity" => {
            parse_antigravity_timeline(reader, &db_entries, &mut timeline, &mut metadata);
        }
        "copilot" => {
            parse_copilot_timeline(reader, &db_entries, &mut timeline, &mut metadata);
        }
        "codex" => {
            parse_codex_timeline(reader, &db_entries, &mut timeline, &mut metadata);
        }
        _ => {}
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

    for (_, (stats, _)) in &db_entries {
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

/// API 5: 獲取可用的有使用記錄月份
pub async fn get_available_months(Path(assistant): Path<String>) -> impl IntoResponse {
    let res: Result<Vec<String>, String> = tokio::task::spawn_blocking(move || {
        let conn = db::get_db_conn()?;
        let mut months = Vec::new();
        if assistant == "all" {
            let mut stmt = conn.prepare("SELECT DISTINCT substr(date, 1, 7) FROM usage_entries ORDER BY date DESC").map_err(|e| e.to_string())?;
            let month_iter = stmt.query_map([], |row| row.get::<_, String>(0)).map_err(|e| e.to_string())?;
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
            let month_iter = stmt.query_map(rusqlite::params_from_iter(params_vec), |row| row.get::<_, String>(0)).map_err(|e| e.to_string())?;
            for m in month_iter {
                months.push(m.map_err(|e| e.to_string())?);
            }
        }
        Ok(months)
    }).await.unwrap_or_else(|_| Err("執行緒執行失敗".to_string()));

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
    let assistant_clone = assistant.clone();
    let query_month = format!("{}-%", year_month);

    let entries_res: Result<Vec<(UsageEntry, String, String)>, String> = tokio::task::spawn_blocking(move || {
        let conn = db::get_db_conn()?;
        let mut query = "SELECT 
                timestamp, session_id, session_name, transcript_path, cwd, version, turn_no, model, model_id,
                tokens_input, tokens_output, tokens_cache_read, tokens_reasoning, tokens_total,
                delta_input, delta_output, delta_cache_read, delta_reasoning, delta_total,
                duration_ms, premium_requests, parent_session_id, agent_nickname, agent_role, assistant_type, reasoning_effort,
                date
             FROM usage_entries WHERE date LIKE ?".to_string();
        let mut params_vec = Vec::new();
        params_vec.push(rusqlite::types::Value::Text(query_month));

        if assistant_clone != "all" {
            let assistants: Vec<&str> = assistant_clone.split(',').collect();
            let mut placeholders = Vec::new();
            for a in assistants {
                placeholders.push("?");
                params_vec.push(rusqlite::types::Value::Text(a.to_string()));
            }
            query.push_str(&format!(" AND assistant_type IN ({})", placeholders.join(",")));
        }
        query.push_str(" ORDER BY timestamp ASC");

        let mut stmt = conn.prepare(&query).map_err(|e| e.to_string())?;
        let mut rows = stmt.query(rusqlite::params_from_iter(params_vec)).map_err(|e| e.to_string())?;

        let mut entries = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let ast_type = row.get::<_, String>(24).map_err(|e| e.to_string())?;
            let tokens_input: Option<u64> = row.get::<_, Option<i64>>(9).map_err(|e| e.to_string())?.map(|v| v as u64);
            let tokens_output: Option<u64> = row.get::<_, Option<i64>>(10).map_err(|e| e.to_string())?.map(|v| v as u64);
            let tokens_cache_read: Option<u64> = row.get::<_, Option<i64>>(11).map_err(|e| e.to_string())?.map(|v| v as u64);
            let tokens_reasoning: Option<u64> = row.get::<_, Option<i64>>(12).map_err(|e| e.to_string())?.map(|v| v as u64);
            let tokens_total: Option<u64> = row.get::<_, Option<i64>>(13).map_err(|e| e.to_string())?.map(|v| v as u64);

            let tokens = if let (Some(input), Some(output), Some(total)) = (tokens_input, tokens_output, tokens_total) {
                Some(TokenStats { input, output, cache_read: tokens_cache_read, cache_write: None, reasoning: tokens_reasoning, total })
            } else {
                None
            };

            let delta_input: Option<u64> = row.get::<_, Option<i64>>(14).map_err(|e| e.to_string())?.map(|v| v as u64);
            let delta_output: Option<u64> = row.get::<_, Option<i64>>(15).map_err(|e| e.to_string())?.map(|v| v as u64);
            let delta_cache_read: Option<u64> = row.get::<_, Option<i64>>(16).map_err(|e| e.to_string())?.map(|v| v as u64);
            let delta_reasoning: Option<u64> = row.get::<_, Option<i64>>(17).map_err(|e| e.to_string())?.map(|v| v as u64);
            let delta_total: Option<u64> = row.get::<_, Option<i64>>(18).map_err(|e| e.to_string())?.map(|v| v as u64);

            let delta_tokens = if let (Some(input), Some(output), Some(total)) = (delta_input, delta_output, delta_total) {
                Some(TokenStats { input, output, cache_read: delta_cache_read, cache_write: None, reasoning: delta_reasoning, total })
            } else {
                None
            };

            let duration_ms: Option<f64> = row.get::<_, Option<i64>>(19).map_err(|e| e.to_string())?.map(|v| v as f64);
            let premium_requests: Option<f64> = row.get::<_, Option<i64>>(20).map_err(|e| e.to_string())?.map(|v| v as f64);

            let cost = if duration_ms.is_some() || premium_requests.is_some() {
                Some(CostStats { total_api_duration_ms: duration_ms, total_duration_ms: None, total_premium_requests: premium_requests })
            } else {
                None
            };

            let entry_date = row.get::<_, String>(26).map_err(|e| e.to_string())?;

            entries.push((UsageEntry {
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
                parent_session_id: row.get(21).ok(),
                agent_nickname: row.get(22).ok(),
                agent_role: row.get(23).ok(),
                reasoning_effort: row.get(25).ok(),
            }, ast_type, entry_date));
        }
        Ok(entries)
    }).await.unwrap_or_else(|_| Err("執行緒執行失敗".to_string()));

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
    let mut monthly_summary = DaySummary::default();
    monthly_summary.total_sessions = sessions_map.len();

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

            let cost_usd = calculate_cost(
                &pricing_rules,
                &last_entry
                    .model
                    .clone()
                    .unwrap_or_else(|| "Unknown Model".to_string()),
                final_input,
                final_output,
                final_cache,
            );

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

        let cost_usd = calculate_cost(
            &pricing_rules,
            &last_entry
                .model
                .clone()
                .unwrap_or_else(|| "Unknown Model".to_string()),
            final_input,
            final_output,
            final_cache,
        );

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
    project_summaries.sort_by(|a, b| b.total_tokens.cmp(&a.total_tokens));

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
    model_summaries.sort_by(|a, b| b.total_tokens.cmp(&a.total_tokens));

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

/// API 12: 獲取可用的有使用記錄年份
pub async fn get_available_years(Path(assistant): Path<String>) -> impl IntoResponse {
    let res: Result<Vec<String>, String> = tokio::task::spawn_blocking(move || {
        let conn = db::get_db_conn()?;
        let mut years = Vec::new();
        if assistant == "all" {
            let mut stmt = conn.prepare("SELECT DISTINCT substr(date, 1, 4) FROM usage_entries ORDER BY date DESC").map_err(|e| e.to_string())?;
            let year_iter = stmt.query_map([], |row| row.get::<_, String>(0)).map_err(|e| e.to_string())?;
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
            let year_iter = stmt.query_map(rusqlite::params_from_iter(params_vec), |row| row.get::<_, String>(0)).map_err(|e| e.to_string())?;
            for y in year_iter {
                years.push(y.map_err(|e| e.to_string())?);
            }
        }
        Ok(years)
    }).await.unwrap_or_else(|_| Err("執行緒執行失敗".to_string()));

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
    let assistant_clone = assistant.clone();
    let query_year = format!("{}-%", year);

    let entries_res: Result<Vec<(UsageEntry, String, String)>, String> = tokio::task::spawn_blocking(move || {
        let conn = db::get_db_conn()?;
        let mut query = "SELECT 
                timestamp, session_id, session_name, transcript_path, cwd, version, turn_no, model, model_id,
                tokens_input, tokens_output, tokens_cache_read, tokens_reasoning, tokens_total,
                delta_input, delta_output, delta_cache_read, delta_reasoning, delta_total,
                duration_ms, premium_requests, parent_session_id, agent_nickname, agent_role, assistant_type, reasoning_effort,
                date
             FROM usage_entries WHERE date LIKE ?".to_string();
        let mut params_vec = Vec::new();
        params_vec.push(rusqlite::types::Value::Text(query_year));

        if assistant_clone != "all" {
            let assistants: Vec<&str> = assistant_clone.split(',').collect();
            let mut placeholders = Vec::new();
            for a in assistants {
                placeholders.push("?");
                params_vec.push(rusqlite::types::Value::Text(a.to_string()));
            }
            query.push_str(&format!(" AND assistant_type IN ({})", placeholders.join(",")));
        }
        query.push_str(" ORDER BY timestamp ASC");

        let mut stmt = conn.prepare(&query).map_err(|e| e.to_string())?;
        let mut rows = stmt.query(rusqlite::params_from_iter(params_vec)).map_err(|e| e.to_string())?;

        let mut entries = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let ast_type = row.get::<_, String>(24).map_err(|e| e.to_string())?;
            let tokens_input: Option<u64> = row.get::<_, Option<i64>>(9).map_err(|e| e.to_string())?.map(|v| v as u64);
            let tokens_output: Option<u64> = row.get::<_, Option<i64>>(10).map_err(|e| e.to_string())?.map(|v| v as u64);
            let tokens_cache_read: Option<u64> = row.get::<_, Option<i64>>(11).map_err(|e| e.to_string())?.map(|v| v as u64);
            let tokens_reasoning: Option<u64> = row.get::<_, Option<i64>>(12).map_err(|e| e.to_string())?.map(|v| v as u64);
            let tokens_total: Option<u64> = row.get::<_, Option<i64>>(13).map_err(|e| e.to_string())?.map(|v| v as u64);

            let tokens = if let (Some(input), Some(output), Some(total)) = (tokens_input, tokens_output, tokens_total) {
                Some(TokenStats { input, output, cache_read: tokens_cache_read, cache_write: None, reasoning: tokens_reasoning, total })
            } else {
                None
            };

            let delta_input: Option<u64> = row.get::<_, Option<i64>>(14).map_err(|e| e.to_string())?.map(|v| v as u64);
            let delta_output: Option<u64> = row.get::<_, Option<i64>>(15).map_err(|e| e.to_string())?.map(|v| v as u64);
            let delta_cache_read: Option<u64> = row.get::<_, Option<i64>>(16).map_err(|e| e.to_string())?.map(|v| v as u64);
            let delta_reasoning: Option<u64> = row.get::<_, Option<i64>>(17).map_err(|e| e.to_string())?.map(|v| v as u64);
            let delta_total: Option<u64> = row.get::<_, Option<i64>>(18).map_err(|e| e.to_string())?.map(|v| v as u64);

            let delta_tokens = if let (Some(input), Some(output), Some(total)) = (delta_input, delta_output, delta_total) {
                Some(TokenStats { input, output, cache_read: delta_cache_read, cache_write: None, reasoning: delta_reasoning, total })
            } else {
                None
            };

            let duration_ms: Option<f64> = row.get::<_, Option<i64>>(19).map_err(|e| e.to_string())?.map(|v| v as f64);
            let premium_requests: Option<f64> = row.get::<_, Option<i64>>(20).map_err(|e| e.to_string())?.map(|v| v as f64);

            let cost = if duration_ms.is_some() || premium_requests.is_some() {
                Some(CostStats { total_api_duration_ms: duration_ms, total_duration_ms: None, total_premium_requests: premium_requests })
            } else {
                None
            };

            let entry_date = row.get::<_, String>(26).map_err(|e| e.to_string())?;

            entries.push((UsageEntry {
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
                parent_session_id: row.get(21).ok(),
                agent_nickname: row.get(22).ok(),
                agent_role: row.get(23).ok(),
                reasoning_effort: row.get(25).ok(),
            }, ast_type, entry_date));
        }
        Ok(entries)
    }).await.unwrap_or_else(|_| Err("執行緒執行失敗".to_string()));

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
    let mut yearly_summary = DaySummary::default();
    yearly_summary.total_sessions = sessions_map.len();

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

        for (sid, s_entries) in &month_sessions_map {
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

            let cost_usd = calculate_cost(
                &pricing_rules,
                &last_entry
                    .model
                    .clone()
                    .unwrap_or_else(|| "Unknown Model".to_string()),
                final_input,
                final_output,
                final_cache,
            );

            m_tokens += final_total;
            m_input += final_input;
            m_output += final_output;
            m_cache_read += final_cache;
            m_reasoning += final_reasoning;
            m_cost_usd += cost_usd;
        }

        yearly_summary.total_tokens += m_tokens;
        yearly_summary.total_input_tokens += m_input;
        yearly_summary.total_output_tokens += m_output;
        yearly_summary.total_cache_read_tokens += m_cache_read;
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

        let cost_usd = calculate_cost(
            &pricing_rules,
            &last_entry
                .model
                .clone()
                .unwrap_or_else(|| "Unknown Model".to_string()),
            final_input,
            final_output,
            final_cache,
        );

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
    project_summaries.sort_by(|a, b| b.total_tokens.cmp(&a.total_tokens));

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
    model_summaries.sort_by(|a, b| b.total_tokens.cmp(&a.total_tokens));

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

/// API 7: 獲取模型價格清單 ( pricing.csv 資訊)
pub async fn get_pricing(Path(_assistant): Path<String>) -> impl IntoResponse {
    let mut entries = Vec::new();
    let file_path = PathBuf::from("pricing.csv");
    if let Ok(file) = File::open(&file_path) {
        let reader = BufReader::new(file);
        let mut lines = reader.lines();
        if let Some(Ok(_header)) = lines.next() {
            for line in lines.flatten() {
                let parts: Vec<&str> = line.split(',').collect();
                if parts.len() >= 6 {
                    let input_price = parts[3].trim().parse::<f64>().unwrap_or(0.0);
                    let cache_input_price = parts[4].trim().parse::<f64>().unwrap_or(0.0);
                    let output_price = parts[5].trim().parse::<f64>().unwrap_or(0.0);
                    let batch_api_price = if parts.len() >= 7 {
                        parts[6].trim().to_string()
                    } else {
                        "N/A".to_string()
                    };
                    entries.push(PricingEntry {
                        model_name: parts[0].trim().to_string(),
                        deployment_type: parts[1].trim().to_string(),
                        unit: parts[2].trim().to_string(),
                        input_price,
                        cache_input_price,
                        output_price,
                        batch_api_price,
                    });
                }
            }
        }
    }
    if entries.is_empty() {
        entries = vec![
            PricingEntry {
                model_name: "Gemini 3.5 Flash".to_string(),
                deployment_type: "Google AI".to_string(),
                unit: "1M Tokens".to_string(),
                input_price: 1.50,
                cache_input_price: 0.375,
                output_price: 9.00,
                batch_api_price: "0.75/0.1875/4.50".to_string(),
            },
            PricingEntry {
                model_name: "Gemini 1.5 Flash".to_string(),
                deployment_type: "Google AI".to_string(),
                unit: "1M Tokens".to_string(),
                input_price: 0.075,
                cache_input_price: 0.01875,
                output_price: 0.30,
                batch_api_price: "0.0375/0.009375/0.15".to_string(),
            },
            PricingEntry {
                model_name: "Gemini 1.5 Pro".to_string(),
                deployment_type: "Google AI".to_string(),
                unit: "1M Tokens".to_string(),
                input_price: 1.25,
                cache_input_price: 0.3125,
                output_price: 5.00,
                batch_api_price: "0.625/0.15625/2.50".to_string(),
            },
            PricingEntry {
                model_name: "Gemini 2.0 Flash".to_string(),
                deployment_type: "Google AI".to_string(),
                unit: "1M Tokens".to_string(),
                input_price: 0.10,
                cache_input_price: 0.025,
                output_price: 0.40,
                batch_api_price: "0.05/0.0125/0.20".to_string(),
            },
        ];
    }
    Json(entries)
}

/// API 8: 手動觸發日誌增量同步
pub async fn trigger_manual_sync(Path(_assistant): Path<String>) -> impl IntoResponse {
    let sync_res = tokio::task::spawn_blocking(|| {
        if let Ok(mut conn) = db::get_db_conn() {
            db::sync_usage_logs(&mut conn)
        } else {
            Err("無法連接至 SQLite 資料庫".to_string())
        }
    })
    .await;

    match sync_res {
        Ok(Ok(_)) => (
            StatusCode::OK,
            Json(serde_json::json!({ "status": "success", "message": "手動增量同步已成功完成！" })),
        )
            .into_response(),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "status": "error", "message": format!("同步失敗: {}", e) })),
        )
            .into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "status": "error", "message": "執行緒執行失敗" })),
        )
            .into_response(),
    }
}

/// API: 獲取 Codex 的 rate limit 資料
pub async fn get_rate_limit(Path(assistant): Path<String>) -> impl IntoResponse {
    if assistant != "codex" {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Only codex is supported" })),
        )
            .into_response();
    }

    let res = tokio::task::spawn_blocking(move || db::get_latest_codex_rate_limit())
        .await
        .unwrap();

    match res {
        Some(val) => (StatusCode::OK, Json(val)).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "No rate limit data found" })),
        )
            .into_response(),
    }
}

#[derive(Serialize)]
pub struct CodexAuthInfo {
    pub name: String,
    pub active: bool,
}

#[derive(Serialize)]
pub struct CodexAuthListResponse {
    pub configs: Vec<CodexAuthInfo>,
    pub current_active: Option<String>,
}

#[derive(Deserialize)]
pub struct SwitchAuthRequest {
    pub name: String,
}

/// API: 獲取 Codex 的 auth 憑證列表
pub async fn get_codex_auth_configs(Path(assistant): Path<String>) -> impl IntoResponse {
    if assistant != "codex" {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Only codex is supported" })),
        )
            .into_response();
    }

    let res = tokio::task::spawn_blocking(move || {
        let codex_dir = db::get_codex_dir();
        let auth_dir = codex_dir.join("auth");
        let active_auth_file = codex_dir.join("auth.json");

        if !auth_dir.exists() {
            let _ = std::fs::create_dir_all(&auth_dir);
        }

        // Read active auth.json contents to compare
        let active_bytes = std::fs::read(&active_auth_file).ok();

        let mut configs = Vec::new();
        let mut current_active = None;

        if let Ok(entries) = std::fs::read_dir(&auth_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Some(filename) = path.file_name().and_then(|f| f.to_str()) {
                        if filename.ends_with(".json") {
                            let mut active = false;
                            if let Some(ref active_content) = active_bytes {
                                if let Ok(content) = std::fs::read(&path) {
                                    if content == *active_content {
                                        active = true;
                                        current_active = Some(filename.to_string());
                                    }
                                }
                            }
                            configs.push(CodexAuthInfo {
                                name: filename.to_string(),
                                active,
                            });
                        }
                    }
                }
            }
        }

        // Sort configs alphabetically by name
        configs.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(CodexAuthListResponse {
            configs,
            current_active,
        })
    })
    .await
    .unwrap_or_else(|_| Err("Thread execution failed".to_string()));

    match res {
        Ok(data) => (StatusCode::OK, Json(data)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        )
            .into_response(),
    }
}

/// API: 切換 Codex 的 auth 憑證
pub async fn switch_codex_auth(
    Path(assistant): Path<String>,
    Json(payload): Json<SwitchAuthRequest>,
) -> impl IntoResponse {
    if assistant != "codex" {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Only codex is supported" })),
        )
            .into_response();
    }

    // Safety check: make sure filename doesn't contain path traversal
    let filename = payload.name.clone();
    if filename.contains('/') || filename.contains('\\') || filename.contains("..") {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid filename" })),
        )
            .into_response();
    }

    let res = tokio::task::spawn_blocking(move || {
        let codex_dir = db::get_codex_dir();
        let auth_dir = codex_dir.join("auth");
        let source_file = auth_dir.join(&filename);
        let dest_file = codex_dir.join("auth.json");

        if !source_file.exists() {
            return Err(format!("Auth file {} does not exist", filename));
        }

        std::fs::copy(&source_file, &dest_file)
            .map_err(|e| format!("Failed to copy file: {}", e))?;

        Ok(())
    })
    .await
    .unwrap_or_else(|_| Err("Thread execution failed".to_string()));

    match res {
        Ok(_) => (StatusCode::OK, Json(serde_json::json!({ "status": "success", "message": format!("Successfully switched to {}", payload.name) }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "status": "error", "message": e }))).into_response(),
    }
}

fn find_npx_path() -> PathBuf {
    // 1. Check in PATH env var
    if let Ok(path_env) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_env) {
            let npx_in_path = dir.join("npx");
            if npx_in_path.exists() && npx_in_path.is_file() {
                return npx_in_path;
            }
        }
    }

    // 2. Try looking in NVM directory under home_dir
    if let Some(home) = dirs::home_dir() {
        let nvm_dir = home.join(".nvm/versions/node");
        if nvm_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(nvm_dir) {
                let mut versions = Vec::new();
                for entry in entries.flatten() {
                    if entry.path().is_dir() {
                        versions.push(entry.path());
                    }
                }
                // Sort versions to pick the latest Node version
                versions.sort();
                if let Some(newest_node) = versions.last() {
                    let npx_fallback = newest_node.join("bin/npx");
                    if npx_fallback.exists() {
                        return npx_fallback;
                    }
                }
            }
        }
    }

    // 3. Fallback to just "npx"
    PathBuf::from("npx")
}

/// API: 獲取 Codex 的重置額度資訊
pub async fn get_codex_reset_info(Path(assistant): Path<String>) -> impl IntoResponse {
    if assistant != "codex" {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Only codex is supported" })),
        )
            .into_response();
    }

    let res = tokio::task::spawn_blocking(move || {
        let codex_dir = db::get_codex_dir();
        let active_auth_file = codex_dir.join("auth.json");

        let npx_path = find_npx_path();
        let mut cmd = std::process::Command::new(npx_path);
        cmd.args(["-y", "@willh/codex-reset-checker", "--json"]);
        if active_auth_file.exists() {
            cmd.arg("--auth").arg(&active_auth_file);
        }

        let output = cmd.output();
        match output {
            Ok(out) => {
                if out.status.success() {
                    let stdout_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(&stdout_str) {
                        Ok(json_val)
                    } else {
                        Err(format!("Failed to parse output as JSON: {}", stdout_str))
                    }
                } else {
                    let stderr_str = String::from_utf8_lossy(&out.stderr).trim().to_string();
                    Err(format!(
                        "Command exited with status code {}: {}",
                        out.status, stderr_str
                    ))
                }
            }
            Err(e) => Err(format!("Failed to execute command: {}", e)),
        }
    })
    .await
    .unwrap_or_else(|_| Err("Thread execution failed".to_string()));

    match res {
        Ok(data) => (StatusCode::OK, Json(data)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use std::env;
    use std::fs;

    #[tokio::test]
    async fn test_yearly_handlers() {
        let temp_dir = std::path::PathBuf::from("temp_test_insights");
        if temp_dir.exists() {
            let _ = fs::remove_dir_all(&temp_dir);
        }
        fs::create_dir_all(&temp_dir).unwrap();
        env::set_var("INSIGHTS_DIR", temp_dir.to_str().unwrap());

        // Initialize SQLite DB
        let conn = db::get_db_conn().unwrap();
        db::init_db(&conn).unwrap();

        // Insert some fake entries
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, timestamp, date, session_id, session_name, cwd, turn_no, model,
                tokens_input, tokens_output, tokens_cache_read, tokens_total,
                delta_input, delta_output, delta_cache_read, delta_total
            ) VALUES (
                'antigravity', '2026-07-01 12:00:00', '2026-07-01', 'session_1', 'Session 1', '/cwd/1', 1, 'Gemini 3.5 Flash',
                100, 50, 20, 150,
                100, 50, 20, 150
            )",
            [],
        ).unwrap();

        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, timestamp, date, session_id, session_name, cwd, turn_no, model,
                tokens_input, tokens_output, tokens_cache_read, tokens_total,
                delta_input, delta_output, delta_cache_read, delta_total
            ) VALUES (
                'antigravity', '2026-07-01 12:05:00', '2026-07-01', 'session_1', 'Session 1', '/cwd/1', 2, 'Gemini 3.5 Flash',
                120, 60, 20, 180,
                20, 10, 0, 30
            )",
            [],
        ).unwrap();

        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, timestamp, date, session_id, session_name, cwd, turn_no, model,
                tokens_input, tokens_output, tokens_cache_read, tokens_total,
                delta_input, delta_output, delta_cache_read, delta_total
            ) VALUES (
                'antigravity', '2025-06-01 12:00:00', '2025-06-01', 'session_2', 'Session 2', '/cwd/2', 1, 'Gemini 3.5 Flash',
                200, 100, 40, 300,
                200, 100, 40, 300
            )",
            [],
        ).unwrap();

        // 1. Test get_available_years
        let conn = db::get_db_conn().unwrap();
        let mut stmt = conn
            .prepare("SELECT DISTINCT substr(date, 1, 4) FROM usage_entries ORDER BY date DESC")
            .unwrap();
        let mut rows = stmt.query([]).unwrap();
        let mut years = Vec::new();
        while let Some(row) = rows.next().unwrap() {
            years.push(row.get::<_, String>(0).unwrap());
        }
        assert_eq!(years, vec!["2026", "2025"]);

        // Cleanup
        let _ = fs::remove_dir_all(&temp_dir);
    }
}
