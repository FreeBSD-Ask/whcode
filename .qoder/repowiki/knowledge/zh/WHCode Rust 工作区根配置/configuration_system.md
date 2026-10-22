## 概述

WHCode 的运行时配置由 `crates/whcode-bin` crate 提供，采用「默认值基线 + JSON 覆盖」的两层模型。当前骨架阶段仅暴露 `--version`、`doctor [--config PATH] [--json]` 三个入口，完整命令面待 M2 迁移至 `whcode-cui`。

## 核心文件与类型

- `crates/whcode-bin/src/config.rs`：配置结构定义、默认值构造、JSON 序列化/反序列化、启动期校验。
- `crates/whcode-bin/src/main.rs`：CLI 参数解析、配置文件读取、`doctor` 诊断输出。
- `whcode-domain::TraceLevel`：追踪级别枚举，作为 `TraceConfig::default_level` 的默认值来源。
- `whcode-platform::{paths, resources}`：平台相关路径与工作区临时目录、物理核数探测。

顶层结构为 `Config`，包含：
- `workspace_path: PathBuf` — 工作区根（数据库、产物与持久化状态边界，ADR-001）。
- `db: DbConfig` — DuckDB 缓冲管理器基线（memory_limit_mb=192、max_memory_limit_mb=384、threads=min(4, cores)、temp_directory）。`max_temp_directory_size_mb` 可选，默认 `None`。
- `trace: TraceConfig` — 追踪基线，`default_level` 默认 `TraceLevel::Redacted`。

## 架构与约定

1. **默认值构造**：`Config::for_workspace(workspace_path)` 通过 `resources::available_physical_cores()` 与 `paths::workspace_temp_dir(&workspace_path)` 计算线程预算与临时目录（形如 `<workspace>/tmp`），并注入 §5.4 规定的内存上限常量。
2. **JSON 覆盖**：`from_json` / `to_json` 基于 `serde`；反序列化失败统一包装为 `WhError::parse_error(retryable=false)`，序列化失败返回 `WhError::internal`。
3. **启动期校验**：`check_temp_directory` 要求临时目录存在且可写，不存在时返回 `IoError`（`retryable=true`，修复权限后重试有效）。
4. **CLI 加载策略**：`load_config` 在 `main.rs` 中实现——若传入 `--config PATH`，则读取该 JSON 文件；否则以当前工作目录为 `workspace_path` 调用 `for_workspace` 生成基线。
5. **诊断输出**：`doctor --json` 输出 `{"schema_version": ..., "doctor": <config.json>}`；普通模式打印 workspace、platform、threads_baseline、memory_limit_mb、trace_default、temp_directory 可用性，并在未设置 `max_temp_directory_size_mb` 时发出警告日志（§5.4 建议）。
6. **依赖约束**：`Cargo.toml` 将 `serde`、`serde_json`、`thiserror` 声明在 `[workspace.dependencies]`，确保跨 crate 版本一致；`unsafe_code = "forbid"` 禁止不安全代码。
7. **文档锚定**：配置字段注释直接引用任务书章节（§5.4）、FR-203、D-05、ADR-001 等设计决策，使默认值与设计契约一一对应。

## 约定与规则

- 配置默认值被定义为“可配置基线”，不得写死为架构定理（见 config.rs 头部注释及 §0.3-6 引用）。
- 配置错误（JSON 解析失败）标记为 `retryable=false`，因为需人工修正而非自动重试。
- `DbConfig::max_temp_directory_size_mb` 使用 `#[serde(default, skip_serializing_if = "Option::is_none")]`，使缺失字段在 JSON 中省略，保持向后兼容。
- `TraceConfig` 派生 `Default`，其默认值由 `TraceLevel` 的 `#[default]` 变体（`redacted`）提供，避免重复硬编码。
- `whcode-bin` 是配置加载的唯一入口；其他 crate 不直接持有配置，而是通过 whcode-domain 的错误类型和 whcode-platform 的路径工具间接消费。
- 测试断言默认值与任务书基线一致（`defaults_match_task_book_baseline`），并要求 JSON roundtrip 保留所有字段值。

## 当前限制

- 仅支持 JSON 格式配置文件；无 `.env`、`.yaml`、环境变量覆盖或 feature-flag 机制。
- 仅 `whcode-bin` crate 拥有配置逻辑；其余 crate 尚未集成配置注入（骨架阶段，M1/T032+ 之后扩展）。
- 临时目录创建属于后续任务（T032/T047），当前 `check_temp_directory` 只做存在性与可写性检查。