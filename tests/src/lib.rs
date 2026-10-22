//! # whcode-tests
//!
//! 跨 crate 契约测试包：分层约束（ADR-003）、JSON 信封契约、三端一致性等。
//!
//! 目录约定（tasks.md T008）：
//! - `contract/` —— 跨 crate 契约测试（经 `Cargo.toml` 的 `[[test]]` 显式注册）；
//! - `unit/`、`integration/`、`crash/`、`stability/`、`fixtures/` —— 各自 README 说明用途，
//!   对应测试随实现里程碑落地（M1 起引入存储/崩溃注入/稳定性夹具）。
//!
//! 注意：仓库根 `tests/` 是 workspace 成员包，而非 cargo 默认的集成测试目录——这样跨 crate
//! 测试才有自己的依赖图与显式测试目标注册。
