# Token 战情室

**Token 战情室是本地优先的 AI Coding Agent Token 使用量与会话还原看板。** 它会读取本机上的 Google Antigravity CLI、GitHub Copilot CLI、GitHub Copilot Chat（VS Code）、Codex Desktop、Codex CLI、Claude Code、Grok Build、Pi Coding Agent 与 OMP 记录，集中呈现每日、月度、年度的 Token 消耗、缓存使用、推理 Token、估算费用、模型分布、项目目录分布与完整 Session 时间轴。

本项目不会代你调用 AI 供应商 API 查询数据；核心数据来源是本地日志、Status Line 收集文件与本地 SQLite。

> 系统环境：支持 Windows 10/11 原生 PowerShell、macOS、Linux 与 WSL。

语言： [繁體中文](README.md) · [简体中文](README.zh-CN.md) · [English](README.en.md) · [日本語](README.ja.md) · [한국어](README.ko.md)

* * *

## 最短上手路径

### 1. 一行安装并启动看板

Linux / macOS：

```bash
curl -fsSL https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.sh | bash && "$HOME/.local/bin/token-usage-insights"
```

Windows PowerShell：

```powershell
irm https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.ps1 | iex; & "$HOME\bin\token-usage-insights.cmd"
```

上述命令会下载并安装当前平台的已编译版本，不需要 Rust、Cargo、WSL 或手动解压。安装完成后，看板会在本机运行。

打开：

```text
http://localhost:3003
```

### 2. 根据你使用的工具决定是否需要设置

| 工具 | 是否需要额外设置 | 默认数据源 | 说明 |
| --- | --- | --- | --- |
| Google Antigravity CLI | 需要 | `~/.gemini/antigravity-cli/usage/usage-YYYY-MM-DD.jsonl` | 通过 `statusline-token.sh` 或 Windows `statusline-token.ps1` 收集 Token 数据 |
| GitHub Copilot CLI | 需要 | `~/.copilot/usage/usage-YYYY-MM-DD.jsonl` | 通过 `statusline-token.sh` 或 Windows `statusline-token.ps1` 收集 Token 数据 |
| GitHub Copilot Chat（VS Code） | 不需要 | VS Code `workspaceStorage/chatSessions` | 看板直接扫描 VS Code Stable 与 Insiders 的本地聊天 Session |
| Codex Desktop / CLI | 不需要 | `~/.codex/sessions`、`~/.codex/archived_sessions` | 看板会直接扫描 Codex 活动中与已归档的本地 Session 记录 |
| Claude Code | 不需要 | `~/.claude/projects` | 看板会直接扫描 Claude Code 的本地项目 Session 记录 |
| Grok Build | 不需要 | `~/.grok/sessions` | 看板会直接扫描 Grok Build 自动保存的 `updates.jsonl` Session stream |
| Pi Coding Agent | 不需要 | `~/.pi/agent/sessions` | 看板会直接扫描 Pi Coding Agent 自动保存的本地 Session JSONL 文件 |
| OMP | 不需要 | `~/.omp/agent/sessions` | 看板会直接扫描 OMP 自动保存的本地 Session JSONL 文件 |

**只使用 VS Code Copilot、Codex Desktop、Codex CLI、Claude Code、Grok Build、Pi Coding Agent 或 OMP 时，执行一行安装命令并打开看板即可。**

### Windows 原生使用

Windows 的一行安装会创建 `%USERPROFILE%\bin\token-usage-insights.cmd` 启动文件；不需要 Rust MSVC toolchain、Visual Studio Build Tools、WSL、Git Bash 或 `jq`。

Windows 默认使用以下原生路径：

| 用途 | Windows 默认路径 |
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

看板内的设置指南会在 Windows 显示 PowerShell 复制、设置与诊断命令。PowerShell collector 使用 .NET JSON 与文件 API，不依赖 Bash、`jq`、`sed` 或 `awk`。

驱动器号、含空格或非 ASCII 字符的路径，以及 UNC 路径都会交由原生路径 API 处理。SQLite 数据库仍建议放在本地磁盘，以避免网络共享的 locking 语义差异。

* * *

## 支持功能

### 数据分析

- 每日、月度、年度 Token 统计
- 输入、输出、缓存读取、缓存写入、推理 Token 拆分
- 根据 `pricing.csv` 进行本地费用估算
- Session 数量、请求次数与 API 耗时统计
- 模型使用量排名
- Cursor 可由本地 `state.vscdb` 的 `agentKv` 记录归因到具体模型；无法唯一匹配时保留为 `Unknown Model`
- 项目工作目录统计
- 可排序的 Session 列表
- 自动读取 GitHub Copilot App（桌面应用）`~/.copilot/data.db` 与 `session-store.db`

### Session 还原

- 右侧抽屉式 Session 时间轴
- 用户提示词、助理回复、推理内容与工具调用步骤
- 工具调用参数、退出码、stdout、stderr
- Codex subagent 相关字段，例如 parent session、agent nickname、agent role
- Markdown 回复渲染与内容清理

### 界面操作

- 五种 CLI 徽章切换
- 每日、月度、年度视图
- 日期、月份、年份快速切换
- 5 秒、10 秒、30 秒实时自动刷新
- 手动同步本地日志到 SQLite
- 深色与浅色主题
- 繁体中文与英文界面切换
- 模型费用表查看

* * *

## 网址参数（深层链接）

看板支持通过网址查询参数（Query String）直接打开指定状态的画面，方便加入书签、分享链接，或从其他工具跳转过来。在看板上切换 Agent、视图、日期、工作目录或图表类型时，网址也会自动更新为当前的状态。

| 参数 | 适用视图 | 可用值 | 说明 |
| --- | --- | --- | --- |
| `agent` | 全部 | `antigravity`、`copilot`、`codex`、`claude`、`cursor`、`grok`、`pi`、`omp` | 指定要显示的 Coding Agent。另支持 `claude-code`、`grok-build`、`pi-coding-agent` 等别名写法 |
| `tab` | 全部 | `daily`、`monthly`、`yearly` | 指定以日（每日）、月（月度）或年（年度）视图显示 |
| `date` | 全部 | `daily`：`YYYY-MM-DD`；`monthly`：`YYYY-MM`；`yearly`：`YYYY` | 指定要显示的日期、月份或年份，格式会依 `tab` 自动对应 |
| `dir` | `daily` | 完整路径、`~` 开头的家目录路径，或唯一的路径后缀（如 `TokenUsageInsights`） | 指定每日视图的工作目录筛选。Windows 路径不区分大小写；找不到匹配目录时会显示全部 |
| `chart` | `daily` | `kline`、`trend` | 指定每日视图的图表类型：K 线图或趋势图 |

示例（`http://localhost:3003` 为默认网址，请依实际 `HOST`/`PORT` 调整）：

```text
http://localhost:3003/?agent=copilot&tab=monthly&date=2026-08
http://localhost:3003/?agent=codex&tab=yearly&date=2026
http://localhost:3003/?agent=claude&tab=daily&date=2026-08-09&chart=trend
http://localhost:3003/?agent=copilot&tab=daily&date=2026-08-09&dir=~/projects/TokenUsageInsights
```

> 路径含有 `~`、空格或非 ASCII 字符时请先进行 URL 编码（`~` 可编码为 `%7E`）。未提供的参数会沿用上次浏览的状态（Cookie / localStorage）。

* * *

## Google Antigravity CLI 设置

Antigravity CLI 需要将本项目的 Status Line 脚本连接到 `settings.json`。脚本会把每次对话后的 Token 累计值与增量写入：

```text
~/.gemini/antigravity-cli/usage/usage-YYYY-MM-DD.jsonl
```

### 1. 安装收集脚本

完成一行安装后，执行：

```bash
mkdir -p ~/.gemini/antigravity-cli && cp ~/.local/share/token-usage-insights/shell/antigravity/statusline-token.sh ~/.gemini/antigravity-cli/statusline-token.sh && chmod +x ~/.gemini/antigravity-cli/statusline-token.sh
```

如果使用自定义安装位置，请将命令中的 `~/.local/share/token-usage-insights` 替换为 `TOKEN_USAGE_INSIGHTS_INSTALL_DIR` 指定的位置。

### 2. 设置 `~/.gemini/antigravity-cli/settings.json`

如果文件不存在，可以创建以下内容。如果文件已经存在，请只合并 `statusLine` 区块，不要覆盖原有设置。

```json
{
  "statusLine": {
    "type": "command",
    "command": "/ABSOLUTE/HOME/.gemini/antigravity-cli/statusline-token.sh",
    "padding": 1
  }
}
```

请将 `/ABSOLUTE/HOME` 替换为 `echo $HOME` 显示的实际主目录路径，例如 `/Users/will` 或 `/home/will`。

### 3. 验证

```bash
echo '{}' | ~/.gemini/antigravity-cli/statusline-token.sh
jq . ~/.gemini/antigravity-cli/settings.json
```

完成后重新进入 Antigravity CLI Session，状态栏会输出类似格式：

```text
model-name • #3 • input 12.3k • cache 4.5k/0 • output 1.2k • reasoning 500 • total 18.5k
```

* * *

## GitHub Copilot CLI 设置

Copilot CLI 与 Antigravity CLI 一样，需要将本项目的 Status Line 脚本连接到 `settings.json`。脚本会把 Token 数据写入：

```text
~/.copilot/usage/usage-YYYY-MM-DD.jsonl
```

### 1. 安装收集脚本

完成一行安装后，执行：

```bash
mkdir -p ~/.copilot && cp ~/.local/share/token-usage-insights/shell/copilot/statusline-token.sh ~/.copilot/statusline-token.sh && chmod +x ~/.copilot/statusline-token.sh
```

如果使用自定义安装位置，请将命令中的 `~/.local/share/token-usage-insights` 替换为 `TOKEN_USAGE_INSIGHTS_INSTALL_DIR` 指定的位置。

### 2. 设置 `~/.copilot/settings.json`

如果文件不存在，可以创建以下内容。如果文件已经存在，请只合并 `statusLine` 区块，不要覆盖原有设置。

```json
{
  "statusLine": {
    "type": "command",
    "command": "/ABSOLUTE/HOME/.copilot/statusline-token.sh",
    "padding": 1
  }
}
```

请将 `/ABSOLUTE/HOME` 替换为 `echo $HOME` 显示的实际主目录路径。

### 3. 验证

```bash
echo '{}' | ~/.copilot/statusline-token.sh
jq . ~/.copilot/settings.json
```

完成后重新进入 Copilot CLI Session，状态栏会开始输出并累积 Token 数据。

* * *

## GitHub Copilot App（桌面应用）

**Copilot App（Tauri 桌面应用）无需任何设置。** 看板会自动读取本机 `~/.copilot/data.db` 与 `~/.copilot/session-store.db`，将 App session 的 token 使用量与 CLI / VS Code 合并显示在 Copilot 页面；Session 列表以 `App` 标示来源，与 `CLI`、`VS Code` 区分。

- 看板会在每次后台同步（每 5 秒）检查这两个 SQLite，并以 `(created_at, id)` 复合游标进行增量同步，避免同一时间戳的多笔 event 重复 upsert；同一个 `(session_id, turn_index)` 不会重复写入。
- App 的 `assistant_usage_events` 是 per-API-call 粒度；看板会按 Session、Turn、Agent 与模型聚合，保留同一回合的多模型归因，再以 per-turn 统计供时间轴使用。
- Session 标题取自 `data.db.sessions.title`。

如果 App 与 CLI 分离，或使用非默认目录，可以指定环境变量：

```bash
COPILOT_APP_DIR="/path/to/copilot-app-data" token-usage-insights
```

`COPILOT_APP_DIR` 的优先级高于 `COPILOT_DIR`；未设置时回退到 `~/.copilot`。

* * *

## GitHub Copilot Chat（VS Code）设置

**VS Code Copilot Chat 不需要安装 Status Line、Hook 或额外收集脚本。**看板会直接读取本地 `workspaceStorage` 中的聊天 Session，并与 Copilot CLI 合并显示；Session 列表会以 `VS Code` 或 `CLI` 标示来源。

支持 VS Code Stable 与 Insiders：

| 平台 | Stable | Insiders |
| --- | --- | --- |
| Windows | `%APPDATA%\Code\User\workspaceStorage` | `%APPDATA%\Code - Insiders\User\workspaceStorage` |
| macOS | `~/Library/Application Support/Code/User/workspaceStorage` | `~/Library/Application Support/Code - Insiders/User/workspaceStorage` |
| Linux | `~/.config/Code/User/workspaceStorage` | `~/.config/Code - Insiders/User/workspaceStorage` |

使用方式：

1. 在 VS Code 中使用 GitHub Copilot Chat，创建至少一个聊天 Session。
2. 启动看板或按右上角同步按钮。
3. 在 Copilot 页面查看合并后的统计与 Session 时间轴。

看板会完整回填现有的 `chatSessions` 文件，并在文件大小或修改时间变化时重新同步；没有 Token 字段的聊天 Session 仍会显示，但 Token 数为 0。数据只读取本地聊天文件，不包含云端 Session、Remote SSH 主机或 `state.vscdb`。

如果 VS Code 使用 `--user-data-dir` 或 Portable Mode，可以指定看板自定义的数据根目录：

macOS / Linux：

```bash
VSCODE_USER_DATA_DIR="/path/to/vscode-user-data" token-usage-insights
```

Windows PowerShell：

```powershell
$env:VSCODE_USER_DATA_DIR = "C:\path\to\vscode-user-data"; & "$HOME\bin\token-usage-insights.cmd"
```

`VSCODE_USER_DATA_DIR` 应指向包含 `User/workspaceStorage` 的 VS Code 用户数据目录。Portable Mode 如果环境变量指向 `data` 目录，请改用 `VSCODE_PORTABLE_DATA_DIR`；看板会同时检查 `data/user-data/User/workspaceStorage` 与 `data/User/workspaceStorage`。

* * *

## Codex 设置

**Codex Desktop 与 Codex CLI 都不需要安装 Hook、Status Line 或额外收集脚本。**

看板会直接扫描：

```text
~/.codex/sessions
~/.codex/archived_sessions
```

使用方式：

1. 先正常使用 Codex Desktop 或 Codex CLI 创建至少一个 Session。
2. 启动本项目。
3. 在左侧选择 Codex。
4. 按右上角同步按钮，或等待后台同步。

注意事项：

- Codex 的身份凭证仍由 Codex 自身管理。
- 看板只读取本地 Session 记录并进行分析。
- 每个 Session 会根据 transcript 的 `originator` 显示 `Desktop` 或 `CLI` 来源标记；无法判断的旧格式会保持未分类。
- 如果显示 API 额度信息，其来源是最后一次本地 Session 日志，并非实时在线查询。

* * *

## Claude Code 设置

**Claude Code 不需要安装 Hook、Status Line 或额外收集脚本。**

看板会直接扫描：

```text
~/.claude/projects
```

使用方式：

1. 先正常使用 Claude Code 创建至少一个项目 Session。
2. 启动本项目。
3. 在左侧选择 Claude Code。
4. 按右上角同步按钮，或等待后台同步。

注意事项：

- Claude Code 的身份凭证仍由 Claude Code 自身管理。
- 看板只读取本地项目 Session 记录并进行分析。
- 如果 `~/.claude/projects` 不存在，Claude Code 页面会显示无数据。

* * *

## Grok Build 设置

**Grok Build 不需要安装 Hook、Status Line 或额外收集脚本。** 看板会直接扫描：

```text
~/.grok/sessions
```

这里使用 Grok Build 内置保存的 Session stream；不读取旧规范中的
`~/.Grok/build/usage/usage-YYYY-MM-DD.jsonl`，也不需要在
`~/.Grok/build/settings.json` 设置 `statusLine`。

使用方式：

1. 先正常使用 Grok Build 创建至少一个 Session。
2. 启动本项目。
3. 在左侧选择 Grok Build。
4. 按右上角同步按钮，或等待后台同步。

Grok Build Session 可能只提供 context token snapshot，也可能包含 provider usage 与成本。看板会优先使用 provider usage/cost；只有 context snapshot 时，费用会根据 `pricing.csv` 的 xAI API 价格估算，并在 Session 列表标示 `Context`，不代表 SuperGrok 或其他订阅方案的每周配额。

* * *

## Pi Coding Agent 设置

**Pi Coding Agent 不需要安装 Hook、Status Line 或额外收集脚本。** 看板会直接扫描：

```text
~/.pi/agent/sessions
```

Pi Coding Agent 会以树状目录结构自动将 Session 保存为本地 JSONL 文件；看板会直接读取这些 Session 记录。

使用方式：

1. 先正常使用 Pi Coding Agent 创建至少一个 Session。
2. 启动或刷新本项目看板。
3. 在左侧选择 Pi Coding Agent。
4. 按右上角同步按钮，或等待后台同步。

Pi Coding Agent 的成本始终直接读取自 Session 每个 turn 自行报告的 `usage.cost` 与相关 usage 数据，不会像 Grok Build 那样回退到 context snapshot 估算；因为 Pi 原生就会提供权威的 token 与成本信息。

* * *

## OMP 设置

**OMP 不需要安装 Hook、Status Line 或额外收集脚本。** 看板会直接扫描：

```text
~/.omp/agent/sessions
```

OMP 是 Pi Coding Agent 的开源分支（<https://github.com/can1357/oh-my-pi>），并使用完全相同的 JSONL 格式持久化 Session。看板会直接读取这些本地 Session 记录。

使用方式：

1. 先正常使用 OMP 创建至少一个 Session。
2. 启动或刷新本项目看板。
3. 在左侧选择 OMP。
4. 按右上角同步按钮，或等待后台同步。

OMP 的成本始终直接读取自 Session 每个 turn 自行报告的 `usage.cost` 与相关 usage 数据，不会像 Grok Build 那样回退到 context snapshot 估算；因为 OMP 原生就会提供权威的 token 与成本信息。

* * *

## 本地数据同步方式

启动服务时，后端会初始化本地 SQLite 并立即同步一次数据。服务启动后，也会每 5 秒进行一次后台同步。

SQLite 默认位置：

```text
~/.token-usage-insights/token_usage_insights.db
```

前端右上角的同步按钮会调用：

```text
GET /api/:assistant/sync
```

这会触发一次完整的本地日志增量同步。

## 导入 / 导出（跨机器汇总）

**一般使用请直接使用看板右上角的导出与导入按钮。** 安装版只需要浏览器即可完成跨机器数据汇总，并支持最大 200 MB 的导入文件。

看板与 CLI 已整合为同一个 `token-usage-insights` 可执行文件。不带参数会启动看板，使用 `export`、`export-all`、`import` 子命令可操作数据；主命令与各子命令均支持 `--help`、`-h`。此整合从下一个发布版本起提供，旧版需要更新或从源代码构建。

`--agent` 用于指定助理（`antigravity` / `copilot` / `codex` / `claude` / `cursor` / `grok` / `pi` / `omp`）。

### 从源代码使用 CLI

先构建一次：

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

数据格式与前端一致，包含以下字段：

- `version`
- `assistant`
- `date`
- `exported_at`
- `records`（每条记录都会有 `import_source_id`）

`import_source_id` 会与 `assistant_type` 一起组成唯一键；重复导入同一条记录会被判定为重复并自动跳过，不会重复写入数据库。

* * *

## 环境变量

环境变量指定的路径会被视为权威设置，不必预先创建；`INSIGHTS_DIR` 会在启动时自动创建。支持原生绝对/相对路径，以及以 `~`、`$HOME`、`%USERPROFILE%`、`%LOCALAPPDATA%` 或 `%APPDATA%` 开头的常见写法。

| 变量 | 默认值 | 用途 |
| --- | --- | --- |
| `HOST` | `0.0.0.0` | 看板服务绑定的 IPv4 或 IPv6 地址 |
| `PORT` | `3003` | 看板服务端口号 |
| `INSIGHTS_DIR` | Windows: `%LOCALAPPDATA%\TokenUsageInsights`; 其他平台：`~/.token-usage-insights` | SQLite 数据库目录 |
| `ANTIGRAVITY_DIR` | `~/.gemini/antigravity-cli` | Antigravity CLI 数据目录 |
| `COPILOT_DIR` | `~/.copilot` | Copilot CLI 数据目录 |
| `COPILOT_APP_DIR` | 同 `COPILOT_DIR` | Copilot App（桌面应用）数据目录，应包含 `data.db` 与 `session-store.db` |
| `VSCODE_USER_DATA_DIR` | 按平台自动检测 | VS Code 用户数据目录，应包含 `User/workspaceStorage` |
| `VSCODE_PORTABLE_DATA_DIR` | 未设置 | VS Code Portable Mode 的 `data` 目录 |
| `CODEX_DIR` | `~/.codex` | Codex Desktop 与 Codex CLI 共用的数据目录 |
| `CLAUDE_DIR` | `~/.claude` | Claude Code 数据目录 |
| `CURSOR_DIR` | `~/.cursor` | Cursor 数据目录 |
| `CURSOR_STATE_DB` | 按平台自动检测 | Cursor `User/globalStorage/state.vscdb` 路径，用于只读获取 `agentKv` 模型信息 |
| `GROK_DIR` | `~/.grok` | Grok Build 数据目录 |
| `PI_DIR` | `~/.pi` | Pi Coding Agent 数据目录 |
| `OMP_DIR` | `~/.omp` | OMP 数据目录 |
| `CORS_ALLOWED_ORIGINS` | `http://localhost:<PORT>,http://127.0.0.1:<PORT>` | 允许的 CORS 来源，以逗号分隔 |

> **默认绑定 `0.0.0.0`，同一局域网内的其他设备可能连接到看板。只需在本机浏览时，请将 `HOST` 设置为 `127.0.0.1`。**

示例：

```bash
HOST="127.0.0.1" INSIGHTS_DIR="/tmp/token-usage-insights" PORT="3010" "$HOME/.local/bin/token-usage-insights"
```

Windows PowerShell 示例：

```powershell
$env:HOST = '127.0.0.1'; $env:INSIGHTS_DIR = 'D:\Token Usage Insights\資料庫'; $env:CODEX_DIR = "$env:USERPROFILE\.codex"; $env:PORT = '3010'; & "$HOME\bin\token-usage-insights.cmd"
```

* * *

## 常驻服务

### Linux：一行安装并启用 systemd 用户服务

```bash
curl -fsSL https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.sh | bash -s -- --service
```

这会下载安装版并立即启用 `token-usage-insights.service`，不需要自行构建或修改 systemd 文件。

### 管理服务

```bash
systemctl --user status token-usage-insights.service
journalctl --user -u token-usage-insights.service -n 50 -f
systemctl --user restart token-usage-insights.service
systemctl --user stop token-usage-insights.service
```

* * *

## 安装选项与手动安装

GitHub Release 提供 Linux、macOS 与 Windows 的已编译可执行文件，安装与运行都不需要 Rust 或 Cargo。

### 一行安装的可选参数

`scripts/get.sh`（Linux / macOS）与 `scripts/get.ps1`（Windows）会自动判断平台与 CPU 架构，从最新（或指定）Release 下载对应压缩包，解压后调用包内的 `install.sh` / `install.ps1`，全程不需要手动下载或解压：

Linux / macOS：

```bash
curl -fsSL https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.sh | bash
```

Linux 如需同时安装并启用 systemd user service：

```bash
curl -fsSL https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.sh | bash -s -- --service
```

Windows PowerShell：

```powershell
irm https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.ps1 | iex
```

安装完成后即可运行（Linux/macOS 需确认 `bin_dir` 已加入 `PATH`；Windows 会创建 `.cmd` shim）：

```bash
token-usage-insights
```

环境变量可控制版本与安装路径（均为可选）：

| 变量 | 适用平台 | 说明 |
| --- | --- | --- |
| `TOKEN_USAGE_INSIGHTS_VERSION` | Linux / macOS / Windows | 指定要安装的 Release tag，例如 `v0.6.2`。默认 `latest` |
| `TOKEN_USAGE_INSIGHTS_INSTALL_DIR` | Linux / macOS | 安装目录，会传递给 `install.sh` |
| `TOKEN_USAGE_INSIGHTS_BIN_DIR` | Linux / macOS | 可执行文件链接目录，会传递给 `install.sh` |

Windows 若要自定义安装位置、bin 目录与端口号，需要先下载脚本再带参数运行（`iex` 管道不支持传递参数）：

```powershell
Invoke-WebRequest -Uri https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.ps1 -OutFile get.ps1
.\get.ps1 -InstallDir 'D:\Apps\Token Usage Insights' -Port 3010
```

### 手动下载安装

如果不想直接执行远程脚本，也可以手动下载对应平台的压缩包并运行包内置的安装脚本。每个 Release 压缩包都包含：

- 单一平台可执行文件
- `static/` 前端资源
- `pricing.csv` 模型费用表
- `shell/` 目录下的 Status Line 与服务脚本
- `scripts/` 目录（含 `install.sh`、`install.ps1`、`get.sh`、`get.ps1`）
- README、LICENSE 与 VERSION

Linux 或 macOS：

```bash
tar -xzf token-usage-insights-<tag>-<target>.tar.gz
cd token-usage-insights-<tag>-<target>
./install.sh
```

Linux 如需安装并启用 systemd user service：

```bash
./install.sh --service
```

Windows：

```powershell
Expand-Archive token-usage-insights-<tag>-x86_64-pc-windows-msvc.zip
cd token-usage-insights-<tag>-x86_64-pc-windows-msvc
powershell -ExecutionPolicy Bypass -File .\install.ps1
```

自定义 Windows 安装位置与端口号：

```powershell
.\install.ps1 -InstallDir 'D:\Apps\Token Usage Insights' -BinDir "$HOME\bin" -Port 3010
```

### CI 验证

`Release` workflow 每次构建都会在 Linux、macOS 与 Windows 上实际运行对应的安装脚本（`install.sh` / `install.ps1`），安装后启动可执行文件并确认：

- 服务会在指定端口响应 `/api/<assistant>/pricing`
- 响应内容确实加载了包内的 `pricing.csv`
- 全新的 `INSIGHTS_DIR` 会被创建并生成 SQLite 数据库

`get.sh` 与 `get.ps1` 也会在每次构建时先进行语法检查（`bash -n` 与 PowerShell AST 解析），确保推送到 Release 的版本可以正常运行。

### 维护者发布

推送 Git tag 后，GitHub Actions 会自动创建对应的 Release：

```bash
git tag vX.Y.Z
git push origin vX.Y.Z
```

* * *

## 旧数据迁移

如果你以前使用过以下独立项目，启动本项目时会自动尝试迁移旧 SQLite 数据：

- `~/.gemini/antigravity-cli/antigravity_cli_token_insights.db`
- `~/.copilot/copilot_cli_token_insights.db`
- `~/.codex/codex_cli_token_insights.db`

迁移成功后，旧数据库会被重命名为带有 `.bak` 后缀的文件。

如果已确认数据迁移完成，可以停用旧服务：

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

## 故障排查

### 看板没有数据

按工具检查数据源是否存在：

```bash
ls ~/.gemini/antigravity-cli/usage
ls ~/.copilot/usage
ls ~/.codex/sessions
ls ~/.codex/archived_sessions
ls ~/.claude/projects
```

Antigravity CLI 与 Copilot CLI 还需要确认 `settings.json` 已设置 `statusLine`，且脚本具有执行权限。

Windows PowerShell 可直接检查原生数据目录：

```powershell
Get-ChildItem "$env:USERPROFILE\.gemini\antigravity-cli\usage"
Get-ChildItem "$env:USERPROFILE\.copilot\usage"
Get-ChildItem "$env:USERPROFILE\.codex\sessions"
Get-ChildItem "$env:USERPROFILE\.codex\archived_sessions"
Get-ChildItem "$env:USERPROFILE\.claude\projects"
```

### Status Line 脚本无法执行

```bash
command -v jq
chmod +x ~/.gemini/antigravity-cli/statusline-token.sh
chmod +x ~/.copilot/statusline-token.sh
```

Status Line 脚本依赖 `jq` 解析 CLI 传入的 JSON。

上述 `jq` 要求只适用于 `.sh` collector。Windows `.ps1` collector 可使用以下命令测试，并会原生处理反斜杠与包含空格的路径：

```powershell
Write-Output '{}' | powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$env:USERPROFILE\.gemini\antigravity-cli\statusline-token.ps1" -Assistant antigravity
```

### 配置文件 JSON 格式错误

```bash
jq . ~/.gemini/antigravity-cli/settings.json
jq . ~/.copilot/settings.json
```

如果已有其他设置，请合并 `statusLine` 对象，不要把整个文件替换成数组或纯字符串。

### 无法连接到 `localhost:3003`

```bash
PORT=3010 "$HOME/.local/bin/token-usage-insights"
```

如果改用其他端口，请打开对应网址，例如：

```text
http://localhost:3010
```

* * *

## 开发命令

本节仅供需要修改或从源代码构建项目的开发者使用；一般使用请采用前述一行安装命令。

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

## 项目文件

```text
src/                 Rust 後端、API、SQLite 同步、價格與時間軸解析
static/              前端 HTML、JavaScript、CSS 與圖片資產
shell/               Bash/PowerShell Status Line collector 與 systemd 服務範本
scripts/             Linux/macOS、Windows 安裝與 Windows smoke test
pricing.csv          模型價格表，本地估算費用依此檔案載入
```

* * *

## 截图

![Token 战情室每日看板](screenshots/codex-daily-2026-07-07-desktop-chrome.png)

![Token 战情室月度看板](screenshots/codex-daily-2026-07-07.png)

![Token 战情室 Session 时间轴](screenshots/codex-daily-2026-07-07-desktop-chrome.png)
