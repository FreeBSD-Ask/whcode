//! # 运行时能力等级（T101、FR-702、D-10/R-10）
//!
//! `Full` / `Restricted` / `ShellOnly` 三级（FR-702）；命名边界（§4.7.1）：
//! Boa ≠ 完整 Node.js、RustPython ≠ 完整 Python、Windows Git Bash ≠ BSD Shell——
//! 受限实现必须列明不支持的能力，不得伪装等价。
//!
//! 边界：纯类型；接入界面与门禁展示属调用方（快照/工具注册表）。

use serde::{Deserialize, Serialize};

/// 能力等级（FR-702）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// 完整能力（全部标准能力可用）。
    Full,
    /// 受限（列明不支持项；未覆盖能力返回结构化错误，FR-708）。
    Restricted,
    /// 仅 Shell（命令执行受限入口，FR-608 同源语义）。
    ShellOnly,
}

impl Capability {
    /// 稳定序列化名。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Restricted => "restricted",
            Self::ShellOnly => "shell_only",
        }
    }
}

/// 运行时声明（名称 + 能力等级 + 未覆盖能力清单，D-08/R-10）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeDeclaration {
    /// 运行时名（如 `js`、`python`、`gitbash`、`bsdshell`——互不混同）。
    pub name: String,
    /// 能力等级。
    pub capability: Capability,
    /// 未覆盖能力（D-08：不宣称完整生态，缺失项显式列出）。
    pub unsupported: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_levels_are_stable() {
        assert_eq!(Capability::Full.as_str(), "full");
        assert_eq!(Capability::Restricted.as_str(), "restricted");
        assert_eq!(Capability::ShellOnly.as_str(), "shell_only");
    }

    #[test]
    fn declarations_never_claim_full_for_restricted_engines() {
        // D-08：受限引擎不得标 Full 且必须列明未覆盖项。
        let js = RuntimeDeclaration {
            name: "js".to_string(),
            capability: Capability::Restricted,
            unsupported: vec!["Node.js 内置模块".to_string(), "文件系统".to_string()],
        };
        assert!(js.capability != Capability::Full);
        assert!(!js.unsupported.is_empty(), "受限必须列明不支持项");
    }
}
