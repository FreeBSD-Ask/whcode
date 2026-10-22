//! # Converge：验收条件判定与差异报告（T076/T077、FR-310/FR-311、V-205）
//!
//! [`converge`] 对每条验收条件输出 `pass / fail / not_tested / not_applicable`
//! 之一并附**证据引用**（FR-310）；存在 `fail`/`not_tested` 时生成**差异报告**，
//! 任务不得静默标记完成（FR-311、V-205）——差异项由调用方追加为可追溯任务并循环
//! （契约 §1 Converge 循环）。
//!
//! 可绑定运行时测试的判定（FR-709）：证据引用来自调用方提供的核对结果；本模块
//! 只做判定归纳与差异生成，不执行运行时（运行时属 M8/`whcode-runtime`）。

use whcode_domain::WhError;

/// 验收条件判定（FR-310 四态）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcceptanceStatus {
    /// 通过。
    Pass,
    /// 失败。
    Fail,
    /// 未测试。
    NotTested,
    /// 不适用（须附不可自动化说明，FR-310 校验）。
    NotApplicable,
}

impl AcceptanceStatus {
    /// 稳定序列化名。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::NotTested => "not_tested",
            Self::NotApplicable => "not_applicable",
        }
    }
}

/// 单条验收条件的判定结果（附证据引用，FR-310）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptanceResult {
    /// 验收条件原文。
    pub criterion: String,
    /// 判定。
    pub status: AcceptanceStatus,
    /// 证据引用（产物/运行结果引用；`not_applicable` 须附说明）。
    pub evidence_ref: Option<String>,
}

/// 运行时测试绑定（T108、FR-709）：命令、环境指纹、退出状态、结果引用。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RuntimeBinding {
    /// 绑定的运行时名。
    pub runtime: String,
    /// 测试命令。
    pub command: String,
    /// 环境指纹（运行时版本 + 平台，FR-709）。
    pub env_fingerprint: String,
    /// 退出状态。
    pub exit_status: Option<i32>,
    /// 结果引用（产物/运行记录 ID）。
    pub result_ref: Option<String>,
}

impl RuntimeBinding {
    /// 判定运行时测试结果：退出码 0 → `pass`（附结果引用）；非零 → `fail`；
    /// 未运行 → `not_tested`（FR-709、FR-310 四态）。
    #[must_use]
    pub fn acceptance_result(&self, criterion: &str) -> AcceptanceResult {
        match self.exit_status {
            Some(0) => AcceptanceResult {
                criterion: criterion.to_string(),
                status: AcceptanceStatus::Pass,
                evidence_ref: Some(
                    self.result_ref
                        .clone()
                        .unwrap_or_else(|| format!("runtime:{}:{}", self.runtime, self.command)),
                ),
            },
            Some(code) => AcceptanceResult {
                criterion: criterion.to_string(),
                status: AcceptanceStatus::Fail,
                evidence_ref: Some(format!(
                    "runtime:{}:{}:exit={code}",
                    self.runtime, self.command
                )),
            },
            None => AcceptanceResult {
                criterion: criterion.to_string(),
                status: AcceptanceStatus::NotTested,
                evidence_ref: None,
            },
        }
    }
}

/// 差异项（FR-311 差异报告的一行）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Difference {
    /// 任务 ID。
    pub task_id: String,
    /// 验收条件原文。
    pub criterion: String,
    /// 判定。
    pub status: AcceptanceStatus,
    /// 差异说明（调用方追加为可追溯任务的依据）。
    pub gap: String,
}

/// Converge 报告（FR-310/311）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConvergeReport {
    /// 任务 ID。
    pub task_id: String,
    /// 每条验收条件的判定。
    pub results: Vec<AcceptanceResult>,
    /// 差异项（fail / not_tested / 证据缺失）。
    pub differences: Vec<Difference>,
}

impl ConvergeReport {
    /// 是否全部通过（可标记 completed 的前提，FR-311）。
    ///
    /// `not_applicable` 附说明（无差异）视为通过；`fail`/`not_tested` 一律不算。
    #[must_use]
    pub fn passed(&self) -> bool {
        self.differences.is_empty()
            && self.results.iter().all(|result| {
                matches!(
                    result.status,
                    AcceptanceStatus::Pass | AcceptanceStatus::NotApplicable
                )
            })
    }
}

/// 归纳验收条件判定并生成差异报告（FR-310/311）。
///
/// 规则：
/// - `not_applicable` 必须附说明（`evidence_ref`），否则视为差异；
/// - `pass` 必须附证据引用，否则视为差异（不以口头通过结项）；
/// - `fail` / `not_tested` 一律进入差异报告（V-205：不得静默标记完成）。
///
/// # Errors
///
/// 验收条件清单为空时返回 `invalid_state`（空验收条件无法核对，FR-304 校验）。
pub fn converge(
    task_id: &str,
    acceptance_criteria: &[String],
    results: &[AcceptanceResult],
) -> Result<ConvergeReport, WhError> {
    if acceptance_criteria.is_empty() {
        return Err(WhError::invalid_state(format!(
            "任务 {task_id} 无验收条件：无法核对（FR-304/310）"
        )));
    }
    let mut ordered = Vec::new();
    let mut differences = Vec::new();
    for criterion in acceptance_criteria {
        let result = results
            .iter()
            .find(|result| &result.criterion == criterion)
            .cloned()
            .unwrap_or(AcceptanceResult {
                criterion: criterion.clone(),
                status: AcceptanceStatus::NotTested,
                evidence_ref: None,
            });
        match result.status {
            AcceptanceStatus::Fail => differences.push(Difference {
                task_id: task_id.to_string(),
                criterion: result.criterion.clone(),
                status: result.status,
                gap: format!(
                    "验收失败：{}",
                    result.evidence_ref.as_deref().unwrap_or("无证据")
                ),
            }),
            AcceptanceStatus::NotTested => differences.push(Difference {
                task_id: task_id.to_string(),
                criterion: result.criterion.clone(),
                status: result.status,
                gap: "未测试：无运行结果引用".to_string(),
            }),
            AcceptanceStatus::Pass if result.evidence_ref.is_none() => {
                differences.push(Difference {
                    task_id: task_id.to_string(),
                    criterion: result.criterion.clone(),
                    status: result.status,
                    gap: "通过但缺少证据引用（FR-310）".to_string(),
                })
            }
            AcceptanceStatus::NotApplicable if result.evidence_ref.is_none() => {
                differences.push(Difference {
                    task_id: task_id.to_string(),
                    criterion: result.criterion.clone(),
                    status: result.status,
                    gap: "不适用但缺少说明（FR-310：须附不可自动化说明）".to_string(),
                })
            }
            _ => {}
        }
        ordered.push(result);
    }
    Ok(ConvergeReport {
        task_id: task_id.to_string(),
        results: ordered,
        differences,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn criteria() -> Vec<String> {
        vec!["存在可执行断言".to_string(), "人工检查通过".to_string()]
    }

    #[test]
    fn all_pass_with_evidence_yields_no_differences() {
        let results = vec![
            AcceptanceResult {
                criterion: "存在可执行断言".to_string(),
                status: AcceptanceStatus::Pass,
                evidence_ref: Some("A-001".to_string()),
            },
            AcceptanceResult {
                criterion: "人工检查通过".to_string(),
                status: AcceptanceStatus::NotApplicable,
                evidence_ref: Some("不可自动化说明：仅人工可验证".to_string()),
            },
        ];
        let report = converge("T-1", &criteria(), &results).expect("核对");
        assert!(report.passed(), "{report:?}");
        assert!(report.differences.is_empty());
        assert_eq!(report.results.len(), 2);
    }

    #[test]
    fn fail_and_not_tested_produce_differences_not_silent_completion() {
        let results = vec![
            AcceptanceResult {
                criterion: "存在可执行断言".to_string(),
                status: AcceptanceStatus::Fail,
                evidence_ref: Some("A-002".to_string()),
            },
            AcceptanceResult {
                criterion: "人工检查通过".to_string(),
                status: AcceptanceStatus::NotTested,
                evidence_ref: None,
            },
        ];
        let report = converge("T-1", &criteria(), &results).expect("核对");
        assert!(!report.passed(), "不得静默标记完成（V-205）");
        assert_eq!(report.differences.len(), 2);
        assert_eq!(report.differences[0].status, AcceptanceStatus::Fail);
        assert_eq!(report.differences[1].status, AcceptanceStatus::NotTested);
    }

    #[test]
    fn pass_without_evidence_and_na_without_note_are_differences() {
        let results = vec![
            AcceptanceResult {
                criterion: "存在可执行断言".to_string(),
                status: AcceptanceStatus::Pass,
                evidence_ref: None,
            },
            AcceptanceResult {
                criterion: "人工检查通过".to_string(),
                status: AcceptanceStatus::NotApplicable,
                evidence_ref: None,
            },
        ];
        let report = converge("T-1", &criteria(), &results).expect("核对");
        assert!(!report.passed(), "无证据的通过与无说明的不适用都是差异");
        assert_eq!(report.differences.len(), 2);
    }

    #[test]
    fn runtime_binding_maps_exit_status_to_acceptance_states() {
        // T108/FR-709：命令、环境指纹、退出状态、结果引用。
        let binding = RuntimeBinding {
            runtime: "python".to_string(),
            command: "python -m pytest".to_string(),
            env_fingerprint: "python-3.12/windows-amd64".to_string(),
            exit_status: Some(0),
            result_ref: Some("R-1".to_string()),
        };
        let result = binding.acceptance_result("存在可执行断言");
        assert_eq!(result.status, AcceptanceStatus::Pass);
        assert_eq!(result.evidence_ref.as_deref(), Some("R-1"));

        let failed = RuntimeBinding {
            exit_status: Some(3),
            ..binding.clone()
        };
        assert_eq!(
            failed.acceptance_result("存在可执行断言").status,
            AcceptanceStatus::Fail
        );

        let not_run = RuntimeBinding {
            exit_status: None,
            ..binding
        };
        assert_eq!(
            not_run.acceptance_result("存在可执行断言").status,
            AcceptanceStatus::NotTested
        );
    }

    #[test]
    fn untested_criterion_defaults_to_not_tested_and_empty_criteria_rejected() {
        let report = converge("T-1", &criteria(), &[]).expect("缺省核对");
        assert!(!report.passed());
        assert!(
            report
                .results
                .iter()
                .all(|result| result.status == AcceptanceStatus::NotTested)
        );

        let err = converge("T-1", &[], &[]).expect_err("空验收条件必须拒绝");
        assert!(err.to_string().contains("无验收条件"));
    }
}
