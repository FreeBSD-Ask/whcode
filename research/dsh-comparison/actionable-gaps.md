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
| 工具级审批 broker（四态：允许一次/拒绝/撤销/无人可问） | `ApprovalOutcome` 封闭四值＋fail-closed（域 C；`packages/interaction/user-approval/src/index.ts`） | 仅命令级 `Capability`；T255 未做 | `whcode-app` 新增 approval 模块；GUI/Web 四态交互卡 | 挂 ADR-057 B3；类型层先定四态 |
| 模型生成摘要（压缩） | `compaction/start|summary|end`＋锁＋usage（域 E） | `fold_messages_for_budget` 只写“折叠 N 条” | `whcode-app/src/context.rs` 增摘要调用；失败回退到现有折叠 | 修订 ADR-051；须定摘要失败与成本上限 |
| 失败模型尝试持久化 | `assistant/attempt` 不进模型历史但可审计（域 A） | 失败只落 `RuntimeTerminated(error)` | `events` 表新增 kind；同步事件上限与清理 | 修订 ADR-044 §事件表 |
| 单工具超时 | 工具声明 `timeoutMs`，guard 执行（域 B） | 只有跨进程取消 | `execute_tool_call` 内 deadline 检查 | 挂 ADR-048 修订 |
| 用量锚点计量 | token-meter 优先复用供应商 usage（域 E/K） | 字符启发式估算 | `metering.rs` 存最近成功 usage | 修订 ADR-025/051 口径 |

## P2：增强（视里程碑排期）

| 能力 | DSH 证据 | WHCode 现状 | 建议落点 | ADR 要求 |
|---|---|---|---|---|
| 会话分叉（精确边界） | `fork(boundary)` 拒绝半截 turn＋`inheritedEventCount`（域 D） | 无 | `whcode-app` 会话命令；边界必须对齐 `turn/end` | 另立 ADR；须定事件复制上限 |
| 计划模式（软引导＋人复核退出） | `plan/mode` 事件＋`exit_plan_mode`（域 G） | 无 | `turn.rs` 系统提示段＋一次确认 | 挂 ADR-057 B2；写清“不强制” |
| 会话 todo 工具 | `todo/write` 整表替换、log-only（域 G） | 无 | `whcode-tools` 新工具＋`events` 新 kind | 明确与 SDD 任务表互不替代 |
| 交付物记录 | `present`＋`workspace-changes`（域 B） | 无 | 工具层 `deliverables` 事件；GUI/Web 卡片 | 挂 ADR-057 B5 |
| 重复调用提醒 | 连续 3/5/8 次注入提醒（域 B） | 无 | 事件式提醒（不注入模型） | 无（事件级） |
| 溢出落盘（spill） | 头尾保留＋地址，保存失败保持内联（域 B） | 64 KiB 截断 | `whcode-tools` 溢出策略 | 须定保留期与上限 |
| 模型目录 | `listModels`＋badge（域 K） | profile 静态声明 | profile 增可选 `models[]`；GUI/Web 选择器 | 修订 ADR-031 |
| 基准进 PR 门禁 | 八组基准＋内存压力通道（域 S） | `bench baseline` 仅手动 | 选 turn 首 token/切换/历史页三项进 CI | 修订 ADR-025；噪声取中位 |
| 文档生成与新鲜度校验 | `gen-tool-catalog`＋`verify-cordis-catalog`（域 P） | 无 | 从 `provider_manifest()` 生成工具目录，脚本校验 | 修订 ADR-045 |

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
  “已实现”（域 I/H/F/E/K §3）；仍未补的是压缩的**语义摘要**、审批的**工具粒度**、
  以及全部设备/协作面。

## 复审触发

- DSH 上游发布 `0.3.x` 或 `1.0.0`（破坏性变更窗口）→ 重读域 A/J/E 的对应源码；
- WHCode 完成 ADR-057 B2—B5 → 重判 P1 表的“现状”列；
- WHCode 接入 shell 工具或运行时默认启用 → 重判域 C/Q/S 的沙箱与资源结论；
- 任一 `[待验证]` 项取得证据 → 更新对应域 §5 与本文相关行。
