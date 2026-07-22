use crate::db::{ContextStats, TokenStats, UsageEntry};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::Value;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub(crate) const CONTEXT_SOURCE_KIND: &str = "grok-build-context";
pub(crate) const USAGE_SOURCE_KIND: &str = "grok-build-usage";

#[derive(Debug, Default, Clone)]
struct SessionMetadata {
    session_id: String,
    cwd: Option<String>,
    model: Option<String>,
    version: Option<String>,
    session_name: Option<String>,
    reasoning_effort: Option<String>,
}

#[derive(Default)]
struct TurnAccumulator {
    turn_no: u32,
    timestamp: String,
    model: Option<String>,
    reasoning_effort: Option<String>,
    usage: Option<TokenStats>,
    context_tokens: u64,
    reported_cost_usd: Option<f64>,
}

fn value_as_u64(value: Option<&Value>) -> Option<u64> {
    value.and_then(|value| {
        value
            .as_u64()
            .or_else(|| value.as_i64().and_then(|number| u64::try_from(number).ok()))
    })
}

fn number_from_keys(value: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|key| value_as_u64(value.get(*key)))
}

fn float_from_keys(value: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| {
        value.get(*key).and_then(|value| {
            value
                .as_f64()
                .or_else(|| value.as_u64().map(|number| number as f64))
        })
    })
}

fn parse_reported_cost(value: &Value) -> Option<f64> {
    float_from_keys(
        value,
        &["total_cost_usd", "totalCostUsd", "costUSD", "costUsd"],
    )
    .or_else(|| {
        float_from_keys(value, &["total_cost_usd_ticks", "totalCostUsdTicks"])
            .map(|ticks| ticks / 10_000_000_000.0)
    })
}

fn parse_model_usage_cost(value: &Value) -> Option<f64> {
    let costs: Vec<f64> = value
        .as_object()?
        .values()
        .filter_map(parse_reported_cost)
        .collect();
    (!costs.is_empty()).then(|| costs.into_iter().sum())
}

fn parse_token_stats(value: &Value) -> Option<TokenStats> {
    let input = number_from_keys(value, &["input_tokens", "inputTokens"]);
    let cache_read = number_from_keys(
        value,
        &[
            "cache_read_input_tokens",
            "cacheReadInputTokens",
            "cachedReadTokens",
            "cached_input_tokens",
            "cachedInputTokens",
        ],
    );
    let cache_write = number_from_keys(
        value,
        &["cache_write_input_tokens", "cacheWriteInputTokens"],
    );
    let output = number_from_keys(value, &["output_tokens", "outputTokens"]);
    let reasoning = number_from_keys(value, &["reasoning_tokens", "reasoningTokens"]);

    if input.is_none()
        && cache_read.is_none()
        && cache_write.is_none()
        && output.is_none()
        && reasoning.is_none()
    {
        return None;
    }

    let input = input.unwrap_or(0);
    let cache_read = cache_read.unwrap_or(0);
    let output = output.unwrap_or(0);
    // Grok's input token count already includes cached reads; when the
    // provider omits totalTokens, input + output preserves the full total.
    let total = number_from_keys(value, &["total_tokens", "totalTokens"])
        .unwrap_or_else(|| input.saturating_add(output));

    Some(TokenStats {
        input,
        output,
        cache_read: (cache_read > 0).then_some(cache_read),
        cache_write: cache_write.filter(|value| *value > 0),
        reasoning: reasoning.filter(|value| *value > 0),
        total,
    })
}

fn parse_model_usage(value: &Value) -> Option<(TokenStats, Option<String>)> {
    let models = value.as_object()?;
    let mut total = TokenStats {
        input: 0,
        output: 0,
        cache_read: None,
        cache_write: None,
        reasoning: None,
        total: 0,
    };
    let mut matched_model = None;
    let mut found = false;

    for (model, usage) in models {
        let Some(stats) = parse_token_stats(usage) else {
            continue;
        };
        found = true;
        matched_model.get_or_insert_with(|| model.clone());
        total.input = total.input.saturating_add(stats.input);
        total.output = total.output.saturating_add(stats.output);
        total.total = total.total.saturating_add(stats.total);
        total.cache_read = Some(
            total
                .cache_read
                .unwrap_or(0)
                .saturating_add(stats.cache_read.unwrap_or(0)),
        );
        total.cache_write = Some(
            total
                .cache_write
                .unwrap_or(0)
                .saturating_add(stats.cache_write.unwrap_or(0)),
        );
        total.reasoning = Some(
            total
                .reasoning
                .unwrap_or(0)
                .saturating_add(stats.reasoning.unwrap_or(0)),
        );
    }

    if found {
        Some((total, matched_model))
    } else {
        None
    }
}

/// Grok reports cached reads as a subset of `inputTokens`, while
/// `totalTokens` includes that cached portion. Keep cache reads in their own
/// field and store non-cached input separately, while preserving the provider
/// total as the complete processed-token count.
fn normalize_provider_token_stats(mut stats: TokenStats) -> TokenStats {
    let cache_read = stats.cache_read.unwrap_or(0);
    if cache_read > 0 && stats.input >= cache_read {
        stats.input = stats.input.saturating_sub(cache_read);
    }
    stats
}

fn usage_from_container(value: &Value) -> Option<(TokenStats, Option<String>)> {
    if let Some(usage) = value.get("usage").and_then(parse_token_stats) {
        return Some((normalize_provider_token_stats(usage), None));
    }
    if let Some(usage) = value.get("modelUsage").and_then(parse_model_usage) {
        return Some((normalize_provider_token_stats(usage.0), usage.1));
    }
    if let Some(usage) = value.get("model_usage").and_then(parse_model_usage) {
        return Some((normalize_provider_token_stats(usage.0), usage.1));
    }
    parse_token_stats(value).map(|stats| (normalize_provider_token_stats(stats), None))
}

fn extract_usage(
    line: &Value,
    update: &Value,
    params: &Value,
) -> Option<(TokenStats, Option<String>)> {
    [line, update, params]
        .into_iter()
        .find_map(usage_from_container)
        .or_else(|| params.get("_meta").and_then(usage_from_container))
        .or_else(|| update.get("_meta").and_then(usage_from_container))
}

fn extract_reported_cost(line: &Value, update: &Value, params: &Value) -> Option<f64> {
    [line, update, params]
        .into_iter()
        .find_map(|value| {
            parse_reported_cost(value).or_else(|| {
                value
                    .get("modelUsage")
                    .and_then(parse_model_usage_cost)
                    .or_else(|| value.get("model_usage").and_then(parse_model_usage_cost))
            })
        })
        .or_else(|| params.get("_meta").and_then(parse_reported_cost))
        .or_else(|| update.get("_meta").and_then(parse_reported_cost))
}

fn extract_model(line: &Value, update: &Value, params: &Value) -> Option<String> {
    [update, line, params]
        .into_iter()
        .find_map(|value| {
            ["model", "model_id", "modelId", "current_model_id"]
                .into_iter()
                .find_map(|key| value.get(key).and_then(Value::as_str).map(str::to_string))
        })
        .or_else(|| params.get("_meta").and_then(extract_model_from_value))
        .or_else(|| update.get("_meta").and_then(extract_model_from_value))
}

fn extract_model_from_value(value: &Value) -> Option<String> {
    ["model", "model_id", "modelId", "current_model_id"]
        .into_iter()
        .find_map(|key| value.get(key).and_then(Value::as_str).map(str::to_string))
}

fn normalize_reasoning_effort(value: &str) -> Option<String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "low" => Some("Low".to_string()),
        "medium" => Some("Medium".to_string()),
        "high" => Some("High".to_string()),
        _ => None,
    }
}

fn extract_reasoning_effort_from_value(value: &Value) -> Option<String> {
    ["reasoning_effort", "reasoningEffort", "effort"]
        .into_iter()
        .find_map(|key| value.get(key).and_then(Value::as_str))
        .and_then(normalize_reasoning_effort)
}

fn extract_reasoning_effort(line: &Value, update: &Value, params: &Value) -> Option<String> {
    [line, update, params]
        .into_iter()
        .find_map(extract_reasoning_effort_from_value)
        .or_else(|| {
            params
                .get("_meta")
                .and_then(extract_reasoning_effort_from_value)
        })
        .or_else(|| {
            update
                .get("_meta")
                .and_then(extract_reasoning_effort_from_value)
        })
}

fn normalize_model_id(model: &str) -> String {
    model
        .to_ascii_lowercase()
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect()
}

fn is_grok45_model_id(model: &str) -> bool {
    matches!(
        normalize_model_id(model).as_str(),
        "grok45"
            | "grok45latest"
            | "grok45build"
            | "grokbuildlatest"
            | "grokbuild"
            | "grokbuild01"
            | "grokcodefast1"
            | "grokcodefast"
            | "grokcodefast10825"
    )
}

fn display_model_name(model: &str, reasoning_effort: Option<&str>) -> String {
    if is_grok45_model_id(model) {
        if let Some(effort) = reasoning_effort.and_then(normalize_reasoning_effort) {
            return format!("Grok 4.5 ({effort})");
        }
        return "Grok 4.5".to_string();
    }

    model.trim().to_string()
}

fn update_value(line: &Value) -> &Value {
    line.get("params")
        .and_then(|params| params.get("update"))
        .unwrap_or(line)
}

fn update_params(line: &Value) -> &Value {
    line.get("params").unwrap_or(&Value::Null)
}

fn update_type(update: &Value) -> &str {
    update
        .get("sessionUpdate")
        .or_else(|| update.get("session_update"))
        .or_else(|| update.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

fn number_from_update(update: &Value, params: &Value, key: &str) -> Option<u32> {
    value_as_u64(update.get(key))
        .or_else(|| {
            params
                .get("_meta")
                .and_then(|meta| value_as_u64(meta.get(key)))
        })
        .and_then(|value| u32::try_from(value).ok())
}

fn timestamp_from_value(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if let Some(text) = value.as_str() {
        if DateTime::parse_from_rfc3339(text).is_ok() {
            return Some(text.to_string());
        }
        if let Ok(seconds) = text.parse::<f64>() {
            return timestamp_from_seconds(seconds);
        }
    }
    value.as_f64().and_then(timestamp_from_seconds)
}

fn timestamp_from_seconds(seconds: f64) -> Option<String> {
    if !seconds.is_finite() {
        return None;
    }
    let whole_seconds = seconds.trunc() as i64;
    let nanos = ((seconds.fract().abs()) * 1_000_000_000.0).round() as u32;
    DateTime::<Utc>::from_timestamp(whole_seconds, nanos)
        .map(|date| date.to_rfc3339_opts(SecondsFormat::Millis, true))
}

pub(crate) fn timestamp_to_rfc3339(value: Option<&Value>) -> String {
    timestamp_from_value(value).unwrap_or_default()
}

pub(crate) fn value_to_text(value: Option<&Value>) -> String {
    let Some(value) = value else {
        return String::new();
    };
    if let Some(text) = value.as_str() {
        return text.to_string();
    }
    if let Some(text) = value.get("text").and_then(Value::as_str) {
        return text.to_string();
    }
    if let Some(text) = value.get("content").and_then(Value::as_str) {
        return text.to_string();
    }
    if let Some(content) = value.get("content") {
        let text = value_to_text(Some(content));
        if !text.is_empty() {
            return text;
        }
    }
    if let Some(message) = value.get("message") {
        let text = value_to_text(Some(message));
        if !text.is_empty() {
            return text;
        }
    }
    if let Some(items) = value.as_array() {
        return items
            .iter()
            .map(|item| value_to_text(Some(item)))
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
    }
    String::new()
}

fn trim_session_name(text: &str) -> Option<String> {
    let normalized = text.trim().replace(['\r', '\n'], " ");
    if normalized.is_empty() {
        None
    } else {
        Some(normalized.chars().take(100).collect())
    }
}

fn read_session_metadata(updates_path: &Path) -> SessionMetadata {
    let session_dir = updates_path.parent().unwrap_or_else(|| Path::new("."));
    let session_id = session_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_string();
    let summary_path = session_dir.join("summary.json");
    let summary = fs::read_to_string(summary_path)
        .ok()
        .and_then(|content| serde_json::from_str::<Value>(&content).ok());

    let info = summary.as_ref().and_then(|value| value.get("info"));
    let cwd = info
        .and_then(|value| value.get("cwd"))
        .or_else(|| summary.as_ref().and_then(|value| value.get("cwd")))
        .and_then(Value::as_str)
        .map(str::to_string);
    let model = summary
        .as_ref()
        .and_then(|value| value.get("current_model_id").or_else(|| value.get("model")))
        .and_then(Value::as_str)
        .map(str::to_string);
    let version = summary
        .as_ref()
        .and_then(|value| value.get("version"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let reasoning_effort = summary
        .as_ref()
        .and_then(|value| {
            value
                .get("reasoning_effort")
                .or_else(|| value.get("reasoningEffort"))
                .or_else(|| value.get("effort"))
        })
        .and_then(Value::as_str)
        .and_then(normalize_reasoning_effort);
    let session_name = summary.as_ref().and_then(|value| {
        value
            .get("generated_title")
            .or_else(|| value.get("session_summary"))
            .and_then(Value::as_str)
            .and_then(trim_session_name)
    });

    SessionMetadata {
        session_id,
        cwd,
        model,
        version,
        session_name,
        reasoning_effort,
    }
}

fn turn_number(
    update: &Value,
    params: &Value,
    next_turn: u32,
    zero_based: &mut Option<bool>,
) -> Option<u32> {
    if let Some(raw) = number_from_update(update, params, "turn_number") {
        let is_zero_based = zero_based.get_or_insert(raw == 0);
        return raw
            .checked_add(u32::from(*is_zero_based))
            .or(Some(next_turn.max(1)));
    }
    number_from_update(update, params, "turnNo")
        .or_else(|| number_from_update(update, params, "turnNumber"))
        .or_else(|| number_from_update(update, params, "turn"))
        .or(Some(next_turn.max(1)))
}

fn finalize_turn(
    metadata: &SessionMetadata,
    turn: TurnAccumulator,
    updates_path: &Path,
) -> Option<UsageEntry> {
    let has_provider_usage = turn.usage.is_some() || turn.reported_cost_usd.is_some();
    let stats = turn
        .usage
        .or_else(|| {
            (turn.context_tokens > 0).then_some(TokenStats {
                input: turn.context_tokens,
                output: 0,
                cache_read: None,
                cache_write: None,
                reasoning: None,
                total: turn.context_tokens,
            })
        })
        .or_else(|| {
            turn.reported_cost_usd.map(|_| TokenStats {
                input: 0,
                output: 0,
                cache_read: None,
                cache_write: None,
                reasoning: None,
                total: 0,
            })
        })?;
    let timestamp = if turn.timestamp.is_empty() {
        "1970-01-01T00:00:00.000Z".to_string()
    } else {
        turn.timestamp
    };
    let model_id = turn
        .model
        .or_else(|| metadata.model.clone())
        .unwrap_or_else(|| "grok-4.5".to_string());
    let reasoning_effort = turn
        .reasoning_effort
        .or_else(|| metadata.reasoning_effort.clone());
    let model = display_model_name(&model_id, reasoning_effort.as_deref());
    let source_kind = if has_provider_usage {
        USAGE_SOURCE_KIND
    } else {
        CONTEXT_SOURCE_KIND
    };

    Some(UsageEntry {
        timestamp,
        session_id: metadata.session_id.clone(),
        session_name: metadata.session_name.clone(),
        transcript_path: Some(updates_path.to_string_lossy().into_owned()),
        cwd: metadata.cwd.clone(),
        version: metadata.version.clone(),
        turn_no: turn.turn_no.max(1),
        model: Some(model),
        model_id: Some(model_id),
        tokens: Some(stats.clone()),
        delta_tokens: Some(stats),
        context: Some(ContextStats {
            current_context_tokens: (turn.context_tokens > 0).then_some(turn.context_tokens),
            displayed_context_limit: None,
            current_context_used_percentage: None,
        }),
        cost: turn
            .reported_cost_usd
            .map(|reported_cost_usd| crate::db::CostStats {
                total_api_duration_ms: None,
                total_duration_ms: None,
                total_premium_requests: None,
                reported_cost_usd: Some(reported_cost_usd),
            }),
        source_kind: Some(source_kind.to_string()),
        parent_session_id: None,
        agent_nickname: None,
        agent_role: None,
        reasoning_effort,
    })
}

/// Keep the full context snapshot for display, but only count the increase
/// from the previous context snapshot in aggregates and cost estimation.
/// Provider usage entries are left untouched; their context snapshot only
/// advances the baseline for a later context-only entry in the same session.
fn normalize_context_snapshot_deltas(entries: &mut [UsageEntry]) {
    let mut previous_context_tokens = None;

    for entry in entries {
        let Some(current_context_tokens) = entry
            .context
            .as_ref()
            .and_then(|context| context.current_context_tokens)
        else {
            continue;
        };

        if entry.source_kind.as_deref() == Some(CONTEXT_SOURCE_KIND) {
            let delta = previous_context_tokens
                .map(|previous| current_context_tokens.saturating_sub(previous))
                .unwrap_or(current_context_tokens);

            if let Some(snapshot) = entry.tokens.as_ref() {
                let mut delta_tokens = snapshot.clone();
                delta_tokens.input = delta;
                delta_tokens.output = 0;
                delta_tokens.cache_read = None;
                delta_tokens.cache_write = None;
                delta_tokens.reasoning = None;
                delta_tokens.total = delta;
                entry.delta_tokens = Some(delta_tokens);
            }
        }

        previous_context_tokens = Some(current_context_tokens);
    }
}

pub(crate) fn find_session_update_files(grok_dir: &Path) -> Vec<PathBuf> {
    fn visit(directory: &Path, files: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                visit(&path, files);
            } else if file_type.is_file()
                && path.file_name().and_then(|name| name.to_str()) == Some("updates.jsonl")
            {
                files.push(path);
            }
        }
    }

    let mut files = Vec::new();
    let sessions_dir = grok_dir.join("sessions");
    if sessions_dir.is_dir() {
        visit(&sessions_dir, &mut files);
    }
    files.sort();
    files
}

pub(crate) fn parse_session_usage_file(updates_path: &Path) -> Result<Vec<UsageEntry>, String> {
    let file = File::open(updates_path).map_err(|error| {
        format!(
            "無法開啟 Grok Build session 檔案 {:?}: {error}",
            updates_path
        )
    })?;
    let reader = BufReader::new(file);
    let metadata = read_session_metadata(updates_path);
    let mut entries = Vec::new();
    let mut current: Option<TurnAccumulator> = None;
    let mut next_turn = 1u32;
    let mut zero_based = None;
    let mut session_name = metadata.session_name.clone();

    for line in reader.lines() {
        let Ok(line) = line else {
            continue;
        };
        let Ok(event) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let update = update_value(&event);
        let params = update_params(&event);
        let update_type = update_type(update);
        let timestamp = timestamp_to_rfc3339(event.get("timestamp"));
        let model = extract_model(&event, update, params);
        let reasoning_effort = extract_reasoning_effort(&event, update, params);

        if matches!(update_type, "turn_started" | "user_message_chunk") && current.is_none() {
            let turn_no =
                turn_number(update, params, next_turn, &mut zero_based).unwrap_or(next_turn);
            current = Some(TurnAccumulator {
                turn_no,
                timestamp: timestamp.clone(),
                model: model.clone(),
                reasoning_effort: reasoning_effort.clone(),
                ..Default::default()
            });
            next_turn = turn_no.saturating_add(1);
        }

        if let Some(current_turn) = current.as_mut() {
            if !timestamp.is_empty() {
                current_turn.timestamp = timestamp;
            }
            if model.is_some() {
                current_turn.model = model;
            }
            if reasoning_effort.is_some() {
                current_turn.reasoning_effort = reasoning_effort;
            }
            if let Some((usage, usage_model)) = extract_usage(&event, update, params) {
                current_turn.usage = Some(usage);
                if usage_model.is_some() {
                    current_turn.model = usage_model;
                }
            }
            if let Some(reported_cost_usd) = extract_reported_cost(&event, update, params) {
                current_turn.reported_cost_usd = Some(reported_cost_usd);
            }
            if let Some(context_tokens) = number_from_update(update, params, "totalTokens") {
                current_turn.context_tokens =
                    current_turn.context_tokens.max(context_tokens as u64);
            }
        }

        if update_type == "user_message_chunk" {
            let content = value_to_text(
                update
                    .get("content")
                    .or_else(|| update.get("chunk"))
                    .or_else(|| update.get("message")),
            );
            if session_name.is_none() {
                session_name = trim_session_name(&content);
            }
        }

        if update_type == "turn_completed" {
            if let Some(turn) = current.take() {
                let mut entry_metadata = metadata.clone();
                entry_metadata.session_name = session_name.clone();
                if let Some(entry) = finalize_turn(&entry_metadata, turn, updates_path) {
                    entries.push(entry);
                }
            }
        }
    }

    if let Some(turn) = current.take() {
        let mut entry_metadata = metadata;
        entry_metadata.session_name = session_name;
        if let Some(entry) = finalize_turn(&entry_metadata, turn, updates_path) {
            entries.push(entry);
        }
    }

    normalize_context_snapshot_deltas(&mut entries);
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_updates_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "token_usage_insights_grok_{name}_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn parses_provider_usage_and_model_usage() {
        let root = test_updates_path("usage");
        let session_dir = root.join("sessions/work/session-1");
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(
            session_dir.join("summary.json"),
            r#"{"info":{"cwd":"/tmp/project"},"current_model_id":"grok-4.5","reasoning_effort":"high","generated_title":"Usage test"}"#,
        )
        .unwrap();
        fs::write(
            session_dir.join("updates.jsonl"),
            concat!(
                r#"{"timestamp":1710000000,"params":{"update":{"sessionUpdate":"turn_started","turn_number":0}}}"#, "\n",
                r#"{"timestamp":1710000001,"params":{"update":{"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"hello"}}}}"#, "\n",
                r#"{"timestamp":1710000002,"params":{"update":{"sessionUpdate":"turn_completed","usage":{"inputTokens":100,"cachedReadTokens":20,"outputTokens":30,"reasoningTokens":4,"totalTokens":130},"modelUsage":{"grok-4.5":{"inputTokens":100,"cachedReadTokens":20,"outputTokens":30,"reasoningTokens":4,"costUSD":0.0123}},"total_cost_usd":0.0123}}}"#, "\n"
            ),
        )
        .unwrap();

        let entries = parse_session_usage_file(&session_dir.join("updates.jsonl")).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].turn_no, 1);
        assert_eq!(entries[0].source_kind.as_deref(), Some(USAGE_SOURCE_KIND));
        assert_eq!(entries[0].tokens.as_ref().unwrap().input, 80);
        assert_eq!(entries[0].tokens.as_ref().unwrap().cache_read, Some(20));
        assert_eq!(entries[0].tokens.as_ref().unwrap().total, 130);
        let tokens = entries[0].tokens.as_ref().unwrap();
        assert_eq!(
            tokens.input + tokens.cache_read.unwrap_or(0) + tokens.output,
            tokens.total
        );
        assert_eq!(entries[0].model.as_deref(), Some("Grok 4.5 (High)"));
        assert_eq!(entries[0].model_id.as_deref(), Some("grok-4.5"));
        assert_eq!(entries[0].reasoning_effort.as_deref(), Some("High"));
        assert_eq!(entries[0].session_name.as_deref(), Some("Usage test"));
        assert_eq!(
            entries[0]
                .cost
                .as_ref()
                .and_then(|cost| cost.reported_cost_usd),
            Some(0.0123)
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn normalizes_cached_input_from_model_usage() {
        let value = serde_json::json!({
            "modelUsage": {
                "grok-4.5": {
                    "inputTokens": 100,
                    "cachedReadTokens": 20,
                    "outputTokens": 30
                }
            }
        });

        let (stats, model) = usage_from_container(&value).unwrap();
        assert_eq!(model.as_deref(), Some("grok-4.5"));
        assert_eq!(stats.input, 80);
        assert_eq!(stats.cache_read, Some(20));
        assert_eq!(stats.total, 130);
    }

    #[test]
    fn preserves_input_when_cached_read_exceeds_input() {
        let stats = normalize_provider_token_stats(TokenStats {
            input: 10,
            output: 3,
            cache_read: Some(20),
            cache_write: None,
            reasoning: None,
            total: 13,
        });

        assert_eq!(stats.input, 10);
        assert_eq!(stats.cache_read, Some(20));
        assert_eq!(stats.total, 13);
    }

    #[test]
    fn normalizes_grok_build_model_aliases_and_reasoning_effort() {
        assert_eq!(
            display_model_name("grok-4.5", Some("low")),
            "Grok 4.5 (Low)"
        );
        assert_eq!(
            display_model_name("grok-build-latest", Some("medium")),
            "Grok 4.5 (Medium)"
        );
        assert_eq!(display_model_name("grok-4.5", None), "Grok 4.5");
    }

    #[test]
    fn context_snapshot_deltas_are_incremental_across_turns() {
        let root = test_updates_path("context");
        let session_dir = root.join("sessions/work/session-2");
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(
            session_dir.join("updates.jsonl"),
            concat!(
                r#"{"timestamp":1710000000,"params":{"_meta":{"totalTokens":1200},"update":{"sessionUpdate":"turn_started","turn_number":0}}}"#, "\n",
                r#"{"timestamp":1710000001,"params":{"_meta":{"totalTokens":1400},"update":{"sessionUpdate":"user_message_chunk","content":{"text":"context fallback"}}}}"#, "\n",
                r#"{"timestamp":1710000002,"params":{"_meta":{"totalTokens":1600},"update":{"sessionUpdate":"turn_completed"}}}"#, "\n",
                r#"{"timestamp":1710000003,"params":{"_meta":{"totalTokens":2600},"update":{"sessionUpdate":"turn_started","turn_number":1}}}"#, "\n",
                r#"{"timestamp":1710000004,"params":{"_meta":{"totalTokens":3200},"update":{"sessionUpdate":"user_message_chunk","content":{"text":"second turn"}}}}"#, "\n",
                r#"{"timestamp":1710000005,"params":{"_meta":{"totalTokens":3200},"update":{"sessionUpdate":"turn_completed"}}}"#, "\n",
                r#"{"timestamp":1710000006,"params":{"_meta":{"totalTokens":4300},"update":{"sessionUpdate":"turn_started","turn_number":2}}}"#, "\n",
                r#"{"timestamp":1710000007,"params":{"_meta":{"totalTokens":5000},"update":{"sessionUpdate":"turn_completed"}}}"#, "\n"
            ),
        )
        .unwrap();

        let entries = parse_session_usage_file(&session_dir.join("updates.jsonl")).unwrap();
        assert_eq!(entries.len(), 3);
        let snapshots: Vec<u64> = entries
            .iter()
            .map(|entry| entry.tokens.as_ref().unwrap().total)
            .collect();
        let deltas: Vec<u64> = entries
            .iter()
            .map(|entry| entry.delta_tokens.as_ref().unwrap().total)
            .collect();

        assert_eq!(snapshots, [1600, 3200, 5000]);
        assert_eq!(deltas, [1600, 1600, 1800]);
        assert_eq!(deltas.iter().sum::<u64>(), 5000);
        assert!(entries
            .iter()
            .all(|entry| entry.source_kind.as_deref() == Some(CONTEXT_SOURCE_KIND)));

        let _ = fs::remove_dir_all(root);
    }
}
