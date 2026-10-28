# 统一功能差异比较：ZCode × Better Harness × WHCode（research/synthesis/）

- **目录性质**：外部参照与调研证据（入库依据 ADR-045）；**不构成 ADR 或实施承诺**。
  完整建议书见 [proposal.md](proposal.md)。
- **来源研究**：
  - [`research/zcode-comparison/`](../zcode-comparison/README.md)——ZCode（zai-org/ZCode，
    commit `3c5b227`，2026-10-20）16 域源码直读报告 + P0—P3 行动指引；
  - [`research/better-harness/`](../better-harness/README.md)——QoderAI/better-harness
    （MIT，commit `b605532` = `0.7.0-alpha2`，2026-10-20）设计要点提取 + 批判融合方案；
  - WHCode 侧以 `specs/001-whcode-harness/tasks.md`、`checklist.md`、`docs/adr/`（ADR-001—045）
    为权威，本目录不断言 WHCode 已实现某能力。
- **证据口径**：两份来源研究均为一手源码直读（file:line 见各域报告）；本文只做综合与
  判定调和，不引入新的一手证据。上游均可能已变更，实施前重读对应文件。

## 1. 三方定位

| 维度 | ZCode | Better Harness | WHCode |
|---|---|---|---|
| 一句话定位 | AI 编码工作台：执行侧全能（turn 循环/工具/权限/MCP/多前端） | Agent Work Loop 评审系统：把工程证据变成按优先级的发现与可验证下一步 | 本地优先编码 Harness：事件可追踪、规格驱动门禁 |
| 语言/运行时 | TypeScript，Node 24，Electron | TypeScript ESM，Node 22，按需 CLI（无常驻） | Rust 1.99.1（零 unsafe），常驻应用 |
| 核心抽象 | turn / 工具调用 / 会话分叉 | Task Episode / 证据五态 / finding | 任务 9 态状态机 / Check / EventRecord |
| 判定主体 | LLM 代理执行 + 事件流记录 | LLM 评审 + 确定性收集器（显式划界） | 程序化门禁 + LLM 仅建议 |
| 数据 | SQLite 多文件 | findings.json + 宿主自有 JSON（无完整性保护） | DuckDB 单文件 14 表（幂等键 + FNV 校验和） |
| 资源策略 | 弱（无界 registry，Electron 多进程） | 有界意识好（池/输出/超时上限） | 铁律体系（RSS/线程/句柄/CPU） |
| 许可 | Apache-2.0 | MIT | BSD-2-Clause |

**互补关系**（本综合的核心判断）：ZCode 只有**执行层**，Better Harness 只有**评审层**，
WHCode 只有**门禁层**。三者是同一质量闭环（执行→评审→门禁）的三段，而非三个竞品。
WHCode 的补齐策略因此是"两翼各取所需"，不是"再造一个 ZCode"或"移植一个 BH"。

## 2. 统一能力矩阵

图例：✅=完整实现；◑=部分/建模未接线；❌=无。判定列给出统一优先级（详见 proposal.md）。

### 执行侧（ZCode 主场）

| # | 能力 | ZCode | BH | WHCode | 统一判定 |
|---|---|---|---|---|---|
| E1 | Agent turn 循环与多轮工具编排 | ✅ | ❌ | ❌（P0-1 进行中） | **P0**（ADR-044，T204—T212） |
| E2 | 工具系统（注册/调度/回填） | ✅（并发上限 10） | ❌ | ◑ `tool_exec` 三步登记已建模未接线 | **P0**（接线，先手协议） |
| E3 | 工具输入校验三层/diff-patch/后台任务 | ✅ | ❌ | ❌ | 随 P0-2 分批 |
| E4 | 子代理/多智能体 | ✅（**无硬上限**，弱点） | ◑ 评审子代理模式（恰好 3、只读、不委托） | ❌ | **P1**；以 BH 纪律 + WHCode 铁律为准，勿抄 ZCode |
| E5 | 技能系统 | ✅（skills-lock **无消费者**） | ✅（router 模式 + 可达性测试 + 创建门槛） | ❌ | **P1 后段**；BH 治理为骨、ZCode 格式为皮 |
| E6 | MCP 客户端 | ✅ | ❌（仅资产盘点） | ❌（全仓无引用） | **P1**（最重项，新 crate 需 ADR） |
| E7 | Hooks | ✅（7 事件 + trust 审批流，重） | ◑（Stop 发现信封 + test-mapping 分级门，轻） | ❌（事件+审计底座已备） | **P2**；从 BH 语义起步、按 ZCode 事件面扩张 |
| E8 | LLM 重试/超时 | ✅（10 次退避**策略**） | ◑（retryable/safe_to_retry **契约语义**） | 5 协议面先手，生成不重试 | **P1**；两者合并（策略无契约会重试出重复副作用） |
| E9 | 上下文压缩 | ✅（手动/自动/响应式 + microcompact + 摘要回填） | ❌ | 预算加载先手，压缩未接线 | **P1**（挂 ADR-044 决策 2） |

### 评审侧（BH 主场）

| # | 能力 | ZCode | BH | WHCode | 统一判定 |
|---|---|---|---|---|---|
| R1 | 证据分级语义 | ❌ | ✅（五态 + 分数天花板） | Check 二值 | **P0**：采纳五态、拒绝分数（假精度） |
| R2 | 评审协议（独立证据域/调和冻结/质量门/代理硬上限） | ❌ | ✅ | 无成文 | **P0**（入库为里程碑评审指南） |
| R3 | 发现工程（finding 四要件 + 修复乐观锁 + 独立复评） | ❌ | ✅ | 缺口登记惯例（非结构化） | P0 随协议入库模板；P2 台账 |
| R4 | 可观测性六道门/恢复证据分级/verify loop 四平面 | ❌ | ✅ | 信号链完备原则（理念级同构） | P0/P1：吸收为 Check 论证语言 |
| R5 | 学习捕获（复发检测 + 负面控制 + 干预台账 + 纵向验证门） | ❌ | ✅ | ❌ | **P2**（台账复用 events 表） |
| R6 | 会话证据源 | 自有会话库 | ✅ 14 宿主解析（最大复杂度负担） | 自有事件表（**后发优势**：唯一数据源、有校验和） | 无需行动 |

### 治理侧（WHCode 主场）

| # | 能力 | ZCode | BH | WHCode | 统一判定 |
|---|---|---|---|---|---|
| G1 | 门禁程序化 | ❌（husky pre-push 可绕过） | ◑（doc-link-graph、test-mapping 程序化；评分靠 LLM） | ✅（traceability/check_counts/layering/layout） | **保持**，融合项一律先过此铁律 |
| G2 | ADR 体系 | ❌ | ◑（9 份全 Proposed、零 Accepted） | ✅（ADR-001—045 走完流程） | **保持**；BH 证明"有格式"≠"有纪律" |
| G3 | 规格驱动/三方追溯 | ❌ | ◑（spec-first 实例质量高，靠 LLM 评审） | ✅（FR↔任务↔Check 程序化） | **保持**；吸收 `[NEEDS CLARIFICATION]` 式显式未知标记 |
| G4 | CI 验收 | ❌（本地钩子） | ✅ 真 CI 3 OS 矩阵，但 action 用可移动 tag | ✅（ADR-037 + 固定 SHA 铁律） | **保持** |
| G5 | 事件溯源/审计 | ◑（SessionEvent 流） | ❌（findings.json 无校验和/幂等） | ✅（幂等键 + IntegrityFlags + 审计表） | **保持**；BH 台账若采纳落 events 表 |

### 平台与安全侧

| # | 能力 | ZCode | BH | WHCode | 统一判定 |
|---|---|---|---|---|---|
| X1 | 前端形态 | ✅ Electron 三形态 | 无（产出报告 HTML/Canvas） | 四适配器 + 原生 GUI 规划（ADR-026 铁律） | 各自定位，无可比缺口 |
| X2 | 权限模型 | ✅ 工具输入级 broker（四态 + escalate/modify 输入改写） | ◑ 宿主权限模式盘点 | ◑ 命令级静态 Capability 11 项 | **P2** 动态 broker（ZCode 证据 + BH hooks 参考） |
| X3 | 会话分叉/回滚 | ✅（原子 fork + 对话剪枝 + 文件前像双路径） | ❌ | ◑ 崩溃恢复先手（单事务幂等） | **P2** 回滚双路径（快照表须有上限） |
| X4 | 远程/协作 | ✅（R3 核心 + Relay V2 + daemon + 会话分享） | ❌ | SSH 原型（M7 范围） | **P2**（M7+，平台/安全类须用户批准） |
| X5 | 安全基线 | ❌ 反例（`===` 比较、无 Origin 校验） | ✅ 只读收集器 + 体系化脱敏 | ✅ 先手（Origin/回环/恒定时间/零落库） | **保持**；BH 脱敏收口方式可借鉴 |
| X6 | 资源纪律 | ❌（无界 Map） | ✅ 池/输出/超时上限意识 | ✅ 铁律体系 | **保持**；BH 上限取值可作参考 |
| X7 | 备份/恢复 | ✅（SHA-256 校验 + 自动预迁移备份 + restore） | ❌ | ❌ | **P2**（ZCode 域 L 先手，运维闭环缺口） |

## 3. 交叉发现（综合两份研究得出的十条）

1. **三层拼图**：执行（ZCode 模式）→ 评审（BH 模式）→ 门禁（WHCode 模式）。ZCode 完全没有
   评审层，BH 完全没有执行层；WHCode 的 Check/任务体系恰好是 BH 评审输出的天然承接物
   （发现→任务→Check 链条成立，无需新增状态机）。
2. **硬上限三方对照**：ZCode 子代理 registry 无界（`InMemoryRuntimeTaskRegistry.tasks: Map`）、
   BH 评审代理"恰好 3 个、只读、不得委托"的硬上限是正面样例、WHCode 资源铁律本来就要求。
   → 一切代理化能力以 BH 纪律为标准，不抄 ZCode 行为。
3. **技能系统三方对照**：ZCode 有格式与注入但锁文件与实现脱节（skills-lock 无任何代码
   消费者，两份锁文件内容还不一致）；BH 有完整治理（SKILL.md=路由器、doc-link-graph
   强制可达、skill-review 执行契约审计、Loop Discovery 覆盖阶梯"先试已有再创建"）；
   WHCode 无。→ 若立项，**以 BH 治理为骨、ZCode 目录格式为皮**，且锁文件必须有消费者。
4. **Hooks 两极**：ZCode 全套重（7 事件 + matcher + trust 审批流，移植需配套安全边界）；
   BH 最小有用集（Stop 链产出 findings 信封、test-mapping 的 pass/remind/block 分级门，
   默认源"离线、确定性、非阻断"）。→ 融合路径：先 BH 语义（分级门而非全有全无），
   后按 ZCode 事件面扩张。
5. **重试的两面**：ZCode 给**策略**（maxRetries=10、指数退避、jitter、Retry-After 优先），
   BH 给**契约语义**（`retryable` 与 `safe_to_retry` 分离——"远端写后超时可能 retryable
   但不 safe to retry"）。只有策略没有契约会重试出重复副作用；只有契约没有策略是空话。
   → P1 必须两者合并。
6. **证据语义是三方共同短板的不同切面**：ZCode 无任何证据分级；BH 有五态但判定靠 LLM；
   WHCode 有程序化门禁但二值语义。→ 三方最优组合 = **WHCode 的程序化 + BH 的语义**，
   即"证据五态由程序登记、态的升级由可复现证据驱动、LLM 只建议"。
7. **复杂度警戒线**：BH 的 14 宿主 JSONL 解析与 819 行评估模型、ZCode 的 50KB steering.ts
   与无界 registry，都是"能力增长失控"的实例。→ WHCode 每项融合必须裁剪最小子集并写
   明失效条件，不为对齐而对齐。
8. **后发优势**：BH 最大的工程负担（跨 14 宿主会话格式解析）在 WHCode 不存在——自有
   DuckDB 事件表就是唯一数据源（带幂等键与校验和）。BH 式评审在 WHCode 的实现成本
   显著低于 BH 自身，这让"评审层"从可望不可即变为 P0 可落地。
9. **运维与学习是两条独立缺肢**：WHCode 缺**运维闭环**（备份/恢复，ZCode 有）与
   **学习闭环**（复发检测/干预台账/纵向验证，BH 有）。两者互不依赖，可分别从两翼取。
10. **AI 判定边界两家给出互补答案**：BH 显式划界（"确定性 writer 可验证/持久化/投影，
    但不撰写读者文案"；source-review 命令"不代写 decision、不调用模型"），ZCode 无此
    纪律。→ WHCode 铁律（门禁程序化）+ BH 划界技术 = 完整答案：**可判定部分程序化，
    不可判定部分只作建议**。

## 4. 独有能力合并清单（去重后）

**仅 ZCode 有**（15 项）：turn 循环与多轮编排；工具全家桶与调度；工具输入级权限 broker
（escalate/modify）；会话原子 fork；对话剪枝 + 文件前像回滚；MCP 客户端全套；Hooks trust
审批流；流式生成重试策略；上下文压缩执行；远程协作全套（R3/Relay/daemon/分享）；备份/恢复
运维闭环；模型轨迹白板/Todo UI；model-option-map 请求体补丁；配置四层合并；Bot Channel。

**仅 BH 有**（10 项）：Task Episode 评审单元与证据五态；评审协议（三域独立/调和冻结/质量门/
硬上限）；finding 工程与修复乐观锁；可观测性六道门；恢复证据分级；verify loop 四平面；
Loop Discovery 十项决策门与最小所有者路由；学习捕获（复发模式 + 负面控制 + 干预台账 +
纵向验证门）；五个自举技能（含多 AI 三角评审）；文档链接图可达性测试。

**仅 WHCode 有**（15 项）：规格驱动门禁（GateId 四类 + force_advance 审计）；FR↔任务↔Check
三方追溯程序化；CI 唯一验收入口 + action 固定 SHA；ADR 体系走完流程；四端对等 SurfaceEntry
（56 条）；事件溯源全链路（幂等键/校验和迁移/四级持久化）；审计表 11 类动作不随清理删除；
工具执行三步登记协议（prepared→running→终态 + recover_uncertain）；崩溃恢复三表原子提交；
资源铁律体系；零 unsafe + clippy 零告警；追踪四级脱敏 + Origin 校验 + 恒定时间比较；
原生 GUI 铁律与四套精确主题；SSH 前端（TOFU + 默认拒 Shell）；CUI JSON 信封 + dry-run
一致性。

**两家共有的先手（WHCode 两头都缺的交汇点）**：无——两家独有集不相交，恰好印证三层
互补判断。

## 修订记录

- **2026-10-29（首版）**：综合 `research/zcode-comparison/`（16 域，ZCode commit `3c5b227`）
  与 `research/better-harness/`（commit `b605532`）两份一手研究；26 行统一能力矩阵
  （执行 9 + 评审 6 + 治理 5 + 平台 6）+ 十条交叉发现 + 独有能力合并清单。建议书见
  [proposal.md](proposal.md)。
