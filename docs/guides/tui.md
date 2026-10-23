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
| 输入流 EOF | —— | 自动退出并恢复终端（`crates/whcode-tui/src/lib.rs:151`） |

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
| `j` | 会话选择下移（到尾自动钳制） | `crates/whcode-tui/src/lib.rs:329-332` |
| `k` | 会话选择上移（到头自动钳制） | `crates/whcode-tui/src/lib.rs:333-336` |
| `r` | 刷新当前视图快照 | `crates/whcode-tui/src/lib.rs:337` |
| 终端 resize | 更新行列数并重绘，不退出 | `crates/whcode-tui/src/state.rs:41-44` |

- 选中行以 `> ` 前缀呈现，不依赖颜色（墨水屏友好；`crates/whcode-tui/src/views.rs:222-233`）。
- 标题含 `{index}/{count}` 视图序号（`crates/whcode-tui/src/lib.rs:459-466`）。
- 键位提示为页脚词条 `tui.footer`（“窗口 {cols}×{rows} · 按键 {keys} · 1—6 视图 · j/k 会话 · r 刷新 · q 退出”，`crates/whcode-domain/src/i18n/zh-CN.properties:21`）。

## 3. 未实现（显式声明，勿按存在使用）

以下能力**当前不存在**，属于已登记的边界而非缺陷隐瞒：

- 无命令面板、无 `:` 命令模式、无插入/普通模式之分（全量检索无命中）；
- 无翻页键（无 PgUp/PgDn）：内容按终端行数限行投影（`crates/whcode-tui/src/lib.rs:414-419`）；
- 无 `?`/F1 帮助屏；
- 无运行时主题切换键：主题仅启动时由 `ui.theme` 配置或 `WHCODE_UI_THEME` 环境变量决定
  （`crates/whcode-bin/src/config.rs:20-24`、`122-126`）；
- 无鼠标、焦点、bracketed paste 处理（`crates/whcode-tui/src/event.rs:8`、`48`）；
- `Ctrl+V` 等组合键不保留修饰语义：除 `Ctrl-C` 外，Ctrl 组合等价于裸字母（`event.rs:37-45`）；
- 方向键、Esc、Enter、F1 等被丢弃（`crates/whcode-tui/src/event.rs:42-45`）；
- CJK 等多字节字符按单 `char` 接收，仅计入按键数，不触发动作（`event.rs:43`、`state.rs:39`）。

## 4. 数据源现状（重要）

TUI 六视图当前由装配根注入**空数据源**（`crates/whcode-tui/src/lib.rs:16-18`、`86-94`）：
界面可完整操作，但列表显示“暂无数据”。数据源不可用时打印提示并回退空源
（`crates/whcode-bin/src/main.rs:286-291`）。diff 视图有独立的未接入提示词条
（`crates/whcode-domain/src/i18n/zh-CN.properties:24`）。
TUI 是只读快照前端：破坏性命令不经 TUI（`ViewSource` 只读，`crates/whcode-tui/src/lib.rs:81-84`）。

## 5. 主题与终端

| 项 | 值 | 证据 |
|---|---|---|
| 主题默认 | `eink-day` | `crates/whcode-tui/src/theme.rs:40-42` |
| 合法取值 | `amoled-night`／`eink-day`／`eink-night` | `crates/whcode-tui/src/theme.rs:60-66` |
| 非法主题名 | 报 `parse_error` 并列出全部合法值 | `crates/whcode-tui/src/theme.rs:77-86` |
| 色值 | 三套精确值（`#000000`/`#E6E6E6` 等） | `crates/whcode-tui/src/theme.rs:20-32` |
| 尺寸探测失败兜底 | 80×24 | `crates/whcode-tui/src/lib.rs:50`、`129` |
| 最小尺寸校验 | **无**（不做“太小即拒绝”） | `crates/whcode-platform/src/term.rs:12-14` |
| 空闲探测 | 输入静默 1 s 起探测，间隔 5 s，退避上限 60 s | `crates/whcode-tui/src/lib.rs:56-64` |

## 6. 已知约束

- 同一工作区同一时刻只允许一个进程打开库：TUI 与 `serve --web` 不应并行
  （`crates/whcode-bin/src/workspace.rs:9-12`）。
- 颜色为 24 位 RGB 精确值；终端不支持真彩时由终端自行近似（无降级映射表，属已知限制）。
