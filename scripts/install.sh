#!/usr/bin/env bash
set -euo pipefail

app_name="token-usage-insights"
default_install_dir="${HOME}/.local/share/${app_name}"
default_bin_dir="${HOME}/.local/bin"

install_dir="${TOKEN_USAGE_INSIGHTS_INSTALL_DIR:-$default_install_dir}"
bin_dir="${TOKEN_USAGE_INSIGHTS_BIN_DIR:-$default_bin_dir}"
port="${PORT:-3003}"
install_service=false

usage() {
  cat <<USAGE
Usage: ./install.sh [--service]

Environment:
  TOKEN_USAGE_INSIGHTS_INSTALL_DIR  Install directory. Default: ${default_install_dir}
  TOKEN_USAGE_INSIGHTS_BIN_DIR      Directory for the executable link. Default: ${default_bin_dir}
  PORT                              Dashboard port. Default: 3003

Options:
  --service                         Install and enable a systemd user service on Linux.
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

"${install_dir}/${app_name}" --backfill-copilot-usage

if [[ "$install_service" == true ]]; then
  if [[ "$(uname -s)" != "Linux" ]]; then
    echo "--service is only supported on Linux with systemd." >&2
    exit 1
  fi
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

[Install]
WantedBy=default.target
SERVICE

  systemctl --user daemon-reload
  systemctl --user enable --now "${app_name}.service"
fi

cat <<DONE
Token 戰情室 installed.

Install directory:
  ${install_dir}

Executable:
  ${bin_dir}/${app_name}

Run:
  PORT=${port} ${bin_dir}/${app_name}
DONE
