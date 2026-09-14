use super::*;

pub(crate) fn parse_cursor_timestamp(s: &str) -> String {
    let parts: Vec<&str> = s.split(" (UTC").collect();
    if parts.is_empty() {
        return s.to_string();
    }
    let dt_part = parts[0].trim();
    let dt_str = if let Some(comma_idx) = dt_part.find(',') {
        dt_part[comma_idx + 1..].trim()
    } else {
        dt_part
    };

    let formats = [
        "%b %e, %Y, %l:%M %p",
        "%b %d, %Y, %I:%M %p",
        "%b %d, %Y, %l:%M %p",
        "%b %e, %Y, %I:%M %p",
        "%Y-%m-%d %H:%M:%S",
    ];

    for fmt in &formats {
        if let Ok(naive_dt) = chrono::NaiveDateTime::parse_from_str(dt_str, fmt) {
            if parts.len() > 1 {
                let tz_str = parts[1].trim_end_matches(')');
                let hours_str = if tz_str.contains(':') {
                    tz_str.split(':').next().unwrap_or("0")
                } else {
                    tz_str
                };
                if let Ok(hours) = hours_str.parse::<i32>() {
                    if let Some(offset) = chrono::FixedOffset::east_opt(hours * 3600) {
                        use chrono::TimeZone;
                        let local_dt = offset.from_local_datetime(&naive_dt);
                        if let chrono::LocalResult::Single(dt_tz) = local_dt {
                            return dt_tz.to_rfc3339();
                        }
                    }
                }
            }
            return naive_dt.format("%Y-%m-%d %H:%M:%S").to_string();
        }
    }

    s.to_string()
}

fn find_cursor_session_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(find_cursor_session_files(&path));
            } else if path.is_file()
                && path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("jsonl"))
            {
                files.push(path);
            }
        }
    }
    files
}

fn cursor_content_to_text(content: &serde_json::Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_string();
    }
    let mut parts = Vec::new();
    if let Some(items) = content.as_array() {
        for item in items {
            let itype = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if itype == "text" {
                if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                    parts.push(text.to_string());
                }
            }
        }
    }
    parts.join(" ")
}

pub(super) fn cursor_response_signature(content: &serde_json::Value) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(text) = content.as_str() {
        if !text.is_empty() {
            parts.push(serde_json::json!(["text", text]));
        }
    } else {
        for item in content.as_array()? {
            match item.get("type").and_then(|value| value.as_str()) {
                Some("text") => {
                    if let Some(text) = item
                        .get("text")
                        .or_else(|| item.get("data"))
                        .and_then(|value| value.as_str())
                        .filter(|value| !value.is_empty())
                    {
                        parts.push(serde_json::json!(["text", text]));
                    }
                }
                Some("tool_use") => {
                    let Some(name) = item.get("name").and_then(|value| value.as_str()) else {
                        continue;
                    };
                    parts.push(serde_json::json!([
                        "tool",
                        name,
                        item.get("input")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null)
                    ]));
                }
                Some("tool-call") => {
                    let Some(name) = item.get("toolName").and_then(|value| value.as_str()) else {
                        continue;
                    };
                    parts.push(serde_json::json!([
                        "tool",
                        name,
                        item.get("args").cloned().unwrap_or(serde_json::Value::Null)
                    ]));
                }
                _ => {}
            }
        }
    }
    if parts.is_empty() {
        return None;
    }
    let serialized = serde_json::to_string(&parts).ok()?;
    Some(format!(
        "{:016x}",
        hash_fnv1a_64(&format!("cursor-response-v2:{serialized}"))
    ))
}

fn cursor_model_from_provider_options(value: &serde_json::Value) -> Option<String> {
    value
        .get("providerOptions")
        .and_then(|provider_options| provider_options.get("cursor"))
        .and_then(|cursor| cursor.get("modelName"))
        .and_then(|model| model.as_str())
        .map(str::trim)
        .filter(|model| !model.is_empty() && model.len() <= 200)
        .map(str::to_string)
}

pub(super) fn parse_cursor_agent_kv_model_signature(raw: &[u8]) -> Option<(String, String)> {
    let event: serde_json::Value = serde_json::from_slice(raw).ok()?;
    if event.get("role").and_then(|value| value.as_str()) != Some("assistant") {
        return None;
    }
    let content = event
        .get("content")
        .or_else(|| event.pointer("/message/content"))?;
    let mut models = HashSet::new();
    if let Some(model) = cursor_model_from_provider_options(&event) {
        models.insert(model);
    }
    if let Some(items) = content.as_array() {
        for item in items {
            if let Some(model) = cursor_model_from_provider_options(item) {
                models.insert(model);
            }
        }
    }
    if models.len() != 1 {
        return None;
    }
    Some((
        cursor_response_signature(content)?,
        models.into_iter().next()?,
    ))
}

fn cursor_model_source_id(path: &Path) -> String {
    let resolved = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let normalized = resolved.to_string_lossy().replace('\\', "/");
    #[cfg(windows)]
    let normalized = normalized.to_lowercase();
    format!("{:016x}", hash_fnv1a_64(&normalized))
}

fn cursor_mode_source_kind(mode: Option<&str>) -> Option<String> {
    match mode {
        Some("agent") => Some(CURSOR_AGENT_SOURCE_KIND.to_string()),
        Some("ide") => Some(CURSOR_IDE_SOURCE_KIND.to_string()),
        _ => None,
    }
}

pub(super) fn cursor_date_from_timestamp(timestamp: &str) -> Option<&str> {
    let date = timestamp.get(..10)?;
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    Some(date)
}

fn run_cursor_model_attribution_migration(conn: &mut Connection) -> Result<(), String> {
    let already_applied: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_state WHERE filename = ?)",
            params![CURSOR_MODEL_ATTRIBUTION_MIGRATION_KEY],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if already_applied {
        return Ok(());
    }

    let tx = conn
        .transaction()
        .map_err(|error| format!("啟動 Cursor 模型歸因遷移失敗: {error}"))?;
    tx.execute(
        "UPDATE usage_entries
         SET model = 'Unknown Model', model_id = 'Unknown Model'
         WHERE assistant_type = 'cursor'
           AND (model IS NULL OR model = '' OR model = 'Cursor Agent')",
        [],
    )
    .map_err(|error| format!("重設 Cursor 籠統模型名稱失敗: {error}"))?;
    tx.execute(
        "DELETE FROM sync_state
         WHERE filename LIKE 'cursor:%'
            OR filename LIKE 'cursor-agent-kv:%'
            OR filename LIKE 'cursor-composer-data:%'",
        [],
    )
    .map_err(|error| format!("重設 Cursor 同步狀態失敗: {error}"))?;
    tx.execute(
        "INSERT OR REPLACE INTO sync_state (filename, last_synced_size, last_synced_time)
         VALUES (?, 1, 0)",
        params![CURSOR_MODEL_ATTRIBUTION_MIGRATION_KEY],
    )
    .map_err(|error| format!("記錄 Cursor 模型歸因遷移失敗: {error}"))?;
    tx.commit()
        .map_err(|error| format!("提交 Cursor 模型歸因遷移失敗: {error}"))
}

pub(super) fn run_cursor_cache_tokens_unknown_migration(
    conn: &mut Connection,
) -> Result<(), String> {
    let already_applied: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_state WHERE filename = ?)",
            params![CURSOR_CACHE_TOKENS_UNKNOWN_MIGRATION_KEY],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if already_applied {
        return Ok(());
    }

    let tx = conn
        .transaction()
        .map_err(|error| format!("啟動 Cursor 快取 Token 遷移失敗: {error}"))?;
    tx.execute(
        "UPDATE usage_entries
         SET tokens_cache_read = NULL,
             tokens_cache_write = NULL,
             tokens_cache_write_5m = NULL,
             tokens_cache_write_1h = NULL,
             delta_cache_read = NULL,
             delta_cache_write = NULL,
             delta_cache_write_5m = NULL,
             delta_cache_write_1h = NULL
         WHERE assistant_type = 'cursor'
           AND COALESCE(tokens_cache_read, 0) = 0
           AND COALESCE(tokens_cache_write, 0) = 0
           AND COALESCE(tokens_cache_write_5m, 0) = 0
           AND COALESCE(tokens_cache_write_1h, 0) = 0
           AND COALESCE(delta_cache_read, 0) = 0
           AND COALESCE(delta_cache_write, 0) = 0
           AND COALESCE(delta_cache_write_5m, 0) = 0
           AND COALESCE(delta_cache_write_1h, 0) = 0",
        [],
    )
    .map_err(|error| format!("將 Cursor 快取 Token 標記為未知失敗: {error}"))?;
    tx.execute(
        "INSERT OR REPLACE INTO sync_state (filename, last_synced_size, last_synced_time)
         VALUES (?, 1, 0)",
        params![CURSOR_CACHE_TOKENS_UNKNOWN_MIGRATION_KEY],
    )
    .map_err(|error| format!("記錄 Cursor 快取 Token 遷移失敗: {error}"))?;
    tx.commit()
        .map_err(|error| format!("提交 Cursor 快取 Token 遷移失敗: {error}"))
}

pub(super) fn open_cursor_state_db(state_db_path: &Path) -> Result<Connection, String> {
    let conn = Connection::open_with_flags(
        state_db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("無法唯讀開啟 Cursor state.vscdb: {error}"))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|error| format!("設定 Cursor state.vscdb busy timeout 失敗: {error}"))?;
    let has_cursor_disk_kv: bool = conn
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM sqlite_master
                WHERE type = 'table' AND name = 'cursorDiskKV'
            )",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("檢查 Cursor cursorDiskKV 表失敗: {error}"))?;
    if !has_cursor_disk_kv {
        return Err("Cursor state.vscdb 缺少 cursorDiskKV 表".to_string());
    }
    Ok(conn)
}

fn cursor_state_max_rowid(conn: &Connection) -> Result<i64, String> {
    conn.query_row(
        "SELECT COALESCE(MAX(rowid), 0) FROM cursorDiskKV",
        [],
        |row| row.get(0),
    )
    .map_err(|error| format!("讀取 Cursor cursorDiskKV 最大 rowid 失敗: {error}"))
}

fn sync_cursor_model_signatures(
    conn: &mut Connection,
    state_db_path: &Path,
) -> Result<String, String> {
    let source_id = cursor_model_source_id(state_db_path);
    let state_key = format!("cursor-agent-kv:v2:{source_id}");
    let source_conn = open_cursor_state_db(state_db_path)?;
    let max_rowid = cursor_state_max_rowid(&source_conn)?;
    let stored_rowid: i64 = conn
        .query_row(
            "SELECT last_synced_size FROM sync_state WHERE filename = ?",
            params![state_key],
            |row| row.get(0),
        )
        .unwrap_or(0);
    let reset_cache = max_rowid < stored_rowid;
    let start_rowid = if reset_cache { 0 } else { stored_rowid };

    let mut mappings = Vec::new();
    if max_rowid > start_rowid {
        let mut statement = source_conn
            .prepare(
                "SELECT CAST(value AS BLOB)
                 FROM cursorDiskKV INDEXED BY sqlite_autoindex_cursorDiskKV_1
                 WHERE rowid > ? AND rowid <= ?
                   AND key >= 'agentKv:blob:' AND key < 'agentKv:blob;'
                   AND instr(CAST(value AS TEXT), '\"modelName\"') > 0",
            )
            .map_err(|error| format!("準備 Cursor agentKv 查詢失敗: {error}"))?;
        let mut rows = statement
            .query(params![start_rowid, max_rowid])
            .map_err(|error| format!("查詢 Cursor agentKv 失敗: {error}"))?;
        while let Some(row) = rows
            .next()
            .map_err(|error| format!("讀取 Cursor agentKv 記錄失敗: {error}"))?
        {
            let raw: Vec<u8> = match row.get(0) {
                Ok(raw) => raw,
                Err(_) => continue,
            };
            if let Some(mapping) = parse_cursor_agent_kv_model_signature(&raw) {
                mappings.push(mapping);
            }
        }
    }
    drop(source_conn);

    if reset_cache || max_rowid > start_rowid {
        let has_mapping_changes = !mappings.is_empty();
        let tx = conn
            .transaction()
            .map_err(|error| format!("啟動 Cursor 模型簽章同步失敗: {error}"))?;
        if reset_cache {
            tx.execute(
                "DELETE FROM cursor_model_signatures WHERE source_id = ?",
                params![source_id],
            )
            .map_err(|error| format!("重設 Cursor 模型簽章快取失敗: {error}"))?;
            tx.execute(
                "UPDATE usage_entries
                 SET model = 'Unknown Model', model_id = 'Unknown Model'
                 WHERE assistant_type = 'cursor'
                   AND model_signature IS NOT NULL",
                [],
            )
            .map_err(|error| format!("清除過期 Cursor 模型歸因失敗: {error}"))?;
            tx.execute("DELETE FROM sync_state WHERE filename LIKE 'cursor:%'", [])
                .map_err(|error| format!("重設 Cursor 逐字稿同步狀態失敗: {error}"))?;
        }
        for (signature, model) in mappings {
            tx.execute(
                "INSERT INTO cursor_model_signatures (
                    source_id, signature, model, is_ambiguous
                 ) VALUES (?, ?, ?, 0)
                 ON CONFLICT(source_id, signature) DO UPDATE SET
                    is_ambiguous = CASE
                        WHEN cursor_model_signatures.model = excluded.model
                        THEN cursor_model_signatures.is_ambiguous
                        ELSE 1
                    END",
                params![source_id, signature, model],
            )
            .map_err(|error| format!("寫入 Cursor 模型簽章快取失敗: {error}"))?;
        }
        if reset_cache || has_mapping_changes {
            tx.execute(
                "UPDATE usage_entries
                 SET model = 'Unknown Model', model_id = 'Unknown Model'
                 WHERE assistant_type = 'cursor'
                   AND model_signature IS NOT NULL
                   AND EXISTS (
                        SELECT 1 FROM cursor_model_signatures signatures
                        WHERE signatures.source_id = ?
                          AND signatures.signature = usage_entries.model_signature
                          AND signatures.is_ambiguous = 1
                   )",
                params![source_id],
            )
            .map_err(|error| format!("清除歧義 Cursor 模型歸因失敗: {error}"))?;
            tx.execute(
                "UPDATE usage_entries
                 SET model = (
                        SELECT signatures.model FROM cursor_model_signatures signatures
                        WHERE signatures.source_id = ?
                          AND signatures.signature = usage_entries.model_signature
                          AND signatures.is_ambiguous = 0
                     ),
                     model_id = (
                        SELECT signatures.model FROM cursor_model_signatures signatures
                        WHERE signatures.source_id = ?
                          AND signatures.signature = usage_entries.model_signature
                          AND signatures.is_ambiguous = 0
                     )
                 WHERE assistant_type = 'cursor'
                   AND model_signature IS NOT NULL
                   AND EXISTS (
                        SELECT 1 FROM cursor_model_signatures signatures
                        WHERE signatures.source_id = ?
                          AND signatures.signature = usage_entries.model_signature
                          AND signatures.is_ambiguous = 0
                   )",
                params![source_id, source_id, source_id],
            )
            .map_err(|error| format!("回填 Cursor 模型歸因失敗: {error}"))?;
        }

        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        tx.execute(
            "INSERT OR REPLACE INTO sync_state (
                filename, last_synced_size, last_synced_time
             ) VALUES (?, ?, ?)",
            params![state_key, max_rowid, now],
        )
        .map_err(|error| format!("更新 Cursor agentKv 同步狀態失敗: {error}"))?;
        tx.commit()
            .map_err(|error| format!("提交 Cursor 模型簽章同步失敗: {error}"))?;
    }

    Ok(source_id)
}

fn load_cursor_model_signatures(
    conn: &Connection,
    source_id: &str,
) -> Result<HashMap<String, String>, String> {
    let mut statement = conn
        .prepare(
            "SELECT signature, model
             FROM cursor_model_signatures
             WHERE source_id = ? AND is_ambiguous = 0",
        )
        .map_err(|error| format!("準備讀取 Cursor 模型簽章快取失敗: {error}"))?;
    let rows = statement
        .query_map(params![source_id], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|error| format!("讀取 Cursor 模型簽章快取失敗: {error}"))?;
    let mut mappings = HashMap::new();
    for row in rows {
        let (signature, model) =
            row.map_err(|error| format!("解析 Cursor 模型簽章快取失敗: {error}"))?;
        mappings.insert(signature, model);
    }
    Ok(mappings)
}

fn load_cursor_ambiguous_model_signatures(
    conn: &Connection,
    source_id: &str,
) -> Result<HashSet<String>, String> {
    let mut statement = conn
        .prepare(
            "SELECT signature
             FROM cursor_model_signatures
             WHERE source_id = ? AND is_ambiguous = 1",
        )
        .map_err(|error| format!("準備讀取 Cursor 歧義模型簽章失敗: {error}"))?;
    let rows = statement
        .query_map(params![source_id], |row| row.get(0))
        .map_err(|error| format!("讀取 Cursor 歧義模型簽章失敗: {error}"))?;
    let mut signatures = HashSet::new();
    for row in rows {
        signatures.insert(row.map_err(|error| format!("解析 Cursor 歧義模型簽章失敗: {error}"))?);
    }
    Ok(signatures)
}

#[derive(Clone, Debug, Default)]
pub(super) struct CursorSessionMetadata {
    pub(super) cwd: Option<String>,
    pub(super) mode: Option<String>,
    pub(super) model: Option<String>,
}

pub(super) fn parse_cursor_session_metadata(
    key: &str,
    raw: &[u8],
) -> Option<(String, CursorSessionMetadata)> {
    let value: serde_json::Value = serde_json::from_slice(raw).ok()?;
    let session_id = value
        .get("composerId")
        .and_then(|item| item.as_str())
        .or_else(|| key.strip_prefix("composerData:"))
        .map(str::trim)
        .filter(|item| !item.is_empty() && item.len() <= 200)?
        .to_string();
    let cwd = value
        .pointer("/workspaceIdentifier/uri/fsPath")
        .or_else(|| value.pointer("/workspaceIdentifier/fsPath"))
        .or_else(|| value.pointer("/workspaceIdentifier/uri/path"))
        .and_then(|item| item.as_str())
        .map(str::trim)
        .filter(|item| !item.is_empty() && item.len() <= 4096)
        .map(str::to_string);
    let unified_mode = value
        .get("unifiedMode")
        .and_then(|item| item.as_str())
        .map(str::trim)
        .filter(|item| !item.is_empty());
    let is_agentic = value.get("isAgentic").and_then(|item| item.as_bool());
    let mode = if is_agentic == Some(false)
        || unified_mode.is_some_and(|item| !item.eq_ignore_ascii_case("agent"))
    {
        Some("ide".to_string())
    } else if is_agentic == Some(true)
        || unified_mode.is_some_and(|item| item.eq_ignore_ascii_case("agent"))
    {
        Some("agent".to_string())
    } else {
        None
    };
    let model = value
        .pointer("/modelConfig/modelName")
        .and_then(|item| item.as_str())
        .map(str::trim)
        .filter(|item| {
            !item.is_empty()
                && item.len() <= 200
                && !item.eq_ignore_ascii_case("default")
                && !item.eq_ignore_ascii_case("auto")
                && !item.eq_ignore_ascii_case("unknown model")
        })
        .map(str::to_string);

    if cwd.is_none() && mode.is_none() && model.is_none() {
        return None;
    }
    Some((session_id, CursorSessionMetadata { cwd, mode, model }))
}

fn sync_cursor_session_metadata(
    conn: &mut Connection,
    state_db_path: &Path,
) -> Result<String, String> {
    let source_id = cursor_model_source_id(state_db_path);
    let state_key = format!("cursor-composer-data:v3:{source_id}");
    let source_conn = open_cursor_state_db(state_db_path)?;
    let max_rowid = cursor_state_max_rowid(&source_conn)?;
    let stored_rowid: i64 = conn
        .query_row(
            "SELECT last_synced_size FROM sync_state WHERE filename = ?",
            params![state_key],
            |row| row.get(0),
        )
        .unwrap_or(0);
    let reset_cache = max_rowid < stored_rowid;
    let start_rowid = if reset_cache { 0 } else { stored_rowid };

    let mut metadata_rows = Vec::new();
    if max_rowid > start_rowid {
        let mut statement = source_conn
            .prepare(
                "SELECT key, CAST(value AS BLOB)
                 FROM cursorDiskKV INDEXED BY sqlite_autoindex_cursorDiskKV_1
                 WHERE rowid > ? AND rowid <= ?
                   AND key >= 'composerData:' AND key < 'composerData;'",
            )
            .map_err(|error| format!("準備 Cursor composerData 查詢失敗: {error}"))?;
        let mut rows = statement
            .query(params![start_rowid, max_rowid])
            .map_err(|error| format!("查詢 Cursor composerData 失敗: {error}"))?;
        while let Some(row) = rows
            .next()
            .map_err(|error| format!("讀取 Cursor composerData 記錄失敗: {error}"))?
        {
            let key: String = match row.get(0) {
                Ok(key) => key,
                Err(_) => continue,
            };
            let raw: Vec<u8> = match row.get(1) {
                Ok(raw) => raw,
                Err(_) => continue,
            };
            if let Some(metadata) = parse_cursor_session_metadata(&key, &raw) {
                metadata_rows.push(metadata);
            }
        }
    }
    drop(source_conn);

    if reset_cache || max_rowid > start_rowid {
        let has_metadata_changes = !metadata_rows.is_empty();
        let tx = conn
            .transaction()
            .map_err(|error| format!("啟動 Cursor Session 中繼資料同步失敗: {error}"))?;
        if reset_cache {
            tx.execute(
                "DELETE FROM cursor_session_metadata WHERE source_id = ?",
                params![source_id],
            )
            .map_err(|error| format!("重設 Cursor Session 中繼資料快取失敗: {error}"))?;
            tx.execute("DELETE FROM sync_state WHERE filename LIKE 'cursor:%'", [])
                .map_err(|error| format!("重設 Cursor 逐字稿同步狀態失敗: {error}"))?;
        }
        for (session_id, metadata) in metadata_rows {
            tx.execute(
                "INSERT INTO cursor_session_metadata (
                    source_id, session_id, cwd, mode, model
                 ) VALUES (?, ?, ?, ?, ?)
                 ON CONFLICT(source_id, session_id) DO UPDATE SET
                    cwd = COALESCE(excluded.cwd, cursor_session_metadata.cwd),
                    mode = COALESCE(excluded.mode, cursor_session_metadata.mode),
                    model = COALESCE(excluded.model, cursor_session_metadata.model)",
                params![
                    source_id,
                    session_id,
                    metadata.cwd,
                    metadata.mode,
                    metadata.model
                ],
            )
            .map_err(|error| format!("寫入 Cursor Session 中繼資料快取失敗: {error}"))?;
        }
        if reset_cache || has_metadata_changes {
            tx.execute(
                "UPDATE usage_entries
                 SET cwd = (
                        SELECT metadata.cwd FROM cursor_session_metadata metadata
                        WHERE metadata.source_id = ?
                          AND metadata.session_id = usage_entries.session_id
                     )
                 WHERE assistant_type = 'cursor'
                   AND EXISTS (
                        SELECT 1 FROM cursor_session_metadata metadata
                        WHERE metadata.source_id = ?
                          AND metadata.session_id = usage_entries.session_id
                          AND metadata.cwd IS NOT NULL
                          AND metadata.cwd != ''
                   )",
                params![source_id, source_id],
            )
            .map_err(|error| format!("回填 Cursor 工作路徑失敗: {error}"))?;
            tx.execute(
                "DELETE FROM usage_entries
                 WHERE rowid IN (
                    SELECT legacy.rowid
                    FROM usage_entries legacy
                    JOIN cursor_session_metadata metadata
                      ON metadata.source_id = ?
                     AND metadata.session_id = legacy.session_id
                    JOIN usage_entries classified
                      ON classified.assistant_type = legacy.assistant_type
                     AND classified.session_id = legacy.session_id
                     AND classified.turn_no = legacy.turn_no
                     AND classified.source_kind = CASE metadata.mode
                        WHEN 'agent' THEN ?
                        WHEN 'ide' THEN ?
                     END
                    WHERE legacy.assistant_type = 'cursor'
                      AND legacy.source_kind = 'legacy'
                 )",
                params![source_id, CURSOR_AGENT_SOURCE_KIND, CURSOR_IDE_SOURCE_KIND],
            )
            .map_err(|error| format!("清除 Cursor legacy 重複記錄失敗: {error}"))?;
            tx.execute(
                "UPDATE usage_entries
                 SET source_kind = CASE (
                        SELECT metadata.mode FROM cursor_session_metadata metadata
                        WHERE metadata.source_id = ?
                          AND metadata.session_id = usage_entries.session_id
                     )
                        WHEN 'agent' THEN ?
                        WHEN 'ide' THEN ?
                        ELSE source_kind
                     END
                 WHERE assistant_type = 'cursor'
                   AND EXISTS (
                        SELECT 1 FROM cursor_session_metadata metadata
                        WHERE metadata.source_id = ?
                          AND metadata.session_id = usage_entries.session_id
                          AND metadata.mode IN ('agent', 'ide')
                   )",
                params![
                    source_id,
                    CURSOR_AGENT_SOURCE_KIND,
                    CURSOR_IDE_SOURCE_KIND,
                    source_id
                ],
            )
            .map_err(|error| format!("回填 Cursor Session 模式失敗: {error}"))?;
            tx.execute(
                "UPDATE usage_entries
                 SET model = (
                        SELECT metadata.model FROM cursor_session_metadata metadata
                        WHERE metadata.source_id = ?
                          AND metadata.session_id = usage_entries.session_id
                     ),
                     model_id = (
                        SELECT metadata.model FROM cursor_session_metadata metadata
                        WHERE metadata.source_id = ?
                          AND metadata.session_id = usage_entries.session_id
                     )
                 WHERE assistant_type = 'cursor'
                   AND EXISTS (
                        SELECT 1 FROM cursor_session_metadata metadata
                        WHERE metadata.source_id = ?
                          AND metadata.session_id = usage_entries.session_id
                          AND metadata.model IS NOT NULL
                          AND metadata.model != ''
                   )
                   AND NOT EXISTS (
                        SELECT 1 FROM cursor_model_signatures signatures
                        WHERE signatures.source_id = ?
                          AND signatures.signature = usage_entries.model_signature
                   )",
                params![source_id, source_id, source_id, source_id],
            )
            .map_err(|error| format!("回填 Cursor Session 模型 fallback 失敗: {error}"))?;
        }

        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        tx.execute(
            "INSERT OR REPLACE INTO sync_state (
                filename, last_synced_size, last_synced_time
             ) VALUES (?, ?, ?)",
            params![state_key, max_rowid, now],
        )
        .map_err(|error| format!("更新 Cursor composerData 同步狀態失敗: {error}"))?;
        tx.commit()
            .map_err(|error| format!("提交 Cursor Session 中繼資料同步失敗: {error}"))?;
    }

    Ok(source_id)
}

fn load_cursor_session_metadata(
    conn: &Connection,
    source_id: &str,
) -> Result<HashMap<String, CursorSessionMetadata>, String> {
    let mut statement = conn
        .prepare(
            "SELECT session_id, cwd, mode, model
             FROM cursor_session_metadata
             WHERE source_id = ?",
        )
        .map_err(|error| format!("準備讀取 Cursor Session 中繼資料失敗: {error}"))?;
    let rows = statement
        .query_map(params![source_id], |row| {
            Ok((
                row.get(0)?,
                CursorSessionMetadata {
                    cwd: row.get(1)?,
                    mode: row.get(2)?,
                    model: row.get(3)?,
                },
            ))
        })
        .map_err(|error| format!("讀取 Cursor Session 中繼資料失敗: {error}"))?;
    let mut mappings = HashMap::new();
    for row in rows {
        let (session_id, metadata) =
            row.map_err(|error| format!("解析 Cursor Session 中繼資料失敗: {error}"))?;
        mappings.insert(session_id, metadata);
    }
    Ok(mappings)
}

pub(super) struct CursorParsedEntry {
    pub(super) entry: UsageEntry,
    pub(super) model_signature: Option<String>,
}

pub(super) fn parse_cursor_session_file(
    filepath: &Path,
    model_mappings: &HashMap<String, String>,
    ambiguous_model_signatures: &HashSet<String>,
    session_metadata: &HashMap<String, CursorSessionMetadata>,
) -> Result<Vec<CursorParsedEntry>, String> {
    let file = File::open(filepath).map_err(|e| format!("無法開啟檔案: {}", e))?;
    let reader = BufReader::new(file);
    let fallback_session_id = filepath
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("unknown-session")
        .to_string();
    let metadata = session_metadata.get(&fallback_session_id);
    let session_cwd = metadata.and_then(|value| value.cwd.clone());
    let source_kind = cursor_mode_source_kind(metadata.and_then(|value| value.mode.as_deref()));

    let mut session_name_selector = InitialUserPromptSelector::default();
    let mut results = Vec::new();

    let mut current_timestamp = String::new();
    let mut current_prompt = String::new();

    for line_res in reader.lines() {
        let line = match line_res {
            Ok(line) => line,
            Err(_) => continue,
        };
        let event: serde_json::Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => continue,
        };

        let role = event.get("role").and_then(|r| r.as_str()).unwrap_or("");

        if role == "user" {
            let content_val = event.get("message").and_then(|m| m.get("content"));
            let text = cursor_content_to_text(content_val.unwrap_or(&serde_json::Value::Null));

            let mut extracted_ts = String::new();
            if let Some(start_idx) = text.find("<timestamp>") {
                let actual_start = start_idx + "<timestamp>".len();
                if let Some(end_idx) = text[actual_start..].find("</timestamp>") {
                    extracted_ts = text[actual_start..(actual_start + end_idx)].to_string();
                }
            }

            if !extracted_ts.is_empty() {
                let parsed_timestamp = parse_cursor_timestamp(&extracted_ts);
                if cursor_date_from_timestamp(&parsed_timestamp).is_some() {
                    current_timestamp = parsed_timestamp;
                }
            }

            let mut clean_prompt = text.clone();
            if let Some(start_idx) = clean_prompt.find("<user_query>") {
                let actual_start = start_idx + "<user_query>".len();
                if let Some(end_idx) = clean_prompt[actual_start..].find("</user_query>") {
                    clean_prompt = clean_prompt[actual_start..(actual_start + end_idx)].to_string();
                }
            }

            current_prompt = clean_prompt.trim().to_string();
            session_name_selector.observe_user_prompt(&current_prompt);
        } else if role == "assistant" {
            session_name_selector.observe_non_user_message();
            let content_val = event.get("message").and_then(|m| m.get("content"));
            let reply_text =
                cursor_content_to_text(content_val.unwrap_or(&serde_json::Value::Null));
            let current_model_signature =
                cursor_response_signature(content_val.unwrap_or(&serde_json::Value::Null));
            let current_model = match current_model_signature.as_ref() {
                Some(signature) if ambiguous_model_signatures.contains(signature) => {
                    "Unknown Model".to_string()
                }
                Some(signature) => model_mappings
                    .get(signature)
                    .cloned()
                    .or_else(|| metadata.and_then(|value| value.model.clone()))
                    .unwrap_or_else(|| "Unknown Model".to_string()),
                None => metadata
                    .and_then(|value| value.model.clone())
                    .unwrap_or_else(|| "Unknown Model".to_string()),
            };

            if current_timestamp.is_empty() {
                if let Ok(metadata) = filepath.metadata() {
                    if let Ok(modified) = metadata.modified() {
                        let datetime: chrono::DateTime<chrono::Utc> = modified.into();
                        current_timestamp = datetime.format("%Y-%m-%d %H:%M:%S").to_string();
                    }
                }
            }
            if current_timestamp.is_empty() {
                current_timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
            }

            let input_tokens = (current_prompt.len() / 4).max(10) as u64;
            let output_tokens = (reply_text.len() / 4).max(10) as u64;
            let total_tokens = input_tokens + output_tokens;

            let tokens = TokenStats {
                input: input_tokens,
                output: output_tokens,
                // Cursor transcripts do not expose cache token counts.
                cache_read: None,
                cache_write: None,
                cache_write_5m: None,
                cache_write_1h: None,
                reasoning: None,
                total: total_tokens,
            };

            results.push(CursorParsedEntry {
                entry: UsageEntry {
                    timestamp: current_timestamp.clone(),
                    session_id: fallback_session_id.clone(),
                    session_name: session_name_selector
                        .selected_name()
                        .map(str::to_string)
                        .or_else(|| Some(fallback_session_id.clone())),
                    transcript_path: Some(filepath.to_string_lossy().into_owned()),
                    cwd: session_cwd.clone(),
                    version: None,
                    turn_no: (results.len() + 1) as u32,
                    model: Some(current_model.clone()),
                    model_id: Some(current_model.clone()),
                    tokens: Some(tokens.clone()),
                    delta_tokens: Some(tokens),
                    context: None,
                    cost: None,
                    source_kind: source_kind.clone(),
                    source_dir_key: None,
                    parent_session_id: None,
                    agent_nickname: None,
                    agent_role: None,
                    reasoning_effort: None,
                },
                model_signature: current_model_signature,
            });
        }
    }

    Ok(results)
}

pub(super) fn sync_cursor_usage_logs(
    conn: &mut Connection,
    cursor_dir: &Path,
) -> Result<(), String> {
    run_cursor_model_attribution_migration(conn)?;
    run_cursor_cache_tokens_unknown_migration(conn)?;

    let state_db_path = get_cursor_state_db_path();
    let source_id = if state_db_path.exists() {
        let source_id = cursor_model_source_id(&state_db_path);
        if let Err(error) = sync_cursor_session_metadata(conn, &state_db_path) {
            eprintln!("同步 Cursor composerData Session 中繼資料失敗: {error}");
        }
        if let Err(error) = sync_cursor_model_signatures(conn, &state_db_path) {
            eprintln!("同步 Cursor agentKv 模型資訊失敗: {error}");
        }
        Some(source_id)
    } else {
        None
    };
    let model_mappings = if let Some(source_id) = source_id.as_deref() {
        load_cursor_model_signatures(conn, source_id)?
    } else {
        HashMap::new()
    };
    let ambiguous_model_signatures = if let Some(source_id) = source_id.as_deref() {
        load_cursor_ambiguous_model_signatures(conn, source_id)?
    } else {
        HashSet::new()
    };
    let session_metadata = if let Some(source_id) = source_id.as_deref() {
        load_cursor_session_metadata(conn, source_id)?
    } else {
        HashMap::new()
    };

    let projects_dir = cursor_dir.join("projects");
    if !projects_dir.exists() {
        return Ok(());
    }

    let files = find_cursor_session_files(&projects_dir);

    for filepath in files {
        let state_path = filepath
            .strip_prefix(cursor_dir)
            .unwrap_or(&filepath)
            .to_string_lossy()
            .into_owned();
        let state_key = format!("cursor:{}", state_path);

        let last_synced_size: u64 = conn
            .query_row(
                "SELECT last_synced_size FROM sync_state WHERE filename = ?",
                params![state_key],
                |row| row.get(0),
            )
            .unwrap_or(0u64);

        let metadata = match fs::metadata(&filepath) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let current_size = metadata.len();

        if current_size != last_synced_size {
            let parsed_entries = match parse_cursor_session_file(
                &filepath,
                &model_mappings,
                &ambiguous_model_signatures,
                &session_metadata,
            ) {
                Ok(entries) => entries,
                Err(e) => {
                    eprintln!("解析 Cursor 會話檔案 {:?} 失敗: {}", filepath, e);
                    continue;
                }
            };

            let tx = conn
                .transaction()
                .map_err(|e| format!("Transaction BEGIN 失敗: {}", e))?;

            let session_ids: HashSet<String> = parsed_entries
                .iter()
                .map(|parsed| parsed.entry.session_id.clone())
                .collect();
            for session_id in session_ids {
                let delete_res = tx.execute(
                    "DELETE FROM usage_entries WHERE assistant_type = 'cursor' AND session_id = ?",
                    params![session_id],
                );

                if let Err(e) = delete_res {
                    eprintln!("清空舊 Cursor Session 資料失敗: {}", e);
                    continue;
                }
            }

            let mut success = true;
            for parsed in &parsed_entries {
                let entry = &parsed.entry;
                let tokens = entry.tokens.as_ref();
                let delta = entry.delta_tokens.as_ref();
                let cost = entry.cost.as_ref();
                let entry_date = cursor_date_from_timestamp(&entry.timestamp).ok_or_else(|| {
                    format!(
                        "Cursor Session {} 的時間戳記無有效日期: {}",
                        entry.session_id, entry.timestamp
                    )
                })?;

                let insert_res = tx.execute(
                    "INSERT INTO usage_entries (
                        assistant_type, timestamp, date, session_id, session_name, transcript_path, cwd, version, turn_no, model, model_id, model_signature,
                        tokens_input, tokens_output, tokens_cache_read, tokens_cache_write, tokens_cache_write_5m, tokens_cache_write_1h, tokens_reasoning, tokens_total,
                        delta_input, delta_output, delta_cache_read, delta_cache_write, delta_cache_write_5m, delta_cache_write_1h, delta_reasoning, delta_total,
                        duration_ms, premium_requests, parent_session_id, agent_nickname, agent_role, reasoning_effort, source_kind
                    ) VALUES (
                        ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                        ?, ?, ?, ?, ?, ?, ?, ?,
                        ?, ?, ?, ?, ?, ?, ?, ?,
                        ?, ?, ?, ?, ?, ?, ?
                    )",
                    params![
                        "cursor",
                        entry.timestamp,
                        entry_date,
                        entry.session_id,
                        entry.session_name.as_deref(),
                        entry.transcript_path.as_deref(),
                        entry.cwd.as_deref(),
                        entry.version.as_deref(),
                        entry.turn_no as i64,
                        entry.model.as_deref(),
                        entry.model_id.as_deref(),
                        parsed.model_signature.as_deref(),
                        tokens.map(|t| t.input as i64),
                        tokens.map(|t| t.output as i64),
                        tokens.and_then(|t| t.cache_read.map(|v| v as i64)),
                        tokens.and_then(|t| t.cache_write.map(|v| v as i64)),
                        tokens.and_then(|t| t.cache_write_5m.map(|v| v as i64)),
                        tokens.and_then(|t| t.cache_write_1h.map(|v| v as i64)),
                        tokens.and_then(|t| t.reasoning.map(|v| v as i64)),
                        tokens.map(|t| t.total as i64),
                        delta.map(|t| t.input as i64),
                        delta.map(|t| t.output as i64),
                        delta.and_then(|t| t.cache_read.map(|v| v as i64)),
                        delta.and_then(|t| t.cache_write.map(|v| v as i64)),
                        delta.and_then(|t| t.cache_write_5m.map(|v| v as i64)),
                        delta.and_then(|t| t.cache_write_1h.map(|v| v as i64)),
                        delta.and_then(|t| t.reasoning.map(|v| v as i64)),
                        delta.map(|t| t.total as i64),
                        cost.and_then(|c| c.total_api_duration_ms.map(|d| d as i64)),
                        cost.and_then(|c| c.total_premium_requests.map(|r| r as i64)),
                        entry.parent_session_id.as_deref(),
                        entry.agent_nickname.as_deref(),
                        entry.agent_role.as_deref(),
                        entry.reasoning_effort.as_deref(),
                        entry.source_kind.as_deref().unwrap_or("cursor")
                    ],
                );

                if let Err(e) = insert_res {
                    eprintln!("寫入 Cursor 資料庫失敗 (turn_no {}): {}", entry.turn_no, e);
                    success = false;
                    break;
                }
            }

            if success {
                let now = SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64;

                let update_state_res = tx.execute(
                    "INSERT OR REPLACE INTO sync_state (filename, last_synced_size, last_synced_time) VALUES (?, ?, ?)",
                    params![state_key, current_size as i64, now],
                );

                if update_state_res.is_ok() {
                    if let Err(e) = tx.commit() {
                        eprintln!("Transaction COMMIT 失敗: {}", e);
                    }
                }
            }
        }
    }

    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;

    fn temp_jsonl_path(prefix: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{}-{unique}.jsonl", std::process::id()))
    }

    #[test]
    fn parse_cursor_session_file_uses_last_initial_consecutive_user_prompt_as_name() {
        let path = temp_jsonl_path("cursor-session-name");
        let content = r#"{"role":"user","message":{"content":"第一條提示"}}
{"role":"user","message":{"content":"第二條提示"}}
{"role":"assistant","message":{"content":"收到"}}
{"role":"user","message":{"content":"後續提示"}}
{"role":"assistant","message":{"content":"完成"}}
"#;

        fs::write(&path, content).unwrap();
        let entries =
            parse_cursor_session_file(&path, &HashMap::new(), &HashSet::new(), &HashMap::new())
                .unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(entries.len(), 2);
        assert!(entries
            .iter()
            .all(|entry| entry.entry.session_name.as_deref() == Some("第二條提示")));
    }

    #[test]
    fn cursor_response_signature_matches_plain_text_agent_kv_content() {
        let transcript_content = serde_json::json!([
            {
                "type": "text",
                "text": "Plain answer"
            }
        ]);
        let agent_kv_content = serde_json::json!([
            {
                "type": "text",
                "data": "Plain answer",
                "providerOptions": {
                    "cursor": {
                        "modelName": "composer-2.5"
                    }
                }
            }
        ]);

        assert_eq!(
            cursor_response_signature(&transcript_content),
            cursor_response_signature(&agent_kv_content)
        );
    }

    #[test]
    fn cursor_response_signature_matches_agent_kv_tool_calls() {
        let transcript_content = serde_json::json!([
            {
                "type": "text",
                "text": "Running"
            },
            {
                "type": "tool_use",
                "name": "Shell",
                "input": {
                    "command": "echo hi",
                    "block_until_ms": 120_000
                }
            }
        ]);
        let agent_kv_event = serde_json::json!({
            "role": "assistant",
            "content": [
                {
                    "type": "text",
                    "data": "Running",
                    "providerOptions": {
                        "cursor": {
                            "modelName": "composer-2.5"
                        }
                    }
                },
                {
                    "type": "tool-call",
                    "toolName": "Shell",
                    "args": {
                        "block_until_ms": 120_000,
                        "command": "echo hi"
                    }
                }
            ]
        });
        let raw = serde_json::to_vec(&agent_kv_event).unwrap();
        let (signature, model) = parse_cursor_agent_kv_model_signature(&raw).unwrap();

        assert_eq!(
            Some(signature),
            cursor_response_signature(&transcript_content)
        );
        assert_eq!(model, "composer-2.5");
    }

    #[test]
    fn cursor_parser_does_not_reuse_a_previous_reply_model() {
        let path = temp_jsonl_path("cursor-model-reset");
        let content = r#"{"role":"user","message":{"content":"Prompt"}}
{"role":"assistant","message":{"content":[{"type":"text","text":"Known reply"}]}}
{"role":"assistant","message":{"content":[{"type":"image","data":"omitted"}]}}
"#;
        fs::write(&path, content).unwrap();
        let signature = cursor_response_signature(
            &serde_json::json!([{"type": "text", "text": "Known reply"}]),
        )
        .unwrap();
        let model_mappings = HashMap::from([(signature, "composer-2.5".to_string())]);

        let entries =
            parse_cursor_session_file(&path, &model_mappings, &HashSet::new(), &HashMap::new())
                .unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].entry.model.as_deref(), Some("composer-2.5"));
        assert!(entries[0].model_signature.is_some());
        assert_eq!(entries[1].entry.model.as_deref(), Some("Unknown Model"));
        assert!(entries[1].model_signature.is_none());
    }

    #[test]
    fn cursor_session_metadata_treats_non_agent_modes_as_ide() {
        let (_, false_overrides_agent_mode) = parse_cursor_session_metadata(
            "composerData:false-overrides-agent",
            br#"{
                "composerId": "false-overrides-agent",
                "unifiedMode": "agent",
                "isAgentic": false
            }"#,
        )
        .unwrap();
        let (_, non_agent_mode_overrides_true) = parse_cursor_session_metadata(
            "composerData:chat-overrides-true",
            br#"{
                "composerId": "chat-overrides-true",
                "unifiedMode": "chat",
                "isAgentic": true
            }"#,
        )
        .unwrap();
        let (_, agent_mode) = parse_cursor_session_metadata(
            "composerData:agent",
            br#"{
                "composerId": "agent",
                "unifiedMode": "agent",
                "isAgentic": true
            }"#,
        )
        .unwrap();

        assert_eq!(false_overrides_agent_mode.mode.as_deref(), Some("ide"));
        assert_eq!(non_agent_mode_overrides_true.mode.as_deref(), Some("ide"));
        assert_eq!(agent_mode.mode.as_deref(), Some("agent"));
    }

    #[test]
    fn cursor_session_metadata_uses_only_concrete_model_configs() {
        let (_, concrete_model) = parse_cursor_session_metadata(
            "composerData:concrete-model",
            br#"{
                "composerId": "concrete-model",
                "unifiedMode": "agent",
                "modelConfig": { "modelName": "composer-2.5" }
            }"#,
        )
        .unwrap();
        let (_, default_model) = parse_cursor_session_metadata(
            "composerData:default-model",
            br#"{
                "composerId": "default-model",
                "unifiedMode": "agent",
                "modelConfig": { "modelName": "default" }
            }"#,
        )
        .unwrap();

        assert_eq!(concrete_model.model.as_deref(), Some("composer-2.5"));
        assert!(default_model.model.is_none());
    }

    #[test]
    fn cursor_state_db_reader_observes_uncheckpointed_wal() {
        let state_db_path = temp_jsonl_path("cursor-state-wal").with_extension("vscdb");
        let writer = Connection::open(&state_db_path).unwrap();
        let journal_mode: String = writer
            .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
            .unwrap();
        assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
        writer
            .execute_batch(
                "PRAGMA wal_autocheckpoint = 0;
                 CREATE TABLE cursorDiskKV (
                    key TEXT PRIMARY KEY,
                    value BLOB
                 );
                 INSERT INTO cursorDiskKV (key, value)
                 VALUES ('agentKv:blob:test', X'7B7D');",
            )
            .unwrap();

        let wal_path = PathBuf::from(format!("{}-wal", state_db_path.to_string_lossy()));
        assert!(wal_path.exists(), "fixture must keep committed data in WAL");

        let reader = open_cursor_state_db(&state_db_path).unwrap();
        let row_count: i64 = reader
            .query_row("SELECT COUNT(*) FROM cursorDiskKV", [], |row| row.get(0))
            .unwrap();
        assert_eq!(row_count, 1, "read-only connection must observe WAL data");

        drop(reader);
        drop(writer);
        let _ = fs::remove_file(&state_db_path);
        let _ = fs::remove_file(wal_path);
        let _ = fs::remove_file(format!("{}-shm", state_db_path.to_string_lossy()));
    }

    #[test]
    fn cursor_cache_token_migration_marks_legacy_values_unknown() {
        let mut conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, timestamp, date, session_id, turn_no,
                tokens_cache_read, tokens_cache_write,
                tokens_cache_write_5m, tokens_cache_write_1h,
                delta_cache_read, delta_cache_write,
                delta_cache_write_5m, delta_cache_write_1h
             ) VALUES (
                'cursor', '2026-07-24T00:00:00Z', '2026-07-24',
                'cursor-cache-session', 1, 0, 0, 0, 0, 0, 0, 0, 0
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO usage_entries (
                assistant_type, timestamp, date, session_id, turn_no,
                tokens_cache_read, tokens_cache_write,
                tokens_cache_write_5m, tokens_cache_write_1h,
                delta_cache_read, delta_cache_write,
                delta_cache_write_5m, delta_cache_write_1h
             ) VALUES (
                'cursor', '2026-07-24T00:01:00Z', '2026-07-24',
                'cursor-cache-measured', 1, 10, 20, 30, 40, 1, 2, 3, 4
             )",
            [],
        )
        .unwrap();

        run_cursor_cache_tokens_unknown_migration(&mut conn).unwrap();
        run_cursor_cache_tokens_unknown_migration(&mut conn).unwrap();

        let values: [Option<u64>; 8] = conn
            .query_row(
                "SELECT
                    tokens_cache_read, tokens_cache_write,
                    tokens_cache_write_5m, tokens_cache_write_1h,
                    delta_cache_read, delta_cache_write,
                    delta_cache_write_5m, delta_cache_write_1h
                 FROM usage_entries
                 WHERE session_id = 'cursor-cache-session'",
                [],
                |row| {
                    Ok([
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ])
                },
            )
            .unwrap();
        let migration_count: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_state WHERE filename = ?",
                params![CURSOR_CACHE_TOKENS_UNKNOWN_MIGRATION_KEY],
                |row| row.get(0),
            )
            .unwrap();
        let measured_values: [u64; 8] = conn
            .query_row(
                "SELECT
                    tokens_cache_read, tokens_cache_write,
                    tokens_cache_write_5m, tokens_cache_write_1h,
                    delta_cache_read, delta_cache_write,
                    delta_cache_write_5m, delta_cache_write_1h
                 FROM usage_entries
                 WHERE session_id = 'cursor-cache-measured'",
                [],
                |row| {
                    Ok([
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ])
                },
            )
            .unwrap();

        assert_eq!(values, [None; 8]);
        assert_eq!(measured_values, [10, 20, 30, 40, 1, 2, 3, 4]);
        assert_eq!(migration_count, 1);
    }

    #[test]
    fn test_parse_cursor_timestamp() {
        let ts = "Wednesday, Jul 8, 2026, 2:24 AM (UTC+8)";
        let parsed = parse_cursor_timestamp(ts);
        assert_eq!(parsed, "2026-07-08T02:24:00+08:00");
        assert_eq!(cursor_date_from_timestamp(&parsed), Some("2026-07-08"));
        assert_eq!(cursor_date_from_timestamp("unknown"), None);
    }
}
