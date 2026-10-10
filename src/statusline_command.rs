//! 產生各 CLI `settings.json` 中 `statusLine.command` 所需的命令字串。
//!
//! Windows 上的 Antigravity CLI / Copilot CLI 不會透過 shell 解析 `command`，
//! 而是以空白切割後直接傳給子程序（參考 GitHub issue #64）。因此：
//!
//! * 不能用雙引號包住路徑：引號會原樣傳給 `powershell.exe -File`，導致找不到檔案。
//! * 路徑以 `/` 取代 `\`，避免被當成跳脫字元。
//! * 路徑含空白時改用 `-Command . '<path>'`：PowerShell 會把 `-Command` 之後的所有
//!   引數以空白重新串接，因此無論宿主是以空白切割或交給 `cmd.exe` 執行都能正確還原。

/// 依平台產生 `statusLine.command` 字串。
///
/// 非 Windows 平台直接回傳腳本路徑（由 shell 執行 `.sh`）。
pub fn statusline_command(script_path: &str, assistant: &str, windows: bool) -> String {
    if !windows {
        return script_path.to_string();
    }

    let normalized_path = script_path.replace('\\', "/");
    if normalized_path.chars().any(char::is_whitespace) {
        let single_quoted = normalized_path.replace('\'', "''");
        format!(
            "powershell.exe -NoProfile -ExecutionPolicy Bypass -Command . '{single_quoted}' -Assistant {assistant}"
        )
    } else {
        format!(
            "powershell.exe -NoProfile -ExecutionPolicy Bypass -File {normalized_path} -Assistant {assistant}"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::statusline_command;

    #[test]
    fn unix_returns_script_path_unchanged() {
        assert_eq!(
            statusline_command(
                "/home/user/.gemini/antigravity-cli/statusline-token.sh",
                "antigravity",
                false
            ),
            "/home/user/.gemini/antigravity-cli/statusline-token.sh"
        );
    }

    #[test]
    fn windows_uses_forward_slashes_without_quotes() {
        let command = statusline_command(
            r"C:\Users\YOUR_NAME\.gemini\antigravity-cli\statusline-token.ps1",
            "antigravity",
            true,
        );
        assert_eq!(
            command,
            "powershell.exe -NoProfile -ExecutionPolicy Bypass -File C:/Users/YOUR_NAME/.gemini/antigravity-cli/statusline-token.ps1 -Assistant antigravity"
        );
        assert!(!command.contains('"'));
        assert!(!command.contains('\\'));
    }

    #[test]
    fn windows_copilot_uses_copilot_assistant_argument() {
        let command = statusline_command(
            r"C:\Users\YOUR_NAME\.copilot\statusline-token.ps1",
            "copilot",
            true,
        );
        assert!(command.ends_with(
            "-File C:/Users/YOUR_NAME/.copilot/statusline-token.ps1 -Assistant copilot"
        ));
    }

    #[test]
    fn windows_path_with_spaces_uses_dot_sourced_command() {
        let command = statusline_command(
            r"C:\Users\Will Huang\.gemini\antigravity-cli\statusline-token.ps1",
            "antigravity",
            true,
        );
        assert_eq!(
            command,
            "powershell.exe -NoProfile -ExecutionPolicy Bypass -Command . 'C:/Users/Will Huang/.gemini/antigravity-cli/statusline-token.ps1' -Assistant antigravity"
        );
        assert!(!command.contains('"'));
    }

    #[test]
    fn windows_path_with_single_quote_is_escaped_for_powershell() {
        let command = statusline_command(
            r"C:\Users\O'Brien Dev\.copilot\statusline-token.ps1",
            "copilot",
            true,
        );
        assert!(command.contains(
            "-Command . 'C:/Users/O''Brien Dev/.copilot/statusline-token.ps1' -Assistant copilot"
        ));
    }

    #[test]
    fn json_serialization_keeps_command_free_of_escapes() {
        let command = statusline_command(
            r"C:\Users\YOUR_NAME\.gemini\antigravity-cli\statusline-token.ps1",
            "antigravity",
            true,
        );
        let json = serde_json::to_string(&command).unwrap();
        assert!(
            !json.contains("\\\\"),
            "backslashes must not appear in JSON: {json}"
        );
        assert!(
            !json.contains("\\\""),
            "escaped quotes must not appear in JSON: {json}"
        );
    }
}
