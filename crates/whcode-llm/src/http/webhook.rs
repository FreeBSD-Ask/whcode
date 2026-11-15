//! # 出站 webhook POST（P6.4／FR-788／ADR-079）
//!
//! 只做一件事：把一段已经准备好的 JSON 正文 POST 到用户配置的端点，并把结果归一成
//! **三分类**（成功／可重试／永久失败）。幂等键由调用方生成并放在 `Idempotency-Key` 头，
//! 接收方可据此去重；本模块不做重试循环（重试语义在 `whcode-app::webhooks`，那里知道
//! `max_attempts` 与去重表，本模块不知道）。
//!
//! 纪律：
//!
//! - 不跟随重定向（`max_redirects(0)`）：跳转可以把请求带到第三主机，而 URL 是用户配置的；
//! - 非 2xx 不作为传输错误（由本模块分类，`4xx` 与 `5xx` 的处理不同）；
//! - 响应正文只读有界字节后丢弃（不解析、不落库、不进日志）；
//! - Bearer 令牌按次传入、不复用、不进 `Debug` 输出与错误文本（ADR-079 决策 1）。
//!
//! 网络栈归属：本模块只在 `http` feature 下编译，启用者是装配根（ADR-031）。

use std::io::Read;
use std::time::Duration;

/// 请求正文字节上限（与 app 侧一致；超出由 app 在构造正文时截断并标 `truncated`）。
pub const MAX_WEBHOOK_BODY_BYTES: usize = 32 * 1024;

/// 响应正文读取上限（读完即丢，只用于释放连接）。
pub const MAX_WEBHOOK_RESPONSE_BYTES: usize = 1024;

/// 一次出站投递请求。
///
/// `Debug` 只打印 URL 与幂等键；`authorization` 恒不输出（它可能含密钥）。
pub struct WebhookRequest<'a> {
    /// 端点 URL（配置校验已保证 https，或回环 http）。
    pub url: &'a str,
    /// JSON 正文（调用方已按 [`MAX_WEBHOOK_BODY_BYTES`] 截断）。
    pub body: &'a str,
    /// 幂等键（`<endpoint>:<event_id>`），写入 `Idempotency-Key` 头。
    pub idempotency_key: &'a str,
    /// 可选的 `Authorization: Bearer` 取值（来自调用方按环境变量求值，按次使用）。
    pub authorization: Option<&'a str>,
    /// 总时限。
    pub timeout: Duration,
}

impl std::fmt::Debug for WebhookRequest<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebhookRequest")
            .field("url", &self.url)
            .field("idempotency_key", &self.idempotency_key)
            .field("bytes", &self.body.len())
            .field("auth", &self.authorization.is_some())
            .finish()
    }
}

/// 投递结果（三分类；ADR-079 决策 4）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebhookOutcome {
    /// 2xx：成功。
    Delivered {
        /// HTTP 状态码。
        status: u16,
    },
    /// 408／429／5xx 或传输失败：可重试。
    Retryable {
        /// HTTP 状态码（传输失败时为 `None`）。
        status: Option<u16>,
        /// 归一化原因（不含请求头与凭据）。
        reason: String,
    },
    /// 其余 4xx：永久失败，不再重试。
    Permanent {
        /// HTTP 状态码。
        status: u16,
        /// 归一化原因。
        reason: String,
    },
}

/// 按 HTTP 状态码分类（纯函数；单测直接钉住边界）。
#[must_use]
pub fn classify_status(status: u16) -> WebhookOutcome {
    match status {
        200..=299 => WebhookOutcome::Delivered { status },
        408 | 429 => WebhookOutcome::Retryable {
            status: Some(status),
            reason: format!("HTTP {status}（临时性错误，可重试）"),
        },
        500..=599 => WebhookOutcome::Retryable {
            status: Some(status),
            reason: format!("HTTP {status}（服务端错误，可重试）"),
        },
        _ => WebhookOutcome::Permanent {
            status,
            reason: format!("HTTP {status}（接收方拒绝，不再重试）"),
        },
    }
}

/// 执行一次 POST，返回三分类结果（不重试、不跟随重定向）。
///
/// # Panics
///
/// 不 panic：URL 非法、TLS 失败、超时都归一成 [`WebhookOutcome::Retryable`] 并给出可读原因。
#[must_use]
pub fn post(request: &WebhookRequest<'_>) -> WebhookOutcome {
    if request.body.len() > MAX_WEBHOOK_BODY_BYTES {
        return WebhookOutcome::Permanent {
            status: 0,
            reason: format!(
                "正文 {:?} 字节超过上限 {MAX_WEBHOOK_BODY_BYTES}：调用方必须先截断",
                request.body.len()
            ),
        };
    }
    let config = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .max_redirects_will_error(true)
        .timeout_global(Some(request.timeout))
        .user_agent(concat!("whcode-webhook/", env!("CARGO_PKG_VERSION")))
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let mut http_request = agent
        .post(request.url)
        .header("Content-Type", "application/json")
        .header("Idempotency-Key", request.idempotency_key);
    if let Some(authorization) = request.authorization {
        http_request = http_request.header("Authorization", authorization);
    }
    match http_request.send(request.body) {
        Ok(response) => {
            let status = response.status().as_u16();
            // 只读有界字节后丢弃：不解析正文，也不让它进入日志与事件。
            let mut sink = Vec::new();
            let _ = response
                .into_body()
                .into_reader()
                .take(MAX_WEBHOOK_RESPONSE_BYTES as u64)
                .read_to_end(&mut sink);
            classify_status(status)
        }
        Err(err) => WebhookOutcome::Retryable {
            status: None,
            reason: format!("传输失败：{}", transport_reason(err)),
        },
    }
}

/// 传输层错误的归一化描述（只用状态类别与简短说明，不回显请求头）。
fn transport_reason(err: ureq::Error) -> String {
    match err {
        ureq::Error::StatusCode(code) => format!("HTTP {code}"),
        ureq::Error::Timeout(_) => "超时".to_string(),
        ureq::Error::Io(io) => format!("I/O：{}", io.kind()),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};

    /// 读到请求头结束（空行）为止：一次 `read` 可能只拿到请求行与部分头。
    fn read_request(stream: &mut TcpStream) -> String {
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let read = stream.read(&mut chunk).expect("读请求");
            if read == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..read]);
            if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        String::from_utf8_lossy(&buffer).to_string()
    }

    /// 写完响应后显式 flush 并稍等：立刻关连接会让客户端读到 reset 而不是状态行。
    fn write_response(stream: &mut TcpStream, response: &str) {
        stream.write_all(response.as_bytes()).expect("写响应");
        stream.flush().expect("刷新响应");
        std::thread::sleep(Duration::from_millis(50));
    }

    /// 分类边界：2xx 成功、408／429／5xx 可重试、其余 4xx 永久失败。
    #[test]
    fn statuses_are_classified_into_three_buckets() {
        for status in [200u16, 202, 204, 299] {
            assert!(matches!(
                classify_status(status),
                WebhookOutcome::Delivered { .. }
            ));
        }
        for status in [408u16, 429, 500, 502, 503, 599] {
            assert!(matches!(
                classify_status(status),
                WebhookOutcome::Retryable { .. }
            ));
        }
        for status in [400u16, 401, 403, 404, 410, 422] {
            assert!(matches!(
                classify_status(status),
                WebhookOutcome::Permanent { .. }
            ));
        }
    }

    /// 正文超上限：调用方必须先截断（本模块拒绝，不静默裁）。
    #[test]
    fn oversized_body_is_rejected_before_any_request() {
        let big = "x".repeat(MAX_WEBHOOK_BODY_BYTES + 1);
        let outcome = post(&WebhookRequest {
            url: "http://127.0.0.1:1/never",
            body: &big,
            idempotency_key: "ci:EV-1",
            authorization: None,
            timeout: Duration::from_millis(50),
        });
        assert!(matches!(
            outcome,
            WebhookOutcome::Permanent { status: 0, .. }
        ));
    }

    /// 回环端点：真实 POST 往返，2xx 判成功并带上幂等键头。
    #[test]
    fn loopback_endpoint_receives_idempotency_key() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("绑定回环端口");
        let address = listener.local_addr().expect("端口地址");
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("接受连接");
            let request = read_request(&mut stream);
            write_response(
                &mut stream,
                "HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
            request
        });
        let outcome = post(&WebhookRequest {
            url: &format!("http://{address}/hook"),
            body: r#"{"event_id":"EV-1"}"#,
            idempotency_key: "ci:EV-1",
            authorization: None,
            timeout: Duration::from_secs(5),
        });
        assert_eq!(
            outcome,
            WebhookOutcome::Delivered { status: 204 },
            "2xx 必须判成功"
        );
        let request = handle.join().expect("服务线程");
        assert!(
            request.to_lowercase().contains("idempotency-key: ci:ev-1"),
            "幂等键必须出现在请求头：{request}"
        );
    }

    /// 4xx 与 5xx 走不同分类（同一回环服务分别返回 422 与 503）。
    #[test]
    fn loopback_status_codes_split_retryable_and_permanent() {
        for (code, expect_retryable) in [(422u16, false), (503, true)] {
            let listener = TcpListener::bind(("127.0.0.1", 0)).expect("绑定回环端口");
            let address = listener.local_addr().expect("端口地址");
            let handle = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().expect("接受连接");
                let _ = read_request(&mut stream);
                let response =
                    format!("HTTP/1.1 {code} X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                write_response(&mut stream, &response);
            });
            let outcome = post(&WebhookRequest {
                url: &format!("http://{address}/hook"),
                body: "{}",
                idempotency_key: "ci:EV-2",
                authorization: None,
                timeout: Duration::from_secs(5),
            });
            handle.join().expect("服务线程");
            match outcome {
                WebhookOutcome::Retryable { status, .. } => {
                    assert!(expect_retryable, "HTTP {code} 不应判可重试");
                    assert_eq!(status, Some(code));
                }
                WebhookOutcome::Permanent { status, .. } => {
                    assert!(!expect_retryable, "HTTP {code} 应判可重试");
                    assert_eq!(status, code);
                }
                WebhookOutcome::Delivered { status } => {
                    panic!("HTTP {code} 不得判成功（status={status}）")
                }
            }
        }
    }
}
