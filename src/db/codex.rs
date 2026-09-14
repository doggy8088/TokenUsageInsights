use super::*;

pub(super) fn find_codex_session_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(find_codex_session_files(&path));
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

pub(super) fn parse_codex_session_file(filepath: &Path) -> Result<Vec<UsageEntry>, String> {
    let file = File::open(filepath).map_err(|e| format!("無法開啟檔案: {}", e))?;
    let reader = BufReader::new(file);
    let fallback_session_id = filepath
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("unknown-session")
        .trim_start_matches("rollout-")
        .to_string();

    let mut events = Vec::new();
    for line_res in reader.lines() {
        let line = match line_res {
            Ok(line) => line,
            Err(_) => continue,
        };
        if let Ok(event) = serde_json::from_str::<serde_json::Value>(&line) {
            events.push(event);
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

    Ok(results)
}
