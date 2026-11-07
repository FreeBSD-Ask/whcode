# 域 L：持久化与事件

> DSH 证据根：`local/dsh-src`（commit `fcc0ad4`）；WHCode 证据根：本仓库。

## 1. 对照表

| 项 | DeepSeek Harness (DSH) | WHCode | 判定 |
|---|---|---|---|
| 存储介质 | JSONL（`session.jsonl[.zstd]`，v1+ 为 `session.vN.jsonl[.zstd]`），一 session 一目录多代际（`docs/architecture.md` Session log） | 单文件 DuckDB，14 表（`crates/whcode-store/migrations/0001_initial.sql` 等；schema v7 见 `crates/whcode-store/src/schema.rs:23`） | **语义不同不可比**：文件代际 vs 关系库 |
| 写入模型 | 句柄单写者：`open(id,'write')` 二次打开即 `SessionAlreadyOwnedError`；跨进程写租约（`docs/subsystems/persistence.md`） | 写者服务单线程持有连接（ADR-017）；前端不直接开库（ADR-018） | **同构**：都是单写者 |
| 事件类型 | `SessionEvent` 唯一类型，无并行持久化类型；事件目录 9426 行（`docs/persistence-catalog.md`） | `EventRecord`＋`EventKind` 稳定名，payload 上限、四级持久化等级（`crates/whcode-domain/src/event.rs`） | **同构** |
| 完整性 | 句柄契约保证“append/flush 后新读至少看到此前缀”；崩溃只可能留未封口尾部 | 幂等键＋FNV-1a 校验和＋迁移链（AGENTS.md §7.1；`whcode-store/src/schema.rs`） | **WHCode 领先**：库内校验和 |
| 迁移 | 相邻 `vN→vN+1` 迁移包，committed generation 永不改名/替换/删除；未来版本直接拒（`docs/architecture.md`） | 迁移 SQL 按版本号顺序执行，失败不写 `schema_migrations`（`schema.rs:4`—`8`、`:37`—`93`） | **同构** |
| 查询/检索 | session-query：逻辑语料（live 优先）＋SQLite 全文索引；事件按 `current`/`shadowed`/`log-only` 三态分类（`docs/subsystems/session-query.md`） | `query.rs`/`overview.rs` 走 SQL；无全文索引、无“遮蔽态”分类 | **DSH 领先** |
| 灾备 | 导出/转录从同一日志派生；无内置备份命令（未在 docs 主轴看到） | `db archive` 归档＋`restore` 恢复（显式确认，无预览）；`purge` 清理 | **WHCode 领先**：运维闭环 |
| 审计 | 审批等以成对事件记日志，无独立审计表 | `audit` 表，11 类动作，不随清理删除，审计失败阻断（AGENTS.md；`crates/whcode-store/src/audit.rs`） | **WHCode 领先** |
| 遥测 | OTel 通道（`docs/subsystems/otel.md`）＋产品遥测（`product-telemetry.md`） | 追踪四级脱敏（redacted 默认），无外部遥测上报 | **取向不同**：DSH 上报，WHCode 本地 |

## 2. DSH 实现解剖

### 2.1 句柄是唯一门

所有读写经 `SessionHandle`：`read` 不回退、`write` 能读到自己刚追加的事件、`close()` 是唯一
拆除且幂等（`persistence.md`）。服务只做 `create/open/stat/list`，不提供按 id 的读写
方法——这样“谁在写”永远是一个具体句柄。

### 2.2 新鲜度契约

“一旦 `append`/`flush` 在写句柄上 resolve，此后**同一后端实例**上开始的任何读都至少看到该前缀”
（`persistence.md`）。这条契约让上层不必猜“写后读是否可见”，也让多句柄并发有明确边界。

### 2.3 代际不可变

committed generation 永不改名、替换、删除；v1+ 用版本化文件名；写打开先编码、校验、**独占发布**
后继代际（`docs/architecture.md`）。失败不会毁掉已有代际，这是“只增不改”的极端形式。

### 2.4 三态分类

session-query 把事件分为 `current`（当前模型上下文）、`shadowed`（被压缩遮蔽）、`log-only`
（从不进模型）（`session-query.md`）。分类复用与模型历史派生相同的 `foldSurface()`，
避免“查询口径”与“模型口径”分叉。

## 3. WHCode 现状核对

- 已实现：14 表（`audit`、`checkpoints`、`context_windows`、`events`、`messages`、
  `runtime_executions`、`runtime_state`、`schema_migrations`、`session_overview`、`sessions`、
  `stage_artifacts`、`tasks`、`tool_executions`、`workspace_meta`）。
- 已实现：schema v7 迁移链（`schema.rs:23`、`:37`），迁移在单事务内执行、SQL 失败不记版本
  （`schema.rs:4`—`8`）；JSON 语义列统一 `VARIANT`（ADR-036）。
- 已实现：写者服务单线程（ADR-017）、前端不触库（ADR-018，`test_layering.rs:135`）、
  审计表不随清理删除、`db archive`/`restore`/`purge` 运维命令。
- 缺失：全文检索（检索 `fts`、`match` 于 `crates/whcode-store` 无实现）、事件遮蔽态分类、
  按会话的跨代际文件发布（库内 schema 演进已覆盖同等需求）。
- 差异：WHCode 的“事件→消息”是两张表（`events`＋`messages`），DSH 是同一日志的投影
  （`deriveMessages()`）。前者写入更重，但查询更快；后者更纯粹，但每次都要 fold。

## 4. 差异判定与可借鉴点

1. **三态分类（current/shadowed/log-only）**：可采纳为查询视图字段。WHCode 折叠后原文仍在
   `events`，但没有“哪些还被模型看见”的标记；加一列/一视图可让 GUI/Web 正确标注
   “已折叠”，也便于审计。代价：折叠时要同步标记。
2. **句柄新鲜度契约**：可采纳为文档级承诺。WHCode 的写者服务是同步单线程，“写后读可见”
   天然成立，但没有写成契约；写进 `data-model.md` 可减少上层猜测。
3. **代际不可变**：不采纳。DuckDB 单文件已经由迁移版本号保证“只增不改”，再引入文件代际
   等于两套版本机制。
4. **全文检索**：可采纳（轻量）。DuckDB 可用 `LIKE`/倒排替代，或引入 FTS 扩展；先做
   `session_overview` 的关键词过滤即可，不必一步到 SQLite FTS。
5. **审计表不随清理删除**：保持，这是 WHCode 相对 DSH 的明确优势（DSH 只有日志事件）。

## 5. 待验证项

1. DSH 是否有内置备份/恢复命令：未在 `docs/subsystems/persistence.md` 与 `docs/user/guide`
   主轴看到。取证：`rg -n "backup|restore" docs/ packages/session/`。
2. WHCode `events` 表的幂等键具体列与唯一约束：只从 AGENTS.md 与模块注释得知。取证：
   读 `crates/whcode-store/migrations/0001_initial.sql` 的 `events` 定义。
3. DSH JSONL 后端是否默认压缩（`.zstd` 是否总是启用）：`docs/architecture.md` 只列两种文件名。
   取证：读 `packages/session/session-persistence-jsonl/README.md`。

## 6. 证据路径清单

- `docs/subsystems/persistence.md`（SessionHandle 单写者）
- `docs/subsystems/persistence.md`（新鲜度契约）
- `docs/subsystems/session-query.md`（三态分类）
- `docs/architecture.md`（代际与迁移）
- `docs/persistence-catalog.md`（事件目录）
- `crates/whcode-store/src/schema.rs:23`（schema v7）
- `crates/whcode-store/src/schema.rs:37`（迁移表）
- `crates/whcode-store/migrations/0001_initial.sql`（14 表）
- `crates/whcode-store/src/audit.rs`（审计表）
- `crates/whcode-domain/src/event.rs`（事件等级）
- `tests/contract/test_layering.rs:135`（前端不触库）
