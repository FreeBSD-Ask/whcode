# Feature Specification: 真实 LLM Provider 接入（add-real-llm-provider）

- **Feature Branch**：`002-add-real-llm-provider`
- **Created**：2026-10-25（补登记；对应代码提交 `3f9564c`，2026-10-25）
- **Status**：**补登记（2026-10-25）：代码早于 spec，门禁倒置已由 ADR-031 记录**；本变更**未完成**——
  阻塞式客户端装配与四端 `llm.run`／`llm.cancel` 接线未交付
- **Input**：用户指令“为并行变更 `add-real-llm-provider` 补写正式 spec 并入库”（2026-10-25）。
  事实来源：提交 `3f9564c`、`docs/adr/ADR-031-llm-provider-network-and-credentials.md`、
  `docs/adr/ADR-032-frontend-parity-and-intended-differences.md`、`CHANGELOG.md` 2026-10-25
  “ADR-031 补录”条目、`specs/001-whcode-harness/tasks.md` T162—T167
- **需求归属**：本变更**不新造 FR 编号**；它把主基线 FR-201—FR-208、FR-616 与 NFR-213／215
  落到“真实供应商”这一具体接入面上。FR 全文以 `specs/001-whcode-harness/spec.md` 为唯一出处
- **适用版本**：`0.1.0`（开发期，未发布）
- **复审日期**：2026-10-25 登记；复审触发见 §8.3

## 0. 交付状态判定口径

| 状态 | 判据 |
|---|---|
| 已交付 | 代码与单测锚点存在，且不依赖未交付的调用点即可验证 |
| 部分交付 | 协议／模型／错误归一层的实现与单测存在，但网络请求调用点或端到端证据缺失 |
| 未交付 | 无实现，仅有决策（ADR）或主账本任务位 |
| 基线已交付 | 由主基线既有任务交付；本变更不新增能力，只复用 |

**当前总判定**：主账本 T162—T167 全部仍未完成（`specs/001-whcode-harness/tasks.md` 未勾选）。
代码实际覆盖到“剖面与凭据模型 + 三格式有界流解码 + 五协议适配 + HTTP/传输错误归一”，
**没有任何 HTTP 请求调用点**，四端 `llm.run`／`llm.cancel` 仍为 `Reserved`（ADR-032 修订记录）。
本 spec 是对既成事实的补登记，**不追认门禁按序完成**。

## 1. 背景与要解决的真实矛盾

主基线已把“本地优先、默认不联网、凭据不落日志”写为默认值（NFR-213、D-05），而
FR-201—FR-208 与 FR-616 要求一次真实模型调用的完整可观察链路（事件、追踪、计量、完整性、
传输进度）。真实供应商接入时，两者在五处直接冲突：

1. **默认离线的可证伪性**：引入网络栈后，“默认不联网”会从“没有代码路径”退化为“靠开关约定”。
   约定可被绕过，代码路径不能。
2. **凭据泄漏面**：凭据进入进程后，日志、事件、错误 `details`、测试夹具与崩溃转储都是泄漏面。
3. **供应商差异扩散**：线协议不止一种（OpenAI 兼容、Anthropic、Gemini、Ollama、Cohere）；
   逐家写分支会把供应商差异扩散到领域 `Provider` 接口。
4. **不可信流**：畸形 JSON、超长帧、未知事件、重复或缺失 chunk 会让“成功”与“静默截断”难以区分。
5. **失败可判定性**：凭据缺失、非法端点、超时、取消、网络错误与 HTTP 状态必须映射为既有错误码，
   不能以“请求失败”一句代替。

本变更的形式是：以**剖面（profile）**声明接入点，以**同步客户端 + 有界解码器 + 错误归一化**
承载传输，以**装配根唯一启用 feature** 让“默认离线”结构性成立。取舍与门禁倒置记录见
ADR-031；四端登记协调见 ADR-032。

## 2. 用户故事与验收场景

### User Story 1 - 声明接入点与凭据边界（Priority: P1，已交付）

用户以剖面声明协议、端点、模型、鉴权来源与时限；密钥只来自环境变量或仓库外文件，
任何输出面都不出现密钥值。

**Independent Test**：不发起网络请求，仅构造剖面并求值凭据；用未设置的环境变量、
空文件、不可读路径、明文外网端点分别触发失败，检查错误消息与 `Debug`／`Serialize` 输出。

**Acceptance Scenarios**：

1. **Given** 五个已注册协议名，**When** 按名解析，**Then** 得到对应枚举；未注册名返回 `None`
   并由调用方转为结构化错误，不静默回落（FR-203、ADR-031 决策 9）
2. **Given** 环境变量缺失或为空，**When** 求值凭据，**Then** 返回 `invalid_state`，消息含变量名、
   不含值（FR-203、NFR-213）
3. **Given** `Secret`，**When** 使用 `Debug` 或序列化，**Then** 输出恒为 `[REDACTED]`（FR-203）
4. **Given** 明文 `http://` 指向非回环主机，**When** 校验端点，**Then** 拒绝并提示改用 `https`
   （NFR-213、ADR-031 决策 5）

### User Story 2 - 协议适配与有界流解码（Priority: P1，部分交付）

同一套帧解码器与协议适配器把五种线协议的流式响应归一到“增量／终止／用量／未知／错误”五种
语义；超长输入必须报错，不得截断后报告成功。

**Independent Test**：纯函数夹具（无网络）：构造 SSE、NDJSON、JSON 数组流与五协议代表帧，
断言增量、终止原因、用量、未知事件与畸形输入的输出。

**Acceptance Scenarios**：

1. **Given** OpenAI 兼容 SSE，**When** 解码 `delta`、`finish_reason` 与 `[DONE]`，**Then** 得到
   增量、终止原因与供应商用量（FR-206、FR-207）
2. **Given** 单帧超过 1 MiB 或单响应累计超过 64 MiB，**When** 解码，**Then** 返回 `parse_error`，
   不静默截断（FR-206、FR-208、NFR-215）
3. **Given** 未识别的协议事件，**When** 解码，**Then** 映射为 `Unknown` 并要求计数留痕，
   不得 panic 或静默丢弃（FR-206、FR-208）
4. **Given** 剖面 `extra_body` 与协议默认体同键，**When** 构造请求体，**Then** 以剖面为准；
   `extra_body` 非对象时返回 `invalid_state`（ADR-031 决策 9）

### User Story 3 - 失败归一与传输可观察（Priority: P1，部分交付）

连接、TLS、读取超时与 HTTP 状态统一映射为既有错误码并附可操作 `details`；传输进度
（连接、断开、重连、字节计数、RTT、断连原因码）进入统一事件体系，可经 CUI JSON 查询导出。

**Independent Test**：以构造的错误值与 `TransportTracker` 断言映射与事件载荷；不访问公网。

**Acceptance Scenarios**：

1. **Given** HTTP 429，**When** 归一化，**Then** `backpressure` 且 `retryable = true`，
   `details.retry_after_ms` 在头部可解析时填入（FR-201、FR-616）
2. **Given** 连接被拒绝或读取超时，**When** 归一化，**Then** 分别为 `io_error` 与 `timeout`，
   可重试性交给调用方决定（FR-201、FR-616）
3. **Given** 一次传输，**When** 记录连接与断开，**Then** 产生 `ProgressChanged` 事件，载荷含
   `state`、`bytes_sent`、`bytes_received`、`rtt_ms_samples`、`reconnect_attempts`、
   `disconnect_reason`，且 RTT 采样不超过 64 个（FR-616、FR-208）

### User Story 4 - 装配与四端接线（Priority: P1，未交付）

用户在 CUI／TUI／Web 显式选择剖面后发起一次真实调用并看到流式增量、计量与失败原因；
未显式选择时任何入口都不联网。

**Independent Test**：本地回环桩服务器（不访问公网）+ 四端契约断言；当前**未交付**，
故只有反面证据：`llm.run` 四端登记为 `Reserved`，CUI 返回退出码 55、Web 返回 501。

**状态**：未交付（T167）。本 spec 不为其写通过判据，只登记缺口与完成判据，防止绿灯下的事实漂移。

### 边界情况

- 凭据文件存在但为空或只有空白：按“凭据缺失”失败，不回落到环境变量（默认值可见、不隐式）。
- 端点含 userinfo（`https://user:pass@host`）、IDN 主机或多段端口：当前校验宽松，
  已登记为 ADR-031 缺口 3，归 T162。
- 供应商在流中途返回错误事件：转为结构化错误，不得当作正常终止。
- 同一请求既有增量又有终止（Ollama 末帧）：归一为“增量 + 终止”，不丢正文。
- 用户取消：桩 Provider 在下一 chunk 前生效；真实 HTTP 读循环中的取消语义未交付（T166）。

## 3. 需求映射与交付状态（FR-201—FR-208、FR-616）

| FR | 需求要点 | 本变更验收场景 | 当前状态 | 代码锚点与证据 |
|---|---|---|---|---|
| FR-201 | 统一事件类型（请求开始／增量／完成失败／进度／运行时终止等） | US3-1、US3-2：状态与传输错误归一；协议层产出增量／终止／错误语义 | 部分交付 | `crates/whcode-llm/src/http/protocols.rs` 的 `DecodeOutcome`；`crates/whcode-llm/src/http/errors.rs`；事件类型基线在 `whcode-domain` 的 `EventKind` 与 `whcode-llm/src/stream.rs`。**缺口**：无调用点把响应接入事件路由 |
| FR-202 | 实时广播；断线后按持久化游标补拉 | 基线能力，本变更只复用；真实断连重连未交付 | 基线已交付（本变更未新增） | `crates/whcode-store/src/directory.rs::event_cursor`、`crates/whcode-store/src/query.rs` 游标测试；重连决策见 ADR-031 决策 8，实现归 T163 |
| FR-203 | 四级追踪；默认不把秘密写入日志与事件 | US1-2、US1-3：凭据求值与 `Secret` 输出面 | 已交付（模型与凭据边界） | `crates/whcode-llm/src/http/profile.rs`（`Secret`、`CredentialRef::resolve`、`validate_endpoint`）、`crates/whcode-llm/src/trace.rs` 四档与双层脱敏；测试 `secret_never_leaks_via_debug_or_serialize`、`redacted_masks_suspicious_fields_and_marks_integrity` |
| FR-204 | 四类 ID 串联一次调用 | 错误 `details` 携带 `request_id`；链路本身为基线能力 | 基线已交付（本变更补错误侧串联） | `crates/whcode-llm/src/correlation.rs`；`http/errors.rs` 的 `details.request_id` 与测试 `details_carry_provider_protocol_status_without_secret_surface` |
| FR-205 | CUI JSON／JSONL 事件查询导出并注明脱敏策略 | 基线能力；传输事件经既有导出可读 | 基线已交付（本变更未新增导出路径） | 主基线 CUI 事件导出；载荷来源 `crates/whcode-llm/src/transport_events.rs` |
| FR-206 | 积压时合并／降频，不得静默丢失关键转换 | US2-1、US2-2、US2-3：有界解码与未知帧计数 | 部分交付 | `http/frames.rs`（`MAX_FRAME_BYTES` 1 MiB、`MAX_RESPONSE_BYTES` 64 MiB、超限 `parse_error`）、`http/protocols.rs` 的 `DecodeOutcome::Unknown`；基线 `stream.rs` 双队列与可查询标记。**缺口**：真实流的标记填充未接线 |
| FR-207 | token、参数、延迟、成本；估算值标记，供应商计量另存 | US2-1、US2-4：五协议用量归一，`cost_is_estimate = false` | 部分交付 | `http/protocols.rs` 的 `usage` 归一；`crates/whcode-llm/src/metering.rs` 本地估算；`ProviderProfile::max_tokens`。**缺口**：真实调用的延迟／成本入账未交付 |
| FR-208 | 完整性标记（缺失 chunk、脱敏、降采样、持久化失败） | US2-2、US2-3、US3-3：超限报错、未知帧、脱敏标记、有界采样 | 部分交付 | `http/frames.rs`、`http/protocols.rs`、`crates/whcode-llm/src/trace.rs` 的 `IntegrityFlags`。**缺口**：chunk 序号连续性校验与持久化失败标记未接线 |
| FR-616 | 传输进度事件（连接／断开／重连／字节／RTT／断连原因码可查询） | US3-3：事件载荷与快照；时限三元组显式 | 部分交付 | `crates/whcode-llm/src/transport_events.rs`（T132 基线，2 例单测）、`ProviderProfile::timeouts`。**缺口**：无真实连接数据源（无调用点），`Provider::transport_snapshot` 默认返回 `None` |

**覆盖说明**：FR-202 与 FR-205 由主基线交付，列入本表只为说明“本变更不重复实现、不据其宣称交付”；
其余七条在本变更内均有实现或部分实现，但**没有任何一条达到端到端可用**。

## 4. 失败路径验收

| 场景 | 期望语义 | 证据或缺口 |
|---|---|---|
| 凭据环境变量未设置或含非 UTF-8 | `invalid_state`，消息含变量名、不含值 | `profile.rs::CredentialRef::resolve`，测试 `secret_never_leaks_via_debug_or_serialize` |
| 凭据文件不可读或为空 | `invalid_state`，消息只含路径、不含内容 | `profile.rs::CredentialRef::resolve`（`File` 分支**无专门单测**，归 T162 补齐） |
| 非法端点：非 `http(s)`、明文外网、缺主机名 | `invalid_state`，消息可操作 | `profile.rs::validate_endpoint`，测试 `endpoint_https_required_and_loopback_http_allowed` |
| 非法端点：userinfo、IDN／punycode、异常端口 | 当前未拒绝或校验宽松 | 已知缺口（ADR-031 缺口 3），归 T162 |
| 超时 | `timeout`，`retryable = true`；连接／读取／总时限分别可配 | `errors.rs::normalize_transport`，测试 `transport_errors_distinguish_timeout_from_io`；**真实时限执行未交付**（T163） |
| 取消 | 桩 Provider 在下一 chunk 前生效并说明生效时机 | `provider.rs`，测试 `cancel_takes_effect_before_next_chunk`；**HTTP 读循环取消未交付**（T166） |
| 网络错误（连接拒绝、TLS 失败、IO） | `io_error`／`timeout`；可重试性由调用方决定，不自动重放流式生成 | `errors.rs::normalize_transport`（ADR-031 决策 8） |
| HTTP 状态 | 400／405／422→`invalid_state`；401／403→`permission_denied`；404→`not_found`；429→`backpressure`（含 `retry_after_ms`）；5xx→`internal`；其余→`internal` | `errors.rs::normalize_status`，测试 `status_map_matches_fr212`、`retry_after_parses_seconds_only` |
| 错误响应正文过大 | 最多读取 64 KiB；正文只用于提取消息，**不得作为控制指令执行** | `errors.rs::read_error_body`、`MAX_ERROR_BODY_BYTES` |
| 凭据出现在日志／事件／错误 | `Secret` 的 `Debug`／`Serialize` 恒为 `[REDACTED]`；错误只含来源名或路径 | 模型层测试已覆盖；**端到端日志断言未交付**（随 T167 装配补，缺口如实登记） |

## 5. 非功能要求与边界

| 项 | 要求 | 现状证据 |
|---|---|---|
| 内存有界（NFR-215） | 单帧 ≤ 1 MiB、单响应累计 ≤ 64 MiB、错误正文 ≤ 64 KiB、RTT 采样 ≤ 64；禁止无界累积 | `frames.rs`、`errors.rs`、`transport_events.rs` 常量与超限测试 |
| 线程与并发（NFR-216 门槛） | 同步客户端，不引入异步运行时；不新增每会话常驻线程；调用占用发起线程 | ADR-031 决策 3；`Cargo.toml` 无异步运行时 |
| 默认不联网（结构性） | `ureq` 为可选依赖，`http` feature 默认关闭且唯一启用者为 `whcode-bin`；启动、`doctor`、`serve` 与空闲路径无请求调用点 | `crates/whcode-llm/Cargo.toml`、`crates/whcode-bin/Cargo.toml`；`grep -rn "ureq::" crates` 仅命中 `http/tls.rs`（相邻变更 T188）与注释 |
| 零 `unsafe`（NFR-215） | 自有 crate 普通 Rust 禁 `unsafe`、禁 `mem::forget`；`ring` 内部实现不构成自有代码承诺 | workspace lint `unsafe_code = "forbid"`，契约测试 `crates_forbid_unsafe_code_via_workspace_lints` |
| 依赖只进装配根（ADR-031 决策 1） | `whcode-domain` 与四个前端 crate 零网络依赖 | `tests/contract/test_layering.rs`（前端不触库／互不依赖）与 `cargo tree` 人工核验 |
| 发布体积 | 当前无调用点，实测增量为 0 字节；接入调用点后必须重测 | ADR-031 证据节（三次 release 构建均为 21 124 096 字节） |
| 凭据纪律（NFR-213） | 只允许环境变量或仓库外文件；不入数据库、事件、日志与夹具 | `profile.rs`；ADR-031 决策 6 |

**依赖事实（以 `Cargo.lock` 为准）**：`ureq 3.5.0`（`default-features = false`，仅 `rustls`）、
`rustls 0.23.46`、`ring 0.17.14`、`webpki-roots 1.0.11`。ADR-031 正文记 `webpki-roots 1.0.10`，
与锁文件不一致；本 spec 采信锁文件，该注记差异作为发现项上报（本变更不修改 ADR）。

## 6. 范围外与非目标

1. **四端 `llm.run`／`llm.cancel` 保持 `Reserved`**：任何前端接线前不得翻转登记表、
   矩阵与 `llm_run_stays_reserved_while_no_frontend_wires_it` 断言（ADR-032 复审触发③）。
   本变更交付 crate 层能力，不等于前端可用。
2. **异步、HTTP/2、QUIC、mTLS、企业代理**不在本变更：引入异步须先补 ADR-015 并发设计档案；
   代理配置与自定义 CA 未接线（自定义 CA 与系统证书库由相邻变更 `add-tls-certificate-store`
   ／ADR-040 的 T188 承担，不并入本变更的交付范围）。
3. **不新增 FR 编号**：五种线协议与剖面字段属工程决策（ADR-031 决策 9），
   此前代码注释引用的 FR-209—214 为幻影编号，已改为既有编号。
4. **不做供应商专属 SDK、不做请求队列与并发调度、不做多路复用**。
5. **不做工具调用协议**：协议层只识别协议自带的终止原因，不执行任何模型输出作为指令。

## 7. 正负和解

- **正面**：网络能力进入项目但“只有装配根能联网”；TLS 不依赖 OpenSSL；
  凭据只有两种来源且输出面恒脱敏；流解码与错误正文均有界；超时三元组显式；
  供应商差异被收敛在剖面与适配器内，领域 `Provider` 接口未改动。
- **负面**：引入 `ring`（含 C 与汇编）扩大第三方安全面与许可证审计面；同步客户端在慢响应下
  占用调用线程；无自动重试，瞬时故障只能由用户重试；端点校验仍有已知宽松项；
  发布体积在当前为 0 增量，但接入调用点后必然增大。
- **承担者**：本变更维护者（T162—T167 剩余实现与端点加固）；安全复核人（依赖公告与许可证）；
  企业网络用户（自定义 CA／代理需另立方案或依赖 ADR-040 的受控支持）；四端维护者（接线时的
  登记税与矩阵同步）。
- **和解理由**：以“依赖只进装配根 + 无调用点即零网络”换取默认离线的可验证性；
  以同步客户端避免为单请求引入异步运行时（NFR-216 的设计门槛）；
  以显式剖面承载供应商差异，避免逐家分支污染领域接口；
  以“宁可报错不可静默截断”承担更多可见失败，换取完整性标记的可信。

## 8. 三重时间

### 8.1 过去来源

- `specs/001-whcode-harness/spec.md`：FR-201—FR-208（事件、追踪、计量、完整性）、
  FR-616（传输进度）、NFR-213（默认回环与凭据纪律）、NFR-215（零 `unsafe` 与有界缓冲）；
- `docs/matrices/decisions.md`：D-05（追踪默认 `redacted`）、D-16（Provider 接入决策位）；
- `docs/adr/ADR-031-llm-provider-network-and-credentials.md`（网络与凭据决策，事后补录）；
- `docs/adr/ADR-032-frontend-parity-and-intended-differences.md`（四端登记协调与复审触发③）。

### 8.2 现在证据

- 代码：`crates/whcode-llm/src/http/{mod,profile,frames,protocols,errors}.rs`、
  `crates/whcode-llm/src/{provider,transport_events}.rs`、
  `crates/whcode-llm/Cargo.toml`、`crates/whcode-bin/Cargo.toml`；
- 单测：`crates/whcode-llm` 共 50 个 `#[test]`，其中 `http` 模块 21 个
  （`profile` 3、`frames` 4、`protocols` 10、`errors` 4），另 `http/tls.rs` 11 个属相邻变更 T188；
- 决策与账本：ADR-031、ADR-032、`CHANGELOG.md` 2026-10-25 条目、主账本 T162—T167；
- 结构性事实：`grep -rn "ureq::" crates` 无 HTTP 请求调用点；`http` feature 默认关闭。

### 8.3 将来失效条件与复审触发

1. 任一 crate 在 `http` feature 关闭路径出现网络依赖（`cargo tree` 可判）；
2. 发布体积增量超过阈值（须先给出接入调用点后的实测值再定阈值）；
3. `ureq`／`rustls`／`ring`／`webpki-roots` 安全公告或许可证变化；
4. 需要异步、HTTP/2、QUIC、mTLS 或企业代理；
5. 任一前端接线 `llm run`／`llm cancel`（ADR-032 复审触发③，须同 PR 翻转登记与矩阵）；
6. T162—T167 任一项状态变化（完成或重新排期），须同步本 spec 的交付状态表。

## 9. 假设、缺口与 0 信任说明

1. **门禁倒置**：代码（`3f9564c`）先于本 spec 入库，违反“G2 设计先行”。本 spec 是补登记，
   不构成对流程合规的追认；补救方式是补齐 G1／G2 产物并把未交付部分留在主账本。
2. **账本唯一**：任务完成状态与检查点仍以 `specs/001-whcode-harness/tasks.md` 与
   `checklist.md` 为准；本变更不建第二账本（`tasks.md` 只做分解与证据映射）。
3. **版本注记差异**：ADR-031 记 `webpki-roots 1.0.10`，`Cargo.lock` 为 `1.0.11`；
   本 spec 采信锁文件，ADR 修订由主代理决定。
4. **未重跑全量测试**：本次补登记只执行目录分类门禁与 EOL 门禁（见 `tasks.md` 验证节），
   不把历史全绿结论当作当前证据（ADR-037：测试通过以 CI 日志与 artifact 为准）。
5. **测试函数命名残留学说**：`http/errors.rs` 存在 `status_map_matches_fr212` 与
   “FR-212”注释，属幻影编号残留；本变更不修改 `crates/**`，作为发现项上报。
6. **`[待验证]` 事项**：企业代理与自定义 CA 的运维代价（ADR-040 已把自定义 CA 转为受控支持，
   但本变更未接线验收）；真实调用接入后的发布体积增量；`File` 凭据分支的专门测试。
