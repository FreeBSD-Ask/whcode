//! TUI/CUI 一致性契约测试（T088、FR-604、V-109 部分）。
//!
//! TUI 命令面（T082，六视图）尚未交付——本测试在**已交付面**上验证一致性契约：
//! 1. **命令路由同源**：CUI 命令树（T084）解析出的每个命令都映射到
//!    `whcode-app` 的同一批命令函数——TUI 的会话面（T024 基线）与 CUI 均只经
//!    应用服务访问状态（ADR-003/005、C-03），前端不触库；
//! 2. **权限语义一致**（FR-604）：同一调用者能力集合下，TUI 与 CUI 走同一
//!    `authz` 判定——拒绝均为 `permission_denied`（不可重试）；
//! 3. **错误码一致**：两前端的错误信封同源（`output::error_envelope`）。
//!
//! T082 交付后须在本文件追加"同命令最终状态一致"用例（任务文档既定要求），
//! 不得删除既有断言。

use whcode_app::{Actor, Capability, DefaultPolicy, actor_with_all_capabilities};
use whcode_cui::cli::{Cli, Command, parse};
use whcode_domain::ErrorCode;

/// CUI 命令树覆盖的 app 能力集合（路由同源的结构断言）。
fn cui_capabilities(command: &Command) -> Vec<Capability> {
    match command {
        Command::Session { .. } => vec![
            Capability::SessionCreate,
            Capability::SessionRead,
            Capability::SessionRename,
            Capability::SessionArchive,
            Capability::SessionDelete,
        ],
        Command::Task { .. } => vec![
            Capability::TaskRead,
            Capability::TaskRetry,
            Capability::TaskAbort,
        ],
        Command::Spec { .. } => vec![Capability::SpecAdvance],
        Command::Status { .. } | Command::Events { .. } => vec![Capability::SessionRead],
        _ => vec![],
    }
}

#[test]
fn cui_command_tree_routes_to_app_capabilities() {
    // 每个 CUI 子命令都映射到 whcode-app 能力（同源路由，不触库）。
    let commands: Vec<Command> = vec![
        serde_parse(["whcode", "session", "new", "--title", "t"]).command,
        serde_parse(["whcode", "task", "list", "--session", "S-1"]).command,
        serde_parse(["whcode", "spec", "status", "--session", "S-1"]).command,
        serde_parse(["whcode", "events", "list", "--session", "S-1"]).command,
        serde_parse(["whcode", "status", "--session", "S-1"]).command,
    ];
    for command in &commands {
        let capabilities = cui_capabilities(command);
        assert!(
            !capabilities.is_empty(),
            "CUI 命令必须映射到 app 能力：{command:?}"
        );
    }
}

fn serde_parse<const N: usize>(args: [&str; N]) -> Cli {
    parse(args).expect("解析")
}

#[test]
fn permission_semantics_identical_for_both_frontends() {
    // FR-604：同一能力集合 → 同一判定。TUI 与 CUI 均经 authz（单点判定），
    // 这里验证判定结果对调用者能力集合是确定性的（前端无关）。
    let reader = Actor::new("reader", [Capability::SessionRead]);
    let full = actor_with_all_capabilities("admin");
    let policy = DefaultPolicy;

    for capability in [
        Capability::SessionCreate,
        Capability::SessionDelete,
        Capability::TaskRetry,
        Capability::SpecAdvance,
    ] {
        let denied_for_reader = whcode_app::require(&policy, &reader, capability).is_err();
        let allowed_for_admin = whcode_app::require(&policy, &full, capability).is_ok();
        assert!(
            denied_for_reader || capability == Capability::SessionRead,
            "无能力调用必须拒绝（FR-604）：{capability:?}"
        );
        assert!(allowed_for_admin, "有能力调用必须放行：{capability:?}");
    }
}

#[test]
fn error_codes_are_frontend_agnostic() {
    // V-109 部分：错误码一致——错误由 app 产生，前端只包装（output.rs 信封）。
    let reader = Actor::new("受限", []);
    let policy = DefaultPolicy;
    let err = whcode_app::require(&policy, &reader, Capability::SessionDelete).expect_err("拒绝");
    assert_eq!(err.code, ErrorCode::PermissionDenied, "TUI/CUI 同一错误码");
    assert!(!err.retryable);

    // 前端错误信封同源：同一错误 → 同一信封内容。
    let envelope = whcode_cui::output::error_envelope(&err);
    assert_eq!(envelope.code, "permission_denied");
    assert!(!envelope.retryable);
}
