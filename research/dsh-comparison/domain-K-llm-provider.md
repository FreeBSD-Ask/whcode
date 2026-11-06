# 域 K：LLM Provider 层

> DSH 证据根：`local/dsh-src`（commit `fcc0ad4`）；WHCode 证据根：本仓库。

## 1. 对照表

| 项 | DeepSeek Harness (DSH) | WHCode | 判定 |
|---|---|---|---|
| 接入抽象 | `ctx.llm` 是唯一入口；provider 翻译留在各自 adapter；请求 deep-frozen，扩展与 adapter 不得改写（`packages/llm/llm/README.md`） | `trait Provider` + `WireProtocol` 枚举，自研 5 协议（`crates/llm` 略；`crates/whcode-llm/src/http/profile.rs:49`—`57`） | **同构**：都是一条流一个 seam |
| 协议广度 | 出货 adapter：`dsh-llm-deepseek`（官方）＋`dsh-llm-pi-ai`（多供应商库）（`packages/llm/README.md`） | openai／anthropic／gemini／ollama／cohere 五协议内置 | **WHCode 领先**：不依赖第三方 provider 库 |
| 一次流＝一次尝试 | 明确规定；重试由独立插件 `dsh-llm-retry` 在 durable step 边界重跑（`llm/README.md`；`llm-retry/README.md`） | 重试 decorator 包住 Provider，仅在首 chunk 前重试（`crates/whcode-llm/src/retry.rs:3`—`12`） | **DSH 领先**：重试位置在循环边界，可落日志 |
| 重试策略 | provider 配置 `retryPolicy`：normal 有界／always 无限；退避 `initialDelayMs`/`maxDelayMs`/`jitterRatio`（`packages/llm/llm-retry/README.md`，Minimal configuration 节） | 固定档：5 次、基准 1 s、×2、上限 30 s、总预算 60 s；429 `retry_after_ms` 优先（`retry.rs:13`—`16`） | **DSH 领先**（可配）；WHCode 有总时长预算这一硬约束 |
| 安全重放判据 | 直接 `ctx.llm.stream()` 不重试——“裸流无法分离已发出 chunk”（`llm-retry/README.md`） | `safe_to_retry`：未消费任何成功 chunk 才重放（`retry.rs:6`—`10`） | **同构**：两侧结论一致，WHCode 写成语义 |
| 取消一致性 | 计划中的重试**先入会话日志再退避**，取消留下一致历史（`llm-retry/README.md`） | 退避睡眠按 50 ms 切片检查取消标记（`retry.rs:7`—`8`） | **DSH 领先**：取消后日志仍自洽 |
| 模型目录 | `listModels`＋badge（selector 文案与 tone），GUI 选择必须以目录为准；`llm/models-updated` 事件通知（`llm/README.md`） | 无模型目录服务；profile 静态声明端点与模型名 | **DSH 领先** |
| 供应商扩展 | `ctx.deepseekLlmApiExtensions`：插件声明**顶层附加字段**，重名/非法同步失败，准备失败仅跳过该 provider（`packages/llm/deepseek-llm-api-extensions/README.md`） | 无；请求体由协议实现固定 | **DSH 领先** |
| 用量计量 | token-meter：优先复用供应商 usage 锚点，否则完整估价（`docs/subsystems/token-meter.md`） | `metering.rs`＋`TransportSnapshot` 记录 reconnect_attempts（`retry.rs:14`—`15`） | **DSH 领先** |
| 凭据 | 专用 adapter（`llm-deepseek-api-key` 用 `apiKeyEnv`；`llm-deepseek-account` 走账号流）（`llm-retry/README.md`） | 凭据不落库不入日志；错误消息 scrub 已注入 secret（`crates/whcode-llm/src/http/profile.rs:217`） | **同构**，各有强项 |

## 2. DSH 实现解剖

### 2.2 冻结请求是硬约束

循环构造的请求 `deep-frozen`，adapter 与扩展都不能改写（`llm/README.md`）。这条纪律把
“谁改了请求”变成不可可能，域 A 的 `agent/request` 也因此只能在绑定前调控制项，不能换
provider/model/effort（域 A 已述）。

### 2.2 重试是独立可挂载插件

`dsh-llm-retry` 无自身配置，策略在 provider adapter 上（`llm-retry/README.md`）。
它只在 agent loop 场景挂载：裸流消费者保持单尝试（Use this package 节）。“计划重试先写日志再退避”
（Summary 节）使崩溃后能区分“打算重试”与“已重试”。

### 2.3 附加字段走注册表

DeepSeek 顶层扩展字段由插件向 `ctx.deepseekLlmApiExtensions` 声明；`register` 同步拒绝重名与
非法名，`prepare()` 并发准备、克隆并冻结返回值，某个 provider 抛错只丢它自己；
`accept()` 在 2xx 后跑回调，多个失败合成一个 `AggregateError`（`deepseek-llm-api-extensions/README.md`）。
“扩展语义不属于协议”这条线划得清楚。

## 3. WHCode 现状核对

- 已实现：5 协议请求体/流解析（`crates/whcode-llm/src/http/protocols.rs:213`、`:320`、`:421`、
  `:532`；名称映射 `profile.rs:49`—`57`），未注册协议返回 `None` 转结构化错误（ADR-031）。
- 已实现：流式重试 decorator（`retry.rs`），`safe_to_retry` 判据、退避取消感知、
  `Retry-After` 优先、尝试次数并入 `TransportSnapshot.reconnect_attempts`。
- 已实现：TLS 信任库（ADR-040）、出站凭据注入与错误消息脱敏（`profile.rs:217`）。
- 缺失：模型目录与能力协商（检索 `list_models`、`model_catalog` 于 `crates/` 无命中）、
  provider 附加字段注册表、按供应商差异化重试策略、usage 锚点式计量。
- 差异：WHCode 的 profile 是**配置层**（端点＋协议＋模型名），DSH 的 adapter 是**代码层插件**；
  前者新增供应商要改协议实现，后者新增供应商要装包。

## 4. 差异判定与可借鉴点

1. **“一次流＝一次尝试”**：应显式写入 ADR-050 口径。WHCode 的重试在 decorator 内部，
   日志只看到最终结果；DSH 的“先入日志再退避”值得改成 `LlmRetryScheduled` 事件，
   代价是事件量，收益是崩溃后可判断“是否已产生重复计费请求”。
2. **provider 差异化重试策略**：可采纳。当前 5/1s/×2/30s/60s 是交互场景的单一和解；
   至少允许 profile 声明 `retry_policy`（有界/关闭），但**不得**提供“always 无限重试”
   ——无限重试与 WHCode 资源铁律冲突。
3. **模型目录**：可采纳为轻量版（profile 声明可用模型＋展示标签），供 GUI/Web 选择器使用；
   完整的 `listModels` 动态目录需要网络调用与缓存策略，收益不明确。
4. **扩展字段注册表**：不采纳。WHCode 只有 5 个协议实现，没有第三方 provider 生态；
   增加注册表等于提前建扩展点。若将来要接 DeepSeek 专有字段，直接在协议里加即可。
5. **usage 锚点计量**：建议采纳为域 E 的前置。WHCode 预算判断依赖字符启发式，
   把上一次成功请求的 usage 作为锚点可以显著减少误折叠，成本是存一个结构。
6. **不采纳** deep-frozen：Rust 的类型系统＋`&LlmRequest` 已提供同等约束，无需运行时冻结。

## 5. 待验证项

1. DSH `retryPolicy.mode: normal` 的具体上限值：README 只给 `always` 示例。取证：
   读 `packages/llm/llm/src/retry-policy.ts` 的默认与校验。
2. DSH `llm-pi-ai` 是否覆盖多协议（若是，则协议广度差距缩小）。取证：读
   `packages/llm/llm-pi-ai/README.md` 的 Summary 与 Known Limitations。
3. WHCode 是否记录 usage（token 输入输出）到事件：`Usage` 类型存在（`turn.rs:28` 引用），
   但未确认落表字段。取证：`rg -n "usage" crates/whcode-app/src/turn.rs` 与 store 落表。

## 6. 证据路径清单

- `packages/llm/llm/README.md`（一 stream 一 attempt、冻结请求）
- `packages/llm/llm/README.md`（listModels/badge）
- `packages/llm/llm-retry/README.md`（durable step 边界重试）
- `packages/llm/llm-retry/README.md`（裸流单尝试）
- `packages/llm/llm-retry/README.md`（retryPolicy 配置样例）
- `packages/llm/deepseek-llm-api-extensions/README.md`（扩展字段注册表）
- `crates/whcode-llm/src/retry.rs:3`（流式重试语义）
- `crates/whcode-llm/src/retry.rs:13`（默认策略 5/1s/×2/30s/60s）
- `crates/whcode-llm/src/http/profile.rs:49`（五协议名）
- `crates/whcode-llm/src/http/protocols.rs:213`（openai 请求体）
