# ZCode × WHCode 差异比较研究（research/zcode-comparison/）

- **目录性质**：外部参照与调研证据（ADR-045）；**不构成 ADR 或实施承诺**；采纳任何一项需走
  WHCode G0—G5 门禁并落到 `specs/` 或 ADR。
- **ZCode 证据**：源码浅克隆于 `local/zcode-src`（**不入库**），commit
  `3c5b227649371507ddc4eb46ac5133afe6db1cab`（2026-10-20），zai-org/ZCode，Apache-2.0，
  TypeScript monorepo（Electron + Node 24 + pnpm）。引用克隆内文件时以**绝对路径**定位。
- **WHCode 证据**：本仓库 `crates/**` 源码、`specs/001-whcode-harness/`、`docs/adr/`；
  实现状态以 `tasks.md` 为权威。
- **证据层级**：两边结论均由源码直读得出（路径随文给出），符合 AGENTS.md §4.2 一级证据；
  个别无法在克隆源码中确认的点标 `[待验证]`。
- **0 信任口径**：本目录结论基于 2026-10-20 ZCode commit 快照；上游此后可能已变更，
  实施前须重读对应文件（§5 待验证项）。本目录**不自行断言 WHCode 已实现某能力**——
  那以 `tasks.md`/`checklist.md` 为准。

## 分域报告索引

| 域 | 文件 | 主题 |
|---|---|---|
| 总览 | [overview.md](overview.md) | 定位与技术栈总览、独有能力清单、待验证项 |
| A | [domain-A-agent-core-loop.md](domain-A-agent-core-loop.md) | Agent 核心循环 |
| B | [domain-B-tool-system.md](domain-B-tool-system.md) | 工具系统 |
| C | [domain-C-permission.md](domain-C-permission.md) | 权限与确认 |
| D | [domain-D-session-fork-rewind.md](domain-D-session-fork-rewind.md) | 会话分支与回滚 |
| E | [domain-E-context.md](domain-E-context.md) | 上下文管理 |
| F | [domain-F-subagent.md](domain-F-subagent.md) | 子代理 / 多智能体 |
| G | [domain-G-plan-mode.md](domain-G-plan-mode.md) | 计划模式 |
| H | [domain-H-skills.md](domain-H-skills.md) | 技能（Skills） |
| I | [domain-I-mcp.md](domain-I-mcp.md) | MCP |
| J | [domain-J-hooks-plugins.md](domain-J-hooks-plugins.md) | Hooks 与插件 |
| K | [domain-K-llm-provider.md](domain-K-llm-provider.md) | LLM Provider 层 |
| L | [domain-L-persistence-events.md](domain-L-persistence-events.md) | 持久化与事件 |
| M | [domain-M-frontend.md](domain-M-frontend.md) | 前端 |
| N | [domain-N-security.md](domain-N-security.md) | 安全 |
| O | [domain-O-remote-collab.md](domain-O-remote-collab.md) | 远程、协作与部署 |
| P | [domain-P-engineering-process.md](domain-P-engineering-process.md) | 工程流程与门禁 |
| 行动 | [actionable-gaps.md](actionable-gaps.md) | 可补齐清单 P0—P3（行动指引） |

## 修订记录

- **2026-10-28（首版入库）**：原差异书 `local/diff-zcode-vs-whcode.md` 按域拆分并**深度修订**。
  16 个并行源码直读子代理分域重读 ZCode（8234 TS 文件）与 WHCode 源码，逐域核实原判定、
  修正失效参照路径、验证存疑项。相对原稿的**结论修正**集中在：域 B（WHCode 非“”，
  store.tool_exec 已建模待接线）、域 C（兜底非 alwaysAsk，ZCode 有 escalate/modify 输入改写）、
  域 D（回滚细分对话剪枝+文件恢复两条路径）、域 E（cacheHint 只用于排序遥测，实际统一
  ephemeral）、域 F（watchdog 超时终止非自动后台化）、域 H（skills-lock 无消费者确认，
  两份锁文件不同）、域 I（WHCode 全仓无 MCP）、域 J（插件路径修正+trust 审批流）、
  域 K（真实路径+取消语义不弱+重试递增窗口）、域 L（迁移编号 0001—0023）、域 M（tui 是
  缺省参数）、域 N（鉴权 token 实为 ZCODE_SERVER_AUTH_TOKEN+无 Origin 校验）、
  域 O（daemon 为 server-cli.js serve fork server-core.js）、域 P（CI 是 husky pre-push，
  reviewRequired 19）。详见各域报告“”节。
