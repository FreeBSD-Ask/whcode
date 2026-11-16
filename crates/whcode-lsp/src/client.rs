//! # LSP 四查询客户端（ADR-078）
//!
//! 封闭集：`textDocument/definition`、`textDocument/references`、`textDocument/hover`、
//! 诊断（拉取 `textDocument/diagnostic`，回退推送 `textDocument/publishDiagnostics`），
//! 以及查询前必需的 `textDocument/didOpen`。
//!
//! ## 生命周期与线程
//!
//! 客户端按“查询期”存在（构造 → initialize → 查询 → Drop），**不是会话常驻资源**。读取由
//! 单个 reader 线程完成：响应走有界阻塞通道（容量 [`RESPONSE_CAPACITY`]），通知走有界
//! 非阻塞通道（容量 [`NOTIFICATION_CAPACITY`]，满则丢弃——通知可丢，响应不可丢）。Drop 时
//! 先发 `shutdown`／`exit`，再 kill 子进程，最后 join reader（ADO-071 的“全线程 join、
//! 无常驻泄漏”口径）。
//!
//! ## 有界性
//!
//! 结果数量与文本长度全部有上限，超限**截断并置 `truncated = true`**（不静默丢弃后冒充完整）。
//! 单文件 didOpen 上限 [`MAX_OPEN_BYTES`]；缓存文档数上限 [`MAX_CACHED_DOCUMENTS`]。
//!
//! ## 正确的空 vs 错误的空
//!
//! 查询成功但没有结果（`[]`／`null`）＝正确的空，返回空 `items`；服务器不支持该方法、
//! 进程退出、超时、帧损坏＝显式错误。诊断特别处理：拉取不支持且推送缓存也无该文件时
//! 返回 `not_supported`，**不返回空列表冒充“没有诊断”**。

use std::collections::HashMap;
use std::collections::HashSet;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, sync_channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use whcode_domain::{ErrorCode, WhError};

use crate::config::{self, ServerConfig};
use crate::transport::{self, encode_frame, parse_frame};

/// 响应通道容量（同时在途的请求数远小于此）。
const RESPONSE_CAPACITY: usize = 8;

/// 通知通道容量（满则丢弃通知，不阻塞响应）。
const NOTIFICATION_CAPACITY: usize = 64;

/// 单次查询返回的位置条数上限。
pub const MAX_LOCATIONS: usize = 100;

/// hover 文本字符上限。
pub const MAX_HOVER_CHARS: usize = 2000;

/// 单次查询返回的诊断条数上限。
pub const MAX_DIAGNOSTICS: usize = 100;

/// 单条诊断消息字符上限（超出截断并置 `truncated`）。
pub const MAX_DIAGNOSTIC_MESSAGE_CHARS: usize = 500;

/// 单个 URI 字符上限（超长的条目丢弃并置 `truncated`）。
pub const MAX_URI_CHARS: usize = 1024;

/// 推送诊断缓存的文件数上限。
pub const MAX_CACHED_DOCUMENTS: usize = 64;

/// 单客户端已 didOpen 的文件数上限。
pub const MAX_OPENED_DOCUMENTS: usize = 32;

/// didOpen 的单文件字节上限（2 MiB）。
pub const MAX_OPEN_BYTES: u64 = 2 * 1024 * 1024;

/// 缓冲区累积上限（声明的 Content-Length 已被 transport 限制，这里再留一帧余量）。
const MAX_BUFFER_BYTES: usize = transport::MAX_MESSAGE_BYTES + 64 * 1024;

/// 一处位置结果（`Location` 或 `LocationLink` 归一后的形状）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspLocation {
    /// 目标文件 URI。
    pub uri: String,
    /// 起始行（0 起）。
    pub start_line: u32,
    /// 起始列（0 起，UTF-16 码元）。
    pub start_character: u32,
    /// 结束行。
    pub end_line: u32,
    /// 结束列。
    pub end_character: u32,
}

/// 一条诊断（只保留封闭集字段）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticSummary {
    /// 起始行（0 起）。
    pub line: u32,
    /// 起始列。
    pub character: u32,
    /// 结束行。
    pub end_line: u32,
    /// 结束列。
    pub end_character: u32,
    /// 严重级别（1=Error，2=Warning，3=Information，4=Hint；缺省 `None`）。
    pub severity: Option<u8>,
    /// 来源（如 `rust-analyzer`）。
    pub source: Option<String>,
    /// 诊断码（数字或字符串，统一成字符串）。
    pub code: Option<String>,
    /// 消息（有界）。
    pub message: String,
}

/// 有界结果：超限时 `truncated = true`，调用方据它决定是否提示“结果被截断”。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bounded<T> {
    /// 结果条目。
    pub items: Vec<T>,
    /// 是否因上限被截断。
    pub truncated: bool,
}

impl<T> Bounded<T> {
    fn empty() -> Self {
        Self {
            items: Vec::new(),
            truncated: false,
        }
    }
}

/// 服务器声明的能力（封闭集内的四个开关）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Capabilities {
    definition: bool,
    references: bool,
    hover: bool,
    pull_diagnostics: bool,
}

/// 与服务器进程（或测试替身）的字节通道。
struct Session {
    /// `Some`＝通道仍在；关闭（丢弃）它会让 reader 线程读到 EOF 而退出——这是 Drop
    /// 不悬挂的前提（测试替身走 `std::io::pipe`，真实子进程则由 kill 兜底）。
    writer: Option<Box<dyn Write + Send>>,
    responses: Receiver<Value>,
    notifications: Receiver<Value>,
    reader: Option<JoinHandle<()>>,
}

impl Session {
    fn new(reader: Box<dyn Read + Send>, writer: Box<dyn Write + Send>) -> Result<Self, WhError> {
        let (response_tx, responses) = sync_channel(RESPONSE_CAPACITY);
        let (notification_tx, notifications) = sync_channel(NOTIFICATION_CAPACITY);
        let handle = std::thread::Builder::new()
            .name("whcode-lsp-reader".to_string())
            .spawn(move || {
                let mut reader = BufReader::new(reader);
                let mut buffer: Vec<u8> = Vec::new();
                let mut chunk = [0u8; 8192];
                loop {
                    let read = match reader.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => read,
                    };
                    buffer.extend_from_slice(&chunk[..read]);
                    loop {
                        match parse_frame(&buffer) {
                            Ok(Some((value, consumed))) => {
                                buffer.drain(..consumed);
                                // 响应＝有 id 且无 method；其余（含服务器→客户端请求）走通知通道。
                                let is_response =
                                    value.get("id").is_some() && value.get("method").is_none();
                                if is_response {
                                    if response_tx.send(value).is_err() {
                                        return;
                                    }
                                } else if notification_tx.try_send(value).is_err() {
                                    // 有界丢弃：通知可丢，响应不可丢。
                                }
                            }
                            Ok(None) => break,
                            Err(_) => return,
                        }
                    }
                    if buffer.len() > MAX_BUFFER_BYTES {
                        // 防御：声明长度与实际流不符时不得无界累积。
                        return;
                    }
                }
            })
            .map_err(|err| {
                WhError::new(
                    ErrorCode::IoError,
                    format!("创建 LSP reader 线程失败：{err}"),
                )
            })?;
        Ok(Self {
            writer: Some(writer),
            responses,
            notifications,
            reader: Some(handle),
        })
    }

    fn write(&mut self, message: &Value) -> Result<(), WhError> {
        let frame = encode_frame(message)?;
        let writer = self.writer.as_mut().ok_or_else(|| {
            WhError::new(
                ErrorCode::InvalidState,
                "LSP 写通道已关闭：客户端可能因超时或服务器退出而终止".to_string(),
            )
        })?;
        writer
            .write_all(&frame)
            .and_then(|()| writer.flush())
            .map_err(|err| {
                WhError::new(
                    ErrorCode::IoError,
                    format!("写入 LSP 服务器失败（管道已关闭？）：{err}"),
                )
            })
    }

    /// 关闭写通道（触发 reader 的 EOF），随后 join reader 线程——不悬挂、不泄漏。
    fn join(&mut self) {
        self.writer = None;
        if let Some(handle) = self.reader.take() {
            let _ = handle.join();
        }
    }
}

/// 语言服务器客户端（单服务器、单次查询期）。
pub struct LspClient {
    server_id: String,
    workspace: PathBuf,
    timeout_ms: u64,
    session: Session,
    child: Option<Child>,
    next_id: i64,
    capabilities: Capabilities,
    opened: HashSet<String>,
    cached_diagnostics: HashMap<String, Vec<DiagnosticSummary>>,
    closed: bool,
}

impl LspClient {
    /// 启动服务器并完成 `initialize` 握手。
    ///
    /// # Errors
    ///
    /// 可执行文件无法启动、握手超时或失败、服务器返回错误时返回结构化错误（含服务器
    /// `id` 与可操作方向）。失败路径不留下子进程。
    pub fn start(server: &ServerConfig, workspace: &Path) -> Result<Self, WhError> {
        let mut command = Command::new(&server.command);
        command
            .args(&server.args)
            .current_dir(workspace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        // 环境收敛与凭据剥离的唯一实现（与 whcode-mcp 同口径）。
        let _ = whcode_runtime::worker::apply_minimal_env(&mut command);
        let mut child = command.spawn().map_err(|err| {
            WhError::new(
                ErrorCode::RuntimeUnavailable,
                format!(
                    "启动 LSP 服务器 {} 失败（{}）：{err}；检查 .whcode/lsp.json 的 command 与 args",
                    server.id, server.command
                ),
            )
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            WhError::new(
                ErrorCode::IoError,
                format!("LSP 服务器 {} 的 stdout 不可用", server.id),
            )
        })?;
        let stdin = child.stdin.take().ok_or_else(|| {
            WhError::new(
                ErrorCode::IoError,
                format!("LSP 服务器 {} 的 stdin 不可用", server.id),
            )
        })?;
        let session = Session::new(Box::new(stdout), Box::new(stdin))?;
        let mut client = Self {
            server_id: server.id.clone(),
            workspace: workspace.to_path_buf(),
            timeout_ms: server.timeout_ms,
            session,
            child: Some(child),
            next_id: 1,
            capabilities: Capabilities::default(),
            opened: HashSet::new(),
            cached_diagnostics: HashMap::new(),
            closed: false,
        };
        if let Err(err) = client.initialize() {
            client.kill_child();
            return Err(err);
        }
        Ok(client)
    }

    /// 服务器标识（错误消息与日志用）。
    #[must_use]
    pub fn server_id(&self) -> &str {
        &self.server_id
    }

    /// 服务器声明的能力（封闭集四项）。
    #[must_use]
    pub fn supports_definition(&self) -> bool {
        self.capabilities.definition
    }

    /// 是否声明 `referencesProvider`。
    #[must_use]
    pub fn supports_references(&self) -> bool {
        self.capabilities.references
    }

    /// 是否声明 `hoverProvider`。
    #[must_use]
    pub fn supports_hover(&self) -> bool {
        self.capabilities.hover
    }

    /// 是否声明 `diagnosticProvider`（拉取诊断）。
    #[must_use]
    pub fn supports_pull_diagnostics(&self) -> bool {
        self.capabilities.pull_diagnostics
    }

    fn initialize(&mut self) -> Result<(), WhError> {
        let params = json!({
            "processId": std::process::id(),
            "clientInfo": {"name": "whcode", "version": env!("CARGO_PKG_VERSION")},
            "rootUri": file_uri(&self.workspace),
            "capabilities": {"textDocument": {"publishDiagnostics": {}}},
            "workspaceFolders": Value::Null,
        });
        let result = self.request("initialize", params)?;
        let caps = result.get("capabilities").cloned().unwrap_or(Value::Null);
        self.capabilities = Capabilities {
            definition: capability_present(&caps, "definitionProvider"),
            references: capability_present(&caps, "referencesProvider"),
            hover: capability_present(&caps, "hoverProvider"),
            pull_diagnostics: capability_present(&caps, "diagnosticProvider"),
        };
        self.notify("initialized", json!({}))?;
        Ok(())
    }

    /// 查符号定义（`textDocument/definition`）。
    ///
    /// # Errors
    ///
    /// 服务器未声明该能力（`not_supported`）、文件不可读、超时或协议错误。
    pub fn definition(
        &mut self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> Result<Bounded<LspLocation>, WhError> {
        self.require(self.capabilities.definition, "definition")?;
        let uri = self.ensure_open(path)?;
        let result = self.request(
            "textDocument/definition",
            position_params(&uri, line, character),
        )?;
        Ok(bounded_locations(&result))
    }

    /// 查引用（`textDocument/references`）。
    ///
    /// # Errors
    ///
    /// 同 [`Self::definition`]。
    pub fn references(
        &mut self,
        path: &Path,
        line: u32,
        character: u32,
        include_declaration: bool,
    ) -> Result<Bounded<LspLocation>, WhError> {
        self.require(self.capabilities.references, "references")?;
        let uri = self.ensure_open(path)?;
        let mut params = position_params(&uri, line, character);
        params["context"] = json!({"includeDeclaration": include_declaration});
        let result = self.request("textDocument/references", params)?;
        Ok(bounded_locations(&result))
    }

    /// 查悬停说明（`textDocument/hover`）。
    ///
    /// # Errors
    ///
    /// 同 [`Self::definition`]。
    pub fn hover(
        &mut self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> Result<Bounded<String>, WhError> {
        self.require(self.capabilities.hover, "hover")?;
        let uri = self.ensure_open(path)?;
        let result = self.request("textDocument/hover", position_params(&uri, line, character))?;
        let Some(text) = parse_hover(&result) else {
            return Ok(Bounded::empty());
        };
        let (text, truncated) = clamp_chars(text, MAX_HOVER_CHARS);
        Ok(Bounded {
            items: vec![text],
            truncated,
        })
    }

    /// 查文件的诊断：优先拉取，回退推送缓存。
    ///
    /// 「服务器不支持拉取且未推送该文件」返回 `not_supported`——不返回空列表冒充“没有诊断”。
    ///
    /// # Errors
    ///
    /// 文件不可读、超时、协议错误，或上述“两者皆无”的情形。
    pub fn diagnostics(&mut self, path: &Path) -> Result<Bounded<DiagnosticSummary>, WhError> {
        let uri = self.ensure_open(path)?;
        if self.capabilities.pull_diagnostics {
            let result = self.request(
                "textDocument/diagnostic",
                json!({"textDocument": {"uri": uri}}),
            )?;
            if let Some((items, truncated)) = parse_pull_diagnostics(&result) {
                let cached = items.clone();
                self.cache_diagnostics(&uri, cached);
                return Ok(Bounded { items, truncated });
            }
            // `kind: "unchanged"`：规范含义是“与上次相同”，只有缓存能回答。
            if let Some(cached) = self.cached_diagnostics.get(&uri).cloned() {
                return Ok(Bounded {
                    items: cached,
                    truncated: false,
                });
            }
            return Err(WhError::new(
                ErrorCode::NotSupported,
                format!(
                    "LSP 服务器 {} 返回的诊断报告为 `unchanged` 但本客户端无该文件的基线",
                    self.server_id
                ),
            ));
        }
        // 推送诊断路径：给服务器一个短的窗口把 didOpen 后的诊断推过来。
        self.drain_notifications();
        let deadline = Instant::now() + Duration::from_millis(self.timeout_ms.min(300));
        while Instant::now() < deadline && !self.cached_diagnostics.contains_key(&uri) {
            match self
                .session
                .notifications
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(message) => self.handle_server_message(message),
                Err(_) => break,
            }
        }
        match self.cached_diagnostics.get(&uri).cloned() {
            Some(items) => Ok(Bounded {
                items,
                truncated: false,
            }),
            None => Err(WhError::new(
                ErrorCode::NotSupported,
                format!(
                    "LSP 服务器 {} 未声明 diagnosticProvider 且未推送该文件诊断：无法给出诊断",
                    self.server_id
                ),
            )),
        }
    }

    fn require(&self, supported: bool, method: &str) -> Result<(), WhError> {
        if supported {
            return Ok(());
        }
        Err(WhError::new(
            ErrorCode::NotSupported,
            format!(
                "LSP 服务器 {} 未声明 {method} 能力：检查服务器版本或改用支持它的服务器",
                self.server_id
            ),
        ))
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, WhError> {
        if self.closed {
            return Err(WhError::new(
                ErrorCode::InvalidState,
                format!("LSP 客户端（{}）已关闭，不能继续请求", self.server_id),
            ));
        }
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.session
            .write(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        let deadline = Instant::now() + Duration::from_millis(self.timeout_ms.max(1));
        loop {
            self.drain_notifications();
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                self.fail();
                return Err(WhError::new(
                    ErrorCode::ToolTimeout,
                    format!(
                        "LSP 服务器 {} 的 {method} 超过 {} ms 未响应：已终止该服务器进程",
                        self.server_id, self.timeout_ms
                    ),
                ));
            }
            match self.session.responses.recv_timeout(remaining) {
                Ok(value) => {
                    if value.get("id").and_then(Value::as_i64) != Some(id) {
                        // 过期响应（超时请求的迟到答复）：丢弃并继续等本次响应。
                        continue;
                    }
                    if let Some(error) = value.get("error") {
                        let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
                        let message = error
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("（服务器未给 message）");
                        let error_code = if code == -32601 {
                            ErrorCode::NotSupported
                        } else {
                            ErrorCode::InvalidState
                        };
                        return Err(WhError::new(
                            error_code,
                            format!("LSP {method} 返回错误 {code}：{message}"),
                        ));
                    }
                    // 响应与通知可能同时到达：先处理已排队的通知（含服务器→客户端请求），
                    // 再返回——否则该请求要等到下一次 drain 才被答复，服务器可能空等。
                    self.drain_notifications();
                    return Ok(value.get("result").cloned().unwrap_or(Value::Null));
                }
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => {
                    self.fail();
                    return Err(WhError::new(
                        ErrorCode::IoError,
                        format!(
                            "LSP 服务器 {} 在 {method} 期间退出：检查服务器日志与其 command/args",
                            self.server_id
                        ),
                    ));
                }
            }
        }
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), WhError> {
        self.session
            .write(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    fn drain_notifications(&mut self) {
        // `Disconnected` 与 `Empty` 同义（都表示“此刻没有更多通知”），循环自然结束。
        while let Ok(message) = self.session.notifications.try_recv() {
            self.handle_server_message(message);
        }
    }

    fn handle_server_message(&mut self, message: Value) {
        if message.get("id").is_some() {
            // 服务器→客户端请求（封闭集外）：回 -32601，避免服务器阻塞在等答复上。
            let id = message.get("id").cloned().unwrap_or(Value::Null);
            let reply = json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32601, "message": "WHCode LSP 客户端只支持四查询"}
            });
            let _ = self.session.write(&reply);
            return;
        }
        if message.get("method").and_then(Value::as_str) == Some("textDocument/publishDiagnostics")
            && let Some(params) = message.get("params")
            && let Some((uri, items)) = parse_publish_diagnostics(params)
        {
            let (items, _) = bound_diagnostics(items);
            self.cache_diagnostics(&uri, items);
        }
    }

    fn cache_diagnostics(&mut self, uri: &str, items: Vec<DiagnosticSummary>) {
        if !self.cached_diagnostics.contains_key(uri)
            && self.cached_diagnostics.len() >= MAX_CACHED_DOCUMENTS
        {
            // 有界缓存已满：丢弃新文档（登记为限制），不无界增长。
            return;
        }
        self.cached_diagnostics.insert(uri.to_string(), items);
    }

    fn ensure_open(&mut self, path: &Path) -> Result<String, WhError> {
        let uri = file_uri(path);
        if self.opened.contains(&uri) {
            return Ok(uri);
        }
        if self.opened.len() >= MAX_OPENED_DOCUMENTS {
            return Err(WhError::new(
                ErrorCode::InvalidState,
                format!(
                    "单客户端已 didOpen {} 个文件（上限 {MAX_OPENED_DOCUMENTS}）：请为后续查询新建客户端",
                    self.opened.len()
                ),
            ));
        }
        let metadata = std::fs::metadata(path).map_err(|err| {
            WhError::new(
                ErrorCode::IoError,
                format!("读取 {} 元数据失败：{err}", path.display()),
            )
        })?;
        if metadata.len() > MAX_OPEN_BYTES {
            return Err(WhError::invalid_state(format!(
                "{} 为 {} 字节，超过 didOpen 上限 {MAX_OPEN_BYTES}",
                path.display(),
                metadata.len()
            )));
        }
        let text = std::fs::read_to_string(path).map_err(|err| {
            WhError::new(
                ErrorCode::IoError,
                format!("读取 {} 失败（LSP 需要 UTF-8 文本）：{err}", path.display()),
            )
        })?;
        let params = json!({
            "textDocument": {
                "uri": uri,
                "languageId": config::language_id(path),
                "version": 1,
                "text": text,
            }
        });
        self.notify("textDocument/didOpen", params)?;
        self.opened.insert(uri.clone());
        Ok(uri)
    }

    fn fail(&mut self) {
        self.closed = true;
        self.kill_child();
    }

    fn kill_child(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        // 规范化关闭：shutdown 请求 + exit 通知，再 kill 兜底；reader 线程必然 join。
        if self.child.is_some() && !self.closed {
            let id = self.next_id;
            let _ = self
                .session
                .write(&json!({"jsonrpc": "2.0", "id": id, "method": "shutdown"}));
            let _ = self
                .session
                .write(&json!({"jsonrpc": "2.0", "method": "exit"}));
        }
        self.kill_child();
        self.session.join();
    }
}

/// 文件路径 → LSP `uri`（`file://` ＋ 百分号编码；Windows 盘符规范为 `/D:/…`）。
#[must_use]
pub fn file_uri(path: &Path) -> String {
    let mut text = path.to_string_lossy().replace('\\', "/");
    if !text.starts_with('/') {
        text.insert(0, '/');
    }
    let mut out = String::from("file://");
    for byte in text.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'.' | b'_' | b'~' | b':' => {
                out.push(*byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn capability_present(capabilities: &Value, key: &str) -> bool {
    match capabilities.get(key) {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(_) => true,
    }
}

fn position_params(uri: &str, line: u32, character: u32) -> Value {
    json!({
        "textDocument": {"uri": uri},
        "position": {"line": line, "character": character},
    })
}

fn bounded_locations(result: &Value) -> Bounded<LspLocation> {
    let (mut items, mut truncated) = parse_locations(result);
    if items.len() > MAX_LOCATIONS {
        items.truncate(MAX_LOCATIONS);
        truncated = true;
    }
    Bounded { items, truncated }
}

/// 解析 `definition`／`references` 的结果：`Location`、`Location[]`、`LocationLink[]`、`null`。
fn parse_locations(value: &Value) -> (Vec<LspLocation>, bool) {
    let mut out = Vec::new();
    let mut truncated = false;
    let entries: Vec<&Value> = match value {
        Value::Null => Vec::new(),
        Value::Array(items) => items.iter().collect(),
        other => vec![other],
    };
    for entry in entries {
        if entry.is_null() {
            continue;
        }
        let (uri_value, range) = if entry.get("targetUri").is_some() {
            // LocationLink：优先 targetSelectionRange（更贴近“定义处”）。
            let range = entry
                .get("targetSelectionRange")
                .or_else(|| entry.get("targetRange"));
            (entry.get("targetUri"), range)
        } else {
            (entry.get("uri"), entry.get("range"))
        };
        let Some(uri) = uri_value.and_then(Value::as_str) else {
            continue;
        };
        if uri.chars().count() > MAX_URI_CHARS {
            truncated = true;
            continue;
        }
        let Some(range) = range else {
            continue;
        };
        let Some(start) = parse_position(range.get("start")) else {
            continue;
        };
        let Some(end) = parse_position(range.get("end")) else {
            continue;
        };
        out.push(LspLocation {
            uri: uri.to_string(),
            start_line: start.0,
            start_character: start.1,
            end_line: end.0,
            end_character: end.1,
        });
    }
    (out, truncated)
}

fn parse_position(value: Option<&Value>) -> Option<(u32, u32)> {
    let value = value?;
    let line = u32::try_from(value.get("line")?.as_u64()?).ok()?;
    let character = u32::try_from(value.get("character")?.as_u64()?).ok()?;
    Some((line, character))
}

/// 解析 `hover` 结果的 `contents` 三形态：字符串、`{value}`、数组（可混合）。
fn parse_hover(value: &Value) -> Option<String> {
    let contents = value.get("contents")?;
    let text = match contents {
        Value::String(text) => text.clone(),
        Value::Object(_) => contents.get("value")?.as_str()?.to_string(),
        Value::Array(items) => {
            let mut parts = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    Value::String(text) => parts.push(text.clone()),
                    Value::Object(_) => {
                        if let Some(text) = item.get("value").and_then(Value::as_str) {
                            parts.push(text.to_string());
                        }
                    }
                    _ => {}
                }
            }
            parts.join("\n")
        }
        _ => return None,
    };
    if text.trim().is_empty() {
        return None;
    }
    Some(text)
}

/// 解析推送诊断：`params = {uri, diagnostics[]}`。
fn parse_publish_diagnostics(params: &Value) -> Option<(String, Vec<DiagnosticSummary>)> {
    let uri = params.get("uri")?.as_str()?.to_string();
    let items = params.get("diagnostics")?.as_array()?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        if let Some(summary) = parse_diagnostic(item) {
            out.push(summary);
        }
    }
    Some((uri, out))
}

/// 解析拉取诊断：`{kind:"full", items:[…]}`；`kind:"unchanged"` 返回 `None` 交给缓存。
fn parse_pull_diagnostics(result: &Value) -> Option<(Vec<DiagnosticSummary>, bool)> {
    match result.get("kind").and_then(Value::as_str) {
        Some("full") => {
            let items = result.get("items")?.as_array()?;
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                if let Some(summary) = parse_diagnostic(item) {
                    out.push(summary);
                }
            }
            Some(bound_diagnostics(out))
        }
        _ => None,
    }
}

fn bound_diagnostics(mut items: Vec<DiagnosticSummary>) -> (Vec<DiagnosticSummary>, bool) {
    let mut truncated = false;
    if items.len() > MAX_DIAGNOSTICS {
        items.truncate(MAX_DIAGNOSTICS);
        truncated = true;
    }
    for item in &mut items {
        if item.message.chars().count() > MAX_DIAGNOSTIC_MESSAGE_CHARS {
            let (clamped, _) = clamp_chars(
                std::mem::take(&mut item.message),
                MAX_DIAGNOSTIC_MESSAGE_CHARS,
            );
            item.message = clamped;
            truncated = true;
        }
    }
    (items, truncated)
}

fn parse_diagnostic(value: &Value) -> Option<DiagnosticSummary> {
    let range = value.get("range")?;
    let (line, character) = parse_position(range.get("start"))?;
    let (end_line, end_character) = parse_position(range.get("end"))?;
    let message = value.get("message")?.as_str()?.to_string();
    let severity = value
        .get("severity")
        .and_then(Value::as_u64)
        .and_then(|raw| u8::try_from(raw).ok());
    let source = value
        .get("source")
        .and_then(Value::as_str)
        .map(str::to_string);
    let code = value.get("code").and_then(|code| match code {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    });
    Some(DiagnosticSummary {
        line,
        character,
        end_line,
        end_character,
        severity,
        source,
        code,
        message,
    })
}

/// 按字符数截断（不是字节）：超出部分丢弃并返回 `truncated = true`。
fn clamp_chars(text: String, limit: usize) -> (String, bool) {
    if text.chars().count() <= limit {
        return (text, false);
    }
    let mut out: String = text.chars().take(limit).collect();
    out.push('…');
    (out, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::pipe;

    // ── 纯解析测试（不需要子进程） ────────────────────────────────────────────

    #[test]
    fn file_uri_normalizes_windows_and_encodes_non_ascii() {
        assert_eq!(
            file_uri(Path::new(r"D:\WHCode\src\main.rs")),
            "file:///D:/WHCode/src/main.rs"
        );
        assert_eq!(
            file_uri(Path::new("/home/user/a b.rs")),
            "file:///home/user/a%20b.rs"
        );
        assert_eq!(
            file_uri(Path::new(r"C:\目录\x.rs")),
            "file:///C:/%E7%9B%AE%E5%BD%95/x.rs"
        );
    }

    #[test]
    fn locations_parse_all_three_shapes() {
        let single = json!({"uri": "file:///a.rs", "range": {"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 5}}});
        let (items, truncated) = parse_locations(&single);
        assert_eq!(items.len(), 1);
        assert!(!truncated);
        assert_eq!(items[0].start_line, 1);
        assert_eq!(items[0].end_character, 5);

        let array = json!([
            {"uri": "file:///a.rs", "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}},
            {"targetUri": "file:///b.rs", "targetSelectionRange": {"start": {"line": 3, "character": 4}, "end": {"line": 3, "character": 8}}}
        ]);
        let (items, _) = parse_locations(&array);
        assert_eq!(items.len(), 2);
        assert_eq!(items[1].uri, "file:///b.rs");
        assert_eq!(items[1].start_line, 3);

        let (empty, truncated) = parse_locations(&Value::Null);
        assert!(empty.is_empty(), "null 是正确的空");
        assert!(!truncated);

        // 破损条目被跳过（不是整批失败）。
        let (items, _) = parse_locations(&json!([{"uri": 3}, {"range": {}}]));
        assert!(items.is_empty());
    }

    #[test]
    fn hover_parses_string_object_and_array() {
        assert_eq!(
            parse_hover(&json!({"contents": "纯文本"})),
            Some("纯文本".to_string())
        );
        assert_eq!(
            parse_hover(&json!({"contents": {"kind": "markdown", "value": "**粗**"}})),
            Some("**粗**".to_string())
        );
        assert_eq!(
            parse_hover(&json!({"contents": [{"language": "rust", "value": "fn f()"}, "说明"]})),
            Some("fn f()\n说明".to_string())
        );
        assert_eq!(parse_hover(&json!({"contents": []})), None);
        assert_eq!(parse_hover(&json!({"contents": "   "})), None);
        assert_eq!(parse_hover(&json!({})), None);
    }

    #[test]
    fn hover_text_is_clamped_with_truncation_flag() {
        let long = "甲".repeat(MAX_HOVER_CHARS + 10);
        let (clamped, truncated) = clamp_chars(long, MAX_HOVER_CHARS);
        assert!(truncated);
        assert_eq!(
            clamped.chars().count(),
            MAX_HOVER_CHARS + 1,
            "截断后加省略号"
        );
        let (short, truncated) = clamp_chars("短".to_string(), MAX_HOVER_CHARS);
        assert!(!truncated);
        assert_eq!(short, "短");
    }

    #[test]
    fn diagnostics_parse_push_and_pull_shapes() {
        let params = json!({
            "uri": "file:///a.rs",
            "diagnostics": [
                {"range": {"start": {"line": 2, "character": 0}, "end": {"line": 2, "character": 3}},
                 "severity": 1, "source": "rust-analyzer", "code": "E0308", "message": "类型不匹配"},
                {"range": {"start": {"line": 4, "character": 1}, "end": {"line": 4, "character": 2}},
                 "message": "无严重级别", "code": 42}
            ]
        });
        let (uri, items) = parse_publish_diagnostics(&params).expect("解析推送诊断");
        assert_eq!(uri, "file:///a.rs");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].severity, Some(1));
        assert_eq!(items[0].code.as_deref(), Some("E0308"));
        assert_eq!(items[1].severity, None);
        assert_eq!(items[1].code.as_deref(), Some("42"));

        let pull = json!({"kind": "full", "items": [
            {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}, "message": "警告"}
        ]});
        let (items, truncated) = parse_pull_diagnostics(&pull).expect("解析拉取诊断");
        assert_eq!(items.len(), 1);
        assert!(!truncated);
        assert!(parse_pull_diagnostics(&json!({"kind": "unchanged"})).is_none());
    }

    #[test]
    fn diagnostics_are_bounded_and_messages_clamped() {
        let long_message = "长".repeat(MAX_DIAGNOSTIC_MESSAGE_CHARS + 5);
        let items: Vec<DiagnosticSummary> = (0..MAX_DIAGNOSTICS + 3)
            .map(|index| DiagnosticSummary {
                line: u32::try_from(index).unwrap_or(0),
                character: 0,
                end_line: 0,
                end_character: 0,
                severity: None,
                source: None,
                code: None,
                message: if index == 0 {
                    long_message.clone()
                } else {
                    "短".to_string()
                },
            })
            .collect();
        let (items, truncated) = bound_diagnostics(items);
        assert_eq!(items.len(), MAX_DIAGNOSTICS, "条数必须截断");
        assert!(truncated, "截断必须显式标记");
        assert_eq!(
            items[0].message.chars().count(),
            MAX_DIAGNOSTIC_MESSAGE_CHARS + 1,
            "超长消息截断并加省略号"
        );
    }

    // ── 协议级测试（std::io::pipe 替身，不启动外部进程） ─────────────────────

    /// 测试替身：按封闭集回答四查询，并记录收到的请求方法。
    fn stub_session(
        respond: impl Fn(&Value) -> Vec<Value> + Send + 'static,
    ) -> (Session, std::thread::JoinHandle<Vec<String>>) {
        let (client_reader, mut server_writer) = pipe().expect("pipe 1");
        let (mut server_reader, client_writer) = pipe().expect("pipe 2");
        let session = Session::new(Box::new(client_reader), Box::new(client_writer)).expect("会话");
        let handle = std::thread::spawn(move || {
            let mut buffer: Vec<u8> = Vec::new();
            let mut chunk = [0u8; 4096];
            let mut methods = Vec::new();
            loop {
                let read = match server_reader.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => read,
                };
                buffer.extend_from_slice(&chunk[..read]);
                while let Ok(Some((value, consumed))) = parse_frame(&buffer) {
                    buffer.drain(..consumed);
                    methods.push(
                        value
                            .get("method")
                            .and_then(Value::as_str)
                            .unwrap_or("(reply)")
                            .to_string(),
                    );
                    for reply in respond(&value) {
                        let frame = encode_frame(&reply).expect("编码响应");
                        if server_writer.write_all(&frame).is_err() {
                            return methods;
                        }
                        let _ = server_writer.flush();
                    }
                }
            }
            methods
        });
        (session, handle)
    }

    fn client_with(session: Session) -> LspClient {
        let capabilities = Capabilities {
            definition: true,
            references: true,
            hover: true,
            pull_diagnostics: true,
        };
        LspClient {
            server_id: "stub".to_string(),
            workspace: PathBuf::from("."),
            timeout_ms: 2_000,
            session,
            child: None,
            next_id: 1,
            capabilities,
            opened: HashSet::new(),
            cached_diagnostics: HashMap::new(),
            closed: false,
        }
    }

    /// 替身应答器：initialize／didOpen／四查询按封闭集回答。
    fn stub_responding(message: &Value) -> Vec<Value> {
        let (Some(id), Some(method)) = (
            message.get("id").cloned(),
            message
                .get("method")
                .and_then(Value::as_str)
                .map(str::to_string),
        ) else {
            return Vec::new();
        };
        let result = match method.as_str() {
            "initialize" => json!({"capabilities": {
                "definitionProvider": true,
                "referencesProvider": true,
                "hoverProvider": true,
                "diagnosticProvider": {"interFileDependencies": false, "workspaceDiagnostics": false}
            }}),
            "textDocument/definition" => json!([
                {"uri": "file:///a.rs", "range": {"start": {"line": 9, "character": 4}, "end": {"line": 9, "character": 7}}}
            ]),
            "textDocument/references" => json!([]),
            "textDocument/hover" => {
                json!({"contents": {"kind": "markdown", "value": "```rust\nfn f()\n```"}})
            }
            "textDocument/diagnostic" => json!({"kind": "full", "items": [
                {"range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 2}},
                 "severity": 2, "message": "未使用变量"}
            ]}),
            _ => return Vec::new(),
        };
        vec![json!({"jsonrpc": "2.0", "id": id, "result": result})]
    }

    fn temp_source(tag: &str) -> PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let dir = std::env::temp_dir().join(format!(
            "whcode-lsp-client-{tag}-{}-{stamp}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("建夹具");
        let path = dir.join("sample.rs");
        std::fs::write(&path, "fn main() {}\n").expect("写源文件");
        path
    }

    /// 握手与四查询的完整协议往返：didOpen 先发、结果按封闭集解析。
    #[test]
    fn protocol_roundtrip_initializes_and_answers_four_queries() {
        let (session, server) = stub_session(stub_responding);
        let mut client = client_with(session);
        client.initialize().expect("握手");
        assert!(client.supports_definition());
        assert!(client.supports_pull_diagnostics());

        let path = temp_source("four");
        let definition = client.definition(&path, 0, 3).expect("定义");
        assert_eq!(definition.items.len(), 1);
        assert_eq!(definition.items[0].start_line, 9);

        let references = client.references(&path, 0, 3, true).expect("引用");
        assert!(references.items.is_empty(), "空数组是正确的空");
        assert!(!references.truncated);

        let hover = client.hover(&path, 0, 3).expect("悬停");
        assert_eq!(hover.items.len(), 1);
        assert!(hover.items[0].contains("fn f()"));

        let diagnostics = client.diagnostics(&path).expect("诊断");
        assert_eq!(diagnostics.items.len(), 1);
        assert_eq!(diagnostics.items[0].severity, Some(2));

        drop(client);
        let methods = server.join().expect("替身线程");
        assert!(
            methods.contains(&"initialize".to_string()),
            "必须完成握手：{methods:?}"
        );
        assert!(
            methods.contains(&"textDocument/didOpen".to_string()),
            "查询前必须 didOpen：{methods:?}"
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap_or(Path::new(".")));
    }

    /// 服务器→客户端请求：回 -32601 而不是让服务器挂住；随后仍能正常收响应。
    #[test]
    fn unsupported_server_request_gets_method_not_found_and_flow_continues() {
        let (session, server) = stub_session(|message| {
            let Some(method) = message.get("method").and_then(Value::as_str) else {
                return Vec::new();
            };
            if method == "textDocument/definition" {
                // 先发一个 server→client 请求，再发本次响应（两帧）。
                return vec![
                    json!({"jsonrpc": "2.0", "id": 99, "method": "window/workDoneProgress/create", "params": {}}),
                    json!({"jsonrpc": "2.0", "id": message.get("id").cloned().unwrap_or(Value::Null), "result": null}),
                ];
            }
            Vec::new()
        });
        let mut client = client_with(session);
        // 直接发请求（未 initialize 也能验证请求/响应通道）。
        let result = client
            .request("textDocument/definition", json!({}))
            .expect("请求");
        assert!(result.is_null(), "null 结果按正确的空处理");
        drop(client);
        let methods = server.join().expect("替身线程");
        assert_eq!(
            methods.first().map(String::as_str),
            Some("textDocument/definition")
        );
        assert!(
            methods.iter().any(|method| method == "(reply)"),
            "服务器→客户端请求必须被答复（-32601），否则服务器会挂住：{methods:?}"
        );
    }

    /// 缺能力声明 → `not_supported`（不是空结果）。
    #[test]
    fn missing_capability_is_not_supported() {
        let (session, server) = stub_session(|message| {
            let Some(id) = message.get("id").cloned() else {
                return Vec::new();
            };
            vec![json!({"jsonrpc": "2.0", "id": id, "result": {"capabilities": {}}})]
        });
        let mut client = client_with(session);
        client.initialize().expect("握手");
        let path = temp_source("nocap");
        let err = client.definition(&path, 0, 0).expect_err("缺能力必须报错");
        assert_eq!(err.code, ErrorCode::NotSupported);
        drop(client);
        let _ = server.join();
        let _ = std::fs::remove_dir_all(path.parent().unwrap_or(Path::new(".")));
    }

    /// 超时：服务器不答复 → `tool_timeout`（不是无限等）。
    #[test]
    fn timeout_is_reported_as_tool_timeout() {
        let (session, server) = stub_session(|_message| Vec::new());
        let mut client = client_with(session);
        client.timeout_ms = 120;
        let err = client
            .request("textDocument/definition", json!({}))
            .expect_err("必须超时");
        assert_eq!(err.code, ErrorCode::ToolTimeout);
        drop(client);
        let _ = server.join();
    }
}
