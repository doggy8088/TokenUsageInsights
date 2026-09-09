# 變更記錄

本文件記錄 TokenUsageInsights 各正式版本的實際變更。內容依 Git 標籤間的提交記錄與檔案差異整理，格式參考 [Keep a Changelog](https://keepachangelog.com/zh-TW/1.1.0/)，版本編號遵循 [Semantic Versioning](https://semver.org/lang/zh-TW/)。

## [未發行]

### 修正

- 補齊五種語系 README 的 Muse Code、Cursor 與 GitHub Copilot App 支援資訊，加入資料來源、零設定使用方式、Cursor 與 Muse Code 說明、Windows 路徑、深層連結、CLI Agent 值、`MUSE_DIR` 與疑難排查命令；並將介面 Agent 數量、語系數量及 v0.9.0 單一執行檔狀態修正為目前實際行為。
- 更新公開首頁的支援來源區塊，加入 Pi Coding Agent、OMP 與 Muse Code，將五種語系的來源數量同步修正為十種，並改用可存取的語意清單與桌面雙列、行動版橫向捲動配置。

## [0.9.0] - 2026-09-09

### 新增與改善

- 新增官方 npm 套件 `token-usage-insights` 的跨平台 npx 包裝：首次執行時依 Windows x64、Linux x64、Intel Mac 或 Apple Silicon Mac 下載同版本 GitHub Release，強制使用 `SHA256SUMS` 驗證完整壓縮包，再啟動原生 `token-usage-insights`；不依賴 npm 12 預設封鎖的安裝生命週期腳本。
- Release workflow 新增 npm Trusted Publishing job，使用 GitHub Actions OIDC、Node.js 24 與最新版 npm 發布，不保存長效 npm Token；首次上架前以 Repository variable 保持停用，並加入 npm 套件 CI、版本／tag／Release 資產檢查及發布後 npx 驗證。
- README 五種語言與 public 網頁新增 `npx --yes token-usage-insights` 啟動方式；public 安裝區加入 npx 分頁、Node.js 版本需求及鍵盤可操作的既有分頁互動。
- 新增正體中文 npm 首次上架與 Trusted Publishing 維護文件，列出 npm、GitHub Environment、Repository variable 的完整人工設定值、首次手動發布流程及常見錯誤排查。
- 新增 CLI `export-all [--out <path>]`，一次匯出資料庫中所有 Agent、所有日期的使用量記錄，保留既有匯出欄位與去重識別碼，支援檔案與 stdout 輸出。
- CLI `import --file` 自動依檔案辨識 Agent，一次匯入完整匯出檔的所有 Agent 與日期；`--agent` 改為選填，可用於篩選或補足舊檔的 Agent 資訊。匯入結果改為逐 Agent 的 JSON 陣列，維持既有檔案格式與重複匯入去重行為。

### 變更

- 將看板與 CLI 整合為單一 `token-usage-insights` 執行檔，無參數時啟動看板，並提供 `export`、`export-all`、`import` 子命令，以及主命令與子命令的 `--help`、`-h`。

### 資料影響

- `export` 與 `export-all` 的記錄欄位及既有 JSON 格式維持不變；完整匯出檔會依 Agent 與日期分組，`import` 仍沿用既有資料表與去重識別碼，不需資料庫遷移。

### 相容性

- 移除獨立的 `token-usage-insights-cli` 建置目標；既有指令與自動化腳本需改用 `token-usage-insights`。未使用獨立 CLI 執行檔的看板使用方式不受影響。
- `import` 的成功結果改為逐 Agent 的 JSON 陣列；依賴舊版單一結果物件的自動化需同步調整。
- npx 執行需要 Node.js 18.18 以上版本；原有 GitHub Release 壓縮包與一行安裝腳本仍可使用，且沒有新增應用程式執行階段環境變數。

## [0.8.3] - 2026-09-06

### 修正

- 修復切換 Coding Agent 類型時任意切換與自動回退月份／年份的問題：
  - 當使用者在特定月份（例如 2026-09）或年份下切換 Coding Agent（例如從 Antigravity 切換至 Pi coding agent），若目標 Agent 該月無數據，原本會自動降級回退至最新有紀錄的月份（如 2026-08），造成使用者操作體驗混亂與困惑。
  - 在 `fetchMonths` 與 `fetchYears` 中引入期間保留機制（`keepMonth` / `keepYear`），當所選月份或年份未在目標 Agent 的紀錄清單中時，自動按降冪排列將該期間補入下拉選單並維持選取，不再任意跳轉其他月份。
  - 當後端回應 404（該期間無紀錄）時，實作 `showNoDataForMonth` 與 `showNoDataForYear` 函式，顯示清晰友善的「此 Agent 於當月／當年無資料」提示卡片（如「Pi Coding Agent 在 2026-09 無資料」），並保留前置設定與重新整理按鈕，不再以錯誤彈窗（Toast）干擾或擅自回退歷史期間。
  - 在 `static/i18n.js` 中補充 `no_data_for_month`、`no_data_for_month_desc`、`no_data_for_year`、`no_data_for_year_desc` 多語系翻譯支援（含正體中文、簡體中文、英文、日文、韓文）。
  - 修復切換至無資料期間或後端回傳 404 時日期／月份標題載入旋轉指示器（Loading Indicator）未停止轉動之問題，並將指示器由日期左側移至右側以消除文字位移晃動：
    - 將 `setTitleMarkup` 中的旋轉圖示置於 `<span class="title-text">` 後方（右側），確保日期文字保持向左對齊且在載入前後維持零像素位移（0px shift），徹底杜絕畫面跳動。
    - 於 `.app-main` 樣式引入 `scrollbar-gutter: stable;`，避免內容增減造成縱向捲軸出現／消失時整個主看板頂部標題產生橫向晃動。
    - 新增 `clearTitleSpinner` 函式，於日報、月報、年報載入之 `finally` 區塊、無資料提示畫面及空狀態切換時確實移除 `.title-sync-icon`，並更新靜態資源快取版本號（`styles.css?v=30`、`app.js?v=61`）。
  - 修復切換語系時當前顯示之期間無資料卡片會被誤判為初始空狀態而重置為安裝設定教學卡片的問題，切換語系後立即以新語言正確刷新無資料提示。
- 修復切換至無數據助理時右側看板殘留前一助理數據的問題：
  - 當切換至未安裝或無任何使用日誌之助理（如 Grok Build）時，原本因保留當前日期與月度／年度清單為空直接返回，導致右側視圖未被隱藏，誤顯示前一個助理（如 Pi coding agent）的統計數據與圖表。
  - 在 `fetchDates` 中，當可選日期清單為空時，一律清空快取數據、重設日期選取欄位並強制切換至空狀態。
  - 在 `fetchMonths` 與 `fetchYears` 中，當清單為空且位於對應分頁時，清空當前數據並顯示空狀態卡片。
  - 在切換助理徽章點擊事件中，當助理變更時立即清空暫存數據、重設側邊欄指標卡片並顯示載入中遮罩，避免異步載入期間閃爍前一助理的殘留數據。
  - 新增 `resetMiniStats()` 工具函式，於空狀態與無資料時重置側邊欄當日彙整指標為預設值 `-`。
  - 在 `switchTab` 中完整同步空狀態容器與各視圖的隱藏狀態，避免切換分頁時空狀態失效。

## [0.8.2] - 2026-09-06

### 新增與改善

- 補充 OpenAI 於 2026-09-03 正式發布的 **GPT-6 Astra** 模型標準費率規則（含短上下文 `GPT-6 Astra (<272k)`、長上下文 `GPT-6 Astra (>272k)` 與預設規則條目），修復 Codex 使用量記錄出現 `gpt-6-astra` 時因缺少價格條目導致的「找不到可用的模型價格規則」錯誤。費率採 OpenAI 官方非邊際階梯牌價（Prompt ≤ 272K：輸入 10.00、快取 1.00、輸出 50.00 美元／每百萬 tokens；Prompt > 272K：輸入 20.00、快取 2.00、輸出 75.00 美元；批次 API 50% 牌價）。
- 補充 `gpt-daybreak-blue-latest` 與 `gpt-daybreak-blue` 模型費率規則，費率完全等同於 `gpt-5.6-sol`（輸入 5.00、快取 0.50、輸出 30.00 美元／每百萬 tokens），修復對應 session 成本計算失敗的問題。
- 新增未定價模型日誌去重抑制機制：在 `src/handlers/mod.rs` 引入執行緒安全之全域紀錄 `WARNED_PRICING_MODELS`，對同一未定價模型（如 `copilot/auto` 或各類地端模型）在執行期間僅於首次發生時輸出警告日誌，徹底解決相同模型在連續 Turn 與每次聚合請求中連續重複輸出「計算成本失敗」刷屏的問題。
- 新增單元測試驗證 `GPT-6 Astra` 長短上下文階梯切換、`gpt-daybreak-blue` 費率對照，以及未定價模型單次日誌抑制機制，並加入實際 turn 用量的回歸斷言。
- 新增 GPT-6 Astra（`docs/research/gpt-6-astra-pricing.md`）與 GPT-Daybreak-Blue（`docs/research/gpt-daybreak-blue-pricing.md`）定價研究文件。

### 相容性

- 本次僅補充模型價格規則、日誌輸出抑制、單元測試與研究文件，無資料庫結構、環境變數或安裝流程變更。

## [0.8.1] - 2026-09-06

### 新增與改善

- 補充 Google 於 2026-09-02 正式發布的 **Gemini 3.8 Flash** 模型標準費率規則（含 Base、Medium、High、Low 四種思考層級變體），修復 Antigravity 使用量記錄出現 `Gemini 3.8 Flash (High)` 時因缺少價格條目導致的「找不到可用的模型價格規則」錯誤。費率採 Google 官方標準牌價（輸入 1.50、快取輸入 0.15、輸出 7.50 美元／每百萬 tokens；批次 API 0.75/0.075/3.75 美元）。
- 新增單元測試驗證 `Gemini 3.8 Flash` 各思考層級與 API 識別碼 `gemini-3.8-flash` 皆能正確套用費率，並加入實際 turn Token 用量的回歸斷言。
- 新增 Gemini 3.8 Flash 官方定價研究文件（`docs/research/gemini-3-8-flash-pricing.md`），記錄官方公布費率與思考層級費率推導說明。

### 相容性

- 本次僅補充模型價格規則、單元測試與研究文件，無資料庫結構、環境變數或安裝流程變更。

## [0.8.0] - 2026-09-02

### 新增與改善

- 新增支援 **Muse Code**（由 Meta 提供的 Muse Spark 模型驅動的 Coding Agent），完整涵蓋 Token 使用量分析與 Session 還原。新增 `src/muse.rs` 解析器，讀取 `~/.local/share/muse/sessions/YYYY/MM/DD/<session_id>/session.jsonl` 的 `model_completed` 事件並還原 `input / cached / output / reasoning` Token；以 `runtime.user_intent.accepted` 首句提示產生會話名稱，並支援日期分片掃描與 `MUSE_DIR` 環境變數自訂。
- 資料庫同步新增 `get_muse_dir()` 與 `sync_muse_usage_logs()`，沿用 `pi / omp / grok` 共用的 `sync_pi_family_usage_logs` 增量機制（`sync_state` + `transcript_path` 去重），確保重跑不重複寫入。
- 前端新增 Muse Code 徽章（`static/index.html`）、`assistantMeta.muse`（`Muse Code` 品牌色 `#3b82f6` 整段高亮）與 `assistantAliasMap` 別名（`muse-code` / `muse_code` / `musecode` 等），並在 `static/i18n.js` 為 zh-TW / zh-CN / en / ja / ko 五語系補齊 `muse_header_description` 與 `assistant_muse`，使副標題正確顯示為「本地監控與分析您的 Muse Code 的 Token 消耗與會話詳細數據」並以品牌色標示。
- `pricing.csv` 依官方 `model-catalog` 新增 `muse-spark-1.2`（輸入 1.25 / 快取 0.15 / 輸出 4.25）與 `muse-spark-1.2-contributor`（0.10 / 0.002 / 0.20），單位 1M Tokens，對應 Meta 公開成本。
- CLI 工具 `src/bin/token-usage-insights-cli.rs` 同步支援 `--agent muse` 的匯入／匯出、別名正規化與說明文件；同步更新 `static/app.js` 的 Agent 切換與搜尋邏輯。

### 修正

- 修正 Muse 在 macOS 上因 `dirs::data_local_dir()` 指向 `~/Library/Application Support` 而漏掃 `~/.local/share/muse/sessions` 的問題；`get_muse_dir()` 改為優先檢查 `~/.local/share/muse/sessions` 是否存在，避免資料為 0。
- 修正 Muse `recorded_at` 時間戳單位錯誤（誤以 nanoseconds 解析導致 date 落在 1970-01-21），改以 microseconds 優先並加入 2020-2100 合理性檢查，同步修正 `src/timeline.rs` 的同類轉換，使 `2026-09-01` 等日期的統計與時間軸正常顯示。
- 修正先前 `pricing.csv` 中 Muse 模型的佔位價格，改為與官方成本一致。

### 資料影響

- `usage_entries` 新增 `assistant_type='muse'` 與 `source_kind='muse-code'` 的資料；既有 8 個 Agent 的資料與索引（`assistant_type, source_kind, session_id, turn_no, usage_identity`）保持不變，無需遷移。
- 既有 `muse` 的錯誤日期資料（1970）會在下次增量同步時由 `sync_state` 重新寫入正確日期，無需手動清理（已於驗證期間清理）。

### 相容性

- 新增選用環境變數 `MUSE_DIR`，未設定時自動偵測 `~/.local/share/muse`；既有 `ANTIGRAVITY_DIR`、`COPILOT_DIR` 等維持相容。
- `GET /api/:assistant/*` 對 `muse` 完全相容，既有前端路由與匯入匯出流程不受影響。

### 其他

- 完成 `cargo fmt` / `cargo clippy --all-targets --all-features` / `cargo test`（143 項）零警告驗證，並以本機真實 `~/.local/share/muse` 資料（220 筆 turn，`GET /api/muse/usage/2026-09-01`）驗證統計與費用估算正確。

## [0.7.5] - 2026-08-30

### 新增與改善

- 新增支援兩個 Agent 類型：**Pi Coding Agent**（<https://pi.dev/>）與 **OMP**（<https://omp.sh/>，Pi 的開源分支）。兩者共用相同的樹狀結構 JSONL session 格式（`<dir>/agent/sessions/**/*.jsonl`），新增 `src/pi.rs` 共用解析器與 `src/omp.rs` 薄封裝，並整合進資料庫同步（`sync_pi_usage_logs` / `sync_omp_usage_logs`）、API handlers、Session 時間軸重建與 CLI 工具；兩者的成本一律直接讀取 session 每個 turn 自行回報的 `usage.cost`，不需要額外的 `pricing.csv` 估算。
- 前端新增 Pi／OMP 的 Agent 徽章、原創 Logo（`pi-logo.svg`／`omp-logo.svg`，避免使用官方商標圖檔）、設定精靈內容，並補齊 zh-TW／zh-CN／en／ja／ko 五語系文案。
- 「AGENT 類型」側欄圖示改為 4 欄 × 2 列的格線排列（`repeat(4, minmax(0, 1fr))`），因應圖示數量增加到 8 個仍維持整齊排版。
- 各 Agent 專屬副標題（首頁描述文字）現在會將該 Agent 的產品名稱以品牌色標示（新增 `agent-name-highlight` 樣式與 `highlightAgentName()`），並修正 Pi／OMP 繁簡中文副標題缺漏「的」字的文法問題。
- 5 份 README（README.md／README.zh-CN.md／README.en.md／README.ja.md／README.ko.md）同步補上 Pi Coding Agent 與 OMP 的說明段落（總覽、setup 需求表、Windows 原生路徑表、專屬設定章節、環境變數表）。
- 靜態檔案伺服器（`/static` 與 fallback 路由）一律附加 `Cache-Control: no-cache` 標頭，強制瀏覽器每次都向伺服器重新驗證（仍透過既有的 ETag／Last-Modified 條件式請求機制回傳 304，不增加頻寬成本），避免部署更新後使用者因瀏覽器快取仍看到舊版 JS／CSS。

### 修正

- 修復 OMP session 檔案因在 `{"type":"session",...}` 標頭前多一行 `{"type":"title",...}`，導致 `read_session_header()` 只讀取檔案第一行而抓不到 `cwd`／`session_id`，畫面顯示為「Unknown CWD」的問題；改為掃描前 10 行尋找 `type == "session"` 的項目。
- 修復 OMP 的 `model_change` 事件使用 `"model"` 欄位（Pi 使用 `"modelId"`）而未被 Session 時間軸解析辨識，導致「Model changed to X」系統訊息遺漏的問題。
- 修復 Pi／OMP usage 資料中的 `usage.reasoning`（推理 Token 數）未被解析、恆為 0 的問題。

以上變更均已通過 `cargo build --release --all-targets`（零警告）、`cargo test`（346 項全數通過）、`cargo clippy --all-targets --all-features`（零警告），並以本機真實 `~/.pi`、`~/.omp` 資料驗證同步結果正確。

## [0.7.4] - 2026-08-28

### 新增與改善

- 切換日期、月份或年份時顯示「資料載入中…」提示層，內容區淡化並停用互動，資料抵達後立即替換；同一期間重整（即時監控、重新整理按鈕）不遮蔽內容，僅在標題顯示旋轉同步圖示提示。
- 日／月／年報載入加入請求序號保護：快速切換期間時僅讓最新一次請求的結果生效，舊回應不再覆蓋新畫面，也不再閃現過期的錯誤通知；同一期間已在載入時不重複發送請求。
- 新增 `loading_data` i18n 鍵（zh-TW／zh-CN／en／ja／ko 五語），並更新靜態資源快取版本號。

### 修正

- 修復在月／年報分頁時誤呼叫日報 API 的問題：網址上的 `month`／`year` 查詢參數先前會被誤當日期請求，404 後閃現「無資料」畫面蓋掉月／年報；現在改為重載當前期間或重建清單。
- 修復月／年報表「最常活動的專案目錄」與「使用的模型佔比」雙欄卡片在常見視窗寬度下，右側卡片被表格 `min-width` 撐出可視範圍且無法捲動找回的問題，改為上下堆疊。

### 效能

- 新增 `PreparedPricingRules`：價格規則標籤（長文閾值、Claude 判定）於載入時一次性解析，日／月／年報聚合計價不再對每筆 usage 反覆執行字串剖析，顯著降低大量 usage 聚合的 CPU 耗時。

### 其他

- `make run` 改為先建置 Release 並直接執行二進位、`make dev` 改為 Debug 版本快速迭代、`make run-release` 等同 `run`，並同步更新說明文字。

## [0.7.3] - 2026-08-27

### 修正

- 補充 GLM-5.3-Flash 模型的標準費率規則，修復對應 session 成本計算擲回「找不到可用的模型價格規則」的問題。定價採 Z.ai 官方標準牌價（輸入 0.15、快取輸入 0.03、輸出 0.50 美元／每百萬 tokens），並新增單元測試驗證 `glm-5.3-flash`、`glm-5.3-flash:cloud`（Ollama 雲端路由）與 `GLM-5.3-Flash`（大小寫變體）三種名稱格式皆能正確套用定價。

## [0.7.2] - 2026-08-27

### 修正

- 補充 Gemini 3.7 Flash 模型（含 Base、Medium、High、Low 四種思考層級變體）的標準費率規則，修復對應 session 成本計算擲回「找不到可用的模型價格規則」的問題。新增單元測試驗證思考層級定價，並附上 API 定價研究文件。

## [0.7.1] - 2026-08-25

### 修正

- 修正只安裝 GitHub Copilot CLI（未安裝 Copilot Desktop App）時，主控台每 5 秒重複輸出「找不到 data.db」警告造成洗版的問題。缺少 `data.db` 屬 CLI-only 用戶的正常狀態，現在改為整個執行期間僅提示一次，並註明屬正常狀態。
- 修正 CLI-only 用戶的 Copilot CLI agent reconciliation 被整個跳過的問題。先前 `data.db` 不存在時會直接放棄校正，導致 subagent 用量永遠停留在 hook 合併列；現在改以空的 App session registry 照常執行拆分校正。App session 的歸類仍以 CLI transcript 存在與否把關，不會被誤判。
- 補充 Cursor Composer 2.5 兩種速度層級（`composer-2.5`、`composer-2.5-fast`）的模型價格規則，修復對應 session 成本計算擲回「找不到可用的模型價格規則」的問題。先前計算失敗的 turn 不會自動補算，需重新匯入或重算該 session 才會顯示成本。

## [0.7.0] - 2026-08-11

### 新增與改善

- 看板支援以網址查詢參數直接開啟指定狀態的畫面：`agent` 指定 Coding Agent、`tab` 指定日 / 月 / 年視圖、`date` 指定日期、月份或年份，並新增 `dir`（每日視圖工作目錄篩選，支援完整路徑、`~` 家目錄寫法與唯一路徑尾碼比對）與 `chart`（每日圖表類型 `kline` / `trend`）參數。切換 Agent、視圖、日期、工作目錄或圖表類型時網址會自動同步，方便加入書籤與分享連結。五種語系的 README 皆已補上用法說明。

### 修正

- 修正每日視圖工作目錄篩選的尾碼比對，改以路徑分隔邊界為準並統一不分大小寫，避免誤配較長的目錄名稱。
- 通知訊息改用 `textContent` 寫入，消除 XSS 注入風險（CodeQL `js/xss-through-exception`）。
- 補充 kimi-k3 模型價格規則，修復成本計算失敗的問題。

## [0.6.4] - 2026-08-08

### 修正

- 修正 GitHub Copilot CLI 與 App 工作目錄 (CWD) 無法顯示的問題。CLI Agent 覆蓋原本含 CWD 的 hook rows 時硬編碼 `cwd = NULL`，App 路徑也從未設定 CWD。現在改從 `session-store.db.sessions` 表取得 CWD，並新增一次性回填遷移補齊既有資料。

### 資料影響

- 新增 `migration:copilot_cwd_backfill_v1` 一次性遷移：掃描 `usage_entries` 中 `assistant_type = 'copilot'` 且 `cwd IS NULL` 的記錄，從 `session-store.db.sessions` 補填工作目錄，不影響其他助理資料。

## [0.6.3] - 2026-08-07

### 新增與改善

- 新增 Claude Opus 5 模型定價規則。
- 完成五語系文件（繁中、簡中、英文、日文、韓文）與網站介面本地化。
- 整理一頁式介紹頁總結並完成 GitHub Pages 發佈設定。
- 調整首頁 footer 版面與課程連結呈現。

### 修正

- 修正每日 Token 摘要重複計算的問題，避免同一 Session 跨多筆記錄時總量被加倍。
- 容錯處理 VS Code Copilot 聊天記錄中的未成對 UTF-16 代理字元（lone surrogate），避免解析時發生 panic。

## [0.6.2] - 2026-08-01

### 新增與改善

- 支援 GitHub Copilot App（Tauri 桌面應用）的 Token 使用量同步，自動讀取 `~/.copilot/data.db` 與 `~/.copilot/session-store.db`，依 Session、Turn、Agent 與模型保存 `assistant_usage_events`，並以 `source_kind = "copilot-app"` 與 CLI / VS Code 區分；Session 清單以 `App` 標示來源。新增 `COPILOT_APP_DIR` 環境變數自訂 App 資料目錄。
- 新增 Grok Build 助理支援，掃描 `~/.grok/sessions` 的 `updates.jsonl`，提供每日、月度、年度統計、Session 清單與時間軸還原。
- 支援 Grok Build 的 provider usage、model alias、reasoning effort、快取讀取與 context-only snapshot，並在 Session 清單標示 `Usage` 或 `Context` 來源。
- 模型用量明細新增 Session 下鑽，可從每日、月度與年度模型統計直接查看對應 Session。
- 看板匯出會依目前報表範圍輸出完整日、月或年資料；CLI 同步支援 `YYYY`、`YYYY-MM` 與 `YYYY-MM-DD`。
- 匯入改為逐筆依 `timestamp` 決定日期，可一次匯入跨日、跨月或跨年的完整檔案。
- 新增 `HOST` 環境變數控制看板綁定位址，啟動訊息會顯示實際可連線網址。

### 變更

- Grok Build Session 若包含 provider 回報成本，統計頁面會優先使用該成本；只有 context snapshot 時才依 xAI API pricing 估算。
- Cursor 會從本機 `state.vscdb` 的 `agentKv` 記錄歸因實際模型與 Mode；無法唯一比對時保留為未知模型，避免錯誤歸因。
- Session 清單改依實際時間排序，統一各來源時間戳解析。
- 費用統計改依每筆資料的實際模型計算，不再以整個 Session 的單一模型套用價格。

### 資料影響

- SQLite `usage_entries` 新增 `reported_cost_usd` 欄位，並以一次性 migration 重新解析既有 Grok Build Session；不會刪除其他助理的資料。
- Copilot App 會依來源目錄、Session、Turn、Agent 與模型隔離記錄，並清理舊版可能合併的資料；既有匯入批次與其他助理資料不受影響。

### 相容性

- 新增 `GROK_DIR`、`COPILOT_APP_DIR`、`CURSOR_STATE_DB` 與 `HOST` 環境變數；既有助理識別碼、API 與資料目錄維持相容。
- 匯入檔案的頂層 `date` 與 CLI `--date` 僅保留作為批次標籤；資料實際日期一律由每筆 `timestamp` 決定。

### 修正

- 防止匯入檔案被寫入不同 Agent，保留來源驗證、重複資料去重、批次追蹤與撤銷能力。
- 修正 Claude 快取寫入費用、每日舊格式用量彙整，以及多模型 Session 的成本計算。
- 新增 Claude Opus 5 與 Gemini 3.6 Flash 費率，並移除已下架的 Grok 模型定價。
- 修正非 Codex 長上下文模型的計價門檻，改由 `pricing.csv` 動態解析各模型的實際門檻，並將 Gemini 3.1 Pro 與 Claude Opus 4.6 的門檻修正為 200K。
- 修正 Gemini 3.1 Pro 的快取價格，改用 Google Standard Context caching 的 0.20／0.40 美元費率。
- 修正模型名稱 contains fallback，優先套用最具體的模型 base，避免 `GPT-5.4-mini-picker` 誤用 `GPT-5.4` 價格。
- 修正 Copilot App 與 CLI 同一 Agent 使用多個模型時可能合併或覆寫用量，以及 CLI 總量暫時不符時錯誤推進同步游標的問題。
- 修正 Grok Build provider cost、headless 快取輸入、同回合多模型歸因、未知模型與時間軸 EOF 回合的解析行為。
- 保留 `grok-build-0.1` 的獨立歷史定價規則，避免誤套 Grok 4.5 定價。

## [0.6.1] - 2026-07-26

### 修正

- 修正 Windows 上既有 Codex transcript 路徑使用不同大小寫或斜線格式時，重新同步可能無法移除舊路徑記錄，造成同一 Session 留下重複資料的問題。
- transcript 路徑會以正規化鍵進行跨格式比對，刪除資料時則使用資料庫保存的原始路徑精確命中索引。

### 相容性

- Windows、macOS 與 Linux 的既有 Codex Session、API、資料庫結構及 `CODEX_DIR` 設定維持相容，不需要手動遷移。

## [0.6.0] - 2026-07-26

### 新增與改善

- Codex 用量收集擴充至 Codex Desktop，並在每日 Session 清單以 `Desktop` 或 `CLI` 標記來源。
- 同時掃描 `~/.codex/sessions` 與 `~/.codex/archived_sessions`，納入作用中及已封存的 Codex Session。
- 保存 Codex transcript 提供的 cache-write Token，讓快取寫入統計更完整。

### 修正

- 修正 Codex Session 封存後因 transcript 移動至 `archived_sessions` 而無法同步或開啟時間軸的問題。
- 修正 Session 從封存狀態還原後，既有同步狀態可能讓資料庫保留舊 transcript 路徑的問題。
- 修正大量 Codex transcript 會讓啟動同步阻塞 HTTP 服務的問題；看板現在會先開始監聽，再於背景執行增量同步。
- 修正背景同步逐檔掃描全部 Codex 資料列，並重複解析沒有 Token 事件之 transcript 的效能問題。

### 資料影響

- 新增一次性 Codex 來源分類遷移；下次同步會重新掃描既有 Codex transcript，將可辨識的記錄分類為 Desktop 或 CLI。
- 自動建立 `(assistant_type, transcript_path)` 複合索引，加速 transcript 路徑查詢與重建。
- 沒有 Token 事件的 Codex transcript 會以同步狀態標記為已檢查；後續只有檔案大小改變時才重新解析，不會新增虛構的用量資料。

### 相容性

- `codex` 助理識別碼、既有 API 與 `CODEX_DIR` 環境變數維持相容；Codex Session 的 `source_kind` 會由 `legacy` 更新為可辨識的來源值。
- 資料庫索引與同步狀態會自動建立，不需要手動遷移或調整安裝設定。

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

[未發行]: https://github.com/doggy8088/TokenUsageInsights/compare/v0.9.0...HEAD
[0.9.0]: https://github.com/doggy8088/TokenUsageInsights/compare/v0.8.3...v0.9.0
[0.8.3]: https://github.com/doggy8088/TokenUsageInsights/compare/v0.8.2...v0.8.3
[0.8.2]: https://github.com/doggy8088/TokenUsageInsights/compare/v0.8.1...v0.8.2
[0.8.1]: https://github.com/doggy8088/TokenUsageInsights/compare/v0.8.0...v0.8.1
[0.8.0]: https://github.com/doggy8088/TokenUsageInsights/compare/v0.7.5...v0.8.0
[0.6.1]: https://github.com/doggy8088/TokenUsageInsights/compare/v0.6.0...v0.6.1
[0.6.0]: https://github.com/doggy8088/TokenUsageInsights/compare/v0.5.0...v0.6.0
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
