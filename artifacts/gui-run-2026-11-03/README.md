# GUI 运行时证据（CI，2026-11-03）

来源：CI run [`41591647401`](https://github.com/FreeBSD-Ask/whcode/actions/runs/41591647401)
（workflow_dispatch，`desktop=true`，main@7b88384，`desktop` job success）。
脚本：`.github/scripts/gui-probe.py --exe target/release/whcode.exe --out artifacts/gui-run-ci`。
文件为本轮 artifact 原样落盘（`summary.json` 行尾统一 LF）。

## 判定（ADR-027 预算 / FR-711—FR-715）

| 指标 | 预算 | 实测（eink-day，空闲 10 s） |
|---|---|---|
| RSS | ≤ 96 MB | **77.6 MB** |
| 线程 | ≤ 24 | **19** |
| 句柄 | ≤ 256 | **232** |
| 空闲 CPU | ≤ 0.2% 单核 | **0.0%** |

- 四套主题均 `theme_cycle.ok = true`：`t` 键循环四步回到起点，逐帧按调色板归主题
  （`expected_themes == actual_themes`，比值见 `summary.json` 的 `observed_ratios`）。
- `focus_initial`／`focus_after_restore`／`minimized_iconic` 全为 true，`exit_code = 0`；
  最小化期间 CPU 增量为 0（`minimized.cpu_seconds_delta = 0.0`）。
- 三套主题精确色值与中间灰占比：见各主题 `pixels.true_gray_ratio`（均 ≤ 5%）。
- 截图：`gui-<theme>.png`（初始帧）、`gui-<theme>-view3.png`（视图 3）、
  `gui-<theme>-diff.png`（diff 预览）、`gui-<theme>-theme<N>.png`（主题循环逐帧）、
  `gui-<theme>-restored.png`（最小化还原）。

## 边界（不得扩大解释）

- 托管 runner 为**虚拟显示**环境（无真实交互桌面），结论是 headless 口径；
  物理键盘输入计数、真实 resize 拖拽等仍需人工留证（V-003 口径，见 `docs/guides/tui.md`）。
- 本轮同 job 的 NFR-205／NFR-206（TUI 空闲 CPU 0.312%／流式期间 0.172%）由
  `measure-nfr-budgets.py` 测得，属 TUI 侧指标，不属于本目录的 GUI 证据。
