use rusqlite::{params, types::Value as SqlValue, Connection};
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::UNIX_EPOCH,
};

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
            None,
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

pub(crate) const SEARCH_INDEX_VERSION: i64 = 1;
const MAX_PROMPT_CHARS: usize = 32_768;
const MAX_REPLY_CHARS: usize = 32_768;
const MAX_TOOL_REQUEST_CHARS: usize = 16_384;
const MAX_TOOL_RESPONSE_CHARS: usize = 16_384;
const SNIPPET_CONTEXT_CHARS: usize = 75;

static SEARCH_INDEXING: AtomicBool = AtomicBool::new(false);
static SEARCH_INDEXED_SESSIONS: AtomicUsize = AtomicUsize::new(0);
static SEARCH_TOTAL_SESSIONS: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct SearchIndexStatus {
    pub indexing: bool,
    pub indexed_sessions: usize,
    pub total_sessions: usize,
}

pub(crate) fn get_search_index_status() -> SearchIndexStatus {
    SearchIndexStatus {
        indexing: SEARCH_INDEXING.load(Ordering::Relaxed),
        indexed_sessions: SEARCH_INDEXED_SESSIONS.load(Ordering::Relaxed),
        total_sessions: SEARCH_TOTAL_SESSIONS.load(Ordering::Relaxed),
    }
}

pub(crate) fn is_valid_search_scope(scope: &str) -> bool {
    matches!(
        scope,
        "user_prompt" | "agent_reply" | "tool_request" | "tool_response"
    )
}

#[derive(Debug, Clone)]
pub(crate) struct GlobalSearchParams {
    pub assistant: String,
    pub query: String,
    pub scopes: Vec<String>,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    pub limit: usize,
    pub offset: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct GlobalSearchHit {
    pub session_id: String,
    pub session_name: Option<String>,
    pub assistant_type: String,
    pub source_kind: String,
    pub source_dir_key: Option<String>,
    pub parent_session_id: Option<String>,
    pub cwd: Option<String>,
    pub date: String,
    pub timestamp: String,
    pub turn_no: u32,
    pub item_index: usize,
    pub content_kind: String,
    pub tool_name: Option<String>,
    pub snippet: String,
    pub snippet_before: String,
    pub snippet_match: String,
    pub snippet_after: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct GlobalSearchResponse {
    pub query: String,
    pub scopes: Vec<String>,
    pub total_matches: usize,
    pub has_more: bool,
    pub index_status: SearchIndexStatus,
    pub results: Vec<GlobalSearchHit>,
}

struct SearchIndexGuard {
    active: bool,
}

impl SearchIndexGuard {
    fn try_acquire() -> Option<Self> {
        SEARCH_INDEXING
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| Self { active: true })
    }
}

impl Drop for SearchIndexGuard {
    fn drop(&mut self) {
        if self.active {
            SEARCH_INDEXING.store(false, Ordering::SeqCst);
        }
    }
}

#[derive(Debug)]
struct ExtractedSearchDocument {
    date: String,
    timestamp: String,
    turn_no: u32,
    item_index: usize,
    content_kind: &'static str,
    tool_name: Option<String>,
    content: String,
}

struct IndexedFileRecord {
    session_name: Option<String>,
    cwd: Option<String>,
    date: String,
    last_timestamp: String,
    file_path: String,
    file_size: i64,
    file_mtime: i64,
    index_version: i64,
}

fn build_session_key(
    assistant_type: &str,
    source_kind: &str,
    source_dir_key: Option<&str>,
    session_id: &str,
    parent_session_id: Option<&str>,
) -> String {
    serde_json::to_string(&(
        assistant_type,
        source_kind,
        source_dir_key.unwrap_or(""),
        session_id,
        parent_session_id.unwrap_or(""),
    ))
    .unwrap_or_else(|_| format!("{assistant_type}:{source_kind}:{session_id}"))
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    let mut chars = text.char_indices();
    if let Some((byte_idx, _)) = chars.nth(max_chars) {
        text[..byte_idx].to_string()
    } else {
        text.to_string()
    }
}

fn is_valid_ymd_date(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit())
}

fn resolve_doc_timestamp_and_date(
    item_timestamp: &str,
    default_date: &str,
    default_timestamp: &str,
) -> (String, String) {
    let trimmed = item_timestamp.trim();
    let ts = if trimmed.is_empty() {
        default_timestamp.to_string()
    } else {
        trimmed.to_string()
    };
    let date = trimmed
        .get(..10)
        .filter(|prefix| is_valid_ymd_date(prefix))
        .unwrap_or(default_date)
        .to_string();
    (date, ts)
}

fn compute_file_signature(source_kind: &str, filepath: &Path) -> Option<(i64, i64)> {
    if source_kind == crate::mcode::SOURCE_KIND {
        let dir = filepath.parent()?;
        let entries = fs::read_dir(dir).ok()?;
        let mut total_size: u64 = 0;
        let mut max_mtime: i64 = 0;
        let mut found = false;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file()
                && path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("jsonl"))
            {
                if let Ok(meta) = fs::metadata(&path) {
                    found = true;
                    total_size = total_size.saturating_add(meta.len());
                    let mtime = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    if mtime > max_mtime {
                        max_mtime = mtime;
                    }
                }
            }
        }
        if !found {
            return None;
        }
        return Some((total_size.min(i64::MAX as u64) as i64, max_mtime));
    }

    let meta = fs::metadata(filepath).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Some((meta.len().min(i64::MAX as u64) as i64, mtime))
}

fn join_non_empty(parts: &[&str], separator: &str) -> String {
    let filtered: Vec<&str> = parts
        .iter()
        .copied()
        .filter(|s| !s.trim().is_empty())
        .collect();
    filtered.join(separator)
}

fn extract_search_documents(
    timeline: &[TimelineItem],
    default_date: &str,
    default_timestamp: &str,
) -> Vec<ExtractedSearchDocument> {
    let mut docs = Vec::new();

    for (item_index, item) in timeline.iter().enumerate() {
        match item {
            TimelineItem::UserPrompt {
                timestamp,
                prompt,
                turn_no,
                ..
            } => {
                if !prompt.trim().is_empty() {
                    let (date, ts) =
                        resolve_doc_timestamp_and_date(timestamp, default_date, default_timestamp);
                    docs.push(ExtractedSearchDocument {
                        date,
                        timestamp: ts,
                        turn_no: *turn_no,
                        item_index,
                        content_kind: "user_prompt",
                        tool_name: None,
                        content: truncate_chars(prompt, MAX_PROMPT_CHARS),
                    });
                }
            }
            TimelineItem::AgentReply {
                timestamp,
                reply,
                reasoning,
                turn_no,
                ..
            } => {
                let reasoning_text = reasoning.as_deref().unwrap_or("");
                let combined = join_non_empty(&[reply.as_str(), reasoning_text], "\n\n");
                if !combined.trim().is_empty() {
                    let (date, ts) =
                        resolve_doc_timestamp_and_date(timestamp, default_date, default_timestamp);
                    docs.push(ExtractedSearchDocument {
                        date,
                        timestamp: ts,
                        turn_no: *turn_no,
                        item_index,
                        content_kind: "agent_reply",
                        tool_name: None,
                        content: truncate_chars(&combined, MAX_REPLY_CHARS),
                    });
                }
            }
            TimelineItem::Reasoning {
                timestamp,
                reasoning,
            } => {
                if !reasoning.trim().is_empty() {
                    let (date, ts) =
                        resolve_doc_timestamp_and_date(timestamp, default_date, default_timestamp);
                    docs.push(ExtractedSearchDocument {
                        date,
                        timestamp: ts,
                        turn_no: 0,
                        item_index,
                        content_kind: "agent_reply",
                        tool_name: None,
                        content: truncate_chars(reasoning, MAX_REPLY_CHARS),
                    });
                }
            }
            TimelineItem::ToolStep {
                timestamp,
                tool_name,
                arguments,
                stdout,
                stderr,
                ..
            } => {
                let (date, ts) =
                    resolve_doc_timestamp_and_date(timestamp, default_date, default_timestamp);

                let args_str = match arguments {
                    serde_json::Value::Null => String::new(),
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Object(map) if map.is_empty() => String::new(),
                    other => serde_json::to_string(other).unwrap_or_default(),
                };
                let req_text = join_non_empty(&[tool_name.as_str(), args_str.as_str()], "\n");
                if !req_text.trim().is_empty() {
                    docs.push(ExtractedSearchDocument {
                        date: date.clone(),
                        timestamp: ts.clone(),
                        turn_no: 0,
                        item_index,
                        content_kind: "tool_request",
                        tool_name: if tool_name.trim().is_empty() {
                            None
                        } else {
                            Some(tool_name.clone())
                        },
                        content: truncate_chars(&req_text, MAX_TOOL_REQUEST_CHARS),
                    });
                }

                let resp_text = join_non_empty(&[stdout.as_str(), stderr.as_str()], "\n");
                if !resp_text.trim().is_empty() {
                    docs.push(ExtractedSearchDocument {
                        date,
                        timestamp: ts,
                        turn_no: 0,
                        item_index,
                        content_kind: "tool_response",
                        tool_name: if tool_name.trim().is_empty() {
                            None
                        } else {
                            Some(tool_name.clone())
                        },
                        content: truncate_chars(&resp_text, MAX_TOOL_RESPONSE_CHARS),
                    });
                }
            }
            TimelineItem::SystemStatus { .. } => {}
        }
    }

    docs
}

fn load_indexed_files_map(conn: &Connection) -> Result<HashMap<String, IndexedFileRecord>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT session_key, session_name, cwd, date, last_timestamp,
                    file_path, file_size, file_mtime, index_version
             FROM search_indexed_files",
        )
        .map_err(|e| format!("讀取已索引會話清單失敗: {e}"))?;

    let mut rows = stmt
        .query([])
        .map_err(|e| format!("查詢已索引會話清單失敗: {e}"))?;
    let mut map = HashMap::new();
    while let Some(row) = rows
        .next()
        .map_err(|e| format!("掃描已索引會話清單失敗: {e}"))?
    {
        let key: String = row.get(0).map_err(|e| e.to_string())?;
        map.insert(
            key,
            IndexedFileRecord {
                session_name: row.get(1).map_err(|e| e.to_string())?,
                cwd: row.get(2).map_err(|e| e.to_string())?,
                date: row.get(3).map_err(|e| e.to_string())?,
                last_timestamp: row.get(4).map_err(|e| e.to_string())?,
                file_path: row.get(5).map_err(|e| e.to_string())?,
                file_size: row.get(6).map_err(|e| e.to_string())?,
                file_mtime: row.get(7).map_err(|e| e.to_string())?,
                index_version: row.get(8).map_err(|e| e.to_string())?,
            },
        );
    }
    Ok(map)
}

/// 依本機對話日誌檔案的 `size` 與 `mtime` 增量更新 SQLite 全文搜尋索引。
pub(crate) fn sync_search_index_with_cancel(
    conn: &mut Connection,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    let Some(_guard) = SearchIndexGuard::try_acquire() else {
        return Ok(());
    };

    let sessions = db::list_all_searchable_sessions(conn)?;
    SEARCH_TOTAL_SESSIONS.store(sessions.len(), Ordering::Relaxed);
    SEARCH_INDEXED_SESSIONS.store(0, Ordering::Relaxed);

    let existing_map = load_indexed_files_map(conn)?;
    let mut seen_keys = HashSet::with_capacity(sessions.len());
    let mut processed: usize = 0;

    for session in sessions {
        if is_cancelled() {
            return Ok(());
        }

        processed += 1;
        SEARCH_INDEXED_SESSIONS.store(processed, Ordering::Relaxed);

        let session_key = build_session_key(
            &session.assistant_type,
            &session.source_kind,
            session.source_dir_key.as_deref(),
            &session.session_id,
            session.parent_session_id.as_deref(),
        );
        seen_keys.insert(session_key.clone());

        let copilot_app_source_dir = if session.source_kind == "copilot-app" {
            match session.source_dir_key.as_deref() {
                Some(key) => db::get_usage_source_directory(
                    conn,
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
            _ => continue,
        };

        let Some((file_size, file_mtime)) = compute_file_signature(&session.source_kind, &filepath)
        else {
            continue;
        };
        let file_path_str = filepath.to_string_lossy().into_owned();

        if let Some(existing) = existing_map.get(&session_key) {
            if existing.index_version == SEARCH_INDEX_VERSION
                && existing.file_path == file_path_str
                && existing.file_size == file_size
                && existing.file_mtime == file_mtime
            {
                if existing.session_name != session.session_name
                    || existing.cwd != session.cwd
                    || existing.date != session.date
                    || existing.last_timestamp != session.last_timestamp
                {
                    conn.execute(
                        "UPDATE search_indexed_files
                         SET session_name = ?1, cwd = ?2, date = ?3, last_timestamp = ?4
                         WHERE session_key = ?5",
                        params![
                            session.session_name,
                            session.cwd,
                            session.date,
                            session.last_timestamp,
                            session_key
                        ],
                    )
                    .map_err(|e| format!("更新搜尋會話中繼資料失敗: {e}"))?;
                }
                continue;
            }
        }

        let db_entries = HashMap::new();
        let Ok((timeline, _)) = parse_session_timeline_file(
            &session.assistant_type,
            &session.source_kind,
            &filepath,
            &db_entries,
            None,
            session.agent_nickname.as_deref(),
            session.model.as_deref(),
        ) else {
            continue;
        };

        let docs = extract_search_documents(&timeline, &session.date, &session.last_timestamp);

        let tx = conn
            .transaction()
            .map_err(|e| format!("開啟搜尋索引交易失敗: {e}"))?;
        tx.execute(
            "DELETE FROM search_documents WHERE session_key = ?1",
            params![session_key],
        )
        .map_err(|e| format!("清除舊搜尋文件失敗: {e}"))?;

        {
            let mut insert_stmt = tx
                .prepare(
                    "INSERT INTO search_documents (
                        session_key, assistant_type, session_id, source_kind,
                        source_dir_key, parent_session_id, date, timestamp,
                        turn_no, item_index, content_kind, tool_name, content
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                )
                .map_err(|e| format!("準備寫入搜尋文件失敗: {e}"))?;

            let source_dir_key = session.source_dir_key.as_deref().unwrap_or("");
            let parent_session_id = session.parent_session_id.as_deref().unwrap_or("");
            for doc in &docs {
                insert_stmt
                    .execute(params![
                        session_key,
                        session.assistant_type,
                        session.session_id,
                        session.source_kind,
                        source_dir_key,
                        parent_session_id,
                        doc.date,
                        doc.timestamp,
                        doc.turn_no as i64,
                        doc.item_index as i64,
                        doc.content_kind,
                        doc.tool_name,
                        doc.content,
                    ])
                    .map_err(|e| format!("寫入搜尋文件失敗: {e}"))?;
            }
        }

        tx.execute(
            "INSERT OR REPLACE INTO search_indexed_files (
                session_key, assistant_type, session_id, source_kind,
                source_dir_key, parent_session_id, session_name, cwd,
                date, last_timestamp, file_path, file_size, file_mtime, index_version
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                session_key,
                session.assistant_type,
                session.session_id,
                session.source_kind,
                session.source_dir_key.as_deref().unwrap_or(""),
                session.parent_session_id.as_deref().unwrap_or(""),
                session.session_name,
                session.cwd,
                session.date,
                session.last_timestamp,
                file_path_str,
                file_size,
                file_mtime,
                SEARCH_INDEX_VERSION,
            ],
        )
        .map_err(|e| format!("更新搜尋檔案索引狀態失敗: {e}"))?;

        tx.commit()
            .map_err(|e| format!("提交搜尋索引交易失敗: {e}"))?;
    }

    if !is_cancelled() {
        let stale_keys: Vec<String> = existing_map
            .keys()
            .filter(|k| !seen_keys.contains(*k))
            .cloned()
            .collect();
        if !stale_keys.is_empty() {
            let tx = conn
                .transaction()
                .map_err(|e| format!("開啟孤兒索引清理交易失敗: {e}"))?;
            for stale_key in stale_keys {
                tx.execute(
                    "DELETE FROM search_documents WHERE session_key = ?1",
                    params![stale_key],
                )
                .map_err(|e| format!("清除孤兒搜尋文件失敗: {e}"))?;
                tx.execute(
                    "DELETE FROM search_indexed_files WHERE session_key = ?1",
                    params![stale_key],
                )
                .map_err(|e| format!("清除孤兒搜尋索引狀態失敗: {e}"))?;
            }
            tx.commit()
                .map_err(|e| format!("提交孤兒索引清理失敗: {e}"))?;
        }
    }

    Ok(())
}

fn normalize_snippet_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out
}

fn find_case_insensitive_char_span(content: &str, query: &str) -> Option<(usize, usize)> {
    let query_lc: Vec<char> = query.chars().flat_map(|c| c.to_lowercase()).collect();
    if query_lc.is_empty() {
        return None;
    }
    let mut flat: Vec<(char, usize)> = Vec::with_capacity(content.chars().count());
    for (orig_idx, ch) in content.chars().enumerate() {
        for lc in ch.to_lowercase() {
            flat.push((lc, orig_idx));
        }
    }
    if flat.len() < query_lc.len() {
        return None;
    }
    for i in 0..=flat.len() - query_lc.len() {
        if flat[i..i + query_lc.len()]
            .iter()
            .zip(query_lc.iter())
            .all(|((a, _), b)| a == b)
        {
            let start = flat[i].1;
            let end = flat[i + query_lc.len() - 1].1 + 1;
            return Some((start, end));
        }
    }
    None
}

fn build_snippet_parts(
    content: &str,
    query: &str,
    context_chars: usize,
) -> (String, String, String) {
    let chars: Vec<char> = content.chars().collect();
    if chars.is_empty() {
        return (String::new(), String::new(), String::new());
    }

    if let Some((start, end)) = find_case_insensitive_char_span(content, query) {
        let ctx_start = start.saturating_sub(context_chars);
        let ctx_end = (end + context_chars).min(chars.len());

        let raw_before: String = chars[ctx_start..start].iter().collect();
        let raw_match: String = chars[start..end].iter().collect();
        let raw_after: String = chars[end..ctx_end].iter().collect();

        let mut before = normalize_snippet_whitespace(&raw_before);
        if ctx_start > 0 {
            before = format!("…{}", before.trim_start());
        } else {
            before = before.trim_start().to_string();
        }
        let mut after = normalize_snippet_whitespace(&raw_after);
        if ctx_end < chars.len() {
            after = format!("{}…", after.trim_end());
        } else {
            after = after.trim_end().to_string();
        }
        (before, raw_match, after)
    } else {
        let preview_end = chars.len().min(context_chars * 2);
        let raw_preview: String = chars[..preview_end].iter().collect();
        let mut preview = normalize_snippet_whitespace(raw_preview.trim());
        if preview_end < chars.len() {
            preview.push('…');
        }
        (preview, String::new(), String::new())
    }
}

fn format_fts5_phrase_literal(query: &str) -> String {
    format!("\"{}\"", query.replace('"', "\"\""))
}

/// 執行跨全歷史的 SQLite 全文關鍵字搜尋。
pub(crate) fn search_global_content(
    conn: &Connection,
    params: &GlobalSearchParams,
) -> Result<GlobalSearchResponse, String> {
    if params.scopes.is_empty() {
        return Err("至少須選擇一項搜尋範圍。".to_string());
    }

    let query_trimmed = params.query.trim();
    if query_trimmed.is_empty() {
        return Err("搜尋關鍵字不可為空。".to_string());
    }

    let use_fts = query_trimmed.chars().count() >= 3;
    let mut from_and_where = String::from("FROM search_documents d ");
    let mut sql_params: Vec<SqlValue> = Vec::new();

    if use_fts {
        from_and_where.push_str("INNER JOIN search_documents_fts fts ON fts.rowid = d.id ");
    }
    from_and_where
        .push_str("LEFT JOIN search_indexed_files f ON f.session_key = d.session_key WHERE ");

    let mut conditions: Vec<String> = Vec::new();

    if use_fts {
        conditions.push("fts.content MATCH ?".to_string());
        sql_params.push(SqlValue::Text(format_fts5_phrase_literal(query_trimmed)));
    }
    conditions.push("INSTR(LOWER(d.content), LOWER(?)) > 0".to_string());
    sql_params.push(SqlValue::Text(query_trimmed.to_string()));

    let scope_placeholders = vec!["?"; params.scopes.len()].join(", ");
    conditions.push(format!("d.content_kind IN ({scope_placeholders})"));
    for scope in &params.scopes {
        sql_params.push(SqlValue::Text(scope.clone()));
    }

    if params.assistant != "all" {
        conditions.push("d.assistant_type = ?".to_string());
        sql_params.push(SqlValue::Text(params.assistant.clone()));
    }
    if let Some(date_from) = params.date_from.as_deref().filter(|s| !s.is_empty()) {
        conditions.push("d.date >= ?".to_string());
        sql_params.push(SqlValue::Text(date_from.to_string()));
    }
    if let Some(date_to) = params.date_to.as_deref().filter(|s| !s.is_empty()) {
        conditions.push("d.date <= ?".to_string());
        sql_params.push(SqlValue::Text(date_to.to_string()));
    }

    from_and_where.push_str(&conditions.join(" AND "));

    let count_sql = format!("SELECT COUNT(*) {from_and_where}");
    let total_matches: usize = conn
        .query_row(
            &count_sql,
            rusqlite::params_from_iter(sql_params.iter()),
            |row| row.get::<_, i64>(0),
        )
        .map_err(|e| format!("統計全域搜尋結果數量失敗: {e}"))?
        .max(0) as usize;

    let mut select_params = sql_params;
    select_params.push(SqlValue::Integer(params.limit as i64));
    select_params.push(SqlValue::Integer(params.offset as i64));

    let select_sql = format!(
        "SELECT d.session_id, f.session_name, d.assistant_type, d.source_kind,
                d.source_dir_key, d.parent_session_id, f.cwd, d.date, d.timestamp,
                d.turn_no, d.item_index, d.content_kind, d.tool_name, d.content
         {from_and_where}
         ORDER BY d.timestamp DESC, d.id DESC
         LIMIT ? OFFSET ?"
    );

    let mut stmt = conn
        .prepare(&select_sql)
        .map_err(|e| format!("準備全域搜尋查詢失敗: {e}"))?;
    let mut rows = stmt
        .query(rusqlite::params_from_iter(select_params.iter()))
        .map_err(|e| format!("執行全域搜尋查詢失敗: {e}"))?;

    let mut results = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(|e| format!("讀取全域搜尋結果列失敗: {e}"))?
    {
        let raw_source_dir_key: String = row.get(4).map_err(|e| e.to_string())?;
        let raw_parent_id: String = row.get(5).map_err(|e| e.to_string())?;
        let content: String = row.get(13).map_err(|e| e.to_string())?;
        let (snippet_before, snippet_match, snippet_after) =
            build_snippet_parts(&content, query_trimmed, SNIPPET_CONTEXT_CHARS);
        let snippet = format!("{snippet_before}{snippet_match}{snippet_after}");

        results.push(GlobalSearchHit {
            session_id: row.get(0).map_err(|e| e.to_string())?,
            session_name: row.get(1).map_err(|e| e.to_string())?,
            assistant_type: row.get(2).map_err(|e| e.to_string())?,
            source_kind: row.get(3).map_err(|e| e.to_string())?,
            source_dir_key: if raw_source_dir_key.is_empty() {
                None
            } else {
                Some(raw_source_dir_key)
            },
            parent_session_id: if raw_parent_id.is_empty() {
                None
            } else {
                Some(raw_parent_id)
            },
            cwd: row.get(6).map_err(|e| e.to_string())?,
            date: row.get(7).map_err(|e| e.to_string())?,
            timestamp: row.get(8).map_err(|e| e.to_string())?,
            turn_no: row.get::<_, i64>(9).map_err(|e| e.to_string()).unwrap_or(0) as u32,
            item_index: row
                .get::<_, i64>(10)
                .map_err(|e| e.to_string())
                .unwrap_or(0) as usize,
            content_kind: row.get(11).map_err(|e| e.to_string())?,
            tool_name: row.get(12).map_err(|e| e.to_string())?,
            snippet,
            snippet_before,
            snippet_match,
            snippet_after,
        });
    }

    let has_more = params.offset.saturating_add(results.len()) < total_matches;
    Ok(GlobalSearchResponse {
        query: query_trimmed.to_string(),
        scopes: params.scopes.clone(),
        total_matches,
        has_more,
        index_status: get_search_index_status(),
        results,
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

    #[test]
    fn extract_search_documents_covers_all_four_scopes_and_truncates_utf8_safely() {
        let long_prompt = "測".repeat(super::MAX_PROMPT_CHARS + 50);
        let timeline = vec![
            TimelineItem::UserPrompt {
                timestamp: "2026-10-05T01:00:00Z".to_string(),
                prompt: long_prompt,
                context: None,
                turn_no: 1,
            },
            TimelineItem::AgentReply {
                timestamp: "2026-10-05T01:00:05Z".to_string(),
                reply: "Here is the fix".to_string(),
                reasoning: Some("Internal thought about cargo".to_string()),
                turn_no: 1,
                model: "gpt-5".to_string(),
                tokens: None,
                duration_ms: None,
                reasoning_effort: None,
            },
            TimelineItem::Reasoning {
                timestamp: "".to_string(),
                reasoning: "Standalone reasoning step".to_string(),
            },
            TimelineItem::ToolStep {
                timestamp: "2026-10-05T01:00:06Z".to_string(),
                tool_name: "run_command".to_string(),
                arguments: serde_json::json!({ "cmd": "cargo test --release" }),
                env: None,
                exit_code: Some(0),
                stdout: "test result: ok".to_string(),
                stderr: "warning: minor".to_string(),
                tool_call_id: None,
                status: "success".to_string(),
            },
            TimelineItem::SystemStatus {
                timestamp: "2026-10-05T01:00:07Z".to_string(),
                status_type: "session_end".to_string(),
                message: "ignored system message".to_string(),
            },
        ];

        let docs = super::extract_search_documents(&timeline, "2026-10-05", "2026-10-05T01:00:00Z");
        assert_eq!(docs.len(), 5);
        assert_eq!(docs[0].content_kind, "user_prompt");
        assert_eq!(docs[0].content.chars().count(), super::MAX_PROMPT_CHARS);
        assert_eq!(docs[0].item_index, 0);

        assert_eq!(docs[1].content_kind, "agent_reply");
        assert!(docs[1].content.contains("Here is the fix"));
        assert!(docs[1].content.contains("Internal thought about cargo"));
        assert_eq!(docs[1].item_index, 1);

        assert_eq!(docs[2].content_kind, "agent_reply");
        assert_eq!(docs[2].content, "Standalone reasoning step");
        assert_eq!(docs[2].item_index, 2);

        assert_eq!(docs[3].content_kind, "tool_request");
        assert_eq!(docs[3].tool_name.as_deref(), Some("run_command"));
        assert!(docs[3].content.contains("cargo test --release"));
        assert_eq!(docs[3].item_index, 3);

        assert_eq!(docs[4].content_kind, "tool_response");
        assert_eq!(docs[4].tool_name.as_deref(), Some("run_command"));
        assert!(docs[4].content.contains("test result: ok"));
        assert!(docs[4].content.contains("warning: minor"));
        assert_eq!(docs[4].item_index, 3);
    }

    #[test]
    fn snippet_builder_preserves_utf8_and_highlights_case_insensitively() {
        let content = format!(
            "{}🚀 請協助排查 PaymentCallback 失敗原因 {}",
            "前言".repeat(50),
            "結尾".repeat(50)
        );
        let (before, matched, after) = super::build_snippet_parts(&content, "paymentcallback", 15);
        assert_eq!(matched, "PaymentCallback");
        assert!(before.starts_with('…'));
        assert!(after.ends_with('…'));
        assert!(before.contains("🚀"));
    }

    #[test]
    fn global_search_sync_and_query_supports_fts5_short_queries_and_incremental_updates() {
        if crate::reporting::is_isolated_insights_test_child() {
            global_search_sync_and_query_body();
            return;
        }
        let temp_dir = std::env::temp_dir().join(format!(
            "tui-global-search-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let output = crate::reporting::spawn_isolated_insights_test(
            "session_search::tests::global_search_sync_and_query_supports_fts5_short_queries_and_incremental_updates",
            &temp_dir,
        );
        assert!(
            output.status.success(),
            "子行程全域搜尋測試失敗:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    fn write_rich_codex_rollout(
        path: &std::path::Path,
        ts_date: &str,
        user_msg: &str,
        assistant_msg: &str,
        tool_arg: &str,
        tool_out: &str,
    ) {
        let lines = [
            format!(
                r#"{{"timestamp":"{ts_date}T10:00:00.000Z","type":"session_meta","payload":{{"id":"s1","cwd":"/tmp/app","cli_version":"0.142.5","model":"gpt-5"}}}}"#
            ),
            format!(
                r#"{{"timestamp":"{ts_date}T10:00:01.000Z","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":100,"output_tokens":10,"total_tokens":110}},"last_token_usage":{{"input_tokens":100,"output_tokens":10,"total_tokens":110}}}}}}}}"#
            ),
            format!(
                r#"{{"timestamp":"{ts_date}T10:00:02.000Z","type":"event_msg","payload":{{"type":"user_message","message":{}}}}}"#,
                serde_json::to_string(user_msg).unwrap()
            ),
            format!(
                r#"{{"timestamp":"{ts_date}T10:00:03.000Z","type":"response_item","payload":{{"type":"function_call","name":"exec_cmd","arguments":{},"call_id":"call_1"}}}}"#,
                serde_json::to_string(&serde_json::json!({ "command": tool_arg }).to_string())
                    .unwrap()
            ),
            format!(
                r#"{{"timestamp":"{ts_date}T10:00:04.000Z","type":"response_item","payload":{{"type":"function_call_output","call_id":"call_1","output":{}}}}}"#,
                serde_json::to_string(tool_out).unwrap()
            ),
            format!(
                r#"{{"timestamp":"{ts_date}T10:00:05.000Z","type":"event_msg","payload":{{"type":"agent_message","message":{}}}}}"#,
                serde_json::to_string(assistant_msg).unwrap()
            ),
        ];
        fs::write(path, format!("{}\n", lines.join("\n"))).unwrap();
    }

    fn global_search_sync_and_query_body() {
        let mut conn =
            Connection::open(crate::db::get_insights_dir().join("token_usage_insights.db"))
                .unwrap();
        crate::db::init_db(&conn).unwrap();

        let codex_root = crate::db::get_insights_dir().join("codex-root");
        std::env::set_var("CODEX_DIR", &codex_root);
        let rollout_dir = codex_root
            .join("sessions")
            .join("2026")
            .join("10")
            .join("05");
        fs::create_dir_all(&rollout_dir).unwrap();

        let rollout_1 = rollout_dir.join("rollout-s1.jsonl");
        write_rich_codex_rollout(
            &rollout_1,
            "2026-10-05",
            "請幫我排查 AI 部署失敗",
            "這是 AI 回應中的 AlphaReplyToken 分析",
            "kubectl get pods ReqNeedleToken",
            "Error: CrashLoopBackOff RespNeedleToken",
        );

        let rollout_2 = rollout_dir.join("rollout-s2.jsonl");
        write_rich_codex_rollout(
            &rollout_2,
            "2026-09-01",
            "另一筆舊對話記錄 PromptNeedle",
            "普通回應",
            "ls",
            "ok",
        );

        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, timestamp, date, transcript_path,
                session_id, turn_no, session_name, cwd, model,
                tokens_input, tokens_output, tokens_total,
                delta_input, delta_output, delta_total
            ) VALUES (
                'codex', 'codex-cli', '2026-10-05 10:00:05', '2026-10-05', ?,
                's1', 1, 'Debug Deploy', '/tmp/app', 'gpt-5',
                100, 10, 110, 100, 10, 110
            )",
            params![rollout_1.to_str().unwrap()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, source_kind, timestamp, date, transcript_path,
                session_id, turn_no, session_name, cwd, model,
                tokens_input, tokens_output, tokens_total,
                delta_input, delta_output, delta_total
            ) VALUES (
                'codex', 'codex-cli', '2026-09-01 08:00:00', '2026-09-01', ?,
                's2', 1, 'Old Session', '/tmp/app', 'gpt-5',
                100, 10, 110, 100, 10, 110
            )",
            params![rollout_2.to_str().unwrap()],
        )
        .unwrap();

        super::sync_search_index_with_cancel(&mut conn, &|| false).expect("建索引必須成功");

        // 1. 預設只搜 user_prompt 不會命中 agent_reply / tool_request / tool_response
        let default_res = super::search_global_content(
            &conn,
            &super::GlobalSearchParams {
                assistant: "codex".to_string(),
                query: "AlphaReplyToken".to_string(),
                scopes: vec!["user_prompt".to_string()],
                date_from: None,
                date_to: None,
                limit: 50,
                offset: 0,
            },
        )
        .unwrap();
        assert_eq!(default_res.total_matches, 0);

        // 2. 勾選 agent_reply 可命中（且大小寫不敏感）
        let reply_res = super::search_global_content(
            &conn,
            &super::GlobalSearchParams {
                assistant: "all".to_string(),
                query: "alphareplytoken".to_string(),
                scopes: vec!["user_prompt".to_string(), "agent_reply".to_string()],
                date_from: None,
                date_to: None,
                limit: 50,
                offset: 0,
            },
        )
        .unwrap();
        assert_eq!(reply_res.total_matches, 1);
        assert_eq!(reply_res.results[0].content_kind, "agent_reply");
        assert_eq!(reply_res.results[0].snippet_match, "AlphaReplyToken");

        // 3. tool_request 與 tool_response 個別命中
        let req_res = super::search_global_content(
            &conn,
            &super::GlobalSearchParams {
                assistant: "codex".to_string(),
                query: "reqneedletoken".to_string(),
                scopes: vec!["tool_request".to_string()],
                date_from: None,
                date_to: None,
                limit: 50,
                offset: 0,
            },
        )
        .unwrap();
        assert_eq!(req_res.total_matches, 1);
        assert_eq!(req_res.results[0].tool_name.as_deref(), Some("exec_cmd"));

        let resp_res = super::search_global_content(
            &conn,
            &super::GlobalSearchParams {
                assistant: "codex".to_string(),
                query: "respneedletoken".to_string(),
                scopes: vec!["tool_response".to_string()],
                date_from: None,
                date_to: None,
                limit: 50,
                offset: 0,
            },
        )
        .unwrap();
        assert_eq!(resp_res.total_matches, 1);
        assert_eq!(resp_res.results[0].content_kind, "tool_response");

        // 4. 短關鍵字 (< 3 chars) 走 fallback 比對
        let short_res = super::search_global_content(
            &conn,
            &super::GlobalSearchParams {
                assistant: "all".to_string(),
                query: "AI".to_string(),
                scopes: vec!["user_prompt".to_string(), "agent_reply".to_string()],
                date_from: None,
                date_to: None,
                limit: 1,
                offset: 0,
            },
        )
        .unwrap();
        assert_eq!(short_res.total_matches, 2);
        assert!(short_res.has_more);

        // 5. 日期範圍過濾
        let date_filtered = super::search_global_content(
            &conn,
            &super::GlobalSearchParams {
                assistant: "all".to_string(),
                query: "PromptNeedle".to_string(),
                scopes: vec!["user_prompt".to_string()],
                date_from: Some("2026-10-01".to_string()),
                date_to: Some("2026-10-31".to_string()),
                limit: 50,
                offset: 0,
            },
        )
        .unwrap();
        assert_eq!(date_filtered.total_matches, 0);

        // 6. 檔案內容更新時增量重索引，移除舊文字並納入新文字
        write_rich_codex_rollout(
            &rollout_1,
            "2026-10-05",
            "更新後的全新提示詞 UpdatedNeedle",
            "更新回應",
            "ls",
            "ok",
        );
        super::sync_search_index_with_cancel(&mut conn, &|| false).expect("增量更新必須成功");

        let updated_hit = super::search_global_content(
            &conn,
            &super::GlobalSearchParams {
                assistant: "codex".to_string(),
                query: "UpdatedNeedle".to_string(),
                scopes: vec!["user_prompt".to_string()],
                date_from: None,
                date_to: None,
                limit: 50,
                offset: 0,
            },
        )
        .unwrap();
        assert_eq!(updated_hit.total_matches, 1);

        let stale_hit = super::search_global_content(
            &conn,
            &super::GlobalSearchParams {
                assistant: "codex".to_string(),
                query: "AlphaReplyToken".to_string(),
                scopes: vec!["agent_reply".to_string()],
                date_from: None,
                date_to: None,
                limit: 50,
                offset: 0,
            },
        )
        .unwrap();
        assert_eq!(stale_hit.total_matches, 0);

        // 7. 刪除 usage_entries 中的 s2 後，孤兒索引須被清除
        conn.execute("DELETE FROM usage_entries WHERE session_id = 's2'", [])
            .unwrap();
        super::sync_search_index_with_cancel(&mut conn, &|| false).expect("清理孤兒必須成功");
        let orphan_hit = super::search_global_content(
            &conn,
            &super::GlobalSearchParams {
                assistant: "all".to_string(),
                query: "PromptNeedle".to_string(),
                scopes: vec!["user_prompt".to_string()],
                date_from: None,
                date_to: None,
                limit: 50,
                offset: 0,
            },
        )
        .unwrap();
        assert_eq!(orphan_hit.total_matches, 0);
    }
}
