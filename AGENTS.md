# Repository Guidelines

## Project Structure & Module Organization
`src/` contains the Rust backend: `main.rs` boots the Axum server, `handlers.rs` exposes HTTP endpoints, `db.rs` manages SQLite sync and migrations, and `pricing.rs` / `timeline.rs` handle pricing and session reconstruction. `static/` holds the frontend (`index.html`, `app.js`, `styles.css`) plus image assets. `shell/` contains helper scripts and `systemd` unit templates for the unified dashboard. Runtime pricing data lives in `pricing.csv`.

## Build, Test, and Development Commands
Use `cargo run` to start the local dashboard on `http://localhost:3003`. Use `cargo build --release` for production builds or before installing the `systemd` service. Run `cargo test` to execute the current Rust test suite. Run `cargo fmt` before committing; use `cargo clippy --all-targets --all-features` for an extra lint pass when touching backend logic. For service installs, render the unit file with `sed "s|<PROJECT_DIR>|$PWD|g" shell/token-usage-insights.service`. On Windows, `scripts\build.ps1` runs `cargo test --release` then `cargo build --release --all-targets` and fails the build if the compiler emits any warning (use `-AllowWarnings` only for local iteration, never for a final build).

**Crucial Rule**: Every build (`cargo build`, `cargo build --release`, `cargo test`, and `scripts\build.ps1`) must complete with zero compiler warnings and zero errors, across every bin target (`token-usage-insights` and `token-usage-insights-cli`), before code is considered done. Treat warnings as build failures: fix them at the source (e.g. remove unused imports/`mut`, or add a narrowly-scoped `#[allow(...)]` with a comment explaining why) rather than suppressing them globally or ignoring them.

## Coding Style & Naming Conventions
Follow standard Rust formatting with 4-space indentation and `snake_case` for functions, modules, and variables. Keep route handlers thin and push data access or parsing into dedicated modules under `src/`. In frontend files, keep plain JavaScript readable and use descriptive camelCase names such as `currentAssistant` and `monthlyChartInstance`. Preserve existing bilingual UI text and avoid renaming assistant identifiers like `antigravity`, `copilot`, or `codex`.

## Testing Guidelines
The repository currently uses Rust unit/integration-style tests embedded under `#[cfg(test)]`, notably in `src/handlers.rs`. Add new backend tests close to the code they exercise unless a dedicated `tests/` directory becomes necessary. Prefer deterministic fixtures by pointing `INSIGHTS_DIR` to a temporary folder, matching the existing yearly handler test pattern. Run `cargo test` after any API, database, or parsing change.

## Commit & Pull Request Guidelines
Recent history uses short conventional prefixes such as `feat:`, `fix:`, `style:`, and scoped forms like `feat(web):`. Keep commit subjects imperative and specific. PRs should describe the user-visible change, note any schema or env var impact, and include screenshots for `static/` UI changes. Link related issues when applicable and list the verification commands you ran.
**Crucial Rule**: Do not automatically commit code changes. All code modifications should be left in the working directory (staged or unstaged) for the user to review and commit manually.

## Release & Changelog Guidelines
Every release must update `CHANGELOG.md` in the same release change before creating the version tag. Move the relevant items from the `Unreleased` / `未發行` section into a version heading with the release date, and derive the entries from both the Git log and the actual diff from the previous tag. Record user-visible additions, changes, fixes, removals, security changes, migrations, environment variable changes, and breaking changes when applicable; do not list a version bump by itself as a product change.

GitHub Release notes must include the real changes for that tag range and a link to the full comparison. Auto-generated download or installation boilerplate is not a substitute for release notes. Keep `CHANGELOG.md`, the GitHub Release notes, `Cargo.toml`, `Cargo.lock`, the release workflow-generated package `VERSION` file, and README version examples synchronized before considering a release complete.

### AI-assisted Release Completion Prompt
When an AI agent is authorized to publish a version, the release is not complete when the tag is pushed or the GitHub Actions workflow starts. The agent must follow this completion prompt:

> Wait for the release workflow to finish and verify that it succeeded. Then verify that the corresponding public GitHub Release exists, is neither a draft nor a prerelease, and contains the expected platform assets and `SHA256SUMS`. Inspect the automatically generated Release body and replace or extend it with the real changes derived from both `git log <previous-tag>..<new-tag>` and the actual diff. Write the Release notes in Traditional Chinese using Taiwan terminology and match the established style of the existing releases: start with `## Token 戰情室 vX.Y.Z` and a concise summary, then include only the relevant sections such as `新增與改善`, `變更`, `修正`, `資料影響`, `相容性`, and `完整差異`. Preserve the useful installation commands and checksum reminder. Add a direct `https://github.com/doggy8088/TokenUsageInsights/compare/<previous-tag>...<new-tag>` comparison link. Do not treat version bumps, generated download boilerplate, commit subjects alone, or workflow completion alone as sufficient release notes. Re-open or query the published Release after editing and verify that the final public content, tag, title, assets, and comparison link are correct before reporting the release as complete.

## Security & Configuration Tips
This project is local-first and reads data from `~/.token-usage-insights`, `~/.gemini/antigravity-cli`, `~/.copilot`, `~/.codex`, `~/.claude`, and `~/.cursor` unless overridden by `INSIGHTS_DIR`, `ANTIGRAVITY_DIR`, `COPILOT_DIR`, `CODEX_DIR`, `CLAUDE_DIR`, or `CURSOR_DIR`. Do not commit local database files, session logs, or personal paths captured during testing.
