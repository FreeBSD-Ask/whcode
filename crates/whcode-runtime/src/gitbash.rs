//! # `gitbash` 运行时（T112、V-304：Windows PortableGit payload）
//!
//! 迁移自 `providers.rs`（原 GitBashProvider 原文搬运），本模块在其上落地：
//!
//! - **payload 四重校验**（ADR-024 决策 3）：PortableGit 以归档（zip/7z）**独立分发、
//!   不随主二进制携带**；安装前经 [`verify_payload`] 转调 [`crate::payload::validate`]
//!   做“来源白名单 + 版本 + 平台 + 哈希”四重校验，失败返回结构化错误（FR-708）。
//!   哈希沿用 FNV-1a 64（[`crate::payload::fnv64`]）：定位是**意外损坏检测**，非密码学；
//!   失效条件——无法防主动篡改，防篡改须在分发侧叠加外部签名校验（与 payload.rs 同口径）。
//! - **Bash 执行**：`bash.exe -c <script>` 子进程（ADR-008：主进程不直接执行不受信任
//!   脚本），stdin 输入经一次性临时文件注入（与 python.rs 同款 `Stdio::from(File)` 模式）；
//!   超时、输出上限、取消、凭据剥离统一经 [`crate::worker`]；并发由本模块静态闸门约束
//!   （上限 [`worker::DEFAULT_MAX_CONCURRENT`]）。请求大小上限 [`MAX_REQUEST_BYTES`]
//!   在进入子进程前纯内存拒绝（有界缓冲纪律）。
//! - **五类能力探测**（V-304、Check 112/155）：管道、重定向、含空格路径、信号终止
//!   （由超时终止路径覆盖）、目标命令，见 [`capability_probes`]；每条脚本自包含、
//!   不依赖网络。
//!
//! 命名边界（§4.7.1）：`gitbash` ≠ `bsdshell`；任意系统 Shell 永不在能力清单内（FR-608）。
//!
//! [`locate_bash`] 候选顺序（有界，≤9 项）：`git_path` 同目录 → `git_path` 上级目录的
//! `bin/` 与 `usr/bin/` → PATH；每个候选须实测 `--version` 输出含 “bash”，不轻信路径存在。
//! 已知失效条件：PATH 回退可能命中 WSL 的 `bash.exe`（其版本行同样含 “bash”，但运行环境
//! 是 Linux）——需要确定性时显式把 `git_path` 配置为 PortableGit 根目录或 `git.exe` 路径，
//! 不依赖 PATH 回退。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use whcode_domain::{ErrorCode, WhError};

use crate::capability::{Capability, RuntimeDeclaration};
use crate::checkpoint::Checkpoint;
use crate::payload::{PayloadMeta, SourceAllowlist};
use crate::provider::{RuntimeProvider, Script};
use crate::worker;

/// payload 校验的预期平台（ADR-013：Windows 11 amd64 唯一验收平台）。
const EXPECTED_PLATFORM: &str = "windows-amd64";

/// 请求上限（脚本 + 输入字节合计；超限在进入子进程前纯内存拒绝——有界缓冲纪律，
/// 与 python 运行时同语义同数值）。
pub const MAX_REQUEST_BYTES: usize = 4 * 1024 * 1024;

/// 探测超时（bash 启动通常远小于此；挂死的候选按不可用处理）。
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// gitbash 子进程进程级并发闸门：与 python 运行时同口径（T111），上限对齐
/// [`worker::DEFAULT_MAX_CONCURRENT`]，RAII 许可；worker 内部另有进程级默认闸门，
/// 双层各有界且无交叉锁序（先取本闸门再进 worker）。
static GITBASH_GATE: OnceLock<worker::ConcurrencyGate> = OnceLock::new();

fn gitbash_gate() -> &'static worker::ConcurrencyGate {
    GITBASH_GATE.get_or_init(|| worker::ConcurrencyGate::new(worker::DEFAULT_MAX_CONCURRENT))
}

/// 校验 PortableGit payload 归档字节（ADR-024 决策 3：来源白名单 + 版本 + 哈希 + 平台四重）。
///
/// 面向**单个 payload 归档文件**的字节（PortableGit 以 zip/7z 分发）；目录形式不做。
/// 预期平台固定为 [`EXPECTED_PLATFORM`]。哈希算法沿用 [`crate::payload::fnv64`]：
/// 定位是意外损坏检测、非密码学；失效条件——无法防主动篡改，防篡改须外部签名校验。
///
/// # Errors
///
/// 转调 [`crate::payload::validate`] 并在错误信息注明 gitbash 语境：来源不在白名单为
/// `permission_denied`（不可重试）；平台不匹配、版本为空或校验和不符为
/// `runtime_unavailable`（不静默接受）。
pub fn verify_payload(
    data: &[u8],
    meta: &PayloadMeta,
    allowlist: &SourceAllowlist,
) -> Result<(), WhError> {
    crate::payload::validate(data, meta, EXPECTED_PLATFORM, allowlist).map_err(|err| WhError {
        message: format!("gitbash payload 校验失败：{}", err.message),
        ..err
    })
}

/// 候选清单追加去重（清单有界：≤9 项，无累积风险）。
fn push_unique(candidates: &mut Vec<PathBuf>, path: PathBuf) {
    if !candidates.contains(&path) {
        candidates.push(path);
    }
}

/// 候选 bash 路径（有界清单）：`git_path` 语义为“PortableGit 根目录或 `git.exe` 路径”。
///
/// - `git_path` 是目录（PortableGit 根）：`<根>/bin/bash.exe`、`<根>/bin/sh.exe`、
///   `<根>/usr/bin/bash.exe`、`<根>/usr/bin/sh.exe`；
/// - `git_path` 是文件：同目录 `bash.exe`/`sh.exe`（PortableGit 的 `bin/git.exe` 与
///   bash 同级），再查上级目录的 `bin/` 与 `usr/bin/`（完整安装的 `cmd/git.exe` 布局）；
/// - 最后回退 PATH 的 `bash.exe`（由 [`Command`] 的 PATH 搜索语义承载）。
fn bash_candidates(git_path: Option<&str>) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(configured) = git_path {
        let configured = Path::new(configured);
        if configured.is_dir() {
            for name in ["bash.exe", "sh.exe"] {
                push_unique(&mut candidates, configured.join("bin").join(name));
                push_unique(
                    &mut candidates,
                    configured.join("usr").join("bin").join(name),
                );
            }
        } else if let Some(dir) = configured.parent() {
            for name in ["bash.exe", "sh.exe"] {
                push_unique(&mut candidates, dir.join(name));
                if let Some(root) = dir.parent() {
                    push_unique(&mut candidates, root.join("bin").join(name));
                    push_unique(&mut candidates, root.join("usr").join("bin").join(name));
                }
            }
        }
    }
    candidates.push(PathBuf::from("bash.exe"));
    candidates
}

/// 候选实测可用：`--version` 成功且 stdout 含 “bash”（sh.exe 同为 bash，版本行一致；
/// 探测统一经 [`worker::execute`]——并发闸门、凭据剥离、超时与输出上限，§10.2）。
fn verify_bash_candidate(path: &Path) -> bool {
    let mut command = Command::new(path);
    command.arg("--version");
    let Ok(result) = worker::execute(&mut command, PROBE_TIMEOUT, worker::MAX_OUTPUT_BYTES) else {
        return false;
    };
    result.exit_code == Some(0) && result.stdout_tail.contains("bash")
}

/// 定位本机 Git Bash（V-304）：依次实测候选（见 [`bash_candidates`]），返回首个
/// `--version` 输出含 “bash” 者；全部失败返回 `None`。未配置 `git_path` 时仅查 PATH
/// 回退——需要确定性时显式配置（见模块文档失效条件）。
///
/// PATH 回退命中时返回**绝对路径**：调用方不应拿到依赖当前工作目录的相对名
/// （GitHub Windows runner 实测：过宽的根候选会走到 PATH 回退）。
#[must_use]
pub fn locate_bash(git_path: Option<&str>) -> Option<PathBuf> {
    let found = bash_candidates(git_path)
        .into_iter()
        .find(|candidate| verify_bash_candidate(candidate))?;
    if found.is_absolute() {
        return Some(found);
    }
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join(&found))
        .find(|candidate| candidate.is_file())
        .or(Some(found))
}

/// 探测 bash 版本摘要（`--version` 首行 “version ” 后的 token，如 `5.3.9(1)-release`）；
/// 探测失败（非零退出/超时/输出不可解析）返回 `None`。
fn probe_bash_version(bash: &Path) -> Option<String> {
    let mut command = Command::new(bash);
    command.arg("--version");
    let result = worker::execute(&mut command, PROBE_TIMEOUT, worker::MAX_OUTPUT_BYTES).ok()?;
    if result.exit_code != Some(0) || result.timed_out || result.cancelled {
        return None;
    }
    let first_line = result.stdout_tail.lines().next()?;
    let token = first_line
        .split("version ")
        .nth(1)?
        .split_whitespace()
        .next()?;
    (!token.is_empty()).then(|| token.to_string())
}

/// 五类能力探测（V-304、Check 112/155）：管道、重定向、含空格路径、信号终止、目标命令。
///
/// 每条脚本自包含（mktemp 生成一次性路径并自行清理）、不依赖网络。`signal_terminate`
/// 的脚本故意阻塞，判定标准是**被超时终止**（`timed_out = true`，即 worker 的 kill
/// 信号终止路径）而非正常退出——调用方须按名字区分判定。
#[must_use]
pub fn capability_probes() -> &'static [(&'static str, &'static str)] {
    &[
        ("pipe", "printf 'a\\n' | tr a-z A-Z"),
        (
            "redirect",
            "f=$(mktemp) && printf x > \"$f\" && cat \"$f\" && rm -f \"$f\"",
        ),
        (
            "space_path",
            "base=$(mktemp -d) && d=\"$base/space dir/probe\" && mkdir -p \"$d\" && printf ok > \"$d/out\" && cat \"$d/out\" && rm -rf \"$base\"",
        ),
        ("signal_terminate", "sleep 30"),
        ("target_command", "git --version"),
    ]
}

/// 运行单条能力探测：`bash -c <script>`，经 [`worker::execute`] 全程受控（并发闸门、
/// 凭据剥离、超时终止、输出上限）。
///
/// # Errors
///
/// 子进程启动失败返回 [`WhError`]；脚本失败（非零退出/超时）随 [`worker::WorkerResult`]
/// 返回，不是错误。
pub fn run_probe(
    bash: &Path,
    script: &str,
    timeout: Duration,
) -> Result<worker::WorkerResult, WhError> {
    let mut command = Command::new(bash);
    command.arg("-c").arg(script);
    worker::execute(&mut command, timeout, worker::MAX_OUTPUT_BYTES)
}

/// 结构化不可用错误（bash 未定位或版本探测失败；与 [`crate::provider::ensure_available`]
/// 同口径：Missing + 可操作建议）。
fn gitbash_unavailable() -> WhError {
    crate::errors::runtime_unavailable(
        "gitbash",
        crate::errors::UnavailableReason::Missing,
        "安装 PortableGit 并把 git_path 配置为其根目录或 git.exe 路径，或确保 PATH 可定位 bash.exe",
    )
}

/// 请求大小上限校验（纯内存，先于一切子进程动作——有界缓冲纪律）。
fn check_request_size(script: &Script) -> Result<(), WhError> {
    let total = script.script.len() + script.input.len();
    if total > MAX_REQUEST_BYTES {
        return Err(WhError::new(
            ErrorCode::InvalidState,
            format!(
                "gitbash 请求 {total} 字节超过上限 {MAX_REQUEST_BYTES}，拒绝执行（有界缓冲纪律）"
            ),
        ));
    }
    Ok(())
}

/// stdin 输入临时文件（RAII：Drop 即删除；NFR-215 资源纪律。清理为尽力而为，
/// 失败不吞执行结果——残留一个临时文件好过丢失一次受控执行记录）。
struct TempInputFile {
    path: PathBuf,
}

impl Drop for TempInputFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// 把 stdin 数据写入一次性临时文件（名字含进程号与进程内序号，避免并发互踩）。
fn write_input_file(input: &str) -> Result<TempInputFile, WhError> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "whcode-bash-input-{}-{}.txt",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, input.as_bytes()).map_err(|err| {
        WhError::new(
            ErrorCode::IoError,
            format!("写入 gitbash stdin 临时文件失败：{err}"),
        )
    })?;
    Ok(TempInputFile { path })
}

/// 执行主干（bash 定位结果统一在此映射结构化错误）：版本探测 → stdin 临时文件 →
/// `bash -c` 子进程（cwd 固定在临时目录，`workdir_policy` 由此为 “temp”）→ 检查点四要素。
fn execute_with_bash(
    bash: Option<&Path>,
    script: &Script,
    cancel: Option<&AtomicBool>,
) -> Result<Checkpoint, WhError> {
    let bash = bash.ok_or_else(gitbash_unavailable)?;
    let version = probe_bash_version(bash).ok_or_else(gitbash_unavailable)?;
    let input_file = write_input_file(&script.input)?;
    let mut command = Command::new(bash);
    command.arg("-c").arg(&script.script);
    // workdir_policy “temp” 的实际承载：脚本 cwd 固定在临时目录，相对路径写入
    // 不污染调用方工作目录（临时目录必已存在——stdin 文件刚刚写入成功）。
    command.current_dir(std::env::temp_dir());
    // worker 只接管 stdout/stderr（无 stdin 写入路径），输入经临时文件以
    // `Stdio::from(File)` 供入；句柄随 Command 丢弃，文件由 TempInputFile RAII 删除。
    let source = std::fs::File::open(&input_file.path).map_err(|err| {
        WhError::new(
            ErrorCode::IoError,
            format!("打开 gitbash stdin 临时文件失败：{err}"),
        )
    })?;
    command.stdin(Stdio::from(source));
    let result = worker::execute_with_cancel(
        &mut command,
        script.timeout,
        worker::MAX_OUTPUT_BYTES,
        cancel,
    )?;
    Ok(Checkpoint {
        script_hash: crate::payload::fnv64(script.script.as_bytes()),
        input_hash: crate::payload::fnv64(script.input.as_bytes()),
        runtime_fingerprint: format!("gitbash/{version}"),
        workdir_policy: "temp".to_string(),
        result: Some(result),
    })
}

/// `gitbash` 运行时（T112、V-304：Windows PortableGit payload 校验 + Bash 执行）。
#[derive(Debug, Clone)]
pub struct GitBashProvider {
    declaration: RuntimeDeclaration,
    /// PortableGit 根目录或 `git.exe` 路径（T112 语义扩展：二者皆可；`None` 时仅
    /// 回退 PATH 定位 bash——需要确定性时显式配置，见 [`locate_bash`] 失效条件）。
    pub git_path: Option<String>,
}

impl Default for GitBashProvider {
    fn default() -> Self {
        Self {
            declaration: RuntimeDeclaration {
                name: "gitbash".to_string(),
                capability: Capability::ShellOnly,
                unsupported: vec!["任意系统 Shell".to_string()],
            },
            git_path: None,
        }
    }
}

impl GitBashProvider {
    /// 协作式取消执行（FR-705，与 python 运行时同口径）：`cancel` 置位后 worker 在
    /// 下一轮询（≤10 ms）终止子进程；取消与超时在 [`worker::WorkerResult`] 中可区分。
    ///
    /// # Errors
    ///
    /// 请求超限、bash 未定位或子进程启动失败时返回 [`WhError`]。
    pub fn execute_with_cancel_flag(
        &self,
        script: &Script,
        cancel: &AtomicBool,
    ) -> Result<Checkpoint, WhError> {
        self.execute_inner(script, Some(cancel))
    }

    /// 执行主干：请求上限校验（纯内存）→ 并发闸门 → bash 定位 → 受控执行。
    /// 有界校验先于一切子进程动作：超限请求在无 bash 的机器上同样被拒绝。
    fn execute_inner(
        &self,
        script: &Script,
        cancel: Option<&AtomicBool>,
    ) -> Result<Checkpoint, WhError> {
        check_request_size(script)?;
        let _permit = gitbash_gate().acquire();
        let bash = locate_bash(self.git_path.as_deref());
        execute_with_bash(bash.as_deref(), script, cancel)
    }
}

impl RuntimeProvider for GitBashProvider {
    fn name(&self) -> &str {
        "gitbash"
    }
    fn declaration(&self) -> &RuntimeDeclaration {
        &self.declaration
    }
    /// V-304：bash 定位成功即视为可用（候选实测 `--version` 输出含 “bash”）。
    /// 探测统一经 [`worker::execute`]，不携带凭据（§10.2）。
    fn is_available(&self) -> bool {
        locate_bash(self.git_path.as_deref()).is_some()
    }
    /// 环境指纹：`gitbash/<bash 版本摘要>`；bash 不可用或版本探测失败时
    /// `gitbash/unavailable`。
    fn env_fingerprint(&self) -> String {
        locate_bash(self.git_path.as_deref())
            .and_then(|bash| probe_bash_version(&bash))
            .map(|version| format!("gitbash/{version}"))
            .unwrap_or_else(|| "gitbash/unavailable".to_string())
    }
    /// # Errors
    ///
    /// bash 未定位或不可用时返回结构化 `runtime_unavailable`（Reason::Missing，FR-708）。
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

    /// bash 可用性前提：返回定位结果；不可用时 eprintln! 显式说明——前提不满足
    /// 不是静默跳过（与 python 运行时 require_python 同口径）。
    fn require_bash() -> Option<PathBuf> {
        match locate_bash(None) {
            Some(bash) => Some(bash),
            None => {
                eprintln!("本机未定位到 Git Bash：bash 相关用例按前提不满足显式跳过（非静默）");
                None
            }
        }
    }

    /// Check 112/155（payload 半边）：verify_payload 正反四态——正确放行；来源不在
    /// 白名单（permission_denied）、平台不符/版本空/哈希不符（runtime_unavailable）全拒绝。
    #[test]
    fn verify_payload_accepts_good_and_rejects_four_bad_states() {
        let data = b"portablegit-archive-bytes";
        let meta = PayloadMeta {
            source: "https://example.com/portablegit".to_string(),
            version: "2.54.0".to_string(),
            platform: "windows-amd64".to_string(),
            checksum: crate::payload::fnv64(data),
        };
        let allowlist = SourceAllowlist::new(["https://example.com/portablegit"]);
        verify_payload(data, &meta, &allowlist).expect("正确 payload 必须放行");

        // ① 来源不在白名单：permission_denied（不可重试，须更换来源或更新白名单）。
        let mut unknown_source = meta.clone();
        unknown_source.source = "https://evil.example/portablegit".to_string();
        let err = verify_payload(data, &unknown_source, &allowlist).expect_err("来源不明必须拒绝");
        assert_eq!(err.code, whcode_domain::ErrorCode::PermissionDenied);
        assert!(!err.retryable, "来源问题不可重试");
        assert!(
            err.message.contains("gitbash"),
            "错误须注明 gitbash 语境：{err}"
        );

        // ② 平台不符。
        let mut wrong_platform = meta.clone();
        wrong_platform.platform = "linux-amd64".to_string();
        let err = verify_payload(data, &wrong_platform, &allowlist).expect_err("平台不符必须拒绝");
        assert_eq!(err.code, whcode_domain::ErrorCode::RuntimeUnavailable);

        // ③ 版本为空。
        let mut no_version = meta.clone();
        no_version.version = " ".to_string();
        let err = verify_payload(data, &no_version, &allowlist).expect_err("空版本必须拒绝");
        assert_eq!(err.code, whcode_domain::ErrorCode::RuntimeUnavailable);
        assert!(err.message.contains("gitbash"), "{err}");

        // ④ 哈希不符（fnv64 沿用 payload.rs 同算法，意外损坏检测）。
        let err =
            verify_payload(b"tampered-bytes", &meta, &allowlist).expect_err("哈希漂移必须拒绝");
        assert_eq!(err.code, whcode_domain::ErrorCode::RuntimeUnavailable);
        assert!(err.message.contains("gitbash"), "{err}");
    }

    /// Check 112/155（探测半边）：本机已装 Git for Windows——locate_bash 返回真实
    /// 路径且 `--version` 输出含 “bash”（真实证据，不伪造）。
    #[test]
    fn locate_bash_finds_local_git_bash_and_version_contains_bash() {
        let Some(bash) = require_bash() else {
            return;
        };
        eprintln!("locate_bash 实测路径：{}", bash.display());
        let mut command = Command::new(&bash);
        command.arg("--version");
        let result = worker::execute(&mut command, PROBE_TIMEOUT, worker::MAX_OUTPUT_BYTES)
            .expect("版本探测");
        assert_eq!(result.exit_code, Some(0));
        let first_line = result
            .stdout_tail
            .lines()
            .next()
            .expect("版本行")
            .to_string();
        eprintln!("bash --version 实测首行：{first_line}");
        assert!(first_line.contains("bash"), "版本行须含 bash：{first_line}");
    }

    /// `git_path` 语义扩展（PortableGit 根目录或 `git.exe` 路径）：从实测 bash 位置
    /// 推导根目录与 git.exe 候选，再经 locate_bash 反查应同样命中（不硬编码机器路径）。
    #[test]
    fn locate_bash_resolves_from_configured_git_path() {
        let Some(bash) = require_bash() else {
            return;
        };
        // PATH 命中时返回相对名（如 “bash.exe”）；先按 PATH 顺序归一化为绝对路径，
        // 与 locate_bash 的候选语义同源。
        let absolute = if bash.is_absolute() {
            bash
        } else {
            let found = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                .map(|dir| dir.join(&bash))
                .find(|candidate| candidate.is_file());
            let Some(found) = found else {
                eprintln!(
                    "PATH 相对名无法归一化为绝对路径：{}（显式说明，非静默）",
                    bash.display()
                );
                return;
            };
            eprintln!("PATH 归一化：{} → {}", bash.display(), found.display());
            found
        };
        // bash 位于 <root>/bin/ 或 <root>/usr/bin/；根目录候选取其上一级与上两级，
        // 两种布局都覆盖（不假设具体层级）。
        let Some(dir) = absolute.parent() else {
            eprintln!(
                "bash 路径无父目录，无法推导根目录：{}（显式说明，非静默）",
                absolute.display()
            );
            return;
        };
        let mut roots: Vec<PathBuf> = Vec::new();
        if let Some(parent) = dir.parent() {
            roots.push(parent.to_path_buf());
            if let Some(grandparent) = parent.parent() {
                roots.push(grandparent.to_path_buf());
            }
        }
        let mut exercised = 0;
        for root in roots {
            for candidate in [
                root.join("cmd").join("git.exe"),
                root.join("bin").join("git.exe"),
                root.clone(),
            ] {
                if !candidate.exists() {
                    continue;
                }
                let Some(configured) = candidate.to_str() else {
                    continue;
                };
                let located = locate_bash(Some(configured)).expect("配置 git_path 后应能定位 bash");
                eprintln!(
                    "git_path={} → bash={}（实测）",
                    candidate.display(),
                    located.display()
                );
                assert!(
                    located.is_file(),
                    "显式 git_path 命中的必须是真实文件而非 PATH 相对名：{}",
                    located.display()
                );
                let name = located
                    .file_name()
                    .expect("文件名")
                    .to_string_lossy()
                    .to_string();
                assert!(
                    name == "bash.exe" || name == "sh.exe",
                    "命中文件应为 bash.exe/sh.exe：{name}"
                );
                exercised += 1;
            }
        }
        if exercised == 0 {
            eprintln!(
                "bash 位置无法推导出 Git 根布局（可能命中 WSL bash 等）：git_path 反查用例按前提不满足显式跳过（非静默）"
            );
        }
    }

    /// Check 112/155：五类能力探测逐条断言（bash 可在时）——管道输出 “A”、重定向
    /// 读回 “x”、含空格路径执行成功、信号终止由超时终止、目标命令输出含 git。
    #[test]
    fn five_capability_probes_pass_when_bash_available() {
        let Some(bash) = require_bash() else {
            return;
        };
        for (name, script) in capability_probes() {
            let timeout = if *name == "signal_terminate" {
                Duration::from_millis(300)
            } else {
                Duration::from_secs(30)
            };
            let result = run_probe(&bash, script, timeout)
                .unwrap_or_else(|err| panic!("{name} 探测执行失败：{err}"));
            eprintln!(
                "probe {name}: exit={:?} timed_out={} stdout={:?}",
                result.exit_code, result.timed_out, result.stdout_tail
            );
            if *name == "signal_terminate" {
                // 信号终止类：判定标准是被超时终止（kill 信号路径），不是正常退出。
                assert!(result.timed_out, "信号终止类必须由超时路径覆盖：{result:?}");
                assert!(!result.cancelled, "超时不得混同取消：{result:?}");
                assert_eq!(result.exit_code, None);
                continue;
            }
            assert_eq!(result.exit_code, Some(0), "{name} 探测非零退出：{result:?}");
            match *name {
                "pipe" => assert!(
                    result.stdout_tail.trim() == "A",
                    "管道探测须输出 “A”：{:?}",
                    result.stdout_tail
                ),
                "redirect" => assert!(
                    result.stdout_tail.contains('x'),
                    "重定向探测须读回 “x”：{:?}",
                    result.stdout_tail
                ),
                "space_path" => assert!(
                    result.stdout_tail.contains("ok"),
                    "含空格路径探测须执行成功：{:?}",
                    result.stdout_tail
                ),
                "target_command" => assert!(
                    result.stdout_tail.contains("git"),
                    "目标命令探测须输出 git 版本：{:?}",
                    result.stdout_tail
                ),
                other => panic!("未知探测项，清单须与断言同步：{other}"),
            }
        }
    }

    /// Check 112/155（信号终止类）：`bash -c "sleep 30"` 超时 300 ms → 被终止且
    /// `timed_out = true`（不混同取消）。
    #[test]
    fn bash_timeout_terminates_long_running_script() {
        let Some(bash) = require_bash() else {
            return;
        };
        let mut command = Command::new(&bash);
        command.arg("-c").arg("sleep 30");
        let result = worker::execute(
            &mut command,
            Duration::from_millis(300),
            worker::MAX_OUTPUT_BYTES,
        )
        .expect("超时路径执行");
        assert!(result.timed_out, "必须标记超时终止");
        assert!(!result.cancelled, "超时不得混同取消");
        assert_eq!(result.exit_code, None);
    }

    /// Check 112（执行半边）：input 经 stdin 临时文件注入；检查点四要素齐备，
    /// 指纹含 bash 版本摘要、workdir_policy 为 “temp”。
    #[test]
    fn execute_runs_script_with_stdin_input_and_records_checkpoint() {
        let provider = GitBashProvider::default();
        if require_bash().is_none() {
            return;
        }
        let checkpoint = provider
            .execute(&Script {
                script: "tr a-z A-Z".to_string(),
                input: "whcode-bash\n".to_string(),
                timeout: Duration::from_secs(30),
            })
            .expect("bash 执行");
        crate::checkpoint::validate(&checkpoint).expect("检查点四要素齐备");
        assert_eq!(checkpoint.workdir_policy, "temp");
        assert_eq!(
            checkpoint.script_hash,
            crate::payload::fnv64("tr a-z A-Z".as_bytes()),
            "脚本哈希为 fnv64"
        );
        assert_ne!(
            checkpoint.input_hash, checkpoint.script_hash,
            "输入哈希独立"
        );
        assert!(
            checkpoint.runtime_fingerprint.starts_with("gitbash/")
                && checkpoint.runtime_fingerprint != "gitbash/unavailable",
            "指纹须含 bash 版本摘要：{}",
            checkpoint.runtime_fingerprint
        );
        let result = checkpoint.result.expect("结果登记");
        assert_eq!(result.exit_code, Some(0));
        assert!(
            result.stdout_tail.contains("WHCODE-BASH"),
            "stdin 注入须生效：{:?}",
            result.stdout_tail
        );
        provider.clone().terminate().expect("终止为无操作成功");
    }

    /// 负路径（确定性）：bash 未定位时执行主干映射为结构化 `runtime_unavailable`
    /// （reason=missing），不 panic、不静默。直接注入 None，不依赖“本机无 bash”
    /// 的环境前提。
    #[test]
    fn unresolved_bash_maps_to_structured_missing_error() {
        let script = Script {
            script: "echo ok".to_string(),
            input: String::new(),
            timeout: Duration::from_secs(1),
        };
        let err = execute_with_bash(None, &script, None).expect_err("未定位必须结构化错误");
        assert_eq!(err.code, whcode_domain::ErrorCode::RuntimeUnavailable);
        assert_eq!(err.details.as_ref().expect("details")["reason"], "missing");
        assert!(!err.retryable, "缺失不可重试");
    }

    /// 负路径（端到端）：未配置 git_path 且 PATH 无 bash → 结构化错误。本机装有
    /// Git for Windows 时前提不成立，显式说明后跳过（非静默）。
    #[test]
    fn execute_without_configured_path_and_without_path_bash_is_structured_error() {
        let provider = GitBashProvider::default();
        if locate_bash(None).is_some() {
            eprintln!(
                "本机 PATH 可定位 bash：“未配置且 PATH 无 bash”前提不成立，端到端负路径显式跳过（确定性映射见 unresolved_bash_maps_to_structured_missing_error）"
            );
            return;
        }
        assert!(!provider.is_available(), "PATH 无 bash 时不可用");
        let err = provider
            .execute(&Script {
                script: "echo ok".to_string(),
                input: String::new(),
                timeout: Duration::from_secs(1),
            })
            .expect_err("bash 缺失必须结构化错误");
        assert_eq!(err.code, whcode_domain::ErrorCode::RuntimeUnavailable);
        assert_eq!(err.details.as_ref().expect("details")["reason"], "missing");
    }

    /// 可用性与指纹同源同判定（Check 112 与 python Check 111 同口径）；
    /// ShellOnly 声明不宣称 Full（D-08/R-10）。
    #[test]
    fn availability_and_fingerprint_are_consistent() {
        let provider = GitBashProvider::default();
        let fingerprint = provider.env_fingerprint();
        assert_eq!(
            provider.is_available(),
            fingerprint != "gitbash/unavailable",
            "is_available 与指纹必须同源同判定：{fingerprint}"
        );
        if fingerprint != "gitbash/unavailable" {
            assert!(
                fingerprint.starts_with("gitbash/"),
                "指纹格式：{fingerprint}"
            );
        }
        assert_eq!(provider.capability(), Capability::ShellOnly);
        assert!(
            provider
                .declaration()
                .unsupported
                .iter()
                .any(|item| item.contains("任意系统 Shell")),
            "FR-608：任意系统 Shell 永不在能力清单内"
        );
    }

    /// 有界缓冲纪律：超限请求在进入子进程前被纯内存拒绝（bash 是否可用均生效）。
    #[test]
    fn oversized_request_is_rejected_before_spawning() {
        let provider = GitBashProvider::default();
        let err = provider
            .execute(&Script {
                script: "echo ok".to_string(),
                input: "A".repeat(MAX_REQUEST_BYTES + 1),
                timeout: Duration::from_secs(1),
            })
            .expect_err("超限请求必须拒绝");
        assert_eq!(err.code, whcode_domain::ErrorCode::InvalidState);
        assert!(err.message.contains("上限"), "错误须说明上限：{err}");
    }
}
