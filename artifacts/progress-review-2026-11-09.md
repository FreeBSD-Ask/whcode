# 复核记录：终态功能建议书 v3 执行态审计与并行批次（2026-11-09）

**性质**：执行前复核（0 信任口径）与并行批次的证据索引；不是验收结论（ADR-037：验收以
CI 日志与 artifact 为准）。**范围**：ADR-059 路线（proposal-v3 45 提案）的当前执行态、
Phase 0 遗留项与并行化裁决。

## 0. 任务认知卡（宪法原则 X）

- **方向**：用户指令“复核后执行：终态功能建议书 v3——不计实现成本口径；检测到并行任务则继续
  执行可并行任务”。先核清磁盘事实与账本，再按依赖与写域把可并行项拆开推进。
- **目标**：Phase 0 收口项（T251／T255—T263 中的 9 个 missing Check）按依赖推进；不因“成本”
  删项，也不因“并行”牺牲证据与门禁。
- **用户意图**（按指令推断）：路线已批准（ADR-059），当前缺的是执行与证据；“并行”指尽量
  同时推进互不冲突的批次，不是降低验收标准。
- **判据**：①每批有可判定 Check 与实测证据；②写域不重叠、台账由 Lead 统一落；③
  fmt／clippy／test 与治理门禁全绿；④未取得项显式登记，不伪造。
- **非目标**：不重排路线优先级；不扩大到 9 个 missing Check 之外的新提案；不动硬边界
  （零 unsafe、原生 GUI、平台范围、资源铁律）。

## 1. 复核方法与命令（可复现）

```powershell
git -C D:\WHCode log --oneline -25
git -C D:\WHCode status --short
cargo clippy --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo fmt --all --check
python .github/scripts/traceability.py
python .github/scripts/check_counts.py
python .github/scripts/check-knowing-first.py
python .github/scripts/check-docs-style.py
python .github/scripts/check-eol.py
python .github/scripts/check-workflows.py
python .github/scripts/check-adr-limits.py
```

## 2. 复核结论（执行前基线）

- **账本一致**：FR=146、任务=281、检查点=376；FR→任务缺 0、幻影 FR/任务 0；证据态
  exercised=319、outcome-supported=49、missing=8（复核起点为 missing 9）。
- **9 个 missing Check 的分布**（复核起点）：347（T251 WS 对话页）、348/349/350（B3 工具卡与
  审批）、354/355/356（B4 高级面板）、357/358（B5 分享导出与预览评估）。这一分布即
  “Phase 0 尚未收口”的精确含义：B2 已收口，B3／B4／B5 未收口。
- **治理门禁**：traceability／check_counts／knowing-first／docs-style／eol／workflows／
  adr-limits 全部通过（复核时点）。clippy 全工作区零告警；fmt 无差异。

## 3. 复核发现（按严重度）

| # | 发现 | 事实依据 | 处置 |
|---|---|---|---|
| F1 | **TUI 测试非环境无关**：`rowbackend::tests::style_is_emitted_once_per_run` 在 `NO_COLOR=1` 环境必然失败（left 0 ≠ right 1） | crossterm 的 `Colored::Display` 在 `NO_COLOR` 非空时整段抑制颜色；本机 `NO_COLOR=1`；`cargo test -p whcode-tui --lib` 单测复现 | 已修（`b5d6bc5`）：测试二进制内 `Once` 固定启用 ANSI，产品继续尊重 `NO_COLOR`；修后两种环境 75/75 通过 |
| F2 | **WS 中继真实缺陷**：`turn_stream_frames` 产出 JSON 载荷，却被直接塞进“已编码 WS 帧”的广播队列；真实浏览器按 RFC 6455 解析会帧错位 | `ws::encode_text_frame` 只在 `poll_events` 路径使用；`drain_turn_pending` 的 delta 路径未编码 | 由并行任务修复（见 §4 流 A）；这解释了 Check 347 条件 4 长期无法闭环 |
| F3 | **`[approval]` 死配置**：配置可解析、有测试，但四个生产 turn 入口仍走 `run_turn`／`run_turn_with_progress`，配置无生效路径 | `run_turn_with_approval` 仅测试调用；`crates/whcode-bin/src/{serve,cui_dispatch,llm_provider}.rs` 调用点未变 | 由 Lead 装配批次收口（读配置 → 构造 `ApprovalPolicy` → 注入闸门）；ADR-064 “已知限制”已临时登记 |
| F4 | **生产无 `AuditSink` 实现**：只有测试用 `CollectingAuditSink`／`FailingSink`；`store::audit::insert_record` 需要 `&Connection` | `impl AuditSink for` 检索结果全在测试模块；`gates::request_transition` 亦同 | 登记为缺口（审批审计的持久化 sink 属装配批次）；Check 349 证据以行为契约与成对审计断言为准，不冒充已落表 |
| F5 | **T251 在途 WIP**：`ServeLlm.factory` 为 `Arc<dyn ProviderFactory + Send + Sync>`，是注入 StubProvider 的前置；ADR-058 §2.8.8 用例 7—11 与三指标归档未取得 | `git status` 的在途 diff；`artifacts/turn-ws-relay-*` 缺失（复核起点） | 交由流 A 收口；未取得前不勾 T251／Check 347 |

## 4. 并行批次（写域划分）

| 流 | 共享任务 | 写域 | 判据 |
|---|---|---|---|
| A | task-1（T251／FR-742／Check 347） | `crates/whcode-bin/src/{serve,llm_provider}.rs`、`docs/adr/ADR-058`、`artifacts/turn-ws-relay-2026-11-09/` | 用例 7—10 可执行、用例 8 三指标归档；用例 10 为 `#[ignore]` 资源实验＋显式命令；用例 11 缺口登记 |
| B | task-2（T255／FR-744／Check 349） | `crates/whcode-app/src/{approval,turn,lib}.rs`、`crates/whcode-domain/src/approval.rs`、`crates/whcode-bin/src/config.rs`、`docs/adr/ADR-064` | 规则正反路径＋审计成对＋不可重试＋有界无线程；默认 allow 行为中性 |
| Lead | T262（FR-753／Check 358）＋TUI 缺陷修复＋装配收口＋台账 | `docs/guides/preview-evaluation.md`、`docs/guides/README.md`、`crates/whcode-tui/src/rowbackend.rs`、`specs/**`、`CHANGELOG.md` | 文档门禁全绿；台账三方追溯一致 |

写域互不重叠；台账（`specs/**`、`CHANGELOG.md`）只由 Lead 落笔。跨流唯一的共享文件
（`crates/whcode-bin/src/serve.rs`）显式归流 A，Lead 的装配收口在其完成后再动。

## 5. 已交付与本记录时点的缺口

- 已交付：F1 修复（`b5d6bc5`）、T262 评估结论与 Check 358 取证（`9ba4628`）、
  T255／Check 349（`c1eed74`）。
- 回填（2026-11-10）：流 A 收尾——T251 的 M7 用例 7—10 已落地并归档
  `artifacts/turn-ws-relay-2026-11-09/`（用例 8 三指标：首帧 delta 1006 ms、503
  `backpressure`＋`Retry-After: 1`、cancel 200、`/ws` 101、`event_missing` 0；用例 10
  资源实验线程 +1 后回基线）；用例 11 按 ADR-058 §3.6 缺口登记。`58f751d` 已提交，
  **推送被网络阻断**（`github.com:443` 连接失败），CI 未运行，故 T251／Check 347 仍未勾
  （ADR-037：以 CI 日志为准）。
- 后续批次（2026-11-10）：Web 元素级 RTL 镜像收口（`2c7ba05`）、hi/bn 文案术语对齐
  （`33f1468`）、B3b 四态审批后端与规则存储（`54b9496`，schema v9）。
- 未闭环（登记，不冒充）：F3 的装配接线（`[approval]` 与交互应答面）；B3b 的 GUI／Web
  工具卡与四态按钮（后端语义与规则存储已于 `54b9496` 落地）、B4（T258／T259）、B5 余项
  （T260／T261／T263）；CI 未在本机运行（ADR-037），勾选以本机实测＋待运行登记表述。

## 6. 复审触发（绑定命令＋判定）

| 命令 | 判定 |
|---|---|
| `cargo test -p whcode-tui --lib` | 非 0 或与 `NO_COLOR` 取值相关 → 复审 F1 的测试保真度 |
| `cargo test -p whcode-bin --bin whcode serve` | 非 0 → 复审 WS 中继编码口径（F2） |
| `rg -n "run_turn_with_approval" crates/` | 生产入口仍为 0 命中 → F3 未收口，`[approval]` 仍不生效 |
| `rg -n "impl AuditSink for" crates/ --glob '!**/tests*'` | 仍无生产实现 → F4 缺口未变，不得宣称“审计已落表”（2026-11-10 已由 `StoreAuditSink` 收口） |
| `python .github/scripts/check_counts.py` | 计数不一致 → 冻结勾选 |

## 7. 修订记录

- 2026-11-09（首版）：执行前复核与并行批次划分；F1—F5 发现、写域表与复审触发入库。
- 2026-11-10：回填流 A 收口与本批后续（RTL 收口、文案审校、B3b 后端）；F4 由
  `StoreAuditSink` 收口，F3 与 B3b UI 仍未闭环。
