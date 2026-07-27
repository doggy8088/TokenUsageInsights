use crate::{
    db::{TokenStats, UsageEntry},
    pricing::{calculate_usage_cost, PricingRule},
};
use serde::Serialize;
use std::collections::HashMap;

pub mod daily;
pub mod misc;
pub mod monthly;
pub mod yearly;

pub use daily::*;
pub use misc::*;
pub use monthly::*;
pub use yearly::*;

pub fn normalize_assistant_name(assistant: &str) -> String {
    let normalized = assistant.trim().to_lowercase();
    match normalized.as_str() {
        "claude-code" | "claude_code" | "claudecode" => "claude".to_string(),
        "cursor" => "cursor".to_string(),
        _ => normalized,
    }
}

pub fn is_supported_assistant(assistant: &str) -> bool {
    matches!(
        normalize_assistant_name(assistant).as_str(),
        "antigravity" | "copilot" | "codex" | "claude" | "cursor"
    )
}

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

fn record_usage(
    result: &mut SessionUsageAggregation,
    pricing_rules: &[PricingRule],
    entry: &UsageEntry,
    tokens: &TokenStats,
) {
    let model = entry_model(entry);
    let model_label = model.unwrap_or("Unknown Model");
    let (cache_write_5m, cache_write_1h) = cache_write_breakdown(tokens);
    let cost_usd = match calculate_usage_cost(
        pricing_rules,
        model,
        tokens.input,
        tokens.output,
        tokens.cache_read.unwrap_or(0),
        cache_write_5m,
        cache_write_1h,
    ) {
        Ok(cost) => cost,
        Err(error) => {
            eprintln!(
                "計算成本失敗: session_id={} turn_no={} model={}: {}",
                entry.session_id, entry.turn_no, model_label, error
            );
            0.0
        }
    };

    add_tokens(&mut result.usage, tokens);
    result.usage.cost_usd += cost_usd;

    let model_usage = if let Some(existing) = result
        .models
        .iter_mut()
        .find(|item| item.model == model_label)
    {
        existing
    } else {
        result.models.push(ModelUsageAggregation {
            model: model_label.to_string(),
            usage: UsageAggregation::default(),
        });
        result.models.last_mut().unwrap()
    };
    add_tokens(&mut model_usage.usage, tokens);
    model_usage.usage.cost_usd += cost_usd;
}

pub(crate) fn summarize_session_usage(
    pricing_rules: &[PricingRule],
    entries: &[UsageEntry],
) -> SessionUsageAggregation {
    let has_delta_usage = entries
        .iter()
        .filter_map(|entry| entry.delta_tokens.as_ref())
        .any(has_usage);
    let mut result = SessionUsageAggregation::default();
    let mut display_entry: Option<&UsageEntry> = None;

    if has_delta_usage {
        for entry in entries {
            let Some(tokens) = entry
                .delta_tokens
                .as_ref()
                .filter(|tokens| has_usage(tokens))
            else {
                continue;
            };
            record_usage(&mut result, pricing_rules, entry, tokens);
            if display_entry.is_none_or(|current| entry.turn_no >= current.turn_no) {
                display_entry = Some(entry);
            }
        }
    } else if let Some(entry) = entries
        .iter()
        .filter(|entry| entry.tokens.as_ref().is_some_and(has_usage))
        .max_by_key(|entry| entry.turn_no)
    {
        record_usage(
            &mut result,
            pricing_rules,
            entry,
            entry.tokens.as_ref().unwrap(),
        );
        display_entry = Some(entry);
    }

    result.display_model = display_entry
        .and_then(entry_model)
        .unwrap_or("Unknown Model")
        .to_string();
    result
}

#[derive(Serialize)]
pub struct DateListResponse {
    pub dates: Vec<String>,
}

#[derive(Serialize)]
pub struct MonthListResponse {
    pub months: Vec<String>,
}

#[derive(Serialize)]
pub struct SetupInfoResponse {
    pub platform: String,
    pub workspace_dir: String,
    pub home_dir: String,
    pub antigravity: AssistantSetupStatus,
    pub copilot: AssistantSetupStatus,
    pub codex: AssistantSetupStatus,
    pub claude: AssistantSetupStatus,
    pub cursor: AssistantSetupStatus,
}

#[derive(Serialize)]
pub struct AssistantSetupStatus {
    pub dir_path: String,
    pub data_path: String,
    pub exists: bool,
    pub script_path: String,
    pub source_script_path: String,
    pub settings_path: String,
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

#[derive(Serialize, Clone)]
pub struct SessionSummary {
    pub session_id: String,
    pub session_name: String,
    pub assistant_type: String,
    pub source_kind: String,
    pub cwd: String,
    pub model: String,
    pub total_tokens: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_read_tokens: u64,
    pub total_cache_write_tokens: u64,
    pub total_reasoning_tokens: u64,
    pub max_turn_no: u32,
    pub timestamp: String,
    pub duration_ms: u64,
    pub total_requests: u64,
    pub cost_usd: f64,
    pub parent_session_id: Option<String>,
    pub agent_nickname: Option<String>,
    pub agent_role: Option<String>,
    pub reasoning_effort: Option<String>,
}

#[derive(Serialize)]
pub struct UsageDetailsResponse {
    pub date: String,
    pub home_dir: String,
    pub summary: DaySummary,
    pub sessions: Vec<SessionSummary>,
    pub raw_entries: Vec<UsageEntry>,
}

#[derive(Serialize)]
pub struct MonthlyDailyBreakdown {
    pub date: String,
    pub total_tokens: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_read_tokens: u64,
    pub total_reasoning_tokens: u64,
    pub sessions_count: usize,
    pub cost_usd: f64,
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

#[derive(Serialize)]
pub struct MonthlyDetailsResponse {
    pub year_month: String,
    pub summary: DaySummary,
    pub daily_breakdown: Vec<MonthlyDailyBreakdown>,
    pub projects: Vec<MonthlyProjectSummary>,
    pub models: Vec<MonthlyModelSummary>,
    pub agent_breakdown: HashMap<String, AgentBreakdown>,
}

#[derive(Serialize)]
pub struct YearlyMonthlyBreakdown {
    pub month: String,
    pub total_tokens: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_read_tokens: u64,
    pub total_reasoning_tokens: u64,
    pub sessions_count: usize,
    pub cost_usd: f64,
}

#[derive(Serialize)]
pub struct YearlyDetailsResponse {
    pub year: String,
    pub summary: DaySummary,
    pub monthly_breakdown: Vec<YearlyMonthlyBreakdown>,
    pub projects: Vec<MonthlyProjectSummary>,
    pub models: Vec<MonthlyModelSummary>,
    pub agent_breakdown: HashMap<String, AgentBreakdown>,
}

#[derive(Serialize)]
pub struct YearListResponse {
    pub years: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use std::env;
    use std::fs;
    use std::sync::OnceLock;
    use tokio::sync::{Mutex, MutexGuard};

    static TEST_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    async fn lock_test_env() -> MutexGuard<'static, ()> {
        TEST_ENV_LOCK.get_or_init(|| Mutex::new(())).lock().await
    }

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

    fn usage_entry(turn_no: u32, model: &str, tokens: TokenStats, has_delta: bool) -> UsageEntry {
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
            parent_session_id: None,
            agent_nickname: None,
            agent_role: None,
            reasoning_effort: None,
        }
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
            usage_entry(
                1,
                "claude-opus-4-8",
                token_stats(100_000, 10_000, 200_000),
                true,
            ),
            usage_entry(
                2,
                "claude-fable-5",
                token_stats(50_000, 5_000, 100_000),
                true,
            ),
            usage_entry(3, "<synthetic>", token_stats(0, 0, 0), true),
        ];

        let result = summarize_session_usage(&rules, &entries);

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
            usage_entry(
                1,
                "claude-opus-4-8",
                token_stats(100_000, 10_000, 200_000),
                false,
            ),
            usage_entry(2, "<synthetic>", token_stats(0, 0, 0), false),
        ];

        let result = summarize_session_usage(&rules, &entries);

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
        let entries = vec![usage_entry(
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

        let result = summarize_session_usage(&rules, &entries);

        assert_eq!(result.usage.cache_write_tokens, 2_500_000);
        assert_eq!(result.usage.cache_write_5m_tokens, 1_500_000);
        assert_eq!(result.usage.cache_write_1h_tokens, 1_000_000);
        assert!((result.usage.cost_usd - 99.75).abs() < 1e-9);
    }

    #[test]
    fn unclassified_cache_writes_do_not_use_anthropic_ttl_pricing() {
        let rules = [PricingRule {
            model_name: "gpt-test".to_string(),
            input_price: 10.0,
            cache_input_price: 1.0,
            output_price: 50.0,
        }];
        let entries = vec![usage_entry(
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

        let result = summarize_session_usage(&rules, &entries);

        assert_eq!(result.usage.cache_write_tokens, 1_000_000);
        assert_eq!(result.usage.cache_write_5m_tokens, 0);
        assert_eq!(result.usage.cache_write_1h_tokens, 0);
        assert!((result.usage.cost_usd - 10.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn test_yearly_handlers() {
        let _guard = lock_test_env().await;
        let temp_dir = std::path::PathBuf::from("temp_test_insights");
        if temp_dir.exists() {
            let _ = fs::remove_dir_all(&temp_dir);
        }
        fs::create_dir_all(&temp_dir).unwrap();
        env::set_var("INSIGHTS_DIR", temp_dir.to_str().unwrap());

        // Initialize SQLite DB
        let conn = db::get_db_conn().unwrap();
        db::init_db(&conn).unwrap();

        // Insert some fake entries
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, timestamp, date, session_id, session_name, cwd, turn_no, model,
                tokens_input, tokens_output, tokens_cache_read, tokens_total,
                delta_input, delta_output, delta_cache_read, delta_total
            ) VALUES (
                'antigravity', '2026-07-01 12:00:00', '2026-07-01', 'session_1', 'Session 1', '/cwd/1', 1, 'Gemini 3.5 Flash',
                100, 50, 20, 150,
                100, 50, 20, 150
            )",
            [],
        ).unwrap();

        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, timestamp, date, session_id, session_name, cwd, turn_no, model,
                tokens_input, tokens_output, tokens_cache_read, tokens_total,
                delta_input, delta_output, delta_cache_read, delta_total
            ) VALUES (
                'antigravity', '2026-07-01 12:05:00', '2026-07-01', 'session_1', 'Session 1', '/cwd/1', 2, 'Gemini 3.5 Flash',
                120, 60, 20, 180,
                20, 10, 0, 30
            )",
            [],
        ).unwrap();

        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, timestamp, date, session_id, session_name, cwd, turn_no, model,
                tokens_input, tokens_output, tokens_cache_read, tokens_total,
                delta_input, delta_output, delta_cache_read, delta_total
            ) VALUES (
                'antigravity', '2025-06-01 12:00:00', '2025-06-01', 'session_2', 'Session 2', '/cwd/2', 1, 'Gemini 3.5 Flash',
                200, 100, 40, 300,
                200, 100, 40, 300
            )",
            [],
        ).unwrap();

        // 1. Test get_available_years
        let conn = db::get_db_conn().unwrap();
        let mut stmt = conn
            .prepare("SELECT DISTINCT substr(date, 1, 4) FROM usage_entries ORDER BY date DESC")
            .unwrap();
        let mut rows = stmt.query([]).unwrap();
        let mut years = Vec::new();
        while let Some(row) = rows.next().unwrap() {
            years.push(row.get::<_, String>(0).unwrap());
        }
        assert_eq!(years, vec!["2026", "2025"]);

        // Cleanup
        let _ = fs::remove_dir_all(&temp_dir);
    }
}
