#!/usr/bin/env bash

# ==============================================================================
# ⚠️ 系統核心邏輯 (System Core Logic) - 請勿修改此區塊！
# ------------------------------------------------------------------------------
# 此指令稿由 TokenUsageInsights 系統自動調用，用於記錄 Token 消耗歷史與狀態。
# ❌ 請勿修改此區塊以上的程式碼（包括 input 讀取、變數解析、JSONL 寫入與狀態更新）。
# 變更這些邏輯會導致後台資料庫無法同步或資料損毀！
#
# 【自訂狀態欄顯示】：
# 若要修改 Tmux / 狀態列顯示格式，請至此檔案最下方的「可自訂區域 (Customizable Area)」。
# ==============================================================================

input="$(cat)"

antigravity_dir="$HOME/.gemini/antigravity-cli"
usage_dir="$antigravity_dir/usage"

state_file="$antigravity_dir/statusline-state.json"
jsonl_file="$usage_dir/usage-$(date +%Y-%m-%d).jsonl"

mkdir -p "$antigravity_dir" "$usage_dir"

now="$(date -Iseconds)"

fmt_tokens() {
  local n="$1"

  if [ -z "$n" ] || ! [[ "$n" =~ ^[0-9]+$ ]]; then
    echo "0"
    return
  fi

  if [ "$n" -ge 999500 ]; then
    awk -v n="$n" 'BEGIN {
      v = n / 1000000
      if (v >= 10) printf "%.1fm", v
      else printf "%.2fm", v
    }'
  elif [ "$n" -ge 1000 ]; then
    awk -v n="$n" 'BEGIN {
      v = n / 1000
      if (v >= 10) printf "%.1fk", v
      else printf "%.2fk", v
    }'
  else
    echo "$n"
  fi
}

# Antigravity session properties
payload_session_id=$(printf '%s' "$input" | jq -r '.conversation_id // .session_id // empty')
session_name=$(printf '%s' "$input" | jq -r '.session_name // empty')
transcript_path=$(printf '%s' "$input" | jq -r '.transcript_path // empty' | sed 's/\/\.gemini\/antigravity\//\/\.gemini\/antigravity-cli\//g')
cwd=$(printf '%s' "$input" | jq -r '.cwd // .workspace.current_dir // empty')
version=$(printf '%s' "$input" | jq -r '.version // empty')

if [ -z "$payload_session_id" ]; then
  payload_session_id="$(date +%Y%m%d-%H%M%S)-$(uuidgen 2>/dev/null || cat /proc/sys/kernel/random/uuid)"
fi

session_id="$payload_session_id"

if [ -z "$session_name" ]; then
  session_name="${session_id:0:8}"
fi

model=$(printf '%s' "$input" | jq -r '
  .model.display_name //
  .model.id //
  .modelName //
  .current_model //
  "unknown"
')

model_id=$(printf '%s' "$input" | jq -r '
  .model.id //
  .modelName //
  .current_model //
  "unknown"
')

input_tokens=$(printf '%s' "$input" | jq -r '.context_window.total_input_tokens // 0')
output_tokens=$(printf '%s' "$input" | jq -r '.context_window.total_output_tokens // 0')
cache_read_tokens=$(printf '%s' "$input" | jq -r '
  .context_window.total_cache_read_tokens //
  .context_window.current_usage.cache_read_input_tokens //
  0
')
cache_write_tokens=$(printf '%s' "$input" | jq -r '
  .context_window.total_cache_write_tokens //
  .context_window.current_usage.cache_creation_input_tokens //
  0
')
reasoning_tokens=$(printf '%s' "$input" | jq -r '.context_window.total_reasoning_tokens // 0')

total_tokens=$(printf '%s' "$input" | jq -r '.context_window.total_tokens // empty')
if [ -z "$total_tokens" ] || ! [[ "$total_tokens" =~ ^[0-9]+$ ]]; then
  total_tokens=$((input_tokens + output_tokens + cache_read_tokens + cache_write_tokens + reasoning_tokens))
fi

# Calculate deltas from state
last_call_input_tokens=$(printf '%s' "$input" | jq -r '.context_window.last_call_input_tokens // 0')
last_call_output_tokens=$(printf '%s' "$input" | jq -r '.context_window.last_call_output_tokens // 0')
current_context_tokens=$(printf '%s' "$input" | jq -r '.context_window.current_usage.input_tokens // 0')
displayed_context_limit=$(printf '%s' "$input" | jq -r '.context_window.context_window_size // 0')
current_context_used_percentage=$(printf '%s' "$input" | jq -r '
  .context_window.current_context_used_percentage //
  .context_window.used_percentage //
  empty
')

total_api_duration_ms=$(printf '%s' "$input" | jq -r '.cost.total_api_duration_ms // 0')
total_duration_ms=$(printf '%s' "$input" | jq -r '.cost.total_duration_ms // 0')
total_premium_requests=$(printf '%s' "$input" | jq -r '.cost.total_premium_requests // 0')
total_lines_added=$(printf '%s' "$input" | jq -r '.cost.total_lines_added // .metrics.files.total_lines_added // 0')
total_lines_removed=$(printf '%s' "$input" | jq -r '.cost.total_lines_removed // .metrics.files.total_lines_removed // 0')

previous_session_id=""
previous_model=""
previous_turn_no=0

previous_input_tokens=0
previous_output_tokens=0
previous_cache_read_tokens=0
previous_cache_write_tokens=0
previous_reasoning_tokens=0
previous_total_tokens=0

if [ -f "$state_file" ]; then
  previous_session_id=$(jq -r '.session_id // ""' "$state_file")
  previous_model=$(jq -r '.model // ""' "$state_file")
  previous_turn_no=$(jq -r '.turn_no // 0' "$state_file")

  previous_input_tokens=$(jq -r '.input_tokens // 0' "$state_file")
  previous_output_tokens=$(jq -r '.output_tokens // 0' "$state_file")
  previous_cache_read_tokens=$(jq -r '.cache_read_tokens // 0' "$state_file")
  previous_cache_write_tokens=$(jq -r '.cache_write_tokens // 0' "$state_file")
  previous_reasoning_tokens=$(jq -r '.reasoning_tokens // 0' "$state_file")
  previous_total_tokens=$(jq -r '.total_tokens // 0' "$state_file")
fi

if [ "$previous_session_id" != "$session_id" ]; then
  previous_model=""
  previous_turn_no=0

  previous_input_tokens=0
  previous_output_tokens=0
  previous_cache_read_tokens=0
  previous_cache_write_tokens=0
  previous_reasoning_tokens=0
  previous_total_tokens=0
fi

delta_input=$((input_tokens - previous_input_tokens))
delta_output=$((output_tokens - previous_output_tokens))
delta_cache_read=$((cache_read_tokens - previous_cache_read_tokens))
delta_cache_write=$((cache_write_tokens - previous_cache_write_tokens))
delta_reasoning=$((reasoning_tokens - previous_reasoning_tokens))
delta_total=$((total_tokens - previous_total_tokens))

if [ "$delta_input" -lt 0 ]; then delta_input=0; fi
if [ "$delta_output" -lt 0 ]; then delta_output=0; fi
if [ "$delta_cache_read" -lt 0 ]; then delta_cache_read=0; fi
if [ "$delta_cache_write" -lt 0 ]; then delta_cache_write=0; fi
if [ "$delta_reasoning" -lt 0 ]; then delta_reasoning=0; fi
if [ "$delta_total" -lt 0 ]; then delta_total=0; fi

if [ "$last_call_input_tokens" -eq 0 ] && [ "$delta_input" -gt 0 ]; then
  last_call_input_tokens=$delta_input
fi
if [ "$last_call_output_tokens" -eq 0 ] && [ "$delta_output" -gt 0 ]; then
  last_call_output_tokens=$delta_output
fi

model_changed=false
if [ -n "$previous_model" ] && [ "$previous_model" != "$model" ]; then
  model_changed=true
fi

turn_no="$previous_turn_no"

if [ "$delta_total" -gt 0 ]; then
  turn_no=$((previous_turn_no + 1))

  jq -n \
    --arg timestamp "$now" \
    --arg session_id "$session_id" \
    --arg session_name "$session_name" \
    --arg transcript_path "$transcript_path" \
    --arg cwd "$cwd" \
    --arg version "$version" \
    --arg model "$model" \
    --arg model_id "$model_id" \
    --arg previous_model "$previous_model" \
    --argjson turn_no "$turn_no" \
    --argjson model_changed "$model_changed" \
    --argjson input_tokens "$input_tokens" \
    --argjson output_tokens "$output_tokens" \
    --argjson cache_read_tokens "$cache_read_tokens" \
    --argjson cache_write_tokens "$cache_write_tokens" \
    --argjson reasoning_tokens "$reasoning_tokens" \
    --argjson total_tokens "$total_tokens" \
    --argjson last_call_input_tokens "$last_call_input_tokens" \
    --argjson last_call_output_tokens "$last_call_output_tokens" \
    --argjson delta_input_tokens "$delta_input" \
    --argjson delta_output_tokens "$delta_output" \
    --argjson delta_cache_read_tokens "$delta_cache_read" \
    --argjson delta_cache_write_tokens "$delta_cache_write" \
    --argjson delta_reasoning_tokens "$delta_reasoning" \
    --argjson delta_total_tokens "$delta_total" \
    --argjson current_context_tokens "$current_context_tokens" \
    --argjson displayed_context_limit "$displayed_context_limit" \
    --arg current_context_used_percentage "$current_context_used_percentage" \
    --argjson total_api_duration_ms "$total_api_duration_ms" \
    --argjson total_duration_ms "$total_duration_ms" \
    --argjson total_premium_requests "$total_premium_requests" \
    --argjson total_lines_added "$total_lines_added" \
    --argjson total_lines_removed "$total_lines_removed" \
    '{
      timestamp: $timestamp,
      session_id: $session_id,
      session_name: $session_name,
      transcript_path: $transcript_path,
      cwd: $cwd,
      version: $version,
      turn_no: $turn_no,
      model: $model,
      model_id: $model_id,
      previous_model: $previous_model,
      model_changed: $model_changed,
      tokens: {
        input: $input_tokens,
        output: $output_tokens,
        cache_read: $cache_read_tokens,
        cache_write: $cache_write_tokens,
        reasoning: $reasoning_tokens,
        total: $total_tokens,
        last_call_input: $last_call_input_tokens,
        last_call_output: $last_call_output_tokens
      },
      delta_tokens: {
        input: $delta_input_tokens,
        output: $delta_output_tokens,
        cache_read: $delta_cache_read_tokens,
        cache_write: $delta_cache_write_tokens,
        reasoning: $delta_reasoning_tokens,
        total: $delta_total_tokens
      },
      context: {
        current_context_tokens: $current_context_tokens,
        displayed_context_limit: $displayed_context_limit,
        current_context_used_percentage: $current_context_used_percentage
      },
      cost: {
        total_api_duration_ms: $total_api_duration_ms,
        total_duration_ms: $total_duration_ms,
        total_premium_requests: $total_premium_requests,
        total_lines_added: $total_lines_added,
        total_lines_removed: $total_lines_removed
      }
    }' >> "$jsonl_file"
fi

jq -n \
  --arg session_id "$session_id" \
  --arg session_name "$session_name" \
  --arg transcript_path "$transcript_path" \
  --arg model "$model" \
  --arg model_id "$model_id" \
  --argjson turn_no "$turn_no" \
  --argjson input_tokens "$input_tokens" \
  --argjson output_tokens "$output_tokens" \
  --argjson cache_read_tokens "$cache_read_tokens" \
  --argjson cache_write_tokens "$cache_write_tokens" \
  --argjson reasoning_tokens "$reasoning_tokens" \
  --argjson total_tokens "$total_tokens" \
  '{
    session_id: $session_id,
    session_name: $session_name,
    transcript_path: $transcript_path,
    model: $model,
    model_id: $model_id,
    turn_no: $turn_no,
    input_tokens: $input_tokens,
    output_tokens: $output_tokens,
    cache_read_tokens: $cache_read_tokens,
    cache_write_tokens: $cache_write_tokens,
    reasoning_tokens: $reasoning_tokens,
    total_tokens: $total_tokens
  }' > "$state_file"

# ==============================================================================
# ✅ 可自訂區域 (Customizable Area)
# ------------------------------------------------------------------------------
# 您可以自由修改以下區塊以調整狀態列（例如 Tmux）的顯示格式與圖示。
# 變更此區塊的邏輯「不會」影響後台的 Token 統計同步。
# ==============================================================================

display_input_total="$(fmt_tokens "$input_tokens")"
display_cache_read="$(fmt_tokens "$cache_read_tokens")"
display_cache_write="$(fmt_tokens "$cache_write_tokens")"
display_output="$(fmt_tokens "$output_tokens")"
display_reasoning="$(fmt_tokens "$reasoning_tokens")"
display_total="$(fmt_tokens "$total_tokens")"
display_delta_total="$(fmt_tokens "$delta_total")"
display_last_input="$(fmt_tokens "$last_call_input_tokens")"
display_last_cache_read="$(fmt_tokens "$delta_cache_read")"
display_last_output="$(fmt_tokens "$last_call_output_tokens")"

line="🤖 $model • #$turn_no • ↑ $display_input_total • c $display_cache_read/$display_cache_write • ↓ $display_output • r $display_reasoning • total $display_total • +$display_delta_total • last $display_last_input/$display_last_cache_read/$display_last_output"

if [ "$model_changed" = true ]; then
  line="$line • from $previous_model"
elif [ -n "$current_context_used_percentage" ]; then
  display_ctx=$(awk -v p="$current_context_used_percentage" 'BEGIN {
    if (p ~ /^[0-9]+(\.[0-9]+)?$/) {
      v = sprintf("%.2f", p)
      if (v ~ /\.00$/) {
        sub(/\.00$/, "", v)
      } else if (v ~ /0$/) {
        sub(/0$/, "", v)
      }
      print v
    } else {
      print p
    }
  }')
  line="$line • ctx ${display_ctx}%"
fi

printf "%s\n" "$line"