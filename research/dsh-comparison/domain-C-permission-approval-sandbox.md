# 域 C：权限、审批与沙箱

> DSH 证据根：`local/dsh-src`（commit `fcc0ad4`）；WHCode 证据根：本仓库。

## 1. 对照表

| 项 | DeepSeek Harness (DSH) | WHCode | 判定 |
|---|---|---|---|
| 决策粒度 | **单次动作**：`approval.request()` 问“这一件事能不能做”，结果 `allowed-once` 只授权本次（`packages/interaction/user-approval/src/index.ts:1`—`28`） | **命令级能力**：`Capability` 枚举 + `require()` 集合包含判定（`crates/whcode-app/src/authz.rs:19`、`:124`） | **语义不同不可比**：一个是行为闸门，一个是功能开关 |
| 结果语义 | 封闭四值 `allowed-once`／`rejected`／`cancelled`／`unavailable`，非 `allowed-once` 一律 fail-closed（同上 `:22`—`28`） | 二值：`Ok` 或 `permission_denied`（`retryable=false`）（`authz.rs:124` 起） | **DSH 领先**：区分“没人可问”与“明确拒绝” |
| 审批人 | `approval/request` waterfall；无 answerer 或 answerer 抛错 → `unavailable`（`docs/subsystems/approval.md`） | 交互式确认在 CUI/GUI 命令树内（`crates/whcode-cui/src/confirm.rs`、`crates/whcode-gui/src/lib.rs` 预览→确认两段式） | **同构**：都在人工入口收口 |
| 会话级策略 | `ask`／`never`，取日志中最后一条 `approval/policy`，可重放（`docs/subsystems/approval.md`） | 无会话级策略；策略对象由装配根注入（`authz.rs:7`—`11`） | **DSH 领先** |
| 审计 | `approval/asked`＋`approval/decided` 成对事件，仅日志不落库（`approval.md`） | 审计表 11 类动作，破坏性命令写入；拒绝不写库 | **WHCode 领先**：审计是持久表且有失败阻断 |
| 预设 | `workspace-write+ask`、`danger-full-access+never`；`custom`/`auto` 保留；组合不满足即加载失败（`docs/subsystems/permission-presets.md`） | 无预设；前端各有自己的确认路径 | **DSH 领先** |
| 沙箱 | 三模式 `read-only`／`workspace-write`／`danger-full-access`，后端 bwrap/Landlock/Seatbelt/Windows ACL；`full`/`partial` 作为**上报事实**（`docs/subsystems/sandbox.md`） | **无进程沙箱**；工具在同进程内以工作区路径绑定（拒绝绝对路径与 `..`）兜底（`crates/whcode-tools/src/paths.rs:1`—`8`） | **结构性缺失（WHCode）** |
| 约束面 | 只管文件效果；网络与进程可见性明确“不在词表内”（`sandbox.md`） | 路径绑定只管工具内文件访问；shell 类工具未接（当前仅有六件套文件工具） | **DSH 领先**：边界写得清楚 |

## 2. DSH 实现解剖

### 2.1 审批是一次性凭证，不是开关

`ApprovalOutcome` 是封闭联合，`allowed-once` 的语义被写进类型名：授权随本次动作结束而失效。
调用方（`dsh-tools`、`dsh-tool-bash`）必须对 `rejected`/`cancelled`/`unavailable` 一律拒绝。
“没有 answerer”被映射成 `unavailable` 而不是“默认允许”，这是典型的 fail-closed 设计
（`docs/subsystems/approval.md`）。

### 2.2 请求不带参数，靠 callId 关联

`ApprovalRequest` 刻意**不复制工具参数**，只带 `callId` 指向已经流式展示的工具调用，
避免“给用户看的参数”和“真正执行的参数”两份漂移（`approval.md`）。这条设计
直接消灭了一类 TOCTOU 型欺骗：审批界面展示的内容与执行内容必须是同一份。

### 2.3 策略折在日志里

`ask`/`never` 的有效值 = 会话日志中最后一条 `approval/policy`，回退到服务配置；
`setApprovalPolicy()` 是唯一写入口，因此重放能重建覆盖（`approval.md`）。
策略变化还会作为 runtime-context 快照追加进 `user/message`，让模型知道“现在没人可问”。

### 2.4 沙箱只承诺文件效果，并如实上报强度

模式词表只覆盖文件写效果；网络与进程可见性被显式排除（`sandbox.md`）。
`SandboxEnforcement` 区分 `full` 与 `partial`，Windows ACL 后端的硬链接、非受限读、
AppContainer ACL 边界被列为当前 `partial` 情形（`sandbox.md`）。**“不完全”是
可读的事实而不是沉默**，这比“声称已隔离”更可靠。

### 2.5 预设把两个旋钮绑成一个选择

预设 = `SandboxMode + ApprovalPolicy` 的命名组合；配置了 `custom`/`auto` 直接抛错，
组合到不提供 `sandboxMode` 事实的 shell 执行器上也抛错（`permission-presets.md`）。
即“预设不能撒谎”：没有沙箱能力的后端不允许挂 `workspace-write` 预设。

## 3. WHCode 现状核对

- 已实现：`Capability` 静态能力集与 `require()` 单点判定，拒绝统一 `permission_denied`
  且不可重试（`crates/whcode-app/src/authz.rs:19`、`:124`；模块注释 `:1`—`11`）。
- 已实现：破坏性命令的 dry-run／两段式确认（`crates/whcode-cui/src/confirm.rs`、
  `crates/whcode-gui/src/lib.rs` 的 `OpConfirm`/预览态），无隐式默认。
- 已实现：工具路径绑定（`crates/whcode-tools/src/paths.rs:20`—`45`）与 MCP 权限面
  fail-closed（`crates/whcode-app/src/turn.rs:1249`）。
- 缺失（检索 `approval`、`sandbox`、`allowed-once`、`bwrap` 于 `crates/` 无实现命中）：
  工具级动态审批 broker、会话级审批策略、进程沙箱、权限预设。ADR-057 T255—T257 仍为未勾选。
- 语义差异：WHCode 的 `Capability` 回答“这个 actor 能不能用这个命令”，DSH 的
  `ApprovalOutcome` 回答“这一次动作是否被允许”。前者可静态证明，后者可细到单次调用。

## 4. 差异判定与可借鉴点

1. **四值封闭结果**：可采纳为 T255 的类型基础。WHCode 现有二值拒绝把“没人可问”和“明确
   拒绝”混为一谈；拆开后可让自动化场景（CI/无头）显式走 `unavailable` 而不是误判为拒绝。
   代价：调用方需处理第四态。
2. **审批请求不复制参数**：必须采纳。T255 若做成“把参数再渲染一遍给 GUI”，就复制了 DSH
   刻意避免的漂移风险；应以 `exec_id` 关联已展示的工具调用。
3. **审批事件成对落审计**：改写采纳。WHCode 已有审计表，无需新事件类；在 `audit` 表加
   `approval_asked`/`approval_decided` 两类动作即可，且要沿用“审计失败阻断执行”的现行口径。
4. **沙箱**：不采纳（当前）。WHCode 的工具面只有同进程文件工具，路径绑定已覆盖主要风险；
   引入 bwrap/Landlock/Seatbelt 需要平台分支（仅 `whcode-platform` 允许）与 Windows ACL
   后端，代价远超当前收益。若将来接 shell 执行（M8），应先立 ADR 并给威胁模型。
5. **`full`/`partial` 事实上报**：可采纳为接口字段。任何未来的隔离能力都应报告强度，
   而不是只报“已启用”，这与 WHCode 的“未知显式”一致。
6. **预设“不能撒谎”**：可采纳为装配期校验。若将来有权限预设，组合到不具备沙箱能力的
   执行器上必须加载失败，而不是降级运行。

## 5. 待验证项

1. DSH `approval/request` 的超时行为：文档未写默认超时，`tool-ask-user` 有 `timed.ts`，
   审批侧未确认。取证：读 `packages/interaction/user-approval/src/index.ts` 中定时器相关代码
   与 `user-questions/src/timed-wait.ts` 的关系。
2. Windows ACL 沙箱的 `partial` 具体边界是否包含“读任意路径不受限”：`sandbox.md` 提到
   unconfined-read，但未给代码位置。取证：读 `packages/sandbox/sandbox-local/src/**` 的
   Windows 后端。
3. WHCode GUI 的确认是否覆盖所有破坏性命令：只核对了 `OpConfirm` 与预览态存在，未逐一核对
   `db purge`、`session delete`、`restore` 三类入口。取证：读 `crates/whcode-gui/src/lib.rs`
   的操作面板分派与 `crates/whcode-cui/src/cli.rs` 的 `--yes` 语义。

## 6. 证据路径清单

- `packages/interaction/user-approval/src/index.ts:1`（审批 seam 定义）
- `docs/subsystems/approval.md`（封闭四值/fail-closed）
- `docs/subsystems/approval.md`（ask/never 策略折叠）
- `docs/subsystems/approval.md`（请求不带参数）
- `docs/subsystems/permission-presets.md`（预设表）
- `docs/subsystems/sandbox.md`（三模式）
- `docs/subsystems/sandbox.md`（full/partial）
- `crates/whcode-app/src/authz.rs:19`（Capability 枚举）
- `crates/whcode-app/src/authz.rs:124`（require 判定）
- `crates/whcode-cui/src/confirm.rs:1`（dry-run/确认）
- `crates/whcode-gui/src/lib.rs`（预览→确认两段式，`OpConfirm`）
- `crates/whcode-tools/src/paths.rs:20`（工作区路径绑定）
- `crates/whcode-app/src/turn.rs:1249`（MCP 权限 fail-closed）
