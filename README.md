# 🌟 Unified AI CLI Token Usage Insights Dashboard

[English Version (英文版)](./README.en.md)

這是一個專為本地終端 AI 助理（**Google Antigravity CLI**、**GitHub Copilot CLI** 與 **Codex CLI**）設計的本地 Token 消耗與會話分析綜合看板。使用高效能的 **Rust (Axum)** 作為後端，搭配 **深色毛玻璃風格 (Glassmorphism)** 前端，協助您在一處集中查看所有本地 AI 助理的 Token 使用量、命中率、對話耗時以及**還原與重建每個會話的完整歷史對話時間軸**！

---

## 🤖 給 Agent / 自動化流程的安裝提示
如果您想讓另一個 AI Agent 或自動化工具快速完成安裝，請直接貼上以下更完整的提示詞：

```text
請在這個環境中安裝並啟動 Unified AI CLI Token Usage Insights Dashboard，並以本地流程完成設定。

工作目標：
- 安裝並啟動這個 Rust/Axum 專案。
- 配置 Google Antigravity CLI 與 GitHub Copilot CLI 的 statusLine 收集腳本，以便在本地自動收集 Token 使用數據。
- 確認看板可在 http://localhost:3003 正常開啟。

請依序完成以下步驟：
1. 先確認目前工作目錄是這個專案的根目錄，並確認以下檔案存在：
   - shell/antigravity/statusline-token.sh
   - shell/copilot/statusline-token.sh
2. 確認 Rust 工具鏈已安裝；若沒有，請使用 rustup 安裝。
3. 執行 cargo build --release，建立 Release 版本二進位檔。
4. 設定 Google Antigravity CLI：
   - 建立或確認 ~/.gemini/antigravity-cli 目錄存在。
   - 將 shell/antigravity/statusline-token.sh 複製到 ~/.gemini/antigravity-cli/statusline-token.sh。
   - 賦予腳本可執行權限：chmod +x ~/.gemini/antigravity-cli/statusline-token.sh。
   - 更新 ~/.gemini/antigravity-cli/settings.json：
     - 若檔案不存在，建立一個新的 JSON 檔案，內容必須是合法的 JSON 物件，並加入 statusLine 設定。
     - 若檔案已存在，請保留原有設定，並將 statusLine 區塊合併進去；請務必維持原本的 JSON 結構，不要把現有設定改成陣列或其他非物件格式。
     - statusLine 的 command 應指向 ~/.gemini/antigravity-cli/statusline-token.sh，請使用實際的家目錄路徑（可由 echo $HOME 取得）。
     - statusLine 範例如下：
       {
         "statusLine": {
           "type": "command",
           "command": "$HOME/.gemini/antigravity-cli/statusline-token.sh",
           "padding": 1
         }
       }
5. 設定 GitHub Copilot CLI：
   - 建立或確認 ~/.copilot 目錄存在。
   - 將 shell/copilot/statusline-token.sh 複製到 ~/.copilot/statusline-token.sh。
   - 賦予腳本可執行權限：chmod +x ~/.copilot/statusline-token.sh。
   - 更新 ~/.copilot/settings.json：
     - 若檔案不存在，建立一個新的 JSON 檔案，內容必須是合法的 JSON 物件，並加入 statusLine 設定。
     - 若檔案已存在，請保留原有設定，並將 statusLine 區塊合併進去；請務必維持原本的 JSON 結構。
     - statusLine 的 command 應指向 ~/.copilot/statusline-token.sh，請使用實際的家目錄路徑（可由 echo $HOME 取得）。
     - statusLine 範例如下：
       {
         "statusLine": {
           "type": "command",
           "command": "$HOME/.copilot/statusline-token.sh",
           "padding": 1
         }
       }
6. 驗證腳本與設定是否有效：
   - 測試執行：echo '{}' | ~/.gemini/antigravity-cli/statusline-token.sh，確認腳本可正常執行。
   - 測試執行：echo '{}' | ~/.copilot/statusline-token.sh，確認腳本可正常執行。
   - 確認 JSON 格式正確：可以使用 jq 或 python -m json.tool 檢查 ~/.gemini/antigravity-cli/settings.json 與 ~/.copilot/settings.json。
7. 回到專案根目錄，執行 cargo run 啟動看板服務。
8. 確認終端輸出包含 http://localhost:3003，並在瀏覽器確認該網址可存取。

注意事項：
- Codex CLI 不需要收集腳本或 statusLine 配置，系統會自動遞迴掃描您本地的 ~/.codex/sessions 目錄。
- 這個專案以本地資料為主，請不要假設會使用遠端 API 或外部服務。
- 若有任何步驟失敗，請停止並回報具體錯誤訊息與你執行的指令。
- 完成後請提供簡短摘要，包含是否成功啟動、網址是否可訪問，以及任何需要注意的事項。
```

---

## 🌟 核心特色 (Key Features)

本看板為多個終端 AI 助理提供統一的本地數據監控與可視化，核心特色如下：

1. **📊 統一即時分析看板 (Consolidated Real-time Dashboard)**
   * **一站式彙整**：在一處看板同時監控 Google Antigravity CLI、GitHub Copilot CLI 與 Codex CLI 的 Token 總消耗、輸入/輸出 Token 佔比、快取讀取 Token 以及推理 Token 使用量。
   * **圖表與趨勢**：以 Chart.js 繪製平滑曲線，展示每日各會話的 Token 消耗波動、快取命中率與對話 Turn 數。
   * **🔴 即時更新機制 (Live Monitor)**：支援一鍵開啟自動刷新（可自訂 5s、10s 或 30s 頻率），與您的終端機對話保持即時同步，並有倒數計時進度條。

2. **📅 月度數據彙整與工具費用對比 (Monthly Aggregation & Cost Comparison)**
   * **月度趨勢**：折線圖展示當月每日的 Token 使用總量與對話會話數。
   * **最常活動的專案目錄**：統計不同專案目錄（CWD）下的會話次數與 Token 消耗，追蹤 AI 在各專案中的使用比率。
   * **模型佔比分析**：分析當月使用的不同 LLM（如 Gemini, GPT-4o, Claude 等）的會話與 Token 分佈。

3. **🔍 互動式會話歷史清單 (Interactive Session History)**
   * 提供完整會話列表，支援會話 ID/名稱、模型、Turn 數、Token 消耗及 API 耗時（毫秒）等欄位的即時升降冪排序，幫助快速篩選高消耗會話。

4. **⏱️ 三大助理時間軸還原 (Session Timeline Drawer)**
   * 點擊會話，右側滑出詳細的歷史對話時間軸。
   * **對話重建**：
     * **使用者提示詞 (User Prompt)**：對話泡泡與附加的 Context 狀態。
     * **推理與回覆 (Agent Reply)**：呈現 LLM 的思維過程與帶有代碼高亮排版的 Markdown 內容。
     * **工具呼叫步驟 (CLI Tool Step)**：展開 CLI 本地工具呼叫細節（引數、Exit Code、Stdout、Stderr），還原 AI 本地操作路徑。

---

## 🔄 舊專案歷史資料自動遷移

如果您先前曾安裝並使用過以下獨立的 Token 統計專案：
* [copilot-cli-token-insights](https://github.com/wengct/copilot-cli-token-insights)
* [antigravity-cli-token-insights](https://github.com/wengct/antigravity-cli-token-insights)
* [codex-cli-token-insights](https://github.com/wengct/codex-cli-token-insights)

當您啟動本專案時，後端服務會**自動偵測並將舊有的歷史數據遷移合併**至本專案的統一資料庫中，並安全備份原有的舊資料庫檔案，確保您的歷史統計數據無縫接軌不遺失。

### 🗑️ 舊服務的停用與刪除方法

由於本專案已經完整整合了上述所有舊專案的功能，為了避免多個背景服務重複運行造成衝突與不必要的資源消耗，建議您在確認資料遷移完成後，手動停用並刪除舊服務的常駐設定：

1. **停止並取消自動啟用 (Disable) 舊服務**：
   根據您先前安裝的專案，依序停用對應的 systemd 使用者服務。例如：
   ```bash
   # 停用舊版 Copilot 統計服務
   systemctl --user stop copilot-cli-token-insights.service
   systemctl --user disable copilot-cli-token-insights.service

   # 停用舊版 Antigravity 統計服務
   systemctl --user stop antigravity-cli-token-insights.service
   systemctl --user disable antigravity-cli-token-insights.service

   # 停用舊版 Codex 統計服務
   systemctl --user stop codex-cli-token-insights.service
   systemctl --user disable codex-cli-token-insights.service
   ```

2. **刪除舊服務的描述檔**：
   ```bash
   rm -f ~/.config/systemd/user/copilot-cli-token-insights.service
   rm -f ~/.config/systemd/user/antigravity-cli-token-insights.service
   rm -f ~/.config/systemd/user/codex-cli-token-insights.service
   ```

3. **重新載入 systemd 設定**：
   ```bash
   systemctl --user daemon-reload
   systemctl --user reset-failed
   ```

4. **刪除舊專案的目錄 (選用)**：
   若您不再需要舊專案的原始碼目錄，可以將其整個資料夾刪除以釋放空間。

---

## ⚙️ 前置作業啟用指南

本專案完全運行於本地端，請按照以下步驟完成各助理的數據收集配置：

### 1️⃣ Codex CLI
* **免安裝 Hook**：Codex 採用無侵入式設計，後端會自動遞迴掃描您本地的 `~/.codex/sessions` 目錄，無須進行任何 statusline 或 hook配置。

### 2️⃣ Google Antigravity CLI
1. **複製收集腳本**：
   ```bash
   mkdir -p ~/.gemini/antigravity-cli
   cp shell/antigravity/statusline-token.sh ~/.gemini/antigravity-cli/statusline-token.sh
   chmod +x ~/.gemini/antigravity-cli/statusline-token.sh
   ```
2. **編輯設定檔 `~/.gemini/antigravity-cli/settings.json`**：
   若為全新配置，請寫入以下內容；若已有設定，請將 `statusLine` 物件合併進去：
   ```json
   {
     "statusLine": {
       "type": "command",
       "command": "$HOME/.gemini/antigravity-cli/statusline-token.sh",
       "padding": 1
     }
   }
   ```
   > [!NOTE]
   > 請將 command 欄位中的路徑替換成您的實際家目錄絕對路徑（可執行 `echo $HOME` 查詢）。

### 3️⃣ GitHub Copilot CLI
1. **複製收集腳本**：
   ```bash
   mkdir -p ~/.copilot
   cp shell/copilot/statusline-token.sh ~/.copilot/statusline-token.sh
   chmod +x ~/.copilot/statusline-token.sh
   ```
2. **編輯設定檔 `~/.copilot/settings.json`**：
   若為全新配置，請寫入以下內容；若已有設定，請將 `statusLine` 物件合併進去：
   ```json
   {
     "statusLine": {
       "type": "command",
       "command": "$HOME/.copilot/statusline-token.sh",
       "padding": 1
     }
   }
   ```
   > [!NOTE]
   > 請將 command 欄位中的路徑替換成您的實際家目錄絕對路徑（可執行 `echo $HOME` 查詢）。

### 💡 Status Line 實際顯示效果

當您設定好 `statusLine` 並在終端機中與 AI 助理（Google Antigravity CLI 或 GitHub Copilot CLI）進行對話時，終端機畫面最下方（或狀態欄）會即時顯示目前會話的 Token 統計資訊。

**畫面效果範例：**
```text
🤖 gemini-1.5-pro • #3 • ↑ 12.3k • c 4.5k/0 • ↓ 1.2k • r 500 • total 18.5k • +2.1k • last 1.5k/200 • ctx 15%
```

**欄位說明：**
* 🤖 **模型名稱**：目前會話所使用的 LLM 模型。
* **#回合數** (`#3`)：當前會話已進行的問答回合數。
* **↑ 輸入 Token** (`↑ 12.3k`)：目前為止累計輸入的 Token 數（`k` 代表千，`m` 代表百萬）。
* **c 快取命中率/寫入** (`c 4.5k/0`)：累計快取讀取（Cache Read）與快取寫入（Cache Write）的 Token 數。
* **↓ 輸出 Token** (`↓ 1.2k`)：目前為止累計輸出的 Token 數。
* **r 推理 Token** (`r 500`)：累計推理用的 Token 數。
* **total 總 Token** (`total 18.5k`)：該會話累計消耗的總 Token 數。
* **+增量 Token** (`+2.1k`)：本次對話所額外消耗的 Token 數量。
* **last 上次呼叫** (`last 1.5k/200`)：上一次 API 請求的輸入/輸出 Token 數。
* **ctx 上下文佔用率** (`ctx 15%` 或 `from <舊模型>`): 目前上下文窗口的使用百分比；若更換模型，則會顯示類似 `• from gemini-1.5-flash` 的模型更換提示。

---

## 🚀 啟動與常駐服務

### 一、本地啟動
在專案根目錄下執行：
```bash
cargo run
```
在瀏覽器打開 [**`http://localhost:3003`**](http://localhost:3003) 即可使用您的綜合看板！

### 二、配置為 Systemd 使用者常駐背景服務
1. **編譯發行版本二進位檔**：
   ```bash
   cargo build --release
   ```
2. **配置服務描述檔**：
   ```bash
   mkdir -p ~/.config/systemd/user/
   sed "s|<PROJECT_DIR>|$PWD|g" shell/token-usage-insights.service > ~/.config/systemd/user/token-usage-insights.service
   systemctl --user daemon-reload
   ```
3. **服務管理命令**：
   * **啟動服務**：`systemctl --user start token-usage-insights.service`
   * **設定自動啟動**：`systemctl --user enable token-usage-insights.service`
   * **查看狀態**：`systemctl --user status token-usage-insights.service`
   * **查看日誌**：`journalctl --user -u token-usage-insights.service -n 50 -f`
   * **重啟服務**：`systemctl --user restart token-usage-insights.service`
   * **停止服務**：`systemctl --user stop token-usage-insights.service`

---

## ⚙️ 進階配置與環境變數

您可以透過設定環境變數來覆蓋預設的本地數據目錄：

- `INSIGHTS_DIR`: 看板 SQLite 資料庫的儲存路徑（預設：`~/.token-usage-insights`）
- `ANTIGRAVITY_DIR`: Google Antigravity CLI 的資料儲存目錄（預設：`~/.gemini/antigravity-cli`）
- `COPILOT_DIR`: GitHub Copilot CLI 的資料儲存目錄（預設：`~/.copilot`）
- `CODEX_DIR`: Codex CLI 的資料儲存目錄（預設：`~/.codex`）

例如，自訂資料庫儲存目錄並啟動看板：
```bash
export INSIGHTS_DIR="/your/custom/path"
cargo run
```
