//! SSH → 应用服务适配器（T098、ADR-005、C-03、ADR-030）。
//!
//! ## 并发设计档案（ADR-015 口径）
//!
//! - **结构**：装配根打开**唯一一条** `Connection`，所有权移交给**专用执行线程**
//!   （`whcode-ssh-exec`，随 [`CommandExecutor`] 生命周期存在，与会话数无关）。
//!   russh Handler（async，运行在 tokio 上）只把 [`WhCodeCommand`] 经**有界命令队列**
//!   （`tokio::sync::mpsc`，容量 [`COMMAND_QUEUE_CAPACITY`]）投递给执行线程，并以
//!   一次性回执（`oneshot`）异步等待结果——投递满即背压拒绝（[`ErrorCode::Backpressure`]），
//!   不无限积压；等待有 [`EXEC_TIMEOUT`] 上限。
//! - **正**：写路径全局唯一（Check 98：SSH 会话期间没有任何第二连接写路径——类型层面
//!   Handler/Acceptor 不持有 `Connection`，运行层面所有库访问都发生在执行线程）；无锁
//!   竞争（单线程顺序使用连接，ADR-002 同一和解）；线程数有界且不随会话增长（NFR-211）；
//!   队列、审计、输出全部有界。
//! - **负（承担者：命令吞吐与时延）**：所有 SSH 命令串行经过执行线程，吞吐上限 =
//!   单线程 DB 命令速率；一个慢命令（≤ [`EXEC_TIMEOUT`]）会占住队列槽位，其他会话
//!   的同类命令可能拿到背压拒绝（可重试）。这是“单写者”语义的直接代价，当前和解。
//! - **失效条件**：SSH 命令速率或慢命令比例高到背压拒绝成为常态（> 队列容量/秒）时，
//!   本设计不再适配——需要按 ADR 另行评估（读写分离或快照读），不得静默扩队列。
//! - **回退路径**：[`WhCodeCommand`] 与 [`CommandOutcome`] 是纯数据，执行线程内部换
//!   任意实现（如 whcode-store 的 WriterService）不影响 Handler 与服务端。
//!
//! ## 审计（Check 97 的“显式授权路径有审计”）
//!
//! [`AuditLog`] 记录每次执行/拒绝/授权判定（bounded：容量 [`AUDIT_CAPACITY`]，超出后
//! 丢弃新条目并累计 `overflow` 计数——显式有界，不静默无限累积）。条目只含会话 ID、
//! Actor id、命令形态与结果，**不含**密钥材料与凭据。

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use duckdb::Connection;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use whcode_app::Actor;
use whcode_domain::{ErrorCode, WhError};

use crate::policy::WhCodeCommand;

/// 有界命令队列容量：满即背压拒绝（可重试），不无限积压（NFR-215）。
pub const COMMAND_QUEUE_CAPACITY: usize = 64;
/// 单命令执行等待上限：超时返回 [`ErrorCode::Timeout`]，调用方以非 0 退出码收场。
pub const EXEC_TIMEOUT: Duration = Duration::from_secs(30);
/// 单条 stdout/stderr 输出上限（字节，超限截断并标记）。
pub const OUTPUT_LIMIT_BYTES: usize = 64 * 1024;
/// 审计条目容量上限（超出丢弃并计数，不无限累积）。
pub const AUDIT_CAPACITY: usize = 4096;

/// 应用服务执行失败时的通用退出码（非 0）。
pub const EXIT_EXECUTION_ERROR: i32 = 1;
/// 执行超时的退出码（非 0）。
pub const EXIT_TIMEOUT: i32 = 124;

/// 当前 Unix 毫秒（饱和；审计时间戳与会话创建时间用）。
pub(crate) fn now_unix_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0),
    )
    .unwrap_or(u64::MAX)
}

/// 一条受限命令的执行结果（stdout/stderr 已按 UTF-8 有损解码并截断到
/// [`OUTPUT_LIMIT_BYTES`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutcome {
    /// 进程退出码语义：0 成功，非 0 失败（白名单外为 [`crate::policy::EXIT_POLICY_DENIED`]）。
    pub exit_code: i32,
    /// 标准输出。
    pub stdout: String,
    /// 标准错误。
    pub stderr: String,
    /// 任一输出流是否发生截断。
    pub truncated: bool,
    /// 补充说明（错误阶段、超时、未实现边界等）。
    pub note: String,
}

impl CommandOutcome {
    /// 成功结果。
    #[must_use]
    pub fn success(stdout: String, stderr: String) -> Self {
        Self {
            exit_code: 0,
            stdout,
            stderr,
            truncated: false,
            note: String::new(),
        }
    }

    /// 失败结果（非 0 退出码 + stderr 说明；不 panic、不静默）。
    #[must_use]
    pub fn failure(exit_code: i32, stderr: String, note: String) -> Self {
        Self {
            exit_code,
            stdout: String::new(),
            stderr,
            truncated: false,
            note,
        }
    }
}

/// 截断到 [`OUTPUT_LIMIT_BYTES`]（按字节保留尾部；UTF-8 边界损失可接受——有损解码同
/// `proto` 的输出口径）。
fn bounded_output(bytes: Vec<u8>) -> (String, bool) {
    if bytes.len() > OUTPUT_LIMIT_BYTES {
        let tail = &bytes[bytes.len() - OUTPUT_LIMIT_BYTES..];
        (String::from_utf8_lossy(tail).into_owned(), true)
    } else {
        (String::from_utf8_lossy(&bytes).into_owned(), false)
    }
}

/// 一条审计记录（只含标识与结果，不含密钥材料、凭据或数据正文）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditRecord {
    /// Unix 毫秒时间戳。
    pub at_ms: u64,
    /// SSH 会话 ID。
    pub session_id: String,
    /// 调用者（Actor id）。
    pub actor_id: String,
    /// 动作形态（`exec: sessions`、`denied`、`shell-granted`、`resize`、`auth-denied`、
    /// `session-closed` 等）。
    pub action: String,
    /// 该动作是否触发了数据库写入（Check 98 佐证：写只发生在执行线程）。
    pub wrote: bool,
    /// 是否成功放行/执行。
    pub ok: bool,
    /// 补充细节（如终端尺寸 `120x40`）。
    pub detail: String,
}

/// 有界审计日志（`Mutex<Vec<_>>`，容量 [`AUDIT_CAPACITY`]，超出丢弃新条目并计数）。
#[derive(Debug, Default)]
pub struct AuditLog {
    entries: Mutex<Vec<AuditRecord>>,
    overflow: AtomicU64,
}

impl AuditLog {
    /// 空审计日志。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 记录一条（锁中毒或容量满按丢弃处理并累计溢出——审计失败不阻塞业务路径）。
    pub fn record(&self, record: AuditRecord) {
        let Ok(mut guard) = self.entries.lock() else {
            self.overflow.fetch_add(1, Ordering::Relaxed);
            return;
        };
        if guard.len() >= AUDIT_CAPACITY {
            self.overflow.fetch_add(1, Ordering::Relaxed);
            return;
        }
        guard.push(record);
    }

    /// 已记录条目快照（按发生顺序）。
    #[must_use]
    pub fn snapshot(&self) -> Vec<AuditRecord> {
        self.entries
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// 因容量上限被丢弃的条目数。
    #[must_use]
    pub fn overflow_count(&self) -> u64 {
        self.overflow.load(Ordering::Relaxed)
    }
}

/// 执行线程的请求（Handler 投递、执行线程消费）。
struct ExecRequest {
    session_id: String,
    actor: Actor,
    terminal: Option<(u32, u32)>,
    command: WhCodeCommand,
    reply: oneshot::Sender<CommandOutcome>,
}

/// 单写者命令执行器：专用线程独占装配根注入的 `Connection`，Handler 经有界队列投递。
///
/// `Drop` 会关闭队列并等待执行线程退出（连接随之释放——RAII，不泄漏句柄）。
#[derive(Debug)]
pub struct CommandExecutor {
    tx: Option<mpsc::Sender<ExecRequest>>,
    handle: Option<std::thread::JoinHandle<()>>,
    audit: Arc<AuditLog>,
}

impl CommandExecutor {
    /// 启动执行线程；`conn` 所有权移交给该线程（此后除本类型外无人能触库）。
    ///
    /// # Errors
    ///
    /// 线程创建失败时返回 [`WhError`]。
    pub fn start(conn: Connection, audit: Arc<AuditLog>) -> Result<Self, WhError> {
        let (tx, rx) = mpsc::channel(COMMAND_QUEUE_CAPACITY);
        let audit_for_thread = Arc::clone(&audit);
        let handle = std::thread::Builder::new()
            .name("whcode-ssh-exec".to_string())
            .spawn(move || executor_loop(conn, rx, audit_for_thread))
            .map_err(|err| {
                WhError::new(ErrorCode::IoError, format!("创建 SSH 执行线程失败：{err}"))
            })?;
        Ok(Self {
            tx: Some(tx),
            handle: Some(handle),
            audit,
        })
    }

    /// 审计日志句柄（服务端共用同一份）。
    #[must_use]
    pub fn audit(&self) -> &Arc<AuditLog> {
        &self.audit
    }

    /// 投递一条受限命令并等待结果（有界队列 + 一次性回执 + 超时上限）。
    ///
    /// # Errors
    ///
    /// 队列满返回 [`ErrorCode::Backpressure`]（可重试）；执行线程已退出返回
    /// [`ErrorCode::Internal`]；超时返回 [`ErrorCode::Timeout`]。
    pub async fn execute(
        &self,
        session_id: impl Into<String>,
        actor: Actor,
        terminal: Option<(u32, u32)>,
        command: WhCodeCommand,
    ) -> Result<CommandOutcome, WhError> {
        let Some(tx) = &self.tx else {
            return Err(WhError::internal("SSH 执行器已关闭：拒绝新命令"));
        };
        let (reply_tx, reply_rx) = oneshot::channel();
        let request = ExecRequest {
            session_id: session_id.into(),
            actor,
            terminal,
            command,
            reply: reply_tx,
        };
        // 有界投递：满即背压（可重试），绝不无限阻塞。
        tx.try_send(request).map_err(|err| match err {
            mpsc::error::TrySendError::Full(_) => WhError::new(
                ErrorCode::Backpressure,
                format!(
                    "SSH 命令队列已满（容量 {COMMAND_QUEUE_CAPACITY}）：稍后重试（背压拒绝，不无限积压）"
                ),
            ),
            mpsc::error::TrySendError::Closed(_) => {
                WhError::internal("SSH 执行线程已退出：命令未执行")
            }
        })?;
        match tokio::time::timeout(EXEC_TIMEOUT, reply_rx).await {
            Ok(Ok(outcome)) => Ok(outcome),
            Ok(Err(_recv)) => Err(WhError::internal(
                "SSH 执行线程在返回结果前退出：结果不可知（不得视为成功）",
            )),
            Err(_) => Err(WhError::new(
                ErrorCode::Timeout,
                format!(
                    "SSH 命令执行超过 {} ms：未取得结果",
                    EXEC_TIMEOUT.as_millis()
                ),
            )),
        }
    }

    /// 关闭队列并等待执行线程退出（排空已接收请求；不静默丢弃）。
    ///
    /// # Errors
    ///
    /// 执行线程 panic 时返回 [`WhError`]。
    pub fn shutdown(mut self) -> Result<(), WhError> {
        self.tx.take();
        self.join_handle()
    }

    fn join_handle(&mut self) -> Result<(), WhError> {
        if let Some(handle) = self.handle.take() {
            handle.join().map_err(|_| {
                WhError::internal("SSH 执行线程异常退出（panic）：已接收命令可能未执行完")
            })?;
        }
        Ok(())
    }
}

impl Drop for CommandExecutor {
    fn drop(&mut self) {
        // 关闭队列 → 执行线程 blocking_recv 返回 None → 退出 → 连接释放（RAII）。
        self.tx.take();
        let _ = self.join_handle();
    }
}

/// 执行线程主循环：独占连接，顺序处理请求直到队列关闭。
fn executor_loop(conn: Connection, mut rx: mpsc::Receiver<ExecRequest>, audit: Arc<AuditLog>) {
    // blocking_recv 只能在非 async 线程调用——本线程即专用执行线程。
    while let Some(request) = rx.blocking_recv() {
        let ExecRequest {
            session_id,
            actor,
            terminal,
            command,
            reply,
        } = request;
        let summary = command.summary();
        let wrote = matches!(command, WhCodeCommand::SessionNew { .. });
        let outcome = run_on_connection(&conn, &actor, &command);
        audit.record(AuditRecord {
            at_ms: now_unix_ms(),
            session_id,
            actor_id: actor.id,
            action: format!("exec: {summary}"),
            wrote,
            ok: outcome.exit_code == 0,
            detail: terminal
                .map(|(cols, rows)| format!("terminal={cols}x{rows}"))
                .unwrap_or_default(),
        });
        // 调用方（channel/连接）可能已消失：回执失败只丢结果，不影响执行线程。
        let _ = reply.send(outcome);
    }
    // rx 关闭：连接在此析构（RAII 释放）。
}

/// 在连接上执行一条受限命令（唯一库访问点；所有写经 whcode-app 应用服务判权后写入）。
fn run_on_connection(conn: &Connection, actor: &Actor, command: &WhCodeCommand) -> CommandOutcome {
    let policy = whcode_app::DefaultPolicy;
    let result: Result<String, WhError> = match command {
        WhCodeCommand::SessionList { include_archived } => {
            list_sessions_text(conn, &policy, actor, *include_archived)
        }
        WhCodeCommand::SessionShow { session_id } => {
            show_session_text(conn, &policy, actor, session_id)
        }
        WhCodeCommand::SessionNew { title } => new_session_text(conn, &policy, actor, title),
        WhCodeCommand::TaskOverview { session_id } => {
            task_overview_text(conn, &policy, actor, session_id.as_deref())
        }
        WhCodeCommand::TaskDetail { task_id } => task_detail_text(conn, &policy, actor, task_id),
    };
    match result {
        Ok(stdout) => {
            let (stdout, truncated) = bounded_output(stdout.into_bytes());
            CommandOutcome {
                exit_code: 0,
                stdout,
                stderr: String::new(),
                truncated,
                note: String::new(),
            }
        }
        Err(err) => CommandOutcome::failure(
            EXIT_EXECUTION_ERROR,
            format!("{}（错误码 {}）", err, err.code.as_str()),
            "应用服务执行失败".to_string(),
        ),
    }
}

/// 会话清单文本（有界：上限沿用 `LIST_SESSIONS_LIMIT`）。
fn list_sessions_text(
    conn: &Connection,
    policy: &dyn whcode_app::Policy,
    actor: &Actor,
    include_archived: bool,
) -> Result<String, WhError> {
    let rows = whcode_app::list_sessions(
        conn,
        policy,
        actor,
        include_archived,
        whcode_app::LIST_SESSIONS_LIMIT,
    )?;
    if rows.is_empty() {
        // 正确的空：显式说明，而不是静默空串。
        return Ok("（还没有会话：session new <标题> 创建）".to_string());
    }
    let mut text = String::new();
    for row in rows {
        text.push_str(&format!(
            "{}\t{}\t{}\t{}{}\n",
            row.id,
            row.title,
            row.stage,
            row.status,
            if row.archived { "\tarchived" } else { "" }
        ));
    }
    Ok(text)
}

/// 会话详情文本。
fn show_session_text(
    conn: &Connection,
    policy: &dyn whcode_app::Policy,
    actor: &Actor,
    session_id: &str,
) -> Result<String, WhError> {
    let row = whcode_app::show_session(conn, policy, actor, session_id)?;
    Ok(format!(
        "id: {}\ntitle: {}\nstage: {}\nstatus: {}\nmodel: {}\narchived: {}\n",
        row.id,
        row.title,
        row.stage,
        row.status,
        row.model.as_deref().unwrap_or("-"),
        row.archived
    ))
}

/// 创建会话文本（会话级写命令；authz 在 app 层判定）。
fn new_session_text(
    conn: &Connection,
    policy: &dyn whcode_app::Policy,
    actor: &Actor,
    title: &str,
) -> Result<String, WhError> {
    let row = whcode_app::new_session(
        conn,
        policy,
        actor,
        whcode_app::NewSession {
            title: title.to_string(),
            model: None,
        },
        now_unix_ms(),
    )?;
    Ok(format!("created {}", row.id))
}

/// 任务总览文本。
fn task_overview_text(
    conn: &Connection,
    policy: &dyn whcode_app::Policy,
    actor: &Actor,
    session_id: Option<&str>,
) -> Result<String, WhError> {
    let overview = whcode_app::task_overview(conn, policy, actor, session_id)?;
    let mut text = format!(
        "total: {}\ndone: {}\nblocked: {}\nactive: {}\n",
        overview.total,
        overview.done,
        overview.blocked,
        overview.active.len()
    );
    for stage in &overview.by_stage {
        text.push_str(&format!("stage {}: {}\n", stage.0, stage.1));
    }
    Ok(text)
}

/// 任务详情文本。
fn task_detail_text(
    conn: &Connection,
    policy: &dyn whcode_app::Policy,
    actor: &Actor,
    task_id: &str,
) -> Result<String, WhError> {
    let detail = whcode_app::task_detail(conn, policy, actor, task_id)?;
    Ok(format!(
        "id: {}\ntitle: {}\nstatus: {}\ncheckpoints: {}\n",
        detail.task.id,
        detail.task.title,
        detail.task.status.as_str(),
        detail.checkpoints.len()
    ))
}

#[cfg(test)]
pub(crate) mod gate {
    //! 测试闸门：让执行线程停在取请求之前，从而确定性地填满有界队列（背压判定）。
    //! 仅测试构建存在。

    use std::sync::{Arc, Condvar, Mutex};

    #[derive(Debug)]
    pub(crate) struct Gate {
        open: Mutex<bool>,
        cv: Condvar,
    }

    impl Gate {
        pub(crate) fn new() -> Arc<Self> {
            Arc::new(Self {
                open: Mutex::new(false),
                cv: Condvar::new(),
            })
        }

        pub(crate) fn open(&self) {
            *self.open.lock().expect("闸门锁中毒") = true;
            self.cv.notify_all();
        }

        pub(crate) fn wait_until_open(&self) {
            let mut open = self.open.lock().expect("闸门锁中毒");
            while !*open {
                open = self.cv.wait(open).expect("闸门条件变量中毒");
            }
        }
    }
}

#[cfg(test)]
impl CommandExecutor {
    /// 测试专用：带闸门启动（执行线程先停在取请求前，便于确定性填满队列）。
    pub(crate) fn start_with_gate(
        conn: Connection,
        audit: Arc<AuditLog>,
    ) -> Result<(Self, Arc<gate::Gate>), WhError> {
        let gate = gate::Gate::new();
        let (tx, rx) = mpsc::channel(COMMAND_QUEUE_CAPACITY);
        let gate_for_thread = Arc::clone(&gate);
        let audit_for_thread = Arc::clone(&audit);
        let handle = std::thread::Builder::new()
            .name("whcode-ssh-exec-test".to_string())
            .spawn(move || {
                gate_for_thread.wait_until_open();
                executor_loop(conn, rx, audit_for_thread)
            })
            .map_err(|err| {
                WhError::new(ErrorCode::IoError, format!("创建测试执行线程失败：{err}"))
            })?;
        Ok((
            Self {
                tx: Some(tx),
                handle: Some(handle),
                audit,
            },
            gate,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use whcode_app::{Capability, actor_with_all_capabilities};

    /// 测试迁移夹具：include_str 复用 whcode-store 的迁移 SQL（前端清单不得依赖
    /// whcode-store——分层门禁；与 whcode-web 测试同一做法）。
    const TEST_MIGRATIONS: &str = concat!(
        include_str!("../../whcode-store/migrations/0001_initial.sql"),
        include_str!("../../whcode-store/migrations/0002_event_trace_indexes.sql"),
        include_str!("../../whcode-store/migrations/0003_query_indexes.sql"),
        include_str!("../../whcode-store/migrations/0004_session_id_sequence.sql"),
        include_str!("../../whcode-store/migrations/0005_session_stage.sql"),
        include_str!("../../whcode-store/migrations/0006_events_variant.sql"),
    );

    fn migrated_memory_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("内存库");
        conn.execute_batch(TEST_MIGRATIONS).expect("迁移");
        conn
    }

    fn read_actor() -> Actor {
        Actor::new(
            "ssh-tester",
            [Capability::SessionRead, Capability::TaskRead],
        )
    }

    fn write_actor() -> Actor {
        Actor::new(
            "ssh-tester",
            [Capability::SessionCreate, Capability::SessionRead],
        )
    }

    #[tokio::test]
    async fn executor_round_trip_runs_whcode_command_via_app_service() {
        let audit = Arc::new(AuditLog::new());
        let executor =
            CommandExecutor::start(migrated_memory_conn(), Arc::clone(&audit)).expect("启动执行器");
        let actor = write_actor();

        let outcome = executor
            .execute(
                "ssh-1",
                actor.clone(),
                Some((80, 24)),
                WhCodeCommand::SessionNew {
                    title: "适配器创建".to_string(),
                },
            )
            .await
            .expect("执行成功");
        assert_eq!(outcome.exit_code, 0, "stderr：{}", outcome.stderr);
        assert!(
            outcome.stdout.starts_with("created S-"),
            "：{}",
            outcome.stdout
        );

        // 只读命令随后可见（同一连接顺序执行）。
        let outcome = executor
            .execute(
                "ssh-1",
                actor,
                None,
                WhCodeCommand::SessionList {
                    include_archived: false,
                },
            )
            .await
            .expect("执行成功");
        assert_eq!(outcome.exit_code, 0);
        assert!(
            outcome.stdout.contains("适配器创建"),
            "：{}",
            outcome.stdout
        );

        // 审计：一条写、一条读，写命令带终端尺寸传递。
        let entries = audit.snapshot();
        assert_eq!(entries.len(), 2, "：{entries:?}");
        assert!(entries[0].wrote, "session new 是写命令");
        assert!(!entries[1].wrote);
        assert_eq!(entries[0].detail, "terminal=80x24", "终端尺寸传递进执行");
        assert!(entries.iter().all(|e| e.ok), "全部成功：{entries:?}");
    }

    #[tokio::test]
    async fn app_errors_map_to_nonzero_exit_and_stderr() {
        let audit = Arc::new(AuditLog::new());
        let executor =
            CommandExecutor::start(migrated_memory_conn(), Arc::clone(&audit)).expect("启动执行器");
        // 无能力调用者：应用层 require 拒绝 → 非 0 退出码 + 结构化 stderr。
        let nobody = Actor::new("ssh-nobody", []);

        let outcome = executor
            .execute(
                "ssh-1",
                nobody,
                None,
                WhCodeCommand::SessionList {
                    include_archived: false,
                },
            )
            .await
            .expect("投递成功");
        assert_ne!(outcome.exit_code, 0, "权限拒绝必须非 0 退出码");
        assert!(
            outcome.stderr.contains("permission_denied"),
            "stderr 结构化说明错误码：{}",
            outcome.stderr
        );
        assert!(!audit.snapshot()[0].ok, "审计记录失败事实");
    }

    #[tokio::test]
    async fn empty_workspace_reports_correct_emptiness_not_silent_empty() {
        let audit = Arc::new(AuditLog::new());
        let executor =
            CommandExecutor::start(migrated_memory_conn(), Arc::clone(&audit)).expect("启动执行器");
        let outcome = executor
            .execute(
                "ssh-1",
                read_actor(),
                None,
                WhCodeCommand::SessionList {
                    include_archived: false,
                },
            )
            .await
            .expect("执行成功");
        assert_eq!(outcome.exit_code, 0);
        assert!(
            outcome.stdout.contains("还没有会话"),
            "正确的空必须显式：{}",
            outcome.stdout
        );
    }

    /// 测试守卫：断言提前 panic 时也要开闸，否则执行线程阻塞在闸门上，`Drop` 的
    /// `join` 会永久等待（实测该测试曾因此挂起超过 60 s）。
    struct GateRelease(Arc<super::gate::Gate>);

    impl Drop for GateRelease {
        fn drop(&mut self) {
            self.0.open();
        }
    }

    #[tokio::test]
    async fn bounded_queue_rejects_with_backpressure_when_full() {
        let (executor, gate) =
            CommandExecutor::start_with_gate(migrated_memory_conn(), Arc::new(AuditLog::new()))
                .expect("启动执行器");
        let _release = GateRelease(Arc::clone(&gate));
        let executor = Arc::new(executor);
        let actor = read_actor();

        // 填满队列（容量 64）：闸门关闭，执行线程不消费。每个 spawn 之后让出执行权，
        // 使该任务完成 try_send 再挂起等回执——单线程运行时下队列被确定性填满。
        let mut tasks = Vec::new();
        for i in 0..COMMAND_QUEUE_CAPACITY {
            let executor = Arc::clone(&executor);
            let actor = actor.clone();
            tasks.push(tokio::spawn(async move {
                executor
                    .execute(
                        format!("ssh-{i}"),
                        actor,
                        None,
                        WhCodeCommand::SessionList {
                            include_archived: false,
                        },
                    )
                    .await
            }));
            tokio::task::yield_now().await;
        }
        // 第 65 条：背压拒绝（不阻塞、可重试），而不是无限积压。
        let err = executor
            .execute(
                "ssh-overflow",
                actor,
                None,
                WhCodeCommand::SessionList {
                    include_archived: false,
                },
            )
            .await
            .expect_err("队列满必须背压拒绝");
        assert_eq!(err.code, ErrorCode::Backpressure);
        assert!(err.retryable, "背压可重试");

        // 开闸：积压请求全部消化（排空语义），结果可用。
        gate.open();
        for (i, task) in tasks.into_iter().enumerate() {
            let outcome = task
                .await
                .expect("任务完成")
                .unwrap_or_else(|_| panic!("积压请求 {i} 必须被消化"));
            assert_eq!(outcome.exit_code, 0);
        }
    }

    #[tokio::test]
    async fn executor_drop_joins_thread_and_releases_connection() {
        let audit = Arc::new(AuditLog::new());
        let executor =
            CommandExecutor::start(migrated_memory_conn(), Arc::clone(&audit)).expect("启动执行器");
        // Drop：关闭队列 + join 线程（RAII；不 panic 即视为回收成功）。
        drop(executor);
    }

    #[tokio::test]
    async fn three_concurrent_sessions_write_through_single_executor() {
        // Check 98 进程内证据：3 个并发会话的写命令全部经唯一执行线程落库；
        // 审计 wrote 计数 == 3，且库中恰好 3 个会话（观察连接为测试基建，只读）。
        let audit = Arc::new(AuditLog::new());
        let observer = migrated_memory_conn();
        let server_conn = observer
            .try_clone()
            .expect("同实例观察连接（测试基建只读）");
        let executor =
            Arc::new(CommandExecutor::start(server_conn, Arc::clone(&audit)).expect("启动执行器"));
        let actor = actor_with_all_capabilities("ssh-concurrent");

        let mut tasks = Vec::new();
        for i in 0..3 {
            let executor = Arc::clone(&executor);
            let actor = actor.clone();
            tasks.push(tokio::spawn(async move {
                executor
                    .execute(
                        format!("ssh-{i}"),
                        actor,
                        None,
                        WhCodeCommand::SessionNew {
                            title: format!("并发会话{i}"),
                        },
                    )
                    .await
                    .expect("执行成功")
            }));
        }
        for task in tasks {
            let outcome = task.await.expect("任务完成");
            assert_eq!(outcome.exit_code, 0, "{}", outcome.stderr);
            assert!(outcome.stdout.starts_with("created S-"));
        }

        let entries = audit.snapshot();
        let writes: Vec<_> = entries.iter().filter(|e| e.wrote).collect();
        assert_eq!(writes.len(), 3, "写审计恰好 3 条：{entries:?}");
        assert!(
            entries.iter().all(|e| e.session_id.starts_with("ssh-")),
            "每个会话独立登记：{entries:?}"
        );

        // 库侧：3 个会话都在（写只经过执行线程的那条连接）。
        let count: i64 = observer
            .query_row("SELECT count(*) FROM sessions", [], |row| row.get(0))
            .expect("查询会话数");
        assert_eq!(count, 3, "并发写全部经单条连接落库");
    }

    #[test]
    fn output_bound_truncates_to_limit_and_flags() {
        let (text, truncated) = bounded_output(vec![b'a'; OUTPUT_LIMIT_BYTES]);
        assert!(!truncated);
        assert_eq!(text.len(), OUTPUT_LIMIT_BYTES);

        let mut bytes = vec![b'a'; OUTPUT_LIMIT_BYTES];
        bytes.extend_from_slice(b"WHCODE-TAIL");
        let (text, truncated) = bounded_output(bytes);
        assert!(truncated, "超出上限必须标记");
        assert_eq!(text.len(), OUTPUT_LIMIT_BYTES);
        assert!(text.ends_with("WHCODE-TAIL"), "保留尾部");
    }

    #[test]
    fn audit_log_is_bounded_and_counts_overflow() {
        let audit = AuditLog::new();
        let base = AuditRecord {
            at_ms: 1,
            session_id: "ssh-1".to_string(),
            actor_id: "a".to_string(),
            action: "exec".to_string(),
            wrote: false,
            ok: true,
            detail: String::new(),
        };
        for _ in 0..AUDIT_CAPACITY {
            audit.record(base.clone());
        }
        assert_eq!(audit.snapshot().len(), AUDIT_CAPACITY);
        assert_eq!(audit.overflow_count(), 0);
        // 超出：丢弃新条目并计数（不无限累积、不挤掉旧条目）。
        audit.record(base);
        assert_eq!(audit.snapshot().len(), AUDIT_CAPACITY);
        assert_eq!(audit.overflow_count(), 1);
    }
}
