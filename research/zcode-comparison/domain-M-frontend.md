# 域 M：前端与 UI

> ZCode 证据路径以 `local/zcode-src`（commit 3c5b227）为根，以下省略前缀，实际绝对路径为
> `D:\WHCode\local\zcode-src\...`。

## 1. 关键路径勘误

- **CLI TUI 是缺省位置参数，非命名子命令**：`apps/zcode-cli/packages/tui` 提供 TUI 入口，
  但 `zcode` 命令缺省即启动 TUI（无 `tui` 子命令），TUI 由 `packages/tui` 承载。
- 桌面四进程确认：`packages/desktop/src/{main,preload,renderer,host}` 分别承载主进程、
  preload 桥、渲染进程与 host 服务；预览用 CDP + Playwright。

## 2. 对照表

| 项 | ZCode | WHCode | 判定 |
|---|---|---|---|
| 前端形态 | 桌面 Electron 四进程 + Web 浏览器前端 + CLI TUI，三形态并存 | TUI/CUI/Web/SSH 四适配器，GUI 规划中（ADR-027 iced + tiny-skia） | 形态相当；WHCode 无 Electron（ADR-026 铁律） |
| 桌面 | Electron main/preload/renderer/host 四进程；渲染进程用 web 前端代码 | 无（规划 iced 原生，禁止浏览器套壳） | **WHCode 先手**：原生 GUI 更符合资源铁律 |
| 预览 | CDP + Playwright 驱动浏览器预览（开发态） | 无预览机制 | ZCode 有开发态预览先手 |
| Web 扫码 | Web 前端支持扫码登录（`packages/web/src` 实现） | 无扫码登录（Web 仅独立浏览器前端） | ZCode 有协作先手 |
| OAuth | 桌面内嵌 OAuth 授权流程 | 无 | ZCode 先手 |
| 分享 | 会话分享（`packages/shared/src/conversation-share.ts`） | 无 | ZCode 先手 |
| TUI 侧栏 | TUI 侧栏卡片（会话列表/工具状态） | TUI 侧栏（会话/日志） | 相当 |
| 主题 | 多主题；深色/浅色/AMOLED | 四套精确色值（amoled/eink-day/eink-night/terminal-green） | WHCode 主题更精确、墨水屏优化 |

## 3. 与 WHCode 差异修正（相对原差异书）

- **修正**：CLI TUI 是缺省位置参数（`zcode` 直接进 TUI），非命名子命令。
- **修正**：桌面四进程结构确认（main/preload/renderer/host），渲染进程复用 web 前端代码。
- **补充**：ZCode 具备预览（CDP+Playwright）、扫码登录、OAuth、会话分享等**协作先手**，
  这些在 WHCode 的 Web 前端尚属空白；但 WHCode 的原生 GUI（ADR-026/027）在资源铁律上
  优于 Electron 套壳。

## 4. 存疑项

- 桌面预览是否生产态保留、还是仅开发态，需读 `packages/desktop/src/main` 的 dev/prod 分支确认
  `[待验证]`。
- TUI 侧栏卡片的具体信息架构与 WHCode 对照需交互实测 `[待验证]`。
