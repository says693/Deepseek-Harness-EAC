//! desktop.log 写入：行格式与 Electron 版一致（本地时间 + 显式时区偏移，
//! issue #4），供「复制日志」与排障使用。单写者：sidecar 的日志经 RPC 事件
//! 汇入，避免多进程交错。

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct Logger {
    file: Mutex<Option<File>>,
}

impl Logger {
    pub fn open(logs_dir: &Path) -> Logger {
        let _ = std::fs::create_dir_all(logs_dir);
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(logs_dir.join("desktop.log"))
            .ok();
        Logger {
            file: Mutex::new(file),
        }
    }

    pub fn log(&self, tag: &str, msg: &str) {
        let line = format!("[{}] [{}] {}\n", now_local_string(), tag, msg);
        if let Ok(mut guard) = self.file.lock() {
            if let Some(f) = guard.as_mut() {
                let _ = f.write_all(line.as_bytes());
            }
        }
        if std::env::var("DSH_DESKTOP_DEBUG").is_ok() {
            print!("{}", line);
        }
    }
}

/// 本地时间 + 时区偏移，例如 `2026-08-21 12:34:56.789 UTC+08:00`。
/// 不引 chrono，用偏移量手算（只用于日志展示）。
pub fn now_local_string() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let total_secs = now.as_secs() as i64;
    let millis = now.subsec_millis();
    let off_secs = local_utc_offset_secs();
    let local = total_secs + off_secs as i64;
    let (y, mo, d, h, mi, s) = civil_from_epoch(local);
    let sign = if off_secs >= 0 { '+' } else { '-' };
    let abs = off_secs.abs();
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03} UTC{}{:02}:{:02}",
        y,
        mo,
        d,
        h,
        mi,
        s,
        millis,
        sign,
        abs / 3600,
        (abs % 3600) / 60
    )
}

fn local_utc_offset_secs() -> i32 {
    // Windows：GetTimeZoneInformation 太重，用 filetime vs localtime 差值。
    // 借助 std：SystemTime -> 文件时间没有直接 local API，改为解析环境 TZ 不可靠。
    // 这里用 win32 GetLocalTime？不引额外依赖的稳妥做法：
    // 比较 UTC 与本地格式化差值需要 OS 调用，退而求其次用 `tzoffset` 逻辑：
    // 直接用 std 不支持 —— 通过 cmd/powershell 太慢。用 windows crate。
    #[cfg(windows)]
    {
        use windows::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
        unsafe {
            let mut tz: TIME_ZONE_INFORMATION = std::mem::zeroed();
            let r = GetTimeZoneInformation(&mut tz);
            let mut bias_min = tz.Bias; // UTC = local + bias（bias 单位：分钟）
            if r == 1 {
                bias_min += tz.DaylightBias;
            } else if r == 2 {
                bias_min += tz.StandardBias;
            }
            // local = utc - bias*60
            -bias_min as i32 * 60
        }
    }
    #[cfg(not(windows))]
    {
        0
    }
}

fn civil_from_epoch(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let h = (rem / 3600) as u32;
    let mi = ((rem % 3600) / 60) as u32;
    let s = (rem % 60) as u32;
    // Howard Hinnant 的 civil_from_days 算法
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
    (y, m, d, h, mi, s)
}
