# ADR-065: 会话分叉——turn 边界、双表前缀复制与幂等谱系（P3.1）

- **状态**：已批准（2026-11-12，路线级批准见 ADR-059 全量采纳 proposal-v3；本条为 Phase 2 的 P3.1）
- **日期**：2026-11-12
- **关联**：ADR-059（终态路线全量采纳）、ADR-002（store 为唯一写者与 SQL 边界）、
  ADR-036（JSON 列口径，本 ADR 无新 JSON 列）、ADR-015（并发设计档案——本 ADR 不需要，
  见决策 8）；`specs/001-whcode-harness/data-model.md`（sessions/events/messages 表）、
  `research/synthesis/proposal-v3.md` P3.1、`research/dsh-comparison/domain-D-session-fork-resume.md`。

## 问题

1. **结构性缺失**：DSH 有 `SessionStore.fork(source, boundary)`（按 seq 边界复制前缀、
   `inheritedEventCount` 落库、拒绝未闭合 turn），WHCode 检索 `fork`/`rewind` 在 `crates/`
   无任何命中——用户想从某个已完成 turn 试着走另一条路时，只能整会话重来或手工搬历史。
2. **历史跨双表**：`append_turn` 把 turn 的事件与消息写入 `events`／`messages` 两表，且共用
   一条全局递增序号（`EV-<seq>`／`MSG-<seq>` 同源）。只复制事件会让子会话的消息面残缺
   （边界轮的最终助手回复落在 `TurnFinished` 之后），只复制消息则丢审计流。
3. **DSH 的谱系靠标记反推**，WHCode 是关系库——谱系应当是行的列，不是约定。

## 决策

1. **谱系落库（schema v10）**：`sessions` 增加 `parent_id`／`forked_at_seq`／
   `inherited_event_count` 三列（可空；DuckDB 不支持带约束的 ADD COLUMN，父会话存在性由
   应用层校验——沿 `0005_session_stage.sql` 先例）；`events` 增加 `origin_event_id` 并建
   唯一索引 `idx_events_origin(session_id, origin_event_id)` 作为复制幂等键。该索引进
   [`REQUIRED_INDEXES`](../../crates/whcode-store/src/indexes.rs)，缺失即打开校验失败。
2. **边界＝turn 结束**：`at_seq = 0` 表示空分叉；`at_seq > 0` 时该 seq 的事件必须是
   `TurnFinished`，否则 `InvalidState`——半截 turn 不能成为新会话的起点。序号越界、
   源会话不存在（`not_found`）同样拒绝。
3. **复制范围＝事件前缀＋边界轮尾随消息**：事件复制 `seq <= at_seq`；消息复制
   `MSG-<n>` 序号小于“源会话中下一个事件的 seq”的全部行（边界是最后一个 turn 时取全部
   剩余消息；空分叉不复制消息）。依据：`append_turn` 先写事件后写消息、`TurnStarted`
   单独提交且是每轮第一行（ADR-060 项 G），所以“下一事件之前”恰好收进边界轮的尾随
   消息、又不把下一轮写在 `TurnStarted` 之后的用户消息混进来。id 不符合 `MSG-<n>` 形态
   的消息（外部夹具）不参与计数与复制，与序号口径一致。
4. **序号基线取双表最大**：新序号从 `GREATEST(全局事件最大 seq, 全局消息最大序号)` 之后
   分配（与 `append_turn` 同口径），保持“序号即物理先后”的全局不变量；否则分叉后再
   追加 turn 会破坏未来分叉边界判定的前提。
5. **有界**：继承事件数与继承消息数各自不超过
   [`MAX_FORK_EVENTS`](../../crates/whcode-store/src/fork.rs)（2000）；超限拒绝而非截断，
   避免新会话静默丢失历史。资源铁律（有界缓冲、不随会话数线性增长）由此满足。
6. **单事务、源不动**：会话行、事件与消息在同一事务内提交，失败整体回滚；复制事件的
   `payload_json`／`ts`／`persistence_level` 与消息的正文／`ts`／`parent_id` 原样保留，
   只有 `seq`／`id`／`session_id` 重新分配。消息不带 origin 键——复制只发生在单事务内，
   源身份由事件的 `origin_event_id` 链间接可溯。
7. **权限与标题默认值在应用层**：`whcode-app` 以 `Capability::SessionCreate` 判定后调用
   store 原语；标题缺省取“{源标题} (fork)”。本批交付 app 命令＋CUI `session fork`；
   TUI/Web 预留后续批次，GUI 按 FR-711 只读 v1 排除（写命令）。tasks／checkpoints 不随
   分叉复制（任务工作流状态不属对话历史），子会话继承源会话的 model 与 stage。
8. **并发纪律**：同步单事务、不新增线程、不加后台任务，不需要 ADR-015 并发设计档案；
   代价是分叉期间持写事务（大前缀复制阻塞同库其他写者——本地单用户场景可接受）。
9. **导出/导入**：导出按 `duckdb_columns()` 动态取列，v10 谱系列自动随导出文件往返并有
   回归测试。兼容边界：v9 及更早的导出文件导入 v10+ 目标会被列集合校验拒绝（既有严格
   口径），恢复路径是在旧版本导入后再升级。

## 正负和解

- **正面**：用户第一次可以在已完成 turn 的边界把完整对话历史（含最后一条助手回复）带到
  新会话试另一条路；谱系与幂等键是行的列而非约定；源会话绝对不动；有界拒绝而非截断。
- **负面**：schema 又升一版（v10），旧导出文件不能再导入新库（列集合校验）；分叉复制
  最多 2000 事件＋2000 消息的写事务，期间同库其他写被阻塞；子会话不带 tasks/checkpoints，
  从分叉点重新积累工作流状态。
- **承担者**：使用者（选择边界、承担复制时长）；存储维护者（双表复制与序号基线口径）；
  装配根维护者（app 权限与标题默认值）。
- **和解理由**：分叉是 P3.5（回滚双路径）的前置，proposal 明确列为本阶段项；正确性
  （历史完整）优先于复制时长，正确性靠“双表＋序号不变量”两个既有事实保证，不引入
  新机制。

## 三重时间

- **过去来源**：DSH 域 D（fork API、边界拒绝未闭合 turn、`inheritedEventCount`、
  `buildForkSeed` 尾随种子）、proposal-v3 P3.1 设计要点（关系库外键谱系、边界对齐
  `turn/end`、幂等键重放、常量上限）；本仓库事实：`append_turn` 双表共用计数器、
  `TurnStarted` 单独提交（ADR-060 项 G）、`0005_session_stage.sql` 的可空加列先例。
- **现在证据**：`crates/whcode-store/src/fork.rs`（4 项测试：边界轮尾随消息与源不动、
  半截 turn／缺失源／越界／空标题拒绝、空分叉与双上限、共用计数器不变量与分叉后
  `append_turn` 兼容）；`crates/whcode-store/src/export.rs` 谱系列往返测试；
  `crates/whcode-store/src/indexes.rs` 索引注册；Check 377 取证记录。
- **将来失效条件与复审触发**（绑定命令＋判定规则，ADR-046 口径）：
  1. `cargo test -p whcode-store fork` 或 `cargo test -p whcode-store export` 不再全绿 →
     复制范围与序号基线口径复审；
  2. `rg -n "GREATEST" crates/whcode-store/src/turn.rs crates/whcode-store/src/fork.rs`
     不再命中双源取最大口径（events/messages 序号机制变化）→ 决策 3／4 失效，复审
     消息选择规则；
  3. `rg -n "MAX_TURN_MESSAGE_BYTES" crates/whcode-store/src/turn.rs` 语义变化（消息正文
     改走内容表／引用，R-05 落地）→ `content_ref` 复制口径复审；
  4. `python .github/scripts/check_counts.py` 计数漂移或 missing>0 → 台账复核；
  5. TUI/Web 分叉入口交付、或 GUI 退出只读 v1 → 本 ADR 修订登记面；
  6. 2027-05-07（与 D15 覆盖率复审同批）→ 复查上限常量与拒绝策略是否匹配真实会话规模。

## 已知限制

- **不复制 tasks／checkpoints**：子会话的任务清单与检查点从空开始；需要沿用工作流状态时
  用户应在源会话继续工作。
- **旧导出文件不能直入 v10+**：列集合严格校验拒绝列缺失的导出文件；先在旧版本导入再升级，
  或使用迁移前备份（ADR-063）回退整体库文件。
- **无跨会话去重**：幂等键只约束“同一目标会话不重复复制同一源事件”；对同一源会话多次
  分叉会产生多个各自独立的子会话（语义如此，非缺陷）。
- **消息幂等靠事务不靠键**：`messages` 无 origin 列，增量修复路径若绕过单事务直接补复制，
  唯一索引不拦截（决策 6 的边界）。
- **前缀截断不静默**：超限即整体拒绝，用户必须选更早边界；没有“部分分叉”模式。
- **门禁只校验结构**：本 ADR 的“已知限制／失效条件”节由 `check-adr-limits.py` 检查是否
  存在，不校验内容是否充分。

## 实施记录

- 2026-11-12：G1 入账（FR-769／T282／Check 377）；`whcode-store::fork`（边界校验＋双表
  前缀复制＋共用序号基线＋双上限＋单事务）；schema v10（`0010_session_fork.sql`）；
  `idx_events_origin` 入必需索引注册表；`whcode-app` 分叉命令与 CUI `session fork`
  （十三套文案同批）；导出/导入谱系列往返测试。测试：store 137 项全绿（含 fork 4 项、
  往返 1 项、索引注册既有动态校验）。

## 修订记录

- 2026-11-12（首版）：P3.1 落账；边界语义、双表复制范围、序号基线、幂等键、有界拒绝、
  权限与标题归属、导出兼容边界成文。
