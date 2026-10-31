# 以 Docker 容器分開檢視不同人的用量資料

這個資料夾提供一個最小化的 Linux 容器映像檔，以及一支管理腳本 `tui-docker.sh`。每個容器都有自己的名稱、資料磁碟區與本機埠號，資料只透過 `import` 匯入，不會讀取本機的 `~/.claude`、`~/.codex` 等日誌，也不會和本機原生執行的 Token 戰情室混在一起。

常見用途：

- 主管或團隊負責人分別檢視每位成員以 `export-all` 匯出的資料。
- 在同一台電腦上保留「本機原生看板」與「別人的資料」兩套互不干擾的檢視。
- 拿到一份 SAS URL 後，一行命令就能開出可瀏覽的看板。

## 檔案說明

| 檔案 | 用途 |
| --- | --- |
| `Dockerfile` | 多階段建置：以 `rust:1-alpine` 編譯 musl 靜態執行檔，執行階段只用 `alpine:3.22`，映像檔約 30 MB |
| `Dockerfile.dockerignore` | 白名單式建置內容，只送出 `Cargo.toml`、`Cargo.lock`、`build.rs`、`src/`、`static/`、`pricing.csv` |
| `tui-docker.sh` | 建立容器、匯入資料、開啟瀏覽器、匯出、清除等管理命令 |

## 需求

- Docker Desktop（macOS / Windows）或 Docker Engine（Linux），需支援 BuildKit（Docker 23 以上預設啟用）。
- Bash。macOS 與 Linux 可直接執行；Windows 請在 WSL 或 Git Bash 中執行。
- 首次建置需要從原始碼編譯 Rust，視電腦效能約需 3 到 10 分鐘，之後重複建置會使用快取。Apple Silicon（arm64）與 x86_64 都可建置。

## 快速開始

在專案根目錄執行：

```bash
# 以檔案建立名為 alice 的容器，匯入後自動開啟瀏覽器
docker/tui-docker.sh up alice ~/Downloads/alice-usage.json

# 以 SAS URL 建立名為 bob 的容器（網址請用單引號包住，避免 & 被 shell 解讀）
docker/tui-docker.sh up bob 'https://<account>.blob.core.windows.net/<container>/<檔名>.json?<SAS>'
```

`up` 會依序執行：

1. 映像檔 `token-usage-insights:local` 不存在時自動建置。
2. 建立容器 `tui-<名稱>` 與資料磁碟區 `tui-<名稱>-data`，自動從 3100 開始挑一個沒被占用的本機埠號，且只綁定 `127.0.0.1`。
3. 等待看板啟動完成後，依序匯入指定的檔案或 SAS URL。
4. 印出看板網址並以預設瀏覽器開啟。

完成後終端機會顯示類似：

```text
✅ alice 的看板已就緒：http://localhost:3100
```

## 匯出資料給容器使用

在資料擁有者的電腦上執行（需已安裝 Token 戰情室）：

```bash
# 匯出成檔案，再以任何方式傳給檢視者
token-usage-insights export-all --out alice-usage.json

# 或直接上傳到 Azure Blob Storage，完成後會印出含 SAS 的 import --url 命令
token-usage-insights export-all --out 'https://<account>.blob.core.windows.net/<container>?<SAS>'
```

把上傳後印出的完整網址交給 `up` 或 `import` 即可。SAS URL 的規則與 `token-usage-insights import --url` 相同：

- 必須是 `https://`，且要指向 JSON 檔（包含檔名），不可只到容器層級。
- SAS 需要讀取權限（`sp` 含 `r`）且尚未過期。
- 下載上限 200 MB；超過時請改用檔案匯入。

## 命令一覽

| 命令 | 說明 |
| --- | --- |
| `up <名稱> [來源...] [--port <埠號>] [--no-open] [--rebuild]` | 建立或啟動容器並匯入來源。來源可為檔案路徑或 `https://` 網址，可指定多個；也可用 `--file` / `--url` 明確指定 |
| `import <名稱> <來源...>` | 對執行中的容器追加匯入，完成後重新整理看板即可看到資料 |
| `open <名稱>` | 以預設瀏覽器開啟看板 |
| `url <名稱>` | 只印出看板網址 |
| `export <名稱> [--out <路徑\|SAS URL>]` | 匯出容器內的全部資料，預設輸出 `./<名稱>-export-all.json` |
| `logs <名稱> [-f]` | 檢視容器日誌 |
| `stop <名稱>` | 停止容器，資料保留 |
| `rm <名稱> [--purge]` | 移除容器；加上 `--purge` 才會刪除資料磁碟區 |
| `ls` | 列出所有由本腳本建立的容器、狀態與網址 |
| `build [--no-cache]` | 重新建置映像檔 |

### 常見操作

```bash
# 列出所有人的看板
docker/tui-docker.sh ls

# 之後拿到 alice 新一個月的資料，追加匯入（重複的紀錄會自動去重）
docker/tui-docker.sh import alice ./alice-2026-11.json

# 隔天重新開啟（容器已停止時會自動啟動，埠號維持不變）
docker/tui-docker.sh up alice

# 指定固定埠號
docker/tui-docker.sh up carol ./carol.json --port 3200

# 不再需要時連資料一起刪除
docker/tui-docker.sh rm bob --purge
```

### 更新容器內的程式版本

專案更新後重新建置映像檔，再以新映像檔重建容器。資料存在磁碟區中，`rm`（不加 `--purge`）不會刪除資料：

```bash
docker/tui-docker.sh build
docker/tui-docker.sh rm alice
docker/tui-docker.sh up alice
```

容器移除後重建時會重新挑選埠號；若要沿用原本的網址，請以 `--port` 指定。

## 不使用腳本，直接以 docker 指令操作

```bash
# 建置（建置內容必須是專案根目錄）
docker build -f docker/Dockerfile -t token-usage-insights:local .

# 建立容器
docker run -d --name tui-alice \
  -v tui-alice-data:/data \
  -p 127.0.0.1:3100:3003 \
  token-usage-insights:local

# 匯入檔案：先複製進容器，再執行 import
docker cp alice-usage.json tui-alice:/tmp/alice-usage.json
docker exec tui-alice /app/token-usage-insights import --file /tmp/alice-usage.json

# 匯入 SAS URL
docker exec tui-alice /app/token-usage-insights import --url 'https://...'

# 開啟 http://localhost:3100
```

## 設計說明

- **資料隔離**：容器內設定 `INSIGHTS_DIR=/data`，資料庫存放在每個名稱專屬的具名磁碟區，容器內也沒有任何 AI 工具的日誌目錄，所以看板只會顯示匯入的資料。
- **匯入在主機端協調**：腳本以 `docker exec` 在容器內執行 `import`，不把來源寫進容器設定，因此 SAS URL 不會出現在 `docker inspect`，重新啟動容器也不會重複匯入或因 SAS 過期而啟動失敗。終端機只顯示不含 query string 的網址。
- **只綁定本機**：埠號以 `127.0.0.1:<埠號>:3003` 發布，區域網路上的其他電腦無法連線。
- **停用自我更新**：容器以 `--no-auto-update` 啟動，且映像檔刻意不放 `VERSION` 檔，避免程式把 `/app` 判定為標準安裝目錄。要升級版本請重新建置映像檔。
- **非 root 執行**：程式以 `app` 使用者身分執行，並設定 `TOKEN_USAGE_INSIGHTS_SERVICE=1`，不會嘗試在容器內開啟瀏覽器。
- **HTTPS 憑證**：程式使用 rustls 與內建的 webpki 根憑證，映像檔不需要另外安裝 `ca-certificates`。

## 環境變數

| 變數 | 預設值 | 說明 |
| --- | --- | --- |
| `TUI_IMAGE` | `token-usage-insights:local` | 腳本使用的映像檔名稱 |
| `TUI_PORT_START` | `3100` | 自動挑選本機埠號的起點 |

## 疑難排解

- **`本機埠號 … 已被使用`**：改用其他 `--port`，或省略 `--port` 讓腳本自動挑選。
- **`容器 … 已使用埠號 …`**：埠號在建立容器時就固定了。執行 `rm <名稱>`（資料會保留）後再以新的 `--port` 執行 `up`。
- **SAS URL 匯入失敗**：確認網址包含檔名、SAS 未過期且具讀取權限，錯誤訊息會顯示 Azure 回傳的 HTTP 狀態與錯誤代碼。
- **看板沒有資料**：執行 `logs <名稱>` 檢查是否有錯誤，並確認匯入時輸出的 `imported` 數量不為 0。重複匯入同一份檔案時 `imported` 為 0、`skipped_duplicates` 等於 `total` 是正常的去重結果。
