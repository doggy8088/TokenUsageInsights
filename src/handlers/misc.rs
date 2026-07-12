use axum::{extract::Path, http::StatusCode, response::IntoResponse, Json};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::PathBuf,
};

use super::*;
use crate::db::{self, UsageDayExportRecord};
use crate::pricing::PricingEntry;

#[derive(Serialize)]
struct UsageDayExportResponse {
    version: u8,
    assistant: String,
    date: String,
    exported_at: String,
    records: Vec<UsageDayExportRecord>,
}

#[derive(Deserialize)]
pub struct UsageDayImportRequest {
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub records: Vec<UsageDayExportRecord>,
}

/// API 7: 獲取模型價格清單 ( pricing.csv 資訊)
pub async fn get_pricing(Path(assistant): Path<String>) -> impl IntoResponse {
    let assistant = normalize_assistant_name(&assistant);
    if !is_supported_assistant(&assistant) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "不支援的助理類型" })),
        )
            .into_response();
    }

    let mut entries = Vec::new();
    let file_path =
        crate::paths::find_resource("pricing.csv").unwrap_or_else(|| PathBuf::from("pricing.csv"));
    if let Ok(file) = File::open(&file_path) {
        let reader = BufReader::new(file);
        let mut lines = reader.lines();
        if let Some(Ok(_header)) = lines.next() {
            for line in lines.map_while(Result::ok) {
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
    Json(entries).into_response()
}

/// API 8: 手動觸發日誌增量同步
pub async fn trigger_manual_sync(Path(assistant): Path<String>) -> impl IntoResponse {
    let assistant = normalize_assistant_name(&assistant);
    if !is_supported_assistant(&assistant) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "不支援的助理類型" })),
        )
            .into_response();
    }

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
    let assistant = normalize_assistant_name(&assistant);
    if !is_supported_assistant(&assistant) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "不支援的助理類型" })),
        )
            .into_response();
    }

    if assistant != "codex" {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Only codex is supported" })),
        )
            .into_response();
    }

    let res = tokio::task::spawn_blocking(db::get_latest_codex_rate_limit)
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

fn is_valid_date(date: &str) -> bool {
    let parts: Vec<&str> = date.split('-').collect();
    if parts.len() != 3 {
        return false;
    }

    let year: i32 = match parts[0].parse() {
        Ok(v) => v,
        Err(_) => return false,
    };
    let month: i32 = match parts[1].parse() {
        Ok(v) => v,
        Err(_) => return false,
    };
    let day: i32 = match parts[2].parse() {
        Ok(v) => v,
        Err(_) => return false,
    };

    if year <= 0 || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return false;
    }

    true
}

fn normalize_import_payload_date(
    route_date: &str,
    payload_date: Option<String>,
) -> Result<String, String> {
    if let Some(payload_date) = payload_date {
        if payload_date.trim().is_empty() {
            return Err("缺少匯入檔案日期欄位".to_string());
        }
        if !is_valid_date(&payload_date) {
            return Err("匯入檔案日期格式不正確".to_string());
        }
        if payload_date != route_date {
            return Err(format!(
                "匯入檔案日期 {payload_date} 與 API 路徑日期 {route_date} 不一致"
            ));
        }
        return Ok(payload_date);
    }

    if is_valid_date(route_date) {
        return Ok(route_date.to_string());
    }

    Err("日期格式不正確".to_string())
}

pub async fn export_usage_day(
    Path((assistant, date)): Path<(String, String)>,
) -> impl IntoResponse {
    let assistant = normalize_assistant_name(&assistant);
    if !is_supported_assistant(&assistant) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "不支援的助理類型" })),
        )
            .into_response();
    }

    if !is_valid_date(&date) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "日期格式不正確，請使用 YYYY-MM-DD" })),
        )
            .into_response();
    }

    let assistant_clone = assistant.clone();
    let date_clone = date.clone();
    let export_res = tokio::task::spawn_blocking(move || {
        let conn = db::get_db_conn()?;
        let records = db::export_usage_day_entries(&conn, &assistant_clone, &date_clone)?;
        Ok::<Vec<crate::db::UsageDayExportRecord>, String>(records)
    })
    .await
    .unwrap_or_else(|_| Err("導出任務執行失敗".to_string()));

    match export_res {
        Ok(records) => {
            if records.is_empty() {
                (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({ "error": "指定日期沒有可匯出的使用紀錄" })),
                )
                    .into_response()
            } else {
                let payload = UsageDayExportResponse {
                    version: 1,
                    assistant: assistant.clone(),
                    date: date.clone(),
                    exported_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
                    records,
                };
                (StatusCode::OK, Json(payload)).into_response()
            }
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": err })),
        )
            .into_response(),
    }
}

pub async fn import_usage_day(
    Path((assistant, date)): Path<(String, String)>,
    Json(payload): Json<UsageDayImportRequest>,
) -> impl IntoResponse {
    let assistant = normalize_assistant_name(&assistant);
    if !is_supported_assistant(&assistant) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "不支援的助理類型" })),
        )
            .into_response();
    }

    if !is_valid_date(&date) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "日期格式不正確，請使用 YYYY-MM-DD" })),
        )
            .into_response();
    }

    let import_date = match normalize_import_payload_date(&date, payload.date) {
        Ok(v) => v,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": err })),
            )
                .into_response();
        }
    };

    if payload.records.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "匯入資料為空" })),
        )
            .into_response();
    }

    let assistant_clone = assistant.clone();
    let import_date_clone = import_date.clone();
    let records = payload.records;
    let import_res = tokio::task::spawn_blocking(move || {
        let mut conn = db::get_db_conn()?;
        let summary =
            db::import_usage_day_entries(&mut conn, &assistant_clone, &import_date_clone, records)?;
        Ok::<crate::db::UsageDayImportSummary, String>(summary)
    })
    .await
    .unwrap_or_else(|_| Err("匯入任務執行失敗".to_string()));

    match import_res {
        Ok(summary) => (StatusCode::OK, Json(summary)).into_response(),
        Err(err) => {
            let status = if err.contains("匯入資料日期不一致")
                || err.contains("日期")
                || err.contains("無效")
            {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            (status, Json(serde_json::json!({ "error": err }))).into_response()
        }
    }
}
