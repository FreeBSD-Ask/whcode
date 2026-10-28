# ADR-043：Windows ARM64 构建交付目标（验收口径不变）

- **状态**：已批准（2026-10-28，用户指令“支持 windows arm64 构建，并在 CI 测试通过发行新的 release”）
- **来源**：用户指令；ADR-013（唯一验收目标）、ADR-039（构建与发布矩阵）、ADR-DB-BUILD（预编译 DuckDB 口径）
- **责任人**：CI 与门禁维护者、发布负责人
- **退出条件**：用户决定把 ARM64 提升为验收目标（须另立 ADR 并按 ADR-013 退出条件补齐完整证据链）；
  `windows-11-arm` 托管 runner 下线或配额不可接受；DuckDB 官方停止提供 Windows ARM64 预编译载荷

## 1. 事实与证据

- GitHub 提供 `windows-11-arm` 托管 runner（公开预览），可原生运行 `aarch64-pc-windows-msvc` 二进制；
  本 ADR 批准日其可用性与配额存在预览期风险，故 ARM64 结论只作平台证据，不进入验收。
- DuckDB 2.0.0 官方提供 Windows ARM64 预编译载荷 `duckdb-shared-libs-windows-arm64.tar.gz`
  （18 389 170 字节，下载后复算 SHA256 `159ce60e…7447`，记录于 `fetch-duckdb.py`），
  与 amd64 载荷并存即可支撑双架构链接。
- 现行口径（ADR-013）：Windows 11 amd64 是唯一验收目标，其他平台结论一律不进入验收结论。
- 现行口径（ADR-039）：构建矩阵只有 `windows-latest` 单架构（debug/release 两档），
  发布资产为 `whcode-<version>-windows-amd64.zip` 与其 `.sha256`。
- 现行口径（ADR-DB-BUILD）：`vendor/duckdb/<版本>/` 单目录存放预编译载荷，仅覆盖 amd64。

## 2. 决策

1. **ARM64 定位为构建交付目标，不是验收目标**：产出可下载、可安装验证的
   `whcode-<version>-windows-arm64.zip`，但 ARM64 的一切结论（测试、冒烟、安装验证）只作
   平台证据，不进入验收；验收仍是 Windows 11 amd64（ADR-013 不变）。
2. **ci.yml 平台矩阵新增 `windows-arm64`（`windows-11-arm` 原生 runner）**：全量测试、
   内置 SSH 回环验收、运行时 feature 门控（T110／ADR-029）与依赖记录（V-001）在原生
   ARM64 上真实运行并上传 artifact；架构无关门禁（fmt／clippy／EOL／计数／追溯）不重复执行。
   Windows x64 lane 另加 ARM64 交叉 `cargo check --target aarch64-pc-windows-msvc`，
   只证明代码级可构建性，**不作为平台结论**（与 ADR-013 决策 4 口径一致）。
3. **build-release.yml 矩阵改为三档**（`fail-fast: false`）：
   - `debug/amd64`（windows-latest）：依赖图可解 + 依赖记录；
   - `release/amd64`（windows-latest）：发布构建 → 打包安装验证 → SHA256 → TUI／Web 冒烟；
   - `release/arm64`（windows-11-arm 原生）：同上链路，打包与安装验证在原生 ARM64 runner
     上取得真实运行证据。
   发布作业下载双架构资产，逐架构用 `check-release.py --arch` 校验后上传两个 zip 与
   两个 `.sha256` 至 GitHub Release。
4. **预编译 DuckDB 按目标三元组分目录**：`vendor/duckdb/<版本>/<三元组>/`，同版本多架构
   载荷可共存；`fetch-duckdb.py` 按主机三元组推断或 `--target` 显式指定，SHA256 与字节数
   仍固定并强制校验。`.cargo/config.toml` 的 `[env]` 表继续指向 amd64 默认目录；
   交叉构建或原生 ARM64 宿主由**环境变量**导出 `DUCKDB_LIB_DIR` 覆盖
   （cargo `[env]` 不覆盖已存在的 OS 环境变量），CI 工作流已按此约定。
5. **包命名带架构标识**：`whcode-<version>-windows-<arch>.zip`（arch ∈ amd64|arm64）。
   `package-windows.py` 增加 `--target`，exe 取自宿主构建 `target/release/` 或交叉构建
   `target/<三元组>/release/`；`--verify` 只在宿主架构与目标一致时放行，交叉产物不得
   在异构宿主上运行（不冒充平台结论，信号链完备原则）。
6. **`check-release.py` 增加 `--arch`**：无 `--arch` 时逐个校验全部架构（每架构包唯一）；
   `--record` 必须与 `--arch` 同用，安装验证记录按架构一一对应交叉校验。
7. **依赖记录按宿主架构记录载荷**：`record_deps.py`／`record-deps.sh` 优先取当前宿主
   三元组的 vendor 载荷，交叉构建载荷由其所属平台记录。

## 3. 正负和解

- **正面**：ARM64 设备用户取得可下载、可安装验证的构建交付物；原生 runner 上的全量测试、
  SSH 回环与冒烟提供真实平台证据；代码级交叉检查可在 amd64 lane 廉价暴露目标相关编译错误。
- **负面**：CI 时间与配额上升（每 push 多一条 ARM64 测试 lane 与一条 release 构建；
  `windows-11-arm` 属公开预览，存在排队与下线风险）；发布资产翻倍（双 zip + 双 sha256）；
  脚本与工作流需维护两架构路径，口径面变宽。
- **承担者**：CI 与门禁维护者（lane 与脚本维护）、发布负责人（双架构校验与发布）、
  ARM64 设备用户（真实环境反馈，属平台证据而非验收义务）。
- **和解理由**：ARM64 只增加“构建交付 + 平台证据”，不扩大验收承诺、不改 ADR-013 的唯一
  验收口径；预览 runner 的失效条件已写入退出条件，风险可观测、可回退（删除 lane 即可）。

## 4. 三重时间

- **过去来源**：ADR-013 收敛为 Windows 11 amd64 唯一验收；ADR-039 建立单架构构建发布矩阵；
  ADR-DB-BUILD 的 vendor 单目录只覆盖 amd64。
- **现在证据**：`windows-11-arm` 托管 runner 可用；DuckDB 2.0.0 官方 Windows ARM64 载荷
  实测存在（SHA256 与字节数已复算）；本 ADR 与代码改动同批落盘。
- **将来失效条件**：`windows-11-arm` runner 退出预览后仍需复审其配额与可用性；
  DuckDB 停止发布 ARM64 载荷或升级后载荷缺失；用户决定 ARM64 进入验收（须新 ADR 与
  完整证据链，含性能分位数与长时稳定性，ADR-013 退出条件）。

## 5. 复审触发

- 每次 release 前核对 `windows-11-arm` 可用性与最近一次 ARM64 lane 结果；
- DuckDB 版本升级时核对 ARM64 载荷的 SHA256 与字节数；
- CI 配额或排队时间显著恶化、预览 runner 出现持续不可用；
- 任何 ARM64 结论被引用为验收结论时（违规，须立即纠正）。

## 6. 修订记录

- **2026-10-28（首版）**：确立 ARM64 构建交付目标；ci.yml 与 build-release.yml 三档矩阵、
  多三元组 vendor 布局、`--target`／`--arch` 脚本口径与双架构发布同批落盘；待 CI 实跑取证
  （ADR-037：以 GitHub Actions 日志与 artifact 为准）。
- **2026-10-28（实现细化）**：`build-release.yml` 的 release/arm64 lane 采用**原生宿主构建**
  （`windows-11-arm` runner 上不传 `--target`），产物落 `target/release/`，与
  `package-windows.py` 的 `binary_path`（target==host 时取 `target/release/`）保持一致；
  交叉 `--target` 构建仅保留在 `ci.yml` 的代码级 `cargo check`（不打包，无路径歧义）。
  首版草案曾用 `--target` 交叉构建 arm64 发布产物，但在原生 ARM64 宿主上会造成
  `target/<三元组>/release/`（构建）与 `target/release/`（打包期望）两套布局打架，
  属实现期发现的缺陷，已改为原生宿主构建消除。
- **2026-10-28（CI 实跑缺陷与修复）**：首轮推送 run `40691533216`（build-release）与
  `40691532314`（ci）的 `windows-arm64` lane 均失败，症状为 `0xC0000135`
  （`STATUS_DLL_NOT_FOUND`）：打包与安装验证通过（包内有 dll），但 `target/release/` 与
  `target/debug/deps/` 缺 `duckdb.dll`，TUI 冒烟与 `whcode_app` 测试二进制无法启动。
  根因：工作流在步骤 `env` 里显式导出的 `DUCKDB_LIB_DIR` 是**相对路径**，而
  `crates/whcode-store/build.rs` 用 `Path::new(lib_dir).join("duckdb.dll").is_file()` 判定
  时，构建脚本进程的 cwd 是 crate 目录，相对路径被判为不存在而**静默跳过复制**；链接期
  rustc 的 cwd 是工作区根，同一相对路径却能命中，于是"编译通过、运行期缺 dll"。
  amd64 lane 未触发，是因为它走 `.cargo/config.toml` 的 `relative = true`（cargo 解析为
  绝对路径）。修复：`build.rs` 把相对 `DUCKDB_LIB_DIR` 按**工作区根**解析（从
  `CARGO_MANIFEST_DIR` 上溯含 `[workspace]` 的 Cargo.toml），与 cargo 的 `relative = true`
  同口径；已本地验证相对路径下 release/debug 均能复制 dll、TUI 冒烟三项通过。
