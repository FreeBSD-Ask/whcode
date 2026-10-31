//! # MCP stdio 会话（FR-736、ADR-054 §2.4—2.6）
//!
//! 一个 [`McpSession`] 对应一个 MCP 服务器子进程：spawn（环境净化 + 进程组 +
//! stderr 丢弃）→ initialize 握手 → 请求/响应配对（newline 分帧、有界行读取）→
//! shutdown（关 stdin → 宽限等待 → `terminate_tree` → reap）。
//!
//! 并发/线程模型（NFR-216、ADR-054 §2.5）：会话本身零常驻线程；每个在途请求由
//! [`Watchdog`] 创建一条瞬态看门狗线程——到点（超时或取消）`terminate_tree` 终止
//! 服务器进程树，使阻塞中的 `read` 以 EOF 解锁。看门狗为分离线程，退出由 `done`
//! 标志驱动（至多存活到 deadline + 一个轮询周期），不 join。
//!
//! 超时窗口覆盖**写与读全程**：看门狗在写请求行之前武装（大参数写入同样受保护）。

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use whcode_domain::{ErrorCode, WhError};

use crate::config::McpServerConfig;
use crate::manager::{MAX_TOOLS_PER_SERVER, McpToolSet};
use crate::protocol::{
    self, CLIENT_NAME, Incoming, InitializeResult, METHOD_NOT_FOUND, PROTOCOL_VERSION,
    ToolCallResult,
};

/// 单行入站消息上限（字节；超出即会话失效并终止进程，防无界累积）。
pub const MAX_LINE_BYTES: usize = 1024 * 1024;

/// 单次请求容忍的外来消息上限（未配对响应/通知/服务器请求；超出即会话失效，
/// 防服务器刷屏造成实际上的无界读取）。
const MAX_FOREIGN_MESSAGES: u32 = 64;

/// shutdown 对优雅退出的宽限时长。
const SHUTDOWN_GRACE: Duration = Duration::from_millis(500);

/// 看门狗轮询间隔（同时是取消请求的响应上限）。
const WATCHDOG_POLL: Duration = Duration::from_millis(50);

/// 取消标志（`Arc<AtomicBool>`；whcode-app 由 `whcode_llm::CancelToken::shared_flag`
/// 桥接构造，跨线程共享给看门狗）。
#[derive(Clone, Default)]
pub struct CancelFlag {
    flag: Arc<AtomicBool>,
}

impl CancelFlag {
    /// 从共享原子标志构造。
    #[must_use]
    pub fn from_shared(flag: Arc<AtomicBool>) -> Self {
        Self { flag }
    }

    /// 是否已请求取消。
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
}

/// 瞬态看门狗：轮询（deadline / 取消 / done），到点终止进程树解锁阻塞读。
struct Watchdog {
    done: Arc<AtomicBool>,
}

impl Watchdog {
    fn arm(pid: u32, deadline: Instant, cancel: Option<CancelFlag>) -> Self {
        let done = Arc::new(AtomicBool::new(false));
        let done_thread = Arc::clone(&done);
        std::thread::spawn(move || {
            loop {
                if done_thread.load(Ordering::SeqCst) {
                    return;
                }
                if Instant::now() >= deadline
                    || cancel.as_ref().is_some_and(CancelFlag::is_cancelled)
                {
                    // 终止服务器进程树（平台层封装；taskkill /T /F 或进程组信号），
                    // 使调用线程阻塞中的 stdout 读以 EOF 解锁。
                    let _ = whcode_platform::process::terminate_tree(pid);
                    return;
                }
                std::thread::sleep(WATCHDOG_POLL);
            }
        });
        Self { done }
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        self.done.store(true, Ordering::SeqCst);
    }
}

/// MCP 服务器子进程会话（per-turn 生命周期；`Drop` 负责全部回收路径）。
pub struct McpSession {
    server_name: String,
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Option<BufReader<ChildStdout>>,
    next_id: u64,
    initialized: bool,
    broken: bool,
    /// 入站通知计数（saturating；不累积消息本体）。
    notifications_seen: u64,
}

impl McpSession {
    /// 启动服务器子进程（不做 initialize——握手由调用方显式执行，便于测试分层）。
    ///
    /// 环境净化（ADR-054 §2.9）：`apply_minimal_env` 先收敛父环境为最小白名单，
    /// 显式 `env` 随后回填（用户为服务器声明的授权通道）；**不再过
    /// `strip_credentials`**——显式配置即用户意图，父环境继承已被 env_clear 断绝。
    ///
    /// # Errors
    ///
    /// 子进程启动失败时返回 [`ErrorCode::IoError`]。
    pub fn spawn(config: &McpServerConfig) -> Result<Self, WhError> {
        let mut command = Command::new(&config.command);
        command.args(&config.args);
        for (key, value) in &config.env {
            command.env(key, value);
        }
        let _ = whcode_runtime::worker::apply_minimal_env(&mut command);
        // 进程组（FR-409 口径）：Unix 自成进程组供按组终止；Windows 空操作。
        whcode_platform::process::configure_child_process_group(&mut command);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // stderr 丢弃（ADR-054 §2.6）：防日志管道填满阻塞服务器；诊断靠退出码。
            .stderr(Stdio::null());
        let mut child = command.spawn().map_err(|err| {
            WhError::new(ErrorCode::IoError, format!("MCP 服务器启动失败：{err}"))
        })?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().map(BufReader::new);
        Ok(Self {
            server_name: config.name.clone(),
            child,
            stdin,
            stdout,
            next_id: 0,
            initialized: false,
            broken: false,
            notifications_seen: 0,
        })
    }

    /// 服务器名（配置声明）。
    #[must_use]
    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    /// 会话是否已失效（IO 失败、协议越界或已 shutdown）。
    #[must_use]
    pub fn is_broken(&self) -> bool {
        self.broken
    }

    /// 入站通知计数（诊断用）。
    #[must_use]
    pub fn notifications_seen(&self) -> u64 {
        self.notifications_seen
    }

    /// 服务器子进程 PID（零残留断言与诊断用；会话关闭后无意义）。
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// initialize 握手：请求 → 版本协商（未知版本中止）→ `notifications/initialized`。
    ///
    /// # Errors
    ///
    /// 请求失败、结果非法或服务器版本不在受支持集合时返回 [`WhError`]。
    pub fn initialize(
        &mut self,
        timeout: Duration,
        cancel: Option<CancelFlag>,
    ) -> Result<InitializeResult, WhError> {
        let params = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": CLIENT_NAME, "version": env!("CARGO_PKG_VERSION") },
        });
        let result = self.request("initialize", &params, timeout, cancel)?;
        let info = protocol::parse_initialize_result(&result).map_err(|err| self.poison(err))?;
        let notification = protocol::encode_notification("notifications/initialized", &json!({}));
        self.write_line(&notification)?;
        self.initialized = true;
        Ok(info)
    }

    /// tools/list（跟随 cursor 分页，累计 [`MAX_TOOLS_PER_SERVER`] 条上限；
    /// 超限 `truncated=true` 显式标记）。
    ///
    /// # Errors
    ///
    /// 未 initialize、请求失败或结果非法时返回 [`WhError`]。
    pub fn list_tools(
        &mut self,
        timeout: Duration,
        cancel: Option<CancelFlag>,
    ) -> Result<McpToolSet, WhError> {
        self.require_initialized()?;
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut params = serde_json::Map::new();
            if let Some(current) = &cursor {
                params.insert("cursor".to_string(), Value::from(current.clone()));
            }
            let result = self.request(
                "tools/list",
                &Value::Object(params),
                timeout,
                cancel.clone(),
            )?;
            let listing = protocol::parse_tools_list(&result).map_err(|err| self.poison(err))?;
            let remaining = MAX_TOOLS_PER_SERVER - tools.len();
            if listing.tools.len() > remaining {
                tools.extend_from_slice(&listing.tools[..remaining]);
                return Ok(McpToolSet {
                    tools,
                    truncated: true,
                });
            }
            tools.extend(listing.tools);
            match listing.next_cursor {
                Some(next) => cursor = Some(next),
                None => {
                    return Ok(McpToolSet {
                        tools,
                        truncated: false,
                    });
                }
            }
        }
    }

    /// tools/call：单次工具调用（超时看门狗 + 取消感知；输出按 max_output 截断）。
    ///
    /// # Errors
    ///
    /// 未 initialize、会话失效、请求失败或超时/取消时返回 [`WhError`]；
    /// 服务器声明 `isError` 的执行失败**不是** `Err`——以
    /// [`ToolCallResult::is_error`] 区分（回填语义由调用方决定）。
    pub fn call_tool(
        &mut self,
        tool: &str,
        arguments: &Value,
        timeout: Duration,
        cancel: Option<CancelFlag>,
        max_output_bytes: usize,
    ) -> Result<ToolCallResult, WhError> {
        self.require_initialized()?;
        let params = json!({ "name": tool, "arguments": arguments });
        let result = self.request("tools/call", &params, timeout, cancel)?;
        let mut call = protocol::parse_tools_call(&result).map_err(|err| self.poison(err))?;
        if call.text.len() > max_output_bytes {
            let mut end = max_output_bytes;
            while end > 0 && !call.text.is_char_boundary(end) {
                end -= 1;
            }
            call.text.truncate(end);
            call.truncated = true;
        }
        Ok(call)
    }

    /// 会话级请求：写请求行 → 逐行读入站消息直到配对响应（外来消息计数丢弃）。
    ///
    /// # Errors
    ///
    /// 会话失效、写失败、读失败、超时、取消、行越界、外来消息越限或服务器返回
    /// JSON-RPC 错误时返回 [`WhError`]。
    fn request(
        &mut self,
        method: &str,
        params: &Value,
        timeout: Duration,
        cancel: Option<CancelFlag>,
    ) -> Result<Value, WhError> {
        if self.broken {
            return Err(WhError::new(
                ErrorCode::InvalidState,
                format!("MCP 服务器 {} 会话已失效", self.server_name),
            ));
        }
        let id = self.next_id;
        self.next_id += 1;
        // 看门狗在写之前武装：大参数写阻塞与读阻塞同受超时/取消保护。
        let deadline = Instant::now() + timeout;
        let _watchdog = Watchdog::arm(self.child.id(), deadline, cancel.clone());
        self.write_line(&protocol::encode_request(id, method, params))?;

        // 读响应期间把 stdout 从 self 取出（借用分离）：读循环内需要 &mut self
        // 做写入/失效/计数，与逐行读取互不重叠；结束后归还。
        let mut stdout = self.stdout.take();
        let outcome = self.await_response(id, stdout.as_mut(), timeout, cancel);
        self.stdout = stdout;
        outcome
    }

    /// 逐行读入站消息直到配对响应（`reader` 已从会话取出，见 [`Self::request`]）。
    fn await_response(
        &mut self,
        id: u64,
        reader: Option<&mut BufReader<ChildStdout>>,
        timeout: Duration,
        cancel: Option<CancelFlag>,
    ) -> Result<Value, WhError> {
        let Some(reader) = reader else {
            return Err(self.poison(WhError::new(
                ErrorCode::IoError,
                "stdout 不可读（会话已关闭）",
            )));
        };
        let mut foreign = 0u32;
        loop {
            let line = match read_line_bounded(reader, MAX_LINE_BYTES) {
                Ok(Some(line)) => line,
                Ok(None) => return Err(self.classify_eof(timeout, cancel)),
                Err(err) => {
                    return Err(self.poison(WhError::new(ErrorCode::IoError, err.to_string())));
                }
            };
            let incoming = protocol::parse_line(&line).map_err(|err| self.poison(err))?;
            match incoming {
                Incoming::Response { id: other, .. } if other != id => foreign += 1,
                Incoming::Response { result, error, .. } => {
                    return match (result, error) {
                        (Some(result), _) => Ok(result),
                        (None, Some(err)) => Err(WhError::new(
                            ErrorCode::RuntimeUnavailable,
                            format!(
                                "MCP 服务器 {} 返回错误 {}：{}",
                                self.server_name, err.code, err.message
                            ),
                        )),
                        (None, None) => Err(self.poison(WhError::new(
                            ErrorCode::ParseError,
                            "响应缺 result 与 error",
                        ))),
                    };
                }
                Incoming::Notification { .. } => {
                    self.notifications_seen = self.notifications_seen.saturating_add(1);
                }
                Incoming::ServerRequest { id: other, .. } => {
                    // 未声明任何服务器→客户端能力：统一拒绝（规范上不应出现）。
                    let rejection = protocol::encode_error_response(
                        other,
                        METHOD_NOT_FOUND,
                        "whcode-mcp 未声明该方法能力",
                    );
                    self.write_line(&rejection)?;
                    foreign += 1;
                }
            }
            if foreign > MAX_FOREIGN_MESSAGES {
                return Err(self.poison(WhError::new(
                    ErrorCode::InvalidState,
                    format!(
                        "MCP 服务器 {} 外来消息超限（{MAX_FOREIGN_MESSAGES}）",
                        self.server_name
                    ),
                )));
            }
        }
    }

    /// EOF 分类：看门狗杀树后读端关闭——按取消/超时/意外退出归因（不混为一谈）。
    ///
    /// 杀树与退出状态登记存在竞态：`try_wait` 短暂轮询确认（≤ 500ms），
    /// 仍存活才归因为"stdout 提前关闭"。
    fn classify_eof(&mut self, timeout: Duration, cancel: Option<CancelFlag>) -> WhError {
        self.broken = true;
        let mut exited = self.child.try_wait().ok().flatten().is_some();
        for _ in 0..50 {
            if exited {
                break;
            }
            match self.child.try_wait() {
                Ok(Some(_)) => {
                    exited = true;
                    break;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(_) => break,
            }
        }
        if cancel.is_some_and(|flag| flag.is_cancelled()) {
            WhError::new(
                ErrorCode::RuntimeUnavailable,
                format!("MCP 服务器 {} 因取消请求被终止", self.server_name),
            )
        } else if exited {
            WhError::new(
                ErrorCode::Timeout,
                format!(
                    "MCP 服务器 {} 在 {}ms 内未响应，已被终止",
                    self.server_name,
                    timeout.as_millis()
                ),
            )
        } else {
            WhError::new(
                ErrorCode::IoError,
                format!("MCP 服务器 {} 的 stdout 提前关闭", self.server_name),
            )
        }
    }

    /// 失败路径统一：会话失效并返回原错误。
    fn poison(&mut self, err: WhError) -> WhError {
        self.broken = true;
        err
    }

    fn require_initialized(&self) -> Result<(), WhError> {
        if self.broken {
            return Err(WhError::new(
                ErrorCode::InvalidState,
                format!("MCP 服务器 {} 会话已失效", self.server_name),
            ));
        }
        if !self.initialized {
            return Err(WhError::new(
                ErrorCode::InvalidState,
                format!("MCP 服务器 {} 未完成 initialize 握手", self.server_name),
            ));
        }
        Ok(())
    }

    fn write_line(&mut self, line: &str) -> Result<(), WhError> {
        let Some(stdin) = self.stdin.as_mut() else {
            return Err(self.poison(WhError::new(
                ErrorCode::IoError,
                "stdin 不可写（会话已关闭）",
            )));
        };
        let result = writeln!(stdin, "{line}").and_then(|()| stdin.flush());
        if let Err(err) = result {
            return Err(self.poison(WhError::new(
                ErrorCode::IoError,
                format!("写入 MCP 服务器失败：{err}"),
            )));
        }
        Ok(())
    }

    /// 关闭会话：stdin EOF → 宽限等待 → `terminate_tree` → reap（幂等）。
    pub fn shutdown(&mut self) {
        self.broken = true;
        // 先关读写端再等/杀：避免杀树后残留管道句柄。
        self.stdout = None;
        self.stdin = None;
        for _ in 0..(SHUTDOWN_GRACE.as_millis() / 10) {
            match self.child.try_wait() {
                Ok(Some(_)) | Err(_) => return,
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        let pid = self.child.id();
        if pid != 0 {
            let _ = whcode_platform::process::terminate_tree(pid);
        }
        let _ = self.child.wait();
    }
}

impl Drop for McpSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// 有界行读取：`fill_buf`/`consume` 分块扫描，超上限立即拒绝（不做无界累积）。
///
/// 返回 `Ok(None)` 表示流已结束且无残留字节（EOF）。行内容必须是 UTF-8
/// （规范要求 JSON-RPC MUST UTF-8）。
fn read_line_bounded<R: BufRead>(reader: &mut R, cap: usize) -> std::io::Result<Option<String>> {
    let mut buf: Vec<u8> = Vec::with_capacity(1024.min(cap));
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if buf.is_empty() {
                return Ok(None);
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "消息行未以换行结束即遇 EOF",
            ));
        }
        match available.iter().position(|&b| b == b'\n') {
            Some(pos) => {
                buf.extend_from_slice(&available[..pos]);
                reader.consume(pos + 1);
                if buf.len() > cap {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("消息行超过 {cap} 字节上限"),
                    ));
                }
                return String::from_utf8(buf)
                    .map(Some)
                    .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err));
            }
            None => {
                let len = available.len();
                buf.extend_from_slice(available);
                reader.consume(len);
                if buf.len() > cap {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("消息行超过 {cap} 字节上限"),
                    ));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_line_bounded_enforces_cap_and_utf8() {
        let mut data: &[u8] = b"hello\n";
        let line = read_line_bounded(&mut data, 16)
            .expect("正常行")
            .expect("非 EOF");
        assert_eq!(line, "hello");

        let mut data: &[u8] = b"abcdef\n";
        let err = read_line_bounded(&mut data, 3).expect_err("超上限拒绝");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);

        let mut data: &[u8] = b"";
        assert!(read_line_bounded(&mut data, 16).expect("EOF").is_none());

        let mut data: &[u8] = b"abc";
        let err = read_line_bounded(&mut data, 16).expect_err("无换行 EOF");
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);

        // 多字节 UTF-8 行不因分块边界被截断（分块经 fill_buf 天然安全）。
        let mut data: &[u8] = "中文行\n".as_bytes();
        let line = read_line_bounded(&mut data, 64)
            .expect("中文行")
            .expect("非 EOF");
        assert_eq!(line, "中文行");
    }

    #[test]
    fn cancel_flag_defaults_to_not_cancelled() {
        let flag = CancelFlag::default();
        assert!(!flag.is_cancelled());
        let shared = Arc::new(AtomicBool::new(true));
        assert!(CancelFlag::from_shared(shared).is_cancelled());
    }
}
