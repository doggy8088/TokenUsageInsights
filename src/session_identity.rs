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
            parent_session_id: entry.parent_session_id.clone(),
        }
    }
}
