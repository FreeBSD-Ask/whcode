//! MCP Streamable HTTP fixture（FR-738、ADR-055 §2.2）：单端点 POST + 会话头 + DELETE，
//! 无 SSE。模式：
//! - `echo`：完整握手/list/call，call 回显 `arguments.text`；
//! - `hang`：握手 OK 后 tools/call 挂起（超时路径）；
//! - `nosession`：initialize 不返回 `Mcp-Session-Id`（后续 POST 无会话头 → 404）；
//! - `auth`：initialize 返回 401（鉴权路径）。
//!
//! 非 initialize 的 POST 必须带 `Mcp-Session-Id`（否则 404）；`notifications/initialized`
//! 返回 202；DELETE 返回 200。每连接一线程（真实服务器并发；hang 不阻塞 DELETE）。
//!
//! 启动后向 stdout 打印 `URL=http://127.0.0.1:<port>/mcp`（测试读取以构造客户端）。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};

const SESSION_ID: &str = "fixture-session-1";
const PROTOCOL: &str = "2025-06-18";

fn main() {
    let mode = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "echo".to_string());
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1:0");
    let port = listener.local_addr().expect("local_addr").port();
    println!("URL=http://127.0.0.1:{port}/mcp");
    let _ = std::io::stdout().flush();

    for stream in listener.incoming() {
        let mut stream = match stream {
            Ok(s) => s,
            Err(_) => continue,
        };
        let mode = mode.clone();
        let _ = std::thread::spawn(move || {
            let _ = handle(&mode, &mut stream);
        });
    }
}

fn handle(mode: &str, stream: &mut TcpStream) -> std::io::Result<()> {
    // 读请求行 + 头（克隆流读、原流写，共享同一套接字）。
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let mut content_length = 0usize;
    let mut has_session = false;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        if line == "\r\n" || line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim();
            let val = v.trim();
            if key.eq_ignore_ascii_case("content-length") {
                content_length = val.parse().unwrap_or(0);
            }
            if key.eq_ignore_ascii_case("mcp-session-id") && val == SESSION_ID {
                has_session = true;
            }
        }
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body)?;
    }
    let body_str = String::from_utf8_lossy(&body).to_string();

    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        return Ok(());
    }
    let method = parts[0];
    let json: serde_json::Value =
        serde_json::from_str(&body_str).unwrap_or(serde_json::Value::Null);
    let method_name = json.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let id = json.get("id").cloned().unwrap_or(serde_json::Value::Null);

    if method == "DELETE" {
        return respond(stream, 200, "{}", None);
    }
    if method != "POST" {
        return respond(stream, 405, "{}", None);
    }
    if mode == "hang" && method_name == "tools/call" {
        // 挂起：不响应（客户端超时/取消路径）。
        std::thread::sleep(std::time::Duration::from_secs(60));
        return Ok(());
    }
    match method_name {
        "initialize" => {
            if mode == "auth" {
                return respond(stream, 401, "{}", None);
            }
            let result = serde_json::json!({
                "protocolVersion": PROTOCOL,
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "http-test", "version": "0.1.0"},
            });
            let resp = json_response(&id, Some(result), None);
            let session = if mode == "nosession" {
                None
            } else {
                Some(SESSION_ID)
            };
            respond(stream, 200, &resp, session)
        }
        "notifications/initialized" => respond(stream, 202, "", None),
        "tools/list" => {
            if !has_session {
                return respond(stream, 404, "{}", None);
            }
            let result = serde_json::json!({
                "tools": [
                    {"name": "echo_text", "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}},
                    {"name": "big_output", "inputSchema": {"type": "object"}},
                ],
            });
            let resp = json_response(&id, Some(result), None);
            respond(stream, 200, &resp, None)
        }
        "tools/call" => {
            if !has_session {
                return respond(stream, 404, "{}", None);
            }
            let text = json
                .get("params")
                .and_then(|p| p.get("arguments"))
                .and_then(|a| a.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or("http-ok")
                .to_string();
            let result = serde_json::json!({
                "content": [{"type": "text", "text": text}],
            });
            let resp = json_response(&id, Some(result), None);
            respond(stream, 200, &resp, None)
        }
        _ => {
            let resp = json_response(&id, None, Some(-32601));
            respond(stream, 200, &resp, None)
        }
    }
}

fn json_response(
    id: &serde_json::Value,
    result: Option<serde_json::Value>,
    code: Option<i64>,
) -> String {
    let mut obj = serde_json::Map::new();
    obj.insert("jsonrpc".into(), serde_json::Value::String("2.0".into()));
    obj.insert("id".into(), id.clone());
    match (result, code) {
        (Some(r), _) => {
            obj.insert("result".into(), r);
        }
        (None, Some(c)) => {
            obj.insert(
                "error".into(),
                serde_json::json!({"code": c, "message": "method not found"}),
            );
        }
        (None, None) => {}
    }
    serde_json::Value::Object(obj).to_string()
}

fn respond(
    stream: &mut TcpStream,
    status: u16,
    body: &str,
    session: Option<&str>,
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        500 => "Internal Server Error",
        _ => "Error",
    };
    let mut resp = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    if let Some(sid) = session {
        resp.push_str(&format!("Mcp-Session-Id: {sid}\r\n"));
    }
    resp.push_str("\r\n");
    resp.push_str(body);
    stream.write_all(resp.as_bytes())
}
