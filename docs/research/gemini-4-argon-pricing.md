# Gemini 4 Argon API 定價研究

研究日期：2026-10-08
資料範圍：Google 官方發布文件（The Keyword / Google DeepMind）
觸發原因：Antigravity 使用量記錄出現 `Gemini 4 Argon (Medium)` 模型名稱（session_id=575e5267-0e01-4a17-acb1-3174ccbe5b61 turn_no=1），`pricing.csv` 缺少對應條目，導致「找不到可用的模型價格規則：Gemini 4 Argon (Medium)」錯誤。

* * *

## 結論

**Gemini 4 Argon 的付費標準 API 單價為：輸入每 100 萬 Token 4.00 美元、
快取內容輸入每 100 萬 Token 0.20 美元（較一般輸入單價折減 95%）、輸出每 100 萬 Token 20.00 美元。
輸出單價包含思考 Token。**

**Gemini 4 Argon 的 High、Medium 與 Low 不是 Google 公開的不同模型 ID，
而是同一個 `Gemini 4 Argon` 模型採用不同思考層級（`thinking_level`）。各變體共用相同單位費率。**

不同思考層級的實際總費用仍可能不同，因為思考層級會影響產生的思考 Token 數量；差異來自計費 Token 數量，不是單位費率。

官方來源：

- [Introducing Gemini 4 Argon | The Keyword (Google)](https://blog.google/innovation-and-ai/models-and-research/gemini-models/gemini-4-argon/)
- [隆重介紹 Gemini 4 Argon | Google 台灣官方部落格](https://blog.google/intl/zh-tw/products/explore-get-answers/gemini-4-argon/)
- [Gemini 4 Argon | Google DeepMind](https://deepmind.google/models/gemini/)

* * *

## 官方價格欄位

Gemini 4 Argon 於 2026-09-30 發布，初期採用限時優惠價（Introductory price），並於官方公告註腳明訂優惠期結束後的標準計費（Standard price）。快取輸入詞元（Cached input tokens）價格比一般輸入詞元便宜 95%（即一般輸入單價的 5%）。
單位為美元／100 萬 Token（Global 區域）：

| 服務層級 | 適用期間 | 輸入 | 快取內容輸入（95% 折扣） | 輸出，含思考 Token |
|---|---|---:|---:|---:|
| Standard（優惠價） | 上市初期優惠期間 | 2.00 | 0.10 | 10.00 |
| Standard（標準價） | 優惠期結束後 | 4.00 | 0.20 | 20.00 |
| Batch／Flex（優惠價，50%） | 上市初期優惠期間 | 1.00 | 0.05 | 5.00 |
| Batch／Flex（標準價，50%） | 優惠期結束後 | 2.00 | 0.10 | 10.00 |

* * *

## `pricing.csv` 採用的費率與理由

`pricing.csv` 依循本儲存庫既有慣例（見 Gemini 3.6 Flash、Gemini 3.7 Flash 與 Gemini 3.8 Flash 的定價研究），記錄**官方付費標準（Standard）牌價**，而非限時優惠價：

| 模型名稱 | 部署類型 | 單位 | 輸入 | 快取輸入 | 輸出 | 批次 API |
|---|---|---|---:|---:|---:|---|
| Gemini 4 Argon（含 Medium／High／Low 變體） | Google AI | 1M Tokens | 4.00 | 0.20 | 20.00 | 2.00/0.10/10.00 |

理由：

1. 優惠價（$2 / $10）為上市初期限時促銷，官方公告註腳已明確記載優惠期結束後恢復標準計費（輸入 $4 / 1M Tokens、輸出 $20 / 1M Tokens）。採用標準價可避免優惠期結束後需再次更新資料，且成本估算偏保守。
2. 快取輸入依官方「比一般輸入詞元便宜 95%」規則計算：$4.00 × 5% = $0.20 / 1M Tokens。
3. 與 Gemini 3.6 / 3.7 / 3.8 Flash 條目的命名與思考層級變體結構完全一致，維持表格一致性。
4. 不同思考層級（Medium／High／Low）對應同一個模型與相同單位費率，總費用差異來自實際產生的思考 Token 數量。

* * *

## 資料適用界線

- 官方發布公告未對 Gemini 4 Argon 額外切分長短上下文不同費率，故採單一標準費率。
- 輸出上限擴增至 1,000,000 Token（自先前的 64K Token 提升），以支援長序列深度推理。
