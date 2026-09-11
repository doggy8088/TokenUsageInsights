use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Utc;
use serde::Deserialize;
use sha2::{Digest, Sha256};

const GITHUB_OWNER: &str = "doggy8088";
const GITHUB_REPO: &str = "TokenUsageInsights";
const APP_NAME: &str = "token-usage-insights";
const USER_AGENT: &str = "token-usage-insights-updater";
const DEFAULT_UPDATE_INTERVAL_HOURS: i64 = 24;
const STARTUP_CHECK_TIMEOUT_SECS: u64 = 4;
const LAST_CHECK_KEY: &str = "last_update_check_at";

#[derive(Debug, Clone, Default)]
pub struct UpdateOptions {
    pub check_only: bool,
    pub force: bool,
    pub target_version: Option<String>,
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

#[derive(Deserialize, Debug)]
pub struct GitHubAsset {
    pub name: String,
    pub browser_download_url: String,
}

#[derive(Deserialize, Debug)]
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

pub fn standard_install_dir() -> PathBuf {
    if let Some(custom) = crate::paths::env_path("TOKEN_USAGE_INSIGHTS_INSTALL_DIR") {
        return custom;
    }

    #[cfg(windows)]
    {
        if let Some(local_app_data) = dirs::data_local_dir() {
            return local_app_data.join("TokenUsageInsights");
        }
    }

    #[cfg(not(windows))]
    {
        if let Some(home) = dirs::home_dir() {
            return home.join(".local").join("share").join(APP_NAME);
        }
    }

    PathBuf::from(".")
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
    let std_dir = standard_install_dir();
    let canonical_std_dir = fs::canonicalize(&std_dir).unwrap_or(std_dir);
    if let Ok(canonical_exe_dir) = fs::canonicalize(exe_dir) {
        if canonical_exe_dir == canonical_std_dir {
            return EnvironmentKind::StandardInstalled {
                install_dir: canonical_std_dir,
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

pub fn verify_sha256(bytes: &[u8], expected_hex: &str) -> bool {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let result = hasher.finalize();
    let actual_hex = hex::encode(result).to_lowercase();
    actual_hex == expected_hex.trim().to_lowercase()
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

struct UpdateLock {
    lock_path: PathBuf,
}

impl UpdateLock {
    fn try_acquire(install_dir: &Path) -> Result<Self, String> {
        let lock_path = install_dir.join(".update.lock");
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
                if let Ok(metadata) = fs::metadata(&lock_path) {
                    if let Ok(modified) = metadata.modified() {
                        if let Ok(elapsed) = modified.elapsed() {
                            if elapsed > Duration::from_secs(600) {
                                let _ = fs::remove_file(&lock_path);
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
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).map_err(|e| format!("建立暫存目錄失敗: {e}"))?;
        Ok(Self { path })
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn extract_archive(bytes: &[u8], dest_dir: &Path, is_zip: bool) -> Result<(), String> {
    fs::create_dir_all(dest_dir).map_err(|e| format!("建立解壓縮目錄失敗: {e}"))?;

    if is_zip {
        let cursor = std::io::Cursor::new(bytes);
        let mut archive =
            zip::ZipArchive::new(cursor).map_err(|e| format!("解析 ZIP 封裝失敗: {e}"))?;
        archive
            .extract(dest_dir)
            .map_err(|e| format!("解壓縮 ZIP 檔案失敗: {e}"))?;
    } else {
        let cursor = std::io::Cursor::new(bytes);
        let tar_gz = flate2::read::GzDecoder::new(cursor);
        let mut archive = tar::Archive::new(tar_gz);
        archive
            .unpack(dest_dir)
            .map_err(|e| format!("解壓縮 tar.gz 檔案失敗: {e}"))?;
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
];

fn backup_installation(install_dir: &Path, backup_dir: &Path) -> Result<(), String> {
    if backup_dir.exists() {
        let err = format!(
            "偵測到先前更新留存的備份目錄 {:?}；為保護先前版本，已停止更新。請手動還原或移除該備份目錄後再試。",
            backup_dir
        );
        return Err(err);
    }
    fs::create_dir_all(backup_dir).map_err(|e| format!("建立備份目錄失敗: {e}"))?;

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
}

fn restore_from_backup(backup_dir: &Path, install_dir: &Path) -> Result<(), String> {
    if !backup_dir.exists() {
        return Ok(());
    }

    let manifest_path = backup_dir.join(".manifest");
    let original_items: std::collections::HashSet<String> = if manifest_path.exists() {
        let content = fs::read_to_string(&manifest_path).unwrap_or_default();
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
                if let Err(e) = fs::remove_dir_all(&path) {
                    log_update(
                        "WARN",
                        "ROLLBACK",
                        &format!("清理新增目錄失敗 {path:?}: {e}"),
                    );
                }
            } else if path.is_file() {
                if let Err(e) = fs::remove_file(&path) {
                    log_update(
                        "WARN",
                        "ROLLBACK",
                        &format!("清理新增檔案失敗 {path:?}: {e}"),
                    );
                }
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

async fn download_bytes(url: &str, timeout_secs: u64) -> Result<Vec<u8>, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .build()
        .map_err(|e| format!("建立 HTTP 用戶端失敗: {e}"))?;

    let resp = client
        .get(url)
        .header("User-Agent", USER_AGENT)
        .send()
        .await
        .map_err(|e| format!("下載失敗 ({url}): {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("下載失敗 HTTP {} ({url})", resp.status()));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("讀取下載內容失敗: {e}"))?;
    Ok(bytes.to_vec())
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
                let is_newer = is_newer_version(remote_version, current_version);
                println!("  目前版本: v{current_version}");
                println!("  最新版本: {remote_version}");
                if is_newer {
                    println!("💡 發現新版本！可執行 npx token-usage-insights@latest 使用最新版。");
                } else {
                    println!("✅ 目前已是最新版本。");
                }
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
                let is_newer = is_newer_version(remote_version, current_version);
                println!("  目前版本: v{current_version}");
                println!("  最新版本: {remote_version}");
                if is_newer {
                    println!("💡 發現新版本！請使用 git pull / cargo build 進行更新。");
                } else {
                    println!("✅ 目前已是最新版本。");
                }
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
                let is_newer = is_newer_version(remote_version, current_version);
                println!("  目前版本: v{current_version}");
                println!("  最新版本: {remote_version}");
                if is_newer {
                    println!("💡 發現新版本！請在標準安裝目錄中執行更新。");
                } else {
                    println!("✅ 目前已是最新版本。");
                }
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

    let target = current_target_triple().ok_or_else(|| {
        let msg = "目前作業系統或硬體架構不支援預先編譯的二進位發行檔".to_string();
        log_update("ERROR", "CHECK", &msg);
        msg
    })?;

    let current_version = env!("CARGO_PKG_VERSION");
    println!("🔍 正在檢查最新發行版本...");
    log_update(
        "INFO",
        "CHECK",
        &format!("開始檢查更新（目前版本 v{current_version}）"),
    );

    let release = fetch_release_with_logging(options.target_version.as_deref(), 15).await?;
    let remote_version = release.tag_name.trim();

    let is_newer = is_newer_version(remote_version, current_version);
    println!("  目前版本: v{current_version}");
    println!("  目標版本: {remote_version}");

    if options.check_only {
        if is_newer {
            println!("💡 發現新版本！可執行 token-usage-insights update 進行更新。");
        } else {
            println!("✅ 目前已是最新版本。");
        }
        return Ok(());
    }

    if !is_newer && !options.force && options.target_version.is_none() {
        println!("✅ 目前已是最新版本 ({remote_version})。使用 --force 可強制重新安裝。");
        log_update("INFO", "CHECK", "已是最新版本，略過更新");
        return Ok(());
    }

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

    // 取得安裝目錄之獨占鎖
    let _lock = UpdateLock::try_acquire(&install_dir)?;

    // 使用 TempDirGuard 確保異常離開時自動清理暫存
    let update_tmp_dir = crate::db::get_insights_dir().join(".update-tmp");
    let tmp_guard = TempDirGuard::new(update_tmp_dir)?;

    println!("⬇️ 正在下載發行包: {archive_name} ...");
    log_update("INFO", "DOWNLOAD", &format!("開始下載 {archive_name}"));
    let archive_bytes = match download_bytes(&asset.browser_download_url, 60).await {
        Ok(b) => b,
        Err(e) => {
            log_update("ERROR", "DOWNLOAD", &e);
            return Err(e);
        }
    };

    println!("⬇️ 正在下載校驗檔 SHA256SUMS ...");
    let sums_bytes = match download_bytes(&checksum_asset.browser_download_url, 15).await {
        Ok(b) => b,
        Err(e) => {
            log_update("ERROR", "DOWNLOAD", &e);
            return Err(e);
        }
    };
    let sums_text = String::from_utf8_lossy(&sums_bytes);

    let expected_hash = match parse_checksum(&sums_text, &archive_name) {
        Some(h) => h,
        None => {
            let err = format!("SHA256SUMS 中未找到 {archive_name} 的校驗碼");
            log_update("ERROR", "VERIFY", &err);
            return Err(err);
        }
    };

    println!("🔒 正在驗證 SHA256 校驗碼...");
    if !verify_sha256(&archive_bytes, &expected_hash) {
        let err = format!("SHA256 校驗失敗！預期 {expected_hash}");
        log_update("ERROR", "VERIFY", &err);
        return Err(err);
    }
    println!("✅ SHA256 校驗通過！");
    log_update("INFO", "VERIFY", "SHA256 校驗通過");

    let is_zip = archive_name.ends_with(".zip");
    let extract_dir = tmp_guard.path.join("extracted");
    println!("📦 正在解壓縮檔案...");
    if let Err(e) = extract_archive(&archive_bytes, &extract_dir, is_zip) {
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
                    if sub.join(APP_NAME).exists() || sub.join(format!("{APP_NAME}.exe")).exists() {
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

    println!("💾 正在備份現有安裝...");
    let backup_dir = install_dir.join(".backup");
    if let Err(e) = backup_installation(&install_dir, &backup_dir) {
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
                if dst.exists() {
                    fs::remove_dir_all(&dst).map_err(|e| format!("清除舊目錄失敗 {dst:?}: {e}"))?;
                }
                copy_dir_recursive(&src, &dst)?;
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
                fs::copy(&src, &dst).map_err(|e| format!("替換檔案失敗 {file}: {e}"))?;
            }
        }

        Ok(())
    })();

    if let Err(err) = install_result {
        eprintln!("❌ 安裝失敗，正在自動回滾: {err}");
        log_update("ERROR", "INSTALL", &format!("安裝失敗: {err}，開始回滾"));
        if let Err(rollback_err) = restore_from_backup(&backup_dir, &install_dir) {
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
            let _ = fs::remove_dir_all(&backup_dir);
        }
        return Err(err);
    }

    let _ = fs::remove_dir_all(&backup_dir);

    println!("🎉 成功更新至版本 {remote_version}！");
    log_update("INFO", "INSTALL", &format!("成功更新至 {remote_version}"));

    Ok(())
}

pub async fn check_and_auto_update_on_launch() {
    // 1. 防止循環重啟
    if std::env::var_os("_TOKEN_USAGE_INSIGHTS_RESTARTED").is_some() {
        return;
    }

    // 2. 檢查是否關閉自動更新（命令列旗標 > 環境變數 > config.yaml）
    let args: Vec<String> = std::env::args().collect();
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

    // 3. 判斷環境
    let env_kind = detect_environment();
    if matches!(env_kind, EnvironmentKind::Npm { .. }) {
        // npm 環境：快速檢查是否有新版，若有僅在終端機提示
        if let Ok(release) = fetch_release(None, 2).await {
            let current_version = env!("CARGO_PKG_VERSION");
            if is_newer_version(&release.tag_name, current_version) {
                println!(
                    "💡 發現新版本 {}！您可以執行 npx token-usage-insights@latest 啟動最新版本。",
                    release.tag_name
                );
            }
        }
        return;
    }

    if !matches!(env_kind, EnvironmentKind::StandardInstalled { .. }) {
        // 非標準安裝目錄（如 Git 開發目錄）：靜默跳過自動更新
        return;
    }

    // 4. 檢查更新檢查間隔（優先讀取環境變數，其次 config.yaml，預設 24 小時）
    let (_, yaml_interval_days) = load_update_config();
    let default_hours = yaml_interval_days
        .map(|d| d * 24)
        .unwrap_or(DEFAULT_UPDATE_INTERVAL_HOURS);
    let interval_hours: i64 = std::env::var("TOKEN_USAGE_INSIGHTS_UPDATE_INTERVAL_HOURS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default_hours);

    if let Ok(conn) = crate::db::get_db_conn() {
        if let Ok(Some(last_check_str)) = crate::db::get_system_metadata(&conn, LAST_CHECK_KEY) {
            if let Ok(last_check) = chrono::DateTime::parse_from_rfc3339(&last_check_str) {
                let elapsed_secs = Utc::now().timestamp() - last_check.timestamp();
                if elapsed_secs >= 0 && elapsed_secs < interval_hours * 3600 {
                    return;
                }
            }
        }
    }

    // 5. 快速檢查（設定超時，不阻礙伺服器啟動）
    let release = match fetch_release(None, STARTUP_CHECK_TIMEOUT_SECS).await {
        Ok(r) => {
            if let Ok(conn) = crate::db::get_db_conn() {
                let now_str = Utc::now().to_rfc3339();
                let _ = crate::db::set_system_metadata(&conn, LAST_CHECK_KEY, &now_str);
            }
            r
        }
        Err(e) => {
            log_update("WARN", "STARTUP_CHECK", &format!("啟動更新檢查略過: {e}"));
            return;
        }
    };

    let current_version = env!("CARGO_PKG_VERSION");
    if !is_newer_version(&release.tag_name, current_version) {
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
    };

    if let Err(e) = run_update(update_opts).await {
        eprintln!("⚠️ 自動更新失敗: {e}，將繼續以現有版本啟動服務。");
        log_update("WARN", "STARTUP_UPDATE", &format!("自動更新失敗: {e}"));
        return;
    }

    println!("🔄 更新完成，正在自動重啟 Token 戰情室...");
    log_update("INFO", "STARTUP_RESTART", "更新完成，重啟進程");

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
        eprintln!("❌ 自動重啟進程失敗: {err}");
    }

    #[cfg(windows)]
    {
        let current_exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from(&args[0]));
        let mut cmd = std::process::Command::new(current_exe);
        if args.len() > 1 {
            cmd.args(&args[1..]);
        }
        cmd.env("_TOKEN_USAGE_INSIGHTS_RESTARTED", "1");
        match cmd.spawn() {
            Ok(_) => std::process::exit(0),
            Err(err) => eprintln!("❌ 自動重啟進程失敗: {err}"),
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

        // Re-run backup should fail because backup_dir already exists
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
    fn update_lock_prevents_concurrent_access() {
        let temp = std::env::temp_dir().join(format!(
            "test-lock-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        fs::create_dir_all(&temp).unwrap();

        let lock1 = UpdateLock::try_acquire(&temp);
        assert!(lock1.is_ok());

        let lock2 = UpdateLock::try_acquire(&temp);
        assert!(lock2.is_err());

        drop(lock1);

        let lock3 = UpdateLock::try_acquire(&temp);
        assert!(lock3.is_ok());

        drop(lock3);
        let _ = fs::remove_dir_all(&temp);
    }
}
