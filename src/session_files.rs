use axum::http::StatusCode;
use std::path::{Path as StdPath, PathBuf};

use crate::db;

pub(crate) fn is_safe_session_id(session_id: &str) -> bool {
    if session_id.is_empty() || session_id.len() > 128 {
        return false;
    }

    if session_id == "." || session_id == ".." {
        return false;
    }

    session_id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.')
}

fn resolve_claude_transcript_path(
    claude_dir: &StdPath,
    session_id: &str,
    transcript_path_db: &str,
) -> Result<PathBuf, String> {
    let mut path = PathBuf::from(transcript_path_db);
    if path.is_relative() {
        path = claude_dir.join(path);
    }

    if !path.exists() {
        return Err("找不到該會話的本地日誌檔案。".to_string());
    }

    let claude_root = claude_dir
        .canonicalize()
        .map_err(|_| "無法存取 Claude Code 根目錄。".to_string())?;
    let canonical_path = path
        .canonicalize()
        .map_err(|_| "無法解析會話日誌路徑。".to_string())?;

    if !canonical_path.starts_with(claude_root) {
        return Err("會話日誌路徑不在預期目錄內。".to_string());
    }

    let file_name = canonical_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if !file_name.contains(session_id) {
        return Err("會話日誌路徑與 session id 不一致。".to_string());
    }

    Ok(canonical_path)
}

fn resolve_codex_transcript_path(
    codex_dir: &StdPath,
    transcript_path_db: &str,
) -> Result<PathBuf, String> {
    let mut path = PathBuf::from(transcript_path_db);
    if path.is_relative() {
        path = codex_dir.join(path);
    }

    if !path.exists() {
        return Err("找不到該 Codex 會話的本地日誌檔案。".to_string());
    }

    let codex_root = codex_dir
        .canonicalize()
        .map_err(|_| "無法存取 Codex 根目錄。".to_string())?;
    let canonical_path = path
        .canonicalize()
        .map_err(|_| "無法解析 Codex 會話日誌路徑。".to_string())?;

    if !canonical_path.starts_with(codex_root) {
        return Err("Codex 會話日誌路徑不在預期目錄內。".to_string());
    }

    Ok(canonical_path)
}

fn resolve_pi_family_transcript_path(
    base_dir: &StdPath,
    assistant_label: &str,
    transcript_path_db: &str,
) -> Result<PathBuf, String> {
    let mut path = PathBuf::from(transcript_path_db);
    if path.is_relative() {
        path = base_dir.join(path);
    }

    if !path.exists() {
        return Err(format!(
            "找不到該 {assistant_label} session 的本地日誌檔案。"
        ));
    }

    let base_root = base_dir
        .canonicalize()
        .map_err(|_| format!("無法存取 {assistant_label} 根目錄。"))?;
    let canonical_path = path
        .canonicalize()
        .map_err(|_| format!("無法解析 {assistant_label} session 日誌路徑。"))?;

    if !canonical_path.starts_with(&base_root) {
        return Err(format!(
            "{assistant_label} session 日誌路徑不在預期目錄內。"
        ));
    }
    if canonical_path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
        return Err(format!("{assistant_label} session 日誌格式不受支援。"));
    }

    Ok(canonical_path)
}

fn resolve_cursor_transcript_path(
    cursor_dir: &StdPath,
    session_id: &str,
    transcript_path_db: &str,
) -> Result<PathBuf, String> {
    let mut path = PathBuf::from(transcript_path_db);
    if path.is_relative() {
        path = cursor_dir.join(path);
    }

    if !path.exists() {
        return Err("找不到該 Cursor 會話的本地日誌檔案。".to_string());
    }

    let cursor_root = cursor_dir
        .canonicalize()
        .map_err(|_| "無法存取 Cursor 根目錄。".to_string())?;
    let canonical_path = path
        .canonicalize()
        .map_err(|_| "無法解析 Cursor 會話日誌路徑。".to_string())?;

    if !canonical_path.starts_with(cursor_root) {
        return Err("Cursor 會話日誌路徑不在預期目錄內。".to_string());
    }

    let file_name = canonical_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if !file_name.contains(session_id) {
        return Err("會話日誌路徑與 session id 不一致。".to_string());
    }

    Ok(canonical_path)
}

fn resolve_grok_transcript_path(
    grok_dir: &StdPath,
    session_id: &str,
    transcript_path_db: &str,
) -> Result<PathBuf, String> {
    let mut path = PathBuf::from(transcript_path_db);
    if path.is_relative() {
        path = grok_dir.join(path);
    }

    if !path.exists() {
        return Err("找不到該 Grok Build session 的本地日誌檔案。".to_string());
    }

    let grok_root = grok_dir
        .canonicalize()
        .map_err(|_| "無法存取 Grok Build 根目錄。".to_string())?;
    let canonical_path = path
        .canonicalize()
        .map_err(|_| "無法解析 Grok Build session 日誌路徑。".to_string())?;

    if !canonical_path.starts_with(&grok_root) {
        return Err("Grok Build session 日誌路徑不在預期目錄內。".to_string());
    }
    if canonical_path.file_name().and_then(|name| name.to_str()) != Some("updates.jsonl") {
        return Err("Grok Build session 日誌格式不受支援。".to_string());
    }
    if canonical_path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        != Some(session_id)
    {
        return Err("Grok Build session 日誌路徑與 session id 不一致。".to_string());
    }

    Ok(canonical_path)
}

fn resolve_vscode_transcript_path(transcript_path_db: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(transcript_path_db);
    if !path.exists() {
        return Err("找不到該 VS Code Copilot 聊天檔案。".to_string());
    }

    let canonical_path = path
        .canonicalize()
        .map_err(|_| "無法解析 VS Code Copilot 聊天檔案路徑。".to_string())?;
    let is_allowed = crate::vscode::discover_workspace_storage_roots()
        .into_iter()
        .filter_map(|root| root.canonicalize().ok())
        .any(|root| {
            canonical_path.starts_with(&root)
                && canonical_path
                    .parent()
                    .and_then(|parent| parent.file_name())
                    .and_then(|name| name.to_str())
                    == Some("chatSessions")
        });
    if !is_allowed {
        return Err("VS Code Copilot 聊天檔案不在允許的 workspaceStorage 目錄內。".to_string());
    }

    let extension = canonical_path.extension().and_then(|value| value.to_str());
    if !matches!(extension, Some("json") | Some("jsonl")) {
        return Err("VS Code Copilot 聊天檔案格式不受支援。".to_string());
    }
    Ok(canonical_path)
}

/// Resolve the `events.jsonl` path for a Copilot App session drawer request.
///
/// `events_session_id` is the *parent* (original main) session id when the
/// request is for a subagent synthetic session (`<main>__<agent_id>`), or the
/// session id itself for a main agent request. Callers MUST obtain this from
/// the database `parent_session_id` column rather than splitting the synthetic
/// id, so a tampered id cannot escape the session-state root.
///
/// `agent_nickname` only distinguishes the missing-directory message for a
/// subagent request; it does not affect path construction or validation.
///
/// Security: the canonicalized path must remain within the
/// `<copilot_app_dir>/session-state` root. Any traversal attempt (synthetic id
/// with `..`, symlinks pointing outside, ...) is rejected with `file_missing`.
pub(crate) fn resolve_copilot_app_events_path(
    copilot_app_dir: &StdPath,
    events_session_id: &str,
    agent_nickname: Option<&str>,
) -> Result<PathBuf, SessionFileError> {
    resolve_copilot_events_path(
        copilot_app_dir,
        events_session_id,
        CopilotEventsSource::App {
            is_subagent: agent_nickname.is_some(),
        },
    )
}

/// Resolve the `events.jsonl` path for a Copilot CLI subagent drawer request.
///
/// CLI subagent rows use a synthetic session id (`<parent_session_id>__<agent_id>`)
/// and share the parent's `events.jsonl` under
/// `<copilot_dir>/session-state/<parent_session_id>/events.jsonl`. The caller
/// MUST pass the database-sourced `parent_session_id` (never a string-split
/// synthetic id) so a tampered id cannot escape the session-state root.
///
/// Mirrors [`resolve_copilot_app_events_path`] security checks but against the
/// Copilot CLI directory ([`db::get_copilot_dir`]).
pub(crate) fn resolve_copilot_cli_subagent_events_path(
    copilot_dir: &StdPath,
    parent_session_id: &str,
) -> Result<PathBuf, SessionFileError> {
    resolve_copilot_events_path(
        copilot_dir,
        parent_session_id,
        CopilotEventsSource::CliSubagent,
    )
}

#[derive(Clone, Copy)]
enum CopilotEventsSource {
    App { is_subagent: bool },
    CliSubagent,
}

impl CopilotEventsSource {
    fn invalid_id_message(self) -> &'static str {
        match self {
            Self::App { .. } => "Copilot App session id 格式不正確，無法定位 events.jsonl。",
            Self::CliSubagent => {
                "Copilot CLI subagent 的 parent session id 格式不正確，無法定位 events.jsonl。"
            }
        }
    }

    fn missing_message(self, session_dir_exists: bool) -> &'static str {
        match (self, session_dir_exists) {
            (Self::App { .. }, true) => {
                "找不到 Copilot App session 的 events.jsonl（session 目錄存在但尚未產生事件檔）。"
            }
            (Self::App { is_subagent: true }, false) => {
                "找不到 Copilot App session 的 events.jsonl（subagent 對應的主 session 目錄不存在）。"
            }
            (Self::App { .. }, false) => "找不到 Copilot App session 的 events.jsonl。",
            (Self::CliSubagent, true) => {
                "找不到 Copilot CLI subagent 對應的主 session events.jsonl（主 session 目錄存在但尚未產生事件檔）。"
            }
            (Self::CliSubagent, false) => {
                "找不到 Copilot CLI subagent 對應的主 session events.jsonl（subagent 對應的主 session 目錄不存在）。"
            }
        }
    }

    fn root_error_message(self) -> &'static str {
        match self {
            Self::App { .. } => "無法存取 Copilot App session-state 根目錄。",
            Self::CliSubagent => "無法存取 Copilot CLI session-state 根目錄。",
        }
    }

    fn path_error_message(self) -> &'static str {
        match self {
            Self::App { .. } => "無法解析 Copilot App events.jsonl 路徑。",
            Self::CliSubagent => "無法解析 Copilot CLI subagent events.jsonl 路徑。",
        }
    }

    fn outside_root_message(self) -> &'static str {
        match self {
            Self::App { .. } => "Copilot App events.jsonl 路徑不在允許的 session-state 目錄內。",
            Self::CliSubagent => {
                "Copilot CLI subagent events.jsonl 路徑不在允許的 session-state 目錄內。"
            }
        }
    }

    fn parent_mismatch_message(self) -> &'static str {
        match self {
            Self::App { .. } => "Copilot App events.jsonl 路徑與 session id 不一致。",
            Self::CliSubagent => {
                "Copilot CLI subagent events.jsonl 路徑與 parent session id 不一致。"
            }
        }
    }

    fn filename_mismatch_message(self) -> &'static str {
        match self {
            Self::App { .. } => "Copilot App session 路徑未指向 events.jsonl。",
            Self::CliSubagent => "Copilot CLI subagent 路徑未指向 events.jsonl。",
        }
    }
}

fn resolve_copilot_events_path(
    copilot_dir: &StdPath,
    session_id: &str,
    source: CopilotEventsSource,
) -> Result<PathBuf, SessionFileError> {
    if !is_safe_session_id(session_id) {
        return Err(SessionFileError::with_reason(
            StatusCode::NOT_FOUND,
            source.invalid_id_message(),
            SessionFileReason::FileMissing,
        ));
    }

    let session_state_root = copilot_dir.join("session-state");
    let session_dir = session_state_root.join(session_id);
    let events_path = session_dir.join("events.jsonl");
    if !events_path.exists() {
        let session_dir_exists = session_dir.exists();
        return Err(SessionFileError::with_reason(
            StatusCode::NOT_FOUND,
            source.missing_message(session_dir_exists),
            if session_dir_exists {
                SessionFileReason::NoEventsYet
            } else {
                SessionFileReason::FileMissing
            },
        ));
    }

    let root_canonical = session_state_root.canonicalize().map_err(|_| {
        SessionFileError::with_reason(
            StatusCode::NOT_FOUND,
            source.root_error_message(),
            SessionFileReason::FileMissing,
        )
    })?;
    let canonical_path = events_path.canonicalize().map_err(|_| {
        SessionFileError::with_reason(
            StatusCode::NOT_FOUND,
            source.path_error_message(),
            SessionFileReason::FileMissing,
        )
    })?;

    if !canonical_path.starts_with(&root_canonical) {
        return Err(SessionFileError::with_reason(
            StatusCode::NOT_FOUND,
            source.outside_root_message(),
            SessionFileReason::FileMissing,
        ));
    }
    let parent_name = canonical_path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str());
    if parent_name != Some(session_id) {
        return Err(SessionFileError::with_reason(
            StatusCode::NOT_FOUND,
            source.parent_mismatch_message(),
            SessionFileReason::FileMissing,
        ));
    }
    if canonical_path.file_name().and_then(|name| name.to_str()) != Some("events.jsonl") {
        return Err(SessionFileError::with_reason(
            StatusCode::NOT_FOUND,
            source.filename_mismatch_message(),
            SessionFileReason::FileMissing,
        ));
    }

    Ok(canonical_path)
}

/// Error locating a session transcript, optionally carrying a stable reason
/// code used by the frontend to choose an empty-state message.
#[derive(Debug)]
pub(crate) struct SessionFileError {
    pub status: StatusCode,
    pub error: String,
    pub reason: Option<SessionFileReason>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SessionFileReason {
    NoEventsYet,
    FileMissing,
    ContentUnavailable,
}

impl SessionFileReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::NoEventsYet => "no_events_yet",
            Self::FileMissing => "file_missing",
            Self::ContentUnavailable => "content_unavailable",
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SessionFileResolutionContext<'a> {
    pub copilot_app_source_dir: Option<&'a StdPath>,
    pub parent_session_id: Option<&'a str>,
    pub agent_nickname: Option<&'a str>,
}

impl SessionFileError {
    fn new(status: StatusCode, error: impl Into<String>) -> Self {
        Self {
            status,
            error: error.into(),
            reason: None,
        }
    }

    fn with_reason(
        status: StatusCode,
        error: impl Into<String>,
        reason: SessionFileReason,
    ) -> Self {
        Self {
            status,
            error: error.into(),
            reason: Some(reason),
        }
    }
}

pub(crate) fn resolve_session_file_path(
    assistant: &str,
    session_id: &str,
    transcript_path_db: Option<&str>,
    source_kind: &str,
    context: SessionFileResolutionContext<'_>,
) -> Result<PathBuf, SessionFileError> {
    match assistant {
        "antigravity" => Ok(db::get_antigravity_dir()
            .join("brain")
            .join(session_id)
            .join(".system_generated/logs/transcript_full.jsonl")),
        "copilot" if source_kind == crate::vscode::SOURCE_KIND => {
            let path = transcript_path_db.ok_or_else(|| {
                SessionFileError::new(
                    StatusCode::NOT_FOUND,
                    "找不到 VS Code Copilot 聊天檔案路徑。",
                )
            })?;
            resolve_vscode_transcript_path(path)
                .map_err(|error| SessionFileError::new(StatusCode::BAD_REQUEST, error))
        }
        "copilot" if source_kind == "copilot-app" => {
            let source_dir = context.copilot_app_source_dir.ok_or_else(|| {
                SessionFileError::with_reason(
                    StatusCode::NOT_FOUND,
                    "找不到 Copilot App session 對應的已登錄來源目錄。",
                    SessionFileReason::FileMissing,
                )
            })?;
            resolve_copilot_app_events_path(
                source_dir,
                context.parent_session_id.unwrap_or(session_id),
                context.agent_nickname,
            )
        }
        "copilot" if source_kind == "copilot-cli" && context.parent_session_id.is_some() => {
            // CLI subagent synthetic session: locate the shared events.jsonl
            // under the parent session's directory, not the synthetic id's.
            let Some(parent_session_id) = context.parent_session_id else {
                return Err(SessionFileError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Copilot CLI subagent 缺少 parent session id。",
                ));
            };
            resolve_copilot_cli_subagent_events_path(&db::get_copilot_dir(), parent_session_id)
        }
        "copilot" => {
            let copilot_dir = db::get_copilot_dir();
            let events_path = copilot_dir
                .join("session-state")
                .join(session_id)
                .join("events.jsonl");
            if events_path.exists() {
                Ok(events_path)
            } else {
                Ok(copilot_dir
                    .join("session-state")
                    .join(format!("{session_id}.jsonl")))
            }
        }
        "codex" => {
            let path = transcript_path_db.ok_or_else(|| {
                SessionFileError::new(
                    StatusCode::NOT_FOUND,
                    "找不到 Codex 會話日誌檔案路徑。".to_string(),
                )
            })?;
            resolve_codex_transcript_path(&db::get_codex_dir(), path)
                .map_err(|error| SessionFileError::new(StatusCode::BAD_REQUEST, error))
        }
        "claude" => {
            let path = transcript_path_db.ok_or_else(|| {
                SessionFileError::new(
                    StatusCode::NOT_FOUND,
                    "找不到 Claude Code 會話日誌檔案路徑。",
                )
            })?;
            resolve_claude_transcript_path(&db::get_claude_dir(), session_id, path)
                .map_err(|error| SessionFileError::new(StatusCode::BAD_REQUEST, error))
        }
        "cursor" => {
            let path = transcript_path_db.ok_or_else(|| {
                SessionFileError::new(StatusCode::NOT_FOUND, "找不到 Cursor 會話日誌檔案路徑。")
            })?;
            resolve_cursor_transcript_path(&db::get_cursor_dir(), session_id, path)
                .map_err(|error| SessionFileError::new(StatusCode::BAD_REQUEST, error))
        }
        "grok" => {
            let path = transcript_path_db.ok_or_else(|| {
                SessionFileError::new(
                    StatusCode::NOT_FOUND,
                    "找不到 Grok Build session 日誌檔案路徑。".to_string(),
                )
            })?;
            resolve_grok_transcript_path(&db::get_grok_dir(), session_id, path)
                .map_err(|error| SessionFileError::new(StatusCode::BAD_REQUEST, error))
        }
        "pi" => {
            let path = transcript_path_db.ok_or_else(|| {
                SessionFileError::new(
                    StatusCode::NOT_FOUND,
                    "找不到 Pi Coding Agent session 日誌檔案路徑。".to_string(),
                )
            })?;
            resolve_pi_family_transcript_path(&db::get_pi_dir(), "Pi Coding Agent", path)
                .map_err(|error| SessionFileError::new(StatusCode::BAD_REQUEST, error))
        }
        "omp" => {
            let path = transcript_path_db.ok_or_else(|| {
                SessionFileError::new(
                    StatusCode::NOT_FOUND,
                    "找不到 OMP session 日誌檔案路徑。".to_string(),
                )
            })?;
            resolve_pi_family_transcript_path(&db::get_omp_dir(), "OMP", path)
                .map_err(|error| SessionFileError::new(StatusCode::BAD_REQUEST, error))
        }
        "muse" => {
            let path = transcript_path_db.ok_or_else(|| {
                SessionFileError::new(
                    StatusCode::NOT_FOUND,
                    "找不到 Muse session 日誌檔案路徑。".to_string(),
                )
            })?;
            resolve_pi_family_transcript_path(&db::get_muse_dir(), "Muse", path)
                .map_err(|error| SessionFileError::new(StatusCode::BAD_REQUEST, error))
        }
        _ => Err(SessionFileError::new(
            StatusCode::BAD_REQUEST,
            "不支援的助理類型",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, time::SystemTime};

    fn copilot_app_fixture_dir(prefix: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "token-insights-test-{prefix}-{}-{unique}",
            std::process::id()
        ))
    }

    fn write_copilot_app_events(app_dir: &StdPath, session_id: &str, lines: &[&str]) {
        let session_dir = app_dir.join("session-state").join(session_id);
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(session_dir.join("events.jsonl"), lines.join("\n")).unwrap();
    }

    #[test]
    fn copilot_app_main_session_resolves_events_jsonl_under_session_state() {
        let app_dir = copilot_app_fixture_dir("app-main-resolve");
        let session_id = "74b6d236-d311-4675-9855-fee91bc508e5";
        write_copilot_app_events(&app_dir, session_id, &["{}"]);

        let resolved = resolve_copilot_app_events_path(&app_dir, session_id, None).unwrap();
        assert!(resolved.ends_with("events.jsonl"));
        assert!(resolved.parent().unwrap().ends_with(session_id));

        let _ = fs::remove_dir_all(&app_dir);
    }

    #[test]
    fn copilot_app_subagent_uses_parent_session_id_for_path() {
        let app_dir = copilot_app_fixture_dir("app-sub-resolve");
        let parent = "74b6d236-d311-4675-9855-fee91bc508e5";
        let agent = "call_v4b32z66";
        write_copilot_app_events(&app_dir, parent, &["{}"]);

        let resolved = resolve_copilot_app_events_path(&app_dir, parent, Some(agent)).unwrap();
        assert!(resolved.parent().unwrap().ends_with(parent));
        assert!(!resolved.to_string_lossy().contains(&format!("__{agent}")));

        let _ = fs::remove_dir_all(&app_dir);
    }

    #[test]
    fn copilot_app_missing_session_dir_returns_file_missing_reason() {
        let app_dir = copilot_app_fixture_dir("app-missing-dir");
        let session_id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

        let error = resolve_copilot_app_events_path(&app_dir, session_id, None).unwrap_err();
        assert_eq!(error.status, StatusCode::NOT_FOUND);
        assert_eq!(error.reason, Some(SessionFileReason::FileMissing));

        let _ = fs::remove_dir_all(&app_dir);
    }

    #[test]
    fn copilot_app_session_dir_without_events_returns_no_events_yet_reason() {
        let app_dir = copilot_app_fixture_dir("app-no-events-yet");
        let session_id = "55555555-6666-7777-8888-999999999999";
        fs::create_dir_all(app_dir.join("session-state").join(session_id)).unwrap();

        let error = resolve_copilot_app_events_path(&app_dir, session_id, None).unwrap_err();
        assert_eq!(error.status, StatusCode::NOT_FOUND);
        assert_eq!(error.reason, Some(SessionFileReason::NoEventsYet));

        let _ = fs::remove_dir_all(&app_dir);
    }

    #[test]
    fn copilot_app_rejects_unsafe_session_id_before_path_lookup() {
        let app_dir = copilot_app_fixture_dir("app-unsafe-id");
        let error = resolve_copilot_app_events_path(&app_dir, "..", None).unwrap_err();
        assert_eq!(error.status, StatusCode::NOT_FOUND);
        assert_eq!(error.reason, Some(SessionFileReason::FileMissing));

        let _ = fs::remove_dir_all(&app_dir);
    }

    #[test]
    fn copilot_app_resolution_uses_the_registered_source_directory() {
        let source_a = copilot_app_fixture_dir("app-source-a");
        let source_b = copilot_app_fixture_dir("app-source-b");
        let session_id = "shared-session-id";
        write_copilot_app_events(&source_a, session_id, &[r#"{"source":"a"}"#]);
        write_copilot_app_events(&source_b, session_id, &[r#"{"source":"b"}"#]);

        let resolved = resolve_session_file_path(
            "copilot",
            session_id,
            None,
            "copilot-app",
            SessionFileResolutionContext {
                copilot_app_source_dir: Some(&source_b),
                ..SessionFileResolutionContext::default()
            },
        )
        .unwrap();

        assert!(resolved.starts_with(source_b.canonicalize().unwrap()));
        assert!(!resolved.starts_with(source_a.canonicalize().unwrap()));

        let _ = fs::remove_dir_all(&source_a);
        let _ = fs::remove_dir_all(&source_b);
    }
}
