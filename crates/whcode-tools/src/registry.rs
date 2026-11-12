//! # 工具注册表与输入校验（FR-725、FR-729、ADR-048 §2.1/§2.7）
//!
//! 注册表以稳定名持有工具描述与实现：重名/别名冲突拒绝注册（域 B：ZCode registry
//! 语义的最小子集）；参数校验一层——白名单 + 必填 + 类型 + 拒绝未知字段（FR-729；
//! ZCode 三层校验的裁剪，v1 参数空间小，见 ADR-048 §2.7）。
//!
//! 有界性：注册表容量由构造时的工具数量决定，不随会话增长（NFR-215）。

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use serde_json::Value;
use whcode_domain::WhError;

/// 参数类型（JSON Schema 子集；FR-725）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamType {
    /// 字符串。
    String,
    /// 整数。
    Integer,
    /// 布尔。
    Boolean,
    /// 字符串数组（T284／FR-771：待办清单这类“无 id 的字符串列表”参数）。
    StringArray,
}

impl ParamType {
    /// Schema 类型名（provider 清单用）。
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            ParamType::String => "string",
            ParamType::Integer => "integer",
            ParamType::Boolean => "boolean",
            ParamType::StringArray => "array",
        }
    }
}

/// 副作用范围（权限面与四端确认判定的依据；FR-623 写确认口径）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SideEffectScope {
    /// 只读：不修改工作区。
    ReadOnly,
    /// 工作区写：修改文件内容（产出 diff 摘要留痕）。
    WorkspaceWrite,
    /// 会话状态写：只改本会话的派生状态（如待办快照），不触碰工作区文件
    /// （T284／FR-771；权限按写类判定，审批默认沿用读侧口径，见 ADR-048 修订）。
    SessionState,
}

/// 单个参数声明。
#[derive(Debug, Clone)]
pub struct ToolParameter {
    /// 参数名。
    pub name: &'static str,
    /// 类型。
    pub kind: ParamType,
    /// 是否必填。
    pub required: bool,
    /// 说明（进 provider 清单）。
    pub description: &'static str,
}

/// 工具描述（稳定名 + 元数据）。
#[derive(Debug, Clone)]
pub struct ToolDescriptor {
    /// 稳定名（如 `read_file`）。
    pub name: &'static str,
    /// 面向模型的说明。
    pub description: &'static str,
    /// 参数声明（顺序即 Schema 属性顺序）。
    pub parameters: Vec<ToolParameter>,
    /// 副作用范围。
    pub side_effect_scope: SideEffectScope,
    /// 协作式执行预算（毫秒；`None` 表示不声明上限）。
    ///
    /// 声明即承诺：实现会在自有循环里检查截止时间并**主动返回** `ErrorCode::ToolTimeout`；
    /// 执行器不会强杀同进程代码（T267／FR-757）。
    pub timeout_ms: Option<u64>,
    /// 输出 schema（FR-759；`None` 表示工具只产文本，不校验结构）。
    ///
    /// 声明后：工具返回的结构化值必须通过 [`validate_output`]，否则整次调用按
    /// 结构化错误失败（不当成功、不静默丢弃字段）。
    pub output_schema: Option<Value>,
}

impl ToolDescriptor {
    /// 参数 JSON Schema 对象（FR-725：provider 可见清单的 parameters 字段）。
    #[must_use]
    pub fn parameters_schema(&self) -> Value {
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();
        for parameter in &self.parameters {
            let schema = match parameter.kind {
                ParamType::StringArray => serde_json::json!({
                    "type": "array",
                    "items": { "type": "string" },
                    "description": parameter.description,
                }),
                _ => serde_json::json!({
                    "type": parameter.kind.as_str(),
                    "description": parameter.description,
                }),
            };
            properties.insert(parameter.name.to_string(), schema);
            if parameter.required {
                required.push(Value::from(parameter.name));
            }
        }
        serde_json::json!({
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        })
    }
}

/// 声明了预算的工具的默认上限（30 s；与 MCP 默认同口径）。
pub const DEFAULT_TOOL_TIMEOUT_MS: u64 = 30_000;
/// 声明预算的允许上限（600 s；越界按上限截断，不拒绝工具调用）。
pub const MAX_TOOL_TIMEOUT_MS: u64 = 600_000;

/// 结构化错误构造（输出契约与参数校验共用）。
fn invalid(message: impl Into<String>) -> WhError {
    WhError::invalid_state(message.into())
}

/// 由声明的预算计算执行截止时间（T267／FR-757）。
///
/// 未声明（`None`）返回 `None`（无预算）；声明值先按 [`MAX_TOOL_TIMEOUT_MS`] 截断再换算——
/// 越界按上限收口而非拒绝（声明由代码给出，截断是防御性收口，不改变“声明即承诺”契约）。
#[must_use]
pub fn effective_deadline(
    timeout_ms: Option<u64>,
    now: std::time::Instant,
) -> Option<std::time::Instant> {
    timeout_ms.map(|ms| now + std::time::Duration::from_millis(ms.min(MAX_TOOL_TIMEOUT_MS)))
}

/// 工具输出的结构化值在**展示面**（事件载荷）的序列化上限（字节；超限截断并标记）。
///
/// 与 [`crate::MAX_TOOL_OUTPUT_BYTES`] 区分：后者约束回填模型的正文，本常量约束进入
/// `ToolCallFinished` 事件的结构化值——展示面不得把无界对象写进事件（NFR-215 有界）。
pub const MAX_OUTPUT_VALUE_BYTES: usize = 2 * 1024;

/// 工具输出（回填 `role:"tool"` 消息的正文；FR-728；结构化输出见 FR-759）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutput {
    /// 结果正文（已按 64 KiB 上限截断，见 [`crate::MAX_TOOL_OUTPUT_BYTES`]）。
    pub content: String,
    /// 是否发生截断（R-05 对策：截断必须显式标记）。
    pub truncated: bool,
    /// 结构化输出（FR-759：`None` 表示工具只提供文本，行为与引入契约前一致）。
    ///
    /// 有 `value` 的工具由 [`Tool::finalize`] 校验 schema、投影出模型面正文，
    /// 并把原值留给展示面（前端卡片）；`content` 在 finalize 前为空串。
    pub value: Option<Value>,
}

impl ToolOutput {
    /// 纯文本输出（既有工具形态）。
    #[must_use]
    pub fn text(content: String, truncated: bool) -> Self {
        Self {
            content,
            truncated,
            value: None,
        }
    }

    /// 结构化输出：模型面正文由 [`Tool::render`] 在 finalize 时投影。
    #[must_use]
    pub fn structured(value: Value) -> Self {
        Self {
            content: String::new(),
            truncated: false,
            value: Some(value),
        }
    }

    /// 展示面用的有界序列化（超限截断并附标记；无结构化值时 `None`）。
    #[must_use]
    pub fn display_value(&self) -> Option<String> {
        let value = self.value.as_ref()?;
        let mut serialized = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
        if serialized.len() > MAX_OUTPUT_VALUE_BYTES {
            let mut cut = MAX_OUTPUT_VALUE_BYTES;
            while cut > 0 && !serialized.is_char_boundary(cut) {
                cut -= 1;
            }
            serialized.truncate(cut);
            serialized.push_str("…[truncated]");
        }
        Some(serialized)
    }
}

/// 输出正文的统一切断（R-05；模型面与展示面共用同一上限口径）。
#[must_use]
pub fn bound_output(content: String) -> ToolOutput {
    let byte_len = content.len();
    if byte_len <= crate::MAX_TOOL_OUTPUT_BYTES {
        return ToolOutput::text(content, false);
    }
    let mut cut = crate::MAX_TOOL_OUTPUT_BYTES;
    while cut > 0 && !content.is_char_boundary(cut) {
        cut -= 1;
    }
    ToolOutput::text(
        format!(
            "{}\n[truncated: 原 {} 字节超过 {} 字节上限，R-05 对策显式截断]",
            &content[..cut],
            byte_len,
            crate::MAX_TOOL_OUTPUT_BYTES
        ),
        true,
    )
}

/// 校验结构化输出是否符合工具声明的 schema（FR-759；JSON Schema 的最小子集）。
///
/// 子集：`type`（根与属性）、`required`、`properties`、`additionalProperties: false`。
/// 判定失败返回 `invalid_state` 并把**具体违例**放进 `details.violations`——结构化错误，
/// 不在一句话里混多种原因。
///
/// # Errors
///
/// schema 本身非法（非对象/未知 `type`）或值不满足声明时返回 `invalid_state`。
pub fn validate_output(schema: &Value, value: &Value) -> Result<(), WhError> {
    let Some(object) = schema.as_object() else {
        return Err(invalid("输出 schema 必须是对象"));
    };
    let mut violations: Vec<String> = Vec::new();
    if let Some(expected) = object.get("type") {
        let Some(expected) = expected.as_str() else {
            return Err(invalid("输出 schema 的 type 必须是字符串"));
        };
        if !type_matches(expected, value) {
            violations.push(format!("根值类型应为 {expected}"));
        }
    }
    if let Some(required) = object.get("required") {
        let Some(required) = required.as_array() else {
            return Err(invalid("输出 schema 的 required 必须是数组"));
        };
        for name in required.iter().filter_map(Value::as_str) {
            if value.get(name).is_none() {
                violations.push(format!("缺少必需字段 {name}"));
            }
        }
    }
    if let Some(properties) = object.get("properties") {
        let Some(properties) = properties.as_object() else {
            return Err(invalid("输出 schema 的 properties 必须是对象"));
        };
        for (name, rule) in properties {
            let Some(actual) = value.get(name) else {
                continue;
            };
            let Some(expected) = rule.get("type").and_then(Value::as_str) else {
                continue;
            };
            if !type_matches(expected, actual) {
                violations.push(format!("字段 {name} 类型应为 {expected}"));
            }
        }
        if object.get("additionalProperties") == Some(&Value::Bool(false))
            && let Some(map) = value.as_object()
        {
            for name in map.keys() {
                if !properties.contains_key(name) {
                    violations.push(format!("未声明的字段 {name}"));
                }
            }
        }
    }
    if violations.is_empty() {
        Ok(())
    } else {
        Err(invalid("工具输出不符合声明的 schema")
            .with_details(serde_json::json!({ "violations": violations })))
    }
}

fn type_matches(expected: &str, value: &Value) -> bool {
    match expected {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        _ => false,
    }
}

/// 工具实现抽象（同步、有界、零 unsafe；进程类工具后续批次经 whcode-runtime）。
pub trait Tool: Send + Sync {
    /// 工具描述（owned：描述体量小，每次构造以避开 static/const 堆初始化限制）。
    fn descriptor(&self) -> ToolDescriptor;

    /// 执行：`workspace` 为工作区根（路径绑定的唯一锚点），`arguments` 为模型声明参数
    /// （已过 [`validate_arguments`]，实现内仍可做语义级校验）。
    ///
    /// # Errors
    ///
    /// 参数语义非法、路径越界或 IO 失败时返回归一化错误。
    fn execute(&self, workspace: &Path, arguments: &Value) -> Result<ToolOutput, WhError>;

    /// 带截止时间的执行（默认忽略截止时间，委托 [`Tool::execute`]）。
    ///
    /// 只有声明 `timeout_ms` 且自身有可中断循环的工具才覆盖本方法；覆盖者必须在循环内
    /// 周期检查 `deadline`，过期即返回 `WhError`（`ErrorCode::ToolTimeout`），
    /// 使“停止等待并标记”成为可判定事实（T267／FR-757）。
    ///
    /// # Errors
    ///
    /// 同 [`Tool::execute`]；另加截止时间到期。
    fn execute_with_deadline(
        &self,
        workspace: &Path,
        arguments: &Value,
        deadline: Option<std::time::Instant>,
    ) -> Result<ToolOutput, WhError> {
        let _ = deadline;
        self.execute(workspace, arguments)
    }

    /// 模型面投影（FR-759）：把结构化输出渲染为回填 `role:"tool"` 的正文。
    ///
    /// 默认把值序列化为紧凑 JSON；需要更省 token 的呈现时覆盖本方法（展示面不受影响，
    /// 结构原值仍由 [`ToolOutput::display_value`] 提供给前端）。
    ///
    /// # Errors
    ///
    /// 值无法序列化时返回 `invalid_state`。
    fn render(&self, _arguments: &Value, value: &Value) -> Result<String, WhError> {
        serde_json::to_string(value).map_err(|err| invalid(format!("渲染工具输出失败：{err}")))
    }

    /// 输出收口（FR-759）：schema 校验 → 模型面投影 → 统一截断。
    ///
    /// 仅对返回了结构化值（[`ToolOutput::value`] 为 `Some`）的工具生效；纯文本工具原样
    /// 通过（行为与引入契约前一致）。校验失败返回结构化错误（`details.violations`），
    /// 由执行器登记为失败并回填模型——不当作成功，也不静默裁剪字段。
    ///
    /// # Errors
    ///
    /// 值不满足声明的 schema，或渲染失败时返回 `invalid_state`。
    fn finalize(&self, arguments: &Value, output: ToolOutput) -> Result<ToolOutput, WhError> {
        let descriptor = self.descriptor();
        let Some(value) = output.value else {
            return Ok(output);
        };
        if let Some(schema) = descriptor.output_schema.as_ref() {
            validate_output(schema, &value)?;
        }
        let rendered = self.render(arguments, &value)?;
        let bounded = bound_output(rendered);
        Ok(ToolOutput {
            value: Some(value),
            ..bounded
        })
    }
}

/// 工具注册表：稳定名 → 实现（重名拒绝；BTreeMap 保证清单输出稳定有序）。
#[derive(Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    /// 空注册表。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册工具；稳定名冲突拒绝（域 B：alias 冲突保 canonical 的最小说法）。
    ///
    /// # Errors
    ///
    /// 重名时返回 [`ErrorCode::InvalidState`]。
    pub fn register(&mut self, tool: Arc<dyn Tool>) -> Result<(), WhError> {
        let name = tool.descriptor().name;
        if self.tools.contains_key(name) {
            return Err(WhError::invalid_state(format!("工具重名：{name}")));
        }
        self.tools.insert(name.to_string(), tool);
        Ok(())
    }

    /// 按稳定名取工具。
    #[must_use]
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    /// 已注册工具数（有界性断言与测试用）。
    #[must_use]
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// 是否为空。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// 全部工具描述（按稳定名升序；工具目录生成与自查用，P7.4／FR-768）。
    #[must_use]
    pub fn descriptors(&self) -> Vec<ToolDescriptor> {
        self.tools.values().map(|tool| tool.descriptor()).collect()
    }

    /// provider 侧工具清单（`tools` 数组元素：type=function + function 元数据，FR-725；
    /// 与 sensenova 实测口径一致，见 `docs/guides/test-model-sensenova.md` §6.1）。
    #[must_use]
    pub fn provider_manifest(&self) -> Vec<Value> {
        self.tools
            .values()
            .map(|tool| {
                let descriptor = tool.descriptor();
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": descriptor.name,
                        "description": descriptor.description,
                        "parameters": descriptor.parameters_schema(),
                    },
                })
            })
            .collect()
    }
}

/// 参数校验一层（FR-729）：对象形态、白名单内、必填齐备、类型匹配、拒绝未知字段。
///
/// # Errors
///
/// 任一判据不满足返回 [`ErrorCode::InvalidInput`] 语义的 [`WhError`]。
pub fn validate_arguments(descriptor: &ToolDescriptor, arguments: &Value) -> Result<(), WhError> {
    let Some(object) = arguments.as_object() else {
        return Err(WhError::invalid_state("工具参数必须是 JSON 对象"));
    };
    let declared: Vec<&ToolParameter> = descriptor.parameters.iter().collect();
    for parameter in &declared {
        match object.get(parameter.name) {
            None => {
                if parameter.required {
                    return Err(WhError::invalid_state(format!(
                        "缺少必填参数 {}：{}",
                        parameter.name, descriptor.name
                    )));
                }
            }
            Some(value) => {
                let ok = match parameter.kind {
                    ParamType::String => value.is_string(),
                    ParamType::Integer => value.is_i64() || value.is_u64(),
                    ParamType::Boolean => value.is_boolean(),
                    ParamType::StringArray => value
                        .as_array()
                        .is_some_and(|items| items.iter().all(Value::is_string)),
                };
                if !ok {
                    return Err(WhError::invalid_state(format!(
                        "参数 {} 类型应为 {}",
                        parameter.name,
                        parameter.kind.as_str()
                    )));
                }
            }
        }
    }
    for key in object.keys() {
        if !declared.iter().any(|parameter| parameter.name == key) {
            return Err(WhError::invalid_state(format!(
                "未知参数 {key}：{} 拒绝未声明字段（FR-729）",
                descriptor.name
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use whcode_domain::ErrorCode;

    struct NoopTool {
        descriptor: ToolDescriptor,
    }

    impl Tool for NoopTool {
        fn descriptor(&self) -> ToolDescriptor {
            self.descriptor.clone()
        }

        fn execute(&self, _workspace: &Path, _arguments: &Value) -> Result<ToolOutput, WhError> {
            Ok(ToolOutput::text("ok".to_string(), false))
        }
    }

    /// 结构化输出工具（T269）：按测试需要返回任意值，用于三路径契约测试。
    struct StructuredTool {
        descriptor: ToolDescriptor,
        value: Value,
    }

    impl Tool for StructuredTool {
        fn descriptor(&self) -> ToolDescriptor {
            self.descriptor.clone()
        }

        fn execute(&self, _workspace: &Path, _arguments: &Value) -> Result<ToolOutput, WhError> {
            Ok(ToolOutput::structured(self.value.clone()))
        }

        fn render(&self, _arguments: &Value, value: &Value) -> Result<String, WhError> {
            // 模型面投影与展示面分离：这里只渲染 summary 字段（省 token），
            // 展示面仍保留完整结构值。
            Ok(value
                .get("summary")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string())
        }
    }

    fn structured_tool(schema: Value, value: Value) -> StructuredTool {
        StructuredTool {
            descriptor: ToolDescriptor {
                name: "structured",
                description: "结构化输出测试工具",
                parameters: vec![],
                side_effect_scope: SideEffectScope::ReadOnly,
                timeout_ms: None,
                output_schema: Some(schema),
            },
            value,
        }
    }

    fn noop(name: &'static str) -> Arc<dyn Tool> {
        Arc::new(NoopTool {
            descriptor: ToolDescriptor {
                name,
                description: "测试工具",
                parameters: vec![ToolParameter {
                    name: "path",
                    kind: ParamType::String,
                    required: true,
                    description: "路径",
                }],
                side_effect_scope: SideEffectScope::ReadOnly,
                timeout_ms: None,
                output_schema: None,
            },
        })
    }

    #[test]
    fn effective_deadline_is_absent_without_declaration_and_capped() {
        let now = std::time::Instant::now();
        assert!(
            effective_deadline(None, now).is_none(),
            "未声明预算＝无截止时间（执行器不得自行设限）"
        );
        let normal =
            effective_deadline(Some(DEFAULT_TOOL_TIMEOUT_MS), now).expect("声明后必有截止时间");
        assert!(normal > now, "正常路径：截止时间在未来");
        let capped = effective_deadline(Some(u64::MAX), now).expect("越界声明仍收口为有截止时间");
        assert!(
            capped <= now + std::time::Duration::from_millis(MAX_TOOL_TIMEOUT_MS),
            "上限路径：越界声明按 MAX_TOOL_TIMEOUT_MS 截断"
        );
    }

    #[test]
    fn register_rejects_duplicate_name() {
        let mut registry = ToolRegistry::new();
        registry.register(noop("read_file")).expect("首次注册");
        let err = registry
            .register(noop("read_file"))
            .expect_err("重名必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn provider_manifest_is_ordered_and_schema_shaped() {
        let mut registry = ToolRegistry::new();
        registry.register(noop("zeta")).expect("注册 zeta");
        registry.register(noop("alpha")).expect("注册 alpha");
        let manifest = registry.provider_manifest();
        assert_eq!(manifest.len(), 2);
        assert_eq!(manifest[0]["function"]["name"], "alpha", "BTreeMap 有序");
        assert_eq!(manifest[0]["type"], "function");
        let parameters = &manifest[0]["function"]["parameters"];
        assert_eq!(parameters["required"][0], "path");
        assert_eq!(parameters["additionalProperties"], false);
    }

    #[test]
    fn validate_rejects_missing_wrong_type_and_unknown() {
        let descriptor = noop("read_file").descriptor();
        let err = validate_arguments(&descriptor, &json!({})).expect_err("缺必填必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);

        let err =
            validate_arguments(&descriptor, &json!({ "path": 1 })).expect_err("类型不符必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);

        let err = validate_arguments(&descriptor, &json!({ "path": "a", "extra": 1 }))
            .expect_err("未知字段必须拒绝（FR-729）");
        assert!(
            err.message.contains("未知参数"),
            "拒绝信息须指名未知字段：{err}"
        );

        validate_arguments(&descriptor, &json!({ "path": "a" })).expect("合法参数通过");
    }

    fn summary_schema() -> Value {
        json!({
            "type": "object",
            "required": ["summary"],
            "properties": { "summary": { "type": "string" } },
            "additionalProperties": false,
        })
    }

    #[test]
    fn structured_output_is_validated_and_rendered_separately_from_display_value() {
        let tool = structured_tool(summary_schema(), json!({ "summary": "完成 3 个文件" }));
        let finalized = tool
            .finalize(&json!({}), ToolOutput::structured(tool.value.clone()))
            .expect("合法输出通过");
        assert_eq!(
            finalized.content, "完成 3 个文件",
            "模型面走 render 投影（不是原始 JSON）"
        );
        assert!(!finalized.truncated);
        assert_eq!(
            finalized.value,
            Some(json!({ "summary": "完成 3 个文件" })),
            "展示面保留结构值"
        );
        let display = finalized.display_value().expect("有结构值");
        assert!(
            display.contains("\"summary\""),
            "展示面序列化保留字段名：{display}"
        );
    }

    #[test]
    fn invalid_structured_output_is_a_structured_error_with_violations() {
        let tool = structured_tool(summary_schema(), json!({}));
        let err = tool
            .finalize(&json!({}), ToolOutput::structured(json!({})))
            .expect_err("缺必需字段必须失败");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(!err.retryable, "契约违约不可重试");
        let violations = err
            .details
            .as_ref()
            .and_then(|details| details.get("violations"))
            .and_then(Value::as_array)
            .expect("结构化错误携带 violations");
        assert!(
            violations
                .iter()
                .any(|item| item.as_str().unwrap_or("").contains("summary")),
            "违例必须指名字段：{violations:?}"
        );

        // 类型不符与未声明字段同样拒绝（additionalProperties: false）。
        let err = tool
            .finalize(&json!({}), ToolOutput::structured(json!({ "summary": 1 })))
            .expect_err("类型不符必须失败");
        assert_eq!(err.code, ErrorCode::InvalidState);
        let err = tool
            .finalize(
                &json!({}),
                ToolOutput::structured(json!({ "summary": "ok", "extra": true })),
            )
            .expect_err("未声明字段必须失败");
        assert_eq!(err.code, ErrorCode::InvalidState);
    }

    #[test]
    fn oversized_structured_output_is_truncated_with_marker_on_both_faces() {
        let long = "x".repeat(crate::MAX_TOOL_OUTPUT_BYTES + 512);
        let tool = structured_tool(summary_schema(), json!({ "summary": long.clone() }));
        let finalized = tool
            .finalize(
                &json!({}),
                ToolOutput::structured(json!({ "summary": long })),
            )
            .expect("超限不失败，按统一上限截断");
        assert!(finalized.truncated, "截断必须显式标记");
        assert!(
            finalized.content.len() <= crate::MAX_TOOL_OUTPUT_BYTES + 128,
            "模型面正文受 64 KiB 上限约束：{}",
            finalized.content.len()
        );
        assert!(finalized.content.contains("[truncated:"), "截断标记可查");
        let display = finalized.display_value().expect("有结构值");
        assert!(
            display.len() <= MAX_OUTPUT_VALUE_BYTES + 32,
            "展示面受 2 KiB 上限约束：{}",
            display.len()
        );
        assert!(display.ends_with("…[truncated]"), "展示面截断可查");
    }

    #[test]
    fn text_only_tools_pass_through_finalize_unchanged() {
        // 回归：无结构值（既有六件套形态）时 finalize 不得改写正文或截断标记。
        let tool = noop("read_file");
        let output = ToolOutput::text("原始正文".to_string(), true);
        let finalized = tool
            .finalize(&json!({}), output)
            .expect("纯文本必须原样通过");
        assert_eq!(finalized.content, "原始正文");
        assert!(finalized.truncated, "既有截断标记保留");
        assert_eq!(finalized.value, None);
        assert_eq!(finalized.display_value(), None, "无结构值即无展示面负载");
    }
}
