//! 產生各 CLI `settings.json` 中 `statusLine.command` 所需的命令字串。
//!
//! Windows 上的 Antigravity CLI / Copilot CLI 不會透過 shell 解析 `command`，
//! 而是以空白切割後直接傳給子程序（參考 GitHub issue #64）；其他宿主則可能交給
//! `cmd.exe` 執行。產生的命令必須在這兩種行為下都能運作，因此：
//!
//! * 不能用雙引號包住路徑：引號會原樣傳給 `powershell.exe -File`，導致找不到檔案。
//! * 路徑以 `/` 取代 `\`，避免被當成跳脫字元。
//! * 路徑只含安全字元時使用可讀的 `-File <path>` 形式。
//! * 路徑含空白、`&`、`%`、單引號或非 ASCII 等任何可能被空白切割或被 `cmd.exe`
//!   解讀的字元時，改用 `-EncodedCommand <base64>`：Base64 只含 `A-Z a-z 0-9 + / =`，
//!   對任何宿主都是單一且不具特殊意義的 token。

use base64::{engine::general_purpose::STANDARD, Engine as _};

/// 依平台產生 `statusLine.command` 字串。
///
/// 非 Windows 平台直接回傳腳本路徑（由 shell 執行 `.sh`）。
pub fn statusline_command(script_path: &str, assistant: &str, windows: bool) -> String {
    if !windows {
        return script_path.to_string();
    }

    let normalized_path = normalize_windows_script_path(script_path);
    if is_simple_command_token(&normalized_path) {
        format!(
            "powershell.exe -NoProfile -ExecutionPolicy Bypass -File {normalized_path} -Assistant {assistant}"
        )
    } else {
        let script = format!(
            "& '{}' -Assistant {assistant}",
            normalized_path.replace('\'', "''")
        );
        format!(
            "powershell.exe -NoProfile -ExecutionPolicy Bypass -EncodedCommand {}",
            encode_powershell_command(&script)
        )
    }
}

/// 去除 `\\?\` verbatim 前綴並把 `\` 轉為 `/`。
fn normalize_windows_script_path(script_path: &str) -> String {
    let stripped =
        crate::paths::strip_windows_verbatim_prefix(std::path::PathBuf::from(script_path));
    stripped.to_string_lossy().replace('\\', "/")
}

/// 只允許英數與 `/ : . _ - ~`：這些字元不會被空白切割、也不會被 `cmd.exe` 解讀。
fn is_simple_command_token(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | ':' | '.' | '_' | '-' | '~'))
}

/// `powershell.exe -EncodedCommand` 需要 UTF-16LE 的 Base64。
fn encode_powershell_command(script: &str) -> String {
    let utf16le: Vec<u8> = script
        .encode_utf16()
        .flat_map(|unit| unit.to_le_bytes())
        .collect();
    STANDARD.encode(utf16le)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_encoded_command(command: &str) -> String {
        let encoded = command
            .rsplit_once("-EncodedCommand ")
            .map(|(_, rest)| rest)
            .expect("command should carry -EncodedCommand");
        let bytes = STANDARD.decode(encoded).expect("valid base64");
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16(&units).expect("valid UTF-16LE")
    }

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
    fn windows_verbatim_prefix_is_stripped() {
        let command = statusline_command(
            r"\\?\C:\Users\YOUR_NAME\.copilot\statusline-token.ps1",
            "copilot",
            true,
        );
        assert!(command.contains("-File C:/Users/YOUR_NAME/.copilot/statusline-token.ps1 "));
        assert!(!command.contains('?'));
    }

    #[test]
    fn windows_path_with_spaces_uses_encoded_command() {
        let command = statusline_command(
            r"C:\Users\Will Huang\.gemini\antigravity-cli\statusline-token.ps1",
            "antigravity",
            true,
        );
        assert!(command
            .starts_with("powershell.exe -NoProfile -ExecutionPolicy Bypass -EncodedCommand "));
        assert!(!command.contains('"'));
        let payload = command
            .rsplit_once("-EncodedCommand ")
            .map(|(_, rest)| rest)
            .unwrap();
        assert!(
            payload.is_ascii() && !payload.contains(' '),
            "encoded payload must be a single ASCII token: {command}"
        );
        assert_eq!(
            decode_encoded_command(&command),
            "& 'C:/Users/Will Huang/.gemini/antigravity-cli/statusline-token.ps1' -Assistant antigravity"
        );
    }

    #[test]
    fn windows_path_with_cmd_metacharacters_uses_encoded_command() {
        let command = statusline_command(
            r"C:\Users\A&B\.copilot\statusline-token.ps1",
            "copilot",
            true,
        );
        assert!(command.contains("-EncodedCommand "));
        assert!(!command.contains('&'));
        assert_eq!(
            decode_encoded_command(&command),
            "& 'C:/Users/A&B/.copilot/statusline-token.ps1' -Assistant copilot"
        );
    }

    #[test]
    fn windows_path_with_single_quote_is_escaped_inside_encoded_command() {
        let command = statusline_command(
            r"C:\Users\O'Brien\.copilot\statusline-token.ps1",
            "copilot",
            true,
        );
        assert_eq!(
            decode_encoded_command(&command),
            "& 'C:/Users/O''Brien/.copilot/statusline-token.ps1' -Assistant copilot"
        );
    }

    #[test]
    fn windows_repeated_whitespace_is_preserved_inside_encoded_command() {
        // 宿主以空白切割後不會還原連續空白，因此這類路徑必須走 Base64 以原樣保留。
        let command = statusline_command(
            r"C:\Status  Lines\.copilot\statusline-token.ps1",
            "copilot",
            true,
        );
        assert!(command.contains("-EncodedCommand "));
        assert_eq!(
            decode_encoded_command(&command),
            "& 'C:/Status  Lines/.copilot/statusline-token.ps1' -Assistant copilot"
        );
    }

    #[test]
    fn windows_non_ascii_path_uses_encoded_command() {
        let command = statusline_command(
            r"C:\Users\王小明\.gemini\antigravity-cli\statusline-token.ps1",
            "antigravity",
            true,
        );
        assert!(command.contains("-EncodedCommand "));
        assert!(command.is_ascii());
        assert_eq!(
            decode_encoded_command(&command),
            "& 'C:/Users/王小明/.gemini/antigravity-cli/statusline-token.ps1' -Assistant antigravity"
        );
    }

    #[test]
    fn json_serialization_keeps_command_free_of_escapes() {
        for path in [
            r"C:\Users\YOUR_NAME\.gemini\antigravity-cli\statusline-token.ps1",
            r"C:\Users\Will Huang\.gemini\antigravity-cli\statusline-token.ps1",
        ] {
            let command = statusline_command(path, "antigravity", true);
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
}
