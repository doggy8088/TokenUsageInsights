<#
.SYNOPSIS
  One-line bootstrap installer for Token 戰情室 (Windows).

.DESCRIPTION
  Downloads the correct prebuilt release archive from GitHub Releases (no
  Rust/Cargo toolchain required), extracts it, and runs the packaged
  install.ps1. Safe to re-run to upgrade to a newer release.

.EXAMPLE
  irm https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.ps1 | iex

.EXAMPLE
  $script = irm https://raw.githubusercontent.com/doggy8088/TokenUsageInsights/main/scripts/get.ps1
  Invoke-Expression "& { $script } -InstallDir 'D:\Apps\Token Usage Insights' -Port 3010"
#>
[CmdletBinding()]
param(
    [string]$Version = "latest",
    [string]$InstallDir,
    [string]$BinDir,
    [int]$Port = 3003
)

$ErrorActionPreference = "Stop"
$Repo = "doggy8088/TokenUsageInsights"
$AppName = "token-usage-insights"
$Target = "x86_64-pc-windows-msvc"

if ($Version -eq "latest") {
    Write-Host "Resolving latest release tag for $Repo ..."
    $Release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -UseBasicParsing
    $Tag = $Release.tag_name
    if (-not $Tag) {
        throw "Failed to resolve the latest release tag."
    }
} else {
    $Tag = $Version
}

$Archive = "$AppName-$Tag-$Target.zip"
$Url = "https://github.com/$Repo/releases/download/$Tag/$Archive"

$WorkDir = Join-Path ([IO.Path]::GetTempPath()) ([IO.Path]::GetRandomFileName())
New-Item -ItemType Directory -Force -Path $WorkDir | Out-Null

try {
    $ArchivePath = Join-Path $WorkDir $Archive
    Write-Host "Downloading $Url ..."
    try {
        Invoke-WebRequest -Uri $Url -OutFile $ArchivePath -UseBasicParsing
    } catch {
        throw "Download failed. Check that $Tag publishes a $Target archive: https://github.com/$Repo/releases/tag/$Tag"
    }

    Write-Host "Extracting ..."
    Expand-Archive -Path $ArchivePath -DestinationPath $WorkDir -Force

    $ExtractedDir = Join-Path $WorkDir "$AppName-$Tag-$Target"
    if (!(Test-Path $ExtractedDir)) {
        $ExtractedDir = (Get-ChildItem -Path $WorkDir -Directory | Select-Object -First 1).FullName
    }

    $InstallScript = Join-Path $ExtractedDir "install.ps1"
    if (!(Test-Path $InstallScript)) {
        throw "install.ps1 not found in extracted release: $ExtractedDir"
    }

    $InstallArgs = @{ Port = $Port }
    if ($InstallDir) { $InstallArgs["InstallDir"] = $InstallDir }
    if ($BinDir) { $InstallArgs["BinDir"] = $BinDir }

    Write-Host "Installing $Tag ..."
    & $InstallScript @InstallArgs
} finally {
    Remove-Item -Recurse -Force $WorkDir -ErrorAction SilentlyContinue
}
