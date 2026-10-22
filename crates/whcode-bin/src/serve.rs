//! `whcode serve --web`：Web 服务进程入口（T090—T095 的装配根接线）。
//!
//! 契约（`contracts/cui-cli.md` §1）：`serve --web [--bind 127.0.0.1:PORT]`。
//!
//! 本模块只做**装配**：打开/初始化工作区库 → 绑定监听（V-110 门禁在
//! `whcode-web::bind`：默认仅回环，对外监听被拒）→ **顺序**接受连接——DuckDB
//! `Connection` 为 `Send` 非 `Sync`（ADR-002 的证据），请求处理不并发共享连接；
//! 请求体有界（`MAX_MESSAGE_BYTES`）。
//!
//! WebSocket：RFC 6455 握手 + 立即正常关闭（`1000`）——会话级事件推送**未实现**
//! （M6 交付为握手契约；前端当前经 `/api/*` 轮询，不依赖 WS，T094）。
//!
//! 认证/Origin 语义见 `whcode-web::security` 与 ADR-022：回环模式认证关闭、缺失
//! `Origin` 放行（同源 GET 与本机 CLI 客户端）；`serve` 无 token 参数——对外监听
//! 由 bind 层直接拒绝（FR-606），token 配置属未来任务。

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use duckdb::Connection;
use whcode_app::actor_with_all_capabilities;
use whcode_domain::{ErrorCode, WhError};
use whcode_web::api::{self, HttpResponse, Method};
use whcode_web::assets;
use whcode_web::bind::{BindConfig, bind};
use whcode_web::security;

use crate::config::Config;
use crate::load_config;

/// 工作区数据库文件名（工作区根下的单文件 DuckDB，ADR-001）。
pub const WORKSPACE_DB_FILE: &str = "whcode.duckdb";

/// `serve` 命令参数。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ServeCli {
    /// `--web`：启动 Web 服务。
    pub web: bool,
    /// `--ssh`：SSH 服务（M7 延后，未实现）。
    pub ssh: bool,
    /// `--bind host:port`（默认 `whcode_web::bind::DEFAULT_BIND`）。
    pub bind: Option<String>,
    /// `--workspace DIR`（默认配置的工作区路径）。
    pub workspace: Option<PathBuf>,
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
                "--workspace" => {
                    cli.workspace = Some(PathBuf::from(next_value(&mut iter, "--workspace")?));
                }
                // `--config PATH` 由 `load_config` 统一处理；此处跳过其值。
                "--config" => {
                    let _ = next_value(&mut iter, "--config")?;
                }
                other => {
                    return Err(WhError::parse_error(format!(
                        "未知参数 {other:?}：可用 --web / --ssh / --bind host:port / \
                         --workspace DIR / --config PATH"
                    )));
                }
            }
        }
        if cli.web && cli.ssh {
            return Err(WhError::invalid_state("--web 与 --ssh 只能选其一"));
        }
        if !cli.web && !cli.ssh {
            return Err(WhError::parse_error(
                "serve 缺少模式参数：可用 --web（SSH 属 M7 延后）",
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
    if cli.ssh {
        eprintln!(
            "serve --ssh 未实现：SSH 服务端属 M7（ADR-013 延后）；当前可用 whcode serve --web"
        );
        return ExitCode::FAILURE;
    }
    let config = match load_config(args) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("配置加载失败：{err}");
            return ExitCode::FAILURE;
        }
    };
    let workspace = cli
        .workspace
        .unwrap_or_else(|| config.workspace_path.clone());
    match serve_web(&workspace, &config, cli.bind.as_deref()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("Web 服务失败：{err}");
            ExitCode::FAILURE
        }
    }
}

/// 启动 Web 服务：工作区库 → 回环监听 → 顺序服务循环。
fn serve_web(workspace: &Path, config: &Config, bind_addr: Option<&str>) -> Result<(), WhError> {
    let db_path = workspace.join(WORKSPACE_DB_FILE);
    // 临时目录属**有效工作区**（`<workspace>/tmp`，platform::paths 约定）：`--workspace`
    // 与配置工作区不一致时，不得在配置工作区/cwd 下创建目录（实测缺陷：曾污染仓库根
    // `tmp/`，触发布局门禁失败）。
    let temp_directory = whcode_platform::paths::workspace_temp_dir(workspace);
    fs::create_dir_all(&temp_directory)
        .map_err(|err| WhError::new(ErrorCode::IoError, format!("创建工作区目录失败：{err}")))?;
    let conn = open_workspace_db(&db_path, config, &temp_directory)?;
    let actor = actor_with_all_capabilities("web-local");

    let bind_config = BindConfig {
        addr: bind_addr
            .unwrap_or(whcode_web::bind::DEFAULT_BIND)
            .to_string(),
        auth_enabled: false,
        origin_check_enabled: false,
    };
    let listener = bind(&bind_config)?;
    let addr = listener
        .local_addr()
        .map_err(|err| WhError::new(ErrorCode::IoError, format!("读取监听地址失败：{err}")))?;
    println!(
        "WHCode Web 服务：http://{addr}/（仅回环，Ctrl-C 退出；FR-606：对外监听被 bind 门禁拒绝）"
    );
    println!("工作区数据库：{}", db_path.display());
    serve_loop(listener, &conn, &actor)
}

/// 打开并初始化工作区数据库（幂等迁移；数据库参数取自配置基线，临时目录属工作区）。
fn open_workspace_db(
    db_path: &Path,
    config: &Config,
    temp_directory: &Path,
) -> Result<Connection, WhError> {
    let conn = Connection::open(db_path).map_err(|err| {
        WhError::new(
            ErrorCode::IoError,
            format!("打开数据库失败（{}）：{err}", db_path.display()),
        )
    })?;
    // DuckDB 接受正斜杠路径；单引号按 SQL 字面量规则转义（工作区路径含 `'` 的极端情形）。
    let temp_literal = temp_directory
        .display()
        .to_string()
        .replace('\\', "/")
        .replace('\'', "''");
    conn.execute_batch(&format!(
        "SET memory_limit = '{}MB'; SET threads = {}; SET temp_directory = '{}';",
        config.db.memory_limit_mb, config.db.threads, temp_literal
    ))
    .map_err(|err| {
        WhError::new(
            ErrorCode::InvalidState,
            format!("配置数据库参数失败：{err}"),
        )
    })?;
    whcode_store::apply_migrations(&conn)?;
    Ok(conn)
}

/// 一次响应：HTTP 响应 + WebSocket 握手后的关闭帧标记。
struct Reply {
    response: HttpResponse,
    /// 101 握手后发送 RFC 6455 正常关闭帧（事件推送未实现的诚实行为）。
    ws_close_frame: bool,
}

impl Reply {
    fn plain(response: HttpResponse) -> Self {
        Self {
            response,
            ws_close_frame: false,
        }
    }
}

/// 服务循环：顺序接受连接（单线程，不共享连接给并发请求）。
fn serve_loop(
    listener: TcpListener,
    conn: &Connection,
    actor: &whcode_app::Actor,
) -> Result<(), WhError> {
    for incoming in listener.incoming() {
        let mut stream = match incoming {
            Ok(stream) => stream,
            Err(err) => {
                eprintln!("接受连接失败：{err}");
                continue;
            }
        };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let reply = match read_request(&mut stream) {
            Ok(raw) => respond(conn, &raw, actor),
            Err(_) => Reply::plain(HttpResponse::text(400, "bad request")),
        };
        if write_response(&mut stream, &reply).is_err() {
            eprintln!("写响应失败");
        }
    }
    Ok(())
}

/// 读取一个完整请求（头 + 按 `Content-Length` 的体；有界，超限报错）。
fn read_request(stream: &mut TcpStream) -> Result<Vec<u8>, WhError> {
    const HEADER_CAP: usize = 16 * 1024;
    let mut buffer = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    let mut header_end: Option<usize> = None;
    let mut content_length = 0usize;
    loop {
        if let Some(end) = header_end
            && buffer.len() >= end + 4 + content_length
        {
            return Ok(buffer);
        }
        if buffer.len() > security::MAX_MESSAGE_BYTES + HEADER_CAP {
            return Err(WhError::invalid_state("请求超过上限"));
        }
        let read = stream
            .read(&mut chunk)
            .map_err(|err| WhError::new(ErrorCode::IoError, format!("读取请求失败：{err}")))?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if header_end.is_none()
            && let Some(position) = find_subsequence(&buffer, b"\r\n\r\n")
        {
            header_end = Some(position);
            content_length = header_content_length(&buffer[..position]);
        }
    }
    if header_end.is_some() {
        Ok(buffer)
    } else {
        Err(WhError::invalid_state("请求头不完整"))
    }
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
fn respond(conn: &Connection, raw: &[u8], actor: &whcode_app::Actor) -> Reply {
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
        if let Some((content_type, body)) = assets::static_asset(path) {
            return Reply::plain(HttpResponse {
                status: 200,
                headers: vec![("content-type".to_string(), content_type.to_string())],
                body: body.to_vec(),
            });
        }
    }
    let is_ws = request.path == "/ws";
    let response = api::handle_request(conn, false, None, &request, actor);
    Reply {
        ws_close_frame: is_ws && response.status == 101,
        response,
    }
}

/// 写响应：状态行 + 头 + 体；101 不带 Content-Length（RFC 7230），随后发关闭帧。
fn write_response(stream: &mut TcpStream, reply: &Reply) -> Result<(), WhError> {
    let response = &reply.response;
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
    }
    head.push_str("connection: close\r\n\r\n");
    stream
        .write_all(head.as_bytes())
        .and_then(|()| stream.write_all(&response.body))
        .map_err(|err| WhError::new(ErrorCode::IoError, format!("写响应失败：{err}")))?;
    if reply.ws_close_frame {
        // RFC 6455 关闭帧：opcode 0x8，状态码 1000（正常关闭）。
        let close_frame: [u8; 4] = [0x88, 0x02, 0x03, 0xE8];
        stream
            .write_all(&close_frame)
            .map_err(|err| WhError::new(ErrorCode::IoError, format!("写关闭帧失败：{err}")))?;
    }
    stream
        .flush()
        .map_err(|err| WhError::new(ErrorCode::IoError, format!("刷新响应失败：{err}")))
}

/// 状态码短语（响应可读性；未知码返回空）。
fn reason(status: u16) -> &'static str {
    match status {
        101 => "Switching Protocols",
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use duckdb::Connection;

    use super::{Reply, ServeCli, WORKSPACE_DB_FILE, header_content_length, reason, respond};
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
    }

    /// 装配路由：静态资产、API（ADR-022 Origin 语义）、404 与 WS 握手。
    #[test]
    fn assembled_routes_serve_static_api_and_ws_handshake() {
        let conn = migrated_memory_conn();
        let actor = actor_with_all_capabilities("tester");

        // 静态资产（GET）。
        let reply = respond(
            &conn,
            &raw_request("GET / HTTP/1.1\r\nhost: 127.0.0.1\r\n\r\n"),
            &actor,
        );
        assert_eq!(reply.response.status, 200);
        assert_eq!(
            reply.response.header("content-type"),
            Some("text/html; charset=utf-8")
        );
        assert!(String::from_utf8_lossy(&reply.response.body).contains("WHCode"));
        let reply = respond(&conn, &raw_request("GET /app.js HTTP/1.1\r\n\r\n"), &actor);
        assert!(
            reply
                .response
                .header("content-type")
                .is_some_and(|value| value.starts_with("text/javascript"))
        );
        // 带查询串的首页（`/?session=`）必须命中静态路由（回归：曾落 404）。
        let reply = respond(
            &conn,
            &raw_request("GET /?session=S-DEMO-1 HTTP/1.1\r\n\r\n"),
            &actor,
        );
        assert_eq!(reply.response.status, 200);
        assert!(String::from_utf8_lossy(&reply.response.body).contains("WHCode"));

        // POST 不属于静态路由 → 404。
        let reply = respond(&conn, &raw_request("POST / HTTP/1.1\r\n\r\n"), &actor);
        assert_eq!(reply.response.status, 404);

        // API：回环模式缺 Origin 放行（ADR-022）；数据为空数组（正确的空）。
        let reply = respond(
            &conn,
            &raw_request("GET /api/sessions HTTP/1.1\r\nhost: 127.0.0.1\r\n\r\n"),
            &actor,
        );
        assert_eq!(reply.response.status, 200);
        let body: serde_json::Value = serde_json::from_slice(&reply.response.body).expect("JSON");
        assert_eq!(body["schema_version"], SCHEMA_VERSION);
        assert_eq!(body["sessions"], serde_json::json!([]));

        // API：非回环 Origin 拒绝（威胁模型不变）。
        let reply = respond(
            &conn,
            &raw_request("GET /api/sessions HTTP/1.1\r\norigin: http://evil.example\r\n\r\n"),
            &actor,
        );
        assert_eq!(reply.response.status, 403);

        // WS：缺 Sec-WebSocket-Key → 400；有 key → 101 + 关闭帧。
        let reply = respond(&conn, &raw_request("GET /ws HTTP/1.1\r\n\r\n"), &actor);
        assert_eq!(reply.response.status, 400);
        let reply = respond(
            &conn,
            &raw_request("GET /ws HTTP/1.1\r\nsec-websocket-key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"),
            &actor,
        );
        assert_eq!(reply.response.status, 101);
        assert!(reply.ws_close_frame, "握手后必须发送正常关闭帧");

        // 未知 API 路径 → 404。
        let reply = respond(
            &conn,
            &raw_request("GET /api/nope HTTP/1.1\r\n\r\n"),
            &actor,
        );
        assert_eq!(reply.response.status, 404);
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
        assert!(!reply.ws_close_frame);
    }

    /// 演示 actor 的能力语义：web-local 全授（单用户本机 harness，与 TUI 同一用户）。
    #[test]
    fn local_actor_has_read_capabilities() {
        let actor: Actor = actor_with_all_capabilities("web-local");
        assert!(!actor.id.is_empty());
        assert!(!actor.capabilities.is_empty(), "本机用户必须具备能力集合");
    }
}
