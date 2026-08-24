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

ln -sfn "${install_dir}/${app_name}" "${bin_dir}/${app_name}"

if [[ "$install_service" == true ]]; then
  case "$(uname -s)" in
    Linux)
      if ! command -v systemctl >/dev/null 2>&1; then
        echo "systemctl was not found; cannot install the user service." >&2
        exit 1
      fi

      service_dir="${HOME}/.config/systemd/user"
      service_file="${service_dir}/${app_name}.service"
      mkdir -p "$service_dir"
      cat > "$service_file" <<SERVICE
[Unit]
Description=Token 戰情室 Dashboard Service
After=network.target

[Service]
Type=simple
WorkingDirectory=${install_dir}
ExecStart=${install_dir}/${app_name}
Restart=always
RestartSec=5
Environment=PORT=${port}
Environment=HOST=${host}

[Install]
WantedBy=default.target
SERVICE

      systemctl --user daemon-reload
      systemctl --user enable --now "${app_name}.service"
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
