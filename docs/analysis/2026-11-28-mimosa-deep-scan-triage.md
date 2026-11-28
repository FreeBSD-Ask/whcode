# Mimosa 全量深扫 triage：64 条唯一发现的判定与处置

**日期**：2026-11-28 | **性质**：治理分析（不承载需求承诺；结论转化为承诺须走 G0—G5 门禁，口径同 `research/` 规则，ADR-045）
| **对象**：仓库全量源码（245 个 `.rs`、4 个 `.js`、34 个 `.py`，合计 283 个受支持源文件）与依赖、CI、Agent 指令面
| **判据**：每条 finding 必须给出判定、反证或修复；无法判定者标 `[待验证]` 并说明复现条件

本报告记录一次 Mimosa 密封深扫的全量 triage。扫描只做静态分析，未执行目标项目、未做运行时验证；
所有 sealed 扫描的 `runStatus` 均为 `inconclusive`（调用图部分不完整），`verdictEffect` 均为 `none`。
因此本报告不构成“项目安全”或“无风险”结论，只对下列 64 条唯一条目逐条给判定。

## 0. 任务认知卡

| 字段 | 内容 |
|---|---|
| 方向 | 用 Mimosa 覆盖仓库全部受支持文件，把返回的每一条提示分析到位并处置 |
| 目标 | 全文件覆盖的 sealed 扫描回执；64 条唯一发现逐条判定；真实问题在代码层修复；误报给可复核反证 |
| 用户意图 | 复述：不计成本与耗时，覆盖所有文件，所有提示都要分析解决，力求最佳；产出分析 PR，不自动合并 |
| 非目标 | 不修改扫描阈值或关闭规则；不以 dismiss／忽略代替判定；不做运行时渗透；不扩大平台验收范围 |
| 现有证据 | 本报告 §2 的 scan ID 与 seal；§4 的逐条行号；§6 的修复与 `pip download --require-hashes` 实跑输出 |
| 手段（后置） | MCP 密封深扫作主回执；CLI 分区密封扫描补齐文件上限之外的源码；其余 Mimosa 检查器覆盖依赖与指令面 |

## 1. 结论

1. **覆盖已补齐**。主扫描（MCP 深扫）只选中 250 个文件；把 `MIMOSA_PROJECT_MAXFILES` 提高到 100000 后
   `selectedFiles` 仍为 250，说明 250 不是该变量控制的上限。改用 22 次按目录的密封扫描补齐：工作树内
   283 个受支持源文件全部至少进入一次扫描。`tests` 分区补出 1 条主扫描未覆盖的发现，证明补齐不是形式动作。
2. **64 条唯一发现中，62 条为误报或测试夹具路径，2 条为真实工程问题**。62 条里：6 条硬编码凭据全部位于
   `#[cfg(test)]` 或集成测试目标；16 条前端“SSRF”是浏览器同源 `fetch`；15 条 `main.rs` 路径穿越是本地操作者
   用 `--config` 指定自己的配置文件；7 条 `write()` 是测试写临时文件且符号被同名解析到另一 crate；6 条 SSH
   条目来自操作者工作区目录；14 条 Python 条目是常量 URL、固定 locale 清单与 argparse 白名单下的本地脚本。
   2 条真实问题：`requirements-dev.txt` 的 `pillow`／`fonttools` 未固定版本，`AGENTS.md` 有一条重复禁令行。
3. **真实问题已修**。四份 Python 开发依赖改为精确版本加 SHA256 哈希锁，并用 `pip download --require-hashes`
   对 `win_amd64` 的 cp312／cp313 两个目标实跑校验通过；`AGENTS.md` 删除重复行。
4. **非代码检查器另有 5 条**：`supply-chain-check` 2 条（即上述真实问题），`pii-check` 1 条（16 个零的测试
   哈希被 Luhn 校验误判），`agent-check` 2 条（AGENTS 禁止凭据写入／上传的禁令被误读为外泄意图）；
   `git-secrets` 6 条与代码侧硬编码凭据同源，全部是 `sk-secret*` 测试夹具，无真实凭据。

## 2. 扫描回执与口径

### 2.1 主回执（MCP 深扫）

| 字段 | 值 |
|---|---|
| scan ID | `scan-2026-11-28T06-37-47.413Z-702f8f549619` |
| seal | `sha256:b3fd02a0b2d98cdf6631b401dbb1dd6e2a73924a8b9956537ddc44cd2e2bd9e2` |
| 深度 | `deep` |
| 选中／解析文件 | 250／250（`truncated: false`，`readFailures: 0`，`parseFailures: 0`） |
| 覆盖状态 | `partial`，`runStatus: inconclusive` |
| 发现 | 49 条（high 43、medium 6） |
| 覆盖缺口 | 调用图部分不完整：部分调用为动态派发或超出分析规模 |
| 依赖摘要 | 679 个包，离线 advisory 匹配 0，未知 0 |

同一输入用 CLI 复跑（`security-scan run --deep`）得到相同的 49 条与相同的覆盖缺口，seal 为
`sha256:fd8cca04db5540feb65b1fe9fa162c43da907ad50fb71767c973dba851295a35`（不同封印目录与时间戳，
摘要一致）。把 `MIMOSA_PROJECT_MAXFILES` 设为 100000 后 `source.limit` 变为 100000，但 `selectedFiles` 仍为
250，因此采用分区扫描补齐。

### 2.2 分区密封扫描（22 次）

| 目录 | selectedFiles | findings | seal（sha256，前缀 `sha256:` 略） |
|---|---|---|---|
| `crates/whcode-store` | 34 | 0 | `1328d1430f5bcad50567d7d0fb7ee4c9f1fe582a5d7655f39ff3f2ed3282e095` |
| `crates/whcode-app` | 41 | 2 | `60beba2776fcb2bb9b3b8006211dbacefce62829c92f254948ae64f4c9edad1f` |
| `crates/whcode-bin` | 20 | 16 | `60815e0fdd5963e724da12e8fb73b344c0b67c65e54a38e6558cf1e03b707b44` |
| `crates/whcode-llm` | 17 | 2 | `fa199eb9421b56406b7d55a0295379539b5dbf45a2268425d406590122c23bab` |
| `crates/whcode-web` | 12 | 16 | `abe65ad5bba651df0a330aab5d7ba9289d198da3a06bf5a33060203d2e2a6974` |
| `crates/whcode-domain` | 15 | 0 | `306fe5466935ae0ad6fc8ca5e169008bafb3d9d8d26bafa3f50c4ee576243c6c` |
| `crates/whcode-runtime` | 12 | 0 | `116a38f42d0cbb6a5bd87dae9dcecd879c62623a753fa81b82d5d2a35fe970cc` |
| `crates/whcode-tools` | 11 | 0 | `959794564f942ae22a30de4b1bad0f89079e9dead3b7ee79637324baad62ea97` |
| `crates/whcode-tui` | 9 | 0 | `95f9dab76e2dd35b834e1570ebdc47773cd913e1fdcac23eb86c37ddc5687b88` |
| `crates/whcode-mcp` | 9 | 0 | `4e8cd8adc5efc2cdcceb41857a617033c38eed288a41fad67abc10d695368439` |
| `crates/whcode-cui` | 9 | 1 | `61df22dd387bbb72febf99de985fa94b6323c9f75ca688cbd493f52628cfa2c7` |
| `crates/whcode-speckit` | 8 | 0 | `634b22c93e2c8029f4b62029592748ac7d3743b61d71e6ac7b793c145bdd9b11` |
| `crates/whcode-platform` | 8 | 0 | `c124d642eee61b3c0a9da7219dc361651885e5fe9e88deb78e3f19ed0b306b55` |
| `crates/whcode-ssh` | 7 | 4 | `124bbc401af56792042c32564187cc0d5e43cbcbf876c8e7294350b818bb7223` |
| `crates/whcode-lsp` | 4 | 0 | `df6d991bc0bc0a894ca9fe98b3eb310b360981e4b8e8e82391937fb5778ea503` |
| `crates/whcode-gui` | 2 | 0 | `28cfc858311d6e86ee46c7676599eb886932a6259036391bfd294de9c8579d36` |
| `tests` | 32 | 1 | `c09623e75d91202d788be7f28d6dc84c7317023a0f14a86a8442ec396973d415` |
| `.github` | 39 | 14 | `d45ff9de9e1d6517cbf8359c8cd2e8fb7df14c5c2fbb69289f877515a5b6c2e1` |
| `artifacts` | 0 | 0 | `11c5149dfb27f67caff335bcdc33238d31e079371d96230606e8840d255c46bb` |
| `.specify` | 0 | 0 | `1d2f561fdf5d7cec19ac6746ade9d0f23a990878a9041e11ce3084749dc4da2f` |
| `specs` | 0 | 0 | `ccdb2e687fcb043eb7d26011a7e75995ebc408a19be59f05297f5abaff0cac0b` |
| `i18n` | — | — | 该目录无受支持源文件，扫描器返回“不支持的文件类型”，未生成回执 |

全部回执目录位于 `C:\Users\ykla\.mimosa\security-scans\whcode-full\`，逐次 `scanId` 与上表顺序一一对应
（第一次为 `scan-2026-11-28T06-49-39.827Z-7c9a63b1d568`，最后一次为
`scan-2026-11-28T06-50-07.001Z-245a29ba6f2a`）。`selectedFiles` 含工程清单与工作流配置等非语义文件，
因此可能大于该目录的受支持源文件数；解析失败数为 0。

### 2.3 非代码检查器

| 检查器 | 受检文件 | 结果 |
|---|---|---|
| `supply-chain-check` | 6 | 2 条 medium（依赖未固定），见 §6.1 |
| `pii-check` | 283 | 1 条 high（误报，见 §5.4） |
| `agent-check` | 6 | 2 条 high（误报，见 §5.4） |
| `git-secrets` | 546 个提交 | 6 条（与代码侧测试夹具同源，`truncated: false`） |
| `release-check`／`delivery-check`／`license-check`／`llm-check` | 0／7／0／283 | 0 条 |

Semgrep 第二意见：Mimosa 托管安装被 PreToolUse 钩子拒绝；随后在仓库外隔离环境安装
`semgrep==1.136.0` 并完成全仓扫描（548 条规则，首轮 93 条、修补后 90 条），结果与逐类判定并入
`docs/analysis/2026-11-28-multi-tool-security-scan.md`。本报告的判定仍以 Mimosa 密封扫描为准。

## 3. 覆盖边界

- 引擎支持的语言为 `.py/.ts/.tsx/.js/.jsx/.go/.java/.php/.rb/.cs/.kt/.kts/.rs`。本仓库受支持源文件共
  283 个（`git ls-files` 与工作树一致）：245 个 Rust、4 个 JavaScript、34 个 Python。
- 不在引擎语义范围内的文件：Markdown（314 个）、PNG（122 个）、JSON（50 个）、TOML（23 个）、SQL（18 个）、
  properties（13 个）、PO（13 个）、YAML（7 个）、HTML（3 个）等。它们由 `delivery-check`、`supply-chain-check`、
  `agent-check`、`llm-check`、`pii-check`、`license-check` 与仓库既有门禁覆盖；语义污点规则不适用于这些类型。
- 全部扫描为静态分析；未执行目标项目、未联网调用被测服务、未做运行时验证。调用图缺口意味着跨文件可达性
  可能不完整，任何“未发现”都不能读作“不存在”。

## 4. 发现汇总

64 条唯一发现（按路径、行号、规则类别去重；同一位置的 high 与 medium 各计一条）：high 57、medium 6、low 1。

| 组 | 条目数 | 位置 | 判定 |
|---|---|---|---|
| A 硬编码凭据 | 6 | 5 个 `#[cfg(test)]` 模块 ＋ 1 个集成测试 | 误报（测试夹具） |
| B `load_config` 路径穿越链 | 15 | `crates/whcode-bin/src/main.rs` | 误报（本地操作者边界） |
| C 测试写文件与 `write` 同名解析 | 7 | `crates/whcode-llm/src/http/{client,profile}.rs` | 误报（测试夹具＋符号同名） |
| D SSH 主机密钥与 `start` 同名解析 | 6 | `crates/whcode-ssh/src/server.rs` | 误报（操作者目录＋符号同名） |
| E 前端 `fetchJson`／`postJson` 与静态 SSRF | 16 | `crates/whcode-web/frontend/{app,settings}.js` | 误报（浏览器同源请求） |
| F Python 脚本路径穿越、SSRF、随机数 | 14 | `.github/scripts/**` | 误报（常量 URL／固定清单／argparse 白名单） |

## 5. 逐条判定

### 5.1 A 组：硬编码凭据（6 条，全部误报）

五处 `sk-secret*` 字面量都在测试模块内，用途是验证 `TraceLevel::Redacted` 的字段名脱敏；
第六处是集成测试目标里的 `sk-leaky`，断言导出结果不含该串。这些值不是凭据，也不进入日志或事件正文。

| 位置 | 证据 |
|---|---|
| `crates/whcode-app/src/events.rs:322` | 所在 `#[cfg(test)]` 起于第 307 行；`seed_events` 播种测试事件 |
| `crates/whcode-app/src/share.rs:297` | 所在 `#[cfg(test)]` 起于第 251 行；`share_bundle_redacts_secrets_and_self_verifies` |
| `crates/whcode-bin/src/cui_dispatch.rs:2761` | 所在 `#[cfg(test)]` 起于第 2122 行；测试工作区内播种事件 |
| `crates/whcode-cui/src/events_cmd.rs:101` | 所在 `#[cfg(test)]` 起于第 80 行；`setup()` 夹具 |
| `crates/whcode-llm/src/trace.rs:187` | 所在 `#[cfg(test)]` 起于第 179 行；断言 `api_key` 被替换为 `[REDACTED]` |
| `tests/integration/test_trace_and_context.rs:136` | 整个文件是集成测试目标；断言 `!serialized.contains("sk-leaky")` |

处置：不改。把字段名改掉会让“按字段名脱敏”的测试失去对象，属于为迎合启发式而削弱证据，仓库规则禁止。
`git-secrets` 的 6 条与上表同源（其中 5 个文件在历史提交中出现），无真实凭据。

### 5.2 B 组：`load_config` 路径穿越链（15 条，全部误报）

`load_config`（`crates/whcode-bin/src/main.rs:1054`）只读取命令行里 `--config` 指定的路径：

```rust
let path = args.get(index + 1).ok_or_else(...)?;
let raw = std::fs::read_to_string(path)...
```

路径来自本机操作者自己的 argv，不来自网络、SSH 客户端或数据库内容。读取任意本地配置文件是 CLI 的既定
能力（用户自由：默认值可见、可改、可撤销），不存在权限边界被跨越的问题。扫描器把“命令行参数”整体视为
不可信输入，才产生整条链。SSH 服务端模式在启动时由 `serve::serve_command` 装配配置，远端会话命令
不经过 `load_config`。

| 位置 | 内容 |
|---|---|
| `main.rs:206` | `doctor(&args[1..])` 调用点 |
| `main.rs:212` | `gui_command(&args[1..])` 调用点 |
| `main.rs:235`、`236`、`242`、`247` | `launch_selected_entry` 内的 TUI／GUI 分发 |
| `main.rs:391` | `fn launch_interactive(args: &[String])` 定义 |
| `main.rs:406` | `prepare_frontend(args)` 调用点 |
| `main.rs:442` | `fn gui_command(args: &[String])` 定义 |
| `main.rs:451` | `prepare_frontend(args)` 调用点 |
| `main.rs:498` | `fn prepare_frontend(args: &[String])` 定义 |
| `main.rs:499` | `load_config(args)` 调用点 |
| `main.rs:607` | `fn doctor(args: &[String])` 定义 |
| `main.rs:609` | `load_config(args)` 调用点 |
| `main.rs:1054` | `fn load_config(args: &[String])` 定义 |

处置：不改代码；威胁模型写入本报告（本地操作者即信任主体）。

### 5.3 C 组：测试写文件与 `write` 同名解析（7 条，全部误报）

三条 high 的 `write`（`client.rs:1498`、`1537`、`1906`）与三条 medium 的跨文件污点指向同一处：
`std::fs::write(&path, "fake-secret-value")`，位于测试函数内，写的是 `unique_temp_path` 生成的临时文件。
扫描器把 `write` 解析到 `crates/whcode-domain/src/i18n.rs:612` 的测试辅助方法（同名 `fn write`），
并声称“定义于 `src/i18n.rs`”，属于跨 crate 同名符号碰撞。`profile.rs:840` 的 `write_secret_file` 同样是
测试辅助函数。

| 位置 | 证据 |
|---|---|
| `crates/whcode-llm/src/http/client.rs:1498` | `auth_header_reaches_server_but_never_leaks_into_errors` 测试内写临时密钥文件 |
| `crates/whcode-llm/src/http/client.rs:1537` | `query_auth_is_encoded_into_request_target` 测试内写临时密钥文件 |
| `crates/whcode-llm/src/http/client.rs:1906` | `profile_with_duplicate_auth_header_is_rejected` 测试内写临时密钥文件 |
| `crates/whcode-llm/src/http/profile.rs:840` | `#[cfg(test)]` 内的 `write_secret_file` 辅助函数 |

处置：不改。测试写临时文件是验证“凭据不泄漏”的必要前置。

### 5.4 D 组：SSH 与 `start` 同名解析（6 条，全部误报）

`server.rs:612`／`618` 的主机密钥与授权清单路径由 `params.workspace_dir` 派生；该字段由
`crates/whcode-bin/src/serve.rs:242` 从 CLI 工作区参数装配，远端 SSH 客户端不参与。`server.rs:626` 的
`start` 被解析到 `crates/whcode-lsp/src/client.rs:252` 的 `Command::new(&server.command)`，但
`whcode-ssh` 并没有 `src/client.rs`；这是按文件名与函数名跨 crate 解析造成的碰撞。语言服务器命令来自
项目配置，由操作者安装与指定。

| 位置 | 反证 |
|---|---|
| `crates/whcode-ssh/src/server.rs:612` | `load_or_create_host_key(&params.workspace_dir)`；`workspace_dir` 来自 CLI |
| `crates/whcode-ssh/src/server.rs:618` | `load_authorized_keys(&params.workspace_dir)`；同上 |
| `crates/whcode-ssh/src/server.rs:626` | `CommandExecutor::start` 不产生子进程；被误指向 `whcode-lsp::client::start` |

处置：不改。

### 5.5 E 组：前端 SSRF（16 条，全部误报）

`fetchJson`／`postJson` 是浏览器端的同源请求包装，URL 为 `/api/i18n`、`/api/approvals/rules`、
`/api/subagents`、`/api/skills` 等固定相对路径；唯一带变量的是 `/api/events/export?session=...&format=...`，
`session` 经 `encodeURIComponent` 编码且主机不变。浏览器的同源策略与 CORS 由浏览器执行，前端代码
没有服务端请求能力；“访问内网、云元数据或本机服务”的 SSRF 语义不成立。

| 位置 | 证据 |
|---|---|
| `crates/whcode-web/frontend/app.js:39`、`161` | `fetchJson` 定义与调用，目标为固定 `/api/*` |
| `crates/whcode-web/frontend/app.js:87`、`101` | `fetchJson`／`postJson` 内的 `fetch(url)` 包装 |
| `crates/whcode-web/frontend/app.js:435`、`446`、`460`、`476`、`492`、`506` | `postJson` 的固定写命令路径 |
| `crates/whcode-web/frontend/app.js:719` | 导出请求，`session` 已编码 |
| `crates/whcode-web/frontend/settings.js:102` | `fetchJson('/api/i18n...')` |
| `crates/whcode-web/frontend/settings.js:422`、`436`、`455`、`495` | 审批规则与技能列表的固定路径 |

处置：不改。

### 5.6 F 组：Python 脚本（14 条，全部误报）

| 位置 | 规则 | 反证 |
|---|---|---|
| `.github/scripts/analyze-nfr204.py:69` | 不安全的随机数 | `random.Random(BOOTSTRAP_SEED)` 是固定种子的统计 bootstrap，需要可复现，不用于安全用途 |
| `.github/scripts/analyze-nfr204.py:331` | 路径穿越 | 在输入文件同目录写 `nfr204-verdict.json`；输入路径由本机操作者以 `sys.argv[1]` 提供 |
| `.github/scripts/fetch-duckdb.py:356` | 路径穿越 | `--target` 由 argparse `choices=sorted(set(PACKAGES) …)` 限定，且 `target not in PACKAGES` 即退出 |
| `.github/scripts/fetch-duckdb.py:175` | SSRF | URL 由常量 `RELEASE_URL` 与固定版本、固定归档名拼成 |
| `.github/scripts/i18n-po.py:122` | 路径穿越 | `locale` 来自模块级固定元组 `LOCALES`，不是外部输入 |
| `.github/scripts/setup-windows-llvm.py:82` | 路径穿越 | `sha256_of` 只读文件算摘要；路径来自 `WHCODE_TOOLCACHE` 或系统临时目录 |
| `.github/scripts/setup-windows-llvm.py:97` | SSRF | URL 为固定 GitHub 发布地址 |
| `.github/scripts/check-actions.py:70` | SSRF | `API` 为固定 `api.github.com`，`repo` 来自 `action-versions.json` 的登记项 |
| `.github/scripts/check-code-scanning.py:83` | SSRF | 主机固定为 GitHub API，仓库参数来自命令行且只影响 API 路径 |
| `.github/scripts/benchmarks/measure-memory-pressure.py:108` | SSRF | `get_json` 只接收本机 `127.0.0.1` 测量服务地址 |
| `.github/scripts/benchmarks/measure-nfr-budgets.py:147` | SSRF | 同上，本机回环测量 |
| `.github/scripts/smoke-sensenova.py:130` | SSRF | 端点为固定 `https://token.sensenova.cn/v1/chat/completions` |
| `.github/scripts/web-smoke.py:51` | SSRF | URL 由本机随机端口拼成 |
| `.github/scripts/web-ui-check.py:42` | SSRF | `base_url` 为本机回环地址 |

处置：不改。为迎合启发式给本地脚本加“协议＋主机＋解析 IP”校验，会引入没有威胁模型对应的复杂度，
与仓库“安全措施必须有威胁模型、证据、代价与失效条件”的要求相反。

### 5.7 非代码检查器条目

| 位置 | 判定 | 证据 |
|---|---|---|
| `requirements-dev.txt:11`、`12` | 真实（已修） | `pillow>=11.0`、`fonttools>=4.50` 未固定；见 §6.1 |
| `crates/whcode-web/src/api.rs:3375` | 误报 | `"hash": "0000000000000000"` 全零测试占位；Luhn 对全零串恒通过 |
| `AGENTS.md:386`、`399`（修前行号） | 误报 | 两条均为“禁止把 API Key 或凭据写入仓库、日志、事件与提交信息”，规则把禁令读成外泄意图 |

`AGENTS.md` 另有一条重复的“禁止用 AI 生成内容补足缺失证据”行（第 401／402 行），与安全判定无关，
但属于同次审阅发现的文档缺陷，已在 §6.2 删除。

## 6. 处置与修复

### 6.1 Python 开发依赖锁定（真实问题）

`requirements-dev.txt` 的四份依赖全部改为精确版本，并附 PyPI 发布文件的 SHA256：

- `pillow==11.3.0`（原 `>=11.0`，保持在 11.x 内）
- `fonttools==4.69.1`（原 `>=4.50`）
- `duckdb==2.0.0`（原已固定，补哈希）
- `pywin32==312 ; sys_platform == "win32"`（原 `>=306`）

pip 检测到 `--hash` 后进入 hash-checking 模式，索引侧同版本文件被替换会安装失败。升级步骤写在文件头注释里：
改版本号后按 PyPI JSON 的 `digests.sha256` 重算，再跑哈希校验命令。

验证证据（本机实跑，`win_amd64` 两个 Python 目标）：

```text
python -m pip download --no-deps --only-binary=:all: --platform win_amd64 \
  --python-version 3.12 --implementation cp --abi cp312 --require-hashes \
  -r requirements-dev.txt -d local/pipcheck
→ Successfully downloaded pillow fonttools duckdb pywin32

python -m pip download ... --python-version 3.13 --abi cp313 ...
→ Successfully downloaded pillow fonttools duckdb pywin32
```

负面与承担者：哈希锁增加维护成本，Python 小版本升级或新增平台时需重算哈希；由维护者在升级依赖时承担。
GUI 探针的实际兼容性只在 `desktop` job（`schedule`／手动触发）观察，本次未取得运行证据，登记为
`[待验证]`。若探针在 11.3.0 上失败，回退口径是把 `pillow` 降到 `11.0.0` 并按同一流程重算哈希。

### 6.2 文档修复

`AGENTS.md` 删除重复的“禁止用 AI 生成内容补足缺失证据”行。该文件同时被 `check-docs-style.py` 与
`check-counts.py` 等门禁覆盖，删除后需复跑门禁。

### 6.3 未改动的条目

62 条误报全部保留原状，不通过改字段名、改常量名或加无威胁模型的校验来“消警”。理由：判定必须有反证，
而不是让扫描器闭嘴；仓库的 Code Scanning 铁律也明确“不得以分析范围收窄表述为已修复”。

## 7. 残留风险与复审触发

1. **静态证据边界**：所有 sealed 扫描 `inconclusive`，调用图缺口未闭合；本报告不能证明不存在未被规则覆盖的
   漏洞。触发复审：Mimosa 引擎升级或规则集变化后重跑，并用 `security-scan compare` 对照本次 seal。
2. **依赖锁的运行时兼容**：`pillow 11.3.0` 与 `fonttools 4.69.1` 的 GUI 探针兼容性未在本机验证。
   触发复审：`desktop` job 下一次 schedule 运行；失败即回退并重算哈希。
3. **Semgrep 第二意见**：Mimosa 托管安装被钩子拒绝后，已改用仓库外隔离环境完成全仓扫描，
   结果见《四工具全量安全扫描》；本报告不重复其逐类判定。触发复审：Semgrep 规则集升级或
   仓库新增语言时复跑。
4. **测试夹具中的敏感字段名**：`api_key`／`sk-secret*` 会持续触发静态规则与 git 历史扫描。
   接受该噪声，因为它们是脱敏测试的对象；若未来引入真实密钥命名规范，再评估改名。

## 8. 复现命令

```bash
# 主回执（MCP）
# security_scan_start({"project": "<repo>", "depth": "deep"}) → security_scan_status

# 分区回执（CLI，逐个目录执行；输出目录在仓库外）
node <plugin>/payload/dist/cli.js security-scan run <repo>/crates/whcode-store \
  --deep --output-dir <repo-external>/whcode-full --json

# 非代码检查面
node <plugin>/payload/dist/cli.js supply-chain-check <repo> --json
node <plugin>/payload/dist/cli.js pii-check <repo> --json
node <plugin>/payload/dist/cli.js agent-check <repo> --json
node <plugin>/payload/dist/cli.js git-secrets <repo> --json

# 依赖锁校验
python -m pip download --no-deps --only-binary=:all: --platform win_amd64 \
  --python-version 3.12 --implementation cp --abi cp312 --require-hashes \
  -r requirements-dev.txt -d local/pipcheck
```

本次扫描的机器可读汇总（去重后 64 条）保存在扫描会话的 `local/mimosa-full/aggregate.json`，按仓库规则
`local/` 不入库；需要复核时按 §8 命令重跑即可重建。

## 9. 声明

- 本报告是静态审计记录，不是运行时验证，也不是“项目安全”的证明。
- 全部判定以 2026-11-28 的工作树为准；行号随代码演进而移动，复核以文件内容与 scan ID 为准。
- 未取得项已标 `[待验证]` 或“阻塞”，未以推断替代证据。
