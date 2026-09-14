# Cursor Agent 與 Grok 4.5 模型用量、費用研究

研究日期：2026-07-27  
本機驗證版本：Cursor `3.10.17`、TokenUsageInsights `main`

* * *

## 結論

**Cursor 的 Agent mode 是工作模式，不是模型名稱。** Agent 負責搜尋、編輯、執行指令與呼叫工具；每個對話分頁另有自己的模型選擇。只有「Agent mode」這項資訊，無法判定實際使用 Grok、Composer、Claude、GPT 或其他模型。

**若明確選用 Grok 4.5，Cursor 的伺服器端 Usage 事件可以分辨模型與費用；目前 TokenUsageInsights 讀取的 Cursor 專案 JSONL 則無法可靠分辨。**

- 個人方案可在 Cursor Usage 頁面檢視，並以 CSV 匯出每筆事件的模型、Input Tokens、Output Tokens、Cache Read 等欄位。
- Teams／Enterprise 管理員可使用官方 Admin API 的 `/teams/filtered-usage-events`，取得逐事件的 `model`、計費類型與部分事件的 Token、費用欄位。
- Cursor `3.10.17` 的本機 `state.vscdb` 確實存有部分逐訊息 `modelInfo.modelName` 與 `tokenCount` 欄位，但本機樣本的 Input／Output 值全部為 0。這是未公開的內部 schema，且缺少 Cache Read 與伺服器核定費用，不適合作為帳務主資料。
- `~/.cursor/projects/**/agent-transcripts/**/*.jsonl` 在本機樣本中只有訊息、工具呼叫與結束狀態，沒有模型或真實 Token 欄位。
- 選用 Auto 時，Cursor Router 可逐次路由到不同模型，且官方變更紀錄明載實際路由模型預設為隱藏。因此即使本機看到 `Auto`，也不代表能取得每次底層模型。

**截至 2026-07-27，Cursor Grok 4.5 一般版每 100 萬 Token 的公開價格為 Input 2 美元、Cache Read 0.50 美元、Output 6 美元；Fast 版為 4 美元、1 美元與 18 美元。** Cursor 官網所列的 Grok 4.5 上線 50% 折扣已於 2026-07-21 結束，因此本研究以未折扣價格比較。Cursor 第一方模型 included usage 的 2 倍增額是另一項持續措施，並未於 7 月 21 日截止。

* * *

## 先釐清：Agent mode、模型與 Auto 是三件事

Cursor 官方 Agent 文件把 Agent 定義為可自主搜尋程式碼、修改檔案、執行終端機指令的助理模式。官方文件也說明每個對話分頁會維持自己的模型選擇。

- [Cursor Agent Overview](https://cursor.com/docs/agent/overview)
- [Cursor Modes](https://cursor.com/docs/agent/modes)

因此應分成三層理解：

| 層級 | 功能 | 是否能直接判定 Grok |
|---|---|---:|
| Agent mode | 決定可使用的工具與自主執行方式 | 否 |
| 明確模型選擇 | 例如 Grok 4.5 High、Grok 4.5 High Fast | 是 |
| Auto／Cursor Router | 依每次請求選擇底層模型 | 不一定 |

Cursor 在 2026-07-22 發布 Cursor Router，Auto 會按請求的類型與複雜度挑選模型。官方變更紀錄明載：

- Balance 與 Intelligence 依實際路由模型的費率計費。
- 實際路由模型可顯示或隱藏，且預設隱藏。
- Grok 4.5 是 Router 所需的價格效率路由選項之一。

來源：

- [Cursor Changelog：Cursor Router](https://cursor.com/changelog)
- [Cursor 員工說明 Auto 目前無法查看確切底層模型](https://forum.cursor.com/t/show-which-model-handled-each-step-when-using-auto-mode/164163/7)

**所以「對話上顯示 Agent」不是模型識別；「對話上顯示 Auto」也未必是實際計費模型。**

* * *

## Grok 4.5 的模型識別

### xAI API 的正式識別

xAI 官方文件列出的基礎模型 ID 為：

```text
grok-4.5
```

正式 alias 包含：

```text
grok-4.5-latest
grok-build-latest
```

來源：

- [xAI Grok 4.5 模型文件](https://docs.x.ai/developers/models/grok-4.5)
- [xAI Models API](https://docs.x.ai/developers/rest-api-reference/inference/models)

### Cursor 內的 reasoning／Fast slug

Cursor 員工於 2026-07-13 說明，產品內的 Grok slug 曾為配合 UI 的 Low、Medium、High 命名而更名：

| 舊 slug | 新 slug |
|---|---|
| `grok-4.5-medium` | `cursor-grok-4.5-low` |
| `grok-4.5-fast-medium` | `cursor-grok-4.5-low-fast` |
| `grok-4.5-high` | `cursor-grok-4.5-medium` |
| `grok-4.5-fast-high` | `cursor-grok-4.5-medium-fast` |
| `grok-4.5-xhigh` | `cursor-grok-4.5-high` |
| `grok-4.5-fast-xhigh` | `cursor-grok-4.5-high-fast` |

Cursor 員工表示這只是命名調整，不應改變效能或計費。這項資訊來自 Cursor 官方社群論壇的員工回覆，而非公開 API schema，匯入時應保留原始 slug，並另外做正規化，不應覆寫原始資料。

來源：

- [Cursor 員工說明 Grok 4.5 slug 更名](https://forum.cursor.com/t/cursor-grok-4-5-high-fast-doesnt-offer-a-50-discount-at-all/165551/8)

**建議的模型群組鍵是 `grok-4.5`，但原始 `model_id` 必須保留完整 reasoning 與 Fast 後綴。** 否則會把費率不同的一般版與 Fast 版混在一起。

* * *

## Grok 4.5 到底多貴

### Cursor 公開價格

Cursor 官方 Models & Pricing 頁面目前列出：

| 版本 | Input | Cache Read | Output | Context window |
|---|---:|---:|---:|---:|
| Cursor Grok 4.5 | USD 2／MTok | USD 0.50／MTok | USD 6／MTok | 256k |
| Cursor Grok 4.5 Fast | USD 4／MTok | USD 1／MTok | USD 18／MTok | 256k |

來源：

- [Cursor Models & Pricing](https://cursor.com/docs/models-and-pricing)
- [Cursor：Introducing Grok 4.5](https://cursor.com/blog/grok-4-5)
- [Cursor Grok 4.5 產品頁](https://cursor.com/grok)

Cursor 產品頁曾標示 included usage 加倍至 2026-07-21；截至本研究日期已過期，故不能再套用 50% 促銷折扣。

Cursor 費用公式：

```text
一般版費用 = Input MTok × 2 + Cache Read MTok × 0.5 + Output MTok × 6
Fast 費用   = Input MTok × 4 + Cache Read MTok × 1 + Output MTok × 18
```

Fast 相對一般版為：

- Input 單價 2 倍
- Cache Read 單價 2 倍
- Output 單價 3 倍

### 與 Composer 2.5 比較

同一份 Cursor Models & Pricing 資料列出的費率如下：

| 模型 | Input | Cache Read | Output |
|---|---:|---:|---:|
| Composer 2.5 | USD 0.50／MTok | USD 0.20／MTok | USD 2.50／MTok |
| Composer 2.5 Fast | USD 3／MTok | USD 0.50／MTok | USD 15／MTok |
| Cursor Grok 4.5 | USD 2／MTok | USD 0.50／MTok | USD 6／MTok |
| Cursor Grok 4.5 Fast | USD 4／MTok | USD 1／MTok | USD 18／MTok |

**Cursor Grok 4.5 一般版比 Composer 2.5 一般版貴，但比 Composer 2.5 Fast 便宜；Cursor Grok 4.5 Fast 則三種 Token 單價都高於 Composer 2.5 Fast。** Cursor 員工也說明，第一方模型池不是以 Token 數量 1:1 扣除，而是按各模型與速度層級的 Token 費率消耗。

來源：

- [Cursor Models & Pricing](https://cursor.com/docs/models-and-pricing)
- [Cursor 員工說明第一方模型池的費率差異](https://forum.cursor.com/t/grok-4-5-pricing-for-subscription-plans/165207/12)

### xAI 直連 API 的價格不同

xAI 官方定價頁補充一般版 Grok 4.5 的 Cached Input 與長 context 價格：

| Context | Input | Cached Input | Output |
|---|---:|---:|---:|
| 少於 200k prompt tokens | USD 2／MTok | USD 0.30／MTok | USD 6／MTok |
| 達 200k prompt tokens | USD 4／MTok | USD 0.60／MTok | USD 12／MTok |

來源：

- [xAI Pricing](https://docs.x.ai/developers/pricing)
- [xAI Grok 4.5](https://docs.x.ai/developers/models/grok-4.5)

這是 xAI 直連 API 的公開價，其 Cached Input 為 0.30 美元、context window 為 500k；Cursor 代管版則是 0.50 美元與 256k。**計算 Cursor 費用時，不能把 xAI 直連 API 的 Cached Input 或長 context 費率套到 Cursor 事件。**

Cursor 內仍應以 Usage／CSV 的核定費用為準，也不應只用 Total Tokens 乘單一費率。

### 費用範例

若一次工作合計 100,000 個非快取 Input Tokens 與 10,000 個 Output Tokens，且 Cache Read 為零：

| 版本 | 計算 | 費用 |
|---|---|---:|
| Grok 4.5 | `0.1 × 2 + 0.01 × 6` | USD 0.26 |
| Grok 4.5 Fast | `0.1 × 4 + 0.01 × 18` | USD 0.58 |

若各有 1 MTok 非快取 Input 與 1 MTok Output，則一般版為 8 美元，Fast 為 22 美元，Fast 是 2.75 倍。

**實際倍數會隨 Input、Cache Read、Output 比例改變，不能只比較 Total Tokens。**

### 為何一個 Agent 工作可能比預期貴

Agent 的一個使用者指令不等於一次模型呼叫。長任務可能反覆讀檔、呼叫工具、接收工具結果、重新送入對話歷史、壓縮 context，或啟動 subagent。xAI 官方文件也說明，agentic request 的 Token 費用包含：

- Input Tokens
- Reasoning Tokens
- Completion Tokens
- Cached Prompt Tokens
- 視情況產生的 server-side tool invocation 費用

來源：

- [xAI Tool Pricing](https://docs.x.ai/developers/pricing#tools-pricing)
- [xAI Tool Usage Details](https://docs.x.ai/developers/tools/tool-usage-details)
- [xAI Prompt Caching Usage and Pricing](https://docs.x.ai/developers/advanced-api-usage/prompt-caching/usage-and-pricing)

Cursor 使用自己的 Agent harness，並非直接把 xAI API 的所有 server-side tools 原樣暴露給使用者。因此 xAI 的工具單價不能直接加到每筆 Cursor 帳單；**Cursor Usage／CSV 或 Cursor Admin API 的核定事件才是 Cursor 帳務準據。**

* * *

## Cursor 方案與用量池

Cursor 官方頁面將 Grok 4.5 視為 first-party model，與 Composer 2.5 同類。對目前的 Teams 方案：

- 每位使用者每月至少含 20 美元 Agent 用量。
- 超出額度後按公開 API 標價計算 on-demand usage。
- Grok 4.5 與 Composer 2.5 免收 Cursor Token Rate。
- 第三方模型可能另收 Cursor Token Rate。

來源：

- [Cursor for Teams FAQ](https://cursor.com/business/teams)
- [Cursor Pricing Policy](https://cursor.com/terms/pricing/2026-04-10)

Cursor 員工另說明新方案可能分成 first-party models pool 與 third-party API pool；舊方案、個人方案與 Enterprise 合約的扣抵順序可能不同。

來源：

- [Cursor 員工說明 Grok 4.5 用量池](https://forum.cursor.com/t/which-usage-pool-does-grok-4-5-draw-from/165461/7)
- [Cursor 員工說明新舊 Teams 方案差異](https://forum.cursor.com/t/teams-first-party-model-pool/165385/10)

Cursor 員工在 2026-07-19 進一步區分兩項活動：

- Grok 4.5 上線 50% 折扣是暫時活動，已於 2026-07-21 結束。
- Cursor first-party models pool 的 2 倍 included usage 是持續增額，沒有 7 月 21 日截止日。
- 2 倍增額適用採新 token 計價的 self-serve Pro、Pro+、Ultra 與 Teams，不包含 Enterprise 與舊 request-based 方案。

來源：

- [Cursor 員工說明永久 2 倍額度與暫時 50% 折扣的差異](https://forum.cursor.com/t/now-available-2x-included-usage-your-plan-now-includes-2x-usage-for-all-cursor-models-composer-2-5-and-cursor-grok-4-5/166007/4)

**「Included」或「Free」只代表當下由方案額度、credit 或促銷吸收，不代表該請求的模型成本為零。** 要比較模型消耗速度，仍應讀取逐事件 Token 類型與模型費率。

* * *

## 可取得用量差異的資料來源

### 1. Cursor Usage 頁面

Cursor 員工於 2026-05-19 說明，個人方案目前可用：

- Web：Cursor Settings／Dashboard 的 Usage 頁面
- IDE：Cursor Settings > Usage
- Web Usage 頁面的 CSV 匯出

來源：

- [Cursor 員工說明個人方案 Usage 管道](https://forum.cursor.com/t/usage-api-cli-command/160967/5)

Cursor 官方定價文件也說明 Dashboard 可查看 usage 與 Token breakdown。

- [Cursor Models & Pricing](https://cursor.com/docs/models-and-pricing)

判定：

**個人方案最可靠且受支援的方式是 Usage 頁面與 CSV。** 但 Dashboard 可能把方案內事件只顯示為 `Included` 或 `Free`，不一定顯示數字費用。

### 2. Usage CSV 匯出

Cursor 員工回覆確認 CSV 包含：

- Model
- Input Tokens
- Output Tokens
- Cache Read
- Total Tokens
- Cost 欄位

來源：

- [Cursor 員工說明 Usage CSV 欄位](https://forum.cursor.com/t/usage-not-showing/154766/17)

已知限制：

- 某些方案內事件的 Cost 可能只顯示 `Included`，而非數字。
- Cursor 沒有公開穩定的 CSV schema 或版本欄位。
- CSV 是人工匯出，不是個人方案的公開自動化 API。

### 3. Cursor Admin API

官方 Admin API 提供：

```http
POST https://api.cursor.com/teams/filtered-usage-events
```

文件列出的逐事件欄位：

```text
timestamp
model
kind
maxMode
requestsCosts
isTokenBasedCall
tokenUsage.inputTokens
tokenUsage.outputTokens
tokenUsage.cacheWriteTokens
tokenUsage.cacheReadTokens
tokenUsage.totalCents
userEmail
```

來源：

- [Cursor Admin API](https://cursor.com/docs/account/teams/admin-api)

重要限制：

- 只有 Team 管理員能建立 Admin API key。
- 官方 schema 表示 `tokenUsage` 只保證在 `isTokenBasedCall = true` 時存在。
- 官方範例中的 `Included in Business` 事件為 `isTokenBasedCall = false`，且未附 `tokenUsage`。
- `/teams/daily-usage-data` 只有每日 `mostUsedModel`，不足以重建逐模型費用。
- 這些 API 不提供給個人 Pro 帳號。

Cursor 員工於 2026-07-08 表示 `conversation_id` 已加入 `/teams/filtered-usage-events`，但截至研究日期，公開 Admin API schema 尚未列出這個欄位。因此本研究把它列為「伺服器可能已提供、公開契約尚未同步」，不能在未實測 API 的情況下視為必備欄位。

來源：

- [Cursor 員工說明 `conversation_id`](https://forum.cursor.com/t/we-need-a-deterministic-way-to-attribute-cursor-token-usage-to-local-ide-sessions-features-and-subagents/164412/6)

### 4. 個人方案 API

Cursor 員工在 2026-05-19 明確表示：

**個人方案目前沒有公開 Usage API 或 CLI 指令。**

來源：

- [Cursor 員工說明個人方案沒有公開 Usage API](https://forum.cursor.com/t/usage-api-cli-command/160967/5)

瀏覽器 Dashboard 內部使用的私人 endpoint 並非公開 API，可能依 session cookie、前端版本或未公開欄位變動，不應作為正式整合契約。

### 5. Cursor CLI 輸出

官方 `stream-json` 格式的初始化事件包含 session model，最終事件可含 `request_id`：

```json
{"type":"system","subtype":"init","session_id":"...","model":"Grok 4.5"}
{"type":"result","session_id":"...","request_id":"..."}
```

但官方格式沒有逐次 Input、Output、Cache Read 或費用。

來源：

- [Cursor CLI Output Format](https://cursor.com/docs/cli/reference/output-format)

判定：

**CLI 輸出適合記錄使用者選擇的 session model 與 request ID，不足以自行計費，也不能保證揭露 Auto 的每次底層路由模型。**

### 6. Cursor hooks

Cursor 員工說明 `beforeSubmitPrompt` hook 可取得 `model` 與 `composer_mode`，其中 `composer_mode` 在送出時是穩定欄位，常見值包含 Ask 的 `chat` 與 Agent 的 `agent`。但包含 `model` 在內的完整 payload schema 尚未寫入公開 hooks 文件。

來源：

- [Cursor 員工說明 `beforeSubmitPrompt` 的 model 與 composer_mode](https://forum.cursor.com/t/beforesubmitprompt-hook-lacks-reliable-current-mode-signal-ask-vs-agent-making-mode-based-command-safety-gates-unreliable/159905/7)

判定：

- 可用來記錄送出時選擇的具名模型與 Agent／Ask 模式。
- 沒有完整 Token breakdown 或費用。
- 無法解決 Auto 的底層 resolved model。
- 因 `model` 尚未列入公開 schema，整合時必須容許欄位缺失與版本變動。

* * *

## 本機資料驗證

本節是 Cursor `3.10.17` 的本機唯讀觀察，不是 Cursor 公開資料契約。沒有讀取或保存對話本文、access token 或 refresh token。

### `~/.cursor/projects/**/agent-transcripts/**/*.jsonl`

本機樣本包含以下事件結構：

```text
role = user
role = assistant
type = turn_ended
message.content[].type = text | tool_use
```

樣本中沒有：

```text
model
model_id
input_tokens
output_tokens
cache_read_tokens
cost
request_id
```

**因此目前 TokenUsageInsights 從 JSONL 文字長度推估 Token、並將模型標為 `Cursor Agent`，只能視為近似活動量，不能用來回答「Grok 與其他模型各用了多少、花了多少」。**

相關現況：

- [`parse_cursor_session_file`](../../src/db/cursor.rs)
- [`parse_cursor_timeline`](../../src/timeline.rs)

### `~/Library/Application Support/Cursor/User/globalStorage/state.vscdb`

本機 Cursor `3.10.17` 的 `cursorDiskKV` 表中，`bubbleId:<conversation-id>:<bubble-id>` 記錄可觀察到：

```text
createdAt
requestId
modelInfo.modelName
tokenCount.inputTokens
tokenCount.outputTokens
subagentSpawnTaskToolCallId
toolFormerData.additionalData.subagentComposerId
```

本機可見的模型值範例為：

```text
composer-2.5
```

同一批資料未觀察到：

```text
cacheReadTokens
cacheWriteTokens
totalCents
usagePool
onDemand
```

判定：

- 可補強本機對話的模型 slug、request ID 與 subagent 關聯線索。
- 本機樣本雖有 `tokenCount.inputTokens` 與 `tokenCount.outputTokens` 欄位，但 89 筆記錄的值全部為 0，未驗證可用於 Token 統計。
- 無法獨立重建 Cursor 帳單，因為缺少 Cache Read、核定價格、方案扣抵與可能的伺服器端修正。
- `cursorDiskKV`、`bubbleId:` 與內部 JSON schema 都沒有公開版本保證。
- Auto 的底層模型可能被 Router 隱藏；本機 `modelInfo.modelName` 不應自動解讀為實際計費路由模型。
- Cursor 執行中直接讀取資料庫可能遇到鎖定或未完成寫入；正式實作應使用唯讀連線或先建立一致性快照。

**本機 SQLite 適合當對話歸因的輔助來源，不適合取代 Usage CSV 或 Admin API。**

### `~/.cursor/ai-tracking/ai-code-tracking.db`

本機 Cursor `3.10.17` 另有 AI code tracking 資料庫，其 schema 可觀察到：

```text
ai_code_hashes.requestId
ai_code_hashes.conversationId
ai_code_hashes.model
conversation_summaries.conversationId
conversation_summaries.model
conversation_summaries.mode
tracked_file_content.conversationId
tracked_file_content.model
ai_deleted_files.conversationId
ai_deleted_files.model
```

這個資料庫的目的偏向追蹤 AI 產生或刪除的程式碼，不是完整模型請求帳務。本機樣本的上述內容表皆為 0 筆，只有 `tracking_state` 為 1 筆，因此無法用本機樣本驗證實際 join 行為。

判定：

- 有資料時可能用 `requestId`、`conversationId`、`model` 輔助程式碼變更與對話的映射。
- 沒有 Token 或費用欄位。
- 不會涵蓋沒有產生可追蹤程式碼的所有 Agent 請求。
- schema 未公開，不應作為完整用量或帳務主來源。

* * *

## TokenUsageInsights 現況的直接影響

目前 Cursor importer 在 [`parse_cursor_session_file`](../../src/db/cursor.rs) 中：

```rust
let input_tokens = (current_prompt.len() / 4).max(10) as u64;
let output_tokens = (reply_text.len() / 4).max(10) as u64;
```

並固定寫入：

```rust
model: Some("Cursor Agent".to_string()),
model_id: Some("Cursor Agent".to_string()),
```

這表示現有 Cursor 數據有兩個限制：

1. Token 是 prompt 與 reply 字元數除以 4 的近似值，不是 Cursor 伺服器計費 Token。
2. 所有模型都被合併成 `Cursor Agent`，無法分出 Grok 4.5、Grok 4.5 Fast、Composer 或 Auto。

目前 [`pricing.csv`](../../pricing.csv) 只有泛用的：

```text
Cursor Agent,Cursor,1M Tokens,3.00,0.30,15.00
```

以及舊 `grok-2`、`grok-2-mini`，沒有 Cursor Grok 4.5 的一般版／Fast／Cache Read 費率。因此即使本機估算 Token，也會套到不正確的模型與價格。

本機 Cursor structured logs 另可觀察到同一主 session 的 `composer-2.5`、`composer-2.5-fast` 模型線索，但 subagent 記錄可能沒有模型，也沒有完整 Token。這再次證明本機資料只能盡力歸因。

**在沒有匯入 Usage CSV 或 Admin API 前，TokenUsageInsights 不應把目前 Cursor 顯示的 Token 或成本宣稱為帳務精確值。**

* * *

## 對 TokenUsageInsights 的可行判定

### 個人方案

| 目標 | 可行性 | 建議來源 |
|---|---:|---|
| 分辨 Grok 4.5／Fast／reasoning slug | 高 | Usage CSV |
| 取得 Input／Output／Cache Read | 高 | Usage CSV |
| 取得每筆實付費用 | 中 | CSV Cost 為數字時；`Included` 時需另算或只記方案內 |
| 自動定期同步 | 低 | 目前無公開個人 Usage API |
| 關聯本機 conversation | 低至中 | 本機 SQLite 與 CSV 的時間、model；若匯出資料未提供 request ID，仍缺少穩定 join key |
| 區分 Auto 實際底層模型 | 低至中 | 取決於 Cursor 是否顯示／匯出 routed model |

### Teams／Enterprise

| 目標 | 可行性 | 建議來源 |
|---|---:|---|
| 分辨逐事件模型 | 高 | `/teams/filtered-usage-events.model` |
| 取得 Token breakdown | 中至高 | `isTokenBasedCall = true` 的 `tokenUsage` |
| 取得核定費用 | 高 | `tokenUsage.totalCents` |
| 自動定期同步 | 高 | Admin API 分頁與日期篩選 |
| 關聯 conversation | 中 | `conversation_id` 已由員工宣布，但公開 schema 尚未同步 |
| 區分 Included 事件的完整 Token | 中低 | 官方契約不保證非 token-based 事件附 `tokenUsage` |

* * *

## 建議資料模型與匯入策略

若後續要讓 TokenUsageInsights 正確顯示 Cursor 模型差異，建議保留：

```text
raw_model_id
normalized_model_family
reasoning_effort
is_fast
input_tokens
output_tokens
cache_read_tokens
cache_write_tokens
server_total_cents
billing_kind
is_token_based_call
request_id
conversation_id
source_kind
```

正規化範例：

```text
raw_model_id = cursor-grok-4.5-high-fast
normalized_model_family = grok-4.5
reasoning_effort = high
is_fast = true
```

來源優先順序：

1. **Cursor Admin API 的伺服器核定事件**
2. **Cursor Usage CSV**
3. 本機 `state.vscdb` 的模型、request ID 與 subagent 歸因線索
4. Cursor Agent transcript JSONL 的對話與工具時間軸
5. 文字長度 Token 估算，只能作為沒有其他資料時的低可信度 fallback

去重時不可只用時間與模型。Agent 一個工作會產生多筆模型呼叫；應優先使用伺服器事件 ID、request ID 或 conversation ID。若沒有穩定 ID，必須把結果標記為近似歸因，而不是精確帳務。

* * *

## 研究限制

- Cursor 文件網站在研究期間有舊 `docs.cursor.com` 路徑轉址與搜尋索引版本落差；本文優先引用可直接開啟的 Cursor 官方頁面、官方變更紀錄與官方論壇員工回覆。
- 未登入使用者的 Cursor Dashboard，未匯出使用者的實際 Usage CSV，也未呼叫需要 Team Admin API key 的 endpoint。
- 沒有 xAI API key，因此未呼叫 `/v1/models` 取得帳號當下的 live model catalog；價格以 xAI 與 Cursor 在 2026-07-27 可公開查閱的頁面為準。
- 本機驗證只代表 Cursor `3.10.17`。Cursor `3.11` 或後續版本可隨時變更 SQLite 與 transcript schema。
- Cursor 官網對 2026-07-21 已截止的促銷與部分區域可用性仍有過期文字。本文只採用與模型識別、定價直接相關且可交叉驗證的內容。
- 本次只有研究文件，沒有修改產品程式碼、資料庫 schema、Cursor 設定或使用者帳務資料。
