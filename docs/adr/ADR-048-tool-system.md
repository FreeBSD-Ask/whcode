# ADR-048：P0-2 工具系统——内建工具集、三步登记接线与 turn 集成

- **状态**：已批准（2026-10-29，用户批准建议书 P0-2 并指示实现；ADR 编号顺延——ADR-047
  已被发布链可重现构建占用）
- **来源**：`research/zcode-comparison/` 域 B（ZCode 工具系统，commit `3c5b227`）+
  `research/synthesis/proposal.md` P0-2/P1-7；ADR-044 决策 6（P0-1/P0-2 边界）与 FR-724
  （`tool_not_wired` 过渡行为的替换承诺）；sensenova 原生 function calling 实测
  （`docs/guides/test-model-sensenova.md` §6.1，提交 1b05733）
- **责任人**：whcode-tools/whcode-app 维护者
- **退出条件**：真实供应商 function calling 调用率低于预期使工具路径形同虚设（复审时以
  实际会话事件统计判定），或工具面维护成本显著超出单代理价值（另立 ADR 裁剪）

## 1. 事实与证据

1. `crates/whcode-store/src/tool_exec.rs`（424 行）已交付**三步登记协议**（`record_intent`
   →`mark_running`→`record_result` 终态 + `recover_uncertain` 崩溃恢复为 unknown、不重放
   副作用），**未被任何 app 层 crate 引用**——域 B 判定修正：WHCode 非“空白”，是“已建模
   待接线“（`research/zcode-comparison/domain-B-tool-system.md`）。
2. ZCode 参照（域 B）：注册表 `tool/registry.ts`（Map + 别名冲突拒绝）、调度器
   `DEFAULT_MAX_CONCURRENCY=10`（拓扑排序/分组并行/环检测）、输入校验三层、内建工具全家桶
   （Read/Write/Edit/Bash/Glob/Grep/WebFetch/…约 25 个）；ApplyPatch/Workflow 有 handler
   但注释关闭。**调度并行是优化不是成立要件**（FR-406 并行调度在本仓已有“未通过一致性
   测试前不启用“的延后判据，Check 172）。
3. sensena-6.8 实测（2026-10-29）：OpenAI 兼容 `tools`/`tool_call_id`/`role:"tool"` 全链路
   可用——P0-2 无需 JSON 协议模拟或换供应商（建议书 D4 已解除的硬前置）。
4. ADR-044/FR-724：P0-1 中模型返回工具调用时以 `tool_not_wired` 显式终止——**过渡行为**，
   本 ADR 兑现其替换承诺（ADR-044 复审触发 1）。
5. 资源铁律（NFR-215/216）：容器有界、禁止 `mem::forget`、不随会话加线程；BH“恰好 3”式
   硬上限为正面样例（`research/synthesis/README.md` §3 发现 2）；ZCode 无硬上限是反例。

## 2. 决策

1. **落点与分层**：不新增 crate——扩展既有 `whcode-tools`（当前 14 行骨架）。分层合规
   （`test_layering`）：工具注册表/描述/输入校验/内建实现在 `whcode-tools`（依赖
   `whcode-domain`，文件操作用 `std::fs`，不触库、无 `cfg(target_os)`）；调度编排、
   tool_exec 登记调用、turn 集成在 `whcode-app`（SQL 只在 store，ADR-018）；进程类工具
   （后续批次）经 `whcode-runtime` 子进程闸门。方向：`app → tools/runtime/store`，无反向。
2. **内建工具 v1 集合（六件套，对照 ZCode 全家桶裁剪）**：`read_file`、`write_file`、
   `edit_file`（精确串替换 + 唯一性校验）、`list_dir`、`glob`（有界遍历，不引新依赖）、
   `grep`（逐行字面/单正则，结果有界）。Bash/WebFetch/MCP 投影等**不在 v1**——Bash 属
   高危面（须先有权限分级与审计叙事，另批提案）；每个工具登记
   `{name, description, parameters(JSON Schema 子集), side_effect_scope}`。
3. **调度 v1 串行**：turn 内按模型声明顺序逐个执行；每轮工具调用数硬上限（默认 8，可
   配置）；并发拓扑调度**不采纳**进 v1（真实矛盾未成立，FR-406 判据延续）。
4. **登记接线**：每次工具执行经 store 三步登记（`record_intent`→`mark_running`→
   `record_result`），终态三值（succeeded/failed/unknown）+ `recover_uncertain` 语义
   复用；事件 `ToolCallStarted/ToolCallFinished` 落 `events` 表（FR-722 口径，payload
   只带标识与计数）。
5. **协议解析**：`LlmChunk.tool_calls` 的真实协议解析 v1 仅做 **openai 兼容族**
   （实测供应商在用）；anthropic/gemini/ollama/cohere 的 tool_calls 帧解析标 `[待验证]`
   （有桩测试路径，真实供应商接入时补）。
6. **回填语义**：工具结果以 `role:"tool"` 消息追加进下一轮 `messages`（FR-720 已备）；
   结果超 64 KiB 截断并附 `truncated` 标记（R-05 对策口径）；`write_file`/`edit_file`
   的 diff 摘要（非全文）进结果，正文不进事件。
7. **安全边界**：路径参数解析为**工作区内相对路径**——拒绝绝对路径、`..` 越界与工作区外
   目标（BH finding-bound 拓扑绑定 fail-closed 教训）；执行前经 `authz` 权限面；输入
   校验一层（参数白名单 + 必填 + 类型 + 拒绝未知字段），ZCode 三层校验裁剪（v1 参数
   空间小，三层无真实矛盾）。
8. **turn 集成**：T207 落地后，FR-724 的 `tool_not_wired` 分支替换为“登记→执行→回填→
   continue“，终止原因枚举保留 `tool_not_wired` 仅供登记缺失场景兜底；替换完成后
   ADR-044 复审触发 1 收口。

## 3. 正负和解

- **正面**：agent turn 从“只会说”变“会做事”；三步登记协议兑现既有先手；sensenova
  function calling 直接可用；六件套覆盖编码任务主要文件操作面。
- **负面**：新增工具维护面（六工具 × 校验 × 测试）；写路径引入工作区文件变更风险
  （缓冲：相对路径绑定 + 权限面 + diff 摘要留痕）；串行执行在大批量调用时慢（v1 接受，
  上限 8/轮控制）。
- **承担者**：whcode-tools 维护者（工具与校验）、whcode-app 维护者（登记与集成）、
  用户（写操作确认走既有四端确认面，FR-623）。
- **和解理由**：P0 级成立要件（建议书 §1）；ZCode 全家桶裁剪为六件套防止形式主义扩张
  （真实矛盾：编码任务的核心动作就是读/写/改/找文件）；并行调度等真实矛盾出现再立。

## 4. 三重时间

- **过去来源**：ZCode 域 B 一手源码（commit `3c5b227`）；本仓 `tool_exec.rs` 登记协议
  （T 判据 V-301—V-307）；sensenova 实测记录（2026-10-29）。
- **现在证据**：P0-1 契约层已落地（`LlmToolCall`/`LlmChunk.tool_calls`/五协议映射，
  提交 `5173580`/`64b3a63`）；tool_exec 待接线为 Grep 实测否定性结论。
- **将来失效条件**：见头部退出条件；复审触发见 §5。

## 5. 复审触发

1. `cargo test -p whcode-tools` 与 layering 门禁随每批提交执行——工具判据失效即触发；
2. P0-2 收口后首个真实供应商批次：统计会话事件中 `ToolCallStarted` 的
   succeeded/failed/unknown 比例——unknown > 5% 或 failed > 30% 触发工具面复审；
3. 新工具提案一律走 Loop Discovery 式覆盖阶梯（先证既有覆盖不足，再新增——
   `docs/guides/milestone-review.md` 步骤 5）。

## 6. 实施摘要

- 需求登记（本提交）：FR-725—729（注册表/六件套/登记接线/解析回填/安全边界）、
  T213—T219、Check 309—315；计数同步（FR 102→107、任务 212→219、Check 308→315）。
- T213（whcode-tools 注册表+校验）、T214（六件套）与 T217（安全边界）不依赖 T207，可
  立即实现；T215/T216（登记接线与 turn 集成）排在 T207 之后。

## 6.1 实施记录（2026-10-30）

- T213/T214（提交 348b2ac）：注册表/工作区绑定/六件套，11 模块测试绿。
- T215—T217（本批）：turn 循环内三步登记（`record_intent`→`mark_running`→
  `record_result`，exec_id `TX-<request_id>-<iteration>-<n>`）、`execute_tool_call`
  （权限面→校验→登记→执行→role:tool 回填，任何失败结构化回填不中断循环）、
  workspace 全链路贯通（CUI/TUI/Web：`TurnRequest.workspace_root`）。
- **登记边界**：登记覆盖“已受理执行”；受理前拒绝（权限/参数/绑定）不产生登记行。
- **权限面 v1 和解**：读工具 SessionRead／写工具 SessionCreate；专用 WorkspaceWrite
  归 P2-1 动态权限 broker（§2.7 注记兑现）。web 端 workspace 经
  `handle_request_with_llm` 线程化贯通。
- FR-724 `tool_not_wired` 过渡行为已替换；turn.rs 的 `#[expect(never_loop)]` 移除
  （循环真实生效）；Check 303 的 iteration_limit 端到端测试补齐。

## 7. 修订记录

- **2026-10-29（首版，已批准）**：用户批准建议书全部功能建议后起草；编号顺延至 048
  （ADR-047 已占用）；六件套裁剪、串行调度、一层校验为对 ZCode 参照的显式减法。

## 7.1 修订 2026-11-12：会话待办工具（P4.6）

- **来源**：`research/synthesis/proposal-v3.md` P4.6、`research/dsh-comparison/domain-G-plan-goal-todo-schedule.md`
  §2.3／§4.4；FR-771／T284／Check 379。

### 决策

1. **新内建工具 `todo_write`**（第七件）：**整表替换**语义——调用方给出 `pending`／
   `in_progress`／`completed` 三个字符串数组，未给出的列表视为空；重复调用覆盖旧清单。
   条目刻意没有 id 与优先级（与 DSH `todo/write` 同口径：清单是快照，不是可增量操作实体）。
2. **与 SDD 任务账本硬隔离**：待办**不写 `tasks` 表**、不进入阶段门禁、不满足验收检查点、
   不参与 `session_overview` 聚合；工具说明与 `docs/guides/tool-catalogue.md` 都写明这一点，
   避免模型把待办当可交付任务提交。
3. **参数类型扩展** `ParamType::StringArray`：JSON Schema 子集的 `array of string`
   （provider 清单渲染 `"type":"array","items":{"type":"string"}`；校验只到“数组且元素皆为
   字符串”，不校验嵌套对象——与 v1 一层校验口径一致）。
4. **副作用范围扩展** `SideEffectScope::SessionState`：只改本会话派生状态，不触碰工作区。
   权限按**写类**判定（`SessionCreate`，与工作区写同档）；**审批默认沿用读侧口径**——
   待办不落文件、载荷有界、全量入事件，若每次清单刷新都弹审批，代价高于风险；
   用户仍可用按工具规则把它设为 `ask`／`deny`（`approval_rules` 表优先于该默认）。
5. **事件 `TodoUpdated`**（`whcode-domain::EventKind`）：log-only（不进模型历史），可靠性分级
   **Critical**——它是清单的唯一持久副本（工具参数不入事件载荷），队列饱和时不得丢弃。
   落点：turn 在工具调用**成功**后，用与工具相同的纯函数
   `whcode_tools::snapshot_from_arguments` 重新规范化并落快照，口径不分叉；
   失败／被拒／超时的调用**不落**（状态未变，写空清单即伪造事实）。
6. **读取面** `whcode-app::todo::current`：取最近一条 `TodoUpdated`（`LIMIT 1` 主键倒序，有界），
   缺失会话 `not_found`、无快照为正确的空、载荷损坏为 `parse_error`（不伪装空清单）。
7. **有界**：条目总数 ≤ 50、单条内容 ≤ 200 字符；超限返回 `invalid_state` 而**不截断**
   ——待办是模型声明的意图，静默裁剪会让清单与意图不一致。

### 正负和解

- **正面**：模型有了轻量计划面而不污染工程账本；参数类型与副作用范围各扩一种，仍在
  v1 一层校验与四态审批的口径内；快照是纯事件、可重放、可审计。
- **负面**：清单只能整表替换、无 id 无法做“改一条”；没有独立 UI 视图（前端待后续批次）；
  `SessionState` 沿用读侧审批默认（对极端多租户部署偏宽，本地优先场景可接受）。
- **承担者**：工具维护者（与事件口径不得分叉）、前端维护者（视图待做）、用户（整表语义
  要求每次给出完整清单）。

### 复审触发（绑定命令＋判定）

1. `cargo test -p whcode-tools todo` 或 `cargo test -p whcode-app todo_write` 不再全绿 → 复核语义；
2. `rg -n "todo_write" crates/whcode-app/src/turn.rs` 不再命中 → 落点被改动，复核 §7.1-5；
3. 引入待办增量操作或 id 化 → 本修订失效，另立 ADR；
4. 待办需要进入模型上下文（跨压缩回填）→ 与 `EventKind::TodoUpdated` 的 log-only 口径冲突，
   须重新设计并复审；
5. 2027-05-07（与 D15 覆盖率复审同批）→ 复查上限（50／200）与审批默认是否匹配真实用法。

### 已知限制

- 只做整表替换：无法按条目 id 修改、删除或重排（重排只能重给全表）。
- 无独立视图：CUI／TUI／GUI／Web 尚无待办卡片，当前可从 `events list` 看到 `TodoUpdated` 事件，
  读取 API `whcode-app::todo::current` 已就绪供前端接入。
- 快照只保留“最近一次”：历史快照可按 `seq` 回溯，但不提供“某次 turn 结束时清单”的专用查询。
- `SessionState` 的审批默认沿用读侧口径（见决策 4 的代价说明）。

## 7.2 修订 2026-11-12：交付物登记（P4.7）

- **来源**：`research/synthesis/proposal-v3.md` P4.7、`research/dsh-comparison/domain-B-tool-system.md`
  §2.4／§4（`present` 工具与 `deliverables/presented` 事件）；FR-772／T285／Check 380。

### 决策

1. **新内建工具 `present_deliverables`**（第八件）：把工作区内**已存在**的文件登记为本会话
   交付物。参数 `paths`（`StringArray`，必填）1—4 个（对齐 DSH `present` 的 4 个上限），
   路径须为工作区内相对路径（沿用绝对路径与 `..` 拒绝口径），且必须是**常规文件**——
   登记不存在的路径等于声明未发生的事实，一律拒绝；目录同样拒绝（交付物是可直接打开的
   文件，不是目录树）。
2. **不改文件、不改工具结果**：本工具只读元数据；登记属产品语义层（与 DSH 同定位）。
   副作用范围沿用 7.1 引入的 `SessionState`（权限按写类、审批默认沿用读侧）。
3. **事件 `DeliverablePresented`**（log-only、Critical）：只在调用**成功**后落，
   载荷含 `paths`（已规范化）、`count` 与 `tool_call_id`；失败／被拒不落。
4. **读取面** `whcode_app::deliverables::list`：累加型事实的聚合视图——按**首次登记顺序**
   去重（同一路径重复登记只出现一次，保留首次 `seq` 与 `tool_call_id`）；事件读取
   ≤ 200 条、条目 ≤ 200 条，达到上限显式 `truncated`；缺失会话 `not_found`、载荷损坏
   `parse_error`（不伪装空清单）。
5. **展示面**：Web 对话页新增交付物卡片（无登记时隐藏；请求失败保持隐藏，不用空清单冒充
   “没有交付物”），数据源 `GET /api/deliverables?session=`；能力面登记
   `view.deliverables`（CUI 无视图面按 ADR-032 排除、TUI／GUI 未排期、Web 已实现）。
6. **有界**：单次 ≤4 个、单路径 ≤400 字符、读取有界见决策 4；全部同步只读元数据，
   不新增线程。

### 正负和解

- **正面**：交付物第一次成为可检索的事实（此前只有工具有 `side_effect` 标记）；模型可显式
  声明“我交付了哪些文件”，用户与审计可在 Web 直接核对；失败路径不产生虚假交付记录。
- **负面**：只验证“存在且是文件”，不验证“由本次 turn 产出”（模型声明优先）；不复制文件、
  不算哈希，文件后续被改动或删除时清单只反映登记时刻；TUI／GUI 视图未做。
- **承担者**：使用者（核对清单与工作区实际状态）、工具维护者（登记口径不得分叉）、
  前端维护者（TUI／GUI 视图待排期）。

### 复审触发（绑定命令＋判定）

1. `cargo test -p whcode-tools present` 或 `cargo test -p whcode-app present_deliverables`
   不再全绿 → 语义复核；
2. `rg -n "DeliverablePresented" crates/whcode-app/src/turn.rs` 不再命中 → 落点被改动；
3. 引入“本次 turn 产出归属”判定（git 差异或写工具回执）→ 本修订的口径需要收紧，另立 ADR；
4. TUI／GUI 交付物视图交付或 Web 视图改版 → 更新 `view.deliverables` 与矩阵；
5. 2027-05-07（与 D15 覆盖率复审同批）→ 复查单次上限（4）与条目上限（200）是否匹配真实用法。

### 已知限制

- 不判定产出归属：任一工作区已存在文件都可被登记，清单不代表“本轮新建/修改”。
- 不复制、不哈希：交付物是路径引用；文件被改动或删除后清单仍显示登记事实（不追溯一致性）。
- TUI／GUI 无视图；CUI 按命令面无视图（可用 `events list` 看到 `DeliverablePresented`）。
- 只做登记，不做“导出交付物包”（打包/压缩属另一能力，未立项）。
