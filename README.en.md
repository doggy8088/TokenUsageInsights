# Token War Room

**Token War Room is a local-first dashboard for AI coding-agent token usage and session reconstruction.** It reads local records from Google Antigravity CLI, GitHub Copilot CLI, GitHub Copilot App, GitHub Copilot Chat (VS Code), Codex Desktop, Codex CLI, Claude Code, Cursor, Grok Build, Pi Coding Agent, OMP, and Muse Code, presenting daily, monthly, and yearly token consumption, cache usage, reasoning tokens, estimated costs, model distribution, project-directory distribution, and complete session timelines in one place.

This project does not call AI provider APIs on your behalf. Its core data sources are local logs, Status Line collector files, and local SQLite.

> System support: native PowerShell on Windows 10/11, macOS, Linux, and WSL.

Language: [繁體中文](README.md) · [简体中文](README.zh-CN.md) · [English](README.en.md) · [日本語](README.ja.md) · [한국어](README.ko.md)

* * *

## Quickest path to get started

### 1. Start or install the dashboard with one command

If Node.js 18.18 or newer is installed, run the dashboard directly without creating a global npm command:

```bash
npx --yes token-usage-insights
```

To install a persistent system command, use the installer for your platform.

Linux / macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.sh | bash && "$HOME/.local/bin/token-usage-insights"
```

Windows PowerShell:

```powershell
irm https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.ps1 | iex; & "$HOME\bin\token-usage-insights.cmd"
```

Both `npx` and the installers download the compiled version for the current platform. Rust, Cargo, WSL, and manual extraction are not required. The dashboard runs locally after the command starts.

Open:

```text
http://localhost:3003
```

### 2. Check whether your tool needs additional setup

| Tool | Additional setup | Default data source | Description |
| --- | --- | --- | --- |
| Google Antigravity CLI | Required | `~/.gemini/antigravity-cli/usage/usage-YYYY-MM-DD.jsonl` | Collects token data through `statusline-token.sh` or the Windows `statusline-token.ps1` |
| GitHub Copilot CLI | Required | `~/.copilot/usage/usage-YYYY-MM-DD.jsonl` | Collects token data through `statusline-token.sh` or the Windows `statusline-token.ps1` |
| GitHub Copilot App | Not required | `~/.copilot/data.db`, `~/.copilot/session-store.db` | The dashboard reads the desktop app's local SQLite databases directly |
| GitHub Copilot Chat (VS Code) | Not required | VS Code `workspaceStorage/chatSessions` | The dashboard scans local chat sessions from VS Code Stable and Insiders directly |
| Codex Desktop / CLI | Not required | `~/.codex/sessions`, `~/.codex/archived_sessions` | The dashboard scans active and archived local Codex sessions directly |
| Claude Code | Not required | `~/.claude/projects` | The dashboard scans local Claude Code project sessions directly |
| Cursor | Not required | `~/.cursor/projects` | The dashboard scans local Cursor transcripts and reads attributable model metadata in read-only mode |
| Grok Build | Not required | `~/.grok/sessions` | The dashboard scans the `updates.jsonl` session streams saved automatically by Grok Build |
| Pi Coding Agent | Not required | `~/.pi/agent/sessions` | The dashboard scans the local session JSONL files saved automatically by Pi Coding Agent |
| OMP | Not required | `~/.omp/agent/sessions` | The dashboard scans the local session JSONL files saved automatically by OMP |
| Muse Code | Not required | `~/.local/share/muse/sessions` | The dashboard scans the local session JSONL files saved automatically by Muse Code |

**If you only use Copilot App, VS Code Copilot, Codex Desktop, Codex CLI, Claude Code, Cursor, Grok Build, Pi Coding Agent, OMP, or Muse Code, run the one-line installation command and open the dashboard.**

### Native Windows usage

The Windows one-line installer creates `%USERPROFILE%\bin\token-usage-insights.cmd`; no Rust MSVC toolchain, Visual Studio Build Tools, WSL, Git Bash, or `jq` is required.

Windows uses the following native paths by default:

| Purpose | Windows default path |
| --- | --- |
| SQLite | `%LOCALAPPDATA%\TokenUsageInsights\token_usage_insights.db` |
| Antigravity | `%USERPROFILE%\.gemini\antigravity-cli` |
| Copilot | `%USERPROFILE%\.copilot` |
| Codex | `%USERPROFILE%\.codex` |
| Claude Code | `%USERPROFILE%\.claude` |
| Cursor | `%USERPROFILE%\.cursor` |
| Grok Build | `%USERPROFILE%\.grok` |
| Pi Coding Agent | `%USERPROFILE%\.pi` |
| OMP | `%USERPROFILE%\.omp` |
| Muse Code | `%USERPROFILE%\.local\share\muse` |

The dashboard's setup guide shows PowerShell copy, configuration, and diagnostic commands on Windows. The PowerShell collector uses .NET JSON and file APIs and does not depend on Bash, `jq`, `sed`, or `awk`.

Drive letters, paths containing spaces or non-ASCII characters, and UNC paths are handled by native path APIs. Keeping the SQLite database on a local disk is still recommended to avoid differences in network-share locking semantics.

* * *

## Features

### Data analysis

- Daily, monthly, and yearly token statistics
- Breakdown of input, output, cache read, cache write, and reasoning tokens
- Local cost estimates based on `pricing.csv`
- Session count, request count, and API duration statistics
- Model usage rankings
- Cursor sessions can be attributed to specific models from local `state.vscdb` `agentKv` records; unmatched sessions remain `Unknown Model`
- Project working-directory statistics
- Sortable session list
- Automatically reads GitHub Copilot App (desktop app) `~/.copilot/data.db` and `session-store.db`

### Session reconstruction

- Session timeline in a right-side drawer
- User prompts, assistant replies, reasoning content, and tool-call steps
- Tool-call arguments, exit codes, stdout, and stderr
- Codex subagent fields such as parent session, agent nickname, and agent role
- Markdown response rendering and content sanitization

### Interface

- Switch between nine coding-agent badges
- Daily, monthly, and yearly views
- Quick date, month, and year switching
- Automatic live refresh every 5, 10, or 30 seconds
- Manually sync local logs to SQLite
- Dark and light themes
- Traditional Chinese, Simplified Chinese, English, Japanese, and Korean interface
- Model pricing table viewer

* * *

## URL parameters (deep links)

The dashboard supports URL query parameters for opening a specific state directly, making it easy to bookmark, share links, or jump in from other tools. When you switch agent, view, date, working directory, or chart type on the dashboard, the URL is automatically updated to reflect the current state.

| Parameter | Applies to | Values | Description |
| --- | --- | --- | --- |
| `agent` | All views | `antigravity`, `copilot`, `codex`, `claude`, `cursor`, `grok`, `pi`, `omp`, `muse` | Selects the coding agent to display. Aliases such as `claude-code`, `grok-build`, `pi-coding-agent`, `oh-my-pi`, and `muse-code` are also supported |
| `tab` | All views | `daily`, `monthly`, `yearly` | Selects the daily, monthly, or yearly view |
| `date` | All views | `daily`: `YYYY-MM-DD`; `monthly`: `YYYY-MM`; `yearly`: `YYYY` | Selects the date, month, or year to display; the format follows `tab` automatically |
| `dir` | `daily` | Full path, `~`-prefixed home path, or a unique path suffix (e.g. `TokenUsageInsights`) | Filters the daily view by working directory. Windows paths are case-insensitive; if no directory matches, all directories are shown |
| `chart` | `daily` | `kline`, `trend` | Selects the daily chart type: candlestick (K-line) or trend chart |

Examples (`http://localhost:3003` is the default URL; adjust to your actual `HOST`/`PORT`):

```text
http://localhost:3003/?agent=copilot&tab=monthly&date=2026-08
http://localhost:3003/?agent=codex&tab=yearly&date=2026
http://localhost:3003/?agent=claude&tab=daily&date=2026-08-09&chart=trend
http://localhost:3003/?agent=copilot&tab=daily&date=2026-08-09&dir=~/projects/TokenUsageInsights
```

> URL-encode paths that contain `~`, spaces, or non-ASCII characters (`~` can be encoded as `%7E`). Parameters that are not provided fall back to the state from your last visit (Cookie / localStorage).

* * *

## Google Antigravity CLI setup

Antigravity CLI requires connecting this project's Status Line script to `settings.json`. The script writes cumulative and incremental tokens after each conversation to:

```text
~/.gemini/antigravity-cli/usage/usage-YYYY-MM-DD.jsonl
```

### 1. Install the collector script

After the one-line installation, run:

```bash
mkdir -p ~/.gemini/antigravity-cli && cp ~/.local/share/token-usage-insights/shell/antigravity/statusline-token.sh ~/.gemini/antigravity-cli/statusline-token.sh && chmod +x ~/.gemini/antigravity-cli/statusline-token.sh
```

If you use a custom installation location, replace `~/.local/share/token-usage-insights` in the command with the location specified by `TOKEN_USAGE_INSIGHTS_INSTALL_DIR`.

### 2. Configure `~/.gemini/antigravity-cli/settings.json`

If the file does not exist, you can create it with the following content. If it already exists, merge only the `statusLine` block; do not overwrite the existing settings.

```json
{
  "statusLine": {
    "type": "command",
    "command": "/ABSOLUTE/HOME/.gemini/antigravity-cli/statusline-token.sh",
    "padding": 1
  }
}
```

Replace `/ABSOLUTE/HOME` with the actual home-directory path shown by `echo $HOME`, such as `/Users/will` or `/home/will`.

### 3. Verify

```bash
echo '{}' | ~/.gemini/antigravity-cli/statusline-token.sh
jq . ~/.gemini/antigravity-cli/settings.json
```

Afterward, re-enter an Antigravity CLI session. The status line will output a format similar to:

```text
model-name • #3 • input 12.3k • cache 4.5k/0 • output 1.2k • reasoning 500 • total 18.5k
```

* * *

## GitHub Copilot CLI setup

Like Antigravity CLI, Copilot CLI requires connecting this project's Status Line script to `settings.json`. The script writes token data to:

```text
~/.copilot/usage/usage-YYYY-MM-DD.jsonl
```

### 1. Install the collector script

After the one-line installation, run:

```bash
mkdir -p ~/.copilot && cp ~/.local/share/token-usage-insights/shell/copilot/statusline-token.sh ~/.copilot/statusline-token.sh && chmod +x ~/.copilot/statusline-token.sh
```

If you use a custom installation location, replace `~/.local/share/token-usage-insights` in the command with the location specified by `TOKEN_USAGE_INSIGHTS_INSTALL_DIR`.

### 2. Configure `~/.copilot/settings.json`

If the file does not exist, you can create it with the following content. If it already exists, merge only the `statusLine` block; do not overwrite the existing settings.

```json
{
  "statusLine": {
    "type": "command",
    "command": "/ABSOLUTE/HOME/.copilot/statusline-token.sh",
    "padding": 1
  }
}
```

Replace `/ABSOLUTE/HOME` with the actual home-directory path shown by `echo $HOME`.

### 3. Verify

```bash
echo '{}' | ~/.copilot/statusline-token.sh
jq . ~/.copilot/settings.json
```

Afterward, re-enter a Copilot CLI session. The status line will begin outputting and accumulating token data.

* * *

## GitHub Copilot App (desktop app)

**Copilot App (Tauri desktop app) requires no setup.** The dashboard automatically reads local `~/.copilot/data.db` and `~/.copilot/session-store.db`, then combines App session token usage with CLI / VS Code usage on the Copilot page. The session list labels the source as `App`, distinct from `CLI` and `VS Code`.

- During each background sync (every 5 seconds), the dashboard checks both SQLite databases and incrementally syncs with a composite `(created_at, id)` cursor. This avoids duplicate upserts for multiple events with the same timestamp, and the same `(session_id, turn_index)` is never written twice.
- App `assistant_usage_events` has per-API-call granularity. The dashboard aggregates by session, turn, agent, and model, preserves multi-model attribution within the same turn, and uses per-turn statistics for the timeline.
- Session titles come from `data.db.sessions.title`.

If App and CLI use separate directories, or if you use a non-default directory, set the environment variable:

```bash
COPILOT_APP_DIR="/path/to/copilot-app-data" token-usage-insights
```

`COPILOT_APP_DIR` takes precedence over `COPILOT_DIR` and falls back to `~/.copilot` when unset.

* * *

## GitHub Copilot Chat (VS Code) setup

**VS Code Copilot Chat requires no Status Line, hook, or additional collector script.** The dashboard reads chat sessions in local `workspaceStorage` directly and combines them with Copilot CLI data; the session list labels the source as `VS Code` or `CLI`.

VS Code Stable and Insiders are supported:

| Platform | Stable | Insiders |
| --- | --- | --- |
| Windows | `%APPDATA%\Code\User\workspaceStorage` | `%APPDATA%\Code - Insiders\User\workspaceStorage` |
| macOS | `~/Library/Application Support/Code/User/workspaceStorage` | `~/Library/Application Support/Code - Insiders/User/workspaceStorage` |
| Linux | `~/.config/Code/User/workspaceStorage` | `~/.config/Code - Insiders/User/workspaceStorage` |

Usage:

1. Use GitHub Copilot Chat in VS Code to create at least one chat session.
2. Start the dashboard or click the sync button in the upper-right corner.
3. View the combined statistics and session timeline on the Copilot page.

The dashboard fully backfills existing `chatSessions` files and resynchronizes them when file size or modification time changes. Chat sessions without token fields are still shown with a token count of 0. Only local chat files are read; cloud sessions, Remote SSH hosts, and `state.vscdb` are not included.

**Where cache-read tokens come from**: VS Code's `chatSessions` files only persist the `promptTokens` of the last model call in a request plus the accumulated `completionTokens`; they never record prompt-cache reads. The dashboard therefore also reads the Copilot Chat extension debug log written next to them, `GitHub.copilot-chat/debug-logs/<sessionId>/main.jsonl`, sums `inputTokens`, `outputTokens`, and `cachedTokens` across every model call of the turn, and splits the result into non-cached input, cache read, and output tokens so cost estimates use the cache-read rate. The debug log is controlled by the VS Code setting `github.copilot.chat.agentDebugLog.fileLogging.enabled` (already enabled by experiment for some users) and keeps only the 50 most recent sessions by default. Sessions without a debug log fall back to VS Code's own token fields and show 0 cache reads.

If VS Code uses `--user-data-dir` or Portable Mode, specify a custom data root for the dashboard:

macOS / Linux:

```bash
VSCODE_USER_DATA_DIR="/path/to/vscode-user-data" token-usage-insights
```

Windows PowerShell:

```powershell
$env:VSCODE_USER_DATA_DIR = "C:\path\to\vscode-user-data"; & "$HOME\bin\token-usage-insights.cmd"
```

`VSCODE_USER_DATA_DIR` should point to the VS Code user-data directory containing `User/workspaceStorage`. If the environment variable points to the `data` directory in Portable Mode, use `VSCODE_PORTABLE_DATA_DIR` instead; the dashboard checks both `data/user-data/User/workspaceStorage` and `data/User/workspaceStorage`.

* * *

## Codex setup

**Neither Codex Desktop nor Codex CLI requires hooks, a Status Line, or an additional collector script.**

The dashboard scans these directories directly:

```text
~/.codex/sessions
~/.codex/archived_sessions
```

Usage:

1. Use Codex Desktop or Codex CLI normally to create at least one session.
2. Start this project.
3. Select Codex on the left.
4. Click the sync button in the upper-right corner, or wait for background sync.

Notes:

- Codex credentials continue to be managed by Codex itself.
- The dashboard only reads local session records for analysis.
- Each session displays a `Desktop` or `CLI` source label based on the transcript `originator`; old formats that cannot be identified remain uncategorized.
- If API quota information is shown, it comes from the latest local session log, not a real-time online query.

* * *

## Claude Code setup

**Claude Code requires no hooks, a Status Line, or an additional collector script.**

The dashboard scans this directory directly:

```text
~/.claude/projects
```

Usage:

1. Use Claude Code normally to create at least one project session.
2. Start this project.
3. Select Claude Code on the left.
4. Click the sync button in the upper-right corner, or wait for background sync.

Notes:

- Claude Code credentials continue to be managed by Claude Code itself.
- The dashboard only reads local project session records for analysis.
- If `~/.claude/projects` does not exist, the Claude Code page shows no data.

* * *

## Cursor setup

**Cursor requires no hooks, a Status Line, or an additional collector script.** The dashboard scans this directory directly:

```text
~/.cursor/projects
```

Cursor stores conversation and Agent transcripts under its project directories. The dashboard also scans the platform-default `Cursor/User/globalStorage/state.vscdb` in read-only mode and uses its `agentKv` records to attribute actual models; sessions that cannot be matched uniquely remain `Unknown Model`.

Usage:

1. Create at least one conversation or Agent session in Cursor.
2. Start or refresh the dashboard.
3. Select Cursor on the left.
4. Click the sync button in the upper-right corner, or wait for background sync.

Cursor's local data does not contain exact token counts or official billing details, so tokens are estimated from text content. Costs are estimated only when `pricing.csv` contains the matching model and do not represent the official bill. Use `CURSOR_DIR` and `CURSOR_STATE_DB` for non-default locations.

* * *

## Grok Build setup

**Grok Build requires no hooks, a Status Line, or an additional collector script.** The dashboard scans this directory directly:

```text
~/.grok/sessions
```

It uses the Session stream saved internally by Grok Build; it does not read the old-format
`~/.Grok/build/usage/usage-YYYY-MM-DD.jsonl`, nor does it require
`statusLine` in `~/.Grok/build/settings.json`.

Usage:

1. Use Grok Build normally to create at least one session.
2. Start this project.
3. Select Grok Build on the left.
4. Click the sync button in the upper-right corner, or wait for background sync.

A Grok Build session may provide only a context token snapshot, or may also include provider usage and cost. The dashboard prioritizes provider usage/cost; when only a context snapshot is available, cost is estimated using the xAI API prices in `pricing.csv` and the session list labels it `Context`. This does not represent the weekly quota of SuperGrok or other subscription plans.

* * *

## Pi Coding Agent setup

**Pi Coding Agent requires no hooks, a Status Line, or an additional collector script.** The dashboard scans this directory directly:

```text
~/.pi/agent/sessions
```

Pi Coding Agent automatically saves sessions as local JSONL files in a tree-structured directory layout, and the dashboard reads those session records directly.

Usage:

1. Use Pi Coding Agent normally to create at least one session.
2. Start or refresh the dashboard.
3. Select Pi Coding Agent on the left.
4. Click the sync button in the upper-right corner, or wait for background sync.

Pi Coding Agent cost is always read directly from each session's own per-turn `usage.cost` and related usage data. Unlike Grok Build, there is no context-snapshot estimation fallback because Pi natively reports authoritative token and cost usage per turn.

* * *

## OMP setup

**OMP requires no hooks, a Status Line, or an additional collector script.** The dashboard scans this directory directly:

```text
~/.omp/agent/sessions
```

OMP is an open-source fork of Pi Coding Agent (<https://github.com/can1357/oh-my-pi>) and persists sessions using the identical JSONL format. The dashboard reads those local session records directly.

Usage:

1. Use OMP normally to create at least one session.
2. Start or refresh the dashboard.
3. Select OMP on the left.
4. Click the sync button in the upper-right corner, or wait for background sync.

OMP cost is always read directly from each session's own per-turn `usage.cost` and related usage data. Unlike Grok Build, there is no context-snapshot estimation fallback because OMP natively reports authoritative token and cost usage per turn.

* * *

## Muse Code setup

**Muse Code requires no hooks, a Status Line, or an additional collector script.** The dashboard scans this directory directly:

```text
~/.local/share/muse/sessions
```

Muse Code stores sessions as `session.jsonl` files grouped by date and session ID. The dashboard parses model-completion events to separate input, cache-read, output, and reasoning tokens, and reconstructs a timeline of user prompts, tool steps, and Agent replies.

Usage:

1. Use Muse Code normally to create at least one session.
2. Start or refresh the dashboard.
3. Select Muse Code on the left.
4. Click the sync button in the upper-right corner, or wait for background sync.

Muse Code costs are estimated from the model reported by the session and `pricing.csv`. If the data is not in the default location, set `MUSE_DIR` to the Muse Code data directory that contains `sessions`.

* * *

## Local data synchronization

When the service starts, the backend initializes local SQLite and performs an immediate data sync. After startup, it also syncs in the background every 5 seconds.

Default SQLite location:

```text
~/.token-usage-insights/token_usage_insights.db
```

The sync button in the upper-right corner of the frontend calls:

```text
GET /api/:assistant/sync
```

This triggers a full incremental sync of local logs.

## Import / export (cross-machine aggregation)

**For normal use, use the export and import buttons in the upper-right corner of the dashboard.** The installed version needs only a browser to aggregate data across machines and supports import files up to 200 MB.

Starting with v0.9.0, the dashboard and CLI share the `token-usage-insights` executable. Run it without arguments to start the dashboard, or use `export`, `export-all`, or `import`. The main command and subcommands support `--help` and `-h`; older installations need an update or a source build.

`--agent` specifies the assistant (`antigravity` / `copilot` / `codex` / `claude` / `cursor` / `grok` / `pi` / `omp` / `muse`).

### Use the CLI from source

Build it once:

```bash
cargo build --release --bin token-usage-insights
```

```bash
# 匯出日、月或年資料（輸出 JSON，含匯入唯一 id）
./target/release/token-usage-insights export --agent codex --date 2026-07 --out monthly-codex-2026-07.json
```

```bash
# 匯入檔案中的所有資料；每筆資料依 timestamp 決定日期
./target/release/token-usage-insights import --agent codex --file monthly-codex-2026-07.json
```

```bash
# 取得 CLI usage 說明
./target/release/token-usage-insights --help
./target/release/token-usage-insights export --help
./target/release/token-usage-insights import --help
```

The data format matches the frontend and contains these fields:

- `version`
- `assistant`
- `date`
- `exported_at`
- `records` (each record has `import_source_id`)

`import_source_id` forms a unique key together with `assistant_type`. Re-importing the same record is detected as a duplicate and skipped automatically, so it is not written to the database twice.

* * *

## Environment variables

Paths specified by environment variables are authoritative and do not need to be created in advance; `INSIGHTS_DIR` is created automatically at startup. Native absolute/relative paths are supported, as are common forms beginning with `~`, `$HOME`, `%USERPROFILE%`, `%LOCALAPPDATA%`, or `%APPDATA%`.

| Variable | Default | Purpose |
| --- | --- | --- |
| `HOST` | `0.0.0.0` | IPv4 or IPv6 address to which the dashboard service binds |
| `PORT` | `3003` | Dashboard service port |
| `INSIGHTS_DIR` | Windows: `%LOCALAPPDATA%\TokenUsageInsights`; other platforms: `~/.token-usage-insights` | SQLite database directory |
| `ANTIGRAVITY_DIR` | `~/.gemini/antigravity-cli` | Antigravity CLI data directory |
| `COPILOT_DIR` | `~/.copilot` | Copilot CLI data directory |
| `COPILOT_APP_DIR` | Same as `COPILOT_DIR` | Copilot App (desktop app) data directory; should contain `data.db` and `session-store.db` |
| `VSCODE_USER_DATA_DIR` | Auto-detected by platform | VS Code user-data directory; should contain `User/workspaceStorage` |
| `VSCODE_PORTABLE_DATA_DIR` | Not set | VS Code Portable Mode `data` directory |
| `CODEX_DIR` | `~/.codex` | Shared data directory for Codex Desktop and Codex CLI |
| `CLAUDE_DIR` | `~/.claude` | Claude Code data directory |
| `CURSOR_DIR` | `~/.cursor` | Cursor data directory |
| `CURSOR_STATE_DB` | Auto-detected by platform | Cursor `User/globalStorage/state.vscdb` path, used to read `agentKv` model information in read-only mode |
| `GROK_DIR` | `~/.grok` | Grok Build data directory |
| `PI_DIR` | `~/.pi` | Pi Coding Agent data directory |
| `OMP_DIR` | `~/.omp` | OMP data directory |
| `MUSE_DIR` | `~/.local/share/muse` | Muse Code data directory; should contain `sessions` |
| `CORS_ALLOWED_ORIGINS` | `http://localhost:<PORT>,http://127.0.0.1:<PORT>` | Comma-separated allowed CORS origins |

> **The default binding is `0.0.0.0`, so other devices on the same local network may connect to the dashboard. For local-only browsing, set `HOST` to `127.0.0.1`.**

Example:

```bash
HOST="127.0.0.1" INSIGHTS_DIR="/tmp/token-usage-insights" PORT="3010" "$HOME/.local/bin/token-usage-insights"
```

Windows PowerShell example:

```powershell
$env:HOST = '127.0.0.1'; $env:INSIGHTS_DIR = 'D:\Token Usage Insights\資料庫'; $env:CODEX_DIR = "$env:USERPROFILE\.codex"; $env:PORT = '3010'; & "$HOME\bin\token-usage-insights.cmd"
```

* * *

## Background service

### Linux: install and enable the systemd user service with one command

```bash
curl -fsSL https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.sh | bash -s -- --service
```

This downloads the installed version and immediately enables `token-usage-insights.service`; you do not need to build or edit a systemd file yourself.

### macOS: install and enable the launchd LaunchAgent with one command

```bash
curl -fsSL https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.sh | bash -s -- --service
```

This installs `com.tokenusageinsights.plist` into `~/Library/LaunchAgents/` and loads it immediately; stdout and stderr logs are located in `~/Library/Logs/`.

### Windows: install and enable background service with one command (Task Scheduler)

```powershell
& ([scriptblock]::Create((irm https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.ps1))) -Service
```

This registers the `TokenUsageInsights_<username>` task in Windows Task Scheduler for the current user and starts it immediately; it starts automatically at user logon in the background, with logs in the installation `logs\` directory (default `%LOCALAPPDATA%\TokenUsageInsights\logs\`).

### Manage the service

Linux:

```bash
systemctl --user status token-usage-insights.service
journalctl --user -u token-usage-insights.service -n 50 -f
systemctl --user restart token-usage-insights.service
systemctl --user stop token-usage-insights.service
```

macOS:

```bash
launchctl print gui/$(id -u)/com.tokenusageinsights
launchctl kickstart -k gui/$(id -u)/com.tokenusageinsights
launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.tokenusageinsights.plist
```

Windows PowerShell:

```powershell
# Resolve installation directory (defaults to %LOCALAPPDATA%\TokenUsageInsights, or dynamically resolved from task/shortcut)
$TaskName = if ($env:USERNAME) { "TokenUsageInsights_$env:USERNAME" } else { "TokenUsageInsights" }
$InstallDir = $null
$Task = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
if (-not $Task) {
    $Task = Get-ScheduledTask -TaskName "TokenUsageInsights" -ErrorAction SilentlyContinue
    if ($Task) {
        $TaskName = "TokenUsageInsights"
    }
}
if ($Task -and $Task.Actions) {
    foreach ($Action in @($Task.Actions)) {
        if ($Action.Arguments -match '(?i)-InstallDir(?:\s+|:)(?:"([^"]+)"|(\S+))') {
            $DetectedInstallDir = if ($Matches[1]) { $Matches[1] } else { $Matches[2] }
            $InstallDir = [Environment]::ExpandEnvironmentVariables($DetectedInstallDir)
            break
        } elseif ($Action.WorkingDirectory) {
            $InstallDir = $Action.WorkingDirectory
            break
        }
    }
}
$StartupShortcut = Join-Path ([Environment]::GetFolderPath('Startup')) "token-usage-insights.lnk"
if (!(Test-Path $StartupShortcut)) {
    $StartupShortcut = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Startup\token-usage-insights.lnk"
}
if (-not $InstallDir -and (Test-Path $StartupShortcut)) {
    $WshShell = New-Object -ComObject WScript.Shell
    $Shortcut = $WshShell.CreateShortcut($StartupShortcut)
    if ($Shortcut.Arguments -match '(?i)-InstallDir(?:\s+|:)(?:"([^"]+)"|(\S+))') {
        $DetectedInstallDir = if ($Matches[1]) { $Matches[1] } else { $Matches[2] }
        $InstallDir = [Environment]::ExpandEnvironmentVariables($DetectedInstallDir)
    } elseif ($Shortcut.WorkingDirectory) {
        $InstallDir = $Shortcut.WorkingDirectory
    }
}
if (-not $InstallDir) {
    $InstallDir = Join-Path $env:LOCALAPPDATA "TokenUsageInsights"
}

# Check service status (Task Scheduler or background process)
Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object {
    ($_.CommandLine -like "*run-service.ps1*" -and $_.CommandLine -like "*$InstallDir*") -or
    ($_.ExecutablePath -and $_.ExecutablePath -like "$InstallDir*")
} | Select-Object ProcessId, Name, CommandLine

# View logs
Get-Content (Join-Path $InstallDir "logs\token-usage-insights.out.log") -Tail 50 -Wait

# Restart service (scoped to this install directory; compatible with Task Scheduler and Startup folder modes)
Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object {
    ($_.CommandLine -like "*run-service.ps1*" -and $_.CommandLine -like "*$InstallDir*") -or
    ($_.ExecutablePath -and $_.ExecutablePath -like "$InstallDir*")
} | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
if (Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue) {
    Start-ScheduledTask -TaskName $TaskName
} elseif (Test-Path $StartupShortcut) {
    Start-Process $StartupShortcut
}

# Stop service (scoped to this install directory)
Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object {
    ($_.CommandLine -like "*run-service.ps1*" -and $_.CommandLine -like "*$InstallDir*") -or
    ($_.ExecutablePath -and $_.ExecutablePath -like "$InstallDir*")
} | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }

# Unregister service
Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
Unregister-ScheduledTask -TaskName "TokenUsageInsights" -Confirm:$false -ErrorAction SilentlyContinue
if (Test-Path $StartupShortcut) {
    Remove-Item $StartupShortcut -Force -ErrorAction SilentlyContinue
}
Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object {
    ($_.CommandLine -like "*run-service.ps1*" -and $_.CommandLine -like "*$InstallDir*") -or
    ($_.ExecutablePath -and $_.ExecutablePath -like "$InstallDir*")
} | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
```

* * *

## Installation options and manual installation

GitHub Releases provide compiled executables for Linux, macOS, and Windows. Rust or Cargo is not required for installation or execution.

### Optional one-line installer parameters

`scripts/get.sh` (Linux / macOS) and `scripts/get.ps1` (Windows) automatically detect the platform and CPU architecture, download the matching archive from the latest (or specified) Release, extract it, and call the packaged `install.sh` / `install.ps1`; no manual download or extraction is required:

Linux / macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.sh | bash
```

To install and enable the background service at the same time (systemd on Linux; launchd on macOS):

```bash
curl -fsSL https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.sh | bash -s -- --service
```

Windows PowerShell:

```powershell
irm https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.ps1 | iex
```

To install and enable the background service at the same time on Windows:

```powershell
& ([scriptblock]::Create((irm https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.ps1))) -Service
```

After installation, run (on Linux/macOS, confirm that `bin_dir` is on `PATH`; Windows creates a `.cmd` shim):

```bash
token-usage-insights
```

Environment variables can control the version and installation paths (all optional):

| Variable | Platforms | Description |
| --- | --- | --- |
| `TOKEN_USAGE_INSIGHTS_VERSION` | Linux / macOS / Windows | Release tag to install, such as `v0.9.3`; defaults to `latest` |
| `TOKEN_USAGE_INSIGHTS_INSTALL_DIR` | Linux / macOS | Installation directory, passed to `install.sh` |
| `TOKEN_USAGE_INSIGHTS_BIN_DIR` | Linux / macOS | Executable-link directory, passed to `install.sh` |

To customize the installation location, bin directory, and port on Windows, first download the script and then run it with parameters (`iex` pipelines do not support parameters):

```powershell
Invoke-WebRequest -Uri https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.ps1 -OutFile get.ps1
.\get.ps1 -InstallDir 'D:\Apps\Token Usage Insights' -Port 3010
```

### Manual download and installation

If you do not want to execute a remote script directly, download the archive for your platform manually and run the installation script included in the package. Each Release archive contains:

- A single-platform executable
- Frontend assets in `static/`
- The model pricing table `pricing.csv`
- Status Line and service scripts in `shell/`
- The `scripts/` directory (including `install.sh`, `install.ps1`, `get.sh`, `get.ps1`, and `run-service.ps1`)
- README, LICENSE, and VERSION

Linux or macOS:

```bash
tar -xzf token-usage-insights-<tag>-<target>.tar.gz
cd token-usage-insights-<tag>-<target>
./install.sh
```

To install and enable the background service (systemd on Linux; launchd on macOS):

```bash
./install.sh --service
```

Windows:

```powershell
Expand-Archive token-usage-insights-<tag>-x86_64-pc-windows-msvc.zip
cd token-usage-insights-<tag>-x86_64-pc-windows-msvc
powershell -ExecutionPolicy Bypass -File .\install.ps1
```

To install and enable the background service on Windows:

```powershell
powershell -ExecutionPolicy Bypass -File .\install.ps1 -Service
```

Custom Windows installation location and port:

```powershell
.\install.ps1 -InstallDir 'D:\Apps\Token Usage Insights' -BinDir "$HOME\bin" -Port 3010
```

### CI verification

The `Release` workflow runs the corresponding installation script (`install.sh` / `install.ps1`) on Linux, macOS, and Windows for every build, then starts the executable and verifies that:

- The service responds to `/api/<assistant>/pricing` on the specified port
- The response actually loads the `pricing.csv` packaged with the application
- A new `INSIGHTS_DIR` is created and an SQLite database is generated

`get.sh` and `get.ps1` also undergo syntax checks (`bash -n` and PowerShell AST parsing) before every build, ensuring the version published to the Release can run correctly.

### Maintainer release

After pushing a Git tag, GitHub Actions automatically creates the corresponding Release:

```bash
git tag vX.Y.Z
git push origin vX.Y.Z
```

* * *

## Legacy data migration

If you previously used any of the following standalone projects, this project automatically attempts to migrate the old SQLite data at startup:

- `~/.gemini/antigravity-cli/antigravity_cli_token_insights.db`
- `~/.copilot/copilot_cli_token_insights.db`
- `~/.codex/codex_cli_token_insights.db`

After a successful migration, the old database is renamed with a `.bak` suffix.

Once you have confirmed that migration is complete, you can disable the old services:

```bash
systemctl --user stop copilot-cli-token-insights.service
systemctl --user disable copilot-cli-token-insights.service
systemctl --user stop antigravity-cli-token-insights.service
systemctl --user disable antigravity-cli-token-insights.service
systemctl --user stop codex-cli-token-insights.service
systemctl --user disable codex-cli-token-insights.service

rm -f ~/.config/systemd/user/copilot-cli-token-insights.service
rm -f ~/.config/systemd/user/antigravity-cli-token-insights.service
rm -f ~/.config/systemd/user/codex-cli-token-insights.service

systemctl --user daemon-reload
systemctl --user reset-failed
```

* * *

## Troubleshooting

### Dashboard has no data

Check whether the data source exists for each tool:

```bash
ls ~/.gemini/antigravity-cli/usage
ls ~/.copilot/usage
ls ~/.copilot/data.db ~/.copilot/session-store.db
ls ~/.codex/sessions
ls ~/.codex/archived_sessions
ls ~/.claude/projects
ls ~/.cursor/projects
ls ~/.grok/sessions
ls ~/.pi/agent/sessions
ls ~/.omp/agent/sessions
ls ~/.local/share/muse/sessions
```

Antigravity CLI and Copilot CLI also require `settings.json` to define `statusLine` and the scripts to have execute permission.

On Windows PowerShell, inspect the native data directories directly:

```powershell
Get-ChildItem "$env:USERPROFILE\.gemini\antigravity-cli\usage"
Get-ChildItem "$env:USERPROFILE\.copilot\usage"
Get-ChildItem "$env:USERPROFILE\.copilot\data.db", "$env:USERPROFILE\.copilot\session-store.db"
Get-ChildItem "$env:USERPROFILE\.codex\sessions"
Get-ChildItem "$env:USERPROFILE\.codex\archived_sessions"
Get-ChildItem "$env:USERPROFILE\.claude\projects"
Get-ChildItem "$env:USERPROFILE\.cursor\projects"
Get-ChildItem "$env:USERPROFILE\.grok\sessions"
Get-ChildItem "$env:USERPROFILE\.pi\agent\sessions"
Get-ChildItem "$env:USERPROFILE\.omp\agent\sessions"
Get-ChildItem "$env:USERPROFILE\.local\share\muse\sessions"
```

### Status Line script cannot run

```bash
command -v jq
chmod +x ~/.gemini/antigravity-cli/statusline-token.sh
chmod +x ~/.copilot/statusline-token.sh
```

The Status Line scripts depend on `jq` to parse the JSON passed by the CLI.

The `jq` requirement above applies only to `.sh` collectors. You can test the Windows `.ps1` collector with the following command; it natively handles backslashes and paths containing spaces:

```powershell
Write-Output '{}' | powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$env:USERPROFILE\.gemini\antigravity-cli\statusline-token.ps1" -Assistant antigravity
```

### Configuration file has invalid JSON

```bash
jq . ~/.gemini/antigravity-cli/settings.json
jq . ~/.copilot/settings.json
```

If you already have other settings, merge the `statusLine` object instead of replacing the entire file with an array or plain string.

### Cannot connect to `localhost:3003`

```bash
PORT=3010 "$HOME/.local/bin/token-usage-insights"
```

If you use another port, open the corresponding URL, for example:

```text
http://localhost:3010
```

* * *

## Development commands

This section is for developers who need to modify or build the project from source. For normal use, use the one-line installation command above.

```bash
git clone https://github.com/doggy8088/TokenUsageInsights.git
cd TokenUsageInsights
cargo fmt
cargo test
cargo clippy --all-targets --all-features
cargo build --release
./target/release/token-usage-insights
```

* * *

## Project files

```text
src/                 Rust 後端、API、SQLite 同步、價格與時間軸解析
static/              前端 HTML、JavaScript、CSS 與圖片資產
shell/               Bash/PowerShell Status Line collector 與 systemd 服務範本
scripts/             Linux/macOS、Windows 安裝與 Windows smoke test
pricing.csv          模型價格表，本地估算費用依此檔案載入
```

* * *

## Screenshots

![Token War Room daily dashboard](screenshots/codex-daily-2026-07-07-desktop-chrome.png)

![Token War Room monthly dashboard](screenshots/codex-daily-2026-07-07.png)

![Token War Room session timeline](screenshots/codex-daily-2026-07-07-desktop-chrome.png)
