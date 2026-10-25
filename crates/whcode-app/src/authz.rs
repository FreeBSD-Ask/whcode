//! # 权限检查与统一拒绝语义（T050、FR-604、§10.2、app-service 契约 §4）
//!
//! 四端一致性的关键点：**拒绝判定只发生在本层**。前端不推断权限；命令执行前经
//! [`require`] 检查，拒绝一律返回 `permission_denied`（`retryable = false`），
//! 不静默降级、不跳过。
//!
//! 模型（M2 口径）：调用者 [`Actor`] 持有显式能力集合 [`Capability`]；默认策略
//! [`DefaultPolicy`] 做"集合包含"判定。更细的策略（角色、阶段裁剪、按会话授权）
//! 经 [`Policy`] trait 在装配根注入（ADR-018 同一层级），本模块不假设具体授权来源。
//!
//! 边界：不做数据库访问；不做审计（审计属破坏性命令的执行路径）。

use std::collections::BTreeSet;

use whcode_domain::WhError;

/// 应用服务能力（命令权限的最小粒度；随里程碑扩展）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Capability {
    /// 创建会话。
    SessionCreate,
    /// 读取会话（show/list/切换/快照）。
    SessionRead,
    /// 重命名会话。
    SessionRename,
    /// 归档/取消归档会话。
    SessionArchive,
    /// 删除会话（破坏性）。
    SessionDelete,
    /// 任务读取（列表/详情/总览）。
    TaskRead,
    /// 重试任务。
    TaskRetry,
    /// 中止任务。
    TaskAbort,
    /// 推进 SDD 阶段（含 force-advance，FR-307/312）。
    SpecAdvance,
    /// 读取证书库设置与状态（ADR-040 决策 4；`/api/certs` 读面）。
    SettingsCertRead,
    /// 写入证书库设置与证书（ADR-040 决策 4；档位、根证书与服务端证书变更）。
    SettingsCertWrite,
}

impl Capability {
    /// 稳定标识（入日志与错误详情）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SessionCreate => "session.create",
            Self::SessionRead => "session.read",
            Self::SessionRename => "session.rename",
            Self::SessionArchive => "session.archive",
            Self::SessionDelete => "session.delete",
            Self::TaskRead => "task.read",
            Self::TaskRetry => "task.retry",
            Self::TaskAbort => "task.abort",
            Self::SpecAdvance => "spec.advance",
            Self::SettingsCertRead => "settings.cert.read",
            Self::SettingsCertWrite => "settings.cert.write",
        }
    }
}

/// 调用者身份与其被授予的能力集合。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Actor {
    /// 调用者标识（入审计与日志；脱敏策略同日志路径）。
    pub id: String,
    /// 已授予的能力集合。
    pub capabilities: BTreeSet<Capability>,
}

impl Actor {
    /// 构造带能力集合的调用者。
    #[must_use]
    pub fn new(id: impl Into<String>, capabilities: impl IntoIterator<Item = Capability>) -> Self {
        Self {
            id: id.into(),
            capabilities: capabilities.into_iter().collect(),
        }
    }

    /// 授予一项能力。
    pub fn grant(&mut self, capability: Capability) {
        self.capabilities.insert(capability);
    }
}

/// 授权策略：判定调用者能否行使某能力。
pub trait Policy: Send + Sync {
    /// 放行返回 `Ok(())`；拒绝返回 `permission_denied`（`retryable = false`）。
    ///
    /// # Errors
    ///
    /// 拒绝时返回 [`whcode_domain::ErrorCode::PermissionDenied`]。
    fn authorize(&self, actor: &Actor, capability: Capability) -> Result<(), WhError>;
}

/// 默认策略：调用者的显式能力集合包含所需能力即放行。
///
/// 默认值可见、可改：拒绝是显式的（能力集合里没有），不存在隐式放行路径。
#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultPolicy;

impl Policy for DefaultPolicy {
    fn authorize(&self, actor: &Actor, capability: Capability) -> Result<(), WhError> {
        if actor.capabilities.contains(&capability) {
            Ok(())
        } else {
            Err(WhError::permission_denied(format!(
                "调用者 {} 未被授予能力 {}；权限拒绝不可重试（FR-604、§10.2）",
                actor.id,
                capability.as_str()
            )))
        }
    }
}

/// 命令执行前的统一检查入口；所有 app 命令必须先经此函数。
///
/// # Errors
///
/// 拒绝时返回 `permission_denied`（`retryable = false`）。
pub fn require(policy: &dyn Policy, actor: &Actor, capability: Capability) -> Result<(), WhError> {
    policy.authorize(actor, capability)
}

/// 测试与本地工作流用的全能力调用者。
#[must_use]
pub fn actor_with_all_capabilities(id: &str) -> Actor {
    Actor::new(
        id,
        [
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
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use whcode_domain::ErrorCode;

    #[test]
    fn denial_is_permission_denied_and_non_retryable() {
        let policy = DefaultPolicy;
        let actor = Actor::new("u-1", [Capability::SessionRead]);
        let err = require(&policy, &actor, Capability::SessionDelete).expect_err("未授予必须拒绝");
        assert_eq!(err.code, ErrorCode::PermissionDenied);
        assert!(!err.retryable, "权限拒绝不可重试（FR-604）");
        assert!(err.to_string().contains("session.delete"));

        require(&policy, &actor, Capability::SessionRead).expect("已授予必须放行");
    }

    #[test]
    fn custom_policy_is_the_single_rejection_point() {
        // 拒绝一切的自定义策略：即使能力集合满足也拒绝——策略是唯一判定点。
        struct DenyAll;
        impl Policy for DenyAll {
            fn authorize(&self, _actor: &Actor, _capability: Capability) -> Result<(), WhError> {
                Err(WhError::permission_denied("策略拒绝"))
            }
        }
        let actor = actor_with_all_capabilities("admin");
        let err = require(&DenyAll, &actor, Capability::SessionCreate).expect_err("策略优先");
        assert_eq!(err.code, ErrorCode::PermissionDenied);
        assert!(!err.retryable);
    }

    #[test]
    fn capability_names_are_stable() {
        assert_eq!(Capability::SessionCreate.as_str(), "session.create");
        assert_eq!(Capability::TaskAbort.as_str(), "task.abort");
        assert_eq!(Capability::SettingsCertRead.as_str(), "settings.cert.read");
        assert_eq!(
            Capability::SettingsCertWrite.as_str(),
            "settings.cert.write"
        );
    }

    /// ADR-040 决策 4：证书读写能力互相独立（读能力不得写），全能力调用者包含二者。
    #[test]
    fn cert_capabilities_are_granted_independently() {
        let policy = DefaultPolicy;
        let reader = Actor::new("cert-reader", [Capability::SettingsCertRead]);
        require(&policy, &reader, Capability::SettingsCertRead).expect("读能力已授予必须放行");
        let err = require(&policy, &reader, Capability::SettingsCertWrite)
            .expect_err("仅读能力不得执行写操作");
        assert_eq!(err.code, ErrorCode::PermissionDenied);
        assert!(err.to_string().contains("settings.cert.write"), "{err}");

        let full = actor_with_all_capabilities("admin");
        for capability in [Capability::SettingsCertRead, Capability::SettingsCertWrite] {
            require(&policy, &full, capability).expect("全能力调用者必须包含证书能力");
        }
    }
}
