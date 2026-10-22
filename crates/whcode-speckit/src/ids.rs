//! # 需求/任务稳定 ID 与引用校验（T071、FR-304）
//!
//! 稳定 ID：归一化后重解析不变（`REQ-001` / `T-001` / `DEC-001` / `CLAR-001`）；
//! 引用校验：任务的需求/计划/依赖引用必须指向已知 ID，无效引用显式列出
//! （FR-306 的 Analyze 输入，`whcode-speckit::analyze` 复用）。
//!
//! 边界：ID 归一化是纯函数；登记表是内存结构，持久化属产物解析结果（M4 产物链）。

use std::collections::BTreeSet;

use whcode_domain::WhError;

/// ID 前缀（稳定 ID 必须命中其一，FR-304）。
pub const ID_PREFIXES: [&str; 4] = ["REQ-", "T-", "DEC-", "CLAR-"];

/// 归一化 ID：去空白、统一大写前缀、去多余分隔。
///
/// 重解析不变（T071 稳定性判据）：`normalize(normalize(x)) == normalize(x)`。
#[must_use]
pub fn normalize_id(raw: &str) -> String {
    let trimmed = raw.trim().trim_matches('*').trim();
    for prefix in ID_PREFIXES {
        let lower = prefix.to_ascii_lowercase();
        if let Some(rest) = trimmed.strip_prefix(&lower) {
            return format!("{prefix}{}", rest.trim_start_matches(['-', '_']));
        }
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return format!("{prefix}{}", rest.trim_start_matches(['-', '_', ' ']));
        }
    }
    trimmed.to_string()
}

/// ID 登记表：已知的需求/任务/决策/澄清 ID（稳定 ID 的唯一事实源）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IdRegistry {
    /// 需求 ID。
    pub requirements: BTreeSet<String>,
    /// 任务 ID。
    pub tasks: BTreeSet<String>,
    /// 计划决策 ID。
    pub decisions: BTreeSet<String>,
    /// 澄清项 ID。
    pub clarifications: BTreeSet<String>,
}

impl IdRegistry {
    /// 空登记表。
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记一个 ID（自动按前缀归类；未知前缀拒绝）。
    ///
    /// # Errors
    ///
    /// 未知前缀返回 `invalid_state`（稳定 ID 必须命中 [`ID_PREFIXES`]）。
    pub fn register(&mut self, id: &str) -> Result<(), WhError> {
        let normalized = normalize_id(id);
        if normalized.starts_with("REQ-") {
            self.requirements.insert(normalized);
        } else if normalized.starts_with("T-") {
            self.tasks.insert(normalized);
        } else if normalized.starts_with("DEC-") {
            self.decisions.insert(normalized);
        } else if normalized.starts_with("CLAR-") {
            self.clarifications.insert(normalized);
        } else {
            return Err(WhError::invalid_state(format!(
                "ID {normalized} 前缀未知：稳定 ID 必须命中 {ID_PREFIXES:?}（FR-304）"
            )));
        }
        Ok(())
    }

    /// 引用是否有效（命中任一已知集合）。
    #[must_use]
    pub fn contains(&self, id: &str) -> bool {
        let normalized = normalize_id(id);
        self.requirements.contains(&normalized)
            || self.tasks.contains(&normalized)
            || self.decisions.contains(&normalized)
            || self.clarifications.contains(&normalized)
    }

    /// 校验一组引用，返回无效引用清单（归一化后仍未知者）。
    #[must_use]
    pub fn invalid_refs<'a, I, S>(&self, refs: I) -> Vec<String>
    where
        I: IntoIterator<Item = &'a S>,
        S: AsRef<str> + 'a + ?Sized,
    {
        refs.into_iter()
            .map(|id| id.as_ref())
            .filter(|id| !self.contains(id))
            .map(normalize_id)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_is_stable_across_reparse() {
        assert_eq!(normalize_id("req-001"), "REQ-001");
        assert_eq!(normalize_id("**REQ-001**"), "REQ-001");
        assert_eq!(normalize_id("T-1"), "T-1");
        // 幂等：重解析不变（T071 判据）。
        let once = normalize_id("  req-1 ");
        assert_eq!(normalize_id(&once), once);
    }

    #[test]
    fn registry_rejects_unknown_prefix_and_finds_invalid_refs() {
        let mut registry = IdRegistry::new();
        registry.register("REQ-001").expect("需求");
        registry.register("T-1").expect("任务");
        registry.register("DEC-001").expect("决策");

        assert!(registry.contains("req-001"), "归一化后命中");
        let invalid = registry.invalid_refs(["REQ-001", "REQ-404", "DEC-001"]);
        assert_eq!(invalid, vec!["REQ-404"], "无效引用显式列出（FR-306 输入）");

        let err = registry.register("BOGUS-1").expect_err("未知前缀必须拒绝");
        assert!(err.to_string().contains("BOGUS-1"));
    }
}
