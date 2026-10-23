//! 维护命令的应用层入口（FR-204/508/509；checkpoint 与 restore 属 FR-510 与
//! `docs/guides/disaster-recovery.md` §8 的应用层收口）。
//!
//! 清理是**破坏性**命令：判定（判据/保护/审计）留在 `whcode-store`，权限与确认语义
//! 在本层统一（`permission_denied` 与 CUI 同源，FR-604）；前端只负责传输。
//!
//! 权限映射：清理按 TTL/时间点删除会话及其关联行，与按 ID 删除同属 `session.delete`
//! 能力——不新增大类，避免同一后果存在两套授权口径。checkpoint（清理 WAL、回收磁盘）
//! 与 restore（向空库写回工作区）同属维护大类，复用 `session.delete`，不另立授权口径。

use std::collections::BTreeMap;
use std::path::Path;

use duckdb::Connection;
use whcode_domain::WhError;
use whcode_store::{CheckpointReason, CheckpointReport, EffectiveWalSettings, WalPolicy};

use crate::authz::{Actor, Capability, Policy, require};

pub use whcode_store::{PurgeCounts, PurgeCriteria, PurgePreview, PurgeProtection, PurgeResult};

/// 清理预览（dry-run）：不写库，返回冻结清单与各行数。
///
/// # Errors
///
/// 权限拒绝或存储失败时返回 [`WhError`]。
pub fn purge_preview(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    criteria: &PurgeCriteria,
    protection: &PurgeProtection,
    now_ms: u64,
) -> Result<PurgePreview, WhError> {
    require(policy, actor, Capability::SessionDelete)?;
    whcode_store::preview_purge(conn, criteria, protection, now_ms)
}

/// 执行清理：按预览冻结的清单删除，同一事务写审计；`confirm = false` 时由 store 拒绝。
///
/// # Errors
///
/// 权限拒绝、确认缺失或存储失败时返回 [`WhError`]。
pub fn purge_execute(
    conn: &mut Connection,
    policy: &dyn Policy,
    actor: &Actor,
    preview: &PurgePreview,
    confirm: bool,
    protection: &PurgeProtection,
    now_ms: u64,
) -> Result<PurgeResult, WhError> {
    require(policy, actor, Capability::SessionDelete)?;
    whcode_store::execute_purge(conn, preview, confirm, &actor.id, protection, now_ms)
}

/// 显式 WAL 覆盖：两字段 `None` 表示保持当前连接设置不变（默认值可见、可改、可撤销）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WalPolicyOverride {
    /// WAL 自动 checkpoint 的容量阈值（如 `16MB`、`1GB`）；`None` 保持现状。
    pub checkpoint_threshold: Option<String>,
    /// 自动 checkpoint 开关与阈值（如 `1MB`、`disabled`）；`None` 保持现状。
    pub wal_autocheckpoint: Option<String>,
}

/// 显式 checkpoint 的结果：生效设置与执行报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointOutcome {
    /// 应用覆盖后从连接读回的生效设置（DuckDB 原文）。
    pub effective: EffectiveWalSettings,
    /// checkpoint 执行报告（触发原因与前后 WAL 大小）。
    pub report: CheckpointReport,
}

/// 显式 checkpoint（disaster-recovery §8）：先应用 WAL 覆盖（`None` 保持现状），再执行
/// `CHECKPOINT`，返回生效设置与执行报告。
///
/// 覆盖值经 store 白名单校验（拒绝引号与控制字符）；显式命令路径不经空闲判定，
/// 空闲窗口固定为 0。
///
/// # Errors
///
/// 权限拒绝、WAL 设置非法或 checkpoint 执行失败时返回 [`WhError`]。
pub fn checkpoint_database(
    conn: &mut Connection,
    policy: &dyn Policy,
    actor: &Actor,
    wal: &WalPolicyOverride,
) -> Result<CheckpointOutcome, WhError> {
    require(policy, actor, Capability::SessionDelete)?;
    let wal_policy = WalPolicy {
        checkpoint_threshold: wal.checkpoint_threshold.clone(),
        wal_autocheckpoint: wal.wal_autocheckpoint.clone(),
        idle_window_ms: 0,
    };
    let effective = whcode_store::apply_wal_policy(conn, &wal_policy)?;
    let report = whcode_store::run_checkpoint(conn, CheckpointReason::Explicit)?;
    Ok(CheckpointOutcome { effective, report })
}

/// 工作区恢复结果：各表实际插入行数与来源文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreReport {
    /// 各表实际插入行数（与 `whcode_store::ImportReport::inserted` 一致）。
    pub inserted: BTreeMap<String, i64>,
    /// 恢复来源文件路径。
    pub source: String,
}

/// 从导出文件恢复整个工作区（disaster-recovery §8）：仅允许导入到空库。
///
/// 保护在编排层、导入本体在 store：恢复是写操作且无 dry-run 预览，`confirm = false`
/// 一律拒绝；目标库已有任一会话/事件/任务行即拒绝且不写任何数据——覆盖语义必须经
/// `db purge` 显式清理，不做隐式覆盖。`import_workspace` 自带格式/schema/校验和/
/// 冲突四层校验与单一事务，本层不重复实现。
///
/// # Errors
///
/// 权限拒绝、未确认、目标库非空、导出文件缺失/损坏/不兼容或导入失败时返回 [`WhError`]。
pub fn restore_workspace(
    conn: &mut Connection,
    policy: &dyn Policy,
    actor: &Actor,
    source_path: &Path,
    confirm: bool,
) -> Result<RestoreReport, WhError> {
    require(policy, actor, Capability::SessionDelete)?;
    if !confirm {
        return Err(WhError::invalid_state(
            "恢复是写操作且不支持预览：必须显式确认（confirm=true），并保证目标库为空",
        ));
    }
    if whcode_store::has_any_workspace_data(conn)? {
        return Err(WhError::invalid_state(
            "目标库非空，拒绝恢复：恢复仅允许导入到空库；先用 db purge 清理，或改用空库",
        ));
    }
    let report = whcode_store::import_workspace(conn, source_path, None)?;
    Ok(RestoreReport {
        inserted: report.inserted,
        source: source_path.display().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authz::{DefaultPolicy, actor_with_all_capabilities};
    use duckdb::params;
    use whcode_domain::ErrorCode;

    const MIGRATIONS: &str = include_str!("../../whcode-store/migrations/0001_initial.sql");

    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().expect("内存库");
        conn.execute_batch(MIGRATIONS).expect("迁移");
        conn
    }

    /// restore 走 `import_workspace` 的 schema 兼容校验，源库与目标库都必须全量迁移。
    fn migrated_full() -> Connection {
        let conn = Connection::open_in_memory().expect("内存库");
        whcode_store::apply_migrations(&conn).expect("迁移");
        conn
    }

    /// 进程内唯一的临时文件路径（目录名含进程 ID + 纳秒时间戳，同 store test_util 思路）。
    fn temp_file(tag: &str) -> std::path::PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let dir =
            std::env::temp_dir().join(format!("whcode-app-test-{}-{stamp}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("创建测试临时目录");
        dir.join(format!("{tag}.json"))
    }

    /// 最小可导出工作区：元数据 + 单会话 + 若干事件。
    fn seed_for_export(conn: &Connection, session_id: &str, event_count: i64) {
        conn.execute(
            "INSERT INTO workspace_meta (workspace_id, schema_version, created_at)
             VALUES ('WS-R', ?, current_timestamp)",
            params![whcode_store::STORE_SCHEMA_VERSION as i64],
        )
        .expect("插入工作区元数据");
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, last_active_at, model, archived, status)
             VALUES (?, '恢复测试', current_timestamp, current_timestamp, NULL, FALSE, 'idle')",
            params![session_id],
        )
        .expect("插入会话");
        for seq in 1..=event_count {
            conn.execute(
                "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                                     kind, payload_json, ts, persistence_level)
                 VALUES (?, ?, ?, NULL, NULL, NULL, 'StreamingChunk', ?,
                         current_timestamp, 'redacted')",
                params![
                    seq,
                    format!("E-{seq}"),
                    session_id,
                    format!("{{\"n\":{seq}}}")
                ],
            )
            .expect("插入事件");
        }
    }

    fn count(conn: &Connection, table: &str) -> i64 {
        conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .expect("计数")
    }

    /// 预览不写库；执行必须显式 `confirm`；缺能力一律 `permission_denied`。
    #[test]
    fn purge_requires_capability_and_explicit_confirm() {
        let mut conn = migrated();
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, last_active_at, status)
             VALUES ('S-OLD', '旧', timestamp '2020-01-01 00:00:00',
                     timestamp '2020-01-01 00:00:00', 'active')",
            [],
        )
        .expect("创建");
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        let protection = PurgeProtection::default();
        // 判据（2020-09）晚于会话活动时间（2020-01），当前时间（2023-11）远超活跃窗口。
        let cutoff = 1_600_000_000_000_i64;
        let now = 1_700_000_000_000_u64;
        let criteria = PurgeCriteria::before(cutoff);
        let preview =
            purge_preview(&conn, &policy, &actor, &criteria, &protection, now).expect("预览");
        assert_eq!(preview.selected_sessions, vec!["S-OLD".to_string()]);
        assert_eq!(count(&conn, "sessions"), 1, "预览不得写库");

        // 未确认：store 拒绝，数据不动。
        let err = purge_execute(
            &mut conn,
            &policy,
            &actor,
            &preview,
            false,
            &protection,
            now,
        )
        .expect_err("未确认必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert_eq!(count(&conn, "sessions"), 1, "拒绝后不得删除");

        // 无能力：拒绝且不写库。
        let weak = Actor::default();
        let err = purge_preview(&conn, &policy, &weak, &criteria, &protection, now)
            .expect_err("缺能力必须拒绝");
        assert_eq!(err.code, ErrorCode::PermissionDenied);

        // 确认后执行。
        purge_execute(&mut conn, &policy, &actor, &preview, true, &protection, now).expect("执行");
        assert_eq!(count(&conn, "sessions"), 0);
    }

    /// checkpoint：两字段覆盖必须生效（读回变化）；全 `None` 只执行 checkpoint、
    /// 设置保持现状；缺能力拒绝。
    #[test]
    fn checkpoint_applies_overrides_none_keeps_settings_and_requires_capability() {
        let mut conn = migrated();
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");

        let before = whcode_store::apply_wal_policy(&conn, &whcode_store::WalPolicy::default())
            .expect("读基线");
        let outcome =
            checkpoint_database(&mut conn, &policy, &actor, &WalPolicyOverride::default())
                .expect("默认 checkpoint");
        assert_eq!(outcome.effective, before, "全 None 覆盖不得改变设置");
        assert_eq!(outcome.report.reason.as_str(), "explicit");
        assert!(!outcome.report.wal_size_before.is_empty());
        assert!(!outcome.report.wal_size_after.is_empty());

        let outcome = checkpoint_database(
            &mut conn,
            &policy,
            &actor,
            &WalPolicyOverride {
                checkpoint_threshold: Some("2GB".to_string()),
                wal_autocheckpoint: Some("8MB".to_string()),
            },
        )
        .expect("覆盖 checkpoint");
        assert_ne!(
            outcome.effective.checkpoint_threshold, before.checkpoint_threshold,
            "覆盖必须生效（DuckDB 可能换算单位）"
        );
        assert_ne!(
            outcome.effective.wal_autocheckpoint, before.wal_autocheckpoint,
            "覆盖必须生效（DuckDB 可能换算单位）"
        );

        let weak = Actor::default();
        let err = checkpoint_database(&mut conn, &policy, &weak, &WalPolicyOverride::default())
            .expect_err("缺能力必须拒绝");
        assert_eq!(err.code, ErrorCode::PermissionDenied);
    }

    /// 恢复往返：导出 → 空库恢复，逐表行数与导出清单一致。
    #[test]
    fn restore_round_trip_into_empty_database() {
        let source = migrated_full();
        seed_for_export(&source, "S-R", 3);
        let path = temp_file("restore-roundtrip");
        let exported = whcode_store::export_workspace(&source, &path, "tester", 1_700_000_000_000)
            .expect("导出");

        let mut target = migrated_full();
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        let report = restore_workspace(&mut target, &policy, &actor, &path, true).expect("恢复");
        assert_eq!(report.source, path.display().to_string());
        assert_eq!(
            report.inserted, exported.manifest.counts,
            "逐表插入行数必须与导出清单一致"
        );
        assert_eq!(report.inserted.get("sessions"), Some(&1));
        assert_eq!(report.inserted.get("events"), Some(&3));
        assert_eq!(count(&target, "sessions"), 1);
        assert_eq!(count(&target, "events"), 3);
        let _ = std::fs::remove_file(&path);
    }

    /// 非空库即使 confirm=true 也必须拒绝，且零写入。
    #[test]
    fn restore_rejects_non_empty_target_and_writes_nothing() {
        let source = migrated_full();
        seed_for_export(&source, "S-R", 2);
        let path = temp_file("restore-nonempty");
        whcode_store::export_workspace(&source, &path, "tester", 1).expect("导出");

        let mut target = migrated_full();
        whcode_store::create_session_with_id(&target, "S-KEEP", "已有", "active", 1_000)
            .expect("预置会话");
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");

        let err = restore_workspace(&mut target, &policy, &actor, &path, true)
            .expect_err("非空库必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(
            err.to_string().contains("空库"),
            "消息须说明空库要求：{err}"
        );
        assert_eq!(count(&target, "sessions"), 1, "预置会话必须保留");
        assert_eq!(count(&target, "events"), 0, "拒绝后不得写入");
        let _ = std::fs::remove_file(&path);
    }

    /// confirm=false 一律拒绝（恢复无预览）；能力不足拒绝；两者均零写入。
    #[test]
    fn restore_requires_confirm_and_capability() {
        let source = migrated_full();
        seed_for_export(&source, "S-R", 1);
        let path = temp_file("restore-guard");
        whcode_store::export_workspace(&source, &path, "tester", 1).expect("导出");
        let mut target = migrated_full();
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");

        let err = restore_workspace(&mut target, &policy, &actor, &path, false)
            .expect_err("未确认必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(
            err.to_string().contains("写操作"),
            "消息须说明恢复是写操作：{err}"
        );
        assert_eq!(count(&target, "sessions"), 0, "未确认不得写入");

        let weak = Actor::default();
        let err = restore_workspace(&mut target, &policy, &weak, &path, true)
            .expect_err("缺能力必须拒绝");
        assert_eq!(err.code, ErrorCode::PermissionDenied);
        assert_eq!(count(&target, "sessions"), 0, "拒绝后不得写入");
        let _ = std::fs::remove_file(&path);
    }

    /// 坏文件与缺失文件按 import 实际行为拒绝：损坏 JSON 为 parse_error，
    /// 文件缺失为 io_error；两者均零写入。
    #[test]
    fn restore_rejects_corrupt_and_missing_source() {
        let mut target = migrated_full();
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");

        let bad = temp_file("restore-bad");
        std::fs::write(&bad, b"not-json{{{").expect("写坏文件");
        let err = restore_workspace(&mut target, &policy, &actor, &bad, true)
            .expect_err("损坏文件必须拒绝");
        assert_eq!(err.code, ErrorCode::ParseError);
        assert_eq!(count(&target, "sessions"), 0, "解析失败不得写入");

        let missing = temp_file("restore-missing");
        let err = restore_workspace(&mut target, &policy, &actor, &missing, true)
            .expect_err("缺失文件必须拒绝");
        assert_eq!(err.code, ErrorCode::IoError);
        assert_eq!(count(&target, "sessions"), 0, "读取失败不得写入");
        let _ = std::fs::remove_file(&bad);
    }
}
