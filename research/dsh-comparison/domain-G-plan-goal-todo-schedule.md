# 域 G：计划、目标、待办与定时

> DSH 证据根：`local/dsh-src`（commit `fcc0ad4`）；WHCode 证据根：本仓库。

## 1. 对照表

| 项 | DeepSeek Harness (DSH) | WHCode | 判定 |
|---|---|---|---|
| 计划模式 | `plan/mode {active}` 是会话语义上的 log-only 事件，可重放；激活时向每个请求注入部署自定义的 `plan:policy` 段（`docs/subsystems/plan.md`） | 无（检索 `plan_mode`/`PlanMode`/`todo_write`/`goal` 于 `crates/` 零命中） | **结构性缺失（WHCode）** |
| 强制力 | **软引导**：只注入提示词，不阻止任何工具；限制由 sandbox/approval 独立执行（`plan.md`） | 无 | **DSH 领先**（但明确不假装强制） |
| 退出协议 | `exit_plan_mode` 工具：必须提交以 `#` 开头的完整 markdown 计划，经 user-questions 复核；批准→静默待生效退出；驳回→本次调用失败并带反馈（`plan.md`） | 无 | **DSH 领先** |
| 待办 | `todo/write` 整表替换、log-only；条目只有 `content`＋三态 `pending/in_progress/completed`，刻意无 id/优先级（`docs/subsystems/todo.md`） | 无会话级 todo；`tasks` 表是 SDD 任务账本 | **语义不同不可比** |
| 目标 | 同会话目标：`GoalRef` 用 revision 做 CAS，阶段 `active/paused/blocked/complete`；进程内激活与持久阶段分离（`docs/subsystems/goal.md`） | 无 | **DSH 领先** |
| 定时 | 宿主级任务，7 种计时（`after/at/every/daily/weekly/cron/rrule`），独立于会话激活；支持复用/新建/每次新建会话三种目标（`docs/subsystems/schedule.md`） | 无定时；turn 是请求驱动的 | **DSH 领先** |
| 与 turn 的关系 | plan 选择在**下一个被接受的 in-turn pre-step** 追加；不强制续跑（`plan.md`） | turn 只由前端命令触发 | **DSH 领先** |

## 2. DSH 实现解剖

### 2.1 plan 是状态，不是模式开关

`plan/mode` 是整值替换的 log-only 事件；`set()` 记录“待生效选择”，`get()` 返回
`{active, pending?}`（`plan.md`）。由于会话事件必须被 turn 包裹，选择只能在下一次
被接受的 pre-step 落盘；若进程在此之前退出，选择丢失——这一限制被写进包 README
（`plan.md`）。这是“不在循环外偷偷改状态”的代价，也是可重放的保证。

### 2.2 退出是被人复核的

`exit_plan_mode` 在计划模式激活后逐步披露；调用必须提交完整计划，经交互 seam 展示；
批准记录一个“静默待生效退出”，在下一次 pre-step 追加，因此当前工具批剩余部分仍在计划
引导下运行（`plan.md`）。驳回则以**失败的调用**返回用户反馈，模型据此修订重提。

### 2.3 todo 是整表快照

`todo/write` 只承载完整列表，不做增量；条目没有稳定 id，写入即覆盖（`todo.md`）。
设计意图是“模型自述的计划板”，不是可被别人并发修改的任务实体——这与 WHCode 的 SDD 任务
有本质区别：后者有状态机、门禁与审计，前者只有展示与重放。

### 2.4 目标是 CAS 实体

`GoalRef{id, revision}`，每次持久变更递增 revision；阶段回答“目标发生了什么”，进程内
activation 回答“还能不能继续起新一轮”（`goal.md`）。把两者分开，避免“重启后
自动继续跑”的意外。

### 2.5 定时是宿主服务

Schedule 把任务存成独立于会话激活的持久单元，七种计时都以已提交的 UTC 时刻为权威，
解码时**不按当前时区重算**；latest-only 补投只投最近一次到期，然后提交下一个未来时刻
（`schedule.md`）。耗尽的规则保留为 `ended`，不删除。

## 3. WHCode 现状核对

- 已实现：SDD 任务 9 态状态机与阶段门禁（`crates/whcode-domain/src/state.rs`、`gate.rs`），
  任务推进必须过门禁并可审计（`docs/adr/ADR-0**` 与 `specs/` FR-3xx）。
- 缺失：会话内计划模式、todo 工具、目标实体、宿主定时。检索 `plan`、`todo`、`schedule`、
  `goal` 在 `crates/` 仅命中 `plan`（计划文档）与 `schedule`（无），无实现。
- 语义差异：WHCode 的“任务”是**工程交付单元**（有 FR/Check/证据要求）；DSH 的 todo 是
  **模型自述的待办**。两者不能互相替代，也不应合并。

## 4. 差异判定与可借鉴点

1. **计划模式软引导**：可采纳（成本低）。WHCode 的 `turn.rs` 已有系统提示拼装点，加一个
   “计划段”即可；但必须像 DSH 一样写清“不强制”，否则会与门禁语义混淆。
2. **退出需人复核**：可采纳（与 GUI/Web 审批面同批）。`exit_plan_mode` 的模式可映射到
   `Capability`＋一次确认，不必新造审批框架。
3. **`plan/mode` 落事件而非内存**：必须采纳。否则 fork/恢复后计划状态丢失；WHCode 已有
   `events` 表，直接加事件类型即可。代价：事件种类增长，需同步 `check_counts` 与 schema。
4. **todo 整表替换**：可采纳为轻量工具，但**不要**接进 SDD 任务表。若接，必须明确“todo 不产生
   门禁效力”，避免两套任务语义打架。
5. **目标 CAS**：暂不采纳。WHCode 的任务状态机已有版本/检查点语义；再加目标实体属于重复建模。
6. **宿主定时**：不采纳（当前）。常驻定时器与“空闲零占用”的资源铁律冲突，且需要新的持久
   调度表；若做，必须给出空闲 CPU 与唤醒预算（ADR-015 并发档案）。

## 5. 待验证项

1. `plan/mode` 的事件声明是否已在 `persistence-catalog.md` 登记：文档指向该目录，但未核对条目。
   取证：`rg -n "plan/mode" docs/persistence-catalog.md`。
2. DSH `schedule` 的存储介质（JSON 文件还是 SQLite）：`schedule.md` 提到 SQLite 物理 schema
   不变，但未写主存储。取证：读 `packages/schedule/schedule/README.md#storage-format`。
3. WHCode 是否已有“计划”相关 FR（FR-7xx 体验对齐批次）尚未实现：只检索了 `crates/`。
   取证：`rg -n "计划模式|plan mode" specs/001-whcode-harness/spec.md`。

## 6. 证据路径清单

- `docs/subsystems/plan.md`（软引导定义）
- `docs/subsystems/plan.md`（pending 与 pre-step 追加）
- `docs/subsystems/plan.md`（exit_plan_mode 协议）
- `docs/subsystems/todo.md`（TodoItem 三态）
- `docs/subsystems/goal.md`（GoalRef CAS）
- `docs/subsystems/schedule.md`（七种计时）
- `packages/plan/plan-mode/src/index.ts`（实现入口）
- `packages/todo/tool-todo/src/types.ts`（todo 类型）
- `crates/whcode-domain/src/state.rs`（SDD 任务状态机）
- `crates/whcode-domain/src/gate.rs`（阶段门禁）
