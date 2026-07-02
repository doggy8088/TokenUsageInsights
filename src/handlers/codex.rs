use axum::{extract::Path, http::StatusCode, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::db;

#[derive(Serialize)]
pub struct CodexAuthInfo {
    pub name: String,
    pub display_name: String,
    pub email: Option<String>,
    pub active: bool,
}

#[derive(Serialize)]
pub struct CodexAuthListResponse {
    pub configs: Vec<CodexAuthInfo>,
    pub current_active: Option<String>,
}

#[derive(Deserialize)]
pub struct SwitchAuthRequest {
    pub name: String,
}

fn decode_base64(s: &str) -> Option<Vec<u8>> {
    let mut s = s.replace('-', "+").replace('_', "/");
    while s.len() % 4 != 0 {
        s.push('=');
    }
    let bytes = s.as_bytes();
    let mut result = Vec::new();
    let mut buffer = 0u32;
    let mut bits = 0;
    for &b in bytes {
        if b == b'=' {
            break;
        }
        let val = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        buffer = (buffer << 6) | val as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            result.push((buffer >> bits) as u8);
        }
    }
    Some(result)
}

fn get_codex_auth_identity(path: &std::path::Path) -> (Option<String>, Option<String>) {
    if let Ok(content) = std::fs::read_to_string(path) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(tokens) = v.get("tokens") {
                if let Some(id_token) = tokens.get("id_token").and_then(|t| t.as_str()) {
                    let parts: Vec<&str> = id_token.split('.').collect();
                    if parts.len() >= 2 {
                        if let Some(payload_bytes) = decode_base64(parts[1]) {
                            if let Ok(payload_json) =
                                serde_json::from_slice::<serde_json::Value>(&payload_bytes)
                            {
                                let name = payload_json
                                    .get("name")
                                    .and_then(|n| n.as_str())
                                    .map(|s| s.to_string());
                                let email = payload_json
                                    .get("email")
                                    .and_then(|e| e.as_str())
                                    .map(|s| s.to_string());
                                return (name, email);
                            }
                        }
                    }
                }
            }
        }
    }
    (None, None)
}

fn get_codex_auth_access_token_exp(path: &std::path::Path) -> Option<u64> {
    if let Ok(content) = std::fs::read_to_string(path) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(tokens) = v.get("tokens") {
                if let Some(access_token) = tokens.get("access_token").and_then(|t| t.as_str()) {
                    let parts: Vec<&str> = access_token.split('.').collect();
                    if parts.len() >= 2 {
                        if let Some(payload_bytes) = decode_base64(parts[1]) {
                            if let Ok(payload_json) =
                                serde_json::from_slice::<serde_json::Value>(&payload_bytes)
                            {
                                return payload_json.get("exp").and_then(|e| e.as_u64());
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

/// API: 獲取 Codex 的 auth 憑證列表
pub async fn get_codex_auth_configs(Path(assistant): Path<String>) -> impl IntoResponse {
    if assistant != "codex" {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Only codex is supported" })),
        )
            .into_response();
    }

    let res = tokio::task::spawn_blocking(move || {
        let codex_dir = db::get_codex_dir();
        let auth_dir = codex_dir.join("auth");
        let active_auth_file = codex_dir.join("auth.json");

        let mut is_empty = true;
        if auth_dir.exists() {
            if let Ok(mut entries) = std::fs::read_dir(&auth_dir) {
                if entries.next().is_some() {
                    is_empty = false;
                }
            }
        } else {
            let _ = std::fs::create_dir_all(&auth_dir);
        }

        if is_empty && active_auth_file.exists() {
            let dest_auth_file = auth_dir.join("auth.json");
            let _ = std::fs::copy(&active_auth_file, &dest_auth_file);
        }

        // Read active auth.json contents to compare
        let active_bytes = std::fs::read(&active_auth_file).ok();

        let mut configs = Vec::new();
        let mut current_active = None;

        if let Ok(entries) = std::fs::read_dir(&auth_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Some(filename) = path.file_name().and_then(|f| f.to_str()) {
                        if filename.ends_with(".json") {
                            let mut active = false;
                            if let Some(ref active_content) = active_bytes {
                                if let Ok(content) = std::fs::read(&path) {
                                    if content == *active_content {
                                        active = true;
                                        current_active = Some(filename.to_string());
                                    }
                                }
                            }
                            let (name_opt, email_opt) = get_codex_auth_identity(&path);
                            let display_name = name_opt.unwrap_or_else(|| filename.to_string());
                            configs.push(CodexAuthInfo {
                                name: filename.to_string(),
                                display_name,
                                email: email_opt,
                                active,
                            });
                        }
                    }
                }
            }
        }

        // Sort configs alphabetically by name
        configs.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(CodexAuthListResponse {
            configs,
            current_active,
        })
    })
    .await
    .unwrap_or_else(|_| Err("Thread execution failed".to_string()));

    match res {
        Ok(data) => (StatusCode::OK, Json(data)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        )
            .into_response(),
    }
}

/// API: 切換 Codex 的 auth 憑證
pub async fn switch_codex_auth(
    Path(assistant): Path<String>,
    Json(payload): Json<SwitchAuthRequest>,
) -> impl IntoResponse {
    if assistant != "codex" {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Only codex is supported" })),
        )
            .into_response();
    }

    // Safety check: make sure filename doesn't contain path traversal
    let filename = payload.name.clone();
    if filename.contains('/') || filename.contains('\\') || filename.contains("..") {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid filename" })),
        )
            .into_response();
    }

    let res = tokio::task::spawn_blocking(move || {
        let codex_dir = db::get_codex_dir();
        let auth_dir = codex_dir.join("auth");
        let source_file = auth_dir.join(&filename);
        let dest_file = codex_dir.join("auth.json");

        if !source_file.exists() {
            return Err(format!("Auth file {} does not exist", filename));
        }

        // Check if access_token in source_file is expired
        if let Some(exp_secs) = get_codex_auth_access_token_exp(&source_file) {
            let current_secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            if exp_secs <= current_secs {
                use chrono::TimeZone;
                let exp_time = chrono::Utc
                    .timestamp_opt(exp_secs as i64, 0)
                    .single()
                    .map(|dt| {
                        dt.with_timezone(&chrono::Local)
                            .format("%Y-%m-%d %H:%M:%S")
                            .to_string()
                    })
                    .unwrap_or_else(|| exp_secs.to_string());
                return Err(format!("憑證已過期，失效時間：{}", exp_time));
            }
        }

        std::fs::copy(&source_file, &dest_file)
            .map_err(|e| format!("Failed to copy file: {}", e))?;

        Ok(())
    })
    .await
    .unwrap_or_else(|_| Err("Thread execution failed".to_string()));

    match res {
        Ok(_) => (StatusCode::OK, Json(serde_json::json!({ "status": "success", "message": format!("Successfully switched to {}", payload.name) }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "status": "error", "message": e }))).into_response(),
    }
}

fn find_npx_path() -> PathBuf {
    // 1. Check in PATH env var
    if let Ok(path_env) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_env) {
            let npx_in_path = dir.join("npx");
            if npx_in_path.exists() && npx_in_path.is_file() {
                return npx_in_path;
            }
        }
    }

    // 2. Try looking in NVM directory under home_dir
    if let Some(home) = dirs::home_dir() {
        let nvm_dir = home.join(".nvm/versions/node");
        if nvm_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(nvm_dir) {
                let mut versions = Vec::new();
                for entry in entries.flatten() {
                    if entry.path().is_dir() {
                        versions.push(entry.path());
                    }
                }
                // Sort versions to pick the latest Node version
                versions.sort();
                if let Some(newest_node) = versions.last() {
                    let npx_fallback = newest_node.join("bin/npx");
                    if npx_fallback.exists() {
                        return npx_fallback;
                    }
                }
            }
        }
    }

    // 3. Fallback to just "npx"
    PathBuf::from("npx")
}

/// API: 獲取 Codex 的重置額度資訊
pub async fn get_codex_reset_info(Path(assistant): Path<String>) -> impl IntoResponse {
    if assistant != "codex" {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Only codex is supported" })),
        )
            .into_response();
    }

    let res = tokio::task::spawn_blocking(move || {
        let codex_dir = db::get_codex_dir();
        let active_auth_file = codex_dir.join("auth.json");

        let npx_path = find_npx_path();
        let mut cmd = std::process::Command::new(&npx_path);

        if let Some(bin_dir) = npx_path.parent() {
            if let Ok(current_path) = std::env::var("PATH") {
                let new_path = std::env::join_paths(
                    std::iter::once(bin_dir.to_path_buf())
                        .chain(std::env::split_paths(&current_path)),
                );
                if let Ok(new_path_val) = new_path {
                    cmd.env("PATH", new_path_val);
                }
            }
        }

        cmd.args(["-y", "@willh/codex-reset-checker", "--json"]);
        if active_auth_file.exists() {
            cmd.arg("--auth").arg(&active_auth_file);
        }

        let output = cmd.output();
        match output {
            Ok(out) => {
                if out.status.success() {
                    let stdout_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(&stdout_str) {
                        Ok(json_val)
                    } else {
                        Err(format!("Failed to parse output as JSON: {}", stdout_str))
                    }
                } else {
                    let stderr_str = String::from_utf8_lossy(&out.stderr).trim().to_string();
                    Err(format!(
                        "Command exited with status code {}: {}",
                        out.status, stderr_str
                    ))
                }
            }
            Err(e) => Err(format!("Failed to execute command: {}", e)),
        }
    })
    .await
    .unwrap_or_else(|_| Err("Thread execution failed".to_string()));

    match res {
        Ok(data) => (StatusCode::OK, Json(data)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        )
            .into_response(),
    }
}
