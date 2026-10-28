# 域 K：LLM Provider 层

> ZCode 证据路径以 `local/zcode-src`（commit 3c5b227）为根，以下省略前缀，实际绝对路径为
> `D:\WHCode\local\zcode-src\...`。

## 1. 关键路径勘误

原任务列出的 `packages/core/src/model/`、`packages/provider/` **实际不存在**。真实结构：
- 重试/超时/脱敏在 `apps/zcode-cli/packages/adapters/src/model/`；
- 凭据密文在 `.../adapters/src/auth/`；
- 4 层合并配置在仓库根 `packages/provider/`（`@zcode/provider`，resolver/registry/config-service）；
- model-option-map 在根 `packages/model-option-map/`（`@zcode/model-option-map`）。

## 2. 对照表

| 项 | ZCode | WHCode | 判定 |
|---|---|---|---|
| 协议面 | 3 种协议注册于 `adapters/src/model/model-execution.ts:396-404` `toAiSdkProviderConfig`：anthropic-messages→`@ai-sdk/anthropic`；openai-responses→`@ai-sdk/openai` `.responses`；openai-chat-completions→`@ai-sdk/openai-compatible`；**确实用 Vercel AI SDK**（`ai`/LanguageModel、streamText `runner.ts:312`）。GLM/z.ai：zai 家族走 `https://api.z.ai/api/paas/v4`（standard，openai-chat-completions）或 `https://api.z.ai/api/anthropic`（coding-plan，anthropic-messages）；bigmodel 走 `open.bigmodel.cn` 同构 | 5 种（openai/anthropic/gemini/ollama/cohere，ADR-031），自研 WireProtocol trait + registry 分派，帧格式 Sse/Ndjson/JsonArray | WHCode 协议面更宽、依赖更少 |
| 流式 | AI SDK streamText + `runner-normalization.ts:16` `normalizeUsage` 归一 | 同步迭代器 ChunkStream（非 async），DecodeOutcome 六态归一 | 相当，风格不同 |
| 重试 | `retry-policy.ts:13-30` 默认 `maxRetries=10`（`maxAttempts=11` 含首次）、`baseDelayMs=2000`、`backoffFactor=2`、`maxDelayMs=60000`、`jitter=true`（0.5–1.0×）；可调 `ZCODE_MODEL_RETRY_MAX_RETRIES/BASE_DELAY_MS/BACKOFF_FACTOR/MAX_DELAY_MS`；`retry-budget.ts` Unbounded/SingleAttempt；`empty-completion-retry.ts:8` 空补全 `EMPTY_COMPLETION_MAX_RETRIES=1`；Retry-After 优先（`runner-retry.ts:15,47`，上限 5min） | 仅幂等 GET 探测重试 2 次（退避 1s/2s）；生成请求不重试 | **ZCode 先手**：流式生成重试是短板补齐项 |
| 超时 | `stream-idle-timeout.ts` 默认 `DEFAULT_MODEL_STREAM_IDLE_TIMEOUT_MS=600000`（10min，`contracts/src/config/index.ts:284`），**每次重试递增 30s**（`+30_000`）；`readNextWithStreamIdleTimeout` 用 `Promise.race` + 自建 `AbortController` 主动 abort | connect 10s / read 120s / total 300s 三档 | WHCode 更完整（三档）；ZCode 有“”独特设计 |
| 取消 | `createLinkedAbortController`（`stream-idle-timeout.ts:63`）把父 signal 链到 attempt AbortController——**不完全依赖 SDK**，自带 abort 传播与主动结束等待 | CancelToken（AtomicBool），下一帧边界生效（cancel_effective_at 语义明确） | **修正**：ZCode 取消语义**不弱于**原判（自带 abort 传播+主动结束） |
| 配置 | 4 层合并确凿（`packages/provider/resolver.ts:183-208`）：zcodeBuiltin→account（zhipu-account 权益 overlay）→template→personal；`ConfigOverlay`（`config-overlay.ts`）实现缺省继承/Config 递归覆盖/其他值整体替换；`model-option-map` 用受限 CEL 表达式（`compiler.ts`）+ JSON Merge Patch 编译（`merge-patch.ts:13`），经 `model-option-map-fetch.ts` 在 fetch 层改写请求体（reasoning/max-output 权威） | `http/profile.rs` ProviderProfile 静态档 | ZCode 配置分层更灵活 |
| 凭据 | api-key 存 personal 层 `provider_config.json`（`runtime-paths.ts:5`）；OAuth token 经 `shared-credentials.ts` 加密落盘；`credential-cipher.ts:4-7` 确认 `enc:v1:` 前缀、aes-256-gcm、IV=12B、authTag=16B、`sha256(secret)` 派生密钥；密钥来自 `ZCODE_CREDENTIAL_SECRET` 或 fallback `zcode-credential-fallback:{platform}:{homedir}:{user}` | 凭据仅 `local/sensenova.env` 本地文件，不入库不入日志 | 定位差异：WHCode 靠不落库；ZCode 靠落库加密。若引入静态加密需 ADR |
| 计量 | `normalizeUsage` 把 AI SDK `LanguageModelUsage` 归一为 `ModelUsage`（input/output/total、cacheRead/Write、reasoning、serverToolUse） | `metering.rs`：prompt/completion tokens、延迟、成本（估算必标 cost_is_estimate，供应商值不混写） | WHCode 可观测性更严谨 |
| 脱敏 | 两套收口：CLI 侧 `runner-debug-redaction.ts`（模型 I/O 落盘脱敏 image/video dataUrl + header sanitize）；shared 侧 `telemetry.ts`/`telemetryRedaction.ts`（`redactTelemetryText` 归一 URL/email/path/凭据、模型 ID 白名单 `TELEMETRY_SAFE_BUILTIN_MODEL_IDS`，未命中降级 `custom`） | 追踪四级（off/metadata/redacted/full），字段名+值前缀双规则，IntegrityFlags 四布尔入载荷可查 | **修正**：ZCode 脱敏成体系（text 归一+模型白名单+model-io 媒体脱敏），**不逊于** WHCode |

## 3. 与 WHCode 差异修正（相对原差异书）

- 原判定**基本成立**：WHCode 5 协议（openai/anthropic/gemini/ollama/cohere）确实更宽。
- **修正**：ZCode 取消语义**不弱于**原判（自带 abort 传播+主动结束，非纯依赖 SDK）；
  ZCode 脱敏成体系（text 归一+模型白名单+model-io 媒体脱敏），不逊于 WHCode。
- ZCode 先手确为流式生成重试（attempt 循环+空补全重试+stream-idle 重试+签名修复重试）。

## 4. 存疑项

- `@zcode/model-option-map` 的 `option-maps.ts` 具体 map 源（map 模板随下载的
  zcode-builtin.json 发布）`[待验证]`。
- z.ai/GLM 模型白名单随远端发布，仓库内仅测试夹具可见 `[待验证]`。
