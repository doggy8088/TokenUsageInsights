[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [string]$InstallDir = $(
        if ($env:LOCALAPPDATA) { Join-Path $env:LOCALAPPDATA "TokenUsageInsights" }
        else { Join-Path $HOME "AppData\Local\TokenUsageInsights" }
    ),
    [string]$BinDir = $(Join-Path $HOME "bin"),
    [int]$Port = 3003
)

$ErrorActionPreference = "Stop"
$AppName = "token-usage-insights"
$InstallDir = [IO.Path]::GetFullPath([Environment]::ExpandEnvironmentVariables($InstallDir))
$BinDir = [IO.Path]::GetFullPath([Environment]::ExpandEnvironmentVariables($BinDir))

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

if ($PSCmdlet.ShouldProcess($InstallDir, "Install Token Usage Insights")) {
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
    $BatchInstallDir = $InstallDir.Replace("%", "%%")
    @"
@echo off
setlocal
set "PORT=$Port"
pushd "$BatchInstallDir"
"$BatchInstallDir\$AppName.exe" %*
set "APP_EXIT_CODE=%ERRORLEVEL%"
popd
exit /b %APP_EXIT_CODE%
"@ | Set-Content -Encoding ASCII $Shim

    & (Join-Path $InstallDir "$AppName.exe") --backfill-copilot-usage
    if ($LASTEXITCODE -ne 0) {
        throw "Copilot usage backfill failed with exit code $LASTEXITCODE"
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
Write-Host "Run:"
Write-Host "  $(Join-Path $BinDir "$AppName.cmd")"
