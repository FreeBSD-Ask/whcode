//! # 执行沙箱强度与如实上报（P5.2／ADR-077）
//!
//! 目的只有一个：**不把“工具级治理”说成“OS 沙箱”**。三档强度按“实际生效的面”判定，
//! 不按意图或配置文件名判定：
//!
//! | 强度 | 判据 |
//! |---|---|
//! | `none` | 模型没有任意命令执行面（shell 工具未启用）——此时不存在可上报的沙箱，也不宣称有 |
//! | `partial` | 有执行面，且只有工具级控制（cwd 锚点、最小环境、凭据剥离、输出截断、超时杀树、并发闸门），OS 层无强制隔离 |
//! | `full` | 有执行面，且 OS 层强制隔离生效、残余缺口清零（**当前不可达**：Windows 需 Win32 级调用，本仓 `forbid(unsafe_code)`） |
//!
//! [`TOOL_CONTROLS`] 与 [`OS_GAPS`] 是**静态事实表**：控制项逐条对应既有实现（注释给出
//! 落点与 ADR-070 威胁编号），缺口逐条对应 ADR-070 登记至 P5.2 的残余风险。表里不许出现
//! “计划中”的条目——上报面只写当前为真的东西，否则就是伪造证据（AGENTS §8）。
//!
//! 边界：本模块是**叶子类型与判定规则**，不含平台探测（`whcode-platform`）也不含输出装配
//! （装配根）；三种强度与缺口描述的文案同源，文本与 JSON 不得各自表述。

use serde::Serialize;

/// 沙箱强度档位（JSON 取 `none`／`partial`／`full`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxStrength {
    /// 无执行面：模型不能发起命令，故不存在可上报的沙箱。
    None,
    /// 仅工具级控制：cwd 锚点／环境收敛／输出与时长有界，OS 层不隔离文件与网络。
    Partial,
    /// OS 层强制隔离生效且残余缺口清零（当前不可达，见 ADR-077）。
    Full,
}

impl SandboxStrength {
    /// 中文标签（`doctor` 文本行；与 JSON 的 `snake_case` 同源同义）。
    #[must_use]
    pub fn label_zh(self) -> &'static str {
        match self {
            Self::None => "无（未启用命令执行面）",
            Self::Partial => "部分（仅工具级控制，无 OS 隔离）",
            Self::Full => "完整（OS 层强制隔离）",
        }
    }
}

/// OS 层强制隔离的当前事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum OsEnforcement {
    /// 已生效的 OS 机制（附机制名）。
    Active {
        /// 机制名（如 `appcontainer`／`jail`）。
        mechanism: &'static str,
    },
    /// 未生效：给出可操作原因（不得留空，也不得写“暂不支持”这类无法核验的免责话）。
    Unavailable {
        /// 原因（静态文案）。
        reason: &'static str,
    },
}

impl OsEnforcement {
    /// 是否已生效。
    #[must_use]
    pub fn is_active(self) -> bool {
        matches!(self, Self::Active { .. })
    }
}

/// 一条工具级控制（已实现、可复核）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SandboxControl {
    /// 稳定标识（JSON 键；改动即协议变更）。
    pub id: &'static str,
    /// 人读描述。
    pub description: &'static str,
    /// 实现落点（模块路径＋ADR-070 威胁编号）。
    pub source: &'static str,
}

/// 一条尚未收敛的缺口（对应 ADR-070 登记的残余风险）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SandboxGap {
    /// 稳定标识。
    pub id: &'static str,
    /// 人读描述（失败路径也写清后果，不只写“不支持”）。
    pub description: &'static str,
    /// 归属威胁编号。
    pub threat: &'static str,
}

/// 工具级控制表（六项，全部已在执行骨干内生效）。
pub const TOOL_CONTROLS: [SandboxControl; 6] = [
    SandboxControl {
        id: "cwd_anchor",
        description: "子进程 cwd 固定为工作区根（workdir_policy = workspace）",
        source: "whcode-runtime::provider（ADR-070 T6）",
    },
    SandboxControl {
        id: "minimal_env",
        description: "子进程只继承最小环境变量集",
        source: "whcode-runtime::worker::apply_minimal_env（ADR-070 T3）",
    },
    SandboxControl {
        id: "credential_strip",
        description: "按名剥离凭据型环境变量，只记名不记值",
        source: "whcode-runtime::worker::strip_credentials（ADR-070 T3）",
    },
    SandboxControl {
        id: "output_truncation",
        description: "输出按上限截断并给显式 truncated 标记",
        source: "whcode-tools::MAX_TOOL_OUTPUT_BYTES（ADR-070 T5／R-05）",
    },
    SandboxControl {
        id: "timeout_tree_kill",
        description: "请求超时钳制在上限内，超时终止整个进程树",
        source: "whcode-tools::MAX_TOOL_TIMEOUT_MS＋whcode-platform::process（ADR-070 T5）",
    },
    SandboxControl {
        id: "concurrency_gate",
        description: "全局与 gitbash 专用并发闸门（有界并发）",
        source: "whcode-runtime::worker::execute_with_cancel（ADR-070 T5）",
    },
];

/// OS 层未隔离时仍存在的缺口表（五项，逐条对应 ADR-070 残余风险登记）。
pub const OS_GAPS: [SandboxGap; 5] = [
    SandboxGap {
        id: "filesystem_write",
        description: "脚本可用绝对路径写工作区外的文件（cwd 是锚点不是监狱）",
        threat: "ADR-070 T6",
    },
    SandboxGap {
        id: "filesystem_read",
        description: "脚本可读磁盘上的凭据文件（如 ~/.ssh），无文件读隔离",
        threat: "ADR-070 T3",
    },
    SandboxGap {
        id: "network",
        description: "子进程网络不受限，无出站目标约束",
        threat: "ADR-070 信任边界",
    },
    SandboxGap {
        id: "disk_quota",
        description: "磁盘写入无配额，超时窗口内可持续写盘",
        threat: "ADR-070 T5",
    },
    SandboxGap {
        id: "secondary_command_path",
        description: "脚本内部的二级命令按已收敛 PATH 解析，仍在 PATH 内查找",
        threat: "ADR-070 T4",
    },
];

/// 沙箱强度上报（`doctor` 的 `sandbox` 块；文本与 JSON 同源）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SandboxReport {
    /// 强度档位。
    pub strength: SandboxStrength,
    /// 是否存在模型可发起的命令执行面（shell 工具是否注册）。
    pub execution_surface: bool,
    /// OS 层强制隔离的当前事实。
    pub os_enforcement: OsEnforcement,
    /// 已生效的工具级控制（无执行面时为空）。
    pub controls: Vec<SandboxControl>,
    /// 尚未收敛的缺口（OS 层隔离生效时为空）。
    pub gaps: Vec<SandboxGap>,
}

impl SandboxReport {
    /// 按“实际生效的面”判定强度并装配上报面。
    ///
    /// 判定规则（顺序即优先级，见 ADR-077 决策 2）：无执行面 → `none`；有执行面且 OS 层
    /// 隔离生效且缺口清零 → `full`；其余 → `partial`。**不得**据配置意图或工具存在与否
    /// 直接给 `full`。
    #[must_use]
    pub fn evaluate(execution_surface: bool, os_enforcement: OsEnforcement) -> Self {
        let controls = if execution_surface {
            TOOL_CONTROLS.to_vec()
        } else {
            Vec::new()
        };
        let gaps = if os_enforcement.is_active() {
            Vec::new()
        } else {
            OS_GAPS.to_vec()
        };
        let strength = if !execution_surface {
            SandboxStrength::None
        } else if gaps.is_empty() {
            SandboxStrength::Full
        } else {
            SandboxStrength::Partial
        };
        Self {
            strength,
            execution_surface,
            os_enforcement,
            controls,
            gaps,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NO_OS: OsEnforcement = OsEnforcement::Unavailable {
        reason: "测试用：OS 层未启用",
    };

    /// 无执行面 → `none`，且不列控制项（不宣称未启用的能力）。
    #[test]
    fn disabled_surface_reports_none_without_controls() {
        let report = SandboxReport::evaluate(false, NO_OS);
        assert_eq!(report.strength, SandboxStrength::None);
        assert!(report.controls.is_empty(), "未启用执行面不得列控制项");
        assert!(!report.gaps.is_empty(), "OS 缺口与执行面无关，必须照实列出");
    }

    /// 有执行面、无 OS 隔离 → `partial`，控制项与缺口都在。
    #[test]
    fn enabled_surface_without_os_isolation_is_partial() {
        let report = SandboxReport::evaluate(true, NO_OS);
        assert_eq!(report.strength, SandboxStrength::Partial);
        assert_eq!(report.controls.len(), TOOL_CONTROLS.len());
        assert_eq!(report.gaps.len(), OS_GAPS.len());
    }

    /// `full` 只在 OS 隔离生效时成立：同为有执行面，隔离生效即 `full` 且缺口清零。
    /// 这同时钉住“Windows 上永远不可能凭配置拿到 full”的实现边界。
    #[test]
    fn full_requires_active_os_enforcement() {
        let active = SandboxReport::evaluate(
            true,
            OsEnforcement::Active {
                mechanism: "test-mechanism",
            },
        );
        assert_eq!(active.strength, SandboxStrength::Full);
        assert!(active.gaps.is_empty(), "隔离生效时缺口必须清零");

        let inactive = SandboxReport::evaluate(true, NO_OS);
        assert_ne!(inactive.strength, SandboxStrength::Full);
    }

    /// 控制项与缺口的标识唯一且非空——重复 id 会让 JSON 消费方按 id 索引时丢条目。
    #[test]
    fn control_and_gap_ids_are_unique_and_nonempty() {
        let mut control_ids: Vec<&str> = TOOL_CONTROLS.iter().map(|item| item.id).collect();
        control_ids.sort_unstable();
        let before = control_ids.len();
        control_ids.dedup();
        assert_eq!(before, control_ids.len(), "控制项 id 必须唯一");
        assert!(TOOL_CONTROLS.iter().all(|item| !item.id.is_empty()));

        let mut gap_ids: Vec<&str> = OS_GAPS.iter().map(|item| item.id).collect();
        gap_ids.sort_unstable();
        let before = gap_ids.len();
        gap_ids.dedup();
        assert_eq!(before, gap_ids.len(), "缺口 id 必须唯一");
        assert!(
            OS_GAPS
                .iter()
                .all(|item| !item.id.is_empty() && item.threat.starts_with("ADR-070")),
            "缺口必须指向 ADR-070 的威胁编号"
        );
    }

    /// JSON 形状：强度是 snake_case 字符串，OS 事实带 `state` 判别标签。
    #[test]
    fn serializes_strength_and_os_state_stably() {
        let report = SandboxReport::evaluate(true, NO_OS);
        let value = serde_json::to_value(&report).expect("序列化");
        assert_eq!(value["strength"], serde_json::json!("partial"));
        assert_eq!(value["execution_surface"], serde_json::json!(true));
        assert_eq!(
            value["os_enforcement"]["state"],
            serde_json::json!("unavailable")
        );
        assert!(
            !value["os_enforcement"]["reason"]
                .as_str()
                .unwrap_or_default()
                .is_empty(),
            "未生效原因不得为空"
        );
        assert_eq!(
            value["controls"].as_array().map(Vec::len),
            Some(TOOL_CONTROLS.len())
        );
    }
}
