# ADR-018：whcode-app 经 whcode-store 公共 API 访问存储

- **状态**：已批准（2026-10-22，M2 实施前置）
- **来源**：M2 任务（tasks.md T046—T052）实施时发现的基线歧义：AGENTS.md §4 与
  `docs/guides/module-design.md` 的分层图写有"whcode-app 不依赖前端与存储实现"，而
  可执行门禁 `tests/contract/test_layering.rs` 只禁止**前端**依赖 `whcode-store`
  （C-03、ADR-003/005）。两条口径必须在不引入虚假抽象的前提下统一。
- **责任人**：应用服务层负责人（whcode-app/whcode-store 边界）
- **退出条件**：若出现第二持久化后端提案（违反 ADR-001 的现状），或门禁需要收紧
  app→store 的依赖面，本 ADR 必须复审并可能被修订取代。

## 决策

1. `whcode-app` **直接依赖 `whcode-store` 的公共 API**（Cargo 依赖，经门禁）；
2. **SQL 只存在于 `whcode-store`**：app 内不得出现 SQL 字符串、`duckdb::Connection`
   的打开/管理，也不得绕过 store 的写者纪律（ADR-002）；
3. 分层门禁维持现状：可执行约束仍为"前端不得依赖 store"；app→store 由本 ADR 与
   代码评审约束，暂不新增门禁断言（避免把尚未形成共识的细节固化为门禁）；
4. "不依赖存储实现"的准确含义收敛为：**app 不内嵌存储实现（SQL/连接管理）**，
   而非"app 与存储类型零耦合"。

## 理由

- ADR-001 规定 DuckDB 是**唯一**默认持久化后端，未经 ADR 不得增加第二后端。
  端口-适配器（repository trait）模式解决的矛盾——"可替换存储后端"——在本项目
  被基线明确排除；为其付出双倍接口面与装配胶水属无真实矛盾的形式主义（宪法 §I）。
- 若改为 trait 方案：store 实现 app 的端口会引入 store→app 的**反向**依赖（更糟），
  或把仓储胶水堆进装配根 `whcode-bin`（违背其薄装配定位）。
- app→store 单向依赖保持依赖图无环：domain ← platform ← store ← app ← 前端 ← bin。
  业务规则与权限判定仍只在 app（store"不负责业务规则、权限判定"的分工不变）。

## 替代方案（已否决）

- **端口 + 适配器**：见上，无真实矛盾；且引入 store↔app 依赖方向问题。
- **app 直接内嵌 SQL**：违反 ADR-002（store 是唯一 DuckDB 链接点），权限与事务
  纪律将散落。
- **维持字面约束、把 M2 命令放进 store**：把命令编排/权限塞进存储层，破坏
  module-design 对 store"不负责业务规则"的定义。

## 影响

- `crates/whcode-app/Cargo.toml` 新增 `whcode-store` 依赖（本 ADR 即依据）；
- `AGENTS.md` §4 分层图与 `module-design.md` 分层图按第 4 条口径同步修订；
- M2 会话/任务命令所需的最小存储原语（会话目录、任务投影）落在
  `whcode-store::directory` / `whcode-store::tasks`，随 T046—T052 同 PR 交付；
- 前端四 crate 的依赖面**不变**：仍然只允许依赖 `whcode-app`。
