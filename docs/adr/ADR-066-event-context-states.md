# ADR-066: 事件三态分类视图——current／shadowed／log-only（P3.2）

- **状态**：已批准（2026-11-12，路线级批准见 ADR-059 全量采纳 proposal-v3；本条为 Phase 2 的 P3.2）
- **日期**：2026-11-12
- **关联**：ADR-059（终态路线全量采纳）、ADR-060（turn 循环与压缩三事件）、ADR-036（JSON 列口径；
  本条**不**改列类型，见编号说明）、ADR-002（store 为唯一 SQL 落点）、ADR-015（并发设计档案——
  本条不需要，见决策 6）；`specs/001-whcode-harness/data-model.md`（events／messages／context_windows）、
  `research/synthesis/proposal-v3.md` P3.2、`research/dsh-comparison/domain-L-persistence-events.md` §2.4、§4.1。
- **编号说明**：proposal-v3 P3.2 行把载体写作“修订 ADR-036”。复核发现 ADR-036 是**JSON 语义列
  一律 `VARIANT`** 的口径，与查询视图无关；本条不新增列、不改列类型，按“无既有承载 → 新立”处理，
  不改动 ADR-036（与 ADR-063 纠正 P3.4 载体编号同一处理）。

## 问题

1. **“还在上下文里吗”无法回答**：压缩折叠后原文仍在 `events`／`messages`，但没有任何标记说明
   哪些历史仍进入模型上下文、哪些只被摘要代表、哪些从不进模型。用户与审计只能靠推测。
2. **纯日志与对话语料混在同一流**：`events` 是本工作区的审计流，其中一部分事件有对话消息孪生
   （`MSG-<seq>`），另一部分（`ContextState`、`AssistantAttempt`、`ProgressChanged` 等）从不进入
   模型上下文；现有导出无法区分。
3. **口径分叉的风险**：若查询层另行实现“什么算被遮蔽”，就会与模型自己的历史派生逻辑分叉——
   同一份数据出现两个真相。

## 决策

1. **三态定义（以本仓库双表事实为准）**：
   - `current`：该事件序号存在对话消息孪生（`MSG-<seq>`），且该消息落在当前模型上下文窗口内；
   - `shadowed`：有孪生但不在窗口内——被压缩摘要代表或预算截断遮蔽，原文仍可查；
   - `log_only`：无孪生——纯审计／观测／控制事件，从不进入模型上下文。
2. **派生而非存储**：不新增列、不写标记表。窗口复用**模型自己的历史派生函数**
   [`crate::context::load_context`](../../crates/whcode-app/src/context.rs)（同一 fold surface，
   DSH 域 L §2.4 的口径原则），孪生由
   [`query::message_seqs_in_range`](../../crates/whcode-store/src/query.rs) 按序号判定。
   分类因此**不会与折叠动作失同步**——折叠改了历史，视图下次读取自然跟着变。
3. **预算基准固定为默认 turn 预算**（`DEFAULT_TURN_BUDGET_TOKENS`）：视图回答“**现在**哪些历史
   还在模型上下文里”，不是对某次 turn 的历史参数复盘；调用方用自定义预算跑过的 turn 不改写视图。
4. **交付面**：`whcode-app::events::EventsExport` 增 `states`（与 `events` 同序、逐条 `seq`＋`state`）
   与 `state_counts`（三值之和等于页内事件数）；CUI `events list`／`events export` 的 JSON 与
   JSONL 头携带同一字段；人类可读行以 `[state]` 标注（数据字段，按既有“数据字段不翻译”口径保留
   英文标识符，不进 i18n）；Web `/api/events` 与 `/api/events/export` 经同一导出函数自动获得。
5. **有界**：按页判定（单页 ≤ [`EXPORT_PAGE_LIMIT`](../../crates/whcode-app/src/events.rs) 1000 条），
   孪生查询按该页 `seq` 的 `[min, max]` 闭区间取集合；窗口本身由预算天然有界。不随会话长度
   线性增长的额外状态。
6. **并发纪律**：同步只读、单查询页、不加线程与后台任务，不需要 ADR-015 并发设计档案。
   代价是每次导出一页多一次消息区间查询与一次上下文装载（均为只读、有界）。
7. **现状登记（不改）**：`context_windows.window_start_seq`／`window_end_seq` 由压缩路径写成
   “压缩发生时本 turn 的事件区间”（`base_seq + 1 .. end_seq`），**不是**被折叠原文的消息区间。
   因此三态**不**据该表推断遮蔽；该字段语义若需修正，另立事项并单独取证（本条不把派生口径
   建在语义未定的列上，也不在本批改写入路径）。

## 正负和解

- **正面**：用户与审计第一次能区分“模型还看得见／只在库里／从不进模型”；查询口径与模型口径同源，
  不会出现两套真相；纯派生，零迁移、零写入、零同步成本；Web 与 CUI 同批获得，四端语义一致。
- **负面**：每次导出一页多一次只读查询与一次上下文装载；`shadowed` 不区分“压缩遮蔽”与“预算
  截断”；视图反映默认预算下的现状，不能回答“某次 turn 当时看见了什么”。
- **承担者**：使用者（理解三态为“现在”的快照，不是历史回放）；应用服务维护者（保持与
  `load_context` 同源，语义变化须同步）；前端维护者（按 `state` 标注，不自行推断）。
- **和解理由**：DSH 域 L 明确把三态列为可采纳的查询视图字段，而“哪段历史还在上下文里”是
  压缩机制引入后必然产生的用户问题；派生方案以既有函数为唯一口径，成本是每页一次有界只读查询，
  不需要新的持久化真相。

## 三重时间

- **过去来源**：DSH 的 `session-query` 三态分类与其 `foldSurface()` 复用原则（域 L §2.4）；
  proposal-v3 P3.2 的“事件三态分类视图”条目；本仓库既有事实：`append_turn` 双表共用全局序号、
  `load_context` 是模型历史的唯一派生入口、`context_windows` 为压缩指针表。
- **现在证据**：`crates/whcode-store/src/query.rs::message_seqs_in_range`（1 项测试）；
  `crates/whcode-app/src/events.rs` 的三态与两项测试（正反例分类＋空会话与“不写入”断言）；
  `crates/whcode-bin/src/cui_dispatch.rs` 人类行标注测试；Check 378 取证记录。
- **将来失效条件与复审触发**（绑定命令＋判定规则，ADR-046 口径）：
  1. `cargo test -p whcode-app events` 或 `cargo test -p whcode-store message_seqs` 不再全绿 →
     三态判定与孪生口径复审；
  2. `rg -n "MSG-" crates/whcode-app/src/events.rs` 不再命中（消息 id 形态或双表共用序号机制
     变化）→ 决策 1／2 失效，须改用显式关联列；
  3. `rg -n "fn load_context" crates/whcode-app/src/context.rs` 语义变化（例如引入多窗口或
     游标式上下文）→ 窗口基准复审；
  4. `python .github/scripts/check_counts.py` 计数漂移或 missing>0 → 台账复核；
  5. 引入全文检索（P3.3）或对话剪枝（P3.5）→ 三态须与新视图同源，本 ADR 修订；
  6. 2027-05-07（与 D15 覆盖率复审同批）→ 复查默认预算基准与页面判定是否匹配真实会话规模。

## 已知限制

- **只看默认预算**：自定义 `budget_tokens` 的 turn 不被回溯；视图不回答“那次运行时模型看见了什么”。
- **不区分遮蔽原因**：`shadowed` 同时涵盖压缩摘要代表与预算截断，不细分为两个状态。
- **只判定当页**：分类作用于导出的一页事件；跨页一致性由 `seq` 单调性保证，不做全量重算。
- **依赖 id 形态**：孪生判定依赖 `MSG-<n>` 约定；外部夹具写入的不规范 id 不计为孪生（视为
  `log_only`），与 `append_turn` 的序号口径一致。
- **不做全文检索**：本视图只回答上下文状态，不提供正文检索（P3.3 另立）。
- **不改 `context_windows` 语义**：该表的窗口边界仍是压缩发生时的事件区间；据它推断遮蔽会得到
  错误答案，决策 7 已登记，修正须另立事项。
- **门禁只校验结构**：本 ADR 的“已知限制／失效条件”节由 `check-adr-limits.py` 检查是否存在，
  不校验内容是否充分。

## 实施记录

- 2026-11-12：G1 入账（FR-770／T283／Check 378）；store 增 `message_seqs_in_range`（区间孪生集合，
  `TRY_CAST` 容错非规范 id）；app `events` 增 `EventContextState`／`EventState`／`EventStateCounts`
  与派生分类，`EventsExport` 携带 `states` 与 `state_counts`；CUI JSON／JSONL 头与人类行标注；
  Web 经同一导出自动获得。测试：store 新增 1 项、app 新增 2 项（分类正反例；空会话＋三表行数
  不变证明零写入）、bin 新增 1 项（有标注显示 `[state]`、无标注不伪造）。

## 修订记录

- 2026-11-12（首版）：P3.2 落账；三态定义、派生口径、预算基准、交付面、有界性与
  `context_windows` 现状登记成文；载体编号按复核结果新立（不改 ADR-036）。
