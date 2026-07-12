#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
workdir="$(mktemp -d)"
trap 'rm -rf "$workdir"' EXIT

release_dir="${workdir}/release"
install_dir="${workdir}/install"
bin_dir="${workdir}/bin"
mkdir -p "$release_dir/static" "$release_dir/shell" "$release_dir/scripts"

cp "${repo_root}/scripts/install.sh" "${release_dir}/install.sh"
cat > "${release_dir}/token-usage-insights" <<'APP'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "${TOKEN_USAGE_INSIGHTS_TEST_LOG:?}"
APP
chmod +x "${release_dir}/token-usage-insights" "${release_dir}/install.sh"
touch "${release_dir}/pricing.csv"

export TOKEN_USAGE_INSIGHTS_INSTALL_DIR="$install_dir"
export TOKEN_USAGE_INSIGHTS_BIN_DIR="$bin_dir"
export TOKEN_USAGE_INSIGHTS_TEST_LOG="${workdir}/calls.log"

"${release_dir}/install.sh"

if ! grep -Fx -- "--backfill-copilot-usage" "$TOKEN_USAGE_INSIGHTS_TEST_LOG" >/dev/null; then
  echo "install.sh should run Copilot backfill automatically." >&2
  exit 1
fi

if rg -n -- "--backfill-copilot(\\s|$)|BackfillCopilot" \
  "${repo_root}/scripts/install.sh" \
  "${repo_root}/scripts/install.ps1" \
  "${repo_root}/README.md" >/dev/null; then
  echo "Copilot backfill should no longer be documented or implemented as an opt-in installer flag." >&2
  exit 1
fi

if ! rg -n -- '\$AppName\.exe"\) --backfill-copilot-usage' "${repo_root}/scripts/install.ps1" >/dev/null; then
  echo "install.ps1 should run Copilot backfill automatically." >&2
  exit 1
fi

echo "Installer backfill tests passed."
