//! # CUI 命令 → SurfaceId 清单（Task 170.1；FR-619—FR-621、ADR-032）
//!
//! 对 [`crate::cli::Command`] 全变体的**穷尽映射**（无通配臂）：新增命令变体若未映射
//! 即编译失败，保证“先登记后接线”（FR-619 判据）。映射目标是
//! [`whcode_app::surface::SURFACES`] 的稳定 `SurfaceId`；保留／未接线命令（`run`、
//! `events show`、`spec next/validate/analyze/converge/force-advance`、`runtime *`）
//! 映射到其预留 `SurfaceId`，由 Task 172 统一返回 `not_supported`（退出码 55）。
//!
//! 说明：顶层 `status` 与 `spec status` 同为 FR-314 状态快照面（同一应用服务
//! `build_snapshot`），故都映射 `spec.status`。

use crate::cli::{
    Command, ContextCommand, DbCommand, EventsCommand, FontCommand, LlmCommand, RuntimeCommand,
    SessionCommand, SpecCommand, TaskCommand,
};

/// 把 CUI 命令映射到登记表 `SurfaceId`（穷尽匹配，无通配臂）。
#[must_use]
pub fn surface_of(command: &Command) -> &'static str {
    match command {
        Command::Session { command } => match command {
            SessionCommand::New { .. } => "session.new",
            SessionCommand::List { .. } => "session.list",
            SessionCommand::Show { .. } => "session.show",
            SessionCommand::Rename { .. } => "session.rename",
            SessionCommand::Archive { .. } => "session.archive",
            SessionCommand::Delete { .. } => "session.delete",
        },
        // llm run／cancel（T167）：CUI 经 `whcode_app::llm` 真实执行（cui_dispatch）。
        Command::Llm { command } => match command {
            LlmCommand::Run { .. } => "llm.run",
            LlmCommand::Cancel { .. } => "llm.cancel",
        },
        Command::Status { .. } => "spec.status",
        Command::Task { command } => match command {
            TaskCommand::List { .. } => "task.list",
            TaskCommand::Show { .. } => "task.show",
            TaskCommand::Retry { .. } => "task.retry",
            TaskCommand::Abort { .. } => "task.abort",
        },
        Command::Context { command } => match command {
            ContextCommand::Budget { .. } => "context.budget",
        },
        Command::Spec { command } => match command {
            SpecCommand::Status { .. } => "spec.status",
            SpecCommand::Next { .. } => "spec.next",
            SpecCommand::Validate { .. } => "spec.validate",
            SpecCommand::Analyze { .. } => "spec.analyze",
            SpecCommand::Converge { .. } => "spec.converge",
            SpecCommand::ForceAdvance { .. } => "spec.force-advance",
        },
        Command::Events { command } => match command {
            EventsCommand::List { .. } => "events.list",
            // 应用层单事件接口未交付：预留（cui_dispatch 显式失败）。
            EventsCommand::Show { .. } => "events.show",
            EventsCommand::Export { .. } => "events.export",
        },
        Command::Db { command } => match command {
            DbCommand::Stats => "db.stats",
            DbCommand::Check => "db.check",
            DbCommand::Purge { .. } => "db.purge",
            DbCommand::Checkpoint { .. } => "db.checkpoint",
            DbCommand::Restore { .. } => "db.restore",
            DbCommand::Compact { .. } => "db.compact",
            DbCommand::Archive { .. } => "db.archive",
            DbCommand::Explain { .. } => "db.explain",
        },
        Command::Runtime { command } => match command {
            // runtime list 由 T173 接线（与 Web /api/runtimes 同形）；当前未接线。
            RuntimeCommand::List => "runtime.list",
            // runtime 其余子命令属 M8：预留。
            RuntimeCommand::Status { .. } => "runtime.status",
            RuntimeCommand::Enable { .. } => "runtime.enable",
            RuntimeCommand::Disable { .. } => "runtime.disable",
            RuntimeCommand::Install { .. } => "runtime.install",
        },
        Command::Font { command } => match command {
            FontCommand::Install { .. } => "font.install",
            FontCommand::Status => "font.status",
            FontCommand::Uninstall { .. } => "font.uninstall",
        },
        // serve／doctor 由装配根主入口处理（whcode_bin::serve::serve_command／whcode_bin::doctor）。
        Command::Serve { .. } => "serve",
        Command::Doctor { .. } => "doctor",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use whcode_app::surface::{FrontendId, Support, find};

    /// 全变体抽样断言：映射存在，且 CUI 端状态为已实现或预留（CUI 命令面无排除）。
    fn assert_mapped(command: &Command) {
        let id = surface_of(command);
        let entry = find(id).unwrap_or_else(|| panic!("{id} 未登记（映射必须是登记表条目）"));
        match entry.support(FrontendId::Cui) {
            Support::Implemented { .. } | Support::Reserved { .. } => {}
            other => panic!("命令 {id} 的 CUI 状态必须是已实现或预留，实际 {other:?}"),
        }
    }

    #[test]
    fn every_command_variant_maps_to_a_registered_surface() {
        let commands = [
            Command::Session {
                command: SessionCommand::New {
                    title: "t".into(),
                    model: None,
                },
            },
            Command::Session {
                command: SessionCommand::List { all: false },
            },
            Command::Session {
                command: SessionCommand::Show {
                    session: "S".into(),
                },
            },
            Command::Session {
                command: SessionCommand::Rename {
                    session: "S".into(),
                    title: "t".into(),
                },
            },
            Command::Session {
                command: SessionCommand::Archive {
                    session: "S".into(),
                    unarchive: false,
                    dry_run: false,
                    yes: false,
                },
            },
            Command::Session {
                command: SessionCommand::Delete {
                    session: "S".into(),
                    dry_run: false,
                    yes: false,
                },
            },
            llm_run(),
            llm_cancel(),
            Command::Status {
                session: "S".into(),
            },
            Command::Task {
                command: TaskCommand::List {
                    session: "S".into(),
                },
            },
            Command::Task {
                command: TaskCommand::Show { task: "T".into() },
            },
            Command::Task {
                command: TaskCommand::Retry { task: "T".into() },
            },
            Command::Task {
                command: TaskCommand::Abort { task: "T".into() },
            },
            Command::Context {
                command: ContextCommand::Budget {
                    session: "S".into(),
                    budget: None,
                },
            },
            Command::Spec {
                command: SpecCommand::Status {
                    session: "S".into(),
                },
            },
            Command::Spec {
                command: SpecCommand::Next {
                    session: "S".into(),
                },
            },
            Command::Spec {
                command: SpecCommand::Validate {
                    session: "S".into(),
                },
            },
            Command::Spec {
                command: SpecCommand::Analyze {
                    session: "S".into(),
                },
            },
            Command::Spec {
                command: SpecCommand::Converge {
                    session: "S".into(),
                },
            },
            Command::Spec {
                command: SpecCommand::ForceAdvance {
                    session: "S".into(),
                    to: "plan".into(),
                    reason: "r".into(),
                },
            },
            Command::Events {
                command: EventsCommand::List {
                    session: "S".into(),
                    after: 0,
                },
            },
            Command::Events {
                command: EventsCommand::Show { event: "E".into() },
            },
            Command::Events {
                command: EventsCommand::Export {
                    session: "S".into(),
                    after: 0,
                    format: "json".into(),
                },
            },
            Command::Db {
                command: DbCommand::Stats,
            },
            Command::Db {
                command: DbCommand::Check,
            },
            Command::Db {
                command: DbCommand::Purge {
                    before: 0,
                    dry_run: false,
                    yes: false,
                },
            },
            Command::Db {
                command: DbCommand::Checkpoint {
                    threshold: None,
                    autocheckpoint: None,
                },
            },
            Command::Db {
                command: DbCommand::Restore {
                    from: "p".into(),
                    yes: false,
                },
            },
            Command::Db {
                command: DbCommand::Compact {
                    dry_run: false,
                    yes: false,
                },
            },
            Command::Db {
                command: DbCommand::Archive { export: "p".into() },
            },
            Command::Db {
                command: DbCommand::Explain {
                    query: "SELECT 1".into(),
                },
            },
            Command::Runtime {
                command: RuntimeCommand::List,
            },
            Command::Runtime {
                command: RuntimeCommand::Status { name: "js".into() },
            },
            Command::Runtime {
                command: RuntimeCommand::Enable { name: "js".into() },
            },
            Command::Runtime {
                command: RuntimeCommand::Disable { name: "js".into() },
            },
            Command::Runtime {
                command: RuntimeCommand::Install { name: "js".into() },
            },
            Command::Font {
                command: crate::cli::FontCommand::Install {
                    from: None,
                    dry_run: false,
                },
            },
            Command::Font {
                command: crate::cli::FontCommand::Status,
            },
            Command::Font {
                command: crate::cli::FontCommand::Uninstall { dry_run: false },
            },
            Command::Serve {
                web: false,
                ssh: false,
                bind: "127.0.0.1:8080".into(),
            },
            Command::Doctor { json: false },
        ];
        for command in &commands {
            assert_mapped(command);
        }
    }

    /// `llm run`／`llm cancel` 的 CUI 形态（T167）：映射正确且 CUI 端已实现。
    #[test]
    fn llm_commands_map_to_implemented_surfaces() {
        for (command, id) in [(llm_run(), "llm.run"), (llm_cancel(), "llm.cancel")] {
            assert_eq!(surface_of(&command), id);
            let entry = find(id).expect("已登记");
            match entry.support(FrontendId::Cui) {
                Support::Implemented { anchor } if !anchor.is_empty() => {}
                other => panic!("{id} 的 CUI 状态必须为已实现（带锚点），实际 {other:?}"),
            }
        }
    }

    fn llm_run() -> Command {
        Command::Llm {
            command: LlmCommand::Run {
                session: "S".into(),
                provider: "p".into(),
                prompt: "hello".into(),
                model: None,
                max_tokens: None,
                request_id: None,
                trace_level: "redacted".into(),
            },
        }
    }

    fn llm_cancel() -> Command {
        Command::Llm {
            command: LlmCommand::Cancel {
                request: "req-1".into(),
            },
        }
    }

    /// 保留命令映射到其预留面（状态为预留，而非伪装已实现）。
    #[test]
    fn retained_commands_map_to_reserved_surfaces() {
        let reserved = [
            Command::Events {
                command: EventsCommand::Show { event: "E".into() },
            },
            Command::Spec {
                command: SpecCommand::Next {
                    session: "S".into(),
                },
            },
            Command::Spec {
                command: SpecCommand::ForceAdvance {
                    session: "S".into(),
                    to: "plan".into(),
                    reason: "r".into(),
                },
            },
            Command::Runtime {
                command: RuntimeCommand::Install { name: "js".into() },
            },
        ];
        for command in &reserved {
            let id = surface_of(command);
            let entry = find(id).expect("已登记");
            assert!(
                matches!(entry.support(FrontendId::Cui), Support::Reserved { .. }),
                "{id} 的 CUI 状态必须为预留"
            );
        }
    }

    /// 已实现命令抽查：锚点语义与登记表一致。
    #[test]
    fn implemented_commands_map_to_implemented_surfaces() {
        let implemented = [
            Command::Session {
                command: SessionCommand::Delete {
                    session: "S".into(),
                    dry_run: false,
                    yes: false,
                },
            },
            Command::Db {
                command: DbCommand::Purge {
                    before: 0,
                    dry_run: false,
                    yes: false,
                },
            },
            Command::Events {
                command: EventsCommand::Export {
                    session: "S".into(),
                    after: 0,
                    format: "json".into(),
                },
            },
            Command::Font {
                command: crate::cli::FontCommand::Status,
            },
            Command::Context {
                command: crate::cli::ContextCommand::Budget {
                    session: "S".into(),
                    budget: None,
                },
            },
            Command::Runtime {
                command: RuntimeCommand::List,
            },
            Command::Doctor { json: false },
        ];
        for command in &implemented {
            let id = surface_of(command);
            let entry = find(id).expect("已登记");
            assert!(
                matches!(entry.support(FrontendId::Cui), Support::Implemented { .. }),
                "{id} 的 CUI 状态必须为已实现"
            );
        }
    }
}
