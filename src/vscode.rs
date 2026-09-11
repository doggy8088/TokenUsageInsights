use crate::db::{CostStats, InitialUserPromptSelector, TokenStats, UsageEntry};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

pub const SOURCE_KIND: &str = "vscode-chat";
const SESSION_ID_PREFIX: &str = "vscode-";

/// Copilot Chat extension workspace storage folder that sits next to
/// `chatSessions/` inside each `workspaceStorage/<workspace>/` directory.
const COPILOT_CHAT_STORAGE_DIR: &str = "GitHub.copilot-chat";
/// Per-session debug log folder written by the Copilot Chat extension when
/// `github.copilot.chat.agentDebugLog.fileLogging.enabled` is on (the setting
/// is also enabled by experiment for many users). Layout:
/// `GitHub.copilot-chat/debug-logs/<sessionId>/main.jsonl`.
const COPILOT_CHAT_DEBUG_LOGS_DIR: &str = "debug-logs";
const COPILOT_CHAT_DEBUG_LOG_FILE: &str = "main.jsonl";
/// The debug logger truncates long prompts and appends this marker.
const COPILOT_CHAT_DEBUG_LOG_TRUNCATED_SUFFIX: &str = "[truncated]";
/// A `user_message` span is emitted a second or two after VS Code stamps the
/// chat request. Accept a timestamp-only match within this window.
const COPILOT_CHAT_DEBUG_LOG_TIMESTAMP_TOLERANCE_MS: i64 = 120_000;
/// Upper bound when walking `parentSpanId` links back to a `user_message`.
const COPILOT_CHAT_DEBUG_LOG_MAX_PARENT_HOPS: usize = 32;

#[derive(Debug, Clone)]
pub struct ChatSession {
    pub session_id: String,
    pub creation_date: Option<i64>,
    pub initial_location: Option<String>,
    pub working_directory: Option<String>,
    pub responder_username: Option<String>,
    pub requests: Vec<ChatRequest>,
}

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub timestamp: Option<i64>,
    pub prompt: String,
    pub agent_id: Option<String>,
    pub model_id: Option<String>,
    pub completion_tokens: Option<u64>,
    pub prompt_tokens: Option<u64>,
    pub elapsed_ms: Option<u64>,
    pub response: Vec<Value>,
}

/// Token usage of one chat request (one user turn) aggregated from the
/// Copilot Chat extension debug log. A single chat request in agent mode fans
/// out into several LLM calls (one per tool-call round). VS Code itself only
/// persists the prompt size of the *last* call as `promptTokens` and the sum
/// of outputs as `completionTokens`, and it never persists cached tokens, so
/// the debug log is the only local source for cache reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DebugLogTurnUsage {
    /// Start time (Unix ms) of the `user_message` span.
    pub timestamp: Option<i64>,
    /// Prompt text recorded by the logger (possibly truncated).
    pub prompt: Option<String>,
    /// Number of `llm_request` spans that reported usage for this turn.
    pub llm_requests: usize,
    /// Sum of `inputTokens` across the turn. Includes cached tokens.
    pub input: u64,
    /// Sum of `outputTokens` across the turn.
    pub output: u64,
    /// Sum of `cachedTokens` (prompt cache reads) across the turn.
    pub cache_read: u64,
}

#[derive(Debug, Deserialize)]
struct SerializedChatSession {
    #[serde(default)]
    #[serde(rename = "creationDate")]
    creation_date: Option<i64>,
    #[serde(default)]
    #[serde(rename = "initialLocation")]
    initial_location: Option<String>,
    #[serde(default)]
    #[serde(rename = "responderUsername")]
    responder_username: Option<String>,
    #[serde(default)]
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    #[serde(default)]
    #[serde(rename = "workingDirectory")]
    working_directory: Option<String>,
    #[serde(default)]
    requests: Vec<SerializedChatRequest>,
}

#[derive(Debug, Deserialize)]
struct SerializedChatRequest {
    #[serde(default)]
    timestamp: Option<i64>,
    #[serde(default)]
    message: Option<SerializedChatMessage>,
    #[serde(default)]
    agent: Option<Value>,
    #[serde(default)]
    #[serde(rename = "modelId")]
    model_id: Option<String>,
    #[serde(default)]
    response: Option<Value>,
    #[serde(default)]
    #[serde(rename = "completionTokens")]
    completion_tokens: Option<u64>,
    #[serde(default)]
    #[serde(rename = "promptTokens")]
    prompt_tokens: Option<u64>,
    #[serde(default)]
    #[serde(rename = "elapsedMs")]
    elapsed_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct SerializedChatMessage {
    #[serde(default)]
    text: String,
}

#[derive(Debug, Deserialize)]
struct OperationLogEntry {
    kind: u8,
    #[serde(default)]
    k: Vec<Value>,
    #[serde(default)]
    v: Option<Value>,
    #[serde(default)]
    i: Option<usize>,
}

pub fn discover_workspace_storage_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();

    #[cfg(target_os = "windows")]
    {
        let base = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .or_else(dirs::config_dir);
        if let Some(base) = base {
            roots.push(base.join("Code").join("User").join("workspaceStorage"));
            roots.push(
                base.join("Code - Insiders")
                    .join("User")
                    .join("workspaceStorage"),
            );
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(base) = dirs::data_dir() {
            roots.push(base.join("Code").join("User").join("workspaceStorage"));
            roots.push(
                base.join("Code - Insiders")
                    .join("User")
                    .join("workspaceStorage"),
            );
        }
    }

    #[cfg(target_os = "linux")]
    {
        if let Some(base) = dirs::config_dir() {
            roots.push(base.join("Code").join("User").join("workspaceStorage"));
            roots.push(
                base.join("Code - Insiders")
                    .join("User")
                    .join("workspaceStorage"),
            );
        }
    }

    if let Some(custom_root) = crate::paths::env_path("VSCODE_USER_DATA_DIR") {
        roots.push(custom_root.join("User").join("workspaceStorage"));
    }

    if let Some(portable_root) = crate::paths::env_path("VSCODE_PORTABLE_DATA_DIR") {
        roots.push(
            portable_root
                .join("user-data")
                .join("User")
                .join("workspaceStorage"),
        );
        roots.push(portable_root.join("User").join("workspaceStorage"));
    }

    let mut seen = HashSet::new();
    roots.retain(|root| seen.insert(root.to_string_lossy().to_lowercase()));
    roots
}

pub fn discover_session_files() -> Vec<PathBuf> {
    let mut files = Vec::new();

    for root in discover_workspace_storage_roots() {
        let workspaces = match fs::read_dir(root) {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        for workspace in workspaces.flatten() {
            let chat_sessions = workspace.path().join("chatSessions");
            let entries = match fs::read_dir(chat_sessions) {
                Ok(entries) => entries,
                Err(_) => continue,
            };

            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }

                let extension = path.extension().and_then(|value| value.to_str());
                if matches!(extension, Some("json") | Some("jsonl")) {
                    files.push(path);
                }
            }
        }
    }

    files.sort();
    files
}

pub fn read_session_file(path: &Path) -> Result<ChatSession, String> {
    let content = fs::read_to_string(path)
        .map_err(|error| format!("無法讀取 VS Code 聊天檔案 {:?}: {error}", path))?;
    let document = if path.extension().and_then(|value| value.to_str()) == Some("jsonl") {
        replay_operation_log(&content)?
    } else {
        serde_json::from_str(&sanitize_json_surrogates(&content))
            .map_err(|error| format!("VS Code 聊天 JSON 格式錯誤 {:?}: {error}", path))?
    };

    let serialized: SerializedChatSession = serde_json::from_value(document)
        .map_err(|error| format!("VS Code 聊天資料結構錯誤 {:?}: {error}", path))?;
    let fallback_id = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("unknown")
        .to_string();
    let session_id = serialized.session_id.unwrap_or(fallback_id);

    let requests = serialized
        .requests
        .into_iter()
        .map(|request| ChatRequest {
            timestamp: request.timestamp,
            prompt: request
                .message
                .map(|message| message.text)
                .unwrap_or_default(),
            agent_id: request.agent.as_ref().and_then(agent_id),
            model_id: request.model_id,
            completion_tokens: request.completion_tokens,
            prompt_tokens: request.prompt_tokens,
            elapsed_ms: request.elapsed_ms,
            response: response_parts(request.response),
        })
        .collect();

    Ok(ChatSession {
        session_id,
        creation_date: serialized.creation_date,
        initial_location: serialized.initial_location,
        working_directory: serialized.working_directory,
        responder_username: serialized.responder_username,
        requests,
    })
}

pub fn is_github_copilot(session: &ChatSession) -> bool {
    if session
        .responder_username
        .as_deref()
        .is_some_and(contains_copilot_marker)
    {
        return true;
    }

    session.requests.iter().any(|request| {
        request
            .agent_id
            .as_deref()
            .is_some_and(contains_copilot_marker)
            || request
                .model_id
                .as_deref()
                .is_some_and(contains_copilot_marker)
    })
}

pub fn to_usage_entries(session: &ChatSession, path: &Path) -> Vec<UsageEntry> {
    let session_id = format!("{SESSION_ID_PREFIX}{}", session.session_id);
    let mut session_name_selector = InitialUserPromptSelector::default();
    for request in &session.requests {
        session_name_selector.observe_user_prompt(&request.prompt);
        if !request.response.is_empty() {
            session_name_selector.observe_non_user_message();
        }
    }
    let session_name = session_name_selector
        .into_name()
        .or_else(|| Some(session.session_id.clone()));
    let fallback_timestamp = session.creation_date.map(timestamp_to_iso);
    let debug_log_turns = debug_log_path(path)
        .filter(|debug_path| debug_path.is_file())
        .map(|debug_path| read_debug_log_turns(&debug_path))
        .unwrap_or_default();
    let debug_log_usage = match_debug_log_turns(&session.requests, &debug_log_turns);

    session
        .requests
        .iter()
        .enumerate()
        .map(|(index, request)| {
            let tokens = token_stats(request, debug_log_usage.get(&index));
            let timestamp = request
                .timestamp
                .map(timestamp_to_iso)
                .or_else(|| fallback_timestamp.clone())
                .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string());
            let model = request
                .model_id
                .clone()
                .or_else(|| response_model(&request.response));
            let cost = request.elapsed_ms.map(|elapsed_ms| CostStats {
                total_api_duration_ms: Some(elapsed_ms as f64),
                total_duration_ms: Some(elapsed_ms as f64),
                total_premium_requests: None,
                reported_cost_usd: None,
            });

            UsageEntry {
                timestamp,
                session_id: session_id.clone(),
                session_name: session_name.clone(),
                transcript_path: Some(path.to_string_lossy().into_owned()),
                cwd: session.working_directory.clone(),
                version: None,
                turn_no: (index + 1) as u32,
                model: model.clone(),
                model_id: model,
                tokens: tokens.clone(),
                delta_tokens: tokens,
                context: None,
                cost,
                source_kind: Some(SOURCE_KIND.to_string()),
                source_dir_key: None,
                parent_session_id: None,
                agent_nickname: None,
                agent_role: None,
                reasoning_effort: None,
            }
        })
        .collect()
}

fn replay_operation_log(content: &str) -> Result<Value, String> {
    let mut state = None::<Value>;
    let mut line_count = 0usize;

    for line in content.lines().filter(|line| !line.trim().is_empty()) {
        line_count += 1;
        let entry: OperationLogEntry = serde_json::from_str(&sanitize_json_surrogates(line))
            .map_err(|error| format!("VS Code 聊天操作記錄格式錯誤: {error}"))?;

        match entry.kind {
            0 => state = entry.v,
            1 => {
                let root = state
                    .as_mut()
                    .ok_or_else(|| "VS Code 聊天操作記錄缺少初始資料".to_string())?;
                apply_set(root, &entry.k, entry.v)?;
            }
            2 => {
                let root = state
                    .as_mut()
                    .ok_or_else(|| "VS Code 聊天操作記錄缺少初始資料".to_string())?;
                apply_push(root, &entry.k, entry.v, entry.i)?;
            }
            3 => {
                let root = state
                    .as_mut()
                    .ok_or_else(|| "VS Code 聊天操作記錄缺少初始資料".to_string())?;
                apply_delete(root, &entry.k)?;
            }
            kind => return Err(format!("不支援的 VS Code 聊天操作類型: {kind}")),
        }
    }

    if line_count == 0 {
        return Err("VS Code 聊天操作記錄是空檔案".to_string());
    }

    state.ok_or_else(|| "VS Code 聊天操作記錄沒有初始資料".to_string())
}

fn sanitize_json_surrogates(input: &str) -> Cow<'_, str> {
    let bytes = input.as_bytes();
    let mut output = None::<String>;
    let mut copied_until = 0usize;
    let mut in_string = false;
    let mut index = 0usize;

    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                in_string = !in_string;
                index += 1;
            }
            b'\\' if in_string => {
                if bytes.get(index + 1) != Some(&b'u') {
                    index += if index + 1 < bytes.len() { 2 } else { 1 };
                    continue;
                }

                let Some(code_unit) = parse_hex_code_unit(bytes, index + 2) else {
                    index += 2;
                    continue;
                };
                let is_high_surrogate = (0xD800..=0xDBFF).contains(&code_unit);
                let is_low_surrogate = (0xDC00..=0xDFFF).contains(&code_unit);
                let has_matching_low_surrogate = is_high_surrogate
                    && bytes.get(index + 6) == Some(&b'\\')
                    && bytes.get(index + 7) == Some(&b'u')
                    && parse_hex_code_unit(bytes, index + 8)
                        .is_some_and(|next| (0xDC00..=0xDFFF).contains(&next));

                if has_matching_low_surrogate {
                    index += 12;
                } else if is_high_surrogate || is_low_surrogate {
                    let sanitized =
                        output.get_or_insert_with(|| String::with_capacity(input.len()));
                    sanitized.push_str(&input[copied_until..index]);
                    sanitized.push_str("\\uFFFD");
                    index += 6;
                    copied_until = index;
                } else {
                    index += 6;
                }
            }
            _ => index += 1,
        }
    }

    match output {
        Some(mut sanitized) => {
            sanitized.push_str(&input[copied_until..]);
            Cow::Owned(sanitized)
        }
        None => Cow::Borrowed(input),
    }
}

fn parse_hex_code_unit(bytes: &[u8], start: usize) -> Option<u16> {
    let digits = bytes.get(start..start + 4)?;
    digits.iter().try_fold(0u16, |value, &digit| {
        let digit = match digit {
            b'0'..=b'9' => u16::from(digit - b'0'),
            b'a'..=b'f' => u16::from(digit - b'a' + 10),
            b'A'..=b'F' => u16::from(digit - b'A' + 10),
            _ => return None,
        };
        Some((value << 4) | digit)
    })
}

fn apply_set(root: &mut Value, path: &[Value], value: Option<Value>) -> Result<(), String> {
    if path.is_empty() {
        return Ok(());
    }

    let (parent_path, key) = path.split_at(path.len() - 1);
    let parent = value_at_path_mut(root, parent_path)?;
    match parent {
        Value::Object(object) => {
            let key = key[0]
                .as_str()
                .ok_or_else(|| "VS Code 聊天物件路徑不是字串".to_string())?;
            if let Some(value) = value {
                object.insert(key.to_string(), value);
            } else {
                object.remove(key);
            }
        }
        Value::Array(array) => {
            let index = key[0]
                .as_u64()
                .ok_or_else(|| "VS Code 聊天陣列路徑不是數字".to_string())?
                as usize;
            if index >= array.len() {
                return Err("VS Code 聊天陣列路徑超出範圍".to_string());
            }
            array[index] = value.unwrap_or(Value::Null);
        }
        _ => return Err("VS Code 聊天操作路徑不是容器".to_string()),
    }
    Ok(())
}

fn apply_delete(root: &mut Value, path: &[Value]) -> Result<(), String> {
    if path.is_empty() {
        return Err("VS Code 聊天刪除操作缺少路徑".to_string());
    }

    let (parent_path, key) = path.split_at(path.len() - 1);
    let parent = value_at_path_mut(root, parent_path)?;
    match parent {
        Value::Object(object) => {
            let key = key[0]
                .as_str()
                .ok_or_else(|| "VS Code 聊天物件路徑不是字串".to_string())?;
            object.remove(key);
        }
        Value::Array(array) => {
            let index = key[0]
                .as_u64()
                .ok_or_else(|| "VS Code 聊天陣列路徑不是數字".to_string())?
                as usize;
            if index >= array.len() {
                return Err("VS Code 聊天刪除索引超出範圍".to_string());
            }
            array.remove(index);
        }
        _ => return Err("VS Code 聊天刪除路徑不是容器".to_string()),
    }
    Ok(())
}

fn apply_push(
    root: &mut Value,
    path: &[Value],
    values: Option<Value>,
    start_index: Option<usize>,
) -> Result<(), String> {
    let target = value_at_path_mut(root, path)?;
    let array = target
        .as_array_mut()
        .ok_or_else(|| "VS Code 聊天 Push 目標不是陣列".to_string())?;
    if let Some(start_index) = start_index {
        if start_index > array.len() {
            return Err("VS Code 聊天 Push 起始位置超出範圍".to_string());
        }
        array.truncate(start_index);
    }
    if let Some(Value::Array(values)) = values {
        array.extend(values);
    }
    Ok(())
}

fn value_at_path_mut<'a>(root: &'a mut Value, path: &[Value]) -> Result<&'a mut Value, String> {
    let mut current = root;
    for segment in path {
        current = match current {
            Value::Object(object) => object
                .get_mut(
                    segment
                        .as_str()
                        .ok_or_else(|| "VS Code 聊天物件路徑不是字串".to_string())?,
                )
                .ok_or_else(|| "找不到 VS Code 聊天操作路徑".to_string())?,
            Value::Array(array) => array
                .get_mut(
                    segment
                        .as_u64()
                        .ok_or_else(|| "VS Code 聊天陣列路徑不是數字".to_string())?
                        as usize,
                )
                .ok_or_else(|| "找不到 VS Code 聊天操作路徑".to_string())?,
            _ => return Err("VS Code 聊天操作路徑不是容器".to_string()),
        };
    }
    Ok(current)
}

fn response_parts(response: Option<Value>) -> Vec<Value> {
    match response {
        Some(Value::Array(parts)) => parts,
        Some(Value::Null) | None => Vec::new(),
        Some(part) => vec![part],
    }
}

fn agent_id(agent: &Value) -> Option<String> {
    agent
        .as_str()
        .map(str::to_string)
        .or_else(|| agent.get("id").and_then(Value::as_str).map(str::to_string))
        .or_else(|| {
            agent
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}

fn contains_copilot_marker(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    value.contains("copilot")
}

fn token_stats(
    request: &ChatRequest,
    debug_log_usage: Option<&DebugLogTurnUsage>,
) -> Option<TokenStats> {
    if let Some(usage) = debug_log_usage.filter(|usage| usage.llm_requests > 0) {
        // The debug log reports `inputTokens` inclusive of cache reads (the
        // same convention as Copilot CLI `assistant_usage_events`). The
        // dashboard stores non-cached input and cache reads separately so
        // pricing can bill each at its own rate.
        let cache_read = usage.cache_read.min(usage.input);
        let input = usage.input - cache_read;
        let output = usage.output;
        return Some(TokenStats {
            input,
            output,
            cache_read: Some(cache_read),
            cache_write: None,
            cache_write_5m: None,
            cache_write_1h: None,
            reasoning: None,
            total: input.saturating_add(cache_read).saturating_add(output),
        });
    }

    if request.prompt_tokens.is_none() && request.completion_tokens.is_none() {
        return None;
    }
    let input = request.prompt_tokens.unwrap_or(0);
    let output = request.completion_tokens.unwrap_or(0);
    Some(TokenStats {
        input,
        output,
        cache_read: None,
        cache_write: None,
        cache_write_5m: None,
        cache_write_1h: None,
        reasoning: None,
        total: input.saturating_add(output),
    })
}

/// Resolve the Copilot Chat debug log that belongs to a `chatSessions` file:
/// `<workspace>/chatSessions/<sessionId>.jsonl` maps to
/// `<workspace>/GitHub.copilot-chat/debug-logs/<sessionId>/main.jsonl`.
/// The path is returned even when the file does not exist so callers can
/// include it in sync-state signatures.
pub fn debug_log_path(session_file: &Path) -> Option<PathBuf> {
    let session_id = session_file.file_stem()?.to_str()?;
    let workspace_dir = session_file.parent()?.parent()?;
    Some(
        workspace_dir
            .join(COPILOT_CHAT_STORAGE_DIR)
            .join(COPILOT_CHAT_DEBUG_LOGS_DIR)
            .join(session_id)
            .join(COPILOT_CHAT_DEBUG_LOG_FILE),
    )
}

/// Read a Copilot Chat debug log and aggregate LLM usage per user turn.
/// Unreadable files yield an empty list so the caller falls back to the
/// token fields persisted by VS Code itself.
pub fn read_debug_log_turns(path: &Path) -> Vec<DebugLogTurnUsage> {
    match fs::read_to_string(path) {
        Ok(content) => parse_debug_log_turns(&content),
        Err(_) => Vec::new(),
    }
}

#[derive(Debug)]
struct DebugLogLlmRequest {
    timestamp: Option<i64>,
    parent_span_id: Option<String>,
    input: u64,
    output: u64,
    cache_read: u64,
}

/// Aggregate `llm_request` spans of a debug log under the `user_message`
/// span they belong to. Spans are linked to their turn by walking
/// `parentSpanId`; spans whose chain does not end at a `user_message` are
/// attributed to the most recent turn that started before them.
fn parse_debug_log_turns(content: &str) -> Vec<DebugLogTurnUsage> {
    let mut turns: Vec<DebugLogTurnUsage> = Vec::new();
    let mut turn_index_by_span: HashMap<String, usize> = HashMap::new();
    let mut parent_by_span: HashMap<String, String> = HashMap::new();
    let mut llm_requests: Vec<DebugLogLlmRequest> = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Value>(&sanitize_json_surrogates(line)) else {
            continue;
        };
        let Some(kind) = event.get("type").and_then(Value::as_str) else {
            continue;
        };
        let span_id = event
            .get("spanId")
            .and_then(Value::as_str)
            .map(str::to_string);
        let parent_span_id = event
            .get("parentSpanId")
            .and_then(Value::as_str)
            .map(str::to_string);
        if let (Some(span), Some(parent)) = (&span_id, &parent_span_id) {
            parent_by_span.insert(span.clone(), parent.clone());
        }
        let timestamp = event.get("ts").and_then(Value::as_i64);
        let attrs = event.get("attrs");

        match kind {
            "user_message" => {
                let prompt = attrs
                    .and_then(|attrs| attrs.get("content"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                if let Some(span) = span_id {
                    turn_index_by_span.insert(span, turns.len());
                }
                turns.push(DebugLogTurnUsage {
                    timestamp,
                    prompt,
                    ..DebugLogTurnUsage::default()
                });
            }
            "llm_request" => {
                let Some(attrs) = attrs else {
                    continue;
                };
                let input = attrs.get("inputTokens").and_then(Value::as_u64);
                let output = attrs.get("outputTokens").and_then(Value::as_u64);
                let cache_read = attrs.get("cachedTokens").and_then(Value::as_u64);
                if input.is_none() && output.is_none() && cache_read.is_none() {
                    // Failed or aborted call without usage; nothing to bill.
                    continue;
                }
                llm_requests.push(DebugLogLlmRequest {
                    timestamp,
                    parent_span_id,
                    input: input.unwrap_or(0),
                    output: output.unwrap_or(0),
                    cache_read: cache_read.unwrap_or(0),
                });
            }
            _ => {}
        }
    }

    if turns.is_empty() {
        return turns;
    }

    for request in llm_requests {
        let by_parent = resolve_debug_log_turn(
            request.parent_span_id.as_deref(),
            &parent_by_span,
            &turn_index_by_span,
        );
        let index = by_parent.or_else(|| {
            let ts = request.timestamp?;
            turns
                .iter()
                .rposition(|turn| turn.timestamp.is_some_and(|start| start <= ts))
        });
        let Some(index) = index else {
            continue;
        };
        let turn = &mut turns[index];
        turn.llm_requests += 1;
        turn.input = turn.input.saturating_add(request.input);
        turn.output = turn.output.saturating_add(request.output);
        turn.cache_read = turn.cache_read.saturating_add(request.cache_read);
    }

    turns
}

fn resolve_debug_log_turn(
    parent_span_id: Option<&str>,
    parent_by_span: &HashMap<String, String>,
    turn_index_by_span: &HashMap<String, usize>,
) -> Option<usize> {
    let mut current = parent_span_id?;
    for _ in 0..COPILOT_CHAT_DEBUG_LOG_MAX_PARENT_HOPS {
        if let Some(index) = turn_index_by_span.get(current) {
            return Some(*index);
        }
        current = parent_by_span.get(current)?;
    }
    None
}

/// Pair debug-log turns with the `requests` array of the chat session.
/// Returns a map from request index to the aggregated usage. Matching prefers
/// identical prompt text (allowing the logger's truncation) and breaks ties by
/// timestamp distance; when no prompt matches, the closest request within
/// [`COPILOT_CHAT_DEBUG_LOG_TIMESTAMP_TOLERANCE_MS`] is used.
fn match_debug_log_turns(
    requests: &[ChatRequest],
    turns: &[DebugLogTurnUsage],
) -> HashMap<usize, DebugLogTurnUsage> {
    let mut matched: HashMap<usize, DebugLogTurnUsage> = HashMap::new();
    if requests.is_empty() || turns.is_empty() {
        return matched;
    }

    for turn in turns {
        let distance = |index: usize| -> Option<i64> {
            let request_ts = requests[index].timestamp?;
            let turn_ts = turn.timestamp?;
            Some((turn_ts - request_ts).abs())
        };
        let candidates = (0..requests.len()).filter(|index| !matched.contains_key(index));

        let mut best_text: Option<(usize, i64)> = None;
        let mut best_time: Option<(usize, i64)> = None;
        for index in candidates {
            let dist = distance(index);
            if debug_log_prompt_matches(&requests[index].prompt, turn.prompt.as_deref()) {
                let dist = dist.unwrap_or(i64::MAX);
                if best_text.is_none_or(|(_, best)| dist < best) {
                    best_text = Some((index, dist));
                }
            }
            if let Some(dist) = dist {
                if dist <= COPILOT_CHAT_DEBUG_LOG_TIMESTAMP_TOLERANCE_MS
                    && best_time.is_none_or(|(_, best)| dist < best)
                {
                    best_time = Some((index, dist));
                }
            }
        }

        if let Some((index, _)) = best_text.or(best_time) {
            matched.insert(index, turn.clone());
        }
    }

    matched
}

fn debug_log_prompt_matches(request_prompt: &str, logged_prompt: Option<&str>) -> bool {
    let Some(logged) = logged_prompt else {
        return false;
    };
    let request_prompt = request_prompt.trim();
    let logged = logged.trim();
    if request_prompt.is_empty() || logged.is_empty() {
        return false;
    }
    if request_prompt == logged {
        return true;
    }
    logged
        .strip_suffix(COPILOT_CHAT_DEBUG_LOG_TRUNCATED_SUFFIX)
        .is_some_and(|prefix| !prefix.is_empty() && request_prompt.starts_with(prefix))
}

fn response_model(parts: &[Value]) -> Option<String> {
    parts.iter().find_map(|part| {
        part.get("modelId")
            .or_else(|| part.get("model"))
            .and_then(Value::as_str)
            .map(str::to_string)
    })
}

pub fn timestamp_to_iso(timestamp: i64) -> String {
    let date_time = if timestamp.unsigned_abs() > 100_000_000_000 {
        DateTime::<Utc>::from_timestamp_millis(timestamp)
    } else {
        DateTime::<Utc>::from_timestamp(timestamp, 0)
    };
    date_time
        .map(|value| value.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replays_vscode_operation_log() {
        let content = concat!(
            r#"{"kind":0,"v":{"sessionId":"abc","requests":[]}}"#,
            "\n",
            r#"{"kind":2,"k":["requests"],"v":[{"requestId":"r1","message":{"text":"hello"}}]}"#,
            "\n",
            r#"{"kind":1,"k":["requests",0,"promptTokens"],"v":12}"#,
            "\n",
            r#"{"kind":3,"k":["requests",0,"requestId"]}"#,
            "\n"
        );
        let value = replay_operation_log(content).expect("operation log should replay");
        assert_eq!(value["sessionId"], "abc");
        assert_eq!(value["requests"][0]["promptTokens"], 12);
        assert!(value["requests"][0].get("requestId").is_none());
    }

    #[test]
    fn replays_operation_log_with_lone_surrogates() {
        let content = concat!(
            r#"{"kind":0,"v":{"sessionId":"abc","requests":[{"message":{"text":"range [\uD800-\uDFFF], emoji \uD83D\uDC69, literal \\uD800"},"promptTokens":1}]}}"#,
            "\n"
        );

        let value = replay_operation_log(content).expect("operation log should replay");
        assert_eq!(
            value["requests"][0]["message"]["text"],
            "range [�-�], emoji 👩, literal \\uD800"
        );
    }

    #[test]
    fn sanitizer_only_allocates_for_invalid_surrogates() {
        let valid = r#"{"text":"emoji \uD83D\uDC69 and literal \\uD800"}"#;
        assert!(matches!(sanitize_json_surrogates(valid), Cow::Borrowed(_)));

        let invalid = r#"{"text":"\uD800 \udfff"}"#;
        assert_eq!(
            sanitize_json_surrogates(invalid),
            r#"{"text":"\uFFFD \uFFFD"}"#
        );
    }

    #[test]
    fn parses_flat_session_and_maps_tokens() {
        let path = PathBuf::from("session.json");
        let session: ChatSession = serde_json::from_value(serde_json::json!({
            "creationDate": 1_735_689_600_000i64,
            "sessionId": "abc",
            "responderUsername": "GitHub Copilot",
            "workingDirectory": "/tmp/project",
            "requests": [{
                "timestamp": 1_735_689_601_000i64,
                "message": {"text": "hello"},
                "agent": {"id": "github.copilot"},
                "modelId": "gpt-4o",
                "promptTokens": 10,
                "completionTokens": 5,
                "elapsedMs": 250,
                "response": [{"kind": "markdownContent", "content": "reply"}]
            }]
        }))
        .map(|raw: SerializedChatSession| ChatSession {
            session_id: raw.session_id.unwrap_or_default(),
            creation_date: raw.creation_date,
            initial_location: raw.initial_location,
            working_directory: raw.working_directory,
            responder_username: raw.responder_username,
            requests: raw
                .requests
                .into_iter()
                .map(|request| ChatRequest {
                    timestamp: request.timestamp,
                    prompt: request
                        .message
                        .map(|message| message.text)
                        .unwrap_or_default(),
                    agent_id: request.agent.as_ref().and_then(agent_id),
                    model_id: request.model_id,
                    completion_tokens: request.completion_tokens,
                    prompt_tokens: request.prompt_tokens,
                    elapsed_ms: request.elapsed_ms,
                    response: response_parts(request.response),
                })
                .collect(),
        })
        .expect("flat session should parse");

        assert!(is_github_copilot(&session));
        let entries = to_usage_entries(&session, &path);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_id, "vscode-abc");
        assert_eq!(
            entries[0].tokens.as_ref().map(|tokens| tokens.total),
            Some(15)
        );
        assert_eq!(entries[0].source_kind.as_deref(), Some(SOURCE_KIND));
    }

    #[test]
    fn session_name_uses_last_prompt_before_first_response() {
        let session = ChatSession {
            session_id: "consecutive-prompts".to_string(),
            creation_date: Some(1_735_689_600_000),
            initial_location: None,
            working_directory: Some("/tmp/project".to_string()),
            responder_username: Some("GitHub Copilot".to_string()),
            requests: vec![
                ChatRequest {
                    timestamp: Some(1_735_689_601_000),
                    prompt: "First prompt".to_string(),
                    agent_id: Some("github.copilot".to_string()),
                    model_id: Some("gpt-4o".to_string()),
                    completion_tokens: None,
                    prompt_tokens: None,
                    elapsed_ms: None,
                    response: Vec::new(),
                },
                ChatRequest {
                    timestamp: Some(1_735_689_602_000),
                    prompt: "Second prompt".to_string(),
                    agent_id: Some("github.copilot".to_string()),
                    model_id: Some("gpt-4o".to_string()),
                    completion_tokens: Some(5),
                    prompt_tokens: Some(10),
                    elapsed_ms: Some(250),
                    response: vec![serde_json::json!({
                        "kind": "markdownContent",
                        "content": "Reply"
                    })],
                },
                ChatRequest {
                    timestamp: Some(1_735_689_603_000),
                    prompt: "Later prompt".to_string(),
                    agent_id: Some("github.copilot".to_string()),
                    model_id: Some("gpt-4o".to_string()),
                    completion_tokens: Some(5),
                    prompt_tokens: Some(10),
                    elapsed_ms: Some(250),
                    response: vec![serde_json::json!({
                        "kind": "markdownContent",
                        "content": "Later reply"
                    })],
                },
            ],
        };

        let entries = to_usage_entries(&session, Path::new("session.json"));

        assert!(entries
            .iter()
            .all(|entry| entry.session_name.as_deref() == Some("Second prompt")));
    }

    fn request(
        timestamp: i64,
        prompt: &str,
        prompt_tokens: Option<u64>,
        completion_tokens: Option<u64>,
    ) -> ChatRequest {
        ChatRequest {
            timestamp: Some(timestamp),
            prompt: prompt.to_string(),
            agent_id: Some("github.copilot.editsAgent".to_string()),
            model_id: Some("copilot/gpt-5.4".to_string()),
            completion_tokens,
            prompt_tokens,
            elapsed_ms: Some(1_000),
            response: vec![serde_json::json!({
                "kind": "markdownContent",
                "content": "reply"
            })],
        }
    }

    /// Mirrors the real `GitHub.copilot-chat/debug-logs/<sid>/main.jsonl`
    /// layout: `user_message` spans own the turn, `llm_request` spans point
    /// back at them through `parentSpanId` (sometimes via a tool span).
    const DEBUG_LOG_FIXTURE: &str = concat!(
        r#"{"v":1,"ts":1783858098260,"dur":0,"sid":"abc","type":"session_start","name":"session_start","spanId":"session-start-abc","status":"ok","attrs":{"copilotVersion":"0.65.0"}}"#,
        "\n",
        r#"{"ts":1783858098936,"dur":0,"sid":"abc","type":"user_message","name":"user_message","spanId":"00d3","status":"ok","attrs":{"content":"幫我優化 README"}}"#,
        "\n",
        r#"{"ts":1783858098937,"dur":0,"sid":"abc","type":"turn_start","name":"turn_start","spanId":"turn_start-00d3-0","status":"ok","attrs":{"turnId":"0"}}"#,
        "\n",
        r#"{"ts":1783858100004,"dur":21078,"sid":"abc","type":"llm_request","name":"chat:gpt-5.4","spanId":"00dc","parentSpanId":"00d3","status":"ok","attrs":{"model":"gpt-5.4","debugName":"panel/editAgent","inputTokens":40446,"outputTokens":2661,"cachedTokens":11264}}"#,
        "\n",
        r#"{"ts":1783858121000,"dur":10,"sid":"abc","type":"tool_call","name":"tool:read_file","spanId":"00de","parentSpanId":"00d3","status":"ok","attrs":{"toolName":"read_file"}}"#,
        "\n",
        r#"{"ts":1783858121175,"dur":3864,"sid":"abc","type":"llm_request","name":"chat:gpt-5.4","spanId":"00e1","parentSpanId":"00de","status":"ok","attrs":{"model":"gpt-5.4","debugName":"panel/editAgent","inputTokens":42072,"outputTokens":115,"cachedTokens":40320}}"#,
        "\n",
        r#"{"ts":1783858125042,"dur":0,"sid":"abc","type":"turn_end","name":"turn_end","spanId":"turn_end-00d3-1","status":"ok","attrs":{"turnId":"1"}}"#,
        "\n",
        r#"{"ts":1783858200000,"dur":0,"sid":"abc","type":"user_message","name":"user_message","spanId":"00f0","status":"ok","attrs":{"content":"這是一段非常長的提示詞前綴[truncated]"}}"#,
        "\n",
        r#"{"ts":1783858200500,"dur":800,"sid":"abc","type":"llm_request","name":"chat:gpt-5.4","spanId":"00f2","parentSpanId":"missing-span","status":"error","attrs":{"model":"gpt-5.4","debugName":"panel/editAgent","error":"aborted"}}"#,
        "\n",
        r#"{"ts":1783858201000,"dur":1500,"sid":"abc","type":"llm_request","name":"chat:gpt-5.4","spanId":"00f3","parentSpanId":"missing-span","status":"ok","attrs":{"model":"gpt-5.4","debugName":"panel/editAgent","inputTokens":1000,"outputTokens":50,"cachedTokens":0}}"#,
        "\n",
        "not json at all",
        "\n"
    );

    #[test]
    fn debug_log_path_resolves_sibling_copilot_chat_dir() {
        let session_file = Path::new("/data/workspaceStorage/ws1/chatSessions/abc.jsonl");
        assert_eq!(
            debug_log_path(session_file),
            Some(PathBuf::from(
                "/data/workspaceStorage/ws1/GitHub.copilot-chat/debug-logs/abc/main.jsonl"
            ))
        );
        assert_eq!(
            debug_log_path(Path::new(
                "/data/workspaceStorage/ws1/chatSessions/abc.json"
            )),
            Some(PathBuf::from(
                "/data/workspaceStorage/ws1/GitHub.copilot-chat/debug-logs/abc/main.jsonl"
            ))
        );
        assert!(debug_log_path(Path::new("abc.json")).is_none());
    }

    #[test]
    fn parses_debug_log_turns_per_user_message() {
        let turns = parse_debug_log_turns(DEBUG_LOG_FIXTURE);
        assert_eq!(turns.len(), 2);

        // Turn 1: two LLM calls, one linked through an intermediate tool span.
        assert_eq!(turns[0].timestamp, Some(1_783_858_098_936));
        assert_eq!(turns[0].prompt.as_deref(), Some("幫我優化 README"));
        assert_eq!(turns[0].llm_requests, 2);
        assert_eq!(turns[0].input, 40_446 + 42_072);
        assert_eq!(turns[0].output, 2_661 + 115);
        assert_eq!(turns[0].cache_read, 11_264 + 40_320);

        // Turn 2: the aborted call carries no usage and is ignored; the call
        // with a dangling parent falls back to the latest turn by timestamp.
        assert_eq!(turns[1].llm_requests, 1);
        assert_eq!(turns[1].input, 1_000);
        assert_eq!(turns[1].output, 50);
        assert_eq!(turns[1].cache_read, 0);
    }

    #[test]
    fn debug_log_without_user_messages_yields_no_turns() {
        let content = concat!(
            r#"{"v":1,"ts":1,"dur":0,"sid":"abc","type":"session_start","name":"session_start","spanId":"s","status":"ok","attrs":{}}"#,
            "\n",
            r#"{"ts":2,"dur":1,"sid":"abc","type":"llm_request","name":"chat:x","spanId":"l","status":"ok","attrs":{"inputTokens":5,"outputTokens":1,"cachedTokens":0}}"#,
            "\n"
        );
        assert!(parse_debug_log_turns(content).is_empty());
        assert!(parse_debug_log_turns("").is_empty());
    }

    #[test]
    fn matches_debug_log_turns_by_prompt_then_timestamp() {
        let requests = vec![
            // Older request that has no debug-log counterpart at all.
            request(1_783_850_000_000, "舊的請求", Some(10), Some(5)),
            request(
                1_783_858_097_242,
                "幫我優化 README",
                Some(42_072),
                Some(2_776),
            ),
            request(
                1_783_858_198_500,
                "這是一段非常長的提示詞前綴，後面還有更多內容",
                None,
                Some(50),
            ),
            // Prompt differs from the logged text (e.g. slash command expansion)
            // but the timestamp is within tolerance.
            request(1_783_858_300_000, "/fix 這個錯誤", Some(500), Some(20)),
        ];
        let mut turns = parse_debug_log_turns(DEBUG_LOG_FIXTURE);
        turns.push(DebugLogTurnUsage {
            timestamp: Some(1_783_858_301_900),
            prompt: Some("Fix the following error".to_string()),
            llm_requests: 1,
            input: 600,
            output: 20,
            cache_read: 100,
        });
        // A turn far away from every request and with unknown text stays unmatched.
        turns.push(DebugLogTurnUsage {
            timestamp: Some(1_790_000_000_000),
            prompt: Some("孤兒回合".to_string()),
            llm_requests: 1,
            input: 1,
            output: 1,
            cache_read: 0,
        });

        let matched = match_debug_log_turns(&requests, &turns);
        assert_eq!(matched.len(), 3);
        assert!(!matched.contains_key(&0));
        assert_eq!(matched[&1].cache_read, 11_264 + 40_320);
        assert_eq!(matched[&2].input, 1_000);
        assert_eq!(matched[&3].cache_read, 100);
    }

    #[test]
    fn to_usage_entries_prefers_debug_log_usage_for_cache_reads() {
        let root = std::env::temp_dir().join(format!(
            "tui-vscode-debug-log-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let workspace = root.join("workspaceStorage").join("ws1");
        let chat_sessions = workspace.join("chatSessions");
        let debug_dir = workspace
            .join(COPILOT_CHAT_STORAGE_DIR)
            .join(COPILOT_CHAT_DEBUG_LOGS_DIR)
            .join("abc");
        fs::create_dir_all(&chat_sessions).expect("chatSessions dir");
        fs::create_dir_all(&debug_dir).expect("debug-logs dir");
        let session_file = chat_sessions.join("abc.jsonl");
        fs::write(&session_file, "").expect("session placeholder");
        fs::write(
            debug_dir.join(COPILOT_CHAT_DEBUG_LOG_FILE),
            DEBUG_LOG_FIXTURE,
        )
        .expect("debug log");

        let session = ChatSession {
            session_id: "abc".to_string(),
            creation_date: Some(1_783_858_090_000),
            initial_location: Some("panel".to_string()),
            working_directory: Some("/tmp/project".to_string()),
            responder_username: Some("GitHub Copilot".to_string()),
            requests: vec![
                request(
                    1_783_858_097_242,
                    "幫我優化 README",
                    Some(42_072),
                    Some(2_776),
                ),
                request(
                    1_783_858_198_500,
                    "這是一段非常長的提示詞前綴，後面還有更多內容",
                    None,
                    Some(50),
                ),
                // No debug-log turn: falls back to VS Code's own token fields.
                request(1_783_859_000_000, "沒有除錯記錄的回合", Some(300), Some(30)),
            ],
        };

        let entries = to_usage_entries(&session, &session_file);
        assert_eq!(entries.len(), 3);

        let first = entries[0].tokens.as_ref().expect("turn 1 tokens");
        assert_eq!(first.cache_read, Some(11_264 + 40_320));
        assert_eq!(first.input, (40_446 - 11_264) + (42_072 - 40_320));
        assert_eq!(first.output, 2_776);
        assert_eq!(first.total, first.input + 51_584 + 2_776);
        assert_eq!(
            entries[0].delta_tokens.as_ref().map(|tokens| tokens.total),
            Some(first.total)
        );

        let second = entries[1].tokens.as_ref().expect("turn 2 tokens");
        assert_eq!(second.input, 1_000);
        assert_eq!(second.cache_read, Some(0));
        assert_eq!(second.total, 1_050);

        let third = entries[2].tokens.as_ref().expect("turn 3 tokens");
        assert_eq!(third.input, 300);
        assert_eq!(third.output, 30);
        assert_eq!(third.cache_read, None);
        assert_eq!(third.total, 330);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_debug_log_keeps_vscode_token_fields() {
        let session = ChatSession {
            session_id: "no-debug-log".to_string(),
            creation_date: Some(1_783_858_090_000),
            initial_location: None,
            working_directory: None,
            responder_username: Some("GitHub Copilot".to_string()),
            requests: vec![request(1_783_858_097_242, "hello", Some(10), Some(5))],
        };
        let entries = to_usage_entries(
            &session,
            Path::new("/nonexistent/workspaceStorage/ws/chatSessions/no-debug-log.jsonl"),
        );
        let tokens = entries[0].tokens.as_ref().expect("tokens");
        assert_eq!(
            (tokens.input, tokens.output, tokens.cache_read, tokens.total),
            (10, 5, None, 15)
        );
    }

    #[test]
    fn empty_copilot_session_produces_no_usage_entries() {
        let session = ChatSession {
            session_id: "empty-session".to_string(),
            creation_date: Some(1_735_689_600_000),
            initial_location: None,
            working_directory: Some("/tmp/project".to_string()),
            responder_username: Some("GitHub Copilot".to_string()),
            requests: Vec::new(),
        };

        assert!(is_github_copilot(&session));
        assert!(to_usage_entries(&session, Path::new("empty-session.jsonl")).is_empty());
    }
}
