use serde::Serialize;
use std::collections::HashMap;
use crate::db::UsageEntry;

pub mod daily;
pub mod monthly;
pub mod yearly;
pub mod codex;
pub mod misc;

pub use daily::*;
pub use monthly::*;
pub use yearly::*;
pub use codex::*;
pub use misc::*;

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
    use super::*;
    use crate::db;
    use std::env;
    use std::fs;
    use axum::{extract::Path, response::IntoResponse};

    #[tokio::test]
    async fn test_yearly_handlers() {
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

    #[tokio::test]
    async fn test_codex_auth_configs_copy() {
        let temp_dir = std::path::PathBuf::from("temp_test_codex_auth");
        if temp_dir.exists() {
            let _ = fs::remove_dir_all(&temp_dir);
        }
        fs::create_dir_all(&temp_dir).unwrap();
        env::set_var("CODEX_DIR", temp_dir.to_str().unwrap());

        // 1. Initially, auth.json does not exist. We call get_codex_auth_configs.
        // It shouldn't copy anything, auth/ should be created but empty.
        let _res = get_codex_auth_configs(Path("codex".to_string())).await;
        let auth_dir = temp_dir.join("auth");
        assert!(auth_dir.exists());
        let count = fs::read_dir(&auth_dir).unwrap().count();
        assert_eq!(count, 0);

        // 2. Now write a fake auth.json (representing active_auth_file)
        let active_auth_file = temp_dir.join("auth.json");
        fs::write(&active_auth_file, b"test-credentials").unwrap();

        // 3. Since auth/ is empty and auth.json exists, calling get_codex_auth_configs
        // should copy auth.json into auth/auth.json
        let _res2 = get_codex_auth_configs(Path("codex".to_string())).await;
        let dest_auth_file = auth_dir.join("auth.json");
        assert!(dest_auth_file.exists());
        let copied_content = fs::read_to_string(&dest_auth_file).unwrap();
        assert_eq!(copied_content, "test-credentials");

        // 4. Overwrite copied file with different content to simulate an existing non-empty directory.
        fs::write(&dest_auth_file, b"different-credentials").unwrap();

        // 5. Calling get_codex_auth_configs again should NOT overwrite it because the directory is not empty.
        let _res3 = get_codex_auth_configs(Path("codex".to_string())).await;
        let copied_content_after = fs::read_to_string(&dest_auth_file).unwrap();
        assert_eq!(copied_content_after, "different-credentials");

        // Cleanup
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_switch_codex_auth_expiration() {
        let temp_dir = std::path::PathBuf::from("temp_test_codex_auth_exp");
        if temp_dir.exists() {
            let _ = fs::remove_dir_all(&temp_dir);
        }
        let auth_dir = temp_dir.join("auth");
        fs::create_dir_all(&auth_dir).unwrap();
        env::set_var("CODEX_DIR", temp_dir.to_str().unwrap());

        // Write expired token JSON (exp: 1000, which is in 1970)
        let expired_json = r#"{
            "tokens": {
                "access_token": "header.eyJleHAiOjEwMDB9.signature"
            }
        }"#;
        fs::write(auth_dir.join("expired.json"), expired_json).unwrap();

        // Write valid token JSON (exp: 2783039221, which is in 2058)
        let valid_json = r#"{
            "tokens": {
                "access_token": "header.eyJleHAiOjI3ODMwMzkyMjF9.signature"
            }
        }"#;
        fs::write(auth_dir.join("valid.json"), valid_json).unwrap();

        // 1. Try to switch to expired credential
        let req_expired = axum::Json(SwitchAuthRequest {
            name: "expired.json".to_string(),
        });
        let res_expired = switch_codex_auth(Path("codex".to_string()), req_expired)
            .await
            .into_response();
        assert_eq!(
            res_expired.status(),
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        );

        // Assert that the destination auth.json was NOT created (switch blocked)
        let active_auth_file = temp_dir.join("auth.json");
        assert!(!active_auth_file.exists());

        // 2. Try to switch to valid credential
        let req_valid = axum::Json(SwitchAuthRequest {
            name: "valid.json".to_string(),
        });
        let res_valid = switch_codex_auth(Path("codex".to_string()), req_valid)
            .await
            .into_response();
        assert_eq!(res_valid.status(), axum::http::StatusCode::OK);

        // Assert that the destination auth.json WAS created (switch successful)
        assert!(active_auth_file.exists());

        // Cleanup
        let _ = fs::remove_dir_all(&temp_dir);
    }
}
