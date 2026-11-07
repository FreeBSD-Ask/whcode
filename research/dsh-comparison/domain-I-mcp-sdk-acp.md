# 域 I：外部互操作：MCP、SDK、ACP、API 网关

> DSH 证据根：`local/dsh-src`（commit `fcc0ad4`）；WHCode 证据根：本仓库。

## 1. 对照表

| 项 | DeepSeek Harness (DSH) | WHCode | 判定 |
|---|---|---|---|
| MCP 协议 | 官方 SDK 协商“现代或受支持的旧版”协议修订；无强制修订的配置项（`docs/subsystems/mcp.md`） | 自研 JSON-RPC 2.0 编解码（`crates/whcode-mcp/src/protocol.rs`） | **DSH 领先**：跟官方修订 |
| MCP 传输 | 本地进程或 HTTP 端点皆可配置；传输由 SDK 负责（`mcp.md`） | stdio（ADR-054）＋ Streamable HTTP（ADR-055）；`command` 与 `url` 互斥（`crates/whcode-mcp/src/config.rs:42`—`46`） | **同构** |
| 工具接入 | 每个服务器贡献**普通 harness 工具**：取消、权限检查、结果记录、图像输出全走主管道（`mcp.md`） | 工具命名 `mcp__<server>__<tool>`，合并进 turn 清单；权限 fail-closed 且不区分读写（`crates/whcode-app/src/turn.rs:1249`） | **同构**；WHCode 权限更保守 |
| 资源/指令 | 共享 resource 工具按调用方 scope 选 provider；服务器 `instructions` 进系统提示；`maxInstructionBytes` 限长（`mcp.md`） | 无 resource 概念；无 instruction 注入 | **DSH 领先** |
| 上限 | 服务器数、超时、重连、启动失败策略由客户端配置（`mcp.md`） | 服务器 ≤ 8、env 键 ≤ 16、单行 ≤ 64 KiB、每服务器工具数上限、清单总预算、默认超时 30 s（上限 600 s）、输出默认 64 KiB（上限 1 MiB）（`config.rs:19`—`34`；`lib.rs:12`—`14`） | **WHCode 领先**：全部是编译期常量且越界拒绝 |
| 进程管理 | 由 SDK/客户端负责；管理器向 Remote 客户端暴露状态/重启/探测（`mcp.md`） | 显式 env 回填＋最小环境＋进程组回收（ADR-054） | **同构** |
| SDK | 出进程 JSON-RPC：TypeScript 客户端/服务端＋Python 运行时 wheel（`docs/architecture.md` 应用启动节） | 无（检索 `json_rpc_server`、`sdk` 于 `crates/` 无实现） | **结构性缺失（WHCode）** |
| ACP | automation-only Agent Client Protocol 服务端，供自动化桥接；其 agent 用一次性机器决策审批（`docs/subsystems/approval.md`） | 无 | **DSH 独有** |
| API 网关 | Typert API Gateway：`@Remote` 标注 → 生成 Host/Client 契约 → 复用 Connection RPC 与 `/api` 路由（`docs/api-gateway.md`） | 自研单线程 HTTP＋WS，路由手写；无生成式契约 | **DSH 领先**：契约由类型生成 |

## 2. DSH 实现解剖

### 2.1 MCP 工具与内建工具同权

DSH 不另造 MCP 通道：`mcp-client` 把发现的工具适配进 `ctx.tools`，于是取消、审批、结果落库、
图像输出全部复用主管道（`mcp.md`）。这避免了“MCP 工具是二等公民”的常见缺陷。

### 2.2 scope 决定可见性

服务器在配置的 Cordis scope 内可见：没有可见配置的调用方拿不到 MCP 工具与提示文本
（`mcp.md`）。resource 工具也按调用方 scope 选 provider，首个 provider 启用共享工具、
最后一个移除时撤销（`mcp.md`）。可见性由组合决定，不由全局开关决定。

### 2.3 传输与协议修订交给 SDK

客户端不实现协议细节，只声明服务器配置；协议修订协商由官方 SDK 完成（`mcp.md`）。
代价是受 SDK 支持的修订集约束，收益是不用自己追规范。

### 2.4 API 网关是类型驱动的 RPC

业务服务用 `@Remote`/`@RemoteScope` 标注暴露方法，构建期生成 Host/Client 契约；复杂宿主对象
经 `TypertLookupMap` 映射为线上 id（如 `agentId`），网关在调用前解析（`api-gateway.md`）。
未标注的方法既不进 Client 类型也不可被调用——**暴露面是类型系统的一部分**。

### 2.5 ACP 把审批降级为机器决策

ACP 是 automation-only 服务端；它为“自己的 agent”提供一次性机器决策的审批 answerer
（`approval.md`）。这意味着自动化场景不是“绕过审批”，而是换一个可审计的决策者。

## 3. WHCode 现状核对

- 已实现：MCP stdio 与 Streamable HTTP 双传输；JSON-RPC 2.0；`mcp__<server>__<tool>` 命名；
  配置越界拒绝（服务器 8、env 16、名称字符集、超时区间）；显式 env 回填与最小环境；
  进程组回收；权限 fail-closed（`crates/whcode-mcp/src/config.rs:19`—`34`、`lib.rs:3`—`14`、
  `crates/whcode-app/src/turn.rs:1249`）。
- 已实现：MCP 工具结果按主管道截断（默认 64 KiB，上限 1 MiB，`config.rs:31`—`34`）。
- 缺失：MCP resource、服务器 instructions 注入、状态/重启/探测管理面、OAuth。
- 缺失：SDK（出进程 API）、ACP、生成式 RPC 契约。WHCode 的对外面是四端命令＋CUI JSON 信封，
  不是可编程 SDK。
- 差异：WHCode 对 MCP 工具统一按“写权限”处理（不区分读写），DSH 走统一审批管道按调用
  逐个判。前者更保守，后者更细。

## 4. 差异判定与可借鉴点

1. **MCP 工具同权**：保持。WHCode 已把 MCP 工具纳入 turn 清单与主管道，这正是 DSH 的做法；
   后续只需把 T255 审批 broker 接上，MCP 就自动获得逐次审批。
2. **scope 可见性**：改写采纳。WHCode 无 scope 概念，但可做“按会话启用服务器子集”
   （`mcp.servers` 在会话元数据里裁剪）；比全局开关更接近 DSH 语义。
3. **resource 与 instructions**：暂不采纳。两者都会扩张提示词面与权限面；若采纳，必须先定
   指令长度上限（DSH 有 `maxInstructionBytes`）与资源读取的权限模型。
4. **SDK（出进程 API）**：建议评估但不立即做。WHCode 的 CUI 已有稳定 JSON 信封，可作为
   “最小 SDK”；真正的 JSON-RPC SDK 需要协议版本策略与长期兼容承诺，属于不可逆决策。
5. **类型驱动 RPC 契约**：不采纳。需要 TypeScript 装饰器与代码生成链，与 Rust 单仓静态分层
   不匹配；WHCode 的手写路由＋契约测试更符合“能多简陋就多简陋”。
6. **ACP 的机器审批**：可借鉴其思想——自动化场景用“显式决策者”而非“默认允许”。WHCode
   将来做无头模式时应照此设计（`unavailable` 显式化，见域 C）。

## 5. 待验证项

1. DSH MCP 的启动失败策略（fail-closed 还是跳过该服务器）：`mcp.md` 提到由客户端配置，
   未给默认值。取证：读 `packages/mcp/mcp-client/README.md#use-this-package`。
2. DSH 是否支持 MCP OAuth：`mcp.md` 未提；ZCode 有 OAuth，DSH 可能交给 SDK。取证：
   检索 `oauth` 于 `packages/mcp/**`。
3. WHCode MCP HTTP 传输是否强制 TLS：`config.rs:44` 允许 `http://127.0.0.1|localhost` 回环，
   非回环是否要求 https 未读实现。取证：读 `valid_url` 全文与 ADR-055 §2.3。

## 6. 证据路径清单

- `docs/subsystems/mcp.md`（MCP 工具同权）
- `docs/subsystems/mcp.md`（scope 可见性）
- `docs/subsystems/mcp.md`（协议协商）
- `docs/api-gateway.md`（@Remote 契约生成）
- `docs/subsystems/approval.md`（ACP 机器审批）
- `crates/whcode-mcp/src/config.rs:19`（服务器上限 8）
- `crates/whcode-mcp/src/config.rs:25`（默认超时 30 s）
- `crates/whcode-mcp/src/config.rs:42`（command/url 互斥）
- `crates/whcode-mcp/src/lib.rs:12`（有界性总述）
- `crates/whcode-app/src/turn.rs:1249`（MCP 权限 fail-closed）
