---
name: bump-and-release
description: 升級 TokenUsageInsights 的 Rust 套件版本並完成正式 GitHub Release。使用於使用者要求 bump patch、minor、major version，或要求建立、發佈、release 新版本時；涵蓋版本檔同步、零警告驗證、正體中文 Conventional Commit、附註 Git tag、推送、GitHub Actions 監看與 Release 成品確認。
---

# Bump and Release

依專案既有慣例完成版本升級與正式發佈。將發佈視為未完成，直到遠端 Release workflow 成功且 GitHub Release 成品可查證。

## 1. 確認發佈範圍

1. 讀取根目錄 `AGENTS.md`、`Cargo.toml`、`Cargo.lock`、README 發佈章節與 `.github/workflows/release.yml`。
2. 執行並檢查：
   - `git status -sb`
   - `git diff`
   - `git branch --show-current`
   - `git remote -v`
   - `git tag --sort=-version:refname`
   - `gh auth status`
3. 僅在工作目錄變更範圍明確時繼續。不得把不相關變更納入 release commit。
4. 確認目前分支追蹤 `origin/main`，並以 `origin` 對應的 GitHub repository 作為 workflow 與 Release 查詢目標；不得因 `gh repo view` 選到 `upstream` 而監看錯誤 repository。
5. 根據使用者指定的 patch、minor 或 major 計算下一個 SemVer。未指定升級類型時，不得猜測；要求使用者明確指定。
6. 確認目標 tag `vX.Y.Z` 在本機與 `origin` 均不存在。若已存在，停止並回報，不得移動、覆寫或強制推送 tag。

## 2. 同步版本

更新以下位置為相同版本：

- `Cargo.toml` 的 package `version`
- `Cargo.lock` 中 `token-usage-insights` package 的 `version`
- `README.md` 內 `TOKEN_USAGE_INSIGHTS_VERSION` 的 Release tag 範例

使用 `rg` 搜尋舊版本，判斷其他命中是否為歷史範例或也應同步。不得機械式取代相依套件版本或歷史 Release 說明。

## 3. 本機驗證

依序執行，任一失敗立即停止發佈並修正來源問題：

```sh
cargo fmt --check
cargo metadata --locked --format-version 1
cargo test --locked
RUSTFLAGS="-D warnings" cargo build --release --locked --all-targets
cargo clippy --all-targets --all-features --locked -- -D warnings
git diff --check
```

必須確認兩個 binary target 均零警告、零錯誤。不得以全域 suppress、忽略輸出或跳過測試完成發佈。

驗證後重新檢查 `git status -sb` 與 `git diff`，確認只有版本與文件變更。

## 4. 建立 release commit

1. 僅暫存確認過的版本檔案，不使用 `git add -A`。
2. 以 `commit_msg_file="$(mktemp -t codex-commit-message)"` 建立每次唯一的 UTF-8 純文字提交訊息檔。
3. 提交訊息遵守 Conventional Commits 1.0.0，格式如下：

```text
release: 升級版本至 vX.Y.Z

說明版本升級目的與本次 Release 的主要修正或功能。

版本與文件：
- 同步更新 Cargo.toml 與 Cargo.lock 的套件版本。
- 更新 README 的指定版本安裝範例。

說明相容性、資料庫結構、環境變數或安裝流程是否有變更。

驗證項目：
- 列出實際執行且通過的完整命令與測試數量。
```

4. 執行 `git diff --cached --check`。
5. 固定使用 `git commit -F "$commit_msg_file"`，不得使用 `git commit -m`。
6. 推送 `main` 至 `origin`，確認遠端接受提交。

## 5. 建立並推送附註標籤

1. 以 `tag_msg_file="$(mktemp -t codex-tag-message)"` 建立唯一的 UTF-8 標籤訊息檔。
2. 標籤訊息使用正體中文，簡述本次 Release 的主要內容。
3. 使用 `git tag -a vX.Y.Z -F "$tag_msg_file"` 建立 annotated tag。
4. 以 `git show --no-patch --format=fuller vX.Y.Z` 核對 tag 指向 release commit。
5. 使用 `git push origin vX.Y.Z` 推送 tag，觸發 `.github/workflows/release.yml`。

不得在 `main` 推送失敗時先推送 tag。不得使用 `--force`。

## 6. 監看正式發佈

1. 使用明確 repository，例如：

```sh
gh run list --repo OWNER/TokenUsageInsights --workflow Release --limit 3
gh run watch RUN_ID --repo OWNER/TokenUsageInsights --interval 10 --exit-status
```

2. 持續監看至 workflow 完成，不得只因 tag 已推送就宣告成功。
3. 確認四個 build jobs 全部成功：
   - `aarch64-apple-darwin`
   - `x86_64-apple-darwin`
   - `x86_64-unknown-linux-gnu`
   - `x86_64-pc-windows-msvc`
4. 確認 `Create GitHub Release` job 成功。
5. 若 workflow 失敗，使用 `gh run view RUN_ID --log-failed` 取得證據並回報。不得刪除或移動既有 tag；若修正需要改變 tag 所指內容，停止並要求使用者決定新版本號。

## 7. 驗證 Release 成品

使用明確 repository 查詢：

```sh
gh release view vX.Y.Z --repo OWNER/TokenUsageInsights \
  --json name,tagName,isDraft,isPrerelease,url,publishedAt,assets
```

確認 Release：

- tag 與版本一致
- 非草稿、非預發佈版本
- 包含 `SHA256SUMS`
- 包含 Apple Silicon macOS、Intel macOS、Linux x86_64、Windows x86_64 共四個壓縮套件

最後確認 `git status -sb` 顯示工作目錄乾淨且 `main` 與 `origin/main` 同步。

## 8. 回報

以正體中文提供可驗證結果：

- 版本、release commit SHA 與 tag
- GitHub Actions workflow 結果
- GitHub Release 連結
- 四平台成品與 `SHA256SUMS` 是否齊全
- 本機驗證摘要
- 工作目錄及遠端同步狀態

僅在上述條件全部成立時使用「已正式發佈」。
