# 域 N：安全与凭据

> ZCode 证据路径以 `local/zcode-src`（commit 3c5b227）为根，以下省略前缀，实际绝对路径为
> `D:\WHCode\local\zcode-src\...`。

## 1. 关键路径勘误

- **鉴权 token 实为 `ZCODE_SERVER_AUTH_TOKEN`**：`ZCODE_SERVER_TOKEN` 仅驱动“”
  标志，实际校验用的 token 值是 `ZCODE_SERVER_AUTH_TOKEN`（`packages/server/src/http.ts`）。
- **Legacy server 用普通字符串比较，非恒定时间**：仅 R3（`packages/zcode-server-cli/src/server-core/http.ts`）
  使用 `timingSafeEqual`；Legacy `packages/server/src/http.ts` 用 `===` 比较，存在时序侧信道。
- ZCode 服务器**无 Origin/scheme+host 校验**（CSRF 防护缺失），这是 WHCode 可借鉴的**增强点**。

## 2. 对照表

| 项 | ZCode | WHCode | 判定 |
|---|---|---|---|
| 鉴权 | `ZCODE_SERVER_AUTH_TOKEN`（Bearer token，`packages/server/src/http.ts`）；R3 用 `timingSafeEqual`；Legacy 用 `===` | `ssh`/`web` 默认回环监听；凭据不入库不入日志 | WHCode 默认安全更强（回环+零落库）；ZCode 有 token 鉴权 |
| 时序安全 | R3 `timingSafeEqual`；Legacy 普通比较 | 凭据本地文件，无网络鉴权路径 | **修正**：ZCode Legacy 非恒定时间（时序侧信道） |
| CSRF/Origin | **无 Origin/scheme+host 校验**（`http.ts` 只验 Bearer token） | Web 默认回环，无跨域面 | **ZCode 缺失**：WHCode 若开 Web 服务应加 Origin 校验 |
| 凭据存储 | `enc:v1:` 前缀、aes-256-gcm、IV=12B、authTag=16B、`sha256(secret)` 派生；密钥 `ZCODE_CREDENTIAL_SECRET` 或 fallback | 仅 `local/sensenova.env`，不入库不入日志 | WHCode 靠不落库更简单；ZCode 靠落库加密可迁移 |
| 脱敏 | 两套收口（runner-debug-redaction + telemetryRedaction 文本归一/模型白名单） | 四级追踪（off/metadata/redacted/full）+ IntegrityFlags | 相当（见域 K） |
| 最小权限 | 服务器进程 fork 后降权（server-cli `--supervisor`） | 默认回环、无对外监听 | 相当 |

## 3. 与 WHCode 差异修正（相对原差异书）

- **修正**：鉴权 token 是 `ZCODE_SERVER_AUTH_TOKEN`，`ZCODE_SERVER_TOKEN` 仅驱动标志位。
- **修正**：Legacy server 普通字符串比较（非恒定时间），仅 R3 用 `timingSafeEqual`。
- **补充**：ZCode 服务器缺 Origin/scheme+host 校验——WHCode 若引入 Web 服务端应以此为
  反例补 CSRF/Origin 校验（安全反教条：不以“”结项）。

## 4. 存疑项

- R3 与 Legacy 服务器的鉴权差异是否有意为之、还是版本漂移，需读提交历史确认 `[待验证]`。
- `ZCODE_SERVER_AUTH_TOKEN` 的默认值（若未设置）是否回退到明文/空，需读配置默认值确认
  `[待验证]`。
