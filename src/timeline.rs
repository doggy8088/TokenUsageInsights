use crate::db::{parse_cursor_timestamp, TokenStats};
use crate::grok::{display_model_name, timestamp_to_rfc3339, value_to_text, UNKNOWN_MODEL};
use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// Timeline Item definition for Session Reconstruction
#[derive(Serialize)]
#[serde(tag = "type")]
pub enum TimelineItem {
    UserPrompt {
        timestamp: String,
        prompt: String,
        context: Option<serde_json::Value>,
        turn_no: u32,
    },
    AgentReply {
        timestamp: String,
        reply: String,
        reasoning: Option<String>,
        turn_no: u32,
        model: String,
        tokens: Option<TokenStats>,
        duration_ms: Option<u64>,
        reasoning_effort: Option<String>,
    },
    ToolStep {
        timestamp: String,
        tool_name: String,
        arguments: serde_json::Value,
        env: Option<serde_json::Value>,
        exit_code: Option<i32>,
        stdout: String,
        stderr: String,
        tool_call_id: Option<String>,
        status: String, // 'running', 'success', 'failed'
    },
    SystemStatus {
        timestamp: String,
        status_type: String, // 'session_start', 'session_end', 'compaction', etc.
        message: String,
    },
}

pub fn parse_antigravity_timeline(
    reader: BufReader<File>,
    db_entries: &HashMap<u32, (TokenStats, String)>,
    timeline: &mut Vec<TimelineItem>,
    metadata: &mut HashMap<String, serde_json::Value>,
) {
    let mut turn_no = 1;
    let mut current_model = "Gemini".to_string();
    let mut pending_tool_indices: Vec<usize> = Vec::new();

    for line_res in reader.lines() {
        let line = match line_res {
            Ok(l) => l,
            Err(_) => continue,
        };
        let step: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let step_type = step.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let timestamp = step
            .get("created_at")
            .or_else(|| step.get("timestamp"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();

        match step_type {
            "USER_INPUT" => {
                let content = step
                    .get("content")
                    .and_then(|c| c.as_str())
                    .unwrap_or("")
                    .to_string();
                let context = step.get("context").cloned();
                timeline.push(TimelineItem::UserPrompt {
                    timestamp,
                    prompt: content,
                    context,
                    turn_no,
                });
            }
            "PLANNER_RESPONSE" => {
                let content = step
                    .get("content")
                    .and_then(|c| c.as_str())
                    .unwrap_or("")
                    .to_string();
                let reasoning = step
                    .get("reasoning")
                    .and_then(|r| r.as_str())
                    .map(|s| s.to_string());

                if !content.is_empty() {
                    // 讀取該回合在 SQLite 中的增量 token
                    let (tokens, model_name) =
                        if let Some((stats, model)) = db_entries.get(&turn_no) {
                            current_model = model.clone();
                            (Some(stats.clone()), current_model.clone())
                        } else {
                            (None, current_model.clone())
                        };

                    timeline.push(TimelineItem::AgentReply {
                        timestamp: timestamp.clone(),
                        reply: content,
                        reasoning,
                        turn_no,
                        model: model_name,
                        tokens,
                        duration_ms: None,
                        reasoning_effort: None,
                    });
                    turn_no += 1;
                }

                if let Some(tool_calls) = step.get("tool_calls").and_then(|t| t.as_array()) {
                    for tool_call in tool_calls {
                        let name = tool_call
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("")
                            .to_string();
                        let args = tool_call
                            .get("args")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null);

                        let idx = timeline.len();
                        pending_tool_indices.push(idx);

                        timeline.push(TimelineItem::ToolStep {
                            timestamp: timestamp.clone(),
                            tool_name: name,
                            arguments: args,
                            env: None,
                            exit_code: None,
                            stdout: "".to_string(),
                            stderr: "".to_string(),
                            tool_call_id: None,
                            status: "running".to_string(),
                        });
                    }
                }
            }
            "RUN_COMMAND" | "GREP_SEARCH" | "LIST_DIRECTORY" | "VIEW_FILE" | "CODE_ACTION"
            | "GENERIC" | "ERROR_MESSAGE" => {
                let content = step
                    .get("content")
                    .and_then(|c| c.as_str())
                    .unwrap_or("")
                    .to_string();
                let error = step.get("error").and_then(|e| e.as_str()).unwrap_or("");
                let exit_code = if step_type == "ERROR_MESSAGE" || !error.is_empty() {
                    Some(1)
                } else {
                    Some(0)
                };

                if !pending_tool_indices.is_empty() {
                    let idx = pending_tool_indices.remove(0);
                    if let Some(TimelineItem::ToolStep {
                        stdout,
                        stderr,
                        exit_code: target_exit_code,
                        status,
                        ..
                    }) = timeline.get_mut(idx)
                    {
                        *stdout = content;
                        *stderr = error.to_string();
                        *target_exit_code = exit_code;
                        *status = if exit_code.unwrap_or(0) == 0 {
                            "success".to_string()
                        } else {
                            "failed".to_string()
                        };
                    }
                } else {
                    timeline.push(TimelineItem::ToolStep {
                        timestamp,
                        tool_name: step_type.to_lowercase(),
                        arguments: serde_json::Value::Null,
                        env: None,
                        exit_code,
                        stdout: content,
                        stderr: error.to_string(),
                        tool_call_id: None,
                        status: if exit_code.unwrap_or(0) == 0 {
                            "success".to_string()
                        } else {
                            "failed".to_string()
                        },
                    });
                }
            }
            "TOOL_CALL" => {
                let name = step
                    .get("tool_name")
                    .and_then(|t| t.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let args = step
                    .get("arguments")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                let stdout = step
                    .get("stdout")
                    .and_then(|o| o.as_str())
                    .unwrap_or("")
                    .to_string();
                let stderr = step
                    .get("stderr")
                    .and_then(|e| e.as_str())
                    .unwrap_or("")
                    .to_string();
                let exit_code = step
                    .get("exit_code")
                    .and_then(|ec| ec.as_i64())
                    .map(|v| v as i32);
                let env = step.get("env").cloned();

                timeline.push(TimelineItem::ToolStep {
                    timestamp,
                    tool_name: name,
                    arguments: args,
                    env,
                    exit_code,
                    stdout,
                    stderr,
                    tool_call_id: None,
                    status: if exit_code.unwrap_or(0) == 0 {
                        "success".to_string()
                    } else {
                        "failed".to_string()
                    },
                });
            }
            "CHECKPOINT" => {
                timeline.push(TimelineItem::SystemStatus {
                    timestamp,
                    status_type: "session_compaction".to_string(),
                    message: "會話截斷壓縮 (Conversation Truncated/Compacted)".to_string(),
                });
            }
            _ => {}
        }
    }
    metadata.insert(
        "selected_model".to_string(),
        serde_json::Value::String(current_model),
    );
}

/// Backwards-compatible entry point that reconstructs a Copilot CLI timeline
/// without any agent filtering. Kept as a thin shim so existing callers and
/// documentation continue to resolve; the Copilot CLI events never carry a
/// top-level `agentId`, so forwarding `None` reproduces the original behavior.
#[allow(dead_code)]
pub fn parse_copilot_timeline(
    reader: BufReader<File>,
    db_entries: &HashMap<u32, (TokenStats, String)>,
    timeline: &mut Vec<TimelineItem>,
    metadata: &mut HashMap<String, serde_json::Value>,
) {
    parse_copilot_timeline_filtered(reader, db_entries, timeline, metadata, None, None);
}

/// Copilot App-aware variant of [`parse_copilot_timeline`].
///
/// `agent_filter` selects which agent's events are reconstructed from the
/// shared `events.jsonl` of a Copilot App session:
/// - `None`: main agent view. Events that carry a non-null top-level `agentId`
///   (subagent `assistant.message`, subagent `tool.execution_*`, `hook.*`,
///   `subagent.*`, `session.error` ...) are skipped so the main agent timeline
///   never lists a subagent's reply or tool execution as the main agent's own.
/// - `Some(agent_id)`: subagent view. Only events whose top-level `agentId`
///   equals `agent_id` are reconstructed, plus shared context events
///   (`session.start`, `session.shutdown`, `session.info`, `user.message`,
///   `system.message`) that have no `agentId` so the user prompt and session
///   metadata remain visible.
///
/// Copilot CLI calls [`parse_copilot_timeline`] which forwards `None`, so its
/// behavior is unchanged (CLI events never carry a top-level `agentId`).
///
/// `db_session_model` is the canonical model for the session row being
/// reconstructed, sourced from the database by the caller. For a subagent
/// synthetic session (`<main>__<agent_id>`) this is the child session's own
/// model (written by the collector), NOT the parent's. It seeds
/// `current_model` so the subagent drawer's `metadata.selected_model` and
/// `AgentReply.model` reflect the child model even when the shared
/// `events.jsonl` only carries the parent's `session.start.selectedModel`.
/// For main sessions it is `None` and the parser falls back to
/// `session.start.selectedModel` as before. The shared `session.start` event
/// is intentionally NOT allowed to override a non-`None` `db_session_model`,
/// because that event belongs to the parent context.
pub fn parse_copilot_timeline_filtered(
    reader: BufReader<File>,
    db_entries: &HashMap<u32, (TokenStats, String)>,
    timeline: &mut Vec<TimelineItem>,
    metadata: &mut HashMap<String, serde_json::Value>,
    agent_filter: Option<&str>,
    db_session_model: Option<&str>,
) {
    let mut current_turn_no = 1;
    let mut has_seen_user_prompt = false;
    // Seed the model from the DB child session model when available so a
    // subagent drawer starts with its own model and is never overwritten by
    // the shared parent `session.start.selectedModel`. Main sessions pass
    // `None` and keep the original parser default.
    let mut current_model = db_session_model
        .filter(|m| !m.is_empty())
        .map(|m| m.to_string())
        .unwrap_or_else(|| "GPT-4o".to_string());
    let mut tool_calls_map = HashMap::new();

    for line_res in reader.lines() {
        let line = match line_res {
            Ok(l) => l,
            Err(_) => continue,
        };
        let event: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let event_type = event.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let event_agent_id = event.get("agentId").and_then(|v| v.as_str());

        // Apply the agent filter before any state mutation so per-agent
        // `tool_calls_map` and `current_turn_no` bookkeeping stay consistent
        // with the filtered event stream.
        let keep = match agent_filter {
            None => event_agent_id.is_none(),
            Some(filter) => match event_agent_id {
                Some(a) => a == filter,
                // Shared context events (no agentId) are useful for both views;
                // skip subagent lifecycle events themselves from the shared
                // set because they are tagged with their own agentId above.
                None => matches!(
                    event_type,
                    "session_meta"
                        | "SESSION_STARTED"
                        | "session.start"
                        | "session.shutdown"
                        | "session.info"
                        | "user.message"
                        | "USER_PROMPT"
                        | "system.message"
                ),
            },
        };
        if !keep {
            continue;
        }

        let timestamp = event
            .get("timestamp")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        let payload = event.get("payload");
        let data = event.get("data");

        match event_type {
            // 舊格式
            "session_meta" | "SESSION_STARTED" => {
                let p = payload.or(data);
                if let Some(p) = p {
                    if let Some(v) = p.get("cli_version") {
                        metadata.insert("copilot_version".to_string(), v.clone());
                    }
                    if let Some(cwd) = p.get("cwd") {
                        metadata.insert("cwd".to_string(), cwd.clone());
                    }
                    if let Some(git) = p.get("git") {
                        if let Some(branch) = git.get("branch") {
                            metadata.insert("git_branch".to_string(), branch.clone());
                        }
                        if let Some(repo) = git.get("repository_url") {
                            metadata.insert("repository".to_string(), repo.clone());
                        }
                    }
                }
                timeline.push(TimelineItem::SystemStatus {
                    timestamp,
                    status_type: "session_start".to_string(),
                    message: "會話開始 (Session Started)".to_string(),
                });
            }
            // 新格式: session.start
            "session.start" => {
                if let Some(p) = data.or(payload) {
                    if let Some(v) = p.get("copilotVersion") {
                        metadata.insert("copilot_version".to_string(), v.clone());
                    }
                    if let Some(ctx) = p.get("context") {
                        if let Some(cwd) = ctx.get("cwd") {
                            metadata.insert("cwd".to_string(), cwd.clone());
                        }
                        if let Some(branch) = ctx.get("branch") {
                            metadata.insert("git_branch".to_string(), branch.clone());
                        }
                        if let Some(repo) = ctx.get("repository") {
                            metadata.insert("repository".to_string(), repo.clone());
                        }
                    }
                    if let Some(model) = p.get("selectedModel").and_then(|m| m.as_str()) {
                        // Only let the shared parent `session.start` event seed
                        // the model when we do not already have a DB-sourced
                        // child session model. Otherwise the parent's
                        // `selectedModel` would clobber the subagent drawer's
                        // canonical child model (see `db_session_model`).
                        if db_session_model.is_none() && model != "auto" {
                            current_model = model.to_string();
                        }
                    }
                }
                timeline.push(TimelineItem::SystemStatus {
                    timestamp,
                    status_type: "session_start".to_string(),
                    message: "會話開始 (Session Started)".to_string(),
                });
            }
            "user.message" | "USER_PROMPT" => {
                let p = payload.or(data);
                if let Some(p) = p {
                    let content = p
                        .get("content")
                        .and_then(|c| c.as_str())
                        .unwrap_or("")
                        .to_string();
                    let context = p.get("context").cloned();
                    timeline.push(TimelineItem::UserPrompt {
                        timestamp,
                        prompt: content,
                        context,
                        turn_no: current_turn_no,
                    });
                    has_seen_user_prompt = true;
                }
            }
            "assistant.message" | "ASSISTANT_REPLY" => {
                let p = payload.or(data);
                if let Some(p) = p {
                    let content = p
                        .get("content")
                        .and_then(|c| c.as_str())
                        .unwrap_or("")
                        .to_string();
                    let reasoning = p
                        .get("reasoning")
                        .and_then(|r| r.as_str())
                        .map(|s| s.to_string());

                    if let Some(model) = p.get("model").and_then(|m| m.as_str()) {
                        current_model = model.to_string();
                    }

                    // 新格式: toolRequests 陣列，直接推入 ToolStep（由後續 tool.execution_complete 補結果）
                    if let Some(tool_requests) = p.get("toolRequests").and_then(|tr| tr.as_array())
                    {
                        for req in tool_requests {
                            let call_id = req
                                .get("toolCallId")
                                .and_then(|i| i.as_str())
                                .unwrap_or("")
                                .to_string();
                            let name = req
                                .get("name")
                                .and_then(|n| n.as_str())
                                .unwrap_or("")
                                .to_string();
                            let args = req
                                .get("arguments")
                                .cloned()
                                .unwrap_or(serde_json::Value::Null);
                            let idx = timeline.len();
                            tool_calls_map.insert(call_id.clone(), idx);
                            timeline.push(TimelineItem::ToolStep {
                                timestamp: timestamp.clone(),
                                tool_name: name,
                                arguments: args,
                                env: None,
                                exit_code: None,
                                stdout: "".to_string(),
                                stderr: "".to_string(),
                                tool_call_id: Some(call_id),
                                status: "running".to_string(),
                            });
                        }
                    }

                    // 有實質回覆內容才推入 AgentReply
                    if !content.is_empty() {
                        let (tokens, model_name) =
                            if let Some((stats, model)) = db_entries.get(&current_turn_no) {
                                current_model = model.clone();
                                (Some(stats.clone()), current_model.clone())
                            } else {
                                (None, current_model.clone())
                            };

                        timeline.push(TimelineItem::AgentReply {
                            timestamp,
                            reply: content,
                            reasoning,
                            turn_no: current_turn_no,
                            model: model_name,
                            tokens,
                            duration_ms: None,
                            reasoning_effort: None,
                        });

                        if has_seen_user_prompt {
                            current_turn_no += 1;
                            has_seen_user_prompt = false;
                        }
                    }
                }
            }
            "tool.call" | "TOOL_CALL" => {
                let p = payload.or(data);
                if let Some(p) = p {
                    let call_id = p
                        .get("id")
                        .and_then(|i| i.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = p
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    let args = p
                        .get("arguments")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);

                    let idx = timeline.len();
                    tool_calls_map.insert(call_id.clone(), idx);

                    timeline.push(TimelineItem::ToolStep {
                        timestamp,
                        tool_name: name,
                        arguments: args,
                        env: None,
                        exit_code: None,
                        stdout: "".to_string(),
                        stderr: "".to_string(),
                        tool_call_id: Some(call_id),
                        status: "running".to_string(),
                    });
                }
            }
            "tool.response" | "TOOL_RESPONSE" => {
                let p = payload.or(data);
                if let Some(p) = p {
                    let call_id = p
                        .get("id")
                        .and_then(|i| i.as_str())
                        .unwrap_or("")
                        .to_string();
                    let stdout = p
                        .get("stdout")
                        .and_then(|o| o.as_str())
                        .unwrap_or("")
                        .to_string();
                    let stderr = p
                        .get("stderr")
                        .and_then(|e| e.as_str())
                        .unwrap_or("")
                        .to_string();
                    let exit_code = p
                        .get("exitCode")
                        .or(p.get("exit_code"))
                        .and_then(|ec| ec.as_i64())
                        .map(|v| v as i32);

                    if let Some(&idx) = tool_calls_map.get(&call_id) {
                        if let Some(TimelineItem::ToolStep {
                            stdout: target_stdout,
                            stderr: target_stderr,
                            exit_code: target_exit_code,
                            status,
                            ..
                        }) = timeline.get_mut(idx)
                        {
                            *target_stdout = stdout;
                            *target_stderr = stderr;
                            *target_exit_code = exit_code;
                            *status = if exit_code.unwrap_or(0) == 0 {
                                "success".to_string()
                            } else {
                                "failed".to_string()
                            };
                        }
                    }
                }
            }
            // 新格式: tool.execution_start（若 assistant.message toolRequests 已建立此 call_id，跳過）
            "tool.execution_start" => {
                if let Some(p) = data.or(payload) {
                    let call_id = p
                        .get("toolCallId")
                        .and_then(|i| i.as_str())
                        .unwrap_or("")
                        .to_string();
                    if !tool_calls_map.contains_key(&call_id) {
                        let name = p
                            .get("toolName")
                            .and_then(|n| n.as_str())
                            .unwrap_or("")
                            .to_string();
                        let args = p
                            .get("arguments")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null);
                        let idx = timeline.len();
                        tool_calls_map.insert(call_id.clone(), idx);
                        timeline.push(TimelineItem::ToolStep {
                            timestamp,
                            tool_name: name,
                            arguments: args,
                            env: None,
                            exit_code: None,
                            stdout: "".to_string(),
                            stderr: "".to_string(),
                            tool_call_id: Some(call_id),
                            status: "running".to_string(),
                        });
                    }
                }
            }
            // 新格式: tool.execution_complete
            "tool.execution_complete" => {
                if let Some(p) = data.or(payload) {
                    let call_id = p
                        .get("toolCallId")
                        .and_then(|i| i.as_str())
                        .unwrap_or("")
                        .to_string();
                    let success = p.get("success").and_then(|s| s.as_bool()).unwrap_or(true);
                    // 優先取 detailedContent，其次取 content
                    let stdout = p
                        .get("result")
                        .and_then(|r| r.get("detailedContent").or_else(|| r.get("content")))
                        .and_then(|c| c.as_str())
                        .unwrap_or("")
                        .to_string();
                    let exit_code: Option<i32> = if success { Some(0) } else { Some(1) };

                    if let Some(&idx) = tool_calls_map.get(&call_id) {
                        if let Some(TimelineItem::ToolStep {
                            stdout: target_stdout,
                            exit_code: target_exit_code,
                            status,
                            ..
                        }) = timeline.get_mut(idx)
                        {
                            *target_stdout = stdout;
                            *target_exit_code = exit_code;
                            *status = if success {
                                "success".to_string()
                            } else {
                                "failed".to_string()
                            };
                        }
                    }
                }
            }
            "session.shutdown" => {
                timeline.push(TimelineItem::SystemStatus {
                    timestamp,
                    status_type: "session_end".to_string(),
                    message: "會話結束 (Session Ended)".to_string(),
                });
            }
            // Copilot App subagent lifecycle markers. These events carry a
            // top-level `agentId`; the filter above ensures only the matching
            // subagent view (or, for the main view, none of them) reaches here.
            "subagent.started" => {
                let p = data.or(payload);
                let display = p
                    .and_then(|p| p.get("agentDisplayName"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("Subagent");
                let name = p
                    .and_then(|p| p.get("agentName"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let model = p.and_then(|p| p.get("model")).and_then(|v| v.as_str());
                if let Some(m) = model {
                    // The subagent lifecycle event carries the model the
                    // subagent actually runs as; prefer it over any inherited
                    // value so AgentReply labels reflect the child model.
                    current_model = m.to_string();
                }
                let message = match (display, name, model) {
                    (d, n, Some(m)) if !n.is_empty() => {
                        format!("子代理啟動 (Subagent Started): {d} [{n}] @ {m}")
                    }
                    (d, n, None) if !n.is_empty() => {
                        format!("子代理啟動 (Subagent Started): {d} [{n}]")
                    }
                    (d, _, _) => format!("子代理啟動 (Subagent Started): {d}"),
                };
                timeline.push(TimelineItem::SystemStatus {
                    timestamp,
                    status_type: "subagent_started".to_string(),
                    message,
                });
            }
            "subagent.completed" => {
                timeline.push(TimelineItem::SystemStatus {
                    timestamp,
                    status_type: "subagent_completed".to_string(),
                    message: "子代理完成 (Subagent Completed)".to_string(),
                });
            }
            "subagent.failed" => {
                timeline.push(TimelineItem::SystemStatus {
                    timestamp,
                    status_type: "subagent_failed".to_string(),
                    message: "子代理失敗 (Subagent Failed)".to_string(),
                });
            }
            _ => {}
        }
    }
    metadata.insert(
        "selected_model".to_string(),
        serde_json::Value::String(current_model),
    );
}

pub fn parse_vscode_timeline(
    session: &crate::vscode::ChatSession,
    db_entries: &HashMap<u32, (TokenStats, String)>,
    timeline: &mut Vec<TimelineItem>,
    metadata: &mut HashMap<String, serde_json::Value>,
) {
    if let Some(cwd) = &session.working_directory {
        metadata.insert("cwd".to_string(), serde_json::Value::String(cwd.clone()));
    }
    if let Some(location) = &session.initial_location {
        metadata.insert(
            "initial_location".to_string(),
            serde_json::Value::String(location.clone()),
        );
    }
    if let Some(username) = &session.responder_username {
        metadata.insert(
            "responder_username".to_string(),
            serde_json::Value::String(username.clone()),
        );
    }

    let fallback_timestamp = session
        .creation_date
        .map(crate::vscode::timestamp_to_iso)
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string());
    let mut current_model = "GitHub Copilot".to_string();

    for (index, request) in session.requests.iter().enumerate() {
        let turn_no = (index + 1) as u32;
        let timestamp = request
            .timestamp
            .map(crate::vscode::timestamp_to_iso)
            .unwrap_or_else(|| fallback_timestamp.clone());
        if !request.prompt.trim().is_empty() {
            timeline.push(TimelineItem::UserPrompt {
                timestamp: timestamp.clone(),
                prompt: request.prompt.clone(),
                context: None,
                turn_no,
            });
        }

        let (tokens, model_from_db) = db_entries
            .get(&turn_no)
            .map(|(stats, model)| (Some(stats.clone()), Some(model.clone())))
            .unwrap_or((None, None));
        if let Some(model) = model_from_db.or_else(|| request.model_id.clone()) {
            current_model = model;
        }

        let mut reasoning = Vec::new();
        let mut reply_tokens = tokens.clone();
        for part in &request.response {
            let kind = part
                .get("kind")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            match kind {
                "thinking" => {
                    if let Some(text) = vscode_part_text(part) {
                        if !text.trim().is_empty() {
                            reasoning.push(text);
                        }
                    }
                }
                "markdownContent" | "markdownVuln" | "text" | "info" | "warning" => {
                    if let Some(reply) = vscode_part_text(part) {
                        if !reply.trim().is_empty() {
                            timeline.push(TimelineItem::AgentReply {
                                timestamp: timestamp.clone(),
                                reply,
                                reasoning: if reasoning.is_empty() {
                                    None
                                } else {
                                    Some(reasoning.join("\n"))
                                },
                                turn_no,
                                model: current_model.clone(),
                                tokens: reply_tokens.take(),
                                duration_ms: request.elapsed_ms,
                                reasoning_effort: None,
                            });
                            reasoning.clear();
                        }
                    }
                }
                "toolInvocationSerialized" | "toolInvocation" => {
                    let call_id = part
                        .get("toolCallId")
                        .or_else(|| part.get("id"))
                        .and_then(|value| value.as_str())
                        .map(str::to_string);
                    let tool_name = part
                        .get("toolName")
                        .or_else(|| part.get("name"))
                        .or_else(|| part.get("toolId"))
                        .or_else(|| part.get("source").and_then(|source| source.get("label")))
                        .and_then(|value| value.as_str())
                        .unwrap_or("VS Code Tool")
                        .to_string();
                    let arguments = part
                        .get("parameters")
                        .or_else(|| part.get("arguments"))
                        .or_else(|| part.get("input"))
                        .or_else(|| part.get("invocationMessage"))
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    let result = part
                        .get("resultDetails")
                        .or_else(|| part.get("result"))
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    let succeeded = part
                        .get("isComplete")
                        .and_then(|value| value.as_bool())
                        .unwrap_or(!result.is_null());
                    timeline.push(TimelineItem::ToolStep {
                        timestamp: timestamp.clone(),
                        tool_name,
                        arguments,
                        env: None,
                        exit_code: Some(if succeeded { 0 } else { 1 }),
                        stdout: vscode_value_text(&result),
                        stderr: String::new(),
                        tool_call_id: call_id,
                        status: if succeeded {
                            "success".to_string()
                        } else {
                            "running".to_string()
                        },
                    });
                }
                _ => {}
            }
        }
    }

    metadata.insert(
        "selected_model".to_string(),
        serde_json::Value::String(current_model),
    );
}

fn vscode_part_text(part: &serde_json::Value) -> Option<String> {
    part.get("content")
        .or_else(|| part.get("value"))
        .or_else(|| part.get("text"))
        .map(vscode_value_text)
}

fn vscode_value_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(values) => values
            .iter()
            .map(vscode_value_text)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        serde_json::Value::Object(object) => object
            .get("value")
            .or_else(|| object.get("text"))
            .or_else(|| object.get("content"))
            .map(vscode_value_text)
            .unwrap_or_else(|| value.to_string()),
        _ => value.to_string(),
    }
}

fn codex_text_from_content(content: &serde_json::Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_string();
    }

    let mut parts = Vec::new();
    if let Some(items) = content.as_array() {
        for item in items {
            match item.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                "input_text" | "output_text" | "text" => {
                    if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                        parts.push(text.to_string());
                    }
                }
                _ => {}
            }
        }
    }
    parts.join("\n")
}

fn codex_tool_arguments(payload: &serde_json::Value) -> serde_json::Value {
    let args = payload
        .get("arguments")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    if let Some(raw_args) = args.as_str() {
        serde_json::from_str(raw_args)
            .unwrap_or_else(|_| serde_json::Value::String(raw_args.to_string()))
    } else {
        args
    }
}

pub fn parse_codex_timeline(
    reader: BufReader<File>,
    db_entries: &HashMap<u32, (TokenStats, String)>,
    timeline: &mut Vec<TimelineItem>,
    metadata: &mut HashMap<String, serde_json::Value>,
) {
    let mut current_model = "GPT-5.3-Codex".to_string();
    let mut current_turn_no = 0u32;
    let mut next_agent_turn_no = 1u32;
    let mut tool_calls_map: HashMap<String, usize> = HashMap::new();
    let mut seen_user_messages: HashSet<String> = HashSet::new();
    let mut seen_agent_messages: HashSet<String> = HashSet::new();
    let mut emitted_turn_tokens: HashSet<u32> = HashSet::new();
    let mut reasoning_effort: Option<String> = None;

    for line_res in reader.lines() {
        let line = match line_res {
            Ok(line) => line,
            Err(_) => continue,
        };
        let event: serde_json::Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => continue,
        };

        let event_type = event.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let timestamp = event
            .get("timestamp")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        let payload = match event.get("payload") {
            Some(payload) => payload,
            None => continue,
        };
        let payload_type = payload.get("type").and_then(|t| t.as_str()).unwrap_or("");

        match event_type {
            "session_meta" => {
                if let Some(version) = payload.get("cli_version") {
                    metadata.insert("codex_version".to_string(), version.clone());
                }
                if let Some(cwd) = payload.get("cwd") {
                    metadata.insert("cwd".to_string(), cwd.clone());
                }
                if let Some(nickname) = payload.get("agent_nickname") {
                    metadata.insert("agent_nickname".to_string(), nickname.clone());
                }
                if let Some(role) = payload.get("agent_role") {
                    metadata.insert("agent_role".to_string(), role.clone());
                }
                if let Some(parent) = payload.get("parent_thread_id") {
                    metadata.insert("parent_session_id".to_string(), parent.clone());
                }
                if let Some(git) = payload.get("git") {
                    if let Some(branch) = git.get("branch") {
                        metadata.insert("git_branch".to_string(), branch.clone());
                    }
                    if let Some(repo) = git.get("repository_url").or_else(|| git.get("repository"))
                    {
                        metadata.insert("repository".to_string(), repo.clone());
                    }
                }
                if let Some(model) = payload.get("model").and_then(|m| m.as_str()) {
                    current_model = model.to_string();
                }
                timeline.push(TimelineItem::SystemStatus {
                    timestamp,
                    status_type: "session_start".to_string(),
                    message: "會話開始 (Session Started)".to_string(),
                });
            }
            "turn_context" => {
                if let Some(cwd) = payload.get("cwd") {
                    metadata
                        .entry("cwd".to_string())
                        .or_insert_with(|| cwd.clone());
                }
                if let Some(model) = payload.get("model").and_then(|m| m.as_str()) {
                    current_model = model.to_string();
                }
                reasoning_effort = payload
                    .get("effort")
                    .or_else(|| payload.get("reasoning_effort"))
                    .and_then(|effort| effort.as_str())
                    .map(|effort| effort.to_string())
                    .or(reasoning_effort);
            }
            "event_msg" => match payload_type {
                "user_message" => {
                    let prompt = payload
                        .get("message")
                        .and_then(|message| message.as_str())
                        .unwrap_or("")
                        .to_string();
                    if !prompt.trim().is_empty()
                        && seen_user_messages.insert(format!("{}:{}", timestamp, prompt))
                    {
                        current_turn_no += 1;
                        next_agent_turn_no = current_turn_no;
                        timeline.push(TimelineItem::UserPrompt {
                            timestamp,
                            prompt,
                            context: None,
                            turn_no: current_turn_no,
                        });
                    }
                }
                "agent_message" => {
                    let reply = payload
                        .get("message")
                        .and_then(|message| message.as_str())
                        .unwrap_or("")
                        .to_string();
                    if !reply.trim().is_empty()
                        && seen_agent_messages.insert(format!("{}:{}", timestamp, reply))
                    {
                        let turn_no = next_agent_turn_no.max(1);
                        let tokens = if emitted_turn_tokens.insert(turn_no) {
                            db_entries.get(&turn_no).map(|(stats, model)| {
                                current_model = model.clone();
                                stats.clone()
                            })
                        } else {
                            None
                        };
                        timeline.push(TimelineItem::AgentReply {
                            timestamp,
                            reply,
                            reasoning: None,
                            turn_no,
                            model: current_model.clone(),
                            tokens,
                            duration_ms: None,
                            reasoning_effort: reasoning_effort.clone(),
                        });
                    }
                }
                "task_started" => {
                    timeline.push(TimelineItem::SystemStatus {
                        timestamp,
                        status_type: "task_started".to_string(),
                        message: "任務開始 (Task Started)".to_string(),
                    });
                }
                "task_complete" => {
                    timeline.push(TimelineItem::SystemStatus {
                        timestamp,
                        status_type: "task_complete".to_string(),
                        message: "任務完成 (Task Complete)".to_string(),
                    });
                }
                _ => {}
            },
            "response_item" => match payload_type {
                "message" => {
                    let role = payload
                        .get("role")
                        .and_then(|role| role.as_str())
                        .unwrap_or("");
                    if role == "user" {
                        if let Some(content) = payload.get("content") {
                            let prompt = codex_text_from_content(content);
                            if !prompt.trim().is_empty()
                                && seen_user_messages.insert(format!("{}:{}", timestamp, prompt))
                            {
                                current_turn_no += 1;
                                next_agent_turn_no = current_turn_no;
                                timeline.push(TimelineItem::UserPrompt {
                                    timestamp,
                                    prompt,
                                    context: None,
                                    turn_no: current_turn_no,
                                });
                            }
                        }
                    } else if role == "assistant" {
                        if let Some(content) = payload.get("content") {
                            let reply = codex_text_from_content(content);
                            if !reply.trim().is_empty()
                                && seen_agent_messages.insert(format!("{}:{}", timestamp, reply))
                            {
                                let turn_no = next_agent_turn_no.max(1);
                                let tokens = if emitted_turn_tokens.insert(turn_no) {
                                    db_entries.get(&turn_no).map(|(stats, model)| {
                                        current_model = model.clone();
                                        stats.clone()
                                    })
                                } else {
                                    None
                                };
                                timeline.push(TimelineItem::AgentReply {
                                    timestamp,
                                    reply,
                                    reasoning: None,
                                    turn_no,
                                    model: current_model.clone(),
                                    tokens,
                                    duration_ms: None,
                                    reasoning_effort: reasoning_effort.clone(),
                                });
                            }
                        }
                    }
                }
                "function_call" => {
                    let call_id = payload
                        .get("call_id")
                        .and_then(|id| id.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = payload
                        .get("name")
                        .and_then(|name| name.as_str())
                        .unwrap_or("function_call")
                        .to_string();
                    let idx = timeline.len();
                    tool_calls_map.insert(call_id.clone(), idx);
                    timeline.push(TimelineItem::ToolStep {
                        timestamp,
                        tool_name: name,
                        arguments: codex_tool_arguments(payload),
                        env: None,
                        exit_code: None,
                        stdout: "".to_string(),
                        stderr: "".to_string(),
                        tool_call_id: Some(call_id),
                        status: "running".to_string(),
                    });
                }
                "function_call_output" => {
                    let call_id = payload
                        .get("call_id")
                        .and_then(|id| id.as_str())
                        .unwrap_or("")
                        .to_string();
                    let output = payload
                        .get("output")
                        .and_then(|output| output.as_str())
                        .unwrap_or("")
                        .to_string();
                    if let Some(&idx) = tool_calls_map.get(&call_id) {
                        if let Some(TimelineItem::ToolStep {
                            stdout,
                            exit_code,
                            status,
                            ..
                        }) = timeline.get_mut(idx)
                        {
                            *stdout = output;
                            *exit_code = Some(0);
                            *status = "success".to_string();
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    metadata.insert(
        "selected_model".to_string(),
        serde_json::Value::String(current_model),
    );
}

fn claude_text_from_content(content: &serde_json::Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_string();
    }

    let mut parts = Vec::new();
    if let Some(items) = content.as_array() {
        for item in items {
            match item.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                "text" => {
                    if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                        parts.push(text.to_string());
                    }
                }
                "tool_result" => {
                    if let Some(text) = item.get("content").and_then(|c| c.as_str()) {
                        parts.push(text.to_string());
                    }
                }
                _ => {}
            }
        }
    }
    parts.join("\n")
}

pub fn parse_claude_timeline(
    reader: BufReader<File>,
    db_entries: &HashMap<u32, (TokenStats, String)>,
    timeline: &mut Vec<TimelineItem>,
    metadata: &mut HashMap<String, serde_json::Value>,
) {
    let mut current_model = "Claude Code".to_string();
    let mut request_turns: HashMap<String, u32> = HashMap::new();
    let mut emitted_reply_tokens: HashSet<String> = HashSet::new();
    let mut tool_calls_map: HashMap<String, usize> = HashMap::new();
    let mut user_turn_no = 0u32;

    for line_res in reader.lines() {
        let line = match line_res {
            Ok(line) => line,
            Err(_) => continue,
        };
        let event: serde_json::Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => continue,
        };

        let timestamp = event
            .get("timestamp")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();

        if let Some(cwd) = event.get("cwd") {
            metadata
                .entry("cwd".to_string())
                .or_insert_with(|| cwd.clone());
        }
        if let Some(version) = event.get("version") {
            metadata
                .entry("copilot_version".to_string())
                .or_insert_with(|| version.clone());
        }
        if let Some(branch) = event.get("gitBranch") {
            metadata
                .entry("git_branch".to_string())
                .or_insert_with(|| branch.clone());
        }

        let message = match event.get("message") {
            Some(message) => message,
            None => continue,
        };
        let role = message.get("role").and_then(|r| r.as_str()).unwrap_or("");
        let content = message.get("content");

        if role == "user" {
            if let Some(content) = content {
                let has_tool_result = content
                    .as_array()
                    .map(|items| {
                        items.iter().any(|item| {
                            item.get("type").and_then(|t| t.as_str()) == Some("tool_result")
                        })
                    })
                    .unwrap_or(false);

                if has_tool_result {
                    if let Some(items) = content.as_array() {
                        for item in items {
                            if item.get("type").and_then(|t| t.as_str()) != Some("tool_result") {
                                continue;
                            }
                            let call_id = item
                                .get("tool_use_id")
                                .and_then(|id| id.as_str())
                                .unwrap_or("")
                                .to_string();
                            let output =
                                claude_text_from_content(item.get("content").unwrap_or(item));
                            let is_error = item
                                .get("is_error")
                                .and_then(|value| value.as_bool())
                                .unwrap_or(false);

                            if let Some(&idx) = tool_calls_map.get(&call_id) {
                                if let Some(TimelineItem::ToolStep {
                                    stdout,
                                    exit_code,
                                    status,
                                    ..
                                }) = timeline.get_mut(idx)
                                {
                                    *stdout = output;
                                    *exit_code = Some(if is_error { 1 } else { 0 });
                                    *status = if is_error {
                                        "failed".to_string()
                                    } else {
                                        "success".to_string()
                                    };
                                }
                            }
                        }
                    }
                } else {
                    let prompt = claude_text_from_content(content);
                    if !prompt.trim().is_empty() {
                        user_turn_no += 1;
                        timeline.push(TimelineItem::UserPrompt {
                            timestamp,
                            prompt,
                            context: None,
                            turn_no: user_turn_no,
                        });
                    }
                }
            }
            continue;
        }

        if role != "assistant" {
            continue;
        }

        if let Some(model) = message.get("model").and_then(|m| m.as_str()) {
            current_model = model.to_string();
        }

        let request_key = event
            .get("requestId")
            .and_then(|id| id.as_str())
            .or_else(|| message.get("id").and_then(|id| id.as_str()))
            .or_else(|| event.get("uuid").and_then(|id| id.as_str()))
            .unwrap_or("")
            .to_string();
        let turn_no = if request_key.is_empty() {
            (request_turns.len() + 1) as u32
        } else if let Some(turn_no) = request_turns.get(&request_key) {
            *turn_no
        } else {
            let next_turn_no = (request_turns.len() + 1) as u32;
            request_turns.insert(request_key.clone(), next_turn_no);
            next_turn_no
        };

        let mut tokens_for_reply = |request_key: &str, turn_no: u32, current_model: &mut String| {
            if request_key.is_empty() || !emitted_reply_tokens.insert(request_key.to_string()) {
                return None;
            }
            db_entries.get(&turn_no).map(|(stats, model)| {
                *current_model = model.clone();
                stats.clone()
            })
        };

        if let Some(content) = content {
            if let Some(items) = content.as_array() {
                let mut reasoning_parts = Vec::new();
                for item in items {
                    match item.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                        "thinking" => {
                            if let Some(thinking) = item.get("thinking").and_then(|t| t.as_str()) {
                                reasoning_parts.push(thinking.to_string());
                            }
                        }
                        "text" => {
                            let reply = item
                                .get("text")
                                .and_then(|text| text.as_str())
                                .unwrap_or("")
                                .to_string();
                            if !reply.trim().is_empty() {
                                let tokens =
                                    tokens_for_reply(&request_key, turn_no, &mut current_model);
                                timeline.push(TimelineItem::AgentReply {
                                    timestamp: timestamp.clone(),
                                    reply,
                                    reasoning: if reasoning_parts.is_empty() {
                                        None
                                    } else {
                                        Some(reasoning_parts.join("\n"))
                                    },
                                    turn_no,
                                    model: current_model.clone(),
                                    tokens,
                                    duration_ms: None,
                                    reasoning_effort: None,
                                });
                                reasoning_parts.clear();
                            }
                        }
                        "tool_use" => {
                            let call_id = item
                                .get("id")
                                .and_then(|id| id.as_str())
                                .unwrap_or("")
                                .to_string();
                            let name = item
                                .get("name")
                                .and_then(|name| name.as_str())
                                .unwrap_or("tool_use")
                                .to_string();
                            let args = item
                                .get("input")
                                .cloned()
                                .unwrap_or(serde_json::Value::Null);
                            let idx = timeline.len();
                            tool_calls_map.insert(call_id.clone(), idx);
                            timeline.push(TimelineItem::ToolStep {
                                timestamp: timestamp.clone(),
                                tool_name: name,
                                arguments: args,
                                env: None,
                                exit_code: None,
                                stdout: "".to_string(),
                                stderr: "".to_string(),
                                tool_call_id: Some(call_id),
                                status: "running".to_string(),
                            });
                        }
                        _ => {}
                    }
                }
            } else {
                let reply = claude_text_from_content(content);
                if !reply.trim().is_empty() {
                    let tokens = tokens_for_reply(&request_key, turn_no, &mut current_model);
                    timeline.push(TimelineItem::AgentReply {
                        timestamp,
                        reply,
                        reasoning: None,
                        turn_no,
                        model: current_model.clone(),
                        tokens,
                        duration_ms: None,
                        reasoning_effort: None,
                    });
                }
            }
        }
    }

    metadata.insert(
        "selected_model".to_string(),
        serde_json::Value::String(current_model),
    );
}

pub fn parse_cursor_timeline(
    reader: BufReader<std::fs::File>,
    db_entries: &HashMap<u32, (TokenStats, String)>,
    timeline: &mut Vec<TimelineItem>,
    metadata: &mut HashMap<String, serde_json::Value>,
) {
    let mut current_model = "Unknown Model".to_string();
    let mut user_turn_no = 0u32;
    let mut agent_turn_no = 0u32;

    let mut current_timestamp = String::new();

    for line_res in reader.lines() {
        let line = match line_res {
            Ok(line) => line,
            Err(_) => continue,
        };
        let event: serde_json::Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => continue,
        };

        let role = event.get("role").and_then(|r| r.as_str()).unwrap_or("");

        if role == "user" {
            let content_val = event.get("message").and_then(|m| m.get("content"));
            let text = cursor_content_to_text(content_val.unwrap_or(&serde_json::Value::Null));

            let mut extracted_ts = String::new();
            if let Some(start_idx) = text.find("<timestamp>") {
                let actual_start = start_idx + "<timestamp>".len();
                if let Some(end_idx) = text[actual_start..].find("</timestamp>") {
                    extracted_ts = text[actual_start..(actual_start + end_idx)].to_string();
                }
            }

            if !extracted_ts.is_empty() {
                current_timestamp = parse_cursor_timestamp(&extracted_ts);
            }

            let mut clean_prompt = text.clone();
            if let Some(start_idx) = clean_prompt.find("<user_query>") {
                let actual_start = start_idx + "<user_query>".len();
                if let Some(end_idx) = clean_prompt[actual_start..].find("</user_query>") {
                    clean_prompt = clean_prompt[actual_start..(actual_start + end_idx)].to_string();
                }
            }

            let prompt = clean_prompt.trim().to_string();
            if !prompt.is_empty() {
                user_turn_no += 1;
                timeline.push(TimelineItem::UserPrompt {
                    timestamp: current_timestamp.clone(),
                    prompt,
                    context: None,
                    turn_no: user_turn_no,
                });
            }
        } else if role == "assistant" {
            let message = match event.get("message") {
                Some(message) => message,
                None => continue,
            };
            let content = message.get("content");

            agent_turn_no += 1;
            let mut reply_parts = Vec::new();

            if let Some(content) = content {
                if let Some(items) = content.as_array() {
                    for item in items {
                        match item.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                            "text" => {
                                if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                                    reply_parts.push(text.to_string());
                                }
                            }
                            "tool_use" => {
                                let name = item
                                    .get("name")
                                    .and_then(|name| name.as_str())
                                    .unwrap_or("tool_use")
                                    .to_string();
                                let args = item
                                    .get("input")
                                    .cloned()
                                    .unwrap_or(serde_json::Value::Null);

                                timeline.push(TimelineItem::ToolStep {
                                    timestamp: current_timestamp.clone(),
                                    tool_name: name,
                                    arguments: args,
                                    env: None,
                                    exit_code: Some(0),
                                    stdout: "".to_string(),
                                    stderr: "".to_string(),
                                    tool_call_id: None,
                                    status: "success".to_string(),
                                });
                            }
                            _ => {}
                        }
                    }
                } else if let Some(text) = content.as_str() {
                    reply_parts.push(text.to_string());
                }
            }

            let reply = reply_parts.join("\n");
            let stats = db_entries.get(&agent_turn_no).map(|(s, _)| s.clone());
            if let Some((_, model_name)) = db_entries.get(&agent_turn_no) {
                current_model = model_name.clone();
            }

            timeline.push(TimelineItem::AgentReply {
                timestamp: current_timestamp.clone(),
                reply,
                reasoning: None,
                turn_no: agent_turn_no,
                model: current_model.clone(),
                tokens: stats,
                duration_ms: None,
                reasoning_effort: None,
            });
        } else {
            let event_type = event.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if event_type == "turn_ended" {
                if let Some(err) = event.get("error").and_then(|e| e.as_str()) {
                    timeline.push(TimelineItem::SystemStatus {
                        timestamp: current_timestamp.clone(),
                        status_type: "error".to_string(),
                        message: err.to_string(),
                    });
                }
            }
        }
    }

    metadata.insert(
        "selected_model".to_string(),
        serde_json::Value::String(current_model),
    );
}

fn cursor_content_to_text(content: &serde_json::Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_string();
    }
    let mut parts = Vec::new();
    if let Some(items) = content.as_array() {
        for item in items {
            let itype = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if itype == "text" {
                if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                    parts.push(text.to_string());
                }
            }
        }
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Write the given JSONL lines to a temp file and return a `BufReader`.
    /// Each test gets a unique file so parallel test runs do not collide.
    fn reader_for_events(lines: &[&str]) -> (BufReader<File>, std::path::PathBuf) {
        let mut path = std::env::temp_dir();
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        path.push(format!(
            "token-insights-timeline-test-{}-{}-{}.jsonl",
            std::process::id(),
            n,
            unique
        ));
        fs::write(&path, lines.join("\n")).unwrap();
        let file = File::open(&path).unwrap();
        (BufReader::new(file), path)
    }

    fn extract_replies(timeline: &[TimelineItem]) -> Vec<String> {
        timeline
            .iter()
            .filter_map(|item| match item {
                TimelineItem::AgentReply { reply, .. } => Some(reply.clone()),
                _ => None,
            })
            .collect()
    }

    fn extract_tool_names(timeline: &[TimelineItem]) -> Vec<String> {
        timeline
            .iter()
            .filter_map(|item| match item {
                TimelineItem::ToolStep { tool_name, .. } => Some(tool_name.clone()),
                _ => None,
            })
            .collect()
    }

    /// A shared events.jsonl containing a main agent turn, a subagent
    /// (`call_v4b32z66`) assistant message + tool execution, and subagent
    /// lifecycle markers. Mirrors the real Copilot App layout.
    fn shared_events() -> Vec<&'static str> {
        vec![
            r#"{"type":"session.start","data":{"copilotVersion":"1.0"},"timestamp":"2026-07-22T10:00:00Z"}"#,
            r#"{"type":"user.message","data":{"content":"Please summarize"},"timestamp":"2026-07-22T10:00:01Z"}"#,
            r#"{"type":"assistant.message","data":{"content":"Main agent reply"},"timestamp":"2026-07-22T10:00:02Z"}"#,
            r#"{"type":"tool.execution_start","data":{"toolCallId":"main-tool-1","toolName":"Bash"},"timestamp":"2026-07-22T10:00:03Z"}"#,
            r#"{"type":"tool.execution_complete","data":{"toolCallId":"main-tool-1","success":true,"result":{"content":"done"}},"timestamp":"2026-07-22T10:00:04Z"}"#,
            r#"{"type":"subagent.started","agentId":"call_v4b32z66","data":{"agentDisplayName":"K2.7","agentName":"K2.7","model":"cbc40143"},"timestamp":"2026-07-22T10:00:05Z"}"#,
            r#"{"type":"assistant.message","agentId":"call_v4b32z66","data":{"content":"Subagent reply"},"timestamp":"2026-07-22T10:00:06Z"}"#,
            r#"{"type":"tool.execution_start","agentId":"call_v4b32z66","data":{"toolCallId":"sub-tool-1","toolName":"Grep"},"timestamp":"2026-07-22T10:00:07Z"}"#,
            r#"{"type":"tool.execution_complete","agentId":"call_v4b32z66","data":{"toolCallId":"sub-tool-1","success":true,"result":{"content":"found"}},"timestamp":"2026-07-22T10:00:08Z"}"#,
            r#"{"type":"subagent.completed","agentId":"call_v4b32z66","timestamp":"2026-07-22T10:00:09Z"}"#,
            r#"{"type":"session.shutdown","timestamp":"2026-07-22T10:00:10Z"}"#,
        ]
    }

    #[test]
    fn main_agent_filter_excludes_subagent_assistant_and_tool_events() {
        let (reader, path) = reader_for_events(&shared_events());
        let db_entries = HashMap::new();
        let mut timeline = Vec::new();
        let mut metadata = HashMap::new();
        parse_copilot_timeline_filtered(
            reader,
            &db_entries,
            &mut timeline,
            &mut metadata,
            None,
            None,
        );

        let replies = extract_replies(&timeline);
        assert_eq!(replies, vec!["Main agent reply".to_string()]);
        let tools = extract_tool_names(&timeline);
        assert_eq!(tools, vec!["Bash".to_string()]);
        // No subagent lifecycle marker should leak into the main agent view.
        assert!(!timeline.iter().any(|item| matches!(
            item,
            TimelineItem::SystemStatus { status_type, .. }
                if status_type == "subagent_started" || status_type == "subagent_completed"
        )));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn subagent_filter_keeps_only_its_agent_events_plus_shared_context() {
        let (reader, path) = reader_for_events(&shared_events());
        let db_entries = HashMap::new();
        let mut timeline = Vec::new();
        let mut metadata = HashMap::new();
        parse_copilot_timeline_filtered(
            reader,
            &db_entries,
            &mut timeline,
            &mut metadata,
            Some("call_v4b32z66"),
            None,
        );

        let replies = extract_replies(&timeline);
        assert_eq!(replies, vec!["Subagent reply".to_string()]);
        let tools = extract_tool_names(&timeline);
        assert_eq!(tools, vec!["Grep".to_string()]);
        // Shared context (user.message + session.start/shutdown) should be kept
        // so the subagent drawer still shows the originating prompt.
        assert!(timeline.iter().any(|item| matches!(
            item,
            TimelineItem::UserPrompt { prompt, .. } if prompt == "Please summarize"
        )));
        // Subagent lifecycle markers for this agent should be present.
        assert!(timeline.iter().any(|item| matches!(
            item,
            TimelineItem::SystemStatus { status_type, .. } if status_type == "subagent_started"
        )));
        // Main agent reply must NOT appear.
        assert!(!extract_replies(&timeline)
            .iter()
            .any(|r| r == "Main agent reply"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn cli_timeline_with_no_agent_filter_is_unchanged_when_events_have_no_agentid() {
        // Pure Copilot CLI events (no top-level agentId) must reconstruct
        // identically to the original behavior, i.e. both the shim and the
        // filtered parser with None produce the same timeline.
        let cli_events = vec![
            r#"{"type":"session.start","data":{"copilotVersion":"1.0"},"timestamp":"2026-07-22T10:00:00Z"}"#,
            r#"{"type":"user.message","data":{"content":"hi"},"timestamp":"2026-07-22T10:00:01Z"}"#,
            r#"{"type":"assistant.message","data":{"content":"hello back"},"timestamp":"2026-07-22T10:00:02Z"}"#,
        ];
        let (reader, path) = reader_for_events(&cli_events);
        let db_entries = HashMap::new();
        let mut timeline = Vec::new();
        let mut metadata = HashMap::new();
        parse_copilot_timeline_filtered(
            reader,
            &db_entries,
            &mut timeline,
            &mut metadata,
            None,
            None,
        );

        assert_eq!(extract_replies(&timeline), vec!["hello back".to_string()]);
        assert!(timeline.iter().any(|item| matches!(
            item,
            TimelineItem::UserPrompt { prompt, .. } if prompt == "hi"
        )));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn subagent_filter_with_no_matching_events_yields_no_agent_specific_items() {
        // No event carries agentId "ghost"; the subagent filter should keep
        // only shared context (user.message/session.start) and no agent-specific
        // replies, tool steps, or subagent lifecycle markers. The handler maps
        // a timeline with no agent-specific items to content_unavailable.
        let (reader, path) = reader_for_events(&shared_events());
        let db_entries = HashMap::new();
        let mut timeline = Vec::new();
        let mut metadata = HashMap::new();
        parse_copilot_timeline_filtered(
            reader,
            &db_entries,
            &mut timeline,
            &mut metadata,
            Some("ghost"),
            None,
        );
        assert!(extract_replies(&timeline).is_empty());
        assert!(extract_tool_names(&timeline).is_empty());
        assert!(!timeline.iter().any(|item| matches!(
            item,
            TimelineItem::SystemStatus { status_type, .. }
                if status_type == "subagent_started" || status_type == "subagent_completed"
        )));
        let _ = fs::remove_file(path);
    }

    /// Helper: extract every `(model, reply)` pair from AgentReply items.
    fn extract_reply_models(timeline: &[TimelineItem]) -> Vec<(String, String)> {
        timeline
            .iter()
            .filter_map(|item| match item {
                TimelineItem::AgentReply { model, reply, .. } => {
                    Some((model.clone(), reply.clone()))
                }
                _ => None,
            })
            .collect()
    }

    /// Helper: read `metadata.selected_model` as a `String`.
    fn metadata_model(metadata: &HashMap<String, serde_json::Value>) -> Option<String> {
        metadata
            .get("selected_model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    /// Regression A: CLI parent/child model mismatch.
    ///
    /// parent `session.start.selectedModel` = GLM5.2-none, but the DB child
    /// session model is gpt-5.4-mini. The subagent drawer's
    /// `metadata.selected_model` and every child `AgentReply.model` must be
    /// gpt-5.4-mini, and GLM5.2-none must NOT appear anywhere in the subagent
    /// drawer output.
    #[test]
    fn cli_subagent_drawer_uses_child_db_model_not_parent_selected_model() {
        let events = vec![
            r#"{"type":"session.start","data":{"copilotVersion":"1.0","context":{"cwd":"/tmp"},"selectedModel":"GLM5.2-none"},"timestamp":"2026-07-22T10:00:00Z"}"#,
            r#"{"type":"user.message","data":{"content":"please run the subagent"},"timestamp":"2026-07-22T10:00:01Z"}"#,
            r#"{"type":"assistant.message","data":{"content":"main agent reply"},"timestamp":"2026-07-22T10:00:02Z"}"#,
            r#"{"type":"subagent.started","agentId":"call_f91rg5gy","data":{"agentDisplayName":"GPT","agentName":"GPT"},"timestamp":"2026-07-22T10:00:05Z"}"#,
            r#"{"type":"assistant.message","agentId":"call_f91rg5gy","data":{"content":"subagent reply"},"timestamp":"2026-07-22T10:00:06Z"}"#,
            r#"{"type":"subagent.completed","agentId":"call_f91rg5gy","timestamp":"2026-07-22T10:00:09Z"}"#,
        ];
        let (reader, path) = reader_for_events(&events);
        let db_entries = HashMap::new();
        let mut timeline = Vec::new();
        let mut metadata = HashMap::new();
        parse_copilot_timeline_filtered(
            reader,
            &db_entries,
            &mut timeline,
            &mut metadata,
            Some("call_f91rg5gy"),
            Some("gpt-5.4-mini"),
        );

        assert_eq!(
            metadata_model(&metadata).as_deref(),
            Some("gpt-5.4-mini"),
            "subagent drawer selected_model must be the child DB model"
        );
        let reply_models = extract_reply_models(&timeline);
        assert!(
            reply_models.iter().all(|(m, _)| m == "gpt-5.4-mini"),
            "every subagent AgentReply.model must be gpt-5.4-mini, got {:?}",
            reply_models
        );
        // The parent model must NOT leak into any subagent AgentReply.
        assert!(
            !reply_models.iter().any(|(m, _)| m == "GLM5.2-none"),
            "GLM5.2-none must not appear in subagent AgentReply models: {:?}",
            reply_models
        );
        // The parent main agent reply must be filtered out.
        assert!(
            !extract_replies(&timeline)
                .iter()
                .any(|r| r == "main agent reply"),
            "main agent reply must not leak into subagent drawer"
        );
        let _ = fs::remove_file(path);
    }

    /// Regression B: Copilot App parent/child model mismatch.
    ///
    /// parent = GLM5.2-medium, child DB model = claude-haiku-4.5. The subagent
    /// drawer's metadata and AgentReply models must show claude-haiku-4.5, and
    /// GLM5.2-medium must NOT appear.
    #[test]
    fn app_subagent_drawer_uses_child_db_model_not_parent_selected_model() {
        let events = vec![
            r#"{"type":"session.start","data":{"copilotVersion":"1.0","context":{"cwd":"/tmp"},"selectedModel":"GLM5.2-medium"},"timestamp":"2026-07-22T10:00:00Z"}"#,
            r#"{"type":"user.message","data":{"content":"please research"},"timestamp":"2026-07-22T10:00:01Z"}"#,
            r#"{"type":"subagent.started","agentId":"call_2m0yl1q0","data":{"agentDisplayName":"Claude","agentName":"Claude"},"timestamp":"2026-07-22T10:00:05Z"}"#,
            r#"{"type":"assistant.message","agentId":"call_2m0yl1q0","data":{"content":"research summary"},"timestamp":"2026-07-22T10:00:06Z"}"#,
            r#"{"type":"subagent.completed","agentId":"call_2m0yl1q0","timestamp":"2026-07-22T10:00:09Z"}"#,
        ];
        let (reader, path) = reader_for_events(&events);
        let db_entries = HashMap::new();
        let mut timeline = Vec::new();
        let mut metadata = HashMap::new();
        parse_copilot_timeline_filtered(
            reader,
            &db_entries,
            &mut timeline,
            &mut metadata,
            Some("call_2m0yl1q0"),
            Some("claude-haiku-4.5"),
        );

        assert_eq!(
            metadata_model(&metadata).as_deref(),
            Some("claude-haiku-4.5"),
            "App subagent drawer selected_model must be the child DB model"
        );
        let reply_models = extract_reply_models(&timeline);
        assert!(
            reply_models.iter().all(|(m, _)| m == "claude-haiku-4.5"),
            "every App subagent AgentReply.model must be claude-haiku-4.5, got {:?}",
            reply_models
        );
        assert!(
            !reply_models.iter().any(|(m, _)| m == "GLM5.2-medium"),
            "GLM5.2-medium must not appear in App subagent AgentReply models: {:?}",
            reply_models
        );
        let _ = fs::remove_file(path);
    }

    /// Regression C: another App mismatch (parent = DP4F, child = K2.7).
    #[test]
    fn app_subagent_drawer_dp4f_parent_k2_7_child_shows_k2_7() {
        let events = vec![
            r#"{"type":"session.start","data":{"copilotVersion":"1.0","context":{"cwd":"/tmp"},"selectedModel":"DP4F"},"timestamp":"2026-07-22T10:00:00Z"}"#,
            r#"{"type":"user.message","data":{"content":"please explore"},"timestamp":"2026-07-22T10:00:01Z"}"#,
            r#"{"type":"subagent.started","agentId":"call_o6g6unk8","data":{"agentDisplayName":"K2.7","agentName":"K2.7"},"timestamp":"2026-07-22T10:00:05Z"}"#,
            r#"{"type":"assistant.message","agentId":"call_o6g6unk8","data":{"content":"explore result"},"timestamp":"2026-07-22T10:00:06Z"}"#,
            r#"{"type":"subagent.completed","agentId":"call_o6g6unk8","timestamp":"2026-07-22T10:00:09Z"}"#,
        ];
        let (reader, path) = reader_for_events(&events);
        let db_entries = HashMap::new();
        let mut timeline = Vec::new();
        let mut metadata = HashMap::new();
        parse_copilot_timeline_filtered(
            reader,
            &db_entries,
            &mut timeline,
            &mut metadata,
            Some("call_o6g6unk8"),
            Some("K2.7"),
        );

        assert_eq!(
            metadata_model(&metadata).as_deref(),
            Some("K2.7"),
            "App subagent drawer selected_model must be K2.7"
        );
        let reply_models = extract_reply_models(&timeline);
        assert!(
            reply_models.iter().all(|(m, _)| m == "K2.7"),
            "every AgentReply.model must be K2.7, got {:?}",
            reply_models
        );
        assert!(
            !reply_models.iter().any(|(m, _)| m == "DP4F"),
            "DP4F must not appear in App subagent AgentReply models: {:?}",
            reply_models
        );
        let _ = fs::remove_file(path);
    }

    /// Regression D: main session drawer still shows the main agent model and
    /// is not overwritten by any subagent's DB model (db_session_model = None).
    #[test]
    fn main_session_drawer_keeps_main_model_without_subagent_override() {
        // The main agent view passes db_session_model = None; the shared
        // session.start.selectedModel seeds the model. Subagent events are
        // filtered out entirely (None filter), so no subagent model can leak.
        let events = vec![
            r#"{"type":"session.start","data":{"copilotVersion":"1.0","context":{"cwd":"/tmp"},"selectedModel":"GLM5.2-medium"},"timestamp":"2026-07-22T10:00:00Z"}"#,
            r#"{"type":"user.message","data":{"content":"hello"},"timestamp":"2026-07-22T10:00:01Z"}"#,
            r#"{"type":"assistant.message","data":{"content":"main reply"},"timestamp":"2026-07-22T10:00:02Z"}"#,
            r#"{"type":"subagent.started","agentId":"call_2m0yl1q0","data":{"agentDisplayName":"Claude","agentName":"Claude","model":"claude-haiku-4.5"},"timestamp":"2026-07-22T10:00:05Z"}"#,
            r#"{"type":"assistant.message","agentId":"call_2m0yl1q0","data":{"content":"sub reply"},"timestamp":"2026-07-22T10:00:06Z"}"#,
            r#"{"type":"subagent.completed","agentId":"call_2m0yl1q0","timestamp":"2026-07-22T10:00:09Z"}"#,
        ];
        let (reader, path) = reader_for_events(&events);
        let db_entries = HashMap::new();
        let mut timeline = Vec::new();
        let mut metadata = HashMap::new();
        // Main agent view: no agent filter and no DB-sourced child model.
        parse_copilot_timeline_filtered(
            reader,
            &db_entries,
            &mut timeline,
            &mut metadata,
            None,
            None,
        );

        assert_eq!(
            metadata_model(&metadata).as_deref(),
            Some("GLM5.2-medium"),
            "main session drawer must keep the parent selectedModel"
        );
        let replies = extract_replies(&timeline);
        assert_eq!(replies, vec!["main reply".to_string()]);
        let reply_models = extract_reply_models(&timeline);
        assert!(
            reply_models.iter().all(|(m, _)| m == "GLM5.2-medium"),
            "main AgentReply.model must be the main model, got {:?}",
            reply_models
        );
        // No subagent reply leaks into the main drawer.
        assert!(
            !replies.iter().any(|r| r == "sub reply"),
            "subagent reply must not leak into main drawer"
        );
        let _ = fs::remove_file(path);
    }

    /// Regression E: multiple subagents under the same parent each show their
    /// own DB-sourced model in their own synthetic-session drawer.
    #[test]
    fn multiple_subagents_each_show_their_own_child_model() {
        let events = vec![
            r#"{"type":"session.start","data":{"copilotVersion":"1.0","context":{"cwd":"/tmp"},"selectedModel":"GLM5.2-medium"},"timestamp":"2026-07-22T10:00:00Z"}"#,
            r#"{"type":"user.message","data":{"content":"please research"},"timestamp":"2026-07-22T10:00:01Z"}"#,
            r#"{"type":"subagent.started","agentId":"call_2m0yl1q0","data":{"agentDisplayName":"Claude","agentName":"Claude"},"timestamp":"2026-07-22T10:00:05Z"}"#,
            r#"{"type":"assistant.message","agentId":"call_2m0yl1q0","data":{"content":"claude summary"},"timestamp":"2026-07-22T10:00:06Z"}"#,
            r#"{"type":"subagent.completed","agentId":"call_2m0yl1q0","timestamp":"2026-07-22T10:00:07Z"}"#,
            r#"{"type":"subagent.started","agentId":"call_o6g6unk8","data":{"agentDisplayName":"K2.7","agentName":"K2.7"},"timestamp":"2026-07-22T10:00:08Z"}"#,
            r#"{"type":"assistant.message","agentId":"call_o6g6unk8","data":{"content":"k2 summary"},"timestamp":"2026-07-22T10:00:09Z"}"#,
            r#"{"type":"subagent.completed","agentId":"call_o6g6unk8","timestamp":"2026-07-22T10:00:10Z"}"#,
        ];
        // First subagent drawer: claude-haiku-4.5.
        {
            let (reader, path) = reader_for_events(&events);
            let db_entries = HashMap::new();
            let mut timeline = Vec::new();
            let mut metadata = HashMap::new();
            parse_copilot_timeline_filtered(
                reader,
                &db_entries,
                &mut timeline,
                &mut metadata,
                Some("call_2m0yl1q0"),
                Some("claude-haiku-4.5"),
            );
            assert_eq!(
                metadata_model(&metadata).as_deref(),
                Some("claude-haiku-4.5"),
                "first subagent drawer must show claude-haiku-4.5"
            );
            let reply_models = extract_reply_models(&timeline);
            assert!(
                reply_models.iter().all(|(m, _)| m == "claude-haiku-4.5"),
                "first subagent AgentReply.model must be claude-haiku-4.5, got {:?}",
                reply_models
            );
            assert!(extract_replies(&timeline) == vec!["claude summary".to_string()]);
            let _ = fs::remove_file(path);
        }
        // Second subagent drawer: K2.7.
        {
            let (reader, path) = reader_for_events(&events);
            let db_entries = HashMap::new();
            let mut timeline = Vec::new();
            let mut metadata = HashMap::new();
            parse_copilot_timeline_filtered(
                reader,
                &db_entries,
                &mut timeline,
                &mut metadata,
                Some("call_o6g6unk8"),
                Some("K2.7"),
            );
            assert_eq!(
                metadata_model(&metadata).as_deref(),
                Some("K2.7"),
                "second subagent drawer must show K2.7"
            );
            let reply_models = extract_reply_models(&timeline);
            assert!(
                reply_models.iter().all(|(m, _)| m == "K2.7"),
                "second subagent AgentReply.model must be K2.7, got {:?}",
                reply_models
            );
            assert!(extract_replies(&timeline) == vec!["k2 summary".to_string()]);
            let _ = fs::remove_file(path);
        }
    }

    /// Regression: `subagent.started` event model updates the current model so
    /// AgentReply labels reflect the subagent's runtime model even when no DB
    /// session model was supplied (defensive fallback path).
    #[test]
    fn subagent_started_event_model_updates_current_model() {
        let events = vec![
            r#"{"type":"session.start","data":{"copilotVersion":"1.0","context":{"cwd":"/tmp"},"selectedModel":"GLM5.2-medium"},"timestamp":"2026-07-22T10:00:00Z"}"#,
            r#"{"type":"user.message","data":{"content":"hi"},"timestamp":"2026-07-22T10:00:01Z"}"#,
            r#"{"type":"subagent.started","agentId":"call_evt_model","data":{"agentDisplayName":"Claude","agentName":"Claude","model":"claude-haiku-4.5"},"timestamp":"2026-07-22T10:00:05Z"}"#,
            r#"{"type":"assistant.message","agentId":"call_evt_model","data":{"content":"sub reply"},"timestamp":"2026-07-22T10:00:06Z"}"#,
            r#"{"type":"subagent.completed","agentId":"call_evt_model","timestamp":"2026-07-22T10:00:07Z"}"#,
        ];
        let (reader, path) = reader_for_events(&events);
        let db_entries = HashMap::new();
        let mut timeline = Vec::new();
        let mut metadata = HashMap::new();
        parse_copilot_timeline_filtered(
            reader,
            &db_entries,
            &mut timeline,
            &mut metadata,
            Some("call_evt_model"),
            None,
        );
        assert_eq!(
            metadata_model(&metadata).as_deref(),
            Some("claude-haiku-4.5"),
            "subagent.started model must seed the drawer when no DB model is supplied"
        );
        let reply_models = extract_reply_models(&timeline);
        assert!(
            reply_models.iter().all(|(m, _)| m == "claude-haiku-4.5"),
            "AgentReply.model must follow subagent.started model, got {:?}",
            reply_models
        );
        let _ = fs::remove_file(path);
    }
}

fn grok_turn_number(
    update: &serde_json::Value,
    next_turn: u32,
    zero_based: &mut Option<bool>,
) -> u32 {
    if let Some(raw) = update
        .get("turn_number")
        .and_then(|value| value.as_u64())
        .and_then(|value| u32::try_from(value).ok())
    {
        let is_zero_based = zero_based.get_or_insert(raw == 0);
        return raw
            .checked_add(u32::from(*is_zero_based))
            .unwrap_or(next_turn.max(1));
    }
    update
        .get("turnNo")
        .or_else(|| update.get("turnNumber"))
        .or_else(|| update.get("turn"))
        .and_then(|value| value.as_u64())
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(next_turn.max(1))
}

fn grok_update_content(update: &serde_json::Value) -> String {
    value_to_text(
        update
            .get("content")
            .or_else(|| update.get("chunk"))
            .or_else(|| update.get("message"))
            .or_else(|| update.get("text")),
    )
}

fn grok_tool_output(update: &serde_json::Value) -> (String, String) {
    let output = update
        .get("rawOutput")
        .or_else(|| update.get("raw_output"))
        .or_else(|| update.get("content"));
    if let Some(output) = output {
        let stdout = value_to_text(Some(output));
        if !stdout.is_empty() {
            return (stdout, String::new());
        }
        if let Some(value) = output.get("stdout").and_then(|value| value.as_str()) {
            let stderr = output
                .get("stderr")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string();
            return (value.to_string(), stderr);
        }
    }
    (String::new(), String::new())
}

pub fn parse_grok_timeline(
    reader: BufReader<File>,
    db_entries: &HashMap<u32, (TokenStats, String)>,
    timeline: &mut Vec<TimelineItem>,
    metadata: &mut HashMap<String, serde_json::Value>,
) {
    let mut current_turn = 0u32;
    let mut next_turn = 1u32;
    let mut zero_based = None;
    let mut current_model = UNKNOWN_MODEL.to_string();
    let mut user_indices = HashMap::<u32, usize>::new();
    let mut reply_parts = HashMap::<u32, Vec<String>>::new();
    let mut reasoning_parts = HashMap::<u32, Vec<String>>::new();
    let mut tool_indices = HashMap::<String, usize>::new();
    let mut session_started = false;
    let mut last_timestamp = String::new();

    for line_res in reader.lines() {
        let Ok(line) = line_res else {
            continue;
        };
        let Ok(event) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let update = event
            .get("params")
            .and_then(|params| params.get("update"))
            .unwrap_or(&event);
        let update_type = update
            .get("sessionUpdate")
            .or_else(|| update.get("session_update"))
            .or_else(|| update.get("type"))
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let timestamp = timestamp_to_rfc3339(event.get("timestamp"));
        if !timestamp.is_empty() {
            last_timestamp = timestamp.clone();
        }

        if let Some(model) = update
            .get("model")
            .or_else(|| update.get("model_id"))
            .or_else(|| update.get("modelId"))
            .and_then(|value| value.as_str())
        {
            current_model = display_model_name(model, None);
        }

        if matches!(update_type, "turn_started" | "user_message_chunk") && current_turn == 0 {
            current_turn = grok_turn_number(update, next_turn, &mut zero_based);
            next_turn = current_turn.saturating_add(1);
            if !session_started {
                timeline.push(TimelineItem::SystemStatus {
                    timestamp: timestamp.clone(),
                    status_type: "session_start".to_string(),
                    message: "Grok Build session started".to_string(),
                });
                session_started = true;
            }
        }

        if update_type == "user_message_chunk" {
            let prompt = grok_update_content(update);
            if !prompt.is_empty() {
                if let Some(index) = user_indices.get(&current_turn).copied() {
                    if let Some(TimelineItem::UserPrompt { prompt: text, .. }) =
                        timeline.get_mut(index)
                    {
                        text.push_str(&prompt);
                    }
                } else {
                    user_indices.insert(current_turn, timeline.len());
                    timeline.push(TimelineItem::UserPrompt {
                        timestamp: timestamp.clone(),
                        prompt,
                        context: None,
                        turn_no: current_turn,
                    });
                }
            }
        } else if update_type == "agent_message_chunk" {
            let reply = grok_update_content(update);
            if !reply.is_empty() {
                reply_parts.entry(current_turn).or_default().push(reply);
            }
        } else if update_type == "agent_thought_chunk" {
            let thought = grok_update_content(update);
            if !thought.is_empty() {
                reasoning_parts
                    .entry(current_turn)
                    .or_default()
                    .push(thought);
            }
        } else if update_type == "tool_call" {
            let call_id = update
                .get("toolCallId")
                .or_else(|| update.get("tool_call_id"))
                .or_else(|| update.get("id"))
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string();
            let tool_name = update
                .get("title")
                .or_else(|| update.get("name"))
                .and_then(|value| value.as_str())
                .unwrap_or("tool_call")
                .to_string();
            let arguments = update
                .get("rawInput")
                .or_else(|| update.get("raw_input"))
                .or_else(|| update.get("input"))
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let index = timeline.len();
            if !call_id.is_empty() {
                tool_indices.insert(call_id.clone(), index);
            }
            timeline.push(TimelineItem::ToolStep {
                timestamp,
                tool_name,
                arguments,
                env: None,
                exit_code: None,
                stdout: String::new(),
                stderr: String::new(),
                tool_call_id: (!call_id.is_empty()).then_some(call_id),
                status: "running".to_string(),
            });
        } else if update_type == "tool_call_update" {
            let call_id = update
                .get("toolCallId")
                .or_else(|| update.get("tool_call_id"))
                .or_else(|| update.get("id"))
                .and_then(|value| value.as_str())
                .unwrap_or("");
            if let Some(index) = tool_indices.get(call_id).copied() {
                let (stdout, stderr) = grok_tool_output(update);
                let raw_status = update
                    .get("status")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                if let Some(TimelineItem::ToolStep {
                    stdout: current_stdout,
                    stderr: current_stderr,
                    exit_code,
                    status,
                    ..
                }) = timeline.get_mut(index)
                {
                    if !stdout.is_empty() {
                        *current_stdout = stdout;
                    }
                    if !stderr.is_empty() {
                        *current_stderr = stderr;
                    }
                    match raw_status {
                        "completed" | "complete" | "success" | "succeeded" => {
                            *status = "success".to_string();
                            *exit_code = Some(0);
                        }
                        "failed" | "error" | "cancelled" | "canceled" => {
                            *status = "failed".to_string();
                            *exit_code = Some(1);
                        }
                        _ if !raw_status.is_empty() => *status = raw_status.to_string(),
                        _ => {}
                    }
                }
            }
        } else if update_type == "turn_completed" {
            let reply = reply_parts
                .remove(&current_turn)
                .unwrap_or_default()
                .join("");
            let reasoning = reasoning_parts
                .remove(&current_turn)
                .map(|parts| parts.join(""))
                .filter(|text| !text.is_empty());
            let (tokens, model) = db_entries
                .get(&current_turn)
                .map(|(stats, model)| (Some(stats.clone()), model.clone()))
                .unwrap_or((None, current_model.clone()));
            current_model = model.clone();
            if !reply.is_empty() || tokens.is_some() {
                timeline.push(TimelineItem::AgentReply {
                    timestamp,
                    reply,
                    reasoning,
                    turn_no: current_turn,
                    model,
                    tokens,
                    duration_ms: None,
                    reasoning_effort: None,
                });
            }
            current_turn = 0;
        }
    }

    if current_turn != 0 {
        let reply = reply_parts
            .remove(&current_turn)
            .unwrap_or_default()
            .join("");
        let reasoning = reasoning_parts
            .remove(&current_turn)
            .map(|parts| parts.join(""))
            .filter(|text| !text.is_empty());
        let (tokens, model) = db_entries
            .get(&current_turn)
            .map(|(stats, model)| (Some(stats.clone()), model.clone()))
            .unwrap_or((None, current_model.clone()));
        if !reply.is_empty() || tokens.is_some() {
            timeline.push(TimelineItem::AgentReply {
                timestamp: last_timestamp,
                reply,
                reasoning,
                turn_no: current_turn,
                model,
                tokens,
                duration_ms: None,
                reasoning_effort: None,
            });
        }
    }

    metadata.insert(
        "selected_model".to_string(),
        serde_json::Value::String(current_model),
    );
}

/// Shared timeline reconstruction for the Pi Coding Agent
/// (<https://pi.dev/>) and its fork OMP (<https://omp.sh/>), which persist
/// sessions using the identical tree-structured JSONL format. See
/// `crate::pi` for the format reference and the DB-sync-side parser.
fn parse_pi_family_timeline(
    reader: BufReader<File>,
    db_entries: &HashMap<u32, (TokenStats, String)>,
    timeline: &mut Vec<TimelineItem>,
    metadata: &mut HashMap<String, serde_json::Value>,
    session_label: &str,
) {
    let mut turn_no = 1u32;
    let mut current_model = UNKNOWN_MODEL.to_string();
    let mut tool_indices: HashMap<String, usize> = HashMap::new();
    let mut session_started = false;

    for line_res in reader.lines() {
        let Ok(line) = line_res else {
            continue;
        };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let entry_type = entry.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let timestamp = entry
            .get("timestamp")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if entry_type == "session" {
            if !session_started {
                timeline.push(TimelineItem::SystemStatus {
                    timestamp: timestamp.clone(),
                    status_type: "session_start".to_string(),
                    message: format!("{session_label} session started"),
                });
                session_started = true;
            }
            if let Some(cwd) = entry.get("cwd").and_then(|v| v.as_str()) {
                metadata.insert(
                    "cwd".to_string(),
                    serde_json::Value::String(cwd.to_string()),
                );
            }
            continue;
        }

        if entry_type == "compaction" {
            if let Some(summary) = entry.get("summary").and_then(|v| v.as_str()) {
                timeline.push(TimelineItem::SystemStatus {
                    timestamp,
                    status_type: "compaction".to_string(),
                    message: summary.to_string(),
                });
            }
            continue;
        }

        if entry_type == "model_change" {
            // Pi uses `modelId`, while OMP's fork uses `model` for the same
            // event; accept either key name.
            let model_id = entry
                .get("modelId")
                .or_else(|| entry.get("model"))
                .and_then(|v| v.as_str());
            if let Some(model_id) = model_id {
                current_model = model_id.to_string();
                timeline.push(TimelineItem::SystemStatus {
                    timestamp,
                    status_type: "model_change".to_string(),
                    message: format!("Model changed to {model_id}"),
                });
            }
            continue;
        }

        if entry_type != "message" {
            continue;
        }
        let Some(message) = entry.get("message") else {
            continue;
        };
        let role = message.get("role").and_then(|v| v.as_str()).unwrap_or("");

        match role {
            "user" => {
                let prompt = value_to_text(message.get("content"));
                if !prompt.is_empty() {
                    timeline.push(TimelineItem::UserPrompt {
                        timestamp,
                        prompt,
                        context: None,
                        turn_no,
                    });
                }
            }
            "assistant" => {
                let mut reply_parts = Vec::new();
                let mut reasoning_parts = Vec::new();
                if let Some(content) = message.get("content").and_then(|v| v.as_array()) {
                    for block in content {
                        match block.get("type").and_then(|v| v.as_str()) {
                            Some("text") => {
                                if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                                    reply_parts.push(text.to_string());
                                }
                            }
                            Some("thinking") => {
                                if let Some(text) = block.get("thinking").and_then(|v| v.as_str()) {
                                    reasoning_parts.push(text.to_string());
                                }
                            }
                            Some("toolCall") => {
                                let call_id = block
                                    .get("id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let tool_name = block
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("tool_call")
                                    .to_string();
                                let arguments = block
                                    .get("arguments")
                                    .cloned()
                                    .unwrap_or(serde_json::Value::Null);
                                let index = timeline.len();
                                if !call_id.is_empty() {
                                    tool_indices.insert(call_id.clone(), index);
                                }
                                timeline.push(TimelineItem::ToolStep {
                                    timestamp: timestamp.clone(),
                                    tool_name,
                                    arguments,
                                    env: None,
                                    exit_code: None,
                                    stdout: String::new(),
                                    stderr: String::new(),
                                    tool_call_id: (!call_id.is_empty()).then_some(call_id),
                                    status: "running".to_string(),
                                });
                            }
                            _ => {}
                        }
                    }
                }

                if let Some(model) = message.get("model").and_then(|v| v.as_str()) {
                    current_model = model.to_string();
                }
                let (tokens, model) = db_entries
                    .get(&turn_no)
                    .map(|(stats, model)| (Some(stats.clone()), model.clone()))
                    .unwrap_or((None, current_model.clone()));
                current_model = model.clone();

                let reply = reply_parts.join("");
                let reasoning = (!reasoning_parts.is_empty()).then(|| reasoning_parts.join(""));
                if !reply.is_empty() || tokens.is_some() {
                    timeline.push(TimelineItem::AgentReply {
                        timestamp,
                        reply,
                        reasoning,
                        turn_no,
                        model,
                        tokens,
                        duration_ms: None,
                        reasoning_effort: None,
                    });
                }
                turn_no += 1;
            }
            "toolResult" => {
                let call_id = message
                    .get("toolCallId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let is_error = message
                    .get("isError")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let text = value_to_text(message.get("content"));
                let (stdout, stderr) = if is_error {
                    (String::new(), text)
                } else {
                    (text, String::new())
                };
                if let Some(index) = tool_indices.get(call_id).copied() {
                    if let Some(TimelineItem::ToolStep {
                        stdout: current_stdout,
                        stderr: current_stderr,
                        exit_code,
                        status,
                        ..
                    }) = timeline.get_mut(index)
                    {
                        if !stdout.is_empty() {
                            *current_stdout = stdout;
                        }
                        if !stderr.is_empty() {
                            *current_stderr = stderr;
                        }
                        *status = if is_error {
                            "failed".to_string()
                        } else {
                            "success".to_string()
                        };
                        *exit_code = Some(if is_error { 1 } else { 0 });
                    }
                } else {
                    let tool_name = message
                        .get("toolName")
                        .and_then(|v| v.as_str())
                        .unwrap_or("tool_result")
                        .to_string();
                    timeline.push(TimelineItem::ToolStep {
                        timestamp,
                        tool_name,
                        arguments: serde_json::Value::Null,
                        env: None,
                        exit_code: Some(if is_error { 1 } else { 0 }),
                        stdout,
                        stderr,
                        tool_call_id: (!call_id.is_empty()).then(|| call_id.to_string()),
                        status: if is_error {
                            "failed".to_string()
                        } else {
                            "success".to_string()
                        },
                    });
                }
            }
            "bashExecution" => {
                let command = message
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let output = message
                    .get("output")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let exit_code = message
                    .get("exitCode")
                    .and_then(|v| v.as_i64())
                    .map(|v| v as i32);
                let cancelled = message
                    .get("cancelled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let status = if cancelled {
                    "failed".to_string()
                } else {
                    match exit_code {
                        Some(0) => "success".to_string(),
                        Some(_) => "failed".to_string(),
                        None => "running".to_string(),
                    }
                };
                timeline.push(TimelineItem::ToolStep {
                    timestamp,
                    tool_name: "bash".to_string(),
                    arguments: serde_json::json!({ "command": command }),
                    env: None,
                    exit_code,
                    stdout: output,
                    stderr: String::new(),
                    tool_call_id: None,
                    status,
                });
            }
            _ => {}
        }
    }

    metadata.insert(
        "selected_model".to_string(),
        serde_json::Value::String(current_model),
    );
}

pub fn parse_pi_timeline(
    reader: BufReader<File>,
    db_entries: &HashMap<u32, (TokenStats, String)>,
    timeline: &mut Vec<TimelineItem>,
    metadata: &mut HashMap<String, serde_json::Value>,
) {
    parse_pi_family_timeline(reader, db_entries, timeline, metadata, "Pi Coding Agent")
}

pub fn parse_omp_timeline(
    reader: BufReader<File>,
    db_entries: &HashMap<u32, (TokenStats, String)>,
    timeline: &mut Vec<TimelineItem>,
    metadata: &mut HashMap<String, serde_json::Value>,
) {
    parse_pi_family_timeline(reader, db_entries, timeline, metadata, "OMP")
}

pub fn parse_muse_timeline(
    reader: BufReader<File>,
    db_entries: &HashMap<u32, (TokenStats, String)>,
    timeline: &mut Vec<TimelineItem>,
    metadata: &mut HashMap<String, serde_json::Value>,
) {
    let mut turn_no: u32 = 1;
    let mut current_model = "muse-spark-1.2".to_string();
    let mut tool_indices: HashMap<String, usize> = HashMap::new();

    for line in reader.lines().map_while(Result::ok) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let payload_type = value
            .get("payload_type")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let payload = value.get("payload");
        let timestamp = value
            .get("recorded_at")
            .and_then(|v| v.as_u64())
            .map(|us| {
                // Muse recorded_at is microseconds (16 digits) -> 2026-09-02
                let secs = us / 1_000_000;
                let nanos = ((us % 1_000_000) * 1000) as u32;
                if let Some(dt) =
                    chrono::DateTime::<chrono::Utc>::from_timestamp(secs as i64, nanos)
                {
                    if secs > 1_577_836_800 && secs < 4_102_444_800 {
                        return dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
                    }
                }
                let secs2 = us / 1_000_000_000;
                let nanos2 = (us % 1_000_000_000) as u32;
                chrono::DateTime::<chrono::Utc>::from_timestamp(secs2 as i64, nanos2)
                    .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
                    .unwrap_or_default()
            })
            .unwrap_or_default();

        if payload_type == "runtime.user_intent.accepted" {
            if let Some(p) = payload {
                let prompt = p
                    .get("model_messages")
                    .and_then(|v| v.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|msg| msg.get("content"))
                    .map(|content| {
                        if let Some(text) = content.as_str() {
                            text.to_string()
                        } else if let Some(arr) = content.as_array() {
                            arr.iter()
                                .filter_map(|item| item.get("text").and_then(|t| t.as_str()))
                                .collect::<Vec<_>>()
                                .join(" ")
                        } else {
                            String::new()
                        }
                    })
                    .or_else(|| {
                        p.get("refill_blocks")
                            .and_then(|v| v.as_array())
                            .and_then(|arr| arr.first())
                            .and_then(|b| b.get("text").and_then(|t| t.as_str()))
                            .map(|s| s.to_string())
                    })
                    .unwrap_or_default();
                if !prompt.trim().is_empty() {
                    timeline.push(TimelineItem::UserPrompt {
                        timestamp: timestamp.clone(),
                        prompt,
                        context: None,
                        turn_no,
                    });
                }
            }
            continue;
        }

        if payload_type == "runtime.session" {
            let Some(p) = payload else { continue };
            let kind = p.get("kind").and_then(|v| v.as_str()).unwrap_or("");
            if kind != "run" {
                // also handle metadata for cwd
                continue;
            }
            let event = p.get("event");
            let ek = event
                .and_then(|e| e.get("kind"))
                .and_then(|v| v.as_str())
                .unwrap_or("");

            match ek {
                "assistant_tool_calls_committed" => {
                    let tool_calls = event
                        .and_then(|e| e.get("tool_calls"))
                        .and_then(|v| v.as_array());
                    if let Some(calls) = tool_calls {
                        for call in calls {
                            let call_id = call
                                .get("call_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let name = call
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("tool")
                                .to_string();
                            let args = call.get("args").cloned().unwrap_or(serde_json::Value::Null);
                            let args_val = if let Value::String(s) = &args {
                                serde_json::from_str(s).unwrap_or(Value::String(s.clone()))
                            } else {
                                args.clone()
                            };
                            let idx = timeline.len();
                            if !call_id.is_empty() {
                                tool_indices.insert(call_id.clone(), idx);
                            }
                            timeline.push(TimelineItem::ToolStep {
                                timestamp: timestamp.clone(),
                                tool_name: name,
                                arguments: args_val,
                                env: None,
                                exit_code: None,
                                stdout: String::new(),
                                stderr: String::new(),
                                tool_call_id: if call_id.is_empty() {
                                    None
                                } else {
                                    Some(call_id)
                                },
                                status: "running".to_string(),
                            });
                        }
                    }
                }
                "tool_result_batch_committed" => {
                    let results = event
                        .and_then(|e| e.get("results"))
                        .and_then(|v| v.as_array());
                    if let Some(results) = results {
                        for result in results {
                            let call_id = result
                                .get("tool_call_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            let text = result
                                .get("text")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            if let Some(&idx) = tool_indices.get(call_id) {
                                if let Some(TimelineItem::ToolStep {
                                    stdout,
                                    stderr,
                                    exit_code,
                                    status,
                                    ..
                                }) = timeline.get_mut(idx)
                                {
                                    *stdout = text.clone();
                                    *stderr = String::new();
                                    *exit_code = Some(0);
                                    *status = "success".to_string();
                                }
                            } else {
                                timeline.push(TimelineItem::ToolStep {
                                    timestamp: timestamp.clone(),
                                    tool_name: "tool_result".to_string(),
                                    arguments: serde_json::Value::Null,
                                    env: None,
                                    exit_code: Some(0),
                                    stdout: text,
                                    stderr: String::new(),
                                    tool_call_id: if call_id.is_empty() {
                                        None
                                    } else {
                                        Some(call_id.to_string())
                                    },
                                    status: "success".to_string(),
                                });
                            }
                        }
                    }
                }
                "model_completed" => {
                    if let Some(_usage) = event.and_then(|e| e.get("usage")) {
                        let model = event
                            .and_then(|e| e.get("model"))
                            .and_then(|v| v.as_str())
                            .unwrap_or(&current_model)
                            .to_string();
                        current_model = model.clone();
                        let (tokens, _) = db_entries
                            .get(&turn_no)
                            .map(|(s, m)| (Some(s.clone()), m.clone()))
                            .unwrap_or((None, model.clone()));
                        timeline.push(TimelineItem::AgentReply {
                            timestamp: timestamp.clone(),
                            reply: String::new(),
                            reasoning: None,
                            turn_no,
                            model,
                            tokens,
                            duration_ms: event
                                .and_then(|e| e.get("duration_ms"))
                                .and_then(|v| v.as_u64()),
                            reasoning_effort: None,
                        });
                        turn_no += 1;
                    }
                }
                "reasoning_summary_committed" | "reasoning_summary_delta" => {
                    // ignore, could add as reasoning but AgentReply already covers
                }
                _ => {}
            }
        }
    }

    metadata.insert(
        "selected_model".to_string(),
        serde_json::Value::String(current_model),
    );
    metadata.insert("cwd".to_string(), serde_json::Value::String(String::new()));
}

/// Reconstructs the conversation timeline of one MiniMax Code session. Unlike
/// the other assistants a single session spans several JSONL files
/// (`messages.jsonl` plus `snapshots/*.jsonl`), so the session directory is
/// passed in instead of one reader. Turns are numbered with
/// [`crate::mcode::assistant_usage`] so they line up with the rows written by
/// `crate::mcode::parse_session_usage`. `metadata["cwd"]` is deliberately left
/// unset so the DB-sourced working directory stays authoritative.
pub fn parse_mcode_timeline(
    session_dir: &Path,
    db_entries: &HashMap<u32, (TokenStats, String)>,
    timeline: &mut Vec<TimelineItem>,
    metadata: &mut HashMap<String, serde_json::Value>,
) {
    let records = crate::mcode::collect_session_records(session_dir);
    let mut turn_no = 1u32;
    let mut current_model = UNKNOWN_MODEL.to_string();
    let mut tool_indices: HashMap<String, usize> = HashMap::new();
    let mut session_started = false;

    for record in &records {
        let Some(message) = record.get("message") else {
            continue;
        };
        let timestamp = crate::mcode::record_timestamp(record)
            .map(crate::mcode::epoch_ms_to_rfc3339)
            .unwrap_or_default();

        if !session_started {
            timeline.push(TimelineItem::SystemStatus {
                timestamp: timestamp.clone(),
                status_type: "session_start".to_string(),
                message: "MiniMax Code session started".to_string(),
            });
            session_started = true;
        }

        match message.get("role").and_then(Value::as_str).unwrap_or("") {
            "user" => {
                let prompt = value_to_text(message.get("content"));
                if !prompt.is_empty() {
                    timeline.push(TimelineItem::UserPrompt {
                        timestamp,
                        prompt,
                        context: None,
                        turn_no,
                    });
                }
            }
            "compactionSummary" => {
                if let Some(summary) = message.get("summary").and_then(Value::as_str) {
                    timeline.push(TimelineItem::SystemStatus {
                        timestamp,
                        status_type: "compaction".to_string(),
                        message: summary.to_string(),
                    });
                }
            }
            "assistant" => {
                let mut reply_parts = Vec::new();
                let mut reasoning_parts = Vec::new();
                if let Some(content) = message.get("content").and_then(Value::as_array) {
                    for block in content {
                        match block.get("type").and_then(Value::as_str) {
                            Some("text") => {
                                if let Some(text) = block.get("text").and_then(Value::as_str) {
                                    reply_parts.push(text.to_string());
                                }
                            }
                            Some("thinking") => {
                                if let Some(text) = block.get("thinking").and_then(Value::as_str) {
                                    reasoning_parts.push(text.to_string());
                                }
                            }
                            Some("toolCall") => {
                                let call_id = block
                                    .get("id")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .to_string();
                                let tool_name = block
                                    .get("name")
                                    .and_then(Value::as_str)
                                    .unwrap_or("tool_call")
                                    .to_string();
                                let arguments =
                                    block.get("arguments").cloned().unwrap_or(Value::Null);
                                let index = timeline.len();
                                if !call_id.is_empty() {
                                    tool_indices.insert(call_id.clone(), index);
                                }
                                timeline.push(TimelineItem::ToolStep {
                                    timestamp: timestamp.clone(),
                                    tool_name,
                                    arguments,
                                    env: None,
                                    exit_code: None,
                                    stdout: String::new(),
                                    stderr: String::new(),
                                    tool_call_id: (!call_id.is_empty()).then_some(call_id),
                                    status: "running".to_string(),
                                });
                            }
                            _ => {}
                        }
                    }
                }

                if let Some(model) = message.get("model").and_then(Value::as_str) {
                    current_model = model.to_string();
                }
                // Gate the DB lookup on the same predicate that assigns turn
                // numbers; otherwise a reply that never consumed a turn would
                // borrow the next billable turn's token stats.
                let billable = crate::mcode::assistant_usage(message);
                let (tokens, model) = match (billable.as_ref(), db_entries.get(&turn_no)) {
                    (Some(_), Some((stats, model))) => (Some(stats.clone()), model.clone()),
                    _ => (None, current_model.clone()),
                };
                current_model = model.clone();

                let reply = reply_parts.join("");
                let reasoning = (!reasoning_parts.is_empty()).then(|| reasoning_parts.join(""));
                if !reply.is_empty() || tokens.is_some() {
                    timeline.push(TimelineItem::AgentReply {
                        timestamp,
                        reply,
                        reasoning,
                        turn_no,
                        model,
                        tokens,
                        duration_ms: None,
                        reasoning_effort: None,
                    });
                }

                if billable.is_some() {
                    turn_no += 1;
                }
            }
            "toolResult" => {
                let call_id = message
                    .get("toolCallId")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let is_error = message
                    .get("isError")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let text = value_to_text(message.get("content"));
                let (stdout, stderr) = if is_error {
                    (String::new(), text)
                } else {
                    (text, String::new())
                };
                if let Some(index) = tool_indices.get(call_id).copied() {
                    if let Some(TimelineItem::ToolStep {
                        stdout: current_stdout,
                        stderr: current_stderr,
                        exit_code,
                        status,
                        ..
                    }) = timeline.get_mut(index)
                    {
                        if !stdout.is_empty() {
                            *current_stdout = stdout;
                        }
                        if !stderr.is_empty() {
                            *current_stderr = stderr;
                        }
                        *status = if is_error {
                            "failed".to_string()
                        } else {
                            "success".to_string()
                        };
                        *exit_code = Some(if is_error { 1 } else { 0 });
                    }
                } else {
                    let tool_name = message
                        .get("toolName")
                        .and_then(Value::as_str)
                        .unwrap_or("tool_result")
                        .to_string();
                    timeline.push(TimelineItem::ToolStep {
                        timestamp,
                        tool_name,
                        arguments: Value::Null,
                        env: None,
                        exit_code: Some(if is_error { 1 } else { 0 }),
                        stdout,
                        stderr,
                        tool_call_id: (!call_id.is_empty()).then(|| call_id.to_string()),
                        status: if is_error {
                            "failed".to_string()
                        } else {
                            "success".to_string()
                        },
                    });
                }
            }
            _ => {}
        }
    }

    metadata.insert(
        "selected_model".to_string(),
        serde_json::Value::String(current_model),
    );
}

#[cfg(test)]
mod pi_family_tests {
    use super::*;
    use std::io::Write;

    fn make_reader(lines: &[&str]) -> BufReader<File> {
        let path = std::env::temp_dir().join(format!(
            "token-usage-insights-pi-timeline-test-{}-{}",
            std::process::id(),
            lines.len()
        ));
        let mut file = File::create(&path).unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
        let file = File::open(&path).unwrap();
        std::fs::remove_file(&path).ok();
        BufReader::new(file)
    }

    #[test]
    fn parse_pi_timeline_reconstructs_prompt_reply_and_tool_call() {
        let reader = make_reader(&[
            r#"{"type":"session","version":3,"id":"sess-1","timestamp":"2024-12-03T14:00:00.000Z","cwd":"/tmp/project"}"#,
            r#"{"type":"message","id":"m1","parentId":null,"timestamp":"2024-12-03T14:00:01.000Z","message":{"role":"user","content":"List files"}}"#,
            r#"{"type":"message","id":"m2","parentId":"m1","timestamp":"2024-12-03T14:00:02.000Z","message":{"role":"assistant","content":[{"type":"text","text":"Sure"},{"type":"toolCall","id":"call_1","name":"bash","arguments":{"command":"ls"}}],"model":"claude-sonnet-4-5","usage":{"input":10,"output":5,"totalTokens":15,"cost":{"total":0.001}}}}"#,
            r#"{"type":"message","id":"m3","parentId":"m2","timestamp":"2024-12-03T14:00:03.000Z","message":{"role":"toolResult","toolCallId":"call_1","toolName":"bash","content":[{"type":"text","text":"file.txt"}],"isError":false}}"#,
        ]);
        let mut db_entries = HashMap::new();
        db_entries.insert(
            1u32,
            (
                TokenStats {
                    input: 10,
                    output: 5,
                    cache_read: None,
                    cache_write: None,
                    cache_write_5m: None,
                    cache_write_1h: None,
                    reasoning: None,
                    total: 15,
                },
                "claude-sonnet-4-5".to_string(),
            ),
        );
        let mut timeline = Vec::new();
        let mut metadata = HashMap::new();

        parse_pi_timeline(reader, &db_entries, &mut timeline, &mut metadata);

        let has_prompt = timeline.iter().any(|item| matches!(item, TimelineItem::UserPrompt { prompt, .. } if prompt == "List files"));
        assert!(has_prompt);
        let has_reply = timeline
            .iter()
            .any(|item| matches!(item, TimelineItem::AgentReply { reply, .. } if reply == "Sure"));
        assert!(has_reply);
        let tool_step = timeline.iter().find_map(|item| match item {
            TimelineItem::ToolStep {
                tool_name,
                stdout,
                status,
                ..
            } if tool_name == "bash" => Some((stdout.clone(), status.clone())),
            _ => None,
        });
        assert_eq!(
            tool_step,
            Some(("file.txt".to_string(), "success".to_string()))
        );
    }

    #[test]
    fn parse_omp_timeline_reads_model_change_with_model_field() {
        // OMP's model_change events use the `model` key instead of Pi's
        // `modelId` key for the same event type.
        let reader = make_reader(&[
            r#"{"type":"title","v":1,"title":"","updatedAt":"2024-12-03T14:00:00.000Z"}"#,
            r#"{"type":"session","version":3,"id":"omp-sess-1","timestamp":"2024-12-03T14:00:00.000Z","cwd":"/tmp/omp-project"}"#,
            r#"{"type":"model_change","id":"m0","parentId":null,"timestamp":"2024-12-03T14:00:00.500Z","model":"ollama/glm-5.3-flash:cloud"}"#,
        ]);
        let db_entries = HashMap::new();
        let mut timeline = Vec::new();
        let mut metadata = HashMap::new();

        parse_omp_timeline(reader, &db_entries, &mut timeline, &mut metadata);

        assert_eq!(
            metadata.get("cwd").and_then(|v| v.as_str()),
            Some("/tmp/omp-project")
        );
        let has_model_change = timeline.iter().any(|item| {
            matches!(item, TimelineItem::SystemStatus { status_type, message, .. }
                if status_type == "model_change" && message.contains("ollama/glm-5.3-flash:cloud"))
        });
        assert!(has_model_change);
    }
}

#[cfg(test)]
mod grok_tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn grok_eof_flush_keeps_last_timestamp_and_unknown_model() {
        let path = std::env::temp_dir().join(format!(
            "token_usage_insights_grok_timeline_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(
            &path,
            concat!(
                r#"{"timestamp":1710000000,"params":{"update":{"sessionUpdate":"turn_started","turn_number":0}}}"#, "\n",
                r#"{"timestamp":1710000001,"params":{"update":{"sessionUpdate":"user_message_chunk","content":{"text":"hello"}}}}"#, "\n",
                r#"{"timestamp":1710000002,"params":{"update":{"sessionUpdate":"agent_message_chunk","content":{"text":"reply"}}}}"#, "\n"
            ),
        )
        .unwrap();

        let mut timeline = Vec::new();
        let mut metadata = HashMap::new();
        parse_grok_timeline(
            BufReader::new(File::open(&path).unwrap()),
            &HashMap::new(),
            &mut timeline,
            &mut metadata,
        );

        let reply = timeline
            .iter()
            .find_map(|item| match item {
                TimelineItem::AgentReply {
                    timestamp, model, ..
                } => Some((timestamp, model)),
                _ => None,
            })
            .unwrap();
        assert_eq!(reply.0, "2024-03-09T16:00:02.000Z");
        assert_eq!(reply.1, UNKNOWN_MODEL);

        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod mcode_tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    fn temp_session_dir(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "token-usage-insights-mcode-timeline-test-{}-{}",
            label,
            std::process::id()
        ));
        std::fs::remove_dir_all(&root).ok();
        let dir = root
            .join("sessions")
            .join("2026")
            .join("09")
            .join("19")
            .join("15-16-05-177-session_bXZzXzU5YjdmZjExNWZjOTQwOWFhN2IwMTdhYWMyMDhmYjI4");
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_lines(path: &Path, lines: &[String]) {
        let mut file = File::create(path).unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
    }

    #[test]
    fn parse_mcode_timeline_reconstructs_prompt_reply_and_tool_call() {
        let session_dir = temp_session_dir("basic");
        write_lines(
            &session_dir.join("messages.jsonl"),
            &[
                r#"{"message_id":"msg-user","turn_id":"turn_1","message":{"role":"user","content":[{"type":"text","text":"please run the tests"}],"timestamp":1789826600000}}"#.to_string(),
                r#"{"message_id":"msg-assistant","turn_id":"turn_1","message":{"role":"assistant","content":[{"type":"thinking","thinking":"plan"},{"type":"text","text":"running"},{"type":"toolCall","id":"call_1","name":"bash","arguments":{"command":"cargo test"}}],"model":"deepseek-v4.1-flash","usage":{"input":10,"output":5,"cacheRead":0,"cacheWrite":0,"totalTokens":15,"cost":{"total":0}},"timestamp":1789826601000}}"#.to_string(),
                r#"{"message_id":"msg-tool","turn_id":"turn_1","message":{"role":"toolResult","toolCallId":"call_1","toolName":"bash","content":[{"type":"text","text":"ok"}],"isError":false,"timestamp":1789826602000}}"#.to_string(),
                r#"{"message_id":"msg-compaction","turn_id":"turn_1","message":{"role":"compactionSummary","summary":"compacted","tokensBefore":100,"timestamp":1789826603000}}"#.to_string(),
            ],
        );

        let mut db_entries: HashMap<u32, (TokenStats, String)> = HashMap::new();
        db_entries.insert(
            1,
            (
                crate::mcode::assistant_usage(&serde_json::json!({
                    "role": "assistant",
                    "usage": {"input": 10, "output": 5, "totalTokens": 15}
                }))
                .unwrap(),
                "deepseek-v4.1-flash".to_string(),
            ),
        );
        let mut timeline = Vec::new();
        let mut metadata = HashMap::new();
        parse_mcode_timeline(&session_dir, &db_entries, &mut timeline, &mut metadata);
        std::fs::remove_dir_all(
            session_dir
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .parent()
                .unwrap(),
        )
        .ok();

        assert!(matches!(
            &timeline[0],
            TimelineItem::SystemStatus { status_type, .. } if status_type == "session_start"
        ));
        assert!(matches!(
            &timeline[1],
            TimelineItem::UserPrompt { prompt, turn_no: 1, .. } if prompt == "please run the tests"
        ));
        // The `toolResult` matched `call_1`, so it must update the tool call in
        // place instead of appending a second tool step.
        assert!(matches!(
            &timeline[2],
            TimelineItem::ToolStep { tool_name, stdout, status, exit_code, .. }
                if tool_name == "bash" && stdout == "ok" && status == "success" && *exit_code == Some(0)
        ));
        match &timeline[3] {
            TimelineItem::AgentReply {
                reply,
                reasoning,
                turn_no,
                model,
                tokens,
                ..
            } => {
                assert_eq!(reply, "running");
                assert_eq!(reasoning.as_deref(), Some("plan"));
                assert_eq!(*turn_no, 1);
                assert_eq!(model, "deepseek-v4.1-flash");
                assert_eq!(tokens.as_ref().unwrap().total, 15);
            }
            _ => panic!("expected agent reply after the tool step"),
        }
        assert!(matches!(
            &timeline[4],
            TimelineItem::SystemStatus { status_type, message, .. }
                if status_type == "compaction" && message == "compacted"
        ));
        assert_eq!(timeline.len(), 5);
        assert_eq!(
            metadata.get("selected_model").and_then(Value::as_str),
            Some("deepseek-v4.1-flash")
        );
        assert!(
            !metadata.contains_key("cwd"),
            "cwd must come from the DB, not the transcript"
        );
    }

    #[test]
    fn parse_mcode_timeline_numbers_turns_with_the_usage_predicate() {
        let session_dir = temp_session_dir("turn-numbers");
        write_lines(
            &session_dir.join("messages.jsonl"),
            &[
                r#"{"message_id":"msg-without-usage","turn_id":"turn_1","message":{"role":"assistant","content":[],"model":"deepseek-v4.1-flash","timestamp":1789828000000}}"#.to_string(),
                r#"{"message_id":"msg-first","turn_id":"turn_1","message":{"role":"assistant","content":[{"type":"text","text":"one"}],"model":"deepseek-v4.1-flash","usage":{"input":10,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":11,"cost":{"total":0}},"timestamp":1789828001000}}"#.to_string(),
                r#"{"message_id":"msg-second","turn_id":"turn_2","message":{"role":"assistant","content":[{"type":"text","text":"two"}],"model":"deepseek-v4.1-flash","usage":{"input":20,"output":2,"cacheRead":0,"cacheWrite":0,"totalTokens":22,"cost":{"total":0}},"timestamp":1789828002000}}"#.to_string(),
            ],
        );

        let mut db_entries: HashMap<u32, (TokenStats, String)> = HashMap::new();
        db_entries.insert(
            1,
            (
                crate::mcode::assistant_usage(&serde_json::json!({
                    "role": "assistant",
                    "usage": {"input": 10, "output": 1, "totalTokens": 11}
                }))
                .unwrap(),
                "deepseek-v4.1-flash".to_string(),
            ),
        );
        let mut timeline = Vec::new();
        let mut metadata = HashMap::new();
        parse_mcode_timeline(&session_dir, &db_entries, &mut timeline, &mut metadata);
        std::fs::remove_dir_all(
            session_dir
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .parent()
                .unwrap(),
        )
        .ok();

        let reply_turns = timeline
            .iter()
            .filter_map(|item| match item {
                TimelineItem::AgentReply { turn_no, .. } => Some(*turn_no),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(reply_turns, vec![1, 2]);
        assert_eq!(
            timeline.len(),
            3,
            "the usage-less assistant must not emit a reply or consume a turn"
        );
    }
}
