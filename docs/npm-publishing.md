# npm 首次上架與 Trusted Publishing 設定

本文件供 `token-usage-insights` 維護者使用。一般使用者只需執行：

```sh
npx --yes token-usage-insights
```

npm 套件本身只包含 JavaScript 啟動器。第一次執行時會從同版本的 GitHub Release 下載目前平台的完整壓縮包，讀取 `SHA256SUMS` 驗證檔案，再安裝原生執行檔、`static`、`shell`、`scripts` 與 `pricing.csv`。後續執行會重用 npm 快取內已驗證的檔案。

此流程不使用 `postinstall`。npm 12 預設不執行相依套件的安裝生命週期腳本，但一般使用者仍可直接執行 `npx --yes token-usage-insights`，不需加入 `--allow-scripts`。

* * *

## 必要條件

- npm 套件名稱：`token-usage-insights`
- npm Registry：`https://registry.npmjs.org/`
- GitHub 擁有者：`doggy8088`
- GitHub Repository：`TokenUsageInsights`
- Trusted Publisher workflow 檔名：`release.yml`
- GitHub Environment：`npm`
- GitHub Actions 啟用變數：`NPM_TRUSTED_PUBLISHING_ENABLED=true`
- 使用 GitHub-hosted `ubuntu-latest` runner
- 發布工作使用 Node.js 24 與最新版 npm
- 一般使用者執行 npx 的最低 Node.js 版本：18.18

npm Trusted Publishing 目前要求 npm CLI 11.5.1 以上及 Node.js 22.14.0 以上；若要使用本文件的 `npm trust` 指令，npm CLI 必須為 11.15.0 以上。專案的 GitHub Actions 固定使用 Node.js 24 並在發布前更新 npm，符合此要求。npm 帳號必須先啟用雙因素驗證。

官方文件：

- [npm Trusted Publishing](https://docs.npmjs.com/trusted-publishers/)
- [npm trust 命令](https://docs.npmjs.com/cli/v11/commands/npm-trust/)
- [npm Provenance](https://docs.npmjs.com/generating-provenance-statements/)
- [npm 12 安裝階段安全預設](https://github.blog/changelog/2026-07-08-npm-install-time-security-and-gat-bypass2fa-deprecation/)
- [GitHub Actions OIDC](https://docs.github.com/en/actions/reference/security/oidc)

* * *

## 為何首次發布需要人工操作

npm 規定套件必須先存在於 Registry，才能替它建立 Trusted Publisher。因此流程分成兩階段：

1. 第一個版本由套件擁有者在本機登入 npm，手動執行一次 `npm publish`。
2. 套件存在後，設定 npm 與 GitHub 的 OIDC 信任關係；後續版本由 GitHub Actions 發布，不再使用長效 npm Token。

Release workflow 的 `publish-npm` job 受 `NPM_TRUSTED_PUBLISHING_ENABLED` 控制。變數尚未設為 `true` 時，GitHub Release 仍會正常建立，但 npm 發布 job 會跳過，避免首次發布因信任關係尚未建立而失敗。

* * *

## 首次上傳套件

### 1. 確認 npm 套件名稱仍可使用

```sh
npm view token-usage-insights name version
```

若回傳 `E404`，表示 Registry 尚無此套件；仍應立即完成後續上架，因為未使用的名稱可能被其他人註冊。若已出現其他擁有者的套件，停止發布並重新決定名稱。

### 2. 建立對應的正式 GitHub Release

先依專案的 Release 規範選定新版本、更新版本檔、提交、推送 `main`，再建立並推送附註 Git tag。版本必須在以下位置一致：

- `Cargo.toml`
- `Cargo.lock`
- `package.json`
- `package-lock.json`
- README 的固定版本安裝範例
- `CHANGELOG.md` 的正式版本標題

推送 `vX.Y.Z` 後，等待 Release workflow 完成，並確認公開 Release 至少包含：

- `token-usage-insights-vX.Y.Z-aarch64-apple-darwin.tar.gz`
- `token-usage-insights-vX.Y.Z-x86_64-apple-darwin.tar.gz`
- `token-usage-insights-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz`
- `token-usage-insights-vX.Y.Z-x86_64-pc-windows-msvc.zip`
- `SHA256SUMS`

首次執行時，`publish-npm` job 應顯示為 skipped，這是預期結果。

### 3. 在維護者電腦登入 npm

從 Repository 根目錄執行：

```sh
npm install --global npm@latest
npm login
npm whoami
```

`npm login` 會開啟瀏覽器。登入擁有 `token-usage-insights` 的 npm 帳號並完成雙因素驗證。`npm whoami` 必須顯示正確的 npm 使用者名稱。

### 4. 確認目前程式碼就是要發布的 Release

```sh
git status --short
git describe --tags --exact-match HEAD
node --version
npm --version
```

必要結果：

- `git status --short` 沒有輸出。
- `git describe` 顯示準備發布的 `vX.Y.Z`。
- Node.js 至少為 22.14.0。
- npm 至少為 11.5.1。

### 5. 安裝開發相依並發布

```sh
npm ci --ignore-scripts
npm test
npm pack --dry-run --ignore-scripts
npm publish --access public
```

`npm publish` 會自動執行 `prepublishOnly`，再次確認 Node 測試、套件內容、Cargo 與 npm 版本一致、目前 Git tag 正確，以及 GitHub Release 的四個壓縮包與 `SHA256SUMS` 都可下載。

首次手動發布不會具有 GitHub Actions OIDC provenance；完成 Trusted Publisher 設定後，後續由 GitHub Actions 發布的版本會自動產生 provenance。

### 6. 驗證首次上架

```sh
npm view token-usage-insights version dist-tags.latest
npx --yes token-usage-insights@X.Y.Z --help
```

`npm view` 應顯示剛發布的版本，`npx` 說明應包含 `export`、`export-all` 與 `import`。

* * *

## 設定 GitHub Environment

1. 開啟 GitHub Repository：`doggy8088/TokenUsageInsights`。
2. 進入 `Settings`。
3. 左側選擇 `Environments`。
4. 選擇 `New environment`。
5. 名稱輸入 `npm`，大小寫必須完全一致。
6. 儲存後，可選擇設定 Required reviewers，讓每次 npm 發布前需要人工核准。
7. 不需要新增 npm Token 或任何 Environment secret。

* * *

## 設定 npm Trusted Publisher

套件首次存在後，登入 [npmjs.com](https://www.npmjs.com/)：

1. 開啟 `token-usage-insights` 套件頁面。
2. 進入 `Settings`。
3. 找到 `Trusted publishing`。
4. 選擇 `GitHub Actions`。
5. 填入下表內容。

| npm 欄位 | 必須填入的值 |
| --- | --- |
| Organization or user | `doggy8088` |
| Repository | `TokenUsageInsights` |
| Workflow filename | `release.yml` |
| Environment name | `npm` |
| Allowed actions | 啟用直接 `npm publish` |

Workflow filename 只填檔名，不要填 `.github/workflows/release.yml`。所有欄位都區分大小寫。npm 儲存設定時不會驗證內容，填錯通常要等到下一次發布才會出現 `ENEEDAUTH`；既有 Trusted Publisher 也不能直接修改，需刪除後重建。

也可以在已登入 npm 且已啟用雙因素驗證的終端機執行：

```sh
npm trust github token-usage-insights \
  --repo doggy8088/TokenUsageInsights \
  --file release.yml \
  --env npm \
  --allow-publish
```

網站與 CLI 二擇一即可，不要建立兩筆相同設定。

* * *

## 啟用 GitHub Actions 自動發布

完成 Trusted Publisher 後設定 Repository variable：

1. 開啟 GitHub Repository 的 `Settings`。
2. 進入 `Secrets and variables` → `Actions`。
3. 切換至 `Variables`。
4. 選擇 `New repository variable`。
5. Name 填入 `NPM_TRUSTED_PUBLISHING_ENABLED`。
6. Value 填入 `true`。
7. 儲存。

不要建立 `NPM_TOKEN`。`release.yml` 的 npm job 使用 `id-token: write` 向 GitHub 取得短效 OIDC Token，再由 npm 驗證 Repository、workflow 與 Environment；長效 Token 不會寫入 GitHub Secrets。

* * *

## 後續版本的自動發布流程

後續每次推送 `vX.Y.Z` tag 時，Release workflow 會依序：

1. 建置與測試四個平台的 Rust 執行檔。
2. 建立完整 GitHub Release 與 `SHA256SUMS`。
3. 等待 `npm` Environment 的核准條件。
4. 將 npm 版本同步為 tag 版本。
5. 驗證 npm 套件、版本及所有 GitHub Release 資產。
6. 透過 Trusted Publishing 執行 `npm publish --access public`。
7. 在乾淨暫存目錄透過 Registry 執行 `npx ... --help`，確認使用者可安裝並啟動。

npm Trusted Publishing 會自動建立 provenance，不需另外使用 npm Token，也不需在發布命令加上 `--provenance`。

* * *

## 常見錯誤

| 錯誤 | 檢查方式 |
| --- | --- |
| `ENEEDAUTH` | 核對 npm Trusted Publisher 的擁有者、Repository、`release.yml`、`npm` Environment 與 Allowed actions |
| Release 資產 404 | 確認 GitHub Release workflow 已完成，版本、tag 與壓縮包檔名一致 |
| `npm 僅能從對應 Release commit 發布` | 切換到正式 Release tag 指向的 commit，再執行首次發布 |
| `版本不一致` | 同步 `Cargo.toml`、`Cargo.lock`、`package.json`、`package-lock.json` |
| npx 下載或校驗失敗 | 確認對應版本的 GitHub Release 與 `SHA256SUMS` 可下載，再重新執行相同 npx 命令 |
| 不支援的平台 | 目前只提供 Windows x64、Linux x64、Intel Mac 與 Apple Silicon Mac |
