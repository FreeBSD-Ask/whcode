//! # SDD 产物 Markdown 解析器（T069/T075、FR-303/FR-309、R-06、contracts/sdd-artifacts.md §4）
//!
//! 解析器必须报告：**无法解析的段落、重复 ID、未识别字段**——禁止静默丢弃信息
//! （FR-309、R-06）。产物默认保留可读 Markdown，内部解析为版本化结构（FR-303），
//! 不假定原始 Markdown 是 JSON Schema 文档。
//!
//! ## 纠偏重试（T075、FR-309）
//!
//! [`parse_with_retry`]：首次按原文解析；失败后自动纠偏重试（默认最多 2 次，纠偏
//! 策略逐级宽松：空白归一 → 列表符与加粗容忍）；仍失败则返回 [`ParseBlocked`]——
//! **保留错误与原始产物原文**，任务标记 `blocked`（由调用方执行，本模块携带原文）。
//!
//! 解析器带版本号 [`PARSER_VERSION`]，随产物校验结果一同持久化。
//!
//! ## 产物格式（本解析器的既定输入，FR-303）
//!
//! ```markdown
//! ## Requirements
//! - **REQ-001**: 需求文本
//!
//! ## Tasks
//! - [ ] T-001: 任务标题 (refs: REQ-001) (plan: DEC-001) (acceptance: 断言一; 断言二) (depends: T-000)
//!
//! ## Clarifications
//! - [ ] **CLAR-001**: 澄清问题 [priority: high]
//! ```

use whcode_domain::WhError;

/// 解析器版本（随产物校验结果持久化）。
pub const PARSER_VERSION: &str = "speckit-parser-1";

/// 默认纠偏重试次数（FR-309：默认最多 2 次）。
pub const DEFAULT_CORRECTION_RETRIES: usize = 2;

/// 解析出的需求条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedRequirement {
    /// 稳定 ID（`REQ-001`）。
    pub id: String,
    /// 需求文本。
    pub text: String,
    /// 原始行（R-06 原文保留）。
    pub raw: String,
}

/// 解析出的任务条目（contracts/sdd-artifacts.md §3 规范化任务对象）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedTask {
    /// 稳定 ID（`T-001`）。
    pub id: String,
    /// 标题。
    pub title: String,
    /// 需求引用。
    pub requirement_refs: Vec<String>,
    /// 计划决策引用。
    pub plan_refs: Vec<String>,
    /// 验收条件。
    pub acceptance_criteria: Vec<String>,
    /// 依赖任务。
    pub depends_on: Vec<String>,
    /// 原始行。
    pub raw: String,
}

/// 解析出的澄清条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clarification {
    /// 稳定 ID（`CLAR-001`）。
    pub id: String,
    /// 澄清问题文本。
    pub text: String,
    /// 是否已解决（`[x]`）。
    pub resolved: bool,
    /// 高优先级（`[priority: high]`）。
    pub high_priority: bool,
    /// 原始行。
    pub raw: String,
}

/// 解析报告：结构化结果 + 全部不可静默丢弃的异常（FR-309）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParseReport {
    /// 解析出的需求。
    pub requirements: Vec<ParsedRequirement>,
    /// 解析出的任务。
    pub tasks: Vec<ParsedTask>,
    /// 解析出的澄清项。
    pub clarifications: Vec<Clarification>,
    /// 无法解析的段落/行（原文保留）。
    pub unparsed_sections: Vec<String>,
    /// 重复 ID。
    pub duplicate_ids: Vec<String>,
    /// 未识别字段（键名保留）。
    pub unknown_fields: Vec<String>,
}

impl ParseReport {
    /// 是否无任何异常（无未解析段落、重复 ID、未识别字段）。
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.unparsed_sections.is_empty()
            && self.duplicate_ids.is_empty()
            && self.unknown_fields.is_empty()
    }

    /// 是否解析出任何条目（空产物视为解析失败——无条目即无信息，FR-309）。
    #[must_use]
    pub fn is_empty_artifact(&self) -> bool {
        self.requirements.is_empty()
            && self.tasks.is_empty()
            && self.clarifications.is_empty()
            && self.unparsed_sections.is_empty()
    }

    /// 异常摘要（纠偏错误与 `ParseBlocked.errors` 用）。
    #[must_use]
    pub fn anomaly_summary(&self) -> Vec<String> {
        let mut errors = Vec::new();
        if !self.unparsed_sections.is_empty() {
            errors.push(format!("无法解析段落 {} 处", self.unparsed_sections.len()));
        }
        if !self.duplicate_ids.is_empty() {
            errors.push(format!("重复 ID：{}", self.duplicate_ids.join(", ")));
        }
        if !self.unknown_fields.is_empty() {
            errors.push(format!("未识别字段：{}", self.unknown_fields.join(", ")));
        }
        errors
    }
}

/// 纠偏重试仍失败：任务须标记 `blocked`，原文与错误保留（FR-309、V-204）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseBlocked {
    /// 已尝试次数（含首次与纠偏）。
    pub attempts: usize,
    /// 每次的异常摘要。
    pub errors: Vec<String>,
    /// 原始产物原文（R-06 原文保留）。
    pub original: String,
}

/// 提取 `## 标题` 段落正文（到下一个 `## ` 或文末）。
fn section<'a>(md: &'a str, heading: &str) -> Option<&'a str> {
    let marker = format!("## {heading}");
    let start = md.find(&marker)? + marker.len();
    let rest = &md[start..];
    let end = rest
        .find("\n## ")
        .map(|offset| offset + 1)
        .unwrap_or(rest.len());
    Some(rest[..end].trim())
}

/// 解析行内标注 `(key: v1; v2)`；返回 (refs, plan, acceptance, depends, unknown)。
type Annotations = (
    Vec<String>,
    Vec<String>,
    Vec<String>,
    Vec<String>,
    Vec<String>,
);

fn annotations(line: &str) -> Annotations {
    let mut refs = Vec::new();
    let mut plan = Vec::new();
    let mut acceptance = Vec::new();
    let mut depends = Vec::new();
    let mut unknown = Vec::new();
    let mut rest = line;
    while let Some(open) = rest.find('(') {
        let Some(close) = rest[open..].find(')') else {
            break;
        };
        let inner = &rest[open + 1..open + close];
        rest = &rest[open + close + 1..];
        let Some((key, values)) = inner.split_once(':') else {
            unknown.push(inner.trim().to_string());
            continue;
        };
        let parsed: Vec<String> = values
            .split([',', ';'])
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
            .collect();
        match key.trim().to_ascii_lowercase().as_str() {
            "refs" | "ref" | "requirement_refs" => refs = parsed,
            "plan" | "decisions" | "plan_refs" => plan = parsed,
            "acceptance" | "acceptance_criteria" => acceptance = parsed,
            "depends" | "depends_on" => depends = parsed,
            other => unknown.push(other.to_string()),
        }
    }
    (refs, plan, acceptance, depends, unknown)
}

/// 剥掉行首列表符与复选框，返回剩余正文。
fn strip_bullet(line: &str, loose: bool) -> Option<&str> {
    let trimmed = line.trim_start();
    if !(trimmed.starts_with("- ") || (loose && trimmed.starts_with("* "))) {
        return None;
    }
    let body = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .map(str::trim_start)?;
    Some(body)
}

fn strip_checkbox(body: &str) -> (bool, &str) {
    if let Some(rest) = body.strip_prefix("[x] ").or(body.strip_prefix("[X] ")) {
        (true, rest)
    } else if let Some(rest) = body.strip_prefix("[ ] ") {
        (false, rest)
    } else {
        (false, body)
    }
}

/// 提取加粗 ID（`**ID**:`），宽松模式容忍无加粗。
fn bold_id(body: &str, loose: bool) -> Option<(&str, &str)> {
    if let Some(rest) = body.strip_prefix("**") {
        let end = rest.find("**")?;
        let id = &rest[..end];
        let after = rest[end + 2..].strip_prefix(':')?;
        Some((id, after.trim_start()))
    } else if loose {
        let (id, after) = body.split_once(':')?;
        Some((id.trim(), after.trim_start()))
    } else {
        None
    }
}

fn parse_id(raw: &str) -> String {
    raw.trim().trim_matches('*').trim().to_string()
}

/// 已知段落标题；其余段落的行进入 `unparsed_sections`（不静默丢弃）。
const KNOWN_SECTIONS: [&str; 3] = ["Requirements", "Tasks", "Clarifications"];

fn unknown_section_lines(md: &str, out: &mut Vec<String>) {
    let mut rest = md;
    while let Some(start) = rest.find("\n## ") {
        let after_heading_marker = &rest[start + 4..];
        let heading_end = after_heading_marker
            .find('\n')
            .map(|offset| start + 4 + offset)
            .unwrap_or(rest.len());
        let heading = rest[start + 4..heading_end].trim();
        if !KNOWN_SECTIONS
            .iter()
            .any(|known| heading.starts_with(known))
        {
            let body_end = rest[heading_end..]
                .find("\n## ")
                .map(|offset| heading_end + offset)
                .unwrap_or(rest.len());
            for line in rest[heading_end..body_end]
                .lines()
                .filter(|line| !line.trim().is_empty())
            {
                out.push(line.trim().to_string());
            }
        }
        rest = &rest[heading_end..];
    }
}

/// 解析 SDD 产物（T069）：报告未解析段落、重复 ID、未识别字段，不静默丢弃。
#[must_use]
pub fn parse_artifact(md: &str) -> ParseReport {
    let mut report = ParseReport::default();
    let mut seen: Vec<String> = Vec::new();
    unknown_section_lines(md, &mut report.unparsed_sections);

    if let Some(body) = section(md, "Requirements") {
        for line in body.lines().filter(|line| !line.trim().is_empty()) {
            let Some(item) = strip_bullet(line, false).and_then(|body| bold_id(body, false)) else {
                report.unparsed_sections.push(line.trim().to_string());
                continue;
            };
            let id = parse_id(item.0);
            if seen.contains(&id) {
                report.duplicate_ids.push(id.clone());
            } else {
                seen.push(id.clone());
            }
            report.requirements.push(ParsedRequirement {
                id,
                text: item.1.trim().to_string(),
                raw: line.trim().to_string(),
            });
        }
    }

    if let Some(body) = section(md, "Tasks") {
        seen.clear();
        for line in body.lines().filter(|line| !line.trim().is_empty()) {
            let Some(item) = strip_bullet(line, false) else {
                report.unparsed_sections.push(line.trim().to_string());
                continue;
            };
            let (_, body) = strip_checkbox(item);
            let Some((id, rest)) = bold_id(body, false).or_else(|| {
                // 既定任务格式为无加粗 `- [ ] T-001: 标题`（见模块文档）。
                let (id, after) = body.split_once(':')?;
                let normalized = id.trim();
                if normalized.is_empty() {
                    None
                } else {
                    Some((normalized, after.trim_start()))
                }
            }) else {
                report.unparsed_sections.push(line.trim().to_string());
                continue;
            };
            let id = parse_id(id);
            if seen.contains(&id) {
                report.duplicate_ids.push(id.clone());
            } else {
                seen.push(id.clone());
            }
            let (refs, plan, acceptance, depends, unknown) = annotations(rest);
            report.unknown_fields.extend(unknown);
            let title = rest
                .split('(')
                .next()
                .unwrap_or(rest)
                .trim()
                .trim_end_matches(':')
                .to_string();
            report.tasks.push(ParsedTask {
                id,
                title,
                requirement_refs: refs,
                plan_refs: plan,
                acceptance_criteria: acceptance,
                depends_on: depends,
                raw: line.trim().to_string(),
            });
        }
    }

    if let Some(body) = section(md, "Clarifications") {
        seen.clear();
        for line in body.lines().filter(|line| !line.trim().is_empty()) {
            let Some(item) = strip_bullet(line, false) else {
                report.unparsed_sections.push(line.trim().to_string());
                continue;
            };
            let (resolved, body) = strip_checkbox(item);
            let Some((id, rest)) = bold_id(body, false) else {
                report.unparsed_sections.push(line.trim().to_string());
                continue;
            };
            let id = parse_id(id);
            if seen.contains(&id) {
                report.duplicate_ids.push(id.clone());
            } else {
                seen.push(id.clone());
            }
            let text = rest
                .split("[priority")
                .next()
                .unwrap_or(rest)
                .trim()
                .to_string();
            let high_priority = rest.contains("[priority: high]");
            report.clarifications.push(Clarification {
                id,
                text,
                resolved,
                high_priority,
                raw: line.trim().to_string(),
            });
        }
    }

    report
}

/// 归一化纠偏：空白折叠、`*` 列表符当 `-`、标题大小写归一。
fn normalize(md: &str) -> String {
    md.lines()
        .map(|line| {
            let trimmed = line.trim_start();
            if let Some(rest) = trimmed.strip_prefix("* ") {
                format!("- {rest}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 宽松解析：加粗容忍（`- T-001: 标题` 当 `**T-001**:` 解析）。
fn parse_loose(md: &str) -> ParseReport {
    let loose = md
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            if trimmed.starts_with("- ") && !trimmed[2..].starts_with("**") {
                let body = &trimmed[2..];
                if let Some((id, rest)) = body.split_once(':') {
                    return format!("- **{}**:{}", id.trim(), rest);
                }
            }
            line.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    parse_artifact(&normalize(&loose))
}

/// 解析并自动纠偏重试（T075、FR-309）：默认最多 2 次纠偏，仍失败返回
/// [`ParseBlocked`]（原文与错误保留，任务须标记 `blocked`）。
///
/// 纠偏策略：第 1 次重试做空白/列表符归一，第 2 次再做加粗容忍。
///
/// # Errors
///
/// 本函数以 `Result` 表达"纠偏成功/仍失败"，不返回 [`WhError`]；签名预留
/// `WhError` 以便上层统一错误信封。仍失败时返回 [`ParseBlocked`]。
pub fn parse_with_retry(md: &str, max_retries: usize) -> Result<ParseReport, ParseBlocked> {
    let accepted = |report: &ParseReport| report.is_clean() && !report.is_empty_artifact();
    let mut attempts = 1usize;
    let mut report = parse_artifact(md);
    let mut errors = report.anomaly_summary();
    if accepted(&report) {
        return Ok(report);
    }
    if report.is_empty_artifact() && errors.is_empty() {
        errors.push("产物无可解析条目（Requirements/Tasks/Clarifications 皆空）".to_string());
    }
    let candidates: [String; 2] = [normalize(md), parse_loose_input(md)];
    for candidate in candidates.iter().take(max_retries) {
        attempts += 1;
        report = if attempts == 2 {
            parse_artifact(candidate)
        } else {
            parse_loose(candidate)
        };
        errors = report.anomaly_summary();
        if accepted(&report) {
            return Ok(report);
        }
    }
    if errors.is_empty() {
        errors.push("产物无可解析条目（Requirements/Tasks/Clarifications 皆空）".to_string());
    }
    Err(ParseBlocked {
        attempts,
        errors,
        original: md.to_string(),
    })
}

/// 归一化输入（第二次纠偏的基础输入）。
fn parse_loose_input(md: &str) -> String {
    normalize(md)
}

/// 供应用服务持久化：解析器版本 + 校验结果（`parser_version` 随产物持久化）。
#[must_use]
pub fn parser_version() -> &'static str {
    PARSER_VERSION
}

/// 便捷错误：解析输入为空时拒绝（调用方校验）。
///
/// # Errors
///
/// 空输入返回 `parse_error`。
pub fn ensure_non_empty(md: &str) -> Result<(), WhError> {
    if md.trim().is_empty() {
        return Err(WhError::parse_error("SDD 产物为空：无内容可解析"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLEAN: &str = r#"# 规格说明

## Requirements
- **REQ-001**: 会话可创建
- **REQ-002**: 会话可归档

## Tasks
- [ ] T-001: 实现会话创建 (refs: REQ-001) (plan: DEC-001) (acceptance: 会话可查询; ID 唯一) (depends: T-000)
- [ ] T-002: 实现归档 (refs: REQ-002)

## Clarifications
- [ ] **CLAR-001**: 归档是否可见 [priority: high]
- [x] **CLAR-002**: 命名规则
"#;

    #[test]
    fn parses_all_three_sections_with_annotations() {
        let report = parse_artifact(CLEAN);
        assert!(report.is_clean(), "{report:?}");
        assert_eq!(report.requirements.len(), 2);
        assert_eq!(report.requirements[0].id, "REQ-001");
        assert_eq!(report.tasks.len(), 2);
        let task = &report.tasks[0];
        assert_eq!(task.id, "T-001");
        assert_eq!(task.requirement_refs, vec!["REQ-001"]);
        assert_eq!(task.plan_refs, vec!["DEC-001"]);
        assert_eq!(task.acceptance_criteria, vec!["会话可查询", "ID 唯一"]);
        assert_eq!(task.depends_on, vec!["T-000"]);
        assert_eq!(report.clarifications.len(), 2);
        assert!(!report.clarifications[0].resolved);
        assert!(report.clarifications[0].high_priority);
        assert!(report.clarifications[1].resolved);
        assert!(!report.clarifications[1].high_priority);
    }

    #[test]
    fn reports_unparsable_sections_duplicates_and_unknown_fields() {
        let dirty = "## Requirements\n- **REQ-001**: 一\n- **REQ-001**: 重复\n不是列表行\n\n## Tasks\n- [ ] T-001: 任务 (refs: REQ-001) (bogus: x)\n\n## Notes\n- **REQ-009**: 额外段落\n";
        let report = parse_artifact(dirty);
        assert!(!report.is_clean(), "异常必须被报告（FR-309）");
        assert_eq!(report.duplicate_ids, vec!["REQ-001"], "重复 ID 报告");
        assert!(
            report.unknown_fields.contains(&"bogus".to_string()),
            "未识别字段报告：{:?}",
            report.unknown_fields
        );
        assert!(
            report
                .unparsed_sections
                .iter()
                .any(|section| section.contains("不是列表行")),
            "无法解析的行保留原文：{:?}",
            report.unparsed_sections
        );
        assert!(
            report
                .unparsed_sections
                .iter()
                .any(|section| section.contains("REQ-009")),
            "未识别段落的条目保留原文不静默丢弃（FR-309）"
        );
    }

    #[test]
    fn correction_retry_normalizes_loose_input() {
        // 松散输入：`*` 列表符 + 无加粗 ID——纠偏 2 次内恢复。
        let loose =
            "## Requirements\n* REQ-001: 会话可创建\n\n## Tasks\n* T-001: 任务 (refs: REQ-001)\n";
        let report = parse_with_retry(loose, DEFAULT_CORRECTION_RETRIES).expect("纠偏成功");
        assert!(report.is_clean());
        assert_eq!(report.requirements[0].id, "REQ-001");
        assert_eq!(report.tasks[0].requirement_refs, vec!["REQ-001"]);
    }

    #[test]
    fn correction_exhaustion_blocks_with_original_preserved() {
        let hopeless = "## Requirements\n这不是能解析的段落\n没有列表也没有 ID\n";
        let blocked = parse_with_retry(hopeless, DEFAULT_CORRECTION_RETRIES)
            .expect_err("不可恢复输入必须阻塞");
        assert_eq!(blocked.attempts, 3, "首次 + 2 次纠偏（FR-309 默认）");
        assert!(!blocked.errors.is_empty(), "错误保留");
        assert!(
            blocked.original.contains("这不是能解析的段落"),
            "原文保留（R-06）"
        );
        // 零纠偏预算：首次失败即阻塞。
        let blocked = parse_with_retry(hopeless, 0).expect_err("零纠偏");
        assert_eq!(blocked.attempts, 1);
    }

    #[test]
    fn empty_input_is_rejected() {
        assert!(ensure_non_empty("").is_err());
        assert!(ensure_non_empty("  \n").is_err());
        assert!(ensure_non_empty(CLEAN).is_ok());
        assert_eq!(parser_version(), PARSER_VERSION);
    }
}
