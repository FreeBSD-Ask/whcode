# 发布检查表（G5；v0.1.0，2026-11-16）

本文件是 G5（发布）的**可判定检查表**：每个条目给出判据、命令与期望证据。发布前逐项
执行并留证；任一条不满足即停止发布（不得以“已知问题”跳过，只能先修、先登记或按
ADR 修改判据）。

**适用版本**：`0.1.0`（M0 工程侧基线；此前预发布标签为 `v0.1.0-m0`—`v0.1.0-m0.6`）。
**发布动作**：推送 `v0.1.0` 标签触发 `.github/workflows/build-release.yml`。

## 1. 版本与账本一致

| 判据 | 命令 | 期望 |
|---|---|---|
| 工作区版本为 0.1.0 | `cargo metadata --no-deps --format-version 1` | `whcode` 包版本 `0.1.0` |
| CHANGELOG 有发布节 | `grep -n "^## \[0.1.0\]" CHANGELOG.md` | 命中 `## [0.1.0] — 2026-11-16`，且 `## [Unreleased]` 为空节 |
| 十三套 README 版本行一致 | `grep -l "0.1.0" README*.md \| wc -l` | 13 |
| 三方追溯与计数 | `python .github/scripts/check_counts.py`、`python .github/scripts/traceability.py` | `counts consistent`、`追溯一致：无断链` |
| 任务无未勾项 | `grep -c "^- \[ \]" specs/001-whcode-harness/tasks.md` | `0` |

## 2. 验收目标与平台证据

| 判据 | 命令／入口 | 期望 |
|---|---|---|
| 唯一验收目标（Windows 11 amd64，`x86_64-pc-windows-gnullvm`） | `platform-verification.yml` 的 `platform-windows` job | success（ADR-073） |
| FreeBSD 硬门禁 | 同工作流 `platform-freebsd` job | success（ADR-013 修订） |
| ARM64 构建交付 | `build-release.yml` 的 `release/arm64` lane | success（不入验收，ADR-043／074） |
| 五平台证据 | `platform-verification.yml` 全 job | success 或按登记 skipped（NetBSD 仅证据） |

**已知间歇项**：A2（FreeBSD `whcode-bin` 测试二进制退出路径 SIGSEGV）仍 open（monitoring）；
红时按 `docs/guides/long-line-plan.md` A2 行判读并重跑，不得改写门禁语义。

**证据节奏（ADR-082，2026-11-16）**：矩阵只随**代码推送**产生证据；纯文档推送跑
`docs-gates` 十项门禁；`extreme_scale`（1M 事件）与 NetBSD 由每周 schedule／手动 dispatch
专项覆盖。核对发布提交时必须确认该提交含代码改动并引用其 run 号；**被取消的运行不是绿**，
不得作为证据。

## 3. 交付物与安装验证

| 判据 | 命令／入口 | 期望 |
|---|---|---|
| 发布包与 SHA256 对齐 | `build-release.yml` 内 `check-release.py --tag v0.1.0 --arch amd64 --record …` | 退出 0（包名／tag／sha256／记录四方一致） |
| 安装验证（仓库外解包运行） | 同工作流 `package-windows.py --verify` | 三条命令有可归因信号、库文件生成 |
| 本机记录（amd64 gnullvm） | `python .github/scripts/package-windows.py --target x86_64-pc-windows-gnullvm --verify --record artifacts/install-verify-windows-amd64.txt` | 记录含 `target_triple = x86_64-pc-windows-gnullvm`（第 4 行；前 3 行是标题、utc、package），且三条命令各有可归因信号 |
| 可重现构建 | `WHCODE_BUILD_STAMP` 由 CI 从提交时间导出（ADR-047） | 同一提交全新构建字节一致 |

## 4. 安全与告警

| 判据 | 命令 | 期望 |
|---|---|---|
| Code Scanning 零 open | `python .github/scripts/check-code-scanning.py` | `0 条 open 告警` |
| 凭据不入库 | `git grep -nE "sk-[A-Za-z0-9]{16,}" -- . ':!local'` | 无命中 |
| 依赖与 action 版本 | `python .github/scripts/check-actions.py` | 通过（最新版本＋固定 SHA） |
| 测试进程资源 | `python .github/scripts/benchmarks/measure-resources.py` | M1/C4 达标 |

## 5. 预算与性能

| 判据 | 入口 | 期望 |
|---|---|---|
| 冻结预算实测 | `perf-budget`（workflow_dispatch、`perf=true`） | 绝对预算步在参考机口径达标（托管 runner 差异按 ADR-025 登记） |
| 压力通道（记录型） | 同 job `measure-memory-pressure.py` | 输出 `RECORDED`；无 `PASS/FAIL` |
| 稳定性 | NFR-204 24 h 深检结论 `artifacts/nfr204-verdict-24h-2026-11-02.md` | `verdict=通过` |

## 6. 发布说明必须列出的已知限制

1. **V-004 外部 SSH 节点未取证**：M7 服务端与回环、real-sshd 互操作 GO；跨机节点仍延后
   （ADR-013／ADR-016 条件）。
2. **真实 LLM 供应商客户端装配未交付**：测试用 SenseNova 冒烟在案（FR-764），产品级客户端
   装配属后续批次。
3. **5.5 小时 CI 长跑未取得合格数据**：NFR-204 以 24 h 本机深检收口（ADR-025 修订）；
   `long-run.yml` 窗口对 0.5 MB/h 无分辨力，结论口径不得改写。
4. **A2**：FreeBSD 测试二进制退出路径间歇 SIGSEGV（monitoring）。
5. **FTS 未启用**：按 ADR-076 触发条件（消息表 >100 万行且检索 p95 >200 ms）再评估。
6. **平台范围**：Windows 11 amd64 为唯一验收目标；FreeBSD 为硬门禁证据通道；其他 BSD
   仅作 SSH 远程开发目标（ADR-013 修订）。
7. **桌面型测试在托管 runner 上主题循环判红（开放观察）**：weekly schedule 的 `desktop`
   job（run `43855538365`，`c9a618c`）`gui-probe.py` 的 `theme_cycle.ok` 对四套主题均为
   false（`actual_themes` 出现 `null`、后两帧像素全零，截图疑似抓到重绘前空帧），重跑同
   现象。交互式主题切换的人工证据仍按 V-003 口径认定，托管 runner 的非交互会话截图时序
   为待归因候因；ADR-027 复审触发已命中，未改写预算或判据（ADR-082 不改变其周期）。

## 7. 发布后监控

- `platform-verification.yml` 随 `main` 运行（含 FreeBSD 硬门禁）；
- `code-scanning` job 每周 schedule 复核零告警（ADR-037／§5.2）；
- `perf-budget` 与 `desktop` 按需人工触发；
- `action-audit` 每周联网复核 action 版本（§5.1）。

## 8. 失效条件与复审

- 版本号递增后本检查表须随新版本重写（不沿用旧证据）；
- 验收目标、硬门禁或预算口径变更（ADR 修订）后，本表判据同步修改；
- 发布流程本身（build-release.yml 结构）变更后，`check-release.py` 与本节须同批复核。
