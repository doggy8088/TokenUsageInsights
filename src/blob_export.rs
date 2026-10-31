//! 將 `export-all` 的 JSON 上傳至 Azure Blob Storage，並供 `import --url` 下載（以 SAS URL 授權）。
//!
//! SAS 的 `sig` 等同密碼：錯誤訊息與一般顯示位址都不得包含 query string；
//! 唯一的例外是匯出成功後刻意印出的完整匯入命令（`export_success_message`）。

use chrono::{DateTime, TimeZone};
use reqwest::{StatusCode, Url};
use std::time::Duration;

/// 完整匯出可能很大，上傳與下載都給予較長的整體逾時。
const NETWORK_TIMEOUT: Duration = Duration::from_secs(600);
const FALLBACK_COMPUTER_NAME: &str = "unknown-host";
/// 下載匯入檔的大小上限，與看板匯入 API 的上限（200 MB）一致。
pub(crate) const MAX_DOWNLOAD_BYTES: usize = 200_000_000;

/// `--out` 是否為 URL（`http://` 或 `https://`）；其餘一律視為本機檔案路徑。
pub(crate) fn is_url(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://")
}

/// 預設檔名：`yyyy-MM-dd_HH-mm-ss_<電腦名稱>.json`。
pub(crate) fn export_file_name<Tz>(now: &DateTime<Tz>, computer_name: &str) -> String
where
    Tz: TimeZone,
    Tz::Offset: std::fmt::Display,
{
    format!(
        "{}_{}.json",
        now.format("%Y-%m-%d_%H-%M-%S"),
        sanitize_computer_name(computer_name)
    )
}

/// 保留字母、數字與 `-_.`（含中文電腦名稱），其餘字元換成 `-`，避免檔名出現路徑分隔或空白。
fn sanitize_computer_name(raw: &str) -> String {
    let cleaned: String = raw
        .trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    if cleaned.is_empty() {
        FALLBACK_COMPUTER_NAME.to_string()
    } else {
        cleaned
    }
}

pub(crate) fn computer_name() -> String {
    let non_empty = |value: String| (!value.trim().is_empty()).then_some(value);
    std::env::var("COMPUTERNAME")
        .ok()
        .and_then(non_empty)
        .or_else(os_hostname)
        .or_else(|| std::env::var("HOSTNAME").ok().and_then(non_empty))
        .map(|name| sanitize_computer_name(&name))
        .unwrap_or_else(|| FALLBACK_COMPUTER_NAME.to_string())
}

#[cfg(unix)]
fn os_hostname() -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: 傳入的長度等於 buf 的實際長度，gethostname 不會寫出界。
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    if rc != 0 {
        return None;
    }
    let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    let name = String::from_utf8_lossy(&buf[..end]).trim().to_string();
    (!name.is_empty()).then_some(name)
}

#[cfg(not(unix))]
fn os_hostname() -> Option<String> {
    // Windows 由 COMPUTERNAME 環境變數提供電腦名稱。
    None
}

/// 由 SAS URL 組出實際上傳的 Blob URL，query string（含簽章）原樣保留。
///
/// - 容器 SAS（`https://<account>.blob.core.windows.net/<container>?<SAS>`）：附加預設檔名。
/// - Blob SAS（`sr=b`）：該 SAS 只授權單一 Blob，直接使用 URL 內指定的 Blob。
pub(crate) fn build_blob_url(sas_url: &str, file_name: &str) -> Result<Url, String> {
    let mut url = Url::parse(sas_url).map_err(|e| format!("無法解析 SAS URL: {e}"))?;
    if url.scheme() != "https" {
        return Err("SAS URL 必須使用 HTTPS".to_string());
    }
    if !url
        .query_pairs()
        .any(|(key, value)| key == "sig" && !value.is_empty())
    {
        return Err("不是有效的 SAS URL（缺少 sig 參數）".to_string());
    }

    let segments: Vec<&str> = url
        .path_segments()
        .map(|parts| parts.filter(|part| !part.is_empty()).collect())
        .unwrap_or_default();
    if segments.is_empty() {
        return Err("SAS URL 必須包含容器名稱，例如 https://<account>.blob.core.windows.net/<container>?<SAS>".to_string());
    }

    let is_blob_sas = url
        .query_pairs()
        .any(|(key, value)| key == "sr" && value == "b");
    if is_blob_sas {
        if segments.len() < 2 {
            return Err("Blob SAS URL 必須包含 Blob 名稱".to_string());
        }
        return Ok(url);
    }

    url.path_segments_mut()
        .map_err(|_| "SAS URL 無法附加檔名".to_string())?
        .pop_if_empty()
        .push(file_name);
    Ok(url)
}

/// 供訊息顯示：移除 query string 與 fragment，避免洩漏 SAS 簽章。
pub(crate) fn display_url(url: &Url) -> String {
    let mut shown = url.clone();
    shown.set_query(None);
    shown.set_fragment(None);
    shown.to_string()
}

fn http_client() -> Result<reqwest::Client, String> {
    // 帶有憑證的請求不跟隨重新導向。
    reqwest::Client::builder()
        .timeout(NETWORK_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| format!("建立 HTTP 用戶端失敗: {}", e.without_url()))
}

pub(crate) async fn upload_json(url: &Url, body: String) -> Result<(), String> {
    let response = http_client()?
        .put(url.clone())
        .header("x-ms-blob-type", "BlockBlob")
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/json; charset=utf-8",
        )
        .body(body)
        .send()
        .await
        .map_err(|e| format!("上傳失敗: {}", e.without_url()))?;

    let status = response.status();
    if status == StatusCode::CREATED {
        return Ok(());
    }
    let body = response.text().await.unwrap_or_default();
    Err(format!(
        "Azure Blob 回應 HTTP {status}{}",
        error_detail(&body)
    ))
}

/// 匯出成功後的提示：不含 SAS 的位址，加上可直接複製到另一台電腦執行的匯入命令。
///
/// 匯入命令刻意包含完整 SAS（使用者要求），呼叫端需提醒妥善保管。
pub(crate) fn export_success_message(count: usize, url: &Url) -> String {
    format!(
        "已匯出 {count} 筆到 {}\n在另一台電腦匯入（網址已含 SAS，請妥善保管）：\ntoken-usage-insights import --url {}",
        display_url(url),
        shell_quote(url.as_str())
    )
}

/// 以單引號包住整段 URL，避免 `&` 被 shell 當成背景執行；內含的單引號以 `'\''` 跳脫。
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// SAS 有 `sp` 但不含讀取（`r`）時回傳其權限字串，另一台電腦將無法用同一網址匯入。
pub(crate) fn sas_without_read_permission(url: &Url) -> Option<String> {
    url.query_pairs()
        .find(|(key, _)| key == "sp")
        .map(|(_, value)| value.into_owned())
        .filter(|permissions| !permissions.contains('r'))
}

/// 驗證 `import --url` 的網址：必須是 HTTPS，且指向單一檔案。SAS 為選用（公開 Blob 不需要）。
pub(crate) fn parse_download_url(value: &str) -> Result<Url, String> {
    let url = Url::parse(value).map_err(|e| format!("無法解析網址: {e}"))?;
    if url.scheme() != "https" {
        return Err("網址必須使用 HTTPS".to_string());
    }
    // 容器 SAS（sr=c）同樣可讀取容器內的 Blob，所以匯出時印出的完整網址帶有 sr=c；
    // 只有網址停在容器本身、沒有檔名時才拒絕。
    let is_container_sas = url
        .query_pairs()
        .any(|(key, value)| key == "sr" && value == "c");
    let segment_count = url
        .path_segments()
        .map_or(0, |parts| parts.filter(|part| !part.is_empty()).count());
    if is_container_sas && segment_count < 2 {
        return Err("這是容器網址，請使用匯出時顯示的完整 Blob 網址（需包含檔名）".to_string());
    }
    if blob_file_name(&url).is_none() || url.path().ends_with('/') {
        return Err("網址必須指向 JSON 檔案，例如 https://<account>.blob.core.windows.net/<container>/<檔名>.json?<SAS>".to_string());
    }
    Ok(url)
}

/// 網址最後一段（解碼後）；僅用作匯入批次的來源檔名，不含 query string。
pub(crate) fn blob_file_name(url: &Url) -> Option<String> {
    let last = url.path_segments()?.rfind(|segment| !segment.is_empty())?;
    Some(percent_decode(last))
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |offset: usize| {
            bytes
                .get(i + offset)
                .and_then(|b| (*b as char).to_digit(16))
        };
        match (bytes[i], hex(1), hex(2)) {
            (b'%', Some(high), Some(low)) => {
                decoded.push((high * 16 + low) as u8);
                i += 3;
            }
            (byte, _, _) => {
                decoded.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

pub(crate) async fn download_json(url: &Url, max_bytes: usize) -> Result<String, String> {
    let mut response = http_client()?
        .get(url.clone())
        .send()
        .await
        .map_err(|e| format!("下載失敗: {}", e.without_url()))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!("下載失敗：HTTP {status}{}", error_detail(&body)));
    }
    let too_large = || format!("匯入檔案超過大小上限（{} MB）", max_bytes / 1_000_000);
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(too_large());
    }

    let mut data = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("下載失敗: {}", e.without_url()))?
    {
        if data.len() + chunk.len() > max_bytes {
            return Err(too_large());
        }
        data.extend_from_slice(&chunk);
    }
    String::from_utf8(data).map_err(|_| "下載內容不是有效的 UTF-8 文字".to_string())
}

/// 從 Azure 的 XML 錯誤內容取出 `<Code>` 與 `<Message>` 首行；不回傳整段內容。
fn error_detail(body: &str) -> String {
    let code = xml_tag(body, "Code");
    let message = xml_tag(body, "Message")
        .and_then(|text| text.lines().next().map(str::trim).map(str::to_string));
    match (code, message) {
        (Some(code), Some(message)) => format!("（{code}: {message}）"),
        (Some(code), None) => format!("（{code}）"),
        _ => String::new(),
    }
}

fn xml_tag<'a>(body: &'a str, tag: &str) -> Option<&'a str> {
    let start = body.find(&format!("<{tag}>"))? + tag.len() + 2;
    let end = body[start..].find(&format!("</{tag}>"))? + start;
    Some(body[start..end].trim())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, TimeZone};

    const SAS: &str =
        "sv=2025-07-05&spr=https&st=2026-10-28T10%3A41%3A36Z&sr=c&sp=racwl&sig=abc%3D";

    #[test]
    fn url_detection_only_matches_http_schemes() {
        assert!(is_url("https://example.blob.core.windows.net/c?sig=x"));
        assert!(is_url("HTTP://example.com/c"));
        assert!(!is_url("all-usage.json"));
        assert!(!is_url(r"C:\exports\all.json"));
        assert!(!is_url("./https/all.json"));
    }

    #[test]
    fn file_name_uses_timestamp_and_sanitized_computer_name() {
        let now = FixedOffset::east_opt(8 * 3600)
            .unwrap()
            .with_ymd_and_hms(2026, 10, 28, 9, 5, 7)
            .unwrap();
        assert_eq!(
            export_file_name(&now, "WILL-PC"),
            "2026-10-28_09-05-07_WILL-PC.json"
        );
        assert_eq!(
            export_file_name(&now, " My PC/01 "),
            "2026-10-28_09-05-07_My-PC-01.json"
        );
        assert_eq!(
            export_file_name(&now, "保哥的電腦"),
            "2026-10-28_09-05-07_保哥的電腦.json"
        );
        assert_eq!(
            export_file_name(&now, "  "),
            "2026-10-28_09-05-07_unknown-host.json"
        );
    }

    #[test]
    fn computer_name_is_never_empty_or_path_like() {
        let name = computer_name();
        assert!(!name.is_empty());
        assert!(!name.contains(['/', '\\', ' ']));
    }

    #[test]
    fn container_sas_gets_file_name_and_keeps_query_byte_identical() {
        for container_path in ["/container", "/container/", "/container/exports"] {
            let sas_url = format!("https://acct.blob.core.windows.net{container_path}?{SAS}");
            let url = build_blob_url(&sas_url, "2026-10-28_09-05-07_PC.json").unwrap();
            assert_eq!(url.query(), Some(SAS));
            let expected_path = format!(
                "{}/2026-10-28_09-05-07_PC.json",
                container_path.trim_end_matches('/')
            );
            assert_eq!(url.path(), expected_path);
        }
    }

    #[test]
    fn file_name_with_chinese_computer_name_is_percent_encoded_in_path() {
        let url = build_blob_url(
            &format!("https://acct.blob.core.windows.net/c?{SAS}"),
            "2026-10-28_09-05-07_保哥.json",
        )
        .unwrap();
        assert!(url.path().starts_with("/c/2026-10-28_09-05-07_%E4%BF%9D"));
    }

    #[test]
    fn blob_sas_is_used_as_is() {
        let sas_url = "https://acct.blob.core.windows.net/c/fixed.json?sr=b&sig=abc";
        let url = build_blob_url(sas_url, "ignored.json").unwrap();
        assert_eq!(url.as_str(), sas_url);
        assert!(build_blob_url(
            "https://acct.blob.core.windows.net/c?sr=b&sig=abc",
            "x.json"
        )
        .is_err());
    }

    #[test]
    fn invalid_sas_urls_are_rejected_without_echoing_the_signature() {
        let cases = [
            "http://acct.blob.core.windows.net/c?sig=secret",
            "https://acct.blob.core.windows.net/c?sv=2025-07-05",
            "https://acct.blob.core.windows.net/?sig=secret",
            "https://acct.blob.core.windows.net?sig=secret",
            "https://",
        ];
        for case in cases {
            let err = build_blob_url(case, "x.json").unwrap_err();
            assert!(!err.contains("secret"), "{err}");
        }
    }

    #[test]
    fn display_url_drops_the_query_string() {
        let url = build_blob_url(
            &format!("https://acct.blob.core.windows.net/c?{SAS}"),
            "x.json",
        )
        .unwrap();
        let shown = display_url(&url);
        assert_eq!(shown, "https://acct.blob.core.windows.net/c/x.json");
        assert!(!shown.contains("sig"));
    }

    #[test]
    fn error_detail_extracts_code_and_first_message_line() {
        let xml = "<?xml version=\"1.0\"?><Error><Code>AuthorizationFailure</Code><Message>This request is not authorized.\nRequestId:abc\nTime:now</Message></Error>";
        assert_eq!(
            error_detail(xml),
            "（AuthorizationFailure: This request is not authorized.）"
        );
        assert_eq!(error_detail("not xml"), "");
    }

    /// 以本機 TCP 伺服器回應固定內容，並回傳收到的完整請求。
    async fn serve_once(response: String) -> (Url, tokio::task::JoinHandle<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut received = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let n = socket.read(&mut chunk).await.unwrap();
                received.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&received);
                if let Some(head_end) = text.find("\r\n\r\n") {
                    let length = text
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|v| v.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if received.len() >= head_end + 4 + length {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            socket.write_all(response.as_bytes()).await.unwrap();
            String::from_utf8_lossy(&received).to_string()
        });
        let url = Url::parse(&format!("http://127.0.0.1:{port}/c/f.json?sig=secret")).unwrap();
        (url, handle)
    }

    #[tokio::test]
    async fn upload_sends_block_blob_put_and_accepts_201() {
        let (url, server) = serve_once(
            "HTTP/1.1 201 Created\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
        )
        .await;
        upload_json(&url, "{\"exports\":[]}".to_string())
            .await
            .unwrap();
        let request = server.await.unwrap();
        let lower = request.to_ascii_lowercase();
        assert!(request.starts_with("PUT /c/f.json?sig=secret HTTP/1.1"));
        assert!(lower.contains("x-ms-blob-type: blockblob"));
        assert!(lower.contains("content-type: application/json; charset=utf-8"));
        assert!(request.ends_with("{\"exports\":[]}"));
    }

    #[tokio::test]
    async fn upload_failure_reports_status_and_code_but_not_the_signature() {
        let body =
            "<Error><Code>AuthorizationPermissionMismatch</Code><Message>denied</Message></Error>";
        let response = format!(
            "HTTP/1.1 403 Forbidden\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let (url, server) = serve_once(response).await;
        let err = upload_json(&url, "{}".to_string()).await.unwrap_err();
        server.await.unwrap();
        assert!(err.contains("403"), "{err}");
        assert!(err.contains("AuthorizationPermissionMismatch"), "{err}");
        assert!(!err.contains("secret"), "{err}");
    }

    #[test]
    fn export_success_message_shows_a_ready_to_run_import_command() {
        let url = build_blob_url(
            &format!("https://acct.blob.core.windows.net/c?{SAS}"),
            "2026-10-28_09-05-07_PC.json",
        )
        .unwrap();
        let message = export_success_message(3, &url);
        let lines: Vec<&str> = message.lines().collect();
        assert_eq!(
            lines[0],
            "已匯出 3 筆到 https://acct.blob.core.windows.net/c/2026-10-28_09-05-07_PC.json"
        );
        assert!(!lines[0].contains("sig"));
        assert_eq!(
            lines[2],
            format!(
                "token-usage-insights import --url 'https://acct.blob.core.windows.net/c/2026-10-28_09-05-07_PC.json?{SAS}'"
            )
        );
        // 匯入命令中的網址必須能被 import --url 接受並還原為同一個 Blob。
        let quoted = lines[2].split_once("--url ").unwrap().1;
        let parsed = parse_download_url(quoted.trim_matches('\'')).unwrap();
        assert_eq!(parsed, url);
    }

    #[test]
    fn shell_quote_escapes_single_quotes() {
        assert_eq!(shell_quote("a&b"), "'a&b'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }

    #[test]
    fn read_permission_warning_only_when_sp_lacks_r() {
        let with =
            |sp: &str| Url::parse(&format!("https://a.example/c/f.json?sp={sp}&sig=x")).unwrap();
        assert_eq!(sas_without_read_permission(&with("racwl")), None);
        assert_eq!(
            sas_without_read_permission(&with("cw")),
            Some("cw".to_string())
        );
        assert_eq!(
            sas_without_read_permission(&Url::parse("https://a.example/c/f.json").unwrap()),
            None
        );
    }

    #[test]
    fn download_url_must_be_https_and_point_to_a_file() {
        let ok = "https://acct.blob.core.windows.net/c/2026-10-28_09-05-07_PC.json?sig=x";
        assert!(parse_download_url(ok).is_ok());
        assert!(parse_download_url("https://example.com/all.json").is_ok());
        // 匯出時印出的網址就是容器 SAS（sr=c）加上 Blob 檔名，必須可匯入。
        assert!(parse_download_url(
            "https://acct.blob.core.windows.net/c/f.json?sr=c&sp=racwl&sig=x"
        )
        .is_ok());
        for bad in [
            "http://acct.blob.core.windows.net/c/f.json?sig=x",
            "https://acct.blob.core.windows.net/c/?sig=x",
            "https://acct.blob.core.windows.net/?sig=x",
            "https://acct.blob.core.windows.net/c?sr=c&sig=secret",
            "not a url",
        ] {
            let err = parse_download_url(bad).unwrap_err();
            assert!(!err.contains("secret"), "{err}");
        }
    }

    #[test]
    fn blob_file_name_is_decoded_and_has_no_query() {
        let url = Url::parse(
            "https://acct.blob.core.windows.net/c/2026-10-28_09-05-07_%E4%BF%9D%E5%93%A5.json?sig=secret",
        )
        .unwrap();
        assert_eq!(
            blob_file_name(&url).as_deref(),
            Some("2026-10-28_09-05-07_保哥.json")
        );
        assert_eq!(percent_decode("a%2"), "a%2");
        assert_eq!(percent_decode("a%zz"), "a%zz");
    }

    #[tokio::test]
    async fn download_returns_the_body_on_200() {
        let body = "{\"exports\":[]}";
        let (url, server) = serve_once(format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ))
        .await;
        let text = download_json(&url, 1024).await.unwrap();
        assert_eq!(text, body);
        assert!(server
            .await
            .unwrap()
            .starts_with("GET /c/f.json?sig=secret HTTP/1.1"));
    }

    #[tokio::test]
    async fn download_rejects_oversized_bodies() {
        let body = "x".repeat(64);
        let (url, server) = serve_once(format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ))
        .await;
        let err = download_json(&url, 16).await.unwrap_err();
        server.await.unwrap();
        assert!(err.contains("大小上限"), "{err}");
    }

    #[tokio::test]
    async fn download_failure_reports_status_and_code_but_not_the_signature() {
        let body = "<Error><Code>BlobNotFound</Code><Message>The specified blob does not exist.</Message></Error>";
        let (url, server) = serve_once(format!(
            "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ))
        .await;
        let err = download_json(&url, 1024).await.unwrap_err();
        server.await.unwrap();
        assert!(err.contains("404") && err.contains("BlobNotFound"), "{err}");
        assert!(!err.contains("secret"), "{err}");
    }

    #[tokio::test]
    async fn upload_connection_error_does_not_leak_the_signature() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let url = Url::parse(&format!("http://127.0.0.1:{port}/c/f.json?sig=secret")).unwrap();
        let err = upload_json(&url, "{}".to_string()).await.unwrap_err();
        assert!(!err.contains("secret"), "{err}");
    }
}
