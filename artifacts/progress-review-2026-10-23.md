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
| 验收检查点（checklist.md） | 249 项全部有复核记录：**191 通过 / 58 未通过** | 复核完成 |

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

## 4. 验收层结果（checklist.md：249 项全部有复核记录，191 通过 / 58 未通过）

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
**85 项通过、32 项未通过**。主要缺口：
① 三时点崩溃注入与“副作用已发生未登记”恢复无用例（139/158/196/207，`tests/crash` 仅有 README）；
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
191 通过（14 项为 2026-10-21/22 的用户复核，177 项为本轮）、58 未通过。这意味着：

- G4 验收门禁**仍未成立**：58 项未通过即意味着对应里程碑不能宣称验收通过；
- `check_counts.py` 只校验“FR/任务/检查点数量一致”，不校验勾选比例，故该缺口不会被现有门禁抓到。

复核前最大的结构性风险是进度指标（95.9%）与验收指标（5.6%）背离；本轮把 235 项判据
逐条落笔、回退 9 条虚高任务。此后任何对外“完成度”表述若只引用 tasks.md 而回避
“191/249 通过”，即构成 AGENTS §1.4“不伪造”与 §5“禁止跨门禁跳跃”的反面案例。

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
**249 项全部有复核记录：191 通过 / 58 未通过**；并据实回退 10 条任务
（T093/T110/T111/T112/T114/T118/T119/T123/T124/T127）。
复核同时暴露并处置四处缺陷——① 仓库无 `.github/workflows/`，`ci/platform-matrix/*.yml` 从未被平台执行；
② `decisions.md` 的 D-07—D-11 状态停留在“待实现”；③ `feature-parity.md` 的 CUI 列仍标“未交付”；
④ `docs/guides/README.md` 的指南索引严重滞后（只列出一份，且把已交付的 CUI 手册仍写作“计划”）。
四项均已同步更正，并在对应文件中写明复核日期与依据。

**复核给出的下一步优先级（按缺口影响）**：补 `tests/crash` 三时点注入（139/158/196/207）→
M8 引擎/worker 接入（153/154/155）或正式降级声明 → Web 写命令与 WS 广播（143/183/199/209）→
预算冻结（133/136/195）→ 发布包与三份指南（118/119）→ CI 接线（3/249）。

**附带教训（已体现在本报告）**：门禁按**源码文本**扫描，注释中书写该字面量同样命中；
首次修复即因注释措辞被门禁再次拒绝，改述后复绿。

