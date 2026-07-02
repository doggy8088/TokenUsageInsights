use axum::{extract::Path, http::StatusCode, response::IntoResponse, Json};
use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::PathBuf,
};

use crate::db;
use crate::pricing::PricingEntry;

/// API 7: 獲取模型價格清單 ( pricing.csv 資訊)
pub async fn get_pricing(Path(_assistant): Path<String>) -> impl IntoResponse {
    let mut entries = Vec::new();
    let file_path = PathBuf::from("pricing.csv");
    if let Ok(file) = File::open(&file_path) {
        let reader = BufReader::new(file);
        let mut lines = reader.lines();
        if let Some(Ok(_header)) = lines.next() {
            for line in lines.flatten() {
                let parts: Vec<&str> = line.split(',').collect();
                if parts.len() >= 6 {
                    let input_price = parts[3].trim().parse::<f64>().unwrap_or(0.0);
                    let cache_input_price = parts[4].trim().parse::<f64>().unwrap_or(0.0);
                    let output_price = parts[5].trim().parse::<f64>().unwrap_or(0.0);
                    let batch_api_price = if parts.len() >= 7 {
                        parts[6].trim().to_string()
                    } else {
                        "N/A".to_string()
                    };
                    entries.push(PricingEntry {
                        model_name: parts[0].trim().to_string(),
                        deployment_type: parts[1].trim().to_string(),
                        unit: parts[2].trim().to_string(),
                        input_price,
                        cache_input_price,
                        output_price,
                        batch_api_price,
                    });
                }
            }
        }
    }
    if entries.is_empty() {
        entries = vec![
            PricingEntry {
                model_name: "Gemini 3.5 Flash".to_string(),
                deployment_type: "Google AI".to_string(),
                unit: "1M Tokens".to_string(),
                input_price: 1.50,
                cache_input_price: 0.375,
                output_price: 9.00,
                batch_api_price: "0.75/0.1875/4.50".to_string(),
            },
            PricingEntry {
                model_name: "Gemini 1.5 Flash".to_string(),
                deployment_type: "Google AI".to_string(),
                unit: "1M Tokens".to_string(),
                input_price: 0.075,
                cache_input_price: 0.01875,
                output_price: 0.30,
                batch_api_price: "0.0375/0.009375/0.15".to_string(),
            },
            PricingEntry {
                model_name: "Gemini 1.5 Pro".to_string(),
                deployment_type: "Google AI".to_string(),
                unit: "1M Tokens".to_string(),
                input_price: 1.25,
                cache_input_price: 0.3125,
                output_price: 5.00,
                batch_api_price: "0.625/0.15625/2.50".to_string(),
            },
            PricingEntry {
                model_name: "Gemini 2.0 Flash".to_string(),
                deployment_type: "Google AI".to_string(),
                unit: "1M Tokens".to_string(),
                input_price: 0.10,
                cache_input_price: 0.025,
                output_price: 0.40,
                batch_api_price: "0.05/0.0125/0.20".to_string(),
            },
        ];
    }
    Json(entries)
}

/// API 8: 手動觸發日誌增量同步
pub async fn trigger_manual_sync(Path(_assistant): Path<String>) -> impl IntoResponse {
    let sync_res = tokio::task::spawn_blocking(|| {
        if let Ok(mut conn) = db::get_db_conn() {
            db::sync_usage_logs(&mut conn)
        } else {
            Err("無法連接至 SQLite 資料庫".to_string())
        }
    })
    .await;

    match sync_res {
        Ok(Ok(_)) => (
            StatusCode::OK,
            Json(serde_json::json!({ "status": "success", "message": "手動增量同步已成功完成！" })),
        )
            .into_response(),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "status": "error", "message": format!("同步失敗: {}", e) })),
        )
            .into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "status": "error", "message": "執行緒執行失敗" })),
        )
            .into_response(),
    }
}

/// API: 獲取 Codex 的 rate limit 資料
pub async fn get_rate_limit(Path(assistant): Path<String>) -> impl IntoResponse {
    if assistant != "codex" {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Only codex is supported" })),
        )
            .into_response();
    }

    let res = tokio::task::spawn_blocking(move || db::get_latest_codex_rate_limit())
        .await
        .unwrap();

    match res {
        Some(val) => (StatusCode::OK, Json(val)).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "No rate limit data found" })),
        )
            .into_response(),
    }
}
