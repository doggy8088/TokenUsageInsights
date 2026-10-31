use std::{
    cmp::Ordering,
    collections::{hash_map::Entry, BTreeMap, HashMap, HashSet},
    hash::Hash,
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

/// 用量列先後順序：(turn_no, timestamp)，供「取最新一筆」語意共用。
fn usage_entry_order(left: &UsageEntry, right: &UsageEntry) -> Ordering {
    left.turn_no
        .cmp(&right.turn_no)
        .then_with(|| left.timestamp.cmp(&right.timestamp))
}

pub(crate) fn latest_usage_entry<'a>(
    entries: impl IntoIterator<Item = &'a UsageEntry>,
) -> Option<&'a UsageEntry> {
    entries
        .into_iter()
        .max_by(|left, right| usage_entry_order(left, right))
}

/// 逐次呼叫列（Copilot App / CLI agents 的逐呼叫對帳列）的
/// `import_source_id` 帶 `:call:{call_identity}` 標記（見
/// `db::replace_copilot_call_rows` 寫入的 `…:call:{call_identity}`）。這類列的
/// `duration_ms` 只是單次呼叫耗時；舊聚合列、Codex session 累積值等其他來源
/// 每列已帶整回合／整個 session 的累積耗時。
const PER_CALL_IMPORT_SOURCE_MARKER: &str = ":call:";

/// 逐次呼叫列判別（review PRRT_kwDOTOgsec6yUKF4）：必須同時滿足
/// (1) 來源種類是 Copilot collector（copilot-app / copilot-cli）；
/// (2) ID 落在該 collector 寫入端的命名空間前綴內（注意：kind "copilot-cli"
///     的列前綴是 "copilot-cli-agents:"，kind 與前綴不同名，對應表照
///     `db::replace_copilot_call_rows` 寫入格式抄錄）；
/// (3) 前綴之後帶 `:call:{call_identity}` 標記。
/// 外部匯入（`import --file/--url`）的 `import_source_id` 是自由格式
/// （`db::normalize_import_source_id` 只做 trim），單比 `:call:` 子字串會把
/// 偽裝的累積快照列誤判成逐次呼叫列而被加總。殘餘風險：同時偽造 kind＋前綴
/// ＋標記的匯入列與真逐次呼叫列不可區分（db.rs 全域同此信任模型）；聚合列的
/// agent 段字面含 `:call:` 亦會誤判，但該段為來源提供的 slug，實際不可達。
fn is_per_call_import_source(source_kind: Option<&str>, import_source_id: &str) -> bool {
    let namespace = match source_kind {
        Some("copilot-app") => "copilot-app:",
        Some("copilot-cli") => "copilot-cli-agents:",
        _ => return false,
    };
    import_source_id
        .strip_prefix(namespace)
        .is_some_and(|rest| rest.contains(PER_CALL_IMPORT_SOURCE_MARKER))
}

/// 非逐次呼叫列的覆蓋認領（review PRRT_kwDOTOgsec6yUKGR），形狀鏡像
/// `db::classify_copilot_app_coverage_claim`：`copilot-app:{key}:{sid}:{turn}`
/// 三段式 ID（sid 為本組 session）認領整個回合；其他形狀只認領自己的
/// (turn, model) 桶（欄位取自 `UsageEntry`，不解析 ID）。回合 0 無法放置於
/// 回合空間，保守落在桶級認領（匯入端對此判定為 Conflict，顯示端無拒絕語意）。
#[derive(Debug, Clone, PartialEq, Eq)]
enum AggregateCoverageClaim {
    WholeTurn(u32),
    Split(u32, Option<String>),
}

fn aggregate_coverage_claim(
    entry: &UsageEntry,
    import_source_id: Option<&str>,
) -> AggregateCoverageClaim {
    if entry.turn_no > 0 {
        if let Some(rest) = import_source_id.and_then(|id| id.strip_prefix("copilot-app:")) {
            let segments: Vec<&str> = rest.split(':').collect();
            if segments.len() == 3 && segments[1] == entry.session_id.as_str() {
                return AggregateCoverageClaim::WholeTurn(entry.turn_no);
            }
        }
    }
    AggregateCoverageClaim::Split(entry.turn_no, entry.model.clone())
}

/// 同認領重複列取最新一筆（排序沿用 `usage_entry_order`）：同一覆蓋範圍的
/// 快照／重寫殘影絕不疊加，維持累積值 latest-only 不變量。
fn keep_latest<'a, K: Eq + Hash>(
    claims: &mut HashMap<K, &'a UsageEntry>,
    key: K,
    entry: &'a UsageEntry,
) {
    match claims.entry(key) {
        Entry::Occupied(mut occupied) => {
            if usage_entry_order(entry, occupied.get()).is_gt() {
                occupied.insert(entry);
            }
        }
        Entry::Vacant(vacant) => {
            vacant.insert(entry);
        }
    }
}

fn entry_duration_ms(entry: &UsageEntry) -> f64 {
    entry
        .cost
        .as_ref()
        .and_then(|cost| cost.total_api_duration_ms)
        .unwrap_or(0.0)
}

/// 計算單一 session 分組的顯示耗時（毫秒）。
///
/// 逐次呼叫列（`is_per_call_import_source`）每列只有單次呼叫耗時，同回合多次
/// 呼叫必須全部加總才等於逐次呼叫接管前聚合列顯示的回合總耗時（review
/// comment 4436837582）；匯入聚合列是整回合／(turn, model) 桶的 per-turn
/// SUM，依實際覆蓋範圍納入（review PRRT_kwDOTOgsec6yUKGR）：與呼叫列同覆蓋
/// 範圍的聚合列讓位不計（呼叫優先防雙計——`copilot-cli` 匯入不經覆蓋裁決，
/// 同覆蓋並存是合法狀態），不重疊的聚合列按認領加總；同認領重複列取最新一
/// 筆（同一覆蓋的快照殘影，絕不疊加），WholeTurn 認領吸收同回合 Split 認領
/// （同回合雙表示）。1448fc7 的「混和群組固定以逐次呼叫列加總為準」收斂為
/// 「同覆蓋範圍以逐次呼叫列為準、不重疊聚合按覆蓋另計」。
/// 沒有逐次呼叫列的分組（舊聚合列、Codex 每列皆為 session 累積值等）維持
/// 「取最新一筆」的累積值語意。
pub(crate) fn session_duration_ms<'a>(
    entries: impl IntoIterator<Item = (&'a UsageEntry, Option<&'a str>)>,
) -> u64 {
    let entries: Vec<(&UsageEntry, Option<&str>)> = entries.into_iter().collect();
    let mut per_call_rows = 0usize;
    let mut per_call_duration = 0.0;
    // 呼叫列已覆蓋的 (turn, model) 桶。重疊判定必須到桶級：同回合跨模型的
    // 聚合列是合法的不重疊歷史（`db::copilot_app_claims_overlap` 同語意），
    // 回合級連坐會把這類聚合列誤丟而少計。
    let mut covered_buckets: HashSet<(u32, Option<String>)> = HashSet::new();
    for &(entry, import_source_id) in &entries {
        if import_source_id
            .is_some_and(|id| is_per_call_import_source(entry.source_kind.as_deref(), id))
        {
            per_call_rows += 1;
            per_call_duration += entry_duration_ms(entry);
            covered_buckets.insert((entry.turn_no, entry.model.clone()));
        }
    }
    if per_call_rows == 0 {
        return latest_usage_entry(entries.iter().map(|(entry, _)| *entry))
            .map(entry_duration_ms)
            .unwrap_or(0.0) as u64;
    }
    let mut whole_turn_claims: HashMap<u32, &UsageEntry> = HashMap::new();
    let mut split_claims: HashMap<(u32, Option<String>), &UsageEntry> = HashMap::new();
    for &(entry, import_source_id) in &entries {
        if import_source_id
            .is_some_and(|id| is_per_call_import_source(entry.source_kind.as_deref(), id))
        {
            continue;
        }
        let claim = aggregate_coverage_claim(entry, import_source_id);
        // 覆蓋讓位：與呼叫列同覆蓋範圍的聚合列不計（呼叫優先）。
        let overlaps_calls = match &claim {
            AggregateCoverageClaim::WholeTurn(turn) => covered_buckets
                .iter()
                .any(|(covered_turn, _)| covered_turn == turn),
            AggregateCoverageClaim::Split(turn, model) => {
                covered_buckets.contains(&(*turn, model.clone()))
            }
        };
        if overlaps_calls {
            continue;
        }
        match claim {
            AggregateCoverageClaim::WholeTurn(turn) => {
                keep_latest(&mut whole_turn_claims, turn, entry);
            }
            AggregateCoverageClaim::Split(turn, model) => {
                keep_latest(&mut split_claims, (turn, model), entry);
            }
        }
    }
    let mut total = per_call_duration;
    for entry in whole_turn_claims.values() {
        total += entry_duration_ms(entry);
    }
    for ((turn, _), entry) in &split_claims {
        // WholeTurn 吸收同回合 Split（同回合雙表示，同回合只留一支）。
        if !whole_turn_claims.contains_key(turn) {
            total += entry_duration_ms(entry);
        }
    }
    total as u64
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
        duration_entry_with_source(
            turn_no,
            timestamp,
            duration_ms,
            Some("copilot-app"),
            "gpt-5",
        )
    }

    fn duration_entry_with_source(
        turn_no: u32,
        timestamp: &str,
        duration_ms: f64,
        source_kind: Option<&str>,
        model: &str,
    ) -> UsageEntry {
        UsageEntry {
            timestamp: timestamp.to_string(),
            session_id: "duration-session".to_string(),
            session_name: None,
            transcript_path: None,
            cwd: None,
            version: None,
            turn_no,
            model: Some(model.to_string()),
            model_id: Some(model.to_string()),
            tokens: None,
            delta_tokens: None,
            context: None,
            cost: Some(CostStats {
                total_api_duration_ms: Some(duration_ms),
                total_duration_ms: None,
                total_premium_requests: None,
                reported_cost_usd: None,
            }),
            source_kind: source_kind.map(str::to_string),
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
    fn session_duration_ms_keeps_free_format_colon_call_ids_latest_only() {
        // 外部匯入的 import_source_id 是自由格式：兩筆 Codex 累積快照即使帶
        // `:call:` 子字樣也不得被當成逐次呼叫列加總成 7000ms（review
        // PRRT_kwDOTOgsec6yUKF4），必須維持累積值 latest-only 3500ms。
        let mut first = duration_entry_with_source(
            1,
            "2026-07-10T10:00:00Z",
            3500.0,
            Some("codex-desktop"),
            "gpt-5",
        );
        first.session_id = "vendor-s".to_string();
        let mut second = duration_entry_with_source(
            2,
            "2026-07-10T10:05:00Z",
            3500.0,
            Some("codex-desktop"),
            "gpt-5",
        );
        second.session_id = "vendor-s".to_string();

        assert_eq!(
            session_duration_ms([
                (&first, Some("vendor:call:1")),
                (&second, Some("vendor:call:2")),
            ]),
            3500
        );
    }

    #[test]
    fn session_duration_ms_requires_collector_namespace_for_per_call_sum() {
        // 來源種類正確但 ID 不在 collector 寫入端命名空間前綴內的自由格式列
        // 不得冒充逐次呼叫列（前綴檢查）：latest-only 3500ms，不可加總成 7000ms。
        let first = duration_entry(1, "2026-07-10T10:00:00Z", 3500.0);
        let second = duration_entry(2, "2026-07-10T10:05:00Z", 3500.0);

        assert_eq!(
            session_duration_ms([
                (&first, Some("vendor:call:1")),
                (&second, Some("vendor:call:2")),
            ]),
            3500
        );
    }

    #[test]
    fn session_duration_ms_requires_source_kind_for_per_call_sum() {
        // ID 前綴正確但來源種類不是 Copilot collector 的列不得冒充逐次呼叫列
        // （來源種類檢查）：latest-only 100ms，不可加總成 200ms。
        let first = duration_entry_with_source(1, "2026-07-10T10:00:00Z", 100.0, None, "gpt-5");
        let second = duration_entry_with_source(1, "2026-07-10T10:01:00Z", 100.0, None, "gpt-5");

        assert_eq!(
            session_duration_ms([
                (&first, Some("copilot-app:aa:duration-session:call:h1")),
                (&second, Some("copilot-app:aa:duration-session:call:h2")),
            ]),
            100
        );
    }

    #[test]
    fn session_duration_ms_mixed_group_prefers_per_call_sum() {
        // 混和分組（逐次呼叫列 + 匯入聚合列並存）同覆蓋範圍以逐次呼叫列加總
        // 為準：同回合重疊時聚合列讓位（1448fc7 鎖定的「呼叫優先」取捨收斂為
        // 「同覆蓋呼叫優先、不重疊聚合另計」，跨回合情境見
        // session_duration_ms_mixed_group_adds_disjoint_aggregate_turns）。
        let call = duration_entry(1, "2026-07-10T10:00:00Z", 100.0);
        let aggregate = duration_entry(1, "2026-07-10T10:05:00Z", 3500.0);

        assert_eq!(
            session_duration_ms([
                (&call, Some("copilot-app:bb:duration-session:call:h9")),
                (&aggregate, Some("copilot-app:bb:duration-session:1")),
            ]),
            100
        );
    }

    #[test]
    fn session_duration_ms_mixed_group_adds_disjoint_aggregate_turns() {
        // 混和分組中跨回合不重疊的聚合列必須按覆蓋範圍納入（review
        // PRRT_kwDOTOgsec6yUKGR）：turn 1 聚合 3500ms + turn 2 呼叫 100ms =
        // 3600ms，不可因存在呼叫列而整組丟棄只剩 100ms。
        let aggregate = duration_entry(1, "2026-07-10T10:00:00Z", 3500.0);
        let call = duration_entry(2, "2026-07-10T10:05:00Z", 100.0);

        assert_eq!(
            session_duration_ms([
                (&aggregate, Some("copilot-app:aa:duration-session:1")),
                (&call, Some("copilot-app:aa:duration-session:call:h9")),
            ]),
            3600
        );
    }

    #[test]
    fn session_duration_ms_mixed_group_sums_disjoint_aggregates_and_keeps_overlap_subordinate() {
        // 不同回合的聚合列是 per-turn SUM，須按不相交覆蓋加總而非整組取最新：
        // 3500 + 400 + 100 = 4000ms（「聚合取最新一支」的錯誤修法會得 500ms；
        // 驗證報告 B-4 所列期望值 3900 為漏加呼叫列的算術筆誤，其公式
        // ΣCall + ΣAggregate 與 500ms 交叉驗證皆指向 4000ms）。
        let first = duration_entry(1, "2026-07-10T10:00:00Z", 3500.0);
        let second = duration_entry(2, "2026-07-10T10:05:00Z", 400.0);
        let call = duration_entry(3, "2026-07-10T10:10:00Z", 100.0);

        assert_eq!(
            session_duration_ms([
                (&first, Some("copilot-app:aa:duration-session:1")),
                (&second, Some("copilot-app:aa:duration-session:2")),
                (&call, Some("copilot-app:aa:duration-session:call:h9")),
            ]),
            4000
        );
    }

    #[test]
    fn session_duration_ms_mixed_group_keeps_cross_model_aggregate_beside_calls() {
        // 同回合跨模型的聚合列與呼叫列覆蓋不同 (turn, model) 桶，是合法的
        // 不重疊歷史（db::copilot_app_claims_overlap 同語意），不可被回合級
        // 連坐丟棄：3500 + 100 = 3600ms。
        let aggregate = duration_entry_with_source(
            1,
            "2026-07-10T10:00:00Z",
            3500.0,
            Some("copilot-app"),
            "gpt-5",
        );
        let call = duration_entry_with_source(
            1,
            "2026-07-10T10:05:00Z",
            100.0,
            Some("copilot-app"),
            "gpt-5-mini",
        );

        assert_eq!(
            session_duration_ms([
                (
                    &aggregate,
                    Some("copilot-app:aa:duration-session:1:main:aa3")
                ),
                (&call, Some("copilot-app:aa:duration-session:call:h9")),
            ]),
            3600
        );
    }

    #[test]
    fn session_duration_ms_mixed_group_keeps_latest_duplicate_aggregate_claim() {
        // 同認領重複聚合列＝同一覆蓋範圍的快照／重寫殘影，只取最新一筆、
        // 絕不疊加：3500/4000ms 兩列同認領 + turn 2 呼叫 100ms = 4100ms。
        let stale = duration_entry_with_source(
            1,
            "2026-07-10T10:00:00Z",
            3500.0,
            Some("copilot-app"),
            "gpt-5",
        );
        let fresh = duration_entry_with_source(
            1,
            "2026-07-10T10:05:00Z",
            4000.0,
            Some("copilot-app"),
            "gpt-5",
        );
        let call = duration_entry(2, "2026-07-10T10:10:00Z", 100.0);

        assert_eq!(
            session_duration_ms([
                (&stale, Some("copilot-app:aa:duration-session:1:main:aa3")),
                (&fresh, Some("copilot-app:aa:duration-session:1:main:bb4")),
                (&call, Some("copilot-app:aa:duration-session:call:h9")),
            ]),
            4100
        );
    }

    #[test]
    fn session_durations_by_identity_sums_disjoint_aggregate_and_call_turns() {
        // 月報模型明細／日報 session 明細共用的分組耗時路徑（handler 層測試
        // 位於 src/handlers/*，非本模組所有）：混和分組跨回合不重疊的聚合 +
        // 呼叫須合計 3600ms，而非只剩呼叫的 100ms。
        let aggregate = duration_entry(1, "2026-07-10T10:00:00Z", 3500.0);
        let call = duration_entry(2, "2026-07-10T10:05:00Z", 100.0);

        let durations = session_durations_by_identity([
            (
                &aggregate,
                "copilot",
                Some("copilot-app:aa:duration-session:1"),
            ),
            (
                &call,
                "copilot",
                Some("copilot-app:aa:duration-session:call:h9"),
            ),
        ]);

        let identity = SessionIdentity::from_entry("copilot", &aggregate);
        assert_eq!(durations.get(&identity).copied(), Some(3600));
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
