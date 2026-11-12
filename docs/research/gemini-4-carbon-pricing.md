# Gemini 4 Carbon API 定價研究

- 研究日期：2026-11-12

- 資料範圍：Google 官方發布文件（The Keyword、Google AI Studio 定價頁）與本機 `agy models` 輸出

- 觸發原因：Antigravity CLI 已提供 `gemini-4-carbon-medium`、`gemini-4-carbon-high`、`gemini-4-carbon-low` 三個模型，`pricing.csv` 缺少對應條目，使用後會出現「找不到可用的模型價格規則」錯誤而無法估算成本。

* * *

## 結論

**Gemini 4 Carbon 的付費標準 API 單價為：輸入每 100 萬 Token 4.00 美元、快取內容輸入每 100 萬 Token 0.20 美元（較一般輸入單價折減 95%）、輸出每 100 萬 Token 20.00 美元。輸出單價包含思考 Token。**

各項費率與 Gemini 4 Argon 完全相同。

**Gemini 4 Carbon 的 High、Medium 與 Low 不是 Google 公開的不同 API 模型 ID，而是 Antigravity 以同一個 `gemini-4-carbon` 模型搭配不同思考層級提供的選項。各變體共用相同單位費率。**

不同思考層級的實際總費用仍可能不同，差異來自產生的思考 Token 數量，不是單位費率。

官方來源：

- [Introducing Gemini 4 Carbon | The Keyword (Google)](https://blog.google/innovation-and-ai/models-and-research/gemini-models/introducing-gemini-4-carbon/)
- [Gemini 4 Carbon is now available for developers | The Keyword (Google)](https://blog.google/innovation-and-ai/technology/developers-tools/gemini-4-carbon-developers/)
- [Gemini API 定價 | Google AI Studio](https://aistudio.google.com/docs/pricing)

* * *

## 官方價格欄位

Gemini 4 Carbon 於 2026-10-28 發布，上市初期採用與 Gemini 4 Argon 相同的限時優惠價。Google AI Studio 定價頁明訂優惠價適用至 2026-11-30，2026-12-01 起改用標準價。單位為美元／100 萬 Token：

| 服務層級 | 適用期間 | 輸入 | 快取內容輸入（95% 折扣） | 輸出，含思考 Token |
|---|---|---:|---:|---:|
| Standard（優惠價） | 至 2026-11-30 | 2.00 | 0.10 | 10.00 |
| Standard（標準價） | 2026-12-01 起 | 4.00 | 0.20 | 20.00 |
| Batch／Flex（優惠價） | 至 2026-11-30 | 1.00 | 0.05 | 5.00 |
| Batch／Flex（標準價） | 2026-12-01 起 | 2.00 | 0.10 | 10.00 |

* * *

## `pricing.csv` 採用的費率與理由

`pricing.csv` 依循本儲存庫既有慣例（見 Gemini 4 Argon 與 Gemini 3.x Flash 的定價研究），記錄**官方付費標準（Standard）牌價**，而非限時優惠價：

| 模型名稱 | 部署類型 | 單位 | 輸入 | 快取輸入 | 輸出 | 批次 API |
|---|---|---|---:|---:|---:|---|
| Gemini 4 Carbon（含 Medium／High／Low 變體） | Google AI | 1M Tokens | 4.00 | 0.20 | 20.00 | 2.00/0.10/10.00 |

理由：

1. 優惠價只到 2026-11-30，官方已公告之後的標準價。採用標準價可避免優惠期結束後再次修改資料。
2. 快取輸入依官方「比一般輸入便宜 95%」規則計算：4.00 × 5% = 0.20 美元。
3. 與 Gemini 4 Argon 條目的命名與思考層級變體結構一致。

**已知影響**：在 2026-11-30 以前的實際用量，本工具估算的成本約為實際帳單的 2 倍，屬於刻意保守的估算。

* * *

## Antigravity 模型代號對應

以 `agy models` 於 2026-11-12 查證的模型代號，在 `src/antigravity_metadata.rs` 的 `model_label` 明確對應為顯示名稱：

| Antigravity 模型代號 | 顯示名稱 |
|---|---|
| `gemini-4-carbon-medium` | Gemini 4 Carbon (Medium) |
| `gemini-4-carbon-high` | Gemini 4 Carbon (High) |
| `gemini-4-carbon-low` | Gemini 4 Carbon (Low) |

Gemini 4 Argon 另有 `agy-demo-*` 舊代號，Carbon 沒有對應的舊代號，因此不額外加入別名。

* * *

## 資料適用界線

- 官方定價頁未對 Gemini 4 Carbon 切分長短上下文費率，故採單一費率。
- Gemini 4 Carbon 只支援隱含快取（implicit caching），不支援明確快取（explicit caching），因此沒有快取儲存費用；這不影響 `pricing.csv` 的欄位。
- Google Cloud 的 Agent Platform 另有非 global 區域約加價 10% 的費率，本工具不區分區域，沿用 Google AI 的 global 費率。
