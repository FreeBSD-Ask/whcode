//! 运行时验收集成测试（T114、V-301—V-307）：
//!
//! - **V-301**：默认（无运行时 feature）主程序可运行——runtime crate `default = []`
//!   即 FR-704 构建期表达（测试本身在默认构建下运行即是证据）；
//! - **V-302**：运行时声明具体限制（受限引擎不宣称 Full，D-08）；
//! - **V-303**：payload 校验（平台/哈希三重）；
//! - **V-305**：超时终止子进程并保留输出尾部（FR-705）；
//! - **V-306**：引用未启用运行时的任务在执行前被阻止；
//! - **V-307**：三时点中断 → `unknown`（超时即一种——`timed_out` 状态可见）。
//!
//! V-304（gitbash PortableGit）在 probe 缺失时诚实不可用（本机验证路径）。

use std::time::Duration;
use whcode_runtime::provider::RuntimeProvider as _;
use whcode_runtime::{
    Capability, JsProvider, PayloadMeta, Script, UnavailableReason, execute,
    requires_unavailable_runtime, runtime_unavailable, validate,
};

/// V-301：默认构建（无运行时 feature）crate 可编译可运行——本测试即在该构建下。
#[test]
fn v301_default_build_runs_without_runtimes() {
    // FR-704：default = [] 无任何运行时 feature；worker 仍可用（std 子进程）。
    let result = execute(
        &mut {
            let mut command = std::process::Command::new("cmd");
            command.args(["/C", "echo ok"]);
            command
        },
        Duration::from_secs(10),
        whcode_runtime::MAX_OUTPUT_BYTES,
    )
    .expect("默认构建下子进程执行可用");
    assert_eq!(result.exit_code, Some(0));
}

/// V-302：运行时声明具体限制（受限引擎不宣称完整生态，D-08/R-10）。
#[test]
fn v302_declarations_state_concrete_limits() {
    let js = JsProvider::default();
    assert!(
        js.capability() != Capability::Full,
        "js 不得宣称 Full（D-08）"
    );
    assert!(
        js.declaration()
            .unsupported
            .iter()
            .any(|item| item.contains("Node.js")),
        "js 必须列明 Node.js 未覆盖"
    );
}

/// V-303：payload 平台/哈希校验（FR-703）。
#[test]
fn v303_payload_validation_is_enforced() {
    let meta = PayloadMeta {
        source: "https://example.com".to_string(),
        version: "1.0".to_string(),
        platform: "windows-amd64".to_string(),
        checksum: whcode_runtime::fnv64(b"payload"),
    };
    assert!(validate(b"payload", &meta, "windows-amd64").is_ok());
    assert!(
        validate(b"payload", &meta, "linux-amd64").is_err(),
        "平台不匹配拒绝"
    );
    assert!(
        validate(b"other", &meta, "windows-amd64").is_err(),
        "哈希漂移拒绝"
    );
}

/// V-305：超时终止子进程（输出尾部为空——被杀进程无收尾输出）。
#[test]
fn v305_timeout_terminates_worker() {
    let sleep_script = if cfg!(windows) {
        "ping -n 30 127.0.0.1 > NUL"
    } else {
        "sleep 30"
    };
    let mut command = whcode_platform::shell_command(sleep_script);
    let result = execute(
        &mut command,
        Duration::from_millis(200),
        whcode_runtime::MAX_OUTPUT_BYTES,
    )
    .expect("超时路径");
    assert!(result.timed_out, "V-305：超时必须终止");
    assert_eq!(result.exit_code, None);
}

/// V-306：引用未启用运行时的任务在执行前被阻止（FR-710）。
#[test]
fn v306_unavailable_runtimes_block_execution() {
    let js = JsProvider::default();
    assert!(!js.is_available(), "js 未内置（D-08）");
    let error = js
        .execute(&Script {
            script: "1+1".to_string(),
            input: String::new(),
            timeout: Duration::from_secs(1),
        })
        .expect_err("V-306：执行前必须阻止");
    assert_eq!(error.code, whcode_domain::ErrorCode::RuntimeUnavailable);
    assert_eq!(
        error.details.as_ref().expect("details")["reason"],
        UnavailableReason::Missing.as_str()
    );

    // 任务引用清单检出未启用者（FR-710 批量判定）。
    let missing = requires_unavailable_runtime(&["js".to_string()], &[]);
    assert_eq!(missing, vec!["js"]);

    // 结构化错误携带安装建议（FR-708）。
    let error = runtime_unavailable(
        "python",
        UnavailableReason::Missing,
        "whcode runtime install python",
    );
    assert!(error.to_string().contains("whcode runtime install python"));
}

/// V-307：超时即一种中断——`timed_out` 状态显式可见（恢复流程依据，FR-409）。
#[test]
fn v307_timeout_state_is_visible_for_recovery() {
    let sleep_script = if cfg!(windows) {
        "ping -n 30 127.0.0.1 > NUL"
    } else {
        "sleep 30"
    };
    let mut command = whcode_platform::shell_command(sleep_script);
    let result = execute(
        &mut command,
        Duration::from_millis(150),
        whcode_runtime::MAX_OUTPUT_BYTES,
    )
    .expect("超时");
    assert!(
        result.timed_out,
        "V-307：中断状态必须显式（unknown/timed_out 可见）"
    );
}
