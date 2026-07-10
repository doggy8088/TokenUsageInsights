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

## Security & Configuration Tips
This project is local-first and reads data from `~/.token-usage-insights`, `~/.gemini/antigravity-cli`, `~/.copilot`, `~/.codex`, `~/.claude`, and `~/.cursor` unless overridden by `INSIGHTS_DIR`, `ANTIGRAVITY_DIR`, `COPILOT_DIR`, `CODEX_DIR`, `CLAUDE_DIR`, or `CURSOR_DIR`. Do not commit local database files, session logs, or personal paths captured during testing.
