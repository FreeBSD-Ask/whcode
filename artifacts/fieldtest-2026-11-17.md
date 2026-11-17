# 字段测试报告：真实供应商端到端代理能力（2026-11-17/18）

> 任务认知卡（宪法原则 X）：
> - **方向**：用真实编码任务实测 WHCode 产品能力，回应“账本全绿但相比 ZCode/DSH 功能几乎未实现”的质疑；
> - **目标**：取得可判定的端到端证据，把“感觉”变成事实清单；
> - **用户意图复述**：验证代理核心能力是否真实可用，而不是核对任务勾选率；
> - **判据**：turn 完成、正文非空、工具调用事件存在、产物可验证；
> - **非目标**：不修代码、不做性能结论、不测 GUI（只读 v1）；
> - **手段（后置）**：CUI `llm run`（＝`whcode_app::turn::run_turn`）＋ SenseNova 6.8 Flash Lite ＋ 沙盒样例库 `local/fieldtest/sample-app`（不入库）。

## 一、方法与口径

- 驱动入口：`target/release/whcode.exe --workspace <沙盒> --cli llm run`（构建 2026-11-17T07:27，晚于全部行为性代码提交）。
- 剖面：`%USERPROFILE%\.whcode\profiles.json`（仓库外，凭据为 `env:SENSENOVA_API_KEY` 引用，密钥不落盘不入库）；追踪 `redacted`。
- 样例：`local/fieldtest/sample-app`——三文件 Rust 库，`stats::median` 植入偶数长度缺陷（应取中间均值，实取单元素）。
- 会话 S-1（沙盒 `whcode.duckdb`，事件随库留存）。

## 二、结果

### A1 工具调用任务——失败（P0 断链）

提示词要求“通读源码、找缺陷、给 diff”。实测（request `llm-1794930953811-a57747d4311d0e32`）：

- `finish_reason = "tool_calls"`：模型发出了原生工具调用意图；
- **事件流水无任何 `ToolCallStarted/Finished`**（仅 TurnStarted→RequestStarted→StreamingChunk×2→RequestCompleted→RuntimeTerminated→TurnFinished）；
- 正文为空（`"\n\n"`），turn 以 `model_finished` 终止（iterations=1，59 completion tokens）；
- 无错误事件、无告警——属“错误的空”（信号链完备原则指名的歧义空）。

### A2 纯文本对照任务——通过

“Vec 与 slice 的区别”两句话问答：`completed=true`、`finish=stop`、正文完整、usage 齐（1513→220 tokens，5.4 s）。**循环编排、流式、用量、终止语义在真实供应商下健康**。

## 三、根因（代码级，file:line）

1. **响应侧**：`crates/whcode-llm/src/http/protocols.rs:284-301` OpenAI 解码器只读 `delta.content`，从不读 `delta.tool_calls` 片段（id/name/arguments 增量无累积）——模型的原生工具调用帧被整段丢弃，`finish_reason="tool_calls"` 只剩字符串。
2. **请求侧**：无 `role:"tool"` 消息装配（全文件无命中）；`protocols.rs:139` 注释自认“tool_result 结构化块需 tool_use_id，随 P0-2 工具接线扩展”；`crates/whcode-llm/src/provider.rs:150` 自认“真实协议的 tool_calls 帧解析属 P0-2 接线范围”。
3. **终止面**：`crates/whcode-app/src/turn.rs:1142` `step_tool_calls.is_empty()` 即 `ModelFinished`——解码缺失使每个需要工具的 turn 静默终止。
4. **验收口径缺口**：turn 循环与工具执行的 Check 证据全部来自桩剧本（`provider.rs` 桩 Provider）；真模型路径从未被任何 Check 覆盖。桩证据 ≠ 端到端信号。

Anthropic（`tool_use`/`tool_result`）、Gemini（`functionCall`/`functionResponse`）适配器同样无命中。`Role::Tool` 枚举与 turn 层“role:tool 回填”注释在档，但领域枚举之下无协议装配。

## 四、功能对齐缺口清单（实测后修订版）

| 级别 | 缺口 | 证据 | 与 ZCode/DSH 对照 |
|---|---|---|---|
| P0 | 真实协议工具调用双向断裂：模型→工具（解码缺失）、工具→模型（回传缺失）；真实模型下文件/搜索/shell/MCP/LSP/技能**全部不可达** | 本报告 §三 | ZCode/DSH 的核心能力即“模型调工具改代码”；WHCode 此链路在真供应商下为零 |
| P1 | 非交互面无审批策略：CUI `run_turn` 无审批闸门，写类工具（write/edit/shell）在 CLI 结构性不可用（`turn.rs:487`、`1849-1853`）；TUI 有交互审批，CLI 无 ask/auto 等价物 | 代码装配 | ZCode permission modes（ask/auto-approve）；DSH 四态审批 |
| P1 | 模型面单薄：五族协议适配器在档，但仅 OpenAI 族被真实供应商打过（且工具路径断裂）；无主流供应商实测 | 本报告 | ZCode/DSH 多供应商常态 |
| P2 | 代理 UX：CLI 无流式渲染与工具卡显示；cost 恒 null（无计价口径）；压缩/计划模式/todo 存在但依附于断链的工具路径 | A1/A2 输出 | 明显体验差 |
| — | 已验证真实可用：纯文本 turn、流式、用量、会话/事件持久化、结构化错误、取消语义、SDK/ACP 面在档 | A2 与代码 | 不落后 |

## 五、结论

- 用户感受**在代理核心能力维度成立**：工具调用链路（编码代理的神经）在真实供应商下断裂，此前“核心循环是真的”的判断需修正为“循环编排与文本对话真实可用，工具链路未接通”。
- 账本没有谎报它量过的东西——但工具执行的验收走的是桩剧本，**桩绿 ≠ 真通**；这是信号链完备原则（交付＝信号存在，而非执行存在）的现实案例。
- 修复面明确且有限：①三家协议解码器补 tool_calls/tool_use/functionCall 解码与累积；②请求装配补 `role:tool`/`tool_result`/`functionResponse`；③真模型 e2e Check（SenseNova 打通一次真实 read_file 调用）替换桩证据口径；④CLI 审批策略装配（P1.1 四态类型已有底座）。

## 六、复审触发

- 修复批次合入后，以“真实供应商完成一次含工具调用的完整 turn”为新 Check 判据；
- DSH 0.3.x 或 ZCode 协议变更时重读对照报告对应域。
