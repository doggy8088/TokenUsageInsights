param(
    [string]$InstallDir = "$env:LOCALAPPDATA\TokenUsageInsights",
    [string]$BinDir = "$HOME\bin",
    [int]$Port = 3003
)

$ErrorActionPreference = "Stop"
$AppName = "token-usage-insights"

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

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
New-Item -ItemType Directory -Force -Path $BinDir | Out-Null

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
@"
@echo off
set PORT=$Port
"$InstallDir\$AppName.exe" %*
"@ | Set-Content -Encoding ASCII $Shim

Write-Host "Token 戰情室 installed."
Write-Host ""
Write-Host "Install directory:"
Write-Host "  $InstallDir"
Write-Host ""
Write-Host "Executable shim:"
Write-Host "  $Shim"
Write-Host ""
Write-Host "Run:"
Write-Host "  $Shim"

