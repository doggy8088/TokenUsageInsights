#!/usr/bin/env bash
set -euo pipefail

app_name="token-usage-insights"
default_install_dir="${HOME}/.local/share/${app_name}"
default_bin_dir="${HOME}/.local/bin"

install_dir="${TOKEN_USAGE_INSIGHTS_INSTALL_DIR:-$default_install_dir}"
bin_dir="${TOKEN_USAGE_INSIGHTS_BIN_DIR:-$default_bin_dir}"
port="${PORT:-3003}"
host="${HOST:-0.0.0.0}"
install_service=false

usage() {
  cat <<USAGE
Usage: ./install.sh [--service]

Environment:
  TOKEN_USAGE_INSIGHTS_INSTALL_DIR  Install directory. Default: ${default_install_dir}
  TOKEN_USAGE_INSIGHTS_BIN_DIR      Directory for the executable link. Default: ${default_bin_dir}
  HOST                              Dashboard bind address. Default: 0.0.0.0
  PORT                              Dashboard port. Default: 3003

Options:
  --service                         Install and enable a background user service
                                    (systemd on Linux; launchd on macOS).
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --service)
      install_service=true
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      usage
      exit 1
      ;;
  esac
done

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [[ -f "${script_dir}/${app_name}" ]]; then
  release_dir="$script_dir"
else
  release_dir="$(cd "${script_dir}/.." && pwd)"
fi

binary_src="${release_dir}/${app_name}"
if [[ ! -f "$binary_src" ]]; then
  echo "Missing executable: ${binary_src}" >&2
  echo "Run this installer from an extracted Token 戰情室 release package." >&2
  exit 1
fi

mkdir -p "$install_dir" "$bin_dir"

install -m 755 "$binary_src" "${install_dir}/${app_name}"

for item in static shell scripts; do
  if [[ -e "${release_dir}/${item}" ]]; then
    rm -rf "${install_dir:?}/${item}"
    cp -R "${release_dir}/${item}" "${install_dir}/${item}"
  fi
done

for file in pricing.csv README.md LICENSE VERSION; do
  if [[ -f "${release_dir}/${file}" ]]; then
    cp "${release_dir}/${file}" "${install_dir}/${file}"
  fi
done

  marker_path="${install_dir}/.install_marker"
  marker_tmp="${install_dir}/.install_marker.tmp.$$"
  printf "token-usage-insights:installed" > "$marker_tmp"
  mv -f "$marker_tmp" "$marker_path"

ln -sfn "${install_dir}/${app_name}" "${bin_dir}/${app_name}"

if [[ "$install_service" == true ]]; then
  runtime_vars=(
    INSIGHTS_DIR
    ANTIGRAVITY_DIR
    COPILOT_DIR
    COPILOT_APP_DIR
    CODEX_DIR
    CLAUDE_DIR
    CURSOR_DIR
    CURSOR_STATE_DB
    GROK_DIR
    PI_DIR
    OMP_DIR
    MUSE_DIR
    MCODE_DIR
    MCODE_STATE_DB
    VSCODE_DIR
    VSCODE_USER_DATA_DIR
    VSCODE_PORTABLE_DATA_DIR
    CORS_ALLOWED_ORIGINS
  )
  case "$(uname -s)" in
    Linux)
      if ! command -v systemctl >/dev/null 2>&1; then
        echo "systemctl was not found; cannot install the user service." >&2
        exit 1
      fi

      service_dir="${HOME}/.config/systemd/user"
      service_file="${service_dir}/${app_name}.service"
      mkdir -p "$service_dir"

      # General unit value escaping (for WorkingDirectory and Environment):
      # Escapes \, ", and % (specifier expansion)
      systemd_escape_value() {
        printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e 's/%/%%/g'
      }

      # General unit value unescaping (when reading existing Environment values):
      # Unescapes %%, \", and \\ back to raw shell values
      systemd_unescape_value() {
        printf '%s' "$1" | sed -e 's/%%/%/g' -e 's/\\"/"/g' -e 's/\\\\/\\/g'
      }

      # Command-line escaping (for ExecStart):
      # Escapes \, ", %, and $ (which systemd expands in ExecStart command lines)
      systemd_escape_exec() {
        printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e 's/\$/\$\$/g' -e 's/%/%%/g'
      }

      install_dir_systemd="$(systemd_escape_value "$install_dir")"
      executable_systemd="$(systemd_escape_exec "${install_dir}/${app_name}")"
      host_systemd="$(systemd_escape_value "$host")"
      port_systemd="$(systemd_escape_value "$port")"

      # 若未於環境變數明確指定更新設定，自動繼承既有 systemd 服務單元之設定
      if [[ -f "$service_file" ]]; then
        if [[ -z "${TOKEN_USAGE_INSIGHTS_AUTO_UPDATE+x}" ]]; then
          existing_auto_update="$(sed -n -E 's/^[[:space:]]*Environment="?TOKEN_USAGE_INSIGHTS_AUTO_UPDATE=(([^"\\]|\\.)*)"?$/\1/p' "$service_file" | tail -n 1)"
          if [[ -n "$existing_auto_update" ]]; then
            TOKEN_USAGE_INSIGHTS_AUTO_UPDATE="$(systemd_unescape_value "$existing_auto_update")"
          fi
        fi
        if [[ -z "${TOKEN_USAGE_INSIGHTS_UPDATE_INTERVAL_HOURS+x}" ]]; then
          existing_interval="$(sed -n -E 's/^[[:space:]]*Environment="?TOKEN_USAGE_INSIGHTS_UPDATE_INTERVAL_HOURS=(([^"\\]|\\.)*)"?$/\1/p' "$service_file" | tail -n 1)"
          if [[ -n "$existing_interval" ]]; then
            TOKEN_USAGE_INSIGHTS_UPDATE_INTERVAL_HOURS="$(systemd_unescape_value "$existing_interval")"
          fi
        fi
        for var in "${runtime_vars[@]}"; do
          if [[ -z "${!var+x}" ]]; then
            existing_val="$(sed -n -E "s/^[[:space:]]*Environment=\"?${var}=(([^\"\\\\]|\\\\.)*)\"?\$/\\1/p" "$service_file" | tail -n 1)"
            if [[ -n "$existing_val" ]]; then
              existing_unescaped="$(systemd_unescape_value "$existing_val")"
              printf -v "$var" '%s' "$existing_unescaped"
            fi
          fi
        done
        if [[ -z "${CORS_ALLOWED_ORIGINS:-}" && -z "${CORS_ALLOWED_ORIGINS+x}" ]]; then
          legacy_cors="$(sed -n -E 's/^[[:space:]]*Environment="?CORS_ALLOW_ORIGIN=(([^"\\]|\\.)*)"?$/\1/p' "$service_file" | tail -n 1)"
          if [[ -n "$legacy_cors" ]]; then
            CORS_ALLOWED_ORIGINS="$(systemd_unescape_value "$legacy_cors")"
          fi
        fi
        # 保留既有服務單元之綁定位址與連接埠：重裝時若未明確指定 HOST／PORT，
        # 不得將自訂綁定（如 127.0.0.1:8080）改寫回預設值
        if [[ -z "${PORT+x}" ]]; then
          existing_port="$(sed -n -E 's/^[[:space:]]*Environment="?PORT=(([^"\\]|\\.)*)"?$/\1/p' "$service_file" | tail -n 1)"
          if [[ -n "$existing_port" ]]; then
            port="$(systemd_unescape_value "$existing_port")"
          fi
        fi
        if [[ -z "${HOST+x}" ]]; then
          existing_host="$(sed -n -E 's/^[[:space:]]*Environment="?HOST=(([^"\\]|\\.)*)"?$/\1/p' "$service_file" | tail -n 1)"
          if [[ -n "$existing_host" ]]; then
            host="$(systemd_unescape_value "$existing_host")"
          fi
        fi

        # 繼承既有設定後重新計算轉義值，確保產生的單元檔使用最終綁定設定
        host_systemd="$(systemd_escape_value "$host")"
        port_systemd="$(systemd_escape_value "$port")"

      fi

      extra_env_systemd=""
      if [[ -n "${TOKEN_USAGE_INSIGHTS_AUTO_UPDATE:-}" ]]; then
        extra_env_systemd+="$(printf '\nEnvironment="TOKEN_USAGE_INSIGHTS_AUTO_UPDATE=%s"' "$(systemd_escape_value "$TOKEN_USAGE_INSIGHTS_AUTO_UPDATE")")"
      fi
      if [[ -n "${TOKEN_USAGE_INSIGHTS_UPDATE_INTERVAL_HOURS:-}" ]]; then
        extra_env_systemd+="$(printf '\nEnvironment="TOKEN_USAGE_INSIGHTS_UPDATE_INTERVAL_HOURS=%s"' "$(systemd_escape_value "$TOKEN_USAGE_INSIGHTS_UPDATE_INTERVAL_HOURS")")"
      fi
      for var in "${runtime_vars[@]}"; do
        val="${!var:-}"
        if [[ -n "$val" ]]; then
          extra_env_systemd+="$(printf '\nEnvironment="%s=%s"' "$var" "$(systemd_escape_value "$val")")"
        fi
      done

      cat > "$service_file" <<SERVICE
[Unit]
Description=Token 戰情室 Dashboard Service
After=network.target

[Service]
Type=simple
WorkingDirectory="${install_dir_systemd}"
ExecStart="${executable_systemd}"
Restart=always
RestartSec=2
Environment="PORT=${port_systemd}"
Environment="HOST=${host_systemd}"
Environment="TOKEN_USAGE_INSIGHTS_SERVICE=1"
Environment="TOKEN_USAGE_INSIGHTS_INSTALL_DIR=${install_dir_systemd}"${extra_env_systemd}

[Install]
WantedBy=default.target
SERVICE

      systemctl --user daemon-reload
      systemctl --user enable "${app_name}.service"
      if systemctl --user is-active --quiet "${app_name}.service"; then
        systemctl --user restart "${app_name}.service"
      else
        systemctl --user start "${app_name}.service"
      fi
      ;;
    Darwin)
      if ! command -v launchctl >/dev/null 2>&1; then
        echo "launchctl was not found; cannot install the launchd agent." >&2
        exit 1
      fi
      if ! command -v plutil >/dev/null 2>&1; then
        echo "plutil was not found; cannot validate the launchd agent plist." >&2
        exit 1
      fi

      launch_agents_dir="${HOME}/Library/LaunchAgents"
      launch_logs_dir="${HOME}/Library/Logs"
      launch_label="com.tokenusageinsights"
      launch_agent_file="${launch_agents_dir}/${launch_label}.plist"
      launch_domain="gui/$(id -u)"
      mkdir -p "$launch_agents_dir" "$launch_logs_dir"

      plist_escape() {
        printf '%s' "$1" | sed \
          -e 's/&/\&amp;/g' \
          -e 's/</\&lt;/g' \
          -e 's/>/\&gt;/g' \
          -e 's/"/\&quot;/g' \
          -e "s/'/\&apos;/g"
      }

      executable_plist="$(plist_escape "${install_dir}/${app_name}")"
      install_dir_plist="$(plist_escape "$install_dir")"
      host_plist="$(plist_escape "$host")"
      port_plist="$(plist_escape "$port")"
      stdout_log_plist="$(plist_escape "${launch_logs_dir}/${launch_label}.out.log")"
      stderr_log_plist="$(plist_escape "${launch_logs_dir}/${launch_label}.err.log")"

      # 若未於環境變數明確指定更新設定，自動繼承既有 launchd agent plist 之設定
      if [[ -f "$launch_agent_file" ]]; then
        if [[ -z "${TOKEN_USAGE_INSIGHTS_AUTO_UPDATE+x}" ]]; then
          if existing_auto_update="$(plutil -extract EnvironmentVariables.TOKEN_USAGE_INSIGHTS_AUTO_UPDATE raw -o - "$launch_agent_file" 2>/dev/null)" && [[ -n "$existing_auto_update" ]]; then
            TOKEN_USAGE_INSIGHTS_AUTO_UPDATE="$existing_auto_update"
          fi
        fi
        if [[ -z "${TOKEN_USAGE_INSIGHTS_UPDATE_INTERVAL_HOURS+x}" ]]; then
          if existing_interval="$(plutil -extract EnvironmentVariables.TOKEN_USAGE_INSIGHTS_UPDATE_INTERVAL_HOURS raw -o - "$launch_agent_file" 2>/dev/null)" && [[ -n "$existing_interval" ]]; then
            TOKEN_USAGE_INSIGHTS_UPDATE_INTERVAL_HOURS="$existing_interval"
          fi
        fi
        for var in "${runtime_vars[@]}"; do
          if [[ -z "${!var+x}" ]]; then
            if existing_val="$(plutil -extract "EnvironmentVariables.${var}" raw -o - "$launch_agent_file" 2>/dev/null)" && [[ -n "$existing_val" ]]; then
              printf -v "$var" '%s' "$existing_val"
            fi
          fi
        done
        if [[ -z "${CORS_ALLOWED_ORIGINS:-}" && -z "${CORS_ALLOWED_ORIGINS+x}" ]]; then
          if legacy_cors="$(plutil -extract EnvironmentVariables.CORS_ALLOW_ORIGIN raw -o - "$launch_agent_file" 2>/dev/null)" && [[ -n "$legacy_cors" ]]; then
            CORS_ALLOWED_ORIGINS="$legacy_cors"
          fi
        fi
        # 保留既有 launchd agent 之綁定位址與連接埠：重裝時若未明確指定 HOST／PORT，不得改寫回預設值
        if [[ -z "${PORT+x}" ]]; then
          if existing_port="$(plutil -extract EnvironmentVariables.PORT raw -o - "$launch_agent_file" 2>/dev/null)" && [[ -n "$existing_port" ]]; then
            port="$existing_port"
          fi
        fi
        if [[ -z "${HOST+x}" ]]; then
          if existing_host="$(plutil -extract EnvironmentVariables.HOST raw -o - "$launch_agent_file" 2>/dev/null)" && [[ -n "$existing_host" ]]; then
            host="$existing_host"
          fi
        fi

        # 繼承既有設定後重新計算轉義值，確保產生的 plist 使用最終綁定設定
        host_plist="$(plist_escape "$host")"
        port_plist="$(plist_escape "$port")"

      fi

      extra_env_plist=""
      if [[ -n "${TOKEN_USAGE_INSIGHTS_AUTO_UPDATE:-}" ]]; then
        auto_update_plist="$(plist_escape "$TOKEN_USAGE_INSIGHTS_AUTO_UPDATE")"
        extra_env_plist+="$(printf '\n    <key>TOKEN_USAGE_INSIGHTS_AUTO_UPDATE</key>\n    <string>%s</string>' "$auto_update_plist")"
      fi
      if [[ -n "${TOKEN_USAGE_INSIGHTS_UPDATE_INTERVAL_HOURS:-}" ]]; then
        interval_plist="$(plist_escape "$TOKEN_USAGE_INSIGHTS_UPDATE_INTERVAL_HOURS")"
        extra_env_plist+="$(printf '\n    <key>TOKEN_USAGE_INSIGHTS_UPDATE_INTERVAL_HOURS</key>\n    <string>%s</string>' "$interval_plist")"
      fi
      for var in "${runtime_vars[@]}"; do
        val="${!var:-}"
        if [[ -n "$val" ]]; then
          val_plist="$(plist_escape "$val")"
          extra_env_plist+="$(printf '\n    <key>%s</key>\n    <string>%s</string>' "$var" "$val_plist")"
        fi
      done

      cat > "$launch_agent_file" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>${launch_label}</string>
  <key>ProgramArguments</key>
  <array>
    <string>${executable_plist}</string>
  </array>
  <key>WorkingDirectory</key>
  <string>${install_dir_plist}</string>
  <key>EnvironmentVariables</key>
  <dict>
    <key>HOST</key>
    <string>${host_plist}</string>
    <key>PORT</key>
    <string>${port_plist}</string>
    <key>TOKEN_USAGE_INSIGHTS_SERVICE</key>
    <string>1</string>
    <key>TOKEN_USAGE_INSIGHTS_INSTALL_DIR</key>
    <string>${install_dir_plist}</string>${extra_env_plist}
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardOutPath</key>
  <string>${stdout_log_plist}</string>
  <key>StandardErrorPath</key>
  <string>${stderr_log_plist}</string>
</dict>
</plist>
PLIST

      plutil -lint "$launch_agent_file"

      # A previous instance may be loaded; bootout is intentionally harmless if it is not.
      launchctl bootout "${launch_domain}/${launch_label}" >/dev/null 2>&1 || true
      launchctl bootstrap "$launch_domain" "$launch_agent_file"
      ;;
    *)
      echo "--service is unsupported on $(uname -s). Supported platforms: Linux (systemd) and macOS (launchd)." >&2
      exit 1
      ;;
  esac
fi

cat <<DONE
Token 戰情室 installed.

Install directory:
  ${install_dir}

Executable:
  ${bin_dir}/${app_name}

Run:
  HOST=${host} PORT=${port} ${bin_dir}/${app_name}
DONE
