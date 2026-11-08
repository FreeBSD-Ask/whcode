# Windows 11 amd64 兼容矩阵与已知限制（T118）

**版本**：`0.1.0`（M9 工程侧基线，未发布） | **更新**：2026-11-06

## 1. 平台支持

| 平台 | 状态 | 依据 |
|---|---|---|
| Windows 11 amd64（`x86_64-pc-windows-msvc`） | **唯一验收目标**，已验收（M0—M9 测试在本机通过；2026-10-23 进度复核后 `fmt`/`clippy`/`test`/契约/布局门禁全绿） | ADR-013 |
| Windows 11 arm64（`aarch64-pc-windows-msvc`） | **构建交付目标**（ADR-043）：产出 arm64 发布包，在原生 `windows-11-arm` runner 取得平台证据；不进入验收 | ADR-043 |
| FreeBSD | **完整平台证据通道（硬门禁）**（2026-10-31 用户指令解除延后）：platform-verification.yml 随 `main` 全量构建（含 GUI／bin）、全量测试、三门与 TUI/Web 冒烟，失败即红；2026-11-03 起 run `41481740766` 全绿（三门 GO＋冒烟，证据 `artifacts/store-acceptance/`）；**2026-11-06 观察**：run `42143683382` 出现 `store_gate = NO-GO`（DuckDB 源码构建报 `platform.hpp:22 'features.h' file not found`，与 2026-11-03 GO 不一致，**根因未归因**，不得读作代码回归；本行为 2026-11-06 观察记录，未经本地复现 `[待验证]`）；**不作验收结论**，性能分位数未取得 | ADR-013 修订（2026-10-31） |
| NetBSD | **仅支持 SSH 远程开发（被连入目标）＋随 CI 自动运行（仅平台证据）**（ADR-013 修订 2026-11-03）：SSH 回环验收 GO（客户端＋服务端四项，`artifacts/ssh-acceptance/netbsd-client.txt`）、进程组零残留 GO（Check 201）；原生 store/DuckDB 豁免（上游 2.0.0 依赖 glibc `features.h`） | ADR-013、T020/T021 |
| OpenBSD | **仅支持 SSH 远程开发（被连入目标）**（ADR-013 修订 2026-11-03）：节点证据从 Windows/FreeBSD 以内置 SSH 客户端连入取得（V-004 口径，M7 或具备主机时）；工具链阻塞记录（ports rust 1.98.1 < MSRV）已入库 | ADR-011/013 |
| DragonFly BSD | **仅支持 SSH 远程开发（被连入目标）**（ADR-013 修订 2026-11-03，同日追加同待遇）：节点证据从 Windows/FreeBSD 以内置 SSH 客户端连入取得（V-004 口径，M7 或具备主机时）；工具链阻塞记录（dports rust 1.85.1 < MSRV）已入库 | ADR-011/013 |
| 32 位平台 | **不支持** | ADR-013 |

## 2. 已知限制（M9 时点；2026-11-06 复核更新）

1. **性能预算已在 Windows 11 amd64 冻结**（ADR-025，2026-10-23）：NFR-208 概览 p95
   实测 6 071 µs（≤ 50 ms）、NFR-210 写入吞吐中位数 6 921 条/秒（≥ 2 000 条/秒）；
   其余八项**保持候选**并登记责任人与退出条件；BSD 无运行证据，不参与冻结；
   5.5 小时时间序列未取得，已由 24 小时本机深检替代收口（NFR-204，见第 2 条）。
2. **稳定性（NFR-204）已收口**（T116/Check 116/195）：5.5 小时 CI 窗口（ADR-033）对
   0.5 MB/h 无分辨力（run 40529349945 无权判定），按 ADR-025 修订升级为 24 小时本机深检；
   287 样本、多口径一致负斜率、线程／句柄无持续增长，`verdict=通过`（2026-11-02），
   证据 `artifacts/nfr204-verdict-24h-2026-11-02.md`。
3. **真实 LLM Provider 已接四端**（T162—T167，ADR-032 追加／ADR-056）：`llm.run`／
   `llm.cancel` 在 CUI／TUI／GUI／Web 均登记 `Implemented`
   （`docs/matrices/feature-parity.md`）；HTTP 客户端 77 项 `http` 测试与本地回环端到端
   见 `artifacts/llm-e2e-2026-10-25.md`（桩供应商，不访问公网）；取消、重试与超时
   契约见 ADR-050／FR-201—208，真实供应商侧的差异未逐一实测 `[待验证]`。
4. **M7 SSH**：服务端已实现（`serve --ssh`，russh + 公钥认证 + 受限入口 + 单写者执行器，
   ADR-030；进程内 41 项测试）；FreeBSD／NetBSD 回环 GO（客户端＋服务端四项，run
   `41481740766`，证据 `artifacts/ssh-acceptance/`）；**V-004 外部节点现场验收仍开放**
   （ADR-013 修订 2026-11-03：OpenBSD／DragonFly 仅作被连入目标）。
   内置客户端已含 known_hosts 风格 TOFU 校验（ADR-005 未决条件③结清）；客户端默认只放行远端
   `whcode` 命令，任意系统 Shell 需显式 `allow_shell` 并留审计（Check 97）。
5. **验收检查点：376 项中 367 项有复核记录，9 项 missing（Check 347—350、354—358）**
   （`python .github/scripts/check_counts.py`，2026-11-08 实测；证据态 exercised 318、
   outcome-supported 49）。未通过项集中在 Phase 24（ADR-057 ZCode 对齐）：B1 余项与
   B3—B5；逐条以 `checklist.md` 为准，本节不复述其判据。
   历史 CI 记录：run 40126145778（2026-10-25）全绿（五平台与 `measurements`）、
   run 41481740766（2026-11-03）平台通道 success；**CI 结论以最新 run 为准**。
   NFR-207/209 与测试进程 RSS 属 `perf-budget` 受控测量（`workflow_dispatch` + `perf`），
   不作 push 门禁。
   TUI 七视图、Web UI（含会话/任务/清理/运行时写命令）、CUI 命令树、SDD 门禁、原生 GUI 与
   可选运行时（JS/Python/GitBash，feature 默认关闭）已有实现与实测记录；
   **实现完成不等于验收通过**，对外表述不得混淆两者。
6. **V-003 交互四项 / V-004 服务端验收**：V-003 已取得（TUI 交互证据），V-004 服务端现场验收
   因 ADR-013 延后未取得。
7. **DuckDB 单文件持久化**（ADR-001）：`memory_limit` 默认 192MB（§5.4 基线），
   不代表进程 RSS（NFR-201—204）。
8. **桌面形态**：原生 GUI 已实现（`whcode gui`，FR-711—FR-717、ADR-027：iced + `tiny-skia`
   软件光栅、内置 OFL 字体子集）；七视图（ADR-056 起含对话视图并已交互化）、六套主题
   （amoled-night／eink-day／eink-night／terminal-green／zcode-day／zcode-night，zcode 系
   2026-11-06 冻结，默认 `zcode-night`；ADR-057／FR-739）；TUI 仍是默认终端形态，Web UI 是
   **独立浏览器前端**。ADR-026 铁律不变：不得以内嵌浏览器引擎（WebView／WebView2、CEF、
   Tauri／wry、Electron、NW.js）承载桌面界面，“自绘窗口 + 内嵌 Web UI”同样禁止。

## 3. 发布包构成（§12.2）

- 二进制：`whcode`（`whcode-bin`，MSVC 构建，Windows 11 amd64）；
- DuckDB 动态库：`vendor/duckdb/2.0.0/duckdb.dll`（SHA256 校验，`.github/scripts/fetch-duckdb.py`）；
- 许可证：BSD-2-Clause（版权主体 FreeBSD 中文社区）；
- 工具链锁定：Rust 1.99.1（`rust-toolchain.toml`），MSVC 14.44（不使用 GNU 工具链）。

**发布判据**：强制验收（V-001—V-307 中已实现部分）全通过 + 本限制清单随包发布；
预算冻结（ADR-025，2026-10-23）与长时稳定性（NFR-204，2026-11-02）已达成，
其余发布判据未齐前**不发正式版本**（预发布标识 `v0.1.0-m*`）。
