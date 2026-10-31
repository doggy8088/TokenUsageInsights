# Antigravity Subagent 階層關係與工作狀態顯示研究

研究日期：2026-10-29
驗證版本：Antigravity CLI (`~/.gemini/antigravity-cli` 本機實測 1,014 個會話、219 個子代理紀錄)、TokenUsageInsights `main` (`d2f6c04`)
對應議題：[GitHub Issue #73 — Antigravity 支援 subagent 工作狀態顯示](https://github.com/doggy8088/TokenUsageInsights/issues/73)

* * *

## 結論

**Antigravity 的每個 Subagent 都有獨立的 `conversationId`（UUID），並各自寫入獨立的 `brain/<child_id>/.system_generated/logs/transcript_full.jsonl` 與 `conversations/<child_id>.db`。目前 TokenUsageInsights 已經把 Antigravity Subagent 當成獨立會話匯入並計算 Token，但因為缺少 `parent_session_id`、`agent_nickname`、`agent_role` 與 `SYSTEM_MESSAGE` 解析，導致所有 Subagent 在介面上全部攤平為頂層會話（且多數名稱退化為 `Start Coding Session`），時間軸抽屜也看不到父子代理之間傳遞的任務提示詞與回報訊息。**

具體根因與關鍵發現如下：

1. **獨立會話儲存模型（與 Claude / Codex 一致，不同於 Copilot 共用單一 `events.jsonl`）**：
   - Antigravity 每次透過 `invoke_subagent`（單一或批次 `Subagents` 陣列）啟動子代理時，都會建立新的子會話 `<child_id>`，擁有自己的 `brain/<child_id>/` 與 `conversations/<child_id>.db`。
   - 因此，前端 `static/app.js` 與 `static/session-utils.js` 既有的樹狀階層渲染器（`sortAndGetFlatSessions`、`parentSessionIdentityKey`、`isSubagent`、縮排連接線、群組 Hover 高亮）**完全不需要新增獨立的合成 ID 機制**；只要後端為 Antigravity 子會話填入正確的 `parent_session_id`、`session_name`、`agent_nickname` 與 `agent_role`，前端就能直接渲染出與 Claude、Codex 一致的多層級 Subagent 樹狀結構。
2. **四種第一手階層與角色資料來源**：
   - **來源 A（子會話自身的 SQLite `conversations/<child_id>.db`）**：`trajectory_metadata_blob.data`（`exa.cortex_pb.CortexTrajectoryMetadata` Protobuf）原生記載 `field 5 = parent_conversation_id`、`field 6 = root_conversation_id`、`field 8 = subagent_spec`（內含 `field 1 = type_name`、`field 2 = role`、`field 3 = initial_prompt`）與 `field 17 = nesting_depth`。本機全部 206 個存活子會話 `.db` 100% 具備完整欄位。
   - **來源 B（父會話目錄 `brain/<parent_id>/.system_generated/subagents/<child_id>.json`）**：父會話在每次衍生子代理時，會寫入 JSON 檔案，記錄 `conversationId`、`subagentDescriptor.typeName`、`subagentDescriptor.role`、`state`（`SUBAGENT_STATE_ALIVE` / `SUBAGENT_STATE_KILLED`）、`spawnStepIndex` 與 `workspaceUris`。本機共 219 筆（含 13 筆後續已清除目錄的子代理），與來源 A 欄位比對 100% 一致（0 筆差異）。
   - **來源 C（子會話目錄 `brain/<child_id>/.system_generated/messages/<msg_id>.json`）**：新版 Antigravity CLI（199/206 個子會話）在子代理收到父代理訊息時會寫入訊息檔，內含 `sender`（父會話 ID）、`recipient`（子會話 ID）、`content`（提示詞）及 `sourceMetadata.tool`（`conversationId`、`stepIndex`、`toolCall.name == "invoke_subagent"`）。舊版 CLI（7/206 個子會話）無此目錄。
   - **來源 D（父子會話的 `transcript_full.jsonl`）**：新版子會話（198/206）以 `type: "SYSTEM_MESSAGE"`（內含 `[Message] timestamp=... sender=<parent_id> priority=... content=<prompt>`）作為第 0 步，完全沒有 `USER_INPUT` 步；舊版子會話（8/206）則以 `type: "USER_INPUT"` 作為第 0 步（不含 `sender` UUID）。
3. **後端現有三個斷點導致階層與名稱遺失**：
   - [`src/antigravity.rs`](../../src/antigravity.rs#L127-L147) 的 `parse_session` 將 `parent_session_id`、`agent_nickname`、`agent_role` 全部寫死為 `None`。
   - [`src/db.rs`](../../src/db.rs#L5235-L5301) 的共用寫入函式 `insert_usage_entries` 在 `INSERT INTO usage_entries` SQL 中**漏掉了 `parent_session_id`、`agent_nickname`、`agent_role` 三個欄位**（即使 `UsageEntry` 有填值也會在寫入 SQLite 時被丟棄）。
   - [`src/db.rs`](../../src/db.rs#L1439-L1484) 的 `get_antigravity_session_name` 與 [`src/timeline.rs`](../../src/timeline.rs#L48-L273) 的 `parse_antigravity_timeline` 只處理 `"USER_INPUT"`，完全忽略 `"SYSTEM_MESSAGE"`，導致 96%（198/206）的子會話抓不到初始任務名稱（退化為 `"Start Coding Session"`），且時間軸抽屜與搜尋功能看不到子代理的任務提示詞。

* * *

## 一、Antigravity 第一手資料結構（Primary Sources）

本節依據 `~/.gemini/antigravity-cli/` 下的實機資料與 Antigravity CLI 二進位檔（`~/.local/bin/agy`）內嵌之 Protobuf Descriptor 進行交叉驗證（所有敏感提示詞、本機路徑與識別碼均已去識別化）。

### 1. 目錄與檔案拓撲

每個頂層會話與每個 Subagent 都是獨立的 UUID 目錄與 SQLite 資料庫：

```text
~/.gemini/antigravity-cli/
├── brain/
│   ├── <parent_session_id>/
│   │   └── .system_generated/
│   │       ├── logs/
│   │       │   ├── transcript.jsonl          # 截斷版日誌（含 truncated_fields）
│   │       │   └── transcript_full.jsonl     # 完整版日誌（含每步 Token 與完整工具輸出）
│   │       ├── messages/
│   │       │   ├── <message_uuid>.json       # 子代理透過 send_message 回傳給父代理的訊息
│   │       │   └── read.json
│   │       └── subagents/
│   │           └── <child_session_id>.json   # 父會話記錄的子代理描述與狀態檔
│   └── <child_session_id>/
│       └── .system_generated/
│           ├── logs/
│           │   ├── transcript.jsonl
│           │   └── transcript_full.jsonl
│           ├── messages/
│           │   ├── <message_uuid>.json       # 父代理透過 invoke_subagent / send_message 送入的訊息
│           │   └── read.json
│           └── subagents/
│               └── <grandchild_id>.json      # 若子代理再衍生下一層子代理（Nested Subagent）
├── conversations/
│   ├── <parent_session_id>.db                # 父會話 SQLite（含 trajectory_metadata_blob、steps、gen_metadata）
│   └── <child_session_id>.db                 # 子會話 SQLite（含 parent_conversation_id 與 subagent_spec）
└── conversation_summaries.db                 # 全域會話摘要快取 SQLite
```

### 2. 來源 A：`conversations/<session_id>.db` 的 `CortexTrajectoryMetadata` Protobuf

TokenUsageInsights 目前已於 [`src/antigravity_metadata.rs`](../../src/antigravity_metadata.rs#L12-L79) 開啟 `conversations/<session_id>.db` 並解析 `trajectory_metadata_blob` 資料表的 Protobuf wire format（目前僅讀取 `field 7 = workspace_uris` 與 `field 1 = workspaces`）。

經解析 `~/.local/bin/agy` 內嵌的 `exa.cortex_pb.CortexTrajectoryMetadata` 與 `exa.cortex_pb.SubagentSpec` Protobuf `DescriptorProto`，完整欄位定義如下：

```protobuf
// Message: .exa.cortex_pb.CortexTrajectoryMetadata (table: trajectory_metadata_blob.data)
message CortexTrajectoryMetadata {
  repeated CortexWorkspaceMetadata workspaces = 1;
  google.protobuf.Timestamp created_at = 2;
  string initialization_state_id = 3;
  AgentScriptItem agent_script = 4;
  string parent_conversation_id = 5;       // 直接父會話 UUID（頂層會話無此欄位）
  string root_conversation_id = 6;         // 根會話 UUID（頂層會話等於自身 UUID）
  repeated string workspace_uris = 7;      // 工作區 URI（如 file:///example/...）
  SubagentSpec subagent_spec = 8;          // 子代理規格（僅 Subagent 會話具備）
  CustomAgentSpec static_config = 10;      // 代理型別靜態設定（field 1 = type_name）
  ExecutableScriptSpec executable_spec = 11;
  SourceMetadata source_metadata = 12;
  repeated int32 mendel_experiment_ids = 15;
  bool is_battle_mode_fork = 16;
  int32 nesting_depth = 17;                // 巢狀深度（頂層無此欄位；第一層子代理 = 1；第二層 = 2）
  string project_id = 18;
  string environment_id = 19;
  repeated string tags = 20;
  repeated StringReplacement replacements = 21;
  BattleModeMetadata battle_mode_metadata = 22;
  RemoteControlDetails remote_control_details = 23;
}

// Message: .exa.cortex_pb.SubagentSpec (CortexTrajectoryMetadata.field 8)
message SubagentSpec {
  string type_name = 1;       // 子代理型別，例如 "self"、"research"、"teamwork_preview_worker"
  string role = 2;            // 子代理角色標題，例如 "Antigravity Subagent Researcher"
  string initial_prompt = 3;  // 啟動子代理時的初始 Prompt 全文
  bool inherit = 4;           // Workspace 模式：inherit
  string branch = 5;          // Workspace 模式：branch
  string workspace_uri = 6;
  Model model = 7;
  ModelTier model_tier = 8;
  bool run_as_task = 9;
  string share = 10;          // Workspace 模式：share
  bool fork = 11;
}
```

實機驗證結果（針對 `conversations/*.db` 中全部 206 個子代理資料庫）：

- **頂層會話**：`field 5`（`parent_conversation_id`）不存在、`field 8`（`subagent_spec`）不存在、`field 17`（`nesting_depth`）不存在；`field 6`（`root_conversation_id`）為自身 `session_id`。
- **第一層子代理（Depth 1）**：`field 5` 為父會話 ID、`field 6` 為根會話 ID（同 `field 5`）、`field 17 = 1`，`field 8` 完整包含 `type_name`（field 1）、`role`（field 2）、`initial_prompt`（field 3）。
- **第二層巢狀子代理（Depth 2，本機共 9 個中間父代理衍生出 99 個孫代理）**：
  - `field 5`（`parent_conversation_id`）指向**直接父代理**（Depth 1 子代理）的 UUID。
  - `field 6`（`root_conversation_id`）指向**最頂層根會話**（Depth 0）的 UUID。
  - `field 17`（`nesting_depth`）為 `2`。

### 3. 來源 B：`brain/<parent_id>/.system_generated/subagents/<child_id>.json`

當父會話呼叫 `invoke_subagent` 成功建立子代理時，Antigravity 會在父會話的 `.system_generated/subagents/` 目錄下為每個子代理建立一個 `<child_id>.json`：

```json
{
  "conversationId": "22222222-2222-4222-8222-222222222222",
  "subagentDescriptor": {
    "typeName": "self",
    "role": "Antigravity Subagent Researcher"
  },
  "state": "SUBAGENT_STATE_ALIVE",
  "spawnStepIndex": 13,
  "workspaceUris": [
    "file:///example/project"
  ]
}
```

實機掃描全數 219 個 `.system_generated/subagents/*.json` 的欄位分布：

- 頂層鍵集合固定為：`["conversationId", "spawnStepIndex", "state", "subagentDescriptor", "workspaceUris"]`（舊版少數無 `workspaceUris`）。
- `state` 枚舉值：`SUBAGENT_STATE_ALIVE`、`SUBAGENT_STATE_KILLED`。
- `subagentDescriptor` 鍵集合：`typeName`（如 `"self"`、`"research"`、`"image-generator"`、`"teamwork_preview"`、`"teamwork_preview_orchestrator"`、`"teamwork_preview_worker"`、`"teamwork_preview_reviewer"`、`"teamwork_preview_challenger"`、`"teamwork_preview_explorer"`、`"teamwork_preview_spec_miner"`、`"teamwork_preview_test_writer"`、`"teamwork_preview_auditor"`、`"teamwork_preview_victory_auditor"`）與 `role`（人類可讀的職務名稱，如 `"Audio Builder Agent"`、`"Independent Review Agent"`）。
- 特例：本機有 1 筆被使用者立即取消的子代理（`44444444-4444-4444-8444-444444444444`），其 `subagentDescriptor` 為空物件 `{}`。
- 與來源 A（SQLite `CortexTrajectoryMetadata`）比對：在所有 206 個同時擁有 `.db` 與父目錄 `subagents/<child_id>.json` 的子會話中，`(parent_id, typeName, role)` **206/206 完全一致（0 筆不符）**。此外，對於測試環境（僅構造 `brain/` 目錄而未構造 `conversations/*.db`）或 `.db` 尚未落盤的情境，此檔案可提供免 SQLite 的純檔案階層解析。

### 4. 來源 C：`brain/<session_id>/.system_generated/messages/<message_uuid>.json`

在新版 Antigravity CLI 中，代理間訊息（包含 `invoke_subagent` 初始派工與後續 `send_message` 雙向通訊）會寫入接收方 `brain/<recipient_id>/.system_generated/messages/<message_uuid>.json`：

```json
{
  "id": "55555555-5555-4555-8555-555555555555",
  "recipient": "22222222-2222-4222-8222-222222222222",
  "sender": "11111111-1111-4111-8111-111111111111",
  "priority": "MESSAGE_PRIORITY_HIGH",
  "timestamp": "2026-10-28T17:01:31.587209Z",
  "renderDetails": {
    "messageTitle": "Message from Parent Agent"
  },
  "content": "<REDACTED_TASK_PROMPT>",
  "sourceMetadata": {
    "tool": {
      "conversationId": "11111111-1111-4111-8111-111111111111",
      "stepIndex": 12,
      "toolCall": {
        "id": "call_1372492",
        "name": "invoke_subagent",
        "argumentsJson": "{\"Prompt\":\"<REDACTED>\",\"Role\":\"Antigravity Subagent Researcher\",\"TypeName\":\"self\"}",
        "thinkingSignature": "<REDACTED_BASE64>"
      }
    }
  }
}
```

注意事項：

- 當父代理使用**批次模式**呼叫 `invoke_subagent`（傳入 `{"Subagents": [{"Role": "...", "TypeName": "...", "Prompt": "..."}, ...]}`）時，每個子代理收到的 `argumentsJson` 都是整個 `Subagents` 陣列的 JSON 字串，無法單靠 `argumentsJson` 直接一對一取得該子代理的 `Role`（必須比對 `content == Prompt`，或直接改讀來源 A / 來源 B）。
- 舊版 Antigravity CLI（本機 206 個子代理中有 7 個，建立於 2026-09-11 至 2026-09-15）尚未引入 `.system_generated/messages/` 目錄，因此不能將 `messages/*.json` 作為唯一的階層來源。

### 5. 來源 D：父子會話的 `transcript_full.jsonl` 事件格式

#### (1) 父會話呼叫 `invoke_subagent` 與工具結果步驟

父會話在 `PLANNER_RESPONSE` 的 `tool_calls` 陣列中記錄 `invoke_subagent`（注意：`args` 在不同版本或情況下可能是 **JSON 字串**或 **JSON 物件**，且支援單一子代理參數或批次 `Subagents` 陣列）：

```json
{
  "step_index": 11,
  "source": "MODEL",
  "type": "PLANNER_RESPONSE",
  "status": "DONE",
  "created_at": "2026-10-28T17:01:11Z",
  "input_tokens": 2666,
  "cache_read_tokens": 19861,
  "output_tokens": 782,
  "thinking": "<REDACTED>",
  "tool_calls": [
    {
      "name": "invoke_subagent",
      "args": "{\"Prompt\": \"<REDACTED>\", \"Role\": \"Antigravity Subagent Researcher\", \"TypeName\": \"self\"}"
    }
  ]
}
```

緊接著對應的 `GENERIC` 步驟記錄產生的子會話 `conversationId`（單一子代理為 `Created the following subagent:`，批次子代理為 `Created the following subagents:` 後接多個獨立 JSON 區塊）：

```json
{
  "step_index": 12,
  "source": "MODEL",
  "type": "GENERIC",
  "status": "DONE",
  "created_at": "2026-10-28T17:01:29Z",
  "content": "Created At: 2026-10-29T01:01:29+08:00\nCompleted At: 2026-10-29T01:01:31+08:00\nCreated the following subagent:\n{\n  \"conversationId\":  \"22222222-2222-4222-8222-222222222222\",\n  \"logAbsoluteUri\":  \"file:///path/to/.gemini/antigravity-cli/brain/22222222-2222-4222-8222-222222222222/.system_generated/logs/transcript.jsonl\",\n  \"workspaceUris\":  [\n    \"file:///example/project\"\n  ]\n}\nThe subagent will send you a message when it has completed its task or requires guidance. There is no need to poll for its response."
}
```

#### (2) 子會話的初始任務步驟（新版 `SYSTEM_MESSAGE` vs 舊版 `USER_INPUT`）

本機 206 個擁有 `transcript_full.jsonl` 的子會話中：

- **新版（198 個，佔 96.1%）**：全程**沒有任何 `USER_INPUT` 步驟**，第 0 步為 `source: "SYSTEM", type: "SYSTEM_MESSAGE"`：
  ```json
  {
    "step_index": 0,
    "source": "SYSTEM",
    "type": "SYSTEM_MESSAGE",
    "status": "DONE",
    "created_at": "2026-10-28T17:01:31Z",
    "content": "The following is a <SYSTEM_MESSAGE> not actually sent by the user. It is provided by the system as important information to pay attention to.\n\n<SYSTEM_MESSAGE>\n[Message] timestamp=2026-10-28T17:01:31Z sender=11111111-1111-4111-8111-111111111111 priority=MESSAGE_PRIORITY_HIGH content=<REDACTED_TASK_PROMPT>\n</SYSTEM_MESSAGE>"
  }
  ```
- **舊版（8 個，佔 3.9%）**：第 0 步為 `source: "USER_EXPLICIT", type: "USER_INPUT"`，格式與一般使用者輸入相同（`<USER_REQUEST>\n...\n</USER_REQUEST>`）。

#### (3) `SYSTEM_MESSAGE` 在所有會話中的四大類用途

掃描本機全部 1,014 個會話中的 3,102 筆 `SYSTEM_MESSAGE` 步驟，其內文格式可歸納為四類：

| 類別 | 筆數 | `sender` 格式 | 內文特徵與語意 |
|---|---:|---|---|
| 背景命令完成通知 | 2,523 | `<own_session_id>/task-<N>` | `content=Task id "<session_id>/task-<N>" finished with result:\n...`（非同步 `run_command` 結束通知） |
| 代理間訊息（父 ↔ 子） | 506 | `<other_session_uuid>`（36 字元 UUID） | 父代理派工給子代理（子會話 step 0 或後續追問），或子代理透過 `send_message` 回報結果給父代理 |
| 系統子代理狀態通知 | 71 | `system` | 包含：<br>1. `Subagent <UUID> has gone idle. This was the subagent's most recent response: ...`<br>2. `The subagent <Role> (<TypeName>) with id "<UUID>" encountered an error and has either stopped or failed to start execution: ...`<br>3. `Subagent "<UUID>" has been explicitly canceled by the user...`<br>4. `[Notice] All your subagents and background tasks have been stopped due to server restart...` |
| Stop Hook 阻擋通知 | 2 | 無 `[Message]` 標頭 | `<SYSTEM_MESSAGE>\nStop hook blocked termination: ...` |

* * *

## 二、TokenUsageInsights 現有 Subagent 架構對照（Claude / Codex / Copilot）

### 1. 資料模型與 SQLite Schema

在 [`src/db.rs`](../../src/db.rs#L128-L155) 與 [`usage_entries` 資料表](../../src/db.rs#L770-L820) 中，每個回合紀錄已定義三個與 Subagent 相關的欄位：

- `parent_session_id: Option<String>`：指向直接父會話的 `session_id`。
- `agent_nickname: Option<String>`：子代理的識別名稱或工作流 ID。
- `agent_role: Option<String>`：子代理的角色或型別名稱。

在報表聚合階段（[`src/handlers/daily.rs:75-102`](../../src/handlers/daily.rs#L75-L102) 與 [`src/handlers/monthly.rs:156-191`](../../src/handlers/monthly.rs#L156-L191)），後端以 [`SessionIdentity`](../../src/session_identity.rs#L5-L10)（`assistant_type, source_kind, source_dir_key, session_id`）分組，並從該 session 最後一筆 `UsageEntry`（`latest_usage_entry`）取出 `parent_session_id`、`agent_nickname`、`agent_role` 與 `session_name`，回傳於 `SessionSummary` JSON。

### 2. Claude 與 Codex 的現有實作方式

| 維度 | Claude Code ([`src/db/claude.rs`](../../src/db/claude.rs#L363-L415)) | Codex ([`src/db/codex.rs`](../../src/db/codex.rs#L254-L369)) | Antigravity 現況 ([`src/antigravity.rs`](../../src/antigravity.rs#L127-L147)) |
|---|---|---|---|
| **檔案位置** | `projects/<proj>/<root_session>/subagents/agent-<id>.jsonl` 與 `.meta.json` | `sessions/**/rollout-*-<session_id>.jsonl` | `brain/<session_id>/.system_generated/logs/transcript_full.jsonl` + `conversations/<session_id>.db` |
| **`session_id`** | `agent-<agentId>` | 子 rollout 自身の `session_meta.payload.id` (UUID) | 子會話目錄名稱 `<session_id>` (UUID) |
| **`parent_session_id`** | 優先讀 `.meta.json` 的 `parentAgentId`（支援巢狀 `agent-<id>`），次選 `event.sessionId` 或路徑中的 `<root_session>` | 讀 `session_meta.payload.parent_thread_id`（若等於自身 `session_id` 則清空為 `None`） | **目前恆為 `None`** |
| **`session_name`** | 優先讀 `.meta.json` 的 `description`，次選首段使用者提示詞 | 優先讀發給該 `agent_path` 的首封 `Message Type: NEW_TASK` payload，次選首段提示詞 | 只讀 `USER_INPUT`（[`src/db.rs:1439`](../../src/db.rs#L1439)），**新版子代理全部回傳 `None` 並退化為 `"Start Coding Session"`** |
| **`agent_nickname`** | 讀 `.meta.json` 的 `name` 或路徑中的 `workflow_id` (`wf_...`) | 讀 `session_meta.payload.agent_nickname` | **目前恆為 `None`** |
| **`agent_role`** | 優先讀 `.meta.json` 的 `workflowPhase`，次選 `agentType`（如 `Explore`、`general-purpose`） | 讀 `session_meta.payload.agent_role` | **目前恆為 `None`** |
| **DB 寫入路徑** | `sync_claude_usage_logs_with_cancel` 獨立 `INSERT` 語句（有寫入上述三欄） | `sync_codex_usage_logs_with_cancel` 獨立 `INSERT` 語句（有寫入上述三欄） | 呼叫共用 `insert_usage_entries`（[`src/db.rs:5252`](../../src/db.rs#L5252)），**SQL 漏掉上述三欄** |

### 3. 前端樹狀渲染與互動機制 (`static/session-utils.js` & `static/app.js`)

前端已經具備完整且通用的多層級樹狀支援：

1. **父子配對鍵（[`static/session-utils.js:29-44`](../../static/session-utils.js#L29-L44)）**：
   - `sessionIdentityKey(s)` 回傳 `JSON.stringify([s.assistant_type, s.source_kind, s.source_dir_key, s.session_id])`。
   - `parentSessionIdentityKey(s)` 將 `s.session_id` 替換為 `s.parent_session_id`，其餘三個欄位（`assistant_type`、`source_kind`、`source_dir_key`）保持與子會話相同。
   - 由於 Antigravity 父會話與子會話位於同一個 `root` 目錄，兩者的 `assistant_type`（`"antigravity"`）、`source_kind`（`"antigravity-session"`）與 `source_dir_key`（`encode_hex(root)`）完全一致，只要 `parent_session_id` 填入父會話 UUID，`parentSessionIdentityKey` 就會自動精準配對。
2. **多層級樹狀展開與孤兒節點容錯（[`static/app.js:4513-4562`](../../static/app.js#L4513-L4562)）**：
   - `sortAndGetFlatSessions` 會在當日會話 Map 中尋找 `parent`。
   - 若父會話存在於當日列表，子會話會掛入 `parent.children`，並依 `depth`（`1, 2, ...`）遞迴扁平化，產生縮排（`paddingLeft = s.depth * 16`）與樹狀連接線（`<span class="tree-connector">`）。
   - 若父會話不在當日列表（例如跨午夜執行，父會話在昨天、子會話在今天，或搜尋過濾後僅子會話命中），該子會話會作為 `depth = 0` root 節點排入列表，但因為 `isSubagent: depth > 0 || Boolean(node.parent_session_id)` 為 `true`（[`static/app.js:4547`](../../static/app.js#L4547)），仍會顯示 `Subagent` 標籤、Role / Nickname 徽章，並在 Tooltip 顯示父會話 ID（`parentName: parentName || node.parent_session_id`）。
   - `getRootParentKey`（[`static/app.js:4696-4721`](../../static/app.js#L4696-L4721)）已內建 Cycle 偵測，可防止異常循環參照造成無窮迴圈，並支援滑鼠移入時整棵家族樹（Root + Children + Grandchildren）同步 `.family-highlight` 高亮。
3. **Subagent 徽章顯示邏輯（[`static/app.js:4751-4771`](../../static/app.js#L4751-L4771) 與 [`4991-5017`](../../static/app.js#L4991-L5017)）**：
   - 第一列顯示：`<span class="badge subagent-badge">子代理</span>` + `agent_nickname`（`.agent-nickname-badge`）+ `agent_role`（`.agent-role-badge`，自動過濾掉無語意的 `"subagent"` / `"sub-agent"`）。
   - 第二列顯示：`session_name`（`.session-name-text`）。
   - 第三列顯示：`session_id`（`.session-id-sub`）。
   - 點擊開啟 Timeline Drawer 時，抽屜頂部 Metadata 面板亦會顯示 `meta-nickname` 與 `meta-role`。

* * *

## 三、Antigravity 整合設計與欄位映射建議

### 1. Subagent 欄位語意映射

對照 Antigravity 的 `SubagentSpec`（`type_name`、`role`、`initial_prompt`）與前端列表／抽屜的視覺呈現，建議採用以下映射規則：

| `UsageEntry` 欄位 | 建議映射之 Antigravity 資料 | 範例值 | 前端顯示位置與理由 |
|---|---|---|---|
| `parent_session_id` | 直接父會話 `conversationId`（來源 A `field 5` 或來源 B 父目錄名稱或來源 C/D `sender`） | `"11111111-1111-4111-8111-111111111111"` | 決定父子樹狀縮排、連接線、家族 Hover 高亮與父代理 Tooltip |
| `agent_role` | `subagent_spec.role` / `subagentDescriptor.role`（若為空則 fallback 至 `type_name`） | `"Antigravity Subagent Researcher"`、`"Audio Builder Agent"` | 顯示於 `.agent-role-badge` 與抽屜「角色 (Role)」欄位；這是使用者或 Orchestrator 賦予子代理最具辨識度的工作職稱 |
| `agent_nickname` | `subagent_spec.type_name` / `subagentDescriptor.typeName`（若與 `agent_role` 相同則設為 `None` 避免重複） | `"self"`、`"research"`、`"teamwork_preview_worker"` | 顯示於 `.agent-nickname-badge` 與抽屜「暱稱 (Nickname)」欄位，讓使用者一眼看出子代理使用的底層 Agent 型別 |
| `session_name` | 子會話的初始任務提示詞摘要（前 100 字元，單行化） | `"Investigate GitHub Issue #73 (\"Antigravity 支援 subagent..."` | 顯示於 `.session-name-text` 主標題列，與頂層會話及 Claude / Codex 子代理命名慣例一致 |

> **為何不把 `role` 放進 `session_name`？**
> 在列表與 Timeline Drawer 中，`agent_role` 已經有專屬的彩色徽章（`.agent-role-badge`）與抽屜欄位（`#meta-role`），而 `session_name` 則是緊接在徽章下方的任務摘要文字（且支援 100 字元預覽與完整 Tooltip）。若 `session_name` 設為初始任務提示詞（當無提示詞時才 fallback 至 `role`），使用者就能在列表同時看到「`[子代理] [self] [Antigravity Subagent Researcher]`」與下方的「`Investigate GitHub Issue #73...`」，資訊密度最高且不重複。

### 2. 多重來源擷取優先序（Fallback Chain）

為了同時兼顧 **100% 歷史相容性（含舊版無 `messages/` 的子代理）**、**同步順序無關性（子會話比父會話先同步或父會話目錄被刪除）** 與 **單元測試便利性（測試 fixture 未必建立 SQLite `.db`）**，建議整合以下擷取順序：

1. **第一優先：`conversations/<session_id>.db` 的 `trajectory_metadata_blob`（[`src/antigravity_metadata.rs`](../../src/antigravity_metadata.rs)）**
   - 擴充 `SessionMetadata` 結構：
     ```rust
     #[derive(Default, Clone, Debug, PartialEq, Eq)]
     pub struct SessionMetadata {
         pub models: HashMap<u32, String>,
         pub cwd: Option<String>,
         pub parent_session_id: Option<String>,
         pub agent_type: Option<String>,
         pub agent_role: Option<String>,
         pub initial_prompt: Option<String>,
     }
     ```
   - 在 `read_session_metadata` 讀取 `trajectory_metadata_blob` 時，一併從 `protobuf_fields(&blob)` 解析：
     - `field 5` (`parent_conversation_id`) -> 驗證為安全 session ID（且不等於自身 `session_id`）後存入 `parent_session_id`。
     - `field 8` (`subagent_spec`) -> 以 `protobuf_fields` 解析內部欄位：
       - `field 1` (`type_name`) -> `agent_type`
       - `field 2` (`role`) -> `agent_role`
       - `field 3` (`initial_prompt`) -> `initial_prompt`
2. **第二優先（Fallback）：掃描 `brain/*/.system_generated/subagents/<session_id>.json` 或 `brain/<session_id>/.system_generated/messages/*.json`**
   - 若 `conversations/<session_id>.db` 不存在（例如輕量測試 fixture 或純 `brain/` 備份）：
     - 可檢查 `brain/*/.system_generated/subagents/<session_id>.json`（由所在父目錄名稱取得 `parent_session_id`，並從 `subagentDescriptor` 取得 `typeName` 與 `role`）。
     - 或從子會話自身の `transcript_full.jsonl` 第一筆 `SYSTEM_MESSAGE` 解析 `[Message] ... sender=<parent_uuid> ... content=<prompt>`，若 `sender` 為合法 UUID（不含 `/task-` 且非 `"system"`）且該會話沒有更早的 `USER_INPUT`，即可在無 `.db` 時推導出 `parent_session_id` 與初始提示詞。

### 3. 子會話名稱擷取 (`get_antigravity_session_name`)

目前 [`src/db.rs:1439-1484`](../../src/db.rs#L1439-L1484) 的 `get_antigravity_session_name` 只觀測 `Some("USER_INPUT")`。應補強如下：

1. 定義輔助函式解析 `SYSTEM_MESSAGE` 內文：
   - 剝除開頭固定的說明前綴與 `<SYSTEM_MESSAGE>` / `</SYSTEM_MESSAGE>` 標籤。
   - 解析 `[Message] timestamp=<ts> sender=<sender> priority=<prio> content=<body>`。
   - 判斷 `sender`：
     - 若 `sender == "system"` 或 `sender.contains("/task-")`：屬於系統狀態或背景命令輸出，**不可**作為會話名稱或 `UserPrompt`。
     - 若 `sender` 為另一個會話的 ID（例如父代理或子代理的 UUID）：屬於代理間訊息（Agent-to-Agent Message），其 `body` 即為傳遞的任務提示詞或回報內容。
2. 在 `get_antigravity_session_name` 中：
   - 若 `USER_INPUT` 存在（頂層會話或舊版 8 個子會話），維持既有 `<USER_REQUEST>` 擷取邏輯。
   - 若遇到來自其他代理的 `SYSTEM_MESSAGE` 且當時尚未完成名稱選擇（例如新版 198 個子會話的第 0 步），將其 `body` 傳入 `selector.observe_user_prompt(body)`。
   - 若 `transcript_full.jsonl` 仍未取得名稱，最後可 fallback 至 `SessionMetadata.initial_prompt`（取前 100 字元）或 `agent_role`。

### 4. 時間軸抽屜（Timeline Drawer）與提示詞搜尋（Session Search）

在 [`src/timeline.rs:48-273`](../../src/timeline.rs#L48-L273) 的 `parse_antigravity_timeline` 中新增 `"SYSTEM_MESSAGE"` 分支處理：

1. **代理間訊息（`sender` 為其他會話 ID，非 `"system"` 且不含 `"/task-"`）**：
   - 在**子會話**中，這是父代理透過 `invoke_subagent` 或 `send_message` 下達的任務指令；在**父會話**中，這是子代理透過 `send_message` 回報的研究／實作結果。
   - 將其轉為 `TimelineItem::UserPrompt { timestamp, prompt: body, context: None, turn_no }`（或在 `metadata` 帶入 `sender` 資訊）。如此一來：
     - 點開子代理的 Timeline Drawer 時，最上方第 1 回合就能清楚展開閱讀父代理下達的完整任務 Prompt（不再只有孤立的 `AgentReply` 與 `ToolStep`）。
     - 當父代理中途透過 `send_message` 追問子代理時，也會在時間軸中呈現為新的回合提示。
     - [`src/session_search.rs:37-45`](../../src/session_search.rs#L37-L45) 的 `timeline_matches_user_prompt` 會自動支援搜尋子代理的任務提示詞！
2. **系統子代理生命週期與錯誤通知（`sender == "system"`）**：
   - 當內文符合子代理狀態變更時，轉為 `TimelineItem::SystemStatus`，讓父會話時間軸能直觀看到子代理的完成、錯誤或取消狀態：
     - `Subagent <id> has gone idle...` -> `status_type: "subagent_completed"`
     - `The subagent ... encountered an error...` -> `status_type: "subagent_failed"`
     - `Subagent "<id>" has been explicitly canceled...` -> `status_type: "subagent_canceled"`
     - `[Notice] All your subagents and background tasks have been stopped...` -> `status_type: "system_notice"`
3. **背景任務完成通知（`sender` 含 `"/task-"`）**：
   - 這是 `run_command` 背景執行的輸出回呼，維持略過或作為工具步驟補充，不轉為 `UserPrompt`，避免汙染使用者提示詞與搜尋結果。
4. **Timeline Drawer 的 `metadata` 補齊**：
   - 與 Codex（[`src/timeline.rs:1006-1014`](../../src/timeline.rs#L1006-L1014)）一致，在 [`src/session_details.rs`](../../src/session_details.rs) 或 `parse_antigravity_timeline` 中將 `parent_session_id`、`agent_nickname`、`agent_role` 寫入回傳的 `metadata` Map，確保抽屜開啟後 `#meta-nickname` 與 `#meta-role` 不被清空。

* * *

## 四、邊界情況與潛在陷阱（Edge Cases）

### 1. `insert_usage_entries` 漏寫欄位問題

- **現況**：[`src/db.rs:5235-5301`](../../src/db.rs#L5235-L5301) 的 `insert_usage_entries` 是 Antigravity（以及 Pi、OMP、Muse、MiniMax Code）共用的批次寫入函式，但目前 SQL 欄位清單停留在 `reasoning_effort, source_dir_key`，**未包含 `parent_session_id, agent_nickname, agent_role`**。
- **影響**：若只修改 `src/antigravity.rs` 而不修改 `insert_usage_entries`，解析出的 `parent_session_id`、`agent_nickname`、`agent_role` 永遠不會寫入 SQLite `usage_entries` 表。
- **對策**：在 `insert_usage_entries` 的 `INSERT INTO usage_entries` 補上 `parent_session_id, agent_nickname, agent_role` 三個欄位與對應的 `entry.parent_session_id.as_deref()`、`entry.agent_nickname.as_deref()`、`entry.agent_role.as_deref()` 參數（對其他未設定這三欄的助理，其值為 `None` 即 SQLite `NULL`，完全相容）。

### 2. 已同步歷史資料的重新匯入（Parser Version Bump）

- **現況**：[`src/db.rs:1507-1536`](../../src/db.rs#L1507-L1536) 使用 `crate::antigravity::PARSER_VERSION`（目前為 `2`）連同 `transcript_full.jsonl`、`<session_id>.db`、`<session_id>.db-wal` 的檔案大小與修改時間計算 `sync_state` 指紋；若指紋未變則跳過解析。
- **影響**：使用者本機已同步過的 206 個歷史 Subagent 會話，若不變更指紋，升級後不會自動補上 `parent_session_id`、`session_name`、`agent_nickname` 與 `agent_role`。
- **對策**：將 [`src/antigravity.rs:10`](../../src/antigravity.rs#L10) 的 `pub(crate) const PARSER_VERSION: u32` 由 `2` 遞增為 `3`。因為 `sync_antigravity_usage_logs_with_cancel` 在重跑時會先 `DELETE FROM usage_entries WHERE assistant_type = 'antigravity' AND session_id = ? ...` 再重新插入（[`src/db.rs:1570-1578`](../../src/db.rs#L1570-L1578)），只需 bump `PARSER_VERSION = 3` 即可無痛完成所有歷史會話的原地升級，無須額外撰寫破壞性的資料庫清除 migration。

### 3. 父子會話目錄的同步順序與延遲落盤

- **情境 A（子會話先於父會話被掃描）**：`find_session_files` 依 UUID 字典序排序（[`src/antigravity.rs:31`](../../src/antigravity.rs#L31)），因此子會話 UUID（如 `11111111-...`）可能排在父會話 UUID（如 `99999999-...`）之前同步。
  - 因為子會話的 `parent_session_id`、`agent_role`、`agent_nickname` 直接儲存在**子會話自己**的 `conversations/<child_id>.db` 與 `transcript_full.jsonl` 中，並有父目錄 `.system_generated/subagents/<child_id>.json` 作為備援，子會話的解析完全不依賴父會話是否已經先寫入 TokenUsageInsights 的 SQLite。
- **情境 B（父會話 `.system_generated/subagents/<child_id>.json` 在同步指紋中）**：
  - 若使用父目錄的 `.system_generated/subagents/<child_id>.json` 作為無 `.db` 時的備援來源，可將對應的 `subagents/<child_id>.json`（若存在）一併納入 [`src/db.rs:1511-1521`](../../src/db.rs#L1511-L1521) 的指紋雜湊計算中，確保 metadata 後到時也能觸發重同步。

### 4. 單一會話啟動多個 Subagent 與批次 `invoke_subagent`

- Antigravity 支援在單一 `invoke_subagent` 工具呼叫中傳入 `Subagents: [...]` 一次平行啟動多個子代理（例如單一父會話衍生 45 個子代理，或於單一步驟平行啟動 3 個子代理）。
- 每個子代理都有各自獨立的 `<child_id>`、`conversations/<child_id>.db` 與 `brain/<parent_id>/.system_generated/subagents/<child_id>.json`。
- 因此**不應**從父會話 `transcript_full.jsonl` 的 `invoke_subagent` 工具參數反推子代理 metadata（因為批次呼叫時難以在未解析後續 `GENERIC` 輸出前對應哪個 `Role` 屬於哪個 `conversationId`），而應直接讀取子會話自身的 `CortexTrajectoryMetadata.subagent_spec`（來源 A）與 `subagents/<child_id>.json`（來源 B），既精確又天然支援批次啟動。

### 5. 多層巢狀 Subagent（Nested Subagents）

- 本機實測存在 3 層架構（Root `11111111-...` -> Depth 1 `22222222-...` [`teamwork_preview`] -> Depth 2 `33333333-...` [`teamwork_preview_orchestrator`] -> Depth 3 Workers）。
- 在 `CortexTrajectoryMetadata` 中：
  - `field 5` (`parent_conversation_id`) 記錄的是**直接父代理**（Depth $N-1$），而非頂層根會話。
  - `field 6` (`root_conversation_id`) 記錄的才是最頂層根會話（Depth 0）。
- 因此，`UsageEntry.parent_session_id` **必須使用 `field 5` (`parent_conversation_id`)**，不可誤用 `field 6` (`root_conversation_id`)，如此前端 `sortAndGetFlatSessions` 才能正確建構 Depth 0 -> Depth 1 -> Depth 2 -> Depth 3 的多層縮排樹。
- 同時必須維持自我參照防護：`if parent_session_id.as_deref() == Some(session_id) { parent_session_id = None; }`。

### 6. 子代理尚無 Token 用量或已被刪除目錄

- 本機 219 個 `subagents/<child_id>.json` 中，有 13 個子會話的 `brain/<child_id>/` 與 `conversations/<child_id>.db` 已被清理，另有少數剛啟動或立即取消的子代理尚未產生含 Token 的 `PLANNER_RESPONSE`。
- 現有 [`src/antigravity.rs:12-33`](../../src/antigravity.rs#L12-L33) 與 [`src/db.rs:1555-1557`](../../src/db.rs#L1555-L1557) 僅針對實際存在 `transcript_full.jsonl` 且至少有一筆 `PLANNER_RESPONSE` Token 紀錄的會話寫入 `usage_entries`。這與 Claude（[`src/db/claude.rs`](../../src/db/claude.rs)）及 Codex（[`src/db/codex.rs`](../../src/db/codex.rs)）只顯示有實際 Token 消耗之 Subagent 的行為完全一致。

### 7. Legacy Hook（`usage/usage-YYYY-MM-DD.jsonl`）相容性

- 若使用者同時留有舊版 statusline hook 產生的 `usage/usage-*.jsonl`（`source_kind = "legacy"`），[`src/db.rs:1526-1536`](../../src/db.rs#L1526-L1536) 與 [`1571-1575`](../../src/db.rs#L1571-L1575) 已確保原生 `antigravity-session` 會取代同一個 `session_id` 的 `legacy` 紀錄。
- 對於僅存於 `legacy`（已無 `brain/<session_id>/`）的古老紀錄，維持 `parent_session_id = None` 即可。

* * *

## 五、建議實作清單

1. **[`src/antigravity_metadata.rs`](../../src/antigravity_metadata.rs)**：
   - 於 `SessionMetadata` 新增 `parent_session_id`、`agent_type`、`agent_role`、`initial_prompt` 欄位。
   - 在 `read_session_metadata` 中從 `trajectory_metadata_blob`（`CortexTrajectoryMetadata`）解析 `field 5`（`parent_conversation_id`）與 `field 8`（`SubagentSpec` 的 `field 1 = type_name`、`field 2 = role`、`field 3 = initial_prompt`），並支援當 `.db` 不存在時從 `brain/*/.system_generated/subagents/<session_id>.json` 讀取備援 metadata。
2. **[`src/antigravity.rs`](../../src/antigravity.rs)**：
   - 將 `PARSER_VERSION` 由 `2` 升級為 `3`。
   - 新增 `SYSTEM_MESSAGE` 代理訊息解析輔助函式，供 `parse_session`、`get_antigravity_session_name` 與 `parse_antigravity_timeline` 共用。
   - 在 `parse_session` 中填入 `UsageEntry` 的 `parent_session_id`、`agent_nickname`（`agent_type`，若與 `agent_role` 相同則省略）與 `agent_role`（`role` 優先，否則 `agent_type`）。
3. **[`src/db.rs`](../../src/db.rs)**：
   - 在 `insert_usage_entries`（[`src/db.rs:5252-5296`](../../src/db.rs#L5252-L5296)）補齊 `parent_session_id`、`agent_nickname`、`agent_role` 欄位寫入。
   - 在 `get_antigravity_session_name`（[`src/db.rs:1439-1484`](../../src/db.rs#L1439-L1484)）支援從代理間 `SYSTEM_MESSAGE`（以及 `SessionMetadata.initial_prompt` / `agent_role` fallback）擷取子會話名稱。
4. **[`src/timeline.rs`](../../src/timeline.rs)**：
   - 在 `parse_antigravity_timeline`（[`src/timeline.rs:48-273`](../../src/timeline.rs#L48-L273)）支援將代理間 `SYSTEM_MESSAGE` 轉為 `TimelineItem::UserPrompt`，並將 `sender=system` 的子代理完成／錯誤／取消事件轉為 `TimelineItem::SystemStatus`。
5. **驗證與測試**：
   - 新增單元測試覆蓋：
     - `CortexTrajectoryMetadata` Protobuf 解析 `parent_conversation_id`（field 5）與 `SubagentSpec`（field 8：type_name、role、initial_prompt），含多層巢狀（Depth 1 & Depth 2）與自我參照防呆。
     - 新版 `SYSTEM_MESSAGE` 子代理與舊版 `USER_INPUT` 子代理的 `session_name`、`parent_session_id`、`agent_nickname`、`agent_role` 完整寫入 SQLite `usage_entries` 並由日報／月報 API 回傳。
     - `parse_antigravity_timeline` 與 `search_user_prompts` 能正確還原並搜尋子代理的 `SYSTEM_MESSAGE` 任務提示詞，同時忽略 `sender=<id>/task-<N>` 的背景任務輸出。
