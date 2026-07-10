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

fn expand_common_prefix(path: PathBuf) -> PathBuf {
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
        if let Some(executable_dir) = executable.parent() {
            roots.extend(executable_dir.ancestors().take(5).map(Path::to_path_buf));
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

#[cfg(test)]
mod tests {
    // Only used by the Windows-specific test below; avoids an unused-import
    // warning (which is a hard error under `-D warnings`) on other platforms.
    #[cfg(windows)]
    use super::*;

    #[cfg(windows)]
    #[test]
    fn native_windows_drive_and_unc_paths_are_preserved() {
        let drive_path = PathBuf::from(r"C:\Users\測試 使用者\.codex\sessions");
        let unc_path = PathBuf::from(r"\\server\AI Data\使用量");

        assert_eq!(expand_common_prefix(drive_path.clone()), drive_path);
        assert_eq!(expand_common_prefix(unc_path.clone()), unc_path);
    }
}
