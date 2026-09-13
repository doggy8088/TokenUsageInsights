<#
.SYNOPSIS
  Background service runner for Token 戰情室 on Windows.
#>
[CmdletBinding()]
param(
    [string]$InstallDir = (Split-Path -Parent $PSScriptRoot),
    [string]$HostAddress = $(if ($env:HOST) { $env:HOST } else { "0.0.0.0" }),
    [int]$Port = $(if ($env:PORT) { [int]$env:PORT } else { 3003 }),
    [AllowNull()][string]$AutoUpdate = $null,
    [AllowNull()][string]$UpdateIntervalHours = $null
)

$ErrorActionPreference = "Stop"
$AppName = "token-usage-insights"
$InstallDir = [IO.Path]::GetFullPath([Environment]::ExpandEnvironmentVariables($InstallDir))

# 載入持久化之服務環境變數 (.service.env)，還原自訂 INSIGHTS_DIR、各 Agent 目錄與 CORS 等設定
$serviceEnvFile = Join-Path $InstallDir ".service.env"
if (Test-Path -LiteralPath $serviceEnvFile) {
    try {
        Get-Content -LiteralPath $serviceEnvFile | ForEach-Object {
            $line = $_.Trim()
            if ($line -and (-not $line.StartsWith("#")) -and ($line -match '^([^=]+)=(.*)$')) {
                $envKey = $matches[1].Trim()
                $envVal = $matches[2]
                [Environment]::SetEnvironmentVariable($envKey, $envVal, "Process")
            }
        }
    } catch {}
}

$env:PORT = "$Port"
$env:HOST = "$HostAddress"
$env:TOKEN_USAGE_INSIGHTS_SERVICE = "1"
$env:TOKEN_USAGE_INSIGHTS_INSTALL_DIR = "$InstallDir"
if ($PSBoundParameters.ContainsKey('AutoUpdate')) {
    if ($AutoUpdate) {
        $env:TOKEN_USAGE_INSIGHTS_AUTO_UPDATE = "$AutoUpdate"
    } else {
        Remove-Item Env:\TOKEN_USAGE_INSIGHTS_AUTO_UPDATE -ErrorAction SilentlyContinue
    }
}

if ($PSBoundParameters.ContainsKey('UpdateIntervalHours')) {
    if ($UpdateIntervalHours) {
        $env:TOKEN_USAGE_INSIGHTS_UPDATE_INTERVAL_HOURS = "$UpdateIntervalHours"
    } else {
        Remove-Item Env:\TOKEN_USAGE_INSIGHTS_UPDATE_INTERVAL_HOURS -ErrorAction SilentlyContinue
    }
}

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

function Exit-WithError {
    param(
        [string]$Message
    )

    Write-Error -Message $Message -ErrorAction Continue
    exit 1
}

function Test-IsRollbackFailed {
    param(
        [string]$InstallDir
    )

    $rollbackFailedMarker = Join-Path $InstallDir ".backup\.rollback_failed"
    $directFailedMarker = Join-Path $InstallDir ".rollback_failed"
    return ((Test-Path -LiteralPath $rollbackFailedMarker) -or (Test-Path -LiteralPath $directFailedMarker))
}

function Test-IsUpdateLockHeld {
    param(
        [string]$LockFile
    )

    if (-not (Test-Path -LiteralPath $LockFile)) {
        return $false
    }

    try {
        $stream = [System.IO.File]::Open($LockFile, [System.IO.FileMode]::Open, [System.IO.FileAccess]::ReadWrite, [System.IO.FileShare]::ReadWrite)
        try {
            $stream.Lock(0, 1)
            $stream.Unlock(0, 1)
            return $false
        } catch {
            return $true
        } finally {
            $stream.Dispose()
        }
    } catch {
        return $true
    }
}

function Wait-ForUpdateLockRelease {
    param(
        [string]$LockFile,
        [int]$MaxWaitDeciseconds = 900
    )

    $waitCount = 0
    while ($waitCount -lt $MaxWaitDeciseconds) {
        if (-not (Test-IsUpdateLockHeld -LockFile $LockFile)) {
            return $true
        }
        Start-Sleep -Milliseconds 100
        $waitCount++
    }

    return (-not (Test-IsUpdateLockHeld -LockFile $LockFile))
}

function Wait-ForExecutableReady {
    param(
        [string]$InstallDir,
        [string]$ExePath
    )

    # 0. 優先檢查是否存有更新回滾失敗標記；若回滾失敗，立即終止並保留備份以供手動修復
    if (Test-IsRollbackFailed -InstallDir $InstallDir) {
        Exit-WithError -Message "偵測到先前更新回滾失敗標記 (.backup\.rollback_failed)；為防止載入損毀之安裝狀態，服務終止運行並保留備份以供手動修復。"
    }

    # 1. 等待更新鎖 (.update.lock) 釋放（確保 updater 程序及任何更新鎖定已完全釋放）
    $lockFile = Join-Path $InstallDir ".update.lock"
    if (-not (Wait-ForUpdateLockRelease -LockFile $lockFile -MaxWaitDeciseconds 900)) {
        Exit-WithError -Message "等待更新程序釋放更新鎖逾時（90 秒），保持停止狀態退出。"
    }

    # 2. 等待 self_replace 或替換 helper 完成：確保執行檔存在且可獨占讀取（無寫入鎖定），且無臨時置換殘留檔
    $readyCount = 0
    $exeReady = $false
    while ($readyCount -lt 150) {
        if (Test-Path -LiteralPath $ExePath) {
            try {
                $exeStream = [System.IO.File]::Open($ExePath, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::Read)
                $exeStream.Dispose()
                $tempReplacements = @(Get-ChildItem -LiteralPath $InstallDir -Filter "*.__temp__.exe" -ErrorAction SilentlyContinue)
                $relocatedReplacements = @(Get-ChildItem -LiteralPath $InstallDir -Filter "*.__relocated__.exe" -ErrorAction SilentlyContinue)
                if ($tempReplacements.Count -eq 0 -and $relocatedReplacements.Count -eq 0) {
                    $exeReady = $true
                    break
                }
            } catch {}
        }
        Start-Sleep -Milliseconds 100
        $readyCount++
    }

    if (-not $exeReady) {
        Exit-WithError -Message "等待執行檔就緒逾時（15 秒），執行檔仍未就緒或臨時替換檔殘留。保留就緒與重啟標記以利後續復原，保持停止狀態退出。"
    }

    # 2.5 驗證執行檔版本是否與 VERSION 檔案一致（若存在 VERSION 檔案），防止載入未完成置換之舊版二進位檔
    $versionFile = Join-Path $InstallDir "VERSION"
    if (Test-Path -LiteralPath $versionFile) {
        $expectedVer = (Get-Content -LiteralPath $versionFile -Raw).Trim().TrimStart('v').TrimStart('V')
        if ($expectedVer) {
            $pinfo = New-Object System.Diagnostics.ProcessStartInfo
            $pinfo.FileName = $ExePath
            $pinfo.Arguments = '--version'
            $pinfo.RedirectStandardOutput = $true
            $pinfo.RedirectStandardError = $true
            $pinfo.UseShellExecute = $false
            $pinfo.CreateNoWindow = $true

            $proc = New-Object System.Diagnostics.Process
            $proc.StartInfo = $pinfo
            if ($proc.Start()) {
                $exited = $proc.WaitForExit(5000)
                if (-not $exited) {
                    try { $proc.Kill() } catch {}
                    Exit-WithError -Message "執行檔版本檢查逾時（5 秒），二進位檔可能異常；中止啟動以確保安全。"
                }
                $stdout = $proc.StandardOutput.ReadToEnd()
                $stderr = $proc.StandardError.ReadToEnd()
                $verOutput = if ($stdout) { $stdout.Trim() } else { $stderr.Trim() }
                $tokens = $verOutput -split '\s+'
                $actualVer = if ($tokens.Count -gt 0) { $tokens[-1].TrimStart('v').TrimStart('V') } else { '' }
                if ($actualVer -ne $expectedVer) {
                    Exit-WithError -Message "執行檔版本 ($verOutput) 與 VERSION 檔案 ($expectedVer) 不符，中止啟動以確保安全。"
                }
            } else {
                Exit-WithError -Message "無法啟動執行檔進行版本檢查，中止啟動以確保安全。"
            }
        }
    }

    # 3. 版本與執行檔驗證成功後，清理備份目錄與更新協商就緒標記檔
    $backupDir = Join-Path $InstallDir ".backup"
    if (Test-Path -LiteralPath $backupDir) {
        Remove-Item -LiteralPath $backupDir -Recurse -Force -ErrorAction SilentlyContinue
    }
    $readyMarker = Join-Path $InstallDir ".update_ready"
    $restartPending = Join-Path $InstallDir ".service_restart_pending"
    Remove-Item -LiteralPath $readyMarker -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $restartPending -Force -ErrorAction SilentlyContinue
}

function Wait-ForUpdateCompletion {
    param(
        [string]$InstallDir,
        [string]$RestartPendingFile
    )

    $lockFile = Join-Path $InstallDir ".update.lock"
    $hasPendingMarker = Test-Path -LiteralPath $RestartPendingFile
    $isLocked = Test-IsUpdateLockHeld -LockFile $lockFile

    if ($hasPendingMarker -or $isLocked) {
        if (Test-IsRollbackFailed -InstallDir $InstallDir) {
            Exit-WithError -Message "偵測到更新程序遺留之回滾失敗標記 (.backup\.rollback_failed)；中止重啟以確保安全，保持停止狀態退出。"
        }

        if (-not (Wait-ForUpdateLockRelease -LockFile $lockFile -MaxWaitDeciseconds 900)) {
            Exit-WithError -Message "等待更新程序完成逾時（90 秒），更新鎖仍未釋放。為防止損毀安裝目錄，保持停止狀態退出。"
        }

        Wait-ForExecutableReady -InstallDir $InstallDir -ExePath (Join-Path $InstallDir "$AppName.exe")
        return $true
    }

    return $false
}

$readyMarker = Join-Path $InstallDir ".update_ready"
$restartPendingFile = Join-Path $InstallDir ".service_restart_pending"

while ($true) {
    if (Test-IsRollbackFailed -InstallDir $InstallDir) {
        Exit-WithError -Message "偵測到先前更新回滾失敗標記 (.backup\.rollback_failed)；為防止載入損毀之安裝狀態，服務終止運行並保留備份以供手動修復。"
    }

    if ((Test-Path -LiteralPath $readyMarker) -or (Test-Path -LiteralPath $restartPendingFile)) {
        Wait-ForExecutableReady -InstallDir $InstallDir -ExePath $Exe
    }

    Rotate-ServiceLog `
        -CurrentLogPath $OutLog `
        -PreviousLogPath (Join-Path $LogDir "$AppName.prev.out.log") `
        -HistoryLogPath (Join-Path $LogDir "$AppName.history.out.log")
    Rotate-ServiceLog `
        -CurrentLogPath $ErrLog `
        -PreviousLogPath (Join-Path $LogDir "$AppName.prev.err.log") `
        -HistoryLogPath (Join-Path $LogDir "$AppName.history.err.log")

    if (Test-IsRollbackFailed -InstallDir $InstallDir) {
        Exit-WithError -Message "偵測到先前更新回滾失敗標記 (.backup\.rollback_failed)；為防止載入損毀之安裝狀態，服務終止運行並保留備份以供手動修復。"
    }

    $Process = $null
    $Process = Start-Process -FilePath $Exe `
        -WorkingDirectory $InstallDir `
        -WindowStyle Hidden `
        -RedirectStandardOutput $OutLog `
        -RedirectStandardError $ErrLog `
        -PassThru

    $restartForLogRotation = $false
    try {
        while (-not $Process.WaitForExit(1000)) {
            if (Test-Path -LiteralPath $restartPendingFile) {
                Write-Host "偵測到更新程序已啟動並設定重啟協商標記，正在協調停止目前服務進程..."
                Stop-Process -Id $Process.Id -Force -ErrorAction SilentlyContinue
                try {
                    $null = $Process.WaitForExit(5000)
                } catch {}
                break
            }

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

    # Exit code 75 indicates the process completed an auto-update and requested the runner to restart it.
    # 必須以同步協定等待新執行檔完全置換並就緒後才可重啟，防範與 self_replace helper 競爭。
    if ($Process.ExitCode -eq 75) {
        Wait-ForExecutableReady -InstallDir $InstallDir -ExePath $Exe
        continue
    }

    # 檢查是否有外部更新程序要求重啟或正在替換檔案；若有更新正在進行，等待鎖釋放後再重啟
    # 注意：在日誌輪轉重啟 ($restartForLogRotation) 前必須先檢查此項，防止輪轉與更新併發時誤啟動舊進程
    $updateCompleted = Wait-ForUpdateCompletion -InstallDir $InstallDir -RestartPendingFile $restartPendingFile
    if ($updateCompleted) {
        continue
    }

    if ($restartForLogRotation) {
        continue
    }

    exit $Process.ExitCode
}
