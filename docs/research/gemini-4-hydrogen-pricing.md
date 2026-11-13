# Gemini 4 Hydrogen API 定價研究

- 研究日期：2026-11-13

- 資料範圍：Google 官方發布文件（The Keyword、Google AI Studio 定價頁、Google Antigravity 文件）與本機 `agy models` 輸出

- 觸發原因：Google 於 2026-11-12 發布 Gemini 4 Hydrogen，`pricing.csv` 缺少對應條目，使用後會出現「找不到可用的模型價格規則」錯誤而無法估算成本。

* * *

## 結論

**Gemini 4 Hydrogen（`gemini-4-hydrogen`）的付費標準 API 單價為：輸入每 100 萬 Token 1.50 美元、快取內容輸入每 100 萬 Token 0.075 美元（較一般輸入單價折減 95%）、輸出每 100 萬 Token 7.50 美元。輸出單價包含思考 Token。**

與 Gemini 4 Carbon 不同，官方定價頁沒有為 Hydrogen 列出限時優惠價或日後調價，也沒有切分長短上下文費率。

**Gemini 4 Hydrogen 提供 Low、Medium、High 與新增的 Extra（CLI 參數為 `xhigh`）四個思考層級，預設為 High。各層級共用同一個 API 模型與相同單位費率。**

不同思考層級的實際總費用仍可能不同，差異來自產生的思考 Token 數量，不是單位費率。

官方來源：

- [Introducing Gemini 4 Hydrogen | The Keyword (Google)](https://blog.google/innovation-and-ai/models-and-research/gemini-models/introducing-gemini-4-hydrogen/)
- [Gemini 4 Hydrogen: everyday coding and agentic workflows for developers | The Keyword (Google)](https://blog.google/innovation-and-ai/technology/developers-tools/build-with-gemini-4-hydrogen/)
- [Gemini API 定價 | Google AI Studio](https://aistudio.google.com/docs/pricing)
- [Choose a model | Google Antigravity](https://antigravity.google/docs/models/)

* * *

## 官方價格欄位

單位為美元／100 萬 Token：

| 服務層級 | 輸入 | 快取內容輸入 | 輸出，含思考 Token |
|---|---:|---:|---:|
| Standard | 1.50 | 0.075 | 7.50 |
| Batch／Flex | 0.75 | 0.0375 | 3.75 |
| Priority | 2.70 | 0.135 | 13.50 |

免費層級（Free tier）的 Standard 與 Priority 皆免費，但有速率限制。

* * *

## `pricing.csv` 採用的費率與理由

`pricing.csv` 依循本儲存庫既有慣例，記錄**官方付費標準（Standard）牌價**：

| 模型名稱 | 部署類型 | 單位 | 輸入 | 快取輸入 | 輸出 | 批次 API |
|---|---|---|---:|---:|---:|---|
| Gemini 4 Hydrogen（含 Low／Medium／High／Extra 變體） | Google AI | 1M Tokens | 1.50 | 0.075 | 7.50 | 0.75/0.0375/3.75 |

理由：

1. 本工具估算的是付費 API 的等價成本，因此不採免費層級，也不採 Priority 層級。
2. 快取輸入依官方「比一般輸入便宜 95%」規則：1.50 × 5% = 0.075 美元，與定價頁數值一致。
3. 與 Gemini 4 Carbon、Gemini 4 Argon 條目的命名與思考層級變體結構一致；Extra 依 Antigravity 模型選單的顯示名稱命名。

* * *

## Antigravity 模型代號

Antigravity 文件以 `--model gemini-4-hydrogen-xhigh` 為例，但沒有列出完整的模型代號清單。本機 `agy models`（agy 1.5.7）於 2026-11-13 查證時尚未列出 Hydrogen，因此本次**沒有**在 `src/antigravity_metadata.rs` 的 `model_label` 新增明確對應，避免寫入未經查證的代號。

未對應的代號會原樣作為模型名稱，計價時先嘗試完全比對，再退回包含比對，所以 `gemini-4-hydrogen-xhigh` 這類代號仍會套用 `Gemini 4 Hydrogen` 的價格（已由單元測試 `gemini_4_hydrogen_thinking_levels_use_packaged_pricing` 驗證），只是儀表板會顯示原始代號。等 `agy models` 列出 Hydrogen 後，再補上顯示名稱對應。

* * *

## 資料適用界線

- 官方定價頁未對 Gemini 4 Hydrogen 切分長短上下文費率，故採單一費率（上下文長度上限為 100 萬 Token）。
- Gemini 4 Hydrogen 只支援隱含快取（implicit caching），不支援明確快取（explicit caching），因此沒有快取儲存費用；這不影響 `pricing.csv` 的欄位。
- 搜尋建構（Search grounding）費用（每月 5,000 次免費，之後每 1,000 次 14 美元）不屬於 Token 單價，本工具不估算。
