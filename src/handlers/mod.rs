use crate::{
    db::UsageEntry,
    reporting::{
        summarize_session_usage, AgentBreakdown, AgentPeriodUsage, DaySummary, MonthlyModelSummary,
        MonthlyProjectSummary,
    },
};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};

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
        "mcode" | "minimax-code" | "minimax_code" | "minimaxcode" | "mini-max-code"
        | "mini_max_code" => "mcode".to_string(),
        _ => normalized,
    }
}

/// 合併本機所有 Agent 的彙總範圍；只供唯讀的報表 API 使用。
pub const ALL_ASSISTANTS_SCOPE: &str = "all";

/// 唯讀報表 API 可接受單一 Agent 或 [`ALL_ASSISTANTS_SCOPE`]；匯入、匯出、
/// 撤銷與 Session 詳情等需要明確來源的 API 仍必須使用 [`is_supported_assistant`]。
pub fn is_supported_assistant_scope(assistant: &str) -> bool {
    normalize_assistant_name(assistant) == ALL_ASSISTANTS_SCOPE || is_supported_assistant(assistant)
}

pub fn is_supported_assistant(assistant: &str) -> bool {
    matches!(
        normalize_assistant_name(assistant).as_str(),
        "antigravity"
            | "copilot"
            | "codex"
            | "claude"
            | "cursor"
            | "grok"
            | "pi"
            | "omp"
            | "muse"
            | "mcode"
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
    pub mcode: AssistantSetupStatus,
}

#[derive(Serialize)]
pub struct AssistantSetupStatus {
    pub dir_path: String,
    pub data_path: String,
    pub exists: bool,
    pub script_path: String,
    pub source_script_path: String,
    pub settings_path: String,
    pub statusline_command: String,
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
pub struct RawUsageEntry {
    pub assistant_type: String,
    #[serde(flatten)]
    pub entry: UsageEntry,
}

#[derive(Serialize)]
pub struct UsageDetailsResponse {
    pub date: String,
    pub home_dir: String,
    pub summary: DaySummary,
    pub sessions: Vec<SessionSummary>,
    pub raw_entries: Vec<RawUsageEntry>,
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
    pub agents: BTreeMap<String, AgentPeriodUsage>,
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
    pub agents: BTreeMap<String, AgentPeriodUsage>,
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
    use std::{env, fs, sync::OnceLock};
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

    #[test]
    fn assistant_scope_accepts_all_only_for_aggregate_reports() {
        assert!(super::is_supported_assistant_scope("all"));
        assert!(super::is_supported_assistant_scope(" ALL "));
        assert!(super::is_supported_assistant_scope("claude-code"));
        assert!(!super::is_supported_assistant_scope("all,codex"));
        assert!(!super::is_supported_assistant_scope("unknown"));
        // 匯入、匯出與 Session 詳情等需要明確來源的 API 不可接受合併範圍。
        assert!(!super::is_supported_assistant("all"));
    }

    async fn response_json(response: axum::response::Response) -> (u16, serde_json::Value) {
        let status = response.status().as_u16();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn all_agents_reports_merge_every_assistant() {
        use axum::{
            extract::{Path, Query},
            response::IntoResponse,
        };

        let _guard = lock_test_env().await;
        let temp_dir = env::temp_dir().join(format!(
            "tui-all-agents-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        fs::create_dir_all(&temp_dir).unwrap();
        let previous_insights_dir = env::var_os("INSIGHTS_DIR");
        env::set_var("INSIGHTS_DIR", &temp_dir);

        // 先建立資料庫檔案，避免 get_db_conn 將舊位置的資料庫搬進測試目錄。
        let conn = rusqlite::Connection::open(temp_dir.join("token_usage_insights.db")).unwrap();
        db::init_db(&conn).unwrap();
        for (assistant, timestamp, date, session_id, turn_no, input, output) in [
            (
                "codex",
                "2026-07-01 09:00:00",
                "2026-07-01",
                "shared",
                1,
                1_000,
                100,
            ),
            (
                "codex",
                "2026-08-02 09:00:00",
                "2026-08-02",
                "codex-2",
                1,
                3_000,
                300,
            ),
            (
                "claude",
                "2026-07-01 10:00:00",
                "2026-07-01",
                "shared",
                1,
                2_000,
                200,
            ),
            (
                "claude",
                "2026-07-01 10:05:00",
                "2026-07-01",
                "shared",
                2,
                500,
                50,
            ),
        ] {
            conn.execute(
                "INSERT INTO usage_entries (
                    assistant_type, timestamp, date, session_id, session_name, cwd, turn_no, model,
                    tokens_input, tokens_output, tokens_cache_read, tokens_total,
                    delta_input, delta_output, delta_cache_read, delta_total
                ) VALUES (?1, ?2, ?3, ?4, ?4, '/cwd/all', ?5, 'gpt-5.5', ?6, ?7, 0, ?6 + ?7, ?6, ?7, 0, ?6 + ?7)",
                rusqlite::params![assistant, timestamp, date, session_id, turn_no, input, output],
            )
            .unwrap();
        }
        drop(conn);

        let yearly = |assistant: &str| {
            super::get_yearly_details(Path((assistant.to_string(), "2026".to_string())))
        };
        let (status, all) = response_json(yearly("ALL").await.into_response()).await;
        assert_eq!(status, 200);
        let (_, codex) = response_json(yearly("codex").await.into_response()).await;
        let (_, claude) = response_json(yearly("claude").await.into_response()).await;

        let summary_u64 =
            |report: &serde_json::Value, key: &str| report["summary"][key].as_u64().unwrap();
        assert_eq!(summary_u64(&all, "total_sessions"), 3);
        for key in ["total_tokens", "total_input_tokens", "total_output_tokens"] {
            assert_eq!(
                summary_u64(&all, key),
                summary_u64(&codex, key) + summary_u64(&claude, key),
                "{key}"
            );
        }
        let all_cost = all["summary"]["total_cost_usd"].as_f64().unwrap();
        let separate_cost = codex["summary"]["total_cost_usd"].as_f64().unwrap()
            + claude["summary"]["total_cost_usd"].as_f64().unwrap();
        assert!((all_cost - separate_cost).abs() < 1e-9);

        let agents = all["agent_breakdown"].as_object().unwrap();
        assert_eq!(agents.len(), 2);
        assert_eq!(agents["claude"]["total_tokens"].as_u64(), Some(2_750));
        assert_eq!(agents["codex"]["total_sessions"].as_u64(), Some(2));

        let months = all["monthly_breakdown"].as_array().unwrap();
        assert_eq!(months.len(), 2);
        assert_eq!(months[0]["month"], "2026-07");
        assert_eq!(
            months[0]["agents"]["codex"]["total_tokens"].as_u64(),
            Some(1_100)
        );
        assert_eq!(
            months[0]["agents"]["claude"]["total_tokens"].as_u64(),
            Some(2_750)
        );
        assert_eq!(months[1]["agents"].as_object().unwrap().len(), 1);

        let (status, monthly) = response_json(
            super::get_monthly_details(Path(("all".to_string(), "2026-07".to_string())))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(
            monthly["daily_breakdown"][0]["agents"]
                .as_object()
                .unwrap()
                .len(),
            2
        );

        let (status, dates) = response_json(
            super::get_available_dates(Path("all".to_string()))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(
            dates["dates"],
            serde_json::json!(["2026-08-02", "2026-07-01"])
        );

        let (status, daily) = response_json(
            super::get_usage_details(Path(("all".to_string(), "2026-07-01".to_string())))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, 200);
        let mut daily_agents = daily["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|session| session["assistant_type"].as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        daily_agents.sort();
        assert_eq!(daily_agents, vec!["claude", "codex"]);

        // 需要明確單一來源的 API 仍拒絕合併範圍。
        let export = super::export_usage_day(Path(("all".to_string(), "2026-07-01".to_string())))
            .await
            .into_response();
        assert_eq!(export.status().as_u16(), 400);
        let details = super::get_session_details(
            Path(("all".to_string(), "shared".to_string())),
            Query(Default::default()),
        )
        .await
        .into_response();
        assert_eq!(details.status().as_u16(), 400);

        match previous_insights_dir {
            Some(value) => env::set_var("INSIGHTS_DIR", value),
            None => env::remove_var("INSIGHTS_DIR"),
        }
        let _ = fs::remove_dir_all(&temp_dir);
    }
}
