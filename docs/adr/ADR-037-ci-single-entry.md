# ADR-037：测试统一入口与 CI 合并

- **状态**：已批准（2026-10-25，用户指令“把所有测试重构合并全部使用 github ci 进行”）
- **来源**：用户选择：两份工作流（`ci.yml` + 长跑）；桌面型测试迁入 CI 用 headless／虚拟显示口径；
  `local/` 脚本保留为复现工具、不再产生验收结论
- **责任人**：CI 与门禁维护者
- **退出条件**：GitHub Actions 配额或 runner 能力不可接受、6 小时上限变化、需要自托管 runner、
  或某个测试无法在 CI 产生可归因信号且必须回到人工证据时，修订本 ADR

## 决策

1. **测试工作流合并为两份**：`.github/workflows/ci.yml` 承担门禁、五平台矩阵、全部功能测试、
   短测量、TUI/Web 冒烟与桌面型测试入口；`.github/workflows/long-run.yml` 只承担 5.5 小时
   稳定性长跑（超出单 job 6 小时窗口，ADR-033）。原 `platform-matrix.yml`、`stability.yml`、
   `action-version-audit.yml` 删除，内容并入 `ci.yml`。
2. **触发与门控**：push `main`、`pull_request`、`workflow_dispatch` 运行测试；测量 job 只在
   push `main` 与手动触发时运行（PR 不跑，避免配额与不可归因波动）；`perf-budget`（NFR-207/209
   与测试进程 RSS）与 `desktop` 仅由 `workflow_dispatch` 的 `perf`／`desktop` 输入触发，
   避免共享 runner 的邻居负载把性能测量变成红绿噪声；每周一 schedule 只运行 `action-audit` job。
3. **桌面型测试进入 CI 入口**：GUI 探针作为 `workflow_dispatch` 的 `desktop` 输入触发；
   托管 runner 为非交互会话，结果按 headless／虚拟显示口径记录，不替代真实桌面人工证据
   （V-003、FR-714）。TUI 只自动化三项（`--version`、`doctor --json`、非交互优雅退出），
   物理键盘／resize／Ctrl-C／强杀残留仍为人工缺口，不得以 CI 绿冒充通过。
4. **测量脚本入库**：EOL 审计、NFR-201/202/205/206 真实进程测量、测试进程资源测量迁入
   `.github/scripts/`（`check-eol.py`、`benchmarks/measure-nfr-budgets.py`、
   `benchmarks/measure-resources.py`），CI 与本地执行同一份实现；`local/` 不再保留同名副本，
   避免双源漂移。`local/` 仅保留一次性证据工具（如 ADR-036 的 A/B 脚本）作复现用途。
5. **验收入口唯一**：测试“通过”结论只能来自 CI 日志与 artifact；本地运行仅用于复现与排障。
   远端仓库已配置（`https://github.com/FreeBSD-Ask/whcode.git`），CI 已实跑并取得全绿证据
   （见“修订记录”）；此后不得再以“无远端/未运行”为由用本地全绿代替 CI 结论。

## 证据与约束

- `ci.yml` 的 Windows 条目在既有步骤上新增：EOL 门禁、TUI 冒烟、Web 服务端冒烟（`/api/status`
  需先建会话、`/api/sessions`、首页 HTML）、证据 artifact。
- `measurements` job 串行执行 `writer_stress`、`memory_limit`、`query_plan`、NFR-203／NFR-206、
  NFR-201／202 真实进程测量与 30 秒长跑冒烟；NFR-207／209 与测试进程 RSS/CPU 归
  `perf-budget` job，NFR-205 归 `desktop` job，均不在 push 门禁内。同一 runner 内不并行，
  避免测量互相污染；GitHub 各 job 使用独立 runner，平台矩阵与测量不共享 CPU。
- `measurements` 在构建前安装 `requirements-dev.txt` 中的 Python `duckdb==2.0.0`（与引擎同版本），
  用于 NFR-202 的 10 万事件夹具种子；该依赖只在开发/CI 测量使用，产品运行期零第三方依赖。
- 桌面 job 安装 `requirements-dev.txt`（Pillow 与 duckdb）并运行 `gui-probe.py`；失败即信号，
  不使用 `continue-on-error` 掩盖。
- `check-workflows.py`、`check-actions.py`、`check_counts.py`、`traceability.py`、
  `check-eol.py` 构成本地可复现的离线门禁；新工作流上线前已在本机通过。

## 正负和解

- **正面**：测试入口、测量与门禁集中到两份文件，引用与权限只有一处；所有测试都能在 CI
  找到对应 job；脚本入库后不再依赖本机 `local/`；桌面型测试获得 CI 入口与统一报告。
- **负面**：每次 push 的 Windows 与测量 job 消耗较多 runner 分钟；测量只在 `main` 与手动
  运行，分支上的性能回归反馈变慢；桌面型测试在托管 runner 上可能因非交互会话失败，
  需人工判断是环境限制还是产品缺陷；旧工作流文件名进入历史记录，需要引用迁移说明。
- **承担者**：CI 与门禁维护者、测量负责人、桌面证据复核人。
- **和解理由**：以“一份 CI 入口 + 一份长跑入口”换取可审计与低漂移；桌面型测试保留在 CI
  入口但不宣称等价真实桌面；测量脚本入库而不是复制，避免同一逻辑两份实现。

## 复审触发

- GitHub 取消 6 小时 job 上限或提供桌面型 runner；
- CI 配额不足以支撑 push 触发的测量；
- 桌面型测试连续因环境原因失败，需要改自托管 runner 或回到人工证据；
- 无远端仓库状态改变（首次推送后须实跑 CI 并登记首轮结果）；
- `measurements` 在托管 runner 再次出现不可归因的预算红：按本 ADR 口径移入受控 job 或登记
  环境差异，**不得放宽 ADR-025 的冻结值**。

## 修订记录

- **2026-10-25（首轮实跑缺口收口）**：run 40118942627 的 `platform-windows` success，但整体 run
  被 `measurements` 拖红，两处根因分别处置：
  1. 托管 runner 为共享环境，NFR-207／209 的绝对延迟未达冻结预算 → 将 NFR-207／209 与测试
     进程 RSS/CPU 移入 `perf-budget` job（仅 `workflow_dispatch` 且 `perf=true`），预算值不变
     （仍为 ADR-025 冻结值），push 门禁只保留对邻居负载不敏感的稳定测量；
  2. `measurements` 的 NFR-202 夹具种子 `import duckdb` 失败（runner 未预装）→
     `requirements-dev.txt` 登记 `duckdb==2.0.0`，job 内先 `pip install` 再构建。
- **收口证据**：run 40126145778（commit `1903806`）全绿——`platform-{windows,freebsd,netbsd,openbsd,dragonfly}`
  与 `measurements` 均 success，`desktop`／`perf-budget`／`action-audit` 按门控 skipped（预期）。
  `measurements` 关键信号：NFR-201 中位 31.27 MB／p95 31.28 MB／峰值 31.43 MB（预算 ≤ 36 MB）；
  NFR-202 中位 35.51 MB／峰值 37.13 MB（预算 ≤ 80 MB）；NFR-203 20 次切换最大增量 1.31 MB
  （预算 ≤ 10 MB）；NFR-206 限速流式 224 chunks／9.10 s；30 秒长跑冒烟事件缺失 0
  （`failed_tasks=1` 为夹具播种的计数校验样本，非产品失败）。
- **2026-10-25（构建与发布独立，ADR-039）**：工作流由两份变三份；`ci.yml` 不再承担构建、
  发布构建、打包安装验证与 TUI/Web 冒烟，这些迁入 `build-release.yml`；`ci.yml` 保留门禁、
  五平台矩阵、功能测试、短测量、桌面入口与每周 action 审计；测试“通过”口径仍以 `ci.yml`
  为准，构建与发布产物结论以 `build-release.yml` 为准；artifact 名称不变。
