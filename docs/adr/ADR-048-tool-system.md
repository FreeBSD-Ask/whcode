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
   副作用），**未被任何 app 层 crate 引用**——域 B 判定修正：WHCode 非"空白"，是"已建模
   待接线"（`research/zcode-comparison/domain-B-tool-system.md`）。
2. ZCode 参照（域 B）：注册表 `tool/registry.ts`（Map + 别名冲突拒绝）、调度器
   `DEFAULT_MAX_CONCURRENCY=10`（拓扑排序/分组并行/环检测）、输入校验三层、内建工具全家桶
   （Read/Write/Edit/Bash/Glob/Grep/WebFetch/…约 25 个）；ApplyPatch/Workflow 有 handler
   但注释关闭。**调度并行是优化不是成立要件**（FR-406 并行调度在本仓已有"未通过一致性
   测试前不启用"的延后判据，Check 172）。
3. sensena-6.8 实测（2026-10-29）：OpenAI 兼容 `tools`/`tool_call_id`/`role:"tool"` 全链路
   可用——P0-2 无需 JSON 协议模拟或换供应商（建议书 D4 已解除的硬前置）。
4. ADR-044/FR-724：P0-1 中模型返回工具调用时以 `tool_not_wired` 显式终止——**过渡行为**，
   本 ADR 兑现其替换承诺（ADR-044 复审触发 1）。
5. 资源铁律（NFR-215/216）：容器有界、禁止 `mem::forget`、不随会话加线程；BH"恰好 3"式
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
8. **turn 集成**：T207 落地后，FR-724 的 `tool_not_wired` 分支替换为"登记→执行→回填→
   continue"，终止原因枚举保留 `tool_not_wired` 仅供登记缺失场景兜底；替换完成后
   ADR-044 复审触发 1 收口。

## 3. 正负和解

- **正面**：agent turn 从"只会说"变"会做事"；三步登记协议兑现既有先手；sensenova
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

## 7. 修订记录

- **2026-10-29（首版，已批准）**：用户批准建议书全部功能建议后起草；编号顺延至 048
  （ADR-047 已占用）；六件套裁剪、串行调度、一层校验为对 ZCode 参照的显式减法。
