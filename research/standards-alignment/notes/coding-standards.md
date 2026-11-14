# T8 研读笔记：编程规范与语言标准

**研读日期**：2026-11-14

## 1. Rust 侧

| 对象 | 现行状态 | 证据层级 |
|---|---|---|
| Ferrocene FLS（Rust 语言规范） | 已捐入 rust-lang 项目（2025-03-26），仓库 github.com/rust-lang/fls 公开；2025H1 目标“发布 rust-lang 自有首版”；社区指认其覆盖有缺口 | 确证（rust-lang 仓库＋Ferrous 博客＋goals 页交叉） |
| Safety-Critical Rust Consortium | Rust Foundation 2024-06 联合 10 家成立；公开仓库 safety-critical-rust-coding-guidelines | 确证（检索交叉） |
| SAE SAfEr Rust Task Force | SAE StandardsWorks 下在研（ISO 26262 语境的语言子集与证据生成） | 确证（检索交叉） |
| MISRA Rust | **不存在**官方“MISRA Rust”；MISRA 出了 C:2025 Addendum 6（评估 MISRA C 规则对 Rust 的适用性，19 页） | 确证（检索交叉；负向发现同样按证据登记） |
| Rust API Guidelines / rustfmt / clippy | rust-lang 官方文档与工具（本仓已在用） | 确证 |

**对 WHCode 的映射**：
- 语言规范参照：普通 Rust 零 unsafe（workspace forbid）＋ clippy all 零告警 ＋ mem_forget deny——本仓对“内存安全子集”的执行方式是**工具强制**，比阅读 FLS 条款更强；FLS 的价值在“语义仲裁”（文档与编译器行为争议时的一手参照），登记为 reference 性质，无门禁动作。
- 编码规范：本仓没有“Rust 版 MISRA”可对标（不存在）；Safety-Critical Rust Consortium guidelines 可在落地阶段抽读一次，取与“零 unsafe＋有界容器”条款重叠度结论 [待验证-未逐条读]。
- 判定预判：已覆盖（工具强制路线），无新动作；负向发现（MISRA Rust 不存在）写入报告防止后来者按讹传对标。

## 2. POSIX.1-2024（IEEE Std 1003.1-2024）

| 事实 | 值 | 证据层级 |
|---|---|---|
| 现行版 | POSIX.1-2024（2024-06-14；The Open Group Base Specifications Issue 8；约 4107 页；对齐 C17） | 确证（Open Group 官方在线版 pubs.opengroup.org/onlinepubs/9799919799） |
| 免费全文 | Open Group 在线版官方免费（章节 2 为 Shell Command Language；2.9 退出状态；3.206 定义） | 确证 |

**对 WHCode 的映射（直接落点）**：shell 工具（ADR-070、T290—T292、FR-777）执行系统 shell 命令——其可解释语义域就是 POSIX shell 命令语言＋各 shell 方言。ADR-070 的威胁模型（命令注入面、引号/扩展语义）可把 POSIX.1-2024 §2 作为语义基准引用：退出状态 0—255 约定、引号与展开次序、“未定义行为不依赖”。判定预判：引用级覆盖（落地候选：ADR-070 或 tool 文档补 POSIX 基准句，不引入新门禁）。

## 3. CERT / MISRA C / AUTOSAR C++（awareness 层）

| 对象 | 状态 | 本仓关系 |
|---|---|---|
| SEI CERT C/C++ | 现行（SEI 维护） | 本仓不写 C；shell 工具的命令面安全性由 ADR-070 威胁模型承担，不由 C 编码规范承担 |
| MISRA C:2023/2025 | 现行 | 同上，参照价值＝其“可判定的规则＋偏离记录（deviation）”形态——与本仓“有意偏离须写明理由”同构 |
| AUTOSAR C++14 | 现行（准则库） | 同上 |

**综判**：编码规范域本仓的真实对标对象是“rustfmt＋clippy＋workspace lints＋NFR-215 有界纪律”，已程序化；外部规范的剩余价值是**偏离记录形态**（MISRA 的 deviation 机制）与 **POSIX 语义基准**（shell 工具）。无新增门禁建议。
