# Copilot App Token Collector — 修改計畫

## 背景

Token 戰情室的 Copilot 頁面目前只收兩種來源：
1. **Copilot CLI** — 透過 `~/.copilot/statusline-token.sh` 寫入 `~/.copilot/usage/usage-YYYY-MM-DD.jsonl`
2. **VS Code Copilot Chat** — 掃描 `workspaceStorage/chatSessions`

**缺口**：Copilot App（Tauri 桌面應用 / 本機 GUI session）的 token 使用量沒被納入。它把資料寫在兩個 SQLite，而不是 JSONL：

| SQLite | 關鍵表 | 內含欄位 |
|---|---|---|
| `~/.copilot/data.db` | `sessions` | `id, title, session_type, model, total_input_tokens, total_output_tokens, total_cached_tokens, total_reasoning_tokens, total_nano_aiu, created_at, updated_at, execution_location, agent, provider_id` |
| `~/.copilot/data.db` | `session_context_usage` | `session_id, ts_ms, current_tokens, token_limit`（時間序列，可用於即時 context 量） |
| `~/.copilot/session-store.db` | `assistant_usage_events` | `id, session_id, turn_index, agent_id, parent_tool_call_id, model, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens, total_nano_aiu, request_multiplier, duration_ms, time_to_first_token_ms, inter_token_latency_ms, initiator, api_endpoint, reasoning_effort, finish_reason, content_filter_triggered, token_details_json, created_at` |

關鍵發現：`src/db.rs` 已有 `get_copilot_session_name()` 會去讀 `~/.copilot/session-state/<id>/events.jsonl` 找 session 標題，證明專案已經知道 `~/.copilot/` 結構，但沒有任何函式讀 `data.db` 或 `session-store.db` 的 usage 表。

## 設計決策

### A. 新增 source kind：`copilot-app`

- 不引入新的 `assistant_type`，避免動到 `is_supported_assistant` 與前端 5 個 badge 的排列。
- 沿用 `assistant_type = "copilot"`，新增 `source_kind = "copilot-app"`，與現有 `copilot-cli`、`vscode-chat`（如有）、`legacy` 並列。
- 前端在 Copilot 頁面的 session 清單以來源標籤區分 `CLI` / `VS Code` / `App`。

### B. 資料對齊策略

Copilot App 的 `assistant_usage_events` 是 per-API-call 顆粒度，而 Token 戰情室的 `usage_entries` 是 per-turn 顆粒度。對齊方式：

- **以 `(session_id, turn_index)` 為 key 做 group-by**：同一個 turn 內可能有多個 API call（例如 tool call 後再回答），把它們的 token 加總成一筆 `UsageEntry`。
- `timestamp` 用該 turn 最早事件的 `created_at`。
- `model` 取該 turn 內最多 token 的 event 的 model。
- `tokens.input / output / cache_read / cache_write / reasoning` 直接加總。
- `delta_tokens`：第一個 turn 等於 tokens；後續 turn 為 `current - previous`（參考 `statusline-token.sh` 的差值邏輯）。
- `total_tokens` = input + output + cache_read + cache_write + reasoning（與 JSONL 邏輯一致）。
- `cwd` 取 `data.db.sessions` 表的 workspace/checkout 路徑（需 join `workspaces` / `project_checkouts`，或退而求其次用 `session-state/<id>/` 內的檔案推導）。
- `session_name` 取 `data.db.sessions.title`；若空則 fallback 到現有 `get_copilot_session_name()`。
- `turn_no` = `turn_index + 1`。
- `reasoning_effort` 取 event 的 `reasoning_effort`（全 turn 應一致）。
- `cost.total_api_duration_ms` 取 turn 內所有 event `duration_ms` 加總。

### C. 增量同步

- 在 `usage_entries` 表新增一個 `import_source_id` 形式：`copilot-app:<session_id>:<turn_index>`，用來做重複偵測。
- 已存在的 `import_source_id` 跳過，避免重複寫入。
- 每次同步只讀 `created_at > last_synced_at` 的新 event（用一個 `session_state` key 記錄上次同步的 max `created_at`）。
- 同步頻率沿用現有 5 秒背景循環。

### D. 環境變數

新增 `COPILOT_APP_DIR`（預設 `~/.copilot`），供未來 App 與 CLI 分家時使用。`data.db` 與 `session-store.db` 都從這個目錄解析。

## 修改清單

### 1. `src/paths.rs`
- 新增 `pub fn copilot_app_dir() -> PathBuf`：讀 `COPILOT_APP_DIR`，fallback `COPILOT_DIR`，再 fallback `~/.copilot`。

### 2. `src/db.rs`

#### 2.1 新增常數與同步 key
- `const COPILOT_APP_SOURCE_KIND: &str = "copilot-app";`
- `const COPILOT_APP_LAST_SYNC_KEY: &str = "sync:copilot_app:last_created_at";`

#### 2.2 新增函式 `sync_copilot_app_usage_logs(conn: &mut Connection) -> Result<(), String>`

步驟：
1. `let app_dir = crate::paths::copilot_app_dir();`
2. 開 `app_dir.join("session-store.db")` 為唯讀 `Connection`。若檔不存在，return `Ok(())`。
3. 開 `app_dir.join("data.db")` 為唯讀 `Connection`（用於查 `sessions` 表取 title/workspace）。
4. 從 `session_state` 讀 `last_created_at`（若有）。
5. SQL（DuckDB 不適用，這裡是 rusqlite）：
   ```sql
   SELECT session_id, turn_index, MIN(created_at) AS ts,
          SUM(input_tokens), SUM(output_tokens),
          SUM(cache_read_tokens), SUM(cache_write_tokens),
          SUM(reasoning_tokens),
          SUM(duration_ms),
          MIN(model) AS model,  -- 或用 argmax
          MIN(reasoning_effort)
   FROM assistant_usage_events
   WHERE created_at > :last
   GROUP BY session_id, turn_index
   ORDER BY ts ASC
   ```
6. 對每筆 group：
   - 用 `session_id` 查 `data.db.sessions` 取 `title`、`session_type`、`model`、`execution_location`、`agent`。
   - 組 `UsageEntry`：
     - `source_kind = Some("copilot-app".into())`
     - `session_name = Some(title)` 或 fallback
     - `cwd`：先試 `data.db.workspace_checkout_bindings` join `workspaces`，失敗則讀 `session-state/<id>/` 目錄內的第一個 `project_session_id` 路徑，再失敗用空字串。
     - `timestamp`：把 `created_at`（格式 `YYYY-MM-DD HH:MM:SS`，UTC）轉 ISO 8601 with `+00:00`。
     - `cost.total_api_duration_ms = Some(sum_duration as f64)`
   - 計算 `delta_tokens`：維護一個 `HashMap<session_id, PrevTurnTokens>`，每個 session 的第一個 turn delta = tokens，後續 turn delta = current - previous。
7. 對每筆 `UsageEntry` 呼叫既有 `insert_usage_entry_if_new(conn, entry, "copilot", "copilot-app:<session_id>:<turn_index>")`（沿用既有 dedup 邏輯）。
8. 把這次同步最大 `created_at` 寫回 `session_state` 的 `COPILOT_APP_LAST_SYNC_KEY`。

#### 2.3 在 `sync_usage_logs(conn)` 末尾插入
```rust
// 2c. Sync GitHub Copilot App (Tauri desktop) usage
if let Err(e) = sync_copilot_app_usage_logs(conn) {
    eprintln!("❌ 同步 Copilot App 失敗: {}", e);
}
```

#### 2.4 既有 `get_copilot_session_name` 不動
- 它仍然服務 CLI session；App session 的 title 從 `data.db.sessions` 拿，不會進入這條路徑。

### 3. `src/handlers/mod.rs`
- `SetupInfoResponse` 新增 `copilot_app: AssistantSetupStatus` 欄位（可選，便於前端設定頁顯示狀態）。
- `is_supported_assistant` 不動（仍是 `copilot`）。

### 4. `src/handlers/misc.rs`（`get_setup_info`）
- 計算 `copilot_app` 的 `data_path` = `<copilot_app_dir>/session-store.db`、`exists` = 該檔是否存在、`script_path` / `source_script_path` 留空（App 不需要 hook 腳本）。

### 5. `static/index.html`
- 在 Copilot badge 旁不加新 badge，但在 setup modal 新增一段「Copilot App 偵測狀態」顯示 `data.db` / `session-store.db` 是否存在。
- 新增 i18n key：`copilot_app_status_label`、`copilot_app_db_found`、`copilot_app_db_missing`。

### 6. `static/i18n.js` 與 `static/app.js`
- session 清單的 `source_kind` 顯示：`copilot-app` → `App` badge；`copilot-cli` → `CLI`；`vscode-chat` → `VS Code`。
- 不新增頂層 agent 切換。

### 7. `src/bin/token-usage-insights-cli.rs`
- `--agent` 已接受 `copilot`，不需新增；`export`/`import` 預設會涵蓋 `copilot-app` source kind，因為它走同一個 `assistant_type`。

### 8. 測試 `src/db.rs` tests module
- 新增 `fn sync_copilot_app_usage_logs_inserts_per_turn_rows()`：
  - 用 tempdir 建假 `~/.copilot/session-store.db` 與 `data.db`，寫入兩個 session 各 3 turn 的假 `assistant_usage_events`。
  - 設 `COPILOT_APP_DIR` 指向 tempdir，呼叫 `sync_copilot_app_usage_logs(&mut conn)`。
  - 驗證 `usage_entries` 有 6 筆 `assistant_type='copilot' AND source_kind='copilot-app'`，且 `delta_tokens` 計算正確。
  - 再呼叫一次，驗證 dedup（應 0 筆新增）。

### 9. `README.md`
- 在「支援功能」與「GitHub Copilot CLI 設定」之間新增一段「GitHub Copilot App（桌面應用）」，說明：
  - 不需要任何設定，看板會自動讀取 `~/.copilot/data.db` 與 `~/.copilot/session-store.db`。
  - Session 清單會以 `App` 標示來源。
  - 支援 `COPILOT_APP_DIR` 環境變數自訂路徑。

### 10. `CHANGELOG.md`
- Added: 支援 GitHub Copilot App（Tauri 桌面應用）的 token 使用量同步，自動讀取 `~/.copilot/data.db` 與 `session-store.db`。

### 11. `pricing.csv`
- 確認是否已有 `GLM5.2-high`、`gpt-5.6-luna`、`DP4F-medium` 等 App 常用 model 的計價；若缺則補（App 透過 `provider_id/model` 欄位帶 model，需在 collector 端做 `model_id → display_name` 的 normalize）。

## 風險與緩解

| 風險 | 緩解 |
|---|---|
| `session-store.db` 被 Copilot App 持有 WAL lock，rusqlite 唯讀開啟仍可能 `SQLITE_BUSY` | 用 `OpenFlags::SQLITE_OPEN_READ_ONLY \| SQLITE_OPEN_NO_MUTEX` 並設 `busy_timeout(2000)`；若仍失敗就 skip 這次同步並 eprintln，不中斷其他 collector |
| `data.db` schema 在未來 Copilot App 版本變動 | 開表前先 `PRAGMA user_version` 或查 `sqlite_master` 確認 `assistant_usage_events` 存在；不存在就 skip |
| 同一個 session 在 CLI 與 App 都有資料，造成重複 | `source_kind` 區分，前端可分開顯示；統計總量時若擔心雙算，可在 `DaySummary` 邏輯加 `source_kind` 篩選（但預設仍全部算入，與現有 VS Code + CLI 合併邏輯一致） |
| `assistant_usage_events.created_at` 是 UTC 字串，時區與 JSONL 的 `date -Iseconds` 不同 | 在 collector 統一轉成 `chrono::DateTime<Utc>` 再 to ISO 8601，`date` 欄位用 UTC date |

## 驗證步驟

1. `cargo build --release` 通過。
2. `cargo test sync_copilot_app` 通過。
3. 啟動看板，在 Copilot 頁面看到本機現有的 6 個 App session（含我目前的 session），token 數與 `data.db.sessions` 的 `total_input_tokens` 等欄位對得上。
4. 在 App 端新開一個 turn 後，5 秒內看板自動出現新 row。
5. `source_kind` 標示為 `App`，與 CLI 的 `CLI` 區分清楚。

## 不在這次範圍

- 不處理 Copilot App 的 `session_context_usage` 時間序列視覺化（未來可在 session drawer 加一條 context usage 折線圖）。
- 不處理 Copilot App 的 subagent / background agent（`agent_id` 非空）的獨立攤分，先全部歸入主 session。
- 不處理 `total_nano_aiu` 估算計價，因為 `pricing.csv` 目前是按 token 計價。