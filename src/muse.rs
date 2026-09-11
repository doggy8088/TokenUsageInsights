use crate::db::{CostStats, TokenStats, UsageEntry};
use serde_json::Value;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub(crate) const SOURCE_KIND: &str = "muse-code";

fn value_as_u64(value: Option<&Value>) -> Option<u64> {
    value.and_then(|value| {
        value
            .as_u64()
            .or_else(|| value.as_i64().and_then(|number| u64::try_from(number).ok()))
    })
}

fn extract_text_from_content(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_string();
    }
    if let Some(items) = content.as_array() {
        let mut parts = Vec::new();
        for item in items {
            if let Some(kind) = item.get("kind").and_then(Value::as_str) {
                if kind == "text" {
                    if let Some(text) = item.get("text").and_then(Value::as_str) {
                        parts.push(text.to_string());
                    }
                }
            } else if let Some(text) = item.get("text").and_then(Value::as_str) {
                parts.push(text.to_string());
            }
        }
        return parts.join(" ");
    }
    String::new()
}

fn extract_user_prompt(payload: &Value) -> Option<String> {
    // runtime.user_intent.accepted stores prompt in model_messages or refill_blocks
    if let Some(messages) = payload.get("model_messages").and_then(Value::as_array) {
        for msg in messages {
            if let Some(content) = msg.get("content") {
                let text = extract_text_from_content(content);
                if !text.trim().is_empty() {
                    return Some(text);
                }
            }
        }
    }
    if let Some(blocks) = payload.get("refill_blocks").and_then(Value::as_array) {
        for block in blocks {
            if let Some(text) = block.get("text").and_then(Value::as_str) {
                if !text.trim().is_empty() {
                    return Some(text.to_string());
                }
            }
        }
    }
    None
}

fn recorded_at_to_rfc3339(recorded_at: u64) -> String {
    // Muse recorded_at is microseconds since epoch (e.g. 1788296525203692 -> 2026-09-02)
    // which is 16 digits. Try microseconds first (correct), fallback to nanoseconds.
    let secs_micro = recorded_at / 1_000_000;
    let nanos_micro = ((recorded_at % 1_000_000) * 1000) as u32;
    if let Some(dt) =
        chrono::DateTime::<chrono::Utc>::from_timestamp(secs_micro as i64, nanos_micro)
    {
        // Sanity: must be after 2020 and before 2100
        if secs_micro > 1_577_836_800 && secs_micro < 4_102_444_800 {
            return dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        }
    }
    let secs = recorded_at / 1_000_000_000;
    let nanos = (recorded_at % 1_000_000_000) as u32;
    if let Some(dt) = chrono::DateTime::<chrono::Utc>::from_timestamp(secs as i64, nanos) {
        return dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    }
    if let Some(dt) =
        chrono::DateTime::<chrono::Utc>::from_timestamp(secs_micro as i64, nanos_micro)
    {
        return dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    }
    String::new()
}

fn parse_usage_from_model_completed(usage: &Value) -> Option<TokenStats> {
    let input = value_as_u64(Some(usage.get("input_tokens")?)).unwrap_or(0);
    // muse reports input_tokens inclusive of cached_tokens, need to separate
    let cached = value_as_u64(usage.get("cached_tokens"))
        .or_else(|| value_as_u64(usage.get("cache_read_tokens")))
        .unwrap_or(0);
    let output = value_as_u64(usage.get("output_tokens")).unwrap_or(0);
    let reasoning = value_as_u64(usage.get("reasoning_tokens")).unwrap_or(0);
    let cache_write = value_as_u64(usage.get("cache_write_tokens")).unwrap_or(0);

    if input == 0 && output == 0 && cached == 0 && reasoning == 0 {
        return None;
    }

    let non_cached_input = input.saturating_sub(cached);
    let total = input.saturating_add(output);

    Some(TokenStats {
        input: non_cached_input,
        output,
        cache_read: if cached > 0 { Some(cached) } else { None },
        cache_write: if cache_write > 0 {
            Some(cache_write)
        } else {
            None
        },
        cache_write_5m: None,
        cache_write_1h: None,
        reasoning: if reasoning > 0 { Some(reasoning) } else { None },
        total,
    })
}

fn find_session_files_recursive(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        if ft.is_dir() {
            find_session_files_recursive(&path, files);
        } else if ft.is_file() && path.file_name().and_then(|n| n.to_str()) == Some("session.jsonl")
        {
            files.push(path);
        }
    }
}

pub(crate) fn find_session_files(dir: &Path) -> Vec<PathBuf> {
    let sessions_root = dir.join("sessions");
    let mut files = Vec::new();
    if sessions_root.is_dir() {
        find_session_files_recursive(&sessions_root, &mut files);
    } else if dir.is_dir() {
        // fallback: dir itself might be sessions root or contain date sharding without sessions prefix
        find_session_files_recursive(dir, &mut files);
    }
    files.sort();
    files
}

fn extract_workspace_info(path: &Path) -> (Option<String>, Option<String>, Option<String>) {
    let Ok(file) = File::open(path) else {
        return (None, None, None);
    };
    let reader = BufReader::new(file);
    let mut cwd: Option<String> = None;
    let mut workspace_root: Option<String> = None;
    let mut model: Option<String> = None;

    for line in reader.lines().take(100) {
        let Ok(line) = line else {
            continue;
        };
        let Ok(obj) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let pt = obj
            .get("payload_type")
            .and_then(Value::as_str)
            .unwrap_or("");
        let payload = obj.get("payload");

        if pt == "runtime.session.metadata" {
            if let Some(record) = payload.and_then(|p| p.get("record")) {
                if cwd.is_none() {
                    if let Some(v) = record.get("workspace_root").and_then(Value::as_str) {
                        workspace_root = Some(v.to_string());
                    }
                }
                if model.is_none() {
                    if let Some(v) = record.get("model_id").and_then(Value::as_str) {
                        model = Some(v.to_string());
                    }
                }
            }
        } else if pt == "runtime.session.route_facts" {
            if let Some(record) = payload.and_then(|p| p.get("record")) {
                if let Some(v) = record.get("cwd").and_then(Value::as_str) {
                    cwd = Some(v.to_string());
                }
            }
        }
        if cwd.is_some() && workspace_root.is_some() && model.is_some() {
            break;
        }
    }

    (
        cwd.or_else(|| workspace_root.clone()),
        workspace_root,
        model,
    )
}

pub(crate) fn parse_session_usage_file(path: &Path) -> Result<Vec<UsageEntry>, String> {
    let file =
        File::open(path).map_err(|e| format!("無法開啟 muse session 檔案 {:?}: {e}", path))?;
    let reader = BufReader::new(file);

    let session_id = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    let transcript_path = path.to_string_lossy().into_owned();
    let (cwd_opt, _workspace_root, default_model) = extract_workspace_info(path);

    let mut session_name: Option<String> = None;
    let mut entries: Vec<(String, TokenStats, Option<u64>, String)> = Vec::new(); // timestamp, stats, duration, model

    for line in reader.lines() {
        let Ok(line) = line else {
            continue;
        };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(obj) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let pt = obj
            .get("payload_type")
            .and_then(Value::as_str)
            .unwrap_or("");
        let payload = obj.get("payload");

        if pt == "runtime.user_intent.accepted" {
            if session_name.is_none() {
                if let Some(p) = payload {
                    if let Some(prompt) = extract_user_prompt(p) {
                        let normalized = prompt.trim().replace('\r', "").replace('\n', " ");
                        if !normalized.is_empty() {
                            session_name = Some(normalized.chars().take(100).collect());
                        }
                    }
                }
            }
            continue;
        }

        if pt == "runtime.session" {
            if let Some(p) = payload {
                let kind = p.get("kind").and_then(Value::as_str).unwrap_or("");
                if kind != "run" {
                    continue;
                }
                let event = p.get("event");
                let ek = event
                    .and_then(|e| e.get("kind"))
                    .and_then(Value::as_str)
                    .unwrap_or("");

                if ek == "model_completed" {
                    let usage = event.and_then(|e| e.get("usage"));
                    let Some(usage_val) = usage else {
                        continue;
                    };
                    let Some(stats) = parse_usage_from_model_completed(usage_val) else {
                        continue;
                    };

                    let model = event
                        .and_then(|e| e.get("model"))
                        .and_then(Value::as_str)
                        .map(|s| s.to_string())
                        .or_else(|| default_model.clone())
                        .unwrap_or_else(|| "muse-spark-1.2".to_string());

                    let duration_ms = event
                        .and_then(|e| e.get("duration_ms"))
                        .and_then(Value::as_u64);

                    let recorded_at = obj.get("recorded_at").and_then(Value::as_u64).unwrap_or(0);
                    let timestamp = if recorded_at > 0 {
                        recorded_at_to_rfc3339(recorded_at)
                    } else {
                        String::new()
                    };

                    entries.push((timestamp, stats, duration_ms, model));
                }
            }
        }
    }

    if entries.is_empty() {
        return Ok(Vec::new());
    }

    // Ensure timestamps are ordered; fallback to file order if missing
    let mut results = Vec::new();
    for (idx, (timestamp, stats, duration_ms, model)) in entries.into_iter().enumerate() {
        let turn_no = (idx + 1) as u32;
        let ts = if timestamp.is_empty() {
            // fallback to current time per turn
            chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        } else {
            timestamp
        };
        let date = ts.get(0..10).unwrap_or("1970-01-01").to_string();
        // Provide both tokens and delta_tokens as same snapshot per call, with delta = stats
        results.push(UsageEntry {
            timestamp: ts.clone(),
            session_id: session_id.clone(),
            session_name: session_name.clone().or_else(|| Some(session_id.clone())),
            transcript_path: Some(transcript_path.clone()),
            cwd: cwd_opt.clone(),
            version: None,
            turn_no,
            model: Some(model.clone()),
            model_id: Some(model.clone()),
            tokens: Some(stats.clone()),
            delta_tokens: Some(stats.clone()),
            context: None,
            cost: duration_ms.map(|duration_ms| CostStats {
                total_api_duration_ms: Some(duration_ms as f64),
                total_duration_ms: Some(duration_ms as f64),
                total_premium_requests: Some(1.0),
                reported_cost_usd: None,
            }),
            source_kind: Some(SOURCE_KIND.to_string()),
            source_dir_key: None,
            parent_session_id: None,
            agent_nickname: None,
            agent_role: None,
            reasoning_effort: None,
        });
        let _ = date;
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "token-usage-insights-muse-test-{}-{}",
            label,
            std::process::id()
        ))
    }

    #[test]
    fn parse_session_usage_file_reads_model_completed() {
        let root = temp_dir("basic");
        let session_dir = root
            .join("sessions")
            .join("2026")
            .join("09")
            .join("02")
            .join("test-session-123");
        fs::create_dir_all(&session_dir).unwrap();
        let path = session_dir.join("session.jsonl");

        let mut file = File::create(&path).unwrap();
        writeln!(
            file,
            r#"{{"schema_version":1,"id":"a","stream":{{"kind":"session","id":"test-session-123"}},"sequence":1,"recorded_at":1788296525203692000,"payload_type":"runtime.user_intent.accepted","payload":{{"model_messages":[{{"content":[{{"kind":"text","text":"Hello, do something"}}]}}]}}}}"#
        ).unwrap();
        writeln!(
            file,
            r#"{{"schema_version":1,"id":"b","stream":{{"kind":"session","id":"test-session-123"}},"sequence":2,"recorded_at":1788296529190050000,"payload_type":"runtime.session","payload":{{"kind":"run","event":{{"kind":"model_completed","usage":{{"input_tokens":35237,"output_tokens":379,"cached_tokens":32305,"reasoning_tokens":258}},"duration_ms":3807,"model":"muse-spark-1.2-contributor"}}}}}}"#
        ).unwrap();

        let entries = parse_session_usage_file(&path).unwrap();
        fs::remove_dir_all(&root).ok();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_id, "test-session-123");
        assert_eq!(entries[0].turn_no, 1);
        assert_eq!(
            entries[0].model.as_deref(),
            Some("muse-spark-1.2-contributor")
        );
        assert_eq!(
            entries[0].session_name.as_deref(),
            Some("Hello, do something")
        );
        let tokens = entries[0].tokens.as_ref().unwrap();
        assert_eq!(tokens.input, 35237 - 32305);
        assert_eq!(tokens.cache_read, Some(32305));
        assert_eq!(tokens.output, 379);
        assert_eq!(tokens.reasoning, Some(258));
    }

    #[test]
    fn parse_session_usage_file_calculates_cost_for_muse_spark_1_3_contributor() {
        let root = temp_dir("spark13");
        let session_dir = root
            .join("sessions")
            .join("2026")
            .join("09")
            .join("11")
            .join("bda9278c-cea8-4033-bd9b-3f6eae98c6b2");
        fs::create_dir_all(&session_dir).unwrap();
        let path = session_dir.join("session.jsonl");

        let mut file = File::create(&path).unwrap();
        writeln!(
            file,
            r#"{{"schema_version":1,"id":"a","stream":{{"kind":"session","id":"bda9278c-cea8-4033-bd9b-3f6eae98c6b2"}},"sequence":1,"recorded_at":1789072091105545,"payload_type":"runtime.session.metadata","payload":{{"kind":"metadata","record":{{"provider_id":"meta","model_id":"muse-spark-1.3-contributor"}}}}}}"#
        ).unwrap();
        writeln!(
            file,
            r#"{{"schema_version":1,"id":"b","stream":{{"kind":"session","id":"bda9278c-cea8-4033-bd9b-3f6eae98c6b2"}},"sequence":2,"recorded_at":1789072091127816,"payload_type":"runtime.session","payload":{{"kind":"run","event":{{"kind":"model_completed","usage":{{"input_tokens":10000,"output_tokens":500,"cached_tokens":8000,"reasoning_tokens":100}},"duration_ms":1200,"model":"muse-spark-1.3-contributor"}}}}}}"#
        ).unwrap();

        let entries = parse_session_usage_file(&path).unwrap();
        fs::remove_dir_all(&root).ok();

        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].model.as_deref(),
            Some("muse-spark-1.3-contributor")
        );

        let rules = crate::pricing::load_prepared_pricing_rules();
        let tokens = entries[0].tokens.as_ref().unwrap();
        let cost = rules
            .calculate_usage_cost(
                entries[0].model.as_deref(),
                tokens.input,
                tokens.output,
                tokens.cache_read.unwrap_or(0),
                0,
                0,
            )
            .expect("cost calculation should succeed for muse-spark-1.3-contributor");
        assert!((cost - 0.000316).abs() < 1e-12);
    }

    #[test]
    fn find_session_files_discovers_nested_session_jsonl() {
        let root = temp_dir("find");
        let nested = root
            .join("sessions")
            .join("2026")
            .join("09")
            .join("02")
            .join("sess-1");
        fs::create_dir_all(&nested).unwrap();
        let fpath = nested.join("session.jsonl");
        fs::write(&fpath, "").unwrap();
        fs::write(nested.join("other.txt"), "ignore").unwrap();

        let files = find_session_files(&root);
        fs::remove_dir_all(&root).ok();
        assert_eq!(files, vec![fpath]);
    }

    #[test]
    fn parse_usage_from_model_completed_handles_zero_cached() {
        let usage: Value = serde_json::from_str(
            r#"{"input_tokens":32336,"output_tokens":192,"cached_tokens":0,"reasoning_tokens":91}"#,
        )
        .unwrap();
        let stats = parse_usage_from_model_completed(&usage).unwrap();
        assert_eq!(stats.input, 32336);
        assert_eq!(stats.cache_read, None);
        assert_eq!(stats.output, 192);
        assert_eq!(stats.reasoning, Some(91));
    }
}
