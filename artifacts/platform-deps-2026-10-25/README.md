# 平台依赖与数据库冒烟证据（2026-10-25，CI 平台证据）

本目录收口 Check 20／21／22 的判据：依赖记录四类版本、工具链一致性、数据库冒烟。
判定只采用 GitHub Actions 实跑日志与 artifact，不采用本地推测、交叉编译或单平台替代。
取证日期：2026-10-25；复审触发：下一次 `ci.yml` 的 FreeBSD／NetBSD job 实跑，或
`rust-toolchain.toml`／`Cargo.toml` 的 `rust-version` 变更。

## 1. 取证来源

| 项 | 值 |
|---|---|
| 仓库 | `FreeBSD-Ask/whcode` |
| workflow run | `40159516841`（`ci`，结论 `success`） |
| commit | `916d7a668d235141ab9628337dd56ba5ef99c7ff` |
| 事件／时间 | `push`；创建于 `2026-10-25T03:47:38Z` |
| FreeBSD job | `platform-freebsd`，id `121067943566`，`success`（`03:47:40Z` → `03:53:52Z`） |
| NetBSD job | `platform-netbsd`，id `121067943414`，`success`（`03:47:40Z` → `03:54:12Z`）——success 来自 MSRV 早退 `exit 0`，**不代表测试通过** |
| FreeBSD 依赖 artifact | `deps-freebsd`，artifact id `12586675042`，7179 B（zip） |
| NetBSD 依赖 artifact | **不存在**；该 run 的 artifact 仅 `deps-freebsd` 与 `ci-measurements`（`12586761681`） |
| 落盘副本 | `artifacts/deps-freebsd.txt`（由 artifact 原样解包，57 192 B，与解包件逐字节一致，未改任何数值） |
| 本地留档 | `local/deps-freebsd.zip`、`local/deps-freebsd.txt`、`local/log-freebsd-40159516841.txt`、`local/log-netbsd-40159516841.txt`、`local/run-jobs.json`、`local/run-artifacts.json` |

说明：`12586761681` 不是 `deps-freebsd`，而是 `ci-measurements`（14 302 B，与本次判据无关）。

## 2. 四类版本摘要

### 2.1 Windows（`artifacts/deps-windows.txt`，`2026-10-24T10:58:51Z`，`record_deps.py`）

| 类别 | 值 |
|---|---|
| OS | `Windows 11` `10.0.26300`，`host_triple = x86_64-pc-windows-msvc` |
| Rust | `pinned_channel = 1.99.1`；`rustc 1.99.1 (58cf8f9c3 2026-10-13)`；`cargo 1.99.1 (5f94df478 2026-08-27)` |
| 编译器 | MSVC `14.44.35207`；Windows SDK `10.0.26100.0` |
| DuckDB | `prebuilt_version = 2.0.0`；`engine_version_runtime = v2.0.0`；crate `duckdb v1.20000.0`；`payload_sha256 = 8785bcbb…39897` |

### 2.2 FreeBSD（`artifacts/deps-freebsd.txt`，`2026-10-25T03:51:49Z`，`record-deps.sh`）

| 类别 | 值 |
|---|---|
| OS | `FreeBSD 15.1-RELEASE-p3`，`machine = amd64`，`host_triple = x86_64-unknown-freebsd` |
| Rust | `pinned_channel = 1.99.1`；`rustc 1.99.1 (58cf8f9c3 2026-10-13)`（host `x86_64-unknown-freebsd`）；`cargo 1.99.1 (5f94df478 2026-08-27)` |
| 编译器 | `FreeBSD clang version 19.1.7`（`cc`／`cxx`／`clang` 三项同值） |
| DuckDB | crate `duckdb v1.20000.0`、`libduckdb-sys v1.20000.0`（`[dependency_tree]`）；`[duckdb]` 段的 `prebuilt_version` 与 `engine_version_runtime` 均为 `<未取得证据…>` 占位符 |

FreeBSD 侧 DuckDB 的补充事实（来自 job 日志，**不属于** deps 记录文件内容）：
`pkg` 安装了 `databases/duckdb` `1.5.6`（`FreeBSD:15:amd64`，提供 `libduckdb.so`），
`libduckdb-sys v1.20000.0` 于 `03:51:50Z` 开始编译并在 `03:53:11Z` 前完成链接构建。

### 2.3 NetBSD

无依赖记录文件。job 日志可读到的事实（**不是** `deps-netbsd.txt`，不能替代判据）：

| 类别 | 值 |
|---|---|
| OS | VM `Release "11" resolved to "11.0"`，`Target OS: netbsd, Release: 11.0` |
| Rust | `rustc 1.96.1 (31fca3adb 2026-06-26)`（host `x86_64-unknown-netbsd`）；`cargo 1.96.1 (356927216 2026-06-26)`；LLVM `21.1.8` |
| 编译器 | 日志未单独记录 C／C++ 编译器版本；`pkgin` 安装 `cmake`、`ninja`、`rust` |
| DuckDB | `pkgin` 安装 `duckdb-1.5.5.tgz`（未进入构建／测试） |

## 3. 门禁与测试结果（原文）

### 3.1 FreeBSD：`GATE_RESULT=NO-GO`

```text
2026-10-25T03:53:11.7045857Z error: could not compile `softbuffer` (lib) due to 25 previous errors
2026-10-25T03:53:11.7104729Z warning: build failed, waiting for other jobs to finish...
2026-10-25T03:53:13.7565934Z GATE_RESULT=NO-GO（版本或 ABI 不匹配，属 R-01 预期风险路径）
```

口径偏差登记（0 信任复核）：该标签写“版本或 ABI 不匹配”，但日志显示的真实原因是
`softbuffer v0.4.8` 在 FreeBSD 上缺少可用后端导致 25 个编译错误（`E0004`／`E0282`／
`E0283`／`E0392`）。`softbuffer` 经 `tests` 的 dev-dependency → `whcode-gui` → `iced` 引入，
并非 DuckDB 依赖。阶段划分：

- `cargo build --locked --workspace --exclude whcode-gui --exclude whcode-bin`：`03:53:11.042Z`
  `Finished dev profile … in 1m 20s`，即含 `whcode-store` 的 dev 构建与 DuckDB 链接**成功**；
- 随后的 `cargo test` 在编译 dev 依赖 `softbuffer` 时失败，`&&` 短路使测试一个都没有执行，
  因此 `whcode-store` 的建库／事务写读／关闭重启／迁移用例**未运行**。

非 DuckDB 的包级测试（`cargo test -p whcode-domain -p whcode-platform -p whcode-speckit -p whcode-llm -p whcode-runtime`）全部通过，
日志中五项 `test result: ok`：51、39、19、43（1 ignored）、22，无 `failed`。

### 3.2 NetBSD：SKIP，未到达依赖记录与门禁

```text
2026-10-25T03:54:08.5878234Z SKIP：pkgsrc rust 1.96.1 低于 MSRV 1.99（环境限制，不作代码结论）
2026-10-25T03:54:09.1694161Z ##[warning]No files were found with the provided path: artifacts/deps-netbsd.txt. No artifacts will be uploaded.
```

SKIP 发生在 `=== 依赖记录（V-001）===` 与 DuckDB Go/No-Go 之前，故该 job 既无
`deps-netbsd.txt`，也无 `GATE_RESULT` 行，更没有执行任何 `whcode-store` 用例。

## 4. 判定表

| Check | Windows | FreeBSD | NetBSD | 结论 |
|---|---|---|---|---|
| 20 依赖记录四类版本 | 已取得（四类齐备） | 部分取得：OS／Rust／编译器齐备；DuckDB 仅有 crate 版本，引擎／预编译版本为占位符 | 未取得（无记录文件） | **维持未勾选** |
| 21 工具链锁定与记录一致 | 一致（1.99.1） | 一致（`pinned_channel` 与 `rustc` 均 1.99.1） | 无法校验：无记录；环境中 `rustc 1.96.1` ≠ 锁定 1.99.1 | **维持未勾选** |
| 22 数据库冒烟留日志 | 已覆盖（`whcode-store` 内联用例） | 未通过（`GATE_RESULT=NO-GO`，冒烟用例未执行） | 未取得（SKIP，未执行） | **维持未勾选** |

## 5. 未取得项与恢复路径

1. **NetBSD 依赖记录（Check 20／21）**：`ci.yml` 自 commit `0e4af3a` 起已有
   `deps-netbsd` 上传步骤（`actions/upload-artifact@cf430e03… # v7.0.2`），缺失原因不是
   缺上传步骤，而是 job 在 `SKIP：pkgsrc rust 1.96.1 低于 MSRV 1.99` 处 `exit 0`，
   依赖记录脚本根本没执行。恢复路径：把 `record-deps.sh` 移到 MSRV 闸门之前（只记录环境，
   不构成代码结论），或让 NetBSD 侧改用 rustup 安装锁定工具链（与 FreeBSD 同构）——
   两者都属 `ci.yml` 语义变更，需另行决策并实跑取证。
2. **FreeBSD DuckDB 版本类别（Check 20）**：`record-deps.sh` 在 DuckDB 段写占位符，因为
   `DUCKDB_LIB_DIR`／`DUCKDB_INCLUDE_DIR` 在记录步骤之后才导出。恢复路径：在
   `=== 依赖记录 ===` 之前导出这两个变量，并让脚本记录 `pkg info duckdb` 的版本。
3. **FreeBSD 数据库冒烟（Check 22）**：`cargo test --workspace` 被 `tests` → `whcode-gui`
   → `iced` → `softbuffer` 的 FreeBSD 编译失败阻断。恢复路径：让 BSD 侧测试排除 `tests`
   包或为 `softbuffer` 提供 FreeBSD 后端（上游能力问题），再把 `whcode-store` 冒烟日志
   落进 artifact。**不得**把 `cargo build` 成功冒充冒烟通过。
4. **NetBSD 数据库冒烟（Check 22）**：依赖第 1 项与一份可在 NetBSD 上编译的 DuckDB
   （`pkgin` 提供的 1.5.5 未进入构建），并用 `GATE_RESULT=GO` 与 `whcode-store` 用例日志
   作为判据。

## 6. 边界

- **入库状态**：`.gitignore` 以 `/artifacts/*` 忽略整个目录，并对个别证据逐个放行；本目录与
  `artifacts/deps-freebsd.txt` 当前**未在放行名单内**（`git check-ignore` 命中第 7 行），
  因此只存在于工作树，不加 `!` 例外就无法被提交。追加例外行属于本次任务边界之外的文件改动，
  需由后续批次在 `.gitignore` 增加 `!/artifacts/deps-freebsd.txt` 与
  `!/artifacts/platform-deps-2026-10-25/` 后再提交。
- ADR-011 修订把 OpenBSD／DragonFly 移出原生目标，本目录不对其作依赖记录或冒烟判定；
  `ci.yml` 中两者的 job 仍会运行，仅作平台证据留档。
- 本次只回答 Check 20／21／22 的取证状态；不改变 `tasks.md`、不改变平台承诺，
  也不改变 ADR-025 的测量口径。
- 旁证：commit `8b1022b` 的 run `40177045322` 同样只有 `deps-freebsd`（id `12593017202`），
  无 `deps-netbsd`；该 run 截至取证时仍在进行（`platform-windows` 失败、`measurements` 未完成），
  故未用作判据。

## 第二轮（2026-10-25，run 40183376819，管线修复后）

- **FreeBSD**：`deps-freebsd`（artifact `12595117841`）四类版本齐备——`version_source = pkg:pkg`，
  DuckDB `1.5.6`（`engine_version_source` 标注“doctor 未运行，退回已核对库版本”），
  payload 取自 `/usr/local/lib`。**Check 20 的 FreeBSD 侧满足**；Check 21 一致（rustc 1.99.1）。
- **NetBSD**：`deps-netbsd`（artifact `12595495649`）已生成（此前从未产出）；
  `[toolchain]` 记录 `rustc 1.96.1`，**与锁定 1.99.1 不一致**（pkgsrc 环境限制）；
  `[duckdb]` 仍为占位符——记录脚本误用 `pkg_info info duckdb`（NetBSD 无 `info` 子命令），
  已在本次提交改为按工具区分调用，并在 NetBSD 段前置导出 `/usr/pkg/lib`；待下一轮 CI 复核。
- **Check 22（数据库冒烟）**：FreeBSD `STORE_GATE_RESULT=NO-GO`——`cargo test -p whcode-store`
  109 通过／5 失败，失败集中于 `compact`（2）、`schema`（1）、`smoke`（1）等，
  `smoke_reports_engine_version_from_runtime` 明确报 `主版本应为 2：v1.5.6`；
  根因是包版 DuckDB 1.5.6 与冻结引擎 2.0.0（ADR-034）不一致，属 R-01 风险的现实化，
  不是 GUI 编译干扰。NetBSD 仍在 MSRV 闸门 SKIP。
- **Windows job 状态**：本 run 的 `platform-windows` 失败于并行会话 TLS 批次
  （`crates/whcode-llm/src/http/tls.rs` 六个测试），与平台证据管线无关。
