//! Web 演示工作区夹具（浏览器端到端测试用；**可重跑、幂等**）。
//!
//! 生成 `artifacts/web-demo/whcode.duckdb`：两个演示会话 + 一个有界事件尾部
//! （`RequestStarted → StreamingChunk×3 → ToolCallStarted → ToolCallFinished →
//! RequestCompleted`），供 Web 界面验证会话清单、事件游标推进与状态快照。
//!
//! 纪律：
//! - 这不是验收断言测试，而是**夹具生成器**（每次运行重建演示数据；删除+重建保证幂等）；
//! - `events.seq` 为工作区全局主键：新事件从 `MAX(seq)` 之后分配（不假设从 1 开始）；
//! - 事件类型使用 `EventKind` 的稳定名（与真实写入路径同一枚举），不伪造类型。
//!
//! 运行：`cargo test -p whcode-tests --test seed_web_demo`（先于浏览器用例执行）。

use std::path::PathBuf;

use duckdb::{Connection, params};
use whcode_store::apply_migrations;

/// 演示会话（ID 固定，便于浏览器用例断言与重复清理）。
const DEMO_SESSIONS: [(&str, &str, &str, &str); 2] = [
    ("S-DEMO-1", "演示：六视图与 Web 联调", "implement", "active"),
    ("S-DEMO-2", "演示：归档候选", "specify", "idle"),
];

/// 演示事件序列（类型, 载荷）——有界尾部，模拟一次完整请求生命周期。
const DEMO_EVENTS: [(&str, &str); 6] = [
    ("RequestStarted", r#"{"model":"demo"}"#),
    (
        "StreamingChunk",
        r#"{"text":"正在读取 specs/001-whcode-harness/spec.md"}"#,
    ),
    ("StreamingChunk", r#"{"text":"已定位 FR-601 六视图要求"}"#),
    ("StreamingChunk", r#"{"text":"生成本次响应"}"#),
    (
        "ToolCallStarted",
        r#"{"tool":"read_file","path":"spec.md"}"#,
    ),
    ("RequestCompleted", r#"{"tokens":128}"#),
];

fn demo_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../artifacts/web-demo");
    std::fs::create_dir_all(&dir).expect("创建演示目录");
    dir
}

#[test]
fn seed_demo_workspace_is_idempotent() {
    let db = demo_dir().join("whcode.duckdb");
    let conn = Connection::open(&db).expect("打开演示库");
    apply_migrations(&conn).expect("迁移（幂等）");

    // 清理演示数据（可重跑：只删演示 ID，不动其他数据）。
    conn.execute(
        "DELETE FROM events WHERE session_id = 'S-DEMO-1' OR session_id = 'S-DEMO-2'",
        [],
    )
    .expect("清理演示事件");
    conn.execute(
        "DELETE FROM sessions WHERE id = 'S-DEMO-1' OR id = 'S-DEMO-2'",
        [],
    )
    .expect("清理演示会话");

    let now_ms = 1_761_000_000_000i64;
    for (id, title, stage, status) in DEMO_SESSIONS {
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, last_active_at, model, archived, status, stage)
             VALUES (?, ?, epoch_ms(?), epoch_ms(?), NULL, FALSE, ?, ?)",
            params![id, title, now_ms, now_ms, status, stage],
        )
        .expect("插入演示会话");
    }

    let base: i64 = conn
        .query_row("SELECT COALESCE(MAX(seq), 0) FROM events", [], |row| {
            row.get(0)
        })
        .expect("读取当前最大 seq");
    for (offset, (kind, payload)) in DEMO_EVENTS.iter().enumerate() {
        let seq = base + 1 + i64::try_from(offset).expect("偏移在 i64 内");
        conn.execute(
            "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                                 kind, payload_json, ts, persistence_level)
             VALUES (?, ?, 'S-DEMO-1', NULL, NULL, NULL, ?, ?, current_timestamp, 'redacted')",
            params![seq, format!("E-DEMO-{seq}"), kind, payload],
        )
        .expect("插入演示事件");
    }

    // 自检：读回数量与最新游标（夹具必须真的可用，不是“写入成功”就算）。
    let sessions: i64 = conn
        .query_row(
            "SELECT count(*) FROM sessions WHERE id IN ('S-DEMO-1','S-DEMO-2')",
            [],
            |row| row.get(0),
        )
        .expect("统计会话");
    assert_eq!(sessions, 2, "两个演示会话必须存在");
    let events: i64 = conn
        .query_row(
            "SELECT count(*) FROM events WHERE session_id = 'S-DEMO-1'",
            [],
            |row| row.get(0),
        )
        .expect("统计事件");
    assert_eq!(
        events,
        i64::try_from(DEMO_EVENTS.len()).expect("长度"),
        "演示事件数量必须与夹具一致"
    );
    let cursor: i64 = conn
        .query_row(
            "SELECT MAX(seq) FROM events WHERE session_id = 'S-DEMO-1'",
            [],
            |row| row.get(0),
        )
        .expect("读取游标");
    assert_eq!(
        cursor,
        base + i64::try_from(DEMO_EVENTS.len()).expect("长度")
    );

    eprintln!(
        "Web 演示工作区就绪：{}（S-DEMO-1 / S-DEMO-2，事件 {} 条，游标 {}）",
        db.display(),
        DEMO_EVENTS.len(),
        cursor
    );
    eprintln!("启动服务：whcode serve --web --bind 127.0.0.1:8737 --workspace artifacts/web-demo");
}
