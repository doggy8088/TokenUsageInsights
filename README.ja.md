# Token 戦情室

**Token 戦情室は、ローカル優先の AI Coding Agent の Token 使用量とセッション復元ダッシュボードです。** Google Antigravity CLI、GitHub Copilot CLI、GitHub Copilot App、GitHub Copilot Chat（VS Code）、Codex Desktop、Codex CLI、Claude Code、Cursor、Grok Build、Pi Coding Agent、OMP、Muse Code のローカル記録を読み取り、日別・月別・年別の Token 消費量、キャッシュ使用量、推論 Token、推定コスト、モデル分布、プロジェクトディレクトリ分布、完全な Session タイムラインをまとめて表示します。

このプロジェクトが AI プロバイダー API を代わりに呼び出してデータを取得することはありません。主なデータソースはローカルログ、Status Line コレクターファイル、ローカル SQLite です。

> システム環境：Windows 10/11 のネイティブ PowerShell、macOS、Linux、WSL に対応しています。

言語： [繁體中文](README.md) · [简体中文](README.zh-CN.md) · [English](README.en.md) · [日本語](README.ja.md) · [한국어](README.ko.md)

* * *

## 最短で始める方法

### 1. 1 行でダッシュボードを起動またはインストール

Node.js 18.18 以降がインストールされている場合、グローバル npm コマンドを作成せずに直接実行できます：

```bash
npx --yes token-usage-insights
```

常設のシステムコマンドとしてインストールする場合は、各プラットフォームのインストーラーを使用します。

Linux / macOS：

```bash
curl -fsSL https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.sh | bash && "$HOME/.local/bin/token-usage-insights"
```

Windows PowerShell：

```powershell
irm https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.ps1 | iex; & "$HOME\bin\token-usage-insights.cmd"
```

`npx` とインストーラーはいずれも現在のプラットフォーム用コンパイル済みバージョンをダウンロードします。Rust、Cargo、WSL、手動展開は必要ありません。コマンドの起動後、ダッシュボードはローカルで実行されます。

開く：

```text
http://localhost:3003
```

### 2. 使用するツールに応じて追加設定の有無を確認

| ツール | 追加設定 | デフォルトのデータソース | 説明 |
| --- | --- | --- | --- |
| Google Antigravity CLI | 必要 | `~/.gemini/antigravity-cli/usage/usage-YYYY-MM-DD.jsonl` | `statusline-token.sh` または Windows の `statusline-token.ps1` で Token データを収集 |
| GitHub Copilot CLI | 必要 | `~/.copilot/usage/usage-YYYY-MM-DD.jsonl` | `statusline-token.sh` または Windows の `statusline-token.ps1` で Token データを収集 |
| GitHub Copilot App | 不要 | `~/.copilot/data.db`、`~/.copilot/session-store.db` | デスクトップアプリのローカル SQLite をダッシュボードが直接読み取り |
| GitHub Copilot Chat（VS Code） | 不要 | VS Code `workspaceStorage/chatSessions` | VS Code Stable と Insiders のローカルチャット Session を直接スキャン |
| Codex Desktop / CLI | 不要 | `~/.codex/sessions`、`~/.codex/archived_sessions` | Codex のアクティブおよびアーカイブ済みローカル Session を直接スキャン |
| Claude Code | 不要 | `~/.claude/projects` | Claude Code のローカルプロジェクト Session を直接スキャン |
| Cursor | 不要 | `~/.cursor/projects` | Cursor のローカル transcript をスキャンし、帰属可能なモデル情報を読み取り専用で取得 |
| Grok Build | 不要 | `~/.grok/sessions` | Grok Build が自動保存する `updates.jsonl` Session stream を直接スキャン |
| Pi Coding Agent | 不要 | `~/.pi/agent/sessions` | Pi Coding Agent が自動保存するローカル Session JSONL ファイルを直接スキャン |
| OMP | 不要 | `~/.omp/agent/sessions` | OMP が自動保存するローカル Session JSONL ファイルを直接スキャン |
| Muse Code | 不要 | `~/.local/share/muse/sessions` | Muse Code が自動保存するローカル Session JSONL ファイルを直接スキャン |

**Copilot App、VS Code Copilot、Codex Desktop、Codex CLI、Claude Code、Cursor、Grok Build、Pi Coding Agent、OMP、Muse Code だけを使用する場合は、1 行のインストールコマンドを実行してダッシュボードを開くだけで利用できます。**

### Windows ネイティブでの利用

Windows の 1 行インストーラーは `%USERPROFILE%\bin\token-usage-insights.cmd` を作成します。Rust MSVC toolchain、Visual Studio Build Tools、WSL、Git Bash、`jq` は必要ありません。

Windows ではデフォルトで次のネイティブパスを使用します：

| 用途 | Windows のデフォルトパス |
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

ダッシュボードの設定ガイドは Windows で PowerShell のコピー、設定、診断コマンドを表示します。PowerShell collector は .NET JSON とファイル API を使用し、Bash、`jq`、`sed`、`awk` に依存しません。

ドライブ文字、空白や非 ASCII 文字を含むパス、UNC パスはすべてネイティブのパス API で処理されます。ネットワーク共有の locking セマンティクスの違いを避けるため、SQLite データベースはローカルディスクに置くことを推奨します。

* * *

## 主な機能

### データ分析

- 日別・月別・年別の Token 統計
- 入力、出力、キャッシュ読み取り、キャッシュ書き込み、推論 Token の内訳
- `pricing.csv` に基づくローカルコスト推定
- Session 数、リクエスト数、API 所要時間の統計
- モデル使用量ランキング
- ローカル `state.vscdb` の `agentKv` 記録から Cursor を具体的なモデルに帰属。 一意に照合できない場合は `Unknown Model` のまま表示
- プロジェクト作業ディレクトリの統計
- 並べ替え可能な Session 一覧
- GitHub Copilot App（デスクトップアプリ）の `~/.copilot/data.db` と `session-store.db` を自動読み込み

### Session の復元

- 右側のドロワーに表示する Session タイムライン
- ユーザープロンプト、アシスタントの返信、推論内容、ツール呼び出し手順
- ツール呼び出しの引数、終了コード、stdout、stderr
- parent session、agent nickname、agent role などの Codex subagent フィールド
- Markdown 返信のレンダリングと内容のサニタイズ

### インターフェース

- 9 種類の Coding Agent バッジを切り替え
- 日別・月別・年別ビュー
- 日付、月、年のクイック切り替え
- 5 秒、10 秒、30 秒間隔の自動ライブ更新
- ローカルログを SQLite に手動同期
- ダークテーマとライトテーマ
- 繁体字中国語、簡体字中国語、英語、日本語、韓国語のインターフェース切り替え
- モデル料金表の表示

* * *

## URL パラメータ（ディープリンク）

ダッシュボードは URL クエリパラメータで特定の状態を直接開くことができ、ブックマークへの追加、リンクの共有、他のツールからの遷移に便利です。ダッシュボード上で Agent、ビュー、日付、作業ディレクトリ、グラフの種類を切り替えると、URL も現在の状態に自動的に更新されます。

| パラメータ | 対象ビュー | 指定できる値 | 説明 |
| --- | --- | --- | --- |
| `agent` | すべて | `antigravity`、`copilot`、`codex`、`claude`、`cursor`、`grok`、`pi`、`omp`、`muse` | 表示する Coding Agent を指定します。`claude-code`、`grok-build`、`pi-coding-agent`、`oh-my-pi`、`muse-code` などのエイリアスも利用可能です |
| `tab` | すべて | `daily`、`monthly`、`yearly` | 日別（daily）、月別（monthly）、年別（yearly）ビューを指定します |
| `date` | すべて | `daily`: `YYYY-MM-DD`、`monthly`: `YYYY-MM`、`yearly`: `YYYY` | 表示する日付・月・年を指定します。形式は `tab` に応じて自動的に対応します |
| `dir` | `daily` | フルパス、`~` で始まるホームディレクトリのパス、または一意のパス末尾（例：`TokenUsageInsights`） | 日別ビューの作業ディレクトリフィルターを指定します。Windows パスは大文字小文字を区別しません。一致するディレクトリがない場合はすべて表示されます |
| `chart` | `daily` | `kline`、`trend` | 日別ビューのグラフの種類（ローソク足チャートまたはトレンドチャート）を指定します |

例（`http://localhost:3003` はデフォルトの URL です。実際の `HOST`/`PORT` に合わせて調整してください）：

```text
http://localhost:3003/?agent=copilot&tab=monthly&date=2026-08
http://localhost:3003/?agent=codex&tab=yearly&date=2026
http://localhost:3003/?agent=claude&tab=daily&date=2026-08-09&chart=trend
http://localhost:3003/?agent=copilot&tab=daily&date=2026-08-09&dir=~/projects/TokenUsageInsights
```

> パスに `~`、スペース、非 ASCII 文字が含まれる場合は URL エンコードしてください（`~` は `%7E` にエンコード可能）。指定されなかったパラメータは前回の閲覧状態（Cookie / localStorage）が引き継がれます。

* * *

## Google Antigravity CLI の設定

Antigravity CLI では、このプロジェクトの Status Line スクリプトを `settings.json` に接続する必要があります。スクリプトは各会話後の累計 Token と増分を次へ書き込みます：

```text
~/.gemini/antigravity-cli/usage/usage-YYYY-MM-DD.jsonl
```

### 1. コレクタースクリプトをインストール

1 行インストールの後、次を実行します：

```bash
mkdir -p ~/.gemini/antigravity-cli && cp ~/.local/share/token-usage-insights/shell/antigravity/statusline-token.sh ~/.gemini/antigravity-cli/statusline-token.sh && chmod +x ~/.gemini/antigravity-cli/statusline-token.sh
```

カスタムインストール先を使用する場合は、コマンド中の `~/.local/share/token-usage-insights` を `TOKEN_USAGE_INSIGHTS_INSTALL_DIR` で指定した場所に置き換えてください。

### 2. `~/.gemini/antigravity-cli/settings.json` を設定

ファイルが存在しない場合は、次の内容で作成できます。既存の場合は `statusLine` ブロックだけを統合し、既存の設定を上書きしないでください。

```json
{
  "statusLine": {
    "type": "command",
    "command": "/ABSOLUTE/HOME/.gemini/antigravity-cli/statusline-token.sh",
    "padding": 1
  }
}
```

`/ABSOLUTE/HOME` を `echo $HOME` で表示される実際のホームディレクトリ（例：`/Users/will` または `/home/will`）に置き換えてください。

### 3. 検証

```bash
echo '{}' | ~/.gemini/antigravity-cli/statusline-token.sh
jq . ~/.gemini/antigravity-cli/settings.json
```

その後 Antigravity CLI Session に入り直すと、Status Line に次のような形式が表示されます：

```text
model-name • #3 • input 12.3k • cache 4.5k/0 • output 1.2k • reasoning 500 • total 18.5k
```

* * *

## GitHub Copilot CLI の設定

Copilot CLI も Antigravity CLI と同様に、このプロジェクトの Status Line スクリプトを `settings.json` に接続する必要があります。スクリプトは Token データを次へ書き込みます：

```text
~/.copilot/usage/usage-YYYY-MM-DD.jsonl
```

### 1. コレクタースクリプトをインストール

1 行インストールの後、次を実行します：

```bash
mkdir -p ~/.copilot && cp ~/.local/share/token-usage-insights/shell/copilot/statusline-token.sh ~/.copilot/statusline-token.sh && chmod +x ~/.copilot/statusline-token.sh
```

カスタムインストール先を使用する場合は、コマンド中の `~/.local/share/token-usage-insights` を `TOKEN_USAGE_INSIGHTS_INSTALL_DIR` で指定した場所に置き換えてください。

### 2. `~/.copilot/settings.json` を設定

ファイルが存在しない場合は、次の内容で作成できます。既存の場合は `statusLine` ブロックだけを統合し、既存の設定を上書きしないでください。

```json
{
  "statusLine": {
    "type": "command",
    "command": "/ABSOLUTE/HOME/.copilot/statusline-token.sh",
    "padding": 1
  }
}
```

`/ABSOLUTE/HOME` を `echo $HOME` で表示される実際のホームディレクトリに置き換えてください。

### 3. 検証

```bash
echo '{}' | ~/.copilot/statusline-token.sh
jq . ~/.copilot/settings.json
```

その後 Copilot CLI Session に入り直すと、Status Line が Token データの出力と蓄積を開始します。

* * *

## GitHub Copilot App（デスクトップアプリ）

**Copilot App（Tauri デスクトップアプリ）に設定は不要です。** ダッシュボードはローカルの `~/.copilot/data.db` と `~/.copilot/session-store.db` を自動的に読み込み、App Session の Token 使用量を CLI / VS Code と Copilot ページで統合表示します。Session 一覧ではソースを `App` と表示し、`CLI`、`VS Code` と区別します。

- バックグラウンド同期（5 秒ごと）のたびに両方の SQLite を確認し、複合カーソル `(created_at, id)` で増分同期します。同じタイムスタンプの複数 event の重複 upsert を防ぎ、同じ `(session_id, turn_index)` は二重に書き込みません。
- App の `assistant_usage_events` は per-API-call 粒度です。ダッシュボードは Session、Turn、Agent、モデル単位で集計し、同一ターン内の複数モデルへの帰属を保持して、タイムラインには per-turn 統計を使用します。
- Session タイトルは `data.db.sessions.title` から取得します。

App と CLI が別ディレクトリにある場合、またはデフォルト以外のディレクトリを使う場合は環境変数を指定できます：

```bash
COPILOT_APP_DIR="/path/to/copilot-app-data" token-usage-insights
```

`COPILOT_APP_DIR` は `COPILOT_DIR` より優先され、未設定時は `~/.copilot` にフォールバックします。

* * *

## GitHub Copilot Chat（VS Code）の設定

**VS Code Copilot Chat に Status Line、Hook、追加の収集スクリプトをインストールする必要はありません。** ダッシュボードはローカルの `workspaceStorage` にあるチャット Session を直接読み込み、Copilot CLI と統合表示します。Session 一覧ではソースを `VS Code` または `CLI` と表示します。

VS Code Stable と Insiders に対応しています：

| プラットフォーム | Stable | Insiders |
| --- | --- | --- |
| Windows | `%APPDATA%\Code\User\workspaceStorage` | `%APPDATA%\Code - Insiders\User\workspaceStorage` |
| macOS | `~/Library/Application Support/Code/User/workspaceStorage` | `~/Library/Application Support/Code - Insiders/User/workspaceStorage` |
| Linux | `~/.config/Code/User/workspaceStorage` | `~/.config/Code - Insiders/User/workspaceStorage` |

使用方法：

1. VS Code で GitHub Copilot Chat を使い、少なくとも 1 つのチャット Session を作成します。
2. ダッシュボードを起動するか、右上の同期ボタンをクリックします。
3. Copilot ページで統合後の統計と Session タイムラインを確認します。

既存の `chatSessions` ファイルは完全に取り込み、ファイルサイズまたは更新日時が変わると再同期します。Token フィールドのないチャット Session も表示されますが、Token 数は 0 です。読み取るのはローカルのチャットファイルだけで、クラウド Session、Remote SSH ホスト、`state.vscdb` は含まれません。

**キャッシュ読み取り Token の取得元**：VS Code の `chatSessions` ファイルには、各リクエストの最後のモデル呼び出しの `promptTokens` と累計の `completionTokens` しか記録されず、Prompt Cache のキャッシュ読み取り数は記録されません。そのためダッシュボードは、同じワークスペースディレクトリに Copilot Chat 拡張機能が書き出すデバッグログ `GitHub.copilot-chat/debug-logs/<sessionId>/main.jsonl` も読み取り、そのターンの全モデル呼び出しの `inputTokens`・`outputTokens`・`cachedTokens` を合計して、非キャッシュ入力・キャッシュ読み取り・出力 Token に分解し、コスト推定にもキャッシュ読み取り単価を適用します。このデバッグログは VS Code 設定 `github.copilot.chat.agentDebugLog.fileLogging.enabled` で制御され（一部ユーザーには実験機能として有効化済み）、既定では最新 50 Session 分のみ保持されます。デバッグログのない Session は VS Code 標準の Token フィールドにフォールバックし、キャッシュ読み取りは 0 と表示されます。

VS Code で `--user-data-dir` または Portable Mode を使う場合は、ダッシュボードのカスタムデータルートを指定できます：

macOS / Linux：

```bash
VSCODE_USER_DATA_DIR="/path/to/vscode-user-data" token-usage-insights
```

Windows PowerShell：

```powershell
$env:VSCODE_USER_DATA_DIR = "C:\path\to\vscode-user-data"; & "$HOME\bin\token-usage-insights.cmd"
```

`VSCODE_USER_DATA_DIR` は `User/workspaceStorage` を含む VS Code ユーザーデータディレクトリを指す必要があります。Portable Mode で環境変数が `data` ディレクトリを指す場合は `VSCODE_PORTABLE_DATA_DIR` を使用してください。ダッシュボードは `data/user-data/User/workspaceStorage` と `data/User/workspaceStorage` の両方を確認します。

* * *

## Codex の設定

**Codex Desktop と Codex CLI のどちらにも Hook、Status Line、追加の収集スクリプトは必要ありません。**

ダッシュボードは次のディレクトリを直接スキャンします：

```text
~/.codex/sessions
~/.codex/archived_sessions
```

使用方法：

1. Codex Desktop または Codex CLI を通常どおり使い、少なくとも 1 つの Session を作成します。
2. このプロジェクトを起動します。
3. 左側で Codex を選択します。
4. 右上の同期ボタンをクリックするか、バックグラウンド同期を待ちます。

注意事項：

- Codex の認証情報は引き続き Codex 自身が管理します。
- ダッシュボードはローカル Session 記録だけを読み取って分析します。
- 各 Session は transcript の `originator` に基づき `Desktop` または `CLI` のソースラベルを表示します。判定できない古い形式は未分類のままです。
- API クォータ情報が表示される場合、そのソースは最新のローカル Session ログであり、リアルタイムのオンライン照会ではありません。

* * *

## Claude Code の設定

**Claude Code に Hook、Status Line、追加の収集スクリプトは必要ありません。**

ダッシュボードは次のディレクトリを直接スキャンします：

```text
~/.claude/projects
```

使用方法：

1. Claude Code を通常どおり使い、少なくとも 1 つのプロジェクト Session を作成します。
2. このプロジェクトを起動します。
3. 左側で Claude Code を選択します。
4. 右上の同期ボタンをクリックするか、バックグラウンド同期を待ちます。

注意事項：

- Claude Code の認証情報は引き続き Claude Code 自身が管理します。
- ダッシュボードはローカルプロジェクト Session 記録だけを読み取って分析します。
- `~/.claude/projects` が存在しない場合、Claude Code ページにはデータがないと表示されます。

* * *

## Cursor の設定

**Cursor に Hook、Status Line、追加の収集スクリプトは必要ありません。** ダッシュボードは次のディレクトリを直接スキャンします：

```text
~/.cursor/projects
```

Cursor は会話と Agent の transcript をプロジェクトディレクトリ配下に保存します。ダッシュボードはプラットフォーム既定の `Cursor/User/globalStorage/state.vscdb` も読み取り専用でスキャンし、その `agentKv` 記録から実際のモデルを帰属します。一意に照合できない Session は `Unknown Model` のまま表示されます。

使用方法：

1. Cursor で少なくとも 1 つの会話または Agent Session を作成します。
2. ダッシュボードを起動または再読み込みします。
3. 左側で Cursor を選択します。
4. 右上の同期ボタンをクリックするか、バックグラウンド同期を待ちます。

Cursor のローカルデータには正確な Token 数や公式の請求明細が含まれないため、Token はテキスト内容から推定します。コストは `pricing.csv` に対応するモデルがある場合のみ推定され、公式請求額とは一致しません。デフォルト以外の場所には `CURSOR_DIR` と `CURSOR_STATE_DB` を使用できます。

* * *

## Grok Build の設定

**Grok Build に Hook、Status Line、追加の収集スクリプトは必要ありません。** ダッシュボードは次のディレクトリを直接スキャンします：

```text
~/.grok/sessions
```

Grok Build が内部保存する Session stream を使用します。旧形式の
`~/.Grok/build/usage/usage-YYYY-MM-DD.jsonl` は読み取らず、`~/.Grok/build/settings.json` に
`statusLine` を設定する必要もありません。

使用方法：

1. Grok Build を通常どおり使い、少なくとも 1 つの Session を作成します。
2. このプロジェクトを起動します。
3. 左側で Grok Build を選択します。
4. 右上の同期ボタンをクリックするか、バックグラウンド同期を待ちます。

Grok Build Session は context token snapshot だけを提供する場合も、provider usage とコストを含む場合もあります。ダッシュボードは provider usage/cost を優先します。context snapshot だけの場合は、`pricing.csv` の xAI API 価格でコストを推定し、Session 一覧に `Context` と表示します。これは SuperGrok や他のサブスクリプションプランの週間クォータを意味しません。

* * *

## Pi Coding Agent の設定

**Pi Coding Agent に Hook、Status Line、追加の収集スクリプトは必要ありません。** ダッシュボードは次のディレクトリを直接スキャンします：

```text
~/.pi/agent/sessions
```

Pi Coding Agent はツリー構造のディレクトリ配下に Session をローカル JSONL ファイルとして自動保存し、ダッシュボードはそれらの Session 記録を直接読み取ります。

使用方法：

1. Pi Coding Agent を通常どおり使い、少なくとも 1 つの Session を作成します。
2. ダッシュボードを起動または再読み込みします。
3. 左側で Pi Coding Agent を選択します。
4. 右上の同期ボタンをクリックするか、バックグラウンド同期を待ちます。

Pi Coding Agent のコストは、各 Session が各 turn ごとに報告する `usage.cost` と関連 usage データから常に直接読み取られます。Pi は turn ごとの権威ある token / cost 情報をネイティブに提供するため、Grok Build のような context snapshot 推定へのフォールバックはありません。

* * *

## OMP の設定

**OMP に Hook、Status Line、追加の収集スクリプトは必要ありません。** ダッシュボードは次のディレクトリを直接スキャンします：

```text
~/.omp/agent/sessions
```

OMP は Pi Coding Agent のオープンソースフォーク（<https://github.com/can1357/oh-my-pi>）で、まったく同じ JSONL 形式で Session を永続化します。ダッシュボードはそれらのローカル Session 記録を直接読み取ります。

使用方法：

1. OMP を通常どおり使い、少なくとも 1 つの Session を作成します。
2. ダッシュボードを起動または再読み込みします。
3. 左側で OMP を選択します。
4. 右上の同期ボタンをクリックするか、バックグラウンド同期を待ちます。

OMP のコストは、各 Session が各 turn ごとに報告する `usage.cost` と関連 usage データから常に直接読み取られます。OMP は turn ごとの権威ある token / cost 情報をネイティブに提供するため、Grok Build のような context snapshot 推定へのフォールバックはありません。

* * *

## Muse Code の設定

**Muse Code に Hook、Status Line、追加の収集スクリプトは必要ありません。** ダッシュボードは次のディレクトリを直接スキャンします：

```text
~/.local/share/muse/sessions
```

Muse Code は Session を日付と Session ID ごとの `session.jsonl` として保存します。ダッシュボードはモデル完了イベントを解析し、入力、キャッシュ読み取り、出力、推論 Token を分けて集計するとともに、ユーザープロンプト、ツール手順、Agent 応答のタイムラインを復元します。

使用方法：

1. Muse Code を通常どおり使い、少なくとも 1 つの Session を作成します。
2. ダッシュボードを起動または再読み込みします。
3. 左側で Muse Code を選択します。
4. 右上の同期ボタンをクリックするか、バックグラウンド同期を待ちます。

Muse Code のコストは、Session が報告するモデルと `pricing.csv` から推定します。データがデフォルトの場所にない場合は、`MUSE_DIR` に `sessions` を含む Muse Code データディレクトリを指定します。

* * *

## ローカルデータの同期方法

サービス起動時にバックエンドがローカル SQLite を初期化し、直ちに 1 回同期します。起動後は 5 秒ごとにバックグラウンド同期も行います。

SQLite のデフォルト位置：

```text
~/.token-usage-insights/token_usage_insights.db
```

フロントエンド右上の同期ボタンは次を呼び出します：

```text
GET /api/:assistant/sync
```

これによりローカルログの完全な増分同期が実行されます。

## インポート / エクスポート（マシン間集約）

**通常はダッシュボード右上のエクスポートとインポートボタンを使用してください。** インストール版はブラウザーだけでマシン間のデータを集約でき、最大 200 MB のインポートファイルに対応します。

v0.9.0 以降、ダッシュボードと CLI は `token-usage-insights` に統合されています。引数なしでダッシュボードを起動し、`export`、`export-all`、`import` でデータを操作できます。各コマンドは `--help` と `-h` に対応します。旧版では更新またはソースからのビルドが必要です。

`--agent` はアシスタント（`antigravity` / `copilot` / `codex` / `claude` / `cursor` / `grok` / `pi` / `omp` / `muse`）を指定します。

### ソースから CLI を使用

最初に 1 回ビルドします：

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

データ形式はフロントエンドと同じで、次のフィールドを含みます：

- `version`
- `assistant`
- `date`
- `exported_at`
- `records`（各レコードに `import_source_id` が含まれます）

`import_source_id` は `assistant_type` と組み合わせて一意キーになります。同じレコードを再インポートすると重複として検出され自動的にスキップされるため、データベースに二重登録されません。

* * *

## 環境変数

環境変数で指定したパスが正式な設定となり、事前に作成する必要はありません。`INSIGHTS_DIR` は起動時に自動作成されます。ネイティブの絶対パス・相対パス、および `~`、`$HOME`、`%USERPROFILE%`、`%LOCALAPPDATA%`、`%APPDATA%` で始まる一般的な形式に対応します。

| 変数 | デフォルト値 | 用途 |
| --- | --- | --- |
| `HOST` | `0.0.0.0` | ダッシュボードサービスがバインドする IPv4 または IPv6 アドレス |
| `PORT` | `3003` | ダッシュボードサービスのポート番号 |
| `INSIGHTS_DIR` | Windows: `%LOCALAPPDATA%\TokenUsageInsights`; その他のプラットフォーム: `~/.token-usage-insights` | SQLite データベースディレクトリ |
| `ANTIGRAVITY_DIR` | `~/.gemini/antigravity-cli` | Antigravity CLI データディレクトリ |
| `COPILOT_DIR` | `~/.copilot` | Copilot CLI データディレクトリ |
| `COPILOT_APP_DIR` | `COPILOT_DIR` と同じ | Copilot App（デスクトップアプリ）のデータディレクトリ。`data.db` と `session-store.db` を含む必要があります |
| `VSCODE_USER_DATA_DIR` | プラットフォームにより自動検出 | VS Code ユーザーデータディレクトリ。`User/workspaceStorage` を含む必要があります |
| `VSCODE_PORTABLE_DATA_DIR` | 未設定 | VS Code Portable Mode の `data` ディレクトリ |
| `CODEX_DIR` | `~/.codex` | Codex Desktop と Codex CLI が共有するデータディレクトリ |
| `CLAUDE_DIR` | `~/.claude` | Claude Code データディレクトリ |
| `CURSOR_DIR` | `~/.cursor` | Cursor データディレクトリ |
| `CURSOR_STATE_DB` | プラットフォームにより自動検出 | Cursor `User/globalStorage/state.vscdb` のパス。読み取り専用で `agentKv` モデル情報を取得するために使用 |
| `GROK_DIR` | `~/.grok` | Grok Build データディレクトリ |
| `PI_DIR` | `~/.pi` | Pi Coding Agent データディレクトリ |
| `OMP_DIR` | `~/.omp` | OMP データディレクトリ |
| `MUSE_DIR` | `~/.local/share/muse` | Muse Code データディレクトリ。`sessions` を含む必要があります |
| `CORS_ALLOWED_ORIGINS` | `http://localhost:<PORT>,http://127.0.0.1:<PORT>` | カンマ区切りの許可 CORS オリジン |

> **デフォルトのバインド先は `0.0.0.0` で、同じローカルネットワーク上の他のデバイスからダッシュボードに接続できる可能性があります。ローカルだけで閲覧する場合は `HOST` を `127.0.0.1` に設定してください。**

例：

```bash
HOST="127.0.0.1" INSIGHTS_DIR="/tmp/token-usage-insights" PORT="3010" "$HOME/.local/bin/token-usage-insights"
```

Windows PowerShell の例：

```powershell
$env:HOST = '127.0.0.1'; $env:INSIGHTS_DIR = 'D:\Token Usage Insights\資料庫'; $env:CODEX_DIR = "$env:USERPROFILE\.codex"; $env:PORT = '3010'; & "$HOME\bin\token-usage-insights.cmd"
```

* * *

## 常駐サービス

### Linux：1 行で systemd ユーザーサービスをインストールして有効化

```bash
curl -fsSL https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.sh | bash -s -- --service
```

これはインストール版をダウンロードして `token-usage-insights.service` を直ちに有効化します。systemd ファイルを自分でビルドまたは編集する必要はありません。

### macOS：1 行で launchd LaunchAgent をインストールして有効化

```bash
curl -fsSL https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.sh | bash -s -- --service
```

これは `com.tokenusageinsights.plist` を `~/Library/LaunchAgents/` にインストールして直ちにロードします。標準出力とエラーログは `~/Library/Logs/` に出力されます。

### Windows：1 行でバックグラウンド常駐サービス（タスクスケジューラ）をインストールして有効化

```powershell
& ([scriptblock]::Create((irm https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.ps1))) -Service
```

これは Windows タスクスケジューラ（Task Scheduler）に `TokenUsageInsights` タスクを登録して直ちに起動します。ユーザーログイン時に自動的にバックグラウンドで実行され、ログは `%LOCALAPPDATA%\TokenUsageInsights\logs\` に出力されます。

### サービスを管理

Linux：

```bash
systemctl --user status token-usage-insights.service
journalctl --user -u token-usage-insights.service -n 50 -f
systemctl --user restart token-usage-insights.service
systemctl --user stop token-usage-insights.service
```

macOS：

```bash
launchctl print gui/$(id -u)/com.tokenusageinsights
launchctl kickstart -k gui/$(id -u)/com.tokenusageinsights
launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.tokenusageinsights.plist
```

Windows PowerShell：

```powershell
# サービス状態を確認（タスクスケジューラまたはバックグラウンドプロセス）
Get-ScheduledTask -TaskName "TokenUsageInsights" -ErrorAction SilentlyContinue
Get-Process -Name "token-usage-insights" -ErrorAction SilentlyContinue

# ログをリアルタイム確認
Get-Content "$env:LOCALAPPDATA\TokenUsageInsights\logs\token-usage-insights.out.log" -Tail 50 -Wait

# サービスを再起動（タスクスケジューラとスタートアップフォルダの両方に対応）
Stop-ScheduledTask -TaskName "TokenUsageInsights" -ErrorAction SilentlyContinue
Get-Process -Name "token-usage-insights" -ErrorAction SilentlyContinue | Stop-Process -Force
$StartupShortcut = Join-Path ([Environment]::GetFolderPath('Startup')) "token-usage-insights.lnk"
if (!(Test-Path $StartupShortcut)) {
    $StartupShortcut = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Startup\token-usage-insights.lnk"
}
if (Get-ScheduledTask -TaskName "TokenUsageInsights" -ErrorAction SilentlyContinue) {
    Start-ScheduledTask -TaskName "TokenUsageInsights"
} elseif (Test-Path $StartupShortcut) {
    Start-Process $StartupShortcut
}

# サービスを停止
Stop-ScheduledTask -TaskName "TokenUsageInsights" -ErrorAction SilentlyContinue
Get-Process -Name "token-usage-insights" -ErrorAction SilentlyContinue | Stop-Process -Force

# 常駐サービスを登録解除
Unregister-ScheduledTask -TaskName "TokenUsageInsights" -Confirm:$false -ErrorAction SilentlyContinue
$StartupShortcut = Join-Path ([Environment]::GetFolderPath('Startup')) "token-usage-insights.lnk"
if (!(Test-Path $StartupShortcut)) {
    $StartupShortcut = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Startup\token-usage-insights.lnk"
}
Remove-Item $StartupShortcut -Force -ErrorAction SilentlyContinue
Get-Process -Name "token-usage-insights" -ErrorAction SilentlyContinue | Stop-Process -Force
```

* * *

## インストールオプションと手動インストール

GitHub Release では Linux、macOS、Windows 用のコンパイル済み実行ファイルを提供しています。インストールと実行に Rust や Cargo は必要ありません。

### 1 行インストーラーのオプション引数

`scripts/get.sh`（Linux / macOS）と `scripts/get.ps1`（Windows）は、プラットフォームと CPU アーキテクチャを自動判定し、最新（または指定した）Release から対応するアーカイブをダウンロードして展開し、パッケージ内の `install.sh` / `install.ps1` を呼び出します。手動のダウンロードや展開は不要です：

Linux / macOS：

```bash
curl -fsSL https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.sh | bash
```

Linux（systemd user service）または macOS（launchd LaunchAgent）で常駐サービスも同時にインストールして有効化する場合：

```bash
curl -fsSL https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.sh | bash -s -- --service
```

Windows PowerShell：

```powershell
irm https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.ps1 | iex
```

Windows PowerShell で常駐サービスも同時にインストールして有効化する場合：

```powershell
& ([scriptblock]::Create((irm https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.ps1))) -Service
```

インストール後に実行します（Linux/macOS では `bin_dir` が `PATH` に含まれることを確認してください。Windows では `.cmd` shim が作成されます）：

```bash
token-usage-insights
```

環境変数でバージョンとインストール先を指定できます（すべて任意）：

| 変数 | 対応プラットフォーム | 説明 |
| --- | --- | --- |
| `TOKEN_USAGE_INSIGHTS_VERSION` | Linux / macOS / Windows | `v0.9.3` のようなインストール対象の Release tag。デフォルトは `latest` |
| `TOKEN_USAGE_INSIGHTS_INSTALL_DIR` | Linux / macOS | `install.sh` に渡すインストールディレクトリ |
| `TOKEN_USAGE_INSIGHTS_BIN_DIR` | Linux / macOS | `install.sh` に渡す実行ファイルリンクディレクトリ |

Windows でインストール先、bin ディレクトリ、ポートをカスタマイズする場合は、先にスクリプトをダウンロードして引数付きで実行してください（`iex` パイプラインは引数に対応しません）：

```powershell
Invoke-WebRequest -Uri https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.ps1 -OutFile get.ps1
.\get.ps1 -InstallDir 'D:\Apps\Token Usage Insights' -Port 3010
```

### 手動ダウンロードとインストール

リモートスクリプトを直接実行したくない場合は、対応プラットフォームのアーカイブを手動でダウンロードし、パッケージ内のインストールスクリプトを実行できます。各 Release アーカイブには次が含まれます：

- 単一プラットフォーム用の実行ファイル
- `static/` のフロントエンドアセット
- モデル料金表 `pricing.csv`
- `shell/` の Status Line およびサービススクリプト
- `scripts/` ディレクトリ（`install.sh`、`install.ps1`、`get.sh`、`get.ps1`、`run-service.ps1` を含む）
- README、LICENSE、VERSION

Linux または macOS：

```bash
tar -xzf token-usage-insights-<tag>-<target>.tar.gz
cd token-usage-insights-<tag>-<target>
./install.sh
```

Linux（systemd user service）または macOS（launchd LaunchAgent）で常駐サービスをインストールして有効化する場合：

```bash
./install.sh --service
```

Windows：

```powershell
Expand-Archive token-usage-insights-<tag>-x86_64-pc-windows-msvc.zip
cd token-usage-insights-<tag>-x86_64-pc-windows-msvc
powershell -ExecutionPolicy Bypass -File .\install.ps1
```

Windows でバックグラウンド常駐サービスをインストールして有効化する場合：

```powershell
powershell -ExecutionPolicy Bypass -File .\install.ps1 -Service
```

Windows のインストール先とポートをカスタマイズ：

```powershell
.\install.ps1 -InstallDir 'D:\Apps\Token Usage Insights' -BinDir "$HOME\bin" -Port 3010
```

### CI 検証

`Release` workflow は各ビルドで Linux、macOS、Windows 上の対応するインストールスクリプト（`install.sh` / `install.ps1`）を実行し、インストール後に実行ファイルを起動して次を確認します：

- 指定したポートでサービスが `/api/<assistant>/pricing` に応答する
- 応答内容がパッケージ内の `pricing.csv` を実際に読み込んでいる
- 新しい `INSIGHTS_DIR` が作成され、SQLite データベースが生成される

`get.sh` と `get.ps1` も各ビルド前に構文チェック（`bash -n` と PowerShell AST 解析）を受け、Release に公開されるバージョンが正常に実行できることを保証します。

### メンテナーによるリリース

Git tag をプッシュすると、GitHub Actions が対応する Release を自動作成します：

```bash
git tag vX.Y.Z
git push origin vX.Y.Z
```

* * *

## 旧データの移行

以前に次のスタンドアロンプロジェクトを使用していた場合、本プロジェクトの起動時に古い SQLite データの移行を自動的に試みます：

- `~/.gemini/antigravity-cli/antigravity_cli_token_insights.db`
- `~/.copilot/copilot_cli_token_insights.db`
- `~/.codex/codex_cli_token_insights.db`

移行に成功すると、古いデータベースは `.bak` にリネームされます。

データ移行が完了したことを確認したら、旧サービスを停止できます：

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

## トラブルシューティング

### ダッシュボードにデータがない

ツールごとにデータソースが存在するか確認します：

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

Antigravity CLI と Copilot CLI では、`settings.json` に `statusLine` が設定され、スクリプトに実行権限があることも確認してください。

Windows PowerShell ではネイティブデータディレクトリを直接確認できます：

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

### Status Line スクリプトを実行できない

```bash
command -v jq
chmod +x ~/.gemini/antigravity-cli/statusline-token.sh
chmod +x ~/.copilot/statusline-token.sh
```

Status Line スクリプトは CLI から渡される JSON の解析に `jq` を使用します。

上記の `jq` 要件は `.sh` collector のみに適用されます。Windows の `.ps1` collector は次のコマンドでテストできます。バックスラッシュや空白を含むパスもネイティブに処理します：

```powershell
Write-Output '{}' | powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$env:USERPROFILE\.gemini\antigravity-cli\statusline-token.ps1" -Assistant antigravity
```

### 設定ファイルの JSON 形式が不正

```bash
jq . ~/.gemini/antigravity-cli/settings.json
jq . ~/.copilot/settings.json
```

他の設定がある場合は、ファイル全体を配列や単なる文字列に置き換えず、`statusLine` オブジェクトを統合してください。

### `localhost:3003` に接続できない

```bash
PORT=3010 "$HOME/.local/bin/token-usage-insights"
```

別のポートを使用する場合は、対応する URL を開きます。例：

```text
http://localhost:3010
```

* * *

## 開発コマンド

このセクションはソースコードを変更またはビルドする開発者向けです。通常の利用では前述の 1 行インストールコマンドを使用してください。

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

## プロジェクトファイル

```text
src/                 Rust 後端、API、SQLite 同步、價格與時間軸解析
static/              前端 HTML、JavaScript、CSS 與圖片資產
shell/               Bash/PowerShell Status Line collector 與 systemd 服務範本
scripts/             Linux/macOS、Windows 安裝與 Windows smoke test
pricing.csv          模型價格表，本地估算費用依此檔案載入
```

* * *

## スクリーンショット

![Token 戦情室の日次ダッシュボード](screenshots/codex-daily-2026-07-07-desktop-chrome.png)

![Token 戦情室の月次ダッシュボード](screenshots/codex-daily-2026-07-07.png)

![Token 戦情室の Session タイムライン](screenshots/codex-daily-2026-07-07-desktop-chrome.png)
