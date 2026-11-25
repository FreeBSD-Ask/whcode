//! # whcode-gui
//!
//! 原生桌面 GUI（FR-711—FR-715、ADR-027；交互对等见 ADR-056）。
//!
//! 边界：
//! - 取数只经 [`whcode_app::view_source`] 的只读快照，**不触库**、不绕过应用服务（ADR-003）；
//! - 不内嵌任何浏览器引擎（ADR-026）；渲染由 `iced` 自绘，使用 **tiny-skia 软件光栅**，
//!   默认不创建 GPU 上下文（NFR-212、ADR-027）；
//! - 空闲不重绘：没有定时器与动画，重绘只由输入或窗口事件触发（FR-714）；
//! - 写命令与 LLM 对话（ADR-056）：经装配根注入的 [`CommandExecutor`] 与
//!   [`GuiLlmExecutor`]，语义与 TUI 三端一致——`run_turn` 同步等待完成（执行期间界面
//!   阻塞，与 TUI 同）；破坏性命令预览→确认两段式；执行器未接入时显式提示
//!   `not_supported`（不静默无响应）。
//!
//! 渲染纪律（§5.3）：帧路径只消费准备阶段生成的纯数据快照，不反序列化 JSON、不查询数据库。

pub mod surface_map;

use std::rc::Rc;

use iced::futures::StreamExt;
use iced::futures::channel::mpsc;
use iced::futures::channel::oneshot;
use iced::keyboard::{Key, key};
use iced::widget::{
    button, column, container, pick_list, rich_text, row, scrollable, span, text, text_input,
};
use iced::{
    Background, Border, Color, Element, Font, Length, Shadow, Size, Subscription, Task, Theme,
    window,
};
use whcode_app::SessionSort;
use whcode_app::commands::{CommandExecutor, FrontendCommand};
use whcode_app::panels::{McpStatus, SubagentState};
use whcode_app::view_source::{
    CheckpointItem, PanelsSnapshot, SessionRow as ViewSessionRow, VIEW_COUNT, ViewId, ViewSnapshot,
    ViewSource,
};
use whcode_domain::WhError;
use whcode_domain::i18n::{I18n, is_rtl_locale};

/// 窗口初始逻辑尺寸（与 FR-712 的预算口径一致）。
const WINDOW_SIZE: Size = Size::new(1200.0, 800.0);

/// 窗口图标（FR-765、ADR-061）：`assets/brand/logo.svg` 经
/// `.github/scripts/make-brand-assets.py` 派生的 64×64 原始 RGBA。
///
/// 用原始 RGBA 而不是 PNG：GUI 不为一张图标引入图像解码依赖（依赖最小化，NFR-215）。
const WINDOW_ICON_RGBA: &[u8] = include_bytes!("../assets/brand/icon-64.rgba");
/// 图标边长（像素），与生成脚本的 `GUI_ICON_SIZE` 必须一致。
const WINDOW_ICON_SIZE: u32 = 64;

/// 内置字体子集（FR-716／ADR-028）：完整字体在 `assets/fonts/source`，
/// 这里只嵌入 `.github/scripts/subset-fonts.py` 生成的子集，避免整份 CJK 字体常驻。
const FONT_INTER: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.subset.ttf");
const FONT_NOTO: &[u8] = include_bytes!("../assets/fonts/NotoSansCJKsc-Regular.subset.otf");
const FONT_JETBRAINS: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Regular.subset.ttf");
const FONT_SARASA: &[u8] = include_bytes!("../assets/fonts/SarasaMonoSC-Regular.subset.ttf");

/// GUI 主题（六套：既有四套铁律精确色值 FR-612／FR-713；whcode 系 FR-739 为
/// GUI 间距（u16 形态，取自 `theme_tokens::spec`；1／3／5／7 为亚网格微调，允许字面量）。
const SP_0_5: u16 = whcode_domain::theme_tokens::spec::SPACE_0_5;
const SP_1: u16 = whcode_domain::theme_tokens::spec::SPACE_1;
const SP_1_5: u16 = whcode_domain::theme_tokens::spec::SPACE_1_5;
const SP_2: u16 = whcode_domain::theme_tokens::spec::SPACE_2;
const SP_2_5: u16 = whcode_domain::theme_tokens::spec::SPACE_2_5;
const SP_3: u16 = whcode_domain::theme_tokens::spec::SPACE_3;
const SP_4: u16 = whcode_domain::theme_tokens::spec::SPACE_4;

/// GUI 间距的 f32 形态（`spacing` 接受 `Into<Pixels>`，`Pixels: From<f32>`）。
const SPF_0_5: f32 = SP_0_5 as f32;
const SPF_1: f32 = SP_1 as f32;
const SPF_1_5: f32 = SP_1_5 as f32;
const SPF_2: f32 = SP_2 as f32;
const SPF_2_5: f32 = SP_2_5 as f32;
const SPF_3: f32 = SP_3 as f32;

/// GUI 字号（f32 形态，唯一取自 `theme_tokens::spec` 的七级体系）。
///
/// ADR-102 校准：此前散落 11／15／20 三个**不在七级内**的字面量，且改动 spec 常量
/// 不会生效。现统一为别名常量——改 spec 即全局生效，代码里不再出现字号字面量。
const FONT_XS: f32 = whcode_domain::theme_tokens::spec::UI_FONT_XS as f32;
const FONT_SM: f32 = whcode_domain::theme_tokens::spec::UI_FONT_SM as f32;
const FONT_CAPTION: f32 = whcode_domain::theme_tokens::spec::UI_FONT_CAPTION as f32;
const FONT_BASE: f32 = whcode_domain::theme_tokens::spec::UI_FONT_BASE as f32;
const FONT_LG: f32 = whcode_domain::theme_tokens::spec::UI_FONT_LG as f32;
const FONT_XL: f32 = whcode_domain::theme_tokens::spec::UI_FONT_XL as f32;

/// 2026-11-06 用户新增并批准冻结，色值源 `whcode_domain::theme_tokens`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GuiTheme {
    /// 墨水屏日间：纯白底、纯黑字（残影最小）。
    EinkDay,
    /// 墨水屏夜间：纯黑底、纯白字。
    EinkNight,
    /// AMOLED 夜间：纯黑底、浅灰字（仅纯黑可关断 OLED 像素）。
    AmoledNight,
    /// 终端绿夜间：纯黑底（OLED 兼容纯黑）、荧光绿字（CRT P1 风格，#00FF00）。
    TerminalGreen,
    /// whcode 日间（FR-739，色值单一事实源 `theme_tokens`）。
    WhcodeDay,
    /// whcode 夜间（**默认**，2026-11-06 用户批准切换；ADR-057/FR-739）。
    #[default]
    WhcodeNight,
}

impl GuiTheme {
    /// 配置名（与 `ui.theme` 取值一致）。
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::EinkDay => "eink-day",
            Self::EinkNight => "eink-night",
            Self::AmoledNight => "amoled-night",
            Self::TerminalGreen => "terminal-green",
            Self::WhcodeDay => "whcode-day",
            Self::WhcodeNight => "whcode-night",
        }
    }

    /// 按配置名解析；未知取值返回 `None`（调用方决定回退或报错）。
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "eink-day" => Some(Self::EinkDay),
            "eink-night" => Some(Self::EinkNight),
            "amoled-night" => Some(Self::AmoledNight),
            "terminal-green" => Some(Self::TerminalGreen),
            "whcode-day" => Some(Self::WhcodeDay),
            "whcode-night" => Some(Self::WhcodeNight),
            _ => None,
        }
    }

    /// 下一套主题（`t` 键循环；FR-713：切换不得要求重启；FR-739 后为六套）。
    #[must_use]
    const fn next(self) -> Self {
        match self {
            Self::EinkDay => Self::EinkNight,
            Self::EinkNight => Self::AmoledNight,
            Self::AmoledNight => Self::TerminalGreen,
            Self::TerminalGreen => Self::WhcodeDay,
            Self::WhcodeDay => Self::WhcodeNight,
            Self::WhcodeNight => Self::EinkDay,
        }
    }

    /// 背景色与前景色（精确 RGB）。既有四套保持铁律字面值；whcode 系从
    /// `whcode_domain::theme_tokens` 冻结值解析（FR-740 单一事实源）。
    #[must_use]
    pub fn colors(self) -> (Color, Color) {
        match self {
            Self::EinkDay => (Color::WHITE, Color::BLACK),
            Self::EinkNight => (Color::BLACK, Color::WHITE),
            Self::AmoledNight => (Color::BLACK, Color::from_rgb(0.902, 0.902, 0.902)),
            Self::TerminalGreen => (Color::BLACK, Color::from_rgb(0.0, 1.0, 0.0)),
            Self::WhcodeDay => whcode_colors(whcode_domain::theme_tokens::WHCODE_DAY),
            Self::WhcodeNight => whcode_colors(whcode_domain::theme_tokens::WHCODE_NIGHT),
        }
    }

    /// diff 增／删行色（T260／FR-622）：whcode 系取冻结终端调色板（与 ZCode 同源
    /// `#1e8a3e`／`#e03131`、`#46bf72`／`#ff5c5c`）；既有四套没有冻结终端调色板，
    /// 两态回落前景色——不引入主题外颜色，状态仍由 `+`／`-` 前缀判定（ui-design §3）。
    #[must_use]
    pub fn diff_colors(self) -> (Color, Color) {
        match self {
            Self::WhcodeDay => {
                let palette = whcode_domain::theme_tokens::TERMINAL_DAY;
                (token_color(palette.green), token_color(palette.red))
            }
            Self::WhcodeNight => {
                let palette = whcode_domain::theme_tokens::TERMINAL_NIGHT;
                (token_color(palette.green), token_color(palette.red))
            }
            _ => {
                let (_, foreground) = self.colors();
                (foreground, foreground)
            }
        }
    }

    /// iced 主题：调色板全部映射到主题色，避免出现框架默认蓝。
    fn iced(self) -> Theme {
        let (background, foreground) = self.colors();
        Theme::custom(
            self.name(),
            iced::theme::Palette {
                background,
                text: foreground,
                primary: foreground,
                success: foreground,
                warning: foreground,
                danger: foreground,
            },
        )
    }

    /// 滚动条样式：轨道取背景色、滑块取前景色（FR-713：不引入主题外的灰色）。
    fn scrollbar(self) -> scrollable::Style {
        let (background, foreground) = self.colors();
        let rail = scrollable::Rail {
            background: Some(Background::Color(background)),
            border: Border::default(),
            scroller: scrollable::Scroller {
                background: Background::Color(foreground),
                border: Border::default(),
            },
        };
        scrollable::Style {
            container: container::Style::default(),
            vertical_rail: rail,
            horizontal_rail: rail,
            gap: None,
            auto_scroll: scrollable::AutoScroll {
                background: Background::Color(background),
                border: Border::default(),
                shadow: Shadow::default(),
                icon: foreground,
            },
        }
    }
}

/// GUI 侧 LLM 执行器（ADR-056；语义与 `whcode_tui::LlmExecutor` 一致）。
///
/// 由装配根实现（包装 `TuiLlmExecutor`）：GUI 不直接依赖 `whcode-bin`，经此 trait 反转。
/// `Send + Sync` 是 Phase 2（后台执行）的硬要求：`run_with_progress` 在独立线程运行
/// （独立线程 + `Task::stream` 有损进度通道），界面线程仅经 `cancel` 取消。
pub trait GuiLlmExecutor: Send + Sync {
    /// 发起一次真实调用（**阻塞至 turn 完成**；在调用方的后台线程上执行，不冻结界面）。
    /// 会话取 GUI 当前选中会话；`request_id` 由调用方预生成并传入（确定性取消，
    /// ADR-087 修订：与 TUI/CUI 显式 `--request-id` 同口径，取消不再从摘要反解析）；
    /// `progress` 逐块回调累计块数（回调必须非阻塞、有损可接受）。
    ///
    /// # Errors
    ///
    /// 剖面未注册、凭据缺失或调用失败时返回 [`WhError`]。
    #[allow(clippy::too_many_arguments)]
    fn run_with_progress(
        &self,
        session_id: &str,
        provider: &str,
        prompt: &str,
        request_id: &str,
        progress: whcode_app::turn::TurnProgress<'_>,
        observer: Option<whcode_app::turn::TurnObserver<'_>>,
        approvals: Option<&dyn whcode_app::approval::ApprovalAnswerer>,
    ) -> Result<String, WhError>;

    /// 取消同机正在运行的调用（取消标记目录跨线程生效；不触库、不需要连接锁）。
    ///
    /// # Errors
    ///
    /// 请求 ID 非法或无活跃调用时返回 [`WhError`]。
    fn cancel(&self, request_id: &str) -> Result<String, WhError>;

    /// 可选剖面名清单（注册表顺序；对话视图显示与默认选择用）。
    fn profiles(&self) -> Vec<String>;

    /// 模型目录摘要（T393／llm.models）：剖面 × 默认模型 × 可选模型清单。
    /// 默认显式不支持——未接线时错误可见（FR-620），不伪造目录。
    ///
    /// # Errors
    ///
    /// 剖面未注册或装配未接线时返回 [`WhError`]。
    fn model_summary(&self) -> Result<String, WhError> {
        Err(WhError::new(
            whcode_domain::ErrorCode::NotSupported,
            "未注入 LLM 执行器：模型目录不可用（装配根未接线）",
        ))
    }
}

/// GUI 侧审批应答面（T256／FR-745）：把待审批请求推给界面，再同步等待界面决定。
///
/// 无轮询、无定时器：请求经与工具信号同一条有界通道送到界面（`try_send`）。通道拥塞
/// 时**不等待**，按“无可用应答者”返回（闸门判 `unavailable`，fail-closed）——界面收不
/// 到的问题不得被当成默认允许。
pub struct GuiApprovals {
    inner: whcode_app::approval::InteractiveApprovals,
    tx: mpsc::Sender<Message>,
}

impl GuiApprovals {
    /// 待审批请求（界面渲染用；有界，最多 `MAX_PENDING_APPROVALS`）。
    #[must_use]
    pub fn pending(&self) -> Vec<whcode_domain::approval::ApprovalRequest> {
        self.inner.pending()
    }

    /// 界面决定：把答案交给等待中的 turn 线程。
    ///
    /// # Errors
    ///
    /// 请求已结算或不存在时返回 `not_found`（界面不得把失败当成功）。
    pub fn decide(
        &self,
        request_id: &str,
        answer: whcode_domain::approval::ApprovalAnswer,
    ) -> Result<(), WhError> {
        self.inner.decide(request_id, answer)
    }
}

impl whcode_app::approval::ApprovalAnswerer for GuiApprovals {
    fn answer(
        &self,
        request: &whcode_domain::approval::ApprovalRequest,
        budget_ms: u64,
    ) -> Result<Option<whcode_domain::approval::ApprovalAnswer>, WhError> {
        let mut sender = self.tx.clone();
        if sender
            .try_send(Message::ApprovalPending(request.clone()))
            .is_err()
        {
            // 界面通道拥塞：立即判不可用，不占用等待预算。
            return Ok(None);
        }
        self.inner.answer(request, budget_ms)
    }
}

/// 工具卡（T256／FR-743）：一次工具调用的可视状态；卡片数有界（`MAX_TOOL_CARDS`）。
#[derive(Debug, Clone, PartialEq, Eq)]
struct ToolCard {
    tool_call_id: String,
    tool_name: String,
    exec_id: String,
    /// 终态稳定名；`None` = 运行中（渲染时取 `gui.tool.running`）。
    state: Option<String>,
    output: Option<String>,
    error_code: Option<String>,
    duration_ms: Option<u64>,
}

/// 工具卡上限（视图有界；超出移除最早的一张）。
const MAX_TOOL_CARDS: usize = 20;

/// 侧栏会话列表上限（ADR-102 功能对齐）：侧栏只承载最近的一页，超出显示剩余计数，
/// 完整目录仍在「会话」视图——帧路径有界，不因会话数增长拖慢重绘。
const SIDEBAR_SESSION_ROWS: usize = 14;

/// 读面详情／门禁摘要文本上限（ADR-091／FR-799）：事件详情与 Spec 门禁摘要各
/// 200 字符（与 TUI 事件详情的 `chars().take(200)` 同口径）；超出在字符边界截断
/// 并注明，不伪造完整内容。字段有界且无定时器（FR-714）。
const READ_DETAIL_TEXT_MAX: usize = 200;

/// 把详情／摘要文本裁到 `READ_DETAIL_TEXT_MAX` 字符（按字符边界，避免切断 UTF-8）；
/// 超出时附加截断注明，不伪造完整内容（ADR-091／FR-799）。
fn truncate_detail(text: &str) -> String {
    let mut chars = text.chars();
    let kept: String = chars.by_ref().take(READ_DETAIL_TEXT_MAX).collect();
    if chars.next().is_some() {
        format!("{kept}…（已截断）")
    } else {
        kept
    }
}

/// 会话操作面板的当前状态（ADR-056；破坏性命令两段式）。
#[derive(Debug, Clone, PartialEq, Eq)]
enum OpMode {
    /// 新建会话：输入即标题。
    SessionNew,
    /// 重命名会话：输入即新标题。
    SessionRename,
    /// 归档/恢复预览已出，待确认（`archived` 为目标状态）。
    SessionArchive { archived: bool },
    /// 删除预览已出，待确认。
    SessionDelete,
    /// 清理预览已出，待确认（`before_ms` 冻结于预览时点）。
    Purge { before_ms: i64 },
    /// 会话分叉（T393／FR-769）：输入即分叉序号（0＝空分叉）。
    SessionFork,
    /// 对话回退（T393／FR-775）：输入即目标序号（0＝会话起点）。
    SessionRewind,
    /// 前像恢复（T393／FR-776）：预览→确认两段式，`exec_id` 由预览阶段冻结。
    SessionRestore { exec_id: String },
    /// 计划模式（T393／FR-774）：输入即 on／off。
    SessionPlanSet,
    /// SDD 阶段推进（ADR-092／FR-800）：`spec.next`，无预览，输入为目标阶段
    /// （空＝沿状态机后继）；经命令门面执行。
    SpecNext,
    /// 人工强制越过门禁（ADR-092／FR-800）：`spec.force-advance`，两步——
    /// 先输入目标阶段冻结于 `to`，再输入必填 reason 后经门面执行。
    SpecForceAdvance { to: Option<String> },
    /// 启用运行时（ADR-092／FR-800 批 2）：`name` 由选中行冻结。
    RuntimeEnable { name: String },
    /// 停用运行时（ADR-092／FR-800 批 2）：`name` 由选中行冻结。
    RuntimeDisable { name: String },
    /// 登记本地 payload（ADR-092／FR-800 批 2）：`name` 冻结后输入
    /// `版本|哈希|本地路径` 三段（路径可含 `|` 取最后两段为哈希与路径）。
    RuntimeInstall { name: String },
    /// 写入审批规则（ADR-092／FR-800 批 3）：输入 `工具名|allow|deny`。
    ApprovalRuleWrite,
    /// 导出事件（ADR-092／FR-800 批 3）：输入 `after_seq|limit`，有界文本呈现。
    EventsExport,
    /// 证书写操作（ADR-092／FR-800 批 3，ADR-040 决策 4 修订）：以输入框指定动作。
    /// 私钥上传（WriteServer）不在 GUI 提供——避免凭据进入输入控件（0 信任／安全底线）。
    CertWrite(CertWriteUiAction),
}

/// GUI 证书写动作（ADR-092 批 3）：只列不将私钥内容带入 GUI 状态的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CertWriteUiAction {
    /// 设置档位：输入 `builtin|builtin|false`（出站|入站|确认降档）。
    SetModes,
    /// 写入追加 CA（仅公钥 PEM 文本，不含私钥）。
    WriteExtraCa,
    /// 重置内置 CA。
    ResetBuiltinCa,
    /// 重新生成自签服务端证书。
    RegenerateServer,
}

/// 添加模型表单的草稿（字段与 `profiles.json` 的必填项一致）。
#[derive(Debug, Clone, Default)]
pub struct ModelDraft {
    /// 模型名（唯一，`profiles[].name`）。
    pub name: String,
    /// 协议族：`openai`／`anthropic`／`gemini`／`ollama`／`cohere`。
    pub protocol: String,
    /// 端点源（`https://host[:port]`）。
    pub base_url: String,
    /// 模型标识（`profiles[].model`）。
    pub model: String,
    /// 密钥环境变量名（Ollama 等无鉴权协议可留空）。
    pub credential_env: String,
}

/// 已注册协议（与 `whcode_llm::http::profile::Protocol` 同名小写形式）。
pub const MODEL_PROTOCOLS: [&str; 5] = ["openai", "anthropic", "gemini", "ollama", "cohere"];

/// 模型配置写面（由装配根注入）。
///
/// GUI 自身不碰文件系统（ADR-003／§5.3）：增删模型经本 trait 落到装配根的配置文件。
/// 工作区文件索引（由装配根注入）。
///
/// composer 的「@ 文件」用它列出候选路径并插入提示词；GUI 自身不遍历文件系统。
pub trait FileIndex {
    /// 按相对路径子串匹配（大小写不敏感），有界返回。
    fn files(&self, needle: &str, limit: usize) -> Vec<String>;
}

pub trait ModelEditor {
    /// 追加一个模型并落盘；返回写入的配置文件路径。
    ///
    /// # Errors
    ///
    /// 名称重复、端点非法、凭据变量名为空（有鉴权协议）、文件不可写时返回可操作错误。
    fn add(&self, draft: &ModelDraft) -> Result<String, WhError>;
}

#[derive(Default)]
pub struct RunOptions {
    /// 主题（FR-612）。
    pub theme: GuiTheme,
    /// 工作区目录名（侧栏「项目」分区显示；ZCode 侧栏同样按项目组织任务）。
    pub workspace: String,
    /// 界面文本层（FR-615）：默认 zh-CN + en 回退。
    pub texts: I18n,
    /// 写命令执行器（FR-623/ADR-056）：`Some` 时会话/任务/清理写操作可用；`None` 时
    /// 显式提示 `not_supported`（不静默无响应，与 TUI 同口径）。
    pub command_executor: Option<Box<dyn CommandExecutor>>,
    /// LLM 执行器（ADR-056 Phase 2）：`Some` 时对话视图可用（后台线程执行）；`None`
    /// 时显式提示 `not_supported`。`Arc` 以便克隆进阻塞线程。
    pub llm_executor: Option<std::sync::Arc<dyn GuiLlmExecutor>>,
    /// 模型配置写面（`None` = 装配未接入：表单可填但保存显式提示不可用）。
    pub model_editor: Option<Rc<dyn ModelEditor>>,
    /// 工作区文件索引（「@ 文件」候选；`None` = 未接入，按钮显式禁用）。
    pub file_index: Option<Rc<dyn FileIndex>>,
}

/// 添加模型表单的文本字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelField {
    Name,
    BaseUrl,
    Model,
    Credential,
}

/// 界面消息（按键、窗口事件与控件回调产生；无定时器）。
#[derive(Debug, Clone)]
enum Message {
    SelectView(ViewId),
    /// ADR-102：顺序切换视图（`Tab` 前进、`Shift+Tab` 后退），免记数字键。
    CycleView(i32),
    NextSession,
    /// ADR-102 功能对齐：点击侧栏任务行直接选中该会话并进入对话（ZCode 行为）。
    SelectSession(usize),
    PrevSession,
    Refresh,
    CycleTheme,
    Resized(Size),
    // —— 对话（ADR-056）——
    /// 剖面名输入。
    ChatProviderChanged(String),
    /// 提示词输入。
    ChatInputChanged(String),
    /// 提交一轮对话（Phase 2：后台线程执行 `run_turn_with_progress`，界面不冻结）。
    ChatSubmit,
    /// turn 完成（后台线程 → oneshot → `Task::stream` 送达；`turn_running` 解除）。
    ChatDone(Result<String, WhError>),
    /// 流式进度（累计块数；有损——通道满即丢弃）。
    ChatProgress(u64, String),
    /// 取消正在运行的调用（按请求 ID）。
    ChatCancel,
    /// 工具调用信号（T256／FR-743）：turn 运行期间经有界通道送达（丢帧可接受）。
    ToolSignal(whcode_app::turn::TurnSignal),
    /// 待审批请求（T256／FR-745）：turn 线程阻塞等待，界面给出四态决定。
    ApprovalPending(whcode_domain::approval::ApprovalRequest),
    /// 界面决定：四态之一（本回合／仅本次／记住规则／拒绝）。
    ApprovalDecide(whcode_domain::approval::ApprovalAnswer),
    // —— 会话/任务/清理操作（ADR-056）——
    /// 操作面板输入（标题／任务 ID）。
    OpInputChanged(String),
    /// 模型目录（T393／llm.models）：拉取剖面×模型摘要显示在对话视图。
    ShowModels,
    // —— 添加模型（ADR-102 修正：让「添加模型」在界面上真实存在）——
    /// 打开添加模型表单。
    OpenModelForm,
    /// 关闭添加模型表单（不写文件）。
    CloseModelForm,
    /// 表单文本字段变更（名称／端点／模型／密钥变量名）。
    ModelFieldChanged(ModelField, String),
    /// 表单协议变更（openai／anthropic／gemini／ollama／cohere）。
    ModelProtocolChanged(String),
    /// 保存模型（写配置文件；失败显式提示原因）。
    SaveModel,
    // —— @ 文件（ADR-102 功能对齐）——
    /// 打开文件候选面板（列出工作区文件）。
    OpenFilePicker,
    /// 切换计划模式（ADR-102 composer 切换器；读 `snapshot.plan_active` 取反）。
    PlanToggle,
    /// 文件候选的过滤词变更。
    FileQueryChanged(String),
    /// 把候选路径插入提示词末尾。
    InsertFile(String),
    /// 关闭文件候选面板。
    CloseFilePicker,
    /// 打开操作面板（新建／重命名；破坏性命令先进入预览态）。
    OpStart(OpMode),
    /// 破坏性命令：先预览（confirm=false，不写库）。
    OpPreview,
    /// 破坏性命令：确认执行（confirm=true）。
    OpConfirm,
    /// 关闭操作面板（不执行任何写操作）。
    OpCancel,
    /// 任务重试（按下标索引 `snapshot.tasks`）。
    TaskRetry(usize),
    /// 任务中止（按下标索引）。
    TaskAbort(usize),
    /// 查看任务检查点（按下标索引 `snapshot.tasks`；FR-748）。
    TaskCheckpoints(usize),
    // —— B4 高级面板（T258、FR-749／750／751）——
    /// 打开／关闭高级面板（`s` 键；打开时在准备阶段取数）。
    TogglePanels,
    /// 重新读取面板数据（面板内刷新；不触库以外的副作用）。
    RefreshPanels,
    /// 撤销一条本地审批规则（工具稳定名）。
    RevokeRule(String),
    // —— 侧栏（B2，FR-746）——
    /// 侧栏搜索词（标题/ID/阶段子串，本地过滤，不触库）。
    SessionFilterChanged(String),
    /// 内容检索词（T319／FR-780：按消息正文检索，经数据源命令）。
    ContentSearchChanged(String),
    /// 执行内容检索（输入框回车或按钮）。
    ContentSearchSubmit,
    /// 侧栏排序键（最近活跃/创建/标题）。
    SessionSortChanged(SessionSort),
    /// 是否展示归档分段（含归档时活跃在前、归档在后）。
    SessionArchivedToggled(bool),
    /// Esc：有面板时取消面板，否则退出。
    EscapeKey,
    // —— ADR-091／FR-799 读面详情——
    /// 事件详情（按 `snapshot.events` 下标；`e` 键，经 `ViewSource::event_detail`）。
    EventDetail(usize),
    /// Spec 门禁摘要（`v`／`z`／`o`；`kind` 取 validate／analyze／converge）。
    SpecGate(&'static str),
    // —— ADR-092／FR-800 批 2 运行时视图——
    /// 选中运行时行（按下标索引 `runtime_entries`）；随后读取其状态（probe=false）。
    RuntimeSelect(usize),
    /// 字符键（门控在 update：对话视图／操作面板打开时忽略全局快捷键）。
    Key(char),
}

/// 应用状态：只读快照 + 交互状态（对话与写操作，ADR-056）。
struct State {
    source: Rc<dyn ViewSource>,
    texts: I18n,
    /// 主题（同时用于 iced 主题与滚动条样式）。
    theme: GuiTheme,
    view: ViewId,
    /// 会话选择位置（`None` = 未选择）。
    selected: Option<usize>,
    /// 会话 ID清单（准备层给出，`j`/`k` 在其中移动）。
    session_ids: Vec<String>,
    /// 当前视图快照。
    snapshot: ViewSnapshot,
    /// 当前窗口逻辑尺寸（状态栏显示，FR-712 口径）。
    size: Size,
    /// 写命令执行器（`None` = 装配未接入，写操作显式提示）。
    executor: Option<Rc<dyn CommandExecutor>>,
    /// LLM 执行器（`None` = 装配未接入，对话显式提示）。`Arc`：ChatSubmit 时克隆进
    /// 后台线程（Phase 2）。
    llm: Option<std::sync::Arc<dyn GuiLlmExecutor>>,
    /// turn 运行中（后台线程持有连接锁）：期间除进度/完成/窗口事件外的一切交互
    /// 冻结在当前画面（数据锁被占用，冻结优于阻塞等待）。
    turn_running: bool,
    /// 当前 turn 的预生成请求 ID（ADR-087 修订）：取消按钮直达执行器，不再从
    /// 完成摘要反解析；完成后随 ChatDone 复位。
    active_request_id: Option<String>,
    /// 当前 turn 的累计块数（进度回调经有损通道送达）。
    chat_progress: u64,
    /// 当前 turn 的流式正文缓冲（FR-741 逐块增量呈现；有界——
    /// `CHAT_STREAMING_CAP_BYTES` 截断，turn 输出预算 1 MiB 为天然上界）。
    chat_streaming: String,
    /// 对话历史缓存（FR-741：提交前与完成后各刷新一次——turn 运行期间连接锁
    /// 被后台线程持有，**不得**触库读历史；缓存渲染另有条数封顶）。
    chat_history: Vec<(String, String)>,
    /// 对话视图：剖面名（默认取执行器剖面清单首项）。
    chat_provider: String,
    /// 对话视图：提示词输入。
    chat_input: String,
    /// 模型目录摘要（T393／llm.models；`None`＝未请求）。
    model_status: Option<String>,
    /// 对话视图：最近一次结果摘要或错误（不复制模型正文）。
    chat_last: Option<String>,
    /// 操作面板模式（`None` = 关闭）。
    op_mode: Option<OpMode>,
    /// 操作面板输入（标题／任务 ID）。
    op_input: String,
    /// 操作面板反馈行（预览结果或错误）。
    op_feedback: Option<String>,
    /// 侧栏搜索词（FR-746；本地过滤，不触库）。
    session_filter: String,
    /// 内容检索词（T319／FR-780；经数据源命令，最新命中优先）。
    content_query: String,
    /// 内容检索命中（有界：默认 20 条；检索失败时清空并写状态行）。
    content_hits: Vec<whcode_app::ContentHit>,
    /// 内容检索状态行（错误或计数摘要；`None` = 无）。
    content_status: Option<String>,
    /// 侧栏排序键。
    session_sort: SessionSort,
    /// 是否展示归档分段。
    session_show_archived: bool,
    /// 未过滤的会话行（目录视图的原始集合；过滤/排序只作用于快照副本）。
    all_sessions: Vec<ViewSessionRow>,
    /// 侧栏常驻会话目录（ADR-102 功能对齐）：过滤＋排序后的结果，任何视图下都在，
    /// 与主区当前快照解耦——ZCode 的侧栏任务列表同样常驻。
    sidebar_sessions: Vec<ViewSessionRow>,
    /// 工作区目录名（侧栏「项目」分区；空串表示装配未提供）。
    workspace_name: String,
    /// 模型配置写面（`None` = 未接入，保存时显式提示）。
    model_editor: Option<Rc<dyn ModelEditor>>,
    /// 添加模型表单（`Some` = 表单打开）。
    model_form: Option<ModelDraft>,
    /// 表单反馈（保存成功路径或失败原因）。
    model_form_status: Option<String>,
    /// 工作区文件索引（`None` = 未接入）。
    file_index: Option<Rc<dyn FileIndex>>,
    /// 文件候选面板：过滤词与命中（`Some` = 面板打开）。
    file_picker: Option<(String, Vec<String>)>,
    /// 最近一次查看的检查点（FR-748；空＝尚未查看）。
    checkpoints: Vec<CheckpointItem>,
    /// 检查点所属任务。
    checkpoint_task: Option<String>,
    /// 工具卡（T256／FR-743）：本次对话期间的调用状态（有界，旧卡先移除）。
    tool_cards: Vec<ToolCard>,
    /// 当前待审批请求（T256／FR-745；`None` = 无）。
    approval: Option<whcode_domain::approval::ApprovalRequest>,
    /// 审批反馈行（决定结果或错误）。
    approval_status: Option<String>,
    /// 本次 turn 的审批应答面（turn 运行期间存在；决定经它交给等待线程）。
    approvals: Option<std::sync::Arc<GuiApprovals>>,
    /// 高级面板是否打开（T258；`s` 切换）。
    panels_open: bool,
    /// 高级面板快照（准备阶段生成；turn 运行期间不刷新）。
    panels: PanelsSnapshot,
    /// 面板反馈行（撤销结果或错误）。
    panel_status: Option<String>,
    // —— ADR-091／FR-799 读面详情（有界，无定时器）——
    /// 事件视图选中行（按 `snapshot.events` 下标）；`None` = 未选择。
    event_selected: Option<usize>,
    /// 事件详情文本（`e` 键，经 `ViewSource::event_detail`；有界 200 字符）。
    event_detail: Option<String>,
    /// Spec 门禁摘要文本（`v`／`z`／`o`，经 `spec_gate_summary`；有界 200 字符）。
    spec_summary: Option<String>,
    // —— ADR-092／FR-800 批 2 运行时视图（按需读取，无定时器）——
    /// 运行时声明清单（`runtime_entries`，probe=false；`None`＝尚未读取）。
    runtime_entries: Option<Vec<whcode_app::runtime_mgmt::RuntimeEntry>>,
    /// 运行时视图选中行下标；`None` = 未选择。
    runtime_selected: Option<usize>,
    /// 选中运行时状态（`runtime_status_view`，probe=false）。
    runtime_status: Option<whcode_app::runtime_mgmt::RuntimeStatusView>,
    /// 证书库状态（ADR-092 批 3；高级面板打开时按需读取，不含私钥）。
    tls_status: Option<whcode_domain::TlsStatus>,
}

impl State {
    fn new(
        source: Rc<dyn ViewSource>,
        texts: I18n,
        theme: GuiTheme,
        executor: Option<Rc<dyn CommandExecutor>>,
        llm: Option<std::sync::Arc<dyn GuiLlmExecutor>>,
    ) -> Self {
        // 侧栏目录常驻（Sessions 快照），主区按默认视图取自己的快照（ADR-102）。
        let sessions_snapshot = source.snapshot(ViewId::Sessions, None);
        let session_ids = sessions_snapshot
            .sessions
            .iter()
            .map(|row| row.id.clone())
            .collect::<Vec<_>>();
        let sidebar_sessions = sessions_snapshot.sessions.clone();
        let snapshot = source.snapshot(ViewId::Chat, None);
        let chat_provider = llm
            .as_ref()
            .and_then(|llm| llm.profiles().into_iter().next())
            .unwrap_or_default();
        let mut state = Self {
            source,
            texts,
            theme,
            view: ViewId::Chat,
            selected: None,
            session_ids,
            snapshot,
            sidebar_sessions,
            workspace_name: String::new(),
            model_editor: None,
            model_form: None,
            model_form_status: None,
            file_index: None,
            file_picker: None,
            size: WINDOW_SIZE,
            executor,
            llm,
            turn_running: false,
            chat_progress: 0,
            active_request_id: None,
            chat_streaming: String::new(),
            chat_history: Vec::new(),
            chat_provider,
            chat_input: String::new(),
            model_status: None,
            chat_last: None,
            op_mode: None,
            op_input: String::new(),
            op_feedback: None,
            session_filter: String::new(),
            content_query: String::new(),
            content_hits: Vec::new(),
            content_status: None,
            session_sort: SessionSort::LastActive,
            session_show_archived: false,
            all_sessions: Vec::new(),
            checkpoints: Vec::new(),
            checkpoint_task: None,
            tool_cards: Vec::new(),
            approval: None,
            approval_status: None,
            approvals: None,
            panels_open: false,
            panels: PanelsSnapshot::default(),
            panel_status: None,
            event_selected: None,
            event_detail: None,
            spec_summary: None,
            runtime_entries: None,
            runtime_selected: None,
            runtime_status: None,
            tls_status: None,
        };
        // 初始化即走一次准备层：默认进入对话时历史缓存与侧栏目录都要就位
        // （ADR-102：不能等用户第一次点击才填内容）。
        state.prepare();
        state
    }

    /// 目录视图重建（FR-746）：过滤 → 排序 → 分组序，并同步 `session_ids`
    /// （选中下标与 `NextSession`/`PrevSession` 依赖该顺序）。
    fn apply_session_view(&mut self) {
        let needle = self.session_filter.trim().to_lowercase();
        let mut rows: Vec<ViewSessionRow> = self
            .all_sessions
            .iter()
            .filter(|row| self.session_show_archived || !row.archived)
            .filter(|row| {
                needle.is_empty()
                    || row.title.to_lowercase().contains(&needle)
                    || row.id.to_lowercase().contains(&needle)
                    || row.stage.to_lowercase().contains(&needle)
            })
            .cloned()
            .collect();
        rows.sort_by(|left, right| {
            let group_rank = |row: &ViewSessionRow| u8::from(row.archived);
            group_rank(left)
                .cmp(&group_rank(right))
                .then_with(|| match self.session_sort {
                    SessionSort::LastActive => right.last_active_at_ms.cmp(&left.last_active_at_ms),
                    SessionSort::Created => right.created_at_ms.cmp(&left.created_at_ms),
                    SessionSort::Title => {
                        left.title.to_lowercase().cmp(&right.title.to_lowercase())
                    }
                })
                .then_with(|| left.id.cmp(&right.id))
        });
        self.sidebar_sessions = rows.clone();
        if self.view == ViewId::Sessions {
            self.snapshot.sessions = rows;
        }
        // 选中下标以侧栏目录为准（主区可能是对话等非会话视图，其快照不含目录）。
        self.session_ids = self
            .sidebar_sessions
            .iter()
            .map(|row| row.id.clone())
            .collect();
        if self.session_ids.is_empty() {
            self.selected = None;
        } else if self
            .selected
            .is_some_and(|index| index >= self.session_ids.len())
        {
            self.selected = Some(self.session_ids.len() - 1);
        }
    }

    /// 以当前选择重新准备快照（准备层；帧路径不查询）。
    fn prepare(&mut self) {
        self.snapshot = self
            .source
            .snapshot(self.view, self.selected_session().as_deref());
        // 侧栏目录常驻：切视图是低频路径，这里多取一次会话快照换取侧栏始终有内容。
        self.all_sessions = self.source.snapshot(ViewId::Sessions, None).sessions;
        self.apply_session_view();
        if self.view == ViewId::Chat {
            self.refresh_chat_history();
        }
        if self.view == ViewId::Runtime {
            // 声明清单按需读取（probe=false，不启动子进程；无定时器）。
            self.runtime_entries = Some(self.source.runtime_entries());
        }
    }

    /// 读取选中运行时状态（ADR-092／FR-800 批 2；probe=false，显式选中才读，不探测）。
    fn runtime_status(&mut self, name: &str) {
        match self.source.runtime_status_view(name) {
            Ok(view) => self.runtime_status = Some(view),
            Err(err) => {
                self.runtime_status = None;
                self.op_feedback = Some(format!("{}: {err}", self.texts.resolve("gui.op.error")));
            }
        }
    }

    /// 对话历史刷新（FR-741）：仅在连接锁空闲时调用（prepare／提交前／完成后）。
    fn refresh_chat_history(&mut self) {
        self.chat_history = self
            .selected_session()
            .map(|session| self.source.conversation_tail(&session, CHAT_HISTORY_MAX))
            .unwrap_or_default();
    }

    fn selected_session(&self) -> Option<String> {
        self.selected
            .and_then(|index| self.session_ids.get(index))
            .cloned()
    }

    /// 帧标题（与 TUI 同一词条：`tui.title`）。
    fn title(&self) -> String {
        self.texts.resolve_fmt(
            "tui.title",
            &[
                ("app", self.texts.resolve("app.title")),
                ("view", self.texts.resolve(self.view.title_key())),
                ("index", &self.view.index().to_string()),
                ("count", &VIEW_COUNT.to_string()),
            ],
        )
    }

    /// 会话标识（状态栏用）：未选择时显示显式占位（不渲染空串）。
    fn session_label(&self) -> String {
        match self.selected_session() {
            Some(id) => self
                .texts
                .resolve_fmt("cui.record.session", &[("session", &id)]),
            None => self.texts.resolve("gui.session.none").to_string(),
        }
    }

    /// 顶栏标题（ADR-102）：当前会话标题；没有会话时是「开始新对话」。
    /// 可用模型清单（LLM 执行器的剖面名；无执行器或未配置时为空）。
    fn model_options(&self) -> Vec<String> {
        self.llm
            .as_ref()
            .map(|llm| llm.profiles())
            .unwrap_or_default()
    }

    fn current_session_title(&self) -> String {
        self.selected
            .and_then(|index| self.sidebar_sessions.get(index))
            .map(|row| row.title.clone())
            .unwrap_or_else(|| self.texts.resolve("gui.chat.new").to_string())
    }

    /// 顶栏副标题：会话阶段与状态；无会话时为空（不占位）。
    fn current_session_meta(&self) -> String {
        self.selected
            .and_then(|index| self.sidebar_sessions.get(index))
            .map(|row| format!("{} · {}", row.stage, row.status))
            .unwrap_or_default()
    }

    /// 状态栏：键位提示 + 窗口尺寸 + 当前主题（与 TUI 同一批词条）。
    fn footer(&self) -> String {
        // ADR-102：底栏说「怎么操作」与当前主题；窗口尺寸对用户无操作含义，
        // 不再复用 TUI 的 cols／rows 模板（终端族与应用族文案分离）。
        self.texts.resolve_fmt(
            "gui.footer",
            &[
                ("theme", self.theme.name()),
                ("session", &self.session_label()),
            ],
        )
    }
}

/// 启动 GUI 闭环。
///
/// `source` 由装配根构造（只读快照生成方），生命周期须覆盖整个应用；
/// 调用方负责在无交互环境时不要进入本函数。
///
/// # Errors
///
/// 窗口创建或渲染后端初始化失败时返回 `iced::Error`。
pub fn run(source: Box<dyn ViewSource>, options: RunOptions) -> iced::Result {
    let source: Rc<dyn ViewSource> = Rc::from(source);
    let RunOptions {
        theme,
        texts,
        workspace,
        command_executor,
        llm_executor,
        model_editor,
        file_index,
    } = options;
    // 执行器以 Rc 共享进状态闭包（iced 的状态构造器要求可重复调用；Rc 克隆即可）。
    let executor_shared: Option<Rc<dyn CommandExecutor>> = command_executor.map(Rc::from);
    let llm_shared: Option<std::sync::Arc<dyn GuiLlmExecutor>> = llm_executor;
    let model_editor_shared: Option<Rc<dyn ModelEditor>> = model_editor;
    let file_index_shared: Option<Rc<dyn FileIndex>> = file_index;

    let mut app = iced::application(
        move || {
            let mut state = State::new(
                Rc::clone(&source),
                texts.clone(),
                theme,
                executor_shared.clone(),
                llm_shared.clone(),
            );
            // 侧栏项目名与模型写面在构造后注入（State::new 签名不变，测试不动）。
            state.workspace_name = workspace.clone();
            state.model_editor = model_editor_shared.clone();
            state.file_index = file_index_shared.clone();
            state
        },
        update,
        view,
    )
    .title(|state: &State| state.title())
    .theme(|state: &State| state.theme.iced())
    .subscription(subscription)
    .window(window::Settings {
        size: WINDOW_SIZE,
        position: window::Position::Centered,
        // 图标缺失不阻止启动（取 `None` 即系统默认图标）：图标是标识，不是功能。
        icon: window::icon::from_rgba(
            WINDOW_ICON_RGBA.to_vec(),
            WINDOW_ICON_SIZE,
            WINDOW_ICON_SIZE,
        )
        .ok(),
        ..window::Settings::default()
    })
    .default_font(Font::with_name("Inter"));
    for bytes in [FONT_INTER, FONT_NOTO, FONT_JETBRAINS, FONT_SARASA] {
        app = app.font(bytes.to_vec());
    }
    app.run()
}

/// 字符是否属于 CJK 区（汉字、假名、CJK 标点与全角符号）。
fn is_cjk(ch: char) -> bool {
    matches!(
        ch as u32,
        0x2E80..=0x9FFF | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFFEF
    )
}

/// 四类文本的字体（FR-716）：比例／等宽 × 拉丁／CJK。
fn font_for(cjk: bool, mono: bool) -> Font {
    match (mono, cjk) {
        (false, false) => Font::with_name("Inter"),
        (false, true) => Font::with_name("Noto Sans CJK SC"),
        (true, false) => Font::with_name("JetBrains Mono"),
        (true, true) => Font::with_name("Sarasa Mono SC"),
    }
}

/// 按“拉丁 / CJK”切分文本，保证每段显式使用对应字体（不依赖回退顺序）。
fn split_runs(line: &str) -> Vec<(bool, &str)> {
    let mut runs: Vec<(bool, &str)> = Vec::new();
    let mut start = 0;
    let mut current: Option<bool> = None;
    for (index, ch) in line.char_indices() {
        let cjk = is_cjk(ch);
        match current {
            Some(value) if value == cjk => {}
            Some(value) => {
                runs.push((value, &line[start..index]));
                start = index;
                current = Some(cjk);
            }
            None => current = Some(cjk),
        }
    }
    if let Some(value) = current {
        runs.push((value, &line[start..]));
    }
    runs
}

/// 一行富文本：按字符类别分段指定字体。
fn rich_line(line: &str, mono: bool, size: f32) -> Element<'static, Message> {
    let spans: Vec<text::Span<'static, (), Font>> = split_runs(line)
        .into_iter()
        .map(|(cjk, run)| span(run.to_string()).font(font_for(cjk, mono)).size(size))
        .collect();
    rich_text(spans).into()
}

/// 单色富文本行（diff 着色用；字体选择与 [`rich_line`] 同源，不改字形）。
fn colored_line(line: &str, mono: bool, size: f32, color: Color) -> Element<'static, Message> {
    let spans: Vec<text::Span<'static, (), Font>> = split_runs(line)
        .into_iter()
        .map(|(cjk, run)| {
            span(run.to_string())
                .font(font_for(cjk, mono))
                .size(size)
                .color(color)
        })
        .collect();
    rich_text(spans).into()
}

/// diff 行色（T260）：`+` 增、`-` 删、其余上下文；颜色只作增强，前缀始终保留。
fn diff_line_color(theme: GuiTheme, marker: char) -> Color {
    let (added, removed) = theme.diff_colors();
    let (_, foreground) = theme.colors();
    match marker {
        '+' => added,
        '-' => removed,
        _ => foreground,
    }
}

/// RTL 行序镜像（FR-766／T279）：LTR 保持登记序，RTL 反转使逻辑首项出现在最右。
///
/// 只重排容器的子项顺序，不改元素自身样式与文本内容（字形走向与 BiDi 由系统
/// 文本栈处理）；GUI 界面不使用方向性字形，箭头翻转因此不涉及。
fn mirrored_order<T>(rtl: bool, children: Vec<T>) -> Vec<T> {
    if rtl {
        let mut ordered = children;
        ordered.reverse();
        ordered
    } else {
        children
    }
}

fn update(state: &mut State, message: Message) -> Task<Message> {
    // turn 运行中：后台线程持有连接锁——除进度/完成/窗口事件外的一切消息冻结在
    // 当前画面（任何触库操作都会阻塞等待锁，冻结优于卡死；完成后自动刷新）。
    if state.turn_running {
        match message {
            Message::ChatProgress(count, delta) => {
                state.chat_progress = count;
                append_streaming(&mut state.chat_streaming, &delta);
            }
            // 工具信号与审批必须照常处理：turn 线程可能正阻塞等待决定。
            Message::ToolSignal(signal) => apply_tool_signal(state, signal),
            // 取消必须穿透冻结门：取消经标记目录跨线程生效（不触库、不取连接锁）；
            // ADR-087 修订前该消息被冻结吞掉，运行中取消按钮实际不可达。
            Message::ChatCancel => chat_cancel(state),
            Message::ApprovalPending(request) => {
                state.approval = Some(request);
                state.approval_status = None;
            }
            Message::ApprovalDecide(answer) => decide_approval(state, answer),
            // 面板关闭不触库；打开要取数，turn 期间保持冻结（不阻塞界面）。
            Message::TogglePanels => {
                state.panels_open = false;
                state.panel_status = None;
            }
            Message::ChatDone(result) => {
                state.turn_running = false;
                state.active_request_id = None;
                state.chat_progress = 0;
                state.chat_streaming.clear();
                state.approval = None;
                state.approvals = None;
                match result {
                    Ok(summary) => {
                        state.chat_last = Some(summary);
                        state.prepare();
                    }
                    Err(err) => {
                        state.chat_last =
                            Some(format!("{}: {err}", state.texts.resolve("gui.chat.error")));
                    }
                }
            }
            Message::Resized(size) => state.size = size,
            _ => {}
        }
        return Task::none();
    }
    match message {
        Message::ContentSearchChanged(text) => {
            state.content_query = text;
            return Task::none();
        }
        Message::ContentSearchSubmit => {
            // turn 运行期间消息被 update 顶部的冻结门拦截，不会到达这里；
            // 连接锁被后台线程持有，任何触库动作都必须等待——冻结优于卡死。
            let needle = state.content_query.trim().to_string();
            if needle.is_empty() {
                state.content_status =
                    Some(state.texts.resolve("gui.content.empty_query").to_string());
                return Task::none();
            }
            match state
                .source
                .content_search(&needle, whcode_app::CONTENT_SEARCH_DEFAULT_LIMIT)
            {
                Ok(hits) => {
                    state.content_status = Some(
                        state
                            .texts
                            .resolve_fmt("gui.content.hits", &[("count", &hits.len().to_string())]),
                    );
                    state.content_hits = hits;
                }
                Err(err) => {
                    // 显式失败：数据源不支持、检索词非法或存储错误都可见。
                    state.content_hits.clear();
                    state.content_status = Some(err.message);
                }
            }
            return Task::none();
        }
        Message::SessionFilterChanged(text) => {
            state.session_filter = text;
            state.apply_session_view();
            return Task::none();
        }
        Message::SessionSortChanged(sort) => {
            state.session_sort = sort;
            state.apply_session_view();
            return Task::none();
        }
        Message::SessionArchivedToggled(on) => {
            state.session_show_archived = on;
            state.apply_session_view();
            return Task::none();
        }
        Message::ChatProgress(_, _) | Message::ChatDone(_) => {
            // turn 未在运行（完成后送达的尾消息），无状态可更新。
        }
        // 迟到的工具/审批消息：turn 已结束，不留下悬挂对话框。
        Message::ToolSignal(_) | Message::ApprovalPending(_) | Message::ApprovalDecide(_) => {}
        Message::SelectView(view) => {
            state.view = view;
            state.prepare();
            return Task::none();
        }
        // ADR-102：Tab 环序切换（-1 后退、+1 前进），到端点回绕。
        // 输入态（操作面板／对话输入）与 turn 运行期间不切换——键盘归输入控件。
        Message::CycleView(step) => {
            if state.op_mode.is_some() || state.view == ViewId::Chat || state.turn_running {
                return Task::none();
            }
            let count = ViewId::ALL.len() as i32;
            let current = ViewId::ALL
                .iter()
                .position(|item| *item == state.view)
                .unwrap_or(0) as i32;
            let next = (current + step).rem_euclid(count);
            if let Some(view) = ViewId::ALL.get(next as usize).copied() {
                state.view = view;
                state.prepare();
            }
            return Task::none();
        }
        Message::SelectSession(index) => {
            if index < state.session_ids.len() {
                state.selected = Some(index);
                state.view = ViewId::Chat;
                state.prepare();
            }
            return Task::none();
        }
        Message::NextSession => {
            if !state.session_ids.is_empty() {
                state.selected = Some(
                    state
                        .selected
                        .map_or(0, |index| (index + 1).min(state.session_ids.len() - 1)),
                );
                state.view = ViewId::Sessions;
                state.prepare();
            }
            return Task::none();
        }
        Message::PrevSession => {
            if !state.session_ids.is_empty() {
                state.selected = Some(state.selected.map_or(0, |index| index.saturating_sub(1)));
                state.view = ViewId::Sessions;
                state.prepare();
            }
            return Task::none();
        }
        Message::Refresh => {
            state.prepare();
            return Task::none();
        }
        Message::CycleTheme => {
            state.theme = state.theme.next();
            return Task::none();
        }
        Message::Resized(size) => {
            state.size = size;
            return Task::none();
        }
        Message::Key(key) => {
            // 面板打开时只接受 `s`（关闭）与 Esc（关闭）；其余键不穿透到视图切换。
            if state.panels_open {
                if key == 's' {
                    let _ = update(state, Message::TogglePanels);
                }
                return Task::none();
            }
            // 全局快捷键仅在「无文本输入场景」生效：对话视图与操作面板打开时，
            // 键盘由文本输入控件接管（数字与 j/k/r/t/q 是内容而非命令）。
            if state.op_mode.is_some() || state.view == ViewId::Chat {
                return Task::none();
            }
            match key {
                '1' | '2' | '3' | '4' | '5' | '6' | '7' | '8' => {
                    let index = (key.to_digit(10).unwrap_or(1) as usize).saturating_sub(1);
                    if let Some(view) = ViewId::ALL.get(index).copied() {
                        let _ = update(state, Message::SelectView(view));
                    }
                    return Task::none();
                }
                // ADR-092／FR-800 批 2：第 11 视图（数字键已用尽，取 `-`）。
                '-' => {
                    let _ = update(state, Message::SelectView(ViewId::Runtime));
                    return Task::none();
                }
                'j' => {
                    let _ = update(state, Message::NextSession);
                    return Task::none();
                }
                'k' => {
                    let _ = update(state, Message::PrevSession);
                    return Task::none();
                }
                'r' => {
                    let _ = update(state, Message::Refresh);
                    return Task::none();
                }
                't' => {
                    let _ = update(state, Message::CycleTheme);
                    return Task::none();
                }
                's' => {
                    let _ = update(state, Message::TogglePanels);
                    return Task::none();
                }
                // ADR-091／FR-799 读面详情键位（按视图门控，避开全局键位）。
                'e' if state.view == ViewId::Events => {
                    let index = state.event_selected.unwrap_or(0);
                    let _ = update(state, Message::EventDetail(index));
                    return Task::none();
                }
                'v' if state.view == ViewId::Spec => {
                    let _ = update(state, Message::SpecGate("validate"));
                    return Task::none();
                }
                'z' if state.view == ViewId::Spec => {
                    let _ = update(state, Message::SpecGate("analyze"));
                    return Task::none();
                }
                'o' if state.view == ViewId::Spec => {
                    let _ = update(state, Message::SpecGate("converge"));
                    return Task::none();
                }
                // ADR-092／FR-800：Spec 写面键位（与 TUI 同语义，按视图门控）。
                'n' if state.view == ViewId::Spec => {
                    let _ = update(state, Message::OpStart(OpMode::SpecNext));
                    return Task::none();
                }
                'F' if state.view == ViewId::Spec => {
                    let _ = update(
                        state,
                        Message::OpStart(OpMode::SpecForceAdvance { to: None }),
                    );
                    return Task::none();
                }
                'q' => return iced::exit(),
                _ => return Task::none(),
            }
        }
        Message::EscapeKey => {
            if state.panels_open {
                state.panels_open = false;
                state.panel_status = None;
            } else if state.op_mode.take().is_some() {
                state.op_input.clear();
                state.op_feedback = None;
            } else {
                return iced::exit();
            }
        }
        Message::ChatProviderChanged(value) => state.chat_provider = value,
        Message::ChatInputChanged(value) => state.chat_input = value,
        Message::ChatSubmit => return chat_submit(state),
        Message::ChatCancel => chat_cancel(state),
        Message::ShowModels => {
            state.model_status = match state.llm.as_ref() {
                Some(llm) => Some(match llm.model_summary() {
                    Ok(summary) => summary,
                    Err(err) => format!("llm.models 失败：{err}"),
                }),
                None => Some("llm.models: not_supported（未注入 LLM 执行器）".to_string()),
            };
            return Task::none();
        }
        // —— 添加模型（ADR-102）——
        Message::OpenModelForm => {
            state.model_form = Some(ModelDraft::default());
            state.model_form_status = None;
            return Task::none();
        }
        Message::CloseModelForm => {
            state.model_form = None;
            state.model_form_status = None;
            return Task::none();
        }
        Message::ModelFieldChanged(field, value) => {
            if let Some(draft) = state.model_form.as_mut() {
                match field {
                    ModelField::Name => draft.name = value,
                    ModelField::BaseUrl => draft.base_url = value,
                    ModelField::Model => draft.model = value,
                    ModelField::Credential => draft.credential_env = value,
                }
            }
            return Task::none();
        }
        Message::ModelProtocolChanged(value) => {
            if let Some(draft) = state.model_form.as_mut() {
                draft.protocol = value;
            }
            return Task::none();
        }
        Message::SaveModel => {
            let Some(draft) = state.model_form.clone() else {
                return Task::none();
            };
            let Some(editor) = state.model_editor.clone() else {
                state.model_form_status =
                    Some(state.texts.resolve("gui.model.no_editor").to_string());
                return Task::none();
            };
            state.model_form_status = Some(match editor.add(&draft) {
                Ok(path) => {
                    state.model_form = None;
                    state
                        .texts
                        .resolve_fmt("gui.model.saved", &[("path", &path)])
                }
                Err(err) => state
                    .texts
                    .resolve_fmt("gui.model.save_failed", &[("error", &err.to_string())]),
            });
            return Task::none();
        }
        Message::PlanToggle => {
            let Some(session) = state.selected_session() else {
                state.op_feedback = Some(state.texts.resolve("gui.chat.none").to_string());
                return Task::none();
            };
            let next = !state.snapshot.plan_active;
            execute(
                state,
                FrontendCommand::SessionPlanSet {
                    session_id: session,
                    active: next,
                },
            );
            return Task::none();
        }
        Message::OpenFilePicker => {
            if let Some(index) = state.file_index.clone() {
                state.file_picker = Some((String::new(), index.files("", 20)));
            }
            return Task::none();
        }
        Message::FileQueryChanged(query) => {
            if let Some(index) = state.file_index.clone() {
                let hits = index.files(&query, 20);
                state.file_picker = Some((query, hits));
            }
            return Task::none();
        }
        Message::InsertFile(path) => {
            if !state.chat_input.is_empty() && !state.chat_input.ends_with(' ') {
                state.chat_input.push(' ');
            }
            state.chat_input.push_str(&path);
            state.file_picker = None;
            return Task::none();
        }
        Message::CloseFilePicker => {
            state.file_picker = None;
            return Task::none();
        }
        Message::OpInputChanged(value) => state.op_input = value,
        Message::OpStart(mode) => {
            state.op_mode = Some(mode);
            state.op_input.clear();
            state.op_feedback = None;
        }
        Message::OpPreview => op_preview(state),
        Message::OpConfirm => op_confirm(state),
        Message::RuntimeSelect(index) => {
            let name = state
                .runtime_entries
                .as_ref()
                .and_then(|entries| entries.get(index))
                .map(|entry| entry.name.clone());
            if let Some(name) = name {
                state.runtime_selected = Some(index);
                state.runtime_status(&name);
            }
        }
        Message::OpCancel => {
            state.op_mode = None;
            state.op_input.clear();
            state.op_feedback = None;
        }
        Message::TaskRetry(index) => task_action(state, index, false),
        Message::TaskAbort(index) => task_action(state, index, true),
        Message::TaskCheckpoints(index) => {
            let task = state.snapshot.tasks.get(index).map(|row| row.id.clone());
            state.checkpoints = task
                .as_deref()
                .map(|task| state.source.checkpoints(task))
                .unwrap_or_default();
            state.checkpoint_task = task;
            return Task::none();
        }
        // —— ADR-091／FR-799 读面详情（有界、按需查询、无定时器）——
        Message::EventDetail(index) => {
            state.event_selected = Some(index);
            state.event_detail = Some(match state.snapshot.events.get(index) {
                None => state
                    .texts
                    .resolve("gui.detail.index_out_of_range")
                    .to_string(),
                Some(row) => {
                    let event_id = format!("E-{}", row.seq);
                    match state.source.event_detail(&event_id) {
                        Ok(summary) => truncate_detail(&summary),
                        Err(err) => {
                            format!("{}：{}", state.texts.resolve("gui.detail.failed"), err)
                        }
                    }
                }
            });
            return Task::none();
        }
        Message::SpecGate(kind) => {
            state.spec_summary = Some(match state.selected_session() {
                None => state.texts.resolve("gui.detail.no_session").to_string(),
                Some(session) => match state.source.spec_gate_summary(&session, kind) {
                    Ok(summary) => truncate_detail(&summary),
                    Err(err) => format!("{}：{}", state.texts.resolve("gui.detail.failed"), err),
                },
            });
            return Task::none();
        }
        Message::TogglePanels => {
            if state.panels_open {
                state.panels_open = false;
                state.panel_status = None;
                return Task::none();
            }
            state.panels_open = true;
            state.panel_status = None;
            refresh_panels(state);
            return Task::none();
        }
        Message::RefreshPanels => {
            if !state.turn_running {
                refresh_panels(state);
            }
            return Task::none();
        }
        Message::RevokeRule(tool) => {
            let Some(executor) = state.executor.clone() else {
                state.panel_status = Some(state.texts.resolve("gui.op.not_supported").to_string());
                return Task::none();
            };
            match executor.execute(FrontendCommand::ApprovalRuleRevoke {
                tool: tool.clone(),
                now_ms: now_ms(),
            }) {
                Ok(whcode_app::CommandOutcome::ApprovalRuleRevoked { removed, .. }) => {
                    let key = if removed {
                        "settings.panels.rules.revoked"
                    } else {
                        "settings.panels.rules.missing"
                    };
                    state.panel_status = Some(state.texts.resolve_fmt(key, &[("tool", &tool)]));
                }
                Ok(_) => {
                    state.panel_status = Some(
                        state
                            .texts
                            .resolve("settings.panels.rules.failed")
                            .to_string(),
                    );
                }
                Err(err) => {
                    state.panel_status = Some(format!(
                        "{}: {err}",
                        state.texts.resolve("settings.panels.rules.failed")
                    ));
                }
            }
            refresh_panels(state);
            return Task::none();
        }
    }
    Task::none()
}

/// 高级面板取数（准备阶段；调用方保证连接锁空闲）。
fn refresh_panels(state: &mut State) {
    state.panels = state.source.panels(state.selected_session().as_deref());
    // ADR-092／FR-800 批 3：证书状态按需读取（不探测网络、不含私钥）。
    // 数据源不支持时保持 None（面板显式提示，不伪造证书状态）。
    state.tls_status = state.source.tls_status().ok();
}

/// 工具信号 → 卡片状态（T256／FR-743）。Started 建卡（“运行中”），Finished 落终态、
/// 输出、错误码与实测时长。信号有损：丢掉的 Finished 只留下“运行中”的卡，不伪造完成。
fn apply_tool_signal(state: &mut State, signal: whcode_app::turn::TurnSignal) {
    use whcode_app::turn::TurnSignal;
    match signal {
        TurnSignal::ToolStarted {
            tool_call_id,
            tool_name,
            exec_id,
            ..
        } => {
            if state.tool_cards.len() >= MAX_TOOL_CARDS {
                state.tool_cards.remove(0);
            }
            state.tool_cards.push(ToolCard {
                tool_call_id,
                tool_name,
                exec_id,
                state: None,
                output: None,
                error_code: None,
                duration_ms: None,
            });
        }
        TurnSignal::ToolFinished {
            tool_call_id,
            exec_id,
            state: term,
            output_value,
            error_code,
            duration_ms,
            ..
        } => {
            let Some(card) = state
                .tool_cards
                .iter_mut()
                .find(|card| card.tool_call_id == tool_call_id)
            else {
                // Started 帧丢失：不为孤立的完成事件伪造工具名。
                return;
            };
            if card.exec_id != exec_id {
                return;
            }
            card.state = Some(term);
            card.output = output_value;
            card.error_code = error_code;
            card.duration_ms = Some(duration_ms);
        }
    }
}

/// 四态决定（T256／FR-745）：把答案交给等待中的 turn 线程；失败显式可见。
fn decide_approval(state: &mut State, answer: whcode_domain::approval::ApprovalAnswer) {
    let (Some(approvals), Some(request)) = (state.approvals.as_ref(), state.approval.as_ref())
    else {
        return;
    };
    let id = request.id.clone();
    match approvals.decide(&id, answer) {
        Ok(()) => {
            state.approval = None;
            state.approval_status = Some(state.texts.resolve("gui.approval.decided").to_string());
        }
        Err(err) => {
            state.approval_status = Some(format!(
                "{}: {err}",
                state.texts.resolve("gui.approval.failed")
            ));
        }
    }
}

/// 对话提交（ADR-056）：同步 `run_turn`——执行期间界面阻塞，与 TUI 同语义。
/// 对话提交（ADR-056 Phase 2）：后台线程执行 `run_turn_with_progress`，界面不冻结。
/// 对话流式缓冲上限（FR-741 有界性；与 turn 单次输出预算 1 MiB 同量级——
/// 达到上限即停止追加，块计数照常，截断点落在字符边界）。
const CHAT_STREAMING_CAP_BYTES: usize = 1024 * 1024;
/// 对话历史拉取条数（≤ `MAX_CONVERSATION_TAIL`；渲染时再按行封顶）。
const CHAT_HISTORY_MAX: usize = 50;
/// 对话历史渲染条数上限（视图有界；仅呈现最近若干条，超出部分省略）。
const CHAT_HISTORY_RENDER: usize = 20;
/// 对话正文渲染行数上限（视图有界；超限以省略行“…”标注）。
const CHAT_RENDER_MAX_LINES: usize = 400;

/// 追加流式增量到缓冲（有界：超 `CHAT_STREAMING_CAP_BYTES` 截断并忽略后续）。
fn append_streaming(buffer: &mut String, delta: &str) {
    if buffer.len() >= CHAT_STREAMING_CAP_BYTES {
        return;
    }
    buffer.push_str(delta);
    if buffer.len() > CHAT_STREAMING_CAP_BYTES {
        let mut cut = CHAT_STREAMING_CAP_BYTES;
        while !buffer.is_char_boundary(cut) {
            cut -= 1;
        }
        buffer.truncate(cut);
    }
}

/// FR-741 markdown 最小集（标题／无序列表／代码围栏／行内码）的行级投影——
/// 纯函数可测；不引入第三方渲染器（零新增依赖、渲染预算可控），最小集之外的
/// 语法按普通文本呈现（显式口径，不伪造完整 markdown 支持）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MdLineKind {
    Heading,
    Bullet,
    Text,
    Code,
}

/// 一行投影：`kind` 决定前缀与字号，`segs` 内 `bool`＝行内码（等宽字体）。
#[derive(Debug, Clone, PartialEq, Eq)]
struct MdLine {
    kind: MdLineKind,
    segs: Vec<(bool, String)>,
}

/// 行内切分：把 `` `code` `` 段与普通段分开（按反引号配对，奇数段为行内码）。
fn split_inline_code(line: &str) -> Vec<(bool, String)> {
    let mut parts = Vec::new();
    for (index, part) in line.split('`').enumerate() {
        if part.is_empty() {
            continue;
        }
        parts.push((index % 2 == 1, part.to_string()));
    }
    if parts.is_empty() {
        parts.push((false, String::new()));
    }
    parts
}

/// 把对话正文投影为行列表（FR-741 最小集；空行产出空 Text 行保结构）。
fn md_project(text: &str) -> Vec<MdLine> {
    let mut lines = Vec::new();
    let mut in_fence = false;
    for raw in text.lines() {
        if raw.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            lines.push(MdLine {
                kind: MdLineKind::Code,
                segs: vec![(true, raw.to_string())],
            });
            continue;
        }
        let trimmed = raw.trim_start();
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
            lines.push(MdLine {
                kind: MdLineKind::Heading,
                segs: split_inline_code(trimmed[hashes..].trim_start()),
            });
            continue;
        }
        if let Some(rest) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            lines.push(MdLine {
                kind: MdLineKind::Bullet,
                segs: split_inline_code(rest),
            });
            continue;
        }
        lines.push(MdLine {
            kind: MdLineKind::Text,
            segs: split_inline_code(raw),
        });
    }
    lines
}

/// 投影为视图元素（有界：最多 `CHAT_RENDER_MAX_LINES` 行，超限以“…”行标注）。
fn md_elements(text: &str) -> Vec<Element<'static, Message>> {
    let mut elements = Vec::new();
    for line in md_project(text).into_iter().take(CHAT_RENDER_MAX_LINES) {
        let size = match line.kind {
            MdLineKind::Heading => 18.0,
            _ => 14.0,
        };
        let mut spans: Vec<text::Span<'static, (), Font>> = Vec::new();
        if line.kind == MdLineKind::Bullet {
            spans.push(
                span("• ".to_string())
                    .font(font_for(false, false))
                    .size(size),
            );
        }
        if line.segs.is_empty() {
            spans.push(
                span(" ".to_string())
                    .font(font_for(false, false))
                    .size(size),
            );
        }
        for (mono, part) in line.segs {
            spans.push(span(part).font(font_for(false, mono)).size(size));
        }
        elements.push(rich_text(spans).into());
    }
    if md_project(text).len() > CHAT_RENDER_MAX_LINES {
        elements.push(rich_line("…", false, FONT_CAPTION));
    }
    elements
}

/// 进度经有界通道 `try_send` 送达（满即丢弃——进度是有损信号）；完成消息经 oneshot
/// 保证送达。执行期间 `turn_running` 冻结其余交互。
fn chat_submit(state: &mut State) -> Task<Message> {
    // 斜杠命令（ADR-102 功能对齐）：GUI 本地动作，不经模型、不写事件。
    // ZCode composer 支持 `/` 唤起命令；这里给出等价的最小命令集，全部对应既有动作。
    let typed = state.chat_input.trim().to_string();
    if let Some(rest) = typed.strip_prefix('/') {
        let name = rest.split_whitespace().next().unwrap_or_default();
        let action = match name {
            "new" => Some(Message::OpStart(OpMode::SessionNew)),
            "rename" => Some(Message::OpStart(OpMode::SessionRename)),
            "archive" => Some(Message::OpStart(OpMode::SessionArchive { archived: true })),
            "unarchive" => Some(Message::OpStart(OpMode::SessionArchive { archived: false })),
            "delete" => Some(Message::OpStart(OpMode::SessionDelete)),
            "models" => Some(Message::ShowModels),
            "theme" => Some(Message::CycleTheme),
            "help" => None,
            _ => {
                state.chat_input.clear();
                state.chat_last = Some(
                    state
                        .texts
                        .resolve_fmt("gui.chat.slash.unknown", &[("name", name)]),
                );
                return Task::none();
            }
        };
        state.chat_input.clear();
        match action {
            Some(message) => return update(state, message),
            None => {
                state.chat_last = Some(state.texts.resolve("gui.chat.slash.help").to_string());
                return Task::none();
            }
        }
    }
    let Some(llm) = state.llm.clone() else {
        state.chat_last = Some(state.texts.resolve("gui.chat.not_supported").to_string());
        return Task::none();
    };
    if state.chat_input.trim().is_empty() {
        state.chat_last = Some(state.texts.resolve("gui.chat.empty").to_string());
        return Task::none();
    }
    let Some(session) = state.selected_session() else {
        state.chat_last = Some(state.texts.resolve("gui.chat.none").to_string());
        return Task::none();
    };
    let provider = if state.chat_provider.trim().is_empty() {
        llm.profiles().into_iter().next().unwrap_or_default()
    } else {
        state.chat_provider.trim().to_string()
    };
    let prompt = std::mem::take(&mut state.chat_input);
    // 提交前最后一次读历史（连接锁仍空闲；turn 运行期间不再触库）。
    state.refresh_chat_history();
    // ADR-087 修订：request_id 提交前预生成（确定性取消），运行中存于状态供
    // Esc／取消按钮直达执行器，不再从完成摘要反解析。
    let request_id = whcode_app::llm::generate_request_id(&session, &provider, &prompt, now_ms());
    state.turn_running = true;
    state.active_request_id = Some(request_id.clone());
    state.chat_progress = 0;
    state.chat_streaming.clear();
    state.tool_cards.clear();
    state.approval = None;
    state.approval_status = None;
    let (progress_tx, progress_rx) = mpsc::channel::<Message>(8);
    // 工具信号与审批走同一条有界通道（64）：卡片丢帧可接受；审批发送失败按不可用处理。
    let (signal_tx, signal_rx) = mpsc::channel::<Message>(64);
    let approvals = std::sync::Arc::new(GuiApprovals {
        inner: whcode_app::approval::InteractiveApprovals::new(),
        tx: signal_tx.clone(),
    });
    state.approvals = Some(std::sync::Arc::clone(&approvals));
    let (done_tx, done_rx) = oneshot::channel::<Result<String, WhError>>();
    std::thread::spawn(move || {
        let progress = |count: u64, delta: &str| {
            // 有界通道 try_send（&mut self，故每次克隆 Sender 保持 Fn）；满即丢弃
            // （进度是有损信号；完成消息走 oneshot 保证送达）。
            let mut sender = progress_tx.clone();
            let _ = sender.try_send(Message::ChatProgress(count, delta.to_string()));
        };
        let observer = |signal: whcode_app::turn::TurnSignal| {
            let mut sender = signal_tx.clone();
            let _ = sender.try_send(Message::ToolSignal(signal));
        };
        let result = llm.run_with_progress(
            &session,
            &provider,
            &prompt,
            &request_id,
            &progress,
            Some(&observer),
            Some(approvals.as_ref() as &dyn whcode_app::approval::ApprovalAnswerer),
        );
        let _ = done_tx.send(result);
    });
    let done_stream = iced::futures::stream::once(async move {
        match done_rx.await {
            Ok(result) => Message::ChatDone(result),
            Err(_) => Message::ChatDone(Err(WhError::invalid_state(
                "对话完成通道关闭（后台线程提前终止）",
            ))),
        }
    });
    Task::stream(progress_rx.chain(signal_rx).chain(done_stream))
}

/// 取消正在运行的调用（ADR-087 修订：请求 ID 取提交前预生成的
/// `active_request_id`——不再从完成摘要反解析，运行中取消才真正可达）。
fn chat_cancel(state: &mut State) {
    let Some(llm) = state.llm.as_ref() else {
        state.chat_last = Some(state.texts.resolve("gui.chat.not_supported").to_string());
        return;
    };
    let Some(request_id) = state.active_request_id.clone() else {
        state.chat_last = Some(state.texts.resolve("gui.chat.no_request").to_string());
        return;
    };
    match llm.cancel(&request_id) {
        Ok(summary) => state.chat_last = Some(summary),
        Err(err) => {
            state.chat_last = Some(format!("{}: {err}", state.texts.resolve("gui.chat.error")));
        }
    }
}

/// 会话/任务/清理写命令统一入口：Capability 与确认语义由 `execute_command` 承担。
fn execute(state: &mut State, command: FrontendCommand) {
    let Some(executor) = state.executor.as_ref() else {
        state.op_feedback = Some(state.texts.resolve("gui.op.not_supported").to_string());
        return;
    };
    match executor.execute(command) {
        Ok(outcome) => {
            state.op_feedback = Some(op_outcome_text(&outcome));
            state.op_mode = None;
            state.op_input.clear();
            // 写后刷新（FR-624 手动刷新语义的对齐：本端写入后本地立即可见）。
            state.prepare();
        }
        Err(err) => {
            state.op_feedback = Some(format!("{}: {err}", state.texts.resolve("gui.op.error")))
        }
    }
}

/// 操作面板预览（破坏性命令 confirm=false；不写库、不写审计）。
fn op_preview(state: &mut State) {
    let Some(mode) = state.op_mode.clone() else {
        return;
    };
    let Some(executor) = state.executor.as_ref() else {
        state.op_feedback = Some(state.texts.resolve("gui.op.not_supported").to_string());
        return;
    };
    let Some(session) = state.selected_session() else {
        state.op_feedback = Some(state.texts.resolve("gui.chat.none").to_string());
        return;
    };
    let now = now_ms();
    let outcome = match mode {
        OpMode::SessionArchive { archived } => executor.execute(FrontendCommand::SessionArchive {
            session_id: session,
            archived,
            confirm: false,
            now_ms: now,
        }),
        OpMode::SessionDelete => executor.execute(FrontendCommand::SessionDelete {
            session_id: session,
            confirm: false,
            now_ms: now,
        }),
        OpMode::Purge { before_ms } => executor.execute(FrontendCommand::PurgePreview {
            before_ms,
            now_ms: now,
        }),
        // T393：fork／rewind／plan 无预览语义（Confirmation::None），输入即确认；
        // restore 走破坏族两段式（预览不写工作区）。
        OpMode::SessionNew
        | OpMode::SessionRename
        | OpMode::SessionFork
        | OpMode::SessionRewind
        | OpMode::SessionPlanSet
        | OpMode::SpecNext
        | OpMode::RuntimeEnable { .. }
        | OpMode::RuntimeDisable { .. }
        | OpMode::RuntimeInstall { .. }
        | OpMode::ApprovalRuleWrite
        | OpMode::EventsExport
        | OpMode::CertWrite { .. } => return,
        // ADR-092：spec.force-advance 两步——首步（to=None）把输入冻结为目标阶段；
        // 次步由 OpConfirm 读取 reason。首步不产生预览 outcome（Confirmation::ReasonRequired）。
        OpMode::SpecForceAdvance { to: None } => {
            let taken = std::mem::take(&mut state.op_input);
            if taken.trim().is_empty() {
                state.op_feedback = Some("强制放行：目标阶段不能为空".to_string());
                return;
            }
            state.op_mode = Some(OpMode::SpecForceAdvance {
                to: Some(taken.trim().to_string()),
            });
            state.op_input.clear();
            state.op_feedback = Some("强制放行：请输入理由（reason）后确认".to_string());
            return;
        }
        OpMode::SpecForceAdvance { to: Some(_) } => return,
        OpMode::SessionRestore { exec_id } => {
            if exec_id.is_empty() {
                // 首次预览：取输入并冻结执行 ID（确认阶段不再重读）。
                let taken = std::mem::take(&mut state.op_input);
                state.op_mode = Some(OpMode::SessionRestore {
                    exec_id: taken.clone(),
                });
                executor.execute(FrontendCommand::SessionRestore {
                    session_id: session,
                    exec_id: taken,
                    workspace: std::path::PathBuf::new(),
                    confirm: false,
                })
            } else {
                return;
            }
        }
    };
    match outcome {
        Ok(result) => {
            // 预览不关面板：下一步 OpConfirm 才执行（FR-623 两段式）。
            state.op_feedback = Some(op_outcome_text(&result));
        }
        Err(err) => {
            state.op_feedback = Some(format!("{}: {err}", state.texts.resolve("gui.op.error")))
        }
    }
}

/// 操作面板确认执行（confirm=true）。
fn op_confirm(state: &mut State) {
    let Some(mode) = state.op_mode.clone() else {
        return;
    };
    let now = now_ms();
    let command = match mode {
        OpMode::SessionNew => {
            let title = std::mem::take(&mut state.op_input);
            FrontendCommand::SessionNew {
                title,
                model: None,
                now_ms: now,
            }
        }
        OpMode::SessionRename => {
            let title = std::mem::take(&mut state.op_input);
            FrontendCommand::SessionRename {
                session_id: state.selected_session().unwrap_or_default(),
                title,
                now_ms: now,
            }
        }
        OpMode::SessionArchive { archived } => FrontendCommand::SessionArchive {
            session_id: state.selected_session().unwrap_or_default(),
            archived,
            confirm: true,
            now_ms: now,
        },
        OpMode::SessionDelete => FrontendCommand::SessionDelete {
            session_id: state.selected_session().unwrap_or_default(),
            confirm: true,
            now_ms: now,
        },
        OpMode::Purge { before_ms } => FrontendCommand::PurgeExecute {
            before_ms,
            confirm: true,
            now_ms: now,
        },
        OpMode::SessionFork => {
            let at_seq = std::mem::take(&mut state.op_input);
            let Ok(at_seq) = at_seq.trim().parse::<i64>() else {
                state.op_mode = None;
                state.op_feedback = Some("分叉序号必须是非负整数（0＝空分叉）".to_string());
                return;
            };
            FrontendCommand::SessionFork {
                source_session: state.selected_session().unwrap_or_default(),
                at_seq,
                title: None,
                now_ms: now,
            }
        }
        OpMode::SessionRewind => {
            let to_seq = std::mem::take(&mut state.op_input);
            let Ok(to_seq) = to_seq.trim().parse::<i64>() else {
                state.op_mode = None;
                state.op_feedback = Some("回退序号必须是非负整数（0＝会话起点）".to_string());
                return;
            };
            FrontendCommand::SessionRewind {
                session_id: state.selected_session().unwrap_or_default(),
                to_seq,
            }
        }
        OpMode::SessionRestore { exec_id } => FrontendCommand::SessionRestore {
            session_id: state.selected_session().unwrap_or_default(),
            // 执行 ID 由预览阶段冻结（FR-776：确认执行与预览同一目标）。
            exec_id,
            workspace: std::path::PathBuf::new(),
            confirm: true,
        },
        OpMode::SessionPlanSet => {
            let active_input = std::mem::take(&mut state.op_input);
            let active = match active_input.trim().to_ascii_lowercase().as_str() {
                "on" => true,
                "off" => false,
                _ => {
                    state.op_mode = None;
                    state.op_feedback = Some("计划模式：请输入 on 或 off".to_string());
                    return;
                }
            };
            FrontendCommand::SessionPlanSet {
                session_id: state.selected_session().unwrap_or_default(),
                active,
            }
        }
        // ADR-092／FR-800：spec.next 经门面推进（输入为空＝沿状态机后继）。
        OpMode::SpecNext => {
            let to_input = std::mem::take(&mut state.op_input);
            let to = if to_input.trim().is_empty() {
                None
            } else {
                Some(to_input.trim().to_string())
            };
            FrontendCommand::SpecNext {
                session_id: state.selected_session().unwrap_or_default(),
                to,
                workspace: std::path::PathBuf::new(),
            }
        }
        // ADR-092／FR-800：spec.force-advance 理由必填（FR-312），审计由门面落表。
        OpMode::SpecForceAdvance { to } => {
            let reason = std::mem::take(&mut state.op_input);
            if reason.trim().is_empty() {
                state.op_feedback = Some("强制放行：理由不能为空".to_string());
                return;
            }
            let Some(to) = to else {
                state.op_feedback = Some("强制放行：缺少目标阶段".to_string());
                return;
            };
            FrontendCommand::SpecForceAdvance {
                session_id: state.selected_session().unwrap_or_default(),
                to,
                reason: reason.trim().to_string(),
                workspace: std::path::PathBuf::new(),
            }
        }
        // ADR-092／FR-800 批 2：运行时启用／停用（name 由选中行冻结，审计由门面落表）。
        OpMode::RuntimeEnable { name } => FrontendCommand::RuntimeEnable {
            name,
            now_ms: now_ms(),
        },
        OpMode::RuntimeDisable { name } => FrontendCommand::RuntimeDisable {
            name,
            now_ms: now_ms(),
        },
        // ADR-092／FR-800 批 2：本地 payload 登记——输入 `版本|哈希|本地路径`，
        // 仅哈希比对不联网（FR-703）；三段皆必填，路径原样保留（不拼工作区）。
        OpMode::RuntimeInstall { name } => {
            let input = std::mem::take(&mut state.op_input);
            let parts: Vec<&str> = input.split('|').map(str::trim).collect();
            if parts.len() != 3 || parts.iter().any(|part| part.is_empty()) {
                state.op_feedback = Some("安装：请按“版本|哈希|本地路径”填写三段".to_string());
                return;
            }
            FrontendCommand::RuntimeInstall {
                name,
                version: parts[0].to_string(),
                hash: parts[1].to_string(),
                payload: std::path::PathBuf::from(parts[2]),
                now_ms: now_ms(),
            }
        }
        // ADR-092／FR-800 批 3：审批规则写入——`工具名|allow|deny`（Ask 由门面拒绝）。
        OpMode::ApprovalRuleWrite => {
            let input = std::mem::take(&mut state.op_input);
            let parts: Vec<&str> = input.split('|').map(str::trim).collect();
            if parts.len() != 2 || parts[0].is_empty() {
                state.op_feedback = Some("规则：请按“工具名|allow|deny”填写".to_string());
                return;
            }
            let Some(rule) = whcode_domain::approval::ApprovalRule::parse(parts[1]) else {
                state.op_feedback = Some("规则：仅 allow／deny（ask 不支持记住）".to_string());
                return;
            };
            if rule == whcode_domain::approval::ApprovalRule::Ask {
                state.op_feedback = Some("规则：ask 不支持记住，请选 allow／deny".to_string());
                return;
            }
            FrontendCommand::ApprovalRuleWrite {
                tool: parts[0].to_string(),
                rule,
                now_ms: now_ms(),
            }
        }
        // ADR-092／FR-800 批 3：事件导出——`after_seq|limit`（默认 0|1000，有界）。
        OpMode::EventsExport => {
            let input = std::mem::take(&mut state.op_input);
            let parts: Vec<&str> = input.split('|').map(str::trim).collect();
            let (after, limit) = if input.trim().is_empty() {
                (0_i64, 1000_usize)
            } else if parts.len() != 2 {
                state.op_feedback = Some("导出：请按“起始序号|条数”填写（留空取默认）".to_string());
                return;
            } else {
                let Ok(after) = parts[0].parse::<i64>() else {
                    state.op_feedback = Some("导出：起始序号须为整数".to_string());
                    return;
                };
                let Ok(limit) = parts[1].parse::<usize>() else {
                    state.op_feedback = Some("导出：条数须为正整数".to_string());
                    return;
                };
                (after, limit)
            };
            FrontendCommand::EventsExport {
                session_id: state.selected_session().unwrap_or_default(),
                after_seq: after,
                limit,
                level: whcode_domain::trace::TraceLevel::Redacted,
            }
        }
        // ADR-092／FR-800 批 3：证书写（不含私钥上传——避免凭据进入输入控件）。
        OpMode::CertWrite(action) => {
            let input = std::mem::take(&mut state.op_input);
            let now = now_ms();
            use whcode_app::commands::CertWriteAction;
            let cert_action = match action {
                CertWriteUiAction::SetModes => {
                    let parts: Vec<&str> = input.split('|').map(str::trim).collect();
                    if parts.len() != 3 {
                        state.op_feedback = Some(
                            "档位：请按“出站|入站|确认”填写（如 builtin|builtin|false）"
                                .to_string(),
                        );
                        return;
                    }
                    let Ok(outbound) = whcode_domain::TrustMode::parse(parts[0]) else {
                        state.op_feedback = Some("档位：出站须为 builtin／system".to_string());
                        return;
                    };
                    let inbound = match parts[1] {
                        "builtin" => whcode_domain::InboundMode::Builtin,
                        "external" => whcode_domain::InboundMode::External,
                        "off" => whcode_domain::InboundMode::Off,
                        _ => {
                            state.op_feedback =
                                Some("档位：入站须为 builtin／external／off".to_string());
                            return;
                        }
                    };
                    let confirm = matches!(parts[2], "true" | "yes" | "1");
                    if inbound == whcode_domain::InboundMode::Off && !confirm {
                        state.op_feedback =
                            Some("降档到明文 HTTP 须显式确认（第三段 true）".to_string());
                        return;
                    }
                    CertWriteAction::SetModes {
                        outbound,
                        inbound,
                        confirm,
                        now_ms: now,
                    }
                }
                CertWriteUiAction::WriteExtraCa => {
                    if input.trim().is_empty() {
                        state.op_feedback = Some("CA：PEM 内容不能为空".to_string());
                        return;
                    }
                    CertWriteAction::WriteCa {
                        kind: whcode_app::cert::CaKind::Extra,
                        pem: input,
                        confirm: false,
                        now_ms: now,
                    }
                }
                CertWriteUiAction::ResetBuiltinCa => CertWriteAction::ResetCa {
                    kind: whcode_app::cert::CaKind::Builtin,
                    now_ms: now,
                },
                CertWriteUiAction::RegenerateServer => CertWriteAction::RegenerateServer {
                    required_sans: Vec::new(),
                    now_ms: now,
                },
            };
            FrontendCommand::CertWrite(cert_action)
        }
    };
    execute(state, command);
}

/// 任务操作（retry/abort；按下标索引快照任务行）。
fn task_action(state: &mut State, index: usize, abort: bool) {
    let Some(task) = state.snapshot.tasks.get(index) else {
        return;
    };
    let task_id = task.id.clone();
    let now = now_ms();
    let command = if abort {
        FrontendCommand::TaskAbort {
            task_id,
            now_ms: now,
        }
    } else {
        FrontendCommand::TaskRetry {
            task_id,
            now_ms: now,
        }
    };
    execute(state, command);
}

/// `CommandOutcome` 的人可读摘要（不复制模型正文；事件已入库）。
#[allow(clippy::match_same_arms)]
fn op_outcome_text(outcome: &whcode_app::commands::CommandOutcome) -> String {
    match outcome {
        whcode_app::commands::CommandOutcome::Session(row) => {
            format!("session={} title={}", row.id, row.title)
        }
        whcode_app::commands::CommandOutcome::SessionArchivePreview {
            session,
            target_archived,
        } => format!(
            "preview: session={} -> archived={}",
            session.id, target_archived
        ),
        whcode_app::commands::CommandOutcome::SessionDeletePreview(preview) => {
            format!("preview: sessions={}", preview.counts.sessions)
        }
        whcode_app::commands::CommandOutcome::SessionDeleted(result) => {
            format!("deleted_sessions={}", result.deleted_sessions.len())
        }
        whcode_app::commands::CommandOutcome::Task(row) => {
            format!("task={} status={}", row.id, row.status.as_str())
        }
        whcode_app::commands::CommandOutcome::PurgePreview(preview) => {
            format!("preview: cutoff_ms={}", preview.cutoff_ms)
        }
        whcode_app::commands::CommandOutcome::PurgeExecuted(result) => {
            format!(
                "purged: sessions={} tasks={}",
                result.deleted.sessions, result.deleted.tasks
            )
        }
        whcode_app::commands::CommandOutcome::ApprovalRuleRevoked { tool, removed } => {
            format!("approval_rule={tool} removed={removed}")
        }
        // T391（ADR-089 批 2）：fork／rewind／restore／plan 门面结果的人可读摘要。
        whcode_app::commands::CommandOutcome::SessionForked(fork) => format!(
            "forked: {} <- {} at={} events={}",
            fork.session.id, fork.source_session, fork.forked_at_seq, fork.inherited_events
        ),
        whcode_app::commands::CommandOutcome::SessionRewound(view) => format!(
            "rewound: target={}",
            view.target_seq
                .map(|seq| seq.to_string())
                .unwrap_or("none".into())
        ),
        whcode_app::commands::CommandOutcome::PreimageRestorePreview(rows) => {
            format!("restore preview: files={}", rows.len())
        }
        whcode_app::commands::CommandOutcome::PreimageRestored { results } => {
            format!("restored: files={}", results.len())
        }
        whcode_app::commands::CommandOutcome::PlanSet(plan) => {
            format!("plan_mode={}", plan.active)
        }
        whcode_app::commands::CommandOutcome::SpecAdvanced(row) => {
            format!("session={} stage={}", row.id, row.stage)
        }
        // ADR-092／FR-800 批 2：运行时管理结果（不回显凭据，仅稳定名与状态）。
        whcode_app::commands::CommandOutcome::RuntimeStatus(view) => {
            format!(
                "runtime={} enabled={} version={}",
                view.name, view.enabled, view.version
            )
        }
        // ADR-092／FR-800 批 3：审批规则写入／事件导出（有界摘要，不回显正文）。
        whcode_app::commands::CommandOutcome::ApprovalRuleWritten { tool, rule } => {
            format!("approval_rule={tool} rule={}", rule.as_str())
        }
        whcode_app::commands::CommandOutcome::EventsExported(export) => {
            format!(
                "export: events={} states={} trace={} redacted={}",
                export.events.len(),
                export.states.len(),
                export.trace_level,
                export.redacted
            )
        }
        // ADR-092／FR-800 批 3：证书写结果只显示档位与重启提示，不回显 PEM／私钥。
        whcode_app::commands::CommandOutcome::CertWritten(status) => {
            format!(
                "cert: outbound={} inbound={} restart_required={}",
                status.outbound.mode.as_str(),
                status.inbound.mode.as_str(),
                status.restart_required
            )
        }
    }
}

/// 毫秒时钟（与 TUI 的 now_ms 同口径：系统墙钟）。
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|delta| delta.as_millis() as u64)
        .unwrap_or(0)
}

fn subscription(_: &State) -> Subscription<Message> {
    iced::event::listen_raw(handle_event)
}

/// 原始事件 → 消息。`RedrawRequested` 必须返回 `None`（否则会形成无限重绘循环）。
///
/// 字符键统一以 [`Message::Key`] 送入 update——全局快捷键的门控在 update 内按状态裁决
/// （对话视图输入中／操作面板打开时停用，由文本输入控件接管键盘）；`listen_raw` 只收
/// fn 指针，无法闭包捕获状态。
fn handle_event(
    event: iced::Event,
    _status: iced::event::Status,
    _window: window::Id,
) -> Option<Message> {
    match event {
        iced::Event::Window(window::Event::Resized(size)) => Some(Message::Resized(size)),
        iced::Event::Keyboard(iced::keyboard::Event::KeyPressed { key, modifiers, .. }) => {
            match key {
                // ADR-102：Tab 环序切换视图（Shift+Tab 反向）；输入态由 update 侧门控拦截。
                Key::Named(key::Named::Tab) => {
                    Some(Message::CycleView(if modifiers.shift() { -1 } else { 1 }))
                }
                Key::Named(key::Named::Escape) => Some(Message::EscapeKey),
                Key::Character(character) => Some(Message::Key(character.chars().next()?)),
                _ => None,
            }
        }
        _ => None,
    }
}

/// 内容检索面板（T319／FR-780）：输入框＋命中列表。与侧栏过滤的分工——过滤只在
/// 已准备的会话目录上做本地子串匹配；这里经数据源命令检索**消息正文**（FR-780），
/// 命中按最新优先、有界（默认 20 条）。输入焦点语义与侧栏过滤一致：数字键在输入框
/// 聚焦时仍会触发全局视图切换（既有行为，本批不改）。
fn content_search_panel(state: &State) -> Element<'_, Message> {
    let theme = state.theme;
    let input = text_input(
        state.texts.resolve("gui.content.placeholder"),
        state.content_query.as_str(),
    )
    .on_input(Message::ContentSearchChanged)
    .on_submit(Message::ContentSearchSubmit)
    .padding([SP_1_5, SP_2])
    .width(Length::Fill)
    .style(move |_theme, status| field_style(theme, status));
    let run = button(rich_line(
        state.texts.resolve("gui.content.run"),
        false,
        FONT_SM,
    ))
    .on_press(Message::ContentSearchSubmit)
    .padding([SP_1, SP_2])
    .style(move |_theme, status| control_button_style(theme, Emphasis::Secondary, status));
    let mut panel = column![row!(input, run).spacing(SPF_1)].spacing(SPF_1);
    if let Some(status) = &state.content_status {
        panel = panel.push(rich_line(status, false, FONT_SM));
    }
    for hit in state.content_hits.iter().take(20) {
        panel = panel.push(rich_line(
            &format!("[{}] {} {}", hit.session_id, hit.role, hit.snippet),
            false,
            FONT_SM,
        ));
    }
    panel.into()
}

/// whcode 系色值解析（FR-740 单一事实源）：从 `theme_tokens` 冻结 hex 构建 iced 颜色。
/// 冻结值合法性由 `whcode-domain::theme_tokens` 测试保证，此处只做不可失败解析的
/// 显式收口（失败即冻结值被改动，属编译期常量缺陷，panic 信息可定位）。
/// 冻结色值（`#RRGGBB`）→ iced 颜色；非法值即模块不变量失败。
fn token_color(hex: &str) -> Color {
    let (r, g, b) = whcode_domain::theme_tokens::parse_rgb(hex)
        .expect("theme_tokens 冻结色值必须为合法 #RRGGBB（模块测试保证）");
    Color::from_rgb8(r, g, b)
}

fn whcode_colors(tokens: whcode_domain::theme_tokens::ThemeTokens) -> (Color, Color) {
    (
        token_color(tokens.background),
        token_color(tokens.foreground),
    )
}

/// 解析 `#RRGGBB` 或 `rgba(r,g,b,a)` 为 iced 颜色（ADR-102 扩展语义色板用）。
///
/// 冻结值合法性由 `whcode-domain::theme_tokens` 的测试保证；此处只做不可失败收口，
/// 非法输入退化为透明（渲染不得 panic）。
fn token_color_any(value: &str) -> Color {
    if let Some((r, g, b)) = whcode_domain::theme_tokens::parse_rgb(value) {
        return Color::from_rgb8(r, g, b);
    }
    let inner = value
        .strip_prefix("rgba(")
        .and_then(|rest| rest.strip_suffix(')'))
        .unwrap_or_default();
    let parts: Vec<f32> = inner
        .split(',')
        .filter_map(|part| part.trim().parse().ok())
        .collect();
    match parts.as_slice() {
        [r, g, b, a] => Color::from_rgba(*r / 255.0, *g / 255.0, *b / 255.0, *a),
        _ => Color::TRANSPARENT,
    }
}

/// 主题的扩展语义色板；四套铁律主题（eink／amoled／terminal-green）无此表，返回 `None`
/// 并回退既有纯反色观感（ADR-102：新增不覆盖）。
fn ui_palette(theme: GuiTheme) -> Option<&'static whcode_domain::theme_tokens::UiPalette> {
    match theme {
        GuiTheme::WhcodeDay => Some(whcode_domain::theme_tokens::WHCODE_DAY.ui),
        GuiTheme::WhcodeNight => Some(whcode_domain::theme_tokens::WHCODE_NIGHT.ui),
        _ => None,
    }
}

/// 侧栏条目样式（ADR-102）：选中用 `card_selected` 圆角块、悬停用弱表面、未选中透明；
/// 文字选中为主前景、未选中为次级前景。
/// 四套铁律主题无扩展色板时保持原纯反色（不改变其观感）。
fn sidebar_style(theme: GuiTheme, selected: bool, status: button::Status) -> button::Style {
    let (background, foreground) = theme.colors();
    if ui_palette(theme).is_none() {
        return button::Style {
            background: Some(Background::Color(if selected {
                foreground
            } else {
                background
            })),
            text_color: if selected { background } else { foreground },
            border: Border::default(),
            shadow: Shadow::default(),
            snap: true,
        };
    }
    let colors = control_colors(theme);
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    let fill = if selected {
        colors.card_selected
    } else if hovered {
        colors.ghost_hover
    } else {
        Color::TRANSPARENT
    };
    button::Style {
        background: Some(Background::Color(fill)),
        text_color: if selected { foreground } else { colors.subtle },
        border: iced::border::rounded(f32::from(whcode_domain::theme_tokens::spec::RADIUS_LG)),
        shadow: Shadow::default(),
        snap: true,
    }
}

/// 控件强调级别（ADR-102 应用族视觉语言）：主操作填充、次操作描边、弱操作文字色。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Emphasis {
    /// 主操作：`primary` 填充 + `primary_foreground` 文字。
    Primary,
    /// 次操作：`secondary` 填充 + 1px 描边。
    Secondary,
    /// 弱操作：透明底，悬停出 `surface_hover`。
    Ghost,
}

/// 控件配色。whcode 系取 `theme_tokens` 冻结语义色板；四套铁律主题回退
/// 「背景＋前景」双色（边框也取前景，不引入色板外色值，守住 ADR-041 决策 2）。
struct ControlColors {
    primary: Color,
    primary_foreground: Color,
    secondary: Color,
    ghost_hover: Color,
    border: Color,
    border_hover: Color,
    text: Color,
    subtle: Color,
    card: Color,
    input: Color,
    input_focused: Color,
    selection: Color,
    tag: Color,
    card_selected: Color,
}

fn control_colors(theme: GuiTheme) -> ControlColors {
    let (background, foreground) = theme.colors();
    let tokens = match theme {
        GuiTheme::WhcodeDay => Some(whcode_domain::theme_tokens::WHCODE_DAY),
        GuiTheme::WhcodeNight => Some(whcode_domain::theme_tokens::WHCODE_NIGHT),
        _ => None,
    };
    let Some(tokens) = tokens else {
        // 四套铁律主题：层级只由反显表达（ADR-041 决策 2）。
        return ControlColors {
            primary: foreground,
            primary_foreground: background,
            secondary: background,
            ghost_hover: foreground,
            border: foreground,
            border_hover: foreground,
            text: foreground,
            subtle: foreground,
            card: background,
            input: background,
            input_focused: background,
            selection: foreground,
            tag: background,
            card_selected: foreground,
        };
    };
    let ui = tokens.ui;
    ControlColors {
        primary: token_color_any(ui.primary),
        primary_foreground: token_color_any(ui.primary_foreground),
        secondary: token_color_any(ui.secondary),
        ghost_hover: token_color_any(tokens.surface_hover),
        border: token_color_any(tokens.border),
        border_hover: token_color_any(tokens.border_hover),
        text: foreground,
        subtle: token_color_any(ui.foreground_subtle),
        card: token_color_any(tokens.card),
        input: token_color_any(ui.input),
        input_focused: token_color_any(ui.input_focused),
        selection: token_color_any(tokens.selected),
        tag: token_color_any(ui.tag),
        card_selected: token_color_any(ui.card_selected),
    }
}

/// 控件圆角（`md`：按钮与输入框；ZCode 的 `--radius-md` 同档）。
fn control_radius() -> iced::border::Radius {
    iced::border::radius(f32::from(whcode_domain::theme_tokens::spec::RADIUS_MD))
}

/// 操作按钮样式（ADR-102）：默认／悬停／按下／禁用四态都有可见反馈，
/// 让「哪里可点」由填充与描边表达，而不是靠用户记忆键位。
fn control_button_style(
    theme: GuiTheme,
    emphasis: Emphasis,
    status: button::Status,
) -> button::Style {
    let colors = control_colors(theme);
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    let disabled = matches!(status, button::Status::Disabled);
    let radius = control_radius();
    let (fill, text_color, border) = match emphasis {
        Emphasis::Primary => (
            if disabled {
                colors.secondary
            } else {
                colors.primary
            },
            if disabled {
                colors.subtle
            } else {
                colors.primary_foreground
            },
            Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius,
            },
        ),
        Emphasis::Secondary => (
            if hovered {
                colors.ghost_hover
            } else {
                colors.secondary
            },
            colors.text,
            Border {
                color: if hovered {
                    colors.border_hover
                } else {
                    colors.border
                },
                width: 1.0,
                radius,
            },
        ),
        Emphasis::Ghost => (
            if hovered {
                colors.ghost_hover
            } else {
                Color::TRANSPARENT
            },
            if disabled { colors.subtle } else { colors.text },
            Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius,
            },
        ),
    };
    button::Style {
        background: Some(Background::Color(fill)),
        text_color,
        border,
        shadow: Shadow::default(),
        snap: true,
    }
}

/// 卡片容器样式（ADR-102）：`card` 底 + 1px `border` + `xl` 圆角。
/// 不用投影：ZCode 的 elevation 在原生渲染里以边框＋底纹近似（预算中性）。
fn card_style(theme: GuiTheme) -> container::Style {
    let colors = control_colors(theme);
    container::Style {
        text_color: None,
        background: Some(Background::Color(colors.card)),
        border: Border {
            color: colors.border,
            width: 1.0,
            radius: iced::border::radius(f32::from(whcode_domain::theme_tokens::spec::RADIUS_XL)),
        },
        shadow: Shadow::default(),
        snap: true,
    }
}

/// 1px 分隔线（ADR-102 细粒度对齐）：ZCode 的顶栏有 `border-b`、状态栏有上边线，
/// iced 的 Border 四边同宽，故用 1px 空容器画单边线。
fn hairline<'a>(theme: GuiTheme) -> Element<'a, Message> {
    let color = control_colors(theme).border;
    container(column![])
        .width(Length::Fill)
        .height(Length::Fixed(1.0))
        .style(move |_theme| container::Style {
            background: Some(Background::Color(color)),
            ..container::Style::default()
        })
        .into()
}

/// 主输入区外壳样式（ADR-102）：`card` 底 + 1px 边框 + `2xl` 圆角。
///
/// ZCode DESIGN：普通输入是 `rounded-lg`，只有「主聊天输入壳」是获批的 `rounded-2xl` 例外。
fn composer_style(theme: GuiTheme) -> container::Style {
    let colors = control_colors(theme);
    container::Style {
        text_color: None,
        background: Some(Background::Color(colors.card)),
        border: Border {
            color: colors.border,
            width: 1.0,
            radius: iced::border::radius(f32::from(whcode_domain::theme_tokens::spec::RADIUS_2XL)),
        },
        shadow: Shadow::default(),
        snap: true,
    }
}

/// 侧栏面板样式（ADR-102）：`sidebar` 底 + 1px 边框 + `xl` 圆角——ZCode 的侧栏
/// 是一块独立面板，靠边框与主区分隔（四套铁律主题无边框，保持原观感）。
fn sidebar_panel_style(theme: GuiTheme) -> container::Style {
    let colors = control_colors(theme);
    let tokens = match theme {
        GuiTheme::WhcodeDay => Some(whcode_domain::theme_tokens::WHCODE_DAY),
        GuiTheme::WhcodeNight => Some(whcode_domain::theme_tokens::WHCODE_NIGHT),
        _ => None,
    };
    let Some(tokens) = tokens else {
        return container::Style {
            text_color: None,
            background: None,
            border: Border::default(),
            shadow: Shadow::default(),
            snap: true,
        };
    };
    container::Style {
        text_color: None,
        background: Some(Background::Color(token_color_any(tokens.sidebar))),
        border: Border {
            color: colors.border,
            width: 1.0,
            radius: iced::border::radius(f32::from(whcode_domain::theme_tokens::spec::RADIUS_XL)),
        },
        shadow: Shadow::default(),
        snap: true,
    }
}

/// 输入框样式（ADR-102）：`input` 底 + 1px 边框；聚焦换 `input_focused` 底与
/// `primary` 边框——聚焦位置一眼可辨。
fn field_style(theme: GuiTheme, status: text_input::Status) -> text_input::Style {
    let colors = control_colors(theme);
    let focused = matches!(status, text_input::Status::Focused { .. });
    let hovered = matches!(
        status,
        text_input::Status::Hovered | text_input::Status::Focused { is_hovered: true }
    );
    text_input::Style {
        background: Background::Color(if focused {
            colors.input_focused
        } else {
            colors.input
        }),
        border: Border {
            color: if focused {
                colors.primary
            } else if hovered {
                colors.border_hover
            } else {
                colors.border
            },
            width: 1.0,
            radius: control_radius(),
        },
        icon: colors.subtle,
        placeholder: colors.subtle,
        value: colors.text,
        selection: colors.selection,
    }
}

/// 快捷键徽章／状态标记样式（ADR-102）：`tag` 底 + `sm` 圆角，用于行内状态标记
/// （如会话的「归档」），不做键位提示——键位不再是界面元素。
fn badge_style(theme: GuiTheme) -> container::Style {
    let colors = control_colors(theme);
    container::Style {
        text_color: Some(colors.subtle),
        background: Some(Background::Color(colors.tag)),
        border: Border {
            color: colors.border,
            width: 1.0,
            radius: iced::border::radius(f32::from(whcode_domain::theme_tokens::spec::RADIUS_SM)),
        },
        shadow: Shadow::default(),
        snap: true,
    }
}

/// 侧栏分组标题（ADR-102 功能对齐）：ZCode 的分区语义（任务／项目／视图），
/// 用小号次级前景色，只做分组不做交互。
fn sidebar_section<'a>(state: &'a State, key: &'static str) -> Element<'a, Message> {
    let subtle = control_colors(state.theme).subtle;
    container(colored_line(
        state.texts.resolve(key),
        false,
        FONT_SM,
        subtle,
    ))
    .padding([SP_1, SP_2_5])
    .width(Length::Fill)
    .into()
}

/// 会话详情标签（ADR-102）：只在**选中具体会话**后出现。
///
/// 事件／diff／Spec／任务这些是「这个会话」的诊断面，不是全局导航——用户明确要求不要
/// 塞进侧栏。未选会话（空态）时整行不出现，界面只剩对话与输入。
fn session_tabs(state: &State) -> Option<Element<'_, Message>> {
    if state.selected.is_none() || state.panels_open {
        return None;
    }
    let theme = state.theme;
    let tab = |item: ViewId| -> Element<'_, Message> {
        let selected = state.view == item;
        let emphasis = if selected {
            Emphasis::Secondary
        } else {
            Emphasis::Ghost
        };
        button(rich_line(
            state.texts.resolve(item.title_key()),
            false,
            FONT_SM,
        ))
        .on_press(Message::SelectView(item))
        .padding([SP_1, SP_2])
        .style(move |_theme, status| control_button_style(theme, emphasis, status))
        .into()
    };
    let split = ViewId::ALL.len().div_ceil(2);
    let first: Vec<Element<'_, Message>> = ViewId::ALL[..split].iter().map(|i| tab(*i)).collect();
    let second: Vec<Element<'_, Message>> = ViewId::ALL[split..].iter().map(|i| tab(*i)).collect();
    Some(
        column![row(first).spacing(SPF_1), row(second).spacing(SPF_1)]
            .spacing(SPF_1)
            .into(),
    )
}

/// 相对更新时间（侧栏任务行右侧）：与 ZCode 任务行一致显示「多久前更新」。
///
/// 只在重绘时读一次系统时间——不建定时器（FR-714）。
fn relative_time_label(state: &State, then_ms: i64) -> String {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(then_ms);
    let minutes = (now_ms - then_ms).max(0) / 60_000;
    if minutes < 1 {
        return state.texts.resolve("gui.time.now").to_string();
    }
    if minutes < 60 {
        return state
            .texts
            .resolve_fmt("gui.time.minutes", &[("n", &minutes.to_string())]);
    }
    let hours = minutes / 60;
    if hours < 24 {
        return state
            .texts
            .resolve_fmt("gui.time.hours", &[("n", &hours.to_string())]);
    }
    state
        .texts
        .resolve_fmt("gui.time.days", &[("n", &(hours / 24).to_string())])
}

/// 侧栏标题截断（字符边界，避免 UTF-8 截断 panic）；侧栏宽度有限，长标题必有省略号。
fn sidebar_title(text: &str) -> String {
    const MAX: usize = 18;
    let mut out: String = text.chars().take(MAX).collect();
    if text.chars().count() > MAX {
        out.push('…');
    }
    out
}

/// 侧栏任务行（ADR-102 功能对齐）：会话标题 + 选中竖条，点击直接进入对话。
///
/// 这是 ZCode 侧栏「任务列表」的等价物——侧栏承载**真实数据**（有哪些会话），
/// 不再是内部视图的目录。
fn sidebar_session(state: &State, index: usize) -> Element<'_, Message> {
    let theme = state.theme;
    let selected = state.selected == Some(index);
    let raw = state
        .sidebar_sessions
        .get(index)
        .map(|row| row.title.as_str())
        .unwrap_or_default();
    let title = if raw.trim().is_empty() {
        state
            .sidebar_sessions
            .get(index)
            .map(|row| row.id.clone())
            .unwrap_or_else(|| format!("{index}"))
    } else {
        sidebar_title(raw)
    };
    let bar_color = if selected {
        control_colors(theme).text
    } else {
        Color::TRANSPARENT
    };
    let bar = container(column![])
        .width(Length::Fixed(2.0))
        // 固定行高：`Fill` 会向父容器要剩余高度，把每个会话行撑成一大块空白。
        .height(Length::Fixed(16.0))
        .style(move |_theme| container::Style {
            background: Some(Background::Color(bar_color)),
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: iced::border::radius(1.0_f32),
            },
            ..container::Style::default()
        });
    let stamp = state
        .sidebar_sessions
        .get(index)
        .map(|row| relative_time_label(state, row.last_active_at_ms))
        .unwrap_or_default();
    let body = row![
        bar,
        container(rich_line(&title, false, FONT_BASE)).width(Length::Fill),
        colored_line(&stamp, false, FONT_XS, control_colors(theme).subtle),
    ]
    .spacing(SPF_1_5)
    .align_y(iced::Alignment::Center);
    let body = if state
        .sidebar_sessions
        .get(index)
        .is_some_and(|row| row.archived)
    {
        body.push(
            container(rich_line(
                state.texts.resolve("gui.session.archived"),
                false,
                FONT_XS,
            ))
            .padding([1, SP_1])
            .style(move |_theme| badge_style(theme)),
        )
    } else {
        body
    };
    button(body)
        .on_press(Message::SelectSession(index))
        .width(Length::Fill)
        .padding([SP_1, SP_2_5])
        .style(move |_theme, status| sidebar_style(theme, selected, status))
        .into()
}

/// diff 渲染行上限（与数据面 `MAX_CONVERGE_DIFFERENCES=200` 同口径；此处截断只作防御）。
const MAX_RENDERED_DIFF_ROWS: usize = 200;

/// diff 视图（T260／FR-622）：unified 文本行＋3px 内嵌左色条；`+`／`-`／上下文三态
/// 着色，`+`／`-` 前缀始终保留（状态不只靠颜色，ui-design §3）。数据面已按 200 行
/// 截断并显式置 `truncated`，此处不再扩展读取面。
fn diff_body(state: &State) -> Element<'_, Message> {
    let rtl = is_rtl_locale(&state.texts.locale);
    let lines: Vec<Element<'_, Message>> = state
        .snapshot
        .diff
        .iter()
        .take(MAX_RENDERED_DIFF_ROWS)
        .map(|line| {
            let color = diff_line_color(state.theme, line.marker);
            let bar: Element<'_, Message> = container("")
                .width(Length::Fixed(3.0))
                .height(Length::Fixed(20.0))
                .style(move |_| container::Style {
                    background: Some(Background::Color(color)),
                    ..container::Style::default()
                })
                .into();
            let body = colored_line(
                &format!("{}{}", line.marker, line.text),
                true,
                FONT_BASE,
                color,
            );
            row(mirrored_order(rtl, vec![bar, body]))
                .spacing(SPF_2)
                .into()
        })
        .collect();
    if lines.is_empty() {
        return rich_line(state.texts.resolve("tui.empty"), true, FONT_LG);
    }
    scrollable(column(lines).spacing(SPF_0_5))
        .style(move |_, _| state.theme.scrollbar())
        .height(Length::Fill)
        .into()
}

/// 只读正文（既有渲染路径；Chat 视图不经过这里）。
fn read_body(state: &State, mono: bool) -> Element<'_, Message> {
    let lines = state.snapshot.render_lines_limited(500);
    if lines.is_empty() {
        rich_line(state.texts.resolve("tui.empty"), mono, FONT_LG)
    } else {
        scrollable(
            column(
                lines
                    .into_iter()
                    .map(|line| rich_line(line.as_str(), mono, FONT_LG))
                    .collect::<Vec<_>>(),
            )
            .spacing(SPF_1),
        )
        .style(move |_, _| state.theme.scrollbar())
        .height(Length::Fill)
        .into()
    }
}

/// 事件视图正文（ADR-091／FR-799）：每行可点击选中并取详情；选中态用 `> ` 前缀
/// （与 `ViewSnapshot::render_lines` 同源视觉语法，不依赖颜色）。
fn events_body(state: &State) -> Element<'_, Message> {
    let theme = state.theme;
    let lines: Vec<Element<'_, Message>> = state
        .snapshot
        .events
        .iter()
        .enumerate()
        .take(200)
        .map(|(index, row)| {
            let marker = if state.event_selected == Some(index) {
                "> "
            } else {
                "  "
            };
            let label = format!("{marker}{} {} {}", row.seq, row.kind, row.summary);
            button(rich_line(&label, false, FONT_BASE))
                .on_press(Message::EventDetail(index))
                .width(Length::Fill)
                .padding([SP_0_5, SP_1_5])
                .style(move |_theme, status| {
                    sidebar_style(theme, state.event_selected == Some(index), status)
                })
                .into()
        })
        .collect();
    if lines.is_empty() {
        rich_line(state.texts.resolve("tui.empty"), false, FONT_LG)
    } else {
        scrollable(column(lines).spacing(SPF_0_5))
            .style(move |_, _| state.theme.scrollbar())
            .height(Length::Fill)
            .into()
    }
}

/// 读面详情区（ADR-091／FR-799）：`events.show` 与 Spec 门禁摘要的 GUI 呈现。/// 数据在准备/按键时按需查询（无定时器、无常驻线程），渲染只读缓存字段。
fn detail_panel(state: &State) -> Element<'_, Message> {
    let label = state.texts.resolve("gui.detail.panel");
    let hint = match state.view {
        ViewId::Events => state.texts.resolve("gui.detail.hint.events"),
        ViewId::Spec => state.texts.resolve("gui.detail.hint.spec"),
        _ => "",
    };
    let text = match state.view {
        ViewId::Events => state.event_detail.as_deref(),
        ViewId::Spec => state.spec_summary.as_deref(),
        _ => None,
    };
    let body = match text {
        Some(value) => value.to_string(),
        None => state.texts.resolve("gui.detail.none").to_string(),
    };
    column![
        rich_line(label, false, FONT_BASE),
        rich_line(hint, false, FONT_SM),
        rich_line(body.as_str(), false, FONT_CAPTION),
    ]
    .spacing(3)
    .into()
}

/// 读面正文（ADR-091）：Events 用可点击行＋详情；Spec 用只读正文＋门禁摘要。
fn read_detail_body(state: &State) -> Element<'_, Message> {
    let main = match state.view {
        ViewId::Events => events_body(state),
        _ => read_body(state, false),
    };
    column![main, detail_panel(state)].spacing(SPF_2).into()
}

/// 对话视图（ADR-056）：剖面 + 提示词 + 发送/取消 + 最近结果。
/// 文件候选面板（ADR-102「@ 文件」）：过滤框 + 有界候选列表，点击插入提示词。
fn file_picker_panel(state: &State) -> Element<'_, Message> {
    let theme = state.theme;
    let (query, hits) = state.file_picker.clone().unwrap_or_default();
    let input = text_input(state.texts.resolve("gui.file.filter"), query.as_str())
        .on_input(Message::FileQueryChanged)
        .padding([SP_1_5, SP_2])
        .width(Length::Fill)
        .style(move |_theme, status| field_style(theme, status));
    let mut list = column![].spacing(SPF_0_5);
    if hits.is_empty() {
        list = list.push(colored_line(
            state.texts.resolve("gui.file.empty"),
            false,
            FONT_SM,
            control_colors(theme).subtle,
        ));
    }
    for hit in hits {
        let label = hit.clone();
        list = list.push(
            button(rich_line(&label, true, FONT_SM))
                .on_press(Message::InsertFile(hit))
                .width(Length::Fill)
                .padding([3, SP_2])
                .style(move |_theme, status| control_button_style(theme, Emphasis::Ghost, status)),
        );
    }
    let close = button(rich_line(
        state.texts.resolve("gui.op.cancel"),
        false,
        FONT_SM,
    ))
    .on_press(Message::CloseFilePicker)
    .padding([SP_1, SP_2_5])
    .style(move |_theme, status| control_button_style(theme, Emphasis::Ghost, status));
    container(
        column![
            row![container(input).width(Length::Fill), close]
                .spacing(SPF_1_5)
                .align_y(iced::Alignment::Center),
            scrollable(list).height(Length::Fixed(170.0)),
        ]
        .spacing(SPF_1_5),
    )
    .padding(SP_2_5)
    .width(Length::Fill)
    .style(move |_theme| card_style(theme))
    .into()
}

/// 添加模型表单（ADR-102）：名称／协议／端点／模型标识／密钥变量名，保存写
/// `profiles.json`（由装配根的 [`ModelEditor`] 落盘，GUI 自身不触文件系统）。
fn model_form_panel(state: &State) -> Element<'_, Message> {
    let theme = state.theme;
    let draft = state.model_form.clone().unwrap_or_default();
    let field = |placeholder: &str, value: &str, kind: ModelField| {
        text_input(placeholder, value)
            .on_input(move |text| Message::ModelFieldChanged(kind, text))
            .padding([SP_1_5, SP_2])
            .width(Length::Fill)
            .style(move |_theme, status| field_style(theme, status))
    };
    let protocol_options: Vec<String> = MODEL_PROTOCOLS
        .iter()
        .map(|item| (*item).to_string())
        .collect();
    let protocol = pick_list(
        protocol_options,
        Some(draft.protocol.clone()),
        Message::ModelProtocolChanged,
    )
    .padding([SP_1, SP_2])
    .text_size(FONT_SM)
    .width(Length::Fixed(130.0));
    let save = button(rich_line(
        state.texts.resolve("gui.model.save"),
        false,
        FONT_CAPTION,
    ))
    .on_press(Message::SaveModel)
    .padding([SP_1_5, SP_3])
    .style(move |_theme, status| control_button_style(theme, Emphasis::Primary, status));
    let cancel = button(rich_line(
        state.texts.resolve("gui.op.cancel"),
        false,
        FONT_CAPTION,
    ))
    .on_press(Message::CloseModelForm)
    .padding([SP_1_5, SP_3])
    .style(move |_theme, status| control_button_style(theme, Emphasis::Ghost, status));
    let mut body = column![
        rich_line(state.texts.resolve("gui.model.add"), false, FONT_BASE),
        row(mirrored_order(
            is_rtl_locale(&state.texts.locale),
            vec![
                field(
                    state.texts.resolve("gui.model.name"),
                    draft.name.as_str(),
                    ModelField::Name,
                )
                .into(),
                protocol.into(),
            ],
        ))
        .spacing(SPF_1_5),
        row(mirrored_order(
            is_rtl_locale(&state.texts.locale),
            vec![
                field(
                    state.texts.resolve("gui.model.base_url"),
                    draft.base_url.as_str(),
                    ModelField::BaseUrl,
                )
                .into(),
                field(
                    state.texts.resolve("gui.model.model_id"),
                    draft.model.as_str(),
                    ModelField::Model,
                )
                .into(),
            ],
        ))
        .spacing(SPF_1_5),
        field(
            state.texts.resolve("gui.model.credential"),
            draft.credential_env.as_str(),
            ModelField::Credential,
        ),
        row(mirrored_order(
            is_rtl_locale(&state.texts.locale),
            vec![save.into(), cancel.into()],
        ))
        .spacing(SPF_1_5),
    ]
    .spacing(SPF_1_5);
    if let Some(status) = state.model_form_status.as_deref() {
        body = body.push(colored_line(
            status,
            false,
            FONT_SM,
            control_colors(theme).subtle,
        ));
    }
    container(body)
        .padding(SP_3)
        .width(Length::Fill)
        .style(move |_theme| card_style(theme))
        .into()
}

fn chat_panel(state: &State) -> Element<'_, Message> {
    let theme = state.theme;
    // 模型：从注册表里选（ADR-102 修正）——此前是自由文本，用户既看不见有哪些模型，
    // 也无从添加。选项来自 LLM 执行器的剖面清单。
    let model_options = state.model_options();
    let model_selected = (!state.chat_provider.is_empty()).then(|| state.chat_provider.clone());
    let model_picker = pick_list(model_options, model_selected, Message::ChatProviderChanged)
        .placeholder(state.texts.resolve("gui.chat.model.choose"))
        .padding([SP_1, SP_2])
        .text_size(FONT_SM)
        .width(Length::Fixed(210.0));
    // 计划模式切换器（ADR-102：ZCode 把权限模式放在 composer 内，一眼可见当前态）。
    // 状态取自 `snapshot.plan_active`（`plan::is_active` 的只读镜像），不本地缓存，
    // 避免与命令层真实状态漂移。
    let plan_active = state.snapshot.plan_active;
    let plan_label = state.texts.resolve(if plan_active {
        "gui.plan.on"
    } else {
        "gui.plan.off"
    });
    let plan_toggle = button(rich_line(plan_label, false, FONT_SM))
        .on_press_maybe(
            (!state.turn_running && state.executor.is_some()).then_some(Message::PlanToggle),
        )
        .padding([SP_1, SP_2])
        .style(move |_theme, status| {
            control_button_style(
                theme,
                if plan_active {
                    Emphasis::Secondary
                } else {
                    Emphasis::Ghost
                },
                status,
            )
        });
    let provider_label = state.texts.resolve("gui.chat.provider");
    let input = text_input(
        state.texts.resolve("gui.chat.prompt.placeholder"),
        state.chat_input.as_str(),
    )
    .on_input(Message::ChatInputChanged)
    .on_submit(Message::ChatSubmit)
    .padding([SP_1_5, SP_2])
    .width(Length::Fill)
    .style(move |_theme, status| field_style(theme, status));
    // turn 运行中：发送禁用（面板开新 turn 会与连接锁竞争）、取消保持可用
    // （取消经标记目录跨线程生效，不触库）；状态行呈现运行进度或最近结果。
    // 发送是主操作（ADR-102 `primary` 填充），取消与模型目录为次／弱操作。
    let send = button(rich_line(
        state.texts.resolve("gui.chat.send"),
        false,
        FONT_BASE,
    ))
    .on_press_maybe((!state.turn_running && state.llm.is_some()).then_some(Message::ChatSubmit))
    .padding([SP_1_5, SP_3])
    .style(move |_theme, status| control_button_style(theme, Emphasis::Primary, status));
    let cancel = button(rich_line(
        state.texts.resolve("gui.chat.cancel"),
        false,
        FONT_BASE,
    ))
    .on_press_maybe(state.turn_running.then_some(Message::ChatCancel))
    .padding([SP_1_5, SP_3])
    .style(move |_theme, status| control_button_style(theme, Emphasis::Secondary, status));
    // 模型目录（T393／llm.models）：只读视图动作，经执行器摘要。
    let models = button(rich_line(
        state.texts.resolve("gui.chat.models"),
        false,
        FONT_BASE,
    ))
    .on_press(Message::ShowModels)
    .padding([SP_1_5, SP_3])
    .style(move |_theme, status| control_button_style(theme, Emphasis::Ghost, status));
    let pick_file = button(rich_line(
        state.texts.resolve("gui.file.at"),
        false,
        FONT_BASE,
    ))
    .on_press_maybe(
        state
            .file_index
            .is_some()
            .then_some(Message::OpenFilePicker),
    )
    .padding([SP_1_5, SP_3])
    .style(move |_theme, status| control_button_style(theme, Emphasis::Ghost, status));
    let add_model = button(rich_line(
        state.texts.resolve("gui.model.add"),
        false,
        FONT_BASE,
    ))
    .on_press(Message::OpenModelForm)
    .padding([SP_1_5, SP_3])
    .style(move |_theme, status| control_button_style(theme, Emphasis::Secondary, status));
    // 正文区（FR-741）：历史（缓存快照，仅最近 CHAT_HISTORY_RENDER 条）→
    // 运行中＝进度行＋流式增量（markdown 最小集投影，有界）；完成后＝最近结果投影；
    // 未选会话＝提示。条目格式“[role] 正文”（状态不依赖颜色，ui-design §3）。
    let skipped = state.chat_history.len().saturating_sub(CHAT_HISTORY_RENDER);
    let mut body: Vec<Element<'_, Message>> = Vec::new();
    if skipped > 0 {
        body.push(rich_line(
            &format!("…（更早 {skipped} 条已省略）"),
            false,
            FONT_SM,
        ));
    }
    let history_start = state.chat_history.len().saturating_sub(CHAT_HISTORY_RENDER);
    let mut history_text = String::new();
    for (role, content) in state.chat_history.iter().skip(history_start) {
        history_text.push_str(&format!("[{role}] {content}\n"));
    }
    if !history_text.is_empty() {
        body.extend(md_elements(history_text.as_str()));
    }
    if state.turn_running {
        // ADR-087 修订：轮头行（请求短码）——与 TUI 对话流同一词条，四端口径一致。
        if let Some(request_id) = state.active_request_id.as_deref() {
            body.push(rich_line(
                state
                    .texts
                    .resolve_fmt("tui.chat.turn.header", &[("request", request_id)])
                    .as_str(),
                false,
                FONT_CAPTION,
            ));
        }
        body.push(rich_line(
            state
                .texts
                .resolve_fmt(
                    "gui.chat.progress",
                    &[("chunks", &state.chat_progress.to_string())],
                )
                .as_str(),
            false,
            13.0,
        ));
        body.extend(md_elements(state.chat_streaming.as_str()));
    } else {
        // 空态由下方居中的欢迎块承担（ADR-102：不留一片空白，也不把提示塞进对话流）。
        if let Some(text) = state.chat_last.as_deref() {
            body.extend(md_elements(text));
        }
    }
    // 工具卡（T256／FR-743）：本次对话的工具调用状态、输出与实测时长。卡片数有界
    // （`MAX_TOOL_CARDS`），状态文本不依赖颜色（ui-design §3）。
    if !state.tool_cards.is_empty() {
        let title = state.texts.resolve("gui.tool.title");
        body.push(rich_line(title, false, FONT_CAPTION));
        for card in &state.tool_cards {
            let status = card.state.as_deref().map_or_else(
                || state.texts.resolve("gui.tool.running").to_string(),
                str::to_string,
            );
            let mut line = format!("{} [{status}]", card.tool_name);
            if let Some(ms) = card.duration_ms {
                line.push_str(&format!(" · {ms} ms"));
            }
            if let Some(code) = card.error_code.as_deref() {
                line.push_str(&format!(" · {code}"));
            }
            body.push(rich_line(&line, true, FONT_SM));
            body.push(rich_line(&card.exec_id, true, FONT_XS));
            if let Some(output) = card.output.as_deref() {
                // 输出有界：卡片只呈现前 200 字符（完整结果在事件表与 tool 消息里）。
                let shown: String = output.chars().take(200).collect();
                body.push(rich_line(&shown, false, FONT_XS));
            }
        }
    }
    // 四态审批（T256／FR-745）：turn 线程阻塞等待，决定经审批面回传。
    if let Some(request) = state.approval.as_ref() {
        let heading = state.texts.resolve("gui.approval.title");
        body.push(rich_line(heading, false, FONT_BASE));
        body.push(rich_line(
            &format!(
                "{}: {}",
                state.texts.resolve("gui.approval.tool"),
                request.tool
            ),
            false,
            13.0,
        ));
        body.push(rich_line(
            &format!(
                "{}: {}",
                state.texts.resolve("gui.approval.exec"),
                request.exec_id
            ),
            true,
            12.0,
        ));
        if let Some(reason) = request.display_reason.as_deref() {
            body.push(rich_line(
                &format!("{}: {reason}", state.texts.resolve("gui.approval.reason")),
                false,
                FONT_SM,
            ));
        }
        let answer_button = |key: &'static str, answer| {
            button(rich_line(state.texts.resolve(key), false, FONT_CAPTION))
                .padding([SP_1, SP_2])
                .on_press(Message::ApprovalDecide(answer))
        };
        body.push(
            row(mirrored_order(
                is_rtl_locale(&state.texts.locale),
                vec![
                    answer_button(
                        "gui.approval.approve_turn",
                        whcode_domain::approval::ApprovalAnswer::ApproveTurn,
                    )
                    .into(),
                    answer_button(
                        "gui.approval.approve_once",
                        whcode_domain::approval::ApprovalAnswer::Approve,
                    )
                    .into(),
                    answer_button(
                        "gui.approval.remember",
                        whcode_domain::approval::ApprovalAnswer::ApproveRemember,
                    )
                    .into(),
                    answer_button(
                        "gui.approval.reject",
                        whcode_domain::approval::ApprovalAnswer::RejectRemember,
                    )
                    .into(),
                ],
            ))
            .spacing(SPF_2)
            .into(),
        );
    }
    if let Some(status) = state.approval_status.as_deref() {
        body.push(rich_line(status, false, FONT_SM));
    }
    body.push(rich_line(
        state.texts.resolve("gui.chat.history.hint"),
        false,
        FONT_CAPTION,
    ));
    if let Some(models) = state.model_status.as_deref() {
        body.push(rich_line(models, true, FONT_SM));
    }
    // ADR-102 主界面对齐（ZCode composer 形制）：对话流占满上方，输入区固定在底部；
    // 输入框满宽，发送／取消／模型在下一行——不再把输入框挤在按钮中间。
    let subtle = control_colors(state.theme).subtle;
    // 空态判定看「有没有对话内容」，不看 body 是否为空——提示行也会进 body，
    // 用 body.is_empty() 会让欢迎块永远不出现（实测：左上角一行提示 + 大片空白）。
    let empty = state.chat_history.is_empty()
        && state.chat_last.is_none()
        && state.chat_streaming.is_empty()
        && !state.turn_running;
    let conversation: Element<'_, Message> = if empty {
        // ZCode 空态：居中欢迎语 + 一个可点入口，而不是左上角一行提示加大片空白。
        let chooser = button(rich_line(
            state.texts.resolve("gui.chat.pick_session"),
            false,
            FONT_CAPTION,
        ))
        .on_press(Message::SelectView(ViewId::Sessions))
        .padding([SP_1_5, SP_3])
        .style(move |_theme, status| control_button_style(theme, Emphasis::Secondary, status));
        container(
            column![
                rich_line(state.texts.resolve("gui.chat.new"), false, FONT_XL),
                colored_line(
                    state.texts.resolve("gui.chat.hint"),
                    false,
                    FONT_CAPTION,
                    subtle
                ),
                chooser,
            ]
            .spacing(SPF_2_5)
            .align_x(iced::Alignment::Center),
        )
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    } else {
        scrollable(column(body)).height(Length::Fill).into()
    };
    // ZCode composer：输入框与操作同在一个圆角卡片里——上为输入、下为模型与按钮，
    // 发送在最右（主操作），不再是三行分离控件。
    let composer_theme = state.theme;
    let rtl = is_rtl_locale(&state.texts.locale);
    let composer = container(
        column![
            // 第一行：输入框 + 发送（发送在输入框右端，ZCode 形态）。
            row(mirrored_order(
                rtl,
                vec![container(input).width(Length::Fill).into(), send.into(),],
            ))
            .spacing(SPF_2)
            .align_y(iced::Alignment::Center),
            // 第二行：模型选择（左）与次要动作（右）。
            row(mirrored_order(
                rtl,
                vec![
                    colored_line(provider_label, false, FONT_XS, subtle),
                    model_picker.into(),
                    // 计划模式紧随模型选择（ZCode 权限模式同位置）。
                    plan_toggle.into(),
                    container(column![]).width(Length::Fill).into(),
                    pick_file.into(),
                    models.into(),
                    add_model.into(),
                    cancel.into(),
                ],
            ))
            .spacing(SPF_1_5)
            .align_y(iced::Alignment::Center),
        ]
        .spacing(SPF_2),
    )
    .padding(SP_3)
    .width(Length::Fill)
    .style(move |_theme| composer_style(composer_theme));
    let mut stack = column![conversation];
    if state.model_form.is_some() {
        stack = stack.push(model_form_panel(state));
    }
    if state.file_picker.is_some() {
        stack = stack.push(file_picker_panel(state));
    }
    // 保存成功后的提示（表单已关闭时也要能看到结果与路径）。
    if state.model_form.is_none()
        && let Some(status) = state.model_form_status.as_deref()
    {
        stack = stack.push(colored_line(status, false, FONT_SM, subtle));
    }
    stack = stack.push(composer);
    stack.spacing(SPF_2_5).into()
}

/// 会话操作按钮行（Sessions 视图；执行器未接入时按钮显式禁用提示走反馈行）。
fn op_buttons(state: &State) -> Element<'_, Message> {
    let enabled = state.executor.is_some();
    let item = |label: &str, message: Message| -> Element<'_, Message> {
        let theme = state.theme;
        let item = button(rich_line(label, false, FONT_CAPTION))
            .padding([SP_1, SP_2])
            .style(move |_t, status| control_button_style(theme, Emphasis::Secondary, status));
        if enabled {
            item.on_press(message).into()
        } else {
            // 执行器未接入：按钮保持可见但落到 Disabled 态（不静默无响应）。
            item.into()
        }
    };
    // 两行排布（ADR-102）：十个动作在一行会超出窗口右边界（实测 Plan mode 被裁掉）。
    let rtl = is_rtl_locale(&state.texts.locale);
    let primary_row = row(mirrored_order(
        rtl,
        vec![
            item(
                state.texts.resolve("gui.op.new"),
                Message::OpStart(OpMode::SessionNew),
            ),
            item(
                state.texts.resolve("gui.op.rename"),
                Message::OpStart(OpMode::SessionRename),
            ),
            item(
                state.texts.resolve("gui.op.archive"),
                Message::OpStart(OpMode::SessionArchive { archived: true }),
            ),
            item(
                state.texts.resolve("gui.op.restore"),
                Message::OpStart(OpMode::SessionArchive { archived: false }),
            ),
            item(
                state.texts.resolve("gui.op.delete"),
                Message::OpStart(OpMode::SessionDelete),
            ),
        ],
    ))
    .spacing(SPF_1_5);
    let secondary_row = row(mirrored_order(
        rtl,
        vec![
            item(
                state.texts.resolve("gui.op.purge"),
                Message::OpStart(OpMode::Purge {
                    before_ms: purge_cutoff_ms(),
                }),
            ),
            // T393（ADR-089 批 4）：fork／rewind／restore／plan 写面对等入口。
            item(
                state.texts.resolve("gui.op.fork"),
                Message::OpStart(OpMode::SessionFork),
            ),
            item(
                state.texts.resolve("gui.op.rewind"),
                Message::OpStart(OpMode::SessionRewind),
            ),
            item(
                state.texts.resolve("gui.op.preimage"),
                Message::OpStart(OpMode::SessionRestore {
                    exec_id: String::new(),
                }),
            ),
            item(
                state.texts.resolve("gui.op.plan"),
                Message::OpStart(OpMode::SessionPlanSet),
            ),
        ],
    ))
    .spacing(SPF_1_5);
    let buttons = column![primary_row, secondary_row].spacing(SPF_1_5);
    if enabled {
        return buttons.into();
    }
    // 按钮全灰时显式说明原因（ADR-102：可点与不可点都要有可见理由，不静默无响应）。
    column![
        buttons,
        colored_line(
            state.texts.resolve("gui.op.not_supported"),
            false,
            FONT_SM,
            control_colors(state.theme).subtle,
        )
    ]
    .spacing(SPF_1)
    .into()
}

/// Spec 视图写命令按钮（ADR-092／FR-800）：阶段推进与强制放行（reason 必填）。
/// 无执行器时按钮不响应（与 op_buttons 同口径，不静默成功）。
fn spec_controls(state: &State) -> Element<'_, Message> {
    let enabled = state.executor.is_some();
    let item = |label: &str, message: Message| -> Element<'_, Message> {
        let theme = state.theme;
        let item = button(rich_line(label, false, FONT_CAPTION))
            .padding([SP_1, SP_2])
            .style(move |_t, status| control_button_style(theme, Emphasis::Secondary, status));
        if enabled {
            item.on_press(message).into()
        } else {
            item.into()
        }
    };
    row(mirrored_order(
        is_rtl_locale(&state.texts.locale),
        vec![
            item(
                state.texts.resolve("gui.op.spec.next"),
                Message::OpStart(OpMode::SpecNext),
            ),
            item(
                state.texts.resolve("gui.op.spec.force"),
                Message::OpStart(OpMode::SpecForceAdvance { to: None }),
            ),
        ],
    ))
    .spacing(SPF_1_5)
    .into()
}

/// 运行时视图写命令按钮（ADR-092／FR-800 批 2）：启用／停用／登记本地 payload。
/// 无选中行或无执行器时不响应（不静默成功）；install 需要选中行冻结 name。
fn runtime_controls(state: &State) -> Element<'_, Message> {
    let enabled = state.executor.is_some() && state.runtime_selected.is_some();
    let selected = state
        .runtime_entries
        .as_ref()
        .and_then(|entries| state.runtime_selected.and_then(|i| entries.get(i)))
        .map(|entry| entry.name.clone());
    let item = |label: String, message: Option<Message>| -> Element<'_, Message> {
        let theme = state.theme;
        let item = button(rich_line(&label, false, FONT_CAPTION))
            .padding([SP_1, SP_2])
            .style(move |_t, status| control_button_style(theme, Emphasis::Secondary, status));
        match message {
            Some(message) if enabled => item.on_press(message).into(),
            _ => item.into(),
        }
    };
    row(mirrored_order(
        is_rtl_locale(&state.texts.locale),
        vec![
            item(
                state.texts.resolve("gui.op.runtime.enable").to_string(),
                selected
                    .clone()
                    .map(|name| Message::OpStart(OpMode::RuntimeEnable { name })),
            ),
            item(
                state.texts.resolve("gui.op.runtime.disable").to_string(),
                selected
                    .clone()
                    .map(|name| Message::OpStart(OpMode::RuntimeDisable { name })),
            ),
            item(
                state.texts.resolve("gui.op.runtime.install").to_string(),
                selected.map(|name| Message::OpStart(OpMode::RuntimeInstall { name })),
            ),
        ],
    ))
    .spacing(SPF_1_5)
    .into()
}

/// 事件视图导出按钮（ADR-092／FR-800 批 3）：有界文本呈现＋手动保存；无原生
/// 保存对话框依赖（不新增 crate，ADR-026／027 不变），在 ADR-092 如实登记偏离。
fn events_controls(state: &State) -> Element<'_, Message> {
    let enabled = state.executor.is_some();
    let theme = state.theme;
    let button = button(rich_line(
        state.texts.resolve("gui.op.export"),
        false,
        FONT_CAPTION,
    ))
    .padding([SP_1, SP_2])
    .style(move |_t, status| control_button_style(theme, Emphasis::Secondary, status));
    let button = if enabled {
        button.on_press(Message::OpStart(OpMode::EventsExport))
    } else {
        button
    };
    row(mirrored_order(
        is_rtl_locale(&state.texts.locale),
        vec![button.into()],
    ))
    .into()
}

/// 运行时视图正文（ADR-092／FR-800 批 2）：声明清单行 + 选中状态；无定时器，
/// 清单在 prepare 阶段按需读取（probe=false），选中才读状态。
fn runtime_body(state: &State) -> Element<'_, Message> {
    let mut rows = column![].spacing(SPF_0_5);
    match state.runtime_entries.as_ref() {
        None => {
            rows = rows.push(rich_line(
                state.texts.resolve("tui.empty"),
                false,
                FONT_CAPTION,
            ))
        }
        Some(entries) if entries.is_empty() => {
            rows = rows.push(rich_line(
                state.texts.resolve("tui.empty"),
                false,
                FONT_CAPTION,
            ));
        }
        Some(entries) => {
            for (index, entry) in entries.iter().enumerate() {
                let marker = if state.runtime_selected == Some(index) {
                    "> "
                } else {
                    "  "
                };
                let label = format!(
                    "{marker}{} [{}] default={} {}",
                    entry.name, entry.capability, entry.default_enabled, entry.enable_requirement
                );
                rows = rows.push(
                    button(rich_line(label.as_str(), false, FONT_SM))
                        .on_press(Message::RuntimeSelect(index))
                        .padding([SP_0_5, SP_1_5]),
                );
            }
        }
    }
    if let Some(view) = state.runtime_status.as_ref() {
        rows = rows.push(rich_line(
            &format!(
                "{}: enabled={} version={} available={}",
                view.name,
                view.enabled,
                if view.version.is_empty() {
                    "-"
                } else {
                    &view.version
                },
                view.available.map_or("?".to_string(), |v| v.to_string())
            ),
            false,
            12.0,
        ));
    }
    scrollable(rows.height(Length::Fill)).into()
}

/// 清理截止：默认早于 30 天（预览面板可改天数——op_input 即天数文本）。
fn purge_cutoff_ms() -> i64 {
    now_ms() as i64 - 30 * 24 * 60 * 60 * 1000
}

/// 操作面板（op_mode 激活时渲染在正文上方）：输入 + 预览/确认/取消 + 反馈行。
fn op_panel(state: &State) -> Element<'_, Message> {
    let mode = state.op_mode.clone().unwrap_or(OpMode::SessionNew);
    let needs_input = matches!(
        mode,
        OpMode::SessionNew
            | OpMode::SessionRename
            | OpMode::Purge { .. }
            | OpMode::SessionFork
            | OpMode::SessionRewind
            | OpMode::SessionRestore { .. }
            | OpMode::SessionPlanSet
            | OpMode::SpecNext
            | OpMode::SpecForceAdvance { .. }
            | OpMode::RuntimeInstall { .. }
            | OpMode::ApprovalRuleWrite
            | OpMode::EventsExport
            | OpMode::CertWrite { .. }
    );
    let destructive = matches!(
        mode,
        OpMode::SessionArchive { .. }
            | OpMode::SessionDelete
            | OpMode::Purge { .. }
            | OpMode::SessionRestore { .. }
    );
    let placeholder = match mode {
        OpMode::SessionNew | OpMode::SessionRename => state.texts.resolve("gui.op.input.title"),
        OpMode::Purge { .. } => state.texts.resolve("gui.op.input.days"),
        _ => "",
    };
    let mut panel = column![];
    let theme = state.theme;
    if needs_input {
        panel = panel.push(
            text_input(placeholder, state.op_input.as_str())
                .on_input(Message::OpInputChanged)
                .on_submit(Message::OpConfirm)
                .padding([SP_1_5, SP_2])
                .width(Length::Fill)
                .style(move |_theme, status| field_style(theme, status)),
        );
    }
    let mut actions: Vec<Element<'_, Message>> = Vec::new();
    if destructive {
        actions.push(
            button(rich_line(
                state.texts.resolve("gui.op.preview"),
                false,
                FONT_CAPTION,
            ))
            .on_press(Message::OpPreview)
            .padding([5, SP_2_5])
            .style(move |_theme, status| control_button_style(theme, Emphasis::Secondary, status))
            .into(),
        );
        actions.push(
            button(rich_line(
                state.texts.resolve("gui.op.confirm"),
                false,
                FONT_CAPTION,
            ))
            .on_press(Message::OpConfirm)
            .padding([5, SP_2_5])
            .style(move |_theme, status| control_button_style(theme, Emphasis::Primary, status))
            .into(),
        );
    } else {
        actions.push(
            button(rich_line(
                state.texts.resolve("gui.op.confirm"),
                false,
                FONT_CAPTION,
            ))
            .on_press(Message::OpConfirm)
            .padding([5, SP_2_5])
            .style(move |_theme, status| control_button_style(theme, Emphasis::Primary, status))
            .into(),
        );
    }
    actions.push(
        button(rich_line(
            state.texts.resolve("gui.op.cancel"),
            false,
            FONT_CAPTION,
        ))
        .on_press(Message::OpCancel)
        .padding([5, SP_2_5])
        .style(move |_theme, status| control_button_style(theme, Emphasis::Ghost, status))
        .into(),
    );
    panel = panel
        .push(row(mirrored_order(is_rtl_locale(&state.texts.locale), actions)).spacing(SPF_1_5));
    if let Some(feedback) = state.op_feedback.as_deref() {
        panel = panel.push(rich_line(feedback, false, FONT_CAPTION));
    }
    panel.spacing(SPF_1_5).into()
}

/// 检查点面板（FR-748）：列表＋差分摘要；回滚未实现时显式标注“仅浏览”。
fn checkpoint_panel(state: &State) -> Element<'_, Message> {
    let mut body = column![].spacing(SPF_0_5);
    body = body.push(rich_line(
        state.texts.resolve("gui.checkpoints.title"),
        false,
        FONT_BASE,
    ));
    if state.checkpoint_task.is_none() {
        body = body.push(rich_line(
            state.texts.resolve("gui.checkpoints.hint"),
            false,
            FONT_SM,
        ));
        return body.into();
    }
    body = body.push(rich_line(
        state.texts.resolve("gui.checkpoints.degraded"),
        false,
        FONT_SM,
    ));
    if state.checkpoints.is_empty() {
        body = body.push(rich_line(
            state.texts.resolve("gui.checkpoints.empty"),
            false,
            FONT_SM,
        ));
    }
    for point in state.checkpoints.iter().take(50) {
        body = body.push(rich_line(
            &format!("#{} {} {}", point.step, point.id, point.summary),
            true,
            FONT_SM,
        ));
    }
    scrollable(body).height(Length::Fixed(160.0)).into()
}

/// 侧栏控件（FR-746）：搜索、排序三键、归档分段开关。全部本地作用于已准备的
/// 快照（不触库）；无控件时（窄窗口以外的场景）仍可用既有键盘导航。
fn session_controls(state: &State) -> Element<'_, Message> {
    let theme = state.theme;
    let search = text_input(
        state.texts.resolve("gui.session.search"),
        state.session_filter.as_str(),
    )
    .on_input(Message::SessionFilterChanged)
    .padding([SP_1_5, SP_2])
    .width(Length::Fill)
    .style(move |_theme, status| field_style(theme, status));
    let sort_button = |sort: SessionSort, key: &'static str| {
        let label = if state.session_sort == sort {
            "• "
        } else {
            ""
        };
        let emphasis = if state.session_sort == sort {
            Emphasis::Secondary
        } else {
            Emphasis::Ghost
        };
        button(rich_line(
            &format!("{label}{}", state.texts.resolve(key)),
            false,
            FONT_SM,
        ))
        .on_press(Message::SessionSortChanged(sort))
        .padding([SP_1, SP_2])
        .style(move |_theme, status| control_button_style(theme, emphasis, status))
    };
    let archived = button(rich_line(
        &format!(
            "{}: {}",
            state.texts.resolve("gui.session.archived"),
            if state.session_show_archived {
                state.texts.resolve("gui.session.on")
            } else {
                state.texts.resolve("gui.session.off")
            }
        ),
        false,
        12.0,
    ))
    .on_press(Message::SessionArchivedToggled(
        !state.session_show_archived,
    ))
    .padding([SP_1, SP_2])
    .style(move |_theme, status| control_button_style(theme, Emphasis::Ghost, status));
    row(mirrored_order(
        is_rtl_locale(&state.texts.locale),
        vec![
            search.into(),
            sort_button(SessionSort::LastActive, "gui.session.sort.recent").into(),
            sort_button(SessionSort::Created, "gui.session.sort.created").into(),
            sort_button(SessionSort::Title, "gui.session.sort.title").into(),
            archived.into(),
        ],
    ))
    .spacing(SPF_1)
    .into()
}

/// 会话目录体（FR-746）：分组标题 + 行（选中标记与既有键盘导航一致）。
fn session_body(state: &State) -> Element<'_, Message> {
    let mut lines: Vec<String> = Vec::new();
    let mut group: Option<bool> = None;
    for (index, session) in state.snapshot.sessions.iter().enumerate() {
        if state.session_show_archived && group != Some(session.archived) {
            group = Some(session.archived);
            lines.push(if session.archived {
                state
                    .texts
                    .resolve("gui.session.group.archived")
                    .to_string()
            } else {
                state.texts.resolve("gui.session.group.active").to_string()
            });
        }
        let marker = if state.selected == Some(index) {
            "> "
        } else {
            "  "
        };
        lines.push(format!(
            "{marker}{} {} [{}]",
            session.id, session.title, session.stage
        ));
    }
    if lines.is_empty() {
        return rich_line(state.texts.resolve("tui.empty"), false, FONT_LG);
    }
    scrollable(
        column(
            lines
                .into_iter()
                .map(|line| rich_line(&line, false, FONT_BASE))
                .collect::<Vec<_>>(),
        )
        .spacing(SPF_0_5),
    )
    .style(move |_, _| state.theme.scrollbar())
    .height(Length::Fill)
    .into()
}

/// 任务面板体（FR-747）：依赖缩进、重试计数、状态；卡片已在准备层按面板口径取全状态。
fn task_body(state: &State) -> Element<'_, Message> {
    let mut lines: Vec<String> = Vec::new();
    if !state.snapshot.summary.is_empty() {
        lines.push(state.snapshot.summary.clone());
    }
    for task in state.snapshot.tasks.iter().take(50) {
        let indent = if task.parent_id.is_some() {
            "  ↳ "
        } else {
            ""
        };
        let retries = if task.retry_count > 0 {
            format!(
                " · {}",
                state.texts.resolve_fmt(
                    "gui.task.retries",
                    &[("count", &task.retry_count.to_string())]
                )
            )
        } else {
            String::new()
        };
        lines.push(format!(
            "{indent}{} {} [{}]{}",
            task.id, task.title, task.status, retries
        ));
    }
    if state.snapshot.tasks.is_empty() {
        return rich_line(state.texts.resolve("tui.empty"), false, FONT_LG);
    }
    scrollable(
        column(
            lines
                .into_iter()
                .map(|line| rich_line(&line, true, FONT_BASE))
                .collect::<Vec<_>>(),
        )
        .spacing(SPF_0_5),
    )
    .style(move |_, _| state.theme.scrollbar())
    .height(Length::Fill)
    .into()
}

/// 任务行按钮（Tasks 视图）：每行 retry/abort（与 TUI 的 R/X 同语义，ADR-056）。
fn task_buttons(state: &State) -> Element<'_, Message> {
    let rtl = is_rtl_locale(&state.texts.locale);
    let theme = state.theme;
    let mut rows = column![].spacing(SPF_0_5);
    for (index, task) in state.snapshot.tasks.iter().take(50).enumerate() {
        let label = format!("{} {} {}", task.id, task.status.as_str(), task.title);
        let retry = button(rich_line(
            state.texts.resolve("gui.task.retry"),
            false,
            FONT_SM,
        ))
        .on_press(Message::TaskRetry(index))
        .padding([SP_1, SP_2])
        .style(move |_theme, status| control_button_style(theme, Emphasis::Secondary, status));
        let abort = button(rich_line(
            state.texts.resolve("gui.task.abort"),
            false,
            FONT_SM,
        ))
        .on_press(Message::TaskAbort(index))
        .padding([SP_1, SP_2])
        .style(move |_theme, status| control_button_style(theme, Emphasis::Ghost, status));
        let points = button(rich_line(
            state.texts.resolve("gui.checkpoints.open"),
            false,
            FONT_SM,
        ))
        .on_press(Message::TaskCheckpoints(index))
        .padding([SP_1, SP_2])
        .style(move |_theme, status| control_button_style(theme, Emphasis::Ghost, status));
        rows = rows.push(
            row(mirrored_order(
                rtl,
                vec![
                    rich_line(label.as_str(), false, FONT_CAPTION),
                    retry.into(),
                    abort.into(),
                    points.into(),
                ],
            ))
            .spacing(SPF_1_5),
        );
    }
    if state.snapshot.tasks.is_empty() {
        rows = rows.push(rich_line(
            state.texts.resolve("tui.empty"),
            false,
            FONT_CAPTION,
        ));
    }
    scrollable(rows.height(Length::Fill)).into()
}

fn view(state: &State) -> Element<'_, Message> {
    // ADR-102 功能对齐（ZCode 侧栏组织）：顶部「新建会话」主操作 → 「视图」分区 →
    // 「设置」分区。文字是功能名，右侧数字是该功能的数据条数，不再是内部键位。
    let action_theme = state.theme;
    let new_session = button(rich_line(
        state.texts.resolve("gui.sidebar.new_session"),
        false,
        FONT_BASE,
    ))
    .on_press_maybe(
        state
            .executor
            .is_some()
            .then_some(Message::OpStart(OpMode::SessionNew)),
    )
    .width(Length::Fill)
    .padding([SP_2, SP_2_5])
    .style(move |_theme, status| control_button_style(action_theme, Emphasis::Primary, status));

    let mut items: Vec<Element<'_, Message>> = vec![new_session.into()];
    // 「会话」分区：真实会话列表（ZCode 侧栏任务列表的等价物）。
    items.push(sidebar_section(state, "gui.sidebar.group.sessions"));
    if state.sidebar_sessions.is_empty() {
        let subtle = control_colors(state.theme).subtle;
        items.push(
            container(colored_line(
                state.texts.resolve("gui.sidebar.empty"),
                false,
                FONT_SM,
                subtle,
            ))
            .padding([SP_1, SP_2])
            .width(Length::Fill)
            .into(),
        );
    } else {
        let shown = state.sidebar_sessions.len().min(SIDEBAR_SESSION_ROWS);
        for index in 0..shown {
            items.push(sidebar_session(state, index));
        }
        if state.sidebar_sessions.len() > shown {
            let subtle = control_colors(state.theme).subtle;
            items.push(
                container(colored_line(
                    &format!("+{}", state.sidebar_sessions.len() - shown),
                    false,
                    FONT_XS,
                    subtle,
                ))
                .padding([SP_0_5, SP_2])
                .width(Length::Fill)
                .into(),
            );
        }
    }
    // 归档开关（ZCode 的「归档任务」等价物）。
    let archive_theme = state.theme;
    let archive_label = format!(
        "{}: {}",
        state.texts.resolve("gui.session.archived"),
        if state.session_show_archived {
            state.texts.resolve("gui.session.on")
        } else {
            state.texts.resolve("gui.session.off")
        }
    );
    items.push(
        button(rich_line(&archive_label, false, FONT_SM))
            .on_press(Message::SessionArchivedToggled(
                !state.session_show_archived,
            ))
            .width(Length::Fill)
            .padding([5, SP_2])
            .style(move |_theme, status| {
                control_button_style(archive_theme, Emphasis::Ghost, status)
            })
            .into(),
    );
    // 「项目」分区：当前工作区（ZCode 侧栏按项目组织任务；本仓库单工作区，故为一行）。
    items.push(sidebar_section(state, "gui.sidebar.group.project"));
    let project_theme = state.theme;
    let project_label = if state.workspace_name.is_empty() {
        "—".to_string()
    } else {
        sidebar_title(&state.workspace_name)
    };
    items.push(
        container(colored_line(
            &project_label,
            false,
            FONT_SM,
            control_colors(project_theme).subtle,
        ))
        .padding([SP_1, SP_2])
        .width(Length::Fill)
        .into(),
    );

    items.push(sidebar_section(state, "gui.sidebar.group.settings"));

    let panel_selected = state.panels_open;
    let panel_theme = state.theme;
    items.push(
        button(rich_line(
            state.texts.resolve("settings.panels.open"),
            false,
            FONT_BASE,
        ))
        .on_press(Message::TogglePanels)
        .width(Length::Fill)
        .padding([7, SP_2])
        .style(move |_theme, status| sidebar_style(panel_theme, panel_selected, status))
        .into(),
    );
    // 固定 264（ZCode 的默认与最小宽度相同，不随窗口收窄）。
    let sidebar_theme = state.theme;
    let sidebar_width = f32::from(whcode_domain::theme_tokens::spec::SIDEBAR_WIDTH);
    let sidebar = container(column![
        container(rich_line("WHCode", false, FONT_BASE)).padding([SP_1, SP_1_5]),
        column(items).spacing(SPF_0_5),
    ])
    .padding(SP_2)
    .width(Length::Fixed(sidebar_width))
    .height(Length::Fill)
    .style(move |_theme| sidebar_panel_style(sidebar_theme));

    let body: Element<'_, Message> = if state.panels_open {
        panels_panel(state)
    } else {
        match state.view {
            ViewId::Chat => chat_panel(state),
            // 台账视图（FR-787／FR-794）：四节统一文本行，只读。
            ViewId::Ledger => read_body(state, false),
            ViewId::Sessions if state.op_mode.is_some() => {
                column![op_panel(state), read_body(state, false)]
                    .spacing(SPF_2)
                    .into()
            }
            ViewId::Sessions => column![
                op_buttons(state),
                session_controls(state),
                content_search_panel(state),
                session_body(state)
            ]
            .spacing(SPF_2)
            .into(),
            ViewId::Tasks => column![
                task_buttons(state),
                task_body(state),
                checkpoint_panel(state)
            ]
            .spacing(SPF_2)
            .into(),
            ViewId::Diff => diff_body(state),
            // ADR-091／FR-799：Events 支持选中详情；Spec 支持门禁摘要。
            ViewId::Events if state.op_mode.is_some() => column![
                op_panel(state),
                events_controls(state),
                read_detail_body(state)
            ]
            .spacing(SPF_2)
            .into(),
            ViewId::Events => column![events_controls(state), read_detail_body(state)]
                .spacing(SPF_2)
                .into(),
            // ADR-092／FR-800：Spec 视图接入阶段推进与强制放行写面。
            ViewId::Spec if state.op_mode.is_some() => column![
                op_panel(state),
                spec_controls(state),
                read_detail_body(state)
            ]
            .spacing(SPF_2)
            .into(),
            ViewId::Spec => column![spec_controls(state), read_detail_body(state)]
                .spacing(SPF_2)
                .into(),
            // ADR-092／FR-800 批 2：运行时视图接入清单、状态与写面按钮。
            ViewId::Runtime if state.op_mode.is_some() => column![
                op_panel(state),
                runtime_controls(state),
                runtime_body(state)
            ]
            .spacing(SPF_2)
            .into(),
            ViewId::Runtime => column![runtime_controls(state), runtime_body(state)]
                .spacing(SPF_2)
                .into(),
            _ => read_body(state, false),
        }
    };

    // 顶栏（ZCode 形制）：左侧当前会话标题（无会话则「开始新对话」），右侧少量动作；
    // 不再有视图标签行与 KPI 条——主区只讲当前对话这一件事。
    let main_theme = state.theme;
    let subtle = control_colors(main_theme).subtle;
    let title_text = state.current_session_title();
    let meta_text = state.current_session_meta();
    let mut title_col = column![rich_line(&title_text, false, FONT_XL)].spacing(SPF_0_5);
    if !meta_text.is_empty() {
        title_col = title_col.push(colored_line(&meta_text, false, FONT_CAPTION, subtle));
    }

    let mut actions: Vec<Element<'_, Message>> = Vec::new();
    // 选中会话后的行级动作（ZCode 任务行的重命名／归档／删除菜单，这里放在顶栏，
    // 因为 iced 没有右键菜单；动作与 ZCode 同名同义）。
    if state.selected.is_some() && !state.panels_open {
        let quick = |key: &'static str, message: Message| -> Element<'_, Message> {
            button(rich_line(state.texts.resolve(key), false, FONT_SM))
                .on_press(message)
                .padding([SP_1, SP_2])
                .style(move |_theme, status| {
                    control_button_style(main_theme, Emphasis::Ghost, status)
                })
                .into()
        };
        actions.push(quick(
            "gui.op.rename",
            Message::OpStart(OpMode::SessionRename),
        ));
        actions.push(quick(
            "gui.op.archive",
            Message::OpStart(OpMode::SessionArchive { archived: true }),
        ));
        actions.push(quick(
            "gui.op.delete",
            Message::OpStart(OpMode::SessionDelete),
        ));
    }
    if state.view != ViewId::Chat {
        // 进入诊断视图后必须能一键回到对话（否则用户会卡在诊断页）。
        actions.push(
            button(rich_line(
                state.texts.resolve("gui.chat.back"),
                false,
                FONT_CAPTION,
            ))
            .on_press(Message::SelectView(ViewId::Chat))
            .padding([5, SP_2_5])
            .style(move |_theme, status| {
                control_button_style(main_theme, Emphasis::Secondary, status)
            })
            .into(),
        );
    }
    // 顶栏右侧常驻「设置」入口（ZCode 也在顶栏给设置）。
    actions.push(
        button(rich_line(
            state.texts.resolve("gui.sidebar.group.settings"),
            false,
            FONT_CAPTION,
        ))
        .on_press(Message::TogglePanels)
        .padding([5, SP_2_5])
        .style(move |_theme, status| control_button_style(main_theme, Emphasis::Ghost, status))
        .into(),
    );
    let header = row(mirrored_order(
        is_rtl_locale(&state.texts.locale),
        std::iter::once(title_col.width(Length::Fill).into())
            .chain(actions)
            .collect(),
    ))
    .spacing(SPF_2)
    .align_y(iced::Alignment::Center);

    let body = match session_tabs(state) {
        Some(tabs) => column![tabs, body].spacing(SPF_2_5).into(),
        None => body,
    };
    let card = container(
        column![header, hairline(main_theme), body]
            .spacing(SPF_3)
            .height(Length::Fill),
    )
    .padding(SP_4)
    .width(Length::Fill)
    .height(Length::Fill)
    .style(move |_theme| card_style(main_theme));

    let main = column![
        card,
        hairline(main_theme),
        colored_line(state.footer().as_str(), false, FONT_SM, subtle),
    ]
    .spacing(SPF_1_5)
    .width(Length::Fill)
    .height(Length::Fill);

    // 外壳行序镜像（FR-766／T279）：RTL 时导航栏在右、主面板在左。
    let shell = if is_rtl_locale(&state.texts.locale) {
        row![main, sidebar].spacing(SPF_2)
    } else {
        row![sidebar, main].spacing(SPF_2)
    };
    container(shell)
        .padding(SP_2)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// 子代理状态词条键（稳定码 → 文本层）。
fn subagent_state_key(state: SubagentState) -> &'static str {
    match state {
        SubagentState::Running => "settings.panels.state.running",
        SubagentState::Ok => "settings.panels.state.ok",
        SubagentState::Failed => "settings.panels.state.failed",
    }
}

/// MCP 状态词条键（配置态与观测态分开：Ok 表示最近一次调用成功）。
fn mcp_state_key(status: McpStatus) -> &'static str {
    match status {
        McpStatus::Configured => "settings.panels.mcp.configured",
        McpStatus::Ok => "settings.panels.mcp.observed_ok",
        McpStatus::Disabled => "settings.panels.mcp.disabled",
        McpStatus::Running => "settings.panels.state.running",
        McpStatus::Failed => "settings.panels.state.failed",
    }
}

/// 高级面板（T258／FR-749／750／751）：四段只读信息 + 规则撤销。
///
/// 帧路径只消费 [`PanelsSnapshot`]（准备阶段生成）；不查询、不序列化、不 spawn。
fn panels_panel(state: &State) -> Element<'_, Message> {
    let rtl = is_rtl_locale(&state.texts.locale);
    let empty = || {
        rich_line(
            state.texts.resolve("settings.panels.empty"),
            false,
            FONT_CAPTION,
        )
    };
    let heading = |key: &'static str| rich_line(state.texts.resolve(key), false, FONT_LG);
    let mut body = column![].spacing(SPF_1_5);

    body = body.push(
        button(rich_line(
            state.texts.resolve("settings.panels.refresh"),
            false,
            FONT_CAPTION,
        ))
        .padding([SP_0_5, SP_2])
        .on_press(Message::RefreshPanels),
    );
    if let Some(status) = state.panel_status.as_deref() {
        body = body.push(rich_line(status, false, FONT_CAPTION));
    }

    // 子代理运行（FR-749）：状态 + 产出摘要 + 终止原因/轮数/时长。
    body = body.push(heading("settings.panels.subagents.title"));
    if state.panels.subagents.is_empty() {
        body = body.push(empty());
    } else {
        for run in state.panels.subagents.iter().take(32) {
            let mut line = format!(
                "{} · {}",
                run.exec_id,
                state.texts.resolve(subagent_state_key(run.state))
            );
            if let Some(reason) = run.termination.as_deref() {
                line.push_str(&format!(" · {reason}"));
            }
            if let Some(iterations) = run.iterations {
                line.push_str(" · ");
                line.push_str(&state.texts.resolve_fmt(
                    "settings.panels.subagents.iterations",
                    &[("n", &iterations.to_string())],
                ));
            }
            if let Some(ms) = run.duration_ms {
                line.push_str(&format!(" · {ms} ms"));
            }
            if let Some(code) = run.error_code.as_deref() {
                line.push_str(&format!(" · {code}"));
            }
            body = body.push(rich_line(&line, true, FONT_SM));
            if let Some(summary) = run.output_summary.as_deref() {
                body = body.push(rich_line(summary, false, FONT_SM));
            }
        }
    }

    // 技能库（FR-750）：名称 — 描述；可达性问题逐条列出。
    body = body.push(heading("settings.panels.skills.title"));
    if state.panels.skills.is_empty() {
        body = body.push(empty());
    } else {
        for skill in state.panels.skills.iter().take(32) {
            body = body.push(rich_line(
                &format!("{} — {}", skill.name, skill.description),
                false,
                FONT_CAPTION,
            ));
            body = body.push(rich_line(&skill.path, true, FONT_XS));
            if !skill.issues.is_empty() {
                body = body.push(rich_line(
                    &state.texts.resolve_fmt(
                        "settings.panels.skills.issues",
                        &[("count", &skill.issues.len().to_string())],
                    ),
                    false,
                    FONT_XS,
                ));
            }
        }
    }
    for warning in state.panels.skill_warnings.iter().take(8) {
        body = body.push(rich_line(warning, false, FONT_XS));
    }

    // MCP 服务器（FR-750）：配置态 + 超时/输出上限 + 事件窗口内观测到的工具。
    body = body.push(heading("settings.panels.mcp.title"));
    if let Some(error) = state.panels.mcp_error.as_deref() {
        body = body.push(rich_line(error, false, FONT_SM));
    } else if !state.panels.mcp_configured {
        body = body.push(rich_line(
            state.texts.resolve("settings.panels.mcp.unconfigured"),
            false,
            FONT_CAPTION,
        ));
    } else if state.panels.mcp.is_empty() {
        body = body.push(empty());
    } else {
        for server in state.panels.mcp.iter().take(8) {
            let enabled = if server.enabled { "" } else { " · off" };
            body = body.push(rich_line(
                &format!(
                    "{} · {}{} · {} · {}",
                    server.name,
                    server.transport,
                    enabled,
                    state.texts.resolve(mcp_state_key(server.status)),
                    state.texts.resolve_fmt(
                        "settings.panels.mcp.timeout",
                        &[
                            ("ms", &server.timeout_ms.to_string()),
                            ("bytes", &server.max_output_bytes.to_string()),
                        ],
                    ),
                ),
                true,
                12.0,
            ));
            if !server.tools.is_empty() {
                body = body.push(rich_line(
                    &state.texts.resolve_fmt(
                        "settings.panels.mcp.tools",
                        &[("tools", &server.tools.join(", "))],
                    ),
                    false,
                    FONT_XS,
                ));
            }
        }
    }

    // 审批规则（FR-751／ADR-092 批 3）：tokens 来自本地表；撤销即写 `permission_change` 审计；
    // 写入经 OpMode::ApprovalRuleWrite（工具名|allow|deny，ask 不支持记住）。
    body = body.push(heading("settings.panels.rules.title"));
    body = body.push(
        button(rich_line(
            state.texts.resolve("settings.approvals.save"),
            false,
            FONT_SM,
        ))
        .on_press(Message::OpStart(OpMode::ApprovalRuleWrite))
        .padding([SP_0_5, SP_1_5]),
    );
    if let Some(error) = state.panels.error.as_deref()
        && state.panels.rules.is_empty()
    {
        body = body.push(rich_line(error, false, FONT_SM));
    } else if state.panels.rules.is_empty() {
        body = body.push(rich_line(
            state.texts.resolve("settings.approvals.empty"),
            false,
            FONT_CAPTION,
        ));
    } else {
        for rule in &state.panels.rules {
            let label = format!("{} · {}", rule.tool, rule.rule.as_str());
            let revoke_theme = state.theme;
            let revoke = button(rich_line(
                state.texts.resolve("settings.approvals.revoke"),
                false,
                FONT_SM,
            ))
            .padding([SP_1, SP_2])
            .on_press(Message::RevokeRule(rule.tool.clone()))
            .style(move |_theme, status| {
                control_button_style(revoke_theme, Emphasis::Ghost, status)
            });
            body = body.push(
                row(mirrored_order(
                    rtl,
                    vec![rich_line(label.as_str(), true, FONT_CAPTION), revoke.into()],
                ))
                .spacing(SPF_2),
            );
        }
    }

    // 证书库（ADR-092／FR-800 批 3，ADR-040 决策 4 修订）：按需读状态（不含私钥），
    // 写操作经门面，私钥上传不在 GUI 提供（避免凭据进入输入控件）。
    body = body.push(heading("settings.panels.cert.title"));
    match state.tls_status.as_ref() {
        None => body = body.push(rich_line(state.texts.resolve("tui.empty"), false, FONT_SM)),
        Some(tls) => {
            let summary = format!(
                "{} / {} · cert={} key={} · restart={}",
                tls.outbound.mode.as_str(),
                tls.inbound.mode.as_str(),
                tls.inbound.cert_present,
                tls.inbound.key_present,
                tls.restart_required
            );
            body = body.push(rich_line(summary.as_str(), false, FONT_SM));
        }
    }
    for (label, action) in [
        ("settings.panels.cert.modes", CertWriteUiAction::SetModes),
        (
            "settings.panels.cert.ca.write",
            CertWriteUiAction::WriteExtraCa,
        ),
        (
            "settings.panels.cert.ca.reset",
            CertWriteUiAction::ResetBuiltinCa,
        ),
        (
            "settings.panels.cert.regenerate",
            CertWriteUiAction::RegenerateServer,
        ),
    ] {
        body = body.push(
            button(rich_line(state.texts.resolve(label), false, FONT_SM))
                .on_press(Message::OpStart(OpMode::CertWrite(action)))
                .padding([SP_0_5, SP_1_5]),
        );
    }

    scrollable(body)
        .style(move |_, _| state.theme.scrollbar())
        .height(Length::Fill)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ADR-102 校准守卫：GUI 间距必须落在 `spec::SPACE_SCALE`，或为登记在案的亚网格微调。
    ///
    /// 1／3／5／7 是图标基线对齐与 1px 描边补偿的亚像素微调（spec 注释已登记），
    /// 其余取值一律须走 `SP_*` 常量——否则改 spec 不会生效，重演字号那类漂移。
    #[test]
    fn spacing_uses_spec_scale_or_registered_micro_adjustments() {
        use whcode_domain::theme_tokens::spec;
        assert_eq!(
            spec::SPACE_SCALE,
            &[2, 4, 6, 8, 10, 12, 16, 24],
            "间距档位变更须同步本守卫与 CSS 的 --space-* 变量"
        );
        const MICRO: [u16; 4] = [1, 3, 5, 7];
        let source = include_str!("lib.rs");
        for (index, line) in source.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") || trimmed.starts_with("///") {
                continue;
            }
            for call in [".padding(", ".spacing("] {
                let Some(start) = line.find(call) else {
                    continue;
                };
                let rest = &line[start + call.len()..];
                let Some(end) = rest.find(')') else { continue };
                for arg in rest[..end].split(',') {
                    let arg = arg.trim().trim_start_matches('[').trim_end_matches(']');
                    // 常量引用（SP_x／SPF_x）里的数字不是字面量，跳过。
                    if arg.starts_with("SP_") || arg.starts_with("SPF_") {
                        continue;
                    }
                    let Ok(value) = arg.parse::<u16>() else {
                        continue;
                    };
                    assert!(
                        spec::SPACE_SCALE.contains(&value) || MICRO.contains(&value),
                        "第 {} 行间距 {value} 不在 SPACE_SCALE 或微调白名单：{line}",
                        index + 1
                    );
                }
            }
        }
    }

    /// ADR-102 校准守卫：GUI 字号必须全部来自 spec 七级别名常量，不得回潮为字面量。
    ///
    /// 校准前散落 11／15／20 三个不在七级内的值，且 62 处为硬编码——改 spec 不生效。
    /// 本测试在编译期常量上做**值域断言**，并静态扫描源码禁止字号字面量（含游离值）。
    #[test]
    fn font_sizes_come_only_from_spec_scale() {
        use whcode_domain::theme_tokens::spec;
        let scale = [
            f32::from(spec::UI_FONT_2XS),
            f32::from(spec::UI_FONT_XS),
            f32::from(spec::UI_FONT_SM),
            f32::from(spec::UI_FONT_CAPTION),
            f32::from(spec::UI_FONT_BASE),
            f32::from(spec::UI_FONT_LG),
            f32::from(spec::UI_FONT_XL),
        ];
        for value in [FONT_XS, FONT_SM, FONT_CAPTION, FONT_BASE, FONT_LG, FONT_XL] {
            assert!(
                scale.contains(&value),
                "GUI 字号 {value} 不在 spec 七级内：{scale:?}"
            );
        }
        // 静态扫描：rich_line／colored_line 的字号位与 text_size 不得出现小数字面量。
        let source = include_str!("lib.rs");
        for (index, line) in source.lines().enumerate() {
            if line.trim_start().starts_with("//") || line.contains("assert") {
                continue;
            }
            let suspicious = (line.contains("rich_line(") || line.contains("colored_line("))
                && line
                    .split(',')
                    .any(|part| part.trim().trim_end_matches(')').parse::<f32>().is_ok());
            assert!(
                !suspicious,
                "第 {} 行出现字号字面量，须改用 FONT_* 常量：{line}",
                index + 1
            );
        }
        // 跨行形态：字号单独成行（rich_line 后换行、再写 false、再写字号、再闭合），
        // 按行扫描会漏——向上回看 6 行是否处于 rich_line／colored_line 调用中。
        let lines: Vec<&str> = source.lines().collect();
        for (index, line) in lines.iter().enumerate() {
            let token = line.trim().trim_end_matches(',');
            if token.parse::<f32>().is_err() {
                continue;
            }
            let context = lines[index.saturating_sub(6)..index].join(" ");
            assert!(
                !(context.contains("rich_line(") || context.contains("colored_line(")),
                "第 {} 行出现跨行字号字面量 {token}，须改用 FONT_* 常量",
                index + 1
            );
        }
    }

    /// RTL 行序镜像（FR-766／T279）：LTR 保持登记序；RTL 反转；空行序恒空。
    #[test]
    fn mirrored_order_reverses_children_only_in_rtl() {
        assert_eq!(mirrored_order(false, vec![1, 2, 3]), vec![1, 2, 3]);
        assert_eq!(mirrored_order(true, vec![1, 2, 3]), vec![3, 2, 1]);
        assert_eq!(mirrored_order(false, Vec::<u8>::new()), Vec::<u8>::new());
        assert_eq!(mirrored_order(true, Vec::<u8>::new()), Vec::<u8>::new());
        assert_eq!(mirrored_order(true, vec!["唯一"]), vec!["唯一"]);
    }

    /// 方向判定随会话语言（FR-766／ADR-062）：阿拉伯语／乌尔都语开镜像，其余保持。
    #[test]
    fn gui_direction_follows_locale() {
        assert!(is_rtl_locale("ar"));
        assert!(is_rtl_locale("ur-PK"));
        assert!(!is_rtl_locale("zh-CN"));
        assert!(!is_rtl_locale("en"));
    }

    #[test]
    fn theme_names_round_trip_and_unknown_is_rejected() {
        for theme in [
            GuiTheme::EinkDay,
            GuiTheme::EinkNight,
            GuiTheme::AmoledNight,
            GuiTheme::TerminalGreen,
            GuiTheme::WhcodeDay,
            GuiTheme::WhcodeNight,
        ] {
            assert_eq!(GuiTheme::parse(theme.name()), Some(theme));
        }
        assert_eq!(GuiTheme::parse("solarized"), None);
    }

    #[test]
    fn theme_colors_are_exact() {
        assert_eq!(GuiTheme::EinkDay.colors(), (Color::WHITE, Color::BLACK));
        assert_eq!(GuiTheme::EinkNight.colors(), (Color::BLACK, Color::WHITE));
        let (background, foreground) = GuiTheme::AmoledNight.colors();
        assert_eq!(background, Color::BLACK);
        assert!((foreground.r - 0.902).abs() < f32::EPSILON);
        assert_eq!(foreground.r, foreground.g);
        assert_eq!(foreground.g, foreground.b);
        // terminal-green：OLED 兼容纯黑底 + 荧光绿字（#00FF00，绿通道满幅）。
        let (background, foreground) = GuiTheme::TerminalGreen.colors();
        assert_eq!(background, Color::BLACK);
        assert_eq!(foreground.r, 0.0);
        assert!((foreground.g - 1.0).abs() < f32::EPSILON);
        assert_eq!(foreground.b, 0.0);
        // whcode 系：从 theme_tokens 冻结值解析（FR-739/740，2026-11-06 用户批准）。
        let (background, foreground) = GuiTheme::WhcodeDay.colors();
        assert_eq!(background, Color::from_rgb8(0xf8, 0xf8, 0xf8));
        assert_eq!(foreground, Color::from_rgb8(0x26, 0x26, 0x26));
        let (background, foreground) = GuiTheme::WhcodeNight.colors();
        assert_eq!(background, Color::from_rgb8(0x16, 0x16, 0x16));
        assert_eq!(foreground, Color::from_rgb8(0xd4, 0xd4, 0xd4));
    }

    #[test]
    fn theme_cycle_returns_to_start_after_six_steps() {
        let mut theme = GuiTheme::EinkDay;
        let mut seen = Vec::new();
        for _ in 0..6 {
            theme = theme.next();
            seen.push(theme.name());
        }
        assert_eq!(
            seen,
            [
                "eink-night",
                "amoled-night",
                "terminal-green",
                "whcode-day",
                "whcode-night",
                "eink-day"
            ]
        );
        assert_eq!(theme, GuiTheme::EinkDay);
    }

    /// 默认主题为 whcode-night（2026-11-06 用户批准切换，ADR-057/FR-739）。
    #[test]
    fn default_theme_is_whcode_night() {
        assert_eq!(GuiTheme::default(), GuiTheme::WhcodeNight);
        let (background, _foreground) = GuiTheme::default().colors();
        assert_eq!(background, Color::from_rgb8(0x16, 0x16, 0x16));
    }

    #[test]
    fn scrollbar_uses_only_theme_colors() {
        for theme in [
            GuiTheme::EinkDay,
            GuiTheme::EinkNight,
            GuiTheme::AmoledNight,
        ] {
            let (background, foreground) = theme.colors();
            let style = theme.scrollbar();
            let rail = style.vertical_rail;
            assert_eq!(rail.background, Some(Background::Color(background)));
            assert_eq!(rail.scroller.background, Background::Color(foreground));
        }
    }

    #[test]
    fn runs_split_latin_and_cjk() {
        assert_eq!(
            split_runs("会话 Sessions 1/6 核对"),
            vec![(true, "会话"), (false, " Sessions 1/6 "), (true, "核对")]
        );
        assert!(split_runs("").is_empty());
        assert_eq!(split_runs("sessions"), vec![(false, "sessions")]);
        assert_eq!(split_runs("会话"), vec![(true, "会话")]);
    }

    #[test]
    fn font_selection_covers_four_categories() {
        assert_eq!(font_for(false, false), Font::with_name("Inter"));
        assert_eq!(font_for(true, false), Font::with_name("Noto Sans CJK SC"));
        assert_eq!(font_for(false, true), Font::with_name("JetBrains Mono"));
        assert_eq!(font_for(true, true), Font::with_name("Sarasa Mono SC"));
    }

    #[test]
    fn cjk_ranges_cover_punctuation_and_fullwidth() {
        assert!(is_cjk('会'));
        assert!(is_cjk('，'));
        assert!(is_cjk('（'));
        assert!(!is_cjk('S'));
        // 通用标点（U+2014）由 Inter/Noto 的拉丁子集覆盖，不按 CJK 切段。
        assert!(!is_cjk('—'));
    }

    /// 单会话测试源（ADR-102 后默认进入对话）：选中会话必须来自真实快照路径，
    /// 手工塞 `session_ids` 会被 `prepare()` 按数据源覆盖——测试也走同一路径。
    struct SingleSessionSource;

    impl ViewSource for SingleSessionSource {
        fn snapshot(&self, view: ViewId, _session: Option<&str>) -> ViewSnapshot {
            let mut snapshot = ViewSnapshot::empty(view);
            if view == ViewId::Sessions {
                snapshot.sessions = vec![ViewSessionRow {
                    id: "S-1".to_string(),
                    title: "测试会话".to_string(),
                    stage: "specify".to_string(),
                    status: "active".to_string(),
                    archived: false,
                    created_at_ms: 1,
                    last_active_at_ms: 1,
                }];
            }
            snapshot
        }
    }

    /// ADR-102：添加模型表单链路——打开、填字段、保存关闭并给出保存位置。
    #[test]
    fn model_form_opens_fills_and_saves() {
        struct StubEditor;
        impl ModelEditor for StubEditor {
            fn add(&self, draft: &ModelDraft) -> Result<String, WhError> {
                Ok(format!("/tmp/{}", draft.name))
            }
        }
        let source: Rc<dyn ViewSource> = Rc::new(SingleSessionSource);
        let mut state = State::new(source, I18n::default(), GuiTheme::EinkDay, None, None);
        state.model_editor = Some(Rc::new(StubEditor));
        assert!(state.model_editor.is_some());
        let _ = update(&mut state, Message::OpenModelForm);
        assert!(state.model_form.is_some(), "点击后表单必须打开");
        let _ = update(
            &mut state,
            Message::ModelFieldChanged(ModelField::Name, "m1".into()),
        );
        let _ = update(
            &mut state,
            Message::ModelFieldChanged(ModelField::BaseUrl, "https://x".into()),
        );
        let _ = update(
            &mut state,
            Message::ModelFieldChanged(ModelField::Model, "gpt".into()),
        );
        let _ = update(
            &mut state,
            Message::ModelFieldChanged(ModelField::Credential, "KEY".into()),
        );
        let _ = update(&mut state, Message::SaveModel);
        assert!(state.model_form.is_none(), "保存成功后表单关闭");
        assert!(
            state
                .model_form_status
                .as_deref()
                .unwrap_or_default()
                .contains("m1"),
            "必须显示保存位置：{:?}",
            state.model_form_status
        );
    }

    /// ADR-102 主界面对齐：打开即对话（ZCode 行为），侧栏目录与主区视图解耦。
    #[test]
    fn defaults_to_chat_with_sidebar_sessions() {
        let source: Rc<dyn ViewSource> = Rc::new(SingleSessionSource);
        let state = State::new(source, I18n::default(), GuiTheme::EinkDay, None, None);
        assert_eq!(state.view, ViewId::Chat, "默认视图必须是对话");
        assert_eq!(state.sidebar_sessions.len(), 1, "侧栏目录必须常驻");
        assert_eq!(state.session_ids, vec!["S-1".to_string()]);
    }

    /// 空数据源状态（T199 顶栏/KPI/状态栏取证；不触库）。
    fn empty_state() -> State {
        let source: Rc<dyn ViewSource> = Rc::new(whcode_app::view_source::EmptyViewSource);
        State::new(source, I18n::default(), GuiTheme::EinkDay, None, None)
    }

    /// B2（FR-746）：目录视图过滤/排序/归档分组——与 Web `session_directory`
    /// 同规则（含归档时活跃在前、归档在后；标题序大小写不敏感）。
    #[test]
    fn session_directory_filters_sorts_and_groups_archived() {
        let mut state = empty_state();
        let row = |id: &str, title: &str, archived: bool, stamp: i64| ViewSessionRow {
            id: id.to_string(),
            title: title.to_string(),
            stage: "plan".to_string(),
            status: "active".to_string(),
            archived,
            created_at_ms: stamp,
            last_active_at_ms: stamp,
        };
        state.all_sessions = vec![
            row("S-1", "Alpha", false, 10),
            row("S-2", "beta", true, 30),
            row("S-3", "Gamma", false, 20),
        ];
        state.session_sort = SessionSort::Title;
        state.apply_session_view();
        assert_eq!(
            state.session_ids,
            vec!["S-1", "S-3"],
            "默认不含归档，标题序"
        );

        state.session_show_archived = true;
        state.apply_session_view();
        assert_eq!(
            state.session_ids,
            vec!["S-1", "S-3", "S-2"],
            "归档排在活跃之后"
        );

        state.session_filter = "gam".to_string();
        state.apply_session_view();
        assert_eq!(state.session_ids, vec!["S-3"], "标题子串过滤");

        state.session_filter = String::new();
        state.session_sort = SessionSort::LastActive;
        state.apply_session_view();
        assert_eq!(
            state.session_ids,
            vec!["S-3", "S-1", "S-2"],
            "最近活跃降序，归档仍排后"
        );
    }

    /// T199：窗口标题与网络层同源词条（应用名 + 视图序号），状态栏含主题名。
    #[test]
    fn title_and_status_bar_use_shared_text_layer() {
        let state = empty_state();
        assert!(
            state.title().contains("旺皇大模型执行框架"),
            "标题须含应用名：{}",
            state.title()
        );
        assert!(
            state.title().contains(&format!("对话 (7/{VIEW_COUNT})")),
            "标题须含视图序号：{}",
            state.title()
        );
        assert_eq!(
            state.session_label(),
            "未选择会话",
            "未选择时必须有显式占位"
        );
        assert!(
            state.footer().contains("eink-day"),
            "状态栏须显示主题名：{}",
            state.footer()
        );
        assert!(
            state.footer().contains("Tab"),
            "状态栏须给出视图切换的可操作提示：{}",
            state.footer()
        );
    }

    /// T199：侧栏选中态只用主题两色（反显），不引入灰色、边框或阴影。
    #[test]
    fn sidebar_selection_inverts_theme_colors_only() {
        for theme in [
            GuiTheme::EinkDay,
            GuiTheme::EinkNight,
            GuiTheme::AmoledNight,
            GuiTheme::TerminalGreen,
        ] {
            let (background, foreground) = theme.colors();
            let selected = sidebar_style(theme, true, button::Status::Active);
            assert_eq!(selected.background, Some(Background::Color(foreground)));
            assert_eq!(selected.text_color, background);
            let normal = sidebar_style(theme, false, button::Status::Active);
            assert_eq!(normal.background, Some(Background::Color(background)));
            assert_eq!(normal.text_color, foreground);
            assert_eq!(normal.border, Border::default(), "侧栏不得加边框");
            assert_eq!(normal.shadow, Shadow::default(), "侧栏不得加阴影");
        }
    }

    /// ADR-102：whcode 系控件三档强调必须有可见差异（主操作填充、次操作描边、
    /// 弱操作透明），否则「哪里可点」又退回靠记忆键位。
    #[test]
    fn control_emphasis_levels_are_visually_distinct() {
        let theme = GuiTheme::WhcodeNight;
        let tokens = whcode_domain::theme_tokens::WHCODE_NIGHT;
        let ui = tokens.ui;
        let primary = control_button_style(theme, Emphasis::Primary, button::Status::Active);
        let secondary = control_button_style(theme, Emphasis::Secondary, button::Status::Active);
        let ghost = control_button_style(theme, Emphasis::Ghost, button::Status::Active);
        assert_eq!(
            primary.background,
            Some(Background::Color(token_color_any(ui.primary))),
            "主操作必须有填充"
        );
        assert_eq!(primary.text_color, token_color_any(ui.primary_foreground));
        assert_eq!(
            secondary.background,
            Some(Background::Color(token_color_any(tokens.ui.secondary)))
        );
        assert!(secondary.border.width > 0.0, "次操作必须有描边");
        assert_eq!(
            ghost.background,
            Some(Background::Color(Color::TRANSPARENT))
        );
        assert_eq!(ghost.border.width, 0.0, "弱操作默认无边框");
    }

    /// ADR-102：聚焦态边框换 `primary`，聚焦位置可辨。
    ///
    /// 实测口径：ZCode `zai-dark` 的 `--color-input` 与 `--color-input-focused` 同值
    /// （#2b2b2b），聚焦差异只由边框承担——断言按真实 token 写，不假设底色必然变化。
    #[test]
    fn field_style_marks_focus() {
        let theme = GuiTheme::WhcodeNight;
        let tokens = whcode_domain::theme_tokens::WHCODE_NIGHT;
        let active = field_style(theme, text_input::Status::Active);
        let focused = field_style(theme, text_input::Status::Focused { is_hovered: false });
        assert_eq!(active.border.color, token_color_any(tokens.border));
        assert_eq!(focused.border.color, token_color_any(tokens.ui.primary));
        assert_ne!(
            active.border.color, focused.border.color,
            "聚焦边框必须与默认边框不同，否则焦点不可见"
        );
        assert!(
            focused.border.width > 0.0,
            "聚焦边框必须有宽度，否则颜色差异不可见"
        );
    }

    /// ADR-041 守住：四套铁律主题仍只用主题两色，不引入第三色值。
    #[test]
    fn iron_themes_keep_two_color_discipline() {
        for theme in [
            GuiTheme::EinkDay,
            GuiTheme::EinkNight,
            GuiTheme::AmoledNight,
            GuiTheme::TerminalGreen,
        ] {
            let (_, foreground) = theme.colors();
            let style = control_button_style(theme, Emphasis::Secondary, button::Status::Active);
            assert_eq!(
                style.border.color, foreground,
                "边框必须是前景色，不得引入第三色值"
            );
            let card = card_style(theme);
            assert_eq!(card.border.color, foreground);
            assert_eq!(card.shadow, Shadow::default(), "铁律主题不得加阴影");
        }
    }

    /// Phase 2 测试桩：记录调用的 LLM 执行器（不触库）。
    struct StubLlm {
        fail: bool,
        /// 收到的 run_with_progress 请求 ID（取消确定性断言用，ADR-087 修订）。
        sent_requests: std::sync::Mutex<Vec<String>>,
        /// cancel 收到的请求 ID。
        cancels: std::sync::Mutex<Vec<String>>,
    }
    impl GuiLlmExecutor for StubLlm {
        fn run_with_progress(
            &self,
            _session_id: &str,
            _provider: &str,
            _prompt: &str,
            request_id: &str,
            progress: whcode_app::turn::TurnProgress<'_>,
            observer: Option<whcode_app::turn::TurnObserver<'_>>,
            _approvals: Option<&dyn whcode_app::approval::ApprovalAnswerer>,
        ) -> Result<String, WhError> {
            self.sent_requests
                .lock()
                .expect("锁")
                .push(request_id.to_string());
            if let Some(observer) = observer {
                observer(whcode_app::turn::TurnSignal::ToolStarted {
                    tool_call_id: "call-1".to_string(),
                    tool_name: "read_file".to_string(),
                    exec_id: "TX-1-0-0".to_string(),
                    iteration: 0,
                });
                observer(whcode_app::turn::TurnSignal::ToolFinished {
                    tool_call_id: "call-1".to_string(),
                    exec_id: "TX-1-0-0".to_string(),
                    state: "completed".to_string(),
                    side_effect: Some("read_only".to_string()),
                    output_value: Some("读到 3 行".to_string()),
                    error_code: None,
                    duration_ms: 7,
                });
            }
            progress(1, "你");
            progress(2, "好");
            if self.fail {
                return Err(WhError::invalid_state("stub 失败"));
            }
            Ok("provider=stub chunks=2 completed=true request_id=R-1".to_string())
        }

        fn cancel(&self, request_id: &str) -> Result<String, WhError> {
            self.cancels
                .lock()
                .expect("锁")
                .push(request_id.to_string());
            Ok("cancelled".to_string())
        }

        fn profiles(&self) -> Vec<String> {
            vec!["stub".to_string()]
        }
    }

    fn interactive_state_with(llm: std::sync::Arc<StubLlm>) -> State {
        let source: Rc<dyn ViewSource> = Rc::new(SingleSessionSource);
        let mut state = State::new(source, I18n::default(), GuiTheme::EinkDay, None, Some(llm));
        // 预置选中会话（对话提交的前置；目录来自数据源快照）。
        state.selected = Some(0);
        state.chat_input = "提示词".to_string();
        state
    }

    fn interactive_state(fail: bool) -> State {
        interactive_state_with(std::sync::Arc::new(StubLlm {
            fail,
            sent_requests: std::sync::Mutex::new(Vec::new()),
            cancels: std::sync::Mutex::new(Vec::new()),
        }))
    }

    /// ADR-087 修订（Check 477）：取消使用提交前预生成的 request_id（不再反解析
    /// 完成摘要），提交即存 active_request_id，完成后复位。
    #[test]
    fn chat_cancel_uses_pregenerated_request_id() {
        let stub = std::sync::Arc::new(StubLlm {
            fail: false,
            sent_requests: std::sync::Mutex::new(Vec::new()),
            cancels: std::sync::Mutex::new(Vec::new()),
        });
        let mut state = interactive_state_with(std::sync::Arc::clone(&stub));
        let _ = update(&mut state, Message::ChatSubmit);
        assert!(state.turn_running, "提交后必须进入运行态");
        let request_id = state
            .active_request_id
            .clone()
            .expect("提交必须预生成 request_id");
        assert!(
            request_id.starts_with("llm-"),
            "预生成 ID 格式：{request_id}"
        );
        // 执行器登记发生在后台线程——有界轮询等待登记完成再断言（主线程先断言
        // 会与线程启动竞态，并经毒丸放大为工作线程 panic）。
        let mut delivered = false;
        for _ in 0..100 {
            let sent = stub.sent_requests.lock().expect("锁");
            if !sent.is_empty() {
                delivered = true;
                break;
            }
            drop(sent);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(delivered, "执行器未在时限内收到请求");
        let sent = stub.sent_requests.lock().expect("锁");
        assert_eq!(
            sent.as_slice(),
            [request_id.as_str()],
            "执行器必须收到同一预生成 ID"
        );
        drop(sent);
        let _ = update(&mut state, Message::ChatCancel);
        let cancels = stub.cancels.lock().expect("锁");
        assert_eq!(cancels.len(), 1, "取消恰好一次到达执行器");
        assert_eq!(cancels[0], request_id, "取消必须携带预生成 ID");
        drop(cancels);
        // 完成后复位（Task 不在单测中执行，手动派发 ChatDone）。
        let _ = update(
            &mut state,
            Message::ChatDone(Ok("provider=stub completed=true request_id=R-1".to_string())),
        );
        assert!(!state.turn_running, "完成后解除运行态");
        assert!(state.active_request_id.is_none(), "完成后必须复位请求 ID");
    }

    /// Phase 2：ChatDone 正确解除 turn_running 并落结果（成功路径）。
    #[test]
    fn chat_done_clears_running_and_records_summary() {
        let mut state = interactive_state(false);
        state.turn_running = true;
        let _ = update(
            &mut state,
            Message::ChatDone(Ok("provider=stub chunks=2 completed=true".to_string())),
        );
        assert!(!state.turn_running, "完成后必须解除运行态");
        assert!(
            state
                .chat_last
                .as_deref()
                .is_some_and(|text| text.contains("provider=stub")),
            "完成后必须呈现摘要：{:?}",
            state.chat_last
        );
    }

    /// Phase 2：ChatDone 错误路径同样解除运行态（不把失败伪装成成功）。
    #[test]
    fn chat_done_error_clears_running_and_records_error() {
        let mut state = interactive_state(true);
        state.turn_running = true;
        let _ = update(
            &mut state,
            Message::ChatDone(Err(WhError::invalid_state("失败"))),
        );
        assert!(!state.turn_running, "失败后必须解除运行态");
        assert!(
            state
                .chat_last
                .as_deref()
                .is_some_and(|text| text.contains("失败")),
            "失败必须可见：{:?}",
            state.chat_last
        );
    }

    /// T393（llm.models）：ShowModels 经执行器拉取摘要；未接线执行器显式 not_supported。
    #[test]
    fn show_models_records_summary_or_explicit_unsupported() {
        // interactive_state 注入 StubLlm（未实现 model_summary → 默认 not_supported）。
        let mut state = interactive_state(false);
        let _ = update(&mut state, Message::ShowModels);
        assert!(
            state
                .model_status
                .as_deref()
                .is_some_and(|text| text.contains("not_supported")),
            "未接线的执行器必须显式 not_supported：{:?}",
            state.model_status
        );
        // 无执行器：同样显式不可用（不伪造目录）。
        let mut state = interactive_state(false);
        state.llm = None;
        let _ = update(&mut state, Message::ShowModels);
        assert!(
            state
                .model_status
                .as_deref()
                .is_some_and(|text| text.contains("not_supported")),
            "无执行器必须显式 not_supported：{:?}",
            state.model_status
        );
    }

    /// FR-741：进度增量逐块累积到流式缓冲；完成后清空（正文落 chat_last）。
    #[test]
    fn chat_progress_appends_streaming_and_done_clears_it() {
        let mut state = interactive_state(false);
        state.turn_running = true;
        let _ = update(&mut state, Message::ChatProgress(1, "你".to_string()));
        let _ = update(&mut state, Message::ChatProgress(2, "好".to_string()));
        assert_eq!(state.chat_progress, 2);
        assert_eq!(state.chat_streaming, "你好");
        let _ = update(
            &mut state,
            Message::ChatDone(Ok("provider=stub chunks=2".to_string())),
        );
        assert!(state.chat_streaming.is_empty(), "完成后缓冲必须清空");
        assert!(
            state
                .chat_last
                .as_deref()
                .is_some_and(|text| text.contains("provider=stub")),
        );
    }

    /// FR-741 有界性：流式缓冲超限截断且截断点落在字符边界（不得 panic）。
    #[test]
    fn streaming_buffer_is_bounded_on_char_boundary() {
        let mut buffer = String::new();
        let cjk_chunk = "你好世界".repeat(64);
        for _ in 0..((CHAT_STREAMING_CAP_BYTES / cjk_chunk.len()) + 8) {
            append_streaming(&mut buffer, &cjk_chunk);
        }
        assert!(
            buffer.len() <= CHAT_STREAMING_CAP_BYTES,
            "缓冲必须有界：{}",
            buffer.len()
        );
        assert!(buffer.is_char_boundary(buffer.len()));
    }

    /// FR-741 markdown 最小集投影：标题／无序列表／代码围栏／行内码；其余按普通文本。
    #[test]
    fn md_project_covers_minimal_markdown() {
        let text =
            "# 标题\n\n- 项目 `code` 一\n* 项目二\n```rust\nlet a = 1;\n```\n普通 `行内` 文本";
        let lines = md_project(text);
        assert_eq!(lines.len(), 6, "空行产出空 Text 行：{lines:?}");
        assert_eq!(lines[0].kind, MdLineKind::Heading);
        assert_eq!(lines[0].segs, vec![(false, "标题".to_string())]);
        assert_eq!(lines[1].kind, MdLineKind::Text);
        assert_eq!(lines[2].kind, MdLineKind::Bullet);
        assert_eq!(
            lines[2].segs,
            vec![
                (false, "项目 ".to_string()),
                (true, "code".to_string()),
                (false, " 一".to_string())
            ]
        );
        assert_eq!(lines[3].kind, MdLineKind::Bullet);
        assert_eq!(lines[4].kind, MdLineKind::Code);
        assert_eq!(lines[4].segs, vec![(true, "let a = 1;".to_string())]);
        assert_eq!(lines[5].kind, MdLineKind::Text);
    }

    /// FR-741 有界渲染：超过 CHAT_RENDER_MAX_LINES 行时以“…”行标注。
    #[test]
    fn md_elements_caps_rendered_lines() {
        let text = "- 项\n".repeat(CHAT_RENDER_MAX_LINES + 25);
        let elements = md_elements(text.as_str());
        assert_eq!(
            elements.len(),
            CHAT_RENDER_MAX_LINES + 1,
            "渲染行必须封顶并以省略行标注"
        );
    }

    /// FR-741：对话历史经数据源读入缓存（进入对话视图即刷新；数据形状为
    /// `(role, content)` 时间正序）。
    #[test]
    fn chat_history_loads_from_source_on_view_enter() {
        struct HistorySource;
        impl whcode_app::view_source::ViewSource for HistorySource {
            fn snapshot(
                &self,
                view: ViewId,
                _session: Option<&str>,
            ) -> whcode_app::view_source::ViewSnapshot {
                let mut snapshot = whcode_app::view_source::ViewSnapshot::empty(view);
                if view == ViewId::Sessions {
                    snapshot.sessions = vec![ViewSessionRow {
                        id: "S-1".to_string(),
                        title: "历史测试".to_string(),
                        stage: "specify".to_string(),
                        status: "active".to_string(),
                        archived: false,
                        created_at_ms: 1,
                        last_active_at_ms: 1,
                    }];
                }
                snapshot
            }
            fn conversation_tail(&self, session_id: &str, max: usize) -> Vec<(String, String)> {
                assert_eq!(session_id, "S-1");
                assert!(max <= 100, "拉取数不得超过 app 层上限");
                vec![
                    ("user".to_string(), "你好".to_string()),
                    ("assistant".to_string(), "# 回答\n- 要点".to_string()),
                ]
            }
        }
        let source: std::rc::Rc<dyn whcode_app::view_source::ViewSource> =
            std::rc::Rc::new(HistorySource);
        let mut state = State::new(source, I18n::default(), GuiTheme::default(), None, None);
        state.selected = Some(0);
        let _ = update(&mut state, Message::SelectView(ViewId::Chat));
        assert_eq!(state.chat_history.len(), 2, "进入对话视图必须刷新历史缓存");
        assert_eq!(
            state.chat_history[0],
            ("user".to_string(), "你好".to_string())
        );
    }

    /// Phase 2：turn 运行中冻结交互（视图切换/刷新被忽略），进度可见。
    #[test]
    fn turn_running_freezes_interactions_and_shows_progress() {
        let mut state = interactive_state(false);
        state.turn_running = true;
        state.chat_progress = 7;
        let _ = update(&mut state, Message::SelectView(ViewId::Tasks));
        let _ = update(&mut state, Message::Refresh);
        let _ = update(&mut state, Message::ChatProgress(9, String::new()));
        assert_eq!(state.view, ViewId::Chat, "运行中不得切换视图");
        assert_eq!(state.chat_progress, 9, "进度必须更新");
        assert!(state.turn_running, "进度不得解除运行态");
    }

    /// Phase 2：ChatSubmit 设运行态；无执行器/无会话/空提示词显式提示且不进入运行态。
    #[test]
    fn chat_submit_guards_are_explicit() {
        let mut state = interactive_state(false);
        state.llm = None;
        let _ = update(&mut state, Message::ChatSubmit);
        assert!(
            state
                .chat_last
                .as_deref()
                .is_some_and(|text| text.contains("not_supported")),
            "无执行器必须显式提示：{:?}",
            state.chat_last
        );

        let mut state = interactive_state(false);
        state.chat_input = "  ".to_string();
        let _ = update(&mut state, Message::ChatSubmit);
        assert!(
            state
                .chat_last
                .as_deref()
                .is_some_and(|text| text.contains("为空")),
            "空提示词必须显式提示：{:?}",
            state.chat_last
        );

        let mut state = interactive_state(false);
        state.selected = None;
        let _ = update(&mut state, Message::ChatSubmit);
        assert!(
            state
                .chat_last
                .as_deref()
                .is_some_and(|text| text.contains("未选择会话")),
            "无会话必须显式提示：{:?}",
            state.chat_last
        );
    }

    /// T256／FR-743：工具信号建卡与落终态；卡片数有界（旧卡先移除）。
    #[test]
    fn tool_signals_build_and_settle_bounded_cards() {
        let mut state = interactive_state(false);
        state.turn_running = true;
        let _ = update(
            &mut state,
            Message::ToolSignal(whcode_app::turn::TurnSignal::ToolStarted {
                tool_call_id: "call-1".to_string(),
                tool_name: "read_file".to_string(),
                exec_id: "TX-1-0-0".to_string(),
                iteration: 0,
            }),
        );
        assert_eq!(state.tool_cards.len(), 1);
        assert_eq!(state.tool_cards[0].state, None, "Started 后为运行中");
        let _ = update(
            &mut state,
            Message::ToolSignal(whcode_app::turn::TurnSignal::ToolFinished {
                tool_call_id: "call-1".to_string(),
                exec_id: "TX-1-0-0".to_string(),
                state: "completed".to_string(),
                side_effect: Some("read_only".to_string()),
                output_value: Some("正文".to_string()),
                error_code: None,
                duration_ms: 42,
            }),
        );
        assert_eq!(state.tool_cards[0].state.as_deref(), Some("completed"));
        assert_eq!(state.tool_cards[0].duration_ms, Some(42));
        assert_eq!(state.tool_cards[0].output.as_deref(), Some("正文"));

        for index in 0..(MAX_TOOL_CARDS + 5) {
            let _ = update(
                &mut state,
                Message::ToolSignal(whcode_app::turn::TurnSignal::ToolStarted {
                    tool_call_id: format!("call-{index}"),
                    tool_name: "shell".to_string(),
                    exec_id: format!("TX-{index}"),
                    iteration: 0,
                }),
            );
        }
        assert_eq!(
            state.tool_cards.len(),
            MAX_TOOL_CARDS,
            "卡片必须有界（旧卡先移除）"
        );
        assert_eq!(
            state
                .tool_cards
                .last()
                .map(|card| card.tool_call_id.as_str()),
            Some("call-24"),
            "最新卡在末尾"
        );
    }

    /// T256／FR-745：四态决定经审批面回传等待中的 turn 线程；已结算后不残留对话框。
    #[test]
    fn approval_decision_reaches_waiting_turn_thread() {
        use whcode_app::approval::ApprovalAnswerer;

        let mut state = interactive_state(false);
        state.turn_running = true;
        let (tx, _rx) = mpsc::channel::<Message>(8);
        let approvals = std::sync::Arc::new(GuiApprovals {
            inner: whcode_app::approval::InteractiveApprovals::new(),
            tx,
        });
        state.approvals = Some(std::sync::Arc::clone(&approvals));
        let request = whcode_domain::approval::ApprovalRequest::new(
            "AR-GUI",
            "TX-1",
            "shell",
            "S-1",
            whcode_domain::approval::ApprovalRule::Ask,
            Some("测试请求".to_string()),
            1,
        )
        .expect("请求");
        let _ = update(&mut state, Message::ApprovalPending(request.clone()));
        assert!(state.approval.is_some(), "待审批必须可见");

        let waiter = {
            let approvals = std::sync::Arc::clone(&approvals);
            let request = request.clone();
            std::thread::spawn(move || approvals.inner.answer(&request, 5_000))
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while approvals.pending().is_empty() {
            assert!(std::time::Instant::now() < deadline, "请求未登记");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let _ = update(
            &mut state,
            Message::ApprovalDecide(whcode_domain::approval::ApprovalAnswer::ApproveTurn),
        );
        assert!(state.approval.is_none(), "决定后对话框必须关闭");
        assert_eq!(
            waiter.join().expect("等待线程").expect("应答"),
            Some(whcode_domain::approval::ApprovalAnswer::ApproveTurn)
        );

        // 已结算后的重复决定：显式失败可见，不伪装成功。
        let _ = update(
            &mut state,
            Message::ApprovalDecide(whcode_domain::approval::ApprovalAnswer::RejectRemember),
        );
        assert!(state.approval.is_none(), "无待审批时不得重新打开对话框");
    }

    /// T260／FR-622：diff 行色取自冻结终端调色板；既有四套无调色板时回落前景色，
    /// 不引入主题外颜色。`+`／`-`／上下文三态映射独立于颜色（标记字符保留）。
    #[test]
    fn diff_colors_follow_theme_tokens_and_legacy_fallback() {
        let day = GuiTheme::WhcodeDay.diff_colors();
        assert_eq!(day.0, token_color("#1e8a3e"));
        assert_eq!(day.1, token_color("#e03131"));
        let night = GuiTheme::WhcodeNight.diff_colors();
        assert_eq!(night.0, token_color("#46bf72"));
        assert_eq!(night.1, token_color("#ff5c5c"));
        for theme in [
            GuiTheme::EinkDay,
            GuiTheme::EinkNight,
            GuiTheme::AmoledNight,
            GuiTheme::TerminalGreen,
        ] {
            let (_, foreground) = theme.colors();
            assert_eq!(
                theme.diff_colors(),
                (foreground, foreground),
                "{theme:?} 不得引入主题外颜色"
            );
        }
        assert_eq!(diff_line_color(GuiTheme::WhcodeNight, '+'), night.0);
        assert_eq!(diff_line_color(GuiTheme::WhcodeNight, '-'), night.1);
        assert_eq!(
            diff_line_color(GuiTheme::WhcodeNight, ' '),
            GuiTheme::WhcodeNight.colors().1
        );
    }

    /// 内容检索（T319／FR-780）：命中来自数据源且状态行给出计数；非法词/空词显式报错、
    /// 命中清空；turn 运行期间显式拒绝——不伪造“无命中”。
    #[test]
    fn content_search_uses_data_source_and_reports_errors() {
        struct SearchSource;
        impl ViewSource for SearchSource {
            fn snapshot(&self, view: ViewId, _session: Option<&str>) -> ViewSnapshot {
                ViewSnapshot::empty(view)
            }
            fn content_search(
                &self,
                needle: &str,
                limit: usize,
            ) -> Result<Vec<whcode_app::ContentHit>, WhError> {
                let _ = limit;
                if needle == "坏词" {
                    return Err(WhError::invalid_state("搜索词超过 128 字符"));
                }
                Ok(vec![whcode_app::ContentHit {
                    session_id: "S-1".to_string(),
                    session_title: "检索会话".to_string(),
                    message_id: "M-1".to_string(),
                    role: "assistant".to_string(),
                    snippet: format!("…{needle}…"),
                }])
            }
        }
        let source: Rc<dyn ViewSource> = Rc::new(SearchSource);
        let mut state = State::new(source, I18n::default(), GuiTheme::EinkDay, None, None);

        let _ = update(&mut state, Message::ContentSearchChanged("台账".into()));
        let _ = update(&mut state, Message::ContentSearchSubmit);
        assert_eq!(state.content_hits.len(), 1);
        assert_eq!(state.content_hits[0].session_id, "S-1");
        assert!(
            state
                .content_status
                .as_deref()
                .unwrap_or_default()
                .contains('1'),
            "状态行给出命中计数：{:?}",
            state.content_status
        );

        // 非法词：错误可见且命中清空（不是把错误画成“无命中”）。
        let _ = update(&mut state, Message::ContentSearchChanged("坏词".into()));
        let _ = update(&mut state, Message::ContentSearchSubmit);
        assert!(state.content_hits.is_empty());
        assert!(
            state
                .content_status
                .as_deref()
                .unwrap_or_default()
                .contains("128"),
            "{:?}",
            state.content_status
        );

        // 空词：显式提示，不触发数据源。
        let _ = update(&mut state, Message::ContentSearchChanged(String::new()));
        let _ = update(&mut state, Message::ContentSearchSubmit);
        assert!(
            state
                .content_status
                .as_deref()
                .unwrap_or_default()
                .contains("空"),
            "{:?}",
            state.content_status
        );

        // turn 运行期间：update 顶部冻结门拦截一切输入——状态与命中保持不变。
        let status_before = state.content_status.clone();
        let hits_before = state.content_hits.len();
        state.turn_running = true;
        let _ = update(&mut state, Message::ContentSearchChanged("台账".into()));
        let _ = update(&mut state, Message::ContentSearchSubmit);
        assert_eq!(state.content_status, status_before, "冻结期间状态不变");
        assert_eq!(state.content_hits.len(), hits_before);
        assert_eq!(
            state.content_query, "",
            "冻结门连输入更新一并拦截（既有口径）"
        );
    }

    /// B4 面板数据流（T258／FR-749／750／751）：打开时准备阶段取数；turn 运行中
    /// 拒绝打开（不阻塞界面）；撤销规则经命令门面执行，结果行可见。
    #[test]
    fn panels_open_fetch_and_revoke_rule() {
        struct StubSource;
        impl ViewSource for StubSource {
            fn snapshot(&self, view: ViewId, _session: Option<&str>) -> ViewSnapshot {
                ViewSnapshot::empty(view)
            }
            fn panels(&self, _session: Option<&str>) -> PanelsSnapshot {
                let mut snapshot = PanelsSnapshot::default();
                snapshot.subagents.push(whcode_app::panels::SubagentRunRow {
                    exec_id: "TX-1".to_string(),
                    state: SubagentState::Ok,
                    output_summary: Some("子任务完成".to_string()),
                    termination: Some("model_finished".to_string()),
                    iterations: Some(2),
                    duration_ms: Some(42),
                    error_code: None,
                });
                snapshot.skills.push(whcode_app::panels::SkillRow {
                    name: "review".to_string(),
                    description: "评审".to_string(),
                    path: "/work/.whcode/skills/review/SKILL.md".to_string(),
                    issues: Vec::new(),
                });
                snapshot.mcp_configured = true;
                snapshot.mcp.push(whcode_app::panels::McpServerRow {
                    name: "local".to_string(),
                    transport: "stdio".to_string(),
                    enabled: true,
                    timeout_ms: 5_000,
                    max_output_bytes: 65_536,
                    status: McpStatus::Ok,
                    tools: vec!["search".to_string()],
                });
                snapshot
                    .rules
                    .push(whcode_app::approval_rules::ApprovalRuleView {
                        tool: "shell".to_string(),
                        rule: whcode_domain::approval::ApprovalRule::Deny,
                    });
                snapshot
            }
        }
        struct RecordingExecutor {
            calls: Rc<std::cell::RefCell<usize>>,
        }
        impl CommandExecutor for RecordingExecutor {
            fn execute(
                &self,
                command: FrontendCommand,
            ) -> Result<whcode_app::commands::CommandOutcome, WhError> {
                *self.calls.borrow_mut() += 1;
                match command {
                    FrontendCommand::ApprovalRuleRevoke { tool, .. } => {
                        Ok(whcode_app::commands::CommandOutcome::ApprovalRuleRevoked {
                            tool,
                            removed: true,
                        })
                    }
                    _ => Err(WhError::invalid_state("测试不接受其他命令")),
                }
            }
        }
        let calls = Rc::new(std::cell::RefCell::new(0_usize));
        let source: Rc<dyn ViewSource> = Rc::new(StubSource);
        let executor: Rc<dyn CommandExecutor> = Rc::new(RecordingExecutor {
            calls: Rc::clone(&calls),
        });
        let mut state = State::new(
            source,
            I18n::default(),
            GuiTheme::EinkDay,
            Some(executor),
            None,
        );

        // turn 运行中：取数冻结（连接锁由 turn 线程持有），面板保持关闭、不读数据。
        state.turn_running = true;
        let _ = update(&mut state, Message::TogglePanels);
        assert!(!state.panels_open);
        assert!(
            state.panels.subagents.is_empty(),
            "turn 运行中不得取面板数据"
        );
        state.turn_running = false;

        let _ = update(&mut state, Message::TogglePanels);
        assert!(state.panels_open);
        assert_eq!(state.panels.subagents.len(), 1);
        assert_eq!(state.panels.skills.len(), 1);
        assert_eq!(state.panels.mcp.len(), 1);
        assert_eq!(state.panels.rules.len(), 1);

        let _ = update(&mut state, Message::RevokeRule("shell".to_string()));
        assert_eq!(*calls.borrow(), 1, "撤销必须经命令门面");
        assert!(
            state
                .panel_status
                .as_deref()
                .is_some_and(|text| text.contains("shell"))
        );

        // turn 开始后可关闭（不触库），但不得重新打开取数。
        state.turn_running = true;
        let _ = update(&mut state, Message::TogglePanels);
        assert!(!state.panels_open);
        let _ = update(&mut state, Message::TogglePanels);
        assert!(!state.panels_open, "turn 运行中不得打开面板");
        state.turn_running = false;

        let _ = update(&mut state, Message::TogglePanels);
        assert!(state.panels_open);
        let _ = update(&mut state, Message::EscapeKey);
        assert!(!state.panels_open, "Esc 关闭面板");
    }

    /// B4 面板文案（T258／FR-749／750／751）：GUI 引用的每个键在十三套目录中
    /// 都必须是译文；回退成键名即失败。
    #[test]
    fn panel_texts_resolve_in_every_catalog() {
        const KEYS: &[&str] = &[
            "settings.panels.title",
            "settings.panels.open",
            "settings.panels.refresh",
            "settings.panels.empty",
            "settings.panels.subagents.title",
            "settings.panels.subagents.iterations",
            "settings.panels.skills.title",
            "settings.panels.skills.issues",
            "settings.panels.mcp.title",
            "settings.panels.mcp.unconfigured",
            "settings.panels.mcp.configured",
            "settings.panels.mcp.disabled",
            "settings.panels.mcp.observed_ok",
            "settings.panels.mcp.timeout",
            "settings.panels.mcp.tools",
            "settings.panels.rules.title",
            "settings.panels.rules.revoked",
            "settings.panels.rules.missing",
            "settings.panels.rules.failed",
            "settings.panels.state.running",
            "settings.panels.state.ok",
            "settings.panels.state.failed",
            "settings.approvals.empty",
            "settings.approvals.revoke",
            // ADR-092／FR-800 批 1：Spec 写面按钮（十三套目录均须有译文，不得回退键名）。
            "gui.op.spec.next",
            "gui.op.spec.force",
            // ADR-092／FR-800 批 2：运行时视图标题与写面按钮。
            "view.runtime",
            "gui.op.runtime.enable",
            "gui.op.runtime.disable",
            "gui.op.runtime.install",
            // ADR-092／FR-800 批 3：事件导出按钮与证书面板。
            "gui.op.export",
            "settings.panels.cert.title",
            "settings.panels.cert.modes",
            "settings.panels.cert.ca.write",
            "settings.panels.cert.ca.reset",
            "settings.panels.cert.regenerate",
        ];
        for (locale, _) in whcode_domain::i18n::BUILTIN_LOCALES {
            let texts = I18n::load(locale, whcode_domain::i18n::LocaleSource::Explicit);
            for key in KEYS {
                assert_ne!(texts.resolve(key), *key, "{locale} 缺 {key}");
            }
        }
    }

    /// ADR-092／FR-800 批 1：Spec 写面经按钮／键位入 GUI——正向（空输入沿后继、
    /// 带目标、reason 必填）、失败（空目标／空 reason 不执行）、键位门控（仅 Spec 视图）。
    /// 以命令记录断言门面形状；落库与审计由 whcode-app 单点保证，本端不伪造。
    #[test]
    fn spec_write_surfaces_positive_failure_and_key_gated() {
        use std::cell::RefCell;

        struct CaptureExecutor {
            calls: Rc<RefCell<Vec<FrontendCommand>>>,
        }
        impl CommandExecutor for CaptureExecutor {
            fn execute(
                &self,
                command: FrontendCommand,
            ) -> Result<whcode_app::commands::CommandOutcome, WhError> {
                self.calls.borrow_mut().push(command);
                Ok(whcode_app::commands::CommandOutcome::PlanSet(
                    whcode_app::PlanView {
                        active: false,
                        seq: None,
                    },
                ))
            }
        }
        let calls = Rc::new(RefCell::new(Vec::new()));
        let executor: Rc<dyn CommandExecutor> = Rc::new(CaptureExecutor {
            calls: Rc::clone(&calls),
        });
        let source: Rc<dyn ViewSource> = Rc::new(SingleSessionSource);
        let mut state = State::new(
            source,
            I18n::default(),
            GuiTheme::EinkDay,
            Some(executor),
            None,
        );
        state.selected = Some(0);

        // 键位门控：非 Spec 视图按 n／F 不进入写模式。
        let _ = update(&mut state, Message::SelectView(ViewId::Events));
        let _ = update(&mut state, Message::Key('n'));
        assert!(state.op_mode.is_none(), "非 Spec 视图 n 不得触发写面");
        let _ = update(&mut state, Message::Key('F'));
        assert!(state.op_mode.is_none(), "非 Spec 视图 F 不得触发写面");

        // 正向 1：spec.next 空输入＝沿状态机后继（to=None）。
        let _ = update(&mut state, Message::SelectView(ViewId::Spec));
        let _ = update(&mut state, Message::Key('n'));
        assert!(matches!(state.op_mode, Some(OpMode::SpecNext)));
        let _ = update(&mut state, Message::OpConfirm);
        match calls.borrow().last().expect("next 应发命令") {
            FrontendCommand::SpecNext { to: None, .. } => {}
            other => panic!("空输入应发 to=None，实际 {other:?}"),
        }
        assert!(state.op_mode.is_none(), "成功后退出操作面板");

        // 正向 2：spec.next 带目标阶段（to=Some）。
        let _ = update(&mut state, Message::Key('n'));
        let _ = update(&mut state, Message::OpInputChanged("plan".into()));
        let _ = update(&mut state, Message::OpConfirm);
        match calls.borrow().last().expect("next 应发命令") {
            FrontendCommand::SpecNext { to: Some(to), .. } if to == "plan" => {}
            other => panic!("带目标应发 to=plan，实际 {other:?}"),
        }

        // 正向 3：spec.force-advance 两步——首步冻结目标、次步读 reason。
        let before = calls.borrow().len();
        let _ = update(&mut state, Message::Key('F'));
        assert!(matches!(
            state.op_mode,
            Some(OpMode::SpecForceAdvance { to: None })
        ));
        let _ = update(&mut state, Message::OpInputChanged(String::new()));
        let _ = update(&mut state, Message::OpPreview);
        assert!(
            state
                .op_feedback
                .as_deref()
                .unwrap_or_default()
                .contains("不能为空"),
            "空目标须显式拒绝：{:?}",
            state.op_feedback
        );
        assert_eq!(calls.borrow().len(), before, "空目标不得发命令");

        let _ = update(&mut state, Message::OpInputChanged("plan".into()));
        let _ = update(&mut state, Message::OpPreview);
        assert!(matches!(
            state.op_mode,
            Some(OpMode::SpecForceAdvance { to: Some(_) })
        ));
        let _ = update(&mut state, Message::OpInputChanged(String::new()));
        let _ = update(&mut state, Message::OpConfirm);
        assert_eq!(calls.borrow().len(), before, "空 reason 不得发命令");
        assert!(
            state
                .op_feedback
                .as_deref()
                .unwrap_or_default()
                .contains("理由"),
            "空 reason 须显式拒绝：{:?}",
            state.op_feedback
        );

        let _ = update(&mut state, Message::OpInputChanged("人工放行".into()));
        let _ = update(&mut state, Message::OpConfirm);
        match calls.borrow().last().expect("force 应发命令") {
            FrontendCommand::SpecForceAdvance { to, reason, .. } => {
                assert_eq!(to, "plan");
                assert_eq!(reason, "人工放行");
            }
            other => panic!("force 形状不符：{other:?}"),
        }
    }

    /// ADR-092／FR-800 批 2：运行时视图——清单按需读取（probe=false）、选中读状态、
    /// 三写面经门面（enable/disable/install），install 三段解析与失败路径不执行。
    #[test]
    fn runtime_view_reads_and_write_surfaces_positive_failure() {
        use std::cell::RefCell;

        struct RuntimeSource;
        impl ViewSource for RuntimeSource {
            fn snapshot(&self, view: ViewId, _session: Option<&str>) -> ViewSnapshot {
                ViewSnapshot::empty(view)
            }
            fn runtime_entries(&self) -> Vec<whcode_app::runtime_mgmt::RuntimeEntry> {
                vec![whcode_app::runtime_mgmt::RuntimeEntry {
                    name: "python".to_string(),
                    capability: "restricted",
                    available: None, // 界面口径不探测
                    unsupported: vec![],
                    default_enabled: false,
                    enable_requirement: "需显式 opt-in",
                }]
            }
            fn runtime_status_view(
                &self,
                name: &str,
            ) -> Result<whcode_app::runtime_mgmt::RuntimeStatusView, WhError> {
                if name != "python" {
                    return Err(WhError::invalid_state("未登记运行时"));
                }
                Ok(whcode_app::runtime_mgmt::RuntimeStatusView {
                    schema_version: whcode_domain::SCHEMA_VERSION,
                    name: name.to_string(),
                    capability: "restricted",
                    available: None,
                    unsupported: vec![],
                    enabled: false,
                    version: String::new(),
                    payload_path: None,
                    default_enabled: false,
                    enable_requirement: "需显式 opt-in".to_string(),
                })
            }
        }
        struct CaptureExecutor {
            calls: Rc<RefCell<Vec<FrontendCommand>>>,
        }
        impl CommandExecutor for CaptureExecutor {
            fn execute(
                &self,
                command: FrontendCommand,
            ) -> Result<whcode_app::commands::CommandOutcome, WhError> {
                self.calls.borrow_mut().push(command);
                Ok(whcode_app::commands::CommandOutcome::RuntimeStatus(
                    whcode_app::runtime_mgmt::RuntimeStatusView {
                        schema_version: whcode_domain::SCHEMA_VERSION,
                        name: "python".to_string(),
                        capability: "restricted",
                        available: None,
                        unsupported: vec![],
                        enabled: true,
                        version: "3.12".to_string(),
                        payload_path: None,
                        default_enabled: false,
                        enable_requirement: "需显式 opt-in".to_string(),
                    },
                ))
            }
        }

        let calls = Rc::new(RefCell::new(Vec::new()));
        let executor: Rc<dyn CommandExecutor> = Rc::new(CaptureExecutor {
            calls: Rc::clone(&calls),
        });
        let source: Rc<dyn ViewSource> = Rc::new(RuntimeSource);
        let mut state = State::new(
            source,
            I18n::default(),
            GuiTheme::EinkDay,
            Some(executor),
            None,
        );

        // 正向：切到运行时视图，清单按需读取；选中行读状态（probe=false）。
        let _ = update(&mut state, Message::SelectView(ViewId::Runtime));
        assert_eq!(state.runtime_entries.as_ref().map(Vec::len), Some(1));
        assert!(state.runtime_status.is_none(), "未选中不得伪造状态");
        let _ = update(&mut state, Message::RuntimeSelect(0));
        assert_eq!(state.runtime_selected, Some(0));
        assert_eq!(
            state.runtime_status.as_ref().map(|v| v.name.as_str()),
            Some("python")
        );
        assert!(
            state
                .runtime_status
                .as_ref()
                .is_some_and(|v| v.available.is_none())
        );

        // 正向：enable / disable 经门面（name 由选中行冻结）。
        let _ = update(
            &mut state,
            Message::OpStart(OpMode::RuntimeEnable {
                name: "python".into(),
            }),
        );
        let _ = update(&mut state, Message::OpConfirm);
        assert!(
            matches!(calls.borrow().last(), Some(FrontendCommand::RuntimeEnable { name, .. }) if name == "python")
        );
        let _ = update(
            &mut state,
            Message::OpStart(OpMode::RuntimeDisable {
                name: "python".into(),
            }),
        );
        let _ = update(&mut state, Message::OpConfirm);
        assert!(
            matches!(calls.borrow().last(), Some(FrontendCommand::RuntimeDisable { name, .. }) if name == "python")
        );

        // 失败路径：install 三段不足不发命令，且反馈可见。
        let before = calls.borrow().len();
        let _ = update(
            &mut state,
            Message::OpStart(OpMode::RuntimeInstall {
                name: "python".into(),
            }),
        );
        let _ = update(&mut state, Message::OpInputChanged("3.12|abc".into()));
        let _ = update(&mut state, Message::OpConfirm);
        assert_eq!(calls.borrow().len(), before, "install 段数不足不得发命令");
        assert!(
            state
                .op_feedback
                .as_deref()
                .unwrap_or_default()
                .contains("版本"),
            "{:?}",
            state.op_feedback
        );

        // 正向：install 三段完整，本地路径原样传递（不联网）。
        let _ = update(
            &mut state,
            Message::OpInputChanged("3.12|abc|C:/tmp/payload.bin".into()),
        );
        let _ = update(&mut state, Message::OpConfirm);
        match calls.borrow().last().expect("install 应发命令") {
            FrontendCommand::RuntimeInstall {
                name,
                version,
                hash,
                payload,
                ..
            } => {
                assert_eq!(name, "python");
                assert_eq!(version, "3.12");
                assert_eq!(hash, "abc");
                assert_eq!(payload.to_string_lossy(), "C:/tmp/payload.bin");
            }
            other => panic!("install 形状不符：{other:?}"),
        }

        // 失败路径：未登记运行时状态读取显式错误，不伪造启用。
        let err = state.source.runtime_status_view("nope");
        assert!(err.is_err(), "未登记运行时必须报错");
    }

    /// ADR-092／FR-800 批 3：证书面——读状态不含私钥、写经门面（降档确认），
    /// 私钥上传不在 GUI（OpMode 无 WriteServer 变体）。
    #[test]
    fn cert_panel_reads_public_state_and_write_requires_confirm() {
        use std::cell::RefCell;

        struct CertSource;
        impl ViewSource for CertSource {
            fn snapshot(&self, view: ViewId, _session: Option<&str>) -> ViewSnapshot {
                ViewSnapshot::empty(view)
            }
            fn tls_status(&self) -> Result<whcode_domain::TlsStatus, WhError> {
                Ok(whcode_domain::TlsStatus {
                    schema_version: whcode_domain::SETTINGS_SCHEMA_VERSION,
                    outbound: whcode_domain::OutboundStatus {
                        mode: whcode_domain::TrustMode::Builtin,
                        source: "builtin".to_string(),
                        builtin: None,
                        extra: None,
                        system_root_count: None,
                        effective_root_count: Some(0),
                    },
                    inbound: whcode_domain::ServerCertStatus {
                        mode: whcode_domain::InboundMode::Builtin,
                        cert_present: true,
                        key_present: true, // 只报存在性，不含内容
                        subject: None,
                        sans: vec![],
                        not_before: None,
                        not_after: None,
                        sha256: None,
                        days_remaining: None,
                    },
                    restart_required: false,
                    notes: vec![],
                })
            }
        }
        struct CaptureExecutor {
            calls: Rc<RefCell<Vec<FrontendCommand>>>,
        }
        impl CommandExecutor for CaptureExecutor {
            fn execute(
                &self,
                command: FrontendCommand,
            ) -> Result<whcode_app::commands::CommandOutcome, WhError> {
                self.calls.borrow_mut().push(command);
                Err(WhError::invalid_state("测试执行器不落证书文件"))
            }
        }

        let calls = Rc::new(RefCell::new(Vec::new()));
        let executor: Rc<dyn CommandExecutor> = Rc::new(CaptureExecutor {
            calls: Rc::clone(&calls),
        });
        let source: Rc<dyn ViewSource> = Rc::new(CertSource);
        let mut state = State::new(
            source,
            I18n::default(),
            GuiTheme::EinkDay,
            Some(executor),
            None,
        );

        // 正向：刷新面板时读证书状态（不含私钥内容）。
        let _ = update(&mut state, Message::RefreshPanels);
        assert!(state.tls_status.is_some(), "证书状态应已读取");
        assert!(
            state
                .tls_status
                .as_ref()
                .is_some_and(|v| v.inbound.key_present)
        );

        // 失败路径：降档到 off 未确认不得发命令。
        let _ = update(
            &mut state,
            Message::OpStart(OpMode::CertWrite(CertWriteUiAction::SetModes)),
        );
        let _ = update(
            &mut state,
            Message::OpInputChanged("builtin|off|false".into()),
        );
        let _ = update(&mut state, Message::OpConfirm);
        assert!(calls.borrow().is_empty(), "未确认的降档不得发命令");
        assert!(
            state
                .op_feedback
                .as_deref()
                .unwrap_or_default()
                .contains("确认"),
            "{:?}",
            state.op_feedback
        );

        // 正向：显式确认后发 SetModes 命令。
        let _ = update(
            &mut state,
            Message::OpInputChanged("builtin|off|true".into()),
        );
        let _ = update(&mut state, Message::OpConfirm);
        match calls.borrow().last().expect("确认后应发命令") {
            FrontendCommand::CertWrite(whcode_app::commands::CertWriteAction::SetModes {
                inbound,
                confirm,
                ..
            }) => {
                assert_eq!(*inbound, whcode_domain::InboundMode::Off);
                assert!(*confirm);
            }
            other => panic!("证书写形状不符：{other:?}"),
        }

        // 空数据源：证书状态不伪造（保持 None）。
        let empty: Rc<dyn ViewSource> = Rc::new(whcode_app::view_source::EmptyViewSource);
        let mut empty_state = State::new(empty, I18n::default(), GuiTheme::EinkDay, None, None);
        let _ = update(&mut empty_state, Message::RefreshPanels);
        assert!(
            empty_state.tls_status.is_none(),
            "不支持的数据源不得伪造证书状态"
        );
    }

    /// ADR-091／FR-799：GUI 读面详情（events.show / spec.*）——正向、失败、有界、
    /// 键位门控。数据源默认不支持时显式说明，不伪造空结果。
    #[test]
    fn read_detail_surfaces_positive_failure_bounded_and_key_gated() {
        struct DetailSource {
            long: bool,
            fail: bool,
        }
        impl ViewSource for DetailSource {
            fn snapshot(&self, view: ViewId, session: Option<&str>) -> ViewSnapshot {
                let mut snapshot = ViewSnapshot::empty(view);
                if view == ViewId::Sessions {
                    snapshot.sessions = vec![ViewSessionRow {
                        id: "S-1".to_string(),
                        title: "详情测试".to_string(),
                        stage: "specify".to_string(),
                        status: "active".to_string(),
                        archived: false,
                        created_at_ms: 1,
                        last_active_at_ms: 1,
                    }];
                }
                if view == ViewId::Events {
                    snapshot.events.push(whcode_app::view_source::EventRow {
                        seq: 7,
                        kind: "session.new".to_string(),
                        summary: "会话创建".to_string(),
                    });
                }
                let _ = session;
                snapshot
            }
            fn event_detail(&self, event_id: &str) -> Result<String, WhError> {
                if self.fail {
                    return Err(WhError::new(
                        whcode_domain::ErrorCode::NotFound,
                        "事件不存在",
                    ));
                }
                Ok(if self.long {
                    "长".repeat(READ_DETAIL_TEXT_MAX + 50)
                } else {
                    format!("事件详情 {event_id}")
                })
            }
            fn spec_gate_summary(&self, session: &str, kind: &str) -> Result<String, WhError> {
                if self.fail {
                    return Err(WhError::new(
                        whcode_domain::ErrorCode::InvalidState,
                        "产物读取失败",
                    ));
                }
                Ok(format!("{session}/{kind} 摘要"))
            }
        }

        // 正向：事件详情与三类门禁摘要可读。
        let source: Rc<dyn ViewSource> = Rc::new(DetailSource {
            long: false,
            fail: false,
        });
        let mut state = State::new(source, I18n::default(), GuiTheme::EinkDay, None, None);
        state.selected = Some(0);
        let _ = update(&mut state, Message::SelectView(ViewId::Events));
        let _ = update(&mut state, Message::EventDetail(0));
        assert_eq!(state.event_selected, Some(0));
        assert!(state.event_detail.as_deref().unwrap().contains("E-7"));
        for kind in ["validate", "analyze", "converge"] {
            let _ = update(&mut state, Message::SpecGate(kind));
            assert!(state.spec_summary.as_deref().unwrap().contains(kind));
        }

        // 失败路径：错误显式说明，不伪造空。
        let source: Rc<dyn ViewSource> = Rc::new(DetailSource {
            long: false,
            fail: true,
        });
        let mut state = State::new(source, I18n::default(), GuiTheme::EinkDay, None, None);
        state.selected = Some(0);
        let _ = update(&mut state, Message::SelectView(ViewId::Events));
        let _ = update(&mut state, Message::EventDetail(0));
        assert!(state.event_detail.as_deref().unwrap().contains("失败"));
        let _ = update(&mut state, Message::SpecGate("validate"));
        assert!(state.spec_summary.as_deref().unwrap().contains("失败"));

        // 有界：超 200 字符按字符边界截断并注明。
        let source: Rc<dyn ViewSource> = Rc::new(DetailSource {
            long: true,
            fail: false,
        });
        let mut state = State::new(source, I18n::default(), GuiTheme::EinkDay, None, None);
        let _ = update(&mut state, Message::SelectView(ViewId::Events));
        let _ = update(&mut state, Message::EventDetail(0));
        let detail = state.event_detail.as_deref().unwrap();
        assert!(detail.contains("已截断"), "超限必须注明截断");
        assert!(detail.chars().count() <= READ_DETAIL_TEXT_MAX + 10);

        // 无数据源：event_detail 默认返回 not_supported，显式说明。
        let source: Rc<dyn ViewSource> = Rc::new(whcode_app::view_source::EmptyViewSource);
        let mut state = State::new(source, I18n::default(), GuiTheme::EinkDay, None, None);
        let _ = update(&mut state, Message::EventDetail(0));
        assert!(state.event_detail.is_some(), "失败也要写状态，不留空");
    }

    /// ADR-091／FR-799：键位门控——e 仅 Events 视图、v/z/o 仅 Spec 视图生效；
    /// 其他视图的同类字符键不改变详情状态。
    #[test]
    fn read_detail_keys_are_view_gated() {
        let source: Rc<dyn ViewSource> = Rc::new(whcode_app::view_source::EmptyViewSource);
        let mut state = State::new(source, I18n::default(), GuiTheme::EinkDay, None, None);
        // 会话视图下按 e / v / z / o 均不改变详情字段。
        for key in ['e', 'v', 'z', 'o'] {
            let _ = update(&mut state, Message::Key(key));
        }
        assert!(state.event_detail.is_none());
        assert!(state.spec_summary.is_none());
        // 切到 Events 视图后 e 生效（默认选中 0）。
        let _ = update(&mut state, Message::SelectView(ViewId::Events));
        let _ = update(&mut state, Message::Key('e'));
        assert!(state.event_detail.is_some(), "Events 视图 e 必须触发详情");
        // 切到 Spec 视图后 v/z/o 生效。
        let _ = update(&mut state, Message::SelectView(ViewId::Spec));
        let _ = update(&mut state, Message::Key('v'));
        assert!(state.spec_summary.is_some(), "Spec 视图 v 必须触发门禁摘要");
    }
}
