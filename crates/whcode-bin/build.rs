//! 构建时版本戳（`WHCODE_BUILD_STAMP`）：**本地时间**，精确到分钟并带时区偏移。
//!
//! 版本号定义为二进制构建时间（用户要求，2026-10-23）：构建脚本在运行时取系统时钟
//! 并注入 `cargo:rustc-env`，`whcode --version` / `doctor` 原样报告。
//!
//! **外部注入（ADR-047，可重现构建）**：环境变量 `WHCODE_BUILD_STAMP` 已存在且合
//! 规式时原样透传（发布构建由 CI 从 git 提交时间导出 UTC `Z` 形式），使同一提交的
//! 全新构建字节一致；未注入或值不合规式时回退构建墙钟——非法值输出构建告警并忽略，
//! 不静默接受。
//!
//! 时区获取（零新依赖、零 unsafe、**不做平台判定**）：
//! 契约测试把 `build.rs` 计入 crate 源码，平台条件编译必须集中在 `whcode-platform`
//! （NFR-101），因此本脚本不按平台名分流，改为按**可用性**依次探测外部时间
//! 命令：先 `powershell`（`Get-Date -Format yyyy-MM-ddTHH:mmzzz`，Windows 形态），
//! 失败再 `date +%Y-%m-%dT%H:%M%z`（POSIX 形态）；命令不存在或输出不合格式时该次
//! 探测自然返回 `None`。两者均不可用时**显式回退 UTC（带 `Z`）并输出构建告警**——
//! 不伪装本地时区，也不静默降级。此为 `whcode-store/build.rs` 同款纪律：以载荷/可用性
//! 分流，而非以平台名分流。
//!
//! 缓存行为（诚实说明）：未输出 `rerun-if-changed` 指令时，Cargo 默认在**本包内**任一
//! 文件变化时重跑本脚本——版本戳随本包源码构建更新；若仅依赖变化导致重链接，版本戳
//! 可能仍是上一次本包构建的时刻（那仍是该二进制的"构建时间"）。

use std::{env, process::Command};

fn main() {
    println!("cargo:rerun-if-env-changed=WHCODE_BUILD_STAMP");
    // 外部注入优先（ADR-047）：发布构建用提交时间保证同一提交可重现；
    // 未注入或非法时走构建墙钟（非法值已在 provided_stamp 内告警）。
    if let Some(stamp) = provided_stamp() {
        println!("cargo:rustc-env=WHCODE_BUILD_STAMP={stamp}");
        return;
    }
    match local_minute_stamp() {
        Some(stamp) => println!("cargo:rustc-env=WHCODE_BUILD_STAMP={stamp}"),
        None => {
            let stamp = format!("{}Z", utc_minute(unix_seconds()));
            println!(
                "cargo:warning=构建期外部时间命令（powershell/date）均不可用，\
                 版本戳回退 UTC（非本地时区）：{stamp}"
            );
            println!("cargo:rustc-env=WHCODE_BUILD_STAMP={stamp}");
        }
    }
}

/// 外部注入的版本戳（ADR-047）：`WHCODE_BUILD_STAMP` 已设置且合规式时返回原值。
///
/// 接受与墙钟路径同门的两种形式：`YYYY-MM-DDTHH:MM±HH:MM`（22，带偏移）与
/// `YYYY-MM-DDTHH:MMZ`（17，UTC）。未设置返回 `None`；设置但非法也返回 `None`
/// 并输出构建告警（回退墙钟，不静默接受垃圾值）。
fn provided_stamp() -> Option<String> {
    let Ok(raw) = env::var("WHCODE_BUILD_STAMP") else {
        return None;
    };
    let trimmed = raw.trim();
    let parsed = if trimmed.len() == 17 && trimmed.ends_with('Z') && valid_head(&trimmed[..16]) {
        Some(trimmed.to_string())
    } else {
        normalize_stamp(trimmed)
    };
    if parsed.is_none() {
        println!(
            "cargo:warning=WHCODE_BUILD_STAMP 注入值 {raw:?} 不合规式\
             （应为 YYYY-MM-DDTHH:MM±HH:MM 或 YYYY-MM-DDTHH:MMZ），忽略并回退构建墙钟"
        );
    }
    parsed
}

/// 本地构建时间戳：`YYYY-MM-DDTHH:MM±HH:MM`；按可用性探测，无平台条件编译（NFR-101）。
fn local_minute_stamp() -> Option<String> {
    query_stamp(&[
        "powershell",
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "Get-Date -Format yyyy-MM-ddTHH:mmzzz",
    ])
    .or_else(|| query_stamp(&["date", "+%Y-%m-%dT%H:%M%z"]))
}

/// 运行 `argv[0]`（其余为参数）并规范化输出；命令缺失、非零退出或格式不符均返回 `None`。
fn query_stamp(argv: &[&str]) -> Option<String> {
    let (program, args) = argv.split_first()?;
    let output = Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    normalize_stamp(&String::from_utf8_lossy(&output.stdout))
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
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
