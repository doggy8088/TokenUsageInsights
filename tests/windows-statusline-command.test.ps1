# 驗證看板為 Windows 產生的 statusLine.command 能被 Antigravity CLI / Copilot CLI 正確執行（GitHub issue #64）。
#
# 這兩個 CLI 不會透過 shell 解析 command，而是以空白切割後直接傳給子程序；
# 因此這裡以「空白切割後直接呼叫」與「交給 cmd.exe 執行」兩種方式實際執行後端回傳的命令，
# 並確認資料目錄含空白或 `&` 等 cmd.exe 中繼字元時也能正常寫入 usage JSONL。
[CmdletBinding()]
param(
    [string]$Executable = (Join-Path $PSScriptRoot "..\target\release\token-usage-insights.exe"),
    [int]$Port = 3977
)

$ErrorActionPreference = "Stop"

function Assert-True {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw $Message }
}

function Assert-Equal {
    param($Expected, $Actual, [string]$Message)
    if ($Expected -ne $Actual) { throw "$Message Expected=[$Expected] Actual=[$Actual]" }
}

# 模擬 CLI 的行為：以空白切割 command，第一段為執行檔，其餘逐一作為獨立引數傳入。
function Invoke-SplitCommand {
    param([string]$Command, [string]$StdinJson)

    $tokens = @($Command -split ' ' | Where-Object { $_ -ne "" })
    $exe = $tokens[0]
    $arguments = @()
    if ($tokens.Length -gt 1) { $arguments = $tokens[1..($tokens.Length - 1)] }

    $previous = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $output = $StdinJson | & $exe @arguments 2>&1
        return @{ ExitCode = $LASTEXITCODE; Output = ($output | Out-String) }
    } finally {
        $ErrorActionPreference = $previous
    }
}

# 模擬透過 shell 執行 command 的宿主（例如 Node.js spawn 搭配 shell: true 會走 cmd.exe）。
function Invoke-CmdShellCommand {
    param([string]$Command, [string]$StdinJson)

    $previous = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $output = $StdinJson | & cmd.exe /d /s /c "$Command" 2>&1
        return @{ ExitCode = $LASTEXITCODE; Output = ($output | Out-String) }
    } finally {
        $ErrorActionPreference = $previous
    }
}

function Get-UsageEntries {
    param([string]$DataDir)

    $usageDir = Join-Path $DataDir "usage"
    if (-not (Test-Path -LiteralPath $usageDir)) { return @() }
    $files = @(Get-ChildItem -LiteralPath $usageDir -Filter "*.jsonl" -File)
    if ($files.Count -eq 0) { return @() }
    Assert-Equal 1 $files.Count "Expected exactly one usage JSONL file in $usageDir."
    return @(Get-Content -LiteralPath $files[0].FullName | Where-Object { $_ -ne "" } | ForEach-Object { $_ | ConvertFrom-Json })
}

function New-Payload {
    param([string]$SessionProperty, [int]$Total)

    $payload = [ordered]@{
        model = [ordered]@{ id = "statusline-command-test" }
        context_window = [ordered]@{
            total_input_tokens = $Total - 2
            total_output_tokens = 2
            total_tokens = $Total
        }
    }
    $payload[$SessionProperty] = "windows-statusline-command-test"
    return ($payload | ConvertTo-Json -Depth 5 -Compress)
}

$Executable = (Resolve-Path -LiteralPath $Executable).Path
$sourceScript = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..\shell\statusline-token.ps1")).Path

# 根目錄刻意包含空白，antigravity 資料目錄另含空白與 cmd.exe 的 `&` 中繼字元；copilot 資料目錄只含安全字元，兩種路徑型態都要通過。
$root = Join-Path ([IO.Path]::GetTempPath()) ("Token Usage Insights Statusline-{0}" -f [guid]::NewGuid())
$antigravityDir = Join-Path $root "antigravity & data"
$copilotDir = Join-Path $root "copilot"
$insightsDir = Join-Path $root "insights"
$workspaceDir = Join-Path $root "workspace"
foreach ($dir in @($antigravityDir, $copilotDir, $insightsDir, $workspaceDir)) {
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
}
Copy-Item -LiteralPath $sourceScript -Destination (Join-Path $antigravityDir "statusline-token.ps1") -Force
Copy-Item -LiteralPath $sourceScript -Destination (Join-Path $copilotDir "statusline-token.ps1") -Force

$previousEnv = @{}
foreach ($name in @("ANTIGRAVITY_DIR", "COPILOT_DIR", "INSIGHTS_DIR", "PORT", "HOST", "TOKEN_USAGE_INSIGHTS_SERVICE")) {
    $previousEnv[$name] = [Environment]::GetEnvironmentVariable($name)
}

$server = $null
try {
    [Environment]::SetEnvironmentVariable("ANTIGRAVITY_DIR", $antigravityDir)
    [Environment]::SetEnvironmentVariable("COPILOT_DIR", $copilotDir)
    [Environment]::SetEnvironmentVariable("INSIGHTS_DIR", $insightsDir)
    [Environment]::SetEnvironmentVariable("PORT", "$Port")
    [Environment]::SetEnvironmentVariable("HOST", "127.0.0.1")
    [Environment]::SetEnvironmentVariable("TOKEN_USAGE_INSIGHTS_SERVICE", "1")

    $serverStdout = Join-Path $root "server-stdout.log"
    $serverStderr = Join-Path $root "server-stderr.log"
    $server = Start-Process -FilePath $Executable -WorkingDirectory $workspaceDir -PassThru -NoNewWindow `
        -RedirectStandardOutput $serverStdout -RedirectStandardError $serverStderr

    $baseUrl = "http://127.0.0.1:$Port"
    $deadline = (Get-Date).AddSeconds(90)
    $ready = $false
    while ((Get-Date) -lt $deadline) {
        if ($server.HasExited) { break }
        try {
            $null = Invoke-RestMethod -Uri "$baseUrl/api/antigravity/setup-info" -TimeoutSec 5
            $ready = $true
            break
        } catch {
            Start-Sleep -Milliseconds 500
        }
    }
    if (-not $ready) {
        if (Test-Path -LiteralPath $serverStdout) { Get-Content -LiteralPath $serverStdout | ForEach-Object { Write-Host $_ } }
        if (Test-Path -LiteralPath $serverStderr) { Get-Content -LiteralPath $serverStderr | ForEach-Object { Write-Host $_ } }
        throw "Dashboard server did not become ready on $baseUrl."
    }

    $cases = @(
        @{
            Name = "antigravity"
            DataDir = $antigravityDir
            SessionProperty = "conversation_id"
            ExpectedForm = "-EncodedCommand "
        },
        @{
            Name = "copilot"
            DataDir = $copilotDir
            SessionProperty = "session_id"
            ExpectedForm = "-File "
        }
    )

    foreach ($case in $cases) {
        $name = $case.Name
        $setup = Invoke-RestMethod -Uri "$baseUrl/api/$name/setup-info" -TimeoutSec 10
        Assert-Equal "windows" $setup.platform "setup-info platform should be windows."

        $status = $setup.$name
        $command = [string]$status.statusline_command
        Write-Host "[$name] statusline_command: $command"

        Assert-True ($command.StartsWith("powershell.exe -NoProfile -ExecutionPolicy Bypass ")) "[$name] command should invoke powershell.exe with bypass policy."
        Assert-True (-not $command.Contains('"')) "[$name] command must not contain double quotes (issue #64)."
        Assert-True (-not $command.Contains('\')) "[$name] command must use forward slashes only (issue #64)."
        Assert-True ($command.Contains($case.ExpectedForm)) "[$name] command should use the '$($case.ExpectedForm)' form for this path."
        $expectedScript = (Join-Path $case.DataDir "statusline-token.ps1").Replace('\', '/')
        if ($case.ExpectedForm -eq "-EncodedCommand ") {
            # 路徑含空白 / 中繼字元時整個命令必須是純 ASCII 且不含空白以外的分隔風險；解碼後需指向正確腳本。
            $encoded = $command.Substring($command.IndexOf("-EncodedCommand ") + "-EncodedCommand ".Length)
            Assert-True ($encoded -notmatch '[^A-Za-z0-9+/=]') "[$name] encoded payload must be plain Base64."
            $decoded = [Text.Encoding]::Unicode.GetString([Convert]::FromBase64String($encoded))
            Write-Host "[$name] decoded command: $decoded"
            Assert-Equal ("& '{0}' -Assistant {1}" -f $expectedScript, $name) $decoded "[$name] decoded command should call the script with -Assistant."
        } else {
            Assert-True ($command.EndsWith(" -File $expectedScript -Assistant $name")) "[$name] command should pass $expectedScript to -File and end with -Assistant $name."
        }

        # 1. 模擬 CLI 以空白切割後直接執行
        $result = Invoke-SplitCommand -Command $command -StdinJson (New-Payload $case.SessionProperty 12)
        Write-Host "[$name] split-argv output: $($result.Output.Trim())"
        Assert-Equal 0 $result.ExitCode "[$name] command failed when executed with whitespace-split argv."
        Assert-True ($result.Output.Trim().Length -gt 0) "[$name] command should print a status line."
        $entries = @(Get-UsageEntries -DataDir $case.DataDir)
        Assert-Equal 1 $entries.Count "[$name] first invocation should append one usage entry."
        Assert-Equal 12 $entries[0].delta_tokens.total "[$name] first delta is wrong."

        # 2. 模擬透過 cmd.exe 執行
        $result = Invoke-CmdShellCommand -Command $command -StdinJson (New-Payload $case.SessionProperty 30)
        Write-Host "[$name] cmd.exe output: $($result.Output.Trim())"
        Assert-Equal 0 $result.ExitCode "[$name] command failed when executed through cmd.exe."
        $entries = @(Get-UsageEntries -DataDir $case.DataDir)
        Assert-Equal 2 $entries.Count "[$name] second invocation should append a second usage entry."
        Assert-Equal 18 $entries[1].delta_tokens.total "[$name] second delta is wrong."

        # 3. 回歸防護：issue #64 回報的舊格式（雙引號包住路徑）在空白切割下必須失敗，證明此測試能抓到該問題
        $legacyCommand = 'powershell.exe -NoProfile -ExecutionPolicy Bypass -File "{0}" -Assistant {1}' -f $status.script_path, $name
        $legacy = Invoke-SplitCommand -Command $legacyCommand -StdinJson (New-Payload $case.SessionProperty 50)
        Write-Host "[$name] legacy quoted form exit code: $($legacy.ExitCode)"
        Assert-True ($legacy.ExitCode -ne 0) "[$name] legacy quoted command unexpectedly succeeded; the regression harness no longer reproduces issue #64."
        $entries = @(Get-UsageEntries -DataDir $case.DataDir)
        Assert-Equal 2 $entries.Count "[$name] legacy quoted command should not have written usage."
    }

    Write-Host "Windows statusLine command validation passed."
} finally {
    if ($server -and -not $server.HasExited) {
        Stop-Process -Id $server.Id -Force -ErrorAction SilentlyContinue
        $server.WaitForExit(10000) | Out-Null
    }
    foreach ($name in $previousEnv.Keys) {
        [Environment]::SetEnvironmentVariable($name, $previousEnv[$name])
    }
    if (Test-Path -LiteralPath $root) {
        Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
    }
}
