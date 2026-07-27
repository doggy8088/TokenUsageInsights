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

pub fn select_session_model(entries: &[UsageEntry]) -> String {
    entries
        .iter()
        .rev()
        .filter_map(|entry| entry.model.as_deref())
        .find(|model| {
            let normalized = model.trim().to_ascii_lowercase();
            !normalized.is_empty()
                && normalized != "cursor agent"
                && normalized != "cursor ide"
                && normalized != "unknown model"
        })
        .map(str::to_string)
        .unwrap_or_else(|| "Unknown Model".to_string())
}

pub fn select_session_mode(assistant_type: &str, entries: &[UsageEntry]) -> Option<String> {
    if normalize_assistant_name(assistant_type) != "cursor" {
        return None;
    }

    let has_ide_source = entries.iter().any(|entry| {
        entry
            .transcript_path
            .as_deref()
            .map(|path| path.replace('\\', "/").contains("/ide-transcripts/"))
            .unwrap_or(false)
            || entry
                .model
                .as_deref()
                .map(|model| model.eq_ignore_ascii_case("Cursor IDE"))
                .unwrap_or(false)
    });
    if has_ide_source {
        return Some("ide".to_string());
    }

    let has_agent_source = entries.iter().any(|entry| {
        entry
            .transcript_path
            .as_deref()
            .map(|path| path.replace('\\', "/").contains("/agent-transcripts/"))
            .unwrap_or(false)
            || entry
                .model
                .as_deref()
                .map(|model| model.eq_ignore_ascii_case("Cursor Agent"))
                .unwrap_or(false)
    });
    has_agent_source.then(|| "agent".to_string())
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
    pub cwd: String,
    pub model: String,
    pub mode: Option<String>,
    pub total_tokens: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_read_tokens: u64,
    pub total_cache_write_tokens: u64,
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
    pub sessions: Vec<ModelSessionDetail>,
}

#[derive(Serialize, Clone)]
pub struct ModelSessionDetail {
    pub session_id: String,
    pub session_name: String,
    pub assistant_type: String,
    pub date: String,
    pub timestamp: String,
    pub cwd: String,
    pub mode: Option<String>,
    pub total_tokens: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_read_tokens: u64,
    pub total_reasoning_tokens: u64,
    pub reasoning_effort: Option<String>,
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
    use super::{select_session_mode, select_session_model};
    use crate::db::{self, UsageEntry};
    use std::env;
    use std::fs;
    use std::sync::OnceLock;
    use tokio::sync::{Mutex, MutexGuard};

    static TEST_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    async fn lock_test_env() -> MutexGuard<'static, ()> {
        TEST_ENV_LOCK.get_or_init(|| Mutex::new(())).lock().await
    }

    fn usage_entry_with_model(model: &str) -> UsageEntry {
        UsageEntry {
            timestamp: "2026-07-24 12:00:00".to_string(),
            session_id: "cursor-session".to_string(),
            session_name: None,
            transcript_path: Some(
                "/tmp/.cursor/projects/demo/agent-transcripts/session.jsonl".to_string(),
            ),
            cwd: None,
            version: None,
            turn_no: 1,
            model: Some(model.to_string()),
            model_id: Some(model.to_string()),
            tokens: None,
            delta_tokens: None,
            context: None,
            cost: None,
            parent_session_id: None,
            agent_nickname: None,
            agent_role: None,
            reasoning_effort: None,
        }
    }

    #[test]
    fn cursor_session_model_prefers_latest_concrete_model() {
        let entries = vec![
            usage_entry_with_model("composer-2.5"),
            usage_entry_with_model("Cursor Agent"),
        ];

        assert_eq!(select_session_model(&entries), "composer-2.5");
        assert_eq!(
            select_session_mode("cursor", &entries).as_deref(),
            Some("agent")
        );
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
