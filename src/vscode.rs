use crate::db::{CostStats, InitialUserPromptSelector, TokenStats, UsageEntry};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub const SOURCE_KIND: &str = "vscode-chat";
const SESSION_ID_PREFIX: &str = "vscode-";

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
    summary_usages: Vec<ChatUsage>,
    debug_usages: Vec<ChatUsage>,
}

#[derive(Debug, Clone)]
struct ChatUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
    cached_tokens: Option<u64>,
}

#[derive(Debug, Clone)]
struct TimedChatUsage {
    timestamp: i64,
    usage: ChatUsage,
}

#[derive(Debug, Deserialize)]
struct SerializedAgentDebugEntry {
    #[serde(default)]
    ts: Option<i64>,
    #[serde(default)]
    #[serde(rename = "type")]
    entry_type: Option<String>,
    #[serde(default)]
    attrs: Option<SerializedAgentDebugAttrs>,
}

#[derive(Debug, Deserialize)]
struct SerializedAgentDebugAttrs {
    #[serde(default)]
    #[serde(rename = "inputTokens")]
    input_tokens: Option<u64>,
    #[serde(default)]
    #[serde(rename = "outputTokens")]
    output_tokens: Option<u64>,
    #[serde(default)]
    #[serde(rename = "cachedTokens")]
    cached_tokens: Option<u64>,
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
    result: Option<Value>,
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
        serde_json::from_str(&content)
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

    let mut requests: Vec<ChatRequest> = serialized
        .requests
        .into_iter()
        .map(deserialize_request)
        .collect();
    attach_agent_debug_usages(path, &session_id, &mut requests);

    Ok(ChatSession {
        session_id,
        creation_date: serialized.creation_date,
        initial_location: serialized.initial_location,
        working_directory: serialized.working_directory,
        responder_username: serialized.responder_username,
        requests,
    })
}

pub fn agent_debug_log_path(session_path: &Path, session_id: &str) -> Option<PathBuf> {
    let chat_sessions_dir = session_path.parent()?;
    if chat_sessions_dir.file_name()?.to_str()? != "chatSessions" {
        return None;
    }
    let workspace_dir = chat_sessions_dir.parent()?;
    Some(
        workspace_dir
            .join("GitHub.copilot-chat")
            .join("debug-logs")
            .join(session_id)
            .join("main.jsonl"),
    )
}

pub fn session_source_metadata(path: &Path, session_id: &str) -> Result<(u64, i64), String> {
    let chat_metadata = fs::metadata(path)
        .map_err(|error| format!("無法讀取 VS Code 聊天檔案資訊 {:?}: {error}", path))?;
    let mut total_size = chat_metadata.len();
    let mut latest_modified = modified_time_nanos(&chat_metadata);

    if let Some(debug_path) = agent_debug_log_path(path, session_id) {
        if let Ok(debug_metadata) = fs::metadata(debug_path) {
            total_size = total_size.saturating_add(debug_metadata.len());
            latest_modified = latest_modified.max(modified_time_nanos(&debug_metadata));
        }
    }

    Ok((total_size, latest_modified))
}

fn modified_time_nanos(metadata: &fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|value| value.as_nanos() as i64)
        .unwrap_or(0)
}

fn attach_agent_debug_usages(path: &Path, session_id: &str, requests: &mut [ChatRequest]) {
    let Some(debug_path) = agent_debug_log_path(path, session_id) else {
        return;
    };
    let Ok(file) = fs::File::open(debug_path) else {
        return;
    };

    let mut usages = BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| parse_agent_debug_usage(&line))
        .collect::<Vec<_>>();
    usages.sort_by_key(|usage| usage.timestamp);

    for timed_usage in usages {
        let request_index = requests
            .iter()
            .enumerate()
            .filter_map(|(index, request)| {
                request
                    .timestamp
                    .filter(|timestamp| *timestamp <= timed_usage.timestamp)
                    .map(|timestamp| (index, timestamp))
            })
            .max_by_key(|(_, timestamp)| *timestamp)
            .map(|(index, _)| index)
            // Session 建立後、第一筆聊天請求寫入前也可能已有 Agent 呼叫。
            .or_else(|| (!requests.is_empty()).then_some(0));

        if let Some(index) = request_index {
            requests[index].debug_usages.push(timed_usage.usage);
        }
    }
}

fn parse_agent_debug_usage(line: &str) -> Option<TimedChatUsage> {
    let entry: SerializedAgentDebugEntry = serde_json::from_str(line).ok()?;
    if entry.entry_type.as_deref() != Some("llm_request") {
        return None;
    }
    let timestamp = entry.ts?;
    let attrs = entry.attrs?;
    let prompt_tokens = attrs.input_tokens;
    let completion_tokens = attrs.output_tokens;
    let cached_tokens = attrs.cached_tokens;
    if prompt_tokens.is_none() && completion_tokens.is_none() && cached_tokens.is_none() {
        return None;
    }

    Some(TimedChatUsage {
        timestamp,
        usage: ChatUsage {
            prompt_tokens: prompt_tokens.unwrap_or(0),
            completion_tokens: completion_tokens.unwrap_or(0),
            cached_tokens,
        },
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

    session
        .requests
        .iter()
        .enumerate()
        .map(|(index, request)| {
            let tokens = token_stats(request);
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
        let entry: OperationLogEntry = serde_json::from_str(line)
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

fn deserialize_request(request: SerializedChatRequest) -> ChatRequest {
    let summary_usages = summary_usages(request.result.as_ref());
    ChatRequest {
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
        summary_usages,
        debug_usages: Vec::new(),
    }
}

fn summary_usages(result: Option<&Value>) -> Vec<ChatUsage> {
    result
        .and_then(|value| value.get("metadata"))
        .and_then(|metadata| metadata.get("summaries"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|summary| summary.get("usage"))
        .filter_map(Value::as_object)
        .map(|usage| ChatUsage {
            prompt_tokens: usage
                .get("prompt_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            completion_tokens: usage
                .get("completion_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            cached_tokens: usage
                .get("prompt_tokens_details")
                .and_then(|details| details.get("cached_tokens"))
                .and_then(Value::as_u64),
        })
        .collect()
}

fn token_stats(request: &ChatRequest) -> Option<TokenStats> {
    if !request.debug_usages.is_empty() {
        return aggregate_usages(&request.debug_usages);
    }

    if request.prompt_tokens.is_none()
        && request.completion_tokens.is_none()
        && request.summary_usages.is_empty()
    {
        return None;
    }

    let mut input = request.prompt_tokens.unwrap_or(0);
    let mut output = request.completion_tokens.unwrap_or(0);
    let mut cache_read = 0u64;
    let mut has_cache_read = false;

    for usage in &request.summary_usages {
        let cached = usage
            .cached_tokens
            .map(|value| value.min(usage.prompt_tokens));
        if let Some(cached) = cached {
            has_cache_read = true;
            cache_read = cache_read.saturating_add(cached);
        }
        input = input.saturating_add(usage.prompt_tokens.saturating_sub(cached.unwrap_or(0)));
        output = output.saturating_add(usage.completion_tokens);
    }

    let total = input.saturating_add(output).saturating_add(cache_read);
    Some(TokenStats {
        input,
        output,
        cache_read: has_cache_read.then_some(cache_read),
        cache_write: None,
        reasoning: None,
        total,
    })
}

fn aggregate_usages(usages: &[ChatUsage]) -> Option<TokenStats> {
    if usages.is_empty() {
        return None;
    }

    let mut input = 0u64;
    let mut output = 0u64;
    let mut cache_read = 0u64;
    let mut has_cache_read = false;
    for usage in usages {
        let cached = usage
            .cached_tokens
            .map(|value| value.min(usage.prompt_tokens));
        if let Some(cached) = cached {
            has_cache_read = true;
            cache_read = cache_read.saturating_add(cached);
        }
        input = input.saturating_add(usage.prompt_tokens.saturating_sub(cached.unwrap_or(0)));
        output = output.saturating_add(usage.completion_tokens);
    }

    Some(TokenStats {
        input,
        output,
        cache_read: has_cache_read.then_some(cache_read),
        cache_write: None,
        reasoning: None,
        total: input.saturating_add(output).saturating_add(cache_read),
    })
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

    fn unique_temp_dir(name: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "token-usage-insights-{name}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn replays_vscode_operation_log() {
        let content = concat!(
            r#"{"kind":0,"v":{"sessionId":"abc","requests":[]}}"#,
            "\n",
            r#"{"kind":2,"k":["requests"],"v":[{"requestId":"r1","message":{"text":"hello"}}]}"#,
            "\n",
            r#"{"kind":1,"k":["requests",0,"promptTokens"],"v":12}"#,
            "\n",
            r#"{"kind":1,"k":["requests",0,"result"],"v":{"metadata":{"summaries":[{"usage":{"prompt_tokens":10,"completion_tokens":2,"prompt_tokens_details":{"cached_tokens":8}}}]}}}"#,
            "\n",
            r#"{"kind":3,"k":["requests",0,"requestId"]}"#,
            "\n"
        );
        let value = replay_operation_log(content).expect("operation log should replay");
        assert_eq!(value["sessionId"], "abc");
        assert_eq!(value["requests"][0]["promptTokens"], 12);
        assert_eq!(
            value["requests"][0]["result"]["metadata"]["summaries"][0]["usage"]
                ["prompt_tokens_details"]["cached_tokens"],
            8
        );
        assert!(value["requests"][0].get("requestId").is_none());
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
            requests: raw.requests.into_iter().map(deserialize_request).collect(),
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
    fn maps_summarization_usage_and_cached_tokens() {
        let request: SerializedChatRequest = serde_json::from_value(serde_json::json!({
            "promptTokens": 68_331,
            "completionTokens": 29_589,
            "result": {
                "metadata": {
                    "summaries": [{
                        "usage": {
                            "prompt_tokens": 201_558,
                            "completion_tokens": 9_945,
                            "total_tokens": 211_503,
                            "prompt_tokens_details": {
                                "cached_tokens": 200_192
                            }
                        }
                    }]
                }
            }
        }))
        .expect("request with summarization usage should parse");

        let stats = token_stats(&deserialize_request(request))
            .expect("summarization usage should produce token stats");

        assert_eq!(stats.input, 69_697);
        assert_eq!(stats.output, 39_534);
        assert_eq!(stats.cache_read, Some(200_192));
        assert_eq!(stats.total, 309_423);
    }

    #[test]
    fn maps_agent_debug_usage_to_requests_and_prefers_it_over_chat_totals() {
        let workspace = unique_temp_dir("vscode-debug-usage");
        let session_id = "debug-session";
        let chat_dir = workspace.join("chatSessions");
        let debug_dir = workspace
            .join("GitHub.copilot-chat")
            .join("debug-logs")
            .join(session_id);
        fs::create_dir_all(&chat_dir).unwrap();
        fs::create_dir_all(&debug_dir).unwrap();

        let session_path = chat_dir.join(format!("{session_id}.json"));
        fs::write(
            &session_path,
            serde_json::json!({
                "sessionId": session_id,
                "responderUsername": "GitHub Copilot",
                "requests": [
                    {
                        "timestamp": 1_000,
                        "message": {"text": "first"},
                        "promptTokens": 999,
                        "completionTokens": 999,
                        "result": {
                            "metadata": {
                                "summaries": [{
                                    "usage": {
                                        "prompt_tokens": 999,
                                        "completion_tokens": 999,
                                        "prompt_tokens_details": {"cached_tokens": 999}
                                    }
                                }]
                            }
                        }
                    },
                    {
                        "timestamp": 2_000,
                        "message": {"text": "second"},
                        "promptTokens": 999,
                        "completionTokens": 999
                    }
                ]
            })
            .to_string(),
        )
        .unwrap();
        fs::write(
            debug_dir.join("main.jsonl"),
            concat!(
                r#"{"ts":900,"type":"llm_request","attrs":{"inputTokens":20,"outputTokens":2,"cachedTokens":10}}"#,
                "\n",
                r#"{"ts":1100,"type":"llm_request","attrs":{"inputTokens":100,"outputTokens":10,"cachedTokens":80}}"#,
                "\n",
                r#"{"ts":1200,"type":"llm_request","attrs":{"inputTokens":150,"outputTokens":20,"cachedTokens":100}}"#,
                "\n",
                r#"{"ts":2100,"type":"llm_request","attrs":{"inputTokens":200,"outputTokens":30,"cachedTokens":0}}"#,
                "\n"
            ),
        )
        .unwrap();

        let session = read_session_file(&session_path).expect("session should parse");
        let entries = to_usage_entries(&session, &session_path);

        assert_eq!(entries.len(), 2);
        let first = entries[0].tokens.as_ref().unwrap();
        assert_eq!(first.input, 80);
        assert_eq!(first.output, 32);
        assert_eq!(first.cache_read, Some(190));
        assert_eq!(first.total, 302);
        let second = entries[1].tokens.as_ref().unwrap();
        assert_eq!(second.input, 200);
        assert_eq!(second.output, 30);
        assert_eq!(second.cache_read, Some(0));
        assert_eq!(second.total, 230);

        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn session_source_metadata_includes_agent_debug_log() {
        let workspace = unique_temp_dir("vscode-debug-metadata");
        let session_id = "metadata-session";
        let chat_dir = workspace.join("chatSessions");
        let debug_dir = workspace
            .join("GitHub.copilot-chat")
            .join("debug-logs")
            .join(session_id);
        fs::create_dir_all(&chat_dir).unwrap();
        fs::create_dir_all(&debug_dir).unwrap();
        let session_path = chat_dir.join(format!("{session_id}.json"));
        fs::write(&session_path, "12345").unwrap();

        let without_debug = session_source_metadata(&session_path, session_id).unwrap();
        fs::write(debug_dir.join("main.jsonl"), "1234567").unwrap();
        let with_debug = session_source_metadata(&session_path, session_id).unwrap();

        assert_eq!(without_debug.0, 5);
        assert_eq!(with_debug.0, 12);

        let _ = fs::remove_dir_all(workspace);
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
                    summary_usages: Vec::new(),
                    debug_usages: Vec::new(),
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
                    summary_usages: Vec::new(),
                    debug_usages: Vec::new(),
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
                    summary_usages: Vec::new(),
                    debug_usages: Vec::new(),
                },
            ],
        };

        let entries = to_usage_entries(&session, Path::new("session.json"));

        assert!(entries
            .iter()
            .all(|entry| entry.session_name.as_deref() == Some("Second prompt")));
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
