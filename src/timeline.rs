use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use serde::Serialize;
use crate::db::TokenStats;

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

    for line_res in reader.lines() {
        let line = match line_res { Ok(l) => l, Err(_) => continue };
        let step: serde_json::Value = match serde_json::from_str(&line) { Ok(v) => v, Err(_) => continue };

        let step_type = step.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let timestamp = step.get("timestamp").and_then(|t| t.as_str()).unwrap_or("").to_string();

        match step_type {
            "USER_INPUT" => {
                let content = step.get("content").and_then(|c| c.as_str()).unwrap_or("").to_string();
                let context = step.get("context").cloned();
                timeline.push(TimelineItem::UserPrompt { timestamp, prompt: content, context, turn_no });
            }
            "PLANNER_RESPONSE" => {
                let content = step.get("content").and_then(|c| c.as_str()).unwrap_or("").to_string();
                let reasoning = step.get("reasoning").and_then(|r| r.as_str()).map(|s| s.to_string());
                
                // 讀取該回合在 SQLite 中的增量 token
                let (tokens, model_name) = if let Some((stats, model)) = db_entries.get(&turn_no) {
                    current_model = model.clone();
                    (Some(stats.clone()), current_model.clone())
                } else {
                    (None, current_model.clone())
                };

                timeline.push(TimelineItem::AgentReply {
                    timestamp,
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
            "TOOL_CALL" => {
                let name = step.get("tool_name").and_then(|t| t.as_str()).unwrap_or("unknown").to_string();
                let args = step.get("arguments").cloned().unwrap_or(serde_json::Value::Null);
                let stdout = step.get("stdout").and_then(|o| o.as_str()).unwrap_or("").to_string();
                let stderr = step.get("stderr").and_then(|e| e.as_str()).unwrap_or("").to_string();
                let exit_code = step.get("exit_code").and_then(|ec| ec.as_i64()).map(|v| v as i32);
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
                    status: if exit_code.unwrap_or(0) == 0 { "success".to_string() } else { "failed".to_string() },
                });
            }
            _ => {}
        }
    }
    metadata.insert("selected_model".to_string(), serde_json::Value::String(current_model));
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
        let line = match line_res { Ok(l) => l, Err(_) => continue };
        let event: serde_json::Value = match serde_json::from_str(&line) { Ok(v) => v, Err(_) => continue };

        let event_type = event.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let timestamp = event.get("timestamp").and_then(|t| t.as_str()).unwrap_or("").to_string();
        let payload = event.get("payload");
        let data = event.get("data");

        match event_type {
            // 舊格式
            "session_meta" | "SESSION_STARTED" => {
                let p = payload.or(data);
                if let Some(p) = p {
                    if let Some(v) = p.get("cli_version") { metadata.insert("copilot_version".to_string(), v.clone()); }
                    if let Some(cwd) = p.get("cwd") { metadata.insert("cwd".to_string(), cwd.clone()); }
                    if let Some(git) = p.get("git") {
                        if let Some(branch) = git.get("branch") { metadata.insert("git_branch".to_string(), branch.clone()); }
                        if let Some(repo) = git.get("repository_url") { metadata.insert("repository".to_string(), repo.clone()); }
                    }
                }
                timeline.push(TimelineItem::SystemStatus { timestamp, status_type: "session_start".to_string(), message: "會話開始 (Session Started)".to_string() });
            }
            // 新格式: session.start
            "session.start" => {
                if let Some(p) = data.or(payload) {
                    if let Some(v) = p.get("copilotVersion") { metadata.insert("copilot_version".to_string(), v.clone()); }
                    if let Some(ctx) = p.get("context") {
                        if let Some(cwd) = ctx.get("cwd") { metadata.insert("cwd".to_string(), cwd.clone()); }
                        if let Some(branch) = ctx.get("branch") { metadata.insert("git_branch".to_string(), branch.clone()); }
                        if let Some(repo) = ctx.get("repository") { metadata.insert("repository".to_string(), repo.clone()); }
                    }
                    if let Some(model) = p.get("selectedModel").and_then(|m| m.as_str()) {
                        if model != "auto" { current_model = model.to_string(); }
                    }
                }
                timeline.push(TimelineItem::SystemStatus { timestamp, status_type: "session_start".to_string(), message: "會話開始 (Session Started)".to_string() });
            }
            "user.message" | "USER_PROMPT" => {
                let p = payload.or(data);
                if let Some(p) = p {
                    let content = p.get("content").and_then(|c| c.as_str()).unwrap_or("").to_string();
                    let context = p.get("context").cloned();
                    timeline.push(TimelineItem::UserPrompt { timestamp, prompt: content, context, turn_no: current_turn_no });
                    has_seen_user_prompt = true;
                }
            }
            "assistant.message" | "ASSISTANT_REPLY" => {
                let p = payload.or(data);
                if let Some(p) = p {
                    let content = p.get("content").and_then(|c| c.as_str()).unwrap_or("").to_string();
                    let reasoning = p.get("reasoning").and_then(|r| r.as_str()).map(|s| s.to_string());

                    if let Some(model) = p.get("model").and_then(|m| m.as_str()) {
                        current_model = model.to_string();
                    }

                    // 新格式: toolRequests 陣列，直接推入 ToolStep（由後續 tool.execution_complete 補結果）
                    if let Some(tool_requests) = p.get("toolRequests").and_then(|tr| tr.as_array()) {
                        for req in tool_requests {
                            let call_id = req.get("toolCallId").and_then(|i| i.as_str()).unwrap_or("").to_string();
                            let name = req.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                            let args = req.get("arguments").cloned().unwrap_or(serde_json::Value::Null);
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
                        let (tokens, model_name) = if let Some((stats, model)) = db_entries.get(&current_turn_no) {
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
                    let call_id = p.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
                    let name = p.get("name").and_then(|n| n.as_str()).unwrap_or("unknown").to_string();
                    let args = p.get("arguments").cloned().unwrap_or(serde_json::Value::Null);

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
                    let call_id = p.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
                    let stdout = p.get("stdout").and_then(|o| o.as_str()).unwrap_or("").to_string();
                    let stderr = p.get("stderr").and_then(|e| e.as_str()).unwrap_or("").to_string();
                    let exit_code = p.get("exitCode").or(p.get("exit_code")).and_then(|ec| ec.as_i64()).map(|v| v as i32);

                    if let Some(&idx) = tool_calls_map.get(&call_id) {
                        if let Some(TimelineItem::ToolStep {
                            stdout: target_stdout,
                            stderr: target_stderr,
                            exit_code: target_exit_code,
                            status,
                            ..
                        }) = timeline.get_mut(idx) {
                            *target_stdout = stdout;
                            *target_stderr = stderr;
                            *target_exit_code = exit_code;
                            *status = if exit_code.unwrap_or(0) == 0 { "success".to_string() } else { "failed".to_string() };
                        }
                    }
                }
            }
            // 新格式: tool.execution_start（若 assistant.message toolRequests 已建立此 call_id，跳過）
            "tool.execution_start" => {
                if let Some(p) = data.or(payload) {
                    let call_id = p.get("toolCallId").and_then(|i| i.as_str()).unwrap_or("").to_string();
                    if !tool_calls_map.contains_key(&call_id) {
                        let name = p.get("toolName").and_then(|n| n.as_str()).unwrap_or("").to_string();
                        let args = p.get("arguments").cloned().unwrap_or(serde_json::Value::Null);
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
                    let call_id = p.get("toolCallId").and_then(|i| i.as_str()).unwrap_or("").to_string();
                    let success = p.get("success").and_then(|s| s.as_bool()).unwrap_or(true);
                    // 優先取 detailedContent，其次取 content
                    let stdout = p.get("result")
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
                        }) = timeline.get_mut(idx) {
                            *target_stdout = stdout;
                            *target_exit_code = exit_code;
                            *status = if success { "success".to_string() } else { "failed".to_string() };
                        }
                    }
                }
            }
            "session.shutdown" => {
                timeline.push(TimelineItem::SystemStatus { timestamp, status_type: "session_end".to_string(), message: "會話結束 (Session Ended)".to_string() });
            }
            _ => {}
        }
    }
    metadata.insert("selected_model".to_string(), serde_json::Value::String(current_model));
}

pub fn parse_codex_timeline(
    reader: BufReader<File>,
    db_entries: &HashMap<u32, (TokenStats, String)>,
    timeline: &mut Vec<TimelineItem>,
    metadata: &mut HashMap<String, serde_json::Value>,
) {
    let mut seen_turn_ids = Vec::new();
    let mut active_turn_id: Option<String> = None;
    let mut current_model = "gpt-5.3-Codex".to_string();
    let mut current_effort: Option<String> = None;
    let mut current_context: Option<serde_json::Value> = None;
    let mut tool_calls_map = HashMap::new();

    for line_res in reader.lines() {
        let line = match line_res { Ok(l) => l, Err(_) => continue };
        let event: serde_json::Value = match serde_json::from_str(&line) { Ok(v) => v, Err(_) => continue };

        let event_type = event.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let timestamp = event.get("timestamp").and_then(|t| t.as_str()).unwrap_or("").to_string();
        let payload = event.get("payload");

        let mut turn_id = None;
        if event_type == "turn_context" {
            if let Some(p) = payload { turn_id = p.get("turn_id").and_then(|id| id.as_str()).map(|s| s.to_string()); }
        } else if event_type == "event_msg" {
            if let Some(p) = payload { turn_id = p.get("turn_id").and_then(|id| id.as_str()).map(|s| s.to_string()); }
        } else if event_type == "response_item" {
            if let Some(meta) = event.get("metadata") { turn_id = meta.get("turn_id").and_then(|id| id.as_str()).map(|s| s.to_string()); }
            if turn_id.is_none() {
                turn_id = event.get("internal_chat_message_metadata_passthrough")
                    .and_then(|m| m.get("turn_id"))
                    .and_then(|id| id.as_str())
                    .map(|s| s.to_string());
            }
        }

        if let Some(tid) = turn_id {
            active_turn_id = Some(tid.clone());
            if !seen_turn_ids.contains(&tid) { seen_turn_ids.push(tid); }
        }

        let turn_no = active_turn_id.as_ref()
            .and_then(|tid| seen_turn_ids.iter().position(|id| id == tid))
            .map(|pos| (pos + 1) as u32)
            .unwrap_or(1);

        match event_type {
            "session_meta" => {
                if let Some(p) = payload {
                    if let Some(v) = p.get("cli_version") { metadata.insert("copilot_version".to_string(), v.clone()); }
                    if let Some(cwd) = p.get("cwd") { metadata.insert("cwd".to_string(), cwd.clone()); }
                    if let Some(git) = p.get("git") {
                        if let Some(branch) = git.get("branch") { metadata.insert("git_branch".to_string(), branch.clone()); }
                        if let Some(repo) = git.get("repository_url") { metadata.insert("repository".to_string(), repo.clone()); }
                    }
                    if let Some(nickname) = p.get("agent_nickname").or_else(|| p.get("source").and_then(|s| s.get("subagent")).and_then(|s| s.get("thread_spawn")).and_then(|t| t.get("agent_nickname"))) {
                        metadata.insert("agent_nickname".to_string(), nickname.clone());
                    }
                    if let Some(role) = p.get("agent_role").or_else(|| p.get("source").and_then(|s| s.get("subagent")).and_then(|s| s.get("thread_spawn")).and_then(|t| t.get("agent_role"))) {
                        metadata.insert("agent_role".to_string(), role.clone());
                    }
                }
                timeline.push(TimelineItem::SystemStatus { timestamp, status_type: "session_start".to_string(), message: "會話開始 (Session Started)".to_string() });
            }
            "turn_context" => {
                if let Some(p) = payload {
                    if let Some(m) = p.get("model").and_then(|v| v.as_str()) {
                        current_model = m.to_string();
                    }
                    if let Some(eff) = p.get("effort")
                        .or_else(|| p.get("collaboration_mode").and_then(|cm| cm.get("settings")).and_then(|s| s.get("reasoning_effort")))
                        .and_then(|v| v.as_str()) {
                        current_effort = Some(eff.to_string());
                    }
                    if let Some(ctx) = p.get("context") {
                        current_context = Some(ctx.clone());
                    }
                }
            }
            "compacted" => {
                timeline.push(TimelineItem::SystemStatus { timestamp, status_type: "session_compaction".to_string(), message: "會話狀態壓縮完成 (Session Compaction Completed)".to_string() });
            }
            "event_msg" => {
                if let Some(p) = payload {
                    let sub_type = p.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    match sub_type {
                        "task_started" => {
                            timeline.push(TimelineItem::SystemStatus { timestamp, status_type: "task_started".to_string(), message: "任務開始 (Task Started)".to_string() });
                        }
                        "task_complete" => {
                            timeline.push(TimelineItem::SystemStatus { timestamp, status_type: "task_complete".to_string(), message: "任務完成 (Task Completed)".to_string() });
                        }
                        "turn_aborted" => {
                            timeline.push(TimelineItem::SystemStatus { timestamp, status_type: "turn_aborted".to_string(), message: "會話中斷 (Turn Aborted)".to_string() });
                        }
                        _ => {}
                    }
                }
            }
            "tool_call" => {
                if let Some(p) = payload {
                    let call_id = p.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
                    let name = p.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                    let args = p.get("arguments").cloned().unwrap_or(serde_json::Value::Null);

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
            "tool_response" => {
                if let Some(p) = payload {
                    let call_id = p.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
                    let stdout = p.get("stdout").and_then(|o| o.as_str()).unwrap_or("").to_string();
                    let stderr = p.get("stderr").and_then(|e| e.as_str()).unwrap_or("").to_string();
                    let exit_code = p.get("exitCode").and_then(|ec| ec.as_i64()).map(|v| v as i32);

                    if let Some(&idx) = tool_calls_map.get(&call_id) {
                        if let Some(TimelineItem::ToolStep { stdout: target_stdout, stderr: target_stderr, exit_code: target_exit_code, status, .. }) = timeline.get_mut(idx) {
                            *target_stdout = stdout;
                            *target_stderr = stderr;
                            *target_exit_code = exit_code;
                            *status = if exit_code.unwrap_or(0) == 0 { "success".to_string() } else { "failed".to_string() };
                        }
                    }
                }
            }
            "response_item" => {
                if let Some(p) = payload {
                    let role = p.get("role").and_then(|r| r.as_str());
                    if role == Some("assistant") {
                        let mut reply = p.get("reply").and_then(|r| r.as_str()).unwrap_or("").to_string();
                        if reply.is_empty() {
                            if let Some(content_arr) = p.get("content").and_then(|c| c.as_array()) {
                                for item in content_arr {
                                    if let Some(txt) = item.get("text").and_then(|t| t.as_str()) {
                                        reply.push_str(txt);
                                    }
                                }
                            }
                        }
                        let reasoning = p.get("reasoning").and_then(|r| r.as_str()).map(|s| s.to_string());
                        
                        let (tokens, model_name) = if let Some((stats, model)) = db_entries.get(&turn_no) {
                            current_model = model.clone();
                            (Some(stats.clone()), current_model.clone())
                        } else {
                            (None, current_model.clone())
                        };

                        // Remove existing AgentReply for this turn_no to avoid duplicates
                        timeline.retain(|item| {
                            if let TimelineItem::AgentReply { turn_no: existing_turn_no, .. } = item {
                                *existing_turn_no != turn_no
                            } else {
                                true
                            }
                        });

                        timeline.push(TimelineItem::AgentReply {
                            timestamp,
                            reply,
                            reasoning,
                            turn_no,
                            model: model_name,
                            tokens,
                            duration_ms: None,
                            reasoning_effort: current_effort.clone(),
                        });
                    } else if role == Some("user") {
                        let mut prompt = p.get("prompt").and_then(|r| r.as_str()).unwrap_or("").to_string();
                        if prompt.is_empty() {
                            if let Some(content_arr) = p.get("content").and_then(|c| c.as_array()) {
                                for item in content_arr {
                                    if let Some(txt) = item.get("text").and_then(|t| t.as_str()) {
                                        prompt.push_str(txt);
                                    }
                                }
                            }
                        }
                        let context = p.get("context").cloned().or_else(|| current_context.clone());
                        timeline.push(TimelineItem::UserPrompt {
                            timestamp,
                            prompt,
                            context,
                            turn_no,
                        });
                    }
                }
            }
            _ => {}
        }
    }
    metadata.insert("selected_model".to_string(), serde_json::Value::String(current_model));
    if let Some(eff) = current_effort {
        metadata.insert("reasoning_effort".to_string(), serde_json::Value::String(eff));
    }
}
