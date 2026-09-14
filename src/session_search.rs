use serde::Serialize;
use std::collections::HashMap;

use crate::{
    db,
    reporting::group_sessions,
    session_details::parse_session_timeline_file,
    session_files::{is_safe_session_id, resolve_session_file_path, SessionFileResolutionContext},
    timeline::TimelineItem,
};

#[derive(Serialize)]
pub(crate) struct SessionSearchMatch {
    session_id: String,
    assistant_type: String,
    source_kind: String,
    source_dir_key: Option<String>,
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
        let mut parent_session_id = None;
        let mut agent_nickname = None;
        let mut model = None;
        for entry in group.entries {
            if transcript_path.is_none() {
                transcript_path = entry.transcript_path;
            }
            if entry.parent_session_id.is_some() {
                parent_session_id = entry.parent_session_id;
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
            parent_session_id,
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
        if !is_safe_session_id(&session.session_id) {
            unavailable_sessions += 1;
            continue;
        }

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
            });
        }
    }

    matches.sort_by(|a, b| {
        a.assistant_type
            .cmp(&b.assistant_type)
            .then_with(|| a.source_kind.cmp(&b.source_kind))
            .then_with(|| a.source_dir_key.cmp(&b.source_dir_key))
            .then_with(|| a.session_id.cmp(&b.session_id))
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
}
