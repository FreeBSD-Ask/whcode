# WHCode 代理协作说明（AGENTS.md）

> 本文件面向在本仓库工作的 AI 编码代理（Claude Code、Codex、Trae、Cursor 等），
> 作用等价于 `AGENTS.md` / `CLAUDE.md`。它是**入口与摘要**，不复制规范正文；
> 与 `docs/guides/`、`specs/`、ADR 冲突时，以那些文档为准。

**最后更新**：2026-10-24 | **适用版本**：`0.1.0`（M0 基线，未发布）

## 1. 项目定位与硬边界

WHCode 是 Rust 原生、本地优先的 LLM 编码 Harness：单文件 DuckDB 持久化、事件可追踪、
规格驱动门禁、TUI/CUI/Web/SSH 多前端共享同一应用服务。

当前硬边界（不得自行放宽）：

- 唯一验收目标：**Windows 11 amd64**，目标三元组 `x86_64-pc-windows-msvc`（ADR-013）；
- **不支持任何 32 位平台**；
- FreeBSD/NetBSD/OpenBSD/DragonFly **延后**：代码与文档保留，不作验收结论；FreeBSD/NetBSD
  在 GitHub CI 随 `main` 自动运行（仅作平台证据），OpenBSD/DragonFly 无原生测试、保留手工占位；
- 不使用任何 GNU 工具链；Rust 锁定 1.99.1（`rust-toolchain.toml`），MSRV 声明为 1.88；
- 许可证 BSD-2-Clause，版权主体 FreeBSD 中文社区；
- **GUI 必须原生**：禁止以内嵌浏览器引擎（WebView／WebView2、CEF、Tauri／wry、Electron、
  NW.js 等）作为桌面界面外壳；Web UI 只作独立浏览器前端（FR-603），不得充当桌面 GUI
  （ADR-026 铁律级）。GUI 需求见 FR-711—FR-715（只读 v1），选型与资源预算见 ADR-027。

### 1.1 项目优先序（2026-10-21）

1. **资源占用**：内存、GPU、线程、句柄四类同时优化；新增固定占用必须有 ADR，空闲态不得随会话数线性增长。
2. **安全优先**：默认回环监听、追踪 `redacted`、最小权限、凭据不落日志与事件；放宽必须用户批准 + ADR。
3. **BSD 兼容**：保持代码级兼容与交叉检查；不改变 ADR-013 的验收范围，BSD 仍延后。
4. **禁用 GNU 工具**：沿用 D-01 修订，Windows 只用 msvc，不引入 GNU 工具链或依赖。
5. **内存安全**：普通 Rust 零 `unsafe`；禁止 `mem::forget`；队列/缓冲/缓存必须有界；句柄 RAII 并有释放测试；汇编仅按 ADR-015 例外（NFR-215）。
6. **性能与并发**：内存/CPU 优化先测量；多线程禁止原型先行；设计前研读一手文献并提交并发档案；汇编仅按 ADR-015 例外（NFR-216/217）。

### 1.2 核心哲学（2026-10-21）

1. **形式主义**：任何代码、接口、文档、流程、UI 都是形式；先问它解决什么真实矛盾，答不出不做。
2. **正负和解**：任何功能都有代价；提案必须写正面、负面、承担者与和解理由。
3. **三重时间**：设计必须有过去来源、现在证据、将来失效条件与复审触发。
4. **物质—意识**：文档必须落到代码/测试/产物，产物必须被文档解释；不一致以可复现事实为准。
5. **没有最佳实践**：现行硬约束是当前和解，不是永恒真理；变更走 ADR 并写复审条件。
6. **0 信任**：不信任报错、文档、外部事实与自己的旧结论；按风险独立复核。

### 1.3 自由、主体性与安全（2026-10-21）

1. **自由不被定义**：不替用户定义善/道德/自由；默认值可见、可改、可撤销，不做“为你好”的强制。
2. **退出权**：用户可随时停止使用并带走全部数据；禁止私有格式锁定、隐藏导出、强制账户/联网、暗黑模式。
3. **山洞**：任何仪表盘/模型都是局部视角；未知与盲区显式标 `unknown`/`[待验证]`，不制造全知幻觉。
4. **证明优先**：测试/测量/复现命令优先；不用宏大叙事替代证据，不以“改造用户工作流”为目标。
5. **主体性**：不做排行榜、连胜、羞耻驱动与行为操纵；工具增强判断，不替代判断。
6. **安全反教条**：安全措施必须有威胁模型、证据、代价与失效条件；不以“最佳实践/合规”结项；默认拒绝等现行措施是当前和解。

### 1.4 一刻哲学工程推论（2026-10-21）

1. **不伪造**：工具、模型与代理不得编造结果、文件内容或外部事实；未知标 `unknown`/`[待验证]`。
2. **反进步叙事**：采用率、星数、下载量、主流共识不是正确性证据。
3. **反苦难**：不以 RTFM、加班、牺牲作为设计或贡献证据；错误信息可操作、不责怪用户。
4. **自反文档**：写明假设、适用范围与复审日期；知识会过时。
5. **和解优先**：评审以可执行共识为目标，不把技术分歧道德化。
6. **去神圣化**：记录具体工作与取舍，不写天才/大佬神话。

详见 `docs/guides/yike-philosophy.md`。

**冲突判定**：以可复现证据为准；涉及用户自由与安全底线时，不得以“效率/主流/最佳实践”为由放宽；
任何一条规则都不得被用来伪造证据、压制反对意见或拒绝必要工作（`docs/guides/philosophy.md` §9）。

## 2. 开工前必读（按顺序）

| 顺序 | 文件 | 读它是为了 |
|---|---|---|
| 0 | `docs/guides/philosophy.md` | 最高层方法论：形式主义、正负和解、三重时间、0 信任 |
| 1 | `specs/001-whcode-harness/spec.md` | 需求基线：FR/NFR、验收场景、Assumptions |
| 2 | `specs/001-whcode-harness/plan.md` | 架构、技术选型、阶段划分 |
| 3 | `specs/001-whcode-harness/data-model.md` | 表结构、状态机、事件 schema |
| 4 | `specs/001-whcode-harness/contracts/` | 六份接口契约（CUI/API/事件等） |
| 5 | `specs/001-whcode-harness/tasks.md` | 任务编号、依赖、完成状态（权威进度） |
| 6 | `specs/001-whcode-harness/checklist.md` | 验收检查点：改了什么就要能对上哪个 Check |
| 7 | `docs/guides/repository-layout.md` | 目录分类与根文件白名单（由测试强制） |
| 8 | `docs/guides/software-engineering-process.md` | G0—G5 门禁、角色合并、产物映射 |
| 9 | `docs/guides/design-principles.md`、`module-design.md` | 设计原则与 14 个 crate 的职责边界 |
| 10 | `docs/guides/ui-design.md`、`gui-suspend.md` | 主题色值、墨水屏/AMOLED、i18n/RTL、挂起规则 |
| 11 | `docs/adr/`、`docs/matrices/` | 已批准决策与平台/能力矩阵（先看，勿重复决策） |
| 12 | `docs/guides/test-model-sensenova.md` | 测试模型接入、图像输入、已知限制与实测记录 |
| 13 | `docs/guides/performance-and-concurrency.md`、`docs/adr/ADR-015-design-first-concurrency-and-assembly.md` | 并发档案、文献门槛、汇编例外 |
| 14 | `docs/guides/yike-philosophy.md` | 一刻哲学源流、二十九章命题与工程翻译 |
| 15 | `engineering-doctrine/README.md` | 可移植工程哲学与开发准则（供其他项目复制） |
| 16 | `docs/guides/signal-chain-completeness.md` | 信号链完备原则（T116 复盘：交付 = 信号存在，而非执行存在） |

## 3. 环境与常用命令

Windows 下若 `cargo` 不在 PATH：

```powershell
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
```

标准验证链（提交前至少跑前五项；涉及布局/CI 时全跑）：

```powershell
python .github/scripts/fetch-duckdb.py                        # 首次构建前置：拉取并校验 DuckDB
cargo build --locked --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo test -p whcode-tests --test layering       # 分层/平台分支/前端不触库
cargo test -p whcode-tests --test layout         # 目录分类门禁
python .github/scripts/traceability.py                        # FR↔任务↔Check 三方追溯（含缺口登记合规）
python .github/scripts/package-windows.py --verify --record artifacts/install-verify-windows-amd64.txt  # 发布包安装验证（交付相关改动必跑）
python local/normalize_eol.py                   # LF + 无 BOM 审计
python local/check_workflows.py                 # CI workflow 自检
python local/measure_resources.py               # 资源铁律 M1/C4：测试进程 RSS/CPU 达标
python .github/scripts/benchmarks/check-frontend-budget.py    # 前端预算审计（涉及前端时）
```

运行入口：

```powershell
cargo run -p whcode-bin --release -- doctor
cargo run -p whcode-bin --release -- doctor --json
cargo run -p whcode-bin --release -- bench baseline --out artifacts/baseline.json
cargo run -p whcode-bin --release --             # 有交互控制台时进入 TUI，按 q 退出
```

### 3.1 测试模型与图像输入（SenseNova 6.8 Flash Lite）

- 端点 `https://token.sensenova.cn/v1/chat/completions`，模型 ID
  `sensenova-6.8-flash-lite`；凭据只存本地 `local/sensenova.env`，不入库、不进日志与事件。
- 图像输入：`messages[].content` 使用内容块数组，图片经 `image_url` 传入；`image_url`
  可用对象 `{"url": ...}` 或字符串。URL 可为公网地址或
  `data:image/{png|jpeg|webp};base64,...` Data URL；支持 JPG/JPEG/PNG/WebP。
- 已实测拒绝的形态：`type: "image_base64"` 返回 400 `invalid content`，不要使用。
- 思考参数：`thinking` 必须是对象 `{"type": "enabled"}`（字符串会 400 `invalid arguments`）；
  `reasoning_effort` 取 `low`/`medium`/`high`/`max`/`none`，项目测试用 `max`。
- 上限：上下文 256K（用户提供，官方未单列，标 `[待验证]`）、`max_tokens` 上限 65536；
  图片张数与总大小官方未对 6.8 单列，
  保持 `[待验证]`，以接口错误响应为准。
- 图像测试约定：小图（建议 ≤ 1 MB）优先 Base64；记录图像数量、字节数、用量与耗时；
  追踪保持 `redacted`，图片内容不得进入日志或事件正文。
- 完整参数、实测记录与官方来源见 `docs/guides/test-model-sensenova.md`。

## 4. 架构与依赖纪律（由 `test_layering` 强制）

单向依赖，禁止反向与跨层捷径：

```text
whcode-domain  ← 叶子：状态/事件/错误/门禁/审计/追踪；零内部依赖
whcode-platform ← 唯一允许 cfg(target_os) / CARGO_CFG_* 的 crate
whcode-app     ← 命令编排/权限/快照；禁止依赖前端；SQL 只在 store（ADR-018）
whcode-store   ← 唯一 DuckDB 链接点与写者服务
whcode-llm / whcode-speckit / whcode-tools / whcode-runtime ← 领域服务
whcode-tui / whcode-cui / whcode-web / whcode-ssh ← 前端适配器：依赖 whcode-app 与 whcode-domain；
                                    不自行打开数据库（web／ssh 只透传装配根注入的 `Connection` 类型，ADR-018/030）
whcode-bin     ← 装配根：配置、日志、feature 组装
```

- 平台差异只能出现在 `whcode-platform`；其他 crate 出现 `cfg(target_os)` 即门禁失败。
- 前端不得直接打开数据库、不得绕过应用服务；`whcode-web`／`whcode-ssh` 只能把装配根注入的
  连接透传给 `whcode-app` 命令（SQL 仍只在 store，ADR-018）；新增能力先落在 `whcode-app`。
- 新增 crate、依赖或跨层调用必须先有 ADR，再改 `Cargo.toml` 与门禁白名单。

### 4.1 Rust 内存安全与资源纪律（NFR-215）

以下为当前强制基线（可复审的和解），不是普适最佳实践：

- 自有 crate 的普通 Rust 禁止 `unsafe`：`workspace.lints.rust.unsafe_code = "forbid"`；契约测试
  `crates_forbid_unsafe_code_via_workspace_lints` 校验 14 个成员与 `tests` 全部继承。
  `forbid` 不可被 `#[allow]` 覆盖；`build.rs` 与测试代码同样受约束。汇编例外仅按 ADR-015
  执行：独立 crate/模块、安全包装、跨平台回退与差分测试，不得在现有 crate 内开洞。
- clippy 默认组（`all`）零告警；CI 以 `-D warnings` 强制，禁止用 `#[allow]` 静默绕过
  与安全相关的告警。
- 禁止 `mem::forget`（clippy `mem_forget = "deny"`）；不得用 `Box::leak` 规避生命周期。
- 队列、缓冲、缓存、事件通道必须有容量上限与上限测试；禁止无界累积；每个会话不得新增
  常驻线程。
- 文件、套接字、线程、事件句柄一律 RAII；实现 `Drop` 或显式关闭，并有释放测试。
- 内存上限与泄漏回归：`memory_limit` 只约束 DuckDB 缓冲管理器，不代表进程 RSS；RSS 按 NFR-201—204、线程/句柄按 NFR-211 测量，长时斜率归 72 小时稳定性测试。
- 第三方依赖内部的 `unsafe` 不作零承诺：以版本锁定、依赖记录与最小特性控制风险。


### 4.2 证据层级与三次复查（00a §7）

1. **证据层级**：代码、commit、测试与遥测 > 邮件/issue > 官方文档 > 复盘/博客 > 二手/AI。
2. **AI 总结不得作为唯一证据**；关键结论必须回到一手来源。
3. **三次复查**：查证（回到一手）→ 交叉验证（至少两个独立来源）→ 判定
   （确证/存疑/无法核验）。
4. 无法核验时标 `[待验证]` 或 `[存疑]`，不得默认为确证。
5. **可逆性分级**：可逆决策允许快速验证；不可逆决策（数据格式、协议、权限、
   并发内存序）必须设计先行并留回退证据。

## 5. 工程过程门禁（摘要）

| 门禁 | 本仓库落点 | 不可省略的证据 |
|---|---|---|
| G0 范围 | `spec.md` 范围/Assumptions、ADR | 范围外事项明确列出 |
| G1 需求 | FR/NFR、`tasks.md`、`checklist.md` | FR ↔ 任务 ↔ Check 三方追溯 |
| G2 设计 | `plan.md`、`data-model.md`、`contracts/`、设计指南 | 关键决策有 ADR；可映射到 crate |
| G3 实现 | `crates/**`、`tests/**` | fmt/clippy/test 全绿；正向与失败路径测试 |
| G4 验收 | `artifacts/`、Check 勾选 | 实测证据；未取得者标注缺口 |
| G5 发布 | `CHANGELOG.md`、git tag、README | 标签指向可复现提交 |

- **禁止跨门禁跳跃**：先写实现再补需求、用交叉编译冒充运行验证、用未复现的断言改基线，
  都属于已发生过的反面案例，不得重演。
- 门禁判定必须由程序实施（测试/脚本），不得以提示词或人工口头结论代替。
- 无法取得证据时写“未取得/阻塞”，不得勾选通过，不得伪造日志。

### 5.1 CI action 版本铁律（2026-10-24）

- 工作流 `uses:` 必须同时满足：**最新 release 版本** + **固定 commit SHA**，写法为
  `owner/repo@<40 位十六进制 SHA> # vX.Y.Z`；禁止 `@main`、`@master`、`@vN` 等可移动引用。
- `.github/scripts/action-versions.json` 是唯一事实源（版本 + SHA + 复核日期）；`.github/scripts/check-actions.py`
  在每次 CI 离线校验，`action-version-audit` 每周联网复核上游最新版本。
- 升级流程：`python .github/scripts/check-actions.py --update` 刷新清单 → 同步工作流引用与版本注释 →
  离线门禁通过 → 提交。例外必须走 ADR，不得以“临时”为名跳过。
- Dependabot 的 `github-actions` PR 只负责提议：合并前必须运行 `--update` 并提交清单，
  否则离线门禁失败；不得以自动升级为由绕过最新版本 + 固定 SHA 的要求。

## 6. UI 与资源纪律

- 前端**能多简陋就多简陋**：无框架、无打包器、服务端直出 + 原生 ESM（ADR-014）。
- 预算：首屏 JS ≤ 50 KB（gzip）、CSS ≤ 20 KB（gzip）、DOM ≤ 2000 节点；无 WebGL/视频/自动播放/第三方字体。
- 主题精确色值：`amoled-night` 背景 `#000000` 前景 `#E6E6E6`；`eink-day` 背景 `#FFFFFF`
  前景 `#000000`；`eink-night` 背景 `#000000` 前景 `#FFFFFF`；GUI 另有 `terminal-green`
  背景 `#000000` 前景 `#00FF00`（2026-10-24 用户新增，OLED 兼容纯黑）。TUI 实现前三套、
  GUI 四套齐备；墨水屏避免大面积中间灰与渐变，中间灰判据按真灰口径（R=G=B）。
- 减少无效动效；能交给 GPU 的交给 GPU，不能的关闭；不得为每个会话增加固定资源占用。
- 窗口最小化/不可见时停止绘制与动画：恢复去抖 ≥250 ms，单次全量重绘，事件游标连续（FR-617）。
- i18n：简体中文与英语必须同时可用；其他语言经 gettext PO 接口；阿拉伯语等 RTL 需逻辑属性与 BiDi 处理。
- **原生 GUI 铁律**（ADR-026）：桌面界面必须原生渲染或原生控件；禁止任何内嵌浏览器引擎的
  壳层（WebView／WebView2、CEF、Tauri／wry、Electron、NW.js）。GUI 需求已立（FR-711—FR-715，
  只读 v1），选型与预算见 ADR-027（iced + `tiny-skia` 软件光栅，默认不建 GPU 上下文）：
  空闲 1200×800 下 RSS ≤ 96 MB、线程 ≤ 24、句柄 ≤ 256、CPU ≤ 0.2% 单核，四套主题精确色值
  （三套基础主题 + `terminal-green`，见 FR-713 与 ADR-027 修订记录）。

## 7. 编码与文档规范

- 源码标识符、类型名、命令名用英文；文档正文用中文；中英混排按上下文判断，不硬拆术语。
- 中文正文（含注释）引号用“”或‘’，不用英文直引号；禁止使用“”‘’。
- 所有文本文件 **UTF-8 无 BOM**，换行 **LF**（`.gitattributes` + `.editorconfig` + rustfmt 三重保证）。
- 不得篡改代码块与非注释代码；注释只在逻辑不自明处添加。
- 未实现事项必须显式标记（`未实现`/`阻塞`），写清当前行为，不得依赖隐含默认值。
- 临时脚本、探测脚本、运行日志一律放 `local/`；**不入库**；临时脚本与运行日志
  由 `.github/scripts/clean.py --scope script` 回收（长期验证工具与凭据在脚本内 `KEEP_ALWAYS` 显式保留）。
- 新增根文件必须同步 `tests/contract/test_layout.rs` 白名单与 `repository-layout.md`；
  根目录不接受源码、脚本或散文档。

## 8. 禁止事项

- 禁止 `cargo fmt` 之外的自动重排大面积无关代码；禁止顺手重构与本次任务无关的文件。
- 禁止跳过测试、删除失败测试、用 `#[ignore]` 藏起失败；实验性用例须显式说明运行方式。
- 禁止在日志、事件、测试夹具、提交内容中写入真实 API Key 或其他凭据。
- 禁止修改全局 git 配置；本机无签名密钥时用 `git commit --no-gpg-sign`，不得改配置绕过。
- 禁止把交叉编译通过、静态推断、单次样本当作平台或性能结论。
- 禁止在 CI 工作流中使用可移动 action 引用（tag／分支）或非最新版本；升级必须同步
  `.github/scripts/action-versions.json` 与工作流 SHA（§5.1）。
- 禁止在未获用户批准时扩大平台范围、引入 GNU 工具链、增加 32 位目标或替换核心依赖。
- 禁止浏览器套壳式桌面 GUI（WebView／WebView2、CEF、Tauri／wry、Electron、NW.js 等，
  含“自绘窗口 + 内嵌 Web UI”）；桌面界面必须原生（ADR-026）。
- 禁止普通 Rust 的 `unsafe` 块、`unsafe fn`、`unsafe impl`、`mem::forget`，以及任何无容量上限的容器累积；汇编仅按 ADR-015 例外执行。
- 禁止暗黑模式、隐藏导出、强制账户/联网、排行榜/连胜/羞耻驱动与行为操纵。
- 禁止以“最佳安全实践/行业标准/检查清单”替代威胁模型、证据与代价分析。
- 禁止编造工具/模型结果、文件内容或外部事实；禁止以采用率/星数/主流共识替代正确性证据。
- 禁止以 RTFM、苦难、加班或“大佬”叙事作为设计、文档与贡献的证据。
- 禁止把测试图片、`base64` 内容、API Key 或凭据写入仓库、日志、事件与提交信息。
- 禁止用“最佳实践”或“业界惯例”替代本项目证据；引用外部实践必须写明适用条件与失效条件。
- 禁止用 AI 生成内容补足缺失证据；AI 只能整理已核验材料。
- 禁止用 AI 生成内容补足缺失证据；AI 只能整理已核验材料。

## 9. 完成定义与提交

一个工作单元完成需同时满足：

0. **信号链完备**（`docs/guides/signal-chain-completeness.md`）：工具/采样/测试
   交付的验收标准是"输出包含可归因信号"，不是"工具不崩溃"。null/空/零是歧义
   证据，须区分正确的空与错误的空（前置缺失）；歧义无法区分时**必须提问**；
1. 对应 `tasks.md` 任务可勾选，且 `checklist.md` 有可判定的 Check；
2. `fmt` / `clippy` / `test` 全绿，新增行为有正向与失败路径测试（含零 unsafe、有界性与
   资源释放测试）；
3. 涉及布局、CI、EOL 的改动跑过对应门禁脚本；
4. 文档与进度同步（spec/tasks/checklist/CHANGELOG/README 中受影响者）；
5. 证据落在 `artifacts/` 或在文档中可定位。
6. 设计与变更记录含形式审查、正负和解、三重时间依据与 0 信任校验（`docs/guides/philosophy.md` §8.1）。
7. 并发/性能改动附并发设计档案、一手文献依据、同夹具前后测量与回退验证（ADR-015）。
8. 自由与安全审查：退出/导出/离线/默认值可撤销可用；安全措施有威胁模型、证据、代价与失效条件（NFR-218/219）。
9. 不伪造/未知显式、反进步叙事、文档假设与复审日期已核对（NFR-220—222）。
10. 证据层级与三次复查已执行；不可逆决策附设计与回退证据。

提交约定（用户已授权代理自行提交）：

- 采用 trunk-based，小步提交；消息用 `type(scope): 摘要`，正文写“为什么”；
- 每次只暂存本次相关文件，不用 `git add -A`；不提交 `local/`、`target/`、`vendor/`、本地状态；
- 提交后回报：改了什么、证据在哪、还有哪些未取得。

## 10. 不确定时

- 需求歧义：先查 `spec.md` 与 ADR；仍不明确则向用户提问，不自行假设后大规模实现。
- 设计冲突：新开 ADR 或在既有 ADR 上追加修订记录，再改代码。
- 平台/性能结论：没有目标平台实测就写“阻塞”，并给出复现所需的命令与环境。
- 发现既有文档与磁盘事实不一致：以可复现命令的结果为准，同步修正文档并在提交信息中说明。
