# 四端对等收口证据（T181，2026-10-24 变更／2026-10-25 02:50 执行）

**范围**：FR-619—625 转正主账本、登记表交付格翻转、矩阵改为逐行受校验产物、四份指南与
CUI 手册回填。**基线**：执行时 HEAD `02c8601`（含并行变更 `add-real-llm-provider` 的未提交
改动，本批次未触碰其文件）。

**纪律**：下列每项均为本机实跑的命令与可归因结果；未取得项显式标注。零样本、退出码缺失
或仅凭静态推断的项不计为通过。

## 1. 账本与追溯

| 命令 | 结果 |
|---|---|
| `python .github/scripts/check_counts.py` | `actual: FR=93 tasks=181 checks=271`；`counts consistent`；退出码 0 |
| `python .github/scripts/traceability.py` | `FR→任务缺 0，FR→验收缺 0，幻影 FR 0，幻影任务 0，已登记缺口 0`；`追溯一致：无断链`；退出码 0 |

口径说明：T162—T167 为并行变更 `add-real-llm-provider` 的占位任务（未勾选；该变更 spec 与
ADR-031 未入库），其编号承载使 `check_counts.py` 的 1..N 连续判据成立；本清单不为其代建
Check。新增 FR 计数来自 FR-619—625 转正（86 → 93）。

## 2. 对等门禁与登记表

| 命令 | 结果 |
|---|---|
| `cargo test -p whcode-tests --test frontend_parity` | 10 passed / 0 failed（含 `feature_parity_matrix_rows_match_registry`、负向 `tampered_matrix_is_rejected`） |
| `cargo test -p whcode-app --lib surface::` | 16 passed / 0 failed（含 `delivered_surfaces_are_implemented_and_pending_keep_milestones`） |
| `cargo test -p whcode-cui` | 16 passed / 0 failed（`retained_commands_map_to_reserved_surfaces` 已移除 runtime list；`implemented_commands_map_to_implemented_surfaces` 增补 `context budget` 与 `runtime list`） |
| `cargo test -p whcode-tui` | 60 passed / 0 failed（含 `write_and_theme_actions_are_implemented`、游标零重绘与推进重绘用例） |

矩阵逐行比对覆盖：行数 54、行序与登记表一致、类别与效果、四端状态符号（✅／◐／⏳／—）、
每个 Excluded 的理由码、每个 Reserved 的里程碑。负向用例证明篡改状态符号或理由码会使
校验失败（非空转）。

## 3. 结构与规范门禁

| 命令 | 结果 |
|---|---|
| `cargo fmt --all --check` | 无差异（`FMT_OK`） |
| `cargo clippy --workspace --all-targets -- -D warnings` | 零告警（退出码 0） |
| `cargo test --locked --workspace` | 退出码 0（全部测试目标通过；Doc-tests 均 0 例） |
| `cargo test -p whcode-tests --test layering` | 6 passed / 0 failed |
| `cargo test -p whcode-tests --test layout` | 6 passed / 0 failed |
| `python .github/scripts/check-actions.py` | 通过（离线 SHA 比对；清单复核日期 2026-10-24） |
| `python .github/scripts/check-workflows.py` | 通过（无 job 级 matrix `if`；顶层 permissions 齐备） |
| `python .github/scripts/check-eol.py` | 486 个跟踪文件、跳过二进制 68；CRLF 0、BOM 0 |
| `python .github/scripts/benchmarks/check-frontend-budget.py` | JS gzip 8206 B／CSS gzip 1142 B，无违例 |

## 4. 资源复测（FR-624 判据）

`python .github/scripts/benchmarks/measure-nfr-budgets.py --only nfr205`：
真实控制台 TUI 空闲 CPU **0.156%**（冻结预算 ≤ 0.5% 单核，ADR-025），判定 PASS；
报告 `artifacts/nfr-budget-report.json`。未回归“0.0% 量级”口径（0.156% 属该量级，
不得表述为 0.000%）。

## 5. 发布包安装验证

`python .github/scripts/package-windows.py --verify --record artifacts/install-verify-windows-amd64.txt`：
发布包 30 911 567 字节；`whcode.exe --version`、`doctor --json`、临时工作区 `db stats --json`
三条命令退出码均为 0，库文件 798 720 字节实际生成；记录写入
`artifacts/install-verify-windows-amd64.txt`。

## 6. 未取得项（显式登记，不冒充通过）

1. **GitHub Actions 首轮运行**：仓库无远端，CI 从未实际触发（Check 3 既有缺口）；本批次
   只证明本地门禁链全绿，不证明 CI 运行结果。
2. **真实 Firefox 运行时三取证**：沿用 `artifacts/web-runtime-2026-10-24/`（2026-10-24
   批次，Marionette 驱动真实窗口），本批次未重跑；原因：该证据与主题／501 语义无耦合，
   重跑不增加信息。
3. **V-003 真实控制台人工证据**：`tui-smoke.py` 只覆盖无需交互终端三项；真实键盘／resize／
   Ctrl-C 回显恢复仍属人工证据缺口（Check 24 既有口径），本批次未重跑。
4. **session.archive 的确认与审计口径**：本次收口复核发现登记（`PreviewConfirm` +
   `audited = true`）与实现（四端无确认、store 不写 `db_purge` 审计）冲突，已登记为
   CHANGELOG 已知未完成第 3 项，未在本次擅改。
