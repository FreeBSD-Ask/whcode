//! # 具体运行时 Provider（T110—T113、D-08/D-09、§4.7.1）
//!
//! 四个 Provider 的**诚实能力门控**：
//!
//! | Provider | capability | 状态 |
//! |---|---|---|
//! | [`JsProvider`]（`js`） | Restricted | D-08：不宣称完整 Node.js；无引擎时返回 `runtime_unavailable`（能力不足结构化错误，FR-708） |
//! | [`PythonProvider`]（`python`） | Restricted | D-09：受控子进程 worker（系统解释器），不可用时结构化错误 |
//! | [`GitBashProvider`]（`gitbash`） | ShellOnly | V-304：Windows PortableGit payload 校验与 Bash 能力探测 |
//! | [`BsdShellProvider`]（`bsdshell`） | ShellOnly | BSD 平台 Shell——与 `gitbash` 是不同能力，不共用名称 |
//!
//! 命名边界（§4.7.1）：`gitbash` ≠ `bsdshell`；受限引擎不得表述为完整生态。
//! 本模块保留跨 Provider 的注册表不变式测试；各 Provider 实现与领域逻辑
//! 分居 [`crate::js`]、[`crate::python`]、[`crate::gitbash`]（任务命名文件，T110—T112）。

use whcode_domain::WhError;

use crate::capability::{Capability, RuntimeDeclaration};
use crate::checkpoint::Checkpoint;
use crate::provider::{RuntimeProvider, Script};

pub use crate::gitbash::GitBashProvider;
pub use crate::js::JsProvider;
pub use crate::python::PythonProvider;

/// `bsdshell` 运行时（T113、§4.7.1：BSD Shell——与 `gitbash` 不同能力）。
#[derive(Debug, Clone)]
pub struct BsdShellProvider {
    declaration: RuntimeDeclaration,
}

impl Default for BsdShellProvider {
    fn default() -> Self {
        Self {
            declaration: RuntimeDeclaration {
                name: "bsdshell".to_string(),
                capability: Capability::ShellOnly,
                unsupported: vec!["Windows Git Bash".to_string()],
            },
        }
    }
}

impl RuntimeProvider for BsdShellProvider {
    fn name(&self) -> &str {
        "bsdshell"
    }
    fn declaration(&self) -> &RuntimeDeclaration {
        &self.declaration
    }
    fn is_available(&self) -> bool {
        // BSD Shell 属 M7 平台范围（ADR-013 延后）——Windows 上不可用。
        false
    }
    fn env_fingerprint(&self) -> String {
        "bsdshell/unavailable".to_string()
    }
    fn execute(&self, _script: &Script) -> Result<Checkpoint, WhError> {
        Err(crate::errors::runtime_unavailable(
            "bsdshell",
            crate::errors::UnavailableReason::Missing,
            "BSD Shell 属 M7 平台范围（ADR-013 延后）",
        ))
    }
    fn terminate(&mut self) -> Result<(), WhError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn providers_never_claim_full_and_names_do_not_collide() {
        let providers: Vec<RuntimeDeclaration> = vec![
            JsProvider::default().declaration().clone(),
            PythonProvider::default().declaration().clone(),
            GitBashProvider::default().declaration().clone(),
            BsdShellProvider::default().declaration().clone(),
        ];
        for declaration in &providers {
            assert!(
                declaration.capability != Capability::Full,
                "{} 不得宣称完整能力（D-08/R-10）",
                declaration.name
            );
            assert!(
                !declaration.unsupported.is_empty(),
                "{} 必须列明未覆盖项",
                declaration.name
            );
        }
        let names: std::collections::BTreeSet<&str> = providers
            .iter()
            .map(|declaration| declaration.name.as_str())
            .collect();
        assert_eq!(names.len(), 4, "四个 Provider 命名互不混同（§4.7.1）");
    }

    #[test]
    fn unavailable_providers_return_structured_errors() {
        // T110/ADR-029 修订：feature `runtime-js` 开启时 js 走 Boa 引擎路径
        // （is_available 为真），本段“js 无引擎诚实不可用”断言仅对默认构建成立；
        // 默认构建行为与语义不变。
        #[cfg(not(feature = "runtime-js"))]
        {
            let mut js = JsProvider::default();
            assert!(!js.is_available(), "js 无引擎诚实不可用（D-08）");
            let err = js
                .execute(&Script {
                    script: "1".to_string(),
                    input: String::new(),
                    timeout: std::time::Duration::from_secs(1),
                })
                .expect_err("不可用必须结构化错误");
            assert_eq!(err.code, whcode_domain::ErrorCode::RuntimeUnavailable);
            js.terminate().expect("终止");
        }

        let bsd = BsdShellProvider::default();
        assert!(!bsd.is_available(), "BSD Shell 属 M7 延后");
    }
}
