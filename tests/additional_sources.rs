use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct TestServer {
    child: Child,
    root: PathBuf,
    url: String,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn write_jsonl(path: &Path, entries: &[Value]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let content = entries
        .iter()
        .map(|entry| format!("{entry}\n"))
        .collect::<String>();
    fs::write(path, content).unwrap();
}

fn codex_session(path: &Path, session: &str, input: u64) {
    write_jsonl(
        path,
        &[
            json!({"timestamp":"2026-09-11T10:00:00Z","type":"session_meta","payload":{"id":session,"originator":"codex_cli_rs","source":"cli","cwd":"/example/project"}}),
            json!({"timestamp":"2026-09-11T10:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Test extra sources"}]}}),
            json!({"timestamp":"2026-09-11T10:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":input,"cached_input_tokens":0,"output_tokens":10,"reasoning_output_tokens":0,"total_tokens":input+10}}}}),
        ],
    );
}

fn hook_session(root: &Path, session: &str) {
    write_jsonl(
        &root.join("usage/usage-2026-09-11.jsonl"),
        &[
            json!({"timestamp":"2026-09-11T10:00:00Z","session_id":session,"turn_no":1,
            "transcript_path":"/another-computer/session.jsonl",
            "tokens":{"input":100,"output":10,"total":110},
            "delta_tokens":{"input":100,"output":10,"total":110}}),
        ],
    );
    write_jsonl(
        &root
            .join("session-state")
            .join(session)
            .join("events.jsonl"),
        &[json!({"type":"user.message","data":{"content":"A remote Copilot session"}})],
    );
}

fn pi_session(root: &Path, session: &str, input: u64) {
    write_jsonl(
        &root.join("agent/sessions/project/same-name.jsonl"),
        &[
            json!({"type":"session","version":3,"id":session,"timestamp":"2026-09-11T10:00:00Z","cwd":"/example/project"}),
            json!({"type":"message","id":"m1","timestamp":"2026-09-11T10:00:01Z","message":{"role":"user","content":"Hello"}}),
            json!({"type":"message","id":"m2","parentId":"m1","timestamp":"2026-09-11T10:00:02Z","message":{"role":"assistant","content":[{"type":"text","text":"Hi"}],"provider":"anthropic","model":"claude-sonnet-4-5","usage":{"input":input,"output":10,"totalTokens":input+10},"stopReason":"stop"}}),
        ],
    );
}

async fn records(client: &reqwest::Client, server: &TestServer, assistant: &str) -> Vec<Value> {
    client
        .get(format!(
            "{}/api/{assistant}/usage/2026-09-11/export",
            server.url
        ))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json::<Value>()
        .await
        .unwrap()["records"]
        .as_array()
        .unwrap()
        .clone()
}

#[tokio::test]
async fn startup_and_manual_sync_reload_extra_homes_without_losing_defaults() {
    let root = std::env::temp_dir().join(format!(
        "insights-extra-sources-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(root.join("insights")).unwrap();
    let primary = root.join("primary");
    let remote = root.join("cloud/laptop");
    let later = root.join("cloud/desktop");
    let rollout = "rollout-2026-09-11T10-00-00-local.jsonl";
    codex_session(&primary.join("codex/sessions").join(rollout), "local", 100);
    codex_session(
        &remote.join("codex/archived_sessions/rollout-2026-09-11T10-00-00-remote.jsonl"),
        "remote",
        200,
    );
    hook_session(&primary.join("copilot"), "copilot-a");
    hook_session(&remote.join("copilot"), "copilot-b");
    pi_session(&primary.join("pi"), "pi-a", 100);
    pi_session(&remote.join("pi"), "pi-b", 200);
    write_jsonl(
        &remote.join("claude/projects/project/extra-only.jsonl"),
        &[
            json!({"type":"assistant","timestamp":"2026-09-11T10:00:00Z","sessionId":"extra-only","message":{"id":"response-1","role":"assistant","model":"claude-sonnet-4-5","content":[{"type":"text","text":"Extra Claude home"}],"usage":{"input_tokens":123,"output_tokens":10}}}),
        ],
    );
    let config_path = root.join("insights/config.yaml");
    fs::write(&config_path, "auto_update: false\nadditional_sources:\n  codex: ['../cloud/laptop/codex']\n  copilot: ['../cloud/laptop/copilot']\n  pi: ['../cloud/laptop/pi']\n  claude: ['../cloud/laptop/claude']\n").unwrap();

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let mut command = Command::new(env!("CARGO_BIN_EXE_token-usage-insights"));
    // Isolate the child from every real source, including platform VS Code homes.
    command
        .env_clear()
        .env("HOME", &primary)
        .env("USERPROFILE", &primary)
        .env("APPDATA", primary.join("appdata"))
        .env("LOCALAPPDATA", primary.join("localappdata"))
        .env("XDG_CONFIG_HOME", primary.join("config"))
        .env("XDG_DATA_HOME", primary.join("data"))
        .env("INSIGHTS_DIR", root.join("insights"))
        .env("HOST", "127.0.0.1")
        .env("PORT", port.to_string())
        .env("CURSOR_STATE_DB", primary.join("cursor/state.vscdb"))
        .env(
            "MCODE_STATE_DB",
            primary.join("mcode/sqlite/runtime-state.sqlite"),
        )
        .arg("--no-auto-update")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(
            fs::File::create(root.join("server.log")).unwrap(),
        ));
    // Winsock loads its providers through SystemRoot even with isolated user data.
    #[cfg(windows)]
    command.env(
        "SystemRoot",
        std::env::var_os("SystemRoot")
            .expect("Windows test server requires SystemRoot for Winsock"),
    );
    for tool in [
        "antigravity",
        "copilot",
        "codex",
        "claude",
        "cursor",
        "grok",
        "pi",
        "omp",
        "muse",
        "mcode",
    ] {
        command.env(format!("{}_DIR", tool.to_uppercase()), primary.join(tool));
    }
    command.env("COPILOT_APP_DIR", primary.join("copilot"));
    let mut server = TestServer {
        child: command.spawn().unwrap(),
        root,
        url: format!("http://127.0.0.1:{port}"),
    };
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(response) = client
            .get(format!("{}/api/codex/dates", server.url))
            .send()
            .await
        {
            if response.status().is_success() {
                let dates = response.json::<Value>().await.unwrap();
                if dates["dates"]
                    .as_array()
                    .is_some_and(|dates| !dates.is_empty())
                    && records(&client, &server, "pi").await.len() == 2
                {
                    break;
                }
            }
        }
        assert!(
            Instant::now() < deadline && server.child.try_wait().unwrap().is_none(),
            "startup failed: {}",
            fs::read_to_string(server.root.join("server.log")).unwrap()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(records(&client, &server, "codex").await.len(), 2);
    assert_eq!(records(&client, &server, "copilot").await.len(), 2);

    // Reload on the same running process. Include a missing cloud mount, duplicate
    // root, and a copy of an existing rollout to exercise all three cases.
    codex_session(
        &later.join("codex/sessions/rollout-2026-09-11T11-00-00-later.jsonl"),
        "later",
        300,
    );
    fs::copy(
        primary.join("codex/sessions").join(rollout),
        later.join("codex/sessions").join(rollout),
    )
    .unwrap();
    fs::write(&config_path, "auto_update: false\nadditional_sources:\n  codex:\n    - '../cloud/laptop/codex'\n    - '../cloud/desktop/codex'\n    - '../cloud/desktop/codex'\n    - '../offline/codex'\n  copilot: ['../cloud/laptop/copilot']\n  pi: ['../cloud/laptop/pi']\n  claude: ['../cloud/laptop/claude']\n").unwrap();
    for _ in 0..2 {
        client
            .get(format!("{}/api/codex/sync", server.url))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
        let rows = records(&client, &server, "codex").await;
        assert_eq!(rows.len(), 3);
        assert_eq!(
            rows.iter()
                .map(|row| row["delta_tokens"]["input"].as_u64().unwrap())
                .sum::<u64>(),
            600
        );
        assert_eq!(records(&client, &server, "copilot").await.len(), 2);
        assert_eq!(records(&client, &server, "pi").await.len(), 2);
    }
    for (assistant, session) in [
        ("codex", "remote"),
        ("codex", "later"),
        ("copilot", "copilot-b"),
        ("pi", "pi-b"),
        ("claude", "extra-only"),
    ] {
        let response = client
            .get(format!("{}/api/{assistant}/session/{session}", server.url))
            .send()
            .await
            .unwrap();
        assert!(
            response.status().is_success(),
            "{assistant}/{session}: {}",
            response.text().await.unwrap()
        );
    }

    let setup = client
        .get(format!("{}/api/claude/setup-info", server.url))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(setup["claude"]["exists"], true);
    assert_eq!(
        PathBuf::from(setup["claude"]["dir_path"].as_str().unwrap()),
        remote.join("claude").canonicalize().unwrap()
    );
    assert_eq!(records(&client, &server, "claude").await.len(), 1);

    // A more complete remote copy of a rollout replaces its older local copy.
    codex_session(&later.join("codex/sessions").join(rollout), "local", 1000);
    client
        .get(format!("{}/api/codex/sync", server.url))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let rows = records(&client, &server, "codex").await;
    assert_eq!(rows.len(), 3);
    assert_eq!(
        rows.iter()
            .map(|row| row["delta_tokens"]["input"].as_u64().unwrap())
            .sum::<u64>(),
        1500
    );

    // Preserve the established empty-state reason even when the missing events
    // directory belongs to an extra source rather than the primary home.
    fs::remove_file(remote.join("copilot/session-state/copilot-b/events.jsonl")).unwrap();
    let missing = client
        .get(format!("{}/api/copilot/session/copilot-b", server.url))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), reqwest::StatusCode::NOT_FOUND);
    assert_eq!(
        missing.json::<Value>().await.unwrap()["reason"],
        "no_events_yet"
    );

    // Invalid configuration must surface through Sync Now and preserve existing data.
    fs::write(&config_path, "additional_sources: {codex: 'not-a-list'}\n").unwrap();
    let response = client
        .get(format!("{}/api/codex/sync", server.url))
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        reqwest::StatusCode::INTERNAL_SERVER_ERROR
    );
    assert!(response.text().await.unwrap().contains("config.yaml"));
    assert_eq!(records(&client, &server, "codex").await.len(), 3);
}
