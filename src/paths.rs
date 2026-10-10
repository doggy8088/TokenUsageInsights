use std::path::{Path, PathBuf};

/// Resolve a configured path without requiring it to exist yet.
///
/// Besides native absolute/relative paths, this accepts the common `~`, `$HOME`,
/// and `%USERPROFILE%` prefixes so values copied from shell configuration work on
/// Windows as well as Unix-like systems.
pub fn env_path(name: &str) -> Option<PathBuf> {
    let value = std::env::var_os(name)?;
    if value.is_empty() {
        return None;
    }

    Some(expand_common_prefix(PathBuf::from(value)))
}

/// 去除 Windows `canonicalize` 產生的 verbatim 前綴（`\\?\C:\...` 與 `\\?\UNC\server\share`），
/// 讓顯示給使用者、寫進 `settings.json` 的路徑維持一般形式。非 Windows 路徑原樣回傳。
pub(crate) fn strip_windows_verbatim_prefix(path: PathBuf) -> PathBuf {
    let raw = path.to_string_lossy();
    if let Some(rest) = raw.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = raw.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    path
}

pub(crate) fn expand_common_prefix(path: PathBuf) -> PathBuf {
    let raw = path.to_string_lossy();
    let home = dirs::home_dir();

    if raw == "~" || raw.eq_ignore_ascii_case("$HOME") {
        return home.unwrap_or(path);
    }

    for prefix in ["~/", "~\\", "$HOME/", "$HOME\\"] {
        if let Some(rest) = raw.strip_prefix(prefix) {
            if let Some(home) = &home {
                return home.join(rest);
            }
        }
    }

    for variable in ["USERPROFILE", "LOCALAPPDATA", "APPDATA"] {
        let prefix = format!("%{variable}%");
        if raw.eq_ignore_ascii_case(&prefix) {
            if let Some(value) = std::env::var_os(variable) {
                return PathBuf::from(value);
            }
        }

        for separator in ['/', '\\'] {
            let prefix_with_separator = format!("{prefix}{separator}");
            if raw
                .get(..prefix_with_separator.len())
                .map(|candidate| candidate.eq_ignore_ascii_case(&prefix_with_separator))
                .unwrap_or(false)
            {
                if let Some(value) = std::env::var_os(variable) {
                    return PathBuf::from(value).join(&raw[prefix_with_separator.len()..]);
                }
            }
        }
    }

    path
}

/// Locate a release resource independently of the process working directory.
pub fn find_resource(relative: impl AsRef<Path>) -> Option<PathBuf> {
    let relative = relative.as_ref();
    if relative.is_absolute() && relative.exists() {
        return Some(relative.to_path_buf());
    }

    let mut roots = Vec::new();
    if let Some(manifest_dir) = std::env::var_os("CARGO_MANIFEST_DIR") {
        roots.push(PathBuf::from(manifest_dir));
    }
    if let Some(manifest_dir) = option_env!("CARGO_MANIFEST_DIR") {
        roots.push(PathBuf::from(manifest_dir));
    }
    if let Ok(executable) = std::env::current_exe() {
        // ~/.local/bin 內是指向安裝目錄的 symlink；先解析 symlink 才能找到
        // 與真實執行檔同層的資源目錄。
        let resolved = std::fs::canonicalize(&executable).unwrap_or_else(|_| executable.clone());
        let mut exes = vec![&resolved];
        if resolved != executable {
            exes.push(&executable);
        }
        for exe in exes {
            if let Some(executable_dir) = exe.parent() {
                roots.extend(executable_dir.ancestors().take(5).map(Path::to_path_buf));
            }
        }
    }
    if let Ok(current_dir) = std::env::current_dir() {
        roots.push(current_dir);
    }

    roots
        .into_iter()
        .map(|root| root.join(relative))
        .find(|candidate| candidate.exists())
}

/// Resolve the Copilot App (Tauri desktop) data directory.
///
/// Honors `COPILOT_APP_DIR` first, then falls back to `COPILOT_DIR`, then to
/// `~/.copilot`. The directory is expected to contain `data.db` and
/// `session-store.db` written by the Copilot App.
pub fn copilot_app_dir() -> PathBuf {
    if let Some(path) = env_path("COPILOT_APP_DIR") {
        return path;
    }
    if let Some(path) = env_path("COPILOT_DIR") {
        return path;
    }
    dirs::home_dir()
        .map(|h| h.join(".copilot"))
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    // Only used by the Windows-specific test below; avoids an unused-import
    // warning (which is a hard error under `-D warnings`) on other platforms.
    #[cfg(windows)]
    use super::*;

    #[test]
    fn verbatim_drive_prefix_is_stripped() {
        let stripped = super::strip_windows_verbatim_prefix(std::path::PathBuf::from(
            r"\\?\C:\Users\YOUR_NAME\.copilot",
        ));
        assert_eq!(stripped.to_string_lossy(), r"C:\Users\YOUR_NAME\.copilot");
    }

    #[test]
    fn verbatim_unc_prefix_is_rewritten_to_plain_unc() {
        let stripped = super::strip_windows_verbatim_prefix(std::path::PathBuf::from(
            r"\\?\UNC\server\share\.copilot",
        ));
        assert_eq!(stripped.to_string_lossy(), r"\\server\share\.copilot");
    }

    #[test]
    fn plain_paths_are_left_untouched() {
        for raw in [
            r"C:\Users\YOUR_NAME",
            "/home/user/.copilot",
            r"\\server\share",
        ] {
            let stripped = super::strip_windows_verbatim_prefix(std::path::PathBuf::from(raw));
            assert_eq!(stripped.to_string_lossy(), raw);
        }
    }

    #[cfg(windows)]
    #[test]
    fn native_windows_drive_and_unc_paths_are_preserved() {
        let drive_path = PathBuf::from(r"C:\Users\測試 使用者\.codex\sessions");
        let unc_path = PathBuf::from(r"\\server\AI Data\使用量");

        assert_eq!(expand_common_prefix(drive_path.clone()), drive_path);
        assert_eq!(expand_common_prefix(unc_path.clone()), unc_path);
    }
}
