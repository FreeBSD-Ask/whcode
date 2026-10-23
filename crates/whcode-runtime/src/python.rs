//! # `python` 运行时（T111、D-09、ADR-024 决策 2：受控子进程 worker）
//!
//! 前身是 `providers.rs` 的 `PythonProvider`（`--version` 探测 + `-c` 直跑）；
//! 本模块在其上落地完整 worker 协议与环境记录。不嵌入解释器、不动态链接
//! `libpython`（D-09），解释器命令可配置（默认 `python`）。
//!
//! ## 环境记录（Check 111/154、ADR-024 决策 2）
//!
//! [`PythonProvider::probe_env`] 以 `<interpreter> -I -c <探测引导>` 采集
//! [`PythonEnv`]：解释器绝对路径（`sys.executable`）、版本、实现名
//! （`platform.python_implementation()`，CPython/PyPy 等）、ABI 标签
//! （`sysconfig` 的 EXT_SUFFIX）、C 扩展可用性（`import ctypes` 且
//! `ctypes.pythonapi` 可用，显式 bool）与 `sys.prefix`（依赖定位：本运行时
//! 依赖用户安装的系统解释器，不随包分发，也不承诺第三方包）。探测失败
//! （无解释器、超时、退出非零、输出不可解析）一律返回 `None`；
//! [`PythonEnv::fingerprint`] 为稳定摘要 `python/<实现>/<版本>/<ABI>`。
//! `-I` 为隔离模式：忽略 `PYTHON*` 环境变量与用户 site 目录，探测与执行
//! 都不继承调用方 Python 环境。
//!
//! ## worker 协议
//!
//! 执行统一经 [`crate::worker`]（超时、取消、输出上限、尾部保留、凭据剥离）：
//! 子进程 `<interpreter> -I -c <worker 引导>` 从 stdin 读一行 JSON 请求
//! `{"script":...,"input":...}`，以受限全局名执行脚本（脚本内 `input` 为字符串
//! 变量，遮蔽同名内置函数；结果约定写入全局名 `result`，未设置则为 null），
//! 把结果/异常序列化为单行 JSON 写 stdout；失败（语法错误、异常、`SystemExit`）
//! 一律非零退出码并把错误 JSON 写 stderr。引导自身封顶输出（结果 16 KiB、
//! 捕获流各 8 KiB、回溯 8 KiB，超限显式标记 `...[WHCODE_TRUNCATED]`）；脚本
//! 的 print/stderr 被重定向进有界捕获，子进程真实 stdout/stderr 只承载协议
//! JSON，父进程侧缓冲因此有界。退出码 0 而 stdout 不是 `ok=true` 的协议 JSON
//! 视为协议破坏，返回结构化错误（0 信任，不静默）。
//!
//! 传输偏离说明：`worker::execute_with_cancel` 只接管 stdout/stderr，没有
//! stdin 写入路径；请求行经 `temp_dir` 下的一次性请求文件（`Stdio::from(File)`）
//! 供入并由 RAII 删除，`workdir_policy` 由此为 "temp"。文件 EOF 自然到来，
//! 不存在管道写/读顺序死锁类；请求上限 [`MAX_REQUEST_BYTES`]，超限在进入
//! 子进程前以纯内存校验拒绝。
//!
//! ## 受限档位是承诺边界，不是沙箱
//!
//! `declaration.unsupported` 列明：任意标准库模块（不承诺完整覆盖）、任意
//! 第三方包、文件系统写访问、网络访问、图形界面。“不支持”指不承诺、不提供、
//! 不验收该能力；子进程以当前用户权限运行，OS 级隔离未实现（与 T104 同口径），
//! 脚本仍可能触碰文件系统与网络——不得把本运行时表述为安全沙箱。

use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use whcode_domain::{ErrorCode, WhError};

use crate::capability::{Capability, RuntimeDeclaration};
use crate::checkpoint::Checkpoint;
use crate::provider::{RuntimeProvider, Script};
use crate::worker;

/// 探测引导（单行、自包含、无外部依赖）：打印 [`PythonEnv`] 各字段的 JSON。
///
/// C 扩展探测经 `exec` 内嵌 try/except（单行写法），`import ctypes` 失败时
/// 如实记 `false` 而不是让整个探测失败——探测要“记录结果”，不是“要求成功”。
const PROBE_BOOTSTRAP: &str = r#"import json, platform, sys, sysconfig; exec("try:\n    import ctypes\n    _ok = getattr(ctypes, 'pythonapi', None) is not None\nexcept Exception:\n    _ok = False"); print(json.dumps({"interpreter_path": sys.executable, "version": platform.python_version(), "implementation": platform.python_implementation(), "abi": sysconfig.get_config_var("EXT_SUFFIX") or "", "sys_prefix": sys.prefix, "c_extensions_ok": bool(_ok)}))"#;

/// worker 引导（见模块文档“worker 协议”一节）：stdin 读一行 JSON 请求，
/// 受限全局执行，stdout/stderr 只承载协议 JSON；一切输出有界并显式标记截断。
const WORKER_BOOTSTRAP: &str = r##"import contextlib
import io
import json
import sys
import traceback

MAX_RESULT_CHARS = 16384
MAX_STREAM_CHARS = 8192
MAX_TRACEBACK_CHARS = 8192


class _BoundedIO(io.StringIO):
    def __init__(self, limit):
        super().__init__()
        self._limit = limit

    def write(self, text):
        room = self._limit - self.tell()
        if room > 0:
            super().write(text if len(text) <= room else text[:room])
            if len(text) > room:
                super().write("...[WHCODE_TRUNCATED]")
        return len(text)


def _cap(text, limit):
    if not isinstance(text, str):
        return ""
    if len(text) <= limit:
        return text
    return text[:limit] + "...[WHCODE_TRUNCATED]"


def _emit(stream, payload):
    stream.write(json.dumps(payload, ensure_ascii=True))
    stream.write("\n")
    stream.flush()


def _run():
    raw = sys.stdin.buffer.readline()
    payload = json.loads(raw.decode("utf-8"))
    source = compile(payload["script"], "<whcode-script>", "exec")
    scope = {"__name__": "__whcode_script__", "input": payload.get("input", "")}
    out = _BoundedIO(MAX_STREAM_CHARS)
    err = _BoundedIO(MAX_STREAM_CHARS)
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        exec(source, scope)
    emitted = {
        "ok": True,
        "result": None,
        "stdout": _cap(out.getvalue(), MAX_STREAM_CHARS),
        "stderr": _cap(err.getvalue(), MAX_STREAM_CHARS),
    }
    if "result" in scope:
        emitted["result"] = _cap(repr(scope["result"]), MAX_RESULT_CHARS)
    _emit(sys.stdout, emitted)


def _main():
    try:
        _run()
    except BaseException:
        _emit(sys.stderr, {"ok": False, "error": _cap(traceback.format_exc(), MAX_TRACEBACK_CHARS)})
        sys.exit(1)


_main()
"##;

/// 请求行上限（脚本 + 输入的 JSON 序列化；超限在进入子进程前拒绝——有界缓冲纪律）。
pub const MAX_REQUEST_BYTES: usize = 4 * 1024 * 1024;

/// 探测超时（解释器启动通常远小于此；挂死的解释器按不可用处理）。
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// 解释器环境记录（Check 111/154：绝对路径、版本、实现名、ABI、C 扩展、依赖定位）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PythonEnv {
    /// 解释器绝对路径（`sys.executable`）。
    pub interpreter_path: String,
    /// 解释器版本（`platform.python_version()`）。
    pub version: String,
    /// 实现名（`platform.python_implementation()`，CPython/PyPy 等）。
    pub implementation: String,
    /// ABI 标签（`sysconfig` 的 EXT_SUFFIX，如 `.cp314-win_amd64.pyd`）。
    pub abi: String,
    /// 解释器安装前缀（`sys.prefix`，依赖定位——系统解释器不随包分发）。
    pub sys_prefix: String,
    /// C 扩展可用性（`import ctypes` 且 `ctypes.pythonapi` 可用；显式 bool）。
    pub c_extensions_ok: bool,
}

impl PythonEnv {
    /// 稳定摘要（实现名 + 版本 + ABI；供检查点指纹与门禁展示）。
    #[must_use]
    pub fn fingerprint(&self) -> String {
        let abi = if self.abi.is_empty() {
            "abi-unknown"
        } else {
            self.abi.as_str()
        };
        format!("python/{}/{}/{}", self.implementation, self.version, abi)
    }
}

/// Python 子进程进程级并发闸门：`worker::default_gate` 未公开，按 T111 口径在
/// 本模块以静态实例承接（上限对齐 [`worker::DEFAULT_MAX_CONCURRENT`]，RAII 许可）；
/// worker 内部另有进程级默认闸门，双层各有界且无交叉锁序（先取本闸门再进 worker）。
static PYTHON_GATE: OnceLock<worker::ConcurrencyGate> = OnceLock::new();

fn python_gate() -> &'static worker::ConcurrencyGate {
    PYTHON_GATE.get_or_init(|| worker::ConcurrencyGate::new(worker::DEFAULT_MAX_CONCURRENT))
}

/// stdin 请求行（单行 JSON；serde 转义保证不含原始换行，`readline` 一次取全）。
#[derive(serde::Serialize)]
struct WorkerRequest<'a> {
    script: &'a str,
    input: &'a str,
}

/// 请求临时文件（RAII：Drop 即删除；NFR-215 资源纪律。清理为尽力而为，
/// 失败不吞执行结果——残留一个临时文件好过丢失一次受控执行记录）。
struct TempRequestFile {
    path: std::path::PathBuf,
}

impl Drop for TempRequestFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// 结构化不可用错误（与 `ensure_available` 对 python 同口径：Missing + 安装建议；
/// execute 内联使用以避免“可用性探测 + 指纹探测”重复起子进程）。
fn python_unavailable() -> WhError {
    crate::errors::runtime_unavailable(
        "python",
        crate::errors::UnavailableReason::Missing,
        "whcode runtime install python",
    )
}

/// 序列化请求行并做双重上限校验（原始长度预检 + 转义后复查；纯内存，无子进程）。
fn serialize_request(script: &Script) -> Result<String, WhError> {
    if script.script.len() + script.input.len() > MAX_REQUEST_BYTES {
        return Err(WhError::new(
            ErrorCode::InvalidState,
            format!(
                "python 请求原始长度超过上限 {MAX_REQUEST_BYTES} 字节，拒绝执行（有界缓冲纪律）"
            ),
        ));
    }
    let request = serde_json::to_string(&WorkerRequest {
        script: script.script.as_str(),
        input: script.input.as_str(),
    })
    .map_err(|err| {
        WhError::new(
            ErrorCode::InvalidState,
            format!("序列化 python worker 请求失败：{err}"),
        )
    })?;
    if request.len() > MAX_REQUEST_BYTES {
        return Err(WhError::new(
            ErrorCode::InvalidState,
            format!(
                "python 请求序列化后 {} 字节超过上限 {MAX_REQUEST_BYTES}，拒绝执行（有界缓冲纪律）",
                request.len()
            ),
        ));
    }
    Ok(request)
}

/// 把请求行写入一次性临时文件（名字含进程号与进程内序号，避免并发互踩）。
fn write_request_file(request: &str) -> Result<TempRequestFile, WhError> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "whcode-py-request-{}-{}.json",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, request.as_bytes()).map_err(|err| {
        WhError::new(
            ErrorCode::IoError,
            format!("写入 python worker 请求临时文件失败：{err}"),
        )
    })?;
    Ok(TempRequestFile { path })
}

/// 协议校验（0 信任）：退出码 0 时 stdout 必须是 `ok=true` 的协议 JSON；
/// 非零退出/超时/取消由 [`worker::WorkerResult`] 字段如实表达，不在此改写。
fn validate_protocol(result: &worker::WorkerResult) -> Result<(), WhError> {
    if result.exit_code != Some(0) || result.timed_out || result.cancelled {
        return Ok(());
    }
    let parsed: serde_json::Value =
        serde_json::from_str(result.stdout_tail.trim()).map_err(|err| {
            WhError::new(
                ErrorCode::InvalidState,
                format!("python worker 协议破坏：退出码 0 但 stdout 不是协议 JSON（{err}）"),
            )
        })?;
    if parsed.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        return Err(WhError::new(
            ErrorCode::InvalidState,
            "python worker 协议破坏：退出码 0 但 ok 标志不是 true".to_string(),
        ));
    }
    Ok(())
}

/// `python` 运行时（T111、D-09：受控子进程 worker，系统解释器）。
#[derive(Debug, Clone)]
pub struct PythonProvider {
    declaration: RuntimeDeclaration,
    /// 解释器命令（默认 `python`；配置可改）。
    pub interpreter: String,
}

impl Default for PythonProvider {
    fn default() -> Self {
        Self {
            declaration: RuntimeDeclaration {
                name: "python".to_string(),
                capability: Capability::Restricted,
                // 承诺边界（见模块文档“受限档位”一节）：只列不承诺的能力，
                // 不宣称沙箱——子进程以当前用户权限运行，OS 级隔离未实现。
                unsupported: vec![
                    "任意标准库模块（不承诺完整覆盖）".to_string(),
                    "任意第三方包".to_string(),
                    "文件系统写访问".to_string(),
                    "网络访问".to_string(),
                    "图形界面".to_string(),
                ],
            },
            interpreter: "python".to_string(),
        }
    }
}

impl PythonProvider {
    /// 探测解释器环境（Check 111/154；供 doctor/门禁展示，本任务不加 CLI）。
    ///
    /// 失败（无解释器、超时、退出非零、输出不可解析）返回 `None`；探测统一经
    /// [`worker::execute`]（并发闸门、凭据剥离、超时与输出上限，§10.2）。
    #[must_use]
    pub fn probe_env(&self) -> Option<PythonEnv> {
        let mut command = Command::new(&self.interpreter);
        command.arg("-I").arg("-c").arg(PROBE_BOOTSTRAP);
        let result = worker::execute(&mut command, PROBE_TIMEOUT, worker::MAX_OUTPUT_BYTES).ok()?;
        if result.exit_code != Some(0) || result.timed_out || result.cancelled {
            return None;
        }
        serde_json::from_str(result.stdout_tail.trim()).ok()
    }

    /// 协作式取消执行（FR-705）：`cancel` 置位后 worker 在下一轮询（≤10 ms）
    /// 终止子进程；取消与超时在 [`worker::WorkerResult`] 中可区分。
    ///
    /// # Errors
    ///
    /// 请求超限或协议破坏时返回 [`WhError`]；取消与超时不是错误，随结果返回。
    pub fn execute_with_cancel_flag(
        &self,
        script: &Script,
        cancel: &AtomicBool,
    ) -> Result<Checkpoint, WhError> {
        self.execute_inner(script, Some(cancel))
    }

    /// 执行主干：请求校验 → 并发闸门 → 环境探测 → 临时请求文件 → worker 执行
    /// → 协议校验 → 检查点四要素。
    fn execute_inner(
        &self,
        script: &Script,
        cancel: Option<&AtomicBool>,
    ) -> Result<Checkpoint, WhError> {
        // 有界校验先于一切子进程动作：超限请求纯内存拒绝（无解释器也生效）。
        let request = serialize_request(script)?;
        let _permit = python_gate().acquire();
        let env = self.probe_env().ok_or_else(python_unavailable)?;
        let request_file = write_request_file(&request)?;
        let result = self.spawn_worker(&request_file, script.timeout, cancel)?;
        validate_protocol(&result)?;
        Ok(Checkpoint {
            script_hash: crate::payload::fnv64(script.script.as_bytes()),
            input_hash: crate::payload::fnv64(script.input.as_bytes()),
            runtime_fingerprint: env.fingerprint(),
            workdir_policy: "temp".to_string(),
            result: Some(result),
        })
    }

    /// 启动 worker 子进程并交由 [`worker::execute_with_cancel`] 全程受控。
    ///
    /// worker 只接管 stdout/stderr（无 stdin 写入路径），请求行经请求文件以
    /// `Stdio::from(File)` 供入；文件句柄随 `Command` 丢弃，请求文件由
    /// [`TempRequestFile`] 的 RAII 删除。
    fn spawn_worker(
        &self,
        request_file: &TempRequestFile,
        timeout: Duration,
        cancel: Option<&AtomicBool>,
    ) -> Result<worker::WorkerResult, WhError> {
        let mut command = Command::new(&self.interpreter);
        command.arg("-I").arg("-c").arg(WORKER_BOOTSTRAP);
        let source = std::fs::File::open(&request_file.path).map_err(|err| {
            WhError::new(
                ErrorCode::IoError,
                format!("打开 python worker 请求文件失败：{err}"),
            )
        })?;
        command.stdin(Stdio::from(source));
        worker::execute_with_cancel(&mut command, timeout, worker::MAX_OUTPUT_BYTES, cancel)
    }
}

impl RuntimeProvider for PythonProvider {
    fn name(&self) -> &str {
        "python"
    }
    fn declaration(&self) -> &RuntimeDeclaration {
        &self.declaration
    }
    fn is_available(&self) -> bool {
        self.probe_env().is_some()
    }
    fn env_fingerprint(&self) -> String {
        self.probe_env()
            .map(|env| env.fingerprint())
            .unwrap_or_else(|| "python/unavailable".to_string())
    }
    fn execute(&self, script: &Script) -> Result<Checkpoint, WhError> {
        self.execute_inner(script, None)
    }
    fn terminate(&mut self) -> Result<(), WhError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Check 111：探测两态一致——fingerprint 与 is_available 必须同源同判定。
    #[test]
    fn python_probes_system_interpreter() {
        let provider = PythonProvider::default();
        // 系统可能无 python——两个分支都合法，关键是 fingerprint 一致性。
        let fingerprint = provider.env_fingerprint();
        assert_eq!(provider.is_available(), fingerprint != "python/unavailable");
        if fingerprint != "python/unavailable" {
            assert!(
                fingerprint.starts_with("python/"),
                "指纹格式应为 python/<实现>/<版本>/<ABI>：{fingerprint}"
            );
        }
    }

    /// Check 111/154：有解释器时记录绝对路径/版本/实现/ABI/C 扩展，指纹稳定；
    /// 无解释器时探测返回 None 且不 panic。
    #[test]
    fn probe_env_records_interpreter_details() {
        let provider = PythonProvider::default();
        if !provider.is_available() {
            assert!(provider.probe_env().is_none(), "无解释器时探测必须为 None");
            eprintln!("本机无 python 解释器：字段级断言按前提不满足显式跳过（非静默）");
            return;
        }
        let env = provider.probe_env().expect("可用则必有探测结果");
        assert!(!env.interpreter_path.is_empty());
        assert!(
            std::path::Path::new(&env.interpreter_path).is_absolute(),
            "解释器路径必须是绝对路径：{}",
            env.interpreter_path
        );
        assert!(!env.version.is_empty());
        assert!(!env.implementation.is_empty());
        assert!(
            !env.abi.is_empty(),
            "Windows 解释器必须给出 ABI 标签（EXT_SUFFIX）"
        );
        assert!(!env.sys_prefix.is_empty());
        let fingerprint = env.fingerprint();
        assert!(
            fingerprint.contains(&env.implementation),
            "指纹须含实现名：{fingerprint}"
        );
        assert!(
            fingerprint.contains(&env.version),
            "指纹须含版本：{fingerprint}"
        );
        assert!(
            fingerprint.contains(&env.abi),
            "指纹须含 ABI：{fingerprint}"
        );
        // 稳定摘要：同一解释器两次探测得到同一指纹。
        let again = provider.probe_env().expect("第二次探测");
        assert_eq!(fingerprint, again.fingerprint(), "指纹必须稳定");
    }

    /// Check 111（后半）：受限档位必须列明不支持项（承诺边界，见模块文档）。
    #[test]
    fn declaration_lists_restricted_boundary() {
        let declaration = PythonProvider::default().declaration().clone();
        assert_eq!(declaration.capability, Capability::Restricted);
        for item in [
            "任意标准库",
            "任意第三方包",
            "文件系统写访问",
            "网络访问",
            "图形界面",
        ] {
            assert!(
                declaration
                    .unsupported
                    .iter()
                    .any(|entry| entry.contains(item)),
                "不支持项缺少：{item}"
            );
        }
    }

    /// 本机无解释器时，协议级用例以前提不满足显式跳过（eprintln! 说明，非静默）。
    fn require_python(provider: &PythonProvider) -> bool {
        if provider.is_available() {
            return true;
        }
        eprintln!("本机无 python 解释器：协议级用例按前提不满足显式跳过（非静默）");
        false
    }

    /// 协议正向：input 注入 + result 全局名 → 成功且输出含结果（Check 111）。
    #[test]
    fn protocol_runs_script_and_returns_result() {
        let provider = PythonProvider::default();
        if !require_python(&provider) {
            return;
        }
        let checkpoint = provider
            .execute(&Script {
                script: "result = int(input) * 7".to_string(),
                input: "6".to_string(),
                timeout: Duration::from_secs(30),
            })
            .expect("协议执行");
        crate::checkpoint::validate(&checkpoint).expect("检查点四要素齐备");
        assert_eq!(checkpoint.workdir_policy, "temp");
        assert_ne!(
            checkpoint.runtime_fingerprint, "python/unavailable",
            "指纹应来自探测结果"
        );
        let result = checkpoint.result.expect("结果登记");
        assert_eq!(result.exit_code, Some(0));
        assert!(!result.timed_out);
        assert!(!result.cancelled);
        let payload: serde_json::Value =
            serde_json::from_str(result.stdout_tail.trim()).expect("协议 JSON");
        assert_eq!(payload["ok"], serde_json::json!(true));
        assert_eq!(payload["result"], "42", "结果须来自脚本全局名 result");
    }

    /// 协议失败路径：脚本 raise → 非零退出码 + stderr 携带错误信息。
    #[test]
    fn protocol_failure_reports_nonzero_exit_and_stderr() {
        let provider = PythonProvider::default();
        if !require_python(&provider) {
            return;
        }
        let checkpoint = provider
            .execute(&Script {
                script: "raise ValueError(\"boom-xyz\")".to_string(),
                input: String::new(),
                timeout: Duration::from_secs(30),
            })
            .expect("脚本失败也是受控完成，随检查点返回");
        let result = checkpoint.result.expect("结果登记");
        assert_ne!(result.exit_code, Some(0), "失败必须非零退出码");
        assert!(
            result.stderr_tail.contains("boom-xyz"),
            "stderr 必须携带错误信息：{:?}",
            result.stderr_tail
        );
    }

    /// 超时路径：死循环脚本 → worker 终止并标记 timed_out（不混同取消）。
    #[test]
    fn protocol_timeout_kills_runaway_script() {
        let provider = PythonProvider::default();
        if !require_python(&provider) {
            return;
        }
        let checkpoint = provider
            .execute(&Script {
                script: "while True: pass".to_string(),
                input: String::new(),
                timeout: Duration::from_millis(300),
            })
            .expect("超时是受控结果");
        let result = checkpoint.result.expect("结果登记");
        assert!(result.timed_out, "必须标记超时");
        assert!(!result.cancelled, "超时不得混同为取消");
        assert_eq!(result.exit_code, None);
    }

    /// 输出上限：超大打印 → 双层有界（子进程侧有界捕获 + worker 尾部上限），
    /// 截断显式标记而非静默丢弃。
    #[test]
    fn protocol_output_is_bounded() {
        let provider = PythonProvider::default();
        if !require_python(&provider) {
            return;
        }
        let checkpoint = provider
            .execute(&Script {
                script: "print(\"A\" * 10000000)".to_string(),
                input: String::new(),
                timeout: Duration::from_secs(60),
            })
            .expect("执行");
        let result = checkpoint.result.expect("结果登记");
        assert!(
            result.stdout_tail.len() < worker::MAX_OUTPUT_BYTES,
            "stdout 尾部必须有界：{} 字节",
            result.stdout_tail.len()
        );
        let payload: serde_json::Value =
            serde_json::from_str(result.stdout_tail.trim()).expect("协议 JSON");
        let captured = payload["stdout"].as_str().expect("stdout 字段");
        assert!(
            captured.len() <= 8192 + 24,
            "捕获流必须有界（8 KiB + 标记）：{}",
            captured.len()
        );
        assert!(
            captured.contains("WHCODE_TRUNCATED"),
            "截断必须显式标记：{captured}"
        );
    }

    /// 取消与超时可区分（FR-705）：置位后由 worker 在轮询间隔内终止。
    #[test]
    fn cancel_is_distinguishable_from_timeout() {
        let provider = PythonProvider::default();
        if !require_python(&provider) {
            return;
        }
        let flag = std::sync::Arc::new(AtomicBool::new(false));
        let worker_flag = std::sync::Arc::clone(&flag);
        let handle = std::thread::spawn(move || {
            provider.execute_with_cancel_flag(
                &Script {
                    script: "import time; time.sleep(30)".to_string(),
                    input: String::new(),
                    timeout: Duration::from_secs(60),
                },
                worker_flag.as_ref(),
            )
        });
        std::thread::sleep(Duration::from_millis(300));
        flag.store(true, Ordering::SeqCst);
        let checkpoint = handle.join().expect("等待线程").expect("取消是受控结果");
        let result = checkpoint.result.expect("结果登记");
        assert!(result.cancelled, "必须标记取消");
        assert!(!result.timed_out, "取消不得混同为超时");
        assert_eq!(result.exit_code, None);
    }

    /// 请求上限：超限请求在进入子进程前被拒绝（有界缓冲纪律；无解释器也生效）。
    #[test]
    fn oversized_request_is_rejected_before_spawning() {
        let provider = PythonProvider::default();
        let err = provider
            .execute(&Script {
                script: "x = 1".to_string(),
                input: "A".repeat(MAX_REQUEST_BYTES + 1),
                timeout: Duration::from_secs(1),
            })
            .expect_err("超限请求必须拒绝");
        assert!(err.to_string().contains("上限"), "错误须说明上限：{err}");
    }
}
