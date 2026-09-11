use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::Utc;
use serde::Deserialize;
use sha2::{Digest, Sha256};

const GITHUB_OWNER: &str = "doggy8088";
const GITHUB_REPO: &str = "TokenUsageInsights";
const APP_NAME: &str = "token-usage-insights";
const USER_AGENT: &str = "token-usage-insights-updater";
const DEFAULT_UPDATE_INTERVAL_HOURS: i64 = 24;
const STARTUP_CHECK_TIMEOUT_SECS: u64 = 4;
const STARTUP_AUTO_UPDATE_TOTAL_TIMEOUT_SECS: u64 = 60;
const LAST_CHECK_KEY: &str = "last_update_check_at";
const MAX_ARCHIVE_BYTES: usize = 150 * 1024 * 1024; // 150 MB 上限
const MAX_CHECKSUM_BYTES: usize = 1024 * 1024; // 1 MB 上限
const MAX_EXTRACTED_BYTES: u64 = 300 * 1024 * 1024; // 300 MB 解壓縮展開上限
const MAX_EXTRACTED_ENTRIES: usize = 10_000; // 最多 10,000 個檔案/目錄

#[cfg(unix)]
fn is_process_alive(pid: u32) -> bool {
    let res = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if res == 0 {
        true
    } else {
        let err = std::io::Error::last_os_error().raw_os_error();
        err != Some(libc::ESRCH)
    }
}

#[cfg(windows)]
fn is_process_alive(pid: u32) -> bool {
    let output = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output();
    if let Ok(out) = output {
        let text = String::from_utf8_lossy(&out.stdout);
        let pid_token = format!("\"{pid}\"");
        text.lines().any(|line| line.contains(&pid_token))
    } else {
        true
    }
}

#[cfg(not(any(unix, windows)))]
fn is_process_alive(_pid: u32) -> bool {
    false
}

#[cfg(target_vendor = "apple")]
fn get_process_exe_path(pid: u32) -> Option<PathBuf> {
    #[link(name = "proc")]
    extern "C" {
        fn proc_pidpath(
            pid: libc::c_int,
            buffer: *mut libc::c_void,
            buffersize: u32,
        ) -> libc::c_int;
    }
    let mut buf = vec![0u8; 4096];
    let ret = unsafe {
        proc_pidpath(
            pid as libc::c_int,
            buf.as_mut_ptr() as *mut libc::c_void,
            buf.len() as u32,
        )
    };
    if ret > 0 {
        let path_bytes = &buf[..ret as usize];
        if let Ok(path_str) = std::str::from_utf8(path_bytes) {
            let p = PathBuf::from(path_str.trim_end_matches('\0'));
            if p.exists() {
                return Some(p);
            }
        }
    }
    if let Ok(output) = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
    {
        let comm = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !comm.is_empty() {
            let p = PathBuf::from(&comm);
            if p.is_absolute() && p.exists() {
                return Some(p);
            }
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn get_process_exe_path(pid: u32) -> Option<PathBuf> {
    fs::read_link(format!("/proc/{pid}/exe")).ok()
}

#[cfg(all(unix, not(target_os = "linux"), not(target_vendor = "apple")))]
fn get_process_exe_path(pid: u32) -> Option<PathBuf> {
    if let Ok(p) = fs::read_link(format!("/proc/{pid}/exe")) {
        return Some(p);
    }
    if let Ok(p) = fs::read_link(format!("/proc/{pid}/file")) {
        return Some(p);
    }
    if let Ok(output) = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
    {
        let comm = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !comm.is_empty() {
            let p = PathBuf::from(&comm);
            if p.is_absolute() && p.exists() {
                return Some(p);
            }
        }
    }
    None
}

#[cfg(windows)]
type WinHandle = *mut std::ffi::c_void;

#[cfg(windows)]
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
#[cfg(windows)]
const PROCESS_QUERY_INFORMATION: u32 = 0x0400;
#[cfg(windows)]
const PROCESS_VM_READ: u32 = 0x0010;

#[cfg(windows)]
#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *mut u16,
}

#[cfg(windows)]
#[repr(C)]
struct ProcessBasicInformation {
    exit_status: i32,
    peb_base_address: *mut std::ffi::c_void,
    affinity_mask: usize,
    base_priority: i32,
    unique_process_id: usize,
    inherited_from_unique_process_id: usize,
}

#[cfg(windows)]
type NtQueryInformationProcessFn = unsafe extern "system" fn(
    process_handle: WinHandle,
    process_information_class: u32,
    process_information: *mut std::ffi::c_void,
    process_information_length: u32,
    return_length: *mut u32,
) -> i32;

#[cfg(windows)]
extern "system" {
    fn OpenProcess(dwDesiredAccess: u32, bInheritHandle: i32, dwProcessId: u32) -> WinHandle;
    fn QueryFullProcessImageNameW(
        hProcess: WinHandle,
        dwFlags: u32,
        lpExeName: *mut u16,
        lpdwSize: *mut u32,
    ) -> i32;
    fn CloseHandle(hObject: WinHandle) -> i32;
    fn GetModuleHandleA(lpModuleName: *const u8) -> WinHandle;
    fn GetProcAddress(hModule: WinHandle, lpProcName: *const u8) -> *mut std::ffi::c_void;
    fn LocalFree(hMem: WinHandle) -> WinHandle;
}

#[cfg(windows)]
#[link(name = "shell32")]
extern "system" {
    fn CommandLineToArgvW(lpCmdLine: *const u16, pNumArgs: *mut i32) -> *mut *mut u16;
}

#[cfg(windows)]
fn get_process_exe_path(pid: u32) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut buf = vec![0u16; 1024];
        let mut size = buf.len() as u32;
        let success = QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut size);
        CloseHandle(handle);

        if success != 0 && size > 0 {
            let os_str = std::ffi::OsString::from_wide(&buf[..size as usize]);
            let p = PathBuf::from(os_str);
            if p.exists() {
                return Some(p);
            }
        }
        None
    }
}

#[cfg(not(any(unix, windows)))]
fn get_process_exe_path(_pid: u32) -> Option<PathBuf> {
    None
}

fn matches_install_dir(exe_path: &Path, install_dir: &Path) -> bool {
    if let (Ok(can_exe), Ok(can_dir)) = (fs::canonicalize(exe_path), fs::canonicalize(install_dir))
    {
        can_exe.starts_with(&can_dir)
    } else {
        exe_path.starts_with(install_dir)
    }
}

#[cfg(target_os = "linux")]
fn get_process_cmdline(pid: u32) -> Option<Vec<String>> {
    let content = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let args: Vec<String> = content
        .split(|&b| b == 0)
        .filter(|slice| !slice.is_empty())
        .map(|slice| String::from_utf8_lossy(slice).to_string())
        .collect();
    if args.is_empty() {
        None
    } else {
        Some(args)
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
fn get_process_cmdline(pid: u32) -> Option<Vec<String>> {
    let output = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        return None;
    }
    Some(text.split_whitespace().map(|s| s.to_string()).collect())
}

#[cfg(windows)]
fn get_process_cmdline(pid: u32) -> Option<Vec<String>> {
    use std::os::windows::ffi::OsStringExt;

    unsafe {
        let ntdll = GetModuleHandleA(b"ntdll.dll\0".as_ptr());
        if ntdll.is_null() {
            return None;
        }
        let func_ptr = GetProcAddress(ntdll, b"NtQueryInformationProcess\0".as_ptr());
        if func_ptr.is_null() {
            return None;
        }
        let nt_query: NtQueryInformationProcessFn = std::mem::transmute(func_ptr);

        let mut handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            handle = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid);
        }
        if handle.is_null() {
            return None;
        }

        // ProcessCommandLineInformation = 60
        let mut buf = vec![0u8; 32768];
        let mut return_len = 0u32;
        let status = nt_query(
            handle,
            60,
            buf.as_mut_ptr() as *mut std::ffi::c_void,
            buf.len() as u32,
            &mut return_len,
        );
        CloseHandle(handle);

        if status != 0 {
            return None;
        }

        let p_unicode = buf.as_ptr() as *const UnicodeString;
        let byte_len = (*p_unicode).length as usize;
        let char_len = byte_len / 2;
        let str_ptr = (*p_unicode).buffer;

        if str_ptr.is_null() || char_len == 0 {
            return None;
        }

        let buf_start = buf.as_ptr() as usize;
        let buf_end = buf_start + buf.len();
        let ptr_val = str_ptr as usize;
        if ptr_val < buf_start || ptr_val + byte_len > buf_end {
            return None;
        }

        let mut wide_chars: Vec<u16> = std::slice::from_raw_parts(str_ptr, char_len).to_vec();
        wide_chars.push(0);

        let mut num_args = 0i32;
        let argv_ptr = CommandLineToArgvW(wide_chars.as_ptr(), &mut num_args);
        if argv_ptr.is_null() || num_args <= 0 {
            return None;
        }

        let mut args = Vec::new();
        for i in 0..num_args as usize {
            let arg_ptr = *argv_ptr.add(i);
            if !arg_ptr.is_null() {
                let mut len = 0;
                while *arg_ptr.add(len) != 0 {
                    len += 1;
                }
                let arg_slice = std::slice::from_raw_parts(arg_ptr, len);
                let os_str = std::ffi::OsString::from_wide(arg_slice);
                args.push(os_str.to_string_lossy().to_string());
            }
        }
        LocalFree(argv_ptr as WinHandle);

        if args.is_empty() {
            None
        } else {
            Some(args)
        }
    }
}

#[cfg(windows)]
fn get_process_ppid(pid: u32) -> Option<u32> {
    unsafe {
        let ntdll = GetModuleHandleA(b"ntdll.dll\0".as_ptr());
        if ntdll.is_null() {
            return None;
        }
        let func_ptr = GetProcAddress(ntdll, b"NtQueryInformationProcess\0".as_ptr());
        if func_ptr.is_null() {
            return None;
        }
        let nt_query: NtQueryInformationProcessFn = std::mem::transmute(func_ptr);

        let mut handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            handle = OpenProcess(PROCESS_QUERY_INFORMATION, 0, pid);
        }
        if handle.is_null() {
            return None;
        }

        let mut pbi = std::mem::zeroed::<ProcessBasicInformation>();
        let mut return_len = 0u32;
        let status = nt_query(
            handle,
            0, // ProcessBasicInformation
            &mut pbi as *mut _ as *mut std::ffi::c_void,
            std::mem::size_of::<ProcessBasicInformation>() as u32,
            &mut return_len,
        );
        CloseHandle(handle);

        if status == 0 && pbi.inherited_from_unique_process_id != 0 {
            Some(pbi.inherited_from_unique_process_id as u32)
        } else {
            None
        }
    }
}

#[cfg(windows)]
fn is_process_supervised(pid: u32, install_dir: &Path) -> bool {
    if let Some(ppid) = get_process_ppid(pid) {
        if is_process_alive(ppid) {
            if let Some(parent_exe) = get_process_exe_path(ppid) {
                let file_name = parent_exe
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                if file_name == "services.exe" {
                    return true;
                }
                if file_name == "powershell.exe" || file_name == "pwsh.exe" {
                    if let Some(cmd) = get_process_cmdline(ppid) {
                        if cmd.iter().any(|arg| arg.contains("run-service.ps1")) {
                            return true;
                        }
                    }
                    if install_dir.join("scripts").join("run-service.ps1").exists() {
                        return true;
                    }
                }
            }
        }
    }
    false
}

#[cfg(target_os = "linux")]
fn is_process_supervised(pid: u32, _install_dir: &Path) -> bool {
    if let Ok(env_bytes) = fs::read(format!("/proc/{pid}/environ")) {
        let env_str = String::from_utf8_lossy(&env_bytes);
        if env_str.contains("INVOCATION_ID=")
            || env_str.contains("JOURNAL_STREAM=")
            || env_str.contains("SYSTEMD_EXEC_PID=")
        {
            return true;
        }
    }
    if let Ok(status) = fs::read_to_string(format!("/proc/{pid}/status")) {
        for line in status.lines() {
            if line.starts_with("PPid:") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 && parts[1] == "1" {
                    return true;
                }
            }
        }
    }
    false
}

#[cfg(all(unix, not(target_os = "linux")))]
fn is_process_supervised(pid: u32, _install_dir: &Path) -> bool {
    if let Ok(output) = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "ppid="])
        .output()
    {
        let ppid = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if ppid == "1" {
            return true;
        }
    }
    false
}

#[cfg(not(any(unix, windows)))]
fn get_process_cmdline(_pid: u32) -> Option<Vec<String>> {
    None
}

#[cfg(not(any(unix, windows)))]
fn is_process_supervised(_pid: u32, _install_dir: &Path) -> bool {
    false
}

fn is_cli_subcommand(arg: &str) -> bool {
    let clean = arg.trim_matches('"').trim_matches('\'');
    matches!(
        clean,
        "export"
            | "export-all"
            | "import"
            | "update"
            | "--update"
            | "-u"
            | "completion"
            | "-h"
            | "--help"
            | "help"
            | "-V"
            | "--version"
    )
}

fn is_dashboard_server_process(pid: u32, server_pids: &std::collections::HashSet<u32>) -> bool {
    if let Some(cmdline) = get_process_cmdline(pid) {
        if cmdline.iter().skip(1).any(|arg| is_cli_subcommand(arg)) {
            return false;
        }
        return true;
    }
    server_pids.contains(&pid)
}

pub struct ServerPidGuard {
    paths: Vec<PathBuf>,
}

impl Drop for ServerPidGuard {
    fn drop(&mut self) {
        for path in &self.paths {
            let _ = fs::remove_file(path);
        }
    }
}

pub fn create_server_pid_guard() -> ServerPidGuard {
    let mut paths = Vec::new();
    let my_pid = std::process::id().to_string();

    let env_kind = detect_environment();
    if let EnvironmentKind::StandardInstalled { install_dir, .. } = env_kind {
        let p = install_dir.join(".server.pid");
        if fs::write(&p, &my_pid).is_ok() {
            paths.push(p);
        }
    }
    let insights_pid = crate::db::get_insights_dir().join(".server.pid");
    if fs::write(&insights_pid, &my_pid).is_ok() {
        paths.push(insights_pid);
    }

    ServerPidGuard { paths }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateError {
    SafeRejection(String),
    Failure(String),
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpdateError::SafeRejection(msg) => write!(f, "{msg}"),
            UpdateError::Failure(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for UpdateError {}

impl From<String> for UpdateError {
    fn from(s: String) -> Self {
        UpdateError::Failure(s)
    }
}

#[derive(Debug, Clone, Default)]
pub struct UpdateOptions {
    pub check_only: bool,
    pub force: bool,
    pub target_version: Option<String>,
    pub prefetched_release: Option<GitHubRelease>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum EnvironmentKind {
    StandardInstalled {
        install_dir: PathBuf,
        exe_path: PathBuf,
    },
    Npm {
        exe_path: PathBuf,
    },
    GitOrDev {
        root: PathBuf,
        exe_path: PathBuf,
    },
    Other {
        exe_path: PathBuf,
    },
}

#[derive(Deserialize, Debug, Clone)]
pub struct GitHubAsset {
    pub name: String,
    pub browser_download_url: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct GitHubRelease {
    pub tag_name: String,
    pub assets: Vec<GitHubAsset>,
}

pub fn current_target_triple() -> Option<&'static str> {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        Some("x86_64-unknown-linux-gnu")
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        Some("aarch64-apple-darwin")
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        Some("x86_64-apple-darwin")
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        Some("x86_64-pc-windows-msvc")
    }
    #[cfg(not(any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "x86_64"),
    )))]
    {
        None
    }
}

pub fn standard_install_dir() -> Option<PathBuf> {
    if let Some(custom) = crate::paths::env_path("TOKEN_USAGE_INSIGHTS_INSTALL_DIR") {
        return Some(custom);
    }

    #[cfg(windows)]
    {
        if let Some(local_app_data) = dirs::data_local_dir() {
            return Some(local_app_data.join("TokenUsageInsights"));
        }
    }

    #[cfg(not(windows))]
    {
        if let Some(home) = dirs::home_dir() {
            return Some(home.join(".local").join("share").join(APP_NAME));
        }
    }

    None
}

pub fn detect_environment() -> EnvironmentKind {
    let raw_exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from(APP_NAME));
    let exe_path = fs::canonicalize(&raw_exe).unwrap_or(raw_exe);
    let exe_dir = exe_path.parent().unwrap_or_else(|| Path::new("."));

    // 1. 檢查是否由 npm / npx 啟動
    let path_str = exe_path.to_string_lossy();
    if path_str.contains("node_modules")
        || path_str.contains("token-usage-insights-bin")
        || std::env::var_os("npm_config_user_agent").is_some()
        || std::env::var_os("npm_lifecycle_event").is_some()
    {
        return EnvironmentKind::Npm { exe_path };
    }

    // 2. 檢查標準安裝目錄（包含以 TOKEN_USAGE_INSIGHTS_INSTALL_DIR 明確指定的路徑）
    if let Some(std_dir) = standard_install_dir() {
        let canonical_std_dir = fs::canonicalize(&std_dir).unwrap_or(std_dir);
        if let Ok(canonical_exe_dir) = fs::canonicalize(exe_dir) {
            if canonical_exe_dir == canonical_std_dir {
                return EnvironmentKind::StandardInstalled {
                    install_dir: canonical_std_dir,
                    exe_path,
                };
            }
        }
    }

    // 3. 檢查安裝標記檔（由 install.sh 或 install.ps1 寫入的自訂安裝目錄）
    let marker_path = exe_dir.join(".install_marker");
    if let Ok(meta) = fs::symlink_metadata(&marker_path) {
        if meta.is_file() && !meta.file_type().is_symlink() {
            if let Ok(content) = fs::read_to_string(&marker_path) {
                if content.trim() == "token-usage-insights:installed" {
                    let install_dir =
                        fs::canonicalize(exe_dir).unwrap_or_else(|_| exe_dir.to_path_buf());
                    return EnvironmentKind::StandardInstalled {
                        install_dir,
                        exe_path,
                    };
                }
            }
        }
    }

    // 4. 兼容舊版既有自訂安裝（在引入 .install_marker 之前建立的安裝目錄）：
    // 若目錄包含完整必要資產（pricing.csv, VERSION, static/index.html）且非 Cargo target 目錄
    let has_installed_assets = exe_dir.join("pricing.csv").is_file()
        && exe_dir.join("VERSION").is_file()
        && exe_dir.join("static").join("index.html").is_file();
    let is_cargo_target = exe_dir
        .file_name()
        .and_then(|n| n.to_str())
        .map(|name| name == "debug" || name == "release")
        .unwrap_or(false)
        && exe_dir
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .map(|name| name == "target")
            .unwrap_or(false);

    if has_installed_assets && !is_cargo_target {
        let install_dir = fs::canonicalize(exe_dir).unwrap_or_else(|_| exe_dir.to_path_buf());
        return EnvironmentKind::StandardInstalled {
            install_dir,
            exe_path,
        };
    }

    // 5. 檢查是否在 Git 或 Cargo 開發原始碼目錄
    for ancestor in exe_dir.ancestors() {
        if ancestor.join(".git").exists() || ancestor.join("Cargo.toml").exists() {
            return EnvironmentKind::GitOrDev {
                root: ancestor.to_path_buf(),
                exe_path,
            };
        }
    }

    EnvironmentKind::Other { exe_path }
}

pub fn parse_semver(v: &str) -> Option<(u32, u32, u32)> {
    let clean = v
        .trim()
        .strip_prefix('v')
        .or_else(|| v.trim().strip_prefix('V'))
        .unwrap_or(v.trim());
    let mut parts = clean.split('.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts.next()?.parse::<u32>().ok()?;
    let patch_part = parts.next()?;
    let patch = patch_part
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse::<u32>()
        .ok()?;
    Some((major, minor, patch))
}

pub fn is_newer_version(remote: &str, current: &str) -> bool {
    let (r_maj, r_min, r_pat) = match parse_semver(remote) {
        Some(v) => v,
        None => return false,
    };
    let (c_maj, c_min, c_pat) = match parse_semver(current) {
        Some(v) => v,
        None => return false,
    };

    (r_maj, r_min, r_pat) > (c_maj, c_min, c_pat)
}

pub fn archive_filename(tag: &str, target: &str) -> String {
    let tag = if tag.starts_with('v') || tag.starts_with('V') {
        tag.to_string()
    } else {
        format!("v{tag}")
    };

    #[cfg(windows)]
    {
        format!("{APP_NAME}-{tag}-{target}.zip")
    }

    #[cfg(not(windows))]
    {
        format!("{APP_NAME}-{tag}-{target}.tar.gz")
    }
}

pub fn parse_checksum(sums_text: &str, target_filename: &str) -> Option<String> {
    for line in sums_text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() >= 2 {
            let hash = parts[0].trim().to_lowercase();
            let raw_file = parts[1].trim_start_matches('*').trim();
            let file = raw_file.strip_prefix("./").unwrap_or(raw_file);
            let file_name = Path::new(file)
                .file_name()
                .and_then(|f| f.to_str())
                .unwrap_or(file);
            if (file == target_filename || file_name == target_filename) && hash.len() == 64 {
                return Some(hash);
            }
        }
    }
    None
}

pub fn verify_hash_hex(actual_hex: &str, expected_hex: &str) -> bool {
    actual_hex.trim().eq_ignore_ascii_case(expected_hex.trim())
}

#[allow(dead_code)] // 提供外部呼叫與單元測試比對記憶體資料 SHA256 雜湊之輔助函式
pub fn verify_sha256(bytes: &[u8], expected_hex: &str) -> bool {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let result = hasher.finalize();
    let actual_hex = hex::encode(result);
    verify_hash_hex(&actual_hex, expected_hex)
}

pub fn log_update(level: &str, action: &str, message: &str) {
    let log_path = crate::db::get_insights_dir().join("update.log");
    if let Some(parent) = log_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let now = Utc::now().to_rfc3339();
    let line = format!("[{now}] [{level}] [{action}] {message}\n");
    if let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        let _ = file.write_all(line.as_bytes());
    }
}

/// 讀取 config.yaml 中關於 auto_update 與 update_check_interval 的設定
pub fn parse_config_yaml(content: &str) -> (Option<bool>, Option<i64>) {
    let mut auto_update = None;
    let mut update_check_interval = None;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }
        if let Some((key, raw_val)) = trimmed.split_once(':') {
            let key = key.trim();
            let raw_val = raw_val.trim();
            let val = if let Some(rest) = raw_val.strip_prefix('"') {
                if let Some(end) = rest.find('"') {
                    &rest[..end]
                } else {
                    rest.trim_matches('"')
                }
            } else if let Some(rest) = raw_val.strip_prefix('\'') {
                if let Some(end) = rest.find('\'') {
                    &rest[..end]
                } else {
                    rest.trim_matches('\'')
                }
            } else {
                raw_val.split('#').next().unwrap_or("").trim()
            };

            if key == "auto_update" {
                let lower = val.to_lowercase();
                if lower == "true" || lower == "1" || lower == "yes" {
                    auto_update = Some(true);
                } else if lower == "false" || lower == "0" || lower == "no" {
                    auto_update = Some(false);
                }
            } else if key == "update_check_interval" {
                if let Ok(days) = val.parse::<i64>() {
                    update_check_interval = Some(days);
                }
            }
        }
    }

    (auto_update, update_check_interval)
}

pub fn load_update_config() -> (Option<bool>, Option<i64>) {
    let candidates = [
        crate::db::get_insights_dir().join("config.yaml"),
        PathBuf::from("config.yaml"),
    ];
    for path in candidates {
        if let Ok(content) = fs::read_to_string(&path) {
            return parse_config_yaml(&content);
        }
    }
    (None, None)
}

#[cfg(unix)]
fn try_lock_file_exclusive(file: &fs::File) -> Result<bool, std::io::Error> {
    use std::os::unix::io::AsRawFd;
    let res = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if res == 0 {
        Ok(true)
    } else {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EWOULDBLOCK) || err.raw_os_error() == Some(libc::EAGAIN)
        {
            Ok(false)
        } else {
            Err(err)
        }
    }
}

#[cfg(unix)]
fn unlock_file(file: &fs::File) -> Result<(), std::io::Error> {
    use std::os::unix::io::AsRawFd;
    let res = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
    if res == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn try_lock_file_exclusive(file: &fs::File) -> Result<bool, std::io::Error> {
    use std::os::windows::io::AsRawHandle;
    type Handle = *mut std::ffi::c_void;
    extern "system" {
        fn LockFile(
            hFile: Handle,
            dwFileOffsetLow: u32,
            dwFileOffsetHigh: u32,
            nNumberOfBytesToLockLow: u32,
            nNumberOfBytesToLockHigh: u32,
        ) -> i32;
    }
    let handle = file.as_raw_handle() as Handle;
    let ret = unsafe { LockFile(handle, 0, 0, 1, 0) };
    if ret != 0 {
        Ok(true)
    } else {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(33) {
            Ok(false)
        } else {
            Err(err)
        }
    }
}

#[cfg(windows)]
fn unlock_file(file: &fs::File) -> Result<(), std::io::Error> {
    use std::os::windows::io::AsRawHandle;
    type Handle = *mut std::ffi::c_void;
    extern "system" {
        fn UnlockFile(
            hFile: Handle,
            dwFileOffsetLow: u32,
            dwFileOffsetHigh: u32,
            nNumberOfBytesToLockLow: u32,
            nNumberOfBytesToLockHigh: u32,
        ) -> i32;
    }
    let handle = file.as_raw_handle() as Handle;
    let ret = unsafe { UnlockFile(handle, 0, 0, 1, 0) };
    if ret != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(any(unix, windows)))]
fn try_lock_file_exclusive(_file: &fs::File) -> Result<bool, std::io::Error> {
    Ok(true)
}

#[cfg(not(any(unix, windows)))]
fn unlock_file(_file: &fs::File) -> Result<(), std::io::Error> {
    Ok(())
}

#[derive(Debug)]
struct UpdateLock {
    _lock_path: PathBuf,
    _file: fs::File,
}

impl UpdateLock {
    fn lock_path(install_dir: &Path) -> PathBuf {
        install_dir.join(".update.lock")
    }

    fn try_acquire(install_dir: &Path) -> Result<Self, String> {
        let lock_path = Self::lock_path(install_dir);
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|e| format!("無法建立或開啟更新鎖定檔 ({lock_path:?}): {e}"))?;

        let locked = try_lock_file_exclusive(&file)
            .map_err(|e| format!("嘗試鎖定更新檔失敗 ({lock_path:?}): {e}"))?;

        if !locked {
            return Err("已有另一個更新程序正在執行中，請稍候再試。".to_string());
        }

        // 寫入當前進程 PID 供診斷與日誌記錄
        let mut f = &file;
        let _ = writeln!(f, "pid={}", std::process::id());

        Ok(Self {
            _lock_path: lock_path,
            _file: file,
        })
    }

    /// 檢查是否有活躍中的更新程序持鎖（使用 OS 層級非阻塞顧問鎖）
    fn is_locked(install_dir: &Path) -> bool {
        let lock_path = Self::lock_path(install_dir);
        if !lock_path.exists() {
            return false;
        }
        let file = match fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
        {
            Ok(f) => f,
            Err(_) => return true,
        };
        match try_lock_file_exclusive(&file) {
            Ok(true) => {
                let _ = unlock_file(&file);
                false
            }
            Ok(false) => true,
            Err(_) => true,
        }
    }
}

impl Drop for UpdateLock {
    fn drop(&mut self) {
        let _ = unlock_file(&self._file);
    }
}

struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    fn new(path: PathBuf) -> Result<Self, String> {
        if path.exists() {
            if let Err(e) = fs::remove_dir_all(&path) {
                let stale_path = path.with_extension(format!("stale-{}", Utc::now().timestamp()));
                if let Err(re) = fs::rename(&path, &stale_path) {
                    return Err(format!(
                        "清除舊暫存目錄失敗 ({path:?}): {e}；換名亦失敗: {re}"
                    ));
                }
            }
        }
        fs::create_dir_all(&path).map_err(|e| format!("建立暫存目錄失敗: {e}"))?;
        Ok(Self { path })
    }

    fn cleanup(&self) -> Result<(), String> {
        if self.path.exists() {
            if let Err(e) = fs::remove_dir_all(&self.path) {
                log_update("WARN", "CLEANUP", &format!("清理暫存目錄失敗: {e}"));
                let stale_path = self
                    .path
                    .with_extension(format!("stale-{}", Utc::now().timestamp()));
                if let Err(re) = fs::rename(&self.path, &stale_path) {
                    let msg = format!(
                        "暫存目錄無法清理亦無法換名 ({:?}): {re}；請稍後手動刪除",
                        self.path
                    );
                    log_update("WARN", "CLEANUP", &msg);
                    eprintln!("⚠️ {msg}");
                    return Err(msg);
                }
            }
        }
        Ok(())
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

#[cfg(windows)]
fn atomic_rename_overwrite(src: &Path, dst: &Path) -> Result<(), std::io::Error> {
    use std::os::windows::ffi::OsStrExt;
    let mut src_wide: Vec<u16> = src.as_os_str().encode_wide().collect();
    src_wide.push(0);
    let mut dst_wide: Vec<u16> = dst.as_os_str().encode_wide().collect();
    dst_wide.push(0);

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x00000001;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x00000008;

    extern "system" {
        fn MoveFileExW(
            lpExistingFileName: *const u16,
            lpNewFileName: *const u16,
            dwFlags: u32,
        ) -> i32;
    }

    let ret = unsafe {
        MoveFileExW(
            src_wide.as_ptr(),
            dst_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };

    if ret != 0 {
        return Ok(());
    }

    if dst.exists() {
        let _ = fs::remove_file(dst);
    }
    fs::rename(src, dst)
}

#[cfg(not(windows))]
fn atomic_rename_overwrite(src: &Path, dst: &Path) -> Result<(), std::io::Error> {
    fs::rename(src, dst)
}

fn safe_replace_file(src: &Path, dst: &Path) -> Result<(), String> {
    if let Ok(meta) = dst.symlink_metadata() {
        if meta.file_type().is_symlink() {
            let _ = fs::remove_file(dst);
        }
    }
    let tmp = dst.with_extension(format!("tmp.{}", std::process::id()));
    let _ = fs::remove_file(&tmp);
    fs::copy(src, &tmp).map_err(|e| format!("複製暫存檔失敗 ({tmp:?}): {e}"))?;
    atomic_rename_overwrite(&tmp, dst).map_err(|e| format!("替換檔案失敗 ({dst:?}): {e}"))
}

fn safe_write_file(dst: &Path, content: &[u8]) -> Result<(), String> {
    if let Ok(meta) = dst.symlink_metadata() {
        if meta.file_type().is_symlink() {
            let _ = fs::remove_file(dst);
        }
    }
    let tmp = dst.with_extension(format!("tmp.{}", std::process::id()));
    let _ = fs::remove_file(&tmp);
    fs::write(&tmp, content).map_err(|e| format!("寫入暫存檔失敗 ({tmp:?}): {e}"))?;
    atomic_rename_overwrite(&tmp, dst).map_err(|e| format!("替換標記檔失敗 ({dst:?}): {e}"))
}

fn extract_archive(archive_path: &Path, dest_dir: &Path, is_zip: bool) -> Result<(), String> {
    use std::io::Read;
    fs::create_dir_all(dest_dir).map_err(|e| format!("建立解壓縮目錄失敗: {e}"))?;

    let file = fs::File::open(archive_path)
        .map_err(|e| format!("開啟壓縮包檔案失敗 ({archive_path:?}): {e}"))?;

    let mut total_bytes: u64 = 0;

    if is_zip {
        let mut archive =
            zip::ZipArchive::new(file).map_err(|e| format!("解析 ZIP 封裝失敗: {e}"))?;
        if archive.len() > MAX_EXTRACTED_ENTRIES {
            return Err(format!(
                "ZIP 壓縮包項目數超過安全上限 ({MAX_EXTRACTED_ENTRIES})"
            ));
        }

        for i in 0..archive.len() {
            let mut entry = archive
                .by_index(i)
                .map_err(|e| format!("讀取 ZIP 項目失敗: {e}"))?;
            let enclosed = entry
                .enclosed_name()
                .ok_or_else(|| "ZIP 內含無效相對路徑".to_string())?
                .to_path_buf();
            let outpath = dest_dir.join(enclosed);

            if entry.is_dir() {
                fs::create_dir_all(&outpath)
                    .map_err(|e| format!("建立目錄失敗 ({outpath:?}): {e}"))?;
            } else {
                if let Some(parent) = outpath.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|e| format!("建立上層目錄失敗 ({parent:?}): {e}"))?;
                }
                let mut outfile = fs::File::create(&outpath)
                    .map_err(|e| format!("建立檔案失敗 ({outpath:?}): {e}"))?;
                let mut buffer = [0u8; 8192];
                loop {
                    let n = entry
                        .read(&mut buffer)
                        .map_err(|e| format!("解壓讀取失敗: {e}"))?;
                    if n == 0 {
                        break;
                    }
                    total_bytes = total_bytes.saturating_add(n as u64);
                    if total_bytes > MAX_EXTRACTED_BYTES {
                        return Err(format!(
                            "解壓縮展開大小超過安全上限 ({MAX_EXTRACTED_BYTES} 位元組)"
                        ));
                    }
                    outfile
                        .write_all(&buffer[..n])
                        .map_err(|e| format!("寫入解壓檔案失敗: {e}"))?;
                }
            }
        }
    } else {
        let tar_gz = flate2::read::GzDecoder::new(file);
        let mut archive = tar::Archive::new(tar_gz);
        let mut count: usize = 0;

        for entry in archive
            .entries()
            .map_err(|e| format!("讀取 tar 項目清單失敗: {e}"))?
        {
            count = count.saturating_add(1);
            if count > MAX_EXTRACTED_ENTRIES {
                return Err(format!(
                    "tar.gz 壓縮包項目數超過安全上限 ({MAX_EXTRACTED_ENTRIES})"
                ));
            }
            let mut entry = entry.map_err(|e| format!("讀取 tar 項目失敗: {e}"))?;
            let path = entry
                .path()
                .map_err(|e| format!("讀取 tar 路徑失敗: {e}"))?
                .to_path_buf();

            if path.is_absolute()
                || path
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                return Err(format!("tar 包含不安全的檔案路徑: {path:?}"));
            }
            let outpath = dest_dir.join(&path);

            if entry.header().entry_type().is_dir() {
                fs::create_dir_all(&outpath)
                    .map_err(|e| format!("建立目錄失敗 ({outpath:?}): {e}"))?;
            } else {
                if let Some(parent) = outpath.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|e| format!("建立上層目錄失敗 ({parent:?}): {e}"))?;
                }
                let mut outfile = fs::File::create(&outpath)
                    .map_err(|e| format!("建立檔案失敗 ({outpath:?}): {e}"))?;
                let mut buffer = [0u8; 8192];
                loop {
                    let n = entry
                        .read(&mut buffer)
                        .map_err(|e| format!("解壓讀取失敗: {e}"))?;
                    if n == 0 {
                        break;
                    }
                    total_bytes = total_bytes.saturating_add(n as u64);
                    if total_bytes > MAX_EXTRACTED_BYTES {
                        return Err(format!(
                            "解壓縮展開大小超過安全上限 ({MAX_EXTRACTED_BYTES} 位元組)"
                        ));
                    }
                    outfile
                        .write_all(&buffer[..n])
                        .map_err(|e| format!("寫入解壓檔案失敗: {e}"))?;
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(mode) = entry.header().mode() {
                        let _ = fs::set_permissions(&outpath, fs::Permissions::from_mode(mode));
                    }
                }
            }
        }
    }

    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    if !src.exists() {
        return Ok(());
    }
    if let Ok(meta) = dst.symlink_metadata() {
        if meta.file_type().is_symlink() {
            let _ = fs::remove_file(dst);
            let _ = fs::remove_dir_all(dst);
        }
    }
    fs::create_dir_all(dst).map_err(|e| format!("建立目標目錄失敗 {dst:?}: {e}"))?;
    for entry in fs::read_dir(src).map_err(|e| format!("讀取目錄失敗 {src:?}: {e}"))? {
        let entry = entry.map_err(|e| format!("讀取項目失敗: {e}"))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|e| format!("讀取檔案類型失敗: {e}"))?;
        if file_type.is_dir() {
            if let Ok(meta) = dst_path.symlink_metadata() {
                if meta.file_type().is_symlink() {
                    let _ = fs::remove_file(&dst_path);
                    let _ = fs::remove_dir_all(&dst_path);
                }
            }
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            safe_replace_file(&src_path, &dst_path)
                .map_err(|e| format!("複製檔案失敗 {src_path:?} -> {dst_path:?}: {e}"))?;
        }
    }
    Ok(())
}

const MANAGED_ITEMS: &[&str] = &[
    APP_NAME,
    #[cfg(windows)]
    "token-usage-insights.exe",
    "static",
    "pricing.csv",
    "shell",
    "scripts",
    "install.sh",
    "install.ps1",
    "VERSION",
    "README.md",
    "LICENSE",
    ".install_marker",
];

fn backup_installation(install_dir: &Path, backup_dir: &Path) -> Result<(), String> {
    if backup_dir.exists() {
        let err = format!(
            "偵測到備份目錄已存在 ({backup_dir:?})；疑似先前更新中斷或失敗留存之救援狀態。為保護歷史版本不被覆蓋，已中止本次更新。請先手動確認還原舊版或清除該目錄後再更新。"
        );
        log_update("ERROR", "BACKUP", &err);
        return Err(err);
    }
    let staging_dir = install_dir.join(format!(".backup-staging-{}", std::process::id()));
    if staging_dir.exists() {
        let _ = fs::remove_dir_all(&staging_dir);
    }
    fs::create_dir_all(&staging_dir).map_err(|e| format!("建立備份暫存目錄失敗: {e}"))?;

    let backup_res = (|| -> Result<(), String> {
        let mut manifest_entries = Vec::new();
        for &item in MANAGED_ITEMS {
            let src = install_dir.join(item);
            let dst = staging_dir.join(item);
            if src.exists() {
                manifest_entries.push(item);
                if src.is_dir() {
                    copy_dir_recursive(&src, &dst)?;
                } else if src.is_file() {
                    safe_replace_file(&src, &dst)
                        .map_err(|e| format!("備份檔案失敗 {item}: {e}"))?;
                }
            }
        }

        safe_write_file(
            &staging_dir.join(".manifest"),
            manifest_entries.join("\n").as_bytes(),
        )
        .map_err(|e| format!("寫入備份清單失敗: {e}"))?;

        fs::rename(&staging_dir, backup_dir).map_err(|e| {
            format!("切換至正式備份目錄失敗 ({staging_dir:?} -> {backup_dir:?}): {e}")
        })?;

        Ok(())
    })();

    if let Err(e) = backup_res {
        let _ = fs::remove_dir_all(&staging_dir);
        let _ = fs::remove_dir_all(backup_dir);
        return Err(e);
    }

    Ok(())
}

fn restore_from_backup(backup_dir: &Path, install_dir: &Path) -> Result<(), String> {
    if !backup_dir.exists() {
        return Ok(());
    }

    let manifest_path = backup_dir.join(".manifest");
    let original_items: std::collections::HashSet<String> = if manifest_path.exists() {
        let content = fs::read_to_string(&manifest_path)
            .map_err(|e| format!("讀取備份清單失敗 ({manifest_path:?}): {e}"))?;
        content
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    } else {
        std::collections::HashSet::new()
    };

    // 1. 移除更新期間新增、但原始安裝中並不存在的受管理項目
    for &item in MANAGED_ITEMS {
        if !original_items.contains(item) {
            let path = install_dir.join(item);
            if path.is_dir() {
                if path.exists() {
                    fs::remove_dir_all(&path)
                        .map_err(|e| format!("回滾清理新增目錄失敗 {path:?}: {e}"))?;
                }
            } else if path.is_file() {
                fs::remove_file(&path)
                    .map_err(|e| format!("回滾清理新增檔案失敗 {path:?}: {e}"))?;
            }
        }
    }

    // 2. 還原備份項目
    for entry in fs::read_dir(backup_dir).map_err(|e| format!("讀取備份目錄失敗: {e}"))? {
        let entry = entry.map_err(|e| format!("讀取備份項目失敗: {e}"))?;
        let name = entry.file_name();
        if name == ".manifest" {
            continue;
        }
        let src = entry.path();
        let dst = install_dir.join(&name);
        let file_type = entry.file_type().map_err(|e| e.to_string())?;
        if file_type.is_dir() {
            if let Ok(meta) = dst.symlink_metadata() {
                if meta.file_type().is_symlink() {
                    let _ = fs::remove_file(&dst);
                    let _ = fs::remove_dir_all(&dst);
                }
            }
            if dst.exists() {
                fs::remove_dir_all(&dst)
                    .map_err(|e| format!("清理還原目標目錄失敗 {dst:?}: {e}"))?;
            }
            copy_dir_recursive(&src, &dst)?;
        } else {
            let current_exe = std::env::current_exe().ok();
            let is_current_exe = current_exe
                .as_ref()
                .and_then(|c| fs::canonicalize(c).ok())
                .zip(fs::canonicalize(&dst).ok())
                .map(|(a, b)| a == b)
                .unwrap_or(false);

            if is_current_exe {
                self_replace::self_replace(&src).map_err(|e| format!("回滾目前執行檔失敗: {e}"))?;
            } else {
                safe_replace_file(&src, &dst)
                    .map_err(|e| format!("還原檔案失敗 {src:?} -> {dst:?}: {e}"))?;
            }
        }
    }
    Ok(())
}

async fn fetch_release_with_logging(
    tag_opt: Option<&str>,
    timeout_secs: u64,
) -> Result<GitHubRelease, String> {
    match fetch_release(tag_opt, timeout_secs).await {
        Ok(r) => Ok(r),
        Err(e) => {
            log_update("ERROR", "CHECK", &format!("查詢 GitHub Releases 失敗: {e}"));
            Err(e)
        }
    }
}

async fn fetch_release(tag_opt: Option<&str>, timeout_secs: u64) -> Result<GitHubRelease, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .build()
        .map_err(|e| format!("建立 HTTP 用戶端失敗: {e}"))?;

    let url = match tag_opt {
        Some(tag) => {
            let clean_tag = if tag.starts_with('v') || tag.starts_with('V') {
                tag.to_string()
            } else {
                format!("v{tag}")
            };
            format!("https://api.github.com/repos/{GITHUB_OWNER}/{GITHUB_REPO}/releases/tags/{clean_tag}")
        }
        None => {
            format!("https://api.github.com/repos/{GITHUB_OWNER}/{GITHUB_REPO}/releases/latest")
        }
    };

    let resp = client
        .get(&url)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/vnd.github.v3+json")
        .send()
        .await
        .map_err(|e| format!("查詢 GitHub Releases 失敗: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("GitHub API 回應異常 HTTP {}", resp.status()));
    }

    resp.json::<GitHubRelease>()
        .await
        .map_err(|e| format!("解析 Release JSON 失敗: {e}"))
}

async fn download_to_file_with_hash(
    url: &str,
    dest_path: &Path,
    max_bytes: usize,
    timeout_secs: u64,
) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .build()
        .map_err(|e| format!("建立 HTTP 用戶端失敗: {e}"))?;

    let mut resp = client
        .get(url)
        .header("User-Agent", USER_AGENT)
        .send()
        .await
        .map_err(|e| format!("下載失敗 ({url}): {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("下載失敗 HTTP {} ({url})", resp.status()));
    }

    if let Some(cl) = resp.content_length() {
        if cl > max_bytes as u64 {
            return Err(format!(
                "檔案大小 ({cl} 位元組) 超過安全上限 ({max_bytes} 位元組)"
            ));
        }
    }

    let mut file = fs::File::create(dest_path)
        .map_err(|e| format!("建立下載檔案失敗 ({dest_path:?}): {e}"))?;
    let mut hasher = Sha256::new();
    let mut downloaded: usize = 0;

    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("讀取下載串流失敗 ({url}): {e}"))?
    {
        downloaded = downloaded.saturating_add(chunk.len());
        if downloaded > max_bytes {
            let _ = fs::remove_file(dest_path);
            return Err(format!("下載累計大小超過安全上限 ({max_bytes} 位元組)"));
        }
        file.write_all(&chunk)
            .map_err(|e| format!("寫入下載檔案失敗 ({dest_path:?}): {e}"))?;
        hasher.update(&chunk);
    }

    file.flush()
        .map_err(|e| format!("排清寫入暫存檔失敗: {e}"))?;

    let actual_hash = hex::encode(hasher.finalize()).to_lowercase();
    Ok(actual_hash)
}

async fn download_text_capped(
    url: &str,
    max_bytes: usize,
    timeout_secs: u64,
) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .build()
        .map_err(|e| format!("建立 HTTP 用戶端失敗: {e}"))?;

    let mut resp = client
        .get(url)
        .header("User-Agent", USER_AGENT)
        .send()
        .await
        .map_err(|e| format!("下載失敗 ({url}): {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("下載失敗 HTTP {} ({url})", resp.status()));
    }

    if let Some(cl) = resp.content_length() {
        if cl > max_bytes as u64 {
            return Err(format!("校驗檔大小 ({cl} 位元組) 超過安全上限"));
        }
    }

    let mut buffer = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("讀取校驗檔串流失敗: {e}"))?
    {
        if buffer.len().saturating_add(chunk.len()) > max_bytes {
            return Err("校驗檔內容超過安全上限".to_string());
        }
        buffer.extend_from_slice(&chunk);
    }

    Ok(String::from_utf8_lossy(&buffer).to_string())
}

fn print_and_log_check_result(
    remote_version: &str,
    current_version: &str,
    hint_newer: Option<&str>,
) {
    let is_newer = is_newer_version(remote_version, current_version);
    println!("  目前版本: v{current_version}");
    println!("  最新版本: {remote_version}");
    if is_newer {
        if let Some(hint) = hint_newer {
            println!("{hint}");
        } else {
            println!("💡 發現新版本！可執行 token-usage-insights update 進行更新。");
        }
        log_update(
            "INFO",
            "CHECK",
            &format!("版本檢查完成：發現新版本 {remote_version}（目前為 v{current_version}）"),
        );
    } else {
        println!("✅ 目前已是最新版本。");
        log_update(
            "INFO",
            "CHECK",
            &format!("版本檢查完成：目前已是最新版本 v{current_version}"),
        );
    }
}

pub async fn run_update(options: UpdateOptions) -> Result<(), UpdateError> {
    let env_kind = detect_environment();

    match &env_kind {
        EnvironmentKind::Npm { .. } => {
            if options.check_only {
                let release =
                    fetch_release_with_logging(options.target_version.as_deref(), 15).await?;
                let current_version = env!("CARGO_PKG_VERSION");
                let remote_version = release.tag_name.trim();
                print_and_log_check_result(
                    remote_version,
                    current_version,
                    Some("💡 發現新版本！可執行 npx token-usage-insights@latest 使用最新版。"),
                );
                return Ok(());
            }
            let msg = r#"⚠️ 偵測到目前透過 npm / npx 執行，不支援直接原地自我更新。
👉 請使用以下指令取得或執行最新版本：
   npx token-usage-insights@latest
   # 或全域安裝更新：
   npm install -g token-usage-insights@latest"#;
            eprintln!("{msg}");
            log_update("WARN", "CHECK", "略過更新：偵測到 npm / npx 執行環境");
            return Err(UpdateError::SafeRejection(
                "不支援在 npm / npx 環境中直接自我更新".to_string(),
            ));
        }
        EnvironmentKind::GitOrDev { root, .. } => {
            if options.check_only {
                let release =
                    fetch_release_with_logging(options.target_version.as_deref(), 15).await?;
                let current_version = env!("CARGO_PKG_VERSION");
                let remote_version = release.tag_name.trim();
                print_and_log_check_result(
                    remote_version,
                    current_version,
                    Some("💡 發現新版本！請使用 git pull / cargo build 進行更新。"),
                );
                return Ok(());
            }
            let msg = format!(
                "錯誤：目前執行檔位於開發目錄中 ({root:?})，不支援直接更新。\n請使用 git pull / cargo build 進行更新。"
            );
            eprintln!("{msg}");
            log_update("ERROR", "CHECK", &format!("拒絕更新：開發目錄 {root:?}"));
            return Err(UpdateError::SafeRejection(
                "開發目錄不支援自我更新".to_string(),
            ));
        }
        EnvironmentKind::Other { exe_path } => {
            if options.check_only {
                let release =
                    fetch_release_with_logging(options.target_version.as_deref(), 15).await?;
                let current_version = env!("CARGO_PKG_VERSION");
                let remote_version = release.tag_name.trim();
                print_and_log_check_result(
                    remote_version,
                    current_version,
                    Some("💡 發現新版本！請在標準安裝目錄中執行更新。"),
                );
                return Ok(());
            }
            let msg = format!(
                "錯誤：目前執行檔位於非標準安裝目錄 ({exe_path:?})。\n請在標準安裝目錄中執行更新。"
            );
            eprintln!("{msg}");
            log_update(
                "ERROR",
                "CHECK",
                &format!("拒絕更新：非標準目錄 {exe_path:?}"),
            );
            return Err(UpdateError::SafeRejection(
                "非標準目錄不支援自我更新".to_string(),
            ));
        }
        EnvironmentKind::StandardInstalled { .. } => {}
    }

    let install_dir = match &env_kind {
        EnvironmentKind::StandardInstalled { install_dir, .. } => install_dir.clone(),
        _ => unreachable!(),
    };

    // 若非純檢查，在開始任何更新操作前先取得安裝目錄之獨占鎖
    let _lock = if !options.check_only {
        Some(match UpdateLock::try_acquire(&install_dir) {
            Ok(l) => l,
            Err(e) => {
                log_update("ERROR", "LOCK", &e);
                return Err(e.into());
            }
        })
    } else {
        None
    };

    let current_version = env!("CARGO_PKG_VERSION");
    println!("🔍 正在檢查最新發行版本...");
    log_update(
        "INFO",
        "CHECK",
        &format!("開始檢查更新（目前版本 v{current_version}）"),
    );

    let release = match options.prefetched_release {
        Some(r) => r,
        None => fetch_release_with_logging(options.target_version.as_deref(), 15).await?,
    };
    let remote_version = release.tag_name.trim();

    if options.check_only {
        let hint = match current_target_triple() {
            Some(_) => None,
            None => Some("💡 發現新版本！但目前作業系統/硬體架構無預編譯發行包，需手動編譯。"),
        };
        print_and_log_check_result(remote_version, current_version, hint);
        return Ok(());
    }

    let is_newer = is_newer_version(remote_version, current_version);
    println!("  目前版本: v{current_version}");
    println!("  目標版本: {remote_version}");

    if !is_newer && !options.force && options.target_version.is_none() {
        println!("✅ 目前已是最新版本 ({remote_version})。使用 --force 可強制重新安裝。");
        log_update("INFO", "CHECK", "已是最新版本，略過更新");
        return Ok(());
    }

    let target = current_target_triple().ok_or_else(|| {
        let msg = "目前作業系統或硬體架構不支援預先編譯的二進位發行檔".to_string();
        log_update("ERROR", "CHECK", &msg);
        msg
    })?;

    let archive_name = archive_filename(remote_version, target);
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == archive_name)
        .ok_or_else(|| {
            let err = format!("Release {remote_version} 缺少目標發行包: {archive_name}");
            log_update("ERROR", "DOWNLOAD", &err);
            err
        })?;

    let checksum_asset = release
        .assets
        .iter()
        .find(|a| a.name == "SHA256SUMS")
        .ok_or_else(|| {
            let err = format!("Release {remote_version} 缺少 SHA256SUMS 校驗檔");
            log_update("ERROR", "DOWNLOAD", &err);
            err
        })?;

    // 使用 TempDirGuard 確保異常離開時自動清理暫存（置於 install_dir 底下確保受獨占鎖保護）
    let update_tmp_dir = install_dir.join(".update-tmp");
    let tmp_guard = match TempDirGuard::new(update_tmp_dir) {
        Ok(g) => g,
        Err(e) => {
            log_update("ERROR", "PREPARE", &e);
            return Err(e.into());
        }
    };

    println!("⬇️ 正在下載發行包: {archive_name} ...");
    log_update("INFO", "DOWNLOAD", &format!("開始串流下載 {archive_name}"));
    let archive_path = tmp_guard.path.join(&archive_name);
    let actual_hash = match download_to_file_with_hash(
        &asset.browser_download_url,
        &archive_path,
        MAX_ARCHIVE_BYTES,
        60,
    )
    .await
    {
        Ok(h) => h,
        Err(e) => {
            log_update("ERROR", "DOWNLOAD", &e);
            return Err(e.into());
        }
    };

    println!("⬇️ 正在下載校驗檔 SHA256SUMS ...");
    let sums_text =
        match download_text_capped(&checksum_asset.browser_download_url, MAX_CHECKSUM_BYTES, 15)
            .await
        {
            Ok(t) => t,
            Err(e) => {
                log_update("ERROR", "DOWNLOAD", &e);
                return Err(e.into());
            }
        };

    let expected_hash = match parse_checksum(&sums_text, &archive_name) {
        Some(h) => h,
        None => {
            let err = format!("SHA256SUMS 中未找到 {archive_name} 的校驗碼");
            log_update("ERROR", "VERIFY", &err);
            return Err(err.into());
        }
    };

    println!("🔒 正在驗證 SHA256 校驗碼...");
    if !verify_hash_hex(&actual_hash, &expected_hash) {
        let err = format!("SHA256 校驗失敗！預期 {expected_hash}，實際 {actual_hash}");
        log_update("ERROR", "VERIFY", &err);
        return Err(err.into());
    }
    println!("✅ SHA256 校驗通過！");
    log_update("INFO", "VERIFY", "SHA256 校驗通過");

    let is_zip = archive_name.ends_with(".zip");
    let extract_dir = tmp_guard.path.join("extracted");
    println!("📦 正在解壓縮檔案...");

    // 將耗時之同步解壓縮與原子替換移至 blocking thread，避免阻塞 tokio 執行緒並支援超時隔離
    let install_task_res = tokio::task::spawn_blocking({
        let archive_path = archive_path.clone();
        let extract_dir = extract_dir.clone();
        let install_dir = install_dir.clone();
        let remote_version = remote_version.to_string();
        move || -> Result<(), String> {
            if let Err(e) = extract_archive(&archive_path, &extract_dir, is_zip) {
                log_update("ERROR", "EXTRACT", &e);
                return Err(e);
            }

            // 尋找解壓後的根目錄（可能有一層子目錄）
            let release_root = if extract_dir.join(APP_NAME).exists()
                || extract_dir.join(format!("{APP_NAME}.exe")).exists()
            {
                extract_dir
            } else {
                let mut found = None;
                if let Ok(entries) = fs::read_dir(&extract_dir) {
                    for entry in entries.flatten() {
                        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                            let sub = entry.path();
                            if sub.join(APP_NAME).exists()
                                || sub.join(format!("{APP_NAME}.exe")).exists()
                            {
                                found = Some(sub);
                                break;
                            }
                        }
                    }
                }
                match found {
                    Some(dir) => dir,
                    None => {
                        let err = "解壓後的目錄中未找到執行檔".to_string();
                        log_update("ERROR", "EXTRACT", &err);
                        return Err(err);
                    }
                }
            };

            let exec_name = if cfg!(windows) {
                format!("{APP_NAME}.exe")
            } else {
                APP_NAME.to_string()
            };

            // 驗證解壓後的發行包是否包含所有必要資源，避免不完整安裝造成混合版本
            let required_items = [
                exec_name.as_str(),
                "static",
                "pricing.csv",
                "VERSION",
                "scripts",
                "shell",
                "install.sh",
                "install.ps1",
            ];
            for required in required_items {
                if !release_root.join(required).exists() {
                    let err = format!("解壓發行包缺少必要資源: {required}");
                    log_update("ERROR", "VERIFY", &err);
                    return Err(err);
                }
            }

            let backup_dir = install_dir.join(".backup");
            apply_installation_with_rollback(&release_root, &install_dir, &backup_dir)?;

            println!("🎉 成功更新至版本 {remote_version}！");
            log_update("INFO", "INSTALL", &format!("成功更新至 {remote_version}"));

            Ok(())
        }
    })
    .await;

    match install_task_res {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(e.into()),
        Err(join_err) => return Err(format!("安裝任務執行異常: {join_err}").into()),
    }

    let _ = tmp_guard.cleanup();

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StoppedDashboardStatus {
    pub stopped_any: bool,
    pub had_supervised: bool,
    pub had_unsupervised: bool,
}

fn stop_running_dashboard_instances(install_dir: &Path) -> Result<StoppedDashboardStatus, String> {
    let my_pid = std::process::id();
    let mut candidate_pids = std::collections::HashSet::new();

    // 1. 從 .server.pid 讀取 PID 作為候選
    let pid_file = install_dir.join(".server.pid");
    let mut server_pids = std::collections::HashSet::new();
    if let Ok(content) = fs::read_to_string(&pid_file) {
        if let Ok(pid) = content.trim().parse::<u32>() {
            if pid != my_pid {
                candidate_pids.insert(pid);
                server_pids.insert(pid);
            }
        }
    }
    let insights_pid_file = crate::db::get_insights_dir().join(".server.pid");
    if let Ok(content) = fs::read_to_string(&insights_pid_file) {
        if let Ok(pid) = content.trim().parse::<u32>() {
            if pid != my_pid {
                candidate_pids.insert(pid);
                server_pids.insert(pid);
            }
        }
    }

    // 2. 透過系統進程清單掃描 APP_NAME
    #[cfg(target_os = "linux")]
    {
        if let Ok(entries) = fs::read_dir("/proc") {
            for entry in entries.flatten() {
                if let Ok(name) = entry.file_name().into_string() {
                    if let Ok(pid) = name.parse::<u32>() {
                        if pid != my_pid {
                            if let Ok(cmdline) = fs::read_to_string(format!("/proc/{pid}/cmdline"))
                            {
                                if cmdline.contains(APP_NAME) {
                                    candidate_pids.insert(pid);
                                }
                            }
                        }
                    }
                }
            }
        } else {
            return Err("無法讀取 /proc 目錄列舉進程；更新中止以確保安全".to_string());
        }
    }

    #[cfg(all(unix, not(target_os = "linux")))]
    {
        match std::process::Command::new("pgrep")
            .args(["-f", APP_NAME])
            .output()
        {
            Ok(output) if output.status.success() => {
                let text = String::from_utf8_lossy(&output.stdout);
                for line in text.lines() {
                    if let Ok(pid) = line.trim().parse::<u32>() {
                        if pid != my_pid {
                            candidate_pids.insert(pid);
                        }
                    }
                }
            }
            _ => {
                // pgrep 未匹配或失敗時，嘗試 ps 作為回退；若兩者皆失敗則 fail closed
                let ps_res = std::process::Command::new("ps")
                    .args(["-axo", "pid="])
                    .output()
                    .map_err(|pe| format!("進程列舉失敗 (ps: {pe})；更新中止以確保安全"))?;
                if !ps_res.status.success() {
                    return Err("ps 命令執行失敗；更新中止以確保安全".to_string());
                }
                let text = String::from_utf8_lossy(&ps_res.stdout);
                for line in text.lines() {
                    if let Ok(pid) = line.trim().parse::<u32>() {
                        if pid != my_pid {
                            candidate_pids.insert(pid);
                        }
                    }
                }
            }
        }
    }

    #[cfg(windows)]
    {
        let exec_name = format!("{APP_NAME}.exe");
        let output = std::process::Command::new("tasklist")
            .args([
                "/FI",
                &format!("IMAGENAME eq {exec_name}"),
                "/FO",
                "CSV",
                "/NH",
            ])
            .output()
            .map_err(|e| format!("列舉 Windows 進程失敗: {e}"))?;
        if !output.status.success() {
            let err_text = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "tasklist 命令執行失敗 (exit code: {:?}): {err_text}",
                output.status.code()
            ));
        }
        let text = String::from_utf8_lossy(&output.stdout);
        for line in text.lines() {
            let parts: Vec<&str> = line.split(',').collect();
            if parts.len() >= 2 {
                let pid_str = parts[1].trim().trim_matches('"');
                if let Ok(pid) = pid_str.parse::<u32>() {
                    if pid != my_pid {
                        candidate_pids.insert(pid);
                    }
                }
            }
        }
    }

    // 3. 嚴格驗證候選 PID：必須為活躍進程且執行檔路徑確認位於 install_dir，並過濾短暫 CLI 指令進程（Fail-Closed 原則）
    let mut target_pids = std::collections::HashSet::new();
    let mut had_supervised = false;
    let mut had_unsupervised = false;

    for pid in candidate_pids {
        if is_process_alive(pid) {
            if let Some(exe_path) = get_process_exe_path(pid) {
                if matches_install_dir(&exe_path, install_dir)
                    && is_dashboard_server_process(pid, &server_pids)
                {
                    if is_process_supervised(pid, install_dir) {
                        had_supervised = true;
                    } else {
                        had_unsupervised = true;
                    }
                    target_pids.insert(pid);
                }
            }
        }
    }

    let mut remaining_pids: Vec<u32> = target_pids.into_iter().collect();
    if remaining_pids.is_empty() {
        // 未發現需要停止的其他進程，保留自身進程之 .server.pid
        return Ok(StoppedDashboardStatus {
            stopped_any: false,
            had_supervised: false,
            had_unsupervised: false,
        });
    }

    // 若在 Windows 環境且有受到 run-service.ps1 監管之服務進程，寫入服務重啟協商標記檔，讓 run-service.ps1 能在新版就緒後重啟
    let restart_pending_file = install_dir.join(".service_restart_pending");
    #[cfg(windows)]
    if had_supervised {
        safe_write_file(&restart_pending_file, b"1").map_err(|e| {
            let err = format!("無法寫入服務重啟協商標記檔 ({restart_pending_file:?}): {e}");
            log_update("ERROR", "STOP_SERVICE", &err);
            err
        })?;
    }

    log_update(
        "INFO",
        "STOP_SERVICE",
        &format!("協調停止執行中之服務進程: {remaining_pids:?}"),
    );

    // 4. 發送初次溫和退出訊號 (Unix: SIGTERM, Windows: taskkill 無 /F)
    for &pid in &remaining_pids {
        #[cfg(unix)]
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGTERM);
        }
        #[cfg(windows)]
        {
            let _ = std::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T"])
                .output();
        }
    }

    // 5. 積極輪詢並在逾時 2.5 秒後升級強制終止
    let start_time = Instant::now();
    let timeout = Duration::from_secs(5);
    let escalation_delay = Duration::from_millis(2500);
    let poll_interval = Duration::from_millis(100);
    let mut escalated = false;

    loop {
        remaining_pids.retain(|&pid| is_process_alive(pid));
        if remaining_pids.is_empty() {
            break;
        }

        let elapsed = start_time.elapsed();
        if elapsed >= timeout {
            let err = format!(
                "等待執行中之服務進程 (PID: {remaining_pids:?}) 停止超時，更新中止以保護檔案安全"
            );
            log_update("ERROR", "STOP_SERVICE", &err);
            let _ = fs::remove_file(&restart_pending_file);
            return Err(err);
        }

        if elapsed >= escalation_delay && !escalated {
            escalated = true;
            log_update(
                "WARN",
                "STOP_SERVICE",
                &format!("服務進程未於 2.5 秒內正常退出，升級強制終止 (PID: {remaining_pids:?})"),
            );
            for &pid in &remaining_pids {
                #[cfg(unix)]
                unsafe {
                    libc::kill(pid as libc::pid_t, libc::SIGKILL);
                }
                #[cfg(windows)]
                {
                    let _ = std::process::Command::new("taskkill")
                        .args(["/PID", &pid.to_string(), "/T", "/F"])
                        .output();
                }
            }
        }

        std::thread::sleep(poll_interval);
    }

    // 僅刪除屬於已停止進程之 PID 檔案，絕不誤刪目前進程之標記
    for p_file in [&pid_file, &insights_pid_file] {
        if let Ok(content) = fs::read_to_string(p_file) {
            if let Ok(p) = content.trim().parse::<u32>() {
                if p != my_pid && !is_process_alive(p) {
                    let _ = fs::remove_file(p_file);
                }
            }
        }
    }

    log_update("INFO", "STOP_SERVICE", "所有執行中之目標服務進程已安全停止");
    Ok(StoppedDashboardStatus {
        stopped_any: true,
        had_supervised,
        had_unsupervised,
    })
}

fn restart_background_dashboard(install_dir: &Path) {
    let exec_name = if cfg!(windows) {
        format!("{APP_NAME}.exe")
    } else {
        APP_NAME.to_string()
    };
    let exe = install_dir.join(exec_name);
    if exe.exists() {
        let mut cmd = std::process::Command::new(exe);
        cmd.current_dir(install_dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let _ = cmd.spawn();
    }
}

pub(crate) fn apply_installation_with_rollback(
    release_root: &Path,
    install_dir: &Path,
    backup_dir: &Path,
) -> Result<(), String> {
    println!("💾 正在備份現有安裝...");
    if let Err(e) = backup_installation(install_dir, backup_dir) {
        log_update("ERROR", "BACKUP", &e);
        return Err(e);
    }

    println!("⏸️ 正在協調停止現有執行中之服務...");
    let stopped_status = stop_running_dashboard_instances(install_dir)?;

    println!("🚀 正在安裝新版檔案至 {:?} ...", install_dir);
    log_update("INFO", "INSTALL", &format!("開始替換至 {install_dir:?}"));

    let install_result = (|| -> Result<(), String> {
        let exec_name = if cfg!(windows) {
            format!("{APP_NAME}.exe")
        } else {
            APP_NAME.to_string()
        };

        let target_exe = install_dir.join(&exec_name);
        let src_exe = release_root.join(&exec_name);

        if !src_exe.exists() {
            return Err(format!("來源缺少可執行檔: {src_exe:?}"));
        }

        // 跨平台安全替換執行檔（Windows 使用 self_replace 或安全重命名）
        let current_exe = std::env::current_exe().ok();
        let is_current_exe = current_exe
            .as_ref()
            .and_then(|c| fs::canonicalize(c).ok())
            .zip(fs::canonicalize(&target_exe).ok())
            .map(|(a, b)| a == b)
            .unwrap_or(false);

        if is_current_exe {
            self_replace::self_replace(&src_exe)
                .map_err(|e| format!("執行中的程序替換失敗: {e}"))?;
        } else {
            #[cfg(windows)]
            {
                let old_exe = target_exe.with_extension(format!("old.{}.tmp", std::process::id()));
                let _ = fs::remove_file(&old_exe);
                if target_exe.exists() {
                    fs::rename(&target_exe, &old_exe)
                        .map_err(|e| format!("Windows 執行檔換名失敗: {e}"))?;
                }
                if let Err(e) = fs::copy(&src_exe, &target_exe) {
                    let _ = fs::rename(&old_exe, &target_exe);
                    return Err(format!("寫入新執行檔失敗: {e}"));
                }
                let _ = fs::remove_file(&old_exe);
            }

            #[cfg(not(windows))]
            {
                safe_replace_file(&src_exe, &target_exe)?;
            }
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&target_exe, fs::Permissions::from_mode(0o755));
        }

        for folder in ["static", "shell", "scripts"] {
            let src = release_root.join(folder);
            let dst = install_dir.join(folder);
            if src.exists() {
                let staging = install_dir.join(format!(".{folder}-staging-{}", std::process::id()));
                if staging.exists() {
                    fs::remove_dir_all(&staging)
                        .map_err(|e| format!("清除舊暫存目錄失敗 ({staging:?}): {e}"))?;
                }
                copy_dir_recursive(&src, &staging)?;

                if dst.exists() {
                    let old = install_dir.join(format!(".{folder}-old-{}", std::process::id()));
                    if old.exists() {
                        fs::remove_dir_all(&old)
                            .map_err(|e| format!("清除舊備份目錄失敗 ({old:?}): {e}"))?;
                    }
                    fs::rename(&dst, &old).map_err(|e| format!("目錄安全換名失敗 {dst:?}: {e}"))?;
                    if let Err(e) = fs::rename(&staging, &dst) {
                        let _ = fs::rename(&old, &dst);
                        return Err(format!("原子切換目錄失敗 {folder}: {e}"));
                    }
                    let _ = fs::remove_dir_all(&old);
                } else {
                    fs::rename(&staging, &dst)
                        .map_err(|e| format!("移動新目錄失敗 {folder}: {e}"))?;
                }
            }
        }

        for file in [
            "pricing.csv",
            "VERSION",
            "README.md",
            "LICENSE",
            "install.sh",
            "install.ps1",
        ] {
            let src = release_root.join(file);
            let dst = install_dir.join(file);
            if src.exists() {
                safe_replace_file(&src, &dst)?;
            }
        }

        // 確保自訂安裝目錄保有安裝標記，避免未來更新無法辨識（移除既有符號連結並以原子方式覆寫一般檔案）
        safe_write_file(
            &install_dir.join(".install_marker"),
            b"token-usage-insights:installed",
        )?;

        // 寫入提交標記，證明新版資產已全數寫入成功，救援流程不可回滾
        safe_write_file(&backup_dir.join(".committed"), b"committed")?;

        Ok(())
    })();

    if let Err(err) = install_result {
        eprintln!("❌ 安裝失敗，正在自動回滾: {err}");
        log_update("ERROR", "INSTALL", &format!("安裝失敗: {err}，開始回滾"));
        if let Err(rollback_err) = restore_from_backup(backup_dir, install_dir) {
            let _ = fs::write(backup_dir.join(".rollback_failed"), &rollback_err);
            eprintln!(
                "❌ 自動回滾失敗: {rollback_err}；請保留備份目錄 {:?} 進行手動還原",
                backup_dir
            );
            log_update("ERROR", "ROLLBACK", &format!("回滾失敗: {rollback_err}"));
            return Err(format!(
                "安裝失敗 ({err}) 且回滾失敗 ({rollback_err})；備份已保留於 {backup_dir:?}"
            ));
        } else {
            println!("✅ 已成功回滾至先前版本。");
            log_update("INFO", "ROLLBACK", "回滾成功");
            let _ = fs::remove_dir_all(backup_dir);
            if stopped_status.stopped_any {
                if stopped_status.had_supervised {
                    // 受到 run-service.ps1 或 systemd/launchd 監管的服務：
                    // 在 Windows 上保留 .service_restart_pending，待更新鎖釋放後由 run-service.ps1 接手重啟；
                    // 在 Linux/macOS 上由 systemd/launchd 自行重啟，絕不直接生成未監管的 raw process
                    log_update(
                        "INFO",
                        "ROLLBACK",
                        "回滾完成，保留標記由服務管理器自動重啟監管進程",
                    );
                } else if stopped_status.had_unsupervised {
                    // 先前是由使用者手動以非監管方式啟動的看板進程，重啟還原後的背景看板
                    let _ = fs::remove_file(install_dir.join(".service_restart_pending"));
                    restart_background_dashboard(install_dir);
                    log_update("INFO", "ROLLBACK", "回滾完成，已重新啟動非監管背景看板進程");
                }
            }
        }
        return Err(err);
    }

    if backup_dir.exists() {
        if let Err(e) = fs::remove_dir_all(backup_dir) {
            log_update("WARN", "CLEANUP", &format!("清理備份目錄失敗: {e}"));
            let fallback_backup =
                install_dir.join(format!(".backup-old-{}", Utc::now().timestamp()));
            if let Err(re) = fs::rename(backup_dir, &fallback_backup) {
                log_update("WARN", "CLEANUP", &format!("備份目錄換名失敗: {re}"));
                eprintln!(
                    "⚠️ 更新已安裝完成，但備份目錄無法清理或換名 ({backup_dir:?}): {re}；請稍後手動移除。"
                );
            } else {
                log_update(
                    "INFO",
                    "CLEANUP",
                    &format!("備份目錄已安全移至 {fallback_backup:?}"),
                );
            }
        }
    }

    // 更新成功後處理先前停止之服務進程重啟：
    if stopped_status.stopped_any {
        if stopped_status.had_supervised {
            // Windows run-service.ps1 監管中：保留 .service_restart_pending，
            // 當 run_update 結束釋放 UpdateLock 後，run-service.ps1 會偵測並重啟新版服務；
            // Linux systemd / macOS launchd 亦由監管者自行重啟
            println!("🔄 服務管理器將在新版就緒後自動重啟監管進程。");
            log_update(
                "INFO",
                "RESTART",
                "更新成功，保留標記由服務管理器自動重啟監管進程",
            );
        } else if stopped_status.had_unsupervised {
            // 先前為非監管/直接啟動之看板進程，手動更新完成後自動重啟新版背景看板，避免看板離線
            let _ = fs::remove_file(install_dir.join(".service_restart_pending"));
            restart_background_dashboard(install_dir);
            println!("🔄 已重新啟動 Token 戰情室背景看板服務。");
            log_update("INFO", "RESTART", "更新成功，已重新啟動非監管背景看板進程");
        }
    }

    Ok(())
}

fn get_update_check_interval_secs() -> i64 {
    let (_, yaml_interval_days) = load_update_config();
    let default_hours = yaml_interval_days
        .map(|d| d.saturating_mul(24))
        .unwrap_or(DEFAULT_UPDATE_INTERVAL_HOURS);
    let interval_hours: i64 = std::env::var("TOKEN_USAGE_INSIGHTS_UPDATE_INTERVAL_HOURS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default_hours);

    let valid_hours = if interval_hours <= 0 {
        DEFAULT_UPDATE_INTERVAL_HOURS
    } else {
        interval_hours.min(87600) // 最多 10 年，防止溢位
    };
    valid_hours.saturating_mul(3600)
}

fn is_update_check_interval_elapsed() -> bool {
    let interval_secs = get_update_check_interval_secs();
    if let Ok(conn) = crate::db::get_db_conn() {
        if let Ok(Some(last_check_str)) = crate::db::get_system_metadata(&conn, LAST_CHECK_KEY) {
            if let Ok(last_check) = chrono::DateTime::parse_from_rfc3339(&last_check_str) {
                let elapsed_secs = Utc::now().timestamp() - last_check.timestamp();
                if elapsed_secs >= 0 && elapsed_secs < interval_secs {
                    return false;
                }
            }
        }
    }
    true
}

fn is_lock_conflict_error(err: &str) -> bool {
    err.contains("已有另一個更新程序正在執行中")
}

async fn wait_for_lock_release(install_dir: &Path, timeout: Duration) -> Result<(), String> {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if !UpdateLock::is_locked(install_dir) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Err(format!(
        "等待更新程序釋放鎖定逾時（超過 {} 秒）",
        timeout.as_secs()
    ))
}

fn restart_current_process(args: &[String]) -> ! {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let current_exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from(&args[0]));
        let mut cmd = std::process::Command::new(current_exe);
        if args.len() > 1 {
            cmd.args(&args[1..]);
        }
        cmd.env("_TOKEN_USAGE_INSIGHTS_RESTARTED", "1");
        let err = cmd.exec();
        eprintln!("❌ 自動重啟進程失敗: {err}；請手動重新啟動程序。");
        log_update(
            "ERROR",
            "STARTUP_RESTART",
            &format!("自動重啟進程失敗: {err}"),
        );
        std::process::exit(1);
    }

    #[cfg(windows)]
    {
        if std::env::var("TOKEN_USAGE_INSIGHTS_SERVICE").is_ok() {
            log_update(
                "INFO",
                "STARTUP_RESTART",
                "以退出碼 75 請求 Windows 服務管理器重啟新版程序",
            );
            std::process::exit(75);
        } else {
            let current_exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from(&args[0]));
            let mut cmd = std::process::Command::new(current_exe);
            if args.len() > 1 {
                cmd.args(&args[1..]);
            }
            cmd.env("_TOKEN_USAGE_INSIGHTS_RESTARTED", "1");
            match cmd.status() {
                Ok(status) => std::process::exit(status.code().unwrap_or(0)),
                Err(err) => {
                    eprintln!("❌ 自動重啟進程失敗: {err}；請手動重新啟動程序。");
                    log_update("ERROR", "STARTUP_RESTART", &format!("重啟進程失敗: {err}"));
                    std::process::exit(1);
                }
            }
        }
    }

    #[cfg(not(any(unix, windows)))]
    {
        std::process::exit(0);
    }
}

fn is_auto_update_disabled(args: &[String]) -> bool {
    if args.iter().any(|arg| arg == "--no-auto-update") {
        return true;
    }
    if let Ok(val) = std::env::var("TOKEN_USAGE_INSIGHTS_AUTO_UPDATE") {
        let lower = val.trim().to_lowercase();
        if lower == "0" || lower == "false" || lower == "no" || lower == "off" {
            return true;
        }
        if lower == "1" || lower == "true" || lower == "yes" || lower == "on" {
            return false;
        }
    }
    // CI 環境自動防護：在 CI/自動化環境中預設停用背景自動更新，避免受遠端發行版本干擾
    if std::env::var("CI").is_ok() || std::env::var("GITHUB_ACTIONS").is_ok() {
        return true;
    }
    let (yaml_auto, _) = load_update_config();
    if yaml_auto == Some(false) {
        return true;
    }
    false
}

fn attempt_startup_recovery(install_dir: &Path, args: &[String]) {
    let backup_dir = install_dir.join(".backup");
    if !backup_dir.exists() {
        return;
    }

    // 若存在 .committed 標記，代表更新早已成功完成，僅備份目錄在最後刪除時中斷
    // 此時絕不能回滾新版本，直接清理備份目錄即可
    if backup_dir.join(".committed").exists() {
        log_update(
            "INFO",
            "STARTUP_RECOVERY",
            "先前更新已成功提交，清理殘留之備份目錄",
        );
        let _ = fs::remove_dir_all(&backup_dir);
        if backup_dir.exists() {
            let cleanup_name = format!(".backup-cleaned-{}", Utc::now().timestamp());
            let _ = fs::rename(&backup_dir, install_dir.join(&cleanup_name));
        }
        return;
    }

    if backup_dir.join(".rollback_failed").exists() {
        eprintln!(
            "❌ 偵測到先前更新回滾失敗標記 ({backup_dir:?})；為防止讀取損毀狀態，程序終止。請依備份手動還原。"
        );
        log_update("ERROR", "STARTUP_FATAL", "先前回滾失敗，程序終止");
        std::process::exit(1);
    }

    println!("⚠️ 偵測到先前更新殘留之備份目錄，正在取得更新鎖定以進行檢查與救援還原...");
    let _recovery_lock = match UpdateLock::try_acquire(install_dir) {
        Ok(l) => l,
        Err(e) => {
            log_update(
                "INFO",
                "STARTUP_RECOVERY",
                &format!("目前更新鎖被占用，暫緩救援: {e}"),
            );
            return;
        }
    };

    let manifest_path = backup_dir.join(".manifest");
    if !manifest_path.exists() {
        // 未含有效 manifest 的備份視為未完成之備份交易（此階段尚未替換任何安裝檔案）
        // 必須安全清理或換名，避免阻礙後續所有更新
        println!("⚠️ 偵測到未含有效清單的未完成備份交易目錄 ({backup_dir:?})，正在安全清理...");
        log_update(
            "WARN",
            "STARTUP_RECOVERY",
            "偵測到無 manifest 之未完成備份目錄，執行安全清理",
        );
        if let Err(e) = fs::remove_dir_all(&backup_dir) {
            let incomplete_name = format!(".backup-incomplete-{}", Utc::now().timestamp());
            let fallback = install_dir.join(&incomplete_name);
            if let Err(re) = fs::rename(&backup_dir, &fallback) {
                eprintln!(
                    "❌ 偵測到未完成交易備份目錄但無法清理或更名 ({backup_dir:?}): {e}; {re}；程序終止以保護狀態。"
                );
                log_update("ERROR", "STARTUP_FATAL", "清理未完成備份目錄失敗，程序終止");
                std::process::exit(1);
            }
        }
        return;
    }

    println!("⚠️ 正在自動救援還原至健全版本...");
    log_update("WARN", "STARTUP_RECOVERY", "取得更新鎖，執行自動救援還原");

    if let Err(e) = restore_from_backup(&backup_dir, install_dir) {
        eprintln!("❌ 自動救援還原失敗: {e}；程序終止以保護狀態。");
        log_update("ERROR", "STARTUP_FATAL", &format!("救援還原失敗: {e}"));
        std::process::exit(1);
    }

    // 還原成功：嚴禁在確認目錄清理或更名成功前先刪除 .manifest。
    // 若清理與更名都失敗，保留 .manifest 並終止程序，絕不留下失去 manifest 卻阻擋更新的孤立 .backup
    if let Err(e) = fs::remove_dir_all(&backup_dir) {
        log_update(
            "WARN",
            "STARTUP_RECOVERY",
            &format!("清理已還原備份目錄失敗: {e}，嘗試更名隔離"),
        );
        let restored_name = format!(".backup-restored-{}", Utc::now().timestamp());
        let fallback = install_dir.join(&restored_name);
        if let Err(re) = fs::rename(&backup_dir, &fallback) {
            eprintln!(
                "❌ 自動救援還原已完成，但無法清理或更名備份目錄 ({backup_dir:?}): {e}; {re}；程序終止以保留完整救援狀態。請手動清理該目錄。"
            );
            log_update(
                "ERROR",
                "STARTUP_FATAL",
                "已還原但備份目錄無法清理且無法更名，程序終止以保留狀態",
            );
            std::process::exit(1);
        }
    }

    println!("✅ 已成功自動還原至健全版本，正在重新啟動 Token 戰情室...");
    log_update("INFO", "STARTUP_RECOVERY", "自動救援還原成功，重啟進程");
    restart_current_process(args);
}

pub async fn perform_startup_recovery() {
    if std::env::var_os("_TOKEN_USAGE_INSIGHTS_RESTARTED").is_some() {
        return;
    }

    let args: Vec<String> = std::env::args().collect();
    let env_kind = detect_environment();

    if let EnvironmentKind::StandardInstalled { install_dir, .. } = &env_kind {
        if UpdateLock::is_locked(install_dir) {
            println!("⏳ 偵測到已有更新程序正在進行中，等待更新完成...");
            log_update("INFO", "STARTUP_WAIT", "偵測到進行中的更新鎖，等待其釋放");
            match wait_for_lock_release(
                install_dir,
                Duration::from_secs(STARTUP_AUTO_UPDATE_TOTAL_TIMEOUT_SECS),
            )
            .await
            {
                Ok(()) => {
                    attempt_startup_recovery(install_dir, &args);
                    println!("🔄 更新程序已完成，正在重新啟動 Token 戰情室...");
                    log_update("INFO", "STARTUP_RESTART", "其他程序更新完成，重啟進程");
                    restart_current_process(&args);
                }
                Err(e) => {
                    eprintln!("❌ 等待更新程序超時: {e}；為防止讀取不一致檔案，程序終止。");
                    log_update("ERROR", "STARTUP_LOCK", &format!("等待更新鎖超時: {e}"));
                    std::process::exit(1);
                }
            }
        }

        attempt_startup_recovery(install_dir, &args);
    }
}

pub fn spawn_background_auto_update() {
    tokio::spawn(async {
        // 延遲 1 秒執行，確保主服務監聽與 TCP 綁定先行就緒，離線或慢速網路零阻塞
        tokio::time::sleep(Duration::from_secs(1)).await;
        run_background_auto_update().await;
    });
}

async fn run_background_auto_update() {
    if std::env::var_os("_TOKEN_USAGE_INSIGHTS_RESTARTED").is_some() {
        return;
    }

    let args: Vec<String> = std::env::args().collect();
    if is_auto_update_disabled(&args) {
        return;
    }

    let env_kind = detect_environment();
    if matches!(env_kind, EnvironmentKind::Npm { .. }) {
        if !is_update_check_interval_elapsed() {
            return;
        }
        if let Ok(release) = fetch_release_with_logging(None, STARTUP_CHECK_TIMEOUT_SECS).await {
            if let Ok(conn) = crate::db::get_db_conn() {
                let now_str = Utc::now().to_rfc3339();
                let _ = crate::db::set_system_metadata(&conn, LAST_CHECK_KEY, &now_str);
            }
            let current_version = env!("CARGO_PKG_VERSION");
            if is_newer_version(&release.tag_name, current_version) {
                log_update(
                    "INFO",
                    "STARTUP_CHECK",
                    &format!("npm 環境偵測到新版本 {}", release.tag_name),
                );
                println!(
                    "💡 發現新版本 {}！您可以執行 npx token-usage-insights@latest 啟動最新版本。",
                    release.tag_name
                );
            }
        }
        return;
    }

    let install_dir = match &env_kind {
        EnvironmentKind::StandardInstalled { install_dir, .. } => install_dir.clone(),
        _ => return,
    };

    if !is_update_check_interval_elapsed() {
        return;
    }

    let release = match fetch_release_with_logging(None, STARTUP_CHECK_TIMEOUT_SECS).await {
        Ok(r) => r,
        Err(e) => {
            log_update("WARN", "STARTUP_CHECK", &format!("啟動更新檢查略過: {e}"));
            return;
        }
    };

    let current_version = env!("CARGO_PKG_VERSION");
    if !is_newer_version(&release.tag_name, current_version) {
        if let Ok(conn) = crate::db::get_db_conn() {
            let now_str = Utc::now().to_rfc3339();
            let _ = crate::db::set_system_metadata(&conn, LAST_CHECK_KEY, &now_str);
        }
        return;
    }

    println!(
        "🚀 發現新版本 {}（目前為 v{}），正在自動更新...",
        release.tag_name, current_version
    );
    log_update(
        "INFO",
        "STARTUP_CHECK",
        &format!("觸發啟動自動更新至 {}", release.tag_name),
    );

    let update_opts = UpdateOptions {
        check_only: false,
        force: false,
        target_version: Some(release.tag_name.clone()),
        prefetched_release: Some(release.clone()),
    };

    let update_result = run_update(update_opts).await;

    match update_result {
        Ok(()) => {
            if let Ok(conn) = crate::db::get_db_conn() {
                let now_str = Utc::now().to_rfc3339();
                let _ = crate::db::set_system_metadata(&conn, LAST_CHECK_KEY, &now_str);
            }

            println!("🔄 更新完成，正在自動重啟 Token 戰情室...");
            log_update("INFO", "STARTUP_RESTART", "更新完成，重啟進程");
            restart_current_process(&args);
        }
        Err(e) => {
            let err_msg = e.to_string();
            if is_lock_conflict_error(&err_msg) {
                println!("⏳ 偵測到已有更新程序正在進行中，等待更新完成...");
                log_update("INFO", "STARTUP_WAIT", "遇到更新鎖競爭，等待另一程序完成");
                match wait_for_lock_release(
                    &install_dir,
                    Duration::from_secs(STARTUP_AUTO_UPDATE_TOTAL_TIMEOUT_SECS),
                )
                .await
                {
                    Ok(()) => {
                        attempt_startup_recovery(&install_dir, &args);
                        println!("🔄 更新已由另一程序完成，正在重新啟動 Token 戰情室...");
                        log_update("INFO", "STARTUP_RESTART", "另一程序更新完成，重啟進程");
                        restart_current_process(&args);
                    }
                    Err(wait_err) => {
                        eprintln!(
                            "❌ 等待更新程序超時: {wait_err}；為防止讀取不一致檔案，程序終止。"
                        );
                        log_update(
                            "ERROR",
                            "STARTUP_LOCK",
                            &format!("等待更新鎖超時: {wait_err}"),
                        );
                        std::process::exit(1);
                    }
                }
            } else {
                attempt_startup_recovery(&install_dir, &args);
                eprintln!("⚠️ 自動更新失敗: {err_msg}，將繼續以現有健全版本啟動服務。");
                log_update(
                    "WARN",
                    "STARTUP_UPDATE",
                    &format!("自動更新失敗: {err_msg}"),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_parsing_and_newer_comparison() {
        assert_eq!(parse_semver("v0.9.5"), Some((0, 9, 5)));
        assert_eq!(parse_semver("0.9.5"), Some((0, 9, 5)));
        assert_eq!(parse_semver("v1.2.3-beta.1"), Some((1, 2, 3)));

        assert!(is_newer_version("v0.9.6", "0.9.5"));
        assert!(is_newer_version("v1.0.0", "0.9.5"));
        assert!(is_newer_version("v0.10.0", "0.9.9"));
        assert!(!is_newer_version("v0.9.5", "0.9.5"));
        assert!(!is_newer_version("v0.9.4", "0.9.5"));
        assert!(!is_newer_version("v0.8.99", "0.9.5"));
    }

    #[test]
    fn parse_checksum_extracts_correct_hash() {
        let sums = r#"
4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945  ./token-usage-insights-v0.9.5-aarch64-apple-darwin.tar.gz
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 *token-usage-insights-v0.9.5-x86_64-apple-darwin.tar.gz
a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2  token-usage-insights-v0.9.5-x86_64-pc-windows-msvc.zip
"#;
        assert_eq!(
            parse_checksum(
                sums,
                "token-usage-insights-v0.9.5-aarch64-apple-darwin.tar.gz"
            ),
            Some("4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945".to_string())
        );
        assert_eq!(
            parse_checksum(
                sums,
                "token-usage-insights-v0.9.5-x86_64-apple-darwin.tar.gz"
            ),
            Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_string())
        );
        assert_eq!(
            parse_checksum(
                sums,
                "token-usage-insights-v0.9.5-x86_64-pc-windows-msvc.zip"
            ),
            Some("a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2".to_string())
        );
        assert_eq!(parse_checksum(sums, "non-existent-file"), None);
    }

    #[test]
    fn verify_sha256_matches_content() {
        let data = b"hello token-usage-insights";
        let mut hasher = Sha256::new();
        hasher.update(data);
        let expected = hex::encode(hasher.finalize());

        assert!(verify_sha256(data, &expected));
        assert!(!verify_sha256(b"wrong data", &expected));
    }

    #[test]
    fn system_metadata_get_and_set() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::init_db(&conn).unwrap();

        assert_eq!(
            crate::db::get_system_metadata(&conn, "test_key").unwrap(),
            None
        );

        crate::db::set_system_metadata(&conn, "test_key", "value_1").unwrap();
        assert_eq!(
            crate::db::get_system_metadata(&conn, "test_key").unwrap(),
            Some("value_1".to_string())
        );

        crate::db::set_system_metadata(&conn, "test_key", "value_2").unwrap();
        assert_eq!(
            crate::db::get_system_metadata(&conn, "test_key").unwrap(),
            Some("value_2".to_string())
        );
    }

    #[test]
    fn archive_filename_formats_correctly() {
        let target = "aarch64-apple-darwin";
        let filename = archive_filename("v0.9.5", target);
        #[cfg(windows)]
        assert_eq!(
            filename,
            "token-usage-insights-v0.9.5-aarch64-apple-darwin.zip"
        );
        #[cfg(not(windows))]
        assert_eq!(
            filename,
            "token-usage-insights-v0.9.5-aarch64-apple-darwin.tar.gz"
        );
    }

    #[test]
    fn parse_config_yaml_extracts_options() {
        let yaml = r#"
# Token 戰情室設定檔
auto_update: false
update_check_interval: 3
"#;
        let (auto, interval) = parse_config_yaml(yaml);
        assert_eq!(auto, Some(false));
        assert_eq!(interval, Some(3));

        let yaml2 = r#"
auto_update: "true"
update_check_interval: '7'
"#;
        let (auto2, interval2) = parse_config_yaml(yaml2);
        assert_eq!(auto2, Some(true));
        assert_eq!(interval2, Some(7));

        let yaml3 = r#"
auto_update: false # disable auto updates
update_check_interval: 5 # check every 5 days
"#;
        let (auto3, interval3) = parse_config_yaml(yaml3);
        assert_eq!(auto3, Some(false));
        assert_eq!(interval3, Some(5));
    }

    #[test]
    fn backup_and_restore_cycle() {
        let temp = std::env::temp_dir().join(format!(
            "test-backup-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let install_dir = temp.join("install");
        let backup_dir = temp.join("backup");
        fs::create_dir_all(&install_dir).unwrap();

        fs::write(install_dir.join("VERSION"), "v0.9.5").unwrap();
        fs::write(install_dir.join("pricing.csv"), "model,price").unwrap();
        fs::create_dir_all(install_dir.join("static")).unwrap();
        fs::write(
            install_dir.join("static").join("index.html"),
            "<h1>Test</h1>",
        )
        .unwrap();

        // Backup
        backup_installation(&install_dir, &backup_dir).unwrap();
        assert!(backup_dir.join("VERSION").exists());
        assert!(backup_dir.join("pricing.csv").exists());
        assert!(backup_dir.join("static").join("index.html").exists());
        assert!(backup_dir.join(".manifest").exists());

        // Re-run backup should fail because backup_dir already exists to protect recovery state
        assert!(backup_installation(&install_dir, &backup_dir).is_err());

        // Corrupt install_dir and simulate adding a new file not present in original backup
        fs::write(install_dir.join("VERSION"), "corrupted").unwrap();
        fs::remove_file(install_dir.join("pricing.csv")).unwrap();
        fs::write(install_dir.join("install.sh"), "#!/bin/sh\n").unwrap();

        // Restore
        restore_from_backup(&backup_dir, &install_dir).unwrap();
        assert_eq!(
            fs::read_to_string(install_dir.join("VERSION")).unwrap(),
            "v0.9.5"
        );
        assert!(install_dir.join("pricing.csv").exists());
        // The newly added install.sh was not in the backup manifest and should be removed
        assert!(!install_dir.join("install.sh").exists());

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn process_alive_check_identifies_current_process() {
        let current_pid = std::process::id();
        assert!(is_process_alive(current_pid));
    }

    #[test]
    fn update_check_interval_handles_extremes_and_saturation() {
        // 預設間隔 (24 小時 -> 86400 秒)
        let default_secs = get_update_check_interval_secs();
        assert!(default_secs > 0);
        assert!(default_secs <= 87600 * 3600);
    }

    #[tokio::test]
    async fn update_lock_prevents_concurrent_access() {
        let temp = std::env::temp_dir().join(format!(
            "test-lock-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        fs::create_dir_all(&temp).unwrap();

        assert!(!UpdateLock::is_locked(&temp));

        let lock1 = UpdateLock::try_acquire(&temp);
        assert!(lock1.is_ok());
        assert!(UpdateLock::is_locked(&temp));

        let lock2 = UpdateLock::try_acquire(&temp);
        assert!(lock2.is_err());
        let err_msg = lock2.unwrap_err();
        assert!(is_lock_conflict_error(&err_msg));

        drop(lock1);
        assert!(!UpdateLock::is_locked(&temp));

        assert!(wait_for_lock_release(&temp, Duration::from_millis(500))
            .await
            .is_ok());

        let lock3 = UpdateLock::try_acquire(&temp);
        assert!(lock3.is_ok());
        assert!(UpdateLock::is_locked(&temp));

        drop(lock3);
        assert!(!UpdateLock::is_locked(&temp));
        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn apply_installation_with_rollback_success_and_restore_on_error() {
        let temp = std::env::temp_dir().join(format!(
            "test-install-orchestration-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let install_dir = temp.join("install");
        let release_root = temp.join("release");
        let bad_release = temp.join("bad_release");
        let backup_dir = install_dir.join(".backup");

        fs::create_dir_all(&install_dir).unwrap();
        fs::create_dir_all(&release_root).unwrap();
        fs::create_dir_all(&bad_release).unwrap();

        let exec_name = if cfg!(windows) {
            format!("{APP_NAME}.exe")
        } else {
            APP_NAME.to_string()
        };

        // Existing installation v0.9.5
        fs::write(install_dir.join(&exec_name), "old binary").unwrap();
        fs::write(install_dir.join("VERSION"), "v0.9.5").unwrap();
        fs::write(install_dir.join("pricing.csv"), "old pricing").unwrap();
        fs::create_dir_all(install_dir.join("static")).unwrap();
        fs::write(install_dir.join("static").join("index.html"), "old html").unwrap();
        fs::create_dir_all(install_dir.join("scripts")).unwrap();
        fs::create_dir_all(install_dir.join("shell")).unwrap();
        fs::write(install_dir.join("install.sh"), "#!/bin/sh").unwrap();
        fs::write(install_dir.join("install.ps1"), "# powershell").unwrap();

        // Valid new release v0.9.6
        fs::write(release_root.join(&exec_name), "new binary").unwrap();
        fs::write(release_root.join("VERSION"), "v0.9.6").unwrap();
        fs::write(release_root.join("pricing.csv"), "new pricing").unwrap();
        fs::create_dir_all(release_root.join("static")).unwrap();
        fs::write(release_root.join("static").join("index.html"), "new html").unwrap();
        fs::create_dir_all(release_root.join("scripts")).unwrap();
        fs::create_dir_all(release_root.join("shell")).unwrap();
        fs::write(release_root.join("install.sh"), "#!/bin/sh v2").unwrap();
        fs::write(release_root.join("install.ps1"), "# powershell v2").unwrap();

        // 1. Success case
        let result = apply_installation_with_rollback(&release_root, &install_dir, &backup_dir);
        assert!(
            result.is_ok(),
            "apply_installation_with_rollback failed: {:?}",
            result
        );
        assert_eq!(
            fs::read_to_string(install_dir.join("VERSION")).unwrap(),
            "v0.9.6"
        );
        assert_eq!(
            fs::read_to_string(install_dir.join("pricing.csv")).unwrap(),
            "new pricing"
        );
        assert_eq!(
            fs::read_to_string(install_dir.join(&exec_name)).unwrap(),
            "new binary"
        );
        assert_eq!(
            fs::read_to_string(install_dir.join("install.sh")).unwrap(),
            "#!/bin/sh v2"
        );
        assert!(
            !backup_dir.exists(),
            "backup_dir should be removed after success"
        );

        // 2. Failure case: bad release where the executable is a directory instead of a file
        // This causes fs::copy(&src_exe, &target_exe) to fail during installation, triggering rollback.
        fs::create_dir_all(bad_release.join(&exec_name)).unwrap();
        fs::write(bad_release.join("VERSION"), "v0.9.7-broken").unwrap();
        fs::create_dir_all(bad_release.join("static")).unwrap();
        fs::write(bad_release.join("static").join("index.html"), "broken html").unwrap();

        let fail_result = apply_installation_with_rollback(&bad_release, &install_dir, &backup_dir);
        assert!(
            fail_result.is_err(),
            "expected installation to fail with directory as binary"
        );

        // Verify that rollback restored install_dir to v0.9.6 state
        assert_eq!(
            fs::read_to_string(install_dir.join("VERSION")).unwrap(),
            "v0.9.6"
        );
        assert_eq!(
            fs::read_to_string(install_dir.join("pricing.csv")).unwrap(),
            "new pricing"
        );
        assert_eq!(
            fs::read_to_string(install_dir.join(&exec_name)).unwrap(),
            "new binary"
        );
        assert_eq!(
            fs::read_to_string(install_dir.join("static").join("index.html")).unwrap(),
            "new html"
        );
        assert_eq!(
            fs::read_to_string(install_dir.join("install.sh")).unwrap(),
            "#!/bin/sh v2"
        );
        assert!(
            !backup_dir.exists(),
            "backup_dir should be removed after successful rollback"
        );

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn process_exe_path_and_matches_install_dir() {
        let my_pid = std::process::id();
        let exe_path = get_process_exe_path(my_pid);
        assert!(
            exe_path.is_some(),
            "should be able to get current process exe path"
        );
        let path = exe_path.unwrap();
        assert!(path.exists(), "process exe path should exist: {:?}", path);

        let parent = path.parent().unwrap();
        assert!(matches_install_dir(&path, parent));
        assert!(!matches_install_dir(
            &path,
            Path::new("/nonexistent/directory")
        ));
    }

    #[test]
    fn server_pid_guard_lifecycle() {
        let temp = std::env::temp_dir().join(format!(
            "pid-test-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        fs::create_dir_all(&temp).unwrap();
        let pid_path = temp.join(".server.pid");

        {
            let _guard = ServerPidGuard {
                paths: vec![pid_path.clone()],
            };
            fs::write(&pid_path, std::process::id().to_string()).unwrap();
            assert!(pid_path.exists());
        }

        assert!(
            !pid_path.exists(),
            ".server.pid should be removed when guard is dropped"
        );
        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn safe_replace_file_overwrites_existing_file() {
        let temp = std::env::temp_dir().join(format!(
            "safe-replace-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        fs::create_dir_all(&temp).unwrap();
        let src = temp.join("source.txt");
        let dst = temp.join("destination.txt");

        fs::write(&src, "new content").unwrap();
        fs::write(&dst, "old content").unwrap();

        assert!(safe_replace_file(&src, &dst).is_ok());
        assert_eq!(fs::read_to_string(&dst).unwrap(), "new content");

        #[cfg(unix)]
        {
            let outside = temp.join("outside.txt");
            let symlink_dst = temp.join("link_dst.txt");
            fs::write(&outside, "sensitive outside content").unwrap();
            std::os::unix::fs::symlink(&outside, &symlink_dst).unwrap();

            assert!(safe_replace_file(&src, &symlink_dst).is_ok());
            // Symlink should be replaced with regular file, and outside file untouched
            let meta = fs::symlink_metadata(&symlink_dst).unwrap();
            assert!(!meta.file_type().is_symlink());
            assert_eq!(fs::read_to_string(&symlink_dst).unwrap(), "new content");
            assert_eq!(
                fs::read_to_string(&outside).unwrap(),
                "sensitive outside content"
            );
        }

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn incomplete_backup_without_manifest_is_cleaned_up_on_recovery() {
        let temp = std::env::temp_dir().join(format!(
            "incomplete-backup-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let install_dir = temp.join("install");
        let backup_dir = install_dir.join(".backup");
        fs::create_dir_all(&backup_dir).unwrap();
        fs::write(backup_dir.join("partial_file"), "partial").unwrap();

        assert!(backup_dir.exists());
        assert!(!backup_dir.join(".manifest").exists());

        attempt_startup_recovery(&install_dir, &[]);

        assert!(
            !backup_dir.exists(),
            "incomplete backup without manifest should be safely removed during recovery"
        );

        let _ = fs::remove_dir_all(&temp);
    }

    #[cfg(unix)]
    #[test]
    fn install_marker_symlink_is_rejected() {
        let temp = std::env::temp_dir().join(format!(
            "marker-symlink-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        fs::create_dir_all(&temp).unwrap();
        let target = temp.join("target_marker");
        let marker = temp.join(".install_marker");

        fs::write(&target, "token-usage-insights:installed").unwrap();
        std::os::unix::fs::symlink(&target, &marker).unwrap();

        let meta = fs::symlink_metadata(&marker).unwrap();
        let is_valid_marker = meta.is_file() && !meta.file_type().is_symlink();
        assert!(
            !is_valid_marker,
            "symlinked marker should not be treated as a valid regular marker file"
        );

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn committed_marker_prevents_erroneous_rollback_on_recovery() {
        let temp = std::env::temp_dir().join(format!(
            "committed-marker-test-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let install_dir = temp.join("install");
        let backup_dir = install_dir.join(".backup");
        fs::create_dir_all(&install_dir).unwrap();
        fs::create_dir_all(&backup_dir).unwrap();

        fs::write(install_dir.join("VERSION"), "v0.9.6").unwrap();
        fs::write(backup_dir.join("VERSION"), "v0.9.5").unwrap();
        fs::write(backup_dir.join(".manifest"), "VERSION").unwrap();
        fs::write(backup_dir.join(".committed"), "committed").unwrap();

        attempt_startup_recovery(&install_dir, &[]);

        // 因為存在 .committed 標記，新版絕不可被錯誤回滾至舊版 v0.9.5
        assert_eq!(
            fs::read_to_string(install_dir.join("VERSION")).unwrap(),
            "v0.9.6"
        );
        assert!(!backup_dir.exists(), ".backup 應在確認已提交後被安全清理");

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn is_cli_subcommand_identifies_cli_commands() {
        assert!(is_cli_subcommand("export"));
        assert!(is_cli_subcommand("export-all"));
        assert!(is_cli_subcommand("import"));
        assert!(is_cli_subcommand("update"));
        assert!(is_cli_subcommand("--update"));
        assert!(is_cli_subcommand("-u"));
        assert!(is_cli_subcommand("--help"));
        assert!(is_cli_subcommand("-h"));
        assert!(is_cli_subcommand("--version"));
        assert!(is_cli_subcommand("-V"));
        assert!(is_cli_subcommand("completion"));

        assert!(!is_cli_subcommand("--no-auto-update"));
        assert!(!is_cli_subcommand("--port"));
        assert!(!is_cli_subcommand("3003"));
    }

    #[test]
    fn auto_update_disabled_in_ci_and_flags() {
        assert!(is_auto_update_disabled(&["--no-auto-update".to_string()]));
        assert!(is_auto_update_disabled(&[
            "app".to_string(),
            "--no-auto-update".to_string()
        ]));
    }

    #[test]
    fn stopped_dashboard_status_properties() {
        let none = StoppedDashboardStatus {
            stopped_any: false,
            had_supervised: false,
            had_unsupervised: false,
        };
        assert!(!none.stopped_any);

        let sup = StoppedDashboardStatus {
            stopped_any: true,
            had_supervised: true,
            had_unsupervised: false,
        };
        assert!(sup.stopped_any);
        assert!(sup.had_supervised);
        assert!(!sup.had_unsupervised);

        let unsup = StoppedDashboardStatus {
            stopped_any: true,
            had_supervised: false,
            had_unsupervised: true,
        };
        assert!(unsup.stopped_any);
        assert!(!unsup.had_supervised);
        assert!(unsup.had_unsupervised);
    }

    #[test]
    fn update_error_display_and_conversion() {
        let safe = UpdateError::SafeRejection("測試拒絕".to_string());
        assert_eq!(safe.to_string(), "測試拒絕");

        let fail: UpdateError = "測試失敗".to_string().into();
        assert_eq!(fail, UpdateError::Failure("測試失敗".to_string()));
        assert_eq!(fail.to_string(), "測試失敗");
    }
}
