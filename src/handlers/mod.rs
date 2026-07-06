use crate::db::UsageEntry;
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
        _ => normalized,
    }
}

pub fn is_supported_assistant(assistant: &str) -> bool {
    matches!(
        normalize_assistant_name(assistant).as_str(),
        "antigravity" | "copilot" | "codex" | "claude"
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
    pub workspace_dir: String,
    pub home_dir: String,
    pub antigravity: AssistantSetupStatus,
    pub copilot: AssistantSetupStatus,
    pub codex: AssistantSetupStatus,
    pub claude: AssistantSetupStatus,
}

#[derive(Serialize)]
pub struct AssistantSetupStatus {
    pub dir_path: String,
    pub exists: bool,
    pub script_path: String,
}

#[derive(Serialize, Default, Clone)]
pub struct DaySummary {
    pub total_sessions: usize,
    pub total_tokens: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_read_tokens: u64,
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
    pub cwd: String,
    pub model: String,
    pub total_tokens: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_read_tokens: u64,
    pub total_reasoning_tokens: u64,
    pub max_turn_no: u32,
    pub timestamp: String,
    pub duration_ms: u64,
    pub cost_usd: f64,
    pub parent_session_id: Option<String>,
    pub agent_nickname: Option<String>,
    pub agent_role: Option<String>,
    pub reasoning_effort: Option<String>,
}

#[derive(Serialize)]
pub struct UsageDetailsResponse {
    pub date: String,
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
    use crate::db;
    use std::env;
    use std::fs;
    use std::sync::OnceLock;
    use tokio::sync::{Mutex, MutexGuard};

    static TEST_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    async fn lock_test_env() -> MutexGuard<'static, ()> {
        TEST_ENV_LOCK.get_or_init(|| Mutex::new(())).lock().await
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
