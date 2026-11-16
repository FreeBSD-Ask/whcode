//! # LSP 工具装配（P5.3b／FR-783／ADR-078）
//!
//! 配置＝`<workspace>/.whcode/lsp.json`（显式 opt-in、无自动发现，与 `.whcode/mcp.json`／
//! `.whcode/shell.json` 同模式）。本模块把 [`whcode_lsp::LspClient`] 适配成
//! [`whcode_tools::LspQueryExecutor`]：每次查询按扩展名挑服务器、新建客户端、查完即弃
//! （客户端按查询期存在，非会话常驻，见 ADR-078 决策 4）。
//!
//! 与 shell 工具的差异（审批口径）：这里的**命令与参数来自用户配置文件**，模型只能给
//! 文件与位置，故四个工具都是只读、无需审批；shell 的脚本文本由模型给出，必须审批。
//!
//! 配置写错**显式失败**（不静默当“未配置”）；未配置＝`Ok(None)`（不注册四工具）。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use whcode_domain::{ErrorCode, WhError};
use whcode_lsp::{LspClient, ServerConfig};
use whcode_tools::{
    DiagnosticView, LocationView, LspQueryExecutor, LspQueryKind, LspQueryOutcome, LspQueryRequest,
    LspQueryResult,
};

/// 构造 LSP 执行器：未配置服务器时返回 `Ok(None)`（FR-704 口径：默认不注册）。
///
/// # Errors
///
/// 配置文件非法（坏 JSON／超限／字段非法）时返回显式错误——不静默降级。
pub fn build_executor(workspace: &Path) -> Result<Option<Arc<dyn LspQueryExecutor>>, WhError> {
    let servers = whcode_lsp::load(workspace)?;
    if servers.is_empty() {
        return Ok(None);
    }
    Ok(Some(Arc::new(LspToolExecutor {
        servers,
        workspace: workspace.to_path_buf(),
    })))
}

/// 按查询期启动客户端的 LSP 执行器。
struct LspToolExecutor {
    servers: Vec<ServerConfig>,
    workspace: PathBuf,
}

impl LspQueryExecutor for LspToolExecutor {
    fn query(&self, request: &LspQueryRequest) -> Result<LspQueryOutcome, WhError> {
        let extension = whcode_lsp::extension_of(&request.path);
        let server = whcode_lsp::for_extension(&self.servers, &extension).ok_or_else(|| {
            let configured: Vec<&str> = self
                .servers
                .iter()
                .flat_map(|server| server.extensions.iter().map(String::as_str))
                .collect();
            WhError::new(
                ErrorCode::NotSupported,
                format!(
                    "无 LSP 服务器负责扩展名 `{extension}`：在 .whcode/lsp.json 增加该扩展名\
                     （当前已配置：{}）",
                    if configured.is_empty() {
                        "无".to_string()
                    } else {
                        configured.join("、")
                    }
                ),
            )
        })?;
        let mut client = LspClient::start(server, &self.workspace)?;
        let outcome = match request.kind {
            LspQueryKind::Definition => {
                let bounded = client.definition(&request.path, request.line, request.character)?;
                LspQueryOutcome {
                    result: LspQueryResult::Locations(
                        bounded.items.iter().map(location_view).collect(),
                    ),
                    truncated: bounded.truncated,
                }
            }
            LspQueryKind::References => {
                let bounded = client.references(
                    &request.path,
                    request.line,
                    request.character,
                    request.include_declaration,
                )?;
                LspQueryOutcome {
                    result: LspQueryResult::Locations(
                        bounded.items.iter().map(location_view).collect(),
                    ),
                    truncated: bounded.truncated,
                }
            }
            LspQueryKind::Hover => {
                let bounded = client.hover(&request.path, request.line, request.character)?;
                LspQueryOutcome {
                    result: LspQueryResult::Hover(bounded.items.into_iter().next()),
                    truncated: bounded.truncated,
                }
            }
            LspQueryKind::Diagnostics => {
                let bounded = client.diagnostics(&request.path)?;
                LspQueryOutcome {
                    result: LspQueryResult::Diagnostics(
                        bounded
                            .items
                            .into_iter()
                            .map(|item| DiagnosticView {
                                line: item.line,
                                character: item.character,
                                severity: item.severity,
                                source: item.source,
                                code: item.code,
                                message: item.message,
                            })
                            .collect(),
                    ),
                    truncated: bounded.truncated,
                }
            }
        };
        // 客户端按查询期存在：显式 drop 以完成 shutdown→kill→join（ADR-078 决策 4）。
        drop(client);
        Ok(outcome)
    }
}

fn location_view(location: &whcode_lsp::LspLocation) -> LocationView {
    LocationView {
        path: path_from_uri(&location.uri),
        line: location.start_line,
        character: location.start_character,
        end_line: location.end_line,
        end_character: location.end_character,
    }
}

/// `file://` URI → 本机显示路径（百分号解码＋平台归一）。
#[must_use]
pub fn path_from_uri(uri: &str) -> String {
    let body = uri.strip_prefix("file://").unwrap_or(uri);
    whcode_platform::paths::display_path_from_uri(&percent_decode(body))
}

/// 最小百分号解码（URI 只用到 `%XX`；非法编码按字面保留，不 panic）。
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
        {
            out.push(high * 16 + low);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 未配置＝`Ok(None)`；坏配置＝显式错误（不静默当未配置）。
    #[test]
    fn absent_and_invalid_config_are_distinguished() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let dir = std::env::temp_dir().join(format!(
            "whcode-lsp-assembly-{}-{stamp}",
            std::process::id()
        ));
        std::fs::create_dir_all(dir.join(".whcode")).expect("建夹具");

        assert!(
            build_executor(&dir).expect("未配置").is_none(),
            "无配置文件必须返回 None"
        );

        std::fs::write(dir.join(".whcode").join("lsp.json"), b"{ not json }").expect("写坏配置");
        let err = build_executor(&dir).err().expect("坏配置必须失败");
        assert_eq!(err.code, ErrorCode::ParseError);

        std::fs::write(
            dir.join(".whcode").join("lsp.json"),
            r#"{"servers":[{"id":"ra","command":"rust-analyzer","extensions":[".rs"]}]}"#,
        )
        .expect("写配置");
        assert!(build_executor(&dir).expect("合法配置").is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// URI → 本机路径：解码空格与非 ASCII、去 `file://` 前缀。
    ///
    /// 断言与路径分隔符无关：FreeBSD 上本用例曾因硬编码 `\` 而红（run `43667118984`）。
    /// 平台归一由 `whcode_platform::paths` 的测试负责，这里只验证解码与去前缀。
    #[test]
    fn uri_is_decoded_to_native_path() {
        let decoded = path_from_uri("file:///D:/a%20b/lib.rs").replace('\\', "/");
        assert!(decoded.ends_with("a b/lib.rs"), "{decoded}");
        let unicode = percent_decode("%E7%9B%AE%E5%BD%95/x.rs");
        assert_eq!(unicode, "目录/x.rs");
        // 非法编码按字面保留（不 panic、不吞字符）。
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
    }

    /// 位置结果映射到视图：0 基坐标原样保留（渲染层负责 +1）。
    #[test]
    fn location_views_keep_zero_based_coordinates() {
        let location = whcode_lsp::LspLocation {
            uri: "file:///D:/ws/src/lib.rs".to_string(),
            start_line: 4,
            start_character: 2,
            end_line: 4,
            end_character: 9,
        };
        let view = location_view(&location);
        // 分隔符归一后再断言（同上一个用例的理由）。
        let normalized = view.path.replace('\\', "/");
        assert!(normalized.ends_with("src/lib.rs"), "{normalized}");
        assert_eq!(view.line, 4);
        assert_eq!(view.end_character, 9);
    }
}
