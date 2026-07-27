# Repository Guidelines

## Project Structure & Module Organization

The Rust 2021 backend lives in `src/`. `main.rs` configures the Axum server, `handlers/` groups daily, monthly, yearly, and miscellaneous API routes, `db.rs` owns SQLite synchronization, and `pricing.rs`, `timeline.rs`, and `paths.rs` contain focused domain logic. The companion CLI is `src/bin/token-usage-insights-cli.rs`. Browser assets are plain HTML, JavaScript, and CSS under `static/`; keep component styles in `static/css/`. Platform installers and smoke tests live in `scripts/`, while status-line collectors and the systemd template live in `shell/`. Update `pricing.csv` when model rates change.

## Build, Test, and Development Commands

- `make run` starts the dashboard at `http://localhost:3003`; override with `make run PORT=3010`.
- `make test` runs the Rust test suite.
- `make lint` formats Rust and runs Clippy across all targets and features.
- `make all` runs formatting, checks, tests, and a release build.
- `cargo run --bin token-usage-insights-cli -- --help` exercises the companion CLI.
- On Windows, `.\scripts\build.ps1` runs locked release tests and builds every target. Use `.\scripts\test-windows.ps1` after collector changes.

CI sets `RUSTFLAGS="-D warnings"`. All binaries, tests, and Clippy checks must finish with zero warnings.

## Coding Style & Naming Conventions

Use standard `rustfmt` output (4-space indentation), `snake_case` for Rust functions/modules, and `PascalCase` for types. Keep route handlers thin; move persistence and parsing into their existing modules. Frontend code uses descriptive `camelCase` names and modular CSS classes. Preserve bilingual UI copy and stable assistant identifiers such as `codex`, `copilot`, and `antigravity`.

## Testing Guidelines

Tests are colocated in `#[cfg(test)]` modules throughout `src/`, including database, route, path, and payload-limit coverage. Name tests after observable behavior in `snake_case`. Use temporary directories and environment overrides instead of real user data. There is no formal coverage threshold; add regression tests for API, import/export, parsing, migration, and database changes.

## Commit & Pull Request Guidelines

History follows Conventional Commits: `feat(web): ...`, `fix(import): ...`, `docs(readme): ...`, and `release: ...`. Keep subjects concise and specific. Pull requests should explain user-visible behavior, note schema or configuration impacts, link related issues, list verification commands, and include screenshots for `static/` changes. Do not automatically commit agent-generated changes; leave them for review.

## Security & Configuration

Never commit generated databases, usage logs, credentials, or personal paths. During tests, isolate data with variables such as `INSIGHTS_DIR`, `CODEX_DIR`, or `CURSOR_STATE_DB`. Keep CORS defaults local unless broader origins are explicitly required.
