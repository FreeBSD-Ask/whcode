# ADR-079：出站 webhook 的幂等投递（P6.4）

- **状态**：已批准（2026-11-15，proposal-v3 P6.4；用户指令“推并继续其他开发”）
- **来源**：`research/synthesis/proposal-v3.md` §4 域 O 的 P6.4（“webhook 幂等投递，先修 DSH
  反例：幂等键＋去重表”）与反建议表“webhook 尽力而为（无幂等）→ 重复投递重复建会话”；
  ADR-031（网络栈只在 `whcode-llm`）、ADR-002／ADR-018（分层与 SQL 归属）、ADR-046（证据态）、
  ADR-080（显式状态机与乐观锁的先例）
- **责任人**：集成面维护者、存储维护者
- **退出条件**：见 §6（绑定命令与失效条件）

## 1. 问题与边界

本仓此前没有任何出站通知面：外部系统（CI、看板、协作工具）只能轮询。DSH 的 webhook 是
“尽力而为”——同一条事件重复投递会重复触发下游副作用（模板里最常见的反例是重复建会话、
重复发通知），而副作用无法事后撤回，只能事前拦住。

本 ADR 建立**幂等投递内核**：幂等键＋去重表＋有界重试＋显式触发命令。边界（非目标）：

- 不做入站 webhook（接收第三方回调）；
- 不做后台常驻投递循环（NFR-211：不为会话或进程增加常驻资源；投递只发生在显式命令内）；
- 不做 HMAC 签名（出站凭据只支持 Bearer，取值来自环境变量；签名方案等真实接收方提出要求
  再评估，见 §6）；
- 不做响应正文解析与落库（读有界字节后丢弃）；
- 本轮不给 Web／TUI／GUI 入口：命令面即交付面，界面入口登记为后续任务。

## 2. 决策

1. **配置**：`<workspace>/.zcode/webhooks.json`，显式 opt-in；**无文件＝零端点＝零请求**
   （与 `.zcode/mcp.json`／`.zcode/lsp.json`／`.zcode/shell.json` 同模式，NFR-213 精神）。
   上限：端点 ≤8、每个端点事件类型 ≤16、`batch` 1—50、`max_attempts` 1—5、
   `timeout_ms` 500—10000。URL 必须 `https`；`http` 只允许回环主机
   （`127.0.0.1`／`::1`／`localhost`），用于本地联调。凭据只写环境变量**名**
   （`token_env`），值不进配置、库、日志与事件。
2. **幂等键**：`<endpoint>:<event_id>`，为 `webhook_deliveries.idempotency_key` 主键
   （schema v13，迁移 `0013_webhook_deliveries.sql`）。认领用
   `INSERT ... ON CONFLICT DO NOTHING`，影响 0 行即“已认领过”，不再投递——**同一条命令
   重复执行不会重复投递**。
3. **状态封闭三态**：`claimed`（已认领、结果未知）／`delivered`／`failed`。`delivered` 为
   终态；`failed` 在 `attempts < max_attempts` 时可重新认领（`attempts + 1`），达到上限即
   终态。状态未知字符串在读取时显式报错，不默认为 `failed`。
4. **重试分类**：2xx＝成功；408／429／5xx 与传输失败（连接、超时、TLS）＝可重试；
   其余 4xx＝**永久失败**（`attempts` 直接置为上限，不再重试）——接收方说“不要再发”就
   不再发，不用重试去赌。
5. **载荷白名单**：`event_id`／`session_id`／`kind`／`seq`／`ts`／`payload`（`payload` 经
   `whcode_llm::trace::sanitize` 按 `redacted` 级别处理，与导出同口径），整体 ≤32 KiB，
   超限截断并标 `truncated: true`。请求头 `Idempotency-Key` 携带幂等键，接收方可据此去重。
6. **网络栈位置**：出站 POST 放在 `whcode-llm` 的 `http` feature（`http::webhook`），
   装配根（`whcode-bin`）实现 app 侧 `WebhookSender` 并注入；app 只做计划、判定与记账，
   不持网络栈（ADR-031：网络能力只经装配根进入）。
7. **命令面**：`webhook status`（读类）与 `webhook deliver`（写类，默认 dry-run，
   `--yes` 执行）。未配置端点时 `status` 输出“未配置端点”、`deliver` 返回 0 条并说明原因——
   不静默成功。
8. **导出口径**：`webhook_deliveries` **不随工作区导出**（机器本地投递状态，含端点名与
   错误文本；重建即可），明确登记进 `EXCLUDED_TABLES` 并写明理由——不重演
   `approval_rules`“既不导出也不登记”的缺口。

## 3. 正负和解

**正面**：同一事件不会被重复投递；失败有界重试且区分永久失败；投递事实可查询；零配置零网络。

**负面与承担者**：

- 配置与命令各多一层（承担者：集成使用者）；
- 去重表只增不减，长期运行需要清理（本轮不提供清理命令，登记为已知限制；承担者：维护者）；
- 端点改名会让同一事件在新名下重新投递（幂等键含端点名，重命名即视为新端点；承担者：
  配置维护者）。

**和解理由**：幂等的成本是一次主键插入与一张小表，收益是不重复触发不可撤回的外部副作用。
对“会产生外部副作用”的动作，这个交换是必需的；不做幂等就只能靠人工对账。

## 4. 三重时间

- **过去**：DSH 反例（无幂等 → 重复建会话）、`proposal-v3` §5 反建议表已明确拒绝
  “尽力而为”形态；本仓 `.zcode/*.json` opt-in 配置先例（ADR-054／ADR-078）；
- **现在**：schema v13＋`whcode-store::webhooks`＋`whcode-app::webhooks`＋`http::webhook`
  出站 POST＋两个 CUI 命令；判据见 Check 436—439；
- **将来失效条件**（绑定命令＋判定）：`cargo test -p whcode-store webhooks` 或
  `cargo test -p whcode-app webhooks` 红 → 停止交付；出现“重放历史事件”的真实需求 →
  复审决策 2 的键口径；接收方要求签名鉴权 → 复审决策 1 的 Bearer-only。

## 5. 回退方案

删除 `0013_webhook_deliveries.sql`、三个模块与两个 CUI 命令，并从 `EXCLUDED_TABLES` 移除
`webhook_deliveries`；已迁移库保留空表，无工作区数据丢失（表内只有投递状态）。回退后
`.zcode/webhooks.json` 成为未使用的文件，删除或保留都不影响运行。

## 6. 已知限制与失效条件

- **无后台投递**：不运行 `webhook deliver` 就没有投递。这是设计而非缺陷；常驻投递需先解
  NFR-211（决策 D13）；
- **`claimed` 悬置**：认领与结果之间进程崩溃会留下 `claimed` 行，该事件不会自动重试
  （重试可能与已发生的投递重复）；处置方式是按幂等键人工核对接收方后再改库（未提供命令），
  承担者是维护者；
- **无清理命令**：`webhook_deliveries` 只增不减；行数上界由“事件数 × 端点”决定，长跑需
  手工清理（未提供命令，登记为缺口）；
- **无签名**：只支持 Bearer；共享密钥型接收方需在网关侧承载；
- **响应只记状态码**与前 128 字符传输层错误文本（不含请求头，不含凭据）；
- **端点改名即新端点**：同一事件会以新幂等键重新投递；
- **无界面入口**：Web／TUI／GUI 未提供入口（命令面交付），四端对等以 CUI 命令面为准；
- **`http` 仅回环**：非回环明文端点被配置校验拒绝，不做“临时放宽”。

## 7. 文献依据（标准研读记录，§1.6）

- **RFC 9110 §9.2.2（幂等方法语义）**：POST 不是幂等方法，因此幂等性必须由**显式键＋去重
  记录**提供，不能依赖方法语义——决策 2 的直接依据；
- **IETF `draft-ietf-httpapi-idempotency-key-header`（草案，非标准）**：`Idempotency-Key`
  头命名参照；草案状态变化或转正即复审（本仓不声称符合该草案）；
- **ISO/IEC/IEEE 29148:2018**（已登记，已覆盖）：每条判据必须可判定——Check 436—439 写成
  “命令＋红灯条件”；
- **ISO/IEC/IEEE 15026-2:2022**（已登记，已覆盖）：claim→argument→evidence——幂等键
  （claim）／去重表（argument）／测试证据（evidence）；
- 一手先例：本仓 ADR-080 的显式状态机与乐观锁（“并发冲突不静默覆盖”），本 ADR 把它换成
  “重复投递不静默重发”。

## 8. 形式审查

- **真实矛盾**：外部副作用无法事后撤回，而重复投递可以事前拦住；本仓缺的正是“事前”的那一步；
- **正负和解**：见 §3，登记门槛与表增长成本显式，承担者明确；
- **0 信任校验**：幂等键唯一性、永久失败不重试、`attempts` 上限、缺配置零请求、明文端点
  仅回环，全部由测试钉住；
- **知难行易**：方向＝P6.4“幂等键＋去重表”；目标＝同一事件不重复投递、失败有界且可查；
  判据＝store／app／llm 三处测试全绿且 `webhook status` 输出可判定；非目标＝入站 webhook、
  常驻投递、HMAC 签名、界面入口。

## 修订记录（2026-11-17：`.zcode` 更名 `.whcode`，用户指令）

**触发**：用户指令「全部改为 whcode 前缀」——产品名为 WHCode，工作区配置目录沿用 ZCode
的 `.zcode` 前缀造成品牌歧义；ZCode 配置互操作的价值由用户裁量终止。

**决策增量**：本 ADR 记录的配置路径 `.zcode/…` 统一更名 `.whcode/…`。只改名字：schema、
上限、显式 opt-in、fail-closed 语义与测试判据全部不变。既有工作区的 `.zcode/*` 配置
需手动迁移至 `.whcode/*`。历史正文、`research/zcode-comparison/` 与 `local/zcode-src`
证据锚点保留原名不改写。
