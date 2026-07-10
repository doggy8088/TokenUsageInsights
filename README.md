# Token 戰情室

**Token 戰情室是本地優先的 AI CLI Token 使用量與會話還原看板。** 它會讀取本機上的 Google Antigravity CLI、GitHub Copilot CLI、Codex CLI 與 Claude Code 記錄，集中呈現每日、月度、年度的 Token 消耗、快取使用、推理 Token、估算費用、模型分佈、專案目錄分佈與完整 Session 時間軸。

本專案不會替你呼叫 AI 供應商 API 查詢資料；核心資料來源是本機日誌、Status Line 收集檔與本機 SQLite。

> 系統環境：支援 Windows 10/11 原生 PowerShell、macOS、Linux 與 WSL。

* * *

## 最短上手路徑

### 1. 啟動看板

```bash
git clone https://github.com/doggy8088/TokenUsageInsights.git
cd TokenUsageInsights
cargo run
```

開啟：

```text
http://localhost:3003
```

### 2. 依你使用的 CLI 決定是否需要設定

| CLI | 是否需要額外設定 | 預設資料來源 | 說明 |
| --- | --- | --- | --- |
| Google Antigravity CLI | 需要 | `~/.gemini/antigravity-cli/usage/usage-YYYY-MM-DD.jsonl` | 透過 `statusline-token.sh` 或 Windows `statusline-token.ps1` 收集 Token 資料 |
| GitHub Copilot CLI | 需要 | `~/.copilot/usage/usage-YYYY-MM-DD.jsonl` | 透過 `statusline-token.sh` 或 Windows `statusline-token.ps1` 收集 Token 資料 |
| Codex CLI | 不需要 | `~/.codex/sessions` | 看板會直接掃描 Codex CLI 本機 Session 記錄 |
| Claude Code | 不需要 | `~/.claude/projects` | 看板會直接掃描 Claude Code 本機專案 Session 記錄 |

**只使用 Codex CLI 或 Claude Code 時，通常只要 `cargo run` 後打開看板即可。**

### Windows 原生執行

Windows 原生建置需要 Rust MSVC toolchain 與 Visual Studio Build Tools 的 C++ workload，不需要 WSL、Git Bash 或 `jq`：

```powershell
git clone https://github.com/doggy8088/TokenUsageInsights.git
Set-Location TokenUsageInsights
cargo run
Start-Process 'http://localhost:3003'
```

Windows 預設使用下列原生路徑：

| 用途 | Windows 預設路徑 |
| --- | --- |
| SQLite | `%LOCALAPPDATA%\TokenUsageInsights\token_usage_insights.db` |
| Antigravity | `%USERPROFILE%\.gemini\antigravity-cli` |
| Copilot | `%USERPROFILE%\.copilot` |
| Codex | `%USERPROFILE%\.codex` |
| Claude Code | `%USERPROFILE%\.claude` |
| Cursor | `%USERPROFILE%\.cursor` |

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
- 專案工作目錄統計
- 可排序的 Session 清單

### Session 還原

- 右側抽屜式 Session 時間軸
- 使用者提示詞、助理回覆、推理內容與工具呼叫步驟
- 工具呼叫參數、退出碼、stdout、stderr
- Codex subagent 相關欄位，如 parent session、agent nickname、agent role
- Markdown 回覆渲染與內容清理

### 介面操作

- 四種 CLI 徽章切換
- 每日、月度、年度視圖
- 日期、月份、年份快速切換
- 5 秒、10 秒、30 秒即時自動刷新
- 手動同步本機日誌到 SQLite
- 深色與淺色主題
- 繁中與英文介面切換
- 模型費用表檢視

* * *

## Google Antigravity CLI 設定

Antigravity CLI 需要把本專案的 Status Line 腳本接到 `settings.json`。腳本會把每次對話後的 Token 累計與增量寫入：

```text
~/.gemini/antigravity-cli/usage/usage-YYYY-MM-DD.jsonl
```

### 1. 安裝收集腳本

```bash
mkdir -p ~/.gemini/antigravity-cli
cp shell/antigravity/statusline-token.sh ~/.gemini/antigravity-cli/statusline-token.sh
chmod +x ~/.gemini/antigravity-cli/statusline-token.sh
```

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

```bash
mkdir -p ~/.copilot
cp shell/copilot/statusline-token.sh ~/.copilot/statusline-token.sh
chmod +x ~/.copilot/statusline-token.sh
```

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

## Codex CLI 設定

**Codex CLI 不需要安裝 Hook、Status Line 或額外收集腳本。**

看板會直接掃描：

```text
~/.codex/sessions
```

使用方式：

1. 先正常使用 Codex CLI 產生至少一個 Session。
2. 啟動本專案。
3. 在左側選擇 Codex CLI。
4. 按右上角同步按鈕，或等待背景同步。

注意事項：

- Codex CLI 的身份憑證仍由 Codex CLI 自身管理。
- 看板只讀取本地 Session 記錄並做分析。
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

* * *

## 環境變數

環境變數指定的路徑會被視為權威設定，不必預先建立；`INSIGHTS_DIR` 會在啟動時自動建立。支援原生絕對/相對路徑，以及開頭為 `~`、`$HOME`、`%USERPROFILE%`、`%LOCALAPPDATA%` 或 `%APPDATA%` 的常見寫法。

| 變數 | 預設值 | 用途 |
| --- | --- | --- |
| `PORT` | `3003` | 看板服務埠號 |
| `INSIGHTS_DIR` | Windows: `%LOCALAPPDATA%\TokenUsageInsights`; 其他平台: `~/.token-usage-insights` | SQLite 資料庫目錄 |
| `ANTIGRAVITY_DIR` | `~/.gemini/antigravity-cli` | Antigravity CLI 資料目錄 |
| `COPILOT_DIR` | `~/.copilot` | Copilot CLI 資料目錄 |
| `CODEX_DIR` | `~/.codex` | Codex CLI 資料目錄 |
| `CLAUDE_DIR` | `~/.claude` | Claude Code 資料目錄 |
| `CURSOR_DIR` | `~/.cursor` | Cursor 資料目錄 |
| `CORS_ALLOWED_ORIGINS` | `http://localhost:<PORT>,http://127.0.0.1:<PORT>` | 允許的 CORS 來源，逗號分隔 |

範例：

```bash
mkdir -p /tmp/token-usage-insights
export INSIGHTS_DIR="/tmp/token-usage-insights"
export PORT="3010"
cargo run
```

Windows PowerShell 範例：

```powershell
$env:INSIGHTS_DIR = 'D:\Token Usage Insights\資料庫'
$env:CODEX_DIR = "$env:USERPROFILE\.codex"
$env:PORT = '3010'
cargo run
```

* * *

## 常駐服務

### 1. 建置 release 版本

```bash
cargo build --release
```

### 2. 安裝 systemd 使用者服務

```bash
mkdir -p ~/.config/systemd/user/
sed "s|<PROJECT_DIR>|$PWD|g" shell/token-usage-insights.service > ~/.config/systemd/user/token-usage-insights.service
systemctl --user daemon-reload
systemctl --user enable token-usage-insights.service
systemctl --user start token-usage-insights.service
```

### 3. 管理服務

```bash
systemctl --user status token-usage-insights.service
journalctl --user -u token-usage-insights.service -n 50 -f
systemctl --user restart token-usage-insights.service
systemctl --user stop token-usage-insights.service
```

### Windows release 啟動

`install.ps1` 預設安裝至 `%LOCALAPPDATA%\TokenUsageInsights`，並在 `%USERPROFILE%\bin` 建立 `.cmd` shim。安裝路徑可包含空白與非 ASCII 字元；shim 會切換至安裝目錄後啟動：

```powershell
powershell -ExecutionPolicy Bypass -File .\install.ps1
& "$HOME\bin\token-usage-insights.cmd"
```

* * *

## GitHub Release 下載安裝

建立任意 Git tag 並推送後，GitHub Actions 會自動建立 Release，並產出 Linux、macOS 與 Windows 的平台壓縮包。

```bash
git tag v0.1.2
git push origin v0.1.2
```

每個 Release 壓縮包都包含：

- 單一平台可執行檔
- `static/` 前端資產
- `pricing.csv` 模型費用表
- `shell/` 目錄下的 Status Line 與服務腳本
- `install.sh` 與 `install.ps1` 安裝腳本
- README、LICENSE 與 VERSION

Linux 或 macOS：

```bash
tar -xzf token-usage-insights-<tag>-<target>.tar.gz
cd token-usage-insights-<tag>-<target>
./install.sh
```

Linux 如需安裝並啟用 systemd user service：

```bash
./install.sh --service
```

Windows：

```powershell
Expand-Archive token-usage-insights-<tag>-x86_64-pc-windows-msvc.zip
cd token-usage-insights-<tag>-x86_64-pc-windows-msvc
powershell -ExecutionPolicy Bypass -File .\install.ps1
```

自訂 Windows 安裝位置與埠號：

```powershell
.\install.ps1 -InstallDir 'D:\Apps\Token Usage Insights' -BinDir "$HOME\bin" -Port 3010
```

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

依 CLI 檢查資料來源是否存在：

```bash
ls ~/.gemini/antigravity-cli/usage
ls ~/.copilot/usage
ls ~/.codex/sessions
ls ~/.claude/projects
```

Antigravity CLI 與 Copilot CLI 還需要確認 `settings.json` 已設定 `statusLine`，且腳本具備執行權限。

Windows PowerShell 可直接檢查原生資料目錄：

```powershell
Get-ChildItem "$env:USERPROFILE\.gemini\antigravity-cli\usage"
Get-ChildItem "$env:USERPROFILE\.copilot\usage"
Get-ChildItem "$env:USERPROFILE\.codex\sessions"
Get-ChildItem "$env:USERPROFILE\.claude\projects"
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
PORT=3010 cargo run
```

若改用其他埠號，請開啟對應網址，例如：

```text
http://localhost:3010
```

* * *

## 開發指令

```bash
cargo fmt
cargo test
cargo clippy --all-targets --all-features
cargo build --release
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
