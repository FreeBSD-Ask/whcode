# SDK JSON-RPC 契约（v1.0；T293／FR-778／ADR-071）

本契约是 D14 版本承诺的承诺载体：**v1.0 方法面与语义一经发布即承诺**；minor 只增不改，
破坏性变更升主版本。决策依据与并发档案见 `docs/adr/ADR-071-sdk-jsonrpc-out-of-process-api.md`。

## 1. 传输与帧

- 载体：`whcode sdk` 子命令，stdin/stdout 行分帧 JSON-RPC 2.0，UTF-8，单连接；
  每行一帧，`\n` 结尾，单行上限 64 KiB（超限该行丢弃并回错误帧后继续）。
- 请求：`{"jsonrpc":"2.0","id":<u64>,"method":"<name>","params":{...}}`；
  通知（服务端→客户端）：`{"jsonrpc":"2.0","method":"<name>","params":{...}}`（无 id）。
- 服务端主循环绑定**一个工作区**（启动参数）；连接生命周期 = 进程生命周期（stdin EOF 优雅关闭）。

## 2. 版本协商

- `initialize` 请求：`{"protocolVersion":"1.0","client":"<名称>","clientVersion":"<版本>"}`
  （仅 `protocolVersion` 必填）。
- 响应：`{"protocolVersion":"1.0","server":"whcode","serverVersion":"<crate 版本>"}`。
- 兼容规则：主版本不同 → 错误响应（`code = invalid_state`，`data.message` 携带服务端
  支持版本），连接继续但仅 `initialize` 可用；主版本相同 → 后续方法可用（minor 由
  服务端声明其支持级别，客户端不得假设未登记的能力）。
- 未 `initialize` 先调用其他方法 → `invalid_state` 错误。

## 3. 方法

### 3.1 `session.create`

- 参数：`{"title": "<可选，≤256 字符>"}`。
- 结果：`{"session": "<会话 ID>", "generation": <u64>, "max_seq": <u64>}`。
- 语义：在绑定工作区创建新会话并作为后续 `turn.run`／`events.follow` 的焦点会话；
  单连接同时只有一个焦点会话，重复 create/open 切换焦点（焦点切换后必须重新
  `events.follow`）。

### 3.2 `session.open`

- 参数：`{"session": "<会话 ID>"}`。
- 结果：同 `session.create`（`generation`＝该会话 `SessionRewound` 累计条数，
  `max_seq`＝当前最大事件 seq）。
- 错误：会话不存在 → `not_found`。

### 3.3 `turn.run`

- 参数：`{"prompt": "<必填，≤256 KiB>", "maxTokens": <可选 u32>}`。
- 结果：`{"requestId": "<turn 请求 ID>", "status": "<completed|failed|cancelled>",
  "text": "<最终文本，有界>", "error": {CUI 错误信封, 可选}}`。
- 语义：**阻塞至 turn 终态**；执行期间该连接的活跃槽被占用（第二次 `turn.run` 立即
  `busy` 错误）；取消经 `turn.cancel`。审批口径与 CUI `llm run` 一致（无交互审批闸门，
  无人值守决策面属 P6.3）。
- 错误：槽满 → `busy`；会话焦点缺失 → `invalid_state`。

### 3.4 `turn.cancel`

- 参数：`{"requestId": "<turn.run 响应前无法预知时，取消当前唯一活跃 turn 可省略>"}`。
- 结果：`{"cancelled": <bool>}`（`false`＝无匹配活跃 turn，非错误）。
- 语义：置取消标志；`turn.run` 的响应以 `cancelled` 终态返回（取消与完成可区分）。

### 3.5 `events.follow`

- 参数：`{"fromSeq": <u64, 0=从头>, "session": "<可选，缺省为焦点会话>"}`。
- 结果（立即）：`{"generation": <u64>, "nextSeq": <u64>, "maxSeq": <u64>}`——登记后由
  **通知**推送增量。
- 通知：`event` 帧参数 `{"session": "<ID>", "seq": <u64>, "generation": <u64>,
  "kind": "<事件类型名>", "payload": {…}}`——单会话内 `seq` 严格递增、无重复无跳过。
- 世代：`generation`＝截至该事件的 `SessionRewound` 累计条数；世代变化即派生口径变化，
  客户端应重算派生视图（原始事件事实流不受回滚影响）。
- 语义：单连接单消费者（重复 follow → `invalid_state`）；轮询间隔 100 ms（实现细节）；
  事件按追踪级别脱敏后出站（`redacted` 默认，与四端同口径）。
- 错误：会话不存在 → `not_found`。

## 4. 通知

| 方法 | 载荷 | 触发 |
|---|---|---|
| `event` | 上节 | `events.follow` 登记后的增量事件 |
| `generation` | `{"session": "<ID>", "generation": <新值>}` | 焦点会话发生 `SessionRewound`（先行提示，随后的 `event` 帧携带新世代） |
| `closed` | `{"reason": "<slow_consumer|shutdown>"}` | 连接关闭前的最后一帧 |

## 5. 错误信封

JSON-RPC `error` 对象：`{"code": <-32602|-32603|...>, "message": "<概述>",
"data": {"schemaVersion": "1.0", "code": "<CUI 稳定码名>", "message": "<可操作说明>",
"retryable": <bool>, "details": <对象|null>}}`——`data` 四字段与 CUI JSON 信封同源
（`contracts/cui-cli.md` §3），稳定码表沿用其 §2。JSON-RPC `code` 只区分
`invalid_params`（-32602）／服务端错误（-32603）／忙（-32000），语义判定以 `data.code` 为准。

## 6. 有界与脱敏

- 单行 64 KiB、`prompt` 256 KiB、`title` 256 字符、出站通道 256 帧（满触发 `closed`）。
- 事件载荷出站经既有脱敏管线（追踪级别 `redacted` 默认）；参数与凭据不入帧
  （与四端同口径，NFR-215/218）。

## 7. 兼容与演进

- minor 演进：只允许**新增**方法／通知／可选参数；已登记语义不得变更。
- 破坏性变更（改语义／删方法／改帧格式）→ 主版本升级 + 本契约新增版本节。
- 本契约版本：**1.0**（2026-11-14，D14 承诺）。
