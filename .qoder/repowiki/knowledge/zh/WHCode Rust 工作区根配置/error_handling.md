## 1. 采用的体系

仓库使用 **Rust**，错误处理围绕 `crates/whcode-domain/src/error.rs` 中定义的结构化错误类型展开：
- 稳定错误码枚举 `ErrorCode`（`thiserror` + `serde` 派生）。
- 业务错误结构体 `WhError`（字段：`code`、`message`、`retryable`、`details: Option<serde_json::Value>`），实现 `Display` 和 `thiserror::Error`。
- 成功/失败信封 `OkEnvelope<T>` 与 `ErrEnvelope`，统一携带 `schema_version`，由四端（TUI/CUI/Web/SSH）共用。

上层 crate（如 `whcode-bin`）通过 `Result<_, WhError>` 传播错误；尚未出现 `panic!`、`expect()`、`unwrap()` 等中止式 API 的使用（在已扫描的 `whcode-bin` 与 `whcode-domain` 范围内）。日志系统位于 `whcode-bin/src/logging.rs`，仅记录不返回错误。

## 2. 关键文件

| 文件 | 作用 |
|---|---|
| `crates/whcode-domain/src/error.rs` | `ErrorCode`、`WhError`、`OkEnvelope`、`ErrEnvelope`、`SCHEMA_VERSION` 的唯一来源 |
| `crates/whcode-bin/src/main.rs` | CLI 入口，将配置加载失败转为 `WhError` 并通过 logger 输出 |
| `crates/whcode-bin/src/config.rs` | 配置解析/序列化方法，明确把解析失败映射为 `parse_error`、内部问题映射为 `internal` |
| `specs/001-whcode-harness/contracts/app-service.md` | 契约层约束错误码集合（被 error.rs 注释引用） |
| `specs/001-whcode-harness/contracts/cui-cli.md` | 契约层约束 JSON 输出信封格式（被 error.rs 注释引用） |

## 3. 架构与约定

### 3.1 错误码集中声明
`ErrorCode` 是仓库内唯一稳定的错误码枚举，变体包括 `PermissionDenied`、`GateFailed`、`RuntimeUnavailable`、`Backpressure`、`InvalidState`、`NotFound`、`IoError`、`ParseError`、`Timeout`、`Internal`。每个变体都有对应的 `WhError` 便捷构造器（`permission_denied`、`gate_failed`、`backpressure` 等）。

### 3.2 可重试语义
`ErrorCode::default_retryable()` 对 `Backpressure`、`Timeout`、`IoError` 返回 `true`；其余默认 `false`。`WhError::new` 用该值初始化 `retryable` 字段。该语义在 `config.rs` 的文档中被显式解释：I/O 失败标记为 `retryable = true`（修复权限后重试有效），而解析错误不可重试（需人工修正配置）。

### 3.3 信封协议
`OkEnvelope` 与 `ErrEnvelope` 共享 `schema_version`（当前 `SCHEMA_VERSION = 1`），且所有 JSON 输出必须携带该字段。`ErrEnvelope` 直接嵌入 `WhError`，通过 `From<WhError>` 自动转换。

### 3.4 四端共用约束
error.rs 的文件头注释明确约束：“四端（TUI/CUI/Web/SSH）共用本类型，不得各自定义”。这意味着新增错误码或信封字段时需在所有终端适配。

### 3.5 边界划分
- `whcode-domain`：纯领域错误模型，无 I/O 依赖。
- `whcode-bin`：基础设施层，负责把底层错误包装成 `WhError` 并输出。
- 其他 crate（`whcode-app`、`whcode-platform`、`whcode-store`、`whcode-web`、`whcode-ssh`、`whcode-tui`、`whcode-cui` 等）目前仍为空骨架，尚未出现自定义错误类型。

## 4. 约定与规则

### 观察到的约定
- 错误通过 `Result<T, WhError>` 向上传播，不在中间层吞掉；`main.rs` 仅在 `doctor` 子命令中消费错误并以 logger 输出。
- 错误消息面向用户，不包含秘密或未脱敏载荷（error.rs 注释明文规定）。
- 细节信息通过 `WhError::with_details(serde_json::Value)` 附加，并在序列化时使用 `skip_serializing_if = "Option::is_none"`。
- 测试覆盖错误码字符串与契约一致（`error_code_strings_match_contract`）、JSON 往返、无效 JSON 产生 `parse_error`、缺失临时目录产生 `io_error`。

### 明确的规则（有文档/契约依据）
1. **所有 JSON 输出必须携带 `schema_version`** — error.rs 文件头注释直接引用任务书 §8.1 与 contracts/cui-cli.md §3。
2. **错误对象必须包含 `code`、`message`、`retryable`、`details` 四字段** — 同上注释，且 `ErrEnvelope` 的 `#[serde(flatten)]` 强制此布局。
3. **四端不得各自定义错误类型** — 同一注释明确要求 TUI/CUI/Web/SSH 共用 `whcode_domain` 中的类型。
4. **错误码字符串与契约保持一致** — `tests::error_code_strings_match_contract` 断言各 `as_str()` 返回值，构成契约级验证。
5. **可重试语义按错误码分类** — `Backpressure`、`Timeout`、`IoError` 默认可重试；权限、门禁、状态、运行时能力问题必须由调用方改变输入或环境（注释引用 FR-604、§10.2）。
6. **配置解析失败返回 `parse_error`（`retryable=false`）** — `config.rs` 的 `from_json` 方法与测试共同保证。
7. **正常路径不应发生的序列化失败返回 `internal`** — `config.rs` 的 `to_json` 方法的 `# Errors` 文档明确说明。

### 尚未实现的边界
- 项目仍处于“骨架阶段”（main.rs 注释明确列出 M1–M8 未实现事项），除 `whcode-domain` 与 `whcode-bin` 外，多数 crate 尚未集成错误处理。
- 未发现 `panic!`/`recover` 策略文档或全局 panic hook。
- 未见中间件式错误拦截（Web/SSH/TUI 尚未实现）。
