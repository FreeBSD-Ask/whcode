# ADR-013：平台范围收敛——Windows 11 amd64 为唯一验收目标

- **状态**：**已批准（2026-10-21，用户决策）**
- **来源**：用户指令“目前专注 Windows 11 amd64，不考虑任何平台 32 位”；《WHCode 技术任务书 v3.0》§9 平台矩阵、§12.1 里程碑门槛；ADR-010（平台可构建与 DuckDB 原生构建分开验收）、ADR-011（OpenBSD/DragonFly 改客户端）
- **责任人**：平台矩阵负责人
- **退出条件**：需要恢复其他平台时，必须提出新的 ADR 并给出该平台的完整证据链（构建、数据库冒烟、TUI、依赖记录、SSH 或客户端连通性）

## 决策

1. **唯一验收目标**：Windows 11 `x86_64`（目标三元组 `x86_64-pc-windows-msvc`）。
   M0—M9 的“平台通过”判据只针对该目标；其他平台的结论一律不进入验收结论。
2. **不考虑任何 32 位平台**：不使用 `i686`/`i586` 等 32 位目标，不为 32 位做兼容、CI 或性能测量。
   已安装/可安装的 32 位目标列表不作为承诺依据。
3. **BSD 全系列延后**（FreeBSD、NetBSD、OpenBSD、DragonFly BSD）：
   - 不删除既有代码、文档与 CI 定义，但**不作为当前验收目标**；
   - BSD CI 原为**手工触发**（`workflow_dispatch`）；**2026-10-24 用户指令两次修订**：
     FreeBSD/NetBSD 先改为随 `main` 自动运行；随后四个 BSD 全部改为真实 VM 运行（版本取
     各平台最新 release，`-current` 因上游 action 不支持而登记为缺口）。结论仍只作平台
     证据，不进入验收。详见文末修订记录；
   - 相关任务（如 US8/M7 的 SSH 服务端与四平台客户端证据）标记为**延后**而非完成；
   - ADR-005/ADR-010/ADR-011 的决策不被撤销：它们仍是未来的方向，只是**不在本轮验收范围**。
4. **不做交叉编译冒充验证**：延后平台不产出“代码级交叉检查通过”当作平台结论的证据；历史交叉检查记录保留为历史材料，不引用为验收。

## 理由

- 任务书 §9 的平台矩阵是**承诺清单**，不是“必须同时交付”的强制并行项；同时承诺五个平台会让当前阶段无法聚焦（FreeBSD/NetBSD 无环境、DuckDB 无 BSD 预编译包、`aws-lc-sys` 需目标 C 工具链——三项都是外部依赖阻塞）。
- Windows 11 amd64 已有完整证据链（构建、依赖记录含 SHA256、数据库冒烟四步、TUI 渲染、写者基线 6 921 条/秒、107+ 测试绿），是唯一可以“立即产出可用版本”的目标。
- 32 位平台在嵌入式数据库、内存预算（NFR-201—210）与终端能力上均缺少可验证基线；纳入范围只会制造无法兑现的承诺。

## 替代方案

- **维持五平台并行承诺**：需要 FreeBSD/NetBSD 真实设备或 CI runner、BSD 的 DuckDB 构建方案、TLS 后端替换决策；任一缺失就让 M0 长期无法收口（现状即如此）。
- **只保留 Windows 但偷偷保留 BSD 自动 CI**：会产生“看起来在支持”的假信号，且每 push 触发昂贵 VM 构建；不允许。
  （**2026-10-24 用户指令修订**：允许 BSD CI 自动运行，但结论仍不进入验收；见修订记录。）
- **改回 32 位兼容**：与用户明确指令冲突，且无收益证据。

## 修订记录

- **2026-10-24（用户指令：BSD CI 自动运行）**：FreeBSD/NetBSD 工作流增加 `push: branches: [main]`
  触发，在 vmactions VM 中真实运行 `cargo check`、不依赖 libduckdb 的包级测试与依赖记录；
  DuckDB 原生构建与全量测试保留为显式 Go/No-Go（失败属 R-01 证据，不使 job 变红）。
  **唯一验收目标仍是 Windows 11 amd64**；BSD 结果只作平台证据，不进入验收结论，也不解除
  T099／M7 与 BSD 任务延后。OpenBSD/DragonFly 无原生测试，保持手工占位。
  代价（私有仓库 Actions 分钟、VM 构建慢、可能长期红灯）由平台矩阵负责人承担；
  失效条件：Actions 配额不可接受或用户撤回指令时，回到手工触发。
- **2026-10-24（用户指令：四个 BSD 全部真实运行、版本取最新）**：`platform-matrix.yml` 把
  OpenBSD（`vmactions/openbsd-vm`）与 DragonFly（`cross-platform-actions`；vmactions 无
  `dragonfly-vm`，已核实 404）从 echo 占位改为真实 VM 运行。版本口径：FreeBSD `release: "15"`、
  NetBSD `release: "11"`、OpenBSD `release: "8"`（action 自动取该系列最新点版本），DragonFly
  精确 `6.4.2`（该 action 不支持前缀匹配）。
  **`-current` 未覆盖（显式缺口）**：上游 vmactions 的 freebsd/netbsd/openbsd action 只提供
  release 表（FreeBSD 10—15、NetBSD 9—11、OpenBSD 6.9—8.0），无 -current／快照输入；FreeBSD
  16.0-CURRENT 在上游仓库标为 shelved（未启用）。因此不得以 release 结果冒充 current 证据；
  恢复条件：上游 action 提供 current 镜像，或另行批准自管 QEMU 快照方案（需新 ADR）。
  工具链低于 `Cargo.toml` 的 `rust-version` 时（如 NetBSD pkgsrc rust 1.96.1 < 1.99）按
  环境限制显式 SKIP 并写明，不伪造通过。代价与失效条件同上一修订（VM 分钟、长期红灯风险；
  配额不可接受或上游 action 不再维护时回到手工触发）。

## 影响

- `spec.md`：User Story 1（M0）的目标平台、独立测试与验收场景改为“Windows 11 amd64 单一目标”；BSD 相关能力（FR-607/FR-611、V-004）标注**延后**，不删需求。
- `docs/matrices/platform-matrix.md`：矩阵区分“在范围内（Windows）”与“延后（BSD，不验收）”；性能预算冻结条件中的“分平台数据”改为“延后平台豁免，但需在恢复时补齐”。
- `docs/matrices/decisions.md`：新增 D-14（平台范围收敛）。
- `tasks.md`：US8（M7）与 BSD 相关任务标注“延后（ADR-013）”；不删除任务，不勾选为完成。
- `.github/workflows/platform-{freebsd,netbsd,openbsd,dragonfly}.yml`：触发器改为仅 `workflow_dispatch`，文件头写明“延后，不随 push 运行”。
- `README.md`：平台表改为“在范围内 / 延后”。
- **不受影响**：`whcode-domain`/`whcode-app`/`whcode-store`/`whcode-tui`/`whcode-cui` 的代码本身保持平台无关；`whcode-platform` 的 `cfg` 分支继续保留（未来恢复时无需重写）。

## 口径约束（防止误读）

- “延后”**不等于**“已支持”或“已放弃”：不得在 README、发行说明或对外材料中声称 BSD 可用；
- 不得把 Windows 的实测数字外推为其他平台结论（G-01）；
- 恢复任一平台时，必须补齐：构建与依赖记录（含 DuckDB 引擎版本）、数据库冒烟四步、TUI 或 CUI 可运行证据、以及该平台的性能分位数——缺一不得宣称通过。
