# ADR-077：沙箱强度评估与如实上报（P5.2 第一步，OS 层强制隔离显式推迟）

- **状态**：已批准（2026-11-15，proposal-v3 P5.2；用户指令“继续功能开发”）
- **来源**：`research/synthesis/proposal-v3.md` W5（P5.2“沙箱评估与强度上报（Windows ACL 试点；
  `full/partial` 字段）”）；ADR-070 威胁模型与“P5.2 落地时逐条复审”的退出条件；
  `research/dsh-comparison/domain-C-*.md`（DSH 自报 `partial` 的能力事实）；FR-781
- **责任人**：安全与可观察性维护者、平台适配维护者、装配根维护者
- **退出条件**：见“已知限制与失效条件”节——任一 OS 层机制落地即复审本 ADR 的强度判据

## 1. 问题与边界

ADR-070 给 shell 工具立了威胁模型，并把 T3（凭据经文件外泄）、T4（二级命令 PATH）、
T5（资源耗尽）、T6（工作区逃逸写）四条残余风险**登记至 P5.2**。P5.2 的提案表述是“沙箱
评估与强度上报”，并明确采纳形态是“**强度如实上报**，不是声称已隔离”。

因此本批要回答的不是“怎么把命令关起来”，而是两件更容易被含糊掉的事：

1. **现在到底有什么**：哪些控制是真生效的（有实现落点、有测试），哪些风险还在；
2. **强度怎么判定**：判据必须能被证伪——不能靠“配置里有沙箱字样”或“工具默认关闭”就给分。

边界（不在本批）：

- **不做 OS 层强制隔离**。理由不是“来不及”，而是当前硬约束下不可实现，见 §2 的事实链；
- 不做沙箱策略编辑面（GUI/Web 配置 UI）、不做 per-会话沙箱、不做网络代理式出站约束。

## 2. 事实链（0 信任校验：为什么 OS 强制隔离本批不可实现）

一手依据：Microsoft Learn《AppContainer isolation》《Launch an AppContainer》（2026-11-15 取读，
页面 `ms.date` 分别为 2025-07-08、2023-07-20，内容含代码样例）。落地步骤是：

1. `CreateAppContainerProfile`（或 `DeriveAppContainerSidFromAppContainerName`）取得 Package SID；
2. `DeriveCapabilitySidsFromName` 构造能力 SID，填 `SID_AND_ATTRIBUTES`；
3. `InitializeProcThreadAttributeList` ＋ `UpdateProcThreadAttribute(PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES)`；
4. `CreateProcess(EXTENDED_STARTUPINFO_PRESENT)` 启动，LPAC 另需
   `PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY`；
5. 工作区目录需按 Package SID 授权 DACL（否则容器内读写全被拒）。

本仓现状（可复验）：

- 依赖树内**没有** `windows-sys`／`windows`／`winapi` 等 FFI 绑定（`grep` 各 `Cargo.toml` 为空）；
- `workspace.lints.rust.unsafe_code = "forbid"`（NFR-215）：`forbid` 不可被 `#[allow]` 覆盖，
  上述 Win32 调用在 Rust 侧全部是 `unsafe fn`，无安全包装依赖即无法调用；
- `std::process::Command` 不暴露 `STARTUPINFOEX.lpAttributeList`——即便拿到 SID 也无法用 std 启动；
- 第 5 步要改写用户工作区 DACL，属不可逆副作用面，必须有回退与审计设计（ADR-015 口径的
  不可逆决策），不属于“顺手加一个字段”的量级。

结论：本批的可行面＝**评估与上报**；强制隔离需要单独立项（新 ADR：安全包装依赖或
ADR-015 式隔离 crate 的汇编例外，二者都涉及“引入 unsafe 面”的宪法级决策）。

## 3. 威胁模型（上报面自身的威胁）

资产：用户对“命令执行有多安全”的判断；`doctor` 输出的可信度。

| # | 威胁 | 对策（本 ADR 落地） | 残余风险 |
|---|---|---|---|
| R1 | **过度声明**：把工具级治理说成 OS 沙箱，用户据此放松审批 | 强度按“实际生效的面”判定；OS 事实由平台 crate 单一来源给；`full` 只在 OS 隔离生效且缺口清零时成立（当前不可达，测试钉住） | 用户仍可能只看 `partial` 不看 `gaps`——文案把缺口逐条列出 |
| R2 | **双口径**：上报说“已启用”、实际未注册 shell 工具 | 执行面判定复用 `shell_tool::build_governance`（与实际注册条件同一处函数），不另立谓词 | — |
| R3 | **静默失败**：配置非法时上报“未启用且一切正常” | 配置非法 → `sandbox_error` 显式给出原因，同时强度记 `none`（不声称已启用） | — |
| R4 | **陈旧表**：控制项表与实际实现漂移（删了实现还列着控制） | 每条控制给 `source` 落点；控制项 id 唯一性有测试；“新增/删除控制必须同批改表”写入失效条件 | 表是静态事实，靠评审与失效条件守 |
| R5 | **伪造缺口清零**：为了让界面好看，把缺口表清空 | 缺口表在 OS 未隔离时**恒非空**（`evaluate` 里由 `os_enforcement` 决定，不由调用方传入）；测试钉住 | — |

## 4. 决策

1. **三档强度，判据可证伪**（`whcode_domain::sandbox::SandboxReport::evaluate`）：
   - `none`：模型没有任意命令执行面（shell 工具未注册）——不宣称存在沙箱；
   - `partial`：有执行面，只有工具级控制，OS 层无强制隔离；
   - `full`：有执行面，且 OS 层隔离生效、缺口清零（**当前不可达**）。
   判定顺序固定：无执行面 → `none`；有执行面且缺口为空 → `full`；其余 → `partial`。
2. **控制项＝六条静态事实**（逐条对应实现与 ADR-070 威胁编号）：`cwd_anchor`、`minimal_env`、
   `credential_strip`、`output_truncation`、`timeout_tree_kill`、`concurrency_gate`。表里不得
   出现“计划中”条目——上报面只写当前为真的事实。
3. **缺口项＝五条残余风险**：`filesystem_write`（T6）、`filesystem_read`（T3）、`network`、
   `disk_quota`（T5）、`secondary_command_path`（T4）。缺口由 `os_enforcement` 决定，调用方
   不能手工清空。
4. **OS 事实只在平台 crate**：`whcode_platform::sandbox::os_enforcement()` 返回
   `Active{mechanism}` 或 `Unavailable{reason}`，`reason` 必须可操作并含 ADR 指针（有测试禁止
   “未知”“暂不支持”）。纯函数、不探测子进程——避免 `doctor` 输出随机器状态漂移。
5. **出口在 `doctor`（文本与 JSON 同源）**：`doctor` 增 `sandbox` 块（`strength`、
   `execution_surface`、`os_enforcement`、`controls`、`gaps`）与 `sandbox_error`；文本行与 JSON
   来自同一份 `SandboxReport`。执行面经 `build_governance` 判定，配置非法显式报错。
6. **不强加新依赖、不做 UI 面**：不引入 Win32 绑定，不改前端预算；GUI/Web 后续可直接消费
   `doctor --json` 的 `sandbox` 块，属独立批次。
7. **非目标**：OS 层强制隔离、沙箱策略配置面、per-会话沙箱、出站网络约束、把缺口包装成
   “已缓解”。

## 5. 正负和解

**正面**：把 ADR-070 的四条残余风险从“文档里的登记”变成“运行时可见、可枚举、可脚本消费”的
事实；上层（GUI/Web/CI/用户）不必读源码就知道当前强度；`full` 不可伪造。

**负面与承担者**：

- 上报只描述现状，**不降低任何风险**——承担者是继续在无隔离环境里跑命令的用户；
- 静态表需要人守（R4），新增控制项若忘记入表，上报就少报——承担者是维护者，用失效条件兜；
- `doctor` 多一次 bash 探测（`build_governance` 内部），与既有 runtimes 探针同量级——承担者
  是 `doctor` 的延迟（一次进程探测，可接受）。

**和解理由**：在“不能做隔离”的硬约束下，**如实上报**是唯一不伪造的交付形态；它把风险从
“隐性”变为“显性”，并为将来的隔离批次留下可对照的基线。

## 6. 三重时间

- **过去**：ADR-070 把 T3/T4/T5/T6 登记至 P5.2，并要求“沙箱落地时逐条复审”；
- **现在**：本批给出上报面与判定规则；ADR-070 的复审结论＝**未收敛**（四条残余仍在），
  现由 `gaps` 显式承载（ADR-070 已加修订记录）；
- **将来失效条件**（绑定命令＋判定）：任一 OS 机制落地 → `os_enforcement()` 返回 `Active`，
  此时 `evaluate` 会自动给 `full` 并清空缺口——**这是必须与真实隔离同时发生的改动**，
  `cargo test -p whcode-domain sandbox` 与 `cargo test -p whcode-platform sandbox` 会立刻红，
  迫使同批复审判据与文档。

## 7. 回退方案

纯读、无迁移、无新依赖：删除 `sandbox` 模块与 `doctor` 的 `sandbox`／`sandbox_error` 键即可，
数据层零影响。生产方（平台 crate）与判定（domain）均可独立回退。

## 8. 已知限制与失效条件

- **不是隔离**：强度 `partial` 的含义是“只有工具级控制”，文件系统与网络仍不设限；
  对外文案不得表述为“已沙箱化”；
- **`full` 不可达**：当前两平台分支均为 `Unavailable`，任何 `full` 输出都意味着实现被改坏；
- **控制表是静态事实**：实现删除而表未改 → 上报过度声明，属 R4；
- **不做配置面**：用户不能通过配置改变强度（没有“开启沙箱”开关）——避免出现“开了但还是
  `partial`”的误导；
- **失效条件与复审触发（绑定命令＋判定规则）**：
  - `cargo test -p whcode-domain sandbox` 或 `cargo test -p whcode-platform sandbox` 红 →
    停止交付，先修判定或事实表；
  - 新增/删除任一条工具级控制（`TOOL_CONTROLS`／`OS_GAPS` 与实际实现不再一一对应）→
    同批复审本 ADR 与 `docs/matrices/runtime-inventory.md`；
  - `grep -rn "unsafe" crates/` 出现新条目，或 `Cargo.toml` 新增 Win32 绑定依赖 →
    本 ADR §2 的事实链失效，必须重开（很可能就是 OS 隔离批次的起点）；
  - `python .github/scripts/check-code-scanning.py` 出现沙箱相关 open 告警 → 修复后重推。

## 9. 文献依据（标准研读记录，§1.6 口径）

- Microsoft Learn《AppContainer isolation》与《Launch an AppContainer》：隔离覆盖面（凭据、
  设备、文件、网络、进程、窗口）与启动所需的完整 API 序列——本 ADR §2 的事实链来源；
- ISO/IEC 25010:2023（已登记，部分覆盖）：`security` 的 confidentiality／integrity／
  authenticity 与 `functional correctness`——把“是否真的隔离”当作可测属性而非宣传口径；
- ISO/IEC 27001:2022（已登记，有意偏离 ISMS）：技术控制同构项参照——本仓不做 ISMS，但要求
  控制项有实现落点与测试，不接受“声称已控制”；
- ISO/IEC/IEEE 15026-2:2022（已登记，已覆盖）：claim→argument→evidence——`strength` 是 claim，
  `controls`／`gaps` 是 argument，`source` 落点与测试是 evidence；
- ISO/IEC/IEEE 15939:2017（已登记，已覆盖）：指标→测量→判定——强度判定规则即 decision criteria。

## 10. 形式审查

- **真实矛盾**：用户需要知道“让模型跑命令有多危险”，而代码当前只能回答“有审批、有超时”，
  无法回答“文件与网络有没有被关起来”；含糊回答会导致错误的风险判断；
- **正负和解**：见 §5，负面（不降风险、静态表需人守）显式且归属明确；
- **0 信任校验**：Win32 可行性以官方文档的 API 序列＋本仓 `forbid(unsafe_code)` 与依赖树
  实测（无 FFI 绑定）为准，不以“应该有 crate 能做”作论据；
- **知难行易**：方向＝P5.2“评估与上报”；目标＝强度可判定、缺口可枚举、`full` 不可伪造；
  判据＝三 crate 测试＋`doctor` 文本/JSON 同源；非目标＝OS 强制隔离、配置面、UI 面；手段
  （静态事实表＋纯函数判定＋`doctor` 单出口）是在确认“隔离不可实现”之后才选的。
