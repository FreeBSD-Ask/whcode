# 可补齐能力清单（P0—P3 行动指引）

> 本文基于 16 域分域报告（`domain-*.md`）交叉归纳，把 ZCode 相对 WHCode 的独有能力
> 按 P0（项目成立要件）—P3（可选增强）分级，并给出 WHCode 侧建议的落点与 ADR 要求。
> 结论性质：**研究指引**，不是验收承诺；任何一项落地都必须先立 ADR 再改代码
> （AGENTS.md §10：设计冲突先 ADR）。

## P0：项目成立要件（不做则其余失去挂载点）

| 能力 | ZCode 证据 | WHCode 现状 | 建议落点 | ADR 要求 |
|---|---|---|---|---|
| Agent turn 循环 | `turn-loop.ts` `runRegularTurnLoop`：中止/预算守卫→消息组装→模型 step→工具执行回填→无工具调用即终止 | `llm.rs` 单次调用 | `whcode-app/turn.rs` 编排模块 | **已立 ADR-044**（T204—T212、Check 300—308） |
| 工具系统 | `tool/registry.ts` + `scheduler.ts`（DEFAULT_MAX_CONCURRENCY=10）；输入校验三层/diff/patch/后台任务 | `tools/src/lib.rs` 仅 62 行声明；`store/tool_exec.rs` 已建模待接线 | P0-2：工具实现 + 真实调度替换“”路径 | 另立 ADR（挂 ADR-044 决策 6） |

## P1：核心增强（补齐后显著改善代理能力）

| 能力 | ZCode 证据 | WHCode 现状 | 建议落点 | ADR 要求 |
|---|---|---|---|---|
| 上下文压缩 | `context/compact/policy.ts`：完整压缩执行（手动/自动/响应式 + microcompact + 摘要回填 + context-usage-log-compact）；`DEFAULT_COMPACT_CONTEXT_WINDOW=200_000` | `context.rs` `compress_context` 只记摘要指针，摘要生成未接线 | P1-3：自动压缩接线 | 挂 ADR-044 决策 2 |
| 子代理 | `runtime/methods/subagent.ts` + `subagent/runner.ts` + `tool-event-mirror.ts`；watchdog 超时终止 + autoBackgroundMs timer | 无子代理 | `whcode-app/subagent.rs` | 另立 ADR |
| 技能系统 | `adapters/src/skills/index.ts` + `context/sections/skills.ts`；${ZCODE_SKILL_DIR} 替换 | 无技能系统（AGENTS.md 为代理协作入口，无 skills 目录） | `whcode-tools`/`whcode-app` 技能加载 | 另立 ADR |
| MCP | `adapters/src/mcp/index.ts` + `core/src/mcp/index.ts`：SDK + 三 transport + OAuth + 进程树/Job Object + 连接池 + 图片归一化 + CUA fail-closed；`MCP_TOOL_TIMEOUT_MS=30_000` | **全仓无 MCP 引用** | `whcode-mcp` 新 crate | 另立 ADR（新 crate 需先 ADR，AGENTS.md §4） |
| 重试 | `retry-policy.ts`：maxRetries=10、baseDelay=2000ms、backoff×2、maxDelay=60s、jitter、Retry-After 优先、空补全重试、stream-idle 递增 30s | 仅幂等 GET 重试 2 次；生成不重试 | `whcode-llm` 重试策略 | 另立 ADR（LLM 层变更） |

## P2：增强（可选，视里程碑排期）

| 能力 | ZCode 证据 | WHCode 现状 | 建议落点 | ADR 要求 |
|---|---|---|---|---|
| 插件/钩子 | `contracts/src/hooks/index.ts` + `services/src/plugins/`：matcher 正则 + command/process 双模式 + workspace 信任审批流（digest+policy+revocation） | 无插件系统 | 视 M8+ 排期 | 另立 ADR |
| 权限分级 | `permission/service.ts` + `broker.ts`：四态 PermissionDecision + build 模式分级 + escalate/modify 输入改写 + WebFetch 域名预批准 + workflow 草稿免确认 | `authz.rs` Capability 11 项（静态） | 动态权限 broker | 另立 ADR |
| 远程协作 | 根 `packages/server/` + `packages/zcode-server-cli/`：R3 核心 + Legacy、会话分享（canonical JSON SHA-256）、Relay V2（JWT+60s）、daemon fork+断连自退+OS 服务注册、分阶段发布 | SSH 客户端原型（M7 范围） | 视 M7+ 排期 | 另立 ADR（平台/安全类必须用户批准） |
| 备份/恢复 | `services/src/backup.ts`：SHA-256 校验和 + 自动预迁移备份 + restore 脚本 | 无备份/恢复命令 | `whcode-app` 运维命令 | 另立 ADR |
| 会话回滚 | `runtime/methods/session-fork.ts` + `rewind-message.ts` + `file-rewind.ts`：对话剪枝 + 工作区文件恢复（完整 beforeContent+structuredPatch） | 崩溃恢复（recover_uncertain）单事务幂等 | 回滚细化 | 另立 ADR |

## P3：可选增强（低优先级，仅记录不排期）

| 能力 | ZCode 证据 | WHCode 现状 | 建议 |
|---|---|---|---|
| 扫码登录/OAuth/分享 | 桌面内嵌 OAuth + Web 扫码 + 会话分享 | 无 | 仅当引入 Web 服务端时评估 |
| 预览 | CDP + Playwright 开发态预览 | 无预览机制 | 开发态工具，低优先级 |
| 配置分层 | `provider/resolver.ts` 4 层合并 + model-option-map CEL 表达式 | `http/profile.rs` 静态档 | 若多供应商则评估 |
| 凭据加密落库 | `credential-cipher.ts` aes-256-gcm + `enc:v1:` 前缀 | 仅本地文件不落库 | **不采纳**（WHCode 靠不落库更简单，落库加密需先立 ADR） |

## 防御性反例（ZCode 缺失、WHCode 应避免）

| 反例 | ZCode 证据 | WHCode 建议 |
|---|---|---|
| 非恒定时间鉴权 | Legacy server 用 `===` 比较 token（仅 R3 用 timingSafeEqual） | 若引入 token 鉴权必须恒定时间比较 |
| 无 Origin/CSRF 校验 | 服务器只验 Bearer token，无 Origin/scheme+host 校验 | Web 服务端默认加 Origin 校验（安全反教条） |
| 无 CI 强制 | CI 仅 husky pre-push（本地可 `--no-verify` 绕过） | WHCode 维持 ADR-037（CI 唯一验收入口） |
| 无 ADR/追溯 | 无 ADR、无三方追溯、无 action 固定 SHA | WHCode 维持 ADR 体系 + traceability + 固定 SHA |

## 归纳

- **WHCode 独有（ZCode 所缺）**：ADR 决策体系、三方追溯（FR↔任务↔Check）、action 固定 SHA、
  CI 唯一验收入口（ADR-037）、原生 GUI 铁律（ADR-026）、零 unsafe 铁律、单一 schema 版本出处、
  崩溃恢复单事务幂等、精确主题色值、资源铁律（线程/句柄/内存）。
- **ZCode 独有（WHCode 所缺）**：Agent turn 循环、完整工具系统、上下文压缩执行、子代理、
  技能系统、MCP、流式生成重试、插件/钩子、动态权限 broker、远程协作全套、备份/恢复、
  会话回滚、配置分层、凭据加密落库。
- **判定修正（相对原差异书）**：ZCode 取消语义不弱于 WHCode（域 K）；ZCode 脱敏成体系
  不逊于 WHCode（域 K）；ZCode 迁移编号为两套序列非“”（域 L）；ZCode CI 是 husky
  非 GitHub Actions（域 P）；ZCode 插件/hooks 非薄层（域 J）；ZCode 兜底权限为 build 模式
  分级非 alwaysAsk（域 C）。
