# CUI 手册（T119、§12.2）

**版本**：`0.1.0` | **更新**：2026-10-25 | 契约：`contracts/cui-cli.md`

## 1. 全局约定

- 所有 JSON 输出含 `schema_version`；错误对象恒四字段（`code`/`message`/`retryable`/
  `details`）；稳定退出码：成功 `0`、用法/参数错误 `2`（clap 解析失败），错误按类别
  `10`（权限）/`20`（门禁）/`30`（运行时）/`40`（背压）/`50`（状态）/`55`（已登记但该端
  未提供，`not_supported`）/`60`（不存在）/`70`（I/O）/`80`（解析）/`90`（超时）/`99`（内部）。
  依赖退出码的脚本按 `55` 判“端未提供”、按 `60` 判“对象不存在”；`55` 为 2026-10-24
  的 BREAKING 变更（原保留命令用 `50`），见 `CHANGELOG.md`。
- `--workspace DIR` 指定工作区（默认 cwd；数据库 `<workspace>/whcode.duckdb`）；
  `--json` 可放在子命令前后（全局旗标）。
- 会话定位为显式 `--session`/`--task`：不设隐式“当前会话”（脚本路径不依赖隐藏状态）。
- 非交互脚本路径可用（ADR-009）：`--json` + 退出码即可编程消费。
- 权限：全部命令经应用服务统一判定，拒绝不可重试（FR-604）。

## 2. 会话命令

```bash
whcode session new --title "修复登录"          # 创建（自动分配 S- 前缀 ID）
whcode session list [--all]                    # 列表（默认不含归档）
whcode session show --session S-1              # 元数据
whcode session rename --session S-1 --title "新标题"
whcode session archive --session S-1 [--unarchive] [--dry-run] [--yes]
whcode session delete --session S-1 [--dry-run] [--yes]   # 破坏性：默认只预览
```

归档属契约 §2-1 破坏族（ADR-038）：默认只输出 dry-run（`dry_run=true`、`current_archived`、
`archived`、`changed`），`--yes` 才执行并写 `session_archive` 审计；`--dry-run` 与 `--yes`
互斥。旧脚本若直接依赖“归档立即生效”，须显式加 `--yes`（属行为变更，见 CHANGELOG）。

删除有预览/确认/审计/活跃会话保护（FR-509）四项机制；显式按 ID 删除（`--yes`）时
不套用 TTL 的活跃窗口保护（否则"删除刚创建的会话"会被静默跳过）；`db purge` 仍保留
活跃窗口保护。

## 3. 任务命令

```bash
whcode task list --session S-1
whcode task show --task T-1        # 依赖/步骤/产物引用/最近事件/失败原因
whcode task retry --task T-1       # 仅 failed/aborted/interrupted 可重试
whcode task abort --task T-1       # 终态与 unknown 不可中止（FR-410）
```

## 4. SDD 门禁

```bash
whcode spec status --session S-1    # 阶段（状态机：Constitution→Specify→Clarify→
                                    # Plan→Tasks→Analyze→Implement→Checklist→Converge）
whcode spec next --session S-1
whcode spec analyze --session S-1   # 五类缺陷（遗漏/孤儿/无效引用/循环依赖/覆盖不足）
whcode spec converge --session S-1  # 验收四态 + 差异报告
whcode spec force-advance --session S-1 --to plan --reason "理由"   # reason 必填
```

高优先级澄清未解决时 Plan 被拒；Analyze 未通过时 Implement 被拒（FR-307）——
仅提示词要求不构成门禁（FR-308）。

## 5. 上下文与运行时

```bash
whcode context budget --session S-1 [--budget N]   # 预算/来源/截断原因/游标（FR-105/108）
whcode runtime list --json                         # 运行时清单（与 GET /api/runtimes 同形）
```

- `context budget` 缺省预算取展示默认口径；**会话不存在返回 `60`（not_found）**，
  不把“会话不存在”伪装成空预算；JSON 含 `schema_version`、预算、来源、截断原因与游标。
- `runtime list` 只读声明清单（`schema_version`/`wired`/`runtimes`），不触库、不启用运行时。
  `runtime status/enable/disable/install` 属 M8 装配开关，返回 `55` + `not_supported`
  （`details.surface_id`、`state`、`milestone`）。

## 6. 事件

```bash
whcode events list --session S-1 --after 100
whcode events export --session S-1 --after 0 --format jsonl   # 首行头部注明脱敏策略
```

`redacted`（默认）级别下疑似秘密字段替换为 `[REDACTED]`（FR-203/205、D-05）。

## 7. 数据库维护

```bash
whcode db stats                     # 文件大小/WAL/已用块/可用块（FR-507/510）
whcode db check                     # 必需索引检查
whcode db purge --before 1700000000000 [--dry-run] [--yes]   # 清理（预览/确认/审计/活跃保护）
whcode db checkpoint                # 显式 checkpoint
whcode db compact [--dry-run] [--yes]   # 复制—校验—切换（不承诺缩小文件，FR-510）
whcode db archive --export PATH     # 工作区导出（whcode-export v1）
whcode db explain --query "SELECT ..."   # 只读查询的查询计划
```

`db vacuum` **不存在**：磁盘压缩能力命名为 `db compact`（FR-510、R-09）。

## 8. 诊断

```bash
whcode doctor [--json]              # 环境与配置自检
```

## 9. 实现状态（2026-10-24 四端对等收口）

已接线（真实执行，退出码见 §1）：
`session new/list/show/rename/archive/delete`、`status`、`task list/show/retry/abort`、
`events list/export`（json/jsonl）、`context budget`、`runtime list`、`spec status`、
`db stats/check/purge/compact/archive/explain`。

未接线（显式失败，退出码 `55` + `not_supported` 信封，不静默成功）：
`run`（Provider 流式装配）、`events show`（按 ID 查询接口未提供）、
`spec next/validate/analyze/converge/force-advance`（需 SDD 产物工作区上下文）、
`runtime status/enable/disable/install`（运行时装配开关属 M8）、
`db checkpoint`（压缩内部步骤，不单独公开）。

同命令一致性：读命令见 `tests/contract/test_tui_cui_parity.rs`
（CUI 视图 vs Web `/api/*` 同数据）；写命令四端同源 `whcode-app::commands` 门面，
最终状态、审计与权限拒绝码的一致性由 `tests/contract/test_frontend_parity.rs` 的行为等价
夹具证明（FR-623）。批次 2 取证：`artifacts/cui-2026-10-22-batch2.txt`；
四端对等收口证据：`artifacts/frontend-parity-2026-10-24/`。

## 10. Web 服务（`serve --web`，ADR-023／ADR-040）

```bash
whcode serve --web                                   # 回环默认（127.0.0.1:8080），HTTPS，无认证
whcode serve --web --bind 192.168.5.4:8737 --token-file /secure/token.txt   # 局域网（RFC1918）必须带 token 文件
whcode serve --web --tls external --bind 127.0.0.1:8443                     # 企业签发证书（按次覆盖设置）
whcode serve --web --tls off                                                # 显式回到明文 HTTP（迁移旧脚本）
```

- 地址边界：只允许回环或 RFC1918 私网；公网 IP 与 `0.0.0.0` 拒绝启动（“仅限局域网”）。
- 局域网监听强制认证与 Origin 检查；token 只从文件读取（不经命令行）。
- **入站 TLS 三档**：`builtin`（默认，每工作区 ECDSA P-256 自签证书）、`external`
  （上传的证书链 + 未加密私钥）、`off`（明文 HTTP，ADR-023 现状）。
  - 优先级：`--tls <mode>` 参数 > 工作区设置（`settings.json` 的 `inbound`）> 默认
    `builtin`；参数是按次覆盖，**不回写**工作区设置。未知取值报 `parse_error`，不回落默认档位。
  - `builtin`/`external` 的证书材料不可用时**拒绝启动**（错误给出证书库路径与 `--tls off`），
    不静默降级明文；`--tls off` 不触碰证书库。
- **证书库路径**：`<workspace>/.whcode/certs/`——`settings.json`、`ca-builtin.pem`、
  `ca-extra.pem`、`ca-builtin.json`、`server-cert.pem`、`server-key.pem`；全部为公开格式
  （PEM／JSON），可直接编辑、删除或 `reset`。私钥只进不出（不入日志、事件、审计与响应）。
- **设置页**：`https://<addr>/settings/certs`（无 JS 时显示只读状态与文件路径）；
  对应 API `GET /api/certs`、`POST /api/certs/modes`、`GET/POST /api/certs/ca`、
  `POST /api/certs/ca/reset`、`GET/POST /api/certs/server`、
  `POST /api/certs/server/self-signed`、`POST /api/certs/verify`。
  能力为 `settings.cert.read`／`settings.cert.write`；降档（入站 `off`、清空内置根、
  上传外部私钥）须 `confirm = true`，成功写 `tls_config_change` 审计。
- **变更需重启**：入站档位与证书替换在重启 `serve --web` 后生效（不热切换监听）；
  响应携带 `restart_required`。出站 CA 变更立即生效（每次请求重新读取）。
- 浏览器凭据：`https://<ip>:<port>/#token=<token>`——fragment 不发送给服务器；
  前端以 `Authorization: Bearer` 调用 `/api/*`。
- 静态外壳（`/`、`app.js` 等）无认证即可加载；**业务数据全部经 `/api/*` 且需 token**。
- 自签证书默认不被浏览器信任：先核对启动输出／设置页给出的 SHA-256 指纹，再加入例外，
  或改用 `external`。**不承诺**公网暴露、吊销（CRL／OCSP）、mTLS 与暴力破解防护
  （ADR-023／ADR-040 退出条件与不承诺项）。

## 11. 人类可读输出（无 `--json`，T196／ADR-041）

默认输出（不加 `--json`）走终端族排版：**标签列 + 值列**的对齐记录与多记录表格，
目的是让 `whcode session show`、`whcode task show` 一类结果能被人直接扫读，而不是
一坨键值对。实现见 `crates/whcode-cui/src/format.rs`。

- **对齐记录**：标签列固定 12 显示列（CJK 记 2 列），值左对齐；首行为标题（如
  `会话 S-1`）；空值只输出标签、不补齐尾部空格。
- **表格**：`session list` 等多行结果按列宽对齐，末列不补尾随空格；空列表输出
  `cui.empty` 显式空态，不输出空白行。
- **强调仅限非颜色**：`Styler::detect()` 只在 stdout 是交互终端、未设 `NO_COLOR`、
  且 `TERM` 不为 `dumb` 时启用粗体等 ANSI 强调；三个条件任一不满足即降级为纯文本。
  本端**不输出颜色、不做主题选择**（ADR-032 `CUI_NO_THEME`），主题色值属 TUI／Web／GUI。
- **管道可重现**：重定向到文件或管道时只输出 UTF-8 纯文本与空格对齐（列间两个空格、
  末列不补尾随空格），不含 ANSI 转义序列、不含 Unicode 线框字符，可用于逐字节比对。
- **边界不变**：`--json` 信封（含 `schema_version`）、错误对象四字段与全部退出码
  均不受本次排版影响；脚本路径继续按 §1 的退出码判定。

证据：`crates/whcode-cui/src/format.rs` 的单测（对齐、仅字重无颜色、CJK 表格、空表）
与 `crates/whcode-bin/src/cui_dispatch.rs` 的 `human_output_is_record_style_and_plain_without_tty`；
归档见 `artifacts/ui-modern-2026-10-25/`。
