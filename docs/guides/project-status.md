# 项目状态快照（2026-11-16）

本页是**带日期的状态快照**，供了解进度用。权威账本始终是
[`tasks.md`](../../specs/001-whcode-harness/tasks.md)（任务勾选）与
[`checklist.md`](../../specs/001-whcode-harness/checklist.md)（验收检查点）。
两者与 `spec.md` 的 FR 三方追溯由 `python .github/scripts/traceability.py` 与
`check_counts.py` 在 CI 中强制；本页数字来自这两个文件在 2026-11-16 的实测统计，
过期即失效，不得引用为验收结论。CI 结论以最新 run 为准，本页不写“当前红／绿”。

## 总量

| 项 | 数量 |
|---|---|
| 功能需求 FR | 171 |
| 任务 | 369（已完成 368，未完成 1——T369 待外部条件回填） |
| 验收检查点 | 465（已有复核记录 464，missing 1——Check 464 待 CI 实跑回填） |

证据态分布（ADR-046，`python .github/scripts/check_counts.py` 实测）：exercised 414、
outcome-supported 49、missing 1。ADR-060 Phase 1、proposal-v3 的 P1.9／P1.10／P2.2b／P3.3／
P5.2／P5.3／P5.6／P6.2—P6.4／P7.1—P7.4 均已收口，最近批次（Check 462—463，主题与配置
更名＋FreeBSD 交付目标落地）的取证见
`checklist.md` 与 `CHANGELOG.md`。
并行会话在途（截至本快照）：无（工作树干净，未入账 WIP 已清空）。
proposal-v3 全部 45 条提案的执行态比对见
[`artifacts/proposal-v3-execution-audit-2026-11-11.md`](../../artifacts/proposal-v3-execution-audit-2026-11-11.md)：
已实现 19、部分 2、在途 1、未实现 23（**2026-11-11 快照**；其后 P1.9／P1.10／P7.3 与
P2.2b／P3.3／P5.x／P6.x／P7.1／P7.2 的进展见 `CHANGELOG.md`，本表不重算）。
回填后须重跑上述脚本并更新本表，不得沿用旧数字。

## 四端能力面

`docs/matrices/feature-parity.md` 为受校验产物（来源 `whcode-app::surface::SURFACES`），
共 **78 条**能力面（60 Command＋18 View）；状态符号为 已实现、部分、预留，
每端另有 1—2 行“不提供”（该端按理由码排除）：

| 端 | 已实现 | 部分 | 预留 | 不提供 |
|---|---|---|---|---|
| CUI | 43 | 22 | 11 | 2 |
| TUI | 28 | 28 | 18 | 4 |
| GUI | 34 | 34 | 7 | 3 |
| Web | 44 | 17 | 13 | 4 |

## 里程碑

| 里程碑 | 状态 |
|---|---|
| M0 平台可行性与基线冻结 | 已发布（`v0.1.0`，2026-11-17 打标签（提交 `712227c`），发布构建 run `43911920856` 三通道 success，产物 amd64／arm64 zip＋sha256；ADR-016 条件 V-003 已取证、V-006 由 ADR-025 收口，V-004 跨机节点按 ADR-013 归 M7 延后） |
| M1 存储与事件基座、M2 会话与任务、M3 LLM 与上下文、M4 SDD 门禁、M5 TUI/CUI、M6 Web | 实现完成，测试在库 |
| M7 SSH | 服务端实现完成（russh 0.64.1，ADR-030）；FreeBSD/NetBSD SSH 回环验收 GO（客户端＋服务端四项，run `41481740766`）；V-004 外部节点延后至 M7；NetBSD/OpenBSD 定位为 SSH 远程开发目标（ADR-013 修订 2026-11-03） |
| M8 可选执行运行时 | 实现完成，feature 默认关闭 |
| M9 优化与发布 | 已交付：兼容矩阵、运行时清单、热点优化（T115）、发布包与指南；稳定性（NFR-204）以 24h 深检收口通过（T116/Check 116/195，2026-11-02） |
| Phase 13 跨里程碑收尾 | 已完成（安全复查、quickstart 全流程、追溯审计） |
| 四端对等（FR-619—625） | 已完成（ADR-032）；65 条能力登记表（2026-11-11 实测）为单一事实源 |
| Phase 24 ZCode 全量体验对齐（ADR-057） | 已完成：立项任务 T244—T263 共 20 项全部转勾（B5 结题 T263 于 2026-11-11 收口）；治理与工程批次 T264—T275（ADR-060 Phase 1）、T276—T279、T280—T281 亦全部转勾 |
| ADR-059 路线 Phase 2（proposal-v3） | 部分交付：P3.4 迁移前备份（T280，ADR-063）、P7.4 工具目录（T281）已收口；P3.1 会话分叉在途但未入账；其余按 proposal-v3 §6 排序待开工 |

## 未完成项与原因

当前账本余 T369 一项（FreeBSD 包 CI 实跑取证回填，外部条件：需推送 tag 或手动触发）；
以下条目是仍开放的观察项、平台缺口与历史收口记录。

- **FreeBSD 构建交付与品牌更名（2026-11-17，用户批准）**：ADR-083 把 FreeBSD 在硬门禁证据
  通道之上升为**构建交付目标**——`build-release.yml` 新增 `package-freebsd` lane（tag／手动
  触发）：rustup 1.100.0＋DuckDB 2.0.0 源码构建＋release 构建（RPATH=$ORIGIN，随包带
  `libduckdb.so`），VM 内仓库外安装验证三条命令与 TUI/Web 冒烟后挂 GitHub Release；
  release job 六件资产数量断言＋`check-release.py --arch freebsd-amd64` 交叉校验；首个
  实跑证据待回填（T369／Check 464），仍不作验收结论、性能分位数未取得。同批完成品牌更名：
  主题 zcode-day／zcode-night 更名 whcode-day／whcode-night、工作区配置 `.zcode/*` 更名
  `.whcode/*`（ADR-053／054／057／070／078／079 修订；冻结色值逐值不变）。

- **CI 证据节奏（2026-11-16 变更，ADR-082，用户批准；首个全绿运行 run `43894584718`，`b803dde`——FreeBSD 28.2 分钟、ARM64 20.0、Windows 25.1、measurements 35.6，关键路径由 FreeBSD 转为 measurements）**：五平台矩阵只随**代码推送**
  产生证据——纯文档推送（`docs/`、`specs/`、`artifacts/`、`i18n/`、根级 `*.md`）走
  `docs-gates` 的十项文档门禁，不重建平台产物；`extreme_scale`（1M 事件）与 NetBSD 改由
  每周 schedule 与手动 dispatch 专项覆盖；同一 ref 的新运行会取消被覆盖的运行，**被取消
  的运行不是绿、不得引用**。因此“某提交没有平台 job”不等于失败，须看该提交是否含代码
  改动；引用证据时须给出 run 号与 job 结论。
- **BSD 验收（2026-11-03 已收口一批）**：Check 22/25/96/99/195/336 与 T099/T240 已转勾（run `41481740766` 硬门禁首次全绿，证据落库 `artifacts/ssh-acceptance/` 与 `artifacts/store-acceptance/`）。ADR-013 修订（2026-10-31）设 FreeBSD 为完整平台证据通道（硬门禁）；修订（2026-11-03，用户指令）定 **NetBSD/OpenBSD/DragonFly 仅支持 SSH 远程开发**（被 Windows/FreeBSD 内置 SSH 客户端连入的目标，不要求原生工具链/引擎；NetBSD 原生 SSH GO 维持为证据，NetBSD 原生 store 豁免——上游 DuckDB 2.0.0 依赖 glibc `features.h`；OpenBSD/DragonFly 节点证据走真实连入取证，DragonFly 同日追加同待遇）。**仍开放**：V-004 外部节点连通性（M7）；性能分位数未取得（perf 通道仅 Windows）。**2026-11-06 观察**：FreeBSD 通道在 run `42143683382` 出现 `store_gate = NO-GO`（DuckDB 源码构建报 `platform.hpp:22 'features.h' file not found`），与 2026-11-03 run `41481740766` 的 GO 不一致，**根因未归因**，不得读作代码回归；CI 结论以最新 run 为准，细节见 `docs/matrices/compatibility.md`。**2026-11-15 观察（当日三次运行，结论互不相同——以单次为准必错）**：手动 dispatch 的 run `43646760888`（`cebf3a4`）红于 FreeBSD `store_gate = NO-GO`（DuckDB 源码构建 `platform.hpp:22 'features.h' file not found`，上游硬编码）、同 run 的 `whcode-bin` 测试二进制退出路径 SIGSEGV 与 coverage-pilot `whcode-app 89.82% < 90%`；推送触发的 run `43661764421`（`e57723b`）FreeBSD 红于 `workspace 测试失败（rc=101）`，但**单线程复跑 139 项全过**——属已登记的 A2 债务（并行压力下的二进制退出路径 SIGSEGV），同 run 的 ARM64 红于 `taskkill` 255 竞态（已在 `e7f82da` 修）；run `43667118984`（`ba92fba`）FreeBSD 红于两条 `whcode-bin` 测试在 FreeBSD 上失败（断言硬编码 `\` 分隔符，已修）；同日 run `43664650070`（`e7f82da`）**全通道 success**（含 FreeBSD）。另发现一处 CI 注解缺陷并修复：BSD 结论注解原以 `*-store.txt` 通配读取，会把仓库里入库的旧平台证据（如 `netbsd-store.txt`）也读进来，导致 freebsd job 上出现与本轮无关的 “features.h NO-GO” 注解（本轮 `freebsd-store.txt` 实为 GO）——已改为只读 `${platform}-*.txt`。**A2 债务仍开放**。
- **GUI 桌面探测主题循环（2026-11-16 观察，开放）**：weekly schedule 的 `desktop` job 在 run
  `43855538365`（`c9a618c`）判红——`gui-probe.py` 的 `theme_cycle.ok` 对四套主题均为 false，
  `actual_themes` 出现 `null` 且后两帧截图像素全零（截图抓到重绘前的空帧），同 run 的
  `exit_code=0`、窗口/最小化/恢复/资源字段正常。重跑该 job 结果相同，非单次抖动。
  ADR-027 的复审触发已命中（“`theme_cycle.ok` 或资源预算失守即复审”）；本机 release 复现命令
  见 `docs/guides/project-status.md` 的 GUI 空闲线程条目。**不作 GUI 结论**：交互式主题切换的
  人工证据仍按 V-003 口径（Check 24／130）认定；托管 runner 的非交互会话截图时序不稳是
  待归因的候因，未改写预算或判据。
- **稳定性（NFR-204，已收口）**：T116/Check 116/195。5.5h CI 窗口对 0.5 MB/h 无分辨力（run 40529349945 无权判定）后按 ADR-025 升级 24h 本机深检——287 样本、多口径一致负斜率（warmup_ols −0.3551、phase_aligned −0.2681、稳态 −1.24）、线程/句柄无持续增长，**verdict=通过**（2026-11-02）；判据增补见 ADR-025 修订记录；证据 `artifacts/nfr204-verdict-24h-2026-11-02.md`。
- **真实 LLM 供应商（已接四端）**：T162—T167 完成。T162—T166 通过 77 项 `http` 测试与
  本地回环端到端（`artifacts/llm-e2e-2026-10-25.md`）；T167 于 2026-10-26 收口——CUI／Web／
  TUI 接线后，GUI 经 ADR-056（2026-11-03 用户批准“所有界面功能对等”）翻转，`llm.run`／
  `llm.cancel` 四端均登记 `Implemented`（`docs/matrices/feature-parity.md`）。
- **内置 TLS 证书库与 Web 证书设置面（已收口）**：T184—T192 与 Check 274—288 全部
  [exercised]。T184—T191 复核通过；Check 284 补齐部分写续传（400 KiB）、明文读取上限、
  TLS 第 97 条连接直接关闭与 HTTP 超限测试，并修复密文暂存跨 tick 清空的缺陷
  （2026-10-26，CI run `40273055639`）；T192 浏览器实机证据（Chrome 信任自签、页面指纹
  三方一致、设置页预览与重新生成、`restart_required`）见
  `artifacts/tls-browser-2026-10-26/tls-browser-evidence.md`。
- **GUI 线程预算**：DuckDB 2.0.0 单连接打开约 80 线程（`async_threads` 默认 64 所致），
  GUI 空闲首批实测 88 线程／297—330 句柄，超过 ADR-027 的 24／256。ADR-042 已于
  2026-10-26 批准方案 A 并实施（`SET async_threads = db.threads`，与 `db.threads` 同预算），
  同夹具复测 88→28 线程、句柄 233、RSS／私有内存与写入吞吐同时改善；**FR-712 线程判据
  仍未通过（28 > 24）**，A′（全局 `threads=1`，须另立修订并复测吞吐）与 P1（DuckDB 移出
  GUI 进程，须新 ADR）均未实施，预算不静默放宽。
- **GUI 空闲线程待查（2026-11-03 观察）**：本机 release 构建、DPI 2.0、客户区 1100×900 下
  空闲线程实测 **28**（CI 托管 runner 同条件实测 17），高于 ADR-027 的「线程 ≤ 24」。
  该数值与 ADR-042 方案 A 实施后的同夹具复测一致（28）；尚未定位残余 4 线程的具体来源
  （可能为高 DPI 渲染路径或本机 16 核的运行时线程池规模差异），**不作为结论**、也不据此
  改预算。复查方式：`python .github/scripts/gui-probe.py --exe target/release/whcode.exe
  --out artifacts/gui-run-<date>`，比较 `resources.threads` 与 `dpi_scale`／核数。
- **ADR 未决项**：无。ADR-058（turn 执行移出 serve 循环线程，WS 逐块增量转发）于
  2026-11-06 经用户「批准开发」转**已批准（附条件）**；条件 M1 复绿（`942dd14`）、
  M2 鉴权前置、M3 计数原子化、M4 busy 语义、M5 有损通道、M9 帧字段、M11 构造器收敛已
  落地并有 6 条测试；M7 用例 7—10 与三指标于 2026-11-10 归档
  （`artifacts/turn-ws-relay-2026-11-09/`，CI run `42680588379` 全平台 success）；
  用例 11 按 ADR-058 §3.6 登记缺口，不冒充通过。
  ADR-042 已批准（2026-10-26 方案 A），残余缺口是 FR-712 线程判据
  28 > 24（见上）。Check 134 于 2026-11-03 转勾——ADR-005、ADR-010、ADR-012 三份
  “部分批准”经证据收口转正（SSH 回环＋real-sshd 互操作、FreeBSD 源码构建 DuckDB 2.0.0、
  大载荷边界量化＋24h 深检），ADR-001—014 无未决条件；M1 基准校准仍按 ADR-012 原退出
  条件执行（属里程碑工作，非 ADR 未决项）。

## 能力快照

- 存储与事件：单文件 DuckDB（2.0.0 引擎，预编译动态库 + SHA256 校验）、16 表版本化 schema
  与可重复迁移、单写者有界队列、稳定游标分页、归档/压缩/导入导出/监控。
- 应用服务：命令统一经 `whcode-app`，错误信封与快照四端同源，三级事件背压。
- SDD 门禁：Constitution、spec/tasks 解析与纠偏、门禁强制与 force-advance 审计、双向追溯。
- LLM：Provider 语义、四级追踪与脱敏、传输层事件；真实 HTTP 客户端与 CUI／TUI／GUI／Web
  四端调用均已接通（T167＋ADR-056）。
- GUI：七视图（含对话）接真实数据、六套主题（amoled-night／eink-day／eink-night／
  terminal-green／whcode-day／whcode-night，whcode 系（原 zcode 系，2026-11-17 更名）
  2026-11-06 冻结，默认 `whcode-night`）、
  脏标记零重绘、最小化挂起与恢复；对话与写命令已接线（ADR-056 四端功能对等）；
  turn 后台执行（界面不冻结，Phase 2）。
- CUI：clap 命令树、`--json` 信封、稳定退出码；未接线命令显式失败。
- Web：编译期嵌入前端，默认仅回环且入站 TLS 默认启用（ADR-040，`--tls off` 可显式关闭）；
  证书设置页 `/settings/certs`。
- SSH：内置客户端（公钥认证、受限命令、超时与输出上限、known_hosts TOFU）；服务端受限入口。
- 运行时（默认关闭）：能力等级、payload 校验、检查点与执行缓存；JS/Python/Git Bash 已接入。

## 质量门禁快照

本地提交前至少跑通：`cargo fmt --all --check`、`cargo clippy --workspace --all-targets
-- -D warnings`、`cargo test --locked --workspace`，以及 `layering`、`layout` 两个契约测试与
`check_counts.py`／`traceability.py`／`check-eol.py`／`check-workflows.py`。
测试结论以 GitHub Actions 日志与 artifact 为准（ADR-037），本地运行只作复现与排障。

## CI 工作流

| 工作流 | 触发 | 职责 |
|---|---|---|
| `platform-verification.yml` | push main、PR、手动 | 门禁与测试、平台矩阵（Windows 必过＝唯一验收；FreeBSD 硬门禁证据通道；NetBSD 自动证据＋SSH 远程开发目标；OpenBSD/DragonFly SSH 远程开发目标）、短测量、Windows SSH 回环／TLS 专项／Playwright tablist 验收；性能与桌面型测试由 `workflow_dispatch` 输入触发 |
| `build-release.yml` | push、tag `v*`、手动 | debug 与 release 矩阵构建、打包安装验证、冒烟、SHA256；tag 时更新 GitHub Release |
| `long-run.yml` | 手动，或 push 改动 `artifacts/long-run-request-*.md` | 5.5 小时稳定性长跑（ADR-033，release 剖面、300 s 采样），一次约消耗 5.5 小时 Windows runner 配额；摘要以公开注解公布 |

平台范围与性能预算见 [`docs/matrices/platform-matrix.md`](../matrices/platform-matrix.md)；
已知限制见 [`docs/matrices/compatibility.md`](../matrices/compatibility.md)。
