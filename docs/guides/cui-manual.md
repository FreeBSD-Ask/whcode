# CUI 手册（T119、§12.2）

**版本**：`0.1.0` | **更新**：2026-10-24 | 契约：`contracts/cui-cli.md`

## 1. 全局约定

- 所有 JSON 输出含 `schema_version`；错误对象恒四字段（`code`/`message`/`retryable`/
  `details`）；稳定退出码：成功 `0`、用法/参数错误 `2`（clap 解析失败），错误按类别
  `10`（权限）/`20`（门禁）/`30`（运行时）/`40`（背压）/`50`（状态）/`55`（已登记但该端
  未提供，`not_supported`）/`60`（不存在）/`70`（I/O）/`80`（解析）/`90`（超时）/`99`（内部）。
  依赖退出码的脚本按 `55` 判“端未提供”、按 `60` 判“对象不存在”；`55` 为 2026-10-24
  的 BREAKING 变更（原保留命令用 `50`），见 `CHANGELOG.md`。
- `--workspace DIR` 指定工作区（默认 cwd；数据库 `<workspace>/whcode.duckdb`）；
  `--json` 可放在子命令前后（全局旗标）。
- 会话定位为显式 `--session`/`--task`：不设隐式"当前会话"（脚本路径不依赖隐藏状态）。
- 非交互脚本路径可用（ADR-009）：`--json` + 退出码即可编程消费。
- 权限：全部命令经应用服务统一判定，拒绝不可重试（FR-604）。

## 2. 会话命令

```bash
whcode session new --title "修复登录"          # 创建（自动分配 S- 前缀 ID）
whcode session list [--all]                    # 列表（默认不含归档）
whcode session show --session S-1              # 元数据
whcode session rename --session S-1 --title "新标题"
whcode session archive --session S-1 [--unarchive] [--dry-run] [--yes]
whcode session delete --session S-1 [--dry-run] [--yes]   # 破坏性：默认只预览
```

归档属契约 §2-1 破坏族（ADR-038）：默认只输出 dry-run（`dry_run=true`、`current_archived`、
`archived`、`changed`），`--yes` 才执行并写 `session_archive` 审计；`--dry-run` 与 `--yes`
互斥。旧脚本若直接依赖“归档立即生效”，须显式加 `--yes`（属行为变更，见 CHANGELOG）。

删除有预览/确认/审计/活跃会话保护（FR-509）四项机制；显式按 ID 删除（`--yes`）时
不套用 TTL 的活跃窗口保护（否则"删除刚创建的会话"会被静默跳过）；`db purge` 仍保留
活跃窗口保护。

## 3. 任务命令

```bash
whcode task list --session S-1
whcode task show --task T-1        # 依赖/步骤/产物引用/最近事件/失败原因
whcode task retry --task T-1       # 仅 failed/aborted/interrupted 可重试
whcode task abort --task T-1       # 终态与 unknown 不可中止（FR-410）
```

## 4. SDD 门禁

```bash
whcode spec status --session S-1    # 阶段（状态机：Constitution→Specify→Clarify→
                                    # Plan→Tasks→Analyze→Implement→Checklist→Converge）
whcode spec next --session S-1
whcode spec analyze --session S-1   # 五类缺陷（遗漏/孤儿/无效引用/循环依赖/覆盖不足）
whcode spec converge --session S-1  # 验收四态 + 差异报告
whcode spec force-advance --session S-1 --to plan --reason "理由"   # reason 必填
```

高优先级澄清未解决时 Plan 被拒；Analyze 未通过时 Implement 被拒（FR-307）——
仅提示词要求不构成门禁（FR-308）。

## 5. 上下文与运行时

```bash
whcode context budget --session S-1 [--budget N]   # 预算/来源/截断原因/游标（FR-105/108）
whcode runtime list --json                         # 运行时清单（与 GET /api/runtimes 同形）
```

- `context budget` 缺省预算取展示默认口径；**会话不存在返回 `60`（not_found）**，
  不把“会话不存在”伪装成空预算；JSON 含 `schema_version`、预算、来源、截断原因与游标。
- `runtime list` 只读声明清单（`schema_version`/`wired`/`runtimes`），不触库、不启用运行时。
  `runtime status/enable/disable/install` 属 M8 装配开关，返回 `55` + `not_supported`
  （`details.surface_id`、`state`、`milestone`）。

## 6. 事件

```bash
whcode events list --session S-1 --after 100
whcode events export --session S-1 --after 0 --format jsonl   # 首行头部注明脱敏策略
```

`redacted`（默认）级别下疑似秘密字段替换为 `[REDACTED]`（FR-203/205、D-05）。

## 7. 数据库维护

```bash
whcode db stats                     # 文件大小/WAL/已用块/可用块（FR-507/510）
whcode db check                     # 必需索引检查
whcode db purge --before 1700000000000 [--dry-run] [--yes]   # 清理（预览/确认/审计/活跃保护）
whcode db checkpoint                # 显式 checkpoint
whcode db compact [--dry-run] [--yes]   # 复制—校验—切换（不承诺缩小文件，FR-510）
whcode db archive --export PATH     # 工作区导出（whcode-export v1）
whcode db explain --query "SELECT ..."   # 只读查询的查询计划
```

`db vacuum` **不存在**：磁盘压缩能力命名为 `db compact`（FR-510、R-09）。

## 8. 诊断

```bash
whcode doctor [--json]              # 环境与配置自检
```

## 9. 实现状态（2026-10-24 四端对等收口）

已接线（真实执行，退出码见 §1）：
`session new/list/show/rename/archive/delete`、`status`、`task list/show/retry/abort`、
`events list/export`（json/jsonl）、`context budget`、`runtime list`、`spec status`、
`db stats/check/purge/compact/archive/explain`。

未接线（显式失败，退出码 `55` + `not_supported` 信封，不静默成功）：
`run`（Provider 流式装配）、`events show`（按 ID 查询接口未提供）、
`spec next/validate/analyze/converge/force-advance`（需 SDD 产物工作区上下文）、
`runtime status/enable/disable/install`（运行时装配开关属 M8）、
`db checkpoint`（压缩内部步骤，不单独公开）。

同命令一致性：读命令见 `tests/contract/test_tui_cui_parity.rs`
（CUI 视图 vs Web `/api/*` 同数据）；写命令四端同源 `whcode-app::commands` 门面，
最终状态、审计与权限拒绝码的一致性由 `tests/contract/test_frontend_parity.rs` 的行为等价
夹具证明（FR-623）。批次 2 取证：`artifacts/cui-2026-10-22-batch2.txt`；
四端对等收口证据：`artifacts/frontend-parity-2026-10-24/`。

## 10. Web 服务（`serve --web`，ADR-023）

```bash
whcode serve --web                                   # 回环默认（127.0.0.1:8080），无认证
whcode serve --web --bind 192.168.5.4:8737        --token-file /secure/token.txt                # 局域网（RFC1918）必须带 token 文件
```

- 地址边界：只允许回环或 RFC1918 私网；公网 IP 与 `0.0.0.0` 拒绝启动（"仅限局域网"）。
- 局域网监听强制认证与 Origin 检查；token 只从文件读取（不经命令行）。
- 浏览器凭据：`http://<ip>:<port>/#token=<token>`——fragment 不发送给服务器；
  前端以 `Authorization: Bearer` 调用 `/api/*`。
- 静态外壳（`/`、`app.js` 等）无认证即可加载；**业务数据全部经 `/api/*` 且需 token**。
- 不承诺公网暴露、TLS 与暴力破解防护（ADR-023 退出条件）。
