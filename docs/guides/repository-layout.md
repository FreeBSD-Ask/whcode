# 仓库分层与目录分类原则

**日期**： 2026-10-21（2026-10-24 修订：`ci/` 整体迁入 `.github/scripts/`） | **适用**： 全仓库 | **强制级**： 新增文件必须归入下表之一，建新顶层目录需先有 ADR

> 路径映射：2026-10-24 起 `ci/` 不再作为顶层目录，原 `ci/**` 全部迁入 `.github/scripts/**`。
> 历史记录（ADR、CHANGELOG、`specs/` 的任务与检查点、`artifacts/` 证据）中的 `ci/` 路径
> 按此映射解读，不回写历史。

本仓库按“**产品代码 / 验证 / 交付（产物与资源） / 治理 / 本地**”分层，每类一个顶层目录，
依赖方向单向（产品 → 被验证 → 交付与治理），本地产物永不入库。

## 1. 顶层目录职责

| 目录 | 类别 | 职责 | 入库规则 |
|---|---|---|---|
| `crates/` | 产品代码 | 14 个成员 crate，按 §6.2 单向依赖（domain → app → 服务 → 前端适配器） | 入库 |
| `tests/` | 验证 | 六类测试：unit / contract / integration / crash / stability / fixtures；每类有 README | 入库 |
| `docs/` | 治理文档 | `adr/`（决策记录）、`matrices/`（平台/能力/决策矩阵）、`guides/`（操作指南） | 入库 |
| `engineering-doctrine/` | 治理（可移植） | 工程哲学与开发准则的独立版本，供其他项目复制；不依赖本仓库代码 | 入库 |
| `specs/` | 治理（需求基线） | 规格、计划、任务、清单、契约；`source-*.md` 为只读源基线 | 入库（源基线禁改） |
| `artifacts/` | 交付产物 | 依赖记录、基准报告、M0 报告等运行期证据 | **默认忽略**；治理报告（如 `m0-report.md`）经 `.gitignore` 放行后入库 |
| `assets/` | 交付（资源） | 可再分发的内置资源：`fonts/`（完整 OFL 字体、许可原文、`SOURCES.md`） | 入库（仅允许 OFL 等可再分发许可；新增资源类型需 ADR） |
| `vendor/` | 交付（二进制） | 预编译 DuckDB（SHA256 校验后解包，PROVENANCE 留证） | **忽略**（可再生成） |
| `script/` | 本地 | 临时脚本、探测脚本、运行日志（不出库；一次性脚本与日志由 `.github/scripts/clean.py` 回收） | **忽略** |
| `target/` | 本地 | Rust 构建产物 | **忽略** |
| `state/` | 本地 | 环境工具自动重建的状态目录（内容不可控，视为不存在） | **忽略** |
| `.github/` | 治理（流程与自动化） | `workflows/`（五平台流水线：Windows 在范围内自动运行，四 BSD 仅手工触发）；`scripts/`（原 `ci/`：平台矩阵、基准、打包与门禁脚本）；PR/Issue 模板；`dependabot.yml`（每日依赖与 Actions 更新，仅开 PR、不自动合并） | 入库 |
| `.specify/` | 治理（工具链） | Spec-Kit 1.0.13 模板、PowerShell 脚本、集成清单与项目宪法（`memory/constitution.md`）；`feature.json` 为当前 feature 指针，切换时产生显式差异 | 入库（2026-10-24 全量纳入） |

根文件只允许工程清单（`Cargo.toml`、`Cargo.lock`、`rust-toolchain.toml`、`rustfmt.toml`）、
标准开源文件（`LICENSE`、`README.md`、`CONTRIBUTING.md`、`SECURITY.md`、`CHANGELOG.md`）、
代理协作入口（`AGENTS.md`）与编码策略文件（`.gitattributes`、`.editorconfig`）；点开头条目不受
`test_layout` 枚举限制，其中 `.specify/` 已按治理工具链入库，其余点开头目录仍视为本机工具状态；由 `test_layout` 门禁强制。

## 2. crate 内部分类（§6.2 单向依赖）

```text
whcode-domain   ← 叶子：状态/事件/错误/门禁/审计/追踪类型，禁止依赖任何内部 crate
whcode-platform ← 唯一允许平台分支（cfg(target_os) / CARGO_CFG_*）的 crate
whcode-app      ← 命令编排/权限/快照；禁止依赖前端与存储实现
whcode-store    ← 唯一 DuckDB 链接点与写者服务
whcode-llm / whcode-speckit / whcode-tools / whcode-runtime ← 领域服务
whcode-tui / whcode-cui / whcode-web / whcode-ssh / whcode-gui ← 前端适配器：只依赖 whcode-app，禁止触库（`whcode-gui` 规划中，见 ADR-027／T150）
whcode-bin      ← 装配根：配置、日志、feature 组装
```

由 `tests/contract/test_layering.rs` 可执行强制：前端不触库、domain 零内部依赖、前端互不依赖、
平台分支集中。

## 3. 命名与放置约定

- 新 crate 进 `crates/whcode-<域>/`；新测试按性质进 `tests/<六类之一>/`，并在该类 README 登记；
- 新自动化脚本按平台进 `.github/scripts/platform-matrix/`、按测量进 `.github/scripts/benchmarks/`，通用门禁/打包脚本放 `.github/scripts/` 根级；
- 新文档按用途进 `docs/{adr,matrices,guides}/`；需求变更走 `specs/` 流程，**不**写进 docs；
- 新内置资源进 `assets/<类型>/`：只放可再分发许可的文件，逐项在 `SOURCES.md`（或同级说明）
  记录上游、版本、许可与 SHA-256；完整字体的运行子集由 `.github/scripts/subset-fonts.py` 生成，不手工改；
- 运行期证据写 `artifacts/`（不入库），要版本化的治理结论写 `docs/` 或放行清单中的 artifacts 文件；
- 临时/探测脚本一律放 `script/`（不入库；一次性脚本与运行日志由 `.github/scripts/clean.py` 回收，
  长期工具、凭据与考据数据在其保留清单中显式豁免——2026-10-24 规则更新）。

## 4. 反模式（禁止）

- 在 crate 里新建 `utils.rs` 杂物间——工具按域归属 crate，或提升到 `whcode-domain`；
- 把“待办脚本”放仓库根或 `docs/`——进 `script/` 或删除；
- 在产品代码里写死路径/版本（如 `vendor/duckdb/1.5.6` 之外的硬编码）——版本经配置与环境变量注入；
- 文档之间绕过链接互相复制内容——一律链接到唯一出处（spec → ADR → 矩阵 → 指南）。
