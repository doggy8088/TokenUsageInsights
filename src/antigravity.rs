//! Antigravity CLI's and IDE's native, per-response token records. No statusline required.
use crate::antigravity_metadata::{
    apply_subagent_descriptor_fallback, is_valid_session_id, read_session_metadata, SessionMetadata,
};
use crate::db::{InitialUserPromptSelector, TokenStats, UsageEntry};
use serde_json::Value;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub(crate) const SOURCE_KIND: &str = "antigravity-session";
pub(crate) const IDE_SOURCE_KIND: &str = "antigravity-ide-session";
// Bump when parsed fields change so unchanged historical sources are re-imported.
pub(crate) const PARSER_VERSION: u32 = 4;

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
    let mut files = Vec::new();
    let mut seen_sessions = HashSet::new();

    // 1. Scan brain/*/.system_generated/logs/transcript_full.jsonl (legacy transcripts or test fixtures)
    let brain = root.join("brain");
    if brain.is_dir() {
        if let Ok(entries) = fs::read_dir(&brain) {
            for entry in entries.flatten() {
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    let sess_name = entry.file_name().to_string_lossy().to_string();
                    let path = entry
                        .path()
                        .join(".system_generated/logs/transcript_full.jsonl");
                    if path.is_file() {
                        seen_sessions.insert(sess_name);
                        files.push(path);
                    }
                }
            }
        }
    }

    // 2. Scan conversations/*.db for sessions without transcript_full.jsonl (standard for Antigravity & Antigravity IDE)
    let conv_dir = root.join("conversations");
    if conv_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&conv_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("db") {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        if !seen_sessions.contains(stem) {
                            seen_sessions.insert(stem.to_string());
                            files.push(path);
                        }
                    }
                }
            }
        }
    }

    files.sort();
    Ok(files)
}

pub(crate) fn session_id(path: &Path) -> Option<&str> {
    if path.extension().and_then(|e| e.to_str()) == Some("db") {
        path.file_stem().and_then(|s| s.to_str())
    } else {
        path.ancestors().nth(3)?.file_name()?.to_str()
    }
}

pub(crate) fn session_root(path: &Path) -> Option<&Path> {
    if path.extension().and_then(|e| e.to_str()) == Some("db") {
        path.parent()?.parent()
    } else {
        path.ancestors().nth(5)
    }
}

#[allow(dead_code)]
pub(crate) fn parse_session(path: &Path) -> Result<Vec<UsageEntry>, String> {
    parse_session_with_source_kind(path, SOURCE_KIND)
}

/// Parse Antigravity session from SQLite database or legacy transcript JSONL.
/// When SQLite generations are available, they provide exact token metrics from Google.
pub(crate) fn parse_session_with_source_kind(
    path: &Path,
    source_kind: &str,
) -> Result<Vec<UsageEntry>, String> {
    let session_id = session_id(path).ok_or("無效的 Antigravity transcript 路徑")?;
    let root = session_root(path);
    let mut metadata = root
        .map(|r| read_session_metadata(r, session_id))
        .transpose()
        .map_err(|e| format!("讀取 Antigravity 會話 metadata 失敗: {e}"))?
        .unwrap_or_default();

    // 1. If generations exist in SQLite database and have token usage, use them directly as ground truth
    let has_valid_generations = metadata
        .generations
        .iter()
        .any(|g| g.input_tokens > 0 || g.output_tokens > 0 || g.cache_read_tokens > 0);
    if has_valid_generations {
        let mut entries = Vec::new();
        let (agent_nickname, agent_role) = resolve_subagent_labels(&metadata);
        let session_name = metadata
            .title
            .clone()
            .or_else(|| {
                metadata
                    .initial_prompt
                    .as_deref()
                    .and_then(prompt_to_session_name)
            })
            .or_else(|| agent_role.clone());

        for (turn_idx, gen) in metadata.generations.iter().enumerate() {
            let turn_no = (turn_idx + 1) as u32;

            let mut timestamp = None;
            for step_idx in &gen.step_indices {
                if let Some((ts, _)) = metadata.step_timestamps.get(step_idx) {
                    timestamp = Some(ts.clone());
                    break;
                }
            }
            let timestamp = timestamp.unwrap_or_else(|| {
                fs::metadata(path)
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .map(|t| {
                        let dt: chrono::DateTime<chrono::Utc> = t.into();
                        dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
                    })
                    .unwrap_or_else(|| {
                        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
                    })
            });

            let model = gen
                .model_name
                .clone()
                .or_else(|| gen.model_uid.clone())
                .unwrap_or_else(|| "Unknown".to_string());
            let model_id = gen.model_uid.clone().or_else(|| Some(model.clone()));

            let total = gen
                .input_tokens
                .checked_add(gen.output_tokens)
                .and_then(|v| v.checked_add(gen.cache_read_tokens))
                .filter(|v| *v <= i64::MAX as u64)
                .unwrap_or(gen.input_tokens + gen.cache_read_tokens + gen.output_tokens);

            let tokens = TokenStats {
                input: gen.input_tokens,
                output: gen.output_tokens,
                cache_read: if gen.cache_read_tokens > 0 {
                    Some(gen.cache_read_tokens)
                } else {
                    None
                },
                cache_write: None,
                cache_write_5m: None,
                cache_write_1h: None,
                reasoning: if gen.reasoning_tokens > 0 {
                    Some(gen.reasoning_tokens)
                } else {
                    None
                },
                total,
            };

            entries.push(UsageEntry {
                timestamp,
                session_id: session_id.to_owned(),
                session_name: session_name.clone(),
                transcript_path: Some(path.to_string_lossy().into_owned()),
                cwd: metadata.cwd.clone(),
                version: None,
                turn_no,
                model: Some(model),
                model_id,
                tokens: Some(tokens.clone()),
                delta_tokens: Some(tokens),
                context: None,
                cost: None,
                source_kind: Some(source_kind.to_owned()),
                source_dir_key: None,
                parent_session_id: metadata.parent_session_id.clone(),
                agent_nickname: agent_nickname.clone(),
                agent_role: agent_role.clone(),
                reasoning_effort: None,
            });
        }
        return Ok(entries);
    }

    // 2. If path is a JSONL transcript, or if a corresponding transcript exists in brain/, read lines
    let jsonl_path = if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
        Some(path.to_path_buf())
    } else if let Some(r) = root {
        let candidate = r
            .join("brain")
            .join(session_id)
            .join(".system_generated/logs/transcript_full.jsonl");
        if candidate.is_file() {
            Some(candidate)
        } else {
            None
        }
    } else {
        None
    };

    let Some(jsonl_file) = jsonl_path else {
        return Ok(Vec::new());
    };

    let file = File::open(&jsonl_file).map_err(|e| e.to_string())?;
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
        let event: Value = match serde_json::from_str(&line) {
            Ok(event) => event,
            Err(_) => continue,
        };
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
            session_name: metadata.title.clone(),
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
            source_kind: Some(source_kind.to_owned()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_test_dir(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "token-usage-insights-antigravity-test-{}-{}",
            label,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn parse_session_skips_malformed_json_lines_and_extracts_valid_usage() {
        let root = temp_test_dir("malformed_json");
        let session_dir = root
            .join("brain")
            .join("test-session-1")
            .join(".system_generated/logs");
        fs::create_dir_all(&session_dir).unwrap();
        let transcript = session_dir.join("transcript_full.jsonl");

        let content = concat!(
            "{\"step_index\":0,\"source\":\"USER_EXPLICIT\",\"type\":\"USER_INPUT\",\"created_at\":\"2026-06-29T07:23:22Z\"}\n",
            "{\"step_index\":1,\"source\":\"MODEL\",\"type\":\"PLANNER_RESPONSE\",\"created_at\":\"2026-06-29T07:23:23Z\",\"input_tokens\":100,\"output_tokens\":20,\"cache_read_tokens\":50,\"model\":\"gemini-2.5-pro\"}\n",
            "s been modified to include a line number before every line, in the format: <line_number>: <original_line>\n",
            "{\"step_index\":3,\"source\":\"MODEL\",\"type\":\"PLANNER_RESPONSE\",\"created_at\":\"2026-06-29T07:23:25Z\",\"input_tokens\":200,\"output_tokens\":40,\"cache_read_tokens\":0,\"model\":\"gemini-2.5-pro\"}\n"
        );
        fs::write(&transcript, content).unwrap();

        let entries = parse_session(&transcript)
            .expect("parse_session should succeed despite malformed lines");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].session_id, "test-session-1");
        assert_eq!(entries[0].turn_no, 1);
        assert_eq!(entries[0].tokens.as_ref().unwrap().input, 100);
        assert_eq!(entries[0].tokens.as_ref().unwrap().output, 20);
        assert_eq!(entries[0].tokens.as_ref().unwrap().cache_read, Some(50));
        assert_eq!(entries[0].tokens.as_ref().unwrap().total, 170);

        assert_eq!(entries[1].turn_no, 2);
        assert_eq!(entries[1].tokens.as_ref().unwrap().input, 200);
        assert_eq!(entries[1].tokens.as_ref().unwrap().output, 40);
        assert_eq!(entries[1].tokens.as_ref().unwrap().total, 240);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn parse_session_skips_unclosed_trailing_line() {
        let root = temp_test_dir("unclosed_line");
        let session_dir = root
            .join("brain")
            .join("test-session-2")
            .join(".system_generated/logs");
        fs::create_dir_all(&session_dir).unwrap();
        let transcript = session_dir.join("transcript_full.jsonl");

        let mut file = File::create(&transcript).unwrap();
        writeln!(
            file,
            "{{\"step_index\":1,\"source\":\"MODEL\",\"type\":\"PLANNER_RESPONSE\",\"created_at\":\"2026-06-29T07:23:23Z\",\"input_tokens\":50,\"output_tokens\":10,\"model\":\"gemini-2.5-pro\"}}"
        ).unwrap();
        // Trailing unclosed line without newline
        write!(
            file,
            "{{\"step_index\":2,\"source\":\"MODEL\",\"type\":\"PLANNER_RESPONSE\",\"created_at\":\"2026-06-29T07:23:24Z\",\"input_tokens\":60"
        ).unwrap();

        let entries =
            parse_session(&transcript).expect("parse_session should ignore trailing partial line");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].tokens.as_ref().unwrap().input, 50);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn parse_session_fails_on_invalid_token_values() {
        let root = temp_test_dir("invalid_tokens");
        let session_dir = root
            .join("brain")
            .join("test-session-3")
            .join(".system_generated/logs");
        fs::create_dir_all(&session_dir).unwrap();
        let transcript = session_dir.join("transcript_full.jsonl");

        fs::write(
            &transcript,
            "{\"step_index\":1,\"source\":\"MODEL\",\"type\":\"PLANNER_RESPONSE\",\"created_at\":\"2026-06-29T07:23:23Z\",\"input_tokens\":-50,\"output_tokens\":10}\n",
        ).unwrap();

        let err = parse_session(&transcript).unwrap_err();
        assert!(err.contains("必須是非負整數"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn parse_session_prefers_sqlite_database_when_available() {
        let root = temp_test_dir("sqlite_session");
        let conv_dir = root.join("conversations");
        fs::create_dir_all(&conv_dir).unwrap();
        let db_path = conv_dir.join("test-db-session.db");

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE gen_metadata(idx INTEGER, data BLOB);
             CREATE TABLE steps(idx INTEGER, metadata BLOB);
             CREATE TABLE trajectory_metadata_blob(data BLOB);",
        )
        .unwrap();

        // Helper to encode varint and length-delimited field
        fn encode_varint(mut value: u64) -> Vec<u8> {
            let mut bytes = Vec::new();
            while value >= 128 {
                bytes.push((value as u8 & 127) | 128);
                value >>= 7;
            }
            bytes.push(value as u8);
            bytes
        }
        fn varint_field(num: u64, val: u64) -> Vec<u8> {
            let mut b = encode_varint(num << 3);
            b.extend(encode_varint(val));
            b
        }
        fn field(num: u64, val: &[u8]) -> Vec<u8> {
            let mut b = encode_varint((num << 3) | 2);
            b.extend(encode_varint(val.len() as u64));
            b.extend(val);
            b
        }

        let mut usage_bytes = Vec::new();
        usage_bytes.extend(varint_field(2, 500)); // input
        usage_bytes.extend(varint_field(5, 100)); // cache_read
        usage_bytes.extend(varint_field(3, 80)); // output
        usage_bytes.extend(varint_field(10, 25)); // reasoning

        let mut inner = Vec::new();
        inner.extend(field(19, b"gemini-3.8-flash-high"));
        inner.extend(field(21, b"Gemini 3.8 Flash (High)"));
        inner.extend(field(4, &usage_bytes));

        let mut gen_blob = Vec::new();
        gen_blob.extend(field(4, b"gen-123"));
        let mut packed_steps = Vec::new();
        packed_steps.extend(encode_varint(1));
        gen_blob.extend(field(2, &packed_steps));
        gen_blob.extend(field(1, &inner));

        conn.execute(
            "INSERT INTO gen_metadata VALUES (0, ?1)",
            rusqlite::params![gen_blob],
        )
        .unwrap();

        let mut ts_bytes = Vec::new();
        ts_bytes.extend(varint_field(1, 1782454505)); // 2026-06-26T06:15:05Z
        let step_meta = field(1, &ts_bytes);
        conn.execute(
            "INSERT INTO steps VALUES (1, ?1)",
            rusqlite::params![step_meta],
        )
        .unwrap();

        // Workspace
        let ws_blob = field(7, b"file:///Users/test/workspace");
        conn.execute(
            "INSERT INTO trajectory_metadata_blob VALUES (?1)",
            rusqlite::params![ws_blob],
        )
        .unwrap();

        drop(conn);

        let files = find_session_files(&root).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0], db_path);

        let entries = parse_session(&db_path).expect("parse_session should read sqlite session");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_id, "test-db-session");
        assert_eq!(entries[0].turn_no, 1);
        assert_eq!(entries[0].model.as_deref(), Some("Gemini 3.8 Flash (High)"));
        assert_eq!(entries[0].timestamp, "2026-06-26T06:15:05Z");
        assert_eq!(entries[0].cwd.as_deref(), Some("/Users/test/workspace"));
        assert_eq!(entries[0].source_kind.as_deref(), Some(SOURCE_KIND));

        let stats = entries[0].tokens.as_ref().unwrap();
        assert_eq!(stats.input, 500);
        assert_eq!(stats.cache_read, Some(100));
        assert_eq!(stats.output, 80);
        assert_eq!(stats.reasoning, Some(25));
        assert_eq!(stats.total, 680);

        // Test IDE source kind
        let ide_entries = parse_session_with_source_kind(&db_path, IDE_SOURCE_KIND).unwrap();
        assert_eq!(ide_entries[0].source_kind.as_deref(), Some(IDE_SOURCE_KIND));

        let _ = fs::remove_dir_all(&root);
    }
}
