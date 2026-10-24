//! 迁移集成测试（T033、T036）。
//!
//! 覆盖六条路径：
//! 1. v0（空库）→ v6：一次应用全部迁移，表数、版本记录与索引齐备；
//! 2. v1 → v6：用真实 v1 SQL 构造历史工作区，跨版升级后数据保留、全部新索引生效；
//! 3. v2 → v6：上一受支持版本升级，消息与事件数据保留；
//! 4. v2 中断：v2 的第一条索引成功、第二条失败时整个事务回滚——版本停在 v1，
//!    第一条索引不得残留；
//! 5. v3 中断：v3 删除旧消息索引、建新索引成功后，任务索引失败——版本停在 v2，
//!    旧消息索引必须随回滚恢复，新索引不得残留（NFR-103、§7.4）；
//! 6. v5 → v6：事件载荷由 TEXT 副本转换为原生 VARIANT，既有行可读回。
//!
//! 新增 schema 版本时必须在同一文件追加对应升级路径与中断回滚用例，不得删除既有断言。

use duckdb::{Connection, params};

/// 历史迁移 SQL 从仓库原文件读入，避免测试复制一份“看似相同”的 DDL。
const V1_SQL: &str = include_str!("../../crates/whcode-store/migrations/0001_initial.sql");
const V2_SQL: &str =
    include_str!("../../crates/whcode-store/migrations/0002_event_trace_indexes.sql");
const V3_SQL: &str = include_str!("../../crates/whcode-store/migrations/0003_query_indexes.sql");
const V4_SQL: &str =
    include_str!("../../crates/whcode-store/migrations/0004_session_id_sequence.sql");
const V5_SQL: &str = include_str!("../../crates/whcode-store/migrations/0005_session_stage.sql");
const V6_SQL: &str = include_str!("../../crates/whcode-store/migrations/0006_events_variant.sql");

/// 构造一个处于 v1 的工作区：执行真实 v1 SQL 并登记迁移记录（校验和与生产代码同算法）。
fn create_v1_workspace(conn: &Connection) {
    conn.execute_batch(V1_SQL).expect("应用 v1 SQL");
    conn.execute(
        "INSERT INTO schema_migrations (version, applied_at, checksum)
         VALUES (1, current_timestamp, ?)",
        params![whcode_store::checksum(V1_SQL)],
    )
    .expect("登记 v1 迁移");
    assert_eq!(
        whcode_store::schema_version(conn).expect("读取版本"),
        1,
        "夹具必须停在 v1"
    );
}

/// 构造一个处于 v2 的工作区：在 v1 基础上执行真实 v2 SQL 并登记迁移记录。
fn create_v2_workspace(conn: &Connection) {
    create_v1_workspace(conn);
    conn.execute_batch(V2_SQL).expect("应用 v2 SQL");
    conn.execute(
        "INSERT INTO schema_migrations (version, applied_at, checksum)
         VALUES (2, current_timestamp, ?)",
        params![whcode_store::checksum(V2_SQL)],
    )
    .expect("登记 v2 迁移");
    assert_eq!(
        whcode_store::schema_version(conn).expect("读取版本"),
        2,
        "夹具必须停在 v2"
    );
}

fn table_count(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT count(*) FROM information_schema.tables
         WHERE table_schema = 'main' AND table_type = 'BASE TABLE'",
        [],
        |row| row.get(0),
    )
    .expect("统计表数量")
}

fn migration_rows(conn: &Connection) -> i64 {
    conn.query_row("SELECT count(*) FROM schema_migrations", [], |row| {
        row.get(0)
    })
    .expect("迁移记录")
}

fn index_exists(conn: &Connection, table: &str, index: &str) -> bool {
    conn.query_row(
        "SELECT count(*) FROM duckdb_indexes()
         WHERE table_name = ? AND index_name = ?",
        params![table, index],
        |row| row.get::<_, i64>(0),
    )
    .expect("查询索引")
        > 0
}

fn event_count(conn: &Connection) -> i64 {
    conn.query_row("SELECT count(*) FROM events", [], |row| row.get(0))
        .expect("统计事件")
}

#[test]
fn fresh_workspace_migrates_to_current_and_reruns_idempotently() {
    let conn = Connection::open_in_memory().expect("内存数据库");

    let first = whcode_store::apply_migrations(&conn).expect("首次迁移");
    assert_eq!(first, whcode_store::STORE_SCHEMA_VERSION);
    assert_eq!(table_count(&conn) as usize, whcode_store::TABLE_COUNT);
    assert_eq!(migration_rows(&conn), 6, "每个 schema 版本恰一行记录");

    let duplicate = whcode_store::apply_migrations(&conn).expect("重复迁移");
    assert_eq!(duplicate, first);
    assert_eq!(migration_rows(&conn), 6, "重复应用不得重复写入迁移记录");

    for (version, sql) in [
        (1, V1_SQL),
        (2, V2_SQL),
        (3, V3_SQL),
        (4, V4_SQL),
        (5, V5_SQL),
        (6, V6_SQL),
    ] {
        let checksum: String = conn
            .query_row(
                "SELECT checksum FROM schema_migrations WHERE version = ?",
                params![version],
                |row| row.get(0),
            )
            .expect("迁移校验和");
        assert_eq!(
            checksum,
            whcode_store::checksum(sql),
            "v{version} 校验和必须与仓库中的脚本一致"
        );
    }

    assert!(index_exists(&conn, "events", "idx_events_request"));
    assert!(index_exists(&conn, "events", "idx_events_tool_call"));
    assert!(index_exists(
        &conn,
        "messages",
        "idx_messages_session_ts_id"
    ));
    assert!(index_exists(&conn, "tasks", "idx_tasks_session_stage"));
    assert!(
        !index_exists(&conn, "messages", "idx_messages_session_ts"),
        "v3 必须删除被三列索引覆盖的旧消息索引"
    );
}

#[test]
fn v1_workspace_upgrades_to_current_and_preserves_data() {
    let conn = Connection::open_in_memory().expect("内存数据库");
    create_v1_workspace(&conn);

    conn.execute(
        "INSERT INTO events
         (seq, id, session_id, task_id, request_id, tool_call_id, kind, payload_json, ts,
          persistence_level)
         VALUES (1, 'E-1', 'S-1', NULL, 'R-1', 'T-1', 'ToolCallStarted', '{}',
                 current_timestamp, 'full')",
        [],
    )
    .expect("写入 v1 数据");
    assert!(
        !index_exists(&conn, "events", "idx_events_request"),
        "v1 不应有新索引"
    );

    let version = whcode_store::apply_migrations(&conn).expect("v1 → 当前版本升级");
    assert_eq!(version, whcode_store::STORE_SCHEMA_VERSION);
    assert_eq!(migration_rows(&conn), 6, "跨版本升级应补齐全部迁移记录");
    assert_eq!(event_count(&conn), 1, "升级不得丢失既有数据");
    assert!(index_exists(&conn, "events", "idx_events_request"));
    assert!(index_exists(&conn, "events", "idx_events_tool_call"));
    assert!(index_exists(
        &conn,
        "messages",
        "idx_messages_session_ts_id"
    ));
    assert!(index_exists(&conn, "tasks", "idx_tasks_session_stage"));

    let stored: String = conn
        .query_row("SELECT request_id FROM events WHERE seq = 1", [], |row| {
            row.get(0)
        })
        .expect("读取既有事件");
    assert_eq!(stored, "R-1");
}

#[test]
fn v2_workspace_upgrades_to_current_and_preserves_data() {
    let conn = Connection::open_in_memory().expect("内存数据库");
    create_v2_workspace(&conn);

    conn.execute(
        "INSERT INTO messages (id, session_id, role, content_ref, token_count, ts, parent_id)
         VALUES ('M-1', 'S-1', 'user', 'ref-1', NULL,
                 CAST('2026-01-01 00:00:00.000001' AS TIMESTAMP), NULL)",
        [],
    )
    .expect("写入 v2 消息");
    assert!(
        index_exists(&conn, "messages", "idx_messages_session_ts"),
        "v2 应保留旧消息索引"
    );
    assert!(
        !index_exists(&conn, "messages", "idx_messages_session_ts_id"),
        "v2 不应有三列消息索引"
    );

    let version = whcode_store::apply_migrations(&conn).expect("v2 → 当前版本升级");
    assert_eq!(version, whcode_store::STORE_SCHEMA_VERSION);
    assert_eq!(
        whcode_store::schema_version(&conn).expect("版本"),
        whcode_store::STORE_SCHEMA_VERSION
    );
    let messages: i64 = conn
        .query_row("SELECT count(*) FROM messages", [], |row| row.get(0))
        .expect("统计消息");
    assert_eq!(messages, 1, "升级不得丢失既有消息");
    assert!(index_exists(
        &conn,
        "messages",
        "idx_messages_session_ts_id"
    ));
    assert!(
        !index_exists(&conn, "messages", "idx_messages_session_ts"),
        "v3 应删除旧两列索引"
    );
}

#[test]
fn interrupted_v2_migration_rolls_back_partial_changes() {
    let conn = Connection::open_in_memory().expect("内存数据库");
    create_v1_workspace(&conn);

    // 人为制造故障：把 events 换成缺少 tool_call_id 的同名表，使 v2 在第一条索引成功后失败。
    // （不能直接 DROP COLUMN：DuckDB 拒绝改动仍被索引依赖的表。）
    conn.execute_batch(
        "DROP TABLE events;
         CREATE TABLE events (
             seq               BIGINT PRIMARY KEY,
             id                TEXT NOT NULL UNIQUE,
             session_id        TEXT,
             task_id           TEXT,
             request_id        TEXT,
             kind              TEXT NOT NULL,
             payload_json      TEXT NOT NULL,
             ts                TIMESTAMP NOT NULL,
             persistence_level TEXT NOT NULL
         );",
    )
    .expect("替换 events 表以注入故障");

    let err = whcode_store::apply_migrations(&conn).expect_err("v2 必须失败");
    assert_eq!(err.code, whcode_domain::ErrorCode::IoError);
    assert!(err.retryable, "迁移失败属 I/O 类，可按修复后重试");

    assert_eq!(
        whcode_store::schema_version(&conn).expect("版本"),
        1,
        "失败迁移不得把版本前移到 v2"
    );
    assert_eq!(migration_rows(&conn), 1, "失败迁移不得写入版本记录");
    assert!(
        !index_exists(&conn, "events", "idx_events_request"),
        "v2 第一条索引必须随事务回滚，不得残留半成品"
    );
    assert!(!index_exists(&conn, "events", "idx_events_tool_call"));

    // 原库仍可读写：失败后不得把工作区变成不可用状态。
    conn.execute(
        "INSERT INTO sessions (id, title, created_at, last_active_at, status)
         VALUES ('S-1', '失败后仍可写', current_timestamp, current_timestamp, 'active')",
        [],
    )
    .expect("失败迁移后原库仍可写");
    let sessions: i64 = conn
        .query_row("SELECT count(*) FROM sessions", [], |row| row.get(0))
        .expect("统计会话");
    assert_eq!(sessions, 1);
}

#[test]
fn interrupted_v3_migration_rolls_back_index_replacement() {
    let conn = Connection::open_in_memory().expect("内存数据库");
    create_v2_workspace(&conn);
    assert!(
        index_exists(&conn, "messages", "idx_messages_session_ts"),
        "夹具必须处于 v2 索引状态"
    );

    // 人为制造故障：把 tasks 换成缺少 stage 的同名表，使 v3 在“删旧索引、建新索引”后失败。
    conn.execute_batch(
        "DROP TABLE tasks;
         CREATE TABLE tasks (
             id              TEXT PRIMARY KEY,
             session_id      TEXT NOT NULL,
             title           TEXT NOT NULL,
             status          TEXT NOT NULL,
             parent_id       TEXT,
             acceptance_json TEXT NOT NULL,
             retry_count     INTEGER NOT NULL DEFAULT 0,
             updated_at      TIMESTAMP NOT NULL
         );",
    )
    .expect("替换 tasks 表以注入故障");

    let err = whcode_store::apply_migrations(&conn).expect_err("v3 必须失败");
    assert_eq!(err.code, whcode_domain::ErrorCode::IoError);

    assert_eq!(
        whcode_store::schema_version(&conn).expect("版本"),
        2,
        "失败迁移不得把版本前移到 v3"
    );
    assert_eq!(migration_rows(&conn), 2, "失败迁移不得写入版本记录");
    assert!(
        index_exists(&conn, "messages", "idx_messages_session_ts"),
        "v3 删除的旧索引必须随事务回滚恢复"
    );
    assert!(
        !index_exists(&conn, "messages", "idx_messages_session_ts_id"),
        "v3 新建索引不得残留"
    );
    assert!(!index_exists(&conn, "tasks", "idx_tasks_session_stage"));

    conn.execute(
        "INSERT INTO sessions (id, title, created_at, last_active_at, status)
         VALUES ('S-1', '失败后仍可写', current_timestamp, current_timestamp, 'active')",
        [],
    )
    .expect("失败迁移后原库仍可写");
}

#[test]
fn version_matches_latest_migration() {
    let conn = Connection::open_in_memory().expect("内存数据库");
    whcode_store::apply_migrations(&conn).expect("迁移");
    assert_eq!(
        whcode_store::schema_version(&conn).expect("版本"),
        whcode_store::STORE_SCHEMA_VERSION
    );
}

#[test]
fn v3_workspace_upgrades_to_v4_and_sequence_is_ready() {
    let conn = Connection::open_in_memory().expect("内存数据库");
    create_v2_workspace(&conn);
    conn.execute_batch(V3_SQL).expect("应用 v3");
    conn.execute(
        "INSERT INTO schema_migrations (version, applied_at, checksum)
         VALUES (3, current_timestamp, ?)",
        params![whcode_store::checksum(V3_SQL)],
    )
    .expect("登记 v3 迁移");

    let version = whcode_store::apply_migrations(&conn).expect("v3 → v4 升级");
    assert_eq!(version, whcode_store::STORE_SCHEMA_VERSION);
    assert_eq!(migration_rows(&conn), 6);

    // v4 的产出：会话 ID 序列可用且单调；旧数据不受影响。
    let first: String = conn
        .query_row(
            "INSERT INTO sessions (id, title, created_at, last_active_at, status)
             VALUES ('S-' || nextval('whcode_session_id_seq'), 'v4 序列', current_timestamp,
                     current_timestamp, 'active') RETURNING id",
            [],
            |row| row.get(0),
        )
        .expect("使用序列生成会话 ID");
    assert!(first.starts_with("S-"), "{first}");
    let second: String = conn
        .query_row(
            "INSERT INTO sessions (id, title, created_at, last_active_at, status)
             VALUES ('S-' || nextval('whcode_session_id_seq'), 'v4 序列二', current_timestamp,
                     current_timestamp, 'active') RETURNING id",
            [],
            |row| row.get(0),
        )
        .expect("第二次使用序列");
    assert_ne!(first, second, "序列必须单调不重复");
}

#[test]
fn v4_migration_is_idempotent_against_existing_sequence() {
    let conn = Connection::open_in_memory().expect("内存数据库");
    create_v3_workspace();
    // 预置同名序列（从 99 起）：IF NOT EXISTS 不得报错，也不得重置既有序列的计数。
    conn.execute_batch("CREATE SEQUENCE whcode_session_id_seq START 99;")
        .expect("预置同名序列");
    let before: i64 = conn
        .query_row("SELECT nextval('whcode_session_id_seq')", [], |row| {
            row.get(0)
        })
        .expect("消耗一个值");
    assert_eq!(before, 99);

    let version = whcode_store::apply_migrations(&conn).expect("幂等通过");
    assert_eq!(version, whcode_store::STORE_SCHEMA_VERSION);
    assert_eq!(migration_rows(&conn), 6);

    let after: i64 = conn
        .query_row("SELECT nextval('whcode_session_id_seq')", [], |row| {
            row.get(0)
        })
        .expect("再次取值");
    assert_eq!(
        after, 100,
        "重复迁移不得重置既有序列的计数（若被重建将回到 99）"
    );
}

fn create_v3_workspace() {
    let conn = Connection::open_in_memory().expect("内存数据库");
    create_v2_workspace(&conn);
    conn.execute_batch(V3_SQL).expect("应用 v3");
    conn.execute(
        "INSERT INTO schema_migrations (version, applied_at, checksum)
         VALUES (3, current_timestamp, ?)",
        params![whcode_store::checksum(V3_SQL)],
    )
    .expect("登记 v3 迁移");
}

#[test]
fn v4_workspace_upgrades_to_v5_and_stage_column_is_backfilled() {
    let conn = Connection::open_in_memory().expect("内存数据库");
    create_v2_workspace(&conn);
    conn.execute_batch(V3_SQL).expect("应用 v3");
    conn.execute(
        "INSERT INTO schema_migrations (version, applied_at, checksum)
         VALUES (3, current_timestamp, ?)",
        params![whcode_store::checksum(V3_SQL)],
    )
    .expect("登记 v3");
    conn.execute_batch(V4_SQL).expect("应用 v4");
    conn.execute(
        "INSERT INTO schema_migrations (version, applied_at, checksum)
         VALUES (4, current_timestamp, ?)",
        params![whcode_store::checksum(V4_SQL)],
    )
    .expect("登记 v4");
    conn.execute(
        "INSERT INTO sessions (id, title, created_at, last_active_at, status)
         VALUES ('S-1', 'v4 会话', current_timestamp, current_timestamp, 'active')",
        [],
    )
    .expect("写入 v4 会话");

    let version = whcode_store::apply_migrations(&conn).expect("v4 → v5 升级");
    assert_eq!(version, whcode_store::STORE_SCHEMA_VERSION);
    assert_eq!(migration_rows(&conn), 6);

    // v5 产出：stage 列存在且既有行回填默认值（DuckDB ALTER 限制：可空列 + UPDATE）。
    let stage: String = conn
        .query_row("SELECT stage FROM sessions WHERE id = 'S-1'", [], |row| {
            row.get(0)
        })
        .expect("读取回填阶段");
    assert_eq!(stage, "specify", "既有行必须回填默认阶段");
}

/// 构造一个处于 v5 的工作区：v1—v5 全部应用并登记，事件载荷仍是文本。
fn create_v5_workspace(conn: &Connection) {
    create_v2_workspace(conn);
    for (version, sql) in [(3, V3_SQL), (4, V4_SQL), (5, V5_SQL)] {
        conn.execute_batch(sql).expect("应用历史迁移");
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at, checksum)
             VALUES (?, current_timestamp, ?)",
            params![version, whcode_store::checksum(sql)],
        )
        .expect("登记历史迁移");
    }
    assert_eq!(
        whcode_store::schema_version(conn).expect("读取版本"),
        5,
        "夹具必须停在 v5"
    );
}

#[test]
fn v5_workspace_upgrades_to_v6_and_payload_becomes_native_variant() {
    let conn = Connection::open_in_memory().expect("内存数据库");
    create_v5_workspace(&conn);

    conn.execute(
        "INSERT INTO events
         (seq, id, session_id, task_id, request_id, tool_call_id, kind, payload_json, ts,
          persistence_level)
         VALUES (1, 'E-1', 'S-1', 'T-1', 'R-1', 'C-1', 'ToolCallStarted',
                 '{\"b\":1,\"a\":{\"x\":[1,2]}}', current_timestamp, 'full')",
        [],
    )
    .expect("写入 v5 文本载荷");

    let text_type: String = conn
        .query_row(
            "SELECT data_type FROM information_schema.columns
             WHERE table_name = 'events' AND column_name = 'payload_json'",
            [],
            |row| row.get(0),
        )
        .expect("查询 v5 列类型");
    assert_eq!(text_type, "VARCHAR", "v5 的载荷列必须是文本");

    let version = whcode_store::apply_migrations(&conn).expect("v5 → v6 升级");
    assert_eq!(version, whcode_store::STORE_SCHEMA_VERSION);
    assert_eq!(migration_rows(&conn), 6, "v6 迁移记录必须写入");
    assert_eq!(event_count(&conn), 1, "升级不得丢失既有事件");
    for index in [
        "idx_events_session_seq",
        "idx_events_task",
        "idx_events_request",
        "idx_events_tool_call",
    ] {
        assert!(
            index_exists(&conn, "events", index),
            "v6 必须重建事件索引 {index}"
        );
    }

    let (payload, variant_kind, request_id): (String, String, String) = conn
        .query_row(
            "SELECT CAST(CAST(payload_json AS JSON) AS VARCHAR), variant_type(payload_json),
                    request_id
             FROM events WHERE seq = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("读回 v6 事件");
    assert_eq!(
        variant_kind, "OBJECT",
        "载荷必须是原生 OBJECT 变体，而非包着 JSON 的 VARCHAR 变体"
    );
    assert_eq!(request_id, "R-1", "追溯列必须随复制保留");

    // VARIANT 会规范化对象键序，因此按 JSON 值比较，不对文本顺序做假设。
    let payload: serde_json::Value = serde_json::from_str(&payload).expect("载荷必须是 JSON 文本");
    assert_eq!(payload, serde_json::json!({"a": {"x": [1, 2]}, "b": 1}));
}
