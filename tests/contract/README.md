# tests/contract

跨 crate 契约测试（tasks.md T008、T088、T089、T095）。
经本包 `Cargo.toml` 的 `[[test]]` 显式注册（cargo 只自动发现 `tests/*.rs` 与 `tests/*/main.rs`，
嵌套目录必须显式声明）。

## 已建立

- `test_layering.rs`（`layering`）——分层与平台隔离：单向依赖、`cfg(target_os)` 只在
  `whcode-platform`、前端不直接打开数据库（T019）。
- `test_layout.rs`（`layout`）——目录分类门禁：根文件白名单、`tests/` 仅六类且各有 README、
  `docs/` 仅 adr/matrices/guides、本地与产物目录不承载源码。
- `test_json_envelope.rs`（`json_envelope`）——CUI JSON 信封：`schema_version` 必备、
  错误对象四字段、稳定退出码（T089）。
- `test_tui_cui_parity.rs`（`tui_cui_parity`）——TUI 与 CUI 命令级一致性（T088）。
- `test_web_security_defaults.rs`（`web_security_defaults`）——Web 默认安全值与会话边界（T095、V-110）。
- `test_frontend_parity.rs`（`frontend_parity`）——四端同源：快照、事件游标与命令语义
  （ADR-032 允许的差异项单独登记）。

## 运行

```powershell
cargo test -p whcode-tests --test layering
cargo test -p whcode-tests --test layout
cargo test -p whcode-tests --test json_envelope
cargo test -p whcode-tests --test tui_cui_parity
cargo test -p whcode-tests --test web_security_defaults
cargo test -p whcode-tests --test frontend_parity
```
