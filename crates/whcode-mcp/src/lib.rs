//! # MCP 客户端最小集（FR-736/FR-737、ADR-054）
//!
//! stdio transport 的 MCP（Model Context Protocol）客户端：显式配置驱动的子进程会话、
//! newline 分帧 JSON-RPC 2.0 协议层、`tools/list` 投影与 `tools/call` 调用。
//!
//! 范围（ADR-054 §2）：v1 只做 stdio 单 transport（官方规范现行双 transport 之一，
//! "Clients SHOULD support stdio whenever possible"；Streamable HTTP/OAuth 留后续
//! 批次）；自研最小 JSON-RPC 客户端——官方 Rust SDK（rmcp）强依赖 tokio，与本仓
//! 全同步架构冲突。
//!
//! 有界性（NFR-215/ADR-054 §2.5—2.7）：
//! - 服务器数 ≤ [`config::MAX_SERVERS`]、单行消息 ≤ [`session::MAX_LINE_BYTES`]；
//! - 每服务器工具 ≤ [`manager::MAX_TOOLS_PER_SERVER`]、MCP 清单总预算
//!   [`manager::MAX_MANIFEST_BYTES`]；
//! - 请求超时默认 30 s（上限 600 s），看门狗线程到点 `terminate_tree` 解锁阻塞读
//!   （瞬态线程，每在途请求一条）；turn 取消令牌同权生效；
//! - 会话生命周期 per-turn：组装清单时 spawn，turn 结束（含早退）经 `Drop` 回收，
//!   无空闲连接池、无常驻线程。

pub mod config;
pub mod manager;
pub mod protocol;
pub mod session;

pub use config::{McpConfig, McpServerConfig};
pub use manager::{
    MAX_TOOLS_PER_SERVER, MCP_TOOL_PREFIX, McpManager, McpManifest, McpToolSet, parse_tool_name,
    tool_display_name,
};
pub use protocol::{McpTool, PROTOCOL_VERSION, ToolCallResult};
pub use session::{CancelFlag, EofKind, HttpTransport, McpSession, StdioTransport, Transport};
