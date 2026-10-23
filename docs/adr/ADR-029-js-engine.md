# ADR-029：恢复受限 JS 引擎（修订 ADR-024 的 D-08 条款）

- **状态**：已批准（2026-10-24）
- **来源**：用户指令“补全所有缺失的功能缺口，不计成本代价”（2026-10-24）；
  ADR-024 退出条件原文“若要引入内置 JS 引擎……必须另立 ADR 并附基准（RSS／体积／
  许可证／启动时延）与回退路径”；T110、FR-708/710、V-302（Check 110/153）
- **责任人**：运行时负责人
- **退出条件**：若 `runtime-js` feature 下出现资源回归（空闲 RSS 超预算、执行延迟不可
  接受）或 Boa 出现不可修复的安全/正确性缺陷，撤销本 ADR 回到 ADR-024 口径
  （`JsProvider` 诚实不可用），回退路径＝关闭 feature 重编译，零数据迁移

## 决策

1. **以 Boa 0.21.1（纯 Rust）作为受限 ECMAScript 引擎**，经 `whcode-runtime` 的
   `runtime-js` feature 接入；**默认构建不启用**（FR-704/NFR-102 不变：可选运行时依赖
   留在 feature 之后，默认 `cargo build` 依赖图无 boa——`cargo tree -p whcode-runtime`
   实测 0 命中）。
2. **能力边界不变**（§4.7.1/D-08 精神延续）：声明保持 `Restricted` + 三项不支持项
   （Node.js 内置模块/文件系统/网络）；指纹 `boa/0.21/js/restricted`；不得表述为
   Node.js 兼容。
3. **Node API 拒绝面显式化**：`require`/`process`/`fs`/`net`/`http`/`child_process`
   六个名字注册为抛出“能力不足”（含运行时名与 API 名）的桩——不崩溃、不静默返回
   undefined（Check 110/153）。
4. **执行有界**：死循环经 `RuntimeLimits`（循环 1 000 000 次、递归 256 层）封顶，
   求值前后墙钟事后判定 `timed_out`；结果/输出按 `MAX_OUTPUT_BYTES` 有界尾部保留。
5. **默认构建行为不变**：feature 关闭时 `JsProvider` 保持诚实不可用
   （`runtime_unavailable` + 安装建议），现有测试语义不变。

## 基准（ADR-024 退出条件要求的四项）

| 项 | 实测（2026-10-24，Windows 11 amd64，rustc 1.99.1 msvc） |
|---|---|
| RSS | feature 构建测试进程（含引擎 1000 次求值）release 15.1 MiB / debug 18.2 MiB |
| 体积 | runtime 库测试二进制 1 081 344 B（无 feature）→ 12 787 712 B（feature，+11.7 MiB）；**默认构建产物零增量**（不启用 feature 时不进依赖图）；依赖包数 573 → 637（+64，供应链面扩大，见理由） |
| 许可证 | `boa_engine` = Unlicense OR MIT（宽松，与 BSD-2-Clause 仓库存容）；Cargo.lock 版本锁定 |
| 启动时延 | 引擎为进程内按需实例化，无启动期常驻成本；单次小脚本求值 release ≈ 0.035 ms（1000 次合计 34.99 ms，28 579.9 次/秒；debug 3 064.9 次/秒） |

## 理由（正负和解）

- **正面**：T110/Check 110/153 从“结构性不可满足”转为可交付；JS 场景不再只能报
  `runtime_unavailable`；与 ADR-024 的资源关切和解——默认构建（发布包口径）零依赖、
  零体积、零启动成本，引擎成本只由显式启用者承担。
- **负面**：启用方承担 +11.7 MiB 二进制与 +64 个依赖包的编译/审计面；Boa ≠ 完整
  Node.js（能力清单已声明）；同步解释器无墙钟中断钩子，超时为“有界解释器 + 事后
  判定”，最坏窗口由循环上限封顶（约百毫秒级），不能在任意长脚本执行中途打断。
- **承担者**：启用 `runtime-js` 的部署方（体积/编译时间/供应链审计）；普通用户不受影响。
- **和解理由**：用户显式要求补全功能缺口且不计成本；项目优先序第 1 条（资源占用）
  经“默认关闭 + feature 门控 + 基准留档”保全——这是“可撤销的放宽”，不是常量级
  全员成本。

## 三重时间

- **过去来源**：ADR-024 的退出条件明确预留本路径（“另立 ADR + 基准”）；D-08 的动机
  （不宣称完整 Node.js、无 Windows 基准不引入）被本 ADR 的基准与声明边界承接。
- **现在证据**：上表四项基准 + `fixed_ecmascript_suite_passes`（9 例，含中文）+
  `node_api_rejections_fail_with_capability_error`（6 名字逐一断言）+
  `runaway_loop_is_bounded_and_fails_structured`；均为可重跑自动化用例。
- **将来失效条件与复审触发**：Boa 版本升级须复测四项基准并同步指纹中的版本串；
  若启用方报告空闲 RSS 或执行延迟回归、或 Boa 出现安全通告，复审本 ADR；FR-704
  门禁（默认构建无运行时依赖）由 Check 9/103/189 持续强制。

## 替代方案（已否决）

- **维持 ADR-024 不引入引擎**：与用户显式指令冲突，且 Check 110/153 永久结构性未通过。
- **rquickjs（QuickJS 绑定）**：C 依赖引入构建期工具链风险（B-04 同类），且绑定层
  unsafe 面更大；Boa 纯 Rust 与 NFR-215 兼容。
- **脚本级正则拦截 Node API 名**：可绕过（字符串拼接/eval），不作安全边界；
  显式桩 + 结构化错误才是可判定路径。
