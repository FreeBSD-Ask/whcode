# ADR-054：MCP 客户端最小集（P1-7 v1——stdio transport、投影与 fail-closed 权限面）

- **状态**：已批准（2026-11-01，建议书 proposal.md D3 排序 + 用户"继续"指令；transport
  范围经意见征询未获表态，按建议项 stdio 先行执行，HTTP/OAuth 留后续批次复审）
- **来源**：`research/zcode-comparison/domain-I-mcp.md`（ZCode MCP 客户端一手证据：三
  transport、OAuth、进程树/Job Object、连接池、工具投影）+ MCP 官方规范 2025-06-18
  transports 页（一手复核，2026-11-01）+ proposal.md P1-7 行（"最重项最后；超时/并发/
  缓冲全部有界；CUA fail-closed 原则保留"）
- **责任人**：whcode-mcp 维护者
- **退出条件**：连续两个批次零 MCP 工具调用（配置存在但模型从未调用 `mcp__*`）或维护
  成本显著超出价值，须另立 ADR 裁剪

## 1. 事实与证据

1. **现行规范只有两个标准 transport**（一手证据，2026-11-01 复核
   `modelcontextprotocol.io/specification/2025-06-18/basic/transports`）：stdio 与
   Streamable HTTP；旧 HTTP+SSE（SSE 独立 transport）**已废弃**，仅保留向后兼容路径；
   "Clients SHOULD support stdio whenever possible"。ZCode 的"三 transport"含废弃 SSE，
   是兼容面而非规范要求。
2. **官方 Rust SDK（rmcp）强依赖 tokio 异步运行时**：本仓全同步架构（`cargo tree` 无
   tokio；HTTP 栈为 ureq 3.5 + rustls 可选 feature），引入 tokio 违反资源优先序
   （1.1 §1）与 NFR-216 线程纪律。
3. stdio 分帧是**newline 分隔的 UTF-8 JSON-RPC**（单行、无内嵌换行；stdout 不得写
   非 MCP 消息；stderr 仅日志）——serde_json 已在树，自研客户端的最小实现面很小。
4. **进程树终止与环境净化已有平台级实现**：`whcode-platform::process::terminate_tree`
   （Windows `taskkill /T /F`，ADR-019 同口径零依赖决策；Unix 进程组 TERM→KILL 梯度）、
   `configure_child_process_group`；`whcode-runtime::worker::apply_minimal_env`
   （最小环境白名单，Check 123 口径）。
5. Windows Job Object（ZCode 主路径，koffi 动态加载 kernel32）需要 FFI `unsafe`——
   本仓普通 Rust 零 unsafe（NFR-215），例外仅按 ADR-015 执行。

## 2. 决策

1. **范围（v1）**：stdio transport 单一；Streamable HTTP、OAuth、SSE 兼容面均不在
   v1——本地优先哲学下 stdio 是本地 MCP 服务器绝对主流，官方 SHOULD 口径同向；
   SSE 已废弃不跟进。HTTP/OAuth 作为 P1-7 后续批次另立验收与 ADR 修订。
2. **新 crate `whcode-mcp`**（第 15 个成员，领域服务层）：依赖 `whcode-domain`（错误）、
   `whcode-platform`（进程树/进程组）、`whcode-runtime`（worker 模块的最小环境注入；
   **服务层内横向依赖，特此声明**——凭据卫生逻辑不得复制，防漂移）、`serde_json`。
   自研最小 JSON-RPC 2.0 客户端（决策 1 证据 2：rmcp/tokio 与同步架构冲突）。
3. **配置**：`<workspace>/.zcode/mcp.json`，显式 opt-in、无自动发现；`enabled` 开关 +
   服务器数上限 8。无配置文件 = 功能关闭（正确的空）；配置存在但解析失败 = 显式报错
   （错误的空，不静默降级）。
4. **会话生命周期 per-turn**：run_turn 组装清单时对每个 enabled 服务器
   spawn→initialize（协议版本 2025-06-18，服务器回显未知版本即中止）→tools/list
   （跟随 cursor，有界）；turn 结束（含早退路径）经 `Drop` 关闭 stdin→宽限等待→
   `terminate_tree`→reap。**不做连接池/空闲保活**（ZCode 30 s idle grace 的 v2 项）。
5. **请求语义**：仅客户端发起（tools/list、tools/call、ping）；请求超时默认 30 s
   （ZCode `MCP_TOOL_TIMEOUT_MS` 同口径，上限 600 s）、看门狗线程到点
   `terminate_tree` 解锁阻塞读（瞬态线程，每在途请求一条，受并发门约束）；
   turn 取消令牌在 watchdog 轮询（50 ms）中同权生效；未匹配 id 的入站消息计数丢弃，
   服务器请求回 `-32601`（未声明能力，规范上不应出现）。
6. **stderr 丢弃**（`Stdio::null()`）：防 stderr 管道填满导致服务器写日志阻塞
   （死锁面）；诊断靠退出码与协议错误。代价：服务器日志不可见——复审触发见 §5。
7. **投影与命名空间**：工具名 `mcp__<server>__<tool>`（服务器名 `[a-z0-9_-]{1,32}`、
   工具名 `[A-Za-z0-9_-]{1,64}`，越界跳过并计数）；描述截 500 字符、单工具
   inputSchema ≤ 4 KiB、每服务器工具 ≤ 64、MCP 清单总预算 ≤ 64 KiB（超限按序丢尾并
   截断标记）。`whcode-tools::ToolDescriptor.name: &'static str` 为静态名契约，MCP
   动态名**不进**内置注册表——清单在 turn 组装层合并，分派在 `execute_tool_call`
   加 `mcp__` 前缀分支。
8. **权限面 fail-closed**：MCP 工具一律按 `WorkspaceWrite` 判定（SessionCreate 能力，
   四端写确认覆盖）——外部服务器代码不可信，与 ZCode CUA fail-closed 同向；细粒度
   allow/deny 白名单留待真实矛盾。参数不做本地白名单校验（内置工具 FR-729 口径不
   适用），只验 JSON 对象形态——inputSchema 契约由服务器侧执行。
9. **凭据卫生**：spawn 走 `apply_minimal_env`（父环境最小白名单，凭据永不透传）+
   显式 `env` 键值回填（≤ 16 键，键名字符集校验）。**显式偏离**：不再过
   `strip_credentials`——用户在本地配置文件里为服务器声明的 env（如其 API token）
   是该服务器的合法输入，属威胁模型内的授权通道而非意外透传；值不入日志与事件。
10. **登记与回填**：`mcp__` 调用走与内置工具同一条三步登记（prepared→running→终态）
    与 `role:tool` 回填；结果取 content[] 的 text 块拼接，非文本块计数注记，输出截断
    沿 64 KiB 口径；`isError` 的调用按失败回填（不伪造成功）。

## 3. 正负和解

- **正面**：代理工具循环获得可扩展的外部工具生态入口（ZCode 最大差距项补齐）；
  stdio 单 transport 使实现面、依赖面、攻击面同时最小；全部机制复用已验证的平台/
  runtime 组件。
- **负面**：每 turn 重启服务器进程（启动延迟 100 ms—1 s 进每次 turn）；连接失败仅以
  system 消息告示（无独立事件类型，事件 schema 不变）；stderr 丢失降低排障能力；
  权限粒度粗（全部按写权限确认）。
- **承担者**：whcode-mcp 维护者（会话与协议）、whcode-app 维护者（接线）、用户
  （配置文件与写确认成本）。
- **和解理由**：v1 以最小可验证闭环兑现代理×外部工具的真实矛盾；池化、事件化、
  细粒度权限都等真实使用证据再立项。

## 4. 三重时间

- **过去来源**：ZCode 域 I 一手源码证据（2026-10-28 入库）；proposal.md P1-7 排序
  （2026-10-29 批准）。
- **现在证据**：MCP 规范 2025-06-18 transports 页（2026-11-01 一手复核）；本仓
  同步架构与平台/runtime 组件实测（§1 证据 2—4）。
- **将来失效条件**：见头部退出条件与 §5。

## 5. 复审触发

- `cargo test -p whcode-mcp` 随批提交；fixture 覆盖 initialize/list/call/超时/
  进程终止零残留（`platform::process_exists` 断言）。
- **stderr 丢弃**：出现需要服务器日志排障的真实事故 → 评估"有界 drain 线程"
  （随连接存亡，非会话常驻）。
- **per-turn 重启**：真实会话中启动延迟成为可测量痛点（turn 均值显著上升）→ 评估
  连接池（带 30 s idle grace 与有界表），须附并发档案。
- **Job Object**：长跑或实测出现孤儿进程/PID 复用事故 → 按 ADR-015 例外流程评估
  Job Object（独立 unsafe 封装模块 + 差分测试）。
- **协议版本**：MCP 规范发布新版本且生态迁移（如 initialize 协商失败率上升）→
  升级 `protocolVersion` 常量并回归 fixture。

## 6. 实施摘要

- FR-736/FR-737、T236—T239、Check 332—335 登记（计数 FR 113→115、任务 235→239、
  Check 331→335）；实现落 `crates/whcode-mcp/`，接线落 `whcode-app/src/turn.rs`；
  分层测试 `ALL_CRATES` 14→15。

## 7. 修订记录

- **2026-11-01（首版，已批准）**：编号 054（053 已被技能最小集占用）。
