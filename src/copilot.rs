//! Read and reconcile Copilot per-call usage without counting snapshots.
use chrono::{DateTime, NaiveDateTime, Utc};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

pub(crate) const PARSER_VERSION: u32 = 2;

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct Metrics {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub reasoning: u64,
    pub duration_ms: u64,
}

impl Metrics {
    fn add(&mut self, other: &Self) -> Result<(), String> {
        for (left, right) in [
            (&mut self.input, other.input),
            (&mut self.output, other.output),
            (&mut self.cache_read, other.cache_read),
            (&mut self.cache_write, other.cache_write),
            (&mut self.reasoning, other.reasoning),
            (&mut self.duration_ms, other.duration_ms),
        ] {
            *left = left
                .checked_add(right)
                .filter(|v| *v <= i64::MAX as u64)
                .ok_or("Copilot 用量超出範圍")?;
        }
        self.input
            .checked_add(self.output)
            .filter(|v| *v <= i64::MAX as u64)
            .ok_or("Copilot token 總數超出範圍")?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Call {
    /// Stable within the source directory and parent session; independent of time/model.
    pub identity: String,
    pub reasoning_effort: Option<String>,
    pub turn_no: Option<u32>,
    /// SQLite identity retained only after a verified cross-source match.
    pub native_identity: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub model: Option<String>,
    pub agent_id: Option<String>,
    pub initiator: Option<String>,
    pub metrics: Metrics,
}

pub(crate) fn parse_timestamp(raw: &str) -> Result<DateTime<Utc>, String> {
    if let Ok(ts) = DateTime::parse_from_rfc3339(raw) {
        return Ok(ts.with_timezone(&Utc));
    }
    // Older session-store versions use timezone-less UTC timestamps.
    for format in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f"] {
        if let Ok(ts) = NaiveDateTime::parse_from_str(raw, format) {
            return Ok(ts.and_utc());
        }
    }
    Err("Copilot 用量缺少有效時間".into())
}

#[derive(Deserialize, Default)]
struct Data {
    usage: Option<Value>,
    #[serde(default, deserialize_with = "nonempty_content")]
    content: bool,
}

// Inspect only whether a reply has text; never retain conversation content.
fn nonempty_content<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    struct Content;
    impl<'de> serde::de::Visitor<'de> for Content {
        type Value = bool;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("reply text or null")
        }
        fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<bool, E> {
            Ok(!value.is_empty())
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<bool, A::Error> {
            while seq.next_element::<serde::de::IgnoredAny>()?.is_some() {}
            Ok(false)
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<bool, A::Error> {
            while map
                .next_entry::<serde::de::IgnoredAny, serde::de::IgnoredAny>()?
                .is_some()
            {}
            Ok(false)
        }
        fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<bool, E> {
            Ok(false)
        }
        fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<bool, E> {
            Ok(false)
        }
        fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<bool, E> {
            Ok(false)
        }
        fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<bool, E> {
            Ok(false)
        }
        fn visit_none<E: serde::de::Error>(self) -> Result<bool, E> {
            Ok(false)
        }
        fn visit_unit<E: serde::de::Error>(self) -> Result<bool, E> {
            Ok(false)
        }
    }
    d.deserialize_any(Content)
}

// Mirror the filtered timeline: consecutive prompts share a turn; the first
// nonempty reply after a prompt advances the next reply. Each agent starts its
// own filtered counter at one, even if first invoked on a later parent turn.
#[derive(Clone)]
struct TurnState {
    next: u32,
    pending: bool,
}
impl TurnState {
    fn new(pending: bool) -> Self {
        Self { next: 1, pending }
    }
}

// Unknown fields are skipped by serde rather than retaining conversations,
// shell output, attachments, and other large unrelated event payloads.
#[derive(Deserialize)]
struct Event {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    data: Data,
    payload: Option<Data>,
    timestamp: Option<String>,
    id: Option<String>,
    #[serde(rename = "agentId")]
    agent_id: Option<String>,
}

pub(crate) fn read_calls(
    path: &Path,
    session_id: &str,
    should_cancel: &(dyn Fn() -> bool + Sync),
) -> Result<Vec<Call>, String> {
    let file = File::open(path).map_err(|e| format!("讀取 Copilot 事件檔失敗: {e}"))?;
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    let mut calls = Vec::new();
    let mut identities: HashMap<(String, Option<String>, String), Call> = HashMap::new();
    let mut turns = HashMap::new();
    let mut seen_prompt = false;
    loop {
        if should_cancel() {
            return Err("Copilot 同步已取消".into());
        }
        line.clear();
        if reader.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            break;
        }
        // Never publish a replacement snapshot while the writer has an
        // incomplete line. The unchanged fingerprint will be retried.
        if !line.ends_with('\n') {
            return Err("Copilot 事件檔仍在寫入，稍後重試".into());
        }
        if line.trim().is_empty() {
            continue;
        }
        let event: Event =
            serde_json::from_str(&line).map_err(|_| "Copilot 事件檔包含無效 JSON".to_string())?;
        if matches!(event.kind.as_str(), "user.message" | "USER_PROMPT") && event.agent_id.is_none()
        {
            seen_prompt = true;
            for state in turns.values_mut() {
                let state: &mut TurnState = state;
                state.pending = true;
            }
        }
        let state = turns
            .entry(event.agent_id.clone().filter(|id| !id.is_empty()))
            .or_insert_with(|| TurnState::new(seen_prompt));
        if matches!(event.kind.as_str(), "assistant.message" | "ASSISTANT_REPLY")
            && event.payload.as_ref().unwrap_or(&event.data).content
            && state.pending
        {
            state.next = state.next.checked_add(1).ok_or("Copilot 回合數超出範圍")?;
            state.pending = false;
        }
        if event.kind != "session.usage_record" {
            continue;
        }
        let usage = event.data.usage.ok_or("Copilot usage record 缺少 usage")?;
        let accounting = &usage["accounting"];
        // Subagents have independent accounting session IDs even though their
        // events are embedded in the parent's transcript. Keep that identity
        // for deduplication while attributing the call via the top-level agent.
        let agent_id = event.agent_id.filter(|s| !s.is_empty());
        let source_session = accounting["sourceSessionId"].as_str().unwrap_or(session_id);
        if agent_id.is_none() && source_session != session_id {
            return Err("Copilot 用量的來源 session 不符".into());
        }
        let identity = accounting["usageId"]
            .as_str()
            .or_else(|| usage["apiCallId"].as_str())
            .or(event.id.as_deref())
            .filter(|s| !s.is_empty())
            .ok_or("Copilot 用量缺少穩定識別碼")?
            .to_owned();
        let count = |key: &str| -> Result<u64, String> {
            match usage.get(key) {
                None | Some(Value::Null) => Ok(0),
                Some(v) => v
                    .as_u64()
                    .filter(|v| *v <= i64::MAX as u64)
                    .ok_or_else(|| format!("Copilot {key} 必須是範圍內的非負整數")),
            }
        };
        let metrics = Metrics {
            input: count("inputTokens")?,
            output: count("outputTokens")?,
            cache_read: count("cacheReadTokens")?,
            cache_write: count("cacheWriteTokens")?,
            reasoning: count("reasoningTokens")?,
            duration_ms: count("duration")?,
        };
        let mut validate = Metrics::default();
        validate.add(&metrics)?;
        let key = (source_session.to_owned(), agent_id.clone(), identity);
        let call = Call {
            turn_no: Some(state.next),
            native_identity: None,
            reasoning_effort: usage["reasoningEffort"].as_str().map(str::to_owned),
            identity: format!(
                "event:{}",
                serde_json::to_string(&key).map_err(|e| e.to_string())?
            ),
            timestamp: parse_timestamp(event.timestamp.as_deref().ok_or("Copilot 用量缺少時間")?)?,
            model: usage["model"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
            agent_id,
            initiator: usage["initiator"].as_str().map(str::to_owned),
            metrics,
        };
        // Agent sequences restart independently. Stable API identities are
        // scoped to the agent, never to sequence or the selected model.
        if let Some(previous) = identities.get(&key) {
            let mut comparable: Call = previous.clone();
            comparable.turn_no = call.turn_no;
            comparable.native_identity = call.native_identity.clone();
            comparable.reasoning_effort = call.reasoning_effort.clone();
            if comparable != call {
                return Err("Copilot 相同用量識別碼的內容不一致".into());
            }
            continue;
        }
        identities.insert(key, call.clone());
        calls.push(call);
    }
    Ok(calls)
}

#[derive(Hash, PartialEq, Eq)]
struct Signature {
    model: Option<String>,
    agent: Option<String>,
    metrics: Metrics,
}

impl From<&Call> for Signature {
    fn from(call: &Call) -> Self {
        Self {
            model: call.model.clone(),
            agent: call.agent_id.clone(),
            metrics: call.metrics.clone(),
        }
    }
}

/// SQLite has no API identity. Match the two representations one-to-one,
/// keeping transcript timestamps (including the correct side of midnight).
/// Refuse ambiguous nearby calls rather than silently double-counting them.
pub(crate) fn merge_calls(
    mut transcript: Vec<Call>,
    native: Vec<Call>,
) -> Result<Vec<Call>, String> {
    let mut candidates: HashMap<Signature, Vec<usize>> = HashMap::new();
    for (index, call) in transcript.iter().enumerate() {
        candidates.entry(call.into()).or_default().push(index);
    }
    let mut matched = vec![false; transcript.len()];
    let mut remaining = Vec::new();
    // Native calls sharing a signature and timestamp are indistinguishable.
    // Pre-count them so an exact hit is consumed only when the pairing is 1:1;
    // otherwise keep both sides untouched and let the caller retry later.
    let mut native_classes: HashMap<(Signature, DateTime<Utc>), usize> = HashMap::new();
    for call in &native {
        *native_classes
            .entry((Signature::from(call), call.timestamp))
            .or_default() += 1;
    }
    // Exact timestamp matches have priority over the proximity fallback.
    for call in native {
        let indices = candidates.get(&Signature::from(&call));
        let exact: Vec<_> = indices
            .into_iter()
            .flatten()
            .copied()
            .filter(|i| !matched[*i] && transcript[*i].timestamp == call.timestamp)
            .collect();
        if exact.len() > 1 {
            return Err("Copilot 相同時間用量無法唯一配對".into());
        }
        if let Some(&i) = exact.first() {
            if native_classes
                .get(&(Signature::from(&call), call.timestamp))
                .is_some_and(|count| *count > 1)
            {
                return Err("Copilot 相同時間用量無法唯一配對".into());
            }
            matched[i] = true;
            transcript[i].native_identity = Some(call.identity.clone());
            if transcript[i].initiator.is_none() {
                transcript[i].initiator = call.initiator.clone();
            }
            if transcript[i].reasoning_effort.is_none() {
                transcript[i].reasoning_effort = call.reasoning_effort.clone();
            }
        } else {
            remaining.push(call);
        }
    }
    let mut edges = Vec::new();
    let mut degree = vec![0usize; transcript.len()];
    for call in &remaining {
        let nearby: Vec<usize> = candidates
            .get(&Signature::from(call))
            .into_iter()
            .flatten()
            .copied()
            .filter(|i| {
                !matched[*i]
                    && (transcript[*i].timestamp - call.timestamp)
                        .num_microseconds()
                        .is_some_and(|delta| delta.unsigned_abs() <= 1_000_000)
            })
            .collect();
        for i in &nearby {
            degree[*i] += 1;
        }
        edges.push(nearby);
    }
    for (call, nearby) in remaining.into_iter().zip(edges) {
        match nearby.as_slice() {
            [] => transcript.push(call),
            [i] if degree[*i] == 1 => {
                transcript[*i].native_identity = Some(call.identity.clone());
                if transcript[*i].initiator.is_none() {
                    transcript[*i].initiator = call.initiator.clone();
                }
                if transcript[*i].reasoning_effort.is_none() {
                    transcript[*i].reasoning_effort = call.reasoning_effort;
                }
            }
            _ => return Err("Copilot 事件檔與 SQLite 用量無法唯一配對，保留原資料待重試".into()),
        }
    }
    Ok(transcript)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn read(events: &[Value], tail: &str, cancel: bool) -> Result<Vec<Call>, String> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "copilot-parser-{}-{}.jsonl",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let content = events.iter().map(|e| format!("{e}\n")).collect::<String>() + tail;
        std::fs::write(&path, content).unwrap();
        let result = read_calls(&path, "s", &|| cancel);
        std::fs::remove_file(path).unwrap();
        result
    }

    fn event(id: &str, time: &str) -> Value {
        json!({"type":"session.usage_record", "id":id, "timestamp":time,
            "data":{"usage":{"model":"A", "inputTokens":100, "outputTokens":10,
                "cacheReadTokens":80, "reasoningTokens":5, "duration":50,
                "accounting":{"sourceSessionId":"s", "usageId":id, "sequence":1}}}})
    }

    fn call(time: &str) -> Call {
        read(&[event("a", time)], "", false).unwrap().remove(0)
    }

    #[test]
    fn native_metadata_fills_missing_initiator_without_erasing_unknown_turns() {
        for native_time in ["2026-07-22T10:00:00Z", "2026-07-22T10:00:00.500Z"] {
            let mut transcript = call("2026-07-22T10:00:00Z");
            transcript.agent_id = Some("child".into());
            transcript.turn_no = Some(2);
            let mut native = transcript.clone();
            native.identity = "sqlite:1".into();
            native.timestamp = parse_timestamp(native_time).unwrap();
            native.turn_no = None;
            native.initiator = Some("sub-agent".into());
            let merged = merge_calls(vec![transcript], vec![native]).unwrap();
            assert_eq!(merged[0].initiator.as_deref(), Some("sub-agent"));
            assert_eq!(merged[0].turn_no, Some(2));
            assert_eq!(merged[0].native_identity.as_deref(), Some("sqlite:1"));
        }
    }

    #[test]
    fn ambiguous_exact_matches_do_not_assign_native_turn_to_arbitrary_event() {
        let a = call("2026-07-22T10:00:00Z");
        let mut b = a.clone();
        b.identity = "event:other".into();
        b.turn_no = Some(2);
        let mut native = b.clone();
        native.identity = "sqlite:1".into();
        assert!(merge_calls(vec![a, b], vec![native]).is_err());
    }

    #[test]
    fn ambiguous_native_exact_pair_keeps_both_sides_for_retry() {
        let mut transcript = call("2026-07-22T10:00:00Z");
        transcript.turn_no = Some(2);
        let mut first = transcript.clone();
        first.identity = "sqlite:1".into();
        first.turn_no = Some(1);
        let mut second = transcript.clone();
        second.identity = "sqlite:2".into();
        second.turn_no = Some(1);
        assert!(merge_calls(vec![transcript], vec![first, second]).is_err());
    }

    #[test]
    fn paired_calls_keep_the_event_derived_turn() {
        for native_time in ["2026-07-22T10:00:00Z", "2026-07-22T10:00:00.500Z"] {
            let mut transcript = call("2026-07-22T10:00:00Z");
            transcript.turn_no = Some(2);
            let mut native = transcript.clone();
            native.identity = "sqlite:1".into();
            native.timestamp = parse_timestamp(native_time).unwrap();
            native.turn_no = Some(1);
            native.agent_id = None;
            let merged = merge_calls(vec![transcript], vec![native]).unwrap();
            assert_eq!(merged[0].turn_no, Some(2), "native_time={native_time}");
        }
    }

    #[test]
    fn unrelated_structured_content_is_ignored_without_retaining_it() {
        let events = [
            json!({"type":"system.message","data":{"content":[{"text":"fixture"}]}}),
            event("a", "2026-07-22T10:00:00Z"),
        ];
        assert_eq!(read(&events, "", false).unwrap().len(), 1);
    }

    #[test]
    fn ignores_cumulative_snapshots_and_dedupes_ids_not_sequences() {
        let a = event("a", "2026-07-22T23:59:59Z");
        let mut b = event("b", "2026-07-23T00:00:01Z");
        b["agentId"] = json!("child");
        b["data"]["usage"]["accounting"]["sourceSessionId"] = json!("child-session");
        b["data"]["usage"]["initiator"] = json!("sub-agent");
        b["data"]["usage"]["model"] = json!("B");
        let rows = read(&[a.clone(), a, b,
            json!({"type":"session.shutdown","data":{"modelMetrics":{"A":{"inputTokens":999}}}}),
            json!({"type":"session.usage_checkpoint","data":{"usage":{}}})], "", false).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].agent_id.as_deref(), Some("child"));
        assert_eq!(rows[1].model.as_deref(), Some("B"));
        assert_eq!(rows[1].metrics.input + rows[1].metrics.output, 110);
        assert_ne!(rows[0].identity, rows[1].identity);
    }

    #[test]
    fn falls_back_to_api_then_event_id_and_rejects_conflicting_duplicates() {
        let mut a = event("a", "2026-07-22T10:00:00Z");
        a["data"]["usage"]["accounting"] = Value::Null;
        a["data"]["usage"]["apiCallId"] = json!("api-a");
        let mut duplicate = a.clone();
        duplicate["id"] = json!("another-event");
        assert_eq!(read(&[a.clone(), duplicate], "", false).unwrap().len(), 1);
        a["data"]["usage"]["apiCallId"] = Value::Null;
        assert_eq!(read(&[a.clone()], "", false).unwrap().len(), 1);
        let mut conflict = a.clone();
        conflict["data"]["usage"]["inputTokens"] = json!(101);
        assert!(read(&[a, conflict], "", false).is_err());
    }

    #[test]
    fn rejects_invalid_incomplete_cancelled_or_overflowing_usage() {
        let a = event("a", "2026-07-22T10:00:00Z");
        assert!(read(std::slice::from_ref(&a), "{", false).is_err());
        assert!(read(std::slice::from_ref(&a), "{bad}\n", false).is_err());
        assert!(read(std::slice::from_ref(&a), "", true).is_err());
        for invalid in [json!(-1), json!(1.5), json!(u64::MAX)] {
            let mut bad = a.clone();
            bad["data"]["usage"]["inputTokens"] = invalid;
            assert!(read(&[bad], "", false).is_err());
        }
        let mut bad = a.clone();
        bad["data"]["usage"]["accounting"]["sourceSessionId"] = json!("other");
        assert!(read(&[bad], "", false).is_err());
        let mut bad = a;
        bad["data"]["usage"]["inputTokens"] = json!(i64::MAX);
        assert!(read(&[bad], "", false).is_err());
    }

    #[test]
    fn merges_each_call_once_with_clock_jitter_and_preserves_sqlite_only_calls() {
        let a = call("2026-07-22T23:59:59.999Z");
        let mut native = a.clone();
        native.timestamp = parse_timestamp("2026-07-23T00:00:00.002Z").unwrap();
        let b = call("2026-07-23T01:00:00Z");
        let c = call("2026-07-23T02:00:00Z");
        let merged = merge_calls(vec![a.clone(), b], vec![native, c]).unwrap();
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].timestamp, a.timestamp);
        assert_eq!(merged.iter().map(|c| c.metrics.input).sum::<u64>(), 300);
        // Exact matches win before a neighbouring call is considered.
        let mut nearby = a.clone();
        nearby.timestamp += chrono::Duration::milliseconds(1);
        assert_eq!(
            merge_calls(vec![a.clone(), nearby.clone()], vec![nearby, a])
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn preserves_distinct_identical_calls_and_refuses_ambiguous_pairing() {
        let a = call("2026-07-22T10:00:00Z");
        let b = call("2026-07-22T10:00:00.010Z");
        let middle = call("2026-07-22T10:00:00.005Z");
        assert_eq!(
            merge_calls(vec![a.clone(), b.clone()], vec![])
                .unwrap()
                .len(),
            2
        );
        assert!(merge_calls(vec![a.clone(), b], vec![middle.clone()]).is_err());
        assert!(merge_calls(vec![a], vec![middle.clone(), middle]).is_err());
    }

    #[test]
    fn normalizes_call_times_to_utc_and_rejects_sum_overflow() {
        let a = call("2026-07-23T07:59:59+08:00");
        let b = call("2026-07-23T08:00:00+08:00");
        assert_eq!(
            a.timestamp,
            parse_timestamp("2026-07-22T23:59:59Z").unwrap()
        );
        assert_eq!(
            b.timestamp,
            parse_timestamp("2026-07-23T00:00:00Z").unwrap()
        );
        let mut huge = a.metrics.clone();
        huge.input = i64::MAX as u64 - 10;
        assert!(huge.add(&a.metrics).is_err());
    }
}
