# Web 前端：部署与安全配置（FR-603、FR-606、FR-609、FR-626、FR-627）

> 适用：`whcode serve --web`。事实基准：`crates/whcode-web`、`crates/whcode-bin/src/serve.rs`、
> `crates/whcode-bin/src/tls.rs`、`crates/whcode-app/src/cert.rs`、`docs/adr/ADR-022`、
> `docs/adr/ADR-023`、`docs/adr/ADR-040`（2026-10-25 对照）。本文只描述**当前实际行为**，
> 不把未取得证据的项写成能力；引用用符号名而非行号，避免并行改动造成行号漂移。

## 1. 启动

```bash
whcode serve --web                                  # 默认 https://127.0.0.1:8080/（入站 TLS builtin）
whcode serve --web --bind 127.0.0.1:8787            # 指定端口（仍为 HTTPS）
whcode serve --web --bind 192.168.1.10:8787 --token-file .\token.txt   # 局域网访问（必须带令牌）
whcode serve --web --tls external --bind 127.0.0.1:8443                # 企业签发证书
whcode serve --web --tls off                                           # 显式回到明文 HTTP（迁移旧脚本）
```

| 项 | 当前行为 | 证据 |
|---|---|---|
| 默认监听 | `127.0.0.1:8080` | `whcode_web::bind::DEFAULT_BIND` |
| 配置来源 | 命令行 `--bind`；配置文件无 web/bind 字段 | `serve.rs::ServeCli`、`config.rs` |
| 地址语法 | 仅 IPv4 字面量 + 端口，其余报 `invalid_state` | `bind.rs::parse_addr` |
| 对外监听 | 仅允许 RFC1918 私网地址，且**必须**同时给 `--token-file` | `bind.rs`、`serve.rs::serve_web` |
| 公网与通配 | 公网 IP 与 `0.0.0.0` 一律**拒绝启动** | `bind.rs::bind` |
| 入站 TLS | **默认 `builtin`**（每工作区 ECDSA P-256 自签证书，397 天） | `serve.rs::serve_web`／`prepare_tls`、`cert.rs` |
| 档位优先级 | `--tls <builtin\|external\|off>` > 工作区 `settings.json` > 默认 `builtin`；参数按次覆盖、不回写设置 | `serve.rs::resolve_inbound_mode` |
| 材料不可用 | 启用档位拒绝启动，错误列出证书库路径与 `--tls off`（不静默降级明文） | `serve.rs::prepare_tls` |
| 启动提示 | 打印协议（`https`／`http`）、地址、入站档位、证书指纹与设置页路径 | `serve.rs::serve_web` |
| 明文 HTTP | 仅在显式 `--tls off`（或设置 `inbound = off`）时 | `docs/adr/ADR-023`、`ADR-040` 决策 3 |

## 2. TLS 事实表（入站）

| 项 | 当前行为 | 证据 |
|---|---|---|
| 默认协议 | HTTPS（TLS 1.2／1.3，`rustls` + `ring` 提供者） | `crates/whcode-bin/src/tls.rs::server_config` |
| 自签材料 | 每工作区唯一私钥，不随包分发；SAN 含 `localhost`、`127.0.0.1`、`::1`、本机名与绑定 IP | `cert.rs::generate_self_signed`／`effective_sans` |
| 自签代价 | 浏览器默认不信任；需核对设置页/启动输出的 SHA-256 指纹后手工加例外，或改用 `external` | 本文件 §3、ADR-040“正负和解” |
| `external` 材料 | 证书链 + **未加密**私钥（PKCS#8）；带口令私钥、证书与私钥不匹配一律拒绝 | `cert.rs::validate_server_pair` |
| 私钥读取权限 | 写入后调用 `whcode_platform::restrict_private_file`；与工作区数据库同一 ACL 边界 | `cert.rs::write_server_material` |
| 握手位置 | 状态机在**装配根** `whcode-bin`（`Phase` 复用请求空闲上限）；`whcode-web` **不依赖 `rustls`**（ADR-031／ADR-040） | `serve.rs::accept_ready`、`crates/whcode-web/Cargo.toml` |
| 握手有界 | 沿用 `REQUEST_IDLE_TIMEOUT`（5 s）：未完成即关闭（TLS 下无法先回 HTTP 400） | `serve.rs::pump_reading` |
| 并发上限 | `MAX_CLIENTS = 96`；超限的 TLS 连接**直接关闭**（握手前无法回 503） | `serve.rs::accept_ready` |
| 变更生效 | 入站档位/证书替换需重启 `serve --web`；不热切换监听、不断开现有连接 | `cert.rs` 返回 `restart_required` |
| 生效范围 | 只加密链路，**不是认证替代品**：RFC1918 绑定仍必须 `--token-file` | `serve.rs::serve_web`、ADR-023／040 |

### 2.1 威胁模型与不承诺

- **要防的**：局域网同网段抓包读取 token 与业务数据——这是本轮将默认协议改为 HTTPS 的
  直接动机；浏览器对自签证书的默认拒绝（需核对指纹后由用户显式信任）。
- **不防的（明确不承诺）**：公网暴露与多用户隔离；同用户或管理员进程读写私钥与追加根
  （证书库与工作区数据库同一 ACL 边界）；吊销（CRL／OCSP）与操作系统信任策略——`system`
  档位只用 `rustls-native-certs` 取根集合，链与主机名校验仍由 `rustls` 完成，**不复用**
  Windows 的策略与吊销；mTLS 与客户端证书；私钥口令保护（`external` 私钥为未加密文件）。
- **代价**：自签证书在浏览器中默认不受信任；入站档位与证书变更需重启；连通性校验同步执行，
  单次最多阻塞服务循环 2 秒（见 §5）；每连接一份有界 TLS 缓冲（`tls.rs` 的上限常量）。

### 2.2 信任自签证书（Windows 证书管理器，手工步骤）

本项目**不提供**自动化信任脚本：改动系统信任库必须由用户知情并亲自操作。

1. 启动 `serve --web`，记录 stdout 给出的证书 SHA-256 指纹（设置页 `/settings/certs`
   显示同一指纹）。
2. `Win+R` → `certmgr.msc` → “受信任的根证书颁发机构” → “证书”。
3. 右键 → “所有任务” → “导入”，选择 `<workspace>\.whcode\certs\server-cert.pem`，
   目标存储为“受信任的根证书颁发机构”。
4. 在证书属性“详细信息”→“指纹”中核对与步骤 1 一致（忽略空格与大小写）。
5. 重启浏览器后访问 `https://<addr>:<port>/settings/certs`。

撤销：在同一位置删除该证书即可。删除 `<workspace>\.whcode\certs\` 会在下次启动重建
（新私钥与新指纹）；浏览器的临时例外不影响系统信任库，退出权仍由用户掌握。

### 2.3 改用 external 与关闭 TLS

- **external**：在设置页 `/settings/certs` 上传企业签发的证书链（PEM，叶子在前）与
  **未加密**私钥，或直接写 `server-cert.pem`／`server-key.pem`，然后重启 `serve --web`；
  也可按次 `--tls external`。证书与私钥不匹配、私钥带口令或超限一律拒绝，运行中的配置不变。
- **关闭（off）**：`--tls off`（按次）或设置页把入站档位设为 `off`（需 `confirm = true`，
  成功写 `tls_config_change` 审计），重启后生效。代价是回到 ADR-023 的明文形态：同网段可
  抓包读取 token 与数据，只应在已隔离的本机或可信链路使用。
- **失败与回退**：启用档位但材料不可用时**拒绝启动**（错误列出证书库路径与 `--tls off`），
  不静默降级明文；回退步骤为 `--tls off`，或删除 `.whcode/certs/` 后由下次启动重建。

## 3. 令牌（认证）

局域网模式必须提供令牌文件；令牌**只从文件读**，不经命令行、不落库、不入日志。

```bash
# 生成（Python 标准库，不引入额外工具）
python -c "import secrets; print(secrets.token_urlsafe(32))" > .\token.txt
```

| 项 | 当前行为 | 证据 |
|---|---|---|
| 提供方式 | `--token-file PATH`，读入后 `trim`，空文件拒绝启动 | `serve.rs::serve_web` |
| 保存位置 | 仅进程内存 | 同上 |
| 传输方式 | HTTP 头 `Authorization: Bearer <token>`（HTTPS 下不再明文可读） | `security.rs::check_auth` |
| 禁止的传参 | URL query 携带 token 被显式拒绝 | `security.rs` 注释与 `api.rs` |
| 浏览器侧 | 从 URL **fragment** `#token=` 读取（fragment 不发往服务器），仅存内存 | `crates/whcode-web/frontend/app.js` |
| 比较方式 | 恒定时间比较 | `security.rs::constant_time_eq` |
| 失败响应 | `403` + `permission_denied` 错误信封 | `api.rs::handle_request` |
| 令牌轮换/有效期 | **未实现**（无生成/轮换/过期命令） | 全库检索无命中；长度由用户负责 |
| 文件权限校验 | **未实现**（不检查 ACL/权限位，由部署者保证） | 全库检索无命中 |

**授权语义**：通过认证的 Web 客户端以 `web-local` 身份获得**全部能力**
（`serve.rs::serve_web`、`crates/whcode-app/src/authz.rs`）。因此“能访问 Web”等价于
“能做所有写操作”（含证书设置）；局域网内共享令牌即共享全部权限，必须向使用者讲清。

## 4. 浏览器侧防护现状

| 防护 | 现状 | 证据 |
|---|---|---|
| Origin 校验 | 有：按 `(scheme, host)` 匹配，端口与路径不参与、大小写不敏感 | `security.rs::origin_scheme_host` |
| Origin scheme | 启用 TLS 时为 `https://`（含局域网监听额外允许 `https://localhost`／`https://127.0.0.1`）；`--tls off` 为 `http://` | `serve.rs::origin_allowlist`、ADR-022 2026-10-25 修订 |
| 缺失 Origin | **放行**（同源 GET 与 CLI 客户端不带该头；认证模式由 token 兜底） | `security.rs::check_origin` |
| 回环允许集 | 明文回环默认 `http://127.0.0.1`、`http://localhost`；`serve` 注入非空集时以注入值为准 | `security.rs::ALLOWED_ORIGINS`／`check_origin` |
| CSRF token | **无**（防线是 Bearer 头 + Origin 白名单 + TLS） | 全库检索无命中 |
| Host 头校验 | **无** | `api.rs`、`serve.rs` 均不读取 `host` |
| 安全响应头 | **无** CSP／X-Frame-Options／X-Content-Type-Options／HSTS／Referrer-Policy | 实际只写 `content-type`／`content-length`／`connection`（及页面 `cache-control: no-store`） |
| 缓存控制 | API 与设置页 `no-store`；静态资产无 ETag/Last-Modified，前端文件编译期内嵌 | `serve.rs`、`assets.rs` |
| 点击劫持/嵌入 | 未禁止（无 X-Frame-Options 与 `frame-ancestors`） | 同上 |

结论口径：当前安全模型是“**仅回环/仅可信局域网 + 入站 TLS + Bearer 令牌 + Origin 白名单**”，
不是面向不可信网络的加固 Web 服务。公网暴露必须另立 ADR（会话、限速、头策略、吊销）。

## 5. 证书库、私钥保管与连通性校验

- **落点与边界**：`<workspace>/.whcode/certs/`（`settings.json`、`ca-builtin.pem`、
  `ca-extra.pem`、`ca-builtin.json`、`server-cert.pem`、`server-key.pem`）。与工作区数据库
  同一信任边界（同用户、同 ACL）；**不承诺**同用户进程隔离——同用户进程可直接读写私钥与
  追加根证书。
- **私钥**：`external`／自签私钥均为**未加密** PEM；只进不出——不写日志、事件、审计正文与
  任何 API 响应，下载面只提供证书链与指纹（`cert.rs` 模块文档、`api.rs` 证书路由）。
- **编辑面**：Web 设置页 `/settings/certs` 与 `/api/certs/*`，能力
  `settings.cert.read`／`settings.cert.write`；降档（入站 `off`、清空内置根、上传外部私钥）
  须 `confirm = true`；成功写 `tls_config_change` 审计，**审计失败即回滚文件**。
- **出站档位与吊销**：`system` 档位读取操作系统证书库（Windows ROOT 存储，
  `rustls-native-certs`）；叠加 `ca-extra.pem` 时由 `rustls` 做链与主机名校验，
  **不复用 Windows 的信任策略与吊销检查**（CRL／OCSP 均不参与）。0 份证书是可操作错误，
  **不回落内置库**。
- **连通性校验**：`POST /api/certs/verify` 以当前出站信任做**一次只握手**校验（不发送 HTTP
  请求与任何凭据）；**同步**执行，单次最多阻塞服务循环 **2 秒**（`PROBE_TIMEOUT`），期间
  其他客户端请求排队。**DNS 解析不受这 2 秒约束**（解析发生在预算计时之外），因此主机名
  解析缓慢时实际阻塞可超过 2 秒——这是已知代价，异步化需 ADR-015 并发档案。

## 6. 路由与端点

静态资源为固定白名单（`/`、`/index.html`、`/app.js`、`/suspend.js`、`/styles.css`、
`/settings.html`、`/settings.js`），编译期内嵌，未知路径与路径穿越一律 404
（`crates/whcode-web/src/assets.rs`）。设置页 `/settings/certs` 由装配根服务端直出
（注入只读摘要，无 JS 时可见）。不存在前端路由或 SPA 回退。

只读端点（`GET`）：

| 路径 | 说明 |
|---|---|
| `/api/status` | 服务与库状态 |
| `/api/sessions` | 会话列表，上限 200 |
| `/api/events` | 事件流，上限 1000 |
| `/api/tasks` | 任务 |
| `/api/runtimes` | 三档运行时能力等级 |
| `/api/i18n` | 界面词条 |
| `/api/certs` | 证书库状态（出站档位与份数、入站证书主题/SAN/有效期/指纹、`restart_required`） |
| `/api/certs/ca`、`/api/certs/server` | CA（内置/追加）与服务端证书链（公开材料，不含私钥） |
| `/ws` | RFC 6455 握手成功后注册为订阅者，按工作区 `seq` 增量广播事件（订阅者上限由 `broadcast::MAX_WS_CLIENTS` 约束，超出回 1013） |

写端点（`POST`，JSON 对象体；字段类型严格，字符串冒充布尔被拒）：

| 路径 | 参数 | 破坏性保护 |
|---|---|---|
| `/api/sessions/new` | `title`（非空）、`model`（可选） | —— |
| `/api/sessions/rename` | `session`、`title` | —— |
| `/api/sessions/archive` | `session`、`archived`（布尔）、`confirm`（缺省 false） | **默认只预览**；确认后才归档并写 `session_archive` 审计 |
| `/api/sessions/delete` | `session`、`confirm`（缺省 false） | **默认只预览**；确认后才删 |
| `/api/tasks/retry`／`/api/tasks/abort` | `task` | —— |
| `/api/purge` | `before_ms`、`confirm`（缺省 false） | **默认只预览**；确认后才删 |
| `/api/certs/modes` | `outbound`／`inbound`、`confirm` | 入站 `off` 需 `confirm=true` |
| `/api/certs/ca`、`/api/certs/ca/reset` | `kind`（builtin/extra）、`pem`、`confirm` | 清空内置根需 `confirm=true`（空内置即使确认也拒绝） |
| `/api/certs/server`、`/api/certs/server/self-signed` | `cert`、`key`、`confirm` | 上传外部私钥须 `confirm=true` |
| `/api/certs/verify` | `host`、`port` | 只握手；同步阻塞 ≤ 2 s（见 §5） |

破坏性操作有三层保护：前端两步式按钮 → API 层缺省预览 → 存储层再次要求 `confirm=true`
（`crates/whcode-app/src/maintenance.rs`）。删除与审计同一事务，审计失败整体回滚
（`crates/whcode-store/src/purge.rs`）；归档与 `session_archive` 审计同样同事务（ADR-038）。
预览必须报满受影响的表名。

### 6.1 保留与预留命令的 501 语义（FR-620／ADR-032）

- 登记表（`whcode-app::surface`）中登记为预留（Reserved）或排除（Excluded）的路径被调用时
  返回 **HTTP 501** + JSON 信封 `code=not_supported`、`retryable=false`，`details` 含
  `surface_id`、`state` 与 `milestone`／`reason_code`；**未登记路径仍为 404**。
  守卫位于认证与 Origin 检查**之后**（`crates/whcode-web/src/api.rs`）。
- 证书设置在 CUI／TUI／GUI 端登记为 `Excluded`（理由码 `WEB_SETTINGS_ONLY`／
  `GUI_READ_ONLY_V1`）：被调用返回 `not_supported`（CUI 退出码 55、Web 501 同码信封）。

## 7. 资源与有界性

| 项 | 值 | 证据 |
|---|---|---|
| 请求体上限 | 1 MiB | `security::MAX_MESSAGE_BYTES` |
| 请求头上限 | 16 KiB | `serve.rs::REQUEST_HEADER_CAP` |
| 读/握手空闲上限 | 5 s | `serve.rs::REQUEST_IDLE_TIMEOUT` |
| 写停滞上限 | 5 s | `serve.rs::WRITE_STALL_TIMEOUT` |
| 并发连接 | `MAX_CLIENTS = 96`（含 WS 订阅者）；无连接池、无限速 | `serve.rs` |
| TLS 缓冲 | 每连接一份明文/密文缓冲，均有容量上限（有界，NFR-215） | `crates/whcode-bin/src/tls.rs` |
| 并发模型 | 单线程非阻塞多路复用；`Connection` 不跨任务共享 | `serve.rs` 模块文档、ADR-002 |
| 分块传输 | 不支持 | `api.rs` |
| 资源预算口径 | NFR-201/202 以 `--tls off` 明文档位测量（与 ADR-025 冻结基线和历史序列同夹具）；TLS 档位的 RSS／CPU 增量**未测量** | `.github/scripts/benchmarks/measure-nfr-budgets.py` |

## 8. 日志与脱敏

- 日志出口统一脱敏：敏感键名 `api_key`/`apikey`/`token`/`secret`/`password`/`authorization`，
  值前缀 `Bearer `、`sk-`、`ghp_` 替换为 `[REDACTED]`（`crates/whcode-bin/src/logging.rs`）。
- 追踪载荷默认 `redacted`（`crates/whcode-domain/src/trace.rs`；配置默认 `config.rs`）。
- 启动输出只打印 URL、入站档位、证书指纹与库路径，不打印令牌；证书库错误消息不含私钥
  （`cert.rs` 的“不泄漏秘密”约束）。
- TLS 握手/解密失败只打印对端地址与错误原因（`serve.rs::report_tls_failure`），不含密钥材料。

## 9. 明确不承诺（不得按“已完成”理解）

1. **公网暴露**：仅回环与 RFC1918；公网与通配地址拒绝启动。
2. **吊销与 OS 策略**：不参与 CRL／OCSP，不复用 Windows 信任策略（`system` 档位同样如此）。
3. **mTLS／客户端证书**、**多用户与账号体系**（认证通过即全能力）。
4. **同用户／管理员隔离**：同用户进程可读写证书库（与数据库同一 ACL 边界）。
5. **热切换**：入站档位与证书替换需重启；出站 CA 变更即时生效。
6. **Host 头校验、安全响应头、CSRF token、限速**：均无。
7. **真实浏览器证据未取得**：自签信任步骤、设置页浏览器操作与重启提示的实机取证
   尚未完成（按 ADR-037，以 CI 日志与 artifact 为验收；本机未运行，登记为未取得）。

## 10. 已知缺口、失效条件与复审触发

1. **ADR-023 与实现不一致**：ADR 写“无/有 token → 401/200”，实现返回 **403**
   （`api.rs`）。以实现为准，ADR 待修订。
2. **Host 头校验缺失**：允许集按 Origin 判定，`Host` 不参与；若将来引入反向代理需重新评估。
3. **令牌无轮换、无过期、无多用户区分**；文件权限不校验。
4. **同一工作区单进程独占**：Web 服务运行时不要并行启动 TUI（`workspace.rs` 模块文档）。
5. **内置根证书包存在陈旧风险**：保真刷新、种子清单与 `reset` 可定位差异；复审周期
   见 `assets/certs/SOURCES.md`（每 6 个月，上次 2026-10-25）。

**失效条件（以下任一成立即修订本文与 ADR-040，并补迁移与回退证据）**：

- 需要公网暴露或多来源白名单配置（ADR-023 退出条件成立）；
- 需要操作系统吊销（CRL／OCSP）或平台信任策略；
- 需要 mTLS 或客户端证书认证；
- 需要证书／档位热切换或异步 TLS（须先补 ADR-015 并发设计档案）；
- 内置证书包更新周期无法维持，或资产版本落后超过复审期；
- `rustls`／`ring`／`rustls-native-certs`／`rcgen` 出现需处置的安全公告；
- Web 端编辑不再满足运维（需要 headless 自动化轮换）。

## 11. 排障

| 现象 | 原因与处置 |
|---|---|
| 启动即报 `invalid_state` | `--bind` 地址非法、公网/通配地址、非私网地址未带 `--token-file`，或启用档位的证书材料不可用（按错误给出的路径检查，或 `--tls off`） |
| 浏览器提示证书不受信任 | 自签证书的正常代价：核对设置页/启动输出的 SHA-256 指纹后加例外，或改用 `--tls external` |
| `https://` 页面 403 | 令牌缺失/不匹配，或 Origin 的 scheme/host 不在允许集（`--tls off` 时允许集为 `http://`） |
| 页面能开但列表为空 | 库为空或 `whcode.duckdb` 位于另一个工作区；确认启动输出打印的库路径 |
| 局域网访问被拒 | 目标地址不是 RFC1918 私网；本项目不承诺公网场景 |
| 设置页提示“未初始化” | `serve --web` 启用档位时首次启动会创建证书库；若初始化失败，错误会列出目录与 `--tls off` |
| `/api/certs/verify` 超时 | 对端不可达、握手失败，或 DNS 解析缓慢（解析不计入 2 s 预算） |
| 关闭网页后仍占用库 | 服务进程仍在运行；`serve` 不随浏览器关闭退出 |
