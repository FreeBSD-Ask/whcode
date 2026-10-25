//! # 四端能力登记表（单一事实源；FR-619—FR-621、ADR-032、Task 169）
//!
//! 本模块是 CUI／TUI／GUI／Web 四端能力面的**唯一定义**（ADR-032 判据 2）：每条
//! [`SurfaceEntry`] 登记一个稳定 [`SurfaceId`] 的类别、所需 [`Capability`]、效果类别、
//! 确认与审计要求，以及四端状态 [`Support`]。前端只声明到 `SurfaceId` 的映射
//! （各端 `surface_map`），不得各自定义“支持什么”；`docs/matrices/feature-parity.md`
//! 由登记表校验（Task 171），手工漂移即对等门禁失败。
//!
//! 登记内容以 `.trae/specs/ensure-frontend-parity/spec.md` 的“对等基线”表为权威，
//! 状态记号映射如下：
//!
//! - ✅ 已实现 → [`Support::Implemented`]，锚点为真实函数名或路由（去源码核对）；
//! - ◐ 已登记排除 → [`Support::Excluded`]，理由码见 [`ExclusionReason`]（ADR-032 全表）；
//! - ⏳ 预留 → [`Support::Reserved`]，带里程碑（`M8`、`M3-llm`）；
//! - ➕ 本变更新增 → 交付时已同 PR 翻转为已实现（T181 收口，2026-10-24）；
//! - — 不适用 → [`Support::NotApplicable`]（判定依据写入条目注释）。
//!
//! 字段约定（登记税的最小化与诚实性）：
//!
//! - `capability` 登记**应用层执行路径实际判定的能力**（`authz::require` 调用点为准）；
//!   尚不经应用层授权的登记面（本地管理、传输、显示配置与预留面）登记占位能力并在
//!   条目注释注明——规则：Read → `SessionRead`、Write → `SessionCreate`、
//!   Destructive → `SessionDelete`（与 `maintenance.rs`“复用 session.delete，不另立
//!   授权口径”一致）。能力扩展（专属 `Capability`）属另一变更，须同 PR 更新本表。
//! - `Excluded` 的 FR／ADR 锚点由 ADR-032 理由码表统一承载（每码一行“锚点”列），
//!   [`Support::Excluded`] 变体只携带理由码。
//! - `audited` 为“是否强制审计”（contracts/app-service.md §2.1）；契约 §2 破坏族
//!   （delete/archive/purge/compact/force-advance）按契约登记为强制审计，现状未及者
//!   由写命令门面（Task 176）补齐。
//!
//! 边界：纯静态数据——无 IO、无线程、无容量累积；不依赖任何前端 crate（分层门禁
//! `test_layering` 强制）。

use std::collections::BTreeSet;

use crate::authz::Capability;

/// 前端标识（`SurfaceEntry::frontends` 的下标语义见 [`FRONTEND_ORDER`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FrontendId {
    /// 命令行界面（一次性请求响应，ADR-019）。
    Cui,
    /// 终端界面（六视图，FR-601）。
    Tui,
    /// 原生桌面 GUI（只读 v1，FR-711）。
    Gui,
    /// 原生 Web UI（FR-603）。
    Web,
}

impl FrontendId {
    /// 稳定标识（入错误详情与矩阵列名）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cui => "cui",
            Self::Tui => "tui",
            Self::Gui => "gui",
            Self::Web => "web",
        }
    }
}

/// 四端固定顺序：`SurfaceEntry::frontends` 数组按下标对应该顺序。
pub const FRONTEND_ORDER: [FrontendId; 4] = [
    FrontendId::Cui,
    FrontendId::Tui,
    FrontendId::Gui,
    FrontendId::Web,
];

/// 登记面类别：命令面或视图面（六视图与“在线可见”为 View）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceKind {
    /// 命令面（CLI 子命令、HTTP 写路由、键盘动作）。
    Command,
    /// 视图面（六视图快照与在线可见，FR-601／622）。
    View,
}

impl SurfaceKind {
    /// 稳定标识。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Command => "command",
            Self::View => "view",
        }
    }
}

/// 效果类别（业务数据效果；仅改呈现状态的主题／语言登记为 Read 并注明）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectClass {
    /// 只读。
    Read,
    /// 写入（业务状态变更，可逆）。
    Write,
    /// 破坏性（不可逆数据删除或替换；契约 §2 确认族）。
    Destructive,
}

impl EffectClass {
    /// 稳定标识。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Destructive => "destructive",
        }
    }
}

/// 确认要求（契约 §2-1：破坏性命令必须 dry-run 或确认）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirmation {
    /// 无需确认。
    None,
    /// 先预览（dry-run）后确认（`db restore` 为确认无预览，取最接近值并注释）。
    PreviewConfirm,
    /// 必须携带理由（`spec force-advance` 的 `--reason`，FR-312）。
    ReasonRequired,
}

impl Confirmation {
    /// 稳定标识。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::PreviewConfirm => "preview_confirm",
            Self::ReasonRequired => "reason_required",
        }
    }
}

/// 排除理由码（ADR-032 全表的蛇形码；差异只允许这些理由）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExclusionReason {
    /// GUI 全部写命令（FR-711／ADR-026／ADR-027）。
    GuiReadOnlyV1,
    /// db 维护、font、serve、doctor、bench（FR-507—510、FR-717、ADR-019、ADR-028）。
    CliLocalAdmin,
    /// CUI 无屏幕视图面（FR-602、ADR-019）。
    CuiCommandSurface,
    /// 视图面任务详情只到摘要（FR-103／104、ADR-009）。
    ViewSummaryOnly,
    /// 需要文件路径的命令输出（FR-205、FR-601／711）。
    CommandOutputOnly,
    /// 终端配色由终端模拟器控制（FR-612、ADR-014）。
    CuiNoTheme,
    /// TUI／GUI 固定六视图，runtime 清单不在其中（FR-601／711）。
    SixViewScope,
    /// CUI 为一次性请求响应，无在线订阅（FR-602）。
    CuiRequestOnly,
    /// GUI v1 以显式刷新为准，禁止定时器重绘（FR-711／714）。
    GuiManualRefresh,
    /// SSH／serve 等独立传输入口（FR-603／607／608）。
    TransportSpecific,
    /// 证书库设置面仅由 Web 提供（FR-626／627、ADR-040 决策 4）。
    WebSettingsOnly,
}

impl ExclusionReason {
    /// 稳定理由码（ADR-032 蛇形）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GuiReadOnlyV1 => "gui_read_only_v1",
            Self::CliLocalAdmin => "cli_local_admin",
            Self::CuiCommandSurface => "cui_command_surface",
            Self::ViewSummaryOnly => "view_summary_only",
            Self::CommandOutputOnly => "command_output_only",
            Self::CuiNoTheme => "cui_no_theme",
            Self::SixViewScope => "six_view_scope",
            Self::CuiRequestOnly => "cui_request_only",
            Self::GuiManualRefresh => "gui_manual_refresh",
            Self::TransportSpecific => "transport_specific",
            Self::WebSettingsOnly => "web_settings_only",
        }
    }
}

/// 单端支持状态（四态判定表，ADR-032；不存在第五种“说不清”）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    /// 已实现：该端存在到登记能力的可用映射，锚点为真实函数名或路由。
    Implemented {
        /// 前端锚点（函数名或路由）。
        anchor: &'static str,
    },
    /// 已排除：按 ADR-032 登记的理由不提供；FR／ADR 锚点见理由码表。
    Excluded {
        /// 稳定理由码。
        reason: ExclusionReason,
    },
    /// 预留：功能未交付，等待交付里程碑（含本变更内的交付任务号）。
    Reserved {
        /// 里程碑或交付任务号。
        milestone: &'static str,
    },
    /// 不适用：能力不属于该端形态范围（判定依据写入条目注释）。
    NotApplicable,
}

impl Support {
    /// 状态码（`not_supported` 详情的 `state` 字段取值，FR-620）。
    #[must_use]
    pub const fn state_code(self) -> &'static str {
        match self {
            Self::Implemented { .. } => "implemented",
            Self::Excluded { .. } => "excluded",
            Self::Reserved { .. } => "reserved",
            Self::NotApplicable => "not_applicable",
        }
    }
}

/// 单条能力登记。
#[derive(Debug, Clone, Copy)]
pub struct SurfaceEntry {
    /// 稳定 SurfaceId（如 `session.delete`、`view.diff`）。
    pub id: &'static str,
    /// 命令面／视图面。
    pub kind: SurfaceKind,
    /// 所需能力（经 `authz::require` 统一判定；占位约定见模块注释）。
    pub capability: Capability,
    /// 效果类别。
    pub effect: EffectClass,
    /// 确认要求。
    pub confirmation: Confirmation,
    /// 是否强制审计（契约 §2.1“是否强制审计”）。
    pub audited: bool,
    /// 四端状态，固定顺序 [Cui, Tui, Gui, Web]（[`FRONTEND_ORDER`]）。
    pub frontends: [Support; 4],
}

impl SurfaceEntry {
    /// 取指定端的状态。
    #[must_use]
    pub fn support(&self, frontend: FrontendId) -> &Support {
        let index = match frontend {
            FrontendId::Cui => 0,
            FrontendId::Tui => 1,
            FrontendId::Gui => 2,
            FrontendId::Web => 3,
        };
        &self.frontends[index]
    }
}

/// 稳定能力白名单（`authz::Capability` 的稳定字符串；新增能力须同 PR 扩展本表）。
const KNOWN_CAPABILITIES: [&str; 11] = [
    "session.create",
    "session.read",
    "session.rename",
    "session.archive",
    "session.delete",
    "task.read",
    "task.retry",
    "task.abort",
    "spec.advance",
    "settings.cert.read",
    "settings.cert.write",
];

/// ADR-032 理由码全表（排除登记只能引用这些码）。
const ADR032_REASON_CODES: [&str; 11] = [
    "gui_read_only_v1",
    "cli_local_admin",
    "cui_command_surface",
    "view_summary_only",
    "command_output_only",
    "cui_no_theme",
    "six_view_scope",
    "cui_request_only",
    "gui_manual_refresh",
    "transport_specific",
    "web_settings_only",
];

/// 能力登记表（对等基线逐行落实；对等基线表见 ensure-frontend-parity spec）。
pub const SURFACES: &[SurfaceEntry] = &[
    // ── session（FR-101／102；会话生命周期） ─────────────────────────────
    SurfaceEntry {
        id: "session.new",
        kind: SurfaceKind::Command,
        capability: Capability::SessionCreate,
        effect: EffectClass::Write,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Implemented {
                anchor: "cui_dispatch::session_command",
            },
            Support::Implemented {
                anchor: "LoopCore::on_key_normal（n → SessionNew）",
            },
            Support::Excluded {
                reason: ExclusionReason::GuiReadOnlyV1,
            },
            Support::Implemented {
                anchor: "POST /api/sessions/new",
            },
        ],
    },
    SurfaceEntry {
        id: "session.list",
        kind: SurfaceKind::Command,
        capability: Capability::SessionRead,
        effect: EffectClass::Read,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Implemented {
                anchor: "cui_dispatch::session_command",
            },
            Support::Implemented {
                anchor: "LoopCore::prepare（ViewId::Sessions）",
            },
            Support::Implemented {
                anchor: "State::prepare（ViewId::Sessions）",
            },
            Support::Implemented {
                anchor: "GET /api/sessions",
            },
        ],
    },
    SurfaceEntry {
        id: "session.show",
        kind: SurfaceKind::Command,
        capability: Capability::SessionRead,
        effect: EffectClass::Read,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Implemented {
                anchor: "cui_dispatch::session_command",
            },
            Support::Implemented {
                anchor: "LoopCore::prepare（ViewId::Sessions）",
            },
            Support::Implemented {
                anchor: "State::prepare（ViewId::Sessions）",
            },
            Support::Implemented {
                anchor: "GET /api/status?session=",
            },
        ],
    },
    SurfaceEntry {
        id: "session.rename",
        kind: SurfaceKind::Command,
        capability: Capability::SessionRename,
        effect: EffectClass::Write,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Implemented {
                anchor: "cui_dispatch::session_command",
            },
            Support::Implemented {
                anchor: "LoopCore::on_key_normal（m → SessionRename）",
            },
            Support::Excluded {
                reason: ExclusionReason::GuiReadOnlyV1,
            },
            Support::Implemented {
                anchor: "POST /api/sessions/rename",
            },
        ],
    },
    // 契约 §2-1 破坏族：archive 须预览→确认并写审计（ADR-038，2026-10-25 补齐）。
    // 四端同口径：CUI 默认 dry-run／`--yes` 执行；TUI `A` 先预览再按 y；Web
    // `/api/sessions/archive` 缺 `confirm` 只预览；GUI 只读排除。审计由 store
    // `set_archived` 在同一事务写 `session_archive`（可逆标记，重复请求同样留痕）。
    SurfaceEntry {
        id: "session.archive",
        kind: SurfaceKind::Command,
        capability: Capability::SessionArchive,
        effect: EffectClass::Write,
        confirmation: Confirmation::PreviewConfirm,
        audited: true,
        frontends: [
            Support::Implemented {
                anchor: "cui_dispatch::session_command",
            },
            Support::Implemented {
                anchor: "LoopCore::request_confirm（A → SessionArchive 预览→确认）",
            },
            Support::Excluded {
                reason: ExclusionReason::GuiReadOnlyV1,
            },
            Support::Implemented {
                anchor: "POST /api/sessions/archive",
            },
        ],
    },
    SurfaceEntry {
        id: "session.delete",
        kind: SurfaceKind::Command,
        capability: Capability::SessionDelete,
        effect: EffectClass::Destructive,
        confirmation: Confirmation::PreviewConfirm,
        audited: true,
        frontends: [
            Support::Implemented {
                anchor: "cui_dispatch::session_command",
            },
            Support::Implemented {
                anchor: "LoopCore::request_confirm（d → SessionDelete 预览→确认）",
            },
            Support::Excluded {
                reason: ExclusionReason::GuiReadOnlyV1,
            },
            Support::Implemented {
                anchor: "POST /api/sessions/delete",
            },
        ],
    },
    // 应用层 switch_session 已交付（FR-102），四端接线均未排期。
    SurfaceEntry {
        id: "session.switch",
        kind: SurfaceKind::Command,
        capability: Capability::SessionRead,
        effect: EffectClass::Read,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Reserved {
                milestone: "未排期",
            },
            Support::Reserved {
                milestone: "未排期",
            },
            Support::Reserved {
                milestone: "未排期",
            },
            Support::Reserved {
                milestone: "未排期",
            },
        ],
    },
    // ── task（FR-103／104、FR-401／408） ────────────────────────────────
    SurfaceEntry {
        id: "task.list",
        kind: SurfaceKind::Command,
        capability: Capability::TaskRead,
        effect: EffectClass::Read,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Implemented {
                anchor: "cui_dispatch::task_command",
            },
            Support::Implemented {
                anchor: "LoopCore::prepare（ViewId::Tasks）",
            },
            Support::Implemented {
                anchor: "State::prepare（ViewId::Tasks）",
            },
            Support::Implemented {
                anchor: "GET /api/tasks?session=",
            },
        ],
    },
    SurfaceEntry {
        id: "task.overview",
        kind: SurfaceKind::Command,
        capability: Capability::TaskRead,
        effect: EffectClass::Read,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Implemented {
                anchor: "cui_dispatch::task_command",
            },
            Support::Implemented {
                anchor: "LoopCore::prepare（ViewId::Tasks）",
            },
            Support::Implemented {
                anchor: "State::prepare（ViewId::Tasks）",
            },
            Support::Implemented {
                anchor: "GET /api/tasks",
            },
        ],
    },
    // 视图面任务详情只到摘要（VIEW_SUMMARY_ONLY）；CUI 保留五类深查。
    SurfaceEntry {
        id: "task.show",
        kind: SurfaceKind::Command,
        capability: Capability::TaskRead,
        effect: EffectClass::Read,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Implemented {
                anchor: "cui_dispatch::task_command",
            },
            Support::Excluded {
                reason: ExclusionReason::ViewSummaryOnly,
            },
            Support::Excluded {
                reason: ExclusionReason::ViewSummaryOnly,
            },
            Support::Excluded {
                reason: ExclusionReason::ViewSummaryOnly,
            },
        ],
    },
    SurfaceEntry {
        id: "task.retry",
        kind: SurfaceKind::Command,
        capability: Capability::TaskRetry,
        effect: EffectClass::Write,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Implemented {
                anchor: "cui_dispatch::task_command",
            },
            Support::Implemented {
                anchor: "LoopCore::task_action（R → TaskRetry）",
            },
            Support::Excluded {
                reason: ExclusionReason::GuiReadOnlyV1,
            },
            Support::Implemented {
                anchor: "POST /api/tasks/retry",
            },
        ],
    },
    SurfaceEntry {
        id: "task.abort",
        kind: SurfaceKind::Command,
        capability: Capability::TaskAbort,
        effect: EffectClass::Write,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Implemented {
                anchor: "cui_dispatch::task_command",
            },
            Support::Implemented {
                anchor: "LoopCore::task_action（X → TaskAbort）",
            },
            Support::Excluded {
                reason: ExclusionReason::GuiReadOnlyV1,
            },
            Support::Implemented {
                anchor: "POST /api/tasks/abort",
            },
        ],
    },
    // ── context（FR-105／108；load_context 判 SessionRead） ─────────────
    SurfaceEntry {
        id: "context.budget",
        kind: SurfaceKind::Command,
        capability: Capability::SessionRead,
        effect: EffectClass::Read,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Implemented {
                anchor: "context_command（context budget）",
            },
            Support::Implemented {
                anchor: "LoopCore::prepare（ViewId::Context）",
            },
            Support::Implemented {
                anchor: "State::prepare（ViewId::Context）",
            },
            Support::Implemented {
                anchor: "GET /api/context?session=&budget=",
            },
        ],
    },
    // ── events（FR-202／205；events_export 判 SessionRead） ─────────────
    SurfaceEntry {
        id: "events.list",
        kind: SurfaceKind::Command,
        capability: Capability::SessionRead,
        effect: EffectClass::Read,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Implemented {
                anchor: "cui_dispatch::events_command",
            },
            Support::Implemented {
                anchor: "LoopCore::prepare（ViewId::Events）",
            },
            Support::Implemented {
                anchor: "State::prepare（ViewId::Events）",
            },
            Support::Implemented {
                anchor: "GET /api/events",
            },
        ],
    },
    SurfaceEntry {
        id: "events.export",
        kind: SurfaceKind::Command,
        capability: Capability::SessionRead,
        effect: EffectClass::Write,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Implemented {
                anchor: "cui_dispatch::events_command",
            },
            Support::Excluded {
                reason: ExclusionReason::CommandOutputOnly,
            },
            Support::Excluded {
                reason: ExclusionReason::CommandOutputOnly,
            },
            Support::Implemented {
                anchor: "GET /api/events/export?format=json|jsonl",
            },
        ],
    },
    // 应用层单事件接口未交付；GUI 按 FR-711 登记排除。
    SurfaceEntry {
        id: "events.show",
        kind: SurfaceKind::Command,
        capability: Capability::SessionRead,
        effect: EffectClass::Read,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Reserved {
                milestone: "未排期",
            },
            Support::Reserved {
                milestone: "未排期",
            },
            Support::Excluded {
                reason: ExclusionReason::GuiReadOnlyV1,
            },
            Support::Reserved {
                milestone: "未排期",
            },
        ],
    },
    // ── spec（FR-301—314；快照判 SessionRead，门禁判 SpecAdvance） ──────
    SurfaceEntry {
        id: "spec.status",
        kind: SurfaceKind::Command,
        capability: Capability::SessionRead,
        effect: EffectClass::Read,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Implemented {
                anchor: "cui_dispatch::spec_command",
            },
            Support::Implemented {
                anchor: "LoopCore::prepare（ViewId::Spec）",
            },
            Support::Implemented {
                anchor: "State::prepare（ViewId::Spec）",
            },
            Support::Implemented {
                anchor: "GET /api/status",
            },
        ],
    },
    // SDD 门禁应用层已交付，四端接线未排期（范围外：保持预留）。
    SurfaceEntry {
        id: "spec.next",
        kind: SurfaceKind::Command,
        capability: Capability::SpecAdvance,
        effect: EffectClass::Write,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Reserved {
                milestone: "未排期",
            },
            Support::Reserved {
                milestone: "未排期",
            },
            Support::Excluded {
                reason: ExclusionReason::GuiReadOnlyV1,
            },
            Support::Reserved {
                milestone: "未排期",
            },
        ],
    },
    SurfaceEntry {
        id: "spec.validate",
        kind: SurfaceKind::Command,
        capability: Capability::SpecAdvance,
        effect: EffectClass::Read,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Reserved {
                milestone: "未排期",
            },
            Support::Reserved {
                milestone: "未排期",
            },
            Support::Excluded {
                reason: ExclusionReason::GuiReadOnlyV1,
            },
            Support::Reserved {
                milestone: "未排期",
            },
        ],
    },
    SurfaceEntry {
        id: "spec.analyze",
        kind: SurfaceKind::Command,
        capability: Capability::SpecAdvance,
        effect: EffectClass::Read,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Reserved {
                milestone: "未排期",
            },
            Support::Reserved {
                milestone: "未排期",
            },
            Support::Excluded {
                reason: ExclusionReason::GuiReadOnlyV1,
            },
            Support::Reserved {
                milestone: "未排期",
            },
        ],
    },
    SurfaceEntry {
        id: "spec.converge",
        kind: SurfaceKind::Command,
        capability: Capability::SpecAdvance,
        effect: EffectClass::Write,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Reserved {
                milestone: "未排期",
            },
            Support::Reserved {
                milestone: "未排期",
            },
            Support::Excluded {
                reason: ExclusionReason::GuiReadOnlyV1,
            },
            Support::Reserved {
                milestone: "未排期",
            },
        ],
    },
    SurfaceEntry {
        id: "spec.force-advance",
        kind: SurfaceKind::Command,
        capability: Capability::SpecAdvance,
        effect: EffectClass::Write,
        confirmation: Confirmation::ReasonRequired,
        audited: true,
        frontends: [
            Support::Reserved {
                milestone: "未排期",
            },
            Support::Reserved {
                milestone: "未排期",
            },
            Support::Excluded {
                reason: ExclusionReason::GuiReadOnlyV1,
            },
            Support::Reserved {
                milestone: "未排期",
            },
        ],
    },
    // ── db（FR-507—510；维护大类复用 session.delete 能力，maintenance.rs） ─
    SurfaceEntry {
        id: "db.purge",
        kind: SurfaceKind::Command,
        capability: Capability::SessionDelete,
        effect: EffectClass::Destructive,
        confirmation: Confirmation::PreviewConfirm,
        audited: true,
        frontends: [
            Support::Implemented {
                anchor: "cui_dispatch::db_command",
            },
            Support::Implemented {
                anchor: "LoopCore::request_confirm（p → Purge 预览→确认）",
            },
            Support::Excluded {
                reason: ExclusionReason::GuiReadOnlyV1,
            },
            Support::Implemented {
                anchor: "POST /api/purge",
            },
        ],
    },
    SurfaceEntry {
        id: "db.stats",
        kind: SurfaceKind::Command,
        capability: Capability::SessionRead,
        effect: EffectClass::Read,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Implemented {
                anchor: "cui_dispatch::db_command",
            },
            Support::Excluded {
                reason: ExclusionReason::CliLocalAdmin,
            },
            Support::Excluded {
                reason: ExclusionReason::CliLocalAdmin,
            },
            Support::Excluded {
                reason: ExclusionReason::CliLocalAdmin,
            },
        ],
    },
    SurfaceEntry {
        id: "db.check",
        kind: SurfaceKind::Command,
        capability: Capability::SessionRead,
        effect: EffectClass::Read,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Implemented {
                anchor: "cui_dispatch::db_command",
            },
            Support::Excluded {
                reason: ExclusionReason::CliLocalAdmin,
            },
            Support::Excluded {
                reason: ExclusionReason::CliLocalAdmin,
            },
            Support::Excluded {
                reason: ExclusionReason::CliLocalAdmin,
            },
        ],
    },
    SurfaceEntry {
        id: "db.compact",
        kind: SurfaceKind::Command,
        capability: Capability::SessionDelete,
        effect: EffectClass::Destructive,
        confirmation: Confirmation::PreviewConfirm,
        audited: true,
        frontends: [
            Support::Implemented {
                anchor: "cui_dispatch::db_command",
            },
            Support::Excluded {
                reason: ExclusionReason::CliLocalAdmin,
            },
            Support::Excluded {
                reason: ExclusionReason::CliLocalAdmin,
            },
            Support::Excluded {
                reason: ExclusionReason::CliLocalAdmin,
            },
        ],
    },
    SurfaceEntry {
        id: "db.archive",
        kind: SurfaceKind::Command,
        capability: Capability::SessionRead,
        effect: EffectClass::Write,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Implemented {
                anchor: "cui_dispatch::db_command",
            },
            Support::Excluded {
                reason: ExclusionReason::CliLocalAdmin,
            },
            Support::Excluded {
                reason: ExclusionReason::CliLocalAdmin,
            },
            Support::Excluded {
                reason: ExclusionReason::CliLocalAdmin,
            },
        ],
    },
    // restore 确认无预览（--yes 必填）；PreviewConfirm 为三值枚举的最接近登记。
    SurfaceEntry {
        id: "db.restore",
        kind: SurfaceKind::Command,
        capability: Capability::SessionDelete,
        effect: EffectClass::Destructive,
        confirmation: Confirmation::PreviewConfirm,
        audited: false,
        frontends: [
            Support::Implemented {
                anchor: "cui_dispatch::db_command",
            },
            Support::Excluded {
                reason: ExclusionReason::CliLocalAdmin,
            },
            Support::Excluded {
                reason: ExclusionReason::CliLocalAdmin,
            },
            Support::Excluded {
                reason: ExclusionReason::CliLocalAdmin,
            },
        ],
    },
    SurfaceEntry {
        id: "db.checkpoint",
        kind: SurfaceKind::Command,
        capability: Capability::SessionDelete,
        effect: EffectClass::Write,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Implemented {
                anchor: "cui_dispatch::db_command",
            },
            Support::Excluded {
                reason: ExclusionReason::CliLocalAdmin,
            },
            Support::Excluded {
                reason: ExclusionReason::CliLocalAdmin,
            },
            Support::Excluded {
                reason: ExclusionReason::CliLocalAdmin,
            },
        ],
    },
    SurfaceEntry {
        id: "db.explain",
        kind: SurfaceKind::Command,
        capability: Capability::SessionRead,
        effect: EffectClass::Read,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Implemented {
                anchor: "cui_dispatch::db_command",
            },
            Support::Excluded {
                reason: ExclusionReason::CliLocalAdmin,
            },
            Support::Excluded {
                reason: ExclusionReason::CliLocalAdmin,
            },
            Support::Excluded {
                reason: ExclusionReason::CliLocalAdmin,
            },
        ],
    },
    // ── runtime（FR-701—710；M8） ──────────────────────────────────────
    SurfaceEntry {
        id: "runtime.list",
        kind: SurfaceKind::Command,
        capability: Capability::SessionRead,
        effect: EffectClass::Read,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Implemented {
                anchor: "runtime_command（RuntimeCommand::List）",
            },
            Support::Excluded {
                reason: ExclusionReason::SixViewScope,
            },
            Support::Excluded {
                reason: ExclusionReason::SixViewScope,
            },
            Support::Implemented {
                anchor: "GET /api/runtimes",
            },
        ],
    },
    SurfaceEntry {
        id: "runtime.status",
        kind: SurfaceKind::Command,
        capability: Capability::SessionRead,
        effect: EffectClass::Read,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Reserved { milestone: "M8" },
            Support::Reserved { milestone: "M8" },
            Support::Excluded {
                reason: ExclusionReason::SixViewScope,
            },
            Support::Reserved { milestone: "M8" },
        ],
    },
    SurfaceEntry {
        id: "runtime.enable",
        kind: SurfaceKind::Command,
        capability: Capability::SessionCreate,
        effect: EffectClass::Write,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Reserved { milestone: "M8" },
            Support::Reserved { milestone: "M8" },
            Support::Excluded {
                reason: ExclusionReason::SixViewScope,
            },
            Support::Reserved { milestone: "M8" },
        ],
    },
    SurfaceEntry {
        id: "runtime.disable",
        kind: SurfaceKind::Command,
        capability: Capability::SessionCreate,
        effect: EffectClass::Write,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Reserved { milestone: "M8" },
            Support::Reserved { milestone: "M8" },
            Support::Excluded {
                reason: ExclusionReason::SixViewScope,
            },
            Support::Reserved { milestone: "M8" },
        ],
    },
    SurfaceEntry {
        id: "runtime.install",
        kind: SurfaceKind::Command,
        capability: Capability::SessionCreate,
        effect: EffectClass::Write,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Reserved { milestone: "M8" },
            Support::Reserved { milestone: "M8" },
            Support::Excluded {
                reason: ExclusionReason::SixViewScope,
            },
            Support::Reserved { milestone: "M8" },
        ],
    },
    // ── llm（FR-201—208；T167 接线：CUI／Web 已实现，TUI 预留，GUI 只读排除） ──
    SurfaceEntry {
        id: "llm.run",
        kind: SurfaceKind::Command,
        capability: Capability::SessionCreate,
        effect: EffectClass::Write,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Implemented {
                anchor: "cui_dispatch::llm_command（LlmCommand::Run → whcode_app::llm::run）",
            },
            Support::Reserved {
                milestone: "M3-llm",
            },
            Support::Excluded {
                reason: ExclusionReason::GuiReadOnlyV1,
            },
            Support::Implemented {
                anchor: "POST /api/llm/run（handle_request_with_llm）",
            },
        ],
    },
    SurfaceEntry {
        id: "llm.cancel",
        kind: SurfaceKind::Command,
        capability: Capability::SessionCreate,
        effect: EffectClass::Write,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Implemented {
                anchor: "cui_dispatch::llm_command（LlmCommand::Cancel → whcode_app::llm::cancel）",
            },
            Support::Reserved {
                milestone: "M3-llm",
            },
            Support::Excluded {
                reason: ExclusionReason::GuiReadOnlyV1,
            },
            Support::Implemented {
                anchor: "POST /api/llm/cancel（handle_request_with_llm）",
            },
        ],
    },
    // ── 显示配置（FR-612／615／618／713；仅改呈现状态，不改业务数据） ─────
    SurfaceEntry {
        id: "theme.switch",
        kind: SurfaceKind::Command,
        capability: Capability::SessionRead,
        effect: EffectClass::Read,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Excluded {
                reason: ExclusionReason::CuiNoTheme,
            },
            Support::Implemented {
                anchor: "LoopCore::on_key_normal（t → CycleTheme）",
            },
            Support::Implemented {
                anchor: "handle_event（t → CycleTheme）",
            },
            Support::Implemented {
                anchor: "data-theme + #theme-select（app.js）",
            },
        ],
    },
    SurfaceEntry {
        id: "locale",
        kind: SurfaceKind::Command,
        capability: Capability::SessionRead,
        effect: EffectClass::Read,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Implemented {
                anchor: "cui_dispatch::dispatch_code（detect_locale）",
            },
            Support::Implemented {
                anchor: "RunOptions::texts（I18n）",
            },
            Support::Implemented {
                anchor: "RunOptions::texts（I18n）",
            },
            Support::Implemented {
                anchor: "GET /api/i18n",
            },
        ],
    },
    // ── 证书库设置（FR-626／627、ADR-040 决策 4；设置面仅 Web 提供） ─────────
    // 读面：状态、CA 下载与连通性校验；视图类别（设置页），六视图之外。
    // 写面：档位、根证书与服务端证书变更；降档（入站 off／空内置根／外部私钥）须确认，
    // 成功变更写 `tls_config_change` 审计（`whcode-app::cert`）。
    SurfaceEntry {
        id: "settings.cert.read",
        kind: SurfaceKind::View,
        capability: Capability::SettingsCertRead,
        effect: EffectClass::Read,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Excluded {
                reason: ExclusionReason::WebSettingsOnly,
            },
            Support::Excluded {
                reason: ExclusionReason::WebSettingsOnly,
            },
            Support::Excluded {
                reason: ExclusionReason::SixViewScope,
            },
            Support::Implemented {
                anchor: "GET /api/certs（/api/certs/ca、/api/certs/server、POST /api/certs/verify）",
            },
        ],
    },
    SurfaceEntry {
        id: "settings.cert.write",
        kind: SurfaceKind::Command,
        capability: Capability::SettingsCertWrite,
        effect: EffectClass::Destructive,
        confirmation: Confirmation::PreviewConfirm,
        audited: true,
        frontends: [
            Support::Excluded {
                reason: ExclusionReason::WebSettingsOnly,
            },
            Support::Excluded {
                reason: ExclusionReason::WebSettingsOnly,
            },
            Support::Excluded {
                reason: ExclusionReason::GuiReadOnlyV1,
            },
            Support::Implemented {
                anchor: "POST /api/certs/modes｜/api/certs/ca｜/api/certs/ca/reset｜\
                         /api/certs/server｜/api/certs/server/self-signed",
            },
        ],
    },
    // ── font（FR-717、ADR-028；本机资源，不触库） ───────────────────────
    SurfaceEntry {
        id: "font.install",
        kind: SurfaceKind::Command,
        capability: Capability::SessionCreate,
        effect: EffectClass::Write,
        confirmation: Confirmation::PreviewConfirm,
        audited: false,
        frontends: [
            Support::Implemented {
                anchor: "cui_dispatch::font_command",
            },
            Support::Excluded {
                reason: ExclusionReason::CliLocalAdmin,
            },
            Support::Excluded {
                reason: ExclusionReason::CliLocalAdmin,
            },
            Support::Excluded {
                reason: ExclusionReason::CliLocalAdmin,
            },
        ],
    },
    SurfaceEntry {
        id: "font.status",
        kind: SurfaceKind::Command,
        capability: Capability::SessionRead,
        effect: EffectClass::Read,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Implemented {
                anchor: "cui_dispatch::font_command",
            },
            Support::Excluded {
                reason: ExclusionReason::CliLocalAdmin,
            },
            Support::Excluded {
                reason: ExclusionReason::CliLocalAdmin,
            },
            Support::Excluded {
                reason: ExclusionReason::CliLocalAdmin,
            },
        ],
    },
    SurfaceEntry {
        id: "font.uninstall",
        kind: SurfaceKind::Command,
        capability: Capability::SessionDelete,
        effect: EffectClass::Destructive,
        confirmation: Confirmation::PreviewConfirm,
        audited: false,
        frontends: [
            Support::Implemented {
                anchor: "cui_dispatch::font_command",
            },
            Support::Excluded {
                reason: ExclusionReason::CliLocalAdmin,
            },
            Support::Excluded {
                reason: ExclusionReason::CliLocalAdmin,
            },
            Support::Excluded {
                reason: ExclusionReason::CliLocalAdmin,
            },
        ],
    },
    // ── 装配与诊断入口（serve 为传输入口，ADR-019／ADR-023） ─────────────
    SurfaceEntry {
        id: "serve",
        kind: SurfaceKind::Command,
        capability: Capability::SessionCreate,
        effect: EffectClass::Write,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Implemented {
                anchor: "whcode_bin::serve::serve_command",
            },
            Support::Excluded {
                reason: ExclusionReason::CliLocalAdmin,
            },
            Support::Excluded {
                reason: ExclusionReason::CliLocalAdmin,
            },
            Support::Excluded {
                reason: ExclusionReason::CliLocalAdmin,
            },
        ],
    },
    SurfaceEntry {
        id: "doctor",
        kind: SurfaceKind::Command,
        capability: Capability::SessionRead,
        effect: EffectClass::Read,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Implemented {
                anchor: "whcode_bin::doctor",
            },
            Support::Excluded {
                reason: ExclusionReason::CliLocalAdmin,
            },
            Support::Excluded {
                reason: ExclusionReason::CliLocalAdmin,
            },
            Support::Excluded {
                reason: ExclusionReason::CliLocalAdmin,
            },
        ],
    },
    SurfaceEntry {
        id: "bench",
        kind: SurfaceKind::Command,
        capability: Capability::SessionRead,
        effect: EffectClass::Read,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Implemented {
                anchor: "whcode_bin::bench_command",
            },
            Support::Excluded {
                reason: ExclusionReason::CliLocalAdmin,
            },
            Support::Excluded {
                reason: ExclusionReason::CliLocalAdmin,
            },
            Support::Excluded {
                reason: ExclusionReason::CliLocalAdmin,
            },
        ],
    },
    // 应用生命周期动作（q 退出）：非业务命令，登记以承载 TUI／GUI 动作清单；
    // CUI 为一次性请求响应（无退出动作），Web 由浏览器标签页生命周期承载。
    SurfaceEntry {
        id: "app.quit",
        kind: SurfaceKind::Command,
        capability: Capability::SessionRead,
        effect: EffectClass::Read,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::NotApplicable,
            Support::Implemented {
                anchor: "TuiState（q 键退出）",
            },
            Support::Implemented {
                anchor: "handle_event（q/Esc → Quit）",
            },
            Support::NotApplicable,
        ],
    },
    // 独立传输入口（TRANSPORT_SPECIFIC，FR-607／608）：不属于四端对等判定，
    // 四端均不适用。
    SurfaceEntry {
        id: "ssh.transport",
        kind: SurfaceKind::Command,
        capability: Capability::SessionRead,
        effect: EffectClass::Read,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::NotApplicable,
            Support::NotApplicable,
            Support::NotApplicable,
            Support::NotApplicable,
        ],
    },
    // ── 视图面（FR-601／622；六视图 + 在线可见；CUI 按 CUI_COMMAND_SURFACE 排除） ──
    SurfaceEntry {
        id: "view.sessions",
        kind: SurfaceKind::View,
        capability: Capability::SessionRead,
        effect: EffectClass::Read,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Excluded {
                reason: ExclusionReason::CuiCommandSurface,
            },
            Support::Implemented {
                anchor: "LoopCore::prepare（ViewId::Sessions）",
            },
            Support::Implemented {
                anchor: "State::prepare（ViewId::Sessions）",
            },
            Support::Implemented {
                anchor: "GET /api/sessions",
            },
        ],
    },
    SurfaceEntry {
        id: "view.tasks",
        kind: SurfaceKind::View,
        capability: Capability::TaskRead,
        effect: EffectClass::Read,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Excluded {
                reason: ExclusionReason::CuiCommandSurface,
            },
            Support::Implemented {
                anchor: "LoopCore::prepare（ViewId::Tasks）",
            },
            Support::Implemented {
                anchor: "State::prepare（ViewId::Tasks）",
            },
            Support::Implemented {
                anchor: "GET /api/tasks",
            },
        ],
    },
    SurfaceEntry {
        id: "view.context",
        kind: SurfaceKind::View,
        capability: Capability::SessionRead,
        effect: EffectClass::Read,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Excluded {
                reason: ExclusionReason::CuiCommandSurface,
            },
            Support::Implemented {
                anchor: "LoopCore::prepare（ViewId::Context）",
            },
            Support::Implemented {
                anchor: "State::prepare（ViewId::Context）",
            },
            Support::Implemented {
                anchor: "GET /api/context?session=&budget=",
            },
        ],
    },
    SurfaceEntry {
        id: "view.events",
        kind: SurfaceKind::View,
        capability: Capability::SessionRead,
        effect: EffectClass::Read,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Excluded {
                reason: ExclusionReason::CuiCommandSurface,
            },
            Support::Implemented {
                anchor: "LoopCore::prepare（ViewId::Events）",
            },
            Support::Implemented {
                anchor: "State::prepare（ViewId::Events）",
            },
            Support::Implemented {
                anchor: "GET /api/events",
            },
        ],
    },
    SurfaceEntry {
        id: "view.spec",
        kind: SurfaceKind::View,
        capability: Capability::SessionRead,
        effect: EffectClass::Read,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Excluded {
                reason: ExclusionReason::CuiCommandSurface,
            },
            Support::Implemented {
                anchor: "LoopCore::prepare（ViewId::Spec）",
            },
            Support::Implemented {
                anchor: "State::prepare（ViewId::Spec）",
            },
            Support::Implemented {
                anchor: "GET /api/status",
            },
        ],
    },
    SurfaceEntry {
        id: "view.diff",
        kind: SurfaceKind::View,
        capability: Capability::SessionRead,
        effect: EffectClass::Read,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Excluded {
                reason: ExclusionReason::CuiCommandSurface,
            },
            Support::Implemented {
                anchor: "LoopCore::prepare（ViewId::Diff）",
            },
            Support::Implemented {
                anchor: "State::prepare（ViewId::Diff）",
            },
            Support::Implemented {
                anchor: "GET /api/diff?session=",
            },
        ],
    },
    // 在线可见（其他端更改；FR-605／624）：Web 经 WebSocket 推送；TUI 低频游标检查
    // （T178 已交付）；GUI 显式刷新（FR-711／714）。
    SurfaceEntry {
        id: "view.changes",
        kind: SurfaceKind::View,
        capability: Capability::SessionRead,
        effect: EffectClass::Read,
        confirmation: Confirmation::None,
        audited: false,
        frontends: [
            Support::Excluded {
                reason: ExclusionReason::CuiRequestOnly,
            },
            Support::Implemented {
                anchor: "LoopCore::on_idle（≥1000 ms 游标检查，推进才重绘）",
            },
            Support::Excluded {
                reason: ExclusionReason::GuiManualRefresh,
            },
            Support::Implemented { anchor: "GET /ws" },
        ],
    },
];

/// 按 SurfaceId 查找登记条目。
#[must_use]
pub fn find(id: &str) -> Option<&'static SurfaceEntry> {
    SURFACES.iter().find(|entry| entry.id == id)
}

/// 全量校验登记表：返回全部违例文本（空即通过）。
///
/// 检查项（Task 169.3）：SurfaceId 唯一且非空；能力在稳定能力白名单内；
/// 已实现必有前端锚点；已排除必有 ADR-032 理由码；预留必有非空里程碑；
/// 视图条目不得在 CUI 标记已实现（`CUI_COMMAND_SURFACE`）；视图条目至少被一个
/// 视图面端（TUI／GUI／Web）实现（孤儿视图条目即失败）。
#[must_use]
pub fn validate_entries(entries: &[SurfaceEntry]) -> Vec<String> {
    let mut violations = Vec::new();
    if entries.is_empty() {
        violations.push("登记表为空：至少登记一条 Surface（FR-619）".to_string());
        return violations;
    }
    let mut seen = BTreeSet::new();
    for (index, entry) in entries.iter().enumerate() {
        let label = if entry.id.is_empty() {
            format!("第 {index} 条（id 为空）")
        } else {
            entry.id.to_string()
        };
        if entry.id.is_empty() {
            violations.push(format!(
                "第 {index} 条 SurfaceId 为空（FR-619：稳定唯一标识）"
            ));
        }
        if !seen.insert(entry.id) {
            violations.push(format!("SurfaceId 重复：{}（登记表必须唯一）", entry.id));
        }
        if !KNOWN_CAPABILITIES.contains(&entry.capability.as_str()) {
            violations.push(format!(
                "{label} 能力 {} 不在稳定能力白名单（authz::Capability；新增能力须同 PR 扩展）",
                entry.capability.as_str()
            ));
        }
        for (frontend, support) in FRONTEND_ORDER.iter().zip(entry.frontends.iter()) {
            match *support {
                Support::Implemented { anchor } => {
                    if anchor.is_empty() {
                        violations.push(format!(
                            "{label} 在 {} 已实现但缺前端锚点（函数名或路由）",
                            frontend.as_str()
                        ));
                    }
                }
                Support::Excluded { reason } => {
                    if !ADR032_REASON_CODES.contains(&reason.as_str()) {
                        violations.push(format!(
                            "{label} 在 {} 的排除理由码 {} 不在 ADR-032 码表",
                            frontend.as_str(),
                            reason.as_str()
                        ));
                    }
                }
                Support::Reserved { milestone } => {
                    if milestone.is_empty() {
                        violations.push(format!(
                            "{label} 在 {} 预留但缺里程碑（ADR-032）",
                            frontend.as_str()
                        ));
                    }
                }
                Support::NotApplicable => {}
            }
        }
        if entry.kind == SurfaceKind::View {
            if matches!(entry.frontends[0], Support::Implemented { .. }) {
                violations.push(format!(
                    "{label} 为视图条目，CUI 不得标记已实现（CUI_COMMAND_SURFACE）"
                ));
            }
            let viewable = entry.frontends[1..4]
                .iter()
                .any(|support| matches!(support, Support::Implemented { .. }));
            if !viewable {
                violations.push(format!(
                    "{label} 为孤儿视图条目：TUI／GUI／Web 均未实现（FR-622）"
                ));
            }
        }
    }
    violations
}

/// 校验登记表 [`SURFACES`]：通过返回 `Ok(())`，否则返回全部违例。
///
/// # Errors
///
/// 登记表存在违例（重复 ID、缺锚点、缺里程碑等）时返回违例文本列表。
pub fn validate() -> Result<(), Vec<String>> {
    let violations = validate_entries(SURFACES);
    if violations.is_empty() {
        Ok(())
    } else {
        Err(violations)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_entry(id: &'static str) -> SurfaceEntry {
        SurfaceEntry {
            id,
            kind: SurfaceKind::Command,
            capability: Capability::SessionRead,
            effect: EffectClass::Read,
            confirmation: Confirmation::None,
            audited: false,
            frontends: [
                Support::Implemented {
                    anchor: "anchor-cui",
                },
                Support::Implemented {
                    anchor: "anchor-tui",
                },
                Support::Implemented {
                    anchor: "anchor-gui",
                },
                Support::Implemented {
                    anchor: "anchor-web",
                },
            ],
        }
    }

    #[test]
    fn registry_passes_validation() {
        assert_eq!(validate(), Ok(()), "登记表必须全量通过不变量校验");
    }

    #[test]
    fn registry_is_nonempty_and_ids_are_unique() {
        assert!(!SURFACES.is_empty());
        let ids: BTreeSet<&str> = SURFACES.iter().map(|entry| entry.id).collect();
        assert_eq!(ids.len(), SURFACES.len(), "SurfaceId 必须唯一");
        assert!(ids.iter().all(|id| !id.is_empty()));
    }

    /// 稳定能力白名单与 `authz::Capability` 一一对齐（ADR-040 决策 4：`settings.cert.read`
    /// ／`settings.cert.write` 已入白名单，供 Task 7 的登记条目引用）。
    #[test]
    fn known_capability_whitelist_matches_capability_enum() {
        let all = [
            Capability::SessionCreate,
            Capability::SessionRead,
            Capability::SessionRename,
            Capability::SessionArchive,
            Capability::SessionDelete,
            Capability::TaskRead,
            Capability::TaskRetry,
            Capability::TaskAbort,
            Capability::SpecAdvance,
            Capability::SettingsCertRead,
            Capability::SettingsCertWrite,
        ];
        assert_eq!(
            all.len(),
            KNOWN_CAPABILITIES.len(),
            "白名单必须与能力枚举一一对齐（新增能力须同 PR 扩展）"
        );
        for capability in all {
            assert!(
                KNOWN_CAPABILITIES.contains(&capability.as_str()),
                "能力 {} 必须在稳定能力白名单",
                capability.as_str()
            );
        }
        let unique: BTreeSet<&str> = KNOWN_CAPABILITIES.iter().copied().collect();
        assert_eq!(unique.len(), KNOWN_CAPABILITIES.len(), "白名单不得重复");
    }

    #[test]
    fn six_views_plus_online_visibility_are_view_kind() {
        let views: Vec<&str> = SURFACES
            .iter()
            .filter(|entry| entry.kind == SurfaceKind::View)
            .map(|entry| entry.id)
            .collect();
        assert_eq!(
            views,
            [
                // 证书设置页（T190）：仅在 Web 提供的视图面，不在六视图内；按登记顺序居前。
                "settings.cert.read",
                "view.sessions",
                "view.tasks",
                "view.context",
                "view.events",
                "view.spec",
                "view.diff",
                "view.changes"
            ],
            "视图面条目为证书设置页 + 六视图 + 在线可见（FR-601／622／624、ADR-040）"
        );
    }

    /// ADR-040 决策 4：证书设置两格仅在 Web 实现；CUI／TUI 按 `web_settings_only` 排除、
    /// GUI 按 `six_view_scope`／`gui_read_only_v1` 排除；写面为破坏族且强制审计。
    #[test]
    fn cert_settings_are_web_only_with_declared_reasons() {
        let read = find("settings.cert.read").expect("settings.cert.read 已登记");
        assert_eq!(read.kind, SurfaceKind::View);
        assert_eq!(read.capability, Capability::SettingsCertRead);
        assert_eq!(read.effect, EffectClass::Read);
        assert!(!read.audited, "读面不强制审计");
        for frontend in [FrontendId::Cui, FrontendId::Tui] {
            assert_eq!(
                read.support(frontend),
                &Support::Excluded {
                    reason: ExclusionReason::WebSettingsOnly
                },
                "{frontend:?} 必须按 web_settings_only 排除"
            );
        }
        assert_eq!(
            read.support(FrontendId::Gui),
            &Support::Excluded {
                reason: ExclusionReason::SixViewScope
            }
        );
        assert!(matches!(
            read.support(FrontendId::Web),
            Support::Implemented { anchor } if !anchor.is_empty()
        ));

        let write = find("settings.cert.write").expect("settings.cert.write 已登记");
        assert_eq!(write.kind, SurfaceKind::Command);
        assert_eq!(write.capability, Capability::SettingsCertWrite);
        assert_eq!(write.effect, EffectClass::Destructive);
        assert_eq!(write.confirmation, Confirmation::PreviewConfirm);
        assert!(write.audited, "证书变更必须写 tls_config_change 审计");
        for frontend in [FrontendId::Cui, FrontendId::Tui] {
            assert_eq!(
                write.support(frontend),
                &Support::Excluded {
                    reason: ExclusionReason::WebSettingsOnly
                }
            );
        }
        assert_eq!(
            write.support(FrontendId::Gui),
            &Support::Excluded {
                reason: ExclusionReason::GuiReadOnlyV1
            }
        );
        assert!(matches!(
            write.support(FrontendId::Web),
            Support::Implemented { anchor } if !anchor.is_empty()
        ));
    }

    /// T167：`llm.run`／`llm.cancel` 随 CUI／Web 接线翻转为已实现（真实锚点）；
    /// TUI 未接线保持预留（`M3-llm`），GUI 仍按 `gui_read_only_v1` 排除——
    /// 不得在无前端映射时宣称可用（ADR-032 决策 3／4）。
    #[test]
    fn llm_surfaces_track_wired_frontends() {
        for id in ["llm.run", "llm.cancel"] {
            let entry = find(id).expect("llm 面已登记");
            for frontend in [FrontendId::Cui, FrontendId::Web] {
                assert!(
                    matches!(
                        entry.support(frontend),
                        Support::Implemented { anchor } if !anchor.is_empty()
                    ),
                    "{id} 在 {} 必须已实现并带前端锚点，实际 {:?}",
                    frontend.as_str(),
                    entry.support(frontend)
                );
            }
            assert_eq!(
                entry.support(FrontendId::Tui),
                &Support::Reserved {
                    milestone: "M3-llm"
                },
                "{id} 在 TUI 仍未接线，必须保持预留"
            );
            assert_eq!(
                entry.support(FrontendId::Gui),
                &Support::Excluded {
                    reason: ExclusionReason::GuiReadOnlyV1
                }
            );
        }
    }

    /// GUI 全部业务写命令登记 `GUI_READ_ONLY_V1`（FR-711、Task 179.1）。
    #[test]
    fn gui_business_writes_are_excluded_with_read_only_v1() {
        for id in [
            "session.new",
            "session.rename",
            "session.archive",
            "session.delete",
            "task.retry",
            "task.abort",
            "db.purge",
            "llm.run",
            "llm.cancel",
        ] {
            let entry = find(id).expect(id);
            assert_eq!(
                entry.support(FrontendId::Gui),
                &Support::Excluded {
                    reason: ExclusionReason::GuiReadOnlyV1
                },
                "{id} 的 GUI 状态必须为 GUI_READ_ONLY_V1"
            );
        }
    }

    /// 破坏族语义（契约 §2）：delete/purge 预览＋确认＋审计；force-advance 必带理由。
    #[test]
    fn destructive_family_carries_confirmation_and_audit() {
        for id in ["session.delete", "db.purge"] {
            let entry = find(id).expect(id);
            assert_eq!(entry.effect, EffectClass::Destructive);
            assert_eq!(entry.confirmation, Confirmation::PreviewConfirm);
            assert!(entry.audited, "{id} 必须强制审计");
        }
        let force = find("spec.force-advance").expect("spec.force-advance");
        assert_eq!(force.confirmation, Confirmation::ReasonRequired);
        assert!(force.audited, "force-advance 审计含完整要素（FR-312）");
    }

    /// 对等基线抽查（T181 收口）：本变更交付的格已翻转为已实现；未交付者仍在预留队列。
    #[test]
    fn delivered_surfaces_are_implemented_and_pending_keep_milestones() {
        // 原 ➕（本变更新增）格：交付后同 PR 翻转为已实现并带前端锚点（ADR-032 判定表）。
        for (id, frontend) in [
            ("context.budget", FrontendId::Cui),
            ("runtime.list", FrontendId::Cui),
            ("view.context", FrontendId::Web),
            ("theme.switch", FrontendId::Web),
            ("theme.switch", FrontendId::Tui),
            ("view.changes", FrontendId::Tui),
            ("session.new", FrontendId::Tui),
            ("session.rename", FrontendId::Tui),
            ("session.archive", FrontendId::Tui),
            ("session.delete", FrontendId::Tui),
            ("task.retry", FrontendId::Tui),
            ("task.abort", FrontendId::Tui),
            ("db.purge", FrontendId::Tui),
            ("llm.run", FrontendId::Cui),
            ("llm.run", FrontendId::Web),
            ("llm.cancel", FrontendId::Cui),
            ("llm.cancel", FrontendId::Web),
        ] {
            let entry = find(id).unwrap_or_else(|| panic!("{id} 未登记"));
            assert!(
                matches!(entry.support(frontend), Support::Implemented { anchor } if !anchor.is_empty()),
                "{id} 在 {} 应随交付翻转为已实现（带锚点），实际 {:?}",
                frontend.as_str(),
                entry.support(frontend)
            );
        }
        // 未来里程碑：TUI 的 LLM 接线（M3-llm）与运行时装配（M8）保持预留（不得提前宣称可用）。
        for (id, frontend, milestone) in [
            ("llm.run", FrontendId::Tui, "M3-llm"),
            ("llm.cancel", FrontendId::Tui, "M3-llm"),
            ("runtime.status", FrontendId::Cui, "M8"),
            ("runtime.enable", FrontendId::Web, "M8"),
        ] {
            let entry = find(id).unwrap_or_else(|| panic!("{id} 未登记"));
            assert_eq!(
                entry.support(frontend),
                &Support::Reserved { milestone },
                "{id} 在 {} 应保持预留（{milestone}）",
                frontend.as_str()
            );
        }
    }

    /// 理由码全表对齐 ADR-032（未知理由码即失败）。
    #[test]
    fn exclusion_reasons_match_adr032_code_table() {
        let codes = [
            ExclusionReason::GuiReadOnlyV1,
            ExclusionReason::CliLocalAdmin,
            ExclusionReason::CuiCommandSurface,
            ExclusionReason::ViewSummaryOnly,
            ExclusionReason::CommandOutputOnly,
            ExclusionReason::CuiNoTheme,
            ExclusionReason::SixViewScope,
            ExclusionReason::CuiRequestOnly,
            ExclusionReason::GuiManualRefresh,
            ExclusionReason::TransportSpecific,
            ExclusionReason::WebSettingsOnly,
        ];
        for reason in codes {
            assert!(
                ADR032_REASON_CODES.contains(&reason.as_str()),
                "理由码 {} 必须在 ADR-032 码表",
                reason.as_str()
            );
        }
        assert_eq!(codes.len(), ADR032_REASON_CODES.len(), "码表必须一一对齐");
    }

    /// 负向自测：重复 ID 必须失败。
    #[test]
    fn duplicate_ids_fail() {
        let entries = [valid_entry("a"), valid_entry("a")];
        let violations = validate_entries(&entries);
        assert!(
            violations.iter().any(|text| text.contains("重复")),
            "重复 ID 必须报错：{violations:?}"
        );
    }

    /// 负向自测：空 ID 必须失败。
    #[test]
    fn empty_ids_fail() {
        let entries = [valid_entry("")];
        let violations = validate_entries(&entries);
        assert!(
            violations
                .iter()
                .any(|text| text.contains("SurfaceId 为空")),
            "空 ID 必须报错：{violations:?}"
        );
    }

    /// 负向自测：已实现缺锚点必须失败。
    #[test]
    fn implemented_without_anchor_fails() {
        let mut entry = valid_entry("x");
        entry.frontends[0] = Support::Implemented { anchor: "" };
        let violations = validate_entries(&[entry]);
        assert!(
            violations.iter().any(|text| text.contains("缺前端锚点")),
            "缺锚点必须报错：{violations:?}"
        );
    }

    /// 负向自测：预留缺里程碑必须失败。
    #[test]
    fn reserved_without_milestone_fails() {
        let mut entry = valid_entry("x");
        entry.frontends[1] = Support::Reserved { milestone: "" };
        let violations = validate_entries(&[entry]);
        assert!(
            violations.iter().any(|text| text.contains("缺里程碑")),
            "缺里程碑必须报错：{violations:?}"
        );
    }

    /// 负向自测：视图条目在 CUI 标记已实现必须失败。
    #[test]
    fn view_implemented_on_cui_fails() {
        let mut entry = valid_entry("view.x");
        entry.kind = SurfaceKind::View;
        let violations = validate_entries(&[entry]);
        assert!(
            violations
                .iter()
                .any(|text| text.contains("CUI 不得标记已实现")),
            "视图条目不得在 CUI 已实现：{violations:?}"
        );
    }

    /// 负向自测：孤儿视图条目（无任何视图面端实现）必须失败。
    #[test]
    fn orphan_view_entry_fails() {
        let mut entry = valid_entry("view.x");
        entry.kind = SurfaceKind::View;
        entry.frontends[0] = Support::Excluded {
            reason: ExclusionReason::CuiCommandSurface,
        };
        entry.frontends[1] = Support::Reserved { milestone: "T177" };
        entry.frontends[2] = Support::Excluded {
            reason: ExclusionReason::GuiReadOnlyV1,
        };
        entry.frontends[3] = Support::Excluded {
            reason: ExclusionReason::CuiCommandSurface,
        };
        let violations = validate_entries(&[entry]);
        assert!(
            violations.iter().any(|text| text.contains("孤儿视图条目")),
            "孤儿视图条目必须报错：{violations:?}"
        );
    }

    /// 负向自测：空登记表必须失败。
    #[test]
    fn empty_registry_fails() {
        let violations = validate_entries(&[]);
        assert!(
            violations.iter().any(|text| text.contains("登记表为空")),
            "空登记表必须报错：{violations:?}"
        );
    }

    /// `find` 只认已登记的 id。
    #[test]
    fn find_returns_registered_entries_only() {
        assert!(find("session.delete").is_some());
        assert!(find("view.diff").is_some());
        assert!(find("no.such.surface").is_none());
    }
}
