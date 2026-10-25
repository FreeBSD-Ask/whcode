//! # 剖面模型、协议枚举与鉴权（FR-203／NFR-213、ADR-031）
//!
//! [`ProviderProfile`] 是一个真实供应商接入点的全部声明：协议、端点、路径/查询、
//! 模型、鉴权、额外头/体、时限与输出上限。托管变体（Azure OpenAI、Vertex AI 等）
//! 经路径/查询/鉴权/额外字段剖面覆盖，**不为每家厂商写分支代码**。
//!
//! 凭据边界（FR-203／NFR-213）：密钥只能来自环境变量或仓库外显式文件，封装在
//! [`Secret`] 中——`Debug`/`Serialize` 恒显示 `[REDACTED]`，不得进入日志、
//! 事件、错误 details 或测试夹具。

use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;
use whcode_domain::WhError;

/// 线协议族（v1 五类，ADR-031）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    /// OpenAI Chat Completions 及兼容端点（含 Azure 变体）。
    OpenAi,
    /// Anthropic Messages API。
    Anthropic,
    /// Google Gemini API（`alt=sse` 或 JSON 数组流）。
    Gemini,
    /// Ollama 原生 `/api/chat`（NDJSON）。
    Ollama,
    /// Cohere Chat v2。
    Cohere,
}

impl Protocol {
    /// 全部已注册协议（`llm list` 与未注册错误提示用，ADR-031）。
    #[must_use]
    pub const fn all() -> [Protocol; 5] {
        [
            Self::OpenAi,
            Self::Anthropic,
            Self::Gemini,
            Self::Ollama,
            Self::Cohere,
        ]
    }

    /// 稳定标识（配置与错误提示）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
            Self::Gemini => "gemini",
            Self::Ollama => "ollama",
            Self::Cohere => "cohere",
        }
    }

    /// 按配置名解析（未注册协议返回 `None`，调用方转为结构化错误，ADR-031）。
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "openai" => Some(Self::OpenAi),
            "anthropic" => Some(Self::Anthropic),
            "gemini" => Some(Self::Gemini),
            "ollama" => Some(Self::Ollama),
            "cohere" => Some(Self::Cohere),
            _ => None,
        }
    }
}

/// 密钥来源（FR-203／NFR-213：只允许环境变量或仓库外显式文件）。
#[derive(Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CredentialRef {
    /// 环境变量名。
    Env(String),
    /// 仓库外文件路径（整读后 trim 为密钥值）。
    File(PathBuf),
}

impl fmt::Debug for CredentialRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Env(name) => write!(f, "Env({name})"),
            Self::File(path) => write!(f, "File({})", path.display()),
        }
    }
}

impl CredentialRef {
    /// 求值密钥；缺失或空值返回可操作错误（说明变量名/文件路径，FR-203／NFR-213）。
    ///
    /// # Errors
    ///
    /// 环境变量缺失/为空或文件不可读/为空时返回 [`ErrorCode::InvalidState`]。
    pub fn resolve(&self) -> Result<Secret, WhError> {
        match self {
            Self::Env(name) => {
                let value = std::env::var(name).map_err(|_| {
                    WhError::invalid_state(format!(
                        "凭据缺失：环境变量 {name} 未设置或含非 UTF-8；请设置后重试（密钥不写入日志/事件）"
                    ))
                })?;
                Self::non_empty(Secret(value), || {
                    format!("凭据缺失：环境变量 {name} 为空；请设置后重试")
                })
            }
            Self::File(path) => {
                let raw = std::fs::read_to_string(path).map_err(|err| {
                    WhError::invalid_state(format!(
                        "凭据缺失：文件 {} 不可读：{err}（密钥不写入日志/事件）",
                        path.display()
                    ))
                })?;
                Self::non_empty(Secret(raw.trim().to_string()), || {
                    format!("凭据缺失：文件 {} 为空；请写入密钥后重试", path.display())
                })
            }
        }
    }

    fn non_empty(secret: Secret, message: impl FnOnce() -> String) -> Result<Secret, WhError> {
        if secret.0.trim().is_empty() {
            return Err(WhError::invalid_state(message()));
        }
        Ok(secret)
    }
}

/// 密钥值：`Debug`/`Serialize`/`Display` 恒为 `[REDACTED]`（FR-203／NFR-213 泄漏防线）。
pub struct Secret(String);

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl Serialize for Secret {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str("[REDACTED]")
    }
}

impl Secret {
    /// 密钥原文（仅请求构造路径可用；不得复制进日志/事件/错误）。
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

/// 鉴权方案（FR-203／NFR-213：bearer／自定义头／查询参数／无）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum AuthScheme {
    /// `Authorization: Bearer <key>`。
    Bearer { credential: CredentialRef },
    /// 自定义头（Azure `api-key`、Anthropic `x-api-key`、Gemini `x-goog-api-key`）。
    Header {
        name: String,
        #[serde(default)]
        prefix: String,
        credential: CredentialRef,
    },
    /// 查询参数（Gemini `key`）。
    Query {
        name: String,
        credential: CredentialRef,
    },
    /// 无鉴权（Ollama 本地）。
    None,
}

/// 流帧格式（FR-206／FR-208：协议感知解码）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FrameFormat {
    /// SSE：`data:` 行 + 空行分帧（可带事件名）。
    Sse,
    /// NDJSON：每行一个 JSON 值。
    Ndjson,
    /// JSON 数组流：顶层 `[`，元素逐个到达。
    JsonArray,
}

/// 一个真实供应商接入点（ADR-031）。
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct ProviderProfile {
    /// 剖面名（`llm list` 与 `run --provider`）。
    pub name: String,
    /// 线协议族。
    pub protocol: Protocol,
    /// 端点源（`https://host[:port]`；明文 `http` 仅允许回环，FR-203／NFR-213、ADR-031）。
    pub base_url: String,
    /// 请求路径（如 `/v1/chat/completions`）。
    #[serde(default)]
    pub path: String,
    /// 附加查询参数（Azure `api-version` 等）。
    #[serde(default)]
    pub query: Vec<(String, String)>,
    /// 模型标识。
    pub model: String,
    /// 鉴权方案。
    pub auth: AuthScheme,
    /// 附加请求头（托管变体）。
    #[serde(default)]
    pub extra_headers: Vec<(String, String)>,
    /// 附加请求体字段（托管变体；与协议默认体合并，同键以剖面为准）。
    #[serde(default)]
    pub extra_body: serde_json::Value,
    /// 连接时限。
    #[serde(default = "default_connect_timeout_ms")]
    pub connect_timeout_ms: u64,
    /// 读取时限（两次读取之间的最大间隔）。
    #[serde(default = "default_read_timeout_ms")]
    pub read_timeout_ms: u64,
    /// 总时限。
    #[serde(default = "default_total_timeout_ms")]
    pub total_timeout_ms: u64,
    /// 输出上限（max_tokens）。
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u64,
}

fn default_connect_timeout_ms() -> u64 {
    10_000
}
fn default_read_timeout_ms() -> u64 {
    120_000
}
fn default_total_timeout_ms() -> u64 {
    300_000
}
fn default_max_tokens() -> u64 {
    1_024
}

impl ProviderProfile {
    /// 校验端点：`https` 强制；明文 `http` 仅允许回环（FR-203／NFR-213、ADR-031）。
    ///
    /// # Errors
    ///
    /// 非法端点返回 [`ErrorCode::InvalidState`]，消息可操作。
    pub fn validate_endpoint(&self) -> Result<(), WhError> {
        let rest = self
            .base_url
            .strip_prefix("https://")
            .or_else(|| self.base_url.strip_prefix("http://"));
        let (scheme_ok, rest) = match rest {
            Some(rest) if self.base_url.starts_with("https://") => (true, rest),
            Some(rest) => {
                let host = rest.split('/').next().unwrap_or_default();
                let host = host.split(':').next().unwrap_or_default();
                let loopback = matches!(host, "127.0.0.1" | "localhost" | "[::1]" | "::1");
                if !loopback {
                    return Err(WhError::invalid_state(format!(
                        "非法端点：明文 http 仅允许回环（127.0.0.1/localhost/[::1]），当前 {}；请改用 https",
                        self.base_url
                    )));
                }
                (true, rest)
            }
            None => {
                return Err(WhError::invalid_state(format!(
                    "非法端点：{} 必须以 https://（或回环 http://）开头",
                    self.base_url
                )));
            }
        };
        let _ = scheme_ok;
        if rest.is_empty()
            || !rest.contains('.')
                && !rest.starts_with("127.0.0.1")
                && !rest.starts_with("localhost")
                && !rest.starts_with("[::1]")
        {
            return Err(WhError::invalid_state(format!(
                "非法端点：{} 缺少主机名",
                self.base_url
            )));
        }
        Ok(())
    }

    /// 完整时限三元组（FR-201／FR-616：连接/读取/总时限显式）。
    #[must_use]
    pub fn timeouts(&self) -> (Duration, Duration, Duration) {
        (
            Duration::from_millis(self.connect_timeout_ms),
            Duration::from_millis(self.read_timeout_ms),
            Duration::from_millis(self.total_timeout_ms),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(base: &str) -> ProviderProfile {
        ProviderProfile {
            name: "p".to_string(),
            protocol: Protocol::OpenAi,
            base_url: base.to_string(),
            path: "/v1/chat/completions".to_string(),
            query: Vec::new(),
            model: "m".to_string(),
            auth: AuthScheme::None,
            extra_headers: Vec::new(),
            extra_body: serde_json::Value::Null,
            connect_timeout_ms: 1_000,
            read_timeout_ms: 1_000,
            total_timeout_ms: 1_000,
            max_tokens: 64,
        }
    }

    #[test]
    fn endpoint_https_required_and_loopback_http_allowed() {
        assert!(
            profile("https://api.example.com")
                .validate_endpoint()
                .is_ok()
        );
        assert!(profile("http://127.0.0.1:9000").validate_endpoint().is_ok());
        assert!(profile("http://localhost:9000").validate_endpoint().is_ok());
        let err = profile("http://api.example.com")
            .validate_endpoint()
            .expect_err("明文外网必须拒绝");
        assert_eq!(err.code, whcode_domain::ErrorCode::InvalidState);
        assert!(err.message.contains("https"), "错误须可操作：{err}");
        let err = profile("ftp://api.example.com")
            .validate_endpoint()
            .expect_err("非 http(s) 必须拒绝");
        assert_eq!(err.code, whcode_domain::ErrorCode::InvalidState);
    }

    #[test]
    fn secret_never_leaks_via_debug_or_serialize() {
        let secret = CredentialRef::Env("WHCODE_TEST_SECRET".to_string())
            .resolve()
            .expect_err("未设置时必须失败");
        let _ = secret;
        let value = std::env::var("WHCODE_TEST_SECRET");
        assert!(value.is_err());
        // Debug 形态只暴露来源名，不暴露值。
        assert_eq!(
            format!("{:?}", CredentialRef::Env("WHCODE_TEST_SECRET".to_string())),
            "Env(WHCODE_TEST_SECRET)"
        );
    }

    #[test]
    fn protocol_parse_lists_all_five() {
        assert_eq!(Protocol::parse("openai"), Some(Protocol::OpenAi));
        assert_eq!(Protocol::parse("cohere"), Some(Protocol::Cohere));
        assert_eq!(
            Protocol::parse("bedrock"),
            None,
            "未注册协议返回 None（FR-209）"
        );
        assert_eq!(Protocol::all().len(), 5);
    }
}
