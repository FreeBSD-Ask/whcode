# 域 O：远程、协作与分享

> DSH 证据根：`local/dsh-src`（commit `fcc0ad4`）；WHCode 证据根：本仓库。

## 1. 对照表

| 项 | DeepSeek Harness (DSH) | WHCode | 判定 |
|---|---|---|---|
| 远程定位 | SSH provider family：把文件系统/子进程/沙箱实现搬到远端主机，Harness 与 Session 存储留在本地（`docs/subsystems/ssh.md`） | SSH 前端：内置 SSH 客户端连入远端**作为界面通道**（`crates/whcode-ssh/src/lib.rs:8`—`17`） | **语义不同不可比**：DSH 是执行世界迁移，WHCode 是访问通道 |
| 远端信任 | 部署方 OpenSSH 认证；helper 在远端用可信本地 provider 执行（`ssh.md`） | 主机密钥 TOFU：首次记录指纹并打印供人工核对（`lib.rs:8`） | **WHCode 领先**（TOFU 显式） |
| 远端能力 | 远端 FS、Bash、终端、LSP、PTC 共享同一执行坐标；Web 工作区视图需另行适配（`ssh.md`） | 命令白名单，白名单外结构化拒绝；任意系统 Shell 是**独立权限**，默认关闭＋显式授权名单（`crates/whcode-ssh/src/policy.rs:28`—`38`） | **WHCode 领先**：默认拒绝 |
| 取消/失败 | 管理 RPC 截止时间不等于执行截止；SSH 断连使挂起操作失效；helper EOF/信号/租约到期启动远端清理；**不重连重放**（`ssh.md`） | 连接失败结构化报错；无远端执行重放概念 | **DSH 领先**：失败语义更完整 |
| 协作/分享 | LiveShare：owner 把单个 Session 分享给外部 viewer，viewer **拿不到 owner 的工具**；view 只读沙箱、write 有界写沙箱；fork 有独立开关注入 ceiling（`docs/subsystems/liveshare.md`） | 无 | **DSH 独有** |
| 分享的权限工程 | 每个带路径工具都有 confined replacement，按授权根解析；`bash/pwsh/cua/plugin_manager/schedule_*` 一律拒绝；子会话未安装 guard 则拒答（`liveshare.md`） | 无 | **DSH 领先**：这是可借鉴的“授权根”范式 |
| Webhook | 认证外部投递 → 可信规则 → 创建普通 root Session；尽力而为、无队列/重试/去重/状态（`docs/subsystems/webhook.md`） | 无 | **DSH 独有** |
| 定时 | 宿主级 schedule（域 G） | 无 | **DSH 独有** |

## 2. DSH 实现解剖

### 2.1 SSH 是“换执行世界”，不是“远程打开文件”

文件系统身份、可执行查找、进程 cwd、沙箱工作区根、LSP 文件 URL 全部指向 SSH 主机
（`ssh.md`）。因此 Bash、PTY、LSP、PTC 自动跟着走，**没有为 SSH 复制一套工具**
（`ssh.md`）。Web 工作区视图若假设本地 FS，则不会被自动远程化——文档明确写了这个缺口
（`ssh.md`）。

### 2.2 通道与信任分离

管理 RPC 走 helper 的 SSH exec 流；普通 stdin/stdout/stderr 与可选 fd 7 控制流走**分别认证**的
转发 Unix socket，各自有独立 SSH 通道窗口（`ssh.md`）。控制流不会被大输出饿死。

### 2.3 不重放已可能执行的远端动作

“SSH 丢失使挂起操作失效；client 如实报告未确认结果，**从不重连重放可能已执行的动作**”
（`ssh.md`）。这与 WHCode 的 `safe_to_retry` 同源：不确定就报告不确定，不猜。

### 2.4 LiveShare 用“替换工具”而不是“过滤工具”

view 档只放行 `read/read_image/glob/grep/present/todo_write/skill/web_search/web_fetch` 与 PTC
`run_code`；每个带路径的工具都有一个**受限于授权根的替代实现**，其余名字（`bash`、`pwsh`、
`cua`、`plugin_manager`、`schedule_*`）直接拒绝（`liveshare.md`）。这是白名单＋替代，
不是黑名单＋检查。文档同时诚实标注：`run_code` 的程序按 **owner 的常驻文件策略**运行，
可以读写授权根之外（`liveshare.md`）。

### 2.5 撤销与降级

viewer 撤回会取消受影响子任务与挂起调用；`write → view` 降级会拒绝后续写，但不承诺立即
取消正在跑的 turn；移除分享会撤销记录的子会话并停止源会话运行中的 turn（`liveshare.md`）。
“不承诺立即取消”是诚实的边界。

## 3. WHCode 现状核对

- 已实现：SSH 客户端（russh，ADR-030）、TOFU 主机密钥校验、命令白名单、系统 Shell 默认关闭
  且需配置开关＋显式授权名单（`crates/whcode-ssh/src/policy.rs:28`—`38`）。
- 已实现：SSH 前端与 TUI/Web/GUI 共享应用服务，不自行开库（ADR-018/030）。
- 缺失：远端执行世界（远端 FS/进程 provider）、会话分享、LiveShare、webhook、定时。
  检索 `liveshare`、`webhook`、`remote_provider` 于 `crates/` 无实现命中。
- 差异：WHCode 的 SSH 只解决“在远端终端里用 WHCode”；DSH 的 SSH 解决“用本地 Harness
  操作远端机器”。两者方向相反。

## 4. 差异判定与可借鉴点

1. **“换执行世界”抽象**：不采纳（当前）。WHCode 无 shell/进程工具，远端 FS provider 没有
   消费方；若 M8 接运行时，应先把 FS/进程抽成 seam，再加 SSH provider。
2. **TOFU + 默认拒 Shell**：保持。这是 WHCode 相对 DSH 的明确安全优势，应写进对外说明。
3. **不重放可能已执行的动作**：可采纳为 SSH/远端操作纪律，并与 `safe_to_retry` 合并成一条
   “不可判定即上报未确认”的通用规则。
4. **LiveShare 的授权根＋工具替换**：可借鉴其范式。若将来做“只读分享”，必须用**替代实现**
   而不是在工具内加 if；且 `run_code` 类能力要么不分享，要么明确标注逃逸面。
5. **Webhook 尽力而为**：不采纳。无队列/重试/去重意味着重复投递会重复建会话；WHCode 的
   资源与审计纪律要求可判定，至少要有幂等键。
6. **定时/分享**：暂不采纳（见域 G/M 的资源理由）。

## 5. 待验证项

1. DSH SSH helper 的部署与校验（`ssh-helper` 独立仓库/发布流程）：`ssh.md` 提到安装产物校验，
   具体机制未读。取证：读 `packages/ssh/ssh/README.md` 与 `.github/workflows/publish-ssh-helper.yml`。
2. LiveShare 的 `run_code` 逃逸是否被文档视为已知缺口：`liveshare.md` 明说按 owner 策略运行。
   取证：读 `packages/experimental/liveshare/README.md#known-limitations`。
3. WHCode SSH 是否支持端口转发与多跳：只读了策略与 TOFU。取证：读 `crates/whcode-ssh/src/lib.rs`
   的连接参数与 `crates/whcode-bin/src/serve.rs` 的 `serve_ssh`。

## 6. 证据路径清单

- `docs/subsystems/ssh.md`（执行世界迁移）
- `docs/subsystems/ssh.md`（通道与信任）
- `docs/subsystems/ssh.md`（不重放）
- `docs/subsystems/ssh.md`（Web 视图缺口）
- `docs/subsystems/liveshare.md`（分享定位）
- `docs/subsystems/liveshare.md`（run_code 逃逸面）
- `docs/subsystems/liveshare.md`（ceiling 与替代工具）
- `docs/subsystems/liveshare.md`（撤销/降级）
- `docs/subsystems/webhook.md`（webhook 定位）
- `crates/whcode-ssh/src/lib.rs:8`（TOFU）
- `crates/whcode-ssh/src/policy.rs:28`（Shell 默认关闭）
