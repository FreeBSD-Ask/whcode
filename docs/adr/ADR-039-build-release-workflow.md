# ADR-039：构建与发布独立矩阵工作流

- **状态**：已批准（2026-10-25，用户指令“把项目构建、release 也移动到 GitHub 独立矩阵 ci”）
- **来源**：用户选择——新建独立工作流；矩阵维度为构建形态（debug／release）；触发为 push
  `main` + PR + 手动；tag 时创建 GitHub Release
- **责任人**：CI 与门禁维护者
- **退出条件**：GitHub Actions 配额或 runner 能力不可接受、tag 发布策略变化、需要自托管 runner、
  或发布链路无法在 CI 产生可归因信号时，修订本 ADR

## 决策

1. **工作流职责三分**：`.github/workflows/ci.yml` 只承担门禁、五平台矩阵、功能测试、短测量、
   桌面入口与每周 action 审计；`.github/workflows/build-release.yml` 承担构建形态矩阵与发布；
   `.github/workflows/long-run.yml` 继续只承担 5.5 小时稳定性长跑（超出单 job 6 小时窗口，
   ADR-033）。三者边界互斥，同一类信号不设第二处实现。
2. **构建矩阵**：`build-release.yml` 使用 `windows-latest`，矩阵为 `profile: [debug, release]`，
   `fail-fast: false`（一种构建形态失败不得掩盖另一种的结论）：
   - `debug`：工作区构建 `cargo build --locked --workspace`，并产出依赖记录
     （`.github/scripts/platform-matrix/record_deps.py artifacts`，V-001）；
   - `release`：发布构建 `cargo build --release --locked -p whcode-bin`、打包与安装验证
     （`package-windows.py --verify --record artifacts/install-verify-windows-amd64.txt`）、
     SHA256 校验、TUI 冒烟与 Web 服务端冒烟，并上传全部产物与证据。
3. **触发与门控**：`push` 到 `main`、`push` tag `v*`、`pull_request`、`workflow_dispatch`
   均运行构建矩阵；**发布作业仅在 tag ref 运行**，`main`、PR 与手动触发只产出构建与验证结论，
   不产生 GitHub Release。
4. **tag 发布口径**：tag 必须形如 `v<version>` 或 `v<version>-<预发布标识>`，其中 `<version>`
   必须与打包脚本解析出的发布版本（`package-windows.py` 的 `read_version()`：crate 显式
   `version`，`version.workspace = true` 时回落根 `Cargo.toml` 的 `[workspace.package]`）
   一致；不一致立即失败，不发布任何资产。附件为 `whcode-<version>-windows-amd64.zip` 与其
   同名附加的 `<zip>.sha256`（sha256sum 兼容格式）；含预发布标识的 tag 按 prerelease 发布；
   重跑以 `--clobber` 覆盖同名资产，保证幂等。
5. **权限最小化**：工作流顶层 `permissions: contents: read`；只有发布作业提升为
   `contents: write`。凭据只用 `secrets.GITHUB_TOKEN`，不引入 PAT、签名密钥或其他长期凭据。
6. **G5 边界**：tag 由用户／发布负责人创建；工作流**不创建 tag**、不改写 `CHANGELOG.md`、
   不改动 `README` 的“未发布”表述。GitHub Release 只是交付渠道（把已有的可复现产物交给
   用户下载），不构成新的发布批准，也不替代 G5 的其他证据。
7. **artifact 名称保持不变**：`deps-windows`、`whcode-windows-amd64`、
   `install-verify-windows-amd64`、`smoke-evidence` 四个名称是既有引用锚点，迁移后名称与内容
   语义均不改变，避免下游命令、文档与历史记录失效。

## 证据与约束

- **现状引用**：合并前的 `ci.yml` 中 `platform-windows` 条目在测试之后继续执行“发布构建 →
  打包并安装验证 → TUI 冒烟 → Web 冒烟 → 上传冒烟证据 → 上传发布包 → 上传安装验证记录”。
  本 ADR 把这七步迁出，`ci.yml` 保留门禁与测试；迁移不是新增能力，而是职责重新归属。
- **新增与改动**：新增 `.github/scripts/check-release.py`，负责 tag 版本与打包版本一致性判定、
  资产清单校验与 `.sha256` 产出；`.github/scripts/action-versions.json` 增加
  `actions/download-artifact` 条目（最新 release 版本 + 固定 40 位 SHA，复核日期同步更新），
  供发布作业下载构建产物。
- **SHA256 独立复算与交叉校验**：`check-release.py` 用标准库 `hashlib` 对同一 zip 独立复算
  sha256（不复用打包进程的输出，也不引入第三方哈希实现），写出 sha256sum 兼容格式的
  `<zip>.sha256`；该文件必须恰为两个字段（64 位十六进制与文件名），文件名须等于当前包。
  发布作业同时下载 `whcode-windows-amd64` 与 `install-verify-windows-amd64` 两个 artifact，
  把复算结果与安装验证记录首部行
  `package = <zip>（<字节> 字节，sha256=<64hex>）` 的包名、字节数、sha256 逐项交叉校验；
  任一不符即中止发布，不产出 GitHub Release。同一判定还拒绝 0 字节或非 zip 的发布包
  （`zipfile.is_zipfile` 与文件大小双重判定），空值与格式残缺一律失败。
  `package-windows.py` 只在安装验证记录内打印该值、不写独立 `.sha256` 文件，故独立附件
  由 `check-release.py` 生成。
- **实现状态（显式登记）**：`build-release.yml` 与 `check-release.py` 随本 ADR 同批落盘；
  2026-10-25 已取得远端实跑证据：build-release run `40153942736` success（`package-debug`
  4m48s、`package-release` 7m05s，push main 下 tag 发布 job 按门控 skipped），ci run
  `40153942697` success（五平台与 `measurements` 全绿）。tag 发布路径仍未实跑，保持缺口。
- **验收口径**：构建与发布结论仍以 GitHub Actions 日志与 artifact 为准（ADR-037）；未实跑
  不得宣称通过，本地运行只作复现与排障。

## 正负和解

- **正面**：职责分离后，`ci.yml` 只回答“门禁与测试是否通过”，`build-release.yml` 只回答
  “构建形态是否成立、发布产物是否可安装”；PR 阶段即可审计完整发布链路而不发布任何东西；
  tag 发布有独立入口与最小权限，`contents: write` 不再出现在每次 push 的工作流里；
  artifact 名称不变，历史引用与下游取用无需迁移。
- **负面**：每个 PR 多消耗两条 Windows runner 构建（debug 与 release 各一条），配额与排队
  时间上升；发布形态的成本不止这两条构建，还包括发布构建、打包安装验证、TUI／Web 冒烟
  与两次 artifact 下载（发布包与安装验证记录），tag 触发的发布作业同样占用 runner 与时间；
  发布链路与测试门禁分处两份文件，引用步骤时需跨文件核对，单点阅读不再完整；
  托管 runner 上的打包冒烟仍是非交互环境结果，压缩包在真实桌面上的行为仍需人工复核。
- **负面（tag 推送风险）**：触发条件为 `push` tag `v*`，任何版本前缀匹配的 `v*` tag
  （包括批量 `git push --tags` 一并推送的历史或无关 tag）都会触发构建矩阵与发布作业；
  发布负责人只应推送打算发布的 tag，批量推送前须先确认目标 tag 集合；如需人工审批，
  可另立 environment 保护规则，本 ADR 未纳入（属后续决策）。
- **承担者**：CI 与门禁维护者（维护两份文件的引用一致性）、发布负责人（tag 命名与版本对齐）、
  发布产物复核人（真实桌面冒烟）。
- **和解理由**：以“每个 PR 两条构建 + tag 发布链运行”的成本，换取发布链路在合并前可见、
  tag 发布权限收敛到单一作业；这比继续把发布嵌在门禁工作流里更符合“最小权限 + 可审计”，
  也比新增发布专用脚本链更少改动面。

## 三重时间

- **过去来源**：ADR-037 把测试合并进单份 `ci.yml` 后，构建与发布仍嵌在 `platform-windows`
  条目内，发布链路因此与门禁共用同一 job 身份与权限；2026-10-25 用户指令要求把构建与
  release 移到独立的矩阵 CI。
- **现在证据**：当前 `ci.yml` 的 Windows 条目步骤序列（构建 → 依赖记录 → 发布构建 → 打包
  安装验证 → TUI 冒烟 → Web 冒烟 → 三处 artifact 上传）即为被迁移对象；`package-windows.py`
  的版本解析与 zip 命名（`whcode-<version>-windows-amd64.zip`）是 tag 校验的事实依据。
- **将来失效条件**：见“退出条件”与“复审触发”；任一成立即须修订本 ADR，不得默认沿用。

## 复审触发

- tag 发布策略变化（如引入签名、校验清单、发布说明模板或资产命名变更）；
- runner 配额或单 job 6 小时上限变化，导致矩阵成本或超时语义改变；
- PR 阶段发布链路频繁因环境原因失败且无法归因（此时须区分环境限制与产品缺陷，不得放宽判据）；
- `action-versions.json` 的联网审计连续失败，固定 SHA 与最新版本要求无法维持。

## 修订记录

- **2026-10-25（首版）**：确立构建与发布独立矩阵工作流，工作流职责由两份变三份。
- **2026-10-25（实跑取证）**：build-release run `40153942736` success，ci run `40153942697`
  success；`package-debug`／`package-release` 均通过，发布 job 因非 tag 被门控跳过；
  artifact 四件齐备。tag 发布路径（Release 创建／附件上传／幂等重跑）未实跑，保持缺口。
- **2026-10-25（修订）**：修正 SHA256 段与代码事实不符的表述——`check-release.py` 用
  `hashlib` 独立复算，发布作业新增下载 `install-verify-windows-amd64` 并与记录首部行的包名、
  字节数、sha256 交叉校验，失败即中止；补记发布形态的真实成本（发布构建、打包安装验证、
  TUI／Web 冒烟、两次 artifact 下载）与 `v*` tag 批量推送触发发布的风险及操作纪律；
  登记发布包有效性（非空、有效 zip）与 `.sha256` 两字段判据。
- **2026-10-28（ADR-043：ARM64 构建交付目标）**：矩阵由两档改三档（debug/amd64、
  release/amd64、release/arm64，arm64 走 `windows-11-arm` 原生 runner，打包与安装验证在
  原生宿主上取得真实运行证据）。发布作业下载双架构资产，`check-release.py --arch` 逐架构
  校验后上传两个 zip 与两个 `.sha256`；安装验证记录改按架构命名
  `install-verify-windows-{amd64,arm64}.txt`（决策 7 的“名称保持不变”仅 amd64 保留，
  `smoke-evidence` 更名 `smoke-evidence-windows-<arch>`，属 ADR-043 批准的命名修订）。
  ARM64 结论只作平台证据，不进入验收（ADR-013 不变）。
