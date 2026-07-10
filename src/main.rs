use axum::{
    http::{header::CONTENT_TYPE, Method},
    routing::get,
    Router,
};
use std::path::PathBuf;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;

mod db;
mod handlers;
mod paths;
mod pricing;
mod timeline;

use handlers::*;

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
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers([CONTENT_TYPE])
}

#[tokio::main]
async fn main() {
    // 初始化 SQLite 資料庫並進行第一次增量同步與遷移
    if let Ok(mut conn) = db::get_db_conn() {
        if let Err(e) = db::init_db(&conn) {
            eprintln!("❌ 初始化 SQLite 資料庫失敗: {}", e);
        } else {
            // 嘗試從舊的個別資料庫遷移數據 (策略 B)
            if let Err(e) = db::migrate_old_databases(&mut conn) {
                eprintln!("⚠️ 數據遷移遭遇錯誤: {}", e);
            }
            if let Err(e) = db::sync_usage_logs(&mut conn) {
                eprintln!("❌ 初次同步日誌檔到 SQLite 失敗: {}", e);
            } else {
                println!("✅ SQLite 資料庫已成功載入並完成增量同步！");
            }
        }
    } else {
        eprintln!("❌ 無法連結到 SQLite 資料庫");
    }

    // 啟動背景定期日誌同步任務 (每 5 秒執行一次)
    tokio::spawn(async {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            let sync_res = tokio::task::spawn_blocking(|| {
                if let Ok(mut conn) = db::get_db_conn() {
                    db::sync_usage_logs(&mut conn)
                } else {
                    Err("無法建立背景資料庫連接".to_string())
                }
            })
            .await;
            if let Err(e) = sync_res {
                eprintln!("⚠️ 背景日誌同步任務異常: {:?}", e);
            } else if let Ok(Err(e)) = sync_res {
                eprintln!("⚠️ 背景日誌同步失敗: {}", e);
            }
        }
    });

    let static_dir = get_static_dir();
    println!("📂 正在服務靜態檔案，目錄來源: {:?}", static_dir);

    // 建立 Axum 路由，支援帶助理前綴的 API 及 fallback 相容 API
    let app = Router::new()
        // 帶 :assistant 變數的路由
        .route("/api/:assistant/dates", get(get_available_dates))
        .route("/api/:assistant/setup-info", get(get_setup_info))
        .route("/api/:assistant/usage/:date", get(get_usage_details))
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

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port))
        .await
        .unwrap();
    println!("🚀 Token 戰情室 is running on: http://localhost:{}", port);

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
