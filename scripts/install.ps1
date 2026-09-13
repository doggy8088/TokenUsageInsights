[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [string]$InstallDir = $(
        if ($env:LOCALAPPDATA) { Join-Path $env:LOCALAPPDATA "TokenUsageInsights" }
        else { Join-Path $HOME "AppData\Local\TokenUsageInsights" }
    ),
    [string]$BinDir = $(Join-Path $HOME "bin"),
    [string]$HostAddress = $(if ($env:HOST) { $env:HOST } else { "0.0.0.0" }),
    [int]$Port = $(if ($env:PORT) { [int]$env:PORT } else { 3003 }),
    [switch]$Service,
    [string]$AutoUpdate = $(if ($env:TOKEN_USAGE_INSIGHTS_AUTO_UPDATE) { $env:TOKEN_USAGE_INSIGHTS_AUTO_UPDATE } else { "" }),
    [string]$UpdateIntervalHours = $(if ($env:TOKEN_USAGE_INSIGHTS_UPDATE_INTERVAL_HOURS) { $env:TOKEN_USAGE_INSIGHTS_UPDATE_INTERVAL_HOURS } else { "" })
)

$ErrorActionPreference = "Stop"
$AppName = "token-usage-insights"
$InstallDir = [IO.Path]::GetFullPath([Environment]::ExpandEnvironmentVariables($InstallDir))
$BinDir = [IO.Path]::GetFullPath([Environment]::ExpandEnvironmentVariables($BinDir))

function Get-StartupShortcutPath {
    param([switch]$EnsureDirectory)

    $startupFolder = $env:TOKEN_USAGE_INSIGHTS_STARTUP_DIR
    if (-not $startupFolder) {
        $startupFolder = [Environment]::GetFolderPath('Startup')
    }

    $fallbackStartupFolder = $null
    if ($env:APPDATA) {
        $fallbackStartupFolder = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Startup"
    }

    if (-not $startupFolder) {
        $startupFolder = $fallbackStartupFolder
    } elseif (
        -not (Test-Path $startupFolder) -and
        $fallbackStartupFolder -and
        -not $startupFolder.Equals($fallbackStartupFolder, [System.StringComparison]::OrdinalIgnoreCase)
    ) {
        $startupFolder = $fallbackStartupFolder
    }

    if ($EnsureDirectory -and $startupFolder -and -not (Test-Path $startupFolder)) {
        New-Item -ItemType Directory -Force -Path $startupFolder | Out-Null
    }

    Join-Path $startupFolder "$AppName.lnk"
}

function Get-RunnerScriptPathFromArguments {
    param([string]$Arguments)

    if (-not $Arguments) {
        return $null
    }

    $match = [regex]::Match($Arguments, '(?i)-(?:File|f)(?:\s+|:)(?:"([^"]+)"|(\S+))')
    if (-not $match.Success) {
        return $null
    }

    $runnerScriptPath = $match.Groups[1].Value
    if (-not $runnerScriptPath) {
        $runnerScriptPath = $match.Groups[2].Value
    }
    if (-not $runnerScriptPath) {
        return $null
    }

    $expandedRunnerScriptPath = [Environment]::ExpandEnvironmentVariables($runnerScriptPath)
    return [IO.Path]::GetFullPath($expandedRunnerScriptPath)
}

function Get-RunnerArgumentValue {
    param(
        [string]$Arguments,
        [string]$ParameterName
    )

    if (-not $Arguments -or -not $ParameterName) {
        return $null
    }

    $pattern = '(?i)-(?:' + [regex]::Escape($ParameterName) + ')(?:\s+|:)(?:"([^"]*)"|(\S+))'
    $match = [regex]::Match($Arguments, $pattern)
    if ($match.Success) {
        if ($match.Groups[1].Success) {
            return $match.Groups[1].Value
        }
        return $match.Groups[2].Value
    }
    return $null
}

function Stop-ExistingServiceInstance {
    param(
        [string[]]$TaskNames,
        [string]$ProcessName,
        [string]$InstallDir
    )

    $runnerScriptPath = [IO.Path]::GetFullPath((Join-Path $InstallDir "scripts\run-service.ps1"))
    $runnerScriptPaths = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::OrdinalIgnoreCase)
    $null = $runnerScriptPaths.Add($runnerScriptPath)

    try {
        foreach ($knownTaskName in @($TaskNames | Where-Object { $_ } | Select-Object -Unique)) {
            $existingTask = Get-ScheduledTask -TaskName $knownTaskName -ErrorAction SilentlyContinue
            if ($existingTask -and $existingTask.Actions) {
                foreach ($action in @($existingTask.Actions)) {
                    $taskRunnerPath = Get-RunnerScriptPathFromArguments -Arguments $action.Arguments
                    if ($taskRunnerPath) {
                        $null = $runnerScriptPaths.Add($taskRunnerPath)
                    }
                }
            }
        }
    } catch {}

    try {
        $startupShortcutPath = Get-StartupShortcutPath
        if (Test-Path $startupShortcutPath) {
            $wshShell = New-Object -ComObject WScript.Shell
            $startupShortcut = $wshShell.CreateShortcut($startupShortcutPath)
            $shortcutRunnerPath = Get-RunnerScriptPathFromArguments -Arguments $startupShortcut.Arguments
            if (-not $shortcutRunnerPath -and $startupShortcut.TargetPath -and $startupShortcut.TargetPath.EndsWith(".ps1", [System.StringComparison]::OrdinalIgnoreCase)) {
                $shortcutRunnerPath = [IO.Path]::GetFullPath([Environment]::ExpandEnvironmentVariables($startupShortcut.TargetPath))
            }
            if ($shortcutRunnerPath) {
                $null = $runnerScriptPaths.Add($shortcutRunnerPath)
            }
        }
    } catch {}

    $runnerFileArguments = @()
    foreach ($knownRunnerScriptPath in $runnerScriptPaths) {
        $runnerFileArguments += "-File `"$knownRunnerScriptPath`""
        $runnerFileArguments += "-File $knownRunnerScriptPath"
        $runnerFileArguments += "-File:`"$knownRunnerScriptPath`""
        $runnerFileArguments += "-File:$knownRunnerScriptPath"
        $runnerFileArguments += "-f `"$knownRunnerScriptPath`""
        $runnerFileArguments += "-f $knownRunnerScriptPath"
        $runnerFileArguments += "-f:`"$knownRunnerScriptPath`""
        $runnerFileArguments += "-f:$knownRunnerScriptPath"
        $runnerScriptPathWithBackslashes = $knownRunnerScriptPath.Replace('/', '\')
        $runnerFileArguments += "-File `"$runnerScriptPathWithBackslashes`""
        $runnerFileArguments += "-File $runnerScriptPathWithBackslashes"
        $runnerFileArguments += "-File:`"$runnerScriptPathWithBackslashes`""
        $runnerFileArguments += "-File:$runnerScriptPathWithBackslashes"
        $runnerFileArguments += "-f `"$runnerScriptPathWithBackslashes`""
        $runnerFileArguments += "-f $runnerScriptPathWithBackslashes"
        $runnerFileArguments += "-f:`"$runnerScriptPathWithBackslashes`""
        $runnerFileArguments += "-f:$runnerScriptPathWithBackslashes"
    }

    $appExecutablePaths = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::OrdinalIgnoreCase)
    $appExecutablePath = [IO.Path]::GetFullPath((Join-Path $InstallDir "$ProcessName.exe"))
    $null = $appExecutablePaths.Add($appExecutablePath)
    foreach ($knownRunnerScriptPath in $runnerScriptPaths) {
        $runnerScriptDir = Split-Path -Parent $knownRunnerScriptPath
        if (-not $runnerScriptDir) {
            continue
        }

        $runnerInstallDir = Split-Path -Parent $runnerScriptDir
        if (-not $runnerInstallDir) {
            continue
        }

        $runnerExecutablePath = [IO.Path]::GetFullPath((Join-Path $runnerInstallDir "$ProcessName.exe"))
        $null = $appExecutablePaths.Add($runnerExecutablePath)
    }

    try {
        foreach ($knownTaskName in @($TaskNames | Where-Object { $_ } | Select-Object -Unique)) {
            Stop-ScheduledTask -TaskName $knownTaskName -ErrorAction SilentlyContinue
        }
    } catch {}

    $runnerHosts = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object {
        ($_.Name -in @("powershell.exe", "pwsh.exe")) -and
        $_.CommandLine -and
        $(
            $matchesRunnerArgument = $false
            foreach ($runnerFileArgument in $runnerFileArguments) {
                if ($_.CommandLine.IndexOf($runnerFileArgument, [System.StringComparison]::OrdinalIgnoreCase) -ge 0) {
                    $matchesRunnerArgument = $true
                    break
                }
            }
            $matchesRunnerArgument
        )
    })
    $runnerHostIds = @($runnerHosts | ForEach-Object { $_.ProcessId })
    foreach ($runnerHost in $runnerHosts) {
        Stop-Process -Id $runnerHost.ProcessId -Force -ErrorAction SilentlyContinue
    }

    $appProcesses = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object {
        ($_.Name -eq "$ProcessName.exe") -and
        $(
            $matchesExecutablePath = $false
            foreach ($knownExecutablePath in $appExecutablePaths) {
                if (
                    ($_.ExecutablePath -and $_.ExecutablePath.Replace('/', '\').Equals($knownExecutablePath.Replace('/', '\'), [System.StringComparison]::OrdinalIgnoreCase)) -or
                    ($_.CommandLine -and (
                        $_.CommandLine.IndexOf("`"$knownExecutablePath`"", [System.StringComparison]::OrdinalIgnoreCase) -ge 0 -or
                        $_.CommandLine.IndexOf($knownExecutablePath, [System.StringComparison]::OrdinalIgnoreCase) -ge 0 -or
                        $_.CommandLine.IndexOf($knownExecutablePath.Replace('/', '\'), [System.StringComparison]::OrdinalIgnoreCase) -ge 0
                    ))
                ) {
                    $matchesExecutablePath = $true
                    break
                }
            }
            $matchesExecutablePath
        )
    })
    $appProcessIds = @($appProcesses | ForEach-Object { $_.ProcessId })
    foreach ($appProcess in $appProcesses) {
        Stop-Process -Id $appProcess.ProcessId -Force -ErrorAction SilentlyContinue
    }

    $deadline = (Get-Date).AddSeconds(15)
    while (@($runnerHostIds | Where-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue }).Count -gt 0 -and (Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 250
    }
    while (@($appProcessIds | Where-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue }).Count -gt 0 -and (Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 250
    }

    $remainingRunnerHostIds = @($runnerHostIds | Where-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue })
    if ($remainingRunnerHostIds.Count -gt 0) {
        throw "Failed to stop the existing background runner before reinstalling."
    }

    $remainingAppProcessIds = @($appProcessIds | Where-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue })
    if ($remainingAppProcessIds.Count -gt 0) {
        throw "Failed to stop the existing $ProcessName process before reinstalling."
    }
}

function Get-DashboardDisplayHost {
    param([string]$HostAddress)

    $parsedIpAddress = $null
    if ([System.Net.IPAddress]::TryParse($HostAddress, [ref]$parsedIpAddress)) {
        if (
            $parsedIpAddress.Equals([System.Net.IPAddress]::Any) -or
            $parsedIpAddress.Equals([System.Net.IPAddress]::IPv6Any)
        ) {
            return "localhost"
        }

        if ($parsedIpAddress.AddressFamily -eq [System.Net.Sockets.AddressFamily]::InterNetworkV6) {
            return "[$HostAddress]"
        }
    }

    return $HostAddress
}

function Get-ScheduledTaskLogonUser {
    try {
        $identity = [System.Security.Principal.WindowsIdentity]::GetCurrent()
        if ($identity -and $identity.Name) {
            return $identity.Name
        }
    } catch {}

    if ($env:USERDOMAIN -and $env:USERNAME) {
        return "$($env:USERDOMAIN)\$($env:USERNAME)"
    }

    if ($env:COMPUTERNAME -and $env:USERNAME) {
        return "$($env:COMPUTERNAME)\$($env:USERNAME)"
    }

    return $null
}

function Format-RunnerArgumentValue([string]$value) {
    if ($null -eq $value) {
        return '""'
    }
    # 跳脫值內的反引號與雙引號，防範參數注入與引號截斷
    $escaped = $value.Replace('`', '``').Replace('"', '\"')
    return "`"$escaped`""
}

function Format-RunnerArgumentString {
    param(
        [string]$RunnerScript,
        [string]$InstallDir,
        [string]$HostAddress,
        [int]$Port,
        [AllowNull()][string]$AutoUpdate = $null,
        [AllowNull()][string]$UpdateIntervalHours = $null
    )

    $runnerScriptQuoted = Format-RunnerArgumentValue $RunnerScript
    $installDirQuoted = Format-RunnerArgumentValue $InstallDir
    $hostAddressQuoted = Format-RunnerArgumentValue $HostAddress

    $runnerArgs = "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File $runnerScriptQuoted -InstallDir $installDirQuoted -HostAddress $hostAddressQuoted -Port $Port"
    if ($null -ne $AutoUpdate) {
        $autoUpdateQuoted = Format-RunnerArgumentValue $AutoUpdate
        $runnerArgs += " -AutoUpdate $autoUpdateQuoted"
    }
    if ($null -ne $UpdateIntervalHours) {
        $updateIntervalHoursQuoted = Format-RunnerArgumentValue $UpdateIntervalHours
        $runnerArgs += " -UpdateIntervalHours $updateIntervalHoursQuoted"
    }
    return $runnerArgs
}

function Set-StartupShortcutForRunner {
    param(
        [string]$ShortcutPath,
        [string]$RunnerScript,
        [string]$InstallDir,
        [string]$HostAddress,
        [int]$Port,
        [AllowNull()][string]$AutoUpdate = $null,
        [AllowNull()][string]$UpdateIntervalHours = $null
    )

    $runnerArgs = Format-RunnerArgumentString `
        -RunnerScript $RunnerScript `
        -InstallDir $InstallDir `
        -HostAddress $HostAddress `
        -Port $Port `
        -AutoUpdate $AutoUpdate `
        -UpdateIntervalHours $UpdateIntervalHours

    $WshShell = New-Object -ComObject WScript.Shell
    $Shortcut = $WshShell.CreateShortcut($ShortcutPath)
    $Shortcut.TargetPath = "powershell.exe"
    $Shortcut.Arguments = $runnerArgs
    $Shortcut.WorkingDirectory = $InstallDir
    $Shortcut.WindowStyle = 7
    $Shortcut.Description = "Token 戰情室 Dashboard Background Service"
    $Shortcut.Save()
}

function Register-DashboardScheduledTask {
    param(
        [string]$TaskName,
        [string]$RunnerScript,
        [string]$InstallDir,
        [string]$HostAddress,
        [int]$Port,
        [AllowNull()][string]$AutoUpdate = $null,
        [AllowNull()][string]$UpdateIntervalHours = $null
    )

    $runnerArgs = Format-RunnerArgumentString `
        -RunnerScript $RunnerScript `
        -InstallDir $InstallDir `
        -HostAddress $HostAddress `
        -Port $Port `
        -AutoUpdate $AutoUpdate `
        -UpdateIntervalHours $UpdateIntervalHours

    $taskLogonUser = Get-ScheduledTaskLogonUser
    $Action = New-ScheduledTaskAction `
        -Execute "powershell.exe" `
        -Argument $runnerArgs `
        -WorkingDirectory $InstallDir

    if ($taskLogonUser) {
        $Trigger = New-ScheduledTaskTrigger -AtLogOn -User $taskLogonUser
    } else {
        $Trigger = New-ScheduledTaskTrigger -AtLogOn
    }

    $Settings = New-ScheduledTaskSettingsSet `
        -AllowStartIfOnBatteries `
        -DontStopIfGoingOnBatteries `
        -ExecutionTimeLimit ([TimeSpan]::Zero) `
        -RestartCount 3 `
        -RestartInterval (New-TimeSpan -Minutes 1)

    Register-ScheduledTask `
        -TaskName $TaskName `
        -Action $Action `
        -Trigger $Trigger `
        -Settings $Settings `
        -Description "Token 戰情室 Dashboard Background Service" `
        -Force | Out-Null
}

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
if (Test-Path (Join-Path $ScriptDir "$AppName.exe")) {
    $ReleaseDir = $ScriptDir
} else {
    $ReleaseDir = Split-Path -Parent $ScriptDir
}

$BinarySrc = Join-Path $ReleaseDir "$AppName.exe"
if (!(Test-Path $BinarySrc)) {
    throw "Missing executable: $BinarySrc. Run this installer from an extracted Token 戰情室 release package."
}

foreach ($RequiredItem in @("static", "pricing.csv")) {
    if (!(Test-Path (Join-Path $ReleaseDir $RequiredItem))) {
        throw "Incomplete release package: missing $RequiredItem in $ReleaseDir"
    }
}

$TaskName = "TokenUsageInsights"
if ($env:USERNAME) {
    $TaskName = "TokenUsageInsights_$env:USERNAME"
}
$registeredAsTask = $false

if ($PSCmdlet.ShouldProcess($InstallDir, "Install Token Usage Insights")) {
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    New-Item -ItemType Directory -Force -Path $BinDir | Out-Null

    $existingTask = $false
    $legacyTaskName = $null
    $detectedTaskName = $null
    $taskNamesToStop = @($TaskName)
    $existingTaskArguments = $null
    $legacyTaskArguments = $null
    try {
        $foundTask = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
        if ($foundTask) {
            $existingTask = $true
            $detectedTaskName = $TaskName
            if ($foundTask.Actions) {
                $firstAction = $foundTask.Actions | Select-Object -First 1
                if ($firstAction) {
                    $existingTaskArguments = $firstAction.Arguments
                }
            }
        }
        if ($TaskName -ne "TokenUsageInsights") {
            $legacyTask = Get-ScheduledTask -TaskName "TokenUsageInsights" -ErrorAction SilentlyContinue
            if ($legacyTask) {
                $existingTask = $true
                $legacyTaskName = "TokenUsageInsights"
                $taskNamesToStop += "TokenUsageInsights"
                if (-not $detectedTaskName) {
                    $detectedTaskName = "TokenUsageInsights"
                }
                if ($legacyTask.Actions) {
                    $firstLegacyAction = $legacyTask.Actions | Select-Object -First 1
                    if ($firstLegacyAction) {
                        $legacyTaskArguments = $firstLegacyAction.Arguments
                        if (-not $existingTaskArguments) {
                            $existingTaskArguments = $firstLegacyAction.Arguments
                        }
                    }
                }
            }
        }
    } catch {}
    $hadScheduledTaskBeforeStop = $existingTask
    $hadStartupShortcutBeforeStop = $false
    $startupShortcutPath = Get-StartupShortcutPath
    $existingShortcut = Test-Path $startupShortcutPath
    $existingShortcutArguments = $null
    if ($existingShortcut) {
        try {
            $wshShell = New-Object -ComObject WScript.Shell
            $shortcutObj = $wshShell.CreateShortcut($startupShortcutPath)
            if ($shortcutObj) {
                $existingShortcutArguments = $shortcutObj.Arguments
            }
        } catch {}
    }
    $hadStartupShortcutBeforeStop = $existingShortcut
    $hadPersistentServiceRegistration = $hadScheduledTaskBeforeStop -or $existingShortcut

    # 若未明確指定更新設定，自動繼承既有服務排程或捷徑中的設定，避免重新安裝時遺失原更新策略
    $persistedAutoUpdate = $null
    if ($PSBoundParameters.ContainsKey('AutoUpdate')) {
        $persistedAutoUpdate = $AutoUpdate
    } elseif ($null -ne [System.Environment]::GetEnvironmentVariable("TOKEN_USAGE_INSIGHTS_AUTO_UPDATE") -and [System.Environment]::GetEnvironmentVariable("TOKEN_USAGE_INSIGHTS_AUTO_UPDATE") -ne "") {
        $persistedAutoUpdate = $AutoUpdate
    }

    $persistedUpdateInterval = $null
    if ($PSBoundParameters.ContainsKey('UpdateIntervalHours')) {
        $persistedUpdateInterval = $UpdateIntervalHours
    } elseif ($null -ne [System.Environment]::GetEnvironmentVariable("TOKEN_USAGE_INSIGHTS_UPDATE_INTERVAL_HOURS") -and [System.Environment]::GetEnvironmentVariable("TOKEN_USAGE_INSIGHTS_UPDATE_INTERVAL_HOURS") -ne "") {
        $persistedUpdateInterval = $UpdateIntervalHours
    }

    $candidateServiceArguments = @($existingTaskArguments, $legacyTaskArguments, $existingShortcutArguments) | Where-Object { $_ }
    if ($null -eq $persistedAutoUpdate) {
        foreach ($candArgs in $candidateServiceArguments) {
            $existingAutoUpdate = Get-RunnerArgumentValue -Arguments $candArgs -ParameterName "AutoUpdate"
            if ($null -ne $existingAutoUpdate) {
                $persistedAutoUpdate = $existingAutoUpdate
                $AutoUpdate = $existingAutoUpdate
                break
            }
        }
    }
    if ($null -eq $persistedUpdateInterval) {
        foreach ($candArgs in $candidateServiceArguments) {
            $existingInterval = Get-RunnerArgumentValue -Arguments $candArgs -ParameterName "UpdateIntervalHours"
            if ($null -ne $existingInterval) {
                $persistedUpdateInterval = $existingInterval
                $UpdateIntervalHours = $existingInterval
                break
            }
        }
    }

    if ($Service -or $hadPersistentServiceRegistration) {
        Stop-ExistingServiceInstance -TaskNames $taskNamesToStop -ProcessName $AppName -InstallDir $InstallDir
    }

    Copy-Item -Force $BinarySrc (Join-Path $InstallDir "$AppName.exe")

    foreach ($Item in @("static", "shell", "scripts")) {
        $Source = Join-Path $ReleaseDir $Item
        $Target = Join-Path $InstallDir $Item
        if (Test-Path $Source) {
            if (Test-Path $Target) {
                Remove-Item -Recurse -Force $Target
            }
            Copy-Item -Recurse -Force $Source $Target
        }
    }

    foreach ($File in @("pricing.csv", "README.md", "LICENSE", "VERSION")) {
        $Source = Join-Path $ReleaseDir $File
        if (Test-Path $Source) {
            Copy-Item -Force $Source (Join-Path $InstallDir $File)
        }
    }

    $MarkerFile = Join-Path $InstallDir ".install_marker"
    $MarkerTmp = Join-Path $InstallDir ".install_marker.tmp.$PID"
    Set-Content -LiteralPath $MarkerTmp -Value "token-usage-insights:installed" -NoNewline -Encoding Ascii
    if (Test-Path -LiteralPath $MarkerFile) {
        try {
            [System.IO.File]::Replace($MarkerTmp, $MarkerFile, $null)
        } catch {
            Move-Item -LiteralPath $MarkerTmp -Destination $MarkerFile -Force
        }
    } else {
        Move-Item -LiteralPath $MarkerTmp -Destination $MarkerFile -Force
    }

    $Shim = Join-Path $BinDir "$AppName.cmd"
    $BatchInstallDir = $InstallDir.Replace("%", "%%")
    @"
@echo off
setlocal
set "HOST=$HostAddress"
set "PORT=$Port"
pushd "$BatchInstallDir"
"$BatchInstallDir\$AppName.exe" %*
set "APP_EXIT_CODE=%ERRORLEVEL%"
popd
exit /b %APP_EXIT_CODE%
"@ | Set-Content -Encoding ASCII $Shim

    if ($Service) {
        $RunnerScript = Join-Path $InstallDir "scripts\run-service.ps1"
        if (!(Test-Path $RunnerScript)) {
            throw "Missing background service runner script: $RunnerScript"
        }

        # 持久化服務執行期環境變數至 .service.env，確保開機或排程啟動時能正確載入自訂目錄與設定
        $serviceEnvFile = Join-Path $InstallDir ".service.env"
        $runtimeEnvVars = @(
            "INSIGHTS_DIR",
            "ANTIGRAVITY_DIR",
            "COPILOT_DIR",
            "CODEX_DIR",
            "CLAUDE_DIR",
            "CURSOR_DIR",
            "GROK_DIR",
            "PI_DIR",
            "OMP_DIR",
            "CORS_ALLOW_ORIGIN"
        )
        $persistedEnvs = @{}
        if (Test-Path -LiteralPath $serviceEnvFile) {
            try {
                Get-Content -LiteralPath $serviceEnvFile | ForEach-Object {
                    $line = $_.Trim()
                    if ($line -and (-not $line.StartsWith("#")) -and ($line -match '^([^=]+)=(.*)$')) {
                        $persistedEnvs[$matches[1].Trim()] = $matches[2]
                    }
                }
            } catch {}
        }
        foreach ($var in $runtimeEnvVars) {
            $envVal = [Environment]::GetEnvironmentVariable($var, "Process")
            if ($null -ne $envVal -and $envVal -ne "") {
                $persistedEnvs[$var] = $envVal
            }
        }
        if ($persistedEnvs.Count -gt 0) {
            $envLines = @()
            foreach ($k in ($persistedEnvs.Keys | Sort-Object)) {
                $envLines += "$k=$($persistedEnvs[$k])"
            }
            Set-Content -LiteralPath $serviceEnvFile -Value $envLines -Encoding UTF8
        }

        $runnerArgs = Format-RunnerArgumentString `
            -RunnerScript $RunnerScript `
            -InstallDir $InstallDir `
            -HostAddress $HostAddress `
            -Port $Port `
            -AutoUpdate $persistedAutoUpdate `
            -UpdateIntervalHours $persistedUpdateInterval

        $taskRegistered = $false
        try {
            Register-DashboardScheduledTask `
                -TaskName $TaskName `
                -RunnerScript $RunnerScript `
                -InstallDir $InstallDir `
                -HostAddress $HostAddress `
                -Port $Port `
                -AutoUpdate $persistedAutoUpdate `
                -UpdateIntervalHours $persistedUpdateInterval
            $taskRegistered = $true
        } catch {
            Write-Warning "Could not register scheduled task: $($_.Exception.Message). Falling back to Startup folder..."
            try {
                Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
            } catch {}

            if ($legacyTaskName -and ($legacyTaskName -ne $TaskName)) {
                try {
                    Unregister-ScheduledTask -TaskName $legacyTaskName -Confirm:$false -ErrorAction SilentlyContinue
                } catch {}
            }

            $survivingTask = $null
            try {
                if ([bool](Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue)) {
                    $survivingTask = $TaskName
                } elseif ($legacyTaskName -and ($legacyTaskName -ne $TaskName) -and [bool](Get-ScheduledTask -TaskName $legacyTaskName -ErrorAction SilentlyContinue)) {
                    $survivingTask = $legacyTaskName
                }
            } catch {}

            if ($survivingTask) {
                # Stop-ExistingServiceInstance 已先停止既有服務；在拋出例外中止 fallback 前嘗試重啟留存之排程工作，避免服務離線
                try {
                    Start-ScheduledTask -TaskName $survivingTask -ErrorAction SilentlyContinue
                } catch {}
                throw "Could not register scheduled task and failed to unregister existing task. Aborting fallback to prevent duplicate execution."
            }

            $startupShortcutPath = Get-StartupShortcutPath -EnsureDirectory
            Set-StartupShortcutForRunner `
                -ShortcutPath $startupShortcutPath `
                -RunnerScript $RunnerScript `
                -InstallDir $InstallDir `
                -HostAddress $HostAddress `
                -Port $Port `
                -AutoUpdate $persistedAutoUpdate `
                -UpdateIntervalHours $persistedUpdateInterval

            Start-Process -FilePath "powershell.exe" `
                -ArgumentList $runnerArgs `
                -WorkingDirectory $InstallDir -WindowStyle Hidden
        }

        if ($taskRegistered) {
            $registeredAsTask = $true

            if ($legacyTaskName -and ($legacyTaskName -ne $TaskName)) {
                try {
                    Unregister-ScheduledTask -TaskName $legacyTaskName -Confirm:$false -ErrorAction SilentlyContinue
                } catch {}

                $legacyStillExists = $false
                try {
                    $legacyStillExists = [bool](Get-ScheduledTask -TaskName $legacyTaskName -ErrorAction SilentlyContinue)
                } catch {}

                if ($legacyStillExists) {
                    try {
                        Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
                    } catch {}
                    # Stop-ExistingServiceInstance 已先停止 legacy task；回滾新 task 註冊後，恢復啟動 legacy task 以免服務離線
                    try {
                        Start-ScheduledTask -TaskName $legacyTaskName -ErrorAction SilentlyContinue
                    } catch {}
                    throw "Scheduled task '$TaskName' registered, but failed to unregister legacy task '$legacyTaskName'. Aborting to prevent duplicate execution."
                }
            }

            try {
                Start-ScheduledTask -TaskName $TaskName
            } catch {
                Write-Warning "Scheduled task registered, but automatic start failed: $($_.Exception.Message)"
            }

            # Registration in Task Scheduler succeeded; remove any stale Startup folder shortcut
            # to avoid dual launches on logon.
            if (Test-Path $startupShortcutPath) {
                if ($PSCmdlet.ShouldProcess($startupShortcutPath, "Remove stale Startup shortcut")) {
                    try {
                        Remove-Item -Force -Path $startupShortcutPath -ErrorAction Stop
                    } catch {
                        throw "Scheduled task registered, but removing the stale Startup shortcut failed: $($_.Exception.Message). Please remove it manually to avoid duplicate execution: $startupShortcutPath"
                    }
                }
            }
        }
    } elseif ($hadPersistentServiceRegistration) {
        $runnerScript = Join-Path (Join-Path $InstallDir "scripts") "run-service.ps1"
        if ($hadScheduledTaskBeforeStop) {
            # 排程工作為 Windows 優先服務常駐方式。若先前同時殘留 Startup 捷徑，刪除過期捷徑避免雙重常駐搶佔連接埠
            if (Test-Path $startupShortcutPath) {
                try {
                    Remove-Item -Force -Path $startupShortcutPath -ErrorAction SilentlyContinue
                } catch {}
            }

            # 嘗試移轉或更新排程工作至目前使用者名稱隔離之 $TaskName
            $taskToStart = $null
            $migratedOrUpdated = $false
            if (Test-Path $runnerScript) {
                try {
                    Register-DashboardScheduledTask `
                        -TaskName $TaskName `
                        -RunnerScript $runnerScript `
                        -InstallDir $InstallDir `
                        -HostAddress $HostAddress `
                        -Port $Port `
                        -AutoUpdate $persistedAutoUpdate `
                        -UpdateIntervalHours $persistedUpdateInterval
                    $taskRegisteredSuccessfully = $true
                    if ($legacyTaskName -and ($legacyTaskName -ne $TaskName)) {
                        try {
                            Unregister-ScheduledTask -TaskName $legacyTaskName -Confirm:$false -ErrorAction SilentlyContinue
                        } catch {}

                        $legacyStillExists = $false
                        try {
                            $legacyStillExists = [bool](Get-ScheduledTask -TaskName $legacyTaskName -ErrorAction SilentlyContinue)
                        } catch {}

                        if ($legacyStillExists) {
                            try {
                                Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
                            } catch {}
                            $taskRegisteredSuccessfully = $false
                        }
                    }
                    if ($taskRegisteredSuccessfully) {
                        $migratedOrUpdated = $true
                        $taskToStart = $TaskName
                    }
                } catch {}
            }

            if (-not $migratedOrUpdated) {
                # 若移轉失敗且舊版排程工作仍存在，退回啟動舊版工作以防服務離線；否則啟動原偵測之工作
                if ($legacyTaskName -and [bool](Get-ScheduledTask -TaskName $legacyTaskName -ErrorAction SilentlyContinue)) {
                    $taskToStart = $legacyTaskName
                } else {
                    $taskToStart = $detectedTaskName
                }
            }

            if ($taskToStart) {
                try {
                    Start-ScheduledTask -TaskName $taskToStart
                } catch {}
            }
        } elseif ($hadStartupShortcutBeforeStop -and (Test-Path $runnerScript)) {
            try {
                $startupShortcutPath = Get-StartupShortcutPath -EnsureDirectory
                Set-StartupShortcutForRunner `
                    -ShortcutPath $startupShortcutPath `
                    -RunnerScript $runnerScript `
                    -InstallDir $InstallDir `
                    -HostAddress $HostAddress `
                    -Port $Port `
                    -AutoUpdate $persistedAutoUpdate `
                    -UpdateIntervalHours $persistedUpdateInterval
            } catch {}

            if (Test-Path $startupShortcutPath) {
                try {
                    Start-Process $startupShortcutPath
                } catch {}
            }
        }
    }

    Write-Host "Token 戰情室 installed."
    Write-Host ""
    Write-Host "Install directory:"
    Write-Host "  $InstallDir"
    Write-Host ""
    Write-Host "Executable shim:"
    Write-Host "  $(Join-Path $BinDir "$AppName.cmd")"
    Write-Host ""
    if ($Service) {
        Write-Host "Background service:"
        if ($registeredAsTask) {
            Write-Host "  Registered task: $TaskName (Task Scheduler)"
        } else {
            Write-Host "  Registered in:   Startup folder"
        }
        Write-Host "  Logs directory:  $(Join-Path $InstallDir 'logs')"
        Write-Host ""
        $displayHost = Get-DashboardDisplayHost -HostAddress $HostAddress
        Write-Host "Dashboard URL:"
        Write-Host "  http://${displayHost}:${Port}"
    } else {
        Write-Host "Run:"
        Write-Host "  $(Join-Path $BinDir "$AppName.cmd")"
    }
}
