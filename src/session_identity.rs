use crate::db::UsageEntry;

/// Complete identity for a session across assistants and local data sources.
///
/// `parent_session_id` participates in the identity because Copilot App / CLI
/// sub-agent calls are stored under a synthetic `<parent>__<agent>` session id
/// with `parent_session_id = <parent>`. A standalone session whose id happens
/// to contain `__` (e.g. `a__b`) would otherwise collide with session `a`'s
/// sub-agent `b` and merge with it in every summary.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct SessionIdentity {
    pub assistant_type: String,
    pub source_kind: String,
    pub source_dir_key: Option<String>,
    pub session_id: String,
    pub parent_session_id: Option<String>,
}

impl SessionIdentity {
    pub(crate) fn from_entry(assistant_type: &str, entry: &UsageEntry) -> Self {
        Self {
            assistant_type: assistant_type.to_string(),
            source_kind: entry
                .source_kind
                .clone()
                .unwrap_or_else(|| "legacy".to_string()),
            source_dir_key: entry.source_dir_key.clone(),
            session_id: entry.session_id.clone(),
            parent_session_id: crate::db::normalize_parent_session_id(
                entry.parent_session_id.clone(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry_with_parent(parent: Option<&str>) -> UsageEntry {
        UsageEntry {
            timestamp: "2026-07-23T12:00:00Z".to_string(),
            session_id: "s".to_string(),
            session_name: None,
            transcript_path: None,
            cwd: None,
            version: None,
            turn_no: 1,
            model: None,
            model_id: None,
            tokens: None,
            delta_tokens: None,
            context: None,
            cost: None,
            source_kind: None,
            source_dir_key: None,
            parent_session_id: parent.map(str::to_string),
            agent_nickname: None,
            agent_role: None,
            reasoning_effort: None,
        }
    }

    #[test]
    fn session_identity_groups_empty_parent_with_parentless() {
        // (thread PRRT_kwDOTOgsec6yUKGs) The frontend `sessionIdentityKey`
        // collapses `Some("")` and `None` into one key; if the backend derived
        // identity keeps them apart, one session splits into two groups and
        // the two rows silently overwrite each other in the session table.
        let empty = SessionIdentity::from_entry("codex", &entry_with_parent(Some("")));
        let blank = SessionIdentity::from_entry("codex", &entry_with_parent(Some("  ")));
        let parentless = SessionIdentity::from_entry("codex", &entry_with_parent(None));
        let child = SessionIdentity::from_entry("codex", &entry_with_parent(Some("parent")));

        assert_eq!(
            empty, parentless,
            "Some(\"\") 必須與 None 分組為同一 session 身分"
        );
        assert_eq!(
            blank, parentless,
            "全空白父層必須與 None 分組為同一 session 身分"
        );
        assert_eq!(empty.parent_session_id, None);
        assert_ne!(empty, child, "真實父層的 session 仍須獨立分組");
    }
}
