//! # whcode-lsp
//!
//! LSP 四查询客户端（封闭集）：`definition`／`references`／`hover`／诊断（拉取，回退推送）。
//! 设计、边界与失效条件见 ADR-078；模型工具面（`lsp_*` 四工具与注册）属 P5.3b，不在本 crate。
//!
//! 三层，各自可单测：
//!
//! - [`transport`]：基础协议帧的编解码（`Content-Length` 必填、UTF-8、单条上限）——纯函数；
//! - [`config`]：`.zcode/lsp.json` 的解析与校验（显式 opt-in、写错显式失败、全部有上限）；
//! - [`client`]：进程生命周期（reader 线程＋有界通道＋Drop join）、握手、didOpen 与四查询。
//!
//! 不做的事（显式）：不内置任何语言服务器、不做 completion／rename／formatting、
//! 不做客户端复用（每查询期一个客户端）、不声称已验证与具体服务器兼容。

pub mod client;
pub mod config;
pub mod transport;

pub use client::{
    Bounded, DiagnosticSummary, LspClient, LspLocation, MAX_CACHED_DOCUMENTS,
    MAX_DIAGNOSTIC_MESSAGE_CHARS, MAX_DIAGNOSTICS, MAX_HOVER_CHARS, MAX_LOCATIONS, MAX_OPEN_BYTES,
    MAX_OPENED_DOCUMENTS, MAX_URI_CHARS, file_uri,
};
pub use config::{
    CONFIG_PATH, DEFAULT_TIMEOUT_MS, MAX_ARG_BYTES, MAX_ARGS, MAX_CONFIG_BYTES, MAX_EXTENSIONS,
    MAX_ID_BYTES, MAX_SERVERS, MAX_TIMEOUT_MS, MIN_TIMEOUT_MS, ServerConfig, extension_of,
    for_extension, language_id, load,
};
pub use transport::{CONTENT_TYPE, MAX_MESSAGE_BYTES, encode_frame, parse_frame};
