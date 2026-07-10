<#
.SYNOPSIS
  Build Token 戰情室 release executables on Windows.

.DESCRIPTION
  Wraps `cargo build --release` for both the dashboard server binary
  (token-usage-insights.exe) and the CLI import/export tool
  (token-usage-insights-cli.exe). Requires the Rust MSVC toolchain and the
  Visual Studio Build Tools C++ workload (see README.md).

  By default this treats any compiler warning as a build failure, matching
  the "no warnings, no errors" policy in AGENTS.md. Use -AllowWarnings to
  build anyway while iterating locally.

.PARAMETER Target
  Optional Rust target triple to cross/native compile for, e.g.
  x86_64-pc-windows-msvc. Defaults to the host's default toolchain target.

.PARAMETER SkipTests
  Skip `cargo test --release` before building. Tests run by default.

.PARAMETER AllowWarnings
  Do not fail the build when the compiler emits warnings.

.EXAMPLE
  .\scripts\build.ps1

.EXAMPLE
  .\scripts\build.ps1 -Target x86_64-pc-windows-msvc -SkipTests
#>
[CmdletBinding()]
param(
    [string]$Target,
    [switch]$SkipTests,
    [switch]$AllowWarnings
)

$ErrorActionPreference = "Stop"

function Invoke-Cargo {
    param(
        [string[]]$CargoArgs,
        [string]$StepName
    )

    Write-Host "==> $StepName"
    $stdoutFile = [IO.Path]::GetTempFileName()
    $stderrFile = [IO.Path]::GetTempFileName()
    try {
        # Use Start-Process with OS-level file redirection instead of
        # PowerShell's native `2>`/`2>&1` stream merging: PowerShell (both
        # Windows PowerShell 5.1 and PowerShell 7.3+'s
        # $PSNativeCommandUseErrorActionPreference) wraps merged native
        # stderr text into ErrorRecord objects, which prints a spurious
        # "NativeCommandError" banner and can abort the script even when the
        # external tool exits 0. Start-Process redirection bypasses that.
        $argumentList = $CargoArgs -join " "
        $process = Start-Process -FilePath "cargo" -ArgumentList $argumentList `
            -NoNewWindow -Wait -PassThru `
            -RedirectStandardOutput $stdoutFile -RedirectStandardError $stderrFile
        $exitCode = $process.ExitCode
        $stdout = Get-Content -LiteralPath $stdoutFile
        $stderr = Get-Content -LiteralPath $stderrFile
    } finally {
        Remove-Item -LiteralPath $stdoutFile, $stderrFile -ErrorAction SilentlyContinue
    }
    $output = @($stdout) + @($stderr)
    $output | ForEach-Object { Write-Host $_ }

    if ($exitCode -ne 0) {
        throw "$StepName failed (exit code $exitCode)."
    }

    if (-not $AllowWarnings) {
        $warnings = $output | Where-Object { $_ -match "^warning:" }
        if ($warnings) {
            $warnings | ForEach-Object { Write-Warning $_ }
            throw "$StepName produced $($warnings.Count) compiler warning(s). Fix them or re-run with -AllowWarnings."
        }
    }
}

$targetArgs = @()
if ($Target) {
    $targetArgs = @("--target", $Target)
}

if (-not $SkipTests) {
    Invoke-Cargo -CargoArgs (@("test", "--release", "--locked") + $targetArgs) -StepName "cargo test --release" | Out-Null
}

Invoke-Cargo -CargoArgs (@("build", "--release", "--locked", "--all-targets") + $targetArgs) -StepName "cargo build --release --all-targets" | Out-Null

$outDir = if ($Target) { "target\$Target\release" } else { "target\release" }
Write-Host ""
Write-Host "Build succeeded with no warnings or errors."
Write-Host "Executables:"
Write-Host "  $outDir\token-usage-insights.exe"
Write-Host "  $outDir\token-usage-insights-cli.exe"
