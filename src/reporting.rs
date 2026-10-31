use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{LazyLock, Mutex},
};

use serde::Serialize;

use crate::{
    db::{DatedUsageEntry, TokenStats, UsageEntry},
    pricing::PreparedPricingRules,
    session_identity::SessionIdentity,
};

#[derive(Debug, Clone, Default)]
pub(crate) struct UsageAggregation {
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cache_write_5m_tokens: u64,
    pub cache_write_1h_tokens: u64,
    pub reasoning_tokens: u64,
    pub cost_usd: f64,
}

impl UsageAggregation {
    fn add(&mut self, other: &Self) {
        self.total_tokens += other.total_tokens;
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_read_tokens += other.cache_read_tokens;
        self.cache_write_tokens += other.cache_write_tokens;
        self.cache_write_5m_tokens += other.cache_write_5m_tokens;
        self.cache_write_1h_tokens += other.cache_write_1h_tokens;
        self.reasoning_tokens += other.reasoning_tokens;
        self.cost_usd += other.cost_usd;
    }

    fn saturating_difference(&self, previous: &Self) -> Self {
        Self {
            total_tokens: self.total_tokens.saturating_sub(previous.total_tokens),
            input_tokens: self.input_tokens.saturating_sub(previous.input_tokens),
            output_tokens: self.output_tokens.saturating_sub(previous.output_tokens),
            cache_read_tokens: self
                .cache_read_tokens
                .saturating_sub(previous.cache_read_tokens),
            cache_write_tokens: self
                .cache_write_tokens
                .saturating_sub(previous.cache_write_tokens),
            cache_write_5m_tokens: self
                .cache_write_5m_tokens
                .saturating_sub(previous.cache_write_5m_tokens),
            cache_write_1h_tokens: self
                .cache_write_1h_tokens
                .saturating_sub(previous.cache_write_1h_tokens),
            reasoning_tokens: self
                .reasoning_tokens
                .saturating_sub(previous.reasoning_tokens),
            cost_usd: (self.cost_usd - previous.cost_usd).max(0.0),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ModelUsageAggregation {
    pub model: String,
    pub usage: UsageAggregation,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SessionUsageAggregation {
    pub usage: UsageAggregation,
    pub models: Vec<ModelUsageAggregation>,
    pub display_model: String,
}

#[derive(Serialize, Default, Clone)]
pub struct DaySummary {
    pub total_sessions: usize,
    pub total_tokens: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_read_tokens: u64,
    pub total_cache_write_tokens: u64,
    pub total_reasoning_tokens: u64,
    pub total_duration_ms: u64,
    pub total_requests: u64,
    pub total_cost_usd: f64,
}

impl DaySummary {
    pub(crate) fn add_usage(&mut self, usage: &UsageAggregation) {
        self.total_tokens += usage.total_tokens;
        self.total_input_tokens += usage.input_tokens;
        self.total_output_tokens += usage.output_tokens;
        self.total_cache_read_tokens += usage.cache_read_tokens;
        self.total_cache_write_tokens += usage.cache_write_tokens;
        self.total_reasoning_tokens += usage.reasoning_tokens;
        self.total_cost_usd += usage.cost_usd;
    }
}

#[derive(Serialize)]
pub struct MonthlyProjectSummary {
    pub cwd: String,
    pub sessions_count: usize,
    pub total_tokens: u64,
    pub cost_usd: f64,
}

#[derive(Serialize)]
pub struct MonthlyModelSummary {
    pub model: String,
    pub mode: Option<String>,
    pub sessions_count: usize,
    pub total_tokens: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_read_tokens: u64,
    pub cost_usd: f64,
}

#[derive(Serialize, Default, Clone)]
pub struct AgentBreakdown {
    pub total_tokens: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_read_tokens: u64,
    pub total_reasoning_tokens: u64,
    pub total_cost_usd: f64,
    pub total_sessions: usize,
}

/// 單一時間區段（日或月）內某個 Agent 的用量，供合併全部 Agent 的堆疊趨勢圖使用。
#[derive(Serialize, Debug, Default, Clone, PartialEq)]
pub struct AgentPeriodUsage {
    pub total_tokens: u64,
    pub cost_usd: f64,
    pub sessions_count: usize,
}

fn has_usage(tokens: &TokenStats) -> bool {
    tokens.total > 0
        || tokens.input > 0
        || tokens.output > 0
        || tokens.cache_read.unwrap_or(0) > 0
        || tokens.cache_write.unwrap_or(0) > 0
        || tokens.reasoning.unwrap_or(0) > 0
}

fn add_tokens(aggregation: &mut UsageAggregation, tokens: &TokenStats) {
    aggregation.total_tokens += tokens.total;
    aggregation.input_tokens += tokens.input;
    aggregation.output_tokens += tokens.output;
    aggregation.cache_read_tokens += tokens.cache_read.unwrap_or(0);
    aggregation.cache_write_tokens += tokens.cache_write.unwrap_or(0);
    let (cache_write_5m, cache_write_1h) = cache_write_breakdown(tokens);
    aggregation.cache_write_5m_tokens += cache_write_5m;
    aggregation.cache_write_1h_tokens += cache_write_1h;
    aggregation.reasoning_tokens += tokens.reasoning.unwrap_or(0);
}

fn cache_write_breakdown(tokens: &TokenStats) -> (u64, u64) {
    (
        tokens.cache_write_5m.unwrap_or(0),
        tokens.cache_write_1h.unwrap_or(0),
    )
}

fn entry_model(entry: &UsageEntry) -> Option<&str> {
    entry
        .model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty())
}

pub(crate) fn latest_usage_entry<'a>(
    entries: impl IntoIterator<Item = &'a UsageEntry>,
) -> Option<&'a UsageEntry> {
    entries.into_iter().max_by(|left, right| {
        left.turn_no
            .cmp(&right.turn_no)
            .then_with(|| left.timestamp.cmp(&right.timestamp))
    })
}

/// 逐次呼叫列（Copilot App / CLI agents 的逐呼叫對帳列）的
/// `import_source_id` 一定帶 `:call:` 標記（見 `db::replace_copilot_call_rows`
/// 寫入的 `…:call:{call_identity}`）。這類列的 `duration_ms` 只是單次呼叫
/// 耗時；舊聚合列、Codex session 累積值等其他來源每列已帶整回合／整個
/// session 的累積耗時。
const PER_CALL_IMPORT_SOURCE_MARKER: &str = ":call:";

fn entry_duration_ms(entry: &UsageEntry) -> f64 {
    entry
        .cost
        .as_ref()
        .and_then(|cost| cost.total_api_duration_ms)
        .unwrap_or(0.0)
}

/// 計算單一 session 分組的顯示耗時（毫秒）。
///
/// 含 `:call:` 標記的逐次呼叫列每列只有單次呼叫耗時，同回合多次呼叫必須
/// 全部加總才等於逐次呼叫接管前聚合列顯示的回合總耗時（review comment
/// 4436837582）；沒有逐次呼叫列的分組（舊聚合列、Codex 每列皆為 session
/// 累積值等）維持「取最新一筆」的累積值語意。混和分組（逐次呼叫列與
/// 匯入聚合列並存）以逐次呼叫列的加總為準。
pub(crate) fn session_duration_ms<'a>(
    entries: impl IntoIterator<Item = (&'a UsageEntry, Option<&'a str>)>,
) -> u64 {
    let entries: Vec<(&UsageEntry, Option<&str>)> = entries.into_iter().collect();
    let mut per_call_rows = 0usize;
    let mut per_call_duration = 0.0;
    for (entry, import_source_id) in &entries {
        if import_source_id
            .as_deref()
            .is_some_and(|id| id.contains(PER_CALL_IMPORT_SOURCE_MARKER))
        {
            per_call_rows += 1;
            per_call_duration += entry_duration_ms(entry);
        }
    }
    if per_call_rows > 0 {
        return per_call_duration as u64;
    }
    latest_usage_entry(entries.iter().map(|(entry, _)| *entry))
        .map(entry_duration_ms)
        .unwrap_or(0.0) as u64
}

/// 依 session 身分（`SessionIdentity`，含 `parent_session_id`）預先彙總各
/// 分組的顯示耗時。判別逐次呼叫列所需的 `import_source_id` 不屬於
/// `UsageEntry`，由呼叫端從 `UsageDayExportRecord`／`DatedUsageEntry` 一併
/// 提供，避免顯示端再回頭全表掃描。
pub(crate) fn session_durations_by_identity<'a>(
    rows: impl IntoIterator<Item = (&'a UsageEntry, &'a str, Option<&'a str>)>,
) -> HashMap<SessionIdentity, u64> {
    let mut grouped: HashMap<SessionIdentity, Vec<(&UsageEntry, Option<&str>)>> = HashMap::new();
    for (entry, assistant_type, import_source_id) in rows {
        grouped
            .entry(SessionIdentity::from_entry(assistant_type, entry))
            .or_default()
            .push((entry, import_source_id));
    }
    grouped
        .into_iter()
        .map(|(identity, group)| (identity, session_duration_ms(group)))
        .collect()
}

static WARNED_PRICING_MODELS: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

fn log_pricing_failure(model: &str, session_id: &str, turn_no: u32, error: &str) {
    // 此模型缺少價格規則時保留用量統計，僅略過錯誤提示。
    if model.eq_ignore_ascii_case("copilot-search-b") {
        return;
    }

    let mut warned = match WARNED_PRICING_MODELS.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if warned.insert(model.to_string()) {
        eprintln!(
            "計算成本失敗: session_id={} turn_no={} model={}: {}（此模型後續不再重複提示）",
            session_id, turn_no, model, error
        );
    }
}

fn record_usage(
    result: &mut SessionUsageAggregation,
    pricing_rules: &PreparedPricingRules,
    entry: &UsageEntry,
    tokens: &TokenStats,
) {
    let model = entry_model(entry);
    let model_label = model.unwrap_or("Unknown Model");
    let (cache_write_5m, cache_write_1h) = cache_write_breakdown(tokens);
    let cost_usd =
        if let Some(reported_cost) = entry.cost.as_ref().and_then(|cost| cost.reported_cost_usd) {
            reported_cost
        } else {
            match pricing_rules.calculate_usage_cost(
                model,
                tokens.input,
                tokens.output,
                tokens.cache_read.unwrap_or(0),
                cache_write_5m,
                cache_write_1h,
            ) {
                Ok(cost) => cost,
                Err(error) => {
                    log_pricing_failure(model_label, &entry.session_id, entry.turn_no, &error);
                    0.0
                }
            }
        };

    add_tokens(&mut result.usage, tokens);
    result.usage.cost_usd += cost_usd;

    if let Some(model_usage) = result
        .models
        .iter_mut()
        .find(|item| item.model == model_label)
    {
        add_tokens(&mut model_usage.usage, tokens);
        model_usage.usage.cost_usd += cost_usd;
    } else {
        let mut usage = UsageAggregation::default();
        add_tokens(&mut usage, tokens);
        usage.cost_usd += cost_usd;
        result.models.push(ModelUsageAggregation {
            model: model_label.to_string(),
            usage,
        });
    }
}

pub(crate) fn summarize_session_usage(
    pricing_rules: &PreparedPricingRules,
    entries: &[UsageEntry],
) -> SessionUsageAggregation {
    let mut result = SessionUsageAggregation::default();
    let mut display_entry: Option<&UsageEntry> = None;

    if let Some(delta_result) = summarize_delta_entries(pricing_rules, entries) {
        result = delta_result;
        display_entry = latest_usage_entry(
            entries
                .iter()
                .filter(|entry| entry.delta_tokens.as_ref().is_some_and(has_usage)),
        );
    } else if let Some((entry, tokens)) = latest_usage_entry(
        entries
            .iter()
            .filter(|entry| entry.tokens.as_ref().is_some_and(has_usage)),
    )
    .and_then(|entry| entry.tokens.as_ref().map(|tokens| (entry, tokens)))
    {
        record_usage(&mut result, pricing_rules, entry, tokens);
        display_entry = Some(entry);
    }

    result.display_model = display_entry
        .and_then(entry_model)
        .unwrap_or("Unknown Model")
        .to_string();
    result
}

fn summarize_delta_entries(
    pricing_rules: &PreparedPricingRules,
    entries: &[UsageEntry],
) -> Option<SessionUsageAggregation> {
    let mut result = SessionUsageAggregation::default();
    let mut recorded_usage = false;
    for entry in entries {
        let Some(tokens) = entry
            .delta_tokens
            .as_ref()
            .filter(|tokens| has_usage(tokens))
        else {
            continue;
        };
        record_usage(&mut result, pricing_rules, entry, tokens);
        recorded_usage = true;
    }
    recorded_usage.then_some(result)
}

fn summarize_delta_usage(
    pricing_rules: &PreparedPricingRules,
    entries: &[UsageEntry],
) -> UsageAggregation {
    summarize_delta_entries(pricing_rules, entries)
        .map(|result| result.usage)
        .unwrap_or_default()
}

#[derive(Debug)]
pub(crate) struct SessionGroup {
    pub entries: Vec<UsageEntry>,
}

pub(crate) type SessionMap = HashMap<SessionIdentity, SessionGroup>;

pub(crate) fn group_sessions<'a>(
    entries: impl IntoIterator<Item = (&'a UsageEntry, &'a str)>,
) -> SessionMap {
    let mut sessions = HashMap::new();
    for (entry, assistant_type) in entries {
        sessions
            .entry(SessionIdentity::from_entry(assistant_type, entry))
            .or_insert_with(|| SessionGroup {
                entries: Vec::new(),
            })
            .entries
            .push(entry.clone());
    }
    sessions
}

pub(crate) fn cursor_session_mode(
    assistant_type: &str,
    entries: &[UsageEntry],
) -> Option<&'static str> {
    if assistant_type != "cursor" {
        return None;
    }

    latest_usage_entry(entries).and_then(|entry| match entry.source_kind.as_deref() {
        Some("cursor-agent") => Some("agent"),
        Some("cursor-ide") => Some("ide"),
        _ => None,
    })
}

pub(crate) fn summarize_models_by_mode(
    sessions: &SessionMap,
    pricing_rules: &PreparedPricingRules,
) -> Vec<MonthlyModelSummary> {
    #[derive(Default)]
    struct ModelStats {
        sessions_count: usize,
        total_tokens: u64,
        total_input_tokens: u64,
        total_output_tokens: u64,
        total_cache_read_tokens: u64,
        cost_usd: f64,
    }

    let mut stats: HashMap<(String, Option<String>), ModelStats> = HashMap::new();
    for (identity, group) in sessions {
        let session_usage = summarize_session_usage(pricing_rules, &group.entries);
        let mode =
            cursor_session_mode(&identity.assistant_type, &group.entries).map(str::to_string);
        for model_usage in session_usage.models {
            let model_stat = stats.entry((model_usage.model, mode.clone())).or_default();
            model_stat.sessions_count += 1;
            model_stat.total_tokens += model_usage.usage.total_tokens;
            model_stat.total_input_tokens += model_usage.usage.input_tokens;
            model_stat.total_output_tokens += model_usage.usage.output_tokens;
            model_stat.total_cache_read_tokens += model_usage.usage.cache_read_tokens;
            model_stat.cost_usd += model_usage.usage.cost_usd;
        }
    }

    let mut summaries = stats
        .into_iter()
        .map(|((model, mode), stats)| MonthlyModelSummary {
            model,
            mode,
            sessions_count: stats.sessions_count,
            total_tokens: stats.total_tokens,
            total_input_tokens: stats.total_input_tokens,
            total_output_tokens: stats.total_output_tokens,
            total_cache_read_tokens: stats.total_cache_read_tokens,
            cost_usd: stats.cost_usd,
        })
        .collect::<Vec<_>>();
    summaries.sort_by_key(|item| std::cmp::Reverse(item.total_tokens));
    summaries
}

#[derive(Debug)]
pub(crate) struct PeriodBreakdown {
    pub label: String,
    pub usage: UsageAggregation,
    pub sessions_count: usize,
    /// 依 Agent 拆分的區段用量；以 BTreeMap 保持序列化順序穩定。
    pub agents: BTreeMap<String, AgentPeriodUsage>,
}

pub(crate) struct PeriodReport {
    pub summary: DaySummary,
    pub breakdown: Vec<PeriodBreakdown>,
    pub projects: Vec<MonthlyProjectSummary>,
    pub models: Vec<MonthlyModelSummary>,
    pub agent_breakdown: HashMap<String, AgentBreakdown>,
}

#[derive(Default)]
struct PeriodBucketAggregation {
    usage: UsageAggregation,
    sessions_count: usize,
    agents: BTreeMap<String, AgentPeriodUsage>,
}

#[derive(Default)]
struct ProjectUsageAggregation {
    sessions_count: usize,
    total_tokens: u64,
    cost_usd: f64,
}

fn summarize_groups(
    sessions: &SessionMap,
    pricing_rules: &PreparedPricingRules,
) -> UsageAggregation {
    let mut usage = UsageAggregation::default();
    for group in sessions.values() {
        let session = summarize_session_usage(pricing_rules, &group.entries);
        usage.add(&session.usage);
    }
    usage
}

fn build_period_breakdown(
    entries: &[DatedUsageEntry],
    bucket_label: &impl Fn(&str) -> String,
    pricing_rules: &PreparedPricingRules,
) -> Vec<PeriodBreakdown> {
    let mut dated_sessions: HashMap<SessionIdentity, Vec<&DatedUsageEntry>> = HashMap::new();
    for record in entries {
        dated_sessions
            .entry(SessionIdentity::from_entry(
                &record.assistant_type,
                &record.entry,
            ))
            .or_default()
            .push(record);
    }

    let mut bucket_totals: HashMap<String, PeriodBucketAggregation> = HashMap::new();
    for (identity, records) in &dated_sessions {
        // Delta-based collectors can be summed inside each bucket. Legacy
        // cumulative collectors must instead contribute the increase from the
        // preceding bucket; otherwise a session spanning two buckets would
        // count both cumulative snapshots in the period total.
        let has_delta_usage = records
            .iter()
            .filter_map(|record| record.entry.delta_tokens.as_ref())
            .any(has_usage);
        let mut session_buckets: HashMap<String, Vec<UsageEntry>> = HashMap::new();
        for record in records {
            session_buckets
                .entry(bucket_label(&record.date))
                .or_default()
                .push(record.entry.clone());
        }

        let mut labels = session_buckets.keys().cloned().collect::<Vec<_>>();
        labels.sort();
        let mut previous_cumulative = UsageAggregation::default();
        for label in labels {
            let cumulative_or_delta = if has_delta_usage {
                summarize_delta_usage(pricing_rules, &session_buckets[&label])
            } else {
                summarize_session_usage(pricing_rules, &session_buckets[&label]).usage
            };
            let bucket_usage = if has_delta_usage {
                cumulative_or_delta
            } else {
                let increment = cumulative_or_delta.saturating_difference(&previous_cumulative);
                previous_cumulative = cumulative_or_delta;
                increment
            };
            let bucket = bucket_totals.entry(label).or_default();
            bucket.usage.add(&bucket_usage);
            bucket.sessions_count += 1;
            let agent = bucket
                .agents
                .entry(identity.assistant_type.clone())
                .or_default();
            agent.total_tokens += bucket_usage.total_tokens;
            agent.cost_usd += bucket_usage.cost_usd;
            agent.sessions_count += 1;
        }
    }

    let mut breakdown = bucket_totals
        .into_iter()
        .map(|(label, bucket)| PeriodBreakdown {
            label,
            usage: bucket.usage,
            sessions_count: bucket.sessions_count,
            agents: bucket.agents,
        })
        .collect::<Vec<_>>();
    breakdown.sort_by(|left, right| left.label.cmp(&right.label));
    breakdown
}

pub(crate) fn build_period_report(
    entries: &[DatedUsageEntry],
    bucket_label: impl Fn(&str) -> String,
    pricing_rules: &PreparedPricingRules,
) -> PeriodReport {
    let sessions = group_sessions(
        entries
            .iter()
            .map(|record| (&record.entry, record.assistant_type.as_str())),
    );
    let mut summary = DaySummary {
        total_sessions: sessions.len(),
        ..Default::default()
    };
    summary.add_usage(&summarize_groups(&sessions, pricing_rules));

    let breakdown = build_period_breakdown(entries, &bucket_label, pricing_rules);

    let mut project_stats: HashMap<String, ProjectUsageAggregation> = HashMap::new();
    let mut agent_breakdown: HashMap<String, AgentBreakdown> = HashMap::new();
    for (identity, group) in &sessions {
        let Some(latest) = latest_usage_entry(&group.entries) else {
            continue;
        };
        let session = summarize_session_usage(pricing_rules, &group.entries);
        let cwd = latest
            .cwd
            .clone()
            .unwrap_or_else(|| "Unknown CWD".to_string());
        let project = project_stats.entry(cwd).or_default();
        project.sessions_count += 1;
        project.total_tokens += session.usage.total_tokens;
        project.cost_usd += session.usage.cost_usd;

        let agent = agent_breakdown
            .entry(identity.assistant_type.clone())
            .or_default();
        agent.total_tokens += session.usage.total_tokens;
        agent.total_input_tokens += session.usage.input_tokens;
        agent.total_output_tokens += session.usage.output_tokens;
        agent.total_cache_read_tokens += session.usage.cache_read_tokens;
        agent.total_reasoning_tokens += session.usage.reasoning_tokens;
        agent.total_cost_usd += session.usage.cost_usd;
        agent.total_sessions += 1;
    }

    let mut projects = project_stats
        .into_iter()
        .map(|(cwd, stats)| MonthlyProjectSummary {
            cwd,
            sessions_count: stats.sessions_count,
            total_tokens: stats.total_tokens,
            cost_usd: stats.cost_usd,
        })
        .collect::<Vec<_>>();
    projects.sort_by_key(|item| std::cmp::Reverse(item.total_tokens));
    let models = summarize_models_by_mode(&sessions, pricing_rules);

    PeriodReport {
        summary,
        breakdown,
        projects,
        models,
        agent_breakdown,
    }
}

/// 測試專用：在乾淨的 `INSIGHTS_DIR` 子行程中執行 handler 測試主體。
///
/// `INSIGHTS_DIR` 是行程全域環境變數；直接在測試行程內 `set_var` 會與
/// handlers/mod.rs 既有 INSIGHTS_DIR 測試（持有自己的模組鎖，無法跨模組
/// 共用）互相覆寫造成 flaky。這裡把同一支測試以 `--exact` 重跑於子行程，
/// 並在建立子行程時就注入全新的 `INSIGHTS_DIR`，讓 fixture 與其他測試完全
/// 隔離；子行程內不需也不可再改寫環境變數。
#[cfg(test)]
pub(crate) fn spawn_isolated_insights_test(
    test_name: &str,
    temp_dir: &std::path::Path,
) -> std::process::Output {
    std::fs::create_dir_all(temp_dir).unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([test_name, "--exact"])
        .env("TUI_INSIGHTS_TEST_CHILD", "1")
        .env("INSIGHTS_DIR", temp_dir)
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(temp_dir);
    output
}

/// 測試專用：判斷目前是否為 `spawn_isolated_insights_test` 建立的子行程。
#[cfg(test)]
pub(crate) fn is_isolated_insights_test_child() -> bool {
    std::env::var_os("TUI_INSIGHTS_TEST_CHILD").is_some()
}

/// 測試專用：同步測試呼叫非同步 handler 所需的單執行緒 runtime。
#[cfg(test)]
pub(crate) fn handler_test_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        db::{CostStats, TokenStats},
        pricing::PricingRule,
    };

    fn token_stats(input: u64, output: u64, cache_read: u64) -> TokenStats {
        TokenStats {
            input,
            output,
            cache_read: Some(cache_read),
            cache_write: Some(0),
            cache_write_5m: None,
            cache_write_1h: None,
            reasoning: None,
            total: input + output + cache_read,
        }
    }

    fn summary_entry(turn_no: u32, model: &str, tokens: TokenStats, has_delta: bool) -> UsageEntry {
        UsageEntry {
            timestamp: format!("2026-07-10T10:{turn_no:02}:00Z"),
            session_id: "mixed-model-session".to_string(),
            session_name: None,
            transcript_path: None,
            cwd: None,
            version: None,
            turn_no,
            model: Some(model.to_string()),
            model_id: Some(model.to_string()),
            tokens: Some(tokens.clone()),
            delta_tokens: has_delta.then_some(tokens),
            context: None,
            cost: None,
            source_kind: None,
            source_dir_key: None,
            parent_session_id: None,
            agent_nickname: None,
            agent_role: None,
            reasoning_effort: None,
        }
    }

    fn usage_entry(source_dir_key: &str, model: &str, total_tokens: u64) -> UsageEntry {
        let tokens = TokenStats {
            input: total_tokens,
            output: 0,
            cache_read: Some(0),
            cache_write: Some(0),
            cache_write_5m: None,
            cache_write_1h: None,
            reasoning: None,
            total: total_tokens,
        };
        UsageEntry {
            timestamp: "2026-07-10T10:00:00Z".to_string(),
            session_id: "shared".to_string(),
            session_name: Some("Shared".to_string()),
            transcript_path: None,
            cwd: Some(format!("/workspace/{source_dir_key}")),
            version: None,
            turn_no: 1,
            model: Some(model.to_string()),
            model_id: Some(model.to_string()),
            tokens: Some(tokens.clone()),
            delta_tokens: Some(tokens),
            context: None,
            cost: None,
            source_kind: Some("copilot-app".to_string()),
            source_dir_key: Some(source_dir_key.to_string()),
            parent_session_id: None,
            agent_nickname: None,
            agent_role: None,
            reasoning_effort: None,
        }
    }

    fn duration_entry(turn_no: u32, timestamp: &str, duration_ms: f64) -> UsageEntry {
        UsageEntry {
            timestamp: timestamp.to_string(),
            session_id: "duration-session".to_string(),
            session_name: None,
            transcript_path: None,
            cwd: None,
            version: None,
            turn_no,
            model: Some("gpt-5".to_string()),
            model_id: Some("gpt-5".to_string()),
            tokens: None,
            delta_tokens: None,
            context: None,
            cost: Some(CostStats {
                total_api_duration_ms: Some(duration_ms),
                total_duration_ms: None,
                total_premium_requests: None,
                reported_cost_usd: None,
            }),
            source_kind: Some("copilot-app".to_string()),
            source_dir_key: Some("aa".to_string()),
            parent_session_id: None,
            agent_nickname: None,
            agent_role: None,
            reasoning_effort: None,
        }
    }

    #[test]
    fn session_duration_ms_sums_per_call_rows() {
        // 同回合兩次呼叫各 100ms（`:call:` 列）→ 顯示 200ms（latest-only 會
        // 錯誤顯示 100ms，review comment 4436837582）。
        let early = duration_entry(1, "2026-07-10T10:00:00Z", 100.0);
        let late = duration_entry(1, "2026-07-10T10:01:00Z", 100.0);

        assert_eq!(
            session_duration_ms([
                (&early, Some("copilot-app:aa:duration-session:call:h1")),
                (&late, Some("copilot-app:aa:duration-session:call:h2")),
            ]),
            200
        );
    }

    #[test]
    fn session_duration_ms_keeps_cumulative_rows_latest_only() {
        // Codex 類來源每列都帶整個 session 的累積耗時 3500ms，取最新一筆即
        // 可，絕不可加總成 10500ms。
        let first = duration_entry(1, "2026-07-10T10:00:00Z", 3500.0);
        let second = duration_entry(2, "2026-07-10T10:05:00Z", 3500.0);
        let third = duration_entry(3, "2026-07-10T10:10:00Z", 3500.0);

        assert_eq!(
            session_duration_ms([
                (&first, Some("codex:legacy:duration-session:1")),
                (&second, Some("codex:legacy:duration-session:2")),
                (&third, None),
            ]),
            3500
        );
    }

    #[test]
    fn session_duration_ms_mixed_group_prefers_per_call_sum() {
        // 混和分組（逐次呼叫列 + 匯入聚合列並存）固定以逐次呼叫列加總為準。
        let call = duration_entry(1, "2026-07-10T10:00:00Z", 100.0);
        let aggregate = duration_entry(1, "2026-07-10T10:05:00Z", 3500.0);

        assert_eq!(
            session_duration_ms([
                (
                    &call,
                    Some("copilot-cli-agents:bb:duration-session:call:h9")
                ),
                (&aggregate, Some("copilot-app:bb:duration-session:1")),
            ]),
            100
        );
    }

    #[test]
    fn session_durations_by_identity_separates_identities_and_sums_calls() {
        // 同名 session 但 parent_session_id 不同（獨立 session vs 合成子
        // Agent）必須分開加總；逐次呼叫組加總各次呼叫、累積組取最新一筆。
        let mut child_call_a = duration_entry(1, "2026-07-10T10:00:00Z", 100.0);
        child_call_a.session_id = "a__b".to_string();
        child_call_a.parent_session_id = Some("a".to_string());
        let mut child_call_b = duration_entry(1, "2026-07-10T10:01:00Z", 150.0);
        child_call_b.session_id = "a__b".to_string();
        child_call_b.parent_session_id = Some("a".to_string());
        let mut standalone = duration_entry(5, "2026-07-10T11:00:00Z", 3500.0);
        standalone.session_id = "a__b".to_string();

        let durations = session_durations_by_identity([
            (
                &child_call_a,
                "copilot",
                Some("copilot-app:aa:a__b:call:h1"),
            ),
            (
                &child_call_b,
                "copilot",
                Some("copilot-app:aa:a__b:call:h2"),
            ),
            (&standalone, "copilot", Some("copilot-app:aa:a__b:5")),
        ]);

        assert_eq!(durations.len(), 2);
        let child_identity = SessionIdentity::from_entry("copilot", &child_call_a);
        let standalone_identity = SessionIdentity::from_entry("copilot", &standalone);
        assert_eq!(durations.get(&child_identity).copied(), Some(250));
        assert_eq!(durations.get(&standalone_identity).copied(), Some(3500));
    }

    #[test]
    fn latest_usage_entry_breaks_equal_turns_by_timestamp() {
        let mut earlier = usage_entry("aa", "earlier", 100);
        earlier.turn_no = 7;
        earlier.timestamp = "2026-07-10T10:00:00Z".to_string();
        let mut later = usage_entry("aa", "later", 100);
        later.turn_no = 7;
        later.timestamp = "2026-07-10T10:01:00Z".to_string();

        let latest = latest_usage_entry([&later, &earlier]);

        assert_eq!(latest.and_then(entry_model), Some("later"));
    }

    #[test]
    fn period_report_keeps_same_id_source_directories_separate() {
        let entries = vec![
            DatedUsageEntry {
                entry: usage_entry("aa", "gpt-5", 100),
                assistant_type: "copilot".to_string(),
                date: "2026-07-10".to_string(),
                import_source_id: None,
            },
            DatedUsageEntry {
                entry: usage_entry("aa", "claude-sonnet-4", 200),
                assistant_type: "copilot".to_string(),
                date: "2026-07-10".to_string(),
                import_source_id: None,
            },
            DatedUsageEntry {
                entry: usage_entry("bb", "gpt-5", 300),
                assistant_type: "copilot".to_string(),
                date: "2026-07-10".to_string(),
                import_source_id: None,
            },
        ];

        let report = build_period_report(
            &entries,
            str::to_string,
            &PreparedPricingRules::from_rules(Vec::new()),
        );

        assert_eq!(report.summary.total_sessions, 2);
        assert_eq!(report.breakdown[0].sessions_count, 2);
        let gpt_summary = report
            .models
            .iter()
            .find(|summary| summary.model == "gpt-5")
            .unwrap();
        assert_eq!(gpt_summary.sessions_count, 2);
        assert_eq!(gpt_summary.total_tokens, 400);
    }

    #[test]
    fn period_report_splits_buckets_by_agent_for_all_agents_scope() {
        let dated = |assistant: &str, date: &str, tokens: u64| {
            let mut entry = usage_entry("aa", "gpt-5", tokens);
            entry.timestamp = format!("{date}T10:00:00Z");
            DatedUsageEntry {
                entry,
                assistant_type: assistant.to_string(),
                date: date.to_string(),
                import_source_id: None,
            }
        };
        // 兩個 Agent 使用相同 session_id，合併模式仍必須視為不同 Session。
        let mut later_claude = dated("claude", "2026-07-11", 2_000_000);
        later_claude.entry.session_id = "claude-second".to_string();
        let entries = vec![
            dated("codex", "2026-07-10", 3_000_000),
            dated("claude", "2026-07-10", 1_000_000),
            later_claude,
        ];

        let report = build_period_report(
            &entries,
            str::to_string,
            &PreparedPricingRules::from_rules(vec![PricingRule {
                model_name: "gpt-5".to_string(),
                input_price: 1.0,
                cache_input_price: 0.0,
                output_price: 0.0,
            }]),
        );

        assert_eq!(report.summary.total_sessions, 3);
        assert_eq!(report.summary.total_tokens, 6_000_000);
        assert_eq!(report.breakdown.len(), 2);

        let first_day = &report.breakdown[0];
        assert_eq!(first_day.label, "2026-07-10");
        assert_eq!(
            first_day.agents.keys().collect::<Vec<_>>(),
            vec!["claude", "codex"]
        );
        assert_eq!(
            first_day.agents["codex"],
            AgentPeriodUsage {
                total_tokens: 3_000_000,
                cost_usd: 3.0,
                sessions_count: 1,
            }
        );
        assert_eq!(first_day.agents["claude"].total_tokens, 1_000_000);
        assert_eq!(
            first_day
                .agents
                .values()
                .map(|agent| agent.total_tokens)
                .sum::<u64>(),
            first_day.usage.total_tokens
        );

        let second_day = &report.breakdown[1];
        assert_eq!(second_day.agents.len(), 1);
        assert_eq!(second_day.agents["claude"].sessions_count, 1);

        assert_eq!(report.agent_breakdown["codex"].total_tokens, 3_000_000);
        assert_eq!(report.agent_breakdown["claude"].total_tokens, 3_000_000);
        assert_eq!(report.agent_breakdown["claude"].total_sessions, 2);
        let agent_cost_sum = report
            .agent_breakdown
            .values()
            .map(|agent| agent.total_cost_usd)
            .sum::<f64>();
        assert!((agent_cost_sum - report.summary.total_cost_usd).abs() < 1e-9);
        assert!((report.summary.total_cost_usd - 6.0).abs() < 1e-9);
    }

    #[test]
    fn period_report_counts_cross_bucket_cumulative_usage_once() {
        let mut first = usage_entry("aa", "gpt-5", 100);
        first.delta_tokens = None;
        first.turn_no = 1;
        let mut second = usage_entry("aa", "gpt-5", 200);
        second.delta_tokens = None;
        second.turn_no = 2;
        second.timestamp = "2026-08-01T10:00:00Z".to_string();
        let entries = vec![
            DatedUsageEntry {
                entry: first,
                assistant_type: "copilot".to_string(),
                date: "2026-07-31".to_string(),
                import_source_id: None,
            },
            DatedUsageEntry {
                entry: second,
                assistant_type: "copilot".to_string(),
                date: "2026-08-01".to_string(),
                import_source_id: None,
            },
        ];

        let report = build_period_report(
            &entries,
            |date| date[..7].to_string(),
            &PreparedPricingRules::from_rules(vec![PricingRule {
                model_name: "gpt-5".to_string(),
                input_price: 0.0,
                cache_input_price: 0.0,
                output_price: 0.0,
            }]),
        );

        assert_eq!(report.summary.total_tokens, 200);
        assert_eq!(report.breakdown.len(), 2);
        assert_eq!(report.breakdown[0].usage.total_tokens, 100);
        assert_eq!(report.breakdown[1].usage.total_tokens, 100);
        assert_eq!(
            report
                .breakdown
                .iter()
                .map(|bucket| bucket.usage.total_tokens)
                .sum::<u64>(),
            report.summary.total_tokens
        );
    }

    #[test]
    fn session_cost_uses_each_delta_model_and_ignores_synthetic_tail() {
        let rules = [
            PricingRule {
                model_name: "claude-opus-4-8".to_string(),
                input_price: 10.0,
                cache_input_price: 0.5,
                output_price: 50.0,
            },
            PricingRule {
                model_name: "claude-fable-5".to_string(),
                input_price: 2.0,
                cache_input_price: 0.2,
                output_price: 4.0,
            },
        ];
        let entries = vec![
            summary_entry(
                1,
                "claude-opus-4-8",
                token_stats(100_000, 10_000, 200_000),
                true,
            ),
            summary_entry(
                2,
                "claude-fable-5",
                token_stats(50_000, 5_000, 100_000),
                true,
            ),
            summary_entry(3, "<synthetic>", token_stats(0, 0, 0), true),
        ];

        let result =
            summarize_session_usage(&PreparedPricingRules::from_rules(rules.into()), &entries);

        assert!((result.usage.cost_usd - 1.74).abs() < 1e-9);
        assert_eq!(result.usage.total_tokens, 465_000);
        assert_eq!(result.display_model, "claude-fable-5");
        assert_eq!(result.models.len(), 2);
        assert!(result
            .models
            .iter()
            .all(|usage| usage.model != "<synthetic>"));
        assert!((result.models[0].usage.cost_usd - 1.6).abs() < 1e-9);
        assert!((result.models[1].usage.cost_usd - 0.14).abs() < 1e-9);
    }

    #[test]
    fn cumulative_session_uses_last_entry_with_real_usage() {
        let rules = [PricingRule {
            model_name: "claude-opus-4-8".to_string(),
            input_price: 10.0,
            cache_input_price: 0.5,
            output_price: 50.0,
        }];
        let entries = vec![
            summary_entry(
                1,
                "claude-opus-4-8",
                token_stats(100_000, 10_000, 200_000),
                false,
            ),
            summary_entry(2, "<synthetic>", token_stats(0, 0, 0), false),
        ];

        let result =
            summarize_session_usage(&PreparedPricingRules::from_rules(rules.into()), &entries);

        assert!((result.usage.cost_usd - 1.6).abs() < 1e-9);
        assert_eq!(result.display_model, "claude-opus-4-8");
        assert_eq!(result.models.len(), 1);
    }

    #[test]
    fn session_cost_uses_cache_write_ttl_breakdown() {
        let rules = [PricingRule {
            model_name: "claude-fable-5".to_string(),
            input_price: 10.0,
            cache_input_price: 1.0,
            output_price: 50.0,
        }];
        let entries = vec![summary_entry(
            1,
            "claude-fable-5",
            TokenStats {
                input: 1_000_000,
                output: 1_000_000,
                cache_read: Some(1_000_000),
                cache_write: Some(2_500_000),
                cache_write_5m: Some(1_500_000),
                cache_write_1h: Some(1_000_000),
                reasoning: None,
                total: 5_500_000,
            },
            true,
        )];

        let result =
            summarize_session_usage(&PreparedPricingRules::from_rules(rules.into()), &entries);

        assert_eq!(result.usage.cache_write_tokens, 2_500_000);
        assert_eq!(result.usage.cache_write_5m_tokens, 1_500_000);
        assert_eq!(result.usage.cache_write_1h_tokens, 1_000_000);
        assert!((result.usage.cost_usd - 99.75).abs() < 1e-9);
    }

    #[test]
    fn session_cost_prefers_provider_reported_cost() {
        let rules = [PricingRule {
            model_name: "grok-4.5".to_string(),
            input_price: 100.0,
            cache_input_price: 100.0,
            output_price: 100.0,
        }];
        let mut entry = summary_entry(
            1,
            "Grok 4.5",
            token_stats(1_000_000, 1_000_000, 1_000_000),
            true,
        );
        entry.cost = Some(CostStats {
            total_api_duration_ms: None,
            total_duration_ms: None,
            total_premium_requests: None,
            reported_cost_usd: Some(0.0123),
        });

        let result =
            summarize_session_usage(&PreparedPricingRules::from_rules(rules.into()), &[entry]);

        assert!((result.usage.cost_usd - 0.0123).abs() < 1e-9);
        assert!((result.models[0].usage.cost_usd - 0.0123).abs() < 1e-9);
    }

    #[test]
    fn unclassified_cache_writes_do_not_use_anthropic_ttl_pricing() {
        let rules = [PricingRule {
            model_name: "gpt-test".to_string(),
            input_price: 10.0,
            cache_input_price: 1.0,
            output_price: 50.0,
        }];
        let entries = vec![summary_entry(
            1,
            "gpt-test",
            TokenStats {
                input: 1_000_000,
                output: 0,
                cache_read: Some(0),
                cache_write: Some(1_000_000),
                cache_write_5m: None,
                cache_write_1h: None,
                reasoning: None,
                total: 2_000_000,
            },
            true,
        )];

        let result =
            summarize_session_usage(&PreparedPricingRules::from_rules(rules.into()), &entries);

        assert_eq!(result.usage.cache_write_tokens, 1_000_000);
        assert_eq!(result.usage.cache_write_5m_tokens, 0);
        assert_eq!(result.usage.cache_write_1h_tokens, 0);
        assert!((result.usage.cost_usd - 10.0).abs() < 1e-9);
    }

    #[test]
    fn missing_copilot_search_b_pricing_is_silent_and_preserves_tokens() {
        let rules = PreparedPricingRules::from_rules(vec![]);

        for model in ["copilot-search-b", "COPILOT-SEARCH-B", " copilot-search-b "] {
            for has_delta in [true, false] {
                let entries = [summary_entry(1, model, token_stats(100, 50, 25), has_delta)];

                let result = summarize_session_usage(&rules, &entries);

                assert_eq!(result.usage.total_tokens, 175);
                assert_eq!(result.usage.input_tokens, 100);
                assert_eq!(result.usage.output_tokens, 50);
                assert_eq!(result.usage.cache_read_tokens, 25);
                assert_eq!(result.usage.cost_usd, 0.0);
                assert_eq!(result.models.len(), 1);
                assert_eq!(result.models[0].model, model.trim());
                assert_eq!(result.models[0].usage.total_tokens, 175);
                assert_eq!(result.models[0].usage.cost_usd, 0.0);
                let was_warned = WARNED_PRICING_MODELS.lock().unwrap().contains(model.trim());
                assert!(!was_warned, "{model} 缺少價格規則時應忽略錯誤提示");
            }
        }
    }

    #[test]
    fn copilot_search_b_uses_available_pricing_and_reported_costs() {
        let rules = PreparedPricingRules::from_rules(vec![PricingRule {
            model_name: "copilot-search-b".to_string(),
            input_price: 2.0,
            cache_input_price: 0.5,
            output_price: 6.0,
        }]);
        let mut entry = summary_entry(
            1,
            "copilot-search-b",
            token_stats(1_000_000, 1_000_000, 1_000_000),
            true,
        );

        let priced = summarize_session_usage(&rules, std::slice::from_ref(&entry));
        assert_eq!(priced.usage.cost_usd, 8.5);
        assert_eq!(priced.models[0].usage.cost_usd, 8.5);

        entry.cost = Some(CostStats {
            total_api_duration_ms: None,
            total_duration_ms: None,
            total_premium_requests: None,
            reported_cost_usd: Some(0.25),
        });
        let reported = summarize_session_usage(&PreparedPricingRules::from_rules(vec![]), &[entry]);
        assert_eq!(reported.usage.cost_usd, 0.25);
        assert_eq!(reported.models[0].usage.cost_usd, 0.25);
    }

    #[test]
    fn missing_pricing_rule_logs_only_once_per_model() {
        let rules = PreparedPricingRules::from_rules(vec![]);

        for model in ["copilot/auto", "copilot-search-b-preview"] {
            let entries = vec![
                summary_entry(1, model, token_stats(100, 50, 0), true),
                summary_entry(2, model, token_stats(200, 80, 0), true),
                summary_entry(3, model, token_stats(300, 90, 0), true),
            ];

            let result = summarize_session_usage(&rules, &entries);
            assert_eq!(result.usage.cost_usd, 0.0);
            assert_eq!(result.models.len(), 1);
            assert_eq!(result.models[0].model, model);
            assert_eq!(result.models[0].usage.cost_usd, 0.0);
            let was_warned = WARNED_PRICING_MODELS.lock().unwrap().contains(model);
            assert!(was_warned, "其他缺少價格規則的模型仍應提示錯誤");
        }
    }

    /// Regression (code review comment 4403291365): a standalone session whose
    /// id equals a synthetic sub-agent id (`<parent>__<agent>`) must not merge
    /// with that sub-agent when sessions are grouped for the daily summary.
    #[test]
    fn group_sessions_keeps_colliding_standalone_and_synthetic_child_separate() {
        let mut standalone = summary_entry(1, "gpt-5", token_stats(100, 0, 0), true);
        standalone.session_id = "a__b".to_string();
        standalone.source_kind = Some("copilot-app".to_string());
        standalone.source_dir_key = Some("aa".to_string());

        let mut child = summary_entry(1, "gpt-5", token_stats(900, 0, 0), true);
        child.session_id = "a__b".to_string();
        child.source_kind = Some("copilot-app".to_string());
        child.source_dir_key = Some("aa".to_string());
        child.parent_session_id = Some("a".to_string());
        child.agent_nickname = Some("b".to_string());

        let entries = [standalone, child];
        let sessions = group_sessions(entries.iter().map(|entry| (entry, "copilot")));

        assert_eq!(
            sessions.len(),
            2,
            "獨立 session a__b 與 session a 的子 Agent b 必須分組成兩個 session"
        );
        let mut totals: Vec<u64> = sessions
            .values()
            .map(|group| {
                group
                    .entries
                    .iter()
                    .filter_map(|entry| entry.delta_tokens.as_ref().map(|tokens| tokens.input))
                    .sum()
            })
            .collect();
        totals.sort();
        assert_eq!(totals, vec![100, 900], "兩組用量不得互相合併");
    }
}
