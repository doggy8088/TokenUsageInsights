use super::*;

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct ClaudeUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation: ClaudeCacheCreation,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct ClaudeCacheCreation {
    #[serde(default)]
    ephemeral_5m_input_tokens: u64,
    #[serde(default)]
    ephemeral_1h_input_tokens: u64,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeSubagentMeta {
    #[serde(default)]
    agent_type: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    parent_agent_id: Option<String>,
    #[serde(default)]
    workflow_phase: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct ClaudeSubagentPathInfo {
    is_subagent_path: bool,
    root_session_id: Option<String>,
    workflow_id: Option<String>,
}

fn non_empty_trimmed(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn format_claude_subagent_session_id(agent_id: &str) -> String {
    let trimmed = agent_id.trim();
    if trimmed.starts_with("agent-") {
        trimmed.to_string()
    } else {
        format!("agent-{trimmed}")
    }
}

fn inspect_claude_subagent_path(filepath: &Path) -> ClaudeSubagentPathInfo {
    let components: Vec<String> = filepath
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(os_str) => os_str.to_str().map(str::to_string),
            _ => None,
        })
        .collect();

    let Some(subagents_idx) = components.iter().rposition(|part| part == "subagents") else {
        return ClaudeSubagentPathInfo::default();
    };

    let root_session_id = subagents_idx
        .checked_sub(1)
        .and_then(|idx| components.get(idx))
        .and_then(|part| non_empty_trimmed(Some(part.as_str())));

    // Match `<session>/subagents/workflows/<workflow_id>/agent-<id>.jsonl`
    let workflow_id = if components.get(subagents_idx + 1).map(String::as_str) == Some("workflows")
        && subagents_idx + 3 < components.len()
    {
        non_empty_trimmed(components.get(subagents_idx + 2).map(String::as_str))
    } else {
        None
    };

    ClaudeSubagentPathInfo {
        is_subagent_path: true,
        root_session_id,
        workflow_id,
    }
}

fn load_claude_subagent_meta(filepath: &Path) -> Option<ClaudeSubagentMeta> {
    let meta_path = filepath.with_extension("meta.json");
    let raw = fs::read_to_string(meta_path).ok()?;
    serde_json::from_str::<ClaudeSubagentMeta>(&raw).ok()
}

pub(super) fn find_claude_session_files(dir: &Path) -> Vec<PathBuf> {
    find_jsonl_files(dir)
        .into_iter()
        .filter(|path| path.file_name().and_then(|name| name.to_str()) != Some("journal.jsonl"))
        .collect()
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

fn extract_claude_user_prompt_for_session_name(raw_text: &str) -> Option<String> {
    let trimmed = raw_text.trim();
    if trimmed.is_empty()
        || trimmed.starts_with("<local-command-stdout>")
        || trimmed.starts_with("<local-command-stderr>")
        || trimmed.starts_with("<local-command-caveat>")
    {
        return None;
    }

    if let (Some(start), Some(end)) = (
        trimmed.find("<command-args>"),
        trimmed.find("</command-args>"),
    ) {
        let args_start = start + "<command-args>".len();
        if args_start <= end {
            let args = trimmed[args_start..end].trim();
            if args.is_empty() {
                return None;
            }
            return Some(args.to_string());
        }
    }

    Some(trimmed.to_string())
}

pub(super) fn parse_claude_session_file(filepath: &Path) -> Result<Vec<UsageEntry>, String> {
    let file = File::open(filepath).map_err(|e| format!("無法開啟檔案: {}", e))?;
    let reader = BufReader::new(file);
    let fallback_session_id = filepath
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("unknown-session")
        .to_string();

    let path_info = inspect_claude_subagent_path(filepath);
    let subagent_meta = load_claude_subagent_meta(filepath);

    let mut session_name_selector = InitialUserPromptSelector::default();
    let mut custom_title: Option<String> = None;
    let mut session_cwd: Option<String> = None;
    let mut session_version: Option<String> = None;
    let mut seen_response_indices: HashMap<String, usize> = HashMap::new();
    let mut results: Vec<UsageEntry> = Vec::new();

    for line_res in reader.lines() {
        let line = match line_res {
            Ok(line) => line,
            Err(_) => continue,
        };
        let event: serde_json::Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => continue,
        };

        if event.get("type").and_then(|t| t.as_str()) == Some("custom-title") {
            if let Some(title) =
                non_empty_trimmed(event.get("customTitle").and_then(|t| t.as_str()))
            {
                custom_title = Some(title);
            }
        }

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
            let is_meta = event
                .get("isMeta")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if !is_meta {
                if let Some(content) = message.get("content") {
                    let has_tool_result = content.as_array().is_some_and(|items| {
                        items.iter().any(|item| {
                            item.get("type").and_then(|item_type| item_type.as_str())
                                == Some("tool_result")
                        })
                    });
                    if has_tool_result {
                        session_name_selector.observe_non_user_message();
                    } else if let Some(prompt_text) = extract_claude_user_prompt_for_session_name(
                        &claude_content_to_text(content),
                    ) {
                        session_name_selector.observe_user_prompt(&prompt_text);
                    }
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
        if response_key.is_empty() {
            continue;
        }

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

        let model = message
            .get("model")
            .and_then(|model| model.as_str())
            .map(|model| model.to_string());

        if let Some(&existing_idx) = seen_response_indices.get(response_key) {
            if let Some(existing) = results.get_mut(existing_idx) {
                let existing_total = existing.tokens.as_ref().map_or(0, |t| t.total);
                if tokens.total >= existing_total {
                    existing.tokens = Some(tokens.clone());
                    existing.delta_tokens = Some(tokens);
                    if model.is_some() {
                        existing.model = model.clone();
                        existing.model_id = model;
                    }
                }
            }
            continue;
        }

        let timestamp = event
            .get("timestamp")
            .and_then(|timestamp| timestamp.as_str())
            .unwrap_or("")
            .to_string();
        let event_session_id = non_empty_trimmed(event.get("sessionId").and_then(|id| id.as_str()));
        let event_agent_id = non_empty_trimmed(event.get("agentId").and_then(|id| id.as_str()));
        let is_sidechain = event
            .get("isSidechain")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let is_subagent = path_info.is_subagent_path
            || subagent_meta.is_some()
            || event_agent_id.is_some()
            || is_sidechain;

        let (session_id, parent_session_id, session_name, agent_nickname, agent_role) =
            if is_subagent {
                let subagent_session_id = if fallback_session_id.starts_with("agent-") {
                    fallback_session_id.clone()
                } else if let Some(agent_id) = event_agent_id.as_deref() {
                    format_claude_subagent_session_id(agent_id)
                } else {
                    fallback_session_id.clone()
                };

                let meta_parent_id = subagent_meta
                    .as_ref()
                    .and_then(|meta| non_empty_trimmed(meta.parent_agent_id.as_deref()))
                    .map(|parent_agent_id| format_claude_subagent_session_id(&parent_agent_id));

                let parent_id = meta_parent_id
                    .or_else(|| event_session_id.clone())
                    .or_else(|| path_info.root_session_id.clone())
                    .filter(|parent| parent != &subagent_session_id);

                let meta_description = subagent_meta
                    .as_ref()
                    .and_then(|meta| non_empty_trimmed(meta.description.as_deref()));
                let name = meta_description
                    .or_else(|| session_name_selector.selected_name().map(str::to_string))
                    .or_else(|| Some(subagent_session_id.clone()));

                let nickname = subagent_meta
                    .as_ref()
                    .and_then(|meta| non_empty_trimmed(meta.name.as_deref()))
                    .or_else(|| path_info.workflow_id.clone());

                let role = subagent_meta.as_ref().and_then(|meta| {
                    non_empty_trimmed(meta.workflow_phase.as_deref())
                        .or_else(|| non_empty_trimmed(meta.agent_type.as_deref()))
                });

                (subagent_session_id, parent_id, name, nickname, role)
            } else {
                let main_session_id =
                    event_session_id.unwrap_or_else(|| fallback_session_id.clone());
                let name = session_name_selector
                    .selected_name()
                    .map(str::to_string)
                    .or_else(|| Some(fallback_session_id.clone()));
                (main_session_id, None, name, None, None)
            };

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

        let idx = results.len();
        seen_response_indices.insert(response_key.to_string(), idx);

        results.push(UsageEntry {
            timestamp,
            session_id,
            session_name,
            transcript_path: Some(filepath.to_string_lossy().into_owned()),
            cwd,
            version,
            turn_no: (idx + 1) as u32,
            model: model.clone(),
            model_id: model,
            tokens: Some(tokens.clone()),
            delta_tokens: Some(tokens),
            context: None,
            cost: None,
            source_kind: None,
            source_dir_key: None,
            parent_session_id,
            agent_nickname,
            agent_role,
            reasoning_effort: None,
        });
    }

    if let Some(title) = custom_title {
        for entry in &mut results {
            if entry.parent_session_id.is_none() {
                entry.session_name = Some(title.clone());
            }
        }
    }

    Ok(results)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_claude_session_file_extracts_slash_command_args_and_ignores_local_command_stdout() {
        let path = temp_jsonl_path("claude-slash-prompt");
        let content = concat!(
            "{\"type\":\"user\",\"isMeta\":true,\"sessionId\":\"session-slash\",\"timestamp\":\"2026-10-08T01:00:00Z\",\"message\":{\"role\":\"user\",\"content\":\"<local-command-caveat>Caveat</local-command-caveat>\"}}\n",
            "{\"type\":\"user\",\"sessionId\":\"session-slash\",\"timestamp\":\"2026-10-08T01:00:01Z\",\"message\":{\"role\":\"user\",\"content\":\"<command-name>/model</command-name><command-message>model</command-message><command-args></command-args>\"}}\n",
            "{\"type\":\"user\",\"sessionId\":\"session-slash\",\"timestamp\":\"2026-10-08T01:00:02Z\",\"message\":{\"role\":\"user\",\"content\":\"<local-command-stdout>Set model to Opus</local-command-stdout>\"}}\n",
            "{\"type\":\"user\",\"sessionId\":\"session-slash\",\"timestamp\":\"2026-10-08T01:00:03Z\",\"message\":{\"role\":\"user\",\"content\":\"<command-name>/plan</command-name><command-message>plan</command-message><command-args>整合應收帳款與應付帳款到後台</command-args>\"}}\n",
            "{\"type\":\"user\",\"sessionId\":\"session-slash\",\"timestamp\":\"2026-10-08T01:00:04Z\",\"message\":{\"role\":\"user\",\"content\":\"<local-command-stdout>Enabled plan mode</local-command-stdout>\"}}\n",
            "{\"type\":\"user\",\"isMeta\":true,\"sessionId\":\"session-slash\",\"timestamp\":\"2026-10-08T01:00:05Z\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"Base directory for this skill: /Users/test/.claude/skills/grilling\"}]}}\n",
            "{\"type\":\"assistant\",\"sessionId\":\"session-slash\",\"timestamp\":\"2026-10-08T01:00:06Z\",\"uuid\":\"a1\",\"requestId\":\"req_1\",\"message\":{\"id\":\"msg_1\",\"role\":\"assistant\",\"model\":\"claude-opus-4-6\",\"content\":[{\"type\":\"text\",\"text\":\"OK\"}],\"usage\":{\"input_tokens\":10,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":0,\"output_tokens\":5}}}\n"
        );
        fs::write(&path, content).unwrap();
        let entries = parse_claude_session_file(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].session_name.as_deref(),
            Some("整合應收帳款與應付帳款到後台")
        );
    }

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
{"type":"assistant","sessionId":"session-1","cwd":"/tmp/project","version":"2.1.201","timestamp":"2026-07-04T19:28:51.753Z","uuid":"a1","requestId":"req_1","message":{"id":"msg_1","role":"assistant","model":"claude-haiku-4-5-20251001","content":[{"type":"thinking","thinking":"working"}],"usage":{"input_tokens":10,"cache_creation_input_tokens":3,"cache_read_input_tokens":7,"output_tokens":2,"cache_creation":{"ephemeral_5m_input_tokens":1,"ephemeral_1h_input_tokens":2}}}}
{"type":"assistant","sessionId":"session-1","cwd":"/tmp/project","version":"2.1.201","timestamp":"2026-07-04T19:28:51.948Z","uuid":"a2","requestId":"req_1","message":{"id":"msg_1","role":"assistant","model":"claude-haiku-4-5-20251001","content":[{"type":"text","text":"Done"}],"usage":{"input_tokens":10,"cache_creation_input_tokens":3,"cache_read_input_tokens":7,"output_tokens":5,"cache_creation":{"ephemeral_5m_input_tokens":1,"ephemeral_1h_input_tokens":2}}}}
"#;

        fs::write(&path, content).unwrap();
        let entries = parse_claude_session_file(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.session_id, "session-1");
        assert_eq!(entry.parent_session_id, None);
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

    #[test]
    fn parse_claude_subagent_and_workflow_subagent_preserve_hierarchy_and_metadata() {
        let base_dir = temp_jsonl_path("claude-subagents").with_extension("");
        let session_dir = base_dir
            .join("projects")
            .join("-Users-test-project")
            .join("parent-session-uuid");
        let subagents_dir = session_dir.join("subagents");
        let workflow_dir = subagents_dir.join("workflows").join("wf_12345678-abc");
        fs::create_dir_all(&workflow_dir).unwrap();

        // Direct depth-1 subagent
        let sub1_jsonl = subagents_dir.join("agent-a1111111111111111.jsonl");
        let sub1_meta = subagents_dir.join("agent-a1111111111111111.meta.json");
        fs::write(
            &sub1_meta,
            r#"{"agentType":"Explore","description":"Survey admin test patterns","spawnDepth":1}"#,
        )
        .unwrap();
        fs::write(
            &sub1_jsonl,
            r#"{"type":"user","isSidechain":true,"agentId":"a1111111111111111","sessionId":"parent-session-uuid","cwd":"/tmp/project","version":"2.1.293","timestamp":"2026-10-08T01:00:00.000Z","message":{"role":"user","content":"Long raw prompt"}}
{"type":"assistant","isSidechain":true,"agentId":"a1111111111111111","sessionId":"parent-session-uuid","cwd":"/tmp/project","version":"2.1.293","timestamp":"2026-10-08T01:00:02.000Z","requestId":"req_sub1","message":{"id":"msg_sub1","role":"assistant","model":"claude-sonnet-4-6","content":[{"type":"text","text":"Found patterns"}],"usage":{"input_tokens":100,"output_tokens":50}}}
"#,
        )
        .unwrap();

        // Nested depth-2 subagent with parentAgentId
        let sub2_jsonl = subagents_dir.join("agent-a2222222222222222.jsonl");
        let sub2_meta = subagents_dir.join("agent-a2222222222222222.meta.json");
        fs::write(
            &sub2_meta,
            r#"{"agentType":"general-purpose","description":"Nested quota research","parentAgentId":"a1111111111111111","spawnDepth":2}"#,
        )
        .unwrap();
        fs::write(
            &sub2_jsonl,
            r#"{"type":"assistant","isSidechain":true,"agentId":"a2222222222222222","sessionId":"parent-session-uuid","cwd":"/tmp/project","version":"2.1.293","timestamp":"2026-10-08T01:05:00.000Z","requestId":"req_sub2","message":{"id":"msg_sub2","role":"assistant","model":"claude-haiku-4-5-20251001","content":[{"type":"text","text":"Quota ok"}],"usage":{"input_tokens":40,"output_tokens":20}}}
"#,
        )
        .unwrap();

        // Workflow subagent + journal.jsonl (journal must be ignored by find_claude_session_files)
        let wf_jsonl = workflow_dir.join("agent-a3333333333333333.jsonl");
        let wf_meta = workflow_dir.join("agent-a3333333333333333.meta.json");
        let wf_journal = workflow_dir.join("journal.jsonl");
        fs::write(&wf_journal, r#"{"type":"launched"}"#).unwrap();
        fs::write(
            &wf_meta,
            r#"{"agentType":"workflow-subagent","description":"verify:41:1","workflowPhase":"Verify","spawnDepth":1,"model":"haiku"}"#,
        )
        .unwrap();
        fs::write(
            &wf_jsonl,
            r#"{"type":"user","isSidechain":true,"agentId":"a3333333333333333","sessionId":"parent-session-uuid","cwd":"/tmp/project","version":"2.1.293","timestamp":"2026-10-08T02:00:00.000Z","message":{"role":"user","content":"[Workflow harness — computed task] Long boilerplate"}}
{"type":"assistant","isSidechain":true,"agentId":"a3333333333333333","sessionId":"parent-session-uuid","cwd":"/tmp/project","version":"2.1.293","timestamp":"2026-10-08T02:00:05.000Z","requestId":"req_wf","message":{"id":"msg_wf","role":"assistant","model":"claude-haiku-5-5","content":[{"type":"text","text":"Verified"}],"usage":{"input_tokens":80,"output_tokens":30}}}
"#,
        )
        .unwrap();

        let discovered = find_claude_session_files(&base_dir.join("projects"));
        assert_eq!(discovered.len(), 3);
        assert!(discovered
            .iter()
            .all(|p| p.file_name().and_then(|n| n.to_str()) != Some("journal.jsonl")));

        let entries_sub1 = parse_claude_session_file(&sub1_jsonl).unwrap();
        assert_eq!(entries_sub1.len(), 1);
        assert_eq!(entries_sub1[0].session_id, "agent-a1111111111111111");
        assert_eq!(
            entries_sub1[0].parent_session_id.as_deref(),
            Some("parent-session-uuid")
        );
        assert_eq!(
            entries_sub1[0].session_name.as_deref(),
            Some("Survey admin test patterns")
        );
        assert_eq!(entries_sub1[0].agent_nickname, None);
        assert_eq!(entries_sub1[0].agent_role.as_deref(), Some("Explore"));

        let entries_sub2 = parse_claude_session_file(&sub2_jsonl).unwrap();
        assert_eq!(entries_sub2.len(), 1);
        assert_eq!(entries_sub2[0].session_id, "agent-a2222222222222222");
        assert_eq!(
            entries_sub2[0].parent_session_id.as_deref(),
            Some("agent-a1111111111111111")
        );
        assert_eq!(
            entries_sub2[0].session_name.as_deref(),
            Some("Nested quota research")
        );
        assert_eq!(
            entries_sub2[0].agent_role.as_deref(),
            Some("general-purpose")
        );

        let entries_wf = parse_claude_session_file(&wf_jsonl).unwrap();
        assert_eq!(entries_wf.len(), 1);
        assert_eq!(entries_wf[0].session_id, "agent-a3333333333333333");
        assert_eq!(
            entries_wf[0].parent_session_id.as_deref(),
            Some("parent-session-uuid")
        );
        assert_eq!(entries_wf[0].session_name.as_deref(), Some("verify:41:1"));
        assert_eq!(
            entries_wf[0].agent_nickname.as_deref(),
            Some("wf_12345678-abc")
        );
        assert_eq!(entries_wf[0].agent_role.as_deref(), Some("Verify"));

        let _ = fs::remove_dir_all(&base_dir);
    }
}
