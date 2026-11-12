//! # 交付物登记工具（T285／FR-772／ADR-048 §7.2）
//!
//! `present_deliverables` 把工作区内**已存在**的文件登记为本会话的交付物：调用方给出
//! 相对路径列表（每次至多 [`MAX_DELIVERABLES_PER_CALL`] 个），工具解析并校验后返回规范化
//! 清单；`whcode-app` 在调用成功后落 `DeliverablePresented` 事件（log-only）。
//!
//! ## 边界
//!
//! - **必须已存在且是文件**：登记不存在的路径等于声明未发生的事实，一律拒绝；
//!   目录同样拒绝（交付物是可直接打开的文件，不是目录树）。
//! - **工作区内**：沿用 [`resolve_in_workspace`] 的绝对路径与 `..` 拒绝口径。
//! - **不改文件**：本工具只读元数据、不写工作区、不改工具结果；登记是产品语义层
//!   （与 DSH `present` 同定位）。
//! - **幂等语义在读取面**：同一路径重复登记不报错，读取面按首次登记顺序去重
//!   （见 `whcode_app::deliverables`）。

use std::path::Path;

use serde_json::{Value, json};
use whcode_domain::WhError;

use crate::paths::resolve_in_workspace;
use crate::registry::{
    ParamType, SideEffectScope, Tool, ToolDescriptor, ToolOutput, ToolParameter, validate_arguments,
};

/// 工具稳定名（与 `whcode-app::turn` 事件落点共用）。
pub const PRESENT_DELIVERABLES_TOOL: &str = "present_deliverables";

/// 单次调用最多登记的交付物个数（对齐 DSH `present` 的 4 个上限）。
pub const MAX_DELIVERABLES_PER_CALL: usize = 4;

/// 单个路径的字符上限。
pub const MAX_DELIVERABLE_PATH_CHARS: usize = 400;

/// 交付物登记工具。
pub struct PresentDeliverables;

fn invalid(message: impl Into<String>) -> WhError {
    WhError::new(whcode_domain::ErrorCode::InvalidState, message.into())
}

/// 规范化并在工作区内校验路径列表（工具与事件落点共用同一口径）。
///
/// 返回去首尾空白后的相对路径（保持调用方顺序）；重复路径被**忽略**而不是拒绝——
/// 模型重复登记同一文件不构成事实错误，读取面按去重呈现。
///
/// # Errors
///
/// 参数非对象／含未知字段／`paths` 缺失或非字符串数组、路径为空、超长、绝对或含 `..`、
/// 超过单次上限、路径不存在或不是文件时返回 `invalid_state`。
pub fn entries_from_arguments(workspace: &Path, arguments: &Value) -> Result<Vec<String>, WhError> {
    let descriptor = PresentDeliverables.descriptor();
    validate_arguments(&descriptor, arguments)?;
    let Some(raw_paths) = arguments.get("paths").and_then(Value::as_array) else {
        return Err(invalid("交付物参数 paths 必须是字符串数组"));
    };
    if raw_paths.is_empty() {
        return Err(invalid("交付物 paths 不得为空；没有交付物就不要调用本工具"));
    }
    let mut normalized: Vec<String> = Vec::with_capacity(raw_paths.len());
    for item in raw_paths {
        let Some(raw) = item.as_str() else {
            return Err(invalid("交付物 paths 的条目必须是字符串"));
        };
        let trimmed = raw.trim();
        if trimmed.chars().count() > MAX_DELIVERABLE_PATH_CHARS {
            return Err(invalid(format!(
                "交付物路径超过 {MAX_DELIVERABLE_PATH_CHARS} 字符：{}",
                trimmed.chars().take(64).collect::<String>()
            )));
        }
        let resolved = resolve_in_workspace(workspace, trimmed)?;
        let metadata = resolved
            .metadata()
            .map_err(|err| invalid(format!("交付物路径无法访问：{trimmed}（{err}）")))?;
        if !metadata.is_file() {
            return Err(invalid(format!(
                "交付物必须是文件：{trimmed} 不是常规文件（目录请逐个列出文件）"
            )));
        }
        if !normalized.iter().any(|existing| existing == trimmed) {
            normalized.push(trimmed.to_string());
        }
    }
    if normalized.len() > MAX_DELIVERABLES_PER_CALL {
        return Err(invalid(format!(
            "单次最多登记 {MAX_DELIVERABLES_PER_CALL} 个交付物，实际 {}；请分多次调用",
            normalized.len()
        )));
    }
    Ok(normalized)
}

impl Tool for PresentDeliverables {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: PRESENT_DELIVERABLES_TOOL,
            description: "把工作区内已存在的文件登记为本会话交付物（每次至多 4 个，须为常规文件）。\
                 登记只记录事实、不改文件；交付物清单可在 Web 对话页查看。",
            parameters: vec![ToolParameter {
                name: "paths",
                kind: ParamType::StringArray,
                required: true,
                description: "工作区内相对路径（字符串数组，1—4 个，必须已存在且是文件）",
            }],
            side_effect_scope: SideEffectScope::SessionState,
            timeout_ms: None,
            output_schema: Some(json!({
                "type": "object",
                "required": ["paths", "count"],
                "properties": {
                    "paths": { "type": "array" },
                    "count": { "type": "integer" },
                },
                "additionalProperties": false,
            })),
        }
    }

    fn execute(&self, workspace: &Path, arguments: &Value) -> Result<ToolOutput, WhError> {
        let paths = entries_from_arguments(workspace, arguments)?;
        Ok(ToolOutput {
            content: String::new(),
            truncated: false,
            value: Some(json!({
                "paths": paths,
                "count": paths.len(),
            })),
        })
    }

    /// 模型面投影：只给计数与路径清单（路径本身就是模型需要核对的事实）。
    fn render(&self, _arguments: &Value, value: &Value) -> Result<String, WhError> {
        let count = value.get("count").and_then(Value::as_u64).unwrap_or(0);
        let paths = value
            .get("paths")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("、")
            })
            .unwrap_or_default();
        Ok(format!("已登记 {count} 个交付物：{paths}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use whcode_domain::ErrorCode;

    struct Sandbox {
        root: std::path::PathBuf,
    }

    impl Sandbox {
        fn new(tag: &str) -> Self {
            static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let root = std::env::temp_dir()
                .join(format!("whcode-present-{}-{tag}-{seq}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(root.join("out")).expect("工作区");
            fs::write(root.join("out/report.md"), "报告\n").expect("文件");
            fs::write(root.join("out/data.json"), "{}\n").expect("文件");
            Self { root }
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn run(workspace: &Path, arguments: Value) -> Result<ToolOutput, WhError> {
        let tool = PresentDeliverables;
        let output = tool.execute(workspace, &arguments)?;
        tool.finalize(&arguments, output)
    }

    #[test]
    fn registers_existing_files_in_order_and_dedups_repeats() {
        let sandbox = Sandbox::new("ok");
        let output = run(
            &sandbox.root,
            json!({ "paths": ["out/report.md", "out/data.json", "out/report.md"] }),
        )
        .expect("登记成功");
        let value = output.value.as_ref().expect("结构化输出");
        assert_eq!(value["count"], 2, "重复路径去重");
        assert_eq!(
            value["paths"],
            json!(["out/report.md", "out/data.json"]),
            "保持调用方顺序"
        );
        assert!(
            output.content.contains("已登记 2 个交付物"),
            "模型面投影：{}",
            output.content
        );
    }

    #[test]
    fn rejects_missing_absolute_escaping_directory_and_oversized_calls() {
        let sandbox = Sandbox::new("bad");
        let err = run(&sandbox.root, json!({ "paths": ["out/missing.md"] })).expect_err("不存在");
        assert_eq!(err.code, ErrorCode::InvalidState);
        let err = run(&sandbox.root, json!({ "paths": ["/etc/passwd"] })).expect_err("绝对路径");
        assert_eq!(err.code, ErrorCode::InvalidState);
        let err = run(&sandbox.root, json!({ "paths": ["../escape.md"] })).expect_err("逃逸");
        assert_eq!(err.code, ErrorCode::InvalidState);
        let err = run(&sandbox.root, json!({ "paths": ["out"] })).expect_err("目录");
        assert_eq!(err.code, ErrorCode::InvalidState);
        let err = run(&sandbox.root, json!({ "paths": [] })).expect_err("空数组");
        assert_eq!(err.code, ErrorCode::InvalidState);
        let err = run(&sandbox.root, json!({})).expect_err("缺参数");
        assert_eq!(err.code, ErrorCode::InvalidState);
        let err = run(
            &sandbox.root,
            json!({ "paths": ["out/report.md"], "note": "x" }),
        )
        .expect_err("未知参数");
        assert_eq!(err.code, ErrorCode::InvalidState);

        // 五个不同文件：超过单次上限必须拒绝（先造出文件）。
        for index in 0..MAX_DELIVERABLES_PER_CALL + 1 {
            fs::write(sandbox.root.join(format!("out/f{index}.md")), "x\n").expect("文件");
        }
        let paths: Vec<String> = (0..=MAX_DELIVERABLES_PER_CALL)
            .map(|index| format!("out/f{index}.md"))
            .collect();
        let err = run(&sandbox.root, json!({ "paths": paths })).expect_err("超过单次上限");
        assert_eq!(err.code, ErrorCode::InvalidState);
    }

    #[test]
    fn boundary_of_four_deliverables_is_accepted() {
        let sandbox = Sandbox::new("four");
        for index in 0..MAX_DELIVERABLES_PER_CALL {
            fs::write(sandbox.root.join(format!("out/b{index}.md")), "x\n").expect("文件");
        }
        let paths: Vec<String> = (0..MAX_DELIVERABLES_PER_CALL)
            .map(|index| format!("out/b{index}.md"))
            .collect();
        let output = run(&sandbox.root, json!({ "paths": paths })).expect("4 个上限本身可用");
        assert_eq!(
            output.value.as_ref().map(|v| v["count"].clone()),
            Some(json!(4))
        );
    }
}
