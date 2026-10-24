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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use whcode_app::surface::{EffectClass, ExclusionReason, FrontendId, SURFACES, Support, find};

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
            GuiAction::NextSession,
            GuiAction::PrevSession,
            GuiAction::Refresh,
            GuiAction::CycleTheme,
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
            GuiAction::NextSession,
            GuiAction::PrevSession,
            GuiAction::Refresh,
            GuiAction::CycleTheme,
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

    /// Task 179：登记表中 GUI 已实现的条目必须全部为读面（GUI 无写能力）。
    #[test]
    fn gui_implemented_surfaces_are_read_only() {
        for entry in SURFACES {
            if matches!(entry.support(FrontendId::Gui), Support::Implemented { .. }) {
                assert_eq!(
                    entry.effect,
                    EffectClass::Read,
                    "{} 在 GUI 已实现但不是读面（FR-711 只读 v1）",
                    entry.id
                );
            }
        }
    }

    /// Task 179：自动刷新登记为 `gui_manual_refresh`（显式 `r` 刷新为其 GUI 形态）。
    #[test]
    fn online_visibility_is_registered_as_manual_refresh() {
        let entry = find("view.changes").expect("view.changes 已登记");
        assert_eq!(
            entry.support(FrontendId::Gui),
            &Support::Excluded {
                reason: ExclusionReason::GuiManualRefresh
            }
        );
        assert_eq!(surface_of(GuiAction::Refresh), "view.changes");
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
        // FR-713 四主题：精确名称与四步回原位（色值断言在 lib.rs theme 测试）。
        let names = [
            crate::GuiTheme::EinkDay,
            crate::GuiTheme::EinkNight,
            crate::GuiTheme::AmoledNight,
            crate::GuiTheme::TerminalGreen,
        ];
        for theme in names {
            assert_eq!(crate::GuiTheme::parse(theme.name()), Some(theme));
        }
        let mut theme = crate::GuiTheme::EinkDay;
        for _ in 0..4 {
            theme = theme.next();
        }
        assert_eq!(theme, crate::GuiTheme::EinkDay, "四步必须回原位");
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
        let runtime = find("runtime.list").expect("runtime.list 已登记");
        assert_eq!(
            runtime.support(FrontendId::Gui),
            &Support::Excluded {
                reason: ExclusionReason::SixViewScope
            }
        );
    }
}
