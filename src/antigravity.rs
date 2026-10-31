//! Antigravity CLI's native, per-response token records. No statusline required.
use crate::antigravity_metadata::{
    apply_subagent_descriptor_fallback, is_valid_session_id, read_session_metadata, SessionMetadata,
};
use crate::db::{InitialUserPromptSelector, TokenStats, UsageEntry};
use serde_json::Value;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub(crate) const SOURCE_KIND: &str = "antigravity-session";
// Bump when parsed fields change so unchanged historical sources are re-imported.
pub(crate) const PARSER_VERSION: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AntigravitySystemMessage {
    /// Inter-agent message (`invoke_subagent` initial prompt or `send_message`).
    AgentMessage { sender: String, content: String },
    /// System lifecycle notification (`sender=system`) for subagent completion/error/cancellation.
    SystemLifecycle {
        status_type: &'static str,
        message: String,
    },
}

pub(crate) fn extract_user_request_text(content: &str) -> &str {
    if let Some(start_idx) = content.find("<USER_REQUEST>") {
        let actual_start = start_idx + "<USER_REQUEST>".len();
        if let Some(end_idx) = content[actual_start..].find("</USER_REQUEST>") {
            return &content[actual_start..(actual_start + end_idx)];
        }
    }
    content
}

fn first_line_trimmed(text: &str) -> String {
    text.lines().next().unwrap_or("").trim().to_owned()
}

/// Parses an Antigravity `SYSTEM_MESSAGE` step's `content` into either an
/// inter-agent message (`AgentMessage`) or a system subagent lifecycle event
/// (`SystemLifecycle`). Background command task notifications (`sender` containing
/// `/task-`) and unrelated system notices return `None`.
pub fn parse_system_message(raw: &str) -> Option<AntigravitySystemMessage> {
    let trimmed = raw.trim();
    let body = if let Some(idx) = trimmed.find("\n<SYSTEM_MESSAGE>") {
        &trimmed[idx + "\n<SYSTEM_MESSAGE>".len()..]
    } else if let Some(rest) = trimmed.strip_prefix("<SYSTEM_MESSAGE>") {
        rest
    } else {
        trimmed
    };
    let body = body.trim();
    let body = body
        .strip_suffix("</SYSTEM_MESSAGE>")
        .map(str::trim)
        .unwrap_or(body);

    let rest = body.strip_prefix("[Message]")?.trim_start();
    let rest = rest.strip_prefix("timestamp=")?.trim_start();
    let (_timestamp, rest) = rest.split_once(char::is_whitespace)?;
    let rest = rest.trim_start().strip_prefix("sender=")?;
    let (sender, rest) = rest.split_once(char::is_whitespace)?;
    let rest = rest.trim_start().strip_prefix("priority=")?;
    let (_priority, rest) = rest.split_once(char::is_whitespace)?;
    let content = rest.trim_start().strip_prefix("content=")?.trim();

    if sender.contains("/task-") {
        return None;
    }

    if sender == "system" {
        if content.starts_with("Subagent ")
            && (content.contains(" has gone idle") || content.contains(" terminated "))
        {
            let summary = content
                .split_once(". ")
                .map(|(head, _)| format!("{head}."))
                .unwrap_or_else(|| first_line_trimmed(content));
            return Some(AntigravitySystemMessage::SystemLifecycle {
                status_type: "subagent_completed",
                message: format!("子代理完成 (Subagent Completed): {summary}"),
            });
        }
        if content.starts_with("The subagent ") && content.contains(" encountered an error") {
            return Some(AntigravitySystemMessage::SystemLifecycle {
                status_type: "subagent_failed",
                message: format!(
                    "子代理失敗 (Subagent Failed): {}",
                    first_line_trimmed(content)
                ),
            });
        }
        if content.starts_with("Subagent ") && content.contains(" has been explicitly canceled") {
            return Some(AntigravitySystemMessage::SystemLifecycle {
                status_type: "subagent_canceled",
                message: format!(
                    "子代理已取消 (Subagent Canceled): {}",
                    first_line_trimmed(content)
                ),
            });
        }
        if content.starts_with("[Notice]") {
            return Some(AntigravitySystemMessage::SystemLifecycle {
                status_type: "system_notice",
                message: first_line_trimmed(content),
            });
        }
        return None;
    }

    if !is_valid_session_id(sender) || content.is_empty() {
        return None;
    }

    Some(AntigravitySystemMessage::AgentMessage {
        sender: sender.to_owned(),
        content: content.to_owned(),
    })
}

fn prompt_to_session_name(prompt: &str) -> Option<String> {
    let mut selector = InitialUserPromptSelector::default();
    selector.observe_user_prompt(extract_user_request_text(prompt));
    selector.into_name()
}

pub(crate) fn resolve_subagent_labels(
    metadata: &SessionMetadata,
) -> (Option<String>, Option<String>) {
    let role = metadata
        .agent_role
        .clone()
        .or_else(|| metadata.agent_type.clone());
    let nickname = metadata.agent_type.clone().filter(|agent_type| {
        !role
            .as_deref()
            .is_some_and(|r| r.eq_ignore_ascii_case(agent_type))
    });
    (nickname, role)
}

pub(crate) fn find_session_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let brain = root.join("brain");
    if !brain.exists() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    for entry in fs::read_dir(&brain).map_err(|e| format!("讀取 Antigravity brain 失敗: {e}"))?
    {
        let entry = entry.map_err(|e| format!("讀取 Antigravity session 失敗: {e}"))?;
        if !entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            continue;
        }
        let path = entry
            .path()
            .join(".system_generated/logs/transcript_full.jsonl");
        if path.is_file() {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

pub(crate) fn session_id(path: &Path) -> Option<&str> {
    path.ancestors().nth(3)?.file_name()?.to_str()
}

/// Only complete JSONL lines are consumed. A malformed completed record is an
/// error so a partially rebuilt session never replaces previously known usage.
pub(crate) fn parse_session(path: &Path) -> Result<Vec<UsageEntry>, String> {
    let session_id = session_id(path).ok_or("無效的 Antigravity transcript 路徑")?;
    let root = path.ancestors().nth(5);
    let mut metadata = root
        .map(|root| read_session_metadata(root, session_id))
        .transpose()
        .map_err(|e| format!("讀取 Antigravity 會話 metadata 失敗: {e}"))?
        .unwrap_or_default();
    let file = File::open(path).map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(file);
    let mut entries = Vec::new();
    let mut line = String::new();
    let mut turn_no = 0;
    let mut name_selector = InitialUserPromptSelector::default();
    let mut saw_user_input = false;
    let mut inferred_parent_id: Option<String> = None;

    loop {
        line.clear();
        if reader.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            break;
        }
        if !line.ends_with('\n') {
            break;
        }
        if line.trim().is_empty() {
            continue;
        }
        let event: Value =
            serde_json::from_str(&line).map_err(|e| format!("Antigravity JSONL 格式錯誤: {e}"))?;
        let step_type = event.get("type").and_then(Value::as_str);

        if !name_selector.is_complete() {
            match step_type {
                Some("USER_INPUT") => {
                    saw_user_input = true;
                    if let Some(content) = event.get("content").and_then(Value::as_str) {
                        name_selector.observe_user_prompt(extract_user_request_text(content));
                    }
                }
                Some("SYSTEM_MESSAGE") => {
                    if let Some(content) = event.get("content").and_then(Value::as_str) {
                        if let Some(AntigravitySystemMessage::AgentMessage {
                            sender,
                            content: body,
                        }) = parse_system_message(content)
                        {
                            if !saw_user_input
                                && inferred_parent_id.is_none()
                                && sender != session_id
                            {
                                inferred_parent_id = Some(sender);
                            }
                            name_selector.observe_user_prompt(extract_user_request_text(&body));
                        }
                    }
                }
                Some("PLANNER_RESPONSE" | "RUN_COMMAND" | "GREP_SEARCH" | "LIST_DIRECTORY")
                | Some("VIEW_FILE" | "CODE_ACTION" | "GENERIC" | "ERROR_MESSAGE" | "TOOL_CALL") => {
                    name_selector.observe_non_user_message();
                }
                _ => {}
            }
        }

        if step_type != Some("PLANNER_RESPONSE") {
            continue;
        }
        turn_no += 1;
        if event.get("input_tokens").is_none()
            && event.get("output_tokens").is_none()
            && event.get("cache_read_tokens").is_none()
        {
            continue;
        }
        let count = |key: &str| -> Result<u64, String> {
            match event.get(key) {
                None | Some(Value::Null) => Ok(0),
                Some(value) => value
                    .as_u64()
                    .filter(|v| *v <= i64::MAX as u64)
                    .ok_or_else(|| format!("Antigravity {key} 必須是非負整數")),
            }
        };
        // Antigravity stores uncached input separately from cache reads.
        let input = count("input_tokens")?;
        let output = count("output_tokens")?;
        let cache_read = count("cache_read_tokens")?;
        let cache_write = count("cache_write_tokens")?;
        let total = input
            .checked_add(output)
            .and_then(|v| v.checked_add(cache_read))
            .and_then(|v| v.checked_add(cache_write))
            .filter(|v| *v <= i64::MAX as u64)
            .ok_or("Antigravity token 總數超出範圍")?;
        let timestamp = event
            .get("created_at")
            .and_then(Value::as_str)
            .filter(|value| chrono::DateTime::parse_from_rfc3339(value).is_ok())
            .ok_or("Antigravity 回應缺少有效的 created_at")?
            .to_owned();
        let model = event
            .get("model_id")
            .or_else(|| event.get("model"))
            .and_then(Value::as_str)
            .filter(|v| !v.trim().is_empty())
            .map(str::to_owned)
            .or_else(|| {
                event
                    .get("step_index")
                    .and_then(Value::as_u64)
                    .and_then(|index| u32::try_from(index).ok())
                    .and_then(|index| metadata.models.get(&index).cloned())
            });
        let tokens = TokenStats {
            input,
            output,
            cache_read: Some(cache_read),
            cache_write: Some(cache_write),
            cache_write_5m: None,
            cache_write_1h: None,
            reasoning: None,
            total,
        };
        entries.push(UsageEntry {
            timestamp,
            session_id: session_id.to_owned(),
            session_name: None,
            transcript_path: Some(path.to_string_lossy().into_owned()),
            cwd: None,
            version: None,
            turn_no,
            model: model.clone(),
            model_id: model,
            tokens: Some(tokens.clone()),
            delta_tokens: Some(tokens),
            context: None,
            cost: None,
            source_kind: Some(SOURCE_KIND.to_owned()),
            source_dir_key: None,
            parent_session_id: None,
            agent_nickname: None,
            agent_role: None,
            reasoning_effort: None,
        });
    }

    if metadata.parent_session_id.is_none() {
        if let Some(parent_id) = inferred_parent_id {
            metadata.parent_session_id = Some(parent_id);
            if let Some(root) = root {
                apply_subagent_descriptor_fallback(root, session_id, &mut metadata, false);
            }
        }
    }

    let (agent_nickname, agent_role) = resolve_subagent_labels(&metadata);
    let session_name = name_selector
        .into_name()
        .or_else(|| {
            metadata
                .initial_prompt
                .as_deref()
                .and_then(prompt_to_session_name)
        })
        .or_else(|| agent_role.clone());

    for entry in &mut entries {
        entry.session_name = session_name.clone();
        entry.cwd = metadata.cwd.clone();
        entry.parent_session_id = metadata.parent_session_id.clone();
        entry.agent_nickname = agent_nickname.clone();
        entry.agent_role = agent_role.clone();
    }

    Ok(entries)
}
