## 1. 使用的方案

仓库未引入第三方日志框架（`log`、`tracing`、`slog` 等均未在依赖中出现）。`whcode-bin` crate 自实现了一个**最小日志器**，直接通过 `eprintln!` 将结构化文本行输出到 `stderr`。所有日志消息在进入输出前统一经过 `redact()` 进行敏感信息脱敏。

## 2. 关键文件

- `crates/whcode-bin/src/logging.rs`：日志级别、脱敏逻辑、格式化器与 `Logger` 结构体的全部实现。
- `crates/whcode-bin/src/main.rs`：唯一消费 `logging::Logger` 的入口，以 `Info` 门限启动 `doctor` 子命令。
- `crates/whcode-domain/src/trace.rs`：定义跨 crate 的 `TraceLevel`（`Off / Redacted / Metadata / Full`），其默认值为 `Redacted`，并声明“疑似秘密字段脱敏后写入”的约定。
- `crates/whcode-domain/src/event.rs`：事件持久化策略引用 `PersistenceLevel::Redacted`，与 trace 脱敏语义对齐。

## 3. 架构与约定

### 3.1 日志级别

模块内自定义四级枚举 `Level`（`Error < Warn < Info < Debug`），对应标签字符串 `ERROR / WARN / INFO / DEBUG`。比较关系用于门限过滤：只有 `level >= threshold` 的消息才会被格式化输出。`format_log(threshold, level, message)` 返回 `Option<String>`，低于门限时为 `None`。

### 3.2 输出格式

每行日志形如 `[LEVEL] message`，其中 `message` 先经 `redact()`。`Logger::new(threshold)` 创建带门限的实例；`logger.log(level, message)` 内部调用 `format_log` 并在条件满足时 `eprintln!`。

### 3.3 脱敏管线（核心约束）

`logging.rs` 头注释明确规则：**API Key、访问令牌与运行时凭据不得以明文写入日志、数据库或测试快照**；默认追踪级别对疑似秘密字段脱敏。脱敏分两遍执行：

1. **自由文本令牌检测**：匹配前缀 `Bearer `、`sk-`、`ghp_`，其后连续非分隔符字符替换为常量 `REDACTED = "[REDACTED]"`。
2. **键值形态检测**：命中敏感键名集合 `api_key / apikey / token / secret / password / authorization` 时，将其后的值替换为 `[REDACTED]`。支持 `key=value` 和 `"key":"value"` 两种语法，值终止于空白、`,`、`}`、`;`、`&`、`"`、`'`。

该函数幂等：已包含 `[REDACTED]` 的输入再次处理不变（由单元测试验证）。

### 3.4 与配置/领域模型的对齐

- `whcode-domain::TraceLevel` 的默认变体是 `Redacted`，其 `writes_content()` 返回 `true`，`requires_sensitive_warning()` 仅对 `Full` 返回 `true`。
- `main.rs` 中 `doctor` 启动 logger 使用 `Level::Info`，调试细节走 `--verbose` 触发 `Level::Debug`，构成实际的运行期门限切换路径。
- 任务书与 ADR 文档（tasks.md T017、FR-203、§10.1、D-05）作为需求来源被引用在模块 doc comment 中。

## 4. 约定与约束

- **无第三方日志依赖**：整个工作区未引入 `log`/`tracing`/`slog`，所有日志均通过 `whcode_bin::logging::Logger` 输出到 stderr。
- **强制脱敏**：`redact()` 是日志路径上的强制中间件——任何进入 `Logger::log` 的消息都会先过脱敏；`REDACTED` 常量是唯一合法占位符。
- **固定敏感键集合与前缀集合**：`SENSITIVE_KEYS`（6 个键名）与 `TOKEN_PREFIXES`（3 个前缀）为硬编码白名单，新增密钥类型需修改这两处常量而非调用方。
- **级别顺序即门限语义**：`PartialOrd` 按 `Error < Warn < Info < Debug` 实现，`format_log` 据此决定是否输出；新增级别必须同时更新 `level_label` 与排序。
- **幂等性保证**：`redact` 被要求幂等，已有单测断言重复调用结果不变，防止双重脱敏破坏日志可读性。
- **默认追踪级别为脱敏模式**：`TraceLevel::default() == Redacted`（见 domain trace 模块测试），意味着在未显式配置的情况下，内容型追踪不会泄漏敏感字段。
- **骨架阶段限制**：当前 `whcode-bin` 仅暴露 `--version`、`doctor [--config PATH] [--json]` 三个入口，日志系统尚未接入 CLI 参数解析（如 `--verbose` 仅在 `main.rs` 中硬编码判断）。
- **未覆盖 sink 扩展点**：目前仅 `stderr`，没有多 sink、JSON 输出、文件轮转或异步写入能力；若未来扩展，应复用现有 `redact` 与 `Level` 抽象。