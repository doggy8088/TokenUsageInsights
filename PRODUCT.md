# Product

## Register

product

## Users

本產品面向在本機終端使用 AI CLI 助理的開發者與技術使用者，包含 Google Antigravity CLI、GitHub Copilot CLI、Codex CLI 與 Claude Code 的重度使用者。使用情境通常是本機開發工作站、macOS 或 WSL 環境，使用者需要在不中斷工作流的狀況下快速理解 token 消耗、成本、模型使用、快取狀態、工作目錄與完整會話歷史。

## Product Purpose

Token Usage Insights 是本地優先的 AI CLI 使用量分析看板。它整合多個本地 AI 助理的日誌與 status line 資料，提供每日、月度與年度 token 分析、成本估算、模型與專案分佈，以及可還原每個 session 的對話時間軸。

成功的產品狀態是：使用者能在數秒內判斷哪個助理、哪個模型、哪個專案或哪個會話造成主要消耗；能查驗估算成本與快取效益；能回溯 AI 在本地執行的工具步驟；也能安全理解資料來源仍保留在本機。

## Brand Personality

精準、克制、專業。

語氣應直接、可驗證、偏操作導向。介面可以保留監控工具的科技感，但視覺表現必須服務資料判讀，不應用裝飾性效果取代層級、狀態與可讀性。

## Anti-references

不應長得像行銷 landing page、泛用 SaaS hero、純裝飾性的毛玻璃樣板、過度霓虹的科幻監控牆、低對比灰字資料表、無意義的紫藍漸層裝飾、只展示漂亮大數字卻缺乏可追溯細節的 hero metric template。

既有深色 glassmorphism 視覺可以作為辨識基礎，但未來調整應避免把模糊、光暈、漸層文字或大陰影當成預設裝飾。資訊密度、掃描速度、狀態清楚度與鍵盤可操作性優先。

## Design Principles

1. 資料先於裝飾：token、成本、快取、模型與時間軸資訊必須比視覺效果更容易被看見與比較。
2. 本地信任感：介面應持續傳達資料來自本機日誌與 SQLite，同時避免暗示有遠端同步或雲端依賴。
3. 快速掃描：核心數字、排序、篩選、狀態與時間維度必須能在密集畫面中被快速定位。
4. 可追溯而非只彙總：總覽數字需要能一路連回 session、turn、工具呼叫與原始上下文。
5. 雙語清晰：繁中與英文 UI 文案都應保持短、準、可操作，避免冗長說明壓過資料。

## Confirmed Design Direction

目前確認的重構方向是 Local Operations Console：保留既有深色工具型看板、側欄控制、資料卡、圖表、表格與 session timeline drawer，但降低裝飾性毛玻璃、霓虹光暈、紫藍漸層與大陰影的使用量。介面應更像可長時間使用的本地分析工具，而不是展示頁或科幻監控牆。

優先重構項目：

1. 建立更克制的深色 surface 階層，讓背景、側欄、主內容、卡片、drawer 與 modal 的層級靠明度、邊界與間距區分。
2. 將 cyan 限定為主要互動、目前選取、focus-visible 與關鍵資料狀態，避免作為泛用裝飾。
3. 移除或壓低漸層文字、過度 glow、寬模糊陰影與純裝飾 pulse，讓資料本身成為視覺焦點。
4. 強化表格、統計卡、時間軸與控制列的一致 spacing、字級、focus、hover、disabled 與 loading 狀態。
5. 修正 mobile / tablet 結構，確保側欄、header controls、統計卡、表格與 drawer 在窄螢幕下不重疊、不溢出。
6. 補上 `prefers-reduced-motion` 版本，將動畫限制在狀態轉換與操作回饋，不做頁面載入表演。

## Accessibility & Inclusion

以 WCAG AA 作為最低目標：一般文字對比至少 4.5:1，大型文字至少 3:1。互動元件需要清楚的 hover、focus-visible、active、disabled 與 loading 狀態。狀態不可只靠顏色傳達，應搭配文字、圖示或結構差異。

動效應尊重 `prefers-reduced-motion`，自動刷新、抽屜、modal、時間軸展開收合與 loading 動畫都應提供減少動作版本。長路徑、session id、模型名稱與雙語文案必須在窄螢幕下可換行、截斷或水平捲動，不得溢出容器。
