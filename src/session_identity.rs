use crate::db::UsageEntry;

/// Complete identity for a session across assistants and local data sources.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct SessionIdentity {
    pub assistant_type: String,
    pub source_kind: String,
    pub source_dir_key: Option<String>,
    pub session_id: String,
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
        }
    }
}
