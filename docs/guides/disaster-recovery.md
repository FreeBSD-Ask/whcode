# 灾难恢复与数据带离（FR-410、FR-508、FR-510、NFR-218）

> 适用：单文件 DuckDB 工作区。事实基准：`crates/whcode-store`、`crates/whcode-bin`、
> `crates/whcode-cui`（2026-10-23 对照）。本文只写**已实现**的路径；未接线的能力
> 单列于 §8，禁止按“存在命令”理解。

## 1. 数据在哪里

| 项 | 值 | 证据 |
|---|---|---|
| 库文件 | `<workspace>/whcode.duckdb`（WAL 为同名 `.wal`） | `crates/whcode-bin/src/workspace.rs:23`、`45-47`；`crates/whcode-store/src/compact.rs:86-88` |
| 工作区解析 | `--workspace DIR` ＞ 配置 `workspace_path` ＞ 当前目录 | `workspace.rs:30-41`；`crates/whcode-bin/src/main.rs:393-412` |
| 临时目录 | `<workspace>/tmp`（启动时创建） | `workspace.rs:56-58` |
| 进程独占 | 同一工作区同一时刻只允许一个进程打开库（跨进程锁冲突走 `io_error`） | `workspace.rs:9-12`、`59-64` |
| 启动动作 | 幂等迁移 + 工具执行不确定态恢复 | `workspace.rs:81-102` |

**恢复的第一步永远是停掉所有 `whcode` 进程**（TUI、`serve --web`、`bench`）。

## 2. 备份：导出（已实现）

```bash
# 工作区导出（whcode-export v1，UTF-8 JSON）
whcode db archive --export D:\backup\whcode-export.json

# 单会话事件导出（json / jsonl）
whcode events export --session S-001 --format jsonl > .\events-S-001.jsonl
whcode events export --session S-001 --after 9900 --format json
```

| 事实 | 说明 | 证据 |
|---|---|---|
| 格式 | `{"format","format_version","manifest","tables","chunk_checksums"}` | `crates/whcode-store/src/export.rs:3-4`、`49-52` |
| 覆盖表 | 11 张业务表，含 `events`、`messages`、`checkpoints`、`tasks` | `export.rs:55-67` |
| 排除表 | **不含** `audit`、`runtime_state`、`schema_migrations`（审计不随导出扩散） | `export.rs:70`、`8-12` |
| 完整性 | 每表每 1000 行一块 FNV-1a-64 校验 + 文件级哈希；**只防意外损坏，不防蓄意篡改** | `export.rs:73`、`299-332`、`438`、`28` |
| 审计 | 导出本身写 `data_export` 审计（计数、内容哈希、排除表） | `export.rs:440-459` |
| 内存纪律 | 导出流式（单条 SELECT、无 OFFSET）；导入整体入内存 | `export.rs:29-31` |
| 注意 | 导出审计写失败时文件已落盘，须调用方自行删除 | `export.rs:33`、`340` |

事件导出的 JSONL 首行是头（`trace_level`/`redacted`/`integrity`），秘密字段不出现
（`crates/whcode-cui/src/events_cmd.rs:33-68`、`116-120`）。

## 3. 恢复：当前只有“文件级还原”

`import_workspace` 库函数存在（含六道校验，任一失败整体拒绝），但**未接入任何
CLI/HTTP 入口**（`export.rs:555-698`；调用点仅在测试中）。

因此当前的恢复流程是**文件级**：

```text
1. 停止所有 whcode 进程
2. 备份“当前”库文件（即使已损坏，保留现场用于取证）：
     copy whcode.duckdb  whcode.duckdb.broken-YYYYMMDD-HHMM
3. 用可信副本覆盖：
     copy <可信副本>.duckdb  whcode.duckdb
4. 删除同名 WAL（避免旧 WAL 叠加到新副本）：
     del whcode.duckdb.wal
5. 启动并核对：
     whcode db stats --json      # 文件大小、WAL、块一致性
     whcode db check             # 必需索引是否齐全
```

导出文件（whcode-export v1）用于**迁移与留档**，当前不能一键回灌；需要回灌时必须
先补齐导入入口（见 §8）。这一点在 `specs/001-whcode-harness/quickstart.md` 中同样按
“接口草案”处理，不构成已交付能力。

## 4. compact（唯一会替换库文件的维护动作）

```bash
whcode db compact              # 缺省即 dry-run：只报文件/WAL 字节，不写库
whcode db compact --yes        # 执行
```

| 阶段 | 行为 | 证据 |
|---|---|---|
| 互斥 | 进程内维护锁，并发调用直接 `invalid_state` | `crates/whcode-store/src/compact.rs:36`、`94-99` |
| 前置 | 清理遗留 `.compact-tmp` / `.compact-backup`（可重跑） | `compact.rs:199`、`214` |
| 复制 | `CHECKPOINT` 后关连接，复制到 `db.compact-tmp` | `compact.rs:193-194`、`200` |
| 校验 | 副本校验 schema、索引、逐表行数；失败删副本且**原库不受影响** | `compact.rs:136-165`、`202-211` |
| 切换 | 原子替换，旧库暂存 `.compact-backup` | `compact.rs:213-220` |
| 收尾 | 清 WAL 残留与备份，重开复验并采集统计 | `compact.rs:221-237` |
| 空间承诺 | **不承诺缩小文件**（已删行的块会随复制保留） | `compact.rs:17-19`；`cui_dispatch.rs:653` |
| 参数冲突 | `--dry-run` 与 `--yes` 同给 → 用法错误（退出码 2） | `cui_dispatch.rs:126-134` |

失败一律以 `WhError` 返回：库缺失 `not_found`、并发/校验失败 `invalid_state`、I/O
`io_error`（`compact.rs:74-76`、`98`、`144-163`、`177-182`）。

## 5. 崩溃恢复与不确定副作用（FR-410）

打开工作区时执行一次恢复扫描：把 `prepared`/`running` 的工具执行一律标为 `unknown`
并返回清单，**不重放副作用**、也不宣称未发生（`crates/whcode-store/src/tool_exec.rs:9-20`）。

| 项 | 值 | 证据 |
|---|---|---|
| 单次扫描上限 | 1000 条，超出 `truncated=true`，下次继续 | `tool_exec.rs:26`；`workspace.rs:99-101` |
| 提示位置 | stderr，含前 5 个 ID | `workspace.rs:93-98` |
| 处置口径 | 核对副作用是否已发生 → 幂等重试或人工决策 | `tool_exec.rs:29` |
| 断言 | 打开工作区即恢复 | `workspace.rs:140-162` |

## 6. 损坏与打开失败的实际表现

**未实现**启动时的 `PRAGMA integrity_check` 或等价校验，也没有“库损坏”专用错误码；
现有检查只有：schema 版本可读（`crates/whcode-store/src/schema.rs:56-70`）、必需索引
（`crates/whcode-store/src/indexes.rs:1-8`，经 `whcode db check`）、导出/导入校验和。

| 场景 | 错误码 | 文案（原文） |
|---|---|---|
| 打开失败（含跨进程锁冲突） | `io_error` | 打开数据库失败（<路径>）：<原因> |
| 迁移失败 | `io_error` | schema 应用迁移 v<N> 失败：<原因> |
| 数据库参数设置失败 | `invalid_state` | 配置数据库参数失败：<原因> |
| 临时目录不存在 | `io_error` | 临时目录不存在：<路径> |
| compact 目标不存在 | `not_found` | —— |

证据：`workspace.rs:59-80`、`schema.rs:40-42`、`115`、`config.rs:111-120`、`compact.rs:177-182`。
退出码映射见 `crates/whcode-cui/src/output.rs:13-16`、`42-57`。

## 7. 清理（purge）的保护边界

```bash
whcode db purge --before 1767225600000 --dry-run     # 预览（Unix 毫秒）
whcode db purge --before 1767225600000 --yes         # 执行
whcode session delete --session S-001 --dry-run
```

| 保护 | 语义 | 证据 |
|---|---|---|
| 审计 | `audit` **永不**随 TTL 清除 | `crates/whcode-store/src/purge.rs:6-7`、`34-36` |
| 全局状态 | `workspace_meta`、`runtime_state`、`schema_migrations` 不随会话删除 | `purge.rs:8` |
| 工具执行 | `tool_executions` 不在会话清理范围 | `purge.rs:9` |
| 活跃会话 | 默认保护 24 h；命中默认**跳过**而非中止 | `purge.rs:56`、`146-151`、`156-174` |
| 双时点检查 | 预览与执行各查一次保护，跳过项列在 `skipped_protected` | `purge.rs:19-29` |
| 确认 | 存储层要求 `confirm == true`，否则拒绝 | `purge.rs:15-16`；`crates/whcode-app/src/maintenance.rs:38-49` |
| 审计一致性 | 删除与审计同一事务，审计失败整体回滚 | `purge.rs:16-18` |
| 批量上限 | 单批 IN 列表 256 个 ID | `purge.rs:53` |

口径提醒：TTL 与活跃窗口交叠时“清理 0 条”是**正确行为**，不是缺陷（`purge.rs:31-36`）。

## 8. 未实现（显式登记，勿依赖）

> 2026-10-24 功能缺口清偿批次接线原第 1—3 条：`db restore --from PATH --yes`（恢复到
> 空库，非空目标拒绝且零写入——`whcode_app::maintenance::restore_workspace`）；
> `db checkpoint [--threshold V] [--autocheckpoint V]`（显式 CHECKPOINT + WAL 策略覆盖，
> 经 `whcode_app::maintenance::checkpoint_database` → `apply_wal_policy`/`run_checkpoint`，
> 两函数自此有命令调用方）。端到端实测：export→restore 往返 1 会话/2 事件一致；
> 非空目标退出码 50 且零写入；`--threshold 32MB` 读回 `30.5 MiB`（DuckDB 原文）；
> 不存在的会话导出（同批修复）退出码 60 + `not_found` 可操作错误。

1. 空闲窗口自动 checkpoint（`should_checkpoint_on_idle`）仍无事件循环调用方——空闲
   维护属调用方职责，显式命令已覆盖实际需要；如需自动空闲维护须先立并发设计档案
   （ADR-015）再接入。
2. 无启动期完整性校验、无自动备份、无快照保留策略（检查点策略 `CheckpointPolicy`
   仅存在于配置与领域类型，未接入写路径）。
3. 无“删除整库/卸载自身”命令；数据带离出口是 `db archive --export`，恢复入口是
   `db restore --from`。

## 9. 演练清单（建议每次发布前执行）

| # | 命令 | 预期 |
|---|---|---|
| 1 | `whcode db stats --json` | 得到 `file_size_bytes`/`wal_size_bytes`/`blocks_consistent` |
| 2 | `whcode db check` | 无缺失索引；有则列出清单 |
| 3 | `whcode db archive --export <备份>` | 生成 whcode-export v1，`audit` 表被排除 |
| 4 | 停进程 → 文件级还原 → 重启 | `db stats` 与还原前一致，工具执行清单不重放副作用 |
| 5 | `whcode db compact --yes` | 副本校验通过才切换；失败时原库不受影响 |
| 6 | `whcode db purge --before <未来时间> --dry-run` | 只读预览，报满 9 张表名，活跃会话被跳过 |
