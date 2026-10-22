//! # 存储 schema 与版本化迁移（T032、T033）
//!
//! 14 张表与 `data-model.md` §1 一一对应。迁移是**唯一**建表入口；其他模块不得自行
//! `CREATE TABLE` 正式表。迁移在一个事务内执行：SQL 失败时不写入 `schema_migrations`，
//! 数据库不会被标记为新版本（NFR-103、§7.4）。
//!
//! 边界：本模块只接受 [`duckdb::Connection`]，不打开连接、不选择路径；连接生命周期由
//! 调用方（`whcode-bin` 或测试夹具）管理。

use duckdb::{Connection, params};
use whcode_domain::{ErrorCode, WhError};

/// 当前正式 schema 版本。
///
/// v2 补齐事件追溯索引（`request_id` / `tool_call_id`，data-model.md §3、FR-204）；
/// v3 补齐消息游标索引 `(session_id, ts, id)` 与任务阶段索引 `(session_id, stage)`；
/// v4 增加会话 ID 序列（`whcode_session_id_seq`，M2/T046 会话命令的自动编号）；
/// v5 会话增加 SDD 阶段列（`sessions.stage`，M4/T068—T081 状态机持久化）；
/// 不改既有行数据。
pub const STORE_SCHEMA_VERSION: u32 = 5;

/// 正式表数量（含 `schema_migrations`）。
pub const TABLE_COUNT: usize = 14;

const MIGRATION_0001: &str = include_str!("../migrations/0001_initial.sql");
const MIGRATION_0002: &str = include_str!("../migrations/0002_event_trace_indexes.sql");
const MIGRATION_0003: &str = include_str!("../migrations/0003_query_indexes.sql");
const MIGRATION_0004: &str = include_str!("../migrations/0004_session_id_sequence.sql");
const MIGRATION_0005: &str = include_str!("../migrations/0005_session_stage.sql");

/// 版本化迁移清单；新增版本时追加，不修改历史 SQL。
const MIGRATIONS: &[(u32, &str)] = &[
    (1, MIGRATION_0001),
    (2, MIGRATION_0002),
    (3, MIGRATION_0003),
    (4, MIGRATION_0004),
    (5, MIGRATION_0005),
];

fn store_err(context: &str, err: duckdb::Error) -> WhError {
    WhError::new(ErrorCode::IoError, format!("schema {context} 失败：{err}"))
}

/// 迁移 SQL 的稳定校验和（FNV-1a 64 位；用于检测脚本漂移，不作密码学用途）。
#[must_use]
pub fn checksum(sql: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in sql.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("fnv1a64:{hash:016x}")
}

/// 读取已应用的最大 schema 版本；无表或无记录时为 0。
pub fn schema_version(conn: &Connection) -> Result<u32, WhError> {
    let version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )
        .map_err(|err| store_err("读取版本", err))?;
    u32::try_from(version).map_err(|_| {
        WhError::new(
            ErrorCode::IoError,
            format!("schema 版本超出 u32 范围：{version}"),
        )
    })
}

/// 应用全部未执行的迁移，返回最终版本。
///
/// 幂等：已应用的版本跳过；重复调用不会重复插入 `schema_migrations`。
pub fn apply_migrations(conn: &Connection) -> Result<u32, WhError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
             version    BIGINT PRIMARY KEY,
             applied_at TIMESTAMP NOT NULL,
             checksum   TEXT NOT NULL
         )",
    )
    .map_err(|err| store_err("创建迁移表", err))?;

    let current = schema_version(conn)?;
    for (version, sql) in MIGRATIONS {
        if *version <= current {
            continue;
        }
        apply_one(conn, *version, sql)?;
    }
    schema_version(conn)
}

fn apply_one(conn: &Connection, version: u32, sql: &str) -> Result<(), WhError> {
    conn.execute_batch("BEGIN TRANSACTION")
        .map_err(|err| store_err("开始迁移事务", err))?;

    let result = (|| -> duckdb::Result<()> {
        conn.execute_batch(sql)?;
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at, checksum)
             VALUES (?, current_timestamp, ?)",
            params![version, checksum(sql)],
        )?;
        Ok(())
    })();

    match result {
        Ok(()) => conn
            .execute_batch("COMMIT")
            .map_err(|err| store_err("提交迁移事务", err)),
        Err(err) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(store_err(&format!("应用迁移 v{version}"), err))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table_count(conn: &Connection) -> i64 {
        conn.query_row(
            "SELECT count(*) FROM information_schema.tables
             WHERE table_schema = 'main' AND table_type = 'BASE TABLE'",
            [],
            |row| row.get(0),
        )
        .expect("统计表数量")
    }

    #[test]
    fn creates_all_tables_and_is_idempotent() {
        let conn = Connection::open_in_memory().expect("内存数据库");
        assert_eq!(
            apply_migrations(&conn).expect("首次迁移"),
            STORE_SCHEMA_VERSION
        );
        assert_eq!(table_count(&conn), TABLE_COUNT as i64);

        assert_eq!(
            apply_migrations(&conn).expect("重复迁移"),
            STORE_SCHEMA_VERSION
        );
        assert_eq!(table_count(&conn), TABLE_COUNT as i64);

        let rows: i64 = conn
            .query_row("SELECT count(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .expect("迁移行数");
        assert_eq!(
            rows,
            i64::try_from(MIGRATIONS.len()).expect("迁移数量可转换"),
            "重复应用不得重复写入迁移记录；每个版本恰一行"
        );
    }

    #[test]
    fn failed_migration_does_not_mark_version() {
        let conn = Connection::open_in_memory().expect("内存数据库");
        apply_migrations(&conn).expect("首次迁移");

        let err = apply_one(&conn, 99, "CREATE TABLE broken (").expect_err("语法错误必须失败");
        assert!(err.to_string().contains("迁移"), "错误须带迁移上下文");

        assert_eq!(schema_version(&conn).expect("版本"), STORE_SCHEMA_VERSION);
        let broken: i64 = conn
            .query_row(
                "SELECT count(*) FROM information_schema.tables
                 WHERE table_schema = 'main' AND table_name = 'broken'",
                [],
                |row| row.get(0),
            )
            .expect("查询 broken");
        assert_eq!(broken, 0, "失败迁移必须回滚建表");
    }

    #[test]
    fn checksum_is_stable_and_versioned() {
        assert_eq!(checksum(MIGRATION_0001), checksum(MIGRATION_0001));
        assert_eq!(checksum(MIGRATION_0002), checksum(MIGRATION_0002));
        assert_eq!(checksum(MIGRATION_0003), checksum(MIGRATION_0003));
        assert!(checksum(MIGRATION_0001).starts_with("fnv1a64:"));
        assert!(checksum(MIGRATION_0002).starts_with("fnv1a64:"));
        assert!(checksum(MIGRATION_0003).starts_with("fnv1a64:"));
        assert_eq!(
            MIGRATIONS.last().map(|(version, _)| *version),
            Some(STORE_SCHEMA_VERSION)
        );
    }

    #[test]
    fn events_table_has_expected_columns() {
        let conn = Connection::open_in_memory().expect("内存数据库");
        apply_migrations(&conn).expect("迁移");
        let columns: i64 = conn
            .query_row(
                "SELECT count(*) FROM information_schema.columns
                 WHERE table_name = 'events'
                   AND column_name IN ('seq','id','session_id','task_id','request_id',
                                       'tool_call_id','kind','payload_json','ts',
                                       'persistence_level')",
                [],
                |row| row.get(0),
            )
            .expect("查询列");
        assert_eq!(columns, 10);
    }

    #[test]
    fn migration_versions_are_strictly_increasing() {
        let versions: Vec<u32> = MIGRATIONS.iter().map(|(version, _)| *version).collect();
        assert!(
            versions.windows(2).all(|pair| pair[0] < pair[1]),
            "迁移版本必须严格递增：{versions:?}"
        );
        assert_eq!(
            versions.last().copied(),
            Some(STORE_SCHEMA_VERSION),
            "当前版本必须等于最后一条迁移"
        );
    }
}
