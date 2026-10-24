//! 受限 WHCode 入口策略（T097、FR-608、§10.2、ADR-030）。
//!
//! SSH 会话可执行的命令只有两类：
//! 1. **WHCode 受限命令集**（[`WhCodeCommand`]）——经 [`crate::adapter`] 走 `whcode-app`
//!    应用服务执行；每条命令映射到一个应用能力（[`WhCodeCommand::required_capability`]），
//!    调用者缺能力即在策略层结构化拒绝（应用服务层 [`whcode_app::require`] 会再判一次，
//!    策略层是前置过滤，**不得**比应用层放得更宽）。
//! 2. **系统 Shell**（[`Decision::SystemShell`]）——显式独立权限，默认关闭：需要
//!    [`SshPolicy::allow_shell`] 配置开关打开**且**调用者出现在 [`SshPolicy::shell_actors`]
//!    显式授权名单。两个条件缺一即拒。能力表达方式的选择（任务书允许二选一）：
//!    `whcode_app::Capability` 是九项应用能力枚举，加“Shell”变体要改 `whcode-app`
//!    （不在本任务文件边界内），故以**配置开关 + 显式授权名单**在 SSH 策略层表达；
//!    语义等价于“专门能力”：默认值可见（全关）、可改（编辑配置）、可撤销（移出名单）。
//!
//! 白名单外与被拒请求一律 [`Decision::Denied`]（携带结构化原因）；执行层把它翻译成
//! 非 0 退出码 + stderr 说明，不 panic、不静默降级。
//!
//! 本模块是纯函数判定，无状态、无 IO——审计由执行层（[`crate::adapter::AuditLog`]）记录。

use std::collections::BTreeSet;

use whcode_app::Capability;
use whcode_domain::ErrorCode;

/// 策略拒绝的固定退出码（非 0；沿用“命令不可执行”惯例 126，仅作为结构化信号）。
pub const EXIT_POLICY_DENIED: i32 = 126;

/// SSH 策略配置：Shell 显式授权（默认全关——FR-608 的安全默认）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SshPolicy {
    /// 是否允许“显式授权的系统 Shell 请求”。`false`（默认）时任何 Shell 请求都被拒。
    pub allow_shell: bool,
    /// 被显式授予 Shell 权限的调用者（Actor id）名单。开关打开时名单之外同样拒绝。
    pub shell_actors: BTreeSet<String>,
}

impl SshPolicy {
    /// 全关策略（默认入口无 Shell）。
    #[must_use]
    pub fn denied() -> Self {
        Self::default()
    }

    /// 构造显式授权名单。
    #[must_use]
    pub fn shell_for(actor_ids: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            allow_shell: true,
            shell_actors: actor_ids.into_iter().map(Into::into).collect(),
        }
    }
}

/// WHCode 受限命令（白名单内；经应用服务执行）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WhCodeCommand {
    /// `sessions [--archived]`：列出会话。
    SessionList {
        /// 是否包含归档会话。
        include_archived: bool,
    },
    /// `session show <id>`：查看会话元数据。
    SessionShow {
        /// 会话 ID。
        session_id: String,
    },
    /// `session new <标题…>`：创建会话（会话级写命令，按 authz 判权）。
    SessionNew {
        /// 会话标题（命令行剩余部分，去首尾空白）。
        title: String,
    },
    /// `tasks [<session-id>]`：任务总览（省略会话为工作区级）。
    TaskOverview {
        /// 会话范围；`None` 为工作区级。
        session_id: Option<String>,
    },
    /// `task <id>`：任务详情。
    TaskDetail {
        /// 任务 ID。
        task_id: String,
    },
}

impl WhCodeCommand {
    /// 执行该命令所需的应用能力。
    #[must_use]
    pub fn required_capability(&self) -> Capability {
        match self {
            Self::SessionList { .. } | Self::SessionShow { .. } => Capability::SessionRead,
            Self::SessionNew { .. } => Capability::SessionCreate,
            Self::TaskOverview { .. } | Self::TaskDetail { .. } => Capability::TaskRead,
        }
    }

    /// 审计摘要（命令形态，不含数据内容）。
    #[must_use]
    pub fn summary(&self) -> String {
        match self {
            Self::SessionList { include_archived } => {
                if *include_archived {
                    "sessions --archived".to_string()
                } else {
                    "sessions".to_string()
                }
            }
            Self::SessionShow { .. } => "session show <id>".to_string(),
            Self::SessionNew { .. } => "session new <title>".to_string(),
            Self::TaskOverview { session_id } => {
                if session_id.is_some() {
                    "tasks <session>".to_string()
                } else {
                    "tasks".to_string()
                }
            }
            Self::TaskDetail { .. } => "task <id>".to_string(),
        }
    }
}

/// 策略判定结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// 白名单内的 WHCode 命令：交执行层经应用服务执行。
    WhCode(WhCodeCommand),
    /// 显式授权的系统 Shell 请求。**M7 只交付权限判定与审计**；实际执行属 M8 运行时
    /// payload（ADR-024），执行层返回结构化的“未实现”说明，不启动任何子进程。
    SystemShell {
        /// 原始命令行（审计用；执行不在 M7 发生）。
        command: String,
    },
    /// 结构化拒绝（白名单外、能力缺失、Shell 未授权等）。
    Denied {
        /// 拒绝原因（面向 stderr 的可操作说明）。
        reason: String,
        /// 结构化错误码（`invalid_state` = 用法错误；`permission_denied` = 越权）。
        code: ErrorCode,
    },
}

/// 判定一条 SSH 命令行。
///
/// 语法（首 token 决定命令；参数按空白切分，`session new` 的标题取剩余整行）：
/// `sessions`、`sessions --archived`、`session show <id>`、`session new <标题…>`、
/// `tasks`、`tasks <session-id>`、`task <id>`、`shell <任意…>`（显式权限）。
#[must_use]
pub fn classify(policy: &SshPolicy, actor: &whcode_app::Actor, command: &str) -> Decision {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return Decision::Denied {
            reason: "命令为空：可用命令见白名单（sessions / session show|new / tasks / task）"
                .to_string(),
            code: ErrorCode::InvalidState,
        };
    }
    let mut tokens = trimmed.split_whitespace();
    let head = tokens.next().unwrap_or_default();
    match head {
        "sessions" => finish_whcode(
            actor,
            WhCodeCommand::SessionList {
                include_archived: match tokens.next() {
                    None => false,
                    Some("--archived") => true,
                    Some(other) => {
                        return unknown_usage(&format!("sessions {other}"));
                    }
                },
            },
        ),
        "session" => match (tokens.next(), tokens.clone().next()) {
            (Some("show"), Some(session_id)) if tokens.count() == 1 => finish_whcode(
                actor,
                WhCodeCommand::SessionShow {
                    session_id: session_id.to_string(),
                },
            ),
            (Some("show"), _) => unknown_usage("session show 需要恰好一个会话 ID"),
            (Some("new"), _) => {
                // 标题取剩余整行并只去首尾空白（内部空格属于用户输入，不得折叠）。
                let title = trimmed
                    .strip_prefix("session")
                    .unwrap_or_default()
                    .trim_start()
                    .strip_prefix("new")
                    .unwrap_or_default()
                    .trim();
                if title.is_empty() {
                    return unknown_usage("session new 需要非空标题：session new <标题…>");
                }
                finish_whcode(
                    actor,
                    WhCodeCommand::SessionNew {
                        title: title.to_string(),
                    },
                )
            }
            (Some(other), _) => unknown_usage(&format!("session {other}")),
            (None, _) => unknown_usage("session 需要 show|new 子命令"),
        },
        "tasks" => {
            let session_id = tokens.next().map(str::to_string);
            if tokens.next().is_some() {
                return unknown_usage("tasks 至多接受一个会话 ID");
            }
            finish_whcode(actor, WhCodeCommand::TaskOverview { session_id })
        }
        "task" => match tokens.next() {
            Some(task_id) if tokens.next().is_none() => finish_whcode(
                actor,
                WhCodeCommand::TaskDetail {
                    task_id: task_id.to_string(),
                },
            ),
            _ => unknown_usage("task 需要恰好一个任务 ID"),
        },
        "shell" => classify_shell(policy, actor, trimmed),
        other => Decision::Denied {
            reason: format!(
                "命令 {other:?} 不在 SSH 受限白名单：可用 sessions / session show|new / \
                 tasks / task（任意系统 Shell 是显式独立权限，默认拒绝——FR-608）"
            ),
            code: ErrorCode::PermissionDenied,
        },
    }
}

/// Shell 请求判定：配置开关与显式授权名单缺一即拒（默认全关）。
fn classify_shell(policy: &SshPolicy, actor: &whcode_app::Actor, command: &str) -> Decision {
    if !policy.allow_shell {
        return Decision::Denied {
            reason: "系统 Shell 未启用：SSH 默认只提供受限 WHCode 入口（FR-608、§10.2）；\
                     放行需要服务端配置开关与显式授权名单同时满足"
                .to_string(),
            code: ErrorCode::PermissionDenied,
        };
    }
    if !policy.shell_actors.contains(&actor.id) {
        return Decision::Denied {
            reason: format!(
                "调用者 {} 不在 Shell 显式授权名单：任意系统 Shell 是独立权限，\
                 不随业务能力授予（FR-608）",
                actor.id
            ),
            code: ErrorCode::PermissionDenied,
        };
    }
    Decision::SystemShell {
        command: command.to_string(),
    }
}

/// WHCode 命令的能力前置检查（应用层会再判一次，这里保证缺能力时给出结构化拒绝）。
fn finish_whcode(actor: &whcode_app::Actor, command: WhCodeCommand) -> Decision {
    let capability = command.required_capability();
    if actor.capabilities.contains(&capability) {
        Decision::WhCode(command)
    } else {
        Decision::Denied {
            reason: format!(
                "调用者 {} 未被授予能力 {}：该命令需要它（拒绝不可重试，FR-604）",
                actor.id,
                capability.as_str()
            ),
            code: ErrorCode::PermissionDenied,
        }
    }
}

/// 用法错误的统一结构化拒绝。
fn unknown_usage(usage: &str) -> Decision {
    Decision::Denied {
        reason: format!("用法错误：{usage}"),
        code: ErrorCode::InvalidState,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use whcode_app::{Actor, Capability};

    fn actor_with(caps: &[Capability]) -> Actor {
        Actor::new("ssh-tester", caps.iter().copied())
    }

    #[test]
    fn default_policy_denies_shell_always() {
        let policy = SshPolicy::default();
        let admin = actor_with(&[
            Capability::SessionRead,
            Capability::SessionCreate,
            Capability::TaskRead,
        ]);
        for command in ["shell", "shell ls -la", "sh", "powershell"] {
            let decision = classify(&policy, &admin, command);
            assert!(
                matches!(decision, Decision::Denied { ref code, .. } if *code == ErrorCode::PermissionDenied),
                "默认策略必须拒绝 Shell 请求：{command} → {decision:?}"
            );
        }
    }

    #[test]
    fn shell_requires_switch_and_explicit_grant() {
        // 开关关闭：即使名单有名字也拒绝。
        let closed = SshPolicy {
            allow_shell: false,
            shell_actors: SshPolicy::shell_for(["ssh-admin"]).shell_actors,
        };
        let admin = Actor::new("ssh-admin", [Capability::SessionRead]);
        assert!(matches!(
            classify(&closed, &admin, "shell ls"),
            Decision::Denied { .. }
        ));

        // 开关打开但调用者不在名单：拒绝。
        let open = SshPolicy::shell_for(["ssh-other"]);
        assert!(matches!(
            classify(&open, &admin, "shell ls"),
            Decision::Denied { .. }
        ));

        // 开关打开且在名单：放行为 SystemShell。
        let granted = SshPolicy::shell_for(["ssh-admin"]);
        assert_eq!(
            classify(&granted, &admin, "shell ls -la"),
            Decision::SystemShell {
                command: "shell ls -la".to_string()
            }
        );
    }

    #[test]
    fn whitelist_commands_map_to_capability_checked_decisions() {
        let policy = SshPolicy::default();
        let reader = actor_with(&[Capability::SessionRead, Capability::TaskRead]);
        let writer = actor_with(&[Capability::SessionCreate]);

        assert_eq!(
            classify(&policy, &reader, "sessions"),
            Decision::WhCode(WhCodeCommand::SessionList {
                include_archived: false
            })
        );
        assert_eq!(
            classify(&policy, &reader, "sessions --archived"),
            Decision::WhCode(WhCodeCommand::SessionList {
                include_archived: true
            })
        );
        assert_eq!(
            classify(&policy, &reader, "session show S-1"),
            Decision::WhCode(WhCodeCommand::SessionShow {
                session_id: "S-1".to_string()
            })
        );
        assert_eq!(
            classify(&policy, &writer, "session new   新会话  标题 "),
            Decision::WhCode(WhCodeCommand::SessionNew {
                title: "新会话  标题".to_string()
            }),
            "标题取剩余整行并去首尾空白"
        );
        assert_eq!(
            classify(&policy, &reader, "tasks"),
            Decision::WhCode(WhCodeCommand::TaskOverview { session_id: None })
        );
        assert_eq!(
            classify(&policy, &reader, "tasks S-1"),
            Decision::WhCode(WhCodeCommand::TaskOverview {
                session_id: Some("S-1".to_string())
            })
        );
        assert_eq!(
            classify(&policy, &reader, "task T-9"),
            Decision::WhCode(WhCodeCommand::TaskDetail {
                task_id: "T-9".to_string()
            })
        );

        // 能力前置检查：只读调用者不能创建会话。
        assert!(matches!(
            classify(&policy, &reader, "session new x"),
            Decision::Denied {
                code: ErrorCode::PermissionDenied,
                ..
            }
        ));
        // 无 TaskRead 不能看任务。
        assert!(matches!(
            classify(&policy, &writer, "tasks"),
            Decision::Denied {
                code: ErrorCode::PermissionDenied,
                ..
            }
        ));
    }

    #[test]
    fn outside_whitelist_and_bad_usage_are_denied_structured() {
        let policy = SshPolicy::default();
        let admin = actor_with(&[
            Capability::SessionRead,
            Capability::SessionCreate,
            Capability::TaskRead,
        ]);
        for command in [
            "",
            "   ",
            "rm -rf /",
            "powershell -c dir",
            "sessions extra",
            "session",
            "session show",
            "session show S-1 extra",
            "session delete S-1",
            "session new",
            "tasks S-1 extra",
            "task",
            "task T-1 extra",
        ] {
            let decision = classify(&policy, &admin, command);
            assert!(
                matches!(decision, Decision::Denied { .. }),
                "白名单外/坏用法必须拒绝：{command:?} → {decision:?}"
            );
            if let Decision::Denied { reason, code } = decision {
                assert!(!reason.is_empty(), "拒绝必须带可操作说明：{command:?}");
                assert!(
                    code == ErrorCode::PermissionDenied || code == ErrorCode::InvalidState,
                    "错误码收敛：{command:?} → {code:?}"
                );
            }
        }
    }

    #[test]
    fn command_summary_is_stable_and_capability_mapping_matches_app() {
        // 能力映射与 whcode-app 的命令权限一致（session.new → session.create 等）。
        assert_eq!(
            WhCodeCommand::SessionNew {
                title: "t".to_string()
            }
            .required_capability(),
            Capability::SessionCreate
        );
        assert_eq!(
            WhCodeCommand::SessionList {
                include_archived: false
            }
            .required_capability(),
            Capability::SessionRead
        );
        assert_eq!(
            WhCodeCommand::TaskDetail {
                task_id: "T-1".to_string()
            }
            .required_capability(),
            Capability::TaskRead
        );
        // 审计摘要不携带数据内容。
        assert_eq!(
            WhCodeCommand::SessionShow {
                session_id: "S-secret".to_string()
            }
            .summary(),
            "session show <id>"
        );
    }
}
