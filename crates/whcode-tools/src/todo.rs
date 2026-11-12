//! # 会话待办工具（T284／FR-771／ADR-048 修订）
//!
//! `todo_write` 以**整表替换**语义维护本会话的轻量待办清单：调用方给出三个状态列表，
//! 工具把它们规范化为 `items`（内容 + 三态），返回结构化值并把短摘要投影给模型。
//! 条目刻意没有 id 与优先级——与 DSH 的 `todo/write` 同口径（域 G §2.3）：清单是
//! **快照**，不是可增量操作的实体。
//!
//! ## 与 SDD 任务账本的隔离（硬声明）
//!
//! 本工具的清单**不是** `tasks` 表：不产生任务、不进入 SDD 阶段门禁、不满足验收检查点、
//! 不参与 `session_overview` 聚合。工具名与说明都写明这一点，避免模型把待办当成
//! 可交付任务提交。待办快照由 `whcode-app` 落为 `TodoUpdated` 事件（log-only）。
//!
//! ## 有界性
//!
//! 条目总数 ≤ [`MAX_TODO_ITEMS`]、单条内容 ≤ [`MAX_TODO_CONTENT_CHARS`] 字符；超限返回
//! `invalid_state` 而**不截断**——待办是模型声明的意图，静默裁剪会让清单与意图不一致。
//! 空内容（含纯空白）、一次调用既给状态又给未知字段同样拒绝。工具本身无 IO、无线程。

use std::path::Path;

use serde_json::{Value, json};
use whcode_domain::WhError;

use crate::registry::{
    ParamType, SideEffectScope, Tool, ToolDescriptor, ToolOutput, ToolParameter, validate_arguments,
};

/// 工具稳定名（与 `whcode-app::turn` 的事件落点共用）。
pub const TODO_WRITE_TOOL: &str = "todo_write";

/// 单次整表替换允许的条目总数上限。
pub const MAX_TODO_ITEMS: usize = 50;

/// 单条待办内容的字符上限（按 `chars()` 计，不按字节）。
pub const MAX_TODO_CONTENT_CHARS: usize = 200;

/// 三态的稳定序列化名（事件载荷与展示面共用）。
pub const TODO_STATUS_PENDING: &str = "pending";
/// 进行中。
pub const TODO_STATUS_IN_PROGRESS: &str = "in_progress";
/// 已完成。
pub const TODO_STATUS_COMPLETED: &str = "completed";

/// 会话待办工具。
pub struct TodoWrite;

fn invalid(message: impl Into<String>) -> WhError {
    WhError::new(whcode_domain::ErrorCode::InvalidState, message.into())
}

/// 规范化一个状态列表：去首尾空白、拒绝空项与超长项，返回 `(content, status)` 序列。
fn normalize(
    arguments: &Value,
    parameter: &str,
    status: &'static str,
) -> Result<Vec<(String, &'static str)>, WhError> {
    let Some(items) = arguments.get(parameter) else {
        return Ok(Vec::new());
    };
    let Some(items) = items.as_array() else {
        return Err(invalid(format!("待办参数 {parameter} 必须是字符串数组")));
    };
    let mut normalized = Vec::with_capacity(items.len());
    for item in items {
        let Some(raw) = item.as_str() else {
            return Err(invalid(format!("待办参数 {parameter} 的条目必须是字符串")));
        };
        let content = raw.trim();
        if content.is_empty() {
            return Err(invalid(format!(
                "待办参数 {parameter} 含空条目；空待办请省略该参数而不是给空串"
            )));
        }
        let chars = content.chars().count();
        if chars > MAX_TODO_CONTENT_CHARS {
            return Err(invalid(format!(
                "待办条目 {chars} 字符超过上限 {MAX_TODO_CONTENT_CHARS}；请缩短后重试"
            )));
        }
        normalized.push((content.to_string(), status));
    }
    Ok(normalized)
}

/// 把工具参数规范化为待办快照（`items` + 三态计数）。
///
/// **唯一口径**：工具执行与 `whcode-app::turn` 的 `TodoUpdated` 落点都调用本函数，
/// 避免“模型看到的清单”与“事件里的清单”两套标准化逻辑分叉。校验失败时返回
/// `invalid_state`（调用方据此不落事件）。
///
/// # Errors
///
/// 参数非对象／含未知字段／条目非字符串、空串、超长，或条目总数超限。
pub fn snapshot_from_arguments(arguments: &Value) -> Result<Value, WhError> {
    let descriptor = TodoWrite.descriptor();
    validate_arguments(&descriptor, arguments)?;
    let pending = normalize(arguments, "pending", TODO_STATUS_PENDING)?;
    let in_progress = normalize(arguments, "in_progress", TODO_STATUS_IN_PROGRESS)?;
    let completed = normalize(arguments, "completed", TODO_STATUS_COMPLETED)?;
    let total = pending.len() + in_progress.len() + completed.len();
    if total > MAX_TODO_ITEMS {
        return Err(invalid(format!(
            "待办共 {total} 条，超过上限 {MAX_TODO_ITEMS}；请精简后重试"
        )));
    }
    let items: Vec<Value> = pending
        .iter()
        .chain(in_progress.iter())
        .chain(completed.iter())
        .map(|(content, status)| json!({ "content": content, "status": status }))
        .collect();
    Ok(json!({
        "items": items,
        "pending": pending.len(),
        "in_progress": in_progress.len(),
        "completed": completed.len(),
    }))
}

impl Tool for TodoWrite {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: TODO_WRITE_TOOL,
            description: "整表替换本会话的轻量待办清单（三态 pending/in_progress/completed）。\
                 清单是快照：未给出的状态列表视为空，重复调用覆盖旧清单。它不是 SDD 任务账本——\
                 不创建任务、不满足阶段门禁与验收检查点；需要可交付任务请走任务命令。",
            parameters: vec![
                ToolParameter {
                    name: "pending",
                    kind: ParamType::StringArray,
                    required: false,
                    description: "待办条目（字符串数组；省略＝空）",
                },
                ToolParameter {
                    name: "in_progress",
                    kind: ParamType::StringArray,
                    required: false,
                    description: "进行中条目（字符串数组；省略＝空）",
                },
                ToolParameter {
                    name: "completed",
                    kind: ParamType::StringArray,
                    required: false,
                    description: "已完成条目（字符串数组；省略＝空）",
                },
            ],
            side_effect_scope: SideEffectScope::SessionState,
            timeout_ms: None,
            output_schema: Some(json!({
                "type": "object",
                "required": ["items", "pending", "in_progress", "completed"],
                "properties": {
                    "items": { "type": "array" },
                    "pending": { "type": "integer" },
                    "in_progress": { "type": "integer" },
                    "completed": { "type": "integer" },
                },
                "additionalProperties": false,
            })),
        }
    }

    fn execute(&self, _workspace: &Path, arguments: &Value) -> Result<ToolOutput, WhError> {
        Ok(ToolOutput {
            content: String::new(),
            truncated: false,
            value: Some(snapshot_from_arguments(arguments)?),
        })
    }

    /// 模型面投影：只给计数摘要（逐条内容已在事件与展示面，重复回填白占上下文）。
    fn render(&self, _arguments: &Value, value: &Value) -> Result<String, WhError> {
        let count = |field: &str| value.get(field).and_then(Value::as_u64).unwrap_or(0);
        Ok(format!(
            "待办已整表替换：共 {} 条（待办 {}／进行中 {}／已完成 {}）",
            count("pending") + count("in_progress") + count("completed"),
            count("pending"),
            count("in_progress"),
            count("completed"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use whcode_domain::ErrorCode;

    fn run(arguments: Value) -> Result<ToolOutput, WhError> {
        let tool = TodoWrite;
        let output = tool.execute(Path::new("."), &arguments)?;
        tool.finalize(&arguments, output)
    }

    fn state(output: &ToolOutput) -> &Value {
        output.value.as_ref().expect("结构化输出")
    }

    #[test]
    fn whole_table_replace_names_items_and_counts() {
        let output = run(json!({
            "pending": ["写 ADR", "补测试"],
            "in_progress": ["实现工具"],
            "completed": ["读提案"],
        }))
        .expect("整表替换");
        let value = state(&output);
        assert_eq!(value["pending"], 2);
        assert_eq!(value["in_progress"], 1);
        assert_eq!(value["completed"], 1);
        let items = value["items"].as_array().expect("条目数组");
        assert_eq!(items.len(), 4, "三态合计");
        assert_eq!(items[0]["status"], "pending");
        assert_eq!(items[0]["content"], "写 ADR");
        assert_eq!(items[3]["status"], "completed");
        assert_eq!(items[3]["content"], "读提案");
        assert!(
            output.content.contains("共 4 条"),
            "模型面投影给计数摘要：{}",
            output.content
        );
        assert!(!output.content.contains("写 ADR"), "摘要不重复逐条内容");
    }

    #[test]
    fn omitted_lists_mean_empty_and_clear_the_board() {
        let output = run(json!({})).expect("空参数＝清空");
        let value = state(&output);
        assert_eq!(value["items"].as_array().map(Vec::len), Some(0));
        assert_eq!(value["pending"], 0);
        assert!(output.content.contains("共 0 条"));
    }

    #[test]
    fn rejects_empty_long_oversized_and_mistyped_entries() {
        let err = run(json!({ "pending": ["  "] })).expect_err("空条目");
        assert_eq!(err.code, ErrorCode::InvalidState);
        let long = "长".repeat(MAX_TODO_CONTENT_CHARS + 1);
        let err = run(json!({ "completed": [long] })).expect_err("超长条目");
        assert_eq!(err.code, ErrorCode::InvalidState);
        let many: Vec<String> = (0..=MAX_TODO_ITEMS).map(|i| format!("项{i}")).collect();
        let err = run(json!({ "pending": many })).expect_err("超条目上限");
        assert_eq!(err.code, ErrorCode::InvalidState);
        let err = run(json!({ "pending": [1, 2] })).expect_err("非字符串条目");
        assert_eq!(err.code, ErrorCode::InvalidState);
        let err = run(json!({ "pending": "不是数组" })).expect_err("非数组参数");
        assert_eq!(err.code, ErrorCode::InvalidState);
        let err = run(json!({ "unknown": ["x"] })).expect_err("未知参数");
        assert_eq!(err.code, ErrorCode::InvalidState);
    }

    #[test]
    fn boundary_values_are_accepted_and_content_is_trimmed() {
        let exact = "边".repeat(MAX_TODO_CONTENT_CHARS);
        let output = run(json!({ "in_progress": [format!("  {exact}  ")] })).expect("上限本身可用");
        let items = state(&output)["items"].as_array().expect("条目");
        assert_eq!(
            items[0]["content"].as_str().map(|s| s.chars().count()),
            Some(MAX_TODO_CONTENT_CHARS),
            "首尾空白被裁剪后长度等于上限"
        );
        let full: Vec<String> = (0..MAX_TODO_ITEMS).map(|i| format!("项{i}")).collect();
        run(json!({ "pending": full })).expect("条目数上限本身可用");
    }
}
