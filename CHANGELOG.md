# 变更记录

本文件记录可观察的变更。格式遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

**版本标记约定**：

- `0.1.0` 对应 M0（平台可行性与基线冻结）的工程侧基线，**不是发布版本**；
- Git 标签形如 `v0.1.0-m0`（预发布标识），只标记“该里程碑证据齐备时的代码状态”；
- 只有在该里程碑的强制验收全部通过后，才会去掉预发布标识。

## [Unreleased]

### 变更

- Code Scanning 修正到零告警并立为纪律（2026-10-29，用户指令）：`security/code-scanning`
  46 条 CodeQL 告警（`rust/cleartext-logging` 45、`rust/log-injection` 1）全部修复。
  生产侧两处根因：`ProviderFactory::create` 接收了从未使用的已求值凭据（`_credential`），
  使密钥值跨越装配边界并污染 dyn 派发调用的返回值链路——该参数删除，凭据由传输层自行
  求值（失败时机不变）；`HttpClient` 原先把 `AuthInjection`（含密钥原文）驻留在实例里，
  使经由该对象产生的返回值整体带污染——改为按请求求值、即用即弃。
  测试侧把凭据／会话标识／证书材料派生值从断言消息中移除，只保留静态文案、常量与 bool
  （CodeQL 的敏感来源是命名启发式，`.len()` 计数同样携带污染）。
  新增 `.github/scripts/check-code-scanning.py`（事实源 GitHub API：0 open 为 GO、有告警为
  FAIL、查询阻塞为 BLOCKED）与 `ci.yml` 的 `code-scanning` job（schedule 与手动触发）；
  AGENTS.md §5.2 立“零告警铁律”，标准验证链与 G4 退出判据同步。
- 发布链可重现构建（2026-10-29，ADR-047，用户指令）：同一提交的全新构建在**同一构建环境**
  （同机或同一 CI 镜像，全量 clean 含依赖重编重链）下字节一致。`whcode-bin/build.rs` 支持
  外部注入 `WHCODE_BUILD_STAMP`（合法值透传、非法值告警回退墙钟）；`build-release.yml` 以
  HEAD 提交时间导出 `WHCODE_BUILD_STAMP`（UTC `Z`）与 `SOURCE_DATE_EPOCH` 注入构建与打包；
  `package-windows.py` 由 `SOURCE_DATE_EPOCH` 驱动 `generated_utc` 与 zip 条目时间戳（条目
  属性与顺序固定）；`.cargo/config.toml` 为两个 windows 目标加 `link-arg=/Brepro`（消除
  MSVC 链接器嵌入 PE 头与调试元数据的链接墙钟——第四产物时间源，v0.1.0-m0.2 同提交两次
  CI 运行实测发现并修复）。发布物版本戳语义改为"源码对应的提交时刻"（2026-10-23
  "版本号 = 构建墙钟"的发布路径修订，本地开发仍为墙钟）。范围：跨环境（不同机器/镜像）
  字节一致明确不支持（Python zlib 压缩流与 MSVC 工具链差异，已在 ADR-047 §1 实测归因）；
  发布页 `.sha256` 仍是权威校验和。
- Windows ARM64 构建交付目标（2026-10-28，ADR-043，用户指令）：新增
  `aarch64-pc-windows-msvc` 发布包 `whcode-<version>-windows-arm64.zip`（与 amd64 包同批
  发布，各带 `.sha256`）；`ci.yml` 平台矩阵新增 `windows-arm64` 条目（`windows-11-arm`
  原生 runner：全量测试、SSH 回环、运行时门控与依赖记录），Windows x64 lane 增加 ARM64
  交叉 `cargo check --target`；`build-release.yml` 矩阵改为 debug/amd64、release/amd64、
  release/arm64 三档。预编译 DuckDB 改为 `vendor/duckdb/<版本>/<三元组>/` 多架构布局，
  `fetch-duckdb.py --target`、`package-windows.py --target`、`check-release.py --arch`
  随之扩展。**验收口径不变**：Windows 11 amd64 仍是唯一验收目标，ARM64 结论只作平台证据。
- ⚠ **BREAKING**：Windows 下无子命令启动默认进入 **GUI**（`whcode` 等价 `whcode gui`），
  不再默认进入 TUI（2026-10-25，T203）。显式入口：`--tui` 终端界面、`--cli` 命令行、
  `--gui` 与 `gui` 子命令等价；其他平台无参数仍进 TUI。选择器可放在子命令前后，两个不同
  选择器同现按用法错误退出 2；没有可用控制台（CI／管道）时不启动界面，打印提示后退出 0。
- 版本标记修正（2026-10-25，用户批准）：删除不符合约定的正式 Release `v0.1.0`
  （0.1.0 属 M0 工程基线、非发布版本，且 M0 仅按 ADR-016 有条件通过），改为预发布标签
  `v0.1.0-m0` 指向当前提交；预发布标识在 M0 强制验收全部通过后移除。
- ⚠ **BREAKING**：`serve --web` 默认启用入站 TLS 与工作区证书库（2026-10-25，
  add-tls-certificate-store，ADR-040／D-20）。默认协议由明文 `http` 改为 `https`
  （每工作区 ECDSA P-256 自签证书，SAN 含 `localhost`／`127.0.0.1`／`::1`／本机名／
  绑定 IP，397 天，到期前 30 天告警），Origin 允许集 scheme 同步改为 `https`。
  新增三档 `--tls <builtin|external|off>`（参数 > 工作区设置 > 默认 `builtin`，
  参数按次覆盖、不回写设置；`off` 为显式明文回退，不被静默改回）。
  新增工作区证书库 `<workspace>/.whcode/certs/`（设置、可编辑内置 Mozilla 根证书副本、
  追加根、种子清单、入站证书链与未加密私钥）、`/settings/certs` 页面与 `/api/certs/*`
  （含同步阻塞 ≤ 2 秒的连通性校验）、能力 `settings.cert.read`／`settings.cert.write`
  与 `tls_config_change` 审计；出站信任两档 `builtin`（默认）／`system`（操作系统证书库），
  两档都叠加 `ca-extra.pem`，任何档位都不关闭链与主机名校验。
  **迁移**：依赖 `http://` 的脚本改用 `https://`（自签证书需 `-k` 或先信任指纹），或显式
  `--tls off`；入站档位与证书变更需重启 `serve --web` 生效。
  **依赖**：新增 `rustls`（直接依赖）、`rustls-native-certs`、`rustls-pki-types`、`rcgen`、
  `x509-parser`、`sha2`（单一 `ring` 提供者，零 `unsafe`）。
  **代价与不承诺**：自签证书默认不被浏览器信任；私钥明文落盘依赖 OS ACL；`system` 档位
  不复用 Windows 吊销与策略；不承诺公网暴露、mTLS 与多用户隔离。真实浏览器握手证据
  **未取得**（归属 T192）；资源证据见 ADR-042（DuckDB 2.0 线程超预算，方案 A 已实施收敛，线程项仍未通过）。
- CI 补齐可机器读取的证据（2026-10-26）：BSD 判据以公开 check-run 注解公布
  `store_gate`／`ssh_loopback_gate`／`process_group_gate`，并写 `*-progress.txt` 阶段证据；
  Windows 侧新增 SSH 回环、TLS 专项与 Playwright tablist 键盘／RTL 验收；长跑改为 release
  剖面并只在 `artifacts/long-run-request-*.md` 变更时触发，摘要以公开注解公布。
- 真实 LLM Provider 核心落地（2026-10-25，T162—T166，ADR-031）：新增阻塞式 `ureq`
  HTTP 客户端（有界流式读取、chunk 序号归一、传输快照、时限／重试／取消）、剖面鉴权
  注入与端点加固、出站信任注入；`http` feature 默认关闭，网络依赖只进装配根。CUI 与 Web
  的 `llm run／cancel` 已接线，TUI／GUI 仍为 `Reserved`（T167 未收口，不宣称四端可用）。
  端到端回环证据 `artifacts/llm-e2e-2026-10-25.md`（不访问公网）；`cargo test -p whcode-llm
  --features http` 77 项通过。
- BSD 回环验收与进程组（2026-10-25）：`whcode-platform` 实现 Unix 进程组终止
  （`configure_child_process_group` + `term_process_group`，零新增依赖），`whcode-runtime`
  补零残留测试；`whcode-ssh` 新增客户端与服务端 `bsd_*_acceptance_loopback`，FreeBSD job
  运行并上传 `ssh-acceptance-freebsd`。BSD 运行证据待 CI；V-004 仍按 ADR-013 延后。
- GUI 线程预算偏差登记（2026-10-25，ADR-042 提议）：DuckDB 2.0.0 单连接打开约 80 线程，
  GUI 空闲实测 88 线程／297—330 句柄，超过 ADR-027 的 24／256；归因为引擎而非界面改动，
  现行预算不放宽，处置待用户批准 ADR-042。NFR-201 主进程空载 RSS 35.69 MB 达标。
- `llm.run`／`llm.cancel` 四端接线收口（2026-10-26，T167，ADR-032 修订记录）：TUI
  新增 `L`／`C` 键位（剖面名→提示词两步输入、请求 ID 输入），经 `llm_provider::TuiLlmExecutor`
  真实编排（与 CUI/Web 同一 `whcode_app::llm` 入口，连接与 TUI 写执行器共享同一条
  工作区连接）；登记表 TUI 格翻转为 `Implemented`，矩阵 feature-parity.md 同步——
  三端 CUI／TUI／Web 均已接线（同步等待完成，同语义），GUI 保持 `GUI_READ_ONLY_V1`
  排除。`cargo test -p whcode-tui` 69 项（含键位测试 2 项）。
- GUI 线程配置收敛（2026-10-26，ADR-042 修订记录，方案 A 批准实施）：判别实验确认
  DuckDB 2.0.0 受支持的 `async_threads`（默认 64）是线程膨胀主因（原“与 db.threads 无关”
  结论不完整）；产品装配、长跑夹具与基准夹具同步 `SET async_threads = threads`。
  GUI 空闲线程 88→28、句柄 297—330→233、私有内存 61→约 44 MB，NFR-210 同夹具复测
  4 625.5 条/秒达标（`artifacts/adr042-bench-after-async-threads.json`）。FR-712 线程判据
  仍未通过（28 > 24，剩余为 worker 池），后续路径：全局 threads=1（须 NFR-210 同夹具
  复冻结）或进程拆分（新 ADR）。
- 四端界面现代化（2026-10-25，modernize-ui-visual-language，ADR-041）：确立终端族（CUI／TUI）与
  应用族（Web／GUI）两套视觉语法，不引入第三色值、渐变、阴影与新增动效。
  TUI 改为三区布局（顶栏／左导航栏 + 列对齐主面板／键位提示栏，CJK 按 2 列截断）；CUI 的
  `session list/show`、`task show`、`context budget`、`spec status` 改为对齐记录／表格（交互终端
  下允许粗体/弱化，**不输出颜色**；管道与 `--json` 恒为纯 ASCII）；Web 改为吸顶应用栏 + 七项
  ARIA tablist（roving tabindex）+ 内容/元信息两栏 + 常驻状态栏；GUI 改为可点击侧栏 + 顶栏
  会话 + KPI 条 + 底部状态栏（仍只读、无定时器）。前端预算脚本新增 DOM 起始标签 ≤ 2 000 与
  `innerHTML`/`outerHTML`/`insertAdjacentHTML` 禁令（FR-618）。
  **展示层变更**：CUI 人类可读文本、TUI 布局与标题/页脚词条随之改变；`--json` 字段集合、错误
  信封四字段与退出码**不变**。依赖人类可读文本抓取的脚本请改用 `--json`。
  证据：`artifacts/ui-modern-2026-10-25/`；主账本新增 T193—T202 与 Check 289—298。GUI 资源采样、
  主进程 RSS 与 Web 运行时冒烟因并行未提交的 cert/TLS 工作阻塞，登记为未取得。
- 构建与发布迁入独立矩阵 CI（2026-10-25，ADR-039）：新增 `.github/workflows/build-release.yml`
  （`profile: [debug, release]`）；debug 做工作区锁定构建与依赖记录，release 做发布构建、打包
  安装验证、SHA256、TUI/Web 冒烟并上传既有 artifact 名称；tag `v*` 时创建/更新 GitHub Release
  （版本不一致即失败、预发布标 prerelease、重跑 `--clobber` 幂等），写权限仅限发布作业。
  `ci.yml` 删除构建与发布步骤，保留门禁与测试。主账本新增 T183/Check 273，已由
  build-release run 40153942736 与 ci run 40153942697（五平台 + measurements 全绿）取证通过；
  tag 发布路径未实跑。
  迁移：`deps-windows`、`whcode-windows-amd64`、`install-verify-windows-amd64`、`smoke-evidence`
  改从 build-release 工作流获取，名称不变。
- ⚠ BREAKING：CUI 保留命令（`run`、`events show`、`spec next/validate/analyze/converge/force-advance`、
  `runtime status/enable/disable/install`）退出码由 50（invalid_state）改为 **55（not_supported）**；
  依赖退出码的脚本按 55 判“端未提供”，按 404 判“路由不存在”（FR-620，ensure-frontend-parity）。
- 四端对等与差异登记（2026-10-24，ensure-frontend-parity，ADR-032/D-17）：
  `whcode-app::surface` 能力登记表（54 条 SSOT）+ 四端 surface_map 清单 + 对等契约门禁
  `tests/contract/test_frontend_parity.rs`（三向一致 + 负向自测）；统一 not_supported 语义
  （CUI 退出码 55、Web 501 JSON、未登记仍 404）；CUI 补 `context budget` 与 `runtime list`；
  Web 补 `/api/context`、`/api/diff`、`/api/events/export` 与四套主题（data-theme +
  ?theme=/选择器运行时切换）与语言链（ui.locale → WHCODE_UI_LOCALE → LANG/LC_ALL → zh-CN）；
  TUI 补写操作（n/m/d/A、R/X、p 两段确认，经 `whcode-app::commands` 门面）、`t` 四主题循环
  与游标在线可见（≥1000ms 低频 O(1) 检查，推进才重绘）；GUI 只读面契约断言。
  证据：`artifacts/frontend-parity-2026-10-24/`。
- 收口（2026-10-24，T181）：
  1. `docs/matrices/feature-parity.md` 已由 14 行手工快照重写为 54 行逐条受校验表（GUI 列
     齐备），门禁逐行比对状态、类别、效果、理由码与里程碑，并新增篡改符号／理由码的负向
     用例——原“GUI 列逐行重写”的退出条件达成。
  2. FR-619—625 已转正主账本：`tasks.md` 登记 T168—T181、`checklist.md` 新增 Check 264—271；
     计数 FR 93／任务 181／Check 271，`check_counts.py` 与 `traceability.py` 全绿——原
     “编号冲突待协调”的退出条件达成（T162—T167 为并行变更 `add-real-llm-provider` 的
     占位任务，未勾选）。
- 收口（2026-10-25，T182／ADR-038）：`session.archive` 的确认与审计口径冲突关闭
  （原“已知未完成第 3 项”）。裁决为**不放宽契约**：四端统一“预览→确认”（CUI 默认 dry-run、
  `--yes` 执行；TUI `A` 两段确认；Web 缺 `confirm` 只预览；GUI 只读排除），
  `whcode-store::set_archived` 在同一事务写 `session_archive` 审计（`detail_json` 含方向、
  更新数与会话清单，100 条截断）。TUI 暂无反归档键，已在 ADR-038 登记为已知不对称。
  ⚠ BREAKING：`whcode session archive` 由“直接执行”改为“默认预览、`--yes` 执行”
  （与 `session delete` 同构）；依赖旧行为的脚本须显式加 `--yes`。证据：
  `ADR-038-session-archive-confirmation-and-audit.md`、Check 272、`cargo test` 相关用例。
- 收口（2026-10-25，ADR-031 补录）：`add-real-llm-provider` 的网络依赖与凭据边界入库
  `docs/adr/ADR-031-llm-provider-network-and-credentials.md`（原“已知未完成第 4 项”）。
  记录：网络依赖只进装配根、`http` feature 默认关闭、默认不联网、`https` 强制与回环明文、
  凭据仅环境变量／仓库外文件、超时 10 s／120 s／300 s、流帧有界、重试策略（未实现）。
  同时修正：`http/protocols.rs` 与 `http/errors.rs` 原未在 `mod.rs` 声明（14 个测试从未编译），
  已接线并修复一处字符字面量类型错误；注释中引用的幻影 FR-209—214 与 `161.x／162.x`
  小节号改为既有 FR-201／203／206／207／208／616、NFR-213／215 与 ADR-031。
  实测：同一代码三次 release 构建（带／不带 feature／还原）均为 21 124 096 字节，
  当前无调用点故体积增量为 0；接入调用点后必须重测。**进展（2026-10-25）**：T162—T166
  已交付（真实 HTTP 客户端与回环端到端）；T167 的 CUI／Web `llm.run`／`llm.cancel` 已接线，
  TUI／GUI 仍为 `Reserved`，四端完整可用未完成。

- DuckDB 2.0.0 升级与功能适配（2026-10-24，ADR-034）：**引擎 2.0.0 LTS + crate 1.20000.0，事件载荷改 VARIANT**。
  - 官方 Windows 资产改为 `duckdb-shared-libs-windows-amd64.tar.gz`（SHA256 `2b831691…b95f`）；
    `fetch-duckdb.py` 改为先校验 SHA256、再拒绝绝对路径与 `..` 的 tar.gz 安全解包；
    `.cargo/config.toml` 指向 `vendor/duckdb/2.0.0`，7 个清单精确锁定 `=1.20000.0`。
  - schema 升至 v6：`events.payload_json` 由 `TEXT` 改为 `VARIANT`，写入走
    `CAST(? AS JSON)::VARIANT`，读取/导出走 `CAST(CAST(payload_json AS JSON) AS VARCHAR)`；
    `stage_artifacts.content` 与原型表 `m0_events` 保持 `TEXT`。
  - `session_overview` upsert 改为 `MERGE INTO`；数据修改 CTE 经 A/B 实测无收益，不采用。
  - 工作区连接显式关闭 `autoinstall_known_extensions`、`autoload_known_extensions`、
    `allow_community_extensions` 与自定义扩展仓库，避免查询触发联网安装；`enable_external_access`
    保留以支持导出/导入。
  - 项目处于开发期，**不承诺读取 1.5.6 旧库，不提供降级或格式转换**；旧库删除或重建即可。
  - 审计证据见 `artifacts/duckdb-2.0-audit-2026-10-24.md`；BSD 平台结论不变（继续阻塞/延后）。

- ⚠ BREAKING：测试统一入口与 CI 合并（2026-10-25，ADR-037）：
  - `platform-matrix.yml`、`stability.yml`、`action-version-audit.yml` 合并为
    `.github/workflows/ci.yml`：门禁、五平台矩阵、全部功能测试、短测量、TUI/Web 冒烟、
    `workflow_dispatch` 桌面入口与每周 action 审计集中在一份工作流；
    5.5 小时长跑保留在 `.github/workflows/long-run.yml`。
  - EOL 审计、NFR-201/202/205/206 真实进程测量与测试进程资源测量迁入
    `.github/scripts/`（`check-eol.py`、`benchmarks/measure-nfr-budgets.py`、
    `benchmarks/measure-resources.py`）；`local/` 不再保留同名副本，只作复现与排障。
  - 测量 job 只在 push `main` 与手动触发运行（PR 不跑），每周一 schedule 只做 action 审计；
    桌面型测试按 headless／虚拟显示口径记录，不替代 V-003 真实控制台人工证据。
  - **CI 已实跑**：首轮 run 40118942627 的 Windows job 通过；`measurements` 因共享 runner 上
    NFR-207/209 未达冻结预算、NFR-202 夹具缺 Python `duckdb` 而失败。处置：性能预算与测试进程
    RSS 移入 `perf-budget`（仅 `workflow_dispatch` 且 `perf=true`，预算值不变）；`measurements`
    安装 `requirements-dev.txt` 中的 `duckdb==2.0.0`。run 40126145778 全绿（五平台与
    `measurements`；NFR-201 中位 31.27 MB、NFR-202 中位 35.51 MB、NFR-203 增量 1.31 MB）。

- 证据补遗（2026-10-25，T181 复核）：
  - 真实 Firefox 运行时三取证以独立脚本、独立夹具（`seed_web_demo` 的 6 条事件）重跑，结论与
    2026-10-24 首轮一致：hidden 3 s 内 API 请求 0／DOM 变更 0／rAF 0；还原后单批 6 次请求全部
    200、其后 ≈1 Hz 续拉；两轮挂起／恢复后游标恒为 6、事件恰 6 条；reduced-motion 两分支与禁用
    GPU 20 轮最小化／还原下 `canvas`／`getContext`／rAF 均为 0。证据
    `artifacts/web-runtime-2026-10-25/`。
  - 远端 CI 逐 job 结论归档至 `artifacts/frontend-parity-2026-10-24/close-gates.md` §8：
    run 40118942627（五平台 success，仅 `measurements` 失败）与 run 40126145778（全绿）；
    补遗提交 `eaced3f` 自身由 run 40129632158 覆盖（页面状态 Success）。
    该 run 的 NFR 读数标为二手（匿名读取 job 日志受限），不作为本机复核结论。

- ⚠ BREAKING：JSON 语义列全面 VARIANT 化（2026-10-24，ADR-036，schema v7）：
  - `tasks.acceptance_json`、`stage_artifacts.refs_json`、`session_overview.by_stage_json`、
    `checkpoints.delta_json`、`checkpoints.artifact_refs_json`、`audit.detail_json` 由 `TEXT`
    改为 `VARIANT`；`stage_artifacts.content` 与原型表 `m0_events` 仍为 `TEXT`。
  - 写入统一 `CAST(? AS JSON)::VARIANT`，读取/导出统一 `CAST(CAST(col AS JSON) AS VARCHAR)`；
    `delta_json` 64 KiB 与 `artifact_refs_json` 条目上限仍在文本层校验，非法 JSON 与顶层
    JSON `null` 入库前拒绝。
  - **文本归一化**：VARIANT 读回会去除多余空白并排序对象键，导出与断言按 JSON 值比较，
    不再保证逐字节相同；开发期不保留 v6 双读。
  - `MERGE` 复核：仅 `session_overview` 为 upsert，其余 `ON CONFLICT DO NOTHING` 不改为 MERGE；
    数据修改 CTE 在 `purge` 场景 A/B 仍无收益（+0.83%），不采用。
  - `variant_type`/`variant_keys`/`variant_extract` 进入健康断言与迁移验证；动态键路径
    （事件摘要、redaction）经评估不采用 SQL 下推。证据：`artifacts/duckdb2-usage-2026-10-24.md`。

- 稳定性窗口改为 5.5 小时（2026-10-24，ADR-033）：**24/72 小时长跑改为 5.5 小时以适配 GitHub CI**。
  - 新窗口 19 800 秒、300 秒采样（约 66 样本），NFR-204 仍按 ≤ 0.5 MB/h 判定；24/72 小时
    深检保留为可选 self-hosted／本机运行，不再作为 CI 门槛。
  - 新增 `stability-long-run`（仅手动触发，`timeout-minutes: 355`＝330 min 长跑 + 25 min 准备），
    产物 `timeseries.jsonl` 与 `run.log` 归档；`run_stability.sh` 默认窗口与采样间隔同步为 5.5h／300s。
  - 文档与账本同步：spec §11.5/NFR-204/SC-020、T116、Check 116、compatibility、README、
    ADR-025 修订记录；**不得表述为 24/72 小时已验证**。

- 目录结构整理批次（2026-10-24）：**`script/` 更名 `local/` 与隐藏目录登记**（用户批准，不计成本）。
  - `script/` → `local/`：与 `.github/scripts/`（入库门禁）消除“script vs scripts”一字母混淆；
    本地目录语义与 repository-layout 五类分层的“本地”一致。引用同步：AGENTS、repository-layout、
    五份指南、CONTRIBUTING、spec 活引用、`.gitignore`、`test_layout.rs` 白名单与 `.py` 豁免条件、
    `clean.py`（`--scope local`，`script` 保留为别名）。历史取证文字中的 `script/` 引用按当时事实保留。
  - `.specify/` 登记：spec-kit 工具配置入库（21 文件）此前无归属，repository-layout 增
    “治理（工具配置）”行；隐藏目录不入布局门禁检查范围（既有设计）。
  - `.screenshots/` 显式进 `.gitignore`（与既有本地工具目录规则对齐）。
  - 合并并行会话在更名交叠期新建的脚本（`fetch_ci_logs.py`、`check_netbsd_run.py` → `local/`）。

- CI 测试与 action 版本铁律（2026-10-24）：**BSD 测试随 main 自动运行；action 固定最新 SHA**。
  - FreeBSD/NetBSD 工作流增加 `push: branches: [main]`：在 vmactions VM 中真实运行
    `cargo check`、不依赖 libduckdb 的包级测试与依赖记录；DuckDB 原生构建保留为显式
    Go/No-Go（失败属 R-01 证据，不使 job 变红）。OpenBSD/DragonFly 无原生测试，保持手工占位。
  - ADR-013 修订记录：BSD CI 自动运行**不改变验收范围**——唯一验收目标仍是 Windows 11 amd64，
    BSD 结果只作平台证据，不进入验收结论。
  - 铁律（AGENTS.md §5.1）：所有 `uses:` 必须为**最新 release 版本 + 固定 commit SHA**
    （`owner/repo@<40 位 SHA> # vX.Y.Z`）；`ci/action-versions.json` 为唯一事实源，
    `ci/check-actions.py` 每次 CI 离线校验，新增每周 `action-version-audit` 联网复核；
    Dependabot 的 Actions PR 必须同步清单。
  - 当前版本：checkout v7.0.1、upload-artifact v7.0.2、freebsd-vm v1.5.9、netbsd-vm v1.5.2。

- Check 47 收口批次（2026-10-24）：**修正数据集夹具缺陷并复跑 NFR-203**。
  - `tests/fixtures/gen_dataset.rs` 以绝对事件数作循环上界，同一库第二次生成时第二个会话
    实际 0 条事件；ADR-025 原“0.32 MB（10 万事件 ×2 会话）”的测量因此失真。改为
    `seq_base + events` 作为上界。
  - `nfr203` 直接断言 10 万事件会话的加载面：尾部恒 `SWITCH_TAIL_LIMIT`、`tail_complete=false`、
    游标覆盖最新事件、尾部只属目标会话、概览行存在；复跑最大 RSS 增量 **3.18 MB ≤ 10 MB**
    （同日另一复跑 1.16 MB），通过。ADR-025 冻结值不变，仅更正证据并留修订记录。
  - 证据：`artifacts/check47-switch-100k-2026-10-24.txt`。账本 Check 47 转入通过（248/12）。

- 长跑采集批次（2026-10-24）：**进程资源快照与 T116 单进程采集器**（24 h 数据未落地，任务不勾选）。
  - `whcode-platform` 新增 `ProcessStats`／`process_stats`（CPU 累计、线程、句柄；非 Windows
    返回结构化错误，不以 0 冒充），供 §11.5 时间序列采样。
  - 新增 `tests/stability/test_stability_run.rs`：单进程持续负载 + 自采样逐行追加 JSONL，
    覆盖 RSS／CPU／线程／句柄／库与 WAL 大小／事件缺失／failed 任务／三类延迟；写者队列深度
    与最老队列项年龄显式 `null` 并附原因。`run_stability.sh` 重写为带时间戳目录与 30 s 冒烟。
  - 上一轮 24 h 长跑作废（短命负载进程、RSS 斜率不可归因、中途编译失败导致产物未落盘）。

- NFR-204 本机长跑序列（2026-10-25）：**12 h 单进程采样落盘并出斜率分析**（T116／Check 116 仍不勾选）。
  - 序列落盘于 `artifacts/stability-24h-20261024-150204/`：12 个样本、单 PID 12392、间隔 3 600 s、
    debug 构建；RSS 斜率三口径均为负（−0.611／−1.855／前 5.5 h −3.954 MB/h），线程恒 17、
    句柄恒 172，`event_missing` 恒 0，清理路径生效（插入 1 536 600／删除 1 500 100）。
  - 进程在 12.0 h 处被外部终止、无 `kind:"summary"` 收尾行，采样间隔不合 ADR-033 的 300 s，
    且为 debug 构建——仅作候选证据（`nfr204-analysis.md`）；`long-run.yml` 的 5.5 h
    CI 运行仍是启用门槛（ADR-037）。

- 四端对等立项（2026-10-24）：**ADR-032 与预留需求 619—625、统一 `not_supported` 语义**。
  - 新增 ADR-032（四端对等五维判据、四态登记、差异理由码），ADR-009 追加修订记录，
    D-17 入决策台账；spec 预留编号 619—625（不计入 FR 计数，正式登记由收口任务统一更新）。
  - 契约同步（交付前不生效）：登记表为单一事实源；预留／排除能力返回 `not_supported`
    （CUI 退出码 55、Web HTTP 501），未登记路径仍 404；`context budget` 命令与 Web 501 语义、
    事件导出复用同一脱敏路径写入契约；CHANGELOG 标注 50→55 属 BREAKING。
  - 文档标注目标口径（TUI §8、Web 部署 §4.1、ui-design §3.1、矩阵 GUI 列占位），**交付前
    不得按已实现使用**；实现任务 T168—T181 尚未落地。

- Web 运行时取证批次（2026-10-24）：**真实 Firefox 最小化/动效/GPU 三项取证，修复挂起机 focus 竞态**。
  - `suspend.js`：`focus` 事件在 `document.hidden` 为真时不再恢复挂起状态（可见性优先）。
    实测最小化时可能残留 `focus`，旧逻辑会把状态机误拉回 Active，hidden 期间继续轮询（FR-617）。
  - 取证：本地 Firefox 155 + Marionette 驱动真实窗口（Playwright/Chromium 默认参数强制页面
    保持 visible，无法取证）：hidden 3 s 内 `/api` 请求 0、DOM 变更 0、rAF 0；还原后单次恢复
    批次（tasks/status/events 各 1 次，均 200）并按 ≈1 Hz 续拉；两轮挂起/恢复后游标恒为 3、
    事件日志恰 3 条无重复无丢失；`prefers-reduced-motion` 两分支实测（reduce 无 150 ms 定时器；
    no-preference opacity 0.6 → 清空且恰一次 150 ms）；禁用 GPU/软件渲染下 20 次最小化-还原全部
    hidden→visible，canvas 0、getContext 0、异常 0。
  - 证据：`artifacts/web-runtime-2026-10-24/`（report.json、visible.png）；临时脚本与浏览器
    配置目录已清理。账本：Check 220/227/228 转入勾选，现状 **247 通过 / 13 未通过**。
  - 说明：取证时并行会话的未提交 `whcode-llm` 改动使 `cargo build` 不可用，前端三资产经同源
    代理取自仓库源码（API 与页面仍由真实后端服务）；构建恢复后须用真实二进制复跑。

- Check 97／229 收口批次（2026-10-24）：**SSH 客户端默认无任意 Shell；Web 无 JS 只读快照服务端直出**。
  - SSH 客户端（Check 97）：`SshTarget` 新增 `allow_shell`（默认 `false`）；校验阶段默认只放行
    首 token 为 `whcode`／`whcode.exe` 的远端命令（含绝对路径与 Windows 路径），其余命令返回
    `PermissionDenied` 且不产生任何登记副作用；显式 `allow_shell` 是独立权限，意图登记携带
    `allow_shell`、结果 `note` 前缀 `shell-granted（显式授权）：` 供审计追溯（FR-608/611）。
  - 服务端回归修复：`authorized_keys` 行带注释、客户端上送公钥不带注释时，`PublicKey` 结构体
    相等（含 comment）会误拒真实客户端（系统 OpenSSH 冒烟实测命中）；改为按公钥指纹
    （`SHA256`，只覆盖公钥数据）比较，并补回归测试。
  - Web（Check 229）：`assets::render_index` 把只读会话快照（最多 50 个）服务端直出到模板内唯一的
    `<!-- WHCODE_INITIAL_VIEW -->` 的 `<noscript>` 块；数据库文本统一 HTML 转义，查询失败渲染
    可见错误而非“错误的空”，空列表显式写“当前没有会话”，超过上限（50）写明“列表已截断”
    （多取一条区分“恰好 50 条”与“被截断”，不静默切掉）；`GET /` 与 `/index.html` 经
    `index_response`（`list_sessions` 只读能力）返回 `no-store` 页面；**认证模式不注入数据**
    （ADR-022：浏览器导航无法携带 Bearer 头，直出会绕过认证边界），只给无 JS 说明。
  - 证据：`cargo test -p whcode-ssh` 45/45（新增 3 例命令策略与授权标记）、web 39/39（新增 6 例
    直出、转义、截断与认证边界）、bin 43/43（新增 3 例装配）；真实 `serve --web` 实跑 7 项判据全部为 true
    （`artifacts/web-nojs-check-2026-10-24.json`，含端口、判据与 `<noscript>` 片段；临时脚本按规则回收）。
    账本：Check 97/229 转入勾选，现状 **244 通过 / 16 未通过**。

- SSH 服务端批次（2026-10-24）：**M7 服务端落地（russh）+ 主机密钥信任接入 + 四处测试缺陷修复**。
  - 服务端：`whcode-ssh` 新增 `server.rs`／`policy.rs`／`adapter.rs`／`hostkey.rs`／`trust.rs`，
    基于 `russh 0.64.1` 实现**仅 Ed25519 公钥认证**（`<workspace>/ssh/authorized_keys`）、
    每连接独立会话上下文（通道上限 8）、pty-req／window-change 记录并随命令传给执行器、
    断连经 `Drop` 归还计数并写审计；`serve --ssh` 默认仅回环、OS 分配端口，启动打印实际地址。
  - 受限入口：`sessions`／`session show|new`／`tasks`／`task` 白名单，默认拒绝任意 Shell；
    Shell 需 `allow_shell` 开关**且**调用者在 `shell_actors`，判定带能力校验与结构化错误码，
    拒绝与授权均写审计（FR-608）。交互式 PTY 不实现（属 M8），`shell_request` 一律拒绝。
  - 单写者：装配根只 `open_workspace_db` 一次并把连接移交单线程执行器（有界队列 64 +
    一次性回执 + 30 s 超时）；3 个并发会话全部经同一连接落库（ADR-005／C-03）。
  - 主机密钥：客户端接入 known_hosts 风格信任文件（TOFU，首次记录并打印指纹供人工核对，
    不匹配结构化拒绝且不改写信任文件，不可读 fail closed），结清 ADR-005 未决条件③。
  - 测试缺陷修复（复核发现，均为未提交实现内的缺陷）：① known_hosts 断言在第三次覆写后
    才取样，改为在首次/二次连接后取快照；② 有界队列测试在单线程运行时无法确定性填满且
    失败时闸门未开导致 `Drop` 永久 join——补 `yield_now` 与闸门守卫；③ 输出截断用例
    `copy_from_slice` 长度不匹配；④ `session new` 标题被 `split_whitespace` 折叠内部空格，
    改为“取剩余整行、只去首尾空白”。
  - 账本：T096—T098 勾选、T099 维持未取得（BSD 延后）；Check 98（单写者）与 182
    （默认无 Shell，服务端口径）转入通过，Check 97 登记客户端任意命令面缺口；
    `cargo test -p whcode-ssh` 41/41 通过。

- 流程收敛批次（2026-10-24）：**第四类（流程性）与第六类（文档治理）八项 Check 收敛**。
  - 转入通过 5 项：Check 90（Web 写命令交付取证，api 33 测试）、184（提交粒度等价抽查，
    发现并修复 2 处能力矩阵缺口——GUI 主题与 `font` 命令）、217（批次级完成记录等价 §15）、
    249（check_counts.py 修复声明解析后全绿）、124（quickstart.md 全流程实跑，
    `artifacts/quickstart-run-2026-10-24/`：M0—M2/M6/M8 通过，未实现步骤显式失败取证）。
  - 注记更新维持未通过 3 项：Check 3（无 git 远端，CI 运行证据结构性阻塞）、109（Windows
    残留断言已补，BSD M7）、134（三 ADR 部分批准的未决项均有处置，字面转正待 BSD）。
  - 门禁修复：`ci/check_counts.py` 只断言时间序最后声明（复核记录历史口径不再误报）；
    `feature-parity.md` 同步 Web 写命令与 theme/font 条目；process 指南计数口径 158/260。
  - 现状：**240 通过 / 20 未通过**（Check 1—260）；未通过项全部为外部条件阻塞
    （BSD/M7/长跑/预算冻结流程），无实现缺陷。

- 工作区清理机制批次（2026-10-24）：**`ci/clean.py` 四 scope 清理机制**（T158，Check 260）。
  - 理论依据：构建可再现性（target/ 可由源码+锁文件再生，删除零信息损失）、源码—产物
    分离、白名单优于黑名单、不可逆操作门禁（dry-run 默认 + `--apply` 显式）、证据留痕。
  - 范围：`target`（cargo 产物）／`temp`（系统 TEMP 下 `whcode-*` 测试残留）／
    `stray`（仓库内垃圾模式，git 跟踪文件恒排除）／`script`（一次性脚本与运行日志）。
  - 保护：凭据（`sensenova.env`）、AGENTS 验证链工具、sacred 考据数据与配套脚本在
    `KEEP_ALWAYS` 显式保留；30 分钟活跃窗口与 cargo/rustc 进程检测跳过使用中对象。
  - 规则更新（用户 2026-10-24）：`script/` 由“保留不清理”改为“按机制回收”。
  - 首次执行释放 ≈55.9 GiB（target 50.7 GB＋TEMP 残留 5.2 GB／2516→30、
    script 150→11）；执行中修复两处实现缺陷（Python 3.14 rmtree onerror 弃用、
    证据报告键缺失），过程记录 `artifacts/clean-run-20261024-summary.json`。

- 安全加固复查（2026-10-24）：**T123 五域复查通过**（任务书 §10、Check 123）。
  - 复查记录产物 `artifacts/security-review-2026-10-24.md`：脱敏默认、权限白名单、
    最小环境、审计保留、Web/SSH 边界逐域以“文件:行号 + 测试名 + 失效条件”取证。
  - 最小环境（新）：`worker::apply_minimal_env`——子进程默认只继承 8 项系统级白名单
    （PATH/PATHEXT/SystemRoot/SystemDrive/COMSPEC/TEMP/TMP/NUMBER_OF_PROCESSORS），
    `env_clear` 禁用完整继承，凭据剥离二次过滤；真实子进程测试验证非白名单父变量
    （USERPROFILE）不继承、`WHCODE_` 前缀永不透传。
  - 审计保留（新测试）：审计行不随会话 TTL 清除、导出不携带 audit 表
    （§10.3 默认策略成文）；管理员显式保留策略配置面显式标记未实现。
  - 显式登记缺口 2 项（OS 级配额、审计保留配置面），均写明退出条件。

- 性能测量战役（2026-10-24）：**T115 按冻结预算完成热点测量与优化**（ADR-025、NFR-201—210）。
  - 测量：`tests/stability/test_nfr_budget.rs`（NFR-207/209 常规回归；NFR-203/206 为
    `#[ignore]` 测量夹具）+ `script/measure_nfr_budgets.py`（真实产品进程采样，
    纪律字段齐备）→ `artifacts/nfr-budget-report.json`。七项候选预算全部实测达标：
    NFR-201 空载 RSS 23.12 MB、NFR-202 活跃 27.33 MB、NFR-203 切换增量 0.32 MB、
    NFR-205 TUI 空闲 CPU 0.0%、NFR-206 限速流式 0.682%、NFR-207 切换 p95 12.9 ms、
    NFR-209 单页 p95 19.8 ms（数值与样本数见 ADR-025 冻结表）。
  - 优化：NFR-209 同夹具实测 p95 40.8 ms 超标（预算 30 ms）——EXPLAIN 热点定位为
    DuckDB 多列 top-k 的 SEQ_SCAN（ART 索引不参与）；`events_page` 改为两阶段
    （仅 seq 窄列 top-k + seq 范围整行回取），同夹具 p95 40.8 → 19.8 ms，逐行语义
    一致（104 项 store 测试与翻页契约回归通过）。未修订预算。
  - 测量方法缺陷修正留档：NFR-205 需 FreeConsole/AttachConsole 到子控制台注入按键；
    NFR-206 须采样测试可执行文件本身而非 `cargo test` 包装进程（否则采到 0.0%
    歧义信号）。

- GUI 终端绿主题批次（2026-10-24）：**新增第四套主题 `terminal-green`**（T157，FR-713 修订）。
  - 色值：纯黑 `#000000` 底（OLED 兼容，可完全关断像素）+ 荧光绿 `#00FF00` 字
    （CRT P1 风格，2026-10-24 用户新增并确认“黑色兼容 OLED”）。
  - 配置体系统一四套：`whcode-tui::Theme` 与 `whcode-gui::GuiTheme` 均加
    `TerminalGreen` 变体（`ui.theme`／`WHCODE_UI_THEME` 可选值扩展；
    FR-612 的“TUI 三套”为下限）；GUI `t` 键循环扩为四套、四步回原位。
  - 判据口径修订（FR-713）：中间灰判定改为**无彩色真灰**（R=G=B 且亮度 32—223）——
    原亮度口径会把彩色前景的正文全部计入（`#00FF00` 亮度 ≈150）；单色前景三套的
    历史记录在新旧口径下数值一致，`terminal-green` 以截图只出现精确两色为主判据。
  - 探针加固：窗口句柄出现 ≠ 应用就绪——标题非空且连续两次采样一致才注入按键
    （修复冷启动时按键全部落空导致取证轮失败）；`ci/gui-probe.py` 四主题取证。
  - 证据：`artifacts/gui-run-2026-10-24/`（四主题截图、`t` 四步循环色值比对、
    资源采样、真灰占比）。

- 性能测量战役（2026-10-24）：**T115 按冻结预算完成热点测量与优化**（ADR-025、NFR-201—210）。
  - 测量：`tests/stability/test_nfr_budget.rs`（NFR-207/209 常规回归；NFR-203/206 为
    `#[ignore]` 测量夹具）+ `script/measure_nfr_budgets.py`（真实产品进程采样，
    纪律字段齐备）→ `artifacts/nfr-budget-report.json`。七项候选预算全部实测达标：
    NFR-201 空载 RSS 23.12 MB、NFR-202 活跃 27.33 MB、NFR-203 切换增量 0.32 MB、
    NFR-205 TUI 空闲 CPU 0.0%、NFR-206 限速流式 0.682%、NFR-207 切换 p95 12.9 ms、
    NFR-209 单页 p95 19.8 ms（数值与样本数见 ADR-025 冻结表）。
  - 优化：NFR-209 同夹具实测 p95 40.8 ms 超标（预算 30 ms）——EXPLAIN 热点定位为
    DuckDB 多列 top-k 的 SEQ_SCAN（ART 索引不参与）；`events_page` 改为两阶段
    （仅 seq 窄列 top-k + seq 范围整行回取），同夹具 p95 40.8 → 19.8 ms，逐行语义
    一致（104 项 store 测试与翻页契约回归通过）。未修订预算。
  - 测量方法缺陷修正留档：NFR-205 需 FreeConsole/AttachConsole 到子控制台注入按键；
    NFR-206 须采样测试可执行文件本身而非 `cargo test` 包装进程（否则采到 0.0%
    歧义信号）。

- 功能缺口清偿批次（2026-10-24）：**八项功能缺口一次性落地**（T110—T112、T114、T156/G-01、
  Check 179/199/209、导出缺陷），全部有自动化测试与实测证据。
  - 受限 JS 引擎（T110，ADR-029 修订 ADR-024 的 D-08 条款）：Boa 0.21.1 经
    `runtime-js` feature 接入，默认构建零依赖零开销（`cargo tree` 无 boa）；
    Node API 六名字显式“能力不足”拒绝桩；固定 ECMAScript 测试集 9 例；
    基准：release 28 579.9 次/秒求值、测试进程 RSS 15.1 MiB、feature 二进制 +11.7 MiB
    （默认构建零增量）、许可证 Unlicense OR MIT。
  - Python worker（T111）：stdin 单行 JSON 协议（请求 ≤4 MiB、输出 8 KiB 有界缓冲），
    `PythonEnv` 记录解释器绝对路径/版本/实现名/ABI/C 扩展探测；受限档位五项不支持项，
    文档写明“承诺边界不是沙箱”。
  - GitBash payload（T112）：`verify_payload` 四重校验复用；五类能力探测（管道/重定向/
    含空格路径/超时终止/目标命令）本机 bash 5.3.9 真实取证；V-304 集成用例补齐（T114）。
  - 检查点事务语义（T156，撤销缺口登记 G-01）：增量差分写入（delta ≤64 KiB、保留有界）、
    任务状态+事件+检查点同事务提交（失败三表全回滚）、按引用恢复加载元数据/产物/游标；
    新增 `Check 256—258`。
  - TUI Diff 视图（Check 179）：接 `converge_differences`，按任务状态诚实映射验收判定，
    GUI 共用同一 ViewSource。
  - WS 事件广播（Check 199/209）：有界队列 64 帧/客户端 + 单线程非阻塞多路复用（生产路径
    零新增线程）；慢客户端断开不停别人（正常客户端最大帧间隔 199—209 ms ≈ 一个轮询周期）；
    >1 MiB 帧拒绝只断发送者（close 1009）。
  - 导出缺陷修复（Check 124 登记）：`events export` 对不存在的会话返回 `not_found`
    （退出码 60 + 可操作指引），不再与“存在但无事件”混淆。
  - CLI：`db checkpoint [--threshold|--autocheckpoint]` 与 `db restore --from --yes`
    接线（应用层判权、空库保护、审计沿用既有机制）；灾备指南 §8 同步改写。

- 内置字体批次（2026-10-23）：**四类 OFL 字体本地化与 GUI 分段渲染**（T153／T154，ADR-028）。
  - 资产：Inter（英语比例）、Noto Sans CJK SC（简中比例）、JetBrains Mono（英语等宽）、
    Sarasa Mono SC（简中等宽）四份**完整字体**与许可原文入库 `assets/fonts/`，
    `SOURCES.md` 记录上游、版本与 SHA-256；全部 SIL OFL 1.1，可再分发。
  - 子集：新增 `ci/subset-fonts.py`，字符集为 ASCII/Latin + GB2312 + 仓库文本字符，
    四份子集合计 7.68 MiB（完整文件不嵌入可执行文件）。
  - GUI：加载四份子集并按字符显式分段——比例文本拉丁 Inter／CJK Noto Sans CJK SC，
    等宽文本（diff 预览）拉丁 JetBrains Mono／CJK Sarasa Mono SC；不依赖字体回退顺序。
    空闲 RSS 41.1→71.8 MB、私有 17.3→40.3 MB，线程 24、句柄 243、CPU 0.00%，
    仍在 FR-712 预算内（余量约 24 MB）。
  - 未完成：TUI 字体安装/状态命令（T155／FR-717）；终端字体不可由程序强制，
    该命令只做用户级安装与终端配置提示。

- TUI 字体声明批次（2026-10-24）：**`whcode font install|status|uninstall`**（T155，
  ADR-028 修订 1、FR-717）。
  - `install` 把四份完整字体复制到用户字体目录（`%LOCALAPPDATA%\Microsoft\Windows\Fonts`）
    并写 HKCU 注册（`reg.exe` 子进程，零 unsafe、零新依赖）；来源支持 `--from` 显式目录、
    可执行文件旁与当前目录三级探测；`--dry-run` 预览；发布包不带完整字体，缺失时报
    可操作错误（指引 `--from` 与 `SOURCES.md`）。
  - `status` 检测安装状态（文件 + 注册）并**只读**解析 Windows Terminal settings.json
    的 `font.face`；`uninstall` 注销并删除，完全可逆，不触碰用户原有字体条目。
  - 终端字体不可由程序强制：`install` 只输出配置片段供用户自行粘贴，不改写用户配置。
  - 真机实测（`artifacts/font-run-2026-10-24/`）：装前全缺 → 装后四份 complete →
    `reg query` 独立交叉验证 → 卸载恢复原状；实测抓出并修复 WT 包族名笔误
    （`8wekybeld3zzbb` → `8wekyb3d8bbwe`，修复前 status 永远检测不到已安装的 WT）。
  - 分层：编排与报告在 `whcode-app::fonts`（注册表经 `FontRegistry` trait 注入，零平台
    分支），reg.exe 实现与路径探测在 `whcode-platform::fonts`，装配在 `whcode-bin`
    （`PlatformFontRegistry`）；i18n 词条 `cui.font.*` 中英齐备。

- GUI 达标批次（2026-10-23）：**字体、主题切换与最小化停绘**（T152）。
  - 字体：CJK 由“读入 19.7 MB `msyh.ttc`”改为**按族名使用系统字体**
    （`Font::with_name("Microsoft YaHei")`，候选 `SimHei`）。系统字体不可再分发，
    因此不随包发布子集；进程只探测文件存在性，不把字体字节读进内存。
    空闲 RSS 95.7→41.1 MB、私有内存 72.4→17.3 MB、句柄 234→244（仍 ≤ 256）。
  - 主题：新增 `t` 键循环三套主题（不重启，FR-713）；滚动条轨道改背景色、滑块改前景色，
    去掉主题外的 `#BEBEBE` 中间灰（灰阶占比 4.6%→1.4%）；`scrollable` 显式 `Fill`，
    修复长列表把页脚挤出可视区。
  - FR-714：最小化 10 s 的 CPU 增量为 0.000 s，还原后标题与画面一致；证据由
    `ci/gui-probe.py` 落 `artifacts/gui-run-2026-10-23/`（含 `theme_cycle` 逐帧色值比对）。
  - 证据脚本修正：声明 per-monitor v2 DPI 感知（物理像素口径）、按屏幕调整窗口并记录
    `dpi_scale`、`AttachThreadInput` 切前台后再注入按键、资源采样移到按键注入之前。
  - 发布包：`whcode.exe` 3.71→7.66 MB、zip 14.65→16.28 MB（iced + tiny-skia 依赖）；
    `ci/package-windows.py --verify` 三条命令通过，记录已刷新。
  - 未取得：125%/150% 缩放与 RTL 证据；应用初始窗口尚未适配工作区（探针代调尺寸）；
    恢复去抖未做帧级计时。详见 `docs/adr/ADR-027-native-gui-framework.md` 修订记录与
    `artifacts/progress-review-2026-10-23.md` §18。

- 交付批次（2026-10-23）：**CI 接线 + 发布包安装验证 + 三份指南 + quickstart 全流程留证**。
  - CI：五份 workflow 从 `ci/platform-matrix/` 迁入 `.github/workflows/platform-*.yml`
    （GitHub 只执行该目录，原位置等于从未运行）；Windows 流水线补齐计数、三方追溯、
    前端预算与打包安装四个门禁步骤。平台执行记录仍未取得（无远程/runner），
    Check 3／249 维持未通过。
  - 发布包：新增 `ci/package-windows.py`（白名单打包、仓库外解包运行验证），
    证据 `artifacts/install-verify-windows-amd64.txt`（Check 118 通过）。
  - 指南：新增 `docs/guides/tui.md`、`web-deployment-security.md`、`disaster-recovery.md`
    （逐条标注实现位置与未实现项），Check 119 通过。
  - quickstart：`artifacts/quickstart-run-2026-10-23.md` 记录 30 条目（成功 20、预期失败 5、
    未取得 5、缺陷登记 1、异常 0）；同步修正 `quickstart.md` 的命令面（`--session` 必填、
    `db compact`／`db archive` 的真实形态、`db purge --before` 为 Unix 毫秒）。
    交互式 TUI、真实 SSH 节点、72 h 长跑仍无证据，Check 124 维持未通过。
  - 新登记缺陷：`whcode events export --session <不存在>` 返回 `count=0` 且退出码 0，
    与“存在但无事件”不可区分（错误的空），待修。

- TUI 渲染修复与 V-003 取证（2026-10-23）：**歧义宽度字符缺字**。
  - 缺陷：页脚“按键 0”显示成“键 0”、“视图”显示成“图”，标题“会话”显示成“话”。
    根因是 `CrosstermBackend` 逐格绝对定位按 `unicode-width`（歧义字符 1 格）计算列号，
    而本机控制台把 `·`、`×`、`—`、`“”` 等渲染为 2 格，后一格写入覆盖尾格、整字被擦除。
  - 修复：新增 `crates/whcode-tui/src/rowbackend.rs`，改用**整行连续输出**（每行一次
    绝对定位），并按**运行时实测**的字形前进格数排版（制表符实测 1 格，与 Unicode EAW
    不一致，故不使用静态宽度表）；溢出行保留最右格锚定到最后一列，避免折行污染下一行。
  - 证据：`artifacts/tui-v003-2026-10-23/`（修复前后帧、像素截图、逐字宽度实测、
    resize API 实测、`summary.json` 10 项自动检查全部通过）。Check 24 补充复核，
    `docs/guides/tui.md` 新增 §7 记录根因、实测数据、代价与残留。

- 程序中文名由“旺皇 Code”改为**旺皇大模型执行框架**（2026-10-23，用户指定）：
  同步 `app.title`、`tui.title`、CLI `about`、Web 首页标题、TUI 视图断言与 README 标题；
  英文标识 `whcode`／`WHCode` 不变，历史变更记录不改写。
- GUI 立项（2026-10-23）：新增 FR-711—FR-715（原生桌面 GUI，只读 v1）与
  `docs/adr/ADR-027-native-gui-framework.md`（选型 iced + `tiny-skia` 软件光栅，
  默认不创建 GPU 上下文）。选型依据为五条路线的同口径实测：TUI 27.4 MB／4 线程、
  Win32 53.6 MB／8 线程（需 19 处 `unsafe`）、iced+软件光栅 80.9 MB／22 线程、
  iced+wgpu 206.5 MB、egui 350.8 MB（后两者必须 GPU）；证据
  `artifacts/gui-spike-2026-10-23.md` 与四张截图。新增 Task T149—T152、Check 250—252
  （250 通过，251／252 未通过：实现为零代码）；`whcode-gui` crate 尚未创建。
- GUI 骨架落地（2026-10-23，T150／T151）：新增 `crates/whcode-gui`（iced + `tiny-skia`，
  默认不建 GPU 上下文）与 `whcode gui` 入口；只读视图模型下沉为 `whcode_app::view_source`，
  由 TUI 与 GUI 共用（前端之间互不依赖）。新增 `ci/gui-probe.py`：三套主题截图、`3` 切视图、
  `q` 退出码与空闲 10 s 资源采样，证据 `artifacts/gui-run-2026-10-23/`。实测 RSS 95.7 MB、
  私有 72.4 MB、线程 24、句柄 234、空闲 CPU 0.00%，中间灰 1.34%—1.61%。
  Check 251 转入通过；Check 252 维持未通过（CJK 字体仍整份常驻、RSS 余量仅 0.3 MB）。
- 决策（2026-10-23）：**ADR-026 记录 GUI 铁律——桌面界面必须原生，严禁浏览器套壳**。
  禁止 WebView／WebView2、CEF、Tauri／wry、Electron、NW.js 以及“自绘窗口 + 内嵌 Web UI”
  等价形态；`whcode-web` 仅作独立浏览器前端（FR-603）。`AGENTS.md` §1 硬边界／§6 UI 纪律／
  §8 禁止事项、`decisions.md`（新增 D-15）、`compatibility.md` 已知限制同步登记；
  不新增 FR 与检查点，`checklist.md` 计数不变（GUI 尚无需求条目，落地前须先立需求与 ADR）。
- 里程碑批次（2026-10-23，Check 55／93／101／102／104／127／133／136／183 通过）：
  - **ADR-024（M8 运行时范围定性）**：JS 不交付引擎（`JsProvider` 声明 `Restricted`，
    执行返回结构化 `runtime_unavailable`，不得表述为 Node.js 支持）；Python 走子进程
    worker（记录解释器绝对路径/版本/实现名/ABI/依赖与 C 扩展探测）；Git Bash 为独立
    payload（来源白名单 + 版本 + 哈希 + 平台四重校验、五类命令验证）；不引入
    Boa／RustPython。同时记录 `whcode-bin → whcode-runtime` 装配依赖的边界（只读声明与
    探针，不加载动态库）。
  - **ADR-025（性能预算冻结）**：在 Windows 11 amd64 冻结 NFR-201—210，冻结值沿用
     spec 候选值；NFR-208（概览 p95 6 071 µs ≤ 50 ms）与 NFR-210（写入 6 921 条/秒
     ≥ 2 000 条/秒）达标，其余八项保持候选并登记责任人与退出条件；BSD 无证据不参与；
    `platform-matrix.md`／`decisions.md`（D-06/D-08/D-09/D-11）与 ADR-005/010/012
    未决项台账同步。
  - **payload 四重校验**（Check 102）：`PayloadMeta` 校验顺序固定为
    来源 → 平台 → 版本非空 → 哈希；新增 `SourceAllowlist`（精确匹配、去空白去重、
    空白名单拒绝一切），来源不明返回 `permission_denied`（不可重试）。
  - **能力等级可见性**（Check 101）：新增 `whcode-bin::runtimes` 清单，经 `doctor --json`、
    `doctor` 人类输出、TUI 摘要与 Web `GET /api/runtimes` 四处展示三档等级；界面路径
    不启动子进程。
  - **子进程取消与并发上限**（Check 104）：`worker::execute_with_cancel` 以 `AtomicBool`
    协作取消（轮询 ≤10 ms，`cancelled` 与超时区分）；`ConcurrencyGate`
    （`Mutex`+`Condvar`，`GatePermit` RAII 归还，不新建线程）限制同时在跑的子进程数，
    默认 `min(4, 可用并行度)`。
  - **Web 写命令落地**（Check 93／183）：新增 `POST /api/sessions/{new,rename,archive,delete}`、
    `POST /api/tasks/{retry,abort}`、`POST /api/purge`；破坏性命令默认只预览
    （`confirm=false`），显式 `confirm=true` 才执行——与 CUI `--dry-run`/`--yes` 同语义。
    数据形状统一到 `whcode-app::views`（`session_value`/`task_value`/`purge_counts_value`），
    三端共用同一字段集合；清理计数补齐 `messages`/`context_windows`/`session_overview`
    三个此前漏报的表名。新增应用层入口 `whcode-app::maintenance`（`purge_preview`/
    `purge_execute`，权限映射 `session.delete`），CUI 与 Web 共用。
    前端新增“维护”页签、会话新建/重命名/归档/删除预览与确认、任务重试/中止按钮、
    清理预览与确认、运行时清单（`data-i18n-placeholder` 支持新增）；体积仍在预算内
    （JS gzip 6 769 B / CSS gzip 946 B）。
  - **三方追溯门禁**（Check 127）：新增 `ci/traceability.py`（FR→任务、FR→验收、幻影 FR、
    幻影任务、编号连续唯一、缺口登记合规六条判据；支持 `FR-201—210` 区间与
    `FR-502/503` 并列写法），并新增“追溯缺口登记”节承载 FR-402/403/404（检查点增量差分、
    同事务提交、恢复加载引用）——登记须含责任人与退出条件，恢复覆盖后自动判为“登记失效”。
  - **资源测量信号修复**（`script/measure_resources.py`）：旧实现每次采样都启动
    PowerShell 枚举进程，29 s 的测试只采到 1 个样本并在“零样本”时输出“峰值 0 MB / PASS”
    ——把“没有信号”当成“达标”。改为持有进程句柄、用 Win32 `GetProcessMemoryInfo`／
    `GetProcessTimes` 50 ms 轮询，报告 `measured` 字段；零样本判为未取得信号并计入违规。
    本轮实测 `session_switch` 峰值 RSS 77.5 MB、491 样本。
  - **测试口径同步**：概览性能测试更名为 `overview_p95_within_frozen_budget_and_report_is_archived`
     并对比冻结预算（报告 `budget.frozen=true`）；`bench` 说明与 TUI 帧预算注记同步澄清
     “帧预算不属于 NFR-201—210、不随 ADR-025 冻结”。
- 稳定性与驻留修复（2026-10-23，Check 206／221 通过）：
  - **会话驻留缓冲**：`whcode-app::residency` 新增有界事件缓冲（200 条 / 256 KiB 双上限，
    超限返回 `backpressure` 而非静默丢弃）、`accounted_bytes` 占用统计与 `event_cursor`；
    卸载立即释放缓冲（只留元数据 + 游标），唤醒从空缓冲开始，历史由补拉还原；
    已卸载会话再收事件显式报 `invalid_state`。
  - **内存可观测性**：`whcode-platform::resources` 新增 `process_rss_bytes` /
    `current_rss_bytes`（Windows 经 `tasklist` 读工作集；其他平台返回结构化“延后”错误，
    不返回 0 伪装成功）。
  - **稳定性故障情形**：新增 `tests/stability/test_failure_modes.rs`（LLM 失败、断网、
    只读库写入失败、磁盘不足）与 `tests/stability/test_residency_memory.rs`
    （实测 RSS 37 756 928 → 6 275 072 字节，下降 31.48 MiB）。
  - **满盘错误分类**：`WhError::io` 将 `ErrorKind::StorageFull` 与 Windows `ERROR_DISK_FULL`
    （112）统一标注为“磁盘空间不足 / 清理空间后可重试”，其余 I/O 错误保留原文。
  - **测试隔离修复**：`whcode-store` 的临时库路径原为“PID + tag”，崩溃残留遇 PID 复用
    会读到旧库造成假失败（本轮全量跑暴露 3 例）；改为 `test_util` 进程内单例根目录
    （PID + 纳秒戳），同一 `tag` 仍映射同一路径。
- 收敛表优先项修复（2026-10-23，Check 144／171／224 通过、201 Windows 侧通过）：
  - **凭据隔离**：`whcode-runtime` 新增 `strip_credentials`（父进程继承与调用方显式设置的
    凭据型环境变量一并剥离：`KEY`/`TOKEN`/`SECRET`/`PASSWORD`/`CREDENTIAL`/`COOKIE`/
    `SIGNATURE`、`SSH_AUTH_SOCK`、`GPG_AGENT_INFO`、`WHCODE_*`），`worker::execute` 与
    provider 探测统一在启动前调用；用子进程探针 + 阳性对照证明“未剥离可见 / 剥离不可见”。
  - **检查点触发策略**：`whcode-domain::checkpoint::CheckpointPolicy`（定时 300 s、按步骤
    关闭、危险操作前开启、保留 10，三类阈值与保留数可配、越界即拒）经 `Config.checkpoint`
    加载期校验；`whcode-runtime::checkpoint` 重导出，避免新增依赖边。调度器接线仍属 M8
    装配，已在模块文档显式标注未接线。
  - **gettext PO 接口**：`whcode-domain::i18n` 新增 `parse_po`（续行、转义、`msgctxt`，
    跳过 fuzzy／复数／废弃条目）与 `I18n::load_with_po`（运行时 `<root>/i18n/po/<locale>.po`，
    256 KiB 上限、语言标识白名单禁路径穿越、缺失即回退内建）；`whcode-bin::load_texts`
    接线（`WHCODE_I18N_DIR` → 可执行文件目录；加载失败告警回退，不静默）。
  - **零残留进程（Windows）**：`whcode-platform` 新增 `process_exists`／`child_pids`，
    新增 `tests/stability/test_process_cleanup.rs` 启动真实 `cmd → ping` 进程树并断言父与子
    PID 在 `terminate_tree` 后均消失；BSD 进程组仍按 ADR-013 延后，Check 201 维持未勾选。
- 副作用恢复协议与三时点崩溃注入（2026-10-23，T114 部分交付／FR-410）：
  `whcode-store::tool_exec` 落地“意图—执行—登记”原语（`record_intent`/`mark_running`/
  `record_result`，状态迁移受限）与启动期恢复 `recover_uncertain`（prepared/running →
  `unknown`，单事务、`LIMIT MAX_RECOVERED + 1` 判截断、可重复执行）；
  `open_workspace_db` 在迁移后调用恢复并对非空结果告警（数量 + 前 5 个 ID + 下一步动作）。
  新增 `tests/crash/test_side_effect_recovery.rs`：父测试以 `--exact --ignored` 自启子进程，
  在步骤边界、意图后、执行中（副作用已落盘）、登记后四个注入点 `abort()`，父进程重开库断言
  恢复状态；“副作用已发生、结果未登记”用例以标记文件行数恒为 1 证实**不重放**。
- 门禁与平台层修复（2026-10-23，进度复核处置）：`whcode-platform` 新增 `sleep_command`
  （Windows `ping` / POSIX `sleep`，秒级粒度），`whcode-runtime` 的超时用例改经它构造命令，
  消除 `whcode-platform` 之外的平台分流残留；`tests/contract/test_layering.rs` 的平台判定
  marker 表补 `cfg!(windows`/`cfg!(unix` 宏形态（原表只覆盖属性写法，该类写法长期漏检）。
  构建脚本的平台条件编译同时改为按可用性探测外部时间命令（见下条）。
- 文档同步（2026-10-23）：README “当前状态”改为里程碑状态表 + 验收口径 + 门禁实测摘要，
  “核心能力”按 M1—M8 实际交付重写（并显式标注真实 LLM Provider 未接线、M7 服务端未交付）；
  ADR 清单更新至 023；`docs/matrices/compatibility.md` 已知限制第 4/5 条改写
  （M7 仅交付 SSH 客户端；实现完成不等于验收通过）；T121 注明风险台账落点为
  `docs/matrices/decisions.md`；进度复核报告 `artifacts/progress-review-2026-10-23.md`
  经 `.gitignore` 放行入库。
- 版本号改为**二进制构建时间**（2026-10-23，用户要求）：`crates/whcode-bin/build.rs` 在
  构建时取系统时钟注入 `WHCODE_BUILD_STAMP`（**本地时间** `YYYY-MM-DDTHH:MM±HH:MM`、
  精确到分钟）。时区获取**不做平台判定**（NFR-101：平台条件编译集中在 `whcode-platform`，
  构建脚本同样计入契约测试），改按可用性依次探测外部时间命令（`powershell` → `date`）；
  两者均不可用时回退 UTC `...Z` 并输出 `cargo:warning`（不静默降级）；零新依赖、零 unsafe。
  `whcode --version`、`doctor` 文本与 `doctor --json` 的 `whcode_version`
  均报告该时间戳。Cargo 包版本保持 `0.1.0`（语义化版本用于依赖与打包；展示版本 = 构建时间）。
- 程序中文名调整为“旺皇 Code”（2026-10-23）：中文文本层 `app.title`、TUI 中文标题、
  CUI `--help` 简介、Web 页首标题与 README 标题同步；英文名保持
  `WHCode Coding Harness` / `whcode` 不变（`?locale=en` 与 `LANG=en` 路径不受影响）。
- 批次 4：Web 仅限局域网 + 认证边界（2026-10-22，ADR-023）：`serve --web` 增加
  `--token-file`；地址边界只允许回环或 RFC1918 私网（公网/`0.0.0.0` 拒绝）；局域网
  监听强制认证与 Origin 检查；前端从 URL fragment 读取 token 并以 Bearer 调用 API；
  响应加 `cache-control: no-store`。Origin 语义修订：缺失 Origin 两种模式均放行
  （同源 GET 真实形态；跨站由 Bearer 阻断），带 Origin 必须命中允许集。
- 批次 3：文本层端到端接线（2026-10-22，T130）：zh-CN/en 词表扩至 55 键并新增
  `resolve_fmt`/`entries`；TUI 标题/页脚/空态与视图摘要、CUI 人类可读输出、Web 标签/
  状态/摘要统一经 `whcode-domain::i18n`；Web 新增 `GET /api/i18n`（?locale= →
  Accept-Language → zh-CN，返回 rtl 与词条）与前端 `data-i18n`/`<html lang dir>`；
  `?locale=` 显式覆盖。修复主入口路由回归（`whcode --workspace DIR` 误入 clap）与
  首屏任务列表延迟。数据字段/错误码/退出码不随语言变化。
- 批次 2：CUI 命令调度装配（2026-10-22，T084/T085 接线）：`whcode <command>` 经
  `whcode-app` 执行会话/任务/事件/状态/SDD 状态与 `db` 维护命令（stats/check/purge/
  compact/archive/explain），`--json` 统一信封、错误四字段、稳定退出码（新增用法错误
  `2`）；`--workspace`/`--json` 为全局旗标；未接线命令（run/events show/spec 门禁/
  runtime/db checkpoint）显式失败。新增写入并修复：显式删除不套 TTL 活跃窗口保护
  （避免"删除刚创建的会话"被静默跳过）。同命令一致性用例（CUI vs Web 读命令）落地。
- 批次 1：前端数据接入（2026-10-22）：TUI 新增 `StoreViewSource`（装配根实现）——六视图
  经 `whcode-app` 只读命令供给（会话/任务/事件/上下文/Spec 真实数据；`j`/`k` 选择会话、
  `r` 刷新、`> ` 选中标记；数据源错误显式写入摘要行，打开失败回退空数据源并告警）。
  Web 新增 `/api/tasks`（会话/工作区总览）与任务列表、会话条目点击选择（aria-current、
  URL 同步、键盘可达）、无参数时自动选中首个会话。修复会话切换的并发事件重复（在途
  守卫 + 会话快照校验 + 游标去重）与事件游标起点负数。演示夹具扩展 3 个任务。
- Web 服务装配与端到端测试（2026-10-22，T090—T095 补装配 + T135/T136 收口）：
  `whcode serve --web [--bind HOST:PORT] [--workspace DIR] [--config PATH]`
  （contracts/cui-cli.md §1）——静态前端**编译期嵌入**（固定白名单路由、无目录遍历面）
  + `/api/*` 经同一应用服务；顺序服务循环（DuckDB 连接 `Send` 非 `Sync`，ADR-002）；
  WebSocket 握手 + 正常关闭（会话推送未实现，前端不依赖）；外部绑定拒绝（FR-606 实测）。
  ADR-022 修订 Origin 匹配（scheme+host、端口不参与；回环模式缺失 Origin 放行，
  非回环 host 仍拒绝）。新增演示工作区夹具 `tests/integration/seed_web_demo.rs`（幂等）。
  真实浏览器与桌面取证：`artifacts/web-browser-2026-10-22.txt`、
  `artifacts/tui-computeruse-2026-10-22.txt`（截图 `web-browser-2026-10-22/history-view.png`）。
- T082/T083/T129/T135 TUI 接线 + T136 收口 + T128 配置接线（2026-10-22）：六视图接入
  真实运行循环（`1`—`6` 切换、`ViewSource` 准备层、`EmptyViewSource` 空数据源显式
  “暂无数据”不伪造）；脏标记门控与空闲零重绘进入循环，重放证据 `run_scripted_steps`
  与真实循环共用 `LoopCore`（探测计数/最小化跳过/还原单次重绘均断言）；窗口最小化
  探测接线（ADR-021 修订：仅待绘制帧 + 输入静默 ≥1s 时探测、间隔 5s→60s 指数退避、
  检测到最小化回落 5s——空闲零探测）；帧路径限行投影 `render_lines_limited`（帧成本
  由终端尺寸界定，帧预算归档 `artifacts/tui-frame-budget.json`，release p95 448 µs）；
  主题经 `ui.theme` + `WHCODE_UI_THEME` 选择并进入渲染单元（FR-612）；前端审计扩展
  `suspend.js` 与 NFR-212 断言（运行时路径禁 GPU 上下文探测、无 `requestAnimationFrame`）；
  真实控制台六视图取证 `artifacts/tui-views-2026-10-22.txt`。
- T068—T081 SDD 门禁状态机（M4）：`whcode-speckit` 交付版本化工作流配置与应用层
  状态机（FR-302）、Markdown 解析器（未解析段落/重复 ID/未识别字段报告 + 纠偏重试
  ≤2 次，仍失败 blocked 并保留原文，FR-303/309/R-06）、Constitution 创建/导入/变更
  审计（FR-301）、稳定 ID 与引用校验（FR-304）、双向追溯与断链检出（FR-305）、
  Analyze 五类缺陷检出（FR-306）、Converge 四态判定与差异报告（FR-310/311）。
  `whcode-app` 交付门禁强制执行（高优先级澄清阻止 Plan、Analyze 阻止 Implement，
  FR-307/308）与 force-advance（reason 必填 + 审计含 actor/reason/跳过门禁/产物版本/
  时间戳，FR-312）与阶段角色显式注入（FR-313）；快照 `session.stage` 对四端同源
  （FR-314）。store 迁移 v5（`sessions.stage` 可空列 + 回填，DuckDB ALTER 限制已记录）。
  speckit 21 例内联、app 5 例新增、CUI spec_cmd、集成测试 V-201—V-207 七项判定
  （`tests/fixtures/sdd/` 违规产物夹具，T081）。
- T056—T067、T132 应用服务（M3）：`whcode-llm` 交付 Provider 语义与桩实现（能力探测/
  取消/流式/计量来源/超时/错误归一化，FR-201）、双队列事件路由（关键不丢弃返回
  `backpressure`、展示可合并并留可查询标记，FR-206/ADR-006）、四级追踪与双层脱敏
  （字段名 + `sk-`/`Bearer` 值模式）与完整性标记（FR-203/208/D-05）、链路关联与断链
  检出（FR-204）、计量来源两级（估算标记 `cost_is_estimate`，FR-207）、传输层进度
  事件（FR-616）。`whcode-app` 交付上下文加载器契约（`items + next_cursor +
  consumed_budget + truncated_reason` 有界迭代，FR-105）、压缩摘要指针（压缩后原文
  仍可查，FR-107）、事件导出（脱敏策略随导出注明，FR-205）、按游标补拉（FR-202/605）。
  `whcode-cui` 交付预算视图（FR-108）与事件 JSON/JSONL 视图。18 例 llm 内联、
  7 例 app 新增、3 例 CUI、1 例集成测试（脱敏/串联/截断/补拉，V-103 关联）。
- T046—T052 应用服务（M2）：`whcode-app` 交付会话命令（new/list/show/rename/archive/
  delete，FR-101）、会话切换有界加载（元数据 + 预聚合概览 + 50 条尾部 + 事件游标，
  FR-102）、任务总览（阶段分布/完成度/阻塞/活跃，FR-103）、任务详情五类信息与
  retry/abort 状态机（FR-104/401/408）、能力模型权限检查（拒绝统一
  `permission_denied` 不可重试，FR-604）、四端同源状态快照（FR-314；`stage`/
  `context`/`llm`/`runtimes` 属 M3/M4/M8，显式 `None`/空标注）。删除复用清理全链
  （预览/确认/审计/活跃保护，FR-509）。28 例内联测试。
- ADR-018：`whcode-app` 经 `whcode-store` 公共 API 访问存储、SQL 只在 store；
  AGENTS/module-design 分层口径同步修订。store 新增 `directory`（会话目录，
  schema v4 会话 ID 序列）与 `tasks`（任务投影，状态/阶段分布、有界详情读取、
  产物只出引用）存储原语；purge 判据扩展按会话 ID 清理；迁移 v0→v4/v1→v4/
  v2→v4/v3→v4 路径补齐。
- T053—T055 集成测试（`tests/`）：`gen_dataset` 夹具（10 万事件 + 1 万任务固定
  分布）；`session_switch` 验证切换有界加载与双向游标无重复无遗漏（V-101/SC-001）；
  `overview_perf` 报告 1 万任务概览 p50/p95/p99 并归档
  `artifacts/overview-perf-m2.json`（p95 7.6 ms，候选预算 50 ms，预算未冻结——
  V-102/SC-002，D-06）。
- T045/T043/T044 集成测试（`tests/integration/`）：`space_report` 覆盖清理/压缩前后
  文件大小、WAL、已用块、可用块四项统计并显式排除任何"VACUUM 释放空间"承诺；
  `durability` 以子进程 `abort`（0xC0000409 fail-fast，不运行析构）模拟进程终止，
  重开后 WAL 重放还原全部已确认事务、`seq` 唯一、按幂等键重放返回既有 seq（V-104/V-106）；
  `idempotency` 覆盖跨写者重启去重、同键不同载荷确定性返回既有结果、缓冲中同键
  不得重复入队（V-106）。
- T042 WAL 策略：`wal.rs` 提供阈值设置（白名单校验后透传 `checkpoint_threshold`/
  `wal_autocheckpoint`，`None` 保持现状）、显式/空闲窗口 checkpoint 与纯函数空闲判定；
  高负载路径无强制 checkpoint，自动阈值行为经 `monitor::wal_status` 观测。
- T041 compact：`compact.rs` 实现 D-10 流程（进程内维护锁 → CHECKPOINT 停写 → 文件复制 →
  副本校验（schema 版本/必需索引/逐表行数）→ 原子切换（备份改名 + 失败恢复）→ 复验）；
  副本校验失败或切换失败不动原库（Check 41）；`SpaceReport` 记录文件大小/WAL/已用块/
  可用块四项；不承诺缩小文件（FR-510、R-09）。BSD 文件锁语义按 ADR-013 延后取证。
- T040 导出/导入：`export.rs` 实现 `whcode-export` v1（UTF-8 JSON：清单 + 表数据 +
  分块校验和）；导出 11 张业务表（`audit`/`runtime_state`/`schema_migrations` 不出库），
  流式写出无 `OFFSET`；导入前依次校验内容哈希、格式/版本、schema 兼容（导出版本 ≤
  目标版本）、分块校验和、列一致与主键/唯一键冲突，任一失败整体拒绝（Check 40）；
  导出写 `data_export` 审计，导入不在强制审计清单（决策已记录）；时间戳经 SQL CAST
  往返无损。
- T039 清理与归档：`purge.rs` 实现会话归档（全有或全无）、按时间清理与 TTL
  （`--before`/TTL 判据，支持仅清理已归档会话）；清理以会话为根删除 `tasks`、
  `checkpoints`、`runtime_executions`、`events`、`messages`、`stage_artifacts`、
  `context_windows`、`session_overview` 与 `sessions` 行，`audit`/全局行不清理；
  FR-509 四项机制：`preview_purge` 只读冻结清理集、`execute_purge` 强制显式确认、
  删除与 `db_purge` 审计同一事务、活跃会话保护（显式名单 + 默认 24h 活跃窗口，
  预览与执行两时点各检查一次，跳过集在结果与审计中报告，`Abort` 策略可整体拒绝）；
  10 例测试覆盖预览—执行一致、确认拒绝、TTL 边界与活跃窗口交互、空清理审计。
- T038 监控：`monitor::collect` 一次输出数据库统计、14 表精确行数、WAL 状态与写者队列深度；
  `plan_query` 仅接受只读查询，支持 `EXPLAIN`/`EXPLAIN ANALYZE`；写者服务新增
  `queued_len`/`capacity` 诊断指标（`AtomicUsize`，SeqCst）。
- T037 会话概览：`session_overview` 预聚合与事务化重建（单会话/全量），标注 `session_id`
  为强一致键、计数与阶段分布为最终一致；6 例测试覆盖滞后检测、upsert、空会话与未知会话拒绝。
- T036 索引与计划：schema v3 补齐 messages `(session_id, ts, id)` 与 tasks `(session_id, stage)`
  索引，删除被覆盖的旧消息索引；新增索引注册表与完整性检查；基准实验记录 8 条关键查询的
  `EXPLAIN`/`EXPLAIN ANALYZE`——单列索引命中，复合索引在 DuckDB 1.5.6 未触发扫描，顺序扫描
  已逐条论证（`artifacts/query-plan-report.json`）。
- T035 游标分页：`events` 按 `seq`、`messages` 按 `(ts, id)` 建立稳定全序，双向排他游标；
  单页上限 1000，无 `OFFSET` 路径；5 例测试覆盖重复请求一致与双向无重复无遗漏。
- T033 跨版本迁移：schema 升级到 v2（事件 `request_id`/`tool_call_id` 追溯索引，FR-204）；
  集成测试覆盖 v0→v2、v1→v2 数据保留与中断回滚（版本号不前移、半成品索引不残留）。
- T034 写者服务：专用写线程 + 有界通道 + 同步确认（ADR-017）；`whcode-store` 34 例测试通过，
  压力证据 `artifacts/writer-service-stress.json`（4 000 事件、`seq` 唯一连续、p50 45 µs）。
- 用户复核接受桌面自动化 TUI 记录，关闭 V-003 并勾选 Check 24；方向键/raw mode 残留限制已注明。
- V-003 桌面自动化冒烟：resize、Ctrl-C、q 均通过；非人工逐键，人工复核待确认（`artifacts/tui-manual-2026-10-21.txt`）。
- T032/T033：14 表 schema 与迁移集成测试；写者服务并发设计档案 ADR-017（T034 设计先行）。
- M0 按 ADR-016 有条件收口：Windows 工程基线 Go；V-003 人工项待补，V-004 延后 M7，V-006 预算不冻结；新增 TUI 人工冒烟协议。
- 工程准则应用至本项目：新增 `ci/check_counts.py` 计数门禁、证据层级/三次复查/可逆性规则与 `doctrine-application.md`；T148、Check 249。
- 新增 `engineering-doctrine/`：可移植工程哲学、过程、需求、设计、并发、安全、UI、交付与模板。
- 完整读取 `sacred_sources.duckdb` 的一刻哲学与四版前言，新增 `docs/guides/yike-philosophy.md`；NFR-220—222、T147、Check 246—248。
- 哲学增补自由、洞穴与安全：不定义用户自由、退出权、反全知幻觉、证明优先、主体性、安全反教条；NFR-218/219、T146、Check 244—245。
- 新增 `docs/guides/performance-and-concurrency.md` 与 ADR-015：并发设计先行、一手文献门槛、汇编例外；NFR-216/217、T145、Check 241—243。
- 新增 `docs/guides/philosophy.md` 与哲学门禁：形式主义审查、正负和解、三重时间依据、
  0 信任原则。
- 需求基线新增 NFR-211—215：资源占用（内存/GPU/线程/句柄）、GPU 纪律、安全优先、
  BSD 代码兼容与禁用 GNU 工具、内存安全与零 `unsafe`。
- 新增 `AGENTS.md` 代理协作入口与过程文档集（软件工程过程、需求工程、设计原则、模块设计、
  UI 设计、测试模型）。
- 契约测试新增 workspace lints 继承校验，确保全部成员保持 `unsafe_code = "forbid"`；
  clippy 增 `mem_forget = "deny"`。
- 新增 `docs/guides/test-model-sensenova.md`，记录图像输入实测与 `thinking` 对象形态。
- NFR-204 判定改为组合判据（2026-10-26，用户批准，ADR-025 修订记录）：冻结值 ≤ 0.5 MB/h
  不变；判定改为“前置条件（release、样本 ≥ 60、单 PID、`event_missing` 全程 0、线程/句柄
  恒定）+ 五要素 + 三态结论（通过／违反／无权判定）”，估计量结论冲突一律无权判定。
  长跑夹具补齐 ADR-025 包络（`memory_limit='192MB'`／`threads=min(4, 逻辑核数)`）与
  summary 纪律字段（`build_profile`、引擎版本、包络回读值、样本 `pid`）；新增
  `.github/scripts/analyze-nfr204.py` 在 `long-run.yml` 程序化判定，verdict 随 artifact
  归档并进公开注解。run 40154046118 维持“现行口径超预算、按组合判据无权判定”登记。
- CI action `cross-platform-actions/action` 升级至 v1.7.0 并固定 SHA（清单复核日期
  2026-10-26；周度联网复核发现的唯一漂移，其余六个 action 经认证途径复核无漂移）。

### 修复

- Web Origin 校验全串精确比较误伤真实浏览器（Origin 含端口；同源 GET 不带 Origin →
  一律 403）：按 ADR-022 改为 scheme + host 匹配（回环模式缺失 Origin 放行，
  非回环 host 仍拒绝；认证模式仍要求 Origin）。
- `serve --web` 静态路由未剥离查询串：`GET /?session=…` 落入 404——已修复并加回归断言。
- Web 挂起（blur/visibilitychange）期间未暂停轮询（FR-617）：真实浏览器实测挂起
  2.5 s 内仍发出 9 次 API 请求 → 修复后 0 次；恢复单次刷新后续拉，游标不重复不遗漏。
- `whcode-tui` 的 `theme` 模块此前**未在 `lib.rs` 声明**（文件在盘但未编译，测试从未
  运行）——已声明并接线（T128 收口，2026-10-22）。
- `tests/stability/run_stability.sh` 工作负载（`extreme_scale` 循环）在异常退出路径
  **不清理**：曾遗留负载循环 4 小时（干扰后续构建与测试）。修复：`EXIT`/`INT`/`TERM`
  trap + `taskkill /T` 进程树清理（Windows 无进程组语义，单杀子 shell 不终止 cargo 下游）。
- 入站 TLS 密文暂存跨 tick 被清空（2026-10-26，Check 284）：`read_tls` 只消费到记录边界时，
  余下密文被下一次读取覆盖，表现为 `cannot decrypt peer's message`。现保留未消费字节并
  按 `MAX_TLS_CIPHERTEXT` 显式限界，补 400 KiB 部分写续传、明文单次读取上限与 TLS 第 97 条
  连接直接关闭测试。
- BSD DuckDB 2.0 源码构建在 `cmake --install` 阶段因静态库体积/安装规则失败（run
  `40267545791`，`store_gate = NO-GO`）：改为只构建 `libduckdb.so` 并手工复制共享库与
  `duckdb.h`，不经 CMake 安装规则；NetBSD 的 rustup 安装改为仅在 `rustc` 可执行时前置 PATH，
  下载失败回退 pkgsrc 并显式 SKIP。

### 计划

- M1 存储与事件基座：14 表 schema 与可重复迁移、单写者服务线程、游标分页、预聚合概览、
  清理/压缩、监控（tasks.md T032—T045）。

### 修复

- 测量证据的环境字段不再伪造（2026-10-25）：`test_memory_limit`／`test_writer_stress`／
  `test_query_plan` 曾把 `machine_note` 写死为本机机型与内存，GitHub runner 上生成的证据
  同样带着该字符串。现改为如实标注“未采集”，保留实测的 `os`／`arch`／`logical_cores`；
  三个夹具已用 `--ignored --nocapture` 复跑并重新生成证据。CI 产物复测记录见
  `artifacts/ci-artifact-verify-2026-10-25.md`。

## [0.1.0] — 2026-10-21（M0 基线）

### 新增

- **存储**：DuckDB 单文件持久化（官方预编译动态库 1.5.6，SHA256 校验）；数据库冒烟四步
  （创建、事务写读、关闭重启、迁移幂等）；事件写者原型（幂等键、批次阈值、关键事件确认、
  错误传播与批次保留）。
- **写者性能**：批量落库（分块 IN 查询已存在键 + 分块多行 `VALUES` 参数化插入 + 插入前
  分配连续 `seq`，不再逐条回读）；实测吞吐中位数 **6 921 条/秒**（20 000 条 × 3 次，
  Windows 11 / i7-1260P / release / DuckDB 1.5.6 / `memory_limit=192MB`）。
- **TUI**：最小闭环（尺寸探测、事件循环、键盘与 resize、退出、终端恢复守卫）；
  非交互 stdin 时**不进入**事件循环（修复管道调用挂起的缺陷）。
- **SSH 客户端原型**：公钥认证、受限命令执行、超时与输出上限、按“意图—执行—登记”留痕。
- **CLI**：`--version`、`doctor [--config PATH] [--json] [--verbose]`、
  `bench baseline --out PATH [--events N] [--repeat R]`；`doctor --json` 增加
  `whcode_version` 与运行期 `duckdb_engine_version`。
- **工程体系**：十三 crate 分层与可执行契约测试（前端不触库、平台分支收敛、目录分类门禁）；
  五平台 CI 定义；依赖记录脚本；基线基准夹具与稳定性测试占位；LF/UTF-8 编码策略；
  SECURITY.md、CONTRIBUTING.md、PR/Issue 模板；BSD-2-Clause 许可（FreeBSD 中文社区）。

### 变更

- 平台范围收敛为 **Windows 11 amd64**（ADR-013），不考虑任何 32 位平台；
  FreeBSD/NetBSD/OpenBSD/DragonFly 延后，CI 改为手工触发。
- MSRV 与可复现工具链分离：`rust-version = "1.88"`（MSRV），`rust-toolchain.toml` 锁定
  `1.99.1`。
- 基准夹具 `memory_limit` 由未经验证的 384MB 改回 §5.4 基线 192MB（ADR-012 证伪了
  “192MB 会 OOM”的断言）。

### 修复

- 事件写入逐条回读 `seq` 造成的吞吐瓶颈（158 → 6 921 条/秒）。
- TUI 在“管道 stdin + 控制台 stdout”环境永久等待按键。
- 错误对象在无细节时省略 `details` 键，违反契约四字段要求。
- 有界队列在快照溢出时可能淘汰关键条目，违反“关键事件不得静默丢弃”。

### 已知限制

- 性能预算**未冻结**：缺 72 小时时间序列（NFR-204、§11.5）。
- V-003 交互四项（物理键盘、resize 重绘、Ctrl-C 恢复、强杀后 raw mode）待人工留证。
- SSH 服务端与运行时（JS/Python/Shell）尚未实现（M7/M8）。
- Web UI、CUI 命令树、SDD 门禁状态机尚未实现（M2—M6）。

## [0.0.0] — 2026-10-20

### 新增

- 初始工程骨架：Cargo workspace（13 crate）、领域类型、有界队列、配置与日志脱敏、
  分层与平台隔离契约测试。
