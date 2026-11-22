# 可补齐能力清单（P0—P3 行动指引）

> 本文基于 19 域报告（`domain-*.md`）交叉归纳，把 DSH 相对 WHCode 的独有能力按
> P0（项目成立要件）—P3（可选增强）分级，并给出 WHCode 侧建议落点与 ADR 要求。
> 结论性质：**研究指引**，不是验收承诺；任何一项落地都必须先立 ADR 再改代码
> （AGENTS.md §10：设计冲突先 ADR）。

## P0：项目成立要件

**当前为空**。与 2026-10-29 的 ZCode 研究相比，WHCode 的执行侧短板已经补上：
turn 循环（ADR-044/T204—T212）、工具系统（ADR-048/T213—T219）、技能（ADR-053）、
MCP stdio/HTTP（ADR-054/055）、压缩折叠（ADR-051）、子代理（ADR-052）、流式重试（ADR-050）
均已实现并有代码证据（见域 A/B/E/F/I/K §3）。DSH 侧没有“不做则项目不成立”的独有能力
尚未被 WHCode 覆盖——DSH 的独有能力集中在**可组合性、设备面与协作面**，属于增强而非要件。

## P1：与当前路线直接相关（ADR-057 批次内）

| 能力 | DSH 证据 | WHCode 现状 | 建议落点 | ADR 要求 |
|---|---|---|---|---|
| 工具级审批 broker（四态：允许一次/拒绝/撤销/无人可问） | `ApprovalOutcome` 封闭四值＋fail-closed（域 C；`packages/interaction/user-approval/src/index.ts`） | **已落地（2026-11-20 复核）**：封闭四态 broker＋fail-closed，等待预算硬上限，`approval_asked`／`approval_decided` 审计成对（T264／FR-754，锚点 `crates/whcode-app/src/approval.rs`）；接线见 ADR-064；CUI 逐次授权见 ADR-090 | `whcode-app` 新增 approval 模块；GUI/Web 四态交互卡 | 挂 ADR-057 B3；类型层先定四态 |
| 模型生成摘要（压缩） | `compaction/start|summary|end`＋锁＋usage（域 E） | **已落地（2026-11-20 复核）**：独立请求身份＋显式预算＋失败一次回退结构折叠＋孤儿锁可检测（T265／FR-755，锚点 `crates/whcode-app/src/compaction.rs`） | `whcode-app/src/context.rs` 增摘要调用；失败回退到现有折叠 | 修订 ADR-051；须定摘要失败与成本上限 |
| 失败模型尝试持久化 | `assistant/attempt` 不进模型历史但可审计（域 A） | **已落地（2026-11-20 复核）**：`EventKind::AssistantAttempt`（T266／FR-756，不进模型历史只留审计事实） | `events` 表新增 kind；同步事件上限与清理 | 修订 ADR-044 §事件表 |
| 单工具超时 | 工具声明 `timeoutMs`，guard 执行（域 B） | **已落地（2026-11-20 复核）**：工具声明 `timeout_ms`＋deadline 检查（锚点 `crates/whcode-tools/src/registry.rs`） | `execute_tool_call` 内 deadline 检查 | 挂 ADR-048 修订 |
| 用量锚点计量 | token-meter 优先复用供应商 usage（域 E/K） | **已落地（2026-11-20 复核）**：流式增量优先取供应商 usage，缺失才回退估算（锚点 `crates/whcode-app/src/llm.rs`） | `metering.rs` 存最近成功 usage | 修订 ADR-025/051 口径 |

## P2：增强（视里程碑排期）

| 能力 | DSH 证据 | WHCode 现状 | 建议落点 | ADR 要求 |
|---|---|---|---|---|
| 会话分叉（精确边界） | `fork(boundary)` 拒绝半截 turn＋`inheritedEventCount`（域 D） | **已落地（2026-11-20 复核）**：ADR-065 | `whcode-app` 会话命令；边界必须对齐 `turn/end` | 另立 ADR；须定事件复制上限 |
| 计划模式（软引导＋人复核退出） | `plan/mode` 事件＋`exit_plan_mode`（域 G） | **已落地（2026-11-20 复核）**：ADR-067 | `turn.rs` 系统提示段＋一次确认 | 挂 ADR-057 B2；写清“不强制” |
| 会话 todo 工具 | `todo/write` 整表替换、log-only（域 G） | **已落地（2026-11-20 复核）**：ADR-048 修订，log-only | `whcode-tools` 新工具＋`events` 新 kind | 明确与 SDD 任务表互不替代 |
| 交付物记录 | `present`＋`workspace-changes`（域 B） | **已落地（2026-11-20 复核）**：ADR-048 §7.2 | 工具层 `deliverables` 事件；GUI/Web 卡片 | 挂 ADR-057 B5 |
| 重复调用提醒 | 连续 3/5/8 次注入提醒（域 B） | **已落地（2026-11-20 复核）**：ADR-044 修订，事件式不注入模型 | 事件式提醒（不注入模型） | 无（事件级） |
| 溢出落盘（spill） | 头尾保留＋地址，保存失败保持内联（域 B） | **未做（2026-11-20 复核）**：仍 64 KiB 截断（`MAX_TOOL_OUTPUT_BYTES`）；已落地的是内存压力实验通道（ADR-025 修订），非工具输出 spill | `whcode-tools` 溢出策略 | 须定保留期与上限 |
| 模型目录 | `listModels`＋badge（域 K） | **已落地（2026-11-20 复核）**：ADR-031 修订 | profile 增可选 `models[]`；GUI/Web 选择器 | 修订 ADR-031 |
| 基准进 PR 门禁 | 八组基准＋内存压力通道（域 S） | **已落地（2026-11-20 复核）**：ADR-075 PR 基准回归门禁；内存压力通道见 FR-792／ADR-025 修订 | 选 turn 首 token/切换/历史页三项进 CI | 修订 ADR-025；噪声取中位 |
| 文档生成与新鲜度校验 | `gen-tool-catalog`＋`verify-cordis-catalog`（域 P） | **已落地（2026-11-20 复核）**：ADR-045 修订 | 从 `provider_manifest()` 生成工具目录，脚本校验 | 修订 ADR-045 |

## P3：远期（仅记录，不排期）

| 能力 | DSH 证据 | 说明 |
|---|---|---|
| SDK / ACP / API 网关 | JSON-RPC、automation-only ACP、类型驱动 RPC（域 I） | CUI JSON 信封可作最小 SDK；完整 SDK 需协议版本承诺（不可逆决策） |
| 远程执行世界（SSH provider） | FS/进程/沙箱成组迁移（域 O） | 需先有 shell 工具与 FS seam；否则无消费方 |
| LiveShare 分享 | 授权根＋工具替换＋撤销（域 O） | 可借鉴“替代实现而非过滤”的范式；门槛是审批与沙箱齐备 |
| Webhook / 宿主定时 | 外部投递建会话、七种计时（域 G/O） | 与“空闲零占用”冲突，需唤醒预算与幂等键 |
| 浏览器/桌面操作 | provider 注册表＋系统权限面（域 R） | 平台与权限面巨大，超出当前定位 |
| 附件/Office/语音 | 先持久化再引用、LibreOffice、STT（域 R） | 图像输入是唯一可能的近期项（见 P2 末尾的口径） |
| PTY/LSP/Workflow/PTC/Jobs | 执行族（域 Q） | 与“默认不启用运行时”的现行和解冲突，须整体重议 |
| 运行期插件组合/HMR | profile/bundle/patch（域 J） | 与静态分层、零 unsafe 直接冲突；不建议 |

## 防御性反例（DSH 有、WHCode 不应照抄）

| 反例 | DSH 证据 | 建议 |
|---|---|---|
| 无限重试（always 模式） | `retryPolicy.mode: always` 持续到成功/取消（域 K） | 禁止；WHCode 保持有界次数＋总预算 |
| 运行期插件加载与多层配置叠加 | profile/bundle/patch/HMR（域 J） | 不采纳；启动失败分级与版本协商的复杂度远超收益 |
| 前端 SPA 插件图 | `packages/client` 2039 个 TS 文件（域 M） | 不采纳；保持 50 KB JS 预算与无打包器 |
| Electron 桌面壳 | Desktop 由 Electron 承载（域 M） | 禁止（ADR-026 铁律） |
| 分散式“各自有界” | 无全局线程/句柄预算（域 S） | 保持统一铁律＋上限测试 |
| Webhook 无幂等/无队列 | 重复投递会重复建会话（域 O） | 若做，必须先有幂等键与去重 |
| `run_code` 按 owner 策略运行 | LiveShare 的 PTC 可读写授权根之外（域 O） | 若做分享，代码执行类能力必须单独评估逃逸面 |
| 无独立审计表 | 只有日志事件（域 L/N） | 保持审计表与失败阻断 |
| per-file 100% 覆盖率 | 对 Node 生态可行（域 P） | 不直接照搬到 Rust（平台分支多）；先试点阈值 |
| 遥测上报 | OTLP（域 L/N） | 保持默认无遥测；若加须 opt-in 且不含会话内容 |

## 归纳

- **DSH 真正值得抄的三件事**：①“先落边界再做事”的事件次序；②审批的封闭四态与 fail-closed；
  ③“验证世界而非自述”的 e2e 纪律。三者都不引入新运行期框架，成本可控。
- **DSH 看起来强但不该抄的三件事**：运行期插件树、Electron/SPA 前端、无限重试。
  它们的收益建立在 Node 生态与产品形态上，与 WHCode 的资源/安全/静态分层约束正面冲突。
- **WHCode 应保持的既有优势**：规格追溯与 ADR、单文件库＋校验和、审计表、资源数值与
  长时门禁、原生 GUI、默认安全、五协议内置。
- **相对 ZCode 研究的判定修正**：MCP、技能、子代理、压缩折叠、流式重试均已从“缺失”变成
  “已实现”（域 I/H/F/E/K §3）；2026-11-20 复核：压缩的**语义摘要**与审批的**工具粒度**
  也已补齐（见下方复核记录）；仍未补的是设备/协作面，以及并行批、中途投喂、spill 等
  执行面余项。

## 复核记录（2026-11-20）

触发条件：本文复审触发第 2 条（“WHCode 完成 ADR-057 B2—B5 → 重判 P1 表的现状列”）已满足，
且 ADR-058—ADR-090 批次大幅推进，按承诺重判并落账。证据口径：本轮逐项回到 `crates/**` 源码
与 ADR 正文，锚点写在上方两表现状列；`N2 凭据引用化` 本轮未复查，维持原判定（`[待验证]`）。

- **P1 五项全部落地**：四态审批 broker、模型生成摘要、失败尝试持久化、单工具超时、
  usage 锚点计量——现状列已逐行更新。
- **P2 九项落地八项**：分叉、计划模式、todo、交付物、重复提醒、模型目录、PR 基准门禁、
  文档校验；唯一未做＝**spill 溢出落盘**（仍 64 KiB 截断）。
- **overview §4 矩阵重判**：DSH 领先 20 行中 4 行翻转为同构（B3 交付物、C1 工具审批、
  D1 分叉、E2 语义压缩）；7 行改善为部分（G1 计划/todo 已补而 Goal CAS＋定时缺、
  I2 出进程 SDK＋ACP 已补而类型驱动网关缺、L2 三态分类＋关键词检索已补而 FTS 缺、
  O2 脱敏本地导出已补而 LiveShare 缺、P1 PR 基准＋覆盖率试点已补而 per-file 100% 不采纳、
  Q1 shell＋LSP 四查询已补而 PTY/工作流缺、C2 强度评估与上报已补而真沙箱缺）。
  即同构 13、DSH 领先 16、WHCode 领先 11。
- **余项重判（2026-11-20 修正初版清单）**：初版把 B2 并行批、spill、Goal CAS、宿主级定时、
  类型驱动 API 网关列入待办，经比对分域深报告的采纳判定，这五项均为**不采纳（当前）**
  （域 B §4.5/§4.6、域 G §4.5/§4.6、域 I §4.5，各有理由与复审条件）——属“有界性／资源
  优先”下的有意和解，不是欠账。真正处于“可采纳未落地”的只有三项：① 准入工具集按步
  失效（域 A §4.2，每步重解析工具清单；本轮复核确认清单在 turn 开始时装配一次——
  `crates/whcode-app/src/turn.rs:516`，循环内仅克隆 `:959`）；② steer 的最小
  有界命令通道（域 A §4.4 附带路径，完整四档投喂与 pre-step 钩子均不复制）；③ L2 全文
  检索第二步 FTS 扩展（域 L §4.4 允许的可选深化，ADR-076 已交付轻量第一步）。
  C 域真沙箱维持不采纳（工具面小、路径绑定已覆盖主要风险，域 C §4.4），O 域远程执行
  世界与 R 域设备面维持 P3 记录不排期。
- **判定可被用户指令覆盖的记录**：域 I 曾判 MCP resource/instructions “暂不采纳”，其后
  ADR-054 修订（P6.2）经用户批准落地——分域判定不是冻结，以用户批准的 ADR 为准；
  引用本目录判定时须核对 ADR 时间线。
- **防御性反例复核**：全部维持；webhook 反例被反向超越——WHCode 以幂等键＋去重＋有界
  重试交付（ADR-079），DSH 的重复投递问题在 WHCode 侧不存在。

## 增量复核（2026-11-23，上游 diff 驱动）

**基线变化**：`fcc0ad4`（2026-11-04，`0.2.2-rc.1`）→ `4b399c3`（2026-11-22，`0.2.3-alpha.2`），
2 628 提交、8 655 文件变更（含整仓格式化、测试快照与依赖锁）。以下只记影响本清单判定的项，
变更主题取自上游 `.agents/notes/implemented/**` 新增笔记（117 篇，筛选与本目录 19 域相关者）。

| 变更 | 上游状态 | 对判定的影响 |
|---|---|---|
| **延迟系统重建**（`2026-11-09-deferred-system-compaction.md`，域 E）：压缩只摘要既有对话，系统提示的重建推迟到下一次模型尝试，由装配按当前源（含未提交改动）解析；`compaction/summary` 事件在准入投影里标记 pending rebuild | implemented | **不构成新差距**。WHCode 无“系统源注册表／分段装配”概念——系统提示在 turn 开始时组装、压缩只折叠消息，不存在“旧系统提示残留”问题。记录为语义差异，不新增建议 |
| **工具可达性与初始披露**（`2026-11-18-tool-reachability-and-initial-disclosure.md`，域 B/A）：工具按可达性 `native`／`program`／`both` 与初始披露 `none`／`name`／`summary`／`full` 两维描述；授权（`restrict()`／`when`）与可达性分离 | **proposed（未实现）** | **强化**下方“准入工具集按步失效”：上游正从“每步全量披露”转向“按步解析＋分级披露”。只作方向印证，**不得引作“DSH 已实现”**；WHCode 若做，先做“按步重解析”，披露分级留作后续 |
| **子代理模型选择实时化**（`2026-11-17-live-subagent-model-selection-setting.md`，域 F）：`allowedRoutes()` 每次调用读 volatile 设置，不再把 allowlist 快照进会话；录制的 `subagent/model-selection-policy` 仅为旧日志校验保留、无代码读取 | implemented | **记录为设计原则**：若 WHCode 未来给 `spawn_subagent` 加模型路由，应“每次调用读当前设置”，不得快照进会话（快照会让编辑对运行中会话失效，且重放不能复原授权） |
| 上下文用量分解（域 E/M）：`buildContextUsageSnapshot` 在请求前算 7 类分解，供 UI 圆环与 debug 日志 | 既有（本快照前已存在） | 无变化；与 ZCode 的 Context Composition 对比见下节“跨书交叉” |

**维持原判定**：P1 五项、P2 八项已落地状态不变；“可采纳未落地”仍为三项（准入工具集按步失效、
steer 有界通道、FTS），其中第一项经本轮上游 diff 得到方向印证。spill、并行批、Goal CAS、
宿主级定时、类型驱动网关、真沙箱维持“不采纳（当前）”。

## 跨书交叉（2026-11-23）

两份上游在**工具清单动态性**上给出一致信号，值得合并为一条建议：

- DSH：`preStep()` 每步 `tools.resolve()`，准入集本步 `finally` 中 dispose（域 A §2.2）。
- ZCode：`apps/zcode-cli/docs/design/v2/loop/prompt-snapshot.md` 明确“**工具列表不记录**，
  每请求从当前 registry 准备”——理由是 MCP 中途连接会改变工具集，固定清单反而破坏
  prompt cache 前缀。

WHCode 现状：`turn.rs:516` 在 turn 开始时装配一次 `tool_manifest`，循环内克隆使用。
两个参照实现都选择“按请求/按步重解析”，且各自给出独立理由（DSH：准入时效；ZCode：
MCP 动态性）。这使该建议从“可选优化”升为“与 MCP 语义一致性相关”的项。

## 复审触发

- DSH 上游发布 `0.3.x` 或 `1.0.0`（破坏性变更窗口）→ 重读域 A/J/E 的对应源码；
  （执行记录：2026-11-20 已按第 2 条重判 P1 表现状列，见复核记录）
- WHCode 完成 ADR-057 B2—B5 → 重判 P1 表的“现状”列；
- WHCode 接入 shell 工具或运行时默认启用 → 重判域 C/Q/S 的沙箱与资源结论；
- 任一 `[待验证]` 项取得证据 → 更新对应域 §5 与本文相关行。
