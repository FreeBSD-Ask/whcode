//! # WAL 与 checkpoint 策略（T042、§5.4、FR-510 关联）
//!
//! 三个能力：
//!
//! 1. [`apply_wal_policy`]：把调用方显式给出的 `checkpoint_threshold` /
//!    `wal_autocheckpoint` 设置写入当前连接（`None` 表示保持现状），并读回生效值；
//! 2. [`run_checkpoint`]：显式执行 `CHECKPOINT`，报告执行前后的 WAL 大小（DuckDB 原文）；
//! 3. [`should_checkpoint_on_idle`]：**纯函数**空闲判定——空闲窗口到期才允许空闲维护
//!    checkpoint，由调用方在事件循环中驱动；本模块不创建线程、不定时器（NFR-211：
//!    不得为每个会话新增常驻线程）。
//!
//! 高负载路径**无强制 checkpoint**（Check 42）：写者服务与查询路径从不调用 `CHECKPOINT`；
//! 超过阈值的自动 checkpoint 由 DuckDB 在事务边界执行，可经 [`crate::monitor::wal_status`]
//! 观测。设置值经白名单校验后内嵌 SQL（DuckDB `SET` 不支持参数绑定），拒绝引号与
//! 控制字符，注入面为零；值是调用方配置而非不可信输入（§10.1）。
//!
//! 边界：本模块只接受 [`duckdb::Connection`]，不打开连接、不选择路径。

use duckdb::Connection;
use whcode_domain::{ErrorCode, WhError};

/// WAL 策略；`None` 字段保持当前连接设置不变（默认值可见、可改、可撤销）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WalPolicy {
    /// WAL 大小达到该阈值时 DuckDB 自动 checkpoint（如 `"16MB"`、`"1GB"`）。
    pub checkpoint_threshold: Option<String>,
    /// 自动 checkpoint 开关与阈值（如 `"1MB"`；DuckDB 接受的任一合法值）。
    pub wal_autocheckpoint: Option<String>,
    /// 空闲维护窗口（毫秒）：距上次活动超过该时长才允许空闲 checkpoint。
    pub idle_window_ms: u64,
}

/// 应用策略后从连接读回的生效设置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveWalSettings {
    /// 生效的 `checkpoint_threshold`（DuckDB 原文）。
    pub checkpoint_threshold: String,
    /// 生效的 `wal_autocheckpoint`（DuckDB 原文）。
    pub wal_autocheckpoint: String,
}

/// checkpoint 触发原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointReason {
    /// 调用方显式触发（维护命令）。
    Explicit,
    /// 空闲窗口到期触发（判定见 [`should_checkpoint_on_idle`]）。
    IdleWindow,
}

impl CheckpointReason {
    /// 稳定标识（入日志与报告）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::IdleWindow => "idle_window",
        }
    }
}

/// 一次显式 checkpoint 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointReport {
    /// 触发原因。
    pub reason: CheckpointReason,
    /// 执行前 WAL 大小（DuckDB 原文，如 `512.0 KiB`）。
    pub wal_size_before: String,
    /// 执行后 WAL 大小（DuckDB 原文；成功 checkpoint 后应为零值）。
    pub wal_size_after: String,
}

fn wal_err(context: &str, err: duckdb::Error) -> WhError {
    WhError::new(ErrorCode::IoError, format!("{context}失败：{err}"))
}

fn setting_err(context: &str) -> WhError {
    WhError::new(ErrorCode::InvalidState, context.to_string())
}

/// 读回当前 WAL 相关设置。
fn read_effective(conn: &Connection) -> Result<EffectiveWalSettings, WhError> {
    let checkpoint_threshold: String = conn
        .query_row(
            "SELECT current_setting('checkpoint_threshold')",
            [],
            |row| row.get(0),
        )
        .map_err(|err| wal_err("读取 checkpoint_threshold", err))?;
    let wal_autocheckpoint: String = conn
        .query_row("SELECT current_setting('wal_autocheckpoint')", [], |row| {
            row.get(0)
        })
        .map_err(|err| wal_err("读取 wal_autocheckpoint", err))?;
    Ok(EffectiveWalSettings {
        checkpoint_threshold,
        wal_autocheckpoint,
    })
}

/// 校验设置值：仅允许字母、数字、点与空格（如 `16MB`、`1.5 GB`、`disabled`），
/// 长度 ≤ 32；拒绝引号、分号等任何可改变 SQL 结构的字符。
fn validate_setting(name: &str, value: &str) -> Result<(), WhError> {
    let ok = !value.is_empty()
        && value.len() <= 32
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == ' ');
    if ok {
        Ok(())
    } else {
        Err(setting_err(&format!(
            "WAL 设置 {name} 含非法字符或超长：仅允许字母/数字/点/空格，长度 ≤ 32"
        )))
    }
}

/// 应用 WAL 策略并读回生效设置。
///
/// # Errors
///
/// 设置值非法（含非法字符）或 DuckDB 拒绝该值时返回 [`WhError`]。
pub fn apply_wal_policy(
    conn: &Connection,
    policy: &WalPolicy,
) -> Result<EffectiveWalSettings, WhError> {
    if let Some(value) = &policy.checkpoint_threshold {
        validate_setting("checkpoint_threshold", value)?;
        conn.execute_batch(&format!("SET checkpoint_threshold='{value}'"))
            .map_err(|err| wal_err("设置 checkpoint_threshold", err))?;
    }
    if let Some(value) = &policy.wal_autocheckpoint {
        validate_setting("wal_autocheckpoint", value)?;
        conn.execute_batch(&format!("SET wal_autocheckpoint='{value}'"))
            .map_err(|err| wal_err("设置 wal_autocheckpoint", err))?;
    }
    read_effective(conn)
}

/// 显式执行 `CHECKPOINT`，报告前后 WAL 大小。
///
/// # Errors
///
/// 读取 WAL 状态或 checkpoint 执行失败时返回 [`WhError`]。
pub fn run_checkpoint(
    conn: &Connection,
    reason: CheckpointReason,
) -> Result<CheckpointReport, WhError> {
    let wal_size_before: String = conn
        .query_row("SELECT wal_size FROM pragma_database_size()", [], |row| {
            row.get(0)
        })
        .map_err(|err| wal_err("读取 WAL 大小", err))?;
    conn.execute_batch("CHECKPOINT")
        .map_err(|err| wal_err("执行 CHECKPOINT", err))?;
    let wal_size_after: String = conn
        .query_row("SELECT wal_size FROM pragma_database_size()", [], |row| {
            row.get(0)
        })
        .map_err(|err| wal_err("读取 WAL 大小", err))?;
    Ok(CheckpointReport {
        reason,
        wal_size_before,
        wal_size_after,
    })
}

/// 空闲判定（纯函数）：距上次活动超过 `idle_window_ms` 才返回 `true`。
///
/// 调用方自行维护 `last_activity_ms`（最近一次写入/命令时间）；本模块不维护状态。
#[must_use]
pub fn should_checkpoint_on_idle(now_ms: u64, last_activity_ms: u64, idle_window_ms: u64) -> bool {
    now_ms.saturating_sub(last_activity_ms) >= idle_window_ms
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::apply_migrations;

    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().expect("内存数据库");
        apply_migrations(&conn).expect("迁移");
        conn
    }

    #[test]
    fn default_policy_keeps_settings_and_readback_is_stable() {
        let conn = migrated();
        let effective = apply_wal_policy(&conn, &WalPolicy::default()).expect("应用默认策略");
        assert!(!effective.checkpoint_threshold.is_empty());
        assert!(!effective.wal_autocheckpoint.is_empty());
        let again = apply_wal_policy(&conn, &WalPolicy::default()).expect("重复应用");
        assert_eq!(effective, again, "默认策略不改设置，两次读回必须一致");
    }

    #[test]
    fn policy_overrides_are_effective_and_rejected_when_invalid() {
        let conn = migrated();
        let before = read_effective(&conn).expect("读取初始设置");
        let effective = apply_wal_policy(
            &conn,
            &WalPolicy {
                checkpoint_threshold: Some("2GB".to_string()),
                wal_autocheckpoint: None,
                idle_window_ms: 1_000,
            },
        )
        .expect("应用覆盖策略");
        assert_ne!(
            effective.checkpoint_threshold, before.checkpoint_threshold,
            "设置必须生效（DuckDB 可能换算单位，如 2GB → 1.8 GiB）"
        );

        for bad in [
            "16 MB; DROP TABLE events",
            "'quoted'",
            "",
            "x".repeat(33).as_str(),
        ] {
            let err = apply_wal_policy(
                &conn,
                &WalPolicy {
                    checkpoint_threshold: Some(bad.to_string()),
                    ..WalPolicy::default()
                },
            )
            .expect_err("非法设置必须拒绝");
            assert_eq!(err.code, ErrorCode::InvalidState, "非法值：{bad}");
        }
        // 拒绝后设置不得被部分应用：读回应仍是上次合法值。
        let effective = read_effective(&conn).expect("读回");
        assert_ne!(effective.checkpoint_threshold, before.checkpoint_threshold);
    }

    #[test]
    fn run_checkpoint_reports_wal_sizes_and_reason() {
        let conn = migrated();
        let report = run_checkpoint(&conn, CheckpointReason::Explicit).expect("显式 checkpoint");
        assert_eq!(report.reason.as_str(), "explicit");
        assert!(!report.wal_size_before.is_empty());
        assert!(!report.wal_size_after.is_empty());

        let idle = run_checkpoint(&conn, CheckpointReason::IdleWindow).expect("空闲 checkpoint");
        assert_eq!(idle.reason.as_str(), "idle_window");
    }

    #[test]
    fn idle_decision_is_pure_and_window_boundary_is_inclusive() {
        assert!(
            !should_checkpoint_on_idle(10_000, 9_500, 1_000),
            "未到窗口不得触发"
        );
        assert!(
            should_checkpoint_on_idle(10_000, 9_000, 1_000),
            "恰好达到空闲窗口即视为到期"
        );
        assert!(should_checkpoint_on_idle(10_000, 0, 1_000));
        assert!(
            !should_checkpoint_on_idle(500, 1_000, 1_000),
            "时钟回退不得误判"
        );
    }
}
