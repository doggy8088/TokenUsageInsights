use crate::db::{parse_cursor_timestamp, TokenStats};
use crate::grok::{timestamp_to_rfc3339, value_to_text};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};

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

pub fn parse_copilot_timeline(
    reader: BufReader<File>,
    db_entries: &HashMap<u32, (TokenStats, String)>,
    timeline: &mut Vec<TimelineItem>,
    metadata: &mut HashMap<String, serde_json::Value>,
) {
    let mut current_turn_no = 1;
    let mut has_seen_user_prompt = false;
    let mut current_model = "GPT-4o".to_string();
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
                        if model != "auto" {
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
    let mut current_model = "Cursor Agent".to_string();
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
    let mut current_model = "Grok 4.5".to_string();
    let mut user_indices = HashMap::<u32, usize>::new();
    let mut reply_parts = HashMap::<u32, Vec<String>>::new();
    let mut reasoning_parts = HashMap::<u32, Vec<String>>::new();
    let mut tool_indices = HashMap::<String, usize>::new();
    let mut session_started = false;

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

        if let Some(model) = update
            .get("model")
            .or_else(|| update.get("model_id"))
            .or_else(|| update.get("modelId"))
            .and_then(|value| value.as_str())
        {
            current_model = model.to_string();
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
                timestamp: String::new(),
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
