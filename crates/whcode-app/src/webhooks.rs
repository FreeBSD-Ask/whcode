//! # webhook 投递编排（P6.4／FR-788／ADR-079）
//!
//! 三件事：**读配置**（`.zcode/webhooks.json`，显式 opt-in）、**建计划**（订阅事件 ×
//! 尚未认领）、**记账**（认领→发送→结果，失败有界重试）。网络与凭据都不在本层：
//! [`WebhookSender`] 由装配根实现，`token_env` 只传变量名，取值在装配根按次求值
//! （ADR-031／ADR-079 决策 1）。
//!
//! 幂等：每条的键是 `<endpoint>:<event_id>`，认领在 store（`INSERT ... ON CONFLICT DO
//! NOTHING`）。**同一批重复执行不会重复投递**——第二次执行看到的是“已认领”而不是空计划。
//!
//! 脱敏：事件载荷按 `TraceLevel::Redacted` 经 `whcode_llm::trace::sanitize` 处理，与事件
//! 导出同口径；正文整体 ≤ [`MAX_BODY_BYTES`]，超限先丢载荷再标 `truncated`。
//!
//! 边界：配置写错**显式失败**（不静默当“未配置”）；未配置＝零端点＝零请求。

use std::collections::BTreeSet;
use std::path::Path;

use duckdb::Connection;
use serde::Deserialize;
use whcode_domain::{ErrorCode, EventKind, TraceLevel, WhError};
use whcode_store::webhooks::{
    DeliveryStatus, MAX_EVENT_KINDS, PendingEvent, claim_delivery, count_by_status,
    list_deliveries, mark_delivered, mark_failed, pending_events,
};

use crate::authz::{Actor, Capability, Policy, require};

/// 配置文件相对工作区的路径。
pub const CONFIG_RELATIVE_PATH: &str = ".zcode/webhooks.json";

/// 端点数量上限。
pub const MAX_ENDPOINTS: usize = 8;

/// 端点名与凭据环境变量名的字符上限。
pub const MAX_NAME_CHARS: usize = 64;

/// 单批投递条数缺省值与上限。
pub const DEFAULT_BATCH: usize = 20;
/// 单批投递条数上限。
pub const MAX_BATCH: usize = 50;

/// 重试次数缺省值（1—5）。
pub const DEFAULT_MAX_ATTEMPTS: i64 = 3;

/// 单次请求时限缺省值与上下限（毫秒）。
pub const DEFAULT_TIMEOUT_MS: u64 = 5_000;
/// 时限下限。
pub const MIN_TIMEOUT_MS: u64 = 500;
/// 时限上限。
pub const MAX_TIMEOUT_MS: u64 = 10_000;

/// 请求正文字节上限（与 `whcode_llm::http::webhook` 一致）。
pub const MAX_BODY_BYTES: usize = 32 * 1024;

fn invalid(message: impl Into<String>) -> WhError {
    WhError::new(ErrorCode::InvalidState, message.into())
}

fn config_err(path: &Path, detail: impl std::fmt::Display) -> WhError {
    invalid(format!(
        "webhook 配置 {} 非法：{detail}（ADR-079 决策 1）",
        path.display()
    ))
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    text.chars().take(max).collect()
}

/// 一个输出端点。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebhookEndpoint {
    /// 端点名（幂等键第一段；改名即新端点）。
    pub name: String,
    /// 端点 URL（https，或回环 http）。
    pub url: String,
    /// 订阅的事件类型稳定名（`EventKind::as_str`）。
    pub events: Vec<String>,
    /// 承载 Bearer 令牌的环境变量名（值不进配置与库）。
    #[serde(default)]
    pub token_env: Option<String>,
    /// 是否启用（缺省启用）。
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

/// 文件形态（缺省值在此落地）。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    #[serde(default)]
    endpoints: Vec<WebhookEndpoint>,
    #[serde(default = "default_batch")]
    batch: usize,
    #[serde(default = "default_max_attempts")]
    max_attempts: i64,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
}

fn default_batch() -> usize {
    DEFAULT_BATCH
}

fn default_max_attempts() -> i64 {
    DEFAULT_MAX_ATTEMPTS
}

fn default_timeout_ms() -> u64 {
    DEFAULT_TIMEOUT_MS
}

/// 校验后的配置（未配置时 `endpoints` 为空）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookConfig {
    /// 端点（已校验；顺序即配置顺序）。
    pub endpoints: Vec<WebhookEndpoint>,
    /// 单批投递条数。
    pub batch: usize,
    /// 单条最大尝试次数。
    pub max_attempts: i64,
    /// 单次请求时限（毫秒）。
    pub timeout_ms: u64,
}

impl WebhookConfig {
    /// 空配置（零端点）。
    #[must_use]
    pub fn empty() -> Self {
        Self {
            endpoints: Vec::new(),
            batch: DEFAULT_BATCH,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            timeout_ms: DEFAULT_TIMEOUT_MS,
        }
    }

    /// 是否没有任何启用端点（零端点＝零请求）。
    #[must_use]
    pub fn is_inert(&self) -> bool {
        !self.endpoints.iter().any(|endpoint| endpoint.enabled)
    }

    /// 启用端点的订阅表：`(端点, 事件类型)`。
    fn enabled(&self) -> impl Iterator<Item = &WebhookEndpoint> {
        self.endpoints.iter().filter(|endpoint| endpoint.enabled)
    }
}

/// 载入并校验配置：文件缺失＝空配置（`Ok`），文件非法＝显式错误。
///
/// # Errors
///
/// JSON 解析失败、字段未知、上限越界、URL 非法、事件类型未知、端点重名时返回
/// [`WhError`]（`invalid_state`）；读取失败（非“不存在”）返回 `io_error`。
pub fn load_config(workspace: &Path) -> Result<WebhookConfig, WhError> {
    let path = workspace.join(CONFIG_RELATIVE_PATH);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(WebhookConfig::empty());
        }
        Err(err) => {
            return Err(WhError::new(
                ErrorCode::IoError,
                format!("读取 webhook 配置 {} 失败：{err}", path.display()),
            ));
        }
    };
    let file: ConfigFile =
        serde_json::from_str(&text).map_err(|err| config_err(&path, err.to_string()))?;
    validate(&path, file)
}

fn validate(path: &Path, file: ConfigFile) -> Result<WebhookConfig, WhError> {
    if file.endpoints.len() > MAX_ENDPOINTS {
        return Err(config_err(
            path,
            format!(
                "端点 {} 个超过 {MAX_ENDPOINTS} 个上限",
                file.endpoints.len()
            ),
        ));
    }
    if !(1..=MAX_BATCH).contains(&file.batch) {
        return Err(config_err(
            path,
            format!("batch 必须在 1—{MAX_BATCH}：收到 {}", file.batch),
        ));
    }
    if !(1..=5).contains(&file.max_attempts) {
        return Err(config_err(
            path,
            format!("max_attempts 必须在 1—5：收到 {}", file.max_attempts),
        ));
    }
    if !(MIN_TIMEOUT_MS..=MAX_TIMEOUT_MS).contains(&file.timeout_ms) {
        return Err(config_err(
            path,
            format!(
                "timeout_ms 必须在 {MIN_TIMEOUT_MS}—{MAX_TIMEOUT_MS}：收到 {}",
                file.timeout_ms
            ),
        ));
    }
    let mut names = BTreeSet::new();
    for endpoint in &file.endpoints {
        if endpoint.name.trim().is_empty() {
            return Err(config_err(path, "端点名不得为空"));
        }
        if endpoint.name.chars().count() > MAX_NAME_CHARS {
            return Err(config_err(
                path,
                format!("端点名超过 {MAX_NAME_CHARS} 字符：{}", endpoint.name),
            ));
        }
        if endpoint.name.chars().any(char::is_control) {
            return Err(config_err(
                path,
                format!("端点名不得含控制字符：{}", endpoint.name),
            ));
        }
        if !names.insert(endpoint.name.as_str()) {
            return Err(config_err(
                path,
                format!("端点名重复：{}（幂等键依赖端点名唯一）", endpoint.name),
            ));
        }
        validate_url(path, &endpoint.name, &endpoint.url)?;
        if endpoint.events.is_empty() {
            return Err(config_err(
                path,
                format!("端点 {} 未订阅任何事件", endpoint.name),
            ));
        }
        if endpoint.events.len() > MAX_EVENT_KINDS {
            return Err(config_err(
                path,
                format!(
                    "端点 {} 订阅 {} 种事件，超过 {MAX_EVENT_KINDS} 种上限",
                    endpoint.name,
                    endpoint.events.len()
                ),
            ));
        }
        for kind in &endpoint.events {
            if EventKind::parse(kind).is_none() {
                return Err(config_err(
                    path,
                    format!(
                        "端点 {} 订阅了未知事件类型 `{kind}`：可用类型见 whcode-domain 的 EventKind",
                        endpoint.name
                    ),
                ));
            }
        }
        if let Some(token_env) = &endpoint.token_env {
            if token_env.trim().is_empty() || token_env.chars().count() > MAX_NAME_CHARS {
                return Err(config_err(
                    path,
                    format!(
                        "端点 {} 的 token_env 非法（空或超过 {MAX_NAME_CHARS} 字符）",
                        endpoint.name
                    ),
                ));
            }
            if !token_env
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
            {
                return Err(config_err(
                    path,
                    format!(
                        "端点 {} 的 token_env 只允许字母、数字与下划线",
                        endpoint.name
                    ),
                ));
            }
        }
    }
    Ok(WebhookConfig {
        endpoints: file.endpoints,
        batch: file.batch,
        max_attempts: file.max_attempts,
        timeout_ms: file.timeout_ms,
    })
}

/// URL 校验：https，或 http 但主机为回环（本地联调）。
fn validate_url(path: &Path, name: &str, url: &str) -> Result<(), WhError> {
    let trimmed = url.trim();
    if trimmed
        .chars()
        .any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return Err(config_err(
            path,
            format!("端点 {name} 的 URL 含空白或控制字符"),
        ));
    }
    let (scheme, rest) = trimmed
        .split_once("://")
        .ok_or_else(|| config_err(path, format!("端点 {name} 的 URL 缺少协议：{trimmed}")))?;
    let host_port = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = if let Some(rest) = host_port.strip_prefix('[') {
        // IPv6 字面量：[::1]:8080
        rest.split(']').next().unwrap_or_default()
    } else {
        host_port.split(':').next().unwrap_or_default()
    };
    if host.is_empty() {
        return Err(config_err(path, format!("端点 {name} 的 URL 缺少主机")));
    }
    match scheme {
        "https" => Ok(()),
        "http" => {
            if is_loopback_host(host) {
                Ok(())
            } else {
                Err(config_err(
                    path,
                    format!(
                        "端点 {name} 的明文 http 只允许回环主机（127.0.0.1／::1／localhost），\
                         实际主机 {host}：改用 https"
                    ),
                ))
            }
        }
        other => Err(config_err(
            path,
            format!("端点 {name} 的协议 `{other}` 不受支持：只允许 https（或回环 http）"),
        )),
    }
}

fn is_loopback_host(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "::1" | "localhost")
}

/// 一条待发送请求（正文、键与凭据变量名；凭据值不在此结构内）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookRequest {
    /// 端点名。
    pub endpoint: String,
    /// 端点 URL。
    pub url: String,
    /// 幂等键 `<endpoint>:<event_id>`。
    pub idempotency_key: String,
    /// 事件标识。
    pub event_id: String,
    /// 事件序号。
    pub event_seq: i64,
    /// JSON 正文（已脱敏、已截断）。
    pub body: String,
    /// Bearer 令牌的环境变量名（值由装配根求值）。
    pub token_env: Option<String>,
    /// 单次时限（毫秒）。
    pub timeout_ms: u64,
}

/// 发送结果（装配根把网络层结果归一成这三类）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebhookSendOutcome {
    /// 2xx。
    Delivered {
        /// HTTP 状态码。
        status: u16,
    },
    /// 可重试（408／429／5xx／传输失败）。
    Retryable {
        /// HTTP 状态码（传输失败时为 `None`）。
        status: Option<u16>,
        /// 归一化原因（不含凭据）。
        reason: String,
    },
    /// 永久失败（其余 4xx）。
    Permanent {
        /// HTTP 状态码。
        status: u16,
        /// 归一化原因。
        reason: String,
    },
}

/// 出站发送口（实现在装配根：网络栈不进 app，ADR-031）。
pub trait WebhookSender {
    /// 发送一条请求；不得 panic，失败归一成 [`WebhookSendOutcome`]。
    fn send(&self, request: &WebhookRequest) -> WebhookSendOutcome;
}

/// 一批投递的结果（用于命令输出与测试断言）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeliverySummary {
    /// 已认领并尝试发送的条数。
    pub attempted: usize,
    /// 成功条数。
    pub delivered: usize,
    /// 可重试失败条数（本轮结束仍待重试）。
    pub retryable_failures: usize,
    /// 永久失败条数（不再重试）。
    pub permanent_failures: usize,
    /// 计划内但不可认领（已投递或已达上限）的条数。
    pub skipped: usize,
}

/// 投递状态视图（`webhook status` 的数据源）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookStatusView {
    /// 配置里的端点总数。
    pub endpoints: usize,
    /// 启用的端点数。
    pub enabled_endpoints: usize,
    /// 三态计数（`claimed`／`delivered`／`failed`）。
    pub counts: [u64; 3],
    /// 最近记录（≤20 条）。
    pub recent: Vec<RecentDelivery>,
}

/// 最近投递事实的最小投影（避免把整行塞进命令输出）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentDelivery {
    /// 幂等键。
    pub idempotency_key: String,
    /// 状态稳定名。
    pub status: String,
    /// 已尝试次数。
    pub attempts: i64,
    /// 最近状态码。
    pub last_status_code: Option<i64>,
    /// 最近错误（已截断）。
    pub last_error: Option<String>,
}

/// 计划投递：按端点取“订阅命中且未认领”的事件，构造有界正文。
///
/// 只读：不认领、不发送（`webhook deliver --dry-run` 与测试用）。
///
/// # Errors
///
/// 缺少 `SessionRead`、`limit` 为 0、正文超限或存储失败时返回 [`WhError`]。
pub fn plan(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    config: &WebhookConfig,
    limit: Option<usize>,
) -> Result<Vec<WebhookRequest>, WhError> {
    require(policy, actor, Capability::SessionRead)?;
    let limit = limit.unwrap_or(config.batch);
    if limit == 0 || limit > MAX_BATCH {
        return Err(invalid(format!(
            "webhook 单批条数必须在 1—{MAX_BATCH}：收到 {limit}"
        )));
    }
    let mut requests = Vec::new();
    for endpoint in config.enabled() {
        if requests.len() >= limit {
            break;
        }
        let remaining = limit - requests.len();
        let events = pending_events(
            conn,
            &endpoint.name,
            &endpoint.events,
            remaining,
            config.max_attempts,
        )?;
        for event in events {
            requests.push(build_request(endpoint, &event, config.timeout_ms)?);
        }
    }
    Ok(requests)
}

/// 构造一条请求：正文脱敏＋截断，键由 store 生成（同一处口径）。
fn build_request(
    endpoint: &WebhookEndpoint,
    event: &PendingEvent,
    timeout_ms: u64,
) -> Result<WebhookRequest, WhError> {
    let body = build_body(event)?;
    Ok(WebhookRequest {
        endpoint: endpoint.name.clone(),
        url: endpoint.url.clone(),
        idempotency_key: whcode_store::webhooks::idempotency_key(&endpoint.name, &event.id)?,
        event_id: event.id.clone(),
        event_seq: event.seq,
        body,
        token_env: endpoint.token_env.clone(),
        timeout_ms,
    })
}

/// 正文构造：白名单字段＋`redacted` 级脱敏；超限先丢载荷再标 `truncated`。
fn build_body(event: &PendingEvent) -> Result<String, WhError> {
    let payload = serde_json::from_str::<serde_json::Value>(&event.payload).ok();
    let sanitized = payload.map(|value| {
        whcode_llm::trace::sanitize(&value, TraceLevel::Redacted)
            .payload
            .unwrap_or(serde_json::Value::Null)
    });
    let mut body = serde_json::json!({
        "event_id": event.id,
        "session_id": event.session_id,
        "kind": event.kind,
        "seq": event.seq,
        "ts": event.ts,
        "payload": sanitized.clone().unwrap_or(serde_json::Value::Null),
        "truncated": sanitized.is_none(),
    });
    let mut text = body.to_string();
    if text.len() > MAX_BODY_BYTES {
        // 载荷是唯一可丢的大块；保留事件身份，让接收方至少知道发生过什么。
        body["payload"] = serde_json::Value::Null;
        body["truncated"] = serde_json::Value::Bool(true);
        text = body.to_string();
        if text.len() > MAX_BODY_BYTES {
            return Err(invalid(format!(
                "webhook 正文 {} 字节超过 {MAX_BODY_BYTES} 上限：事件 {} 的标识或时间戳过长",
                text.len(),
                event.id
            )));
        }
    }
    Ok(text)
}

/// 执行一批投递：认领→发送→记账；失败按 `max_attempts` 有界重试，永久失败一次到底。
///
/// # Errors
///
/// 缺少 `SessionCreate`、`limit` 越界、配置无启用端点（调用方应先判）、存储失败时返回
/// [`WhError`]。单条发送失败**不是**错误：它被记账为 `failed` 并计入返回汇总。
pub fn deliver(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    config: &WebhookConfig,
    sender: &dyn WebhookSender,
    limit: Option<usize>,
) -> Result<DeliverySummary, WhError> {
    require(policy, actor, Capability::SessionCreate)?;
    let requests = plan(conn, policy, actor, config, limit)?;
    let mut summary = DeliverySummary::default();
    for request in requests {
        let Some(_claimed) = claim_delivery(
            conn,
            &request.endpoint,
            &request.event_id,
            request.event_seq,
            config.max_attempts,
        )?
        else {
            summary.skipped += 1;
            continue;
        };
        summary.attempted += 1;
        match sender.send(&request) {
            WebhookSendOutcome::Delivered { status } => {
                mark_delivered(conn, &request.idempotency_key, Some(status))?;
                summary.delivered += 1;
            }
            WebhookSendOutcome::Retryable { status, reason } => {
                mark_failed(
                    conn,
                    &request.idempotency_key,
                    status,
                    &reason,
                    false,
                    config.max_attempts,
                )?;
                summary.retryable_failures += 1;
            }
            WebhookSendOutcome::Permanent { status, reason } => {
                mark_failed(
                    conn,
                    &request.idempotency_key,
                    Some(status),
                    &reason,
                    true,
                    config.max_attempts,
                )?;
                summary.permanent_failures += 1;
            }
        }
    }
    Ok(summary)
}

/// 状态视图：端点计数＋三态计数＋最近 20 条。
///
/// # Errors
///
/// 缺少 `SessionRead` 或存储失败时返回 [`WhError`]。
pub fn status(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    config: &WebhookConfig,
) -> Result<WebhookStatusView, WhError> {
    require(policy, actor, Capability::SessionRead)?;
    let counts = count_by_status(conn)?;
    let recent = list_deliveries(conn, None, 20)?
        .into_iter()
        .map(|record| RecentDelivery {
            idempotency_key: record.idempotency_key,
            status: record.status.as_str().to_string(),
            attempts: record.attempts,
            last_status_code: record.last_status_code,
            last_error: record.last_error.map(|text| truncate_chars(&text, 160)),
        })
        .collect();
    Ok(WebhookStatusView {
        endpoints: config.endpoints.len(),
        enabled_endpoints: config.enabled().count(),
        counts,
        recent,
    })
}

/// 三态计数的稳定名顺序（命令输出与测试共用）。
pub const STATUS_ORDER: [DeliveryStatus; 3] = [
    DeliveryStatus::Claimed,
    DeliveryStatus::Delivered,
    DeliveryStatus::Failed,
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authz::{DefaultPolicy, actor_with_all_capabilities};
    use whcode_store::schema::apply_migrations;

    fn fixture() -> Connection {
        let conn = Connection::open_in_memory().expect("内存数据库");
        apply_migrations(&conn).expect("迁移");
        conn
    }

    fn seed_events(conn: &Connection) {
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, last_active_at, model, archived, status, stage)
             VALUES ('S-1', 'webhook', epoch_ms(1000), epoch_ms(1000), NULL, FALSE, 'active', 'implement')",
            [],
        )
        .expect("插入会话");
        for (seq, id, kind) in [
            (1i64, "EV-1", "TurnFinished"),
            (2, "EV-2", "TurnFinished"),
            (3, "EV-3", "StreamingChunk"),
        ] {
            conn.execute(
                "INSERT INTO events (seq, id, session_id, kind, payload_json, ts, persistence_level)
                 VALUES (?, ?, 'S-1', ?, CAST('{\"token\":\"abcdef\",\"ok\":true}' AS JSON)::VARIANT,
                         current_timestamp, 'redacted')",
                duckdb::params![seq, id, kind],
            )
            .expect("插入事件");
        }
    }

    fn workspace_with(tag: &str, config: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("whcode-webhook-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".zcode")).expect("创建 .zcode");
        std::fs::write(dir.join(".zcode/webhooks.json"), config).expect("写配置");
        dir
    }

    fn valid_config() -> WebhookConfig {
        WebhookConfig {
            endpoints: vec![WebhookEndpoint {
                name: "ci".to_string(),
                url: "https://example.invalid/hook".to_string(),
                events: vec!["TurnFinished".to_string()],
                token_env: None,
                enabled: true,
            }],
            batch: DEFAULT_BATCH,
            max_attempts: 3,
            timeout_ms: DEFAULT_TIMEOUT_MS,
        }
    }

    struct FixedSender {
        outcome: WebhookSendOutcome,
    }

    impl WebhookSender for FixedSender {
        fn send(&self, _request: &WebhookRequest) -> WebhookSendOutcome {
            self.outcome.clone()
        }
    }

    /// 配置校验：缺文件＝空配置；明文非回环、未知事件、重名、越界全部显式报错。
    #[test]
    fn config_validation_is_explicit() {
        let empty = workspace_with("empty", "{\"endpoints\": []}");
        let config = load_config(&empty).expect("空配置");
        assert!(config.is_inert(), "零端点即零请求");

        let missing = workspace_with("missing", "");
        let _ = std::fs::remove_file(missing.join(".zcode/webhooks.json"));
        assert!(load_config(&missing).expect("缺文件").is_inert());

        let cases = [
            r#"{"endpoints":[{"name":"ci","url":"http://example.com/h","events":["TurnFinished"]}]}"#.to_string(),
            r#"{"endpoints":[{"name":"ci","url":"https://x.invalid/h","events":["NotAEvent"]}]}"#.to_string(),
            r#"{"endpoints":[{"name":"ci","url":"https://x.invalid/h","events":["TurnFinished"]},{"name":"ci","url":"https://y.invalid/h","events":["TurnFinished"]}]}"#.to_string(),
            r#"{"endpoints":[],"batch":0}"#.to_string(),
            r#"{"endpoints":[],"max_attempts":9}"#.to_string(),
            r#"{"endpoints":[],"timeout_ms":100}"#.to_string(),
            r#"{"endpoints":[],"unknown_field":true}"#.to_string(),
        ];
        for (index, case) in cases.iter().enumerate() {
            let dir = workspace_with(&format!("bad-{index}"), case);
            let err = load_config(&dir).expect_err("非法配置必须报错");
            assert_eq!(err.code, ErrorCode::InvalidState, "配置：{case}");
            assert!(err.message.contains("webhook 配置"), "{}", err.message);
        }

        // 回环 http 允许（本地联调）。
        let loopback = workspace_with(
            "loopback",
            r#"{"endpoints":[{"name":"local","url":"http://127.0.0.1:9000/h","events":["TurnFinished"]}]}"#,
        );
        assert_eq!(
            load_config(&loopback).expect("回环 http").endpoints.len(),
            1
        );
    }

    /// 计划：只含订阅类型、正文脱敏、键形状稳定，且不写库。
    #[test]
    fn plan_builds_redacted_bodies_without_claiming() {
        let conn = fixture();
        seed_events(&conn);
        let actor = actor_with_all_capabilities("tester");
        let config = valid_config();
        let requests = plan(&conn, &DefaultPolicy, &actor, &config, None).expect("计划");
        assert_eq!(
            requests.len(),
            2,
            "两条 TurnFinished，StreamingChunk 不订阅"
        );
        assert_eq!(requests[0].idempotency_key, "ci:EV-1");
        assert_eq!(requests[0].event_seq, 1);
        assert!(
            !requests[0].body.contains("abcdef"),
            "载荷必须按 redacted 级脱敏：{}",
            requests[0].body
        );
        assert!(requests[0].body.contains("TurnFinished"));
        // 计划是只读的：重复计划结果相同（未认领）。
        let again = plan(&conn, &DefaultPolicy, &actor, &config, None).expect("再计划");
        assert_eq!(again.len(), 2);
    }

    /// 投递：成功→delivered；重跑同批→skipped（不重复投递）。
    #[test]
    fn deliver_is_idempotent_across_runs() {
        let conn = fixture();
        seed_events(&conn);
        let actor = actor_with_all_capabilities("tester");
        let config = valid_config();
        let sender = FixedSender {
            outcome: WebhookSendOutcome::Delivered { status: 204 },
        };
        let first = deliver(&conn, &DefaultPolicy, &actor, &config, &sender, None).expect("投递");
        assert_eq!(first.attempted, 2);
        assert_eq!(first.delivered, 2);
        let second =
            deliver(&conn, &DefaultPolicy, &actor, &config, &sender, None).expect("再投递");
        assert_eq!(
            second.attempted + second.skipped,
            0,
            "没有未认领事件：第二次不应产生任何投递（ADR-079 决策 2）"
        );
    }

    /// 可重试失败：第一次记 failed(1 次)，第二次重试到 2 次，达上限后不再出现。
    #[test]
    fn retryable_failures_retry_until_limit() {
        let conn = fixture();
        seed_events(&conn);
        let actor = actor_with_all_capabilities("tester");
        let config = WebhookConfig {
            max_attempts: 2,
            ..valid_config()
        };
        let sender = FixedSender {
            outcome: WebhookSendOutcome::Retryable {
                status: Some(503),
                reason: "HTTP 503".to_string(),
            },
        };
        let first =
            deliver(&conn, &DefaultPolicy, &actor, &config, &sender, None).expect("第 1 轮");
        assert_eq!(first.retryable_failures, 2);
        let second =
            deliver(&conn, &DefaultPolicy, &actor, &config, &sender, None).expect("第 2 轮");
        assert_eq!(second.attempted, 2, "未达上限应重试");
        let third =
            deliver(&conn, &DefaultPolicy, &actor, &config, &sender, None).expect("第 3 轮");
        assert_eq!(third.attempted, 0, "达到 max_attempts 后不再重试");
        let counts = count_by_status(&conn).expect("统计");
        assert_eq!(counts, [0, 0, 2]);
        let failed = list_deliveries(&conn, Some(DeliveryStatus::Failed), 10).expect("失败列表");
        assert!(failed.iter().all(|record| record.attempts == 2));
    }

    /// 永久失败：一次到底，不再重试。
    #[test]
    fn permanent_failure_is_not_retried() {
        let conn = fixture();
        seed_events(&conn);
        let actor = actor_with_all_capabilities("tester");
        let config = valid_config();
        let sender = FixedSender {
            outcome: WebhookSendOutcome::Permanent {
                status: 422,
                reason: "HTTP 422".to_string(),
            },
        };
        let first = deliver(&conn, &DefaultPolicy, &actor, &config, &sender, None).expect("投递");
        assert_eq!(first.permanent_failures, 2);
        let second =
            deliver(&conn, &DefaultPolicy, &actor, &config, &sender, None).expect("再投递");
        assert_eq!(second.attempted, 0);
        let failed = list_deliveries(&conn, Some(DeliveryStatus::Failed), 10).expect("失败列表");
        assert!(
            failed
                .iter()
                .all(|record| record.attempts == config.max_attempts),
            "永久失败应直接置为上限：{failed:?}"
        );
    }

    /// 权限与状态视图：无能力者不能计划/投递/查状态；状态给出三态计数。
    #[test]
    fn permissions_and_status_view() {
        let conn = fixture();
        seed_events(&conn);
        let denied = Actor::new("nobody", []);
        let config = valid_config();
        let sender = FixedSender {
            outcome: WebhookSendOutcome::Delivered { status: 200 },
        };
        assert_eq!(
            plan(&conn, &DefaultPolicy, &denied, &config, None)
                .expect_err("无 SessionRead")
                .code,
            ErrorCode::PermissionDenied
        );
        assert_eq!(
            deliver(&conn, &DefaultPolicy, &denied, &config, &sender, None)
                .expect_err("无 SessionCreate")
                .code,
            ErrorCode::PermissionDenied
        );
        let actor = actor_with_all_capabilities("tester");
        deliver(&conn, &DefaultPolicy, &actor, &config, &sender, None).expect("投递");
        let view = status(&conn, &DefaultPolicy, &actor, &config).expect("状态");
        assert_eq!(view.endpoints, 1);
        assert_eq!(view.enabled_endpoints, 1);
        assert_eq!(view.counts, [0, 2, 0]);
        assert_eq!(view.recent.len(), 2);
        assert!(view.recent.iter().all(|item| item.status == "delivered"));
    }
}
