# 技术债务复核报告（2026-10-25）

**范围**：全部已交付代码（M0—M6/M8 + Phase 13 + 资源铁律 + 四端对等收口 + TLS 进行中变更），
按 `#[test]` 计数 712 例（`crates/` + `tests/`，grep 口径）。
**取证基线**：工作树状态（HEAD `dc4e5b3` + 16 处未提交改动，属 T184—T192 TLS 变更的进行中工作单元）。
**方法**：静态红线扫描（五项，全部独立重跑）+ 契约测试覆盖 + 组件级分配有界抽查 + 任务清单
技术债标记逐项。0 信任执行：未沿用 2026-10-22 版报告的任何结论，全部证据为本日重跑所得。

## 结论：无未管理的债务

所有技术债均以**结构化标记**（阻塞/未取得/待验证/占位）显式登记在 `tasks.md`（42 处标记、
17 项未勾选任务）、`checklist.md`（35 处标记、26 项未勾选 Check）、ADR 与指南中，并附恢复
路径。无 TODO/FIXME/HACK 残留、无 unsafe（NFR-215 `forbid` 锁定）、无忙等、无每会话常驻
线程、无 mem::forget/Box::leak。与 2026-10-22 版相比：新增代码（HTTP Provider、四端对等、
`serve.rs` 多路复用服务器）未引入任何红线违例；`tui/src/views.rs` 已按 T150 下沉至应用层
（组件抽查已按新位置核实）。

## 静态红线扫描结果（五项全过）

| 红线 | 扫描结果 | 判定 |
|---|---|---|
| 无 TODO/FIXME/HACK/XXX | `crates/` 与 `tests/` 合计 0 处 | ✅ |
| 无 unsafe（NFR-215） | `Cargo.toml` 第 55 行 `workspace.lints.rust.unsafe_code = "forbid"`；grep `[[:space:]]unsafe[[:space:]]`：`crates/` 1 处命中且为注释（`whcode-runtime/src/js.rs:216` “无需 unsafe 闭包 API”，非代码）；`tests/` 0 处。实际 unsafe 代码 0 处 | ✅ |
| 无忙等（无 sleep 的 loop+try_wait/try_recv） | `worker.rs` try_wait 轮询在 `None` 分支 sleep 10 ms ✓；`ssh/server.rs` 客户端两处 loop 走 russh `channel.wait().await`（异步挂起，非轮询）✓；`ssh/proto.rs` loop 以 deadline + `tokio::time::timeout` 界定 ✓；`bin/serve.rs` serve_loop 整 tick 无进展停靠 `IDLE_PARK`（25 ms）✓；`app/gates.rs` loop 在测试代码内且以阶段到达为 break 条件 ✓；`app/context.rs` loop 以预算超限 break + 分页游标界定 ✓；`serve.rs` 写出泵/订阅者队列 loop 以队列深度与 WouldBlock 界定 ✓ | ✅ |
| 无每会话常驻线程 | 全部 spawn 站点逐一核对：非测试区仅两处，均为**启动期单例**——`ssh/adapter.rs:213`（`whcode-ssh-exec` 执行线程，`CommandExecutor::start` 仅在 `server.rs:623` 服务启动时调用一次，非每会话）；`store/service.rs:131`（`whcode-writer` 写者线程，启动期单例）。其余 8 处（`worker.rs:353/384`、`python.rs:619`、`adapter.rs:532`、`serve.rs:1520/1671`、`service.rs:512`、`platform/process.rs:303`）全部位于 `#[cfg(test)]` 测试代码 | ✅ |
| 无 mem::forget / Box::leak | `crates/` + `tests/` 0 处（含 `ManuallyDrop` 附加扫描） | ✅ |

## 组件级分配有界扫描（旧表 8 项 + 新增 serve.rs）

| 组件 | 分配 | 有界 |
|---|---|---|
| `app/queue.rs` BoundedQueue | `capacity` 字段 + `push_critical` 满载背压错误 + `push_snapshot` 淘汰最旧 | ✅ |
| `app/context.rs` items | 预算截断（`consumed + tokens > budget_tokens` → truncated/break） | ✅ |
| `llm/stream.rs` EventRouter | `CRITICAL_CAPACITY = 256`（常量）/ display 容量由构造参数给定 | ✅ |
| `store/query.rs` 分页 | `MAX_PAGE_SIZE = 1000`；`fetch_limit` 拒绝 0 与超限，内部取 `limit+1` 探页 | ✅（旧报告的 `1..=1000` 字面量已改为常量口径，等价强制） |
| `store/directory.rs` 列表 | `MAX_LIST_LIMIT = 1000`；“list limit 须在 1..={MAX_LIST_LIMIT}” | ✅ |
| `app/session.rs` switch | `SWITCH_TAIL_LIMIT = 50`，`tail_complete` 显式标注截断 | ✅ |
| `web/security.rs` + `web/api.rs` | `MAX_MESSAGE_BYTES = 1024 * 1024`（1 MB）；`parse_request` 体超限即拒 | ✅ |
| `tui` ViewSnapshot | **位置变更**：`tui/src/views.rs` 已不存在（T150 下沉），现为 `app/src/view_source.rs`（TUI 经 `pub use whcode_app::view_source as views` 别名引用）；渲染只消费，投影行数受上界（`limited_projection_is_bounded_and_consistent` 测试锁定） | ✅ |
| **新增** `bin/serve.rs`（单线程多路复用服务器） | 连接上限 `MAX_CLIENTS = 96`；订阅者上限 `MAX_WS_CLIENTS = 32`（超出握手后 1013 显式拒绝）；请求头 `REQUEST_HEADER_CAP = 16 KiB`；入站帧缓冲 `WS_INPUT_CAP = MAX_MESSAGE_BYTES + WS_HEADER_MAX`；每订阅者出站队列 `MAX_QUEUED_FRAMES = 64` 帧且溢出即断（慢客户端策略）；Writing 阶段单响应 buf（≤ 1 MB）+ 部分写残余；读空闲 5 s / 写停滞 `WRITE_STALL_TIMEOUT` 5 s；事件轮询 200 ms（`POLL_PAGE_LIMIT = 500`）；唯一循环线程空闲停靠 25 ms | ✅（并发设计档案见 `docs/adr/ADR-017-writer-service-concurrency.md` 与模块文档；每 tick 最坏工作量以常量界定） |

## 结构化技术债登记（按仓库分区实测计数）

| 分区 | 标记计数 | 性质 |
|---|---|---|
| `crates/`（.rs） | 77 处 | 绝大多数为领域语言与文档注释（`TaskStatus::Blocked` 状态名、审计夹具“阻塞解除”、有界性说明）；**真实债务仅 1 处**：`platform/src/fonts.rs:38` 第三方解包安装位置官方文档未单列，标 `[待验证]` |
| `specs/…/tasks.md` | 42 处 | 17 项未勾选任务（见下表） |
| `specs/…/checklist.md` | 35 处 | 26 项未勾选 Check（见下表） |
| `specs/…/spec.md` | 12 处 | 多为需求正文含“阻塞”字样（领域语言）+ D-01—D-14 待决策事项 + ADR-013 延后声明，非债务残留 |
| `docs/` | 27 个文件 | ADR 11 份（延后/有条件决策：ADR-005/007/009/011/013/027/030/031/039/040 等）、指南 13 份（方法论与已知限制声明）、矩阵 2 份（decisions.md、runtime-inventory.md） |

### 未勾选任务与 Check 明细（17 任务 / 26 Check）

| 类别 | 数量 | 明细 | 恢复路径 |
|---|---|---|---|
| 范围外延后（BSD，ADR-013） | 10 | Check 20/21/22/25/96/99/109（部分）/134（部分，另涉 ADR-005/010/012 部分批准）/195（部分）/201（部分）+ T099 | 新 ADR + 用户批准 + 目标平台真实环境，补分平台证据；不得以交叉编译代替运行验证 |
| 阻塞（需 CI 长跑数据） | 2 | T116、Check 116（NFR-204 5.5 h 长跑）；Check 195 部分（NFR-201—210 分平台实测与冻结） | `long-run.yml` 5.5 h CI 运行（据 `tasks.md`：2026-10-25 已触发 run 40156278359，main@dc4e5b3，seconds=19800；run 状态本机 `gh` 不可用，标 `[待验证]`）；数据落地前不勾选。本机 12 h debug 序列（`artifacts/stability-24h-20261024-150204/nfr204-analysis.md`）仅作候选证据 |
| 进行中变更（占位） | 6+15 | T162—T167（真实 LLM Provider：spec 未入库→ADR-031 已于 2026-10-25 补录入库 `docs/adr/ADR-031-llm-provider-network-and-credentials.md`；代码已在 `crates/whcode-llm/src/http/`，`mod.rs` 已声明全部 4 个子模块——14 个此前从未编译的测试已恢复；客户端与 `llm run` 装配未交付，四端 `llm.run` 保持 Reserved，ADR-032）；Check 274—288 + T184—T192（TLS 证书库变更，工作树未提交改动即本批进行中工作） | 里程碑推进：T163/T167 交付装配与四端登记翻转；T184—T192 按 ADR-040 逐项交付 |
| 人工/平台证据未取得 | 1 | V-004 SSH 服务端验收（Check 25/99/131，属 M7 + ADR-013 延后） | BSD 延后路径同上 |
| 诚实待验证 | 1 | `fonts.rs:38` 解包安装位置 `[待验证]` | 以官方文档或接口实测定案 |

### 与 2026-10-22 版已知债务清单的差异（独立复核修正）

- **V-003 交互式 TUI 四项已取得**：Check 24 已勾选，2026-10-23 真实控制台驱动
  （`CreateProcess(CREATE_NEW_CONSOLE)` + kernel32）10 项自动检查全部通过
  （`artifacts/tui-v003-2026-10-23/`），并顺带修复缺字缺陷（整行输出方案）。
  旧清单“人工证据未取得”已不再成立，本报告据实修正。
- **X4 删除吞吐**：实测 125 s @ 1M 行，已按 NFR-216 以实测修订标准 ≤ 300 s
  （`docs/guides/resource-budget.md:57`、`tasks.md:839`）；批量/分区删除为候选优化，须先冻结预算。
- **T162—T167 的阻塞理由部分过时**：任务文本仍写“阻塞：变更 spec 未入库”，但 ADR-031
  已于 2026-10-25 补录（同日批次）。实质剩余缺口是装配未交付与四端登记翻转，任务保持
  未勾选是正确的；建议下次触及该批次时同步刷新阻塞措辞（不构成债务违规）。

## 已偿还债务（2026-10-22 以来，抽样核验）

以 `tasks.md` 与 `CHANGELOG.md` 为准，抽查两条以上：

- **T155（TUI 内置字体）/ T156（G-01 检查点事务语义清偿）/ T157（GUI terminal-green 主题）**
  均已勾选，附各自验证记录（`tasks.md:331-333`）。
- **T168—T181 四端对等收口**：14 项全部勾选——能力登记表 54 条 SSOT、四端 surface_map、
  对等契约门禁 `tests/contract/test_frontend_parity.rs`、统一 not_supported 语义、
  `feature-parity.md` 重写为 54 行受校验表（CHANGELOG“收口（2026-10-24，T181）”条目与
  `tasks.md:362-375` 一致）。
- **T182 / ADR-038**：会话归档确认与审计口径冲突关闭，ADR-038 已入库，Check 272 登记。
- **T183 / ADR-039**：构建与发布迁入独立矩阵工作流，由 build-release run 40153942736 与
  ci run 40153942697 取证通过（tag 发布路径未实跑，已在 CHANGELOG 如实标注）。
- **ADR-031 补录与 http 模块接线修复**：`docs/adr/ADR-031-llm-provider-network-and-credentials.md`
  已入库；`http/mod.rs` 现声明 `errors/frames/profile/protocols` 四个子模块（本日实测），
  14 个此前从未编译的测试已恢复编译；注释中幻影 FR 编号已修正（CHANGELOG“收口
  （2026-10-25，ADR-031 补录）”条目）。
- **CI 测量缺口收口**：`fa0bdc8`（托管 runner 测量缺口）与 `b33befe`（Check 3/116/263
  复核记录）已提交，Check 263 勾选。

## 已知限制（不构成未管理债务）

1. NFR-204 长时泄漏斜率：等 5.5 h CI 长跑数据（T116/Check 116）；本机 12 h debug 序列
   仅作候选证据，不满足 ADR-033 的采样间隔与构建口径。
2. NFR-201—210 预算未冻结：仅 Windows 单平台观测（Check 195，ADR-035 已修订
   DuckDB 2.0 相关项，其余分平台与冻结仍待 ADR）。
3. `llm run` 四端 Reserved：HTTP Provider 客户端与装配未交付（ADR-032 复审触发③）。
4. TLS 变更（T184—T192）进行中：工作树 16 处未提交改动属该批次，验收证据未取得，
   全部 Check 274—288 未勾选。
5. 长跑 run 40156278359 的实际执行状态：本机无 `gh`，`[待验证]`。

## 处置建议

- **无需要重写的代码**：静态红线五项全过、组件分配有界全过（含新增 serve.rs 逐常量核实）、
  帧路径无 JSON 反序列化（T083 结构测试仍在）；
- **优先推进 T163/T167**：ADR-031 已入库，装配交付与四端登记翻转是 LLM 批次唯一剩余缺口；
  同时刷新 T162—T167 的阻塞措辞；
- **T184—T192（TLS）按 ADR-040 顺序交付**并在完成后复跑本报告五项红线
  （`serve.rs` 将新增 TLS 缓冲，须复核其上限）；
- **T116 等 `long-run.yml` 数据落地后勾选**，并以 CI 日志为唯一通过口径（ADR-037）；
- **BSD 项维持延后**，恢复须新 ADR + 用户批准 + 真实平台环境。

## 复核更新（2026-10-25 第二轮）：长跑数据落地，NFR-204 斜率超标

**更新范围**：本节为本日第二轮复核增量，更新上文“已知限制”第 1／5 条与“处置建议”的 T116 行；
其余结论（红线五项、组件有界、结构化登记）未重扫，仍以上文为准。

### 长跑执行状态核实（GitHub API，git 凭据）

- 上文 `[待验证]` 的 run 40156278359：**cancelled**（02:55—03:06 UTC，重复触发被
  `stability-long-run` concurrency 组处置）；**有效运行是 run 40154046118**——
  job `long-run` **success**（02:20:07—07:53:46 UTC，19860 s ≈ 5.5 h），main@dc4e5b3。
- 产物 `stability-long-run`（artifact 12591646947，1867 B，已解包至本地
  `local/longrun-40154046118/` 复核）：65 个样本、`kind:"summary"` 收尾行、
  `test stability_run_collects_section_11_5_series ... ok`（1 passed）。
- 序列健康度：`event_missing` 恒 0；线程 21／句柄 176 全程不变；RSS 首 34.37 MB、
  末 38.09 MB、峰值 47.32 MB。

### NFR-204 斜率：超预算（登记时不得勾选达标）

- summary 行自报斜率 **0.6987 MB/h**（首末口径），独立最小二乘复算 **0.943 MB/h**，
  均 **> 0.5 MB/h 预算**（NFR-204）；summary 自标“NFR-204 候选斜率”。
- 两项 §11.5 指标未取得：写者队列深度／最老队列项年龄（`sample.unavailable`），
  Check 116 判据“含 §11.5 全部指标”仍不满足。
- 异常一处：样本 28（2.33 h）事件计数自 391 800 重置为 102 080、CPU 15.78%、
  RSS 峰值 47.32 MB——工作循环重启，`failed_tasks=1`；登记时须注明。
- 处置：斜率超标按 NFR-216 流程办理——先优化或经 ADR 修订预算，不得默认达标；
  T116/Check 116/195 登记时应记录“数据已落地、斜率超标、两项指标未取得”。

### 仓库状态变化（相对第一轮基线 dc4e5b3＋16 处在途）

- **5281a39 已入库** UI 现代化批次（上文“已知限制”第 4 条所述 16 处在途改动已清）；
- 当前工作树 **35 处未提交改动**属 TLS／设置页批次延续（`tls.rs`、`cert_probe.rs`、
  `crates/llm/src/http/tls.rs`、`web frontend/settings.{html,js}`、安全默认值契约测试等），
  仍为进行中工作单元，不作为债务；
- tasks.md／checklist.md 本轮复核时点无未提交改动（并行会话中途状态波动，
  后续登记须先与 `git status` 对账）。
