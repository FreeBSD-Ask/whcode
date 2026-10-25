//! # 真实 LLM HTTP 传输层（FR-201／FR-203／FR-206／FR-616、NFR-215、ADR-031）
//!
//! 当前交付：有界流帧解码（[`frames`]）、五协议适配器（[`protocols`]）与剖面／凭据模型
//! （[`profile`]）。阻塞式 `ureq` 客户端与 `llm run` 装配属 T162—T163（未交付）；
//! 本模块自身不引入网络依赖，网络依赖只在 `http` feature 打开时进入装配根。
//! `tls`（仅 `http` feature）做操作系统的出站信任装配与只握手校验（T188、ADR-040 决策 2）。

pub mod errors;
pub mod frames;
pub mod profile;
pub mod protocols;
#[cfg(feature = "http")]
pub mod tls;
