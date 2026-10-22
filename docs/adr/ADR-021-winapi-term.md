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

## 修订记录

- **2026-10-22（事件循环接线，T135 完成）**：探测消费端接入真实运行循环
  （`whcode-tui/src/lib.rs` 的 `LoopCore::maybe_probe`）。调用约定进一步收窄以守住
  空闲 CPU 预算（NFR-205），比初始“≥5s 间隔”更严：

  1. **只在存在待绘制帧（脏）且输入静默 ≥1s 时探测**——无待绘制帧时探测不改变任何
     行为（空闲探测成本为零）；连续输入期间输入本身即证明窗口可见（零探测）；
  2. 间隔自 5s 起**指数退避**至 60s 上限（≈0.5% 单核占用上界），检测到最小化后回落
     5s 以及时发现还原（还原后单次全量重绘，FR-617）；
  3. 探测失败（`None`，如非 Windows）**不改变门控状态**、按退避继续低频复核（不伪装）；
  4. 输入事件到达即按“还原”记账（输入 ⇒ 可见）——避免门控因探测失败卡在最小化态。

  证据：`whcode-tui` 内联重放测试（探测计数/最小化跳过/还原单次重绘/空闲零探测/
  连续输入零探测，`run_scripted_steps` 与真实循环共用 `LoopCore`）；`whcode-bin`
  装配根注入 `is_window_minimized`（`launch_interactive`）。
