[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("antigravity", "copilot")]
    [string]$Assistant
)

$ErrorActionPreference = "Stop"
$Utf8NoBom = New-Object System.Text.UTF8Encoding($false)

function Get-NestedValue {
    param($Object, [string]$Path)

    $current = $Object
    foreach ($part in $Path.Split(".")) {
        if ($null -eq $current) { return $null }
        $property = $current.PSObject.Properties[$part]
        if ($null -eq $property) { return $null }
        $current = $property.Value
    }
    return $current
}

function Get-FirstValue {
    param($Object, [string[]]$Paths, $Default = $null)

    foreach ($path in $Paths) {
        $value = Get-NestedValue $Object $path
        if ($null -ne $value -and [string]$value -ne "") { return $value }
    }
    return $Default
}

function Get-TextValue {
    param($Object, [string[]]$Paths, [string]$Default = "")
    return [string](Get-FirstValue $Object $Paths $Default)
}

function Convert-ToUInt64 {
    param($Value, [UInt64]$Default = 0)

    [UInt64]$parsed = 0
    if ($null -ne $Value -and [UInt64]::TryParse([string]$Value, [ref]$parsed)) {
        return $parsed
    }
    return $Default
}

function Get-UInt64Value {
    param($Object, [string[]]$Paths, [UInt64]$Default = 0)
    return Convert-ToUInt64 (Get-FirstValue $Object $Paths $null) $Default
}

function Get-NumberValue {
    param($Object, [string[]]$Paths)

    $value = Get-FirstValue $Object $Paths 0
    [double]$parsed = 0
    if ([double]::TryParse(
        [string]$value,
        [Globalization.NumberStyles]::Float,
        [Globalization.CultureInfo]::InvariantCulture,
        [ref]$parsed
    )) {
        return $parsed
    }
    return [double]0
}

function Get-Delta {
    param([UInt64]$Current, [UInt64]$Previous)
    if ($Current -ge $Previous) { return [UInt64]($Current - $Previous) }
    return [UInt64]0
}

function Format-TokenCount {
    param([UInt64]$Value)

    if ($Value -ge 999500) {
        $scaled = [double]$Value / 1000000
        $format = if ($scaled -ge 10) { "0.0m" } else { "0.00m" }
        return $scaled.ToString($format, [Globalization.CultureInfo]::InvariantCulture)
    }
    if ($Value -ge 1000) {
        $scaled = [double]$Value / 1000
        $format = if ($scaled -ge 10) { "0.0k" } else { "0.00k" }
        return $scaled.ToString($format, [Globalization.CultureInfo]::InvariantCulture)
    }
    return [string]$Value
}

function Resolve-DataDirectory {
    param([string]$EnvironmentName, [string[]]$DefaultSegments)

    $configured = [Environment]::GetEnvironmentVariable($EnvironmentName)
    if (![string]::IsNullOrWhiteSpace($configured)) {
        $expanded = [Environment]::ExpandEnvironmentVariables($configured)
        if ($expanded -eq "~") { return $script:UserHome }
        if ($expanded.StartsWith("~\") -or $expanded.StartsWith("~/")) {
            return Join-Path $script:UserHome $expanded.Substring(2)
        }
        return $expanded
    }

    $path = $script:UserHome
    foreach ($segment in $DefaultSegments) { $path = Join-Path $path $segment }
    return $path
}

$UserHome = [Environment]::GetFolderPath([Environment+SpecialFolder]::UserProfile)
if ([string]::IsNullOrWhiteSpace($UserHome)) { $UserHome = $HOME }
if ([string]::IsNullOrWhiteSpace($UserHome)) { $UserHome = $env:USERPROFILE }
if ([string]::IsNullOrWhiteSpace($UserHome)) {
    throw "Cannot resolve the current Windows user profile directory."
}

if ($Assistant -eq "antigravity") {
    $dataDir = Resolve-DataDirectory "ANTIGRAVITY_DIR" @(".gemini", "antigravity-cli")
} else {
    $dataDir = Resolve-DataDirectory "COPILOT_DIR" @(".copilot")
}
$usageDir = Join-Path $dataDir "usage"
$stateFile = Join-Path $dataDir "statusline-state.json"
$jsonlFile = Join-Path $usageDir ("usage-{0}.jsonl" -f (Get-Date -Format "yyyy-MM-dd"))
New-Item -ItemType Directory -Force -Path $dataDir, $usageDir | Out-Null

$inputText = [Console]::In.ReadToEnd()
try {
    $payload = $inputText | ConvertFrom-Json
} catch {
    [Console]::Error.WriteLine("Invalid status-line JSON input: {0}", $_.Exception.Message)
    exit 1
}

$sessionPaths = if ($Assistant -eq "antigravity") {
    @("conversation_id", "session_id")
} else {
    @("session_id")
}
$sessionId = Get-TextValue $payload $sessionPaths
if ([string]::IsNullOrWhiteSpace($sessionId)) {
    $sessionId = "{0}-{1}" -f (Get-Date -Format "yyyyMMdd-HHmmss"), [guid]::NewGuid()
}

$sessionName = Get-TextValue $payload @("session_name")
if ($Assistant -eq "antigravity" -and [string]::IsNullOrWhiteSpace($sessionName)) {
    $sessionName = $sessionId.Substring(0, [Math]::Min(8, $sessionId.Length))
}
$transcriptPath = Get-TextValue $payload @("transcript_path")
if ($Assistant -eq "antigravity") {
    $transcriptPath = $transcriptPath.Replace("/.gemini/antigravity/", "/.gemini/antigravity-cli/")
    $transcriptPath = $transcriptPath.Replace("\.gemini\antigravity\", "\.gemini\antigravity-cli\")
}
$cwd = Get-TextValue $payload @("cwd", "workspace.current_dir")
$version = Get-TextValue $payload @("version")
$model = Get-TextValue $payload @("model.display_name", "model.id", "modelName", "current_model") "unknown"
$modelId = Get-TextValue $payload @("model.id", "modelName", "current_model") "unknown"

$inputTokens = Get-UInt64Value $payload @("context_window.total_input_tokens")
$outputTokens = Get-UInt64Value $payload @("context_window.total_output_tokens")
$cacheReadPaths = @("context_window.total_cache_read_tokens")
$cacheWritePaths = @("context_window.total_cache_write_tokens")
if ($Assistant -eq "antigravity") {
    $cacheReadPaths += "context_window.current_usage.cache_read_input_tokens"
    $cacheWritePaths += "context_window.current_usage.cache_creation_input_tokens"
}
$cacheReadTokens = Get-UInt64Value $payload $cacheReadPaths
$cacheWriteTokens = Get-UInt64Value $payload $cacheWritePaths
$reasoningTokens = Get-UInt64Value $payload @("context_window.total_reasoning_tokens")
$providedTotal = Get-FirstValue $payload @("context_window.total_tokens") $null
if ($null -eq $providedTotal) {
    $totalTokens = $inputTokens + $outputTokens + $cacheReadTokens + $cacheWriteTokens + $reasoningTokens
} else {
    $totalTokens = Convert-ToUInt64 $providedTotal
}

$lastCallInputTokens = Get-UInt64Value $payload @("context_window.last_call_input_tokens")
$lastCallOutputTokens = Get-UInt64Value $payload @("context_window.last_call_output_tokens")
if ($Assistant -eq "antigravity") {
    $currentContextTokens = Get-UInt64Value $payload @("context_window.current_usage.input_tokens")
    $displayedContextLimit = Get-UInt64Value $payload @("context_window.context_window_size")
    $contextPercentage = Get-TextValue $payload @(
        "context_window.current_context_used_percentage",
        "context_window.used_percentage"
    )
} else {
    $currentContextTokens = Get-UInt64Value $payload @("context_window.current_context_tokens")
    $displayedContextLimit = Get-UInt64Value $payload @("context_window.displayed_context_limit")
    $contextPercentage = Get-TextValue $payload @("context_window.current_context_used_percentage")
}

$totalApiDurationMs = Get-NumberValue $payload @("cost.total_api_duration_ms")
$totalDurationMs = Get-NumberValue $payload @("cost.total_duration_ms")
$totalPremiumRequests = Get-NumberValue $payload @("cost.total_premium_requests")
$lineAddedPaths = @("cost.total_lines_added")
$lineRemovedPaths = @("cost.total_lines_removed")
if ($Assistant -eq "antigravity") {
    $lineAddedPaths += "metrics.files.total_lines_added"
    $lineRemovedPaths += "metrics.files.total_lines_removed"
}
$totalLinesAdded = Get-NumberValue $payload $lineAddedPaths
$totalLinesRemoved = Get-NumberValue $payload $lineRemovedPaths

$previous = $null
if (Test-Path -LiteralPath $stateFile) {
    try { $previous = Get-Content -Raw -LiteralPath $stateFile | ConvertFrom-Json }
    catch { $previous = $null }
}
$previousSessionId = Get-TextValue $previous @("session_id")
$previousModel = Get-TextValue $previous @("model")
$previousTurnNo = Get-UInt64Value $previous @("turn_no")
$previousInputTokens = Get-UInt64Value $previous @("input_tokens")
$previousOutputTokens = Get-UInt64Value $previous @("output_tokens")
$previousCacheReadTokens = Get-UInt64Value $previous @("cache_read_tokens")
$previousCacheWriteTokens = Get-UInt64Value $previous @("cache_write_tokens")
$previousReasoningTokens = Get-UInt64Value $previous @("reasoning_tokens")
$previousTotalTokens = Get-UInt64Value $previous @("total_tokens")

if ($previousSessionId -ne $sessionId) {
    $previousModel = ""
    $previousTurnNo = 0
    $previousInputTokens = 0
    $previousOutputTokens = 0
    $previousCacheReadTokens = 0
    $previousCacheWriteTokens = 0
    $previousReasoningTokens = 0
    $previousTotalTokens = 0
}

$deltaInput = Get-Delta $inputTokens $previousInputTokens
$deltaOutput = Get-Delta $outputTokens $previousOutputTokens
$deltaCacheRead = Get-Delta $cacheReadTokens $previousCacheReadTokens
$deltaCacheWrite = Get-Delta $cacheWriteTokens $previousCacheWriteTokens
$deltaReasoning = Get-Delta $reasoningTokens $previousReasoningTokens
$deltaTotal = Get-Delta $totalTokens $previousTotalTokens
if ($Assistant -eq "antigravity") {
    if ($lastCallInputTokens -eq 0 -and $deltaInput -gt 0) { $lastCallInputTokens = $deltaInput }
    if ($lastCallOutputTokens -eq 0 -and $deltaOutput -gt 0) { $lastCallOutputTokens = $deltaOutput }
}

$modelChanged = ![string]::IsNullOrWhiteSpace($previousModel) -and $previousModel -ne $model
$turnNo = $previousTurnNo
if ($deltaTotal -gt 0) {
    $turnNo = $previousTurnNo + 1
    $entry = [ordered]@{
        timestamp = (Get-Date -Format "o")
        session_id = $sessionId
        session_name = $sessionName
        transcript_path = $transcriptPath
        cwd = $cwd
        version = $version
        turn_no = $turnNo
        model = $model
        model_id = $modelId
        previous_model = $previousModel
        model_changed = $modelChanged
        tokens = [ordered]@{
            input = $inputTokens
            output = $outputTokens
            cache_read = $cacheReadTokens
            cache_write = $cacheWriteTokens
            reasoning = $reasoningTokens
            total = $totalTokens
            last_call_input = $lastCallInputTokens
            last_call_output = $lastCallOutputTokens
        }
        delta_tokens = [ordered]@{
            input = $deltaInput
            output = $deltaOutput
            cache_read = $deltaCacheRead
            cache_write = $deltaCacheWrite
            reasoning = $deltaReasoning
            total = $deltaTotal
        }
        context = [ordered]@{
            current_context_tokens = $currentContextTokens
            displayed_context_limit = $displayedContextLimit
            current_context_used_percentage = $contextPercentage
        }
        cost = [ordered]@{
            total_api_duration_ms = $totalApiDurationMs
            total_duration_ms = $totalDurationMs
            total_premium_requests = $totalPremiumRequests
            total_lines_added = $totalLinesAdded
            total_lines_removed = $totalLinesRemoved
        }
    }
    $entryJson = $entry | ConvertTo-Json -Depth 8 -Compress
    [IO.File]::AppendAllText($jsonlFile, $entryJson + [Environment]::NewLine, $Utf8NoBom)
}

$state = [ordered]@{
    session_id = $sessionId
    session_name = $sessionName
    transcript_path = $transcriptPath
    model = $model
    model_id = $modelId
    turn_no = $turnNo
    input_tokens = $inputTokens
    output_tokens = $outputTokens
    cache_read_tokens = $cacheReadTokens
    cache_write_tokens = $cacheWriteTokens
    reasoning_tokens = $reasoningTokens
    total_tokens = $totalTokens
}
[IO.File]::WriteAllText($stateFile, ($state | ConvertTo-Json -Depth 4 -Compress), $Utf8NoBom)

$line = "{0} | #{1} | in {2} | cache {3}/{4} | out {5} | reasoning {6} | total {7} | +{8} | last {9}/{10}/{11}" -f @(
    $model,
    $turnNo,
    (Format-TokenCount $inputTokens),
    (Format-TokenCount $cacheReadTokens),
    (Format-TokenCount $cacheWriteTokens),
    (Format-TokenCount $outputTokens),
    (Format-TokenCount $reasoningTokens),
    (Format-TokenCount $totalTokens),
    (Format-TokenCount $deltaTotal),
    (Format-TokenCount $lastCallInputTokens),
    (Format-TokenCount $deltaCacheRead),
    (Format-TokenCount $lastCallOutputTokens)
)
if ($modelChanged) { $line += " | from $previousModel" }
elseif (![string]::IsNullOrWhiteSpace($contextPercentage)) { $line += " | ctx $contextPercentage%" }
Write-Output $line
