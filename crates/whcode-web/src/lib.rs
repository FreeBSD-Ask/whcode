//! # whcode-web
//!
//! Web API、WebSocket 与浏览器界面（ADR-004、FR-603/606/609）。
//!
//! **当前状态：骨架，功能未实现。** 交付计划：API/WS 接入应用服务 → M6 / T090；默认
//! `127.0.0.1` 绑定与外部监听门禁 → T091；认证/Origin/消息大小限制 → T092；业务界面 →
//! T093；断线补拉 → T094。
//!
//! 硬约束：Web 是**原生 Web UI**，不是 TUI 的 PTY 镜像（ADR-004）；默认仅监听回环地址，
//! 对外监听必须显式启用身份验证、Origin 检查与安全配置（FR-606）；Web token 不经 URL
//! query 传递，WebSocket 须校验来源、身份、授权与消息大小（§10.2）。

pub mod api;
pub mod assets;
pub mod bind;
pub mod security;
pub mod ws;
