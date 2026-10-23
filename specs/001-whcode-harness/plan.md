# Implementation Plan: WHCode — Rust 原生 LLM 编码 Harness

**Branch**: `001-whcode-harness` | **Date**: 2026-10-20 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/001-whcode-harness/spec.md`

**Note**: This template is filled in by the `/speckit-plan` command; its definition describes the execution workflow.

## Summary

为 WHCode——Rust 原生 LLM 编码 Harness——建立可追踪、可验证、可恢复、指标可复现的全量需求基线与实施设计。基线源文本为《WHCode 技术任务书 v3.0》（`source-whcode-taskbook-v3.md`）。本计划覆盖任务书 M0—M9 全部里程碑：M0 平台可行性与基线冻结（建议 MVP）、M1 存储与事件基座、M2 会话/任务应用服务、M3 LLM 与上下文、M4 SDD 门禁状态机、M5 TUI/CUI、M6 Web UI、M7 BSD SSH、M8 可选执行运行时、M9 优化与发布。

技术路线：Cargo 多 crate workspace（`whcode-domain`/`app`/`llm`/`store`/`speckit`/`tools`/`runtime`/`tui`/`cui`/`web`/`ssh`/`platform`/`bin`），DuckDB 作为唯一默认持久化后端（每工作区一个数据库文件、应用层单写者服务串行化写入），TUI/CUI/Web/SSH 四端共享同一应用服务层，外部工具执行采用"意图记录—执行—结果登记"协议，运行时（JavaScript/Python/Shell）一律在受控子进程中执行。

## Technical Context

**Language/Version**: Rust（工具链版本由 M0 锁定；具体稳定版号在 M0 基线报告中冻结，当前候选为最新 stable）

**Primary Dependencies**: tokio（异步运行时）、duckdb（Rust crate，原生依赖）、ratatui + crossterm（TUI）、clap（CUI）、serde/serde_json（产物与协议）、axum 或等价（Web API/WebSocket）、russh 或等价（SSH 服务端）——后两者在 M0/M6/M7 按平台验证结论确认，属 `[待验证]`（见 research.md R-05、R-06）

**Storage**: DuckDB（默认且唯一，ADR-001；未经 ADR 批准不得增加第二默认后端）

**Testing**: cargo test（单元/集成/属性测试）、Nextest（并行选择）、Criterion 或等价基准夹具（NFR 测量）、CUI JSON 契约测试、平台 CI 矩阵（FreeBSD/OpenBSD/NetBSD/DragonFly/Windows）

**Target Platform**: Windows 11 amd64（`x86_64-pc-windows-msvc`，ADR-013 唯一验收目标）；FreeBSD/NetBSD/OpenBSD/DragonFly 延后（代码与文档保留，恢复须新 ADR 并补齐证据链）

**Project Type**: 多 crate workspace——本地 TUI + 自动化 CUI + Web 服务 + SSH 入口的单仓库项目（非库/非单一 CLI）

**Performance Goals**: 以任务书 §5.2 的 NFR-201—210 候选预算为起点，M0 基线报告冻结后方可作验收承诺；事件写入吞吐候选 ≥ 2,000 条/秒、会话切换 ≤ 200 ms、概览查询 p95 ≤ 50 ms、单页查询 p95 ≤ 30 ms

**Constraints**: 单写者事务一致性与检查点同事务提交（FR-403/ADR-007）；有界队列与满载语义（§5.3、§7.3）；历史注入有界（FR-105/FR-505）；Web 默认仅监听 127.0.0.1（FR-606）；秘密默认脱敏（FR-203）；运行时子进程隔离（ADR-008）；不承诺跨组件 exactly-once（§0.3-8）；桌面 GUI 若实现必须原生、禁止浏览器套壳（ADR-026）

**Scale/Scope**: 13 个 crate、14 张核心表、79 条 FR 定义、NFR-101—107 与 NFR-201—222、里程碑 M0—M9、31 项验收 V-001—V-307、14 项待决策 D-01—D-14（另有 2026-10-23 新增的 D-15 GUI 形态铁律，见 `docs/matrices/decisions.md`）、12 项登记风险 R-01—R-12（来源追溯见 spec.md）

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

`.specify/memory/constitution.md` 当前为**未批准的模板占位**（全部为 `[PRINCIPLE_*]` 占位符）。按 [checklists/requirements.md](./checklists/requirements.md) 校验说明第 6 条，本轮门禁以任务书冻结基线替代：

| # | 门禁 | 判据来源 | Phase 0 前结论 |
|---|---|---|---|
| C-01 | 默认持久化后端仅 DuckDB；增补第二默认后端必须先有批准 ADR | 任务书封面、ADR-001、§9.1 | PASS——计划未引入任何其他默认存储 |
| C-02 | 架构基线 ADR-001—010 不可绕过；变更必须走 ADR | §2 | PASS——结构与接口均映射至既有 ADR（见下表） |
| C-03 | 前端不得直接触库；一律经应用服务 | ADR-003/005 | PASS——`whcode-tui/cui/web/ssh` 仅依赖 `whcode-app` 接口 |
| C-04 | 外部副作用不得承诺 exactly-once；必须走意图—执行—登记 | §0.3-8、ADR-007 | PASS——`tool_executions` 与恢复协议按此建模 |
| C-05 | 不受信任脚本不得进入主进程 | ADR-008、§10.2 | PASS——运行时全部为子进程 worker |
| C-06 | 未验证能力不得表述为已交付；性能数字须有测量条件 | §0.1、NFR-2xx、§5.2 测量纪律 | PASS——候选指标一律标注 `[待验证]` 并归属 M0 |
| C-07 | 规范用语（MUST/SHOULD/MAY/TBD）与需求属性标记保持 | §0.1/§0.2 | PASS——spec.md 逐条保留 P0/P1 与 `[硬性]/[架构]/[优化]/[待验证]` |
| C-08 | SDD 门禁由程序实施，提示词不构成门禁 | G-05、FR-308 | PASS——阶段校验归属 `whcode-speckit` + `whcode-app` |

**ADR 映射（C-02 支撑）**：ADR-001→`whcode-store` 单文件工作区；ADR-002→写者服务；ADR-003/004/005/009→`whcode-app` 共享命令面 + 四端适配器；ADR-006→`Event Router` 双队列分离；ADR-007→`whcode-tools` 意图协议；ADR-008/010→`whcode-runtime` 子进程与平台分开验收。

**Phase 1 后复评**：结构、数据模型与契约（data-model.md、contracts/）逐项核对 C-01—C-08，无新增违例。Web/SSH 依赖选型（C-06 相关）保持 `[待验证]`，未表述为已冻结；DuckDB 在 BSD 的构建风险（C-01/C-06 交叉项）列为 M0 Go/No-Go 条件，而非静默降级。

## Project Structure

### Documentation (this feature)

```text
specs/001-whcode-harness/
├── spec.md              # 需求基线（/speckit-specify 产出）
├── plan.md              # 本文件（/speckit-plan 产出）
├── research.md          # Phase 0 产出：决策与备选
├── data-model.md        # Phase 1 产出：实体、状态迁移、事务与游标规则
├── contracts/           # Phase 1 产出：跨端与对外契约
│   ├── app-service.md   #   应用服务命令/事件/快照契约
│   ├── cui-cli.md       #   CUI 命令树、退出码、JSON schema
│   ├── event-schema.md  #   事件类型与持久化等级
│   ├── sdd-artifacts.md #   SDD 产物规范化 schema 与门禁规则
│   └── runtime-protocol.md # 子进程运行时协议
├── quickstart.md        # Phase 1 产出：验证运行指南
├── checklists/
│   └── requirements.md  # 规格质量校验清单（自动产物）
├── checklist.md         # 验收检查点（本管道第四步）
├── tasks.md             # /speckit-tasks 产出
└── source-whcode-taskbook-v3.md  # 需求源文本（只读基线）
```

### Source Code (repository root)

```text
Cargo.toml                    # workspace 根
crates/
├── whcode-domain/            # 领域类型、状态枚举、事件类型、门禁结果（无 UI/网络/DB 依赖）
├── whcode-app/               # 命令处理、权限检查、用例编排、状态快照、阶段门禁调用
├── whcode-llm/               # Provider 抽象、流式响应、重试、计量、追踪级别
├── whcode-store/             # DuckDB schema、迁移、写者服务、读取连接、事务、清理、导出
├── whcode-speckit/           # 工作流配置、Markdown 解析、产物校验、追溯、Converge 门禁
├── whcode-tools/             # 工具注册、权限策略、执行意图与结果登记
├── whcode-runtime/           # 运行时 provider、payload 校验、worker 子进程协议
├── whcode-tui/               # ratatui/crossterm 界面与会话树、事件流、diff 预览
├── whcode-cui/               # clap 命令、JSON/JSONL 输出、退出码
├── whcode-web/               # HTTP API、WebSocket、认证与 Origin 校验、静态前端
├── whcode-ssh/               # SSH 认证、会话隔离、终端尺寸、受限入口
├── whcode-platform/          # OS 差异：进程/信号/PTY/路径/资源探测（cfg 集中于此）
└── whcode-bin/               # 主程序、配置加载、日志初始化、feature 装配
tests/
├── unit/                     # 与 crate 内联测试之外的跨模块单元测试
├── contract/                 # CUI JSON、事件 schema、SDD 产物、三端一致性契约测试
├── integration/              # 存储—事件—检查点—恢复—门禁端到端场景
├── crash/                    # 崩溃注入：步骤边界、执行前/后、登记前（V-105、V-307）
├── stability/                # 24/72 小时稳定性脚本与指标采集
└── fixtures/                 # 数据集生成器（10 万事件、1 万任务）、损坏产物样例
ci/
├── platform-matrix/          # 五平台构建与原生冒烟（V-001—V-004）
└── benchmarks/               # 基线性能报告脚本（V-006）
docs/
├── adr/                      # ADR-001—010 及后续决策记录
├── matrices/                 # 平台兼容矩阵、三端功能能力矩阵、运行时能力清单
└── guides/                   # CUI 手册、TUI 键位、Web 部署与安全、灾难恢复
```

**Structure Decision**: 采用任务书 §6.2 的 13-crate Cargo workspace，`crates/` 下按 domain→app→service→store→adapter 单向依赖；`tests/` 分为 unit/contract/integration/crash/stability/fixtures 六类；CI 与基准脚本、ADR 与矩阵文档独立成目录以承载 §12.2 交付物。crate 名允许在实现前调整，但依赖方向与"前端不直接触库"（ADR-003）不变；两个尚无稳定接口的模块优先在同一 crate 内以模块边界表达，避免过度拆分（§6.2）。

## Complexity Tracking

> **Fill ONLY if Constitution Check has violations that must be justified**

**违规：无。** C-01—C-08 全部 PASS，本节不启用。

记录两项**非违规的观察项**（不构成门禁违规，仅登记以便后续复核）：

| 观察 | 说明 | 处置 |
|---|---|---|
| O-01 依赖面较宽 | Web/SSH 服务端 crate 选型未定（`[待验证]`），触及 C-06 边界 | 不表述为已交付；M0/M6/M7 依平台验证证据定案（research.md R-05/R-06） |
| O-02 范围大（M0—M9 单 feature） | 与"一个 feature 一个 spec"的常规粒度相比偏大 | 属用户明确决定的范围（全量基线 + M0 起步）；tasks.md 以里程碑为阶段边界，M0 为建议 MVP |
