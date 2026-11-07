# 域 M：前端：Web、桌面、CLI

> DSH 证据根：`local/dsh-src`（commit `fcc0ad4`）；WHCode 证据根：本仓库。

## 1. 对照表

| 项 | DeepSeek Harness (DSH) | WHCode | 判定 |
|---|---|---|---|
| 形态 | 浏览器 Web UI（`dsh web`，默认 `127.0.0.1:3080`）、Electron 桌面、CLI、SDK/ACP 无头（`README.md` Run 节；`docs/architecture.md` 应用启动节） | TUI、CUI、Web、GUI、SSH 五适配器共享应用服务（`crates/whcode-{tui,cui,web,gui,ssh}`） | **同构**：多前端共享 Host |
| 前端技术 | React＋浏览器 Cordis 插件图：Client Modules/Slots/Conversation 四层（`docs/subsystems/web-client.md`） | 无框架、服务端直出＋原生 ESM；GUI 原生 iced＋tiny-skia（ADR-014/026/027） | **取向相反**：DSH 重前端工程，WHCode 极简＋原生 |
| 桌面壳 | Electron 承载同一 Web 应用，Desktop Host 走 Node IPC（`docs/architecture.md` Desktop 节） | **禁止浏览器套壳**；iced 自绘窗口（ADR-026 铁律） | **WHCode 领先**（就其自身目标而言） |
| 资源预算 | 未见公开的前端预算；Electron 多进程本身较重（`docs/architecture.md`:桌面包、私有 Host、渲染进程） | 首屏 JS ≤ 50 KB gzip、CSS ≤ 20 KB gzip、DOM ≤ 2000 节点（`.github/scripts/benchmarks/check-frontend-budget.py:27`—`29`），且由脚本强制 | **WHCode 领先**：预算可判定 |
| GUI 空闲预算 | 无对应公开数值 | 1200×800 空闲 RSS ≤ 96 MB、线程 ≤ 24、句柄 ≤ 256、CPU ≤ 0.2% 单核（ADR-027 §4；实测 41.1 MB/24 线程/CPU 0.0%，`ADR-027` 修订段） | **WHCode 独有** |
| 会话视图 | Chat／Trajectory 双视图，目标快照独立、按需激活（`conversation.md`） | GUI 七视图（`ViewId`），Web 会话页＋tail API（T250/T251） | **DSH 领先**：视图插件化 |
| 重连语义 | 物理 WS 恢复与逻辑流重开分离；每个模型定义替换/续传语义（`web-client.md`） | WS 断线重连＋事件游标（FR-617）；GUI 挂起恢复去抖 ≥250 ms | **同构**，DSH 语义分层更细 |
| 主题 | 未见公开的精确色值表（web-styling 文档讲风格） | 六套主题精确色值，`theme_tokens` 单一事实源，默认 `zcode-night`（AGENTS.md §6；ADR-057/FR-739） | **WHCode 领先** |
| 可访问性/i18n | 文档 i18n 完整（`.zh.md` 成对），UI i18n 未见主轴 | 简中/英文必须同时可用；RTL 逻辑属性要求（AGENTS.md §6） | **取向不同** |

## 2. DSH 实现解剖

### 2.1 浏览器端也是 Cordis 应用

Web Client 由独立加载的插件组成：Client Modules 装载插件图、API Gateway 提供类型化 Host 通信、
Slots 组合 React UI、Conversation 把会话历史窗口变成目标自有视图（`web-client.md`）。
依赖方向固定为 Host 状态 → Remote 传输 → Client 模型 → UI 适配 → Conversation → Slots → React；
表现组件**拿不到** `ctx`、传输对象或别的功能插件实现（`web-client.md`）。

### 2.2 Client 模型不是第二事实源

每个 API controller 有配对的 Host/Client 面：Host 拥有权威变更与流生产，Client 只维护
“最近可用投影”，并编码延迟响应与替换基线的合并规则（`web-client.md`）。
会话侧分三层：`ClientSessions → SessionManager → Session`，每个 `Session` 只持有一个连续
逻辑事件窗口（`web-client.md`）。

### 2.3 重连分两层

物理恢复由 Gateway mux 负责；逻辑恢复由每个 `RemoteStream` 在 Connection 发布可用代际时
自行重开。持久会话日志校验逻辑序号区间并用每个代际的开口快照**整体替换**窗口；
控制流与工作区流在断线期间保留最后值、重连后原子替换；普通通知**不重放**
（`web-client.md`）。没有全能的 `resync()`。

### 2.4 会话视图按目标激活

每个会话维护单调的活跃目标集；创建/读取源不激活目标，只有 shell 显式激活或首次订阅才建
builder（`conversation.md`）。Chat 与 Trajectory 可以识别同一事件族，但各自持有
Definition State 与最终节点载荷，互不 import。

## 3. WHCode 现状核对

- 已实现：五适配器（`crates/whcode-tui`、`whcode-cui`、`whcode-web`、`whcode-gui`、`whcode-ssh`），
  前端不触库（`tests/contract/test_layering.rs:135`），四端对等 `SurfaceEntry` 登记。
- 已实现：前端预算脚本（JS/CSS/DOM 三项，含 DOM 注入 API 禁用检查，`check-frontend-budget.py:41`—`42`）。
- 已实现：GUI 原生自绘（ADR-026/027），空闲资源实测优于预算（ADR-027 修订段）。
- 已实现：六套主题精确色值＋`theme_tokens` 单一事实源（ADR-057/FR-739）。
- 已实现：Web 会话页与 tail API（T250/T251），GUI 对话历史（T250）。
- 缺失：前端视图插件化（GUI 七视图为编译期枚举）、双视图（Chat/Trajectory）、重连逻辑的
  分层语义（有游标但无“代际替换”概念）。
- 差异：WHCode 的 Web 是服务端直出＋少量 JS（预算 50 KB），DSH 的 Web 是完整 SPA 插件图
  （`packages/client` 2039 个 TS 文件）。两者在“能力/成本”上是对立取舍。

## 4. 差异判定与可借鉴点

1. **重连分层（物理/逻辑）**：可采纳为概念。WHCode 的 WS 重连只有“游标补齐”，没有“代际
   整体替换”；对 GUI/Web 的会话窗口，代际替换能避免半截状态。代价：需要给流加代际号。
2. **Client 模型不得成为第二事实源**：应写成前端纪律。WHCode 的 GUI 状态机（`whcode-gui/src/lib.rs`）
   已经在做投影，但需明确“投影不得产生权威状态”。
3. **视图按目标激活**：不采纳（当前）。GUI 七视图是编译期枚举，改成运行期注册与静态分层冲突；
   若将来需要第三视图，用枚举＋feature 即可。
4. **React/SPA**：不采纳。与 ADR-014 的预算与“无打包器”铁律直接冲突；DSH 的 2039 个 TS 文件
   本身就是 WHCode 明确要避免的成本。
5. **Electron**：禁止（ADR-026），无讨论空间。
6. **主题精确色值**：保持并对外说明。DSH 没有公开色值表，这是 WHCode 可验证的优势。

## 5. 待验证项

1. DSH Web/Desktop 是否有公开的资源预算（RSS/线程）：未在 `docs/` 主轴找到。取证：
   `rg -n "RSS|memory budget|thread budget" docs/ packages/client/`。
2. DSH 桌面是否强制单一 Web 技术栈（Electron 版本与签名流程）：`docs/architecture.md` 只述结构。
   取证：读 `apps/desktop/README.md` 的打包与签名节。
3. WHCode Web 的 WS 是否有“代际/快照替换”机制：只读了 FR-617 与 tail API。取证：
   读 `crates/whcode-bin/src/serve.rs` 的 WS 帧与 `crates/whcode-web/src/**`。

## 6. 证据路径清单

- `README.md`（Run 节：`npx @deepseek-ai/dsh web`）
- `docs/subsystems/web-client.md`（四层基础）
- `docs/subsystems/web-client.md`（依赖方向）
- `docs/subsystems/web-client.md`（会话三层模型）
- `docs/subsystems/web-client.md`（重连分层）
- `docs/subsystems/conversation.md`（目标激活）
- `docs/architecture.md`（Desktop 节）
- `.github/scripts/benchmarks/check-frontend-budget.py:27`（JS/CSS/DOM 预算）
- `docs/adr/ADR-027-native-gui-framework.md`（GUI 资源预算）
- `docs/adr/ADR-026-native-gui-only.md`（原生 GUI 铁律）
- `tests/contract/test_layering.rs:135`（前端不触库）
