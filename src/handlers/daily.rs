use axum::{extract::Path, http::StatusCode, response::IntoResponse, Json};
use std::{
    collections::HashMap,
    fs::File,
    io::BufReader,
    path::PathBuf,
};

use super::*;
use crate::db::{self, TokenStats};
use crate::pricing::{calculate_cost, load_pricing_rules};
use crate::timeline::{
    parse_antigravity_timeline, parse_codex_timeline, parse_copilot_timeline, TimelineItem,
};

pub async fn get_available_dates(Path(assistant): Path<String>) -> impl IntoResponse {
    let res: Result<Vec<String>, String> = tokio::task::spawn_blocking(move || {
        let conn = db::get_db_conn()?;
        db::get_available_dates(&conn, &assistant)
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
        db::get_usage_entries_by_date(&conn, &date_clone, &assistant_clone)
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
    let session_info: Result<(String, Option<String>), String> = tokio::task::spawn_blocking({
        let sid = session_id.clone();
        move || {
            let conn = db::get_db_conn()?;
            db::get_session_assistant_and_transcript(&conn, &sid)
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
        if let Ok(conn) = db::get_db_conn() {
            let session_cwd = db::get_session_cwd(&conn, &sid_clone).unwrap_or(None);
            let map = db::get_session_turns_token_stats(&conn, &sid_clone).unwrap_or_default();
            (map, session_cwd)
        } else {
            (HashMap::new(), None)
        }
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
