# 域 N：安全

> DSH 证据根：`local/dsh-src`（commit `fcc0ad4`）；WHCode 证据根：本仓库。

## 1. 对照表

| 项 | DeepSeek Harness (DSH) | WHCode | 判定 |
|---|---|---|---|
| 安全声明 | 明示“实验性开发者预览、未做安全审计、不得视为安全或生产可用”；列出沙箱局限与责任豁免（`SAFETY.md`） | 安全优先列为项目第二优先序；默认回环、redacted、最小权限；放宽需用户批准＋ADR（`AGENTS.md` §1.1） | **取向不同**：DSH 声明边界，WHCode 约束设计 |
| 凭据模型 | 配置只存**引用**（env 名），provider 持有值；每次操作重新解析，轮换无需重启；`describe()` 只返回来源与可写性，返回值类型里**没有放值的槽**（`docs/subsystems/credentials.md`） | 凭据只存本地文件，不落库、不进日志与事件；错误消息 scrub 已注入 secret（`crates/whcode-llm/src/http/profile.rs:217`） | **同构**，DSH 的“无槽位”设计更彻底 |
| 空值语义 | seam 级统一：**空存储值在所有 provider 中都视为不存在**（`credentials.md`） | 未配置即报错，不静默用空 | **同构** |
| 鉴权 | 本地回环为主；LAN/远端需显式配置 | LAN 监听必须 `--token-file`；Bearer 恒定时间比较；token 不经 URL query（`crates/whcode-web/src/security.rs:26`—`71`；`serve.rs:309`） | **WHCode 领先** |
| Origin/CSRF | 由 Connection 的 trust checks 负责（`docs/subsystems/web-client.md`） | `security::check_origin` 校验 scheme+host；回环缺 Origin 放行（同源 GET/本机 CLI）（`serve.rs:36`—`37`；ADR-022） | **同构**，WHCode 规则写死可测 |
| 沙箱 | 文件效果沙箱三模式＋三平台后端，强度上报 `full`/`partial`（域 C） | 无（工具仅路径绑定） | **结构性缺失（WHCode）** |
| 遥测 | 产品遥测 OTLP/HTTP：调用方自选事件与字段；exporter **不自动收集会话数据或标识**；仅 Desktop 收集（`docs/subsystems/product-telemetry.md`） | 无外部遥测；追踪默认 `redacted`，四级可调，`full` 需界面警告（`crates/whcode-domain/src/trace.rs:3`—`20`） | **WHCode 领先**（本地优先） |
| 匿名身份 | 有 `identity` 包（共享匿名身份） | 无账户/身份概念（明令禁止强制账户） | **WHCode 领先**（自由原则） |
| 审计 | 审批成对事件记日志；无独立审计表 | `audit` 表 11 类动作，失败阻断执行，不随清理删除（`crates/whcode-store/src/audit.rs`） | **WHCode 领先** |
| 注入面 | Web 前端允许 React 渲染；由组件与 Slots 约束 | Web 禁用 `innerHTML/outerHTML/insertAdjacentHTML`（脚本检查即失败，`check-frontend-budget.py:41`—`42`） | **WHCode 领先** |

## 2. DSH 实现解剖

### 2.1 安全声明是“威胁模型的替代品”

`SAFETY.md` 明确：未审计、实验性、可执行模型生成的代码与命令、可加载第三方插件、可访问网络/
进程/凭据/文件；沙箱与审批“降低风险但不保证隔离”（`SAFETY.md`）。它没有给出威胁
模型与攻击面清单，而是给出使用建议（最小权限、一次性 VM、备份、审查插件）。这是一种
**责任边界声明**，不是安全设计。

### 2.2 凭据引用化

配置里只有 `CredentialRef`（POSIX 环境变量名，品牌类型防混用）；`resolve()` 每次操作重新
解析并返回来源层（`env`/`file`/`project-env`/`user-env`），因此轮换即时生效；
`describe()` 的返回结构没有值字段（`credentials.md`）。这是“结构上无法泄露”的做法，
比“约定不打印”更强。

### 2.3 遥测默认不收集会话

产品遥测需要调用方显式选择事件与字段；exporter 不自动收集会话数据或标识；普通 Web 客户端
不收集（`product-telemetry.md`）。默认关闭、按消费者开启。

### 2.4 信任检查在 Connection

前端文档把“信任检查”归给 Connection（`web-client.md`），但未展开具体规则（Origin？token？
本机判定？）。这一点在 DSH 侧证据不足，标 `[待验证]`。

## 3. WHCode 现状核对

- 已实现：追踪四级（默认 `Redacted`，`Full` 需界面警告）（`crates/whcode-domain/src/trace.rs:3`—`36`）。
- 已实现：Bearer 恒定时间比较、token 不经 URL query、Origin 校验、LAN 必须 token（`crates/whcode-web/src/security.rs:26`—`91`；`serve.rs:309`）。
- 已实现：TLS 信任库（ADR-040）、凭据不落库（ADR-031）、错误消息 scrub。
- 已实现：审计表与失败阻断；Web 前端禁用 DOM 注入 API（脚本门禁）。
- 缺失（检索 `sandbox`、`bwrap`、`seatbelt` 于 `crates/` 零命中）：进程沙箱；工具级审批 broker
  （T255 未做，域 C）。
- 差异：WHCode 的安全面是“默认关闭＋可判定”，DSH 的安全面是“默认开放＋声明风险”。

## 4. 差异判定与可借鉴点

1. **`describe()` 无值槽位**：可采纳。WHCode 的配置/诊断输出应定义“结构上不含 secret”的类型，
   而不是依赖调用方记得脱敏；这与 CodeQL 的敏感数据流治理同向（AGENTS.md §5.2）。
2. **每次操作重新解析凭据**：可采纳。当前 profile 在装配期读取凭据；改成请求期解析可支持
   轮换，且与 DSH 的“不缓存”一致。代价：每请求一次文件/env 读取（可缓存到 mtime）。
3. **遥测默认关闭**：保持并写明。WHCode 无遥测是自由原则的体现；若将来加，必须 opt-in 且
   默认不含会话内容与标识。
4. **沙箱**：不采纳（当前，同域 C）。若接 shell 执行，必须先有威胁模型与强度上报字段。
5. **不采纳** “未审计”作为结项理由：WHCode 的安全措施必须有威胁模型、证据、代价与失效条件
   （AGENTS.md §1.3-6），不能只写声明。

## 5. 待验证项

1. DSH Connection 的信任检查具体规则（Origin 白名单？本机判定？）：`web-client.md` 只有一句。
   取证：读 `packages/client/connection/README.md` 的 trust policy 节。
2. DSH 是否有凭据落盘加密：`credentials-local` 的存储格式未读。取证：读
   `packages/credentials/credentials-local/README.md`。
3. WHCode 追踪 `Full` 级别的界面警告落在哪一端：只读了 domain 注释。取证：
   `rg -n "Full" crates/whcode-tui/src crates/whcode-gui/src | head`。

## 6. 证据路径清单

- `SAFETY.md`（未审计声明）
- `SAFETY.md`（沙箱局限）
- `docs/subsystems/credentials.md`（引用化与空值语义）
- `docs/subsystems/credentials.md`（describe 无值）
- `docs/subsystems/product-telemetry.md`（遥测不自动收集）
- `docs/subsystems/web-client.md`（Connection trust checks）
- `crates/whcode-web/src/security.rs:26`（恒定时间比较）
- `crates/whcode-web/src/security.rs:68`（Bearer 校验）
- `crates/whcode-bin/src/serve.rs:309`（LAN token 强制）
- `crates/whcode-domain/src/trace.rs:3`（redacted 默认）
- `.github/scripts/benchmarks/check-frontend-budget.py:41`（DOM 注入禁用）
