# 變更記錄

本文件記錄 TokenUsageInsights 各正式版本的實際變更。內容依 Git 標籤間的提交記錄與檔案差異整理，格式參考 [Keep a Changelog](https://keepachangelog.com/zh-TW/1.1.0/)，版本編號遵循 [Semantic Versioning](https://semver.org/lang/zh-TW/)。

## [未發行]

### 新增

- 支援 GitHub Copilot App（Tauri 桌面應用）的 Token 使用量同步，自動讀取 `~/.copilot/data.db` 與 `~/.copilot/session-store.db`，以 `(session_id, turn_index)` 為單位聚合 `assistant_usage_events` 並以 `source_kind = "copilot-app"` 與 CLI / VS Code 區分；Session 清單以 `App` 標示來源。新增 `COPILOT_APP_DIR` 環境變數自訂 App 資料目錄。

## [0.5.0] - 2026-07-20

### 新增與改善

- 在「今日會話列表」加入工作目錄下拉選單，自動彙整當日所有 Session 的工作目錄、正規化路徑並去除重複。
- 選取工作目錄後，同步篩選每日摘要、側欄快速統計、Token 與成本指標卡、K 線圖、趨勢圖及 Sessions 清單。
- 將使用者家目錄前綴縮寫為 `~`，並支援 Unix 與 Windows 路徑格式，縮短選單、列表、時間軸與設定資訊中的路徑顯示。
- 調整 Sessions 標題、工作目錄選單與提示詞搜尋欄的響應式排版，避免長路徑或窄螢幕造成控制項擠壓。

### 變更

- 工作目錄篩選不寫入瀏覽器持久化儲存空間；重新整理頁面、切換日期或手動重新載入每日資料時，會恢復顯示整天資料。
- 即時資料更新與圖表模式切換會保留當下工作目錄篩選，避免操作過程中突然重設。

### 相容性

- 每日明細 API 新增 `home_dir`，Session 摘要新增 `total_requests`；既有欄位與端點維持相容。
- 資料庫結構、環境變數與安裝流程沒有變更，不需要資料遷移或設定調整。

## [0.4.1] - 2026-07-16

### 修正

- 修正透過一行安裝腳本安裝後，從 `~/.local/bin/token-usage-insights` symlink 在任意工作目錄啟動時，無法定位套件內 `static/` 資源而結束的問題。
- 資源定位會先解析執行檔的真實路徑，並在解析失敗或原始路徑不同時保留既有搜尋路徑作為備援。

### 相容性

- 既有 API、資料庫結構、環境變數與安裝流程維持相容。

## [0.4.0] - 2026-07-16

### 新增

- 在「今日會話列表」加入 USER 提示詞搜尋，可不區分大小寫搜尋當日所有 Session 的完整 USER 提示詞內容。
- 搜尋介面提供防抖、請求取消、符合筆數、無結果、部分檔案無法搜尋與失敗狀態，並在資料自動刷新時保留搜尋條件。
- 在 Session 列表最右側加入可排序的工作路徑欄位；過長路徑以省略號呈現，並保留完整路徑提示。
- 新增每日 Session 搜尋 API，支援 Antigravity、Copilot CLI、VS Code Copilot Chat、Codex、Claude Code 與 Cursor。

### 變更

- Session 名稱改為採用開頭連續 USER 提示詞中的最後一筆；若開頭只有一筆則維持第一筆，若開頭不是 USER 訊息則退回後續第一筆 USER 提示詞。
- 將一致的 Session 名稱選取規則套用至所有支援的本機助理來源。
- 更新前端靜態資源版本，確保瀏覽器載入新增的搜尋翻譯與工作路徑欄位。

### 資料影響

- 新增一次性同步遷移，重設受影響來源的同步狀態並依新規則重建 Session 名稱；既有用量資料不會直接刪除。

### 相容性

- 搜尋 API 為新增端點，既有 API、環境變數與安裝流程維持相容。
- 未新增破壞性資料庫結構變更。

## [0.3.2] - 2026-07-16

### 修正

- 側邊欄切換快捷鍵只在單獨按下 `Command+B` 或 `Ctrl+B` 時觸發。
- 同時按下 `Ctrl`、`Option` 或 `Shift` 等額外修飾鍵時，不再攔截作業系統或瀏覽器的既有快捷鍵。
- 保留輸入框、文字區域、選取欄位與 `contenteditable` 元素的按鍵排除行為。

## [0.3.1] - 2026-07-16

### 修正

- 不再同步沒有任何請求、模型或 Token 資料的空白 VS Code Copilot Chat 工作階段，避免灌高工作階段數並產生無效的模型計價警告。
- 新增一次性資料清理，精確移除既有的空白 VS Code Copilot Chat 佔位紀錄，同時保留具有實際用量的異常紀錄。
- 零 Token 工作階段直接以零成本處理；存在 Token 卻缺少模型時，改為回報明確的中繼資料錯誤。
- 修正 Copilot CLI 的 `inputTokens` 已包含 `cacheReadTokens`，卻再次計入輸入 Token 與費用的問題。
- 在 Copilot CLI 日誌同步、資料匯入、既有統一資料庫及舊版獨立資料庫遷移等路徑統一套用可重複執行的 Token 正規化。
- 統一每日、每月、年度與多助理統計的非快取輸入 Token 定義，移除前端重複扣除快取或額外加入推理 Token 的推算。

### 測試

- 新增空白 Copilot 工作階段、既有佔位資料清理、缺少模型計價、快取輸入正規化、匯入與資料庫遷移的回歸測試。

## [0.3.0] - 2026-07-15

### 新增

- 在每日用量加入可與 Session 趨勢圖切換的實驗性 Token K 線圖。
- 提供 5 分鐘、15 分鐘、30 分鐘、1 小時、2 小時與 4 小時六種時間刻度。
- 依事件時間彙整輸入、輸出及快取讀寫 Token，並在每個有效區間顯示估算費用。
- 加入 MA5 累積用量移動平均線、每小時 Token 斜率，以及加速、降溫或平穩的動能判定。
- 為高密度刻度加入最多 24 根 K 棒的可視視窗、拖曳、觸控板、滾輪、範圍滑桿、方向按鈕與鍵盤操作。

### 變更

- 每日資料邊界維持 UTC 00:00–24:00，圖表刻度、浮動資訊、Session 列表與對話時間則以瀏覽器本地時區顯示。
- 即時刷新會保留使用者正在檢視的歷史視窗；位於最新端時才會自動跟隨新資料。
- 依可視區間動態調整 Y 軸範圍、MA5 與動能標記，並隱藏未來或視窗外的圖形。
- 補齊深色與淺色主題、ARIA 說明、鍵盤焦點、減少動作偏好及行動版操作樣式。

### 修正

- 修正分類軸自動略過刻度時，後半段 K 棒外框與費用標籤錯置到圖表左側的問題。
- 忽略未來時間戳的 Token，避免提前納入累積量、費用與趨勢線。
- 將自訂圖形裁切在圖表範圍內，避免覆蓋座標軸與控制項。

## [0.2.2] - 2026-07-14

### 新增

- 在工作階段重建抽屜加入估算費用，沿用每日工作階段資料的 `cost_usd`，使列表與明細顯示一致。
- 新增專案專用的版本升級與正式發佈技能，涵蓋版本同步、零警告驗證、標籤、GitHub Actions 與 Release 資產檢查。

### 變更

- 統一從表格列與 Token 圖表開啟工作階段抽屜的資料傳遞方式，改為傳入完整 Session 物件。
- 桌面版工作階段統計區擴充為七欄，並調整 900px 與 480px 以下的響應式排列。

### 修正

- 修正 `scripts/get.sh` 透過管線解析最新版號時，因 `grep -m1` 提前關閉管線而間歇觸發 `curl: (23)` 的問題。
- 最新版號改由 GitHub `/releases/latest` 的最終重新導向網址取得，並檢查未發生重新導向的邊界情況。

## [0.2.1] - 2026-07-12

### 修正

- 修正 Antigravity 與 GitHub Copilot CLI 共用同步 SQL 寫入 26 個欄位、卻只有 25 個 `VALUES` 佔位符，導致 SQLite 拒絕整批資料的問題。
- 補齊 Antigravity 完整同步路徑回歸測試，驗證 `source_kind`、`tokens_cache_write` 與 `delta_cache_write` 等欄位可正確保存。
- 將 README、`scripts/get.sh` 與 `scripts/get.ps1` 的 GitHub Raw 來源分支修正為 `main`。

## [0.2.0] - 2026-07-12

### 新增

- 新增 VS Code 工作區儲存資料與 GitHub Copilot Chat 工作階段解析器。
- 解析工作階段、對話要求、模型、Token 用量、工具呼叫與時間軸內容，並重播 VS Code 操作紀錄以處理更新及刪除。
- 加入來源識別與來源範圍唯一索引，讓 Copilot CLI 與 VS Code Copilot Chat 可在相同助理類型下分來源同步，避免互相覆蓋或重複。
- 將新來源整合進每日明細、工作階段時間軸、CLI 匯出與匯入流程。
- 新增解析、重播、資料保存及既有 Copilot 資料來源遷移測試。

### 變更

- 將 Linux、macOS 與 Windows 的一行安裝命令改為 README 的主要上手方式。
- 統一前端中英文顯示名稱為 GitHub Copilot。

## [0.1.5] - 2026-07-11

### 修正

- 將匯入 API 的請求內容上限提高至 200 MiB，避免大型 JSON 被 Axum 預設限制拒絕。
- 修正每日匯入 SQL 的欄位、佔位符與參數數量不一致問題。
- 強化匯入交易與錯誤處理，避免部分寫入造成資料不一致。
- 在 `Cargo.toml` 設定預設執行目標並調整 Makefile，使 `make run` 在多執行檔專案中仍會啟動儀表板伺服器。

### 測試

- 新增大型請求與匯入資料寫入的回歸測試。

## [0.1.4] - 2026-07-11

### 新增

- 新增 `scripts/get.sh`，可偵測 Linux 或 macOS 的平台與 CPU 架構，下載指定或最新 Release 後執行安裝。
- 新增 `scripts/get.ps1`，提供對應的 Windows 一行安裝流程。
- 新增 `scripts/build.ps1`，在 Windows 執行 release 測試與全部 targets 建置。

### 變更

- 改善執行檔資源定位，確保從 Release 套件安裝後仍可讀取前端與定價資產。
- 將 Rust 編譯警告視為建置失敗，涵蓋儀表板與 CLI 兩個 binary targets。
- Release workflow 新增安裝腳本語法檢查、實際安裝、服務啟動與 API 煙霧測試。

## [0.1.3] - 2026-07-11

### 新增

- 新增每日使用量 JSON 匯出與匯入 API。
- 新增 `token-usage-insights-cli`，支援依助理與日期匯出、匯入資料。
- 加入匯入來源識別、重複資料排除及延伸 Codex 欄位保存。
- 在 Web 介面加入匯出、匯入操作與結果提示。

### 變更

- 將語言切換按鈕改為台灣與美國國旗圖示，並記錄素材來源。

### 修正

- 修正 `token-usage-insights-cli` 共用主程式模組時的路徑解析，恢復兩個 binary targets 的正常編譯。

## [0.1.2] - 2026-07-10

### 新增

- 新增 PowerShell Status Line，以及 Antigravity 與 GitHub Copilot 的 Windows 啟動腳本。
- 重整 `install.ps1`，提供 Windows 原生安裝流程。
- 新增 Windows 測試腳本，並在 GitHub Actions Release workflow 執行 Windows 測試與安裝煙霧測試。
- 新增跨平台 Release 資源定位模組，使執行檔可找到 `static/` 與 `pricing.csv`。

### 修正

- 改善 Windows 環境變數、使用者目錄、路徑分隔符與執行檔資源定位。
- 修正 Codex 工作階段重建、增量 Token 計算、重複事件與累積計數重設處理。
- 改善前端載入、錯誤顯示與跨平台路徑處理。

## [0.1.1] - 2026-07-10

### 新增

- 新增 Cursor 助理類型、本機日誌解析、API、時間軸、工作階段清單與前端篩選介面。
- 新增 Cursor 圖示與中英文介面文字。
- 擴充 Cursor 的 GPT-5、Claude Sonnet、Haiku、Opus 與 Fable 系列模型定價。
- 新增 GPT-5.6 模型 ID 與定價資料。

### 變更

- 將助理選擇器調整為單行顯示。
- 加入跨平台換行正規化設定，並更新 repository URL、專案歸屬與畫面截圖來源。

### 修正

- Codex 工作階段日誌缺少 `model` 欄位時，不再套用可能造成錯誤費用估算的預設模型。

## [0.1.0] - 2026-07-07

### 新增

- 建立 Axum、SQLite 與靜態前端組成的統一儀表板，同步 Antigravity、GitHub Copilot CLI、Codex CLI 與 Claude Code 的本機使用紀錄。
- 提供每日、每月與年度彙整，以及圖表、明細表、專案與模型統計。
- 顯示輸入、輸出、快取讀取、快取寫入與推理 Token，並依 `pricing.csv` 估算費用及處理 272K context 定價門檻。
- 重建 Codex 與 Antigravity 對話時間軸，解析工具呼叫、推理能力、壓縮事件、Git 分支及儲存庫資訊。
- 從 Codex 與 Antigravity 對話內容產生工作階段名稱。
- 提供 Codex 額度限制、手動重置查詢、憑證清單、憑證切換與過期檢查。
- 提供 Linux systemd user service、Antigravity 與 Copilot Status Line、Makefile、安裝腳本及 tag 驅動的跨平台 GitHub Release workflow。
- 建立正體中文與英文介面、行動版側邊欄、偏好保存與圖表導覽。

### 變更

- 將統一資料庫預設位置改為 `~/.token-usage-insights`，並提供舊資料庫自動遷移。
- 將後端拆分為 handlers、資料庫、定價與時間軸模組，前端拆分語系與樣式資源。
- 將 API 讀取路徑上的同步改為每 5 秒執行的背景增量同步。

### 修正

- 修正硬編碼的個人家目錄路徑、Codex 設定語系及跨平台資源路徑。
- 修正 Codex 與 Antigravity 時間軸解析、Codex Token 增量計算及 Antigravity Git 資訊顯示。
- 修正行動版側邊欄遮擋、黑畫面、標題擠壓、圖表導覽索引與年度版面問題。
- 修正並補齊多個 Gemini、Claude、GPT 與 GPT-OSS 模型的定價規則。

[未發行]: https://github.com/doggy8088/TokenUsageInsights/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/doggy8088/TokenUsageInsights/compare/v0.4.1...v0.5.0
[0.4.1]: https://github.com/doggy8088/TokenUsageInsights/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/doggy8088/TokenUsageInsights/compare/v0.3.2...v0.4.0
[0.3.2]: https://github.com/doggy8088/TokenUsageInsights/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/doggy8088/TokenUsageInsights/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/doggy8088/TokenUsageInsights/compare/v0.2.2...v0.3.0
[0.2.2]: https://github.com/doggy8088/TokenUsageInsights/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/doggy8088/TokenUsageInsights/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/doggy8088/TokenUsageInsights/compare/v0.1.5...v0.2.0
[0.1.5]: https://github.com/doggy8088/TokenUsageInsights/compare/v0.1.4...v0.1.5
[0.1.4]: https://github.com/doggy8088/TokenUsageInsights/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/doggy8088/TokenUsageInsights/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/doggy8088/TokenUsageInsights/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/doggy8088/TokenUsageInsights/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/doggy8088/TokenUsageInsights/releases/tag/v0.1.0
