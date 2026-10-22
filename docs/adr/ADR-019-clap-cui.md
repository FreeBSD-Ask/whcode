# ADR-019：CUI 命令树采用 clap

- **状态**：已批准（2026-10-22，M5/T084 实施前置）
- **来源**：M5/T084 实施前的依赖决策。`plan.md` 的 Primary Dependencies 已列 clap
  （CUI 命令树）；`contracts/cui-cli.md` §1 定义稳定命令树。本 ADR 记录依赖引入决策。
- **责任人**：CUI 负责人
- **退出条件**：若 clap 的 derive 宏出现与零 unsafe 或 MSRV 冲突的版本，须复审并
  评估退回手写参数解析（契约不变）。

## 决策

1. CUI 命令树采用 **clap（derive API）**：`crates/whcode-cui` 与 `crates/whcode-bin`
   新增 `clap` 依赖（`features = ["derive"]`，default-features 关闭以最小化）；
2. 命令树结构以 `contracts/cui-cli.md` §1 为唯一接口草案；参数命名、退出码、
   错误 JSON 与 `--json` 语义按契约 §2—4 统一；
3. clap 只做**参数解析**：命令语义（权限、dry-run、审计）仍全部在 `whcode-app`
   （分层不变，ADR-003/005）；CUI 不触库。

## 理由

- `plan.md` 既定选型；手写解析器需自行覆盖子命令/参数校验/`--help` 生成，
  与 clap 相比无真实收益（解析器不是本项目的核心矛盾）；
- clap 零 unsafe（`#[forbid(unsafe_code)]` workspace lints 继承不受影响）、
  无 MSVC 工具链问题（ADR-013 约束不受影响）；
- 非交互脚本路径（ADR-009）依赖稳定的参数与退出码，clap 的结构化解析降低漂移风险。

## 替代方案（已否决）

- **手写参数解析**：`--help`/子命令/类型校验自行实现，维护成本高且无功能收益。
- **不交付命令树**：违反 T084 与 cui-cli 契约。

## 影响

- workspace `Cargo.toml` 新增 `clap.workspace = true`（locked 依赖记录）；
- `crates/whcode-cui/src/cli.rs`（T084）与 `tests/contract/test_tui_cui_parity.rs`（T088）
  随本 ADR 同 PR 交付；
- 二进制体积与编译时间增长（MSVC 构建，Windows 11 amd64 验收不受影响）。
