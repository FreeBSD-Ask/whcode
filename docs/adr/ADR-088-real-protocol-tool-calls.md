# ADR-088：真实协议工具调用双向接线（P0）

- **状态**：已批准（2026-11-19，用户指令“继续”，承接字段测试① P0 发现与“你点头我就开
  P0 批次”的确认）
- **编号认领**：ADR-088（`ls docs/adr/` 复核，087 之后无占用）；FR-797；T384—T389；
  Check 478—479

## 任务认知卡

- **方向**：修复字段测试①判定的 P0 断链——三家协议适配器不读工具调用帧、请求侧无
  工具结果回传，真实模型下工具面整体不可达。
- **目标**：OpenAI 族（SenseNova 实测）真模型端到端打通一次 read_file；Anthropic/
  Gemini/Ollama 同层补齐并以单元测试取证。
- **用户意图复述**：让 WHCode 的代理“神经接通”——模型能真的调工具改代码，UI 批次
  的工具卡才会被点亮。
- **判据**：真模型 e2e（工具事件、真实输出、integrity_gaps=0）＋协议单元测试
  （跨帧累积、请求序列化往返）。
- **非目标**：跨轮工具链路持久化（messages 表无 tool_call_id 列，T389 边界）；写类
  工具的 CLI 审批装配（P1，另批）；Anthropic/Gemini 的 tools 参数格式转换（无真实
  供应商可取证，登记边界）。
- **手段（后置）**：`LlmMessage` 结构扩展＋`DecodeOutcome` 片段＋`ChunkNormalizer`
  跨帧累积＋请求装配三分支。

## 问题

`artifacts/fieldtest-2026-11-17.md` 实证：真实供应商返回 `finish_reason=tool_calls`
时事件流无任何 ToolCall、正文为空、turn 静默按 `model_finished` 终止。根因三段：
①OpenAI 解码器只读 `delta.content`；②`messages` 序列化无 tool 消息形状；
③`LlmMessage` 无工具调用结构化字段（桩 Provider 之外无处承载）。

## 决策

1. **消息结构**：`LlmMessage` 增 `tool_calls: Vec<LlmToolCall>`（assistant 声明）与
   `tool_call_id: Option<String>`（工具结果关联）；便捷构造
   `assistant_with_tool_calls`／`tool_result`／`plain`。turn 循环在 assistant 回填时
   携带本步 `step_tool_calls`、工具结果回填时携带 `tool_call.id`。
2. **解码**：`DecodeOutcome::Delta`／`DeltaAndFinish` 增 `tool_fragments`（
   `ToolCallFragment{index,id,name,arguments_delta}`）；`ChunkNormalizer` 按 index
   跨帧累积，终结 chunk 整体交付（BTreeMap 保序；缺 id／name 的不完整调用丢弃——
   不伪造）。OpenAI（delta.tool_calls 分片）、Anthropic（content_block_start
   tool_use＋input_json_delta）、Gemini（functionCall 整体，**无调用 ID——以函数名
   合成，回传侧 `tool_call_id` 承载函数名**）、Ollama（message.tool_calls，同合成）
   四族落地；Cohere 走 messages_flat 复用 OpenAI 形状。
3. **请求装配**：OpenAI/Ollama/Cohere `messages_flat` 三分支（assistant.tool_calls /
   role:tool+tool_call_id / 纯文本）；Anthropic `tool_use`／`tool_result` 块
   （相邻 tool 结果合并进同一 user，满足严格交替）；Gemini `functionCall`／
   `functionResponse` parts（相邻同角色合并）。
4. **终止原因**：工具调用终止原因跨协议透传（`tool_calls`／`tool_use`／`stop`），
   turn 循环以“step_tool_calls 非空”为执行依据，不依赖原因字符串。

## 理由与约束

- 累积器有界（BTreeMap 按帧内 index；流结束即收口）；不完整调用丢弃并在文档留痕，
  不伪造调用。
- 既有 `LlmChunk.tool_calls` 契约不变（桩 Provider 行为零变化）。
- 零 unsafe 维持；全部改动在 whcode-llm／whcode-app 层，分层不变。

## 已知限制与失效条件（复审触发，绑定命令＋判定）

- **已知限制**：①跨轮工具链路未持久化（messages 表无 tool_call_id 列）——重载后
  role:tool 无前置锚点，本批以“下一轮上下文不含上一轮消息”的既有口径回避（字段
  测试② turn 2 实证），多轮工具记忆属 T389；②Anthropic/Gemini 的 tools 参数格式
  转换未做（turn 侧 tools 为 OpenAI 形状）——接入真实 Anthropic/Gemini 供应商前
  必须补齐；③Gemini/Ollama 的调用 ID 为合成（函数名），供应商侧无真 ID。
- **将来失效条件与复审触发（绑定命令＋判定）**：
  1. T389 落地后重跑字段测试② turn 2——模型应能复述上一轮工具调用；
  2. 接入任一 Anthropic/Gemini 真实供应商时，先补 tools 参数转换并重跑单元取证；
  3. `cargo test -p whcode-llm` 任一协议测试失败 → 回滚对应解码分支。

## 替代方案

- **只修 OpenAI 族**（最小修复）：SenseNova 即可跑通，但四族同层分叉会在接入第二
  家时重现同类断链——否决，四族一次补齐（Anthropic/Gemini 单元级取证）。
- **content 编码承载 tool_call_id**（不动 LlmMessage）：把调用 ID 编进文本——协议
  语义错误、下游无法可靠分离——否决。

## 影响

- **正面**：代理核心能力在真实供应商下打通（工具六件套/MCP/LSP/技能全部可达）；
  TUI/GUI 工具卡将被真实信号点亮（ADR-087 复审触发 3 可执行）。
- **负面**：`LlmMessage`/`DecodeOutcome` 公共结构变更波及 28 处构造点；跨轮持久化
  缺口显式登记（T389）。
- **承担者**：whcode-llm 维护者（协议层）、whcode-app 维护者（turn 回填）、
  供应商接入者（tools 参数转换）。

## 正负和解

以“四族一次补齐＋单元级取证”换真实供应商覆盖的可判定边界：OpenAI 族有真模型 e2e，
其余三族如实标注单元级——不冒充平台结论（ADR-013 口径的协议层版本）。

## 三重时间

- **过去来源**：`artifacts/fieldtest-2026-11-17.md`（P0 实证）、provider.rs 与
  protocols.rs 的自认注释、ADR-044/048（turn 与工具系统）。
- **现在证据**：`artifacts/fieldtest2-2026-11-19.md`（真模型 read_file 全链）＋
  whcode-llm 70 项单元测试（含四族解码/装配新用例）。
- **将来失效条件**：见“已知限制与失效条件”。

## 关联

- 需求：FR-797；任务 T384—T389；验收 Check 478—479。
- 字段测试：`fieldtest-2026-11-17.md`（断链实证）→ `fieldtest2-2026-11-19.md`（修复实证）。
