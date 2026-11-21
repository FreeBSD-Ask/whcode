# GUI 资源复测（本机，2026-11-21）

来源：本机 Windows 11 开发会话，release 构建（`cargo build --release -p whcode-bin --locked`，
`target/release/whcode.exe`，32 457 216 字节），main@`85d0521`。

脚本：`python .github/scripts/gui-probe.py --resources-only --out artifacts/gui-run-2026-11-21`

## 为什么是「资源专用模式」

本会话无桌面直流访问权：PIL `ImageGrab` 报 `screen grab failed`（窗口可创建、可枚举，
但截屏与按键注入不可达）。ADR-027 的预算判据是 RSS／私有内存／线程／句柄／空闲 CPU，
与像素无关，因此本次以 `--resources-only` 只取资源证据——**像素、主题色值与键位断言
本轮未取证，不冒充通过**；`summary.json` 中各处带 `screenshots: false` 与
`theme_cycle.skipped = true` 标记。

## 判定（ADR-027 预算 / FR-712）

| 指标 | 预算 | 实测（eink-day，空闲 10 s） | 结论 |
|---|---|---|---|
| RSS（WorkingSet） | ≤ 96 MB | **81.9 MB** | 通过 |
| 私有内存 | ≤ 80 MB | **46.9 MB** | 通过 |
| 线程 | ≤ 24 | **28** | **未通过**（存量缺口，见下） |
| 句柄 | ≤ 256 | **251** | 通过（余量仅 5，见下） |
| 空闲 CPU | ≤ 0.2% 单核 | **0.0%** | 通过 |

## 与既有记录的对照

- **线程 28**：与 `docs/guides/project-status.md`「GUI 空闲线程待查（2026-11-03 观察）」
  同值——本机 DPI 2.0、客户区 1100×900、16 核。本轮在**第 11 视图**（见下）的构建上
  **再次复现**，说明该残余与四批新增（runtime 视图、证书面板、事件导出、规则写入）
  无关，属 ADR-042 方案 A 之后的存量缺口（FR-712 未达 24）。
- **句柄 251 vs 预算 256**：余量仅 5。历史本机记录为 233—251 区间波动；本轮新增视图
  与面板渲染会短暂抬高句柄数。**登记为观察项**，不据此放宽预算；若后续越过 256 需
  定位来源（图标／字体／线程池句柄）。
- **RSS 81.9 MB** 高于 2026-11-03 CI run 的 77.6 MB：本机 DPI 2.0（客户区 1100×900 vs
  CI 的 1004×688）与 11 视图（CI 当时 6 视图）共同解释；仍在 96 MB 预算内。
- **视图数**：窗口标题实测 `WHCode Coding Harness  |  Sessions (1/11)`，确认
  `ViewId` 计数为 11（ADR-092 批 2 第 11 视图生效），且标题随视图切换的格式未变。

## 未取得

- 四套主题的精确色值／中间灰占比、`theme_cycle.ok`、焦点与正常退出码（`q` 未能注入，
  进程由脚本 terminate）——须在交互式桌面上跑完整模式。
- `minimized` 的最小化 CPU 增量（本轮 `measure_resources` 已采，但因无焦点，最小化
  路径未走完；见 `summary.json`）。
- CI 结论（ADR-037）：本目录仅本机复现证据，不作验收结论。
