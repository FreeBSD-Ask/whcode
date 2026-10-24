//! `whcode serve --web`：Web 服务进程入口（T090—T095 的装配根接线）。
//!
//! 契约（`contracts/cui-cli.md` §1）：`serve --web [--bind 127.0.0.1:PORT]`。
//!
//! 本模块只做**装配**：打开/初始化工作区库 → 绑定监听（V-110 门禁在
//! `whcode-web::bind`：默认仅回环，对外监听被拒）→ **单线程非阻塞多路复用**服务
//! 循环——DuckDB `Connection` 为 `Send` 非 `Sync`（ADR-002 的证据），请求处理不并发
//! 共享连接；请求体与 WS 帧有界（`MAX_MESSAGE_BYTES`）。
//!
//! WebSocket（Check 199/209、FR-603）：RFC 6455 握手成功后**保持连接**并注册为
//! 工作区事件订阅者——事件经 `whcode_web::broadcast` 增量轮询后广播下发。游标起点
//! 为启动时工作区最大 `seq`，历史事件经 `/api/events` 补拉（FR-202/605）；v1 不再
//! 立即关闭（原 M6 的“握手 + 关闭帧 1000”契约被事件推送取代）。订阅者上限
//! `broadcast::MAX_WS_CLIENTS`，超出在握手后回 `1013 Try Again Later` 并断开。
//!
//! ## 并发设计档案（ADR-015 口径；单线程多路复用）
//! - **结构**：listener 与全部客户端 socket 非阻塞；唯一循环线程每 tick：accept →
//!   请求/帧读泵 → 出站写泵 → 事件轮询（`Instant` 节流 [`EVENT_POLL_INTERVAL`]）→
//!   广播并逐帧刷新出站队列；整 tick 无进展时停靠 [`IDLE_PARK`]——空闲 CPU 近零。
//!   生产路径零新增线程（NFR-211）；测试内 spawn 的服务器/读帧线程属测试基建。
//! - **正**：无锁——`Connection` 单线程顺序使用即是当前和解（ADR-002）；无 async
//!   运行时与线程池的内存/调度开销；每 tick 最坏工作量有界：连接 ≤ [`MAX_CLIENTS`]、
//!   订阅者 ≤ `MAX_WS_CLIENTS`、单批帧 ≤ `broadcast::POLL_BATCH_FRAMES`（= 出站队列
//!   容量 64：一批帧即使全部入队也不溢出，溢出即真慢）、每客户端出站 ≤ 64 帧、
//!   入站帧缓冲 ≤ `MAX_MESSAGE_BYTES + 14`。
//! - **负（承担者：广播尾延迟与单核 CPU）**：广播延迟下界 = 轮询周期 200 ms；
//!   一次广播成本 O(订阅者数 × 帧数)；吞吐上限 = 64 帧/批 × 5 批/秒 = 320 事件/秒。
//!   慢客户端策略是“断开慢者，不停别人”：出站队列满即断开该连接（Check 199），
//!   超限入站帧只断发送者（Check 209）。
//! - **失效条件**：事件产生速率持续超过 320 事件/秒（尾延迟线性增长），或订阅者
//!   接近 32 且帧增大时单核成为瓶颈；此时本设计不再适配。
//! - **回退路径**：`broadcast::BroadcastState` 为纯逻辑（web 层），另立 ADR 评估
//!   线程池/async 后，可在不动协议字节变换（`whcode-web::ws`）与安全判定
//!   （`whcode-web::security`）的前提下替换并发模型。
//!
//! 认证/Origin 语义见 `whcode-web::security` 与 ADR-022：回环模式认证关闭、缺失
//! `Origin` 放行（同源 GET 与本机 CLI 客户端）；`serve` 无 token 参数——对外监听
//! 由 bind 层直接拒绝（FR-606），token 配置属未来任务。

use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use duckdb::Connection;
use whcode_app::{Actor, DefaultPolicy, actor_with_all_capabilities};
use whcode_domain::{ErrorCode, WhError};
use whcode_web::api::{self, HttpResponse, Method};
use whcode_web::assets;
use whcode_web::bind::{BindConfig, bind};
use whcode_web::broadcast::{self, BroadcastState, ClientId, PushResult};
use whcode_web::security;
use whcode_web::ws;

use crate::config::Config;
use crate::load_config;
use crate::workspace;

/// 空闲停靠时长：整 tick 无任何 I/O 进展时休眠，避免忙转（空闲 CPU 近零）。
const IDLE_PARK: Duration = Duration::from_millis(25);
/// 事件轮询周期（`Instant` 节流）：广播延迟下界，与订阅者上限构成显式取舍
/// （见模块文档并发设计档案）。
const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(200);
/// HTTP 请求读取空闲上限（沿用原 `set_read_timeout(5 s)` 语义：超时回 400）。
const REQUEST_IDLE_TIMEOUT: Duration = Duration::from_secs(5);
/// 响应/关闭帧写停滞上限：对端不读超过该时长即断开（写泵有界，NFR-215）。
const WRITE_STALL_TIMEOUT: Duration = Duration::from_secs(5);
/// 并发连接上限（HTTP 读取 + 写响应 + WS 订阅者总和；资源有界，NFR-215）。
const MAX_CLIENTS: usize = 96;
/// 请求头缓冲上限（与原 read_request 一致：16 KiB 头 + 1 MiB 体）。
const REQUEST_HEADER_CAP: usize = 16 * 1024;
/// WS 帧头最大字节数（2 固定 + 8 为 64 位长度 + 4 掩码）。
const WS_HEADER_MAX: usize = 14;
/// 订阅者入站帧缓冲上限：单帧最大载荷 + 最大帧头（超限断开，Check 209）。
const WS_INPUT_CAP: usize = security::MAX_MESSAGE_BYTES + WS_HEADER_MAX;
/// 控制帧（ping/close）载荷上限（RFC 6455 §5.5）。
const CONTROL_PAYLOAD_MAX: usize = 125;

const CLOSE_NORMAL: u16 = 1000;
const CLOSE_PROTOCOL_ERROR: u16 = 1002;
const CLOSE_MESSAGE_TOO_BIG: u16 = 1009;
const CLOSE_TRY_AGAIN_LATER: u16 = 1013;

/// `serve` 命令参数。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ServeCli {
    /// `--web`：启动 Web 服务。
    pub web: bool,
    /// `--ssh`：SSH 服务（M7 延后，未实现）。
    pub ssh: bool,
    /// `--bind host:port`（默认 `whcode_web::bind::DEFAULT_BIND`；仅回环或 RFC1918）。
    pub bind: Option<String>,
    /// `--token-file PATH`：认证 token 文件（局域网监听必填；token 只从文件读取）。
    pub token_file: Option<PathBuf>,
}

impl ServeCli {
    /// 解析 `serve` 参数；未知参数报错（错误可操作：列出可用参数）。
    ///
    /// # Errors
    ///
    /// 未知参数、缺值、`--web` 与 `--ssh` 同时给出或缺一时返回 [`WhError`]。
    pub fn parse(args: &[String]) -> Result<Self, WhError> {
        let mut cli = Self::default();
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--web" => cli.web = true,
                "--ssh" => cli.ssh = true,
                "--bind" => cli.bind = Some(next_value(&mut iter, "--bind")?),
                "--token-file" => {
                    cli.token_file = Some(PathBuf::from(next_value(&mut iter, "--token-file")?));
                }
                "--workspace" => {
                    // 值由 `workspace::resolve_workspace` 统一读取；此处仅校验缺值。
                    let _ = next_value(&mut iter, "--workspace")?;
                }
                // `--config PATH` 由 `load_config` 统一处理；此处跳过其值。
                "--config" => {
                    let _ = next_value(&mut iter, "--config")?;
                }
                other => {
                    return Err(WhError::parse_error(format!(
                        "未知参数 {other:?}：可用 --web / --ssh / --bind host:port / \
                         --token-file PATH / --workspace DIR / --config PATH"
                    )));
                }
            }
        }
        if cli.web && cli.ssh {
            return Err(WhError::invalid_state("--web 与 --ssh 只能选其一"));
        }
        if !cli.web && !cli.ssh {
            return Err(WhError::parse_error(
                "serve 缺少模式参数：可用 --web / --ssh",
            ));
        }
        Ok(cli)
    }
}

fn next_value<'a>(
    iter: &mut impl Iterator<Item = &'a String>,
    flag: &str,
) -> Result<String, WhError> {
    iter.next()
        .cloned()
        .ok_or_else(|| WhError::parse_error(format!("{flag} 缺少值")))
}

/// `serve` 命令入口。
pub fn serve_command(args: &[String]) -> ExitCode {
    let cli = match ServeCli::parse(args) {
        Ok(cli) => cli,
        Err(err) => {
            eprintln!("serve 参数错误：{err}");
            return ExitCode::FAILURE;
        }
    };
    let config = match load_config(args) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("配置加载失败：{err}");
            return ExitCode::FAILURE;
        }
    };
    let workspace = match workspace::resolve_workspace(args, &config) {
        Ok(workspace) => workspace,
        Err(err) => {
            eprintln!("工作区解析失败：{err}");
            return ExitCode::FAILURE;
        }
    };
    if cli.ssh {
        return match serve_ssh(&workspace, &config, cli.bind.as_deref()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("SSH 服务失败：{err}");
                ExitCode::FAILURE
            }
        };
    }
    match serve_web(
        &workspace,
        &config,
        cli.bind.as_deref(),
        cli.token_file.as_deref(),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("Web 服务失败：{err}");
            ExitCode::FAILURE
        }
    }
}

/// `serve --ssh` 装配（T096—T098、ADR-030）：工作区库 → SSH 状态目录（主机密钥/
/// 授权清单）→ 单写者执行器 → russh 监听。默认仅回环（`127.0.0.1:0`，OS 分配端口，
/// 启动时打印实际地址）；`--bind` 显式指定。`--token-file` 不适用于 SSH（认证是公钥）。
fn serve_ssh(workspace: &Path, config: &Config, bind_addr: Option<&str>) -> Result<(), WhError> {
    let connection = workspace::open_workspace_db(workspace, config)?;
    let params = whcode_ssh::server::SshServeParams {
        workspace_dir: workspace.to_path_buf(),
        connection,
        bind: bind_addr
            .unwrap_or(whcode_ssh::server::DEFAULT_BIND_ADDR)
            .to_string(),
        // M7 默认装配：只读能力 + 无 Shell（FR-608 安全默认；显式授权属配置面，见 ADR-030）。
        config: whcode_ssh::server::SshServerConfig::default(),
    };
    whcode_ssh::server::serve_ssh_blocking(params)
}

/// 服务安全配置（认证/Origin 允许集，ADR-023）。
struct ServeSecurity {
    auth_enabled: bool,
    token: Option<String>,
    /// Origin 允许集（认证模式生效；回环模式由 `security::ALLOWED_ORIGINS` 兜底）。
    origins: Vec<String>,
}

impl ServeSecurity {
    /// 回环默认（无认证；仅测试使用）。
    #[cfg(test)]
    fn loopback() -> Self {
        Self {
            auth_enabled: false,
            token: None,
            origins: Vec::new(),
        }
    }
}

/// 启动 Web 服务：工作区库 → 监听（回环或 RFC1918）→ 单线程多路复用服务循环。
///
/// 安全边界（ADR-023）：局域网监听必须提供 `--token-file`（认证 + Origin 检查）；
/// 公网/通配地址由 `bind` 层拒绝。
fn serve_web(
    workspace: &Path,
    config: &Config,
    bind_addr: Option<&str>,
    token_file: Option<&Path>,
) -> Result<(), WhError> {
    let db_path = workspace::workspace_db_path(workspace);
    // 写命令（FR-609）需要 `&mut Connection`（store 的事务签名）；服务循环本就单线程
    // 顺序处理请求，不存在并发共享（ADR-002）。
    let mut conn = workspace::open_workspace_db(workspace, config)?;
    let actor = actor_with_all_capabilities("web-local");

    let addr_text = bind_addr
        .unwrap_or(whcode_web::bind::DEFAULT_BIND)
        .to_string();
    let lan = whcode_web::bind::is_lan(&addr_text);
    let token = match token_file {
        Some(path) => {
            let raw = fs::read_to_string(path).map_err(|err| {
                WhError::new(
                    ErrorCode::IoError,
                    format!("读取 token 文件失败（{}）：{err}", path.display()),
                )
            })?;
            let token = raw.trim().to_string();
            if token.is_empty() {
                return Err(WhError::invalid_state(format!(
                    "token 文件为空（{}）：拒绝启动（ADR-023）",
                    path.display()
                )));
            }
            Some(token)
        }
        None => None,
    };
    if lan && token.is_none() {
        return Err(WhError::invalid_state(
            "局域网监听必须提供 --token-file（ADR-023：认证仅在显式 token 下启用；\
             token 只从文件读取，不经命令行传递）",
        ));
    }
    let auth_enabled = token.is_some();
    let bind_config = BindConfig {
        addr: addr_text,
        auth_enabled,
        origin_check_enabled: auth_enabled,
    };
    let listener = bind(&bind_config)?;
    let addr = listener
        .local_addr()
        .map_err(|err| WhError::new(ErrorCode::IoError, format!("读取监听地址失败：{err}")))?;
    let mut origins = vec![format!("http://{addr}")];
    if !addr.ip().is_loopback() {
        origins.push(format!("http://localhost:{}", addr.port()));
        origins.push(format!("http://127.0.0.1:{}", addr.port()));
    }
    if lan {
        println!("WHCode Web 服务：http://{addr}/（局域网 · 认证启用；Ctrl-C 退出；ADR-023）");
    } else {
        println!(
            "WHCode Web 服务：http://{addr}/（仅回环，Ctrl-C 退出；FR-606：外网地址被 bind 门禁拒绝）"
        );
    }
    println!("工作区数据库：{}", db_path.display());
    let security = ServeSecurity {
        auth_enabled,
        token,
        origins,
    };
    serve_loop(listener, &mut conn, actor, security)
}

/// 一次响应：HTTP 响应 + WS 升级标记。
struct Reply {
    response: HttpResponse,
    /// 101 握手成功：连接转入订阅者复用（事件广播，Check 199/209）——不再立即关闭。
    ws_upgrade: bool,
}

impl Reply {
    fn plain(response: HttpResponse) -> Self {
        Self {
            response,
            ws_upgrade: false,
        }
    }
}

/// 连接阶段（单线程状态机；阶段转移只发生在服务循环内）。
enum Phase {
    /// 读 HTTP 请求：有界累积缓冲 + 5 s 空闲上限（超时回 400，同原 read_timeout）。
    Reading {
        buf: Vec<u8>,
        header_end: Option<usize>,
        content_length: usize,
    },
    /// 写响应/握手字节；写完按 `become_subscriber` 转移（`Some` = 已注册的订阅者 id）
    /// 或关闭（`connection: close` 语义不变）。
    Writing {
        buf: Vec<u8>,
        pos: usize,
        become_subscriber: Option<ClientId>,
    },
    /// 已订阅：入站攒帧缓冲有界（[`WS_INPUT_CAP`]）；`out` 是写了一半的出站帧残余
    /// （PartialWrite 处理；至多一帧）。
    Subscriber {
        id: ClientId,
        input: Vec<u8>,
        out: Option<Vec<u8>>,
    },
    /// 写完剩余字节（错误响应/关闭帧）后断开；写停滞超时同样断开。
    Closing { buf: Vec<u8>, pos: usize },
}

/// 一条客户端连接（socket RAII：从连接表移除即关闭并注销订阅者）。
struct Client {
    stream: TcpStream,
    phase: Phase,
    last_active: Instant,
    /// 标记待回收：tick 末统一移除并注销（[`reap`]）。
    dead: bool,
}

/// 服务循环：单线程非阻塞多路复用（结构、正负与失效条件见模块文档并发设计档案）。
fn serve_loop(
    listener: TcpListener,
    conn: &mut Connection,
    actor: Actor,
    security: ServeSecurity,
) -> Result<(), WhError> {
    listener.set_nonblocking(true).map_err(|err| {
        WhError::new(ErrorCode::IoError, format!("监听套接字非阻塞化失败：{err}"))
    })?;
    let policy = DefaultPolicy;
    let mut state = BroadcastState::new();
    let mut clients: Vec<Client> = Vec::new();
    // 游标起点 = 启动时工作区最大 seq（与 build_snapshot().event_cursor 同一底层
    // `query::latest_seq`；直接取它是因为快照还要求已存在会话，空库会 NotFound）。
    // v1 只广播启动后的新事件；历史经 /api/events 补拉（FR-202/605）。
    let mut cursor = whcode_store::query::latest_seq(conn)?;
    let mut last_poll = Instant::now();
    loop {
        let mut progressed = false;
        accept_ready(&listener, &mut clients, &mut progressed);
        for client in clients.iter_mut() {
            pump_client(client, &mut state, conn, &actor, &security, &mut progressed);
        }
        if last_poll.elapsed() >= EVENT_POLL_INTERVAL {
            last_poll = Instant::now();
            match broadcast::poll_events(conn, &policy, &actor, cursor) {
                Ok(batch) => {
                    cursor = batch.cursor;
                    for frame in batch.frames {
                        progressed = true;
                        for slow in state.broadcast_frame(frame) {
                            // 慢客户端策略：断开慢者，不停别人。队列已满的连接不再写
                            // 关闭帧（只会继续积压），直接断开（Check 199）。
                            if let Some(client) = clients.iter_mut().find(|candidate| {
                                matches!(&candidate.phase, Phase::Subscriber { id, .. } if *id == slow)
                            }) {
                                client.dead = true;
                            }
                        }
                        // 逐帧即时刷新：正常客户端当帧送达（队列不积压），慢客户端
                        // 队列抬升、涨满即被断——广播延迟无劣化的落点（Check 199）。
                        flush_queues(&mut clients, &mut state, &mut progressed);
                    }
                }
                Err(err) => eprintln!("事件轮询失败：{err}"),
            }
        }
        reap(&mut clients, &mut state);
        if !progressed {
            std::thread::sleep(IDLE_PARK);
        }
    }
}

/// 接受就绪连接（非阻塞：一次 accept 尽可能多；超并发上限时尽力回 503 后丢弃——
/// 显式拒绝，不静默排队无限连接）。
fn accept_ready(listener: &TcpListener, clients: &mut Vec<Client>, progressed: &mut bool) {
    loop {
        match listener.accept() {
            Ok((stream, _addr)) => {
                *progressed = true;
                // 非阻塞化失败的连接会卡死唯一循环线程，与超限连接同样丢弃。
                if stream.set_nonblocking(true).is_err() || clients.len() >= MAX_CLIENTS {
                    let mut rejected = stream;
                    let _ = rejected.write_all(b"HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
                    drop(rejected);
                    continue;
                }
                clients.push(Client {
                    stream,
                    phase: Phase::Reading {
                        buf: Vec::with_capacity(4096),
                        header_end: None,
                        content_length: 0,
                    },
                    last_active: Instant::now(),
                    dead: false,
                });
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock => return,
            Err(err) => {
                eprintln!("接受连接失败：{err}");
                return;
            }
        }
    }
}

/// 推进单个连接：按阶段读（请求/WS 帧）与写（响应/出站队列）各尽力一次。
fn pump_client(
    client: &mut Client,
    state: &mut BroadcastState,
    conn: &mut Connection,
    actor: &Actor,
    security: &ServeSecurity,
    progressed: &mut bool,
) {
    if client.dead {
        return;
    }
    if matches!(&client.phase, Phase::Reading { .. }) {
        pump_reading(client, conn, actor, security, state, progressed);
        return;
    }
    if matches!(&client.phase, Phase::Subscriber { .. }) {
        pump_subscriber(client, state, progressed);
        return;
    }
    if matches!(&client.phase, Phase::Writing { .. }) {
        pump_writing(client, state, progressed);
        return;
    }
    pump_closing(client, progressed);
}

/// HTTP 请求读取泵：非阻塞读 + 有界累积 + 完整性判定；完成后路由。
fn pump_reading(
    client: &mut Client,
    conn: &mut Connection,
    actor: &Actor,
    security: &ServeSecurity,
    state: &mut BroadcastState,
    progressed: &mut bool,
) {
    read_request_bytes(client, progressed);
    if client.dead {
        return;
    }
    // 5 s 内未读出完整请求 → 400（沿用原实现 read_timeout 的语义）。
    if client.last_active.elapsed() >= REQUEST_IDLE_TIMEOUT {
        let reply = Reply::plain(HttpResponse::text(400, "bad request"));
        enter_writing(client, &reply, None);
        return;
    }
    let outcome = {
        let Phase::Reading {
            buf,
            header_end,
            content_length,
        } = &mut client.phase
        else {
            return;
        };
        if buf.len() > security::MAX_MESSAGE_BYTES + REQUEST_HEADER_CAP {
            RequestOutcome::Oversize
        } else {
            if header_end.is_none()
                && let Some(position) = find_subsequence(buf, b"\r\n\r\n")
            {
                *header_end = Some(position);
                *content_length = header_content_length(&buf[..position]);
            }
            if header_end.is_some_and(|end| buf.len() >= end + 4 + *content_length) {
                RequestOutcome::Complete(std::mem::take(buf))
            } else {
                RequestOutcome::Pending
            }
        }
    };
    match outcome {
        RequestOutcome::Pending => {}
        RequestOutcome::Oversize => {
            let reply = Reply::plain(HttpResponse::text(400, "bad request"));
            enter_writing(client, &reply, None);
        }
        RequestOutcome::Complete(raw) => {
            let reply = respond(conn, &raw, actor, security);
            route_reply(client, reply, state);
        }
    }
}

/// 请求完整性判定结果（把判定与副作用分开，避免阶段借用纠缠）。
enum RequestOutcome {
    /// 还没读齐，等下一个 tick。
    Pending,
    /// 超过请求上限 → 400。
    Oversize,
    /// 完整请求字节。
    Complete(Vec<u8>),
}

/// 非阻塞读尽当前可读字节（有界累积在 [`Phase::Reading`] 的 buf 中）。
fn read_request_bytes(client: &mut Client, progressed: &mut bool) {
    let mut chunk = [0u8; 4096];
    loop {
        match client.stream.read(&mut chunk) {
            Ok(0) => {
                // 对端关闭：半途请求无法也不必响应（原实现读到 EOF 同样失败）。
                client.dead = true;
                return;
            }
            Ok(n) => {
                *progressed = true;
                client.last_active = Instant::now();
                if let Phase::Reading { buf, .. } = &mut client.phase {
                    buf.extend_from_slice(&chunk[..n]);
                }
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock => return,
            Err(_) => {
                client.dead = true;
                return;
            }
        }
    }
}

/// 路由响应：普通 HTTP → 写完即断（`connection: close` 不变）；WS 101 → 注册为
/// 订阅者并保持连接；注册被拒（超订阅者上限）→ 握手 + 1013 关闭帧后断开（显式拒绝）。
fn route_reply(client: &mut Client, reply: Reply, state: &mut BroadcastState) {
    if !reply.ws_upgrade {
        enter_writing(client, &reply, None);
        return;
    }
    match state.register() {
        Ok(id) => enter_writing(client, &reply, Some(id)),
        Err(_) => {
            let mut bytes = render_response(&reply.response);
            bytes.extend_from_slice(&close_frame(CLOSE_TRY_AGAIN_LATER));
            enter_writing_bytes(client, bytes, None);
        }
    }
}

/// 进入响应写出阶段（`pos: 0`；写停滞计时从此刻起）。
fn enter_writing(client: &mut Client, reply: &Reply, become_subscriber: Option<ClientId>) {
    let bytes = render_response(&reply.response);
    enter_writing_bytes(client, bytes, become_subscriber);
}

fn enter_writing_bytes(client: &mut Client, buf: Vec<u8>, become_subscriber: Option<ClientId>) {
    client.phase = Phase::Writing {
        buf,
        pos: 0,
        become_subscriber,
    };
    client.last_active = Instant::now();
}

/// 响应写出泵：非阻塞写，PartialWrite 留待下个 tick；写完按语义转移或关闭。
fn pump_writing(client: &mut Client, state: &mut BroadcastState, progressed: &mut bool) {
    if client.last_active.elapsed() >= WRITE_STALL_TIMEOUT {
        // 对端不读：放弃。已注册但握手未写完的订阅者必须注销（注册发生在写出前）。
        if let Phase::Writing {
            become_subscriber: Some(id),
            ..
        } = &client.phase
        {
            state.unregister(*id);
        }
        client.dead = true;
        return;
    }
    let (mut buf, mut pos, become_subscriber) = match &mut client.phase {
        Phase::Writing {
            buf,
            pos,
            become_subscriber,
        } => (std::mem::take(buf), *pos, *become_subscriber),
        _ => return,
    };
    while pos < buf.len() {
        match client.stream.write(&buf[pos..]) {
            Ok(0) => break,
            Ok(n) => {
                pos += n;
                client.last_active = Instant::now();
                *progressed = true;
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock => break,
            Err(_) => {
                if let Some(id) = become_subscriber {
                    state.unregister(id);
                }
                client.dead = true;
                return;
            }
        }
    }
    if pos >= buf.len() {
        match become_subscriber {
            Some(id) => {
                client.phase = Phase::Subscriber {
                    id,
                    input: Vec::new(),
                    out: None,
                };
            }
            None => client.dead = true,
        }
    } else {
        buf.drain(..pos);
        if let Phase::Writing { buf: slot, .. } = &mut client.phase {
            *slot = buf;
        }
    }
}

/// 收尾写出泵（错误响应/关闭帧）：写完即断；停滞超 [`WRITE_STALL_TIMEOUT`] 断开。
fn pump_closing(client: &mut Client, progressed: &mut bool) {
    if client.last_active.elapsed() >= WRITE_STALL_TIMEOUT {
        client.dead = true;
        return;
    }
    let (mut buf, mut pos) = match &mut client.phase {
        Phase::Closing { buf, pos } => (std::mem::take(buf), *pos),
        _ => return,
    };
    while pos < buf.len() {
        match client.stream.write(&buf[pos..]) {
            Ok(0) => break,
            Ok(n) => {
                pos += n;
                client.last_active = Instant::now();
                *progressed = true;
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock => break,
            Err(_) => {
                client.dead = true;
                return;
            }
        }
    }
    if pos >= buf.len() {
        client.dead = true;
    } else {
        buf.drain(..pos);
        if let Phase::Closing { buf: slot, .. } = &mut client.phase {
            *slot = buf;
        }
    }
}

/// 订阅者泵：读入站帧（ping→pong、close→注销、超限/分片/非文本→只断该客户端）
/// 并写出站队列。
fn pump_subscriber(client: &mut Client, state: &mut BroadcastState, progressed: &mut bool) {
    let id = match &client.phase {
        Phase::Subscriber { id, .. } => *id,
        _ => return,
    };
    read_subscriber_bytes(client, state, progressed);
    if !client.dead {
        write_subscriber_queue(client, state, id, progressed);
    }
}

/// 订阅者入站读取：非阻塞读 + 逐块消化完整帧（入站缓冲不随流水帧无界增长）。
fn read_subscriber_bytes(client: &mut Client, state: &mut BroadcastState, progressed: &mut bool) {
    let id = match &client.phase {
        Phase::Subscriber { id, .. } => *id,
        _ => return,
    };
    let mut chunk = [0u8; 4096];
    loop {
        match client.stream.read(&mut chunk) {
            Ok(0) => {
                client.dead = true; // 对端关闭
                return;
            }
            Ok(n) => {
                *progressed = true;
                client.last_active = Instant::now();
                if let Phase::Subscriber { input, .. } = &mut client.phase {
                    input.extend_from_slice(&chunk[..n]);
                }
                process_ws_frames(client, state, id);
                if client.dead {
                    return;
                }
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock => return,
            Err(_) => {
                client.dead = true;
                return;
            }
        }
    }
}

/// WS 帧头（客户端→服务端；连接管理属本模块——`whcode-web::ws` 只做字节变换）。
struct WsFrameHeader {
    fin: bool,
    opcode: u8,
    masked: bool,
    payload_len: usize,
    header_len: usize,
}

/// 单帧解析结论。
enum WsDecision {
    /// 头未齐或载荷未齐：攒帧，等下一个 tick。
    Wait,
    /// 拒绝（附关闭码）：只断该客户端。
    Reject(u16),
    /// 一帧完整字节与帧头。
    Frame(WsFrameHeader, Vec<u8>),
}

/// 帧头解析结果。
enum WsHeader {
    NeedMore,
    /// 声明载荷超过 `MAX_MESSAGE_BYTES`：不等后续字节，立即拒绝（§10.2）。
    Oversize,
    Ready(WsFrameHeader),
}

/// 解析帧头（前 2/4/10 字节）；64 位长度立即按上限判定，避免为巨帧攒缓冲。
fn parse_ws_header(bytes: &[u8]) -> WsHeader {
    if bytes.len() < 2 {
        return WsHeader::NeedMore;
    }
    let fin = bytes[0] & 0x80 != 0;
    let opcode = bytes[0] & 0x0F;
    let masked = bytes[1] & 0x80 != 0;
    let mut header_len = 2usize;
    let payload_len = match (bytes[1] & 0x7F) as usize {
        126 => {
            if bytes.len() < 4 {
                return WsHeader::NeedMore;
            }
            header_len = 4;
            u16::from_be_bytes([bytes[2], bytes[3]]) as usize
        }
        127 => {
            if bytes.len() < 10 {
                return WsHeader::NeedMore;
            }
            header_len = 10;
            let declared = u64::from_be_bytes([
                bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7], bytes[8], bytes[9],
            ]);
            if declared > security::MAX_MESSAGE_BYTES as u64 {
                return WsHeader::Oversize;
            }
            declared as usize
        }
        len7 => len7,
    };
    if payload_len > security::MAX_MESSAGE_BYTES {
        return WsHeader::Oversize;
    }
    WsHeader::Ready(WsFrameHeader {
        fin,
        opcode,
        masked,
        payload_len,
        header_len,
    })
}

/// 逐帧消化入站缓冲：攒齐一帧处理一帧（增量解析）。
fn process_ws_frames(client: &mut Client, state: &mut BroadcastState, id: ClientId) {
    loop {
        if client.dead {
            return;
        }
        let decision = {
            let Phase::Subscriber { input, .. } = &mut client.phase else {
                return;
            };
            match parse_ws_header(input) {
                WsHeader::NeedMore => {
                    if input.len() > WS_INPUT_CAP {
                        // 纵深防御：合法流的部分帧不会触及该上限（单帧超限在头部即拒）。
                        WsDecision::Reject(CLOSE_MESSAGE_TOO_BIG)
                    } else {
                        WsDecision::Wait
                    }
                }
                WsHeader::Oversize => WsDecision::Reject(CLOSE_MESSAGE_TOO_BIG),
                WsHeader::Ready(header) => {
                    let total =
                        header.header_len + usize::from(header.masked) * 4 + header.payload_len;
                    if input.len() < total {
                        WsDecision::Wait
                    } else {
                        let frame: Vec<u8> = input.drain(..total).collect();
                        WsDecision::Frame(header, frame)
                    }
                }
            }
        };
        match decision {
            WsDecision::Wait => return,
            WsDecision::Reject(code) => {
                reject_subscriber(client, state, id, code);
                return;
            }
            WsDecision::Frame(header, frame) => {
                handle_client_frame(client, state, id, &header, &frame);
            }
        }
    }
}

/// 处理一帧：控制帧（ping/close）与文本帧；非文本/分片/未掩码 → 拒绝（语义与
/// `decode_client_frame` 一致，只断该客户端——Check 209）。
fn handle_client_frame(
    client: &mut Client,
    state: &mut BroadcastState,
    id: ClientId,
    header: &WsFrameHeader,
    frame: &[u8],
) {
    match header.opcode {
        0x8 => {
            // close：注销并回正常关闭帧（Closing 写完即断）。
            state.unregister(id);
            let buf = close_frame(CLOSE_NORMAL);
            client.phase = Phase::Closing { buf, pos: 0 };
            client.last_active = Instant::now();
        }
        0x9 => {
            // ping → pong（控制帧必须不分片且 ≤ 125 字节，RFC 6455 §5.5）。
            if !header.fin || !header.masked || header.payload_len > CONTROL_PAYLOAD_MAX {
                reject_subscriber(client, state, id, CLOSE_PROTOCOL_ERROR);
                return;
            }
            let payload = unmask(header, frame);
            let mut pong = Vec::with_capacity(payload.len() + 2);
            pong.push(0x8A);
            pong.push(payload.len() as u8);
            pong.extend_from_slice(&payload);
            if let Some(queue) = state.queue_mut(id)
                && queue.push(pong) == PushResult::Overflowed
            {
                client.dead = true; // reap 统一注销
            }
        }
        0xA => {
            // pong：忽略（心跳回执，无状态）。
        }
        _ => {
            // 数据帧：文本、无分片、掩码语义与 decode_client_frame 一致。
            // v1 客户端文本不驱动任何状态（广播是单向流），合法帧静默消费。
            if !header.fin {
                // 分片不支持（M6 口径），拒绝语义不变。
                reject_subscriber(client, state, id, CLOSE_PROTOCOL_ERROR);
                return;
            }
            if ws::decode_client_frame(frame, security::MAX_MESSAGE_BYTES).is_err() {
                reject_subscriber(client, state, id, CLOSE_PROTOCOL_ERROR);
            }
        }
    }
}

/// 提取并去掩码的帧载荷（frame 已含完整头 + 掩码 + 载荷）。
fn unmask(header: &WsFrameHeader, frame: &[u8]) -> Vec<u8> {
    let mask_start = header.header_len;
    let payload_start = mask_start + 4;
    let mask = &frame[mask_start..payload_start];
    frame[payload_start..]
        .iter()
        .enumerate()
        .map(|(index, byte)| byte ^ mask[index % 4])
        .collect()
}

/// 拒绝订阅者：注销 + 尽力发送关闭帧后断开（只断该客户端，不影响他人）。
fn reject_subscriber(client: &mut Client, state: &mut BroadcastState, id: ClientId, code: u16) {
    state.unregister(id);
    let buf = close_frame(code);
    client.phase = Phase::Closing { buf, pos: 0 };
    client.last_active = Instant::now();
}

/// 订阅者出站写出：先写上一帧残余（PartialWrite），再从队列续弹；
/// 每帧写尽或 WouldBlock 为止（队列深度因此保持浅——Check 199 的时延保证落点）。
fn write_subscriber_queue(
    client: &mut Client,
    state: &mut BroadcastState,
    id: ClientId,
    progressed: &mut bool,
) {
    let mut pending = match &mut client.phase {
        Phase::Subscriber { out, .. } => out.take().unwrap_or_default(),
        _ => return,
    };
    loop {
        if pending.is_empty()
            && let Some(frame) = state.queue_mut(id).and_then(|queue| queue.pop())
        {
            pending = frame;
        }
        if pending.is_empty() {
            break;
        }
        match client.stream.write(&pending) {
            Ok(0) => break,
            Ok(n) => {
                pending.drain(..n);
                client.last_active = Instant::now();
                *progressed = true;
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock => break,
            Err(_) => {
                client.dead = true; // reap 统一注销（socket RAII）
                break;
            }
        }
    }
    if let Phase::Subscriber { out, .. } = &mut client.phase {
        *out = (!pending.is_empty()).then_some(pending);
    }
}

/// 广播后即时刷新所有订阅者出站队列（正常客户端当帧送达；慢客户端队列抬升，
/// 涨满即被断——Check 199 的“断开慢者不停别人”落点）。
fn flush_queues(clients: &mut [Client], state: &mut BroadcastState, progressed: &mut bool) {
    for client in clients.iter_mut() {
        if client.dead {
            continue;
        }
        let id = match &client.phase {
            Phase::Subscriber { id, .. } => *id,
            _ => continue,
        };
        write_subscriber_queue(client, state, id, progressed);
    }
}

/// 回收死连接（socket RAII：移除即关闭；订阅者移除即注销——重复注销幂等）。
fn reap(clients: &mut Vec<Client>, state: &mut BroadcastState) {
    clients.retain(|client| {
        if client.dead
            && let Phase::Subscriber { id, .. } = &client.phase
        {
            state.unregister(*id);
        }
        !client.dead
    });
}

/// 服务端→客户端关闭帧（不掩码；2 字节状态码）。
fn close_frame(code: u16) -> Vec<u8> {
    vec![0x88, 0x02, (code >> 8) as u8, (code & 0xFF) as u8]
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// 从请求头提取 `Content-Length`（缺失或非法按 0——本服务只消费 GET）。
fn header_content_length(headers: &[u8]) -> usize {
    let text = String::from_utf8_lossy(headers);
    text.lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())?
        })
        .unwrap_or(0)
}

/// 路由一个请求：静态资产（GET，路由前剥离查询串）→ `/api/*`（handle_request，
/// ADR-022 Origin 语义）。
fn respond(
    conn: &mut Connection,
    raw: &[u8],
    actor: &whcode_app::Actor,
    security: &ServeSecurity,
) -> Reply {
    let request = match api::parse_request(raw) {
        Ok(request) => request,
        Err(_) => return Reply::plain(HttpResponse::text(400, "bad request")),
    };
    if request.method == Method::Get {
        // 首页可带查询串（`/?session=S-DEMO-1`）：静态路由只看路径部分。
        let path = request
            .path
            .split('?')
            .next()
            .unwrap_or(request.path.as_str());
        // 首页服务端直出：注入只读会话快照（无 JS 时 `<noscript>` 可见，FR-618）。
        // 启用认证时不注入数据：业务数据只经认证后的 `/api/*`（ADR-022）。
        if path == "/" || path == "/index.html" {
            return Reply::plain(index_response(conn, actor, security));
        }
        if let Some((content_type, body)) = assets::static_asset(path) {
            return Reply::plain(HttpResponse {
                status: 200,
                headers: vec![("content-type".to_string(), content_type.to_string())],
                body: body.to_vec(),
            });
        }
    }
    let is_ws = request.path == "/ws";
    let origins: Vec<&str> = security.origins.iter().map(String::as_str).collect();
    // 三档能力等级（FR-702、Check 101）：只注入声明，不在请求路径启动子进程。
    let runtime_inventory = serde_json::to_value(crate::runtimes::inventory(false))
        .unwrap_or_else(|_| serde_json::Value::Array(Vec::new()));
    let response = api::handle_request_with_runtimes(
        conn,
        security.auth_enabled,
        security.token.as_deref(),
        &origins,
        &request,
        actor,
        Some(&runtime_inventory),
    );
    Reply {
        ws_upgrade: is_ws && response.status == 101,
        response,
    }
}

/// 首页首屏只读快照的会话上限（HTML 直出需守住 DOM 预算，比 TUI/GUI 的 200 更小）。
const INITIAL_SESSION_LIMIT: usize = 50;

/// 服务端直出首页（FR-618）：把会话快照渲染进 `<noscript>`。
///
/// 查询失败必须渲染成可见错误——不得用“空列表”冒充“没有会话”（信号链完备原则）。
/// 启用认证时**不注入任何业务数据**（ADR-022）：首页是浏览器导航，无法携带 Bearer 头，
/// 直出快照会绕过认证边界。
fn index_response(
    conn: &Connection,
    actor: &whcode_app::Actor,
    security: &ServeSecurity,
) -> HttpResponse {
    let body = if security.auth_enabled {
        assets::render_index_auth_required()
    } else {
        let listing = whcode_app::session::list_sessions(
            conn,
            &DefaultPolicy,
            actor,
            false,
            // 多取一条：用来区分“恰好 50 条”与“被截断”，不得让读者误判总量。
            INITIAL_SESSION_LIMIT + 1,
        );
        let (sessions, truncated, error) = match listing {
            Ok(mut rows) => {
                let truncated = rows.len() > INITIAL_SESSION_LIMIT;
                if truncated {
                    rows.truncate(INITIAL_SESSION_LIMIT);
                }
                (
                    rows.into_iter()
                        .map(|row| assets::InitialSession {
                            id: row.id,
                            title: row.title,
                            status: row.status,
                        })
                        .collect::<Vec<_>>(),
                    truncated,
                    None,
                )
            }
            Err(err) => (Vec::new(), false, Some(err.message)),
        };
        assets::render_index(&sessions, truncated, error.as_deref())
    };
    HttpResponse {
        status: 200,
        headers: vec![
            (
                "content-type".to_string(),
                "text/html; charset=utf-8".to_string(),
            ),
            ("cache-control".to_string(), "no-store".to_string()),
        ],
        body: body.into_bytes(),
    }
}

/// 渲染响应字节：状态行 + 头 + 体。101 不带 Content-Length 与 `connection: close`
/// （RFC 7230：升级后连接保持——订阅复用，不再立即关闭）。
fn render_response(response: &HttpResponse) -> Vec<u8> {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\n",
        response.status,
        reason(response.status)
    );
    for (name, value) in &response.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    if response.status != 101 {
        head.push_str(&format!("content-length: {}\r\n", response.body.len()));
        head.push_str("connection: close\r\n");
    }
    head.push_str("\r\n");
    let mut bytes = head.into_bytes();
    bytes.extend_from_slice(&response.body);
    bytes
}

/// 状态码短语（响应可读性；未知码返回空）。
fn reason(status: u16) -> &'static str {
    match status {
        101 => "Switching Protocols",
        200 => "OK",
        201 => "Created",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use std::io::{ErrorKind, Read as _, Write as _};
    use std::net::{SocketAddr, TcpStream};
    use std::time::{Duration, Instant};

    use duckdb::{Connection, params};
    use whcode_web::ws::accept_key;

    use super::{
        INITIAL_SESSION_LIMIT, Reply, ServeCli, ServeSecurity, header_content_length, reason,
        respond, serve_loop, serve_ssh,
    };
    use crate::workspace::WORKSPACE_DB_FILE;
    use whcode_app::{Actor, actor_with_all_capabilities};
    use whcode_domain::{ErrorCode, SCHEMA_VERSION};
    use whcode_store::apply_migrations;

    fn migrated_memory_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("内存库");
        apply_migrations(&conn).expect("迁移");
        conn
    }

    fn raw_request(request: &str) -> Vec<u8> {
        request.as_bytes().to_vec()
    }

    #[test]
    fn parse_accepts_ssh_mode_with_bind() {
        let ssh: [String; 3] = ["--ssh".into(), "--bind".into(), "127.0.0.1:2222".into()];
        let cli = ServeCli::parse(&ssh).expect("解析");
        assert!(cli.ssh);
        assert!(!cli.web);
        assert_eq!(cli.bind.as_deref(), Some("127.0.0.1:2222"));
    }

    /// `serve --ssh` 装配链（fail closed）：工作区没有授权公钥清单时必须报错且错误可操作，
    /// 不得静默回退为“无授权”或“全授权”（T096、ADR-030）。
    #[test]
    fn ssh_assembly_fails_closed_without_authorized_keys() {
        use crate::config::Config;
        let dir = std::env::temp_dir().join(format!("whcode-serve-ssh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时工作区");
        let config = Config::for_workspace(&dir);
        let outcome = serve_ssh(&dir, &config, None);
        let _ = std::fs::remove_dir_all(&dir);
        let err = outcome.expect_err("缺授权清单必须失败");
        assert!(
            err.message.contains("authorized_keys"),
            "错误必须指明缺什么、怎么补：{err}"
        );
    }

    #[test]
    fn parse_requires_exactly_one_mode() {
        let web: [String; 1] = ["--web".into()];
        assert!(ServeCli::parse(&web).expect("解析").web);
        let both: [String; 2] = ["--web".into(), "--ssh".into()];
        let err = ServeCli::parse(&both).expect_err("--web 与 --ssh 只能选其一");
        assert_eq!(err.code, ErrorCode::InvalidState);
        let none: [String; 0] = [];
        assert!(ServeCli::parse(&none).is_err(), "缺模式必须报错");
        let unknown: [String; 1] = ["--port".into()];
        let err = ServeCli::parse(&unknown).expect_err("未知参数必须报错");
        assert!(err.message.contains("--web"), "错误须列出可用参数：{err}");
        // 带值参数解析。
        let with_bind: [String; 2] = ["--web".into(), "--bind".into()];
        assert!(ServeCli::parse(&with_bind).is_err(), "--bind 缺值必须报错");
        // --token-file：缺值报错；有值时解析为路径（ADR-023）。
        let with_token: [String; 2] = ["--web".into(), "--token-file".into()];
        assert!(
            ServeCli::parse(&with_token).is_err(),
            "--token-file 缺值必须报错"
        );
        let token_file: [String; 3] = [
            "--web".into(),
            "--token-file".into(),
            "C:/tmp/token.txt".into(),
        ];
        assert_eq!(
            ServeCli::parse(&token_file).expect("解析").token_file,
            Some(std::path::PathBuf::from("C:/tmp/token.txt"))
        );
    }

    /// 装配路由：静态资产、API（ADR-022 Origin 语义）、写命令、404 与 WS 握手。
    #[test]
    fn assembled_routes_serve_static_api_and_ws_handshake() {
        let mut conn = migrated_memory_conn();
        let actor = actor_with_all_capabilities("tester");
        let security = ServeSecurity::loopback();
        let mut send = |request: &str| respond(&mut conn, &raw_request(request), &actor, &security);

        // 静态资产（GET）。
        let reply = send("GET / HTTP/1.1\r\nhost: 127.0.0.1\r\n\r\n");
        assert_eq!(reply.response.status, 200);
        assert_eq!(
            reply.response.header("content-type"),
            Some("text/html; charset=utf-8")
        );
        assert!(String::from_utf8_lossy(&reply.response.body).contains("WHCode"));
        let reply = send("GET /app.js HTTP/1.1\r\n\r\n");
        assert!(
            reply
                .response
                .header("content-type")
                .is_some_and(|value| value.starts_with("text/javascript"))
        );
        // 带查询串的首页（`/?session=`）必须命中静态路由（回归：曾落 404）。
        let reply = send("GET /?session=S-DEMO-1 HTTP/1.1\r\n\r\n");
        assert_eq!(reply.response.status, 200);
        assert!(String::from_utf8_lossy(&reply.response.body).contains("WHCode"));

        // POST 不属于静态路由 → 404。
        let reply = send("POST / HTTP/1.1\r\n\r\n");
        assert_eq!(reply.response.status, 404);

        // API：回环模式缺 Origin 放行（ADR-022）；数据为空数组（正确的空）。
        let reply = send("GET /api/sessions HTTP/1.1\r\nhost: 127.0.0.1\r\n\r\n");
        assert_eq!(reply.response.status, 200);
        let body: serde_json::Value = serde_json::from_slice(&reply.response.body).expect("JSON");
        assert_eq!(body["schema_version"], SCHEMA_VERSION);
        assert_eq!(body["sessions"], serde_json::json!([]));

        // 写命令（FR-609）：POST + JSON 体 → 201 创建；随后列表可见。
        let payload = r#"{"title":"装配根写命令"}"#;
        let reply = send(&format!(
            "POST /api/sessions/new HTTP/1.1\r\ncontent-type: application/json\r\n\
             content-length: {}\r\n\r\n{payload}",
            payload.len()
        ));
        assert_eq!(reply.response.status, 201);
        let body: serde_json::Value = serde_json::from_slice(&reply.response.body).expect("JSON");
        assert_eq!(body["session"]["title"], "装配根写命令");

        // API：非回环 Origin 拒绝（威胁模型不变）。
        let reply = send("GET /api/sessions HTTP/1.1\r\norigin: http://evil.example\r\n\r\n");
        assert_eq!(reply.response.status, 403);

        // WS：缺 Sec-WebSocket-Key → 400；有 key → 101 + 升级标记（订阅复用）。
        let reply = send("GET /ws HTTP/1.1\r\n\r\n");
        assert_eq!(reply.response.status, 400);
        let reply = send("GET /ws HTTP/1.1\r\nsec-websocket-key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n");
        assert_eq!(reply.response.status, 101);
        assert!(
            reply.ws_upgrade,
            "握手成功必须标记升级（连接保持进入订阅复用，不再立即关闭）"
        );

        // 未知 API 路径 → 404。
        let reply = send("GET /api/nope HTTP/1.1\r\n\r\n");
        assert_eq!(reply.response.status, 404);
    }

    /// 无 JS 兜底（Check 229）：首页由服务端直出会话快照，标题必须转义。
    #[test]
    fn index_page_renders_readonly_snapshot_server_side() {
        let mut conn = migrated_memory_conn();
        let actor = actor_with_all_capabilities("tester");
        let security = ServeSecurity::loopback();
        let payload = r#"{"title":"<script>alert(1)</script> 只读标题"}"#;
        let create = respond(
            &mut conn,
            &raw_request(&format!(
                "POST /api/sessions/new HTTP/1.1\r\ncontent-type: application/json\r\n\
                 content-length: {}\r\n\r\n{payload}",
                payload.len()
            )),
            &actor,
            &security,
        );
        assert_eq!(
            create.response.status,
            201,
            "{}",
            String::from_utf8_lossy(&create.response.body)
        );

        let reply = respond(
            &mut conn,
            &raw_request("GET / HTTP/1.1\r\nhost: 127.0.0.1\r\n\r\n"),
            &actor,
            &security,
        );
        assert_eq!(reply.response.status, 200);
        let body = String::from_utf8_lossy(&reply.response.body).into_owned();
        assert!(body.contains("<noscript>"), "无 JS 时必须有只读快照");
        assert!(body.contains("只读快照"), "快照必须有可读标题");
        assert!(
            body.contains("&lt;script&gt;alert(1)&lt;/script&gt; 只读标题"),
            "数据库文本必须经 HTML 转义"
        );
        assert!(
            !body.contains("<script>alert(1)</script>"),
            "不得把会话标题原样注入 HTML"
        );
    }

    /// 认证边界（ADR-022）：启用认证时首页不注入业务数据（浏览器导航无法带 Bearer 头）。
    #[test]
    fn index_page_withholds_snapshot_when_auth_enabled() {
        let mut conn = migrated_memory_conn();
        let actor = actor_with_all_capabilities("tester");
        let payload = r#"{"title":"认证模式秘密标题"}"#;
        let create = respond(
            &mut conn,
            &raw_request(&format!(
                "POST /api/sessions/new HTTP/1.1\r\ncontent-type: application/json\r\n\
                 content-length: {}\r\n\r\n{payload}",
                payload.len()
            )),
            &actor,
            &ServeSecurity::loopback(),
        );
        assert_eq!(create.response.status, 201);

        let authenticated = ServeSecurity {
            auth_enabled: true,
            token: Some("secret-token".to_string()),
            origins: vec!["http://127.0.0.1".to_string()],
        };
        let reply = respond(
            &mut conn,
            &raw_request("GET / HTTP/1.1\r\nhost: 127.0.0.1\r\n\r\n"),
            &actor,
            &authenticated,
        );
        assert_eq!(reply.response.status, 200);
        let body = String::from_utf8_lossy(&reply.response.body).into_owned();
        assert!(
            !body.contains("认证模式秘密标题"),
            "认证模式不得直出会话数据（ADR-022）"
        );
        assert!(body.contains("<noscript>"), "仍需无 JS 说明");
        assert!(body.contains("仅在本机回环模式"), "必须写明快照的提供条件");
        assert!(
            !body.contains("<!-- WHCODE_INITIAL_VIEW -->"),
            "标记必须被替换"
        );
    }

    /// 截断必须显式（信号链完备）：超过上限时写明“可能还有更多”，而不是静默切掉。
    #[test]
    fn index_page_flags_truncated_snapshot_instead_of_silent_cut() {
        let mut conn = migrated_memory_conn();
        let actor = actor_with_all_capabilities("tester");
        let security = ServeSecurity::loopback();
        for index in 1..=(INITIAL_SESSION_LIMIT + 1) {
            let payload = format!(r#"{{"title":"会话 {index}"}}"#);
            let create = respond(
                &mut conn,
                &raw_request(&format!(
                    "POST /api/sessions/new HTTP/1.1\r\ncontent-type: application/json\r\n\
                     content-length: {}\r\n\r\n{payload}",
                    payload.len()
                )),
                &actor,
                &security,
            );
            assert_eq!(
                create.response.status,
                201,
                "第 {index} 个会话创建失败：{}",
                String::from_utf8_lossy(&create.response.body)
            );
        }

        let reply = respond(
            &mut conn,
            &raw_request("GET / HTTP/1.1\r\nhost: 127.0.0.1\r\n\r\n"),
            &actor,
            &security,
        );
        let body = String::from_utf8_lossy(&reply.response.body).into_owned();
        assert_eq!(
            body.matches("<li><code>").count(),
            INITIAL_SESSION_LIMIT,
            "快照必须严格截到上限"
        );
        assert!(
            body.contains("noscript-truncated"),
            "超限时必须写明列表已截断"
        );
    }

    #[test]
    fn content_length_and_reason_helpers() {
        assert_eq!(
            header_content_length(b"GET / HTTP/1.1\r\ncontent-length: 12\r\n"),
            12
        );
        assert_eq!(
            header_content_length(b"GET / HTTP/1.1\r\nx-other: 1\r\n"),
            0
        );
        assert_eq!(reason(200), "OK");
        assert_eq!(reason(101), "Switching Protocols");
        assert_eq!(reason(418), "");
        assert_eq!(WORKSPACE_DB_FILE, "whcode.duckdb");
        let reply = Reply::plain(whcode_web::api::HttpResponse::text(200, "x"));
        assert!(!reply.ws_upgrade);
    }

    /// 演示 actor 的能力语义：web-local 全授（单用户本机 harness，与 TUI 同一用户）。
    #[test]
    fn local_actor_has_read_capabilities() {
        let actor: Actor = actor_with_all_capabilities("web-local");
        assert!(!actor.id.is_empty());
        assert!(!actor.capabilities.is_empty(), "本机用户必须具备能力集合");
    }

    // ── 真实 socket 测试（Check 199/209 证据）───────────────────────────────

    /// 测试服务器：内存库 + 随机端口。服务器线程与测试线程经 `Connection::try_clone`
    /// 共享同一内存库实例（DuckDB 同实例多连接）；起线程属测试基建，生产路径零新增
    /// 线程（NFR-211）。
    struct TestServer {
        addr: SocketAddr,
        /// 测试侧连接：直接 INSERT 事件驱动广播。
        conn: Connection,
    }

    fn spawn_test_server() -> TestServer {
        let conn = migrated_memory_conn();
        let server_conn = conn.try_clone().expect("克隆内存库连接");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("绑定随机端口");
        let addr = listener.local_addr().expect("读取端口");
        let actor = actor_with_all_capabilities("test-web");
        let security = ServeSecurity::loopback();
        std::thread::Builder::new()
            .name("serve-loop-test".into())
            .spawn(move || {
                let mut server_conn = server_conn;
                let _ = serve_loop(listener, &mut server_conn, actor, security);
            })
            .expect("启动服务器线程");
        TestServer { addr, conn }
    }

    fn seed_event(conn: &Connection, session_id: &str, seq: i64, payload: &str) {
        conn.execute(
            "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                                 kind, payload_json, ts, persistence_level)
             VALUES (?, ?, ?, NULL, NULL, NULL, 'StreamingChunk', ?, current_timestamp, 'redacted')",
            params![seq, format!("E-{seq}"), session_id, payload],
        )
        .expect("插入事件");
    }

    /// 建立并完成 WS 握手（RFC 6455 §1.3 示例 key），返回升级后的 socket。
    fn ws_connect(addr: SocketAddr) -> TcpStream {
        let mut stream = TcpStream::connect(addr).expect("连接测试服务器");
        stream
            .set_read_timeout(Some(Duration::from_millis(100)))
            .expect("设置读超时");
        let key = "dGhlIHNhbXBsZSBub25jZQ==";
        let request =
            format!("GET /ws HTTP/1.1\r\nhost: {addr}\r\nsec-websocket-key: {key}\r\n\r\n");
        stream.write_all(request.as_bytes()).expect("写握手请求");
        let head = read_http_head(&mut stream);
        assert!(head.starts_with("HTTP/1.1 101"), "握手必须 101：{head}");
        assert!(
            head.contains(accept_key(key).as_str()),
            "Sec-WebSocket-Accept 必须匹配（RFC 6455 §1.3）"
        );
        stream
    }

    /// 读取 HTTP 响应头（至 `\r\n\r\n`；上限 5 s）。
    fn read_http_head(stream: &mut TcpStream) -> String {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut buf: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 512];
        while !buf.ends_with(b"\r\n\r\n") {
            if Instant::now() >= deadline {
                break;
            }
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(_) => continue, // 读超时（100 ms）→ 回到 deadline 判定
            }
        }
        String::from_utf8_lossy(&buf).into_owned()
    }

    /// 从持久缓冲读出一个服务端帧（不掩码）；缓冲跨调用保留残余字节——一次 read
    /// 可能带回多帧，逐帧消费不得丢弃。到 deadline 未齐或对端关闭返回 `None`。
    fn read_server_frame(
        buf: &mut Vec<u8>,
        stream: &mut TcpStream,
        deadline: Instant,
    ) -> Option<(u8, Vec<u8>)> {
        let mut chunk = [0u8; 4096];
        loop {
            if let Some((header_len, total, opcode)) = try_parse_server_frame(buf) {
                let payload = buf[header_len..total].to_vec();
                buf.drain(..total);
                return Some((opcode, payload));
            }
            if Instant::now() >= deadline {
                return None;
            }
            match stream.read(&mut chunk) {
                Ok(0) => return None,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(_) => continue, // 读超时（100 ms）→ 回到 deadline 判定
            }
        }
    }

    /// 尝试从缓冲解析一个完整服务端帧；返回（帧头长，帧总长，opcode）。未齐返回 `None`。
    fn try_parse_server_frame(buf: &[u8]) -> Option<(usize, usize, u8)> {
        if buf.len() < 2 {
            return None;
        }
        let opcode = buf[0] & 0x0F;
        let (header_len, payload_len) = match (buf[1] & 0x7F) as usize {
            126 => {
                if buf.len() < 4 {
                    return None;
                }
                (4, u16::from_be_bytes([buf[2], buf[3]]) as usize)
            }
            127 => {
                if buf.len() < 10 {
                    return None;
                }
                (
                    10,
                    u64::from_be_bytes([
                        buf[2], buf[3], buf[4], buf[5], buf[6], buf[7], buf[8], buf[9],
                    ]) as usize,
                )
            }
            len7 => (2, len7),
        };
        let total = header_len + payload_len;
        if buf.len() < total {
            return None;
        }
        Some((header_len, total, opcode))
    }

    /// 同步读 `count` 个文本帧；返回（收到数，最大帧间隔，首帧时延）。
    fn drain_frames(
        stream: &mut TcpStream,
        count: usize,
        budget: Duration,
    ) -> (usize, Duration, Duration) {
        let start = Instant::now();
        let deadline = start + budget;
        let mut buf: Vec<u8> = Vec::new();
        let mut received = 0usize;
        let mut max_interval = Duration::ZERO;
        let mut last_frame = start;
        let mut first_frame = Duration::ZERO;
        while received < count {
            let Some((_opcode, _payload)) = read_server_frame(&mut buf, stream, deadline) else {
                break;
            };
            let now = Instant::now();
            if received == 0 {
                first_frame = now - start;
            } else {
                max_interval = max_interval.max(now - last_frame);
            }
            last_frame = now;
            received += 1;
        }
        (received, max_interval, first_frame)
    }

    /// 并发读帧线程（测试基建）：读 `count` 个文本帧并记录到达时刻与 `seq`；
    /// 返回原 socket 以便复用。budget 内未收满则返回已收到的部分。
    fn spawn_frame_reader(
        stream: TcpStream,
        count: usize,
        budget: Duration,
    ) -> std::thread::JoinHandle<(TcpStream, Vec<(Instant, i64)>)> {
        std::thread::Builder::new()
            .name("ws-frame-reader".into())
            .spawn(move || {
                let mut stream = stream;
                let deadline = Instant::now() + budget;
                let mut buf: Vec<u8> = Vec::new();
                let mut received = Vec::new();
                while received.len() < count && Instant::now() < deadline {
                    let Some((opcode, payload)) =
                        read_server_frame(&mut buf, &mut stream, deadline)
                    else {
                        break;
                    };
                    if opcode != 0x1 {
                        continue; // 非文本帧（不应出现）：不计数
                    }
                    let event: serde_json::Value =
                        serde_json::from_slice(&payload).expect("帧载荷为 JSON");
                    received.push((Instant::now(), event["seq"].as_i64().expect("seq 字段")));
                }
                (stream, received)
            })
            .expect("启动读帧线程")
    }

    /// 探测连接已被服务器关闭：EOF 或非超时错误；`wait` 内仅超时（连接仍开放）
    /// 返回 `false`。
    fn connection_closed(stream: &mut TcpStream, wait: Duration) -> bool {
        let deadline = Instant::now() + wait;
        let mut probe = [0u8; 4096];
        loop {
            match stream.read(&mut probe) {
                Ok(0) => return true,
                Ok(_) => {}
                Err(err)
                    if err.kind() == ErrorKind::WouldBlock || err.kind() == ErrorKind::TimedOut =>
                {
                    if Instant::now() >= deadline {
                        return false;
                    }
                }
                Err(_) => return true,
            }
        }
    }

    /// Check 199/209 正向路径：握手 → 写 3 条事件 → 按 seq 升序收到 3 帧（游标不重复）。
    #[test]
    fn ws_client_receives_incremental_events() {
        let server = spawn_test_server();
        whcode_store::create_session_with_id(&server.conn, "S-1", "增量", "active", 1_000)
            .expect("创建会话");
        // 握手完成（服务循环已运行、游标已取）后再写事件：广播只发新事件。
        let mut client = ws_connect(server.addr);
        for seq in 1..=3 {
            seed_event(&server.conn, "S-1", seq, &format!(r#"{{"n":{seq}}}"#));
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut frame_buf: Vec<u8> = Vec::new();
        let mut seqs = Vec::new();
        for _ in 0..3 {
            let (_, payload) =
                read_server_frame(&mut frame_buf, &mut client, deadline).expect("5 s 内收到帧");
            let event: serde_json::Value = serde_json::from_slice(&payload).expect("帧载荷为 JSON");
            seqs.push(event["seq"].as_i64().expect("seq 字段"));
        }
        assert_eq!(seqs, vec![1, 2, 3], "按工作区 seq 升序，游标推进不重复");
    }

    /// Check 199：慢客户端存在时，正常客户端的广播延迟无劣化（对比测试）。
    ///
    /// B 握手后停止读；写入 200 条约 8 KiB 载荷的事件（约 1.6 MiB，超过 B 的内核
    /// 接收缓冲与 64 帧出站队列之和），断言：A 收满 200 帧、最大帧间隔 ≤ 1 s
    /// （广播周期 200 ms × 5 宽松倍数）、第 200 帧在写入完成后 ≤ 2 s 到达
    /// （64 帧/批 × 5 批/秒的节奏上限，见 serve 模块文档），且 B 被断开。
    /// 基线（无 B）与对比（有 B）的最大帧间隔均以 println! 留证。
    #[test]
    fn slow_client_does_not_delay_other_subscribers() {
        let server = spawn_test_server();
        whcode_store::create_session_with_id(&server.conn, "S-1", "慢客户端", "active", 1_000)
            .expect("创建会话");
        let mut fast = ws_connect(server.addr);

        // 基线：无慢客户端时 20 帧。
        for seq in 1..=20 {
            seed_event(&server.conn, "S-1", seq, r#"{"n":1}"#);
        }
        let (baseline_count, baseline_interval, _) =
            drain_frames(&mut fast, 20, Duration::from_secs(5));
        assert_eq!(baseline_count, 20, "基线 20 帧必须全部到达");
        println!("基线（无慢客户端）：最大帧间隔 {baseline_interval:?}");

        // 慢客户端 B：握手后停止读。
        let mut slow = ws_connect(server.addr);

        // A 由读帧线程持续读取（正常读），不会被内核缓冲或队列积压误判为慢；
        // 200 条 × 约 8 KiB ≈ 1.6 MiB > 内核缓冲 + 64 帧队列：B 必然溢出被断。
        let big = format!(r#"{{"pad":"{}"}}"#, "x".repeat(8192));
        let reader = spawn_frame_reader(fast, 200, Duration::from_secs(20));
        let insert_start = Instant::now();
        for seq in 21..=220 {
            seed_event(&server.conn, "S-1", seq, &big);
        }
        let insert_elapsed = insert_start.elapsed();
        let insert_done = Instant::now();
        let (_fast, frames) = reader.join().expect("读帧线程完成");
        let max_interval = frames
            .windows(2)
            .map(|pair| pair[1].0 - pair[0].0)
            .max()
            .unwrap_or(Duration::ZERO);
        let tail = frames.last().map_or(Duration::ZERO, |(ts, _)| {
            ts.saturating_duration_since(insert_done)
        });
        println!(
            "有慢客户端 B：写入 200 条耗时 {insert_elapsed:?}；A 最大帧间隔 {max_interval:?}；\
             最后一条事件写入后到第 200 帧到达 {tail:?}"
        );
        assert_eq!(frames.len(), 200, "慢客户端存在时 A 仍须收满 200 帧");
        // 时延上界：广播周期 200 ms × 5（宽松倍数，覆盖 tick 停靠与调度抖动）。
        assert!(
            max_interval <= Duration::from_millis(1000),
            "A 的最大帧间隔 {max_interval:?} 超过 1 s 上界（Check 199）"
        );
        // 尾时延：200 事件 ÷ 64 帧/批 = 4 批 × 200 ms = 800 ms 理论值，取 2.5 倍余量。
        assert!(
            tail <= Duration::from_millis(2000),
            "第 200 帧在写入完成后 {tail:?} 才到达，超过 2 s 上界（Check 199）"
        );
        assert!(
            max_interval <= baseline_interval * 10 + Duration::from_millis(1000),
            "有 B 的最大帧间隔 {max_interval:?} 相对基线 {baseline_interval:?} 出现数量级劣化"
        );

        // B 被断开：读到关闭帧，或连接关闭（读尽残余后 EOF / 重置）。
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut slow_buf: Vec<u8> = Vec::new();
        let mut saw_close = false;
        while let Some((opcode, _payload)) = read_server_frame(&mut slow_buf, &mut slow, deadline) {
            if opcode == 0x8 {
                saw_close = true;
                break;
            }
        }
        assert!(
            saw_close || connection_closed(&mut slow, Duration::from_secs(5)),
            "慢客户端 B 必须被断开（Check 199）"
        );
    }

    /// Check 209：超限入站帧被拒且不中断其他订阅者。
    ///
    /// C 发送声明 >1 MiB 载荷的掩码文本帧（帧头 + 掩码 + 512 KiB 真实字节分片）——
    /// 服务端按帧头立即拒绝（§10.2），不等载荷发满；随后 A 仍能收到后续广播帧。
    #[test]
    fn oversized_client_frame_is_rejected_without_dropping_subscribers() {
        let server = spawn_test_server();
        whcode_store::create_session_with_id(&server.conn, "S-1", "超限", "active", 1_000)
            .expect("创建会话");
        let mut subscriber = ws_connect(server.addr);
        let mut offender = ws_connect(server.addr);

        let declared = (whcode_web::security::MAX_MESSAGE_BYTES + 1) as u64;
        let mut frame = vec![0x81, 0x80 | 127];
        frame.extend_from_slice(&declared.to_be_bytes());
        frame.extend_from_slice(&[0x01, 0x02, 0x03, 0x04]); // 掩码
        offender.write_all(&frame).expect("写帧头与掩码");
        let chunk = vec![0xAAu8; 64 * 1024];
        for _ in 0..8 {
            if offender.write_all(&chunk).is_err() {
                break; // 服务端已拒绝并断开：剩余分片写失败属预期
            }
        }
        // 拒绝：关闭帧（1009）或连接关闭。
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut offender_buf: Vec<u8> = Vec::new();
        let mut saw_close = false;
        while let Some((opcode, payload)) =
            read_server_frame(&mut offender_buf, &mut offender, deadline)
        {
            if opcode == 0x8 {
                assert_eq!(payload, [0x03, 0xF1], "关闭码应为 1009（Message Too Big）");
                saw_close = true;
                break;
            }
        }
        assert!(
            saw_close || connection_closed(&mut offender, Duration::from_secs(5)),
            "超限发送者必须被拒绝（关闭帧或连接关闭，Check 209）"
        );

        // 其他订阅者不受影响：后续广播照常到达。
        for seq in 1..=2 {
            seed_event(&server.conn, "S-1", seq, &format!(r#"{{"n":{seq}}}"#));
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut subscriber_buf: Vec<u8> = Vec::new();
        let (_, payload) = read_server_frame(&mut subscriber_buf, &mut subscriber, deadline)
            .expect("订阅者仍收到帧");
        let event: serde_json::Value = serde_json::from_slice(&payload).expect("JSON");
        assert_eq!(
            event["seq"].as_i64(),
            Some(1),
            "订阅者未被超限帧中断（Check 209）"
        );
    }
}
