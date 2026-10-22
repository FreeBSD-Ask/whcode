//! 持久性集成测试（T043、V-104）：已确认事务的进程终止 + 重启一致性。
//!
//! **子进程模式**：父测试把当前测试二进制以
//! `child_committed_survives_abort --exact --ignored` 再启动一次；子进程向文件库提交
//! 20 个关键事件并逐条收到 `Committed` 确认后调用 `std::process::abort()` 模拟进程
//! 终止——`abort` 不运行任何析构，等价于进程被强杀，连接不关闭、WAL 不检查点。
//! 父进程随后重开数据库断言：全部已确认事件完好、`seq` 唯一无歧义，且按幂等键重放
//! 返回既有 `seq`（V-106 关联）。
//!
//! 确认边界：仅**已确认**（收到 `Committed`）的事件参与存活断言；仍在线冲缓冲中的
//! 非关键事件允许丢失（FR-502、ADR-017），不参与断言。
//!
//! 子进程测试标注 `#[ignore]`：正常 `cargo test` 跳过；只由父测试带环境变量调用，
//! 直接以 `--ignored` 运行时也会静默返回（不污染交互运行）。

use std::path::PathBuf;
use std::process::Command;

use duckdb::Connection;
use whcode_domain::{EventKind, EventRecord, PersistenceLevel, SCHEMA_VERSION};
use whcode_store::{WriteOutcome, WriterConfig, WriterService, apply_migrations};

const CONFIRMED_EVENTS: u64 = 20;
const KEY_PREFIX: &str = "dur-k-";

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("whcode-durability-{}-{tag}", std::process::id()));
    // 上次运行被中断（进程被杀）时目录会遗留；Windows 会复用 PID，先清再建（夹具幂等）。
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("创建临时目录");
    dir
}

fn event_record(seq: u64, kind: EventKind) -> EventRecord {
    EventRecord {
        schema_version: SCHEMA_VERSION,
        seq,
        id: format!("E-{seq}"),
        kind,
        session_id: Some("S-1".to_string()),
        task_id: None,
        request_id: None,
        tool_call_id: None,
        payload: serde_json::json!({"n": seq}),
        ts_ms: 1_700_000_000_000 + seq,
        persistence_level: PersistenceLevel::Redacted,
    }
}

/// 子进程入口：提交并确认关键事件后强杀自身。
#[test]
#[ignore = "仅由 committed_transactions_survive_process_abort 以子进程调用"]
fn child_committed_survives_abort() {
    if std::env::var("WHCODE_DURABILITY_CHILD").is_err() {
        return; // 直接 --ignored 运行：静默跳过，不执行任何副作用。
    }
    let db = std::env::var("WHCODE_DURABILITY_DB").expect("父进程须提供数据库路径");
    let conn = Connection::open(&db).expect("子进程打开库");
    apply_migrations(&conn).expect("子进程迁移");
    let service = WriterService::start(conn, WriterConfig::default()).expect("启动写者");

    for seq in 1..=CONFIRMED_EVENTS {
        let outcome = service
            .submit(
                event_record(seq, EventKind::ToolCallStarted),
                format!("{KEY_PREFIX}{seq}"),
            )
            .expect("关键事件提交");
        assert_eq!(
            outcome,
            WriteOutcome::Committed { seq },
            "每个关键事件都必须在 abort 前得到提交确认"
        );
    }
    // 非关键事件进入缓冲：abort 后允许丢失，不参与存活断言（确认边界）。
    for seq in (CONFIRMED_EVENTS + 1)..=(CONFIRMED_EVENTS + 5) {
        let _ = service.submit(
            event_record(seq, EventKind::StreamingChunk),
            format!("{KEY_PREFIX}{seq}"),
        );
    }
    eprintln!("子进程：{CONFIRMED_EVENTS} 个关键事件已确认，即将 abort");
    std::process::abort();
}

#[test]
fn committed_transactions_survive_process_abort() {
    let dir = temp_dir("parent");
    let db = dir.join("durability.duckdb");
    {
        let conn = Connection::open(&db).expect("父进程建库");
        apply_migrations(&conn).expect("迁移");
    }
    assert!(
        std::fs::File::open(&db).is_ok(),
        "OS 层文件必须可打开（父进程连接必须已释放锁）"
    );

    let exe = std::env::current_exe().expect("定位当前测试二进制");
    let output = Command::new(exe)
        .args([
            "child_committed_survives_abort",
            "--exact",
            "--ignored",
            "--nocapture",
        ])
        .env("WHCODE_DURABILITY_CHILD", "1")
        .env("WHCODE_DURABILITY_DB", &db)
        .output()
        .expect("启动子进程");
    assert!(
        !output.status.success(),
        "子进程必须以 abort 非正常终止；实际 {output:?}；子进程 stderr：{}",
        String::from_utf8_lossy(&output.stderr)
    );

    // 进程终止后重开：WAL 重放必须还原全部已确认事务。
    // 纪律：DuckDB 每文件同时只允许一个写连接——本测试内先断言、后释放、再重放。
    let conn = Connection::open(&db).expect("重开库");
    let total: i64 = conn
        .query_row("SELECT count(*) FROM m0_events", [], |row| row.get(0))
        .expect("统计事件");
    assert!(
        total >= i64::try_from(CONFIRMED_EVENTS).expect("数量可转换"),
        "已确认事件必须全部存活（实际 {total}，上限为确认数+缓冲数）"
    );
    assert!(
        total <= i64::try_from(CONFIRMED_EVENTS + 5).expect("数量可转换"),
        "存活事件不得超出提交过的范围"
    );
    for seq in 1..=CONFIRMED_EVENTS {
        let present: i64 = conn
            .query_row(
                "SELECT count(*) FROM m0_events WHERE idempotency_key = ?",
                [format!("{KEY_PREFIX}{seq}")],
                |row| row.get(0),
            )
            .expect("按键查询");
        assert_eq!(present, 1, "已确认事件 E-{seq} 必须恰好存在一次");
    }
    let distinct: i64 = conn
        .query_row("SELECT count(DISTINCT seq) FROM m0_events", [], |row| {
            row.get(0)
        })
        .expect("统计 seq");
    assert_eq!(distinct, total, "重启后 seq 必须唯一无歧义");

    // 幂等重放：终止前确认过的键重放返回既有 seq，不重复创建（V-106）。
    // 先释放统计用连接，再为写者打开新连接（单写者纪律）。
    drop(conn);
    let conn2 = Connection::open(&db).expect("重放阶段重开库");
    let service = WriterService::start(conn2, WriterConfig::default()).expect("重启写者");
    let outcome = service
        .submit(
            event_record(1, EventKind::ToolCallStarted),
            format!("{KEY_PREFIX}1"),
        )
        .expect("重放提交");
    assert!(
        matches!(outcome, WriteOutcome::Deduplicated { seq: 1 }),
        "重放必须返回既有 seq=1，实际 {outcome:?}"
    );
    service.shutdown().expect("关闭写者");

    let _ = std::fs::remove_dir_all(&dir);
}
