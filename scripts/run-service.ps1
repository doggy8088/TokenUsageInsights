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

function Exit-WithRollback {
    param(
        [string]$InstallDir,
        [string]$Message
    )

    # 啟動驗證失敗時若留存更新備份，先自 .backup 回滾再退出：
    # 否則備份會殘留並讓後續重試持續撞上同一驗證失敗，服務永遠無法回到可用版本。
    # 回滾期間必須獨占更新鎖，避免其他更新程序同時更動同一份備份交易
    $backupDir = Join-Path $InstallDir ".backup"
    $lockFile = Join-Path $InstallDir ".update.lock"
    if (Test-Path -LiteralPath $backupDir) {
        $lockStream = Enter-UpdateLock -LockFile $lockFile
        if (-not $lockStream) {
            Write-Warning "無法取得更新鎖（其他更新程序可能正在進行），保留備份目錄且不執行回滾，避免與進行中的更新互相破壞。"
        } else {
            try {
                Write-Warning "啟動驗證失敗 ($Message)；正在自備份回滾至先前版本..."
                if (Restore-ServiceBackup -InstallDir $InstallDir) {
                    Write-Host "已成功完成備份交易處理（回滾或清理已提交備份）；服務將於下次啟動時載入對應版本。"
                } else {
                    Write-Warning "自備份回滾失敗，已保留備份目錄以供手動修復。"
                }
            } finally {
                $lockStream.Dispose()
            }
        }
    }

    Exit-WithError -Message $Message
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

function Enter-UpdateLock {
    param(
        [string]$LockFile
    )

    # 以獨占檔案共用模式開啟更新鎖檔：與 Rust 更新程序使用的 OS 檔案鎖互斥，
    # 讓呼叫端能在整個回滾／提交流程期間獨占更新權，避免「先檢查後使用」的 TOCTOU 競態
    try {
        return [System.IO.File]::Open($LockFile, [System.IO.FileMode]::OpenOrCreate, [System.IO.FileAccess]::ReadWrite, [System.IO.FileShare]::None)
    } catch {
        return $null
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

function Stop-ServiceProcessGracefully {
    param(
        [System.Diagnostics.Process]$Process,
        [string]$InstallDir,
        [int]$TimeoutSeconds = 30
    )

    # 透過協商標記要求看板進程優雅停機（完成進行中的資料庫寫入後自行退出），
    # 逾時不強制終止：強制終止會中斷 spawn_blocking 中的 SQLite 寫入，由呼叫端 fail closed 處理
    $stopRequestFile = Join-Path $InstallDir ".service_stop_requested"
    Set-Content -LiteralPath $stopRequestFile -Value "stop" -Force -ErrorAction SilentlyContinue
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while (((Get-Date) -lt $deadline) -and (-not $Process.HasExited)) {
        Start-Sleep -Milliseconds 200
    }
    Remove-Item -LiteralPath $stopRequestFile -Force -ErrorAction SilentlyContinue
    if (-not $Process.HasExited) {
        return $false
    }
    try {
        $null = $Process.WaitForExit(5000)
    } catch {}
    return $true
}

function Test-ServicePortResponding {
    param(
        [string]$HostAddress = $null,
        [int]$Port = 0,
        [int]$TimeoutMs = 1000
    )

    # 確認看板連接埠確實可連線：PID 檔僅代表已建立 PID 守衛，不足以證明服務可提供服務
    if ($Port -le 0) {
        return $false
    }

    $target = $HostAddress
    if ((-not $target) -or $target -eq "0.0.0.0" -or $target -eq "::" -or $target -eq "*") {
        $target = "127.0.0.1"
    }

    try {
        $client = New-Object System.Net.Sockets.TcpClient
        try {
            $iar = $client.BeginConnect($target, $Port, $null, $null)
            if (-not $iar.AsyncWaitHandle.WaitOne($TimeoutMs)) {
                return $false
            }
            $client.EndConnect($iar)
            return $true
        } finally {
            $client.Close()
        }
    } catch {
        return $false
    }
}

function Test-IsProcessHealthy {
    param(
        [System.Diagnostics.Process]$Process,
        [string]$InstallDir,
        [int]$TimeoutSeconds = 5,
        [string]$BindHostAddress = $script:HostAddress,
        [int]$BindPortAddress = $script:Port,
        [int]$HealthDwellMilliseconds = 1500
    )

    if (-not $Process) {
        return $false
    }

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    $pidFile = Join-Path $InstallDir ".server.pid"
    while ((Get-Date) -lt $deadline) {
        if ($Process.HasExited) {
            return $false
        }
        if (Test-Path -LiteralPath $pidFile) {
            try {
                $pidContent = (Get-Content -LiteralPath $pidFile -Raw).Trim()
                if ($pidContent -eq "$($Process.Id)") {
                    # PID 相符後仍需通過兩項健康證據：連接埠可連線，且程序於觀察窗口內持續存活；
                    # 否則主迴圈會立即標記 .committed 並刪除唯一備份，使後續啟動失敗無法回滾
                    if (($BindPortAddress -le 0) -or (Test-ServicePortResponding -HostAddress $BindHostAddress -Port $BindPortAddress)) {
                        $dwellDeadline = (Get-Date).AddMilliseconds($HealthDwellMilliseconds)
                        while ((Get-Date) -lt $dwellDeadline) {
                            if ($Process.HasExited) {
                                return $false
                            }
                            Start-Sleep -Milliseconds 100
                        }
                        return (-not $Process.HasExited)
                    }
                }
            } catch {}
        }
        Start-Sleep -Milliseconds 100
    }
    return $false
}

function Restore-ServiceBackup {
    param(
        [string]$InstallDir
    )

    $backupDir = Join-Path $InstallDir ".backup"

    # 先驗證備份根目錄本身為正規目錄：符號連結、重剖析點或非目錄會讓後續的讀取清單、
    # 複製與刪除操作落在無關目錄上，繞過清單白名單檢查而把外部檔案還原進安裝目錄
    $backupItem = Get-Item -LiteralPath $backupDir -Force -ErrorAction SilentlyContinue
    if (-not $backupItem) {
        return $false
    }
    if ((-not $backupItem.PSIsContainer) -or ($backupItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint)) {
        Set-Content -LiteralPath (Join-Path $InstallDir ".rollback_failed") -Value "unsafe backup directory (symbolic link, reparse point or non-directory): $backupDir" -Force -ErrorAction SilentlyContinue
        Write-Error -Message "備份目錄為符號連結、重剖析點或非正規目錄 ($backupDir)；拒絕還原以確保安全。" -ErrorAction Continue
        return $false
    }

    # .committed 代表新版已通過健康驗證、僅清理備份時被中斷：此時絕不可回滾，
    # 應保留目前版本並清理或隔離殘留備份目錄（與啟動救援協定一致）
    if (Test-Path -LiteralPath (Join-Path $backupDir ".committed")) {
        Write-Host "偵測到已提交的更新備份（.committed），保留目前版本並清理殘留備份目錄..."
        try {
            Remove-Item -LiteralPath $backupDir -Recurse -Force -ErrorAction Stop
        } catch {
            Write-Warning "清理已提交備份失敗: $($_.Exception.Message)，嘗試改名隔離..."
        }
        if (Test-Path -LiteralPath $backupDir) {
            $committedName = ".backup-committed-" + [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
            try {
                Move-Item -LiteralPath $backupDir -Destination (Join-Path $InstallDir $committedName) -Force -ErrorAction Stop
                Write-Host "已將殘留之已提交備份目錄隔離至: $committedName"
            } catch {
                Write-Warning "已提交備份目錄改名隔離失敗: $($_.Exception.Message)"
            }
        }
        return (-not (Test-Path -LiteralPath $backupDir))
    }

    $manifest = Join-Path $backupDir ".manifest"
    if (-not (Test-Path -LiteralPath $manifest)) {
        return $false
    }

    try {
        $originalItems = @(Get-Content -LiteralPath $manifest | ForEach-Object { $_.Trim() } | Where-Object { $_ })
        $managedItems = @(
            "token-usage-insights",
            "token-usage-insights.exe",
            "static",
            "model_prices_and_context_window.json",
            "pricing.csv",
            "shell",
            "scripts",
            "install.sh",
            "install.ps1",
            "VERSION",
            "README.md",
            "LICENSE",
            ".install_marker",
            ".service.env"
        )

        # 驗證備份清單僅包含受管理項目，防範遭竄改的清單以相對或絕對路徑跳出安裝目錄
        foreach ($rel in $originalItems) {
            if ($managedItems -notcontains $rel) {
                throw "備份清單包含非受管理項目 ($rel)，拒絕還原以防範路徑穿越攻擊。"
            }
        }

        # 清單必須包含平台執行檔與靜態資源，否則截斷或遭竄改的清單會在移除執行檔後無法還原
        foreach ($required in @("token-usage-insights.exe", "static")) {
            if ($originalItems -notcontains $required) {
                throw "備份清單缺少必要項目 ($required)，拒絕還原以避免安裝目錄失去執行檔或基礎資源。"
            }
        }

        # 驗證清單項目皆確實存在於備份目錄，避免截斷或遭竄改的備份被誤判為還原成功而留下混合版本
        foreach ($rel in $originalItems) {
            $relSrcPath = Join-Path $backupDir $rel
            if (-not (Test-Path -LiteralPath $relSrcPath)) {
                throw "備份清單項目不存在於備份目錄 ($rel)，拒絕還原以避免留下混合版本安裝。"
            }
            $relSrcItem = Get-Item -LiteralPath $relSrcPath -Force
            if ($relSrcItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) {
                throw "備份清單項目為符號連結或重剖析點 ($rel)，拒絕還原以確保安全。"
            }
        }

        # 1. 移除更新期間新增、但原始安裝中並不存在的受管理項目
        foreach ($m in $managedItems) {
            if ($originalItems -notcontains $m) {
                $p = Join-Path $InstallDir $m
                if (Test-Path -LiteralPath $p) {
                    Remove-Item -LiteralPath $p -Recurse -Force -ErrorAction Stop
                }
            }
        }

        # 2. 還原備份項目；若目標項目為目錄，先完整移除目標目錄再遞迴複製，防範新舊檔案混合
        foreach ($rel in $originalItems) {
            $src = Join-Path $backupDir $rel
            $dst = Join-Path $InstallDir $rel
            if (Test-Path -LiteralPath $src) {
                if (Test-Path -LiteralPath $dst) {
                    Remove-Item -LiteralPath $dst -Recurse -Force -ErrorAction Stop
                }
                $parent = Split-Path -Parent $dst
                if ($parent -and -not (Test-Path -LiteralPath $parent)) {
                    New-Item -ItemType Directory -Force -Path $parent | Out-Null
                }
                Copy-Item -LiteralPath $src -Destination $dst -Force -Recurse -ErrorAction Stop
            }
        }
        if (Test-Path -LiteralPath $backupDir) {
            try {
                Remove-Item -LiteralPath $backupDir -Recurse -Force -ErrorAction Stop
            } catch {
                Write-Warning "清理已還原備份目錄失敗: $($_.Exception.Message)，嘗試改名隔離..."
            }
        }
        if (Test-Path -LiteralPath $backupDir) {
            $restoredName = ".backup-restored-" + [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
            $restoredDir = Join-Path $InstallDir $restoredName
            try {
                Move-Item -LiteralPath $backupDir -Destination $restoredDir -Force -ErrorAction Stop
                Write-Host "已將未清理之備份目錄隔離至: $restoredName"
            } catch {
                Write-Warning "備份目錄改名隔離失敗: $($_.Exception.Message)"
            }
        }
        if (Test-Path -LiteralPath $backupDir) {
            return $false
        }
        return $true
    } catch {
        $marker = Join-Path $backupDir ".rollback_failed"
        Set-Content -LiteralPath $marker -Value "run-service rollback failed: $_" -Force -ErrorAction SilentlyContinue
        $directMarker = Join-Path $InstallDir ".rollback_failed"
        Set-Content -LiteralPath $directMarker -Value "run-service rollback failed: $_" -Force -ErrorAction SilentlyContinue
        return $false
    }
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
        Exit-WithRollback -InstallDir $InstallDir -Message "等待執行檔就緒逾時（15 秒），執行檔仍未就緒或臨時替換檔殘留。已嘗試自備份回滾，保留就緒與重啟標記以利後續復原，保持停止狀態退出。"
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
                    Exit-WithRollback -InstallDir $InstallDir -Message "執行檔版本檢查逾時（5 秒），二進位檔可能異常；已嘗試自備份回滾並中止啟動以確保安全。"
                }
                $stdout = $proc.StandardOutput.ReadToEnd()
                $stderr = $proc.StandardError.ReadToEnd()
                $verOutput = if ($stdout) { $stdout.Trim() } else { $stderr.Trim() }
                $tokens = $verOutput -split '\s+'
                $actualVer = if ($tokens.Count -gt 0) { $tokens[-1].TrimStart('v').TrimStart('V') } else { '' }
                if ($actualVer -ne $expectedVer) {
                    Exit-WithRollback -InstallDir $InstallDir -Message "執行檔版本 ($verOutput) 與 VERSION 檔案 ($expectedVer) 不符；已嘗試自備份回滾並中止啟動以確保安全。"
                }
            } else {
                Exit-WithRollback -InstallDir $InstallDir -Message "無法啟動執行檔進行版本檢查；已嘗試自備份回滾並中止啟動以確保安全。"
            }
        }
    }

    # 3. 版本與執行檔初步驗證成功後，移除更新協商標記檔（保留 .backup 目錄直至新進程確認健康啟動）
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

    # 若存在更新備份目錄 (.backup)，監控新版服務進程是否確認健康就緒；若確認健康始清理備份，若啟動失敗則自備份自動回滾
    $backupDir = Join-Path $InstallDir ".backup"
    if (Test-Path -LiteralPath $backupDir) {
        $isHealthy = Test-IsProcessHealthy -Process $Process -InstallDir $InstallDir -TimeoutSeconds 5 -BindHostAddress $HostAddress -BindPortAddress $Port

        if ($isHealthy) {
            # 提交流程（標記 .committed、移除移交標記、清理備份）必須在獨占更新鎖保護下完成，
            # 且必須於健康驗證之後才取得鎖：新版看板在啟動救援階段同樣需要更新鎖才能完成就緒。
            # 取鎖可能與其他更新程序競爭，故採有界重試，避免交易永久殘留而阻擋後續更新
            $lockFile = Join-Path $InstallDir ".update.lock"
            $commitLock = $null
            $commitLockWait = 0
            while ((-not $commitLock) -and ($commitLockWait -lt 300)) {
                $commitLock = Enter-UpdateLock -LockFile $lockFile
                if (-not $commitLock) {
                    Start-Sleep -Milliseconds 200
                    $commitLockWait++
                }
            }
            if (-not $commitLock) {
                Write-Warning "無法取得更新鎖（已重試 60 秒，其他更新程序可能正在進行）；本次不提交更新並保留備份目錄與移交標記，該交易將由持有鎖之更新程序處理。"
            } else {
                try {
                    Write-Host "新版服務進程已確認健康就緒，標記更新提交並清理備份目錄..."
                    $committedMarker = Join-Path $backupDir ".committed"
                    $commitSuccess = $false
                    try {
                        Set-Content -LiteralPath $committedMarker -Value "committed" -Force
                        $commitSuccess = (Test-Path -LiteralPath $committedMarker)
                    } catch {
                        $commitSuccess = $false
                    }
                    if ($commitSuccess) {
                        $handoffMarker = Join-Path $backupDir ".handing_off"
                        if (Test-Path -LiteralPath $handoffMarker) {
                            Remove-Item -LiteralPath $handoffMarker -Force -ErrorAction SilentlyContinue
                        }
                        if (Test-Path -LiteralPath $backupDir) {
                            try {
                                Remove-Item -LiteralPath $backupDir -Recurse -Force -ErrorAction Stop
                            } catch {
                                Write-Warning "清理備份目錄失敗: $($_.Exception.Message)，嘗試改名隔離..."
                            }
                        }
                        if (Test-Path -LiteralPath $backupDir) {
                            $quarantineName = ".backup-quarantined-" + [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
                            $quarantineDir = Join-Path $InstallDir $quarantineName
                            try {
                                Move-Item -LiteralPath $backupDir -Destination $quarantineDir -Force -ErrorAction Stop
                                Write-Host "已將未清理之備份目錄隔離至: $quarantineName"
                            } catch {
                                Write-Warning "備份目錄改名隔離失敗: $($_.Exception.Message)"
                            }
                        }
                        if (Test-Path -LiteralPath $backupDir) {
                            if ($Process -and -not $Process.HasExited) {
                                Stop-Process -Id $Process.Id -Force -ErrorAction SilentlyContinue
                                try { $null = $Process.WaitForExit(5000) } catch {}
                            }
                            Exit-WithError -Message "新版服務進程已就緒但備份目錄無法清理或隔離 ($backupDir)，將阻擋後續原地更新；已終止進程進入可診斷之失敗狀態。"
                        }
                    } else {
                        if ($Process -and -not $Process.HasExited) {
                            Stop-Process -Id $Process.Id -Force -ErrorAction SilentlyContinue
                            try { $null = $Process.WaitForExit(5000) } catch {}
                        }
                        Exit-WithError -Message "標記更新提交失敗；無法安全提交更新，已終止服務進程並保留備份以供手動救援。"
                    }
                } finally {
                    $commitLock.Dispose()
                }
            }
        } elseif ($Process.HasExited -and ($Process.ExitCode -eq 75)) {
            # 退出碼 75 表示看板已完成就地更新並要求 runner 重啟：不得視為啟動失敗而回滾，
            # 交由下方的退出碼 75 分支等待新版執行檔就緒後重新啟動
            Write-Host "看板已回報就地更新完成（退出碼 75）；保留更新備份並等待新版執行檔就緒。"
        } else {
            Write-Warning "新版服務進程啟動後異常或未能及時就緒，執行自備份自動回滾至先前版本..."
            if ($Process -and -not $Process.HasExited) {
                Stop-Process -Id $Process.Id -Force -ErrorAction SilentlyContinue
                try { $null = $Process.WaitForExit(5000) } catch {}
            }

            # 回滾期間獨占更新鎖，避免其他更新程序同時更動同一份備份交易
            $rollbackLock = Enter-UpdateLock -LockFile (Join-Path $InstallDir ".update.lock")
            if (-not $rollbackLock) {
                Exit-WithError -Message "新版服務進程啟動失敗且無法取得更新鎖執行回滾（其他更新程序可能正在進行）；已保留備份目錄以供手動修復。"
            }
            try {
                $restored = Restore-ServiceBackup -InstallDir $InstallDir
            } finally {
                $rollbackLock.Dispose()
            }
            if ($restored) {
                Write-Host "已成功自備份回滾至先前版本，重新啟動原版服務..."
                continue
            } else {
                Exit-WithError -Message "新版服務進程啟動失敗且回滾失敗，已保留備份目錄以供手動修復。"
            }
        }
    }

    $restartForLogRotation = $false
    try {
        while (-not $Process.WaitForExit(1000)) {
            if (Test-Path -LiteralPath $restartPendingFile) {
                Write-Host "偵測到更新程序已啟動並設定重啟協商標記，正在協調服務進程優雅停機..."
                if (-not (Stop-ServiceProcessGracefully -Process $Process -InstallDir $InstallDir)) {
                    Exit-WithError -Message "等待看板進程完成優雅停機逾時（30 秒）；為避免中斷進行中的資料庫寫入，已保留更新備份並保持停止狀態，請確認進程狀態後重試。"
                }
                break
            }

            $outItem = Get-Item -LiteralPath $OutLog -ErrorAction SilentlyContinue
            $errItem = Get-Item -LiteralPath $ErrLog -ErrorAction SilentlyContinue
            if (($outItem -and $outItem.Length -ge $MaxActiveLogBytes) -or ($errItem -and $errItem.Length -ge $MaxActiveLogBytes)) {
                Write-Warning "Active log size exceeded ${MaxActiveLogBytes} bytes. Restarting service to rotate logs..."
                $restartForLogRotation = $true
                if (-not (Stop-ServiceProcessGracefully -Process $Process -InstallDir $InstallDir)) {
                    Exit-WithError -Message "等待看板進程完成優雅停機逾時（30 秒）；為避免中斷進行中的資料庫寫入，日誌輪轉將於下次啟動時重試。"
                }
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
