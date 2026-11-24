# ADR-102：ZCode UI 视觉与布局 1:1 复刻（保留原生渲染栈）

- **状态**：已批准（2026-11-23，用户指令“1:1 复刻 ZCODE UI”“保留原生 GUI”“双端同批”）
- **关联**：ADR-026（原生 GUI 铁律）；ADR-027（GUI 选型与预算）；ADR-014（Web 无框架）；
  ADR-041（视觉语言分层）；ADR-057（ZCode 体验对齐，本 ADR 是其视觉层的加严修订）；
  `local/zcode-src`（基线 commit `3c5b227`；`DESIGN.md`＋`packages/ui/src/styles.css`）
- **上游依据**：用户 2026-11-23 指令——现有界面“看不懂、不知道怎么操作”，要求 1:1 复刻
  ZCode UI；技术栈裁决为“保留原生 GUI”，端序裁决为“双端同批”

## 任务认知卡

- **方向**：把 WHCode 的 GUI 与 Web 视觉层从“极简 2 色平面”升级为 ZCode 的完整设计系统，
  使熟悉 ZCode 的用户零学习成本迁移。
- **目标**：①完整语义 token 体系（ZCode `zai-light`／`zai-dark` 核心集）；②圆角层级、
  字号体系、间距节奏、elevation 四类视觉规格；③工作区布局骨架（侧栏＋对话流＋输入区＋
  Side Pane）；④双端由同一 token 事实源派生。
- **用户意图**：不是加操作提示，而是让界面本身长得和 ZCode 一样。
- **非目标**：不引入 Electron／React／Tailwind／Vite（ADR-026／014 铁律不变）；不复制
  ZCode 的桌面窗口控件（无 Electron 主进程语义）；不复制账户／付费／遥测等非本仓库能力；
  不改既有四套主题精确色值（eink／amoled／terminal-green 一字不动）。
- **现有证据**：ZCode `DESIGN.md` 为设计权威源、`styles.css` 为色值唯一事实源
  （`zai-light` 163 token／`zai-dark` 162 token）；WHCode `theme_tokens.rs` 仅 14 token；
  GUI 仅消费 `(bg, fg)` 两色（14 处 `Color::`）；Web CSS 340 行、无圆角无阴影。
- **手段（后置）**：Rust token 结构扩展 → Web CSS 受校验产物重写 → 布局骨架重构 →
  GUI 语义色消费。

## 问题

三道界面（GUI／Web／TUI）视觉层级缺失：只有背景与前景两色，卡片、边框、输入框、菜单、
浮层全无区分；无圆角、无阴影、无字号层级。结果是用户无法从外观判断“哪里可点、哪里是
输入、哪里是状态”，迁移成本高于 ZCode。

## 决策

1. **视觉 1:1，实现原生**：复刻 ZCode 的 token 值、圆角层级、字号体系、间距节奏、
   elevation 与布局骨架；渲染仍用 iced（GUI）与无框架 HTML/CSS（Web）。禁止引入
   WebView／Electron／React／Tailwind／Vite。
2. **token 单一事实源扩展**：`whcode_domain::theme_tokens` 从 14 字段扩展为 ZCode 核心
   语义集（约 60 项），值取自 `zai-light`／`zai-dark` 块；既有四套主题不重定义。
   Web `styles.css` 与 GUI 渲染同源派生，契约测试逐值断言。
3. **视觉规格常量**：圆角（xl 12px／lg 8px／md 6px／sm 4px／2xl 16px）、字号
   （14px 基准，七级 `ui-xl`…`ui-2xs`）、间距（4px 基准，5 档）、elevation（三档阴影）
   写成 Rust 常量与 CSS 变量，双端共用。
4. **布局骨架**：Web 首页从“顶部 7 tab”改为 ZCode 工作区形态——左侧栏（会话列表）＋
   主区（对话流＋底部输入）＋右侧详情面板；GUI 同步。状态页、设置页保留但纳入同一视觉体系。
5. **渐进与可停**：按 token → CSS → 布局 → GUI 四步交付，每步独立可验证、可回退；
   不一次性重写全部视图。

## 正负和解

| 项 | 内容 |
|---|---|
| 正面 | 迁移成本显著下降；视觉层级让可点/可读/状态一目了然；双端同源减少漂移 |
| 负面 | token 从 14 增至约 60，维护面扩大；CSS 体积上升（预算 20 KB gzip 压力）；GUI 渲染代码量增加；既有“极简无阴影”设计语言被替换 |
| 承担者 | GUI／Web 维护者承担渲染与契约测试；用户承担默认外观变化（主题仍可切、可撤销） |
| 和解理由 | 现有 2 色平面已被用户判定为不可用；ZCode 的 token 集经其产品验证，复刻比自创便宜 |

## 三重时间

- **过去来源**：ADR-041 视觉语言分层、ADR-057 体验对齐批次 R 的 token 提案、
  `research/zcode-comparison/domain-M2-ui-experience.md` §7。
- **现在证据**：ZCode 基线 `3c5b227` 的 `DESIGN.md` 与 `styles.css` 逐值可查；
  WHCode GUI 仅 2 色、Web CSS 340 行无圆角。
- **已知限制与失效条件**：①ZCode 大版本演进（`local/zcode-src` commit 变更）→ 复审 token；
  ②Web CSS 超 20 KB gzip 预算 → 按需裁剪非核心 token；③GUI RSS 超 96 MB → 复审渲染路径；
  ④复制 ZCode 的桌面窗口控件需 Electron 主进程，本仓库不复制，属有意差异。

## 复审触发（绑定命令）

1. `python .github/scripts/check-docs-style.py` 或 `check-frontend-budget.py` 失败 ⇒ 复审视觉层。
2. `cargo test -p whcode-tests --test frontend_parity` 失败 ⇒ 复审 token 与登记表一致性。
3. `local/zcode-src` 的 `git rev-parse HEAD` 变更 ⇒ 复审 token 基线。
4. 本 ADR 一年未被引用，或用户判定视觉仍未达可用 ⇒ 复审方向。

## 影响

- 新增：`theme_tokens` 完整 token 集与视觉规格常量；Web CSS 重写；布局骨架重构。
- 保留：ADR-026／027／014 全部铁律；既有四套主题精确色值。
- 有意差异：不复制 Electron 窗口控件、账户／付费／遥测界面。

## 落地记录（2026-11-23）

- `theme_tokens`：新增 `UiPalette`（32 项）与 `spec` 模块（圆角五档／字号七级／间距五档／
  elevation 三档／侧栏与内容宽度），双主题色板取自 ZCode `zai-light`／`zai-dark` 原值；
  三项新测试断言冻结锚点。
- GUI：侧栏改 ZCode 形制（260px、品牌头、圆角条目、悬停/选中底色、快捷键徽章）；
  新增 `Tab`／`Shift+Tab` 环序切换（输入态与 turn 运行时门控）。
- TUI：新增 `UiEvent::Tab`／`BackTab` 与 `cycle_view`，`1—9/0/-` 直达保留；底栏文案改
  `Tab/1-9`（十三套同步）。
- Web：首页改侧栏工作区（品牌＋纵向视图导航＋主题选择），主区标题随视图更新；
  `styles.css` 重写为完整 token + 圆角 + 字号体系（17.7 KB，gzip 4.3 KB）；
  `/settings/certs`、`/chat` 保持纵向流。
- 验证：`check-frontend-budget` 通过（CSS gzip 4295 B、JS 10809 B、DOM 98）；
  `i18n-po --check` 同步；`fmt` 通过；`coherence pre` 零反例；`traceability` 零断链
  （FR=186／任务=427／Check=499）。
- **未取得**：`cargo check`／`clippy`／`test` 因本机缺 llvm-mingw 链接器阻塞，Rust 侧仅经
  `rustfmt`（语法解析）与 API 逐项核对，未经编译验证；按 ADR-037 以 CI 日志为准。
- 已知差异：WHCode `foreground` 冻结为 `neutral-800／neutral-300`（#262626／#d4d4d4），
  而 ZCode `zai-*` 实际为 `neutral-900／neutral-100`（#171717／#f5f5f5）；此为既有冻结决策，
  本轮不改，登记为复审项。

## 落地记录（2026-11-23 第二批：GUI 原生控件层）

用户追加指令「继续开发，不用 HTML 技术」——即桌面端必须继续走 iced 原生渲染，不得以
WebView／HTML 冒充原生界面（ADR-026 不变）。本批只动 GUI 的原生绘制代码：

- **按钮三档强调**（`Emphasis::Primary／Secondary／Ghost`）：主操作用 `primary` 填充 +
  `primary_foreground` 文字，次操作用 `secondary` 填充 + 1px 描边，弱操作透明、悬停显影。
  接入点：会话操作行、任务行、Spec／运行时／事件控制行、对话发送／取消／模型目录、
  操作确认面板、规则撤销、内容检索、侧栏面板入口。
- **输入框聚焦态**（`field_style`）：默认 `input` 底 + `border` 边框；悬停 `border_hover`；
  聚焦换 `input_focused` 底与 `primary` 边框。接入点：会话搜索、对话 provider、提示词、
  内容检索、操作输入。
- **卡片与面板**（`card_style`／`sidebar_panel_style`）：内容卡与侧栏面板均 `xl` 圆角 +
  1px 边框；正文包进卡片，KPI 与状态栏用次级前景色分层。
- **侧栏选中状态竖条**：选中项左侧 2px 主前景竖条，使「我在哪」不依赖底色深浅。
- **顶栏**：标题 18px + 会话标签 + 右侧常驻「高级面板」入口。
- **空态与禁用理由**：对话空态给「去选择会话」按钮；执行器未接入时按钮行下方显示
  `gui.op.not_supported`（可点与不可点都有可见理由，不静默无响应）。
- **底栏**：改用 `gui.footer`（操作提示 + 主题），不再显示窗口尺寸；十三套同步。
- **窄窗口**：窗口逻辑宽度 < 900 时侧栏收窄到 `SIDEBAR_MIN`（200），宽度让给正文。
- **边界守住**：四套铁律主题（eink／amoled／terminal-green）无扩展色板时仍只用主题两色，
  边框取前景色、不引第三色值、不加阴影（`iron_themes_keep_two_color_discipline` 断言）。
- **未取得**：`cargo check`／`clippy`／`test` 仍因本机缺 llvm-mingw 链接器阻塞；三个新单测
  未经本机运行，iced 0.14 的 `Button::style`／`TextInput::style`／`Container::style` 签名与
  `Border`／`container::Style`／`text_input::Style` 字段已逐项比对 registry 源码。

## 落地记录（2026-11-23 第三批：侧栏与主界面的功能对齐）

用户纠正前两批的方向：「不是把既有 1-9 填充到 ZCode UI，而是文字功能对齐」「主界面对齐文字和功能」。
据此重读 `local/zcode-src` 的侧栏实现，确认 ZCode 的侧栏**不是视图目录而是项目→任务树**
（`WorkspaceSidebar.tsx` 只有三行：`项目树是唯一组织视图`，实体为 `ProjectSidebar`；
文案为 `workspaceSidebar.newConversation＝新建任务`、`conversationsSection＝任务`、
`projectsSection＝项目`、`archivedTasks＝归档任务`、`settings＝设置`）。本批把 WHCode 改成同一组织方式：

- **侧栏承担真实数据**：顶部「新建会话」主操作（`primary` 填充）→「会话」分区（真实会话列表，
  点击进入对话）→ 归档开关 →「设置」分区（面板）。删掉 `1`—`9` 键位徽章，也删掉内部视图目录。
- **主区顶部视图标签行**：原侧栏的视图切换移到主区，两排文字标签（会话／任务／上下文／事件／
  Spec 状态／diff 预览／对话／台账／文件前像／充分性／运行时），选中用 `primary` 填充。
- **打开即对话**：默认视图 `Sessions → Chat`（ZCode 行为）。为此把侧栏目录与主区快照解耦：
  新增 `sidebar_sessions`，`prepare()` 在任意视图下都刷新侧栏目录；`session_ids`（选中下标）
  改以侧栏目录为准——修掉「主区非会话视图时选中下标为空」的真实缺陷。
- **对话区形制**：对话流在上（`Fill`）、composer 固定在下——输入框满宽，发送／取消／模型目录
  在下一行；不再把输入框夹在按钮之间。
- **会话操作按钮两行**：十个动作一行会超出窗口右边界（实测 `Plan mode` 被裁），拆为两行。
- **文案去技术词**：KPI 由「Sessions N · view rows M」改为「N 个会话」（`gui.kpi.count` 十三套同步）；
  侧栏与对话空态提供可点入口（`gui.sidebar.empty`／`gui.chat.pick_session`）。
- **附带修复（实跑发现）**：`prepare_frontend` 只调 `detect_locale(None)`，把 `--lang` 与配置
  `ui.locale` 一并忽略，`whcode gui --lang zh-CN` 不生效；改为 `--lang` → `effective_locale()`
  → `detect_locale` 链。

**验证（本机实跑，非 CI）**：`cargo clippy -p whcode-gui/-p whcode-bin --all-targets -D warnings`
通过；`cargo test -p whcode-gui` 47 passed；`cargo build -p whcode-bin` 成功并以 llvm-mingw 链接；
GUI 实际启动截图 `artifacts/gui-zcode-main-zh-2026-11-23.png`（中文）、
`artifacts/gui-zcode-main-2026-11-23.png`、`artifacts/gui-zcode-sidebar-2026-11-23.png`。

**工具链更正**：llvm-mingw 20261104 一直在工具缓存里（`%LOCALAPPDATA%\Temp\whcode-toolcache`，
路径含 `ykla`），此前把 `ykla` 误写成 `yka` 才判定「缓存失效」。本批未安装任何 GNU 组件，
也未新增下载。

## 落地记录（2026-11-23 第四批：主区只留对话）

用户判定「界面不直观，必须和 ZCode 完全对齐」。复盘发现前一批仍把 11 个内部视图平铺成主区
标签行——那正是「不直观」的来源：ZCode 主区只有对话，其余能力收在侧栏与设置里。本批据此重排：

- **删除主区视图标签行**：`view_tab`／`view_tabs` 移除；主区只剩当前对话。
- **侧栏「视图」分区默认收起**：点「视图 展开/收起」才列出诊断视图（任务／上下文／事件／
  Spec 状态／diff 预览／台账／文件前像／充分性／运行时／会话），选中态带竖条；进入任一诊断视图后，
  主区顶栏出现「返回对话」——不会把人困在诊断页。
- **侧栏新增「项目」分区**：显示当前工作区目录名（`RunOptions.workspace` → `State.workspace_name`；
  `State::new` 签名不变，测试不动）。至此侧栏顺序与 ZCode 同构：新建会话 → 会话列表 → 归档 →
  项目 → 视图（收起）→ 设置。
- **顶栏改成会话标题**：左为当前会话标题（无会话即「开始新对话」）＋阶段·状态，右为「返回对话」
  （仅诊断视图）与「设置」；删除 KPI 条（`kpi()` 一并移除，主区不再有计数行）。
- **空态居中**：改为「开始新对话 ＋ 说明 ＋ 去选择会话」的居中欢迎块。首次实现用 `body.is_empty()`
  判定失效（提示行也进 body），实跑截图发现仍是「左上角一行提示 + 大片空白」，改用
  `chat_history.is_empty() && chat_last.is_none() && chat_streaming.is_empty() && !turn_running`。
- **composer 卡片化**：输入框、模型剖面、模型目录／取消／发送收进同一个圆角卡片；发送在最右且为
  `primary` 填充，与 ZCode 的输入区形态一致。
- **文字**：新增 `gui.chat.new／gui.chat.back／gui.sidebar.expand／gui.sidebar.collapse／
  gui.sidebar.group.project`，`gui.footer` 增加 `{session}` 占位（状态栏显示当前会话），十三套同步。

**验证（本机实跑）**：`cargo clippy -p whcode-gui -p whcode-bin --all-targets -D warnings` 通过；
`cargo test -p whcode-gui` 46 passed（删去 KPI 测试 1 条）；`cargo build -p whcode-bin` 成功；
GUI 实跑截图 `artifacts/gui-zcode-final-zh-2026-11-23.png`。

**术语清理（同日，用户反馈「没有剖面这个术语」）**：
- 中文目录里「剖面」（profile 的自造译法）全部改为「模型」——`gui.chat.provider`、
  `gui.chat.provider.placeholder`、`cli.llm.run.provider／model／max-tokens`、`web.models.profile／empty`、
  `tui.chat.composer.hint`、`tui.chat.status.no_provider`（简中 9 处、繁中 6 处，残留计数均为 0）。
- 实测发现一个真 bug：`gui.chat.provider` 模板原为 `模型：{provider}` 而代码用 `resolve`（非
  `resolve_fmt`），界面直接显示 `模型：{provider}` 字面量；改为无占位的纯标签「模型」，十三套同步。
- 界面上的「ZCode」字样只来自本地验证工作区里手建的测试会话标题（`ZCode UI 对齐` 等），已用
  `session delete` 删除，截图工作区现为空；源码中的「ZCode」仅存在于 `///` 注释与 ADR 溯源，
  不参与渲染。

## 落地记录（2026-11-23 第五批：模型选择/添加 + 诊断视图迁入会话）

用户反馈：「添加模型，选择模型的功能完全没看见，视图那些东西应该在具体会话显示，不要乱塞」。

- **模型改为下拉选择**：`composer` 里原来的自由文本输入框（要用户自己敲模型名）换成
  `pick_list`，选项来自 `GuiLlmExecutor::profiles()`（LLM 剖面注册表）。
- **「添加模型」真正可用**：新增公开写面
  `ModelEditor::add(ModelDraft)`（名称／协议／端点／模型标识／密钥环境变量名）。GUI 不碰文件系统，
  由装配根实现 `StoreModelEditor`：读现有 `profiles.json`（缺失则新建、裸数组自动归一为
  `{"profiles":[]}`）→ 查重名 → 按协议生成 auth（openai/cohere=Bearer、anthropic=x-api-key 头、
  gemini=query key、ollama=none）→ `ProviderRegistry::from_json` 同规则校验 → 写 `.tmp` 再 rename。
- **加载回落**：`ServeLlm::from_env` 在 `WHCODE_LLM_PROFILES` 未设置时回落到
  `<workspace>/profiles.json`——否则「添加」写出的文件永远加载不到。
- **诊断视图迁入会话**：侧栏删除「视图」折叠目录（`ToggleViewMenu` 状态与消息一并移除）；
  改为**选中具体会话后**在主区顶部出现的「会话详情」标签行（事件／diff／Spec／任务／台账／
  前像／充分性／运行时／上下文／对话）。未选会话（空态）时整行不出现。
- **文案**：新增 `gui.chat.model.choose`、`gui.model.*`（9 项）、`gui.chat.add_model` 由
  `gui.model.add` 复用，十三套同步；中文「剖面」已在上一批统一为「模型」。

**验证（本机实跑）**：`cargo clippy -p whcode-gui -p whcode-bin --all-targets -D warnings` 通过；
`cargo test -p whcode-gui` **47 passed**（新增 `model_form_opens_fills_and_saves`）；
`cargo test -p whcode-bin --bin whcode model_editor` **2 passed**（追加成功／重名拒绝／空字段拒绝／
Ollama 免密钥）；构建后在工作区放 `profiles.json`，GUI 实跑截图
`artifacts/gui-model-picker-2026-11-23.png`：下拉显示「本地 Ollama」、「添加模型」按钮可见、
侧栏无视图目录、未选会话无详情标签。

**未取得（如实登记）**：表单的「点击打开」与外观只由单测覆盖（`Message::OpenModelForm` →
字段变更 → `SaveModel`），本机 PostMessage 合成点击未被 iced 接受，未经人工目视；保存后注册表
不热更新，界面明确提示「重启后出现在模型列表」。

**仍与 ZCode 不同**：①侧栏底部未固定、无折叠按钮；②无项目切换（单工作区）。

## 落地记录（2026-11-23 第六批：细粒度对齐，尺寸 + 具体功能）

用户要求「细粒度对齐」「细粒度到具体功能」。本批直接以 ZCode 源码常量与 DESIGN 条款为基准：

| 项 | ZCode 基准 | 本批取值 |
|---|---|---|
| 侧栏宽度 | `WORKSPACE_SIDEBAR_DEFAULT_WIDTH_PX = 264`，`_MIN_WIDTH_PX` 同为 264 | 264，且**移除**窄窗收窄（原 200） |
| 侧栏分组标题 | `text-ui-base` 下的 `px-2.5 py-1`（10/4） | 12px 字号、内距 [4,10] |
| 任务行 | `text-ui-base`(14) + `px-2.5 py-1`，行间 `py-px` | 14px 字号、内距 [4,10]、行间 2px |
| 任务行右侧 | 显示更新时间 | **相对时间四档**：刚刚／N 分钟前／N 小时前／N 天前（十三套） |
| 顶栏 | `text-ui-xl`(18) + `caption`(13)，`border-b` | 18/13，下加 1px 分隔线 |
| 主聊天输入壳 | DESIGN 明确「main chat input shell 是 `rounded-2xl` 例外」 | 16px 圆角，发送与输入同排右端，模型在第二行 |
| 卡片内距 | 「main content cards usually use 16px horizontal padding」 | 16px |
| 面板间隙 | 侧栏与内容紧凑相邻 | 外壳 margin 8、面板 gap 8（原 12/16） |
| 任务行动作 | 行内菜单：重命名／归档／删除 | 落在**顶栏**（iced 无右键/悬停菜单），动作同名同义 |

**功能项补齐**：选中会话后顶栏出现「重命名／归档／删除」——这三个动作此前只在「会话」视图的
按钮行里，进入对话后不可达；现在与 ZCode 任务行菜单等价可达。

**验证（本机实跑）**：clippy（gui＋bin，-D warnings）通过；`cargo test -p whcode-gui` 47 passed；
构建后实跑截图 `artifacts/gui-align-2026-11-23.png`：侧栏 264、任务行「1 分钟前」、顶栏与状态栏
各有分隔线、composer 大圆角且发送在输入框右端、模型下拉在第二行。

**仍缺（如实登记，不作「已对齐」宣称）**：①侧栏顶部缺项目切换器（ZCode 有项目名 + 下拉）；
②任务行没有悬停式行内菜单与未读点；③composer 无附件／图片／@文件／斜杠命令／权限模式；
④无用量面板与远程设备；⑤字体依赖系统是否装 Inter／JetBrains Mono（FR-716 按族名取用）。


## 落地记录（2026-11-25 第七批：@ 文件索引接入 + Web 视觉层级）

用户指令「继续全ui界面ui对齐」。本批收口第六批登记的两项缺口，并把 Web 的控件层级补齐。

- **GUI「@ 文件」真正可用**（第六批仍缺项 ③ 的一部分）：`FileIndex` trait 此前**无任何实现**，
  `RunOptions.file_index` 恒为 `None`，按钮虽在但恒禁用。新增
  `crates/whcode-bin/src/file_index.rs` 的 `WorkspaceFileIndex`——深度上限 8、条目上限 4000、
  单次返回上限 50，跳过 `.git`／`target`／`node_modules`／`vendor`／`.whcode`／`dist`／
  `build`／`.cache`，**不跟随符号链接**（防逃逸）；匹配为大小写不敏感子串，短路径优先
  以贴合“顶层文件更可能被引用”。装配根注入，GUI 仍不触碰文件系统（分层门禁不变）。
- **Web 控件三档**：`button.primary`（填充）／默认（描边，等价 ZCode outline）／
  `button.ghost`（透明，悬停显影）。**未设 `button.secondary`**——默认 button 已是描边档，
  再加一档无法与它区分，属形式冗余。接入点：会话重命名／归档预览／删除预览改 ghost
  （弱操作），新建保持 submit（主操作）。
- **Web 空态样式** `.empty`：居中、`fg-subtle`、虚线边框 + `xl` 圆角（ZCode 空列表同形制）。
- **Web 静态文案与 i18n 同步**：`diff`→`Diff`、`维护`→`归档管理`、`排序`→`排序方式`、
  `最近活跃`→`按活动时间`。运行时本由 i18n 覆盖，但静态 HTML 与目录不一致会误导无 JS
  读者与检索，故一并对齐。

**有意差异（登记，不作“已对齐”宣称）**：①侧栏项目切换器——WHCode 单工作区（每次运行绑定
一个根），多项目下拉无对应能力，不造空壳；②任务行悬停式行内菜单与未读点——iced 无悬停菜单，
动作已等价落在顶栏（第六批），未读点需“已读”状态而库中无此字段；③用量面板与远程设备——
无对应能力。

**验证（本机）**：`cargo test -p whcode-bin file_index::` 2 通过（子串大小写、跳过构建目录、
limit 有界）；`clippy --workspace --all-targets -D warnings` 通过；`cargo test --locked
--workspace` 通过；`check-frontend-budget` 通过（CSS gzip 5237 B / 20480、DOM 99）；
`web-ui-check.py`（playwright，1280x800）`WEB_TABLIST_GATE=GO`；counts／traceability／
docs-style／EOL／coherence pre（反例 0）全绿。
