# turn WS relay 归档（ADR-058 §2.8.8 用例 8／10，FR-742／Check 347 条件 4）

- 日期：2026-11-09
- 平台：Windows 11 amd64（`x86_64-pc-windows-msvc`，Rust 1.99.1；验收目标平台，ADR-013）
- 夹具：`whcode_llm::StubProvider` 定速剧本（无网络、无凭据），经
  `ServeLlm.factory: Arc<dyn ProviderFactory + Send + Sync>` 注入；服务循环为 `serve.rs`
  的真实 `serve_loop`（内存 DuckDB 工作区，回环随机端口）。

## 本目录内容

| 文件 | 内容 |
|---|---|
| `turn-ws-relay-e2e.json` | 用例 8 三指标的原始测量与判定（OS／架构／构建类型／命令／场景／测量值） |
| `turn-resources.json` | 用例 10 的线程与句柄三组原始采样、派生差值与判定、`measurement_note`（口径修正说明） |
| `README.md` | 本文件：再生成命令与口径说明 |

## 再生成命令（仓库根目录执行）

用例 8（三指标；设环境变量即写盘，不设则只在日志里打印）：

```powershell
$env:WHCODE_TURN_WS_RELAY_ARCHIVE = "1"
cargo test -p whcode-bin turn_ws_relay_first_delta_busy_codes_and_event_missing -- --nocapture --test-threads=1
```

用例 10（资源实验；`#[ignore]`，运行即写盘）：

```powershell
cargo test -p whcode-bin turn_resources -- --ignored --test-threads=1 --nocapture
```

## 判定规则（由测试程序实施，不以口头结论代替）

用例 8：

1. 首帧 delta 延迟：从发起 `POST /api/llm/run` 到订阅者收到首个 `llm.chunk` 帧的毫秒数，
   必须不早于桩工厂的持锁窗口（900 ms）、不大于 5 s；
2. turn 期间响应码：触库路径 `GET /api/sessions` = 503 ＋ `code=backpressure` ＋
   `retryable=true` ＋ `Retry-After: 1`；`POST /api/llm/cancel` = 200；`GET /ws` 握手 = 101；
   `GET /` = 200 且注入标记原样保留（未注入业务数据，见 ADR-058 §2.3 收口说明）；
3. `event_missing == 0`：以 `whcode_app::events_export` 导出的期望 seq 集合与订阅者实收
   事件帧 seq 集合作差，差集为空。

用例 10：

1. `during_turn.threads - baseline.threads <= 2`；
2. `after_turn.threads <= baseline.threads`（结束后回到基线，不归零即失败）；
3. `after_turn.handles <= baseline.handles`。

## 口径与已知限制

- 时间字段与耗时字段（`latency_ms`、`ts_ms`、`duration_ms`、`elapsed_ms`）在跨路径比对中
  被剔除：两条路径不共享时钟，逐值比较无意义；其余字段必须逐字段一致。事件表比对不含
  `ts` 列（行级时间戳同上）。
- 资源实验的基线必须在**装配与预热之后**采样：首次测量把 `relay_security`／`seeded_session`
  的一次性装配（DuckDB 线程池、套接字与客户端）算进了 turn 增量，读数差 79—80 线程，已作废；
  旧读数与修正理由保留在 `turn-resources.json` 的 `measurement_note` 中。
- 事件与消息共用同一序号池（`MAX(seq)+1`），消息序号编码在 `MSG-<seq>`：用例 9 的“不丢事件”
  判据是两表合并后序号连续无空洞，而不是事件表单独连续。
- 测量为单次运行结果，不声称跨运行可重复性；作为 Check 347 条件 4 的端到端证据，CI 日志与
  artifact 为准（ADR-037）。
- 用例 11（差分回退验证）本机未取得，见 ADR-058 §3.6 的缺口登记与确切命令。
