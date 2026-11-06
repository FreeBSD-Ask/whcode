# 总览：DeepSeek Harness × WHCode 定位与技术栈

> 本文件是 `research/dsh-comparison/` 的总览篇。分域深度报告见 `domain-{A..S}-*.md`。

## 1. 定位与技术栈

| 维度 | DeepSeek Harness (DSH) | WHCode |
|---|---|---|
| 一句话定位 | “一切皆插件”的开源 agent harness：运行期组装能力，产品面覆盖 Web/Desktop/CLI/SDK/ACP | Rust 原生、本地优先的编码 Harness：事件可追踪、规格驱动门禁、资源有界 |
| 语言/运行时 | TypeScript（ESM），Node ≥ 24.9，pnpm 11，Cordis 插件框架（vendored） | Rust 1.99.1，Cargo workspace 15 crate，零 unsafe 铁律 |
| 版本/许可 | `0.2.2-rc.1`，MIT，developer preview（明示破坏性变更） | `0.1.0`（M0 基线），BSD-2-Clause |
| 仓库规模 | 约 50 个包组、`packages/client` 单组 2039 个 TS 文件、187 MB 浅克隆 | 15 crate，单文件 DuckDB，无 Node/Electron 依赖 |
| 组合方式 | profile → 有序 bundle → patch 层 → `--patch`；按 id 整行替换；HMR | 编译期 crate 分层，单向依赖由契约测试强制；新 crate 需 ADR |
| 执行单元 | turn ⊃ step；事件三域（session/agent/capability）；五/七步流水线 | turn 循环 + 六种终止原因；事件表 + 消息表；同步、有界 |
| 存储 | JSONL 代际文件（`session.vN.jsonl[.zstd]`）+ session-query SQLite FTS | DuckDB 14 表 schema v7，幂等键 + FNV 校验和 + VARIANT 列 |
| 前端 | React 浏览器插件图 + Electron + CLI；无公开资源预算 | 五适配器（TUI/CUI/Web/GUI/SSH）；JS ≤ 50 KB、DOM ≤ 2000、GUI 空闲 RSS ≤ 96 MB |
| 模型接入 | `ctx.llm` seam + DeepSeek/pi-ai adapter；重试是独立插件 | 自研 5 协议（openai/anthropic/gemini/ollama/cohere）+ 流式重试 decorator |
| 安全姿态 | 默认开放执行/插件/网络；`SAFETY.md` 声明未审计；沙箱三模式可上报 `partial` | 默认回环、redacted、最小权限；无沙箱；审批 broker 待做（T255） |
| 治理 | 覆盖率/基准/快照/文档生成门禁；决策以 Agent Note 归档 | FR↔任务↔Check 三方追溯、ADR 体系、CI 唯一验收、action 固定 SHA |

## 2. 架构分歧（本研究的核心判断）

两者不是同一物种的两个版本，而是**两种组合哲学**：

1. **运行期可组合 vs 编译期可证明**（域 J）。DSH 把模型适配器、工具注册表、会话日志、agent loop
   都做成插件，靠“可逆效果”在卸载时回卷；WHCode 把依赖边界写进 Cargo 与契约测试。
   DSH 能现场换实现，WHCode 能静态证明“前端不触库、平台差异只在 `whcode-platform`”。
   两者的失效条件也相反：DSH 怕插件膨胀与组合不可诊断，WHCode 怕能力增长时被编译期结构卡住。

2. **边界即事实 vs 边界即代码**（域 A/D/E）。DSH 把 `turn/start`、`step/start`、失败尝试
   （`assistant/attempt`）、压缩锁、分叉种子全落日志，因此“崩溃后能判定”；WHCode 把 turn
   边界与迭代事件同批提交（`append_turn`），更省写但边界在崩溃时不可见。

3. **一次性凭证 vs 命令级能力**（域 C）。DSH 的审批回答“这一次动作能否做”（`allowed-once`），
   无 answerer 即 `unavailable`；WHCode 的 `Capability` 回答“这个 actor 能否用这个命令”。
   前者细到调用，后者可静态证明。

4. **语义压缩 vs 结构折叠**（域 E）。DSH 用模型生成摘要并记录 provider/model/usage，
   `compaction/start|summary|end` 三段可审计；WHCode 的摘要只是“折叠了 N 条消息”的程序文本，
   `summary_ref` 是指针。这是**当前最大的能力差**。

5. **自带执行世界 vs 可选受控运行时**（域 Q）。DSH 的 subprocess/shell/PTY/LSP/PTC 是产品主线，
   FS 与进程共享命名空间，SSH 可整体搬走；WHCode 的运行时默认不启用（`default = []`），
   能力等级 `Full/Restricted/ShellOnly` 必须可见。

## 3. 独有能力清单

**DSH 独有**（WHCode 无对应物，详见各域）：

1. Cordis 插件树 + profile/bundle/patch/HMR/`isolate` realm（J）
2. 单 inbox 四档投喂（`send/followup/steer/inject`）+ `agent/pre-step` 可重写准入（A）
3. 会话分叉 API（精确 seq 边界、拒绝半截 turn、`inheritedEventCount`）+ 代际迁移（D）
4. 模型生成摘要的压缩 + 锁 + 图像卸载 + `surfaceOp replace`（E）
5. 子代理 provider 注册表（本地/外部统一）、`delivery` 两档、命名寻址、Team 任务板（F）
6. 计划模式（软引导 + `exit_plan_mode` 人复核）、Goal CAS、todo 整表、宿主级七种定时（G）
7. 技能 provider 注册表 + 不完整观察语义；extensions 运行时检视与沙箱化动态插件（H）
8. MCP resource/instructions + scope 可见性；SDK/ACP/类型驱动 API 网关（I）
9. SSH 执行世界迁移（FS/Bash/PTY/LSP/PTC 同行）；LiveShare 授权根＋工具替换（O）
10. Webhook 外部投递建会话（O）
11. shell/PTY/LSP/workflow/PTC/jobs 执行族（Q）
12. 浏览器操作 provider 三选一、桌面操作 CUA 权限面（R）
13. 内容寻址附件“先持久化再引用”、Office→PDF、语音输入（R）
14. per-file 100% 覆盖率门禁、PR 级基准、快照回放、文档生成与新鲜度校验（P）
15. OpenTelemetry 遥测通道与产品遥测（opt-in）（L/N）

**WHCode 独有**（DSH 无对应物）：

1. 规格驱动门禁：FR↔任务↔Check 三方追溯、计数声明、知难行易门禁（P）
2. ADR 体系（001—058）与绑定命令式复审触发（P）
3. 单文件 DuckDB 14 表 + 幂等键 + FNV 校验和 + VARIANT JSON 列口径（L）
4. 审计表 11 类动作、失败阻断、不随清理删除（N）
5. 检查点保留 10 条 + `recover_uncertain` + 崩溃恢复（D/L）
6. 资源铁律与实测数值（RSS/线程/句柄/CPU、5.5 h 斜率、GUI 预算）（S）
7. 原生 GUI（iced＋tiny-skia），禁止浏览器套壳（M）
8. 五协议内置 wire 实现（openai/anthropic/gemini/ollama/cohere）（K）
9. 六套主题精确色值与 `theme_tokens` 单一事实源（M）
10. SSH TOFU + 命令白名单 + 默认拒 Shell（O）
11. Web 安全默认：Origin 校验、恒定时间 Bearer、token 不经 URL（N）
12. 前端预算脚本（JS/CSS/DOM）与 DOM 注入 API 禁用（M）
13. CI action 固定 SHA + 每周复核 + Code Scanning 零告警（P）
14. CUI JSON 信封 + dry-run/确认语义 + 四端对等 `SurfaceEntry`

## 4. 能力矩阵（摘要）

图例：✅完整；◑部分/待接线；❌无。判定列见各域。

| # | 能力 | DSH | WHCode | 判定 |
|---|---|---|---|---|
| A1 | turn/step 循环 | ✅ | ✅ | 同构 |
| A2 | loop 可替换 | ✅ | ❌ | DSH 领先 |
| A3 | 中途投喂/步前拦截 | ✅ | ❌ | DSH 领先 |
| A4 | 终止原因枚举 | ◑（turn/end reason） | ✅（六种） | WHCode 领先 |
| B1 | 工具注册/校验/截断 | ✅ | ✅ | 同构 |
| B2 | 并行批/超时/守卫 | ✅ | ❌ | DSH 领先 |
| B3 | 交付物 | ✅ | ❌ | DSH 领先 |
| C1 | 工具级审批 | ✅ | ❌（T255 未做） | DSH 领先 |
| C2 | 进程沙箱 | ✅ | ❌ | DSH 领先 |
| C3 | 审计持久化 | ❌ | ✅ | WHCode 领先 |
| D1 | 会话分叉 | ✅ | ❌ | DSH 领先 |
| D2 | 检查点有界保留 | ❌ | ✅ | WHCode 领先 |
| E1 | 上下文预算 | ✅（按窗口） | ✅（固定 128k） | 同构 |
| E2 | 模型摘要压缩 | ✅ | ❌（仅结构折叠） | DSH 领先 |
| F1 | 子代理 | ✅（可外部） | ✅（同步嵌套） | DSH 领先 |
| F2 | 硬上限 | ✅（默认深度 1／每树 8） | ✅（深度 1／每 turn 2） | 同构；WHCode 更严 |
| G1 | 计划/目标/待办/定时 | ✅ | ❌ | DSH 领先 |
| H1 | 技能 | ✅ | ✅ | 同构 |
| H2 | 运行时自改 | ✅ | ❌ | DSH 领先 |
| I1 | MCP | ✅ | ✅ | 同构 |
| I2 | SDK/ACP/API 网关 | ✅ | ❌ | DSH 领先 |
| J1 | 运行期插件组合 | ✅ | ❌ | DSH 领先 |
| J2 | 编译期依赖证明 | ❌ | ✅ | WHCode 领先 |
| K1 | 协议覆盖 | ◑（DeepSeek/pi-ai） | ✅（五协议） | WHCode 领先 |
| K2 | 重试语义 | ✅（可配/落日志） | ✅（有界/首 chunk 前） | 同构 |
| L1 | 事件溯源 | ✅ | ✅ | 同构 |
| L2 | 全文检索/三态分类 | ✅ | ❌ | DSH 领先 |
| L3 | 备份/恢复 | ❌ | ✅ | WHCode 领先 |
| M1 | 多前端 | ✅ | ✅ | 同构 |
| M2 | 原生 GUI/资源预算 | ❌ | ✅ | WHCode 领先 |
| N1 | 默认安全 | ❌（声明风险） | ✅ | WHCode 领先 |
| N2 | 凭据引用化 | ✅ | ◑ | DSH 领先 |
| O1 | 远程执行世界 | ✅ | ❌ | DSH 领先 |
| O2 | 会话分享 | ✅ | ❌ | DSH 领先 |
| P1 | 覆盖率/基准门禁 | ✅ | ❌ | DSH 领先 |
| P2 | 规格追溯/ADR | ❌ | ✅ | WHCode 领先 |
| Q1 | Shell/PTY/LSP/工作流 | ✅ | ❌（运行时可选） | DSH 领先 |
| R1 | 浏览器/桌面/附件/语音 | ✅ | ❌ | DSH 领先 |
| S1 | 资源预算与实测 | ❌ | ✅ | WHCode 领先 |
| S2 | 长时稳定性门禁 | ❌ | ✅ | WHCode 领先 |

**计数**：矩阵 40 行——DSH 明确领先 20 行、WHCode 明确领先 11 行、同构 9 行
（同构行中 F2 的硬上限 WHCode 更严）。完整证据与判定理由见各域 §1。

## 5. 交叉发现（十条）

1. **组合哲学决定其余一切**：DSH 的运行期插件树带来能力可换、组合可现场改，也带来
   “启动失败分级、版本协商、配置层叠加”这一整套复杂度和可诊断性负担（域 J）；WHCode 的
   编译期分层带来可证明的依赖边界，代价是每次能力增长都要动 ADR 与 Cargo。没有免费的一侧。
2. **“边界即事实”是 DSH 最强的工程习惯**：turn/step 边界、失败尝试、压缩锁、分叉种子都落日志
   （域 A/D/E）。WHCode 的事件表已具备同等承载力，差的是“先落边界再做事”的次序。
3. **审批粒度是安全模型的分水岭**：DSH 的 `allowed-once`＋`unavailable` 让“没人可问”成为
   显式状态；WHCode 的二值拒绝把“拒绝”与“无人可问”混同（域 C）。这是 T255 最该先修的类型问题。
4. **压缩是当前最大能力差**：WHCode 的摘要不含语义（域 E）；长会话下模型会丢失任务状态。
   这是唯一建议“尽快补”的能力缺口，且有清晰的最小实现路径（一次模型调用＋锁＋失败回退）。
5. **有界性两种做法**：WHCode 统一硬上限（迭代 16、工具输出 64 KiB、MCP 服务器 8、子代理 2），
   DSH 分散到各子系统配置（spill `maxInlineTokens`、并行 10、重试 always）。前者可判定，
   后者可调优；WHCode 不应放弃统一口径。
6. **可替换性与可证明性不可兼得**：DSH 的 `agent-loop` 可换（域 A），但“哪个 loop 在跑”只能
   从配置读；WHCode 只有一个 `run_turn`，但能静态证明四端同源。若 WHCode 要可替换，应做成
   trait + 编译期实现，而不是运行期插件。
7. **前端是成本观的分水岭**：DSH 的 Web 是 2039 个文件的 SPA 插件图，WHCode 的 Web 预算是
   50 KB JS + 2000 DOM（域 M）。两者都能用，但后者的可验证性来自预算，前者的能力来自工程规模。
8. **安全姿态是“默认开放＋声明” vs “默认关闭＋可判定”**：DSH 的 `SAFETY.md` 是责任声明，
   WHCode 的回环/TOFU/恒定时间/默认拒 Shell 是设计约束（域 N/O）。对本地工具，后者更符合
   “不制造全知幻觉”的项目哲学。
9. **工程门禁方向相反**：DSH 校验“代码与文档一致”（生成目录、README 契约、覆盖率、基准），
   WHCode 校验“规格与任务一致”（追溯、计数、门禁）。两者都需要，但 WHCode 缺的是
   “代码→文档”的防腐化，DSH 缺的是“规格→证据”的可追溯。
10. **资源纪律不能靠生态惯性**：DSH 没有公开 RSS/线程预算，靠基准与内存压力车道发现退化
    （域 S）；WHCode 有数值、有样本、有 5.5 h 斜率。后者是更可持续的做法。

## 6. 待验证项与证据缺口

1. DSH `Turn flow` 的多条语义（`prepareCall` 取消提交、`startsRequestSeries`）来自
   `docs/architecture.md` 与 `agent.ts` 直读，但未逐条跑通；属“源码可读但未执行验证”。
2. DSH 的默认 `maxDepth`、`maxInlineTokens`、MCP 启动失败策略、HMR 粒度、`isolate` 边界
   在各域 §5 列出，均未取得最终值。
3. DSH 的 Web/Desktop 资源预算（RSS/线程）未在文档主轴找到，不能断言“DSH 无预算”，
   只能写“未见公开值”。
4. WHCode 侧若干“缺失”结论以检索为据（`sandbox`/`approval`/`spill`/`fork` 等零命中），
   检索范围是 `crates/`；`specs/` 中可能有计划但未实现，二者在分域报告里分开写。
5. WHCode `tasks.md` 与磁盘存在少量不一致（如 T251 勾选状态），本目录一律以磁盘代码为准，
   并在相关域注明。
6. DSH 上游迭代极快（0.2.2-rc.1，README 明示破坏性变更）；任何实施前须重读对应文件，
   `[待验证]` 项须先取得证据。

## 7. 证据层级说明

- 一级证据：DSH 源码（`packages/**`）、WHCode 源码（`crates/**`）与测试；
- 二级证据：DSH `docs/**`（团队撰写，可能滞后）、WHCode `specs/**`、`docs/adr/**`；
- 引用口径：源码给 `文件:行号`；Markdown 只给文件路径（上游文档增删频繁，行号易失效），
  精确定位按文中节标题在该文件内检索；
- 两者冲突时以源码为准，并在分域报告 §2/§3 写明冲突点。
- 本目录所有“DSH 领先/WHCode 领先”判定都不是价值判断，只描述**该域机制完整度**；
  是否采纳见 [actionable-gaps.md](actionable-gaps.md)，且必须过 WHCode 的资源/安全/BSD 约束。
