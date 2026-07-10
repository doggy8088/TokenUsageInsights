[CmdletBinding()]
param()

& (Join-Path $PSScriptRoot "..\statusline-token.ps1") -Assistant antigravity
