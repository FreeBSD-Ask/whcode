# ADR-022：Web Origin 校验语义修正（全串比较误伤真实浏览器）

- **状态**：已批准（2026-10-22，`serve --web` 装配后浏览器实测暴露）
- **来源**：T090—T095（Web API 与安全默认值）在真实浏览器下不可用；属“库层通过、
  端到端从未运行”的信号链缺口
- **责任人**：Web 适配层负责人
- **退出条件**：若未来引入多来源白名单配置（配置文件驱动的 Origin 集合），本判定
  扩展为配置驱动并复审

## 背景与复现

`security::check_origin` 以**全串精确比较**匹配 `ALLOWED_ORIGINS =
["http://127.0.0.1", "http://localhost"]`（**不含端口**）。真实浏览器行为与两个独立缺陷：

1. 浏览器 `fetch()` 的 `Origin` 携带**实际端口**（如 `http://127.0.0.1:8737`）——
   全串比较必然不命中 → 403；
2. **同源 GET 请求不发送 `Origin` 头**（`/api/sessions`、`/api/events`、`/api/status`
   全部是 GET）——"缺少 Origin 头" → 403。

合并效果：**任何真实浏览器都无法访问 API**。此前 Web 服务入口（`serve --web`）未装配，
缺陷停留在库层未被端到端发现。复现（修订前）：`check_origin(false, &[],
Some("http://127.0.0.1:8737"))` 与 `check_origin(false, &[], None)` 均返回
`permission_denied`。

## 决策

命中判定改为 **scheme + host**（端口不参与；scheme/host 大小写不敏感；IPv6 字面量
去括号去端口）；缺失 `Origin` 的语义按监听模式区分：

| 模式 | Origin 命中 | 缺失 Origin |
|---|---|---|
| 回环（认证关闭，V-110 默认） | 非回环 host 一律拒绝 | **放行**（同源 GET 与本机 CLI 客户端不带 Origin） |
| 对外（认证开启） | 显式允许集（同样按 scheme + host） | 拒绝（对外部署来源必须显式） |

## 威胁模型与代价（正负和解）

- **防护目标不变**：拦截**非回环来源网页**对回环服务的跨站请求（DNS rebinding、
  恶意页面 `fetch`）——跨站浏览器请求必带 `Origin: http://恶意站`，host 不在允许集
  即拒绝；相似前缀（`127.0.0.1.evil.example`）与任意端口同样拒绝；
- **放开的成本**：回环模式下本机客户端（curl 等）无需伪造 `Origin` 头。本机进程
  本就可直接读取数据库文件，不构成新增攻击面；
- **不再对 Origin 用恒定时间比较**：Origin 是公开信息而非秘密；恒定时间比较仅保留
  给 token（`check_auth`）。

## 影响与验证

- `crates/whcode-web/src/security.rs`：`check_origin` 重写 + `origin_scheme_host`；
- 契约测试 `test_web_security_defaults::origin_checks_apply` 按新模式锁定（端口变体、
  相似前缀、缺失 Origin 两模式）；`security.rs` 单测覆盖大小写与 IPv6 字面量；
- 浏览器端到端证据：`artifacts/web-browser-2026-10-22.txt`（真实浏览器 API 200、
  标签点击、轮询游标推进）。

- **2026-10-22（认证模式缺失 Origin，ADR-023 复审）**：局域网认证实测发现"认证模式
  要求 Origin"与真实浏览器冲突（同源 GET 不发送 Origin → 一律 403）。修订为：**缺失
  Origin 两种模式均放行**——跨站由 Bearer token 阻断（自定义头触发预检，本服务不返回
  CORS 许可）；带 Origin 时仍必须命中允许集。原规则的初衷（对外监听来源显式）由
  ADR-023 的地址边界 + 认证承担。证据：`artifacts/web-lan-2026-10-22-batch4.txt`。

- **2026-10-25（TLS，ADR-040）**：`serve --web` 默认启用 TLS 后，Origin 允许集的 scheme
  由 `http` 改为 `https`（回环模式下的“非回环 host 一律拒绝”判定与认证模式的允许集
  同此口径）；**host／端口匹配语义不变**——仍按 scheme + host 比较、端口不参与匹配、
  大小写不敏感、IPv6 字面量去括号去端口；缺失 Origin 的两种模式语义同样不变。
  明文档位（`--tls off` 或入站档位 `off`）下允许集仍为 `http`。**判定依据**：同一浏览器
  在 HTTPS 页面发起的同源与跨站请求，其 `Origin` 头一律携带 `https` scheme，
  若允许集仍写 `http` 则全部 403（与本 ADR 2026-10-22 记录的全串比较缺陷同类）。

- **实施说明（2026-10-25 修订补充，T190 已落地）**：`crates/whcode-bin/src/serve.rs` 的
  `origin_allowlist(addr, tls_enabled)` 按协议构造允许集（TLS → `https`，`--tls off` → `http`），
  `crates/whcode-web/src/security.rs` 的 `check_origin` 在调用方传入**非空**允许集时始终以其为准
  （与认证是否开启无关），只有空集才回落到明文 `ALLOWED_ORIGINS`——修订前回环模式忽略注入的
  `https` 允许集，HTTPS 页面的同源请求与 WebSocket 升级会被一律 403。
  **证据**：`security.rs` 单测（https 放行／明文拒绝／跨站拒绝，两种认证模式）与
  `test_web_security_defaults::origin_checks_apply`、`api.rs` 的证书 API 用例；
  真实浏览器 HTTPS 端到端证据**尚未取得**（登记缺口；复现：`serve --web` 默认档位 +
  浏览器访问 `/api/certs`，以 GitHub Actions 日志为验收口径，ADR-037）。
