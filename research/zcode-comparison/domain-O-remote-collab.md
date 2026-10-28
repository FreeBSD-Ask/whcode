# 域 O：远程协作与共享

> ZCode 证据路径以 `local/zcode-src`（commit 3c5b227）为根，以下省略前缀，实际绝对路径为
> `D:\WHCode\local\zcode-src\...`。

## 1. 关键路径勘误

- **真实路径在根 `packages/server/` + `packages/zcode-server-cli/`**，非 `apps/zcode-cli/packages/server/`。
  - `packages/server/src/remote/create-backend.ts`（远程后端创建）；
  - `packages/zcode-server-cli/`（server 核心 + daemon：`src/server-cli.js serve --supervisor` fork `server-core.js`）。
- `daemon` 模式：`server-cli.js serve --supervisor` 先 fork `server-core.js` 子进程，子进程断连时
  父进程自退；支持 OS 服务注册（systemd/launchd/Windows 服务）。
- `packages/shared/src/conversation-share.ts` 承载会话分享。

## 2. 对照表

| 项 | ZCode | WHCode | 判定 |
|---|---|---|---|
| 远程 | 服务器（R3 核心 + Legacy）+ 客户端连接 | SSH 客户端原型（M7 范围），无远程服务器 | WHCode SSH 原型；ZCode 有完整 C/S |
| 会话分享 | `conversation-share.ts`：integrity 用 **canonical JSON SHA-256** 校验分享内容；导出/导入会话 | 无会话分享 | **ZCode 先手**：完整性校验+分享闭环 |
| Relay | **Relay V2**：JWT 认证 + 60s 短时 token；Relay 中继（跨 NAT 打洞） | 无 Relay | **ZCode 先手** |
| 守护进程 | `--supervisor` fork 子进程，子进程断连自退；OS 服务注册（systemd/launchd/Windows） | 无 daemon（SSH 按需启动） | ZCode 运维先手 |
| 发布 | **分阶段发布**（immutable releases，不可变发布物） | 无发布服务器（GitHub Release 资产） | ZCode 先手：可分阶段灰度 |
| 鉴权 | `ZCODE_SERVER_AUTH_TOKEN`（见域 N） | SSH 密钥/口令 | 相当 |

## 3. 与 WHCode 差异修正（相对原差异书）

- **修正**：远程协作真实路径是根 `packages/server/` + `packages/zcode-server-cli/`，
  非 apps 下的路径。
- **补充**：ZCode 具备会话分享（canonical JSON SHA-256 integrity）、Relay V2（JWT+60s token）、
  daemon 守护（fork+断连自退+OS 服务注册）与分阶段发布（immutable releases）四大协作/运维先手，
  均属 WHCode M7+ 可借鉴候选。

## 4. 存疑项

- Relay V2 的具体打洞协议（TURN/STUN/自研）未在本次直读中展开，需另立研究 `[待验证]`。
- OS 服务注册的 Windows 侧实现（服务安装脚本 vs 内嵌）需读 `server-cli.js` 平台分支确认
  `[待验证]`。
