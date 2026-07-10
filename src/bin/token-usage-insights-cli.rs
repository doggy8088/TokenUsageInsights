use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::PathBuf;

mod db;
mod paths;

const EXPORT_VERSION: u8 = 1;
const HELP_TEXT: &str = r#"Token 使用量 CLI 匯入 / 匯出工具

用途:
  export  匯出某日資料為 JSON（可重複匯入且支援重複資料去重）
  import  匯入匯出 JSON 檔（預設以檔內日期為準，可用 --date 覆蓋）

共用參數:
  --agent <name>      助理名稱: antigravity / copilot / codex / claude / cursor
                     亦可使用 claude-code / claude_code / claudecode（會正規化為 claude）

匯出:
  token-usage-insights-cli export --agent <name> --date YYYY-MM-DD --out <path>
  例如:
  token-usage-insights-cli export --agent codex --date 2026-07-09 --out daily.json

匯入:
  token-usage-insights-cli import --agent <name> --file <path> [--date YYYY-MM-DD]
  例如:
  token-usage-insights-cli import --agent codex --file daily.json
  token-usage-insights-cli import --agent codex --file daily.json --date 2026-07-09

注意:
  - 若未指定 export 的 --out，會直接輸出到 stdout
  - import 會以 `assistant_type + import_source_id` 做資料去重，重複匯入只會插入一次
"#;

#[derive(Serialize)]
struct UsageDayExportPayload {
    version: u8,
    assistant: String,
    date: String,
    exported_at: String,
    records: Vec<db::UsageDayExportRecord>,
}

#[derive(Deserialize)]
struct UsageDayImportPayload {
    #[serde(default)]
    version: Option<u8>,
    #[serde(default)]
    assistant: Option<String>,
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    exported_at: Option<String>,
    #[serde(default)]
    records: Vec<db::UsageDayExportRecord>,
}

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        print_help();
        return 1;
    }

    match args[1].as_str() {
        "export" => run_export(&args[2..]),
        "import" => run_import(&args[2..]),
        "-h" | "--help" | "help" => {
            print_help();
            0
        }
        _ => {
            eprintln!("未知指令：{}", args[1]);
            print_help();
            2
        }
    }
}

fn run_export(args: &[String]) -> i32 {
    if has_help(args) {
        print_export_help();
        return 0;
    }

    let mut assistant = None::<String>;
    let mut date = None::<String>;
    let mut out_path = None::<String>;

    let mut i = 0usize;
    while i < args.len() {
        match args[i].as_str() {
            "--agent" => {
                assistant = Some(next_flag_value(args, &mut i, "agent"));
            }
            "--date" => {
                date = Some(next_flag_value(args, &mut i, "date"));
            }
            "--out" => {
                out_path = Some(next_flag_value(args, &mut i, "out"));
            }
            arg => {
                eprintln!("未知參數: {arg}");
                return 2;
            }
        }
        i += 1;
    }

    let assistant = match assistant {
        Some(v) => normalize_assistant_name(&v),
        None => {
            eprintln!("缺少 --agent");
            return 2;
        }
    };

    let date = match date {
        Some(v) => v,
        None => {
            eprintln!("缺少 --date");
            return 2;
        }
    };

    if !is_supported_assistant(&assistant) {
        eprintln!("不支援的助理類型: {assistant}");
        return 2;
    }

    if !is_valid_date(&date) {
        eprintln!("日期格式不正確，請使用 YYYY-MM-DD");
        return 2;
    }

    let mut conn = match db::get_db_conn() {
        Ok(conn) => conn,
        Err(err) => {
            eprintln!("開啟資料庫失敗: {err}");
            return 1;
        }
    };

    if let Err(err) = db::init_db(&conn) {
        eprintln!("初始化資料庫失敗: {err}");
        return 1;
    }

    let records = match db::export_usage_day_entries(&conn, &assistant, &date) {
        Ok(v) => v,
        Err(err) => {
            eprintln!("匯出資料失敗: {err}");
            return 1;
        }
    };

    if records.is_empty() {
        eprintln!("指定日期沒有可匯出的資料");
        return 1;
    }

    let payload = UsageDayExportPayload {
        version: EXPORT_VERSION,
        assistant: assistant.clone(),
        date: date.clone(),
        exported_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        records,
    };

    let json = match serde_json::to_string_pretty(&payload) {
        Ok(v) => v,
        Err(err) => {
            eprintln!("產生匯出 JSON 失敗: {err}");
            return 1;
        }
    };

    match out_path {
        Some(out) => {
            if let Err(err) = fs::write(PathBuf::from(&out), json) {
                eprintln!("寫入檔案失敗 {out}: {err}");
                return 1;
            }
            println!("已匯出 {} 筆到 {out}", payload.records.len());
        }
        None => {
            println!("{json}");
        }
    }

    0
}

fn run_import(args: &[String]) -> i32 {
    if has_help(args) {
        print_import_help();
        return 0;
    }

    let mut assistant = None::<String>;
    let mut date = None::<String>;
    let mut file_path = None::<String>;

    let mut i = 0usize;
    while i < args.len() {
        match args[i].as_str() {
            "--agent" => {
                assistant = Some(next_flag_value(args, &mut i, "agent"));
            }
            "--date" => {
                date = Some(next_flag_value(args, &mut i, "date"));
            }
            "--file" => {
                file_path = Some(next_flag_value(args, &mut i, "file"));
            }
            arg => {
                eprintln!("未知參數: {arg}");
                return 2;
            }
        }
        i += 1;
    }

    let assistant = match assistant {
        Some(v) => normalize_assistant_name(&v),
        None => {
            eprintln!("缺少 --agent");
            return 2;
        }
    };

    if !is_supported_assistant(&assistant) {
        eprintln!("不支援的助理類型: {assistant}");
        return 2;
    }

    let file_path = match file_path {
        Some(v) => PathBuf::from(v),
        None => {
            eprintln!("缺少 --file");
            return 2;
        }
    };

    if !file_path.exists() {
        eprintln!("找不到檔案: {:?}", file_path);
        return 1;
    }

    let input = match fs::read_to_string(&file_path) {
        Ok(v) => v,
        Err(err) => {
            eprintln!("讀取匯入檔案失敗: {err}");
            return 1;
        }
    };

    let payload: UsageDayImportPayload = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(err) => {
            eprintln!("解析 JSON 失敗: {err}");
            return 1;
        }
    };

    let imported_from = match normalize_import_date(date, payload.date) {
        Ok(v) => v,
        Err(err) => {
            eprintln!("{err}");
            return 2;
        }
    };

    if let Some(payload_assistant) = payload.assistant {
        let normalized_payload_assistant = normalize_assistant_name(&payload_assistant);
        if normalized_payload_assistant != assistant {
            eprintln!(
                "警告：檔案內 assistant={normalized_payload_assistant}，但目前指定為 {assistant}，將以 CLI 指定值匯入。"
            );
        }
    }

    if payload.records.is_empty() {
        eprintln!("匯入檔案沒有 records");
        return 2;
    }

    let mut conn = match db::get_db_conn() {
        Ok(conn) => conn,
        Err(err) => {
            eprintln!("開啟資料庫失敗: {err}");
            return 1;
        }
    };

    if let Err(err) = db::init_db(&conn) {
        eprintln!("初始化資料庫失敗: {err}");
        return 1;
    }

    let summary = match db::import_usage_day_entries(&mut conn, &assistant, &imported_from, payload.records) {
        Ok(v) => v,
        Err(err) => {
            eprintln!("匯入失敗: {err}");
            return 1;
        }
    };

    match serde_json::to_string_pretty(&summary) {
        Ok(out) => println!("{out}"),
        Err(err) => {
            eprintln!("輸出匯入結果失敗: {err}");
            return 1;
        }
    }

    0
}

fn normalize_import_date(route_date: Option<String>, file_date: Option<String>) -> Result<String, String> {
    if let Some(file_date) = file_date {
        if file_date.trim().is_empty() {
            return Err("匯入檔案日期欄位不能為空".to_string());
        }

        if !is_valid_date(&file_date) {
            return Err(format!("匯入檔案日期欄位格式不正確: {file_date}"));
        }

        if let Some(route_date) = route_date {
            if !is_valid_date(&route_date) {
                return Err(format!("--date 格式不正確: {route_date}"));
            }

            if route_date != file_date {
                return Err(format!("匯入檔案日期 {file_date} 與 --date 指定 {route_date} 不一致"));
            }
            return Ok(route_date);
        }

        return Ok(file_date);
    }

    if let Some(route_date) = route_date {
        if !is_valid_date(&route_date) {
            return Err(format!("--date 格式不正確: {route_date}"));
        }
        return Ok(route_date);
    }

    Err("缺少日期：請在匯入檔案內提供 date 欄位，或使用 --date 指定".to_string())
}

fn next_flag_value(args: &[String], i: &mut usize, flag: &str) -> String {
    match args.get(*i + 1) {
        Some(value) => {
            if value.starts_with("--") {
                eprintln!("缺少 --{flag} 的值");
                std::process::exit(2);
            }
            *i += 1;
            value.clone()
        }
        None => {
            eprintln!("缺少 --{flag} 的值");
            std::process::exit(2);
        }
    }
}

fn normalize_assistant_name(assistant: &str) -> String {
    let normalized = assistant.trim().to_lowercase();
    match normalized.as_str() {
        "claude-code" | "claude_code" | "claudecode" => "claude".to_string(),
        "cursor" => "cursor".to_string(),
        _ => normalized,
    }
}

fn is_supported_assistant(assistant: &str) -> bool {
    matches!(
        normalize_assistant_name(assistant).as_str(),
        "antigravity" | "copilot" | "codex" | "claude" | "cursor"
    )
}

fn is_valid_date(date: &str) -> bool {
    let parts: Vec<&str> = date.split('-').collect();
    if parts.len() != 3 {
        return false;
    }
    let year = match parts[0].parse::<i32>() {
        Ok(v) => v,
        Err(_) => return false,
    };
    let month = match parts[1].parse::<i32>() {
        Ok(v) => v,
        Err(_) => return false,
    };
    let day = match parts[2].parse::<i32>() {
        Ok(v) => v,
        Err(_) => return false,
    };
    if year <= 0 || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return false;
    }
    true
}

fn print_help() {
    println!("{HELP_TEXT}");
}

fn print_export_help() {
    println!(
        r#"export usage:
  token-usage-insights-cli export --agent <name> --date YYYY-MM-DD --out <path>

參數:
  --agent <name>    助理名稱（antigravity/copilot/codex/claude/cursor）
  --date YYYY-MM-DD  匯出日期
  --out <path>      輸出檔案路徑，不指定則輸出到 stdout
  --help, -h        顯示此說明
"#
    );
}

fn print_import_help() {
    println!(
        r#"import usage:
  token-usage-insights-cli import --agent <name> --file <path> [--date YYYY-MM-DD]

參數:
  --agent <name>      助理名稱（antigravity/copilot/codex/claude/cursor）
  --file <path>       匯入檔案
  --date YYYY-MM-DD    覆蓋匯入日期，不指定則使用檔案中的 date
  --help, -h          顯示此說明
"#
    );
}

fn has_help(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "--help" || arg == "-h")
}
