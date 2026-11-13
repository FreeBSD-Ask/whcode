# ADR-069：工具链升级至 Rust 1.100.0 并引入 1.100 新功能

- **状态**：已批准（2026-11-14，用户指令：阅读 1.100.0 文档、全面引入新增功能、全局工具链与组件适配）
- **来源**：Rust 1.100.0 官方发布公告（2026-11-12）与 releases.rs 逐项清单双源交叉复核；
  本机实测（`rustc -vV`、全量构建）；清华 TUNA 与 rsproxy.cn 镜像可用性双探针实测
- **责任人**：工具链负责人、平台矩阵负责人、GUI 验收负责人
- **退出条件**：见"将来失效条件与复审触发"节——验证链因工具链差异失败且仓库内无法修复时，
  回退口径为 `rust-toolchain.toml` 改回 `1.99.1` 并重开 MSRV 对齐

## 背景

锁定工具链自 M0 起为 1.99.1（D-01 修订口径，T002/T021）。2026-11-12 上游发布 1.100.0，
2026-11-14 用户指令升级并全面引入新功能。与 D-01 的一致性：只升版本号，host triple 交给
各平台 runner 默认值，"不使用任何 GNU 工具链"口径不变（ADR-013）；MSRV 跟随锁定工具链
的既有口径不变（Cargo.toml 注释保留演进史）。

## 决策

1. **工具链与 MSRV**：`rust-toolchain.toml` `channel = "1.100.0"`（本机实测 rustc 1.100.0
   `0ef1bc386 2026-11-09`、cargo 1.100.0、LLVM 23.1.5）；workspace `rust-version = "1.100"`；
   目录级 override 同步为 `1.100.0-x86_64-pc-windows-msvc`（机器本地，不入库）。
2. **采用的新功能（逐项：真实矛盾 → 落点）**：
   - `CommandExt::show_window`（Rust 1.100 稳定）：GUI（windows 子系统）主进程 spawn
     控制台子进程会闪黑窗，是每次 spawn 都发生的真实矛盾。新增
     `whcode-platform::process::configure_child_window`（SW_HIDE + CREATE_NO_WINDOW
     双保险：前者覆盖 GUI 子进程首窗，后者杜绝控制台子进程新建控制台窗口，stdio 经
     管道不受影响）。落点：platform 无头命令（`shell_command`／`taskkill`／`tasklist`／
     `powershell`）、`resources.rs` 两处查询、`whcode-runtime` worker spawn、`whcode-mcp`
     服务器 spawn（服务器被配置为 GUI 程序时首窗按 SW_HIDE 隐藏）。
   - `str::trim_prefix`：`whcode-llm::http::frames` 的 SSE `data:` 解析原以
     `strip_prefix(..).unwrap_or(同一变量)` 表达"有则剥除"，替换为 `trim_prefix`
     （语义等价、去掉一层 Option 装饰）。仓库内其余 `strip_prefix` 用法语义不同
     （需要"是否剥除"的判定分支）或为 all-or-nothing 引号剥除，**不换**。
   - never type `!`（1.100 稳定，`Infallible` 成为别名）：`whcode-store::service` 非测试
     构建的闸门占位 `type Gate = Infallible` 改为 `type Gate = !`（行为等价，意图更直白）。
   - Cargo 新 lint 系统：`[workspace.lints.cargo] unused_dependencies = "warn"`——未使用
     依赖首次有内建可见性。仅 warn 不 deny：该 lint 只在全 target 构建时可靠、不覆盖
     dev-dependencies，防误报破坏门禁。首轮构建实测零告警（未发现未使用依赖）。
   - 工具链红利（自动生效，不改代码）：Unicode 18.0.0、next solver 进入 orphanck、
     `aligned_fields_in_packed` deny 级 lint（本仓库无 `repr(packed)`，实测不触发）、
     扩展后的 dropping lints 覆盖 `drop_in_place`（本仓库无直接调用）。
3. **评估后不采用（登记理由，防止"为 adopting 而 adopting"）**：
   - Allocator API（`std::alloc::Allocator`／`Vec::*_in`／`Box::new_in` 等）：与进程 RSS
     上限目标相关，但自定义分配器是架构级变更，需独立 ADR + 并发档案 + 同夹具前后测量；
     现行口径 `memory_limit` 只约束 DuckDB 缓冲管理器、进程级上限靠有界容器纪律。
     **暂缓**，登记为 1.100 红利的后续方向（M9 资源深化时复审）。
   - `Child::main_thread_handle`：Windows 进程树终止已按 `taskkill /T` 收敛并有零残留
     门禁（Check 109/201），线程句柄路径属重复机制。
   - `core::mem::DropGuard`：与 `whcode-bin::serve`／`whcode-ssh::proto` 的测试局部手写
     Guard 属等价样板替换，无行为收益，不做顺手重构。
   - `bool::toggle`／`Vec::from_fn`／`Rc::map`／`Arc::map`／`Default for Once`／
     `UnsafeCell::replace`：现有惯用形式已是最优表达，替换属装饰性变更（且本仓库无
     `UnsafeCell` 直接使用）。
   - cargo 依赖冷却（`min-publish-age`，RFC 3923）：本仓库 `publish = false`，无发布面。
   - `cargo install` 锁定行为变更、`-Cprofile-sample-use`、rustdoc
     `invalid_markdown_table` 门禁化：与本项目工作流无交点，暂不引入。
4. **兼容性变更面（1.100 的行为变化）与兜底**：闭包在 `if let` 中的捕获精度、never type
   fallback 全 edition 统一为 `!`、`box` pattern 移除、dyn 兼容性检查扩展、`Box` impl
   泛化（推断差异）、x86 "Rust" ABI 改用 SSE 寄存器——由 fmt/clippy `-D warnings`/全量
   测试兜底；首轮本机全量构建通过且零告警（2026-11-14，`cargo build --locked
   --workspace`，4 分 22 秒）。cargo 新的中间产物目录布局（build-configuration hash 上移）
   不影响本仓库脚本：门禁脚本以 `cargo` 输出与 `target/` 固定根路径为准，实测全量构建无碍。
5. **本地构建镜像（机器本地状态，不入库）**：rustup 分发走清华 TUNA
   （`RUSTUP_DIST_SERVER`／`RUSTUP_UPDATE_ROOT` 用户级持久环境变量，实测可用）；crates.io
   注册表走 rsproxy.cn sparse（用户级 `~/.cargo/config.toml` 源替换）——TUNA 经双探针
   实测**不提供** cargo 可用的 crates registry 镜像（git 智能协议与 sparse 端点均 404，
   帮助站无 crates 页）。镜像只影响下载路径，不改变 `Cargo.lock` 与构建产物；CI 在
   GitHub runner 上保持官方源。
6. **CI 与平台**：FreeBSD/NetBSD job 的 rustup `--default-toolchain` 同步 1.100.0；
   NetBSD MSRV 闸门从 Cargo.toml 动态读取（既有设计，避免手写漂移）。ARM64 构建交付
   通道（ADR-043）不受影响。平台一致性证据（Check 21）待下一轮 CI 回填。

## 形式审查（宪法原则 I）

升级解决的真实矛盾：a) 工具链落后上游两个版本，新 deny 级 lint 与兼容性变更越晚面对
成本越高；b) GUI 宿主每次 spawn 子进程闪控制台窗，此前无 std 级收口点（项目从未使用
`creation_flags`）；c) 未使用依赖此前只能靠人工目测。三者都有真实代价，不是"为了新而新"。

## 正负和解（宪法原则 II）

- **正面**：未使用依赖可见性、GUI 宿主闪窗消除、与上游同步（安全修复与生态兼容）、
  `trim_prefix`/`!` 表达力、DuckDB/依赖的 MSRV 余量增大。
- **负面**：a) 兼容性变更面一次性（构建 4 分 22 秒重编全部 crate）——已兜底；
  b) 新 rustfmt/clippy 排版与 lint 可能产生差异——由门禁兜底，首轮零告警；
  c) `show_window` 的视觉验收需 GUI 宿主人工确认——登记为复审条件；
  d) warn 级新 lint 的噪声由维护者承担（当前为零）。
- **承担者**：一次性重编成本由本地与 CI runner 承担；视觉验收由 GUI 验收负责人承担。
- **和解理由**：负面均一次性或有界、有明确承担者；正面长期持续。

## 将来失效条件与复审触发（绑定命令 + 判定规则，ADR-046 口径）

- `cargo build --locked --workspace` 或 `cargo clippy --locked --workspace --all-targets
  -- -D warnings` 在 1.100.x 上失败且无法在仓库内修复 → 复审本 ADR（回退 = 锁定值改回
  `1.99.1` 并重开 MSRV 对齐）。
- `[workspace.lints.cargo] unused_dependencies` 连续两轮 CI 零告警 → 评估升 deny
  （`Cargo.toml` 一行改动 + 本 ADR 追加修订记录）。
- GUI 冒烟验收（1200×800 空闲态）人工确认 spawn 子进程无闪窗；若 SW_HIDE +
  CREATE_NO_WINDOW 双保险仍有闪窗 → 复审 `configure_child_window` 落点与口径。
- Allocator API 暂缓项：M9 资源深化评估时复审（独立 ADR + 同夹具前后测量 + 回退验证）。
- 已知限制：`unused_dependencies` lint 不覆盖 dev-dependencies、只在全 target 构建时
  可靠（上游声明，首轮实测一致）；`configure_child_window` 的视觉效果未经 GUI 宿主
  人工确认前标 `[待验证]`；NetBSD 侧 rustup 安装 1.100.0 的可达性（`static.rust-lang.org`
  出口）未实测，以下一轮 CI run 为准。

## 0 信任校验

- 版本与功能清单：官方 blog（2026-11-12）与 releases.rs 双源交叉；本地 `rustc -vV`
  复现（`0ef1bc386 2026-11-09`），三者一致。
- 镜像可用性：TUNA rustup dist 实测 200；TUNA crates.io 经 `git ls-remote` 与 sparse
  `config.json` 双探针 404 判定不可用（不以"曾提供"的旧知识为准）；rsproxy 官方推荐
  sparse 配置原文照录。
- 兼容性结论（"零编译错误、零告警"）以本机全量构建为准，不以公告为准；测试与平台
  结论以 CI 日志与 artifact 为准（ADR-037），未取得前不写"通过"。
