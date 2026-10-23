# 决策台账（D-01—D-15）

本文件记录 tasks.md T030 要求的决策点、结论、证据引用与责任人。状态取值：**已决策**（方向已批准）、**待验证**（结论未取得证据）、**已修订**（原结论被正式修订）。责任人以角色标注，具名责任人待项目排期后回填。

“已决策”不等于“已实现”：凡实现归属后续里程碑的条目，在结论列显式标注归属，不得据此宣称能力已交付。

| 编号 | 主题 | 结论 | 证据引用 | 责任人 | 状态 |
|---|---|---|---|---|---|
| D-01 | Windows 验收工具链与目标三元组 | **已修订**：Windows 验收使用 `x86_64-pc-windows-msvc`，**不使用任何 GNU 工具链**。实测 rustc 1.99.1（host `x86_64-pc-windows-msvc`，LLVM 23.1.1）、MSVC 工具集 14.44.35207、Windows SDK 10.0.26100.0。本机 rustup 默认 host 为 gnu，故采用**目录级** `rustup override set 1.99.1-x86_64-pc-windows-msvc`（机器本地状态，不入库）；`rust-toolchain.toml` 只锁定版本号 `1.99.1`，不锁定主机三元组 | `artifacts/deps-windows.txt`；`rust-toolchain.toml`；`docs/adr/ADR-011-platform-commitment-revision.md`（同批四项决策的批准记录）；`docs/adr/ADR-010.md` | 平台矩阵负责人 | 已修订 |
| D-02 | 单库单工作区 | 一个工作区默认对应一个 DuckDB 数据库文件，默认不按会话分库；任务、事件、消息、产物与检查点在同一事务边界内更新 | `docs/adr/ADR-001.md`；`specs/001-whcode-harness/data-model.md`（`workspace_meta` 为版本源）；`specs/001-whcode-harness/spec.md` Assumptions | 架构负责人（存储边界） | 已决策 |
| D-03 | Web 为独立 UI | Web 是独立原生前端，经 HTTP API / WebSocket 访问同一应用服务，不是 TUI 的 PTY 镜像。技术选型（Web 框架、WebSocket 库、前端构建方式）**待验证**（TBD-04，归属 M6） | `docs/adr/ADR-004.md`；`spec.md` FR-603、FR-606；`research.md` R-05 | Web 端负责人 | 已决策 |
| D-04 | SSH 公钥认证 + 受限入口 | 默认公钥认证 + 受限 WHCode 交互入口，不默认授予任意系统 Shell；**追加客户端范围**：OpenBSD/DragonFly 改由内置 SSH 客户端控制（连接、受限命令、取回输出、按 ADR-007 登记）。服务端实现选型仍待验证（TBD-05，M7） | `docs/adr/ADR-005.md`；`docs/adr/ADR-011-platform-commitment-revision.md`；`spec.md` FR-607、FR-608、FR-611；`crates/whcode-ssh/src/proto.rs` | SSH 入口负责人 | 已决策 |
| D-05 | 追踪默认 redacted | 追踪级别默认 `redacted`；日志路径强制脱敏，禁止明文写入 API Key、令牌与凭据。已实现于 `whcode-domain::TraceLevel` 的默认值与 `whcode-bin` 日志脱敏（`redact`/`[REDACTED]`），并有默认值单测 | `crates/whcode-domain/src/trace.rs`（`default_is_redacted`）；`crates/whcode-bin/src/logging.rs`；`spec.md` FR-203 | 安全与可观察性负责人 | 已决策 |
| D-06 | 性能预算 M0 冻结 | **已冻结（2026-10-23，Windows 11 amd64）**：冻结值＝spec 候选值，NFR-208（p95 5 971 µs）与 NFR-210（6 921 条/秒）实测达标；NFR-201/202/203/205/206/207/209 未取得合规测量、NFR-204 缺 72 小时序列，均保持候选；BSD 按 ADR-013 不参与 | `docs/adr/ADR-025-performance-budget-freeze.md`；`docs/matrices/platform-matrix.md`；`artifacts/overview-perf-m2.json`、`artifacts/baseline-192mb.json`；`spec.md` NFR-201—210、SC-021 | 性能与存储负责人 | 已决策（范围受限） |
| D-07 | 运行时独立 payload | 方向已定：运行时采用独立 payload、按需安装并校验来源/版本/哈希/平台兼容性。**接口已交付**（T102）：`whcode-runtime::payload` 做版本/平台/哈希三重校验（FNV-1a 64，定位为意外损坏检测、非密码学），失败返回结构化错误；内置或下载方式仍由 TBD-06 的独立 ADR 定案 | `docs/adr/ADR-008.md`；`spec.md` FR-703；`research.md` TBD-06；`crates/whcode-runtime/src/payload.rs` | 运行时负责人 | 已决策 |
| D-08 | 不宣称完整 Node.js | **已定性（ADR-024）**：JS **不交付引擎**；`JsProvider` 声明 `Restricted`，执行请求返回结构化 `runtime_unavailable`（能力不足 + 不支持项），UI/门禁/文档不得表述为 Node.js；V-302 的“受限 JS 通过 ECMAScript 测试集”判据在该决策下不可满足，如要恢复须新 ADR + 基准——**修订（2026-10-24，ADR-029）**：按其预留的“新 ADR + 基准”路径恢复受限 JS 引擎——Boa 0.21.1（纯 Rust，Unlicense/MIT）经 `runtime-js` feature 接入，**默认构建零依赖零开销**（`cargo tree` 无 boa），Node API 六名字显式“能力不足”拒绝桩；基准四项（RSS/体积/许可证/启动时延）与回退路径见 ADR-029；声明保持 Restricted + 三项不支持项，指纹 `boa/0.21/js/restricted`；T110／Check 110/153 转入通过 | `docs/adr/ADR-024-runtime-scope.md`；`docs/adr/ADR-029-js-engine.md`；`spec.md` FR-710；`crates/whcode-runtime/src/js.rs` | 运行时负责人 | 已决策（ADR-029 修订） |
| D-09 | Python 子进程协议 | 方向已定：Python 以子进程 worker 运行，先验证子进程协议，不把动态链接 `libpython` 视为完整 Python 环境。**受控子进程路径已交付**（T111）：`PythonProvider` 以系统解释器命令构造受控子进程并记录解释器路径/版本/实现/ABI 指纹，不可用时返回结构化错误；完整 worker 协议与受限档位证据随 ADR-024 收口——**复核（2026-10-24，功能缺口清偿批次）**：T111 完整落地，本行表述自此与代码一致——worker 协议（stdin 单行 JSON 请求经临时文件注入，请求 ≤4 MiB、输出 8 KiB 有界缓冲 + 显式截断标记、失败非零退出 + stderr）+ `PythonEnv` 记录解释器绝对路径/版本/实现名/ABI/`sys_prefix`/C 扩展探测 bool + 受限档位五项不支持项（承诺边界不是沙箱）；取消与超时可区分；Check 111/154 转入通过 | `docs/adr/ADR-024-runtime-scope.md`；`docs/adr/ADR-008.md`；`spec.md` FR-705、FR-709；`crates/whcode-runtime/src/python.rs` | 运行时负责人 | 已决策 |
| D-10 | 独立 db compact | 方向已定：`db compact` 采用“复制—校验—切换—备份—回滚”的独立流程，并与行删除、块回收、WAL checkpoint 区分。**已实现**（T041） | `spec.md` FR-510、§7.4 与横切约束；`research.md` R-09；`crates/whcode-store/src/compact.rs` | 存储负责人 | 已决策 |
| D-11 | 默认系统分配器 | 方向已定：默认使用系统内存分配器，除非基准证明更换有益；不得以分配器替换替代端到端测量。**已定案**（T117）：ADR-020 冻结“默认系统分配器，替换须基准证明”；预算已由 ADR-025 冻结，但 NFR-201/202/203/205/206/207/209 尚无合规测量，热点优化（T115）仍须先补齐测量（T115 保持阻塞） | `spec.md` NFR-201—210；`research.md` R-11、TBD-08；`docs/adr/ADR-020-allocator.md`、`docs/adr/ADR-025-performance-budget-freeze.md` | 性能负责人 | 已决策 |
| D-12 | P1 运行时不阻塞首版 | 方向已定：全部可选运行时默认关闭（`default = []`，不加载动态库、不占端口、不注册工具），首版在无运行时的情况下可用；发布级验证**待实现**（V-301，归属 M8/M9） | `crates/whcode-runtime/Cargo.toml`；`spec.md` FR-704、V-301；`docs/adr/ADR-008.md` | 运行时负责人 | 已决策 |
| D-13 | 平台承诺修订 | OpenBSD 与 DragonFly BSD **不再作为原生安装目标**，改为经 WHCode 内置 SSH 客户端控制；`whcode-ssh` 职责扩展为“客户端 + 服务端”，完整能力属 M7；V-004 判据改为“FreeBSD/NetBSD 服务端验收 + 四平台客户端连通性/命令执行证据”；M0 仅交付客户端风险原型，四平台无节点证据 | `docs/adr/ADR-011-platform-commitment-revision.md`；`docs/adr/ADR-DB-BUILD.md`；`docs/matrices/platform-matrix.md`；`spec.md` FR-607、FR-611 | 平台矩阵负责人 | 已决策 |
| D-14 | 平台范围收敛 | **Windows 11 amd64 为唯一验收目标**；不考虑任何 32 位平台；FreeBSD/NetBSD/OpenBSD/DragonFly 全部**延后**（保留代码与文档，不验收；BSD CI 改为手工触发）。恢复任一平台须新 ADR 并补齐完整证据链 | `docs/adr/ADR-013-platform-scope-windows-only.md`；`docs/matrices/platform-matrix.md`；`spec.md` US1、Assumptions；`.github/workflows/platform-*.yml` | 平台矩阵负责人 | 已决策 |
| D-15 | GUI 形态（铁律） | **已决策（2026-10-23，用户指令）**：桌面 GUI 必须**原生**（平台原生控件或自绘渲染）；**禁止浏览器套壳**——WebView／WebView2、CEF、Tauri／wry、Electron、NW.js，以及“自绘窗口 + 内嵌 Web UI”等价形态一律禁止；`whcode-web` 只作独立浏览器前端（FR-603）。GUI 当前无 FR，落地前须先立需求与 ADR，并给资源、三套主题、无障碍与能力矩阵扩列证据 | `docs/adr/ADR-026-native-gui-only.md`；`docs/adr/ADR-014-minimal-frontend.md`；`docs/adr/ADR-004.md`；`AGENTS.md` §1／§6／§8 | UI 与前端负责人 | 已决策（铁律） |

## 与 ADR 的对应关系

| 决策 | 对应 ADR | 备注 |
|---|---|---|
| D-01 | ADR-010、ADR-011 | 工具链与目标三元组在平台矩阵中登记；GNU 工具链禁用 |
| D-02 | ADR-001 | 单库单工作区 |
| D-03 | ADR-004 | Web 独立 UI；选型待 M6 |
| D-04 | ADR-005、ADR-011 | 服务端（FreeBSD/NetBSD）与客户端（四平台）双范围 |
| D-05 | ADR-006 相关约束 | 追踪默认脱敏 |
| D-06 | ADR-025、ADR-010 | 已冻结（Windows 11 amd64；未测项候选），见平台矩阵 |
| D-07—D-09 | ADR-008、ADR-024 | 运行时方向与 M8 范围；JS 不交付引擎 |
| D-10 | ADR-001 相关约束 | 存储维护流程；实现归属 M1 |
| D-11 | 待建 ADR-ALLOCATOR（M9） | 默认系统分配器 |
| D-12 | ADR-008 | 默认可选运行时关闭 |
| D-13 | ADR-011 | 平台承诺修订 |
| D-14 | ADR-013 | 平台范围收敛（Windows 11 amd64 唯一验收目标） |
| D-15 | ADR-026 | GUI 必须原生；禁止浏览器套壳（铁律） |

## 维护纪律

- 任何结论变更须同步更新本台账、对应 ADR 与 `docs/matrices/platform-matrix.md`，不得只改一处。
- 未取得证据的条目保持“待验证”，不得因文档评审通过而改为“已决策”。
- 状态为“已决策”但实现归属后续里程碑的条目，必须在结论列保留归属标注，避免把方向当成已交付。

## M9 更新（2026-10-22，T121）

| 风险 | 状态更新 | 依据 |
|---|---|---|
| R-01（DuckDB 原生构建） | 已关闭路径：改用官方预编译动态库，SHA256 校验 | `docs/adr/ADR-DB-BUILD.md`、T022 |
| R-02（BSD 平台） | 延后维持：代码级兼容，无原生证据 | ADR-013、T020/T021 |
| R-05（大 payload） | 对策生效：有界 payload + 引用化（stream/事件层不含正文） | M3 交付 |
| R-06（解析失败） | 对策生效：解析器保留原文、纠偏重试 ≤2、未知段落报告 | M4/T069/T075 |
| R-09（空间回收） | 对策生效：compact 复制—校验—切换 + 四项统计不承诺缩小 | M1/T041、D-10 |
| R-10（运行时交付） | 边界冻结：Full/Restricted/ShellOnly、payload 独立分发 | `runtime-inventory.md`、T120 |
| R-11（性能实现） | ADR-020：默认系统分配器，替换须基准证明 | D-11、T117 |
| R-12（性能预算） | **部分解除**：ADR-025 已在 Windows 11 amd64 冻结（NFR-208/210 达标）；未测项仍候选，T115/T116 保持阻塞 | `compatibility.md` 已知限制、`docs/adr/ADR-025-performance-budget-freeze.md` |
| 新增 R-13（自动提交工具扰动） | 仓库 git 历史被外部自动提交拆碎（英文消息、中间态提交）——建议关闭工具并 squash | 2026-10-22 会话记录 |

每 PR 清单检查（§15 七项）在 `docs/guides/pr-checklist.md` 维持。
