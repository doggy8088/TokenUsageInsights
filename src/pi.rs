//! Shared parser for the Pi Coding Agent (<https://pi.dev/>) local session
//! files, and for its open-source fork OMP (<https://omp.sh/>), which persists
//! sessions using the exact same tree-structured JSONL format under
//! `<dir>/agent/sessions/`.
//!
//! File layout: `<dir>/agent/sessions/--<cwd>--/<timestamp>_<uuid>.jsonl`.
//! Format reference:
//! <https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/session-format.md>
use crate::db::{CostStats, TokenStats, UsageEntry};
use serde_json::Value;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub(crate) const SOURCE_KIND: &str = "pi-session";

/// Recursively collects every `*.jsonl` session file under
/// `<dir>/agent/sessions/`.
pub(crate) fn find_session_files(dir: &Path) -> Vec<PathBuf> {
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
                && path.extension().and_then(|ext| ext.to_str()) == Some("jsonl")
            {
                files.push(path);
            }
        }
    }

    let mut files = Vec::new();
    let sessions_dir = dir.join("agent").join("sessions");
    if sessions_dir.is_dir() {
        visit(&sessions_dir, &mut files);
    }
    files.sort();
    files
}

fn value_as_u64(value: Option<&Value>) -> Option<u64> {
    value.and_then(|value| {
        value
            .as_u64()
            .or_else(|| value.as_i64().and_then(|number| u64::try_from(number).ok()))
    })
}

fn parse_token_stats(usage: &Value) -> Option<TokenStats> {
    let input = value_as_u64(usage.get("input")).unwrap_or(0);
    let output = value_as_u64(usage.get("output")).unwrap_or(0);
    let cache_read = value_as_u64(usage.get("cacheRead"));
    let cache_write = value_as_u64(usage.get("cacheWrite"));
    let reasoning = value_as_u64(usage.get("reasoning"));
    let total = value_as_u64(usage.get("totalTokens")).unwrap_or_else(|| {
        input
            .saturating_add(output)
            .saturating_add(cache_read.unwrap_or(0))
            .saturating_add(cache_write.unwrap_or(0))
    });

    if input == 0 && output == 0 && cache_read.unwrap_or(0) == 0 && total == 0 {
        return None;
    }

    Some(TokenStats {
        input,
        output,
        cache_read: cache_read.filter(|value| *value > 0),
        cache_write: cache_write.filter(|value| *value > 0),
        cache_write_5m: None,
        cache_write_1h: None,
        reasoning: reasoning.filter(|value| *value > 0),
        total,
    })
}

pub(crate) fn parse_reported_cost(usage: &Value) -> Option<f64> {
    usage
        .get("cost")
        .and_then(|cost| cost.get("total"))
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value >= 0.0)
}

struct SessionHeaderInfo {
    session_id: String,
    cwd: Option<String>,
    parent_session_id: Option<String>,
}

fn read_session_header(path: &Path) -> SessionHeaderInfo {
    let fallback_id = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("unknown")
        .to_string();

    let Ok(file) = File::open(path) else {
        return SessionHeaderInfo {
            session_id: fallback_id,
            cwd: None,
            parent_session_id: None,
        };
    };
    let reader = BufReader::new(file);
    // The `session` header entry is usually the first line, but OMP (and
    // potentially future Pi versions) may prepend metadata lines such as
    // `{"type":"title",...}` before it, so scan a handful of leading lines
    // rather than assuming line 1 is always the header.
    for line in reader.lines().take(10) {
        let Ok(line) = line else {
            continue;
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(header) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if header.get("type").and_then(Value::as_str) == Some("session") {
            let session_id = header
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or(fallback_id);
            let cwd = header
                .get("cwd")
                .and_then(Value::as_str)
                .map(str::to_string);
            let parent_session_id = header
                .get("parentSession")
                .or_else(|| header.get("parentSessionId"))
                .or_else(|| header.get("parentId"))
                .and_then(Value::as_str)
                .and_then(|val| {
                    let val = val.trim();
                    if val.is_empty() {
                        return None;
                    }
                    if val.ends_with(".jsonl") {
                        let p = Path::new(val);
                        let stem = p.file_stem()?.to_str()?;
                        if let Some((_, uuid)) = stem.rsplit_once('_') {
                            Some(uuid.to_string())
                        } else {
                            Some(stem.to_string())
                        }
                    } else {
                        Some(val.to_string())
                    }
                });
            return SessionHeaderInfo {
                session_id,
                cwd,
                parent_session_id,
            };
        }
    }

    SessionHeaderInfo {
        session_id: fallback_id,
        cwd: None,
        parent_session_id: None,
    }
}

/// Parses one Pi/OMP session JSONL file into per-turn [`UsageEntry`] rows.
/// Each assistant message that carries a `usage` object is treated as a
/// complete, self-contained turn (Pi/OMP already report token counts and
/// cost per assistant turn, unlike providers that require delta accumulation
/// across streaming chunks).
pub(crate) fn parse_session_usage_file(
    path: &Path,
    source_kind: &str,
) -> Result<Vec<UsageEntry>, String> {
    let file =
        File::open(path).map_err(|error| format!("無法開啟 session 檔案 {:?}: {error}", path))?;
    let reader = BufReader::new(file);
    let header = read_session_header(path);
    let transcript_path = path.to_string_lossy().into_owned();
    let mut entries = Vec::new();
    let mut turn_no = 1u32;
    let mut session_name: Option<String> = None;
    let mut agent_role: Option<String> = None;

    for line in reader.lines() {
        let Ok(line) = line else {
            continue;
        };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(entry_value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let entry_type = entry_value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("");

        if entry_type == "session_info" {
            if let Some(name) = entry_value.get("name").and_then(Value::as_str) {
                if session_name.is_none() {
                    session_name = Some(name.to_string());
                }
                if name.starts_with("subagent-") {
                    let parts: Vec<&str> = name.split('-').collect();
                    if parts.len() >= 2 && !parts[1].is_empty() {
                        agent_role = Some(parts[1].to_string());
                    }
                }
            }
            continue;
        }

        if entry_type == "message" {
            if let Some(message) = entry_value.get("message") {
                let role = message.get("role").and_then(Value::as_str).unwrap_or("");
                if role == "system" && agent_role.is_none() {
                    if let Some(sections) = message.get("sections") {
                        if let Some(preamble) = sections.get("preamble").and_then(Value::as_str) {
                            if let Some(start) = preamble.find("<active_agent name=\"") {
                                let rest = &preamble[start + 20..];
                                if let Some(end) = rest.find('"') {
                                    agent_role = Some(rest[..end].to_string());
                                }
                            }
                        }
                    }
                }
                if role == "user"
                    && (session_name.is_none()
                        || session_name
                            .as_deref()
                            .unwrap_or("")
                            .starts_with("subagent-"))
                {
                    if let Some(content) = message.get("content").and_then(Value::as_array) {
                        for item in content {
                            if let Some(text) = item.get("text").and_then(Value::as_str) {
                                let trimmed = text.trim();
                                if !trimmed.is_empty()
                                    && !trimmed.starts_with("<skill")
                                    && !trimmed.starts_with("<loaded_context>")
                                {
                                    let first_line = trimmed.lines().next().unwrap_or("").trim();
                                    let clean_name =
                                        if let Some(stripped) = first_line.strip_prefix("Task:") {
                                            stripped.trim()
                                        } else {
                                            first_line
                                        };
                                    if !clean_name.is_empty() {
                                        session_name = Some(clean_name.chars().take(100).collect());
                                        break;
                                    }
                                }
                            }
                        }
                    } else if let Some(text) = message.get("content").and_then(Value::as_str) {
                        let trimmed = text.trim();
                        if !trimmed.is_empty()
                            && !trimmed.starts_with("<skill")
                            && !trimmed.starts_with("<loaded_context>")
                        {
                            let first_line = trimmed.lines().next().unwrap_or("").trim();
                            let clean_name =
                                if let Some(stripped) = first_line.strip_prefix("Task:") {
                                    stripped.trim()
                                } else {
                                    first_line
                                };
                            if !clean_name.is_empty() {
                                session_name = Some(clean_name.chars().take(100).collect());
                            }
                        }
                    }
                }
            }
        }

        if entry_type != "message" {
            continue;
        }
        let Some(message) = entry_value.get("message") else {
            continue;
        };
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(usage) = message.get("usage") else {
            continue;
        };
        let Some(tokens) = parse_token_stats(usage) else {
            continue;
        };
        let reported_cost_usd = parse_reported_cost(usage);
        let model = message
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string);
        let timestamp = entry_value
            .get("timestamp")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        entries.push(UsageEntry {
            timestamp,
            session_id: header.session_id.clone(),
            session_name: session_name.clone(),
            transcript_path: Some(transcript_path.clone()),
            cwd: header.cwd.clone(),
            version: None,
            turn_no,
            model: model.clone(),
            model_id: model,
            tokens: Some(tokens.clone()),
            delta_tokens: Some(tokens),
            context: None,
            cost: reported_cost_usd.map(|reported_cost_usd| CostStats {
                total_api_duration_ms: None,
                total_duration_ms: None,
                total_premium_requests: None,
                reported_cost_usd: Some(reported_cost_usd),
            }),
            source_kind: Some(source_kind.to_string()),
            source_dir_key: None,
            parent_session_id: header.parent_session_id.clone(),
            agent_nickname: header
                .parent_session_id
                .as_ref()
                .map(|_| "Subagent".to_string()),
            agent_role: agent_role.clone(),
            reasoning_effort: None,
        });
        turn_no += 1;
    }

    for entry in &mut entries {
        if entry.session_name.is_none() {
            entry.session_name = session_name.clone();
        }
        if entry.agent_role.is_none() {
            entry.agent_role = agent_role.clone();
        }
    }

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_jsonl_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "token-usage-insights-pi-test-{}-{}",
            label,
            std::process::id()
        ))
    }

    #[test]
    fn parse_session_usage_file_reads_cwd_when_title_precedes_session_header() {
        // OMP prepends a `{"type":"title",...}` line before the session
        // header, unlike Pi which starts directly with `{"type":"session"}`.
        let root = temp_jsonl_path("omp-title-prefix");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("session.jsonl");
        let mut file = File::create(&path).unwrap();
        writeln!(
            file,
            r#"{{"type":"title","v":1,"title":"","updatedAt":"2024-12-03T14:00:00.000Z"}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"type":"session","version":3,"id":"omp-sess-1","timestamp":"2024-12-03T14:00:00.000Z","cwd":"/tmp/omp-project"}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"type":"model_change","id":"m0","parentId":null,"timestamp":"2024-12-03T14:00:00.500Z","model":"ollama/glm-5.3-flash:cloud"}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"type":"message","id":"m1","parentId":null,"timestamp":"2024-12-03T14:00:01.000Z","message":{{"role":"assistant","content":[{{"type":"text","text":"Hi!"}}],"model":"ollama/glm-5.3-flash:cloud","usage":{{"input":10,"output":5,"totalTokens":15,"cost":{{"total":0}}}}}}}}"#
        )
        .unwrap();

        let entries = parse_session_usage_file(&path, SOURCE_KIND).unwrap();

        fs::remove_dir_all(&root).ok();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_id, "omp-sess-1");
        assert_eq!(entries[0].cwd.as_deref(), Some("/tmp/omp-project"));
    }

    #[test]
    fn find_session_files_recurses_under_agent_sessions() {
        let root = temp_jsonl_path("find-session-files");
        let session_dir = root.join("agent").join("sessions").join("--tmp--project");
        fs::create_dir_all(&session_dir).unwrap();
        let session_file = session_dir.join("2024-12-03T14-00-00_abc.jsonl");
        fs::write(&session_file, "").unwrap();
        // Non-jsonl files must be ignored.
        fs::write(session_dir.join("notes.txt"), "ignore me").unwrap();

        let files = find_session_files(&root);

        fs::remove_dir_all(&root).ok();
        assert_eq!(files, vec![session_file]);
    }

    #[test]
    fn parse_session_usage_file_extracts_turns_with_cost() {
        let root = temp_jsonl_path("parse-usage");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("session.jsonl");
        let mut file = File::create(&path).unwrap();
        writeln!(
            file,
            r#"{{"type":"session","version":3,"id":"sess-1","timestamp":"2024-12-03T14:00:00.000Z","cwd":"/tmp/project"}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"type":"message","id":"m1","parentId":null,"timestamp":"2024-12-03T14:00:01.000Z","message":{{"role":"user","content":"Hello"}}}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"type":"message","id":"m2","parentId":"m1","timestamp":"2024-12-03T14:00:02.000Z","message":{{"role":"assistant","content":[{{"type":"text","text":"Hi!"}}],"provider":"anthropic","model":"claude-sonnet-4-5","usage":{{"input":100,"output":50,"cacheRead":10,"cacheWrite":0,"reasoning":15,"totalTokens":150,"cost":{{"input":0.001,"output":0.002,"cacheRead":0.0001,"cacheWrite":0,"total":0.0031}}}},"stopReason":"stop"}}}}"#
        )
        .unwrap();

        let entries = parse_session_usage_file(&path, SOURCE_KIND).unwrap();

        fs::remove_dir_all(&root).ok();

        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.session_id, "sess-1");
        assert_eq!(entry.cwd.as_deref(), Some("/tmp/project"));
        assert_eq!(entry.turn_no, 1);
        assert_eq!(entry.model.as_deref(), Some("claude-sonnet-4-5"));
        assert_eq!(entry.source_kind.as_deref(), Some(SOURCE_KIND));
        let tokens = entry.tokens.as_ref().unwrap();
        assert_eq!(tokens.input, 100);
        assert_eq!(tokens.output, 50);
        assert_eq!(tokens.cache_read, Some(10));
        assert_eq!(tokens.reasoning, Some(15));
        assert_eq!(tokens.total, 150);
        assert_eq!(
            entry.cost.as_ref().and_then(|cost| cost.reported_cost_usd),
            Some(0.0031)
        );
    }

    #[test]
    fn parse_session_usage_file_skips_non_usage_messages() {
        let root = temp_jsonl_path("parse-skip");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("session.jsonl");
        let mut file = File::create(&path).unwrap();
        writeln!(
            file,
            r#"{{"type":"session","version":3,"id":"sess-2","timestamp":"2024-12-03T14:00:00.000Z","cwd":"/tmp/project"}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"type":"message","id":"m1","parentId":null,"timestamp":"2024-12-03T14:00:01.000Z","message":{{"role":"user","content":"Hello"}}}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"type":"message","id":"m2","parentId":"m1","timestamp":"2024-12-03T14:00:02.000Z","message":{{"role":"toolResult","toolCallId":"call_1","toolName":"bash","content":[{{"type":"text","text":"ok"}}],"isError":false}}}}"#
        )
        .unwrap();

        let entries = parse_session_usage_file(&path, SOURCE_KIND).unwrap();

        fs::remove_dir_all(&root).ok();
        assert!(entries.is_empty());
    }

    #[test]
    fn parse_session_usage_file_extracts_role_and_task_from_subagent_messages() {
        let root = temp_jsonl_path("parse-subagent-role");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("session.jsonl");
        let mut file = File::create(&path).unwrap();
        writeln!(
            file,
            r#"{{"type":"session","version":3,"id":"child-pi-2","timestamp":"2026-10-14T07:04:20.848Z","cwd":"/tmp/worktree","parentSession":"/Users/test/.pi/agent/sessions/--test--/2026-10-14T05-51-16-433Z_parent-uuid.jsonl"}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"type":"session_info","id":"info1","name":"subagent-worker-12345"}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"type":"message","id":"m0","message":{{"role":"user","content":[{{"type":"text","text":"<skill name=\"test\">ignore</skill>"}},{{"type":"text","text":"Task: Implement rule engine"}}]}}}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"type":"message","id":"m1","message":{{"role":"assistant","content":[{{"type":"text","text":"Done"}}],"provider":"anthropic","model":"claude-3-7-sonnet","usage":{{"input":200,"output":100,"totalTokens":300}}}}}}"#
        )
        .unwrap();

        let entries = parse_session_usage_file(&path, SOURCE_KIND).unwrap();
        fs::remove_dir_all(&root).ok();

        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.session_id, "child-pi-2");
        assert_eq!(entry.parent_session_id.as_deref(), Some("parent-uuid"));
        assert_eq!(entry.agent_nickname.as_deref(), Some("Subagent"));
        assert_eq!(entry.agent_role.as_deref(), Some("worker"));
        assert_eq!(entry.session_name.as_deref(), Some("Implement rule engine"));
    }

    #[test]
    fn parse_session_usage_file_extracts_parent_session_from_path_or_id() {
        let root = temp_jsonl_path("parse-subagent");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("session.jsonl");
        let mut file = File::create(&path).unwrap();
        writeln!(
            file,
            r#"{{"type":"session","version":3,"id":"child-pi-1","timestamp":"2026-10-14T05:49:40.319Z","cwd":"/tmp/project","parentSession":"/Users/test/.pi/agent/sessions/--test--/2026-10-14T02-44-37-856Z_parent-pi-uuid.jsonl"}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"type":"message","id":"m1","parentId":null,"timestamp":"2026-10-14T05:49:41.000Z","message":{{"role":"assistant","content":[{{"type":"text","text":"Subagent response"}}],"provider":"anthropic","model":"claude-3-7-sonnet","usage":{{"input":200,"output":100,"totalTokens":300}}}}}}"#
        )
        .unwrap();

        let entries = parse_session_usage_file(&path, SOURCE_KIND).unwrap();

        fs::remove_dir_all(&root).ok();

        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.session_id, "child-pi-1");
        assert_eq!(entry.parent_session_id.as_deref(), Some("parent-pi-uuid"));
        assert_eq!(entry.agent_nickname.as_deref(), Some("Subagent"));
    }
}
