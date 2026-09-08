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
    for command in [None, Some("export"), Some("export-all"), Some("import")] {
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
    ] {
        let result = Command::new(env!("CARGO_BIN_EXE_token-usage-insights"))
            .env("INSIGHTS_DIR", &missing_dir)
            .args(args)
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(2));
        assert!(!missing_dir.exists());
    }
}
