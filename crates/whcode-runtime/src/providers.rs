//! # 具体运行时 Provider（T110—T113、D-08/D-09、§4.7.1）
//!
//! 四个 Provider 的**诚实能力门控**：
//!
//! | Provider | capability | M8 状态 |
//! |---|---|---|
//! | [`JsProvider`]（`js`） | Restricted | D-08：不宣称完整 Node.js；无引擎时返回 `runtime_unavailable`（能力不足结构化错误，FR-708） |
//! | [`PythonProvider`]（`python`） | Restricted | D-09：受控子进程 worker（系统解释器），不可用时结构化错误 |
//! | [`GitBashProvider`]（`gitbash`） | ShellOnly | V-304：Windows PortableGit payload 校验（`git.exe` 探测） |
//! | [`BsdShellProvider`]（`bsdshell`） | ShellOnly | BSD 平台 Shell——与 `gitbash` 是不同能力，不共用名称 |
//!
//! 命名边界（§4.7.1）：`gitbash` ≠ `bsdshell`；受限引擎不得表述为完整生态。
//! `js`/`python` 的完整实现属后续接入；当前交付接口与诚实不可用路径。

use whcode_domain::WhError;

use crate::capability::{Capability, RuntimeDeclaration};
use crate::checkpoint::Checkpoint;
use crate::provider::{RuntimeProvider, Script, ensure_available};
use crate::worker;

/// `js` 运行时（T110、D-08：受限 ECMAScript 引擎）。
#[derive(Debug, Clone)]
pub struct JsProvider {
    declaration: RuntimeDeclaration,
}

impl Default for JsProvider {
    fn default() -> Self {
        Self {
            declaration: RuntimeDeclaration {
                name: "js".to_string(),
                capability: Capability::Restricted,
                unsupported: vec![
                    "Node.js 内置模块".to_string(),
                    "文件系统".to_string(),
                    "网络".to_string(),
                ],
            },
        }
    }
}

impl RuntimeProvider for JsProvider {
    fn name(&self) -> &str {
        "js"
    }
    fn declaration(&self) -> &RuntimeDeclaration {
        &self.declaration
    }
    fn is_available(&self) -> bool {
        // D-08：未内置引擎（Boa 等属后续接入）——诚实不可用，不伪装。
        false
    }
    fn env_fingerprint(&self) -> String {
        "js/unavailable".to_string()
    }
    fn execute(&self, _script: &Script) -> Result<Checkpoint, WhError> {
        Err(crate::errors::runtime_unavailable(
            "js",
            crate::errors::UnavailableReason::Missing,
            "受限 ECMAScript 引擎未内置（D-08：不宣称完整 Node.js）",
        ))
    }
    fn terminate(&mut self) -> Result<(), WhError> {
        Ok(())
    }
}

/// `python` 运行时（T111、D-09：受控子进程 worker，系统解释器）。
#[derive(Debug, Clone)]
pub struct PythonProvider {
    declaration: RuntimeDeclaration,
    /// 解释器命令（默认 `python`；配置可改）。
    pub interpreter: String,
}

impl Default for PythonProvider {
    fn default() -> Self {
        Self {
            declaration: RuntimeDeclaration {
                name: "python".to_string(),
                capability: Capability::Restricted,
                unsupported: vec!["任意标准库".to_string(), "任意第三方包".to_string()],
            },
            interpreter: "python".to_string(),
        }
    }
}

impl PythonProvider {
    fn probe(&self) -> Option<String> {
        let mut command = std::process::Command::new(&self.interpreter);
        command.arg("--version");
        // 探测同样不携带凭据（§10.2）。
        let _ = worker::strip_credentials(&mut command);
        let output = command.output().ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
    }
}

impl RuntimeProvider for PythonProvider {
    fn name(&self) -> &str {
        "python"
    }
    fn declaration(&self) -> &RuntimeDeclaration {
        &self.declaration
    }
    fn is_available(&self) -> bool {
        self.probe().is_some()
    }
    fn env_fingerprint(&self) -> String {
        self.probe()
            .unwrap_or_else(|| "python/unavailable".to_string())
    }
    fn execute(&self, script: &Script) -> Result<Checkpoint, WhError> {
        ensure_available(self)?;
        let mut command = std::process::Command::new(&self.interpreter);
        command.arg("-c").arg(&script.script);
        let result = worker::execute(&mut command, script.timeout, worker::MAX_OUTPUT_BYTES)?;
        Ok(Checkpoint {
            script_hash: crate::payload::fnv64(script.script.as_bytes()),
            input_hash: crate::payload::fnv64(script.input.as_bytes()),
            runtime_fingerprint: self.env_fingerprint(),
            workdir_policy: "temp".to_string(),
            result: Some(result),
        })
    }
    fn terminate(&mut self) -> Result<(), WhError> {
        Ok(())
    }
}

/// `gitbash` 运行时（T112、V-304：Windows PortableGit）。
#[derive(Debug, Clone)]
pub struct GitBashProvider {
    declaration: RuntimeDeclaration,
    /// `git.exe` 路径（PortableGit 安装位置）。
    pub git_path: Option<String>,
}

impl Default for GitBashProvider {
    fn default() -> Self {
        Self {
            declaration: RuntimeDeclaration {
                name: "gitbash".to_string(),
                capability: Capability::ShellOnly,
                unsupported: vec!["任意系统 Shell".to_string()],
            },
            git_path: None,
        }
    }
}

impl RuntimeProvider for GitBashProvider {
    fn name(&self) -> &str {
        "gitbash"
    }
    fn declaration(&self) -> &RuntimeDeclaration {
        &self.declaration
    }
    fn is_available(&self) -> bool {
        // V-304：PortableGit payload 探测（git.exe 存在且可执行 --version）。
        self.git_path
            .as_ref()
            .map(|path| {
                let mut command = std::process::Command::new(path);
                command.arg("--version");
                // 探测同样不携带凭据（§10.2）。
                let _ = worker::strip_credentials(&mut command);
                command.output().is_ok_and(|output| output.status.success())
            })
            .unwrap_or(false)
    }
    fn env_fingerprint(&self) -> String {
        "gitbash/unavailable".to_string()
    }
    fn execute(&self, _script: &Script) -> Result<Checkpoint, WhError> {
        ensure_available(self)?;
        Err(crate::errors::runtime_unavailable(
            "gitbash",
            crate::errors::UnavailableReason::CapabilityInsufficient,
            "Bash/coreutils 执行属后续接入（V-304 payload 校验已交付）",
        ))
    }
    fn terminate(&mut self) -> Result<(), WhError> {
        Ok(())
    }
}

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
            JsProvider::default().declaration.clone(),
            PythonProvider::default().declaration.clone(),
            GitBashProvider::default().declaration.clone(),
            BsdShellProvider::default().declaration.clone(),
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

        let bsd = BsdShellProvider::default();
        assert!(!bsd.is_available(), "BSD Shell 属 M7 延后");
    }

    #[test]
    fn python_probes_system_interpreter() {
        let provider = PythonProvider::default();
        // 系统可能无 python——两个分支都合法，关键是 fingerprint 一致性。
        let fingerprint = provider.env_fingerprint();
        assert_eq!(provider.is_available(), fingerprint != "python/unavailable");
    }
}
