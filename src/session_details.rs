use axum::http::StatusCode;
use serde::Serialize;
use std::{collections::HashMap, fs::File, io::BufReader, path::Path};

use crate::{
    db::{self, TokenStats},
    handlers::daily::{resolve_session_file_path, SessionFileErrorExt},
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

    let filepath = resolve_session_file_path(
        &resolved_assistant,
        &session_id,
        transcript_path_db.as_deref(),
        &source_kind,
        parent_session_id.as_deref(),
        agent_nickname.as_deref(),
    )?;
    if !filepath.exists() {
        let session_dir_exists = if resolved_assistant == "copilot" {
            let base_dir = if source_kind == "copilot-app" {
                crate::paths::copilot_app_dir()
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
