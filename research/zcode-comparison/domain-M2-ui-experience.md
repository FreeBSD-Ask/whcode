# 域 M2：ZCode UI 体验层勘察与视觉规格提取（批次 R，T249）

- **基线**：`local/zcode-src` commit `3c5b227649371507ddc4eb46ac5133afe6db1cab`（2026-10-20，
  zai-org/ZCode；克隆内 `git log -1` 复核一致）。本文所有源码相对路径均省略前缀
  `D:\WHCode\local\zcode-src\`。
- **勘察日期**：2026-11-06。
- **用途**：落实 `specs/001-whcode-harness/plan-ui-zcode-alignment.md` §2 批次 R——为 B1—B5
  各实现批提供体验级参照与 zcode-day／zcode-night 主题精确色值提案；接续
  [domain-M-frontend.md](domain-M-frontend.md)（形态级），不重复其内容。协议域语义见
  domain-B（工具系统）／domain-C（权限）／domain-G（计划模式），本文只补 UI 呈现层。
- **失效条件**：ZCode 基线演进（`local/zcode-src` HEAD commit ≠ `3c5b2276`，或上游大版本）
  → 复审本文；判定规则＝文件头部基线 commit 字段比对（ADR-057 复审触发节）。勘察结论
  只入 research，不入矩阵；采纳任何一项须走 WHCode G0—G5 门禁。
- **证据层级**：全部结论为第 1 层证据（源码直读），锚点随文给出；无法在克隆内核验的点标
  `[待验证]` 并注明原因（汇总见 §10）。三次复查：查证（逐文件直读）→ 交叉验证（协议
  schema 与 UI 消费侧互证、桌面 Web 栈与 TUI 栈互证、行号 grep 实证）→ 判定（确证／存疑）。
- **纪律**：本文只勘察不决策；§7 提案表是“候选值”，冻结须按 FR-739 流程。WHCode 既有
  四套主题（amoled-night 纯黑 `#000000`／eink-day `#FFFFFF/#000000`／eink-night
  `#000000/#FFFFFF`／terminal-green `#000000/#00FF00`）的精确色值与墨水屏口径**不可稀释**，
  zcode 系主题为**新增而非覆盖**。WHCode 侧锚点本次已复核在库：
  `crates/whcode-gui/src/lib.rs`（Theme 枚举 EinkDay/EinkNight/AmoledNight/TerminalGreen，
  第 50—67 行）、`crates/whcode-web/src/assets.rs`（`THEMES` 四套，第 57 行）。

---

## 1. 对话流信息架构（勘察面①）

### 1.1 ZCode 实现（源码锚点）

**协议投影层（CLI 下发，桌面／Web／移动共用）**——`packages/shared/src/zcode-protocol-v4/rows.ts`：

- 对话流由封闭的 row kind 联合构成：`turnHeader`（第 70 行）、`userInput`（118）、
  `assistantText`（172）、`reasoning`（184）、`toolCall`（207）、`artifact`（252）、
  `subagent`（268）、`hookInvocation`（323）、`timelineMarker`（427）。
- `turnHeader` 是**权威轮次边界**（第 60—112 行）：`origin` 枚举
  `userInput/backgroundResult/goalContinuation/editRerun/workflowLaunch`；`state` 枚举
  `running/completedSuccess/completedInterrupted/failed`；携带权威工时 `activeMs`
  （注释明示排除权限等待／用户输入等待／verifier 等待）与
  `modelDurationMs/toolDurationMs/otherDurationMs` 分解，以及每轮文件摘要
  `fileChanges {additions, deletions, files, state: active|reverted}`。
- `assistantText.state` 枚举 `streaming/complete/interrupted/failed`（第 181 行）；不变量
  “追加只能进 state=streaming 的行”（第 169—171 行注释）。

**UI 分组层**——`packages/ui/src/v4/conversationTurnRenderUnits.ts`：

- `buildConversationTurnRenderUnits` 把行序列折叠成 turn 渲染单元
  `{header, userInputs, assistantWorkRows, hookInvocations, orderedRows}`（第 27—33 行）；
  可见性过滤：`EnterPlanMode` 工具行不显示、空 reasoning 块过滤（第 47—60 行）。
- “已工作”折叠段边界：`splitTurnTailRows`（第 102—118 行）把轮尾 `turnTailBoundary`
  marker 与 artifact 行拆出 flow；`lightBoundary` lane（第 120—122 行）做轮顶轻边界
  （modelChange 渲染在用户输入之前，lane 由 CLI 投影裁决、UI 不得自行推断，第 96—98 行注释）。
- 折叠策略：running 展开；异常终态**强制展开**（`shouldForceOpenAbnormalHistory`，第
  216—226 行：header `completedInterrupted|failed`）；终态以 header 为权威，冷快照缺
  header 才回退 sessionPhase（防止 background-only work 把已终态主轮推回 running）。

**时间线渲染层**——`packages/ui/src/v4/ConversationTimeline.tsx`（2207 行）：

- 虚拟滚动 `@tanstack/react-virtual`（第 21 行导入），`ROW_OVERSCAN = 8`（第 116 行）；
  历史区（virtualHistory）与**实时尾部（live tail）分离渲染**（第 52—57 行导入；实时尾部
  不进虚拟器，第 2094—2121 行）。
- 滚动：吸底／back-to-bottom 按钮（第 2177—2202 行）、prepend 虚拟锚点补偿、每会话滚动
  记忆（第 80—85 行导入）、历史向上分页 loadOlder（第 101 行）；用户滚动意图 TTL
  `USER_SCROLL_INTENT_TTL_MS = 1200`（第 120 行）。
- 内容列宽：草稿态 `max-w-2xl`、会话态 864px 以上 `max-w-4xl`、1280px 以上 `max-w-6xl`
  并向侧栏让位 24rem（`packages/ui/src/v4/conversationLayout.ts` 第 3—11 行）。

**流式增量呈现**——`packages/ui/src/components/ai-elements/message.tsx`：

- assistant 正文用 `streamdown` 流式 markdown 渲染器（第 41 行），插件集
  cjk（`@streamdown/cjk`）＋code＋math（KaTeX，`styles.css:6` 引入样式）＋mermaid
  （第 12—15、428 行）；渲染模式两态 `streaming | static`（第 475、702—705 行：只有真实
  流式输出进 streaming，完成态长消息走 static，防 block state 反复同步）。
- 流式进入动画 `zcode-stream-text-in` 900ms `cubic-bezier(0.16,1,0.3,1)`
  （`styles.css:1019,1093—1105`）。
- 消息内结构 `packages/ui/src/lib/taskChatMessageTypes.ts`：`parts` 记录流式事件**真实
  到达顺序**（第 78—83 行注释：按事件顺序回放，而非固定“工具在上、正文在下”）；
  `streaming/interrupted/turnIndex/feedback(like|dislike)` 独立字段（第 70—93 行）；
  大消息 `bodyRefs` 首屏 preview＋按需回填、工具 `toolSlice` 游标“查看更多工具调用”
  （第 86—89 行）——有界累积的现成范式。

**停止／重试入口**：

- Web／桌面：输入框在 `snapshot.control.canStop` 且无草稿时显示**停止按钮**，快捷键 Esc
  （`packages/ui/src/v4/ConversationComposer.tsx` 第 1296—1337、1821—1828、2386—2397 行；
  文案“停止生成”，`i18n/locales/zh-CN.ts:5335`）。旧状态机注释（第 1336 行）：
  “streaming＋空草稿 → Stop；有草稿 → 发送键（入队）”。
- 重试／分叉／编辑挂在**最新一条 assistant 正文**上（`ConversationTurnGroup.tsx` 第
  853—860 行：`item.latest && canRetryLatestAssistant` 才传 `onRetry/onFork`）；编辑重跑带
  “对话 + 文件重置”恢复本轮文件语义（`zh-CN.ts:2780—2790`）。
- TUI：Esc 在 busy 时 `abortControllerRef.current?.abort()` 并提示“Pausing current
  output...”（`apps/zcode-cli/packages/tui/src/app-keyboard.ts` 第 298—306 行）；每 turn
  一个 AbortController（`app-submit.ts` 第 220—278 行）。

**TUI 会话流**——`apps/zcode-cli/packages/tui/src/`：

- `Message {role: agent|system|timeline|user, parts: TranscriptPart[]}`（`app-model.ts`
  第 20—30 行）；TranscriptPart 三类：text／thought（`thinking|thought` 两态）／tool。
- 流式增量：`appendStreamingTextDelta`／`appendStreamingThoughtDelta` 按 assistantMessageId
  原位追加 delta，无 id 则新建 `streamProjected` 消息（`app-transcript-stream.ts` 第
  5—30 行）；终结 `markStreamingMessageComplete`／`finalizeStreamProjectedMessages`。
- 容器：`scrollbox` stickyScroll＋stickyStart=bottom＋viewportCulling（
  `app-transcript-components.tsx` 第 39—46 行）；用户消息上下各留 1 行（第 21 行）；
  markdown 渲染 `app-markdown.tsx`。

**Web 壳**：`packages/web/src/main.tsx` 挂载 `@zcode/ui` App；移动远程控制壳
`packages/web/src/web-remote-control/`（ui/ 下 8 文件为连接／设备／扫码页），对话本体复用
同一 v4 组件栈（`packages/web/package.json` 依赖 `@zcode/ui: workspace:*`）。

### 1.2 交互形态描述

一轮对话＝一个 turn 组：轮首用户输入气泡（`rounded-xl rounded-tr-xs border-border
bg-surface px-4 py-3`，624px 以上最大宽 xl——`v4/ConversationUserInputBubble.tsx:27`）；
随后“已工作 {duration}／工作中 {duration}／已停止”可折叠状态条（`AssistantHistoryStatus`，
`ConversationTurnGroup.tsx` 第 640—701 行；文案 `zh-CN.ts:5322—5330`），收起时只留一行
耗时摘要；轮尾最终正文**不进折叠区**，只有最新正文带 fork／retry／编辑／复制。运行期输入
框变停止键。消息按角色分区的五色轨迹 token
（`--color-trajectory-user/assistant/reasoning/tool-call/tool-result`，日间 `#2563eb/
#0f766e/#7c3aed/#d97706/#0284c7`，夜间 `#60a5fa/#2dd4bf/#a78bfa/#f59e0b/#38bdf8`，
`styles.css:176—180,520—524,348—352,684—688`）供轨迹时间线（`ModelTrajectoryPane` 族）
使用。

### 1.3 对 WHCode 的可迁移要点

- GUI(iced)：turn 建模为渲染单元（轮边界＋用户行＋工作折叠段＋最终正文）；折叠态默认
  一行耗时摘要，中断／失败强制展开；正文流式增量复用 `run_turn_with_progress`，markdown
  最小集对应 streamdown 的标题／列表／代码块／行内码（FR-741）。
- Web：历史区＋实时尾双通道；WS 逐块 delta 原位追加与 `appendStreamingTextDelta` 同构；
  无 JS 降级显示有界尾部快照（FR-742）；停止入口放输入框右缘＋Esc；重试／编辑只挂最新
  正文。
- bodyRefs preview＋按需回填、toolSlice 游标直接对应“消息列表有界”要求（NFR-215）。

---

## 2. 工具调用卡（勘察面②）

### 2.1 ZCode 实现（源码锚点）

**状态机——两套命名体系并存（本次关键发现）**：

1. **v4 协议 row 状态**（权威）：`toolCall.status = "inputStreaming" | "pendingApproval" |
   "running" | "success" | "error" | "cancelled"`（`packages/shared/src/zcode-protocol-v4/
   rows.ts:212`）；`subagent.status = "running" | "success" | "failed" | "cancelled"`
   （rows.ts:272）。`pendingApproval` 时 `approvalInteractionId` 指向 pendingInteractions
   项；`error` 必带 `{code, message}`；运行中进度 `progress: ToolProgress` 终态清除；
   Bash 有界 `outputPreview` 终态或后台移交时清除。
2. **legacy compact 摘要状态**（AI SDK 投影）：`"input-streaming"→pending`、
   `"input-available"→running`、`"output-available"→completed`、`"output-error"→failed`、
   `"output-denied"→denied`，另有 rawStatus `stopped`（中断）——
   `packages/shared/src/tool-call-summary.ts`（映射表第 38—44 行；stopped 分支第
   195—203 行）。两套并存：v4 是 wire 协议，compact 是旧摘要消息面的展示态。

**进度**：`ToolProgress {bytes?, previewLine?, fraction?, total?, message?≤200 字,
updatedAt}`（`packages/shared/src/zcode-protocol-v4/toolDisplay.ts` 第 189—199 行），
MCP `notifications/progress` 投影到行。

**卡片骨架**——`packages/ui/src/ToolCallBlocks/ToolLayout.tsx`（363 行）：

- 展开状态按 `toolId` 进程内持久（`toolLayoutOpenState: Map<string, boolean>`，第 10 行）；
  `forceOpen/autoOpen/autoCollapseOnComplete`；折叠卸载延迟
  `TOOL_CONTENT_COLLAPSE_UNMOUNT_DELAY_MS = 300`（第 12 行）。
- 摘要行（`ToolSummaryRow.tsx`）＝图标＋kind 标签＋来源徽章＋主文本＋次文本＋diff 计数＋
  状态词＋状态指示物（空环灯）；窄屏 @max-[360px] 按 `prioritizePrimaryText` 隐藏标签
  保主文本（第 66—92 行）。
- 进度指示（`ToolCallProgressIndicator.tsx`）：有 fraction 用 determinate 细进度条
  （`h-1 w-16 rounded-full bg-brand`，width 200ms 过渡），否则 indeterminate 扫光
  （1.2s ease-in-out infinite，`styles.css:998—1010`）。

**参数与输出的展开／截断**：

- 终态 output 统一 **head+tail 截断（R-10）**，超出 `{totalBytes, ref}` 按需回拉
  （`toolDisplay.ts` 第 156—188 行 `toolOutputSchema`）；display 载荷信封层
  `.optional().catch(undefined)` 不设门——解析失败只降级为“无载荷”，不拒整条 row
  （第 158—178 行记录 2026-09-21 会话卡死事故根因）。
- Bash 运行中有界预览：`packages/shared/src/execution-output-preview.ts`
  `OUTPUT_PREVIEW_MAX_CHARACTERS = 4096`。
- 结构化 display kind 查表解析：`packages/ui/src/ToolCallBlocks/toolResultDisplay.ts`
  （`task_output` ≤2000 字符；`cua` 媒体 ≤2 图、单图 base64 ≤200KB；工作流 display 用
  shared strict schema 逐 kind safeParse）。
- 参数渲染按工具族分派：`renderers/` 40+ 渲染器（`edit.tsx`／`execute.tsx`／`agent.tsx`/
  `cua.tsx`／`skill.tsx`／`changes-group.tsx`／`fallback.tsx` 等）。
- CLI 侧 head+tail 的具体常量未在本次勘察中定位到定义行 `[待验证]`（见 §10 #3）。

**耗时呈现**：轮级“已工作 {duration}”（`turnHeader.activeMs` 权威，UI 不反推，
`conversationTurnRenderUnits.ts` 第 185—200 行）；后台任务卡独立计时
（`packages/ui/src/BackgroundTaskElapsedLabel.tsx`：≥1 分钟“m 分 s 秒”，mountedAt 基线与
job.elapsedMs 取大者防回退）；工具行自带 `startedAt/endedAt`（rows.ts 第 232—233 行）。

**TUI 工具卡**——`apps/zcode-cli/packages/tui/src/app-tool-components.tsx`：

- `ToolTranscriptPart.status = "pending" | "running" | "completed" | "failed"`
  （`app-model.ts` 第 66 行）；缺省标题“Tool {toolName} {status}”；detail 缩进 2 列、
  输出缩进 4 列；**输出上限 `MAX_OUTPUT_LINES = 8`**（第 11 行）；diff 结果转
  ShikiDiffView（§5）；错误行用 `palette.danger`。

### 2.2 交互形态描述

工具调用默认收起为一行摘要（图标＋kind 词＋目标路径＋状态词），运行中附细进度条或扫光；
点击整行展开参数（代码块）与输出（head+tail 截断文本，超出按 ref 回拉完整输出）；失败卡
带可复制错误 tooltip；多文件编辑聚合成 changes-group 单卡（响应式文件 chip，放不下折为
“+N”）。状态链：inputStreaming（参数流式输入中）→ pendingApproval（卡上出现审批入口）→
running → success／error／cancelled。

### 2.3 对 WHCode 的可迁移要点

- FR-743 状态映射建议取 v4 六态命名（`ToolCallStarted/Finished` 事件映射
  streaming→running→success/error/cancelled；pendingApproval 待 B3a broker）；
  denied／stopped 两展示态（compact 面）对应 WHCode `permission_denied` 不可重试语义。
- GUI(iced)：一行摘要＋展开两段（参数／输出）；展开状态会话内存 Map 即可；进度条
  determinate/indeterminate 两态。
- Web：摘要行 CSS 截断＋窄屏降级；截断常量（预览 4096、display 2000）登记进 token；
  ref 回拉对应既有 `view.diff`／导出管线。
- renderer 起步小集：edit／execute／agent（子代理）／fallback 四个。

---

## 3. 审批交互流（勘察面③）

### 3.1 ZCode 实现（源码锚点）

**三套并存的实际枚举（本次关键发现，锚点均直读复核）**：

1. **broker 决定枚举** `PermissionDecision = "allow" | "deny" | "escalate" | "modify"`
   ——两处一致：`apps/zcode-cli/packages/contracts/src/events/session.events.ts:1141` 与
   `packages/shared/src/zcode-protocol-legacy-types.ts:44`
   （`zcodePermissionDecisionSchema`）。escalate／modify 是正式决定值。
2. **规则行为枚举** `PermissionRuleBehavior = "allow" | "deny" | "ask"`
   （`apps/zcode-cli/packages/contracts/src/interfaces/permission.port.ts:10`；镜像
   legacy-types.ts:45）。
3. **UI 选项 kind 枚举**（v4 协议）
   `kind: z.enum(["allowOnce", "allowAlways", "deny", "custom"])`
   （`packages/shared/src/zcode-protocol-v4/snapshot.ts:238`）；另有 `fullAccessOption`
   （optionId 固定 `"fullAccess"`、kind 强制 custom，snapshot.ts 第 58—60、245—251 行）；
   工作流确认窗第三选项 Refine 的稳定 optionId
   `WORKFLOW_REFINE_PERMISSION_OPTION_ID = "workflowRefine"`
   （`zcode-protocol-legacy-types.ts:53`）。

**选项策略（防“记住规则”失效）**：`PermissionOptionsPolicy =
"no-always-allow" | "session-always-allow"`（permission.port.ts 第 45—52 行）——每次
代码都不同的工具（CreateWorkflow 脚本）禁用永久 always-allow，或降为仅存活于运行时内存的
session 级授权。

**决定载荷**（`PermissionBrokerResult`，permission.port.ts 第 74—97 行）：

- `permissionUpdates: PermissionUpdate[]`（`{type: "addRules", behavior, rules:
  [{toolName, ruleContent?}]}`）——**记住规则的持久化通道**（写入
  `PermissionRuleset {allow?, deny?, ask?, scope?: "session"}`）；
- `sessionPermissionUpdates`——仅运行时内存 ruleset，**永不持久化**（broker 应答侧合成，
  wire schema strict）；
- `modifiedInput`／`inputAdjustments`——确认窗内用户改过的输入字段（keyed by 工具字段名，
  `null`＝恢复默认；仅随 allow、且仅由声明 `applyInputAdjustments` 的工具应用）＝**改写
  输入入口**；
- `reason`／`preserveReasonFormatting`／`reasonSource`（`plan_approval_feedback |
  workflow_refine_feedback`）——deny 带理由可升级为真实用户消息。

**桌面／Web 审批 UI**——`packages/ui/src/PermissionDialog.tsx`（1104 行）：

- 选项按 `getPermissionOptionDisplayKind` 归一显示类（allowAlways／allowOnce／
  rejectAlways／rejectOnce／custom，字符串包含规则，`lib/permissionRequest.ts` 第 30—56
  行），排序 allowOnce→allowAlways→rejectOnce→rejectAlways（第 71—80 行）；
- 文案（`i18n/locales/zh-CN.ts:6643—6662`）：“允许／始终允许／允许本会话／完全访问／
  始终允许此命令（命令前缀／仅此命令）／拒绝／始终拒绝”；键盘“Tab／上下键选择，回车
  确认”（:6659）；
- “记住规则”作用域展示截断 `PERMISSION_RULE_SCOPE_MAX_DISPLAY_CHARS = 160`
  （PermissionDialog.tsx 第 90 行）；
- deny 反馈入口：`freeText`（snapshot.ts:241）开启“告诉模型接下来应该怎么做...”反馈框
  （`zh-CN.ts:443—444`）；上限 `MAX_PERMISSION_FEEDBACK_CHARS = 4_096`
  （snapshot.ts:234）；提交前 trim，纯空白落普通 deny（PermissionDialog.tsx 第 662 行
  注释）；
- 来源徽章：subagent 发起显示来源（`v4/pendingInteractionAdapter.ts` 第 83—85 行；
  `InteractionRequestOriginBadge.tsx`）；工具自报确认预览走独立 `display` 通道（与工具卡
  同一投影形状，adapter 第 86—88 行）；
- 工作流确认窗内直接改设置：`packages/ui/src/permissionWorkflowSettings.ts`（设置只随
  放行应答走，拒绝不带；Shift+Tab 回焦点）；
- MCP 专用权限图标 `MCP_PERMISSION_TOOL_ICON`（PermissionDialog.tsx 第 52 行）与 CUA
  规则常量 `OFFICIAL_CUA_PERMISSION_RULE_TOOL_NAME`（legacy-types.ts:47）。

**TUI 审批**——`apps/zcode-cli/packages/tui/src/`：

- `ApprovalDecision = "allow_once" | "allow_project" | "deny"`（`app-model.ts` 第 119、
  246 行）；`approvalMode: "user-once"` 降为 `["allow_once", "deny"]`
  （`app-approval.ts` 第 8—10 行）；上下键选择、回车确认、**Esc＝deny**（第 36—38 行）；
- bash 规则作用域预览 exact／prefix（`ruleContent` 以 `:*` 结尾判前缀，最多 5 条，
  `app-approval.ts` 第 54—72 行）；CUA 项目级授权特判文案（第 61—67 行）。

**会话／任务模式族**（域 C 交互语境）：`zcodeSessionModeSchema = plan | build | edit |
yolo | guarded | auto`（legacy-types.ts:76）；`ZCodeTaskMode` 11 值含 dontAsk/
bypassPermissions（`zcode-task-types-core.ts` 第 149—161 行）。

### 3.2 交互形态描述

权限请求以模态对话框出现在会话流：标题“需要权限”＋工具名＋摘要＋涉及文件／命令预览
（可含工具自报结构化预览），下方选项按钮组（允许 → 始终允许，可展开作用域“命令前缀／
仅此命令” → 拒绝，可展开反馈输入），危险工具另有“完全访问”项。选“始终允许”由应答携带
addRules 落规则集；“允许本会话”只进内存；确认窗内可直接改参数后放行（modify）；拒绝可留
自由文本反馈给模型（escalate 反馈通道）。TUI 同语义三选项上下键呈现。

### 3.3 对 WHCode 的可迁移要点

- B3a broker（FR-744）：对齐 `PermissionDecision` 四值＋`PermissionUpdate` 规则更新＋
  `sessionPermissionUpdates` 内存级分离；`no-always-allow` 策略是记住规则的防滥用闸
  （与 FR-604 拒绝不可重试不冲突）。
- B3b 双端 UI（FR-745）：四态决定映射 allowOnce／allowAlways(project)／deny／fullAccess；
  作用域展开（前缀／精确）与 deny 反馈输入是低成本高价值补充；`approvalMode: "user-once"`
  即破坏性工具默认确认；escalate／modify 完整语义（Refine 反馈升级、窗内改参）登记为
  v2 预留（WHCode 无动态工作流工具，但 broker 载荷可预留字段位）。

---

## 4. todo／任务面板（勘察面④）

### 4.1 ZCode 实现（源码锚点）

**数据结构**：

- 工具契约：`TodoItem {content, status: pending|in_progress|completed, priority:
  high|medium|low}`；TodoWrite 约束“至多一项 in_progress”（并行 subagent 场景的硬拒绝
  已注释放开）（`apps/zcode-cli/packages/contracts/src/tools/todo.ts` 第 11—17、59—75
  行）；`TodoWriteOutput {oldTodos, todos, summary {total, pending, inProgress,
  completed}}`（第 77—100 行）。
- UI 投影：`ZCodePlanStep {id, title, status: pending|in_progress|completed}`
  （`packages/shared/src/zcode-task-types-core.ts` 第 636—640 行）——UI 面板消费的投影
  **无 priority 字段**，优先级只在工具层。

**展示结构**——`packages/ui/src/TodoPanel.tsx`（192 行）：

- 会话流内嵌可折叠 Plan 面板（`components/ai-elements/plan.tsx`＋`queue.tsx` 基元）；
  标题行＋等宽字体计数 `n/m`（第 112—117 行）；
- **全部完成整板隐藏**（第 64—69 行 Bugfix 注释：防“已完成 todo”误导）；展开态
  **只列未完成项**（第 73 行），完成项靠计数表达；
- 运行中：`in_progress` 项标题 `animated-gradient-text` 渐变（第 33—37 行），标题行转
  菊花＋当前活动步标题（第 119—126 行）；非运行期降级普通状态色（第 39—45 行：停止后
  不残留活动动画）；
- 状态色：pending `text-foreground-subtlest`、in_progress `text-foreground`、completed
  `text-success`＋删除线（第 27—45 行）。

**TUI 侧栏 todos 段**——`app-sidebar.tsx` 第 277—294 行：进度行 `n/m`＋前 6 条
（`slice(0, 6)`）状态 marker＋按状态着色＋截断；侧栏 todos 段缺省展开。

### 4.2 交互形态描述

todo 面板是会话流内嵌轻量折叠条（非侧栏）：收起时“当前活动步标题＋n/m”，展开后垂直列
未完成项，进行中项渐变呼吸。会话级任务管理另在侧栏 TaskList（`packages/ui/src/
TaskList.tsx`）分层承担：todo＝turn 内执行清单，task＝会话容器。

### 4.3 对 WHCode 的可迁移要点

- FR-747 套四条规则：全完成隐藏、展开只列未完成、进行中强调（GUI 用静态强调色即可，
  渐变动画登记有意差异）、计数 n/m 等宽；数据面 `{id, title, status}` 最小集对齐
  `task.overview`（依赖与重试计数按 WHCode 语义扩展，ZCode 无重试计数字段）；
  “停止后不残留活动态”写入判据。

---

## 5. diff 呈现（勘察面⑤）

### 5.1 ZCode 实现（源码锚点）

**对话流内 diff（工具卡）**：

- 单文件编辑卡：`renderers/edit.tsx`——可折叠展开内联 diff（
  `renderers/EditInlineDiffContent.tsx`：**首帧只渲染轻量 hunk 文本**（无 token 化，防大
  diff 展开掉帧），effect 里异步补 Shiki 高亮 token，第 66—77 行注释）；完整 diff 经
  `buildEditCodeViewerSource` 送侧栏 CodeViewer（type: "patch"）。
- 多文件编辑聚合：`renderers/changes-group.tsx`——单卡响应式文件 chip 列表：逐 chip
  测宽，放不下折为“+N”（`resolveResponsiveFileChipCount` 第 29—57 行；零可用宽度返回 0
  保留完整 +N 提示）。
- 护栏：`packages/ui/src/lib/patchDiffPreview.ts` 第 3—6 行——
  `MAX_PATCH_DIFF_SAFE_LINE_COUNT = 1200`、`MAX_PATCH_DIFF_SAFE_CHAR_COUNT = 180_000`、
  hunk 行号上限 1200、纯文本降级 800 行；超限显式“已省略 N 行”标记（
  `lightweight-diff-preview.tsx` 第 143 行），标记可解析回行数
  （`parseTruncatedMarkerOmittedLineCount`）。
- 行配色：added/removed/context 三类——背景 `color-mix(in srgb, var(--color-diff-added)
  14%, transparent)`、行号槽 18% 混底、**3px 内嵌左色条**（inset boxShadow）——
  `lightweight-diff-preview.tsx` 第 82—116 行。

**Git 面板 diff**：

- `packages/ui/src/GitPaneChangeCard.tsx`：每文件一张折叠卡（Chevron 展开），按可用性
  分派 `getGitPaneDiffPreviewPlan`（`GitPane/helpers.ts` 第 143—150 行：rich＝DiffViewer
  完整视图；plain-text＝patch 不可安全渲染时降级纯文本预览）；大 diff 用 memo cacheKey
  防浅比较失效（第 62—70 行注释）；预览面板另有
  `previewPanePatchFallbackContent.tsx`（DiffViewer＋高亮＋语言推断＋纯文本回退）与
  `previewPaneCodeReview.ts`。
- 每轮文件变更摘要：`turnHeader.fileChanges {additions, deletions, files}`（rows.ts 第
  104—110 行）驱动 `v4/ConversationFileSummaryPanel.tsx`；消息级摘要
  `MessageChangeSummaryPanel.tsx`＋`lib/taskChangeSummary.ts`。

**TUI diff**——`apps/zcode-cli/packages/tui/src/app-shiki-diff-view.tsx`：

- **宽度自适应双视图**：终端宽 >120 列 split（双栏），否则 unified（第 13、95—98 行）；
  4 列行号 gutter（第 14 行）；Shiki 高亮按行异步补 token（cacheKey 失效重算，第 60—86
  行）；`truncated` 尾部显式标记（第 91—93 行）。
- 数据面 `ToolResultDisplay {structuredPatch: [{oldStart, oldLines, newStart, newLines,
  lines}], truncated, filePath, filetype}`（`app-model.ts` 第 73—94 行）。

### 5.2 交互形态描述

工具卡内 diff＝默认收起、展开先纯文本 hunk 后高亮增强；Git 面板＝文件列表→单文件折叠卡→
完整 DiffViewer（行号＋高亮＋hunk 分隔）；超限不渲染、显式省略标记；两处共用同一套
added/removed 色与 3px 左条语言。TUI 按终端宽自动切分双栏／单栏。

### 5.3 对 WHCode 的可迁移要点

- B5 diff 增强：GUI(iced) 用文本块渲染 unified diff（±前缀着色＋容器边框模拟左条）；
  Web 复用 `view.diff`，首帧纯文本＋异步增强与无 JS 降级天然同构。
- 护栏数值（1200 行／180k 字符／800 行降级）登记进前端预算审计口径；省略标记必须显式
  （信号链完备：区分正确的空与截断的空）。

---

## 6. 子代理／技能／MCP 面板（勘察面⑥）

### 6.1 ZCode 实现（源码锚点）

**子代理**：

- 对话流内：`subagent` row（rows.ts 第 268—290 行）渲染为 Agent 工具卡
  （`v4/ConversationAgentToolCallRow.tsx`）；`childSessionId` 存在即可下钻——点击打开
  **子会话侧栏**（`app-shell/SubagentSessionSidePane.tsx`，订阅
  `conversation/<childSessionId>`，**不内嵌 child rows**——rows.ts 第 283—285 行注释）；
  下钻入口与 tab 标题共用同一 title resolver（第 47—48 行注释）。
- 管理页：`settings/SubagentsSection.tsx`（列表＋表单：名称／提示词／模型选择含 thought
  级别／启用开关）。
- 颜色：`lib/subagentColors.ts` 第 3—23 行——8 色 AgentColor（yellow/red/orange/green/
  cyan/blue/purple/pink），按名字 hash 定色（第 40—43 行），双主题色类成对定义
  （`bg-sky-300 dark:bg-sky-400/32` 等）；另有节点色 token
  `--color-subagent-node[-hover|-foreground]`（zai-light `#6f7a2f` 系，`styles.css` 第
  327—329、500—502、664—666、829—831 行）。
- TUI：`app-sidebar-subagents.tsx`（侧栏段）＋`app-subagent-view.tsx`／
  `app-subagent-transcript.ts`（子代理转录镜像）。

**技能（Skills）**：

- 无独立“技能库页面”：技能经聊天输入 `/` 斜杠面板分节呈现（
  `packages/ui/src/slashCommandPanelSections.tsx` 第 52—72 行：skills 节含 loading/error/
  empty 三态）与 `@` 提及系统（`mentions/`）；工具卡面有专属渲染器
  `ToolCallBlocks/renderers/skill.tsx`；工作区文件树技能节点用 violet 系 token
  `--color-skill-node[-hover|-foreground]`（`styles.css` 第 321—323、494—496、658—660、
  823—825 行）。

**MCP**：

- 设置页：`settings/McpSettingsSection.tsx`＋`McpServerList.tsx`——每服务器行**状态
  圆点**（connected→green-500／connecting→yellow-500＋旋转 loader／error→red-500／
  disabled→foreground-subtlest，`McpServerList.tsx` 第 21—48 行）＋scope 徽章
  （workspace/user）＋**工具计数**（`settings.mcp.status.toolCount`，第 125—129 行）＋
  失败原因 tooltip（`McpFailurePresentation.tsx`）；表单 `McpServerForm.tsx`。
- TUI：`app-sidebar-mcp.tsx`＋`app-mcp-status.ts`（侧栏状态段）。
- 状态快照契约 `ZCodeMcpServerStatusSnapshot`；刷新失败分档
  `McpServerStatusListRefreshOutcome`（McpSettingsSection.tsx 第 99—102 行）。

### 6.2 交互形态描述

子代理＝对话流摘要卡（彩色标识＋状态）→点击下钻侧栏实时转录（两级，不内嵌）；技能＝
输入框斜杠面板按节浏览过滤（无常驻面板）；MCP＝设置页资源列表（状态灯＋工具数＋作用域
徽章），运行态在 TUI 侧栏有轻量状态段。

### 6.3 对 WHCode 的可迁移要点

- B4（FR-749／750）：子代理视图做“摘要卡＋下钻订阅”两级（GUI 侧栏、Web 侧栏页），不
  内嵌子转录防无界累积；技能库浏览 Web 做 `/` 面板等价（服务端直出分节列表）＋GUI 设置
  视图；MCP 面板四态灯（含 disabled）＋工具计数＋失败 tooltip 覆盖 FR-750 语义。
- 子代理配色 hash 定色（名字→8 色）零状态可复用；采纳则色值纳入 token 单一事实源
  （FR-740）。

---

## 7. 视觉 token 草表与 zcode-day／zcode-night 色值提案

### 7.1 主题体系与事实源

- 主题清单：`Theme = "light" | "dark" | "zai-light" | "zai-dark" | "system"`，其中
  light/dark **归一化为 zai-light/zai-dark**（`packages/ui/src/useTheme.ts` 第 27—31 行
  `normalizeThemePreference`），默认主题 **zai-dark**（第 71—73 行，注释“默认主题统一
  收敛到 Zai dark”）。即 **shipped 主题只有 zai 两套**（light/dark 为兼容别名）。
  **ZCode 无 AMOLED 纯黑主题**（全仓检索 oled/amoled 仅 carousel 无障碍属性误命中——
  修正 domain-M-frontend.md §2 对照表相应行，见 §8）。
- 样式事实源 `packages/ui/src/styles.css`（2279 行）：头部 import 链——`tailwindcss`
  （:1）、`tw-animate-css`（:2，动效工具类）、`shadcn/tailwind.css`（:4，radius/间距
  基础变量提供方）、`katex`（:6）；语义 token 在 `@theme`（第 143—338 行，亮色语义），
  `.dark`（340—511）为兼容暗色块，**`.theme-zai-light`（第 513 行起）与
  `.theme-zai-dark`（第 677 行起）为品牌主题全套字面值**（块头行号 grep 实证）。
- TUI 独立 token 全集：`apps/zcode-cli/packages/tui/src/theme/defaults.ts`
  （`DARK_TUI_THEME`／`LIGHT_TUI_THEME` 全字面 hex，第 8—126 行；思考流透明度 dark
  0.68／light 0.72，第 66、124 行）。
- 设计规范 `DESIGN.md`：间距（第 352—364 行）、圆角（第 366—429 行）、阴影（第
  556—569 行）、尺寸基线（图标 size-3—6、控件高 h-6—9）。

### 7.2 token 草表

**色板（语义 token → zai-light／zai-dark 字面值；出处＝styles.css 行号）**

| token | zcode-day（=theme-zai-light） | zcode-night（=theme-zai-dark） | 出处 |
|---|---|---|---|
| background | `#f8f8f8` | `#161616` | 516／680 |
| background-win-alt | `#ececee` | `#2b2b2b` | 517／681 |
| sidebar | `#f0f0f0` | `#161616` | 537／701 |
| panel／header | `#ffffff` | `#202020` | 535—536／699—700 |
| card／popover／input | `#ffffff` | `#2b2b2b` | 540,543,547／704,707,711 |
| surface | `rgba(13,13,13,0.03)` | `rgba(255,255,255,0.05)` | 538／702 |
| surface-hover | `rgba(13,13,13,0.05)` | `rgba(255,255,255,0.10)` | 539／703 |
| hover | `rgba(13,13,13,0.05)` | `rgba(255,255,255,0.05)` | 533／697 |
| selected | `rgba(13,13,13,0.05)` | `rgba(255,255,255,0.10)` | 534／698 |
| border | `rgba(13,13,13,0.10)` | `rgba(255,255,255,0.10)` | 531／695 |
| border-hover | `rgba(13,13,13,0.15)` | `rgba(255,255,255,0.15)` | 532／696 |
| brand | `#000000` | `#ffffff` | 519／683 |
| primary（primary-foreground 反向） | `#000000`（`#ffffff`） | `#ffffff`（`#000000`） | 597—598／761—762 |
| foreground | `var(--color-neutral-800)` | `var(--color-neutral-300)` | 609／773 |
| foreground-subtle | neutral-800 @60% | neutral-300 @60% | 610／774 |
| foreground-subtlest | neutral-800 @40% | neutral-300 @30% | 612／776 |
| accent | `#ebf4ff` | `#001d3d` | 528／692 |
| success | `#1e8a3e` | `#46bf72` | 615／779 |
| destructive | `#e03131` | `#ff5c5c` | 619／783 |
| warning | `#e07b00` | `#ff8a30` | 621／786 |
| diff-added（前景） | `#1e8a3e`（`#ffffff`） | `#46bf72`（`#000000`） | 628—629／793—794 |
| diff-removed（前景） | `#e03131`（`#ffffff`） | `#ff5c5c`（`#000000`） | 630—631／795—796 |
| find-highlight（active） | `#fff4eb`（`#ffb26b`） | `#542500`（`#ff8a30`） | 529—530／693—694 |
| tag（行内码底） | `#e6e6e6` | `#363636` | 645／810 |
| input-border-focused | =border-hover（**非** brand） | 同左 | 551／715 |
| node 色系（file/skill/command/subagent/session/plugin） | 各自 0.10/0.16 透明底＋实色前景（如 subagent `#6f7a2f`） | 各自 0.16/0.22＋浅前景（如 subagent `#c8cd90`） | 655—674／820—839 |

注①：foreground 系经 Tailwind 调色板间接引用；`--color-neutral-800`／
`--color-neutral-300` 具体 hex 在本克隆内无字面定义（node_modules 未随克隆；`@theme`
未覆写），Tailwind v4 公开默认值 `#262626`／`#d4d4d4`——**冻结前须以运行时
`getComputedStyle` 复核**，标 `[待验证]`（§10 #1）。
注②：zai-dark 背景 `#161616` **非纯黑**，与 WHCode `amoled-night` 的 `#000000` 铁律语义
不同——两套主题并存时不得互相改写。
注③：终端 ANSI 十六色两组字面值已备（zai-light 第 558—573 行、zai-dark 第 722—737 行；
TUI 组见 `tui/theme/defaults.ts`），GUI/Web 若做终端面板可整体引用，本表不展开。

**间距阶梯**（DESIGN.md 第 352—364 行）：基单位 4px；节奏 4（图标紧排）／8（控件内距与
行内间隙）／12（密列表与菜单行）／16（标准卡与面板内距）／20—24（大区块与对话框）；
间距不随界面字号缩放（:299）。

**字号阶梯**（styles.css 第 150—166 行，基准 `--ui-font-size: 14px`，:76）：

| token | 值 |
|---|---|
| text-ui-2xs | 基准−5px（仅工作流图轴面专用刻度） |
| text-ui-xs | 基准−4px |
| text-ui-sm | 基准−2px |
| text-ui-caption | 基准−1px |
| text-ui-base | 14px（正文缺省） |
| text-ui-lg／xl | +2px／+4px |
| settings-title／title-lg | +10px／+16px |
| mobile-input-safe | 固定 16px（iOS 聚焦缩放下限） |

等宽栈显式加入 CJK 字形回退（`--font-mono` 含“Microsoft YaHei UI”等，:146—148——
Windows Consolas 不含中文的修复，WHCode Web 直出同样适用）。

**圆角**（DESIGN.md 第 366—429 行；类名阶梯由 `shadcn/tailwind.css` 提供，styles.css:4；
像素值未在克隆内第一手复核 `[待验证]`）：首个圆角容器从 `rounded-xl` 起，嵌套依次降到
lg→md→sm（最小 sm）；基础控件（按钮／输入）`rounded-lg` 并随最近圆角父级降级；菜单／
下拉浮层 `rounded-lg`（选项 hover `rounded-md`）；对话框／主输入壳／todo 浮动面板／
toast `rounded-2xl`（白名单封闭，:386—392）；`rounded-full` 仅限胶囊与圆；
用户输入气泡＝`rounded-xl rounded-tr-xs`（尾巴收小）。

**阴影／层级**（DESIGN.md 第 556—569 行）：三层——base 无阴影（靠背景对比）／overlay
`shadow-md`（菜单、气泡、对话框）／attention `shadow-lg`（仅 toast 与重要浮卡）；克制
深度，先背景对比、边框、圆角，再阴影。全局滚动条统一为透明轨道＋圆角 thumb（border 色，
styles.css 第 847—872 行）。

**动效时长**（styles.css 第 928—1200 行直读；`tw-animate-css` 提供工具类）：

| 动效 | 时长／缓动 | 锚点 |
|---|---|---|
| 内容列宽／位移过渡 | 150ms ease-out | ConversationTimeline.tsx:2059 等容器类 |
| 折叠展开／淡出 | 300ms ease-in-out | styles.css:1113—1130 |
| 输入瀑布 | 260ms cubic-bezier(0.22,1,0.36,1) | styles.css:1029—1044 |
| 流式文本进入 | 900ms cubic-bezier(0.16,1,0.3,1) | styles.css:1019,1093—1105 |
| 工具进度扫光（indeterminate） | 1.2s ease-in-out infinite | styles.css:998—1010 |
| 渐变文本底流（todo 活动项） | 4s linear infinite | styles.css:928,981 |
| 骨架 shimmer | 1.8s linear infinite | styles.css:1054—1073 |
| 反应气泡弹出 | 450ms cubic-bezier(0.34,1.56,0.64,1) | styles.css:1159 |
| 窗口不可见 | **全部禁用**（`html[data-document-hidden]` transition/animation none） | styles.css:91—96（防后台流式时动画对象无上限累积——与 WHCode FR-617 同哲学，互为印证） |
| 思考流透明度 | dark 0.68／light 0.72 | tui/theme/defaults.ts:66,124 |

### 7.3 zcode-day／zcode-night 精确 hex 提案表（冻结候选）

提案＝**直接采纳 theme-zai-light／theme-zai-dark 字面值**（出处见 §7.2 表末列）。WHCode
落地最小必需组（GUI Theme 枚举与 Web THEMES 扩展所需，其余按 FR-740 由单一事实源派生）：

| 语义 | zcode-day | zcode-night | 源码出处（styles.css） |
|---|---|---|---|
| 背景（窗口／画布） | `#f8f8f8` | `#161616` | 516／680 |
| 前景（正文） | `[待验证]#262626`（neutral-800 运行时复核） | `[待验证]#d4d4d4`（neutral-300） | 609／773 |
| 前景-次级（subtlest 等效） | `[待验证]`（neutral-800@40%） | `[待验证]`（neutral-300@30%） | 612／776 |
| 强调（brand/primary） | `#000000` | `#ffffff` | 519,597／683,761 |
| 强调-背景面（accent） | `#ebf4ff` | `#001d3d` | 528／692 |
| 边框 | `rgba(13,13,13,0.10)` | `rgba(255,255,255,0.10)` | 531／695 |
| 边框-悬停 | `rgba(13,13,13,0.15)` | `rgba(255,255,255,0.15)` | 532／696 |
| 卡片／浮层 | `#ffffff` | `#2b2b2b` | 540,543／704,707 |
| 面板／页头 | `#ffffff` | `#202020` | 535—536／699—700 |
| 侧栏 | `#f0f0f0` | `#161616` | 537／701 |
| 成功 | `#1e8a3e` | `#46bf72` | 615／779 |
| 危险 | `#e03131` | `#ff5c5c` | 619／783 |
| 警告 | `#e07b00` | `#ff8a30` | 621／786 |
| diff 增行 | `#1e8a3e` | `#46bf72` | 628／793 |
| diff 删行 | `#e03131` | `#ff5c5c` | 630／795 |

落点：`crates/whcode-gui/src/lib.rs` Theme 枚举新增两变体＋`crates/whcode-web/src/
assets.rs` THEMES 扩展＋双端 hex 断言测试升级为六套主题全覆盖（FR-739／Check 344）；
**既有四套主题一个值都不动**（ADR-057 决策 2 铁律）。默认主题是否切为 zcode 系由用户
单独批准（默认值可见可改可撤销）。

---

## 8. 与既有报告的关系

- 接续 `domain-M-frontend.md`（形态级：Electron 四进程／Web／TUI 三形态、预览、扫码、
  分享）——本文不重复形态对照，专注其 §4 遗留的“TUI 侧栏卡片信息架构需交互实测”
  （本文 §1.1 已补 TUI 结构级勘察，交互手感仍待实机）与体验层七面。
- **修正 domain-M-frontend.md §2 一处判定**：其对照表称 ZCode 主题“多主题；深色/浅色/
  AMOLED”。本次直读 `useTheme.ts` 全文并全仓检索（amoled/oled 仅 carousel 无障碍属性
  误命中）确认：ZCode **无 AMOLED 主题**，shipped 为 light／dark／zai-light／zai-dark
  ＋system，默认 zai-dark；WHCode 的 amoled-night 纯黑是 ZCode 不具备的差异化能力。
- 对 domain-B／domain-C 的协议语义结论，本文以行级锚点复核并细化：工具状态**两套命名
  体系并存**（v4 row 六态 vs compact 展示态，§2.1）、审批**三套枚举并存**（§3.1），
  采纳时以本文为准；冲突以 ADR／specs 主账本为最终裁决。
- 本文结论只入 research（计划书 §2 纪律），不改 `docs/matrices/`；B1—B5 采纳时按
  FR-739—753 落账本。

## 9. 复核记录（三次复查）

1. **查证**：七面全部回到源码直读，无二手转述；关键枚举（toolCall.status、
   PermissionDecision、option kind、ApprovalDecision、TodoStatus）逐一定位定义行；
   主题块行号 grep 实证（`.theme-zai-light`=513、`.theme-zai-dark`=677）。
2. **交叉验证**：协议 schema（packages/shared）与 UI 消费侧（packages/ui）互证
   （pendingApproval／display catch 双侧一致）；桌面 Web 栈与 TUI 栈互证（审批 v4 四值
   与 TUI 三值并存双源确认）；`PermissionDecision` 双处定义一致（contracts:1141 与
   legacy-types:44）；本文件前一版中断稿中的独立发现（tool-call-summary 映射、
   Refine optionId、scope 截断常量）经重读源码逐条核实后并入本文。
3. **判定**：确证＝正文（含行号锚点）；存疑＝§10 逐条列明原因，不混入确证结论。

## 10. 存疑项汇总

| # | 事项 | 状态 | 原因与补充路径 |
|---|---|---|---|
| 1 | Tailwind v4 调色板间接引用值的具体 hex（neutral-800／neutral-300 及 subtle/subtlest 混合结果；§7.3 两处 `[待验证]`） | `[待验证]` | node_modules 未随克隆；`@theme` 无字面覆写。冻结时用运行时 `getComputedStyle` 复核（B1 双端 hex 断言取值时结清） |
| 2 | 圆角类名阶梯的像素值（sm/md/lg/xl/2xl）与 shadcn/tailwind.css 提供的间距变量具体值 | `[待验证]` | 同上（DESIGN.md 只锁类名阶梯）；同一运行时复核可一并取得 |
| 3 | CLI 侧 output head+tail 截断（R-10）的具体行数／字节数常量 | `[待验证]` | 协议层只锁 shape（truncated {totalBytes, ref}）；实现常量在 CLI 投影侧（apps/zcode-cli 内），本次未定位到定义行 |
| 4 | ConversationTimeline 历史尾窗（cold snapshot 保留行数）与 loadOlder 每页大小 | `[待验证]` | 常量未在直读段出现，可能在 CLI 快照侧；锚点：`conversationTurnRenderUnits.ts` 注释“冷恢复尾窗缺 header” |
| 5 | web-remote-control 移动端对话对 v4 组件的复用完整度 | `[待验证]` | `packages/web/package.json` 依赖 `@zcode/ui` 已证共用栈；逐组件复用清单未核对（web-remote-control/ui 仅 8 个壳层文件） |
| 6 | 子代理下钻侧栏（SubagentSessionSidePane）内部渲染细节 | 部分核验 | 入口与订阅模型已确证（childSessionId＋`conversation/<child>` 订阅）；侧栏内部分块未逐行读 |
| 7 | TUI markdown 主题（`app-markdown-theme.ts`）与主题同步（`app-theme-sync.ts`）细节 | 未逐行核验 | TUI token 全集已取得（theme/defaults.ts）；markdown 元素到 token 的映射未展开 |
| 8 | 交互手感（动画节奏、折叠跟手度）未经运行实测 | `[待验证]` | 静态源码阅读；B1 实现批以 ZCode 实机对照复核（并入批次 R 失效条件） |
| 9 | zai 主题 bright terminal 色（styles.css:566—573、730—737）已提取但 WHCode 暂无终端面板消费方 | 登记备用 | 非缺口；做终端面板时整体引用 |
