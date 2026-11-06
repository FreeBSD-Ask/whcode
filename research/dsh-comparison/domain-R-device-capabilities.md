# 域 R：设备与多媒体能力

> DSH 证据根：`local/dsh-src`（commit `fcc0ad4`）；WHCode 证据根：本仓库。

## 1. 对照表

| 项 | DeepSeek Harness (DSH) | WHCode | 判定 |
|---|---|---|---|
| 浏览器操作 | `browser-use` 共享服务＋provider（Playwright MCP／Chrome DevTools MCP／Stagehand）；launched browser 归确切 agent/session，跨 turn 复用；fork 后浏览器状态重置（`docs/subsystems/browser-use.md`） | 无（检索 `playwright`、`browser_use` 于 `crates/` 零命中） | **DSH 独有** |
| 桌面操作 | `computer-use` 共享服务＋Cua Driver provider；`ctx.cuaAccess` 管系统权限（辅助功能/录屏）与应用级同意，经 User Questions 征得（`docs/subsystems/computer-use.md`） | 无（GUI 只读 v1，FR-711—715） | **DSH 独有** |
| 附件 | 内容寻址附件：`sha256:<digest>` 不可解析；**先持久化再发布引用**，事件与模型块只带引用＋元数据，绝不带 object URL/临时路径/base64（`docs/subsystems/attachment.md`） | 无图片/附件管线（`crates/` 中 `image` 仅命中 MCP 协议与进程模块） | **结构性缺失（WHCode）** |
| Office 预览 | `office-to-pdf`：LibreOffice 转换、准入上限、PDF 复用；无模型工具、无会话事件（`docs/subsystems/office-to-pdf.md`） | 无 | **DSH 独有** |
| 语音输入 | 实验性：STT 服务定义＋本地 SenseVoice provider＋浏览器 Remote 消费；音频**不回退到别的 provider**，本地 provider 串行并限队列（`docs/subsystems/voice-input.md`） | 无 | **DSH 独有** |
| 原生代码 | `native/system`：Landlock launcher、POSIX flock 绑定，按架构构建/发布，npm optional deps 只装匹配平台（`native/README.md`） | 仅 `whcode-platform` 允许平台分支（cfg 集中）；无原生 addon | **语义不同不可比** |
| 与沙箱的关系 | 浏览器/桌面 provider 都走审批与沙箱；computer-use 有独立系统权限面 | 不适用 | **DSH 领先** |

## 2. DSH 实现解剖

### 2.1 浏览器归会话，不归进程

launched browser 属于**确切**的 live Agent 与 Session，跨 turn 复用；Session runtime 释放时关闭
其启动的资源；重载或 fork 会得到全新浏览器状态，登录态**不从会话日志恢复**
（`browser-use.md`）。attached browser 仍属外部，provider 在自身实例内为单个会话预留，
拒绝第二个会话同时附着，拆除只断开不关闭（`browser-use.md`）。共享服务只登记名字、
拒绝第二个 provider，**没有**统一的浏览操作方法或模型可控 selector。

### 2.2 桌面权限是人给的

CUA provider 拥有 `ctx.cuaAccess`：系统权限（辅助功能、录屏）由人选，应用级同意经 User
Questions；`CuaApplicationState` 记录已授权/已拒绝应用与持久化是否可用（`computer-use.md`）。
即“模型不能自己给自己授权”。

### 2.3 附件先落盘再引用

生产者把字节交给 `ctx.attachments`，**只有对象持久后**才发布不可变内容寻址引用；会话事件与
模型可见块只带引用与元数据（`attachment.md`）。用户消息被接受时，图片先移到
`<DSH_HOME>/attachments/v1` 再追加 user 事件（`attachment.md`）。这条顺序消除了“日志里指向
一个已消失临时文件”的悬空引用。

### 2.4 语音不回退

`resolve()` 固定一个 provider 实例；provider 缺失或不支持的语言直接失败，**音频不回退到
另一个 provider**（`docs/subsystems/voice-input.md`）。本地 provider 串行、限队列、独立 worker 生命周期
（§Browser and Host ownership）——与 WHCode 的资源纪律同向。

## 3. WHCode 现状核对

- 已实现：GUI 原生自绘（无浏览器引擎，ADR-026）、主题与暂停绘制（ADR-027、gui-suspend）、
  平台差异集中（`crates/whcode-platform/src/process.rs` 等）。
- 已实现：MCP 协议层可承载 image content（`crates/whcode-mcp/src/protocol.rs`），但**无附件
  存储与模型图像输入管线**（`crates/` 检索 `attachment`、`image_url`、`media_type` 无实现）。
- 缺失：浏览器操作、桌面操作、附件、Office 预览、语音、原生 addon。检索 `playwright`、
  `cua`、`attachment`、`speech` 于 `crates/` 零命中。
- 差异：WHCode 刻意把交互面收在“文本＋终端＋GUI 只读”；DSH 把设备能力做成一等公民。

## 4. 差异判定与可借鉴点

1. **附件先持久化再引用**：若 WHCode 接图像输入，必须照此顺序：先落盘（内容寻址）→ 再写
   `user/message` 事件带引用；禁止 base64 进事件。这与“追踪 redacted、图片不进日志”的现行
   口径一致。
2. **引用类型不可解析**：可采纳。`AttachmentId` 禁止消费者从 `sha256:` 推导路径；WHCode 若做
   附件，应同样只提供 `resolve_path()` 而非暴露存储布局。
3. **浏览器/桌面 provider 只登记名字**：不采纳（当前）。两者都会引入巨大的平台与权限面；
   WHCode 的 GUI 只读 v1 已明确不做。
4. **语音不回退 provider**：可采纳为通用原则（“显式失败优于静默换后端”），适用于 MCP、
   LLM 路由与将来的 STT。
5. **原生 addon（Landlock/flock）**：暂不采纳。WHCode 的沙箱策略是“不做进程沙箱”，
   Landlock launcher 只在 Linux 有意义；Windows 目标平台无对应收益。
6. **Office→PDF**：不采纳。需要 LibreOffice 依赖与临时文件管理，与“单文件、本地优先、
   资源有界”的定位不符。

## 5. 待验证项

1. DSH 浏览器 provider 是否默认启用：`browser-use.md` 说“experimental，需显式激活”。取证：
   读 `packages/bundle/base/cordis.patch.yml` 是否有 browser-use 行。
2. DSH 附件是否有大小/数量上限：`attachment.md` 未给数值。取证：读
   `packages/attachment/attachment/README.md` 的配置节。
3. WHCode 是否计划图像输入（测试模型文档支持图像，但产品面未接）：取证：
   `rg -n "图像|image" specs/001-whcode-harness/spec.md | head`。

## 6. 证据路径清单

- `docs/subsystems/browser-use.md`（provider 选择）
- `docs/subsystems/browser-use.md`（会话所有权）
- `docs/subsystems/browser-use.md`（attached 语义）
- `docs/subsystems/computer-use.md`（cuaAccess 权限面）
- `docs/subsystems/attachment.md`（先持久化再引用）
- `docs/subsystems/office-to-pdf.md`（转换服务）
- `docs/subsystems/voice-input.md`（不回退 provider）
- `native/README.md`（原生 workspace）
- `crates/whcode-platform/src/process.rs`（WHCode 平台层）
- `crates/whcode-mcp/src/protocol.rs`（image content 承载）
