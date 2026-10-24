//! # whcode-ssh
//!
//! SSH 入口：公钥认证、会话隔离、终端尺寸变化与安全退出（FR-607/608、ADR-005、ADR-030）。
//!
//! ## 已交付（M7，T096—T098；进程内测试为准，四 BSD 平台证据仍缺——Check 96/97/99）
//!
//! - [`proto`]：SSH **客户端**。按 ADR-007 的“意图记录—执行—结果登记”协议执行命令；
//!   主机密钥经 [`trust::KnownHosts`] 做 TOFU 校验（首次连接记录指纹并打印供人工核对，
//!   不匹配即结构化拒绝）——ADR-005 登记的“恒接受”阻断项已在本版结清；
//! - [`hostkey`]：服务端主机密钥（首次启动生成 Ed25519 并落盘 `<workspace>/ssh/host_key`，
//!   指纹只以 `SHA256:…` 形式进日志）与授权公钥清单（`<workspace>/ssh/authorized_keys`）；
//! - [`server`]：`russh::server` 监听（默认仅回环）、仅公钥认证（仅 Ed25519）、每连接独立
//!   [`server::SessionContext`]（会话 ID + 授权 Actor + 有界通道状态）、pty 尺寸记录、
//!   channel close / disconnect 的资源回收；
//! - [`policy`]：受限 WHCode 命令白名单（经 [`adapter`] 走 `whcode-app` 应用服务），
//!   白名单外一律结构化拒绝（非 0 退出码 + stderr 说明）；任意系统 Shell 是显式独立权限
//!   （配置开关默认关闭 + 显式授权名单），默认入口无 Shell（FR-608）；
//! - [`adapter`]：单条装配根注入的 `Connection` 由**专用执行线程**独占，SSH 会话经有界
//!   命令队列投递请求（满即背压拒绝）——服务端全程只有一条数据库连接、一个写入方
//!   （ADR-005、C-03、Check 98）。
//!
//! ## 仍未交付（显式登记，不得虚称）
//!
//! - 四 BSD 服务端验收证据（V-004、Check 96/99）：ADR-013 延后，本机无平台环境；
//! - 系统 Shell 的实际执行（M8 运行时 payload，ADR-024）：M7 只交付权限判定与审计，
//!   放行结果为结构化的“未实现”说明，不启动任何子进程；
//! - 交互式 PTY 终端：pty-req 仅记录尺寸，不提供交互 shell 通道（`shell_request` 恒拒绝）。
//!
//! ## 硬约束（ADR-005/030、§10.2）
//!
//! SSH 是独立传输入口，**复用**应用服务，不得每会话各自打开数据库并直接写入；默认只开放
//! 受限 WHCode 交互入口；密钥材料与口令不进日志、事件与审计（只记 `SHA256:…` 指纹）。

pub mod adapter;
pub mod hostkey;
pub mod policy;
pub mod proto;
pub mod server;
pub mod trust;
