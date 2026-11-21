//! # GUI 只读动作 → SurfaceId 清单（Task 170.3／Task 179；FR-711、ADR-032）
//!
//! 原生 GUI 为**只读 v1**（FR-711／ADR-026／ADR-027）：本清单只含读动作——视图切换
//! （`1`—`6`）、会话选择（`j`／`k`）、显式刷新（`r`）、主题循环（`t`）、退出（`q`／Esc）。
//! 全部写命令按登记表排除：业务写命令理由码 `gui_read_only_v1`，本地管理命令为
//! `cli_local_admin`（对等基线表逐行口径），运行时管理为 `six_view_scope`。
//!
//! 刷新语义（Task 179.2）：GUI 无定时器与自动刷新，`view.changes` 在 GUI 登记为
//! `gui_manual_refresh`——`r` 键的显式刷新是该面的 GUI 形态（FR-711／714）。
//! 四主题入口与 FR-713 一致：`t` 键循环 `eink-day`／`eink-night`／`amoled-night`／
//! `terminal-green`，切换不要求重启（[`crate::GuiTheme`]）。
//!
//! 本模块是纯数据清单（Task 179.3 允许的范围）：不新增 GUI 业务行为、不做 IO、
//! 不改变既有渲染与输入路径。

use whcode_app::view_source::ViewId;

/// GUI 读动作（与 [`crate::handle_event`] 的既有键位一一对应；无任何写动作）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuiAction {
    /// 视图切换（`1`—`6`）。
    SelectView(ViewId),
    /// 会话选择下移（`j`）。
    NextSession,
    /// 会话选择上移（`k`）。
    PrevSession,
    /// 显式刷新（`r`；无自动刷新，FR-714）。
    Refresh,
    /// 主题循环（`t`；FR-713）。
    CycleTheme,
    /// 打开／关闭高级面板（`s`；T258／FR-749／750／751）。
    OpenPanels,
    /// 撤销一条本地审批规则（面板内按钮；FR-751）。
    RevokeRule,
    /// 会话分叉（T393／FR-769：分叉序号输入面板）。
    SessionFork,
    /// 对话回退（T393／FR-775：目标序号输入面板）。
    SessionRewind,
    /// 前像恢复（T393／FR-776：执行 ID 输入→预览→确认）。
    SessionRestore,
    /// 计划模式切换（T393／FR-774：on／off 输入面板）。
    SessionPlanSet,
    /// 模型目录（T393／llm.models：对话视图按钮）。
    ModelCatalogue,
    /// 事件详情（ADR-091／FR-799：`e` 键／事件行点击 → events.show）。
    EventDetail,
    /// Spec 门禁摘要（ADR-091／FR-799：`v`／`z`／`o` → spec.* 只读面）。
    SpecGateValidate,
    SpecGateAnalyze,
    SpecGateConverge,
    /// 退出（`q`／Esc）。
    Quit,
}

/// 把 GUI 动作映射到登记表 `SurfaceId`（穷尽匹配，无通配臂）。
#[must_use]
pub fn surface_of(action: GuiAction) -> &'static str {
    match action {
        GuiAction::SelectView(view) => view_surface(view),
        GuiAction::NextSession | GuiAction::PrevSession => "view.sessions",
        GuiAction::Refresh => "view.changes",
        GuiAction::CycleTheme => "theme.switch",
        GuiAction::OpenPanels => "settings.subagents",
        GuiAction::RevokeRule => "settings.rules.revoke",
        GuiAction::SessionFork => "session.fork",
        GuiAction::SessionRewind => "session.rewind",
        GuiAction::SessionRestore => "session.restore",
        GuiAction::SessionPlanSet => "session.plan",
        GuiAction::ModelCatalogue => "llm.models",
        GuiAction::EventDetail => "events.show",
        GuiAction::SpecGateValidate => "spec.validate",
        GuiAction::SpecGateAnalyze => "spec.analyze",
        GuiAction::SpecGateConverge => "spec.converge",
        GuiAction::Quit => "app.quit",
    }
}

/// 六视图 → 视图面 SurfaceId（穷尽匹配；新增 `ViewId` 即编译失败）。
#[must_use]
fn view_surface(view: ViewId) -> &'static str {
    match view {
        ViewId::Sessions => "view.sessions",
        ViewId::Tasks => "view.tasks",
        ViewId::Context => "view.context",
        ViewId::Events => "view.events",
        ViewId::Spec => "view.spec",
        ViewId::Diff => "view.diff",
        ViewId::Chat => "view.chat",
        ViewId::Ledger => "view.ledger",
        ViewId::Preimages => "view.preimages",
        ViewId::Coherence => "view.coherence",
        ViewId::Runtime => "view.runtime",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use whcode_app::surface::{EffectClass, FrontendId, SURFACES, Support, find};

    /// 全动作断言：映射存在且 GUI 端状态与登记表一致。
    #[test]
    fn every_action_maps_to_a_registered_surface() {
        let actions = [
            GuiAction::SelectView(ViewId::Sessions),
            GuiAction::SelectView(ViewId::Tasks),
            GuiAction::SelectView(ViewId::Context),
            GuiAction::SelectView(ViewId::Events),
            GuiAction::SelectView(ViewId::Spec),
            GuiAction::SelectView(ViewId::Diff),
            GuiAction::SelectView(ViewId::Chat),
            GuiAction::NextSession,
            GuiAction::PrevSession,
            GuiAction::Refresh,
            GuiAction::CycleTheme,
            GuiAction::OpenPanels,
            GuiAction::RevokeRule,
            GuiAction::SessionFork,
            GuiAction::SessionRewind,
            GuiAction::SessionRestore,
            GuiAction::SessionPlanSet,
            GuiAction::ModelCatalogue,
            GuiAction::EventDetail,
            GuiAction::SpecGateValidate,
            GuiAction::SpecGateAnalyze,
            GuiAction::SpecGateConverge,
            GuiAction::Quit,
        ];
        for action in actions {
            let id = surface_of(action);
            let entry = find(id).unwrap_or_else(|| panic!("{id} 未登记（映射必须是登记表条目）"));
            match entry.support(FrontendId::Gui) {
                Support::Implemented { .. } | Support::Excluded { .. } => {}
                other => panic!("动作 {id} 的 GUI 状态异常：{other:?}"),
            }
        }
    }

    /// Task 179：GUI 清单不含任何写／破坏性动作（只读 v1，FR-711）。
    #[test]
    fn gui_actions_never_reach_write_or_destructive_surfaces() {
        let actions = [
            GuiAction::SelectView(ViewId::Sessions),
            GuiAction::SelectView(ViewId::Tasks),
            GuiAction::SelectView(ViewId::Context),
            GuiAction::SelectView(ViewId::Events),
            GuiAction::SelectView(ViewId::Spec),
            GuiAction::SelectView(ViewId::Diff),
            GuiAction::SelectView(ViewId::Chat),
            GuiAction::NextSession,
            GuiAction::PrevSession,
            GuiAction::Refresh,
            GuiAction::CycleTheme,
            GuiAction::OpenPanels,
            GuiAction::Quit,
        ];
        for action in actions {
            let id = surface_of(action);
            let entry = find(id).expect("已登记");
            assert_eq!(
                entry.effect,
                EffectClass::Read,
                "GUI 动作 {id} 不得指向写／破坏性面（FR-711）"
            );
        }
    }

    /// ADR-056／092：GUI 已实现条目 = 读面 ∪ 白名单写面（对话、会话/任务/清理与 SDD 阶段推进）。
    /// 白名单外的写/破坏性面不得在 GUI 出现已实现，如 `settings.cert.write`。
    #[test]
    fn gui_implemented_surfaces_are_reads_or_whitelisted_writes() {
        const WRITES: [&str; 22] = [
            "session.new",
            "session.rename",
            "session.archive",
            "session.delete",
            "task.retry",
            "task.abort",
            "db.purge",
            "llm.run",
            "llm.cancel",
            "settings.rules.revoke",
            // T393（ADR-089 批 4）：fork／rewind／restore／plan 写面入白名单。
            "session.fork",
            "session.rewind",
            "session.restore",
            "session.plan",
            // ADR-092／FR-800 批 1：SDD 阶段推进两面入白名单（reason 必填＋审计同源）。
            "spec.next",
            "spec.force-advance",
            // ADR-092／FR-800 批 2：运行时管理三面入白名单（审计由 app 落表，不联网）。
            "runtime.enable",
            "runtime.disable",
            "runtime.install",
            // ADR-092／FR-800 批 3：审批规则写入与事件导出入白名单。
            "settings.rules.write",
            "events.export",
            // ADR-092／FR-800 批 3：证书写面入白名单（Destructive，降档确认＋审计）。
            "settings.cert.write",
        ];
        for entry in SURFACES {
            if !matches!(entry.support(FrontendId::Gui), Support::Implemented { .. }) {
                continue;
            }
            let whitelisted = WRITES.contains(&entry.id);
            assert!(
                entry.effect == EffectClass::Read || whitelisted,
                "{} 在 GUI 已实现但既非读面也不在 ADR-056 写白名单",
                entry.id
            );
            if whitelisted {
                assert!(
                    matches!(
                        entry.support(FrontendId::Gui),
                        Support::Implemented { anchor } if anchor.contains("whcode_gui::State::")
                    ),
                    "{} 的 GUI 锚点必须指向 GUI 真实函数（ADR-056）",
                    entry.id
                );
            }
        }
    }

    /// ADR-092／FR-800：view.changes 由排除正名为已实现——`r` 键显式刷新是真实
    /// GUI 锚点；FR-714 禁止周期定时器不变（无自动刷新，不伪造“在线可见”）。
    #[test]
    fn online_visibility_is_registered_as_explicit_refresh() {
        let entry = find("view.changes").expect("view.changes 已登记");
        assert!(matches!(
            entry.support(FrontendId::Gui),
            Support::Implemented { anchor } if anchor.contains("GuiAction::Refresh")
        ));
        assert_eq!(surface_of(GuiAction::Refresh), "view.changes");
        assert_eq!(entry.effect, EffectClass::Read);
    }

    /// Task 179：四主题入口与 FR-713 一致——`t` 键循环四套精确色值主题，不要求重启。
    #[test]
    fn theme_entry_matches_fr713_and_t_key() {
        let entry = find("theme.switch").expect("theme.switch 已登记");
        assert_eq!(surface_of(GuiAction::CycleTheme), "theme.switch");
        assert!(matches!(
            entry.support(FrontendId::Gui),
            Support::Implemented { .. }
        ));
        // FR-713＋FR-739 六主题：精确名称与六步回原位（色值断言在 lib.rs theme 测试）。
        let names = [
            crate::GuiTheme::EinkDay,
            crate::GuiTheme::EinkNight,
            crate::GuiTheme::AmoledNight,
            crate::GuiTheme::TerminalGreen,
            crate::GuiTheme::WhcodeDay,
            crate::GuiTheme::WhcodeNight,
        ];
        for theme in names {
            assert_eq!(crate::GuiTheme::parse(theme.name()), Some(theme));
        }
        let mut theme = crate::GuiTheme::EinkDay;
        for _ in 0..6 {
            theme = theme.next();
        }
        assert_eq!(theme, crate::GuiTheme::EinkDay, "六步必须回原位");
    }

    /// Task 179：GUI 清单只映射六视图与显式刷新、主题、退出，不出现运行时等被
    /// `six_view_scope` 排除的面。
    #[test]
    fn gui_actions_stay_within_six_view_scope() {
        for action in [
            GuiAction::NextSession,
            GuiAction::PrevSession,
            GuiAction::Quit,
        ] {
            let id = surface_of(action);
            assert!(find(id).is_some(), "{id} 必须已登记");
        }
        // ADR-092／FR-800 批 2：runtime.list 的 GUI 面已由排除翻转为已实现，
        // 该测试保留“其余动作仍在范围内”的语义，不再断言 runtime.list 排除。
        let runtime = find("runtime.list").expect("runtime.list 已登记");
        assert!(
            matches!(
                runtime.support(FrontendId::Gui),
                Support::Implemented { .. }
            ),
            "runtime.list 的 GUI 面应已实现（ADR-092 批 2）"
        );
    }

    /// ADR-091／FR-799：4 个 Read 面的 GUI 状态必须是已实现（锚点指向真实函数），
    /// 且经 GuiAction 可达；锁定翻转不回退。
    #[test]
    fn gui_read_surfaces_are_implemented_and_reachable() {
        for (action, id) in [
            (GuiAction::EventDetail, "events.show"),
            (GuiAction::SpecGateValidate, "spec.validate"),
            (GuiAction::SpecGateAnalyze, "spec.analyze"),
            (GuiAction::SpecGateConverge, "spec.converge"),
        ] {
            assert_eq!(surface_of(action), id);
            let entry = find(id).unwrap_or_else(|| panic!("{id} 未登记"));
            assert_eq!(entry.effect, EffectClass::Read, "{id} 必须是 Read 面");
            match entry.support(FrontendId::Gui) {
                Support::Implemented { anchor } => {
                    assert!(!anchor.is_empty(), "{id} 的 GUI 锚点不得为空");
                }
                other => panic!("{id} 的 GUI 状态异常：{other:?}"),
            }
        }
    }
}
