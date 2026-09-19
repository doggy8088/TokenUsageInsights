//! MiniMax Code (`mcode`) local session parser.
//!
//! MiniMax Code keeps every session in its own dated directory:
//!
//! ```text
//! <dir>/sessions/<YYYY>/<MM>/<DD>/<HH-MM-SS-mmm>-session_<base64url>/
//! ```
//!
//! The conversation is spread across two newline-delimited JSON streams that
//! share the record shape `{"message_id", "turn_id", "message": { ... }}`:
//!
//! * `messages.jsonl` — the displayed/compacted transcript
//! * `snapshots/*.jsonl` — the per-context replay records
//!
//! The two streams do not overlap, so the authoritative per-session view is
//! their union deduplicated by `message_id` and ordered by `message.timestamp`
//! (a millisecond epoch integer). Session identity comes from `manifest.json`
//! (falling back to the base64url directory suffix), while the working
//! directory and session title come from MiniMax Code's runtime SQLite ledger
//! (`db::sync_mcode_usage_logs`) rather than from prompt text.
use crate::db::{CostStats, TokenStats, UsageEntry};
use serde_json::Value;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub(crate) const SOURCE_KIND: &str = "mcode-session";

const MESSAGES_FILE_NAME: &str = "messages.jsonl";
const SNAPSHOTS_DIR_NAME: &str = "snapshots";
const MANIFEST_FILE_NAME: &str = "manifest.json";
const SESSION_DIR_PREFIX: &str = "session_";

fn value_as_u64(value: Option<&Value>) -> Option<u64> {
    value.and_then(|value| {
        value
            .as_u64()
            .or_else(|| value.as_i64().and_then(|number| u64::try_from(number).ok()))
    })
}

/// Millisecond epoch timestamp of one transcript record.
pub(crate) fn record_timestamp(record: &Value) -> Option<i64> {
    record
        .get("message")
        .and_then(|message| message.get("timestamp"))
        .and_then(Value::as_i64)
}

/// Formats a millisecond epoch timestamp as a UTC RFC 3339 string. The result
/// always starts with `YYYY-MM-DD` and ends with `Z`, matching the timestamps
/// written for every other assistant so date bucketing keeps working.
pub(crate) fn epoch_ms_to_rfc3339(epoch_ms: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(epoch_ms)
        .map(|datetime| datetime.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .unwrap_or_default()
}

fn decode_base64url(input: &str) -> Option<String> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

    let mut buffer: u32 = 0;
    let mut bits: u32 = 0;
    let mut bytes = Vec::new();
    for byte in input.bytes() {
        if byte == b'=' {
            break;
        }
        let value = ALPHABET.iter().position(|candidate| *candidate == byte)? as u32;
        buffer = (buffer << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push(((buffer >> bits) & 0xFF) as u8);
            buffer &= (1u32 << bits) - 1;
        }
    }

    let decoded = String::from_utf8(bytes).ok()?;
    (!decoded.is_empty()).then_some(decoded)
}

/// Resolves a session directory to its session id. `manifest.json` is the
/// authoritative source; when it is missing or unreadable the base64url suffix
/// of the directory name (`...-session_<base64url>`) is decoded instead.
pub(crate) fn read_session_metadata(session_dir: &Path) -> String {
    let directory_name = session_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("unknown");

    if let Ok(contents) = fs::read_to_string(session_dir.join(MANIFEST_FILE_NAME)) {
        if let Ok(manifest) = serde_json::from_str::<Value>(&contents) {
            if let Some(session_id) = manifest.get("sessionId").and_then(Value::as_str) {
                if !session_id.trim().is_empty() {
                    return session_id.to_string();
                }
            }
        }
    }

    directory_name
        .rsplit_once(SESSION_DIR_PREFIX)
        .and_then(|(_, suffix)| decode_base64url(suffix))
        .unwrap_or_else(|| directory_name.to_string())
}

/// Path recorded as the session transcript. Every session directory carries
/// `messages.jsonl`, so it doubles as the stable identity used to scope the
/// delete-and-reinsert rebuild.
pub(crate) fn session_transcript_path(session_dir: &Path) -> PathBuf {
    session_dir.join(MESSAGES_FILE_NAME)
}

/// Every JSONL file that belongs to one session, in read order: the displayed
/// transcript first, then the dated snapshot records.
pub(crate) fn session_jsonl_files(session_dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let messages = session_transcript_path(session_dir);
    if messages.is_file() {
        files.push(messages);
    }

    let mut snapshots = Vec::new();
    if let Ok(entries) = fs::read_dir(session_dir.join(SNAPSHOTS_DIR_NAME)) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
                snapshots.push(path);
            }
        }
    }
    snapshots.sort();
    files.extend(snapshots);
    files
}

/// True when `path` is a MiniMax Code session directory. Sessions always carry
/// a `messages.jsonl` transcript, so it doubles as the marker used to stop the
/// recursive scan from descending into `snapshots/`, `reports/`, and
/// `artifacts/`.
fn is_session_dir(path: &Path) -> bool {
    session_transcript_path(path).is_file()
}

fn collect_session_dirs(directory: &Path, session_dirs: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            continue;
        }
        if is_session_dir(&path) {
            session_dirs.push(path);
        } else {
            collect_session_dirs(&path, session_dirs);
        }
    }
}

/// Recursively collects every session directory under `<dir>/sessions/`.
pub(crate) fn find_session_dirs(dir: &Path) -> Vec<PathBuf> {
    let mut session_dirs = Vec::new();
    let sessions_root = dir.join("sessions");
    if sessions_root.is_dir() {
        collect_session_dirs(&sessions_root, &mut session_dirs);
    } else if dir.is_dir() {
        // The caller may already point at the sharded sessions root.
        collect_session_dirs(dir, &mut session_dirs);
    }
    session_dirs.sort();
    session_dirs
}

/// Reads the union of a session's JSONL streams, deduplicated by `message_id`
/// and ordered chronologically. Records without a `message_id` are kept
/// verbatim so unexpected shapes are not silently dropped.
pub(crate) fn collect_session_records(session_dir: &Path) -> Vec<Value> {
    let mut seen = std::collections::HashSet::new();
    let mut records = Vec::new();

    for path in session_jsonl_files(session_dir) {
        let Ok(file) = File::open(&path) else {
            continue;
        };
        for line in BufReader::new(file).lines() {
            let Ok(line) = line else {
                continue;
            };
            if line.trim().is_empty() {
                continue;
            }
            let Ok(record) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            match record.get("message_id").and_then(Value::as_str) {
                Some(message_id) => {
                    if seen.insert(message_id.to_string()) {
                        records.push(record);
                    }
                }
                None => records.push(record),
            }
        }
    }

    // The individual streams are chronological but interleave (the snapshots
    // are written before the compacted transcript), so sort once across the
    // union using a stable sort to keep file order for identical timestamps.
    records.sort_by_key(|record| record_timestamp(record).unwrap_or(i64::MIN));
    records
}

/// Token counts for one assistant turn. MiniMax Code reports `input` as
/// non-cached input, which maps directly onto `TokenStats::input`.
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

/// The single predicate shared by the usage and timeline parsers: a message
/// only counts as a billable turn when it is an assistant message carrying a
/// usage block that yields non-zero token counts.
pub(crate) fn assistant_usage(message: &Value) -> Option<TokenStats> {
    if message.get("role").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    parse_token_stats(message.get("usage")?)
}

/// A reported cost of `0` means the provider did not bill the call (MiniMax
/// Code's own ledger records `cost.total == 0` for custom providers), so it is
/// dropped and `pricing.csv` estimation is used instead.
fn reported_cost_usd(usage: &Value) -> Option<f64> {
    crate::pi::parse_reported_cost(usage).filter(|value| *value > 0.0)
}

/// Parses one session directory into per-turn [`UsageEntry`] rows. `cwd` and
/// `session_name` are supplied by the caller from MiniMax Code's runtime
/// SQLite ledger.
pub(crate) fn parse_session_usage(
    session_dir: &Path,
    cwd: Option<&str>,
    session_name: Option<&str>,
) -> Result<Vec<UsageEntry>, String> {
    let session_id = read_session_metadata(session_dir);
    let transcript_path = session_transcript_path(session_dir)
        .to_string_lossy()
        .into_owned();

    let mut entries = Vec::new();
    let mut turn_no = 1u32;
    for record in collect_session_records(session_dir) {
        let Some(message) = record.get("message") else {
            continue;
        };
        let Some(tokens) = assistant_usage(message) else {
            continue;
        };
        let timestamp = record_timestamp(&record)
            .map(epoch_ms_to_rfc3339)
            .unwrap_or_default();
        let model = message
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string);
        let cost = message
            .get("usage")
            .and_then(reported_cost_usd)
            .map(|reported_cost_usd| CostStats {
                total_api_duration_ms: None,
                total_duration_ms: None,
                total_premium_requests: None,
                reported_cost_usd: Some(reported_cost_usd),
            });

        entries.push(UsageEntry {
            timestamp,
            session_id: session_id.clone(),
            session_name: session_name.map(str::to_string),
            transcript_path: Some(transcript_path.clone()),
            cwd: cwd.map(str::to_string),
            version: None,
            turn_no,
            model: model.clone(),
            model_id: model,
            tokens: Some(tokens.clone()),
            delta_tokens: Some(tokens),
            context: None,
            cost,
            source_kind: Some(SOURCE_KIND.to_string()),
            source_dir_key: None,
            parent_session_id: None,
            agent_nickname: None,
            agent_role: None,
            reasoning_effort: None,
        });
        turn_no += 1;
    }

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_dir(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "token-usage-insights-mcode-test-{}-{}",
            label,
            std::process::id()
        ));
        fs::remove_dir_all(&root).ok();
        root
    }

    fn write_lines(path: &Path, lines: &[String]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut file = File::create(path).unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
    }

    fn write_empty_file(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, "").unwrap();
    }

    fn user_record(message_id: &str, timestamp: i64, text: &str) -> String {
        format!(
            r#"{{"message_id":"{message_id}","turn_id":"turn_1","message":{{"role":"user","content":[{{"type":"text","text":"{text}"}}],"timestamp":{timestamp}}}}}"#
        )
    }

    fn assistant_record(
        message_id: &str,
        timestamp: i64,
        input: u64,
        output: u64,
        cache_read: u64,
        cost_total: f64,
    ) -> String {
        format!(
            r#"{{"message_id":"{message_id}","turn_id":"turn_1","message":{{"role":"assistant","content":[{{"type":"text","text":"done"}}],"provider":"custom_provider:llmshare","model":"deepseek-v4.1-flash","usage":{{"input":{input},"output":{output},"cacheRead":{cache_read},"cacheWrite":0,"totalTokens":{},"cost":{{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":{cost_total}}}}},"timestamp":{timestamp}}}}}"#,
            input + output + cache_read
        )
    }

    fn session_dir(root: &Path, dir_name: &str) -> PathBuf {
        let dir = root
            .join("sessions")
            .join("2026")
            .join("09")
            .join("19")
            .join(dir_name);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn find_session_dirs_discovers_dated_sessions_without_descending_into_them() {
        let root = temp_dir("find-dirs");
        let dir = session_dir(
            &root,
            "15-16-05-177-session_bXZzXzU5YjdmZjExNWZjOTQwOWFhN2IwMTdhYWMyMDhmYjI4",
        );
        write_empty_file(&dir.join("messages.jsonl"));
        // Nested report artifacts also contain `.jsonl` files but must not be
        // treated as sessions, and the scan must not recurse past a session.
        write_empty_file(&dir.join("reports").join("tool-outputs").join("tool.jsonl"));

        let dirs = find_session_dirs(&root);
        fs::remove_dir_all(&root).ok();

        assert_eq!(dirs, vec![dir]);
    }

    #[test]
    fn session_jsonl_files_lists_messages_then_sorted_snapshots() {
        let root = temp_dir("jsonl-files");
        let dir = session_dir(&root, "17-10-23-777-session_abc");
        write_empty_file(&dir.join("messages.jsonl"));
        write_empty_file(&dir.join("snapshots").join("g000000000001--ctx_b.jsonl"));
        write_empty_file(&dir.join("snapshots").join("g000000000000--ctx_a.jsonl"));
        write_empty_file(&dir.join("snapshots").join("env-g000000000000--ctx_a.json"));
        write_empty_file(
            &dir.join("reports")
                .join("tool-outputs")
                .join("tool.readable.v1.jsonl"),
        );

        let files = session_jsonl_files(&dir);
        fs::remove_dir_all(&root).ok();

        assert_eq!(session_transcript_path(&dir), dir.join("messages.jsonl"));
        assert_eq!(
            files,
            vec![
                dir.join("messages.jsonl"),
                dir.join("snapshots").join("g000000000000--ctx_a.jsonl"),
                dir.join("snapshots").join("g000000000001--ctx_b.jsonl"),
            ]
        );
    }

    #[test]
    fn read_session_metadata_prefers_manifest_then_base64url_directory_suffix() {
        let root = temp_dir("session-id");
        let dir = session_dir(
            &root,
            "15-16-05-177-session_bXZzXzU5YjdmZjExNWZjOTQwOWFhN2IwMTdhYWMyMDhmYjI4",
        );
        fs::write(
            dir.join("manifest.json"),
            r#"{"schemaVersion":1,"sessionId":"mvs_from_manifest"}"#,
        )
        .unwrap();
        assert_eq!(read_session_metadata(&dir), "mvs_from_manifest");

        fs::remove_file(dir.join("manifest.json")).unwrap();
        assert_eq!(
            read_session_metadata(&dir),
            "mvs_59b7ff115fc9409aa7b017aac208fb28"
        );

        let plain = session_dir(&root, "not-a-minimax-directory");
        assert_eq!(read_session_metadata(&plain), "not-a-minimax-directory");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn epoch_ms_to_rfc3339_formats_utc_millis_with_trailing_z() {
        assert_eq!(
            epoch_ms_to_rfc3339(1_789_830_965_177),
            "2026-09-19T15:16:05.177Z"
        );
        assert_eq!(epoch_ms_to_rfc3339(i64::MAX), "");
    }

    #[test]
    fn parse_session_usage_unions_streams_dedupes_and_orders_turns_by_timestamp() {
        let root = temp_dir("union");
        let dir = session_dir(
            &root,
            "15-16-05-177-session_bXZzXzU5YjdmZjExNWZjOTQwOWFhN2IwMTdhYWMyMDhmYjI4",
        );
        // `messages.jsonl` holds the later, compacted records while the
        // snapshots hold the earlier ones; a shared record appears in both.
        write_lines(
            &dir.join("messages.jsonl"),
            &[
                assistant_record("msg-late", 1_789_831_000_000, 40, 4, 100, 0.0),
                assistant_record("msg-shared", 1_789_830_900_000, 30, 3, 90, 0.0),
            ],
        );
        write_lines(
            &dir.join("snapshots").join("g000000000000--ctx_a.jsonl"),
            &[
                assistant_record("msg-early", 1_789_830_800_000, 10, 1, 70, 0.0),
                // Duplicate of the `messages.jsonl` record: must be dropped.
                assistant_record("msg-shared", 1_789_830_900_000, 30, 3, 90, 0.0),
            ],
        );

        let entries = parse_session_usage(&dir, Some("/tmp/project"), None).unwrap();
        fs::remove_dir_all(&root).ok();

        assert_eq!(entries.len(), 3);
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.turn_no)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(
            entries[0].tokens.as_ref().unwrap().input,
            10,
            "oldest snapshot turn must be numbered first"
        );
        assert_eq!(entries[2].timestamp, "2026-09-19T15:16:40.000Z");
        assert!(entries.iter().all(|entry| {
            entry.session_id == "mvs_59b7ff115fc9409aa7b017aac208fb28"
                && entry.session_name.is_none()
                && entry.cwd.as_deref() == Some("/tmp/project")
                && entry.source_kind.as_deref() == Some(SOURCE_KIND)
                && entry.model.as_deref() == Some("deepseek-v4.1-flash")
        }));
    }

    #[test]
    fn parse_session_usage_maps_usage_and_drops_zero_reported_cost() {
        let root = temp_dir("usage-mapping");
        let dir = session_dir(
            &root,
            "13-23-40-436-session_bXZzX2E0YzYyZThhNDFhZTRiNzViMmQ1OTgxMDg4NjliYjg2",
        );
        write_lines(
            &dir.join("messages.jsonl"),
            &[assistant_record(
                "msg-1",
                1_789_824_220_670,
                18_615,
                161,
                0,
                0.0,
            )],
        );

        let entries = parse_session_usage(&dir, None, Some("Runtime title")).unwrap();
        fs::remove_dir_all(&root).ok();

        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.session_name.as_deref(), Some("Runtime title"));
        assert_eq!(entry.timestamp, "2026-09-19T13:23:40.670Z");
        let tokens = entry.tokens.as_ref().unwrap();
        assert_eq!(tokens.input, 18_615);
        assert_eq!(tokens.output, 161);
        assert_eq!(tokens.cache_read, None);
        assert_eq!(tokens.total, 18_776);
        assert!(
            entry.cost.is_none(),
            "cost.total == 0 must fall back to pricing.csv estimation"
        );
        assert_eq!(entry.delta_tokens.as_ref().unwrap().total, 18_776);
    }

    #[test]
    fn parse_session_usage_keeps_positive_reported_cost_and_skips_non_turns() {
        let root = temp_dir("non-turns");
        let dir = session_dir(
            &root,
            "13-30-15-760-session_bXZzXzczZDQ2ZWY0YWExNjQ1ODc5YzBiMmE2OWQ0ZWIxOWU4",
        );
        write_lines(
            &dir.join("messages.jsonl"),
            &[
                user_record("msg-user", 1_789_824_615_000, "hello"),
                r#"{"message_id":"msg-custom","turn_id":"turn_1","message":{"role":"custom","customType":"todo_cadence_reminder","content":"reminder","timestamp":1789824616000}}"#.to_string(),
                r#"{"message_id":"msg-compaction","turn_id":"turn_1","message":{"role":"compactionSummary","summary":"sum","tokensBefore":10,"timestamp":1789824617000}}"#.to_string(),
                r#"{"message_id":"msg-no-usage","turn_id":"turn_1","message":{"role":"assistant","content":[{"type":"text","text":"hi"}],"timestamp":1789824618000}}"#.to_string(),
                assistant_record("msg-zero-usage", 1_789_824_619_000, 0, 0, 0, 0.0),
                assistant_record("msg-cost", 1_789_824_620_000, 100, 50, 10, 0.0031),
            ],
        );

        let entries = parse_session_usage(&dir, None, None).unwrap();
        fs::remove_dir_all(&root).ok();

        assert_eq!(entries.len(), 1, "only the billable assistant turn counts");
        let entry = &entries[0];
        assert_eq!(entry.turn_no, 1);
        assert_eq!(
            entry.cost.as_ref().and_then(|cost| cost.reported_cost_usd),
            Some(0.0031)
        );
        let tokens = entry.tokens.as_ref().unwrap();
        assert_eq!(tokens.input, 100);
        assert_eq!(tokens.output, 50);
        assert_eq!(tokens.cache_read, Some(10));
    }

    #[test]
    fn decode_base64url_rejects_invalid_input() {
        assert_eq!(
            decode_base64url("bXZzXzU5YjdmZjExNWZjOTQwOWFhN2IwMTdhYWMyMDhmYjI4").as_deref(),
            Some("mvs_59b7ff115fc9409aa7b017aac208fb28")
        );
        assert_eq!(decode_base64url("not*base64").as_deref(), None);
        assert_eq!(decode_base64url("").as_deref(), None);
    }
}
