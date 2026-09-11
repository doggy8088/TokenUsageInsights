[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [string]$InstallDir = $(
        if ($env:LOCALAPPDATA) { Join-Path $env:LOCALAPPDATA "TokenUsageInsights" }
        else { Join-Path $HOME "AppData\Local\TokenUsageInsights" }
    ),
    [string]$BinDir = $(Join-Path $HOME "bin"),
    [string]$HostAddress = $(if ($env:HOST) { $env:HOST } else { "0.0.0.0" }),
    [int]$Port = $(if ($env:PORT) { [int]$env:PORT } else { 3003 }),
    [switch]$Service
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

function Set-StartupShortcutForRunner {
    param(
        [string]$ShortcutPath,
        [string]$RunnerScript,
        [string]$InstallDir,
        [string]$HostAddress,
        [int]$Port
    )

    $WshShell = New-Object -ComObject WScript.Shell
    $Shortcut = $WshShell.CreateShortcut($ShortcutPath)
    $Shortcut.TargetPath = "powershell.exe"
    $Shortcut.Arguments = "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File `"$RunnerScript`" -InstallDir `"$InstallDir`" -HostAddress `"$HostAddress`" -Port $Port"
    $Shortcut.WorkingDirectory = $InstallDir
    $Shortcut.WindowStyle = 7
    $Shortcut.Description = "Token 戰情室 Dashboard Background Service"
    $Shortcut.Save()
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
    $taskNamesToStop = @($TaskName)
    try {
        $existingTask = [bool](Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue)
        if ($TaskName -ne "TokenUsageInsights") {
            $legacyTaskName = "TokenUsageInsights"
            $legacyTask = Get-ScheduledTask -TaskName $legacyTaskName -ErrorAction SilentlyContinue
            if ($legacyTask) {
                $existingTask = $true
                $taskNamesToStop += $legacyTaskName
            } else {
                $legacyTaskName = $null
            }
        }
    } catch {}
    $hadScheduledTaskBeforeStop = $existingTask
    $hadStartupShortcutBeforeStop = $false
    $startupShortcutPath = Get-StartupShortcutPath
    $existingShortcut = Test-Path $startupShortcutPath
    $hadStartupShortcutBeforeStop = $existingShortcut
    $hadPersistentServiceRegistration = $hadScheduledTaskBeforeStop -or $existingShortcut

    if ($Service -or $hadPersistentServiceRegistration) {
        Stop-ExistingServiceInstance -TaskNames $taskNamesToStop -ProcessName $AppName -InstallDir $InstallDir
    }

    if ($legacyTaskName) {
        try {
            Unregister-ScheduledTask -TaskName $legacyTaskName -Confirm:$false -ErrorAction SilentlyContinue
        } catch {}
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
    if (Test-Path -LiteralPath $MarkerFile) {
        Remove-Item -LiteralPath $MarkerFile -Force -ErrorAction SilentlyContinue
    }
    Set-Content -LiteralPath $MarkerTmp -Value "token-usage-insights:installed" -NoNewline -Encoding Ascii
    Move-Item -LiteralPath $MarkerTmp -Destination $MarkerFile -Force

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

        $taskRegistered = $false
        try {
            $taskLogonUser = Get-ScheduledTaskLogonUser
            $Action = New-ScheduledTaskAction `
                -Execute "powershell.exe" `
                -Argument "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File `"$RunnerScript`" -InstallDir `"$InstallDir`" -HostAddress `"$HostAddress`" -Port $Port" `
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
            $taskRegistered = $true
        } catch {
            Write-Warning "Could not register scheduled task: $($_.Exception.Message). Falling back to Startup folder..."
            try {
                Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
            } catch {}

            $taskStillExists = $false
            try {
                $taskStillExists = [bool](Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue)
            } catch {}

            if ($taskStillExists) {
                throw "Could not register scheduled task and failed to unregister existing task '$TaskName'. Aborting fallback to prevent duplicate execution."
            }

            $startupShortcutPath = Get-StartupShortcutPath -EnsureDirectory
            Set-StartupShortcutForRunner `
                -ShortcutPath $startupShortcutPath `
                -RunnerScript $RunnerScript `
                -InstallDir $InstallDir `
                -HostAddress $HostAddress `
                -Port $Port

            Start-Process -FilePath "powershell.exe" `
                -ArgumentList "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File `"$RunnerScript`" -InstallDir `"$InstallDir`" -HostAddress `"$HostAddress`" -Port $Port" `
                -WorkingDirectory $InstallDir -WindowStyle Hidden
        }

        if ($taskRegistered) {
            $registeredAsTask = $true

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
        $startupShortcutReady = Test-Path $startupShortcutPath
        if ($hadStartupShortcutBeforeStop -and (Test-Path $runnerScript) -and -not $startupShortcutReady) {
            try {
                $startupShortcutPath = Get-StartupShortcutPath -EnsureDirectory
                Set-StartupShortcutForRunner `
                    -ShortcutPath $startupShortcutPath `
                    -RunnerScript $runnerScript `
                    -InstallDir $InstallDir `
                    -HostAddress $HostAddress `
                    -Port $Port
                $startupShortcutReady = Test-Path $startupShortcutPath
            } catch {}
        }

        if ($hadStartupShortcutBeforeStop -and $startupShortcutReady) {
            try {
                Start-Process $startupShortcutPath
            } catch {}
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
