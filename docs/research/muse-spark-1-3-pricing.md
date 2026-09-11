# Muse Spark 1.3 API 定價研究

研究日期：2026-09-11
資料來源：Meta Muse Code 本地模型型錄（`~/.local/share/muse/model-catalog/6d657461__p746268.json`）
觸發原因：Muse 使用量記錄出現 `muse-spark-1.3-contributor` 模型名稱（session_id=bda9278c-cea8-4033-bd9b-3f6eae98c6b2 turn_no=1），`pricing.csv` 缺少對應條目，導致「找不到可用的模型價格規則」錯誤。

---

## 結論

**Muse Spark 1.3 於 2026-09-02 發布，由 Meta 提供。官方型錄中包含兩種模型變體：**

1. **標準模型（`muse-spark-1.3`）：**
   - 輸入：每 100 萬 Token 1.25 美元
   - 快取輸入（Cached Input）：每 100 萬 Token 0.15 美元
   - 輸出：每 100 萬 Token 4.25 美元
   - 批次 API：N/A
   - 上下文上限：1,007,997 Token；輸出上限：128,000 Token
   - 思考層級變體：minimal、low、medium、high、xhigh、max

2. **貢獻者優惠模型（`muse-spark-1.3-contributor`）：**
   - 輸入：每 100 萬 Token 0.10 美元
   - 快取輸入（Cached Input）：每 100 萬 Token 0.002 美元
   - 輸出：每 100 萬 Token 0.20 美元
   - 批次 API：N/A
   - 上下文上限：1,007,997 Token；輸出上限：128,000 Token
   - 說明：提供折扣費率，但使用內容（含跨 session 訊息）可能用於產品改善。
   - 思考層級變體：minimal、low、medium、high、xhigh

---

## `pricing.csv` 採用的費率

| 模型名稱 | 部署類型 | 單位 | 輸入價格(美金) | 快取輸入價格(美金) | 輸出價格(美金) | 批次 API 價格(美金) |
|---|---|---|---:|---:|---:|---|
| muse-spark-1.3 | Meta | 1M Tokens | 1.25 | 0.15 | 4.25 | N/A |
| muse-spark-1.3-contributor | Meta | 1M Tokens | 0.10 | 0.002 | 0.20 | N/A |

---

## 與 Muse Spark 1.2 的比較

Muse Spark 1.3 的定價結構與先前版本的 Muse Spark 1.2 完全一致：
- Standard 費率皆為：輸入 1.25 / 快取 0.15 / 輸出 4.25 美元。
- Contributor 費率皆為：輸入 0.10 / 快取 0.002 / 輸出 0.20 美元。
- 相同等級的模型標籤經 `pricing.rs` 解析時，精確比對優先套用個別版本規則。
