# Claude Haiku 5.5 API 定價研究

研究日期：2026-10-08
資料範圍：Anthropic Claude Platform 官方定價與模型文件
觸發原因：Claude Code 使用量記錄出現 `claude-haiku-5-5` 模型名稱（session_id=c3e4ba13-5d3e-43aa-b96b-16d97d7c1224 turn_no=1），`pricing.csv` 缺少對應條目，導致「找不到可用的模型價格規則：claude-haiku-5-5」錯誤。

---

## 結論

**Claude Haiku 5.5（官方模型 ID：`claude-haiku-5-5`）於 2026-10-07 發布，官方 API 定價依 Prompt 上下文長度（以 100,000 Token 為門檻）分為兩個層級：**

1. **短上下文（Prompt ≤ 100K Tokens）：**
   - 輸入（Base Input）：每 100 萬 Token 0.10 美元
   - 快取內容輸入（Cache Read，0.1x 輸入單價）：每 100 萬 Token 0.01 美元
   - 5 分鐘快取寫入（5m Cache Write，1.25x 輸入單價）：每 100 萬 Token 0.125 美元
   - 1 小時快取寫入（1h Cache Write，2.0x 輸入單價）：每 100 萬 Token 0.20 美元
   - 輸出（含 Adaptive Thinking Token）：每 100 萬 Token 0.50 美元
2. **長上下文（Prompt > 100K Tokens）：**
   - 輸入（Base Input）：每 100 萬 Token 0.50 美元
   - 快取內容輸入（Cache Read，0.1x 輸入單價）：每 100 萬 Token 0.05 美元
   - 5 分鐘快取寫入（5m Cache Write，1.25x 輸入單價）：每 100 萬 Token 0.625 美元
   - 1 小時快取寫入（1h Cache Write，2.0x 輸入單價）：每 100 萬 Token 1.00 美元
   - 輸出（含 Adaptive Thinking Token）：每 100 萬 Token 2.50 美元

官方來源：

- [Claude Haiku 5.5 Overview | Claude Platform Docs](https://platform.claude.com/docs/en/models/haiku-5-5/overview)
- [Pricing | Claude Platform Docs](https://platform.claude.com/docs/en/about-claude/pricing)

---

## 官方價格欄位

單位為美元／100 萬 Token（Global 預設路由）：

| 服務層級 | 適用條件 | 輸入 | 5m 快取寫入 | 1h 快取寫入 | 快取讀取 | 輸出 | 批次 API（輸入／輸出） |
|---|---|---:|---:|---:|---:|---:|---|
| Standard（≤ 100K） | Prompt ≤ 100,000 Tokens | 0.10 | 0.125 | 0.20 | 0.01 | 0.50 | 0.05 / 0.25 |
| Long Context（> 100K） | Prompt > 100,000 Tokens | 0.50 | 0.625 | 1.00 | 0.05 | 2.50 | 0.25 / 1.25 |

---

## `pricing.csv` 採用的費率與理由

依循本儲存庫既有 Claude 門檻模型與 Cursor 清單結構（如 `Claude Opus 4.6 (<200k)` / `Claude Opus 4.6 (>200k)`）：

| 模型名稱 | 部署類型 | 單位 | 輸入 | 快取輸入 | 輸出 | 批次 API |
|---|---|---|---:|---:|---:|---|
| Claude Haiku 5.5 (<100k) | Global | 1M Tokens | 0.10 | 0.01 | 0.50 | N/A |
| Claude Haiku 5.5 (>100k) | Global | 1M Tokens | 0.50 | 0.05 | 2.50 | N/A |
| Claude Haiku 5.5 | Global | 1M Tokens | 0.10 | 0.01 | 0.50 | N/A |
| claude-haiku-5-5 | Cursor | 1M Tokens | 0.10 | 0.01 | 0.50 | N/A |

理由：

1. Anthropic 對 Claude Haiku 5.5 採 100,000 Token 門檻的分段定價。`src/pricing.rs` 的 `parse_threshold_rule` 可直接解析 `(<100k)` 與 `(>100k)` 標籤，並將一般輸入、快取讀取與 Claude 快取寫入（5m / 1h）加總為 Prompt Token 後自動選擇對應階梯。
2. Claude 模型的 5 分鐘與 1 小時快取寫入費用在 `src/pricing.rs` 中分別按對應階梯 `input_price` 的 1.25 倍與 2.0 倍自動計算（≤ 100K 為 $0.125 / $0.20，> 100K 為 $0.625 / $1.00），與官方費率完全一致。
3. 同時保留無門檻的 `Claude Haiku 5.5`（Global）與 `claude-haiku-5-5`（Cursor）條目，確保回退比對與前端費率表搜尋一致。

---

## 資料適用界線

- 上下文窗口為 1,000,000 Token；同步 Messages API 輸出上限為 128,000 Token。
- 若透過 `inference_geo: "us"` 指定美國資料落地或使用雲端平台區域端點，會有 1.1x 加成，不納入 Global 標準牌價。
