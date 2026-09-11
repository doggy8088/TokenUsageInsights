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
fn get_process_exe_path(pid: u32) -> Option<PathBuf> {
    let script =
        format!("(Get-CimInstance Win32_Process -Filter \"ProcessId = {pid}\").ExecutablePath");
    if let Ok(output) = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
    {
        let out = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !out.is_empty() {
            let p = PathBuf::from(&out);
            if p.exists() {
                return Some(p);
            }
        }
    }
    None
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

    // 2. 檢查是否在 Git 或 Cargo 開發原始碼目錄
    for ancestor in exe_dir.ancestors() {
        if ancestor.join(".git").exists() || ancestor.join("Cargo.toml").exists() {
            return EnvironmentKind::GitOrDev {
                root: ancestor.to_path_buf(),
                exe_path,
            };
        }
    }

    // 3. 檢查標準安裝目錄（包含以 TOKEN_USAGE_INSIGHTS_INSTALL_DIR 明確指定的路徑）
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

    // 4. 檢查安裝標記檔（由 install.sh 或 install.ps1 寫入的自訂安裝目錄）
    let marker_path = exe_dir.join(".install_marker");
    if marker_path.is_file() {
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

#[derive(Debug)]
struct UpdateLock {
    lock_path: PathBuf,
}

impl UpdateLock {
    fn lock_path(install_dir: &Path) -> PathBuf {
        install_dir.join(".update.lock")
    }

    fn try_acquire(install_dir: &Path) -> Result<Self, String> {
        let lock_path = Self::lock_path(install_dir);
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(mut file) => {
                let _ = writeln!(file, "pid={}", std::process::id());
                Ok(Self { lock_path })
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let content = fs::read_to_string(&lock_path).unwrap_or_default();
                let recorded_pid = content.lines().find_map(|line| {
                    line.strip_prefix("pid=")
                        .and_then(|s| s.trim().parse::<u32>().ok())
                });

                if let Some(pid) = recorded_pid {
                    if is_process_alive(pid) {
                        return Err(format!(
                            "已有另一個更新程序正在執行中（PID {pid}），請稍候再試。"
                        ));
                    }
                    // 程序已終止，安全清除遺留鎖定檔並重試
                    log_update(
                        "WARN",
                        "LOCK",
                        &format!("偵測到已終止程序殘留之鎖定檔 (PID {pid})，自動清除"),
                    );
                    fs::remove_file(&lock_path).map_err(|e| {
                        format!("清除已終止程序遺留之更新鎖定檔失敗 ({lock_path:?}): {e}")
                    })?;
                    return Self::try_acquire(install_dir);
                }

                if let Ok(metadata) = fs::metadata(&lock_path) {
                    if let Ok(modified) = metadata.modified() {
                        if let Ok(elapsed) = modified.elapsed() {
                            if elapsed > Duration::from_secs(600) {
                                log_update(
                                    "WARN",
                                    "LOCK",
                                    "偵測到無法辨識 PID 且超過 10 分鐘之過期鎖定檔，自動清除",
                                );
                                fs::remove_file(&lock_path).map_err(|e| {
                                    format!("清除過期更新鎖定檔失敗 ({lock_path:?}): {e}")
                                })?;
                                return Self::try_acquire(install_dir);
                            }
                        }
                    }
                }
                Err("已有另一個更新程序正在執行中，請稍候再試。".to_string())
            }
            Err(e) => Err(format!("無法建立更新鎖 ({lock_path:?}): {e}")),
        }
    }

    /// 檢查是否有活躍中的更新程序持鎖
    fn is_locked(install_dir: &Path) -> bool {
        let lock_path = Self::lock_path(install_dir);
        if !lock_path.exists() {
            return false;
        }
        let content = fs::read_to_string(&lock_path).unwrap_or_default();
        let recorded_pid = content.lines().find_map(|line| {
            line.strip_prefix("pid=")
                .and_then(|s| s.trim().parse::<u32>().ok())
        });
        if let Some(pid) = recorded_pid {
            return is_process_alive(pid);
        }
        if let Ok(metadata) = fs::metadata(&lock_path) {
            if let Ok(modified) = metadata.modified() {
                if let Ok(elapsed) = modified.elapsed() {
                    return elapsed <= Duration::from_secs(600);
                }
            }
        }
        false
    }
}

impl Drop for UpdateLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.lock_path);
    }
}

struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    fn new(path: PathBuf) -> Result<Self, String> {
        if path.exists() {
            fs::remove_dir_all(&path).map_err(|e| format!("清除舊暫存目錄失敗 ({path:?}): {e}"))?;
        }
        fs::create_dir_all(&path).map_err(|e| format!("建立暫存目錄失敗: {e}"))?;
        Ok(Self { path })
    }

    fn cleanup(&self) -> Result<(), String> {
        if self.path.exists() {
            fs::remove_dir_all(&self.path)
                .map_err(|e| format!("清理暫存目錄失敗 ({:?}): {e}", self.path))?;
        }
        Ok(())
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
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
    fs::create_dir_all(dst).map_err(|e| format!("建立目標目錄失敗 {dst:?}: {e}"))?;
    for entry in fs::read_dir(src).map_err(|e| format!("讀取目錄失敗 {src:?}: {e}"))? {
        let entry = entry.map_err(|e| format!("讀取項目失敗: {e}"))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|e| format!("讀取檔案類型失敗: {e}"))?;
        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)
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
    fs::create_dir_all(backup_dir).map_err(|e| format!("建立備份目錄失敗: {e}"))?;

    let backup_res = (|| -> Result<(), String> {
        let mut manifest_entries = Vec::new();
        for &item in MANAGED_ITEMS {
            let src = install_dir.join(item);
            let dst = backup_dir.join(item);
            if src.exists() {
                manifest_entries.push(item);
                if src.is_dir() {
                    copy_dir_recursive(&src, &dst)?;
                } else if src.is_file() {
                    fs::copy(&src, &dst).map_err(|e| format!("備份檔案失敗 {item}: {e}"))?;
                }
            }
        }

        fs::write(backup_dir.join(".manifest"), manifest_entries.join("\n"))
            .map_err(|e| format!("寫入備份清單失敗: {e}"))?;

        Ok(())
    })();

    if let Err(e) = backup_res {
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
                fs::copy(&src, &dst)
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

pub async fn run_update(options: UpdateOptions) -> Result<(), String> {
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
            return Err("不支援在 npm / npx 環境中直接自我更新".to_string());
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
            return Err("開發目錄不支援自我更新".to_string());
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
            return Err("非標準目錄不支援自我更新".to_string());
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
                return Err(e);
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
            return Err(e);
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
            return Err(e);
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
                return Err(e);
            }
        };

    let expected_hash = match parse_checksum(&sums_text, &archive_name) {
        Some(h) => h,
        None => {
            let err = format!("SHA256SUMS 中未找到 {archive_name} 的校驗碼");
            log_update("ERROR", "VERIFY", &err);
            return Err(err);
        }
    };

    println!("🔒 正在驗證 SHA256 校驗碼...");
    if !verify_hash_hex(&actual_hash, &expected_hash) {
        let err = format!("SHA256 校驗失敗！預期 {expected_hash}，實際 {actual_hash}");
        log_update("ERROR", "VERIFY", &err);
        return Err(err);
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
        Ok(Err(e)) => return Err(e),
        Err(join_err) => return Err(format!("安裝任務執行異常: {join_err}")),
    }

    let _ = tmp_guard.cleanup();

    Ok(())
}

fn stop_running_dashboard_instances(install_dir: &Path) -> Result<(), String> {
    let my_pid = std::process::id();
    let mut target_pids = std::collections::HashSet::new();

    // 1. 從 .server.pid 讀取 PID
    let pid_file = install_dir.join(".server.pid");
    if let Ok(content) = fs::read_to_string(&pid_file) {
        if let Ok(pid) = content.trim().parse::<u32>() {
            if pid != my_pid && is_process_alive(pid) {
                target_pids.insert(pid);
            }
        }
    }
    let insights_pid_file = crate::db::get_insights_dir().join(".server.pid");
    if let Ok(content) = fs::read_to_string(&insights_pid_file) {
        if let Ok(pid) = content.trim().parse::<u32>() {
            if pid != my_pid && is_process_alive(pid) {
                if let Some(exe_path) = get_process_exe_path(pid) {
                    if matches_install_dir(&exe_path, install_dir) {
                        target_pids.insert(pid);
                    }
                } else {
                    target_pids.insert(pid);
                }
            }
        }
    }

    // 2. 透過系統進程清單掃描 APP_NAME 並比對安裝目錄
    #[cfg(unix)]
    {
        if let Ok(output) = std::process::Command::new("pgrep")
            .arg("-f")
            .arg(APP_NAME)
            .output()
        {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines() {
                if let Ok(pid) = line.trim().parse::<u32>() {
                    if pid != my_pid && is_process_alive(pid) {
                        if let Some(exe_path) = get_process_exe_path(pid) {
                            if matches_install_dir(&exe_path, install_dir) {
                                target_pids.insert(pid);
                            }
                        }
                    }
                }
            }
        }
    }

    #[cfg(windows)]
    {
        let exec_name = format!("{APP_NAME}.exe");
        if let Ok(output) = std::process::Command::new("tasklist")
            .args([
                "/FI",
                &format!("IMAGENAME eq {exec_name}"),
                "/FO",
                "CSV",
                "/NH",
            ])
            .output()
        {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines() {
                let parts: Vec<&str> = line.split(',').collect();
                if parts.len() >= 2 {
                    let pid_str = parts[1].trim().trim_matches('"');
                    if let Ok(pid) = pid_str.parse::<u32>() {
                        if pid != my_pid && is_process_alive(pid) {
                            if let Some(exe_path) = get_process_exe_path(pid) {
                                if matches_install_dir(&exe_path, install_dir) {
                                    target_pids.insert(pid);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let mut remaining_pids: Vec<u32> = target_pids.into_iter().collect();
    if remaining_pids.is_empty() {
        let _ = fs::remove_file(pid_file);
        let _ = fs::remove_file(insights_pid_file);
        return Ok(());
    }

    log_update(
        "INFO",
        "STOP_SERVICE",
        &format!("協調停止執行中之服務進程: {remaining_pids:?}"),
    );

    // 3. 發送初次溫和退出訊號 (Unix: SIGTERM, Windows: taskkill 無 /F)
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

    // 4. 積極輪詢並在逾時 2.5 秒後升級強制終止
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

    let _ = fs::remove_file(pid_file);
    let _ = fs::remove_file(insights_pid_file);
    log_update("INFO", "STOP_SERVICE", "所有執行中之服務進程已安全停止");
    Ok(())
}

pub(crate) fn apply_installation_with_rollback(
    release_root: &Path,
    install_dir: &Path,
    backup_dir: &Path,
) -> Result<(), String> {
    println!("⏸️ 正在協調停止現有執行中之服務...");
    stop_running_dashboard_instances(install_dir)?;

    println!("💾 正在備份現有安裝...");
    if let Err(e) = backup_installation(install_dir, backup_dir) {
        log_update("ERROR", "BACKUP", &e);
        return Err(e);
    }

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
                if target_exe.exists() {
                    let _ = fs::remove_file(&target_exe);
                }
                fs::copy(&src_exe, &target_exe).map_err(|e| format!("寫入新執行檔失敗: {e}"))?;
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
            ".install_marker",
        ] {
            let src = release_root.join(file);
            let dst = install_dir.join(file);
            if src.exists() {
                fs::copy(&src, &dst).map_err(|e| format!("替換檔案失敗 {file}: {e}"))?;
            }
        }

        // 確保自訂安裝目錄保有安裝標記，避免未來更新無法辨識
        fs::write(
            install_dir.join(".install_marker"),
            "token-usage-insights:installed",
        )
        .map_err(|e| format!("寫入安裝標記失敗: {e}"))?;

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

pub async fn check_and_auto_update_on_launch() {
    // 1. 防止循環重啟
    if std::env::var_os("_TOKEN_USAGE_INSIGHTS_RESTARTED").is_some() {
        return;
    }

    let args: Vec<String> = std::env::args().collect();

    // 2. 判斷環境
    let env_kind = detect_environment();
    if matches!(env_kind, EnvironmentKind::Npm { .. }) {
        // npm 環境：套用檢查間隔，若有新版僅提示並記錄本次檢查
        if args.iter().any(|arg| arg == "--no-auto-update") {
            return;
        }
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
        _ => {
            // 非標準安裝目錄（如 Git 開發目錄）：靜默跳過自動更新
            return;
        }
    };

    // 3. 若已有其他更新程序正在進行中，等待其完成並重啟，嚴禁同時啟動伺服器或干擾更新中之備份目錄
    if UpdateLock::is_locked(&install_dir) {
        println!("⏳ 偵測到已有更新程序正在進行中，等待更新完成...");
        log_update("INFO", "STARTUP_WAIT", "偵測到進行中的更新鎖，等待其釋放");
        match wait_for_lock_release(
            &install_dir,
            Duration::from_secs(STARTUP_AUTO_UPDATE_TOTAL_TIMEOUT_SECS),
        )
        .await
        {
            Ok(()) => {
                let backup_dir = install_dir.join(".backup");
                if backup_dir.join(".rollback_failed").exists() {
                    eprintln!("❌ 其他程序之更新回滾失敗；程序終止以保護狀態。");
                    log_update("ERROR", "STARTUP_FATAL", "其他程序回滾失敗，程序終止");
                    std::process::exit(1);
                }
                if backup_dir.exists() && backup_dir.join(".manifest").exists() {
                    eprintln!("⚠️ 偵測到另一程序更新中斷遺留之備份，進行自動救援還原...");
                    if let Err(re) = restore_from_backup(&backup_dir, &install_dir) {
                        eprintln!("❌ 救援還原失敗: {re}；程序終止。");
                        std::process::exit(1);
                    }
                    let _ = fs::remove_dir_all(&backup_dir);
                }
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

    // 4. 此時目錄已確認無進行中更新鎖，檢查先前更新之殘留中斷狀態與回滾保護
    let backup_dir = install_dir.join(".backup");
    if backup_dir.join(".rollback_failed").exists() {
        eprintln!(
            "❌ 偵測到先前更新回滾失敗標記 ({backup_dir:?})；為防止讀取損毀檔案，程序終止。請依備份手動還原。"
        );
        log_update("ERROR", "STARTUP_FATAL", "先前回滾失敗，程序終止");
        std::process::exit(1);
    }
    if backup_dir.exists() && backup_dir.join(".manifest").exists() {
        println!("⚠️ 偵測到未完成之中斷更新備份，正在自動救援還原至健全版本...");
        log_update(
            "WARN",
            "STARTUP_RECOVERY",
            "偵測到中斷更新備份，執行自動救援還原",
        );
        if let Err(e) = restore_from_backup(&backup_dir, &install_dir) {
            eprintln!("❌ 自動救援還原失敗: {e}；程序終止以保護狀態。");
            log_update("ERROR", "STARTUP_FATAL", &format!("救援還原失敗: {e}"));
            std::process::exit(1);
        }
        let _ = fs::remove_dir_all(&backup_dir);
        println!("✅ 已成功自動還原至健全版本，正在重新啟動 Token 戰情室...");
        log_update("INFO", "STARTUP_RECOVERY", "自動救援還原成功，重啟進程");
        restart_current_process(&args);
    }

    // 5. 檢查是否關閉自動更新（命令列旗標 > 環境變數 > config.yaml）
    // 注意：此旗標僅關閉後續的檢查與更新發起，不得繞過上述之更新鎖定與救援協調
    if args.iter().any(|arg| arg == "--no-auto-update") {
        return;
    }
    if let Ok(val) = std::env::var("TOKEN_USAGE_INSIGHTS_AUTO_UPDATE") {
        let lower = val.trim().to_lowercase();
        if lower == "0" || lower == "false" || lower == "no" || lower == "off" {
            return;
        }
    } else {
        let (yaml_auto, _) = load_update_config();
        if yaml_auto == Some(false) {
            return;
        }
    }

    // 6. 檢查更新檢查間隔（優先讀取環境變數，其次 config.yaml，預設 24 小時）
    if !is_update_check_interval_elapsed() {
        return;
    }

    // 7. 快速檢查（設定超時 STARTUP_CHECK_TIMEOUT_SECS 秒，不阻礙伺服器啟動）
    let release = match fetch_release_with_logging(None, STARTUP_CHECK_TIMEOUT_SECS).await {
        Ok(r) => r,
        Err(e) => {
            log_update("WARN", "STARTUP_CHECK", &format!("啟動更新檢查略過: {e}"));
            return;
        }
    };

    let current_version = env!("CARGO_PKG_VERSION");
    if !is_newer_version(&release.tag_name, current_version) {
        // 沒有新版本：成功確認當前已是最新，記錄本次檢查時間
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

    // 為自動更新設定整體時間上限，避免慢速網路長時間阻塞伺服器啟動
    let update_result = tokio::time::timeout(
        Duration::from_secs(STARTUP_AUTO_UPDATE_TOTAL_TIMEOUT_SECS),
        run_update(update_opts),
    )
    .await;

    match update_result {
        Ok(Ok(())) => {
            if let Ok(conn) = crate::db::get_db_conn() {
                let now_str = Utc::now().to_rfc3339();
                let _ = crate::db::set_system_metadata(&conn, LAST_CHECK_KEY, &now_str);
            }

            println!("🔄 更新完成，正在自動重啟 Token 戰情室...");
            log_update("INFO", "STARTUP_RESTART", "更新完成，重啟進程");
            restart_current_process(&args);
        }
        Ok(Err(e)) => {
            if is_lock_conflict_error(&e) {
                println!("⏳ 偵測到已有更新程序正在進行中，等待更新完成...");
                log_update("INFO", "STARTUP_WAIT", "遇到更新鎖競爭，等待另一程序完成");
                match wait_for_lock_release(
                    &install_dir,
                    Duration::from_secs(STARTUP_AUTO_UPDATE_TOTAL_TIMEOUT_SECS),
                )
                .await
                {
                    Ok(()) => {
                        let backup_dir = install_dir.join(".backup");
                        if backup_dir.join(".rollback_failed").exists() {
                            eprintln!("❌ 其他程序之更新回滾失敗；程序終止以保護狀態。");
                            log_update("ERROR", "STARTUP_FATAL", "其他程序回滾失敗，程序終止");
                            std::process::exit(1);
                        }
                        if backup_dir.exists() && backup_dir.join(".manifest").exists() {
                            eprintln!("⚠️ 偵測到另一程序更新中斷遺留之備份，進行自動救援還原...");
                            if let Err(re) = restore_from_backup(&backup_dir, &install_dir) {
                                eprintln!("❌ 救援還原失敗: {re}；程序終止。");
                                std::process::exit(1);
                            }
                            let _ = fs::remove_dir_all(&backup_dir);
                        }
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
                let backup_dir = install_dir.join(".backup");
                if backup_dir.join(".rollback_failed").exists() {
                    eprintln!(
                        "❌ 自動更新回滾失敗 ({backup_dir:?})；為防止讀取損毀狀態，程序終止。"
                    );
                    log_update("ERROR", "STARTUP_FATAL", "回滾失敗，程序終止");
                    std::process::exit(1);
                }
                if backup_dir.exists() && backup_dir.join(".manifest").exists() {
                    eprintln!("⚠️ 偵測到更新中斷殘留備份，進行自動救援還原...");
                    if let Err(re) = restore_from_backup(&backup_dir, &install_dir) {
                        eprintln!("❌ 還原備份失敗: {re}；程序終止以保護狀態。");
                        log_update("ERROR", "STARTUP_FATAL", &format!("自動還原失敗: {re}"));
                        std::process::exit(1);
                    }
                    let _ = fs::remove_dir_all(&backup_dir);
                    println!("✅ 已自備份還原完成。");
                }
                eprintln!("⚠️ 自動更新失敗: {e}，將繼續以現有健全版本啟動服務。");
                log_update("WARN", "STARTUP_UPDATE", &format!("自動更新失敗: {e}"));
            }
        }
        Err(_) => {
            eprintln!(
                "⚠️ 自動更新逾時（超過 {STARTUP_AUTO_UPDATE_TOTAL_TIMEOUT_SECS} 秒），將繼續以現有版本啟動服務。"
            );
            log_update(
                "WARN",
                "STARTUP_UPDATE",
                &format!("自動更新超過上限 {STARTUP_AUTO_UPDATE_TOTAL_TIMEOUT_SECS} 秒，略過"),
            );
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
}
