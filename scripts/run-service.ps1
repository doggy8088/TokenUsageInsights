<#
.SYNOPSIS
  Background service runner for Token 戰情室 on Windows.
#>
[CmdletBinding()]
param(
    [string]$InstallDir = (Split-Path -Parent $PSScriptRoot),
    [string]$HostAddress = $(if ($env:HOST) { $env:HOST } else { "0.0.0.0" }),
    [int]$Port = $(if ($env:PORT) { [int]$env:PORT } else { 3003 })
)

$ErrorActionPreference = "Stop"
$AppName = "token-usage-insights"
$InstallDir = [IO.Path]::GetFullPath([Environment]::ExpandEnvironmentVariables($InstallDir))

$env:PORT = "$Port"
$env:HOST = "$HostAddress"
$env:TOKEN_USAGE_INSIGHTS_SERVICE = "1"
$env:TOKEN_USAGE_INSIGHTS_INSTALL_DIR = "$InstallDir"

$Exe = Join-Path $InstallDir "$AppName.exe"
if (!(Test-Path $Exe)) {
    throw "Executable not found: $Exe"
}

$LogDir = Join-Path $InstallDir "logs"
if (!(Test-Path $LogDir)) {
    New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
}
$OutLog = Join-Path $LogDir "$AppName.out.log"
$ErrLog = Join-Path $LogDir "$AppName.err.log"
$MaxHistoryBytes = 5MB
$MaxActiveLogBytes = 10MB

function Rotate-ServiceLog {
    param(
        [string]$CurrentLogPath,
        [string]$PreviousLogPath,
        [string]$HistoryLogPath
    )

    if (!(Test-Path $CurrentLogPath)) {
        return
    }

    $currentLogItem = Get-Item -LiteralPath $CurrentLogPath -ErrorAction SilentlyContinue
    if (-not $currentLogItem -or $currentLogItem.Length -le 0) {
        return
    }

    try {
        $rotationCompleted = $false
        $historyLogItem = Get-Item -LiteralPath $HistoryLogPath -ErrorAction SilentlyContinue
        $historyLogLength = if ($historyLogItem) { $historyLogItem.Length } else { 0 }
        $currentLogLength = $currentLogItem.Length
        $resetHistory = $currentLogLength -ge $MaxHistoryBytes -or ($historyLogLength + $currentLogLength) -gt $MaxHistoryBytes
        $bytesToCopy = [Math]::Min([int64]$currentLogLength, [int64]$MaxHistoryBytes)
        if (-not $resetHistory) {
            $remainingHistoryBudget = [Math]::Max([int64]0, [int64]($MaxHistoryBytes - $historyLogLength))
            $bytesToCopy = [Math]::Min($bytesToCopy, $remainingHistoryBudget)
        }
        if ($bytesToCopy -le 0) {
            $rotationCompleted = $true
        } else {
            $readStream = [System.IO.File]::Open($CurrentLogPath, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
            try {
                $historyFileMode = if ($resetHistory) { [System.IO.FileMode]::Create } else { [System.IO.FileMode]::Append }
                $writeStream = [System.IO.File]::Open($HistoryLogPath, $historyFileMode, [System.IO.FileAccess]::Write, [System.IO.FileShare]::ReadWrite)
                try {
                    if ($currentLogLength -gt $bytesToCopy) {
                        $readStream.Seek(-$bytesToCopy, [System.IO.SeekOrigin]::End) | Out-Null

                        while ($true) {
                            $candidateByte = $readStream.ReadByte()
                            if ($candidateByte -lt 0) {
                                break
                            }
                            if (($candidateByte -band 0xC0) -ne 0x80) {
                                $readStream.Seek(-1, [System.IO.SeekOrigin]::Current) | Out-Null
                                break
                            }
                        }

                        while ($true) {
                            $nextByte = $readStream.ReadByte()
                            if ($nextByte -lt 0 -or $nextByte -eq 10) {
                                if ($nextByte -eq 10) {
                                    $readStream.Seek(-1, [System.IO.SeekOrigin]::Current) | Out-Null
                                }
                                break
                            }
                            if ($nextByte -eq 13) {
                                $followingByte = $readStream.ReadByte()
                                if ($followingByte -eq 10) {
                                    $readStream.Seek(-2, [System.IO.SeekOrigin]::Current) | Out-Null
                                } elseif ($followingByte -ge 0) {
                                    $readStream.Seek(-1, [System.IO.SeekOrigin]::Current) | Out-Null
                                } else {
                                    $readStream.Seek(-1, [System.IO.SeekOrigin]::Current) | Out-Null
                                }
                                break
                            }
                        }
                    }
                    $readStream.CopyTo($writeStream)
                    $rotationCompleted = $true
                } finally {
                    $writeStream.Dispose()
                }
            } finally {
                $readStream.Dispose()
            }
        }
    } catch {
        Write-Warning "Log rotation failed for ${CurrentLogPath}: $($_.Exception.Message)"
    }

    if ($rotationCompleted) {
        try {
            Move-Item -LiteralPath $CurrentLogPath -Destination $PreviousLogPath -Force -ErrorAction Stop
        } catch {
            Write-Warning "Log rotation move failed for ${CurrentLogPath}: $($_.Exception.Message)"
            $timestamp = (Get-Date).ToString("yyyyMMddHHmmss")
            $fallbackPrev = "${PreviousLogPath}.${timestamp}.bak"
            try {
                Move-Item -LiteralPath $CurrentLogPath -Destination $fallbackPrev -Force -ErrorAction Stop
            } catch {
                Write-Warning "Fallback log rotation move failed for ${CurrentLogPath}: $($_.Exception.Message)"
            }
        }
    }
}

while ($true) {
    Rotate-ServiceLog `
        -CurrentLogPath $OutLog `
        -PreviousLogPath (Join-Path $LogDir "$AppName.prev.out.log") `
        -HistoryLogPath (Join-Path $LogDir "$AppName.history.out.log")
    Rotate-ServiceLog `
        -CurrentLogPath $ErrLog `
        -PreviousLogPath (Join-Path $LogDir "$AppName.prev.err.log") `
        -HistoryLogPath (Join-Path $LogDir "$AppName.history.err.log")

    $Process = $null
    $Process = Start-Process -FilePath $Exe `
        -WorkingDirectory $InstallDir `
        -WindowStyle Hidden `
        -RedirectStandardOutput $OutLog `
        -RedirectStandardError $ErrLog `
        -PassThru

    $restartForLogRotation = $false
    try {
        while (-not $Process.WaitForExit(5000)) {
            $outItem = Get-Item -LiteralPath $OutLog -ErrorAction SilentlyContinue
            $errItem = Get-Item -LiteralPath $ErrLog -ErrorAction SilentlyContinue
            if (($outItem -and $outItem.Length -ge $MaxActiveLogBytes) -or ($errItem -and $errItem.Length -ge $MaxActiveLogBytes)) {
                Write-Warning "Active log size exceeded ${MaxActiveLogBytes} bytes. Restarting service to rotate logs..."
                $restartForLogRotation = $true
                Stop-Process -Id $Process.Id -Force -ErrorAction SilentlyContinue
                try {
                    $null = $Process.WaitForExit(5000)
                } catch {}
                break
            }
        }
    } finally {
        if ($Process -and -not $Process.HasExited) {
            Stop-Process -Id $Process.Id -Force -ErrorAction SilentlyContinue
            try {
                $null = $Process.WaitForExit(5000)
            } catch {}
        }
    }

    # Exit code 75 indicates the process completed an auto-update and requested the runner to restart it
    if ($restartForLogRotation -or $Process.ExitCode -eq 75) {
        continue
    }

    # If an external updater requested this service to stop for an update,
    # wait for the update to complete and restart the service
    $restartPendingFile = Join-Path $InstallDir ".service_restart_pending"
    if (Test-Path $restartPendingFile) {
        $lockFile = Join-Path $InstallDir ".update.lock"
        $waitCount = 0
        while ((Test-Path $lockFile) -and $waitCount -lt 900) {
            Start-Sleep -Milliseconds 100
            $waitCount++
        }
        Remove-Item -LiteralPath $restartPendingFile -Force -ErrorAction SilentlyContinue
        continue
    }

    exit $Process.ExitCode
}
