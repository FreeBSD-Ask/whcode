//! 构建时版本戳（`WHCODE_BUILD_STAMP`）：**本地时间**，精确到分钟并带时区偏移。
//!
//! 版本号定义为二进制构建时间（用户要求，2026-10-23）：构建脚本在运行时取系统时钟
//! 并注入 `cargo:rustc-env`，`whcode --version` / `doctor` 原样报告。
//!
//! 时区获取（零新依赖、零 unsafe）：
//! - Windows：PowerShell `Get-Date -Format yyyy-MM-ddTHH:mmzzz`（子进程，构建期一次）；
//! - 其他平台：`date +%Y-%m-%dT%H:%M%z`（POSIX）；
//! - 两者失败时**显式回退 UTC（带 `Z`）**——不伪装本地时区。
//!
//! 缓存行为（诚实说明）：未输出 `rerun-if-changed` 指令时，Cargo 默认在**本包内**任一
//! 文件变化时重跑本脚本——版本戳随本包源码构建更新；若仅依赖变化导致重链接，版本戳
//! 可能仍是上一次本包构建的时刻（那仍是该二进制的"构建时间"）。

use std::process::Command;

fn main() {
    let stamp = local_minute_stamp();
    println!("cargo:rustc-env=WHCODE_BUILD_STAMP={stamp}");
}

/// 本地构建时间戳：`YYYY-MM-DDTHH:MM±HH:MM`（失败回退 UTC `YYYY-MM-DDTHH:MMZ`）。
fn local_minute_stamp() -> String {
    if let Some(stamp) = query_local() {
        return stamp;
    }
    format!("{}Z", utc_minute(unix_seconds()))
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

#[cfg(target_os = "windows")]
fn query_local() -> Option<String> {
    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Get-Date -Format yyyy-MM-ddTHH:mmzzz",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    normalize_stamp(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(not(target_os = "windows"))]
fn query_local() -> Option<String> {
    let output = Command::new("date")
        .args(["+%Y-%m-%dT%H:%M%z"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    normalize_stamp(&String::from_utf8_lossy(&output.stdout))
}

/// 规范化为 `YYYY-MM-DDTHH:MM±HH:MM`；格式不符或偏移缺失时返回 `None`。
fn normalize_stamp(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let bytes = raw.as_bytes();
    if bytes.len() < 17 {
        return None;
    }
    let head = &raw[..16];
    if !valid_head(head) {
        return None;
    }
    let offset = &raw[16..];
    let offset = match offset.len() {
        // +HH:MM（PowerShell zzz）
        6 if offset.as_bytes()[3] == b':' => offset.to_string(),
        // +HHMM（POSIX date %z）
        5 if is_sign(offset.as_bytes()[0]) => {
            format!("{}:{}", &offset[..3], &offset[3..])
        }
        _ => return None,
    };
    if !valid_offset(&offset) {
        return None;
    }
    Some(format!("{head}{offset}"))
}

fn is_sign(byte: u8) -> bool {
    byte == b'+' || byte == b'-'
}

/// `YYYY-MM-DDTHH:MM`：长度与分隔符位置固定，其余为 ASCII 数字。
fn valid_head(head: &str) -> bool {
    let bytes = head.as_bytes();
    bytes.len() == 16
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b':'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7 | 10 | 13) || byte.is_ascii_digit())
}

/// `±HH:MM`：符号 + 4 位数字 + 冒号。
fn valid_offset(offset: &str) -> bool {
    let bytes = offset.as_bytes();
    bytes.len() == 6
        && is_sign(bytes[0])
        && bytes[3] == b':'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 0 | 3) || byte.is_ascii_digit())
}

/// Unix 秒 → `YYYY-MM-DDTHH:MM`（UTC，用于回退）。
fn utc_minute(seconds: u64) -> String {
    let days = seconds / 86_400;
    let seconds_of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}")
}

/// 自 1970-01-01 起的天数 → (年, 月, 日)；Howard Hinnant civil_from_days（1970 年后）。
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day)
}
