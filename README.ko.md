# Token 전황실

**Token 전황실은 로컬 우선 방식의 AI Coding Agent Token 사용량 및 세션 복원 대시보드입니다.** Google Antigravity CLI, GitHub Copilot CLI, GitHub Copilot App, GitHub Copilot Chat(VS Code), Codex Desktop, Codex CLI, Claude Code, Cursor, Grok Build, Pi Coding Agent, OMP, Muse Code의 로컬 기록을 읽어 일별·월별·연별 Token 소비량, 캐시 사용량, 추론 Token, 예상 비용, 모델 분포, 프로젝트 디렉터리 분포와 전체 Session 타임라인을 한곳에 표시합니다.

이 프로젝트는 AI 공급자 API를 대신 호출하여 데이터를 조회하지 않습니다. 핵심 데이터 원본은 로컬 로그, Status Line 수집 파일, 로컬 SQLite입니다.

> 시스템 환경: Windows 10/11 네이티브 PowerShell, macOS, Linux 및 WSL을 지원합니다.

언어: [繁體中文](README.md) · [简体中文](README.zh-CN.md) · [English](README.en.md) · [日本語](README.ja.md) · [한국어](README.ko.md)

* * *

## 가장 빠른 시작 방법

### 1. 한 줄로 대시보드 시작 또는 설치

Node.js 18.18 이상이 설치되어 있다면 전역 npm 명령을 만들지 않고 바로 실행할 수 있습니다.

```bash
npx --yes token-usage-insights
```

고정된 시스템 명령으로 설치하려면 플랫폼별 설치 프로그램을 사용하세요.

Linux / macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.sh | bash && "$HOME/.local/bin/token-usage-insights"
```

Windows PowerShell:

```powershell
irm https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.ps1 | iex; & "$HOME\bin\token-usage-insights.cmd"
```

`npx`와 설치 프로그램은 모두 현재 플랫폼에 맞는 컴파일된 버전을 다운로드합니다. Rust, Cargo, WSL 또는 수동 압축 해제가 필요하지 않습니다. 명령이 시작되면 대시보드가 로컬에서 실행됩니다.

열기:

```text
http://localhost:3003
```

### 2. 사용하는 도구에 따라 추가 설정 필요 여부 확인

| 도구 | 추가 설정 | 기본 데이터 원본 | 설명 |
| --- | --- | --- | --- |
| Google Antigravity CLI | 필요 | `~/.gemini/antigravity-cli/usage/usage-YYYY-MM-DD.jsonl` | `statusline-token.sh` 또는 Windows의 `statusline-token.ps1`로 Token 데이터를 수집 |
| GitHub Copilot CLI | 필요 | `~/.copilot/usage/usage-YYYY-MM-DD.jsonl` | `statusline-token.sh` 또는 Windows의 `statusline-token.ps1`로 Token 데이터를 수집 |
| GitHub Copilot App | 불필요 | `~/.copilot/data.db`, `~/.copilot/session-store.db` | 대시보드가 데스크톱 앱의 로컬 SQLite를 직접 읽음 |
| GitHub Copilot Chat(VS Code) | 불필요 | VS Code `workspaceStorage/chatSessions` | VS Code Stable 및 Insiders의 로컬 채팅 Session을 직접 스캔 |
| Codex Desktop / CLI | 불필요 | `~/.codex/sessions`, `~/.codex/archived_sessions` | Codex의 활성 및 보관된 로컬 Session 기록을 직접 스캔 |
| Claude Code | 불필요 | `~/.claude/projects` | Claude Code의 로컬 프로젝트 Session 기록을 직접 스캔 |
| Cursor | 불필요 | `~/.cursor/projects` | Cursor 로컬 transcript를 스캔하고 귀속 가능한 모델 정보를 읽기 전용으로 조회 |
| Grok Build | 불필요 | `~/.grok/sessions` | Grok Build가 자동 저장하는 `updates.jsonl` Session stream을 직접 스캔 |
| Pi Coding Agent | 불필요 | `~/.pi/agent/sessions` | Pi Coding Agent가 자동 저장하는 로컬 Session JSONL 파일을 직접 스캔 |
| OMP | 불필요 | `~/.omp/agent/sessions` | OMP가 자동 저장하는 로컬 Session JSONL 파일을 직접 스캔 |
| Muse Code | 불필요 | `~/.local/share/muse/sessions` | Muse Code가 자동 저장하는 로컬 Session JSONL 파일을 직접 스캔 |

**Copilot App, VS Code Copilot, Codex Desktop, Codex CLI, Claude Code, Cursor, Grok Build, Pi Coding Agent, OMP 또는 Muse Code만 사용하는 경우 한 줄 설치 명령을 실행하고 대시보드를 열기만 하면 됩니다.**

### Windows 네이티브 사용

Windows 한 줄 설치는 `%USERPROFILE%\bin\token-usage-insights.cmd` 실행 파일을 만듭니다. Rust MSVC toolchain, Visual Studio Build Tools, WSL, Git Bash 또는 `jq`가 필요하지 않습니다.

Windows는 기본적으로 다음 네이티브 경로를 사용합니다.

| 용도 | Windows 기본 경로 |
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

대시보드의 설정 안내는 Windows에서 PowerShell 복사, 설정 및 진단 명령을 표시합니다. PowerShell collector는 .NET JSON 및 파일 API를 사용하며 Bash, `jq`, `sed`, `awk`에 의존하지 않습니다.

드라이브 문자, 공백이나 비 ASCII 문자가 포함된 경로, UNC 경로는 모두 네이티브 경로 API로 처리됩니다. 네트워크 공유의 locking 의미 차이를 피하려면 SQLite 데이터베이스는 로컬 디스크에 두는 것이 좋습니다.

* * *

## 지원 기능

### 데이터 분석

- 일별·월별·연별 Token 통계
- 입력, 출력, 캐시 읽기, 캐시 쓰기, 추론 Token 분류
- `pricing.csv`에 따른 로컬 비용 추정
- Session 수, 요청 수 및 API 소요 시간 통계
- 모델 사용량 순위
- 로컬 `state.vscdb`의 `agentKv` 기록으로 Cursor를 구체적인 모델에 귀속하며, 고유하게 일치하지 않으면 `Unknown Model`로 유지
- 프로젝트 작업 디렉터리 통계
- 정렬 가능한 Session 목록
- GitHub Copilot App(데스크톱 앱)의 `~/.copilot/data.db`와 `session-store.db` 자동 읽기

### Session 복원

- 오른쪽 서랍에 표시되는 Session 타임라인
- 사용자 프롬프트, 어시스턴트 응답, 추론 내용 및 도구 호출 단계
- 도구 호출 인수, 종료 코드, stdout, stderr
- parent session, agent nickname, agent role 등의 Codex subagent 필드
- Markdown 응답 렌더링 및 콘텐츠 정리

### 인터페이스

- 9가지 Coding Agent 배지 전환
- 일별·월별·연별 보기
- 날짜·월·연도 빠른 전환
- 5초, 10초, 30초 간격의 실시간 자동 새로 고침
- 로컬 로그를 SQLite에 수동 동기화
- 어두운 테마와 밝은 테마
- 번체 중국어, 간체 중국어, 영어, 일본어 및 한국어 인터페이스 전환
- 모델 가격표 보기

* * *

## URL 매개변수(딥 링크)

대시보드는 URL 쿼리 매개변수로 특정 상태를 바로 열 수 있어서, 북마크에 추가하거나 링크를 공유하거나 다른 도구에서 바로 이동하기에 편리합니다. 대시보드에서 Agent, 보기, 날짜, 작업 디렉터리, 차트 유형을 전환하면 URL도 현재 상태로 자동 업데이트됩니다.

| 매개변수 | 적용 보기 | 사용 가능한 값 | 설명 |
| --- | --- | --- | --- |
| `agent` | 전체 | `antigravity`, `copilot`, `codex`, `claude`, `cursor`, `grok`, `pi`, `omp`, `muse` | 표시할 Coding Agent를 지정합니다. `claude-code`, `grok-build`, `pi-coding-agent`, `oh-my-pi`, `muse-code` 같은 별칭도 지원합니다 |
| `tab` | 전체 | `daily`, `monthly`, `yearly` | 일별(daily), 월별(monthly), 연별(yearly) 보기를 지정합니다 |
| `date` | 전체 | `daily`: `YYYY-MM-DD`, `monthly`: `YYYY-MM`, `yearly`: `YYYY` | 표시할 날짜·월·연도를 지정하며, 형식은 `tab`에 따라 자동으로 매핑됩니다 |
| `dir` | `daily` | 전체 경로, `~`로 시작하는 홈 디렉터리 경로, 또는 고유한 경로 접미사(예: `TokenUsageInsights`) | 일별 보기의 작업 디렉터리 필터를 지정합니다. Windows 경로는 대소문자를 구분하지 않으며, 일치하는 디렉터리가 없으면 전체를 표시합니다 |
| `chart` | `daily` | `kline`, `trend` | 일별 보기의 차트 유형(캔들차트 또는 추세 차트)을 지정합니다 |

예시(`http://localhost:3003`은 기본 URL이며, 실제 `HOST`/`PORT`에 맞게 조정하세요):

```text
http://localhost:3003/?agent=copilot&tab=monthly&date=2026-08
http://localhost:3003/?agent=codex&tab=yearly&date=2026
http://localhost:3003/?agent=claude&tab=daily&date=2026-08-09&chart=trend
http://localhost:3003/?agent=copilot&tab=daily&date=2026-08-09&dir=~/projects/TokenUsageInsights
```

> 경로에 `~`, 공백 또는 비 ASCII 문자가 포함된 경우 URL 인코딩을 먼저 적용하세요(`~`는 `%7E`로 인코딩 가능). 지정하지 않은 매개변수는 마지막으로 사용한 상태(Cookie / localStorage)를 이어받습니다.

* * *

## Google Antigravity CLI 설정

Antigravity CLI는 이 프로젝트의 Status Line 스크립트를 `settings.json`에 연결해야 합니다. 스크립트는 각 대화 후 누적 Token과 증분을 다음 위치에 기록합니다.

```text
~/.gemini/antigravity-cli/usage/usage-YYYY-MM-DD.jsonl
```

### 1. 수집 스크립트 설치

한 줄 설치가 끝난 후 다음을 실행합니다.

```bash
mkdir -p ~/.gemini/antigravity-cli && cp ~/.local/share/token-usage-insights/shell/antigravity/statusline-token.sh ~/.gemini/antigravity-cli/statusline-token.sh && chmod +x ~/.gemini/antigravity-cli/statusline-token.sh
```

사용자 지정 설치 위치를 사용한다면 명령의 `~/.local/share/token-usage-insights`를 `TOKEN_USAGE_INSIGHTS_INSTALL_DIR`로 지정한 위치로 바꾸세요.

### 2. `~/.gemini/antigravity-cli/settings.json` 설정

파일이 없다면 다음 내용으로 만들 수 있습니다. 이미 존재한다면 `statusLine` 블록만 병합하고 기존 설정을 덮어쓰지 마세요.

```json
{
  "statusLine": {
    "type": "command",
    "command": "/ABSOLUTE/HOME/.gemini/antigravity-cli/statusline-token.sh",
    "padding": 1
  }
}
```

`/ABSOLUTE/HOME`을 `echo $HOME`에 표시되는 실제 홈 디렉터리 경로(예: `/Users/will` 또는 `/home/will`)로 바꾸세요.

### 3. 확인

```bash
echo '{}' | ~/.gemini/antigravity-cli/statusline-token.sh
jq . ~/.gemini/antigravity-cli/settings.json
```

그 후 Antigravity CLI Session에 다시 들어가면 상태 표시줄에 다음과 비슷한 형식이 출력됩니다.

```text
model-name • #3 • input 12.3k • cache 4.5k/0 • output 1.2k • reasoning 500 • total 18.5k
```

* * *

## GitHub Copilot CLI 설정

Copilot CLI도 Antigravity CLI와 마찬가지로 이 프로젝트의 Status Line 스크립트를 `settings.json`에 연결해야 합니다. 스크립트는 Token 데이터를 다음 위치에 기록합니다.

```text
~/.copilot/usage/usage-YYYY-MM-DD.jsonl
```

### 1. 수집 스크립트 설치

한 줄 설치가 끝난 후 다음을 실행합니다.

```bash
mkdir -p ~/.copilot && cp ~/.local/share/token-usage-insights/shell/copilot/statusline-token.sh ~/.copilot/statusline-token.sh && chmod +x ~/.copilot/statusline-token.sh
```

사용자 지정 설치 위치를 사용한다면 명령의 `~/.local/share/token-usage-insights`를 `TOKEN_USAGE_INSIGHTS_INSTALL_DIR`로 지정한 위치로 바꾸세요.

### 2. `~/.copilot/settings.json` 설정

파일이 없다면 다음 내용으로 만들 수 있습니다. 이미 존재한다면 `statusLine` 블록만 병합하고 기존 설정을 덮어쓰지 마세요.

```json
{
  "statusLine": {
    "type": "command",
    "command": "/ABSOLUTE/HOME/.copilot/statusline-token.sh",
    "padding": 1
  }
}
```

`/ABSOLUTE/HOME`을 `echo $HOME`에 표시되는 실제 홈 디렉터리 경로로 바꾸세요.

### 3. 확인

```bash
echo '{}' | ~/.copilot/statusline-token.sh
jq . ~/.copilot/settings.json
```

그 후 Copilot CLI Session에 다시 들어가면 상태 표시줄이 Token 데이터를 출력하고 누적하기 시작합니다.

* * *

## GitHub Copilot App(데스크톱 앱)

**Copilot App(Tauri 데스크톱 앱)은 설정이 필요하지 않습니다.** 대시보드는 로컬 `~/.copilot/data.db`와 `~/.copilot/session-store.db`를 자동으로 읽어 App session의 token 사용량을 CLI / VS Code와 Copilot 페이지에서 통합 표시합니다. Session 목록에서는 소스를 `App`으로 표시하며 `CLI`, `VS Code`와 구분합니다.

- 대시보드는 5초마다 백그라운드 동기화를 수행할 때마다 두 SQLite를 확인하고 `(created_at, id)` 복합 커서로 증분 동기화합니다. 같은 타임스탬프의 여러 event가 중복 upsert되는 것을 방지하며 동일한 `(session_id, turn_index)`는 두 번 기록되지 않습니다.
- App의 `assistant_usage_events`는 per-API-call 단위입니다. 대시보드는 Session, Turn, Agent 및 모델별로 집계하고 같은 턴의 다중 모델 귀속을 보존한 뒤 타임라인에는 per-turn 통계를 사용합니다.
- Session 제목은 `data.db.sessions.title`에서 가져옵니다.

App과 CLI가 분리되어 있거나 기본이 아닌 디렉터리를 사용한다면 환경 변수를 지정할 수 있습니다.

```bash
COPILOT_APP_DIR="/path/to/copilot-app-data" token-usage-insights
```

`COPILOT_APP_DIR`은 `COPILOT_DIR`보다 우선하며 설정하지 않으면 `~/.copilot`으로 fallback합니다.

* * *

## GitHub Copilot Chat(VS Code) 설정

**VS Code Copilot Chat에는 Status Line, Hook 또는 추가 수집 스크립트를 설치할 필요가 없습니다.** 대시보드는 로컬 `workspaceStorage`의 채팅 Session을 직접 읽어 Copilot CLI와 통합 표시하며, Session 목록에는 소스를 `VS Code` 또는 `CLI`로 표시합니다.

VS Code Stable 및 Insiders를 지원합니다.

| 플랫폼 | Stable | Insiders |
| --- | --- | --- |
| Windows | `%APPDATA%\Code\User\workspaceStorage` | `%APPDATA%\Code - Insiders\User\workspaceStorage` |
| macOS | `~/Library/Application Support/Code/User/workspaceStorage` | `~/Library/Application Support/Code - Insiders/User/workspaceStorage` |
| Linux | `~/.config/Code/User/workspaceStorage` | `~/.config/Code - Insiders/User/workspaceStorage` |

사용 방법:

1. VS Code에서 GitHub Copilot Chat을 사용해 채팅 Session을 하나 이상 만듭니다.
2. 대시보드를 시작하거나 오른쪽 위 동기화 버튼을 클릭합니다.
3. Copilot 페이지에서 통합된 통계와 Session 타임라인을 확인합니다.

대시보드는 기존 `chatSessions` 파일을 모두 채우고 파일 크기나 수정 시간이 변경되면 다시 동기화합니다. Token 필드가 없는 채팅 Session도 표시되지만 Token 수는 0입니다. 로컬 채팅 파일만 읽으며 클라우드 Session, Remote SSH 호스트 또는 `state.vscdb`는 포함하지 않습니다.

VS Code에서 `--user-data-dir` 또는 Portable Mode를 사용하는 경우 대시보드의 사용자 지정 데이터 루트를 지정할 수 있습니다.

macOS / Linux:

```bash
VSCODE_USER_DATA_DIR="/path/to/vscode-user-data" token-usage-insights
```

Windows PowerShell:

```powershell
$env:VSCODE_USER_DATA_DIR = "C:\path\to\vscode-user-data"; & "$HOME\bin\token-usage-insights.cmd"
```

`VSCODE_USER_DATA_DIR`은 `User/workspaceStorage`를 포함하는 VS Code 사용자 데이터 디렉터리를 가리켜야 합니다. Portable Mode에서 환경 변수가 `data` 디렉터리를 가리키면 `VSCODE_PORTABLE_DATA_DIR`을 사용하세요. 대시보드는 `data/user-data/User/workspaceStorage`와 `data/User/workspaceStorage`를 모두 확인합니다.

* * *

## Codex 설정

**Codex Desktop과 Codex CLI에는 Hook, Status Line 또는 추가 수집 스크립트가 필요하지 않습니다.**

대시보드는 다음 디렉터리를 직접 스캔합니다.

```text
~/.codex/sessions
~/.codex/archived_sessions
```

사용 방법:

1. Codex Desktop 또는 Codex CLI를 평소처럼 사용하여 Session을 하나 이상 만듭니다.
2. 이 프로젝트를 시작합니다.
3. 왼쪽에서 Codex를 선택합니다.
4. 오른쪽 위 동기화 버튼을 클릭하거나 백그라운드 동기화를 기다립니다.

참고:

- Codex 자격 증명은 계속 Codex 자체가 관리합니다.
- 대시보드는 분석을 위해 로컬 Session 기록만 읽습니다.
- 각 Session은 transcript의 `originator`에 따라 `Desktop` 또는 `CLI` 소스 표시를 보여 줍니다. 판별할 수 없는 이전 형식은 분류되지 않은 상태로 유지됩니다.
- API 할당량 정보가 표시되는 경우 최신 로컬 Session 로그에서 가져오며 실시간 온라인 조회가 아닙니다.

* * *

## Claude Code 설정

**Claude Code에는 Hook, Status Line 또는 추가 수집 스크립트가 필요하지 않습니다.**

대시보드는 다음 디렉터리를 직접 스캔합니다.

```text
~/.claude/projects
```

사용 방법:

1. Claude Code를 평소처럼 사용하여 프로젝트 Session을 하나 이상 만듭니다.
2. 이 프로젝트를 시작합니다.
3. 왼쪽에서 Claude Code를 선택합니다.
4. 오른쪽 위 동기화 버튼을 클릭하거나 백그라운드 동기화를 기다립니다.

참고:

- Claude Code 자격 증명은 계속 Claude Code 자체가 관리합니다.
- 대시보드는 분석을 위해 로컬 프로젝트 Session 기록만 읽습니다.
- `~/.claude/projects`가 없으면 Claude Code 페이지에 데이터가 없다고 표시됩니다.

* * *

## Cursor 설정

**Cursor에는 Hook, Status Line 또는 추가 수집 스크립트가 필요하지 않습니다.** 대시보드는 다음 디렉터리를 직접 스캔합니다.

```text
~/.cursor/projects
```

Cursor는 대화와 Agent transcript를 프로젝트 디렉터리 아래에 저장합니다. 대시보드는 플랫폼 기본 `Cursor/User/globalStorage/state.vscdb`도 읽기 전용으로 스캔하고 `agentKv` 기록을 사용하여 실제 모델을 귀속합니다. 고유하게 일치하지 않는 Session은 `Unknown Model`로 유지됩니다.

사용 방법:

1. Cursor에서 대화 또는 Agent Session을 하나 이상 만듭니다.
2. 대시보드를 시작하거나 새로 고칩니다.
3. 왼쪽에서 Cursor를 선택합니다.
4. 오른쪽 위 동기화 버튼을 클릭하거나 백그라운드 동기화를 기다립니다.

Cursor의 로컬 데이터에는 정확한 Token 수나 공식 청구 내역이 없으므로 Token은 텍스트 내용에서 추정합니다. 비용은 `pricing.csv`에 일치하는 모델이 있을 때만 추정되며 공식 청구액과 같지 않습니다. 기본 위치가 아닌 경우 `CURSOR_DIR` 및 `CURSOR_STATE_DB`를 사용할 수 있습니다.

* * *

## Grok Build 설정

**Grok Build에는 Hook, Status Line 또는 추가 수집 스크립트가 필요하지 않습니다.** 대시보드는 다음 디렉터리를 직접 스캔합니다.

```text
~/.grok/sessions
```

Grok Build가 내부적으로 저장하는 Session stream을 사용합니다. 이전 형식의
`~/.Grok/build/usage/usage-YYYY-MM-DD.jsonl`은 읽지 않으며 `~/.Grok/build/settings.json`에
`statusLine`을 설정할 필요도 없습니다.

사용 방법:

1. Grok Build를 평소처럼 사용하여 Session을 하나 이상 만듭니다.
2. 이 프로젝트를 시작합니다.
3. 왼쪽에서 Grok Build를 선택합니다.
4. 오른쪽 위 동기화 버튼을 클릭하거나 백그라운드 동기화를 기다립니다.

Grok Build Session은 context token snapshot만 제공할 수도 있고 provider usage와 비용을 포함할 수도 있습니다. 대시보드는 provider usage/cost를 우선하며 context snapshot만 있는 경우 `pricing.csv`의 xAI API 가격으로 비용을 추정하고 Session 목록에 `Context`로 표시합니다. 이는 SuperGrok 또는 다른 구독 요금제의 주간 할당량을 의미하지 않습니다.

* * *

## Pi Coding Agent 설정

**Pi Coding Agent에는 Hook, Status Line 또는 추가 수집 스크립트가 필요하지 않습니다.** 대시보드는 다음 디렉터리를 직접 스캔합니다.

```text
~/.pi/agent/sessions
```

Pi Coding Agent는 트리 구조 디렉터리 아래에 Session을 로컬 JSONL 파일로 자동 저장하며, 대시보드는 이러한 Session 기록을 직접 읽습니다.

사용 방법:

1. Pi Coding Agent를 평소처럼 사용하여 Session을 하나 이상 만듭니다.
2. 대시보드를 시작하거나 새로 고칩니다.
3. 왼쪽에서 Pi Coding Agent를 선택합니다.
4. 오른쪽 위 동기화 버튼을 클릭하거나 백그라운드 동기화를 기다립니다.

Pi Coding Agent의 비용은 각 Session이 각 turn마다 보고하는 `usage.cost` 및 관련 usage 데이터에서 항상 직접 읽습니다. Pi는 turn별 권위 있는 token 및 비용 정보를 기본 제공하므로 Grok Build처럼 context snapshot 추정으로 되돌아가지 않습니다.

* * *

## OMP 설정

**OMP에는 Hook, Status Line 또는 추가 수집 스크립트가 필요하지 않습니다.** 대시보드는 다음 디렉터리를 직접 스캔합니다.

```text
~/.omp/agent/sessions
```

OMP는 Pi Coding Agent의 오픈 소스 포크(<https://github.com/can1357/oh-my-pi>)이며 동일한 JSONL 형식으로 Session을 저장합니다. 대시보드는 이러한 로컬 Session 기록을 직접 읽습니다.

사용 방법:

1. OMP를 평소처럼 사용하여 Session을 하나 이상 만듭니다.
2. 대시보드를 시작하거나 새로 고칩니다.
3. 왼쪽에서 OMP를 선택합니다.
4. 오른쪽 위 동기화 버튼을 클릭하거나 백그라운드 동기화를 기다립니다.

OMP의 비용은 각 Session이 각 turn마다 보고하는 `usage.cost` 및 관련 usage 데이터에서 항상 직접 읽습니다. OMP는 turn별 권위 있는 token 및 비용 정보를 기본 제공하므로 Grok Build처럼 context snapshot 추정으로 되돌아가지 않습니다.

* * *

## Muse Code 설정

**Muse Code에는 Hook, Status Line 또는 추가 수집 스크립트가 필요하지 않습니다.** 대시보드는 다음 디렉터리를 직접 스캔합니다.

```text
~/.local/share/muse/sessions
```

Muse Code는 날짜와 Session ID로 구분된 `session.jsonl` 파일에 Session을 저장합니다. 대시보드는 모델 완료 이벤트를 분석하여 입력, 캐시 읽기, 출력, 추론 Token을 분리하고 사용자 프롬프트, 도구 단계 및 Agent 응답 타임라인을 복원합니다.

사용 방법:

1. Muse Code를 평소처럼 사용하여 Session을 하나 이상 만듭니다.
2. 대시보드를 시작하거나 새로 고칩니다.
3. 왼쪽에서 Muse Code를 선택합니다.
4. 오른쪽 위 동기화 버튼을 클릭하거나 백그라운드 동기화를 기다립니다.

Muse Code 비용은 Session이 보고한 모델과 `pricing.csv`를 기준으로 추정합니다. 데이터가 기본 위치에 없다면 `MUSE_DIR`을 `sessions`가 포함된 Muse Code 데이터 디렉터리로 설정합니다.

* * *

## 로컬 데이터 동기화 방식

서비스가 시작되면 백엔드가 로컬 SQLite를 초기화하고 즉시 한 번 데이터를 동기화합니다. 시작 후에는 5초마다 백그라운드 동기화도 수행합니다.

SQLite 기본 위치:

```text
~/.token-usage-insights/token_usage_insights.db
```

프런트엔드 오른쪽 위 동기화 버튼은 다음을 호출합니다.

```text
GET /api/:assistant/sync
```

이 작업은 로컬 로그의 전체 증분 동기화를 수행합니다.

## 가져오기 / 내보내기(컴퓨터 간 집계)

**일반적인 사용에서는 대시보드 오른쪽 위의 내보내기 및 가져오기 버튼을 사용하세요.** 설치 버전은 브라우저만으로 컴퓨터 간 데이터를 집계할 수 있으며 최대 200 MB의 가져오기 파일을 지원합니다.

v0.9.0부터 대시보드와 CLI는 `token-usage-insights` 실행 파일로 통합되었습니다. 인수 없이 실행하면 대시보드가 시작되며, `export`, `export-all`, `import` 하위 명령으로 데이터를 처리합니다. 각 명령은 `--help`와 `-h`를 지원합니다. 이전 설치는 업데이트하거나 소스에서 빌드해야 합니다.

`--agent`는 어시스턴트(`antigravity` / `copilot` / `codex` / `claude` / `cursor` / `grok` / `pi` / `omp` / `muse`)를 지정합니다.

### 소스에서 CLI 사용

먼저 한 번 빌드합니다.

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

데이터 형식은 프런트엔드와 같으며 다음 필드를 포함합니다.

- `version`
- `assistant`
- `date`
- `exported_at`
- `records`(각 레코드에 `import_source_id`가 포함됨)

`import_source_id`는 `assistant_type`과 함께 고유 키를 구성합니다. 같은 레코드를 다시 가져오면 중복으로 판정되어 자동으로 건너뛰므로 데이터베이스에 중복 기록되지 않습니다.

* * *

## 환경 변수

환경 변수로 지정한 경로는 권위 있는 설정으로 취급되며 미리 만들 필요가 없습니다. `INSIGHTS_DIR`은 시작 시 자동으로 생성됩니다. 네이티브 절대/상대 경로와 `~`, `$HOME`, `%USERPROFILE%`, `%LOCALAPPDATA%`, `%APPDATA%`로 시작하는 일반적인 형식을 지원합니다.

| 변수 | 기본값 | 용도 |
| --- | --- | --- |
| `HOST` | `0.0.0.0` | 대시보드 서비스가 바인딩할 IPv4 또는 IPv6 주소 |
| `PORT` | `3003` | 대시보드 서비스 포트 |
| `INSIGHTS_DIR` | Windows: `%LOCALAPPDATA%\TokenUsageInsights`; 기타 플랫폼: `~/.token-usage-insights` | SQLite 데이터베이스 디렉터리 |
| `ANTIGRAVITY_DIR` | `~/.gemini/antigravity-cli` | Antigravity CLI 데이터 디렉터리 |
| `COPILOT_DIR` | `~/.copilot` | Copilot CLI 데이터 디렉터리 |
| `COPILOT_APP_DIR` | `COPILOT_DIR`과 동일 | Copilot App(데스크톱 앱) 데이터 디렉터리. `data.db` 및 `session-store.db`를 포함해야 함 |
| `VSCODE_USER_DATA_DIR` | 플랫폼별 자동 감지 | VS Code 사용자 데이터 디렉터리. `User/workspaceStorage`를 포함해야 함 |
| `VSCODE_PORTABLE_DATA_DIR` | 설정되지 않음 | VS Code Portable Mode의 `data` 디렉터리 |
| `CODEX_DIR` | `~/.codex` | Codex Desktop 및 Codex CLI가 공유하는 데이터 디렉터리 |
| `CLAUDE_DIR` | `~/.claude` | Claude Code 데이터 디렉터리 |
| `CURSOR_DIR` | `~/.cursor` | Cursor 데이터 디렉터리 |
| `CURSOR_STATE_DB` | 플랫폼별 자동 감지 | Cursor `User/globalStorage/state.vscdb` 경로. 읽기 전용으로 `agentKv` 모델 정보를 가져오는 데 사용 |
| `GROK_DIR` | `~/.grok` | Grok Build 데이터 디렉터리 |
| `PI_DIR` | `~/.pi` | Pi Coding Agent 데이터 디렉터리 |
| `OMP_DIR` | `~/.omp` | OMP 데이터 디렉터리 |
| `MUSE_DIR` | `~/.local/share/muse` | Muse Code 데이터 디렉터리. `sessions`를 포함해야 함 |
| `CORS_ALLOWED_ORIGINS` | `http://localhost:<PORT>,http://127.0.0.1:<PORT>` | 쉼표로 구분한 허용 CORS origin |

> **기본 바인딩은 `0.0.0.0`이므로 같은 로컬 네트워크의 다른 장치가 대시보드에 연결할 수 있습니다. 로컬에서만 보려면 `HOST`를 `127.0.0.1`로 설정하세요.**

예:

```bash
HOST="127.0.0.1" INSIGHTS_DIR="/tmp/token-usage-insights" PORT="3010" "$HOME/.local/bin/token-usage-insights"
```

Windows PowerShell 예:

```powershell
$env:HOST = '127.0.0.1'; $env:INSIGHTS_DIR = 'D:\Token Usage Insights\資料庫'; $env:CODEX_DIR = "$env:USERPROFILE\.codex"; $env:PORT = '3010'; & "$HOME\bin\token-usage-insights.cmd"
```

* * *

## 상주 서비스

### Linux: 한 줄로 systemd 사용자 서비스 설치 및 활성화

```bash
curl -fsSL https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.sh | bash -s -- --service
```

이 명령은 설치 버전을 다운로드하고 `token-usage-insights.service`를 즉시 활성화합니다. systemd 파일을 직접 빌드하거나 수정할 필요가 없습니다.

### 서비스 관리

```bash
systemctl --user status token-usage-insights.service
journalctl --user -u token-usage-insights.service -n 50 -f
systemctl --user restart token-usage-insights.service
systemctl --user stop token-usage-insights.service
```

* * *

## 설치 옵션 및 수동 설치

GitHub Release는 Linux, macOS 및 Windows용 컴파일된 실행 파일을 제공합니다. 설치와 실행에 Rust 또는 Cargo가 필요하지 않습니다.

### 한 줄 설치의 선택적 매개 변수

`scripts/get.sh`(Linux / macOS) 및 `scripts/get.ps1`(Windows)은 플랫폼과 CPU 아키텍처를 자동으로 판단하고 최신(또는 지정된) Release에서 해당 압축 패키지를 다운로드한 뒤 압축을 풀고 패키지의 `install.sh` / `install.ps1`을 호출합니다. 수동 다운로드나 압축 해제가 필요하지 않습니다.

Linux / macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.sh | bash
```

Linux에서 systemd 사용자 서비스를 함께 설치하고 활성화하려면:

```bash
curl -fsSL https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.sh | bash -s -- --service
```

Windows PowerShell:

```powershell
irm https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.ps1 | iex
```

설치가 끝나면 실행합니다(Linux/macOS는 `bin_dir`가 `PATH`에 포함되는지 확인하고, Windows는 `.cmd` shim을 만듭니다).

```bash
token-usage-insights
```

환경 변수로 버전과 설치 경로를 제어할 수 있습니다(모두 선택 사항).

| 변수 | 대상 플랫폼 | 설명 |
| --- | --- | --- |
| `TOKEN_USAGE_INSIGHTS_VERSION` | Linux / macOS / Windows | 설치할 Release tag(예: `v0.9.2`); 기본값은 `latest` |
| `TOKEN_USAGE_INSIGHTS_INSTALL_DIR` | Linux / macOS | `install.sh`에 전달할 설치 디렉터리 |
| `TOKEN_USAGE_INSIGHTS_BIN_DIR` | Linux / macOS | `install.sh`에 전달할 실행 파일 링크 디렉터리 |

Windows에서 설치 위치, bin 디렉터리 및 포트를 사용자 지정하려면 먼저 스크립트를 다운로드한 뒤 매개 변수와 함께 실행해야 합니다(`iex` 파이프라인은 매개 변수를 지원하지 않음).

```powershell
Invoke-WebRequest -Uri https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.ps1 -OutFile get.ps1
.\get.ps1 -InstallDir 'D:\Apps\Token Usage Insights' -Port 3010
```

### 수동 다운로드 및 설치

원격 스크립트를 직접 실행하지 않으려면 플랫폼에 맞는 압축 패키지를 수동으로 다운로드하고 패키지에 포함된 설치 스크립트를 실행할 수 있습니다. 각 Release 압축 패키지에는 다음이 포함됩니다.

- 단일 플랫폼 실행 파일
- `static/`의 프런트엔드 자산
- 모델 가격표 `pricing.csv`
- `shell/` 디렉터리의 Status Line 및 서비스 스크립트
- `scripts/` 디렉터리(`install.sh`, `install.ps1`, `get.sh`, `get.ps1` 포함)
- README, LICENSE 및 VERSION

Linux 또는 macOS:

```bash
tar -xzf token-usage-insights-<tag>-<target>.tar.gz
cd token-usage-insights-<tag>-<target>
./install.sh
```

Linux에서 systemd 사용자 서비스를 설치하고 활성화하려면:

```bash
./install.sh --service
```

Windows:

```powershell
Expand-Archive token-usage-insights-<tag>-x86_64-pc-windows-msvc.zip
cd token-usage-insights-<tag>-x86_64-pc-windows-msvc
powershell -ExecutionPolicy Bypass -File .\install.ps1
```

Windows 설치 위치 및 포트 사용자 지정:

```powershell
.\install.ps1 -InstallDir 'D:\Apps\Token Usage Insights' -BinDir "$HOME\bin" -Port 3010
```

### CI 검증

`Release` workflow는 매 빌드마다 Linux, macOS 및 Windows에서 해당 설치 스크립트(`install.sh` / `install.ps1`)를 실제로 실행하고, 설치 후 실행 파일을 시작하여 다음을 확인합니다.

- 서비스가 지정된 포트에서 `/api/<assistant>/pricing`에 응답함
- 응답 내용이 패키지에 포함된 `pricing.csv`를 실제로 읽음
- 새로운 `INSIGHTS_DIR`이 생성되고 SQLite 데이터베이스가 만들어짐

`get.sh` 및 `get.ps1`도 매 빌드 전에 구문 검사(`bash -n` 및 PowerShell AST 분석)를 수행하여 Release에 게시되는 버전이 정상적으로 실행되는지 확인합니다.

### 유지 관리자의 릴리스

Git tag를 push하면 GitHub Actions가 해당 Release를 자동으로 만듭니다.

```bash
git tag vX.Y.Z
git push origin vX.Y.Z
```

* * *

## 이전 데이터 마이그레이션

이전에 다음 독립 프로젝트를 사용했다면 이 프로젝트를 시작할 때 오래된 SQLite 데이터를 자동으로 마이그레이션하려고 시도합니다.

- `~/.gemini/antigravity-cli/antigravity_cli_token_insights.db`
- `~/.copilot/copilot_cli_token_insights.db`
- `~/.codex/codex_cli_token_insights.db`

마이그레이션이 성공하면 이전 데이터베이스의 이름에 `.bak`가 붙습니다.

데이터 마이그레이션이 완료되었는지 확인한 뒤 이전 서비스를 중지할 수 있습니다.

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

## 문제 해결

### 대시보드에 데이터가 없음

도구별로 데이터 원본이 존재하는지 확인합니다.

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

Antigravity CLI와 Copilot CLI는 `settings.json`에 `statusLine`이 설정되어 있고 스크립트에 실행 권한이 있는지도 확인해야 합니다.

Windows PowerShell에서는 네이티브 데이터 디렉터리를 직접 확인할 수 있습니다.

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

### Status Line 스크립트를 실행할 수 없음

```bash
command -v jq
chmod +x ~/.gemini/antigravity-cli/statusline-token.sh
chmod +x ~/.copilot/statusline-token.sh
```

Status Line 스크립트는 CLI가 전달한 JSON을 분석하기 위해 `jq`에 의존합니다.

위의 `jq` 요구 사항은 `.sh` collector에만 적용됩니다. Windows `.ps1` collector는 다음 명령으로 테스트할 수 있으며 백슬래시와 공백이 포함된 경로를 네이티브 방식으로 처리합니다.

```powershell
Write-Output '{}' | powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$env:USERPROFILE\.gemini\antigravity-cli\statusline-token.ps1" -Assistant antigravity
```

### 설정 파일 JSON 형식 오류

```bash
jq . ~/.gemini/antigravity-cli/settings.json
jq . ~/.copilot/settings.json
```

다른 설정이 이미 있다면 파일 전체를 배열이나 일반 문자열로 바꾸지 말고 `statusLine` 객체를 병합하세요.

### `localhost:3003`에 연결할 수 없음

```bash
PORT=3010 "$HOME/.local/bin/token-usage-insights"
```

다른 포트를 사용하는 경우 해당 URL을 엽니다. 예:

```text
http://localhost:3010
```

* * *

## 개발 명령

이 절은 프로젝트를 수정하거나 소스에서 빌드해야 하는 개발자를 위한 것입니다. 일반적인 사용에는 앞서 설명한 한 줄 설치 명령을 사용하세요.

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

## 프로젝트 파일

```text
src/                 Rust 後端、API、SQLite 同步、價格與時間軸解析
static/              前端 HTML、JavaScript、CSS 與圖片資產
shell/               Bash/PowerShell Status Line collector 與 systemd 服務範本
scripts/             Linux/macOS、Windows 安裝與 Windows smoke test
pricing.csv          模型價格表，本地估算費用依此檔案載入
```

* * *

## 스크린샷

![Token 전황실 일일 대시보드](screenshots/codex-daily-2026-07-07-desktop-chrome.png)

![Token 전황실 월간 대시보드](screenshots/codex-daily-2026-07-07.png)

![Token 전황실 Session 타임라인](screenshots/codex-daily-2026-07-07-desktop-chrome.png)
