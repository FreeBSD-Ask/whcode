//! MCP 会话生命周期集成测试（FR-736、ADR-054 §5；fixture = `mcp-test-server`）。
//!
//! 覆盖：initialize 握手、tools/list、tools/call（含 isError/big_output 截断）、
//! 超时看门狗终止进程树、取消感知、外来消息容忍、未握手守卫、Drop 后零残留。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use whcode_domain::ErrorCode;
use whcode_mcp::{CancelFlag, McpServerConfig, McpSession};

const TIMEOUT: Duration = Duration::from_secs(10);

fn fixture_config(mode: &str) -> McpServerConfig {
    McpServerConfig {
        name: "test".to_string(),
        command: env!("CARGO_BIN_EXE_mcp-test-server").to_string(),
        url: None,
        args: vec![mode.to_string()],
        env: Default::default(),
        timeout_ms: 30_000,
        max_output_bytes: 64 * 1024,
        enabled: true,
    }
}

/// 轮询断言进程已消失（杀树后 tasklist 收敛需要短暂时间；`Err` 不当"不存在"）。
fn assert_process_gone(pid: u32) {
    for _ in 0..100 {
        match whcode_platform::process::process_exists(pid) {
            Ok(false) => return,
            Ok(true) => std::thread::sleep(Duration::from_millis(20)),
            Err(err) => panic!("进程存在性查询失败：{err}"),
        }
    }
    panic!("进程 {pid} 在 2s 内未消失：零残留判据失败");
}

#[test]
fn initialize_tools_list_and_call_roundtrip() {
    let config = fixture_config("echo");
    let mut session = McpSession::spawn(&config).expect("spawn");
    let info = session.initialize(TIMEOUT, None).expect("握手");
    assert_eq!(info.server_name, "test-echo");
    assert_eq!(info.protocol_version, whcode_mcp::PROTOCOL_VERSION);
    assert!(!session.is_broken());

    let listing = session.list_tools(TIMEOUT, None).expect("tools/list");
    assert!(!listing.truncated);
    let names: Vec<&str> = listing.tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["echo_text", "big_output"], "保持服务器声明顺序");

    let call = session
        .call_tool(
            "echo_text",
            &serde_json::json!({"text": "你好 MCP"}),
            TIMEOUT,
            None,
            64 * 1024,
        )
        .expect("tools/call");
    assert!(!call.is_error && !call.truncated);
    assert_eq!(call.text, "你好 MCP");
    assert_eq!(call.non_text_count, 0);

    // 服务器声明 isError=true 的调用：结果带 is_error 标记而非 Err（不伪造成功）。
    let call = session
        .call_tool(
            "missing_tool",
            &serde_json::json!({}),
            TIMEOUT,
            None,
            64 * 1024,
        )
        .expect("未知工具也返回结构化结果");
    assert!(call.is_error);

    session.shutdown();
}

#[test]
fn big_output_is_truncated_with_marker() {
    let config = fixture_config("echo");
    let mut session = McpSession::spawn(&config).expect("spawn");
    session.initialize(TIMEOUT, None).expect("握手");
    let call = session
        .call_tool(
            "big_output",
            &serde_json::json!({}),
            TIMEOUT,
            None,
            64 * 1024,
        )
        .expect("调用成功");
    assert!(call.truncated, "200KiB 输出必须显式截断标记");
    assert!(
        call.text.len() <= 64 * 1024,
        "截断后不得超过上限：{}",
        call.text.len()
    );
    session.shutdown();
}

#[test]
fn timeout_watchdog_terminates_tree_with_zero_residue() {
    let config = fixture_config("hang");
    let mut session = McpSession::spawn(&config).expect("spawn");
    let info = session.initialize(TIMEOUT, None).expect("握手成功");
    let pid = session.pid();
    assert_eq!(info.server_name, "test-hang");

    let started = std::time::Instant::now();
    let err = session
        .call_tool(
            "echo_text",
            &serde_json::json!({"text": "x"}),
            Duration::from_millis(300),
            None,
            64 * 1024,
        )
        .expect_err("挂起服务器必须在超时后失败");
    let elapsed = started.elapsed();
    assert_eq!(err.code, ErrorCode::Timeout, "归因超时：{err}");
    assert!(
        (300..5000).contains(&elapsed.as_millis()),
        "看门狗应在 deadline 附近触发，实际 {elapsed:?}"
    );
    assert!(session.is_broken(), "超时后会话必须标记失效");

    // 看门狗已杀树；Drop reap 后零残留（ADR-054 §5 判据）。
    drop(session);
    assert_process_gone(pid);
}

#[test]
fn cancel_flag_stops_in_flight_call() {
    let config = fixture_config("hang");
    let mut session = McpSession::spawn(&config).expect("spawn");
    session.initialize(TIMEOUT, None).expect("握手");
    let pid = session.pid();

    // 并行线程 150ms 后置位取消；调用侧超时 30s——取消必须先于超时生效。
    let flag = Arc::new(AtomicBool::new(false));
    let cancel = CancelFlag::from_shared(Arc::clone(&flag));
    let setter = {
        let flag = Arc::clone(&flag);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            flag.store(true, Ordering::SeqCst);
        })
    };

    let started = std::time::Instant::now();
    session
        .call_tool(
            "echo_text",
            &serde_json::json!({"text": "x"}),
            Duration::from_secs(30),
            Some(cancel),
            64 * 1024,
        )
        .expect_err("取消必须终止调用");
    assert!(session.is_broken(), "取消后会话必须标记失效");
    setter.join().expect("setter 线程");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "取消应远早于 30s 超时，实际 {:?}",
        started.elapsed()
    );
    drop(session);
    assert_process_gone(pid);
}

#[test]
fn spam_mode_tolerates_foreign_messages() {
    let config = fixture_config("spam");
    let mut session = McpSession::spawn(&config).expect("spawn");
    session.initialize(TIMEOUT, None).expect("握手");
    let call = session
        .call_tool(
            "echo_text",
            &serde_json::json!({"text": "s"}),
            TIMEOUT,
            None,
            64 * 1024,
        )
        .expect("外来消息被计数丢弃后正常配对");
    assert_eq!(call.text, "spam-ok");
    assert!(
        session.notifications_seen() >= 1,
        "通知必须被计数（不静默吞掉）：{}",
        session.notifications_seen()
    );
    session.shutdown();
}

#[test]
fn calls_before_initialize_are_rejected() {
    let config = fixture_config("echo");
    let mut session = McpSession::spawn(&config).expect("spawn");
    let err = session
        .list_tools(TIMEOUT, None)
        .expect_err("未握手必须拒绝");
    assert_eq!(err.code, ErrorCode::InvalidState);
    session.shutdown();
}
