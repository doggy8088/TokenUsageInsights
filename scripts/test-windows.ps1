[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
$Root = Join-Path ([IO.Path]::GetTempPath()) ("Token Usage Insights Test-{0}" -f [guid]::NewGuid())
$PreviousAntigravityDir = $env:ANTIGRAVITY_DIR
$PreviousCopilotDir = $env:COPILOT_DIR

function Assert-Equal {
    param($Expected, $Actual, [string]$Message)
    if ($Expected -ne $Actual) {
        throw "$Message Expected=$Expected Actual=$Actual"
    }
}

try {
    $cases = @(
        @{
            Name = "antigravity"
            EnvironmentName = "ANTIGRAVITY_DIR"
            Directory = Join-Path $Root "AntigravityData"
            Script = Join-Path $PSScriptRoot "..\shell\antigravity\statusline-token.ps1"
            SessionProperty = "conversation_id"
        },
        @{
            Name = "copilot"
            EnvironmentName = "COPILOT_DIR"
            Directory = Join-Path $Root "CopilotData"
            Script = Join-Path $PSScriptRoot "..\shell\copilot\statusline-token.ps1"
            SessionProperty = "session_id"
        }
    )

    foreach ($case in $cases) {
        [Environment]::SetEnvironmentVariable($case.EnvironmentName, $case.Directory)
        $payload = [ordered]@{
            model = [ordered]@{ id = "test-model" }
            context_window = [ordered]@{
                total_input_tokens = 10
                total_output_tokens = 2
                total_tokens = 12
            }
        }
        $payload[$case.SessionProperty] = "windows-path-test"
        $json = $payload | ConvertTo-Json -Depth 5 -Compress

        $null = $json | powershell.exe -NoProfile -ExecutionPolicy Bypass -File $case.Script
        if ($LASTEXITCODE -ne 0) { throw "$($case.Name) collector failed on first invocation." }
        $jsonl = Get-ChildItem -LiteralPath (Join-Path $case.Directory "usage") -Filter "*.jsonl" -File
        Assert-Equal 1 @($jsonl).Count "$($case.Name) should create one JSONL file."
        $entries = @(Get-Content -LiteralPath $jsonl.FullName | ForEach-Object { $_ | ConvertFrom-Json })
        Assert-Equal 1 $entries.Count "$($case.Name) should append the first positive delta."
        Assert-Equal 12 $entries[0].delta_tokens.total "$($case.Name) first delta is wrong."

        $null = $json | powershell.exe -NoProfile -ExecutionPolicy Bypass -File $case.Script
        if ($LASTEXITCODE -ne 0) { throw "$($case.Name) collector failed on repeat invocation." }
        $entries = @(Get-Content -LiteralPath $jsonl.FullName | ForEach-Object { $_ | ConvertFrom-Json })
        Assert-Equal 1 $entries.Count "$($case.Name) should not append a zero delta."

        $payload.context_window.total_input_tokens = 20
        $payload.context_window.total_output_tokens = 4
        $payload.context_window.total_tokens = 24
        $json = $payload | ConvertTo-Json -Depth 5 -Compress
        $null = $json | powershell.exe -NoProfile -ExecutionPolicy Bypass -File $case.Script
        if ($LASTEXITCODE -ne 0) { throw "$($case.Name) collector failed on delta invocation." }
        $entries = @(Get-Content -LiteralPath $jsonl.FullName | ForEach-Object { $_ | ConvertFrom-Json })
        Assert-Equal 2 $entries.Count "$($case.Name) should append the second positive delta."
        Assert-Equal 12 $entries[1].delta_tokens.total "$($case.Name) second delta is wrong."
        Assert-Equal 2 $entries[1].turn_no "$($case.Name) turn number is wrong."
    }

    Write-Host "Windows collector smoke tests passed."
} finally {
    $env:ANTIGRAVITY_DIR = $PreviousAntigravityDir
    $env:COPILOT_DIR = $PreviousCopilotDir
    if (Test-Path -LiteralPath $Root) {
        Remove-Item -LiteralPath $Root -Recurse -Force
    }
}
