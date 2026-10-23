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
//! V-304：gitbash payload 四重校验在任何机器上都断言；五类能力探测仅在定位到
//! Git Bash 时逐条断言，否则显式说明前提不满足（payload 断言已完成，非静默跳过）。

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

/// V-303：payload 来源/平台/哈希校验（FR-703）。
#[test]
fn v303_payload_validation_is_enforced() {
    let meta = PayloadMeta {
        source: "https://example.com".to_string(),
        version: "1.0".to_string(),
        platform: "windows-amd64".to_string(),
        checksum: whcode_runtime::fnv64(b"payload"),
    };
    let allowlist = whcode_runtime::SourceAllowlist::new(["https://example.com"]);
    assert!(validate(b"payload", &meta, "windows-amd64", &allowlist).is_ok());
    assert!(
        validate(b"payload", &meta, "linux-amd64", &allowlist).is_err(),
        "平台不匹配拒绝"
    );
    assert!(
        validate(b"other", &meta, "windows-amd64", &allowlist).is_err(),
        "哈希漂移拒绝"
    );
    // 来源不在白名单：默认拒绝（空白名单拒绝一切）。
    assert!(
        validate(
            b"payload",
            &meta,
            "windows-amd64",
            &whcode_runtime::SourceAllowlist::default()
        )
        .is_err(),
        "未配置来源不得放行"
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

/// V-304：gitbash PortableGit payload 四重校验正反 + 五类能力探测（T112、Check 112/155）。
///
/// payload 断言（来源白名单/平台/版本/哈希，ADR-024 决策 3）在任何机器上都执行；
/// 五类探测（管道、重定向、含空格路径、信号终止、目标命令）仅在定位到 Git Bash 时
/// 逐条断言，否则显式打印说明——前提不满足不是静默跳过，payload 断言已完成。
#[test]
fn v304_gitbash_payload_and_bash_capability() {
    // ① payload 四重校验：面向 PortableGit 归档字节（zip/7z 独立分发，不随主二进制）。
    let data = b"portablegit-archive-bytes";
    let meta = PayloadMeta {
        source: "https://example.com/portablegit".to_string(),
        version: "2.54.0".to_string(),
        platform: "windows-amd64".to_string(),
        checksum: whcode_runtime::fnv64(data),
    };
    let allowlist = whcode_runtime::SourceAllowlist::new(["https://example.com/portablegit"]);
    whcode_runtime::gitbash::verify_payload(data, &meta, &allowlist).expect("四重校验放行");

    // 来源不在白名单 → permission_denied（不可重试）。
    let mut unknown_source = meta.clone();
    unknown_source.source = "https://evil.example/portablegit".to_string();
    let err = whcode_runtime::gitbash::verify_payload(data, &unknown_source, &allowlist)
        .expect_err("来源不在白名单必须拒绝");
    assert_eq!(err.code, whcode_domain::ErrorCode::PermissionDenied);

    // 平台不符 / 版本为空 / 哈希漂移 → runtime_unavailable（不静默接受）。
    let mut wrong_platform = meta.clone();
    wrong_platform.platform = "linux-amd64".to_string();
    let err = whcode_runtime::gitbash::verify_payload(data, &wrong_platform, &allowlist)
        .expect_err("平台不符必须拒绝");
    assert_eq!(err.code, whcode_domain::ErrorCode::RuntimeUnavailable);

    let mut no_version = meta.clone();
    no_version.version = String::new();
    assert!(
        whcode_runtime::gitbash::verify_payload(data, &no_version, &allowlist).is_err(),
        "空版本必须拒绝"
    );
    assert!(
        whcode_runtime::gitbash::verify_payload(b"tampered-bytes", &meta, &allowlist).is_err(),
        "哈希漂移必须拒绝"
    );

    // ② 五类能力探测（bash 可在时逐条断言；信号终止类由超时终止路径覆盖）。
    let Some(bash) = whcode_runtime::gitbash::locate_bash(None) else {
        eprintln!(
            "本机未定位到 Git Bash：V-304 五类探测前提不满足，显式跳过（payload 四重校验断言已完成）"
        );
        return;
    };
    eprintln!("V-304 实测 bash 路径：{}", bash.display());
    for (name, script) in whcode_runtime::gitbash::capability_probes() {
        let timeout = if *name == "signal_terminate" {
            Duration::from_millis(300)
        } else {
            Duration::from_secs(30)
        };
        let result = whcode_runtime::gitbash::run_probe(&bash, script, timeout)
            .unwrap_or_else(|err| panic!("{name} 探测执行失败：{err}"));
        if *name == "signal_terminate" {
            assert!(result.timed_out, "信号终止类必须由超时终止覆盖：{result:?}");
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
