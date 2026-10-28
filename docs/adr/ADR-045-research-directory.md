# ADR-045：研究目录（research/）的定位、分类与入库规则

- **状态**：已批准（2026-10-28，用户指令“”）
- **来源**：用户指令；`docs/guides/repository-layout.md`（建新顶层目录需先有 ADR）；差异比较书初稿 `local/diff-zcode-vs-whcode.md`（2026-10-28，不入库）
- **责任人**：研究文档维护者、布局门禁维护者
- **退出条件**：研究目录废弃（用户决定移除，须另立 ADR 并按本 ADR 退出条件清理）；研究结论并入 `docs/` 或 `specs/`（须走对应门禁）；ZCode 上游不再作为参照对象

## 1. 事实与证据

- 现行 `docs/guides/repository-layout.md` 顶层目录表只允许 `crates/`、`tests/`、`docs/`、
  `engineering-doctrine/`、`.specify/`、`specs/`、`artifacts/`、`assets/`、`vendor/`、`local/`、
  `target/`、`state/`、`.github/`，并由 `tests/contract/test_layout.rs` 的
  `top_level_entries_follow_layout` 硬强制；`docs/` 子目录限 `adr/`、`matrices/`、`guides/` 三类。
- 差异比较书 `local/diff-zcode-vs-whcode.md` 是 WHCode × ZCode 的**事实对照与差距登记**，
  按比较书自身边界“”。它目前存于
  `local/`（不入库），被 `docs/adr/ADR-044-agent-turn-loop-orchestrator.md` 引用
  （其 §1.2 与 §5 均引 `local/diff-zcode-vs-whcode.md`）。
- 外部参照/调研证据（对照、差距、参照路径）与需求基线（`specs/`）语义不同：前者是
  “”，后者是“”。把它们混在 `specs/` 会模糊“”
  与“”的边界。
- ZCode 源码浅克隆在 `local/zcode-src`（commit `3c5b227649371507ddc4eb46ac5133afe6db1cab`，
  2026-10-20，Apache-2.0），共 8234 个 TypeScript 文件，属本地克隆，**不入库**（体积与
  第三方内容不可作为仓库产物）。

## 2. 决策

1. **新建顶层 `research/` 目录**：定位为“”的归属地，与 `docs/`（治理文档）
   和 `specs/`（需求基线）解耦。`research/` 承载**事实比较、差距登记、参照路径、存疑项**，
   不承载需求承诺；任何研究结论转化为承诺必须走 G0—G5 门禁并落到 `specs/` 或 ADR。
2. **分类边界**：
   - `docs/adr/`：决策记录（已批准/提议的决策本身）；
   - `docs/guides/`、`docs/matrices/`：操作指南与矩阵（本仓库治理）；
   - `specs/`：需求基线（FR/NFR/任务/Check/契约）；
   - `research/`：**外部参照与调研证据**（对照、差距、参照路径、存疑项、上游源码分析）；
   - `local/`：临时/探测脚本与不入库克隆（如 `zcode-src`、差异书源稿）。
3. **入库规则**：`research/` 内文档入库；其引用的外部源码克隆**不入库**（仍在 `local/`），
   研究文档以**路径 + commit 哈希**指向克隆，不得把克隆内容复制进 `research/`。
4. **更新布局门禁**：`docs/guides/repository-layout.md` 顶层目录表增补 `research/` 一行
   （类别：治理（研究证据）），§1 职责表、§3 放置约定同步；`tests/contract/test_layout.rs`
   的 `top_level_entries_follow_layout` 的 `allowed_dirs` 增补 `research`。
5. **差异比较书演化为 `research/zcode-comparison/`**：按域拆分（A—P 十六域逐篇 +
   overview + actionable-gaps + README 索引），入库；原 `local/diff-zcode-vs-whcode.md`
   保留为源稿留档（不入库）。
6. **引用迁移**：`docs/adr/ADR-044-agent-turn-loop-orchestrator.md` 中指向
   `local/diff-zcode-vs-whcode.md` 的引用改为指向 `research/zcode-comparison/` 对应条目，
   保持一致性。

## 3. 正负和解

- **正面**：研究证据从“”升级为“”的治理文档；
  分域拆分让 P0—P3 行动项有独立挂载点；0 信任复核（重读源码）消除旧结论的过时风险；
  外部克隆不入库，不污染仓库体积。
- **负面**：新增顶层目录与布局门禁白名单改动（`test_layout.rs` 需同步）；研究文档篇幅
  显著增加，维护者需与 ZCode 上游变更保持同步（有失效条件）。
- **承担者**：研究文档维护者（与上游同步、复审触发）、布局门禁维护者（白名单一致性）。
- **和解理由**：研究是“”不是“”，入库后 ADR-044 等引用可回溯；研究目录与需求
  基线解耦，避免把调研结论误当验收承诺；外部克隆不入库符合“”定位。

## 4. 三重时间

- **过去来源**：差异比较书初稿（2026-10-28，ZCode commit 3c5b227）；ADR-044 引用其 §4 P0-1；
  `repository-layout.md` 现行目录分类；`test_layout.rs` 硬门禁。
- **现在证据**：本次研究重读 ZCode 源码（8234 TS 文件）与 WHCode `crates/**`、`specs/`、
  `docs/adr/`；`tasks.md` 为实现权威。
- **将来失效条件**：ZCode 上游 commit 变更（参照路径失效）→ 复审触发；任一 P0—P3 落地 →
  对应域报告更新；研究目录若膨胀需再立 ADR 分层；用户决定移除研究目录（须另立 ADR）。

## 5. 复审触发

- ZCode 上游 commit 变更（重读对应文件，更新路径与 commit）；
- 任一 P0—P3 行动项落地或废弃（更新 `actionable-gaps.md` 与对应域报告）；
- 研究目录文件数或体积显著膨胀（再立 ADR 分层）；
- 布局门禁白名单与 `repository-layout.md` 出现不一致（立即纠正）。

## 6. 实施摘要

- 本 ADR 批准同日：更新 `repository-layout.md` 与 `test_layout.rs` 白名单；建立
  `research/zcode-comparison/`（README/overview/domain-{A..P}/actionable-gaps）；
  迁移 ADR-044 引用路径；原 `local/diff-zcode-vs-whcode.md` 保留为源稿。
- 门禁：`cargo test -p whcode-tests --test layout`（research 白名单）、
  `cargo test -p whcode-tests --test layering`、`python .github/scripts/check-eol.py`
  （LF + 无 BOM）。

## 7. 修订记录

- **2026-10-28（首版批准）**：确立 `research/` 定位与入库规则；同日建立
  `research/zcode-comparison/`（README／overview／domain-{A..P} 十六篇／actionable-gaps，
  19 文件），`repository-layout.md` 与 `test_layout.rs` 白名单增补 `research`；
  ADR-044、`plan-p0-1-turn-loop.md`、`tasks.md` 中指向 `local/diff-zcode-vs-whcode.md`
  的引用全部迁移至 `research/zcode-comparison/` 对应条目；原差异书保留 `local/` 源稿留档。
  门禁待 CI 实跑取证（ADR-037：以 GitHub Actions 日志为准）。
