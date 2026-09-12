use std::process::Command;

#[test]
fn help_and_invalid_commands_exit_without_initializing_the_server() {
    let missing_dir = std::env::temp_dir().join(format!(
        "insights-help-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    for command in [
        None,
        Some("export"),
        Some("export-all"),
        Some("import"),
        Some("update"),
    ] {
        for flag in ["--help", "-h"] {
            let mut process = Command::new(env!("CARGO_BIN_EXE_token-usage-insights"));
            process
                .env("INSIGHTS_DIR", &missing_dir)
                .env("HOST", "invalid-host");
            if let Some(command) = command {
                process.arg(command);
            }
            let result = process.arg(flag).output().unwrap();
            assert!(result.status.success(), "{:?}", result);
            let help = String::from_utf8(result.stdout).unwrap();
            assert!(help.contains("token-usage-insights"));
            assert!(!help.contains("token-usage-insights-cli"));
            assert!(!missing_dir.exists());
        }
    }
    for args in [
        vec!["unknown"],
        vec!["--unknown"],
        vec!["export"],
        vec!["import"],
        vec!["export-all", "--out"],
        vec!["update", "--unknown"],
        vec!["update", "-v", "-f"],
        vec!["update", "--target-version", "--check"],
    ] {
        let result = Command::new(env!("CARGO_BIN_EXE_token-usage-insights"))
            .env("INSIGHTS_DIR", &missing_dir)
            .args(args)
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(2));
        assert!(!missing_dir.exists());
    }

    // In a git repository checkout, `update`, `--update`, and `-u` should be rejected by safety check (exit code 2)
    for cmd in ["update", "--update", "-u"] {
        let result = Command::new(env!("CARGO_BIN_EXE_token-usage-insights"))
            .env_remove("npm_config_user_agent")
            .env_remove("npm_lifecycle_event")
            .env_remove("npm_package_json")
            .env("INSIGHTS_DIR", &missing_dir)
            .arg(cmd)
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(2));
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(
            stderr.contains("開發目錄")
                || stderr.contains("非標準安裝目錄")
                || stderr.contains("npm")
        );
    }
    assert!(missing_dir.join("update.log").exists());
    let _ = std::fs::remove_dir_all(&missing_dir);
}
