# Qoder Better Harness 设计要点提取（research/better-harness/）

- **目录性质**：外部参照与调研证据（入库依据 ADR-045）；**不构成 ADR 或实施承诺**。
  批判与融合方案见 [critique-and-fusion.md](critique-and-fusion.md)。
- **Better Harness 证据**：ZCode 插件缓存完整仓库快照
  `C:\Users\ykla\.zcode\cli\plugins\cache\better-harness\better-harness\0.7.0-alpha2`
  （MIT 许可，作者 Qoder，`https://github.com/QoderAI/better-harness`，
  Node 22 + ESM + npm workspaces）。上游浅克隆于 `local/better-harness-src`（**不入库**，
  ADR-045），commit `b605532fcf1014bcb48d3e6940a5bc64e80cddd5`（2026-10-20）；
  经文件树 diff 核对该 commit 即 `0.7.0-alpha2`（唯一差异为插件安装器添加的
  `marketplace.json`，非上游文件）。本文引用路径相对快照根。
- **WHCode 证据**：本仓库 `crates/**`、`specs/001-whcode-harness/`、`docs/adr/`；
  实现状态以 `tasks.md` 为权威。
- **0 信任口径**：结论由本仓库直读快照源码与文档得出（file:line 随文给出）；
  上游可能已变更，实施前须重读对应文件。本文**不断言 WHCode 已实现某能力**——以
  `tasks.md`/`checklist.md` 为准。

## 1. 定位与技术栈总览

| 维度 | Better Harness | WHCode |
|---|---|---|
| 定位 | Agent Work Loop 评审系统：把项目/会话/代理资产证据转为按优先级排序的发现与可验证下一步 | Rust 原生、本地优先 LLM 编码 Harness：事件可追踪、规格驱动门禁 |
| 语言/运行时 | TypeScript ESM，Node 22.20（`package.json:133` engines `>=22.20.0 <25`），根仅 3 个运行时依赖（tree-sitter-wasm、esbuild-wasm、yaml，`package.json:137-141`） | Rust 1.99.1（零 unsafe 铁律），14 crate workspace |
| 形态 | 按需 CLI（`scripts/better-harness.mjs` 薄门面）+ 斜杠命令 Skill + 宿主插件壳，无常驻进程 | 常驻应用（TUI/CUI/Web/SSH/GUI 多前端共享应用服务） |
| 评审单元 | Task Episode（一个用户目标 + 一个验收边界，可跨 turn/session） | 任务（SDD 规格 9 态状态机）+ Check（299 条验收检查点） |
| 判定主体 | LLM 代理评审（评分、发现筛选、verified 判定）+ 确定性收集器（事实层） | 程序化门禁（traceability/check_counts/layering/layout）+ LLM 仅建议 |
| 许可 | MIT（`LICENSE`） | BSD-2-Clause（MIT 与 BSD-2 双向兼容；本文只提取设计并重写，不复制文本） |

## 2. 架构总览

三层开放（README.md "What is open"）：

1. **工程实践**——`references/` 四域知识库（session-evidence / project-harness /
   agent-customize / loop-engineering，共 66 篇 .md）；
2. **评估模型**——`models/agent-work-loop.md`（819 行：五维、15 检查、证据五态、评分
   天花板、学习捕获规则）与 `models/harness-engineering.md`（F0—F4 流畅度）；
3. **可运行实现**——`skills/better-harness/SKILL.md` 规范工作流 + `scripts/` 确定性
   收集器/渲染器 + 薄宿主适配器。

评审流程五步（`skills/better-harness/SKILL.md:11-215`）：

```text
Step1 证据包冻结（evidence-bundle：topology/provider/window/depth/authority，schema v3）
Step2 恰好 3 个只读证据代理并行（会话/项目/代理资产），硬上限 3 个委托，specialist 不定级
Step3 主代理调和：保留全部候选→按同 target/后果/owner/修复路线合并→冻结发现与分数
Step4 授权渲染（禁止手写 HTML/Canvas/Markdown；render --validate 通过才算成功）
Step5 finding-bound 修复（独立 post-fix 子代理复评）与后续跟进
```

核心边界原则（README.md）：**"配置的资产只能证明机制存在；只有关联到任务的证据才能
证明它被使用或改善了结果"**——与 WHCode `docs/guides/signal-chain-completeness.md`
的"交付 = 信号存在，而非执行存在"同构。

## 3. 设计要点提取

### 3.1 评估模型层

| 编号 | 要点 | 证据 |
|---|---|---|
| A1 | **五维 × 15 检查**：Task Understanding / Controlled Execution / Change Validation / Reliable Delivery / Learning Capture，各 3 个稳定检查 ID（如 `goal-understanding`、`relevant-check`、`rollback-recovery`、`later-validation`）；模型自述"不是必选流程、不是转录评分、不是资产清单、不是仓库成熟度模型" | `models/agent-work-loop.md:78-89` |
| A2 | **证据五态 + 三不可判定态**：`Present`（机制存在）→`Wired`（路由可达）→`Exercised`（被关联任务使用并留痕）→`Outcome-supported`（可比后期结果支持效果声明）；`Missing`/`Unobserved`/`Not applicable` 显式保留；"证据态不是通过/失败" | `models/agent-work-loop.md:93-108` |
| A3 | **分数天花板表**：最高证据态封顶分数——Missing/Unobserved/N/A→59、Present→74、Wired→84、Exercised→94、Outcome-supported→100；"是天花板，不是公式"；分数不产生也不抑制发现 | `models/agent-work-loop.md:110-124` |
| A4 | **纵向验证门**：Loop Effectiveness（五维分数）只能由"可比后期 Task Episode 或独立结果窗口"改变；同窗修复只更新 Repair Progress（`verified/partial/blocked`，由独立复核代理判定）；回归必须保留 stop/revert 阻断 | `models/agent-work-loop.md:151-164` |
| A5 | **干预台账**：`interventionLedger` 存于宿主自有 `findings.json`，保学习跨窗口连续性；恢复的台账只保纵向验证连续性，不合成当前评审状态 | `models/agent-work-loop.md:635-648` |
| A6 | **一手来源锚定**：五维各自映射 OpenAI Harness Engineering、Google Testing Overview、OTel Logs 规范、GitHub protected branches、Google SRE postmortem culture，并声明"来源解释形状，不冻结术语与分数" | `models/agent-work-loop-rationale.md:10-56` |
| A7 | **F0—F4 流畅度分级**：Missing/Present/Usable/Repeatable/Enforced；"F4 保留给机械证明，不是写得好的文档"；12 个检测清单（Context Map→Report Confidence）作为证据契约而非第二评分模型 | `models/harness-engineering.md:13-21,44-60` |

### 3.2 评审流程层

| 编号 | 要点 | 证据 |
|---|---|---|
| B1 | **三证据域独立 + 硬上限**：会话/项目/代理customize 三代理只读并行，互不传递结论；"本证据轮硬上限 3 个委托代理"；specialist 永不定最终严重度 | `skills/better-harness/SKILL.md:69-119` |
| B2 | **调和纪律**：保留每个候选；只合并"同 target、同观察后果、同 owner、同修复路线"；每个被弃候选留工作理由；禁止为凑 5 行/缩短报告而丢弃合格发现；**先冻结发现与分数，再允许 support track 塑形** | `SKILL.md:129-176` |
| B3 | **八道质量门**：证据资格（计数/搜索缺席/理论风险只是线索）、读者价值、事实一致、资产问责（清单不证明选择/使用/有用/结果）、隐私、可执行修复（命令必须已在目标中发现）、分数纪律、候选晋升（须过与发现同标准） | `skills/better-harness/references/findings-review.md:20-100` |
| B4 | **三条 support track**：Bootstrap(0→1)/Operationalize(1→60)/Optimize(60→100) 是用户旅程标签非分数段；Optimize 需≥2 个可比 Task Episode；track 不得新增发现、改严重度、重评分 | `references/support-bootstrap.md`、`support-operationalize.md`、`support-optimize.md` |
| B5 | **finding 结构要求**：被检视的缺口 + 有界影响 + 最小 owner 对齐修复 + 验证路线；"计数、文件名、资产存在性、严重度、年龄、churn、分数单独都不构成发现" | `models/agent-work-loop.md:126-134` |
| B6 | **finding-bound 修复协议**：机器回调验证（contract/workspacePath/findingsPath/findingId/expectedRevision）→ workspace-topology 绑定 fail-closed（拒绝绝对/逃逸路径）→ 最小 owner 加载 → 实际输出 1—12 行（来自真实 diff 而非期望）→ **恰好 1 个独立只读子代理**复评 verified/partial/blocked → `record-fix-output` 乐观锁（`--expected-revision`，失败不改动报告工件） | `references/finding-bound-fix.md:11-152` |
| B7 | **确定性/代理判断边界**：evidence-bundle、task-loop-report（"deterministic findings…without a total score"）、record-fix-output（"只重建投影，不生成读者文案"）为程序产出；`harness source-review` 三阶段 create→decision→apply，帮助文本声明"不代写 decision、不调用模型"；generated 字段（`learningCaptureDiagnostics` 等）不可被评审输入改写 | `scripts/better-harness-cli/registry.mjs:317-331`、`scripts/harness-analysis/report-source/cli.mjs:17-47`、`apply-review.mjs:44-118` |

### 3.3 知识与实践库层（references/ 四域，选核心）

| 编号 | 要点 | 证据 |
|---|---|---|
| C1 | **可观测性六道门**：discoverable / runnable / readable / correlatable / verifiable / safe+reversible，逐场景逐门返回；"logger 导入、调用计数、仪表盘、'日志少'只是搜索线索"；缺失段必须命名它阻塞了哪条链 | `references/project-harness/observability.md:63-84` |
| C2 | **恢复证据分级**：Present（有机制与 owner）→Wired（绑定受影响资源/入口/权限/后置条件/验证路线）→Exercised（安全演练/排练/实际恢复，记录前后态）；"**不得为提升证据强度而执行破坏性回滚**"；文件名匹配不足为证 | `references/project-harness/recovery-evidence.md` |
| C3 | **Loop Discovery 十项决策门**：重复意图/既有覆盖/稳定输入/可复用程序/验证/停止条件/安全边界/状态契约/可观测性契约/评估契约——缺则返回 `Needs more evidence`；"不得从文件年龄、行数、churn、缓存路径、标题或计数推广循环" | `references/loop-engineering/loop-discovery.md:94-123` |
| C4 | **最小持久所有者路由**：covered→Skill→automation→hook/rule→script→command→custom agent→MCP→skip，"优先最小持久所有者，不默认 Skill 或自动化"；runtime-fit 六分类（workflow/agent/evaluator-optimizer/scheduled/human-gated/skill-shaped）；"确定性 workflow 能给更紧控制时，不得推广自治代理" | `loop-discovery.md:63-92,144-165` |
| C5 | **学习回路检测模式**：10 个模式各带必需签名 + **负面控制**（如 `recurring-correction` 的反例是"两个无关验证失败"）；复发默认阈值 = 2 个独立可比 Task Episode；覆盖原因码（`checked-clean`/`insufficient-episodes`/…）即使无候选也必须输出；"`checked-clean` 只表示该检查有可用证据且无候选，不表示整个仓库干净" | `references/loop-engineering/learning-loop-patterns.md:49-96` |
| C6 | **来源（provenance）四分类**：`host-observed` / `deterministic-derived` / `ai-reviewed` / `unavailable`，每个归一化信号必须记录来源类别 | `learning-loop-patterns.md:31-37` |
| C7 | **verify loop 四平面**：Discover（覆盖来自数据非记忆；**intent 源是唯一能发现"承诺了但没实现"的源**）→Exercise（每例机器可触发，钉死 fixture/viewport/时钟/种子）→Judge（一个关联句柄驱动参数化探针，四值判定）→Scope（diff→case 匹配）；**覆盖天花板定理："验证覆盖以记录的意图为界；没写下来的行为循环不会检查"** | `references/project-harness/agent-verify-loop.md:70-167` |
| C8 | **core-change-watch 纪律**：静态收集器不得启动代理；"推断的核心边界"措辞强制留痕；历史热路径须与现存文件求交后才可作当前编辑目标建议 | `references/project-harness/core-change-watch.md:10-32,100-101` |

### 3.4 技能 / 钩子 / 资产层

| 编号 | 要点 | 证据 |
|---|---|---|
| D1 | **主 Skill 是路由器**：frontmatter 触发条件 + 短 body + 一跳 references；文档链接图测试强制"每个 reference 必须从 SKILL.md 路由可达，否则代理永远加载不到" | `AGENTS.md`（Doc Link Integrity）、`test/skills-docs/doc-link-graph.test.mjs` |
| D2 | **Stop 钩子发现信封**：`{kind, status: ok\|findings\|skipped, summary{findings,errors,warnings,advisories,maxSeverity}, findings[]}`；默认源"离线、确定性、非阻断、跨平台"；扩展规则禁止把复用逻辑放钩子目录 | `hooks/review-trigger/README.md` |
| D3 | **资产舒适区阈值**：核心指令文件 1—3 个且根文件 <200 行；repo Skills 3—10；用户级 Skills 10—30（需显式授权）；单 MCP 服务器高价值工具 5—15 | `hooks/review-trigger/README.md`（Candidate Asset Threshold Source） |
| D4 | **test-mapping 分级门**：`pass/remind/block` 三档；仅 critical 路径 + high/medium 置信度才 block；未知映射在 critical 路径只 remind | `hooks/git-scripts/test-mapping/policy.mjs:3-27` |
| D5 | **blast-radius**：tree-sitter 多语言解析（go/python/javascript-like）估计变更影响半径，供映射门与评审用 | `hooks/git-scripts/blast-radius/` |
| D6 | **五个自举技能**：skill-review（把 Skill 当执行契约审，P0 指向缺失/矛盾资源、P1 浪费上下文/重复所有权、P2 风格）；harness-skill-creator（最小技能图 + 本地验证器判定通过而非模型输出）；triangulate-spec-review（≥2 个独立 AI 评审者同一提示，P1/P2 清零才接受；"评审者是证据不是权威"；不得接受平均分）；change-traceability-review（Story→Spec→commit→diff→tests→risk 链）；architecture-model-bootstrap（确定性骨架 + LLM 语义精化 + **人工确认保存**，"没有绑定到真实路径的元素不得保留"） | `.agents/skills/*/SKILL.md` |
| D7 | **change-traceability spec 契约**：`docs/specs/<yyyy-mm-dd>-<slug>.md`，六节（Traceability/Intent/Acceptance Scenarios/Non-goals/Plan and Tasks/Test and Review Evidence）+ `[NEEDS CLARIFICATION: ...]` 显式未知 + Status: Draft→Accepted→Implemented→Superseded 由可见证据驱动 | `.agents/skills/change-traceability-review/references/spec-contract.md` |
| D8 | **commit 契约**：Conventional Commits 基线 + 正文散文式提及 Story/Spec/Test 证据；"Spec: none 不是无需 Spec 的证明"；一个连贯 spec 任务一个聚焦提交 | `references/commit-contract.md` |

### 3.5 契约与互操作层

| 编号 | 要点 | 证据 |
|---|---|---|
| E1 | **宿主适配矩阵**：12 宿主 × 7 列（Positioning/Shell/ConfiguredAssets/SessionEvidence/DefaultOutput/Rules/Smoke）+ 生命周期处置表（native/manual/unavailable 逐宿主）+ 拆分触发条件（5 条）；"宿主差异只进矩阵与 capability-local provider，产品判断留在 canonical owner"；无原生生命周期契约的宿主以 `UNKNOWN_HOST` 拒绝而非借用他宿主路线 | `docs/adapters/README.md:38-93,330-342` |
| E2 | **Agent-friendly CLI 契约**：成功信封 `summary`（快速定位，永不是事实源）/`data`（schema 治理的事实）/`effects`（**实际观察到的**变更而非计划变更）/`verification`（`passed: null` 显式 not_run，不得假装未检条件通过）/`artifacts`/`request_id` | `references/project-harness/friendly-cli.md:188-253` |
| E3 | **错误信封双重试语义**：`retryable`（条件可能随时间/修复而成功）与 `safe_to_retry`（重复同一调用不会产生重复效果）分离；"远端写后超时可能 retryable 但不 safe to retry"；`remediation[]` 给受限修复动作 | `friendly-cli.md:255-302` |
| E4 | **变更协议 Plan→Apply→Verify→Recover**：plan 带 `plan_id`+digest+expiry；apply 拒绝过期/目标不匹配/digest 变化/前置失败；`--expect-hash` 防丢失更新；幂等键绑定 principal+command+target+input digest；"通用 `--yes` 只确认已界定计划，不得绕过授权/目标检查/漂移检查/不可逆策略/人工批准" | `friendly-cli.md:362-407` |
| E5 | **长操作 JSONL 事件流**：每行完整 JSON + schema version + 单调 sequence + 终态事件强制；断线客户端可从 operation ID 或 sequence cursor 恢复；"取消报告执行是否停止与哪些效果残留，不等同回滚" | `friendly-cli.md:326-360` |
| E6 | **证据类分离**：fixture/package/native-host/installed-application/deployed-site/post-publish 证据互不替代，"一类证据不满足另一类的验收门" | `docs/ARCHITECTURE.md`（DX Routing） |
| E7 | **三级命令受众**：workflow/advanced/maintainer，子命令受众不得宽于父级；机器清单默认完整并报告受众元数据 | `scripts/better-harness-cli/registry.mjs:339-347`、`docs/ARCHITECTURE.md`（Agent-Friendly CLI Contracts） |
| E8 | **生命周期命令只读**：plugin status/plan/verify "不读原始会话转录、不联系注册表、不改宿主设置、不注册 apply 路径"；mutation 步骤声明为外部 `host-plugin-state`，验证步骤声明为只读 `host-observation`，"消费者不会把验证命令误认为 CLI 执行的变更" | `docs/adapters/README.md:95-97`、`docs/ARCHITECTURE.md:70-72` |

### 3.6 治理层

| 编号 | 要点 | 证据 |
|---|---|---|
| F1 | **ADR 体系存在但全部挂起**：9 份 ADR（0001—0009）状态全部 Proposed、无一 Accepted；"Accepted 不自动授权实现，非平凡实现切片仍需 dated Spec + 验收场景 + 测试证据 + 风险评审 + 激活门" | `docs/adrs/README.md:12-35` |
| F2 | **spec-first 实例质量高**：`docs/specs/` 数十篇 dated spec；样例含 AC 编号场景 + 非目标 + 实现证据（"焦点契约测试通过 30 项；文档路由 5 项"） | `docs/specs/2026-07-21-tighten-loop-discovery-evidence.md` |
| F3 | **诚实声明纪律**："README 排位是展示选择，不是支持级别声明"；静态分析中 branch protection/required CI 标 `UNVERIFIED`；"本地 POSIX 通过不是 Windows 证据，以对应 GitHub Actions job 为权威回执" | `README.md`、`AGENTS.md`（Test and Verify） |
| F4 | **测试哲学**：断言行为不断言恰好包含它的文本；禁止"全仓 grep 禁词"型测试（"训练贡献者混淆字符串而非修复"）；"若只能 grep 观察，代码需要导出 seam 而非正则"；正则仅用于确为契约处（脱敏保证、冻结 help 文本） | `AGENTS.md`（Test and Verify） |
| F5 | **AI 作者/确定性 writer 分离**："AI 变更工作流产出的读者文案归该 AI 所有；确定性 writer 可验证、版本检查、持久化、投影，但不得撰写或翻译" | `docs/ARCHITECTURE.md:24-27` |
| F6 | **CI 为真 GitHub Actions 矩阵**（ubuntu/macos/windows × Node 22/24 + Rust 工具链跑桌面 crate 测试），但 action 引用 `actions/checkout@v6` 等**可移动 tag、未固定 SHA** | `.github/workflows/ci.yml:17-60` |

## 4. 与 WHCode 的快速对照

- **同构项**（理念已有、BH 更机制化）：信号链完备 ↔ "配置≠使用"+证据五态；Check 二值通过 ↔
  证据态分级；崩溃恢复幂等 ↔ record-fix-output 乐观锁；三方追溯 ↔ change-traceability-review。
- **WHCode 先手**：事件溯源全链路（幂等键/校验和迁移）、审计表不随清理删除、门禁判定程序化
  （BH 评分/发现筛选为 LLM 判断）、CI action 固定 SHA、SurfaceEntry 对等契约、零 unsafe、
  资源铁律、DuckDB 单文件 schema 版本唯一出处。
- **BH 先手**：证据五态语义、评审协议（独立证据域/调和冻结/质量门/硬上限）、finding 工程结构、
  修复独立复评、学习回路检测模式（负面控制/覆盖原因码）、Loop Discovery 决策门、六道门、
  恢复证据分级、CLI 信封语义（retryable/safe_to_retry、verification null）、文档链接图测试、
  五个自举技能（含多 AI 三角评审）。

批判、P0—P3 融合方案、不采纳清单与待验证项见 [critique-and-fusion.md](critique-and-fusion.md)。

## 修订记录

- **2026-10-28（首版）**：基于插件缓存 `0.7.0-alpha2` 快照全量直读（README/模型 4 篇/
  ARCHITECTURE/主 Skill 及 references 9 篇/.agents 五技能/hooks/适配矩阵/ADR 索引/
  AGENTS.md/CI/specs 样例/friendly-cli/agent-verify-loop/observability/recovery-evidence/
  loop-discovery/learning-loop-patterns/core-change-watch + Node 实现层子代理报告）。
  证据 commit 固定为 `b605532fcf1014bcb48d3e6940a5bc64e80cddd5`（= `0.7.0-alpha2`，
  2026-10-20，经 `local/better-harness-src` 克隆与文件树 diff 核对）。
