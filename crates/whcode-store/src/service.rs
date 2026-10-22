//! # 写者服务（T034，ADR-017）
//!
//! 专用写线程独占 [`duckdb::Connection`]，调用方经**有界通道**提交请求并等待确认：
//!
//! ```text
//! submit --try_send--> [有界通道] --> 写线程（唯一写者） --COMMIT--> 确认
//! ```
//!
//! 设计约束（ADR-017）：
//! - 队列满返回背压错误，不无限阻塞调用方；
//! - 关键事件立即 flush 并在 COMMIT 后确认；
//! - 非关键事件可先返回 [`WriteOutcome::Buffered`]，由行数阈值或时间窗口触发提交；
//! - 失败批次保留在写线程内，调用方可依幂等键重试；
//! - 关闭时排空队列并 join；不静默丢弃已接收请求。
//!
//! 测试注入（仅 `cfg(test)`）：闸门可让写线程在取请求前停住，从而确定性地填满通道；
//! 另可注入一次写线程 panic，验证故障路径。生产构建不包含这些代码。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use duckdb::Connection;
use whcode_domain::{ErrorCode, EventRecord, WhError};

use crate::writer::{EventWriter, WriteOutcome, WriterConfig, now_ms};

struct WriteRequest {
    record: EventRecord,
    idempotency_key: String,
    reply: SyncSender<Result<WriteOutcome, WhError>>,
}

/// 测试用闸门：控制写线程何时开始取请求，并可注入一次 panic。
#[cfg(test)]
mod gate {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Condvar, Mutex};

    /// `open = false` 时写线程停在取请求之前；`poison_next` 使下一次取请求直接 panic。
    #[derive(Debug)]
    pub(super) struct Gate {
        open: Mutex<bool>,
        cv: Condvar,
        poison_next: AtomicBool,
    }

    impl Gate {
        pub(super) fn new(open: bool) -> Arc<Self> {
            Arc::new(Self {
                open: Mutex::new(open),
                cv: Condvar::new(),
                poison_next: AtomicBool::new(false),
            })
        }

        pub(super) fn set_open(&self, open: bool) {
            *self.open.lock().expect("闸门锁中毒") = open;
            self.cv.notify_all();
        }

        pub(super) fn wait_until_open(&self) {
            let mut open = self.open.lock().expect("闸门锁中毒");
            while !*open {
                open = self.cv.wait(open).expect("闸门条件变量中毒");
            }
        }

        pub(super) fn poison_next(&self) {
            self.poison_next.store(true, Ordering::SeqCst);
        }

        pub(super) fn take_poison(&self) -> bool {
            self.poison_next.swap(false, Ordering::SeqCst)
        }
    }
}

#[cfg(test)]
use gate::Gate;

/// 非测试构建的闸门占位：`Infallible` 不可构造，`Option<Arc<Gate>>` 恒为 `None`。
#[cfg(not(test))]
type Gate = std::convert::Infallible;

/// 有界写者服务。`Drop` 会关闭通道并等待写线程退出；需要错误详情时调用
/// [`WriterService::shutdown`]。
pub struct WriterService {
    tx: Option<SyncSender<WriteRequest>>,
    handle: Option<JoinHandle<Result<(), WhError>>>,
    config: WriterConfig,
    /// 排队等待写线程处理的请求数（诊断指标；写线程异常退出时可能滞后）。
    queued: Arc<AtomicUsize>,
}

impl WriterService {
    /// 启动写线程；`conn` 的所有权转移给该线程。
    ///
    /// # Errors
    ///
    /// 配置非法、建表失败或线程创建失败时返回错误。
    pub fn start(conn: Connection, config: WriterConfig) -> Result<Self, WhError> {
        Self::start_impl(conn, config, None)
    }

    /// 启动写线程并返回测试闸门（仅测试构建）。
    #[cfg(test)]
    fn start_with_gate(
        conn: Connection,
        config: WriterConfig,
    ) -> Result<(Self, Arc<Gate>), WhError> {
        let gate = Gate::new(false);
        let service = Self::start_impl(conn, config, Some(Arc::clone(&gate)))?;
        Ok((service, gate))
    }

    fn start_impl(
        conn: Connection,
        config: WriterConfig,
        gate: Option<Arc<Gate>>,
    ) -> Result<Self, WhError> {
        let writer = EventWriter::new(conn, config)?;
        let capacity = config.max_pending;
        let (tx, rx) = sync_channel(capacity);
        let window = Duration::from_millis(config.batch_window_ms.max(1));
        let queued = Arc::new(AtomicUsize::new(0));
        let handle = thread::Builder::new()
            .name("whcode-writer".to_string())
            .spawn({
                let queued = Arc::clone(&queued);
                move || run(writer, rx, window, gate, queued)
            })
            .map_err(|err| WhError::new(ErrorCode::IoError, format!("创建写线程失败：{err}")))?;
        Ok(Self {
            tx: Some(tx),
            handle: Some(handle),
            config,
            queued,
        })
    }

    /// 生效配置。
    pub const fn config(&self) -> WriterConfig {
        self.config
    }

    /// 当前排队等待写线程处理的请求数（诊断用）。
    ///
    /// 写线程异常退出时该值可能滞后；正常关闭并 join 后应为 0。
    pub fn queued_len(&self) -> usize {
        self.queued.load(Ordering::SeqCst)
    }

    /// 有界队列容量。
    pub const fn capacity(&self) -> usize {
        self.config.max_pending
    }

    /// 提交一条事件并等待写线程的即时结果。
    ///
    /// 非关键事件可能返回 [`WriteOutcome::Buffered`]（已被写线程接收，等待批次提交）；
    /// 关键事件在 COMMIT 成功后返回 [`WriteOutcome::Committed`]。
    ///
    /// # Errors
    ///
    /// 队列满返回背压错误；写者已关闭或写线程异常退出时返回明确错误。
    pub fn submit(
        &self,
        record: EventRecord,
        idempotency_key: impl Into<String>,
    ) -> Result<WriteOutcome, WhError> {
        let reply = self.enqueue(record, idempotency_key.into())?;
        reply.recv().map_err(|_| WhError {
            // 请求可能已被接收但结果未知：按幂等键重试不会造成副作用重复（V-106）。
            retryable: true,
            ..WhError::internal("写线程在确认前退出：请求结果未知，须按幂等键重试")
        })?
    }

    /// 只入队、不等确认（仅测试使用：用于确定性地填满通道并观察背压）。
    #[cfg(test)]
    fn enqueue_nowait(
        &self,
        record: EventRecord,
        idempotency_key: impl Into<String>,
    ) -> Result<(), WhError> {
        drop(self.enqueue(record, idempotency_key.into())?);
        Ok(())
    }

    /// 把请求放入有界通道；不等待写线程处理。
    fn enqueue(
        &self,
        record: EventRecord,
        idempotency_key: String,
    ) -> Result<Receiver<Result<WriteOutcome, WhError>>, WhError> {
        let tx = self
            .tx
            .as_ref()
            .ok_or_else(|| WhError::invalid_state("写者服务已关闭，拒绝新请求"))?;
        let (reply_tx, reply_rx) = sync_channel(1);
        let request = WriteRequest {
            record,
            idempotency_key,
            reply: reply_tx,
        };
        // 先加后发：写线程可能在发送成功后立刻取走请求；若先发后加，计数会下溢。
        self.queued.fetch_add(1, Ordering::SeqCst);
        match tx.try_send(request) {
            Ok(()) => Ok(reply_rx),
            Err(TrySendError::Full(_)) => {
                self.queued.fetch_sub(1, Ordering::SeqCst);
                Err(WhError::backpressure(format!(
                    "写者队列已满（容量 {}）：请求未被接收，调用方须背压或重试（ADR-017）",
                    self.config.max_pending
                )))
            }
            Err(TrySendError::Disconnected(_)) => {
                self.queued.fetch_sub(1, Ordering::SeqCst);
                Err(WhError::invalid_state("写者服务已关闭，拒绝新请求"))
            }
        }
    }

    /// 关闭服务：停止接收、排空队列、flush 并 join。
    ///
    /// # Errors
    ///
    /// 写线程 panic 或最终 flush 失败时返回错误；未提交批次保留在写线程内，
    /// 调用方须按幂等键重试。
    pub fn shutdown(mut self) -> Result<(), WhError> {
        self.tx.take();
        match self.handle.take() {
            Some(handle) => handle
                .join()
                .map_err(|_| WhError::internal("写线程 panic：写者服务不可用"))?,
            None => Ok(()),
        }
    }
}

impl Drop for WriterService {
    fn drop(&mut self) {
        self.tx.take();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn run(
    mut writer: EventWriter,
    rx: Receiver<WriteRequest>,
    window: Duration,
    _gate: Option<Arc<Gate>>,
    queued: Arc<AtomicUsize>,
) -> Result<(), WhError> {
    loop {
        #[cfg(test)]
        if let Some(gate) = _gate.as_ref() {
            gate.wait_until_open();
            if gate.take_poison() {
                panic!("注入写线程故障：验证 panic 故障路径（ADR-017 §6）");
            }
        }
        match rx.recv_timeout(window) {
            Ok(request) => {
                queued.fetch_sub(1, Ordering::SeqCst);
                let outcome = writer.submit_at(request.record, request.idempotency_key, now_ms());
                let _ = request.reply.send(outcome);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // 时间窗口到期：失败时保留批次，下一次请求或关闭时再重试。
                let _ = writer.flush_if_due(now_ms());
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                writer.flush()?;
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::{Path, PathBuf};
    use whcode_domain::{EventKind, PersistenceLevel, SCHEMA_VERSION};

    fn temp_db(tag: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "whcode-writer-service-{}-{tag}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("创建临时目录");
        let db = dir.join("service.duckdb");
        (dir, db)
    }

    fn service_at(db: &Path, config: WriterConfig) -> WriterService {
        let conn = Connection::open(db).expect("打开测试库");
        WriterService::start(conn, config).expect("启动写者服务")
    }

    fn service_with_gate(db: &Path, config: WriterConfig) -> (WriterService, Arc<Gate>) {
        let conn = Connection::open(db).expect("打开测试库");
        WriterService::start_with_gate(conn, config).expect("启动写者服务")
    }

    fn event(seq: u64, kind: EventKind) -> EventRecord {
        EventRecord {
            schema_version: SCHEMA_VERSION,
            seq,
            id: format!("E-{seq}"),
            kind,
            session_id: Some("S-001".to_string()),
            task_id: None,
            request_id: None,
            tool_call_id: None,
            payload: json!({"n": seq}),
            ts_ms: 1_700_000_000_000 + seq,
            persistence_level: PersistenceLevel::Redacted,
        }
    }

    fn count_after_shutdown(service: WriterService, db: &Path) -> i64 {
        service.shutdown().expect("关闭写者");
        let conn = Connection::open(db).expect("重开测试库");
        conn.query_row("SELECT count(*) FROM m0_events", [], |row| row.get(0))
            .expect("统计事件")
    }

    #[test]
    fn defaults_match_baseline_and_config_is_effective() {
        let config = WriterConfig::default();
        assert_eq!(config.batch_rows, 500, "默认批次行数须为 500（FR-503）");
        assert_eq!(
            config.batch_window_ms, 50,
            "默认时间窗口须为 50 ms（FR-503）"
        );
        assert!(config.max_pending > 0, "队列必须有界（ADR-017 不变量 4）");

        let (_dir, db) = temp_db("config");
        let custom = WriterConfig {
            batch_rows: 5,
            batch_window_ms: 3_600_000,
            max_pending: 32,
            ..WriterConfig::default()
        };
        let service = service_at(&db, custom);
        assert_eq!(service.config(), custom, "配置必须原样生效，不得被静默改写");
        drop(service);
    }

    #[test]
    fn critical_event_is_confirmed() {
        let (_dir, db) = temp_db("critical");
        let service = service_at(&db, WriterConfig::default());
        let outcome = service
            .submit(event(1, EventKind::ToolCallStarted), "k-critical")
            .expect("关键事件提交");
        assert!(matches!(outcome, WriteOutcome::Committed { seq: 1 }));
        assert_eq!(count_after_shutdown(service, &db), 1);
    }

    #[test]
    fn buffered_event_is_flushed_by_time_window() {
        let (_dir, db) = temp_db("window");
        let config = WriterConfig {
            batch_rows: 500,
            batch_window_ms: 10,
            ..WriterConfig::default()
        };
        let service = service_at(&db, config);
        let outcome = service
            .submit(event(1, EventKind::StreamingChunk), "k-buffered")
            .expect("非关键事件提交");
        assert_eq!(outcome, WriteOutcome::Buffered);
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(count_after_shutdown(service, &db), 1);
    }

    #[test]
    fn batch_row_threshold_is_configurable() {
        let (_dir, db) = temp_db("threshold");
        let config = WriterConfig {
            batch_rows: 5,
            batch_window_ms: 3_600_000,
            ..WriterConfig::default()
        };
        let service = service_at(&db, config);
        for index in 1..=4 {
            let outcome = service
                .submit(
                    event(index, EventKind::StreamingChunk),
                    format!("k-{index}"),
                )
                .expect("未达阈值应入缓冲");
            assert_eq!(outcome, WriteOutcome::Buffered, "第 {index} 条不应提前提交");
        }
        let outcome = service
            .submit(event(5, EventKind::StreamingChunk), "k-5")
            .expect("达到阈值应提交");
        assert_eq!(outcome, WriteOutcome::Committed { seq: 5 });
        assert_eq!(count_after_shutdown(service, &db), 5);
    }

    #[test]
    fn duplicate_key_is_deduplicated_through_service() {
        let (_dir, db) = temp_db("dedup");
        let service = service_at(&db, WriterConfig::default());
        let first = service
            .submit(event(1, EventKind::ToolCallStarted), "k-same")
            .expect("首次提交");
        let second = service
            .submit(event(2, EventKind::ToolCallStarted), "k-same")
            .expect("重复提交");
        assert!(matches!(first, WriteOutcome::Committed { seq: 1 }));
        assert!(matches!(second, WriteOutcome::Deduplicated { seq: 1 }));
        assert_eq!(count_after_shutdown(service, &db), 1);
    }

    #[test]
    fn queue_is_bounded_and_reports_backpressure() {
        let (_dir, db) = temp_db("backpressure");
        let config = WriterConfig {
            max_pending: 2,
            ..WriterConfig::default()
        };
        let (service, gate) = service_with_gate(&db, config);
        // 闸门关闭：写线程停在取请求之前，通道容量 2 被确定性地填满。
        service
            .enqueue_nowait(event(1, EventKind::StreamingChunk), "k-1")
            .expect("第 1 条应入队");
        service
            .enqueue_nowait(event(2, EventKind::StreamingChunk), "k-2")
            .expect("第 2 条应入队");
        let err = service
            .enqueue_nowait(event(3, EventKind::StreamingChunk), "k-3")
            .expect_err("第 3 条应被背压拒绝");
        assert_eq!(err.code, ErrorCode::Backpressure, "队列满须返回背压错误码");
        assert!(err.retryable, "背压错误必须可重试");

        gate.set_open(true);
        assert_eq!(
            count_after_shutdown(service, &db),
            2,
            "已接收请求必须最终提交（ADR-017 不变量 3）"
        );
    }

    #[test]
    fn drop_drains_accepted_requests() {
        let (_dir, db) = temp_db("drop");
        let service = service_at(
            &db,
            WriterConfig {
                batch_rows: 500,
                batch_window_ms: 3_600_000,
                ..WriterConfig::default()
            },
        );
        for index in 1..=3 {
            let outcome = service
                .submit(
                    event(index, EventKind::StreamingChunk),
                    format!("k-{index}"),
                )
                .expect("入缓冲");
            assert_eq!(outcome, WriteOutcome::Buffered);
        }
        drop(service);
        let conn = Connection::open(&db).expect("重开测试库");
        let count: i64 = conn
            .query_row("SELECT count(*) FROM m0_events", [], |row| row.get(0))
            .expect("统计事件");
        assert_eq!(count, 3, "Drop 必须排空已接收请求并 flush");
    }

    #[test]
    fn writer_panic_is_reported_and_later_requests_are_rejected() {
        let (_dir, db) = temp_db("panic");
        let (service, gate) = service_with_gate(&db, WriterConfig::default());
        gate.set_open(true);
        gate.poison_next();
        let err = service
            .submit(event(1, EventKind::StreamingChunk), "k-panic")
            .expect_err("写线程 panic 后提交必须失败");
        assert_eq!(err.code, ErrorCode::Internal, "确认前退出须返回内部错误");
        assert!(err.retryable, "结果未知，必须允许按幂等键重试");

        let err = service
            .submit(event(2, EventKind::StreamingChunk), "k-after")
            .expect_err("写线程已退出，后续提交必须被拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);

        let err = service.shutdown().expect_err("panic 后的 join 必须报错");
        assert_eq!(err.code, ErrorCode::Internal);
    }

    #[test]
    fn multiple_producers_keep_seq_unique() {
        let (_dir, db) = temp_db("multi");
        let config = WriterConfig {
            batch_rows: 10,
            batch_window_ms: 10,
            ..WriterConfig::default()
        };
        let service = service_at(&db, config);
        std::thread::scope(|scope| {
            for producer in 0..4 {
                let service = &service;
                scope.spawn(move || {
                    for index in 0..25 {
                        let seq = producer * 25 + index + 1;
                        service
                            .submit(event(seq, EventKind::StreamingChunk), format!("k-{seq}"))
                            .expect("并发提交");
                    }
                });
            }
        });
        let count = count_after_shutdown(service, &db);
        assert_eq!(count, 100);

        let conn = Connection::open(&db).expect("重开测试库");
        let distinct: i64 = conn
            .query_row("SELECT count(DISTINCT seq) FROM m0_events", [], |row| {
                row.get(0)
            })
            .expect("统计 seq");
        assert_eq!(distinct, 100, "seq 必须唯一");
    }

    #[test]
    fn queue_depth_tracks_waiting_requests_and_capacity() {
        let (_dir, db) = temp_db("depth");
        let config = WriterConfig {
            max_pending: 2,
            batch_rows: 2,
            ..WriterConfig::default()
        };
        let (service, gate) = service_with_gate(&db, config);
        assert_eq!(service.capacity(), 2);
        assert_eq!(service.queued_len(), 0);

        service
            .enqueue_nowait(event(1, EventKind::StreamingChunk), "k-1")
            .expect("入队 1");
        service
            .enqueue_nowait(event(2, EventKind::StreamingChunk), "k-2")
            .expect("入队 2");
        assert_eq!(service.queued_len(), 2, "两条等待中的请求必须可观测");

        let err = service
            .enqueue_nowait(event(3, EventKind::StreamingChunk), "k-3")
            .expect_err("第三条应被背压拒绝");
        assert_eq!(err.code, ErrorCode::Backpressure);
        assert_eq!(service.queued_len(), 2, "被拒绝的请求不得计入队列深度");

        gate.set_open(true);
        // 写线程排空有延迟：等待背压解除（有界时限 + sleep 1ms——非忙等；高负载下
        // 纯 yield 计数会偶发耗尽，实测于批次 3 全量套件复现）。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match service.submit(event(3, EventKind::ToolCallStarted), "k-critical") {
                Ok(_) => break,
                Err(err)
                    if err.code == ErrorCode::Backpressure
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                Err(err) => panic!("关键事件确认失败：{err}"),
            }
        }
        assert_eq!(service.queued_len(), 0, "确认返回时先前请求必须已出队");
        service.shutdown().expect("关闭");
    }
}
