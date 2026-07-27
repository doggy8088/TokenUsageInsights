use axum::{
    extract::DefaultBodyLimit,
    http::{header::CONTENT_TYPE, Method},
    routing::{delete, get, post},
    Router,
};
use std::{
    net::{IpAddr, SocketAddr},
    path::PathBuf,
};
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;

mod db;
mod handlers;
mod paths;
mod pricing;
mod timeline;
mod vscode;

use handlers::*;

const MAX_IMPORT_PAYLOAD_BYTES: usize = 200_000_000;
const DEFAULT_BIND_HOST: &str = "0.0.0.0";

fn import_usage_route() -> axum::routing::MethodRouter {
    post(import_usage_day).layer(DefaultBodyLimit::max(MAX_IMPORT_PAYLOAD_BYTES))
}

fn build_cors_layer() -> CorsLayer {
    let default_port = std::env::var("PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(3003);

    let allowed_origins: Vec<axum::http::HeaderValue> = std::env::var("CORS_ALLOWED_ORIGINS")
        .ok()
        .and_then(|origins| {
            let parsed = origins
                .split(',')
                .filter_map(|origin| {
                    let trimmed = origin.trim();
                    if trimmed.is_empty() {
                        None
                    } else {
                        trimmed.parse::<axum::http::HeaderValue>().ok()
                    }
                })
                .collect::<Vec<_>>();
            if parsed.is_empty() {
                None
            } else {
                Some(parsed)
            }
        })
        .unwrap_or_else(|| {
            vec![
                format!("http://localhost:{default_port}")
                    .parse::<axum::http::HeaderValue>()
                    .unwrap(),
                format!("http://127.0.0.1:{default_port}")
                    .parse::<axum::http::HeaderValue>()
                    .unwrap(),
            ]
        });

    CorsLayer::new()
        .allow_origin(allowed_origins)
        .allow_methods([Method::GET, Method::POST, Method::DELETE, Method::OPTIONS])
        .allow_headers([CONTENT_TYPE])
}

fn parse_bind_address(host: &str, port: u16) -> Result<SocketAddr, String> {
    let host = host.trim();
    let ip_address = host
        .parse::<IpAddr>()
        .map_err(|_| format!("HOST 必須是有效的 IPv4 或 IPv6 位址，目前值為 {host:?}"))?;
    Ok(SocketAddr::new(ip_address, port))
}

fn configured_bind_address(port: u16) -> Result<SocketAddr, String> {
    let host = std::env::var("HOST").unwrap_or_else(|_| DEFAULT_BIND_HOST.to_string());
    parse_bind_address(&host, port)
}

fn browser_url_for_bind_address(bind_address: SocketAddr) -> String {
    if bind_address.ip().is_unspecified() {
        format!("http://localhost:{}", bind_address.port())
    } else {
        format!("http://{bind_address}")
    }
}

fn initialize_database_schema() -> Result<(), String> {
    let conn = db::get_db_conn()?;
    db::init_db(&conn)
}

fn spawn_usage_sync_task() {
    tokio::spawn(async {
        let mut migrate_legacy_databases = true;
        loop {
            let should_migrate = migrate_legacy_databases;
            let sync_res = tokio::task::spawn_blocking(move || {
                let mut conn = db::get_db_conn()?;
                if should_migrate {
                    db::migrate_old_databases(&mut conn)?;
                }
                db::sync_usage_logs(&mut conn)
            })
            .await;

            match sync_res {
                Ok(Ok(())) if should_migrate => {
                    println!("✅ SQLite 資料庫已成功載入並完成增量同步！");
                }
                Ok(Ok(())) => {}
                Ok(Err(error)) => eprintln!("⚠️ 背景日誌同步失敗: {error}"),
                Err(error) => eprintln!("⚠️ 背景日誌同步任務異常: {error:?}"),
            }

            migrate_legacy_databases = false;
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    });
}

#[tokio::main]
async fn main() {
    if let Err(error) = initialize_database_schema() {
        eprintln!("❌ 初始化 SQLite 資料庫失敗: {error}");
    }

    let static_dir = get_static_dir();
    println!("📂 正在服務靜態檔案，目錄來源: {:?}", static_dir);

    // 建立 Axum 路由，支援帶助理前綴的 API 及 fallback 相容 API
    let app = Router::new()
        // 帶 :assistant 變數的路由
        .route("/api/:assistant/dates", get(get_available_dates))
        .route("/api/:assistant/setup-info", get(get_setup_info))
        .route("/api/:assistant/usage/:date", get(get_usage_details))
        .route(
            "/api/:assistant/usage/:date/session-search",
            get(search_sessions_by_user_prompt),
        )
        .route("/api/:assistant/usage/:date/export", get(export_usage_day))
        .route("/api/:assistant/usage/:date/import", import_usage_route())
        .route("/api/:assistant/imports", get(get_usage_import_batches))
        .route(
            "/api/:assistant/imports/:batch_id",
            delete(rollback_usage_import_batch),
        )
        .route(
            "/api/:assistant/session/:session_id",
            get(get_session_details),
        )
        .route("/api/:assistant/months", get(get_available_months))
        .route(
            "/api/:assistant/monthly/:year_month",
            get(get_monthly_details),
        )
        .route("/api/:assistant/years", get(get_available_years))
        .route("/api/:assistant/yearly/:year", get(get_yearly_details))
        .route("/api/:assistant/pricing", get(get_pricing))
        .route("/api/:assistant/sync", get(trigger_manual_sync))
        .route("/api/:assistant/rate-limit", get(get_rate_limit))
        // 靜態檔案路由
        .nest_service("/static", ServeDir::new(&static_dir))
        .fallback_service(ServeDir::new(&static_dir))
        .layer(build_cors_layer());

    let port = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(3003); // 預設使用 3003 Port

    let bind_address = configured_bind_address(port).unwrap_or_else(|error| {
        eprintln!("❌ 無法解析服務綁定位址: {error}");
        std::process::exit(1);
    });
    let listener = tokio::net::TcpListener::bind(bind_address)
        .await
        .unwrap_or_else(|error| {
            eprintln!("❌ 無法綁定服務位址 {bind_address}: {error}");
            std::process::exit(1);
        });
    println!("🌐 服務綁定位址: {bind_address}");
    println!(
        "🚀 Token 戰情室 is running on: {}",
        browser_url_for_bind_address(bind_address)
    );

    // HTTP 先開始監聽；可能耗時的遷移與 transcript 同步在 blocking thread 執行。
    spawn_usage_sync_task();
    axum::serve(listener, app).await.unwrap();
}

/// 獲取靜態檔案的基準路徑
fn get_static_dir() -> PathBuf {
    if let Some(path) = paths::find_resource("static") {
        return path;
    }
    eprintln!("❌ 無法定位 static 目錄。請在專案根目錄下執行此程式。");
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{header::CONTENT_TYPE, Method, Request, StatusCode},
        Router,
    };
    use tower::ServiceExt;

    use super::*;

    #[test]
    fn import_payload_limit_is_200_megabytes() {
        assert_eq!(MAX_IMPORT_PAYLOAD_BYTES, 200_000_000);
    }

    #[test]
    fn parse_bind_address_accepts_ipv4_and_ipv6() {
        assert_eq!(
            parse_bind_address("127.0.0.1", 3003).unwrap(),
            "127.0.0.1:3003".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(
            parse_bind_address("::1", 3003).unwrap(),
            "[::1]:3003".parse::<SocketAddr>().unwrap()
        );
    }

    #[test]
    fn parse_bind_address_rejects_non_ip_host() {
        let error = parse_bind_address("localhost", 3003).unwrap_err();

        assert!(error.contains("IPv4 或 IPv6"));
        assert!(error.contains("localhost"));
    }

    #[test]
    fn browser_url_uses_localhost_for_unspecified_addresses() {
        assert_eq!(
            browser_url_for_bind_address("0.0.0.0:3003".parse().unwrap()),
            "http://localhost:3003"
        );
        assert_eq!(
            browser_url_for_bind_address("[::]:3003".parse().unwrap()),
            "http://localhost:3003"
        );
    }

    #[test]
    fn browser_url_preserves_specific_ipv4_and_ipv6_addresses() {
        assert_eq!(
            browser_url_for_bind_address("127.0.0.1:3003".parse().unwrap()),
            "http://127.0.0.1:3003"
        );
        assert_eq!(
            browser_url_for_bind_address("[::1]:3003".parse().unwrap()),
            "http://[::1]:3003"
        );
    }

    #[tokio::test]
    async fn import_route_allows_json_larger_than_the_default_limit() {
        let app = Router::new().route("/api/:assistant/usage/:date/import", import_usage_route());
        let payload = format!(r#"{{"padding":"{}"}}"#, "x".repeat(3 * 1024 * 1024));
        let request = Request::builder()
            .method(Method::POST)
            .uri("/api/unsupported/usage/2026-07-10/import")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(payload))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
