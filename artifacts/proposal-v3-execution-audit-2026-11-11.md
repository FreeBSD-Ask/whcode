# 终态功能建议书 v3 执行态比对（2026-11-11）

- **性质**：执行态比对记录（0 信任口径），对照
  [`research/synthesis/proposal-v3.md`](../research/synthesis/proposal-v3.md) 全部 45 条提案
  与当前实现；不是验收结论（ADR-037：通过以 CI 日志与 artifact 为准）。承接
  [progress-review-2026-11-09.md](progress-review-2026-11-09.md)。
- **基线**：`main` `40dc6ed`；工作树另有未提交改动（见 F3）。

## 0. 任务认知卡（宪法原则 X）

- **方向**：用户指令“继续比较功能实施书V3，不用子代理”——把 45 条提案逐条与磁盘事实对上，
  形成可判定的执行态台账。
- **目标**：每条提案有四值状态（已实现／部分／在途／未实现）与证据锚点；缺口显式登记。
- **判据**：①每条有 `file:line` 或可复现命令；②负面结论必须给出检索命令与“无命中”事实；
  ③不重排路线优先级，下一步只给建议不代决策。
- **非目标**：不改 `specs/` 与 ADR；不在本批实施任何提案；不重读上游快照（复审触发未到）。

## 1. 方法与可复现命令

```powershell
git -C D:\WHCode log --oneline -8
git -C D:\WHCode status --short
python .github/scripts/check_counts.py
rg -n "pub enum ApprovalOutcome" crates/whcode-domain/src/approval.rs
rg -n "timeout_ms|output_schema|fn render" crates/whcode-tools/src/registry.rs
rg -n "AssistantAttempt" crates/whcode-domain/src/event.rs
rg -n "usage_anchor" crates/whcode-store/src
rg -n -i "steer|interject" crates                        # P1.8 负证据
rg -n "findings|learning|shadowed|log_only" crates --glob "!**/*_test*"
rg -n "PlanMode|Deliverable|Steer" crates/whcode-domain/src/event.rs
rg -n "pub struct ProviderProfile" -A 30 crates/whcode-llm/src/http/profile.rs   # 无 models[]
rg -n -i "sandbox|webhook" crates                        # P5.2／P6.4 负证据
```

## 2. 计数结论

| 状态 | 条数 | 条目 |
|---|---|---|
| 已实现 | 22 | P1.1—P1.7、P2.1、P2.3、P3.1、P3.2、P3.4、P3.6、P4.1—P4.4、P4.6、P7.1、P7.4、P7.5、P7.6 |
| 部分 | 2 | P3.3（仅标题／ID 匹配）、P7.2（夹具与手动 job 在，PR 门禁与三指标中位缺） |
| 在途 | 0 | （P3.1 已于 2026-11-12 收口，见 W3 表） |
| 未实现 | 21 | P1.8—P1.10、P2.2、P2.4、P2.5、P3.5、P4.5、P4.7、P4.8、P5.1—P5.6、P6.1—P6.4、P7.3 |

账本实测（`check_counts.py`，本文件写作时点）：FR=146、任务=281、Check=376；
exercised=327＋outcome-supported=49＝376，missing=0，计数一致。
2026-11-12 复核（P3.1 收口后）：FR=147、任务=282、Check=377；exercised=328＋
outcome-supported=49＝377，missing=0，计数一致。
2026-11-12 第二次复核（P3.2 收口后）：FR=148、任务=283、Check=378；exercised=329＋
outcome-supported=49＝378，missing=0，计数一致。
2026-11-12 第三次复核（P4.6 收口后）：FR=149、任务=284、Check=379；exercised=330＋
outcome-supported=49＝379，missing=0，计数一致。

## 3. 逐线比对

### W1 执行层（10 条）

| 编号 | 状态 | 证据 |
|---|---|---|
| P1.1 四态审批 | 已实现 | FR-754／T264／Check 359；类型 `crates/whcode-domain/src/approval.rs:27`，broker 与策略 `crates/whcode-app/src/approval.rs:215,285,417`，turn 接线 `crates/whcode-app/src/turn.rs:335`；GUI 卡（T256）与 Web 卡（T257）见 Check 348／350 |
| P1.2 模型摘要 | 已实现 | FR-755／T265／Check 360；`crates/whcode-app/src/compaction.rs:28,55`（purpose 与 request_id 派生） |
| P1.3 失败尝试持久化 | 已实现 | FR-756／T266／Check 361；`crates/whcode-domain/src/event.rs:46`（`AssistantAttempt`）、`crates/whcode-app/src/turn.rs:846` |
| P1.4 单工具超时 | 已实现 | FR-757／T267／Check 362；`crates/whcode-tools/src/registry.rs:76,130`（声明与截止时间） |
| P1.5 usage 锚点 | 已实现 | FR-758／T268／Check 363；`crates/whcode-store/src/usage_anchor.rs:35,88,121`；预算校准 `crates/whcode-app/src/context.rs:287` |
| P1.6 工具输出契约 | 已实现 | FR-759／T269／Check 364；`crates/whcode-tools/src/registry.rs:81,331`（schema 与 render 投影） |
| P1.7 turn 边界先落 | 已实现 | FR-760／T270／Check 365；`crates/whcode-app/src/turn.rs:563`（`TurnStarted` 单独提交） |
| P1.8 steer/inject | 未实现 | `rg -n -i "steer|interject" crates` 无命中；ADR-015 并发档案未交 |
| P1.9 skill 工具 | 未实现 | 内置工具仅 6 个（`docs/guides/tool-catalogue.md`：read_file／write_file／edit_file／list_dir／glob／grep）；技能仍是扫描＋注入（`crates/whcode-app/src/skills.rs:31`），模型无法按名取正文 |
| P1.10 重复调用提醒 | 未实现 | `rg -n -i "duplicate|repeat" crates/whcode-app crates/whcode-domain` 仅命中测试夹具与无关串，无事件形态 |

### W2 评审层（5 条）

| 编号 | 状态 | 证据 |
|---|---|---|
| P2.1 证据态映射 | 已实现 | FR-761／T271／Check 366；`docs/guides/evidence-states.md`（七值表＋升级判据＋拒绝评分制） |
| P2.2 findings 表 | 未实现 | `rg -n "findings" crates` 无命中；无新 ADR |
| P2.3 评审协议 | 已实现 | T272／Check 367；`docs/guides/review-protocol.md`（三域独立／调和冻结／质量门／硬上限 3） |
| P2.4 学习捕获台账 | 未实现 | `rg -n -i "learning" crates` 无命中；无新 ADR |
| P2.5 可观测性门→Check 论证语言 | 未实现 | `rg -n "六道门|verify loop|四平面" docs/guides docs/adr` 无命中 |

### W3 会话与历史（6 条）

| 编号 | 状态 | 证据 |
|---|---|---|
| P3.1 会话分叉 | 已实现（待验收） | 2026-11-12 收口：ADR-065／FR-769／T282／Check 377 入账；`crates/whcode-store/src/fork.rs`（边界＋双表前缀复制＋共用序号基线＋双上限，4 测试）、`migrations/0010_session_fork.sql`（v10）、`export.rs` 谱系列往返、app `session.rs::fork_session`＋surface `session.fork`、CUI `session fork`＋bin 派发端到端、十三套文案同批；全量 1124/0，CI 待运行（ADR-037） |
| P3.2 事件三态视图 | 已实现（待验收） | 2026-11-12 收口：ADR-066／FR-770／T283／Check 378 入账；`crates/whcode-store/src/query.rs::message_seqs_in_range`（孪生判定）＋`crates/whcode-app/src/events.rs` 三态与计数（复用 `load_context` 同源派生、零写入）＋CUI JSON/JSONL/人类行标注＋Web `/api/events` 经同一导出自带；载体按复核结果新立 ADR-066（不改 ADR-036，`context_windows` 语义局限已登记） |
| P3.3 全文检索 | 部分 | 目录级关键词匹配已有（`crates/whcode-app/src/session.rs:69`：匹配标题或 ID、128 字符上限）；正文级关键词与 FTS 无 |
| P3.4 备份/恢复闭环 | 已实现 | FR-767／T280／Check 375；`crates/whcode-store/src/backup.rs:34,114,196,236,274`（保留 5 份／复制／迁移前挂钩／SHA-256 校验／裁剪） |
| P3.5 回滚双路径 | 未实现 | 无对话剪枝与文件前像；`rg -n -i "rewind|prune" crates` 仅命中备份裁剪与回退注释 |
| P3.6 分享脱敏导出 | 已实现 | FR-752／T261／Check 357；`crates/whcode-app/src/share.rs:65,117,235`（导出／canonical 化／SHA-256） |

### W4 体验层（8 条）

| 编号 | 状态 | 证据 |
|---|---|---|
| P4.1 B2 收口 | 已实现 | T253／T254（`specs/001-whcode-harness/tasks.md:807,819`） |
| P4.2 B3 收口 | 已实现 | T255—T257（`tasks.md:832,848,866`）；底座即 P1.1 四态类型 |
| P4.3 B4 收口 | 已实现 | T258／T259（`tasks.md:881,892`） |
| P4.4 B5 收口 | 已实现 | T260—T263（`tasks.md:905,919,934,942`） |
| P4.5 计划模式 | 未实现 | `rg -n "PlanMode|plan_mode" crates/whcode-domain/src/event.rs` 无命中 |
| P4.6 会话 todo 工具 | 已实现（待验收） | 2026-11-12 收口（`c4328d2`）：ADR-048 §7.1／FR-771／T284／Check 379；`whcode-tools/src/todo.rs`（`todo_write` 整表替换、三态、50×200 上限）＋`ParamType::StringArray`＋`SideEffectScope::SessionState`＋`EventKind::TodoUpdated`（log-only、Critical）＋turn 成功调用落快照＋`app::todo::current` 读取面＋工具目录 7 件；与 `tasks` 硬隔离 |
| P4.7 交付物记录 | 未实现 | `rg -n "Deliverable" crates/whcode-domain/src/event.rs` 无命中 |
| P4.8 模型目录 | 未实现 | `ProviderProfile` 为单模型字段（`crates/whcode-llm/src/http/profile.rs:411,425`），无 `models[]`、选择器与 badge |

### W5 平台与执行环境（6 条）——全部未实现

| 编号 | 状态 | 证据 |
|---|---|---|
| P5.1 shell 工具 | 未实现 | 工具目录无 shell；`crates/whcode-app/src/approval.rs:474` 只预注册了名为 shell 的默认 ask 规则（占位，非工具）；威胁模型未交 |
| P5.2 沙箱强度上报 | 未实现 | `rg -n -i "sandbox" crates` 无命中 |
| P5.3 LSP 四查询 | 未实现 | 无 `whcode-lsp` crate；`rg -n -i "\blsp\b" crates` 无命中 |
| P5.4 受控 PTY | 未实现（D13 冻结） | NFR-211 修订未落 `specs/`（ADR-059 决策 2-D13：修订落地前不得开工） |
| P5.5 jobs 运行时 | 未实现（D13 冻结） | 同上；`rg -n "job_" crates` 无命中 |
| P5.6 运行时默认启用 | 未实现 | `crates/whcode-bin/Cargo.toml:59-62`：runtime feature 仍为空实现、默认关闭 |

### W6 生态与互操作（4 条）——全部未实现

| 编号 | 状态 | 证据 |
|---|---|---|
| P6.1 SDK JSON-RPC | 未实现 | 无 `whcode-sdk`；CUI JSON 信封未对外承诺协议版本 |
| P6.2 MCP resource/instructions | 未实现 | `crates/whcode-mcp/src/protocol.rs:213` 只解析 `serverInfo`；无 resources/instructions 限长与权限模型 |
| P6.3 ACP 服务端 | 未实现 | `rg -n -i "\bacp\b" crates` 无命中 |
| P6.4 webhook 幂等投递 | 未实现 | `rg -n -i "webhook" crates` 无命中 |

### W7 治理与工程（6 条）

| 编号 | 状态 | 证据 |
|---|---|---|
| P7.1 覆盖率试点 | 已实现 | FR-762／T273／Check 368；`.github/scripts/check-coverage.py`（app 90.35%／store 93.56%，豁免清单为空） |
| P7.2 基准进 PR 门禁 | 部分 | 测量夹具与 `perf-budget` job 在（`.github/workflows/platform-verification.yml:1191`），但仅 workflow_dispatch 触发；提案的三指标（turn 首 token／会话切换／历史页）与多轮取中位未建 |
| P7.3 内存压力通道 | 未实现 | `measure-resources.py` 仅 session_switch 场景；无压力观察通道 |
| P7.4 工具目录 | 已实现 | FR-768／T281／Check 376；`crates/whcode-tools/src/catalogue.rs:35,48` |
| P7.5 ADR 已知限制节 | 已实现 | FR-763／T274／Check 369；`.github/scripts/check-adr-limits.py` |
| P7.6 真 API 冒烟 | 已实现 | FR-764／T275／Check 370；`.github/scripts/smoke-sensenova.py` |

## 4. 关键发现

| # | 发现 | 事实 | 处置 |
|---|---|---|---|
| F1 | 账本全绿 | `check_counts.py`：FR=146／任务=281／Check=376，missing=0 | Phase 0 与 Phase 1（T264—T275）收口态成立 |
| F2 | proposal-v3 自述过时 | §3“进行中：T251…B2—B5”与 §6 Phase 0 均已完成；§4 表内 P1.1—P1.7 等已实现条目仍以“尚未存在”口径列出 | 在 §11 追加修订记录并指向本文件；§3 可补一行当日快照 |
| F3 | P3.1 在途但未入账 | 工作树未提交 `migrations/0010_session_fork.sql`＋schema v10；引用的 T282／FR-769／ADR-065 不存在于账本 | **已清偿（2026-11-12）**：ADR-065／FR-769／T282／Check 377 先于代码提交入账（G1），随后 WIP 落地收口，见 W3 表 P3.1 行 |
| F4 | project-status.md 快照滞后 | 2026-11-08 页写“任务未完成 5／missing 6／B4／B5 余项”，现均已收口 | 下次快照刷新统一改；本文件只登记不代改 |
| F5 | W5/W6 全线未动 | 23 条未实现中 10 条属 W5/W6；P5.4/P5.5 仍被 D13 冻结 | 与 proposal §6 依赖序一致，无漂移 |
| F6 | Phase 2 可开工集明确 | 除在途 P3.1 外，P3.2／P4.5—P4.8／P5.1（前置 P1.1 已满足）／P6.1（依赖 P1.3 已满足）／P7.2 均无未满足前置 | **2026-11-12 更新**：P3.1、P3.2、P4.6 已收口；推荐序改为 P4.7 交付物记录（中）→ P5.1（先威胁模型）→ P6.1（先并发档案）→ P4.5 计划模式／P4.8 模型目录；CUI／TUI／GUI／Web 的待办视图与 TUI/Web 分叉入口可作小批穿插 |

## 5. 复审触发（绑定命令＋判定）

| 命令 | 判定 |
|---|---|
| `python .github/scripts/check_counts.py` | missing>0 或计数不一致 → 本文件计数行失效，重跑比对 |
| `git status --short` | P3.1 的 WIP 提交且 ADR-065／FR-769／T282 入账 → P3.1 重判为“已实现（待验收）” |
| `rg -n -i "steer" crates` | 出现命中 → P1.8 状态重判 |
| `rg -n "models" crates/whcode-llm/src/http/profile.rs` | 出现命中 → P4.8 状态重判 |
| `rg -n "shadowed\|log_only" crates/whcode-app/src` | 出现命中 → P3.2 状态重判（2026-11-12 已命中：收口） |

## 6. 修订记录

- 2026-11-11（首版）：45 条执行态比对完成；F1—F6 发现入库。
- 2026-11-12（复核）：复审触发 2 命中——P3.1 收口（ADR-065／FR-769／T282／Check 377
  入账＋代码落地），重判为“已实现（待验收）”；计数复核 147／282／377（exercised 328），
  missing=0；W3 表、F3、§2 已同步。
- 2026-11-12（第二次复核）：P3.2 收口（ADR-066／FR-770／T283／Check 378），重判为“已实现
  （待验收）”；已实现 21／未实现 22；计数复核 148／283／378（exercised 329），missing=0；
  §2、W3 表、F6 与复审触发表已同步。
- 2026-11-12（第三次复核）：P4.6 收口（ADR-048 §7.1／FR-771／T284／Check 379，`c4328d2`），
  重判为“已实现（待验收）”；已实现 22／未实现 21；计数复核 149／284／379（exercised 330），
  missing=0。
