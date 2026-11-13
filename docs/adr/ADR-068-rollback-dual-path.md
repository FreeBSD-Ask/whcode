# ADR-068：回滚双路径（对话剪枝＋文件前像）

- **状态**：已批准（2026-11-13，proposal-v3 P3.5／FR-775／FR-776／T288／T289）
- **来源**：`research/synthesis/proposal-v3.md` P3.5、`research/dsh-comparison/domain-D-session-fork-resume.md`
  §1／§4.4（对话回滚与文件前像都是缺失项；前像须先定保留策略）、ADR-065（分叉边界口径）
- **责任人**：turn 循环维护者、store 维护者、前端维护者
- **退出条件**：需要跨会话或跨分支的三方合并式回滚（CRDT／版本图）时，事件前缀模型不够，
  必须另立 ADR；需要“回滚到任意事件中间态”时，本文的 turn 边界约束必须废止

## 背景

会话分叉（ADR-065）解决“从某个历史点另开一支”，但不解决两件日常动作：**清掉最近的错误
对话继续在同一会话里工作**（对话剪枝），以及**把被工具改坏的文件恢复原样**（文件回滚）。
DSH 只有分叉、没有文件前像；WHCode 有任务检查点，但它属于 SDD 状态机，不覆盖会话对话与
工作区文件。两条能力都必须在不破坏“事件只增不改”与资源有界两个既有约束下实现。

## 决策

### A. 对话剪枝（FR-775／T288）

1. **不删事件，只落边界**：新增 `EventKind::SessionRewound`（log-only、Critical），载荷
   `{target_seq, requested_by}`。`target_seq` 必须是 `0` 或本会话一条 `TurnFinished` 事件的
   `seq`（与 ADR-065 的分叉边界同口径：**不接受半截 turn**）；`target_seq` 必须小于当前最大
   `seq`（回退到末尾是无操作，显式拒绝）。
2. **派生规则（唯一口径）**：设最新一条 `SessionRewound` 为 `(target, rewind_seq)`，则历史中
   序号 `s` 的消息/事件**有效**当且仅当 `s <= target || s > rewind_seq`。多次回退以最新一条
   为准；回退后新增的 turn 自然落在有效区。规则实现在应用层（`context::load_context`、
   `llm::conversation_tail`），**不改表结构**。
3. **只影响派生，不影响事实**：事件与消息仍全部可查（`events list`／导出／三态视图），
   审计可复原任意时刻的完整历史；模型上下文与对话尾只呈现有效区。
4. **权限与通道**：能力 `session.rewind`（写类）；CUI `session rewind --session --to-seq`、
   Web `POST /api/rewind`；读面 `GET /api/rewind`（当前边界与最新回退序号）。

### B. 文件前像（FR-776／T289）

5. **前像在写之前抓、成功才登记**：turn 在执行 `write_file`／`edit_file` 之前读取目标文件
   原内容（存在、常规文件、≤ [`MAX_PREIMAGE_BYTES`]），仅在该调用**成功**后写入
   `file_preimages`（schema v11）；失败／被拒不留前像（登记不存在的“改前状态”即伪造事实）。
   非 UTF-8 或超限文件**显式跳过**并记录原因码，不静默丢弃。
6. **有界保留**：每会话最多 [`MAX_PREIMAGES_PER_SESSION`] 条、合计 ≤
   [`MAX_PREIMAGE_TOTAL_BYTES`]；插入时按创建顺序裁剪最旧。资源铁律（ADR-025／NFR-201）
   不被破坏：前像是小文本快照，不是工作区镜像。
7. **恢复是破坏族**：`session restore --session --exec-id`（或 `--seq` 取该序号前最近一次
   写调用）默认 dry-run 列出将恢复的路径与字节数，`--yes` 执行；执行前写
   `AuditAction::PreimageRestore` 审计，失败即不执行（沿用“审计失败阻断”口径）。恢复只写
   工作区内路径（`resolve_in_workspace` 同一口径），缺失目录按需创建；不回滚 `tools` 之外
   的副作用（例如外部进程）。
8. **展示面**：Web 提供只读 `GET /api/preimages?session=`（路径、字节数、来源 `exec_id`）；
   恢复写面 v1 只在 CUI（GUI／TUI／Web 写面未排期，矩阵如实登记）。

## 正负和解

- **正面**：用户不必为一次错误回答新建会话；被模型改坏的文件有可核对的改前内容；两条路径都
  不删事实，审计与导出仍完整。
- **负面**：回退后模型“忘记”被剪掉的对话（这正是目的，但用户可能误解为丢数据——文档与 CUI
  文案必须说清）；前像只覆盖 `write_file`／`edit_file`，`shell`（未立项）等未来写路径需另行
  接入；前像对大文件不生效（超限跳过）。
- **承担者**：使用者（理解“剪枝不是删除”）、turn 维护者（抓取时机与成功条件）、store 维护者
  （裁剪与迁移）、前端维护者（Web 写面未排期）。
- **和解理由**：以“事件只增不改＋派生过滤”换取可回退且可审计；以“有界小文本前像”换取资源
  铁律下的可证恢复能力，不引入工作区镜像或外部快照工具。

## 三重时间

- **过去来源**：DSH 域 D（分叉边界拒绝半截 turn）；ADR-065（`TurnFinished` 边界）；ADR-046
  （证据态）；ADR-025（资源预算）；ADR-036（JSON／存储类型口径）。
- **现在证据**：`crates/whcode-store/src/preimage.rs`、`migrations/0011_file_preimages.sql`、
  `crates/whcode-app/src/rewind.rs`／`preimage.rs`、`turn.rs` 的抓取与派生过滤、Check 383／384。
- **将来失效条件**：出现第二条写文件路径（shell／PTY／外部编辑器）→ 前像抓取必须扩展或明确
  豁免；需要“按事件逐条回退”而非 turn 边界 → 剪枝规则废止；前像总量贴近上限 → 复审保留策略。

## 复审触发（绑定命令＋判定）

1. `cargo test -p whcode-app -- rewind` 或 `cargo test -p whcode-store -- preimage` 不再全绿
   → 语义复核；
2. `rg -n "SessionRewound" crates/whcode-app/src/context.rs` 不再命中 → 派生过滤被拆掉
   （回退变成“只写事件不生效”）；
3. 新增写工作区的工具而未接入前像 → 必须在本 ADR 决策 5 里补登记或写明豁免理由；
4. `python .github/scripts/benchmarks/measure-resources.py` 显示 RSS 或库体积显著增长 →
   重估前像上限；
5. 2027-05-07（与 D15 复审同批）→ 复查前像上限（55 条／8 MiB）与回退粒度是否匹配真实用法。

## 已知限制

- 回退只到 turn 边界（`TurnFinished`）；回退到半截 turn 被拒绝，不静默裁剪。
- 剪枝不删除事件：数据库体积不会因回退变小，需要压缩请用既有的 `db purge`／归档路径。
- 文件前像只覆盖 `write_file`／`edit_file`，且单文件 ≤256 KiB；超限与非 UTF-8 显式跳过并记录。
- 恢复不回滚对话（两条路径独立）；Web／GUI／TUI 的写面恢复未排期。
