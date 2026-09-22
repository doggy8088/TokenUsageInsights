use super::*;

// Shared by the Codex CLI and Desktop transcript formats.
#[derive(Debug, Clone, Default, serde::Deserialize)]
struct CodexTokenUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    cached_input_tokens: u64,
    #[serde(default)]
    cache_write_input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    reasoning_output_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
}

pub(super) fn find_codex_session_files(dir: &Path) -> Vec<PathBuf> {
    find_jsonl_files(dir)
}

fn codex_content_to_text(content: &serde_json::Value) -> String {
    if let Some(text) = content.as_str() {
        return text.replace('\r', "").replace('\n', " ");
    }

    let mut parts = Vec::new();
    if let Some(items) = content.as_array() {
        for item in items {
            match item.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                "input_text" | "output_text" | "text" => {
                    if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                        parts.push(text.replace('\r', "").replace('\n', " "));
                    }
                }
                _ => {}
            }
        }
    }
    parts.join(" ")
}

fn codex_source_kind_from_metadata(payload: &serde_json::Value) -> &'static str {
    let originator = payload
        .get("originator")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    if originator.contains("desktop") {
        return CODEX_DESKTOP_SOURCE_KIND;
    }
    if matches!(
        originator.as_str(),
        "codex-tui" | "codex_cli_rs" | "codex_exec"
    ) {
        return CODEX_CLI_SOURCE_KIND;
    }

    match payload.get("source").and_then(|value| value.as_str()) {
        Some("cli" | "exec") => CODEX_CLI_SOURCE_KIND,
        _ => CODEX_OTHER_SOURCE_KIND,
    }
}

fn codex_usage_to_stats(usage: CodexTokenUsage) -> TokenStats {
    let cache_read = usage.cached_input_tokens;
    let cache_write = usage.cache_write_input_tokens;
    let input = usage.input_tokens.saturating_sub(cache_read);
    let output = usage.output_tokens;
    let total = if usage.total_tokens > 0 {
        usage.total_tokens
    } else {
        input.saturating_add(cache_read).saturating_add(output)
    };

    TokenStats {
        input,
        output,
        cache_read: Some(cache_read),
        cache_write: Some(cache_write),
        cache_write_5m: None,
        cache_write_1h: None,
        reasoning: Some(usage.reasoning_output_tokens),
        total,
    }
}

fn codex_usage_delta_to_stats(
    previous: Option<&CodexTokenUsage>,
    current: &CodexTokenUsage,
) -> TokenStats {
    let (
        input_tokens,
        cached_input_tokens,
        cache_write_input_tokens,
        output_tokens,
        reasoning_output_tokens,
    ) = match previous {
        Some(previous)
            if current.input_tokens >= previous.input_tokens
                && current.cached_input_tokens >= previous.cached_input_tokens
                && current.cache_write_input_tokens >= previous.cache_write_input_tokens
                && current.output_tokens >= previous.output_tokens
                && current.reasoning_output_tokens >= previous.reasoning_output_tokens =>
        {
            (
                current.input_tokens - previous.input_tokens,
                current.cached_input_tokens - previous.cached_input_tokens,
                current.cache_write_input_tokens - previous.cache_write_input_tokens,
                current.output_tokens - previous.output_tokens,
                current.reasoning_output_tokens - previous.reasoning_output_tokens,
            )
        }
        _ => (
            current.input_tokens,
            current.cached_input_tokens,
            current.cache_write_input_tokens,
            current.output_tokens,
            current.reasoning_output_tokens,
        ),
    };

    let cache_read = cached_input_tokens;
    let cache_write = cache_write_input_tokens;
    let input = input_tokens.saturating_sub(cache_read);
    let output = output_tokens;
    let total = input_tokens.saturating_add(output);

    TokenStats {
        input,
        output,
        cache_read: Some(cache_read),
        cache_write: Some(cache_write),
        cache_write_5m: None,
        cache_write_1h: None,
        reasoning: Some(reasoning_output_tokens),
        total,
    }
}

/// Result of parsing one Codex rollout transcript.
pub(super) struct CodexSessionParse {
    pub(super) entries: Vec<UsageEntry>,
    /// Number of lines that could not be read or were not valid JSON.
    ///
    /// Codex appends to a rollout while it runs, so a torn last line means the
    /// read caught the file mid-write and the parsed entries may be incomplete.
    pub(super) malformed_lines: usize,
}

pub(super) fn parse_codex_session_file_with_diagnostics(
    filepath: &Path,
) -> Result<CodexSessionParse, String> {
    let file = File::open(filepath).map_err(|e| format!("無法開啟檔案: {}", e))?;
    let reader = BufReader::new(file);
    let fallback_session_id = filepath
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("unknown-session")
        .trim_start_matches("rollout-")
        .to_string();

    let mut events = Vec::new();
    let mut malformed_lines = 0usize;
    for line_res in reader.lines() {
        let line = match line_res {
            Ok(line) => line,
            Err(_) => {
                malformed_lines += 1;
                continue;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(event) = serde_json::from_str::<serde_json::Value>(&line) {
            events.push(event);
        } else {
            malformed_lines += 1;
        }
    }

    let mut session_id = fallback_session_id.clone();
    let mut session_name_selector = InitialUserPromptSelector::default();
    let mut session_cwd: Option<String> = None;
    let mut session_version: Option<String> = None;
    let mut parent_session_id: Option<String> = None;
    let mut agent_nickname: Option<String> = None;
    let mut agent_role: Option<String> = None;
    let mut current_model = "GPT-5.3-Codex".to_string();
    let mut reasoning_effort: Option<String> = None;
    let mut source_kind = CODEX_OTHER_SOURCE_KIND.to_string();
    let mut session_identity_locked = false;

    for event in &events {
        let event_type = event.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let payload = match event.get("payload") {
            Some(payload) => payload,
            None => continue,
        };
        let payload_type = payload.get("type").and_then(|t| t.as_str()).unwrap_or("");

        if event_type == "session_meta" {
            let detected_source_kind = codex_source_kind_from_metadata(payload);
            if source_kind == CODEX_OTHER_SOURCE_KIND
                || detected_source_kind == CODEX_DESKTOP_SOURCE_KIND
            {
                source_kind = detected_source_kind.to_string();
            }
            if !session_identity_locked {
                if let Some(id) = payload
                    .get("id")
                    .and_then(|id| id.as_str())
                    .filter(|id| !id.is_empty())
                    .or_else(|| {
                        payload
                            .get("session_id")
                            .and_then(|id| id.as_str())
                            .filter(|id| !id.is_empty())
                    })
                {
                    session_id = id.to_string();
                    session_identity_locked = true;
                }
            }
            session_cwd = payload
                .get("cwd")
                .and_then(|cwd| cwd.as_str())
                .map(|cwd| cwd.to_string())
                .or(session_cwd);
            session_version = payload
                .get("cli_version")
                .and_then(|version| version.as_str())
                .map(|version| version.to_string())
                .or(session_version);
            parent_session_id = payload
                .get("parent_thread_id")
                .and_then(|id| id.as_str())
                .map(|id| id.to_string())
                .or(parent_session_id);
            agent_nickname = payload
                .get("agent_nickname")
                .and_then(|name| name.as_str())
                .map(|name| name.to_string())
                .or(agent_nickname);
            agent_role = payload
                .get("agent_role")
                .and_then(|role| role.as_str())
                .map(|role| role.to_string())
                .or(agent_role);
            if let Some(model) = payload.get("model").and_then(|model| model.as_str()) {
                current_model = model.to_string();
            }
        } else if event_type == "turn_context" {
            session_cwd = payload
                .get("cwd")
                .and_then(|cwd| cwd.as_str())
                .map(|cwd| cwd.to_string())
                .or(session_cwd);
            if let Some(model) = payload.get("model").and_then(|model| model.as_str()) {
                current_model = model.to_string();
            }
            reasoning_effort = payload
                .get("effort")
                .or_else(|| payload.get("reasoning_effort"))
                .and_then(|effort| effort.as_str())
                .map(|effort| effort.to_string())
                .or(reasoning_effort);
        }

        match (event_type, payload_type) {
            ("event_msg", "user_message") => {
                if let Some(message) = payload.get("message").and_then(|message| message.as_str()) {
                    session_name_selector.observe_user_prompt(message);
                }
            }
            ("response_item", "message")
                if payload.get("role").and_then(|role| role.as_str()) == Some("user") =>
            {
                if let Some(content) = payload.get("content") {
                    session_name_selector.observe_user_prompt(&codex_content_to_text(content));
                }
            }
            ("event_msg", "agent_message")
            | ("response_item", "function_call" | "function_call_output") => {
                session_name_selector.observe_non_user_message();
            }
            ("response_item", "message")
                if payload.get("role").and_then(|role| role.as_str()) == Some("assistant") =>
            {
                session_name_selector.observe_non_user_message();
            }
            _ => {}
        }
    }

    let session_name = session_name_selector.into_name();
    let completed_task_duration_ms = events
        .iter()
        .filter_map(|event| {
            if event.get("type").and_then(|value| value.as_str()) != Some("event_msg") {
                return None;
            }
            let payload = event.get("payload")?;
            if payload.get("type").and_then(|value| value.as_str()) != Some("task_complete") {
                return None;
            }
            payload.get("duration_ms").and_then(|value| value.as_u64())
        })
        .fold(None::<u64>, |total, duration_ms| {
            Some(total.unwrap_or_default().saturating_add(duration_ms))
        });

    if parent_session_id.as_deref() == Some(session_id.as_str()) {
        parent_session_id = None;
    }

    let mut results = Vec::new();
    let mut model_for_turn = current_model.clone();
    let mut effort_for_turn = reasoning_effort.clone();
    let mut previous_total_usage: Option<CodexTokenUsage> = None;

    for event in events {
        let event_type = event.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let timestamp = event
            .get("timestamp")
            .and_then(|timestamp| timestamp.as_str())
            .unwrap_or("")
            .to_string();
        let payload = match event.get("payload") {
            Some(payload) => payload,
            None => continue,
        };
        let payload_type = payload.get("type").and_then(|t| t.as_str()).unwrap_or("");

        if event_type == "turn_context" {
            if let Some(model) = payload.get("model").and_then(|model| model.as_str()) {
                model_for_turn = model.to_string();
            }
            effort_for_turn = payload
                .get("effort")
                .or_else(|| payload.get("reasoning_effort"))
                .and_then(|effort| effort.as_str())
                .map(|effort| effort.to_string())
                .or(effort_for_turn);
            continue;
        }

        if event_type != "event_msg" || payload_type != "token_count" {
            continue;
        }

        let info = match payload.get("info") {
            Some(info) => info,
            None => continue,
        };
        let total_usage = match info
            .get("total_token_usage")
            .cloned()
            .and_then(|value| serde_json::from_value::<CodexTokenUsage>(value).ok())
        {
            Some(usage) => usage,
            None => continue,
        };
        let delta_tokens = codex_usage_delta_to_stats(previous_total_usage.as_ref(), &total_usage);
        previous_total_usage = Some(total_usage.clone());

        let context = info
            .get("model_context_window")
            .and_then(|window| window.as_u64())
            .map(|window| ContextStats {
                current_context_tokens: None,
                displayed_context_limit: Some(window),
                current_context_used_percentage: None,
            });

        results.push(UsageEntry {
            timestamp,
            session_id: session_id.clone(),
            session_name: session_name
                .clone()
                .or_else(|| Some(fallback_session_id.clone())),
            transcript_path: Some(filepath.to_string_lossy().into_owned()),
            cwd: session_cwd.clone(),
            version: session_version.clone(),
            turn_no: (results.len() + 1) as u32,
            model: Some(model_for_turn.clone()),
            model_id: Some(model_for_turn.clone()),
            tokens: Some(codex_usage_to_stats(total_usage)),
            delta_tokens: Some(delta_tokens),
            context,
            cost: completed_task_duration_ms.map(|duration_ms| CostStats {
                total_api_duration_ms: Some(duration_ms as f64),
                total_duration_ms: None,
                total_premium_requests: None,
                reported_cost_usd: None,
            }),
            source_kind: Some(source_kind.clone()),
            source_dir_key: None,
            parent_session_id: parent_session_id.clone(),
            agent_nickname: agent_nickname.clone(),
            agent_role: agent_role.clone(),
            reasoning_effort: effort_for_turn.clone(),
        });
    }

    Ok(CodexSessionParse {
        entries: results,
        malformed_lines,
    })
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
    fn parse_codex_session_file_derives_delta_from_cumulative_usage() {
        let path = temp_jsonl_path("codex-parser");

        let content = r#"{"timestamp":"2026-07-07T10:58:17.474Z","type":"session_meta","payload":{"session_id":"session-1","cwd":"/tmp/project","cli_version":"0.142.5","model":"gpt-5.5"}}
{"timestamp":"2026-07-07T10:58:26.197Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":10,"reasoning_output_tokens":4,"total_tokens":110},"last_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":10,"reasoning_output_tokens":4,"total_tokens":110},"model_context_window":258400}}}
{"timestamp":"2026-07-07T10:59:26.197Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":10,"reasoning_output_tokens":4,"total_tokens":110},"last_token_usage":{"input_tokens":0,"cached_input_tokens":0,"output_tokens":0,"reasoning_output_tokens":0,"total_tokens":19347},"model_context_window":258400}}}
{"timestamp":"2026-07-07T11:00:26.197Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":130,"cached_input_tokens":30,"output_tokens":15,"reasoning_output_tokens":7,"total_tokens":145},"last_token_usage":{"input_tokens":30,"cached_input_tokens":10,"output_tokens":5,"reasoning_output_tokens":3,"total_tokens":35},"model_context_window":258400}}}
"#;

        fs::write(&path, content).unwrap();
        let entries = parse_codex_session_file_with_diagnostics(&path)
            .unwrap()
            .entries;
        let _ = fs::remove_file(&path);

        assert_eq!(entries.len(), 3);

        let first = entries[0].delta_tokens.as_ref().unwrap();
        assert_eq!(first.input, 80);
        assert_eq!(first.cache_read, Some(20));
        assert_eq!(first.output, 10);
        assert_eq!(first.reasoning, Some(4));
        assert_eq!(first.total, 110);

        let anomalous = entries[1].delta_tokens.as_ref().unwrap();
        assert_eq!(anomalous.input, 0);
        assert_eq!(anomalous.cache_read, Some(0));
        assert_eq!(anomalous.output, 0);
        assert_eq!(anomalous.reasoning, Some(0));
        assert_eq!(anomalous.total, 0);

        let third = entries[2].delta_tokens.as_ref().unwrap();
        assert_eq!(third.input, 20);
        assert_eq!(third.cache_read, Some(10));
        assert_eq!(third.output, 5);
        assert_eq!(third.reasoning, Some(3));
        assert_eq!(third.total, 35);

        let total = entries
            .iter()
            .map(|entry| entry.delta_tokens.as_ref().unwrap().total)
            .sum::<u64>();
        assert_eq!(total, 145);
    }

    #[test]
    fn parse_codex_session_file_sums_completed_task_durations() {
        let path = temp_jsonl_path("codex-task-duration");

        let content = r#"{"timestamp":"2026-07-07T10:58:17.474Z","type":"session_meta","payload":{"session_id":"session-duration","model":"gpt-5.5"}}
{"timestamp":"2026-07-07T10:58:18.000Z","type":"event_msg","payload":{"type":"task_started"}}
{"timestamp":"2026-07-07T10:58:19.000Z","type":"event_msg","payload":{"type":"task_complete","duration_ms":1200}}
{"timestamp":"2026-07-07T10:58:20.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":10,"reasoning_output_tokens":4,"total_tokens":110},"model_context_window":258400}}}
{"timestamp":"2026-07-07T10:58:21.000Z","type":"event_msg","payload":{"type":"task_started"}}
{"timestamp":"2026-07-07T10:58:22.000Z","type":"event_msg","payload":{"type":"task_complete","duration_ms":2300}}
{"timestamp":"2026-07-07T10:58:23.000Z","type":"event_msg","payload":{"type":"task_started","duration_ms":9000}}
{"timestamp":"2026-07-07T10:58:24.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":130,"cached_input_tokens":30,"output_tokens":15,"reasoning_output_tokens":7,"total_tokens":145},"model_context_window":258400}}}
"#;

        fs::write(&path, content).unwrap();
        let entries = parse_codex_session_file_with_diagnostics(&path)
            .unwrap()
            .entries;
        let _ = fs::remove_file(&path);

        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|entry| {
            entry
                .cost
                .as_ref()
                .and_then(|cost| cost.total_api_duration_ms)
                == Some(3500.0)
        }));
    }

    #[test]
    fn parse_codex_session_file_uses_last_initial_consecutive_user_prompt_as_name() {
        let path = temp_jsonl_path("codex-session-name");
        let content = r#"{"timestamp":"2026-07-16T00:00:00Z","type":"session_meta","payload":{"session_id":"session-name","model":"gpt-5.5"}}
{"timestamp":"2026-07-16T00:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"第一條提示"}}
{"timestamp":"2026-07-16T00:00:02Z","type":"event_msg","payload":{"type":"user_message","message":"第二條提示"}}
{"timestamp":"2026-07-16T00:00:03Z","type":"event_msg","payload":{"type":"agent_message","message":"收到"}}
{"timestamp":"2026-07-16T00:00:04Z","type":"event_msg","payload":{"type":"user_message","message":"後續提示"}}
{"timestamp":"2026-07-16T00:00:05Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":10,"reasoning_output_tokens":4,"total_tokens":110},"model_context_window":258400}}}
"#;

        fs::write(&path, content).unwrap();
        let entries = parse_codex_session_file_with_diagnostics(&path)
            .unwrap()
            .entries;
        let _ = fs::remove_file(&path);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_name.as_deref(), Some("第二條提示"));
    }

    #[test]
    fn parse_codex_session_file_ignores_repeats_and_handles_resets() {
        let path = temp_jsonl_path("codex-parser");

        let content = r#"{"timestamp":"2026-06-17T13:50:00.000Z","type":"session_meta","payload":{"session_id":"session-2","cwd":"/tmp/project","cli_version":"0.142.5","model":"gpt-5.5"}}
{"timestamp":"2026-06-17T13:50:51.243Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1000,"cached_input_tokens":200,"output_tokens":100,"reasoning_output_tokens":40,"total_tokens":1100},"last_token_usage":{"input_tokens":1000,"cached_input_tokens":200,"output_tokens":100,"reasoning_output_tokens":40,"total_tokens":1100},"model_context_window":121600}}}
{"timestamp":"2026-06-17T13:50:54.339Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1000,"cached_input_tokens":200,"output_tokens":100,"reasoning_output_tokens":40,"total_tokens":1100},"last_token_usage":{"input_tokens":1000,"cached_input_tokens":200,"output_tokens":100,"reasoning_output_tokens":40,"total_tokens":1100},"model_context_window":121600}}}
{"timestamp":"2026-06-17T13:53:01.169Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":0,"cached_input_tokens":0,"output_tokens":0,"reasoning_output_tokens":0,"total_tokens":121600},"last_token_usage":{"input_tokens":0,"cached_input_tokens":0,"output_tokens":0,"reasoning_output_tokens":0,"total_tokens":0},"model_context_window":121600}}}
{"timestamp":"2026-06-17T14:43:08.185Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":200,"cached_input_tokens":50,"output_tokens":20,"reasoning_output_tokens":8,"total_tokens":121820},"last_token_usage":{"input_tokens":200,"cached_input_tokens":50,"output_tokens":20,"reasoning_output_tokens":8,"total_tokens":220},"model_context_window":258400}}}
"#;

        fs::write(&path, content).unwrap();
        let entries = parse_codex_session_file_with_diagnostics(&path)
            .unwrap()
            .entries;
        let _ = fs::remove_file(&path);

        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].delta_tokens.as_ref().unwrap().total, 1100);
        assert_eq!(entries[1].delta_tokens.as_ref().unwrap().total, 0);
        assert_eq!(entries[2].delta_tokens.as_ref().unwrap().total, 0);

        let after_reset = entries[3].delta_tokens.as_ref().unwrap();
        assert_eq!(after_reset.input, 150);
        assert_eq!(after_reset.cache_read, Some(50));
        assert_eq!(after_reset.output, 20);
        assert_eq!(after_reset.reasoning, Some(8));
        assert_eq!(after_reset.total, 220);
    }

    #[test]
    fn parse_codex_session_file_keeps_subagent_identity_separate_from_parent() {
        let path = temp_jsonl_path("codex-subagent");
        let content = r#"{"timestamp":"2026-07-10T03:45:00.000Z","type":"session_meta","payload":{"session_id":"parent-session","id":"child-session","forked_from_id":"parent-session","parent_thread_id":"parent-session","cwd":"/tmp/project","cli_version":"0.142.5","model":"gpt-5.5","agent_nickname":"reviewer","agent_role":"review","source":{"subagent":{"thread_spawn":{"parent_thread_id":"parent-session","depth":1,"agent_nickname":"reviewer","agent_role":"review"}}}}}
{"timestamp":"2026-07-10T03:45:00.500Z","type":"session_meta","payload":{"session_id":"parent-session","id":"parent-session","cwd":"/tmp/project","cli_version":"0.142.5","model":"gpt-5.5","source":"cli"}}
{"timestamp":"2026-07-10T03:45:01.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":10,"reasoning_output_tokens":4,"total_tokens":110},"last_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":10,"reasoning_output_tokens":4,"total_tokens":110},"model_context_window":258400}}}
"#;

        fs::write(&path, content).unwrap();
        let entries = parse_codex_session_file_with_diagnostics(&path)
            .unwrap()
            .entries;
        let _ = fs::remove_file(&path);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_id, "child-session");
        assert_eq!(
            entries[0].parent_session_id.as_deref(),
            Some("parent-session")
        );
        assert_ne!(entries[0].session_id, "parent-session");
    }

    #[test]
    fn parse_codex_desktop_session_preserves_source_and_cache_write_tokens() {
        let path = temp_jsonl_path("codex-desktop");
        let content = r#"{"timestamp":"2026-07-26T10:00:00Z","type":"session_meta","payload":{"id":"desktop-session","session_id":"desktop-session","originator":"Codex Desktop","source":"vscode","cwd":"/tmp/project","cli_version":"0.145.0-alpha.30"}}
{"timestamp":"2026-07-26T10:00:00.500Z","type":"session_meta","payload":{"id":"desktop-session","session_id":"desktop-session","source":"cli","cwd":"/tmp/project","cli_version":"0.145.0-alpha.30"}}
{"timestamp":"2026-07-26T10:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"cache_write_input_tokens":5,"output_tokens":10,"reasoning_output_tokens":4,"total_tokens":110},"model_context_window":258400}}}
{"timestamp":"2026-07-26T10:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":150,"cached_input_tokens":30,"cache_write_input_tokens":8,"output_tokens":15,"reasoning_output_tokens":7,"total_tokens":165},"model_context_window":258400}}}
"#;

        fs::write(&path, content).unwrap();
        let entries = parse_codex_session_file_with_diagnostics(&path)
            .unwrap()
            .entries;
        let _ = fs::remove_file(&path);

        assert_eq!(entries.len(), 2);
        assert!(entries
            .iter()
            .all(|entry| entry.source_kind.as_deref() == Some(CODEX_DESKTOP_SOURCE_KIND)));

        let first = entries[0].delta_tokens.as_ref().unwrap();
        assert_eq!(first.cache_write, Some(5));

        let second = entries[1].delta_tokens.as_ref().unwrap();
        assert_eq!(second.input, 40);
        assert_eq!(second.cache_read, Some(10));
        assert_eq!(second.cache_write, Some(3));
        assert_eq!(second.output, 5);
        assert_eq!(second.reasoning, Some(3));
        assert_eq!(second.total, 55);
    }

    #[test]
    fn parse_codex_session_file_reports_malformed_lines() {
        let path = temp_jsonl_path("codex-malformed");

        fs::write(&path, b"{\"type\":\"event_msg\"}\n").unwrap();
        let complete = parse_codex_session_file_with_diagnostics(&path).unwrap();
        assert_eq!(complete.malformed_lines, 0);

        // A torn trailing line is what a transcript looks like while Codex is
        // still appending to it.
        fs::write(
            &path,
            b"{\"type\":\"event_msg\"}\n{\"timestamp\":\"2026-07-26T10:00:0",
        )
        .unwrap();
        let truncated = parse_codex_session_file_with_diagnostics(&path).unwrap();
        assert_eq!(truncated.malformed_lines, 1);
        assert!(truncated.entries.is_empty());

        fs::write(&path, b"{\"type\":\"event_msg\"}\n\n\n").unwrap();
        let blank_lines = parse_codex_session_file_with_diagnostics(&path).unwrap();
        assert_eq!(blank_lines.malformed_lines, 0, "blank lines are not damage");

        let _ = fs::remove_file(&path);
    }
}
