//! # 稳定性故障情形（Check 206、§11.5）
//!
//! 四类故障的最小可复现用例：LLM 请求失败、断网、数据库写入失败、磁盘空间不足。
//! 判据是“结构化错误 + 不静默 + 不产生半完成状态”，不是“没有崩溃”。
//!
//! 真实满盘卷需要专用环境：设置 `WHCODE_DISK_FULL_DIR` 指向已耗尽空间的目录后，
//! 用例会执行真实写入并断言错误分类；未设置时显式打印“未执行”，不伪造通过。

use std::io::ErrorKind;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use whcode_domain::{ErrorCode, WhError};
use whcode_llm::{CancelToken, ChunkStream, LlmRequest, Provider, ProviderCapabilities};

/// 请求失败的 Provider（模拟上游 5xx／限流／超时后的归一化失败）。
struct FailingProvider {
    code: ErrorCode,
}

impl Provider for FailingProvider {
    fn name(&self) -> &str {
        "failing-stub"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: false,
            cancel: false,
            usage_reporting: false,
        }
    }

    fn is_available(&self) -> bool {
        true
    }

    fn execute(
        &self,
        _request: &LlmRequest,
        _cancel: &CancelToken,
    ) -> Result<ChunkStream, WhError> {
        Err(WhError::new(self.code, "上游返回 503（测试注入）")
            .with_details(serde_json::json!({ "status": 503, "provider": "failing-stub" })))
    }
}

fn request() -> LlmRequest {
    LlmRequest {
        request_id: "R-1".to_string(),
        session_id: "S-1".to_string(),
        task_id: None,
        model: "test-model".to_string(),
        prompt: "hello".to_string(),
        max_tokens: 16,
        timeout: None,
    }
}

/// 场景 1：LLM 请求失败——结构化错误、原文可追溯、重试语义按错误码而非“一律重试”。
#[test]
fn llm_request_failure_is_structured_and_retry_semantics_are_explicit() {
    let provider = FailingProvider {
        code: ErrorCode::RuntimeUnavailable,
    };
    let err = provider
        .execute(&request(), &CancelToken::default())
        .expect_err("上游失败必须返回错误");
    assert_eq!(err.code, ErrorCode::RuntimeUnavailable);
    assert!(!err.retryable, "能力/配置类失败不得盲目重试");
    assert_eq!(
        err.details.as_ref().expect("details 保留原文")["status"],
        503
    );

    let provider = FailingProvider {
        code: ErrorCode::Timeout,
    };
    let err = provider
        .execute(&request(), &CancelToken::default())
        .expect_err("超时必须返回错误");
    assert!(err.retryable, "超时属可重试类别");
}

/// 场景 2：断网——连接失败有界且可归类；恢复后按游标补拉，不重复、不缺失。
#[test]
fn offline_network_failure_is_bounded_and_replay_has_no_gaps() {
    use duckdb::params;
    use whcode_app::authz::{DefaultPolicy, actor_with_all_capabilities};
    use whcode_app::replay::{REPLAY_PAGE_LIMIT, replay_since};
    use whcode_store::{apply_migrations, create_session_with_id};

    // 取一个刚释放的本机端口：连接必然失败（连接被拒或超时）。
    let listener = TcpListener::bind("127.0.0.1:0").expect("绑定临时端口");
    let address = listener.local_addr().expect("本地地址");
    drop(listener);

    let started = Instant::now();
    let result = TcpStream::connect_timeout(&address, Duration::from_secs(2));
    let elapsed = started.elapsed();
    let err = result.expect_err("对端不可用必须返回错误");
    assert!(
        elapsed < Duration::from_secs(5),
        "网络失败必须有界（实际 {elapsed:?}）"
    );
    assert!(
        matches!(
            err.kind(),
            ErrorKind::ConnectionRefused
                | ErrorKind::TimedOut
                | ErrorKind::ConnectionAborted
                | ErrorKind::ConnectionReset
        ),
        "错误类别应可归类：{:?}",
        err.kind()
    );

    // 断网不丢数据：恢复后从游标补拉，恰好补齐剩余事件。
    let conn = duckdb::Connection::open_in_memory().expect("内存数据库");
    apply_migrations(&conn).expect("迁移");
    create_session_with_id(&conn, "S-1", "断网", "active", 1_000).expect("创建会话");
    for seq in 1..=10 {
        conn.execute(
            "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                                 kind, payload_json, ts, persistence_level)
             VALUES (?, ?, 'S-1', NULL, NULL, NULL, 'StreamingChunk', '{}',
                     current_timestamp, 'redacted')",
            params![seq, format!("E-{seq}")],
        )
        .expect("插入事件");
    }
    let page = replay_since(
        &conn,
        &DefaultPolicy,
        &actor_with_all_capabilities("tester"),
        "S-1",
        4,
        REPLAY_PAGE_LIMIT,
    )
    .expect("补拉");
    assert!(page.complete);
    assert_eq!(
        page.events.iter().map(|row| row.seq).collect::<Vec<_>>(),
        (5..=10).collect::<Vec<_>>(),
        "补拉从游标之后开始，无重复无缺失"
    );
}

/// 场景 3：数据库写入失败——只读库写入返回结构化错误，且不留下半完成状态。
#[test]
fn read_only_database_write_failure_is_structured_and_leaves_no_partial_state() {
    let path = unique_temp_path("readonly.duckdb");
    {
        let conn = duckdb::Connection::open(&path).expect("创建数据库");
        whcode_store::apply_migrations(&conn).expect("迁移");
    }

    let config = duckdb::Config::default()
        .access_mode(duckdb::AccessMode::ReadOnly)
        .expect("只读配置");
    let conn = duckdb::Connection::open_with_flags(&path, config).expect("只读打开");
    let err = whcode_store::create_session_with_id(&conn, "S-1", "只读写入", "active", 1_000)
        .expect_err("只读库必须拒绝写入");
    assert_eq!(err.code, ErrorCode::IoError, "{err}");
    assert!(!err.message.is_empty(), "错误必须可操作");

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
        .expect("查询会话数");
    assert_eq!(count, 0, "写入失败不得留下半完成状态");

    drop(conn);
    let _ = std::fs::remove_file(&path);
}

/// 场景 4：磁盘空间不足——满盘错误单独分类且可重试（含 Windows `ERROR_DISK_FULL`）。
///
/// 真实满盘写入仅在设置 `WHCODE_DISK_FULL_DIR` 时执行。
#[test]
fn disk_full_is_classified_and_reported_without_silence() {
    let err = WhError::io(
        "写出导出文件",
        &std::io::Error::new(ErrorKind::StorageFull, "no space left on device"),
    );
    assert_eq!(err.code, ErrorCode::IoError);
    assert!(err.retryable, "满盘属环境问题，清理空间后可重试");
    assert!(err.message.contains("磁盘空间不足"), "{err}");
    assert!(
        err.message.contains("清理空间"),
        "错误消息必须可操作：{err}"
    );

    // Windows 的 ERROR_DISK_FULL=112 归入 Other：仍须按满盘处置。
    let err = WhError::io("写出导出文件", &std::io::Error::from_raw_os_error(112));
    assert!(err.message.contains("磁盘空间不足"), "{err}");

    let Some(dir) = std::env::var_os("WHCODE_DISK_FULL_DIR").map(PathBuf::from) else {
        eprintln!("未执行真实满盘写入：设置 WHCODE_DISK_FULL_DIR 指向已耗尽空间的目录后重跑本用例");
        return;
    };
    let path = dir.join("whcode-disk-full-probe.bin");
    let mut file = std::fs::File::create(&path).expect("创建探测文件");
    let chunk = vec![0_u8; 8 * 1024 * 1024];
    let mut failure: Option<std::io::Error> = None;
    for _ in 0..1024 {
        use std::io::Write;
        if let Err(err) = file.write_all(&chunk).and_then(|()| file.flush()) {
            failure = Some(err);
            break;
        }
    }
    drop(file);
    let _ = std::fs::remove_file(&path);
    let failure = failure.expect("已耗尽空间的卷上写入必须以错误结束");
    let err = WhError::io("真实满盘写入", &failure);
    assert!(err.message.contains("磁盘空间不足"), "{err}");
    assert!(err.retryable);
}

/// 唯一临时路径（进程 ID + 时间戳，避免并发用例互相覆盖）。
fn unique_temp_path(name: &str) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis());
    std::env::temp_dir().join(format!(
        "whcode-stability-{}-{stamp}-{name}",
        std::process::id()
    ))
}
