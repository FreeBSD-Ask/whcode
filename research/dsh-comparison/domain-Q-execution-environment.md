# 域 Q：执行环境：Shell、子进程、终端、LSP、PTC、Workflow、Jobs

> DSH 证据根：`local/dsh-src`（commit `fcc0ad4`）；WHCode 证据根：本仓库。

## 1. 对照表

| 项 | DeepSeek Harness (DSH) | WHCode | 判定 |
|---|---|---|---|
| 分层 | 四层缝：`ctx.subprocess`（进程）→ `ctx.shell`（Bash/Pwsh 执行器）→ `ctx.terminals`（PTY）＋`ctx.jobs`（后台作业）＋`ctx.lsp`（语言服务） | 无 shell 工具；`whcode-runtime` 是**可选运行时**（默认不启用），提供 js/python/gitbash/bsdshell 四 provider（`crates/whcode-runtime/src/lib.rs:4`—`22`） | **DSH 领先**：执行环境是产品主线 |
| 进程模型 | 子进程缝拥有可执行查找、`DSH_*` 受管环境、凭据 scrub（`scrubbedParentEnv`）、`CollectedOutput`（`docs/subsystems/subprocess.md`） | 受控子进程：超时、输出上限、退出码、stderr 尾部（`crates/whcode-runtime/src/worker.rs`）；进程组创建/终止（`crates/whcode-platform/src/process.rs:130`—`292`） | **同构**；DSH 的“执行世界与 FS provider 同命名空间”更彻底 |
| 环境变量 | `DSH_*` 为 Harness 所有：先删继承值再合并当前快照（`shell.md`）；凭据统一 scrub | 最小环境＋显式 env 回填（MCP 侧，`crates/whcode-mcp`）；运行时 payload 哈希校验 | **DSH 领先**：环境命名空间有所有权 |
| PTY | 持久终端：`TerminalSessionId` 品牌 id、按 owner Agent 授权；`TerminalWaitReason` 区分“静默/超时”与“会话退出”（`docs/subsystems/terminal.md`） | 无 | **DSH 独有** |
| LSP | `ctx.lsp` 能力缝：四种封闭语义查询，UTF-16 零基坐标，模型工具负责一基光标转换（`docs/subsystems/lsp.md`） | 无 | **DSH 独有** |
| Workflow | 模型写编排脚本、脚本起子代理；每 context 只允许一个 `ctx.workflowEngine`（`docs/subsystems/workflow.md`） | 无 | **DSH 独有** |
| PTC | `ptc-runtime`：沙箱化 Node 执行；workflow/LiveShare 的 `run_code` 都走它 | 无（`js` provider 无内置引擎，诚实不可用，D-08） | **DSH 领先** |
| 后台作业 | `ctx.jobs`：`<kind>-<uuid>` 品牌 id、owner 授权、可列出/停止/跟随输出（`docs/subsystems/jobs.md`） | 无 | **DSH 独有** |
| 能力门控 | 运行时能力由 provider 声明；沙箱后端按模式提供 | 能力等级 `Full/Restricted/ShellOnly`，未启用运行时不加载动态库/不占端口（FR-702/704，`lib.rs:22`—`26`） | **WHCode 领先**：构建期默认关闭＋等级可见 |
| 校验/隔离 | payload 由部署方安装；sandbox 负责文件效果 | payload 来源/版本/哈希/平台校验（FR-703）；执行前检查点四要素（FR-706） | **WHCode 领先**：产物校验与检查点 |

## 2. DSH 实现解剖

### 2.1 执行世界与文件系统同命名空间

一个 provider 的 spawn 工作目录、可执行路径、普通进程与终端会话，与挂载的 FS provider 处于
同一路径/进程命名空间（`subprocess.md`）。因此把 FS 与 subprocess 一起指向 SSH，Bash、PTY、
LSP 就自动跟着走（域 O）。`resolveExecutable()` 只在该 provider 的**已 scrub PATH** 里解析，
并保留显式覆盖（`subprocess.md`）。

### 2.2 环境变量有主人

`DSH_*` 是 Harness 拥有的子进程事实；模型面 bash 工具经 `ctx.shellEnv` 收集，再经
`ShellExecRequest.dshEnv` 传入，subprocess 服务**先删除继承的 `DSH_*` 再合并当前快照**
（`shell.md`）。这防止“父进程里残留的 DSH_* 泄漏进子进程”。

### 2.3 PTY 的等待语义分开记

`TerminalWaitReason` 独立于 `TerminalSessionStatus`：“静默或超时返回”时顶层 shell 仍活着；
`session_exit` 才表示 shell 退出（`terminal.md`）。这把“没输出”与“死了”分开，避免
模型把超时当崩溃。

### 2.4 LSP 是封闭四查询

只暴露四种语义查询，联合类型封闭——增加一种要跨缝、provider、工具全改（`lsp.md`）。
坐标零基 UTF-16 与协议一致，模型面工具自己处理一基转换。范围收得极小，是可维护性的来源。

### 2.5 Workflow 单引擎

`ctx.workflowEngine` 每 context 只允许一个实现，第二个替换第一个而不是并存（`workflow.md`）。
模型写的编排脚本经 PTC 在**调用会话的文件策略**下运行（`workflow.md`）——这既是能力，也是
逃逸面（域 O 已标注同类问题）。

## 3. WHCode 现状核对

- 已实现：`whcode-runtime` 四 provider（js 诚实不可用、python 经系统解释器、gitbash payload 校验、
  bsdshell 延后），默认 `default = []` 不启用（`crates/whcode-runtime/src/lib.rs:4`—`26`）。
- 已实现：受控子进程（超时/输出上限/退出码/stderr 尾部）；进程组创建、终止、查询
  （`crates/whcode-platform/src/process.rs:130`—`401`）；payload 来源/版本/哈希/平台校验（FR-703）；
  执行前检查点四要素（FR-706/ADR-007）；重复执行缓存仅显式启用（FR-707）。
- 暂无：shell 工具（模型不能执行命令）、PTY、LSP、workflow、jobs。检索 `pty`、`lsp`、
  `workflow`、`job` 于 `crates/` 无实现命中（`whcode-runtime` 的 worker 除外）。
- 差异：WHCode 把“执行”做成**可选、默认关闭、能力分级**；DSH 把“执行”做成**默认开启的
  产品能力**。这是两种安全与体验取舍。

## 4. 差异判定与可借鉴点

1. **环境变量所有权**：可采纳。WHCode 若接 shell 工具，`DSH_*` 式“Harness 拥有的变量”
   必须先删继承值再合并，防止环境污染。
2. **`resolveExecutable()` 走 scrub PATH**：可采纳。当前 runtime 用系统解释器；至少应把
   PATH 解析限定在已知目录，避免 PATH 劫持。
3. **PTY 等待语义分离**：可采纳为设计要点（若做终端）。超时≠退出是模型最容易误判的地方。
4. **LSP 四查询封闭集**：可采纳为“能力面先小后大”的范式；WHCode 若做 LSP，先只做
   definition/references/hover/diagnostics 四类。
5. **Workflow/PTC**：不采纳（当前）。模型写编排脚本会引入新的执行面与逃逸面，与资源铁律
   和静态分层冲突；先有 shell 与审批 broker 再谈。
6. **jobs/终端/PTY**：不采纳（当前）。常驻终端与“每会话不加常驻线程/句柄”冲突，需先有
   句柄预算与回收测试。
7. **保持** “默认不启用运行时＋能力等级可见”：这是 WHCode 相对 DSH 的明确优势，不应为了
   功能对齐而改为默认开启。

## 5. 待验证项

1. DSH `ptc-runtime` 的沙箱边界（Node 进程权限、文件策略来源）：`workflow.md` 说按调用会话
   文件策略。取证：读 `docs/subsystems/ptc-runtime.md`。
2. WHCode `whcode-runtime` 的 `js` provider“无内置引擎”是否意味着该能力完全不可用：只读了
   模块注释。取证：读 `crates/whcode-runtime/src/js.rs`。
3. DSH jobs 的上限与回收策略：`jobs.md` 未给数量上限。取证：读 `packages/jobs/jobs/README.md`
   的 Known Limitations。

## 6. 证据路径清单

- `docs/subsystems/subprocess.md`（进程缝职责）
- `docs/subsystems/subprocess.md`（同命名空间与 scrub PATH）
- `docs/subsystems/shell.md`（DSH_* 环境所有权）
- `docs/subsystems/terminal.md`（等待原因）
- `docs/subsystems/lsp.md`（四查询封闭集）
- `docs/subsystems/workflow.md`（单引擎）
- `docs/subsystems/jobs.md`（JobId/授权）
- `crates/whcode-runtime/src/lib.rs:22`（默认不启用）
- `crates/whcode-platform/src/process.rs:130`（进程组）
- `crates/whcode-platform/src/process.rs:292`（终止树）
