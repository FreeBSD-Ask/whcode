//! # whcode-web
//!
//! Web API、WebSocket 与浏览器界面（ADR-004、FR-603/606/609）。
//!
//! 能力范围：HTTP API 与 WebSocket 经 whcode-app 命令访问同一应用服务；默认绑定
//! 回环地址，外部监听须显式启用认证、Origin 检查与安全配置（FR-606）。能力与差异的
//! 权威登记见 `whcode-app::surface` 与 `docs/matrices/`，本处不维护状态快照。
//!
//! 硬约束：Web 是**原生 Web UI**，不是 TUI 的 PTY 镜像（ADR-004）；默认仅监听回环地址，
//! 对外监听必须显式启用身份验证、Origin 检查与安全配置（FR-606）；Web token 不经 URL
//! query 传递，WebSocket 须校验来源、身份、授权与消息大小（§10.2）。

pub mod api;
pub mod assets;
pub mod bind;
pub mod broadcast;
pub mod security;
pub mod surface_map;
pub mod ws;
