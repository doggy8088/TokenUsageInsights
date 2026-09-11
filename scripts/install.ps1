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
    $startupFolder = [Environment]::GetFolderPath('Startup')
    if (-not (Test-Path $startupFolder)) {
        $startupFolder = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Startup"
    }

    Join-Path $startupFolder "$AppName.lnk"
}

function Stop-ExistingServiceInstance {
    param(
        [string]$TaskName,
        [string]$ProcessName,
        [string]$InstallDir
    )

    $runnerScriptPath = [IO.Path]::GetFullPath((Join-Path $InstallDir "scripts\run-service.ps1"))
    $quotedRunnerFileArgument = "-File `"$runnerScriptPath`""
    $unquotedRunnerFileArgument = "-File $runnerScriptPath"

    try {
        Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    } catch {}

    $runnerHosts = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object {
        ($_.Name -in @("powershell.exe", "pwsh.exe")) -and
        $_.CommandLine -and
        (
            $_.CommandLine.IndexOf($quotedRunnerFileArgument, [System.StringComparison]::OrdinalIgnoreCase) -ge 0 -or
            $_.CommandLine.IndexOf($unquotedRunnerFileArgument, [System.StringComparison]::OrdinalIgnoreCase) -ge 0
        )
    })
    $runnerHostIds = @($runnerHosts | ForEach-Object { $_.ProcessId })
    foreach ($runnerHost in $runnerHosts) {
        Stop-Process -Id $runnerHost.ProcessId -Force -ErrorAction SilentlyContinue
    }

    $runningProcesses = @(Get-Process -Name $ProcessName -ErrorAction SilentlyContinue)
    if ($runningProcesses.Count -gt 0) {
        $runningProcesses | Stop-Process -Force -ErrorAction SilentlyContinue
    }

    $deadline = (Get-Date).AddSeconds(15)
    while (($runnerHostIds | Where-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue }) -and (Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 250
    }
    while ((Get-Process -Name $ProcessName -ErrorAction SilentlyContinue) -and (Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 250
    }

    if ($runnerHostIds | Where-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue }) {
        throw "Failed to stop the existing background runner before reinstalling."
    }

    if (Get-Process -Name $ProcessName -ErrorAction SilentlyContinue) {
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

    if ($Service) {
        Stop-ExistingServiceInstance -TaskName $TaskName -ProcessName $AppName -InstallDir $InstallDir
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

        $StartupShortcut = Get-StartupShortcutPath

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

            Start-ScheduledTask -TaskName $TaskName
            $registeredAsTask = $true

            # Registration in Task Scheduler succeeded; remove any stale Startup folder shortcut
            # to avoid dual launches on logon.
            Remove-Item -Force -Path $StartupShortcut -ErrorAction SilentlyContinue
        } catch {
            Write-Warning "Could not register scheduled task: $($_.Exception.Message). Falling back to Startup folder..."
            try {
                Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
            } catch {}

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
