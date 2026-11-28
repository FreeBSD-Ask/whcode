# 四工具全量安全扫描：覆盖、判定与修补

**日期**：2026-11-28 | **性质**：治理分析（不承载需求承诺；结论转化为承诺须走 G0—G5 门禁，口径同 `research/` 规则，ADR-045）
| **对象**：仓库全量（1005 个跟踪文件；245 个 Rust、4 个 JavaScript、34 个 Python 源文件）与依赖、CI、Agent 指令面
| **判据**：每个工具的覆盖必须可定位；每条发现给出判定与证据；真实问题在代码层修复；无法运行的工具登记阻塞，不以推断替代结果

本报告汇总 Mimosa、Semgrep、Claude Security、SonarQube 四套工具在本仓库的实跑情况。
全部结论为静态证据；未执行目标项目、未做运行时验证，不构成“项目安全”或“无风险”结论。

## 0. 任务认知卡

| 字段 | 内容 |
|---|---|
| 方向 | 用四套工具覆盖仓库全部文件，汇总发现并修补真实问题，产出不自动合并的分析 PR |
| 目标 | 每个工具的覆盖与回执可复核；去重后的每条发现有着落；真实问题修复；阻塞项如实登记 |
| 用户意图 | 复述：不计成本代价，覆盖所有文件，所有提示都要分析解决，力求最佳；PR 不主动合并；不使用子代理 |
| 非目标 | 不关闭或降低规则阈值；不以 dismiss／忽略代替判定；不做运行时渗透；不扩大平台验收范围 |
| 现有证据 | §2 的工具状态与命令；§3 的 Semgrep 计数与行号；§5 的提交哈希；Mimosa 逐条报告见 `docs/analysis/2026-11-28-mimosa-deep-scan-triage.md` |
| 手段（后置） | Mimosa 密封扫描作主回执；Semgrep 固定版本隔离环境补规则层；Claude Security 与 SonarQube 按可用性执行或登记阻塞；主会话复核高危面 |

## 1. 结论

1. **Mimosa 完成全文件覆盖**：主扫描 250 文件上限之外补跑 22 次分区密封扫描，283 个受支持源文件全部至少进入一次扫描；去重后 64 条唯一发现逐条判定，62 条为误报或测试夹具，2 条真实问题（Python 开发依赖未固定、`AGENTS.md` 重复行）已修。详见单独报告。
2. **Semgrep 完成全仓规则扫描**：固定 1.136.0，548 条规则，首轮 93 条、修补后 90 条。其中 3 条真实问题已修（Dependabot 缺冷却期 2 条、FreeBSD 发布步骤 `curl | sh` 1 条）；其余 87 条为 INFO 级通用建议或经反证的误报。
3. **Claude Security 未产出发现**：其官方 `scan.js` 依赖 Claude Code 的 Workflow 门面（`export`、`parallel`、`setTimeout`），在 ZCode 编译器下无法运行；按同方法搭建的 ZCode 适配工作流在 high 档研究阶段被用户停止，停止前没有任何发现上报。按用户“不使用子代理”的指令，未恢复、未重建，也不把它计作“干净”。
4. **SonarQube 阻塞**：插件已启用，但其 MCP 服务器入口是 `sonar run mcp`；本机没有 `sonar` 可执行文件，ZCode 未注册 `mcp__sonarqube__*` 工具，也没有服务器地址、令牌或本地实例。未取得任何 SonarQube 结果。
5. **主会话复核新增 1 条真实设计问题**：工作区 `.whcode/mcp.json` 在每一轮自动连接并执行其中声明的命令，没有工作区信任确认（`crates/whcode-app/src/turn.rs:578`）。该条定级 high，本次未修，原因与建议见 §4.1。
6. **修补合计 6 项**：Python 开发依赖精确版本＋SHA256 哈希锁、Dependabot 7 天冷却期、FreeBSD 发布改用固定版本校验的 rustup-init、`AGENTS.md` 去重、`.gitignore` 忽略 Mimosa 本机状态，以及上述报告与变更记录。全部改动在 `security/mimosa-deep-audit-2026-11-28` 分支，未合并。

## 2. 工具状态与覆盖

| 工具 | 状态 | 覆盖 | 回执或证据 |
|---|---|---|---|
| Mimosa | 完成 | 283 个受支持源文件（245 `.rs`、4 `.js`、34 `.py`）＋非代码检查面 | 主回执 `scan-2026-11-28T06-37-47.413Z-702f8f549619`，seal `sha256:b3fd02a0…`；22 次分区扫描；详见 Mimosa 报告 |
| Semgrep | 完成 | 830 个跟踪文件（首轮 957），548 条规则，跳过 4 个大于 1 MB 文件与 45 个 `.semgrepignore` 匹配项 | `local/semgrep-full.json`（首轮 93 条）、`local/semgrep-after-fixes.json`（90 条） |
| Claude Security | 未产出发现 | 官方管道不可运行；适配工作流停在研究阶段，0 条上报 | 编译诊断：`scan.js:L1` `export` 不允许；`parallel`／`setTimeout` 不存在；运行记录 `dwfrun-52084de1…`、`dwfrun-171730aa…` 均为 stopped、0 条 findings |
| SonarQube | 阻塞 | 0 个文件 | 插件配置 `sonar run mcp`；`which sonar` 无结果；ZCode MCP 工具清单无 `mcp__sonarqube__*`；无服务器地址／令牌／本地实例 |

### 2.1 Semgrep 运行口径

- 版本：`semgrep==1.136.0`，安装在仓库外的隔离虚拟环境 `C:\Users\ykla\.zcode\temp\semgrep-venv`，未改仓库依赖。
- 规则集：`p/default`、`p/secrets`、`p/cwe-top-25`、`p/owasp-top-ten`、`p/rust`、`p/python`、`p/javascript`、`p/command-injection`、`p/sql-injection`、`p/xss`；共 548 条规则。
- 指标关闭（`--metrics off`），未向 Semgrep 服务上传代码或扫描指标。
- 首轮 93 条；修补 Dependabot 与 rustup 两处后复跑为 90 条，扫描面与规则集相同。

### 2.2 Claude Security 不可用的证据

官方插件 `claude-security@claude-plugins-official` 0.12.0 已启用，但 `workflows/scan.js` 是 Claude Code 的 Workflow 脚本：

- 第 1 行 `export const meta=...`：ZCode 编译器报“`export` is not allowed in a workflow script”。
- 第 76、80 行 `parallel(...)`、第 66 行 `setTimeout(...)`：ZCode 门面不存在这些名字。
- 其余大量隐式 any 与 `Agent` 类型差异，均来自两套工作流 API 不同。

按插件 job 说明书，Workflow 工具缺失时应停止并说明，不得手工替代。用户随后明确“不使用子代理”，因此适配工作流也被停止，未恢复。

### 2.3 SonarQube 阻塞的证据

- `~/.zcode/cli/config.json` 的 `plugins.enabledPlugins` 含 `sonarqube@claude-plugins-official: true`。
- 插件 `mcp.json` 声明服务器命令为 `sonar run mcp`；`which sonar` 无结果，`~/.local/bin` 与 npm 全局目录均无该可执行文件。
- ZCode 本会话的 MCP 工具清单没有 `mcp__sonarqube__*`；`config.json` 的 `mcp.servers` 也没有 sonarqube 条目。
- 没有 `SONAR_TOKEN`／`SONAR_HOST_URL` 环境变量，没有 Docker，也没有 sonar-scanner。
- 结论：需要先安装 SonarQube CLI、完成 `sonar auth login` 并配置 Cloud／Server；本机未取得，登记阻塞。

## 3. Semgrep 发现逐类判定

修补后 90 条：high 0、medium 0、ERROR 6、WARNING 9、INFO 75。逐类如下。

### 3.1 已修（3 条）

| 位置 | 规则 | 处置 |
|---|---|---|
| `.github/dependabot.yml:15` | `dependabot-missing-cooldown`（medium） | 增加 `cooldown.default-days: 7`，提交 `65d5058` |
| `.github/dependabot.yml:38` | `dependabot-missing-cooldown`（medium） | 同上 |
| `.github/workflows/build-release.yml:234` | `gha-curl-pipe-shell`（ERROR） | 改为下载版本化 `rustup-init` 1.29.3（FreeBSD x86_64），按固定 SHA256 校验后执行，提交 `3fcd40d` |

Dependabot 冷却期字段已用官方 JSON Schema 核对：`cooldown.default-days` 为 0—90 的整数，两个更新项均在 schema 允许范围内。

### 3.2 Python 动态 URL（9 条，误报）

`measure-memory-pressure.py:108`、`measure-nfr-budgets.py:147`、`check-actions.py:70`、`check-code-scanning.py:83`、`fetch-duckdb.py:175`、`setup-windows-llvm.py:97`、`smoke-sensenova.py:130`、`web-smoke.py:51`、`web-ui-check.py:42`。

规则担心 `urllib` 接受 `file://` 后读取本地文件。实际数据流：

- 基准与冒烟脚本的 URL 由本机回环地址（`127.0.0.1`＋随机端口）拼成；
- `fetch-duckdb.py` 与 `setup-windows-llvm.py` 的 URL 是常量发布地址；
- `check-actions.py` 的主机是常量 `api.github.com`，仓库名来自 `action-versions.json` 登记项；
- `check-code-scanning.py` 的主机固定，仓库参数只影响 API 路径；
- `smoke-sensenova.py` 的端点是固定 `https://token.sensenova.cn/v1/chat/completions`。

没有攻击者可控字符串进入 `urlopen`。为迎合规则给本地脚本加协议／主机／IP 校验，会引入没有威胁模型对应的复杂度，本次不改。

### 3.3 Python 子进程（3 条，误报）

`tui-smoke.py:81`、`web-smoke.py:71`、`web-smoke.py:94`。三处均为 `subprocess.run([exe, ...])`／`Popen([exe, ...])`，显式 `shell=False`，参数以列表传递，不含 shell 解析；`exe` 来自脚本自己的 `--exe` 参数或 `CARGO_BIN_EXE_*`。不存在命令注入路径。

### 3.4 Python 3.6 兼容性（2 条，误报）

`web-smoke.py:93` 的 `encoding`／`errors` 参数。本仓库开发与 CI 使用现代 Python（`requirements-dev.txt` 与工作流均未声明 3.6），规则按通用最低版本提示，不适用。

### 3.5 Rust INFO（76 条，无行动）

- `temp-dir` 70 条：63 条位于 `#[cfg(test)]`；7 条生产路径为 `cancel_dir_from_env`、`bench.rs`、`paths.rs`、`gitbash.rs` 两处、`python.rs`、`store/test_util.rs`，其中暴露面明确的六处已经做 `..` 清洗，`paths.rs` 只是平台默认临时目录的取值函数。没有攻击者可控路径进入删除或写入。
- `current-exe` 3 条：`fonts.rs:180` 与 `main.rs:174` 用于定位随包资源，`main.rs` 的 1 条为测试。
- `args` 3 条：`main.rs:191` 的 CLI 参数解析与两个测试用 MCP 服务器二进制，均非安全问题。

### 3.6 其他复核（无发现）

- SQL：`prepare(&format!(...))` 的插值全部是静态列清单（`FINDING_COLUMNS`、`EVENT_PROJECTION`、`DELIVERY_COLUMNS`、`COLUMNS`）或固定前缀（`EXPLAIN `），值一律走 `?`＋`params!` 绑定。
- Web 鉴权：`crates/whcode-web/src/security.rs` 使用 `Authorization: Bearer` 头、恒定时间比较、Origin 检查，token 不入 URL query；未发现绕过。
- 进程启动：`Command::new` 的可变参数来自操作者配置（LSP／MCP／Python 解释器）或测试夹具；SSH 相关 `sshd` 调用全部在 `#[cfg(test)]`。
- `unsafe`：全仓 `crates/**/*.rs` 未检出 `unsafe {`，与 workspace lint 一致。

## 4. 主会话复核发现

### 4.1 工作区 MCP 配置自动执行（真实，high，未修）

**证据链**：

- `crates/whcode-app/src/turn.rs:578`：每一轮调用 `McpManager::connect_workspace(request.workspace_root.as_deref())`，无用户确认步骤。
- `crates/whcode-mcp/src/config.rs:16`：配置路径 `<workspace>/.whcode/mcp.json`；`:107` 直接读取并解析。
- `crates/whcode-mcp/src/session.rs:202`：`Command::new(&config.command)`，随后追加 `args` 与 `env` 并启动子进程。
- `docs/adr/ADR-054-mcp-client-minimal.md:41`：把“配置文件存在”表述为“显式 opt-in”，但该文件由工作区作者控制，不是使用者的确认。

**影响**：打开一个不受信任的仓库并让代理跑一轮，工作区就能让 WHCode 以当前用户权限执行任意命令（CWE-829，必要时可落为 CWE-78）。攻击者不需要网络可达，只需要仓库被打开。

**为什么不本次修**：修复会改变默认行为，涉及四种前端（TUI／GUI／CUI／Web）的信任确认与用户配置存放位置，属于需要 ADR 的不可逆设计决策；仓库规则要求不可逆决策设计先行并留回退证据。

**建议**（供批准后实施）：

1. 在工作区之外保存“已信任工作区”清单（用户配置目录，按 canonical path），只有清单内的工区才自动连接 MCP；
2. 首次遇到未信任工作区的 MCP 配置时，显示一次确认，列出服务器名与命令，默认拒绝；
3. 提供 `--allow-workspace-mcp` 与 `--no-mcp` 两个显式开关，默认值可撤销、可审计；
4. 为上述行为补 ADR 与失败路径测试（拒绝、清单损坏、路径不存在）。

### 4.2 其他高危面

- 认证与权限：`authz.rs` 能力模型、Web 鉴权、SSH `authorized_keys`（缺失即失败）均按显式拒绝实现，未发现绕过。
- 文件路径：工作区相对路径由 store／app 层规范化，MCP 配置与技能目录有大小上限与名字白名单。
- 依赖：Cargo.lock 锁定；Python 开发依赖已加哈希锁；Dependabot 冷却期已加。

## 5. 修补清单（分支 `security/mimosa-deep-audit-2026-11-28`）

| 提交 | 内容 |
|---|---|
| `01a981f` | `.gitignore` 忽略 Mimosa 本机状态与扫描标记 |
| `b79fff4` | Python 开发依赖固定版本并加 SHA256 哈希锁（`pip download --require-hashes` 对 win_amd64 cp312／cp313 实跑通过） |
| `58cfe89` | `AGENTS.md` 删除重复的 AI 证据禁令行 |
| `65d5058` | Dependabot 两个更新项增加 7 天冷却期 |
| `3fcd40d` | FreeBSD 发布改用固定版本＋SHA256 校验的 rustup-init 1.29.3 |
| 本报告 | 四工具覆盖、Semgrep 逐类判定、MCP 工作区信任问题登记 |

`cargo check --locked --workspace` 在本机 1.100.0 gnullvm 工具链下通过（2m33s）；`cargo fmt --all --check` 通过；`check-workflows.py`、`check-docs-style.py`、`check-eol.py` 通过。`cargo test` 未在本机运行，测试结论以 PR 的 CI 日志为准（ADR-037）。

## 6. 残留风险与复审触发

1. **Claude Security 无结果**：官方管道与 ZCode 不兼容，适配工作流被停止。复审触发：在 Claude Code 会话中运行 `claude-security` 全仓 scan，或 ZCode 工作流门面兼容后再跑；本次不把它计作干净。
2. **SonarQube 无结果**：缺少 CLI、服务器与认证。复审触发：安装 `sonar` 并完成 `sonar auth login`／配置服务器后，对全仓跑一次分析并把质量门结果附到本报告。
3. **MCP 工作区信任未修**：在 ADR 批准并落地前，打开不受信任仓库前应先检查 `.whcode/mcp.json`；已知风险接受者为本机用户。
4. **Semgrep 覆盖缺口**：4 个大于 1 MB 文件与 45 个 `.semgrepignore` 匹配项未扫；其中是否含源码需按 `--verbose` 清单复核。
5. **静态证据边界**：全部工具均为静态分析，未执行目标项目；调用图与跨文件可达性不完整，任何“未发现”都不能读作“不存在”。

## 7. 复现命令

```bash
# Semgrep（仓库外隔离环境）
uv venv --python 3.13 <repo-external>/semgrep-venv
uv pip install --python <repo-external>/semgrep-venv/Scripts/python.exe "semgrep==1.136.0" "setuptools<81"
<repo-external>/semgrep-venv/Scripts/semgrep.exe scan \
  --config p/default --config p/secrets --config p/cwe-top-25 --config p/owasp-top-ten \
  --config p/rust --config p/python --config p/javascript --config p/command-injection \
  --config p/sql-injection --config p/xss --metrics off --disable-version-check \
  --exclude target --exclude vendor --exclude local --exclude .git \
  --json --output local/semgrep-after-fixes.json .

# Mimosa（主回执经 MCP；分区扫描见 Mimosa 报告 §8）
# Claude Security：需 Claude Code 会话的 Workflow 工具
# SonarQube：需 sonar CLI + 服务器 + sonar auth login
```

## 8. 声明

- 本报告是静态审计记录，不是运行时验证，也不是“项目安全”的证明。
- SonarQube 与 Claude Security 未产出结果，已按阻塞登记，未以其他工具的结果替代。
- 全部判定以 2026-11-28 的工作树为准；行号随代码演进而移动，复核以文件内容与提交哈希为准。
