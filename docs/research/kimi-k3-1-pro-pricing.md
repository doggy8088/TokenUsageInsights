# Kimi K3.1 Pro 定價查證

查證日期：2026-11-05

來源：

* [Ollama 官方模型頁面](https://ollama.com/library/kimi-k3.1-pro)
* [Ollama 官方定價頁](https://ollama.com/pricing)
* [Kimi 官方定價說明](https://www.kimi.ai/resources/kimi-k3-1-pro-pricing)

* * *

**Ollama 與 Kimi 官方公布的 Kimi K3.1 Pro 費率一致，以每百萬 Token、美元計價。**

| 輸入 | 快取輸入 | 輸出 |
| ---: | ---: | ---: |
| 2.00 | 0.20 | 10.00 |

官方 API 與 Ollama API 使用 `kimi-k3.1-pro` 作為模型名稱，Ollama CLI 範例使用
`kimi-k3.1-pro:cloud`。`pricing.csv` 依既有 Kimi 系列雙格式慣例同時收錄
`Kimi K3.1 Pro` 展示名稱與 `kimi-k3.1-pro` 小寫識別碼，部署類型為 `Global`；
現有名稱比對也可套用至 `:cloud` 名稱。

費率為單一價率，不依上下文長度分層；上下文視窗為 1M tokens（1,048,576）。
官方另有快取寫入費率（5 分鐘 TTL 2.00、1 小時 TTL 4.00），`pricing.csv` 無對應
欄位，故不推定收錄。官方未列出批次 API 費率，因此該欄記為 `N/A`，不推定折扣。

另有獨立模型 `kimi-k3.1-pro-highspeed`（輸入 4.00、快取輸入 0.20、輸出 20.00），
名稱與費率皆不同，不在本次收錄範圍；其名稱包含 `kimi-k3.1-pro`，若後續有使用量
需求應單獨建立規則，避免被標準版規則以包含比對套用而低估一半成本。

以上為 Ollama 雲端與 Kimi API 公布的 Token 費率；Kimi K3.1 Pro 雖開放權重，
自行部署模型的硬體或電力成本不在此估算範圍。
