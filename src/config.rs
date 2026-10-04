use serde::Deserialize;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

/// Keys deliberately match dashboard assistant identifiers, with separate
/// entries for the two Copilot desktop collectors.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Harness {
    Antigravity,
    Copilot,
    CopilotApp,
    Vscode,
    Codex,
    Claude,
    Cursor,
    Grok,
    Pi,
    Omp,
    Muse,
    Mcode,
}

impl Harness {
    pub(crate) fn primary_root(self) -> Option<PathBuf> {
        use crate::db;
        Some(match self {
            Self::Antigravity => db::get_antigravity_dir(),
            Self::Copilot => db::get_copilot_dir(),
            Self::CopilotApp => crate::paths::copilot_app_dir(),
            Self::Vscode => return None, // VS Code discovers multiple platform defaults.
            Self::Codex => db::get_codex_dir(),
            Self::Claude => db::get_claude_dir(),
            Self::Cursor => db::get_cursor_dir(),
            Self::Grok => db::get_grok_dir(),
            Self::Pi => db::get_pi_dir(),
            Self::Omp => db::get_omp_dir(),
            Self::Muse => db::get_muse_dir(),
            Self::Mcode => db::get_mcode_dir(),
        })
    }
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct SourceConfig {
    #[serde(default)]
    additional_sources: BTreeMap<Harness, Vec<PathBuf>>,
}

/// Keep the existing config search order shared with the updater. The first
/// existing file wins; a malformed or unreadable file must not silently select
/// a different set of source directories from a fallback file.
pub(crate) fn read_config_file() -> Result<Option<(PathBuf, String)>, String> {
    let mut candidates = vec![crate::db::get_insights_dir().join("config.yaml")];
    #[cfg(windows)]
    if let Some(dir) = dirs::data_local_dir() {
        candidates.push(dir.join("TokenUsageInsights/config.yaml"));
    }
    #[cfg(not(windows))]
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".token-usage-insights/config.yaml"));
    }
    candidates.push(PathBuf::from("config.yaml"));
    for path in candidates {
        match std::fs::read_to_string(&path) {
            Ok(content) => return Ok(Some((path, content))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("無法讀取 {}: {error}", path.display())),
        }
    }
    Ok(None)
}

impl SourceConfig {
    pub(crate) fn load() -> Result<Self, String> {
        match read_config_file()? {
            Some((path, content)) => Self::parse(&content, &path),
            None => Ok(Self::default()),
        }
    }

    fn parse(content: &str, config_path: &Path) -> Result<Self, String> {
        let mut config: Self = serde_yaml_ng::from_str(content)
            .map_err(|error| format!("{} 設定格式錯誤: {error}", config_path.display()))?;
        let base = config_path.parent().unwrap_or_else(|| Path::new("."));
        for (harness, paths) in &mut config.additional_sources {
            for path in paths {
                if path.as_os_str().to_string_lossy().trim().is_empty() {
                    return Err(format!(
                        "{} 的 additional_sources.{harness:?} 不可包含空白路徑",
                        config_path.display()
                    ));
                }
                *path = crate::paths::expand_common_prefix(path.clone());
                if path.is_relative() {
                    *path = base.join(&path);
                }
                *path = absolute_root(path);
            }
        }
        Ok(config)
    }

    pub(crate) fn roots(&self, harness: Harness) -> Vec<PathBuf> {
        let mut roots: Vec<_> = harness.primary_root().into_iter().collect();
        if let Some(additional) = self.additional_sources.get(&harness) {
            roots.extend(additional.iter().cloned());
        }
        // Copilot CLI and App share the same home layout. A configured Copilot
        // home can contain both kinds of sessions, just like the default home.
        if harness == Harness::CopilotApp {
            if let Some(additional) = self.additional_sources.get(&Harness::Copilot) {
                roots.extend(additional.iter().cloned());
            }
        }
        deduplicate_roots(roots)
    }
}

pub(crate) fn absolute_root(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir().unwrap_or_default().join(path)
        }
    })
}

pub(crate) fn deduplicate_roots(roots: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    roots
        .into_iter()
        .map(|path| absolute_root(&path))
        .filter(|path| seen.insert(path.clone()))
        .collect()
}

pub(crate) fn configured_roots(harness: Harness) -> Result<Vec<PathBuf>, String> {
    Ok(SourceConfig::load()?.roots(harness))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_update_config_and_empty_documents_keep_default_sources() {
        for yaml in [
            "",
            "# comment\n",
            "auto_update: yes\nupdate_check_interval: '7'\n",
        ] {
            let config = SourceConfig::parse(yaml, Path::new("config.yaml")).unwrap();
            assert!(config.additional_sources.is_empty());
            assert_eq!(
                config.roots(Harness::Codex),
                vec![absolute_root(&crate::db::get_codex_dir())]
            );
        }
    }

    #[test]
    fn paths_accept_yaml_quotes_home_prefixes_and_config_relative_locations() {
        let base = std::env::temp_dir().join("insights-config-tests");
        let config = SourceConfig::parse(
            "additional_sources:\n  codex:\n    - '../Cloud Drive/laptop/.codex'\n    - '~/cloud/desktop/.codex'\n    - '$HOME/cloud/desktop/.codex'\n    - '../Cloud Drive/hash # directory/.codex'\n  claude: []\n",
            &base.join("config.yaml"),
        ).unwrap();
        let roots = config.roots(Harness::Codex);
        assert!(roots.contains(&base.join("../Cloud Drive/laptop/.codex")));
        assert!(roots.contains(&base.join("../Cloud Drive/hash # directory/.codex")));
        if let Some(home) = dirs::home_dir() {
            assert_eq!(
                roots
                    .iter()
                    .filter(|p| **p == home.join("cloud/desktop/.codex"))
                    .count(),
                1
            );
        }
        assert_eq!(config.roots(Harness::Claude).len(), 1);
    }

    #[test]
    fn every_harness_accepts_multiple_roots_and_copilot_shares_app_homes() {
        let keys = [
            "antigravity",
            "copilot",
            "copilot_app",
            "vscode",
            "codex",
            "claude",
            "cursor",
            "grok",
            "pi",
            "omp",
            "muse",
            "mcode",
        ];
        let yaml = format!(
            "additional_sources:\n{}",
            keys.iter()
                .map(|key| format!("  {key}: ['first/{key}', 'second/{key}']\n"))
                .collect::<String>()
        );
        let config = SourceConfig::parse(&yaml, Path::new("config.yaml")).unwrap();
        assert_eq!(config.additional_sources.len(), keys.len());
        for (harness, paths) in &config.additional_sources {
            assert_eq!(paths.len(), 2);
            assert!(paths
                .iter()
                .all(|path| config.roots(*harness).contains(path)));
        }
        for path in &config.additional_sources[&Harness::Copilot] {
            assert!(config.roots(Harness::CopilotApp).contains(path));
        }
    }

    #[test]
    fn invalid_source_shapes_and_typos_report_the_config_file() {
        for yaml in [
            "additional_sources: [codex]",
            "additional_sources: {codxe: []}",
            "additional_sources: {codex: /tmp/codex}",
            "additional_sources: {codex: ['']}",
            "additional_sources: {codex: ['   ']}",
            "additional_sources: {codex: [}",
        ] {
            let error = SourceConfig::parse(yaml, Path::new("test-config.yaml")).unwrap_err();
            assert!(error.contains("test-config.yaml"), "{error}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn repeated_and_symlinked_roots_are_scanned_once() {
        let root =
            std::env::temp_dir().join(format!("insights-source-links-{}", std::process::id()));
        std::fs::create_dir_all(root.join("original")).unwrap();
        std::os::unix::fs::symlink(root.join("original"), root.join("alias")).unwrap();
        assert_eq!(
            deduplicate_roots(vec![
                root.join("original"),
                root.join("alias"),
                root.join("original/.")
            ])
            .len(),
            1
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
