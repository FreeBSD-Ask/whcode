---
description: "Task list for WHCode — Rust 原生 LLM 编码 Harness"
---

# Tasks: WHCode — Rust 原生 LLM 编码 Harness

**Input**: Design documents from `/specs/001-whcode-harness/`

**Prerequisites**: plan.md (required), spec.md (required for user stories), research.md, data-model.md, contracts/, quickstart.md

**Tests**: 本项目的测试任务**非可选**：源文本《WHCode 技术任务书 v3.0》§15 要求每个 PR 更新正向与失败路径测试，§11 定义 31 项验收（V-001—V-307），§12.2 将测试夹具、基准脚本与稳定性报告列为交付物。

**Organization**: 任务按用户故事组织；US-n ↔ 里程碑 M-(n-1)（spec.md 优先级映射）。建议 MVP 为 US1（M0 可行性与基线），M0 未通过前不得进入大规模实现（任务书 §12.1）。

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

## Path Conventions

- 仓库根为 Cargo workspace；实现位于 `crates/<crate>/src/`
- 测试位于仓库根 `tests/{unit,contract,integration,crash,stability,fixtures}/`
- CI 与基准脚本位于 `ci/{platform-matrix,benchmarks}/`；文档位于 `docs/{adr,matrices,guides}/`

## 全局执行前置：知难行易（任务认知卡）

本条款适用于**全部任务**（T001—T220，含已勾选与未勾选），依据宪法原则 X 与
`docs/guides/knowing-before-doing.md`：

1. 开工前必须写任务认知卡：方向、目标、用户意图复述、非目标/边界、现有证据、手段（后置）；
   歧义处标 `[须提问]`，不得自行假设后大规模实现。
2. **已勾选任务不补写**：以其来源 FR、验收判据（Check）与实现记录作为最小“知”证据；禁止
   事后补写卡片冒充历史证据（NFR-220）。
3. **当前未勾选任务**（T099、T116、T207—T212、T215—T219）开工前写卡，任务块末标注
   （开工前置：任务认知卡，见 `docs/guides/knowing-before-doing.md` §3）；未写卡不得开工、
   不得勾选。
4. 执行中若发现“知”有误，回到认知卡修正方向与目标，不在错误方向上加速。

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: 工程初始化与基础结构

- [x] T001 建立 Cargo workspace 根 `Cargo.toml`，声明 13 个成员 crate（whcode-domain/app/llm/store/speckit/tools/runtime/tui/cui/web/ssh/platform/bin）
- [x] T002 [P] 固定工具链：新增 `rust-toolchain.toml`（版本占位，由 T021 以 M0 实测锁定值回填）
- [x] T003 [P] 统一格式与静态检查：新增 `rustfmt.toml` 与 clippy 配置，CI 中 deny warnings（**边界（2026-10-23）**：流水线已迁入 `.github/workflows/platform-windows.yml`，fmt/clippy 步骤就位；本仓库无远程与 runner，平台执行记录未取得，见 Check 3）
- [x] T004 [P] 生成 `crates/*/src/lib.rs`（`whcode-bin` 为 `main.rs`）骨架与 crate 内模块占位
- [x] T005 [P] 新增五平台 CI 骨架 `ci/platform-matrix/{freebsd,openbsd,netbsd,dragonfly,windows}.yml`（**迁移（2026-10-23）**：五份迁至 `.github/workflows/platform-*.yml`——GitHub 只执行该目录；`ci/platform-matrix/` 保留依赖记录与交叉检查脚本）
- [x] T006 [P] 新增仓库根 `.gitignore` 补充项与 PR 清单模板 `docs/guides/pr-checklist.md`（§15 七项）
- [x] T007 建立文档骨架：`docs/adr/ADR-001..010.md`（未批准状态）、`docs/matrices/`、`docs/guides/`
- [x] T008 [P] 建立测试目录 `tests/{unit,contract,integration,crash,stability,fixtures}/` 与占位 README
- [x] T009 [P] 新增 `Cargo.toml` workspace 级共享依赖（serde/serde_json/thiserror）与 feature 组织（可选运行时 feature 默认关闭，FR-704；tokio 延后至 M2 引入，见 Notes）

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: 所有用户故事共享的核心类型与基础设施，必须先行完成

**⚠️ CRITICAL**: 本阶段完成前不得开始任何用户故事实现

- [x] T010 在 `crates/whcode-domain/src/state.rs` 定义四组状态枚举：任务（pending/ready/running/blocked/completed/failed/aborted/interrupted/unknown）、LLM 请求、运行时执行、SDD 阶段（data-model.md §2，§3.1）
- [x] T011 [P] 在 `crates/whcode-domain/src/event.rs` 定义事件类型枚举与 payload 上限、持久化等级（contracts/event-schema.md，FR-201）
- [x] T012 [P] 在 `crates/whcode-domain/src/error.rs` 定义结构化错误（`code`/`message`/`retryable`/`details`）与含 `schema_version` 的输出信封（contracts/cui-cli.md §3）
- [x] T013 [P] 在 `crates/whcode-domain/src/gate.rs` 定义门禁结果与阶段转换记录类型（来源阶段、目标阶段、执行者、校验结果、时间戳）（§3.1）
- [x] T014 在 `crates/whcode-platform/src/{process,paths,resources,term}.rs` 建立平台抽象层，`cfg` 条件编译集中在 `whcode-platform`（NFR-101/102）
- [x] T015 [P] 在 `crates/whcode-app/src/queue.rs` 实现有界队列与满载语义（关键命令背压/失败，展示快照可合并）（§5.3、§7.3-2）
- [x] T016 [P] 在 `crates/whcode-bin/src/config.rs` 实现配置加载：工作区路径、数据库参数基线（`memory_limit` 默认 192 MB、`threads` ≤ min(4,物理核)、`temp_directory`）、追踪级别默认 `redacted`（§5.4、D-05）
- [x] T017 [P] 在 `crates/whcode-bin/src/logging.rs` 初始化日志与追踪，默认脱敏、禁止明文写入密钥（FR-203、§10.1）
- [x] T018 [P] 在 `crates/whcode-domain/src/audit.rs` 定义审计记录类型（`actor/action/target/detail_json/ts`）与写入钩子（§10.3）
- [x] T019 在 `tests/contract/test_layering.rs` 实现分层契约测试：前端 crate 不得依赖 `whcode-store` 写入路径（ADR-003/005，C-03 门禁）

**Checkpoint**: 核心类型与基础设施就绪，用户故事可开始并行实现

---

## Phase 3: User Story 1 - M0 平台可行性与基线冻结 (Priority: P1) 🎯 MVP

**Goal**: 平台可行性（ADR-013 后 Windows 11 amd64 为唯一验收目标，BSD 延后）、数据库冒烟、TUI 最小闭环、SSH 风险原型、事件写者原型、基线性能报告与 ADR 批准，冻结平台矩阵与性能预算

**Independent Test**: 按 quickstart.md §2 逐项执行，M0 退出判据 V-001—V-007 全部通过并留存证据

### Tests for User Story 1

- [x] T020 [US1] 在 `ci/platform-matrix/record-deps.sh` 实现五平台构建与依赖记录脚本，输出含 Rust/DuckDB/编译器/OS 版本（V-001）（Windows 已取得证据；四 BSD 脚本已就绪但无平台环境，证据阻塞）
- [x] T021 [US1] 在 `ci/platform-matrix/` 与 `rust-toolchain.toml` 回填 M0 实测锁定的工具链版本，产出 `artifacts/deps-<platform>.txt`（V-001）（Windows 锁定 1.99.1 + msvc；四 BSD 待各平台实测）

### Implementation for User Story 1

- [x] T022 [P] [US1] 在 `crates/whcode-store/src/smoke.rs` 实现数据库冒烟路径：创建、事务写读、关闭重启、迁移（V-002）（Windows 四步冒烟通过；BSD 阻塞）
- [x] T023 [P] [US1] 在 `docs/adr/ADR-DB-BUILD.md` 记录各平台 DuckDB 原生构建方式、补丁与上游状态（R-01/R-02、TBD-01）
- [x] T024 [P] [US1] 在 `crates/whcode-tui/src/lib.rs` 实现最小 TUI 闭环：启动、键盘输入、resize、退出、异常恢复（V-003）（逻辑层测试通过；管道 stdin 挂起缺陷已修复并有自动冒烟脚本 `script/tui_smoke.py`；V-003 交互四项待人工）——**2026-10-23 补**：V-003 改由真实控制台驱动完成（`script/tui_v003.py`，10 项检查全过，证据 `artifacts/tui-v003-2026-10-23/`）；同轮修复“歧义宽度字符缺字”缺陷：新增 `crates/whcode-tui/src/rowbackend.rs`（整行连续输出 + 运行时实测字形宽度 + 溢出行末格锚定），58 例单测通过
- [x] T025 [P] [US1] 在 `crates/whcode-ssh/src/proto.rs` 实现内置 SSH 客户端原型：公钥认证、连接与执行受限命令、取回输出、超时与输出上限、按 ADR-007 登记（FR-611；原为四 BSD 服务端原型，已按 ADR-011 修订为客户端原型）（服务端验收属 M7；V-004 未取得证据）
- [x] T026 [US1] 在 `crates/whcode-store/src/writer.rs` 实现事件写者原型：幂等键、批次提交、关键事件确认、写入错误传播（V-005、FR-502）（14 例测试通过：含跨分块 seq 连续、跨 flush 去重、超 256 键去重、失败重试后 seq 无歧义；`flush_entries` 已改为批量插入：256 键/查询定位已存在键、64 行/语句多行 VALUES、插入前分配连续 seq，不再回读；实测吞吐 6 921 条/秒 @192MB，见 `artifacts/baseline-192mb.json`）
- [x] T027 [US1] 在 `tests/stability/` 与 `ci/benchmarks/` 实现基线基准夹具：固定硬件/数据集/采样方法并记录测量纪律字段（V-006、R-12）（已产出 `crates/whcode-bin/src/bench.rs`、`ci/benchmarks/run-baseline.py`、`tests/stability/baseline_check.rs`；夹具内存参数已按 ADR-012 更正为 §5.4 基线 192MB。首批实测 158 条/秒；写者优化后复测中位数 **6 921 条/秒**（超 NFR-210 候选值）；**预算已按 ADR-025 冻结（2026-10-23，Windows 11 amd64）**，跨平台与 72 小时数据仍缺；24/72 小时稳定性测试未实现）
- [x] T028 [US1] 在 `docs/matrices/platform-matrix.md` 与 `docs/adr/` 冻结平台矩阵与性能预算，或产出修订 ADR（V-006、D-01/D-06）（已产出平台矩阵与 ADR-011；**性能预算经 ADR-025 冻结（2026-10-23）**：范围 Windows 11 amd64、冻结值＝spec 候选值、NFR-208/210 实测达标、其余八项保持候选并登记责任人与退出条件；`platform-matrix.md` 冻结条件逐条标注）
- [x] T029 [US1] 在 `docs/adr/ADR-001..010.md` 补齐批准记录（决策、理由、约束、状态、责任人、退出条件）（V-007、C-02）（ADR-001/002/003/004/006/007/008/009 已批准；**ADR-005 与 ADR-010 为部分批准**，未决条件已写明）
- [x] T030 [US1] 在 `docs/matrices/decisions.md` 记录 D-01/D-02/D-03/D-04/D-06/D-12 的决策与证据（spec.md Assumptions）（D-01—D-13 全表；D-06 于 2026-10-23 改为“已冻结（Windows 11 amd64）”并链 ADR-025）
- [x] T031 [US1] 产出 M0 退出报告 `artifacts/m0-report.md`：五平台证据包、Go/No-Go 陈述、阻塞项清单（§11.1 退出条件）（**结论：M0 整体未通过**——Windows Go，FreeBSD/NetBSD 环境阻塞，OpenBSD/DragonFly 按 ADR-011 不适用；V-003/V-004/V-006 未通过；阻塞项 B-01—B-08 已列明解除条件）

**Checkpoint**: M0 通过后，平台与性能承诺方可冻结，后续里程碑可全面展开；任一硬目标失败须先解决或正式调整平台承诺

---

## Phase 4: User Story 2 - M1 存储与事件基座 (Priority: P1)

**Goal**: 版本化 schema 与迁移、单写者服务、游标分页、预聚合概览、备份/清理/压缩与数据库监控

**Independent Test**: 按 quickstart.md §3 执行；断线补拉、崩溃重启一致性、幂等重复、清理/压缩证据均可无 UI 运行

- [x] T032 [US2] 在 `crates/whcode-store/src/schema.rs` 与 `crates/whcode-store/migrations/` 实现 14 张表的版本化 schema 与可重复迁移（FR-501、data-model.md §1）
- [x] T033 [P] [US2] 在 `tests/integration/test_migration.rs` 实现“从上两个受支持版本迁移”测试，并验证迁移失败不误标新版本（NFR-103、§7.4）
- [x] T034 [US2] 在 `crates/whcode-store/src/writer.rs` 完成写者服务：有界队列、`idempotency_key`、批量阈值（默认 500 行/50 ms，可配置）、关键事件显式 flush、失败可重试（FR-407、FR-502/503、§7.3）
- [x] T035 [P] [US2] 在 `crates/whcode-store/src/query.rs` 实现稳定排序键 + 游标分页（双向比较方向明确，禁止大 OFFSET 默认路径）（FR-106、FR-504、§7.2）
- [x] T036 [P] [US2] 在 `crates/whcode-store/src/indexes.rs` 建立 events/messages/tasks 索引，并以基准验证查询计划（不规定全表扫描一律失败）（FR-511、§7.2、TBD-03/TBD-10）
- [x] T037 [P] [US2] 在 `crates/whcode-store/src/overview.rs` 实现 `session_overview` 预聚合与重建逻辑，标注强一致/最终一致字段（FR-506、§7.3-8）
- [x] T038 [P] [US2] 在 `crates/whcode-store/src/monitor.rs` 实现数据库统计、查询计划分析、WAL 状态与写者队列监控（FR-507）
- [x] T039 [US2] 在 `crates/whcode-store/src/purge.rs` 实现归档/按时间清理/TTL，含预览、确认、审计与活跃会话保护（FR-508/509）
- [x] T040 [P] [US2] 在 `crates/whcode-store/src/export.rs` 实现导出/导入：版本、工作区元数据、产物、事件与内容清单，大对象分块校验，导入前兼容性检查（FR-508、§7.4）
- [x] T041 [US2] 在 `crates/whcode-store/src/compact.rs` 实现维护锁 → 副本校验 → 原子切换（含 Windows 文件替换与 BSD 文件锁语义分别测试）（FR-510、R-09）
- [x] T042 [P] [US2] 在 `crates/whcode-store/src/wal.rs` 实现 WAL/CHECKPOINT 阈值、空闲窗口与显式维护触发（§5.4）
- [x] T043 [US2] 在 `tests/integration/test_durability.rs` 实现已确认事务的进程终止 + 重启一致性测试（V-104）
- [x] T044 [P] [US2] 在 `tests/integration/test_idempotency.rs` 实现重复幂等键测试：返回既有结果或确定冲突，不重复创建（V-106）
- [x] T045 [US2] 在 `tests/integration/test_space_report.rs` 实现清理/压缩空间统计测试：数据库/WAL/已用块/可用块，不承诺 `VACUUM` 释放空间（V-107、FR-510）

**Checkpoint**: 存储与事件基座可独立验证，M2—M4 具备挂载基础

---

## Phase 5: User Story 3 - M2 会话与任务应用服务 (Priority: P1)

**Goal**: 会话/任务命令、权限检查、状态快照与统一错误语义，四端可共享

**Independent Test**: 按 quickstart.md §4，以测试夹具直连应用命令验证（不经 UI）

- [x] T046 [US3] 在 `crates/whcode-app/src/session.rs` 实现会话命令：new/list/show/rename/archive/delete（FR-101）
- [x] T047 [US3] 在 `crates/whcode-app/src/session.rs` 实现会话切换：仅加载元数据、概览、上下文指针与有界活跃尾部（FR-102、C-03）
- [x] T048 [US3] 在 `crates/whcode-app/src/overview.rs` 实现任务总览：阶段分布、完成度、阻塞任务、活跃任务（FR-103）
- [x] T049 [US3] 在 `crates/whcode-app/src/task.rs` 实现任务详情：依赖、步骤、产物引用、最近事件、失败原因（FR-104）
- [x] T050 [US3] 在 `crates/whcode-app/src/authz.rs` 实现权限检查与一致的拒绝语义（`permission_denied`，`retryable=false`）（FR-604、§10.2）
- [x] T051 [US3] 在 `crates/whcode-app/src/snapshot.rs` 实现状态快照生成，字段契约见 contracts/state-snapshot.md，含 `event_cursor`（FR-314）
- [x] T052 [US3] 在 `crates/whcode-app/src/task.rs` 实现任务 retry/abort：重试计数、可重试失败不丢失会话、不可恢复错误向上传播（FR-401/408）
- [x] T053 [US3] 在 `tests/fixtures/gen_dataset.rs` 实现数据集生成器：10 万事件会话与 1 万任务（固定阶段分布）（V-101/V-102）
- [x] T054 [US3] 在 `tests/integration/test_session_switch.rs` 验证 10 万事件会话切换的有界加载与游标无重复/无遗漏（V-101、SC-001）；**2026-10-22 修订**：新增 `test_extreme_scale.rs` 极限测试（1M 事件，60GB 会话库同构访问模式，资源铁律 X1—X5）——用户判定初版 100k 夹具过于保守
- [x] T055 [US3] 在 `tests/integration/test_overview_perf.rs` 报告概览查询 p50/p95/p99 并对比冻结预算（V-102、SC-002）（2026-10-23 更新：ADR-025 冻结 NFR-208 后测试更名为 `overview_p95_within_frozen_budget_and_report_is_archived`，报告字段 `budget.frozen=true`）

**Checkpoint**: 应用服务闭环可经 CUI/测试夹具验证，三端可并行接入

---

## Phase 6: User Story 4 - M3 LLM 调用与上下文管理 (Priority: P1)

**Goal**: Provider 抽象、流式事件、四级追踪、预算化上下文注入与断线补拉

**Independent Test**: 按 quickstart.md §5，用桩 Provider 固定速率与 chunk 大小执行

- [x] T056 [US4] 在 `crates/whcode-llm/src/provider.rs` 实现 Provider 语义：能力探测、取消、流式结果、计量来源、超时与错误归一化（FR-201、§8.4）
- [x] T057 [US4] 在 `crates/whcode-llm/src/stream.rs` 实现流式事件发射与积压合并，保证关键状态转换不丢失（FR-206、R-04）
- [x] T058 [US4] 在 `crates/whcode-llm/src/trace.rs` 实现追踪级别 off/metadata/redacted/full 与秘密字段脱敏（FR-203、D-05）
- [x] T059 [US4] 在 `crates/whcode-llm/src/correlation.rs` 串联 `request_id/session_id/task_id/tool_call_id`（FR-204）
- [x] T060 [US4] 在 `crates/whcode-app/src/context.rs` 实现上下文加载器契约：`items + next_cursor + consumed_budget + truncated_reason`，有界迭代（FR-105/505、§4.1）
- [x] T061 [P] [US4] 在 `crates/whcode-app/src/context.rs` 实现上下文压缩与摘要指针，压缩后原始历史仍可查（FR-107）
- [x] T062 [P] [US4] 在 `crates/whcode-cui/src/context_cmd.rs` 暴露预算视图：当前预算、装入来源、截断原因（FR-108）
- [x] T063 [P] [US4] 在 `crates/whcode-llm/src/metering.rs` 记录 token/参数/延迟/成本，估算值标记 `cost_is_estimate`（FR-207）
- [x] T064 [P] [US4] 在 `crates/whcode-llm/src/trace.rs` 增加完整性标记：缺失 chunk、脱敏、降采样、持久化失败（FR-208）
- [x] T065 [US4] 在 `crates/whcode-cui/src/events_cmd.rs` 实现事件 JSON/JSONL 查询与导出，并在导出注明脱敏策略（FR-205）
- [x] T066 [US4] 在 `crates/whcode-app/src/replay.rs` 实现按持久化游标的补拉与重连（FR-202、FR-605）
- [x] T067 [US4] 在 `tests/integration/test_trace_and_context.rs` 验证脱敏、链路串联、预算截断与重连补拉（V-103、V-104）
- [x] T132 [US4] 在 `crates/whcode-llm/src/transport_events.rs` 实现传输层进度事件（FR-616）：连接建立/断开、重连尝试、按连接字节收发计数、往返时延、断连原因码，入事件体系并经 CUI JSON 可导出（M3 交付）

**Checkpoint**: LLM 与上下文能力可独立验证，SDD 与前端可接入

---

## Phase 7: User Story 5 - M4 SDD 门禁状态机 (Priority: P1)

**Goal**: 版本化工作流状态机、产物解析与校验、双向追溯、Clarify/Analyze 门禁、Converge 差异报告与人工放行审计

**Independent Test**: 按 quickstart.md §6，用 `tests/fixtures/sdd/` 违规产物夹具验证 V-201—V-207

- [x] T068 [US5] 在 `crates/whcode-speckit/src/workflow.rs` 实现版本化工作流配置与阶段定义（输入、必需产物、校验器、转换）（FR-302）
- [x] T069 [US5] 在 `crates/whcode-speckit/src/parser.rs` 实现 Markdown 解析器：报告无法解析段落、重复 ID、未识别字段，禁止静默丢弃（FR-303/309、R-06）
- [x] T070 [US5] 在 `crates/whcode-speckit/src/constitution.rs` 实现 Constitution 创建/导入、版本记录与变更审计（FR-301）
- [x] T071 [P] [US5] 在 `crates/whcode-speckit/src/ids.rs` 定义需求/任务稳定 ID 与引用校验（FR-304）
- [x] T072 [P] [US5] 在 `crates/whcode-speckit/src/trace.rs` 实现双向追溯：Plan 决策→需求、任务→需求与计划决策（FR-305）
- [x] T073 [US5] 在 `crates/whcode-speckit/src/analyze.rs` 实现遗漏需求、孤儿任务、无效引用、循环依赖、覆盖不足检出（FR-306）
- [x] T074 [US5] 在 `crates/whcode-app/src/gates.rs` 在应用服务中强制执行门禁：阻止 Plan/Implement 转换（FR-307/308、C-08）
- [x] T075 [US5] 在 `crates/whcode-speckit/src/parser.rs` 实现纠偏重试（默认 ≤2）后置 `blocked` 并保留原文与错误（FR-309）
- [x] T076 [US5] 在 `crates/whcode-speckit/src/converge.rs` 实现验收条件 `pass/fail/not_tested/not_applicable` 与证据引用（FR-310）
- [x] T077 [US5] 在 `crates/whcode-speckit/src/converge.rs` 生成差异报告，禁止静默标记完成（FR-311）
- [x] T078 [US5] 在 `crates/whcode-cui/src/spec_cmd.rs` 实现 `force-advance --reason` 与审计记录（FR-312）
- [x] T079 [US5] 在 `crates/whcode-app/src/stage_role.rs` 实现阶段角色与允许操作的显式注入（不依赖历史文本推断）（FR-313）
- [x] T080 [US5] 在 `crates/whcode-app/src/snapshot.rs` 保证阶段状态、门禁结果与产物版本对四端同源（FR-314）
- [x] T081 [US5] 在 `tests/fixtures/sdd/` 与 `tests/integration/test_sdd_gates.rs` 建立违规产物夹具与 V-201—V-207 验证用例

**Checkpoint**: SDD 门禁由程序实施，Implement/Converge 循环具备可审计的失败路径

---

## Phase 8: User Story 6 - M5 TUI 与 CUI (Priority: P2)

**Goal**: TUI 六视图与 CUI 命令树接入同一应用服务，四端能力对等由契约测试保障

**Independent Test**: 按 quickstart.md §7，同命令在 TUI/CUI 的最终状态、权限与错误码一致（V-109）

- [x] T082 [US6] 在 `crates/whcode-tui/src/views/` 实现会话树、任务、上下文、事件、Spec 状态、diff 预览（FR-601）
- [x] T083 [US6] 在 `crates/whcode-tui/src/render.rs` 落实渲染纪律：只读快照、脏标记重绘、不在帧路径反序列化 JSON（§5.3）
- [x] T084 [US6] 在 `crates/whcode-cui/src/cli.rs` 实现 clap 命令树（contracts/cui-cli.md §1）
- [x] T085 [P] [US6] 在 `crates/whcode-cui/src/output.rs` 实现 `--json` 输出：`schema_version` + 错误对象四字段 + 稳定退出码（FR-602）
- [x] T086 [P] [US6] 在 `crates/whcode-cui/src/confirm.rs` 为破坏性命令实现 dry-run/确认机制（§8.1）
- [x] T087 [US6] 在 `docs/matrices/feature-parity.md` 建立三端功能能力矩阵，并落实“新能力同 PR 定义 API/CLI/权限/测试”（FR-610）
- [x] T088 [US6] 在 `tests/contract/test_tui_cui_parity.rs` 实现 TUI/CUI 命令级一致性契约测试（V-109）
- [x] T089 [P] [US6] 在 `tests/contract/test_json_envelope.rs` 校验全部 CUI JSON 输出与错误对象契约（FR-602、§8.1）

**新增需求任务（2026-10-21，FR-612—FR-616）**：

- [x] T128 [P] [US6] 在 `crates/whcode-tui/src/theme.rs` 实现三套渲染主题（FR-612）：A 屏夜间（纯黑 #000000 背景）、墨水屏日间（纯白 #FFFFFF 背景 + 黑字）、墨水屏夜间（纯黑背景 + 白字）；主题经配置选择，测试断言背景色精确为纯黑/纯白（OLED 仅纯黑关断像素；e-ink 黑→白转换残影最重）
- [x] T129 [US6] 在 `crates/whcode-tui/src/render.rs` 强化渲染纪律（FR-613）：事件驱动 + 脏标记重绘已有基线，新增空闲期零重绘断言（重绘计数器）并接入 NFR-205 空闲 CPU 验证；文档写明“终端绘制由终端模拟器承担，WHCode 不直接调度 GPU”
- [x] T130 [P] [US6] 在 `crates/whcode-domain/src/i18n.rs` 与 `i18n/` 资源目录实现文本层（FR-615）：zh-CN 与 en 双语、gettext PO 加载接口、运行时区域检测、zh-CN→en 回退链；为 RTL 语言预留双向文本处理接口（渲染层不假设 LTR；依据：GNU gettext 已支持 Rust 提取）（2026-10-23 补记：`parse_po`/`Catalog::from_po`/`I18n::load_with_po` 落地，运行时读 `<root>/i18n/po/<locale>.po`（有界、语言标识白名单）；`whcode-bin::load_texts` 接线；Check 224 已通过）
- [x] T131 [US6] 在 `crates/whcode-app/src/residency.rs` 实现会话驻留策略（FR-614）：空闲会话卸载为“元数据+游标”、按需唤醒、卸载不丢已确认事件、卸载与唤醒可查询；禁止会话级预分配固定大缓冲（2026-10-23 补记：新增有界驻留缓冲（200 条 / 256 KiB，超限背压）+ `accounted_bytes` 占用统计；`tests/stability/test_residency_memory.rs` 实测 RSS 下降 31.48 MiB；Check 221 通过）
- [x] T133 [P] [US7] 在 `crates/whcode-web/frontend/` 实现动效总开关（FR-613 前端部分）：尊重 `prefers-reduced-motion`、默认关闭非必要动效、CSS 仅使用 GPU 合成友好的 transform/opacity（M6 交付）
- [x] T135 [US6] 在 `crates/whcode-platform/src/term.rs` 与 `crates/whcode-tui/src/render.rs` 实现 TUI 窗口最小化检测（Windows `GetConsoleWindow`+`IsIconic`）与渲染门控：最小化期间跳过绘制、关键事件继续记账，还原后单次全量重绘（FR-617）（2026-10-22 完成接线）：检测源经 ADR-021（PowerShell 子进程 P/Invoke，零 unsafe、零新依赖，本机实测 `Some(false)`）；门控消费端在 `tui/render.rs`（minimized 跳过绘制、关键事件记账、还原单次全量重绘）；**事件循环接线**（`tui/lib.rs`）：仅在有待绘制帧且输入静默 ≥1s 时探测，间隔 5s→60s 指数退避、检测到最小化回落 5s——空闲（无待绘制帧）与连续输入期间零探测，空闲成本为零；输入事件证明窗口可见（直接按还原记账）；重放证据 `run_scripted_steps`（探测计数/跳过计数/还原单次重绘均断言）
- [x] T136 [US7] 在 `crates/whcode-web/frontend/` 实现挂起状态机（Active/Suspended/Recovering）：`visibilitychange` + `blur/focus` 判定、恢复去抖 ≥250 ms、恢复时取快照并丢弃中间帧、WebGL/Canvas 上下文释放与重建、GPU 特性探测失败降级、`prefers-reduced-motion` 与动效总开关（FR-617）**2026-10-24 补充**：真实 Firefox 最小化取证发现残留 `focus` 会把挂起机误拉回 Active；focus 分支改为 `document.hidden` 为真时不恢复（可见性优先），hidden 3 s 零请求/零 DOM 变更/零 rAF，证据 `artifacts/web-runtime-2026-10-24/`（Check 227）
- [x] T137 [US7] 在 `crates/whcode-web/frontend/` 落实极简前端基线（ADR-014）：静态 HTML/CSS/原生 JS、服务端直出、无 node 构建链；新增 `ci/benchmarks/check-frontend-budget.py` 做体积与依赖审计（JS ≤ 50 KB gzip、CSS ≤ 20 KB gzip、无 WebGL/视频/自动播放、无第三方字体），并接入 CI 门禁（FR-618）**2026-10-24 补充**：首页增加服务端直出 `<noscript>` 只读快照（会话数 + `ID／标题／状态`，HTML 转义、超限写明截断、查询失败可见；仅未认证回环模式注入，认证模式不注入数据），见 Check 229 与 ADR-014 修订记录

**Checkpoint**: 本地双端交付，Web/SSH 可复用同一能力矩阵

---

## Phase 9: User Story 7 - M6 Web UI (Priority: P2)

**Goal**: 原生 Web UI 经 HTTP/WebSocket 接入应用服务，默认安全绑定与断线补拉

**Independent Test**: 按 quickstart.md §7，浏览器完成七类业务操作；未启用认证的对外监听被拒绝（V-109/V-110）

- [x] T090 [US7] 在 `crates/whcode-web/src/api.rs` 实现 HTTP API 与 WebSocket，复用应用服务命令/快照/事件（FR-603、ADR-004）
- [x] T091 [US7] 在 `crates/whcode-web/src/bind.rs` 实现默认 `127.0.0.1` 绑定；对外监听必须显式开启认证与 Origin 检查（FR-606）
- [x] T092 [US7] 在 `crates/whcode-web/src/security.rs` 实现认证、授权、Origin 校验、消息大小限制，token 不经 URL query（§10.2）
- [x] T093 [P] [US7] 在 `crates/whcode-web/frontend/` 实现会话、任务、历史、追踪、SDD、清理与运行时业务界面（FR-609）（2026-10-23 更新：写命令落地后七类齐备——会话 `POST /api/sessions/new|rename|archive|delete`、任务 `POST /api/tasks/retry|abort`、历史 `GET /api/events`、追踪 `trace_level`、SDD `GET /api/status`、清理 `POST /api/purge`（预览→确认）、运行时 `GET /api/runtimes`；前端 `index.html`/`app.js` 接线并新增“维护”页签；自动化证据见 `api::tests::{session_write_commands_default_to_preview,task_write_commands_retry_and_abort,purge_preview_then_confirm,write_routes_reject_bad_bodies_and_missing_capability}` 与契约 `tui_cui_parity` 第 4 段；**真实浏览器逐项点按记录未取得**）
- [x] T094 [P] [US7] 在 `crates/whcode-web/frontend/src/events.ts` 实现断线重连与按游标补齐（FR-605）（落点修订：ADR-014 不设打包器，前端为 `frontend/app.js` 单文件，无 `src/events.ts`；重连与游标续拉在 `app.js` 实现，浏览器实测见 `artifacts/web-browser-2026-10-22.txt`）
- [x] T095 [US7] 在 `tests/contract/test_web_security_defaults.rs` 验证 V-110 安全默认值与三端（含 Web）一致性（V-109）

**Checkpoint**: 三端对等闭环完成

---

## Phase 10: User Story 8 - M7 BSD SSH 接入 (Priority: P2)

> **范围状态（ADR-013，2026-10-21）**：Windows 11 amd64 为唯一验收目标，本阶段全部任务**延后**。
> 任务保留不删除；恢复时须新 ADR 并补齐 D-14 要求的证据链。**不得**勾选为完成。

**Goal**: 四大 BSD 上公钥认证、会话隔离、resize、安全退出与受限入口

**Independent Test**: 按 quickstart.md §7，四平台分别验收；任一失败记录明确阻塞原因（不以单一平台代替）

- [x] T096 [US8] 在 `crates/whcode-ssh/src/server.rs` 实现公钥认证、会话隔离、终端尺寸变化与安全退出（FR-607）（**2026-10-24 完成**：`server.rs`（1 456 行）以 russh 0.64.1 实现公钥认证（仅 Ed25519 + `authorized_keys` 清单）、每连接独立 `SessionContext`（通道上限 8）、pty-req/window-change 记录并随命令传给执行器、连接断开经 `Drop` 归还计数并写审计；`shell_request` 一律拒绝（FR-608，交互 PTY 属 M8）；进程内 10 项服务端测试（9 tokio + 1）覆盖认证成功/拒绝、越权命令、resize、并发会话、断连回收；`cargo test -p whcode-ssh` 41/41 通过。**未取得**：BSD 现场验收（属 T099／ADR-013 延后））
- [x] T097 [US8] 在 `crates/whcode-ssh/src/policy.rs` 实现受限 WHCode 入口；任意系统 Shell 为显式独立权限（FR-608、§10.2）（**2026-10-24 完成**：`policy.rs` 白名单 `sessions`／`session show|new`／`tasks`／`task`，默认 `SshPolicy::denied()` 拒绝 Shell，显式授权需 `allow_shell` 开关 **且** 调用者在 `shell_actors`，判定带能力校验与结构化错误码；5 项单测覆盖默认拒绝、双条件授权、越权与用法错误。**注**：本节指服务端入口；内置 SSH 客户端的默认命令策略同批次收口（首 token 白名单 + 显式 `allow_shell` 审计，见 Check 97））
- [x] T098 [P] [US8] 在 `crates/whcode-ssh/src/adapter.rs` 复用应用服务，去除每会话独立数据库写入路径（ADR-005、C-03）（**2026-10-24 完成**：`adapter.rs` 单线程 `whcode-ssh-exec` 执行器持有装配根注入的唯一 `Connection`（`serve.rs` 只 `open_workspace_db` 一次），命令经有界队列（64）+ 一次性回执 + 30 s 超时；`three_concurrent_ssh_sessions_write_through_single_connection` 证明 3 个并发会话全部经同一连接落库；非测试代码无 `Connection::open`）
- [ ] T099 [US8] 在 `artifacts/ssh-acceptance/` 产出四 BSD 的认证/尺寸/断连/权限边界证据与阻塞清单（V-004、R-02）**未取得（2026-10-24 复核）**：`artifacts/ssh-acceptance/` 不存在；ADR-013 将 BSD 全部延后，本机无平台环境（M0 阻塞项 B-01），恢复须新 ADR 并补分平台证据（开工前置：任务认知卡，见 `docs/guides/knowing-before-doing.md` §3）

**Checkpoint**: 五平台接入形态齐备

---

## Phase 11: User Story 9 - M8 可选执行运行时 (Priority: P3)

**Goal**: Runtime Provider 与子进程 worker、能力等级、独立 payload、能力不足结构化错误与副作用恢复

**Independent Test**: 按 quickstart.md §8，验证 V-301—V-307

- [x] T100 [US9] 在 `crates/whcode-runtime/src/provider.rs` 实现 Provider 接口（`name/capability/is_available/env_fingerprint/execute/terminate`）（FR-701）
- [x] T101 [US9] 在 `crates/whcode-runtime/src/capability.rs` 实现能力等级 `Full/Restricted/ShellOnly` 并接入界面与门禁展示（FR-702）（2026-10-23 补记：新增 `whcode-bin::runtimes` 清单，经 `doctor --json`/`doctor` 人类输出/TUI 摘要 `tui.runtimes`/Web `GET /api/runtimes` 四处可见；界面路径不启动子进程）
- [x] T102 [US9] 在 `crates/whcode-runtime/src/payload.rs` 实现 payload 独立分发与来源/版本/哈希/平台校验（FR-703、D-07）（2026-10-23 补记：新增 `SourceAllowlist`（默认拒绝）与固定顺序四重校验，来源不明返回不可重试的 `permission_denied`）
- [x] T103 [US9] 在 `Cargo.toml` 与 `crates/whcode-runtime/src/lib.rs` 落实 feature 门控：未启用运行时不加载动态库、不占端口、不注册工具（FR-704）
- [x] T104 [US9] 在 `crates/whcode-runtime/src/worker.rs` 实现受控子进程：超时、输出上限、取消、可用资源限制（FR-705）（2026-10-23 补记：新增启动前凭据环境剥离 `strip_credentials`、协作式取消 `execute_with_cancel`（`AtomicBool` 轮询 ≤10 ms，`cancelled` 与超时区分）与 `ConcurrencyGate`（`Mutex`+`Condvar` 限并发、`GatePermit` RAII 归还，不新建线程）；**资源限制为“并发子进程上限”，OS 级 CPU/内存配额未实现**；Check 104 已通过）
- [x] T105 [US9] 在 `crates/whcode-runtime/src/checkpoint.rs` 实现执行前检查点与脚本哈希/输入哈希/运行时指纹/工作目录策略记录（FR-405、FR-706）（2026-10-23 补记：触发策略 `CheckpointPolicy`（定时/按步骤/危险操作前 + 保留数）落于 `whcode-domain::checkpoint`，运行时重导出；配置经 `Config.checkpoint` 加载期校验；调度器接线仍属 M8 装配，未接入 Provider 路径）
- [x] T106 [P] [US9] 在 `crates/whcode-runtime/src/cache.rs` 限制重复执行缓存仅用于纯函数或显式启用缓存的任务（FR-707）
- [x] T107 [US9] 在 `crates/whcode-runtime/src/errors.rs` 实现能力不足/缺失/不兼容的结构化错误，并接入 Analyze 阻止执行（FR-708/710）
- [x] T108 [P] [US9] 在 `crates/whcode-speckit/src/converge.rs` 支持绑定运行时测试（命令、环境指纹、退出状态、结果引用）（FR-709）
- [x] T109 [US9] 在 `crates/whcode-platform/src/process.rs` 实现 Windows 子进程树与 BSD 进程组/信号的终止清理，保留 stderr 尾部（FR-409、§10.2）（复核注记 2026-10-23：Windows 子进程树经 `taskkill /T /F /PID` 已实现并有用例；2026-10-23 补记：新增 `process_exists`/`child_pids` 与 `tests/stability/test_process_cleanup.rs`，Windows 零残留已自动化验证；**BSD 进程组分支仍未实现**——`terminate_tree` 在非 Windows 返回结构化错误并注明属 M7，不静默伪装成功；Check 201 因 BSD 证据缺失维持未勾选）
- [x] T110 [P] [US9] 在 `crates/whcode-runtime/src/js.rs` 接入受限 ECMAScript 引擎，Node API 缺失时返回能力不足（D-08、§4.7.1）（**2026-10-24 完成**：按 ADR-029（修订 ADR-024 的 D-08 条款，附基准与回退路径）接入 Boa 0.21.1，feature `runtime-js` 门控——默认构建零依赖零开销（FR-704/NFR-102，`cargo tree -p whcode-runtime` 无 boa）；引擎路径：`input` 冻结全局注入，`require`/`process`/`fs`/`net`/`http`/`child_process` 六名字注册为抛出“能力不足”的桩（Check 109/153），死循环经 `RuntimeLimits`（循环 1 000 000 次/递归 256 层）有界 + 墙钟事后判定 `timed_out`；固定 ECMAScript 测试集 9 例（含中文内容与中文标识符）全过；基准（同脚本 1 000 次求值）：debug 3 064.9 次/秒、release 28 579.9 次/秒，测试进程 RSS release 15.1 MiB；默认构建行为不变（诚实不可用）。声明保持 Restricted + 三项不支持项，指纹 `boa/0.21/js/restricted`，不表述为 Node.js）
- [x] T111 [P] [US9] 在 `crates/whcode-runtime/src/python.rs` 实现 Python 子进程 worker 与受限实现档位（D-09、§4.7.1）（**2026-10-24 完成**：worker 协议——子进程 `<interpreter> -I -c <引导>` 从 stdin 读单行 JSON 请求（一次性临时文件注入，请求上限 4 MiB），脚本输出重定向进 8 KiB 有界缓冲（超限写显式截断标记），结果/回溯封顶后以协议 JSON 返回，失败非零退出 + stderr；`PythonEnv` 记录解释器绝对路径/版本/实现名/ABI（本机实测 `.cp314-win_amd64.pyd`）/sys_prefix/C 扩展探测 bool（Check 111/154）；受限档位列五项不支持项，文档写明“承诺边界不是沙箱——子进程以当前用户权限运行，OS 级隔离未实现”；取消与超时可区分；输出上限、凭据剥离、并发闸门经 worker 既有机制）
- [x] T112 [P] [US9] 在 `crates/whcode-runtime/src/gitbash.rs` 实现 Windows PortableGit payload 校验与 Bash/coreutils/管道/重定向验证（V-304）（**2026-10-24 完成**：payload 校验转调 `payload::validate`（来源白名单/版本/平台/哈希四重；fnv64 定位意外损坏检测、防篡改须外部签名——失效条件注明）；`locate_bash` 候选 ≤9 项逐个实测 `--version`；Bash 执行 `bash -c` + stdin 临时文件注入 + worker 超时/输出上限/取消；ADR-024 五类探测全部真实取证（本机 bash 5.3.9：管道→`A`、重定向读回、含空格路径、超时终止 `timed_out=true`、目标命令 `git version 2.54.0.windows.1`）；execute cwd 固定临时目录，workdir_policy "temp" 名副其实）
- [x] T113 [P] [US9] 在 `crates/whcode-runtime/src/bsdshell.rs` 实现 BSD Shell provider，名称与能力不与 `gitbash` 混同（§4.7.1）
- [x] T114 [US9] 在 `tests/integration/test_runtime_acceptance.rs` 与 `tests/crash/test_side_effect_recovery.rs` 覆盖 V-301—V-307（含三时点中断 → `unknown`）（FR-406、FR-410）（**2026-10-24 完成**：V-301—V-303/V-305—V-307 由 `test_runtime_acceptance.rs` 六用例与 `tests/crash/test_side_effect_recovery.rs` 四注入点覆盖（2026-10-23 复核确认）；V-304 随 T112 补 `v304_gitbash_payload_and_bash_capability`（payload 四重正反 + 五类探测），V-301—V-307 全部有自动化用例）

**Checkpoint**: 运行时能力矩阵与限制可公开声明

---

## Phase 12: User Story 10 - M9 优化与发布 (Priority: P3)

**Goal**: 性能优化、24/72 小时稳定性、发布包与文档、兼容矩阵与已知限制

**Independent Test**: 按 quickstart.md §9，产出时间序列报告与发布清单核对结果

- [x] T115 [US10] 在 `crates/*/src/` 按冻结预算优化热点（先测量后优化，禁止 `Box::leak` 通用化）（NFR-201—210、§5.3）（**2026-10-24 完成**：测量战役补齐七项候选实测——NFR-201 空载 RSS 中位 23.12 MB、NFR-202 活跃 27.33 MB、NFR-203 切换 20 次增量 3.18 MB（夹具修正后复跑；原 0.32 MB 的测量含“第二会话 0 事件”缺陷，见 ADR-025 修订记录）、NFR-205 TUI 空闲 CPU 0.0%、NFR-206 限速流式 0.682%（满喂容量 ≈1.5M chunks/s/核佐证）、NFR-207 切换 p95 12.9 ms、NFR-209 单页 p95 19.8 ms；报告 `artifacts/nfr-budget-report.json` + `tests/stability/test_nfr_budget.rs`（NFR-203/206 为 `#[ignore]` 测量夹具，运行方式已注明）+ `script/measure_nfr_budgets.py`（真实进程采样，纪律字段齐备）。**唯一超标项 NFR-209（同夹具 p95 40.8 ms > 30 ms）经热点定位（EXPLAIN 证实 DuckDB 对多列 top-k 生成 SEQ_SCAN 而非索引扫描）并以两阶段分页优化至 p95 19.8 ms 达标——同夹具前后对比留档，无预算修订**；测量方法两处缺陷修正留档：NFR-205 须 AttachConsole 子控制台注入、NFR-206 须采样测试可执行文件而非 cargo 包装进程。NFR-204（72 h 斜率）属 T116 保持候选。分配器决策仍按 ADR-020 冻结）
- [ ] T116 [P] [US10] 在 `tests/stability/run_stability.sh`／`.github/workflows/long-run.yml` 执行 5.5 小时稳定性测试并采集规定指标（§11.5、ADR-033）**阻塞（2026-10-22）**：需 5.5h 连续运行环境；NFR-204 长时泄漏斜率未测量。**部分进展（2026-10-23）**：四类故障情形已纳入 `tests/stability/test_failure_modes.rs`（Check 206 通过）；长跑与指标时间序列仍未取得，任务保持未完成——**长跑重启（2026-10-24 15:02:04，进行中）**：13:05 启动的旧长跑经复核作废——工作负载为逐轮重建的短命 `extreme_scale` 进程，且 13:26 起工作区未提交改动导致编译失败，采样对象与 RSS 斜率不可归因，`timeseries.json` 从未落盘。新采集器 `tests/stability/test_stability_run.rs` 为**单进程**持续负载并按间隔自采样，逐行追加 `artifacts/stability-24h-20261024-150204/timeseries.jsonl`（中断不丢已采样本）；采集 RSS/CPU/线程/句柄/库与 WAL 大小/事件缺失/failed 任务/提交-查询-切换延迟，写者队列深度与最老队列项年龄显式 `null` 并附原因（直连 `Connection`，不经 `WriterService`，不以 0 冒充）。14:30 启动的首轮在首个样本前（14:53）因需空载运行全量性能测试而主动停止，无数据丢失；清理路径已用 40 秒小阈值冒烟验证（`deleted_total=50020`、`event_missing=0`）。**口径变更（ADR-033）**：窗口改为 5.5 小时，CI 入口 `.github/workflows/long-run.yml`；此前 24 h 本机运行仅作历史材料，不再作为门槛。**5.5 h CI 长跑尚未运行**——数据未落地前不勾选。**2026-10-25 本机 12 h 序列分析**：`artifacts/stability-24h-20261024-150204/` 已落盘 12 个样本（单 PID 12392、间隔 3 600 s、debug 构建），RSS 斜率三口径均为负（最小二乘 −0.611 MB/h、首末 −1.855 MB/h、前 5.5 h 子集 −3.954 MB/h），线程恒 17／句柄恒 172，`event_missing` 恒 0，清理路径生效（插入 1 536 600／删除 1 500 100）；但进程在 12.0 h 处被外部终止、无 `kind:"summary"` 收尾行，间隔与 ADR-033 的 300 s 不符且为 debug 构建——**仅作候选证据**（`nfr204-analysis.md`），任务仍不勾选，等 `long-run.yml` 的 5.5 h CI 运行。**2026-10-25 补记**：该本机 24 h 运行随后自然收尾（23 个样本 + `kind:"summary"`，summary 自报首末斜率 0.4867 MB/h，线程 17／句柄 172 全程不变），仍为 debug／3 600 s 间隔，不构成 Check 116 证据；CI 5.5 h 口径（run 40154046118）斜率 0.6987／0.943 MB/h 超预算的登记见 `artifacts/tech-debt-review-2026-10-25.md`。**CI 5.5 h 长跑已取得（2026-10-25）**：run `40154046118` success（main@dc4e5b3，02:20—07:53 UTC，job 19 860 s），artifact `stability-long-run` 含 65 样本与 `kind:"summary"` 收尾行；RSS 斜率首末 0.6987 MB/h、最小二乘 0.943 MB/h（**超 0.5 MB/h 预算**），线程 21／句柄 176 恒定、`event_missing` 恒 0；样本 28／56 为 400k 阈值清理周期（非进程重启）；写者队列深度／最老队列项年龄仍缺（夹具直连 `Connection`），构建为 debug（`long-run.yml` 未加 `--release`）。处置分析见 `artifacts/nfr204-disposition-2026-10-25.md`：0.5 MB/h×5.5 h＝2.75 MB 小于噪声 σ 3.17 MB，**判据在当前窗口分辨力不足**（预算源自 72 h 前提，ADR-033 缩窗未重导），建议“夹具口径修正→同口径复测→ADR-025 组合判据”，修订提议已入 ADR-025 待批准。任务维持不勾选——**ADR-025 组合判据已批准并触发同口径复测（2026-10-26，用户批准）**：修订提议转为正式修订记录（冻结值 ≤ 0.5 MB/h 不变，判定改为前置条件 + 五要素 + 三态结论）；夹具补齐包络（`memory_limit='192MB'`／`threads=min(4, 逻辑核数)`）与 summary 纪律字段（`build_profile`、引擎版本、包络回读值、样本 `pid`），8 秒冒烟验证字段齐备；新增 `.github/scripts/analyze-nfr204.py` 在 `long-run.yml` 程序化判定（合成序列验证通过／违反／无权判定／前置不满足四态退出码）；`long-run-request-2026-10-26.md` 已变更以触发 release+包络复测。CI 复测 verdict 未取得前任务不勾选——**根因定位（2026-10-27）**：该复测 run `40315332443` success 但 timeseries 未落盘：cargo test 将测试二进制 cwd 设为 whcode-tests 包根，相对 `WHCODE_STABILITY_OUT` 实际写入 `tests/artifacts/`（本机 8 s 冒烟复现）；回查三次 CI 长跑 artifact 均只含 run.log，此前“65 样本已取得”实为 stdout 样本行。`long-run.yml` 已改传 `github.workspace` 绝对路径，重跑待 request 变更触发（开工前置：任务认知卡，见 `docs/guides/knowing-before-doing.md` §3）
- [x] T117 [P] [US10] 在 `docs/adr/ADR-ALLOCATOR.md` 记录分配器决策：默认系统分配器，除非基准证明有益（D-11）
- [x] T118 [US10] 在 `ci/` 与 `docs/matrices/` 产出 Windows 11 amd64 发布包、平台兼容矩阵与已知限制说明（BSD 延后并在限制中注明；§12.2、§9.1、ADR-013） **完成（2026-10-23，交付批次）**：新增 `ci/package-windows.py`（白名单打包 + 仓库外临时目录解包运行验证），产出 `target/dist/whcode-0.1.0-windows-amd64.zip`；证据 `artifacts/install-verify-windows-amd64.txt`（三条命令退出码 0，`duckdb_engine_version = v1.5.6` 为运行期实测，临时工作区实际生成 1 060 864 字节库文件）；兼容矩阵与已知限制随包发布（`compatibility.md`、`platform-matrix.md`），BSD 延后写入已知限制第 8 条（Check 118 通过）；**修订（2026-10-28，ADR-043）**：打包与发布扩展到 Windows ARM64（`package-windows.py --target aarch64-pc-windows-msvc`、`check-release.py --arch`、`build-release.yml` 三档矩阵）；ARM64 包在原生 `windows-11-arm` runner 打包与安装验证，只作平台证据，不进入验收（Check 118 口径同步修订）；**修订（2026-10-29，ADR-047）**：发布链可重现构建——`whcode-bin/build.rs` 支持注入 `WHCODE_BUILD_STAMP`（合法透传、非法告警回退墙钟），`build-release.yml` 以 HEAD 提交时间导出 `SOURCE_DATE_EPOCH` 与版本戳，`package-windows.py` 由 `SOURCE_DATE_EPOCH` 驱动 `generated_utc` 与 zip 条目时间戳；同提交同机两次全新构建 zip SHA256 一致，跨机差异（zlib／MSVC）实测归因并明确排除（ADR-047 §1）
- [x] T119 [P] [US10] 在 `docs/guides/` 完成 CUI 手册、TUI 键位表、Web 部署与安全配置、灾难恢复说明（§12.2） **完成（2026-10-23，交付批次）**：`cui-manual.md`、`tui.md`、`web-deployment-security.md`、`disaster-recovery.md` 四份齐备；每份逐条给出实现位置（`文件:行号`），并把未实现/未接线能力单列（TUI 无命令面板与翻页键、Web 无 TLS/安全响应头/限速、无 restore 子命令、WAL 自动检查点未接入循环）；`docs/guides/README.md` 的“未交付”节改为交付记录（Check 119 通过）
- [x] T120 [P] [US10] 在 `docs/matrices/runtime-inventory.md` 产出运行时能力清单、payload 校验与许可证清单（§12.2）
- [x] T121 [US10] 在 `docs/adr/` 与 `docs/risk-register.md` 更新 ADR 与风险台账，并落实每 PR 清单检查（§13、§15）（落点修订：风险台账实际并入 `docs/matrices/decisions.md`（R-12 未解除、R-13 新增），`docs/risk-register.md` 未建立；本行保留原路径以便追溯）

**Checkpoint**: 强制验收全通过，剩余偏差形成公开限制说明

---

## Phase 13: Polish & Cross-Cutting Concerns

**Purpose**: 跨里程碑收尾与一致性加固

- [x] T122 [P] 同步 `docs/adr/` 与 D-01—D-14 的最终决策状态，消除 spec/plan 中的遗留 `[待验证]` 标记
- [x] T123 安全加固复查：默认脱敏、权限白名单、最小环境、审计保留策略、Web/SSH 边界（§10）（**2026-10-24 完成**：复查记录产物 `artifacts/security-review-2026-10-24.md`——五域逐项以“文件:行号 + 测试名”取证：脱敏默认（logging 7 例 + trace 四档 + redacted 默认）、权限白名单（authz require + 四端同源 + Web 写路由判权）、Web/SSH 边界（回环/token/Origin/WS 限额 + SSH 客户端 9 例；服务端 M7 如实登记不虚勾）；**本轮补齐两处缺口**：最小环境——`worker::apply_minimal_env` 白名单 8 项 + `env_clear` 禁用完整继承（2 个新测试含真实子进程 `cmd /C set` 验证），审计保留——`audit_records_survive_purge_and_export_excludes_them` 证明审计不随会话 TTL 清除且导出不携带；**显式登记 2 项缺口**：OS 级配额（T104 口径）、管理员显式审计保留策略配置面——均写明退出条件）
- [x] T124 [P] 按 `quickstart.md` 全流程执行一次端到端验证并记录产物（§10 验证记录要求） **部分完成（2026-10-23，交付批次）**：可执行子集全流程留证于 `artifacts/quickstart-run-2026-10-23.md`（30 条目：成功取证 20、预期失败 5、未取得 5、缺陷登记 1、异常 0）；同步修正文档 3 处（`status`／`task list`／`spec status|validate` 缺必填 `--session`；`db compact` 无 `--export`；`db purge --before` 需 Unix 毫秒）；登记缺陷 1 项（不存在的会话导出返回 `count=0` 且退出码 0，与“存在但无事件”不可区分）；**维持未完成**：交互式 TUI（V-003）、真实 SSH 节点（V-004）、72 小时长跑三项无替代证据（Check 124 未通过）（**2026-10-24 完成**：quickstart.md 全流程实跑取证 `artifacts/quickstart-run-2026-10-24/execution.log`——M0—M2/M6/M8 通过、baseline 复跑 p95 4185 µs、未实现步骤显式失败取证（run/spec validate/runtime list → not_wired、serve --ssh 未实现、bench session-switch 未知子命令）、72h 属 T116 阻塞；Check 124 同批转通过）
- [x] T125 [P] 契约兼容审查：事件 schema、快照 schema、CUI JSON 的版本与变更记录（§8.3、§15）
- [x] T126 清理与重构：clippy 清理、模块边界复核、删除未使用 feature 与死代码
- [x] T127 追溯审计：spec FR ↔ tasks ↔ checklist.md 三方引用完整性复核（FR-305/306）（2026-10-23 交付 `ci/traceability.py`：六条判据含缺口登记合规；输出“FR→任务缺 0，FR→验收缺 0，幻影 0，已登记缺口 3，无断链”；检查点家族（增量差分／同事务提交／恢复加载引用）的三个缺口以登记节承载，不在正文冒充覆盖）
- [x] T134 [P] 目录分类门禁：`tests/contract/test_layout.rs` 将 `docs/guides/repository-layout.md` 的五类分层编码为可执行检查（顶层条目白名单、crate 命名、tests 六类 + README、docs/ci 子目录、specs 命名、本地目录防源码混入），随 CI 与每 PR 运行（7 例检查全过，门禁首跑即抓到并清除误建目录；LF/编码策略与 SECURITY.md 白名单已并入）
- [x] T138 [P] 建立软件工程过程文档 `docs/guides/software-engineering-process.md`：迭代增量模型、阶段门禁 G0—G5（输入/活动/产物/退出判据）、单人项目下的评审替代机制、产物映射表
- [x] T139 [P] 建立需求工程规范 `docs/guides/requirements-engineering.md`：FR 模板、需求质量判据、三方追溯抽查方法、变更控制流程（以 ADR-012/ADR-013 为案例）
- [x] T140 [P] 建立设计原则与模块设计文档 `docs/guides/design-principles.md`、`docs/guides/module-design.md`：原则→可执行落点，13 个 crate 的职责/依赖/失败模式/测试位置
- [x] T141 [P] 建立 UI 设计规范 `docs/guides/ui-design.md`：三套主题精确色值、墨水屏残影约束、i18n/RTL、极简前端预算与最小化挂起
- [x] T142 [P] 将 2026-10-21 四项项目优先序落成可执行检查（NFR-211—214）：资源（内存/GPU/线程/句柄）基线测量与回归门禁、安全默认值复核、BSD 交叉检查保持、GNU 工具禁用审计；阈值在 M1 冻结前标 `[待验证]`
- [x] T143 [P] 内存安全与 Rust 实践门禁（NFR-215）：workspace lints 保持 `unsafe_code = "forbid"` 且全部成员继承；契约测试校验继承关系与 `mem::forget` 禁用；有界队列/缓冲与句柄释放测试接入 G3；长时泄漏仍归 NFR-204 稳定性测试
- [x] T144 [P] 建立核心哲学 `docs/guides/philosophy.md`，并把形式主义审查、正负和解、三重时间依据与 0 信任原则接入 G0—G5、设计原则、PR 清单与 AGENTS.md
- [x] T145 [P] 建立性能与并发工程规范 `docs/guides/performance-and-concurrency.md` 与 ADR-015：并发设计档案、一手文献门槛、测量→设计→复测流程、汇编例外与回退验证
- [x] T146 [P] 将自由、主体性与安全反教条落为可执行判据：退出/导出/离线/默认值可撤销进入验收；安全措施附威胁模型、证据、代价与失效条件；禁止暗黑模式与成瘾设计（NFR-218/219）
- [x] T147 [P] 将一刻哲学融入工程：建立 `docs/guides/yike-philosophy.md`，接入不伪造、反进步叙事、用户友好与文档时效判据（NFR-220—222），同步 AGENTS/设计/过程/PR 清单
- [x] T148 [P] 将工程准则应用至本项目：新增 `ci/check_counts.py` 计数门禁并接入 Windows CI；补证据层级、三次复查与可逆性规则；产出 `docs/guides/doctrine-application.md`（**接线（2026-10-23）**：计数与三方追溯（`ci/traceability.py`）均已进入 `.github/workflows/platform-windows.yml`；平台执行记录未取得，见 Check 249）

---

## Phase 14: 原生桌面 GUI（只读 v1，M10，Priority: P2）

**Purpose**: 按 ADR-026／ADR-027 落地原生 GUI 的只读版本；先立需求与预算，再写代码

- [x] T149 [P] 完成 GUI 框架选型实测并按 ADR-027 定案：同一屏内容比较 egui／iced（wgpu 与 tiny-skia）／Win32，记录 RSS、线程、句柄、首帧、依赖与 unsafe 面（证据 `artifacts/gui-spike-2026-10-23.md`；结论 iced + tiny-skia，`spec.md` FR-711—715 随此立项）
- [x] T150 新建 `whcode-gui` crate：只依赖 `whcode-app`，实现六视图只读骨架（会话选择、刷新、1-6 切换）；同步 `Cargo.toml` 成员、`test_layering` 成员与依赖方向、`repository-layout.md` 与 `plan.md`（FR-711）（**2026-10-23 完成**：只读视图模型下沉到 `whcode_app::view_source`（TUI 与 GUI 共用，前端互不依赖）；新增 crate 与 `whcode gui` 入门子命令；`test_layering` 6/6、`test_layout` 7/7；实跑证据 `artifacts/gui-run-2026-10-23/summary.json`）
- [x] T151 交付 GUI 自动化证据入口：窗口截图、按键注入、空闲与交互资源采样三件套，结果落 `artifacts/gui-*/` 并可重跑（FR-715）（**2026-10-23 完成**：`ci/gui-probe.py` 三套主题各截图、`3` 切视图与 `q` 退出码实测、空闲 10 s 资源采样；产物 `artifacts/gui-run-2026-10-23/`）
- [x] T152 GUI 资源与主题达标：字体按需加载、空闲零重绘、三套主题精确色值与灰阶占比量化，按 FR-712／FR-713／FR-714 判定（预算 RSS ≤ 96 MB、线程 ≤ 24、句柄 ≤ 256、空闲 CPU ≤ 0.2% 单核）（**2026-10-23 完成**：CJK 字体改为按族名使用系统字体（微软雅黑不可再分发，不随包发布子集，进程不读入字体字节），RSS 由 95.7 MB 降至 41.1 MB、私有 72.4→17.3 MB；空闲实测（200% 缩放、1100×900 物理像素＝99 万像素，与 1200×800 同量级）线程 24、句柄 244、CPU 0.00%；三套主题分别只出现 `#FFFFFF`/`#000000`、`#000000`/`#FFFFFF`、`#000000`/`#E6E6E6`，中间灰 1.39%／1.39%／1.62%（≤5%）；`t` 键循环切换不重启（三次回原位，逐帧色值比对）；最小化 10 s 的 CPU 增量为 0.000 s，还原后标题与画面一致；`cargo tree` 无 wgpu。**未取得**：125%/150% 缩放与 RTL 证据（本机仅 200% 缩放，v1 无阿拉伯语界面）；应用初始窗口未适配工作区，探针代为调整尺寸（缺口见 ADR-027 修订记录））
- [x] T153 字体资产与运行子集：四份 OFL 字体（Inter／Noto Sans CJK SC／JetBrains Mono／Sarasa Mono SC）完整文件与许可入库 `assets/fonts/`，`SOURCES.md` 记录上游、版本与 SHA-256；`ci/subset-fonts.py` 生成 GUI 子集（ASCII/Latin + GB2312 + 仓库文本字符），子集缺字回退系统字体（FR-716／ADR-028）（**2026-10-23 完成**：完整字体合计 40.76 MiB（Inter 0.39／Noto 15.68／JetBrains 0.26／Sarasa 24.43），许可原文四份入库；子集 7.68 MiB（0.15／3.40／0.14／3.98），字符集 8958 个码位）
- [x] T154 GUI 四类字体分派：按字符类别显式分段渲染（比例：拉丁 Inter／CJK Noto；等宽：拉丁 JetBrains Mono／CJK Sarasa Mono SC），不依赖回退顺序；加载四份子集并记录 RSS 与字形证据（FR-716）（**2026-10-23 完成**：`font_for`／`split_runs`／`rich_line` 落地，7 项单元测试通过；`ci/gui-probe.py` 三主题截图＋diff 预览等宽截图；空闲 RSS 71.8 MB、私有 40.3 MB、线程 24、句柄 243、CPU 0.00%，仍 ≤96 MB）
- [x] T155 TUI 字体声明：`whcode font install|status|uninstall` 用户级安装内置字体并检测终端配置，显式、可逆、不静默改写用户配置；文档说明终端字体不可由程序强制（FR-717）（**2026-10-24 完成**：`font install` 复制四份完整字体到 `%LOCALAPPDATA%\Microsoft\Windows\Fonts` 并写 HKCU 注册（reg.exe 子进程，零 unsafe、零新依赖），来源支持 `--from`／exe 旁／cwd 三级探测与 `--dry-run`（ADR-028 修订 1）；`status` 检测安装状态并只读解析 Windows Terminal settings.json 的 `font.face`；`uninstall` 完全可逆。真机实测（`artifacts/font-run-2026-10-24/`）：装前全缺 → 装后四份 complete → `reg query` 交叉验证 → 卸载恢复原状、不误删用户原有条目；实测抓出并修复 WT 包族名笔误（8wekybeld3zzbb → 8wekyb3d8bbwe）。`install` 只输出 WT 配置片段供用户自行粘贴，不改写用户配置；文档说明终端字形由终端模拟器控制、程序不可强制。**视觉证据（2026-10-24 用户豁免代改配置取证）**：声明 `face="Sarasa Mono SC"` 后 WT 中 TUI 的 CJK/拉丁字形与默认字体对照肉眼可辨（`artifacts/font-run-2026-10-24/tui-wt-sarasa-on.png` vs `tui-wt-default.png`），取证后已恢复原配置并卸载字体）
- [x] T156 检查点事务语义（G-01 清偿）：store 增量差分写入（delta_json ≤64 KiB、artifact 引用 ≤100、按 `DEFAULT_RETAIN` 裁剪有界）、任务状态 + 关键事件 + 检查点引用同一事务提交、按引用恢复加载元数据/产物/上下文游标（完整历史按需读取）；app 层 `transition_task_with_checkpoint`／`recover_task` 判权编排（FR-402/403/404）（**2026-10-24 完成**：`whcode-store/src/checkpoints.rs` 新增 `insert_checkpoint`/`task_transition`/`recover_from_checkpoint`（duckdb `Transaction`，失败整体回滚——三表回滚注入测试证明）；app 编排 TaskRetry/TaskRead 判权；20 次转移保留恰 10 行差分（FR-402）、注入失败三表零残留（FR-403）、恢复联查 stage_artifacts 与游标（FR-404）；同步撤销追溯缺口 G-01 登记，新增 Check 256—258）
- [x] T157 GUI 终端绿主题：新增 `terminal-green`（纯黑 `#000000` 底 + 荧光绿 `#00FF00` 字，OLED 兼容纯黑，2026-10-24 用户新增），`t` 键循环扩为四套、四步回原位；FR-713 修订（真灰口径）与探针/色值证据（FR-713）（**2026-10-24 完成**：`whcode-tui::Theme` 与 `whcode-gui::GuiTheme` 均加 `TerminalGreen`（配置体系统一四套）；`ci/gui-probe.py` 四主题取证（`artifacts/gui-run-2026-10-24/`）：terminal-green 截图只出现 `#000000`/`#00FF00` 精确两色、真灰 0.0001、`t` 四步循环回原位 exit 0，四主题全过；资源 RSS 69.9 MB、线程 24、句柄 241 仍在 FR-712 预算内。探针加固：标题连续两次采样一致才注入按键（修复冷启动按键落空）；FR-713 中间灰判据改真灰口径（R=G=B），单色三套新旧口径数值一致）
- [x] T158 清理机制：`ci/clean.py` 四 scope（target／temp／stray／script）白名单制清理临时文件、中间产物与垃圾文件——构建可再现性（target 可再生）、白名单优于黑名单、dry-run 默认 + --apply 显式、活跃保护（30 分钟 mtime 窗口 + cargo/rustc 进程检测）、凭据与验证链工具显式保留、证据留痕（--record）；规则更新：script/ 由“保留不清理”改为“按机制回收”（2026-10-24 用户更新）（**2026-10-24 完成**：首次执行释放 ≈55.9 GiB（target 归零、TEMP 2516→30、script 150→11，保留清单精确命中）；复核复验：机制对新增垃圾持续可见（并行重建 target 2.39 GB 与新测试残留 24 对象被 dry-run 正常列出），EOL/layout/追溯门禁全绿；过程记录 `artifacts/clean-run-20261024-summary.json`）
- [x] T159 [P] DuckDB 2.0.0 升级与功能适配：引擎 2.0.0 LTS + crate `=1.20000.0`、Windows 资产改 tar.gz 安全解包、schema v6 与 `events.payload_json` VARIANT、`session_overview` MERGE、资源/查询计划/崩溃/安全/打包全面审计；不提供 1.5.6 旧库兼容（开发期直接重建），决策见 `docs/adr/ADR-034-duckdb-2-feature-adaptation.md` 与 `.trae/specs/migrate-duckdb-200/`（分阶段证据：`artifacts/duckdb-2.0-audit-2026-10-24.md`；2026-10-24 实现完成：安装验证 `v2.0.0`/schema 6、query_plan 2/8 索引扫描、NFR-201 经 ADR-035 修订为 36 MB、NFR-210 release 4 054.6 条/秒；NFR-204 5.5 h 长跑缺口已登记）
- [x] T160 [P] JSON 语义列全面 VARIANT 化与 2.0 能力逐项取舍：schema v7 将 `tasks.acceptance_json`、`stage_artifacts.refs_json`、`session_overview.by_stage_json`、`checkpoints.delta_json`、`checkpoints.artifact_refs_json`、`audit.detail_json` 改为 `VARIANT`；读写/导出/压缩统一显式转换，文本层上限与合法性校验保留；MERGE 复核（仅 overview）、数据修改 CTE A/B（purge 场景 +0.83%，不采用）、`variant_type`/`variant_keys`/`variant_extract` 健康断言与迁移验证；决策见 `docs/adr/ADR-036-json-columns-variant.md` 与 `.trae/specs/extend-duckdb2-adaptation/`（证据：`artifacts/duckdb2-usage-2026-10-24.md`）
- [x] T161 [P] 测试统一入口与 CI 合并：`platform-matrix.yml`／`stability.yml`／`action-version-audit.yml` 合并为 `.github/workflows/ci.yml`（门禁、五平台矩阵、全部测试、短测量、TUI/Web 冒烟、桌面入口、每周 action 审计），5.5 小时长跑保留为 `long-run.yml`；EOL、NFR 真实进程与测试进程资源脚本迁入 `.github/scripts/`，`local/` 只作复现；决策见 `docs/adr/ADR-037-ci-single-entry.md`（2026-10-25 用户指令：所有测试走 GitHub CI；桌面型测试按 headless／虚拟显示口径，不替代真实桌面人工证据）

---

## Phase 15: LLM 真实供应商（预留，未交付，Priority: P1）

**Purpose**: `add-real-llm-provider` 变更的任务位（ADR-031／D-16）。ADR-031 已于 2026-10-25
补录（`docs/adr/ADR-031-llm-provider-network-and-credentials.md`）；变更的**正式 spec 已于
2026-10-25 补登记**（`specs/002-add-real-llm-provider/`，状态：代码早于 spec，门禁倒置已由
ADR-031 记录）。以下 6 条按 FR-201—208 与 FR-616 及现存代码锚点登记，**标题与交付状态以
主账本为准**。T162—T166 已于 2026-10-25 收口；`llm run／cancel` 仅 CUI／Web 可用，
TUI／GUI 仍为 `M3-llm` 预留，**不得据此宣称四端可用**（T167 未勾选）。

- [x] T162 [US4] 真实 Provider 剖面与凭据边界：剖面模型、协议枚举、鉴权头构造与凭据不落日志／事件（FR-203；锚点 `crates/whcode-llm/src/http/profile.rs`）——**已完成（2026-10-25；见本 Phase 收口记录）**
- [x] T163 [US4] 阻塞式 HTTP 客户端与协议适配器：`ureq` 客户端、请求构造、响应读取与网络错误映射（FR-201；锚点 `crates/whcode-llm/src/http/mod.rs`）——**已完成（2026-10-25；见本 Phase 收口记录）**
- [x] T164 [US4] 响应归一化：流式／非流式响应统一为领域事件与用量统计（FR-206／207）——**已完成（2026-10-25；见本 Phase 收口记录）**
- [x] T165 [US4] 传输进度事件与脱敏：连接／重连／字节计数／往返时延／断连原因码入事件体系（FR-616、FR-203）——**已完成（2026-10-25；见本 Phase 收口记录）**
- [x] T166 [US4] 超时、重试与取消：单次调用超时、有界重试与取消语义（FR-201／202）——**已完成（2026-10-25；见本 Phase 收口记录）**
- [x] T167 [US4] 装配根接线与登记翻转：`http` feature 装配、凭据文件读取、`llm.run／cancel` 四端登记与矩阵同步（FR-208、FR-616；ADR-032 复审触发③）——**部分完成（2026-10-25）**：`http` feature、凭据读取、Provider 工厂与 `llm.run／cancel` 的 CUI／Web 面已接线；TUI／GUI 仍按 ADR-032 保持 `Reserved`，四端全翻转未完成，故不勾选——**收口（2026-10-26）**：TUI 接线完成——`L`／`C` 键位（剖面名→提示词两步输入、请求 ID 输入），经 `llm_provider::TuiLlmExecutor` 真实编排（与 CUI/Web 同一 `whcode_app::llm` 入口、同一权限与会话校验，连接与 TUI 写执行器共享同一条工作区连接）；登记表 TUI 格翻转为 `Implemented` 并补锚点，矩阵 feature-parity.md 同步，ADR-032 修订记录同批。GUI 按 `GUI_READ_ONLY_V1` 维持排除（写能力由 CUI/TUI/Web 承担，属登记的有意差异而非未交付）。TUI 键位测试 2 项（空数据源显式提示未选择会话、取消请求到达执行器），`cargo test -p whcode-tui` 69 项通过

**2026-10-25 收口记录（T162—T166）**：`crates/whcode-llm/src/http/{client,chunks,tls}.rs` 落地阻塞式 `ureq` 客户端、有界流式读取、chunk 序号归一、传输快照、时限／重试／取消与出站信任；`cargo test -p whcode-llm --features http` 77 项通过。端到端回环证据 `artifacts/llm-e2e-2026-10-25.md`：本地桩供应商（不访问公网）下 `whcode --cli llm run` 退出 0，正文“你好”、`completed=true`、`finish_reason=stop`、供应商用量与 transport 快照均出现；`llm.cancel` 对不存在请求返回结构化 not_found（退出码 60）。T167 保持未勾选。

---

## Phase 16: 四端对等与差异登记（M6／M10，Priority: P1）

**Purpose**: 按 ADR-032 落地能力登记表、受校验矩阵与四端补齐（`ensure-frontend-parity`）。
任务编号沿用变更内锚点（代码注释与 CHANGELOG 已引用），2026-10-24 收口时统一转正。

- [x] T168 [US6] 立项与决策：变更 spec、ADR-032、D-17 风险台账与预留 FR 编号（619—625）落档，契约同步标注“交付前不生效”（**2026-10-24 完成**：`docs/adr/ADR-032-frontend-parity-and-intended-differences.md`、`docs/matrices/decisions.md` D-17；正式编号于 T181 转正）
- [x] T169 [US6] 能力登记表：`whcode-app::surface` 定义 54 条 `SurfaceId` 的类别／能力／效果／确认／审计与四端状态，`validate` 自洽（**2026-10-24 完成**：`crates/whcode-app/src/surface.rs`；16 项内联测试）
- [x] T170 [US6] 四端清单：CUI／TUI／GUI／Web 各自只声明到 `SurfaceId` 的映射，不得自定义支持面（**2026-10-24 完成**：四个 crate 的 `surface_map.rs`）
- [x] T171 [US6] 对等门禁：登记表 × 四端清单 × 矩阵三向一致，含缺映射／孤儿动作／矩阵篡改／过期排除的负向用例（**2026-10-24 完成**：`tests/contract/test_frontend_parity.rs`；T181 收口把矩阵改为逐行比对）
- [x] T172 [US6] 统一不可用语义：预留／排除能力返回结构化 `not_supported`（CUI 55、Web 501、未登记仍 404），认证与 Origin 检查在先（**2026-10-24 完成**：`whcode-domain` 错误码、`cui_dispatch` 守卫、`whcode-web/src/api.rs`）
- [x] T173 [US6] CUI 读取面补齐：`context budget`（预算／来源／截断／游标）与 `runtime list`（与 Web `/api/runtimes` 同形）（**2026-10-24 完成**：`crates/whcode-bin/src/cui_dispatch.rs`）
- [x] T174 [US6] Web 读取面补齐：`/api/context`、`/api/diff`、`/api/events/export` 与前端标签（**2026-10-24 完成**：`whcode-web/src/api.rs`、`frontend/app.js`）
- [x] T175 [US6] Web 显示配置：四套主题（`data-theme` + `?theme=` + 页面选择器）与语言解析链（`ui.locale`→`WHCODE_UI_LOCALE`→`LANG`／`LC_ALL`→`zh-CN`）（**2026-10-24 完成**：`assets.rs`、`styles.css`、`app.js`）
- [x] T176 [US6] 写命令门面：`whcode-app::commands` 统一七命令执行入口，CUI／Web 迁移，预览／确认／审计／权限同源（**2026-10-24 完成**：`crates/whcode-app/src/commands.rs`）
- [x] T177 [US6] TUI 写操作：`n`／`m`／`A`／`d`／`R`／`X`／`p` 经门面执行，破坏性命令先预览后确认，无执行器显式 `not_supported`（**2026-10-24 完成**：`crates/whcode-tui/src/lib.rs`）
- [x] T178 [US6] TUI 主题与在线可见：`t` 四主题循环；`ViewSource` 游标查询与 ≥1000 ms 空闲检查，推进才重绘（**2026-10-24 完成**：`lib.rs`、`theme.rs`、`whcode-bin/src/store_source.rs`）
- [x] T179 [US6] GUI 只读对等：GUI 动作清单仅映射读面，写命令登记 `GUI_READ_ONLY_V1`，冻结 `GUI_MANUAL_REFRESH`（**2026-10-24 完成**：`crates/whcode-gui/src/surface_map.rs` + 契约断言）
- [x] T180 [US6] 行为等价：同一 purge 经 CUI 形状与 Web 形状执行，最终计数、审计次数与权限拒绝码一致（**2026-10-24 完成**：`test_frontend_parity.rs::purge_through_facade_has_identical_outcome_and_audit`）
- [x] T181 [US6] 收口：FR-619—625 转正主账本（本文件与 `checklist.md` 挂钩）、登记表 13 格随交付翻转为已实现、矩阵改为 54 行逐行受校验产物并补负向篡改用例、四份指南与 CUI 手册回填、证据归档（**2026-10-24 完成**：`docs/matrices/feature-parity.md`、`docs/guides/{tui,ui-design,web-deployment-security,cui-manual}.md`、`artifacts/frontend-parity-2026-10-24/`；遗留的 archive 确认／审计口径冲突在 CHANGELOG 已知未完成第 3 项登记，2026-10-25 由 T182 关闭）

---

## Phase 17: 写命令确认与审计补遗（Priority: P1）

**Purpose**: 关闭 `CHANGELOG.md` 已知未完成第 3 项——`session.archive` 的登记要求
（`PreviewConfirm` + `audited = true`）与实现不一致。裁决见 ADR-038：不放宽契约与登记表，
以实现补齐确认与审计。

- [x] T182 [US3] 会话归档确认与审计：四端统一“预览→确认”（CUI 默认 dry-run、`--yes` 执行；TUI `A` 两段确认；Web 缺 `confirm` 只预览；GUI 只读排除），新增 `session_archive` 审计并在 `set_archived` 同一事务写入（FR-623；ADR-038；**2026-10-25 完成**：`crates/whcode-store/src/purge.rs`、`crates/whcode-app/src/{commands,session}.rs`、`crates/whcode-bin/src/cui_dispatch.rs`、`crates/whcode-tui/src/lib.rs`、`crates/whcode-web/src/{api.rs,frontend/app.js,frontend/index.html}`；`docs/adr/ADR-038-session-archive-confirmation-and-audit.md`）

---

## Phase 18: 构建与发布独立矩阵工作流（Priority: P1）

**Purpose**: 按 ADR-039 把构建与发布从测试门禁中迁出；`build-release.yml` 以 profile 矩阵
承担工作区构建证据与发布链路，tag ref 时创建 GitHub Release；`ci.yml` 只保留门禁与测试。

- [x] T183 [P] 构建与发布独立矩阵工作流：新增 `.github/workflows/build-release.yml`
  （`profile: [debug, release]`；debug 做 `cargo build --locked --workspace` 与依赖记录；
  release 做发布构建、打包安装验证、SHA256、TUI/Web 冒烟与 artifact 上传；tag ref 时以
  `gh` 创建/更新 GitHub Release，校验 tag 与包版本一致、预发布标 prerelease、重跑 `--clobber`
  幂等）；`ci.yml` 删除对应构建与发布步骤；新增 `.github/scripts/check-release.py`；
  `action-versions.json` 登记 `actions/download-artifact`。决策见
  `docs/adr/ADR-039-build-release-workflow.md`；验收以 GitHub Actions 实跑为准（ADR-037）。
  **2026-10-25 完成**：build-release run
  `https://github.com/FreeBSD-Ask/whcode/actions/runs/40153942736` success——`package-debug`
  4m48s、`package-release` 7m05s，`release` 因非 tag 按门控 skipped；artifact 四件齐备
  （`deps-windows`、`whcode-windows-amd64` 29 821 700 B、`install-verify-windows-amd64`、
  `smoke-evidence`）。ci run `https://github.com/FreeBSD-Ask/whcode/actions/runs/40153942697`
  success——`platform-{windows,freebsd,netbsd,openbsd,dragonfly}` 与 `measurements` 全部
  success，`desktop`／`perf-budget`／`action-audit` 按门控 skipped。tag 发布路径未实跑
  （未创建 tag），已在 Check 273 登记缺口。

---

## Phase 19: 内置 TLS 证书库与 Web 证书设置（Priority: P0）

**Purpose**: 关闭“出站信任根在编译期写死、`serve --web` 只提供明文 HTTP”的现状：把信任什么
证书、用什么服务端证书、是否启用变为可见、可编辑、可关闭的工作区配置。决策与取舍见 ADR-040
（ADR-039 已被发布流程占用）。契约与需求见 FR-626／FR-627 与 `.trae/specs/add-tls-certificate-store/`。

- [x] T184 决策与追溯登记：新增 `docs/adr/ADR-040-tls-certificate-store.md`（威胁模型、备选方案、正负和解、三重时间、逐项依赖批准表）；ADR-031 追加“自定义 CA 与系统证书库由未支持转为受控支持”修订、ADR-023 追加“TLS 转默认”修订、ADR-022 补 HTTPS Origin scheme、ADR-032 新增理由码 `web_settings_only`；主 spec 修订 FR-606 并新增 FR-626／FR-627；`contracts/app-service.md` 增列 `cert` 命令面、`contracts/cui-cli.md` 的 `serve --web` 增 `--tls`；本文件登记 T184—T192、`checklist.md` 登记 Check 274—288 并同步计数声明（FR-626／FR-627）；验证：`python .github/scripts/traceability.py`、`python .github/scripts/check_counts.py`、`python .github/scripts/check-eol.py` 退出码 0
- [x] T185 内置 CA 资产与许可：`assets/certs/ca-bundle.pem` 原样入库（来源 `https://curl.se/ca/cacert.pem`）并记录下载日期、上游 SHA-256、证书份数；`assets/certs/SOURCES.md` 写明上游地址、版本日期、SHA-256、MPL-2.0 许可与原文片段、更新与复审触发；`docs/guides/repository-layout.md` 的 `assets/` 说明增列 `certs/`（FR-626）；验证：重算 SHA-256 与份数一致、资产无 BOM 且无私钥块、`python .github/scripts/check-eol.py`、`cargo test -p whcode-tests --test layout`
- [x] T186 领域类型、能力与审计动作：`crates/whcode-domain/src/tls.rs`（`TrustMode`、`InboundMode`、`TlsSettings`、`CertStatus`／`ServerCertStatus`、文件名与容量上限常量，serde 往返与两向默认值、未知档位解析失败）；`AuditAction::TlsConfigChange`（`tls_config_change`）；`whcode-platform` 的 `workspace_certs_dir()`；`whcode-app::authz` 新增 `Capability::SettingsCertRead`／`SettingsCertWrite`（`settings.cert.read`／`settings.cert.write`）并扩展 `KNOWN_CAPABILITIES` 与全能力 actor（FR-626／FR-627）；验证：`cargo test -p whcode-domain -p whcode-platform -p whcode-app`、`cargo clippy --workspace --all-targets -- -D warnings`
- [x] T187 工作区证书库编排（app）：新增 `crates/whcode-app/src/cert.rs`——初始化、状态读取、设置保存、CA 读取／写入／重置、种子清单与保真刷新、自签证书生成与重新生成、外部证书与私钥保存；校验 CA 仅接受 CERTIFICATE 块（出现 PRIVATE KEY 即拒绝）、份数与字节上限、PEM 可解析；同目录临时文件加原子替换，审计或替换失败回滚原字节（FR-626）；验证：`cargo test -p whcode-app`（种子／保真刷新／已修改保留／重置／空与超限／私钥块拒绝／非法 PEM／只读目录可操作错误／两工作区指纹不同）、`cargo fmt --all --check`
- [x] T188 出站信任装配（llm，`http` feature）：新增 `crates/whcode-llm/src/http/tls.rs`——由信任材料构造 `ureq::Agent`，`builtin` 用可编辑副本、`system` 经 `rustls-native-certs` 读取操作系统库，`ca-extra.pem` 在两档均追加；份数与字节有界；系统库为 0 时返回可操作错误且不回落内置库；不设置 `disable_verification`、不提供跳过校验入口（FR-626）；真实 Provider 调用仍由 T163／T167 接入，本任务不宣称 `llm run` 可用；验证：`cargo test -p whcode-llm --features http`（`rcgen` 本地自签夹具，测试不访问公网）
- [x] T189 入站 TLS 与非阻塞握手（web + bin）：`crates/whcode-web/src/tls.rs` 只持纯配置类型与状态投影（档位、上限、证书私钥校验结果与指纹；不引入 `rustls`）；`rustls::ServerConfig` 构造与非阻塞 `TlsSession`（明文与密文字节缓冲各有上限）落在装配根 `crates/whcode-bin/src/serve.rs`；`serve` 的 `--tls` 解析与优先级（参数 > 工作区设置 > 默认 `builtin`）、连接层抽象、握手沿用 5 秒空闲上限、`https` Origin 允许集与启动输出、默认档位失败拒绝启动（FR-626，分层依据见 ADR-040）；验证：`cargo test -p whcode-web -p whcode-bin`（分片 ClientHello、部分写、握手超时、外部证书档位、TLS 下 WebSocket 升级、`--tls off`、局域网无 token 拒绝），并记录前后线程与空闲内存数值
- [x] T190 Web 证书 API 与设置页：`crates/whcode-web/src/api.rs` 新增 `GET /api/certs`、`POST /api/certs/modes`、`GET|POST /api/certs/ca`、`POST /api/certs/ca/reset`、`GET|POST /api/certs/server`、`POST /api/certs/server/self-signed`、`POST /api/certs/verify`，请求体上限在解析前生效；认证与 Origin 复用 `security`、能力经 `whcode_app::authz::require`、降档要求 `confirm = true`、成功写 `tls_config_change` 审计且审计失败回滚；私钥只进不出；新增 `frontend/settings.html`／`settings.js`（原生 ESM）与首页入口、`zh-CN`／`en` 词条、无 JS 只读降级；`whcode-app::surface` 登记两项证书能力与 Web 路由映射并重新生成 `docs/matrices/feature-parity.md`（理由码 `web_settings_only`，FR-627）；验证：`cargo test -p whcode-web`、`cargo test -p whcode-tests --test frontend_parity`、`python .github/scripts/benchmarks/check-frontend-budget.py`
- [x] T191 装配根可见性与文档同步：`crates/whcode-bin` 在 `serve --web` 启动时初始化证书库并在默认档位失败时拒绝，TUI／GUI／CUI 装配时确保出站信任库可用并报告错误；`doctor` 文本与 `--json` 增加出站档位／份数／修改标记与入站档位／指纹／有效期；`README.md`、`docs/guides/cui-manual.md`、`docs/guides/web-deployment-security.md`、`quickstart.md`、`docs/guides/module-design.md`、`docs/matrices/decisions.md` 与 `CHANGELOG.md`（BREAKING：默认 HTTPS、Origin scheme 变化、脚本迁移）同步（FR-626／FR-627）；验证：`cargo test -p whcode-bin`、文档不再残留“Web 无 TLS”表述、`python .github/scripts/traceability.py`、`python .github/scripts/check_counts.py`
- [x] T192 验收证据、门禁与资源复核：真实运行取证（默认 HTTPS 与指纹一致、`--tls off` 明文、`external` 证书、局域网 token + HTTPS、设置页操作与 `restart_required` 提示）落 `artifacts/tls-2026-10-25/`；同夹具记录 `serve --web` 空闲与单连接 TLS 的 RSS／线程／句柄／CPU 前后对比且线程不增加；标准验证链全跑（`fetch-duckdb.py`、`cargo build --locked`、`fmt --check`、`clippy -D warnings`、`test --locked`、`layering`、`layout`、`traceability`、`check_counts`、`package-windows.py --verify`、`check-eol`、`check-workflows`、前端预算与资源测量）；CI 未实际运行时按 ADR-037 登记缺口（FR-626／FR-627）；验证：全部门禁退出码 0，证据可复现，未取得项显式标“未取得／阻塞”——**部分完成（2026-10-25 复核）**：T184—T191 已随提交 `94f970b` 与本次复核落地；`artifacts/tls-2026-10-25/` 五类场景与资源对比已取得，Check 274—283／285—287 有对应证据；证据已随提交 `be1559a` 入库，CI run `40225169689` 全部作业 success；——**收口（2026-10-26）**：浏览器实机证据已取得（`artifacts/tls-browser-2026-10-26/tls-browser-evidence.md`）：真实 Chrome 信任自签证书、页面指纹＝启动输出＝API 三方一致（8f874f21…d785b0d）、设置页预览确认与重新生成成功、提示区明示“需重启 serve --web 后生效”（restart_required 实机验证）；无凭据读取 permission_denied 反向证据同批。T192 转入勾选

---

## Phase 20: 四端界面现代化（视觉语言分层，Priority: P2）

**Purpose**: 在不增加内存与 CPU 占用的前提下把四端界面做得更现代，并让终端族（CUI／TUI）
与应用族（Web／GUI）使用两套可区分的视觉语法。决策见 ADR-041（ADR-039 已被发布流程占用，
ADR-040 属 TLS 变更）；变更规格见 `.trae/specs/modernize-ui-visual-language/`。

- [x] T193 [P] 视觉语言分层落档：新增 `docs/adr/ADR-041-ui-visual-language-tiers.md`（两族语法、
  色值与动效纪律、CUI 非颜色强调边界、TUI 装饰字符清单、资源中性判定口径与复审触发）；
  `docs/matrices/decisions.md` 增补 D-19；ADR-032 修订记录注明 `CUI_NO_THEME` 仍排除配色；
  `docs/guides/ui-design.md` 新增 §12 视觉语言分层与 §13 间距／字号刻度（FR-601／612／613／615；
  ADR-041）。**2026-10-25 完成**：编号按并行占用顺延为 ADR-041。
- [x] T194 [P] 结构化列投影（app）：`crates/whcode-app/src/view_source.rs` 新增 `ColumnRow` 与
  `render_columns_limited`（会话／任务／事件三视图；字段借用快照，行数上界沿用 `max`；
  `render_lines_limited` 行为不变）；单测覆盖列序、选中标记、派生数字列、`max = 0` 与万行快照
  有界性（FR-601；ADR-041 决策 8）。
- [x] T195 TUI 三区布局：`crates/whcode-tui/src/lib.rs` 的 `draw_frame` 改为顶栏（应用名、视图
  序号、当前会话）／左导航栏 + 主面板（单线边框、列标题、列对齐、选中反显、右侧滚动位置）／
  键位提示栏；CJK 按 2 列截断；极窄/极矮终端退化为纯正文；`render.rs` 的渲染门控与内容装配
  分离（`render()` 只回答“本帧是否绘制”）；`tests_support.rs` 增加带会话/反馈行的绘制；测试
  覆盖 24 行帧、导航栏 `>` 标记、CJK 截断、空态与英文文本层（FR-601／612／613／615；ADR-041
  决策 1／4）。帧预算 p95 1 913 µs（预算 16 000 µs）。
- [x] T196 [P] CUI 记录式输出：新增 `crates/whcode-cui/src/format.rs`（标签列 12 列的对齐记录、
  多记录成表、显式空态；ASCII 直通与 CJK 显示列；仅在交互终端、未设 `NO_COLOR`、`TERM` 非
  `dumb` 时启用粗体/弱化，绝不输出颜色）；`crates/whcode-bin/src/cui_dispatch.rs` 的
  `session list/show`、`task show`、`context budget`、`spec status` 改用记录／表格；`--json`、
  错误信封与退出码不变；`zh-CN`／`en` 新增记录词条（FR-602／615／625；ADR-032、ADR-041 决策 3）。
  **验证缺口**：`whcode-bin` 因并行未提交的 cert/TLS 工作无法编译，调用侧接线未取得编译证据
  （`format.rs` 4 项单测已通过）。
- [x] T197 [P] Web 应用外壳：`crates/whcode-web/frontend/index.html` 改为吸顶应用栏（标题 +
  七项 tablist + 主题选择）、内容与元信息两栏（窄屏单列）与常驻状态栏；`styles.css` 改为间距／
  字号刻度变量与逻辑属性（RTL）；四套主题色值不变；`crates/whcode-web/src/assets.rs` 新增外壳
  与 ARIA 双向关联断言（FR-603／612／618；ADR-014、ADR-041 决策 1／6）。
- [x] T198 Web 键盘与 ARIA：`frontend/app.js` 改为 `selectTab` + `wireTabs`（roving tabindex、
  方向键／Home/End 循环、RTL 方向反转、焦点可见），面板 `role="tabpanel"` 与 `aria-labelledby`
  关联；不新增定时器与事件订阅（FR-609／617；ADR-041 决策 6）。
- [x] T199 [P] GUI 原生外壳：`crates/whcode-gui/src/lib.rs` 侧栏改为可点击六视图按钮（选中反显、
  只用主题两色、无边框与阴影）、顶栏（视图标题 + 当前会话）、KPI 条（优先准备层摘要，否则可得
  计数）、正文与底部状态栏（键位提示 + 主题名）；保持 `listen_raw`、无定时器、无写命令；单测
  覆盖标题/状态栏、KPI 回落与侧栏样式（FR-711／713／714；ADR-041 决策 1／5）。
- [x] T200 [P] 前端预算门禁扩展：`.github/scripts/benchmarks/check-frontend-budget.py` 新增 DOM
  起始标签 ≤ 2 000 与 `innerHTML`／`outerHTML`／`insertAdjacentHTML` 禁令，并把体积、DOM 与
  注入检查拆为可自测的纯函数；`--selftest` 负向自检通过（FR-618；ADR-041 决策 7）。
  实测：JS gzip 8 781 B、CSS gzip 1 910 B、DOM 起始标签 73。
- [x] T201 资源中性实测与证据归档：TUI 帧预算（p50 1 513／p95 1 913／p99 2 021 µs，预算
  16 000 µs）、同机 legacy 单面板参照（p95 1 071—1 471 µs，debug 噪声 ±30%）、Web 预算与
  DOM 计数、四端结构测试结论落 `artifacts/ui-modern-2026-10-25/`；GUI 资源采样、主进程 RSS、
  Web 运行时冒烟因并行 cert/TLS 工作无法编译而**未取得**，按 ADR-037 登记缺口（FR-613／617／
  618；ADR-041 决策 8）。
- [x] T202 追溯收口：本文件与 `checklist.md` 登记 T193—T202／Check 289—298 并同步计数声明；
  `docs/guides/tui.md`、`docs/guides/cui-manual.md`、`CHANGELOG.md` 记录布局与人类可读输出
  变更（JSON 与退出码不变）；`python .github/scripts/check_counts.py`、`traceability.py`、
  `check-eol.py` 退出码 0。

---

## Phase 21: 入口默认值（Priority: P1）

**Purpose**: 用户指令（2026-10-25）：Windows 下无子命令时默认启动 GUI；`--cli`／`--tui`
显式选择入口，非 Windows 维持 TUI。

- [x] T203 [US6] 入口选择器与 Windows 默认 GUI：`crates/whcode-bin/src/main.rs` 新增
  `--gui`／`--cli`／`--tui` 全局选择器（可出现在子命令前后；两个不同选择器同时出现按用法
  错误退出 2；`--workspace`／`--config` 的取值不被误吃），无子命令时按
  `default_entry(is_windows)` 启动 GUI（Windows）或 TUI（其他平台）；默认 GUI 仅在存在交互
  控制台时建窗口，否则打印提示并以退出码 0 结束（保持 `tui-smoke.py` 第 3 项语义）；契约
  `contracts/cui-cli.md` §1、`docs/guides/{tui,cui-manual}.md`、`README.md`、`AGENTS.md`
  同步。**2026-10-25 完成**：`cargo test -p whcode-bin`（5 项选择器单测）；真实控制台实测
  默认启动 GUI（进程运行 5 s）、`--tui` 注入 `q` 退出码 0、`--cli --help` 退出 0、
  `--cli --tui` 退出 2、stdin 关闭时退出 0；Check 299。

---

## Phase 22: Agent Turn 循环编排器（P0-1，Priority: P1）

**Purpose**: 研究 `research/zcode-comparison/` §4 P0-1（2026-10-28 按 ADR-045 入库）：把 `llm run`
的单次模型调用升级为多轮 agent turn 循环（中止检查→预算检查→消息组装→模型 step→终止判定→
工具执行回填接线点），补齐“无 turn 循环即非编码 Harness”的结构性缺失。决策见 ADR-044
（2026-10-28 已批准）；实施计划见 `specs/001-whcode-harness/plan-p0-1-turn-loop.md`。
依赖：T167（llm 四端接线）、T034（事件写者）、T056+（事件流）；工具执行归 P0-2，不在本阶段。

- [x] T204 [P] 多消息契约：`crates/whcode-llm/src/provider.rs` 新增 `LlmMessage{role, content}`——**复核（2026-10-29，P0-1 契约层批次）**：实现与单测全绿（workspace build/fmt/clippy -D warnings/test 57 套件 0 失败、layering/layout 门禁通过，本机运行证据；CI 验收按 ADR-037 待远端流水线）。
  与四角色枚举（system/user/assistant/tool），`LlmRequest.prompt` 改为
  `messages: Vec<LlmMessage>`（非空校验上移调用方）；`StubProvider` 兼容单条 user 消息
  （默认行为不变）（FR-720；ADR-044 决策 3）
- [x] T205 [P] 五协议多消息映射：`crates/whcode-llm/src/http/protocols.rs` 五个 `request_body`——**复核（2026-10-29，P0-1 契约层批次）**：实现与单测全绿（workspace build/fmt/clippy -D warnings/test 57 套件 0 失败、layering/layout 门禁通过，本机运行证据；CI 验收按 ADR-037 待远端流水线）。
  由单条 user 消息改为 messages 数组映射（anthropic/gemini 按 system 段 + 交替轮次展开）；
  每协议新增角色与顺序单测（FR-720；ADR-044 决策 3）
- [x] T206 [P] `StubProvider` 多步剧本：支持可编程步序（每步 chunk 序列、finish_reason、——**复核（2026-10-29，P0-1 契约层批次）**：实现与单测全绿（workspace build/fmt/clippy -D warnings/test 57 套件 0 失败、layering/layout 门禁通过，本机运行证据；CI 验收按 ADR-037 待远端流水线）。
  tool_calls 列表），无剧本时保持现行单步行为（既有测试不破）；供 turn 循环无网络测试
  （FR-718 测试夹具；ADR-044 决策 4）
- [x] T207 turn 循环骨架：`crates/whcode-app/src/turn.rs` 新增 `run_turn`——前置校验复用——**复核（2026-10-30，P0-1 R1 批次）**：turn.rs 循环骨架/事件落表/预算接线实现，workspace fmt/clippy -D warnings/test 57 套件 0 失败（本机证据；CI 验收按 ADR-037 待本轮推送流水线）。
  `llm.rs`（权限 `Capability::SessionCreate`/会话存在性/剖面/凭据/请求 ID），循环执行中止
  检查→预算检查→消息组装→模型 step→终止判定；终止判定表：无工具调用（model_finished）/
  迭代上限 `MAX_TURN_ITERATIONS`（默认 16，可配置，iteration_limit）/取消（cancelled）/
  预算超限（budget_exceeded）/工具未接线（tool_not_wired）；单会话同进程活跃 turn 登记
  （RAII，冲突拒绝）；取消标记在每轮迭代与每个 chunk 前检查（FR-718／719／723／724；
  ADR-044 决策 4／7）（开工前置：任务认知卡，见 `docs/guides/knowing-before-doing.md` §3）
- [x] T208 事件与消息落表接线：turn 关键事件经 store 写者公共 API 落 `events` 表（kind 用——**复核（2026-10-30，P0-1 R1 批次）**：turn.rs 循环骨架/事件落表/预算接线实现，workspace fmt/clippy -D warnings/test 57 套件 0 失败（本机证据；CI 验收按 ADR-037 待本轮推送流水线）。
  domain `EventKind` 稳定名；payload 只含标识与计数，turn_id 贯穿，不含正文）；turn 内
  user/assistant/tool 轮次落 `messages` 表（>1 MiB 走引用或登记缺口，不静默截断）；信封
  `events` 数组 kind 收敛到 `EventKind` 稳定名（废除 `llm.*` 小写名）（FR-722；ADR-044 决策 5）（开工前置：任务认知卡，见 `docs/guides/knowing-before-doing.md` §3）
- [x] T209 预算检查接线：每轮迭代前经 `load_context` 检查上下文预算（FR-105 契约），预算——**复核（2026-10-30，P0-1 R1 批次）**：turn.rs 循环骨架/事件落表/预算接线实现，workspace fmt/clippy -D warnings/test 57 套件 0 失败（本机证据；CI 验收按 ADR-037 待本轮推送流水线）。
  不足以容纳下一轮时显式终止 turn 并把 truncated_reason 写入终止事件与信封（FR-721；
  ADR-044 决策 2；自动压缩归 P1-3，不在本阶段）（开工前置：任务认知卡，见 `docs/guides/knowing-before-doing.md` §3）
- [x] T210 信封与四端适配：`LlmRunOutcome` 新增 `turn_id`/`iterations`/`termination_reason`；——**复核（2026-10-30，P0-1 R2 批次）**：四端切 run_turn + 集成测试 + 文档同步全绿（workspace 57+1 套件 0 失败；本机证据，CI 验收按 ADR-037 以推送流水线为准）。
  TUI 结果视图、CUI `llm run` JSON 信封、Web `llm/run` 响应、SSH 输出四端同步；
  `surface.rs` 登记与对等契约门禁同步；`contracts/cui-cli.md` 同步（FR-718／722；ADR-044
  决策 1）（开工前置：任务认知卡，见 `docs/guides/knowing-before-doing.md` §3）
- [x] T211 测试集：正向（多轮至 model_finished、事件 seq 单调、turn_id 贯穿、messages 可重建）——**复核（2026-10-30，P0-1 R2 批次）**：四端切 run_turn + 集成测试 + 文档同步全绿（workspace 57+1 套件 0 失败；本机证据，CI 验收按 ADR-037 以推送流水线为准）。
  与失败路径（迭代上限/预算超限/tool_not_wired/取消/权限拒绝/未知会话/空提示词）测试；
  安全断言（事件与信封不含提示词/正文/凭据，FR-203 口径）与资源断言（单会话单活跃 turn、
  无新线程、无无界累积）（FR-718—724；ADR-044 决策 4／7）（开工前置：任务认知卡，见 `docs/guides/knowing-before-doing.md` §3）
- [x] T212 文档同步：spec.md FR-718—724、tasks.md 本阶段勾选、checklist.md Check 300—308、——**复核（2026-10-30，P0-1 R2 批次）**：四端切 run_turn + 集成测试 + 文档同步全绿（workspace 57+1 套件 0 失败；本机证据，CI 验收按 ADR-037 以推送流水线为准）。
  CHANGELOG（`llm run` 语义升级/信封字段扩展/事件命名收敛）与实现记录（FR-305 口径；
  测试验收入口按 ADR-037）（开工前置：任务认知卡，见 `docs/guides/knowing-before-doing.md` §3）

- [x] T213 [P] 工具注册表：`crates/whcode-tools/src/registry.rs` 登记稳定名/描述/参数——**复核（2026-10-29，P0-2 工具实现批次）**：实现与模块测试全绿（workspace fmt/clippy -D warnings/test 57 套件 0 失败、layering/layout 门禁通过；本机证据，CI 验收按 ADR-037 待流水线）。
  Schema 子集/副作用范围，重名冲突拒绝，provider 可见清单；输入校验一层（白名单+必填+
  类型+拒绝未知字段）（FR-725、FR-729；ADR-048 §2.1/2.7）
- [x] T214 [P] 内建工具六件套：`crates/whcode-tools/src/builtins/` read_file/write_file/——**复核（2026-10-29，P0-2 工具实现批次）**：实现与模块测试全绿（workspace fmt/clippy -D warnings/test 57 套件 0 失败、layering/layout 门禁通过；本机证据，CI 验收按 ADR-037 待流水线）。
  edit_file/list_dir/glob/grep——`std::fs` 实现、有界（读 ≤1 MiB、遍历与结果条目上限）、
  写工具产 diff 摘要（FR-726；ADR-048 §2.2）
- [x] T215 工具执行登记接线：`crates/whcode-app` 工具执行经 store 三步登记与——**复核（2026-10-30，P0-2 R3 批次）**：实现与测试全绿（workspace 58 套件 0 失败；本机证据，CI 验收按 ADR-037 待推送流水线）。
  recover_uncertain；ToolCallStarted/Finished 事件落表（payload 只带标识与计数）
  （FR-727；ADR-048 §2.4）（开工前置：任务认知卡，见 `docs/guides/knowing-before-doing.md` §3）
- [x] T216 turn 集成：模型 tool_calls 解析（openai 兼容族）→ 串行执行（每轮上限 8）→——**复核（2026-10-30，P0-2 R3 批次）**：实现与测试全绿（workspace 58 套件 0 失败；本机证据，CI 验收按 ADR-037 待推送流水线）。
  role:"tool" 回填 → continue；FR-724 的 tool_not_wired 分支替换（依赖 T207）
  （FR-728；ADR-048 §2.5/2.8）（开工前置：任务认知卡，见 `docs/guides/knowing-before-doing.md` §3）
- [x] T217 [P] 工具安全边界：工作区相对路径绑定（拒绝绝对/.. 逃逸）、authz 权限面、——**复核（2026-10-30，P0-2 R3 批次）**：实现与测试全绿（workspace 58 套件 0 失败；本机证据，CI 验收按 ADR-037 待推送流水线）。
  结果 64 KiB 截断标记（FR-729；ADR-048 §2.6/2.7）（开工前置：任务认知卡，见 `docs/guides/knowing-before-doing.md` §3）
- [x] T218 [P] 测试集：注册表冲突/校验拒绝、六件套正反路径、登记三步与恢复、解析回填、——**复核（2026-10-30，P0-2 R3 批次）**：实现与测试全绿（workspace 58 套件 0 失败；本机证据，CI 验收按 ADR-037 待推送流水线）。
  越界拒绝、截断标记；安全断言（结果不进事件）（ADR-048；layering 口径）（开工前置：任务认知卡，见 `docs/guides/knowing-before-doing.md` §3）
- [x] T219 文档同步：spec/tasks/checklist 勾选、CHANGELOG、ADR-048 §6 实施记录——**复核（2026-10-30，P0-2 R3 批次）**：实现与测试全绿（workspace 58 套件 0 失败；本机证据，CI 验收按 ADR-037 待推送流水线）。
  （FR-305 口径；测试验收入口按 ADR-037）（开工前置：任务认知卡，见 `docs/guides/knowing-before-doing.md` §3）

**Checkpoint**: 工具六件套经三步登记可被 turn 循环真实执行与回填（P0-2 闭环）

**Checkpoint**: turn 循环骨架可独立验证（桩剧本驱动），P0-2 工具系统具备挂载点
（tool_not_wired 路径替换为真实执行）

---

## Phase 23: 知难行易（宪法原则 X，Priority: P1）

**Purpose**: 从一刻哲学第 21 章 §21.2／§21.4 与四版前言提取“知难行易”，立为宪法原则 X（1.2.0，ADR-049），并以唯一正文、全局落点、程序门禁与全部任务适用条款落地。

- [x] T220 [P] 知难行易全局落地：新增 `docs/guides/knowing-before-doing.md`（唯一正文）与
  `docs/adr/ADR-049-constitution-knowing-before-doing.md`；宪法新增原则 X（1.1.1 → 1.2.0）；
  `AGENTS.md` 五处、`philosophy.md` §11、`yike-philosophy.md` 第 21 章与 §3、
  `software-engineering-process.md` G0/G1/§1.5/§5/§6、`requirements-engineering.md` §2.1/§6、
  `pr-checklist.md`、`docs/guides/README.md` 落点；`engineering-doctrine` 00/01 与五份模板；
  `tasks.md` 全局执行前置与 13 个未勾选任务标注；新增 `.github/scripts/check-knowing-first.py`
  并接入 `ci.yml`；`checklist.md` Check 316。**复核（2026-10-29，修宪批次）**：
  `check-knowing-first.py` 退出 0（18 个落点、13 个未勾选任务带标注）；失败路径实测
  （移除落点后退出 1，逐字节恢复后退出 0）；`check_counts.py`/`traceability.py` 退出 0；
  `cargo test -p whcode-tests --test layout` 通过；workspace 全量测试隔离复跑退出 0
  （首轮与 clippy/layout 并发时 whcode-store 套件异常退出，隔离复跑 115/115 通过）
  （本机证据；CI 验收按 ADR-037 待流水线）。- [x] T220 [P] 知难行易全局落地：新增 `docs/guides/knowing-before-doing.md`（唯一正文）与
  `docs/adr/ADR-049-constitution-knowing-before-doing.md`；宪法新增原则 X（1.1.1 → 1.2.0）；
  `AGENTS.md` 五处、`philosophy.md` §11、`yike-philosophy.md` 第 21 章与 §3、
  `software-engineering-process.md` G0/G1/§1.5/§5/§6、`requirements-engineering.md` §2.1/§6、
  `pr-checklist.md`、`docs/guides/README.md` 落点；`engineering-doctrine` 00/01 与五份模板；
  `tasks.md` 全局执行前置与 13 个未勾选任务标注；新增 `.github/scripts/check-knowing-first.py`
  并接入 `ci.yml`；`checklist.md` Check 316。**复核（2026-10-29，修宪批次）**：
  `check-knowing-first.py` 退出 0（18 个落点、13 个未勾选任务带标注）；失败路径实测
  （移除落点后退出 1，逐字节恢复后退出 0）；`check_counts.py`/`traceability.py` 退出 0；
  `cargo test -p whcode-tests --test layout` 通过；workspace 全量测试隔离复跑退出 0
  （首轮与 clippy/layout 并发时 whcode-store 套件异常退出，隔离复跑 115/115 通过）
  （本机证据；CI 验收按 ADR-037 待流水线）。

- [x] T221 流式重试 decorator：`crates/whcode-llm/src/retry.rs`——safe_to_retry——**复核（2026-10-31，P1-2 批次）**：decorator 修复（重启失败静默结束缺陷）+ 测试 7 项全绿（workspace 本机证据；CI 验收按 ADR-037 待推送流水线）。
  判据（零 chunk 已发射才重放）、退避（retry_after_ms 优先）、总预算上界、取消感知、
  attempts 并入 reconnect_attempts（FR-730；ADR-050 §2）
- [x] T222 重试模块测试：首 chunk 前重试成功、中途失败上抛、非重试不重试、——**复核（2026-10-31，P1-2 批次）**：decorator 修复（重启失败静默结束缺陷）+ 测试 7 项全绿（workspace 本机证据；CI 验收按 ADR-037 待推送流水线）。
  次数上界、重启失败上抛（回归）、策略校验（FR-730；ADR-050 §2.6）
- [x] T223 工厂接线与文档：`LocalProviderFactory::create` 包 decorator + ADR-050——**复核（2026-10-31，P1-2 批次）**：decorator 修复（重启失败静默结束缺陷）+ 测试 7 项全绿（workspace 本机证据；CI 验收按 ADR-037 待推送流水线）。- [x] T223 工厂接线与文档：`LocalProviderFactory::create` 包 decorator + ADR-050——**复核（2026-10-31，P1-2 批次）**：decorator 修复（重启失败静默结束缺陷）+ 测试 7 项全绿（workspace 本机证据；CI 验收按 ADR-037 待推送流水线）。

- [x] T224 [P] 折叠函数：`crates/whcode-app/src/context.rs` `fold_messages_for_budget`————**复核（2026-10-31，P1-1 批次）**：折叠函数与 turn 接线实现，workspace 58 套件 0 失败（本机证据；CI 验收按 ADR-037 待推送流水线）。
  保留首条 user + 预算内尾部窗口，中段折叠为摘要消息（含折叠计数与首行），构造上
  保证 ≤ 预算（FR-731；ADR-051 §2.2）
- [x] T225 turn 接线：预算压力分支改折叠一次 + compress_context 指针 + ContextState——**复核（2026-10-31，P1-1 批次）**：折叠函数与 turn 接线实现，workspace 58 套件 0 失败（本机证据；CI 验收按 ADR-037 待推送流水线）。
  (compressed=true) 事件落表，折叠后仍超才 budget_exceeded（FR-731；ADR-051 §2.3）
- [x] T226 [P] 测试：折叠保留锚点与尾部窗口、摘要内容可查、turn 预算压力折叠后——**复核（2026-10-31，P1-1 批次）**：折叠函数与 turn 接线实现，workspace 58 套件 0 失败（本机证据；CI 验收按 ADR-037 待推送流水线）。- [x] T226 [P] 测试：折叠保留锚点与尾部窗口、摘要内容可查、turn 预算压力折叠后——**复核（2026-10-31，P1-1 批次）**：折叠函数与 turn 接线实现，workspace 58 套件 0 失败（本机证据；CI 验收按 ADR-037 待推送流水线）。

- [ ] T227 子代理执行器：`crates/whcode-app/src/turn.rs` spawn_subagent 工具调用——**复核（2026-10-31，P1-5 批次）**：子代理编排实现 + 4 项测试全绿（workspace 58 套件 0 失败；本机证据；CI 验收按 ADR-037 待推送流水线）。**任务认知卡**：方向=子代理编排；目标=嵌套 run_turn+硬上限+回填；判据=Check 323—326；非目标=并行执行。
  分支——同步嵌套 run_turn（独立 request_id/turn_id、深度参数）、三重硬上限
  （FR-732；ADR-052 §2.2—2.3）
- [ ] T228 事件与结果回填：子 turn 事件落父会话、父 ToolCall 事件关联——**复核（2026-10-31，P1-5 批次）**：子代理编排实现 + 4 项测试全绿（workspace 58 套件 0 失败；本机证据；CI 验收按 ADR-037 待推送流水线）。**任务认知卡**：方向=子代理编排；目标=嵌套 run_turn+硬上限+回填；判据=Check 323—326；非目标=并行执行。
  tool_call_id、role:tool 结果回填（FR-733；ADR-052 §2.4）
- [ ] T229 [P] 测试：正常委派（子 turn 完成父继续）、数量上限拒绝、深度超限拒绝、——**复核（2026-10-31，P1-5 批次）**：子代理编排实现 + 4 项测试全绿（workspace 58 套件 0 失败；本机证据；CI 验收按 ADR-037 待推送流水线）。**任务认知卡**：方向=子代理编排；目标=嵌套 run_turn+硬上限+回填；判据=Check 323—326；非目标=并行执行。
  子失败结构化回填、父取消传播（FR-732—733；ADR-052）
- [ ] T230 文档同步：spec/tasks/checklist 勾选、ADR-052 §6 实施记录、CHANGELOG——**任务认知卡**：方向=子代理编排收口；目标=嵌套 run_turn + 三重硬上限 + 事件回填；判据=Check 323—326；非目标=并行执行/镜像前缀。——**复核（2026-10-31，P1-5 批次）**：子代理编排实现 + 4 项测试全绿（workspace 58 套件 0 失败；本机证据；CI 验收按 ADR-037 待推送流水线）。**任务认知卡**：方向=子代理编排；目标=嵌套 run_turn+硬上限+回填；判据=Check 323—326；非目标=并行执行。
- [x] T231 [P] - [ ] T231 [P] 工具清单随请求下发：LlmRequest.tools + openai 兼容族映射——**任务认知卡**：方向=真实供应商工具调用前置；目标=LlmRequest 携带 manifest 并映射进 openai tools 数组；判据=请求体含 tools 字段且与注册表一致；非目标=anthropic/gemini 映射延后——**复核（2026-10-31，P1-2 T231 批次）**：tools 字段/五协议映射/工厂接线/7 项测试全绿（workspace fmt/clippy -D warnings + llm 63 tests 0 失败；本机证据；CI 验收按 ADR-037 待推送流水线）。
  （anthropic/gemini 等按能力登记）；FR-734 为真实供应商工具调用的前置（ADR-048
  追加；〔未实现〕显式标记）——**认知卡**：方向=真实供应商工具调用前置；目标=LlmRequest
  携带 manifest 并映射进 openai tools 数组；判据=请求体含 tools 字段且与注册表一致；
  非目标=anthropic/gemini 映射（按能力登记延后）
  （FR-305 口径；CI 验收按 ADR-037）

  继续至完成、二次压力显式终止、指针与事件落表（FR-731；ADR-051）

  落档 + CHANGELOG（FR-730；ADR-050 §6）


---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: 无依赖，可立即开始
- **Foundational (Phase 2)**: 依赖 Setup 完成——**阻塞全部用户故事**
- **User Stories (Phase 3—12)**: 均依赖 Foundational；里程碑间存在实质前置：US2 依赖 US1 的 DuckDB 构建结论，US3/US4 依赖 US2 的存储与游标，US5 依赖 US3 的应用服务与 US4 的事件链路，US6—US8 依赖 US5 的阶段/门禁状态，US9 依赖 US3/US5 的检查点与门禁，US10 依赖全部
- **Polish (Phase 13)**: 依赖所需里程碑完成

### User Story Dependencies

- **US1 (P1, M0)**: Foundational 之后即可开始，无故事间依赖——**建议 MVP**
- **US2 (P1, M1)**: 依赖 US1 的 V-002 结论（原生构建可行）
- **US3 (P1, M2)**: 依赖 US2 的写者服务与游标
- **US4 (P1, M3)**: 依赖 US2 的事件持久化与 US3 的应用服务
- **US5 (P1, M4)**: 依赖 US3（命令面）与 US4（事件/追踪）
- **US6 (P2, M5)**: 依赖 US3/US5；可与 US7 并行
- **US7 (P2, M6)**: 依赖 US3/US5；Web 复用 US6 建立的能力矩阵
- **US8 (P2, M7)**: 复用 US3 应用服务与 US1 的 V-004 结论；平台承诺修订后，US8 依赖关系变为“FreeBSD/NetBSD 服务端 + 四平台客户端”
- **US9 (P3, M8)**: 依赖 US3/US4/US5 的检查点、事件与门禁
- **US10 (P3, M9)**: 依赖全部里程碑

### Within Each User Story

- 测试/证据任务与实现任务同 PR 交付（§15 清单）；先写失败路径用例再补实现
- 模型与类型先于服务；服务先于界面；核心实现先于集成
- 每个故事在其 Checkpoint 处独立验证，未通过不进入下一优先级

### Parallel Opportunities

- Setup 与 Foundational 中标 [P] 的任务可并行
- US1 中 T022/T023/T024/T025 可并行（不同 crate）；US2 中 T033/T035—T038/T040/T042 可并行
- US6 与 US7 在 US5 完成后可由不同开发者并行；US9 的四个运行时 provider（T110—T113）可并行
- 各故事的测试任务在实现前可并行编写

---

## Parallel Example: User Story 1

```bash
# M0 并行分支（不同 crate，互不冲突）：
Task: "实现数据库冒烟路径 in crates/whcode-store/src/smoke.rs"          # T022
Task: "记录各平台 DuckDB 原生构建方式 in docs/adr/ADR-DB-BUILD.md"      # T023
Task: "实现最小 TUI 闭环 in crates/whcode-tui/src/lib.rs"               # T024
Task: "实现 SSH 客户端原型 in crates/whcode-ssh/src/proto.rs"           # T025
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. 完成 Phase 1: Setup
2. 完成 Phase 2: Foundational（关键——阻塞全部故事）
3. 完成 Phase 3: US1（M0）
4. **停下并验证**：按 quickstart.md §2 执行 V-001—V-007，产出 `artifacts/m0-report.md`
5. 冻结平台矩阵与性能预算后再进入 M1

### Incremental Delivery

1. Setup + Foundational → 基础就绪
2. US1（M0）→ 独立验证 → 平台与预算冻结（MVP）
3. US2（M1）存储与事件 → 验证 → 可用基座
4. US3（M2）→ US4（M3）→ US5（M4）逐段交付并验证
5. US6/US7/US8 三端接入 → US9 运行时 → US10 优化与发布
6. 每个里程碑在其 Checkpoint 处停止验证，未通过不得推进（§12.1 退出标准）

### Parallel Team Strategy

1. 团队共同完成 Setup + Foundational
2. Foundational 完成后：按 crate 边界分工（存储/应用服务/LLM/前端各占一路）
3. US6 与 US7、US9 的四个运行时 provider 可分派给不同开发者
4. 五平台验收与 SSH 需按平台分派，且四 BSD 分别取证（不得以单一平台代替）

---

## Notes

- [P] tasks = 不同文件、无未完成依赖
- [Story] 标签映射任务到用户故事，便于追溯（US-n ↔ M-(n-1)）
- 每个故事必须可独立完成与验证；跨故事依赖已在 Dependencies 节显式声明
- 测试任务非可选：源基线要求每 PR 更新正向与失败路径测试，且 V-001—V-307 为验收判据
- 任务描述中的文件路径为规划值；实现前若调整 crate 名称，须保持职责边界与依赖方向（§6.2、ADR-003）
- 涉及 `[待验证]` 的任务（T021/T023/T025/T028/T102/T110—T113）必须以实测证据结项，不得以推断结论关闭

### 实现记录（2026-10-20，T001—T019 完成）

验证证据：`cargo fmt --all`（无差异）、`cargo clippy --workspace --all-targets -- -D warnings`（零告警）、
`cargo test --workspace`（50 个测试通过）、`cargo run -p whcode-bin -- doctor`（冒烟输出含
`api_key=[REDACTED]` 证明日志路径脱敏生效）。

实现期记录的三项偏差（均属任务描述内的合理调整，未改变职责边界）：

1. **T009**：workspace 共享依赖只引入 `serde`/`serde_json`/`thiserror`，**tokio 延后至 M2**（首个
   异步命令面出现时）——骨架阶段有界队列为同步实现，提前引入异步运行时属无用依赖。
2. **T014**：`term.rs` 为**接口 stub**——`terminal_size()` 固定返回 `None`，实际探测在 M0/T024 与
   crossterm 一并接线；禁止骨架阶段伪造尺寸（测试 `unprobed_terminal_reports_none` 固定该行为）。
3. **工具链（重要，属 D-01/V-001 待决项）**：本机 `rust-toolchain.toml` 的 `channel = "stable"` 被
   rustup 解析为 **msvc** 工具链（rustup 默认 host 为 `x86_64-pc-windows-msvc`），而本机未安装
   MSVC 构建工具，链接失败（`link.exe` 落到 Git 自带同名 Unix 工具）。处置：使用**目录级**
   `rustup override set stable-x86_64-pc-windows-gnu`（仅本机生效，未写入仓库文件，保持仓库可移植）。
   该差异必须由 M0 记录并在 D-01 中明确 Windows 验收采用 msvc 还是 gnu——**不得**以本机 gnu 结果
   冒充 msvc 平台验收（ADR-010）。

### 实现记录（2026-10-21，M0 阶段 T020—T026）

验证证据与边界（只记录已取得者，未取得者单列）：

1. **工具链（T002/T021，D-01 修订）**：`rust-toolchain.toml` 仅锁 `channel = "1.99.1"`，不写 host
   triple；本机用目录级 `rustup override` 选择 `x86_64-pc-windows-msvc`（机器本地状态，不入库）。
   实测 rustc 1.99.1（`58cf8f9c3`，2026-10-13）、MSVC 14.44.35207、Windows SDK 10.0.26100.0；
   **不使用任何 GNU 工具链**。
2. **DuckDB（T022/T023）**：改用官方预编译动态库（不源码构建）。`ci/fetch-duckdb.py` 按固定 URL
   校验归档 SHA256 `44cf5958…3fe96a`（13 408 787 字节）后解包到 `vendor/duckdb/1.5.6/`；
   `duckdb.dll` 载荷 SHA256 `7e90bdef…c84ed0`（36 697 896 字节）；`.cargo/config.toml` 注入
   `DUCKDB_LIB_DIR`，`crates/whcode-store/build.rs` 仅 Windows 复制 DLL；`whcode doctor --json`
   实测运行期引擎版本 `v1.5.6`。结论与替代方案见 `docs/adr/ADR-DB-BUILD.md`。
3. **T020/T021（依赖记录）**：`ci/platform-matrix/record-deps.sh`（BSD shell 侧）与
   `record_deps.py`（Windows Python 侧）均已实现，产出 `artifacts/deps-windows.txt`（含 host
   triple、rustc、MSVC、Windows SDK、预编译库与载荷哈希、运行期引擎版本、依赖树）。已取得的
   证据仅 Windows；FreeBSD/NetBSD 脚本就绪但本机无平台环境，OpenBSD/DragonFly BSD 已移出原生
   目标（ADR-011），因此**不得**把 T020/T021 记为“五平台完成”。
4. **交叉检查（边界）**：`cargo check --target x86_64-unknown-freebsd` 与
   `cargo check --target x86_64-unknown-netbsd` 已完成类型检查（产物位于
   `target/x86_64-unknown-{freebsd,netbsd}/`，2026-10-21 12:34），但该检查**不链接**，只证明
   代码级兼容，**不构成 DuckDB 原生库可构建或可运行的证据**（ADR-010、G-01）。
5. **存储测试（T022/T026）**：`crates/whcode-store/src/smoke.rs` 4 例（创建、事务写读、关闭重启、
   迁移）与 `crates/whcode-store/src/writer.rs` 10 例（幂等键、批次、确认、错误传播等）合计
   14 例通过；仅 Windows 平台结果。
6. **TUI（T024）**：`crates/whcode-tui/` 实现 `UiEvent`/`TuiState`/`TerminalSession`/
   `TerminalGuard`/`run`/`run_scripted`，主程序已装配；逻辑层 15 例测试通过。交互式终端的
   V-003 证据**未取得**，需人工冒烟（辅助脚本 `script/tui_non_tty_smoke.py` 只覆盖非交互分支）。
7. **SSH（T025，范围已修订）**：`crates/whcode-ssh/src/proto.rs` 实现内置 SSH **客户端**原型
   （公钥认证、受限命令、输出尾部与截断、意图—执行—登记），9 例测试通过；原四 BSD 服务端原型
   按 ADR-011 修订为客户端原型，**服务端验收属 M7，V-004 未取得证据**。

**未取得／阻塞的证据（不得勾选或声称完成）**：

- FreeBSD、NetBSD、OpenBSD、DragonFly BSD 均无运行证据（V-001/V-002 的 BSD 部分阻塞）；
  OpenBSD/DragonFly 的判据已改为客户端连通性，证据同样尚未取得。
- V-003 交互式终端证据未取得（T024 仅逻辑层测试）。
- V-004 服务端验收未取得（T025 仅客户端原型）。
- 性能预算未冻结（T027 基准夹具进行中，T028—T031 未完成）。
- 因此 T027—T031 保持未勾选。

### 实现记录（2026-10-23，门禁修复 + 文档同步；复核报告 `artifacts/progress-review-2026-10-23.md`）

**背景**：用户要求“版本号 = 二进制构建时间”后，`76bbf98`/`c3a76e5` 两次提交在门禁红灯状态下
进入 main——`c3a76e5` 的构建脚本含平台条件编译（NFR-101），版本戳用例为对编译目标恒真的断言，
导致 `clippy -D warnings` 与 `test_layering` 双红。进度复核时以实测复现（见报告 §2.1）。

**交付**：

1. `crates/whcode-bin/build.rs`：删除平台条件编译，改按**可用性**依次探测外部时间命令
   （`powershell` → `date`）；两者均不可用时回退 UTC 并输出 `cargo:warning`（不静默降级）。
   与 `whcode-store/build.rs` 的“以可用性分流、不以平台名分流”纪律一致。
2. `crates/whcode-bin/src/main.rs`：版本戳用例只校验格式（长度 17/22、分隔符、偏移形状）与
   年份合理性，去掉平台分支与恒真断言；回退情形由构建告警人工复核。
3. `crates/whcode-platform/src/process.rs`：新增 `sleep_command`（Windows `ping` / POSIX `sleep`），
   导出至 crate 根；`crates/whcode-runtime/src/worker.rs` 的超时用例改经它构造命令，
   消除平台分流在 `whcode-platform` 之外的残留（T104 纪律）。
4. `tests/contract/test_layering.rs`：marker 表补 `cfg!(windows`/`cfg!(unix` 宏形态——
   原表只覆盖属性写法，宏调用形态长期未被检出（本次在 `worker.rs` 中实测命中）。
5. 文档同步：README 当前状态（里程碑表、验收口径、门禁实测）与核心能力、ADR 清单更新至
   023；`docs/matrices/compatibility.md` 已知限制第 4/5 条改写（M7 只交付客户端；
   实现完成 ≠ 验收通过）；T121 落点注明风险台账并入 `docs/matrices/decisions.md`。

**验证证据**：`cargo fmt --all --check` 无差异；`cargo clippy --workspace --all-targets -- -D warnings`
零告警；`cargo test --locked --workspace` 退出码 0（47 个测试目标、403 例通过、4 例默认忽略）；
`layering` 6/6、`layout` 7/7；EOL 294 文件零违规；`whcode --version` 实测
`2026-10-23T01:55+08:00`（本地偏移生效）。

### 实现记录（2026-10-22，批次 4：Web 仅限局域网 + 认证边界；ADR-023）

**背景**：用户指令"Web 仅限局域网"。原 `serve --web` 只有回环无认证模式：
非回环地址被 bind 层拒绝（FR-606 后半不可达），且认证模式的 Origin 规则与真实浏览器
冲突（同源 GET 不带 Origin → 一律 403，实测复现）。

**交付**：

1. ADR-023：地址边界只允许**回环或 RFC1918 私网**；公网/通配（`0.0.0.0`）拒绝启动。
   局域网监听**强制** `--token-file`（认证 + Origin 检查）；token 只从文件读取
   （不经命令行；空文件拒绝）。
2. `bind::is_lan` + 门禁（含单测：10/8、172.16/12、192.168/16 允许；公网/通配拒绝）。
3. `api::handle_request` 新增 Origin 允许集参数（认证模式按绑定地址计算：
   `http://<bind>` + `localhost`/`127.0.0.1` 同端口）。
4. **Origin 语义修订**（ADR-023）：缺失 Origin **两种模式均放行**——同源 GET 与 CLI
   不带 Origin；认证模式跨站由 Bearer 阻断（自定义头触发预检，本服务不返回 CORS
   许可）。带 Origin 时仍必须命中允许集。原"认证模式要求 Origin"会使局域网浏览器
   一律 403（实测缺陷）。
5. 前端：token 经 URL **fragment**（`#token=…`）读取——fragment 不发送给服务器
   （§10.2 禁止 query token）；内存保存并附加 `Authorization: Bearer`。静态资产/API
   响应加 `cache-control: no-store`（修复旧前端缓存导致的复核失败）。
6. WS：认证模式下同样要求 Bearer（curl 实测 403 → 101）；浏览器无法为 WS 设置
   自定义头 → 认证模式列表页不使用 WS（前端轮询；已记录）。

**验证证据**：`artifacts/web-lan-2026-10-22-batch4.txt`——地址边界 3 例拒绝 + 回环正常；
认证/Origin 矩阵 8 行（403/200/101）；浏览器局域网页面（无 token 未连接；带 token
Connected/Cursor 6/事件 6 行/任务摘要，token 不入 URL/日志）。

**未取得（显式标注）**：公网/TLS/暴力破解防护不承诺（ADR-023 退出条件）；多用户
隔离与审计未实现（单令牌）。

### 实现记录（2026-10-22，批次 3：文本层接线——T130 端到端落地）

**背景**：`whcode-domain::i18n`（T130）已交付并有测试，但 TUI/Web/CUI 全部硬编码中文
（55 个键中仅 10 个且无消费方）——又一处"组件在、功能不在"。

**交付**：

1. 资源与接口：zh-CN/en 键表扩充到 55 个（TUI 标题/页脚/空态、视图摘要、CUI 人类行、
   Web 标签/状态），新增 `I18n::resolve_fmt`（`{name}` 占位符）与 `entries()`（Web 下发）；
   `Default = zh-CN`。
2. TUI：`RunOptions.texts: I18n`；标题/页脚/空提示经文本层；`ViewId::title_key()`；
   `StoreViewSource` 的视图摘要与数据源错误经 `resolve_fmt`（数据字段不翻译）。
3. CUI：调度的人类可读输出经文本层（`cui.*` 键）；数据字段、错误码与退出码不随语言变化。
4. Web：`GET /api/i18n`（`?locale=` → Accept-Language → zh-CN；返回 locale/rtl/entries）；
   前端 `data-i18n` 覆盖标签、`<html lang/dir>` 设置、状态/游标/任务摘要经词条；
   `?locale=` 显式覆盖（可见、可改、可撤销），失败时保持内置默认文本（可见降级）。

**验证证据**：`artifacts/i18n-2026-10-22-batch3.txt`——双语键表/占位符一致性测试；
TUI 真实控制台 zh 13/13、en 13/13；CUI 同一命令 zh/en 输出（数据字段不变）;
浏览器 `?locale=en|zh-CN|ar`（lang/dir=rtl、标签/状态/摘要、回退不伪造）。

**发现并修复**：主入口路由回归——`whcode --workspace DIR`（无子命令）被误送 CUI
clap（要求子命令）；改为“含位置参数 → CUI；仅旗标 → 交互入口”，并跳过带值旗标的值
（`--workspace DIR` 的路径不再是"位置参数"）。另修复首屏带 `?session=` 时任务列表
需等 15 个轮询周期才出现（启动即拉取）。

**未完成（显式标注）**：clap `--help` 文本与应用/存储层错误消息仍为中文（需
错误码→文本映射，属后续）；RTL 的 BiDi 重排属渲染层后续（接口已备）。

### 实现记录（2026-10-22，批次 2：CUI 命令调度装配——T084/T085 接线）

**背景**：CUI 的 clap 命令树、信封与三个视图模块早已交付，但 `whcode` 无任何 CUI
子命令可执行（Check 90 阻塞、T088 的"同命令"用例无法落地）——又一处"组件在、功能不在"。

**交付**：

1. `whcode <command>` 调度（`crates/whcode-bin/src/cui_dispatch.rs`）：解析用
   `whcode_cui::cli::parse_outcome`（区分帮助/版本与用法错误）；执行经 `whcode-app`
   （权限判定 FR-604）与 `whcode-store`（SQL 只在 store，ADR-018）；`--json` 统一信封、
   错误四字段落 stderr、稳定退出码（0/2/10…99）；`--workspace` 与 `--json` 为全局旗标。
2. 已接线：`session new/list/show/rename/archive/delete`（默认 dry-run，`--yes` 执行，
   `--dry-run --yes` 互斥）、`status`、`task list/show/retry/abort`、
   `events list/export`（json/jsonl）、`spec status`、`db stats/check/purge/compact/archive/
   explain`（explain 只读白名单 + 长度上限）。
3. 未接线命令显式失败（`run`、`events show`、`spec next/validate/analyze/converge/
   force-advance`、`runtime *`、`db checkpoint`）——错误信息说明原因，退出码 50，不静默成功。
4. 同命令一致性用例（V-109 追加）：`test_tui_cui_parity.rs::same_read_commands_yield_
   identical_state_across_cui_and_web`（CUI 事件视图/会话清单/任务总览 vs Web `/api/*`）。

**验证证据**：`artifacts/cui-2026-10-22-batch2.txt`（真实 24 条命令与退出码）；
`cargo test --workspace` **395 例通过 / 0 失败**（另 4 例 #[ignore]）；bin 26、cui 13、parity 4 绿。

**发现并修复**：`session delete` 初版套用 TTL 活跃窗口保护 → 删除刚创建的会话被静默
跳过；显式按 ID 删除改为不套活跃窗口（TTL 清理保留保护）。`--json` 初版只允许放在
子命令前（clap 非全局）→ 改为全局旗标。

**未完成**：Web 写 API 未交付 → "写命令跨端一致"（Check 90 完整）未取得；
`spec` 门禁命令需 SDD 产物工作区上下文。

### 实现记录（2026-10-22，批次 1：前端数据接入——TUI 真实数据源 + Web 任务/会话选择）

**交付**：

1. TUI `StoreViewSource`（装配根）：经 `whcode-app` 只读命令（`list_sessions` /
   `task_overview` / `events_export` / `load_context` / `build_snapshot`）产出六视图快照；
   查询只在准备层（视图切换、`j`/`k` 会话选择、`r` 显式刷新）；`ViewSource` 增加会话
   范围参数；`ViewSnapshot` 增加 summary 首行与选中标记（`> ` 前缀，不依赖颜色）；
   数据源错误写入摘要行（显式失败，不伪造）。工作区库打开失败（含跨进程锁）→ 显式
   警告并回退空数据源（TUI 仍可用）。
2. Web：`/api/tasks`（会话/工作区总览，FR-103）+ 前端任务列表（总计/完成/阻塞 +
   活跃任务）；会话条目可点击选择（`role=button`、`aria-current`、URL 同步、键盘
   Enter/Space），无 `?session=` 时自动选中首个会话。
3. 演示夹具扩展：3 个任务（running/failed/completed）+ 会话 `created_at` 排序确定。

**验证证据**：`cargo test`（tui 44 + 24、bin 19、web 18、layout 7、seed 1 绿）；
真实控制台 `script/tui_views_smoke.py --workspace artifacts/web-demo` 13/13
（会话标记 / 任务数据 / 选择移动 / 正确的空 / 事件尾部 / 刷新）；
浏览器实测与快速切换回归、发现项与修复见
`artifacts/web-browser-2026-10-22-batch1.txt`。

**本轮发现并修复**：

1. 会话切换时 `selectSession` 与轮询循环并发拉取 → 事件重复（实测 12 行）：在途守卫 +
   会话快照校验 + `seq <= cursor` 去重（快速 S2→S1 后保持 6 行）；
2. 事件视图游标起点负数（`6 - 200` 饱和为 -194）：`.max(0)`；
3. 无 `?session=` 时页面不选中任何会话：自动选中首个并同步 URL。

**未完成**：Diff 视图无数据源（converge 差异未接线，摘要行显式标注）；
Check 93 仍为 4/7 类操作；TUI 与 serve 同时打开同一工作区受 DuckDB 跨进程锁限制
（单写者服务统一所有权属后续任务）。

### 实现记录（2026-10-22，Web 服务装配 + 浏览器/桌面端到端测试；ADR-022）

**背景**：用户要求以电脑操作（computer-use）与浏览器控制（browser-use）对“需要点击/网页
调试”的功能做全量测试。测试前置暴露两处“库层通过、端到端从未运行”的缺口，先装配后测试：
`serve --web` 未装配（契约 contracts/cui-cli.md §1 已定义）；Origin 校验按全串精确比较——
真实浏览器（Origin 含端口、同源 GET 不带 Origin）一律 403。

**验证证据**：`cargo test --workspace` **383 例通过 / 0 失败**（另 4 例 `#[ignore]` 实验）；
fmt/clippy 零差异零告警；分层/布局/EOL/workflow/计数/前端预算门禁全绿（JS gzip 2477 B /
CSS 763 B）；真实浏览器与桌面取证：`artifacts/web-browser-2026-10-22.txt`（截图
`web-browser-2026-10-22/history-view.png`）、`artifacts/tui-computeruse-2026-10-22.txt`。

**已交付**：

1. `whcode serve --web [--bind HOST:PORT] [--workspace DIR] [--config PATH]`：
   静态前端**编译期嵌入**（固定白名单、无目录遍历面）+ `/api/*` 经 `handle_request`；
   顺序服务循环（DuckDB `Connection` 为 `Send` 非 `Sync`，单线程）；WS 握手 + 正常关闭
   （事件推送未实现，前端不依赖）；外部绑定按 FR-606 拒绝（实测退出码 1）；临时目录归
   **有效工作区**（`<workspace>/tmp`）。`whcode-bin` 新增 `whcode-web` 依赖（装配根职责）。
2. ADR-022：Origin 按 scheme+host 匹配（端口不参与）；回环模式缺失 Origin 放行
   （同源 GET/CLI），认证模式仍要求；威胁模型不变（非回环 host 一律 403）。
3. FR-617 修复：Web 挂起（blur/visibilitychange）期间**暂停轮询**——真实浏览器实测
   修复前 2.5 s 内 9 次请求 → 修复后 0 次；恢复去抖后单次刷新并续拉（游标不重复不遗漏）；
   定时器竞态守卫（非 Active 状态即使触发也不发请求）。
4. 静态路由剥离查询串（`GET /?session=…` 曾 404，回归断言补齐）。
5. 夹具与测试：`tests/integration/seed_web_demo.rs`（幂等演示工作区）；`whcode-web/src/assets.rs`
   （嵌入资产 + 路由白名单/首页结构断言）；serve 单测（参数解析/静态/API/404/403/WS）。
6. 浏览器实测：标签点击（aria-selected 单选 + section 显隐）、会话清单、历史 6 事件、
   状态 JSON、轮询 1 Hz（84 请求游标恒 6）、WS open→close:1000、canvas=0、无 JS 错误、
   断线（未连接）→ 重连（已连接、游标 6、日志 6 行无缺失）。
7. 桌面实测（computer-use）：六视图标题逐帧核对、按键计数（导航键不计）、最大化/还原
   （120×30↔209×51）、最小化/还原内容完整、Ctrl-C 退出、eink-day/amoled-night 双主题截图。

**发现并修复的缺陷（诚实记录）**：

1. Origin 全串比较误伤真实浏览器（ADR-022）；
2. `serve` 临时目录取 `--config` 工作区而非 `--workspace`——曾污染仓库根 `tmp/`，
   触发布局门禁失败；已改为工作区约定并设 DuckDB `temp_directory`；
3. 集成测试夹具目录“先清再建”缺失：进程被杀后遗留 + Windows 复用 PID →
   `S-DET` 主键冲突（本轮实测失败）；已为 6 个夹具补齐幂等清理。

**未取得／阻塞（显式标记，不得勾选）**：任务视图无数据源与 `/api/tasks` 路由
（Check 93 仅部分）；会话条目不可点击选择（仅 URL 参数 `?session=`）；CUI 命令执行
入口未装配（Check 90 阻塞）；内置浏览器无法触发真实 `visibilityState=hidden`
（后台标签仍报 visible）——Check 227/228 遗留缺口；`q` 退出在 computer-use 暗色实例
未生效（同工具 a/z/1—6 均生效；q 退出由两个控制台冒烟脚本覆盖 exit code 0）。

**资源（铁律）**：Web 服务进程（1 Hz 轮询客户端在线 26 min）RSS **25.6 MB**、线程 4、
句柄 135、CPU 8.95 s / 1579 s ≈ 0.6% 单核——预算 ≤512 MB 达标；TUI 进程 8.3 MB /
1 线程（上批）；帧预算 release p95 281 µs（`artifacts/tui-frame-budget.json`）。

### 实现记录（2026-10-22，T082/T083/T129/T135 接线 + T136/T128 收口）

**背景**：上批交付把六视图/渲染纪律/最小化门控做成了**组件**（`views.rs`/`render.rs`
含测试），但未接入真实运行循环——`whcode` 启动的 TUI 仍只画单段落，`theme` 模块
甚至未在 `lib.rs` 声明（文件在盘、测试从未运行）。本批完成最后一公里接线——
也是信号链完备原则（`docs/guides/signal-chain-completeness.md`）的首个前置案例：
交付从“组件存在”修正为“信号端到端存在”。

**验证证据**：`cargo test --workspace` 375 例通过 / 0 失败（另 4 例为 `#[ignore]`
实验，须显式运行）；fmt/clippy 零差异零告警；分层/布局/EOL/workflow/前端预算门禁
全绿；真实控制台取证 `artifacts/tui-views-2026-10-22.txt`（六视图帧逐帧落盘，自动
检查全通过）；帧预算 `artifacts/tui-frame-budget.json`（release 归档：p50 250 µs /
p95 281 µs，候选预算 p95 ≤ 16 ms 未冻结）。

**已交付**：

1. T082/T083/T129 接线：`run_with`/`LoopCore`（真实循环与重放 `run_scripted_steps`
   共用同一核心——重放证据与真实行为同源）；`1`—`6` 视图切换（导航键不计入普通
   按键）；`ViewSource` 准备层（`EmptyViewSource` 空数据源，帧内显式“暂无数据”，
   不伪造）；`Renderer::render` 返回待绘制行、最小化跳过计数、空闲零重绘；帧路径
   限行投影 `render_lines_limited(rows)`——**帧成本由终端尺寸界定**（10k 行快照与
   200 行同阶；完整历史不进帧路径，Check 83 实测暴露并修复）。
2. T135 接线：最小化探测由 `whcode-bin` 装配根注入（`is_window_minimized`，
   ADR-021 修订）；事件循环只在**有待绘制帧 + 输入静默 ≥1s** 时探测，间隔 5s→60s
   指数退避、检测到最小化回落 5s；空闲（无待绘制帧）与连续输入期间零探测；
   输入事件按“还原”记账（输入 ⇒ 可见）。重放断言：探测计数/最小化跳过/还原
   单次全量重绘/空闲零探测/连续输入零探测。
3. T136/T128 收口：`ui.theme`（kebab 稳定名）+ `WHCODE_UI_THEME` 覆盖（非法值
   警告回落、可撤销）；主题进入渲染单元（测试断言背景精确 `#000000`/`#FFFFFF`）；
   前端审计扩展 `suspend.js` 与 NFR-212 断言（运行时路径禁 `getContext`/GPU 探测
   调用、禁 `requestAnimationFrame`）。
4. 真实控制台资源采样（铁律）：TUI 进程峰值 RSS **8.3 MB**、线程 **1**、句柄 120、
   CPU 累计 0.03 s（约 2 s 交互会话；DuckDB 未载入——TUI 单线程不触库，
   ADR-003 的运行时侧证据）。

**发现并修复的缺陷（诚实记录）**：

1. `theme` 模块未声明——模块文件存在而测试从未运行（已声明 + 接线 + 测试入册）；
2. `run_stability.sh` 工作负载异常退出不清理——**实测遗留负载循环 4 小时**
   （11:21—15:30 反复执行 extreme_scale，干扰构建与测试，曾致 extreme_scale 非
   确定性失败）；修复：`EXIT`/`INT`/`TERM` trap + `taskkill /T` 进程树清理
   （Windows 无进程组语义，单杀子 shell 不终止 cargo 下游），已用 SIGTERM
   实测验证（0 残留）；
3. `app.js` 未使用导入 `probeGpu`——改为注释说明不接线原因（运行时调用即创建
   GPU 上下文，违反 NFR-212）。

**未完成／阻塞（显式标记，不得勾选）**：T099（四 BSD 验收证据，ADR-013 延后；T096—T098 已交付，ADR-030）；
T115（预算未冻结）；T116（24/72h 长跑）；**视图业务数据源装配**（`ViewSource`
注入真实会话/任务/事件——Check 179 仅部分证据）；**Web 浏览器实测**（Web 服务
入口未装配，Check 220/227/228 的 Web 侧未取得）。

### 实现记录（2026-10-22，资源铁律落地：标准具体化 + 极限测试 + 实测复核）

**铁律标准具体化**（`docs/guides/resource-budget.md`）：内存（测试进程峰值 RSS
≤ 512MB、组件分配有界、切换增量 ≤ 10MB、驻留仅元数据+游标）与 CPU（禁忙等、
空闲零重绘、切换 ≤ 200ms、概览 p95 ≤ 50ms、单页 ≤ 30ms）两类标准，静态红线
五条（无界累积/忙等/帧路径 JSON/每会话线程/leak）。

**极限测试（用户修订：初版 100k 夹具过于保守，会话库 30—60GB）**：
`test_extreme_scale.rs`（1M 事件单会话，≈150MB 库文件，60GB 同构访问模式）——
X1 夹具 1M 事件写入 116s（吞吐 ≈ 8.6k 行/s）；X3 大表聚合 count/MAX **1.4ms**
（索引/元数据）；X2 全序翻页 1000 页 **O(1) 内存**（无全表物化，累计计数/求和/
相邻递增比较）；X5 极限切换 **17—27ms**、尾部恒定 50 条（NFR-207 候选 200ms
达标）；X4 极限清理 1M 行删除 **实测 125s**（DuckDB MVCC 删除 + 事务提交成本；
按 NFR-216 测量纪律以实测修订标准 ≤ 300s，删除吞吐候选 ≥ 8k 行/s——批量/
分区删除为后续优化候选）；上下文截断 O(1)。

**实测复核（script/measure_resources.py，PowerShell 100ms 轮询 WorkingSet）**：
extreme_scale 峰值 RSS **174.3MB**、session_switch **85.6MB**——均 ≤ 512MB
预算（铁律 M1 达标）；报告 `artifacts/resource-report.json`。

**复核结论**：静态红线逐项过（无界累积/忙等/帧路径 JSON/每会话线程/leak——
无violation）；实测两个重负载测试进程内存达标；**X4 清理吞吐为后续优化候选**
（NFR-216：批量/分区删除，候选值经实测修订）。workspace 352 例全绿。

### 实现记录（2026-10-22，TUI 视图与渲染纪律完成，T082/T083/T129/T136/T137；T135 阻塞）

验证证据：`cargo test --workspace` 351 例通过 / 0 失败（tui 21 例含渲染纪律
四例）；fmt/clippy 零差异零告警；分层/布局/EOL/workflow/前端预算全绿。

已交付：T082（`tui/views.rs` 六视图只读快照：会话/任务/上下文/事件/Spec/diff，
render_lines 从准备层数据渲染）；T083（`tui/render.rs` 只读快照 + 脏标记重绘 +
**不在帧路径反序列化 JSON**——结构测试剥离注释/测试模块后断言无 serde_json）；
T129（空闲期零重绘——redraw_count 计数器，100 次空闲心跳零重绘断言；文档写明
终端绘制由终端模拟器承担，WHCode 不直接调度 GPU）；T136（`frontend/suspend.js`
挂起状态机 Active/Suspended/Recovering：visibilitychange + blur/focus 判定、
恢复去抖 ≥250ms、恢复时取快照并丢弃中间帧——仅保留最后一份待处理快照；
WebGL/Canvas 上下文：WHCode 前端不使用（ADR-014/FR-618），probeGpu 探测失败
自动降级纯文本）；T137（check-frontend-budget.py 已随 M6 交付，JS gzip 1.9KB
/CSS 0.7KB）。

**T135 阻塞（显式标记，不得勾选）**：渲染门控消费端已交付（render.rs minimized
门控 + 还原单次全量重绘测试），仅缺探测源——Win32 FFI 须 unsafe，NFR-215
（--forbid 不可覆盖，T143 锁定）排除；ADR-021 已记录否决与恢复路径。

**T116 采集链路交付**：`tests/stability/run_stability.sh`（RSS/线程/句柄采样
时间序列 JSON 输出 + STABILITY_SMOKE 冒烟模式验证采集链路——本机冒烟通过，
null 进程为空闲期正确行为）；**24/72 小时连续执行仍阻塞**（需长跑环境），
冒烟结果不作稳定性结论。

### 实现记录（2026-10-22，M6/M8 完成，T090—T095 + T100—T114 + T133/T137）

验证证据：`cargo test --workspace` 345 例通过 / 0 失败（web 14、platform 11、
runtime 14、speckit 22、契约 web_security_defaults 5 + runtime_acceptance 6）；
fmt/clippy 零差异零告警；分层/布局/EOL/workflow/前端预算门禁全绿。

**M6（T090—T095 + T133/T137）**：api.rs HTTP API + WebSocket（RFC 6455 握手
SHA1/base64 最小实现——握手魔术串非密码学用途；分片/二进制帧拒绝不静默降级）；
bind.rs 默认仅回环、对外监听拒绝启动（V-110/FR-606）；security.rs Bearer 认证
（恒定时间比较）+ Origin 允许集 + 消息上限 + token 不经 URL query（§10.2）；
frontend 静态 HTML/CSS/原生 ESM（ADR-014 无构建链），断线重连指数退避 + 按游标
补齐（T094/FR-605），prefers-reduced-motion 尊重（T133）；check-frontend-budget.py
（T137：JS gzip 1.5KB / CSS 0.7KB 预算 50KB/20KB）。

**M8（T100—T114）**：RuntimeProvider 六要素接口、能力三级、payload 三重校验
（FNV-1a 意外损坏检测非密码学）、受控子进程（超时轮询/输出上限/stderr 尾部）、
检查点四要素、缓存仅显式启用、能力不足结构化错误 + 未启用运行时检出（V-306）、
converge RuntimeBinding 四态映射（FR-709）、Windows taskkill /T /F 进程树清理
（T109 std 路径；BSD 属 M7 延后不静默）、四 Provider 诚实能力门控（js 无引擎
不可用 D-08、python 系统解释器 probe、gitbash V-304 探测、bsdshell M7 延后）、
V-301/302/303/305/306/307 验收测试（T114）。

**偏差（已留档）**：1. HTTP+WebSocket 手写于 std（零新依赖，契合 ADR-014 极简）；
2. 分层门禁抓到 runtime 内平台 cfg——platform 差异移入 whcode-platform
（`shell_command`，NFR-101 门禁生效的证据）；3. worker 超时经轮询 try_wait
（std 无 wait_timeout）；4. `store SessionRow` 增补 Serialize（Web JSON 需要）。

### 实现记录（2026-10-22，Phase 13 清点，T122—T127 + T138—T148）

验证证据：`ci/check_counts.py` 通过（FR=79 / tasks=148 / checks=249，counts
consistent——T127 三方追溯审计）；`cargo run -p whcode-bin -- doctor` 运行成功
（api_key=[REDACTED] 脱敏生效，T124 端到端运行记录）；layout 7 例 / EOL /
workflow 审计全绿；workspace 测试全绿（T126 清理重构：clippy 零告警维持）。

T138—T148 的交付物均已在早前提交落盘（software-engineering-process、
philosophy、yike-philosophy、design-principles、requirements-engineering、
ui-design、performance-and-concurrency、doctrine-application 八份指南；
ci/check_counts.py 计数门禁；unsafe_code="forbid" 契约测试 T143）。

**T122 保留项（诚实标注）**：plan.md 剩余 5 处 `[待验证]` 为**诚实的开放决策**
（Web HTTP/SSH 依赖选型未定——axum/russh 按 M6/M7 平台证据定案），不是遗留
过期标记，不消除；ADR-018/019/020 与 decisions.md 已同步 D-01—D-14 最终状态。

**T124 范围说明**：quickstart §2—§9 全流程中 M6/M7/M8 命令面未交付——本记录
覆盖已交付里程碑的 doctor/测试验证；M6/M8 交付后须重跑全流程。

### 实现记录（2026-10-22，M9 文档类完成，T117—T121；T115/T116 阻塞）

验证证据：文档全绿（layout 7 例、EOL/workflow 审计通过）。

已交付：T117（ADR-020 分配器：默认系统分配器，替换须基准证明，D-11）、
T118（`docs/matrices/compatibility.md` 兼容矩阵与已知限制，BSD 延后注明）、
T119（`docs/guides/cui-manual.md` CUI 手册）、T120（`runtime-inventory.md`
运行时能力清单与许可证清单）、T121（decisions.md 风险台账更新 + 新增
R-13 自动提交扰动）。

**阻塞（显式标记，不得勾选）**：T115 性能优化——D-06 预算未冻结（缺跨平台
基线与 72h 时间序列）；T116 稳定性测试——需 24/72h 连续运行环境。

### 实现记录（2026-10-22，M5 批次三，T084/T088 + ADR-019）

验证证据：workspace 全绿（cui cli 4 例解析测试、契约 tui_cui_parity 3 例）；
fmt/clippy 零差异零告警。ADR-019（clap 采用，plan.md 既定选型，derive API、
default-features 关闭）；T084 命令树按 contracts/cui-cli.md §1（force-advance
强制 --reason、破坏性命令 dry-run/yes 旗标）；T088 一致性契约（路由同源/权限
语义一致/错误码前端无关）——T082 交付后须追加"同命令最终状态一致"用例。

### 实现记录（2026-10-22，M5 部分完成，T085/T086/T087/T089/T130/T131）

验证证据：`cargo test --workspace` 全绿（domain i18n 2、app residency 2、cui 9、
契约 json_envelope 4）；fmt/clippy 零差异零告警；分层/布局/EOL/workflow 全绿。

已交付：T085（CUI 统一 `--json` 信封 + 稳定退出码）、T086（破坏性命令显式确认，
默认 dry-run）、T087（feature-parity 矩阵首版）、T089（JSON 信封契约测试）、
T130（i18n 双语 + 区域检测 + 回退链 + RTL 预留）、T131（会话驻留策略）。

**未完成（显式标记，不得勾选）**：T082/T083/T129/T135（TUI 六视图/渲染纪律/空闲
零重绘/最小化检测——TUI 渲染层）、T084（clap 命令树——需新增 clap 依赖，须先有
ADR）、T088（TUI/CUI 契约一致性——依赖 T084）、T133/T136/T137（Web 前端动效/
挂起/极简基线）；M6（T090—T095 Web UI）、M8（T100—T114 运行时）、M9（T115—T121
优化发布）、Phase 13 收尾——按未完成推进。

### 实现记录（2026-10-22，M4 完成，T068—T081）

验证证据：`cargo test --workspace` 283 例通过 / 0 失败（speckit 21、app 44、
集成 sdd_gates 7 例 V-201—V-207）；fmt/clippy 零差异零告警；分层/布局/EOL/workflow
门禁全绿。

实现期记录的偏差（均为任务描述内的合理调整，已留档）：

1. **store 迁移 v5**：`sessions.stage` 列持久化 SDD 阶段（FR-314 四端同源的存储
   依据）。DuckDB 1.5.6 不支持带约束的 ADD COLUMN——迁移用可空列 + 回填，
   NULL 由 directory 层兜底为 `specify`；迁移测试补 v4→v5 路径。
2. **Constitution 门禁输入显式化**：强制条款的核对结论由调用方提供（FR-313 禁止
   从历史推断），V-201 测试以显式 verdict 驱动；审计失败时变更不生效（AuditSink 契约）。
3. **状态机经由 Clarify**：workflow 状态机中 Specify → Clarify → Plan（Analyze 为
   独立阶段）——门禁测试沿合法链路推进到目标阶段再验证拒绝（V-202/V-203）。
4. **force_advance 参数打包为 `ForceAdvance`**：clippy too_many_arguments（10/7）；
   `request_transition` 移除 now_ms（update_stage 不需要）。
5. **CUI 夹具用 include_str 迁移 SQL**（含 v5）——分层门禁禁止 CUI 触库（C-03）。
6. **解析器空产物视为失败**：`is_empty_artifact` 并入 parse_with_retry 的接受条件
   （无条目即无信息，FR-309）；未知段落的行进 `unparsed_sections`（原文保留）。

### 实现记录（2026-10-22，M3 完成，T056—T067 + T132）

验证证据：`cargo test --workspace` 全绿（llm 18、app 42、cui 3、集成
trace_and_context 1）；fmt/clippy 零差异零告警；分层/布局/EOL/workflow 门禁全绿。

实现期记录的偏差（均为任务描述内的合理调整，已留档）：

1. **Provider 为同步迭代器流式**：tokio 延后引入（T009 实现记录既定），桩 Provider
   的取消在下一个 chunk 之前生效（§8.4 要求的既定说明），真实 HTTP Provider 属后续。
2. **事件导出查询核心在 `whcode-app::events`**（T065 的 CUI 部分为展示包装）：分层
   门禁禁止 CUI 触库（C-03），查询必须经应用服务；CUI 经 include_str 迁移 SQL 做
   夹具（不引入 store 依赖）。
3. **app 新增 `whcode-llm` 依赖**：事件导出经 llm 追踪层脱敏（FR-205）；llm 为同层
   领域服务，方向 app→llm 与分层图一致。
4. **压缩摘要只记指针**：摘要正文由 LLM 调用生成（后续接入），本模块只记录
   `context_windows` 指针行，FR-107 的"原文仍可查"由测试断言。
5. **store `EventRow` 增补 Serialize**：事件导出 JSON 化需要（serde 为 store 既有依赖）。
6. **V-103 断线补拉**：补拉从排他游标之后开始，断线期间的关键事件集合在补拉中
   齐备（durability 测试已覆盖 V-104，不重复）。

### 实现记录（2026-10-22，M2 完成，T046—T055）

验证证据：`cargo test --workspace` 220 例通过 / 0 失败（app 28、store 90、迁移 8、
切换与概览性能集成测试各 1/2）；fmt/clippy 零差异零告警；分层/布局/EOL/workflow
门禁全绿；概览性能证据 `artifacts/overview-perf-m2.json`（p95 7.6 ms，候选预算
50 ms，**预算未冻结**）。

实现期记录的偏差（均为任务描述内的合理调整，已留档）：

1. **ADR-018（架构口径统一）**：AGENTS/module-design 的"app 不依赖存储实现"与
   可执行门禁（只禁前端触库）不一致。裁决：app 直接依赖 store 公共 API、SQL 只在
   store（端口-适配器被 ADR-001 唯一后端约束否决）；分层图同步修订。
2. **store 新增 `directory`/`tasks` 存储原语与 schema v4（会话 ID 序列）**：T046—T052
   的必要使能；`test_migration` 按 v4 引入补齐 v0/v1/v2/v3→v4 路径（其文档既定要求）。
3. **purge 判据扩展 `Sessions`**：会话删除命令复用 FR-509 全链（预览/确认/审计/
   活跃保护），避免第二套删除路径。
4. **命令事件发射延后至 M3**：M2 验收（Check 46—55）不要求命令写事件；事件流属
   M3（T056+），已在本层文档显式标注，`event_cursor` 以工作区 `events.seq` 最大值为准。
5. **内存增量预算（Check 47）**：切换的有界性以结构断言（尾部长度恒 50）验证；
   NFR-201—204 预算未冻结，RSS 测量归 T142/M9。
6. **任务创建命令不在 M2 命令面**（FR-101 无 create task）：任务行由夹具/后续
   里程碑产生；`store::tasks::insert_task` 为存储原语而非用户命令。

### 实现记录（2026-10-21，M0 阻塞项推进）

以下仅记录本机已取得的实测结果；未取得的证据单列，不作为完成依据。**预算未冻结**。

1. **吞吐优化（B-07 主项，T026）**：写者 `flush_entries` 由“逐条 INSERT + 逐条回读 `seq`”
   （每批 2N 次往返）改为“1 次分块 IN 查询已存在键（256 键/查询）+ 分块多行 VALUES 批量插入
   （64 行/语句）+ 插入前分配连续 `seq`、不再回读”。同一夹具、同一机器复测：优化前
   158.1 / 142.4 / 159.6 条/秒（中位数 158.1），延迟 p50 7 034.9 / p95 12 305.2 /
   p99 20 172.6 µs（600 样本）；优化后 3 359.1 / 2 965.0 / 3 997.2 条/秒（中位数 3 359.1，
   约 21 倍），延迟 p50 6 975.4 / p95 14 117.9 / p99 20 727.4 µs；每次 20 200 行落库，
   库文件 1 847 296 字节。证据：`artifacts/baseline-after-opt.json`（本地，不入库）；
   优化前结果保留在 `artifacts/baseline-windows-amd64.json`。
2. **结论口径（T027/T028）**：优化后吞吐已超过 NFR-210 候选值 ≥ 2 000 条/秒，但**预算仍不冻结**：
   缺跨平台基线数据与 72 小时时间序列，D-06 的冻结条件未满足。
   - 追加（同日，夹具内存更正后复测）：`memory_limit` 回到 §5.4 基线 **192MB**，吞吐
     7 427.5 / 6 921.2 / 6 242.1 条/秒（中位数 **6 921.2**），延迟 p50 3 942.7 / p95 5 586.7 /
     p99 10 994.8 µs；证据 `artifacts/baseline-192mb.json`。两组差异属单机三样本观测，
     **不得**推断“调低 `memory_limit` 会提速”（ADR-012）。
3. **TUI 管道挂起缺陷（B-05 关联，T024）**：缺陷为 `terminal_size()` 查询**输出**侧，Windows 上
   “管道 stdin + 控制台 stdout”仍返回 `Some`，导致无参数启动进入 TUI 并永久等待按键。修复：
   `whcode-platform` 新增 `stdin_is_interactive()`（`std::io::stdin().is_terminal()`），
   `whcode-bin::launch_interactive()` 改为双重判定。
4. **自动冒烟（V-003 可自动化部分）**：`script/tui_smoke.py` 三项自动检查全部通过——管道 stdin
   下优雅退出（退出码 0）、`--version`、`doctor` 非交互；产物
   `artifacts/tui-smoke/<时间戳>/summary.json`。
5. **仍未取得的人工证据（V-003 不能据此判通过）**：物理键盘输入计数、真实 resize 重绘、
   Ctrl-C 退出与终端回显恢复、强杀进程后 raw mode 残留——四项必须人工执行并留证。
6. **测试规模（2026-10-21 实测）**：`cargo test --workspace` 107 例通过 / 0 失败；
   `cargo fmt --all --check` 无差异；`cargo clippy --workspace --all-targets -- -D warnings`
   零告警；EOL 审计 0 CRLF / 0 BOM（Windows 本机）。
7. **`memory_limit` OOM 断言已独立复核并证伪（ADR-012）**：192MB + 4 线程下用同一
   `EventWriter` 写入 2 000 条与 20 000 条（131 字节 payload）**全部成功、无 DuckDB 错误**；
   回读值 `183.1 MiB` 只是 192MB 的 MiB 表示，不是“用量”。据此夹具已从 384MB 改回基线 192MB，
   `DEFAULT_MEMORY_LIMIT_MB` 保持 192。ADR-012 状态为**部分批准**（断言证伪证据充分；
   “192MB 可冻结为基线”仍需大载荷、长时与多平台数据）。

**未完成项**：跨平台吞吐基线、72 小时时间序列、V-003 人工四项、V-004 服务端验收仍未取得；
因此 B-05 与 B-07 均不得记为已关闭。
