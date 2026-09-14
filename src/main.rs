use axum::{
    extract::DefaultBodyLimit,
    http::{header::CACHE_CONTROL, header::CONTENT_TYPE, HeaderValue, Method},
    routing::{delete, get, post},
    Router,
};
use std::{
    net::{IpAddr, SocketAddr},
    path::PathBuf,
};
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;

mod browser;
mod cli;
mod db;
mod grok;
mod handlers;
mod muse;
mod omp;
mod paths;
mod pi;
mod pricing;
mod reporting;
mod session_details;
mod timeline;
mod updater;
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

/// 背景日誌同步任務的控制代碼：支援通知停止並等待進行中的同步（含 spawn_blocking 的 SQLite 寫入）完成
struct UsageSyncTask {
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    handle: tokio::task::JoinHandle<()>,
}

/// 等待背景日誌同步結束時，每次檢查間隔同時也是警告輸出的節奏
const SYNC_JOIN_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// 新版看板確認健康後延後提交移交交易的時間：讓 axum::serve 的立即失敗得以先反映，
/// 避免在服務確實可用之前就提交並刪除唯一的回滾備份
const HANDOFF_COMMIT_DELAY: std::time::Duration = std::time::Duration::from_secs(2);

/// 自動更新請求與終止訊號可能幾乎同時抵達時的仲裁窗口；
/// 訊號處理任務為非同步執行，需保留短暫時間讓稍後抵達的終止訊號得以優先處理
const SIGNAL_ARBITRATION_WINDOW: std::time::Duration = std::time::Duration::from_millis(500);

impl UsageSyncTask {
    /// 通知同步迴圈停止，並等待進行中的同步（含 spawn_blocking 的 SQLite 寫入）確實結束後才返回。
    ///
    /// spawn_blocking 無法被取消，逾時也不會中斷既有的寫入，因此這裡不設總逾時而改為定期輸出警示並持續等待：
    /// 程序絕不在資料庫寫入途中被本流程終止（服務管理器的 TimeoutStopSec 仍是最終防線）
    async fn shutdown(self) {
        let _ = self.shutdown_tx.send(true);
        let mut handle = self.handle;
        let mut waited = std::time::Duration::ZERO;
        while !Self::wait_for_completion(&mut handle, SYNC_JOIN_POLL_INTERVAL).await {
            waited += SYNC_JOIN_POLL_INTERVAL;
            eprintln!(
                "⏳ 背景日誌同步仍在進行中（已等待 {} 秒）；將持續等待 SQLite 寫入結束後才繼續停機或更新流程...",
                waited.as_secs()
            );
        }
    }

    /// 等待同步任務結束；回傳 false 代表在指定時間內仍未結束，但任務本身持續執行、不會被取消
    async fn wait_for_completion(
        handle: &mut tokio::task::JoinHandle<()>,
        timeout: std::time::Duration,
    ) -> bool {
        tokio::time::timeout(timeout, handle).await.is_ok()
    }
}

fn spawn_usage_sync_task() -> UsageSyncTask {
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
    let handle = tokio::spawn(async move {
        let mut migrate_legacy_databases = true;
        loop {
            // 收到停機通知時不再排入新的同步，讓進行中的同步（若有的話）自然結束
            if *shutdown_rx.borrow() {
                break;
            }

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

            // 以可取消的等待取代固定睡眠，停機時可立即結束迴圈而不必等滿 5 秒
            let stop_requested = tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => false,
                changed = shutdown_rx.changed() => changed.is_err() || *shutdown_rx.borrow(),
            };
            if stop_requested {
                break;
            }
        }
    });

    UsageSyncTask {
        shutdown_tx,
        handle,
    }
}

#[tokio::main]
async fn main() {
    updater::wait_for_parent_exit_if_requested().await;

    if let Some(code) = cli::run(&std::env::args().collect::<Vec<_>>()).await {
        std::process::exit(code);
    }

    // 看板服務啟動前優先檢查並執行本機交易救援（若先前更新意外中斷）
    updater::perform_startup_recovery().await;

    let database_ready = match initialize_database_schema() {
        Ok(()) => true,
        Err(error) => {
            eprintln!("❌ 初始化 SQLite 資料庫失敗: {error}");
            false
        }
    };

    // 資料庫結構初始化失敗代表所有資料庫相關 API 皆無法服務：一律以錯誤碼終止啟動，交由服務管理器重啟與復原。
    // 若本次啟動帶著尚未提交的更新交易（.backup/.handing_off），繼續提供服務還會讓服務管理器判定健康而提交更新
    // 並刪除唯一的回滾備份；因此必須在建立 PID 與綁定連接埠之前終止，讓重啟後的啟動救援自動回滾至先前版本
    if !database_ready {
        let has_pending_handoff = match updater::detect_environment() {
            updater::EnvironmentKind::StandardInstalled { install_dir, .. } => {
                updater::has_pending_handoff_transaction(&install_dir)
            }
            _ => false,
        };

        if has_pending_handoff {
            eprintln!(
                "❌ SQLite 資料庫結構初始化失敗且存在未提交的更新交易；保留更新備份 (.backup) 並終止本次啟動，下次啟動將自動回滾至先前版本。"
            );
            updater::log_update(
                "ERROR",
                "STARTUP",
                "資料庫結構初始化失敗；保留備份目錄並終止啟動以觸發自動回滾",
            );
        } else {
            eprintln!(
                "❌ SQLite 資料庫結構初始化失敗；資料庫相關 API 無法服務，終止本次啟動以交由服務管理器重啟與復原。"
            );
            updater::log_update(
                "ERROR",
                "STARTUP",
                "資料庫結構初始化失敗；終止啟動以避免對外提供無法服務的看板",
            );
        }

        std::process::exit(1);
    }

    // 建立協調式優雅停機通知通道，供系統終止信號與背景自動更新任務協同使用
    // 移交提交閘門：收到更新請求或進入更新流程時必須關閉，避免本世代（舊版）的延遲任務在更新期間誤提交並刪除唯一的回滾備份
    let handoff_commit_allowed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let (shutdown_reason_tx, mut shutdown_reason_rx) =
        tokio::sync::mpsc::channel::<updater::ShutdownReason>(1);
    let (graceful_tx, graceful_rx) = tokio::sync::oneshot::channel::<()>();

    // 記錄終止訊號是否已抵達：自動更新路徑需要據此讓訊號優先於更新請求
    let signal_received = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    // 啟動背景非阻塞自動更新檢查（若非標準安裝或檢查間隔未滿將自動略過）
    updater::spawn_background_auto_update(shutdown_reason_tx.clone());

    // 服務 runner 可透過 .service_stop_requested 要求看板優雅停機：
    // 讓進程完成進行中的資料庫寫入後自行退出，避免以強制終止中斷 SQLite 寫入
    if let updater::EnvironmentKind::StandardInstalled { install_dir, .. } =
        updater::detect_environment()
    {
        let stop_watch_tx = shutdown_reason_tx.clone();
        let stop_watch_signal = signal_received.clone();
        tokio::spawn(async move {
            let stop_request = install_dir.join(".service_stop_requested");
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                if stop_request.exists() {
                    let _ = std::fs::remove_file(&stop_request);
                    println!("👋 收到服務 runner 之優雅停機要求，開始結束服務...");
                    updater::log_update(
                        "INFO",
                        "SHUTDOWN",
                        "收到服務 runner 之優雅停機要求 (.service_stop_requested)",
                    );
                    // 與終止訊號共用同一旗標：自動更新路徑據此讓停機要求優先於更新請求
                    stop_watch_signal.store(true, std::sync::atomic::Ordering::SeqCst);
                    let _ = stop_watch_tx.send(updater::ShutdownReason::Signal).await;
                    return;
                }
            }
        });
    }

    let signal_tx = shutdown_reason_tx.clone();
    let signal_received_watcher = signal_received.clone();
    tokio::spawn(async move {
        shutdown_signal(signal_tx, signal_received_watcher).await;
    });

    let shutdown_reason_task = tokio::spawn({
        let commit_gate = handoff_commit_allowed.clone();
        async move {
            let reason = shutdown_reason_rx
                .recv()
                .await
                .unwrap_or(updater::ShutdownReason::Signal);
            // 收到任何停機原因即關閉本世代的移交提交閘門：
            // 程序在完成自身健康確認前即要停止時，延遲提交任務不得刪除唯一的回滾備份
            commit_gate.store(false, std::sync::atomic::Ordering::SeqCst);
            let _ = graceful_tx.send(());
            reason
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
        .route("/api/:assistant/model-sessions", get(get_model_sessions))
        .route("/api/:assistant/years", get(get_available_years))
        .route("/api/:assistant/yearly/:year", get(get_yearly_details))
        .route("/api/:assistant/pricing", get(get_pricing))
        .route("/api/:assistant/sync", get(trigger_manual_sync))
        .route("/api/:assistant/rate-limit", get(get_rate_limit))
        // 靜態檔案路由
        // 一律附加 Cache-Control: no-cache，強制瀏覽器每次都向伺服器驗證
        // （ServeDir 會自動處理 ETag/Last-Modified 條件式請求，未變更的
        // 檔案仍會回 304 節省頻寬），避免部署更新後使用者仍看到瀏覽器
        // 快取的舊版 JS/CSS（即使忘記更新 ?v=N 版本號也不受影響）。
        .nest_service(
            "/static",
            tower::ServiceBuilder::new()
                .layer(SetResponseHeaderLayer::overriding(
                    CACHE_CONTROL,
                    HeaderValue::from_static("no-cache"),
                ))
                .service(ServeDir::new(&static_dir)),
        )
        .fallback_service(
            tower::ServiceBuilder::new()
                .layer(SetResponseHeaderLayer::overriding(
                    CACHE_CONTROL,
                    HeaderValue::from_static("no-cache"),
                ))
                .service(ServeDir::new(&static_dir)),
        )
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
    let browser_url = browser_url_for_bind_address(bind_address);
    browser::announce_dashboard(&browser_url);

    // HTTP 先開始監聽；可能耗時的遷移與 transcript 同步在 blocking thread 執行。
    let usage_sync_task = spawn_usage_sync_task();
    let pid_guard = updater::create_server_pid_guard();
    if let updater::EnvironmentKind::StandardInstalled { install_dir, .. } =
        updater::detect_environment()
    {
        // Windows 服務 runner 監管模式下，更新提交與備份清理由 runner 於新版進程確認健康就緒後執行；
        // 此處提早提交會刪除備份而使 runner 失去回滾依據，無法在服務後續啟動失敗時還原舊版
        // 僅 Windows 服務 runner 監管模式才由 runner 提交；Unix 即使在環境變數設定下也不可略過提交，
        // 否則 .backup/.handing_off 會永久殘留，下次啟動將誤判為未完成的更新交易
        if cfg!(windows) && updater::is_windows_service_runner() {
            updater::log_update(
                "INFO",
                "STARTUP",
                "偵測到 Windows 服務 runner 監管模式；更新提交與備份清理交由 runner 於健康驗證通過後執行",
            );
        } else {
            // 移交提交會刪除唯一的回滾備份，因此延後到服務確實開始提供後才執行：
            // 若 axum::serve 立即失敗或程序在就緒前退出，本任務會隨程序結束而不會誤提交
            let commit_install_dir = install_dir.clone();
            let commit_gate = handoff_commit_allowed.clone();
            tokio::spawn(async move {
                tokio::time::sleep(HANDOFF_COMMIT_DELAY).await;
                if !commit_gate.load(std::sync::atomic::Ordering::SeqCst) {
                    updater::log_update(
                        "INFO",
                        "STARTUP",
                        "更新流程已開始；略過本世代之移交提交，改由新版程序於健康就緒後提交",
                    );
                    return;
                }
                // 提交可能因其他更新程序持有更新鎖而暫時無法進行：改為有界重試，
                // 避免移交交易與備份永久殘留而阻擋後續更新
                for _ in 0..120 {
                    updater::complete_handoff_and_commit_if_needed(&commit_install_dir);
                    if !updater::has_pending_handoff_transaction(&commit_install_dir) {
                        return;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
                updater::log_update(
                    "WARN",
                    "STARTUP",
                    "移交提交重試逾時；備份交易將由後續啟動或更新程序處理",
                );
            });
        }
    }
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = graceful_rx.await;
        })
        .await
        .unwrap();

    // HTTP 伺服器已停止服務，但背景日誌同步（含 spawn_blocking 中的 SQLite 寫入）可能仍在進行；
    // 必須等待其確實結束後才繼續停機或更新流程。`.server.pid` 刻意保留到此時才移除，
    // 讓更新程序與服務 runner 能以 PID 檔消失作為「停機與資料庫寫入皆已完成」的可觀察訊號
    usage_sync_task.shutdown().await;

    drop(pid_guard);

    let shutdown_reason = shutdown_reason_task
        .await
        .unwrap_or(updater::ShutdownReason::Signal);
    match shutdown_reason {
        updater::ShutdownReason::Signal => {
            println!("👋 接收到終止信號，Token 戰情室已安全停止。");
        }
        updater::ShutdownReason::AutoUpdate(mut opts) => {
            // 終止訊號即為本次更新的取消旗標：更新流程會在開始檔案替換前重新仲裁並中止
            opts.cancel_flag = Some(signal_received.clone());
            let (target_exe, backup_dir, install_dir) = match updater::detect_environment() {
                updater::EnvironmentKind::StandardInstalled { install_dir, .. } => (
                    updater::get_target_exe(&install_dir),
                    install_dir.join(".backup"),
                    Some(install_dir),
                ),
                _ => (
                    std::env::current_exe().unwrap_or_else(|_| PathBuf::from(updater::APP_NAME)),
                    PathBuf::from(".backup"),
                    None,
                ),
            };
            let args: Vec<String> = std::env::args().collect();

            // 一般終止訊號優先於自動更新：背景更新檢查可能先送出更新請求，若使用者隨後要求停止服務，
            // 該訊號會排在更新請求之後；訊號處理任務為非同步執行，因此保留短暫仲裁窗口後再次確認
            if !signal_received.load(std::sync::atomic::Ordering::SeqCst) {
                tokio::time::sleep(SIGNAL_ARBITRATION_WINDOW).await;
            }
            if signal_received.load(std::sync::atomic::Ordering::SeqCst) {
                println!("👋 接收到終止信號（優先於自動更新），Token 戰情室已安全停止。");
                updater::log_update(
                    "INFO",
                    "RESTART",
                    "終止信號優先於自動更新；取消本次更新並停止服務",
                );
                return;
            }

            // 開始更新前先收斂既有的移交交易（例如本次啟動的延遲提交任務尚未執行）：
            // 否則殘留備份會讓後續 backup_installation 拒絕本次更新
            if let Some(dir) = install_dir.as_deref() {
                updater::complete_handoff_and_commit_if_needed(dir);
            }

            println!("🔄 看板服務已完成優雅停機，正在執行自動更新並套用新版本...");
            updater::log_update("INFO", "RESTART", "服務已優雅停機，開始執行自動更新");

            // 進入更新流程前關閉本世代的移交提交閘門：更新完成後由新版程序自行提交，
            // 避免舊世代的延遲任務在更新期間刪除唯一的回滾備份
            handoff_commit_allowed.store(false, std::sync::atomic::Ordering::SeqCst);

            match updater::run_update(opts).await {
                Ok(outcome) if !outcome.installed => {
                    // 另一個更新程序已搶先完成升級，或已是最新版本而無需安裝：此時不得重啟服務
                    let current = install_dir
                        .as_deref()
                        .map(updater::get_installed_version)
                        .unwrap_or_else(|| "最新版".to_string());
                    println!(
                        "✅ 無需變更安裝（其他更新程序已完成或已是最新版本），目前版本 v{current}；維持服務運作。"
                    );
                    updater::log_update(
                        "INFO",
                        "RESTART",
                        &format!("未執行安裝（其他更新程序已完成或已是最新版本），維持目前版本 v{current} 並重新啟動服務"),
                    );
                    // 取得結果期間可能已收到終止訊號：此時應依停機要求停止，而非把停機轉為重啟
                    if signal_received.load(std::sync::atomic::Ordering::SeqCst) {
                        println!("👋 未執行安裝且已收到終止信號，依停機要求停止服務。");
                        updater::log_update(
                            "INFO",
                            "RESTART",
                            "未執行安裝且已收到終止信號；停止服務且不重啟",
                        );
                        return;
                    }
                    // 目前伺服器已為自動更新優雅停機：此處必須重新啟動，否則手動啟動且未受監管的安裝會永久停止
                    updater::restart_current_process(&target_exe, &args);
                }
                Ok(_outcome) => {
                    let installed = install_dir
                        .as_deref()
                        .map(updater::get_installed_version)
                        .unwrap_or_else(|| "最新版".to_string());
                    // 重啟前再次仲裁：若終止訊號在安裝完成後才抵達，應依停機要求停止而非重啟
                    if signal_received.load(std::sync::atomic::Ordering::SeqCst) {
                        println!("👋 更新已完成但收到終止信號，依停機要求停止服務。");
                        updater::log_update(
                            "INFO",
                            "RESTART",
                            "更新已完成但收到終止信號；停止服務且不重啟",
                        );
                        return;
                    }
                    println!("🔄 更新完成，正在自動重啟 Token 戰情室至新版 v{installed}...");
                    updater::log_update(
                        "INFO",
                        "RESTART",
                        &format!("更新完成，重啟目前服務至 v{installed}"),
                    );
                    updater::restart_current_process(&target_exe, &args);
                }
                Err(updater::UpdateError::RollbackFailed(err)) => {
                    eprintln!(
                        "❌ 自動更新失敗且回滾復原亦失敗: {err}；為防止載入損毀狀態，中止重啟以保留備份 ({backup_dir:?})。請依備份手動復原。"
                    );
                    updater::log_update(
                        "ERROR",
                        "RESTART",
                        &format!("更新失敗且回滾失敗 ({err})，中止重啟以保留備份狀態"),
                    );
                    std::process::exit(1);
                }
                Err(err) => {
                    let has_rollback_failed = backup_dir.join(".rollback_failed").exists()
                        || install_dir
                            .as_ref()
                            .map(|d| d.join(".rollback_failed").exists())
                            .unwrap_or(false);
                    if has_rollback_failed {
                        eprintln!(
                            "❌ 自動更新失敗且回滾復原亦失敗；為防止載入損毀狀態，中止重啟以保留備份 ({backup_dir:?})。請依備份手動復原。"
                        );
                        updater::log_update(
                            "ERROR",
                            "RESTART",
                            "更新失敗且回滾失敗，中止重啟以保留備份狀態",
                        );
                        std::process::exit(1);
                    }
                    if signal_received.load(std::sync::atomic::Ordering::SeqCst) {
                        println!("👋 更新已依終止信號取消，Token 戰情室已安全停止。");
                        updater::log_update(
                            "INFO",
                            "RESTART",
                            &format!("更新已依終止信號取消 ({err})；停止服務且不重啟"),
                        );
                        return;
                    }
                    eprintln!("❌ 自動更新失敗: {err}；正在重啟以維持服務運作...");
                    updater::log_update(
                        "ERROR",
                        "RESTART",
                        &format!("自動更新失敗: {err}；重啟原服務"),
                    );
                    updater::restart_current_process(&target_exe, &args);
                }
            }
        }
    }
}

async fn shutdown_signal(
    shutdown_tx: tokio::sync::mpsc::Sender<updater::ShutdownReason>,
    signal_received: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut sig) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            sig.recv().await;
        } else {
            std::future::pending::<()>().await;
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    // 先記錄訊號已抵達，再送出停機原因，確保自動更新路徑讀取旗標時訊號已確實發生
    signal_received.store(true, std::sync::atomic::Ordering::SeqCst);
    let _ = shutdown_tx.send(updater::ShutdownReason::Signal).await;
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

    #[tokio::test]
    async fn usage_sync_task_shutdown_waits_for_inflight_sync() {
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
        let finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let finished_flag = finished.clone();

        let handle = tokio::spawn(async move {
            // 模擬進行中的同步：等待停機通知後才結束
            while !*shutdown_rx.borrow_and_update() {
                if shutdown_rx.changed().await.is_err() {
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            finished_flag.store(true, std::sync::atomic::Ordering::SeqCst);
        });

        let task = UsageSyncTask {
            shutdown_tx,
            handle,
        };
        task.shutdown().await;

        assert!(
            finished.load(std::sync::atomic::Ordering::SeqCst),
            "shutdown 必須等待進行中的同步結束後才返回，避免更新流程在 SQLite 寫入中途終止程序"
        );
    }

    #[tokio::test]
    async fn usage_sync_wait_for_completion_keeps_running_blocking_task() {
        let (_shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
        let release = std::sync::Arc::new(tokio::sync::Notify::new());
        let release_watcher = release.clone();

        let mut handle = tokio::spawn(async move {
            // 模擬無法取消的 spawn_blocking 同步：收到通知後仍需時間完成
            while !*shutdown_rx.borrow_and_update() {
                if shutdown_rx.changed().await.is_err() {
                    break;
                }
            }
            release_watcher.notified().await;
        });

        let completed =
            UsageSyncTask::wait_for_completion(&mut handle, std::time::Duration::from_millis(50))
                .await;

        assert!(!completed, "同步仍在進行時應回報未完成");
        assert!(
            !handle.is_finished(),
            "等待逾時不得取消或丟棄仍在執行的同步任務（spawn_blocking 無法被取消）"
        );

        // 送出停機通知並讓模擬中的同步完成，驗證任務仍可正常結束
        let _ = _shutdown_tx.send(true);
        release.notify_one();
        handle.await.unwrap();
    }

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
