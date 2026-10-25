//! # 真实 LLM HTTP 传输层（FR-201／FR-203／FR-206／FR-616、NFR-215、ADR-031）
//!
//! 当前交付：有界流帧解码（[`frames`]）、五协议适配器（[`protocols`]）、剖面／凭据模型
//! （[`profile`]）、解码结果到 chunk 流的归一（[`chunks`]，T164），以及仅 `http` feature
//! 打开的阻塞式客户端（[`client`]，T163—T166；`llm run` 的装配属 T167）。
//! `http` feature 默认关闭，网络依赖只在装配根打开时进入编译图。
//! `tls`（仅 `http` feature）做操作系统的出站信任装配与只握手校验（T188、ADR-040 决策 2）。

pub mod chunks;
#[cfg(feature = "http")]
pub mod client;
pub mod errors;
pub mod frames;
pub mod profile;
pub mod protocols;
#[cfg(feature = "http")]
pub mod tls;
