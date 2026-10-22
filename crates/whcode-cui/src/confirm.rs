//! # 破坏性命令 dry-run/确认机制（T086、§8.1、FR-602）
//!
//! 破坏性命令（delete/archive/purge/compact/force-advance）必须支持 dry-run 或
//! 确认机制（§8.1、app-service 契约 §2-1）。[`Confirm`] 显式表达确认状态：
//!
//! - [`Confirm::dry_run`]：只预览，不执行——预览输出与实际执行影响集一致
//!   （Check 86，由各命令的 preview/execute 双路径保证，如 purge 的
//!   `preview_purge`/`execute_purge`）；
//! - [`Confirm::yes`]：显式确认执行（无隐式默认，FR-509 第 2 项同源语义）。
//!
//! 边界：本模块只表达确认语义；交互式确认输入属命令树（T084）。

/// 破坏性命令的确认状态（显式，无隐式默认）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirm {
    /// 只预览不执行（dry-run，输出与实际执行影响集一致，Check 86）。
    DryRun,
    /// 显式确认执行。
    Yes,
}

impl Confirm {
    /// 是否允许执行。
    #[must_use]
    pub const fn allows_execution(self) -> bool {
        matches!(self, Self::Yes)
    }

    /// 从命令行参数解析：`--dry-run` → DryRun；`--yes` → Yes；
    /// 两者皆无 → DryRun（**默认安全**：破坏性命令缺省只预览，不做隐式执行）。
    #[must_use]
    pub fn from_flags(_dry_run: bool, yes: bool) -> Self {
        if yes { Self::Yes } else { Self::DryRun }
    }
}

/// dry-run 与执行的影响集一致性校验（Check 86）：
/// 预览集与执行集相等才视为一致（由调用方以集合比较断言）。
#[must_use]
pub fn dry_run_consistent(preview: &[String], executed: &[String]) -> bool {
    let mut a = preview.to_vec();
    let mut b = executed.to_vec();
    a.sort();
    b.sort();
    a == b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_dry_run_and_execution_requires_explicit_yes() {
        // 默认安全：破坏性命令缺省只预览（§8.1，无隐式默认）。
        assert!(!Confirm::from_flags(false, false).allows_execution());
        assert!(
            Confirm::from_flags(false, true).allows_execution(),
            "显式 --yes 才执行"
        );
        assert!(!Confirm::from_flags(true, false).allows_execution());
        // 显式 Yes 语义。
        assert!(Confirm::Yes.allows_execution());
        assert!(!Confirm::DryRun.allows_execution());
    }

    #[test]
    fn dry_run_consistency_requires_equal_sets() {
        assert!(dry_run_consistent(
            &["S-1".to_string()],
            &["S-1".to_string()],
        ));
        assert!(dry_run_consistent(
            &["S-2".to_string(), "S-1".to_string()],
            &["S-1".to_string(), "S-2".to_string()],
        ));
        assert!(
            !dry_run_consistent(
                &["S-1".to_string()],
                &["S-1".to_string(), "S-2".to_string()],
            ),
            "预览集与执行集不一致必须检出（Check 86）"
        );
    }
}
