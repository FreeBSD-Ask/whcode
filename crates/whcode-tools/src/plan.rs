//! # 计划模式工具（T287／FR-774／ADR-067）
//!
//! `plan_mode` 让模型声明“当前在讨论计划还是执行”：`{active: true}` 进入计划模式，
//! `{active: false}` 退出。工具本身**不改状态**（纯函数、无副作用）：状态由 turn 在调用
//! 成功后落 `PlanModeChanged` 事件（`source = model`），并在此前的审批面上对**退出**
//! 施加人工复核（伪工具名 `plan_mode_exit`）。
//!
//! ## 边界
//!
//! - **软引导**：进入计划模式只影响下一 turn 注入的引导段，**不阻止任何工具调用**；
//!   真正的限制由审批与沙箱独立执行（ADR-067 决策 2，文档与引导段都须明写）。
//! - **退出需人复核**：本工具不接受任何“无法复核”的退出路径；非交互装配下退出直接失败
//!   （fail-closed），由用户经 `session plan --off` 或 Web `POST /api/plan` 关闭。
//! - **参数最小**：只有一个必填布尔 `active`；未知字段拒绝（与其他工具同一校验口径）。

use serde_json::{Value, json};
use whcode_domain::{ErrorCode, WhError};

use crate::registry::{
    ParamType, SideEffectScope, Tool, ToolDescriptor, ToolOutput, ToolParameter, validate_arguments,
};

/// 工具稳定名（与 `whcode-app::turn` 状态落点共用）。
pub const PLAN_MODE_TOOL: &str = "plan_mode";

/// 退出复核使用的伪工具名（审批规则键；不对应任何真实工具，ADR-067 决策 3）。
pub const PLAN_MODE_EXIT_RULE: &str = "plan_mode_exit";

/// 计划引导段（注入到激活状态的 turn；常量、有界、明写“不强制”）。
pub const PLAN_GUIDANCE: &str = "当前会话处于**计划模式**（用户可见、可随时关闭）。\
本模式下请先产出计划（目标、步骤、涉及文件、验证方式），在用户认可前不要执行写操作；\
需要动手时先请用户退出计划模式。注意：本段只是引导，系统并不阻止任何工具调用——\
真正的限制由审批与沙箱独立执行。";

/// 计划引导段字符上限（常量文本的边界断言，防止后续编辑无界膨胀）。
pub const MAX_PLAN_GUIDANCE_CHARS: usize = 400;

/// 计划模式工具。
pub struct PlanMode;

/// 解析并校验 `active` 参数（turn 的审批前置判定与事件落点共用同一口径）。
///
/// # Errors
///
/// 参数非对象、含未知字段、`active` 缺失或非布尔时返回 `invalid_state`。
pub fn plan_active_from_arguments(arguments: &Value) -> Result<bool, WhError> {
    validate_arguments(&PlanMode.descriptor(), arguments)?;
    arguments
        .get("active")
        .and_then(Value::as_bool)
        .ok_or_else(|| WhError::new(ErrorCode::InvalidState, "计划模式参数 active 必须是布尔值"))
}

impl Tool for PlanMode {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: PLAN_MODE_TOOL,
            description: "进入或退出计划模式（软引导：只影响引导段，不阻止工具调用；\
                 退出需要用户在审批面确认）。",
            parameters: vec![ToolParameter {
                name: "active",
                kind: ParamType::Boolean,
                required: true,
                description: "true＝进入计划模式；false＝退出（需人工复核）",
            }],
            side_effect_scope: SideEffectScope::SessionState,
            timeout_ms: None,
            output_schema: Some(json!({
                "type": "object",
                "required": ["active"],
                "properties": { "active": { "type": "boolean" } },
                "additionalProperties": false,
            })),
        }
    }

    fn execute(
        &self,
        _workspace: &std::path::Path,
        arguments: &Value,
    ) -> Result<ToolOutput, WhError> {
        let active = plan_active_from_arguments(arguments)?;
        Ok(ToolOutput {
            content: String::new(),
            truncated: false,
            value: Some(json!({ "active": active })),
        })
    }

    /// 模型面投影：状态与“不强制”口径都要说清，避免模型把计划模式当只读锁。
    fn render(&self, _arguments: &Value, value: &Value) -> Result<String, WhError> {
        let active = value
            .get("active")
            .and_then(Value::as_bool)
            .ok_or_else(|| WhError::new(ErrorCode::InvalidState, "计划模式输出缺少 active"))?;
        Ok(if active {
            "已进入计划模式（软引导：不会阻止工具调用；限制由审批与沙箱执行）".to_string()
        } else {
            "已退出计划模式".to_string()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(arguments: Value) -> Result<ToolOutput, WhError> {
        let tool = PlanMode;
        let output = tool.execute(std::path::Path::new("."), &arguments)?;
        tool.finalize(&arguments, output)
    }

    #[test]
    fn toggles_both_directions_with_model_facing_projection() {
        let on = run(json!({ "active": true })).expect("进入");
        assert_eq!(
            on.value.as_ref().map(|v| v["active"].clone()),
            Some(json!(true))
        );
        assert!(on.content.contains("已进入计划模式"), "{}", on.content);
        assert!(
            on.content.contains("不会阻止工具调用"),
            "投影必须明写不强制：{}",
            on.content
        );

        let off = run(json!({ "active": false })).expect("退出");
        assert_eq!(
            off.value.as_ref().map(|v| v["active"].clone()),
            Some(json!(false))
        );
        assert!(off.content.contains("已退出计划模式"), "{}", off.content);
    }

    #[test]
    fn rejects_missing_mistyped_and_unknown_arguments() {
        for (arguments, tag) in [
            (json!({}), "缺 active"),
            (json!({ "active": "yes" }), "active 非布尔"),
            (json!({ "active": true, "note": "x" }), "未知字段"),
            (json!(["active"]), "非对象"),
        ] {
            let err = run(arguments).expect_err(tag);
            assert_eq!(err.code, ErrorCode::InvalidState, "{tag}");
        }
    }

    #[test]
    fn guidance_text_stays_within_its_declared_bound() {
        assert!(
            PLAN_GUIDANCE.chars().count() <= MAX_PLAN_GUIDANCE_CHARS,
            "引导段超出声明上限"
        );
        assert!(PLAN_GUIDANCE.contains("不阻止"), "引导段必须写明不强制");
    }
}
