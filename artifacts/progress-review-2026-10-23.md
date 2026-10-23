# 开发进度复核报告（2026-10-23）

**范围**：`specs/001-whcode-harness` 全部 148 条任务、里程碑 M0—M9 与 Phase 13、`main` 顶端提交。
**方法**：0 信任复核——本轮所有结论来自本机实测命令（AGENTS §3 标准验证链），文档陈述与磁盘
事实不一致时以可复现命令结果为准；未取得证据者一律标“未取得/阻塞”，不默认通过。
**环境**：Windows 11 amd64，`rustc 1.99.1`（`x86_64-pc-windows-msvc`，无 GNU 工具链），
DuckDB 运行期 `v1.5.6`，复核时提交 `c3a76e5`，本地时间 `2026-10-23T01:45+08:00`。

---

## 1. 总体判定

**工程实现推进至 M9 文档侧收口（任务 142/148，95.9%），但当前 `main` 顶端提交未通过门禁，
验收层（checklist.md）几乎未启动复核，对外文档已落后于实现。**

三条判定线的现状互不一致，必须分别陈述，不得以“任务已勾选”代替另两条：

| 判定线 | 现状 | 结论 |
|---|---|---|
| 任务清单（tasks.md） | 132 勾选 / 16 未勾选（其中 10 条经复核回退） | 复核前 142 条中有 10 条无实现或证据 |
| 门禁链（fmt/clippy/test/契约/布局） | **2 项失败**（clippy、layering） | **不通过** |
| 验收检查点（checklist.md） | 249 项全部有复核记录：**200 通过 / 49 未通过** | 复核完成 |

---

## 2. 门禁实测结果（本轮全部重跑）

| 命令 | 结果 | 判定 |
|---|---|---|
| `cargo build --locked --workspace` | `Finished` 7.64 s（增量） | 通过 |
| `cargo fmt --all --check` | 无差异，退出码 0 | 通过 |
| `cargo clippy --workspace --all-targets -- -D warnings` | **1 error**：`assertions_on_constants`，[main.rs](file:///d:/WHCode/crates/whcode-bin/src/main.rs) 测试内 `assert!(!cfg!(windows), …)` | **失败** |
| `cargo test --locked --workspace` | 47 个测试目标：**402 通过 / 0 失败 / 4 忽略**（4 个忽略均为显式登记的实验记录器或子进程入口，非藏匿失败） | 通过（见下注） |
| `cargo test -p whcode-tests --test layering` | **5 通过 / 1 失败**：`platform_cfg_is_confined_to_platform_crate` | **失败** |
| `cargo test -p whcode-tests --test layout` | 7 通过 | 通过 |
| `python script/normalize_eol.py` | 跟踪文件 294，CRLF 违规 0，BOM 违规 0 | 通过 |
| `python script/check_workflows.py` | 五平台 workflow 定义 OK | 通过 |
| `python ci/check_counts.py` | `FR=79 tasks=148 checks=249`，计数一致 | 通过 |
| `python ci/benchmarks/check-frontend-budget.py` | JS gzip 4 587 B / CSS gzip 946 B（预算 50 KB / 20 KB） | 通过 |
| `python script/measure_resources.py --test extreme_scale --test session_switch` | 峰值 RSS 175.2 MB / 69.7 MB（预算 512 MB），墙钟 388.3 s / 29.3 s | 通过（`artifacts/resource-report.json`） |

> 注：全量测试行写“通过”是指**跳过 1 个失败门禁用例**（`-- --skip platform_cfg_is_confined_to_platform_crate`）
> 后的结果。原始 `cargo test --locked --workspace` 因该用例失败而以退出码 101 中止，
> 该命令本身**不通过**。日志曾临时置于 `script/`，复核结束已删除。

### 2.1 阻断项：顶端提交 `c3a76e5` 未过门禁（最高优先级）

失败根因同源于“构建时间戳改为本地时区”这一未验证变更：

1. **NFR-101 平台判定越界**：[build.rs](file:///d:/WHCode/crates/whcode-bin/build.rs)
   新增 `#[cfg(target_os = "windows")]` / `#[cfg(not(target_os = "windows"))]`。契约测试把
   `build.rs` 计入 crate 源码（[test_layering.rs](file:///d:/WHCode/tests/contract/test_layering.rs#L80-L92)），
   门禁报错文本即命中 `cfg(target_os`。仓库既有先例已给出合规写法：
   [whcode-store/build.rs](file:///d:/WHCode/crates/whcode-store/build.rs#L9-L11) 明确“**不做平台判定**”，
   改以“`DUCKDB_LIB_DIR` 下是否存在 `duckdb.dll`”这类**载荷探测**自然分流。
2. **clippy 常数断言**：`main.rs` 测试中的 `assert!(!cfg!(windows), …)` 在 `-D warnings` 下为硬错误，
   且该断言在运行时不可能改变行为（对已编译目标恒真/恒假），本身也属于“伪验证”。

**流程违规（需在提交信息或记录中说明）**：该变更在门禁两项红的状态下进入 `main`。本轮复核开始时
工作树为“已修改未提交”，复核期间被外部自动提交工具固化（AGENTS 记录的 R-13 扰动，见
[decisions.md](file:///d:/WHCode/docs/matrices/decisions.md#L59)），复现条件为：任何一次 `main` 上的
`cargo clippy … -D warnings` 或 `cargo test --workspace`。

**修复方向（未实施，待用户确认取舍）**：
- 时间戳获取按“先试后回退”的顺序探测外部命令，不用 `cfg` 分支；两条路径均失败时显式回退 UTC 并保留 `Z`；
- 测试改为断言**输出格式**（长度、分隔符、偏移形状、年份区间），取消对编译目标的恒真断言；
- 若必须保留平台分支，则须先出 ADR 修订 NFR-101 与门禁白名单——按 §8 禁止事项，不得改门禁迁就实现。

---

## 3. 任务与里程碑进度（权威口径：tasks.md）

148 条任务中 142 条勾选，未勾选 6 条，逐条理由如下（全部为环境/前置阻塞，非漏做）：

| 任务 | 里程碑 | 状态 | 依据 |
|---|---|---|---|
| T096—T099 | M7 BSD SSH 服务端 | **延后** | ADR-013 把验收目标收敛为 Windows 11 amd64；恢复须新 ADR 并补 D-14 证据链 |
| T115 | M9 热点优化 | **阻塞** | D-06 性能预算未冻结（缺分平台基线与 72 h 时间序列）；ADR-020 已冻结分配器决策 |
| T116 | M9 稳定性 | **阻塞** | 需 24/72 h 连续运行环境；NFR-204 长时泄漏斜率未测量 |

分里程碑：

| 里程碑 | 范围 | 状态 |
|---|---|---|
| M0 平台可行性与基线 | T020—T031 | Windows 工程基线**有条件通过**（ADR-016）；V-004 延后 M7、V-006 预算不冻结 |
| M1 存储与事件基座 | T032—T045 | 实现完成；测试与实测证据在库（写者、游标分页、预聚合、purge/compact/导出） |
| M2 会话与任务应用服务 | T046—T055 | 实现完成（含 10 万事件/1 万任务夹具、极限规模测试） |
| M3 LLM 与上下文 | T056—T067、T132 | 实现完成（**桩 Provider**，真实 HTTP Provider 未接线） |
| M4 SDD 门禁状态机 | T068—T081 | 实现完成（V-201—V-207 集成判定） |
| M5 TUI 与 CUI | T082—T089、T130/T131 | 实现完成，批次 1—3 前端数据/命令/文本层接线落地 |
| M6 Web | T090—T095 | 实现完成，批次 4 收口为“仅局域网 + 强制认证”（ADR-023） |
| M7 BSD SSH | T096—T099 | 延后（不作验收结论） |
| M8 可选执行运行时 | T100—T114 | 接口/门控/检查点/缓存交付；JS/Python/GitBash 引擎与 worker 未接入（T110—T112、T114 回退） |
| M9 优化与发布 | T115—T121 | 兼容矩阵与运行时清单已交付；优化与长时稳定性阻塞，发布包与三份指南缺失（T118/T119 回退） |
| Phase 13 收尾 | T122—T127、T134、T138—T148 | 部分完成（T123/T124/T127 回退） |

发布状态：标签 `v0.1.0-m0`、`v0.1.0-m9`（预发布标识，非正式版本）；展示版本已改为
**二进制构建时间**，Cargo 包版本仍为 `0.1.0`。按
[compatibility.md](file:///d:/WHCode/docs/matrices/compatibility.md) 的发布判据，
预算冻结与长时稳定性未达成前不发正式版本。

---

## 4. 验收层结果（checklist.md：249 项全部有复核记录，200 通过 / 49 未通过）

**批次一（Check 1—31，Phase 1—3 / M0）**：25 项通过，5 项未通过——Check 3（CI 未接线）、
Check 20/21/22（分平台口径缺 FreeBSD/NetBSD 证据）、Check 25（SSH 四平台记录未取得）。

**批次二（Check 32—89，M1—M6）**：53 项通过（逐条引用具名用例或实跑命令输出），
2 项未通过——Check 47/55 要求“符合**冻结**预算”，而 D-06 未冻结（V-006）。
Check 90/93 维持“部分证据”，不勾选。

**批次三（Check 96—127 + 225，M7—M9 与 Phase 13）**：14 项通过，19 项未通过——
M7 四项延后（ADR-013）；M8 五项能力未接入（101 无 UI 展示、102 来源未校验、
104 取消/资源限制缺失、109 无残留进程断言、110/111/112 引擎与 worker 未接入、114 缺 crash 测试）；
M9 四项（115/116 阻塞、118 无发布包、119 缺三份指南）；Phase 13 三项（123 无复查记录、
124 未跑 quickstart、127 无断链审计）。

**批次四（Check 128—249，需求覆盖 / 边界与异常 / 回归）**：108 项首轮 + 9 项补遗——
**89 项通过、28 项未通过**。主要缺口：
① 三时点崩溃注入与“副作用已发生未登记”恢复**已补齐**（139/158/196/207 于 2026-10-23 补录通过，
见 §9 续二）；
② M8 引擎/worker 未接入（153/154/155）；
③ Web 写命令、WS 广播、慢客户端对比与“无 JS 只读视图”未验证（143/183/199/209/229）；
④ 预算未冻结导致的预算类判据（133/136/195）；
⑤ 并行调度、检查点触发策略、PO 加载器、子进程凭据剥离、零残留进程等能力缺口（172/171/224/144/201）；
⑥ ADR 全部批准不可达（134，005/010/012 为部分批准）；
⑦ 无 PR 流程导致的过程类判据（184/217）；
⑧ `ci/check_counts.py` 未接入真实 CI（249）。

**由此产生的任务回退（10 条）**：T093、T110、T111、T112、T114、T118、T119、T123、T124、T127
由 `[x]` 回退为 `[ ]` 并逐条写明原因——复核前它们被标记完成，但对应实现或证据不存在。
另有两条保留完成但补注记：T094（前端落点是 `app.js`，任务写的 `src/events.ts` 不存在）、
T109（Windows 子进程树已实现；BSD 进程组分支未实现，显式返回错误）。

`checklist.md` 的勾选权归复核人，生成时一律 `[ ]`；现状 **249 项全部有复核记录**：
200 通过（14 项为 2026-10-21/22 的用户复核，177 项为本轮，4 项为补录，5 项为收敛表修复后补录）、
49 未通过。这意味着：

- G4 验收门禁**仍未成立**：49 项未通过即意味着对应里程碑不能宣称验收通过；
- `check_counts.py` 只校验“FR/任务/检查点数量一致”，不校验勾选比例，故该缺口不会被现有门禁抓到。

复核前最大的结构性风险是进度指标（95.9%）与验收指标（5.6%）背离；本轮把 249 项判据
逐条落笔、回退 10 条虚高任务。此后任何对外“完成度”表述若只引用 tasks.md 而回避
“200/249 通过”，即构成 AGENTS §1.4“不伪造”与 §5“禁止跨门禁跳跃”的反面案例。

---

## 5. 文档一致性问题（以磁盘事实为准）

| 位置 | 陈述 | 事实 | 处置 |
|---|---|---|---|
| [compatibility.md](file:///d:/WHCode/docs/matrices/compatibility.md) §2.5 | “TUI 六视图 / Web UI 未交付（T082/T090—T095）” | T082/T083、T090—T095 已勾选，批次 1—4 于 2026-10-22/23 交付并有浏览器与真实控制台取证 | 已过时，须更新（含 §1 表格“M0—M9 全部测试在本机通过”需加“除本轮 2 项门禁失败外”限定） |
| [README.md](file:///d:/WHCode/README.md) “当前状态/核心能力” | 仅述 M0 与“最小 TUI 闭环”，链接目录写“ADR-001—017” | 已交付 CUI 命令树、SDD 门禁、Web（仅局域网）、运行时；ADR 实至 023 + DB 构建决策 | 对外表述落后两个里程碑批次，须同步 |
| tasks.md T121 | 落点写 `docs/risk-register.md` | 该文件不存在；风险台账实际在 `docs/matrices/decisions.md`（R-12 未解除、R-13 新增） | 属任务描述与落点不一致，须在任务行注明路径变更（不得依赖隐含默认） |

其余抽查项（AGENTS.md、CHANGELOG、spec/plan/data-model/contracts、六份契约、ADR 六要素、
`artifacts/` 27 个证据条目 + 7 个取证子目录）与实现记录相互一致，未发现“伪造证据”迹象。

---

## 6. 既有诚实缺口（登记在册，不得声称已交付）

1. 性能预算未冻结（D-06）：事件写入 6 921 条/秒 @192 MB、概览 p95 7.6 ms、TUI 帧 p95 448 µs
   均为本机单平台观测，不作承诺；
2. 24/72 h 长时稳定性与内存泄漏斜率未测量（T116）；
3. 真实 LLM Provider 未接线（M3 桩交付），取消语义为“下个 chunk 前生效”；
4. M7 BSD 服务端与 V-004 节点验收延后；FreeBSD/NetBSD 仅有“代码级交叉类型检查”证据，
   不构成运行验证（ADR-010、`artifacts/cross-check-windows.json` 的 `is_runtime_evidence: false`）；
5. 单令牌、无 TLS、无公网暴露与多用户隔离（ADR-023 退出条件）；
6. X4 删除吞吐（1 M 行约 125 s）已按 NFR-216 以实测把标准放宽至 ≤ 300 s，批量/分区删除列为
   预算冻结后的优化候选。

---

## 7. 建议下一步（按优先级）

1. **修复 `c3a76e5` 引入的两处门禁失败**（去 `cfg` 分支 + 改格式断言），使
   `clippy -D warnings` 与 `layering` 复绿；提交信息须说明“门禁在红灯状态下曾进入 main”的原因与复核证据。
2. **启动 G4 验收复核**：按 tasks.md 已完成批次顺序勾选 checklist 第一层（Check 1—127），
   每项附可判定证据出处；无法取证者保持 `[ ]` 并标“未取得/阻塞”。
3. **同步对外文档**：README 当前状态/核心能力/ADR 清单，compatibility.md 已知限制第 5 条，
   T121 落点注明（或在 decisions.md 建立唯一台账并在任务行更正路径）。
4. **为自动提交扰动设处置**：R-13 已登记但未关闭；在门禁链未纳入 pre-commit 前，
   建议约定“提交后必须重跑 §3 前五项并回报”，否则红提交会反复进入 `main`。
5. 预算冻结评审（T115 前置）与 24 h 长跑窗口安排（T116），二者不解除则 M9 无法从“文档完成”转为“验收完成”。

---

## 8. 复现命令

```powershell
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
python ci/fetch-duckdb.py
cargo build --locked --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings          # 本轮失败
cargo test --locked --workspace                                # 本轮 101 中止
cargo test --locked --workspace -- --skip platform_cfg_is_confined_to_platform_crate
cargo test -p whcode-tests --test layering                     # 本轮 1 失败
cargo test -p whcode-tests --test layout
python script/normalize_eol.py
python script/check_workflows.py
python ci/check_counts.py
python script/measure_resources.py --test extreme_scale --test session_switch
python ci/benchmarks/check-frontend-budget.py
```

---

## 9. 复核后处置（2026-10-23 01:5x，用户要求先修 cfg 与断言）

**已修复并提交 `86d93ad`**（`fix(version): 去除构建脚本的平台条件编译并改正版本戳断言`）：

1. `crates/whcode-bin/build.rs` 删除平台条件编译，改按**可用性**依次探测外部时间命令
   （`powershell` → `date`），两者均不可用时回退 UTC 并输出 `cargo:warning`（不静默降级）；
   与 `whcode-store/build.rs` 既有纪律一致。
2. `crates/whcode-bin/src/main.rs` 的版本戳用例删除 `cfg!(windows)` 平台分支，改为只校验
   格式（长度 17 或 22、分隔符、偏移形状）与年份合理性。
3. `CHANGELOG.md` 相应条目同步为“不做平台判定 + 回退告警”。

修复后实测：`fmt` 零差异、`clippy -D warnings` 零告警、`cargo test --locked --workspace`
**退出码 0**（47 个目标全通过，`layering` 6/6）、`layout` 7/7、EOL 294 文件零违规、
`ci/check_counts.py` 一致、`whcode --version` 与 `doctor` 实测 `2026-10-23T01:55+08:00`。
因此 §1 的“门禁链不通过”一项在本提交后转为**通过**；§4（checklist 14/249）与 §5（文档滞后）
**仍未处置**。

**本轮新发现（已处置）**：`test_layering.rs` 的平台判定 marker 表只含
`cfg(windows)`/`cfg(unix)` 等字面量，**不匹配 `cfg!(windows)`**；而
[worker.rs](file:///d:/WHCode/crates/whcode-runtime/src/worker.rs#L111) 测试代码正使用该形态
在 `whcode-platform` 之外做平台分流。处置：`whcode-platform` 新增 `sleep_command`
（Windows `ping` / POSIX `sleep`）并导出，`whcode-runtime` 改经它构造超时用例命令；
marker 表补 `cfg!(windows`/`cfg!(unix` 两种宏形态，`layering` 复绿（6/6）。

**文档与证据处置**：README 当前状态（里程碑表、验收口径、门禁实测）与核心能力重写，
ADR 清单更新至 023；`compatibility.md` 已知限制第 4/5 条改写（M7 只交付客户端、
实现完成 ≠ 验收通过）；T121 注明风险台账并入 `decisions.md`；本报告经 `.gitignore`
放行后随仓库版本化（README 引用它作为复核依据）。

**复核进度（2026-10-23 续）**：checklist 四个批次（Check 1—249）已逐条取证落笔，
**249 项全部有复核记录：200 通过 / 49 未通过**；并据实回退 10 条任务
（T093/T110/T111/T112/T114/T118/T119/T123/T124/T127）。
复核同时暴露并处置四处缺陷——① 仓库无 `.github/workflows/`，`ci/platform-matrix/*.yml` 从未被平台执行；
② `decisions.md` 的 D-07—D-11 状态停留在“待实现”；③ `feature-parity.md` 的 CUI 列仍标“未交付”；
④ `docs/guides/README.md` 的指南索引严重滞后（只列出一份，且把已交付的 CUI 手册仍写作“计划”）。
四项均已同步更正，并在对应文件中写明复核日期与依据。

**复核进度（2026-10-23 续二）：补齐三时点崩溃注入**——按优先级补交付
Check 139/158/196/207（V-105/V-307、FR-410、T114 部分）：

- `whcode-store::tool_exec` 新增“意图—执行—登记”原语（`record_intent` → `mark_running` →
  `record_result`，状态迁移受限）与启动期恢复 `recover_uncertain`（prepared/running →
  `unknown`，单事务、`LIMIT MAX_RECOVERED + 1` 判截断、可重复执行）；
- `open_workspace_db` 在迁移后调用恢复，非空结果输出告警（数量、前 5 个 ID、下一步动作）；
- `tests/crash/test_side_effect_recovery.rs`：父测试以 `--exact --ignored` 自启子进程，
  在**步骤边界 / 意图后 / 执行中（副作用已落盘） / 登记后**四个注入点 `abort()`，
  父进程重开库断言恢复状态；“副作用已发生、结果未登记”用例以标记文件行数恒为 1 证实不重放。

证据：`cargo test -p whcode-tests --test side_effect_recovery` 4 通过 / 1 ignored；
`cargo test --locked --workspace` 48 目标 / 412 通过 / 0 失败 / 5 忽略；
`fmt --check` 与 `clippy -D warnings` 零差异零告警。T114 仍为未完成（V-304 无用例，Check 114 未通过）。

**复核给出的下一步优先级（按缺口影响）**：M8 引擎/worker 接入（153/154/155）或正式降级声明 →
Web 写命令与 WS 广播（143/183/199/209）→ 预算冻结（133/136/195）→ 发布包与三份指南（118/119）
→ CI 接线（3/249）。

**附带教训（已体现在本报告）**：门禁按**源码文本**扫描，注释中书写该字面量同样命中；
首次修复即因注释措辞被门禁再次拒绝，改述后复绿。

---

## 10. 第二、三层复核续（2026-10-23 第二轮，Check 128—249）

**方法**：不重读旧注记，直接以源码与目录事实复验该区间 26 项未通过的判据；同时对区间内
通过记录所引用的具名证据做引用核对（防止“引用不存在的用例”这类伪造）。

**引用核对结果**：该区间共引用 53 个具名证据，49 个在 `crates/**`、`tests/**` 的源码中命中；
其余 4 个为契约测试文件名（`test_tui_cui_parity`、`test_web_security_defaults`，文件实际存在，
用例数分别为 4、5）与 `script/tui_smoke.py` 的检查项名（`graceful_exit_on_closed_stdin`，
脚本第 106 行有该键，产物 `artifacts/tui-smoke/20261022-185913/summary.json` 存在）。
**未发现虚构引用**。

**改判情况**：无。26 项未通过的理由经复验全部成立（当时未新增通过、未新增未通过，
总数维持 195 / 54）。后续按 §10 排序修复了 3 项，现行计数见 §11。

**本轮复验所依据的直接事实**：

- `crates/whcode-runtime/src/` 仅 9 个文件，**无** `js.rs`／`python.rs`／`gitbash.rs`；
- `payload.rs` 的 `PayloadMeta.source` 只记录、`validate` 不判来源；
- `checkpoint.rs` 只有 `Checkpoint` 结构与四要素 `validate`，无触发方式与保留数配置；
- `provider.rs`／`worker.rs` **无** `env_clear`／`env_remove`（子进程继承父环境）；
- `whcode-tui`／`whcode-web` 对 `Capability` 零命中（三档能力等级无 UI 展示）；
- `whcode-web/src/ws.rs` 只有握手与帧解析，**无订阅/广播**；
- `tests/stability/` 只有 `baseline_check.rs`（计数/分位数、幂等键 2 例），无四类故障情形；
- `.github/workflows` **不存在**；ADR-005/010/012 状态为“部分批准”；
- TUI Diff 视图数据仅来自 `tests_support.rs` 夹具，`StoreViewSource` 未供数。

**缺口收敛表（26 项 → 14 类）**：

| # | 根因 | Check | 闭环最小动作 | 门禁落点 |
|---|---|---|---|---|
| 1 | 预算未冻结（D-06／V-006） | 133、136、195、243 | 取得跨平台基线与 72 h 时间序列 → 冻结或 ADR 修订 | 预算状态位 + 新增预算冻结校验脚本 |
| 2 | M8 引擎／provider 未接入 | 153、154、155 | 实现 `js.rs`（固定 ECMAScript 测试集）、`python.rs`（版本/ABI/路径/依赖/C 扩展）、`gitbash.rs`（payload 哈希 + 五类命令）；或出正式降级 ADR | `cargo test -p whcode-runtime` |
| 3 | Web 写命令与 WS 广播缺失 | 183、199、209 | 会话/任务写 API + `/ws` 订阅广播 + 慢客户端对比用例 | `test_web_security_defaults`、新增广播用例 |
| 4 | TUI Diff 视图无数据源 | 179 | converge 差异接入 `StoreViewSource` | TUI 快照用例 |
| 5 | 子进程未剥离凭据环境 | 144 | 构造子进程前 `env_clear` + 白名单注入，并断言凭据变量不可见 | `whcode-runtime` 用例 |
| 6 | 检查点无触发策略 | 171 | 定时／按步骤／危险操作前三类触发 + 保留数配置项 | `whcode-runtime` 用例 |
| 7 | 零残留进程无自动化验证 | 201 | 终止后枚举子进程/句柄断言为空（Windows Job Object 优先） | `tests/stability` 或平台用例 |
| 8 | 稳定性四情形未纳入 | 206 | LLM 失败／断网／磁盘不足／写入失败四类注入用例 | `tests/stability` |
| 9 | 卸载内存下降未实测 | 221 | 卸载前后 RSS 采样对比 | `script/measure_resources.py` 扩展 |
| 10 | gettext PO 加载器未实现 | 224 | PO 解析 + 资源目录结构 + zh-CN→en 回退链契约测试 | 契约测试 |
| 11 | 浏览器运行时证据缺口 | 220、227、228、229 | 桌面浏览器验证 `visibilityState=hidden`、20 次最小化-还原、`prefers-reduced-motion`、无 JS 只读视图 | 人工留证 + 预算门禁 |
| 12 | 单人流程／CI 未接线 | 184、217、249 | 建立 PR 或等价记录（矩阵 + §15 七项清单）；落地 `.github/workflows/` 并跑 `check_counts.py` | CI |
| 13 | ADR 部分批准 | 134 | 解除 ADR-005/010/012 未决条件（依赖第 1、2 类） | ADR 状态位 |
| 14 | M7 SSH 服务端延后 | 182 | 按 ADR-013 保持延后；恢复须新 ADR | — |

**排序建议**：闭环成本低且可自动化门禁的前置项（第 5、6、7、8、9、10 类）先做；
第 2 类（M8）无论实现还是降级都需要 ADR，宜尽早定性；第 1 类锁死发布口径，
在第 2 类定性后再启动冻结，避免预算随实现变动反复冻结。

---

## 11. 收敛表修复进展（2026-10-23）

按 §10 排序执行第 5、6、7、10 类；计数由 195 / 54 变为 **198 / 51**。

| 类别 | Check | 交付内容 | 证据 |
|---|---|---|---|
| 5 凭据隔离 | 144 → **通过** | `whcode-runtime::worker::strip_credentials`：父进程继承与调用方显式设置的凭据型变量一并剥离（`KEY`/`TOKEN`/`SECRET`/`PASSWORD`/`CREDENTIAL`/`COOKIE`/`SIGNATURE`、`SSH_AUTH_SOCK`、`GPG_AGENT_INFO`、`WHCODE_*`）；`worker::execute` 与 python／git 探测统一调用 | `credential_variable_detection_covers_common_channels`、`strip_credentials_removes_credentials_from_child`（子进程探针 + 阳性对照：未剥离时退出码 7，剥离后 0，且 stdout 证明探针确已执行） |
| 6 检查点策略 | 171 → **通过** | `whcode-domain::checkpoint::CheckpointPolicy`（定时/按步骤/危险操作前 + 保留数，越界即拒）+ `retain_recent` 有界裁剪；`Config.checkpoint` 参与 JSON 配置并在加载期校验；运行时重导出同一份定义（不新增依赖边） | `policy_triggers_on_timer_steps_and_dangerous_actions`、`policy_rejects_out_of_range_values`、`retention_keeps_recent_items_and_reports_drops`、`policy_roundtrips_through_json_with_defaults`、`checkpoint_policy_is_configurable_and_validated_at_load` |
| 10 PO 接口 | 224 → **通过** | `parse_po`（续行/转义/`msgctxt`，跳过 fuzzy／复数／废弃）、`Catalog::from_po`、`I18n::load_with_po`（`<root>/i18n/po/<locale>.po`，256 KiB 上限、语言标识白名单禁穿越）；`whcode-bin::load_texts` 接线（`WHCODE_I18N_DIR` → 可执行文件目录，失败告警回退） | 6 个用例：解析边界、PO 覆盖 + en 回退 + 键名兜底、无 PO 等价内建、非法语言标识、超限拒绝、目录结构（BOM/LF）+ 内建目录 PO 往返无损 |
| 7 零残留（Windows） | 201 → **部分通过**（维持未勾选） | `whcode-platform::process_exists`／`child_pids`；`tests/stability/test_process_cleanup.rs` 启动真实 `cmd → ping` 树，`terminate_tree` 后断言父与子 PID 均消失，并先断言确实观测到子进程 | 用例通过（0.82 s）；**BSD 进程组仍无实现与平台证据（ADR-013）**，故整条判据未满足 |

**当时未执行**：第 8 类（Check 206）与第 9 类（Check 221）随后在 §12 补齐；
第 1—4、11—14 类按 §10 排序在其后。

**新增/修改文件**：`crates/whcode-domain/src/checkpoint.rs`（新）、`crates/whcode-domain/src/i18n.rs`、
`crates/whcode-runtime/src/{worker.rs,providers.rs,checkpoint.rs,lib.rs}`、
`crates/whcode-platform/src/{process.rs,lib.rs}`、`crates/whcode-bin/src/{main.rs,cui_dispatch.rs,config.rs}`、
`tests/stability/test_process_cleanup.rs`（新）、`tests/Cargo.toml`、`tests/stability/README.md`。

**门禁**：`cargo test --locked --workspace` **49 个目标 / 429 通过 / 0 失败 / 6 忽略**（含
`layering`、`layout`、`process_cleanup`）；`cargo fmt --all --check` 零差异；
`clippy --workspace --all-targets -- -D warnings` 零告警；`normalize_eol.py`（297 文件，
CRLF/BOM 均 0）与 `ci/check_counts.py`（FR=79/tasks=148/checks=249）通过。

---

## 12. 收敛表第 8、9 类修复（2026-10-23 续）

| 类别 | Check | 交付内容 | 证据 |
|---|---|---|---|
| 9 卸载内存 | 221 → **通过** | `whcode-app::residency` 新增有界驻留缓冲（200 条 / 256 KiB 双上限，超限 `backpressure`）、`accounted_bytes`、`event_cursor`；卸载释放缓冲、唤醒从空开始、已卸载会话再收事件显式 `invalid_state`；`whcode-platform` 新增 `process_rss_bytes`／`current_rss_bytes`（Windows `tasklist` 工作集，非 Windows 结构化“延后”错误） | 单元：`unload_releases_buffer_memory_and_wake_starts_empty`、`buffering_is_bounded_by_count_and_bytes_with_backpressure`、`buffering_into_unloaded_session_is_rejected`、`unload_then_wake_replays_without_duplicates_or_gaps`（缓冲 1—4 + 补拉 5—10 = 全集且无重复）；实测：`residency_memory` 装载 128 会话 × 240 KiB 后卸载，accounted 31 469 350 B，**RSS 37 756 928 → 6 275 072 B（下降 31 481 856 B ≈ 30.0 MiB）**，唤醒不预分配 |
| 8 稳定性故障 | 206 → **通过** | `tests/stability/test_failure_modes.rs`：① LLM 失败（失败 Provider → 结构化错误 + `details` 原文 + 按错误码的重试语义）；② 断网（临时端口连接失败有界可归类 + 恢复后补拉无重复无缺失）；③ 写入失败（只读库写入 → `io_error` 且零半完成状态）；④ 磁盘不足（`WhError::io` 分类 `StorageFull`／Windows 112，消息含“清理空间后可重试”；真实满盘写入由 `WHCODE_DISK_FULL_DIR` 门控，本机未执行并显式打印） | 4 个用例全部通过（2.05 s）；域层分类用例 `io_error_classifies_disk_full_and_keeps_other_errors_verbatim` |

**边界与代价**：驻留缓冲以“超限背压”换取有界内存——缓冲区满时调用方需补拉或延后；
这符合 FR-614“禁止会话级预分配固定大缓冲”，但把压力转移给了调用方，需在 UI 层把
`backpressure` 呈现为可操作提示（当前仅结构化错误，UI 接线属 M8）。真实满盘与 24/72 h
长跑仍未取得，T116 保持未完成。

**计数**：200 通过 / 49 未通过；第二、三层未通过 21 项，归并为 9 类（§10 表中第 1—4、
7、11—14 类）。

**过程中发现并修复的测试隔离缺陷**：`whcode-store` 多个测试辅助函数用
`进程 ID + tag` 命名临时库；进程崩溃后残留的库文件在 PID 复用时被读到，导致
`Deduplicated` 之类的假失败（本轮全量跑中出现 3 例）。改为 `crates/whcode-store/src/test_util.rs`：
进程内单例根目录（PID + 纳秒戳，每进程唯一），同一 `tag` 仍映射同一路径。
教训：测试隔离的“唯一键”必须同时覆盖“跨运行”，不能只覆盖“跨用例”。

**门禁**：`cargo test --locked --workspace` **51 个目标 / 440 通过 / 0 失败 / 6 忽略**；
`cargo fmt --all --check` 零差异；`clippy --workspace --all-targets -- -D warnings` 零告警；
`normalize_eol.py`（299 文件，CRLF/BOM 均 0）与 `ci/check_counts.py`（FR=79/tasks=148/checks=249）
通过。另：全量跑前需清理锁住 `target` 下测试二进制的残留进程（LNK1104），本轮已用
`script/` 临时脚本清理并删除。

## 13. 里程碑批次：ADR-024／ADR-025／追溯门禁／Web 写命令（2026-10-23）

**范围**：用户批准“门禁链 + 追溯门禁双绿、M8 与预算两项定性完成、Web 写命令落地”。

**定性（ADR）**：

| ADR | 结论 | 影响 |
|---|---|---|
| ADR-024 运行时范围 | JS 不交付引擎（`Restricted` + 结构化 `runtime_unavailable`）；Python 子进程 worker（记录绝对路径/版本/实现名/ABI/依赖）；Git Bash 独立 payload（四重校验）；不引入 Boa／RustPython | Check 110/111/154 的口径确定但仍未实现（不勾选）；153 维持未通过 |
| ADR-025 预算冻结 | Windows 11 amd64 冻结 NFR-201—210，冻结值＝spec 候选值；NFR-208/210 达标 | Check 55/133/136 转入通过；195 仍未通过（八项无实测、72 h 缺失） |

**代码（Check 101/102/104）**：

- `payload::SourceAllowlist` + 固定顺序四重校验（来源不明 → `permission_denied` 不可重试）；
- `bin::runtimes` 清单四处可见（doctor JSON/人类输出、TUI 摘要、Web `/api/runtimes`）；
- `worker::execute_with_cancel`（协作取消）+ `ConcurrencyGate`（并发上限，RAII 归还）。

**Web 写命令（Check 93/183）**：会话 4 条、任务 2 条、清理预览/确认各 1 条；破坏性命令
默认预览；数据形状集中到 `whcode-app::views`，CUI 与 Web 共用；清理计数补齐三个漏报表名。
前端新增“维护”页签与按钮，JS gzip 6 769 B（预算 50 KB）。

**追溯门禁（Check 127）**：`ci/traceability.py` 六条判据；FR-402/403/404（检查点增量差分、
同事务提交、恢复加载引用）以“缺口登记 G-01”承载，登记须含责任人与退出条件，恢复覆盖后
自动判为“登记失效”。

**计数**：200 → **209 通过 / 40 未通过**（新增通过 55、93、101、102、104、127、133、136、183）；
未进行任何“按实测反向冻结”或伪造覆盖的操作。

**未取得**：真实浏览器逐项点按新按钮的记录（API 层自动化已齐）；V-304（Git Bash 五类命令）；
Python ABI/C 扩展探测记录；72 h 时间序列。

---

## §14 交付批次（2026-10-23）：CI 接线、发布包、指南与 quickstart

**结论**：Check 118、119 转入通过；209 → **211 通过 / 38 未通过**；Check 3／124／249 因此前
口径未变（缺平台执行记录、缺交互与长跑证据）维持未通过，未做“接线即通过”的替换。

### 14.1 CI 接线（Check 3／5／249）

**过去来源**：五份 workflow 一直放在 `ci/platform-matrix/`，而 GitHub Actions 只执行
`.github/workflows/`，因此从未被平台执行（批次一已登记该缺陷）。

**本轮动作**：五份迁入 `.github/workflows/platform-*.yml`；`ci/platform-matrix/` 只留依赖记录与
交叉检查脚本；Windows 流水线补齐计数、三方追溯、前端预算、打包安装四个步骤；自检脚本
`script/check_workflows.py` 改扫新目录并新增“必须在 `.github/workflows/` 下”的位置判据。

**正负和解**：正面是流水线终于可被执行、配置单一来源；负面是路径变更波及 7 处文档与
Check 5 的取证口径（已逐处同步，非静默改名）；承担者是后续维护者。

**未取得**：本仓库无远程、无 runner，平台执行记录仍为空。Check 3／249 按此维持未通过，
不以本机 `fmt/clippy/test` 通过冒充 CI 证据。

### 14.2 发布包与安装验证（Check 118）

新增 `ci/package-windows.py`：白名单打包（exe + `duckdb.dll` + README/LICENSE/CHANGELOG +
兼容矩阵 + PACKAGE-INFO），并在**仓库外**临时目录解包、切换 cwd 后运行三条命令。
证据 `artifacts/install-verify-windows-amd64.txt`：

| 命令 | 退出码 | 可归因信号 |
|---|---|---|
| `whcode.exe --version` | 0 | `whcode 2026-10-23T12:56+08:00` |
| `whcode.exe doctor --json` | 0 | `duckdb_engine_version = v1.5.6`（运行期实测）、三档运行时等级 |
| `whcode.exe --workspace <tmp> db stats --json` | 0 | 临时工作区实际生成 1 060 864 字节库文件 |

包产物 `target/dist/whcode-0.1.0-windows-amd64.zip`（14 652 047 字节）不入库，可复现：
`cargo build --release --locked -p whcode-bin` + `python ci/package-windows.py --verify`。

### 14.3 三份指南（Check 119）

`docs/guides/tui.md`、`web-deployment-security.md`、`disaster-recovery.md`。写法约束：每条结论
附 `文件:行号`；**未实现项单列**（TUI 无命令面板/翻页/帮助/鼠标/粘贴、Web 无 TLS/无安全响应头/
无 Host 校验/无 CSRF token/无限速、无 restore 子命令、WAL 自动检查点未接入循环、无启动期
完整性校验）。同时纠正一处文档与实现冲突：`db compact` 不承诺缩小文件。

### 14.4 quickstart 全流程留证（Check 124，维持未通过）

`artifacts/quickstart-run-2026-10-23.md`：30 条目 = 成功取证 20、预期失败 5、未取得 5、
缺陷登记 1、异常 0。命令全部在仓库外临时工作区执行。

**发现的文档漂移（已按实测修正 `quickstart.md`）**：`status`／`task list`／`spec status|validate`
的 `--session` 为必填；`db compact` 没有 `--export`（备份走 `db archive --export`）；
`db purge --before` 取 Unix 毫秒；`db compact` 参数冲突退出码 2。

**新登记缺陷（待修）**：`whcode events export --session S-NOT-EXIST` 返回 `count=0` 且退出码 0，
与“会话存在但无事件”不可区分（错误的空，违反信号链完备原则）。退出条件：不存在的会话返回
`not_found`，并新增“不存在会话 vs 空会话”对照用例；修好后重跑本节留证。

**未取得**：交互式 TUI（V-003）、真实 SSH 节点（V-004）、72 小时长跑——三项无替代证据，
Check 124 不得判通过。

### 14.5 复审触发

1. 仓库接入远程 runner 后，重跑 Check 3／249 并只以平台执行记录为准；
2. 发布包构成变化（新增 payload 或运行时）时必须重跑 `ci/package-windows.py --verify`；
3. 上述缺陷修复后，Check 124 的结论需重新评估，而不是沿用本节。

## §15 V-003 交互式 TUI 取证与缺字缺陷修复（2026-10-23）

**结论**：V-003 由真实控制台驱动取得，10 项自动检查全部通过；同轮发现并修复一处**真实渲染
缺陷**（歧义宽度字符整字被擦除）。Check 24 补充复核，Check 124 仍因缺 SSH 节点与 72 h 长跑
维持未通过，不因 TUI 取证而改判。

### 15.1 缺陷与证据链（先证伪，再归因）

| 层 | 假设 | 检验 | 结论 |
|---|---|---|---|
| 读屏器 | 读回丢字 | `--probe` 让 cmd 直接输出同一串中文并逐码位打印 | 读回完整，排除 |
| 词条 | i18n 缺词条 | 起 `serve --web` 读 `/api/i18n` 的 `entries` | 词条完整，排除 |
| 终端 | 宽度语义 | 分层写屏对照 + 网格读回 + 逐字光标前进量 | `·`、`×`、`—`、`“”` 占 **2 格**，制表符/块元素占 1 格 |
| 后端 | 逐格定位覆盖尾格 | 像素截图（GDI `BitBlt`）确认缺字真实存在 | 成立 |

缺陷现象（修复前帧 `artifacts/tui-v003-2026-10-23/before-fix/s1-01-startup.txt`）：标题
“会话”显示成“话”、页脚“按键 0”显示成“键 0”、“视图”显示成“图”。

### 15.2 修复：整行输出 + 运行时实测宽度

1. `CrosstermBackend` 每格一次 `MoveTo`，列号按 `unicode-width`（歧义字符 1 格）→ 写下一格
   时覆盖该字尾格，conhost 擦掉整字。新增 `crates/whcode-tui/src/rowbackend.rs`：
   **每行只发一次绝对定位，行内连续输出**，宽度由终端自行推进。
2. 宽度来源不用静态表：Unicode East Asian Width 把制表符也记为 Ambiguous，而本机实测为 1 格
   （`width-probe.txt`）。改为**运行时实测**——首次出现的字形写到末行同一行连续列位、读光标
   列差，结果按字形缓存（上限 1024）。
3. 溢出行尾部压缩：保留最右格并锚定到最后一列，被挤掉的正文列不显示，换取不折行、不污染
   下一行；写满时**不**发 `EL`（否则会擦掉最后一格，即右边框）。

正负和解：正面是整类“宽度假设不一致”缺陷消失、无新依赖；负面是脏行整行重写（写入量上界
= 行数 × 列数）、首见字形有一次实测成本、溢出行的尾部正文被挤掉；承担者是后续维护者。

### 15.3 V-003 结果（`summary.json`，自动化 10/10）

| 检查 | 结果 | 可归因信号 |
|---|---|---|
| s1_startup | 通过 | 标题“会话（1/6）”、页脚“窗口 120×30” |
| s1_glyphs_complete | 通过 | 标题/页脚 5 个中文词条逐词在屏（词条清单见未入库的 `script/tui_v003.py`） |
| s1_raw_mode_entered | 通过 | 控制台输入模式 `0x1f0`（ECHO/LINE 均关） |
| s1_keyboard_count | 通过 | 注入真实 KEY_EVENT 后页脚“按键 2” |
| s1_view_switch | 通过 | `3` → 上下文（3/6）、`1` → 会话（1/6） |
| s1_resize | 通过 | 窗口像素改尺寸 → 页脚 120×30 → **95×20** → 120×30 |
| s1_quit_by_q | 通过 | 退出码 0，随后 cmd 继续执行并打印标记 |
| s1_mode_restored | 通过 | 退出后 `0x1f7`（ECHO/LINE 恢复） |
| s2_quit_by_ctrl_c | 通过 | 退出码 0、模式恢复 |
| s3_abnormal_kill | 通过 | `taskkill /F` 后同控制台可继续执行；raw mode 残留**实测** false |

驱动脚本 `script/tui_v003.py`（临时，未入库）另修两处测量错误：`echo %ERRORLEVEL%>file`
被 cmd 当成句柄重定向（改为重定向前置）；`SetConsoleWindowInfo` 在 ConPTY 下恒返回 87
（改为改窗口像素尺寸，实测见 `resize-probe.txt`）。读屏时 `EL` 擦出的空格是 NUL，
直接取字符串会在首个 NUL 截断（已在读屏器显式归一）。

### 15.4 未取得与复审触发

1. 本结论仅覆盖 Windows 11 amd64 + 本机 conhost/字体环境；换终端或字体需重跑 V-003，
   不得沿用“缺字已修”的结论；
2. V-004（真实 SSH 节点）与 72 h 长跑仍缺证据，Check 124 维持未通过；
3. 若日后为 width 逻辑引入依赖（如 `unicode-width` 的 cjk 模式），须先立 ADR——本机实测
   证明它与控制台行为不一致，不能直接替换运行时实测。
