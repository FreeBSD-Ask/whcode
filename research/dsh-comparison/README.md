# DeepSeek Harness × WHCode 功能比较研究（research/dsh-comparison/）

- **目录性质**：外部参照与调研证据（ADR-045）；**不构成 ADR 或实施承诺**。采纳任何一项需走
  WHCode G0—G5 门禁并落到 `specs/` 或 ADR。
- **DSH 证据**：源码浅克隆于 `local/dsh-src`（**不入库**），commit
  `fcc0ad462172522aaf5c93a7b2e01f6b9f0b0159`（2026-11-04），deepseek-ai/deepseek-harness，
  MIT，版本 `0.2.2-rc.1`，TypeScript pnpm monorepo（Node ≥ 24.9），基于 Cordis 全插件框架，
  developer preview（上游明示会有破坏性变更）。引用以相对克隆根的 `path:line` 定位。
- **WHCode 证据**：本仓库 `crates/**`、`specs/001-whcode-harness/`、`docs/adr/`；
  实现状态以 `tasks.md` 勾选 + 磁盘代码为准（两者不一致时以代码为准并登记）。
- **证据层级**：两侧结论均由源码/文档直读得出（路径随文给出），属 AGENTS.md §4.2 一级/二级证据；
  无法确认处标 `[待验证]` 并写取证方式。
- **引用口径**：源码引用给 `文件:行号`（行号经检索核对）；Markdown 引用只给文件路径
  （上游文档会增删段落，行号易失效），需要精确定位时按文中节标题在该文件内检索。
- **0 信任口径**：本目录结论基于 2026-11-04 DSH commit 快照；上游迭代极快（0.2.2-rc.1），
  实施前须重读对应文件。本目录**不自行断言 WHCode 已实现某能力**——那以 `tasks.md`/
  `checklist.md` 为准。

## 分域报告索引

| 域 | 文件 | 主题 |
|---|---|---|
| 总览 | [overview.md](overview.md) | 定位与技术栈、独有能力清单、交叉发现、待验证项 |
| A | [domain-A-agent-core-loop.md](domain-A-agent-core-loop.md) | Agent 核心循环（turn/step/inbox/取消/重试） |
| B | [domain-B-tool-system.md](domain-B-tool-system.md) | 工具系统（注册/流水线/守卫/溢出/交付物） |
| C | [domain-C-permission-approval-sandbox.md](domain-C-permission-approval-sandbox.md) | 权限、审批与沙箱 |
| D | [domain-D-session-fork-resume.md](domain-D-session-fork-resume.md) | 会话分叉、恢复与回滚 |
| E | [domain-E-context-compaction.md](domain-E-context-compaction.md) | 上下文管理与压缩 |
| F | [domain-F-subagent-team.md](domain-F-subagent-team.md) | 子代理与 Team |
| G | [domain-G-plan-goal-todo-schedule.md](domain-G-plan-goal-todo-schedule.md) | 计划、目标、待办与定时 |
| H | [domain-H-skill-extensions.md](domain-H-skill-extensions.md) | 技能与运行时自改 |
| I | [domain-I-mcp-sdk-acp.md](domain-I-mcp-sdk-acp.md) | MCP、SDK、ACP、API 网关 |
| J | [domain-J-plugin-architecture.md](domain-J-plugin-architecture.md) | 插件架构：Cordis／profile／bundle／patch／HMR |
| K | [domain-K-llm-provider.md](domain-K-llm-provider.md) | LLM Provider 层 |
| L | [domain-L-persistence-events.md](domain-L-persistence-events.md) | 持久化与事件 |
| M | [domain-M-frontend.md](domain-M-frontend.md) | 前端：Web／桌面／CLI |
| N | [domain-N-security.md](domain-N-security.md) | 安全与凭据 |
| O | [domain-O-remote-collab.md](domain-O-remote-collab.md) | 远程、协作与分享 |
| P | [domain-P-engineering-process.md](domain-P-engineering-process.md) | 工程流程与门禁 |
| Q | [domain-Q-execution-environment.md](domain-Q-execution-environment.md) | 执行环境：Shell／子进程／PTY／LSP／PTC／Workflow／Jobs |
| R | [domain-R-device-capabilities.md](domain-R-device-capabilities.md) | 设备与多媒体：浏览器／桌面／附件／Office／语音 |
| S | [domain-S-resource-performance.md](domain-S-resource-performance.md) | 资源纪律与性能 |
| 行动 | [actionable-gaps.md](actionable-gaps.md) | 可补齐清单 P0—P3 与反例清单 |

## 读法建议

1. 先读 [overview.md](overview.md) 的第 1—2 节（定位与独有能力），建立“两者不是竞品”的判断。
2. 按 WHCode 当前里程碑选域：执行侧看 A/B/C/E/F/Q；体验侧看 G/H/M；治理侧看 L/N/P/S。
3. 任何“可借鉴”项先读该域 §4 的“代价/风险”与 §5 的待验证项，再看
   [actionable-gaps.md](actionable-gaps.md) 的优先级与 ADR 要求。

## 与既有研究的关系

- `research/zcode-comparison/`（ZCode，16 域）与 `research/better-harness/`（评审系统）已综合为
  `research/synthesis/`。本目录是**第三个执行侧参照**，与二者可三方对读：
  ZCode 给“功能广度”，DSH 给“架构可组合性”，BH 给“评审纪律”，WHCode 给“门禁与资源纪律”。
- 与 `research/synthesis/` 的判定不冲突：DSH 在“运行期可组合”上比 ZCode 更彻底，在
  “规格驱动门禁”上完全空白，与 synthesis 的“三层互补”判断一致。

## 修订记录

- **2026-11-06（首版入库）**：DSH 0.2.2-rc.1（commit `fcc0ad4`）源码直读，19 域报告 +
  overview + actionable-gaps。相对 ZCode 研究的**新增判定**集中在：域 J（运行期插件树 vs
  编译期分层是两者最根本的架构分歧）、域 E（DSH 摘要由模型生成且带锁，WHCode 只是结构折叠）、
  域 C（DSH 审批是“一次性凭证”，WHCode 是“命令级能力”）、域 P（DSH 覆盖率与基准进 PR，
  WHCode 追溯与计数进 CI）。
