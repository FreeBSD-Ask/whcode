//! # 真实 LLM HTTP 传输层（FR-209—FR-214、ADR-031）
//!
//! 当前交付：有界流帧解码（[`frames`]）与剖面／凭据模型（[`profile`]）。
//! 协议适配器与阻塞式 `ureq` 客户端由 add-real-llm-provider 的 T162—T163 接入；
//! 本模块自身不引入网络依赖，网络依赖只在 `http` feature 打开时进入装配根。

pub mod frames;
pub mod profile;
