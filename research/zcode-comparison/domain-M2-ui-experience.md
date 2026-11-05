# 域 M2：ZCode 前端体验层（工具卡／审批流／todo／diff／面板／视觉 token）

> **基线**：`local/zcode-src` commit `3c5b2276`（勘察日期 2026-11-06）。
> **用途**：批次 R（Task T249，ADR-057 已批准）产出——把“体验差距”变成可勘察、可登记、
> 可判定的清单，供 B1—B5 实现批与 zcode-day/night 主题色值冻结使用。
> **失效条件**：ZCode 基线演进（本文件头部 commit ≠ `local/zcode-src` HEAD 且 UI 相关差异
> 清单非空）即触发复审（ADR-057 复审触发，绑定判定＝头部 commit 比对）。
> **证据层级**：第一层＝源码；每个结论附 `local/zcode-src` 下相对路径锚点。无法核验处
> 显式标 `[待验证]`；三次复查未通过的结论不写“确证”。
> **范围**：接续 `domain-M-frontend.md`（形态级），本报告只补**体验级**七面；权限协议语义
> 见 domain-C、工具系统见 domain-B、计划模式见 domain-G，本报告不重复。

## ① 对话流信息架构

- **实现锚点**：`packages/ui/src/lib/taskChatMessageTypes.ts`（消息类型，`status: string`）；
  `packages/ui/src/App.tsx`（装配根）；`packages/ui/src/LexicalChatInput.tsx`（富文本输入，
  Lexical 编辑器）；`packages/ui/src/styles.css`（`@source ".../streamdown/dist/*.js"`——
  流式 markdown 渲染采用 **streamdown** 库；`@import "katex"`——公式渲染）；轨迹时间线
  `ModelTrajectoryPane/Timeline.tsx`。
- **交互形态**：消息按角色分区并以五色 token 区分（`--color-trajectory-user/assistant/
  reasoning/tool-call/tool-result`，zai-light 块 styles.css:520-524 / zai-dark 块 684-688）；
  流式正文经 streamdown 渐进渲染 markdown；输入区为 Lexical 富文本＋工具条
  （`chat-input-toolbar/`），停止/重试入口在工具条而非消息行。
- **对 WHCode 的可迁移要点**：GUI（iced）消息流按五角色分区＋状态色 token（对应
  `ToolCallStarted/Finished` 事件链）；Web 保持服务端直出 markdown 最小集＋WS 增量
  （streamdown 是 React 库不可复用，语义对齐即可）；KaTeX 公式渲染属预算外，不做
  （Web DOM/JS 预算与 GUI 渲染成本，FR-741 已限定最小集）。

## ② 工具调用卡

- **实现锚点**：`packages/ui/src/ToolCallBlocks/`——`ToolLayout.tsx`（卡壳）、
  `ToolSummaryRow.tsx`（摘要行）、`ToolPresentation.tsx`、`toolResultDisplay.ts`、
  `fileSummaries*.ts`（文件摘要启发式）、`renderers/{edit,execute,web-fetch,search,skill,
  fallback}.tsx`（按工具族分渲染器）；状态映射 `packages/shared/src/tool-call-summary.ts:39-43`。
- **状态机（实测枚举）**：AI SDK 工具状态→展示态——`input-streaming`→pending、
  `input-available`→running、`output-available`→completed、`output-error`→failed、
  `output-denied`→**denied**；另有 `stopped`（中断态，`getCompactToolCallStatusMessageId`
  tool-call-summary.ts:197-203）。
- **交互形态**：收起时只留摘要行（图标＋kindLabel＋primaryText＋statusLabel＋diffCount），
  点击经 Collapsible 展开；展开内容卸载延迟 300 ms（`TOOL_CONTENT_COLLAPSE_UNMOUNT_DELAY_MS`，
  ToolLayout.tsx:15）；打开状态按 toolId 进程内持久（`toolLayoutOpenState` Map）；支持
  `autoOpen`／`autoCollapseOnComplete`；摘要行带复制按钮；状态词配指示物（空环灯）与
  悬停提示（statusIndicator/statusTooltip）。
- **对 WHCode 的可迁移要点**：FR-743 工具卡＝“摘要行＋可展开”两态；状态五值
  （pending/running/completed/failed/denied/stopped）直接映射 WHCode `EventKind`；
  GUI 起步 renderer 小集＝edit/execute/search/fallback 四个；Web 卡片服务端直出同结构；
  打开状态持久化属交互细节，v1 可不做（登记为有意差异）。

## ③ 审批交互流

- **实现锚点**：`packages/ui/src/PermissionDialog.tsx`（审批对话框）；协议类型
  `packages/shared/src/zcode-task-types-core.ts:646-680`（`ZCodePermissionRequest`：
  `options[]` 每项 `{optionId, kind, name, description?, response}`；`freeText`＝拒绝可附
  反馈；`display`＝工具自报确认预览，复用工具卡同一投影形状）；决定枚举
  `packages/shared/src/zcode-protocol-legacy-types.ts:44`——
  **`"allow" | "deny" | "escalate" | "modify"`**（四态）；规则行为枚举同文件 :45——
  `"allow" | "deny" | "ask"`；响应携带 `modifiedInput`（改写输入）与
  `permissionUpdates[]`（记住规则）。
- **交互形态**：选项经 `sortPermissionOptions` 排序、`getPermissionOptionDisplayKind`
  决定按钮形态（PermissionDialog.tsx:41-46 import）；“记住规则”的 scope 展示截断
  160 字符（`PERMISSION_RULE_SCOPE_MAX_DISPLAY_CHARS`）；反馈文本上限
  `MAX_PERMISSION_FEEDBACK_CHARS`（@zcode/shared/zcode-protocol-v4）；请求带来源徽章
  （`InteractionRequestOriginBadge.tsx`）；工作流确认窗有第三选项 Refine（拒绝附修改
  意见，`WORKFLOW_REFINE_PERMISSION_OPTION_ID`）。会话模式枚举
  `zcodeSessionModeSchema`（zcode-protocol-legacy-types.ts:76）＝
  plan/build/edit/yolo/guarded/auto；任务模式 `ZCodeTaskMode`（zcode-task-types-core.ts:
  150-161）11 值（含 dontAsk/bypassPermissions）。
- **对 WHCode 的可迁移要点**：FR-745 四态直接对齐——“批准”＝allow、“拒绝”＝deny、
  “记住规则”＝permissionUpdates（写本地规则存储、设置页可撤销）、“仅本次”＝allow 不带
  updates；escalate/modify（升级与改写输入）登记为 v2 预留（FR-745 判据不含，登记
  差异）；工具自报预览（display）对应 WHCode 工具卡投影复用，B3a broker 设计输入。
  会话模式族与 WHCode `authz::Capability` 的映射在 B3a ADR 中定。

## ④ todo／计划面板

- **实现锚点**：`packages/ui/src/TodoPanel.tsx`（数据型 `ZCodePlanStep`，状态
  `pending/in_progress/completed` 三值）；呈现组件 `components/ai-elements/plan.js` 与
  `queue.js`；状态色映射 TodoPanel.tsx:18-38。
- **交互形态**：面板可折叠；三态呈现——pending＝次级前景色、in_progress＝流式时动画
  渐变字（`animated-gradient-text`）／空闲时降级为普通前景色、completed＝删除线＋成功色；
  明确的 bugfix 注记：任务停止后不残留 in_progress 动画（空闲类替换流式类）。
- **对 WHCode 的可迁移要点**：FR-747 面板三态渲染规则照抄语义（pending 次级／
  running 强调／completed 删除线）；动画渐变字在 iced 预算内不做，用静态强调色
  （登记有意差异）；“停止后不残留活动态”写入 FR-747 判据。

## ⑤ diff 呈现

- **实现锚点**：`packages/ui/src/previewPanePatchFallbackContent.tsx`（`DiffViewer`＋
  `HighlightedLightweightDiffPreview`＋语言推断 `inferCodeLanguage`＋纯文本回退
  `getPlainTextPatchFallbackLines`）；变更摘要 `MessageChangeSummaryPanel.tsx`＋
  `lib/taskChangeSummary.ts`；git 变更卡 `GitPaneChangeCard.tsx`。
- **交互形态**：补丁视图带语法高亮；无法高亮时回退纯文本行渲染（保底不空白）；
  变更摘要按文件聚合（MessageChangeSummaryPanel）；预览面板按内容类型分渲染器
  （previewPaneCode/Markdown/Image/Pdf/Pptx/Xlsx/Office…，见 ui/src 根目录清单）。
- **对 WHCode 的可迁移要点**：T260（view.diff 增强）＝高亮渐进增强＋纯文本回退保底；
  变更摘要面板对应 WHCode `view.changes`（GUI 现为手动刷新，登记 ADR-056 后续项）；
  多类型预览面板不做（预算，登记有意差异）。

## ⑥ 子代理／技能／MCP 面板

- **实现锚点**：子代理＝轨迹时间线呈现而非独立面板——`ModelTrajectoryPane/Timeline/
  ExpandedContent.tsx`，节点色 token `--color-subagent-node/-hover`（zai 块）；
  技能＝工具卡一种（`renderers/skill.tsx`）＋斜杠面板（`slashCommandPanelSections.tsx`）；
  MCP＝设置页节（`SettingsPage.tsx:2129-2132`，`mode="mcp"`）＋权限块专用图标
  （`MCP_PERMISSION_TOOL_ICON`，PermissionDialog.tsx:52）＋ CUA 专用规则常量
  （`OFFICIAL_CUA_PERMISSION_RULE_TOOL_NAME`）。
- **交互形态**：子代理运行融入主时间线（reasoning/tool 角色分色），不做树状面板；
  技能经斜杠命令面板发现；MCP 服务器管理在设置页。
- **对 WHCode 的可迁移要点**：FR-749（子代理视图）v1＝运行状态卡（事件源既有
  子代理编排事件），轨迹时间线属 B4 可选增强（登记排期）；FR-750（技能/MCP 面板）
  ＝设置页节模式（Web 已有 `/settings/certs` 先例）；斜杠面板对应 WHCode 命令面
  （CUI 语义，GUI/Web 登记差异）。

## ⑦ 视觉 token 与 zcode 主题色值提案

- **主题机制**：`useTheme.ts`——`Theme = "light" | "dark" | "zai-light" | "zai-dark" |
  "system"`，存储键 `zcode-theme`，解析为 light/dark 两系；样式经 Tailwind v4 `@theme`
  语义变量（styles.css:143-339）＋ `.dark` 覆盖（:340）＋ **`.theme-zai-light`（:505 起）
  ＋ `.theme-zai-dark`（:675 起）** 两个具体主题块；Electron vibrancy 透明根背景由
  `--color-background` 反灌 `theme-color` meta（useTheme.ts:44-58）。
- **字体阶梯**：`--ui-font-size: 14px` 基准，text-ui-xl +4／lg +2／base ±0／caption −1／
  sm −2／xs −4／2xs −5 px（styles.css:150-166）；等宽栈显式加入 CJK 字形回退
  （styles.css:146-148，Windows Consolas 不含中文的修复）。
- **zai-light 精确值**（styles.css:516-565，zcode-day 提案来源）：
  background `#f8f8f8`／background-win-alt `#ececee`／brand `#000000`／foreground
  neutral-800（:609）／accent `#ebf4ff`／header·panel `#ffffff`／sidebar `#f0f0f0`／
  card·popover·input `#ffffff`／border `rgba(13,13,13,0.10)`／border-hover `0.15`／
  hover·selected `0.05`／surface `0.03`／surface-hover `0.05`／find-highlight
  `#fff4eb`→active `#ffb26b`；terminal 12 色：black `#5c5c5c` red `#e03131`
  green `#1e8a3e` yellow `#e07b00` blue `#0b7fff` magenta `#9e77ed` cyan `#0aa7a7`
  white `#adadad`（+bright 系未列全，:566 起）。
- **zai-dark 精确值**（styles.css:680-729，zcode-night 提案来源）：
  background `#161616`／background-win-alt `#2b2b2b`／brand `#ffffff`／foreground
  neutral-300（:773）／accent `#001d3d`／header·panel `#202020`／sidebar `#161616`／
  card·popover·input `#2b2b2b`／border `rgba(255,255,255,0.10)`／hover `0.05`／
  selected `0.10`／surface `0.05`／surface-hover `0.10`／find-highlight `#542500`→
  active `#ff8a30`；terminal 12 色：black `#363636` red `#ff5c5c` green `#46bf72`
  yellow `#ff8a30` blue `#4099ff` magenta `#7b5ce5` cyan `#42c8c8` white `#adadad`。
- **节点色**（zai-light 块）：command `rgba(86,98,112,0.10/0.16)`、subagent
  `rgba(111,122,47,0.10/0.16)`、session `rgba(20,128,122,0.10/0.16)`、plugin
  `rgba(184,122,26,0.10/0.16)`。
- **动效**：`tw-animate-css`；折叠卸载延迟 300 ms；**文档 hidden 时全局禁用动画**
  （styles.css:91-96，`html[data-document-hidden]`——防后台流式时内存无上限累积）。
  该设计与 WHCode FR-617（窗口不可见停绘）同哲学，互为印证。
- **对 WHCode 的可迁移要点（zcode-day/night 提案表）**：
  | token | zcode-day（←zai-light） | zcode-night（←zai-dark） |
  |---|---|---|
  | background | `#f8f8f8` | `#161616` |
  | background-alt(win) | `#ececee` | `#2b2b2b` |
  | foreground | neutral-800 `[待验证]` | neutral-300 `[待验证]` |
  | brand | `#000000` | `#ffffff` |
  | accent | `#ebf4ff` | `#001d3d` |
  | header/panel | `#ffffff` | `#202020` |
  | sidebar | `#f0f0f0` | `#161616` |
  | card/popover/input | `#ffffff` | `#2b2b2b` |
  | border | `rgba(13,13,13,0.10)` | `rgba(255,255,255,0.10)` |
  | border-hover | `rgba(13,13,13,0.15)` | `rgba(255,255,255,0.15)` |
  | hover / selected | `rgba(13,13,13,0.05)` | `rgba(255,255,255,0.05 / 0.10)` |
  | surface / surface-hover | `rgba(13,13,13,0.03 / 0.05)` | `rgba(255,255,255,0.05 / 0.10)` |
  | success/warning/destructive | 未提取（semantic 块 :279-286 引用_inverse）`[待验证]` | 同左 |
  | terminal 12 色 | 见上（light 组） | 见上（dark 组） |
  落点＝`crates/whcode-gui/src/lib.rs` Theme 枚举新增两变体＋`crates/whcode-web/src/
  assets.rs` THEMES 扩展＋hex 断言测试升级六套（FR-739/Check 344）；**既有四套主题
  （amoled-night 纯黑 #000000／eink 双主题真灰口径／terminal-green 纯黑+#00FF00）
  一个值都不动**（ADR-057 决策 2 铁律）。

## 与既有报告的关系

- 接续 `domain-M-frontend.md`（形态级对比：Electron 四进程／扫码／OAuth／分享等），
  本报告补体验级七面，不重复其内容。
- 协议层语义已在 domain-B（工具系统）、domain-C（权限）、domain-G（计划模式）登记；
  本报告只补 UI 呈现层，冲突时以那些域报告为准。

## 存疑项汇总

1. Tailwind v4 默认调色板具体 hex（neutral-50—950／sky 系）不在本仓源码内
   （位于 node_modules，未入库）→ `[待验证]`；zcode 主题若引用 neutral 值需在冻结时
   以渲染实测确认（B1 批次双端 hex 断言取值时结清）。
2. 圆角（--radius）与间距阶梯由 `shadcn/tailwind.css` 提供（styles.css:4 @import），
   本文件未见到具体值 → `[待验证]`；WHCode 提案暂用自家既有间距/圆角，不做对齐承诺。
3. zai-light／zai-dark 块的起止行号按前景值行号（609／773）与块头选择器
   （`.theme-zai-light`／`.theme-zai-dark`，awk 实证）交叉定位；块内 bright terminal 色
   （:566 起）未逐一提取 → B1 冻结时补齐。
4. `success/warning/destructive` 语义色只提取到 `_foreground: inverse` 引用
   （:279-286），底色未提取 → `[待验证]`。
5. TUI（apps/zcode-cli/packages/tui）本轮未勘察；ZCode 桌面/Web 体验以 packages/ui 为
   事实源。TUI 侧栏卡片信息架构维持 domain-M-frontend.md 的 `[待验证]`。
6. 本报告结论基于静态源码阅读；交互手感（动画时长、折叠动效）未经运行实测 →
   B1 实现批以 ZCode 实机对照复核（登记为批次 R 失效条件的一部分）。
