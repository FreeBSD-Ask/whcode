# TUI 使用与键位（FR-601）

> 适用：`whcode --tui`（Windows 上无参数默认启动 GUI；其他平台无参数仍进 TUI）。
> 存在交互控制台时才会进入。事实基准：`crates/whcode-tui/src`
> （2026-10-24 对照）；改动按键解析时必须同步本文（Check 119 口径）。
> 交互冒烟协议另见 [tui-manual-smoke.md](./tui-manual-smoke.md)。

## 1. 启动与退出

| 动作 | 方式 | 说明 |
|---|---|---|
| 启动 | `whcode --tui`（无位置参数） | Windows 无参数默认 GUI；非 Windows 无参数进 TUI。有位置参数时走 CUI 命令面（`crates/whcode-bin/src/main.rs`） |
| 退出 | `q` 或 `Ctrl-C` | 两者语义一致，均请求退出（`crates/whcode-tui/src/state.rs:36`） |
| 退出保证 | —— | 正常退出、`Ctrl-C` 与 panic 均恢复 raw mode 与主屏（`crates/whcode-tui/src/session.rs:66-80`、`143-159`、`174-179`） |
| 输入流 EOF | —— | 自动退出并恢复终端（`crates/whcode-tui/src/lib.rs:160`） |

### 1.1 运行环境（按键无反应时先查这里）

TUI 直接读 Windows 控制台输入，键盘事件能否送达取决于运行环境：

- **可以**：Windows Terminal、conhost（cmd／PowerShell 窗口）。`whcode` 双击启动的
  控制台窗口同样可用，窗口需要处于输入焦点。
- **不可以**：Git Bash／mintty 等伪终端。那里没有 Windows 控制台输入句柄，
  raw mode 与按键读取都不成立；请改用 `cmd`／PowerShell 启动，或使用 CUI 子命令。
- **控制台处于选择模式**：在窗口里拖选或点选会进入 QuickEdit（标记）模式，控制台暂停
  输入，按键全部无效。按 `Esc` 或回车退出选择模式即可恢复。
- 启动条件同时检查输出侧尺寸（`terminal_size()`）与输入侧是否终端
  （`stdin_is_interactive()`）；两者任一不成立就不进入 TUI，改为打印骨架提示后正常退出。

2026-10-25 用真实控制台复现：新控制台启动 release 构建，注入 `q` 后进程正常退出，
事件循环收到 `Key('q')`——按键路径本身无缺陷；无效报告均来自上述环境差异。

## 2. 视图与导航

六视图固定顺序（`crates/whcode-tui/src/views.rs:15`、`36-43`），按数字键切换
（`crates/whcode-tui/src/views.rs:99-109`）：

| 键 | 视图 | 稳定 ID |
|---|---|---|
| `1` | 会话 | `sessions` |
| `2` | 任务 | `tasks` |
| `3` | 上下文 | `context` |
| `4` | 事件 | `events` |
| `5` | Spec 状态 | `spec` |
| `6` | diff 预览 | `diff` |

| 键 | 动作 | 证据 |
|---|---|---|
| `j` | 会话选择下移（到尾自动钳制） | `crates/whcode-tui/src/lib.rs:338-341` |
| `k` | 会话选择上移（到头自动钳制） | `crates/whcode-tui/src/lib.rs:342-345` |
| `r` | 刷新当前视图快照 | `crates/whcode-tui/src/lib.rs:346` |
| 终端 resize | 更新行列数并重绘，不退出 | `crates/whcode-tui/src/state.rs:41-44` |

**写操作与主题（2026-10-24 交付，FR-623／625）**：全部写命令经 `whcode-app::commands`
门面执行（TUI 不触库、不自行判权）；破坏性命令先执行预览段并显示“将删 N 个会话（事件 M）”，
再按 `y` 确认，其他键取消（取消不改变数据库、不写执行审计）；未注入执行器时显式提示
`not_supported`。

| 键 | 视图 | 动作 |
|---|---|---|
| `n` | 会话 | 新建会话（单行输入标题，Enter 提交，Esc 取消，上限 200 字符） |
| `m` | 会话 | 重命名选中会话（同上输入方式） |
| `A` | 会话 | 归档选中会话（预览 → `y` 确认；ADR-038） |
| `d` | 会话 | 删除选中会话（预览 → `y` 确认） |
| `R` | 任务 | 重试选中任务 |
| `X` | 任务 | 中止选中任务 |
| `p` | 任意 | 清理 30 天前会话（预览 → `y` 确认） |
| `t` | 任意 | 四主题循环（`eink-day`→`eink-night`→`amoled-night`→`terminal-green`→回 `eink-day`，不重启） |

- 选中行以 `> ` 前缀 + 反显呈现，不依赖颜色（墨水屏友好；`crates/whcode-app/src/view_source.rs`
  的 `render_columns_limited`，2026-10-25 起视图模型已下沉到应用层）。
- 键位提示为底栏词条 `tui.footer`（`1-6 视图  j/k 移动  r 刷新  t 主题  q 退出  |  {cols}x{rows}`，
  `crates/whcode-domain/src/i18n/zh-CN.properties`）；写操作键位在状态行按上下文提示。

### 2.1 三区布局（ADR-041，2026-10-25）

```text
顶栏：应用名  |  视图 (n/6)  |  当前会话
导航栏（14 列）  主面板（单线边框 + 列标题 + 列对齐正文 + 右侧滚动位置）
（写操作反馈行，仅非空时占一行）
键位提示栏
```

- **导航栏**：六视图固定顺序，当前项以 `>` 与反显双重标记；占 14 列，右侧留 1 列空白与主面板
  左边框分隔。
- **主面板**：会话／任务／事件视图按列定义渲染（会话：标记/ID/标题/阶段/状态；任务：标记/ID/
  标题/状态/重试；事件：序号/类型/摘要），摘要行位于列标题之上；Spec／上下文／diff 仍为文本行。
- **宽度纪律**：CJK 与全角字符按 2 列计算，超宽以 ASCII `~` 截断；极窄（<24 列）或极矮
  （<6 行）时退化为无 chrome 的纯正文，空态仍显式提示。
- **装饰字符**：只使用实测为 1 格的制表符与 ASCII；新文案不引入 `·`、`×`、`—`、`“”` 等
  歧义宽度字形（避免触发 `RowBackend` 的运行时字形实测）。
- 其他端产生的更改由低频游标检查感知：间隔 ≥1000 ms（`CURSOR_CHECK_INTERVAL`），查询为
  O(1) 单值，仅游标推进时置脏重绘一次，不新增常驻线程（FR-624）。

## 3. 未实现（显式声明，勿按存在使用）

以下能力**当前不存在**，属于已登记的边界而非缺陷隐瞒：

- 无命令面板、无 `:` 命令模式、无插入/普通模式之分（全量检索无命中）；
- 无翻页键（无 PgUp/PgDn）：内容按终端行数限行投影（`crates/whcode-tui/src/lib.rs:421-428`）；
- 无 `?`/F1 帮助屏；
- 无鼠标、焦点、bracketed paste 处理（`crates/whcode-tui/src/event.rs:8`、`48`）；
- `Ctrl+V` 等组合键不保留修饰语义：除 `Ctrl-C` 外，Ctrl 组合等价于裸字母（`event.rs:37-45`）；
- 方向键与 F1 等仍未映射（`crates/whcode-tui/src/event.rs:48-54`）；Enter／Esc／Backspace
  已用于单行输入与确认（同上）；
- CJK 等多字节字符按单 `char` 接收：普通浏览态只计入按键、不触发动作；单行输入态进入
  输入缓冲（受 200 字符上限约束）（`event.rs:43`、`crates/whcode-tui/src/lib.rs:508-510`）。

## 4. 数据源现状（重要）

TUI 六视图的数据由装配根经 `ViewSource` 注入：`whcode-bin` 打开工作区库后优先注入
`StoreViewSource`（真实数据，`crates/whcode-bin/src/main.rs:284-291`）；**仅当库打开失败**
时回退 `EmptyViewSource` 并打印提示（`crates/whcode-tui/src/lib.rs:16-19`、`95-99`），
此时列表显示“暂无数据”。diff 视图另有独立的未接入提示词条
（`crates/whcode-domain/src/i18n/zh-CN.properties:24`）。
视图数据仍是只读快照：`ViewSource` 只读，TUI 不直接打开数据库；写操作经
`whcode-app::commands` 门面（装配根注入执行器）执行，权限与审计由应用层统一判定
（`crates/whcode-tui/src/lib.rs:83-92`、`513-527`）。

## 5. 主题与终端

| 项 | 值 | 证据 |
|---|---|---|
| 主题默认 | `eink-day` | `crates/whcode-tui/src/theme.rs:44-46` |
| 合法取值 | `amoled-night`／`eink-day`／`eink-night`／`terminal-green` | `crates/whcode-tui/src/theme.rs:78-84` |
| 运行时切换 | `t` 键按 `eink-day`→`eink-night`→`amoled-night`→`terminal-green` 循环，四步回原位 | `crates/whcode-tui/src/theme.rs:54-63` |
| 非法主题名 | 报 `parse_error` 并列出全部合法值 | `crates/whcode-tui/src/theme.rs:77-86` |
| 色值 | 四套精确值（`#000000`/`#E6E6E6`/`#FFFFFF`/`#00FF00`） | `crates/whcode-tui/src/theme.rs:20-36`、`65-74` |
| 尺寸探测失败兜底 | 80×24 | `crates/whcode-tui/src/lib.rs:58-59`、`138` |
| 最小尺寸校验 | **无**（不做“太小即拒绝”） | `crates/whcode-platform/src/term.rs:12-14` |
| 空闲探测 | 输入静默 1 s 起探测，间隔 5 s，退避上限 60 s | `crates/whcode-tui/src/lib.rs:64-73` |

## 6. 已知约束

- 同一工作区同一时刻只允许一个进程打开库：TUI 与 `serve --web` 不应并行
  （`crates/whcode-bin/src/workspace.rs:9-12`）。
- 颜色为 24 位 RGB 精确值；终端不支持真彩时由终端自行近似（无降级映射表，属已知限制）。

## 7. 渲染后端与歧义宽度字符（2026-10-23）

TUI 不使用 ratatui 的 `CrosstermBackend`，改用 `crates/whcode-tui/src/rowbackend.rs` 的
`RowBackend`：**每行只发一次绝对定位，行内连续输出**。它来自一次实测缺陷（V-003 复测）：

- **现象**：页脚“按键 0”显示成“键 0”、“视图”显示成“图”，标题“会话”显示成“话”。
  修复前帧见 `artifacts/tui-v003-2026-10-23/before-fix/s1-01-startup.txt`，像素见
  `before-fix/window.png`。
- **根因**：`CrosstermBackend` 逐格 `MoveTo` 定位、列号按 `unicode-width`（歧义字符记 1 格）；
  本机控制台（CP 65001、中文区域）把 `·`、`×`、`—`、`“”`、`’`、`°`、`±`、`§`、`●`、
  `→`、`…` 渲染成 **2 格**，于是写下一格时覆盖了该字的尾格，conhost 把整字擦除。
- **实测**（`artifacts/tui-v003-2026-10-23/width-probe.txt`，逐字光标前进量）：制表符
  （`─│┌┼━`）、块元素（`█▏`）、`✓`、盲文均为 **1 格**——与 Unicode East Asian Width
  不一致（它们在 Unicode 里同属 Ambiguous），因此**任何静态宽度表都不可靠**。
  `RowBackend` 改为运行时实测：在真实终端（`live`）下，首次出现的**非 ASCII** 字形写到
  屏幕末行同一行连续列位、读光标列差得到前进格数，结果按字形缓存（上限 1024 个，超出即
  停止实测、回落 ratatui 口径）；ASCII 恒按 1 格，测试与嵌入方（`with_size`）不做实测。
  校准会擦掉末行，因此实现把该行强制并入本帧脏集合重画。

代价与残留（正负和解）：

- 脏行整行重写（一帧写入上界 = 行数 × 列数），不再享受行内局部更新的带宽优势；
- 行在终端里的实际宽度大于影子屏时做**尾部压缩**：保留最右格并锚定到最后一列，被挤掉的
  正文列不显示，但边框与整行宽度保持一致（不折行、不污染下一行）；
- 首次出现的字形有一次性实测成本（写 1 个字形 + 读 1 次光标），之后走缓存；
- 异常强杀（S3）不保证恢复终端内容，残留帧属预期，raw mode 由 `TerminalGuard` 以外的
  路径无法回收——实测残留为 false 仅指本机 conhost 行为，不是跨终端承诺。

V-003 证据（2026-10-23）：`artifacts/tui-v003-2026-10-23/summary.json`（10 项自动检查全过）、
`s1-01-startup.txt` 至 `s3-after-kill.txt` 逐帧文本、`window.png` 像素截图、
`resize-probe.txt`（ConPTY 下 `SetConsoleWindowInfo` 恒返回 87，改用窗口像素尺寸触发真实 resize）。

## 8. 四端对等交付（2026-10-24，T177／T178；FR-619—625、ADR-032）

本节记录已**交付并可用**的能力；键位见 §2 的写操作与主题表，未实现项以 §3 为准。

- **写操作与确认**（FR-623）：会话 new／rename／archive／delete、任务 retry／abort、
  清理 purge 经 `whcode-app::commands` 门面执行（前端不触库、不自行判权）；破坏性命令
  （`A`／`d`／`p`）先执行预览段并显示将变更的对象（删除会话数与事件数、归档当前→目标状态），
  再按 `y` 确认；取消确认不改变数据库状态、不写执行审计；无执行器时动作显式提示
  `not_supported`，不静默无响应。归档确认后由 store 在同一事务写 `session_archive` 审计
  （ADR-038）；TUI 目前没有反归档键，还原需经 CUI `--unarchive` 或 Web。
- **`t` 四主题循环**（FR-625）：`t` 键运行时按 `eink-day`→`eink-night`→`amoled-night`→
  `terminal-green` 循环，四步回原位；色值与 FR-612／FR-713 精确一致，不要求重启；
  默认仍取 `ui.theme`／`WHCODE_UI_THEME`。
- **游标刷新**（FR-624）：`ViewSource` 提供游标查询；TUI 空闲时低频检查游标
  （`CURSOR_CHECK_INTERVAL` = 1000 ms、测试断言；查询 O(1) 单值），仅游标推进时置脏并
  重绘一帧，游标未变零重绘；不新增常驻线程、不做周期性全量重绘（FR-613／FR-617 纪律不变）；
  其他端产生的更改自动可见。资源复测用 `.github/scripts/benchmarks/measure-resources.py`，
  结果写入 `artifacts/frontend-parity-2026-10-24/close-gates.md`。
