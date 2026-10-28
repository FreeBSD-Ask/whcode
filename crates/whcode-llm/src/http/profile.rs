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

/// 鉴权注入结果：待发送的请求头与查询参数（FR-203／NFR-213）。
///
/// 值可能含密钥，因此 `Debug` **只打印名称**，值恒不输出；下层不得把本结构写入
/// 日志、事件或错误 `details`。密钥原文单独保留在私有字段中，仅用于
/// [`AuthInjection::redact`]——供应商可能在错误正文里回显凭据。
pub struct AuthInjection {
    headers: Vec<(String, String)>,
    query: Vec<(String, String)>,
    secrets: Vec<String>,
}

impl AuthInjection {
    /// 请求头（名称与值）。
    #[must_use]
    pub fn headers(&self) -> &[(String, String)] {
        &self.headers
    }

    /// 查询参数（名称与值）。
    #[must_use]
    pub fn query(&self) -> &[(String, String)] {
        &self.query
    }

    /// 是否未注入任何鉴权项（`AuthScheme::None`）。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.headers.is_empty() && self.query.is_empty()
    }

    /// 把文本中出现的密钥值替换为 `[REDACTED]`（供应商错误正文可能回显凭据）。
    ///
    /// 只替换密钥原文；前缀（如 `Bearer `）不是秘密，无需遮蔽。
    #[must_use]
    pub fn redact(&self, text: &str) -> String {
        let mut scrubbed = text.to_string();
        for secret in &self.secrets {
            if secret.is_empty() {
                continue;
            }
            scrubbed = scrubbed.replace(secret.as_str(), "[REDACTED]");
        }
        scrubbed
    }
}

impl fmt::Debug for AuthInjection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let headers: Vec<&str> = self.headers.iter().map(|(name, _)| name.as_str()).collect();
        let query: Vec<&str> = self.query.iter().map(|(name, _)| name.as_str()).collect();
        f.debug_struct("AuthInjection")
            .field("headers", &headers)
            .field("query", &query)
            .finish()
    }
}

impl AuthScheme {
    /// 声明将要注入的请求头名（**不求值凭据**；装配期冲突校验用）。
    ///
    /// 头名来自方案本身而非密钥值，因此装配期可以在不接触密钥的前提下拒绝
    /// “extra_headers 与鉴权头重复”的剖面。
    #[must_use]
    pub fn declared_header_names(&self) -> Vec<&str> {
        match self {
            Self::None | Self::Query { .. } => Vec::new(),
            Self::Bearer { .. } => vec!["authorization"],
            Self::Header { name, .. } => vec![name.as_str()],
        }
    }

    /// 声明将要注入的查询参数名（**不求值凭据**；装配期冲突校验用）。
    #[must_use]
    pub fn declared_query_names(&self) -> Vec<&str> {
        match self {
            Self::None | Self::Bearer { .. } | Self::Header { .. } => Vec::new(),
            Self::Query { name, .. } => vec![name.as_str()],
        }
    }

    /// 按方案求值凭据并生成注入项（T162；FR-203／NFR-213）。
    ///
    /// `prefix` 原样拼接在密钥之前（需要空格时请显式包含，例如 `Bearer `）。
    ///
    /// # Errors
    ///
    /// 凭据缺失／为空、头名非法、值含控制字符（头注入）或查询参数名非法时返回
    /// [`ErrorCode::InvalidState`](whcode_domain::ErrorCode::InvalidState)；
    /// 错误消息只含来源名或头／参数名，**不含密钥值**。
    pub fn inject(&self) -> Result<AuthInjection, WhError> {
        match self {
            Self::None => Ok(AuthInjection {
                headers: Vec::new(),
                query: Vec::new(),
                secrets: Vec::new(),
            }),
            Self::Bearer { credential } => {
                let secret = credential.resolve()?;
                let value = format!("Bearer {}", secret.expose());
                validate_header_value("authorization", &value)?;
                Ok(AuthInjection {
                    headers: vec![("authorization".to_string(), value)],
                    query: Vec::new(),
                    secrets: vec![secret.expose().to_string()],
                })
            }
            Self::Header {
                name,
                prefix,
                credential,
            } => {
                validate_header_name(name)?;
                let secret = credential.resolve()?;
                let value = format!("{prefix}{}", secret.expose());
                validate_header_value(name, &value)?;
                Ok(AuthInjection {
                    headers: vec![(name.clone(), value)],
                    query: Vec::new(),
                    secrets: vec![secret.expose().to_string()],
                })
            }
            Self::Query { name, credential } => {
                validate_query_name(name)?;
                let secret = credential.resolve()?;
                validate_query_value(name, secret.expose())?;
                Ok(AuthInjection {
                    headers: Vec::new(),
                    query: vec![(name.clone(), secret.expose().to_string())],
                    secrets: vec![secret.expose().to_string()],
                })
            }
        }
    }
}

/// 请求头名校验（RFC 7230 token；防头名注入与非法形态）。
///
/// # Errors
///
/// 名称为空或含非 token 字符时返回 [`ErrorCode::InvalidState`](whcode_domain::ErrorCode::InvalidState)。
pub fn validate_header_name(name: &str) -> Result<(), WhError> {
    if name.is_empty() || !name.bytes().all(is_token_char) {
        return Err(WhError::invalid_state(format!(
            "非法请求头名 {name:?}：必须是非空 RFC 7230 token（不得含空格、冒号或控制字符）"
        )));
    }
    Ok(())
}

/// 请求头值校验：拒绝 CR／LF／NUL 等控制字符（防头注入，FR-203）。
///
/// 错误只含头名，不回显值——值可能含密钥。
///
/// # Errors
///
/// 值含控制字符时返回 [`ErrorCode::InvalidState`](whcode_domain::ErrorCode::InvalidState)。
pub fn validate_header_value(name: &str, value: &str) -> Result<(), WhError> {
    if value.bytes().any(|byte| byte < 0x20 || byte == 0x7f) {
        return Err(WhError::invalid_state(format!(
            "请求头 {name} 的值含控制字符（CR／LF／NUL）：构成头注入风险；密钥与前缀不得跨行\
             （密钥不写入日志与错误）"
        )));
    }
    Ok(())
}

/// 查询参数名校验：非空，且不得含分隔符、空白或控制字符。
///
/// # Errors
///
/// 名称非法时返回 [`ErrorCode::InvalidState`](whcode_domain::ErrorCode::InvalidState)。
pub fn validate_query_name(name: &str) -> Result<(), WhError> {
    if name.is_empty()
        || name.bytes().any(|byte| {
            byte < 0x20 || byte == 0x7f || matches!(byte, b' ' | b'&' | b'=' | b'#' | b'?')
        })
    {
        return Err(WhError::invalid_state(format!(
            "非法查询参数名 {name:?}：不得为空，不得含空格、控制字符或 & = # ? 分隔符"
        )));
    }
    Ok(())
}

/// 查询参数值校验：拒绝控制字符（值由客户端做百分号编码，这里只拦非法控制面）。
///
/// # Errors
///
/// 值含控制字符时返回 [`ErrorCode::InvalidState`](whcode_domain::ErrorCode::InvalidState)；
/// 错误只含参数名，不回显值。
pub fn validate_query_value(name: &str, value: &str) -> Result<(), WhError> {
    if value.bytes().any(|byte| byte < 0x20 || byte == 0x7f) {
        return Err(WhError::invalid_state(format!(
            "查询参数 {name} 的值含控制字符：不得跨行；密钥不写入日志与错误"
        )));
    }
    Ok(())
}

fn is_token_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
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
    /// 校验端点：`https` 强制；明文 `http` 仅允许回环；拒绝 userinfo、非 ASCII 主机与
    /// 异常端口（FR-203／NFR-213、ADR-031 决策 5、T162）。
    ///
    /// 所有错误消息使用**去掉 userinfo 与查询串的端点形式**，避免把端点里内嵌的口令或
    /// 查询参数密钥写进日志与错误 `details`。
    ///
    /// # Errors
    ///
    /// 非法端点返回 [`ErrorCode::InvalidState`]，消息可操作。
    pub fn validate_endpoint(&self) -> Result<(), WhError> {
        let parts = split_endpoint(&self.base_url)?;
        let (host, port) = split_authority(parts.authority)?;
        validate_host(host)?;
        validate_port(port)?;
        if parts.scheme == "http" && !is_loopback(host) {
            return Err(WhError::invalid_state(format!(
                "非法端点：明文 http 仅允许回环（127.0.0.1／localhost／[::1]），当前 {}；请改用 https",
                redacted_endpoint(&self.base_url)
            )));
        }
        if !is_loopback(host) && !host.contains('.') {
            return Err(WhError::invalid_state(format!(
                "非法端点：{} 的主机“{host}”缺少点分域名（形如 api.example.com）",
                redacted_endpoint(&self.base_url)
            )));
        }
        if parts.rest.contains('#') {
            return Err(WhError::invalid_state(format!(
                "非法端点：{} 含片段（#…）：HTTP 请求目标不使用片段；请删除",
                redacted_endpoint(&self.base_url)
            )));
        }
        if parts
            .rest
            .bytes()
            .any(|byte| byte < 0x20 || byte == 0x7f || byte == b' ')
        {
            return Err(WhError::invalid_state(format!(
                "非法端点：{} 的路径含空白或控制字符；请做百分号编码",
                redacted_endpoint(&self.base_url)
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

/// 端点拆分：协议、权威段（主机与端口）与剩余路径／查询（T162）。
struct EndpointParts<'a> {
    scheme: &'static str,
    authority: &'a str,
    rest: &'a str,
}

fn split_endpoint(base_url: &str) -> Result<EndpointParts<'_>, WhError> {
    let (scheme, rest) = if let Some(rest) = base_url.strip_prefix("https://") {
        ("https", rest)
    } else if let Some(rest) = base_url.strip_prefix("http://") {
        ("http", rest)
    } else {
        return Err(WhError::invalid_state(format!(
            "非法端点：{} 必须以 https://（仅回环允许 http://）开头；明文端点会把内容与凭据暴露给中间人",
            redacted_endpoint(base_url)
        )));
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    Ok(EndpointParts {
        scheme,
        authority,
        rest: tail,
    })
}

/// 拆分权威段为（主机，端口）；拒绝 userinfo 与多冒号（未加方括号的 IPv6）。
fn split_authority(authority: &str) -> Result<(&str, Option<&str>), WhError> {
    if authority.contains('@') {
        return Err(WhError::invalid_state(
            "非法端点：权威段含 userinfo（形如 user:pass@host）；凭据只能来自剖面的 auth\
             （环境变量或仓库外文件），不得内嵌在端点里（NFR-213）",
        ));
    }
    if authority.is_empty() {
        return Err(WhError::invalid_state(
            "非法端点：缺少主机名（base_url 形如 https://host[:port]）",
        ));
    }
    if let Some(inner) = authority.strip_prefix('[') {
        let Some((host, tail)) = inner.split_once(']') else {
            return Err(WhError::invalid_state(
                "非法端点：IPv6 主机缺少右方括号（形如 [::1]:8443）",
            ));
        };
        if host.is_empty() || !host.contains(':') {
            return Err(WhError::invalid_state(
                "非法端点：方括号内必须是 IPv6 地址（含冒号）",
            ));
        }
        let port = match tail {
            "" => None,
            tail => Some(tail.strip_prefix(':').ok_or_else(|| {
                WhError::invalid_state("非法端点：IPv6 地址的方括号后只能是 :port")
            })?),
        };
        return Ok((host, port));
    }
    let mut parts = authority.split(':');
    let host = parts.next().unwrap_or_default();
    let port = parts.next();
    if parts.next().is_some() {
        return Err(WhError::invalid_state(
            "非法端点：主机段含多个冒号；IPv6 地址必须写成 [addr]:port 形式",
        ));
    }
    Ok((host, port))
}

fn validate_host(host: &str) -> Result<(), WhError> {
    if host.is_empty() {
        return Err(WhError::invalid_state(
            "非法端点：缺少主机名（base_url 形如 https://host[:port]）",
        ));
    }
    if !host.is_ascii() {
        return Err(WhError::invalid_state(format!(
            "非法端点：主机“{host}”含非 ASCII 字符（IDN）；请改用 punycode（xn--）形式，\
             避免同形字符与解析差异（ADR-031 缺口 3）"
        )));
    }
    if host.bytes().any(|byte| {
        byte < 0x20 || byte == 0x7f || matches!(byte, b' ' | b'/' | b'?' | b'#' | b'@' | b'\\')
    }) {
        return Err(WhError::invalid_state(format!(
            "非法端点：主机“{host}”含非法字符（空白、控制字符或 URL 分隔符）"
        )));
    }
    Ok(())
}

fn validate_port(port: Option<&str>) -> Result<(), WhError> {
    let Some(port) = port else {
        return Ok(());
    };
    if port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(WhError::invalid_state(format!(
            "非法端点：端口“{port}”必须为非空十进制数字（形如 :8080）"
        )));
    }
    let value: u16 = port.parse().map_err(|_| {
        WhError::invalid_state(format!(
            "非法端点：端口 {port} 超出 1—65535；请改为合法端口"
        ))
    })?;
    if value == 0 {
        return Err(WhError::invalid_state(
            "非法端点：端口 0 不是可连接端口；请改为 1—65535",
        ));
    }
    Ok(())
}

fn is_loopback(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

/// 去掉端点中的 userinfo 与查询串（错误消息与日志用，避免泄露内嵌口令或查询参数密钥）。
fn redacted_endpoint(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let path = tail.split(['?', '#']).next().unwrap_or_default();
    format!("{scheme}://{host}{path}")
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

    /// 唯一临时路径（进程 ID + 时间戳），避免并发用例互相覆盖。
    fn unique_temp_path(name: &str) -> PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis());
        std::env::temp_dir().join(format!(
            "whcode-llm-profile-{}-{stamp}-{name}",
            std::process::id()
        ))
    }

    fn write_secret_file(name: &str, content: &str) -> PathBuf {
        let path = unique_temp_path(name);
        std::fs::write(&path, content).expect("写入测试凭据文件");
        path
    }

    #[test]
    fn auth_injection_consumes_every_scheme_and_redacts_debug() {
        let path = write_secret_file("header.txt", "fake-secret-value\n");
        let file = CredentialRef::File(path.clone());

        let bearer = AuthScheme::Bearer {
            credential: file.clone(),
        }
        .inject()
        .expect("Bearer 注入");
        assert_eq!(
            bearer.headers().to_vec(),
            vec![(
                "authorization".to_string(),
                "Bearer fake-secret-value".to_string()
            )]
        );
        assert!(bearer.query().is_empty());

        let custom = AuthScheme::Header {
            name: "x-api-key".to_string(),
            prefix: String::new(),
            credential: file.clone(),
        }
        .inject()
        .expect("自定义头注入");
        assert_eq!(
            custom.headers().to_vec(),
            vec![("x-api-key".to_string(), "fake-secret-value".to_string())]
        );

        let prefixed = AuthScheme::Header {
            name: "authorization".to_string(),
            prefix: "Bearer ".to_string(),
            credential: file.clone(),
        }
        .inject()
        .expect("带前缀的头注入");
        assert_eq!(prefixed.headers()[0].1, "Bearer fake-secret-value");

        let query = AuthScheme::Query {
            name: "key".to_string(),
            credential: file.clone(),
        }
        .inject()
        .expect("查询参数注入");
        assert_eq!(
            query.query().to_vec(),
            vec![("key".to_string(), "fake-secret-value".to_string())]
        );
        assert!(query.headers().is_empty());

        assert!(AuthScheme::None.inject().expect("无鉴权").is_empty());

        for injection in [&bearer, &custom, &query] {
            let debug = format!("{injection:?}");
            let leaked = debug.contains("fake-secret-value");
            assert!(
                !leaked,
                "AuthInjection 的 Debug 不得输出密钥（检测到密钥值）"
            );
        }
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn auth_injection_rejects_header_injection_and_missing_credentials() {
        let crlf = write_secret_file("crlf.txt", "abc\r\nx-injected: 1");
        let err = AuthScheme::Header {
            name: "x-api-key".to_string(),
            prefix: String::new(),
            credential: CredentialRef::File(crlf.clone()),
        }
        .inject()
        .expect_err("含 CR/LF 的密钥必须拒绝（头注入）");
        assert_eq!(err.code, whcode_domain::ErrorCode::InvalidState);
        assert!(err.message.contains("x-api-key"), "{err}");
        assert!(
            !err.message.contains("abc"),
            "错误消息不得回显密钥值：{}",
            err.message
        );
        let _ = std::fs::remove_file(&crlf);

        let ok_file = write_secret_file("ok.txt", "v");
        let err = AuthScheme::Header {
            name: "bad name".to_string(),
            prefix: String::new(),
            credential: CredentialRef::File(ok_file.clone()),
        }
        .inject()
        .expect_err("非法头名必须拒绝");
        assert_eq!(err.code, whcode_domain::ErrorCode::InvalidState);

        let err = AuthScheme::Query {
            name: "a&b".to_string(),
            credential: CredentialRef::File(ok_file.clone()),
        }
        .inject()
        .expect_err("非法查询参数名必须拒绝");
        assert_eq!(err.code, whcode_domain::ErrorCode::InvalidState);
        let _ = std::fs::remove_file(&ok_file);

        let missing = unique_temp_path("missing.txt");
        let err = CredentialRef::File(missing.clone())
            .resolve()
            .expect_err("不可读文件必须失败");
        assert_eq!(err.code, whcode_domain::ErrorCode::InvalidState);
        assert!(err.message.contains("不可读"), "{err}");
        assert!(
            err.message.contains(&missing.display().to_string()),
            "错误须含文件路径（路径不是秘密）：{}",
            err.message
        );

        let empty = write_secret_file("empty.txt", "");
        let err = CredentialRef::File(empty.clone())
            .resolve()
            .expect_err("空文件必须失败");
        assert_eq!(err.code, whcode_domain::ErrorCode::InvalidState);
        assert!(err.message.contains("为空"), "{err}");
        let _ = std::fs::remove_file(&empty);

        let blank = write_secret_file("blank.txt", "  \r\n\t\n");
        let err = CredentialRef::File(blank.clone())
            .resolve()
            .expect_err("只有空白的文件必须失败");
        assert_eq!(err.code, whcode_domain::ErrorCode::InvalidState);
        assert!(err.message.contains("为空"), "{err}");
        let _ = std::fs::remove_file(&blank);
    }

    #[test]
    fn endpoint_hardening_rejects_userinfo_ports_and_non_ascii_hosts() {
        let err = profile("https://user:s3cr3t@api.example.com")
            .validate_endpoint()
            .expect_err("userinfo 必须拒绝");
        assert_eq!(err.code, whcode_domain::ErrorCode::InvalidState);
        assert!(err.message.contains("userinfo"), "{err}");
        assert!(
            !err.message.contains("s3cr3t"),
            "错误不得回显端点内嵌口令：{}",
            err.message
        );

        for base in [
            "https://api.example.com:abc",
            "https://api.example.com:",
            "https://api.example.com:0",
            "https://api.example.com:70000",
        ] {
            let err = profile(base)
                .validate_endpoint()
                .expect_err("端口格式必须拒绝");
            assert_eq!(err.code, whcode_domain::ErrorCode::InvalidState, "{base}");
        }

        let err = profile("https://例え.jp")
            .validate_endpoint()
            .expect_err("IDN 主机必须拒绝");
        assert!(err.message.contains("punycode"), "{err}");

        let err = profile("https://[::1")
            .validate_endpoint()
            .expect_err("IPv6 缺右方括号必须拒绝");
        assert!(err.message.contains("右方括号"), "{err}");

        let err = profile("http://::1:9000")
            .validate_endpoint()
            .expect_err("未加方括号的 IPv6 必须拒绝");
        assert_eq!(err.code, whcode_domain::ErrorCode::InvalidState);

        let err = profile("https://:8443")
            .validate_endpoint()
            .expect_err("缺主机名必须拒绝");
        assert_eq!(err.code, whcode_domain::ErrorCode::InvalidState);

        let err = profile("https://api.example.com#frag")
            .validate_endpoint()
            .expect_err("片段必须拒绝");
        assert!(err.message.contains("片段"), "{err}");

        assert!(
            profile("https://[::1]:8443").validate_endpoint().is_ok(),
            "带方括号的 IPv6 与端口必须接受"
        );
        assert!(
            profile("http://[::1]:9000").validate_endpoint().is_ok(),
            "回环 IPv6 明文允许"
        );
        assert!(
            profile("https://api.example.com/v1?x=1")
                .validate_endpoint()
                .is_ok(),
            "路径与查询必须接受"
        );
    }
}
