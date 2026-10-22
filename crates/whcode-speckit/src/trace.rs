//! # 双向追溯（T072、FR-305）
//!
//! Plan 关键决策可追溯至需求；任务可追溯至需求与计划决策（FR-305）。
//! [`forward_trace`] 从任务出发（任务 → 需求 + 决策）；[`backward_trace`] 从需求
//! 出发（需求 → 任务 + 决策）。断链（任务缺失引用）由 [`trace_gaps`] 检出——
//! FR-305 要求可追溯，静默断链即追溯失效。
//!
//! 边界：内存结构（产物解析结果之上的投影）；持久化属产物链（M4）。

use std::collections::{BTreeMap, BTreeSet};

use crate::parser::ParsedTask;

/// 追溯索引：任务 → 需求/决策 的映射 + 反向投影。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Traceability {
    /// 任务 ID → 需求引用。
    pub task_to_requirements: BTreeMap<String, BTreeSet<String>>,
    /// 任务 ID → 计划决策引用。
    pub task_to_decisions: BTreeMap<String, BTreeSet<String>>,
}

impl Traceability {
    /// 从解析出的任务清单构建追溯索引。
    #[must_use]
    pub fn from_tasks(tasks: &[ParsedTask]) -> Self {
        let mut trace = Self::default();
        for task in tasks {
            trace.task_to_requirements.insert(
                task.id.clone(),
                task.requirement_refs.iter().cloned().collect(),
            );
            trace
                .task_to_decisions
                .insert(task.id.clone(), task.plan_refs.iter().cloned().collect());
        }
        trace
    }

    /// 正向追溯：任务 → 需求 + 计划决策（FR-305）。
    #[must_use]
    pub fn forward_trace(&self, task_id: &str) -> Option<(Vec<String>, Vec<String>)> {
        let requirements = self.task_to_requirements.get(task_id)?;
        let decisions = self.task_to_decisions.get(task_id)?;
        Some((
            requirements.iter().cloned().collect(),
            decisions.iter().cloned().collect(),
        ))
    }

    /// 反向追溯：需求 → 引用它的任务 + 决策（FR-305 双向）。
    #[must_use]
    pub fn backward_trace(&self, requirement_id: &str) -> (Vec<String>, BTreeSet<String>) {
        let mut tasks = Vec::new();
        let mut decisions = BTreeSet::new();
        for (task_id, requirements) in &self.task_to_requirements {
            if requirements.contains(requirement_id) {
                tasks.push(task_id.clone());
                if let Some(task_decisions) = self.task_to_decisions.get(task_id) {
                    decisions.extend(task_decisions.iter().cloned());
                }
            }
        }
        (tasks, decisions)
    }

    /// 追溯断链：没有任何需求引用的任务 ID（FR-305 要求任务可追溯至需求）。
    #[must_use]
    pub fn trace_gaps(&self) -> Vec<String> {
        self.task_to_requirements
            .iter()
            .filter(|(_, requirements)| requirements.is_empty())
            .map(|(task_id, _)| task_id.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, refs: &[&str], plans: &[&str]) -> ParsedTask {
        ParsedTask {
            id: id.to_string(),
            title: format!("任务 {id}"),
            requirement_refs: refs.iter().map(|item| (*item).to_string()).collect(),
            plan_refs: plans.iter().map(|item| (*item).to_string()).collect(),
            acceptance_criteria: vec![],
            depends_on: vec![],
            raw: String::new(),
        }
    }

    #[test]
    fn forward_and_backward_traces_are_bidirectional() {
        let trace = Traceability::from_tasks(&[
            task("T-1", &["REQ-1", "REQ-2"], &["DEC-1"]),
            task("T-2", &["REQ-1"], &[]),
        ]);

        // 正向：任务 → 需求 + 决策（FR-305）。
        let (requirements, decisions) = trace.forward_trace("T-1").expect("正向");
        assert_eq!(requirements, vec!["REQ-1", "REQ-2"]);
        assert_eq!(decisions, vec!["DEC-1"]);

        // 反向：需求 → 任务 + 决策（FR-305 双向）。
        let (tasks, decisions) = trace.backward_trace("REQ-1");
        assert_eq!(tasks, vec!["T-1", "T-2"]);
        assert!(decisions.contains("DEC-1"));
    }

    #[test]
    fn trace_gaps_detect_tasks_without_requirement_refs() {
        let trace =
            Traceability::from_tasks(&[task("T-1", &["REQ-1"], &[]), task("T-orphan", &[], &[])]);
        assert_eq!(trace.trace_gaps(), vec!["T-orphan"], "断链可检出，不静默");
    }
}
