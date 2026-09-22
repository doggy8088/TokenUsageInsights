#!/usr/bin/env bash
#
# 驗證 scripts/install.sh --service 在 Linux 產生的 systemd 使用者服務單元。
#
# 測試不需要 systemd：以 stub 取代 uname 與 systemctl，在暫存目錄中執行真正的
# install.sh，再檢查產生的單元檔內容。
#
# 保護的重點（Issue #59）：WorkingDirectory= 不可加上雙引號，因為 systemd 取用
# 該值時不做引號剝除，會把引號視為路徑的一部分並以 "path is not absolute" 拒絕
# 載入；ExecStart= 則必須保留雙引號。同時確認規格符（%）仍被正確轉義。
#
# 用法：bash tests/install-systemd.test.sh
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
installer="${repo_root}/scripts/install.sh"
app_name="token-usage-insights"

failures=0

pass() {
  printf 'ok - %s\n' "$*"
}

fail() {
  printf 'FAIL - %s\n' "$*" >&2
  failures=$((failures + 1))
}

tmp_root="$(mktemp -d)"
cleanup() {
  rm -rf "$tmp_root"
}
trap cleanup EXIT

stub_dir="${tmp_root}/stubs"
mkdir -p "$stub_dir"

# install.sh 以 uname -s 選擇服務管理器；固定回報 Linux 才能驗證 systemd 分支。
cat > "${stub_dir}/uname" <<'STUB'
#!/usr/bin/env bash
if [[ "${1:-}" == "-s" ]]; then
  echo Linux
else
  exec /usr/bin/uname "$@"
fi
STUB

# systemctl 只會被呼叫來 reload／enable／start 服務，記錄後直接成功。
cat > "${stub_dir}/systemctl" <<'STUB'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "${SYSTEMCTL_LOG}"
exit 0
STUB

chmod +x "${stub_dir}/uname" "${stub_dir}/systemctl"

# 準備一個符合 Release 壓縮包結構的安裝來源目錄
release_dir="${tmp_root}/release"
mkdir -p "${release_dir}/static" "${release_dir}/shell" "${release_dir}/scripts"
cp "${installer}" "${release_dir}/install.sh"
printf '#!/usr/bin/env bash\nexit 0\n' > "${release_dir}/${app_name}"
chmod +x "${release_dir}/${app_name}"
cp "${repo_root}/shell/token-usage-insights.service" "${release_dir}/shell/"

new_home() {
  local home_dir="${tmp_root}/home-$1"
  mkdir -p "$home_dir"
  printf '%s' "$home_dir"
}

install_into() {
  local install_dir="$1" home_dir="$2"

  # 不繼承測試環境可能存在的 PORT／HOST，避免影響既有單元設定的繼承驗證
  env -u PORT -u HOST \
    "PATH=${stub_dir}:${PATH}" \
    "HOME=${home_dir}" \
    "SYSTEMCTL_LOG=${tmp_root}/systemctl.log" \
    "TOKEN_USAGE_INSIGHTS_INSTALL_DIR=${install_dir}" \
    "TOKEN_USAGE_INSIGHTS_BIN_DIR=${home_dir}/bin" \
    bash "${release_dir}/install.sh" --service
}

unit_file_for() {
  printf '%s' "$1/.config/systemd/user/${app_name}.service"
}

unit_value() {
  sed -n "s/^$2=//p" "$1"
}

begin_scenario() {
  printf '\n== %s ==\n' "$1"
}

require_unit_file() {
  local unit_file="$1" label="$2"

  if [[ -f "$unit_file" ]]; then
    pass "${label}: 產生單元檔"
    return 0
  fi

  fail "${label}: install.sh 未產生 ${unit_file}"
  return 1
}

check_working_directory() {
  local unit_file="$1" expected="$2" label="$3"
  local actual count

  count="$(grep -c '^WorkingDirectory=' "$unit_file" || true)"
  if [[ "$count" != "1" ]]; then
    fail "${label}: WorkingDirectory 應恰好出現一次，實際出現 ${count} 次"
    return
  fi

  if grep -q '^WorkingDirectory="' "$unit_file"; then
    fail "${label}: WorkingDirectory 不得以雙引號包住（systemd 會把引號視為路徑的一部分，導致 path is not absolute）"
    return
  fi

  actual="$(unit_value "$unit_file" WorkingDirectory)"
  if [[ "$actual" != "$expected" ]]; then
    fail "${label}: WorkingDirectory 應為 ${expected}，實際為 ${actual}"
    return
  fi

  if [[ "$actual" != /* ]]; then
    fail "${label}: WorkingDirectory 必須是絕對路徑，實際為 ${actual}"
    return
  fi

  pass "${label}: WorkingDirectory=${actual}"
}

check_exec_start() {
  local unit_file="$1" expected="$2" label="$3"
  local actual

  actual="$(unit_value "$unit_file" ExecStart)"
  if [[ "$actual" != "\"${expected}\"" ]]; then
    fail "${label}: ExecStart 應為 \"${expected}\"，實際為 ${actual}"
    return
  fi

  pass "${label}: ExecStart=${actual}"
}

check_environment() {
  local unit_file="$1" name="$2" expected="$3" label="$4"
  local actual

  actual="$(sed -n "s/^Environment=\"${name}=\(.*\)\"\$/\1/p" "$unit_file")"
  if [[ "$actual" != "$expected" ]]; then
    fail "${label}: Environment ${name} 應為 ${expected}，實際為 ${actual}"
    return
  fi

  pass "${label}: Environment ${name}=${actual}"
}

begin_scenario "情境 1：一般安裝目錄"
home_one="$(new_home one)"
install_one="${tmp_root}/app"
install_into "$install_one" "$home_one"
unit_one="$(unit_file_for "$home_one")"

if require_unit_file "$unit_one" "一般安裝目錄"; then
  check_working_directory "$unit_one" "$install_one" "一般安裝目錄"
  check_exec_start "$unit_one" "${install_one}/${app_name}" "一般安裝目錄"
  check_environment "$unit_one" "TOKEN_USAGE_INSIGHTS_INSTALL_DIR" "$install_one" "一般安裝目錄"
  check_environment "$unit_one" "PORT" "3003" "一般安裝目錄"
fi

begin_scenario "情境 2：安裝目錄含空白與規格符（%）"
home_two="$(new_home two)"
install_two="${tmp_root}/Token 戰情室 100% 用量"
expected_two="${tmp_root}/Token 戰情室 100%% 用量"
install_into "$install_two" "$home_two"
unit_two="$(unit_file_for "$home_two")"

if require_unit_file "$unit_two" "含空白與規格符"; then
  check_working_directory "$unit_two" "$expected_two" "含空白與規格符"
  check_exec_start "$unit_two" "${expected_two}/${app_name}" "含空白與規格符"
  check_environment "$unit_two" "TOKEN_USAGE_INSIGHTS_INSTALL_DIR" "$expected_two" "含空白與規格符"
fi

begin_scenario "情境 3：升級 v1.0.3 遺留的加引號單元"
home_three="$(new_home three)"
install_three="${tmp_root}/app-upgraded"
service_dir="${home_three}/.config/systemd/user"
mkdir -p "$service_dir"

# 模擬 v1.0.3 產生的單元，並帶有使用者自訂的綁定位址與連接埠
cat > "${service_dir}/${app_name}.service" <<LEGACY
[Unit]
Description=Token 戰情室 Dashboard Service
After=network.target

[Service]
Type=simple
WorkingDirectory="${tmp_root}/app-legacy"
ExecStart="${tmp_root}/app-legacy/${app_name}"
Restart=always
RestartSec=2
Environment="PORT=3999"
Environment="HOST=127.0.0.1"
Environment="TOKEN_USAGE_INSIGHTS_SERVICE=1"
Environment="TOKEN_USAGE_INSIGHTS_INSTALL_DIR=${tmp_root}/app-legacy"

[Install]
WantedBy=default.target
LEGACY

install_into "$install_three" "$home_three"
unit_three="$(unit_file_for "$home_three")"

if require_unit_file "$unit_three" "升級既有單元"; then
  check_working_directory "$unit_three" "$install_three" "升級既有單元"
  check_exec_start "$unit_three" "${install_three}/${app_name}" "升級既有單元"
  check_environment "$unit_three" "PORT" "3999" "升級既有單元"
  check_environment "$unit_three" "HOST" "127.0.0.1" "升級既有單元"
fi

begin_scenario "情境 4：shell/token-usage-insights.service 範本"
template="${repo_root}/shell/token-usage-insights.service"
if [[ ! -f "$template" ]]; then
  fail "範本不存在：${template}"
else
  template_count="$(grep -c '^WorkingDirectory=' "$template" || true)"
  template_value="$(unit_value "$template" WorkingDirectory)"
  if [[ "$template_count" != "1" || "$template_value" != "<PROJECT_DIR>" ]]; then
    fail "Makefile 服務範本: WorkingDirectory 應為未加引號的 <PROJECT_DIR>，實際為 ${template_value}（共 ${template_count} 行）"
  else
    pass "Makefile 服務範本: WorkingDirectory=${template_value}"
  fi
fi

begin_scenario "情境 5：systemd-analyze --user verify（僅在可用時執行）"
if ! command -v systemd-analyze >/dev/null 2>&1; then
  printf 'skip - 找不到 systemd-analyze，略過此檢查\n'
elif systemd-analyze --user verify "$unit_one" > "${tmp_root}/systemd-analyze.log" 2>&1; then
  pass "systemd-analyze --user verify 接受產生的單元"
elif grep -q 'WorkingDirectory' "${tmp_root}/systemd-analyze.log"; then
  fail "systemd-analyze --user verify 不接受 WorkingDirectory：$(tr '\n' ' ' < "${tmp_root}/systemd-analyze.log")"
else
  printf 'skip - systemd-analyze 回報與 WorkingDirectory 無關的訊息，不列入判定：\n'
  sed 's/^/    /' "${tmp_root}/systemd-analyze.log"
fi

printf '\n'
if [[ "$failures" -ne 0 ]]; then
  printf '%s 項檢查未通過\n' "$failures" >&2
  exit 1
fi

printf 'install.sh systemd 單元測試全數通過\n'
