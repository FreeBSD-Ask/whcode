# ADR-050：流式生成重试（策略与 safe_to_retry 判据）

- **状态**：已批准（2026-10-31，建议书 proposal-v2 D9 批准后实施；实现由并行会话先行
  落码 `3cbabf0`/`4984cc9`，本 ADR 补齐设计文档化与 0e84507 批次补齐接线/测试/回归修复）
- **来源**：`research/zcode-comparison/` 域 K（ZCode 重试策略证据）+
  `research/better-harness/`（friendly-cli 的 retryable/safe_to_retry 契约语义）+
  `research/synthesis/proposal.md` P1-2
- **责任人**：whcode-llm 维护者
- **退出条件**：真实会话统计显示首 chunk 失败率 < 1%（重试形同虚设）或重试放大故障
  （熔断/雪崩迹象），须复审策略参数或回退为不重试

## 1. 事实与证据

1. 流式生成路径此前**不重试**：`http/client.rs` 的 `RetryPolicy::none()` 注释明确
   "流式生成路径的语义"——单次网络抖动即废掉整个 turn（R1 后 turn 是多轮编排，
   单步失败代价放大）。
2. 探测路径（幂等 GET）已有重试（`probe_get`，ADR-031 决策 8），429 的
   `retry_after_ms` 已随错误 details 归一化——供应商口径的等待值可用。
3. BH 的 `friendly-cli.md` 契约语义：`retryable`（条件可能随重试改善）与
   `safe_to_retry`（重放不产生重复效果）是两个概念——"远端写后超时可能 retryable
   但不 safe to retry"。
4. ZCode 对照（域 K）：maxRetries=10、baseDelay 2000ms、×2、上限 60s、jitter。

## 2. 决策

1. **safe_to_retry 判据（核心）**：只有当调用方**尚未消费任何成功 chunk** 时才允许
   重放——零输出已产生，重放不产生重复事件/重复正文（幂等充分条件）；一旦有 chunk
   已发射，任何失败原样上抛（调用方已持有部分输出，重放必然重复计费与事件）。
2. **落点**：Provider decorator（`retry.rs::with_stream_retry`），不侵入五协议实现；
   连接期失败（execute 即错）与流建立后、首 chunk 前的失败统一在 decorator 判定。
3. **默认策略（交互优先的当前和解）**：max_retries=5、base_delay=1s、×2、
   max_delay=30s、total_budget=60s——对照 ZCode 10×2s（§1.4）：交互场景长时间阻塞
   调用线程（Web 单线程循环）的代价高于边际成功率；策略经工厂装配可调。
4. **等待口径**：错误 details 的 `retry_after_ms`（429 backpressure）优先于固定退避；
   退避睡眠 50 ms 切片、**取消感知**（每片检查取消标记，取消即上抛）。
5. **可观测**：实际尝试次数并入 `TransportSnapshot.reconnect_attempts`（信封
   `transport` 既有字段，四端可见）。
6. **不伪造**：重试耗尽/取消/超预算一律上抛最后一次真实错误；重启请求失败不得以
   "流结束"（返回 None）伪造成功——`RetryStream` 重试环显式上抛（0e84507 批次
   回归测试 `restart_failure_is_surfaced_not_silently_dropped` 固化）。

## 3. 正负和解

- **正面**：单次抖动不再废掉整个 turn；供应商 Retry-After 被尊重；取消即时生效。
- **负面**：失败路径的延迟上界变长（最多 60 s）；decorator 增加一层间接；
  退避期间占用调用线程（同步语义，不新增线程）。
- **承担者**：whcode-llm 维护者（策略参数）、用户（偶发的长等待）。
- **和解理由**：交互优先数值 + 总预算上界把最坏情况定死在 60 s；真实失败率统计
  （§5）决定策略是否保留。

## 4. 三重时间

- **过去来源**：ZCode retry-policy 一手源码（域 K）；BH 契约语义；本仓探测重试先例。
- **现在证据**：`crates/whcode-llm/src/retry.rs` 7 项模块测试（重试成功/中途不重试/
  非重试不重试/次数上界/重启失败上抛/策略校验），提交 0e84507。
- **将来失效条件**：见头部退出条件；复审触发见 §5。

## 5. 复审触发

- `cargo test -p whcode-llm retry` 随每批提交执行——判据失效即触发；
- 真实供应商会话启用后：从事件统计 RequestFailed（首 chunk 前）占比——若 > 5%
  说明策略未生效或供应商不稳，若 < 0.1% 连续两个批次说明策略冗余；
- 依赖升级（russh/ureq）触及传输层时复核 decorator 与错误分类的兼容性
  （2026-10-31 russh 0.65 破损先例：依赖 PR CI 红灯不得合入）。

## 6. 实施摘要

- `3cbabf0`/`4984cc9`（并行会话）：retry.rs 落码。
- `0e84507`（本批）：工厂接线（`LocalProviderFactory::create` 包 decorator）+
  7 项模块测试 + 重启失败静默结束缺陷修复 + 本 ADR 落档 + FR-730 登记。

## 7. 修订记录

- **2026-10-31（首版，已批准）**：D9 批准后由并行会话落码、本会话补齐设计与登记；
  编号 050（049 已被宪法原则 X 占用）。
