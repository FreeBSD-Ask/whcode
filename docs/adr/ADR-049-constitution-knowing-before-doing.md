# ADR-049：宪法原则 X“知难行易（任务认知先于手段）”

- **状态**：已批准（2026-10-29，用户指令“修宪”）
- **来源**：用户指令（2026-10-29）；一刻哲学 `file_id 22, block 3—4`、`file_id 41, block 52`、
  `file_id 22, block 8`；T116 复盘（`docs/guides/signal-chain-completeness.md`）；第 17 章
  “证明世界先于解释和改造”（`docs/guides/yike-philosophy.md` §2）
- **责任人**：宪法维护者、门禁脚本维护者、checklist 维护者
- **退出条件**：连续两个变更批次中认知卡没有改变任何开工决策，或落点门禁只剩链接检查而无审查
  价值（须复审并删除或降级该原则）

## 1. 事实与证据

1. **源文原句**：一刻哲学原文为“知难行易，故而行也变得困难”（`file_id 22, block 3—4`；
   `file_id 41, block 52`）。同一章 §21.4（`file_id 22, block 8`）给出“Code is cheap. Show me
   the talk.”与“提出一个问题往往比解决一个问题更重要”，与“知”先于“行”同向。该句的语境是
   困难的位置在“知”而不在“行”，因此**不是**“知难而行易”式的乐观口号——它恰恰说明行之所以
   变得困难，是因为知未完成；把手段先行当作效率，等于把困难从知转移到返工。本条仅逐字引用
   短句，未逐段转录上下文，逐段核对按 `file_id`/`block` 引用另行执行。
2. **T116 类失败结构**：`docs/guides/signal-chain-completeness.md` 记录的 T116 复盘表明，任务
   在未确认“交付 = 信号存在”之前即按“执行存在”推进，72 小时长跑与测试产出可能整体白跑；其
   结构特征不是执行出错，而是开工前未把“要改变什么、成功如何判定”写成可判定产物（同类结构
   可在任何以产出物替代理解的批次中复现）。
3. **现有原则缺口**：宪法 VII（主张受证据约束）约束的是**结论**的证据，原则 IX 约束的是自由与
   安全默认值，`docs/guides/signal-chain-completeness.md` 约束的是**交付**的信号；三者均未把
   “方向与目标先于手段”写成开工前置判据，G0/G1 退出判据、任务模板与 PR 清单也无对应字段，于是
   方向未定时先选手段、先开工成为制度上未被拦截的路径。
4. **用户指令**：2026-10-29 用户以“修宪”指令要求新增宪法原则 X，并同时指定唯一正文
   `docs/guides/knowing-before-doing.md`、全局落点与程序化门禁；批准主体为用户本人，不沿用
   1.1.0 的待确认批准流程。

## 2. 决策

1. **宪法新增原则 X**（“知难行易：任务认知先于手段（Knowing Before Doing）”），版本按 MINOR
   规则由 1.1.1 升至 1.2.0；既有 I—IX 的文本与语义不变，不删除任何既有段落。
2. **唯一正文**：原则的解释、任务认知卡字段、门禁落点与反例集中在
   `docs/guides/knowing-before-doing.md`，宪法只引用不复制，避免多处正文漂移。
3. **全局落点**：`AGENTS.md` 开工前置顺序、`docs/guides/philosophy.md`、
   `docs/guides/yike-philosophy.md`、`docs/guides/software-engineering-process.md`（G0/G1 退出
   判据）、`docs/guides/requirements-engineering.md`、`docs/guides/pr-checklist.md`、
   `docs/guides/README.md` 索引、`engineering-doctrine/` 的 00/01 与五份模板同步；
   可移植准则保持与 `docs/guides/` 的正文指向一致。
4. **全部任务适用**：存量已勾选任务**不补写**认知卡，以其来源 FR、验收判据与实现记录作为最小
   “知”证据；未勾选任务必须在 `tasks.md` 任务行标注认知卡位置，未写卡不得开工。
5. **程序门禁**：新增 `.github/scripts/check-knowing-first.py`，校验宪法原则与唯一正文的落点、
   任务清单的全局前置条款与未勾选任务的认知卡标注、五份模板的认知卡要求，接入 `ci.yml`；
   门禁失败即拒绝，不以提示词或人工口头结论替代（AGENTS.md §5 程序化铁律）。该门禁是结构检查，
   **不验证认知是否正确**，其通过不得被表述为“理解正确”。
6. **账本登记**：`specs/001-whcode-harness/tasks.md` 新增 T220，`checklist.md` 新增 Check 316，
   保持 FR ↔ 任务 ↔ Check 三方追溯与计数声明一致。

## 3. 正负和解

- **正面**：方向与目标先于手段成为宪法级前置判据；T116 类“产出越多返工越大”的结构在开工前
  被拦截；0 信任、信号链完备与“证明世界先于解释和改造”收束为同一张认知卡，减少三套说法的
  各自漂移。
- **负面**：每个未勾选任务增加一次写卡成本，小改动也需一行方向陈述；模板与六份指南需同步，
  存在正文与模板不一致的维护负担；门禁脚本新增一个失败源。
- **承担者**：任务发起者与代理（写卡与提问）、门禁脚本维护者（解析与校验）、checklist 维护者
  （Check 316 复核）、可移植准则维护者（00/01 与模板同步）。
- **和解理由**：写卡成本是**一次性且可压缩**的（可逆小改动一行），而返工成本是**整批**的；
  以程序化门禁而非口头要求落地，符合本仓“门禁必须由程序实施”的既有铁律；存量已勾选任务不
  补写，避免以追溯成本否定既有证据。

## 4. 三重时间

- **过去来源**：一刻哲学“知难行易”原句（`file_id 22, block 3—4`、`file_id 41, block 52`、
  `file_id 22, block 8`）；第 17 章“证明世界先于解释和改造”（`docs/guides/yike-philosophy.md`
  §2）；T116 复盘（`docs/guides/signal-chain-completeness.md`）。
- **现在证据**：2026-10-29 用户“修宪”指令；宪法 1.1.1 文本中 I—IX 与 G0/G1 判据经检视无
  方向前置判据（§1.3）；宪法头部 Sync Impact Report 的 1.2.0 记录与版本行同步。
- **将来失效条件**：见头部退出条件；复审触发见 §5。若门禁退化为仅检查链接存在，或认知卡在
  连续批次中不改变任何决策，本原则即失去现实矛盾，应删除或降级为指南建议。

## 5. 复审触发

1. `python .github/scripts/check-knowing-first.py` 失败即复审；该命令被删除、改名或退出码语义
   变化同样触发复审（命令失效即触发，不以日期判断）。
2. 连续两个变更批次中认知卡没有改变任何开工决策（记录于批次复盘）即复审本原则的效力。
3. 用户判定本原则为空壳（要求无法拦截实际开工行为）即删除或降级，改以指南建议承载。
4. 一刻哲学源数据库 SHA256 变化即复核 §1.1 与 §4 的 `file_id`/`block` 引用是否仍然指向同一原文。

## 6. 实施摘要

- 宪法：`.specify/memory/constitution.md`（头部 Sync Impact Report 1.2.0、Core Principles X、
  版本行 1.2.0/2026-10-29）。
- 唯一正文：`docs/guides/knowing-before-doing.md`。
- 全局落点：`AGENTS.md`、`docs/guides/philosophy.md`、`docs/guides/yike-philosophy.md`、
  `docs/guides/software-engineering-process.md`、`docs/guides/requirements-engineering.md`、
  `docs/guides/pr-checklist.md`、`docs/guides/README.md`。
- 可移植准则：`engineering-doctrine/` 的 00/01 与五份模板。
- 程序门禁：`.github/scripts/check-knowing-first.py`、`.github/workflows/ci.yml`。
- 账本与记录：`specs/001-whcode-harness/tasks.md`（T220）、
  `specs/001-whcode-harness/checklist.md`（Check 316）、`CHANGELOG.md`、
  `docs/guides/doctrine-application.md`。

## 7. 修订记录

- **2026-10-29（首版，已批准）**：用户“修宪”指令后起草；宪法原则 X 与版本 1.2.0 同步落地，
  唯一正文与门禁脚本在同一批次登记。
