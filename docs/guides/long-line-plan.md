# 长线计划（Long-Line Plan）

> 建立：2026-11-01 | 适用：`main@ba5b8e9` 起 | 性质：**工作序列与门禁的权威索引**（非验收结论）
>
> 本页把 WHCode 的剩余工作按依赖与门禁串成序列，供本会话与滚动子代理遵循。
> 与 `specs/001-whcode-harness/tasks.md`（任务勾选）、`checklist.md`（检查点）互补：
> 本页是**方向**，账本仍是 tasks/checklist。与本页冲突时以 tasks/checklist/ADR 为准。

## 0. 目标、判据、非目标（知难行易 · 任务认知卡）

- **方向**：在**唯一验收目标 Windows 11 amd64** 不变的前提下，把 FreeBSD 硬门禁平台证据车道收口到 CI 全绿，然后按路线图深化代理工具循环，再进入 M1 校准。
- **目标**：① FreeBSD 车道（全量构建/测试/三门/冒烟）CI 全绿且失败即红；② M0 强制验收（含 Check 336 及平台证据回填）闭环；③ P0-2 工具循环（已实测去险）按 ADR 落地；④ M1 校准后转正 ADR-012/Check 134。
- **判据**：每阶段以 `tasks.md` 勾选 + `checklist.md` Check 转勾为证据；CI 结论以 GitHub Actions 日志/artifact 为准（ADR-037），本地运行只作复现。
- **非目标**：验收范围改变（仍 Windows 11 amd64，ADR-013）；性能分位数（perf 车道仍仅 Windows）；GUI 运行时证据（CI VM 无显示）；MCP 客户端/插件市场/浏览器自动化；跨会话记忆；浏览器套壳桌面 GUI（ADR-026 永不做）。
- **复审触发**：本计划每阶段结束复核一次；vmactions 镜像/端口策略变更、Actions 配额不可接受、或任一阶段连续 3 轮 CI 红灯时，回 `docs/adr/` 复查并可能修订（ADR-046 口径）。

## 1. 阶段序列

### 阶段 A — FreeBSD 硬门禁车道收口（当前，M0 强制验收前置）

| 步 | 工作 | 门禁/证据 | 状态 |
|---|---|---|---|
| A1 | FreeBSD `child_pids` 修复：**根因已定位**——`ps -ax -o pid=,ppid=` 在 NetBSD 输出**畸形单列**（run 41290577560 诊断：首行字面 `,ppid=`、裸 PID 无 PPID、仅 24 行；`=` 表头抑制逗号列表不被 NetBSD 正确处理），FreeBSD 同调用间歇不稳定。修复＝`ps -axo pid,ppid`（无 `=`）＋表头跳过解析＋失败诊断 | `platform-freebsd`＋`platform-netbsd` CI 绿 | **in-flight** |
| A2 | whcode-bin 测试二进制 **SIGSEGV**（signal 11 退出崩溃，run 41290577560 首次露出，此前 child_pids 失败中止未到）——识别崩溃测试、判 flaky/FreeBSD 特有/真 bug | 定位 + `platform-freebsd` 绿 | pending |
| A3 | 回填 Check 336 注记（run ID）＋Check 22/25/96/99/201 注记更新；T240 转勾 | `platform-freebsd` 绿证据 | gated on A1 |
| A4 | 计数声明复核（116/243/339）＋`m0-report.md` 更新 | `check_counts.py` consistent | gated on A1 |

### 阶段 B — P0-2 工具循环深化（M0 收口后主线）

| 步 | 工作 | 门禁/证据 | 状态 |
|---|---|---|---|
| B1 | tool-calling 实测写回 `docs/guides/test-model-sensenova.md`（4 场景已过；补 `required`/指定函数名、`reasoning_content` 字段探测） | 实测记录 | 前置已解除（S1） |
| B2 | 工具循环资源预算 + 白名单安全面（新 ADR；循环硬上限 + 工具白名单 + 副作用恢复语义） | ADR + 新 FR/任务/Check 登记 | pending |
| B3 | FR-406 并行子任务（Check 172 判据已备：状态与写入一致性测试通过后启用） | Check 172 | pending |

### 阶段 C — M1 校准（M0 强制验收通过后）

| 步 | 工作 | 门禁/证据 | 状态 |
|---|---|---|---|
| C1 | ADR-012 退出条件：1 MiB 级大载荷、长时写入、`temp_directory` 溢写验证、缓冲管理器直接用量证据 | ADR-012 转正 | pending |
| C2 | FR-503 批次基线校准；ADR-033 的 72h 序列复审触发 | 基线报告 | pending |
| C3 | Check 134 转正（M1 校准最后一角，M0 内不可转正是诚实状态） | Check 134 | pending |

### 阶段 D — 后续里程碑（实现已在库，待收口/验收）

| 里程碑 | 状态 | 备注 |
|---|---|---|
| M2 会话与任务、M3 LLM 上下文、M4 SDD 门禁、M5 TUI/CUI、M6 Web | 实现完成，测试在库 | 测试在库，按里程碑验收节奏 |
| M7 SSH | 服务端实现完成（russh 0.64.1，ADR-030） | 四 BSD 现场验收延后（ADR-013） |
| M8 可选执行运行时 | 实现完成，feature 默认关闭 | 按需验收 |

## 2. 横切持续项（跨阶段，外部/长时）

- **长跑 5.5h verdict**（Check 116/195/T116）：第三方长跑结果，verdict 未出，出后回填。
- **性能分位数**（NFR-201—210）：perf 车道仍仅 Windows，未取得。
- **GUI 运行时证据**：CI VM 无显示，只证编译与工作区测试通过（FreeBSD 显式缺口）。
- **Code Scanning 零告警铁律**（§5.2）：推送后复核，0 条 open 告警保持。
- **并行会话协调**：同仓共享索引——按路径提交自己的文件，`git add -A` 禁用；新 ADR 编号先 `ls docs/adr/` 认领；勿暂存并行会话的 checklist/tasks/whcode-mcp/ADR-055 改动。

## 3. 明确不做（硬边界，不得自行放宽）

- MCP 客户端、插件市场、浏览器自动化（依赖面与本地优先/最小资源哲学冲突，等真实矛盾再评估）。
- 跨会话记忆（真实矛盾答不出）。
- 浏览器套壳桌面 GUI（ADR-026 铁律：原生渲染或原生控件）。
- 32 位平台、GNU 工具链、非最新+固定 SHA 的 CI action 引用（ADR-013/D-01/§5.1）。
- 普通 Rust `unsafe`、`mem::forget`、无界容器（NFR-215）。

## 4. 与并行会话的边界

并行会话（MCP Streamable HTTP ADR-055 等）与主线并存。本计划的主线（阶段 A—C）与其 MCP 工作无依赖冲突；共享文件的计数声明以并行会话实际登记值为准（当前 FR 116／任务 243／Check 339）。任何跨阶段交接需在 `CHANGELOG.md` 登记。
