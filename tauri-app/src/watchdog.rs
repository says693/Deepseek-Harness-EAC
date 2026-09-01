//! 看门狗：同一 exe 以 `--dsh-watchdog` 参数再入（替代 Electron 版的
//! watchdog.js 独立 Node 进程，免额外运行时）。轮询父 PID：
//!   cleanExit=true → 用户主动退出/更新，安静退出；
//!   有更新实例接管 → 旧看门狗退出；
//!   否则视为意外崩溃 → 拉起应用（10 分钟内最多 5 次，15s 宽限）。

use serde_json::Value;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const MAX_RESTARTS: u32 = 5;
const WINDOW_MS: u64 = 10 * 60 * 1000;
const GRACE_MS: u64 = 15 * 1000;
const POLL_MS: u64 = 2000;

struct WatchArgs {
    pid: u32,
    exe: String,
    state: PathBuf,
    log: PathBuf,
}

fn arg_of(name: &str) -> Option<String> {
    let prefix = format!("--{}=", name);
    std::env::args()
        .find(|a| a.starts_with(&prefix))
        .map(|a| a[prefix.len()..].to_string())
}

fn wlog(log_file: &PathBuf, msg: &str) {
    use std::io::Write;
    let line = format!("[{}] {}\n", crate::logging::now_local_string(), msg);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_file)
    {
        let _ = f.write_all(line.as_bytes());
    }
}

/// main.rs 入口检测到 --dsh-watchdog 时进入此循环，不再返回。
pub fn run_as_watchdog() -> ! {
    let Some(args) = (|| {
        Some(WatchArgs {
            pid: arg_of("pid")?.parse().ok()?,
            exe: arg_of("exe")?,
            state: PathBuf::from(arg_of("state")?),
            log: PathBuf::from(arg_of("log")?),
        })
    })() else {
        std::process::exit(0);
    };
    watchdog_loop(args)
}

fn watchdog_loop(args: WatchArgs) -> ! {
    let mut restart_count: u32 = 0;
    let mut window_start = Instant::now();
    let mut last_launch: Option<Instant> = None;

    wlog(
        &args.log,
        &format!("watchdog: started watching={} exe={}", args.pid, args.exe),
    );
    loop {
        std::thread::sleep(Duration::from_millis(POLL_MS));
        // 干净退出标记检查提到最前，不依赖 alive 探测：主壳进程对象在
        // exit 后可能被 OpenProcess 短暂/滞留判定为存活，导致 alive
        // 短路使看门狗永远读不到 cleanExit → 空转残留（真机验证发现）。
        {
            let state: Value = std::fs::read_to_string(&args.state)
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or(Value::Null);
            if state.get("cleanExit").and_then(|v| v.as_bool()) == Some(true) {
                wlog(&args.log, "watchdog: clean exit marker found, exiting");
                std::process::exit(0);
            }
        }
        if crate::procwin::alive(args.pid) {
            continue;
        }
        let state: Value = std::fs::read_to_string(&args.state)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(Value::Null);
        let newer_pid = state.get("pid").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        if newer_pid != 0 && newer_pid != args.pid && crate::procwin::alive(newer_pid) {
            wlog(
                &args.log,
                &format!(
                    "watchdog: newer instance pid={} is running, exiting",
                    newer_pid
                ),
            );
            std::process::exit(0);
        }
        wlog(
            &args.log,
            &format!(
                "watchdog: watched pid={} is gone without clean-exit marker",
                args.pid
            ),
        );
        // 拉起护栏：10 分钟窗口内最多 5 次；每次拉起间隔 ≥15s。
        if let Some(t) = last_launch {
            if t.elapsed() < Duration::from_millis(GRACE_MS) {
                continue;
            }
        }
        if window_start.elapsed() > Duration::from_millis(WINDOW_MS) {
            window_start = Instant::now();
            restart_count = 0;
        }
        if restart_count >= MAX_RESTARTS {
            wlog(
                &args.log,
                &format!(
                    "watchdog: too many restarts ({}/{}), giving up",
                    restart_count, MAX_RESTARTS
                ),
            );
            std::process::exit(0);
        }
        if args.exe.is_empty() || !std::path::Path::new(&args.exe).exists() {
            wlog(
                &args.log,
                &format!("watchdog: app exe missing: {}", args.exe),
            );
            std::process::exit(0);
        }
        restart_count += 1;
        last_launch = Some(Instant::now());
        wlog(
            &args.log,
            &format!(
                "watchdog: relaunching app (attempt {}/{})",
                restart_count, MAX_RESTARTS
            ),
        );
        let cwd = std::path::Path::new(&args.exe)
            .parent()
            .map(|p| p.to_path_buf());
        let _ = crate::procwin::spawn_detached(&args.exe, &[], cwd.as_deref());
    }
}

// ---------------------------------------------------------------------------
// run-state.json 维护（主进程侧）
// ---------------------------------------------------------------------------

pub fn write_run_state(
    state_file: &std::path::Path,
    pid: u32,
    version: &str,
    extra: Option<Value>,
) {
    let mut doc = serde_json::json!({
        "pid": pid,
        "cleanExit": false,
        "startedAt": iso_now(),
        "version": version,
    });
    if let (Some(obj), Some(extra)) = (
        doc.as_object_mut(),
        extra.and_then(|e| e.as_object().cloned()),
    ) {
        for (k, v) in extra {
            obj.insert(k, v);
        }
    }
    let _ = std::fs::write(state_file, serde_json::to_string(&doc).unwrap_or_default());
}

pub fn mark_clean_exit(state_file: &std::path::Path) -> bool {
    let mut doc: Value = std::fs::read_to_string(state_file)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if let Some(obj) = doc.as_object_mut() {
        obj.insert("cleanExit".into(), Value::Bool(true));
        obj.insert("endedAt".into(), Value::String(iso_now()));
    }
    std::fs::write(state_file, serde_json::to_string(&doc).unwrap_or_default()).is_ok()
}

/// 上次运行未正常退出时返回其状态（用于「已自动恢复」通知）。
pub fn detect_unclean_previous_run(state_file: &std::path::Path, own_pid: u32) -> Option<Value> {
    let prev: Value = std::fs::read_to_string(state_file)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())?;
    let clean = prev
        .get("cleanExit")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let pid = prev.get("pid").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    if !clean && pid != 0 && pid != own_pid {
        Some(prev)
    } else {
        None
    }
}

/// ISO8601 UTC 时间戳（无外部依赖）。
pub fn iso_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() as i64;
    let millis = now.subsec_millis();
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let (h, mi, s) = (
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
        (rem % 60) as u32,
    );
    // civil_from_days
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        y, m, d, h, mi, s, millis
    )
}
