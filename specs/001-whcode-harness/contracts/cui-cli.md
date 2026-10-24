# Contract: CUI 命令行、退出码与 JSON 输出

**Feature**: [spec.md](../spec.md) | **Date**: 2026-10-20

**来源**：任务书 §8.1（CUI 命令草案与修订说明）、FR-205/602。

## 1. 命令树（接口草案）

```text
whcode
  session new|list|show|rename|archive|delete
  run --session ID [--model MODEL] [--trace-level LEVEL]
  status [--json]
  task list|show|retry|abort
  context budget --session ID [--budget N]
  spec status|next|validate|analyze|converge|force-advance
  events list|show|export --session ID [--after SEQ] [--format jsonl]
  db stats|check|purge --before TS|checkpoint [--threshold V] [--autocheckpoint V]|compact|archive --export PATH|restore --from PATH --yes|explain QUERY
  runtime list|status|enable|disable|install NAME
  font install [--from DIR] [--dry-run]
  font status
  font uninstall [--dry-run]
  serve --web [--bind 127.0.0.1:PORT]
  serve --ssh [--bind HOST:PORT]
  gui [--workspace DIR]
  doctor [--json]
```

实现时必须统一参数命名、退出码、错误 JSON 与 `--json` 语义；破坏性命令需支持 dry-run 或确认机制（§8.1）。

四端对等接线说明（FR-619—623、ADR-032）：

- `context budget` 输出与 `whcode_app::load_context` 同源：JSON 含 `schema_version`、预算、
  来源、截断原因与游标（FR-105／108）。
- `runtime list` 输出与 Web `/api/runtimes` 同形（`schema_version`、`wired`、`runtimes`，
  状态名不翻译）；`runtime status|enable|disable|install` 为预留命令。
- 保留命令（`spec next|validate|analyze|converge|force-advance`、`events show`、`llm run`
  等应用层或前端接线未交付项）统一返回 `not_supported` 信封：退出码 **55**，`details` 含
  `surface_id`、`state`（reserved／excluded）与 `milestone` 或 `reason_code`（FR-620、
  ADR-032）；不静默成功、不伪装成 404 语义（`not_found` 仍留给未登记路由）。
`gui` 为装配根入口（与 `serve` 同层）：只读六视图，非交互、无 stdout 协议，退出码 0/1（FR-711）。
`gui` 的按键：`1`—`6` 切视图、`j`/`k` 上下选择会话、`r` 刷新、`t` 循环四套主题（不重启，FR-713）、
`q`／`Esc` 退出。
`font` 为本机资源命令（不触库、不经过会话授权）：把内置 OFL 字体安装到用户级字体目录并写
HKCU 注册（显式、可逆，`uninstall` 撤销）；`install` 输出 Windows Terminal 配置片段供用户自行
粘贴，**不自动改写用户终端配置**（FR-717、ADR-028）；终端字形由终端模拟器控制，程序不声称
可强制更换。

## 2. 修订约束（不得回退）

1. `db vacuum` 不作为通用空间回收承诺：可提供与 DuckDB 原生命令一致的子命令，但必须提示其语义与局限；用户关心的磁盘压缩能力命名为 `db compact`（FR-510、R-09）
2. 数据库命令不接受任意未受控 SQL 作为默认公开 API；`db explain` 只运行只读查询或内部预定义模板，并实施超时与输出限制
3. `spec force-advance` 必须要求 `--reason`，且只用于明确列出的门禁；不得绕过完整性或安全约束而不留下审计（FR-312）

### 退出码表（稳定契约，含 55）

| 退出码 | 语义 |
|---|---|
| 0 | 成功 |
| 2 | 用法／参数错误（clap 解析失败） |
| 10 | `permission_denied` |
| 20 | `gate_failed` |
| 30 | `runtime_unavailable` |
| 40 | `backpressure` |
| 50 | `invalid_state` |
| 55 | `not_supported`——已登记但该端不提供（预留或已登记排除），`retryable=false`（FR-620、ADR-032） |
| 60 | `not_found`（未登记路由／实体不存在） |
| 70 | `io_error` |
| 80 | `parse_error` |
| 90 | `timeout` |
| 99 | `internal` |

55 为 ensure-frontend-parity 变更新增：稳定码，介于 50（状态异常）与 60（不存在）之间——
“已登记但该端不提供”在语义上介于两者之间。CUI 保留命令退出码由 50 改为 55 属 BREAKING
行为变更，CHANGELOG 须标注并附脚本迁移说明（按 55 判“端未提供”，按 404 判“路由不存在”）；
码表同步 `docs/guides/cui-manual.md`。

## 3. 输出契约

- 所有 JSON 输出必须包含 `schema_version`
- 错误对象必须包含 `code`、`message`、`retryable`、`details`
- 退出码必须稳定且文档化；非交互路径必须可供脚本使用（ADR-009）
- `events export` 必须在导出中注明脱敏策略（FR-205）；`--format jsonl` 输出逐行事件，与事件 schema 一致
- 命令输出字段变更视为 schema 变更，须做版本兼容审查（§15 PR 清单）

## 4. 非交互与脚本化要求

| 能力 | 要求 |
|---|---|
| 会话/任务查询 | `--json` 提供稳定结构，不依赖终端宽度或颜色 |
| 事件导出 | `--after SEQ` 游标增量导出，断线流程可脚本化 |
| 维护命令 | 全部支持 dry-run 预览；输出清理前后空间统计 |
| 自检 | `doctor --json` 输出环境、平台、运行时与 schema 版本 |
| 门禁 | `spec analyze/validate` 以退出码区分通过/失败，失败时列出未通过项 |

## 5. 验证点

- 契约测试：每个命令的 `--json` 输出对 `schema_version` 与错误对象字段做断言
- 破坏性命令：dry-run 输出与实际执行前的确认语义一致
- 与 TUI/Web 一致性：同命令在三端最终状态、权限与错误码一致（V-109）
