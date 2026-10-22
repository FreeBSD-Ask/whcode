# tests/contract

跨 crate 契约测试（tasks.md T008、T088、T089、T095）。

经本包 `Cargo.toml` 的 `[[test]]` 显式注册（cargo 只自动发现 `tests/*.rs` 与 `tests/*/main.rs`，
嵌套目录必须显式声明）。

已建立：
- `test_layering.rs` —— 分层与平台隔离（T019）

计划：
- `test_json_envelope.rs` —— CUI JSON 信封与错误四字段（T089）
- `test_tui_cui_parity.rs` —— 三端命令级一致性（T088）
- `test_web_security_defaults.rs` —— V-110 默认安全值（T095）
