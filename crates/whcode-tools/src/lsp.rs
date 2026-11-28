//! # LSP 四查询工具（P5.3b／FR-783／ADR-078）
//!
//! 工具本体与执行器抽象在本 crate；真正的 LSP 通信由装配根注入
//! [`LspQueryExecutor`] 实现（本 crate 不依赖 `whcode-lsp`，分层不变——与 shell 工具同
//! 范式，ADR-070）。
//!
//! 封闭集四个工具：`lsp_definition`／`lsp_references`／`lsp_hover`／`lsp_diagnostics`。
//! 它们都是**只读**（`SideEffectScope::ReadOnly`）：模型只能指定文件与位置，**不能指定要
//! 启动哪个服务器或参数**——命令来自用户配置文件（`.whcode/lsp.json`），这是与 shell 工具
//! 的本质区别（shell 的脚本文本由模型给出，故必须审批）。
//!
//! 位置参数对外是 **1 基**（`line`／`character` 从 1 开始，与编辑器、grep 输出一致），
//! 传入执行器前转成 LSP 的 **0 基**；`0` 视为参数错误——避免“差一行”的静默错误。
//!
//! 有界性：结果条数上限由执行器保证（`whcode-lsp` 侧 ≤100 条），文本再由
//! [`crate::bound_output`] 截断；任一截断都显式标注，不冒充完整结果。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use serde_json::Value;
use whcode_domain::{ErrorCode, WhError};

use crate::paths::resolve_in_workspace;
use crate::registry::{
    ParamType, SideEffectScope, Tool, ToolDescriptor, ToolOutput, ToolParameter, bound_output,
    validate_arguments,
};

/// 定义查询工具名。
pub const LSP_DEFINITION_TOOL: &str = "lsp_definition";
/// 引用查询工具名。
pub const LSP_REFERENCES_TOOL: &str = "lsp_references";
/// 悬停查询工具名。
pub const LSP_HOVER_TOOL: &str = "lsp_hover";
/// 诊断查询工具名。
pub const LSP_DIAGNOSTICS_TOOL: &str = "lsp_diagnostics";

/// 工具级超时（毫秒）：包含进程启动、握手与一次查询；超过即报 `tool_timeout`。
pub const LSP_TOOL_TIMEOUT_MS: u64 = 30_000;

/// 封闭集查询类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LspQueryKind {
    /// 符号定义。
    Definition,
    /// 符号引用。
    References,
    /// 悬停说明。
    Hover,
    /// 文件诊断。
    Diagnostics,
}

impl LspQueryKind {
    fn tool_name(self) -> &'static str {
        match self {
            Self::Definition => LSP_DEFINITION_TOOL,
            Self::References => LSP_REFERENCES_TOOL,
            Self::Hover => LSP_HOVER_TOOL,
            Self::Diagnostics => LSP_DIAGNOSTICS_TOOL,
        }
    }

    /// 是否需要位置（诊断只需文件）。
    fn needs_position(self) -> bool {
        !matches!(self, Self::Diagnostics)
    }
}

/// 查询请求（由工具构造，执行器实现消费）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspQueryRequest {
    /// 查询类别。
    pub kind: LspQueryKind,
    /// 目标文件（已解析为工作区内绝对路径）。
    pub path: PathBuf,
    /// 行（**0 基**，已由工具从 1 基换算）。
    pub line: u32,
    /// 列（**0 基**）。
    pub character: u32,
    /// 引用查询是否包含声明处（其余类别忽略）。
    pub include_declaration: bool,
}

/// 一处位置结果（0 基坐标）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocationView {
    /// 目标文件路径（执行器把 `file://` URI 还原成平台路径）。
    pub path: String,
    /// 起始行（0 基）。
    pub line: u32,
    /// 起始列（0 基）。
    pub character: u32,
    /// 结束行。
    pub end_line: u32,
    /// 结束列。
    pub end_character: u32,
}

/// 一条诊断（0 基坐标）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticView {
    /// 行（0 基）。
    pub line: u32,
    /// 列（0 基）。
    pub character: u32,
    /// 严重级别（1=error，2=warning，3=info，4=hint）。
    pub severity: Option<u8>,
    /// 来源（如 `rust-analyzer`）。
    pub source: Option<String>,
    /// 诊断码。
    pub code: Option<String>,
    /// 消息。
    pub message: String,
}

/// 查询结果（封闭集三形态）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LspQueryResult {
    /// 位置列表（definition／references）。
    Locations(Vec<LocationView>),
    /// 悬停文本（`None`＝服务器明确无悬停信息）。
    Hover(Option<String>),
    /// 诊断列表。
    Diagnostics(Vec<DiagnosticView>),
}

/// 查询结果＋截断标记。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspQueryOutcome {
    /// 结果本体。
    pub result: LspQueryResult,
    /// 是否因上限被截断（条数或文本）。
    pub truncated: bool,
}

/// LSP 查询执行器（装配根实现；本 crate 只定义契约）。
pub trait LspQueryExecutor: Send + Sync {
    /// 执行一次查询。
    ///
    /// # Errors
    ///
    /// 服务器未配置该扩展名、服务器未声明能力、超时、进程退出、协议错误时返回结构化错误
    /// （区分 `not_supported`／`tool_timeout`／`io_error`／`parse_error`）。
    fn query(&self, request: &LspQueryRequest) -> Result<LspQueryOutcome, WhError>;
}

/// 一个 LSP 查询工具（按类别分派）。
pub struct LspTool {
    kind: LspQueryKind,
    executor: Arc<dyn LspQueryExecutor>,
}

impl LspTool {
    /// 由执行器构造（`whcode-app` 经 `registry_with_shell_and_lsp` 注册）。
    #[must_use]
    pub fn new(kind: LspQueryKind, executor: Arc<dyn LspQueryExecutor>) -> Self {
        Self { kind, executor }
    }

    fn run(
        &self,
        workspace: &std::path::Path,
        arguments: &Value,
        deadline: Option<Instant>,
    ) -> Result<ToolOutput, WhError> {
        let descriptor = Tool::descriptor(self);
        validate_arguments(&descriptor, arguments)?;
        if deadline
            .is_some_and(|deadline| deadline.saturating_duration_since(Instant::now()).is_zero())
        {
            return Err(WhError::new(
                ErrorCode::ToolTimeout,
                "LSP 查询截止时间已过（未发起查询）".to_string(),
            ));
        }
        let raw_path = arguments
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| WhError::invalid_state("参数 path 必须为字符串".to_string()))?;
        let path = resolve_in_workspace(workspace, raw_path)?;
        let (line, character) = if self.kind.needs_position() {
            (
                one_based(arguments, "line")?,
                one_based(arguments, "character")?,
            )
        } else {
            (0, 0)
        };
        let include_declaration = arguments
            .get("include_declaration")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let request = LspQueryRequest {
            kind: self.kind,
            path,
            line,
            character,
            include_declaration,
        };
        let outcome = self.executor.query(&request)?;
        Ok(self.render(&request, outcome))
    }

    fn render(&self, request: &LspQueryRequest, outcome: LspQueryOutcome) -> ToolOutput {
        let (body, empty_note) = match outcome.result {
            LspQueryResult::Locations(items) => {
                if items.is_empty() {
                    (
                        String::new(),
                        Some(match self.kind {
                            LspQueryKind::Definition => {
                                "未找到定义（服务器返回 0 处）：符号可能是宏展开、外部 crate 或索引未就绪"
                            }
                            _ => "未找到引用（服务器返回 0 处）：可能确实无人引用，或索引未就绪",
                        }),
                    )
                } else {
                    let mut lines = Vec::with_capacity(items.len());
                    for item in &items {
                        lines.push(format!(
                            "{}:{}:{}",
                            item.path,
                            item.line + 1,
                            item.character + 1
                        ));
                    }
                    (lines.join("\n"), None)
                }
            }
            LspQueryResult::Hover(Some(text)) => (text, None),
            LspQueryResult::Hover(None) => (
                String::new(),
                Some("该位置没有悬停信息（服务器返回空结果）"),
            ),
            LspQueryResult::Diagnostics(items) => {
                if items.is_empty() {
                    (String::new(), Some("该文件当前没有诊断（服务器返回 0 条）"))
                } else {
                    let mut lines = Vec::with_capacity(items.len());
                    for item in &items {
                        let severity = severity_label(item.severity);
                        let source = item.source.as_deref().unwrap_or("?");
                        let code = item
                            .code
                            .as_deref()
                            .map_or_else(String::new, |code| format!(" [{code}]"));
                        lines.push(format!(
                            "{}:{}:{severity} {source}{code} {}",
                            request.path.display(),
                            item.line + 1,
                            item.message
                        ));
                    }
                    (lines.join("\n"), None)
                }
            }
        };
        let mut content = match empty_note {
            Some(note) => note.to_string(),
            None => body,
        };
        if outcome.truncated {
            content.push_str("\n（结果已截断：达到上限，仅返回前若干条）");
        }
        let bounded = bound_output(content);
        ToolOutput::text(bounded.content, outcome.truncated || bounded.truncated)
    }
}

fn one_based(arguments: &Value, name: &str) -> Result<u32, WhError> {
    let raw = arguments
        .get(name)
        .and_then(Value::as_u64)
        .ok_or_else(|| WhError::invalid_state(format!("参数 {name} 必须为正整数（1 基）")))?;
    if raw == 0 {
        return Err(WhError::invalid_state(format!(
            "参数 {name} 从 1 开始计数（收到 0）：请使用编辑器/grep 报出的行号"
        )));
    }
    let value = u32::try_from(raw)
        .map_err(|_| WhError::invalid_state(format!("参数 {name} 超出 u32 范围：{raw}")))?;
    // 1 基 → 0 基；`checked_sub` 已在 `raw == 0` 分支上挡住。
    Ok(value - 1)
}

fn severity_label(severity: Option<u8>) -> &'static str {
    match severity {
        Some(1) => "error",
        Some(2) => "warning",
        Some(3) => "info",
        Some(4) => "hint",
        _ => "unknown",
    }
}

impl Tool for LspTool {
    fn descriptor(&self) -> ToolDescriptor {
        let position_params = |description_line: &'static str,
                               description_character: &'static str| {
            vec![
                ToolParameter {
                    name: "path",
                    kind: ParamType::String,
                    required: true,
                    description: "工作区内的文件路径（相对工作区根）",
                },
                ToolParameter {
                    name: "line",
                    kind: ParamType::Integer,
                    required: true,
                    description: description_line,
                },
                ToolParameter {
                    name: "character",
                    kind: ParamType::Integer,
                    required: true,
                    description: description_character,
                },
            ]
        };
        let parameters = match self.kind {
            LspQueryKind::Diagnostics => vec![ToolParameter {
                name: "path",
                kind: ParamType::String,
                required: true,
                description: "工作区内的文件路径（相对工作区根）",
            }],
            LspQueryKind::References => {
                let mut params =
                    position_params("行号（1 基，与编辑器一致）", "列号（1 基，按字符计）");
                params.push(ToolParameter {
                    name: "include_declaration",
                    kind: ParamType::Boolean,
                    required: false,
                    description: "是否包含声明处；缺省 true",
                });
                params
            }
            _ => position_params("行号（1 基，与编辑器一致）", "列号（1 基，按字符计）"),
        };
        let (description, timeout_ms) = match self.kind {
            LspQueryKind::Definition => (
                "经语言服务器查询符号定义位置（LSP textDocument/definition）；只读，\
                 服务器与参数来自用户配置（.whcode/lsp.json），模型不能指定",
                Some(LSP_TOOL_TIMEOUT_MS),
            ),
            LspQueryKind::References => (
                "经语言服务器查询符号引用位置（LSP textDocument/references）；只读，\
                 可指定是否包含声明处",
                Some(LSP_TOOL_TIMEOUT_MS),
            ),
            LspQueryKind::Hover => (
                "经语言服务器查询符号的悬停说明（LSP textDocument/hover）；只读，\
                 返回纯文本（类型签名、文档注释等）",
                Some(LSP_TOOL_TIMEOUT_MS),
            ),
            LspQueryKind::Diagnostics => (
                "经语言服务器查询文件诊断（LSP 拉取，回退推送缓存）；只读，\
                 返回 error/warning/info/hint 与消息",
                Some(LSP_TOOL_TIMEOUT_MS),
            ),
        };
        ToolDescriptor {
            name: self.kind.tool_name(),
            description,
            parameters,
            side_effect_scope: SideEffectScope::ReadOnly,
            timeout_ms,
            output_schema: None,
        }
    }

    fn execute(
        &self,
        workspace: &std::path::Path,
        arguments: &Value,
    ) -> Result<ToolOutput, WhError> {
        self.run(workspace, arguments, None)
    }

    fn execute_with_deadline(
        &self,
        workspace: &std::path::Path,
        arguments: &Value,
        deadline: Option<Instant>,
    ) -> Result<ToolOutput, WhError> {
        self.run(workspace, arguments, deadline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use serde_json::json;

    fn workspace() -> PathBuf {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("whcode-lsp-tool-{}-{seq}", std::process::id()));
        // Code Scanning 收口：路径字符串不含 ".." 才使用（防临时目录遍历注入）。
        let dir_checked = dir.display().to_string();
        if dir_checked.contains("..") {
            panic!("临时目录路径不得包含 ..");
        }
        let dir = std::path::PathBuf::from(dir_checked);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).expect("建夹具");
        std::fs::write(dir.join("src/lib.rs"), "fn helper() {}\n").expect("写夹具");
        dir
    }

    /// 记录请求的桩执行器；按类别返回预置结果。
    struct MockExecutor {
        requests: Mutex<Vec<LspQueryRequest>>,
        outcome: LspQueryOutcome,
    }

    impl MockExecutor {
        fn new(outcome: LspQueryOutcome) -> Self {
            Self {
                requests: Mutex::new(Vec::new()),
                outcome,
            }
        }
    }

    impl LspQueryExecutor for MockExecutor {
        fn query(&self, request: &LspQueryRequest) -> Result<LspQueryOutcome, WhError> {
            self.requests.lock().expect("锁").push(request.clone());
            Ok(self.outcome.clone())
        }
    }

    fn locations(items: Vec<LocationView>, truncated: bool) -> LspQueryOutcome {
        LspQueryOutcome {
            result: LspQueryResult::Locations(items),
            truncated,
        }
    }

    /// 1 基入参转 0 基请求，路径解析到工作区；引用默认包含声明。
    #[test]
    fn position_is_converted_to_zero_based_and_path_resolved() {
        let ws = workspace();
        let mock = Arc::new(MockExecutor::new(locations(
            vec![LocationView {
                path: "src/lib.rs".to_string(),
                line: 4,
                character: 2,
                end_line: 4,
                end_character: 6,
            }],
            false,
        )));
        let executor: Arc<dyn LspQueryExecutor> = mock.clone();
        let tool = LspTool::new(LspQueryKind::Definition, executor);
        let output = tool
            .execute(
                &ws,
                &json!({"path": "src/lib.rs", "line": 10, "character": 5}),
            )
            .expect("执行");
        assert!(
            output.content.contains("src/lib.rs:5:3"),
            "{}",
            output.content
        );
        assert!(!output.truncated);

        let requests = mock.requests.lock().expect("锁");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].line, 9, "1 基 10 行 → 0 基 9");
        assert_eq!(requests[0].character, 4);
        assert_eq!(requests[0].kind, LspQueryKind::Definition);
        assert!(requests[0].include_declaration, "引用默认包含声明");
        assert!(requests[0].path.ends_with("src/lib.rs"));
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// 位置参数从 1 开始：`0` 必须显式拒绝（不静默当第 0 行）。
    #[test]
    fn zero_based_line_is_rejected() {
        let ws = workspace();
        let executor = Arc::new(MockExecutor::new(locations(Vec::new(), false)));
        let tool = LspTool::new(LspQueryKind::Hover, executor);
        let err = tool
            .execute(
                &ws,
                &json!({"path": "src/lib.rs", "line": 0, "character": 1}),
            )
            .expect_err("行 0 必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(err.message.contains("从 1 开始"), "{}", err.message);
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// 空结果是正确的空：给出可操作说明，且不置截断。
    #[test]
    fn empty_results_are_explained_not_silent() {
        let ws = workspace();
        let definition = LspTool::new(
            LspQueryKind::Definition,
            Arc::new(MockExecutor::new(locations(Vec::new(), false))),
        );
        let output = definition
            .execute(
                &ws,
                &json!({"path": "src/lib.rs", "line": 1, "character": 1}),
            )
            .expect("执行");
        assert!(output.content.contains("未找到定义"), "{}", output.content);
        assert!(!output.truncated);

        let diagnostics = LspTool::new(
            LspQueryKind::Diagnostics,
            Arc::new(MockExecutor::new(LspQueryOutcome {
                result: LspQueryResult::Diagnostics(Vec::new()),
                truncated: false,
            })),
        );
        let output = diagnostics
            .execute(&ws, &json!({"path": "src/lib.rs"}))
            .expect("执行");
        assert!(output.content.contains("没有诊断"), "{}", output.content);
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// 截断标记必须出现在正文里（不只信 `truncated` 字段）。
    #[test]
    fn truncation_is_stated_in_text() {
        let ws = workspace();
        let tool = LspTool::new(
            LspQueryKind::References,
            Arc::new(MockExecutor::new(locations(
                vec![LocationView {
                    path: "src/lib.rs".to_string(),
                    line: 0,
                    character: 0,
                    end_line: 0,
                    end_character: 1,
                }],
                true,
            ))),
        );
        let output = tool
            .execute(
                &ws,
                &json!({"path": "src/lib.rs", "line": 1, "character": 1}),
            )
            .expect("执行");
        assert!(output.truncated);
        assert!(output.content.contains("结果已截断"), "{}", output.content);
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// 诊断渲染含严重级别、来源与诊断码；悬停原样返回。
    #[test]
    fn diagnostics_and_hover_render() {
        let ws = workspace();
        let diagnostics = LspTool::new(
            LspQueryKind::Diagnostics,
            Arc::new(MockExecutor::new(LspQueryOutcome {
                result: LspQueryResult::Diagnostics(vec![DiagnosticView {
                    line: 2,
                    character: 0,
                    severity: Some(2),
                    source: Some("rust-analyzer".to_string()),
                    code: Some("unused_variables".to_string()),
                    message: "unused variable".to_string(),
                }]),
                truncated: false,
            })),
        );
        let output = diagnostics
            .execute(&ws, &json!({"path": "src/lib.rs"}))
            .expect("执行");
        assert!(
            !output.content.contains("error") && output.content.contains("warning"),
            "{}",
            output.content
        );
        assert!(
            output.content.contains("rust-analyzer"),
            "{}",
            output.content
        );
        assert!(
            output.content.contains("[unused_variables]"),
            "{}",
            output.content
        );
        assert!(
            output.content.contains(":3:"),
            "1 基行号：{}",
            output.content
        );

        let hover = LspTool::new(
            LspQueryKind::Hover,
            Arc::new(MockExecutor::new(LspQueryOutcome {
                result: LspQueryResult::Hover(Some("fn helper() -> ()".to_string())),
                truncated: false,
            })),
        );
        let output = hover
            .execute(
                &ws,
                &json!({"path": "src/lib.rs", "line": 1, "character": 4}),
            )
            .expect("执行");
        assert_eq!(output.content, "fn helper() -> ()");
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// 描述符：只读副作用、四个稳定名、诊断无位置参数。
    #[test]
    fn descriptors_declare_read_only_and_closed_set() {
        let executor: Arc<dyn LspQueryExecutor> =
            Arc::new(MockExecutor::new(locations(Vec::new(), false)));
        let names: Vec<&str> = [
            LspQueryKind::Definition,
            LspQueryKind::References,
            LspQueryKind::Hover,
            LspQueryKind::Diagnostics,
        ]
        .into_iter()
        .map(|kind| Tool::descriptor(&LspTool::new(kind, Arc::clone(&executor))).name)
        .collect();
        assert_eq!(
            names,
            vec![
                LSP_DEFINITION_TOOL,
                LSP_REFERENCES_TOOL,
                LSP_HOVER_TOOL,
                LSP_DIAGNOSTICS_TOOL
            ]
        );
        for kind in [LspQueryKind::Definition, LspQueryKind::Diagnostics] {
            let descriptor = Tool::descriptor(&LspTool::new(kind, Arc::clone(&executor)));
            assert_eq!(descriptor.side_effect_scope, SideEffectScope::ReadOnly);
            assert_eq!(descriptor.timeout_ms, Some(LSP_TOOL_TIMEOUT_MS));
            let has_line = descriptor
                .parameters
                .iter()
                .any(|parameter| parameter.name == "line");
            assert_eq!(
                has_line,
                kind.needs_position(),
                "只有位置类查询声明 line 参数"
            );
        }
    }
}
