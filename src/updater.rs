use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::Utc;
use serde::Deserialize;
use sha2::{Digest, Sha256};

const GITHUB_OWNER: &str = "doggy8088";
const GITHUB_REPO: &str = "TokenUsageInsights";
const APP_NAME: &str = "token-usage-insights";
const USER_AGENT: &str = "token-usage-insights-updater";
const DEFAULT_UPDATE_INTERVAL_HOURS: i64 = 24;
const STARTUP_CHECK_TIMEOUT_SECS: u64 = 4;
const STARTUP_AUTO_UPDATE_TOTAL_TIMEOUT_SECS: u64 = 60;
const LAST_CHECK_KEY: &str = "last_update_check_at";
const MAX_ARCHIVE_BYTES: usize = 150 * 1024 * 1024; // 150 MB 上限
const MAX_CHECKSUM_BYTES: usize = 1024 * 1024; // 1 MB 上限
const MAX_EXTRACTED_BYTES: u64 = 300 * 1024 * 1024; // 300 MB 解壓縮展開上限
const MAX_EXTRACTED_ENTRIES: usize = 10_000; // 最多 10,000 個檔案/目錄

#[cfg(unix)]
fn is_process_alive(pid: u32) -> bool {
    let res = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if res == 0 {
        true
    } else {
        let err = std::io::Error::last_os_error().raw_os_error();
        err != Some(libc::ESRCH)
    }
}

#[cfg(windows)]
fn is_process_alive(pid: u32) -> bool {
    let output = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output();
    if let Ok(out) = output {
        let text = String::from_utf8_lossy(&out.stdout);
        let pid_token = format!("\"{pid}\"");
        text.lines().any(|line| line.contains(&pid_token))
    } else {
        true
    }
}

#[cfg(not(any(unix, windows)))]
fn is_process_alive(_pid: u32) -> bool {
    false
}

#[cfg(target_vendor = "apple")]
fn get_process_exe_path(pid: u32) -> Option<PathBuf> {
    #[link(name = "proc")]
    extern "C" {
        fn proc_pidpath(
            pid: libc::c_int,
            buffer: *mut libc::c_void,
            buffersize: u32,
        ) -> libc::c_int;
    }
    let mut buf = vec![0u8; 4096];
    let ret = unsafe {
        proc_pidpath(
            pid as libc::c_int,
            buf.as_mut_ptr() as *mut libc::c_void,
            buf.len() as u32,
        )
    };
    if ret > 0 {
        let path_bytes = &buf[..ret as usize];
        if let Ok(path_str) = std::str::from_utf8(path_bytes) {
            let p = PathBuf::from(path_str.trim_end_matches('\0'));
            if p.exists() {
                return Some(p);
            }
        }
    }
    if let Ok(output) = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
    {
        let comm = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !comm.is_empty() {
            let p = PathBuf::from(&comm);
            if p.is_absolute() && p.exists() {
                return Some(p);
            }
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn get_process_exe_path(pid: u32) -> Option<PathBuf> {
    fs::read_link(format!("/proc/{pid}/exe")).ok()
}

#[cfg(all(unix, not(target_os = "linux"), not(target_vendor = "apple")))]
fn get_process_exe_path(pid: u32) -> Option<PathBuf> {
    if let Ok(p) = fs::read_link(format!("/proc/{pid}/exe")) {
        return Some(p);
    }
    if let Ok(p) = fs::read_link(format!("/proc/{pid}/file")) {
        return Some(p);
    }
    if let Ok(output) = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
    {
        let comm = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !comm.is_empty() {
            let p = PathBuf::from(&comm);
            if p.is_absolute() && p.exists() {
                return Some(p);
            }
        }
    }
    None
}

#[cfg(windows)]
type WinHandle = *mut std::ffi::c_void;

#[cfg(windows)]
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
#[cfg(windows)]
const PROCESS_QUERY_INFORMATION: u32 = 0x0400;
#[cfg(windows)]
const PROCESS_VM_READ: u32 = 0x0010;

#[cfg(windows)]
#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *mut u16,
}

#[cfg(windows)]
#[repr(C)]
struct ProcessBasicInformation {
    exit_status: i32,
    peb_base_address: *mut std::ffi::c_void,
    affinity_mask: usize,
    base_priority: i32,
    unique_process_id: usize,
    inherited_from_unique_process_id: usize,
}

#[cfg(windows)]
type NtQueryInformationProcessFn = unsafe extern "system" fn(
    process_handle: WinHandle,
    process_information_class: u32,
    process_information: *mut std::ffi::c_void,
    process_information_length: u32,
    return_length: *mut u32,
) -> i32;

#[cfg(windows)]
extern "system" {
    fn OpenProcess(dwDesiredAccess: u32, bInheritHandle: i32, dwProcessId: u32) -> WinHandle;
    fn QueryFullProcessImageNameW(
        hProcess: WinHandle,
        dwFlags: u32,
        lpExeName: *mut u16,
        lpdwSize: *mut u32,
    ) -> i32;
    fn CloseHandle(hObject: WinHandle) -> i32;
    fn GetModuleHandleA(lpModuleName: *const u8) -> WinHandle;
    fn GetProcAddress(hModule: WinHandle, lpProcName: *const u8) -> *mut std::ffi::c_void;
    fn LocalFree(hMem: WinHandle) -> WinHandle;
    fn ReadProcessMemory(
        hProcess: WinHandle,
        lpBaseAddress: *const std::ffi::c_void,
        lpBuffer: *mut std::ffi::c_void,
        nSize: usize,
        lpNumberOfBytesRead: *mut usize,
    ) -> i32;
}

#[cfg(windows)]
#[link(name = "shell32")]
extern "system" {
    fn CommandLineToArgvW(lpCmdLine: *const u16, pNumArgs: *mut i32) -> *mut *mut u16;
}

#[cfg(windows)]
fn get_process_exe_path(pid: u32) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut buf = vec![0u16; 1024];
        let mut size = buf.len() as u32;
        let success = QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut size);
        CloseHandle(handle);

        if success != 0 && size > 0 {
            let os_str = std::ffi::OsString::from_wide(&buf[..size as usize]);
            let p = PathBuf::from(os_str);
            if p.exists() {
                return Some(p);
            }
        }
        None
    }
}

#[cfg(not(any(unix, windows)))]
fn get_process_exe_path(_pid: u32) -> Option<PathBuf> {
    None
}

fn matches_install_dir(exe_path: &Path, install_dir: &Path) -> bool {
    if let (Ok(can_exe), Ok(can_dir)) = (fs::canonicalize(exe_path), fs::canonicalize(install_dir))
    {
        can_exe.starts_with(&can_dir)
    } else {
        exe_path.starts_with(install_dir)
    }
}

#[cfg(target_os = "linux")]
fn get_process_cmdline(pid: u32) -> Option<Vec<String>> {
    let content = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let args: Vec<String> = content
        .split(|&b| b == 0)
        .filter(|slice| !slice.is_empty())
        .map(|slice| String::from_utf8_lossy(slice).to_string())
        .collect();
    if args.is_empty() {
        None
    } else {
        Some(args)
    }
}

#[cfg(target_os = "macos")]
fn get_process_cmdline(pid: u32) -> Option<Vec<String>> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as libc::c_int];
    let mut size: libc::size_t = 0;
    unsafe {
        if libc::sysctl(
            mib.as_mut_ptr(),
            3,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        ) != 0
            || size <= std::mem::size_of::<libc::c_int>()
        {
            return None;
        }

        let mut buf = vec![0u8; size];
        if libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buf.as_mut_ptr() as *mut libc::c_void,
            &mut size,
            std::ptr::null_mut(),
            0,
        ) != 0
        {
            return None;
        }

        let argc = *(buf.as_ptr() as *const libc::c_int);
        if argc <= 0 {
            return None;
        }

        let mut idx = std::mem::size_of::<libc::c_int>();
        // Skip the exec_path (null-terminated string)
        while idx < size && buf[idx] != 0 {
            idx += 1;
        }
        // Skip null padding between exec_path and argv[0]
        while idx < size && buf[idx] == 0 {
            idx += 1;
        }

        let mut args = Vec::new();
        for _ in 0..argc {
            if idx >= size {
                break;
            }
            let start = idx;
            while idx < size && buf[idx] != 0 {
                idx += 1;
            }
            let slice = &buf[start..idx];
            args.push(String::from_utf8_lossy(slice).to_string());
            idx += 1; // skip null delimiter
        }

        if args.is_empty() {
            None
        } else {
            Some(args)
        }
    }
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn get_process_cmdline(pid: u32) -> Option<Vec<String>> {
    let output = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        return None;
    }
    Some(text.split_whitespace().map(|s| s.to_string()).collect())
}

#[cfg(windows)]
fn get_process_cmdline(pid: u32) -> Option<Vec<String>> {
    use std::os::windows::ffi::OsStringExt;

    unsafe {
        let ntdll = GetModuleHandleA(b"ntdll.dll\0".as_ptr());
        if ntdll.is_null() {
            return None;
        }
        let func_ptr = GetProcAddress(ntdll, b"NtQueryInformationProcess\0".as_ptr());
        if func_ptr.is_null() {
            return None;
        }
        let nt_query: NtQueryInformationProcessFn = std::mem::transmute(func_ptr);

        let mut handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            handle = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid);
        }
        if handle.is_null() {
            return None;
        }

        // ProcessCommandLineInformation = 60
        let mut buf = vec![0u8; 32768];
        let mut return_len = 0u32;
        let status = nt_query(
            handle,
            60,
            buf.as_mut_ptr() as *mut std::ffi::c_void,
            buf.len() as u32,
            &mut return_len,
        );
        CloseHandle(handle);

        if status != 0 {
            return None;
        }

        let p_unicode = buf.as_ptr() as *const UnicodeString;
        let byte_len = (*p_unicode).length as usize;
        let char_len = byte_len / 2;
        let str_ptr = (*p_unicode).buffer;

        if str_ptr.is_null() || char_len == 0 {
            return None;
        }

        let buf_start = buf.as_ptr() as usize;
        let buf_end = buf_start + buf.len();
        let ptr_val = str_ptr as usize;
        if ptr_val < buf_start || ptr_val + byte_len > buf_end {
            return None;
        }

        let mut wide_chars: Vec<u16> = std::slice::from_raw_parts(str_ptr, char_len).to_vec();
        wide_chars.push(0);

        let mut num_args = 0i32;
        let argv_ptr = CommandLineToArgvW(wide_chars.as_ptr(), &mut num_args);
        if argv_ptr.is_null() || num_args <= 0 {
            return None;
        }

        let mut args = Vec::new();
        for i in 0..num_args as usize {
            let arg_ptr = *argv_ptr.add(i);
            if !arg_ptr.is_null() {
                let mut len = 0;
                while *arg_ptr.add(len) != 0 {
                    len += 1;
                }
                let arg_slice = std::slice::from_raw_parts(arg_ptr, len);
                let os_str = std::ffi::OsString::from_wide(arg_slice);
                args.push(os_str.to_string_lossy().to_string());
            }
        }
        LocalFree(argv_ptr as WinHandle);

        if args.is_empty() {
            None
        } else {
            Some(args)
        }
    }
}

#[cfg(windows)]
fn get_process_ppid(pid: u32) -> Option<u32> {
    unsafe {
        let ntdll = GetModuleHandleA(b"ntdll.dll\0".as_ptr());
        if ntdll.is_null() {
            return None;
        }
        let func_ptr = GetProcAddress(ntdll, b"NtQueryInformationProcess\0".as_ptr());
        if func_ptr.is_null() {
            return None;
        }
        let nt_query: NtQueryInformationProcessFn = std::mem::transmute(func_ptr);

        let mut handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            handle = OpenProcess(PROCESS_QUERY_INFORMATION, 0, pid);
        }
        if handle.is_null() {
            return None;
        }

        let mut pbi = std::mem::zeroed::<ProcessBasicInformation>();
        let mut return_len = 0u32;
        let status = nt_query(
            handle,
            0, // ProcessBasicInformation
            &mut pbi as *mut _ as *mut std::ffi::c_void,
            std::mem::size_of::<ProcessBasicInformation>() as u32,
            &mut return_len,
        );
        CloseHandle(handle);

        if status == 0 && pbi.inherited_from_unique_process_id != 0 {
            Some(pbi.inherited_from_unique_process_id as u32)
        } else {
            None
        }
    }
}

#[cfg(windows)]
fn get_process_supervisor_pid(pid: u32) -> Option<u32> {
    if let Some(ppid) = get_process_ppid(pid) {
        if is_process_alive(ppid) {
            if let Some(parent_exe) = get_process_exe_path(ppid) {
                let file_name = parent_exe
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                if file_name == "powershell.exe" || file_name == "pwsh.exe" {
                    if let Some(cmd) = get_process_cmdline(ppid) {
                        if cmd
                            .iter()
                            .any(|arg| arg.to_lowercase().contains("run-service.ps1"))
                        {
                            return Some(ppid);
                        }
                    }
                }
            }
        }
    }
    None
}

#[cfg(not(windows))]
fn get_process_supervisor_pid(_pid: u32) -> Option<u32> {
    None
}

#[cfg(windows)]
fn is_process_supervised(pid: u32, _install_dir: &Path) -> bool {
    if get_process_supervisor_pid(pid).is_some() {
        return true;
    }
    if let Some(ppid) = get_process_ppid(pid) {
        if is_process_alive(ppid) {
            if let Some(parent_exe) = get_process_exe_path(ppid) {
                let file_name = parent_exe
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                if file_name == "services.exe" {
                    return true;
                }
            }
        }
    }
    false
}

#[cfg(target_os = "linux")]
fn is_process_supervised(pid: u32, _install_dir: &Path) -> bool {
    if let Ok(env_bytes) = fs::read(format!("/proc/{pid}/environ")) {
        let env_str = String::from_utf8_lossy(&env_bytes);
        if env_str.contains("INVOCATION_ID=")
            || env_str.contains("JOURNAL_STREAM=")
            || env_str.contains("SYSTEMD_EXEC_PID=")
        {
            return true;
        }
    }
    if let Ok(cgroup) = fs::read_to_string(format!("/proc/{pid}/cgroup")) {
        if cgroup.contains(".service") || cgroup.contains("system.slice") {
            return true;
        }
    }
    false
}

#[cfg(all(unix, not(target_os = "linux")))]
fn is_process_supervised(pid: u32, _install_dir: &Path) -> bool {
    if let Ok(output) = std::process::Command::new("launchctl").arg("list").output() {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            let pid_str = pid.to_string();
            for line in text.lines() {
                if let Some(first) = line.split_whitespace().next() {
                    if first == pid_str {
                        return true;
                    }
                }
            }
        }
    }
    false
}

#[cfg(not(any(unix, windows)))]
fn get_process_cmdline(_pid: u32) -> Option<Vec<String>> {
    None
}

#[cfg(not(any(unix, windows)))]
fn is_process_supervised(_pid: u32, _install_dir: &Path) -> bool {
    false
}

#[allow(dead_code)] // 供各平台進程檢測與重啟函式比對關鍵環境變數
const RELEVANT_ENV_VARS: &[&str] = &[
    "PORT",
    "HOST",
    "INSIGHTS_DIR",
    "TOKEN_USAGE_INSIGHTS_INSTALL_DIR",
    "TOKEN_USAGE_INSIGHTS_AUTO_UPDATE",
    "TOKEN_USAGE_INSIGHTS_UPDATE_INTERVAL_HOURS",
    "TOKEN_USAGE_INSIGHTS_SERVICE",
    "CORS_ALLOWED_ORIGINS",
    "ANTIGRAVITY_DIR",
    "COPILOT_DIR",
    "COPILOT_APP_DIR",
    "CODEX_DIR",
    "CLAUDE_DIR",
    "CURSOR_DIR",
    "CURSOR_STATE_DB",
    "GROK_DIR",
    "PI_DIR",
    "OMP_DIR",
    "MUSE_DIR",
    "VSCODE_DIR",
    "VSCODE_USER_DATA_DIR",
    "VSCODE_PORTABLE_DATA_DIR",
];

#[cfg(target_os = "linux")]
fn get_process_relevant_envs(pid: u32) -> Option<Vec<(String, String)>> {
    let bytes = fs::read(format!("/proc/{pid}/environ")).ok()?;
    let mut envs = Vec::new();
    for entry in bytes.split(|&b| b == 0) {
        if let Ok(s) = std::str::from_utf8(entry) {
            if let Some((k, v)) = s.split_once('=') {
                if RELEVANT_ENV_VARS.contains(&k) {
                    envs.push((k.to_string(), v.to_string()));
                }
            }
        }
    }
    Some(envs)
}

#[cfg(target_os = "linux")]
fn get_process_cwd(pid: u32) -> Option<PathBuf> {
    fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

#[cfg(all(unix, not(target_os = "linux")))]
fn get_process_relevant_envs(pid: u32) -> Option<Vec<(String, String)>> {
    // macOS 上使用 ps -E 的輸出無法可靠解析包含空白的環境變數值（split_whitespace 會截斷），
    // 改以 /usr/bin/env sysctl kern.procargs2 解析 null-byte 分隔的原始環境字串。
    // 若無法無損讀取，回傳 None 讓呼叫端略過重啟以防路徑偏離。
    let output = std::process::Command::new("sysctl")
        .args(["-b", &format!("kern.procargs2.{pid}")])
        .output()
        .ok()?;
    if !output.status.success() || output.stdout.is_empty() {
        return None;
    }
    // kern.procargs2 格式：argc (4 bytes LE) | exec_path\0 | args... | envs...
    // 所有欄位以 NUL 分隔；我們跳過 argc + exec_path + argv，取 env 區段
    let bytes = &output.stdout;
    if bytes.len() < 4 {
        return None;
    }
    let argc = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    // 跳過 argc 欄位（4 bytes）後，以 NUL 拆分
    let mut parts = bytes[4..].split(|&b| b == 0);
    // 第一個部分是 exec_path，再跳過 argc 個 argv 項目
    let _ = parts.next(); // exec_path
    for _ in 0..argc {
        parts.next(); // argv[i]
    }
    // 其餘為 KEY=VALUE 形式的環境變數
    let mut envs = Vec::new();
    for entry in parts {
        if entry.is_empty() {
            break;
        }
        if let Ok(s) = std::str::from_utf8(entry) {
            if let Some((k, v)) = s.split_once('=') {
                if RELEVANT_ENV_VARS.contains(&k) {
                    envs.push((k.to_string(), v.to_string()));
                }
            }
        }
    }
    Some(envs)
}

#[cfg(all(unix, not(target_os = "linux")))]
fn get_process_cwd(pid: u32) -> Option<PathBuf> {
    if let Ok(output) = std::process::Command::new("lsof")
        .args(["-a", "-d", "cwd", "-p", &pid.to_string(), "-Fn"])
        .output()
    {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines() {
                if let Some(path_str) = line.strip_prefix('n') {
                    let path = PathBuf::from(path_str.trim());
                    if path.is_dir() {
                        return Some(path);
                    }
                }
            }
        }
    }
    None
}

#[cfg(windows)]
fn get_process_relevant_envs(pid: u32) -> Option<Vec<(String, String)>> {
    unsafe {
        let ntdll = GetModuleHandleA(b"ntdll.dll\0".as_ptr());
        if ntdll.is_null() {
            return None;
        }
        let func_ptr = GetProcAddress(ntdll, b"NtQueryInformationProcess\0".as_ptr());
        if func_ptr.is_null() {
            return None;
        }
        let nt_query: NtQueryInformationProcessFn = std::mem::transmute(func_ptr);

        let handle = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid);
        if handle.is_null() {
            return None;
        }

        let mut pbi = std::mem::zeroed::<ProcessBasicInformation>();
        let mut return_len = 0u32;
        let status = nt_query(
            handle,
            0, // ProcessBasicInformation
            &mut pbi as *mut _ as *mut std::ffi::c_void,
            std::mem::size_of::<ProcessBasicInformation>() as u32,
            &mut return_len,
        );

        if status != 0 || pbi.peb_base_address.is_null() {
            CloseHandle(handle);
            return None;
        }

        #[cfg(target_pointer_width = "64")]
        let (proc_params_offset, env_offset) = (0x20usize, 0x80usize);
        #[cfg(target_pointer_width = "32")]
        let (proc_params_offset, env_offset) = (0x10usize, 0x48usize);

        let peb_ptr = pbi.peb_base_address as usize;
        let mut params_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
        let mut bytes_read = 0usize;

        let ok1 = ReadProcessMemory(
            handle,
            (peb_ptr + proc_params_offset) as *const std::ffi::c_void,
            &mut params_ptr as *mut _ as *mut std::ffi::c_void,
            std::mem::size_of::<*mut std::ffi::c_void>(),
            &mut bytes_read,
        );

        if ok1 == 0 || params_ptr.is_null() {
            CloseHandle(handle);
            return None;
        }

        let mut env_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
        let ok2 = ReadProcessMemory(
            handle,
            (params_ptr as usize + env_offset) as *const std::ffi::c_void,
            &mut env_ptr as *mut _ as *mut std::ffi::c_void,
            std::mem::size_of::<*mut std::ffi::c_void>(),
            &mut bytes_read,
        );

        if ok2 == 0 || env_ptr.is_null() {
            CloseHandle(handle);
            return None;
        }

        // 循環逐頁讀取完整以雙重 NUL 結尾的環境變數區塊。
        // 若在緩衝區上限或讀取錯誤前未出現雙重 NUL 結尾，拒絕不完整的捕獲以防重啟時遺漏關鍵環境變數。
        let mut all_u16: Vec<u16> = Vec::new();
        let mut curr_ptr = env_ptr as usize;
        let mut block_terminated = false;
        const MAX_ENV_BYTES: usize = 128 * 1024; // 上限 128 KB，防範異常超大或損壞區塊

        while all_u16.len() * 2 < MAX_ENV_BYTES {
            let bytes_to_page_end = 4096 - (curr_ptr & 0xFFF);
            let read_size = bytes_to_page_end.max(512).min(4096);
            let mut page_buf = vec![0u8; read_size];
            let mut chunk_bytes_read = 0usize;

            let ok = ReadProcessMemory(
                handle,
                curr_ptr as *const std::ffi::c_void,
                page_buf.as_mut_ptr() as *mut std::ffi::c_void,
                read_size,
                &mut chunk_bytes_read,
            );

            if ok == 0 || chunk_bytes_read < 2 {
                break;
            }

            let u16_chunk: &[u16] =
                std::slice::from_raw_parts(page_buf.as_ptr() as *const u16, chunk_bytes_read / 2);
            all_u16.extend_from_slice(u16_chunk);
            curr_ptr += chunk_bytes_read;

            // 檢查是否已出現雙重 NUL 結尾
            let mut start = 0;
            for i in 0..all_u16.len() {
                if all_u16[i] == 0 {
                    if i == start {
                        block_terminated = true;
                        break;
                    }
                    start = i + 1;
                }
            }

            if block_terminated {
                break;
            }

            if chunk_bytes_read < read_size {
                break;
            }
        }
        CloseHandle(handle);

        if !block_terminated {
            return None;
        }

        let mut results = Vec::new();
        let mut start = 0;
        for i in 0..all_u16.len() {
            if all_u16[i] == 0 {
                if i > start {
                    let entry = String::from_utf16_lossy(&all_u16[start..i]);
                    if let Some((k, v)) = entry.split_once('=') {
                        if RELEVANT_ENV_VARS.contains(&k) {
                            results.push((k.to_string(), v.to_string()));
                        }
                    }
                } else {
                    break;
                }
                start = i + 1;
            }
        }
        Some(results)
    }
}

#[cfg(windows)]
fn get_process_cwd(pid: u32) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;

    unsafe {
        let ntdll = GetModuleHandleA(b"ntdll.dll\0".as_ptr());
        if ntdll.is_null() {
            return None;
        }
        let func_ptr = GetProcAddress(ntdll, b"NtQueryInformationProcess\0".as_ptr());
        if func_ptr.is_null() {
            return None;
        }
        let nt_query: NtQueryInformationProcessFn = std::mem::transmute(func_ptr);

        let mut handle = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid);
        if handle.is_null() {
            handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        }
        if handle.is_null() {
            return None;
        }

        let mut pbi = std::mem::zeroed::<ProcessBasicInformation>();
        let mut return_len = 0u32;
        let status = nt_query(
            handle,
            0, // ProcessBasicInformation
            &mut pbi as *mut _ as *mut std::ffi::c_void,
            std::mem::size_of::<ProcessBasicInformation>() as u32,
            &mut return_len,
        );

        if status != 0 || pbi.peb_base_address.is_null() {
            CloseHandle(handle);
            return None;
        }

        #[cfg(target_pointer_width = "64")]
        let (proc_params_offset, curdir_offset) = (0x20usize, 0x38usize);
        #[cfg(target_pointer_width = "32")]
        let (proc_params_offset, curdir_offset) = (0x10usize, 0x24usize);

        let peb_ptr = pbi.peb_base_address as usize;
        let mut params_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
        let mut bytes_read = 0usize;

        let ok1 = ReadProcessMemory(
            handle,
            (peb_ptr + proc_params_offset) as *const std::ffi::c_void,
            &mut params_ptr as *mut _ as *mut std::ffi::c_void,
            std::mem::size_of::<*mut std::ffi::c_void>(),
            &mut bytes_read,
        );

        if ok1 == 0 || params_ptr.is_null() {
            CloseHandle(handle);
            return None;
        }

        let mut unicode_str = UnicodeString {
            length: 0,
            maximum_length: 0,
            buffer: std::ptr::null_mut(),
        };

        let ok2 = ReadProcessMemory(
            handle,
            (params_ptr as usize + curdir_offset) as *const std::ffi::c_void,
            &mut unicode_str as *mut _ as *mut std::ffi::c_void,
            std::mem::size_of::<UnicodeString>(),
            &mut bytes_read,
        );

        if ok2 == 0 || unicode_str.buffer.is_null() || unicode_str.length == 0 {
            CloseHandle(handle);
            return None;
        }

        let char_len = (unicode_str.length as usize) / 2;
        if char_len > 4096 {
            CloseHandle(handle);
            return None;
        }

        let mut wide_buf = vec![0u16; char_len];
        let ok3 = ReadProcessMemory(
            handle,
            unicode_str.buffer as *const std::ffi::c_void,
            wide_buf.as_mut_ptr() as *mut std::ffi::c_void,
            unicode_str.length as usize,
            &mut bytes_read,
        );
        CloseHandle(handle);

        if ok3 == 0 || bytes_read < unicode_str.length as usize {
            return None;
        }

        let os_str = std::ffi::OsString::from_wide(&wide_buf);
        let path = PathBuf::from(os_str.to_string_lossy().trim().to_string());
        if path.is_dir() {
            Some(path)
        } else {
            None
        }
    }
}

#[cfg(not(any(unix, windows)))]
fn get_process_relevant_envs(_pid: u32) -> Option<Vec<(String, String)>> {
    None
}

#[cfg(not(any(unix, windows)))]
fn get_process_cwd(_pid: u32) -> Option<PathBuf> {
    None
}

fn get_process_relevant_envs_with_retry(pid: u32) -> Option<Vec<(String, String)>> {
    for _ in 0..3 {
        if let Some(envs) = get_process_relevant_envs(pid) {
            return Some(envs);
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    None
}

fn is_cli_subcommand(arg: &str) -> bool {
    let clean = arg.trim_matches('"').trim_matches('\'');
    matches!(
        clean,
        "export"
            | "export-all"
            | "import"
            | "update"
            | "--update"
            | "-u"
            | "completion"
            | "-h"
            | "--help"
            | "help"
            | "-V"
            | "--version"
    )
}

fn get_process_cmdline_with_retry(pid: u32) -> Option<Vec<String>> {
    for attempt in 0..3 {
        if let Some(cmd) = get_process_cmdline(pid) {
            return Some(cmd);
        }
        if attempt < 2 {
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
    }
    None
}

fn get_process_cwd_with_retry(pid: u32) -> Option<PathBuf> {
    for attempt in 0..3 {
        if let Some(cwd) = get_process_cwd(pid) {
            return Some(cwd);
        }
        if attempt < 2 {
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
    }
    None
}

fn is_dashboard_server_process(pid: u32, server_pids: &std::collections::HashSet<u32>) -> bool {
    if let Some(cmdline) = get_process_cmdline_with_retry(pid) {
        if cmdline.iter().skip(1).any(|arg| is_cli_subcommand(arg)) {
            return false;
        }
        return true;
    }
    server_pids.contains(&pid)
}

pub struct ServerPidGuard {
    paths: Vec<PathBuf>,
}

impl Drop for ServerPidGuard {
    fn drop(&mut self) {
        for path in &self.paths {
            let _ = fs::remove_file(path);
        }
    }
}

pub fn create_server_pid_guard() -> ServerPidGuard {
    let mut paths = Vec::new();
    let my_pid = std::process::id().to_string();

    let env_kind = detect_environment();
    if let EnvironmentKind::StandardInstalled { install_dir, .. } = env_kind {
        let p = install_dir.join(".server.pid");
        if safe_write_file(&p, my_pid.as_bytes()).is_ok() {
            paths.push(p);
        }
    }
    let insights_pid = crate::db::get_insights_dir().join(".server.pid");
    if safe_write_file(&insights_pid, my_pid.as_bytes()).is_ok() {
        paths.push(insights_pid);
    }

    ServerPidGuard { paths }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateError {
    SafeRejection(String),
    Failure(String),
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpdateError::SafeRejection(msg) => write!(f, "{msg}"),
            UpdateError::Failure(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for UpdateError {}

impl From<String> for UpdateError {
    fn from(s: String) -> Self {
        UpdateError::Failure(s)
    }
}

#[derive(Debug, Clone, Default)]
pub struct UpdateOptions {
    pub check_only: bool,
    pub force: bool,
    pub target_version: Option<String>,
    pub prefetched_release: Option<GitHubRelease>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum EnvironmentKind {
    StandardInstalled {
        install_dir: PathBuf,
        exe_path: PathBuf,
    },
    Npm {
        exe_path: PathBuf,
    },
    GitOrDev {
        root: PathBuf,
        exe_path: PathBuf,
    },
    Other {
        exe_path: PathBuf,
    },
}

#[derive(Deserialize, Debug, Clone)]
pub struct GitHubAsset {
    pub name: String,
    pub browser_download_url: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct GitHubRelease {
    pub tag_name: String,
    pub assets: Vec<GitHubAsset>,
}

pub fn current_target_triple() -> Option<&'static str> {
    #[cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"))]
    {
        Some("x86_64-unknown-linux-gnu")
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        Some("aarch64-apple-darwin")
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        Some("x86_64-apple-darwin")
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64", target_env = "msvc"))]
    {
        Some("x86_64-pc-windows-msvc")
    }
    #[cfg(not(any(
        all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"),
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "x86_64", target_env = "msvc"),
    )))]
    {
        None
    }
}

pub fn standard_install_dir() -> Option<PathBuf> {
    if let Some(custom) = crate::paths::env_path("TOKEN_USAGE_INSIGHTS_INSTALL_DIR") {
        return Some(custom);
    }

    #[cfg(windows)]
    {
        if let Some(local_app_data) = dirs::data_local_dir() {
            return Some(local_app_data.join("TokenUsageInsights"));
        }
    }

    #[cfg(not(windows))]
    {
        if let Some(home) = dirs::home_dir() {
            return Some(home.join(".local").join("share").join(APP_NAME));
        }
    }

    None
}

pub fn detect_environment() -> EnvironmentKind {
    let raw_exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from(APP_NAME));
    let exe_path = fs::canonicalize(&raw_exe).unwrap_or(raw_exe);
    detect_environment_with_path(&exe_path)
}

pub(crate) fn detect_environment_with_path(exe_path: &Path) -> EnvironmentKind {
    let exe_dir = exe_path.parent().unwrap_or_else(|| Path::new("."));

    // 1. 檢查是否由 npm / npx 啟動
    let path_str = exe_path.to_string_lossy();
    if path_str.contains("node_modules")
        || path_str.contains("token-usage-insights-bin")
        || std::env::var_os("npm_config_user_agent").is_some()
        || std::env::var_os("npm_lifecycle_event").is_some()
    {
        return EnvironmentKind::Npm {
            exe_path: exe_path.to_path_buf(),
        };
    }

    // 2. 檢查是否在 Git 或 Cargo 開發原始碼目錄（優先攔截以保護原始碼不被更新機制覆寫）
    for ancestor in exe_dir.ancestors() {
        if ancestor.join(".git").exists() || ancestor.join("Cargo.toml").exists() {
            return EnvironmentKind::GitOrDev {
                root: ancestor.to_path_buf(),
                exe_path: exe_path.to_path_buf(),
            };
        }
    }

    // 3. 檢查標準安裝目錄（包含以 TOKEN_USAGE_INSIGHTS_INSTALL_DIR 明確指定的路徑）
    if let Some(std_dir) = standard_install_dir() {
        let canonical_std_dir = fs::canonicalize(&std_dir).unwrap_or(std_dir);
        if let Ok(canonical_exe_dir) = fs::canonicalize(exe_dir) {
            if canonical_exe_dir == canonical_std_dir {
                return EnvironmentKind::StandardInstalled {
                    install_dir: canonical_std_dir,
                    exe_path: exe_path.to_path_buf(),
                };
            }
        }
    }

    // 4. 檢查安裝標記檔（由 install.sh 或 install.ps1 寫入的自訂安裝目錄）
    let marker_path = exe_dir.join(".install_marker");
    if let Ok(meta) = fs::symlink_metadata(&marker_path) {
        if meta.is_file() && !meta.file_type().is_symlink() {
            if let Ok(content) = fs::read_to_string(&marker_path) {
                if content.trim() == "token-usage-insights:installed" {
                    let install_dir =
                        fs::canonicalize(exe_dir).unwrap_or_else(|_| exe_dir.to_path_buf());
                    return EnvironmentKind::StandardInstalled {
                        install_dir,
                        exe_path: exe_path.to_path_buf(),
                    };
                }
            }
        }
    }

    // 5. 兼容舊版既有自訂安裝（在引入 .install_marker 之前建立的安裝目錄）：
    // 若目錄包含完整必要資產（pricing.csv, VERSION, static/index.html）
    let has_installed_assets = exe_dir.join("pricing.csv").is_file()
        && exe_dir.join("VERSION").is_file()
        && exe_dir.join("static").join("index.html").is_file();

    if has_installed_assets {
        let install_dir = fs::canonicalize(exe_dir).unwrap_or_else(|_| exe_dir.to_path_buf());
        return EnvironmentKind::StandardInstalled {
            install_dir,
            exe_path: exe_path.to_path_buf(),
        };
    }

    EnvironmentKind::Other {
        exe_path: exe_path.to_path_buf(),
    }
}

pub fn parse_semver(v: &str) -> Option<(u32, u32, u32)> {
    let clean = v
        .trim()
        .strip_prefix('v')
        .or_else(|| v.trim().strip_prefix('V'))
        .unwrap_or(v.trim());
    let mut parts = clean.split('.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts.next()?.parse::<u32>().ok()?;
    let patch_part = parts.next()?;
    let patch = patch_part
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse::<u32>()
        .ok()?;
    Some((major, minor, patch))
}

pub fn is_newer_version(remote: &str, current: &str) -> bool {
    let (r_maj, r_min, r_pat) = match parse_semver(remote) {
        Some(v) => v,
        None => return false,
    };
    let (c_maj, c_min, c_pat) = match parse_semver(current) {
        Some(v) => v,
        None => return false,
    };

    (r_maj, r_min, r_pat) > (c_maj, c_min, c_pat)
}

pub fn archive_filename(tag: &str, target: &str) -> String {
    let tag = tag.trim();

    #[cfg(windows)]
    {
        format!("{APP_NAME}-{tag}-{target}.zip")
    }

    #[cfg(not(windows))]
    {
        format!("{APP_NAME}-{tag}-{target}.tar.gz")
    }
}

pub fn parse_checksum(sums_text: &str, target_filename: &str) -> Option<String> {
    for line in sums_text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() >= 2 {
            let hash = parts[0].trim().to_lowercase();
            let raw_file = parts[1].trim_start_matches('*').trim();
            let file = raw_file.strip_prefix("./").unwrap_or(raw_file);
            let file_name = Path::new(file)
                .file_name()
                .and_then(|f| f.to_str())
                .unwrap_or(file);
            if (file == target_filename || file_name == target_filename) && hash.len() == 64 {
                return Some(hash);
            }
        }
    }
    None
}

pub fn verify_hash_hex(actual_hex: &str, expected_hex: &str) -> bool {
    actual_hex.trim().eq_ignore_ascii_case(expected_hex.trim())
}

#[allow(dead_code)] // 提供外部呼叫與單元測試比對記憶體資料 SHA256 雜湊之輔助函式
pub fn verify_sha256(bytes: &[u8], expected_hex: &str) -> bool {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let result = hasher.finalize();
    let actual_hex = hex::encode(result);
    verify_hash_hex(&actual_hex, expected_hex)
}

pub fn log_update(level: &str, action: &str, message: &str) {
    let log_path = crate::db::get_insights_dir().join("update.log");
    if let Some(parent) = log_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let now = Utc::now().to_rfc3339();
    let line = format!("[{now}] [{level}] [{action}] {message}\n");
    if let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        let _ = file.write_all(line.as_bytes());
    }
}

/// 讀取 config.yaml 中關於 auto_update 與 update_check_interval 的設定
pub fn parse_config_yaml(content: &str) -> (Option<bool>, Option<i64>) {
    let mut auto_update = None;
    let mut update_check_interval = None;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }
        if let Some((key, raw_val)) = trimmed.split_once(':') {
            let key = key.trim();
            let raw_val = raw_val.trim();
            let val = if let Some(rest) = raw_val.strip_prefix('"') {
                if let Some(end) = rest.find('"') {
                    &rest[..end]
                } else {
                    rest.trim_matches('"')
                }
            } else if let Some(rest) = raw_val.strip_prefix('\'') {
                if let Some(end) = rest.find('\'') {
                    &rest[..end]
                } else {
                    rest.trim_matches('\'')
                }
            } else {
                raw_val.split('#').next().unwrap_or("").trim()
            };

            if key == "auto_update" {
                let lower = val.to_lowercase();
                if lower == "true" || lower == "1" || lower == "yes" {
                    auto_update = Some(true);
                } else if lower == "false" || lower == "0" || lower == "no" {
                    auto_update = Some(false);
                }
            } else if key == "update_check_interval" {
                if let Ok(days) = val.parse::<i64>() {
                    update_check_interval = Some(days);
                }
            }
        }
    }

    (auto_update, update_check_interval)
}

pub fn load_update_config() -> (Option<bool>, Option<i64>) {
    let candidates = [
        crate::db::get_insights_dir().join("config.yaml"),
        PathBuf::from("config.yaml"),
    ];
    for path in candidates {
        if let Ok(content) = fs::read_to_string(&path) {
            return parse_config_yaml(&content);
        }
    }
    (None, None)
}

#[cfg(unix)]
fn try_lock_file_exclusive(file: &fs::File) -> Result<bool, std::io::Error> {
    use std::os::unix::io::AsRawFd;
    let res = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if res == 0 {
        Ok(true)
    } else {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EWOULDBLOCK) || err.raw_os_error() == Some(libc::EAGAIN)
        {
            Ok(false)
        } else {
            Err(err)
        }
    }
}

#[cfg(unix)]
fn unlock_file(file: &fs::File) -> Result<(), std::io::Error> {
    use std::os::unix::io::AsRawFd;
    let res = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
    if res == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn try_lock_file_exclusive(file: &fs::File) -> Result<bool, std::io::Error> {
    use std::os::windows::io::AsRawHandle;
    type Handle = *mut std::ffi::c_void;

    #[repr(C)]
    struct Overlapped {
        internal: usize,
        internal_high: usize,
        offset: u32,
        offset_high: u32,
        event: Handle,
    }

    const LOCKFILE_FAIL_IMMEDIATELY: u32 = 0x00000001;
    const LOCKFILE_EXCLUSIVE_LOCK: u32 = 0x00000002;
    const ERROR_LOCK_VIOLATION: i32 = 33;

    extern "system" {
        fn LockFileEx(
            hFile: Handle,
            dwFlags: u32,
            dwReserved: u32,
            nNumberOfBytesToLockLow: u32,
            nNumberOfBytesToLockHigh: u32,
            lpOverlapped: *mut Overlapped,
        ) -> i32;
    }
    let handle = file.as_raw_handle() as Handle;
    let mut overlapped = std::mem::MaybeUninit::<Overlapped>::zeroed();
    let ret = unsafe {
        LockFileEx(
            handle,
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            1,
            0,
            overlapped.as_mut_ptr(),
        )
    };
    if ret != 0 {
        Ok(true)
    } else {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(ERROR_LOCK_VIOLATION) {
            Ok(false)
        } else {
            Err(err)
        }
    }
}

#[cfg(windows)]
fn unlock_file(file: &fs::File) -> Result<(), std::io::Error> {
    use std::os::windows::io::AsRawHandle;
    type Handle = *mut std::ffi::c_void;
    extern "system" {
        fn UnlockFile(
            hFile: Handle,
            dwFileOffsetLow: u32,
            dwFileOffsetHigh: u32,
            nNumberOfBytesToLockLow: u32,
            nNumberOfBytesToLockHigh: u32,
        ) -> i32;
    }
    let handle = file.as_raw_handle() as Handle;
    let ret = unsafe { UnlockFile(handle, 0, 0, 1, 0) };
    if ret != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(any(unix, windows)))]
fn try_lock_file_exclusive(_file: &fs::File) -> Result<bool, std::io::Error> {
    Ok(true)
}

#[cfg(not(any(unix, windows)))]
fn unlock_file(_file: &fs::File) -> Result<(), std::io::Error> {
    Ok(())
}

#[derive(Debug)]
struct UpdateLock {
    _lock_path: PathBuf,
    _file: fs::File,
}

impl UpdateLock {
    fn lock_path(install_dir: &Path) -> PathBuf {
        install_dir.join(".update.lock")
    }

    fn try_acquire(install_dir: &Path) -> Result<Self, String> {
        let lock_path = Self::lock_path(install_dir);
        if let Ok(meta) = fs::symlink_metadata(&lock_path) {
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x00000400;
                if meta.file_type().is_symlink()
                    || (meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0
                    || !meta.is_file()
                {
                    return Err(format!(
                        "拒絕在符號連結、重剖析點或非正規檔案上建立更新鎖 ({lock_path:?})；更新中止以確保安全"
                    ));
                }
            }
            #[cfg(not(windows))]
            {
                if meta.file_type().is_symlink() || !meta.is_file() {
                    return Err(format!(
                        "拒絕在符號連結或非正規檔案上建立更新鎖 ({lock_path:?})；更新中止以確保安全"
                    ));
                }
            }
        }

        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);

        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }

        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x00200000;
            options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        }

        let file = options
            .open(&lock_path)
            .map_err(|e| format!("無法建立或開啟更新鎖定檔 ({lock_path:?}): {e}"))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let handle_meta = file
                .metadata()
                .map_err(|e| format!("無法讀取開啟之更新鎖定檔元資料 ({lock_path:?}): {e}"))?;
            if !handle_meta.is_file() {
                return Err(format!("開啟的更新鎖定檔非正規檔案 ({lock_path:?})"));
            }
            let sym_meta = fs::symlink_metadata(&lock_path)
                .map_err(|e| format!("無法重新確認更新鎖定檔路徑元資料 ({lock_path:?}): {e}"))?;
            if sym_meta.file_type().is_symlink()
                || sym_meta.dev() != handle_meta.dev()
                || sym_meta.ino() != handle_meta.ino()
            {
                return Err(format!(
                    "偵測到更新鎖定檔路徑在開啟後被置換或為符號連結 ({lock_path:?})；更新中止以確保安全"
                ));
            }
        }

        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x00000400;
            let handle_meta = file
                .metadata()
                .map_err(|e| format!("無法讀取開啟之更新鎖定檔元資料 ({lock_path:?}): {e}"))?;
            if !handle_meta.is_file()
                || (handle_meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0
            {
                return Err(format!(
                    "開啟的更新鎖定檔為符號連結、重剖析點或非正規檔案 ({lock_path:?})"
                ));
            }
            let sym_meta = fs::symlink_metadata(&lock_path)
                .map_err(|e| format!("無法重新確認更新鎖定檔路徑元資料 ({lock_path:?}): {e}"))?;
            if sym_meta.file_type().is_symlink()
                || (sym_meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0
            {
                return Err(format!(
                    "偵測到更新鎖定檔路徑在開啟後被置換或為符號連結 ({lock_path:?})；更新中止以確保安全"
                ));
            }
        }

        let locked = try_lock_file_exclusive(&file)
            .map_err(|e| format!("嘗試鎖定更新檔失敗 ({lock_path:?}): {e}"))?;

        if !locked {
            return Err("已有另一個更新程序正在執行中，請稍候再試。".to_string());
        }

        // 寫入當前進程 PID 供診斷與日誌記錄
        let mut f = &file;
        let _ = writeln!(f, "pid={}", std::process::id());

        Ok(Self {
            _lock_path: lock_path,
            _file: file,
        })
    }

    /// 檢查是否有活躍中的更新程序持鎖（使用 OS 層級非阻塞顧問鎖）
    fn is_locked(install_dir: &Path) -> bool {
        let lock_path = Self::lock_path(install_dir);
        match fs::symlink_metadata(&lock_path) {
            Ok(meta) => {
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x00000400;
                    if meta.file_type().is_symlink()
                        || (meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0
                        || !meta.is_file()
                    {
                        return true;
                    }
                }
                #[cfg(not(windows))]
                {
                    if meta.file_type().is_symlink() || !meta.is_file() {
                        return true;
                    }
                }
            }
            Err(_) => {
                if !lock_path.exists() {
                    return false;
                }
                return true;
            }
        }

        let mut options = fs::OpenOptions::new();
        options.read(true).write(true);

        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }

        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x00200000;
            options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        }

        let file = match options.open(&lock_path) {
            Ok(f) => f,
            Err(_) => return true,
        };

        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if let Ok(handle_meta) = file.metadata() {
                if !handle_meta.is_file() {
                    return true;
                }
                if let Ok(sym_meta) = fs::symlink_metadata(&lock_path) {
                    if sym_meta.file_type().is_symlink()
                        || sym_meta.dev() != handle_meta.dev()
                        || sym_meta.ino() != handle_meta.ino()
                    {
                        return true;
                    }
                }
            }
        }

        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x00000400;
            if let Ok(handle_meta) = file.metadata() {
                if !handle_meta.is_file()
                    || (handle_meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0
                {
                    return true;
                }
            }
            if let Ok(sym_meta) = fs::symlink_metadata(&lock_path) {
                if sym_meta.file_type().is_symlink()
                    || (sym_meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0
                {
                    return true;
                }
            }
        }

        match try_lock_file_exclusive(&file) {
            Ok(true) => {
                let _ = unlock_file(&file);
                false
            }
            Ok(false) => true,
            Err(_) => true,
        }
    }
}

impl Drop for UpdateLock {
    fn drop(&mut self) {
        let _ = unlock_file(&self._file);
    }
}

struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    fn new(path: PathBuf) -> Result<Self, String> {
        if path.exists() {
            if let Err(e) = fs::remove_dir_all(&path) {
                let stale_path = path.with_extension(format!("stale-{}", Utc::now().timestamp()));
                if let Err(re) = fs::rename(&path, &stale_path) {
                    return Err(format!(
                        "清除舊暫存目錄失敗 ({path:?}): {e}；換名亦失敗: {re}"
                    ));
                }
            }
        }
        fs::create_dir_all(&path).map_err(|e| format!("建立暫存目錄失敗: {e}"))?;
        Ok(Self { path })
    }

    fn cleanup(&self) -> Result<(), String> {
        if self.path.exists() {
            if let Err(e) = fs::remove_dir_all(&self.path) {
                log_update("WARN", "CLEANUP", &format!("清理暫存目錄失敗: {e}"));
                let stale_path = self
                    .path
                    .with_extension(format!("stale-{}", Utc::now().timestamp()));
                if let Err(re) = fs::rename(&self.path, &stale_path) {
                    let msg = format!(
                        "暫存目錄無法清理亦無法換名 ({:?}): {re}；請稍後手動刪除",
                        self.path
                    );
                    log_update("WARN", "CLEANUP", &msg);
                    eprintln!("⚠️ {msg}");
                    return Err(msg);
                }
            }
        }
        Ok(())
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

#[cfg(windows)]
fn atomic_rename_overwrite(src: &Path, dst: &Path) -> Result<(), std::io::Error> {
    use std::os::windows::ffi::OsStrExt;
    let mut src_wide: Vec<u16> = src.as_os_str().encode_wide().collect();
    src_wide.push(0);
    let mut dst_wide: Vec<u16> = dst.as_os_str().encode_wide().collect();
    dst_wide.push(0);

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x00000001;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x00000008;

    extern "system" {
        fn MoveFileExW(
            lpExistingFileName: *const u16,
            lpNewFileName: *const u16,
            dwFlags: u32,
        ) -> i32;
    }

    let ret = unsafe {
        MoveFileExW(
            src_wide.as_ptr(),
            dst_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };

    if ret != 0 {
        return Ok(());
    }

    if dst.exists() {
        let _ = fs::remove_file(dst);
    }
    fs::rename(src, dst)
}

#[cfg(not(windows))]
fn atomic_rename_overwrite(src: &Path, dst: &Path) -> Result<(), std::io::Error> {
    fs::rename(src, dst)
}

fn safe_replace_file(src: &Path, dst: &Path) -> Result<(), String> {
    if let Ok(meta) = dst.symlink_metadata() {
        if meta.file_type().is_symlink() {
            let _ = fs::remove_file(dst);
        }
    }
    let tmp = dst.with_extension(format!("tmp.{}", std::process::id()));
    let _ = fs::remove_file(&tmp);
    fs::copy(src, &tmp).map_err(|e| format!("複製暫存檔失敗 ({tmp:?}): {e}"))?;
    atomic_rename_overwrite(&tmp, dst).map_err(|e| format!("替換檔案失敗 ({dst:?}): {e}"))
}

fn safe_write_file(dst: &Path, content: &[u8]) -> Result<(), String> {
    if let Some(parent) = dst.parent() {
        if !parent.exists() {
            let _ = fs::create_dir_all(parent);
        }
    }
    if let Ok(meta) = dst.symlink_metadata() {
        if meta.file_type().is_symlink() {
            let _ = fs::remove_file(dst);
        } else if meta.is_dir() {
            return Err(format!("目標路徑為目錄，無法覆寫檔案 ({dst:?})"));
        }
    }
    let tmp = dst.with_extension(format!("tmp.{}", std::process::id()));
    let _ = fs::remove_file(&tmp);
    fs::write(&tmp, content).map_err(|e| format!("寫入暫存檔失敗 ({tmp:?}): {e}"))?;
    atomic_rename_overwrite(&tmp, dst).map_err(|e| format!("替換標記檔失敗 ({dst:?}): {e}"))
}

fn extract_archive(archive_path: &Path, dest_dir: &Path, is_zip: bool) -> Result<(), String> {
    use std::io::Read;
    fs::create_dir_all(dest_dir).map_err(|e| format!("建立解壓縮目錄失敗: {e}"))?;

    let file = fs::File::open(archive_path)
        .map_err(|e| format!("開啟壓縮包檔案失敗 ({archive_path:?}): {e}"))?;

    let mut total_bytes: u64 = 0;

    if is_zip {
        let mut archive =
            zip::ZipArchive::new(file).map_err(|e| format!("解析 ZIP 封裝失敗: {e}"))?;
        if archive.len() > MAX_EXTRACTED_ENTRIES {
            return Err(format!(
                "ZIP 壓縮包項目數超過安全上限 ({MAX_EXTRACTED_ENTRIES})"
            ));
        }

        for i in 0..archive.len() {
            let mut entry = archive
                .by_index(i)
                .map_err(|e| format!("讀取 ZIP 項目失敗: {e}"))?;
            let enclosed = entry
                .enclosed_name()
                .ok_or_else(|| "ZIP 內含無效相對路徑".to_string())?
                .to_path_buf();
            let outpath = dest_dir.join(&enclosed);
            if !outpath.starts_with(dest_dir) {
                return Err(format!(
                    "ZIP 包含超出解壓目錄的不安全檔案路徑: {enclosed:?}"
                ));
            }

            if let Some(mode) = entry.unix_mode() {
                if mode & 0o170000 == 0o120000 {
                    return Err(format!("ZIP 包含不安全的符號連結: {:?}", entry.name()));
                }
            }

            if entry.is_dir() {
                fs::create_dir_all(&outpath)
                    .map_err(|e| format!("建立目錄失敗 ({outpath:?}): {e}"))?;
            } else {
                if let Some(parent) = outpath.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|e| format!("建立上層目錄失敗 ({parent:?}): {e}"))?;
                }
                let mut outfile = fs::File::create(&outpath)
                    .map_err(|e| format!("建立檔案失敗 ({outpath:?}): {e}"))?;
                let mut buffer = [0u8; 8192];
                loop {
                    let n = entry
                        .read(&mut buffer)
                        .map_err(|e| format!("解壓讀取失敗: {e}"))?;
                    if n == 0 {
                        break;
                    }
                    total_bytes = total_bytes.saturating_add(n as u64);
                    if total_bytes > MAX_EXTRACTED_BYTES {
                        return Err(format!(
                            "解壓縮展開大小超過安全上限 ({MAX_EXTRACTED_BYTES} 位元組)"
                        ));
                    }
                    outfile
                        .write_all(&buffer[..n])
                        .map_err(|e| format!("寫入解壓檔案失敗: {e}"))?;
                }
            }
        }
    } else {
        let tar_gz = flate2::read::GzDecoder::new(file);
        let mut archive = tar::Archive::new(tar_gz);
        let mut count: usize = 0;

        for entry in archive
            .entries()
            .map_err(|e| format!("讀取 tar 項目清單失敗: {e}"))?
        {
            count = count.saturating_add(1);
            if count > MAX_EXTRACTED_ENTRIES {
                return Err(format!(
                    "tar.gz 壓縮包項目數超過安全上限 ({MAX_EXTRACTED_ENTRIES})"
                ));
            }
            let mut entry = entry.map_err(|e| format!("讀取 tar 項目失敗: {e}"))?;
            let path = entry
                .path()
                .map_err(|e| format!("讀取 tar 路徑失敗: {e}"))?
                .to_path_buf();

            if path.is_absolute()
                || path.to_string_lossy().starts_with('/')
                || path.to_string_lossy().starts_with('\\')
                || path.components().any(|c| {
                    matches!(
                        c,
                        std::path::Component::ParentDir
                            | std::path::Component::RootDir
                            | std::path::Component::Prefix(..)
                    )
                })
            {
                return Err(format!("tar 包含不安全的檔案路徑: {path:?}"));
            }
            let entry_type = entry.header().entry_type();
            if entry_type.is_symlink() || entry_type.is_hard_link() {
                return Err(format!("tar 包含不安全的連結項目: {path:?}"));
            }
            let outpath = dest_dir.join(&path);
            if !outpath.starts_with(dest_dir) {
                return Err(format!("tar 包含超出解壓目錄的不安全檔案路徑: {path:?}"));
            }

            if entry.header().entry_type().is_dir() {
                fs::create_dir_all(&outpath)
                    .map_err(|e| format!("建立目錄失敗 ({outpath:?}): {e}"))?;
            } else {
                if let Some(parent) = outpath.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|e| format!("建立上層目錄失敗 ({parent:?}): {e}"))?;
                }
                let mut outfile = fs::File::create(&outpath)
                    .map_err(|e| format!("建立檔案失敗 ({outpath:?}): {e}"))?;
                let mut buffer = [0u8; 8192];
                loop {
                    let n = entry
                        .read(&mut buffer)
                        .map_err(|e| format!("解壓讀取失敗: {e}"))?;
                    if n == 0 {
                        break;
                    }
                    total_bytes = total_bytes.saturating_add(n as u64);
                    if total_bytes > MAX_EXTRACTED_BYTES {
                        return Err(format!(
                            "解壓縮展開大小超過安全上限 ({MAX_EXTRACTED_BYTES} 位元組)"
                        ));
                    }
                    outfile
                        .write_all(&buffer[..n])
                        .map_err(|e| format!("寫入解壓檔案失敗: {e}"))?;
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(mode) = entry.header().mode() {
                        let _ = fs::set_permissions(&outpath, fs::Permissions::from_mode(mode));
                    }
                }
            }
        }
    }

    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    if !src.exists() {
        return Ok(());
    }
    if let Ok(meta) = dst.symlink_metadata() {
        if meta.file_type().is_symlink() {
            let _ = fs::remove_file(dst);
            let _ = fs::remove_dir(dst);
        }
    }
    fs::create_dir_all(dst).map_err(|e| format!("建立目標目錄失敗 {dst:?}: {e}"))?;
    for entry in fs::read_dir(src).map_err(|e| format!("讀取目錄失敗 {src:?}: {e}"))? {
        let entry = entry.map_err(|e| format!("讀取項目失敗: {e}"))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|e| format!("讀取檔案類型失敗: {e}"))?;
        if file_type.is_symlink() {
            return Err(format!(
                "拒絕處理目錄中的符號連結項目 ({src_path:?})；更新中止以確保系統安全"
            ));
        }
        if file_type.is_dir() {
            if let Ok(meta) = dst_path.symlink_metadata() {
                if meta.file_type().is_symlink() {
                    let _ = fs::remove_file(&dst_path);
                    let _ = fs::remove_dir(&dst_path);
                }
            }
            copy_dir_recursive(&src_path, &dst_path)?;
        } else if file_type.is_file() {
            safe_replace_file(&src_path, &dst_path)
                .map_err(|e| format!("複製檔案失敗 {src_path:?} -> {dst_path:?}: {e}"))?;
        } else {
            return Err(format!(
                "目錄包含不支援的檔案類型 ({src_path:?})；更新中止以確保安全"
            ));
        }
    }
    Ok(())
}

const MANAGED_ITEMS: &[&str] = &[
    APP_NAME,
    #[cfg(windows)]
    "token-usage-insights.exe",
    "static",
    "pricing.csv",
    "shell",
    "scripts",
    "install.sh",
    "install.ps1",
    "VERSION",
    "README.md",
    "LICENSE",
    ".install_marker",
];

fn backup_installation(install_dir: &Path, backup_dir: &Path) -> Result<(), String> {
    if backup_dir.exists() {
        let err = format!(
            "偵測到備份目錄已存在 ({backup_dir:?})；疑似先前更新中斷或失敗留存之救援狀態。為保護歷史版本不被覆蓋，已中止本次更新。請先手動確認還原舊版或清除該目錄後再更新。"
        );
        log_update("ERROR", "BACKUP", &err);
        return Err(err);
    }
    let staging_dir = install_dir.join(format!(".backup-staging-{}", std::process::id()));
    if staging_dir.exists() {
        let _ = fs::remove_dir_all(&staging_dir);
    }
    fs::create_dir_all(&staging_dir).map_err(|e| format!("建立備份暫存目錄失敗: {e}"))?;

    let backup_res = (|| -> Result<(), String> {
        let mut manifest_entries = Vec::new();
        for &item in MANAGED_ITEMS {
            let src = install_dir.join(item);
            let dst = staging_dir.join(item);
            if let Ok(meta) = src.symlink_metadata() {
                if meta.file_type().is_symlink() {
                    return Err(format!(
                        "安裝目錄包含不安全的符號連結項目 ({src:?})，已拒絕備份以確保系統安全"
                    ));
                }
                if meta.is_dir() {
                    manifest_entries.push(item);
                    copy_dir_recursive(&src, &dst)?;
                } else if meta.is_file() {
                    manifest_entries.push(item);
                    safe_replace_file(&src, &dst)
                        .map_err(|e| format!("備份檔案失敗 {item}: {e}"))?;
                } else {
                    return Err(format!(
                        "安裝目錄包含不支援的檔案類型 ({src:?})，已中止備份"
                    ));
                }
            }
        }

        safe_write_file(
            &staging_dir.join(".manifest"),
            manifest_entries.join("\n").as_bytes(),
        )
        .map_err(|e| format!("寫入備份清單失敗: {e}"))?;

        fs::rename(&staging_dir, backup_dir).map_err(|e| {
            format!("切換至正式備份目錄失敗 ({staging_dir:?} -> {backup_dir:?}): {e}")
        })?;

        Ok(())
    })();

    if let Err(e) = backup_res {
        let _ = fs::remove_dir_all(&staging_dir);
        let _ = fs::remove_dir_all(backup_dir);
        return Err(e);
    }

    Ok(())
}

fn restore_from_backup(backup_dir: &Path, install_dir: &Path) -> Result<(), String> {
    match fs::symlink_metadata(backup_dir) {
        Ok(meta) => {
            if meta.file_type().is_symlink() || !meta.is_dir() {
                return Err(format!(
                    "拒絕在符號連結或非正規目錄之備份目錄上執行還原 ({backup_dir:?})；更新中止以確保安全"
                ));
            }
        }
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                return Ok(());
            }
            return Err(format!("無法讀取備份目錄元資料 ({backup_dir:?}): {e}"));
        }
    }

    let manifest_path = backup_dir.join(".manifest");
    let original_items: std::collections::HashSet<String> = if manifest_path.exists() {
        let content = fs::read_to_string(&manifest_path)
            .map_err(|e| format!("讀取備份清單失敗 ({manifest_path:?}): {e}"))?;
        content
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    } else {
        std::collections::HashSet::new()
    };

    // 1. 移除更新期間新增、但原始安裝中並不存在的受管理項目
    for &item in MANAGED_ITEMS {
        if !original_items.contains(item) {
            let path = install_dir.join(item);
            if let Ok(meta) = path.symlink_metadata() {
                if meta.file_type().is_symlink() {
                    let _ = fs::remove_file(&path);
                    let _ = fs::remove_dir(&path);
                } else if meta.is_dir() {
                    fs::remove_dir_all(&path)
                        .map_err(|e| format!("回滾清理新增目錄失敗 {path:?}: {e}"))?;
                } else {
                    let _ = fs::remove_file(&path);
                }
            }
        }
    }

    // 2. 還原備份項目
    for entry in fs::read_dir(backup_dir).map_err(|e| format!("讀取備份目錄失敗: {e}"))? {
        let entry = entry.map_err(|e| format!("讀取備份項目失敗: {e}"))?;
        let name = entry.file_name();
        if name == ".manifest" {
            continue;
        }
        let src = entry.path();
        let dst = install_dir.join(&name);
        let file_type = entry.file_type().map_err(|e| e.to_string())?;
        if file_type.is_symlink() {
            return Err(format!(
                "拒絕還原備份目錄中的符號連結項目 ({src:?})；更新中止以確保系統安全"
            ));
        }
        if file_type.is_dir() {
            if let Ok(meta) = dst.symlink_metadata() {
                if meta.file_type().is_symlink() {
                    let _ = fs::remove_file(&dst);
                    let _ = fs::remove_dir(&dst);
                }
            }
            if dst.exists() {
                fs::remove_dir_all(&dst)
                    .map_err(|e| format!("清理還原目標目錄失敗 {dst:?}: {e}"))?;
            }
            copy_dir_recursive(&src, &dst)?;
        } else if file_type.is_file() {
            let current_exe = std::env::current_exe().ok();
            let is_current_exe = current_exe
                .as_ref()
                .and_then(|c| fs::canonicalize(c).ok())
                .zip(fs::canonicalize(&dst).ok())
                .map(|(a, b)| a == b)
                .unwrap_or(false);

            if is_current_exe {
                self_replace::self_replace(&src).map_err(|e| format!("回滾目前執行檔失敗: {e}"))?;
            } else {
                safe_replace_file(&src, &dst)
                    .map_err(|e| format!("還原檔案失敗 {src:?} -> {dst:?}: {e}"))?;
            }
        } else {
            return Err(format!(
                "備份目錄包含不支援的檔案類型 ({src:?})，已中止還原"
            ));
        }
    }
    Ok(())
}

async fn fetch_release_with_logging(
    tag_opt: Option<&str>,
    timeout_secs: u64,
) -> Result<GitHubRelease, String> {
    match fetch_release(tag_opt, timeout_secs).await {
        Ok(r) => Ok(r),
        Err(e) => {
            log_update("ERROR", "CHECK", &format!("查詢 GitHub Releases 失敗: {e}"));
            Err(e)
        }
    }
}

fn validate_download_url(url: &str) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("無效的下載網址 ({url}): {e}"))?;
    let scheme = parsed.scheme();
    let host = parsed.host_str().unwrap_or_default();
    let is_loopback =
        host == "127.0.0.1" || host == "localhost" || host == "::1" || host == "[::1]";

    if scheme == "https" || (scheme == "http" && (cfg!(test) || is_loopback)) {
        Ok(())
    } else {
        Err(format!("拒絕使用非 HTTPS 下載網址以維護傳輸安全: {url}"))
    }
}

fn build_secure_http_client(timeout_secs: u64) -> Result<reqwest::Client, String> {
    let redirect_policy = reqwest::redirect::Policy::custom(|attempt| {
        let scheme = attempt.url().scheme();
        let host = attempt.url().host_str().unwrap_or_default();
        let is_loopback =
            host == "127.0.0.1" || host == "localhost" || host == "::1" || host == "[::1]";
        if scheme == "https" || (scheme == "http" && (cfg!(test) || is_loopback)) {
            if attempt.previous().len() >= 10 {
                attempt.error("超過重新導向次數上限 (10)")
            } else {
                attempt.follow()
            }
        } else {
            attempt.error("拒絕重新導向至非加密 HTTP 協定")
        }
    });

    reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .redirect(redirect_policy)
        .build()
        .map_err(|e| format!("建立 HTTP 用戶端失敗: {e}"))
}

async fn fetch_release(tag_opt: Option<&str>, timeout_secs: u64) -> Result<GitHubRelease, String> {
    let client = build_secure_http_client(timeout_secs)?;

    let url = match tag_opt {
        Some(tag) => {
            let clean_tag = if tag.starts_with('v') || tag.starts_with('V') {
                tag.to_string()
            } else {
                format!("v{tag}")
            };
            format!("https://api.github.com/repos/{GITHUB_OWNER}/{GITHUB_REPO}/releases/tags/{clean_tag}")
        }
        None => {
            format!("https://api.github.com/repos/{GITHUB_OWNER}/{GITHUB_REPO}/releases/latest")
        }
    };

    let resp = client
        .get(&url)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/vnd.github.v3+json")
        .send()
        .await
        .map_err(|e| format!("查詢 GitHub Releases 失敗: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("GitHub API 回應異常 HTTP {}", resp.status()));
    }

    resp.json::<GitHubRelease>()
        .await
        .map_err(|e| format!("解析 Release JSON 失敗: {e}"))
}

async fn download_to_file_with_hash(
    url: &str,
    dest_path: &Path,
    max_bytes: usize,
    timeout_secs: u64,
) -> Result<String, String> {
    validate_download_url(url)?;
    let client = build_secure_http_client(timeout_secs)?;

    let mut resp = client
        .get(url)
        .header("User-Agent", USER_AGENT)
        .send()
        .await
        .map_err(|e| format!("下載失敗 ({url}): {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("下載失敗 HTTP {} ({url})", resp.status()));
    }

    if let Some(cl) = resp.content_length() {
        if cl > max_bytes as u64 {
            return Err(format!(
                "檔案大小 ({cl} 位元組) 超過安全上限 ({max_bytes} 位元組)"
            ));
        }
    }

    let mut file = fs::File::create(dest_path)
        .map_err(|e| format!("建立下載檔案失敗 ({dest_path:?}): {e}"))?;
    let mut hasher = Sha256::new();
    let mut downloaded: usize = 0;

    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("讀取下載串流失敗 ({url}): {e}"))?
    {
        downloaded = downloaded.saturating_add(chunk.len());
        if downloaded > max_bytes {
            let _ = fs::remove_file(dest_path);
            return Err(format!("下載累計大小超過安全上限 ({max_bytes} 位元組)"));
        }
        file.write_all(&chunk)
            .map_err(|e| format!("寫入下載檔案失敗 ({dest_path:?}): {e}"))?;
        hasher.update(&chunk);
    }

    file.flush()
        .map_err(|e| format!("排清寫入暫存檔失敗: {e}"))?;

    let actual_hash = hex::encode(hasher.finalize()).to_lowercase();
    Ok(actual_hash)
}

async fn download_text_capped(
    url: &str,
    max_bytes: usize,
    timeout_secs: u64,
) -> Result<String, String> {
    validate_download_url(url)?;
    let client = build_secure_http_client(timeout_secs)?;

    let mut resp = client
        .get(url)
        .header("User-Agent", USER_AGENT)
        .send()
        .await
        .map_err(|e| format!("下載失敗 ({url}): {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("下載失敗 HTTP {} ({url})", resp.status()));
    }

    if let Some(cl) = resp.content_length() {
        if cl > max_bytes as u64 {
            return Err(format!("校驗檔大小 ({cl} 位元組) 超過安全上限"));
        }
    }

    let mut buffer = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("讀取校驗檔串流失敗: {e}"))?
    {
        if buffer.len().saturating_add(chunk.len()) > max_bytes {
            return Err("校驗檔內容超過安全上限".to_string());
        }
        buffer.extend_from_slice(&chunk);
    }

    Ok(String::from_utf8_lossy(&buffer).to_string())
}

fn print_and_log_check_result(
    remote_version: &str,
    current_version: &str,
    hint_newer: Option<&str>,
) {
    let is_newer = is_newer_version(remote_version, current_version);
    println!("  目前版本: v{current_version}");
    println!("  最新版本: {remote_version}");
    if is_newer {
        if let Some(hint) = hint_newer {
            println!("{hint}");
        } else {
            println!("💡 發現新版本！可執行 token-usage-insights update 進行更新。");
        }
        log_update(
            "INFO",
            "CHECK",
            &format!("版本檢查完成：發現新版本 {remote_version}（目前為 v{current_version}）"),
        );
    } else {
        println!("✅ 目前已是最新版本。");
        log_update(
            "INFO",
            "CHECK",
            &format!("版本檢查完成：目前已是最新版本 v{current_version}"),
        );
    }
}

pub(crate) fn get_installed_version(install_dir: &Path) -> String {
    if let Ok(content) = fs::read_to_string(install_dir.join("VERSION")) {
        let v = content.trim().trim_start_matches(['v', 'V']);
        if !v.is_empty() {
            return v.to_string();
        }
    }
    env!("CARGO_PKG_VERSION").to_string()
}

pub async fn run_update(options: UpdateOptions) -> Result<(), UpdateError> {
    let env_kind = detect_environment();

    match &env_kind {
        EnvironmentKind::Npm { .. } => {
            if options.check_only {
                let release =
                    fetch_release_with_logging(options.target_version.as_deref(), 15).await?;
                let current_version = env!("CARGO_PKG_VERSION");
                let remote_version = release.tag_name.trim();
                print_and_log_check_result(
                    remote_version,
                    current_version,
                    Some("💡 發現新版本！可執行 npx token-usage-insights@latest 使用最新版。"),
                );
                return Ok(());
            }
            let msg = r#"⚠️ 偵測到目前透過 npm / npx 執行，不支援直接原地自我更新。
👉 請使用以下指令取得或執行最新版本：
   npx token-usage-insights@latest
   # 或全域安裝更新：
   npm install -g token-usage-insights@latest"#;
            eprintln!("{msg}");
            log_update("WARN", "CHECK", "略過更新：偵測到 npm / npx 執行環境");
            return Err(UpdateError::SafeRejection(
                "不支援在 npm / npx 環境中直接自我更新".to_string(),
            ));
        }
        EnvironmentKind::GitOrDev { root, .. } => {
            if options.check_only {
                let release =
                    fetch_release_with_logging(options.target_version.as_deref(), 15).await?;
                let current_version = env!("CARGO_PKG_VERSION");
                let remote_version = release.tag_name.trim();
                print_and_log_check_result(
                    remote_version,
                    current_version,
                    Some("💡 發現新版本！請使用 git pull / cargo build 進行更新。"),
                );
                return Ok(());
            }
            let msg = format!(
                "錯誤：目前執行檔位於開發目錄中 ({root:?})，不支援直接更新。\n請使用 git pull / cargo build 進行更新。"
            );
            eprintln!("{msg}");
            log_update("ERROR", "CHECK", &format!("拒絕更新：開發目錄 {root:?}"));
            return Err(UpdateError::SafeRejection(
                "開發目錄不支援自我更新".to_string(),
            ));
        }
        EnvironmentKind::Other { exe_path } => {
            if options.check_only {
                let release =
                    fetch_release_with_logging(options.target_version.as_deref(), 15).await?;
                let current_version = env!("CARGO_PKG_VERSION");
                let remote_version = release.tag_name.trim();
                print_and_log_check_result(
                    remote_version,
                    current_version,
                    Some("💡 發現新版本！請在標準安裝目錄中執行更新。"),
                );
                return Ok(());
            }
            let msg = format!(
                "錯誤：目前執行檔位於非標準安裝目錄 ({exe_path:?})。\n請在標準安裝目錄中執行更新。"
            );
            eprintln!("{msg}");
            log_update(
                "ERROR",
                "CHECK",
                &format!("拒絕更新：非標準目錄 {exe_path:?}"),
            );
            return Err(UpdateError::SafeRejection(
                "非標準目錄不支援自我更新".to_string(),
            ));
        }
        EnvironmentKind::StandardInstalled { .. } => {}
    }

    let install_dir = match &env_kind {
        EnvironmentKind::StandardInstalled { install_dir, .. } => install_dir.clone(),
        _ => unreachable!(),
    };

    // 若非純檢查，在開始任何更新操作前先執行啟動救援以還原或清理先前中斷之殘留備份
    if !options.check_only {
        perform_startup_recovery().await;
    }

    run_update_in_dir(&install_dir, options).await
}

pub(crate) async fn run_update_in_dir(
    install_dir: &Path,
    options: UpdateOptions,
) -> Result<(), UpdateError> {
    // 若非純檢查，在開始任何更新操作前先取得安裝目錄之獨占鎖
    let _lock = if !options.check_only {
        Some(match UpdateLock::try_acquire(install_dir) {
            Ok(l) => l,
            Err(e) => {
                log_update("ERROR", "LOCK", &e);
                return Err(e.into());
            }
        })
    } else {
        None
    };

    // 取得更新鎖後，重新讀取安裝目錄目前實際之版本，防範排隊等待鎖期間已被其他更新程序完成升級
    let current_version_str = get_installed_version(install_dir);
    let current_version = current_version_str.as_str();
    println!("🔍 正在檢查最新發行版本...");
    log_update(
        "INFO",
        "CHECK",
        &format!("開始檢查更新（目前版本 v{current_version}）"),
    );

    let release = match options.prefetched_release {
        Some(r) => r,
        None => fetch_release_with_logging(options.target_version.as_deref(), 15).await?,
    };
    let remote_version = release.tag_name.trim();

    if options.check_only {
        let hint = match current_target_triple() {
            Some(_) => None,
            None => Some("💡 發現新版本！但目前作業系統/硬體架構無預編譯發行包，需手動編譯。"),
        };
        print_and_log_check_result(remote_version, current_version, hint);
        return Ok(());
    }

    let is_newer = is_newer_version(remote_version, current_version);
    println!("  目前版本: v{current_version}");
    println!("  目標版本: {remote_version}");

    if !is_newer && !options.force && options.target_version.is_none() {
        println!("✅ 目前已是最新版本 ({remote_version})。使用 --force 可強制重新安裝。");
        log_update("INFO", "CHECK", "已是最新版本，略過更新");
        return Ok(());
    }

    let target = current_target_triple().ok_or_else(|| {
        let msg = "目前作業系統或硬體架構不支援預先編譯的二進位發行檔".to_string();
        log_update("ERROR", "CHECK", &msg);
        msg
    })?;

    let archive_name = archive_filename(remote_version, target);
    let alt_name = if let Some(stripped) = remote_version.strip_prefix('v') {
        archive_filename(stripped, target)
    } else {
        archive_filename(&format!("v{remote_version}"), target)
    };
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == archive_name || a.name == alt_name)
        .ok_or_else(|| {
            let err = format!("Release {remote_version} 缺少目標發行包: {archive_name}");
            log_update("ERROR", "DOWNLOAD", &err);
            err
        })?;

    let checksum_asset = release
        .assets
        .iter()
        .find(|a| a.name == "SHA256SUMS")
        .ok_or_else(|| {
            let err = format!("Release {remote_version} 缺少 SHA256SUMS 校驗檔");
            log_update("ERROR", "DOWNLOAD", &err);
            err
        })?;

    // 使用 TempDirGuard 確保異常離開時自動清理暫存（置於 install_dir 底下確保受獨占鎖保護）
    let update_tmp_dir = install_dir.join(".update-tmp");
    let tmp_guard = match TempDirGuard::new(update_tmp_dir) {
        Ok(g) => g,
        Err(e) => {
            log_update("ERROR", "PREPARE", &e);
            return Err(e.into());
        }
    };

    println!("⬇️ 正在下載發行包: {} ...", asset.name);
    log_update("INFO", "DOWNLOAD", &format!("開始串流下載 {}", asset.name));

    // 安全性驗證：確認 asset.name 為單純檔案名稱，防止路徑穿越攻擊
    {
        let name_path = std::path::Path::new(&asset.name);
        let is_safe = name_path
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
            && name_path.components().count() == 1
            && !asset.name.contains('/')
            && !asset.name.contains('\\');
        if !is_safe {
            let err = format!(
                "發行包名稱包含非法路徑字元，拒絕下載以防路徑穿越: {:?}",
                asset.name
            );
            log_update("ERROR", "DOWNLOAD", &err);
            return Err(err.into());
        }
    }

    let archive_path = tmp_guard.path.join(&asset.name);
    let actual_hash = match download_to_file_with_hash(
        &asset.browser_download_url,
        &archive_path,
        MAX_ARCHIVE_BYTES,
        60,
    )
    .await
    {
        Ok(h) => h,
        Err(e) => {
            log_update("ERROR", "DOWNLOAD", &e);
            return Err(e.into());
        }
    };

    println!("⬇️ 正在下載校驗檔 SHA256SUMS ...");
    let sums_text =
        match download_text_capped(&checksum_asset.browser_download_url, MAX_CHECKSUM_BYTES, 15)
            .await
        {
            Ok(t) => t,
            Err(e) => {
                log_update("ERROR", "DOWNLOAD", &e);
                return Err(e.into());
            }
        };

    let expected_hash = match parse_checksum(&sums_text, &asset.name) {
        Some(h) => h,
        None => {
            let err = format!("SHA256SUMS 中未找到 {} 的校驗碼", asset.name);
            log_update("ERROR", "VERIFY", &err);
            return Err(err.into());
        }
    };

    println!("🔒 正在驗證 SHA256 校驗碼...");
    if !verify_hash_hex(&actual_hash, &expected_hash) {
        let err = format!("SHA256 校驗失敗！預期 {expected_hash}，實際 {actual_hash}");
        log_update("ERROR", "VERIFY", &err);
        return Err(err.into());
    }
    println!("✅ SHA256 校驗通過！");
    log_update("INFO", "VERIFY", "SHA256 校驗通過");

    let is_zip = asset.name.ends_with(".zip");
    let extract_dir = tmp_guard.path.join("extracted");
    println!("📦 正在解壓縮檔案...");

    // 將耗時之同步解壓縮與原子替換移至 blocking thread，避免阻塞 tokio 執行緒並支援超時隔離
    let install_task_res = tokio::task::spawn_blocking({
        let archive_path = archive_path.clone();
        let extract_dir = extract_dir.clone();
        let install_dir = install_dir.to_path_buf();
        let remote_version = remote_version.to_string();
        move || -> Result<(), String> {
            if let Err(e) = extract_archive(&archive_path, &extract_dir, is_zip) {
                log_update("ERROR", "EXTRACT", &e);
                return Err(e);
            }

            // 尋找解壓後的根目錄（可能有一層子目錄）
            let release_root = if extract_dir.join(APP_NAME).exists()
                || extract_dir.join(format!("{APP_NAME}.exe")).exists()
            {
                extract_dir
            } else {
                let mut found = None;
                if let Ok(entries) = fs::read_dir(&extract_dir) {
                    for entry in entries.flatten() {
                        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                            let sub = entry.path();
                            if sub.join(APP_NAME).exists()
                                || sub.join(format!("{APP_NAME}.exe")).exists()
                            {
                                found = Some(sub);
                                break;
                            }
                        }
                    }
                }
                match found {
                    Some(dir) => dir,
                    None => {
                        let err = "解壓後的目錄中未找到執行檔".to_string();
                        log_update("ERROR", "EXTRACT", &err);
                        return Err(err);
                    }
                }
            };

            let exec_name = if cfg!(windows) {
                format!("{APP_NAME}.exe")
            } else {
                APP_NAME.to_string()
            };

            // 驗證解壓後的發行包是否包含所有必要資源，避免不完整安裝造成混合版本
            let required_items = [
                exec_name.as_str(),
                "static",
                "pricing.csv",
                "VERSION",
                "scripts",
                "shell",
                "install.sh",
                "install.ps1",
            ];
            for required in required_items {
                if !release_root.join(required).exists() {
                    let err = format!("解壓發行包缺少必要資源: {required}");
                    log_update("ERROR", "VERIFY", &err);
                    return Err(err);
                }
            }

            let backup_dir = install_dir.join(".backup");
            apply_installation_with_rollback(&release_root, &install_dir, &backup_dir)?;

            println!("🎉 成功更新至版本 {remote_version}！");
            log_update("INFO", "INSTALL", &format!("成功更新至 {remote_version}"));

            Ok(())
        }
    })
    .await;

    match install_task_res {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(e.into()),
        Err(join_err) => return Err(format!("安裝任務執行異常: {join_err}").into()),
    }

    let _ = tmp_guard.cleanup();

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoppedProcessSpec {
    pub pid: u32,
    pub is_supervised: bool,
    pub supervisor_pid: Option<u32>,
    pub is_server: bool,
    pub exe_path: PathBuf,
    pub args: Option<Vec<String>>,
    pub envs: Vec<(String, String)>,
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DashboardProcessPlan {
    pub stopped_specs: Vec<StoppedProcessSpec>,
    #[cfg(unix)]
    pub supervised_unix_pids: Vec<u32>,
}

fn stop_running_dashboard_instances(install_dir: &Path) -> Result<DashboardProcessPlan, String> {
    let my_pid = std::process::id();
    let mut candidate_pids = std::collections::HashSet::new();

    // 1. 從 .server.pid 讀取 PID 作為候選
    let pid_file = install_dir.join(".server.pid");
    let mut server_pids = std::collections::HashSet::new();
    if let Ok(content) = fs::read_to_string(&pid_file) {
        if let Ok(pid) = content.trim().parse::<u32>() {
            if pid != my_pid {
                candidate_pids.insert(pid);
                server_pids.insert(pid);
            }
        }
    }
    let insights_pid_file = crate::db::get_insights_dir().join(".server.pid");
    if let Ok(content) = fs::read_to_string(&insights_pid_file) {
        if let Ok(pid) = content.trim().parse::<u32>() {
            if pid != my_pid {
                candidate_pids.insert(pid);
                server_pids.insert(pid);
            }
        }
    }

    // 2. 透過系統進程清單掃描 APP_NAME
    #[cfg(target_os = "linux")]
    {
        if let Ok(entries) = fs::read_dir("/proc") {
            for entry in entries.flatten() {
                if let Ok(name) = entry.file_name().into_string() {
                    if let Ok(pid) = name.parse::<u32>() {
                        if pid != my_pid {
                            if let Ok(cmdline) = fs::read_to_string(format!("/proc/{pid}/cmdline"))
                            {
                                if cmdline.contains(APP_NAME) {
                                    candidate_pids.insert(pid);
                                }
                            }
                        }
                    }
                }
            }
        } else {
            return Err("無法讀取 /proc 目錄列舉進程；更新中止以確保安全".to_string());
        }
    }

    #[cfg(all(unix, not(target_os = "linux")))]
    {
        match std::process::Command::new("pgrep")
            .args(["-f", APP_NAME])
            .output()
        {
            Ok(output) if output.status.success() => {
                let text = String::from_utf8_lossy(&output.stdout);
                for line in text.lines() {
                    if let Ok(pid) = line.trim().parse::<u32>() {
                        if pid != my_pid {
                            candidate_pids.insert(pid);
                        }
                    }
                }
            }
            Ok(output) if output.status.code() == Some(1) => {
                // pgrep 回傳 1 代表系統中無任何匹配之進程，為正常狀態
            }
            _ => {
                // pgrep 執行失敗或未安裝時，嘗試 ps 作為回退；若兩者皆失敗則 fail closed
                let ps_res = std::process::Command::new("ps")
                    .args(["-axo", "pid,command"])
                    .output()
                    .map_err(|pe| format!("進程列舉失敗 (ps: {pe})；更新中止以確保安全"))?;
                if !ps_res.status.success() {
                    return Err("ps 命令執行失敗；更新中止以確保安全".to_string());
                }
                let text = String::from_utf8_lossy(&ps_res.stdout);
                for line in text.lines() {
                    if line.contains(APP_NAME) {
                        let trimmed = line.trim_start();
                        if let Some((pid_str, _)) = trimmed.split_once(' ') {
                            if let Ok(pid) = pid_str.trim().parse::<u32>() {
                                if pid != my_pid {
                                    candidate_pids.insert(pid);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[cfg(windows)]
    {
        let exec_name = format!("{APP_NAME}.exe");
        let output = std::process::Command::new("tasklist")
            .args([
                "/FI",
                &format!("IMAGENAME eq {exec_name}"),
                "/FO",
                "CSV",
                "/NH",
            ])
            .output()
            .map_err(|e| format!("列舉 Windows 進程失敗: {e}"))?;
        if !output.status.success() {
            let err_text = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "tasklist 命令執行失敗 (exit code: {:?}): {err_text}",
                output.status.code()
            ));
        }
        let text = String::from_utf8_lossy(&output.stdout);
        for line in text.lines() {
            let parts: Vec<&str> = line.split(',').collect();
            if parts.len() >= 2 {
                let pid_str = parts[1].trim().trim_matches('"');
                if let Ok(pid) = pid_str.parse::<u32>() {
                    if pid != my_pid {
                        candidate_pids.insert(pid);
                    }
                }
            }
        }
    }

    // 3. 嚴格驗證候選 PID：必須為活躍進程且執行檔路徑確認位於 install_dir，並過濾短暫 CLI 指令進程（Fail-Closed 原則）
    let mut stopped_specs = Vec::new();
    #[cfg(unix)]
    let mut supervised_unix_pids = Vec::new();
    let mut pids_to_stop = Vec::new();

    for pid in candidate_pids {
        if is_process_alive(pid) {
            let exe_path = match get_process_exe_path(pid) {
                Some(p) => p,
                None => {
                    if !is_process_alive(pid) {
                        continue;
                    }
                    let msg = format!(
                        "無法驗證活躍候選進程 (PID: {pid}) 之執行檔路徑；為防在服務執行中覆寫檔案，更新中止以確保安全 (Fail-Closed)"
                    );
                    log_update("ERROR", "STOP_SERVICE", &msg);
                    return Err(msg);
                }
            };

            if matches_install_dir(&exe_path, install_dir) {
                if !is_process_alive(pid) {
                    continue;
                }
                if !is_dashboard_server_process(pid, &server_pids) {
                    let msg = format!(
                        "偵測到有 Token 戰情室指令進程 (PID: {pid}) 正在安裝目錄中執行；為防檔案衝突，更新中止以確保安全 (Fail-Closed)"
                    );
                    log_update("ERROR", "STOP_SERVICE", &msg);
                    return Err(msg);
                }

                let supervisor_pid = get_process_supervisor_pid(pid);
                let is_sup = is_process_supervised(pid, install_dir);
                let is_server = server_pids.contains(&pid);
                let args = get_process_cmdline_with_retry(pid);
                let envs = get_process_relevant_envs_with_retry(pid);
                let cwd = get_process_cwd_with_retry(pid);

                // 非監管進程重啟驗證：若為非監管進程，必須確保重啟所需之元資料可用，否則中止更新以防未停止進程即覆寫或重啟失敗
                if !is_sup {
                    if !is_process_alive(pid) {
                        continue;
                    }
                    if cwd.is_none() {
                        let msg = format!(
                            "非監管進程 (PID: {pid}) 無法可靠取得工作目錄，無法保證安全重啟；更新中止以確保安全 (Fail-Closed)"
                        );
                        log_update("ERROR", "STOP_SERVICE", &msg);
                        return Err(msg);
                    }
                    if args.is_none() && !is_server {
                        let msg = format!(
                            "非監管進程 (PID: {pid}) 無法取得命令列參數且非已知服務，無法保證安全重啟；更新中止以確保安全 (Fail-Closed)"
                        );
                        log_update("ERROR", "STOP_SERVICE", &msg);
                        return Err(msg);
                    }
                    if envs.is_none() {
                        let msg = format!(
                            "非監管進程 (PID: {pid}) 無法讀取目標進程環境變數，無法保證安全重啟；更新中止以確保安全 (Fail-Closed)"
                        );
                        log_update("ERROR", "STOP_SERVICE", &msg);
                        return Err(msg);
                    }
                }

                let spec = StoppedProcessSpec {
                    pid,
                    is_supervised: is_sup,
                    supervisor_pid,
                    is_server,
                    exe_path,
                    args: args.or_else(|| {
                        if is_server {
                            Some(vec![APP_NAME.to_string()])
                        } else {
                            None
                        }
                    }),
                    envs: envs.unwrap_or_default(),
                    cwd,
                };

                #[cfg(unix)]
                if is_sup {
                    log_update(
                        "INFO",
                        "STOP_SERVICE",
                        &format!("進程 (PID: {pid}) 受到 Unix 服務管理器監管；將在檔案替換提交後再發送 SIGTERM 以免舊版搶先重啟"),
                    );
                    supervised_unix_pids.push(pid);
                    continue;
                }

                pids_to_stop.push(pid);
                stopped_specs.push(spec);
            }
        }
    }

    // 若在 Windows 環境且有受到 run-service.ps1 監管之服務進程，寫入服務重啟協商標記檔，讓 run-service.ps1 能在新版就緒後重啟
    let restart_pending_file = install_dir.join(".service_restart_pending");
    #[cfg(windows)]
    {
        let had_supervised = stopped_specs.iter().any(|s| s.is_supervised);
        if had_supervised {
            safe_write_file(&restart_pending_file, b"1").map_err(|e| {
                let err = format!("無法寫入服務重啟協商標記檔 ({restart_pending_file:?}): {e}");
                log_update("ERROR", "STOP_SERVICE", &err);
                err
            })?;

            // 針對 Windows 監管進程，必須明確等待所有受監管子進程完全終止，防範檔案替換時發生共享衝突 (sharing violation)
            let supervised_pids: Vec<u32> = stopped_specs
                .iter()
                .filter(|s| s.is_supervised)
                .map(|s| s.pid)
                .collect();

            if !supervised_pids.is_empty() {
                log_update(
                    "INFO",
                    "STOP_SERVICE",
                    &format!("等待 Windows 監管服務子進程安全退出: {supervised_pids:?}"),
                );

                let wait_start = Instant::now();
                let sup_timeout = Duration::from_secs(6);
                let sup_escalation = Duration::from_millis(2500);
                let mut sup_escalated = false;
                let mut remaining_sup = supervised_pids;

                while !remaining_sup.is_empty() {
                    remaining_sup.retain(|&pid| is_process_alive(pid));
                    if remaining_sup.is_empty() {
                        break;
                    }

                    let elapsed = wait_start.elapsed();
                    if elapsed >= sup_timeout {
                        let err = format!(
                            "等待 Windows 監管服務進程 (PID: {remaining_sup:?}) 停止逾時，更新中止以保護檔案安全"
                        );
                        log_update("ERROR", "STOP_SERVICE", &err);
                        let _ = fs::remove_file(&restart_pending_file);
                        return Err(err);
                    }

                    if elapsed >= sup_escalation && !sup_escalated {
                        sup_escalated = true;
                        log_update(
                            "WARN",
                            "STOP_SERVICE",
                            &format!(
                                "監管進程未於 2.5 秒內退出，升級強制終止 (PID: {remaining_sup:?})"
                            ),
                        );
                        for &pid in &remaining_sup {
                            let _ = std::process::Command::new("taskkill")
                                .args(["/PID", &pid.to_string(), "/T", "/F"])
                                .output();
                        }
                    }

                    std::thread::sleep(Duration::from_millis(100));
                }

                log_update(
                    "INFO",
                    "STOP_SERVICE",
                    "所有 Windows 監管服務子進程已確認完全退出",
                );
            }
        }
    }

    if pids_to_stop.is_empty() {
        return Ok(DashboardProcessPlan {
            stopped_specs,
            #[cfg(unix)]
            supervised_unix_pids,
        });
    }

    log_update(
        "INFO",
        "STOP_SERVICE",
        &format!("協調停止執行中之服務進程: {pids_to_stop:?}"),
    );

    // 4. 發送初次溫和退出訊號 (Unix: SIGTERM, Windows: taskkill 無 /F)
    for &pid in &pids_to_stop {
        #[cfg(unix)]
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGTERM);
        }
        #[cfg(windows)]
        {
            let _ = std::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T"])
                .output();
        }
    }

    // 5. 積極輪詢並在逾時 2.5 秒後升級強制終止
    let start_time = Instant::now();
    let timeout = Duration::from_secs(5);
    let escalation_delay = Duration::from_millis(2500);
    let poll_interval = Duration::from_millis(100);
    let mut escalated = false;

    loop {
        pids_to_stop.retain(|&pid| is_process_alive(pid));
        if pids_to_stop.is_empty() {
            break;
        }

        let elapsed = start_time.elapsed();
        if elapsed >= timeout {
            let err = format!(
                "等待執行中之服務進程 (PID: {pids_to_stop:?}) 停止超時，更新中止以保護檔案安全"
            );
            log_update("ERROR", "STOP_SERVICE", &err);
            let _ = fs::remove_file(&restart_pending_file);
            return Err(err);
        }

        if elapsed >= escalation_delay && !escalated {
            escalated = true;
            log_update(
                "WARN",
                "STOP_SERVICE",
                &format!("服務進程未於 2.5 秒內正常退出，升級強制終止 (PID: {pids_to_stop:?})"),
            );
            for &pid in &pids_to_stop {
                #[cfg(unix)]
                unsafe {
                    libc::kill(pid as libc::pid_t, libc::SIGKILL);
                }
                #[cfg(windows)]
                {
                    let _ = std::process::Command::new("taskkill")
                        .args(["/PID", &pid.to_string(), "/T", "/F"])
                        .output();
                }
            }
        }

        std::thread::sleep(poll_interval);
    }

    // 僅刪除屬於已停止進程之 PID 檔案，絕不誤刪目前進程之標記
    for p_file in [&pid_file, &insights_pid_file] {
        if let Ok(content) = fs::read_to_string(p_file) {
            if let Ok(p) = content.trim().parse::<u32>() {
                if p != my_pid && !is_process_alive(p) {
                    let _ = fs::remove_file(p_file);
                }
            }
        }
    }

    log_update("INFO", "STOP_SERVICE", "所有執行中之目標服務進程已安全停止");
    Ok(DashboardProcessPlan {
        stopped_specs,
        #[cfg(unix)]
        supervised_unix_pids,
    })
}

fn restart_dashboard_instance(
    spec: &StoppedProcessSpec,
    install_dir: &Path,
) -> Result<u32, String> {
    let exec_name = if cfg!(windows) {
        format!("{APP_NAME}.exe")
    } else {
        APP_NAME.to_string()
    };
    let exe = if spec.exe_path.exists() {
        spec.exe_path.clone()
    } else {
        install_dir.join(&exec_name)
    };
    if !exe.exists() {
        return Err(format!("找不到執行檔: {exe:?}"));
    }

    let fallback_args = vec![APP_NAME.to_string()];
    let args = match &spec.args {
        Some(a) => a,
        None if spec.is_server => &fallback_args,
        None => {
            return Err("無法可靠取得先前進程之命令列參數，略過自動重啟以防組態重設".to_string());
        }
    };

    let child_args = if args.len() > 1 { &args[1..] } else { &[] };

    if child_args.iter().any(|arg| is_cli_subcommand(arg)) {
        return Err("先前進程包含非看板 CLI 子命令，略過自動重啟".to_string());
    }

    let mut cmd = std::process::Command::new(&exe);
    cmd.args(child_args);

    let cwd = match &spec.cwd {
        Some(c) => c.as_path(),
        None => {
            return Err(
                "無法可靠取得先前進程之工作目錄，略過自動重啟以防資料與組態偏離".to_string(),
            );
        }
    };
    cmd.current_dir(cwd);

    // 先從繼承的環境中移除所有相關環境變數，確保重啟進程的環境不受更新器自身環境污染
    for &key in RELEVANT_ENV_VARS {
        cmd.env_remove(key);
    }
    // 再套用從目標進程記憶體讀取的原始環境變數
    for (k, v) in &spec.envs {
        cmd.env(k, v);
    }

    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("啟動背景看板進程失敗: {e}"))?;

    let child_pid = child.id();

    std::thread::sleep(Duration::from_millis(50));
    if let Ok(Some(status)) = child.try_wait() {
        return Err(format!("背景看板進程啟動後立即異常退出: {status}"));
    }

    Ok(child_pid)
}

#[cfg(windows)]
fn is_any_dashboard_running_in_dir(install_dir: &Path) -> bool {
    let my_pid = std::process::id();
    let mut server_pids = std::collections::HashSet::new();

    let pid_file = install_dir.join(".server.pid");
    if let Ok(content) = fs::read_to_string(&pid_file) {
        if let Ok(pid) = content.trim().parse::<u32>() {
            if pid != my_pid && is_process_alive(pid) {
                server_pids.insert(pid);
                if let Some(exe_path) = get_process_exe_path(pid) {
                    if matches_install_dir(&exe_path, install_dir)
                        && is_dashboard_server_process(pid, &server_pids)
                    {
                        return true;
                    }
                }
            }
        }
    }
    let insights_pid = crate::db::get_insights_dir().join(".server.pid");
    if let Ok(content) = fs::read_to_string(&insights_pid) {
        if let Ok(pid) = content.trim().parse::<u32>() {
            if pid != my_pid && is_process_alive(pid) {
                server_pids.insert(pid);
                if let Some(exe_path) = get_process_exe_path(pid) {
                    if matches_install_dir(&exe_path, install_dir)
                        && is_dashboard_server_process(pid, &server_pids)
                    {
                        return true;
                    }
                }
            }
        }
    }
    let exec_name = format!("{APP_NAME}.exe");
    if let Ok(output) = std::process::Command::new("tasklist")
        .args([
            "/FI",
            &format!("IMAGENAME eq {exec_name}"),
            "/FO",
            "CSV",
            "/NH",
        ])
        .output()
    {
        let text = String::from_utf8_lossy(&output.stdout);
        for line in text.lines() {
            let parts: Vec<&str> = line.split(',').collect();
            if parts.len() >= 2 {
                let pid_str = parts[1].trim().trim_matches('"');
                if let Ok(pid) = pid_str.parse::<u32>() {
                    if pid == my_pid || !is_process_alive(pid) {
                        continue;
                    }
                    if let Some(exe_path) = get_process_exe_path(pid) {
                        if matches_install_dir(&exe_path, install_dir)
                            && is_dashboard_server_process(pid, &server_pids)
                        {
                            return true;
                        }
                    }
                }
            }
        }
    }
    false
}

#[allow(dead_code)] // 於 Windows 服務重啟流程使用，並於跨平台單元測試驗證環境變數與參數傳遞
fn configure_windows_runner_command(
    cmd: &mut std::process::Command,
    runner_script: &Path,
    install_dir: &Path,
    spec: &StoppedProcessSpec,
) {
    cmd.args([
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-WindowStyle",
        "Hidden",
        "-File",
    ]);
    cmd.arg(runner_script);
    cmd.args(["-InstallDir"]);
    cmd.arg(install_dir);

    let get_env = |target: &str| -> Option<&str> {
        spec.envs
            .iter()
            .find(|(k, _)| k == target)
            .map(|(_, v)| v.as_str())
    };

    if let Some(host) = get_env("HOST") {
        cmd.args(["-HostAddress", host]);
    }
    if let Some(port) = get_env("PORT") {
        cmd.args(["-Port", port]);
    }
    if let Some(auto_update) = get_env("TOKEN_USAGE_INSIGHTS_AUTO_UPDATE") {
        cmd.args(["-AutoUpdate", auto_update]);
    }
    if let Some(interval) = get_env("TOKEN_USAGE_INSIGHTS_UPDATE_INTERVAL_HOURS") {
        cmd.args(["-UpdateIntervalHours", interval]);
    }

    let cwd = match &spec.cwd {
        Some(c) => c.as_path(),
        None => install_dir,
    };
    cmd.current_dir(cwd);

    // 先從繼承的環境中移除所有相關環境變數，確保守護進程不受更新器自身環境污染
    for &key in RELEVANT_ENV_VARS {
        cmd.env_remove(key);
    }
    // 套用從目標進程記憶體讀取的原始環境變數 (含 INSIGHTS_DIR, 自訂資料庫路徑等)
    for (k, v) in &spec.envs {
        cmd.env(k, v);
    }

    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
}

#[allow(dead_code)] // 於 Windows 移交重啟流程使用，並於跨平台單元測試驗證腳本產生與命令列跳脫
fn build_windows_deferred_restart_script(
    updater_pid: u32,
    install_dir: &Path,
    exe_path: &Path,
    expected_version: &str,
    spec: &StoppedProcessSpec,
) -> Result<String, String> {
    let fallback_args = vec![APP_NAME.to_string()];
    let args = match &spec.args {
        Some(a) => a,
        None if spec.is_server => &fallback_args,
        None => {
            return Err("無法可靠取得先前進程之命令列參數，略過自動重啟以防組態重設".to_string());
        }
    };

    let child_args = if args.len() > 1 { &args[1..] } else { &[] };
    if child_args.iter().any(|arg| is_cli_subcommand(arg)) {
        return Err("先前進程包含非看板 CLI 子命令，略過自動重啟".to_string());
    }

    let cwd = match &spec.cwd {
        Some(c) => c.as_path(),
        None => install_dir,
    };

    let expected_clean = expected_version.trim().trim_start_matches(['v', 'V']);

    let mut ps_script = String::new();
    ps_script.push_str(&format!("$updaterPid = {updater_pid};\n"));
    ps_script.push_str(&format!(
        "$installDir = '{}';\n",
        install_dir.to_string_lossy().replace('\'', "''")
    ));
    ps_script.push_str(&format!(
        "$exePath = '{}';\n",
        exe_path.to_string_lossy().replace('\'', "''")
    ));
    ps_script.push_str(&format!(
        "$expectedVersion = '{}';\n",
        expected_clean.replace('\'', "''")
    ));
    ps_script.push_str(&format!(
        "$cwd = '{}';\n",
        cwd.to_string_lossy().replace('\'', "''")
    ));

    ps_script.push_str("$argList = @(");
    for (idx, arg) in child_args.iter().enumerate() {
        if idx > 0 {
            ps_script.push_str(", ");
        }
        ps_script.push_str(&format!("'{}'", arg.replace('\'', "''")));
    }
    ps_script.push_str(");\n");

    for (k, v) in &spec.envs {
        if !RELEVANT_ENV_VARS.contains(&k.as_str())
            || k == "INSIGHTS_DIR"
            || k == "PORT"
            || k == "HOST"
        {
            ps_script.push_str(&format!("$env:{} = '{}';\n", k, v.replace('\'', "''")));
        }
    }

    ps_script.push_str(r#"
# 1. 等待更新程序退出
while (Get-Process -Id $updaterPid -ErrorAction SilentlyContinue) {
    Start-Sleep -Milliseconds 100
}

# 2. 等待更新鎖 (.update.lock) 釋放
$lockFile = Join-Path $installDir '.update.lock'
$waitCount = 0
while ($waitCount -lt 300) {
    $isLocked = $false
    if (Test-Path -LiteralPath $lockFile) {
        try {
            $stream = [System.IO.File]::Open($lockFile, [System.IO.FileMode]::Open, [System.IO.FileAccess]::ReadWrite, [System.IO.FileShare]::ReadWrite)
            try {
                $stream.Lock(0, 1)
                $stream.Unlock(0, 1)
            } catch {
                $isLocked = $true
            } finally {
                $stream.Dispose()
            }
        } catch {
            $isLocked = $true
        }
    }
    if (-not $isLocked) { break }
    Start-Sleep -Milliseconds 100
    $waitCount++
}

# 3. 等待 self_replace 臨時置換檔完全清理且執行檔可獨占讀取
$readyCount = 0
$exeReady = $false
while ($readyCount -lt 150) {
    if (Test-Path -LiteralPath $exePath) {
        try {
            $exeStream = [System.IO.File]::Open($exePath, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::Read)
            $exeStream.Dispose()
            $tempFiles = @(Get-ChildItem -LiteralPath $installDir -Filter '*.__temp__.exe' -ErrorAction SilentlyContinue)
            $relocatedFiles = @(Get-ChildItem -LiteralPath $installDir -Filter '*.__relocated__.exe' -ErrorAction SilentlyContinue)
            if ($tempFiles.Count -eq 0 -and $relocatedFiles.Count -eq 0) {
                $exeReady = $true
                break
            }
        } catch {}
    }
    Start-Sleep -Milliseconds 100
    $readyCount++
}

# 4. 驗證執行檔版本是否已為新版
$versionMatched = $false
$vCheckCount = 0
while ($vCheckCount -lt 50) {
    try {
        $out = (& $exePath --version 2>&1 | Out-String).Trim()
        $tokens = $out -split '\s+'
        $actualVer = if ($tokens.Count -gt 0) { $tokens[-1].TrimStart('v').TrimStart('V') } else { '' }
        if ($actualVer -eq $expectedVersion) {
            $versionMatched = $true
            break
        }
    } catch {}
    Start-Sleep -Milliseconds 100
    $vCheckCount++
}

# 5. 依版本驗證結果決定啟動或拒絕載入舊版
$logDir = Join-Path $installDir 'logs'
if (!(Test-Path -LiteralPath $logDir)) {
    New-Item -ItemType Directory -Force -Path $logDir | Out-Null
}
$logFile = Join-Path $installDir 'update.log'
$logTime = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')

if ($versionMatched) {
    Add-Content -LiteralPath $logFile -Value "[$logTime] [INFO] [RESTART] 移交守護進程已確認新版執行檔版本 ($expectedVersion)，正在重新啟動看板服務..."
    if ($argList.Count -gt 0) {
        Start-Process -FilePath $exePath -ArgumentList $argList -WorkingDirectory $cwd -WindowStyle Hidden
    } else {
        Start-Process -FilePath $exePath -WorkingDirectory $cwd -WindowStyle Hidden
    }
} else {
    Add-Content -LiteralPath $logFile -Value "[$logTime] [ERROR] [RESTART] 移交守護進程驗證新版執行檔版本失敗 (預期 $expectedVersion)，中止啟動以防載入舊版。"
}
"#);

    Ok(ps_script)
}

#[cfg(windows)]
fn schedule_windows_deferred_restart(
    spec: &StoppedProcessSpec,
    install_dir: &Path,
    expected_version: &str,
) -> Result<(), String> {
    let my_pid = std::process::id();
    let exec_name = format!("{APP_NAME}.exe");
    let exe = if spec.exe_path.exists() {
        spec.exe_path.clone()
    } else {
        install_dir.join(&exec_name)
    };

    let ps_script =
        build_windows_deferred_restart_script(my_pid, install_dir, &exe, expected_version, spec)?;

    let mut cmd = std::process::Command::new("powershell.exe");
    cmd.args([
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-WindowStyle",
        "Hidden",
        "-Command",
        &ps_script,
    ]);

    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    cmd.creation_flags(CREATE_NO_WINDOW);

    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    cmd.spawn()
        .map_err(|e| format!("啟動 Windows 移交重啟進程失敗: {e}"))?;

    Ok(())
}

#[cfg(windows)]
fn restart_windows_supervised_service(
    spec: &StoppedProcessSpec,
    install_dir: &Path,
    is_current_exe: bool,
    expected_version: &str,
) -> Result<Option<u32>, String> {
    log_update(
        "INFO",
        "RESTART",
        &format!(
            "正在為監管進程 (原 PID: {}) 協調重啟或移交服務管理器...",
            spec.pid
        ),
    );

    // 若原服務守護進程 (PowerShell runner) 仍活躍，代表其正持有 .service_restart_pending 標記並等待 .update.lock 釋放；
    // 將該守護進程指定為唯一重啟權擁有者，避免啟動第二個 runner 或看板進程造成端口衝突與重複執行
    if let Some(sup_pid) = spec.supervisor_pid {
        if is_process_alive(sup_pid) {
            log_update(
                "INFO",
                "RESTART",
                &format!(
                    "偵測到原服務守護進程 (PID: {sup_pid}) 仍活躍並正在等待更新鎖釋放；交由其獨佔自動重啟權，略過外部重複重啟"
                ),
            );
            println!(
                "🔄 服務守護進程 (PID: {sup_pid}) 仍在運行，將在更新鎖釋放後自動重新啟動看板服務。"
            );
            return Ok(None);
        }
    }

    // 1. 先等待短暫時間 (1.5 秒)，檢查 runner 是否仍在運行並已透過協商標記自動重啟看板
    let start_wait = Instant::now();
    while start_wait.elapsed() < Duration::from_millis(1500) {
        if is_any_dashboard_running_in_dir(install_dir) {
            println!("🔄 服務管理器已自動重新啟動 Token 戰情室背景看板服務。");
            log_update("INFO", "RESTART", "服務管理器已自動重新啟動背景看板服務");
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    // 2. 若看板尚未運行，代表先前為舊版 runner（在子進程終止時已退出）或管理器未自動恢復；
    // 執行相容交接：依序嘗試以工作排程器 (Scheduled Task) 或重新啟動 run-service.ps1 守護進程
    println!("🔄 偵測到服務管理器尚未自動重啟，正在啟動相容移交重啟服務...");
    log_update(
        "INFO",
        "RESTART",
        "服務管理器尚未自動重啟，執行相容移交重啟 (ScheduledTask / run-service.ps1)",
    );

    let mut started = false;
    let mut runner_pid = None;

    // 2a. 嘗試以工作排程器啟動 (TaskName: TokenUsageInsights_<USERNAME> 或 TokenUsageInsights)
    let username = std::env::var("USERNAME").unwrap_or_default();
    let mut task_candidates = Vec::new();
    if !username.is_empty() {
        task_candidates.push(format!("TokenUsageInsights_{username}"));
    }
    task_candidates.push("TokenUsageInsights".to_string());

    for task_name in task_candidates {
        let output = std::process::Command::new("schtasks")
            .args(["/Run", "/TN", &task_name])
            .output();
        if let Ok(out) = output {
            if out.status.success() {
                log_update(
                    "INFO",
                    "RESTART",
                    &format!("成功透過工作排程器 ({task_name}) 啟動服務"),
                );
                started = true;
                break;
            }
        }
    }

    // 2b. 若工作排程器無法啟動（例如使用啟動資料夾 Startup 捷徑安裝之環境），啟動 run-service.ps1 作為守護進程
    if !started {
        let runner_script = install_dir.join("scripts").join("run-service.ps1");
        if runner_script.exists() {
            let mut cmd = std::process::Command::new("powershell.exe");
            configure_windows_runner_command(&mut cmd, &runner_script, install_dir, spec);

            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);

            if let Ok(child) = cmd.spawn() {
                log_update(
                    "INFO",
                    "RESTART",
                    "已透過 PowerShell 背景啟動 run-service.ps1 守護進程",
                );
                runner_pid = Some(child.id());
                started = true;
            } else {
                log_update(
                    "WARN",
                    "RESTART",
                    "啟動 run-service.ps1 失敗，嘗試直接啟動看板進程",
                );
            }
        }
    }

    // 2c. 若以上皆未成功，回退直接以 restart_dashboard_instance 或移交守護啟動看板進程
    if !started {
        if is_current_exe {
            schedule_windows_deferred_restart(spec, install_dir, expected_version)?;
            return Ok(None);
        }
        return restart_dashboard_instance(spec, install_dir).map(Some);
    }

    // 若為當前執行檔更新，run-service.ps1 會等待更新鎖釋放後才啟動新版，此處直接回傳 runner_pid 不提前超時回退
    if is_current_exe {
        log_update(
            "INFO",
            "RESTART",
            "已成功啟動服務移交 (ScheduledTask / run-service.ps1)，將於更新程序退出並釋放更新鎖後自動載入新版",
        );
        println!("🔄 已成功移交服務管理器，將於更新程序退出後自動載入新版看板服務。");
        return Ok(runner_pid);
    }

    // 3. 等待確認新進程是否成功啟動 (最多等待 5 秒)
    let verify_start = Instant::now();
    while verify_start.elapsed() < Duration::from_secs(5) {
        if is_any_dashboard_running_in_dir(install_dir) {
            println!("🔄 已確認服務已成功重新啟動。");
            log_update("INFO", "RESTART", "已確認監管服務重新啟動成功");
            return Ok(runner_pid);
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    // 若 5 秒後仍未偵測到看板運行，嘗試直接啟動看板作為最後保障
    log_update(
        "WARN",
        "RESTART",
        "服務移交後 5 秒內未偵測到看板進程，執行直接啟動",
    );
    restart_dashboard_instance(spec, install_dir).map(Some)
}

pub(crate) fn apply_installation_with_rollback(
    release_root: &Path,
    install_dir: &Path,
    backup_dir: &Path,
) -> Result<(), String> {
    println!("💾 正在備份現有安裝...");
    if let Err(e) = backup_installation(install_dir, backup_dir) {
        log_update("ERROR", "BACKUP", &e);
        return Err(e);
    }

    println!("⏸️ 正在協調停止現有執行中之服務...");
    let process_plan = match stop_running_dashboard_instances(install_dir) {
        Ok(plan) => plan,
        Err(err) => {
            // 清理已建立之備份目錄與可能寫入的協商標記檔，避免殘留 .backup 阻止後續更新
            let _ = fs::remove_dir_all(backup_dir);
            let _ = fs::remove_file(install_dir.join(".service_restart_pending"));
            log_update(
                "ERROR",
                "STOP_SERVICE",
                &format!("停止服務進程失敗，已清理備份目錄: {err}"),
            );
            return Err(err);
        }
    };

    println!("🚀 正在安裝新版檔案至 {:?} ...", install_dir);
    log_update("INFO", "INSTALL", &format!("開始替換至 {install_dir:?}"));

    let exec_name = if cfg!(windows) {
        format!("{APP_NAME}.exe")
    } else {
        APP_NAME.to_string()
    };

    let target_exe = install_dir.join(&exec_name);
    let current_exe = std::env::current_exe().ok();
    let is_current_exe = current_exe
        .as_ref()
        .and_then(|c| fs::canonicalize(c).ok())
        .zip(fs::canonicalize(&target_exe).ok())
        .map(|(a, b)| a == b)
        .unwrap_or(false);

    let install_result = (|| -> Result<(), String> {
        let src_exe = release_root.join(&exec_name);

        if !src_exe.exists() {
            return Err(format!("來源缺少可執行檔: {src_exe:?}"));
        }

        // 跨平台安全替換執行檔（Windows 使用 self_replace 或安全重命名）
        if is_current_exe {
            self_replace::self_replace(&src_exe)
                .map_err(|e| format!("執行中的程序替換失敗: {e}"))?;
        } else {
            #[cfg(windows)]
            {
                let old_exe = target_exe.with_extension(format!("old.{}.tmp", std::process::id()));
                let _ = fs::remove_file(&old_exe);
                if target_exe.exists() {
                    fs::rename(&target_exe, &old_exe)
                        .map_err(|e| format!("Windows 執行檔換名失敗: {e}"))?;
                }
                if let Err(e) = fs::copy(&src_exe, &target_exe) {
                    let _ = fs::rename(&old_exe, &target_exe);
                    return Err(format!("寫入新執行檔失敗: {e}"));
                }
                let _ = fs::remove_file(&old_exe);
            }

            #[cfg(not(windows))]
            {
                safe_replace_file(&src_exe, &target_exe)?;
            }
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&target_exe, fs::Permissions::from_mode(0o755));
        }

        for folder in ["static", "shell", "scripts"] {
            let src = release_root.join(folder);
            let dst = install_dir.join(folder);
            if src.exists() {
                let staging = install_dir.join(format!(".{folder}-staging-{}", std::process::id()));
                if staging.exists() {
                    fs::remove_dir_all(&staging)
                        .map_err(|e| format!("清除舊暫存目錄失敗 ({staging:?}): {e}"))?;
                }
                copy_dir_recursive(&src, &staging)?;

                if let Ok(meta) = dst.symlink_metadata() {
                    if meta.file_type().is_symlink() {
                        let _ = fs::remove_file(&dst);
                        let _ = fs::remove_dir(&dst);
                    }
                }

                if dst.exists() {
                    let old = install_dir.join(format!(".{folder}-old-{}", std::process::id()));
                    if old.exists() {
                        fs::remove_dir_all(&old)
                            .map_err(|e| format!("清除舊備份目錄失敗 ({old:?}): {e}"))?;
                    }
                    fs::rename(&dst, &old).map_err(|e| format!("目錄安全換名失敗 {dst:?}: {e}"))?;
                    if let Err(e) = fs::rename(&staging, &dst) {
                        let _ = fs::rename(&old, &dst);
                        return Err(format!("原子切換目錄失敗 {folder}: {e}"));
                    }
                    let _ = fs::remove_dir_all(&old);
                } else {
                    fs::rename(&staging, &dst)
                        .map_err(|e| format!("移動新目錄失敗 {folder}: {e}"))?;
                }
            }
        }

        for file in [
            "pricing.csv",
            "VERSION",
            "README.md",
            "LICENSE",
            "install.sh",
            "install.ps1",
        ] {
            let src = release_root.join(file);
            let dst = install_dir.join(file);
            if src.exists() {
                safe_replace_file(&src, &dst)?;
            }
        }

        // 確保自訂安裝目錄保有安裝標記，避免未來更新無法辨識（移除既有符號連結並以原子方式覆寫一般檔案）
        safe_write_file(
            &install_dir.join(".install_marker"),
            b"token-usage-insights:installed",
        )?;

        // 寫入更新就緒標記，供 Windows 服務守護進程確認新執行檔已完全寫入就緒
        safe_write_file(&install_dir.join(".update_ready"), b"ready")?;

        Ok(())
    })();

    if let Err(err) = install_result {
        eprintln!("❌ 安裝失敗，正在自動回滾: {err}");
        log_update("ERROR", "INSTALL", &format!("安裝失敗: {err}，開始回滾"));
        if let Err(rollback_err) = restore_from_backup(backup_dir, install_dir) {
            let _ = fs::write(backup_dir.join(".rollback_failed"), &rollback_err);
            eprintln!(
                "❌ 自動回滾失敗: {rollback_err}；請保留備份目錄 {:?} 進行手動還原",
                backup_dir
            );
            log_update("ERROR", "ROLLBACK", &format!("回滾失敗: {rollback_err}"));
            return Err(format!(
                "安裝失敗 ({err}) 且回滾失敗 ({rollback_err})；備份已保留於 {backup_dir:?}"
            ));
        } else {
            println!("✅ 已成功回滾至先前版本。");
            log_update("INFO", "ROLLBACK", "回滾成功");
            let _ = fs::remove_dir_all(backup_dir);

            // 逐一處理先前停止之進程，獨立處理監管與非監管進程
            for spec in &process_plan.stopped_specs {
                if spec.is_supervised {
                    #[cfg(windows)]
                    {
                        log_update(
                            "INFO",
                            "ROLLBACK",
                            &format!("回滾完成，保留標記由 Windows 服務管理器自動重啟監管進程 (原 PID: {})", spec.pid),
                        );
                    }
                } else {
                    #[cfg(windows)]
                    if is_current_exe {
                        let original_version =
                            fs::read_to_string(install_dir.join("VERSION")).unwrap_or_default();
                        let _ =
                            schedule_windows_deferred_restart(spec, install_dir, &original_version);
                        println!(
                            "🔄 回滾完成，已排定於更新程序退出後重新啟動 PID {} 對應之原版背景看板服務。",
                            spec.pid
                        );
                        continue;
                    }
                    match restart_dashboard_instance(spec, install_dir) {
                        Ok(_) => {
                            println!(
                                "🔄 回滾完成，已重新啟動 PID {} 對應之 Token 戰情室背景看板服務。",
                                spec.pid
                            );
                            log_update(
                                "INFO",
                                "ROLLBACK",
                                &format!(
                                    "回滾完成，已重新啟動非監管背景看板進程 (原 PID: {})",
                                    spec.pid
                                ),
                            );
                        }
                        Err(restart_err) => {
                            eprintln!("⚠️ 回滾完成，但無法自動重新啟動看板進程 (原 PID: {}): {restart_err}；請手動啟動看板服務。", spec.pid);
                            log_update(
                                "WARN",
                                "ROLLBACK",
                                &format!(
                                    "回滾後重啟進程 (原 PID: {}) 失敗: {restart_err}",
                                    spec.pid
                                ),
                            );
                        }
                    }
                }
            }

            #[cfg(windows)]
            {
                let has_supervised = process_plan.stopped_specs.iter().any(|s| s.is_supervised);
                if !has_supervised {
                    let _ = fs::remove_file(install_dir.join(".service_restart_pending"));
                }
            }
        }
        return Err(err);
    }

    let expected_version = fs::read_to_string(install_dir.join("VERSION")).unwrap_or_default();
    let expected_version = expected_version
        .trim()
        .trim_start_matches(['v', 'V'])
        .to_string();
    let expected_version = if expected_version.is_empty() {
        env!("CARGO_PKG_VERSION").to_string()
    } else {
        expected_version
    };
    let _ = &expected_version;
    let _ = is_current_exe;

    // 1. Unix 上在檔案寫入完成後，通知先前記錄之監管服務進程退出以讓 supervisor 自動載入新版執行檔
    #[cfg(unix)]
    {
        for &pid in &process_plan.supervised_unix_pids {
            if is_process_alive(pid) {
                log_update(
                    "INFO",
                    "RESTART",
                    &format!("通知 Unix 服務管理器重啟服務進程 (PID: {pid})"),
                );
                unsafe {
                    libc::kill(pid as libc::pid_t, libc::SIGTERM);
                }
                println!("🔄 已通知服務管理器重啟 (PID: {pid})，將由 supervisor 自動載入新版。");
            }
        }
    }

    // 2. 逐一處理先前停止之進程，獨立處理監管與非監管進程（保留備份直至重啟確認成功）
    let mut restart_errors: Vec<String> = Vec::new();
    let mut spawned_pids: Vec<u32> = Vec::new();
    for spec in &process_plan.stopped_specs {
        if spec.is_supervised {
            #[cfg(windows)]
            {
                match restart_windows_supervised_service(
                    spec,
                    install_dir,
                    is_current_exe,
                    &expected_version,
                ) {
                    Ok(maybe_pid) => {
                        if let Some(pid) = maybe_pid {
                            spawned_pids.push(pid);
                        }
                    }
                    Err(restart_err) => {
                        let msg = format!(
                            "重啟 Windows 監管服務 (原 PID: {}) 失敗: {restart_err}",
                            spec.pid
                        );
                        eprintln!("⚠️ 更新完成，但無法重新啟動 Windows 監管服務 (原 PID: {}): {restart_err}；請手動啟動服務。", spec.pid);
                        log_update("ERROR", "RESTART", &msg);
                        restart_errors.push(msg);
                    }
                }
            }
        } else {
            #[cfg(windows)]
            if is_current_exe {
                match schedule_windows_deferred_restart(spec, install_dir, &expected_version) {
                    Ok(_) => {
                        println!(
                            "🔄 已排定於更新程序退出後由移交守護進程自動啟動 PID {} 對應之新版背景看板服務。",
                            spec.pid
                        );
                        log_update(
                            "INFO",
                            "RESTART",
                            &format!(
                                "更新成功，已排定於更新程序退出並釋放執行檔後由移交守護進程啟動新版看板 (原 PID: {}, 目標版本: {expected_version})",
                                spec.pid
                            ),
                        );
                    }
                    Err(err) => {
                        let msg = format!(
                            "排定 Windows 移交重啟進程 (原 PID: {}) 失敗: {err}",
                            spec.pid
                        );
                        eprintln!("⚠️ 更新完成，但無法排定移交重啟 (原 PID: {}): {err}；請於更新後手動啟動看板服務。", spec.pid);
                        log_update("ERROR", "RESTART", &msg);
                        restart_errors.push(msg);
                    }
                }
                continue;
            }

            match restart_dashboard_instance(spec, install_dir) {
                Ok(child_pid) => {
                    spawned_pids.push(child_pid);
                    println!(
                        "🔄 已重新啟動 PID {} 對應之 Token 戰情室背景看板服務 (新 PID: {child_pid})。",
                        spec.pid
                    );
                    log_update(
                        "INFO",
                        "RESTART",
                        &format!(
                            "更新成功，已重新啟動非監管背景看板進程 (原 PID: {}, 新 PID: {child_pid})",
                            spec.pid
                        ),
                    );
                }
                Err(restart_err) => {
                    let msg = format!("重啟進程 (原 PID: {}) 失敗: {restart_err}", spec.pid);
                    eprintln!("⚠️ 更新完成，但無法自動重新啟動先前停止之看板進程 (原 PID: {}): {restart_err}；請手動啟動看板服務。", spec.pid);
                    log_update("ERROR", "RESTART", &msg);
                    restart_errors.push(msg);
                }
            }
        }
    }

    #[cfg(windows)]
    {
        let has_supervised = process_plan.stopped_specs.iter().any(|s| s.is_supervised);
        if !has_supervised {
            let _ = fs::remove_file(install_dir.join(".service_restart_pending"));
        }
    }

    // 3. 若重啟失敗，此時備份依然完整留存，執行安全自動回滾恢復原版本並重新啟動原服務
    if !restart_errors.is_empty() {
        let combined = restart_errors.join("; ");
        eprintln!("❌ 重啟新版看板服務失敗: {combined}，正在自動回滾至先前版本...");
        log_update(
            "ERROR",
            "RESTART",
            &format!("重啟新版服務失敗: {combined}，開始回滾"),
        );

        // 回滾前先終止本輪重啟已成功啟動之新版子進程及可能已由 Unix 監管者重啟之新版進程，避免新舊進程同時存活導致連接埠衝突或重複執行
        let mut rollback_stop_pids = spawned_pids.clone();
        #[cfg(unix)]
        {
            for &sup_pid in &process_plan.supervised_unix_pids {
                if !rollback_stop_pids.contains(&sup_pid) {
                    rollback_stop_pids.push(sup_pid);
                }
            }
            for pid_file in &[
                install_dir.join(".server.pid"),
                crate::db::get_insights_dir().join(".server.pid"),
            ] {
                if let Ok(content) = fs::read_to_string(pid_file) {
                    if let Ok(pid) = content.trim().parse::<u32>() {
                        if pid != std::process::id()
                            && is_process_alive(pid)
                            && is_process_supervised(pid, install_dir)
                            && !rollback_stop_pids.contains(&pid)
                        {
                            rollback_stop_pids.push(pid);
                        }
                    }
                }
            }
        }

        for &stop_pid in &rollback_stop_pids {
            if is_process_alive(stop_pid) {
                log_update(
                    "INFO",
                    "ROLLBACK",
                    &format!("回滾前停止本輪已啟動或監管之新版進程 (PID: {stop_pid})"),
                );
                #[cfg(unix)]
                unsafe {
                    libc::kill(stop_pid as libc::pid_t, libc::SIGTERM);
                }
                #[cfg(windows)]
                {
                    let _ = std::process::Command::new("taskkill")
                        .args(["/PID", &stop_pid.to_string(), "/T", "/F"])
                        .output();
                }
            }
        }

        let stop_deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < stop_deadline
            && rollback_stop_pids.iter().any(|&p| is_process_alive(p))
        {
            std::thread::sleep(Duration::from_millis(50));
        }

        #[cfg(unix)]
        for &stop_pid in &rollback_stop_pids {
            if is_process_alive(stop_pid) {
                unsafe {
                    libc::kill(stop_pid as libc::pid_t, libc::SIGKILL);
                }
            }
        }

        if let Err(rollback_err) = restore_from_backup(backup_dir, install_dir) {
            let _ = fs::write(backup_dir.join(".rollback_failed"), &rollback_err);
            eprintln!(
                "❌ 自動回滾失敗: {rollback_err}；請保留備份目錄 {:?} 進行手動還原",
                backup_dir
            );
            log_update("ERROR", "ROLLBACK", &format!("回滾失敗: {rollback_err}"));
            return Err(format!(
                "重啟失敗 ({combined}) 且回滾失敗 ({rollback_err})；備份已保留於 {backup_dir:?}"
            ));
        } else {
            println!("✅ 已成功回滾至先前版本。正在恢復原服務...");
            log_update("INFO", "ROLLBACK", "回滾成功，正在恢復原服務");
            let _ = fs::remove_dir_all(backup_dir);

            // 重新啟動先前版本的進程
            for spec in &process_plan.stopped_specs {
                if spec.is_supervised {
                    #[cfg(windows)]
                    {
                        log_update(
                            "INFO",
                            "ROLLBACK",
                            &format!("回滾完成，保留標記由 Windows 服務管理器自動重啟監管進程 (原 PID: {})", spec.pid),
                        );
                    }
                    #[cfg(unix)]
                    {
                        if is_process_alive(spec.pid) {
                            log_update(
                                "INFO",
                                "ROLLBACK",
                                &format!(
                                    "回滾完成，通知 Unix 服務管理器重啟以載入舊版服務 (PID: {})",
                                    spec.pid
                                ),
                            );
                            unsafe {
                                libc::kill(spec.pid as libc::pid_t, libc::SIGTERM);
                            }
                            println!("🔄 回滾完成，已通知 Unix 服務管理器重啟 (PID: {}) 以載入舊版服務。", spec.pid);
                        }
                    }
                } else {
                    #[cfg(windows)]
                    if is_current_exe {
                        let original_version =
                            fs::read_to_string(install_dir.join("VERSION")).unwrap_or_default();
                        let _ =
                            schedule_windows_deferred_restart(spec, install_dir, &original_version);
                        continue;
                    }
                    let _ = restart_dashboard_instance(spec, install_dir);
                }
            }

            #[cfg(unix)]
            {
                let mut unix_pids_to_notify = std::collections::HashSet::new();
                for &pid in &process_plan.supervised_unix_pids {
                    unix_pids_to_notify.insert(pid);
                }
                for pid_file in &[
                    install_dir.join(".server.pid"),
                    crate::db::get_insights_dir().join(".server.pid"),
                ] {
                    if let Ok(content) = fs::read_to_string(pid_file) {
                        if let Ok(pid) = content.trim().parse::<u32>() {
                            if pid != std::process::id()
                                && is_process_alive(pid)
                                && is_process_supervised(pid, install_dir)
                            {
                                unix_pids_to_notify.insert(pid);
                            }
                        }
                    }
                }

                for pid in unix_pids_to_notify {
                    if is_process_alive(pid) {
                        log_update(
                            "INFO",
                            "ROLLBACK",
                            &format!(
                                "回滾完成，通知 Unix 服務管理器重啟以載入舊版服務 (PID: {pid})"
                            ),
                        );
                        unsafe {
                            libc::kill(pid as libc::pid_t, libc::SIGTERM);
                        }
                        println!(
                            "🔄 回滾完成，已通知 Unix 服務管理器重啟 (PID: {pid}) 以載入舊版服務。"
                        );
                    }
                }
            }

            #[cfg(windows)]
            {
                let has_supervised = process_plan.stopped_specs.iter().any(|s| s.is_supervised);
                if !has_supervised {
                    let _ = fs::remove_file(install_dir.join(".service_restart_pending"));
                }
            }

            return Err(format!(
                "更新檔案替換成功，但重啟新版服務失敗 ({combined})；已自動回滾至先前版本並恢復原服務。"
            ));
        }
    }

    // 4. 重啟確認成功後，才標記提交並清理備份目錄
    let _ = safe_write_file(&backup_dir.join(".committed"), b"committed");
    if backup_dir.exists() {
        if let Err(e) = fs::remove_dir_all(backup_dir) {
            log_update("WARN", "CLEANUP", &format!("清理備份目錄失敗: {e}"));
            let fallback_backup =
                install_dir.join(format!(".backup-old-{}", Utc::now().timestamp()));
            if let Err(re) = fs::rename(backup_dir, &fallback_backup) {
                log_update("WARN", "CLEANUP", &format!("備份目錄換名失敗: {re}"));
                eprintln!(
                    "⚠️ 更新已安裝完成，但備份目錄無法清理或換名 ({backup_dir:?}): {re}；請稍後手動移除。"
                );
            } else {
                log_update(
                    "INFO",
                    "CLEANUP",
                    &format!("備份目錄已安全移至 {fallback_backup:?}"),
                );
            }
        }
    }

    Ok(())
}

fn get_update_check_interval_secs() -> i64 {
    let (_, yaml_interval_days) = load_update_config();
    let default_hours = yaml_interval_days
        .map(|d| d.saturating_mul(24))
        .unwrap_or(DEFAULT_UPDATE_INTERVAL_HOURS);
    let interval_hours: i64 = std::env::var("TOKEN_USAGE_INSIGHTS_UPDATE_INTERVAL_HOURS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default_hours);

    let valid_hours = if interval_hours <= 0 {
        DEFAULT_UPDATE_INTERVAL_HOURS
    } else {
        interval_hours.min(87600) // 最多 10 年，防止溢位
    };
    valid_hours.saturating_mul(3600)
}

fn is_update_check_interval_elapsed() -> bool {
    let interval_secs = get_update_check_interval_secs();
    if let Ok(conn) = crate::db::get_db_conn() {
        if let Ok(Some(last_check_str)) = crate::db::get_system_metadata(&conn, LAST_CHECK_KEY) {
            if let Ok(last_check) = chrono::DateTime::parse_from_rfc3339(&last_check_str) {
                let elapsed_secs = Utc::now().timestamp() - last_check.timestamp();
                if elapsed_secs >= 0 && elapsed_secs < interval_secs {
                    return false;
                }
            }
        }
    }
    true
}

fn is_lock_conflict_error(err: &str) -> bool {
    err.contains("已有另一個更新程序正在執行中")
}

async fn wait_for_lock_release(install_dir: &Path, timeout: Duration) -> Result<(), String> {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if !UpdateLock::is_locked(install_dir) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Err(format!(
        "等待更新程序釋放鎖定逾時（超過 {} 秒）",
        timeout.as_secs()
    ))
}

pub async fn wait_for_parent_exit_if_requested() {
    if let Ok(val) = std::env::var("_TOKEN_USAGE_INSIGHTS_WAIT_PID") {
        std::env::remove_var("_TOKEN_USAGE_INSIGHTS_WAIT_PID");
        if let Ok(parent_pid) = val.trim().parse::<u32>() {
            let start = tokio::time::Instant::now();
            let timeout = Duration::from_secs(10);
            while is_process_alive(parent_pid) && start.elapsed() < timeout {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

fn get_target_exe(install_dir: &Path) -> PathBuf {
    let exec_name = if cfg!(windows) {
        format!("{APP_NAME}.exe")
    } else {
        APP_NAME.to_string()
    };
    install_dir.join(exec_name)
}

#[allow(dead_code)] // 於 Windows 服務重啟流程使用，並於跨平台單元測試驗證環境變數判定
fn is_windows_service_runner() -> bool {
    match std::env::var("TOKEN_USAGE_INSIGHTS_SERVICE") {
        Ok(val) => {
            let clean = val.trim();
            clean == "1" || clean.eq_ignore_ascii_case("true")
        }
        Err(_) => false,
    }
}

fn restart_current_process(exe_path: &Path, args: &[String]) -> ! {
    let exe = if exe_path.exists() {
        exe_path.to_path_buf()
    } else {
        std::env::current_exe().unwrap_or_else(|_| PathBuf::from(&args[0]))
    };

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let mut cmd = std::process::Command::new(exe);
        if args.len() > 1 {
            cmd.args(&args[1..]);
        }
        cmd.env("_TOKEN_USAGE_INSIGHTS_RESTARTED", "1");
        let err = cmd.exec();
        eprintln!("❌ 自動重啟進程失敗: {err}；請手動重新啟動程序。");
        log_update(
            "ERROR",
            "STARTUP_RESTART",
            &format!("自動重啟進程失敗: {err}"),
        );
        std::process::exit(1);
    }

    #[cfg(windows)]
    {
        if is_windows_service_runner() {
            if let Some(parent) = exe.parent() {
                let ready_marker = parent.join(".update_ready");
                let _ = safe_write_file(&ready_marker, b"ready");
            }
            log_update(
                "INFO",
                "STARTUP_RESTART",
                "以退出碼 75 請求 Windows 服務管理器重啟新版程序",
            );
            std::process::exit(75);
        } else {
            let mut cmd = std::process::Command::new(exe);
            if args.len() > 1 {
                cmd.args(&args[1..]);
            }
            cmd.env("_TOKEN_USAGE_INSIGHTS_RESTARTED", "1");
            let parent_pid = std::process::id();
            cmd.env("_TOKEN_USAGE_INSIGHTS_WAIT_PID", parent_pid.to_string());
            use std::os::windows::process::CommandExt;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
            cmd.creation_flags(CREATE_NEW_PROCESS_GROUP);
            match cmd.spawn() {
                Ok(_) => {
                    log_update(
                        "INFO",
                        "STARTUP_RESTART",
                        "已啟動新進程，目前進程安全退出以釋放連接埠與資源",
                    );
                    std::process::exit(0);
                }
                Err(err) => {
                    eprintln!("❌ 自動重啟進程失敗: {err}；請手動重新啟動程序。");
                    log_update("ERROR", "STARTUP_RESTART", &format!("重啟進程失敗: {err}"));
                    std::process::exit(1);
                }
            }
        }
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = exe;
        std::process::exit(0);
    }
}

fn is_auto_update_disabled(args: &[String]) -> bool {
    if args.iter().any(|arg| arg == "--no-auto-update") {
        return true;
    }
    if let Ok(val) = std::env::var("TOKEN_USAGE_INSIGHTS_AUTO_UPDATE") {
        let lower = val.trim().to_lowercase();
        if lower == "0" || lower == "false" || lower == "no" || lower == "off" {
            return true;
        }
        if lower == "1" || lower == "true" || lower == "yes" || lower == "on" {
            return false;
        }
    }
    // CI 環境自動防護：在 CI/自動化環境中預設停用背景自動更新，避免受遠端發行版本干擾
    if std::env::var("CI").is_ok() || std::env::var("GITHUB_ACTIONS").is_ok() {
        return true;
    }
    let (yaml_auto, _) = load_update_config();
    if yaml_auto == Some(false) {
        return true;
    }
    false
}

#[derive(Debug, PartialEq, Eq)]
enum RecoveryStatus {
    CleanedOrNoBackup,
    SuccessRestored,
    LockContended,
}

fn attempt_startup_recovery(install_dir: &Path, args: &[String]) -> RecoveryStatus {
    let backup_dir = install_dir.join(".backup");
    match fs::symlink_metadata(&backup_dir) {
        Ok(meta) => {
            if meta.file_type().is_symlink() || !meta.is_dir() {
                eprintln!(
                    "❌ 偵測到 .backup 為符號連結或非正規目錄 ({backup_dir:?})；救援中止以確保安全 (Fail-Closed)。"
                );
                log_update(
                    "ERROR",
                    "STARTUP_FATAL",
                    &format!("拒絕在符號連結或非正規目錄之 .backup 上執行救援 ({backup_dir:?})"),
                );
                std::process::exit(1);
            }
        }
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                return RecoveryStatus::CleanedOrNoBackup;
            }
            eprintln!("❌ 無法讀取 .backup 元資料 ({backup_dir:?}): {e}；救援中止以確保安全。");
            log_update(
                "ERROR",
                "STARTUP_FATAL",
                &format!("無法讀取 .backup 元資料: {e}"),
            );
            std::process::exit(1);
        }
    }

    // 優先嘗試取得更新鎖，若更新鎖被占用代表另有更新程序正在進行，絕不可在此時介入修改或刪除備份目錄
    let recovery_lock = match UpdateLock::try_acquire(install_dir) {
        Ok(l) => l,
        Err(e) => {
            log_update(
                "INFO",
                "STARTUP_RECOVERY",
                &format!("目前更新鎖被占用，暫緩救援: {e}"),
            );
            return RecoveryStatus::LockContended;
        }
    };

    // 若存在 .committed 標記，代表更新早已成功完成，僅備份目錄在最後刪除時中斷
    // 此時絕不能回滾新版本，直接清理備份目錄即可
    if backup_dir.join(".committed").exists() {
        log_update(
            "INFO",
            "STARTUP_RECOVERY",
            "先前更新已成功提交，清理殘留之備份目錄",
        );
        let _ = fs::remove_dir_all(&backup_dir);
        if backup_dir.exists() {
            let cleanup_name = format!(".backup-cleaned-{}", Utc::now().timestamp());
            let _ = fs::rename(&backup_dir, install_dir.join(&cleanup_name));
        }
        drop(recovery_lock);
        return RecoveryStatus::CleanedOrNoBackup;
    }

    if backup_dir.join(".rollback_failed").exists() {
        eprintln!(
            "❌ 偵測到先前更新回滾失敗標記 ({backup_dir:?})；為防止讀取損毀狀態，程序終止。請依備份手動還原。"
        );
        log_update("ERROR", "STARTUP_FATAL", "先前回滾失敗，程序終止");
        std::process::exit(1);
    }

    let manifest_path = backup_dir.join(".manifest");
    if !manifest_path.exists() {
        // 未含有效 manifest 的備份視為未完成之備份交易（此階段尚未替換任何安裝檔案）
        // 必須安全清理或換名，避免阻礙後續所有更新
        println!("⚠️ 偵測到未含有效清單的未完成備份交易目錄 ({backup_dir:?})，正在安全清理...");
        log_update(
            "WARN",
            "STARTUP_RECOVERY",
            "偵測到無 manifest 之未完成備份目錄，執行安全清理",
        );
        if let Err(e) = fs::remove_dir_all(&backup_dir) {
            let incomplete_name = format!(".backup-incomplete-{}", Utc::now().timestamp());
            let fallback = install_dir.join(&incomplete_name);
            if let Err(re) = fs::rename(&backup_dir, &fallback) {
                eprintln!(
                    "❌ 偵測到未完成交易備份目錄但無法清理或更名 ({backup_dir:?}): {e}; {re}；程序終止以保護狀態。"
                );
                log_update("ERROR", "STARTUP_FATAL", "清理未完成備份目錄失敗，程序終止");
                std::process::exit(1);
            }
        }
        drop(recovery_lock);
        return RecoveryStatus::CleanedOrNoBackup;
    }

    println!("⚠️ 正在自動救援還原至健全版本...");
    log_update("WARN", "STARTUP_RECOVERY", "取得更新鎖，執行自動救援還原");

    if let Err(e) = restore_from_backup(&backup_dir, install_dir) {
        eprintln!("❌ 自動救援還原失敗: {e}；程序終止以保護狀態。");
        log_update("ERROR", "STARTUP_FATAL", &format!("救援還原失敗: {e}"));
        std::process::exit(1);
    }

    // 還原成功：嚴禁在確認目錄清理或更名成功前先刪除 .manifest。
    // 若清理與更名都失敗，保留 .manifest 並終止程序，絕不留下失去 manifest 卻阻擋更新的孤立 .backup
    if let Err(e) = fs::remove_dir_all(&backup_dir) {
        log_update(
            "WARN",
            "STARTUP_RECOVERY",
            &format!("清理已還原備份目錄失敗: {e}，嘗試更名隔離"),
        );
        let restored_name = format!(".backup-restored-{}", Utc::now().timestamp());
        let fallback = install_dir.join(&restored_name);
        if let Err(re) = fs::rename(&backup_dir, &fallback) {
            eprintln!(
                "❌ 自動救援還原已完成，但無法清理或更名備份目錄 ({backup_dir:?}): {e}; {re}；程序終止以保留完整救援狀態。請手動清理該目錄。"
            );
            log_update(
                "ERROR",
                "STARTUP_FATAL",
                "已還原但備份目錄無法清理且無法更名，程序終止以保留狀態",
            );
            std::process::exit(1);
        }
    }

    // 釋放更新鎖後再執行重啟，避免子程序或被重啟程序繼承鎖檔案
    drop(recovery_lock);

    println!("✅ 已成功自動還原至健全版本，正在重新啟動 Token 戰情室...");
    log_update("INFO", "STARTUP_RECOVERY", "自動救援還原成功，重啟進程");
    let target_exe = get_target_exe(install_dir);
    restart_current_process(&target_exe, args);
    #[allow(unreachable_code)]
    RecoveryStatus::SuccessRestored
}

pub async fn perform_startup_recovery() {
    if std::env::var_os("_TOKEN_USAGE_INSIGHTS_RESTARTED").is_some() {
        return;
    }

    let args: Vec<String> = std::env::args().collect();
    let env_kind = detect_environment();

    if let EnvironmentKind::StandardInstalled { install_dir, .. } = &env_kind {
        let mut had_lock_wait = false;
        let start_time = tokio::time::Instant::now();
        let total_timeout = Duration::from_secs(STARTUP_AUTO_UPDATE_TOTAL_TIMEOUT_SECS);

        loop {
            if UpdateLock::is_locked(install_dir) {
                had_lock_wait = true;
                println!("⏳ 偵測到已有更新程序正在進行中，等待更新完成...");
                log_update("INFO", "STARTUP_WAIT", "偵測到進行中的更新鎖，等待其釋放");
                let elapsed = start_time.elapsed();
                if elapsed >= total_timeout {
                    eprintln!("❌ 等待更新程序超時；為防止讀取不一致檔案，程序終止。");
                    log_update("ERROR", "STARTUP_LOCK", "等待更新鎖超時");
                    std::process::exit(1);
                }
                let remaining = total_timeout - elapsed;
                if let Err(e) = wait_for_lock_release(install_dir, remaining).await {
                    eprintln!("❌ 等待更新程序超時: {e}；為防止讀取不一致檔案，程序終止。");
                    log_update("ERROR", "STARTUP_LOCK", &format!("等待更新鎖超時: {e}"));
                    std::process::exit(1);
                }
            }

            match attempt_startup_recovery(install_dir, &args) {
                RecoveryStatus::SuccessRestored => return,
                RecoveryStatus::CleanedOrNoBackup => {
                    if had_lock_wait {
                        if UpdateLock::is_locked(install_dir) {
                            // 鎖釋放後又有另一更新程序搶先加鎖，繼續等待
                            continue;
                        }
                        let installed = get_installed_version(install_dir);
                        if parse_semver(&installed) > parse_semver(env!("CARGO_PKG_VERSION")) {
                            println!(
                                "🔄 更新程序已完成，正在重新啟動 Token 戰情室至新版 v{installed}..."
                            );
                            log_update(
                                "INFO",
                                "STARTUP_RESTART",
                                &format!("其他程序更新完成，重啟至 v{installed}"),
                            );
                            let target_exe = get_target_exe(install_dir);
                            restart_current_process(&target_exe, &args);
                        }
                    }
                    break;
                }
                RecoveryStatus::LockContended => {
                    // 鎖定在救援檢查前已被其他程序占用，回到迴圈等待釋放
                    had_lock_wait = true;
                    continue;
                }
            }
        }
    }
}

/// 於伺服器綁定 TCP 監聽與服務靜態檔案前執行啟動自動更新檢查；
/// 若發現新版本並完成替換，將重啟至新版並退出目前進程，避免在對外提供 HTTP 服務期間覆寫磁碟檔案導致檔案鎖定或資源不一致衝突。
pub async fn run_startup_auto_update() {
    run_startup_auto_update_impl().await;
}

#[allow(dead_code)] // 供相容性保留或外部呼叫
pub fn spawn_background_auto_update() {
    tokio::spawn(async {
        run_startup_auto_update_impl().await;
    });
}

async fn run_startup_auto_update_impl() {
    if std::env::var_os("_TOKEN_USAGE_INSIGHTS_RESTARTED").is_some() {
        return;
    }

    let args: Vec<String> = std::env::args().collect();
    if is_auto_update_disabled(&args) {
        return;
    }

    let env_kind = detect_environment();
    if matches!(env_kind, EnvironmentKind::Npm { .. }) {
        if !is_update_check_interval_elapsed() {
            return;
        }
        if let Ok(release) = fetch_release_with_logging(None, STARTUP_CHECK_TIMEOUT_SECS).await {
            if let Ok(conn) = crate::db::get_db_conn() {
                let now_str = Utc::now().to_rfc3339();
                let _ = crate::db::set_system_metadata(&conn, LAST_CHECK_KEY, &now_str);
            }
            let current_version = env!("CARGO_PKG_VERSION");
            if is_newer_version(&release.tag_name, current_version) {
                log_update(
                    "INFO",
                    "STARTUP_CHECK",
                    &format!("npm 環境偵測到新版本 {}", release.tag_name),
                );
                println!(
                    "💡 發現新版本 {}！您可以執行 npx token-usage-insights@latest 啟動最新版本。",
                    release.tag_name
                );
            }
        }
        return;
    }

    let install_dir = match &env_kind {
        EnvironmentKind::StandardInstalled { install_dir, .. } => install_dir.clone(),
        _ => return,
    };

    if !is_update_check_interval_elapsed() {
        return;
    }

    let release = match fetch_release_with_logging(None, STARTUP_CHECK_TIMEOUT_SECS).await {
        Ok(r) => r,
        Err(e) => {
            log_update("WARN", "STARTUP_CHECK", &format!("啟動更新檢查略過: {e}"));
            return;
        }
    };

    let current_version_str = get_installed_version(&install_dir);
    let current_version = current_version_str.as_str();
    if !is_newer_version(&release.tag_name, current_version) {
        if let Ok(conn) = crate::db::get_db_conn() {
            let now_str = Utc::now().to_rfc3339();
            let _ = crate::db::set_system_metadata(&conn, LAST_CHECK_KEY, &now_str);
        }
        return;
    }

    println!(
        "🚀 發現新版本 {}（目前為 v{}），正在自動更新...",
        release.tag_name, current_version
    );
    log_update(
        "INFO",
        "STARTUP_CHECK",
        &format!("觸發啟動自動更新至 {}", release.tag_name),
    );

    let update_opts = UpdateOptions {
        check_only: false,
        force: false,
        target_version: None,
        prefetched_release: Some(release.clone()),
    };

    let update_result = run_update(update_opts).await;

    match update_result {
        Ok(()) => {
            if let Ok(conn) = crate::db::get_db_conn() {
                let now_str = Utc::now().to_rfc3339();
                let _ = crate::db::set_system_metadata(&conn, LAST_CHECK_KEY, &now_str);
            }

            let installed = get_installed_version(&install_dir);
            if parse_semver(&installed) > parse_semver(env!("CARGO_PKG_VERSION")) {
                println!("🔄 更新完成，正在自動重啟 Token 戰情室至新版 v{installed}...");
                log_update(
                    "INFO",
                    "STARTUP_RESTART",
                    &format!("更新完成，重啟至 v{installed}"),
                );
                let target_exe = get_target_exe(&install_dir);
                restart_current_process(&target_exe, &args);
            } else {
                log_update("INFO", "STARTUP_CHECK", "目前進程與磁碟版本相符，無需重啟");
            }
        }
        Err(e) => {
            let err_msg = e.to_string();
            if is_lock_conflict_error(&err_msg) {
                println!("⏳ 偵測到已有更新程序正在進行中，等待更新完成...");
                log_update("INFO", "STARTUP_WAIT", "遇到更新鎖競爭，等待另一程序完成");
                let start_time = tokio::time::Instant::now();
                let total_timeout = Duration::from_secs(STARTUP_AUTO_UPDATE_TOTAL_TIMEOUT_SECS);
                loop {
                    if UpdateLock::is_locked(&install_dir) {
                        let elapsed = start_time.elapsed();
                        if elapsed >= total_timeout {
                            // 背景自動更新任務中遇到鎖競爭超時，記錄警告並放棄本輪更新；
                            // 絕不在背景任務中呼叫 process::exit，以免強制終止正在服務的看板程序
                            eprintln!("⚠️ 等待更新程序超時；本輪背景更新略過，看板服務繼續運行。");
                            log_update(
                                "WARN",
                                "STARTUP_LOCK",
                                "背景更新等待更新鎖超時，放棄本輪更新，不終止程序",
                            );
                            return;
                        }
                        let remaining = total_timeout - elapsed;
                        if let Err(wait_err) = wait_for_lock_release(&install_dir, remaining).await
                        {
                            eprintln!(
                                "⚠️ 等待更新程序超時: {wait_err}；本輪背景更新略過，看板服務繼續運行。"
                            );
                            log_update(
                                "WARN",
                                "STARTUP_LOCK",
                                &format!(
                                    "背景更新等待更新鎖超時: {wait_err}，放棄本輪更新，不終止程序"
                                ),
                            );
                            return;
                        }
                    }

                    match attempt_startup_recovery(&install_dir, &args) {
                        RecoveryStatus::SuccessRestored => return,
                        RecoveryStatus::CleanedOrNoBackup => {
                            if UpdateLock::is_locked(&install_dir) {
                                continue;
                            }
                            let installed = get_installed_version(&install_dir);
                            if parse_semver(&installed) != parse_semver(env!("CARGO_PKG_VERSION")) {
                                println!(
                                    "🔄 更新已由另一程序完成，正在重新啟動 Token 戰情室至新版 v{installed}..."
                                );
                                log_update(
                                    "INFO",
                                    "STARTUP_RESTART",
                                    &format!("另一程序更新完成，重啟至 v{installed}"),
                                );
                                let target_exe = get_target_exe(&install_dir);
                                restart_current_process(&target_exe, &args);
                            }
                            break;
                        }
                        RecoveryStatus::LockContended => continue,
                    }
                }
            } else {
                attempt_startup_recovery(&install_dir, &args);
                eprintln!("⚠️ 自動更新失敗: {err_msg}，將繼續以現有健全版本啟動服務。");
                log_update(
                    "WARN",
                    "STARTUP_UPDATE",
                    &format!("自動更新失敗: {err_msg}"),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_parsing_and_newer_comparison() {
        assert_eq!(parse_semver("v0.9.5"), Some((0, 9, 5)));
        assert_eq!(parse_semver("0.9.5"), Some((0, 9, 5)));
        assert_eq!(parse_semver("v1.2.3-beta.1"), Some((1, 2, 3)));

        assert!(is_newer_version("v0.9.6", "0.9.5"));
        assert!(is_newer_version("v1.0.0", "0.9.5"));
        assert!(is_newer_version("v0.10.0", "0.9.9"));
        assert!(!is_newer_version("v0.9.5", "0.9.5"));
        assert!(!is_newer_version("v0.9.4", "0.9.5"));
        assert!(!is_newer_version("v0.8.99", "0.9.5"));
    }

    #[test]
    fn parse_checksum_extracts_correct_hash() {
        let sums = r#"
4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945  ./token-usage-insights-v0.9.5-aarch64-apple-darwin.tar.gz
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 *token-usage-insights-v0.9.5-x86_64-apple-darwin.tar.gz
a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2  token-usage-insights-v0.9.5-x86_64-pc-windows-msvc.zip
"#;
        assert_eq!(
            parse_checksum(
                sums,
                "token-usage-insights-v0.9.5-aarch64-apple-darwin.tar.gz"
            ),
            Some("4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945".to_string())
        );
        assert_eq!(
            parse_checksum(
                sums,
                "token-usage-insights-v0.9.5-x86_64-apple-darwin.tar.gz"
            ),
            Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_string())
        );
        assert_eq!(
            parse_checksum(
                sums,
                "token-usage-insights-v0.9.5-x86_64-pc-windows-msvc.zip"
            ),
            Some("a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2".to_string())
        );
        assert_eq!(parse_checksum(sums, "non-existent-file"), None);
    }

    #[test]
    fn verify_sha256_matches_content() {
        let data = b"hello token-usage-insights";
        let mut hasher = Sha256::new();
        hasher.update(data);
        let expected = hex::encode(hasher.finalize());

        assert!(verify_sha256(data, &expected));
        assert!(!verify_sha256(b"wrong data", &expected));
    }

    #[test]
    fn system_metadata_get_and_set() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::init_db(&conn).unwrap();

        assert_eq!(
            crate::db::get_system_metadata(&conn, "test_key").unwrap(),
            None
        );

        crate::db::set_system_metadata(&conn, "test_key", "value_1").unwrap();
        assert_eq!(
            crate::db::get_system_metadata(&conn, "test_key").unwrap(),
            Some("value_1".to_string())
        );

        crate::db::set_system_metadata(&conn, "test_key", "value_2").unwrap();
        assert_eq!(
            crate::db::get_system_metadata(&conn, "test_key").unwrap(),
            Some("value_2".to_string())
        );
    }

    #[test]
    fn archive_filename_formats_correctly() {
        let target = "aarch64-apple-darwin";
        let filename_v = archive_filename("v0.9.5", target);
        let filename_raw = archive_filename("0.9.5", target);
        #[cfg(windows)]
        {
            assert_eq!(
                filename_v,
                "token-usage-insights-v0.9.5-aarch64-apple-darwin.zip"
            );
            assert_eq!(
                filename_raw,
                "token-usage-insights-0.9.5-aarch64-apple-darwin.zip"
            );
        }
        #[cfg(not(windows))]
        {
            assert_eq!(
                filename_v,
                "token-usage-insights-v0.9.5-aarch64-apple-darwin.tar.gz"
            );
            assert_eq!(
                filename_raw,
                "token-usage-insights-0.9.5-aarch64-apple-darwin.tar.gz"
            );
        }
    }

    #[test]
    fn parse_config_yaml_extracts_options() {
        let yaml = r#"
# Token 戰情室設定檔
auto_update: false
update_check_interval: 3
"#;
        let (auto, interval) = parse_config_yaml(yaml);
        assert_eq!(auto, Some(false));
        assert_eq!(interval, Some(3));

        let yaml2 = r#"
auto_update: "true"
update_check_interval: '7'
"#;
        let (auto2, interval2) = parse_config_yaml(yaml2);
        assert_eq!(auto2, Some(true));
        assert_eq!(interval2, Some(7));

        let yaml3 = r#"
auto_update: false # disable auto updates
update_check_interval: 5 # check every 5 days
"#;
        let (auto3, interval3) = parse_config_yaml(yaml3);
        assert_eq!(auto3, Some(false));
        assert_eq!(interval3, Some(5));
    }

    #[test]
    fn backup_and_restore_cycle() {
        let temp = std::env::temp_dir().join(format!(
            "test-backup-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let install_dir = temp.join("install");
        let backup_dir = temp.join("backup");
        fs::create_dir_all(&install_dir).unwrap();

        fs::write(install_dir.join("VERSION"), "v0.9.5").unwrap();
        fs::write(install_dir.join("pricing.csv"), "model,price").unwrap();
        fs::create_dir_all(install_dir.join("static")).unwrap();
        fs::write(
            install_dir.join("static").join("index.html"),
            "<h1>Test</h1>",
        )
        .unwrap();

        // Backup
        backup_installation(&install_dir, &backup_dir).unwrap();
        assert!(backup_dir.join("VERSION").exists());
        assert!(backup_dir.join("pricing.csv").exists());
        assert!(backup_dir.join("static").join("index.html").exists());
        assert!(backup_dir.join(".manifest").exists());

        // Re-run backup should fail because backup_dir already exists to protect recovery state
        assert!(backup_installation(&install_dir, &backup_dir).is_err());

        // Corrupt install_dir and simulate adding a new file not present in original backup
        fs::write(install_dir.join("VERSION"), "corrupted").unwrap();
        fs::remove_file(install_dir.join("pricing.csv")).unwrap();
        fs::write(install_dir.join("install.sh"), "#!/bin/sh\n").unwrap();

        // Restore
        restore_from_backup(&backup_dir, &install_dir).unwrap();
        assert_eq!(
            fs::read_to_string(install_dir.join("VERSION")).unwrap(),
            "v0.9.5"
        );
        assert!(install_dir.join("pricing.csv").exists());
        // The newly added install.sh was not in the backup manifest and should be removed
        assert!(!install_dir.join("install.sh").exists());

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn process_alive_check_identifies_current_process() {
        let current_pid = std::process::id();
        assert!(is_process_alive(current_pid));
    }

    #[test]
    fn is_windows_service_runner_checks_truthy_values() {
        std::env::set_var("TOKEN_USAGE_INSIGHTS_SERVICE", "1");
        assert!(is_windows_service_runner());

        std::env::set_var("TOKEN_USAGE_INSIGHTS_SERVICE", "true");
        assert!(is_windows_service_runner());

        std::env::set_var("TOKEN_USAGE_INSIGHTS_SERVICE", "TRUE");
        assert!(is_windows_service_runner());

        std::env::set_var("TOKEN_USAGE_INSIGHTS_SERVICE", "0");
        assert!(!is_windows_service_runner());

        std::env::set_var("TOKEN_USAGE_INSIGHTS_SERVICE", "false");
        assert!(!is_windows_service_runner());

        std::env::remove_var("TOKEN_USAGE_INSIGHTS_SERVICE");
        assert!(!is_windows_service_runner());
    }

    #[test]
    fn process_cmdline_current_process_returns_valid_argv() {
        let current_pid = std::process::id();
        let cmdline = get_process_cmdline(current_pid);
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            assert!(
                cmdline.is_some(),
                "Linux and macOS should reliably retrieve process cmdline"
            );
            let args = cmdline.unwrap();
            assert!(!args.is_empty(), "cmdline should have at least argv[0]");
            let expected_args: Vec<String> = std::env::args().collect();
            assert_eq!(args, expected_args, "cmdline should match std::env::args()");
        }
    }

    #[test]
    fn update_check_interval_handles_extremes_and_saturation() {
        // 預設間隔 (24 小時 -> 86400 秒)
        let default_secs = get_update_check_interval_secs();
        assert!(default_secs > 0);
        assert!(default_secs <= 87600 * 3600);
    }

    #[tokio::test]
    async fn update_lock_prevents_concurrent_access() {
        let temp = std::env::temp_dir().join(format!(
            "test-lock-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        fs::create_dir_all(&temp).unwrap();

        assert!(!UpdateLock::is_locked(&temp));

        let lock1 = UpdateLock::try_acquire(&temp);
        assert!(lock1.is_ok());
        assert!(UpdateLock::is_locked(&temp));

        let lock2 = UpdateLock::try_acquire(&temp);
        assert!(lock2.is_err());
        let err_msg = lock2.unwrap_err();
        assert!(is_lock_conflict_error(&err_msg));

        drop(lock1);
        assert!(!UpdateLock::is_locked(&temp));

        assert!(wait_for_lock_release(&temp, Duration::from_millis(500))
            .await
            .is_ok());

        let lock3 = UpdateLock::try_acquire(&temp);
        assert!(lock3.is_ok());
        assert!(UpdateLock::is_locked(&temp));

        drop(lock3);
        assert!(!UpdateLock::is_locked(&temp));
        let _ = fs::remove_dir_all(&temp);
    }

    #[cfg(unix)]
    #[test]
    fn update_lock_rejects_symlink() {
        let temp = std::env::temp_dir().join(format!(
            "test-lock-symlink-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        fs::create_dir_all(&temp).unwrap();

        let outside_file = temp.join("external_lock_target.txt");
        fs::write(&outside_file, "external file content").unwrap();

        let lock_symlink = temp.join(".update.lock");
        std::os::unix::fs::symlink(&outside_file, &lock_symlink).unwrap();

        // 檢查 is_locked 應回傳 true (fail-closed)
        assert!(UpdateLock::is_locked(&temp));

        // 嘗試獲取鎖應被拒絕
        let lock_res = UpdateLock::try_acquire(&temp);
        assert!(lock_res.is_err());
        let err_msg = lock_res.unwrap_err();
        assert!(
            err_msg.contains("符號連結") || err_msg.contains("非正規檔案"),
            "應明確拒絕符號連結鎖檔: {err_msg}"
        );

        // 外部檔案不應被寫入 pid
        let outside_content = fs::read_to_string(&outside_file).unwrap();
        assert_eq!(outside_content, "external file content");

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn apply_installation_with_rollback_success_and_restore_on_error() {
        let temp = std::env::temp_dir().join(format!(
            "test-install-orchestration-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let install_dir = temp.join("install");
        let release_root = temp.join("release");
        let bad_release = temp.join("bad_release");
        let backup_dir = install_dir.join(".backup");

        fs::create_dir_all(&install_dir).unwrap();
        fs::create_dir_all(&release_root).unwrap();
        fs::create_dir_all(&bad_release).unwrap();

        let exec_name = if cfg!(windows) {
            format!("{APP_NAME}.exe")
        } else {
            APP_NAME.to_string()
        };

        // Existing installation v0.9.5
        fs::write(install_dir.join(&exec_name), "old binary").unwrap();
        fs::write(install_dir.join("VERSION"), "v0.9.5").unwrap();
        fs::write(install_dir.join("pricing.csv"), "old pricing").unwrap();
        fs::create_dir_all(install_dir.join("static")).unwrap();
        fs::write(install_dir.join("static").join("index.html"), "old html").unwrap();
        fs::create_dir_all(install_dir.join("scripts")).unwrap();
        fs::create_dir_all(install_dir.join("shell")).unwrap();
        fs::write(install_dir.join("install.sh"), "#!/bin/sh").unwrap();
        fs::write(install_dir.join("install.ps1"), "# powershell").unwrap();

        // Valid new release v0.9.6
        fs::write(release_root.join(&exec_name), "new binary").unwrap();
        fs::write(release_root.join("VERSION"), "v0.9.6").unwrap();
        fs::write(release_root.join("pricing.csv"), "new pricing").unwrap();
        fs::create_dir_all(release_root.join("static")).unwrap();
        fs::write(release_root.join("static").join("index.html"), "new html").unwrap();
        fs::create_dir_all(release_root.join("scripts")).unwrap();
        fs::create_dir_all(release_root.join("shell")).unwrap();
        fs::write(release_root.join("install.sh"), "#!/bin/sh v2").unwrap();
        fs::write(release_root.join("install.ps1"), "# powershell v2").unwrap();

        // 1. Success case
        let result = apply_installation_with_rollback(&release_root, &install_dir, &backup_dir);
        assert!(
            result.is_ok(),
            "apply_installation_with_rollback failed: {:?}",
            result
        );
        assert_eq!(
            fs::read_to_string(install_dir.join("VERSION")).unwrap(),
            "v0.9.6"
        );
        assert_eq!(
            fs::read_to_string(install_dir.join("pricing.csv")).unwrap(),
            "new pricing"
        );
        assert_eq!(
            fs::read_to_string(install_dir.join(&exec_name)).unwrap(),
            "new binary"
        );
        assert_eq!(
            fs::read_to_string(install_dir.join("install.sh")).unwrap(),
            "#!/bin/sh v2"
        );
        assert!(
            !backup_dir.exists(),
            "backup_dir should be removed after success"
        );

        // 2. Failure case: bad release where the executable is a directory instead of a file
        // This causes fs::copy(&src_exe, &target_exe) to fail during installation, triggering rollback.
        fs::create_dir_all(bad_release.join(&exec_name)).unwrap();
        fs::write(bad_release.join("VERSION"), "v0.9.7-broken").unwrap();
        fs::create_dir_all(bad_release.join("static")).unwrap();
        fs::write(bad_release.join("static").join("index.html"), "broken html").unwrap();

        let fail_result = apply_installation_with_rollback(&bad_release, &install_dir, &backup_dir);
        assert!(
            fail_result.is_err(),
            "expected installation to fail with directory as binary"
        );

        // Verify that rollback restored install_dir to v0.9.6 state
        assert_eq!(
            fs::read_to_string(install_dir.join("VERSION")).unwrap(),
            "v0.9.6"
        );
        assert_eq!(
            fs::read_to_string(install_dir.join("pricing.csv")).unwrap(),
            "new pricing"
        );
        assert_eq!(
            fs::read_to_string(install_dir.join(&exec_name)).unwrap(),
            "new binary"
        );
        assert_eq!(
            fs::read_to_string(install_dir.join("static").join("index.html")).unwrap(),
            "new html"
        );
        assert_eq!(
            fs::read_to_string(install_dir.join("install.sh")).unwrap(),
            "#!/bin/sh v2"
        );
        assert!(
            !backup_dir.exists(),
            "backup_dir should be removed after successful rollback"
        );

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn process_exe_path_and_matches_install_dir() {
        let my_pid = std::process::id();
        let exe_path = get_process_exe_path(my_pid);
        assert!(
            exe_path.is_some(),
            "should be able to get current process exe path"
        );
        let path = exe_path.unwrap();
        assert!(path.exists(), "process exe path should exist: {:?}", path);

        let parent = path.parent().unwrap();
        assert!(matches_install_dir(&path, parent));
        assert!(!matches_install_dir(
            &path,
            Path::new("/nonexistent/directory")
        ));
    }

    #[test]
    fn server_pid_guard_lifecycle() {
        let temp = std::env::temp_dir().join(format!(
            "pid-test-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        fs::create_dir_all(&temp).unwrap();
        let pid_path = temp.join(".server.pid");

        {
            let _guard = ServerPidGuard {
                paths: vec![pid_path.clone()],
            };
            safe_write_file(&pid_path, std::process::id().to_string().as_bytes()).unwrap();
            assert!(pid_path.exists());
        }

        assert!(
            !pid_path.exists(),
            ".server.pid should be removed when guard is dropped"
        );
        let _ = fs::remove_dir_all(&temp);
    }

    #[cfg(unix)]
    #[test]
    fn safe_write_file_replaces_symlink_without_modifying_target() {
        let temp = std::env::temp_dir().join(format!(
            "safe-write-symlink-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        fs::create_dir_all(&temp).unwrap();

        let sensitive_target = temp.join("sensitive.txt");
        fs::write(&sensitive_target, "sensitive content").unwrap();

        let link_path = temp.join(".server.pid");
        std::os::unix::fs::symlink(&sensitive_target, &link_path).unwrap();

        assert!(safe_write_file(&link_path, b"12345").is_ok());

        // 敏感目標檔案內容絕不能被竄改
        let target_content = fs::read_to_string(&sensitive_target).unwrap();
        assert_eq!(target_content, "sensitive content");

        // 符號連結應已被替換為正規檔案，且內容為新寫入之內容
        let meta = fs::symlink_metadata(&link_path).unwrap();
        assert!(!meta.file_type().is_symlink());
        assert_eq!(fs::read_to_string(&link_path).unwrap(), "12345");

        let _ = fs::remove_dir_all(&temp);
    }

    #[cfg(unix)]
    #[test]
    fn restore_from_backup_rejects_symlink_backup_dir() {
        let temp = std::env::temp_dir().join(format!(
            "test-restore-symlink-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let install_dir = temp.join("install");
        let outside_dir = temp.join("outside");
        let backup_link = install_dir.join(".backup");

        fs::create_dir_all(&install_dir).unwrap();
        fs::create_dir_all(&outside_dir).unwrap();
        fs::write(outside_dir.join(".manifest"), "malicious.txt").unwrap();
        fs::write(outside_dir.join("malicious.txt"), "evil").unwrap();

        std::os::unix::fs::symlink(&outside_dir, &backup_link).unwrap();

        let res = restore_from_backup(&backup_link, &install_dir);
        assert!(res.is_err(), "應拒絕符號連結之備份目錄");
        let err = res.unwrap_err();
        assert!(err.contains("符號連結") || err.contains("非正規目錄"));
        assert!(!install_dir.join("malicious.txt").exists());

        let _ = fs::remove_dir_all(&temp);
    }

    #[cfg(unix)]
    #[test]
    fn restore_from_backup_rejects_symlink_entries_inside_backup() {
        let temp = std::env::temp_dir().join(format!(
            "test-restore-entry-symlink-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let install_dir = temp.join("install");
        let backup_dir = temp.join("backup");
        let outside_dir = temp.join("outside");

        fs::create_dir_all(&install_dir).unwrap();
        fs::create_dir_all(&backup_dir).unwrap();
        fs::create_dir_all(&outside_dir).unwrap();

        let outside_secret = outside_dir.join("secret.txt");
        fs::write(&outside_secret, "sensitive data").unwrap();
        fs::write(backup_dir.join(".manifest"), "secret.txt").unwrap();

        let symlink_entry = backup_dir.join("secret.txt");
        std::os::unix::fs::symlink(&outside_secret, &symlink_entry).unwrap();

        let res = restore_from_backup(&backup_dir, &install_dir);
        assert!(res.is_err(), "應拒絕還原備份目錄中的符號連結項目");
        let err = res.unwrap_err();
        assert!(err.contains("符號連結項目"));
        assert!(!install_dir.join("secret.txt").exists());

        let _ = fs::remove_dir_all(&temp);
    }

    #[cfg(unix)]
    #[test]
    fn copy_dir_recursive_rejects_nested_symlink() {
        let temp = std::env::temp_dir().join(format!(
            "test-copy-dir-symlink-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let src_dir = temp.join("src");
        let dst_dir = temp.join("dst");
        let outside_dir = temp.join("outside");

        fs::create_dir_all(&src_dir).unwrap();
        fs::create_dir_all(&outside_dir).unwrap();

        let outside_secret = outside_dir.join("secret.txt");
        fs::write(&outside_secret, "sensitive data").unwrap();

        let symlink_entry = src_dir.join("link_to_secret.txt");
        std::os::unix::fs::symlink(&outside_secret, &symlink_entry).unwrap();

        let res = copy_dir_recursive(&src_dir, &dst_dir);
        assert!(
            res.is_err(),
            "copy_dir_recursive 應拒絕複製內含符號連結之項目"
        );
        let err = res.unwrap_err();
        assert!(err.contains("符號連結項目"));
        assert!(!dst_dir.join("link_to_secret.txt").exists());

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn safe_replace_file_overwrites_existing_file() {
        let temp = std::env::temp_dir().join(format!(
            "safe-replace-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        fs::create_dir_all(&temp).unwrap();
        let src = temp.join("source.txt");
        let dst = temp.join("destination.txt");

        fs::write(&src, "new content").unwrap();
        fs::write(&dst, "old content").unwrap();

        assert!(safe_replace_file(&src, &dst).is_ok());
        assert_eq!(fs::read_to_string(&dst).unwrap(), "new content");

        #[cfg(unix)]
        {
            let outside = temp.join("outside.txt");
            let symlink_dst = temp.join("link_dst.txt");
            fs::write(&outside, "sensitive outside content").unwrap();
            std::os::unix::fs::symlink(&outside, &symlink_dst).unwrap();

            assert!(safe_replace_file(&src, &symlink_dst).is_ok());
            // Symlink should be replaced with regular file, and outside file untouched
            let meta = fs::symlink_metadata(&symlink_dst).unwrap();
            assert!(!meta.file_type().is_symlink());
            assert_eq!(fs::read_to_string(&symlink_dst).unwrap(), "new content");
            assert_eq!(
                fs::read_to_string(&outside).unwrap(),
                "sensitive outside content"
            );
        }

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn incomplete_backup_without_manifest_is_cleaned_up_on_recovery() {
        let temp = std::env::temp_dir().join(format!(
            "incomplete-backup-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let install_dir = temp.join("install");
        let backup_dir = install_dir.join(".backup");
        fs::create_dir_all(&backup_dir).unwrap();
        fs::write(backup_dir.join("partial_file"), "partial").unwrap();

        assert!(backup_dir.exists());
        assert!(!backup_dir.join(".manifest").exists());

        attempt_startup_recovery(&install_dir, &[]);

        assert!(
            !backup_dir.exists(),
            "incomplete backup without manifest should be safely removed during recovery"
        );

        let _ = fs::remove_dir_all(&temp);
    }

    #[cfg(unix)]
    #[test]
    fn install_marker_symlink_is_rejected() {
        let temp = std::env::temp_dir().join(format!(
            "marker-symlink-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        fs::create_dir_all(&temp).unwrap();
        let target = temp.join("target_marker");
        let marker = temp.join(".install_marker");

        fs::write(&target, "token-usage-insights:installed").unwrap();
        std::os::unix::fs::symlink(&target, &marker).unwrap();

        let meta = fs::symlink_metadata(&marker).unwrap();
        let is_valid_marker = meta.is_file() && !meta.file_type().is_symlink();
        assert!(
            !is_valid_marker,
            "symlinked marker should not be treated as a valid regular marker file"
        );

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn committed_marker_prevents_erroneous_rollback_on_recovery() {
        let temp = std::env::temp_dir().join(format!(
            "committed-marker-test-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let install_dir = temp.join("install");
        let backup_dir = install_dir.join(".backup");
        fs::create_dir_all(&install_dir).unwrap();
        fs::create_dir_all(&backup_dir).unwrap();

        fs::write(install_dir.join("VERSION"), "v0.9.6").unwrap();
        fs::write(backup_dir.join("VERSION"), "v0.9.5").unwrap();
        fs::write(backup_dir.join(".manifest"), "VERSION").unwrap();
        fs::write(backup_dir.join(".committed"), "committed").unwrap();

        attempt_startup_recovery(&install_dir, &[]);

        // 因為存在 .committed 標記，新版絕不可被錯誤回滾至舊版 v0.9.5
        assert_eq!(
            fs::read_to_string(install_dir.join("VERSION")).unwrap(),
            "v0.9.6"
        );
        assert!(!backup_dir.exists(), ".backup 應在確認已提交後被安全清理");

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn is_cli_subcommand_identifies_cli_commands() {
        assert!(is_cli_subcommand("export"));
        assert!(is_cli_subcommand("export-all"));
        assert!(is_cli_subcommand("import"));
        assert!(is_cli_subcommand("update"));
        assert!(is_cli_subcommand("--update"));
        assert!(is_cli_subcommand("-u"));
        assert!(is_cli_subcommand("--help"));
        assert!(is_cli_subcommand("-h"));
        assert!(is_cli_subcommand("--version"));
        assert!(is_cli_subcommand("-V"));
        assert!(is_cli_subcommand("completion"));

        assert!(!is_cli_subcommand("--no-auto-update"));
        assert!(!is_cli_subcommand("--port"));
        assert!(!is_cli_subcommand("3003"));
    }

    #[test]
    fn auto_update_disabled_in_ci_and_flags() {
        assert!(is_auto_update_disabled(&["--no-auto-update".to_string()]));
        assert!(is_auto_update_disabled(&[
            "app".to_string(),
            "--no-auto-update".to_string()
        ]));
    }

    #[test]
    fn dashboard_process_plan_and_spec_properties() {
        let plan = DashboardProcessPlan {
            stopped_specs: vec![
                StoppedProcessSpec {
                    pid: 1234,
                    is_supervised: false,
                    supervisor_pid: None,
                    is_server: false,
                    exe_path: PathBuf::from("/opt/token-usage-insights/token-usage-insights"),
                    args: Some(vec![
                        "/opt/token-usage-insights/token-usage-insights".to_string(),
                        "--no-auto-update".to_string(),
                    ]),
                    envs: vec![("PORT".to_string(), "3003".to_string())],
                    cwd: Some(PathBuf::from("/opt/token-usage-insights")),
                },
                StoppedProcessSpec {
                    pid: 5678,
                    is_supervised: true,
                    supervisor_pid: Some(4321),
                    is_server: true,
                    exe_path: PathBuf::from("/opt/token-usage-insights/token-usage-insights"),
                    args: None,
                    envs: vec![],
                    cwd: None,
                },
            ],
            #[cfg(unix)]
            supervised_unix_pids: vec![9999],
        };

        assert_eq!(plan.stopped_specs.len(), 2);
        assert!(!plan.stopped_specs[0].is_supervised);
        assert_eq!(plan.stopped_specs[0].supervisor_pid, None);
        assert!(!plan.stopped_specs[0].is_server);
        assert_eq!(plan.stopped_specs[0].pid, 1234);
        assert_eq!(
            plan.stopped_specs[0].args.as_ref().unwrap(),
            &[
                "/opt/token-usage-insights/token-usage-insights",
                "--no-auto-update"
            ]
        );
        assert_eq!(
            plan.stopped_specs[0].envs,
            vec![("PORT".to_string(), "3003".to_string())]
        );
        assert!(plan.stopped_specs[1].is_supervised);
        assert_eq!(plan.stopped_specs[1].supervisor_pid, Some(4321));
        assert!(plan.stopped_specs[1].is_server);
        assert_eq!(plan.stopped_specs[1].pid, 5678);

        #[cfg(unix)]
        assert_eq!(plan.supervised_unix_pids, vec![9999]);
    }

    #[test]
    fn update_error_display_and_conversion() {
        let safe = UpdateError::SafeRejection("測試拒絕".to_string());
        assert_eq!(safe.to_string(), "測試拒絕");

        let fail: UpdateError = "測試失敗".to_string().into();
        assert_eq!(fail, UpdateError::Failure("測試失敗".to_string()));
        assert_eq!(fail.to_string(), "測試失敗");
    }

    #[test]
    fn restart_dashboard_instance_validations() {
        let temp = std::env::temp_dir().join(format!(
            "restart-validations-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let _ = fs::create_dir_all(&temp);
        let dummy_exe = temp.join(APP_NAME);
        let _ = fs::write(&dummy_exe, b"");

        // 1. args 為 None 且非已知服務時應拒絕重啟
        let spec_no_args = StoppedProcessSpec {
            pid: 1111,
            is_supervised: false,
            supervisor_pid: None,
            is_server: false,
            exe_path: dummy_exe.clone(),
            args: None,
            envs: vec![],
            cwd: Some(temp.clone()),
        };
        let res_no_args = restart_dashboard_instance(&spec_no_args, &temp);
        assert!(res_no_args.is_err());
        assert!(res_no_args
            .unwrap_err()
            .contains("無法可靠取得先前進程之命令列參數"));

        // 1b. args 為 None 但 is_server 為 true 時，應採用安全回退預設參數重啟（不因缺少 args 而在參數校驗階段拒絕）
        let spec_server_fallback = StoppedProcessSpec {
            pid: 1112,
            is_supervised: false,
            supervisor_pid: None,
            is_server: true,
            exe_path: dummy_exe.clone(),
            args: None,
            envs: vec![],
            cwd: Some(temp.clone()),
        };
        let res_server = restart_dashboard_instance(&spec_server_fallback, &temp);
        if let Err(e) = res_server {
            assert!(
                !e.contains("無法可靠取得先前進程之命令列參數"),
                "已知服務在 args 為 None 時應採用回退參數，實際錯誤: {e}"
            );
        }

        // 2. args 包含 CLI subcommand 時應拒絕重啟
        let spec_subcommand = StoppedProcessSpec {
            pid: 2222,
            is_supervised: false,
            supervisor_pid: None,
            is_server: false,
            exe_path: dummy_exe.clone(),
            args: Some(vec![APP_NAME.to_string(), "export-all".to_string()]),
            envs: vec![],
            cwd: Some(temp.clone()),
        };
        let res_subcommand = restart_dashboard_instance(&spec_subcommand, &temp);
        assert!(res_subcommand.is_err());
        assert!(res_subcommand
            .unwrap_err()
            .contains("先前進程包含非看板 CLI 子命令"));

        // 3. cwd 為 None 時應拒絕重啟以防組態偏離
        let spec_no_cwd = StoppedProcessSpec {
            pid: 3333,
            is_supervised: false,
            supervisor_pid: None,
            is_server: false,
            exe_path: dummy_exe.clone(),
            args: Some(vec![APP_NAME.to_string()]),
            envs: vec![],
            cwd: None,
        };
        let res_no_cwd = restart_dashboard_instance(&spec_no_cwd, &temp);
        assert!(res_no_cwd.is_err());
        assert!(res_no_cwd
            .unwrap_err()
            .contains("無法可靠取得先前進程之工作目錄"));

        // 4. 執行檔不存在時應報錯
        let empty_dir = temp.join("empty_dir");
        let spec_missing_exe = StoppedProcessSpec {
            pid: 4444,
            is_supervised: false,
            supervisor_pid: None,
            is_server: false,
            exe_path: empty_dir.join("nonexistent_exe"),
            args: Some(vec![APP_NAME.to_string()]),
            envs: vec![],
            cwd: Some(temp.clone()),
        };
        let res_missing = restart_dashboard_instance(&spec_missing_exe, &empty_dir);
        assert!(res_missing.is_err());
        assert!(res_missing.unwrap_err().contains("找不到執行檔"));

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn configure_windows_runner_command_preserves_envs_and_arguments() {
        let install_dir = PathBuf::from("C:\\Program Files\\TokenUsageInsights");
        let runner_script = install_dir.join("scripts").join("run-service.ps1");
        let envs = vec![
            ("HOST".to_string(), "127.0.0.1".to_string()),
            ("PORT".to_string(), "8080".to_string()),
            ("INSIGHTS_DIR".to_string(), "C:\\data\\insights".to_string()),
            (
                "TOKEN_USAGE_INSIGHTS_AUTO_UPDATE".to_string(),
                "1".to_string(),
            ),
            (
                "TOKEN_USAGE_INSIGHTS_UPDATE_INTERVAL_HOURS".to_string(),
                "12".to_string(),
            ),
        ];

        let spec = StoppedProcessSpec {
            pid: 1234,
            exe_path: install_dir.join("token-usage-insights.exe"),
            cwd: Some(PathBuf::from("C:\\working")),
            args: Some(vec!["token-usage-insights".to_string()]),
            envs,
            is_server: true,
            is_supervised: true,
            supervisor_pid: None,
        };

        let mut cmd = std::process::Command::new("powershell.exe");
        configure_windows_runner_command(&mut cmd, &runner_script, &install_dir, &spec);

        let args: Vec<String> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().to_string())
            .collect();
        assert!(args.contains(&"-File".to_string()));
        assert!(args.contains(&"-InstallDir".to_string()));
        assert!(args.contains(&"-HostAddress".to_string()));
        assert!(args.contains(&"127.0.0.1".to_string()));
        assert!(args.contains(&"-Port".to_string()));
        assert!(args.contains(&"8080".to_string()));
        assert!(args.contains(&"-AutoUpdate".to_string()));
        assert!(args.contains(&"1".to_string()));
        assert!(args.contains(&"-UpdateIntervalHours".to_string()));
        assert!(args.contains(&"12".to_string()));

        let env_map: std::collections::HashMap<String, Option<String>> = cmd
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().to_string(),
                    v.map(|s| s.to_string_lossy().to_string()),
                )
            })
            .collect();
        assert_eq!(
            env_map.get("INSIGHTS_DIR").and_then(|v| v.as_deref()),
            Some("C:\\data\\insights")
        );
        assert_eq!(cmd.get_current_dir(), Some(Path::new("C:\\working")));
    }

    #[test]
    fn relevant_env_vars_contains_all_critical_keys() {
        let expected = [
            "PORT",
            "HOST",
            "INSIGHTS_DIR",
            "TOKEN_USAGE_INSIGHTS_INSTALL_DIR",
            "TOKEN_USAGE_INSIGHTS_AUTO_UPDATE",
            "TOKEN_USAGE_INSIGHTS_UPDATE_INTERVAL_HOURS",
            "TOKEN_USAGE_INSIGHTS_SERVICE",
            "CORS_ALLOWED_ORIGINS",
            "ANTIGRAVITY_DIR",
            "COPILOT_DIR",
            "COPILOT_APP_DIR",
            "CODEX_DIR",
            "CLAUDE_DIR",
            "CURSOR_DIR",
            "CURSOR_STATE_DB",
            "GROK_DIR",
            "PI_DIR",
            "OMP_DIR",
            "MUSE_DIR",
            "VSCODE_DIR",
            "VSCODE_USER_DATA_DIR",
            "VSCODE_PORTABLE_DATA_DIR",
        ];
        for key in expected {
            assert!(
                RELEVANT_ENV_VARS.contains(&key),
                "RELEVANT_ENV_VARS 應包含關鍵環境變數 {key}"
            );
        }
    }

    #[test]
    fn get_installed_version_reads_version_file_or_fallback() {
        let temp = std::env::temp_dir().join(format!(
            "test-inst-ver-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let _ = fs::create_dir_all(&temp);

        // 1. VERSION 檔案不存在時回退至 CARGO_PKG_VERSION
        assert_eq!(get_installed_version(&temp), env!("CARGO_PKG_VERSION"));

        // 2. VERSION 檔案包含 'v' 前綴
        fs::write(temp.join("VERSION"), "v1.2.3\n").unwrap();
        assert_eq!(get_installed_version(&temp), "1.2.3");

        // 3. VERSION 檔案不含 'v' 前綴
        fs::write(temp.join("VERSION"), "2.0.0").unwrap();
        assert_eq!(get_installed_version(&temp), "2.0.0");

        // 4. VERSION 檔案空白時回退
        fs::write(temp.join("VERSION"), "  \n").unwrap();
        assert_eq!(get_installed_version(&temp), env!("CARGO_PKG_VERSION"));

        let _ = fs::remove_dir_all(&temp);
    }

    static ENV_TEST_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[tokio::test]
    async fn wait_for_parent_exit_returns_immediately_when_no_env() {
        let _guard = ENV_TEST_MUTEX.lock().await;
        std::env::remove_var("_TOKEN_USAGE_INSIGHTS_WAIT_PID");
        wait_for_parent_exit_if_requested().await;
        assert!(std::env::var("_TOKEN_USAGE_INSIGHTS_WAIT_PID").is_err());
    }

    #[tokio::test]
    async fn wait_for_parent_exit_cleans_env_when_pid_not_alive() {
        let _guard = ENV_TEST_MUTEX.lock().await;
        // 使用一個極不可能存活的 PID
        std::env::set_var("_TOKEN_USAGE_INSIGHTS_WAIT_PID", "999999999");
        wait_for_parent_exit_if_requested().await;
        assert!(std::env::var("_TOKEN_USAGE_INSIGHTS_WAIT_PID").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn backup_installation_rejects_top_level_symlink() {
        let temp = std::env::temp_dir().join(format!(
            "backup-symlink-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let install_dir = temp.join("install");
        let external_dir = temp.join("external");
        let backup_dir = temp.join(".backup");
        fs::create_dir_all(&install_dir).unwrap();
        fs::create_dir_all(&external_dir).unwrap();

        // 建立指向外部目錄的 top-level symlink "static"
        std::os::unix::fs::symlink(&external_dir, install_dir.join("static")).unwrap();

        let res = backup_installation(&install_dir, &backup_dir);
        assert!(res.is_err());
        let err = res.unwrap_err();
        assert!(err.contains("符號連結"), "應拒絕包含符號連結的項目: {err}");

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn detect_environment_prioritizes_git_or_dev_over_markers_and_assets() {
        let temp = std::env::temp_dir().join(format!(
            "detect-env-test-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let git_root = temp.join("repo");
        let bin_dir = git_root.join("target").join("release");
        fs::create_dir_all(git_root.join(".git")).unwrap();
        fs::create_dir_all(&bin_dir).unwrap();

        // 在 bin_dir 放置 .install_marker 以及所有已安裝資產
        fs::write(
            bin_dir.join(".install_marker"),
            "token-usage-insights:installed",
        )
        .unwrap();
        fs::write(bin_dir.join("pricing.csv"), "model,cost").unwrap();
        fs::write(bin_dir.join("VERSION"), "0.9.5").unwrap();
        fs::create_dir_all(bin_dir.join("static")).unwrap();
        fs::write(bin_dir.join("static").join("index.html"), "<html></html>").unwrap();

        let dummy_exe = bin_dir.join(APP_NAME);
        let env_kind = detect_environment_with_path(&dummy_exe);
        assert!(
            matches!(env_kind, EnvironmentKind::GitOrDev { .. }),
            "即使殘留 .install_marker 或安裝資產，在 git 儲存庫內執行應優先判定為 GitOrDev"
        );

        // 在獨立安裝目錄中，且無任何 git 或 cargo ancestor，應判定為 StandardInstalled
        let standalone_install = temp.join("standalone_app");
        fs::create_dir_all(&standalone_install).unwrap();
        fs::write(
            standalone_install.join(".install_marker"),
            "token-usage-insights:installed",
        )
        .unwrap();
        let standalone_exe = standalone_install.join(APP_NAME);
        let standalone_env = detect_environment_with_path(&standalone_exe);
        assert!(
            matches!(standalone_env, EnvironmentKind::StandardInstalled { .. }),
            "無 git/cargo ancestor 且含有效 marker 應判定為 StandardInstalled"
        );

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn process_supervisor_pid_default_behavior() {
        let my_pid = std::process::id();
        #[cfg(not(windows))]
        {
            assert_eq!(get_process_supervisor_pid(my_pid), None);
        }
        #[cfg(windows)]
        {
            // Windows 環境下，非由 run-service.ps1 啟動之測試進程應回傳 None
            let sup = get_process_supervisor_pid(my_pid);
            let _ = sup;
        }
    }

    #[test]
    fn extract_archive_rejects_unsafe_tar_paths() {
        let temp = std::env::temp_dir().join(format!(
            "extract-unsafe-tar-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let _ = fs::create_dir_all(&temp);

        // 建立包含不安全 RootDir / ParentDir 路徑的 tar.gz
        let tar_path = temp.join("unsafe.tar.gz");
        let file = fs::File::create(&tar_path).unwrap();
        let gz = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut builder = tar::Builder::new(gz);

        let data = b"hello unsafe";
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();

        // 1. 測試根路徑名稱 "/escaped.txt"
        header.as_mut_bytes()[..12].copy_from_slice(b"/escaped.txt");
        header.set_cksum();
        builder.append(&header, &data[..]).unwrap();

        // 2. 測試 Windows 反斜線根路徑 "\\escaped.txt"
        let mut header2 = tar::Header::new_gnu();
        header2.set_size(data.len() as u64);
        header2.set_mode(0o644);
        header2.as_mut_bytes()[..12].copy_from_slice(b"\\escaped.txt");
        header2.set_cksum();
        builder.append(&header2, &data[..]).unwrap();

        // 3. 測試父目錄穿越路徑 "../escaped.txt"
        let mut header3 = tar::Header::new_gnu();
        header3.set_size(data.len() as u64);
        header3.set_mode(0o644);
        header3.as_mut_bytes()[..14].copy_from_slice(b"../escaped.txt");
        header3.set_cksum();
        builder.append(&header3, &data[..]).unwrap();

        builder.into_inner().unwrap().finish().unwrap();

        let extract_dest = temp.join("dest");
        let res = extract_archive(&tar_path, &extract_dest, false);
        assert!(res.is_err());
        let err = res.unwrap_err();
        assert!(
            err.contains("不安全") || err.contains("超出"),
            "應拒絕不安全的 tar 路徑: {err}"
        );

        let _ = fs::remove_dir_all(&temp);
    }

    struct MockHttpReleaseServer {
        addr: std::net::SocketAddr,
        shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    }

    impl MockHttpReleaseServer {
        async fn start(
            archive_name: String,
            archive_bytes: Vec<u8>,
            checksums_content: String,
        ) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();

            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                loop {
                    tokio::select! {
                        _ = &mut shutdown_rx => break,
                        accept_res = listener.accept() => {
                            if let Ok((mut stream, _)) = accept_res {
                                let mut req_buf = [0u8; 2048];
                                let n = stream.read(&mut req_buf).await.unwrap_or(0);
                                let req_str = String::from_utf8_lossy(&req_buf[..n]);
                                let first_line = req_str.lines().next().unwrap_or("");
                                let path = first_line.split_whitespace().nth(1).unwrap_or("");

                                let (status, content_type, body): (&str, &str, Vec<u8>) = if path == format!("/{}", archive_name) {
                                    ("200 OK", "application/octet-stream", archive_bytes.clone())
                                } else if path == "/SHA256SUMS" {
                                    ("200 OK", "text/plain", checksums_content.as_bytes().to_vec())
                                } else {
                                    ("404 Not Found", "text/plain", b"not found".to_vec())
                                };

                                let response = format!(
                                    "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                    body.len()
                                );
                                let _ = stream.write_all(response.as_bytes()).await;
                                let _ = stream.write_all(&body).await;
                                let _ = stream.flush().await;
                            }
                        }
                    }
                }
            });

            MockHttpReleaseServer {
                addr,
                shutdown_tx: Some(shutdown_tx),
            }
        }
    }

    impl Drop for MockHttpReleaseServer {
        fn drop(&mut self) {
            if let Some(tx) = self.shutdown_tx.take() {
                let _ = tx.send(());
            }
        }
    }

    fn create_test_release_archive(version: &str, target: &str) -> (String, Vec<u8>) {
        use std::io::Write;

        let archive_name = archive_filename(version, target);
        let prefix = format!("{APP_NAME}-v{version}-{target}");
        let mut files: Vec<(&str, &[u8], u32)> = vec![
            ("VERSION", version.as_bytes(), 0o644),
            ("pricing.csv", b"model,input,output\ntest,1,2\n", 0o644),
            ("README.md", b"# Updated", 0o644),
            ("LICENSE", b"MIT", 0o644),
            ("static/index.html", b"<h1>Dashboard</h1>", 0o644),
            ("scripts/run-service.ps1", b"# runner", 0o644),
            ("shell/token-usage-insights.service", b"# service", 0o644),
            ("install.sh", b"#!/bin/sh\nexit 0\n", 0o755),
            ("install.ps1", b"# installer\n", 0o644),
        ];
        let exec_name = if target.contains("windows") {
            format!("{APP_NAME}.exe")
        } else {
            APP_NAME.to_string()
        };
        files.push((&exec_name, b"binary_content_v2", 0o755));

        if archive_name.ends_with(".zip") {
            let mut buf = std::io::Cursor::new(Vec::new());
            {
                let mut zip = zip::ZipWriter::new(&mut buf);
                let options = zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated);
                for (name, content, _) in files {
                    let entry_name = format!("{prefix}/{name}");
                    zip.start_file(entry_name, options).unwrap();
                    zip.write_all(content).unwrap();
                }
                zip.finish().unwrap();
            }
            (archive_name, buf.into_inner())
        } else {
            let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            {
                let mut builder = tar::Builder::new(&mut gz);
                for (name, content, mode) in files {
                    let entry_name = format!("{prefix}/{name}");
                    let mut header = tar::Header::new_gnu();
                    header.set_size(content.len() as u64);
                    header.set_mode(mode);
                    header.set_cksum();
                    builder
                        .append_data(&mut header, entry_name, content)
                        .unwrap();
                }
                builder.finish().unwrap();
            }
            let bytes = gz.finish().unwrap();
            (archive_name, bytes)
        }
    }

    #[tokio::test]
    async fn e2e_run_update_success() {
        let temp = std::env::temp_dir().join(format!(
            "test-e2e-success-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let install_dir = temp.join("install");
        fs::create_dir_all(&install_dir).unwrap();
        fs::write(install_dir.join("VERSION"), "0.9.0\n").unwrap();
        fs::write(
            install_dir.join(".install_marker"),
            "token-usage-insights:installed",
        )
        .unwrap();
        fs::write(
            install_dir.join("pricing.csv"),
            "model,input,output\nold,1,2\n",
        )
        .unwrap();

        let target = current_target_triple().expect("Target triple must be supported");
        let new_version = "0.9.1";
        let (archive_name, archive_bytes) = create_test_release_archive(new_version, target);
        let mut hasher = Sha256::new();
        hasher.update(&archive_bytes);
        let checksum = hex::encode(hasher.finalize());
        let checksums = format!("{checksum}  {archive_name}\n");

        let server =
            MockHttpReleaseServer::start(archive_name.clone(), archive_bytes, checksums).await;

        let release = GitHubRelease {
            tag_name: format!("v{new_version}"),
            assets: vec![
                GitHubAsset {
                    name: archive_name.clone(),
                    browser_download_url: format!("http://{}/{}", server.addr, archive_name),
                },
                GitHubAsset {
                    name: "SHA256SUMS".to_string(),
                    browser_download_url: format!("http://{}/SHA256SUMS", server.addr),
                },
            ],
        };

        let options = UpdateOptions {
            check_only: false,
            force: true,
            target_version: Some(new_version.to_string()),
            prefetched_release: Some(release),
        };

        let res = run_update_in_dir(&install_dir, options).await;
        assert!(
            res.is_ok(),
            "run_update_in_dir should succeed: {:?}",
            res.err()
        );

        // 驗證新版檔案已確實安裝就緒
        let updated_version = fs::read_to_string(install_dir.join("VERSION")).unwrap();
        assert_eq!(updated_version.trim(), new_version);
        assert!(install_dir.join("static").join("index.html").exists());
        assert!(install_dir.join("pricing.csv").exists());
        assert!(install_dir.join(".install_marker").exists());
        assert!(
            !install_dir.join(".backup").exists(),
            ".backup 應於成功後清除"
        );

        let _ = fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn e2e_run_update_checksum_mismatch_triggers_rollback() {
        let temp = std::env::temp_dir().join(format!(
            "test-e2e-rollback-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let install_dir = temp.join("install");
        fs::create_dir_all(&install_dir).unwrap();
        fs::write(install_dir.join("VERSION"), "0.9.0\n").unwrap();
        fs::write(
            install_dir.join(".install_marker"),
            "token-usage-insights:installed",
        )
        .unwrap();
        fs::write(
            install_dir.join("pricing.csv"),
            "model,input,output\noriginal,1,2\n",
        )
        .unwrap();

        let target = current_target_triple().expect("Target triple must be supported");
        let new_version = "0.9.1";
        let (archive_name, archive_bytes) = create_test_release_archive(new_version, target);
        // 刻意給予不相符之 SHA256 校驗碼觸發失敗
        let wrong_checksum = "0000000000000000000000000000000000000000000000000000000000000000";
        let checksums = format!("{wrong_checksum}  {archive_name}\n");

        let server =
            MockHttpReleaseServer::start(archive_name.clone(), archive_bytes, checksums).await;

        let release = GitHubRelease {
            tag_name: format!("v{new_version}"),
            assets: vec![
                GitHubAsset {
                    name: archive_name.clone(),
                    browser_download_url: format!("http://{}/{}", server.addr, archive_name),
                },
                GitHubAsset {
                    name: "SHA256SUMS".to_string(),
                    browser_download_url: format!("http://{}/SHA256SUMS", server.addr),
                },
            ],
        };

        let options = UpdateOptions {
            check_only: false,
            force: true,
            target_version: Some(new_version.to_string()),
            prefetched_release: Some(release),
        };

        let res = run_update_in_dir(&install_dir, options).await;
        assert!(res.is_err(), "校驗和不符應回傳錯誤");
        let err = res.unwrap_err();
        assert!(
            err.to_string().contains("校驗") || err.to_string().contains("SHA256"),
            "錯誤訊息應提及校驗和不符: {err}"
        );

        // 驗證原版本檔案完好如初
        let current_version = fs::read_to_string(install_dir.join("VERSION")).unwrap();
        assert_eq!(current_version.trim(), "0.9.0");
        let pricing = fs::read_to_string(install_dir.join("pricing.csv")).unwrap();
        assert!(pricing.contains("original"));

        let _ = fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn e2e_run_update_lock_contention() {
        let temp = std::env::temp_dir().join(format!(
            "test-e2e-lock-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let install_dir = temp.join("install");
        fs::create_dir_all(&install_dir).unwrap();
        fs::write(install_dir.join("VERSION"), "0.9.0\n").unwrap();
        fs::write(
            install_dir.join(".install_marker"),
            "token-usage-insights:installed",
        )
        .unwrap();

        // 在測試中先取得更新鎖
        let lock = UpdateLock::try_acquire(&install_dir).expect("初始鎖定應成功");

        let options = UpdateOptions {
            check_only: false,
            force: true,
            target_version: Some("0.9.1".to_string()),
            prefetched_release: None,
        };

        let res = run_update_in_dir(&install_dir, options).await;
        assert!(res.is_err(), "更新鎖被占用時應立即失敗");
        let err = res.unwrap_err();
        assert!(is_lock_conflict_error(&err.to_string()));

        drop(lock);
        let _ = fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn e2e_run_update_service_handoff_and_ready_protocol() {
        let temp = std::env::temp_dir().join(format!(
            "test-e2e-handoff-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let install_dir = temp.join("install");
        fs::create_dir_all(&install_dir).unwrap();
        fs::write(install_dir.join("VERSION"), "0.9.0\n").unwrap();
        fs::write(
            install_dir.join(".install_marker"),
            "token-usage-insights:installed",
        )
        .unwrap();

        let target = current_target_triple().expect("Target triple must be supported");
        let new_version = "0.9.1";
        let (archive_name, archive_bytes) = create_test_release_archive(new_version, target);
        let mut hasher = Sha256::new();
        hasher.update(&archive_bytes);
        let checksum = hex::encode(hasher.finalize());
        let checksums = format!("{checksum}  {archive_name}\n");

        let server =
            MockHttpReleaseServer::start(archive_name.clone(), archive_bytes, checksums).await;

        let release = GitHubRelease {
            tag_name: format!("v{new_version}"),
            assets: vec![
                GitHubAsset {
                    name: archive_name.clone(),
                    browser_download_url: format!("http://{}/{}", server.addr, archive_name),
                },
                GitHubAsset {
                    name: "SHA256SUMS".to_string(),
                    browser_download_url: format!("http://{}/SHA256SUMS", server.addr),
                },
            ],
        };

        let options = UpdateOptions {
            check_only: false,
            force: true,
            target_version: Some(new_version.to_string()),
            prefetched_release: Some(release),
        };

        let res = run_update_in_dir(&install_dir, options).await;
        assert!(res.is_ok());

        // 驗證更新就緒標記已產出，確保 Windows 服務守護進程能收到就緒協商信號
        let ready_file = install_dir.join(".update_ready");
        assert!(ready_file.exists(), "更新成功後必須產生 .update_ready 標記");
        let ready_content = fs::read_to_string(&ready_file).unwrap();
        assert_eq!(ready_content.trim(), "ready");

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn validate_download_url_enforces_https_or_loopback() {
        assert!(validate_download_url(
            "https://github.com/doggy8088/TokenUsageInsights/releases/download/v0.9.1/file.zip"
        )
        .is_ok());
        assert!(validate_download_url(
            "https://objects.githubusercontent.com/github-production-release-asset-2e65be/file.zip"
        )
        .is_ok());
        assert!(validate_download_url("http://127.0.0.1:8080/archive.zip").is_ok());
        assert!(validate_download_url("http://localhost:3000/archive.zip").is_ok());
        assert!(validate_download_url("not a url").is_err());
        assert!(validate_download_url("ftp://example.com/file.zip").is_err());
    }

    #[test]
    fn build_windows_deferred_restart_script_generates_correct_powershell() {
        let spec = StoppedProcessSpec {
            pid: 12345,
            is_supervised: false,
            supervisor_pid: None,
            is_server: true,
            exe_path: PathBuf::from("C:\\test\\bin\\token-usage-insights.exe"),
            args: Some(vec![
                "token-usage-insights.exe".to_string(),
                "--no-auto-update".to_string(),
            ]),
            envs: vec![
                ("PORT".to_string(), "3003".to_string()),
                ("HOST".to_string(), "127.0.0.1".to_string()),
                ("INSIGHTS_DIR".to_string(), "C:\\data".to_string()),
            ],
            cwd: Some(PathBuf::from("C:\\test")),
        };

        let script = build_windows_deferred_restart_script(
            9999,
            Path::new("C:\\test"),
            Path::new("C:\\test\\bin\\token-usage-insights.exe"),
            "v0.9.6",
            &spec,
        )
        .expect("產生移交重啟腳本應成功");

        // 驗證腳本包含更新程序 PID 等待
        assert!(script.contains("$updaterPid = 9999;"));
        assert!(
            script.contains("while (Get-Process -Id $updaterPid -ErrorAction SilentlyContinue)")
        );

        // 驗證腳本包含更新鎖釋放等待
        assert!(script.contains("Join-Path $installDir '.update.lock'"));
        assert!(script.contains("$stream.Lock(0, 1)"));

        // 驗證腳本包含 self_replace 暫存置換檔清理等待
        assert!(script.contains("*.__temp__.exe"));
        assert!(script.contains("*.__relocated__.exe"));

        // 驗證腳本包含 --version 執行與精確版本驗證
        assert!(script.contains("& $exePath --version"));
        assert!(script.contains("$expectedVersion = '0.9.6';"));
        assert!(script.contains("$actualVer -eq $expectedVersion"));

        // 驗證版本不符時拒絕啟動並記錄錯誤
        assert!(script.contains("移交守護進程驗證新版執行檔版本失敗"));
        assert!(script.contains("中止啟動以防載入舊版"));

        // 驗證版本相符時才以原參數啟動
        assert!(script.contains("移交守護進程已確認新版執行檔版本"));
        assert!(script.contains("Start-Process -FilePath $exePath"));
    }

    #[tokio::test]
    async fn run_startup_auto_update_exits_early_when_restarted_flag_set() {
        std::env::set_var("_TOKEN_USAGE_INSIGHTS_RESTARTED", "1");
        run_startup_auto_update().await;
        std::env::remove_var("_TOKEN_USAGE_INSIGHTS_RESTARTED");
    }
}
