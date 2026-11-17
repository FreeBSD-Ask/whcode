# ADR-087：TUI 对话流重做（Chat 视图 v1）

- **状态**：已批准（2026-11-18，用户指令“先把 UI 做好”，选项确认落 TUI）
- **编号认领**：ADR-087（`ls docs/adr/` 复核，086 之后无占用）；FR-796；T376—T381；Check 472—475

## 任务认知卡

- **方向**：把 TUI 第 7 视图（`ViewId::Chat`）从占位面板升级为真对话界面——流式正文、
  工具卡、轮次边界、Esc 取消、历史回放；对齐 ADR-057 已落的 GUI 对话视图形态
  （FR-741/FR-745 底座已有）。
- **目标**：用户在 TUI 内完成“选会话 → 输入提示词 → 看流式回复 → Esc 停止 → 读历史”
  的完整闭环，不离开终端。
- **用户意图复述**：用户以 ZCode 的对话体验为参照，认为当前各端“查询表单式”的 LLM 交互
  不成话；选定先落 TUI（终端交互主力）。
- **非目标**：P0 工具解码修复（另批，ADR-087 不碰 `whcode-llm` 协议层）；TUI 交互审批卡
  （v1 登记“无人可问”结构化失败，GUI 已有交互卡）；Web/CUI 改动。
- **判据**：scripted 自动化与单测证明——delta 原位追加、Done 后正文以 `outcome.text`
  全量替换、工具卡由 `TurnSignal` 驱动（无信号即无卡，不伪造）、Esc 带确定性 request_id
  取消、提交门控四类结构化拒绝、行缓冲有界。
- **手段（后置）**：复用 `run_turn_with_progress`／`TurnObserver`／`tool_call::describe`；
  线程模型复用 ADR-056 Phase 2 的“有损通道＋完成保达”模式。

## 问题

TUI 的 Chat 视图当前为占位（`render.rs` 对 `ViewId::Chat` 渲染 0 行、无按键处理）；`llm`
交互走状态行两步输入（剖面名→提示词），同步阻塞等待，结果只显示“最近结果摘要”，正文不
上屏。与 GUI（对话视图：历史回放＋流式＋工具卡＋审批卡＋取消，T244—T248）和 Web 相比，
TUI 是交互体验的洼地；字段测试（`artifacts/fieldtest-2026-11-17.md`）确认四端对话形态
均未达到“现代代理 UI”下限，用户指令先补 TUI。

## 决策

1. **执行器面**：`whcode_tui::LlmExecutor` 新增 `run_streaming`（默认实现回落 `run`，
   丢弃流式语义，空执行器与测试夹具零改动）；装配根 `TuiLlmExecutor` 覆写——
   `ApprovalGate::for_store("tui-local", …, approvals: None)`、`TurnRequest.request_id =
   Some(预生成值)`（取消确定性）、`run_turn_with_approval + progress + observer` 直通。
   `whcode_app::llm::generate_request_id` 由 `pub(crate)` 升为 `pub`（前端预生成与 CUI
   显式 `--request-id` 同语义；校验仍由 turn 入口执行）。
2. **线程模型**：提交即 `std::thread::spawn`（每 turn 一个，随 turn 终止而终止，无常驻）；
   进程内 `mpsc` 双通道——delta 通道有界 64（`try_send` 满即丢，**有损**；完成消息走
   阻塞 send 保证送达）；`Done` 后流式行以 `outcome.text` **全量替换**，弥补丢帧，
   最终正文以 turn 结果为权威。`turn_running` 期间冻结视图切换与会话/任务写操作
   （Esc 与 Composer 除外）；数据变更通知与游标推进在运行期间不触发库查询。
3. **渲染**：Chat 视图行缓冲（有界 `CHAT_BUFFER_MAX = 1000` 行，超限丢最旧并登记）；
   行型：轮头（request 短码）、用户、助手流式（原位追加）、工具卡（`TurnSignal` →
   `tool_call::describe` + 既有 `tool.call.*` 词条）、轮尾（终止原因＋耗时＋摘要）。
   真实供应商暂无工具信号（P0 未修）即无工具卡行——如实空，不伪造。
4. **交互**：Composer 单行输入（`InputMode::Text { kind: ChatPrompt }`）；剖面名沿用
   `P` 键输入（`ChatProvider`），空则取注册表首项（与 GUI `chat_submit` 同口径）；
   Enter 提交、Esc＝运行中取消／空闲返回会话视图；提交门控：无执行器／无会话／空提示词
   ／已在运行中——四类结构化拒绝进状态行。
5. **历史回放**：进入 Chat 视图与**下一次提交前**经 `ViewSource::conversation_tail`
   刷新（上限与 GUI 同为 50 条）；turn 收口后**保留转写在屏**（轮头／轮尾可见，避免
   回放截断刚结束的一轮——与 GUI“收口即刷新”的差异如实登记）；turn 运行期间不触库
   （GUI 同款纪律）。
6. **i18n**：新词条一次改齐十三套目录；`i18n-po.py` 同步 PO；四主题 token 复用现有。

## 理由与约束

- 资源纪律（NFR-211）：无常驻线程——线程寿命＝单次 turn；通道有界；行缓冲有界并有测试。
- 分层纪律：TUI 只经 `whcode-app` 服务面（`run_turn_with_progress`／`describe`），
  不触库、不依赖 `whcode-bin`／`whcode-gui`。
- 信号链完备：无工具信号时不渲染占位卡；取消后的终止行如实标注（`cancelled`）。
- 审批面：v1 无 TUI 审批卡——写类工具调用按 `approval_unavailable` 结构化失败并照常
  显示卡片（failed 态），不静默吞掉（GUI 交互审批卡的 TUI 化另批立项）。

## 替代方案

- **同步直渲（不加线程）**：实现最简，但 turn 期间整个界面冻结、流式不可见——与
  ZCode/GUI 形态落差未消除，否决。
- **复用 GUI trait（TUI 依赖 whcode-gui）**：违反前端平级与分层（GUI 是前端适配器），
  否决；两侧 trait 保持同形，装配根一者实现两者。
- **TUI 审批卡同批**：扩大批次面且审批交互需键盘焦点模型设计，另批。

## 影响

- **正面**：TUI 从“查询表单”升级为可用对话端；取消确定性（request_id 前置）优于 GUI
  现状（GUI 从摘要文本反解析 request_id）；为后续 TUI 审批卡与 P0 修复提供渲染面。
- **负面**：`LlmExecutor` trait 增方法（实现方需适配）；TUI 渲染路径新增分支；
  delta 通道有损——依赖 Done 全量替换保证最终一致性（接受：与 GUI 同口径）。
- **承担者**：TUI 维护者（实现与测试）、装配根维护者（执行器适配）、用户（键位变化）。

## 正负和解

以“有损流式＋完成全量替换”换取界面不冻结与实现简单；以“审批卡缺席＋写类工具结构化
失败”换取 v1 批次可收口；两处差异均显式登记于 FR-796 与 surface 登记表，可复审、可
撤销（恢复旧行为只需不进入 Chat 视图）。

## 三重时间

- **过去来源**：ADR-056（GUI 交互化与 Phase 2 线程模型）、ADR-032（TUI LLM 接线）、
  T167/T244—T248、字段测试报告（2026-11-17）。
- **现在证据**：`whcode-app` 回调面（`TurnProgress`／`TurnObserver`）在库并有 GUI 消费
  方；TUI Chat 视图占位（render.rs 0 行）在案。
- **已知限制**：①delta 通道有损（容量 64，`try_send` 满即丢）——最终正文以完成消息
  全量替换兜底，流式期间屏上文本可能短暂缺段；②TUI v1 无交互审批卡——写类工具调用按
  `approval_unavailable` 结构化失败（GUI 已有交互审批卡，差异在 FR-796 登记）；③真实
  供应商的工具卡渲染依赖 P0 解码修复（`artifacts/fieldtest-2026-11-17.md`）——修复前
  真模型对话不出现工具卡（如实空，不伪造）；④Chat 转写缓冲上限 1000 行，超出丢最旧。
- **将来失效条件与复审触发（绑定命令＋判定）**：
  1. `python .github/scripts/check-frontend-budget.py` 涉前端资产超线 → 复审渲染方案
     （TUI 不产 JS/CSS 资产，本触发主要约束后续 Web 侧）；
  2. ADR-087 批次合入后 TUI Chat scripted 冒烟任一断言失败 → 回滚本批渲染分支；
  3. P0 解码修复（真协议 tool_calls）合入后 → 用真模型重跑 TUI 对话冒烟，工具卡应出现
     真实信号；无卡即复审；
  4. `whcode-app` 回调面签名变更（`TurnProgress`／`TurnObserver`）→ 本批适配层复审。

## 关联

- 需求：FR-796（spec.md）；任务 T376—T381（tasks.md）；验收 Check 472—475（checklist.md）。
- 字段测试背景：`artifacts/fieldtest-2026-11-17.md`（P0 断链与本批边界）。
