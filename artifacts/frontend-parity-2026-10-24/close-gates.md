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

## 6. 补遗复跑（2026-10-25）

### 6.1 Web 运行时三取证重跑（真实 Firefox）

`python script/web_runtime_probe.py`（临时脚本，按规则回收）：Firefox 155.0.1 + Marionette 驱动
真实窗口，`serve --web` 指向 `artifacts/web-demo`（`seed_web_demo` 的 6 条事件夹具：
`/api/events?after=6`）。三组判据全部通过（`artifacts/web-runtime-2026-10-25/report.json`）：

| 判据 | 实测 |
|---|---|
| 最小化 3 s | `visibilityState` hidden；`/api` 请求 **0**、DOM 变更 **0**、rAF **0** |
| 还原恢复 | 单次恢复批次 6 次请求全部 200；其后事件轮询 5 次、相邻间隔 1.09—1.15 s（≈1 Hz；14 247／15 355／16 444 ms） |
| 两轮挂起/恢复 | 游标恒为 **6**、事件日志恰 **6** 条（无重复、无丢失） |
| 动效两分支 | reduce：opacity 恒空、150 ms 定时器 0；no-preference：opacity `0.6` → 空、`setTimeout(150)` 恰 1 次 |
| 禁用 GPU 20 轮 | 全部 hidden→visible；`canvas` 0、`getContext` 0、rAF 0、异常与未处理拒绝为空 |

截图 `artifacts/web-runtime-2026-10-25/visible.png`；运行日志 `run.log`。
与 2026-10-24 首轮（3 条事件、游标 3）相比，本轮用真实写入路径的 6 条事件夹具，结论一致。

## 7. 未取得项（显式登记，不冒充通过）

1. **GitHub Actions 远端运行**：已取得（2026-10-25），见 §8。
2. **V-003 真实控制台人工证据**：`tui-smoke.py` 只覆盖无需交互终端三项；真实键盘／resize／
   Ctrl-C 回显恢复仍属人工证据缺口（Check 24 既有口径），本批次未重跑。
3. **session.archive 的确认与审计口径**：本次收口复核发现登记（`PreviewConfirm` +
   `audited = true`）与实现（四端无确认、store 不写 `db_purge` 审计）冲突，已登记为
   CHANGELOG 已知未完成第 3 项，未在本次擅改。

## 8. 远端 CI 证据（2026-10-25）

远端为 `https://github.com/FreeBSD-Ask/whcode.git`，测试入口唯一为 `.github/workflows/ci.yml`
（ADR-037）。下列结论取自 GitHub REST 匿名查询 `GET /repos/{owner}/{repo}/actions/runs/{id}`
与 `/runs/{id}/jobs`（`status`、`conclusion`、逐 step 结论），非本地复述：

| run | head | 结论 | 失败步骤 |
|---|---|---|---|
| [40118942627](https://github.com/FreeBSD-Ask/whcode/actions/runs/40118942627) | `78bf0a6` | failure | `measurements` 的“NFR-207/NFR-209 性能预算（独占执行）” |
| [40126145778](https://github.com/FreeBSD-Ask/whcode/actions/runs/40126145778) | `1903806` | success | 无 |

run 40118942627 中五个平台 job 均为 success，仅 `measurements` 失败；处置为把性能预算与测试进程
RSS 移入受控 `perf-budget`（仅 `workflow_dispatch` 且 `perf=true`）、`measurements` 安装
`requirements-dev.txt` 的 `duckdb==2.0.0` 修复 NFR-202 夹具种子，预算值未改。run 40126145778
（含本批次 `551fe99`、`7cd25b3`、`5e5981d`、`aa48469`）逐 job 结论：

| job | 结论 |
|---|---|
| `platform-windows` | success |
| `platform-freebsd` | success |
| `platform-netbsd` | success |
| `platform-dragonfly` | success |
| `platform-openbsd` | success |
| `measurements` | success |
| `desktop`、`perf-budget`、`action-audit` | skipped（按门控：`desktop`／`perf-budget` 需 `workflow_dispatch` 且 `perf=true`；`action-audit` 仅 schedule／dispatch） |

该 run 的 `measurements` 日志记录 NFR-201 中位 31.27 MB、NFR-202 中位 35.51 MB、NFR-203 最大
增量 1.31 MB、30 秒冒烟事件缺失 0；本文件未复核日志原文（匿名读取 job 日志返回 403），数值以
`checklist.md` 计数声明中登记的读数为准并标为二手。

**边界**：run 40126145778 早于本 §6 的补遗复跑与本节改动，故它证明的是 T181 收口主体的门禁
（fmt／clippy／全量测试／五平台／短测量），不覆盖本次补遗的文档与证据文件改动。
