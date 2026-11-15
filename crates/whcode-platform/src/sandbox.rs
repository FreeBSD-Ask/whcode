//! # OS 层沙箱事实（P5.2／ADR-077）
//!
//! 只回答一个问题：**当前进程所运行的平台上，OS 层强制隔离是否已生效**。答案只能是
//! [`OsEnforcement::Active`] 或 [`OsEnforcement::Unavailable`]，后者必须带可操作原因；
//! 不允许“未知”“暂不支持”这类无法核验的措辞（AGENTS §1.4 不伪造、§10 不确定时写阻塞）。
//!
//! 为什么这里是唯一落点：`cfg(target_os)` 只允许出现在 `whcode-platform`（NFR-101／分层
//! 门禁），OS 能力事实必须与平台分支同处一地，避免界面层各写一套“本平台大概支持”。
//!
//! 当前事实（2026-11-15，ADR-077 实测与官方文档核对）：
//! - Windows：AppContainer 需要 `CreateAppContainerProfile`、`DeriveAppContainerSidFromAppContainerName`、
//!   `InitializeProcThreadAttributeList`＋`UpdateProcThreadAttribute(PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES)`、
//!   `CreateProcess(EXTENDED_STARTUPINFO_PRESENT)`，并需给工作区加 DACL 授权；这些全部是 Win32
//!   级调用，`std::process::Command` 不暴露 `lpAttributeList`——在 `forbid(unsafe_code)` 下不可实现；
//! - Unix 家族：本批未启用 `chroot`／`jail`／`namespaces` 等机制。
//!
//! 因此两个分支当前都返回 `Unavailable`，且这是**如实结论**而非占位：`sandbox` 上报表据此
//! 只会给出 `partial`（见 `whcode_domain::sandbox`）。

use whcode_domain::sandbox::OsEnforcement;

/// 当前平台的 OS 层强制隔离事实。
///
/// 纯函数（不探测子进程、不读配置），因为“是否生效”在编译期与运行期都只有两种确定答案，
/// 用探测冒充确定性只会让 `doctor` 的输出随机器漂移。
#[must_use]
pub fn os_enforcement() -> OsEnforcement {
    #[cfg(windows)]
    {
        OsEnforcement::Unavailable {
            reason: "Windows 的 AppContainer／受限令牌需 CreateAppContainerProfile、\
                     UpdateProcThreadAttribute(SECURITY_CAPABILITIES) 与 CreateProcess 级调用，\
                     并需改写工作区 DACL；本仓 forbid(unsafe_code) 且无安全包装依赖（ADR-077）",
        }
    }
    #[cfg(not(windows))]
    {
        OsEnforcement::Unavailable {
            reason: "本批未启用 chroot／jail／namespaces 等 OS 层机制（ADR-077）",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 未生效原因必须可操作：非空、指得出 ADR，且不得用“未知”“暂不支持”搪塞。
    #[test]
    fn unavailable_reason_is_actionable() {
        let fact = os_enforcement();
        let OsEnforcement::Unavailable { reason } = fact else {
            panic!("当前两个平台分支均未启用 OS 隔离，不得报 Active");
        };
        assert!(!reason.trim().is_empty(), "原因不得为空");
        assert!(reason.contains("ADR-077"), "原因须可回溯到决策：{reason}");
        for vague in ["未知", "暂不支持", "TODO"] {
            assert!(!reason.contains(vague), "不得用含糊措辞：{reason}");
        }
    }

    /// 确定性：同一进程内多次调用必须给出同一结论（不随机器状态漂移）。
    #[test]
    fn os_enforcement_is_deterministic() {
        assert_eq!(os_enforcement(), os_enforcement());
        assert!(!os_enforcement().is_active());
    }
}
