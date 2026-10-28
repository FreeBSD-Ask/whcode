# 域 I：MCP

> ZCode 证据路径以 `local/zcode-src`（commit 3c5b227）为根，以下省略前缀，实际绝对路径为
> `D:\WHCode\local\zcode-src\...`。

## 1. 对照表

| 项 | ZCode | WHCode | 判定 |
|---|---|---|---|
| 客户端 | `packages/adapters/src/mcp/index.ts:1909-1970` `McpAdapter.createTransport()`：stdio→`ProcessTreeStdioClientTransport`（`:1917-1943`）；http（StreamableHTTP）→`StreamableHTTPClientTransport`（`:1953-1959`）；sse→`SSEClientTransport`（`:1964-1968`）。**OAuth**：`createOAuthClientProvider`（`:2029-2058`）；`authorization_code` 用自研 `createMcpOAuthTokenProvider`（`oauth-provider.ts:34`，强制 401 汇入跨进程单飞 refresh 锁），`client_credentials` 用 SDK `ClientCredentialsProvider`；`resolveAuthorizationCodeOAuthConfig`（`:2395-2413`）无 oauth 字段且无 Authorization 头时**自动推导** authorization_code。**自研 stdio-transport**：`stdio-transport.ts:47` `ProcessTreeStdioClientTransport extends StdioClientTransport`；`start()` Windows 接管 Job Object（`:111-118`）；`_dispose`（`:157-176`）先杀进程树再交还 SDK；进程树管理 `process-tree.ts:33-44`（Windows `taskkill /PID x /T /F` `:173-194` 2s 超时；POSIX SIGINT→SIGTERM→SIGKILL 梯度 `:196-228`）；**Windows Job Object** `windows-job-object.ts:27-66` 经 koffi 动态加载 `kernel32.dll`，设 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`（`:3-4,144`），原生不可用时保留 taskkill 回退 | 无 | 空白 |
| 连接池 | `pool.ts:18` `DEFAULT_IDLE_GRACE_MS=30_000`；`scheduleClose`（`:94-105`）仅在 `refs.size===0` 时 `unref()` 定时关闭；**隔离** `connectionKey`（`:548-560`）默认 session 隔离（scope=leaseId），仅 `config.isolation==="workspace"` 时用 `workspaceIdentity ?? workingDirectory` 允许跨 session 复用；`revalidateEntry`（`:113-162`）死连接原地重连 | 无 | 空白 |
| 工具投影 | `core/src/mcp/index.ts:76-104` `registerMcpTools`：遍历 `McpToolDescriptor[]`→`registry.register(createMcpToolEntry(...))`，支持 allow/deny（新旧名任一命中即拒/允）。**超时** `MCP_TOOL_TIMEOUT_MS=30_000`（`:48`），工具级 `_meta.timeoutMs` 覆盖服务器级（`descriptor.ts:33`，上限 30 分钟 `MCP_TOOL_META_TIMEOUT_MAX_MS`）；**图片归一化** `core/src/mcp/image-normalization.ts`（inline base64 上限 200 KiB `:21`，超出落 artifact 或压缩）；**CUA 凭据 fail-closed**（`:64-73,87-88` `officialCuaServerNames` 缺省不投影官方 CUA 规范名；adapter 侧 `:127,1975` `trustedOrigins` 缺失即抛 `official_auth_unavailable`；官方鉴权与 OAuth **互斥** `:2034-2036,2399-2403`） | 无 | 空白 |

## 2. 与 WHCode 差异修正（相对原差异书）

- 原判定“”**仍成立**：`D:\WHCode` 全仓 `.rs/.toml/.ts` 无任何 MCP 引用；
  `docs/adr/ADR-044...md:37,56` 仅把“”列为**未实施的 P2-2 规划**。
- ZCode 则完整实现 MCP（SDK client + 三 transport + OAuth + 进程树/Job Object + 连接池 +
  工具投影 + 图片归一化），为显著差异项。

## 3. 存疑项

- 官方 MCP 鉴权仅对 `http`/`stdio` 的 `zcode_official`/`jwt_token` 配置生效
  （`index.ts:2423-2430`），`sse` 不覆盖 `[待验证]` 是否属设计缺口。
- `MCP_TOOL_TIMEOUT_MS=30_000` 是**默认值**，工具级 `_meta.timeoutMs` 可覆盖；原差异书
  “”仅指默认路径 `[待验证]` 精确上限。
