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
/// Also supports directories that directly contain `chats/*.jsonl` or `*.jsonl`.
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
                        scan_jsonl_in_dir(&chats_dir, &mut files);
                    }
                }
            }
        }
    }

    // 2. Check direct <dir>/chats/
    let direct_chats = dir.join("chats");
    if direct_chats.is_dir() {
        scan_jsonl_in_dir(&direct_chats, &mut files);
    }

    // 3. Fallback: if dir itself contains session-*.jsonl
    scan_jsonl_in_dir(dir, &mut files);

    files.sort();
    files.dedup();
    files
}

fn scan_jsonl_in_dir(dir: &Path, files: &mut Vec<PathBuf>) {
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
            if filename.starts_with("session-") || filename.contains("session") {
                files.push(path);
            }
        }
    }
}

/// Attempts to read the original workspace root from `<project_hash>/.project_root`.
pub(crate) fn read_project_root(session_path: &Path) -> Option<String> {
    // Expected structure: .../<project_hash>/chats/session-*.jsonl
    let chats_dir = session_path.parent()?;
    let project_dir = chats_dir.parent()?;
    let project_root_file = project_dir.join(".project_root");
    if let Ok(content) = fs::read_to_string(&project_root_file) {
        let trimmed = content.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
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
                {
                    return Some(trimmed.to_string());
                }
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
    let transcript_path = path.to_string_lossy().into_owned();

    let mut turn_order: Vec<String> = Vec::new();
    let mut turn_map: HashMap<String, GeminiTurnRecord> = HashMap::new();

    let mut process_message = |msg: &Value| {
        let msg_type = msg.get("type").and_then(Value::as_str).unwrap_or("");

        if msg_type == "user" && session_name.is_none() {
            if let Some(content) = msg.get("content") {
                if let Some(prompt) = extract_prompt_from_content(content) {
                    let first_line = prompt.lines().next().unwrap_or("").trim();
                    if !first_line.is_empty() {
                        session_name = Some(first_line.chars().take(100).collect());
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

        // Extract session ID from header if present
        if let Some(sid) = obj.get("sessionId").and_then(Value::as_str) {
            if !sid.is_empty() {
                session_id = sid.to_string();
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
            cwd: cwd_opt.clone(),
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
            parent_session_id: None,
            agent_nickname: None,
            agent_role: None,
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
