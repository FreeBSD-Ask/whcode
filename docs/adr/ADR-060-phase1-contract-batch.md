# ADR-060: Phase 1 首批——执行层契约收口（P1.1—P1.7、P2.1/P2.3、P7.1/P7.5/P7.6）

- **状态**：**已批准（2026-11-07，用户“批准所有不计成本的，最佳建议方案”＋“继续，你接手”）**
  ——批准对象为 [proposal-v3](../../research/synthesis/proposal-v3.md) 的 Phase 1 首批 11 项，
  路线级批准见 ADR-059。
- **日期**：2026-11-07
- **关联**：ADR-059（路线全量采纳）、ADR-044（turn 循环）、ADR-048（工具系统）、ADR-050（流式重试）、
  ADR-051（压缩）、ADR-046（证据态与复审触发）、ADR-025/037（性能与验收入口）、
  `research/dsh-comparison/`（域 A/B/C/E/P 证据）、`research/better-harness/`（证据五态）。
- **范围边界**：只做**执行层契约与判定语义**，不做 UI（审批交互卡属 T256/T257）；不新增 crate；
  不改平台层；不引入运行期插件。

## 问题

1. **审批无类型**：现行 `Capability` 只回答“actor 能否用该命令”，无法表达“这一次动作是否被
   允许”；“没人可问”与“明确拒绝”在二值拒绝里不可区分（域 C）。
2. **压缩无语义**：`fold_messages_for_budget()` 只写“折叠 N 条”，`summary_ref` 是指针，长会话
   丢任务状态（域 E）。
3. **失败不可审计**：模型请求失败只留 `RuntimeTerminated(error)`，失败的流内容与尝试次数不可见
   （域 A）。
4. **工具无预算**：只有跨进程取消，没有“单工具最多 N 秒”；重复调用无提示；输出无结构化契约
   （域 B）。
5. **turn 边界与执行同批提交**：崩溃时边界事件不可见（域 A）。
6. **计量靠启发式**：预算判断用字符估算，供应商 usage 不参与（域 E/K）。
7. **证据语义与门禁语言未对齐**：Check 证据态与 BH 五态是两套说法；ADR 无“已知限制”强制节；
   覆盖率与真 API 冒烟缺门禁（域 P）。

## 决策

### 1. 逐项契约（FR-754—FR-764）

| 项 | 提案 | FR | 契约要点（失败路径必须显式） |
|---|---|---|---|
| A | P1.1 | FR-754 | 封闭四值 `allowed_once/rejected/cancelled/unavailable`；仅 `allowed_once` 放行；无应答者、应答者异常、超时一律 `unavailable`（**不得**降为允许或拒绝）；请求只带 `exec_id`；`approval_asked`/`approval_decided` 成对审计，审计失败阻断执行 |
| B | P1.2 | FR-755 | 超预算先做一次模型摘要（独立 `request_id`、`purpose=compaction`、显式 `max_tokens`）；落 `CompactionStarted/CompactionSummary/CompactionFinished` 三事件（start 先落、end 最后落，崩溃留孤儿锁）；摘要失败回退现行结构折叠且不阻塞 turn；usage/provider/model 入 summary |
| C | P1.3 | FR-756 | 失败尝试落 `assistant_attempt` 事件（不进模型历史）：`failure.code/message`、已收块数、字节数、是否已发出 chunk；事件有容量上限与截断标记 |
| D | P1.4 | FR-757 | 工具声明 `timeout_ms`（可选，缺省无超时）；到期返回结构化结果（`code=tool_timeout`、`retryable=false`、`timed_out=true`）；**不**强杀同进程代码，只停止等待并标记；上限常量 |
| E | P1.5 | FR-758 | 保存最近一次成功请求的 usage（输入/输出 token、provider/model）；预算判断＝锚点＋增量估算；锚点缺失回退字符启发式；只存一条（有界） |
| F | P1.6 | FR-759 | 工具可声明输出 schema 与 `render(args, value) -> String`（模型面）＋展示面分离；schema 校验失败→结构化错误；输出仍受统一截断上限 |
| G | P1.7 | FR-760 | `Turn_started` 在首次模型调用前**单独提交**（不与迭代事件同批）；未闭合的 turn 在下次打开时可见（`interrupted` 语义） |
| H | P2.1 | FR-761 | 五态命名对齐（`present/wired/exercised/outcome-supported`＋`not-applicable`，缺失为 `missing`）；升级判据必须可复现；明确拒绝评分制（假精度） |
| I | P7.1 | FR-762 | `whcode-app`/`whcode-store` 行覆盖 ≥90%（`cargo-llvm-cov`），脚本可复现；CI 先登记非阻断，阈值与豁免清单成文 |
| J | P7.5 | FR-763 | 新 ADR 必须有“已知限制/失效条件”节，脚本校验（存量 ADR 白名单豁免） |
| K | P7.6 | FR-764 | sensenova 单条真 API 冒烟：凭据缺失自跳过（不假绿），结果归档；凭据不入日志/事件 |

### 2. 顺序与冻结

1. 实施序：A（类型与 broker 后端）→ G（边界先落）→ C（失败尝试）→ D（超时）→ E（usage 锚点）
   → F（输出契约）→ B（模型摘要）→ H/I/J/K（治理项）。
2. **A 的接线冻结**：本批只交付类型、判定与审计；`execute_tool_call` 接入审批属 T255/B3b，
   本批不得在 turn 路径强制 `ask`（否则无 UI 时全部工具变 `unavailable`，属功能性回归）。
   A 的验收以“判定函数＋审计对账＋fail-closed 用例”为准。
3. **B 的模型调用**：复用现有 `Provider` 与 `complete` 路径，不新增线程；摘要调用与 turn 共用
   取消令牌；单次失败即回退，不重试（避免与 ADR-050 重试叠加）。
4. **E 的锚点**：只存一条，随请求成功覆盖；不做多供应商矩阵。

### 3. 与宪法/铁律的关系

- 零 unsafe：本批全部纯 Rust，无 FFI。
- 资源铁律：D 的超时不创建线程（用现有取消切片检查＋截止时间比较）；E 只存一条锚点；
  C 的事件有上限常量。**不触碰 NFR-211**（PTY/jobs 不在本批）。
- 安全优先：A 的 `unavailable` 是最小权限表达；K 的凭据只从 `local/sensenova.env` 读取。

## 正负和解

- **正面**：审批第一次有可判定的类型与审计对账；压缩具备语义；失败可审计；工具具备预算与契约；
  证据语言与外部评审体系对齐；覆盖率/ADR 限制/真 API 冒烟让治理闭环。
- **负面**：A 的类型与调用方需处理第四态（迁移面）；B 增加一次模型调用成本与失败回退分支；
  C 增加事件量（需上限与清理口径）；I 拉长 CI；K 需要凭据管理纪律。
- **承担者**：实施批次负责人（逐项验收与预算）；门禁维护者（计数与证据态）；测试负责人
  （失败路径用例）。
- **和解理由**：这些都属“契约缺失”而非“功能缺失”，补的是可判定性；成本集中且可分批，
  与资源/安全铁律不冲突。

## 三重时间

- **过去来源**：DSH 域 A/B/C/E、ZCode 域 K/E、BH 证据五态；proposal-v3 §4 W1/W2/W7。
- **现在证据**：`cargo test --workspace` 绿（2026-11-07 基线）；`check_counts.py` 输出
  FR=131/tasks=263/checks=358。
- **将来失效条件与复审触发**：
  1. T255/B3b 接入审批 UI 时，A 的冻结解除，须重审“默认策略与放行矩阵”；
  2. 真实会话中摘要调用失败率 >20% → B 回退策略复审；
  3. 事件表体积因 C 增长 >30% → C 的采样/上限复审；
  4. 覆盖率试点 6 个月（2027-05-07）→ I 阈值与范围复审；
  5. DSH 0.3.x → A/B/E 的对照结论重读（`research/dsh-comparison/` 域 A/B/E）。

## 实施记录

- 2026-11-07：G1 入账（FR-754—764、T264—275、Check 359—370）；代码按 §2 顺序实施，
  A 先落地。逐项验收后在本节追加“项—提交—证据”。
- **项 D（P1.4／FR-757，T267）—`bb24e51`**：`ToolDescriptor.timeout_ms`＋`effective_deadline`
  （上限 600 s 截断、未声明＝无预算）＋`ErrorCode::ToolTimeout`（`retryable=false`，CUI 退出码
  复用 90）＋glob/grep 扫描循环协作式时钟检查；到期登记终态 `TimedOut`（`timed_out=true`
  的结构化载体），不建线程、不强杀。测试三路径（正常/超时/上限）；Check 362＝exercised；
  CI run 42365370076 全绿。同一提交含检查点浏览的 trait 派发修复（T254：`checkpoints` 移入
  `impl ViewSource`，消除“GUI 恒空”的错误空）。
- **项 E（P1.5／FR-758，T268）**：schema v8 新增 `usage_anchor` 单行表（`scope='last'` 主键＋
  CHECK；导出排除：本地校准值）；store `save_anchor/load_anchor` **只收供应商实测**；turn 预算
  门禁按 provider/model 身份匹配后用实测字符/token 比校准（合理区间 [0.5, 16]），缺失/不匹配/
  读取失败一律回退 4 字符启发式（行为与现行一致），压力事件带 `estimate_basis`／
  `chars_per_token`。测试：store 3＋app 4＋迁移集成 1（v7→v8）。

## 修订记录

- 2026-11-07（首版）：Phase 1 首批 11 项契约与顺序入库；A 的接线冻结与 B 的回退策略成文。
