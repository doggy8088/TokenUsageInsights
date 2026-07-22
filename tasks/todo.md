# 2026-07-22 修正 Grok 教學翻譯與圖示版型回歸

## Goal and acceptance criteria

- [ ] Grok Build 教學顯示實際中文或英文翻譯文字，不顯示 i18n key。
- [ ] Setup modal 的 body 保留既有 flex 版型，所有助理 logo 與步驟圖示維持正確尺寸。
- [ ] 前端資源版本號更新，避免載入舊版 i18n/CSS 快取。
- [ ] 通過前端語法、Rust 測試、格式與零警告驗證。

## Plan

- [x] 比對 JS、i18n、CSS 與靜態資源快取版本，確認回歸根因。
- [x] 修正翻譯查找、modal body 顯示方式與資源 cache bust。
- [x] 驗證 Grok setup-info 路徑、翻譯 key 對應與 icon 版型規則。
- [x] 完成前端與 Rust 全套驗證並記錄結果。

## Risk and rollback

- Risk: low；僅調整前端翻譯查找、modal 顯示樣式與靜態資源版本號。
- Rollback: 還原 `static/app.js`、`static/index.html`、`static/styles.css` 與本節任務紀錄。

## Working notes

- `static/app.js` 原本仍引用 `i18n.js?v=27`；本次資源版本已加入 Grok 字串，需避免舊快取覆蓋。
- `.modal-body` 的既有 CSS 是 `display: flex`；選取 body 不應以 `display: block` 覆蓋它。

## Results

- `t()` 現在會先尋找正確的 assistant-specific key，再回退到通用 key；完整的 `grok_*` key 不會再被重複加上 `grok_` 前綴。
- Setup modal 選取 body 時恢復原本的 `display: flex`，不再以 `display: block` 破壞 modal 內容版型。
- cache-bust 已更新：`styles.css?v=21`、`redesign.css?v=19`、`i18n.js?v=28`、`app.js?v=42`。
- HTTP smoke 驗證首頁資源版本與 `/api/grok/setup-info` 的 `~/.grok/sessions` 路徑。
- 驗證通過：`node --check static/app.js`、Grok 中英文翻譯 key 檢查、`cargo fmt -- --check`、`cargo test --locked`（53 + 38）、`cargo build --release --locked`、`cargo clippy --locked --all-targets --all-features -- -D warnings`、`git diff --check`。

# 2026-07-10 windows_native_support

# 2026-07-22 重新實作 Grok Build 前置設定教學

## Goal and acceptance criteria

- [x] 切換至 Grok Build 後，空資料卡與前置設定教學都只使用 Grok Build 的文字與 logo。
- [x] Grok Build 教學顯示與 Codex/Claude 相同的自動記錄說明，路徑使用 `~/.grok/sessions`。
- [x] 舊助理的非同步日期、使用量或 setup 回應不得覆蓋目前選取的助理。
- [x] Antigravity/Copilot 的 Status Line 教學維持原本行為。
- [x] 通過前端語法、Rust 測試、格式與零警告驗證。

## Plan

- [x] 恢復本次任務前的乾淨工作樹並重新檢查完整流程。
- [x] 讓助理選取狀態透過 snapshot 傳遞至空資料畫面、setup modal 與相關請求。
- [x] 加入最小必要的 modal body/title 路由修正。
- [x] 驗證 Grok 路徑、logo、教學內容與非同步切換邊界。
- [x] 更新結果與 lessons。

## Risk and rollback

- Risk: medium；調整前端助理切換與資料載入的狀態傳遞。
- Rollback: 還原 `static/app.js`、`static/index.html` 與本節任務紀錄的變更。

## Working notes

- Grok setup data path 的權威來源是 `src/handlers/daily.rs` 的 `get_grok_dir().join("sessions")`。
- `static/index.html` 已有 Grok 專屬 modal body；本次重點是保證正確的 assistant snapshot 路由到該 body。

## Results

- `static/app.js` 現在以 assistant snapshot 路由 setup modal、空資料卡 logo/按鈕與日期/使用量請求。
- Grok setup path 由 `/api/grok/setup-info` 提供，驗證值為 `/Users/sdsg5bpnl/.grok/sessions`。
- 驗證通過：`node --check static/app.js`、前端結構回歸檢查、`cargo fmt -- --check`、`cargo test --locked`（53 + 38）、`cargo clippy --locked --all-targets --all-features -- -D warnings`、`git diff --check`。
- 執行 `cargo run` 時曾遇到 3003 埠被既存服務占用；確認占用者結束後重新啟動成功，API 與靜態版本檢查通過。

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
