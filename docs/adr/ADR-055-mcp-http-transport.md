# ADR-055: MCP 客户端第二传输——Streamable HTTP 请求-响应（P1-7 批次二）

**状态**：已批准（2026-11-01，P1-7 批次二实施收口转正；实施与证据见 §6）
**前置**：ADR-054（MCP 客户端最小集 stdio v1，已批准）；ADR-013（平台验收口径）

## 1. 背景与决策

批次一（ADR-054）以 stdio 传输兑现最小可验证闭环，MCP 客户端可接本机子进程型服务器。
批次二引入**第二传输 Streamable HTTP**，覆盖网络型/远端 MCP 服务器（ZCode 域 I 三传输
中的 HTTP；本仓同步架构无 tokio 约束见 ADR-054 §1）。

**首手证据边界**（2026-11-01 复核）：本环境 ZCode 活动 `config.json` 的 MCP 服务器全部为
`"type":"stdio"`（duckduckgo/wikipedia/fetch/webresearch，均 command/args/timeoutMs），
**无任何 HTTP/OAuth 型服务器**——批次二是前瞻扩展，非已证实的真实使用需求。

本 ADR 决策范围：**Streamable HTTP 请求-响应形态**（单端点 POST + 会话头 + DELETE 结束），
**不含** SSE 接收与 OAuth（规范二者均标注可选；无首手使用证据，各自留待真实矛盾，见 §5）。
与 ADR-054 的 stdio 专有条款（进程树、stderr、env 卫生）区分：HTTP 无进程树/stderr/env，
对应条款见本 ADR。

## 2. 决策（每条含正面/负面/承担者）

1. **传输抽象**：`trait Transport`（对象安全）暴露三个原语——`write_message`（写一帧）、
   `read_message`（读一帧，`Ok(None)`=对端关闭）、`abort`（超时/取消/终止的强制动作）。
   现有 `McpSession` 的协议层（握手/list/call/配对/外来消息计数/poison）保持传输无关；
   stdio 特化逻辑（Child+管道+看门狗 terminate_tree+stderr 丢弃）迁入 `StdioTransport`。
   正面：接缝单一，HTTP 实现只写三个原语。负面：trait 增加动态分派（非帧路径，可接受）。
   承担者：whcode-mcp 维护者。
2. **HTTP 传输语义**：单端点 `url`，POST 发送 JSON-RPC 帧；`initialize` 响应头取
   `Mcp-Session-Id`（密码学安全 ASCII 0x21—0x7E）；后续请求**必须**回带该头；无会话头
   （除 initialize）→ 400；404 → 会话失效，报错由上层按“需重建会话”处理；DELETE+会话头
   显式结束会话；服务器不发会话头 = 无状态模式，也接受（各请求独立）。响应体取
   `application/json`；服务器要求 SSE（405 或 Content-Type text/event-stream）→ 显式报
   “该服务器要求 SSE 流，本批次不支持（见 ADR-055 §5 复审触发）”。
   正面：覆盖规范要求的请求-响应主体。负面：无 SSE 则服务器主动推送不可达。
   承担者：whcode-mcp 维护者。
3. **配置**：`McpServerConfig` 增 `url: Option<String>`；与 `command` **互斥**（url 存在=HTTP，
   否则 stdio），同时存在/同时缺失 → 解析错误。URL scheme：`https` 允许任意；`http` **仅限
   回环**（127.0.0.1/localhost，含 `::1`）——远程明文 HTTP 拒绝。服务器数上限 ≤8 同 ADR-054
   §2.3；超时/输出上限同 ADR-054 §2.3（缺省 30s/64 KiB，上限 600s/1 MiB）。
   正面：配置形态单义。负面：无。承担者：用户（配置 url）。
4. **生命周期**：每 turn 建/拆（同 ADR-054 §2.4，无连接池）；`Drop` 尽力发 DELETE 结束会话
   并释放连接（**不阻塞**；失败仅丢弃，无进程 reap——无进程树）。连接错误归因
   `RuntimeUnavailable`（握手/版本协商）与 `IoError`（传输中断）。
   正面：无常驻连接，符合“空闲态不随会话数线性增长”。负面：每 turn 建/拆网络往返。
   承担者：whcode-mcp 维护者。
5. **超时与取消**：同 30s 默认/600s 上限；`ureq` 请求超时 + 取消标志轮询；**无
   watchdog terminate_tree**（HTTP 无进程树对应物）；取消以 `abort` 断开连接。
   正面：超时路径明确。负面：无。承担者：whcode-mcp 维护者。
6. **鉴权**：本批次**不发** `Authorization` 头（无 OAuth）；URL 凭据（userinfo）拒绝；
   凭据不落日志/事件（ADR-054 §2.9 口径沿用）；服务器返回 401/WWW-Authenticate → 显式报
   “需鉴权，本批次不支持（见 §5）”。
   正面：最小权限、凭据卫生。负面：需鉴权服务器不可用（明确报错，不静默）。
   承担者：whcode-mcp 维护者。
7. **复用与上限**：工具名 `mcp__<server>__<tool>`、描述 ≤500 字符、inputSchema ≤4 KiB、
   每服务器 ≤64、清单总预算 ≤64 KiB——与 ADR-054 §2.7 完全一致（传输无关）。
   正面：复用已验证投影。负面：无。承担者：无新增。
8. **权限 fail-closed**：MCP 工具一律按 SessionCreate 判定（同 ADR-054 §2.8），HTTP 传输不
   放宽——网络面扩大，权限不降级。
   正面：外部不可信代码面不扩大权限。负面：无。承担者：whcode-app 维护者。
9. **投影与登记**：`mcp__` 命名空间合并、三步登记（prepared→running→终态）、`role:tool`
   回填、text 块拼接、isError 失败回填——与 stdio 走同一路径（传输无关，零改动）。
   正面：登记/回填复用。负面：无。承担者：无新增。
10. **依赖与安全底线**：仅新增 `ureq`（rustls feature，已在依赖树，whcode-llm 同口径）；
   无 tokio（保持同步架构）；零 `unsafe`（workspace forbid 不变）；URL 经 `ureq` 解析校验。
   正面：依赖增量最小。负面：无。承担者：whcode-mcp 维护者。

## 3. 正负和解

- **正面**：远端/网络 MCP 服务器可接入（跨机器工具生态）；请求-响应主体完整覆盖规范。
- **负面**：网络面扩大（HTTP 端点、会话头、状态码语义新增）；无 SSE → 服务器推送不可用；
  无 OAuth → 需鉴权服务器不可用；每 turn 建/拆网络会话（延迟）；远程明文 HTTP 禁止
  （仅回环 http）。
- **承担者**：whcode-mcp 维护者（传输与协议）、用户（配置 url 与安全边界认知）。
- **和解理由**：以最小请求-响应闭环覆盖真实第二传输形态；SSE 与 OAuth 都等真实使用证据
  再立项（规范标注可选，首手无使用证据）；安全边界收紧（远程 https、回环 http）优先于
  功能面。

## 4. 三重时间

- **过去来源**：MCP 规范 2025-06-18 transports 页（2026-11-01 一手复核，单端点 POST/GET、
  Mcp-Session-Id、无状态允许、DELETE、400/404/405/202 语义）；ZCode 域 I 首手源码证据
  （2026-10-28）；ADR-054（stdio v1 基线）。
- **现在证据**：本仓同步架构 + ureq 组件实测（whcode-llm 既有，ADR-040 口径）；
  whcode-mcp 接缝审计（2026-11-01：trait 三原语建议，协议层传输无关）。
- **将来失效条件**：SSE 需求出现（§5 触发）；鉴权需求出现（§5 触发）；每 turn 建/拆延迟
  成为痛点（§5 触发）；协议版本生态迁移（ADR-054 §5 触发）。

## 5. 复审触发（绑定命令 + 判定规则，ADR-046 口径）

- `cargo test -p whcode-mcp`：随批提交；fixture 覆盖 http initialize（含版本协商失败）/
  list/call（含 isError）/DELETE/超时/取消/404 会话失效。
- **SSE 接收**：真实服务器返回 `text/event-stream` 或 405 且降级不可行（命令：fixture 触发
  或用户配置的服务器实测；判定：`tools/call` 显式报“需 SSE”）→ 评估 SSE 接收（有界行读
  已有，扩展）。
- **鉴权**：真实服务器返回 401 + `WWW-Authenticate`（命令：配置的服务器实测；判定：显式
  报“需鉴权”）→ 评估 OAuth 2.1 授权码 + PKCE 流程（含 token 短期驻留与凭据卫生论证）。
- **连接池**：每 turn 建/拆的启动延迟成为可测量痛点（命令：turn 均值前后对比测量；判定：
  显著上升）→ 评估连接池（带 idle grace 与有界表，附并发档案，同 ADR-054 §5）。
- **协议版本**：MCP 规范发布新版本且生态迁移（命令：initialize 协商失败率；判定：上升）→
  升级 `PROTOCOL_VERSION` 并回归 fixture。

## 6. 实施摘要

- ADR-054 修订：§2.3—2.6 的 stdio 专有条款（env 卫生、进程树、stderr 丢弃、per-turn 重启）
  标注“stdio 特有”；HTTP 对应条款以本 ADR §2 为准。§5 复审触发并入本 ADR §5。
- 实现：`session.rs` 抽 `trait Transport` + `StdioTransport`（现逻辑迁入，行为不变）+
  `HttpTransport`（新）；`config.rs` 加 `url` 字段与互斥/scheme 校验；`manager.rs` 连接分派
  按传输类型；`lib.rs` 导出 `Transport`/`HttpTransport`。
- 登记：FR-738 / T241—T243 / Check 337—339（计数以并行会话 T240/Check 336 落地后的实际
  基数为准，登记时按 ADR-046 口径同步 spec/tasks/checklist/software-engineering-process）。
- fixture：`http-test-server`（单端点 POST + 会话头 + DELETE，无 SSE；模式：echo/hang/
  nosession/auth，每连接一线程）。
- 实现细节（ureq 3.5 API 实测）：`RequestBuilder` 无 per-request 超时——每请求独立
  `Agent`（`timeout_global` 覆盖连接+写+读整个操作预算，MCP HTTP 低频、代价可忽略）；
  `http_status_as_error(false)` 关闭 4xx/5xx 短路，由状态码分支归因（404 会话失效/
  401 鉴权/405 SSE/其余显式报状态）；响应头经 `http` crate `.headers()` 读取。
- 实施证据（2026-11-01，P1-7 批次二）：HTTP 集成 6/6（握手往返+会话头回带/超时归因/
  预置取消/404/401/shutdown 失效）+ stdio 集成 6/6（行为保持，重构无回归）；
  `clippy -p whcode-mcp --all-targets -D warnings` 干净；`cargo build/test --locked
  --workspace`、`fmt --check`、layering、layout 全绿（workspace 级 clippy 本地被杀软
  拦截未取得——`os error 5` 复制 build-script-build.exe，whcode-store/whcode-bin，
  已登记，按 ADR-037 以 CI 为准）。
- 登记：FR-738 / T241—T243 / Check 337—339（计数 116/243/339，ADR-046 口径同步
  spec/tasks/checklist/software-engineering-process）；追溯与计数门禁全绿
  （traceability：FR=116/任务=243/检查点=339 无断链；check_counts：counts consistent）。
- 验收：layering `ALL_CRATES` 不变（无新 crate）；CI 六车道 + freebsd 平台证据（ADR-013
  口径）待推送流水线（ADR-037）。

## 7. 修订记录

- **2026-11-01（草案）**：批次二设计定稿（scope＝HTTP 请求-响应，无 SSE/OAuth）；编号 055
  （054 为 stdio v1 批次一）。
- **2026-11-01（转正，P1-7 批次二）**：实施收口——trait Transport + StdioTransport
  重构（提交 f82aab5，行为保持）+ HttpTransport + config url XOR command + connect
  分派落地；fixture + HTTP 集成 6/6 全绿；§6 补实施证据与 ureq 3.5 API 实测细节。
