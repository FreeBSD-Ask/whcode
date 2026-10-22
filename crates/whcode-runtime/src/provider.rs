//! # Runtime Provider 接口（T100、FR-701、contracts/runtime-protocol.md）
//!
//! `name/capability/is_available/env_fingerprint/execute/terminate`（FR-701）；
//! 全部脚本在受控子进程执行（ADR-008，[`crate::worker`]）；执行前检查点记录
//! （[`crate::checkpoint`]，FR-706）。
//!
//! 命名边界（§4.7.1）：各 Provider 能力互不混同（`gitbash` ≠ `bsdshell`）。

use whcode_domain::WhError;

use crate::capability::{Capability, RuntimeDeclaration};
use crate::checkpoint::Checkpoint;

/// 执行请求（脚本 + 输入 + 超时）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Script {
    /// 脚本文本。
    pub script: String,
    /// 输入（stdin）。
    pub input: String,
    /// 超时。
    pub timeout: std::time::Duration,
}

/// 运行时 Provider 接口（FR-701 六要素）。
pub trait RuntimeProvider: Send + Sync {
    /// 运行时名。
    fn name(&self) -> &str;
    /// 能力声明（等级 + 未覆盖项）。
    fn declaration(&self) -> &RuntimeDeclaration;
    /// 能力等级（FR-702 快捷）。
    fn capability(&self) -> Capability {
        self.declaration().capability
    }
    /// 是否可用（安装/启用）。
    fn is_available(&self) -> bool;
    /// 环境指纹（版本 + 平台，T105 检查点记录）。
    fn env_fingerprint(&self) -> String;
    /// 执行（受控子进程，先记检查点）。
    ///
    /// # Errors
    ///
    /// 不可用或执行失败时返回 [`WhError`]。
    fn execute(&self, script: &Script) -> Result<Checkpoint, WhError>;
    /// 终止（取消语义）。
    ///
    /// # Errors
    ///
    /// 终止失败时返回 [`WhError`]。
    fn terminate(&mut self) -> Result<(), WhError>;
}

/// Provider 执行的统一入口：不可用时结构化错误（FR-701/708）。
///
/// # Errors
///
/// 不可用时返回 `runtime_unavailable`（含安装建议）。
pub fn ensure_available(provider: &dyn RuntimeProvider) -> Result<(), WhError> {
    if !provider.is_available() {
        return Err(crate::errors::runtime_unavailable(
            provider.name(),
            crate::errors::UnavailableReason::Missing,
            "whcode runtime install <name>",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::Capability;

    struct StubRuntime;

    impl RuntimeProvider for StubRuntime {
        fn name(&self) -> &str {
            "stub"
        }
        fn declaration(&self) -> &RuntimeDeclaration {
            static DECL: std::sync::OnceLock<RuntimeDeclaration> = std::sync::OnceLock::new();
            DECL.get_or_init(|| RuntimeDeclaration {
                name: "stub".to_string(),
                capability: Capability::ShellOnly,
                unsupported: vec!["文件系统写入".to_string()],
            })
        }
        fn is_available(&self) -> bool {
            true
        }
        fn env_fingerprint(&self) -> String {
            "stub-1.0/windows-amd64".to_string()
        }
        fn execute(&self, _script: &Script) -> Result<Checkpoint, WhError> {
            Ok(Checkpoint {
                script_hash: "h".to_string(),
                input_hash: "i".to_string(),
                runtime_fingerprint: self.env_fingerprint(),
                workdir_policy: "temp".to_string(),
                result: None,
            })
        }
        fn terminate(&mut self) -> Result<(), WhError> {
            Ok(())
        }
    }

    #[test]
    fn provider_contract_six_elements() {
        let provider = StubRuntime;
        assert_eq!(provider.name(), "stub");
        assert_eq!(provider.capability(), Capability::ShellOnly);
        assert!(provider.is_available());
        assert!(provider.env_fingerprint().contains("windows-amd64"));
        let checkpoint = provider
            .execute(&Script {
                script: "echo hi".to_string(),
                input: String::new(),
                timeout: std::time::Duration::from_secs(5),
            })
            .expect("执行");
        assert_eq!(checkpoint.workdir_policy, "temp");
        ensure_available(&provider).expect("可用");
    }
}
