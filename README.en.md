# 🌟 Unified AI CLI Token Usage Insights Dashboard

[繁體中文版 (Traditional Chinese Version)](./README.md)

This is a local token consumption and session analysis dashboard designed specifically for local terminal AI assistants including **Google Antigravity CLI**, **GitHub Copilot CLI**, and **Codex CLI**. Powered by a high-performance **Rust (Axum)** backend and a beautiful **dark glassmorphic (Glassmorphism)** frontend, it helps you monitor token usage, cache hit rates, and API latency across all local AI assistants in one single place, and **reconstructs/restores the full conversation timeline of every session**!

> [!IMPORTANT]
> **System Requirement**: This project is only supported on **WSL (Windows Subsystem for Linux)** or **macOS** environments.

---

## 🤖 Agent Quick Start Prompt
If you want another AI agent or automation flow to set this up quickly, paste this more complete prompt:

```text
Please install and launch Unified AI CLI Token Usage Insights Dashboard in this environment using the local-first workflow described in this README.

Goal:
- Install and start this Rust/Axum project locally.
- Configure statusLine collection scripts for Google Antigravity CLI and GitHub Copilot CLI to automatically collect token usage data locally.
- Confirm the dashboard is available at http://localhost:3003.

Please complete the following steps in order:
1. Confirm that the current working directory is the project root and that the following files exist:
   - shell/antigravity/statusline-token.sh
   - shell/copilot/statusline-token.sh
2. Verify that the Rust toolchain is installed; if not, install it with rustup.
3. Run cargo build --release to build the release binary.
4. Set up Google Antigravity CLI:
   - Create or confirm that ~/.gemini/antigravity-cli exists.
   - Copy shell/antigravity/statusline-token.sh to ~/.gemini/antigravity-cli/statusline-token.sh.
   - Make the script executable with chmod +x ~/.gemini/antigravity-cli/statusline-token.sh.
   - Update ~/.gemini/antigravity-cli/settings.json:
     - If the file does not exist, create a new JSON file whose top-level value is a valid JSON object and include a statusLine block.
     - If it already exists, preserve the existing settings and merge the statusLine block into it; keep the original JSON structure intact and do not turn it into an array or another non-object format.
     - The statusLine command should point to ~/.gemini/antigravity-cli/statusline-token.sh using the actual home directory path (for example, from echo $HOME).
     - Example `statusLine` configuration:
       {
         "statusLine": {
           "type": "command",
           "command": "$HOME/.gemini/antigravity-cli/statusline-token.sh",
           "padding": 1
         }
       }
5. Set up GitHub Copilot CLI:
   - Create or confirm that ~/.copilot exists.
   - Copy shell/copilot/statusline-token.sh to ~/.copilot/statusline-token.sh.
   - Make the script executable with chmod +x ~/.copilot/statusline-token.sh.
   - Update ~/.copilot/settings.json:
     - If the file does not exist, create a new JSON file whose top-level value is a valid JSON object and include a statusLine block.
     - If it already exists, preserve the existing settings and merge the statusLine block into it; keep the original JSON structure intact.
     - The statusLine command should point to ~/.copilot/statusline-token.sh using the actual home directory path (for example, from echo $HOME).
     - Example `statusLine` configuration:
       {
         "statusLine": {
           "type": "command",
           "command": "$HOME/.copilot/statusline-token.sh",
           "padding": 1
         }
       }
6. Validate the scripts and configurations:
   - Run echo '{}' | ~/.gemini/antigravity-cli/statusline-token.sh to confirm execution.
   - Run echo '{}' | ~/.copilot/statusline-token.sh to confirm execution.
   - Verify the JSON format is valid for ~/.gemini/antigravity-cli/settings.json and ~/.copilot/settings.json using jq or python -m json.tool.
7. Return to the project root and run cargo run to start the dashboard service.
8. Confirm that the terminal output includes http://localhost:3003 and that the URL is reachable in a browser.

Notes:
- Codex CLI does not require any helper scripts or statusLine configuration; the backend automatically recursively scans your local ~/.codex/sessions directory.
- This project is local-first; do not assume any remote API or external service.
- If any step fails, stop and report the exact error message and the command that failed.
- After completion, provide a short summary including whether startup succeeded, whether the URL is reachable, and any caveats.
```

---

## 🌟 Key Features

This dashboard provides comprehensive local data visualization and monitoring across multiple CLI assistants:

1. **📊 Consolidated Real-time Dashboard**
   * **All-in-One Dashboard**: Simultaneously monitor Google Antigravity CLI, GitHub Copilot CLI, and Codex CLI for daily total token usage, input/output ratio, cache read tokens, and reasoning tokens.
   * **Trends & Analytics**: Interactive Chart.js curves displaying daily session token consumption, cache hit rate, and session turns.
   * **🔴 Live Monitor (Auto-refresh)**: Enable auto-refresh (5s, 10s, 30s intervals) to sync live with active terminal conversations, complete with a countdown progress bar.

2. **📅 Monthly Aggregation & Cost Comparison**
   * **Monthly Trends**: Line charts tracking daily token usage and session counts across the month.
   * **Most Active Project Directories**: Tracks session counts and token metrics by working directory (CWD) to identify where AI assistance is utilized most.
   * **Model Breakdown**: Analyzes session counts and token distributions for different LLMs (e.g., Gemini, GPT-4o, Claude, etc.) used.

3. **🔍 Interactive Session History**
   * View the complete list of historical sessions. Sort columns (Session ID/name, model, turns, token count, API duration in ms) ascending or descending to filter out high-consuming sessions.

4. **⏱️ Session Timeline Drawer**
   * Click any session to slide out a comprehensive historical conversation timeline drawer.
   * **Reconstruct Conversations**:
     * **User Prompt**: Clear dialogue bubbles showing prompt text along with context attachments.
     * **Agent Reply**: Shows LLM reasoning steps alongside Markdown responses formatted with code syntax highlighting.
     * **Tool Step (CLI Tool Step)**: Expands tool invocation details, including arguments, exit codes, stdout, and stderr.

---

## 🔄 Legacy Data Auto-Migration

If you have previously installed and used any of the following standalone token insights projects:
* [copilot-cli-token-insights](https://github.com/wengct/copilot-cli-token-insights)
* [antigravity-cli-token-insights](https://github.com/wengct/antigravity-cli-token-insights)
* [codex-cli-token-insights](https://github.com/wengct/codex-cli-token-insights)

Upon launching this dashboard, the backend will **automatically detect and migrate/merge the historical data** into the unified database of this project, while safely backing up the legacy database files to ensure no data is lost.

### 🗑️ Disabling & Deleting Legacy Services

Since this project consolidates all features of the standalone projects, we recommend stopping, disabling, and removing the legacy background services to avoid conflicts and save system resources:

1. **Stop and Disable Legacy Services**:
   Run the following commands to stop and disable the systemd user services for the legacy projects you previously installed:
   ```bash
   # Stop and disable legacy Copilot insights service
   systemctl --user stop copilot-cli-token-insights.service
   systemctl --user disable copilot-cli-token-insights.service

   # Stop and disable legacy Antigravity insights service
   systemctl --user stop antigravity-cli-token-insights.service
   systemctl --user disable antigravity-cli-token-insights.service

   # Stop and disable legacy Codex insights service
   systemctl --user stop codex-cli-token-insights.service
   systemctl --user disable codex-cli-token-insights.service
   ```

2. **Remove Legacy Service Unit Files**:
   ```bash
   rm -f ~/.config/systemd/user/copilot-cli-token-insights.service
   rm -f ~/.config/systemd/user/antigravity-cli-token-insights.service
   rm -f ~/.config/systemd/user/codex-cli-token-insights.service
   ```

3. **Reload Systemd User Daemon**:
   ```bash
   systemctl --user daemon-reload
   systemctl --user reset-failed
   ```

4. **Delete Legacy Project Folders (Optional)**:
   You may safely delete the code repositories of the legacy projects to free up space.

---

## ⚙️ Data Collection Setup

Since this project is local-first, follow these steps to configure data collection for each CLI assistant:

### 1️⃣ Codex CLI
* **No Hooks Needed**: Codex features a non-intrusive design. The backend automatically scans the local `~/.codex/sessions` directory, requiring no statusline or hook configurations.

### 2️⃣ Google Antigravity CLI
1. **Deploy Data Collection Script**:
   ```bash
   mkdir -p ~/.gemini/antigravity-cli
   cp shell/antigravity/statusline-token.sh ~/.gemini/antigravity-cli/statusline-token.sh
   chmod +x ~/.gemini/antigravity-cli/statusline-token.sh
   ```
2. **Edit Configuration File `~/.gemini/antigravity-cli/settings.json`**:
   For new configurations, paste the following; for existing settings, merge the `statusLine` object block:
   ```json
   {
     "statusLine": {
       "type": "command",
       "command": "$HOME/.gemini/antigravity-cli/statusline-token.sh",
       "padding": 1
     }
   }
   ```
   > [!NOTE]
   > Replace the `command` path with your actual home directory path (run `echo $HOME` in terminal to find it).

### 3️⃣ GitHub Copilot CLI
1. **Deploy Data Collection Script**:
   ```bash
   mkdir -p ~/.copilot
   cp shell/copilot/statusline-token.sh ~/.copilot/statusline-token.sh
   chmod +x ~/.copilot/statusline-token.sh
   ```
2. **Edit Configuration File `~/.copilot/settings.json`**:
   For new configurations, paste the following; for existing settings, merge the `statusLine` object block:
   ```json
   {
     "statusLine": {
       "type": "command",
       "command": "$HOME/.copilot/statusline-token.sh",
       "padding": 1
     }
   }
   ```
   > [!NOTE]
   > Replace the `command` path with your actual home directory path (run `echo $HOME` in terminal to find it).

### 💡 Status Line Display Effect

Once you have configured the `statusLine` and interact with your AI assistant (Google Antigravity CLI or GitHub Copilot CLI), a real-time token statistics status line will be displayed at the bottom of your terminal.

**Visual Example:**
```text
🤖 gemini-1.5-pro • #3 • ↑ 12.3k • c 4.5k/0 • ↓ 1.2k • r 500 • total 18.5k • +2.1k • last 1.5k/200 • ctx 15%
```

**Field Breakdown:**
* 🤖 **Model Name**: The LLM model currently being used for the session.
* **# Turn Number** (`#3`): The number of conversation turns in the current session.
* **↑ Input Tokens** (`↑ 12.3k`): Total cumulative input tokens (`k` for thousand, `m` for million).
* **c Cache Read/Write** (`c 4.5k/0`): Cumulative cache read tokens / cache write tokens.
* **↓ Output Tokens** (`↓ 1.2k`): Total cumulative output tokens.
* **r Reasoning Tokens** (`r 500`): Cumulative reasoning tokens.
* **total Total Tokens** (`total 18.5k`): Total cumulative tokens consumed in the current session.
* **+ Delta Tokens** (`+2.1k`): Tokens consumed by the most recent prompt/turn.
* **last Last API Call** (`last 1.5k/200`): Input/output tokens of the last API request.
* **ctx Context Usage** (`ctx 15%` or `from <old_model>`): Percentage of the context window size used; or a model switch indicator (e.g., `• from gemini-1.5-flash`) if the model changes.

---

## 🚀 Setup & Launch

### 1. Launch Locally
From the project root directory, run:
```bash
cargo run
```
Open [**`http://localhost:3003`**](http://localhost:3003) in your browser to start exploring your dashboard!

### 2. Run as a Systemd Background Service
1. **Compile the Release Binary**:
   ```bash
   cargo build --release
   ```
2. **Configure the Service File**:
   ```bash
   mkdir -p ~/.config/systemd/user/
   sed "s|<PROJECT_DIR>|$PWD|g" shell/token-usage-insights.service > ~/.config/systemd/user/token-usage-insights.service
   systemctl --user daemon-reload
   ```
3. **Service Management Commands**:
   * **Start Service**: `systemctl --user start token-usage-insights.service`
   * **Enable Auto-start on Boot**: `systemctl --user enable token-usage-insights.service`
   * **Check Status**: `systemctl --user status token-usage-insights.service`
   * **Check Real-time Logs**: `journalctl --user -u token-usage-insights.service -n 50 -f`
   * **Restart Service**: `systemctl --user restart token-usage-insights.service`
   * **Stop Service**: `systemctl --user stop token-usage-insights.service`

---

## ⚙️ Advanced Configurations & Env Variables

You can override default data folders by setting environment variables before starting the service:

- `INSIGHTS_DIR`: Path to save the central SQLite database (default: `~/.token-usage-insights`)
- `ANTIGRAVITY_DIR`: Google Antigravity CLI data path (default: `~/.gemini/antigravity-cli`)
- `COPILOT_DIR`: GitHub Copilot CLI data path (default: `~/.copilot`)
- `CODEX_DIR`: Codex CLI session data path (default: `~/.codex`)

Example of running with a custom database directory:
```bash
export INSIGHTS_DIR="/your/custom/path"
cargo run
```

## Interface Demonstration

<img width="1920" height="869" alt="image" src="https://github.com/user-attachments/assets/0e334f40-7771-4366-82f5-6f2bcec24b81" />


<img width="1920" height="869" alt="image" src="https://github.com/user-attachments/assets/5ebc79ea-f44b-4c59-9f12-f37b4f6bb6e0" />


<img width="1920" height="869" alt="image" src="https://github.com/user-attachments/assets/92739400-be86-4306-b7d6-aadc236c1aef" />

