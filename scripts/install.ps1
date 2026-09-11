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
    param(
        [switch]$EnsureDirectory
    )

    $startupFolder = [Environment]::GetFolderPath('Startup')
    if (-not (Test-Path $startupFolder)) {
        $startupFolder = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Startup"
    }
    if ($EnsureDirectory -and (-not (Test-Path $startupFolder))) {
        New-Item -ItemType Directory -Force -Path $startupFolder | Out-Null
    }

    Join-Path $startupFolder "$AppName.lnk"
}

function Stop-ExistingServiceInstance {
    param(
        [string]$TaskName,
        [string]$ProcessName,
        [string]$InstallDir,
        [string]$StartupShortcutPath
    )

    $targetDirs = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::OrdinalIgnoreCase)
    if ($InstallDir) {
        $null = $targetDirs.Add([IO.Path]::GetFullPath($InstallDir))
    }

    try {
        $existingTaskObj = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
        if ($existingTaskObj -and $existingTaskObj.Actions) {
            foreach ($act in $existingTaskObj.Actions) {
                if ($act.WorkingDirectory) {
                    $null = $targetDirs.Add([IO.Path]::GetFullPath($act.WorkingDirectory))
                }
                if ($act.Argument -and $act.Argument -match '-InstallDir\s+"?([^"]+?)(?:"|\s|$)') {
                    $null = $targetDirs.Add([IO.Path]::GetFullPath($Matches[1]))
                }
                if ($act.Argument -and $act.Argument -match '-File\s+"?([^"]+?[\\/]run-service\.ps1)"?') {
                    $scriptDir = Split-Path -Parent $Matches[1]
                    $installParent = Split-Path -Parent $scriptDir
                    if ($installParent) {
                        $null = $targetDirs.Add([IO.Path]::GetFullPath($installParent))
                    }
                }
            }
        }
    } catch {}

    if ($StartupShortcutPath -and (Test-Path $StartupShortcutPath)) {
        try {
            $wsh = New-Object -ComObject WScript.Shell
            $sc = $wsh.CreateShortcut($StartupShortcutPath)
            if ($sc.WorkingDirectory) {
                $null = $targetDirs.Add([IO.Path]::GetFullPath($sc.WorkingDirectory))
            }
            if ($sc.Arguments -and $sc.Arguments -match '-InstallDir\s+"?([^"]+?)(?:"|\s|$)') {
                $null = $targetDirs.Add([IO.Path]::GetFullPath($Matches[1]))
            }
            if ($sc.Arguments -and $sc.Arguments -match '-File\s+"?([^"]+?[\\/]run-service\.ps1)"?') {
                $scriptDir = Split-Path -Parent $Matches[1]
                $installParent = Split-Path -Parent $scriptDir
                if ($installParent) {
                    $null = $targetDirs.Add([IO.Path]::GetFullPath($installParent))
                }
            }
        } catch {}
    }

    try {
        Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    } catch {}

    $runnerScriptPaths = @($targetDirs | ForEach-Object { [IO.Path]::GetFullPath((Join-Path $_ "scripts\run-service.ps1")) })
    $appExecutablePaths = @($targetDirs | ForEach-Object { [IO.Path]::GetFullPath((Join-Path $_ "$ProcessName.exe")) })

    $runnerHosts = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object {
        $cmd = $_.CommandLine
        if (($_.Name -in @("powershell.exe", "pwsh.exe")) -and $cmd) {
            $cmdNorm = $cmd.Replace('\', '/')
            $match = $false
            foreach ($scriptPath in $runnerScriptPaths) {
                $scriptPathNorm = $scriptPath.Replace('\', '/')
                $quoted = "-File `"$scriptPathNorm`""
                $unquoted = "-File $scriptPathNorm"
                if (
                    $cmdNorm.IndexOf($quoted, [System.StringComparison]::OrdinalIgnoreCase) -ge 0 -or
                    $cmdNorm.IndexOf($unquoted, [System.StringComparison]::OrdinalIgnoreCase) -ge 0
                ) {
                    $match = $true
                    break
                }
            }
            $match
        } else {
            $false
        }
    })
    $runnerHostIds = @($runnerHosts | ForEach-Object { $_.ProcessId })
    foreach ($runnerHost in $runnerHosts) {
        Stop-Process -Id $runnerHost.ProcessId -Force -ErrorAction SilentlyContinue
    }

    $appProcesses = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object {
        if ($_.Name -eq "$ProcessName.exe") {
            $exePath = $_.ExecutablePath
            $cmd = $_.CommandLine
            $exePathNorm = if ($exePath) { [IO.Path]::GetFullPath($exePath).Replace('\', '/') } else { $null }
            $cmdNorm = if ($cmd) { $cmd.Replace('\', '/') } else { $null }
            $match = $false
            foreach ($exeTarget in $appExecutablePaths) {
                $exeTargetNorm = $exeTarget.Replace('\', '/')
                if (
                    ($exePathNorm -and $exePathNorm.Equals($exeTargetNorm, [System.StringComparison]::OrdinalIgnoreCase)) -or
                    ($cmdNorm -and (
                        $cmdNorm.IndexOf("`"$exeTargetNorm`"", [System.StringComparison]::OrdinalIgnoreCase) -ge 0 -or
                        $cmdNorm.IndexOf($exeTargetNorm, [System.StringComparison]::OrdinalIgnoreCase) -ge 0
                    ))
                ) {
                    $match = $true
                    break
                }
            }
            $match
        } else {
            $false
        }
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
$registeredAsTask = $false

if ($PSCmdlet.ShouldProcess($InstallDir, "Install Token Usage Insights")) {
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    New-Item -ItemType Directory -Force -Path $BinDir | Out-Null

    $existingTask = $false
    try {
        $existingTask = [bool](Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue)
    } catch {}
    $StartupShortcut = Get-StartupShortcutPath
    $existingShortcut = Test-Path $StartupShortcut

    if ($Service -or $existingTask -or $existingShortcut -or (Get-Process -Name $AppName -ErrorAction SilentlyContinue)) {
        Stop-ExistingServiceInstance -TaskName $TaskName -ProcessName $AppName -InstallDir $InstallDir -StartupShortcutPath $StartupShortcut
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

            $StartupShortcut = Get-StartupShortcutPath -EnsureDirectory
            $WshShell = New-Object -ComObject WScript.Shell
            $Shortcut = $WshShell.CreateShortcut($StartupShortcut)
            $Shortcut.TargetPath = "powershell.exe"
            $Shortcut.Arguments = "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File `"$RunnerScript`" -InstallDir `"$InstallDir`" -HostAddress `"$HostAddress`" -Port $Port"
            $Shortcut.WorkingDirectory = $InstallDir
            $Shortcut.WindowStyle = 7
            $Shortcut.Description = "Token 戰情室 Dashboard Background Service"
            $Shortcut.Save()

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
            if ($PSCmdlet.ShouldProcess($StartupShortcut, "Remove stale Startup shortcut")) {
                try {
                    Remove-Item -Force -Path $StartupShortcut -ErrorAction Stop
                } catch {
                    Write-Warning "Scheduled task registered, but removing the stale Startup shortcut failed: $($_.Exception.Message)"
                }
            }
        }
    } elseif ($existingTask) {
        try {
            Start-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
        } catch {}
    } elseif ($existingShortcut -and (Test-Path $StartupShortcut)) {
        try {
            Start-Process $StartupShortcut
        } catch {}
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
