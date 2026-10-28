# 域 L：持久化与事件

> ZCode 证据路径以 `local/zcode-src`（commit 3c5b227）为根，以下省略前缀，实际绝对路径为
> `D:\WHCode\local\zcode-src\...`。

## 1. 关键路径勘误

- 迁移编号**不是**任务列的“”，实际分两套独立编号序列：
  - CLI 会话库（`packages/services/src/session/tasksDatabase/schema-v1.ts`）使用
    `apps/zcode-cli/packages/services/src/session/tasksDatabase/schema-v1.ts`，迁移 **0001—0006**；
  - 共享 SQLite 迁移（`packages/shared/src/sqlite-migrations/`）使用 `schema-migrations.ts`，
    迁移 **0001—0023**。
- `tasks-index` 表数**超过**原判 6 张（实际 7+ 张：conversations/messages/tasks/dependencies/...）。

## 2. 对照表

| 项 | ZCode | WHCode | 判定 |
|---|---|---|---|
| 存储引擎 | SQLite 多文件（CLI 会话库 + 共享迁移库 + 各插件自有库），`better-sqlite3` 同步 API | DuckDB 单文件 14 表，schema v7，`whcode-store` 唯一链接点 | WHCode 单文件更简洁；ZCode 多文件职责分离 |
| 迁移 | 0001—0006（tasks-index）+ 0001—0023（共享库）；`schema-migrations.ts` 逐号执行，无版本表，靠 `user_version` PRAGMA | `migrations.rs` 顺序迁移，schema_version 唯一出处 | 相当；WHCode 有单一版本出处更可审计 |
| 备份/恢复 | **先手**：`backup`/`restore` 命令（`packages/services/src/backup.ts`）——SHA-256 校验和 + 自动预迁移备份 + `restore` 脚本；运维闭环完整 | 无备份/恢复命令（仅崩溃恢复单事务幂等） | **ZCode 先手**：WHCode 缺运维闭环 |
| 事件 | `SessionEvent` 流（`packages/core/src/runtime/session-events.ts`），事件经 `recordSessionEvent` 落库 | `EventRecord` + `events` 表（EventKind 十类），`record_intent`→`mark_running`→`record_result` 三步登记 | 相当；WHCode 有崩溃恢复（`recover_uncertain`） |
| 原子性 | SQLite 事务；`tasksDatabase` 写操作包事务 | 三表原子提交（UPDATE tasks + INSERT events + INSERT checkpoints，`checkpoints.rs:276`） | WHCode 更强（跨表原子+审计） |
| 校验和 | SHA-256 校验和用于备份完整性 | 无备份校验和 | **ZCode 先手** |

## 3. 与 WHCode 差异修正（相对原差异书）

- **修正**：迁移编号序列不是“”，实际为两套独立序列（0001—0006 + 0001—0023）。
- **修正**：tasks-index 表数超过原判 6 张（实际 7+ 张）。
- **补充**：ZCode 具备备份/恢复运维闭环（SHA-256 校验和 + 自动预迁移备份 + restore 脚本），
  这是 WHCode 所缺的**运维先手**，值得列为 P 级补齐候选。

## 4. 存疑项

- 具体迁移号对应表结构变更需逐号读 `schema-migrations.ts` 确认 `[待验证]`。
- ZCode 事件 `SessionEvent` 的持久化是否全量落库、还是仅内存流，需读 `recordSessionEvent`
  调用点确认 `[待验证]`。
