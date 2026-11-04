//! Read and reconcile Copilot per-call usage without counting snapshots.
use chrono::{DateTime, NaiveDateTime, Utc};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

// Parser/reconciliation revision, hashed into the Copilot CLI/App transcript
// fingerprints: bumping it re-syncs every unchanged session exactly once.
// 回合推導改為「每則使用者訊息一個統計回合」並新增回覆累計顯示時升版。
pub(crate) const PARSER_VERSION: u32 = 6;

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

    /// 解析單一 `session.usage_record` 的 `usage` 物件。缺欄與明確 null 視同 0。
    /// `read_calls` 的逐呼叫用量列與時間軸的回覆累計共用此解析，兩側對同一次
    /// 呼叫永遠得到同一組數值。
    pub(crate) fn from_usage(usage: &Value) -> Result<Metrics, String> {
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
        Ok(metrics)
    }
}

/// 時間軸側讀出單一 `session.usage_record` 事件的用量。與 `read_calls` 的
/// typed 解析同規則：只認新格式 `data.usage`，明確 null 視同缺欄；缺少
/// `usage` 傳回 `None`（匯入端對此仍屬錯誤並保留原資料待重試，時間軸僅
/// 不累計該筆）。
pub(crate) fn usage_record_metrics(event: &Value) -> Option<Result<Metrics, String>> {
    let usage = event
        .get("data")
        .filter(|value| !value.is_null())
        .and_then(|data| data.get("usage"))
        .filter(|value| !value.is_null())?;
    Some(Metrics::from_usage(usage))
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
}

// The shared body precedence for Copilot events (review 4433863670): an
// explicit `"payload": null` means "no payload" and falls back to the other
// side — the same rule `Option<Data>` encodes on the typed import path, where
// a null `payload` deserializes to `None` and `unwrap_or(&data)` reads `data`.
// Keeping one helper pins both sides to identical explicit-null precedence.
// `payload`-first callers pass `(payload, data)`; new-format `data`-first
// callers pass `(data, payload)` — the parameter order is the precedence.
pub(crate) fn event_body<'a>(
    first: Option<&'a Value>,
    second: Option<&'a Value>,
) -> Option<&'a Value> {
    first
        .filter(|v| !v.is_null())
        .or_else(|| second.filter(|v| !v.is_null()))
}

// 統計回合 = 每則「有主體」的使用者訊息，以該 Agent 檢視可見的提示序號編號
// (`max(1, 已見提示數)`)：同一則訊息的多次 API 呼叫（工具呼叫、中途文字回覆、
// final）同屬一個回合，回覆不再推進回合。共用提示（無 agentId）計入所有 Agent
// 檢視，專屬提示只計入該 Agent；三個回合推導站點（本函式、
// `timeline::parse_copilot_timeline_filtered`、`db::copilot_filtered_agent_turns`）
// 必須同步，詳情頁的用量才會掛回自己的回覆。
// 這與來源 `assistant_usage_events.turn_index` 的原生語意一致（一個回合可收到
// 多次 API 呼叫），SQLite-only 呼叫的 `turn_index + 1` 因此與主時間軸同編號。
// 無主體提示在時間軸完全不可見，不計入序號（review 4433948800）。
#[derive(Clone)]
struct TurnState {
    /// 該檢視已見的有主體提示數。
    prompts: u32,
}
impl TurnState {
    fn new(prompts: u32) -> Self {
        Self { prompts }
    }
    fn turn_no(&self) -> u32 {
        self.prompts.max(1)
    }
    fn advance(&mut self) -> Result<(), String> {
        self.prompts = self
            .prompts
            .checked_add(1)
            .ok_or("Copilot 回合數超出範圍")?;
        Ok(())
    }
}

// Unknown fields are skipped by serde rather than retaining conversations,
// shell output, attachments, and other large unrelated event payloads.
#[derive(Deserialize)]
struct Event {
    #[serde(rename = "type")]
    kind: String,
    // `Option<Data>` makes an explicit `"data": null` and a missing `data` key
    // indistinguishable, matching `event_body`'s explicit-null rule on the
    // Value-based sides (review 4433948800).
    data: Option<Data>,
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
    let mut calls: Vec<Call> = Vec::new();
    let mut identities: HashMap<(String, Option<String>, String), usize> = HashMap::new();
    let mut turns: HashMap<Option<String>, TurnState> = HashMap::new();
    let mut shared_prompts = 0u32;
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
        // A prompt opens a turn only when its event body is present — the same
        // `payload`/`data` non-null rule the filtered timeline applies through
        // `event_body`. Classifying by type alone lets `{"type":"user.message",
        // "payload":null}` push usage onto a turn the timeline never uses, so
        // the reply cannot attach its usage (review 4433948800).
        // A prompt counts only when its event body is present — the same
        // `payload`/`data` non-null rule the filtered timeline applies through
        // `event_body`. Classifying by type alone lets `{"type":"user.message",
        // "payload":null}` claim a turn the timeline never shows, misplacing
        // every later reply's usage (review 4433948800).
        let is_prompt = matches!(event.kind.as_str(), "user.message" | "USER_PROMPT")
            && (event.payload.is_some() || event.data.is_some());
        // One empty-`agentId` rule for prompt classification, the entry key,
        // and usage attribution: an empty id is the shared timeline (`None`).
        let agent = event.agent_id.filter(|id| !id.is_empty());
        if is_prompt && agent.is_none() {
            // 共用提示計入所有 Agent 檢視（含尚未建立的：其狀態以當前共用
            // 提示數起步），每個檢視的回合序號永遠與自己的提示列同編號。
            shared_prompts = shared_prompts
                .checked_add(1)
                .ok_or("Copilot 回合數超出範圍")?;
            for state in turns.values_mut() {
                let state: &mut TurnState = state;
                state.advance()?;
            }
        }
        let state = turns
            .entry(agent.clone())
            .or_insert_with(|| TurnState::new(shared_prompts));
        if is_prompt {
            // (review comment 4436837470) 專屬提示是該子 Agent 自己的使用者
            // 訊息，只推進它自己的回合序號；共用提示已在上面推進所有狀態
            // （此處於共用提示時重設為同一值，冪等）。
            if agent.is_some() {
                state.advance()?;
            }
        }
        if event.kind != "session.usage_record" {
            continue;
        }
        let usage = event
            .data
            .and_then(|data| data.usage)
            .ok_or("Copilot usage record 缺少 usage")?;
        let accounting = &usage["accounting"];
        // Subagents have independent accounting session IDs even though their
        // events are embedded in the parent's transcript. Keep that identity
        // for deduplication while attributing the call via the top-level agent.
        let agent_id = agent;
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
        let metrics = Metrics::from_usage(&usage)?;
        let key = (source_session.to_owned(), agent_id.clone(), identity);
        let call = Call {
            turn_no: Some(state.turn_no()),
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
        if let Some(&idx) = identities.get(&key) {
            let mut comparable: Call = calls[idx].clone();
            comparable.turn_no = call.turn_no;
            comparable.native_identity = call.native_identity.clone();
            comparable.reasoning_effort = call.reasoning_effort.clone();
            if comparable != call {
                return Err("Copilot 相同用量識別碼的內容不一致".into());
            }
            // Fill-only metadata merge into the retained call: a later
            // duplicate may carry metadata the first copy lacks
            // (absent-then-present). The first `Some` still wins and strict
            // fields stay loud errors above (thread PRRT_kwDOTOgsec6yUXNX).
            if calls[idx].reasoning_effort.is_none() {
                calls[idx].reasoning_effort = call.reasoning_effort;
            }
            continue;
        }
        identities.insert(key, calls.len());
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

/// Index key for the absent-versus-nonzero twin check in `merge_calls`:
/// (model, agent, timestamp, input, output).
type TwinKey = (Option<String>, Option<String>, DateTime<Utc>, u64, u64);

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
    // (review 4433948817) Index the event-file rows by (time, model, agent,
    // input, output). When a native call shares that key with an event row but
    // their `Signature` differs, some metric the event file omitted (parsed as
    // 0) is non-zero in SQLite — reasoning, cache or duration. The all-or-nothing
    // `Signature` then misses on both the exact and window gates and the call
    // would be appended as an independent row, double-counting its whole
    // input/output. A shared key is the fingerprint of that drift shape.
    let mut twins: HashSet<TwinKey> = HashSet::new();
    for call in &transcript {
        twins.insert((
            call.model.clone(),
            call.agent_id.clone(),
            call.timestamp,
            call.metrics.input,
            call.metrics.output,
        ));
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
    let mut stolen_edges = Vec::new();
    let mut degree = vec![0usize; transcript.len()];
    for call in &remaining {
        let mut nearby = Vec::new();
        let mut stolen = Vec::new();
        for i in candidates
            .get(&Signature::from(call))
            .into_iter()
            .flatten()
            .copied()
        {
            let within_window = (transcript[i].timestamp - call.timestamp)
                .num_microseconds()
                .is_some_and(|delta| delta.unsigned_abs() <= 1_000_000);
            if !within_window {
                continue;
            }
            if matched[i] {
                // (review 4433863665) A window candidate consumed by an exact
                // match stays a conflict signal: it was this call's near
                // representation too, so an empty remaining set no longer
                // proves the call is independent of the event file.
                stolen.push(i);
            } else {
                nearby.push(i);
                degree[i] += 1;
            }
        }
        edges.push(nearby);
        stolen_edges.push(stolen);
    }
    for ((call, nearby), stolen) in remaining.into_iter().zip(edges).zip(stolen_edges) {
        match nearby.as_slice() {
            // Only an untouched window proves an independent SQLite-only call.
            // Without the `stolen` guard an exact hit can consume another
            // call's only near candidate and this arm appends a phantom
            // SQLite-only row, double-counting the same usage (review
            // 4433863665). The `twins` guard extends the same rule to
            // signature-drifted twins (review 4433948817): a same-time event
            // row that matches on model/agent/input/output necessarily
            // disagrees on another metric (equal ones would have matched by
            // signature), so independence is unproven — keep both sources
            // intact and retry through the established 保留原資料待重試 path
            // instead of appending a phantom row. Genuinely independent calls
            // differ in input/output or time and still append below.
            [] if stolen.is_empty() => {
                if twins.contains(&(
                    call.model.clone(),
                    call.agent_id.clone(),
                    call.timestamp,
                    call.metrics.input,
                    call.metrics.output,
                )) {
                    return Err("Copilot 事件檔與 SQLite 用量無法唯一配對，保留原資料待重試".into());
                }
                transcript.push(call)
            }
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
    fn absent_then_present_reasoning_effort_fills_retained_call() {
        // (thread PRRT_kwDOTOgsec6yUXNX) The retained duplicate may lack
        // metadata a later copy supplies. Event-only sessions have no SQLite
        // value to back-fill `reasoning_effort`, so the dedup must merge the
        // missing metadata into the retained call instead of discarding it.
        let mut first = event("a", "2026-07-22T10:00:00Z");
        first["id"] = json!("event-first");
        let mut second = first.clone();
        second["id"] = json!("event-second");
        second["data"]["usage"]["reasoningEffort"] = json!("high");
        let rows = read(&[first, second], "", false).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].reasoning_effort.as_deref(), Some("high"));
    }

    #[test]
    fn present_then_absent_reasoning_effort_keeps_first_value() {
        // Fill-only semantics: the first `Some` wins over later `None`/`Some`
        // copies and conflicting strict fields stay loud errors.
        let mut first = event("a", "2026-07-22T10:00:00Z");
        first["id"] = json!("event-first");
        first["data"]["usage"]["reasoningEffort"] = json!("high");
        let mut absent = first.clone();
        absent["id"] = json!("event-absent");
        absent["data"]["usage"]["reasoningEffort"] = Value::Null;
        let rows = read(&[first.clone(), absent], "", false).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].reasoning_effort.as_deref(), Some("high"));
        let mut low = first.clone();
        low["id"] = json!("event-low");
        low["data"]["usage"]["reasoningEffort"] = json!("low");
        let rows = read(&[first, low], "", false).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].reasoning_effort.as_deref(),
            Some("high"),
            "後到的值不得覆寫保留份的既有推理強度"
        );
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
    fn exact_hit_must_not_orphan_the_only_window_candidate_of_another_native_call() {
        // (review 4433863665) 事件檔 0ms/+750ms、SQLite −750ms/0ms、同 signature：
        // exact 拿走 e0 後，sqlite:1 唯一的一秒內候選已被消耗，不得再附加成幽靈
        // SQLite-only 列（同一呼叫計兩次、總量 +50%）；衝突時必須保留原資料待重試。
        let mut e0 = call("2026-07-22T10:00:00Z");
        e0.identity = "event:e0".into();
        let mut e1 = e0.clone();
        e1.identity = "event:e1".into();
        e1.timestamp = parse_timestamp("2026-07-22T10:00:00.750Z").unwrap();
        let mut n1 = e0.clone();
        n1.identity = "sqlite:1".into();
        n1.timestamp = parse_timestamp("2026-07-22T09:59:59.250Z").unwrap();
        n1.turn_no = None;
        let mut n2 = e0.clone();
        n2.identity = "sqlite:2".into();
        n2.turn_no = None;
        // 與 native 迭代順序無關，兩種排序都必須拒絕配對。
        for natives in [vec![n1.clone(), n2.clone()], vec![n2.clone(), n1.clone()]] {
            let result = merge_calls(vec![e0.clone(), e1.clone()], natives);
            assert!(
                result.is_err(),
                "exact 消耗唯一候選後不得附加獨立 SQLite-only 列，實得 {} 列",
                result.as_ref().map(Vec::len).unwrap_or(0)
            );
            assert!(result.unwrap_err().contains("保留原資料待重試"));
        }
        // 重試：來源資料可解析（SQLite 時間補正為各自 exact 對應）→ 1:1 合併、無雙計。
        n1.timestamp = e1.timestamp;
        let merged = merge_calls(vec![e0.clone(), e1.clone()], vec![n1, n2]).unwrap();
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].native_identity.as_deref(), Some("sqlite:2"));
        assert_eq!(merged[1].native_identity.as_deref(), Some("sqlite:1"));
        assert_eq!(merged.iter().map(|c| c.metrics.input).sum::<u64>(), 200);
    }

    #[test]
    fn stolen_window_candidate_does_not_block_distinct_safe_pairing() {
        // 同 review 4433863665 場景的變體：exact 仍消耗 e0，但 sqlite:1 另有互異的
        // 安全候選 e1（Δ950ms）→ 兩筆呼叫各有安全候選，必須照常 1:1 合併，不得過度衝突。
        let mut e0 = call("2026-07-22T10:00:00Z");
        e0.identity = "event:e0".into();
        let mut e1 = e0.clone();
        e1.identity = "event:e1".into();
        e1.timestamp = parse_timestamp("2026-07-22T10:00:00.200Z").unwrap();
        let mut n1 = e0.clone();
        n1.identity = "sqlite:1".into();
        n1.timestamp = parse_timestamp("2026-07-22T09:59:59.250Z").unwrap();
        n1.turn_no = None;
        let mut n2 = e0.clone();
        n2.identity = "sqlite:2".into();
        n2.turn_no = None;
        for natives in [vec![n1.clone(), n2.clone()], vec![n2.clone(), n1.clone()]] {
            let merged = merge_calls(vec![e0.clone(), e1.clone()], natives).unwrap();
            assert_eq!(merged.len(), 2);
            assert_eq!(merged[0].native_identity.as_deref(), Some("sqlite:2"));
            assert_eq!(merged[1].native_identity.as_deref(), Some("sqlite:1"));
            assert_eq!(merged.iter().map(|c| c.metrics.input).sum::<u64>(), 200);
        }
    }

    #[test]
    fn far_sqlite_only_call_still_appends_without_any_window_candidate() {
        // 真的沒有任何秒內候選（含被 exact 消耗者）的 SQLite 呼叫仍是獨立呼叫，
        // 必須照舊附加成 SQLite-only 列，衝突守衛不得一律拒絕。
        let a = call("2026-07-22T10:00:00Z");
        let mut far = a.clone();
        far.identity = "sqlite:far".into();
        far.timestamp = parse_timestamp("2026-07-22T10:00:02Z").unwrap();
        far.turn_no = None;
        let merged = merge_calls(vec![a.clone()], vec![far]).unwrap();
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].native_identity, None);
        assert_eq!(merged[1].identity, "sqlite:far");
    }

    #[test]
    fn absent_reasoning_tokens_and_nonzero_sqlite_reasoning_must_not_append_independent_row() {
        // (review 4433948817) 事件檔省略選填指標（解析折 0）而 SQLite 保留非零值：
        // 即使時間、模型、input/output 與其他指標全等，全指標 `Signature` 仍不同 →
        // exact 與 ±1 秒窗兩個閘門同時失效 → 不得落附加臂把同一呼叫存成獨立列
        // （整筆 input/output 雙計並存下幽靈 `:call:` 指紋）。同時間且
        // model/Agent/input/output 全等的指標不一致候選必須保守重試、保留原資料。
        for (absent, nonzero) in [("reasoningTokens", 300u64), ("cacheReadTokens", 80)] {
            let mut usage_event = event("a", "2026-07-22T10:00:00Z");
            usage_event["data"]["usage"]
                .as_object_mut()
                .unwrap()
                .remove(absent);
            let transcript = read(&[usage_event], "", false).unwrap();
            let mut native = transcript[0].clone();
            native.identity = "sqlite:1".into();
            native.turn_no = None;
            if absent == "reasoningTokens" {
                native.metrics.reasoning = nonzero;
            } else {
                native.metrics.cache_read = nonzero;
            }
            let result = merge_calls(transcript, vec![native]);
            assert!(
                result.is_err(),
                "省略 {absent} 對 SQLite 非零不得附加獨立列（{absent}={nonzero}）：實得 {} 列、input 總和 {}（真實 100）",
                result.as_ref().map(Vec::len).unwrap_or(0),
                result
                    .as_ref()
                    .map(|rows| rows.iter().map(|call| call.metrics.input).sum::<u64>())
                    .unwrap_or(0),
            );
            assert!(
                result.unwrap_err().contains("保留原資料待重試"),
                "缺欄-vs-非零的同時間候選必須走保守重試路線"
            );
        }
    }

    #[test]
    fn independent_same_time_calls_with_different_input_output_still_append() {
        // 防過度重試（同 review 4433948817 邊界）：真正獨立的 SQLite 呼叫照舊附加成
        // SQLite-only 列。保守重試只針對「同時間、同模型/Agent、input/output 全等但
        // 指標不一致」的缺欄-vs-非零形態；input/output 不同或無同時間孿生都不算。
        let a = call("2026-07-22T10:00:00Z");
        // 同時間但 input/output 不全等 → 判定為不同的呼叫 → 照常附加兩列。
        let mut same_time = a.clone();
        same_time.identity = "sqlite:same-time".into();
        same_time.turn_no = None;
        same_time.metrics.input = 200;
        same_time.metrics.output = 20;
        let merged = merge_calls(vec![a.clone()], vec![same_time]).unwrap();
        assert_eq!(
            merged.len(),
            2,
            "input/output 不同的同時間呼叫是真正獨立的呼叫"
        );
        assert_eq!(merged[0].native_identity, None);
        assert_eq!(merged[1].identity, "sqlite:same-time");
        // 指標不全等但沒有同時間孿生（距所有事件列 >1 秒）→ 照舊附加。
        let mut far = a.clone();
        far.identity = "sqlite:far".into();
        far.turn_no = None;
        far.metrics.reasoning = 300;
        far.timestamp = parse_timestamp("2026-07-22T10:00:02Z").unwrap();
        let merged = merge_calls(vec![a], vec![far]).unwrap();
        assert_eq!(merged.len(), 2, "無同時間孿生的指標不一致呼叫照舊附加");
        assert_eq!(merged[1].identity, "sqlite:far");
    }

    #[test]
    fn event_body_falls_back_to_data_when_payload_is_explicit_null() {
        // (review 4433863670) 明確 `payload: null` 視同缺欄退回 `data`；參數序即
        // 優先序，新格式 data 優先的呼叫端以 (data, payload) 反轉。
        let body = json!({"content":"hi"});
        let explicit_null = Value::Null;
        let payload = json!({"content":"from payload"});
        assert_eq!(event_body(Some(&explicit_null), Some(&body)), Some(&body));
        assert_eq!(event_body(None, Some(&body)), Some(&body));
        assert_eq!(event_body(Some(&payload), Some(&body)), Some(&payload));
        assert_eq!(event_body(Some(&body), Some(&explicit_null)), Some(&body));
        assert_eq!(event_body(Some(&body), Some(&payload)), Some(&body));
        assert_eq!(event_body(Some(&explicit_null), None), None);
    }

    #[test]
    fn bodyless_prompt_event_shapes_do_not_open_import_turns() {
        // (review 4433948800) 主體缺席的提示（明確 null、缺欄、兩側皆 null）在
        // 時間軸完全不可見、不計入回合序號；匯入端不得只憑事件類型把用量推到
        // 回合 2，否則回覆掛不到自己的用量（repro-r7 實測：匯入回合 2 vs 時間軸
        // 回合 1）。
        for prompt in [
            json!({"type":"user.message","payload":null}),
            json!({"type":"user.message"}),
            json!({"type":"user.message","payload":null,"data":null}),
        ] {
            let label = prompt.to_string();
            let rows = read(
                &[
                    prompt,
                    json!({"type":"assistant.message","data":{"content":"hi"}}),
                    event("u1", "2026-07-22T10:00:10Z"),
                ],
                "",
                false,
            )
            .unwrap();
            assert_eq!(
                rows[0].turn_no,
                Some(1),
                "無主體提示不得開回合，用量須留在回覆的回合 1: {label}"
            );
        }
    }

    #[test]
    fn body_prompt_counts_one_turn_per_user_message() {
        // 回歸守衛（防過度收斂）：有主體的提示各佔一個統計回合（每則使用者
        // 訊息分別累計），回覆不再推進回合。主體存在與內容無關（時間軸連空
        // 主體的 `"data":{}` 都記錄為提示），且明確 null 的 `payload` 退回
        // `data` 側的既有優先序不變（review 4433863670）。
        for prompt in [
            json!({"type":"user.message","payload":{"content":"q"}}),
            json!({"type":"user.message","data":{"content":"q"}}),
            json!({"type":"user.message","payload":null,"data":{"content":"q"}}),
            json!({"type":"user.message","data":{}}),
        ] {
            let label = prompt.to_string();
            let rows = read(
                &[
                    prompt.clone(),
                    json!({"type":"assistant.message","data":{"content":"hi"}}),
                    event("u1", "2026-07-22T10:00:10Z"),
                    prompt,
                    json!({"type":"assistant.message","data":{"content":"hi again"}}),
                    event("u2", "2026-07-22T10:01:10Z"),
                ],
                "",
                false,
            )
            .unwrap();
            assert_eq!(
                rows[0].turn_no,
                Some(1),
                "第一則有主體提示的回合是 1，回覆不推進回合: {label}"
            );
            assert_eq!(
                rows[1].turn_no,
                Some(2),
                "第二則有主體提示另開回合: {label}"
            );
        }
    }

    #[test]
    fn mid_message_replies_stay_in_the_message_turn_and_next_message_restarts() {
        // 回歸（PR #74 review 2026-11-01）：同一則使用者訊息的中途文字回覆與
        // final 同屬一個統計回合；下一則使用者訊息另開回合。舊推導只推進「提示
        // 後第一個回覆」，第二個回覆的呼叫會與下一則訊息的呼叫共用回合，兩條
        // final 都顯示合計，後到的用量還會回灌先到的回覆。
        let rows = read(
            &[
                json!({"type":"user.message","data":{"content":"m1"}}),
                event("c1", "2026-11-01T06:52:20Z"),
                json!({"type":"assistant.message","data":{"content":"mid","phase":"commentary"}}),
                event("c2", "2026-11-01T06:52:39Z"),
                json!({"type":"assistant.message","data":{"content":"final","phase":"final_answer"}}),
                json!({"type":"user.message","data":{"content":"m2"}}),
                event("c3", "2026-11-01T06:53:55Z"),
                json!({"type":"assistant.message","data":{"content":"final 2","phase":"final_answer"}}),
            ],
            "",
            false,
        )
        .unwrap();
        assert_eq!(
            rows.iter().map(|row| row.turn_no).collect::<Vec<_>>(),
            vec![Some(1), Some(1), Some(2)],
            "中途回覆與 final 同屬訊息 1 的回合，下一則訊息另開回合"
        );
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
