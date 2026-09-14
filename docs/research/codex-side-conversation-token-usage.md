# Codex `/side` conversation Token 用量研究

研究日期：2026-07-22
驗證版本：Codex CLI `0.145.0`、TokenUsageInsights `main`

* * *

## 結論

**目前抓不到 `/side` 用量的根因不是 JSONL parser 漏掉欄位，而是 `/side` 本身是 ephemeral fork，Codex 刻意不把它寫入 rollout JSONL 或持久化 thread store。**

因此：

- 既有 `~/.codex/sessions/**/*.jsonl` 掃描無法事後取得 `/side`。
- 已經結束、當時又沒有即時擷取的 side conversation，沒有可靠的官方資料來源可供完整回補。
- 要準確取得未來的用量，必須在 side thread 還存活時擷取 app-server 的 `thread/tokenUsage/updated` 通知。
- 官方 OTel 匯出可作為較低侵入性的備援，但目前匯出的 usage event 沒有 `ephemeral` 與 `forkedFromId`，無法可靠判定事件是否來自 `/side`，也無法準確掛回父 session。

建議採用的正式方案是：**讓 Codex CLI、TokenUsageInsights 共用同一個本機 Unix socket app-server，由 TokenUsageInsights 即時保存 ephemeral fork 的 Token 快照。**

* * *

## 根因證據

### `/side` 是 ephemeral fork

官方 CLI 文件將 `/side` 定義為從目前 chat 建立的 ephemeral fork。Codex `0.145.0` 原始碼也會在啟動 side conversation 時設定 `config.ephemeral = true`，再呼叫 `thread/fork`。

相關來源：

- [官方 `/side` 指令文件](https://learn.chatgpt.com/docs/developer-commands?surface=cli#start-a-side-chat-with-side)
- [TUI side conversation 將設定改為 ephemeral](https://github.com/openai/codex/blob/rust-v0.145.0/codex-rs/tui/src/app/side.rs#L469-L481)
- [TUI 透過 `thread/fork` 建立 side thread](https://github.com/openai/codex/blob/rust-v0.145.0/codex-rs/tui/src/app_server_session.rs#L606-L658)
- [協定中的 `Thread.ephemeral` 定義](https://github.com/openai/codex/blob/rust-v0.145.0/codex-rs/app-server-protocol/src/protocol/v2/thread_data.rs#L170-L190)

Codex 原始碼把 ephemeral 的語意定義為不 materialize 到磁碟。官方 app-server 測試亦驗證 ephemeral fork 沒有 path，也不會出現在持久化 thread list 中。

- [ephemeral thread 不應寫入磁碟](https://github.com/openai/codex/blob/rust-v0.145.0/codex-rs/app-server-protocol/src/protocol/v2/thread_data.rs#L183-L186)
- [ephemeral fork 的整合測試](https://github.com/openai/codex/blob/rust-v0.145.0/codex-rs/app-server/tests/suite/v2/thread_fork.rs#L1261-L1340)
- [`--ephemeral` 不產生 rollout 檔案的測試](https://github.com/openai/codex/blob/rust-v0.145.0/codex-rs/exec/tests/suite/ephemeral.rs#L51-L86)

### TokenUsageInsights 現況

目前同步器只遞迴掃描 `CODEX_DIR/sessions` 下的 JSONL：

- [`find_codex_session_files`](../../src/db/codex.rs)
- [`sync_codex_usage_logs`](../../src/db.rs)
- [`parse_codex_session_file`](../../src/db/codex.rs)

parser 已能處理持久化 subagent，並讀取 `event_msg` 的 `token_count.info.total_token_usage`。這類 subagent 有自己的 rollout JSONL，所以與 `/side` 的問題不同。

**結論是沒有可再補強的 `/side` JSONL 欄位；真正缺少的是 ephemeral thread 的即時資料入口。**

* * *

## CLI 與 Desktop 的整合差異

上述精確方案適用於 Codex CLI 的 `/side`，因為 CLI `0.145.0` 可改連共用的 Unix socket app-server。

本機目前執行中的 Codex Desktop app-server 使用預設 stdio transport，由 Desktop 程序持有單一雙向連線，沒有另外公開可供 TokenUsageInsights 加入的 socket。獨立 app-server 的 `--listen` 預設值也確實是 `stdio://`。

- [app-server 的 transport 預設值](https://github.com/openai/codex/blob/rust-v0.145.0/codex-rs/app-server/src/main.rs#L17-L31)

因此，若要擷取的是 Desktop 內部建立的 side conversation，外部程式目前不能在事後附加到既有 stdio app-server。可行方向只剩：

- Codex Desktop 未來支援共用 app-server 或公開 side usage event。
- 在 Codex 端增加明確的 persistence、hook payload 或 OTel metadata。
- 使用 OTel 擷取所有 response usage，但接受無法可靠分類 `/side` 的限制。

**不能把 CLI 的 Unix socket 方案直接宣稱為 Desktop 的現成解法。**

* * *

## 可用方案比較

| 方案 | Token 準確度 | 辨識 ephemeral fork | 父 session 關聯 | 可補歷史資料 | 維護風險 | 判定 |
|---|---:|---:|---:|---:|---:|---|
| 共用 app-server 並接收通知 | 高 | 高 | 高 | 否 | 中 | **建議正式方案** |
| OTel `codex.sse_event` | 高 | 低 | 低 | 否 | 中 | 備援或總量觀測 |
| 改用 `/fork` 或 `/new` | 高 | 不適用 | 視 rollout 而定 | 否 | 低 | 操作 workaround |
| 讀 `logs_2.sqlite` | 低至中 | 低 | 低 | 部分且不可靠 | 高 | 不建議 |
| Codex hooks | 不可用 | 不可用 | 部分 | 否 | 低 | 排除 |

### 為何不建議直接讀 `logs_2.sqlite`

本機驗證可看到部分 `post sampling token usage` tracing 訊息與 `thread/tokenUsage/updated` 事件名稱，但這不是公開穩定的資料契約：

- app-server 的通知 log 通常只記事件名稱，不含完整 notification payload。
- tracing 文字可能只有累計總量，未必有 input、cached input、cache write、output、reasoning 的完整拆分。
- 無法可靠確認某個沒有 rollout 的 thread 就是 `/side`，因為其他內部 ephemeral 工作也可能沒有持久化資料。
- schema 與 log 文字可隨 Codex 版本改變。

因此它只能當診斷線索，不應成為正式同步來源。

### 為何 hooks 不可用

Codex `0.145.0` 的 legacy notify、`Stop` 與 `SessionEnd` hook payload 有 thread、turn、transcript path 等資訊，但沒有 Token usage：

- [legacy notify payload](https://github.com/openai/codex/blob/rust-v0.145.0/codex-rs/hooks/src/legacy_notify.rs#L13-L27)
- [`StopRequest`](https://github.com/openai/codex/blob/rust-v0.145.0/codex-rs/hooks/src/events/stop.rs#L24-L43)
- [`SessionEndRequest`](https://github.com/openai/codex/blob/rust-v0.145.0/codex-rs/hooks/src/events/session_end.rs#L27-L33)

即使 hook 在 side conversation 結束時有被觸發，也沒有足夠欄位可重建用量。

* * *

## 建議架構：共用本機 app-server

### 連線拓撲

```text
Codex CLI TUI ─┐
               ├─ Unix socket app-server
Insights ──────┘       │
                       ├─ thread/started
                       ├─ turn/started
                       └─ thread/tokenUsage/updated
                                │
                                └─ TokenUsageInsights SQLite
```

Codex CLI `0.145.0` 已提供 app-server daemon 與 remote TUI：

```sh
codex app-server daemon start
codex --remote unix://
```

未帶會阻止 daemon 重用的啟動 override 時，CLI 也會探測預設 control socket。正式整合仍應顯示連線狀態，不能假設 daemon 一定存在。

預設 socket：

```text
~/.codex/app-server-control/app-server-control.sock
```

官方 app-server 文件確認 `--remote` 支援 Unix socket，且 socket 上承載 WebSocket JSON-RPC：

- [Codex app-server 連線與 transport 文件](https://learn.chatgpt.com/docs/app-server#connect-the-cli-terminal-ui)
- [app-server daemon control socket 路徑](https://github.com/openai/codex/blob/rust-v0.145.0/codex-rs/app-server-transport/src/transport/mod.rs#L53-L64)

### 為何第二個 client 可以收到用量

在共用 app-server 中，新 thread 建立時，server 會把所有已完成 `initialize` 的 connection 加入該 thread 的 listener。TokenUsageInsights 若先連上 daemon，之後建立的 side thread 就會自動把用量通知送到 Insights connection。

- [新 thread 自動附加所有已初始化 connection](https://github.com/openai/codex/blob/rust-v0.145.0/codex-rs/app-server/src/lib.rs#L1083-L1097)
- [建立 thread listener](https://github.com/openai/codex/blob/rust-v0.145.0/codex-rs/app-server/src/request_processors/thread_processor.rs#L2980-L3013)

這也界定了啟動順序：**Insights 必須在要觀測的 side thread 建立前完成連線與初始化。** 連線前已結束的 side thread 仍無法回補。

### 要接收的事件

#### `thread/started`

保存以下 metadata：

- `thread.id`
- `thread.ephemeral`
- `thread.forkedFromId`
- `thread.sessionId`
- `thread.createdAt`
- `thread.modelProvider`
- `thread.cwd`

`/side` 可觀察為 `ephemeral = true` 且通常有 `forkedFromId`。目前協定沒有 `isSideConversation` 之類的專屬旗標，因此正式資料名稱應使用「ephemeral fork」，不要宣稱能區分所有可能的 ephemeral fork 類型。

#### `turn/started`

記錄 side thread 真正開始的新 turn，並用來驗證後續 usage 的 `turnId` 確實屬於 side 的新工作。

#### `thread/tokenUsage/updated`

官方 app-server 會另外串流 active thread 的 Token usage。Payload 包含：

- `threadId`
- `turnId`
- `tokenUsage.total`
- `tokenUsage.last`
- `tokenUsage.modelContextWindow`

Token breakdown 包含：

- `totalTokens`
- `inputTokens`
- `cachedInputTokens`
- `cacheWriteInputTokens`
- `outputTokens`
- `reasoningOutputTokens`

來源：

- [官方 app-server event 文件](https://learn.chatgpt.com/docs/app-server#events)
- [`ThreadTokenUsageUpdatedNotification` 與 breakdown schema](https://github.com/openai/codex/blob/rust-v0.145.0/codex-rs/app-server-protocol/src/protocol/v2/thread.rs#L1459-L1525)
- [core `token_count` 轉成 app-server notification](https://github.com/openai/codex/blob/rust-v0.145.0/codex-rs/app-server/src/bespoke_event_handling.rs#L1547-L1563)

* * *

## 計數與去重規則

side fork 會複製父 thread 的歷史與累計 Token。若直接把第一個 live event 的 `total` 當成 side 用量，會把父 session 的歷史重複計算。

`/side` 會使用 `excludeTurns = true`。目前 app-server 會略過這條 fork 路徑的歷史 usage replay；一般 resume 或 fork 的 replay 也只送給發出請求的 connection，不會送給 Insights 這類其他 subscriber。因此，Insights 不能假設自己一定看得到一筆可直接使用的 fork baseline。

- [side fork 設定 `excludeTurns`](https://github.com/openai/codex/blob/rust-v0.145.0/codex-rs/tui/src/app_server_session.rs#L630-L653)
- [`excludeTurns` fork 略過 usage replay](https://github.com/openai/codex/blob/rust-v0.145.0/codex-rs/app-server/src/request_processors/thread_processor.rs#L4261-L4284)
- [歷史 usage replay 限定單一 connection](https://github.com/openai/codex/blob/rust-v0.145.0/codex-rs/app-server/src/request_processors/token_usage_replay.rs#L29-L57)

建議規則：

1. 收到 `thread/started` 後建立 ephemeral thread metadata，但先不新增用量。
2. 第一個屬於 side 新 turn 的 usage event，以 `tokenUsage.last` 當作增量，並保存 `tokenUsage.total` 作為後續 baseline；不可把第一個完整 `total` 全算到 side。
3. 後續事件以目前 total 相對上一個 total 計算逐欄位差值；若任何欄位倒退或 total 無法形成合理差值，回退使用該 event 的 `last`。
4. 以 thread id、turn id 與 cumulative snapshot fingerprint 做冪等去重，避免重連、重送或同一 snapshot 的 rate-limit 更新造成重複資料。
5. 只把 `ephemeral = true` 的 live 資料寫成新來源，例如 `source_kind = 'codex-app-server-ephemeral'`；持久化 thread 繼續交由 JSONL importer，避免雙重計算。
6. `parent_session_id` 使用 `forkedFromId`。不要使用 side thread 自己的 `sessionId` 取代父 session。

父 thread 在 TokenUsageInsights 中的最新 cumulative total 可作為一致性檢查，但不應當成唯一 baseline，因為既有 JSONL 同步可能落後於 live fork 時點。

* * *

## OTel 備援方案

Codex 官方支援在 `config.toml` 啟用 OTel log exporter，並建議保留 `log_user_prompt = false`：

```toml
[otel]
environment = "local"
log_user_prompt = false
exporter = { otlp-http = { endpoint = "http://127.0.0.1:3003/v1/logs", protocol = "json" } }
```

官方來源：

- [Codex OTel 設定文件](https://learn.chatgpt.com/docs/config-file/config-advanced#observability-and-telemetry)
- [`response.completed` 匯出的 Token 欄位](https://github.com/openai/codex/blob/rust-v0.145.0/codex-rs/otel/src/events/session_telemetry.rs#L926-L950)
- [OTel 共用 metadata](https://github.com/openai/codex/blob/rust-v0.145.0/codex-rs/otel/src/events/shared.rs#L1-L53)

`codex.sse_event` 的 `response.completed` 有 per-response Token breakdown 與 `conversation.id`，但共用 metadata 沒有 `ephemeral`、`forkedFromId` 或 `/side` 類型。因此：

- 可用來避免漏掉整體 Codex Token。
- 不適合單獨實作精確的 side conversation 分類。
- 若與 JSONL importer 同時啟用，必須定義單一主資料來源或跨來源去重，否則持久化 session 會被重複計算。

* * *

## 建議實作順序

1. 新增 app-server Unix socket client，啟動後送出 `initialize` 與 `initialized`。
2. 新增 live thread metadata 與 cumulative snapshot 的儲存層，先不要直接混入既有 JSONL 同步狀態。
3. 實作 `thread/started`、`turn/started`、`thread/tokenUsage/updated` 的狀態機，並以第一筆 `last` 建立安全 baseline。
4. 將確認後的 side 增量 materialize 到 `usage_entries`，使用獨立 `source_kind`。
5. UI 顯示 app-server daemon 連線狀態，以及「只涵蓋連線後 ephemeral conversations」的資料範圍。
6. 補齊下列測試：
   - fork baseline 不計入 side。
   - 同一 cumulative snapshot 重送不重複計數。
   - Token counter reset 能正確恢復。
   - `cacheWriteInputTokens` 與 reasoning Token 正確保存。
   - persistent thread 不會同時由 live stream 與 JSONL 重複計算。
   - Unix socket 斷線重連後不把歷史 snapshot 重算。

**不建議先做 `logs_2.sqlite` parser；它無法解決可靠分類、完整 breakdown 與資料契約穩定性三個核心問題。**

* * *

## 研究限制

- 結論以 Codex CLI `0.145.0` 與對應 tag 原始碼為準。app-server WebSocket TCP transport 仍標示為 experimental；本機 Unix socket 是較合適的整合面。
- 協定沒有 side conversation 專屬類型，只能準確識別 ephemeral fork 與其來源 thread。
- 研究未啟動 daemon、未改寫 Codex 設定，也未產生實際 API 用量；避免改變目前使用者環境與產生額外 Token。
- 本次只有研究文件，沒有修改產品程式碼或資料庫 schema。
