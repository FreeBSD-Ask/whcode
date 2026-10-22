//! # Analyze：五类缺陷检出（T073、FR-306、V-203）
//!
//! 一次检出的五类：**遗漏需求**（无任务覆盖的需求）、**孤儿任务**（需求与决策引用
//! 皆空的任务）、**无效引用**（指向未知 ID）、**循环依赖**、**覆盖不足**（任务无
//! 验收条件）。未通过阻止 Implement（FR-307、应用服务 T074 执行）。
//!
//! 边界：内存分析，输入为解析产物 + Constitution 强制条款；不触库。

use std::collections::{BTreeMap, BTreeSet};

use whcode_domain::{GateId, GateResult};

use crate::ids::IdRegistry;
use crate::parser::{Clarification, ParsedRequirement, ParsedTask};

/// Analyze 报告（五类缺陷 + 各自的门禁结果）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AnalyzeReport {
    /// 遗漏需求：无任务覆盖的需求 ID。
    pub missing_requirements: Vec<String>,
    /// 孤儿任务：需求与决策引用皆空的任务 ID。
    pub orphan_tasks: Vec<String>,
    /// 无效引用：(任务, 引用) 清单。
    pub invalid_refs: Vec<(String, String)>,
    /// 循环依赖：环上的任务 ID 链。
    pub cycles: Vec<Vec<String>>,
    /// 覆盖不足：无验收条件的任务 ID。
    pub uncovered_acceptance: Vec<String>,
}

impl AnalyzeReport {
    /// 是否全部通过（五类皆空）。
    #[must_use]
    pub fn passed(&self) -> bool {
        self.missing_requirements.is_empty()
            && self.orphan_tasks.is_empty()
            && self.invalid_refs.is_empty()
            && self.cycles.is_empty()
            && self.uncovered_acceptance.is_empty()
    }

    /// Analyze 门禁结果（FR-306；未通过时理由可定位）。
    #[must_use]
    pub fn gate_result(&self) -> GateResult {
        if self.passed() {
            GateResult::pass(GateId::Analyze)
        } else {
            let mut reasons = Vec::new();
            if !self.missing_requirements.is_empty() {
                reasons.push(format!(
                    "遗漏需求：{}",
                    self.missing_requirements.join(", ")
                ));
            }
            if !self.orphan_tasks.is_empty() {
                reasons.push(format!("孤儿任务：{}", self.orphan_tasks.join(", ")));
            }
            if !self.invalid_refs.is_empty() {
                reasons.push(format!(
                    "无效引用：{}",
                    self.invalid_refs
                        .iter()
                        .map(|(task, id)| format!("{task} → {id}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            if !self.cycles.is_empty() {
                reasons.push(format!(
                    "循环依赖：{}",
                    self.cycles
                        .iter()
                        .map(|cycle| cycle.join(" → "))
                        .collect::<Vec<_>>()
                        .join("; ")
                ));
            }
            if !self.uncovered_acceptance.is_empty() {
                reasons.push(format!(
                    "覆盖不足（无验收条件）：{}",
                    self.uncovered_acceptance.join(", ")
                ));
            }
            GateResult::fail_with(GateId::Analyze, reasons)
        }
    }
}

/// 检出循环依赖：按 `depends_on` 建图后 DFS 找环。
fn find_cycles(tasks: &[ParsedTask]) -> Vec<Vec<String>> {
    let edges: BTreeMap<&str, Vec<&str>> = tasks
        .iter()
        .map(|task| {
            (
                task.id.as_str(),
                task.depends_on.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    let mut cycles = Vec::new();
    let mut visited: BTreeSet<&str> = BTreeSet::new();
    for start in tasks {
        let mut path: Vec<&str> = vec![];
        let mut on_path: BTreeSet<&str> = BTreeSet::new();
        if let Some(cycle) = dfs(
            start.id.as_str(),
            &edges,
            &mut visited,
            &mut path,
            &mut on_path,
        ) {
            cycles.push(cycle);
        }
    }
    cycles
}

fn dfs<'a>(
    node: &'a str,
    edges: &BTreeMap<&'a str, Vec<&'a str>>,
    visited: &mut BTreeSet<&'a str>,
    path: &mut Vec<&'a str>,
    on_path: &mut BTreeSet<&'a str>,
) -> Option<Vec<String>> {
    if let Some(position) = path.iter().position(|item| *item == node) {
        let cycle: Vec<String> = path[position..]
            .iter()
            .map(|item| (*item).to_string())
            .collect();
        return Some(cycle);
    }
    if visited.contains(node) {
        return None;
    }
    visited.insert(node);
    path.push(node);
    on_path.insert(node);
    for next in edges.get(node).into_iter().flatten() {
        if let Some(cycle) = dfs(next, edges, visited, path, on_path) {
            return Some(cycle);
        }
    }
    path.pop();
    on_path.remove(node);
    None
}

/// 执行 Analyze（FR-306）：五类缺陷检出 + 门禁结果。
#[must_use]
pub fn analyze(
    requirements: &[ParsedRequirement],
    tasks: &[ParsedTask],
    _clarifications: &[Clarification],
    registry: &IdRegistry,
) -> AnalyzeReport {
    let mut report = AnalyzeReport::default();

    // 遗漏需求：无任务覆盖。
    let covered: BTreeSet<&str> = tasks
        .iter()
        .flat_map(|task| task.requirement_refs.iter().map(String::as_str))
        .collect();
    report.missing_requirements = requirements
        .iter()
        .map(|req| req.id.clone())
        .filter(|id| !covered.contains(id.as_str()))
        .collect();

    // 孤儿任务 + 无效引用 + 覆盖不足。
    for task in tasks {
        if task.requirement_refs.is_empty() && task.plan_refs.is_empty() {
            report.orphan_tasks.push(task.id.clone());
        }
        for id in task
            .requirement_refs
            .iter()
            .chain(task.plan_refs.iter())
            .chain(task.depends_on.iter())
        {
            if !registry.contains(id) {
                report.invalid_refs.push((task.id.clone(), id.clone()));
            }
        }
        if task.acceptance_criteria.is_empty() {
            report.uncovered_acceptance.push(task.id.clone());
        }
    }

    // 循环依赖。
    report.cycles = find_cycles(tasks);
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{ParsedRequirement, ParsedTask};

    fn requirement(id: &str) -> ParsedRequirement {
        ParsedRequirement {
            id: id.to_string(),
            text: "需求".to_string(),
            raw: String::new(),
        }
    }

    fn task(
        id: &str,
        refs: &[&str],
        plans: &[&str],
        acceptance: &[&str],
        depends: &[&str],
    ) -> ParsedTask {
        ParsedTask {
            id: id.to_string(),
            title: format!("任务 {id}"),
            requirement_refs: refs.iter().map(|item| (*item).to_string()).collect(),
            plan_refs: plans.iter().map(|item| (*item).to_string()).collect(),
            acceptance_criteria: acceptance.iter().map(|item| (*item).to_string()).collect(),
            depends_on: depends.iter().map(|item| (*item).to_string()).collect(),
            raw: String::new(),
        }
    }

    #[test]
    fn clean_project_passes_analyze() {
        let mut registry = IdRegistry::new();
        registry.register("REQ-1").expect("登记");
        registry.register("DEC-1").expect("登记");
        let report = analyze(
            &[requirement("REQ-1")],
            &[task("T-1", &["REQ-1"], &["DEC-1"], &["断言"], &[])],
            &[],
            &registry,
        );
        assert!(report.passed(), "{report:?}");
        assert!(report.gate_result().passed);
    }

    #[test]
    fn five_defects_are_detected_and_gate_fails() {
        let mut registry = IdRegistry::new();
        registry.register("REQ-1").expect("登记");
        let report = analyze(
            &[requirement("REQ-1"), requirement("REQ-2")],
            &[
                task("T-orphan", &[], &[], &[], &[]), // 孤儿 + 覆盖不足
                task("T-badref", &["REQ-404"], &[], &["断言"], &[]), // 无效引用
                task("T-c1", &["REQ-1"], &[], &["断言"], &["T-c2"]), // 循环依赖
                task("T-c2", &["REQ-1"], &[], &["断言"], &["T-c1"]),
            ],
            &[],
            &registry,
        );
        assert!(!report.passed());
        assert_eq!(report.missing_requirements, vec!["REQ-2"], "遗漏需求");
        assert!(
            report.orphan_tasks.contains(&"T-orphan".to_string()),
            "孤儿任务"
        );
        assert!(
            report
                .invalid_refs
                .contains(&("T-badref".to_string(), "REQ-404".to_string())),
            "无效引用：{:?}",
            report.invalid_refs
        );
        assert!(!report.cycles.is_empty(), "循环依赖：{:?}", report.cycles);
        assert!(
            report
                .uncovered_acceptance
                .contains(&"T-orphan".to_string()),
            "覆盖不足"
        );

        let gate = report.gate_result();
        assert!(!gate.passed, "未通过阻止 Implement（FR-307）");
        assert_eq!(gate.gate, GateId::Analyze);
        assert!(
            gate.reasons
                .iter()
                .any(|reason| reason.contains("遗漏需求")),
            "理由可定位：{:?}",
            gate.reasons
        );
    }
}
