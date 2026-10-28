# Better Harness 批判与融合方案（用 WHCode 工程理论）

> 本文以 `docs/guides/philosophy.md`、`engineering-doctrine/`、AGENTS.md §1.1—§1.4 的
> 工程理论对 Better Harness（下称 BH，证据 commit `b605532` = `0.7.0-alpha2`，见
> [README.md](README.md)）做批判，并给出 P0—P3 融合方案。结论性质：**研究指引**，
> 不是验收承诺；任何一项落地必须先立 ADR 再改代码（AGENTS.md §10）。

## 1. 逐条批判

### 1.1 形式主义：它解决什么真实矛盾？

BH 的自我诊断成立：AI 编码代理"快速改代码，但周边 workflow 是弱点"——模糊目标、
即兴路径、"能跑"无证明、速度压倒保障、教训流失（`README.md` "Why Better Harness?"）。
这与 WHCode 的真实痛点对应：`checklist.md` 的 Check 大多登记的是**机制存在级**证据
（"有测试""有脚本"），缺少"被关联任务使用过""后期可比结果支持效果"的分级。
**结论：真实矛盾成立，值得吸收。**

但两处过度形式化：

- **分数的假精度**。59/74/84/94/100 的五档天花板数字携带了它无法兑现的精度承诺——
  模型自己承认"是天花板不是公式"（`models/agent-work-loop.md:120`）、README 承认
  "显示趋势，不是改进的因果证明"。按山洞哲学（AGENTS.md §1.3.3），这制造局部全知
  幻觉。**WHCode 若吸收，采纳证据五态，拒绝分数**；如需排序，用三档粗分级
  （缺口/已接线/已验证）即可。
- **模型文档规模**。819 行评估模型 + 15 检查 + 10 决策门 + 8 质量门 + 66 篇 references
  构成一套需要专门训练才能一致执行的评审语言。对 BH（多宿主、面向公众）是合理投资；
  对 WHCode（单人 + 多代理协作）应**裁剪为最小可用子集**（见 §3 P0），不为对齐而对齐。

### 1.2 门禁程序化冲突（最关键的批判）

WHCode 铁律（AGENTS.md §5）："**门禁判定必须由程序实施（测试/脚本），不得以提示词
或人工口头结论代替**"。BH 的核心判定链——五维评分、发现筛选、严重度、verified 判定、
support track 选择——全部是 LLM 代理判断。BH 自己划了边界（确定性收集器 vs 代理评审，
且 source-review 命令"不代写 decision、不调用模型"，`report-source/cli.mjs:17-33`），
但读者面上的 Loop Effectiveness 分数仍呈现为"效果度量"。

**融合铁律**：BH 机制进入 WHCode 时按可判定性切分——

- **可判定部分程序化**（可成为门禁/Check 证据）：证据收集、契约校验、文档链接图、
  密钥扫描、乐观锁、覆盖率原因码输出；
- **不可判定部分只作建议**（永不进入 Check 勾选或门禁判定）：发现筛选、严重度、
  证据态升级判定、verified/partial/blocked 复评。LLM 复评结论在 WHCode 只能登记为
  "代理声明 + 证据引用"，最终判定仍是程序或人。

### 1.3 正负和解

任何吸收都有代价，先写明承担者：

| 吸收项 | 正面 | 负面 | 承担者 | 和解理由 |
|---|---|---|---|---|
| Check 证据五态 | 缺口从"未通过"细化为六种可判定状态，防"配置存在即通过" | checklist.md 全量改语义 + traceability/check_counts 脚本同步，一次性迁移成本 | 维护者（一次） | 三方追溯已是本仓门禁，语义升级复用同一批脚本 |
| 评审协议（独立证据域→调和→冻结） | 里程碑评审盲区可见，避免 lead 一手遮天 | 一次里程碑评审 ≈ 4—5 次模型调用（3 收集 + 调和 + 可选复评），不可复现 | 用户（token/时间） | 评审是里程碑级低频操作，且协议强制"不可判定→Unobserved"抑制编造 |
| 确定性证据收集器 | 评审输入可复现、可审计、离线 | 新增命令面维护成本 | 维护者 | 与 doctor/bench 同类的运维命令，落 whcode-bin 不新增常驻资源 |
| 干预台账 | 学习跨里程碑连续，防"改了就忘" | 新增持久化状态需生命周期管理 | 维护者 | 落 DuckDB events 表自动获得幂等键/审计/清理策略，不新造存储 |

### 1.4 三重时间

- **过去来源**：BH 五维各锚定一手来源并声明"来源解释形状，不冻结术语与分数"
  （`agent-work-loop-rationale.md:52-56`）——比多数项目的"最佳实践"引用严谨，
  值得 WHCode 在 ADR/checklist 论证中效仿（引一手 + 声明适用边界）。
- **现在证据**：证据五态本身就是"现在证据"的分级语言。
- **将来失效**：BH 把复审触发**机制化**了——`stale-or-conflicting-asset` 模式要求
  "命名资产 + 稳定检查触发器 + 规范真值 + 可作出 clean/gap 判定的有界维护循环"
  （`learning-loop-patterns.md`、`loop-discovery.md`）。WHCode 现行做法是在文档写
  复审日期（自反文档），BH 的做法是**复审条件绑定可执行检查**。
  **融合点：ADR/Check 的复审条款从"写日期"升级为"绑定命令 + 判定规则"，可执行检查
  失效即触发复审。**

### 1.5 0 信任

强项（值得保持）：`Unobserved` 显式且"不可判定≠Missing"（`agent-work-loop.md:105-108`）、
收集器只读、敏感字段默认脱敏（禁止把 prompt/会话 id/密钥写入读者输出，`findings-review.md`
门 5）、负面控制防误报、coverage 原因码"即使无候选也输出"。

弱项（WHCode 融合时须补）：

1. **lead 剪裁不可审计**：主代理决定给每个子代理看什么（"给 Agent 1 只给 facts 信封"），
   剪裁行为本身无痕迹。WHCode 落地时，输入清单应随事件落库（events 表），剪裁可追溯。
2. **干预台账无完整性保护**：`interventionLedger` 存宿主自有 JSON，无校验和/幂等键；
   WHCode 落 DuckDB events 表（幂等键 + IntegrityFlags）自动强于此。
3. **会话源信任宿主本地文件**：14 宿主 JSONL 解析无内容校验。WHCode 只需解析自己的
   会话格式（自有 schema + 校验和），攻击面小得多。

### 1.6 信号链完备（同构确认）

`docs/guides/signal-chain-completeness.md` 的核心命题——"交付 = 输出包含可归因信号；
null/空/零是歧义证据，须区分正确的空与错误的空"——在 BH 中有三处机制化对应：

- "配置的能力不是被观测的使用"（证据五态的前提公理）；
- 可观测性六道门要求"缺失段必须命名它阻塞了哪条链"（歧义必须定位，不许'日志少'含糊结案）；
- CLI 信封 `verification.passed: null` 必须附显式 `not_run`/`unavailable` 原因，
  "不得假装未检条件通过"（`friendly-cli.md:237-239`）——**正确的空与错误的空在契约层强制区分**。

**结论：BH 是 WHCode 信号链完备原则迄今见到的最完整机制化参照，这是本次比较的
最大价值点。**

### 1.7 资源铁律

BH 是按需 CLI，无常驻占用，且收集器有界意识好：事实池上限 `MAX_FACT_SESSION_POOL=40`、
根 CLI 机器输出缓冲 64MB 上限、lifecycle 超时默认 5s 上限 60s（`session-core-facts.mjs:8`、
`better-harness-cli/cli.mjs:22`、`read-only-command.mjs:103-118`）。
但每次评审 ≈ 4—5 次模型调用（三收集 + lead + 可选复评），这是它的主要资源代价。
WHCode 融合时的铁律映射：收集器为确定性同步命令（零新增线程、有界缓冲、执行完退出）；
LLM 评审协议是**显式用户触发的里程碑操作**，绝不自动周期运行（区别于 BH 的
`/schedule /better-harness` 定期评审设想——该项不采纳）。

### 1.8 工程过程对比

| 维度 | BH | WHCode | 判定 |
|---|---|---|---|
| CI | 真 GitHub Actions 3 OS × Node 22/24 矩阵 | ci.yml 多 job + ADR-037 唯一验收入口 | 相当；BH 覆盖三平台是优点 |
| action 引用 | `actions/checkout@v6` 可移动 tag（`ci.yml:40`） | 最新 release + 固定 SHA（AGENTS.md §5.1） | **WHCode 先手**，勿倒退 |
| ADR | 有体系但 9 份全部 Proposed、零 Accepted（`docs/adrs/README.md:14-22`） | ADR-001—045 均走完接受流程 | **WHCode 先手**；BH 证明"有 ADR 格式"≠"有 ADR 纪律" |
| 追溯 | change-traceability-review 技能（LLM 评审 Story→Spec→commit→tests 链）+ doc-link-graph 程序测试 | traceability.py + check_counts.py 程序门禁 | WHCode 先手（程序 vs 提示词）；BH 的 spec 实例质量值得学 |
| spec-first | dated spec + AC 编号 + 非目标 + 实现证据，实例严谨（`docs/specs/2026-07-21-*`） | specs/001 spec-kit 全套 | 同构；BH 的 `[NEEDS CLARIFICATION]` 标记与 WHCode `[待验证]` 同义 |
| 测试哲学 | 断言行为不断言文本、禁全仓 grep 禁词、'只能 grep 就导出 seam' | layering/layout 契约测试 + 门禁脚本 | BH 的测试哲学条目可补入 WHCode 测试指南 |

## 2. 与 ZCode 比较研究的交叉定位

三方视角下三者的分工（衔接 `research/zcode-comparison/actionable-gaps.md`）：

- **ZCode** 强在**执行侧**（turn 循环、工具系统、权限 broker、MCP）——WHCode 的 P0-1/P0-2 已立项补齐；
- **BH** 强在**评审侧**（工作流质量度量、发现工程、学习回路）——ZCode 完全没有这一层，
  WHCode 的门禁体系是它的"程序化特例"，但缺 BH 的证据分级语义与评审协议；
- **WHCode** 强在**治理侧**（程序化门禁、事件溯源、审计、追溯）。

三者可拼成完整图景：**执行（ZCode 模式）→ 评审（BH 模式）→ 门禁（WHCode 模式）**。
BH 的评审输出（发现）恰好是 WHCode 门禁体系（Check/任务）的输入源之一——
"发现→任务→Check"链条天然成立，无需新增状态机。

## 3. 融合方案（P0—P3）

> 每项标注：BH 证据、WHCode 现状、建议落点、ADR 要求。分级含义与
> `research/zcode-comparison/actionable-gaps.md` 一致（P0 项目成立要件 → P3 仅记录）。

### P0：语义升级（不改代码结构，改判定语言与文档）

| 能力 | BH 证据 | WHCode 现状 | 建议落点 | ADR 要求 |
|---|---|---|---|---|
| **Check 证据五态** | `Present/Wired/Exercised/Outcome-supported` + `Missing/Unobserved/Not applicable`（`agent-work-loop.md:93-108`）；"配置的能力不是被观测的使用" | checklist.md Check 二值（通过/缺口）；traceability.py 三方追溯已程序化 | `specs/001-whcode-harness/checklist.md` Check 语义升级 + `traceability.py`/`check_counts.py` 同步解析五态；新 Check 默认要求登记证据类别（机制存在/被使用/有后期结果） | **需 ADR**（验收语义变更，影响门禁脚本契约） |
| **复审条件机制化** | stale-or-conflicting-asset：命名资产+稳定触发器+规范真值+clean/gap 判定（`learning-loop-patterns.md:60-66`） | ADR/文档写复审日期（自反文档），无可执行绑定 | ADR 模板与 AGENTS.md 完成定义增补：复审条款优先写成"绑定命令 + 判定规则"，命令失效即触发复审 | 文档修订（AGENTS.md §9 + ADR 模板），随上一项同 ADR |
| **评审协议入库** | 三证据域独立（硬上限 3）→调和（保候选/合并条件/冻结后才排序）→八道质量门（`SKILL.md:69-176`、`findings-review.md`） | 无成文评审协议；G4 验收靠门禁脚本 + 人工 | `docs/guides/` 新增里程碑评审指南：确定性证据包命令 → 并行独立证据收集（代理数硬上限）→ 调和冻结 → 发现登记为任务/Check 候选；**LLM 判定仅 advisory**（§1.2 铁律写入协议） | 文档指南（无需 ADR，但引用 §1.2 铁律） |

### P1：确定性证据收集器（新增命令面，先立 ADR）

| 能力 | BH 证据 | WHCode 现状 | 建议落点 | ADR 要求 |
|---|---|---|---|---|
| **harness 证据包命令**（最小集） | `evidence-bundle` 冻结 topology/window/authority（schema v3）；收集器"不启动代理"（`core-change-watch.md:100-101`）；事实池有界 | `doctor`/`bench baseline` 已有运维命令先例 | `whcode-bin` 子命令 `harness-evidence`：项目拓扑（workspace 成员）+ git 有界历史 + 依赖治理线索 + AGENTS.md/specs 链接图校验（仿 doc-link-graph）+ 密钥扫描（`agent-guardrails/secret-scan.mjs` 语义）；输出 JSON 信封 | **需 ADR**（新增命令面与输出契约；零 unsafe、有界、离线、LF/无 BOM 门禁同源） |
| **CLI 信封语义升级** | `effects`=实际变更、`verification.passed: null` 显式 not_run、`retryable`/`safe_to_retry` 分离、幂等键四元绑定（`friendly-cli.md:188-407`） | CUI JSON 信封 + 稳定退出码 + dry-run 一致性已有（域 M 先手），但无 retry 语义与 verification 空态区分 | `whcode-cui` 信封契约增补字段；错误码表增加两值试语义 | **需 ADR**（对外契约变更，影响契约测试） |

### P2：学习回路与修复工程（视里程碑排期）

| 能力 | BH 证据 | WHCode 现状 | 建议落点 | ADR 要求 |
|---|---|---|---|---|
| **发现→任务转化协议** | finding 结构（被检视缺口+有界影响+最小 owner 修复+验证路线）；"计数/文件名/资产存在性单独不构成发现"（`agent-work-loop.md:126-134`） | 复盘有"缺口登记"惯例，无结构化 finding 模板 | 评审指南附 finding 登记模板；合格的发现直接落 `tasks.md`（已有状态机承接，无需新表） | 随 P0 评审指南 |
| **干预台账（学习捕获）** | `interventionLedger` 跨窗口连续性；恢复的台账只保纵向验证连续性（`agent-work-loop.md:635-648`）；Loop Effectiveness 只能由可比后期结果改变（同窗修复≠有效） | 无对应物；长跑稳定性（ADR-033）是程序化先例 | 里程碑评审的"改进项登记"复用 events 表（幂等键 + 审计）；"同窗修复不勾 Check，只登记修复声明"规则入评审指南 | 随 P0/P1 ADR 附带 |
| **技能体系最小集** | 主 Skill=路由器 + 一跳 references + doc-link-graph 可达性测试；skill-review 执行契约审计（P0=指向缺失资源）；triangulate 多 AI 三角评审 | AGENTS.md 为协作入口；无 skills 目录；M1 前不排期（域 H 待验证项：ZCode skills-lock 无消费者） | 若立项：`.zcode/skills`（或沿用 AGENTS.md + references 结构）；**先采纳 doc-link-graph 式可达性测试**（成本低、独立于技能格式） | **需 ADR**（新机制；对照 zcode-comparison 域 H 教训：锁文件必须有消费者） |

### P3：仅记录不排期

| 能力 | BH 证据 | 建议 |
|---|---|---|
| 宿主适配矩阵格式 | 12 宿主 × 7 列 + 生命周期处置表 + 拆分触发条件（`docs/adapters/README.md`） | WHCode 已有 56 条 SurfaceEntry 对等登记（先手）；仅借鉴"拆分触发条件"与"显式 unavailable 行"格式，不迁移 |
| 多 AI 三角评审（triangulate-spec-review） | ≥2 独立评审者同提示、P1/P2 清零才接受、"评审者是证据不是权威" | 可在 G2 设计门禁的人工评审中作为可选实践记录 |
| blast-radius 语法解析 | tree-sitter 多语言影响半径 | WHCode 单语言 Rust + cargo 依赖图已可答；不采纳 |
| 定期评审调度（/schedule） | BH 设想定期 `/better-harness` | **不采纳**：与资源铁律冲突（周期性模型调用），评审保持用户显式触发 |

### 不采纳与防御性反例（BH 自身缺点，WHCode 应避免）

1. **分数呈现**（59/74/84/94/100）：假精度制造全知幻觉（§1.1）；WHCode 用证据态 +
   三档粗分级。
2. **LLM 判定进入验收语义**：BH 的 verified 复评是 LLM 判断；WHCode 只能登记为代理声明，
   Check 勾选仍以 CI 证据为准（ADR-037）。
3. **可移动 action 引用**：BH 用 `@v6` tag（`ci.yml:40`）；WHCode 维持固定 SHA 铁律。
4. **ADR 长期挂起**：BH 9 份 ADR 全 Proposed 零 Accepted——"有 ADR 格式"不等于
   "有 ADR 纪律"；WHCode 保持接受/取代流程走完。
5. **14 宿主会话解析**：BH 最大复杂度负担（平台格式 × slug 折叠规则 × 压缩帧特性检测）；
   WHCode 只解析自有会话格式，不扩张宿主面。
6. **全仓 grep 禁词型测试**：BH 明文禁止并说明理由（"训练贡献者混淆字符串"）——
   WHCode 的 layering/layout 门禁是结构化契约测试，不属于此反例，但新增门禁时应避免
   沦为全文扫描。

## 4. 待验证项

1. **BH 上游时效**：全部结论基于 commit `b605532`（2026-10-20）；上游可能已变更，
   实施前重读对应文件（ADR-045 口径）。
2. **desktop Rust crates 与 Node CLI 的职责边界**【待验证】：`packages/better-harness-desktop/rust/`
   下 11 个 crate（evidence-host、arch-core、oxc-service 等）仅列目录未读；BH 自身也在
   "native engines 用 Rust"（`docs/ARCHITECTURE.md:16-20`），其对 WHCode 的参照意义
   （Rust 原生引擎如何嵌入 Node 宿主）未评估。
3. **renderer/Canvas 实现细节**【待验证】：`harness render` 的 HTML/Canvas 渲染器与
   validate-canvas 的 TSX transform 校验仅读注册表描述，未读实现——若 WHCode 采纳
   "授权渲染"协议需补读。
4. **hooks 安装面**：`hooks.json.template` 是 Claude Code 格式；其他宿主的 hooks 注册
   方式与 trust 审批（对照 zcode-comparison 域 J 的教训）未核实。
5. **intent-correlation-analysis 的 claim-contract**：`skills/intent-correlation-analysis/references/`
   未读全；其证据强度四级（direct/observed/correlated/inferred）与 WHCode 证据五态的
   关系待对照。
6. **WHCode 侧复核**：Check 五态融合前，需以 `check_counts.py` 实际口径复核 299 条
   Check 的证据类别分布（多少是"机制存在"级），避免高估迁移成本。

## 5. 修订记录

- **2026-10-28（首版）**：证据 commit `b605532fcf1014bcb48d3e6940a5bc64e80cddd5`
  （= `0.7.0-alpha2`）。批判结论：真实矛盾成立；门禁程序化冲突以 §1.2 铁律化解；
  最大价值 = 信号链完备的机制化参照（证据五态/六道门/verification null）。
  融合方案 P0 三项（Check 五态、复审条件机制化、评审协议入库）、P1 两项
  （证据包命令、信封语义）、P2 三项、P3 四项（两项不采纳）；反例 6 条；待验证 6 项。
