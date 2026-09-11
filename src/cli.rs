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
    if args.len() < 2 {
        return None;
    }

    if args.len() == 2 && args[1] == "--no-auto-update" {
        return None;
    }

    Some(match args[1].as_str() {
        "export" => run_export(&args[2..]),
        "export-all" => run_export_all(&args[2..]),
        "import" => run_import(&args[2..]),
        "update" | "--update" | "-u" => run_update_cli(&args[2..]).await,
        "-h" | "--help" | "help" => {
            print_help();
            0
        }
        _ => {
            eprintln!("未知指令：{}", args[1]);
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
                let val = next_flag_value(args, &mut i, "target-version");
                if val.starts_with('-') {
                    eprintln!("缺少 --target-version 的值");
                    return 2;
                }
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
        Ok(()) => 0,
        Err(crate::updater::UpdateError::SafeRejection(_)) => 2,
        Err(crate::updater::UpdateError::Failure(err)) => {
            eprintln!("❌ 更新失敗：{err}");
            1
        }
    }
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
}
