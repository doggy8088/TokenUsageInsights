use crate::db;
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const EXPORT_VERSION: u8 = 1;
const HELP_TEXT: &str = r#"Token 戰情室：看板、使用量匯入 / 匯出與自我更新

用法:
  token-usage-insights [子命令] [參數]
  不帶參數時啟動看板；HOST 預設 0.0.0.0，PORT 預設 3003。
  INSIGHTS_DIR 可指定資料庫目錄。
  --help, -h         顯示此說明
  --version, -V      顯示版本資訊
  --no-auto-update   啟動看板時略過自動更新檢查

用途:
  update      更新 Token 戰情室至最新版本（亦可使用 --update 或 -u）
  export      匯出指定日、月或年的資料為 JSON（可重複匯入且支援重複資料去重）
  export-all  一次匯出資料庫中所有 Agent、所有日期的使用量記錄
  import      匯入 JSON 檔內的所有資料（每筆資料依 timestamp 決定日期）

更新:
  token-usage-insights update [參數]
  token-usage-insights --update [參數]
  token-usage-insights -u [參數]
  例如:
  token-usage-insights update
  token-usage-insights update --check
  token-usage-insights update --force
  token-usage-insights update --target-version v0.9.6

參數:
  -c, --check                 僅檢查是否有新版本，不進行下載與安裝
  -f, --force                 強制重新下載並覆蓋現有安裝（即使已是最新版本）
  -v, --target-version <TAG>  指定安裝特定版本標籤（例如 v0.9.6）

共用參數:
  --agent <name>      助理名稱: antigravity / copilot / codex / claude / cursor / grok / pi / omp / muse
                     亦可使用 claude-code / claude_code / claudecode（會正規化為 claude）

匯出:
  token-usage-insights export --agent <name> --date YYYY[-MM[-DD]] --out <path>
  例如:
  token-usage-insights export --agent codex --date 2026-07-09 --out daily.json
  token-usage-insights export-all --out all-usage.json

匯入:
  token-usage-insights import --file <path> [--agent <name>]
  例如:
  token-usage-insights import --file all-usage.json

注意:
  - 若未指定 export 的 --out，會直接輸出到 stdout
  - import 自動依檔案 assistant 判斷 Agent，完整匯出檔會匯入全部 Agent
  - --agent 僅供篩選完整匯出檔或指定舊檔 Agent；單一 Agent 檔案必須一致
  - import 會以 `assistant_type + import_source_id` 做資料去重，重複匯入只會插入一次
  - 每次 import 都會建立可追蹤、可由看板撤銷的匯入批次
"#;

#[derive(Serialize, Deserialize)]
struct UsageDayExportPayload {
    version: u8,
    assistant: String,
    date: String,
    exported_at: String,
    records: Vec<db::UsageDayExportRecord>,
}

#[derive(Serialize, Deserialize)]
struct UsageAllExportPayload {
    version: u8,
    exported_at: String,
    exports: Vec<UsageDayExportPayload>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum UsageImportFile {
    All(UsageAllExportPayload),
    Single(UsageDayImportPayload),
}

impl UsageImportFile {
    fn into_imports(self, target: Option<&str>) -> Result<Vec<UsageDayImportPayload>, String> {
        if let Some(target) = target {
            let payload = self.for_assistant(target);
            validate_import_source_assistant(target, payload.assistant.as_deref())?;
            if !is_supported_assistant(target) || payload.records.is_empty() {
                return Err("不支援的 Agent 或檔案沒有對應記錄".to_string());
            }
            return Ok(vec![UsageDayImportPayload {
                assistant: Some(target.to_string()),
                ..payload
            }]);
        }
        let payloads = match self {
            Self::Single(payload) => vec![payload],
            Self::All(payload) => payload
                .exports
                .into_iter()
                .map(|group| UsageDayImportPayload {
                    version: Some(group.version),
                    assistant: Some(group.assistant),
                    date: Some(group.date),
                    exported_at: Some(group.exported_at),
                    records: group.records,
                })
                .collect(),
        };
        let mut imports = std::collections::BTreeMap::<String, UsageDayImportPayload>::new();
        for mut payload in payloads {
            let assistant = payload
                .assistant
                .as_deref()
                .map(normalize_assistant_name)
                .filter(|name| !name.is_empty())
                .ok_or("檔案缺少 assistant，無法判斷 Agent；舊版檔案請指定 --agent")?;
            if !is_supported_assistant(&assistant) {
                return Err(format!("不支援的助理類型: {assistant}"));
            }
            payload.assistant = Some(assistant.clone());
            if let Some(existing) = imports.get_mut(&assistant) {
                existing.date = Some("all".to_string());
                existing.records.extend(payload.records);
            } else {
                imports.insert(assistant, payload);
            }
        }
        let imports: Vec<_> = imports
            .into_values()
            .filter(|payload| !payload.records.is_empty())
            .collect();
        if imports.is_empty() {
            return Err("匯入檔案沒有 records".to_string());
        }
        Ok(imports)
    }

    fn for_assistant(self, assistant: &str) -> UsageDayImportPayload {
        match self {
            Self::Single(payload) => payload,
            Self::All(payload) => UsageDayImportPayload {
                version: Some(payload.version),
                assistant: Some(assistant.to_string()),
                date: Some("all".to_string()),
                exported_at: Some(payload.exported_at),
                records: payload
                    .exports
                    .into_iter()
                    .filter(|group| normalize_assistant_name(&group.assistant) == assistant)
                    .flat_map(|group| group.records)
                    .collect(),
            },
        }
    }
}

#[derive(Deserialize)]
struct UsageDayImportPayload {
    // Kept for schema parity with the exported JSON; not read during import
    // (import always re-derives these from the current run, not the file).
    #[allow(dead_code)]
    #[serde(default)]
    version: Option<u8>,
    #[serde(default)]
    assistant: Option<String>,
    #[serde(default)]
    date: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    exported_at: Option<String>,
    #[serde(default)]
    records: Vec<db::UsageDayExportRecord>,
}

// None means start the dashboard; commands finish before server initialization.
pub(crate) async fn run(args: &[String]) -> Option<i32> {
    if args.is_empty() {
        return None;
    }

    // 解析並消耗位於子命令前的全域旗標（如 --no-auto-update），或直到遇到 `--` end-of-options。
    // 任何在子命令或 `--` 之後出現的 token 均原樣保留交由對應子命令解析，避免誤傷合法參數值。
    let mut idx = 1;
    while idx < args.len() {
        let arg = &args[idx];
        if arg == "--" {
            idx += 1;
            break;
        }
        if arg == "--no-auto-update" {
            idx += 1;
            continue;
        }
        break;
    }

    let subcmd_args = &args[idx..];
    if subcmd_args.is_empty() {
        return None;
    }

    Some(match subcmd_args[0].as_str() {
        "export" => run_export(&subcmd_args[1..]),
        "export-all" => run_export_all(&subcmd_args[1..]),
        "import" => run_import(&subcmd_args[1..]),
        "update" | "--update" | "-u" => run_update_cli(&subcmd_args[1..]).await,
        "-V" | "--version" | "version" => {
            println!("token-usage-insights {}", env!("CARGO_PKG_VERSION"));
            0
        }
        "-h" | "--help" | "help" => {
            print_help();
            0
        }
        _ => {
            eprintln!("未知指令：{}", subcmd_args[0]);
            print_help();
            2
        }
    })
}

fn collect_all_exports(conn: &rusqlite::Connection) -> Result<UsageAllExportPayload, String> {
    // A read transaction keeps the group list and records in the same snapshot.
    let tx = conn
        .unchecked_transaction()
        .map_err(|err| err.to_string())?;
    let mut stmt = tx
        .prepare(
            "SELECT DISTINCT assistant_type, date FROM usage_entries ORDER BY assistant_type, date",
        )
        .map_err(|err| err.to_string())?;
    let groups = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|err| err.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| err.to_string())?;
    drop(stmt);
    let exported_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let mut exports = Vec::with_capacity(groups.len());
    for (assistant, date) in groups {
        let records = db::export_usage_day_entries(&tx, &assistant, &date)?;
        exports.push(UsageDayExportPayload {
            version: EXPORT_VERSION,
            assistant,
            date,
            exported_at: exported_at.clone(),
            records,
        });
    }
    tx.commit().map_err(|err| err.to_string())?;
    Ok(UsageAllExportPayload {
        version: EXPORT_VERSION,
        exported_at,
        exports,
    })
}

fn run_export_all(args: &[String]) -> i32 {
    if has_help(args) {
        println!("export-all usage:\n  token-usage-insights export-all [--out <path>]\n\n匯出資料庫已收錄的所有 Agent、所有日期與完整使用量欄位。\n--out <path>  輸出 JSON 檔案；省略時輸出到 stdout。\n不接受 --agent 或 --date 篩選；不會掃描尚未同步的來源日誌。");
        return 0;
    }
    let mut out_path = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--out" => out_path = Some(next_flag_value(args, &mut i, "out")),
            arg => {
                eprintln!("未知參數: {arg}");
                return 2;
            }
        }
        i += 1;
    }
    let result = (|| -> Result<(), String> {
        let conn = db::get_db_conn()?;
        db::init_db(&conn)?;
        let payload = collect_all_exports(&conn)?;
        let count: usize = payload
            .exports
            .iter()
            .map(|group| group.records.len())
            .sum();
        let json = serde_json::to_string_pretty(&payload).map_err(|err| err.to_string())?;
        if let Some(out) = out_path {
            fs::write(&out, json).map_err(|err| format!("寫入檔案失敗 {out}: {err}"))?;
            println!("已匯出 {count} 筆到 {out}");
        } else {
            println!("{json}");
        }
        Ok(())
    })();
    match result {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("匯出全部資料失敗: {err}");
            1
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

    if !is_valid_period(&date) {
        eprintln!("資料範圍格式不正確，請使用 YYYY、YYYY-MM 或 YYYY-MM-DD");
        return 2;
    }

    let conn = match db::get_db_conn() {
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

    let records = match db::export_usage_period_entries(&conn, &assistant, &date) {
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

    let payload = match serde_json::from_str::<UsageImportFile>(&input) {
        Ok(v) => v,
        Err(err) => {
            eprintln!("解析 JSON 失敗: {err}");
            return 1;
        }
    };

    let assistant = assistant.as_deref().map(normalize_assistant_name);
    let imports = match payload.into_imports(assistant.as_deref()) {
        Ok(imports) => imports,
        Err(err) => {
            eprintln!("{err}");
            return 2;
        }
    };

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

    let mut summaries = Vec::new();
    for payload in imports {
        let assistant = payload
            .assistant
            .as_deref()
            .expect("validated import assistant");
        let imported_from = date
            .clone()
            .or(payload.date)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "all".to_string());
        let summary = match db::import_usage_day_entries(
            &mut conn,
            assistant,
            &imported_from,
            payload.records,
            db::UsageImportMetadata {
                source_assistant: Some(assistant.to_string()),
                source_file_name: file_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_string),
            },
        ) {
            Ok(v) => v,
            Err(err) => {
                eprintln!(
                    "匯入 {assistant} 失敗: {err}；先前完成的 Agent 已保留，可重新執行並自動去重"
                );
                return 1;
            }
        };
        summaries.push(serde_json::json!({"assistant": assistant, "summary": summary}));
    }

    match serde_json::to_string_pretty(&summaries) {
        Ok(out) => println!("{out}"),
        Err(err) => {
            eprintln!("輸出匯入結果失敗: {err}");
            return 1;
        }
    }

    0
}

fn print_update_help() {
    println!(
        r#"update usage:
  token-usage-insights update [參數]

參數:
  -c, --check                 僅檢查是否有新版本，不進行下載與安裝
  -f, --force                 強制重新下載並覆蓋現有安裝（即使已是最新版本）
  -v, --target-version <TAG>  指定安裝特定版本標籤（例如 v0.9.6）
  -h, --help                  顯示此說明
"#
    );
}

async fn run_update_cli(args: &[String]) -> i32 {
    if has_help(args) {
        print_update_help();
        return 0;
    }

    let mut check_only = false;
    let mut force = false;
    let mut target_version = None;

    let mut i = 0usize;
    while i < args.len() {
        match args[i].as_str() {
            "-c" | "--check" => {
                check_only = true;
            }
            "-f" | "--force" => {
                force = true;
            }
            "-v" | "--target-version" => {
                let val = next_update_flag_value(args, &mut i, "target-version");
                target_version = Some(val);
            }
            arg => {
                eprintln!("未知參數: {arg}");
                print_update_help();
                return 2;
            }
        }
        i += 1;
    }

    let opts = crate::updater::UpdateOptions {
        check_only,
        force,
        target_version,
        prefetched_release: None,
    };

    match crate::updater::run_update(opts).await {
        Ok(_) => 0,
        Err(crate::updater::UpdateError::SafeRejection(_)) => 2,
        Err(crate::updater::UpdateError::Failure(err)) => {
            eprintln!("❌ 更新失敗：{err}");
            1
        }
        Err(crate::updater::UpdateError::RollbackFailed(err)) => {
            eprintln!("❌ 更新失敗且自動回滾失敗：{err}");
            1
        }
    }
}

fn is_option_token(val: &str) -> bool {
    // 一般 CLI 旗標值解析器僅將以 '--' 開頭之長選項視為旗標（如 --agent, --out），
    // 允許任意以 '-' 開頭之合法檔名（如 -f、-report.json、-）作為參數值
    val.starts_with("--") && val.len() > 2
}

fn is_update_option_token(val: &str) -> bool {
    // update 子命令專屬選項判斷：拒絕已知 update 選項作為 --target-version 的值
    matches!(
        val,
        "-c" | "--check" | "-f" | "--force" | "-v" | "--target-version" | "-h" | "--help"
    ) || (val.starts_with("--") && val.len() > 2)
}

fn parse_flag_value(args: &[String], i: &mut usize, flag: &str) -> Result<String, String> {
    match args.get(*i + 1) {
        Some(value) => {
            if is_option_token(value) {
                return Err(format!("缺少 --{flag} 的值"));
            }
            *i += 1;
            Ok(value.clone())
        }
        None => Err(format!("缺少 --{flag} 的值")),
    }
}

fn next_flag_value(args: &[String], i: &mut usize, flag: &str) -> String {
    match parse_flag_value(args, i, flag) {
        Ok(val) => val,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(2);
        }
    }
}

fn parse_update_flag_value(args: &[String], i: &mut usize, flag: &str) -> Result<String, String> {
    match args.get(*i + 1) {
        Some(value) => {
            if is_update_option_token(value) {
                return Err(format!("缺少 --{flag} 的值"));
            }
            *i += 1;
            Ok(value.clone())
        }
        None => Err(format!("缺少 --{flag} 的值")),
    }
}

fn next_update_flag_value(args: &[String], i: &mut usize, flag: &str) -> String {
    match parse_update_flag_value(args, i, flag) {
        Ok(val) => val,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(2);
        }
    }
}

fn normalize_assistant_name(assistant: &str) -> String {
    let normalized = assistant.trim().to_lowercase();
    match normalized.as_str() {
        "claude-code" | "claude_code" | "claudecode" => "claude".to_string(),
        "cursor" => "cursor".to_string(),
        "grok-build" | "grok_build" | "grokbuild" => "grok".to_string(),
        "pi-coding-agent" | "pi_coding_agent" | "picodingagent" => "pi".to_string(),
        "oh-my-pi" | "oh_my_pi" | "ohmypi" => "omp".to_string(),
        "muse" | "muse-code" | "muse_code" | "musecode" | "code-muse" | "code_muse" => {
            "muse".to_string()
        }
        _ => normalized,
    }
}

fn validate_import_source_assistant(
    target_assistant: &str,
    payload_assistant: Option<&str>,
) -> Result<Option<String>, String> {
    let Some(payload_assistant) = payload_assistant else {
        return Ok(None);
    };
    let payload_assistant = normalize_assistant_name(payload_assistant);
    if payload_assistant != target_assistant {
        return Err(format!(
            "匯入已取消：檔案內 assistant={payload_assistant}，但 --agent 指定為 {target_assistant}。"
        ));
    }
    Ok(Some(payload_assistant))
}

fn is_supported_assistant(assistant: &str) -> bool {
    matches!(
        normalize_assistant_name(assistant).as_str(),
        "antigravity" | "copilot" | "codex" | "claude" | "cursor" | "grok" | "pi" | "omp" | "muse"
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

fn is_valid_period(period: &str) -> bool {
    match period.len() {
        4 => period.parse::<i32>().is_ok_and(|year| year > 0),
        7 => {
            let Some((year, month)) = period.split_once('-') else {
                return false;
            };
            year.parse::<i32>().is_ok_and(|year| year > 0)
                && month
                    .parse::<i32>()
                    .is_ok_and(|month| (1..=12).contains(&month))
        }
        10 => is_valid_date(period),
        _ => false,
    }
}

fn print_help() {
    println!("{HELP_TEXT}");
}

fn print_export_help() {
    println!(
        r#"export usage:
  token-usage-insights export --agent <name> --date YYYY[-MM[-DD]] --out <path>

參數:
  --agent <name>    助理名稱（antigravity/copilot/codex/claude/cursor/grok/pi/omp/muse）
  --date <period>     匯出年份、月份或日期
  --out <path>      輸出檔案路徑，不指定則輸出到 stdout
  --help, -h        顯示此說明
"#
    );
}

fn print_import_help() {
    println!(
        r#"import usage:
  token-usage-insights import --file <path> [--agent <name>]

參數:
  --agent <name>      選填：篩選 Agent 或指定缺少 assistant 的舊檔案
  --file <path>       匯入檔案
  --date <label>       相容舊版，僅作為匯入紀錄標籤，不影響資料日期
  --help, -h          顯示此說明

預設依檔案 assistant 自動判斷；完整匯出檔一次匯入所有 Agent。
各 Agent 分別建立匯入批次；中途失敗時，已完成的批次會保留，重試會自動去重。
"#
    );
}

fn has_help(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "--help" || arg == "-h")
}

#[cfg(test)]
mod tests {
    use super::validate_import_source_assistant;

    #[test]
    fn export_all_preserves_every_agent_date_and_import_identity() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        super::db::init_db(&conn).unwrap();
        for agent in [
            "antigravity",
            "copilot",
            "codex",
            "claude",
            "cursor",
            "grok",
            "pi",
            "omp",
            "muse",
            "future-agent",
        ] {
            for date in ["2020-01-01", "2026-09-09"] {
                conn.execute(
                    "INSERT INTO usage_entries (assistant_type, date, timestamp, session_id, turn_no, tokens_input, tokens_output, tokens_total, reasoning_effort, import_source_id) VALUES (?1, ?2, ?3, ?4, 1, 11, 22, 33, 'high', ?4)",
                    rusqlite::params![agent, date, format!("{date}T12:00:00Z"), format!("{agent}-{date}")],
                ).unwrap();
            }
        }
        let all = super::collect_all_exports(&conn).unwrap();
        assert_eq!(all.exports.len(), 20);
        for group in &all.exports {
            let expected =
                super::db::export_usage_day_entries(&conn, &group.assistant, &group.date).unwrap();
            assert_eq!(
                serde_json::to_value(&group.records).unwrap(),
                serde_json::to_value(expected).unwrap()
            );
        }
        let json = serde_json::to_string(&all).unwrap();
        let selected = serde_json::from_str::<super::UsageImportFile>(&json)
            .unwrap()
            .for_assistant("codex");
        assert_eq!(selected.records.len(), 2);
        assert!(selected
            .records
            .iter()
            .all(|record| record.entry.session_id.starts_with("codex-")));
        let mut target = rusqlite::Connection::open_in_memory().unwrap();
        super::db::init_db(&target).unwrap();
        for expected in [2, 0] {
            let selected = serde_json::from_str::<super::UsageImportFile>(&json)
                .unwrap()
                .for_assistant("codex");
            let summary = super::db::import_usage_day_entries(
                &mut target,
                "codex",
                "all",
                selected.records,
                super::db::UsageImportMetadata::default(),
            )
            .unwrap();
            assert_eq!(summary.imported, expected);
        }
    }

    #[test]
    fn export_all_empty_database_and_legacy_import_are_supported() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        super::db::init_db(&conn).unwrap();
        assert!(super::collect_all_exports(&conn)
            .unwrap()
            .exports
            .is_empty());
        let legacy = serde_json::from_str::<super::UsageImportFile>(
            r#"{"assistant":"claude","records":[]}"#,
        )
        .unwrap()
        .for_assistant("codex");
        assert!(validate_import_source_assistant("codex", legacy.assistant.as_deref()).is_err());
    }

    #[test]
    fn import_infers_and_groups_agents_and_validates_before_writing() {
        let record = serde_json::json!({"timestamp":"2026-09-09T00:00:00Z", "session_id":"test", "turn_no":1});
        let group = |agent: &str| serde_json::json!({"version":1, "exported_at":"now", "assistant":agent, "date":"2026-09-09", "records":[record.clone()]});
        let file = serde_json::json!({"version":1,"exported_at":"now","exports":[group("codex"),group("claude-code"),group("codex")]});
        let imports = serde_json::from_value::<super::UsageImportFile>(file)
            .unwrap()
            .into_imports(None)
            .unwrap();
        assert_eq!(imports.len(), 2);
        assert_eq!(imports[0].assistant.as_deref(), Some("claude"));
        assert_eq!(imports[1].records.len(), 2);
        let single = serde_json::json!({"assistant":"codex", "records":[record.clone()]});
        assert_eq!(
            serde_json::from_value::<super::UsageImportFile>(single)
                .unwrap()
                .into_imports(None)
                .unwrap()[0]
                .assistant
                .as_deref(),
            Some("codex")
        );
        let legacy = serde_json::json!({"records":[record]});
        assert!(
            serde_json::from_value::<super::UsageImportFile>(legacy.clone())
                .unwrap()
                .into_imports(None)
                .is_err()
        );
        assert!(serde_json::from_value::<super::UsageImportFile>(legacy)
            .unwrap()
            .into_imports(Some("codex"))
            .is_ok());
        let invalid = serde_json::json!({"version":1,"exported_at":"now","exports":[group("codex"),group("unknown")]});
        assert!(serde_json::from_value::<super::UsageImportFile>(invalid)
            .unwrap()
            .into_imports(None)
            .is_err());
    }

    #[test]
    fn import_source_assistant_must_match_cli_target() {
        let error = validate_import_source_assistant("antigravity", Some("codex")).unwrap_err();
        assert!(error.contains("匯入已取消"));
        assert!(error.contains("assistant=codex"));
        assert!(error.contains("--agent 指定為 antigravity"));
    }

    #[test]
    fn import_source_assistant_accepts_alias_and_legacy_file() {
        assert_eq!(
            validate_import_source_assistant("claude", Some("claude-code")).unwrap(),
            Some("claude".to_string())
        );
        assert_eq!(
            validate_import_source_assistant("codex", None).unwrap(),
            None
        );
    }

    #[test]
    fn parse_flag_value_rejects_missing_and_option_like_values() {
        // 1. update 子命令旗標解析測試（驗證指向旗標位置 index 1 時，對後續參數值之正確解析與拒絕）
        let mut i1 = 1;
        let args_short = vec!["update".to_string(), "-v".to_string(), "-f".to_string()];
        let err_short =
            super::parse_update_flag_value(&args_short, &mut i1, "target-version").unwrap_err();
        assert_eq!(err_short, "缺少 --target-version 的值");
        assert_eq!(i1, 1);

        let mut i2 = 1;
        let args_long = vec![
            "update".to_string(),
            "-v".to_string(),
            "--force".to_string(),
        ];
        let err_long =
            super::parse_update_flag_value(&args_long, &mut i2, "target-version").unwrap_err();
        assert_eq!(err_long, "缺少 --target-version 的值");
        assert_eq!(i2, 1);

        let mut i3 = 1;
        let args_end = vec!["update".to_string(), "-v".to_string()];
        let err_end =
            super::parse_update_flag_value(&args_end, &mut i3, "target-version").unwrap_err();
        assert_eq!(err_end, "缺少 --target-version 的值");
        assert_eq!(i3, 1);

        let mut j = 1;
        let args_valid = vec!["update".to_string(), "-v".to_string(), "v0.9.6".to_string()];
        let val = super::parse_update_flag_value(&args_valid, &mut j, "target-version").unwrap();
        assert_eq!(val, "v0.9.6");
        assert_eq!(j, 2);

        // 2. 一般命令（如 export/import）旗標解析測試：
        // 驗證以 - 開頭之檔名（如 -f、-report.json）與單一 dash (-) 均為合法路徑值，不得誤判為缺少值
        let mut f_idx = 1;
        let args_f = vec!["export".to_string(), "--out".to_string(), "-f".to_string()];
        let val_f = super::parse_flag_value(&args_f, &mut f_idx, "out").unwrap();
        assert_eq!(val_f, "-f");
        assert_eq!(f_idx, 2);

        let mut k = 1;
        let args_dash_file = vec![
            "export".to_string(),
            "--out".to_string(),
            "-report.json".to_string(),
        ];
        let val_file = super::parse_flag_value(&args_dash_file, &mut k, "out").unwrap();
        assert_eq!(val_file, "-report.json");
        assert_eq!(k, 2);

        let mut m = 1;
        let args_single_dash = vec!["export".to_string(), "--out".to_string(), "-".to_string()];
        let val_dash = super::parse_flag_value(&args_single_dash, &mut m, "out").unwrap();
        assert_eq!(val_dash, "-");
        assert_eq!(m, 2);

        // 驗證一般命令遇到 -- 開頭之其他旗標時仍會正確拒絕
        let mut err_idx = 1;
        let args_missing_out = vec![
            "export".to_string(),
            "--out".to_string(),
            "--agent".to_string(),
            "claude".to_string(),
        ];
        let err_missing =
            super::parse_flag_value(&args_missing_out, &mut err_idx, "out").unwrap_err();
        assert_eq!(err_missing, "缺少 --out 的值");
        assert_eq!(err_idx, 1);
    }

    #[tokio::test]
    async fn cli_run_normalizes_no_auto_update_combinations() {
        // 單獨使用 --no-auto-update 應啟動服務器（回傳 None）
        let single = vec![
            "token-usage-insights".to_string(),
            "--no-auto-update".to_string(),
        ];
        assert_eq!(super::run(&single).await, None);

        // 結合 --help 應正常印出說明並以 0 結束
        let with_help = vec![
            "token-usage-insights".to_string(),
            "--no-auto-update".to_string(),
            "--help".to_string(),
        ];
        assert_eq!(super::run(&with_help).await, Some(0));

        // 結合不存在之子命令應以 2 結束
        let with_invalid = vec![
            "token-usage-insights".to_string(),
            "--no-auto-update".to_string(),
            "nonexistent-cmd".to_string(),
        ];
        assert_eq!(super::run(&with_invalid).await, Some(2));
    }

    #[tokio::test]
    async fn cli_run_version_flag_returns_zero() {
        let version_long = vec!["token-usage-insights".to_string(), "--version".to_string()];
        assert_eq!(super::run(&version_long).await, Some(0));

        let version_short = vec!["token-usage-insights".to_string(), "-V".to_string()];
        assert_eq!(super::run(&version_short).await, Some(0));

        let version_cmd = vec!["token-usage-insights".to_string(), "version".to_string()];
        assert_eq!(super::run(&version_cmd).await, Some(0));
    }

    #[tokio::test]
    async fn cli_run_handles_end_of_options_and_subcommand_arguments() {
        // 在 -- 之後的 --no-auto-update 應被視為子命令而非全域旗標
        let after_delimiter = vec![
            "token-usage-insights".to_string(),
            "--".to_string(),
            "--no-auto-update".to_string(),
        ];
        assert_eq!(super::run(&after_delimiter).await, Some(2));

        // 單獨 -- 應啟動看板（回傳 None）
        let delimiter_only = vec!["token-usage-insights".to_string(), "--".to_string()];
        assert_eq!(super::run(&delimiter_only).await, None);

        // 空參數應啟動看板
        assert_eq!(super::run(&[]).await, None);

        // 全域旗標在子命令前被消耗，子命令與參數保持完整傳遞
        let update_with_flag = vec![
            "token-usage-insights".to_string(),
            "--no-auto-update".to_string(),
            "update".to_string(),
            "--help".to_string(),
        ];
        assert_eq!(super::run(&update_with_flag).await, Some(0));

        // 未知子命令在全域旗標之後正確被辨識
        let invalid_after_flag = vec![
            "token-usage-insights".to_string(),
            "--no-auto-update".to_string(),
            "unknown-cmd".to_string(),
            "--no-auto-update".to_string(),
        ];
        assert_eq!(super::run(&invalid_after_flag).await, Some(2));
    }
}
