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

    // 2. 檢查標準安裝目錄
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

    // 3. 檢查是否在 Git 或 Cargo 開發原始碼目錄
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
            let file = parts[1].trim_start_matches('*').trim();
            if file == target_filename && hash.len() == 64 {
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

fn backup_installation(install_dir: &Path, backup_dir: &Path) -> Result<(), String> {
    if backup_dir.exists() {
        let _ = fs::remove_dir_all(backup_dir);
    }
    fs::create_dir_all(backup_dir).map_err(|e| format!("建立備份目錄失敗: {e}"))?;

    let backup_items = [
        APP_NAME,
        #[cfg(windows)]
        "token-usage-insights.exe",
        "static",
        "pricing.csv",
        "shell",
        "scripts",
        "VERSION",
        "README.md",
        "LICENSE",
    ];

    for item in backup_items {
        let src = install_dir.join(item);
        let dst = backup_dir.join(item);
        if src.is_dir() {
            copy_dir_recursive(&src, &dst)?;
        } else if src.is_file() {
            fs::copy(&src, &dst).map_err(|e| format!("備份檔案失敗 {item}: {e}"))?;
        }
    }

    Ok(())
}

fn restore_from_backup(backup_dir: &Path, install_dir: &Path) -> Result<(), String> {
    if !backup_dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(backup_dir).map_err(|e| format!("讀取備份目錄失敗: {e}"))? {
        let entry = entry.map_err(|e| format!("讀取備份項目失敗: {e}"))?;
        let src = entry.path();
        let dst = install_dir.join(entry.file_name());
        if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            let _ = fs::remove_dir_all(&dst);
            copy_dir_recursive(&src, &dst)?;
        } else {
            let _ = fs::copy(&src, &dst);
        }
    }
    Ok(())
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
                let release = fetch_release(options.target_version.as_deref(), 15).await?;
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
        EnvironmentKind::GitOrDev { root, .. } if !options.force && !options.check_only => {
            let msg = format!(
                "錯誤：目前執行檔位於開發目錄中 ({root:?})，不支援直接更新。\n請使用 git pull / cargo build，或透過 --force 強制執行。"
            );
            eprintln!("{msg}");
            log_update("ERROR", "CHECK", &format!("拒絕更新：開發目錄 {root:?}"));
            return Err("開發目錄不支援自我更新".to_string());
        }
        EnvironmentKind::Other { exe_path } if !options.force && !options.check_only => {
            let msg = format!(
                "錯誤：目前執行檔位於非標準安裝目錄 ({exe_path:?})。\n請在標準安裝目錄中執行，或透過 --force 強制執行。"
            );
            eprintln!("{msg}");
            log_update(
                "ERROR",
                "CHECK",
                &format!("拒絕更新：非標準目錄 {exe_path:?}"),
            );
            return Err("非標準目錄不支援自我更新".to_string());
        }
        _ => {}
    }

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

    let release = fetch_release(options.target_version.as_deref(), 15).await?;
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

    let install_dir = match &env_kind {
        EnvironmentKind::StandardInstalled { install_dir, .. } => install_dir.clone(),
        EnvironmentKind::GitOrDev { root, .. } => root.clone(),
        EnvironmentKind::Other { exe_path } => exe_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf(),
        EnvironmentKind::Npm { exe_path } => exe_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf(),
    };

    let update_tmp_dir = crate::db::get_insights_dir().join(".update-tmp");
    let _ = fs::remove_dir_all(&update_tmp_dir);
    fs::create_dir_all(&update_tmp_dir).map_err(|e| format!("建立暫存目錄失敗: {e}"))?;

    println!("⬇️ 正在下載發行包: {archive_name} ...");
    log_update("INFO", "DOWNLOAD", &format!("開始下載 {archive_name}"));
    let archive_bytes = download_bytes(&asset.browser_download_url, 60).await?;

    println!("⬇️ 正在下載校驗檔 SHA256SUMS ...");
    let sums_bytes = download_bytes(&checksum_asset.browser_download_url, 15).await?;
    let sums_text = String::from_utf8_lossy(&sums_bytes);

    let expected_hash = parse_checksum(&sums_text, &archive_name).ok_or_else(|| {
        let err = format!("SHA256SUMS 中未找到 {archive_name} 的校驗碼");
        log_update("ERROR", "VERIFY", &err);
        err
    })?;

    println!("🔒 正在驗證 SHA256 校驗碼...");
    if !verify_sha256(&archive_bytes, &expected_hash) {
        let err = format!("SHA256 校驗失敗！預期 {expected_hash}");
        log_update("ERROR", "VERIFY", &err);
        return Err(err);
    }
    println!("✅ SHA256 校驗通過！");
    log_update("INFO", "VERIFY", "SHA256 校驗通過");

    let is_zip = archive_name.ends_with(".zip");
    let extract_dir = update_tmp_dir.join("extracted");
    println!("📦 正在解壓縮檔案...");
    extract_archive(&archive_bytes, &extract_dir, is_zip)?;

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
        found.ok_or_else(|| "解壓後的目錄中未找到執行檔".to_string())?
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

        #[cfg(windows)]
        {
            let old_exe = install_dir.join(format!("{APP_NAME}.exe.old"));
            let _ = fs::remove_file(&old_exe);
            if target_exe.exists() {
                fs::rename(&target_exe, &old_exe)
                    .map_err(|e| format!("Windows 執行檔換名失敗: {e}"))?;
            }
            fs::copy(&src_exe, &target_exe).map_err(|e| format!("寫入新執行檔失敗: {e}"))?;
        }

        #[cfg(not(windows))]
        {
            if target_exe.exists() {
                let _ = fs::remove_file(&target_exe);
            }
            fs::copy(&src_exe, &target_exe).map_err(|e| format!("寫入新執行檔失敗: {e}"))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&target_exe, fs::Permissions::from_mode(0o755));
            }
        }

        for folder in ["static", "shell", "scripts"] {
            let src = release_root.join(folder);
            let dst = install_dir.join(folder);
            if src.exists() {
                let _ = fs::remove_dir_all(&dst);
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
        let _ = restore_from_backup(&backup_dir, &install_dir);
        let _ = fs::remove_dir_all(&backup_dir);
        let _ = fs::remove_dir_all(&update_tmp_dir);
        return Err(err);
    }

    let _ = fs::remove_dir_all(&backup_dir);
    let _ = fs::remove_dir_all(&update_tmp_dir);

    println!("🎉 成功更新至版本 {remote_version}！");
    log_update("INFO", "INSTALL", &format!("成功更新至 {remote_version}"));

    Ok(())
}

pub async fn check_and_auto_update_on_launch() {
    // 1. 防止循環重啟
    if std::env::var_os("_TOKEN_USAGE_INSIGHTS_RESTARTED").is_some() {
        return;
    }

    // 2. 檢查是否關閉自動更新
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--no-auto-update") {
        return;
    }
    if let Ok(val) = std::env::var("TOKEN_USAGE_INSIGHTS_AUTO_UPDATE") {
        let lower = val.trim().to_lowercase();
        if lower == "0" || lower == "false" || lower == "no" || lower == "off" {
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

    // 4. 檢查更新檢查間隔（預設 24 小時）
    let interval_hours: i64 = std::env::var("TOKEN_USAGE_INSIGHTS_UPDATE_INTERVAL_HOURS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_UPDATE_INTERVAL_HOURS);

    if let Ok(conn) = crate::db::get_db_conn() {
        if let Ok(Some(last_check_str)) = crate::db::get_system_metadata(&conn, LAST_CHECK_KEY) {
            if let Ok(last_check) = chrono::DateTime::parse_from_rfc3339(&last_check_str) {
                let elapsed_secs = Utc::now().timestamp() - last_check.timestamp();
                if elapsed_secs >= 0 && elapsed_secs < interval_hours * 3600 {
                    return;
                }
            }
        }
        let now_str = Utc::now().to_rfc3339();
        let _ = crate::db::set_system_metadata(&conn, LAST_CHECK_KEY, &now_str);
    }

    // 5. 快速檢查（設定超時，不阻礙伺服器啟動）
    let release = match fetch_release(None, STARTUP_CHECK_TIMEOUT_SECS).await {
        Ok(r) => r,
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
4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945  token-usage-insights-v0.9.5-aarch64-apple-darwin.tar.gz
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 *token-usage-insights-v0.9.5-x86_64-apple-darwin.tar.gz
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
}
