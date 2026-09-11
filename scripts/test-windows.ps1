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

    $psExe = if (Get-Command "powershell.exe" -ErrorAction SilentlyContinue) { "powershell.exe" } else { "pwsh" }

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

        $null = $json | & $psExe -NoProfile -ExecutionPolicy Bypass -File $case.Script
        if ($LASTEXITCODE -ne 0) { throw "$($case.Name) collector failed on first invocation." }
        $jsonl = Get-ChildItem -LiteralPath (Join-Path $case.Directory "usage") -Filter "*.jsonl" -File
        Assert-Equal 1 @($jsonl).Count "$($case.Name) should create one JSONL file."
        $entries = @(Get-Content -LiteralPath $jsonl.FullName | ForEach-Object { $_ | ConvertFrom-Json })
        Assert-Equal 1 $entries.Count "$($case.Name) should append the first positive delta."
        Assert-Equal 12 $entries[0].delta_tokens.total "$($case.Name) first delta is wrong."

        $null = $json | & $psExe -NoProfile -ExecutionPolicy Bypass -File $case.Script
        if ($LASTEXITCODE -ne 0) { throw "$($case.Name) collector failed on repeat invocation." }
        $entries = @(Get-Content -LiteralPath $jsonl.FullName | ForEach-Object { $_ | ConvertFrom-Json })
        Assert-Equal 1 $entries.Count "$($case.Name) should not append a zero delta."

        $payload.context_window.total_input_tokens = 20
        $payload.context_window.total_output_tokens = 4
        $payload.context_window.total_tokens = 24
        $json = $payload | ConvertTo-Json -Depth 5 -Compress
        $null = $json | & $psExe -NoProfile -ExecutionPolicy Bypass -File $case.Script
        if ($LASTEXITCODE -ne 0) { throw "$($case.Name) collector failed on delta invocation." }
        $entries = @(Get-Content -LiteralPath $jsonl.FullName | ForEach-Object { $_ | ConvertFrom-Json })
        Assert-Equal 2 $entries.Count "$($case.Name) should append the second positive delta."
        Assert-Equal 12 $entries[1].delta_tokens.total "$($case.Name) second delta is wrong."
        Assert-Equal 2 $entries[1].turn_no "$($case.Name) turn number is wrong."
    }

    $installCmd = Get-Command (Resolve-Path (Join-Path $PSScriptRoot "install.ps1")).Path
    Assert-Equal $true $installCmd.Parameters.ContainsKey("Service") "install.ps1 should declare -Service."
    Assert-Equal $true $installCmd.Parameters.ContainsKey("HostAddress") "install.ps1 should declare -HostAddress."

    $getCmd = Get-Command (Resolve-Path (Join-Path $PSScriptRoot "get.ps1")).Path
    Assert-Equal $true $getCmd.Parameters.ContainsKey("Service") "get.ps1 should declare -Service."
    Assert-Equal $true $getCmd.Parameters.ContainsKey("HostAddress") "get.ps1 should declare -HostAddress."
    Assert-Equal ([Nullable[int]].Name) $getCmd.Parameters["Port"].ParameterType.Name "get.ps1 should allow install.ps1 to keep its own PORT default."

    $runnerCmd = Get-Command (Resolve-Path (Join-Path $PSScriptRoot "run-service.ps1")).Path
    Assert-Equal $true $runnerCmd.Parameters.ContainsKey("InstallDir") "run-service.ps1 should declare -InstallDir."
    Assert-Equal $true $runnerCmd.Parameters.ContainsKey("HostAddress") "run-service.ps1 should declare -HostAddress."

    $getScriptContent = Get-Content -Raw -LiteralPath (Join-Path $PSScriptRoot "get.ps1")
    if (-not $getScriptContent.Contains('if ($null -ne $Port) { $InstallArgs["Port"] = $Port }')) {
        throw "get.ps1 should only forward -Port when explicitly provided."
    }

    $installScriptContent = Get-Content -Raw -LiteralPath (Join-Path $PSScriptRoot "install.ps1")
    $stopCallIndex = $installScriptContent.IndexOf('Stop-ExistingServiceInstance -TaskName $TaskName -ProcessName $AppName -InstallDir $InstallDir')
    $copyBinaryIndex = $installScriptContent.IndexOf('Copy-Item -Force $BinarySrc (Join-Path $InstallDir "$AppName.exe")')
    if ($stopCallIndex -lt 0 -or $copyBinaryIndex -lt 0 -or $stopCallIndex -ge $copyBinaryIndex) {
        throw "install.ps1 should stop an existing service instance before copying the executable."
    }

    if (-not $installScriptContent.Contains('[System.Net.IPAddress]::IPv6Any')) {
        throw "install.ps1 should detect unspecified IPv6 dashboard hosts."
    }
    if (-not $installScriptContent.Contains('return "[$HostAddress]"')) {
        throw "install.ps1 should bracket IPv6 dashboard hosts when printing the URL."
    }
    if (-not $installScriptContent.Contains('-AtLogOn -User $env:USERNAME')) {
        throw "install.ps1 should scope the logon trigger to the current user."
    }
    $tryIndex = $installScriptContent.IndexOf('try {')
    $actionIndex = $installScriptContent.IndexOf('$Action = New-ScheduledTaskAction')
    if ($tryIndex -lt 0 -or $actionIndex -lt 0 -or $tryIndex -ge $actionIndex) {
        throw "install.ps1 should define scheduled task action inside the guarded try block."
    }

    Write-Host "Windows collector smoke tests passed."
} finally {
    $env:ANTIGRAVITY_DIR = $PreviousAntigravityDir
    $env:COPILOT_DIR = $PreviousCopilotDir
    if (Test-Path -LiteralPath $Root) {
        Remove-Item -LiteralPath $Root -Recurse -Force
    }
}
