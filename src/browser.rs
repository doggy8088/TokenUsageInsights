use std::{
    io::{self, IsTerminal},
    process::{Command, Stdio},
};

const SERVICE_MODE_ENV: &str = "TOKEN_USAGE_INSIGHTS_SERVICE";
const STARTUP_SEPARATOR: &str = "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━";

#[derive(Debug, PartialEq, Eq)]
struct BrowserCommand {
    program: &'static str,
    args: Vec<String>,
}

impl BrowserCommand {
    fn new(program: &'static str, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            program,
            args: args.into_iter().map(Into::into).collect(),
        }
    }
}

pub(crate) fn announce_dashboard(url: &str) {
    let stdin_is_terminal = io::stdin().is_terminal();
    let stdout_is_terminal = io::stdout().is_terminal();
    let service_mode = service_mode_from_value(std::env::var(SERVICE_MODE_ENV).ok().as_deref());

    println!("{}", startup_banner(url, stdout_is_terminal));

    if should_open_browser(service_mode, stdin_is_terminal, stdout_is_terminal) {
        open_browser_in_background(url.to_string());
    }
}

fn startup_banner(url: &str, use_terminal_style: bool) -> String {
    let message = format!(
        "🚀 Token 戰情室 v{} is running on: {url}",
        crate::APP_VERSION
    );
    if use_terminal_style {
        format!("\n\x1b[1;96m{STARTUP_SEPARATOR}\n{message}\n{STARTUP_SEPARATOR}\x1b[0m\n")
    } else {
        message
    }
}

fn service_mode_from_value(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        let value = value.trim();
        !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
    })
}

fn should_open_browser(
    service_mode: bool,
    stdin_is_terminal: bool,
    stdout_is_terminal: bool,
) -> bool {
    !service_mode && stdin_is_terminal && stdout_is_terminal
}

fn open_browser_in_background(url: String) {
    std::thread::spawn(move || {
        let commands = browser_commands(&url);
        let mut failures = Vec::new();

        for command in commands {
            let mut process = Command::new(command.program);
            process
                .args(&command.args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());

            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;

                const CREATE_NO_WINDOW: u32 = 0x0800_0000;
                process.creation_flags(CREATE_NO_WINDOW);
            }

            match process.status() {
                Ok(status) if status.success() => return,
                Ok(status) => failures.push(format!(
                    "{} 結束碼 {}",
                    command.program,
                    status
                        .code()
                        .map_or_else(|| "未知".to_string(), |code| code.to_string())
                )),
                Err(error) => failures.push(format!("{}: {error}", command.program)),
            }
        }

        eprintln!(
            "⚠️ 無法自動開啟預設瀏覽器，請手動開啟 {url}（{}）",
            failures.join("；")
        );
    });
}

#[cfg(target_os = "macos")]
fn browser_commands(url: &str) -> Vec<BrowserCommand> {
    vec![BrowserCommand::new("open", [url])]
}

#[cfg(target_os = "linux")]
fn browser_commands(url: &str) -> Vec<BrowserCommand> {
    if is_wsl() {
        vec![
            BrowserCommand::new("cmd.exe", ["/C", "start", "", url]),
            BrowserCommand::new("wslview", [url]),
            BrowserCommand::new("xdg-open", [url]),
        ]
    } else {
        vec![
            BrowserCommand::new("xdg-open", [url]),
            BrowserCommand::new("gio", ["open", url]),
        ]
    }
}

#[cfg(target_os = "linux")]
fn is_wsl() -> bool {
    std::env::var_os("WSL_DISTRO_NAME").is_some()
        || std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .is_ok_and(|release| release.to_ascii_lowercase().contains("microsoft"))
}

#[cfg(windows)]
fn browser_commands(url: &str) -> Vec<BrowserCommand> {
    vec![BrowserCommand::new("cmd.exe", ["/C", "start", "", url])]
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn browser_commands(_url: &str) -> Vec<BrowserCommand> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_banner_highlights_interactive_output() {
        let banner = startup_banner("http://localhost:3003", true);

        assert!(banner.contains("\x1b[1;96m"));
        assert!(banner.contains(STARTUP_SEPARATOR));
        assert!(banner.contains(&format!(
            "🚀 Token 戰情室 v{} is running on: http://localhost:3003",
            env!("CARGO_PKG_VERSION")
        )));
        assert!(banner.ends_with("\x1b[0m\n"));
    }

    #[test]
    fn startup_banner_keeps_service_logs_plain() {
        assert_eq!(
            startup_banner("http://localhost:3003", false),
            format!(
                "🚀 Token 戰情室 v{} is running on: http://localhost:3003",
                env!("CARGO_PKG_VERSION")
            )
        );
    }

    #[test]
    fn browser_opens_only_for_non_service_interactive_sessions() {
        assert!(should_open_browser(false, true, true));
        assert!(!should_open_browser(true, true, true));
        assert!(!should_open_browser(false, false, true));
        assert!(!should_open_browser(false, true, false));
    }

    #[test]
    fn service_mode_uses_fail_closed_environment_parsing() {
        assert!(!service_mode_from_value(None));
        assert!(!service_mode_from_value(Some("")));
        assert!(!service_mode_from_value(Some("0")));
        assert!(!service_mode_from_value(Some("FALSE")));
        assert!(service_mode_from_value(Some("1")));
        assert!(service_mode_from_value(Some("true")));
        assert!(service_mode_from_value(Some("unexpected")));
    }

    #[test]
    fn platform_browser_command_receives_dashboard_url() {
        let url = "http://localhost:3003";
        let commands = browser_commands(url);

        assert!(!commands.is_empty());
        assert!(commands
            .iter()
            .all(|command| command.args.iter().any(|argument| argument == url)));
    }
}
