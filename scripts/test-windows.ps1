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

function Assert-True {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) {
        throw $Message
    }
}

function Invoke-GetScriptInstallCapture {
    param([Nullable[int]]$Port = $null)

    $tempRoot = Join-Path $Root ([guid]::NewGuid())
    $packageRoot = Join-Path $tempRoot "package"
    $releaseDir = Join-Path $packageRoot "token-usage-insights-v-test-x86_64-pc-windows-msvc"
    $capturePath = Join-Path $tempRoot "install-args.json"
    New-Item -ItemType Directory -Force -Path (Join-Path $releaseDir "static") | Out-Null

    Set-Content -LiteralPath (Join-Path $releaseDir "pricing.csv") -Value "model,input,output"
    $fakeInstallScript = @'
param(
    [string]$InstallDir,
    [string]$BinDir,
    [string]$HostAddress,
    [int]$Port,
    [switch]$Service
)
$payload = [ordered]@{
    Keys = @($PSBoundParameters.Keys | Sort-Object)
    Port = if ($PSBoundParameters.ContainsKey('Port')) { $Port } else { $null }
    HostAddress = if ($PSBoundParameters.ContainsKey('HostAddress')) { $HostAddress } else { $null }
    Service = $PSBoundParameters.ContainsKey('Service')
}
$payload | ConvertTo-Json -Compress | Set-Content -LiteralPath '__CAPTURE_PATH__'
'@.Replace('__CAPTURE_PATH__', $capturePath.Replace("'", "''"))
    Set-Content -LiteralPath (Join-Path $releaseDir "install.ps1") -Value $fakeInstallScript

    function Invoke-RestMethod { @{ tag_name = "v-test" } }
    function Invoke-WebRequest {
        param([string]$Uri, [string]$OutFile)
        Set-Content -LiteralPath $OutFile -Value "placeholder"
    }
    function Expand-Archive {
        param([string]$Path, [string]$DestinationPath, [switch]$Force)
        Microsoft.PowerShell.Management\Copy-Item -Recurse -Force $releaseDir (Join-Path $DestinationPath (Split-Path $releaseDir -Leaf))
    }

    try {
        $arguments = @{
            Version = "latest"
            InstallDir = (Join-Path $tempRoot "install")
            BinDir = (Join-Path $tempRoot "bin")
            HostAddress = "127.0.0.1"
            Service = $true
        }
        if ($null -ne $Port) {
            $arguments["Port"] = $Port
        }

        & (Join-Path $PSScriptRoot "get.ps1") @arguments
        Get-Content -Raw -LiteralPath $capturePath | ConvertFrom-Json
    } finally {
        Remove-Item Function:\Invoke-RestMethod -ErrorAction SilentlyContinue
        Remove-Item Function:\Invoke-WebRequest -ErrorAction SilentlyContinue
        Remove-Item Function:\Expand-Archive -ErrorAction SilentlyContinue
        if (Test-Path -LiteralPath $tempRoot) {
            Remove-Item -LiteralPath $tempRoot -Recurse -Force
        }
    }
}

function Invoke-InstallServiceTest {
    param(
        [string]$HostAddress,
        [int]$Port,
        [bool]$ServiceInstall = $true,
        [switch]$FailScheduledTaskAction,
        [switch]$FailStartScheduledTask,
        [switch]$TaskTargetsLegacyInstall,
        [switch]$RemoveStartupShortcutBeforeInstall,
        [switch]$WhatIf
    )

    $tempRoot = Join-Path $Root ([guid]::NewGuid())
    $releaseDir = Join-Path $tempRoot "release"
    $scriptDir = Join-Path $releaseDir "scripts"
    $installDir = Join-Path $tempRoot "installed"
    $binDir = Join-Path $tempRoot "bin"
    $previousAppData = $env:APPDATA
    $previousUsername = $env:USERNAME
    $previousUserDomain = $env:USERDOMAIN
    $previousStartupOverride = $env:TOKEN_USAGE_INSIGHTS_STARTUP_DIR
    $env:APPDATA = Join-Path $tempRoot "AppData\Roaming"
    $env:USERNAME = "test-user"
    $env:USERDOMAIN = "test-domain"
    $startupShortcut = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Startup\token-usage-insights.lnk"
    $env:TOKEN_USAGE_INSIGHTS_STARTUP_DIR = Split-Path -Parent $startupShortcut

    New-Item -ItemType Directory -Force -Path (Join-Path $releaseDir "static") | Out-Null
    New-Item -ItemType Directory -Force -Path (Join-Path $releaseDir "shell") | Out-Null
    New-Item -ItemType Directory -Force -Path $scriptDir | Out-Null
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $startupShortcut) | Out-Null

    Set-Content -LiteralPath (Join-Path $releaseDir "token-usage-insights.exe") -Value "binary"
    Set-Content -LiteralPath (Join-Path $releaseDir "pricing.csv") -Value "model,input,output"
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot "install.ps1") -Destination (Join-Path $scriptDir "install.ps1")
    Set-Content -LiteralPath (Join-Path $scriptDir "run-service.ps1") -Value "Write-Host 'runner'"
    Set-Content -LiteralPath $startupShortcut -Value "shortcut"

    $global:serviceEvents = New-Object System.Collections.Generic.List[string]
    $global:hostMessages = New-Object System.Collections.Generic.List[string]
    $global:mockShortcuts = @{}
    $global:runnerProcessAlive = $true
    $global:otherRunnerProcessAlive = $true
    $global:appProcessAlive = $true
    $global:otherAppProcessAlive = $true
    $global:scheduledTaskTriggerUser = $null
    $runnerCommandLine = "powershell.exe -File `"$installDir\scripts\run-service.ps1`" -InstallDir `"$installDir`""
    $otherInstallDir = "$installDir-old"
    $otherRunnerCommandLine = "powershell.exe -File `"$otherInstallDir\scripts\run-service.ps1`" -InstallDir `"$otherInstallDir`""
    $legacyTaskActionArguments = "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -f:`"$otherInstallDir\scripts\run-service.ps1`" -InstallDir `"$otherInstallDir`" -HostAddress `"127.0.0.1`" -Port 3003"
    $appExecutablePath = "$installDir\token-usage-insights.exe"
    $otherAppExecutablePath = "$otherInstallDir\token-usage-insights.exe"

    function Stop-ScheduledTask {
        [CmdletBinding()]
        param([string]$TaskName)
        $global:serviceEvents.Add("StopScheduledTask")
    }
    function Get-CimInstance {
        [CmdletBinding()]
        param([string]$ClassName)
        $processes = @()
        if ($global:runnerProcessAlive) {
            $processes += [pscustomobject]@{
                Name = "powershell.exe"
                ProcessId = 111
                CommandLine = $runnerCommandLine
            }
        }
        if ($global:otherRunnerProcessAlive) {
            $processes += [pscustomobject]@{
                Name = "powershell.exe"
                ProcessId = 112
                CommandLine = $otherRunnerCommandLine
            }
        }
        if ($global:appProcessAlive) {
            $processes += [pscustomobject]@{
                Name = "token-usage-insights.exe"
                ProcessId = 221
                CommandLine = "`"$appExecutablePath`""
                ExecutablePath = $appExecutablePath
            }
        }
        if ($global:otherAppProcessAlive) {
            $processes += [pscustomobject]@{
                Name = "token-usage-insights.exe"
                ProcessId = 222
                CommandLine = "`"$otherAppExecutablePath`""
                ExecutablePath = $otherAppExecutablePath
            }
        }
        return $processes
    }
    function Get-Process {
        [CmdletBinding()]
        param([string]$Name, [int]$Id)
        if ($PSBoundParameters.ContainsKey("Id")) {
            if (($Id -eq 111) -and $global:runnerProcessAlive) {
                return [pscustomobject]@{ Id = 111; Name = "powershell" }
            }
            if (($Id -eq 112) -and $global:otherRunnerProcessAlive) {
                return [pscustomobject]@{ Id = 112; Name = "powershell" }
            }
            if (($Id -eq 221) -and $global:appProcessAlive) {
                return [pscustomobject]@{ Id = 221; Name = "token-usage-insights" }
            }
            if (($Id -eq 222) -and $global:otherAppProcessAlive) {
                return [pscustomobject]@{ Id = 222; Name = "token-usage-insights" }
            }

            return
        }
    }
    function Stop-Process {
        [CmdletBinding()]
        param(
            [Parameter(ValueFromPipeline = $true)]$InputObject,
            [int]$Id,
            [switch]$Force
        )
        process {
            if ($PSBoundParameters.ContainsKey("Id")) {
                if ($Id -eq 111) {
                    $global:runnerProcessAlive = $false
                    $global:serviceEvents.Add("StopRunner")
                }
                if ($Id -eq 112) {
                    $global:otherRunnerProcessAlive = $false
                    $global:serviceEvents.Add("StopOtherRunner")
                }
                if ($Id -eq 221) {
                    $global:appProcessAlive = $false
                    $global:serviceEvents.Add("StopApp")
                }
                if ($Id -eq 222) {
                    $global:otherAppProcessAlive = $false
                    $global:serviceEvents.Add("StopOtherApp")
                }

                return
            }
        }
    }
    function Copy-Item {
        [CmdletBinding(DefaultParameterSetName = "Path")]
        param(
            [Parameter(Mandatory = $true, Position = 0, ParameterSetName = "Path")]
            [string]$Path,
            [Parameter(Mandatory = $true, Position = 1, ParameterSetName = "Path")]
            [string]$Destination,
            [Parameter(Mandatory = $true, ParameterSetName = "LiteralPath")]
            [string]$LiteralPath,
            [Parameter(Mandatory = $true, ParameterSetName = "LiteralPathDestination")]
            [string]$LiteralDestination,
            [switch]$Force,
            [switch]$Recurse
        )

        $global:serviceEvents.Add("CopyItem")
        Microsoft.PowerShell.Management\Copy-Item @PSBoundParameters
    }
    function New-ScheduledTaskAction {
        [CmdletBinding()]
        param([Parameter(ValueFromRemainingArguments = $true)]$RemainingArgs)
        if ($FailScheduledTaskAction) {
            throw "Simulated scheduled task action failure."
        }
        @{ Action = "ok" }
    }
    function Get-ScheduledTask {
        [CmdletBinding()]
        param([string]$TaskName)
        if ($TaskTargetsLegacyInstall) {
            return [pscustomobject]@{
                Actions = @([pscustomobject]@{ Arguments = $legacyTaskActionArguments })
            }
        }

        return $null
    }
    function New-ScheduledTaskTrigger {
        [CmdletBinding()]
        param(
            [string]$User,
            [Parameter(ValueFromRemainingArguments = $true)]$RemainingArgs
        )
        $global:scheduledTaskTriggerUser = $User
        @{ Trigger = "ok" }
    }
    function New-ScheduledTaskSettingsSet {
        [CmdletBinding()]
        param([Parameter(ValueFromRemainingArguments = $true)]$RemainingArgs)
        @{ Settings = "ok" }
    }
    function Register-ScheduledTask {
        [CmdletBinding()]
        param(
            [string]$TaskName,
            [Parameter(ValueFromRemainingArguments = $true)]$RemainingArgs
        )
        $global:serviceEvents.Add("RegisterScheduledTask")
    }
    function Start-ScheduledTask {
        [CmdletBinding()]
        param([string]$TaskName)
        if ($FailStartScheduledTask) {
            throw "Simulated scheduled task start failure."
        }
        $global:serviceEvents.Add("StartScheduledTask")
    }
    function Unregister-ScheduledTask {
        [CmdletBinding()]
        param([string]$TaskName, [switch]$Confirm)
        $global:serviceEvents.Add("UnregisterScheduledTask")
    }
    function New-Object {
        [CmdletBinding()]
        param([string]$ComObject)

        if ($ComObject -eq "WScript.Shell") {
            $shell = [pscustomobject]@{}
            $shell | Add-Member -MemberType ScriptMethod -Name CreateShortcut -Value {
                param([string]$ShortcutPath)
                $global:serviceEvents.Add("CreateShortcut")
                $existingShortcutData = $global:mockShortcuts[$ShortcutPath]
                $shortcut = [pscustomobject]@{
                    ShortcutPath = $ShortcutPath
                    TargetPath = if ($existingShortcutData) { $existingShortcutData.TargetPath } else { $null }
                    Arguments = if ($existingShortcutData) { $existingShortcutData.Arguments } else { $null }
                    WorkingDirectory = if ($existingShortcutData) { $existingShortcutData.WorkingDirectory } else { $null }
                    WindowStyle = if ($existingShortcutData) { $existingShortcutData.WindowStyle } else { $null }
                    Description = if ($existingShortcutData) { $existingShortcutData.Description } else { $null }
                }
                $shortcut | Add-Member -MemberType ScriptMethod -Name Save -Value {
                    Set-Content -LiteralPath $this.ShortcutPath -Value "shortcut" -Force
                    $global:mockShortcuts[$this.ShortcutPath] = [pscustomobject]@{
                        TargetPath = $this.TargetPath
                        Arguments = $this.Arguments
                        WorkingDirectory = $this.WorkingDirectory
                        WindowStyle = $this.WindowStyle
                        Description = $this.Description
                    }
                    $global:serviceEvents.Add("SaveShortcut")
                }
                return $shortcut
            }
            return $shell
        }

        throw "Unexpected New-Object mock request: $ComObject"
    }
    function Start-Process {
        [CmdletBinding()]
        param(
            [string]$FilePath,
            [string]$ArgumentList,
            [string]$WorkingDirectory,
            [string]$WindowStyle
        )
        if ($FilePath -eq "powershell.exe") {
            $global:serviceEvents.Add("StartFallbackProcess")
        } elseif ($FilePath -eq $startupShortcut) {
            $global:serviceEvents.Add("StartStartupShortcut")
        }
    }
    function Write-Host {
        param([Parameter(ValueFromRemainingArguments = $true)]$Arguments)
        $global:hostMessages.Add(($Arguments -join " "))
    }

    try {
        if ($RemoveStartupShortcutBeforeInstall) {
            Remove-Item -LiteralPath $startupShortcut -Force -ErrorAction SilentlyContinue
        }

        $arguments = @{
            InstallDir = $installDir
            BinDir = $binDir
            HostAddress = $HostAddress
            Port = $Port
        }
        if ($ServiceInstall) {
            $arguments["Service"] = $true
        }
        if ($WhatIf) {
            $arguments["WhatIf"] = $true
        }

        & (Join-Path $scriptDir "install.ps1") @arguments

        [pscustomobject]@{
            Events = @($global:serviceEvents)
            Output = @($global:hostMessages)
            TriggerUser = $global:scheduledTaskTriggerUser
            OtherRunnerStopped = (-not $global:otherRunnerProcessAlive)
            OtherAppStopped = (-not $global:otherAppProcessAlive)
            StartupShortcutExists = (Test-Path -LiteralPath $startupShortcut)
        }
    } finally {
        foreach ($functionName in @(
            "Stop-ScheduledTask",
            "Get-CimInstance",
            "Get-Process",
            "Stop-Process",
            "Copy-Item",
            "New-ScheduledTaskAction",
            "Get-ScheduledTask",
            "New-ScheduledTaskTrigger",
            "New-ScheduledTaskSettingsSet",
            "Register-ScheduledTask",
            "Start-ScheduledTask",
            "Unregister-ScheduledTask",
            "New-Object",
            "Start-Process",
            "Write-Host"
        )) {
            Remove-Item "Function:\$functionName" -ErrorAction SilentlyContinue
        }
        Remove-Variable serviceEvents, hostMessages, mockShortcuts, runnerProcessAlive, otherRunnerProcessAlive, appProcessAlive, otherAppProcessAlive, scheduledTaskTriggerUser -Scope Global -ErrorAction SilentlyContinue
        $env:APPDATA = $previousAppData
        $env:USERNAME = $previousUsername
        $env:USERDOMAIN = $previousUserDomain
        $env:TOKEN_USAGE_INSIGHTS_STARTUP_DIR = $previousStartupOverride

        if (Test-Path -LiteralPath $tempRoot) {
            Remove-Item -LiteralPath $tempRoot -Recurse -Force
        }
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

    $capturedInstallArgs = Invoke-GetScriptInstallCapture
    Assert-Equal $false $capturedInstallArgs.Keys.Contains("Port") "get.ps1 should not pass -Port when the caller omits it."
    Assert-Equal "127.0.0.1" $capturedInstallArgs.HostAddress "get.ps1 should forward -HostAddress."
    Assert-Equal $true $capturedInstallArgs.Service "get.ps1 should forward -Service."

    $capturedInstallArgsWithPort = Invoke-GetScriptInstallCapture -Port 3010
    Assert-Equal 3010 $capturedInstallArgsWithPort.Port "get.ps1 should forward an explicit -Port value."

    $runnerCmd = Get-Command (Resolve-Path (Join-Path $PSScriptRoot "run-service.ps1")).Path
    Assert-Equal $true $runnerCmd.Parameters.ContainsKey("InstallDir") "run-service.ps1 should declare -InstallDir."
    Assert-Equal $true $runnerCmd.Parameters.ContainsKey("HostAddress") "run-service.ps1 should declare -HostAddress."

    $installIpv6Result = Invoke-InstallServiceTest -HostAddress "::1" -Port 4010
    $copyIndex = $installIpv6Result.Events.IndexOf("CopyItem")
    $stopRunnerIndex = $installIpv6Result.Events.IndexOf("StopRunner")
    $stopAppIndex = $installIpv6Result.Events.IndexOf("StopApp")
    Assert-True ($copyIndex -gt $stopRunnerIndex -and $copyIndex -gt $stopAppIndex) "install.ps1 should stop existing service processes before copying files."
    Assert-Equal $false $installIpv6Result.OtherRunnerStopped "install.ps1 should not stop a different runner whose install path merely shares a prefix."
    Assert-Equal $false $installIpv6Result.OtherAppStopped "install.ps1 should not stop a different installed executable whose path merely shares a prefix."
    Assert-True ($installIpv6Result.Output -contains "  http://[::1]:4010") "install.ps1 should bracket IPv6 dashboard URLs."
    $expectedTriggerUser = try {
        $identity = [System.Security.Principal.WindowsIdentity]::GetCurrent()
        if ($identity -and $identity.Name) {
            $identity.Name
        } else {
            "test-domain\test-user"
        }
    } catch {
        "test-domain\test-user"
    }
    Assert-Equal $expectedTriggerUser $installIpv6Result.TriggerUser "install.ps1 should scope the logon trigger to the current user."

    $installLegacyTaskResult = Invoke-InstallServiceTest -HostAddress "127.0.0.1" -Port 3003 -TaskTargetsLegacyInstall
    Assert-Equal $true $installLegacyTaskResult.OtherRunnerStopped "install.ps1 should stop the runner tied to an existing scheduled task from a previous install directory."
    Assert-Equal $true $installLegacyTaskResult.OtherAppStopped "install.ps1 should stop the executable tied to an existing scheduled task from a previous install directory."

    $installNonServiceRestartResult = Invoke-InstallServiceTest -HostAddress "127.0.0.1" -Port 3003 -ServiceInstall:$false
    Assert-True ($installNonServiceRestartResult.Events -contains "StartStartupShortcut") "install.ps1 should relaunch the existing Startup shortcut when rerun without -Service."

    $installTaskToStartupMigrationResult = Invoke-InstallServiceTest -HostAddress "127.0.0.1" -Port 3003 -ServiceInstall:$false -TaskTargetsLegacyInstall -RemoveStartupShortcutBeforeInstall
    Assert-Equal $false ($installTaskToStartupMigrationResult.Events -contains "StartStartupShortcut") "install.ps1 should not convert a task-based service into a Startup shortcut when rerun without -Service."
    Assert-Equal $false ($installTaskToStartupMigrationResult.Events -contains "StartScheduledTask") "install.ps1 should not restart the scheduled task when rerun without -Service."

    $installWildcardResult = Invoke-InstallServiceTest -HostAddress "::" -Port 3003
    Assert-True ($installWildcardResult.Output -contains "  http://localhost:3003") "install.ps1 should print localhost for unspecified IPv6 dashboard URLs."

    $installFallbackResult = Invoke-InstallServiceTest -HostAddress "127.0.0.1" -Port 3003 -FailScheduledTaskAction
    Assert-True ($installFallbackResult.Events -contains "SaveShortcut") "install.ps1 should create a Startup shortcut when scheduled task registration setup fails."
    Assert-True ($installFallbackResult.Events -contains "StartFallbackProcess") "install.ps1 should start the fallback background runner when scheduled task setup fails."
    Assert-True ($installFallbackResult.Output -contains "  Registered in:   Startup folder") "install.ps1 should report Startup folder registration after falling back."

    $installPostRegistrationFailureResult = Invoke-InstallServiceTest -HostAddress "127.0.0.1" -Port 3003 -FailStartScheduledTask
    Assert-True ($installPostRegistrationFailureResult.Events -contains "RegisterScheduledTask") "install.ps1 should still register the scheduled task before a post-registration start failure."
    Assert-Equal $false ($installPostRegistrationFailureResult.Events -contains "SaveShortcut") "install.ps1 should not fall back to the Startup shortcut after scheduled task registration succeeds."
    Assert-Equal $false ($installPostRegistrationFailureResult.Events -contains "StartFallbackProcess") "install.ps1 should not launch the fallback runner after scheduled task registration succeeds."
    Assert-True ($installPostRegistrationFailureResult.Output -contains "  Registered task: TokenUsageInsights_test-user (Task Scheduler)") "install.ps1 should continue reporting the scheduled task after a post-registration start failure."

    $installWhatIfResult = Invoke-InstallServiceTest -HostAddress "127.0.0.1" -Port 3003 -WhatIf
    Assert-Equal $true $installWhatIfResult.StartupShortcutExists "install.ps1 should not remove an existing Startup shortcut during -WhatIf."
    Assert-Equal $false ($installWhatIfResult.Output -contains "Token 戰情室 installed.") "install.ps1 should not output completion message during -WhatIf."
    Assert-Equal $false ($installWhatIfResult.Output -contains "  Registered in:   Startup folder") "install.ps1 should not report service registration during -WhatIf."

    Write-Host "Windows collector smoke tests passed."
} finally {
    $env:ANTIGRAVITY_DIR = $PreviousAntigravityDir
    $env:COPILOT_DIR = $PreviousCopilotDir
    if (Test-Path -LiteralPath $Root) {
        Remove-Item -LiteralPath $Root -Recurse -Force
    }
}
