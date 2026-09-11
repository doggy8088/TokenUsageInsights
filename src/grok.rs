use crate::db::{ContextStats, TokenStats, UsageEntry};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::Value;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub(crate) const CONTEXT_SOURCE_KIND: &str = "grok-build-context";
pub(crate) const USAGE_SOURCE_KIND: &str = "grok-build-usage";
pub(crate) const UNKNOWN_MODEL: &str = "Unknown Model";

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
    model_usages: Vec<ModelUsage>,
    context_tokens: u64,
    reported_cost_usd: Option<f64>,
}

#[derive(Debug, Clone)]
struct ModelUsage {
    model: String,
    stats: TokenStats,
    reported_cost_usd: Option<f64>,
}

#[derive(Debug, Clone)]
struct ParsedUsage {
    total: TokenStats,
    models: Vec<ModelUsage>,
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

fn has_true_flag(value: &Value, keys: &[&str]) -> bool {
    keys.iter()
        .any(|key| value.get(*key).and_then(Value::as_bool).unwrap_or(false))
}

fn costs_are_trusted(value: &Value) -> bool {
    !has_true_flag(
        value,
        &[
            "costIsPartial",
            "cost_is_partial",
            "usageIsIncomplete",
            "usage_is_incomplete",
        ],
    )
}

fn finite_nonnegative(value: f64) -> Option<f64> {
    (value.is_finite() && value >= 0.0).then_some(value)
}

fn parse_reported_cost(value: &Value) -> Option<f64> {
    if !costs_are_trusted(value) {
        return None;
    }

    float_from_keys(
        value,
        &[
            "costUsdTicks",
            "cost_usd_ticks",
            "total_cost_usd_ticks",
            "totalCostUsdTicks",
        ],
    )
    .map(|ticks| ticks / 10_000_000_000.0)
    .and_then(finite_nonnegative)
    .or_else(|| {
        float_from_keys(
            value,
            &["total_cost_usd", "totalCostUsd", "costUSD", "costUsd"],
        )
        .and_then(finite_nonnegative)
    })
}

fn parse_token_stats(value: &Value, input_includes_cache: bool) -> Option<TokenStats> {
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
    let total = number_from_keys(value, &["total_tokens", "totalTokens"]).unwrap_or_else(|| {
        let prompt = if input_includes_cache {
            input
        } else {
            input.saturating_add(cache_read)
        };
        prompt.saturating_add(output)
    });

    let stats = TokenStats {
        input,
        output,
        cache_read: (cache_read > 0).then_some(cache_read),
        cache_write: cache_write.filter(|value| *value > 0),
        cache_write_5m: None,
        cache_write_1h: None,
        reasoning: reasoning.filter(|value| *value > 0),
        total,
    };

    Some(if input_includes_cache {
        normalize_provider_token_stats(stats)
    } else {
        stats
    })
}

fn parse_model_usage(
    value: &Value,
    parent_costs_are_trusted: bool,
    input_includes_cache: bool,
) -> Option<Vec<ModelUsage>> {
    let models = value.as_object()?;
    let model_usages: Vec<ModelUsage> = models
        .iter()
        .filter_map(|(model, usage)| {
            let stats = parse_token_stats(usage, input_includes_cache)?;
            Some(ModelUsage {
                model: model.clone(),
                stats,
                reported_cost_usd: parent_costs_are_trusted
                    .then(|| parse_reported_cost(usage))
                    .flatten(),
            })
        })
        .collect();

    (!model_usages.is_empty()).then_some(model_usages)
}

fn sum_token_stats(model_usages: &[ModelUsage]) -> TokenStats {
    let mut total = TokenStats {
        input: 0,
        output: 0,
        cache_read: None,
        cache_write: None,
        cache_write_5m: None,
        cache_write_1h: None,
        reasoning: None,
        total: 0,
    };

    for usage in model_usages {
        total.input = total.input.saturating_add(usage.stats.input);
        total.output = total.output.saturating_add(usage.stats.output);
        total.total = total.total.saturating_add(usage.stats.total);
        total.cache_read = Some(
            total
                .cache_read
                .unwrap_or(0)
                .saturating_add(usage.stats.cache_read.unwrap_or(0)),
        );
        total.cache_write = Some(
            total
                .cache_write
                .unwrap_or(0)
                .saturating_add(usage.stats.cache_write.unwrap_or(0)),
        );
        total.reasoning = Some(
            total
                .reasoning
                .unwrap_or(0)
                .saturating_add(usage.stats.reasoning.unwrap_or(0)),
        );
    }

    total
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

fn usage_from_container(value: &Value) -> Option<ParsedUsage> {
    let nested_usage = value.get("usage");
    let usage_value = nested_usage.unwrap_or(value);
    // The official headless projection uses snake_case totals whose input is
    // already uncached, while ACP uses camelCase full input. `modelUsage`
    // remains camelCase in both surfaces, so its semantics must follow the
    // enclosing totals rather than its own key casing.
    let input_includes_cache = usage_value.get("input_tokens").is_none();
    let parent_costs_are_trusted = costs_are_trusted(value) && costs_are_trusted(usage_value);
    let model_usage = value
        .get("modelUsage")
        .or_else(|| value.get("model_usage"))
        .or_else(|| nested_usage.and_then(|usage| usage.get("modelUsage")))
        .or_else(|| nested_usage.and_then(|usage| usage.get("model_usage")));
    let models = model_usage
        .and_then(|value| parse_model_usage(value, parent_costs_are_trusted, input_includes_cache))
        .unwrap_or_default();
    let total = parse_token_stats(usage_value, input_includes_cache)
        .or_else(|| (!models.is_empty()).then(|| sum_token_stats(&models)))?;

    Some(ParsedUsage { total, models })
}

fn extract_usage(line: &Value, update: &Value, params: &Value) -> Option<ParsedUsage> {
    [line, update, params]
        .into_iter()
        .find_map(usage_from_container)
        .or_else(|| params.get("_meta").and_then(usage_from_container))
        .or_else(|| update.get("_meta").and_then(usage_from_container))
}

fn parse_model_usage_cost(value: &Value) -> Option<f64> {
    let costs: Vec<f64> = parse_model_usage(value, true, true)?
        .into_iter()
        .filter_map(|usage| usage.reported_cost_usd)
        .collect();
    (!costs.is_empty()).then(|| costs.into_iter().sum())
}

fn parse_reported_cost_from_container(value: &Value) -> Option<f64> {
    if !costs_are_trusted(value) {
        return None;
    }

    parse_reported_cost(value)
        .or_else(|| {
            value.get("usage").and_then(|usage| {
                if !costs_are_trusted(usage) {
                    return None;
                }
                parse_reported_cost(usage).or_else(|| {
                    usage
                        .get("modelUsage")
                        .and_then(parse_model_usage_cost)
                        .or_else(|| usage.get("model_usage").and_then(parse_model_usage_cost))
                })
            })
        })
        .or_else(|| {
            value
                .get("modelUsage")
                .and_then(parse_model_usage_cost)
                .or_else(|| value.get("model_usage").and_then(parse_model_usage_cost))
        })
        .or_else(|| {
            value
                .get("_meta")
                .and_then(parse_reported_cost_from_container)
        })
}

fn extract_reported_cost(line: &Value, update: &Value, params: &Value) -> Option<f64> {
    [line, update, params]
        .into_iter()
        .find_map(parse_reported_cost_from_container)
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
            | "grokcodefast1"
            | "grokcodefast"
            | "grokcodefast10825"
    )
}

fn is_grok46_model_id(model: &str) -> bool {
    matches!(
        normalize_model_id(model).as_str(),
        "grok46" | "grok46latest"
    )
}

fn is_grok_build_01_model_id(model: &str) -> bool {
    matches!(
        normalize_model_id(model).as_str(),
        "grokbuild01" | "grokbuild01latest"
    )
}

pub(crate) fn display_model_name(model: &str, reasoning_effort: Option<&str>) -> String {
    if model.trim().is_empty() {
        return UNKNOWN_MODEL.to_string();
    }
    if is_grok45_model_id(model) {
        if let Some(effort) = reasoning_effort.and_then(normalize_reasoning_effort) {
            return format!("Grok 4.5 ({effort})");
        }
        return "Grok 4.5".to_string();
    }
    if is_grok46_model_id(model) {
        if let Some(effort) = reasoning_effort.and_then(normalize_reasoning_effort) {
            return format!("Grok 4.6 ({effort})");
        }
        return "Grok 4.6".to_string();
    }
    if is_grok_build_01_model_id(model) {
        return "Grok Build 0.1".to_string();
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

    let total_nanos = (seconds * 1_000_000_000.0).round();
    if !total_nanos.is_finite() {
        return None;
    }
    let total_nanos = total_nanos as i128;
    let whole_seconds = total_nanos.div_euclid(1_000_000_000);
    if whole_seconds < i64::MIN as i128 || whole_seconds > i64::MAX as i128 {
        return None;
    }
    let nanos = total_nanos.rem_euclid(1_000_000_000) as u32;
    DateTime::<Utc>::from_timestamp(whole_seconds as i64, nanos)
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
) -> Vec<UsageEntry> {
    let has_provider_usage = turn.usage.is_some() || turn.reported_cost_usd.is_some();
    let total_stats = turn
        .usage
        .or_else(|| {
            (turn.context_tokens > 0).then_some(TokenStats {
                input: turn.context_tokens,
                output: 0,
                cache_read: None,
                cache_write: None,
                cache_write_5m: None,
                cache_write_1h: None,
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
                cache_write_5m: None,
                cache_write_1h: None,
                reasoning: None,
                total: 0,
            })
        });
    let Some(total_stats) = total_stats.or_else(|| {
        turn.reported_cost_usd.map(|_| TokenStats {
            input: 0,
            output: 0,
            cache_read: None,
            cache_write: None,
            cache_write_5m: None,
            cache_write_1h: None,
            reasoning: None,
            total: 0,
        })
    }) else {
        return Vec::new();
    };
    let timestamp = if turn.timestamp.is_empty() {
        "1970-01-01T00:00:00.000Z".to_string()
    } else {
        turn.timestamp
    };
    let reasoning_effort = turn
        .reasoning_effort
        .or_else(|| metadata.reasoning_effort.clone());
    let source_kind = if has_provider_usage {
        USAGE_SOURCE_KIND
    } else {
        CONTEXT_SOURCE_KIND
    };

    let model_usages = turn.model_usages;
    let all_model_costs_present = model_usages
        .iter()
        .all(|usage| usage.reported_cost_usd.is_some());
    let model_specs = match model_usages.len() {
        0 => vec![(
            turn.model.or_else(|| metadata.model.clone()),
            total_stats,
            turn.reported_cost_usd,
        )],
        1 => {
            let usage = model_usages.into_iter().next().expect("length checked");
            vec![(
                Some(usage.model),
                usage.stats,
                usage.reported_cost_usd.or(turn.reported_cost_usd),
            )]
        }
        _ if turn.reported_cost_usd.is_none() || all_model_costs_present => model_usages
            .into_iter()
            .map(|usage| (Some(usage.model), usage.stats, usage.reported_cost_usd))
            .collect(),
        _ => vec![(None, total_stats, turn.reported_cost_usd)],
    };

    model_specs
        .into_iter()
        .map(|(model_id, stats, reported_cost_usd)| {
            let model_id = model_id.filter(|model| !model.trim().is_empty());
            let model = model_id
                .as_deref()
                .map(|model| display_model_name(model, reasoning_effort.as_deref()))
                .unwrap_or_else(|| UNKNOWN_MODEL.to_string());
            UsageEntry {
                timestamp: timestamp.clone(),
                session_id: metadata.session_id.clone(),
                session_name: metadata.session_name.clone(),
                transcript_path: Some(updates_path.to_string_lossy().into_owned()),
                cwd: metadata.cwd.clone(),
                version: metadata.version.clone(),
                turn_no: turn.turn_no.max(1),
                model: Some(model),
                model_id,
                tokens: Some(stats.clone()),
                delta_tokens: Some(stats),
                context: Some(ContextStats {
                    current_context_tokens: (turn.context_tokens > 0)
                        .then_some(turn.context_tokens),
                    displayed_context_limit: None,
                    current_context_used_percentage: None,
                }),
                cost: reported_cost_usd.map(|reported_cost_usd| crate::db::CostStats {
                    total_api_duration_ms: None,
                    total_duration_ms: None,
                    total_premium_requests: None,
                    reported_cost_usd: Some(reported_cost_usd),
                }),
                source_kind: Some(source_kind.to_string()),
                source_dir_key: None,
                parent_session_id: None,
                agent_nickname: None,
                agent_role: None,
                reasoning_effort: reasoning_effort.clone(),
            }
        })
        .collect()
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
            if let Some(usage) = extract_usage(&event, update, params) {
                current_turn.usage = Some(usage.total);
                current_turn.model_usages = usage.models;
                match current_turn.model_usages.as_slice() {
                    [usage] => current_turn.model = Some(usage.model.clone()),
                    [] => {}
                    _ => current_turn.model = None,
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
                entries.extend(finalize_turn(&entry_metadata, turn, updates_path));
            }
        }
    }

    if let Some(turn) = current.take() {
        let mut entry_metadata = metadata;
        entry_metadata.session_name = session_name;
        entries.extend(finalize_turn(&entry_metadata, turn, updates_path));
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
        let session_dir = root.join("sessions").join("work").join("session-1");
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
                r#"{"timestamp":1710000002,"params":{"update":{"sessionUpdate":"turn_completed","usage":{"inputTokens":100,"cachedReadTokens":20,"outputTokens":30,"reasoningTokens":4,"totalTokens":130,"costUsdTicks":123000000,"costIsPartial":false,"usageIsIncomplete":false,"modelUsage":{"grok-4.5":{"inputTokens":100,"cachedReadTokens":20,"outputTokens":30,"reasoningTokens":4,"costUSD":0.0123}}}}}}"#, "\n"
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

        let usage = usage_from_container(&value).unwrap();
        assert_eq!(usage.models[0].model, "grok-4.5");
        assert_eq!(usage.total.input, 80);
        assert_eq!(usage.total.cache_read, Some(20));
        assert_eq!(usage.total.total, 130);
    }

    #[test]
    fn preserves_uncached_input_from_official_headless_usage() {
        let value = serde_json::json!({
            "usage": {
                "input_tokens": 80,
                "cache_read_input_tokens": 20,
                "output_tokens": 30,
                "total_tokens": 130
            },
            "modelUsage": {
                "grok-4.5": {
                    "inputTokens": 80,
                    "cacheReadInputTokens": 20,
                    "outputTokens": 30
                }
            }
        });

        let usage = usage_from_container(&value).unwrap();
        assert_eq!(usage.total.input, 80);
        assert_eq!(usage.total.cache_read, Some(20));
        assert_eq!(usage.total.total, 130);
        assert_eq!(usage.models.len(), 1);
        assert_eq!(usage.models[0].stats.input, 80);
        assert_eq!(usage.models[0].stats.cache_read, Some(20));
        assert_eq!(usage.models[0].stats.total, 130);
    }

    #[test]
    fn parses_cost_ticks_and_rejects_partial_or_incomplete_costs() {
        assert_eq!(
            parse_reported_cost(&serde_json::json!({"costUsdTicks": 10_000_000_000u64})),
            Some(1.0)
        );
        assert_eq!(
            parse_reported_cost(
                &serde_json::json!({"costUsdTicks": 10_000_000_000u64, "costIsPartial": true})
            ),
            None
        );
        assert_eq!(
            parse_reported_cost(
                &serde_json::json!({"costUsdTicks": 10_000_000_000u64, "usageIsIncomplete": true})
            ),
            None
        );
        assert_eq!(
            extract_reported_cost(
                &Value::Null,
                &serde_json::json!({
                    "usage": {
                        "costUsdTicks": 10_000_000_000u64,
                        "usageIsIncomplete": true,
                        "modelUsage": {
                            "grok-4.5": {"costUSD": 1.0}
                        }
                    }
                }),
                &Value::Null,
            ),
            None
        );
    }

    #[test]
    fn preserves_each_model_usage_in_a_multi_model_turn() {
        let value = serde_json::json!({
            "usage": {
                "inputTokens": 300,
                "outputTokens": 60,
                "totalTokens": 360,
                "modelUsage": {
                    "grok-4.5": {
                        "inputTokens": 100,
                        "outputTokens": 20,
                        "totalTokens": 120,
                        "costUSD": 0.01
                    },
                    "grok-build-0.1": {
                        "inputTokens": 200,
                        "outputTokens": 40,
                        "totalTokens": 240,
                        "costUSD": 0.02
                    }
                }
            }
        });

        let usage = usage_from_container(&value).unwrap();
        let mut models: Vec<&str> = usage
            .models
            .iter()
            .map(|model| model.model.as_str())
            .collect();
        models.sort_unstable();

        assert_eq!(models, ["grok-4.5", "grok-build-0.1"]);
        assert_eq!(usage.total.total, 360);
        assert_eq!(
            usage
                .models
                .iter()
                .find(|model| model.model == "grok-4.5")
                .and_then(|model| model.reported_cost_usd),
            Some(0.01)
        );
        assert_eq!(
            usage
                .models
                .iter()
                .find(|model| model.model == "grok-build-0.1")
                .and_then(|model| model.reported_cost_usd),
            Some(0.02)
        );
    }

    #[test]
    fn parses_multi_model_turn_into_separate_entries() {
        let root = test_updates_path("multi-model");
        let session_dir = root.join("sessions").join("work").join("session-multi");
        fs::create_dir_all(&session_dir).unwrap();
        let events = [
            serde_json::json!({
                "timestamp": 1710000000,
                "params": {"update": {"sessionUpdate": "turn_started", "turn_number": 0}}
            }),
            serde_json::json!({
                "timestamp": 1710000001,
                "params": {"update": {"sessionUpdate": "user_message_chunk", "content": {"text": "hello"}}}
            }),
            serde_json::json!({
                "timestamp": 1710000002,
                "params": {"update": {
                    "sessionUpdate": "turn_completed",
                    "usage": {
                        "inputTokens": 300,
                        "outputTokens": 60,
                        "totalTokens": 360,
                        "costUsdTicks": 300_000_000,
                        "modelUsage": {
                            "grok-4.5": {"inputTokens": 100, "outputTokens": 20, "totalTokens": 120, "costUSD": 0.01},
                            "grok-build-0.1": {"inputTokens": 200, "outputTokens": 40, "totalTokens": 240, "costUSD": 0.02}
                        }
                    }
                }}
            }),
        ];
        let content = events
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(session_dir.join("updates.jsonl"), format!("{content}\n")).unwrap();

        let entries = parse_session_usage_file(&session_dir.join("updates.jsonl")).unwrap();
        let mut models: Vec<(&str, f64)> = entries
            .iter()
            .map(|entry| {
                (
                    entry.model_id.as_deref().unwrap(),
                    entry
                        .cost
                        .as_ref()
                        .and_then(|cost| cost.reported_cost_usd)
                        .unwrap(),
                )
            })
            .collect();
        models.sort_by(|left, right| left.0.cmp(right.0));

        assert_eq!(models.len(), 2);
        assert_eq!(models[0].0, "grok-4.5");
        assert!((models[0].1 - 0.01).abs() < 1e-12);
        assert_eq!(models[1].0, "grok-build-0.1");
        assert!((models[1].1 - 0.02).abs() < 1e-12);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn preserves_input_when_cached_read_exceeds_input() {
        let stats = normalize_provider_token_stats(TokenStats {
            input: 10,
            output: 3,
            cache_read: Some(20),
            cache_write: None,
            cache_write_5m: None,
            cache_write_1h: None,
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
        assert_eq!(
            display_model_name("grok-4.6", Some("high")),
            "Grok 4.6 (High)"
        );
        assert_eq!(display_model_name("grok-4.6-latest", None), "Grok 4.6");
        assert_eq!(
            display_model_name("grok-build-0.1", Some("high")),
            "Grok Build 0.1"
        );
        assert!(!is_grok45_model_id("grok-build-0.1"));
        assert!(is_grok46_model_id("grok-4.6"));
        assert!(is_grok_build_01_model_id("grok-build-0.1"));
    }

    #[test]
    fn timestamp_normalizes_negative_epochs_and_nanosecond_carry() {
        assert_eq!(
            timestamp_from_seconds(-0.5).as_deref(),
            Some("1969-12-31T23:59:59.500Z")
        );
        assert_eq!(
            timestamp_from_seconds(0.999_999_999_6).as_deref(),
            Some("1970-01-01T00:00:01.000Z")
        );
    }

    #[test]
    fn context_snapshot_deltas_are_incremental_across_turns() {
        let root = test_updates_path("context");
        let session_dir = root.join("sessions").join("work").join("session-2");
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
        assert!(entries
            .iter()
            .all(|entry| entry.model.as_deref() == Some(UNKNOWN_MODEL)));
        assert!(entries.iter().all(|entry| entry.model_id.is_none()));

        let _ = fs::remove_dir_all(root);
    }
}
