//! 构建时版本戳（`WHCODE_BUILD_STAMP`）：UTC 时间、精确到分钟。
//!
//! 版本号定义为**二进制构建时间**（用户要求，2026-10-23）：构建脚本在运行时取
//! 系统时钟并注入 `cargo:rustc-env`，`whcode --version` / `doctor` 原样报告。
//!
//! 缓存行为（诚实说明）：未输出 `rerun-if-changed` 指令时，Cargo 默认在**本包内**
//! 任一文件变化时重跑本脚本——版本戳随本包源码构建更新；若仅依赖变化导致重链接，
//! 版本戳可能仍是上一次本包构建的时刻（那仍是该二进制的"构建时间"）。
//!
//! 零依赖：不引 `chrono` 等新依赖（AGENTS §4 的依赖纪律），日历换算用
//! Howard Hinnant 的 civil_from_days 算法（公历、1970 年起，纯整数运算）。

fn main() {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    println!("cargo:rustc-env=WHCODE_BUILD_STAMP={}", utc_minute(seconds));
}

/// Unix 秒 → `YYYY-MM-DDTHH:MMZ`（UTC，精确到分钟）。
fn utc_minute(seconds: u64) -> String {
    let days = seconds / 86_400;
    let seconds_of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}Z")
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
