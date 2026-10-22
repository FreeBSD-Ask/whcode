//! 追踪级别（任务书 FR-203、§10.1、D-05）。
//!
//! 默认级别为 `redacted`：疑似秘密字段脱敏后写入。`full` 级别必须在界面中警告潜在敏感
//! 信息写入，并提供访问控制、清理与导出策略——该界面告警属前端职责（M5/M6）。

use serde::{Deserialize, Serialize};

/// 追踪级别四档。
///
/// 默认级别为 `Redacted`（D-05）——由 `#[default]` 表达，避免 `Default` 实现与枚举声明分离。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceLevel {
    /// 不记录追踪内容。
    Off,
    /// 仅元数据（时长、token 数、状态），无正文。
    Metadata,
    /// 记录内容但对疑似秘密字段脱敏（默认）。
    #[default]
    Redacted,
    /// 记录完整内容，需界面告警与访问控制。
    Full,
}

impl TraceLevel {
    /// 稳定序列化名。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Metadata => "metadata",
            Self::Redacted => "redacted",
            Self::Full => "full",
        }
    }

    /// 是否记录内容正文（`Redacted` 与 `Full` 记录，区别在脱敏）。
    pub const fn writes_content(self) -> bool {
        matches!(self, Self::Redacted | Self::Full)
    }

    /// 是否需要在界面中提示敏感信息风险。
    pub const fn requires_sensitive_warning(self) -> bool {
        matches!(self, Self::Full)
    }
}

impl std::fmt::Display for TraceLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_redacted() {
        assert_eq!(TraceLevel::default(), TraceLevel::Redacted);
    }

    #[test]
    fn only_content_levels_write_bodies() {
        assert!(!TraceLevel::Off.writes_content());
        assert!(!TraceLevel::Metadata.writes_content());
        assert!(TraceLevel::Redacted.writes_content());
        assert!(TraceLevel::Full.writes_content());
    }

    #[test]
    fn full_level_requires_warning() {
        assert!(TraceLevel::Full.requires_sensitive_warning());
        assert!(!TraceLevel::Redacted.requires_sensitive_warning());
    }
}
