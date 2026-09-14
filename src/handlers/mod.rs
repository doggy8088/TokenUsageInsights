use crate::{
    db::UsageEntry,
    reporting::{
        summarize_session_usage, AgentBreakdown, DaySummary, MonthlyModelSummary,
        MonthlyProjectSummary, UsageAggregation,
    },
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
        "grok-build" | "grok_build" | "grokbuild" | "grok" => "grok".to_string(),
        "pi-coding-agent" | "pi_coding_agent" | "picodingagent" | "pi" => "pi".to_string(),
        "omp" | "oh-my-pi" | "oh_my_pi" | "ohmypi" => "omp".to_string(),
        "muse" | "muse-code" | "muse_code" | "musecode" | "code-muse" | "code_muse" => {
            "muse".to_string()
        }
        _ => normalized,
    }
}

pub fn is_supported_assistant(assistant: &str) -> bool {
    matches!(
        normalize_assistant_name(assistant).as_str(),
        "antigravity" | "copilot" | "codex" | "claude" | "cursor" | "grok" | "pi" | "omp" | "muse"
    )
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
    pub copilot_app: AssistantSetupStatus,
    pub codex: AssistantSetupStatus,
    pub claude: AssistantSetupStatus,
    pub cursor: AssistantSetupStatus,
    pub grok: AssistantSetupStatus,
    pub pi: AssistantSetupStatus,
    pub omp: AssistantSetupStatus,
    pub muse: AssistantSetupStatus,
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

#[derive(Serialize, Clone)]
pub struct SessionSummary {
    pub session_id: String,
    pub session_name: String,
    pub assistant_type: String,
    pub source_kind: String,
    pub source_dir_key: Option<String>,
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

#[derive(Serialize, Clone)]
pub struct ModelSessionDetail {
    pub session_id: String,
    pub session_name: String,
    pub assistant_type: String,
    pub source_kind: String,
    pub source_dir_key: Option<String>,
    pub date: Option<String>,
    pub timestamp: String,
    pub cwd: String,
    pub model: String,
    pub total_tokens: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_read_tokens: u64,
    pub total_cache_write_tokens: u64,
    pub total_reasoning_tokens: u64,
    pub max_turn_no: u32,
    pub duration_ms: u64,
    pub total_requests: u64,
    pub cost_usd: f64,
    pub session_model: String,
    pub session_total_tokens: u64,
    pub session_total_input_tokens: u64,
    pub session_total_output_tokens: u64,
    pub session_total_cache_read_tokens: u64,
    pub session_total_cache_write_tokens: u64,
    pub session_total_reasoning_tokens: u64,
    pub session_cost_usd: f64,
    pub parent_session_id: Option<String>,
    pub agent_nickname: Option<String>,
    pub agent_role: Option<String>,
    pub reasoning_effort: Option<String>,
}

#[derive(Serialize)]
pub struct ModelSessionsResponse {
    pub period: String,
    pub model: String,
    pub mode: Option<String>,
    pub sessions: Vec<ModelSessionDetail>,
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
    use crate::db::{self, TokenStats};
    use crate::pricing::PreparedPricingRules;
    use crate::pricing::PricingRule;
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
            source_dir_key: None,
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
            usage_entry(
                1,
                "claude-opus-4-8",
                token_stats(100_000, 10_000, 200_000),
                false,
            ),
            usage_entry(2, "<synthetic>", token_stats(0, 0, 0), false),
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
        let mut entry = usage_entry(
            1,
            "Grok 4.5",
            token_stats(1_000_000, 1_000_000, 1_000_000),
            true,
        );
        entry.cost = Some(db::CostStats {
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

        let result =
            summarize_session_usage(&PreparedPricingRules::from_rules(rules.into()), &entries);

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

    #[test]
    fn missing_pricing_rule_logs_only_once_per_model() {
        let rules = PreparedPricingRules::from_rules(vec![]);
        let entries = vec![
            usage_entry(1, "copilot/auto", token_stats(100, 50, 0), true),
            usage_entry(2, "copilot/auto", token_stats(200, 80, 0), true),
            usage_entry(3, "copilot/auto", token_stats(300, 90, 0), true),
        ];

        let result = summarize_session_usage(&rules, &entries);
        assert_eq!(result.usage.cost_usd, 0.0);
        assert_eq!(result.models.len(), 1);
        assert_eq!(result.models[0].model, "copilot/auto");
        assert_eq!(result.models[0].usage.cost_usd, 0.0);

        // Verify the model was recorded so subsequent pricing warnings are suppressed.
        assert!(crate::reporting::was_pricing_model_warned("copilot/auto"));
    }
}
