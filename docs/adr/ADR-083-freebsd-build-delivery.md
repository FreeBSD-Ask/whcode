# ADR-083：FreeBSD 构建交付目标（发布包）

- **状态**：已批准（2026-11-17；用户问「release 为何没有 FreeBSD 包」并听取升级方案后回复
  「全部批准」；ADR-013 同日追加修订记录）
- **来源**：v0.1.0 发布事实（`docs/guides/release-checklist.md`：产物仅 amd64／arm64 两个
  zip）；ADR-013 修订史（2026-10-28 ARM64 升交付目标、2026-10-31 FreeBSD 升完整平台证据
  通道）；`platform-verification.yml` FreeBSD lane（全量构建＋全量测试＋三门＋冒烟的全绿
  既有能力，run `43902805603` 等）；`.github/scripts/platform-matrix/bsd-duckdb.sh`（DuckDB
  2.0.0 源码构建与 env 约定）；`package-windows.py`（白名单打包与安装验证判据）
- **责任人**：发布维护者、平台维护者
- **退出条件**：见 §8（绑定命令与判定规则）

## 0. 任务认知卡（宪法原则 X）

- **方向**：GitHub Release 缺 FreeBSD 包——用户确认这是范围问题而非缺陷，并批准升级；
- **目标**：tag／手动发布产出 `whcode-<version>-freebsd-amd64.tar.gz`＋`.sha256`，经仓库外
  安装验证与 TUI／Web 冒烟后挂上 GitHub Release；
- **判据**：release job 数量断言通过、`check-release.py --arch freebsd-amd64` 四方一致、
  VM 内三条命令各有可归因信号；
- **非目标**：FreeBSD 验收结论（仍是 Windows 11 amd64）、性能分位数（perf 通道仍仅
  Windows）、NetBSD／OpenBSD／DragonFly 交付、pkg/ports 官方载体、GUI 在 VM 内的窗口级
  验证。

## 1. 问题与边界

现状：`build-release.yml` 的 `package` job 矩阵只有两条 Windows lane；`release` job 只上传
2 zip＋2 sha256。FreeBSD 用户只能按 README 从源码构建（DuckDB 还要先源码构建）。

过去（ADR-013 修订史）：「交付＝Windows、证据＝BSD」的分层是 2026-10-28／10-31 两次用户
指令的和解；交付范围属于用户自由（AGENTS §1.3），本次由用户改变裁量。

边界：验收目标不变；FreeBSD 包是**交付物＋可安装性证据**，不进验收结论；QEMU（vmactions）
环境的证据身份照旧登记。

## 2. 决策

1. **角色升级**：FreeBSD 从「完整平台证据通道（硬门禁）」扩展为「构建交付目标」。证据
   通道职责与硬门禁语义不变——`platform-verification.yml` 随 `main` 的全量构建、全量测试、
   三门与冒烟照旧，失败即红。
2. **新 job**：`build-release.yml` 新增 `package-freebsd`（ubuntu-latest 宿主＋
   `vmactions/freebsd-vm`，release 15、latest pkg、X11／字体依赖面与平台通道同源）；触发
   继承 workflow `on:`（tag `v*` 与 `workflow_dispatch`）。
3. **工具链与构建**：rustup 1.100.0（pkg 的 rust 版本不确定，无法满足可复现要求，与平台
   通道口径一致）；DuckDB 2.0.0 由 `bsd-duckdb.sh` 源码构建（PREFIX
   `/usr/local/whcode-duckdb-2.0.0`），`DUCKDB_LIB_DIR`／`DUCKDB_INCLUDE_DIR` 注入链接；
   release 构建设 `RUSTFLAGS='-C link-arg=-Wl,-rpath,$ORIGIN'`——`libduckdb.so` 与可执行
   文件同目录即可运行，不依赖系统路径。
4. **包形态**：`whcode-<version>-freebsd-amd64.tar.gz`（VM 内系统 tar＝bsdtar，默认
   restricted pax，POSIX 一致）＋`.sha256`（sha256sum 兼容格式）。内容白名单与 Windows 包
   对齐：`whcode`、`libduckdb.so*`（含 SONAME 目标与符号链接）、`README.md`、`LICENSE`、
   `CHANGELOG.md`、`compatibility.md`、`platform-matrix.md`、五份字体许可与来源、
   `PACKAGE-INFO.txt`（脚本生成）。
5. **安装验证**（VM 内、仓库外）：解包到 `/tmp` 随机目录，依次运行 `./whcode --version`／
   `./whcode doctor`／`./whcode doctor --json`，记录退出码与输出摘要，写入
   `install-verify-freebsd-amd64.txt`（首部行与 `package-windows.py` 同 schema：
   `package = <名>（<字节> 字节，sha256=<hex>）`）。三条命令各有可归因信号才判 GO；
   数据落 `/tmp`，不污染仓库。
6. **冒烟**：`tui-smoke.py`／`web-smoke.py` 对包内 release 二进制各跑一遍（证据 JSON 落
   `artifacts/`，随 lane 上传）。
7. **发布挂载**：`release` job 增下载 freebsd 包与安装验证记录；`check-release.py` 扩展
   freebsd tar.gz 模式（`--arch freebsd-amd64`）；GitHub Release 资产变为
   2 zip＋2 zip.sha256＋1 tar.gz＋1 tar.gz.sha256，数量断言同步。
8. **文案收口**：`package-windows.py` 的 amd64 包 `platform_scope` 从「FreeBSD 延后」改为
   「FreeBSD 独立发布包（ADR-083）；NetBSD/OpenBSD/DragonFly 延后」。

## 3. 正负和解

- **正面**：FreeBSD 用户获得可下载、可自验的包；安装验证与冒烟给出可安装性证据；
  Windows 包职责与判据不变。
- **负面／承担者**：tag 构建时长增加（DuckDB 源码构建约 16 分钟＋release 编译，实测基线
  见平台通道），承担者＝发布节奏（低频事件，可接受；不为它加缓存——简陋优先，复审触发
  见 §8）；QEMU 包与物理机存在环境差异（证据身份照旧）；发布资产从 4 件变 6 件，承担者
  ＝Release 页浏览者；A2（测试二进制退出路径 SIGSEGV）属测试面，不影响 release 产物，但
  安装验证若遇退出异常须如实登记、不得冒充通过。

## 4. 三重时间

- **过去**：ADR-013 两次修订＋FreeBSD lane 多轮全绿记录（run `43902805603`：FreeBSD
  25.6 分钟含 DuckDB 源码构建 16.4 分钟）。
- **现在**：`bsd-duckdb.sh`／`tui-smoke.py`／`web-smoke.py`／安装验证记录格式全部已有
  实现，本 ADR 只做组合与判据收口。
- **将来失效条件（绑定命令＋判定）**：`python .github/scripts/check-release.py --dir
  dist --tag <tag> --arch freebsd-amd64 --record <记录>` 红 → 发布中止；vmactions 上游
  变更导致 VM 不可用 → §5 回退；出现 pkg/ports 官方载体需求 → 重开 ADR。

## 5. 回退方案

删除 `package-freebsd` job 与 `release` job 的 freebsd 步骤即回到两平台发布；已发布资产
可保留（tar.gz 无系统级注册，删除解包目录即完全卸载）；ADR-013 修订记录同日登记本条
回退口径。

## 6. 已知限制与失效条件

- **不作 FreeBSD 验收结论**；性能分位数未取得（perf 通道仍仅 Windows）。
- 包内 `libduckdb.so` 为构建时产物，非 FreeBSD ports 官方包；DuckDB 版本升级须同步复审
  本 ADR 与 `bsd-duckdb.sh`。
- 安装验证不含 GUI 窗口路径（VM 无显示器；GUI 验收证据由 Windows 通道覆盖）。
- NetBSD／OpenBSD／DragonFly 不在本 ADR 范围（延后口径不变，仅 SSH 远程开发目标）。

## 7. 文献依据（标准研读记录，§1.6 口径）

- POSIX.1-2017（IEEE 1003.1）tar/ustar 容器格式：VM 内 bsdtar 默认 restricted pax 输出，
  POSIX 一致；判定＝容器仅用于本仓分发，不解析第三方包。登记：`standards-baseline.md`
  同日新增行。
- GNU coreutils `sha256sum` 输出格式（`<hex>  <file>` 双空格）：与 Windows 包 `.sha256`
  同格式，校验链一致。
- Keep a Changelog 1.1.0／SemVer 2.0.0：既有登记沿用（版本标记约定见 CHANGELOG 头）。

## 8. 复审触发（绑定命令＋判定）

- `python .github/scripts/check-workflows.py` 红（含 action SHA 铁律）→ 停止交付。
- `python .github/scripts/check-release.py --dir dist --tag "$GITHUB_REF_NAME"
  --arch freebsd-amd64 --record verify-freebsd/install-verify-freebsd-amd64.txt` 红 →
  发布中止，不得以本地推断冒充。
- 首个 FreeBSD 包经 tag／dispatch 实跑后回填 tasks T369／Check 464；连续两次发布失败 →
  重开 ADR。
