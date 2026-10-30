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
}

impl ParamType {
    /// Schema 类型名（provider 清单用）。
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            ParamType::String => "string",
            ParamType::Integer => "integer",
            ParamType::Boolean => "boolean",
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
}

impl ToolDescriptor {
    /// 参数 JSON Schema 对象（FR-725：provider 可见清单的 parameters 字段）。
    #[must_use]
    pub fn parameters_schema(&self) -> Value {
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();
        for parameter in &self.parameters {
            properties.insert(
                parameter.name.to_string(),
                serde_json::json!({
                    "type": parameter.kind.as_str(),
                    "description": parameter.description,
                }),
            );
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

/// 工具输出（回填 `role:"tool"` 消息的正文；FR-728）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutput {
    /// 结果正文（已按 64 KiB 上限截断，见 [`crate::MAX_TOOL_OUTPUT_BYTES`]）。
    pub content: String,
    /// 是否发生截断（R-05 对策：截断必须显式标记）。
    pub truncated: bool,
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
            Ok(ToolOutput {
                content: "ok".to_string(),
                truncated: false,
            })
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
            },
        })
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
}
