//! MCP Streamable HTTP 传输集成测试（FR-738、ADR-055 §2.2；fixture = `http-test-server`）。
//!
//! 覆盖：initialize 握手 + 会话头往返、tools/list、tools/call、超时（ureq 请求超时）、
//! 预置取消（写前置检查）、无会话头 404、401 鉴权、shutdown 后失效。
//!
//! 关键判据：fixture 对非 initialize 的 POST 强制要求 `Mcp-Session-Id`（否则 404）——
//! 因此完整往返成功即证明会话头已被客户端捕获并回带。

use std::io::{BufRead, BufReader};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use serde_json::json;
use whcode_domain::ErrorCode;
use whcode_mcp::{CancelFlag, McpServerConfig, McpSession};

const TIMEOUT: Duration = Duration::from_secs(10);

/// fixture 子进程守卫：测试结束杀子进程（`Drop` 级联，防泄漏）。
struct Fixture {
    child: std::process::Child,
    url: String,
}

impl Fixture {
    fn spawn(mode: &str) -> Self {
        let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_http-test-server"))
            .arg(mode)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("启动 http-test-server");
        let stdout = child.stdout.take().expect("fixture stdout");
        let mut reader = BufReader::new(stdout);
        let mut url_line = String::new();
        reader
            .read_line(&mut url_line)
            .expect("读取 fixture URL 行");
        let url = url_line
            .trim()
            .strip_prefix("URL=")
            .expect("URL= 前缀")
            .to_string();
        Self { child, url }
    }

    fn url(&self) -> &str {
        &self.url
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn http_config(url: &str) -> McpServerConfig {
    McpServerConfig {
        name: "http-test".to_string(),
        command: String::new(),
        url: Some(url.to_string()),
        args: vec![],
        env: Default::default(),
        timeout_ms: 30_000,
        max_output_bytes: 64 * 1024,
        enabled: true,
    }
}

#[test]
fn http_initialize_list_call_roundtrip() {
    let fixture = Fixture::spawn("echo");
    let config = http_config(fixture.url());
    let mut session = McpSession::connect(&config).expect("connect");
    let info = session.initialize(TIMEOUT, None).expect("握手");
    assert_eq!(info.server_name, "http-test");
    assert_eq!(info.protocol_version, whcode_mcp::PROTOCOL_VERSION);
    assert!(!session.is_broken());

    // fixture 对非 initialize 强制会话头；能通过即证明会话头往返成功。
    let listing = session.list_tools(TIMEOUT, None).expect("tools/list");
    assert!(!listing.truncated);
    let names: Vec<&str> = listing.tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["echo_text", "big_output"], "保持服务器声明顺序");

    let call = session
        .call_tool(
            "echo_text",
            &json!({"text": "你好 HTTP"}),
            TIMEOUT,
            None,
            64 * 1024,
        )
        .expect("tools/call");
    assert!(!call.is_error && !call.truncated);
    assert_eq!(call.text, "你好 HTTP");
    assert_eq!(call.non_text_count, 0);
    session.shutdown();
}

#[test]
fn http_timeout_marks_session_broken() {
    let fixture = Fixture::spawn("hang");
    let config = http_config(fixture.url());
    let mut session = McpSession::connect(&config).expect("connect");
    session.initialize(TIMEOUT, None).expect("握手");

    let started = Instant::now();
    let err = session
        .call_tool(
            "echo_text",
            &json!({"text": "x"}),
            Duration::from_millis(300),
            None,
            64 * 1024,
        )
        .expect_err("挂起服务器必须在请求超时后失败");
    assert_eq!(err.code, ErrorCode::Timeout, "归因超时：{err}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "ureq 请求超时应生效，实际 {:?}",
        started.elapsed()
    );
    assert!(session.is_broken(), "超时后会话必须标记失效");
    session.shutdown();
}

#[test]
fn http_pre_cancelled_request_is_rejected() {
    let fixture = Fixture::spawn("echo");
    let config = http_config(fixture.url());
    let mut session = McpSession::connect(&config).expect("connect");
    session.initialize(TIMEOUT, None).expect("握手");

    // 预置取消：写前置检查拒绝（阻塞中的 ureq 不可中断，ADR-055 §2.5）。
    let flag = Arc::new(AtomicBool::new(true));
    let cancel = CancelFlag::from_shared(flag);
    let err = session
        .call_tool(
            "echo_text",
            &json!({"text": "x"}),
            Duration::from_secs(30),
            Some(cancel),
            64 * 1024,
        )
        .expect_err("预置取消必须拒绝");
    assert_eq!(err.code, ErrorCode::RuntimeUnavailable, "归因取消：{err}");
    assert!(session.is_broken(), "取消后会话必须标记失效");
    session.shutdown();
}

#[test]
fn http_missing_session_id_is_reported_as_session_invalid() {
    let fixture = Fixture::spawn("nosession");
    let config = http_config(fixture.url());
    let mut session = McpSession::connect(&config).expect("connect");
    // initialize 无会话头也成功（服务器不要求）；后续 POST 无头 → 404。
    session.initialize(TIMEOUT, None).expect("握手（无会话头）");
    let err = session.list_tools(TIMEOUT, None).expect_err("无会话头→404");
    assert_eq!(err.code, ErrorCode::RuntimeUnavailable, "归因 404：{err}");
    assert!(err.message.contains("会话已失效"), "指名会话失效：{err}");
    session.shutdown();
}

#[test]
fn http_auth_challenge_is_reported() {
    let fixture = Fixture::spawn("auth");
    let config = http_config(fixture.url());
    let mut session = McpSession::connect(&config).expect("connect");
    let err = session
        .initialize(TIMEOUT, None)
        .expect_err("401 必须显式报错");
    assert_eq!(err.code, ErrorCode::RuntimeUnavailable, "归因 401：{err}");
    assert!(err.message.contains("鉴权"), "指名鉴权：{err}");
}

#[test]
fn http_shutdown_breaks_session_and_blocks_calls() {
    let fixture = Fixture::spawn("echo");
    let config = http_config(fixture.url());
    let mut session = McpSession::connect(&config).expect("connect");
    session.initialize(TIMEOUT, None).expect("握手");
    session.shutdown();
    assert!(session.is_broken(), "shutdown 后会话失效");
    let err = session.list_tools(TIMEOUT, None).expect_err("关闭后拒绝");
    assert_eq!(err.code, ErrorCode::InvalidState, "归因失效：{err}");
}
