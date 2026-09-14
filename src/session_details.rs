use axum::http::StatusCode;
use serde::Serialize;
use std::{collections::HashMap, fs::File, io::BufReader, path::Path};

use crate::{
    db::{self, TokenStats},
    session_files::{resolve_session_file_path, SessionFileErrorExt, SessionFileResolutionContext},
    timeline::{
        parse_antigravity_timeline, parse_claude_timeline, parse_codex_timeline,
        parse_copilot_timeline_filtered, parse_cursor_timeline, parse_grok_timeline,
        parse_muse_timeline, parse_omp_timeline, parse_pi_timeline, parse_vscode_timeline,
        TimelineItem,
    },
};

type TimelineMetadata = HashMap<String, serde_json::Value>;
type SessionTimelineResult = Result<(Vec<TimelineItem>, TimelineMetadata), (StatusCode, String)>;

pub(crate) struct SessionDetailsError {
    pub status: StatusCode,
    pub payload: serde_json::Value,
}

impl SessionDetailsError {
    fn new(status: StatusCode, error: impl Into<String>) -> Self {
        Self {
            status,
            payload: serde_json::json!({ "error": error.into() }),
        }
    }

    fn with_reason(
        status: StatusCode,
        error: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            status,
            payload: serde_json::json!({
                "error": error.into(),
                "reason": reason.into(),
            }),
        }
    }
}

impl From<SessionFileErrorExt> for SessionDetailsError {
    fn from(error: SessionFileErrorExt) -> Self {
        match error.reason {
            Some(reason) => Self::with_reason(error.status, error.error, reason),
            None => Self::new(error.status, error.error),
        }
    }
}

pub(crate) fn parse_session_timeline_file(
    assistant: &str,
    source_kind: &str,
    filepath: &Path,
    db_entries: &HashMap<u32, (TokenStats, String)>,
    copilot_agent_filter: Option<&str>,
    copilot_session_model: Option<&str>,
) -> SessionTimelineResult {
    let mut timeline = Vec::new();
    let mut metadata = HashMap::new();

    if source_kind == crate::vscode::SOURCE_KIND {
        let session = crate::vscode::read_session_file(filepath)
            .map_err(|error| (StatusCode::BAD_REQUEST, error))?;
        parse_vscode_timeline(&session, db_entries, &mut timeline, &mut metadata);
        return Ok((timeline, metadata));
    }

    let file = File::open(filepath).map_err(|error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("開啟日誌檔案失敗: {error}"),
        )
    })?;
    let reader = BufReader::new(file);
    match assistant {
        "antigravity" => {
            parse_antigravity_timeline(reader, db_entries, &mut timeline, &mut metadata)
        }
        "copilot" => parse_copilot_timeline_filtered(
            reader,
            db_entries,
            &mut timeline,
            &mut metadata,
            copilot_agent_filter,
            copilot_session_model,
        ),
        "codex" => parse_codex_timeline(reader, db_entries, &mut timeline, &mut metadata),
        "claude" => parse_claude_timeline(reader, db_entries, &mut timeline, &mut metadata),
        "cursor" => parse_cursor_timeline(reader, db_entries, &mut timeline, &mut metadata),
        "grok" => parse_grok_timeline(reader, db_entries, &mut timeline, &mut metadata),
        "pi" => parse_pi_timeline(reader, db_entries, &mut timeline, &mut metadata),
        "omp" => parse_omp_timeline(reader, db_entries, &mut timeline, &mut metadata),
        "muse" => parse_muse_timeline(reader, db_entries, &mut timeline, &mut metadata),
        _ => return Err((StatusCode::BAD_REQUEST, "不支援的助理類型".to_string())),
    }

    Ok((timeline, metadata))
}

fn get_git_info(cwd: &str) -> (Option<String>, Option<String>) {
    let path = Path::new(cwd);
    if !path.exists() {
        return (None, None);
    }

    let command_output = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(path)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
    };
    (
        command_output(&["symbolic-ref", "--short", "HEAD"]),
        command_output(&["config", "--get", "remote.origin.url"]),
    )
}

#[derive(Serialize)]
struct LegacyEventWrapper {
    event_type: String,
    event_data: serde_json::Value,
}

fn legacy_timeline(timeline: Vec<TimelineItem>) -> Vec<LegacyEventWrapper> {
    timeline
        .into_iter()
        .map(|item| match item {
            TimelineItem::UserPrompt {
                timestamp,
                prompt,
                context,
                turn_no,
            } => {
                let attachments = context
                    .as_ref()
                    .and_then(|value| value.get("attachments"))
                    .and_then(serde_json::Value::as_array)
                    .cloned()
                    .unwrap_or_default();
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
            TimelineItem::AgentReply {
                timestamp,
                reply,
                reasoning,
                turn_no,
                model,
                tokens,
                duration_ms: _,
                reasoning_effort,
            } => {
                let reply = match reasoning {
                    Some(reasoning) => format!(
                        "<details><summary>🧠 LLM Reasoning Process</summary>\n{reasoning}\n</details>\n\n{reply}"
                    ),
                    None => reply,
                };
                LegacyEventWrapper {
                    event_type: "AssistantReply".to_string(),
                    event_data: serde_json::json!({
                        "timestamp": timestamp,
                        "reply": reply,
                        "model": model,
                        "reasoning_effort": reasoning_effort,
                        "input_tokens": tokens.as_ref().map(|value| value.input),
                        "output_tokens": tokens.as_ref().map(|value| value.output),
                        "cache_read_tokens": tokens.as_ref().and_then(|value| value.cache_read),
                        "cache_write_tokens": tokens.as_ref().and_then(|value| value.cache_write),
                        "reasoning_tokens": tokens.as_ref().and_then(|value| value.reasoning),
                        "total_tokens": tokens.as_ref().map(|value| value.total),
                        "tool_requests": Vec::<serde_json::Value>::new(),
                        "turn_no": turn_no,
                    }),
                }
            }
            TimelineItem::ToolStep {
                timestamp,
                tool_name,
                arguments,
                env: _,
                exit_code,
                stdout,
                stderr,
                tool_call_id: _,
                status,
            } => {
                let content = if stderr.is_empty() {
                    stdout
                } else {
                    format!("Stdout:\n{stdout}\n\nStderr:\n{stderr}")
                };
                LegacyEventWrapper {
                    event_type: "ToolStep".to_string(),
                    event_data: serde_json::json!({
                        "timestamp": timestamp,
                        "tool_name": tool_name,
                        "arguments": arguments,
                        "result": matches!(status.as_str(), "success" | "failed").then(|| {
                            serde_json::json!({
                                "content": content,
                                "exitCode": exit_code,
                            })
                        }),
                        "turn_no": 1,
                    }),
                }
            }
            TimelineItem::SystemStatus {
                timestamp,
                status_type,
                message,
            } => LegacyEventWrapper {
                event_type: "SystemStatus".to_string(),
                event_data: serde_json::json!({
                    "timestamp": timestamp,
                    "status_type": status_type,
                    "message": message,
                }),
            },
        })
        .collect()
}

pub(crate) fn load_session_details(
    assistant: String,
    session_id: String,
    requested_source_kind: Option<String>,
    source_dir_key: Option<String>,
) -> Result<serde_json::Value, SessionDetailsError> {
    let conn = db::get_db_conn()
        .map_err(|error| SessionDetailsError::new(StatusCode::INTERNAL_SERVER_ERROR, error))?;
    let info = db::get_session_assistant_and_transcript(
        &conn,
        &assistant,
        &session_id,
        requested_source_kind.as_deref(),
        source_dir_key.as_deref(),
    )
    .map_err(|error| SessionDetailsError::new(StatusCode::NOT_FOUND, error))?;
    let (
        resolved_assistant,
        transcript_path_db,
        source_kind,
        source_dir_key,
        parent_session_id,
        agent_nickname,
    ) = info;

    if resolved_assistant != assistant {
        return Err(SessionDetailsError::new(
            StatusCode::NOT_FOUND,
            "找不到該會話資料或助理類型不符",
        ));
    }

    let copilot_app_source_dir = if source_kind == "copilot-app" {
        let key = source_dir_key.as_deref().ok_or_else(|| {
            SessionDetailsError::with_reason(
                StatusCode::NOT_FOUND,
                "Copilot App session 缺少來源目錄識別。",
                "file_missing",
            )
        })?;
        Some(
            db::get_usage_source_directory(&conn, &resolved_assistant, &source_kind, key)
                .map_err(|error| {
                    SessionDetailsError::new(StatusCode::INTERNAL_SERVER_ERROR, error)
                })?
                .ok_or_else(|| {
                    SessionDetailsError::with_reason(
                        StatusCode::NOT_FOUND,
                        "找不到 Copilot App session 對應的已登錄來源目錄。",
                        "file_missing",
                    )
                })?,
        )
    } else {
        None
    };

    let filepath = resolve_session_file_path(
        &resolved_assistant,
        &session_id,
        transcript_path_db.as_deref(),
        &source_kind,
        SessionFileResolutionContext {
            copilot_app_source_dir: copilot_app_source_dir.as_deref(),
            parent_session_id: parent_session_id.as_deref(),
            agent_nickname: agent_nickname.as_deref(),
        },
    )?;
    if !filepath.exists() {
        let session_dir_exists = if resolved_assistant == "copilot" {
            let base_dir = if source_kind == "copilot-app" {
                copilot_app_source_dir
                    .clone()
                    .unwrap_or_else(crate::paths::copilot_app_dir)
            } else {
                db::get_copilot_dir()
            };
            let directory_id = if matches!(source_kind.as_str(), "copilot-app" | "copilot-cli") {
                parent_session_id.as_deref().unwrap_or(&session_id)
            } else {
                &session_id
            };
            base_dir.join("session-state").join(directory_id).exists()
        } else {
            false
        };
        return Err(SessionDetailsError::with_reason(
            StatusCode::NOT_FOUND,
            "找不到該會話的本地日誌檔。",
            if session_dir_exists {
                "no_events_yet"
            } else {
                "file_missing"
            },
        ));
    }

    let session_cwd = db::get_session_cwd(
        &conn,
        &resolved_assistant,
        &session_id,
        Some(&source_kind),
        source_dir_key.as_deref(),
    )
    .unwrap_or(None);
    let session_model = db::get_session_model(
        &conn,
        &resolved_assistant,
        &session_id,
        Some(&source_kind),
        source_dir_key.as_deref(),
    )
    .unwrap_or(None);
    let db_entries = db::get_session_turns_token_stats(
        &conn,
        &resolved_assistant,
        &session_id,
        Some(&source_kind),
        source_dir_key.as_deref(),
    )
    .unwrap_or_default();

    let agent_filter = (resolved_assistant == "copilot"
        && matches!(source_kind.as_str(), "copilot-app" | "copilot-cli"))
    .then_some(agent_nickname.as_deref())
    .flatten();
    let (timeline, mut metadata) = parse_session_timeline_file(
        &resolved_assistant,
        &source_kind,
        &filepath,
        &db_entries,
        agent_filter,
        session_model.as_deref(),
    )
    .map_err(|(status, error)| SessionDetailsError::new(status, error))?;

    if agent_filter.is_some()
        && !timeline.iter().any(|item| match item {
            TimelineItem::AgentReply { .. } | TimelineItem::ToolStep { .. } => true,
            TimelineItem::SystemStatus { status_type, .. } => matches!(
                status_type.as_str(),
                "subagent_started" | "subagent_completed" | "subagent_failed"
            ),
            TimelineItem::UserPrompt { .. } => false,
        })
    {
        return Err(SessionDetailsError::with_reason(
            StatusCode::NOT_FOUND,
            "Copilot subagent 的 events.jsonl 中找不到對應 agentId 的事件，可能該 subagent 尚未寫入事件或檔案已被置換。",
            "content_unavailable",
        ));
    }

    if let Some(cwd) = session_cwd {
        metadata
            .entry("cwd".to_string())
            .or_insert_with(|| serde_json::Value::String(cwd.clone()));
        let (branch, repository) = get_git_info(&cwd);
        if let Some(branch) = branch {
            metadata
                .entry("git_branch".to_string())
                .or_insert_with(|| serde_json::Value::String(branch));
        }
        if let Some(repository) = repository {
            metadata
                .entry("repository".to_string())
                .or_insert_with(|| serde_json::Value::String(repository));
        }
    }

    let mut total_tokens = 0;
    let mut total_input_tokens = 0;
    let mut total_output_tokens = 0;
    let mut total_cache_read_tokens = 0;
    let mut total_reasoning_tokens = 0;
    for (tokens, _) in db_entries.values() {
        total_tokens += tokens.total;
        total_input_tokens += tokens.input;
        total_output_tokens += tokens.output;
        total_cache_read_tokens += tokens.cache_read.unwrap_or(0);
        total_reasoning_tokens += tokens.reasoning.unwrap_or(0);
    }
    metadata.insert("total_tokens".to_string(), total_tokens.into());
    metadata.insert("total_input_tokens".to_string(), total_input_tokens.into());
    metadata.insert(
        "total_output_tokens".to_string(),
        total_output_tokens.into(),
    );
    metadata.insert(
        "total_cache_read_tokens".to_string(),
        total_cache_read_tokens.into(),
    );
    metadata.insert(
        "total_reasoning_tokens".to_string(),
        total_reasoning_tokens.into(),
    );

    Ok(serde_json::json!({
        "session_id": session_id,
        "metadata": metadata,
        "timeline": legacy_timeline(timeline),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_files::resolve_copilot_cli_subagent_events_path;
    use std::{collections::HashMap, fs};

    /// Regression test: a Copilot CLI subagent synthetic session row must
    /// resolve its drawer events.jsonl via the parent session's directory
    /// (not the synthetic id's), and the agent filter must keep only that
    /// subagent's events while preserving shared context.
    #[test]
    fn cli_subagent_drawer_resolves_via_parent_and_filters_by_agent_id() {
        let tmp = std::env::temp_dir().join(format!(
            "cli-drawer-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&tmp).unwrap();
        let parent = "drawer-parent-session";
        let agent = "call_drawer";
        let synthetic = format!("{parent}__{agent}");
        let session_dir = tmp.join("session-state").join(parent);
        fs::create_dir_all(&session_dir).unwrap();
        // Write events: shared context (no agentId), main agent reply (no
        // agentId), and the subagent's reply + tool call (tagged with agentId).
        let events = vec![
            serde_json::json!({
                "type": "session.start",
                "timestamp": "2026-07-22T10:00:00Z",
                "data": { "copilotVersion": "1.0.0", "context": { "cwd": "/tmp" } }
            }),
            serde_json::json!({
                "type": "user.message",
                "timestamp": "2026-07-22T10:00:05Z",
                "payload": { "content": "please run the subagent" }
            }),
            serde_json::json!({
                "type": "assistant.message",
                "timestamp": "2026-07-22T10:00:10Z",
                "payload": { "content": "main agent reply" }
            }),
            serde_json::json!({
                "type": "assistant.message",
                "timestamp": "2026-07-22T10:00:20Z",
                "agentId": agent,
                "payload": { "content": "subagent reply" }
            }),
            serde_json::json!({
                "type": "tool.execution_complete",
                "timestamp": "2026-07-22T10:00:25Z",
                "agentId": agent,
                "payload": { "callId": "tool-1" }
            }),
        ];
        let mut file_content = String::new();
        for ev in &events {
            file_content.push_str(&ev.to_string());
            file_content.push('\n');
        }
        fs::write(session_dir.join("events.jsonl"), file_content).unwrap();

        // Resolve the CLI subagent path directly against the temp copilot dir:
        // must point at the parent's events.jsonl, not the synthetic id's.
        let resolved = resolve_copilot_cli_subagent_events_path(&tmp, parent).unwrap();
        assert!(
            resolved.to_string_lossy().ends_with("events.jsonl"),
            "resolved path must end with events.jsonl: {:?}",
            resolved
        );
        assert!(
            resolved.to_string_lossy().contains(parent),
            "resolved path must be under the parent session dir: {:?}",
            resolved
        );
        assert!(
            !resolved.to_string_lossy().contains(&synthetic),
            "must NOT resolve under the synthetic id dir: {:?}",
            resolved
        );

        // Parse with the agent filter (simulating get_session_details'
        // copilot_agent_filter decision for source_kind = "copilot-cli").
        let file = std::fs::File::open(&resolved).unwrap();
        let reader = std::io::BufReader::new(file);
        let db_entries: HashMap<u32, (crate::db::TokenStats, String)> = HashMap::new();
        let mut timeline = Vec::new();
        let mut metadata = HashMap::new();
        crate::timeline::parse_copilot_timeline_filtered(
            reader,
            &db_entries,
            &mut timeline,
            &mut metadata,
            Some(agent),
            None,
        );

        // The subagent view must include shared context (session start, user
        // prompt) and the subagent's own reply + tool call, but NOT the main
        // agent's reply.
        let has_main_reply = timeline.iter().any(|item| match item {
            TimelineItem::AgentReply { reply, .. } => reply.contains("main agent reply"),
            _ => false,
        });
        assert!(
            !has_main_reply,
            "main agent reply must be filtered out of subagent view"
        );

        let has_subagent_reply = timeline.iter().any(|item| match item {
            TimelineItem::AgentReply { reply, .. } => reply.contains("subagent reply"),
            _ => false,
        });
        assert!(
            has_subagent_reply,
            "subagent reply must appear in its own view"
        );

        // Shared context preserved for readability.
        let has_user_prompt = timeline
            .iter()
            .any(|item| matches!(item, TimelineItem::UserPrompt { .. }));
        assert!(
            has_user_prompt,
            "shared user prompt must remain visible to subagent"
        );

        let _ = fs::remove_dir_all(tmp);
    }

    /// Regression: `parse_session_timeline_file` must thread the DB-sourced
    /// child session model into the Copilot timeline parser so a subagent
    /// drawer shows the child model, not the shared parent
    /// `session.start.selectedModel`. Covers both `copilot-app` and
    /// `copilot-cli` source kinds (they share `parse_copilot_timeline_filtered`).
    #[test]
    fn parse_session_timeline_file_threads_child_model_for_subagent_drawer() {
        let tmp = std::env::temp_dir().join(format!(
            "drawer-child-model-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&tmp).unwrap();
        let parent = "child-model-parent";
        let agent = "call_child_model";
        let session_dir = tmp.join("session-state").join(parent);
        fs::create_dir_all(&session_dir).unwrap();
        // Parent session.start carries GLM5.2-none, but the child DB model is
        // gpt-5.4-mini. The subagent drawer must show gpt-5.4-mini.
        let events = vec![
            serde_json::json!({
                "type": "session.start",
                "timestamp": "2026-07-22T10:00:00Z",
                "data": {
                    "copilotVersion": "1.0.0",
                    "context": { "cwd": "/tmp" },
                    "selectedModel": "GLM5.2-none"
                }
            }),
            serde_json::json!({
                "type": "user.message",
                "timestamp": "2026-07-22T10:00:01Z",
                "payload": { "content": "please run the subagent" }
            }),
            serde_json::json!({
                "type": "assistant.message",
                "timestamp": "2026-07-22T10:00:02Z",
                "payload": { "content": "main agent reply" }
            }),
            serde_json::json!({
                "type": "subagent.started",
                "timestamp": "2026-07-22T10:00:05Z",
                "agentId": agent,
                "data": { "agentDisplayName": "GPT", "agentName": "GPT" }
            }),
            serde_json::json!({
                "type": "assistant.message",
                "timestamp": "2026-07-22T10:00:06Z",
                "agentId": agent,
                "payload": { "content": "subagent reply" }
            }),
            serde_json::json!({
                "type": "subagent.completed",
                "timestamp": "2026-07-22T10:00:07Z",
                "agentId": agent
            }),
        ];
        let mut file_content = String::new();
        for ev in &events {
            file_content.push_str(&ev.to_string());
            file_content.push('\n');
        }
        fs::write(session_dir.join("events.jsonl"), file_content).unwrap();

        let resolved = resolve_copilot_cli_subagent_events_path(&tmp, parent).unwrap();
        let db_entries: HashMap<u32, (crate::db::TokenStats, String)> = HashMap::new();
        let (timeline, metadata) = parse_session_timeline_file(
            "copilot",
            "copilot-cli",
            &resolved,
            &db_entries,
            Some(agent),
            Some("gpt-5.4-mini"),
        )
        .unwrap();

        let selected_model = metadata
            .get("selected_model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        assert_eq!(
            selected_model.as_deref(),
            Some("gpt-5.4-mini"),
            "subagent drawer metadata.selected_model must be the child DB model"
        );

        let reply_models: Vec<(String, String)> = timeline
            .iter()
            .filter_map(|item| match item {
                TimelineItem::AgentReply { model, reply, .. } => {
                    Some((model.clone(), reply.clone()))
                }
                _ => None,
            })
            .collect();
        assert!(
            reply_models.iter().all(|(m, _)| m == "gpt-5.4-mini"),
            "every subagent AgentReply.model must be gpt-5.4-mini, got {:?}",
            reply_models
        );
        assert!(
            !reply_models.iter().any(|(m, _)| m == "GLM5.2-none"),
            "GLM5.2-none must not appear in subagent AgentReply models: {:?}",
            reply_models
        );
        // The main agent reply must be filtered out of the subagent view.
        let replies: Vec<String> = reply_models.into_iter().map(|(_, r)| r).collect();
        assert!(
            !replies.iter().any(|r| r == "main agent reply"),
            "main agent reply must not leak into the subagent drawer"
        );

        let _ = fs::remove_dir_all(tmp);
    }
}
