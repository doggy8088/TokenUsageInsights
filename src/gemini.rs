use crate::db::{TokenStats, UsageEntry};
use serde_json::Value;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub(crate) const SOURCE_KIND: &str = "gemini-cli";
pub(crate) const UNKNOWN_MODEL: &str = "Unknown Model";

fn value_as_u64(value: Option<&Value>) -> Option<u64> {
    value.and_then(|value| {
        value
            .as_u64()
            .or_else(|| value.as_i64().and_then(|number| u64::try_from(number).ok()))
    })
}

/// Recursively scans for Gemini CLI session files under `<gemini_dir>/tmp/*/chats/*.jsonl`.
/// Also supports subdirectories for subagent sessions (`chats/<parent_uuid>/*.jsonl`),
/// direct `<dir>/chats/`, and `<dir>` itself.
pub(crate) fn find_session_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();

    // 1. Check standard ~/.gemini/tmp/*/chats/
    let tmp_dir = dir.join("tmp");
    if tmp_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&tmp_dir) {
            for entry in entries.flatten() {
                let project_dir = entry.path();
                if project_dir.is_dir() {
                    let chats_dir = project_dir.join("chats");
                    if chats_dir.is_dir() {
                        scan_jsonl_in_dir(&chats_dir, &mut files, 0);
                    }
                }
            }
        }
    }

    // 2. Check direct <dir>/chats/
    let direct_chats = dir.join("chats");
    if direct_chats.is_dir() {
        scan_jsonl_in_dir(&direct_chats, &mut files, 0);
    }

    // 3. Fallback: if dir itself contains session-*.jsonl
    scan_jsonl_in_dir(dir, &mut files, 0);

    files.sort();
    files.dedup();
    files
}

fn scan_jsonl_in_dir(dir: &Path, files: &mut Vec<PathBuf>, depth: usize) {
    if depth > 3 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
            let filename = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if depth == 0 {
                if filename.starts_with("session-") || filename.contains("session") {
                    files.push(path);
                }
            } else {
                files.push(path);
            }
        } else if path.is_dir() {
            scan_jsonl_in_dir(&path, files, depth + 1);
        }
    }
}

/// Attempts to read the original workspace root by checking for `.project_root` up the directory tree.
pub(crate) fn read_project_root(session_path: &Path) -> Option<String> {
    let mut curr = session_path.parent();
    while let Some(dir) = curr {
        let project_root_file = dir.join(".project_root");
        if let Ok(content) = fs::read_to_string(&project_root_file) {
            let trimmed = content.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
        curr = dir.parent();
    }
    None
}

fn parse_token_stats(tokens: &Value) -> Option<TokenStats> {
    let input_raw = value_as_u64(tokens.get("input")).unwrap_or(0);
    let cached = value_as_u64(tokens.get("cached")).unwrap_or(0);
    let output = value_as_u64(tokens.get("output")).unwrap_or(0);
    let reasoning = value_as_u64(tokens.get("thoughts"))
        .or_else(|| value_as_u64(tokens.get("reasoning")))
        .unwrap_or(0);
    let total_raw = value_as_u64(tokens.get("total"));

    if input_raw == 0 && output == 0 && cached == 0 && reasoning == 0 {
        return None;
    }

    // Gemini CLI reports input tokens inclusive of cached tokens.
    // Uncached prompt input is input_raw - cached so pricing does not double-count.
    let non_cached_input = input_raw.saturating_sub(cached);
    let total = total_raw.unwrap_or_else(|| {
        non_cached_input
            .saturating_add(output)
            .saturating_add(cached)
            .saturating_add(reasoning)
    });

    Some(TokenStats {
        input: non_cached_input,
        output,
        cache_read: if cached > 0 { Some(cached) } else { None },
        cache_write: None,
        cache_write_5m: None,
        cache_write_1h: None,
        reasoning: if reasoning > 0 { Some(reasoning) } else { None },
        total,
    })
}

#[derive(Debug, Clone)]
struct GeminiTurnRecord {
    timestamp: String,
    model: String,
    tokens: TokenStats,
}

pub(crate) fn extract_prompt_from_content(content: &Value) -> Option<String> {
    if let Some(text) = content.as_str() {
        let trimmed = text.trim();
        if !trimmed.is_empty()
            && !trimmed.starts_with("<session_context>")
            && !trimmed.starts_with("<hook_context>")
            && !trimmed.starts_with("<loaded_context>")
        {
            return Some(trimmed.to_string());
        }
    }
    if let Some(arr) = content.as_array() {
        for item in arr {
            if let Some(text) = item.get("text").and_then(Value::as_str) {
                let trimmed = text.trim();
                if !trimmed.is_empty()
                    && !trimmed.starts_with("<session_context>")
                    && !trimmed.starts_with("<hook_context>")
                    && !trimmed.starts_with("<loaded_context>")
                {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

pub(crate) fn format_session_prompt_title(prompt: &str) -> String {
    let target = if let Some(start) = prompt.find("<objective>") {
        let after = &prompt[start + 11..];
        if let Some(end) = after.find("</objective>") {
            &after[..end]
        } else {
            after
        }
    } else {
        prompt
    };
    for line in target.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(role) = line.strip_prefix("Role:") {
            let role = role.trim();
            if !role.is_empty() {
                return role.chars().take(100).collect();
            }
        }
        if let Some(task) = line.strip_prefix("Task:") {
            let task = task.trim();
            if !task.is_empty() {
                return task.chars().take(100).collect();
            }
        }
        return line.chars().take(100).collect();
    }
    prompt
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .chars()
        .take(100)
        .collect()
}

pub(crate) fn extract_role_from_prompt(prompt: &str) -> Option<String> {
    for line in prompt.lines() {
        let trimmed = line.trim();
        if let Some(role) = trimmed.strip_prefix("Role:") {
            let role = role.trim();
            if !role.is_empty() {
                return Some(role.chars().take(50).collect());
            }
        }
    }
    None
}

/// Parses a single Gemini CLI `session-*.jsonl` file into a list of [`UsageEntry`] items.
pub(crate) fn parse_session_usage_file(path: &Path) -> Result<Vec<UsageEntry>, String> {
    let file = File::open(path)
        .map_err(|e| format!("無法開啟 Gemini CLI session 檔案 {:?}: {e}", path))?;
    let reader = BufReader::new(file);

    let fallback_id = path
        .file_stem()
        .and_then(|name| name.to_str())
        .map(|name| {
            if let Some(stripped) = name.strip_prefix("session-") {
                stripped.to_string()
            } else {
                name.to_string()
            }
        })
        .unwrap_or_else(|| "unknown".to_string());

    let mut session_id = fallback_id.clone();
    let mut session_name: Option<String> = None;
    let cwd_opt = read_project_root(path);
    let mut subagent_cwd: Option<String> = None;
    let transcript_path = path.to_string_lossy().into_owned();

    // Check directory layout for subagent: chats/<parent_session_id>/<subagent_id>.jsonl
    let mut parent_session_id: Option<String> = None;
    let mut is_subagent = false;
    let mut agent_role: Option<String> = None;

    if let Some(parent_dir) = path.parent() {
        if parent_dir
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            == Some("chats")
        {
            let p_name = parent_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            if !p_name.is_empty() && p_name != "chats" {
                parent_session_id = Some(p_name.to_string());
                is_subagent = true;
            }
        }
    }

    let mut turn_order: Vec<String> = Vec::new();
    let mut turn_map: HashMap<String, GeminiTurnRecord> = HashMap::new();

    let mut process_message = |msg: &Value| {
        let msg_type = msg.get("type").and_then(Value::as_str).unwrap_or("");

        if msg_type == "user" {
            if let Some(content) = msg.get("content") {
                if let Some(prompt) = extract_prompt_from_content(content) {
                    if session_name.is_none() {
                        let title = format_session_prompt_title(&prompt);
                        if !title.is_empty() {
                            session_name = Some(title);
                        }
                    }
                    if agent_role.is_none() {
                        agent_role = extract_role_from_prompt(&prompt);
                    }
                }
            }
        }

        if msg_type == "gemini" {
            let id = msg
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if id.is_empty() {
                return;
            }

            let Some(tokens_val) = msg.get("tokens") else {
                return;
            };
            let Some(stats) = parse_token_stats(tokens_val) else {
                return;
            };

            let timestamp = msg
                .get("timestamp")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let model = msg
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or(UNKNOWN_MODEL)
                .to_string();

            let record = GeminiTurnRecord {
                timestamp,
                model,
                tokens: stats,
            };

            if !turn_map.contains_key(&id) {
                turn_order.push(id.clone());
            }
            turn_map.insert(id, record);
        }
    };

    for line in reader.lines() {
        let Ok(line) = line else {
            continue;
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(obj) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };

        // Extract session ID and kind from header if present
        if let Some(sid) = obj.get("sessionId").and_then(Value::as_str) {
            if !sid.is_empty() {
                session_id = sid.to_string();
            }
        }
        if let Some(kind) = obj.get("kind").and_then(Value::as_str) {
            if kind == "subagent" {
                is_subagent = true;
            }
        }
        if let Some(pid) = obj
            .get("parentSessionId")
            .or_else(|| obj.get("parentId"))
            .and_then(Value::as_str)
        {
            let trimmed_pid = pid.trim();
            if !trimmed_pid.is_empty() {
                parent_session_id = Some(trimmed_pid.to_string());
                is_subagent = true;
            }
        }
        if subagent_cwd.is_none() {
            if let Some(dirs) = obj.get("directories").and_then(Value::as_array) {
                if let Some(d) = dirs.first().and_then(Value::as_str) {
                    if !d.trim().is_empty() {
                        subagent_cwd = Some(d.trim().to_string());
                    }
                }
            }
        }

        // Check for {$set: {messages: [...]}}
        if let Some(set_obj) = obj.get("$set") {
            if let Some(messages) = set_obj.get("messages").and_then(Value::as_array) {
                for msg in messages {
                    process_message(msg);
                }
            }
        }

        // Check top-level message object
        process_message(&obj);
    }

    if turn_order.is_empty() {
        return Ok(Vec::new());
    }

    let effective_cwd = cwd_opt.or(subagent_cwd);
    let (sub_parent_id, agent_nickname, sub_role) = if is_subagent {
        (parent_session_id, Some("Subagent".to_string()), agent_role)
    } else {
        (None, None, None)
    };

    let mut entries = Vec::with_capacity(turn_order.len());
    for (idx, id) in turn_order.into_iter().enumerate() {
        let Some(record) = turn_map.remove(&id) else {
            continue;
        };
        let turn_no = (idx + 1) as u32;
        let ts = if record.timestamp.is_empty() {
            chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        } else {
            record.timestamp
        };

        entries.push(UsageEntry {
            timestamp: ts,
            session_id: session_id.clone(),
            session_name: session_name.clone().or_else(|| Some(session_id.clone())),
            transcript_path: Some(transcript_path.clone()),
            cwd: effective_cwd.clone(),
            version: None,
            turn_no,
            model: Some(record.model.clone()),
            model_id: Some(record.model.clone()),
            tokens: Some(record.tokens.clone()),
            delta_tokens: Some(record.tokens),
            context: None,
            cost: None,
            source_kind: Some(SOURCE_KIND.to_string()),
            source_dir_key: None,
            parent_session_id: sub_parent_id.clone(),
            agent_nickname: agent_nickname.clone(),
            agent_role: sub_role.clone(),
            reasoning_effort: None,
        });
    }

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_test_dir(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "token-usage-insights-gemini-test-{}-{}",
            label,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn find_session_files_finds_tmp_project_chats() {
        let root = temp_test_dir("find_files");
        let project_dir = root.join("tmp").join("proj_abc123");
        let chats_dir = project_dir.join("chats");
        fs::create_dir_all(&chats_dir).unwrap();

        let s1 = chats_dir.join("session-2026-10-13T07-49-65a0cf79.jsonl");
        File::create(&s1).unwrap();

        let files = find_session_files(&root);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0], s1);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn read_project_root_extracts_cwd_from_parent() {
        let root = temp_test_dir("project_root");
        let project_dir = root.join("tmp").join("proj_xyz");
        let chats_dir = project_dir.join("chats");
        fs::create_dir_all(&chats_dir).unwrap();

        let root_file = project_dir.join(".project_root");
        fs::write(&root_file, "/path/to/my/awesome-project\n").unwrap();

        let session_file = chats_dir.join("session-test.jsonl");
        File::create(&session_file).unwrap();

        let cwd = read_project_root(&session_file);
        assert_eq!(cwd, Some("/path/to/my/awesome-project".to_string()));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn parse_session_usage_file_handles_streaming_updates_and_tokens() {
        let root = temp_test_dir("parse_session");
        let chats_dir = root.join("tmp").join("proj_1").join("chats");
        fs::create_dir_all(&chats_dir).unwrap();
        let session_file = chats_dir.join("session-2026-10-13T07-49-test.jsonl");

        let mut file = File::create(&session_file).unwrap();
        // Line 1: Header
        writeln!(
            file,
            r#"{{"sessionId":"sess-uuid-123","projectHash":"hash123","startTime":"2026-10-13T07:49:00.000Z"}}"#
        )
        .unwrap();
        // Line 2: System context user message (should not be session_name)
        writeln!(
            file,
            r#"{{"id":"ctx1","type":"user","content":[{{"text":"<session_context>\nSystem info\n</session_context>"}}]}}"#
        )
        .unwrap();
        // Line 3: Real user question
        writeln!(
            file,
            r#"{{"id":"u1","type":"user","content":[{{"text":"如何計算 Gemini CLI token 用量？"}}]}}"#
        )
        .unwrap();
        // Line 4: Turn 1 initial update
        writeln!(
            file,
            r#"{{"id":"turn1","timestamp":"2026-10-13T07:49:05.000Z","type":"gemini","model":"gemini-3.8-flash","tokens":{{"input":1000,"output":10,"cached":0,"thoughts":50,"total":1060}}}}"#
        )
        .unwrap();
        // Line 5: Turn 1 updated with tool call and higher cached
        writeln!(
            file,
            r#"{{"id":"turn1","timestamp":"2026-10-13T07:49:10.000Z","type":"gemini","model":"gemini-3.8-flash","tokens":{{"input":1200,"output":30,"cached":800,"thoughts":100,"total":1330}}}}"#
        )
        .unwrap();
        // Line 6: Turn 2 user response
        writeln!(
            file,
            r#"{{"id":"u2","type":"user","content":[{{"text":"下一步呢？"}}]}}"#
        )
        .unwrap();
        // Line 7: Turn 2 gemini response
        writeln!(
            file,
            r#"{{"id":"turn2","timestamp":"2026-10-13T07:50:00.000Z","type":"gemini","model":"gemini-3.8-flash","tokens":{{"input":2000,"output":50,"cached":1500,"thoughts":120,"total":2170}}}}"#
        )
        .unwrap();

        let entries = parse_session_usage_file(&session_file).unwrap();
        assert_eq!(entries.len(), 2, "應有 2 個 turn");

        // Turn 1 assertions
        assert_eq!(entries[0].turn_no, 1);
        assert_eq!(entries[0].session_id, "sess-uuid-123");
        assert_eq!(
            entries[0].session_name.as_deref(),
            Some("如何計算 Gemini CLI token 用量？")
        );
        assert_eq!(entries[0].model.as_deref(), Some("gemini-3.8-flash"));
        let t1 = entries[0].tokens.as_ref().unwrap();
        assert_eq!(t1.input, 400); // 1200 - 800 cached
        assert_eq!(t1.cache_read, Some(800));
        assert_eq!(t1.reasoning, Some(100));
        assert_eq!(t1.output, 30);
        assert_eq!(t1.total, 1330);

        // Turn 2 assertions
        assert_eq!(entries[1].turn_no, 2);
        let t2 = entries[1].tokens.as_ref().unwrap();
        assert_eq!(t2.input, 500); // 2000 - 1500 cached
        assert_eq!(t2.cache_read, Some(1500));
        assert_eq!(t2.reasoning, Some(120));
        assert_eq!(t2.output, 50);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn find_session_files_finds_subagent_chats() {
        let root = temp_test_dir("find_subagents");
        let project_dir = root.join("tmp").join("proj_sub");
        let chats_dir = project_dir.join("chats");
        let parent_dir = chats_dir.join("parent-uuid-999");
        fs::create_dir_all(&parent_dir).unwrap();

        let main_file = chats_dir.join("session-2026-10-14T10-00-parent.jsonl");
        let sub_file = parent_dir.join("child-uuid-888.jsonl");
        File::create(&main_file).unwrap();
        File::create(&sub_file).unwrap();

        let files = find_session_files(&root);
        assert_eq!(files.len(), 2);
        assert!(files.contains(&main_file));
        assert!(files.contains(&sub_file));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn parse_session_usage_file_handles_subagent_hierarchy_and_roles() {
        let root = temp_test_dir("subagent_hierarchy");
        let chats_dir = root.join("tmp").join("proj_x").join("chats");
        let parent_dir = chats_dir.join("parent-uuid-999");
        fs::create_dir_all(&parent_dir).unwrap();
        let sub_file = parent_dir.join("child-uuid-888.jsonl");

        let mut file = File::create(&sub_file).unwrap();
        writeln!(
            file,
            r#"{{"sessionId":"child-uuid-888","kind":"subagent","directories":["/tmp/test-project"]}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"id":"ctx","type":"user","content":[{{"text":"<loaded_context>\nIgnored\n</loaded_context>"}},{{"text":"<objective>\nRole: Codebase Debugger\nTask: Find all null offset errors\n</objective>"}}]}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"id":"t1","timestamp":"2026-10-14T10:05:00.000Z","type":"gemini","model":"gemini-3.8-flash","tokens":{{"input":500,"output":20,"cached":100,"thoughts":10,"total":530}}}}"#
        )
        .unwrap();

        let entries = parse_session_usage_file(&sub_file).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_id, "child-uuid-888");
        assert_eq!(
            entries[0].parent_session_id.as_deref(),
            Some("parent-uuid-999")
        );
        assert_eq!(entries[0].agent_nickname.as_deref(), Some("Subagent"));
        assert_eq!(entries[0].agent_role.as_deref(), Some("Codebase Debugger"));
        assert_eq!(
            entries[0].session_name.as_deref(),
            Some("Codebase Debugger")
        );
        assert_eq!(entries[0].cwd.as_deref(), Some("/tmp/test-project"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn parses_current_environment_gemini_session_if_present() {
        if let Some(home) = dirs::home_dir() {
            let gemini_dir = home.join(".gemini");
            let files = find_session_files(&gemini_dir);
            for file in files {
                let entries = parse_session_usage_file(&file);
                assert!(
                    entries.is_ok(),
                    "Failed to parse {:?}: {:?}",
                    file,
                    entries.err()
                );
            }
        }
    }
}
