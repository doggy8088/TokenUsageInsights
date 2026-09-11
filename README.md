# Token 戰情室

**Token 戰情室是本機優先的 AI Coding Agent Token 使用量與會話還原看板。** 它會讀取本機上的 Google Antigravity CLI、GitHub Copilot CLI、GitHub Copilot App、GitHub Copilot Chat（VS Code）、Codex Desktop、Codex CLI、Claude Code、Cursor、Grok Build、Pi Coding Agent、OMP 與 Muse Code 記錄，集中呈現每日、月度、年度的 Token 消耗、快取使用、推理 Token、估算費用、模型分佈、專案目錄分佈與完整 Session 時間軸。

本專案不會替你呼叫 AI 供應商 API 查詢資料；核心資料來源是本機日誌、Status Line 收集檔與本機 SQLite。

> 系統環境：支援 Windows 10/11 原生 PowerShell、macOS、Linux 與 WSL。

語言： [繁體中文](README.md) · [简体中文](README.zh-CN.md) · [English](README.en.md) · [日本語](README.ja.md) · [한국어](README.ko.md)

* * *

## 最短上手路徑

### 1. 一行啟動或安裝看板

已安裝 Node.js 18.18 或更新版本時，可直接執行，不會建立全域 npm 命令：

```bash
npx --yes token-usage-insights
```

若要安裝成固定的系統命令，可使用下列安裝腳本。

Linux / macOS：

```bash
curl -fsSL https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.sh | bash && "$HOME/.local/bin/token-usage-insights"
```

Windows PowerShell：

```powershell
irm https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.ps1 | iex; & "$HOME\bin\token-usage-insights.cmd"
```

`npx` 與安裝腳本都會下載目前平台的已編譯版本，不需要 Rust、Cargo、WSL 或手動解壓縮。指令執行後，看板會在本機執行。

開啟：

```text
http://localhost:3003
```

### 2. 依你使用的工具決定是否需要設定

| 工具 | 是否需要額外設定 | 預設資料來源 | 說明 |
| --- | --- | --- | --- |
| Google Antigravity CLI | 需要 | `~/.gemini/antigravity-cli/usage/usage-YYYY-MM-DD.jsonl` | 透過 `statusline-token.sh` 或 Windows `statusline-token.ps1` 收集 Token 資料 |
| GitHub Copilot CLI | 需要 | `~/.copilot/usage/usage-YYYY-MM-DD.jsonl` | 透過 `statusline-token.sh` 或 Windows `statusline-token.ps1` 收集 Token 資料 |
| GitHub Copilot App | 不需要 | `~/.copilot/data.db`、`~/.copilot/session-store.db` | 看板直接讀取 Copilot 桌面應用程式的本機 SQLite |
| GitHub Copilot Chat（VS Code） | 不需要 | VS Code `workspaceStorage/chatSessions` | 看板直接掃描 VS Code Stable 與 Insiders 的本機聊天 Session |
| Codex Desktop / CLI | 不需要 | `~/.codex/sessions`、`~/.codex/archived_sessions` | 看板會直接掃描 Codex 作用中與已封存的本機 Session 記錄 |
| Claude Code | 不需要 | `~/.claude/projects` | 看板會直接掃描 Claude Code 本機專案 Session 記錄 |
| Cursor | 不需要 | `~/.cursor/projects` | 看板會直接掃描 Cursor 本機 transcript，並唯讀取得可歸因的模型資訊 |
| Grok Build | 不需要 | `~/.grok/sessions` | 看板會直接掃描 Grok Build 自動保存的 `updates.jsonl` Session stream |
| Pi Coding Agent | 不需要 | `~/.pi/agent/sessions` | 看板會直接掃描 Pi Coding Agent 自動保存的本機 Session JSONL 檔案 |
| OMP | 不需要 | `~/.omp/agent/sessions` | 看板會直接掃描 OMP 自動保存的本機 Session JSONL 檔案 |
| Muse Code | 不需要 | `~/.local/share/muse/sessions` | 看板會直接掃描 Muse Code 自動保存的本機 Session JSONL 檔案 |

**只使用 Copilot App、VS Code Copilot、Codex Desktop、Codex CLI、Claude Code、Cursor、Grok Build、Pi Coding Agent、OMP 或 Muse Code 時，執行一行安裝指令並開啟看板即可。**

### Windows 原生使用

Windows 的一行安裝會建立 `%USERPROFILE%\bin\token-usage-insights.cmd` 啟動檔；不需要 Rust MSVC toolchain、Visual Studio Build Tools、WSL、Git Bash 或 `jq`。

Windows 預設使用下列原生路徑：

| 用途 | Windows 預設路徑 |
| --- | --- |
| SQLite | `%LOCALAPPDATA%\TokenUsageInsights\token_usage_insights.db` |
| Antigravity | `%USERPROFILE%\.gemini\antigravity-cli` |
| Copilot | `%USERPROFILE%\.copilot` |
| Codex | `%USERPROFILE%\.codex` |
| Claude Code | `%USERPROFILE%\.claude` |
| Cursor | `%USERPROFILE%\.cursor` |
| Grok Build | `%USERPROFILE%\.grok` |
| Pi Coding Agent | `%USERPROFILE%\.pi` |
| OMP | `%USERPROFILE%\.omp` |
| Muse Code | `%USERPROFILE%\.local\share\muse` |

看板內的設定指南會在 Windows 顯示 PowerShell 複製、設定與診斷命令。PowerShell collector 使用 .NET JSON 與檔案 API，不依賴 Bash、`jq`、`sed` 或 `awk`。

磁碟機代號、含空白或非 ASCII 字元的路徑，以及 UNC 路徑都會交由原生路徑 API 處理。SQLite 資料庫仍建議放在本機磁碟，以避免網路分享的 locking 語意差異。

* * *

## 支援功能

### 資料分析

- 每日、月度、年度 Token 統計
- 輸入、輸出、快取讀取、快取寫入、推理 Token 分拆
- 依 `pricing.csv` 進行本地估算費用
- Session 數、請求次數與 API 耗時統計
- 模型使用量排名
- Cursor 可由本機 `state.vscdb` 的 `agentKv` 記錄歸因至具體模型；無法唯一比對時保留為 `Unknown Model`
- 專案工作目錄統計
- 可排序的 Session 清單
- 自動讀取 GitHub Copilot App（桌面應用）`~/.copilot/data.db` 與 `session-store.db`

### Session 還原

- 右側抽屜式 Session 時間軸
- 使用者提示詞、助理回覆、推理內容與工具呼叫步驟
- 工具呼叫參數、退出碼、stdout、stderr
- Codex subagent 相關欄位，如 parent session、agent nickname、agent role
- Markdown 回覆渲染與內容清理

### 介面操作

- 九種 Coding Agent 徽章切換
- 每日、月度、年度視圖
- 日期、月份、年份快速切換
- 5 秒、10 秒、30 秒即時自動刷新
- 手動同步本機日誌到 SQLite
- 深色與淺色主題
- 正體中文、簡體中文、英文、日文與韓文介面切換
- 模型費用表檢視

* * *

## 網址參數（深層連結）

看板支援以網址查詢參數（Query String）直接開啟指定狀態的畫面，方便加入書籤、分享連結，或從其他工具跳轉過來。在看板上切換 Agent、視圖、日期、工作目錄或圖表類型時，網址也會自動更新為目前的狀態。

| 參數 | 適用視圖 | 可用值 | 說明 |
| --- | --- | --- | --- |
| `agent` | 全部 | `antigravity`、`copilot`、`codex`、`claude`、`cursor`、`grok`、`pi`、`omp`、`muse` | 指定要顯示的 Coding Agent。另支援 `claude-code`、`grok-build`、`pi-coding-agent`、`oh-my-pi`、`muse-code` 等別名寫法 |
| `tab` | 全部 | `daily`、`monthly`、`yearly` | 指定以日（每日）、月（月度）或年（年度）視圖顯示 |
| `date` | 全部 | `daily`：`YYYY-MM-DD`；`monthly`：`YYYY-MM`；`yearly`：`YYYY` | 指定要顯示的日期、月份或年份，格式會依 `tab` 自動對應 |
| `dir` | `daily` | 完整路徑、`~` 開頭的家目錄路徑，或唯一的路徑尾碼（如 `TokenUsageInsights`） | 指定每日視圖的工作目錄篩選。Windows 路徑不分大小寫；找不到符合目錄時會顯示全部 |
| `chart` | `daily` | `kline`、`trend` | 指定每日視圖的圖表類型：K 線圖或趨勢圖 |

範例（`http://localhost:3003` 為預設網址，請依實際 `HOST`/`PORT` 調整）：

```text
http://localhost:3003/?agent=copilot&tab=monthly&date=2026-08
http://localhost:3003/?agent=codex&tab=yearly&date=2026
http://localhost:3003/?agent=claude&tab=daily&date=2026-08-09&chart=trend
http://localhost:3003/?agent=copilot&tab=daily&date=2026-08-09&dir=~/projects/TokenUsageInsights
```

> 路徑含有 `~`、空白或非 ASCII 字元時請先進行 URL 編碼（`~` 可編碼為 `%7E`）。未提供的參數會沿用前次瀏覽的狀態（Cookie / localStorage）。

* * *

## Google Antigravity CLI 設定

Antigravity CLI 需要把本專案的 Status Line 腳本接到 `settings.json`。腳本會把每次對話後的 Token 累計與增量寫入：

```text
~/.gemini/antigravity-cli/usage/usage-YYYY-MM-DD.jsonl
```

### 1. 安裝收集腳本

完成一行安裝後，執行：

```bash
mkdir -p ~/.gemini/antigravity-cli && cp ~/.local/share/token-usage-insights/shell/antigravity/statusline-token.sh ~/.gemini/antigravity-cli/statusline-token.sh && chmod +x ~/.gemini/antigravity-cli/statusline-token.sh
```

若使用自訂安裝位置，請將指令中的 `~/.local/share/token-usage-insights` 替換為 `TOKEN_USAGE_INSIGHTS_INSTALL_DIR` 指定的位置。

### 2. 設定 `~/.gemini/antigravity-cli/settings.json`

若檔案不存在，可以建立以下內容。若檔案已存在，請只合併 `statusLine` 區塊，不要覆蓋原本設定。

```json
{
  "statusLine": {
    "type": "command",
    "command": "/ABSOLUTE/HOME/.gemini/antigravity-cli/statusline-token.sh",
    "padding": 1
  }
}
```

請將 `/ABSOLUTE/HOME` 替換成 `echo $HOME` 顯示的實際家目錄路徑，例如 `/Users/will` 或 `/home/will`。

### 3. 驗證

```bash
echo '{}' | ~/.gemini/antigravity-cli/statusline-token.sh
jq . ~/.gemini/antigravity-cli/settings.json
```

完成後重新進入 Antigravity CLI Session，狀態列會輸出類似格式：

```text
model-name • #3 • input 12.3k • cache 4.5k/0 • output 1.2k • reasoning 500 • total 18.5k
```

* * *

## GitHub Copilot CLI 設定

Copilot CLI 與 Antigravity CLI 一樣，需要把本專案的 Status Line 腳本接到 `settings.json`。腳本會把 Token 資料寫入：

```text
~/.copilot/usage/usage-YYYY-MM-DD.jsonl
```

### 1. 安裝收集腳本

完成一行安裝後，執行：

```bash
mkdir -p ~/.copilot && cp ~/.local/share/token-usage-insights/shell/copilot/statusline-token.sh ~/.copilot/statusline-token.sh && chmod +x ~/.copilot/statusline-token.sh
```

若使用自訂安裝位置，請將指令中的 `~/.local/share/token-usage-insights` 替換為 `TOKEN_USAGE_INSIGHTS_INSTALL_DIR` 指定的位置。

### 2. 設定 `~/.copilot/settings.json`

若檔案不存在，可以建立以下內容。若檔案已存在，請只合併 `statusLine` 區塊，不要覆蓋原本設定。

```json
{
  "statusLine": {
    "type": "command",
    "command": "/ABSOLUTE/HOME/.copilot/statusline-token.sh",
    "padding": 1
  }
}
```

請將 `/ABSOLUTE/HOME` 替換成 `echo $HOME` 顯示的實際家目錄路徑。

### 3. 驗證

```bash
echo '{}' | ~/.copilot/statusline-token.sh
jq . ~/.copilot/settings.json
```

完成後重新進入 Copilot CLI Session，狀態列會開始輸出並累積 Token 資料。

* * *

## GitHub Copilot App（桌面應用）

**Copilot App（Tauri 桌面應用）不需任何設定。** 看板會自動讀取本機 `~/.copilot/data.db` 與 `~/.copilot/session-store.db`，將 App session 的 token 使用量與 CLI / VS Code 合併顯示在 Copilot 頁面；Session 清單以 `App` 標示來源，與 `CLI`、`VS Code` 區分。

- 看板會在每次背景同步（每 5 秒）檢查這兩個 SQLite 並以 `(created_at, id)` 複合游標做增量同步，避免同一時間戳的多筆 event 重複 upsert，同一個 `(session_id, turn_index)` 不會重複寫入。
- App 的 `assistant_usage_events` 是 per-API-call 顆粒度；看板會依 Session、Turn、Agent 與模型聚合，保留同一回合的多模型歸因，再以 per-turn 統計供時間軸使用。
- Session 標題取自 `data.db.sessions.title`。

若 App 與 CLI 分家、或使用非預設目錄，可指定環境變數：

```bash
COPILOT_APP_DIR="/path/to/copilot-app-data" token-usage-insights
```

`COPILOT_APP_DIR` 會優先於 `COPILOT_DIR`，未設定時 fallback 到 `~/.copilot`。

* * *

## GitHub Copilot Chat（VS Code）設定

**VS Code Copilot Chat 不需要安裝 Status Line、Hook 或額外收集腳本。**看板會直接讀取本機 `workspaceStorage` 內的聊天 Session，並與 Copilot CLI 合併顯示；Session 清單會以 `VS Code` 或 `CLI` 標示來源。

支援 VS Code Stable 與 Insiders：

| 平台 | Stable | Insiders |
| --- | --- | --- |
| Windows | `%APPDATA%\Code\User\workspaceStorage` | `%APPDATA%\Code - Insiders\User\workspaceStorage` |
| macOS | `~/Library/Application Support/Code/User/workspaceStorage` | `~/Library/Application Support/Code - Insiders/User/workspaceStorage` |
| Linux | `~/.config/Code/User/workspaceStorage` | `~/.config/Code - Insiders/User/workspaceStorage` |

使用方式：

1. 以 VS Code 使用 GitHub Copilot Chat 產生至少一個聊天 Session。
2. 啟動看板或按右上角同步按鈕。
3. 在 Copilot 頁面查看合併後的統計與 Session 時間軸。

看板會完整回填現有 `chatSessions` 檔案，也會在檔案大小或修改時間變更時重新同步；沒有 Token 欄位的聊天 Session 仍會顯示，但 Token 數為 0。資料只讀取本機聊天檔案，不包含雲端 Session、Remote SSH 主機或 `state.vscdb`。

**快取讀取 Token 來源**：VS Code 的 `chatSessions` 檔案只記錄每個請求最後一次模型呼叫的 `promptTokens` 與累計的 `completionTokens`，並不記錄 Prompt Cache 的快取讀取數。看板會另外讀取 Copilot Chat 擴充功能在同一個工作區目錄下寫入的除錯記錄 `GitHub.copilot-chat/debug-logs/<sessionId>/main.jsonl`，把該回合所有模型呼叫的 `inputTokens`、`outputTokens` 與 `cachedTokens` 加總後，拆成非快取輸入、快取讀取與輸出 Token，成本估算也會依快取讀取費率計價。此除錯記錄由 VS Code 設定 `github.copilot.chat.agentDebugLog.fileLogging.enabled` 控制（部分使用者已由實驗功能開啟），且預設只保留最近 50 個 Session 的記錄；沒有除錯記錄的 Session 會回退使用 VS Code 內建的 Token 欄位，快取讀取會顯示為 0。

若 VS Code 使用 `--user-data-dir` 或 Portable Mode，可指定看板自訂的資料根目錄：

macOS / Linux：

```bash
VSCODE_USER_DATA_DIR="/path/to/vscode-user-data" token-usage-insights
```

Windows PowerShell：

```powershell
$env:VSCODE_USER_DATA_DIR = "C:\path\to\vscode-user-data"; & "$HOME\bin\token-usage-insights.cmd"
```

`VSCODE_USER_DATA_DIR` 應指向包含 `User/workspaceStorage` 的 VS Code 使用者資料目錄。Portable Mode 若環境變數指向 `data` 目錄，請改用 `VSCODE_PORTABLE_DATA_DIR`；看板會同時檢查 `data/user-data/User/workspaceStorage` 與 `data/User/workspaceStorage`。

* * *

## Codex 設定

**Codex Desktop 與 Codex CLI 都不需要安裝 Hook、Status Line 或額外收集腳本。**

看板會直接掃描：

```text
~/.codex/sessions
~/.codex/archived_sessions
```

使用方式：

1. 先正常使用 Codex Desktop 或 Codex CLI 產生至少一個 Session。
2. 啟動本專案。
3. 在左側選擇 Codex。
4. 按右上角同步按鈕，或等待背景同步。

注意事項：

- Codex 的身分憑證仍由 Codex 自身管理。
- 看板只讀取本機 Session 記錄並做分析。
- 每個 Session 會依 transcript 的 `originator` 顯示 `Desktop` 或 `CLI` 來源標記；無法判定的舊格式會維持未分類。
- API 額度資訊若有顯示，來源是最後一次本機 Session 日誌，不是即時線上查詢。

* * *

## Claude Code 設定

**Claude Code 不需要安裝 Hook、Status Line 或額外收集腳本。**

看板會直接掃描：

```text
~/.claude/projects
```

使用方式：

1. 先正常使用 Claude Code 產生至少一個專案 Session。
2. 啟動本專案。
3. 在左側選擇 Claude Code。
4. 按右上角同步按鈕，或等待背景同步。

注意事項：

- Claude Code 的身份憑證仍由 Claude Code 自身管理。
- 看板只讀取本地專案 Session 記錄並做分析。
- 若 `~/.claude/projects` 不存在，Claude Code 頁面會顯示無資料。

* * *

## Cursor 設定

**Cursor 不需要安裝 Hook、Status Line 或額外收集腳本。** 看板會直接掃描：

```text
~/.cursor/projects
```

Cursor 會將對話與 Agent transcript 保存於專案目錄下。看板也會唯讀掃描平台預設的 `Cursor/User/globalStorage/state.vscdb`，以其中的 `agentKv` 記錄歸因實際模型；無法唯一比對時會保留為 `Unknown Model`。

使用方式：

1. 先在 Cursor 中產生至少一個對話或 Agent Session。
2. 啟動或重新整理本專案看板。
3. 在左側選擇 Cursor。
4. 按右上角同步按鈕，或等待背景同步。

Cursor 本機資料不包含精確 Token 或官方帳務明細，因此 Token 依文字內容估算；只有在 `pricing.csv` 存在對應模型時才會估算費用，結果不等同官方帳單。非預設資料位置可使用 `CURSOR_DIR` 與 `CURSOR_STATE_DB` 指定。

* * *

## Grok Build 設定

**Grok Build 不需要安裝 Hook、Status Line 或額外收集腳本。** 看板會直接掃描：

```text
~/.grok/sessions
```

這裡採用 Grok Build 內建保存的 Session stream；不讀取舊規格中的
`~/.Grok/build/usage/usage-YYYY-MM-DD.jsonl`，也不需要在
`~/.Grok/build/settings.json` 設定 `statusLine`。

使用方式：

1. 先正常使用 Grok Build 產生至少一個 Session。
2. 啟動本專案。
3. 在左側選擇 Grok Build。
4. 按右上角同步按鈕，或等待背景同步。

Grok Build Session 可能只提供 context token snapshot，也可能包含 provider usage 與成本。看板會優先使用 provider usage/cost；只有 context snapshot 時，費用會依 `pricing.csv` 的 xAI API 價格估算，並在 Session 清單標示 `Context`，不代表 SuperGrok 或其他訂閱方案的週配額。

* * *

## Pi Coding Agent 設定

**Pi Coding Agent 不需要安裝 Hook、Status Line 或額外收集腳本。** 看板會直接掃描：

```text
~/.pi/agent/sessions
```

Pi Coding Agent 會把 Session 以樹狀目錄結構自動保存為本機 JSONL 檔案；看板會直接讀取這些 Session 記錄。

使用方式：

1. 先正常使用 Pi Coding Agent 產生至少一個 Session。
2. 啟動或重新整理本專案看板。
3. 在左側選擇 Pi Coding Agent。
4. 按右上角同步按鈕，或等待背景同步。

Pi Coding Agent 的成本一律直接讀取 Session 每個 turn 自行回報的 `usage.cost` 與相關 usage 資料，不會像 Grok Build 那樣回退到 context snapshot 估算；因為 Pi 原生就會提供權威的 token 與成本資訊。

* * *

## OMP 設定

**OMP 不需要安裝 Hook、Status Line 或額外收集腳本。** 看板會直接掃描：

```text
~/.omp/agent/sessions
```

OMP 是 Pi Coding Agent 的開源分支（<https://github.com/can1357/oh-my-pi>），會以完全相同的 JSONL 格式保存 Session。看板會直接讀取這些本機 Session 記錄。

使用方式：

1. 先正常使用 OMP 產生至少一個 Session。
2. 啟動或重新整理本專案看板。
3. 在左側選擇 OMP。
4. 按右上角同步按鈕，或等待背景同步。

OMP 的成本一律直接讀取 Session 每個 turn 自行回報的 `usage.cost` 與相關 usage 資料，不會像 Grok Build 那樣回退到 context snapshot 估算；因為 OMP 原生就會提供權威的 token 與成本資訊。

* * *

## Muse Code 設定

**Muse Code 不需要安裝 Hook、Status Line 或額外收集腳本。** 看板會直接掃描：

```text
~/.local/share/muse/sessions
```

Muse Code 會將 Session 依日期與 Session ID 分層保存為 `session.jsonl`。看板會解析其中的模型完成事件，分拆輸入、快取讀取、輸出與推理 Token，並還原使用者提示詞、工具步驟與 Agent 回覆時間軸。

使用方式：

1. 先正常使用 Muse Code 產生至少一個 Session。
2. 啟動或重新整理本專案看板。
3. 在左側選擇 Muse Code。
4. 按右上角同步按鈕，或等待背景同步。

Muse Code 的費用會依 Session 回報的模型與 `pricing.csv` 估算。若資料不在預設位置，可設定 `MUSE_DIR` 指向包含 `sessions` 的 Muse Code 資料目錄。

* * *

## 本地資料同步方式

啟動服務時，後端會初始化本機 SQLite 並立即同步一次資料。服務啟動後，也會每 5 秒背景同步一次。

SQLite 預設位置：

```text
~/.token-usage-insights/token_usage_insights.db
```

前端右上角的同步按鈕會呼叫：

```text
GET /api/:assistant/sync
```

這會觸發一次完整的本機日誌增量同步。

## 匯入 / 匯出（跨機器彙整）

**一般使用請直接使用看板右上角的匯出與匯入按鈕。** 安裝版只需要瀏覽器即可完成跨機器資料彙整，並支援最大 200 MB 的匯入檔案。

看板與 CLI 自 v0.9.0 起整合為同一個 `token-usage-insights` 執行檔。不帶參數會啟動看板；使用 `export`、`export-all`、`import` 子命令可直接操作使用量資料。主命令與各子命令均支援 `--help`、`-h`；較舊的安裝版本需更新或從原始碼建置。

`--agent` 會指定助理（`antigravity` / `copilot` / `codex` / `claude` / `cursor` / `grok` / `pi` / `omp` / `muse`）。

### 從原始碼使用 CLI

先建置一次：

```bash
cargo build --release --bin token-usage-insights
```

```bash
# 匯出日、月或年資料（輸出 JSON，含匯入唯一 id）
./target/release/token-usage-insights export --agent codex --date 2026-07 --out monthly-codex-2026-07.json
```

```bash
# 一次匯出所有 Agent、所有日期的使用量記錄
./target/release/token-usage-insights export-all --out all-usage.json

# 自動判斷 Agent，匯入完整匯出檔的全部 Agent 與日期
./target/release/token-usage-insights import --file all-usage.json
```

`export-all` 不需指定 Agent 或日期；省略 `--out` 時輸出 JSON 到 stdout。匯出涵蓋 SQLite 已收錄的所有使用量記錄，包含原生與匯入資料，保留既有匯出格式的完整欄位及 `import_source_id`。執行前請透過看板完成來源日誌同步；此命令不會自行掃描來源日誌，也不是包含原始對話檔、設定與匯入批次歷程的資料庫備份。

完整匯出檔包含 `version`、`exported_at` 與 `exports` 陣列；每個元素都是原有格式的單一 Agent、單日匯出，依 Agent 與日期排序。空資料庫會輸出空的 `exports` 陣列。`import --file` 自動依檔案中的 `assistant` 匯入所有 Agent；單一 Agent 檔案也可自動辨識。只有需要篩選特定 Agent，或舊檔缺少 `assistant` 時，才需要指定 `--agent`。缺少或不支援的 Agent 會在寫入前回報錯誤。

各 Agent 的全部日期合併為一個匯入批次，輸出 JSON 陣列列出各 Agent 與匯入結果。若中途資料庫寫入失敗，先前成功的 Agent 批次會保留；重新匯入會自動去重。

```bash
# 匯入檔案中的所有資料；每筆資料依 timestamp 決定日期
./target/release/token-usage-insights import --file monthly-codex-2026-07.json
```

```bash
# 取得 CLI usage 說明
./target/release/token-usage-insights --help
./target/release/token-usage-insights update --help
./target/release/token-usage-insights export --help
./target/release/token-usage-insights export-all --help
./target/release/token-usage-insights import --help

# 自我更新至最新版本（亦支援 --check 僅檢查、--force 強制覆蓋、--target-version 指定版本）
./target/release/token-usage-insights update
./target/release/token-usage-insights update --check
./target/release/token-usage-insights update --target-version v0.9.6
```

資料格式使用和前端一致，內含欄位：

- `version`
- `assistant`
- `date`
- `exported_at`
- `records`（每筆會有 `import_source_id`）

`import_source_id` 會與 `assistant_type` 一起做唯一鍵，重複匯入同一筆會被判為重複並自動跳過，不會重複寫入資料庫。

* * *

## 環境變數

環境變數指定的路徑會被視為權威設定，不必預先建立；`INSIGHTS_DIR` 會在啟動時自動建立。支援原生絕對/相對路徑，以及開頭為 `~`、`$HOME`、`%USERPROFILE%`、`%LOCALAPPDATA%` 或 `%APPDATA%` 的常見寫法。

| 變數 | 預設值 | 用途 |
| --- | --- | --- |
| `HOST` | `0.0.0.0` | 看板服務綁定的 IPv4 或 IPv6 位址 |
| `PORT` | `3003` | 看板服務埠號 |
| `INSIGHTS_DIR` | Windows: `%LOCALAPPDATA%\TokenUsageInsights`; 其他平台: `~/.token-usage-insights` | SQLite 資料庫目錄 |
| `TOKEN_USAGE_INSIGHTS_AUTO_UPDATE` | `true` | 是否在啟動時自動檢查更新（設為 `0`、`false`、`no` 或 `off` 可停用） |
| `TOKEN_USAGE_INSIGHTS_UPDATE_INTERVAL_HOURS` | `24` | 自動檢查更新的間隔週期（小時，有效範圍 1 至 87600） |
| `TOKEN_USAGE_INSIGHTS_INSTALL_DIR` | 自動偵測 | 自訂安裝目錄，作為更新目標與環境辨識依據 |
| `ANTIGRAVITY_DIR` | `~/.gemini/antigravity-cli` | Antigravity CLI 資料目錄 |
| `COPILOT_DIR` | `~/.copilot` | Copilot CLI 資料目錄 |
| `COPILOT_APP_DIR` | 同 `COPILOT_DIR` | Copilot App（桌面應用）資料目錄，應包含 `data.db` 與 `session-store.db` |
| `VSCODE_USER_DATA_DIR` | 依平台自動偵測 | VS Code 使用者資料目錄，應包含 `User/workspaceStorage` |
| `VSCODE_PORTABLE_DATA_DIR` | 未設定 | VS Code Portable Mode 的 `data` 目錄 |
| `CODEX_DIR` | `~/.codex` | Codex Desktop 與 Codex CLI 共用資料目錄 |
| `CLAUDE_DIR` | `~/.claude` | Claude Code 資料目錄 |
| `CURSOR_DIR` | `~/.cursor` | Cursor 資料目錄 |
| `CURSOR_STATE_DB` | 依平台自動偵測 | Cursor `User/globalStorage/state.vscdb` 路徑，用於唯讀取得 `agentKv` 模型資訊 |
| `GROK_DIR` | `~/.grok` | Grok Build 資料目錄 |
| `PI_DIR` | `~/.pi` | Pi Coding Agent 資料目錄 |
| `OMP_DIR` | `~/.omp` | OMP 資料目錄 |
| `MUSE_DIR` | `~/.local/share/muse` | Muse Code 資料目錄，應包含 `sessions` |
| `CORS_ALLOWED_ORIGINS` | `http://localhost:<PORT>,http://127.0.0.1:<PORT>` | 允許的 CORS 來源，逗號分隔 |

### 設定檔 (config.yaml)

除了環境變數與命令列旗標（如 `--no-auto-update`）之外，亦可在 `~/.token-usage-insights/config.yaml`（Windows 為 `%LOCALAPPDATA%\TokenUsageInsights\config.yaml`）中設定更新行為：

```yaml
# ~/.token-usage-insights/config.yaml
auto_update: true          # 是否在服務啟動時自動檢查並更新（可透過 --no-auto-update 或環境變數覆寫）
update_check_interval: 1   # 自動檢查更新的間隔週期（天）
```

優先順序：命令列旗標（如 `--no-auto-update`） > 環境變數（如 `TOKEN_USAGE_INSIGHTS_AUTO_UPDATE`） > `config.yaml` 設定檔 > 預設值。

> **預設綁定 `0.0.0.0`，同一區網內的其他裝置可能連線到看板。只需在本機瀏覽時，請將 `HOST` 設為 `127.0.0.1`。**

範例：

```bash
HOST="127.0.0.1" INSIGHTS_DIR="/tmp/token-usage-insights" PORT="3010" "$HOME/.local/bin/token-usage-insights"
```

Windows PowerShell 範例：

```powershell
$env:HOST = '127.0.0.1'; $env:INSIGHTS_DIR = 'D:\Token Usage Insights\資料庫'; $env:CODEX_DIR = "$env:USERPROFILE\.codex"; $env:PORT = '3010'; & "$HOME\bin\token-usage-insights.cmd"
```

* * *

## 常駐服務

### Linux：一行安裝並啟用 systemd 使用者服務

```bash
curl -fsSL https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.sh | bash -s -- --service
```

這會下載安裝版並立即啟用 `token-usage-insights.service`，不需要自行建置或修改 systemd 檔案。

### macOS：一行安裝並啟用 launchd LaunchAgent

```bash
curl -fsSL https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.sh | bash -s -- --service
```

這會將 `com.tokenusageinsights.plist` 安裝到 `~/Library/LaunchAgents/` 並立即載入；標準輸出與錯誤日誌位於 `~/Library/Logs/`。

### Windows：一行安裝並啟用背景常駐服務（工作排程器）

```powershell
& ([scriptblock]::Create((irm https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.ps1))) -Service
```

這會透過 Windows 工作排程器（Task Scheduler）註冊專屬於目前使用者的 `TokenUsageInsights_<username>` 背景工作並立即啟動；使用者每次登入時均會自動於背景執行，標準輸出與錯誤日誌位於安裝目錄下的 `logs\`（預設為 `%LOCALAPPDATA%\TokenUsageInsights\logs\`）。

### 管理服務

Linux 可使用：

```bash
systemctl --user status token-usage-insights.service
journalctl --user -u token-usage-insights.service -n 50 -f
systemctl --user restart token-usage-insights.service
systemctl --user stop token-usage-insights.service
```

macOS 可使用：

```bash
launchctl print gui/$(id -u)/com.tokenusageinsights
launchctl kickstart -k gui/$(id -u)/com.tokenusageinsights
launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.tokenusageinsights.plist
```

Windows PowerShell 可使用：

```powershell
# 解析安裝目錄（預設為 %LOCALAPPDATA%\TokenUsageInsights，或由已註冊排程/捷徑動態解析）
$TaskName = if ($env:USERNAME) { "TokenUsageInsights_$env:USERNAME" } else { "TokenUsageInsights" }
$InstallDir = $null
$Task = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
if (-not $Task) {
    $Task = Get-ScheduledTask -TaskName "TokenUsageInsights" -ErrorAction SilentlyContinue
    if ($Task) {
        $TaskName = "TokenUsageInsights"
    }
}
if ($Task -and $Task.Actions) {
    foreach ($Action in @($Task.Actions)) {
        if ($Action.Arguments -match '(?i)-InstallDir(?:\s+|:)(?:"([^"]+)"|(\S+))') {
            $DetectedInstallDir = if ($Matches[1]) { $Matches[1] } else { $Matches[2] }
            $InstallDir = [Environment]::ExpandEnvironmentVariables($DetectedInstallDir)
            break
        } elseif ($Action.WorkingDirectory) {
            $InstallDir = $Action.WorkingDirectory
            break
        }
    }
}
$StartupShortcut = Join-Path ([Environment]::GetFolderPath('Startup')) "token-usage-insights.lnk"
if (!(Test-Path $StartupShortcut)) {
    $StartupShortcut = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Startup\token-usage-insights.lnk"
}
if (-not $InstallDir -and (Test-Path $StartupShortcut)) {
    $WshShell = New-Object -ComObject WScript.Shell
    $Shortcut = $WshShell.CreateShortcut($StartupShortcut)
    if ($Shortcut.Arguments -match '(?i)-InstallDir(?:\s+|:)(?:"([^"]+)"|(\S+))') {
        $DetectedInstallDir = if ($Matches[1]) { $Matches[1] } else { $Matches[2] }
        $InstallDir = [Environment]::ExpandEnvironmentVariables($DetectedInstallDir)
    } elseif ($Shortcut.WorkingDirectory) {
        $InstallDir = $Shortcut.WorkingDirectory
    }
}
if (-not $InstallDir) {
    $InstallDir = Join-Path $env:LOCALAPPDATA "TokenUsageInsights"
}
$TargetExe = "$InstallDir\token-usage-insights.exe".ToLowerInvariant().Replace('/', '\')
$EscapedDir = [regex]::Escape($InstallDir)

# 檢視服務狀態（工作排程器或背景行程）
Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object {
    ($_.CommandLine -like "*run-service.ps1*" -and $_.CommandLine -match "(?i)[\s`"'\\]$EscapedDir([\\`"'\s]|$)") -or
    ($_.ExecutablePath -and ($_.ExecutablePath.ToLowerInvariant().Replace('/', '\') -eq $TargetExe))
} | Select-Object ProcessId, Name, CommandLine

# 檢視即時日誌
Get-Content (Join-Path $InstallDir "logs\token-usage-insights.out.log") -Tail 50 -Wait

# 重啟服務（僅限此安裝目錄，自動相容工作排程器與啟動資料夾模式）
Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object {
    ($_.CommandLine -like "*run-service.ps1*" -and $_.CommandLine -match "(?i)[\s`"'\\]$EscapedDir([\\`"'\s]|$)") -or
    ($_.ExecutablePath -and ($_.ExecutablePath.ToLowerInvariant().Replace('/', '\') -eq $TargetExe))
} | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
if (Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue) {
    Start-ScheduledTask -TaskName $TaskName
} elseif (Test-Path $StartupShortcut) {
    Start-Process $StartupShortcut
}

# 停止服務（僅限此安裝目錄）
Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object {
    ($_.CommandLine -like "*run-service.ps1*" -and $_.CommandLine -match "(?i)[\s`"'\\]$EscapedDir([\\`"'\s]|$)") -or
    ($_.ExecutablePath -and ($_.ExecutablePath.ToLowerInvariant().Replace('/', '\') -eq $TargetExe))
} | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }

# 解除安裝常駐服務
Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
Unregister-ScheduledTask -TaskName "TokenUsageInsights" -Confirm:$false -ErrorAction SilentlyContinue
if (Test-Path $StartupShortcut) {
    Remove-Item $StartupShortcut -Force -ErrorAction SilentlyContinue
}
Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object {
    ($_.CommandLine -like "*run-service.ps1*" -and $_.CommandLine -match "(?i)[\s`"'\\]$EscapedDir([\\`"'\s]|$)") -or
    ($_.ExecutablePath -and ($_.ExecutablePath.ToLowerInvariant().Replace('/', '\') -eq $TargetExe))
} | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
```

* * *

## 安裝選項與手動安裝

GitHub Release 提供 Linux、macOS 與 Windows 的已編譯可執行檔，安裝與執行都不需要 Rust 或 Cargo。

### 使用 npx 直接執行

電腦已有 Node.js 18.18 或更新版本時，執行以下命令即可下載目前版本並啟動看板：

```bash
npx --yes token-usage-insights
```

`npx` 不會建立全域命令；每次都可使用相同命令啟動。若需要固定的 `token-usage-insights` 系統命令、自訂安裝目錄，或安裝 Linux systemd 服務，請改用下一節的安裝腳本。

### 一行安裝的選用參數

`scripts/get.sh`（Linux / macOS）與 `scripts/get.ps1`（Windows）會自動判斷平台與 CPU 架構、從最新（或指定）Release 下載對應壓縮包、解壓後呼叫套件內的 `install.sh` / `install.ps1`，全程不需要手動下載或解壓：

Linux / macOS：

```bash
curl -fsSL https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.sh | bash
```

Linux（systemd user service）或 macOS（launchd LaunchAgent）如需同時安裝並啟用常駐服務：

```bash
curl -fsSL https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.sh | bash -s -- --service
```

Windows PowerShell：

```powershell
irm https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.ps1 | iex
```

Windows PowerShell 如需同時安裝並啟用常駐服務：

```powershell
& ([scriptblock]::Create((irm https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.ps1))) -Service
```

安裝完成後即可執行（Linux/macOS 需確認 `bin_dir` 已加入 `PATH`；Windows 會建立 `.cmd` shim）：

```bash
token-usage-insights
```

環境變數可控制版本與安裝路徑（皆為選用）：

| 變數 | 適用平台 | 說明 |
| --- | --- | --- |
| `TOKEN_USAGE_INSIGHTS_VERSION` | Linux / macOS / Windows | 指定要安裝的 Release tag，例如 `v0.9.5`。預設 `latest` |
| `TOKEN_USAGE_INSIGHTS_INSTALL_DIR` | Linux / macOS | 安裝目錄，會轉交給 `install.sh` |
| `TOKEN_USAGE_INSIGHTS_BIN_DIR` | Linux / macOS | 執行檔連結目錄，會轉交給 `install.sh` |

Windows 若要自訂安裝位置、bin 目錄與埠號，需先下載腳本再帶參數執行（`iex` 管線不支援傳參數）：

```powershell
Invoke-WebRequest -Uri https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.ps1 -OutFile get.ps1
.\get.ps1 -InstallDir 'D:\Apps\Token Usage Insights' -Port 3010
```

### 手動下載安裝

若不想直接執行遠端腳本，也可以手動下載對應平台壓縮包並執行套件內建的安裝腳本。每個 Release 壓縮包都包含：

- 單一平台可執行檔
- `static/` 前端資產
- `pricing.csv` 模型費用表
- `shell/` 目錄下的 Status Line 與服務腳本
- `scripts/` 目錄（含 `install.sh`、`install.ps1`、`get.sh`、`get.ps1`、`run-service.ps1`）
- README、LICENSE 與 VERSION

Linux 或 macOS：

```bash
tar -xzf token-usage-insights-<tag>-<target>.tar.gz
cd token-usage-insights-<tag>-<target>
./install.sh
```

Linux（systemd user service）或 macOS（launchd LaunchAgent）如需安裝並啟用常駐服務：

```bash
./install.sh --service
```

Windows：

```powershell
Expand-Archive token-usage-insights-<tag>-x86_64-pc-windows-msvc.zip
cd token-usage-insights-<tag>-x86_64-pc-windows-msvc
powershell -ExecutionPolicy Bypass -File .\install.ps1
```

Windows 如需安裝並啟用背景常駐服務：

```powershell
powershell -ExecutionPolicy Bypass -File .\install.ps1 -Service
```

自訂 Windows 安裝位置與埠號：

```powershell
.\install.ps1 -InstallDir 'D:\Apps\Token Usage Insights' -BinDir "$HOME\bin" -Port 3010
```

### CI 驗證

`Release` workflow 每次建置都會在 Linux、macOS 與 Windows 上實際執行對應的安裝腳本（`install.sh` / `install.ps1`），安裝後啟動可執行檔並確認：

- 服務會在指定埠號回應 `/api/<assistant>/pricing`
- 回應內容確實載入了套件內的 `pricing.csv`
- 全新的 `INSIGHTS_DIR` 會被建立並產生 SQLite 資料庫

`get.sh` 與 `get.ps1` 也會在每次建置時先做語法檢查（`bash -n` 與 PowerShell AST 剖析），確保推送到 Release 的版本可以正常執行。

### 維護者發行

推送 Git tag 後，GitHub Actions 會自動建立對應 Release：

```bash
git tag vX.Y.Z
git push origin vX.Y.Z
```

npm 第一次上架、Trusted Publishing 的必要欄位、GitHub Environment、Repository variable 與後續 OIDC 自動發布流程，請依照 [npm 首次上架與 Trusted Publishing 設定](docs/npm-publishing.md) 操作。

* * *

## 舊資料遷移

若你以前使用過下列獨立專案，啟動本專案時會自動嘗試遷移舊 SQLite 資料：

- `~/.gemini/antigravity-cli/antigravity_cli_token_insights.db`
- `~/.copilot/copilot_cli_token_insights.db`
- `~/.codex/codex_cli_token_insights.db`

遷移成功後，舊資料庫會被改名為 `.bak`。

若你已確認資料遷移完成，可以停用舊服務：

```bash
systemctl --user stop copilot-cli-token-insights.service
systemctl --user disable copilot-cli-token-insights.service
systemctl --user stop antigravity-cli-token-insights.service
systemctl --user disable antigravity-cli-token-insights.service
systemctl --user stop codex-cli-token-insights.service
systemctl --user disable codex-cli-token-insights.service

rm -f ~/.config/systemd/user/copilot-cli-token-insights.service
rm -f ~/.config/systemd/user/antigravity-cli-token-insights.service
rm -f ~/.config/systemd/user/codex-cli-token-insights.service

systemctl --user daemon-reload
systemctl --user reset-failed
```

* * *

## 疑難排查

### 看板沒有資料

依工具檢查資料來源是否存在：

```bash
ls ~/.gemini/antigravity-cli/usage
ls ~/.copilot/usage
ls ~/.copilot/data.db ~/.copilot/session-store.db
ls ~/.codex/sessions
ls ~/.codex/archived_sessions
ls ~/.claude/projects
ls ~/.cursor/projects
ls ~/.grok/sessions
ls ~/.pi/agent/sessions
ls ~/.omp/agent/sessions
ls ~/.local/share/muse/sessions
```

Antigravity CLI 與 Copilot CLI 還需要確認 `settings.json` 已設定 `statusLine`，且腳本具備執行權限。

Windows PowerShell 可直接檢查原生資料目錄：

```powershell
Get-ChildItem "$env:USERPROFILE\.gemini\antigravity-cli\usage"
Get-ChildItem "$env:USERPROFILE\.copilot\usage"
Get-ChildItem "$env:USERPROFILE\.copilot\data.db", "$env:USERPROFILE\.copilot\session-store.db"
Get-ChildItem "$env:USERPROFILE\.codex\sessions"
Get-ChildItem "$env:USERPROFILE\.codex\archived_sessions"
Get-ChildItem "$env:USERPROFILE\.claude\projects"
Get-ChildItem "$env:USERPROFILE\.cursor\projects"
Get-ChildItem "$env:USERPROFILE\.grok\sessions"
Get-ChildItem "$env:USERPROFILE\.pi\agent\sessions"
Get-ChildItem "$env:USERPROFILE\.omp\agent\sessions"
Get-ChildItem "$env:USERPROFILE\.local\share\muse\sessions"
```

### Status Line 腳本無法執行

```bash
command -v jq
chmod +x ~/.gemini/antigravity-cli/statusline-token.sh
chmod +x ~/.copilot/statusline-token.sh
```

Status Line 腳本依賴 `jq` 解析 CLI 傳入的 JSON。

上述 `jq` 需求只適用於 `.sh` collector。Windows `.ps1` collector 可用下列命令測試，並會原生處理反斜線與含空白路徑：

```powershell
Write-Output '{}' | powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$env:USERPROFILE\.gemini\antigravity-cli\statusline-token.ps1" -Assistant antigravity
```

### 設定檔 JSON 格式錯誤

```bash
jq . ~/.gemini/antigravity-cli/settings.json
jq . ~/.copilot/settings.json
```

若已經有其他設定，請合併 `statusLine` 物件，不要把整個檔案替換成陣列或純字串。

### 連不上 `localhost:3003`

```bash
PORT=3010 "$HOME/.local/bin/token-usage-insights"
```

若改用其他埠號，請開啟對應網址，例如：

```text
http://localhost:3010
```

* * *

## 開發指令

本節僅供需要修改或從原始碼建置專案的開發者使用；一般使用請採用前述一行安裝指令。

```bash
git clone https://github.com/doggy8088/TokenUsageInsights.git
cd TokenUsageInsights
cargo fmt
cargo test
cargo clippy --all-targets --all-features
cargo build --release
./target/release/token-usage-insights
```

* * *

## 專案檔案

```text
src/                 Rust 後端、API、SQLite 同步、價格與時間軸解析
static/              前端 HTML、JavaScript、CSS 與圖片資產
shell/               Bash/PowerShell Status Line collector 與 systemd 服務範本
scripts/             Linux/macOS、Windows 安裝與 Windows smoke test
pricing.csv          模型價格表，本地估算費用依此檔案載入
```

* * *

## 畫面展示

![Token 戰情室每日看板](screenshots/codex-daily-2026-07-07-desktop-chrome.png)

![Token 戰情室月度看板](screenshots/codex-daily-2026-07-07.png)

![Token 戰情室 Session 時間軸](screenshots/codex-daily-2026-07-07-desktop-chrome.png)
