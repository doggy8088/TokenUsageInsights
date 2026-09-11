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

if (Test-Path $OutLog) {
    $outItem = Get-Item $OutLog -ErrorAction SilentlyContinue
    if ($outItem -and $outItem.Length -gt 0) {
        Add-Content -LiteralPath (Join-Path $LogDir "$AppName.history.out.log") -Value (Get-Content -LiteralPath $OutLog) -ErrorAction SilentlyContinue
        Move-Item -LiteralPath $OutLog -Destination (Join-Path $LogDir "$AppName.prev.out.log") -Force -ErrorAction SilentlyContinue
    }
}
if (Test-Path $ErrLog) {
    $errItem = Get-Item $ErrLog -ErrorAction SilentlyContinue
    if ($errItem -and $errItem.Length -gt 0) {
        Add-Content -LiteralPath (Join-Path $LogDir "$AppName.history.err.log") -Value (Get-Content -LiteralPath $ErrLog) -ErrorAction SilentlyContinue
        Move-Item -LiteralPath $ErrLog -Destination (Join-Path $LogDir "$AppName.prev.err.log") -Force -ErrorAction SilentlyContinue
    }
}

$Process = Start-Process -FilePath $Exe `
    -WorkingDirectory $InstallDir `
    -WindowStyle Hidden `
    -RedirectStandardOutput $OutLog `
    -RedirectStandardError $ErrLog `
    -PassThru

try {
    while (-not $Process.WaitForExit(500)) {
        # Active wait until process exits
    }
    exit $Process.ExitCode
} finally {
    if ($Process -and -not $Process.HasExited) {
        Stop-Process -Id $Process.Id -Force -ErrorAction SilentlyContinue
    }
}
