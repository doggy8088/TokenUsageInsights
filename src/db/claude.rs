use super::*;

pub(super) fn find_claude_session_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(find_claude_session_files(&path));
            } else if path.is_file()
                && path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("jsonl"))
            {
                files.push(path);
            }
        }
    }
    files
}

fn claude_content_to_text(content: &serde_json::Value) -> String {
    if let Some(text) = content.as_str() {
        return text.replace('\r', "").replace('\n', " ");
    }

    let mut parts = Vec::new();
    if let Some(items) = content.as_array() {
        for item in items {
            match item.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                "text" => {
                    if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                        parts.push(text.replace('\r', "").replace('\n', " "));
                    }
                }
                "tool_result" => {
                    if let Some(text) = item.get("content").and_then(|c| c.as_str()) {
                        parts.push(text.replace('\r', "").replace('\n', " "));
                    }
                }
                _ => {}
            }
        }
    }
    parts.join(" ")
}

pub(super) fn parse_claude_session_file(filepath: &Path) -> Result<Vec<UsageEntry>, String> {
    let file = File::open(filepath).map_err(|e| format!("無法開啟檔案: {}", e))?;
    let reader = BufReader::new(file);
    let fallback_session_id = filepath
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("unknown-session")
        .to_string();

    let mut session_name_selector = InitialUserPromptSelector::default();
    let mut session_cwd: Option<String> = None;
    let mut session_version: Option<String> = None;
    let mut seen_response_keys = HashSet::new();
    let mut results = Vec::new();

    for line_res in reader.lines() {
        let line = match line_res {
            Ok(line) => line,
            Err(_) => continue,
        };
        let event: serde_json::Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => continue,
        };

        if session_cwd.is_none() {
            session_cwd = event
                .get("cwd")
                .and_then(|cwd| cwd.as_str())
                .map(|cwd| cwd.to_string());
        }
        if session_version.is_none() {
            session_version = event
                .get("version")
                .and_then(|version| version.as_str())
                .map(|version| version.to_string());
        }

        let message = match event.get("message") {
            Some(message) => message,
            None => continue,
        };
        let role = message
            .get("role")
            .and_then(|role| role.as_str())
            .unwrap_or("");

        if role == "user" {
            if let Some(content) = message.get("content") {
                let has_tool_result = content.as_array().is_some_and(|items| {
                    items.iter().any(|item| {
                        item.get("type").and_then(|item_type| item_type.as_str())
                            == Some("tool_result")
                    })
                });
                if has_tool_result {
                    session_name_selector.observe_non_user_message();
                } else {
                    session_name_selector.observe_user_prompt(&claude_content_to_text(content));
                }
            }
            continue;
        }

        if role != "assistant" {
            continue;
        }
        session_name_selector.observe_non_user_message();

        let usage_value = match message.get("usage") {
            Some(usage) => usage.clone(),
            None => continue,
        };
        let usage = match serde_json::from_value::<ClaudeUsage>(usage_value) {
            Ok(usage) => usage,
            Err(_) => continue,
        };

        let response_key = event
            .get("requestId")
            .and_then(|id| id.as_str())
            .or_else(|| message.get("id").and_then(|id| id.as_str()))
            .or_else(|| event.get("uuid").and_then(|id| id.as_str()))
            .unwrap_or("");
        if response_key.is_empty() || !seen_response_keys.insert(response_key.to_string()) {
            continue;
        }

        let timestamp = event
            .get("timestamp")
            .and_then(|timestamp| timestamp.as_str())
            .unwrap_or("")
            .to_string();
        let session_id = event
            .get("sessionId")
            .and_then(|id| id.as_str())
            .unwrap_or(&fallback_session_id)
            .to_string();
        let cwd = event
            .get("cwd")
            .and_then(|cwd| cwd.as_str())
            .map(|cwd| cwd.to_string())
            .or_else(|| session_cwd.clone());
        let version = event
            .get("version")
            .and_then(|version| version.as_str())
            .map(|version| version.to_string())
            .or_else(|| session_version.clone());
        let model = message
            .get("model")
            .and_then(|model| model.as_str())
            .map(|model| model.to_string());

        let input = usage.input_tokens;
        let cache_read = usage.cache_read_input_tokens;
        let reported_cache_write = usage.cache_creation_input_tokens;
        let explicit_cache_write_5m = usage.cache_creation.ephemeral_5m_input_tokens;
        let cache_write_1h = usage.cache_creation.ephemeral_1h_input_tokens;
        let explicit_cache_write = explicit_cache_write_5m.saturating_add(cache_write_1h);
        let cache_write = reported_cache_write.max(explicit_cache_write);
        let cache_write_5m = explicit_cache_write_5m
            .saturating_add(reported_cache_write.saturating_sub(explicit_cache_write));
        let output = usage.output_tokens;
        let total = input
            .saturating_add(cache_read)
            .saturating_add(cache_write)
            .saturating_add(output);
        let tokens = TokenStats {
            input,
            output,
            cache_read: Some(cache_read),
            cache_write: Some(cache_write),
            cache_write_5m: Some(cache_write_5m),
            cache_write_1h: Some(cache_write_1h),
            reasoning: None,
            total,
        };

        results.push(UsageEntry {
            timestamp,
            session_id,
            session_name: session_name_selector
                .selected_name()
                .map(str::to_string)
                .or_else(|| Some(fallback_session_id.clone())),
            transcript_path: Some(filepath.to_string_lossy().into_owned()),
            cwd,
            version,
            turn_no: (results.len() + 1) as u32,
            model: model.clone(),
            model_id: model,
            tokens: Some(tokens.clone()),
            delta_tokens: Some(tokens),
            context: None,
            cost: None,
            source_kind: None,
            source_dir_key: None,
            parent_session_id: None,
            agent_nickname: None,
            agent_role: None,
            reasoning_effort: None,
        });
    }

    Ok(results)
}
#[cfg(test)]
mod tests {
    use super::*;

    fn temp_jsonl_path(prefix: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{}-{unique}.jsonl", std::process::id()))
    }

    #[test]
    fn parse_claude_session_file_deduplicates_request_usage() {
        let path = temp_jsonl_path("claude-parser");

        let content = r#"{"type":"user","sessionId":"session-1","cwd":"/tmp/project","version":"2.1.201","timestamp":"2026-07-04T19:28:48.190Z","uuid":"u1","message":{"role":"user","content":"Build the report"}}
{"type":"user","sessionId":"session-1","cwd":"/tmp/project","version":"2.1.201","timestamp":"2026-07-04T19:28:49.190Z","uuid":"u2","message":{"role":"user","content":"Use monthly grouping"}}
{"type":"assistant","sessionId":"session-1","cwd":"/tmp/project","version":"2.1.201","timestamp":"2026-07-04T19:28:51.753Z","uuid":"a1","requestId":"req_1","message":{"id":"msg_1","role":"assistant","model":"claude-haiku-4-5-20251001","content":[{"type":"thinking","thinking":"working"}],"usage":{"input_tokens":10,"cache_creation_input_tokens":3,"cache_read_input_tokens":7,"output_tokens":5,"cache_creation":{"ephemeral_5m_input_tokens":1,"ephemeral_1h_input_tokens":2}}}}
{"type":"assistant","sessionId":"session-1","cwd":"/tmp/project","version":"2.1.201","timestamp":"2026-07-04T19:28:51.948Z","uuid":"a2","requestId":"req_1","message":{"id":"msg_1","role":"assistant","model":"claude-haiku-4-5-20251001","content":[{"type":"text","text":"Done"}],"usage":{"input_tokens":10,"cache_creation_input_tokens":3,"cache_read_input_tokens":7,"output_tokens":5,"cache_creation":{"ephemeral_5m_input_tokens":1,"ephemeral_1h_input_tokens":2}}}}
"#;

        fs::write(&path, content).unwrap();
        let entries = parse_claude_session_file(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.session_id, "session-1");
        assert_eq!(entry.session_name.as_deref(), Some("Use monthly grouping"));
        assert_eq!(entry.cwd.as_deref(), Some("/tmp/project"));
        assert_eq!(entry.version.as_deref(), Some("2.1.201"));
        assert_eq!(entry.model.as_deref(), Some("claude-haiku-4-5-20251001"));

        let tokens = entry.tokens.as_ref().unwrap();
        assert_eq!(tokens.input, 10);
        assert_eq!(tokens.cache_write, Some(3));
        assert_eq!(tokens.cache_write_5m, Some(1));
        assert_eq!(tokens.cache_write_1h, Some(2));
        assert_eq!(tokens.cache_read, Some(7));
        assert_eq!(tokens.output, 5);
        assert_eq!(tokens.total, 25);
    }

    #[test]
    fn parse_claude_session_file_defaults_unclassified_cache_writes_to_5m() {
        let path = temp_jsonl_path("claude-cache-default");
        let content = r#"{"type":"assistant","sessionId":"session-cache-default","timestamp":"2026-07-04T19:28:51.753Z","uuid":"a1","requestId":"req_1","message":{"id":"msg_1","role":"assistant","model":"claude-haiku-4-5-20251001","content":[{"type":"text","text":"Done"}],"usage":{"input_tokens":10,"cache_creation_input_tokens":3,"cache_read_input_tokens":7,"output_tokens":5}}}
"#;

        fs::write(&path, content).unwrap();
        let entries = parse_claude_session_file(&path).unwrap();
        let _ = fs::remove_file(&path);

        let tokens = entries[0].tokens.as_ref().unwrap();
        assert_eq!(tokens.input, 10);
        assert_eq!(tokens.cache_write, Some(3));
        assert_eq!(tokens.cache_write_5m, Some(3));
        assert_eq!(tokens.cache_write_1h, Some(0));
        assert_eq!(tokens.total, 25);
    }
}
