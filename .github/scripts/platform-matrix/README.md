# ci/platform-matrix

平台依赖记录与交叉检查脚本（tasks.md T005/T020/T021；§9、§11.1）。Windows 11 amd64 为唯一验收目标；BSD 延后、仅手工触发（ADR-013）。

**五份 workflow 已迁入 `.github/workflows/`（2026-10-23）**：GitHub Actions 只执行该目录下的文件，
原先放在本目录的 `.yml` 从未被平台执行（Check 3／249 登记的缺陷）。本目录只保留脚本。

| 文件（`.github/workflows/`） | 平台 | 范围（ADR-013） | 状态 |
|---|---|---|---|
| `platform-windows.yml` | Windows 11 x86_64 | **在范围内（唯一验收目标）** | **已接线**；平台执行记录尚未取得（本仓库无远程/runner），本机证据见 `artifacts/install-verify-windows-amd64.txt` 与 `artifacts/m0-report.md` |
| `platform-freebsd.yml` | FreeBSD x86_64 | 延后 | 保留定义，仅 `workflow_dispatch` 手工触发；不作验收 |
| `platform-netbsd.yml` | NetBSD x86_64 | 延后 | 同上 |
| `platform-openbsd.yml` | OpenBSD x86_64 | 延后 | 同上（本就不作原生目标，见 ADR-011） |
| `platform-dragonfly.yml` | DragonFly x86_64 | 延后 | 同上 |

**32 位平台不在范围内**（ADR-013）：不构建、不测量、不提供承诺。

辅助脚本：

| 文件 | 用途 |
|---|---|
| `record_deps.py` | Windows 依赖记录（V-001，含测量纪律字段；本项目不使用 GNU 工具，Windows 侧不依赖 bash） |
| `record-deps.sh` | FreeBSD/NetBSD 依赖记录（POSIX sh） |
| `cross-check.py` | 本机跨目标**代码级**检查（见下方纪律：不构成运行验证） |

发布包脚本在 `ci/` 根级：`ci/package-windows.py`（打包 + 仓库外解包安装验证；T118／Check 118）。

纪律：

- 构建前置：任何构建前必须先 `python ci/fetch-duckdb.py` 拉取并校验预编译 DuckDB（ADR-DB-BUILD）；
- CI 至少分为：构建检查、可运行的原生测试、依赖验证、定期 24/72 小时稳定性测试（§9.1，后两者尚未建立）；
- 平台可构建与 DuckDB 原生构建**分开**验收（ADR-010、V-001/V-002）；
- 依赖记录必须含 Rust/DuckDB/编译器/OS 四类版本（V-001）；
- **交叉检查不等于运行验证**：`cargo check --target <bsd>` 只做类型检查；`whcode-store`（aws-lc-sys）与
  `whcode-ssh`（russh→aws-lc-rs）在缺目标 C 工具链时无法交叉检查，这是记录在案的阻塞（ADR-010、G-01）；
- 不得以 CI 编译通过单独判定平台可交付；不得以单一 BSD 结果代替四 BSD 验收（R-02）；
- OpenBSD/DragonFly 的判据已按 ADR-011 改为“客户端连通性 + 受限命令执行 + 登记”，不得宣称原生能力。
