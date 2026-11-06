# 域 P：工程流程与门禁

> DSH 证据根：`local/dsh-src`（commit `fcc0ad4`）；WHCode 证据根：本仓库。

## 1. 对照表

| 项 | DeepSeek Harness (DSH) | WHCode | 判定 |
|---|---|---|---|
| 测试层级 | 六层：单测／per-file 100% 覆盖率门禁／真 API e2e／owner-local expected／性能基准／快照回放（`docs/testing.md`） | 单测／契约（layering/layout/parity）/集成／崩溃／稳定性五目录＋CI 唯一验收（ADR-037） | **DSH 领先**：层级更细 |
| 覆盖率 | `packages/*/*/src` **每文件 100%**；未覆盖行常被当作待删死代码（`docs/testing.md`；`package.json:62`） | 无覆盖率门禁；clippy `-D warnings`＋契约测试 | **DSH 领先** |
| 生成与校验 | 大量 `gen-*`/`verify-*` 脚本：cordis 目录、config 目录、tool 目录、模块图、依赖目录、文档图（`scripts/` 目录，约 30 个） | 手写门禁：traceability/check_counts/eol/workflows/code-scanning（`.github/scripts/`） | **取向不同**：DSH 文档从代码生成，WHCode 门禁校验规格 |
| e2e 纪律 | “验证世界而不是自述”：断言重跑命令或外部读文件，**关键词探针会让作弊代理通过**；未触碰文件必须字节相同（`docs/testing.md`） | 无 e2e 层（CI 冒烟未自动化 GUI 交互） | **DSH 领先**：反作弊断言 |
| 真 API 测试 | “不要吝啬真 API”——无 key 测试只证管道；每个出货 profile 一个冒烟（`docs/testing.md`） | 测试模型约定在文档（`docs/guides/test-model-sensenova.md`），无 CI 真 API 冒烟 | **DSH 领先** |
| 快照回放 | 录制会话回放作为期望输出；浏览器快照 Chromium/WebKit 对比（`docs/testing.md`） | 无录制回放 | **DSH 领先** |
| 提交前门禁 | lefthook（`lefthook.yml`）＋CI 强制 | git hooks 无；CI 全量门禁（`ci.yml:78`—`226`） | **同构**：都以 CI 为最终屏障 |
| action 引用 | 用可移动 tag（如 `actions/checkout@v7.0.1`、`pnpm/action-setup@v5.0.0`；`.github/workflows/ci-master.yml:60`、`:64`） | 全部 `owner/repo@<40位SHA> # vX.Y.Z`＋`action-versions.json` 单一事实源（AGENTS.md §5.1） | **WHCode 领先** |
| 文档纪律 | 每包 README 必须有 Known Limitations 节或显式豁免（`packages/README.md`）；文档中英成对（`.i18n.yaml`） | 文档中文正文＋验收 FR；无“已知限制”强制节 | **DSH 领先**：限制强制显式化 |
| 决策记录 | `.agents/notes/implemented/**` 归档决策注记（无 ADR 体系） | ADR-001—058 走流程，带复审触发（ADR-046） | **WHCode 领先**：可追溯决策 |

## 2. DSH 实现解剖

### 2.1 覆盖率门禁被当作死代码探测器

per-file 100% 是**门禁**（`docs/testing.md`），且官方口径是“未覆盖行常常是应该删掉的
死代码，而不是要补的测试”（同节）。这条把覆盖率从虚荣指标变成了死代码探测器。
pwsh 相关文件在无 pwsh 的宿主上自我跳过并在 vitest 配置中豁免，CI runner 装 pwsh 强制全额
（同节）——豁免是显式登记的。

### 2.2 反作弊的 e2e

“e2e 断言要么重跑命令、要么外部读文件；对代理自身输出的关键词探针会放过作弊代理”
（`testing.md`），并要求断言未触碰文件**字节相同**（§Verify the world, not the self-report）。这条直接来自
“代理会伪造证据”的现实威胁。

### 2.3 文档契约由脚本强制

`verify-package-readme-model-experience.ts`／`verify-package-readme-limitations.ts`（`packages/README.md`）
强制每个包 README 有模型体验与已知限制节（或登记豁免）；`docs/` 的表格由 `gen-*` 从代码生成，
并有 `verify-cordis-catalog` 校验新鲜度。文档腐化被当作代码问题处理。

### 2.4 决策以 Agent Note 归档

`.agents/notes/implemented/**` 存档已实施决策（如 `2026-08-31-released-session-format-migrations`），
`.agents/skills/` 存自举技能。没有 ADR 状态机，但有“归档在案”的习惯；与 WHCode 的 ADR
相比少了流程与复审触发，多了“决策密度”。

### 2.5 提交者工作流

AGENTS.md 要求：测试随代码、每个 registry 有 HMR 安全测试（dispose 后断言清理）
（`testing.md`）、失败诊断走 flake 工作流而非重跑了事（§How specs execute）。

## 3. WHCode 现状核对

- 已实现：三方追溯（`traceability.py`）、计数声明（`check_counts.py`）、分层/布局契约测试、
  EOL/无 BOM 审计、CI 语义门禁（`check-workflows.py`）、Code Scanning 零告警复核、
  action 固定 SHA＋每周复核（`AGENTS.md` §3、§5.1、§5.2）。
- 已实现：知难行易门禁（`check-knowing-first.py`）、门禁判定程序化（§5）。
- 缺失：覆盖率门禁、真 API CI 冒烟、录制回放、反作弊 e2e、文档生成与新鲜度校验。
- 差异：WHCode 的门禁校验**规格一致性**（FR↔任务↔Check），DSH 的门禁校验**代码与文档的
  一致性**。前者防漂移，后者防腐化。

## 4. 差异判定与可借鉴点

1. **“验证世界而不是自述”**：必须采纳（理念级）。WHCode 的验收 Check 应要求“可复现命令 +
   外部可读产物”，禁止以日志文案为证——这已是信号链完备原则的变体，可把它写死成
   契约测试模式。
2. **未触碰文件字节相同**：可采纳。工具类 Check 的回归测试应断言无关文件不变。
3. **覆盖率门禁**：可评估。Rust 侧 `cargo-llvm-cov` 可行，但“每文件 100%”在包含 FFI/平台
   分支的 crate 上成本高；建议先对 `whcode-app`/`whcode-store` 试点行覆盖阈值（如 90%），
   并把“未覆盖即删”的口径写进 PR 检查单。
4. **README 强制 Known Limitations**：可采纳为 ADR/文档门禁：每个新 ADR 必须有“失效条件与
   已知限制”节（ADR-046 已要求复审触发，可扩展为限制显式化）。
5. **文档生成**：部分可借鉴。`tool-catalog`/`config-catalog` 式的“从代码生成能力目录”适合
   WHCode 的工具清单（`registry.provider_manifest()` 已有数据源），生成后用脚本校验新鲜度。
6. **不采纳**：`always` 无限重试的 always-mode e2e（域 K）、真 API 全量在 CI（成本与凭据风险）；
   WHCode 的 e2e 冒烟可用 StubProvider + 少量真 API 手动轮次。

## 5. 待验证项

1. DSH 第三方 action 是否曾有安全事件驱动的 SHA 化讨论：现抽查确认是 tag 引用，但未查 issue/安全策略。
   取证：`rg -n "pin|sha" CONTRIBUTING.md .github/`。
2. DSH 覆盖率门禁的豁免清单与理由：`scripts/coverage-exempt.ts` 存在。取证：读该文件头部。
3. WHCode CI 是否已包含 GUI/Web 冒烟自动化：FR-617 相关任务状态。取证：
   `rg -n "冒烟" specs/001-whcode-harness/tasks.md | head`。

## 6. 证据路径清单

- `docs/testing.md`（六层测试）
- `docs/testing.md`（per-file 100% 门禁）
- `docs/testing.md`（覆盖率＝死代码探测器）
- `docs/testing.md`（快照回放）
- `docs/testing.md`（真 API 政策）
- `docs/testing.md`（反作弊 e2e）
- `package.json:62`（test:coverage）
- `packages/README.md`（README 契约）
- `.github/workflows/ci.yml:78`（WHCode 门禁链）
- `.github/scripts/check-workflows.py`（CI 语义门禁）
