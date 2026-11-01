use serde::Serialize;
use std::collections::HashMap;

use crate::{
    db,
    reporting::group_sessions,
    session_details::parse_session_timeline_file,
    session_files::{resolve_session_file_path, SessionFileResolutionContext},
    timeline::TimelineItem,
};

#[derive(Serialize)]
pub(crate) struct SessionSearchMatch {
    session_id: String,
    assistant_type: String,
    source_kind: String,
    source_dir_key: Option<String>,
    /// 與 `SessionIdentity` 同一組身分欄位：獨立 session `a__b`（無父層）
    /// 與 session `a` 的合成子 Agent `b`（合成 id 同為 `a__b`）必須能在
    /// 搜尋結果中被區分並單獨選取（comment 4403291365）。
    parent_session_id: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct SessionSearchResponse {
    matches: Vec<SessionSearchMatch>,
    unavailable_sessions: usize,
}

struct SearchableSession {
    session_id: String,
    assistant_type: String,
    transcript_path: Option<String>,
    source_kind: String,
    source_dir_key: Option<String>,
    parent_session_id: Option<String>,
    agent_nickname: Option<String>,
    model: Option<String>,
}

fn timeline_matches_user_prompt(timeline: &[TimelineItem], normalized_query: &str) -> bool {
    timeline.iter().any(|item| {
        matches!(
            item,
            TimelineItem::UserPrompt { prompt, .. }
                if prompt.to_lowercase().contains(normalized_query)
        )
    })
}

fn build_searchable_sessions(
    entries: &[db::UsageDayRecordWithAssistant],
) -> Vec<SearchableSession> {
    group_sessions(
        entries
            .iter()
            .map(|row| (&row.record.entry, row.assistant_type.as_str())),
    )
    .into_iter()
    .map(|(identity, group)| {
        let mut transcript_path = None;
        let mut agent_nickname = None;
        let mut model = None;
        for entry in group.entries {
            if transcript_path.is_none() {
                transcript_path = entry.transcript_path;
            }
            if entry.agent_nickname.is_some() {
                agent_nickname = entry.agent_nickname;
            }
            if entry.model.is_some() {
                model = entry.model;
            }
        }
        SearchableSession {
            session_id: identity.session_id,
            assistant_type: identity.assistant_type,
            transcript_path,
            source_kind: identity.source_kind,
            source_dir_key: identity.source_dir_key,
            // 身分鍵（SessionIdentity）已含父層，直接沿用同一組欄位，
            // 確保搜尋列與其他彙整看到完全相同的 session 身分。
            parent_session_id: identity.parent_session_id,
            agent_nickname,
            model,
        }
    })
    .collect()
}

pub(crate) fn search_user_prompts(
    assistant: &str,
    date: &str,
    normalized_query: &str,
) -> Result<SessionSearchResponse, String> {
    let conn = db::get_db_conn()?;
    let entries = db::get_usage_entries_by_date(&conn, date, assistant)?;
    let sessions = build_searchable_sessions(&entries);

    let mut matches = Vec::new();
    let mut unavailable_sessions = 0;
    for session in sessions {
        // 不在此以檔名字元集（is_safe_session_id）前置過濾 session id：匯入端
        // 接受的 id（如 Codex 子層 `vendor:child`）是合法身分、詳情頁開得了，
        // 前置過濾會讓同列被計入 unavailable 且 transcript 永不被文字搜尋
        // （comment 4433781428）。路徑安全完全沿用 resolve_session_file_path
        // 的來源專屬驗證——id 參與組路徑的來源（hook、copilot-app／CLI
        // subagent）在解析器內部保留 is_safe_session_id，DB 路徑來源則以
        // canonicalize＋根目錄 containment 防路徑穿越；解析失敗一律計入
        // unavailable_sessions，與詳情 API 同一條安全界線。

        let copilot_app_source_dir = if session.source_kind == "copilot-app" {
            match session.source_dir_key.as_deref() {
                Some(key) => db::get_usage_source_directory(
                    &conn,
                    &session.assistant_type,
                    &session.source_kind,
                    key,
                )?,
                None => None,
            }
        } else {
            None
        };
        if session.source_kind == "copilot-app" && copilot_app_source_dir.is_none() {
            unavailable_sessions += 1;
            continue;
        }

        let filepath = match resolve_session_file_path(
            &session.assistant_type,
            &session.session_id,
            session.transcript_path.as_deref(),
            &session.source_kind,
            SessionFileResolutionContext {
                copilot_app_source_dir: copilot_app_source_dir.as_deref(),
                parent_session_id: session.parent_session_id.as_deref(),
                agent_nickname: session.agent_nickname.as_deref(),
            },
        ) {
            Ok(path) if path.exists() => path,
            _ => {
                unavailable_sessions += 1;
                continue;
            }
        };
        let db_entries = HashMap::new();
        let (timeline, _) = match parse_session_timeline_file(
            &session.assistant_type,
            &session.source_kind,
            &filepath,
            &db_entries,
            session.agent_nickname.as_deref(),
            session.model.as_deref(),
        ) {
            Ok(result) => result,
            Err(_) => {
                unavailable_sessions += 1;
                continue;
            }
        };

        if timeline_matches_user_prompt(&timeline, normalized_query) {
            matches.push(SessionSearchMatch {
                session_id: session.session_id,
                assistant_type: session.assistant_type,
                source_kind: session.source_kind,
                source_dir_key: session.source_dir_key,
                parent_session_id: session.parent_session_id,
            });
        }
    }

    matches.sort_by(|a, b| {
        a.assistant_type
            .cmp(&b.assistant_type)
            .then_with(|| a.source_kind.cmp(&b.source_kind))
            .then_with(|| a.source_dir_key.cmp(&b.source_dir_key))
            .then_with(|| a.session_id.cmp(&b.session_id))
            // 合成子 Agent id 可能與獨立 session id 相同（`a__b`），補上
            // 父層決定性排序，避免同身分鍵前綴的兩列順序不穩。
            .then_with(|| a.parent_session_id.cmp(&b.parent_session_id))
    });
    Ok(SessionSearchResponse {
        matches,
        unavailable_sessions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{TokenStats, UsageEntry};
    use rusqlite::{params, Connection};
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn user_prompt(prompt: &str, turn_no: u32) -> TimelineItem {
        TimelineItem::UserPrompt {
            timestamp: "2026-07-16T00:00:00Z".to_string(),
            prompt: prompt.to_string(),
            context: None,
            turn_no,
        }
    }

    fn usage_record(source_dir_key: &str) -> db::UsageDayRecordWithAssistant {
        let tokens = TokenStats {
            input: 10,
            output: 5,
            cache_read: Some(0),
            cache_write: Some(0),
            cache_write_5m: None,
            cache_write_1h: None,
            reasoning: None,
            total: 15,
        };
        db::UsageDayRecordWithAssistant {
            record: db::UsageDayExportRecord {
                entry: UsageEntry {
                    timestamp: "2026-07-16T00:00:00Z".to_string(),
                    session_id: "shared-search-session".to_string(),
                    session_name: None,
                    transcript_path: None,
                    cwd: None,
                    version: None,
                    turn_no: 1,
                    model: Some("gpt-5".to_string()),
                    model_id: None,
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
                },
                import_source_id: None,
                usage_identity: None,
            },
            assistant_type: "copilot".to_string(),
            date: "2026-07-16".to_string(),
        }
    }

    #[test]
    fn user_prompt_search_checks_every_turn_case_insensitively() {
        let timeline = vec![
            user_prompt("先建立專案", 1),
            user_prompt("Please FIX the payment callback", 2),
        ];

        assert!(timeline_matches_user_prompt(&timeline, "fix the payment"));
    }

    #[test]
    fn user_prompt_search_ignores_non_user_timeline_content() {
        let timeline = vec![
            user_prompt("整理今日工作", 1),
            TimelineItem::SystemStatus {
                timestamp: "2026-07-16T00:00:01Z".to_string(),
                status_type: "session_start".to_string(),
                message: "secret keyword".to_string(),
            },
        ];

        assert!(!timeline_matches_user_prompt(&timeline, "secret keyword"));
    }

    #[test]
    fn prompt_search_keeps_same_session_id_source_directories_separate() {
        let entries = vec![usage_record("aa"), usage_record("bb")];

        let sessions = build_searchable_sessions(&entries);

        assert_eq!(sessions.len(), 2);
        for source_dir_key in ["aa", "bb"] {
            assert!(sessions.iter().any(|session| {
                session.assistant_type == "copilot"
                    && session.source_kind == "copilot-app"
                    && session.source_dir_key.as_deref() == Some(source_dir_key)
                    && session.session_id == "shared-search-session"
            }));
        }
    }

    #[test]
    fn prompt_search_keeps_colliding_standalone_and_synthetic_child_separate() {
        // 真實獨立 session `a__b`（無父層）與 session `a` 的子 Agent `b`
        // （合成 id 同為 `a__b`、parent_session_id = `a`）在搜尋結果中必須
        // 是兩個可單獨選取的身分（comment 4403291365）。
        let mut standalone = usage_record("aa");
        standalone.record.entry.session_id = "a__b".to_string();
        let mut child = usage_record("aa");
        child.record.entry.session_id = "a__b".to_string();
        child.record.entry.parent_session_id = Some("a".to_string());

        let sessions = build_searchable_sessions(&[standalone, child]);
        assert_eq!(sessions.len(), 2, "碰撞的兩列必須分開成兩個可搜尋 session");

        let matches: Vec<SessionSearchMatch> = sessions
            .into_iter()
            .map(|session| SessionSearchMatch {
                session_id: session.session_id,
                assistant_type: session.assistant_type,
                source_kind: session.source_kind,
                source_dir_key: session.source_dir_key,
                parent_session_id: session.parent_session_id,
            })
            .collect();
        let mut identity_tuples: Vec<(String, Option<String>)> = matches
            .iter()
            .map(|row| (row.session_id.clone(), row.parent_session_id.clone()))
            .collect();
        identity_tuples.sort();
        assert_eq!(
            identity_tuples,
            vec![
                ("a__b".to_string(), None),
                ("a__b".to_string(), Some("a".to_string())),
            ],
            "獨立 session 與合成子 Agent 必須是兩個不同的搜尋身分"
        );

        // 序列化契約：搜尋列必須原樣帶出 parent_session_id，前端才可用
        // 同一把 5 欄位身分鍵區分並單獨選取。
        for row in &matches {
            let value = serde_json::to_value(row).expect("搜尋列必須可序列化");
            let expected_parent = match row.parent_session_id.as_deref() {
                Some(parent) => serde_json::json!(parent),
                None => serde_json::Value::Null,
            };
            assert_eq!(
                value.get("parent_session_id"),
                Some(&expected_parent),
                "搜尋列序列化必須帶 parent_session_id（前端身分鍵契約）"
            );
        }
    }

    /// 寫入一支含 session_meta＋token_count＋USER 提示詞的 Codex rollout
    /// （cb3e0ff fixture 的事件形狀），提示詞供搜尋命中。
    fn write_codex_rollout(path: &std::path::Path, prompt: &str) {
        let meta = r#"{"timestamp":"2026-10-04T10:00:00.000Z","type":"session_meta","payload":{"id":"rollout","cwd":"/tmp/project","cli_version":"0.142.5","model":"gpt-5"}}"#;
        let tokens = r#"{"timestamp":"2026-10-04T10:00:01.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"output_tokens":10,"total_tokens":110},"last_token_usage":{"input_tokens":100,"output_tokens":10,"total_tokens":110}}}}"#;
        let user_message = format!(
            r#"{{"timestamp":"2026-10-04T10:00:02.000Z","type":"event_msg","payload":{{"type":"user_message","message":"{prompt}"}}}}"#
        );
        fs::write(path, format!("{meta}\n{tokens}\n{user_message}\n")).unwrap();
    }

    /// 以 cb3e0ff fixture 的欄位集合寫入一列 usage_entries。
    fn insert_usage_row(
        conn: &Connection,
        assistant: &str,
        source_kind: &str,
        session_id: &str,
        transcript_path: &str,
        parent_session_id: Option<&str>,
    ) {
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, timestamp, date, transcript_path,
                session_id, turn_no, session_name, model,
                tokens_input, tokens_output, tokens_total,
                delta_input, delta_output, delta_total,
                parent_session_id
            ) VALUES (
                ?, ?, '2026-10-04 10:00:00', '2026-10-04', ?,
                ?, 1, 'Session Name', 'gpt-5',
                100, 10, 110,
                100, 10, 110,
                ?
            )",
            params![
                assistant,
                source_kind,
                transcript_path,
                session_id,
                parent_session_id
            ],
        )
        .unwrap();
    }

    /// 回歸測試（comment 4433781428）：cb3e0ff 移除 handler 層檔名檢查後，
    /// 匯入端合法 id（如 Codex 子層 `vendor:child`）詳情開得了，搜尋卻在
    /// 讀檔前以 is_safe_session_id 誤殺、只計 unavailable，transcript 永不被
    /// 文字搜尋。搜尋必須沿用 resolve_session_file_path 的來源專屬驗證：
    /// transcript 含搜尋文字的 `vendor:child` 與 `child`（父層
    /// `vendor:parent`）都要能被搜到並在結果中選取。
    #[test]
    fn prompt_search_finds_imported_codex_session_ids_outside_filename_charset() {
        if crate::reporting::is_isolated_insights_test_child() {
            prompt_search_imported_codex_session_ids_body();
            return;
        }
        let temp_dir = std::env::temp_dir().join(format!(
            "tui-session-search-imported-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let output = crate::reporting::spawn_isolated_insights_test(
            "session_search::tests::prompt_search_finds_imported_codex_session_ids_outside_filename_charset",
            &temp_dir,
        );
        assert!(
            output.status.success(),
            "子行程搜尋測試失敗:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    fn prompt_search_imported_codex_session_ids_body() {
        // 先建立資料庫檔案，避免 get_db_conn 將舊位置的資料庫搬進測試目錄。
        let conn = Connection::open(crate::db::get_insights_dir().join("token_usage_insights.db"))
            .unwrap();
        crate::db::init_db(&conn).unwrap();

        let codex_root = crate::db::get_insights_dir().join("codex-root");
        std::env::set_var("CODEX_DIR", &codex_root);
        let rollout_dir = codex_root
            .join("sessions")
            .join("2026")
            .join("10")
            .join("04");
        fs::create_dir_all(&rollout_dir).unwrap();

        for (session_id, parent, rollout_name) in [
            ("child", Some("vendor:parent"), "rollout-child.jsonl"),
            ("vendor:child", None, "rollout-vendor-child.jsonl"),
        ] {
            let rollout = rollout_dir.join(rollout_name);
            write_codex_rollout(&rollout, "needle-prompt 請修 build");
            insert_usage_row(
                &conn,
                "codex",
                "codex-cli",
                session_id,
                rollout.to_str().unwrap(),
                parent,
            );
        }
        drop(conn);

        let response = super::search_user_prompts("codex", "2026-10-04", "needle-prompt")
            .expect("搜尋必須成功");
        assert_eq!(
            response.unavailable_sessions, 0,
            "匯入端合法 id 不得被計入無法搜尋"
        );
        let mut found: Vec<(String, Option<String>)> = response
            .matches
            .iter()
            .map(|row| (row.session_id.clone(), row.parent_session_id.clone()))
            .collect();
        found.sort();
        assert_eq!(
            found,
            vec![
                ("child".to_string(), Some("vendor:parent".to_string())),
                ("vendor:child".to_string(), None),
            ],
            "transcript 含搜尋文字的 vendor:child／child 必須出現在搜尋結果（可被選取）"
        );
    }

    /// 安全界線回歸（comment 4433781428 的修正不得鬆動 resolver 防線）：
    /// 移除 id 前置過濾後，路徑穿越 id（`foo/../../bar`、`../../etc/passwd`）
    /// 與指向來源 root 之外的 transcript_path 都必須被 resolve_session_file_path
    /// 攔下：計入 unavailable、不得命中、不得 panic。root 之外的兩支檔案刻意
    /// 各含搜尋詞——若任何路徑繞過 resolver 直接讀檔（或 id 被用來組路徑逃出
    /// root），它們就會被解析成 UserPrompt 命中，本測試即 RED。
    #[test]
    fn prompt_search_rejects_traversal_ids_and_out_of_root_transcripts() {
        if crate::reporting::is_isolated_insights_test_child() {
            prompt_search_traversal_safety_body();
            return;
        }
        let temp_dir = std::env::temp_dir().join(format!(
            "tui-session-search-traversal-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let output = crate::reporting::spawn_isolated_insights_test(
            "session_search::tests::prompt_search_rejects_traversal_ids_and_out_of_root_transcripts",
            &temp_dir,
        );
        assert!(
            output.status.success(),
            "子行程搜尋測試失敗:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    fn prompt_search_traversal_safety_body() {
        // 先建立資料庫檔案，避免 get_db_conn 將舊位置的資料庫搬進測試目錄。
        let conn = Connection::open(crate::db::get_insights_dir().join("token_usage_insights.db"))
            .unwrap();
        crate::db::init_db(&conn).unwrap();

        let insights_dir = crate::db::get_insights_dir();
        let codex_root = insights_dir.join("codex-root");
        let copilot_root = insights_dir.join("copilot-root");
        std::env::set_var("CODEX_DIR", &codex_root);
        std::env::set_var("COPILOT_DIR", &copilot_root);
        fs::create_dir_all(&codex_root).unwrap();
        fs::create_dir_all(&copilot_root).unwrap();

        // Codex root 之外的 rollout：含搜尋詞，任何未驗證讀檔都會命中。
        let outside_dir = insights_dir.join("outside");
        fs::create_dir_all(&outside_dir).unwrap();
        let outside_rollout = outside_dir.join("escape-rollout.jsonl");
        write_codex_rollout(&outside_rollout, "escape-needle codex");

        // hook 來源以 session id 組路徑：`../../etc/passwd` 會指到
        // <insights>/etc/passwd/events.jsonl（copilot root 之外），
        // 同樣刻意含搜尋詞。
        let hook_escape_dir = insights_dir.join("etc").join("passwd");
        fs::create_dir_all(&hook_escape_dir).unwrap();
        let outside_events = hook_escape_dir.join("events.jsonl");
        fs::write(
            &outside_events,
            r#"{"timestamp":"2026-10-04T11:00:00.000Z","type":"user.message","payload":{"content":"escape-needle hook"}}"#,
        )
        .unwrap();

        insert_usage_row(
            &conn,
            "codex",
            "codex-cli",
            "foo/../../bar",
            outside_rollout.to_str().unwrap(),
            None,
        );
        insert_usage_row(
            &conn,
            "copilot",
            "copilot-cli",
            "../../etc/passwd",
            outside_events.to_str().unwrap(),
            None,
        );
        drop(conn);

        let response = super::search_user_prompts("all", "2026-10-04", "escape-needle")
            .expect("穿越 id 不得讓搜尋 panic");
        assert_eq!(
            response.unavailable_sessions, 2,
            "穿越 id／root 外 transcript 必須被計入無法搜尋"
        );
        assert!(
            response.matches.is_empty(),
            "root 外檔案（各含搜尋詞）不得被讀入或命中"
        );
    }
}
