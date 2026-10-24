# Web 前端：部署与安全配置（FR-603、FR-606、FR-609）

> 适用：`whcode serve --web`。事实基准：`crates/whcode-web`、`crates/whcode-bin/src/serve.rs`、
> `docs/adr/ADR-022`、`docs/adr/ADR-023`（2026-10-23 对照）。威胁模型与失效条件以 ADR-023 为准，
> 本文只描述**当前实际行为**，不把未实现项写成能力。

## 1. 启动

```bash
whcode serve --web                                  # 默认 127.0.0.1:8080，仅回环
whcode serve --web --bind 127.0.0.1:8787            # 指定端口
whcode serve --web --bind 192.168.1.10:8787 --token-file .\token.txt   # 局域网访问（必须带令牌）
```

| 项 | 当前行为 | 证据 |
|---|---|---|
| 默认监听 | `127.0.0.1:8080` | `crates/whcode-web/src/bind.rs:14`、`crates/whcode-bin/src/serve.rs:182-184` |
| 配置来源 | **只有**命令行 `--bind`；配置文件无 web/bind 字段 | `serve.rs:44`、`62`；`config.rs:52-66` |
| 地址语法 | 仅 IPv4 字面量 + 端口，其余报 `invalid_state` | `bind.rs:40-51` |
| 对外监听 | 仅允许 RFC1918 私网地址，且**必须**同时给 `--token-file` | `bind.rs:68-77`、`101-116`；`serve.rs:205-210` |
| 公网与通配 | 公网 IP 与 `0.0.0.0` 一律**拒绝启动** | `bind.rs:90-100` |
| 启动提示 | 回环/局域网两种口径分别打印到 stdout，并打印库路径 | `serve.rs:226-233` |
| 明文 HTTP | 无 TLS | `docs/adr/ADR-023-lan-only-web.md:33-34` |

## 2. 令牌（认证）

局域网模式必须提供令牌文件；令牌**只从文件读**，不经命令行、不落库、不入日志。

```bash
# 生成（Python 标准库，不引入额外工具）
python -c "import secrets; print(secrets.token_urlsafe(32))" > .\token.txt
```

| 项 | 当前行为 | 证据 |
|---|---|---|
| 提供方式 | `--token-file PATH`，读入后 `trim`，空文件拒绝启动 | `serve.rs:186-204` |
| 保存位置 | 仅进程内存 | `serve.rs:234-238` |
| 传输方式 | HTTP 头 `Authorization: Bearer <token>` | `crates/whcode-web/src/security.rs:63-67` |
| 禁止的传参 | URL query 携带 token 被显式拒绝 | `security.rs:157-167` |
| 浏览器侧 | 从 URL **fragment** `#token=` 读取（fragment 不发往服务器），仅存内存 | `crates/whcode-web/frontend/app.js:74-92` |
| 比较方式 | 恒定时间比较 | `security.rs:27-37`、`68` |
| 失败响应 | `403` + `permission_denied` 错误信封 | `crates/whcode-web/src/api.rs:186-204` |
| 令牌轮换/有效期 | **未实现**（无生成/轮换/过期命令） | 全库检索无命中；ADR-023 载明长度由用户负责 |
| 文件权限校验 | **未实现**（不检查 ACL/权限位，由部署者保证） | 全库检索无命中 |

**授权语义**：通过认证的 Web 客户端以 `web-local` 身份获得**全部能力**
（`serve.rs:180`、`crates/whcode-app/src/authz.rs:124-139`）。因此“能访问 Web”等价于
“能做所有写操作”。局域网内共享令牌即共享全部权限，这一点必须向使用者讲清。

## 3. 浏览器侧防护现状

| 防护 | 现状 | 证据 |
|---|---|---|
| Origin 校验 | 有：按 `(scheme, host)` 匹配，端口与路径不参与、大小写不敏感、支持 IPv6 字面量 | `security.rs:90-137` |
| 缺失 Origin | **放行**（非浏览器客户端与部分同源请求不带该头） | `security.rs:100-103`；设计理由 `78-81` |
| 回环允许集 | `http://127.0.0.1`、`http://localhost` | `security.rs:20` |
| 局域网允许集 | 由装配根按监听地址计算，每请求执行 | `serve.rs:221-225`；`api.rs:289-293` |
| CSRF token | **无**（防线是 Bearer 头 + Origin 白名单） | 全库检索无命中 |
| Host 头校验 | **无** | `api.rs`、`serve.rs` 均不读取 `host` |
| 安全响应头 | **无** CSP／X-Frame-Options／X-Content-Type-Options／HSTS／Referrer-Policy／Cache-Control | 全库检索无命中；实际只写 `content-type`／`content-length`／`connection` |
| 缓存控制 | 无 ETag/Last-Modified/指纹；前端文件为编译期内嵌 | `assets.rs:14-20`；`serve.rs:353-367` |
| 点击劫持/嵌入 | 未禁止（无 X-Frame-Options 与 `frame-ancestors`） | 同上 |

结论口径：当前安全模型是“**仅回环/仅可信局域网 + Bearer 令牌 + Origin 白名单**”，
不是面向不可信网络的加固 Web 服务；需要公网暴露时必须另立 ADR（TLS、会话、限速、头策略）。

## 4. 路由与端点

静态资源为固定白名单（`/`、`/index.html`、`/app.js`、`/suspend.js`、`/styles.css`），
编译期内嵌，未知路径与路径穿越一律 404（`crates/whcode-web/src/assets.rs:26-35`、`59-67`）。
不存在前端路由或 SPA 回退。

只读端点（`GET`）：

| 路径 | 说明 |
|---|---|
| `/api/status` | 服务与库状态（`api.rs:310`） |
| `/api/sessions` | 会话列表，上限 200（`api.rs:321-322`） |
| `/api/events` | 事件流，上限 1000（`api.rs:340-347`） |
| `/api/tasks` | 任务（`api.rs:355`） |
| `/api/runtimes` | 三档运行时能力等级（`api.rs:379`） |
| `/api/i18n` | 界面词条（`api.rs:393`） |
| `/ws` | **仅握手**，随后发送正常关闭帧；前端用轮询补齐（`serve.rs:407-413`、`app.js:2-11`） |

写端点（`POST`，JSON 对象体；字段类型严格，字符串冒充布尔被拒）：

| 路径 | 参数 | 破坏性保护 |
|---|---|---|
| `/api/sessions/new` | `title`（非空）、`model`（可选） | —— |
| `/api/sessions/rename` | `session`、`title` | —— |
| `/api/sessions/archive` | `session`、`archived`（布尔） | —— |
| `/api/sessions/delete` | `session`、`confirm`（缺省 false） | **默认只预览**；确认后才删 |
| `/api/tasks/retry`／`/api/tasks/abort` | `task` | —— |
| `/api/purge` | `before_ms`、`confirm`（缺省 false） | **默认只预览**；确认后才删 |

破坏性操作有三层保护：前端两步式按钮（预览成功才启用确认，`app.js:328-345`、`384-402`）→
API 层缺省预览（`api.rs:535-557`、`636-648`）→ 存储层再次要求 `confirm=true`
（`crates/whcode-app/src/maintenance.rs:38-49`）。删除与审计同一事务，审计失败整体回滚
（`crates/whcode-store/src/purge.rs:16-18`）。预览必须报满受影响的表名（`purge.rs:177-197`）。

### 4.1 保留与预留命令的 501 语义（2026-10-24 交付，FR-620／ADR-032）

- **当前行为**：登记表（`whcode-app::surface`）中登记为预留（Reserved）或排除（Excluded）
  的路径被调用时返回 **HTTP 501** + JSON 错误信封 `code=not_supported`、`retryable=false`，
  `details` 含 `surface_id`、`state`（reserved／excluded）与 `milestone` 或 `reason_code`；
  **未登记路径仍为 404**（末尾兜底）。守卫位于认证与 Origin 检查**之后**，不放宽安全默认值
  （`crates/whcode-web/src/api.rs:215-249`、`337-353`）。
- **不回显内部细节**：501 响应只含登记元数据（里程碑／理由码），message 不携带内部路径
  或凭据；认证与 Origin 检查仍在先——未认证请求维持 403，501 仅对通过认证的请求可达。
- **不伪装**：不得把已登记能力伪装成 404 或静默成功；已实现路径继续走正常路由。

## 5. 资源与有界性

| 项 | 值 | 证据 |
|---|---|---|
| 请求体上限 | 1 MiB | `security.rs:17`；`api.rs:170`、`994-999` |
| 请求头上限 | 16 KiB | `serve.rs:287` |
| 读超时 | 5 s | `serve.rs:273` |
| 写超时/空闲 keep-alive | **无**（每响应 `connection: close`） | `serve.rs:402` |
| 并发模型 | 单线程顺序处理，`Connection` 不跨任务共享 | `serve.rs:6-8`、`259-283` |
| 分块传输 | 不支持 | `api.rs:46` |
| 连接数/限速 | **无**（无连接池、无速率限制） | 全库检索无命中 |

## 6. 日志与脱敏

- 日志出口统一脱敏：敏感键名 `api_key`/`apikey`/`token`/`secret`/`password`/`authorization`，
  值前缀 `Bearer `、`sk-`、`ghp_` 替换为 `[REDACTED]`（`crates/whcode-bin/src/logging.rs:8-20`、`52`）。
- 追踪载荷默认 `redacted`（`crates/whcode-domain/src/trace.rs:19-20`；配置默认
  `config.rs:44-49`），可疑字段 10 项与值前缀见 `crates/whcode-llm/src/trace.rs:29-43`。
- 启动输出只打印 URL 与库路径，不打印令牌（`serve.rs:226-233`）。

## 7. 已知缺口与不一致（不得按“已完成”理解）

1. **ADR-023 与实现不一致**：ADR 文中写“无/有 token → 401/200”，实现返回 **403**
   （`api.rs:188`）。以实现为准，ADR 待修订。
2. 无 TLS、无速率限制、无公网加固；`0.0.0.0` 与公网地址被拒绝启动。
3. 无 Host 头校验、无安全响应头、无 CSRF token、无缓存策略。
4. WebSocket 仅握手，实时性依赖轮询（正常 1 s，失败指数退避 500→8000 ms）。
5. 令牌无轮换、无过期、无多用户区分；认证通过即全能力。
6. 同一工作区单进程独占：Web 服务运行时不要并行启动 TUI（`workspace.rs:9-12`）。

## 8. 排障

| 现象 | 原因与处置 |
|---|---|
| 启动即报 `invalid_state` | `--bind` 地址非法、公网/通配地址、或非私网地址未带 `--token-file` |
| 浏览器 403 | 令牌缺失/不匹配，或 Origin 不在允许集（含端口不参与的判定口径） |
| 页面能开但列表为空 | 库为空或 `whcode.duckdb` 位于另一个工作区；确认启动输出打印的库路径 |
| 局域网访问被拒 | 目标地址不是 RFC1918 私网；本项目不承诺公网场景 |
| 关闭网页后仍占用库 | 服务进程仍在运行；`serve` 不随浏览器关闭退出 |
