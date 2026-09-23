<#
.SYNOPSIS
  驗證 scripts/get.ps1 以 README 記載的 irm 一行安裝方式取得時可以被正確剖析。

.DESCRIPTION
  README 的 Windows 安裝方式是先以 irm（Invoke-RestMethod）把 get.ps1 取回成字串，
  再交給 iex、[scriptblock]::Create 或 Invoke-Expression "& { $script } ..." 執行。
  irm 解碼 HTTP 回應時不會剝除 UTF-8 BOM，檔案開頭若有 BOM，字串第一個字元就是
  U+FEFF；PowerShell 剖析器不把它當成空白，`<#` 因此不再被視為區塊註解開頭，註解
  內容被當成程式碼剖析，產生 "Missing closing ')'" 等 ParserError（Issue #62）。

  Parser::ParseFile 讀檔時會自動剝除 BOM，所以只對檔案做 AST 檢查無法發現這個問題；
  本測試改以 irm 的方式把位元組解碼成字串後再剖析。

  另外要求 get.ps1 只包含 ASCII：不帶 BOM 的檔案在 Windows PowerShell 5.1 以
  `.\get.ps1` 直接執行時會以系統 ANSI 字碼頁解讀，只有純 ASCII 能讓 irm 與檔案兩種
  執行方式得到相同的剖析結果。

.EXAMPLE
  pwsh -NoProfile -File tests/get-ps1-bootstrap.test.ps1
#>
[CmdletBinding()]
param(
    [string]$Path = (Join-Path (Split-Path -Parent $PSScriptRoot) "scripts/get.ps1")
)

$ErrorActionPreference = "Stop"
$Failures = 0

function Pass([string]$Message) {
    Write-Host "ok - $Message"
}

function Fail([string]$Message) {
    [Console]::Error.WriteLine("FAIL - $Message")
    $script:Failures++
}

function Test-ParsesCleanly([string]$Source, [string]$Description) {
    $errors = $null
    [void][System.Management.Automation.Language.Parser]::ParseInput($Source, [ref]$null, [ref]$errors)
    if ($errors.Count -eq 0) {
        Pass "$Description 剖析沒有錯誤"
        return
    }

    $first = $errors[0]
    Fail ("{0} 剖析出現 {1} 個錯誤，第一個在第 {2} 行：{3}" -f $Description, $errors.Count, $first.Extent.StartLineNumber, $first.Message)
}

$bytes = [IO.File]::ReadAllBytes($Path)

if ($bytes.Length -ge 3 -and $bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF) {
    Fail "get.ps1 開頭含有 UTF-8 BOM（EF BB BF），irm | iex 會因此產生 ParserError；請改存為不含 BOM 的 UTF-8"
} else {
    Pass "get.ps1 不含 UTF-8 BOM"
}

$nonAsciiIndex = [Array]::FindIndex($bytes, [Predicate[byte]] { param($b) $b -ge 0x80 })
if ($nonAsciiIndex -ge 0) {
    $line = 1 + @($bytes[0..$nonAsciiIndex] | Where-Object { $_ -eq 0x0A }).Count
    Fail "get.ps1 第 $line 行含有非 ASCII 字元；不帶 BOM 時 Windows PowerShell 5.1 會以 ANSI 字碼頁讀取，請只使用 ASCII"
} else {
    Pass "get.ps1 只包含 ASCII 字元"
}

# irm 依 Content-Type 的 charset（raw.githubusercontent.com 回應 utf-8）解碼本文；
# Encoding.GetString 與 irm 一樣不會剝除 BOM，因此可以重現實際取得的字串。
$content = [Text.Encoding]::UTF8.GetString($bytes)

Test-ParsesCleanly $content "irm | iex"
Test-ParsesCleanly "& { $content } -InstallDir 'D:\Apps\Token Usage Insights' -Port 3010" 'Invoke-Expression "& { $script } ..."'

try {
    [void][scriptblock]::Create($content)
    Pass "[scriptblock]::Create((irm ...)) 可以建立指令碼區塊"
} catch {
    Fail "[scriptblock]::Create((irm ...)) 失敗：$($_.Exception.Message)"
}

Write-Host ""
if ($Failures -ne 0) {
    [Console]::Error.WriteLine("$Failures 項檢查未通過")
    exit 1
}

Write-Host "get.ps1 bootstrap 測試全數通過"
