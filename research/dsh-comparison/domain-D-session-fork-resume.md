# 域 D：会话分叉、恢复与回滚

> DSH 证据根：`local/dsh-src`（commit `fcc0ad4`）；WHCode 证据根：本仓库。

## 1. 对照表

| 项 | DeepSeek Harness (DSH) | WHCode | 判定 |
|---|---|---|---|
| 事实源 | append-only `SessionEvent` 日志，模型历史由 `deriveMessages()` 从日志投影（`docs/architecture.md` Session log 一节） | `events` 表（seq 单调、幂等键、FNV 校验和），消息单独落 `messages` 表（`crates/whcode-store/src/**`） | **同构**：都是事件溯源 |
| 分叉 | `SessionStore.fork(source, boundary?, childId?)`：按**闭区间 seq 边界**复制前缀，`isSeeded:true`＋精确 `inheritedEventCount`，可指定任意稳定 turn 间位置（`docs/subsystems/session.md`） | **无分叉**（检索 `fork`/`revert`/`rewind` 于 `crates/` 无实现命中） | **结构性缺失（WHCode）** |
| 边界合法性 | 边界落在开着的 turn 内则**拒绝**，不静默裁剪（`session.md`） | 不适用 | **DSH 领先**：拒绝半截分叉 |
| 迁移 | 相邻 `vN→vN+1` 迁移链，committed generation 永不改名/替换/删除；只读 open 不发布后继，写 open 先校验再独占发布（`docs/architecture.md` Session log） | DuckDB schema v7，迁移带 FNV-1a 校验和（ADR-036；`crates/whcode-store/src/migrations*`） | **语义不同不可比**：文件代际 vs 库内 schema |
| 中断尾修复 | 未封口尾部由 handle 消费者负责；只为“已被后续 `turn/start` 封住的发布期重启”插 `turn/end`（`docs/architecture.md`） | `recover_uncertain` 把停在中间态的工具执行改成终态（`crates/whcode-store/src/tool_exec.rs`） | **同构**：都只修“可判定的半截” |
| 检查点 | 无独立检查点表；靠日志前缀 + 分叉边界 | `checkpoints` 表按任务保留默认 10 条（`crates/whcode-domain/src/checkpoint.rs:27`），超限裁剪（`crates/whcode-store/src/checkpoints.rs:190`—`194`） | **WHCode 领先**：有界保留 |
| 文件回滚 | **无**：分叉只复制对话，不恢复工作区文件（`session.md` 只谈事件） | 无（`recover_from_checkpoint` 只恢复任务状态与消息游标，`checkpoints.rs:426`—`440`） | **两侧都缺**：DSH 缺文件前像，WHCode 缺对话回滚 |
| 导出 | transcript/telemetry/persistence 都从同一日志派生 | `events_export` 按需导出；`db archive` 归档文件 | **同构** |

## 2. DSH 实现解剖

### 2.1 分叉是“精确前缀复制”

`fork()` 拿到源 `Session`，按传入的 `SessionSeq` 边界（默认取当前最后一条）选出事件前缀，
**先关闭源里开着的尾**，再深拷贝事件生成独立会话，标记 `isSeeded: true` 并写入精确的
`inheritedEventCount`（`docs/subsystems/session.md`）。子会话没有 `parentSession`
字段——父子关系靠种子标记里的 `inherited: true` 反推（`session.md`）。

### 2.2 边界必须落在 turn 之间

`fork()` 对“前缀结束在一个未闭合 turn 内部”直接拒绝（`session.md`）。相形之下
`dsh-subagent-fork-in-process` 会自行裁剪到已完成前缀，因为工具内委派常发生在父 turn 还开着的时候。
两条路径分开，避免普通会话分叉静默丢掉半个 turn。

### 2.3 种子标记与代际

`buildForkSeed` 在继承前缀的切点上追加 `session/end-seed { inherited: true }`，再追加合成的
“关闭”事件；解档时定位**最后**一个带 `inherited: true` 的标记，并要求它恰好出现在
`SessionHeader.isSeeded` 为真时（`session.md`）。代际迁移只增不改：
v1+ 使用 `session.vN.jsonl`，committed generation 永不移动（`docs/architecture.md`）。

### 2.4 恢复的责任划分

未封口尾部（unsealed tail）的普通修复由**句柄消费者**负责；迁移只为一种受限情形补
`turn/end`——发布期重启已被后续 `turn/start` 封住（`docs/architecture.md`）。这条边界
把“能判定的”交给程序，“不能判定的”留给人或上层。

## 3. WHCode 现状核对

- 已实现：事件表 seq 单调与幂等键（`crates/whcode-domain/src/event.rs`）、任务状态机检查点
  同事务写入（`crates/whcode-store/src/checkpoints.rs:276`—`360`）、检查点默认保留 10 条并裁剪
  （`checkpoint.rs:27`；`checkpoints.rs:190`）。
- 已实现：`recover_from_checkpoint` 只加载检查点行与其引用（FR-404 按引用恢复），
  `recover_uncertain` 修复三步登记的中间态（`crates/whcode-store/src/tool_exec.rs`）。
- 缺失：会话分叉、对话剪枝、文件前像回滚。检索 `fork`/`revert`/`rewind`（`crates/`）仅命中
  `serve.rs` 注释中的“回退”，与能力无关。
- 差异：WHCode 的检查点属于**任务**（SDD 状态机），DSH 的分叉属于**会话**（对话树）。两者
  不在同一层，不能互相替代。

## 4. 差异判定与可借鉴点

1. **分叉边界拒绝半截 turn**：可采纳。若 WHCode 将来做会话分叉，边界必须对齐
   `turn/end` 事件 seq；落在 turn 内应显式拒绝（`InvalidState`），不得裁剪。
2. **`inheritedEventCount` 显式化**：可采纳。WHCode 若复制事件前缀，必须在会话元数据里存
   “继承前缀长度”，否则重放无法区分“自己的事件”和“继承来的事件”。
3. **父子关系靠标记而非外键**：不采纳。WHCode 是关系库，`sessions.parent_id` 外键更自然，
   也便于 `ON DELETE` 语义；DSH 用标记是因为它是 append-only 文件。
4. **文件前像回滚**：不建议现在做。需要每 turn 抓工作区快照，成本与磁盘增长都高；WHCode 的
   资源铁律要求有界，落地前必须先定保留策略与上限。
5. **保留 10 条检查点**：保持。DSH 没有等价机制，这是 WHCode 的既有优势（有界、可证）。

## 5. 待验证项

1. DSH 分叉后子会话的**持久化时机**：文档说“只由 agent-loop 发布的会话才持久化”
   （`docs/architecture.md`），但未确认 fork 出的 seeded 会话是否立即落盘。取证：读
   `packages/session/**` 的 persistence 写入条件与 `isSeeded` 分支。
2. WHCode `recover_from_checkpoint` 是否恢复消息正文还是只恢复游标：只读了函数签名与模块注释
   （`checkpoints.rs:12`—`14`、`:426`），未读函数体。取证：读 `:426`—`500`。
3. DSH 迁移链是否存在“拒绝未来版本”的显式错误码：`docs/architecture.md` 说“refuses a future
   version”，未给错误类型。取证：`rg -n "future version|UnsupportedVersion" packages/session/`。

## 6. 证据路径清单

- `docs/subsystems/session.md`（fork API）
- `docs/subsystems/session.md`（边界拒绝）
- `docs/subsystems/session.md`（seed 标记）
- `docs/architecture.md`（Session log／代际迁移）
- `packages/core/session/src/**`（`inheritedEventCount` 字段定义于 `session.md` 引用）
- `crates/whcode-domain/src/checkpoint.rs:27`（保留 10 条）
- `crates/whcode-store/src/checkpoints.rs:190`（裁剪）
- `crates/whcode-store/src/checkpoints.rs:426`（按引用恢复）
- `crates/whcode-store/src/tool_exec.rs`（recover_uncertain）
