# WHCode 能力补齐建议书（综合 ZCode × Better Harness 研究）

> 本文综合 [`research/zcode-comparison/`](../zcode-comparison/actionable-gaps.md) 与
> [`research/better-harness/`](../better-harness/critique-and-fusion.md) 两份一手研究，
> 形成统一的优先级路线与决策请求。**性质：研究指引 + 决策请求**——任何一项落地均需
> 用户批准后先立 ADR，再走 G0—G5 门禁（AGENTS.md §5、§10）；本文不构成实施承诺。
> 统一能力矩阵与交叉发现见 [README.md](README.md)。

## 1. 摘要（一页决策视图）

**战略判断**：WHCode 的差距不在"再造一个 ZCode"，而在把治理先手升级为
**门禁 + 评审双层质量系统**，同时按最小必需集补齐执行层。三层拼图：

```text
执行层（学 ZCode 最小集）→ 评审层（学 BH 模式）→ 门禁层（保持 WHCode 先手）
```

**三条主线**（互不阻塞，可并行）：

| 主线 | 内容 | 状态 |
|---|---|---|
| A 执行层 | P0-1 turn 循环（ADR-044 进行中）→ P0-2 工具系统接线 → P1 压缩/重试/子代理/技能/MCP | P0-1 落地中 |
| B 评审层 | BH 三项语义升级：Check 证据五态、复审条件机制化、里程碑评审协议入库 | 文档/语义级，随时可启动 |
| C 治理层 | 保持并强化既有先手（程序化门禁/ADR/追溯/CI SHA/资源铁律），融合项一律先过门禁程序化铁律 | 持续 |

**需用户表态的决策点**（详见 §7）：① Check 五态 ADR 立项；② 评审协议入库；③ P1 排序确认；
④ sensenova tool calling 实测授权（消耗 API 额度）；⑤ P2 中平台/安全类事项的逐项批准。

## 2. 总目标与正负和解

**目标**：让 WHCode 成为"执行—评审—门禁"闭环完整的本地优先 Harness：代理能多轮干活
（执行层），干完有证据约束的评审（评审层），评审结论能程序化落地为任务与 Check（门禁层）。

**正面**：系统性盲区可见（BH 诊断的五类失效：模糊目标/即兴路径/无证明/速度压倒保障/教训
流失）；评审结论直接转化为任务与 Check，学习不流失。

**负面与承担者**：

| 代价 | 承担者 | 缓解 |
|---|---|---|
| 执行层补齐的工程量（E1/E2/E4/E5/E6/E8/E9 七项，估计跨多个里程碑） | 维护者 | 依赖排序（§3），每项独立 ADR 独立验收 |
| 里程碑评审 ≈ 4—5 次模型调用（3 收集 + 调和 + 可选复评） | 用户（token/时间） | 用户显式触发、里程碑级低频；**不做定期自动评审** |
| Check 五态迁移的一次性成本（checklist + 脚本同步） | 维护者 | 先用 check_counts.py 复核 299 条 Check 证据类别分布再定迁移方案（§6 前置） |
| 新命令面（harness-evidence）维护成本 | 维护者 | 确定性、有界、离线、零新增线程，与 doctor/bench 同类 |

**失效条件**（三重时间之将来项）：若 sensenova 实测不支持 tool calling 且 JSON 模拟代价
过高，执行层路线退化为"人驱动命令 + 评审层为主"；复审条件机制化若发现可执行检查维护
成本高于收益，退回"写日期 + 人工复审"并在 ADR 记录。

## 3. 统一路线图（阶段与依赖）

```text
主线 A（执行层，进行中）
  P0-1 turn 循环（ADR-044）──→ P0-2 工具系统 ADR+接线 ──┬→ E9 上下文压缩（挂 ADR-044 决策 2）
        ↑ 前置：sensenova tool calling 实测              ├→ E8 流式重试（独立，可早）
                                                        ├→ E4 子代理（依赖 turn+工具；硬上限必须）
                                                        ├→ E5 技能最小集（依赖工具；BH 治理为骨）
                                                        └→ E6 MCP（依赖工具投影；最重，最后）

主线 B（评审层，可与 A 并行，无代码依赖）
  B1 Check 证据五态（ADR）──→ B2 复审条件机制化（同 ADR 附带）
  B3 里程碑评审协议入库（指南文档；含发现→任务转化模板）
  B4 harness-evidence 证据包命令（依赖事件表已有，可早）
  B5 CUI 信封语义升级（retryable/safe_to_retry/verification 空态；契约 ADR）

既有里程碑（照旧，不被两翼打乱）
  M0 收口（长跑 verdict、NetBSD 回填、Check 134）→ M1 校准批次（ADR-012 转正、FR-503）
```

排序原则：① 文档/语义级（B1—B3）成本最低、价值最高，先行；② 执行层按依赖链推进，
重项（MCP）最后；③ P2 一律按里程碑排期，不进入当前批次。

## 4. 分项建议

### P0（项目成立要件）

| # | 能力 | 来源证据 | WHCode 现状 | 落点与 ADR | 和解要点 |
|---|---|---|---|---|---|
| P0-1 | Agent turn 循环 | zcode-comparison 域 A（`runRegularTurnLoop` 结构、状态机为事件流投影） | 进行中 | ADR-044 已立（FR-718—724/T204—T212/Check 300—308） | 状态机照 BH/ZCode 双证做投影而非事实源，事件流保持权威 |
| P0-2 | 工具系统接线 | 域 B（调度并发 10、输入校验三层）；BH 评审子代理"恰好 3"硬上限为纪律样例 | `store/tool_exec` 三步登记已建模未接线（先手协议） | `whcode-app` 编排 + `whcode-tools` 实现；另立 ADR 挂 ADR-044 决策 6 | 复用自有登记协议（比 ZCode 事件流更严），调度并发上限取 ZCode 值并加有界测试 |
| P0-3 | **Check 证据五态** | BH `agent-work-loop.md:93-108` | Check 二值（通过/缺口） | checklist 语义升级 + traceability.py/check_counts.py 同步；**需 ADR** | 采纳五态、拒绝分数；LLM 不得改变态级，升级须可复现证据（README.md §3 发现 6） |
| P0-4 | **复审条件机制化** | BH stale-or-conflicting-asset 模式 | ADR/文档写复审日期 | ADR 模板与 AGENTS.md §9 增补"复审条款优先绑定命令 + 判定规则"；随 P0-3 同 ADR | 可执行检查失效即触发复审；维护成本过高则退回日期制（§2 失效条件） |
| P0-5 | **里程碑评审协议入库** | BH 三域独立收集/调和冻结/八道质量门/代理硬上限（`SKILL.md:69-176`、`findings-review.md`） | 无成文评审协议 | `docs/guides/` 里程碑评审指南：证据包命令 → 并行独立收集（硬上限）→ 调和冻结 → 发现登记为任务/Check 候选；含 finding 登记模板（被检视缺口+有界影响+最小 owner 修复+验证路线） | **LLM 判定仅 advisory** 铁律写入协议；评审为用户显式触发的里程碑操作 |

### P1（核心增强）

| # | 能力 | 来源证据 | 落点与 ADR | 和解要点 |
|---|---|---|---|---|
| P1-1 | 上下文压缩执行 | 域 E（手动/自动/响应式 + microcompact + 摘要回填 + circuit breaker） | `whcode-app` 接线 `compress_context`；挂 ADR-044 决策 2 | 摘要回填走 events 表；压缩预算有界 |
| P1-2 | 流式生成重试（策略+契约合并） | 域 K（10 次退避/jitter/Retry-After）+ BH friendly-cli（retryable/safe_to_retry 分离） | `whcode-llm` 重试策略；**需 ADR** | 策略与契约缺一不可（交叉发现 5）；幂等语义按 BH 四元绑定 |
| P1-3 | harness-evidence 证据包命令 | BH evidence-bundle（冻结 topology/window/authority）、收集器"不启动代理" | `whcode-bin` 子命令；**需 ADR**（输出契约） | 读自有 DuckDB 事件表（后发优势，交叉发现 8）；零 unsafe/有界/离线 |
| P1-4 | CUI 信封语义升级 | BH `effects`=实际变更、`verification.passed: null` 显式 not_run、retryable/safe_to_retry | `whcode-cui` 契约增补；**需 ADR** | 对外契约变更，契约测试同步；正确的空与错误的空强制区分 |
| P1-5 | 子代理 | 域 F（进程内子 runtime + 事件镜像 + **无硬上限教训**） | `whcode-app/subagent.rs`；**需 ADR** | 硬并发上限 + registry 有界（BH"恰好 3"样例 + 资源铁律）；watchdog=超时终止，后台化独立开关 |
| P1-6 | 技能系统最小集 | 域 H（格式/注入；skills-lock 无消费者教训）+ BH（router 模式、doc-link-graph 可达性测试、创建门槛） | `.zcode/skills` 或 references 结构；**需 ADR**（对照域 J/H 教训） | BH 治理为骨、ZCode 格式为皮；锁文件必须有消费者；先试已有再创建（Loop Discovery 阶梯） |
| P1-7 | MCP 客户端 | 域 I（三 transport + OAuth + Job Object + 连接池） | `whcode-mcp` 新 crate；**需 ADR** | 最重项最后；超时/并发/缓冲全部有界；CUA fail-closed 原则保留 |

### P2（按里程碑排期）

| # | 能力 | 来源 | 要点 |
|---|---|---|---|
| P2-1 | 动态权限 broker | ZCode 域 C 四态决策 + escalate/modify；BH hooks permissionDecision | 从静态 Capability 向工具输入级规则演进；escalate/modify 需交互面配合 |
| P2-2 | Hooks 最小集 | BH Stop 发现信封 + test-mapping pass/remind/block 分级门 | 先 BH 语义（分级门），默认源离线/确定性/非阻断；ZCode trust 审批流后置 |
| P2-3 | 备份/恢复 | ZCode 域 L（SHA-256 + 自动预迁移备份 + restore） | `whcode-app` 运维命令；补运维闭环缺口 |
| P2-4 | 会话回滚双路径 | ZCode 域 D（对话剪枝 + 文件前像恢复） | 文件快照表须有上限（快照膨胀威胁模型）；对话剪枝基于 events 表 branchGeneration |
| P2-5 | 干预台账/学习捕获 | BH interventionLedger + 复发检测模式（负面控制/覆盖原因码） | 台账落 events 表（幂等+审计）；"同窗修复不勾 Check"规则入评审指南 |
| P2-6 | 远程协作 | ZCode 域 O（R3/Relay/daemon/分享） | M7+；平台/安全类须用户逐项批准 |

### P3（仅记录不排期）

扫码登录/预览/配置分层（ZCode）；适配矩阵格式借鉴与多 AI 三角评审（BH，可选实践）。

### 不采纳清单（两翼一致或单翼否决）

| 项 | 否决理由 |
|---|---|
| BH 分数呈现（59/74/84/94/100） | 假精度制造全知幻觉（山洞哲学）；采纳五态即可 |
| BH 定期自动评审（/schedule） | 周期性模型调用与资源铁律冲突；评审保持显式触发 |
| 凭据加密落库（ZCode `enc:v1:`） | 不落库更简单；落库加密需威胁模型证明必要性 |
| 14 宿主会话解析 | 自有事件表是唯一数据源（后发优势）；不扩张宿主面 |
| blast-radius 语法解析 | 单语言 Rust + cargo 依赖图已可答 |
| ZCode 无界 registry/50KB steering 式增长 | 融合项一律最小子集 + 失效条件 |

### 防御性反例（参照系自身缺点，融合时不得引入）

LLM 判定进入验收语义（BH verified 复评）；可移动 action 引用（BH `@v6`）；ADR 长期挂起
（BH 9 份全 Proposed）；时序不安全比较与无 Origin 校验（ZCode Legacy server）；锁文件无
消费者（ZCode skills-lock）；子代理无界（ZCode registry）。

## 5. 资源与代价总账

- **执行层七项**（P1-1/2/5/6/7 + P0-2 分批）：每项一个 ADR + 实现 + 正反路径测试 + 资源
  上限测试；MCP 最重（新 crate + 进程管理 + Job Object）。
- **评审层**：每次里程碑评审 ≈ 4—5 次模型调用；B4 命令为确定性代码，零模型成本。
- **治理层**：B1/B2 一次性迁移成本（299 条 Check + 2 个脚本）；此后 Check 维护语义更丰富
  但增量成本不变。
- **资源铁律约束全部适用**：新命令零新增线程、缓冲有界、执行完退出；子代理/评审代理有
  硬上限；无任何常驻新增。

## 6. 风险与前置未知

1. **sensenova-6.8-flash-lite tool calling 未实测**（决定 P0-2 ADR 写法：原生 function
   calling vs JSON 协议模拟 vs 换 provider）——建议作为 P0-2 立项第一个任务，实测结果
   更新 `docs/guides/test-model-sensenova.md`。【阻塞 P0-2】
2. **Check 五态迁移成本未测**：先用 check_counts.py 复核 299 条 Check 的证据类别分布，
   估计"机制存在级"占比后再定迁移批次。【阻塞 P0-3 ADR】
3. **上游时效**：两份研究的上游结论均基于 2026-10-20 commit 快照；实施每项前重读对应
   上游文件（0 信任口径）。
4. **CI 验收**：本机无远端仓库时，各项验收按 ADR-037 登记缺口，不得以本地全绿冒充。
5. 未核实项沿用两份来源研究登记：BH 桌面 Rust crates 职责边界、渲染器实现、
   claim-contract；ZCode 侧 skills-lock 写入方、prePlanMode 死代码等（见各自待验证节）。

## 7. 决策请求清单（需用户表态）

> **批准记录（2026-10-29）**：用户批准全部功能建议——D1—D5 按本文建议口径生效：
> D1 立项 Check 证据五态 + 复审条件机制化（先跑迁移成本复核）；D2 评审协议入库；
> D3 P1 排序确认（重试可提前并行）；D4 授权 sensenova tool calling 实测；
> D5 远程协作进 M7+ 排期、届时逐项批准（AGENTS.md §8 平台范围铁律保留）。

| # | 决策 | 建议 | 影响 |
|---|---|---|---|
| D1 | 是否立项 Check 证据五态 + 复审条件机制化（P0-3/P0-4，一个 ADR） | **建议立项**（先跑迁移成本复核） | 门禁语义升级，影响 traceability/check_counts |
| D2 | 是否将里程碑评审协议入库为指南（P0-5） | **建议入库**（纯文档） | 新增 docs/guides 一篇 |
| D3 | P1 执行层排序是否按 §3 依赖链（压缩→重试→子代理→技能→MCP） | **建议确认**（重试可提前并行） | 里程碑分配 |
| D4 | 是否授权 sensenova tool calling 实测（P0-2 前置） | **建议授权**（消耗少量 API 额度） | P0-2 ADR 写法 |
| D5 | P2-6 远程协作等平台/安全类是否进入排期 | **建议 M7+ 再议**（须逐项批准） | 范围 |

## 8. 修订记录

- **2026-10-29（第二阶段）**：规则重载后建议续篇见 [proposal-v2.md](proposal-v2.md)
  （P0 收口批次 R1—R4、双会话协调协议、m0.4 发布与 P1-2 提前建议、决策请求 D6—D9）。

- **2026-10-29（批准）**：用户批准全部功能建议（D1—D5），本建议书升格为已批准路线；
  各项进入 G0—G5 流程（ADR 起草/实测/指南入库）。
- **2026-10-29（首版）**：综合两份一手研究形成统一建议书：三条主线、P0 五项/P1 七项/
  P2 六项、不采纳六类、反例六条；决策请求五项（D1—D5）。与既有里程碑（M0 收口/M1 校准）
  的关系：两翼不打乱主线，B1—B3 文档级可立即启动。
