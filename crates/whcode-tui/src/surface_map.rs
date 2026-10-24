//! # TUI 动作 → SurfaceId 清单（Task 170.2；FR-619—FR-621、ADR-032）
//!
//! 把 TUI 键盘动作映射到 [`whcode_app::surface::SURFACES`] 的稳定 `SurfaceId`。
//! 写动作（会话 new／rename／archive／delete、任务 retry／abort、清理 purge）是
//! **预留动作**：由 Task 177 经 `whcode-app::commands` 门面实现，当前 `surface_of`
//! 仍返回正确 id，但登记表这些面的 TUI 状态为预留（不伪装已实现，FR-620）。
//! 主题循环（`t`）由 Task 178 交付；`view.changes` 的游标自动可见同样属 T178
//! （当前 `r` 手动刷新为其过渡形态）。
//!
//! 视图切换（`1`—`6`）映射到六视图条目（View 类）；`j`／`k` 会话选择是会话视图
//! 的导航动作，映射 `view.sessions`；`q` 退出映射应用生命周期动作 `app.quit`。

use whcode_app::view_source::ViewId;

/// TUI 动作（与真实键位一一对应；写动作当前为预留，由 T177 实现）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuiAction {
    /// 视图切换（`1`—`6`）。
    ViewSwitch(ViewId),
    /// 会话选择下移（`j`）。
    SessionNext,
    /// 会话选择上移（`k`）。
    SessionPrev,
    /// 显式刷新（`r`）。
    Refresh,
    /// 主题循环（`t`；预留动作，T178）。
    ThemeCycle,
    /// 退出（`q`）。
    Quit,
    /// 会话新建（预留动作，T177）。
    SessionNew,
    /// 会话重命名（预留动作，T177）。
    SessionRename,
    /// 会话归档（预留动作，T177）。
    SessionArchive,
    /// 会话删除（预留动作，T177；先预览后确认）。
    SessionDelete,
    /// 任务重试（预留动作，T177）。
    TaskRetry,
    /// 任务中止（预留动作，T177）。
    TaskAbort,
    /// 清理（预留动作，T177；先预览后确认）。
    Purge,
}

/// 把 TUI 动作映射到登记表 `SurfaceId`（穷尽匹配，无通配臂）。
#[must_use]
pub fn surface_of(action: TuiAction) -> &'static str {
    match action {
        TuiAction::ViewSwitch(view) => view_surface(view),
        TuiAction::SessionNext | TuiAction::SessionPrev => "view.sessions",
        TuiAction::Refresh => "view.changes",
        TuiAction::ThemeCycle => "theme.switch",
        TuiAction::Quit => "app.quit",
        TuiAction::SessionNew => "session.new",
        TuiAction::SessionRename => "session.rename",
        TuiAction::SessionArchive => "session.archive",
        TuiAction::SessionDelete => "session.delete",
        TuiAction::TaskRetry => "task.retry",
        TuiAction::TaskAbort => "task.abort",
        TuiAction::Purge => "db.purge",
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
    use whcode_app::surface::{FrontendId, Support, find};

    /// 全动作断言：映射存在，且 TUI 端状态为已实现或预留（以 SURFACES 为准）。
    #[test]
    fn every_action_maps_to_a_registered_surface() {
        let actions = [
            TuiAction::ViewSwitch(ViewId::Sessions),
            TuiAction::ViewSwitch(ViewId::Tasks),
            TuiAction::ViewSwitch(ViewId::Context),
            TuiAction::ViewSwitch(ViewId::Events),
            TuiAction::ViewSwitch(ViewId::Spec),
            TuiAction::ViewSwitch(ViewId::Diff),
            TuiAction::SessionNext,
            TuiAction::SessionPrev,
            TuiAction::Refresh,
            TuiAction::ThemeCycle,
            TuiAction::Quit,
            TuiAction::SessionNew,
            TuiAction::SessionRename,
            TuiAction::SessionArchive,
            TuiAction::SessionDelete,
            TuiAction::TaskRetry,
            TuiAction::TaskAbort,
            TuiAction::Purge,
        ];
        for action in actions {
            let id = surface_of(action);
            let entry = find(id).unwrap_or_else(|| panic!("{id} 未登记（映射必须是登记表条目）"));
            match entry.support(FrontendId::Tui) {
                Support::Implemented { .. } | Support::Reserved { .. } => {}
                other => panic!("动作 {id} 的 TUI 状态必须是已实现或预留，实际 {other:?}"),
            }
        }
    }

    /// 视图切换映射到六视图条目（全部已实现）。
    #[test]
    fn view_switch_maps_to_six_view_surfaces() {
        let cases = [
            (ViewId::Sessions, "view.sessions"),
            (ViewId::Tasks, "view.tasks"),
            (ViewId::Context, "view.context"),
            (ViewId::Events, "view.events"),
            (ViewId::Spec, "view.spec"),
            (ViewId::Diff, "view.diff"),
        ];
        for (view, id) in cases {
            assert_eq!(surface_of(TuiAction::ViewSwitch(view)), id);
            let entry = find(id).expect("已登记");
            assert!(
                matches!(entry.support(FrontendId::Tui), Support::Implemented { .. }),
                "{id} 的 TUI 状态必须为已实现（六视图已交付）"
            );
        }
    }

    /// 写动作与主题／在线可见为预留动作：映射正确且 TUI 端登记为预留（T177／T178）。
    #[test]
    fn write_and_pending_actions_stay_reserved() {
        let cases = [
            (TuiAction::SessionNew, "session.new"),
            (TuiAction::SessionRename, "session.rename"),
            (TuiAction::SessionArchive, "session.archive"),
            (TuiAction::SessionDelete, "session.delete"),
            (TuiAction::TaskRetry, "task.retry"),
            (TuiAction::TaskAbort, "task.abort"),
            (TuiAction::Purge, "db.purge"),
            (TuiAction::ThemeCycle, "theme.switch"),
            (TuiAction::Refresh, "view.changes"),
        ];
        for (action, id) in cases {
            assert_eq!(surface_of(action), id);
            let entry = find(id).expect("已登记");
            match entry.support(FrontendId::Tui) {
                Support::Reserved { .. } => {}
                other => panic!("预留动作 {id} 的 TUI 状态必须为预留，实际 {other:?}"),
            }
        }
    }

    /// 退出动作映射到应用生命周期动作面（已实现）。
    #[test]
    fn quit_maps_to_app_quit_surface() {
        assert_eq!(surface_of(TuiAction::Quit), "app.quit");
        let entry = find("app.quit").expect("app.quit 已登记");
        assert!(matches!(
            entry.support(FrontendId::Tui),
            Support::Implemented { .. }
        ));
    }
}
