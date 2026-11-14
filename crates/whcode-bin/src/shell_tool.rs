//! # shell 工具装配（T290／FR-777／ADR-070）
//!
//! 配置＝`<workspace>/.zcode/shell.json`（显式 opt-in、无自动发现，与
//! `.zcode/mcp.json` 同目录同模式）；执行器＝[`GitBashProvider::execute_in`]
//! 适配 `whcode_tools::ShellExecutor`（FR-706 检查点四要素随结果返回）。
//!
//! 启用三条件（缺一不注册，FR-704 口径）：①配置文件存在（本模块判定）；
//! ②bash 可用探测通过（本模块判定）；③装配存在审批闸门（`whcode-app::turn`
//! 装配处判定——非交互装配模型面不出现该工具）。配置写错（坏 JSON／越界）
//! **显式失败**，不静默降级为未启用。
//!
//! 白名单语义见 ADR-070 决策 4：只约束"持久允许"的免审批通道（单命令且首词
//! 精确命中），不提供"阻止执行"的承诺。

use std::path::Path;
use std::sync::Arc;

use serde::Deserialize;
use whcode_domain::WhError;
use whcode_runtime::provider::{RuntimeProvider, Script};
use whcode_runtime::providers::GitBashProvider;
use whcode_tools::{ShellExecutor, ShellGovernance, ShellOutcome, ShellRequest};

/// 配置文件路径（相对工作区根）。
pub const CONFIG_PATH: &str = ".zcode/shell.json";

/// 配置文件字节上限（与 `.zcode/mcp.json` 的有界口径一致；超限显式拒绝）。
pub const MAX_CONFIG_BYTES: usize = 16 * 1024;

/// 单条白名单条目的字节上限（首词是单一名词，不是脚本）。
pub const MAX_ALLOWED_COMMAND_BYTES: usize = 64;

/// 配置文件形态（反序列化载体）。
#[derive(Debug, Deserialize)]
struct ShellConfigFile {
    /// 免审批白名单（首词精确匹配；缺省空＝一切脚本每次询问）。
    #[serde(default)]
    allowed_commands: Vec<String>,
}

/// 已校验的 shell 工具配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellConfig {
    /// 免审批白名单（已过条数与条目合法性校验）。
    pub allowed_commands: Vec<String>,
}

impl ShellConfig {
    /// 读取并校验配置；文件不存在返回 `Ok(None)`（未 opt-in，FR-704 默认关闭）。
    ///
    /// # Errors
    ///
    /// 文件超限、JSON 非法、白名单条数超上限或条目含空白／控制结构时返回
    /// 结构化错误——显式 opt-in 写错必须显式失败。
    pub fn load(workspace: &Path) -> Result<Option<Self>, WhError> {
        let path = workspace.join(CONFIG_PATH);
        let Ok(bytes) = std::fs::read(&path) else {
            return Ok(None);
        };
        let display = path.display();
        if bytes.len() > MAX_CONFIG_BYTES {
            return Err(WhError::invalid_state(format!(
                "shell 配置 {} 字节超上限 {MAX_CONFIG_BYTES}：{display}",
                bytes.len()
            )));
        }
        let parsed: ShellConfigFile = serde_json::from_slice(&bytes).map_err(|err| {
            WhError::parse_error(format!("shell 配置解析失败（{display}）：{err}"))
        })?;
        if parsed.allowed_commands.len() > whcode_tools::MAX_ALLOWED_COMMANDS {
            return Err(WhError::invalid_state(format!(
                "白名单条目超过 {} 上限：{}",
                whcode_tools::MAX_ALLOWED_COMMANDS,
                parsed.allowed_commands.len()
            )));
        }
        for entry in &parsed.allowed_commands {
            if entry.is_empty() || entry.len() > MAX_ALLOWED_COMMAND_BYTES {
                return Err(WhError::invalid_state(format!(
                    "白名单条目长度非法（1..={MAX_ALLOWED_COMMAND_BYTES} 字节）：{entry}"
                )));
            }
            if entry.chars().any(|c| {
                c.is_whitespace()
                    || matches!(
                        c,
                        ';' | '|' | '&' | '$' | '`' | '>' | '<' | '(' | ')' | '\n' | '\r'
                    )
            }) {
                return Err(WhError::invalid_state(format!(
                    "白名单条目必须是单一名词（不含空白与控制结构）：{entry}"
                )));
            }
        }
        Ok(Some(Self {
            allowed_commands: parsed.allowed_commands,
        }))
    }
}

/// [`GitBashProvider`] → [`ShellExecutor`] 适配器：执行走 `execute_in`
/// （cwd 固定工作区根，`workdir_policy = "workspace"`），检查点要素随结果透传。
#[derive(Debug, Default, Clone)]
pub struct GitBashShellExecutor {
    provider: GitBashProvider,
}

impl ShellExecutor for GitBashShellExecutor {
    fn execute(&self, request: ShellRequest) -> Result<ShellOutcome, WhError> {
        let script = Script {
            script: request.script,
            input: String::new(),
            timeout: request.timeout,
        };
        let checkpoint = self.provider.execute_in(&script, &request.workdir)?;
        let result = checkpoint.result.ok_or_else(|| {
            WhError::new(
                whcode_domain::ErrorCode::IoError,
                "gitbash 执行未返回结果（worker 层失败）".to_string(),
            )
        })?;
        Ok(ShellOutcome {
            exit_code: result.exit_code,
            stdout_tail: result.stdout_tail,
            stderr_tail: result.stderr_tail,
            stdout_truncated: result.stdout_truncated,
            stderr_truncated: result.stderr_truncated,
            timed_out: result.timed_out,
            script_hash: checkpoint.script_hash,
            runtime_fingerprint: checkpoint.runtime_fingerprint,
            workdir_policy: checkpoint.workdir_policy,
        })
    }
}

/// 构造治理束（装配根调用）：配置 opt-in ＋ bash 可用探测，双条件缺一返回
/// `Ok(None)`（FR-704：默认不注册）。
///
/// # Errors
///
/// 配置文件存在但非法（坏 JSON／越界条目）时返回错误——不静默降级。
pub fn build_governance(workspace: &Path) -> Result<Option<Arc<ShellGovernance>>, WhError> {
    build_governance_with(workspace, &GitBashProvider::default())
}

/// [`build_governance`] 的可注入变体（测试用：可传入指定 `git_path` 的 provider）。
///
/// # Errors
///
/// 同 [`build_governance`]。
pub fn build_governance_with(
    workspace: &Path,
    provider: &GitBashProvider,
) -> Result<Option<Arc<ShellGovernance>>, WhError> {
    let config = ShellConfig::load(workspace)?;
    Ok(governance_from(config, provider.is_available(), provider.clone()).map(Arc::new))
}

/// opt-in＋探测的**双条件装配判定**（纯逻辑，ADR-070 决策 3）：配置缺失或 bash
/// 不可用皆返回 `None`（FR-704 默认不注册）。抽成纯函数是因为“探测失败”不可在
/// 任意机器上确定性构造——`locate_bash` 对失效的 `git_path` 会回退到已知安装目录
/// （GitHub runner 实测），前提依赖机器状态的测试是伪测试。
fn governance_from(
    config: Option<ShellConfig>,
    available: bool,
    provider: GitBashProvider,
) -> Option<ShellGovernance> {
    let config = config?;
    if !available {
        return None;
    }
    Some(ShellGovernance {
        executor: Arc::new(GitBashShellExecutor { provider }),
        allowed_commands: config.allowed_commands,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use whcode_domain::ErrorCode;

    fn workspace(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "whcode-shell-tool-{tag}-{}-{}",
            std::process::id(),
            std::time::Instant::now().elapsed().as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".zcode")).expect("建夹具");
        dir
    }

    /// 文件不存在＝未 opt-in：返回 `Ok(None)`（FR-704 默认关闭）。
    #[test]
    fn absent_config_means_not_opted_in() {
        let ws = workspace("absent");
        assert_eq!(ShellConfig::load(&ws).expect("读取"), None);
        assert!(build_governance(&ws).expect("构造").is_none());
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// 合法配置解析出白名单条目。
    #[test]
    fn valid_config_loads_allowlist() {
        let ws = workspace("valid");
        std::fs::write(
            ws.join(CONFIG_PATH),
            r#"{"allowed_commands": ["git", "cargo", "python"]}"#,
        )
        .expect("写配置");
        let config = ShellConfig::load(&ws).expect("读取").expect("已 opt-in");
        assert_eq!(config.allowed_commands, vec!["git", "cargo", "python"]);
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// 坏 JSON／超限条目／含空白或控制结构的条目：显式失败（不静默降级）。
    #[test]
    fn invalid_config_fails_loudly() {
        let ws_bad = workspace("bad-json");
        std::fs::write(ws_bad.join(CONFIG_PATH), b"{ not json }").expect("写坏配置");
        let err = ShellConfig::load(&ws_bad).expect_err("坏 JSON 必须失败");
        assert_eq!(err.code, ErrorCode::ParseError);

        let ws_many = workspace("too-many");
        let entries: Vec<String> = (0..=whcode_tools::MAX_ALLOWED_COMMANDS)
            .map(|i| format!("cmd{i}"))
            .collect();
        std::fs::write(
            ws_many.join(CONFIG_PATH),
            format!(
                r#"{{"allowed_commands": {}}}"#,
                serde_json::to_string(&entries).unwrap()
            ),
        )
        .expect("写配置");
        let err = ShellConfig::load(&ws_many).expect_err("超限条数必须失败");
        assert_eq!(err.code, ErrorCode::InvalidState);

        let ws_compound = workspace("compound");
        std::fs::write(
            ws_compound.join(CONFIG_PATH),
            r#"{"allowed_commands": ["git status"]}"#,
        )
        .expect("写配置");
        let err = ShellConfig::load(&ws_compound).expect_err("含空白条目必须失败");
        assert_eq!(err.code, ErrorCode::InvalidState);

        for ws in [&ws_bad, &ws_many, &ws_compound] {
            let _ = std::fs::remove_dir_all(ws);
        }
    }

    /// 双条件装配判定的确定性矩阵（FR-704／ADR-070 决策 3）：配置缺失或 bash
    /// 不可用皆不注册；两条件齐备才构造治理束。经纯函数 `governance_from` 测试
    /// ——"探测失败"依赖机器状态（`locate_bash` 会回退到已知安装目录），不可
    /// 在任意机器上确定性构造，runner 实测（run 43553353128）伪前提即红。
    #[test]
    fn governance_dual_condition_matrix() {
        let config = ShellConfig {
            allowed_commands: vec!["git".to_string()],
        };
        // 配置缺失：无论探测结果如何都不注册。
        assert!(governance_from(None, true, GitBashProvider::default()).is_none());
        assert!(governance_from(None, false, GitBashProvider::default()).is_none());
        // bash 不可用：不注册。
        assert!(governance_from(Some(config.clone()), false, GitBashProvider::default()).is_none());
        // 双条件齐备：构造治理束且白名单就位（不触发子进程探测）。
        let governance = governance_from(Some(config), true, GitBashProvider::default())
            .expect("双条件齐备必须就绪");
        assert_eq!(governance.allowed_commands, vec!["git".to_string()]);
    }

    /// 配置存在＋bash 可用（git_path 缺省，按前提不满足显式跳过）：
    /// 治理束就绪且适配器经 `execute_in` 在工作区执行（cwd 固定生效）。
    #[test]
    fn governance_executes_in_workspace_when_bash_available() {
        let ws = workspace("exec");
        std::fs::write(ws.join(CONFIG_PATH), r#"{"allowed_commands": ["echo"]}"#).expect("写配置");
        let provider = GitBashProvider::default();
        if !provider.is_available() {
            eprintln!("本机未定位到 Git Bash：适配器执行用例按前提不满足显式跳过（非静默）");
            let _ = std::fs::remove_dir_all(&ws);
            return;
        }
        let governance = build_governance_with(&ws, &provider)
            .expect("构造")
            .expect("双条件满足必须就绪");
        assert_eq!(governance.allowed_commands, vec!["echo"]);
        let outcome = governance
            .executor
            .execute(ShellRequest {
                script: "echo WHCODE_SHELL_OK > marker.txt && cat marker.txt".to_string(),
                workdir: ws.clone(),
                timeout: std::time::Duration::from_secs(15),
                max_output_bytes: whcode_tools::MAX_TOOL_OUTPUT_BYTES,
            })
            .expect("执行成功");
        assert_eq!(outcome.exit_code, Some(0));
        assert!(
            outcome.stdout_tail.contains("WHCODE_SHELL_OK"),
            "{outcome:?}"
        );
        assert!(ws.join("marker.txt").exists(), "cwd 必须固定在工作区");
        assert_eq!(outcome.workdir_policy, "workspace");
        assert!(outcome.runtime_fingerprint.starts_with("gitbash/"));
        let _ = std::fs::remove_dir_all(&ws);
    }
}
