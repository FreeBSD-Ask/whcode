# ADR-021：终端窗口最小化探测路径（T135、FR-617）

- **状态**：已批准（2026-10-22，T135 落地——PowerShell 子进程路径）
- **来源**：T135 要求 TUI 窗口最小化检测（Windows `GetConsoleWindow`+`IsIconic`，
  FR-617）。这两个 Win32 API 是 `unsafe extern` 函数——workspace
  `unsafe_code = "forbid"`（NFR-215）经 `--forbid` 传递不可被成员覆盖，
  且契约测试 T143 锁定该设置。
- **责任人**：平台层负责人
- **退出条件**：若 crossterm 未来提供窗口状态查询 API，本路径可简化（复审）。

## 决策

**PowerShell 子进程做 Win32 查询**（`std::process::Command`，worker.rs 同模式）：
Add-Type P/Invoke `GetConsoleWindow`+`IsIconic`，Rust 代码**零 unsafe**（NFR-215
合规），**零新依赖**。非 Windows 平台或 PowerShell 失败返回 `None`（不伪装）。

本机实测：`Some(false)`（控制台未最小化）——真实探测，非桩。

## 成本与调用约定

每次 ≈ 200—500ms（PowerShell 启动 + Add-Type 编译）——**不得在渲染帧路径调用**；
由事件循环低频调用（≥ 5s 间隔），结果经 `RenderEvent::Minimized/Restored` 传入
渲染门控（`tui/render.rs`，门控本身零成本）。

## 否决的替代方案

- **winapi/windows crate 进程内 FFI**：`unsafe extern` 与 NFR-215 `--forbid`
  不可覆盖冲突（T143 锁定）。
- **crossterm_winapi 安全包装**：不暴露 IsIconic/GetConsoleWindow。
- **ADR-015 例外（独立 crate + unsafe + 安全包装）**：引入新 crate 复杂度高；
  PowerShell 子进程路径以零成本达到同等效果。

## 影响

- `crates/whcode-platform/src/term.rs` 新增 `is_window_minimized()`；
- `whcode-tui/render.rs` 渲染门控消费端已交付（minimized 置真跳过绘制、
  关键事件记账置脏、还原单次全量重绘——消费方经 RenderEvent 接入）。
