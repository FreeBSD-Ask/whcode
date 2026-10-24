# TUI 使用与键位（FR-601）

> 适用：`whcode`（无参数且存在交互控制台时进入）。事实基准：`crates/whcode-tui/src`
> （2026-10-23 对照）；改动按键解析时必须同步本文（Check 119 口径）。
> 交互冒烟协议另见 [tui-manual-smoke.md](./tui-manual-smoke.md)。

## 1. 启动与退出

| 动作 | 方式 | 说明 |
|---|---|---|
| 启动 | `whcode`（无位置参数、仅旗标或空参数） | 有位置参数时走 CUI 命令面（`crates/whcode-bin/src/main.rs:105-112`） |
| 退出 | `q` 或 `Ctrl-C` | 两者语义一致，均请求退出（`crates/whcode-tui/src/state.rs:36`） |
| 退出保证 | —— | 正常退出、`Ctrl-C` 与 panic 均恢复 raw mode 与主屏（`crates/whcode-tui/src/session.rs:66-80`、`143-159`、`174-179`） |
| 输入流 EOF | —— | 自动退出并恢复终端（`crates/whcode-tui/src/lib.rs:160`） |

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

- 选中行以 `> ` 前缀呈现，不依赖颜色（墨水屏友好；`crates/whcode-tui/src/views.rs:222-233`）。
- 标题含 `{index}/{count}` 视图序号（`crates/whcode-tui/src/lib.rs:467-475`）。
- 键位提示为页脚词条 `tui.footer`（“窗口 {cols}×{rows} · 按键 {keys} · 1—6 视图 · j/k 会话 · r 刷新 · q 退出”，`crates/whcode-domain/src/i18n/zh-CN.properties:21`）。

## 3. 未实现（显式声明，勿按存在使用）

以下能力**当前不存在**，属于已登记的边界而非缺陷隐瞒：

- 无命令面板、无 `:` 命令模式、无插入/普通模式之分（全量检索无命中）；
- 无翻页键（无 PgUp/PgDn）：内容按终端行数限行投影（`crates/whcode-tui/src/lib.rs:421-428`）；
- 无 `?`/F1 帮助屏；
- 无运行时主题切换键：主题仅启动时由 `ui.theme` 配置或 `WHCODE_UI_THEME` 环境变量决定
  （`crates/whcode-bin/src/config.rs:20-24`、`122-126`）；
- 无鼠标、焦点、bracketed paste 处理（`crates/whcode-tui/src/event.rs:8`、`48`）；
- `Ctrl+V` 等组合键不保留修饰语义：除 `Ctrl-C` 外，Ctrl 组合等价于裸字母（`event.rs:37-45`）；
- 方向键、Esc、Enter、F1 等被丢弃（`crates/whcode-tui/src/event.rs:42-45`）；
- CJK 等多字节字符按单 `char` 接收，仅计入按键数，不触发动作（`event.rs:43`、`state.rs:39`）。
- 本变更（ensure-frontend-parity）交付后，`t` 键四主题循环、写操作与游标刷新将改变上述
  部分现状，见 §8；交付前以本节为准。

## 4. 数据源现状（重要）

TUI 六视图的数据由装配根经 `ViewSource` 注入：`whcode-bin` 打开工作区库后优先注入
`StoreViewSource`（真实数据，`crates/whcode-bin/src/main.rs:284-291`）；**仅当库打开失败**
时回退 `EmptyViewSource` 并打印提示（`crates/whcode-tui/src/lib.rs:16-19`、`95-99`），
此时列表显示“暂无数据”。diff 视图另有独立的未接入提示词条
（`crates/whcode-domain/src/i18n/zh-CN.properties:24`）。
TUI 是只读快照前端：破坏性命令不经 TUI（`ViewSource` 只读，`crates/whcode-tui/src/lib.rs:83-92`）。

## 5. 主题与终端

| 项 | 值 | 证据 |
|---|---|---|
| 主题默认 | `eink-day` | `crates/whcode-tui/src/theme.rs:40-42` |
| 合法取值 | `amoled-night`／`eink-day`／`eink-night` | `crates/whcode-tui/src/theme.rs:60-66` |
| 非法主题名 | 报 `parse_error` 并列出全部合法值 | `crates/whcode-tui/src/theme.rs:77-86` |
| 色值 | 三套精确值（`#000000`/`#E6E6E6` 等） | `crates/whcode-tui/src/theme.rs:20-32` |
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

## 8. ensure-frontend-parity 变更预告（本节能力交付后生效）

以下为已立项变更（FR-619—625、ADR-032）的目标行为，**交付前不得按已实现使用**；
交付时由实现任务回填本文并更新 §3／§4 的现状口径。

- **写操作与确认**（FR-623）：会话 new／rename／archive／delete、任务 retry／abort、
  清理 purge 经 `whcode-app::commands` 门面执行（前端不触库、不自行判权）；破坏性命令
  先预览后确认，取消确认不改变数据库状态、不写执行审计；无执行器时动作显式提示
  `not_supported`，不静默无响应。具体键位由实现任务定案并回填本文（不占用既有
  `q`／`1`—`6`／`j`／`k`／`r`）。
- **`t` 四主题循环**（FR-625）：`t` 键运行时循环 `amoled-night`→`eink-day`→
  `eink-night`→`terminal-green` 四步回原位，色值与 FR-612／FR-713 精确一致，
  不要求重启；默认仍取 `ui.theme`／`WHCODE_UI_THEME`。交付前 §3“无运行时主题切换键”
  仍为现状。
- **游标刷新**（FR-624）：`ViewSource` 增加游标查询；TUI 空闲时低频检查游标
  （间隔 ≥1000 ms、常量命名并测试断言；查询 O(1)），仅游标推进时置脏并重绘一帧，
  游标未变零重绘；不新增常驻线程、不做周期性全量重绘（FR-613／FR-617 纪律不变）；
  其他端产生的更改自动可见；空闲 CPU 复测不回归 NFR-205 基线
  （`local/measure_resources.py`）。
