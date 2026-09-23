# 2026-09-23 新增 GPT-6 Sol 與 Luna 定價

## Goal and acceptance criteria

- [x] `pricing.csv` 新增 GPT-6 Sol、GPT-6 Luna 的 Global 短／長上下文、預設及 Cursor 規則，採用官方 Standard 與 Batch/Flex 每百萬 Token 費率。
- [x] 回歸測試驗證兩個模型的短／長上下文計價與模型 ID／顯示名稱別名。
- [x] 研究紀錄說明官方來源、272K 門檻、Batch/Flex 折扣及 CSV 未表達的額外費用。
- [x] `CHANGELOG.md` 的未發行區記錄這項可見變更。
- [x] `cargo test --locked`、`cargo fmt -- --check`、Release 全目標 build 與 `git diff --check` 通過；Rust compiler 零警告。
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` 無警告；若既有 lint 阻擋，需在 Results 記錄。
- [x] 依專案規範建立繁體中文 Conventional Commit。

## Plan

- [x] Checkpoint A：查核官方 GPT-6 定價與既有 GPT-6 Astra 資料格式。
- [x] Checkpoint B：確認最小修改範圍及測試案例。
- [x] Checkpoint C：更新價格資料、研究紀錄、回歸測試與變更記錄。
- [x] Checkpoint D：執行 Rust 驗證並檢查最終差異。
- [x] Checkpoint E：提交變更並記錄結果。

## Risk and rollback

- Risk：低；只影響本機 Token 成本估算資料與其回歸測試，不會改動帳務、資料庫結構或外部服務。
- Rollback：以單一提交回復 CSV、研究文件、測試與變更記錄即可。

## Dependencies and environment

- Rust stable 與 Cargo lockfile；CSV 仍使用每百萬 Token 的美元費率。

## Working notes

- 官方標準定價依 Prompt 輸入 Token 是否超過 272K 分級；本專案門檻計算使用輸入加快取讀取 Token。
- Batch/Flex 採 Standard 費率 50%；快取寫入、Fast mode、區域處理加價不屬於目前 CSV 欄位範圍。

## Results

- 新增 GPT-6 Sol、GPT-6 Luna 的 Global 短／長上下文及預設費率，並補上 Cursor Standard 價格；官方 Standard 與 Batch/Flex 數值來源與範圍記於 `docs/research/gpt-6-sol-luna-pricing.md`。
- `pricing::tests::gpt_6_sol_and_luna_context_tiers_use_packaged_pricing` 通過；完整 `cargo test --locked` 通過 376 個主 binary 測試及 2 個 CLI 測試。
- `cargo fmt -- --check`、`cargo build --release --locked --all-targets`、`git diff --check` 通過；Release 建置沒有 Rust 編譯警告。CSV 驗證結果為 217 筆資料、7 欄，無格式錯誤。
- `cargo clippy --locked --all-targets --all-features -- -D warnings` 仍被既有 `clippy::uninlined_format_args` lint 擋下（216 項）；本次 `src/pricing.rs` 的 lint 訊息僅在原有第 361、403 行，新增測試沒有 lint 訊息。未擴大修改不相關程式碼。

# 2026-07-29 Issue #27 動態模型 Token 計價門檻

## Goal and acceptance criteria

- [x] `pricing.csv` 將非 Codex 長上下文模型的門檻修正為 200K，並保留其他模型的實際門檻。
- [x] `src/pricing.rs` 從定價規則解析門檻，不再硬編碼 `272K`。
- [x] 長上下文判斷使用 prompt/input 加快取讀取 Token，不把輸出 Token 誤算進門檻。
- [x] 新增門檻格式、邊界值與規則排序的回歸測試。
- [x] 通過格式、測試、build、Clippy 與 diff 檢查，且無 compiler warnings。

## Plan

- [x] 從 `origin/main` 與舊綜合 commit 比對計價邏輯及 CSV 規則。
- [x] 實作可變 K 值門檻解析與規則選擇。
- [x] 更新模型定價資料並補齊最小回歸測試。
- [x] 執行完整驗證並記錄結果。

## Risk and rollback

- Risk: medium；影響所有使用長上下文分級價格的模型成本估算。
- Rollback: 還原 `src/pricing.rs`、`pricing.csv` 與本節任務紀錄；不涉及資料庫 schema 或資料遷移。

## Working notes

- 目前主線仍以 `272K` 辨識長上下文；#27 branch 不引入 Grok parser 或 UI 變更。

## Results

- `src/pricing.rs` 新增可解析任意 `N k` threshold 的規則選擇，threshold row 優先於 default row；prompt 門檻排除 output 並納入 cache read/Claude cache write。
- `pricing.csv` 將 Gemini 3.1 Pro 與 Claude Opus 4.6 的門檻由 272K 修正為 200K；GPT-5.4/5.5 的 272K 規則保留。
- 驗證通過：`cargo fmt -- --check`、`cargo test --locked`（80 + 53）、`cargo build --release --locked --all-targets`、`cargo clippy --locked --all-targets --all-features -- -D warnings`、`git diff --check`。

# 2026-07-10 windows_native_support

## Goal and acceptance criteria

- [x] Windows 10/11 can build, run, and install without WSL, Git Bash, or Unix-only collector dependencies.
- [x] Drive-letter, UNC, spaces, Unicode, and common profile path prefixes are handled through native path APIs.
- [x] Windows defaults use `%LOCALAPPDATA%` for app data and `%USERPROFILE%`-relative assistant directories.
- [x] Existing Windows databases and separator-specific sync state remain migration-compatible.
- [x] Antigravity and Copilot have native PowerShell collectors with the existing JSONL/delta contract.
- [ ] Verification commands pass and observed results are recorded in the final verification story.

## Plan

- [x] Locate authoritative path, migration, resource, installer, release, collector, API, and UI code.
- [x] Design the smallest cross-platform path/resource layer and backward-compatible migrations.
- [x] Implement backend, installer, collector, setup UI, release CI, and documentation changes.
- [x] Add Rust regression tests and a PowerShell collector smoke test.
- [ ] Run `cargo fmt --check`, `cargo test`, `cargo clippy --all-targets --all-features`, `cargo build --release`, and `scripts/test-windows.ps1`.
- [ ] Run an installed-release HTTP smoke test from a different working directory.

## Risk and rollback

- Risk: medium.
- Affected components: path resolution, SQLite startup migration, resource discovery, setup API/UI, Windows release installation, status-line collection.
- Rollback: revert this change set. Database relocation deletes the source only after copy length verification and destination sync; a failed relocation preserves the source.
- Monitoring signals: startup database-path diagnostics, sync errors, setup-info paths, Windows CI collector/install/API smoke tests.

## Dependencies and environment

- Rust stable with the MSVC toolchain on Windows.
- Visual Studio Build Tools C++ workload for native compilation.
- Windows PowerShell 5.1 or newer for installer and collectors.
- No new Rust or JavaScript dependencies.

## Working notes

- Persisted sync-state keys use `/` even on Windows; migrations recognize historical `/` and `\` values.
- Assistant directory overrides are authoritative even before the directory exists.
- `INSIGHTS_DIR` is created by `get_db_conn`; Windows defaults to `%LOCALAPPDATA%\TokenUsageInsights`.
- Release resources are resolved relative to the executable/project rather than only the process CWD.
- SQLite on UNC paths is parsed correctly but local-disk storage is recommended because SMB locking varies.

## Results

- Added shared native path/resource handling, Windows-safe database migration, PowerShell collectors, installer hardening, native setup commands, and Windows release smoke coverage.
- Verification evidence is the remaining checkpoint and will be reported command-by-command after execution.

# 2026-07-29 Issue #28 Grok Build 支援

## Goal and acceptance criteria

- [x] 支援 Grok Build session logs 的 metadata、usage/context delta、model 與 reasoning effort 解析。
- [x] 將 Grok Build 同步至既有 SQLite、每日/月度/年度統計與成本估算流程，不破壞主線既有 assistant。
- [x] 新增 Grok setup-info、CLI 說明/匯入支援、狀態切換、logo、翻譯與設定教學。
- [x] 保留 Grok provider reported cost，並正確處理 context snapshot 的增量計價。
- [x] 新增 parser/handler 回歸測試，通過前端語法、格式、測試、build、Clippy 與 diff 檢查，且無 compiler warnings。

## Plan

- [x] 從 `origin/main` 與舊綜合 commit 拆出 Grok 相關檔案與依賴邊界。
- [x] 移植 Grok parser、timeline 與 SQLite sync，適配主線現有 schema/migration。
- [x] 接上 handler、CLI、靜態前端與安裝文件，沿用 #27 的動態計價門檻基底。
- [x] 執行完整驗證與 deterministic fixture smoke check。

## Risk and rollback

- Risk: medium；新增本機資料來源、SQLite rows、API assistant type 與前端選項。
- Rollback: 還原 Grok-specific source/parser、schema additions、routes、static assets、scripts/docs 與本節任務紀錄；既有 assistant 資料不應被刪除或重寫。

## Working notes

- Grok logs are read-only source data; derived rows use `assistant_type = 'grok'` and source kinds for usage/context records.
- `origin/main` already contains the current cache-write and model-attribution pricing behavior; this branch must preserve it.
- Provider costs are stored in `usage_entries.reported_cost_usd`; context-only rows use incremental deltas before estimated pricing.
- Transcript access is constrained to `GROK_DIR/sessions/<session_id>/updates.jsonl`; parser migration resets only `grok:` sync state.
- `pricing.csv` inherits #27 的動態 threshold rows，並新增 Grok 4.5 與 Grok Build 0.1 的獨立定價。

## Results

- 新增 `src/grok.rs` parser、Grok timeline reconstruction、SQLite sync、parser migration、`reported_cost_usd` round-trip 與來源標記。
- 接上 daily handler transcript path validation、setup-info、CLI aliases/help、frontend assistant selector/setup modal、雙語翻譯、Grok logo、README 與 CHANGELOG。
- `pricing.csv` 保留 #27 的 Gemini/Claude 200K 門檻、Grok 4.5 定價與 `grok-build-0.1` 的獨立歷史定價，避免兩者互相套用。
- 驗證：`cargo fmt --all -- --check`、`cargo test --locked`（主 binary 83、CLI binary 60）、`cargo build --release --locked --all-targets`、`cargo clippy --locked --all-targets --all-features -- -D warnings`、`node --check static/app.js`、`node --check static/i18n.js`、`git diff --check` 全部通過，無 compiler warnings。

## Review follow-up

- [x] 以官方 Grok Build wire fixture 驗證 `costUsdTicks`、partial/incomplete flags 與多模型 `modelUsage`。
- [x] 驗證 Grok Build 0.1 使用獨立歷史定價且不會套用 Grok 4.5、未知模型、負數/進位 timestamp 與 EOF timeline 行為。
- [x] 完成 Rust、前端語法、Clippy、release build、diff 與 PR mergeability 檢查。

### Review follow-up results

- `cargo test --locked` 通過：主 binary 96 tests、CLI binary 64 tests。
- `cargo fmt -- --check`、`cargo clippy --locked --all-targets --all-features -- -D warnings`、`cargo build --release --locked --all-targets`、`node --check static/app.js`、`node --check static/i18n.js` 與 `git diff --check` 全部通過，零 compiler warnings。
- PR #30 以 PR #29 的已推送 commit 為共同父系；`CHANGELOG.md` 衝突已在 merge commit 中整合，後續修正只保留 Grok/文件與測試差異。

# 2026-07-10 codex_session_count_mismatch

## Goal and acceptance criteria

- [x] Explain, with code and local-data evidence, why the Codex daily metric shows 8 sessions while the session table shows 3.
- [x] Identify the exact counting/filtering rule used by each UI surface.
- [x] Provide a deterministic verification query or command; do not change product behavior without explicit approval.

## Plan

- [x] Define the target flow and identify the two rendered counts.
- [x] Trace both counts through frontend state, API handlers, and SQL aggregation.
- [x] Correlate the 2026-07-10 local Codex files and database rows without exposing transcript content.
- [x] Verify the root cause independently and record results.

## Risk and rollback

- Risk: low; read-only diagnosis.
- Affected components: Codex daily summary and session-list reporting only.
- Rollback: not applicable unless a later fix is requested.

## Working notes

- Target flow: select Codex and 2026/07/10 -> compare the left total-session metric with the right session-table badge and rows.
- The API constructs `summary.total_sessions` and `sessions` from the same session map, so their raw cardinality is identical.
- The frontend table first converts `sessions` into a parent/child forest and counts only the flattened, root-reachable result.
- Codex subagent metadata consistently uses `id` for the rollout UUID and `session_id`/`parent_thread_id` for the parent thread; the parser currently prefers `session_id` over `id`.
- Browser-plugin invocation was blocked because the runtime request lacked required sandbox metadata; localhost API and database checks provided the deterministic repro instead.

## Results

- Reproduced the screenshot state: API summary/raw list contained 8 sessions; 5 had `parent_session_id == session_id`, so the frontend forest retained only 3 roots.
- Audited 45 Codex JSONL rollout files for the date. Across 36 subagent metadata rows, `id` matched the file UUID 36/36, while `session_id` matched `parent_thread_id` 36/36.
- Parser field precedence collapses subagent rollout IDs into their parent ID. Per-file sync then deletes existing rows for that shared ID before inserting the current file, so sibling/parent data can replace one another.
- No product code was changed. Verification used the live daily API, redacted metadata-field correlation, and independent backend/frontend/data audits.

# 2026-07-10 fix_codex_session_identity

## Goal and acceptance criteria

- [x] Codex subagent rollouts use their own metadata `id` while retaining `parent_thread_id` as the parent relation.
- [x] Existing collapsed Codex database rows are removed and all JSONL files are deterministically reparsed once.
- [x] The daily table preserves every unique API session even when legacy or malformed parent links contain self/cyclic references.
- [x] Regression tests fail on the old behavior and pass after the fix.
- [x] For 2026-07-10, the daily summary count, raw API session count, and rendered table count agree.

## Plan

- [x] Checkpoint A: capture failing parser/tree behavior and locate migration/test patterns.
- [x] Checkpoint B: implement parser identity precedence, rebuild migration, and frontend cycle guards.
- [x] Checkpoint C: add regression coverage and run targeted/full verification.
- [x] Checkpoint D: verify the live API/UI outcome and document results.

## Risk and rollback

- Risk: medium; this changes Codex session identity and rebuilds derived local database rows.
- Affected components: Codex JSONL parsing, Codex sync state, derived `usage_entries`, and daily session-tree rendering.
- Source safety: files under `CODEX_DIR` remain read-only; only derived SQLite rows and sync markers are rebuilt.
- Rollback: revert the parser/UI changes and migration marker. The original Codex JSONL files remain the source of truth and can be reparsed.
- Monitoring signals: Codex sync errors, distinct transcript/session counts, self-parent count, and daily summary/table cardinality.

## Dependencies and environment

- No new dependencies.
- Active localhost service may need restart before the new parser migration executes.

## Working notes

- Current sample invariant: subagent `payload.id` matches rollout filename UUID; `payload.session_id`, `forked_from_id`, and `parent_thread_id` identify the parent.
- Subagent rollouts contain a second embedded parent `session_meta`; canonical identity is locked from the first valid metadata event while later events may still enrich non-identity fields.
- Empty/token-less reparses preserve existing rows and do not advance file state; current sources are reconciled by transcript path and canonical session ID.
- Final migration marker is `migration:codex_session_identity_v6` because earlier v4/v5 attempts may have partially executed during live readiness testing.

## Results

- Parser and sync now retain distinct parent/child rollout identities, preserve legacy data safely during empty parses, and rekey Windows path variants without touching source JSONL or unrelated assistants.
- Frontend tree flattening emits every unique session once for valid, self-parent, and cyclic graphs; identifier lookup and HTML interpolation are hardened.
- Regression proof: the two-metadata parser fixture failed before the identity lock and passed afterward; `cargo test` passed 12/12, `cargo fmt -- --check` passed, and Clippy passed for all targets/features.
- Frontend deterministic assertions passed 12/12 across normal trees, self/cycles, duplicate IDs, prototype-key IDs, and escaped rendering.
- Live 2026-07-10 result changed from 9 sessions / 5 self-parent / 9 retained transcripts / 936 rows to 45 / 0 / 45 / 3905; 36 sessions retain valid parents and no parent is missing.
- Live cardinality is `summary=45`, `raw=45`, and `frontend-flat=45`, with zero duplicate flat rows.
- HTTP smoke passed: `/` and `/static/app.js` returned 200, the dashboard shell/title rendered in source, and the served script contains the cycle and identifier-safety fixes.
- Browser-plugin validation was blocked by missing sandbox metadata in the browser runtime; no external-browser fallback was used.
- Pre-migration DB/old binaries and startup logs are retained under `%TEMP%\token-usage-insights-pre-codex-v4-20260710-204706` for rollback.

# 2026-07-10 release_v0.1.2

## Goal and acceptance criteria

- [x] Merge the existing remote v0.1.1, GPT-5.6 pricing, and line-ending commits without rewriting history.
- [x] Bump crate, lockfile, and README release examples consistently to `0.1.2` / `v0.1.2`.
- [x] Pass local release-gating tests, including the native Windows collector smoke.
- [ ] Push `improve` and annotated tag `v0.1.2` without force.
- [ ] Confirm the tag-triggered Release workflow succeeds for all four targets.
- [ ] Confirm GitHub Release `v0.1.2` is published with four archives and `SHA256SUMS`.

## Plan

- [x] Inspect workflow triggers, remote branch divergence, existing tags, and v0.1.1 release state.
- [x] Merge `origin/improve` into local `improve` with an explicit merge commit.
- [x] Update all authoritative version references to 0.1.2.
- [x] Run fmt, locked tests, Clippy, Windows collector smoke, and release build.
- [ ] Commit the release bump and push branch/tag.
- [ ] Monitor CI and validate the published release assets.

## Risk and rollback

- Risk: medium; pushing the tag creates public release artifacts.
- Affected components: crate metadata, release packaging, four platform builds, and GitHub Release.
- Rollback before tag push: revert the version commit locally.
- Rollback after tag push but before publication: delete the remote tag only if the workflow fails before a release is published.
- Published releases are immutable history by default; fix forward with a new patch tag instead of moving `v0.1.2`.
- Monitoring signals: Release workflow job conclusions, artifact count/names, checksum presence, and release draft/prerelease flags.

## Dependencies and environment

- Authenticated GitHub CLI account `doggy8088` with `repo` and `workflow` scopes.
- `origin` points to `doggy8088/TokenUsageInsights` and release triggers on every pushed tag.
- No remote `v0.1.2` tag existed at discovery time.

## Working notes

- `v0.1.1` already existed and its Release workflow completed successfully, so the safe next patch is v0.1.2.
- Local `b5e84a5` and remote commits were merged without conflicts or history rewriting.

## Results

- Local release gates passed: `cargo fmt -- --check`, `cargo test --locked` (12/12), `cargo clippy --locked --all-targets --all-features`, and `scripts/test-windows.ps1`.
- An isolated `%TEMP%` `cargo build --release --locked` produced the 0.1.2 Windows binary (4,463,616 bytes); the verified temporary build tree was removed afterward.
- Pending version commit, push, CI completion, and release asset verification.

## 2026-07-10 release_v0.1.2 發布結果

- [x] `improve` 已推送至 `3854033b08b2146c133f6c46a431e808fe1fdbba`。
- [x] annotated tag `v0.1.2` 已推送，且 peeled commit 與版本提交一致。
- [x] GitHub Actions Release run `29095068751` 全部成功。
- [x] Linux x64、macOS Intel、macOS Apple Silicon、Windows x64 四個建置 job 全部成功。
- [x] Windows 原生 collector 測試與安裝後 HTTP smoke test 均通過。
- [x] GitHub Release `Token 戰情室 v0.1.2` 已正式發布，非草稿且非預覽版。
- [x] 四個平台封裝與 `SHA256SUMS` 共五個資產均存在。
- [x] `SHA256SUMS` 共四筆，逐一涵蓋所有平台封裝。

### Results

- 版本提交：`3854033b08b2146c133f6c46a431e808fe1fdbba`（`release: bump version to 0.1.2`）。
- CI：https://github.com/doggy8088/TokenUsageInsights/actions/runs/29095068751
- Release：https://github.com/doggy8088/TokenUsageInsights/releases/tag/v0.1.2
- 發布時間：`2026-07-10T13:13:54Z`。
- 發布方式：推送 annotated tag `v0.1.2` 觸發既有 CI；未 force push、未改寫既有標籤或歷史。
- 回滾方式：保留既有 `v0.1.0`、`v0.1.1` Release；如需停止採用本版，可回退下載與部署至前一版，不需改寫 Git 歷史。

# 2026-07-29 PR #29/#30 review feedback

## Goal and acceptance criteria

- [x] 完成 PR #29 與 PR #30 維護者及 Copilot 提出的正確性修正。
- [x] PR #29 的 Gemini 3.1 Pro 快取價格與封裝 `pricing.csv` 回歸測試一致，且 contains fallback 會選最具體模型 base。
- [x] PR #30 的 Grok 4.5 動態門檻、官方 `costUsdTicks`、`modelUsage`、多模型歸因、Unknown Model、時間戳與 EOF timeline 行為均有回歸覆蓋。
- [x] PR #30 不將 Grok Build 0.1 誤判為 Grok 4.5，並依官方定價保留獨立歷史費率；README 路徑說明不再造成支援範圍誤解。
- [x] 新 commit 已推送至兩個 PR 的原有 head branch，並完成遠端 mergeability 驗證。

## Plan

- [x] Checkpoint A：取得 PR metadata、review comments、thread 狀態、changed files 與本地分支狀態。
- [x] Checkpoint B：在 PR #29 分支完成定價資料、模型匹配與測試修正。
- [x] Checkpoint C：在 PR #30 分支完成 Grok 解析、模型歸因、時間軸、定價與文件修正。
- [x] Checkpoint D：執行 Rust/JavaScript 驗證、比較兩分支共同檔案並確認 merge base。
- [x] Checkpoint E：建立 Conventional Commit、推送兩個 head branch，記錄遠端驗證結果。

## Risk and rollback

- Risk: medium；影響定價計算、Grok provider cost、模型歸因、時間軸與兩個相互關聯的 PR 分支。
- Rollback: 保留推送前的 remote head SHA；如驗證失敗，停止推送或以明確的新修正提交回復，禁止 force-push 或改寫 PR 歷史。
- Merge safety: PR #30 已將 PR #29 的修正作為共同父系；PR #29 合併後，PR #30 的差異會只保留 Grok/文件/測試修正，避免共同檔案產生重複衝突。

## Dependencies and environment

- Rust stable、Cargo lockfile、Node.js（`node --check`）。
- `origin` 為 `git@github.com:sdsg5bpnl/TokenUsageInsights.git`；本機未安裝 `gh` CLI，review thread 狀態以 GitHub connector 讀取結果為準。
- 不使用真實使用者 session 或資料庫；測試沿用 repository fixture/temp-dir 模式。

## Working notes

- PR #29：修正 Gemini 3.1 Pro cache pricing；contains fallback 以正規化 base 長度選最具體候選，同一 base 再選 threshold 規則；補封裝 CSV 與 `GPT-5.4-mini-picker` regression tests。
- PR #30：補 `costUsdTicks`/flags、headless Token 語意、多模型 SQLite 識別與 timeline 彙整、保留 `modelUsage` 模型歸因與 `grok-build-0.1` 獨立歷史定價、未知模型不可猜成 Grok 4.5，並修正 timestamp normalization 與 EOF incomplete turn 時間。
- PR #30 先在本地合併 PR #29 分支並保留 merge commit，讓 PR #29 的 head 成為 PR #30 的 ancestor；未改寫歷史或 force-push。

## Results

- `cargo test --locked` 通過：主 binary 96 tests、CLI binary 64 tests。
- `cargo fmt -- --check`、`cargo clippy --locked --all-targets --all-features -- -D warnings`、`cargo build --release --locked --all-targets`、`node --check static/app.js`、`node --check static/i18n.js` 與 `git diff --check` 全部通過，零 compiler warnings。
- Commit：PR #29 `3ddc533`；PR #30 `ff08424`、`d0b6052`。
- Remote：PR #29 head `3ddc533ccf8ebe99d5bbdb21a334fdd675fe386c`；PR #30 head `d0b6052de11a4cf15e39fd43c0f5c45abe95f4aa`。
- PR #29 與 PR #30 目前皆為 open、GitHub `mergeable=true`；`git merge-tree --write-tree` 以 PR #29 head 與 PR #30 head 模擬合併成功，且 PR #29 head 是 PR #30 的 ancestor。
- 替代 PR 依官方現行定價頁恢復 Grok Build 0.1 的獨立歷史費率，並保留不誤套 Grok 4.5 的安全判斷。
