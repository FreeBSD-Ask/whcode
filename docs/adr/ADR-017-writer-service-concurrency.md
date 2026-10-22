# ADR-017 写者服务并发设计档案（T034）

**状态**：已批准并实现（用户 2026-10-21 批准；T034 实现与验证见 §11）
**日期**：2026-10-21 | **依据**：ADR-015（设计先行）、ADR-002（单写者边界）

本文件是 ADR-015 要求的**首个并发设计档案**。它先于代码存在；T034 的实现必须逐项兑现
本文的不变量与验证计划，否则不得合并。

## 1. 问题与约束

- 目标：在单进程内提供**有界队列 + 批量提交 + 幂等键 + 关键事件确认 + 失败可重试**的写者服务。
- 约束：DuckDB 单文件、单写者；不得引入 async 运行时；自有代码零 `unsafe`；
  队列必须有界；不得每会话新增常驻线程。
- 非目标：多进程写、分布式共识、跨平台并发语义差异（Windows 为唯一验收目标）。

## 2. 设计决策

采用 **一个专用写线程 + 有界通道 + 同步确认**：

```text
调用方 --try_send(WriteRequest)--> 有界通道 --> 写线程（唯一持有 Connection）
   ^                                                 |
   +-------- 确认通道（WriteOutcome） <--------------+
```

- `std::sync::mpsc::sync_channel(capacity)` 承载请求；容量由配置固定，默认 1024，上限 65536。
- 写线程独占 `duckdb::Connection`，不在调用方加 `Mutex<Connection>`。
- 写线程按“行数阈值（默认 500）或时间窗口（默认 50 ms）”聚批，单事务提交。
- 关键事件绕过时间窗口，立即 flush；确认在 COMMIT 成功后返回。
- 幂等键以数据库 `UNIQUE` 约束为权威；内存索引仅作快路径。
- 关闭：停止接收 → 排空队列 → 最后一次提交 → 线程 join；拒绝新请求返回明确错误。

## 3. 不变量

1. `seq` 在工作区内唯一、单调递增；同一幂等键至多一个持久化结果。
2. 已确认（COMMIT 成功）的事件在重启后仍可读；未确认事件不得被报告为成功。
3. 任何已接受请求最终得到“提交成功 / 确定失败 / 关闭拒绝”之一；不得静默丢弃。
4. 队列长度不超过配置容量；满时返回背压错误，不无限阻塞调用方。
5. 关键事件确认前，不得返回成功。
6. 写线程是唯一写入者；读连接不得执行写 SQL。

## 4. 线性化点与内存序

- **线性化点**：事务 `COMMIT` 成功返回。幂等键去重的线性化点是 `UNIQUE` 约束冲突的读取/插入。
- 通道的发送/接收提供 happens-before；不使用自旋或裸原子共享数据。
- 关闭标志若使用 `AtomicBool`，采用 `SeqCst`；其余状态由通道消息传递，避免额外内存序假设。
- 不引入无锁队列；无锁结构需单独 ADR 与模型检验，当前不满足收益/风险比。

## 5. 进展保证

- 写线程存活且数据库可用时，每个已接受请求**最终**提交或返回确定错误（无饥饿保证依赖
  `sync_channel` 的 FIFO 语义）。
- 数据库持续失败时，批次保留并按退避重试；重试次数与总时限可配置，超时返回可重试错误。
- 关闭过程中不再接收新请求，已接收请求排空；排空超时返回确定错误，不静默丢弃。

## 6. 失败模式

| 失败 | 行为 | 证据 |
|---|---|---|
| 队列满 | 返回背压错误，调用方可重试 | 压力测试断言 |
| COMMIT 失败 | 批次保留，按策略重试；不确认 | 注入失败测试 |
| 写线程 panic | 句柄标记不可用；后续请求返回确定错误 | 故障测试 |
| 关闭超时 | 返回确定错误；未提交批次保留在内存并记录 | 关闭测试 |
| 进程崩溃 | 已 COMMIT 数据可读；未 COMMIT 不可见 | 崩溃重启测试 |
| 幂等键冲突 | 返回既有结果或确定冲突 | 单元测试 |

## 7. 验证计划

1. **单元**：阈值/窗口、关键事件立即 flush、队列满背压、重复键、失败保留批次。
2. **并发压力**：N 个生产者 × M 条事件，断言 `seq` 连续唯一、无丢失、无重复确认；
   运行于 Windows 11 amd64，记录线程数、RSS、队列深度。
3. **确定性**：用可控时钟/注入失败复现批次边界；不依赖“跑几小时没崩”。
4. **崩溃**：提交后强杀，重启校验；未提交批次不得出现。
5. **不适用声明**：本设计不使用无锁结构，Loom/Shuttle 不适用于通道语义；若未来引入
   无锁结构，必须新增 ADR 并引入模型检验。

## 8. 文献依据（一手来源）

| # | 文献 | 采用点 | 与本案差异 |
|---|---|---|---|
| 1 | Herlihy & Wing, *Linearizability: A Correctness Condition for Concurrent Objects*, ACM TOPLAS 12(3), 1990, DOI 10.1145/78969.78972 | 线性化点定义 | 本案以事务 COMMIT 为线性化点；DuckDB 事务语义由引擎保证 |
| 2 | Hoare, *Monitors: An Operating System Structuring Concept*, CACM 17(10), 1974, DOI 10.1145/355620.361161 | 有界缓冲/条件同步 | 本案用通道而非 monitor；不共享连接 |
| 3 | Welsh, Culler & Brewer, *SEDA: An Architecture for Well-Conditioned, Scalable Internet Services*, SOSP 2001, DOI 10.1145/502034.502057 | 有界队列 + 背压 + 分阶段 | 本案只有单阶段写者，不引入事件驱动运行时 |
| 4 | DuckDB 官方并发文档（单写者/多读者） | 写入者唯一性 | 以本机实测与 `ADR-DB-BUILD.md` 为准，不把文档当运行证据 |

## 9. 回退方案

若专用写线程在实测中造成不可接受的延迟或复杂度：

- 回退到同步写者（M0 原型路径），保持相同 API 与幂等语义；
- 回退必须附同夹具前后测量、失败模式对比与 ADR 修订；
- 不得以“原型先行”方式直接替换本设计。

## 10. 退出条件

- 不变量 1—6 均有测试或注入证据；
- 压力测试在 Windows 11 amd64 上记录队列深度、RSS、线程数与延迟分位数；
- 用户或复核人批准本档案后，T034 才进入实现。

## 11. 实现与验证记录（2026-10-21）

**实现位置**：`crates/whcode-store/src/service.rs`（`WriterService`）包装 `writer.rs` 的
`EventWriter`。任务文本写作 `writer.rs`，实现拆为 `service.rs`，使同步写者引擎与线程服务
分离；模块归属仍是 `whcode-store`，未新增 crate、未改变分层依赖。

**不变量证据**（`cargo test -p whcode-store`，34 例通过）：

| 不变量 | 证据 |
|---|---|
| 1 `seq` 唯一、同键至多一个结果 | `multiple_producers_keep_seq_unique`、`duplicate_key_is_deduplicated_through_service` |
| 2 已确认事件关闭重开后可读 | `critical_event_is_confirmed`（关闭后重开库统计） |
| 3 已接受请求不得静默丢弃 | `queue_is_bounded_and_reports_backpressure`、`drop_drains_accepted_requests` |
| 4 队列有界、满时背压 | `queue_is_bounded_and_reports_backpressure`（闸门确定性填满容量 2） |
| 5 关键事件确认前不返回成功 | `critical_event_is_confirmed`、`writer.rs::critical_event_is_committed_and_confirmed` |
| 6 写线程唯一写入者 | 读路径不写库；`tests/contract/test_layering.rs` 维持前端不触库 |

**失败模式**：写线程 panic 由 `writer_panic_is_reported_and_later_requests_are_rejected`
覆盖（确认前退出返回可重试 `internal`，后续请求返回 `invalid_state`，`shutdown` 报错）；
COMMIT 失败保留批次由 `writer.rs::write_failure_propagates_and_batch_is_retained` 覆盖。

**压力证据**（`artifacts/writer-service-stress.json`，Windows 11 amd64）：
8 生产者 × 500 事件 = 4 000 条，批次 500 行 / 50 ms，队列容量 1024；用时 0.91 s；
库内 4 000 条且 `seq` 1—4000 唯一连续；背压重试 0；延迟 p50 = 45 µs、p95 = 62 µs、
p99 = 94.5 ms、max = 216 ms（长尾来自持有写线程的批次提交等待）；RSS 12.7 MB → 30.4 MB；
配置并发 9（8 生产者 + 1 写线程），收尾后进程线程 2。

**未覆盖与边界**：单机单次样本不构成预算冻结（V-006）；崩溃重启一致性归 T043；
数据库层故障注入与退避重试归 T043 / T042，本文只覆盖写线程 panic 与写者层提交失败。

**可观测性扩展（T038）**：`WriterService::queued_len()` 以 `AtomicUsize`（SeqCst）记录
等待中的请求数——发送前加一、写线程取走时减一、拒绝时回退；容量由 `capacity()` 暴露。
该指标只用于监控（`monitor::WriterStats`），不参与调度或正确性判定；写线程 panic 后可能滞后。

**与本文设计的已知偏差**（不得声称已实现 §5 的全部语义）：

- **退避与总时限未实现**：写线程在下一个时间窗口重试 `flush_if_due`，批次保留在内存；
  可配置退避、重试次数与总时限随 T042/T043 落地。
- **关闭无超时**：`shutdown` 阻塞 join 直至队列排空；排空超时与“确定错误”归后续任务。
- **队列深度未逐样本采样**：压力实验记录容量 1024 与背压重试次数（本次为 0），
  不记录瞬时占用。

