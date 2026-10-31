//! 测试夹具：最小 MCP 服务器（仅测试与本地排障用，不是产品代码；ADR-054 §5）。
//!
//! 用法：`mcp-test-server <mode>`，mode ∈ {echo, hang, spam}：
//! - `echo`：initialize/tools/list/tools/call 正常应答（echo_text 回显、big_output
//!   输出 200 KiB、其他工具 isError=true）；
//! - `hang`：initialize 正常应答后对一切后续请求不响应（超时看门狗测试）；
//! - `spam`：tools/call 应答前先写一条通知与一个错误 id 的响应（外来消息容忍测试）。
//!
//! 服务器端只读 stdin 行、写 stdout 行；stdin 关闭即退出（配合客户端优雅关闭）。

use std::io::{BufRead, Write};
use std::time::Duration;

fn main() {
    let mode = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "echo".to_string());
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let Some(id) = value.get("id").and_then(|v| v.as_u64()) else {
            continue; // 通知：服务器端无需处理
        };
        let Some(method) = value.get("method").and_then(|v| v.as_str()) else {
            continue;
        };
        if mode == "spam" && method != "initialize" {
            // 先写外来消息：一条通知 + 一个错误 id 的响应，再走正常应答。
            let _ = writeln!(
                out,
                r#"{{"jsonrpc":"2.0","method":"notifications/message","params":{{}}}}"#
            );
            let _ = writeln!(out, r#"{{"jsonrpc":"2.0","id":99999,"result":{{}}}}"#);
            let _ = out.flush();
        }
        let response = match (mode.as_str(), method) {
            (_, "initialize") => serde_json::json!({
                "jsonrpc": "2.0", "id": id,
                "result": {
                    "protocolVersion": value["params"]["protocolVersion"].clone(),
                    "capabilities": {},
                    "serverInfo": {"name": format!("test-{mode}"), "version": "0.0.1"},
                },
            }),
            ("hang", _) => {
                // 不响应：睡到被看门狗终止。
                loop {
                    std::thread::sleep(Duration::from_secs(3600));
                }
            }
            (_, "tools/list") => serde_json::json!({
                "jsonrpc": "2.0", "id": id,
                "result": {"tools": [
                    {"name": "echo_text", "description": "回显文本",
                     "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}},
                    {"name": "big_output", "description": "输出 200KiB 文本",
                     "inputSchema": {"type": "object"}},
                ]},
            }),
            (_, "tools/call") => {
                let name = value["params"]["name"].as_str().unwrap_or_default();
                let echo_text = if mode == "spam" {
                    // spam 模式：可区分的正常应答（外来消息已在上面先行写入）。
                    "spam-ok".to_string()
                } else {
                    value["params"]["arguments"]["text"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                };
                if name == "echo_text" {
                    let text = echo_text;
                    serde_json::json!({
                        "jsonrpc": "2.0", "id": id,
                        "result": {"content": [{"type": "text", "text": text}], "isError": false},
                    })
                } else if name == "big_output" {
                    serde_json::json!({
                        "jsonrpc": "2.0", "id": id,
                        "result": {"content": [{"type": "text", "text": "x".repeat(200 * 1024)}], "isError": false},
                    })
                } else {
                    serde_json::json!({
                        "jsonrpc": "2.0", "id": id,
                        "result": {"content": [{"type": "text", "text": "未知工具"}], "isError": true},
                    })
                }
            }
            _ => serde_json::json!({
                "jsonrpc": "2.0", "id": id,
                "error": {"code": -32601, "message": "fixture 未实现该方法"},
            }),
        };
        if writeln!(out, "{response}").is_err() || out.flush().is_err() {
            break;
        }
    }
}
