# 发布检查表（G5；v0.1.0，2026-11-16）

本文件是 G5（发布）的**可判定检查表**：每个条目给出判据、命令与期望证据。发布前逐项
执行并留证；任一条不满足即停止发布（不得以“已知问题”跳过，只能先修、先登记或按
ADR 修改判据）。

**适用版本**：`0.1.1`（发布候选，2026-11-22；工程侧基线 M0＋四端对等全量对齐；
此前正式标签为 `v0.1.0`，预发布 `v0.1.0-m0`—`v0.1.0-m0.8`）。
**发布动作**：推送 `v0.1.1` 标签触发 `.github/workflows/build-release.yml`。
**证据状态**：工程侧账本与本地门禁全绿；CI 平台矩阵与发布构建证据待 run 号回填（ADR-037）。

## 1. 版本与账本一致

| 判据 | 命令 | 期望 |
|---|---|---|
| 工作区版本为 0.1.0 | `cargo metadata --no-deps --format-version 1` | `whcode` 包版本 `0.1.0` |
| CHANGELOG 有发布节 | `grep -n "^## \[0.1.0\]" CHANGELOG.md` | 命中 `## [0.1.0] — 2026-11-16`，且 `## [Unreleased]` 为空节 |
| README 版本行一致（中英两套） | `grep -l "0.1.0" README*.md \| wc -l` | 2（2026-11-17 用户指令收敛为中英） |
| 三方追溯与计数 | `python .github/scripts/check_counts.py`、`python .github/scripts/traceability.py` | `counts consistent`、`追溯一致：无断链` |
| 任务无未勾项 | `grep -c "^- \[ \]" specs/001-whcode-harness/tasks.md` | `0` |

## 2. 验收目标与平台证据

| 判据 | 命令／入口 | 期望 |
|---|---|---|
| 唯一验收目标（Windows 11 amd64，`x86_64-pc-windows-gnullvm`） | `platform-verification.yml` 的 `platform-windows` job | success（ADR-073） |
| FreeBSD 硬门禁 | 同工作流 `platform-freebsd` job | success（ADR-013 修订） |
| ARM64 构建交付 | `build-release.yml` 的 `release/arm64` lane | success（不入验收，ADR-043／074） |
| FreeBSD 构建交付（发布包，ADR-083） | `build-release.yml` 的 `package-freebsd` job（tag／手动触发） | success（不入验收）；首个实跑证据待回填（Check 464） |
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
2. **真实 LLM 供应商**：客户端已接四端（T162—T167），真实协议工具调用链路按 ADR-088 收口
   （T384—T389／Check 478—479）；**Anthropic／Gemini 无真实凭据与端点，仅单元级取证**
   （ADR-088 修订，2026-11-21）。字段测试记录见 `artifacts/fieldtest-2026-11-17.md` 及
   `fieldtest2-2026-11-19.md`／`fieldtest3-2026-11-19.md`。
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

### 6.0 四项限制的处置状态（v0.1.1，2026-11-22 用户“解决 3 的限制”）

上述 §6 七条已知限制中，四条被点名要求处理。逐项核查结论如下——**四项均为外部条件，
本机不可解；“解决”在此语境的诚实含义是把前置、承担者与退出条件写清，不伪造通过**
（宪法原则“不伪造”“山洞”）：

| 限制 | 本机核查事实 | 能否本机解决 | 退出条件与承担者 |
|---|---|---|---|
| V-004 外部 SSH 节点 | M7 服务端与回环已 GO（run 41481740766）；跨机节点需真实外部 BSD 主机，本机无可连节点 | 否——需外部节点 | 用户在 ADR-013 口径下提供至少一台外部 BSD 节点并执行 `ssh-acceptance`，证据落 artifacts；承担者＝用户／平台负责人 |
| Anthropic/Gemini 真凭据 | `local/sensenova.env` 仅 SenseNova；无该两家 API key 与端点；ADR-088 修订已把 tools 参数转换做单元级取证 | 否——需凭据与端点 | 用户提供凭据后，按 ADR-088 复审触发 2 重跑单元取证并做真模型 e2e；承担者＝用户／LLM 接入者 |
| A2 FreeBSD 退出路径 SIGSEGV | run 41290577560/41529766439 首现并复现，取证管线（truss-parallel／单线程复跑／VM 标记）已接线；间歇性、并行压力下退出路径崩溃 | 否——需 FreeBSD runner 复现 | 下一次 FreeBSD CI 复现时取 `truss-parallel.log` 定位 atexit／库析构顺序并修复；承担者＝平台负责人；未修复前按 monitoring 判读红，不改写门禁 |
| 桌面主题循环判红（托管 runner） | weekly desktop job run 43855538365 `theme_cycle.ok` 四主题 false（空帧截图）；本机无截屏与按键注入权限（资源专用模式同款限制） | 否——需 runner 交互桌面 | 在有交互桌面的 runner 上重跑 `gui-probe.py` 完整模式，按 V-003 人工证据口径认定；承担者＝CI／GUI 负责人；ADR-027 复审触发已命中但未改写预算 |

四项均不构成 v0.1.1 发布的本地阻断（本机门禁全绿、账本全勾），但必须在发布说明中如实列出，
不得表述为已解决。

## 6.1 本版发布记录（v0.1.1，2026-11-22，发布候选）

| 项 | 事实 |
|---|---|
| 版本 | `Cargo.toml` 工作区版本 0.1.1；README 中英两套版本行一致 |
| 账本 | FR 179／任务 410／Check 488 全勾；`counts consistent`、追溯无断链 |
| 本版范围 | 四端对等全量对齐（ADR-089/091/092）、真实协议工具调用（ADR-088 及修订）、
  GUI 线程预算收敛（ADR-093，空闲 28→22 满足 FR-712）、CUI 写类授权（ADR-090）等，
  详见 CHANGELOG `## [0.1.1]` 节 |
| 本地门禁 | fmt、clippy（`-D warnings`）、workspace 全量测试、layering/layout、
  counts/traceability/knowing-first/adr-limits/docs-style/eol/i18n/frontend-budget、
  coherence pre(反例 0)/mid(0) 全绿；GUI 线程 22≤24、吞吐 4194.0/s≥2000（单机样本，R-12） |
| 标签 | `v0.1.1`（annotated → 提交 `a313a88`，2026-11-22 19:34+08:00 推送） |
| 验收链证据 | **运行中**：`platform-verification` run `45022965966`（main push 触发，
  2026-11-22T12:34:15Z，含 Windows amd64 验收、FreeBSD 硬门禁、ARM64/NetBSD 证据；
  A2 间歇项按 monitoring 判读）。结论待回填，不冒充通过（ADR-037） |
| 发布构建 | **运行中**：`build-release` run `45022997717`（v0.1.1 tag 触发，
  2026-11-22T12:34:37Z；amd64/arm64 zip＋sha256、freebsd-amd64.tar.gz＋sha256 六件产物，
  ADR-083）。结论待回填 |
| Code Scanning | 0 条 open（2026-11-21 复核）；CI 每周复核保持 |
| 本机安装验证 | `artifacts/install-verify-windows-amd64.txt`（2026-11-16，gnullvm）——
  v0.1.1 产物需随发布构建重新执行 `package-windows.py --verify` |

v0.1.0 的发布记录为历史事实，移至 §6.2，不改写。

## 6.2 历史发布记录（v0.1.0，2026-11-17）

| 项 | 事实 |
|---|---|
| 标签 | `v0.1.0`（annotated）→ 提交 `712227c`，2026-11-17 推送 |
| 验收链证据 | run `43902805603`（代码提交 `14c2f3b`）：windows 16.6／arm64 14.8／freebsd 25.6／openbsd 1.4／dragonfly 2.3／measurements 27.8 分钟，全部 success |
| 文档提交 | run `43908904591`／`43910634901`／`43911318731`：triage＋docs-gates 0.3—0.5 分钟（ADR-082 分流） |
| 发布构建 | run `43911920856`：package-amd64-release 11.1／package-arm64-release 8.1／package-amd64-debug 5.9／release 0.6 分钟，全部 success |
| 产物 | `whcode-0.1.0-windows-amd64.zip` 33,364,477 字节＋`.sha256`；`whcode-0.1.0-windows-arm64.zip` 31,388,490 字节＋`.sha256` |
| Code Scanning | 0 条 open 告警（2026-11-17 复核；ADR-037／§5.2） |
| 本机安装验证 | `artifacts/install-verify-windows-amd64.txt`（2026-11-16T16:40:24Z，gnullvm，doctor `--json` 退出 0） |

下一版本起 Release 资产为六件：两平台 zip 各带 `.sha256`＋`whcode-<v>-freebsd-amd64.tar.gz` 带 `.tar.gz.sha256`（ADR-083），release job 数量断言同步收紧；v0.1.0 的两件产物为历史事实，不改写。

## 7. 发布后监控

- `platform-verification.yml` 随 `main` 运行（含 FreeBSD 硬门禁）；
- `code-scanning` job 每周 schedule 复核零告警（ADR-037／§5.2）；
- `perf-budget` 与 `desktop` 按需人工触发；
- `action-audit` 每周联网复核 action 版本（§5.1）。

## 8. 失效条件与复审

- 版本号递增后本检查表须随新版本重写（不沿用旧证据）；
- 验收目标、硬门禁或预算口径变更（ADR 修订）后，本表判据同步修改；
- 发布流程本身（build-release.yml 结构）变更后，`check-release.py` 与本节须同批复核。
