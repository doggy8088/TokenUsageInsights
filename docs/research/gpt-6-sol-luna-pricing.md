# GPT-6 Sol 與 Luna API 定價研究

研究日期：2026-09-23
資料範圍：OpenAI 官方 API 定價與模型文件

## 結論

OpenAI 官方定價依輸入上下文是否超過 272,000 Tokens 分為短、長上下文。標準費率與 Batch/Flex 費率如下，單位均為美元／100 萬 Tokens：

| 模型 | 上下文 | Standard 輸入／快取輸入／輸出 | Batch/Flex 輸入／快取輸入／輸出 |
|---|---|---|---|
| GPT-6 Sol | ≤272K | 2.00／0.20／10.00 | 1.00／0.10／5.00 |
| GPT-6 Sol | >272K | 4.00／0.40／15.00 | 2.00／0.20／7.50 |
| GPT-6 Luna | ≤272K | 0.10／0.01／0.50 | 0.05／0.005／0.25 |
| GPT-6 Luna | >272K | 0.20／0.02／0.75 | 0.10／0.01／0.375 |

官方模型 ID 為 `gpt-6-sol` 與 `gpt-6-luna`。`pricing.csv` 依現有 GPT-6 Astra 格式記錄 Global 的短上下文、長上下文及預設 Standard／Batch 費率，並在 Cursor 區段加入同模型 ID 的 Standard 價格供模型費率表查詢。

## 額外費用與欄位界線

- 快取輸入價格已納入；GPT-6 快取寫入另按未快取輸入價格的 1.25 倍計費，但 `pricing.csv` 沒有獨立快取寫入欄位。
- Batch 與 Flex 均按 Standard 的 50% 計價，因此沿用批次欄位記錄三段費率。
- Fast mode、區域處理加價及工具呼叫費用不屬於目前這份逐 Token 價格表範圍。
- 上下文門檻由現有規則解析器依輸入與快取讀取 Token 總數選擇；超過門檻後整筆依長上下文費率估算。

## 官方來源

- [OpenAI API 定價](https://developers.openai.com/api/docs/pricing)
- [GPT-6 Sol 模型文件](https://developers.openai.com/api/docs/models/gpt-6-sol)
- [GPT-6 Luna 模型文件](https://developers.openai.com/api/docs/models/gpt-6-luna)
