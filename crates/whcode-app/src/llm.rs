//! # LLM 调用编排（`llm run`／`llm cancel`；T167、FR-201—208、ADR-031／ADR-032）
//!
//! 本模块是四端共用的**唯一**调用入口：权限判定（既有 [`Capability::SessionCreate`]，
//! 不新造旁路）、会话存在性、剖面解析、凭据求值、流式 chunk 收集、有界事件与取消标记
//! 都在这里完成；前端只负责把参数传进来、把结果渲染出去。
//!
//! 边界与取舍：
//!
//! - **网络依赖不进 app**：真实传输由装配根实现 [`ProviderFactory`]（`whcode-bin` 按
//!   `http` feature 装配 `whcode-llm` 的真实 Provider）；本模块只见 [`Provider`] trait，
//!   零网络依赖（`test_layering` 可复核）。
//! - **同步、无线程**：一次调用占用调用方线程（CUI 单次请求、Web 单线程服务循环），
//!   不新增线程、锁或异步运行时（ADR-031 决策 3、NFR-216）。
//! - **有界**：剖面文件 ≤ 64 KiB、剖面 ≤ 32 个、单次调用 chunk ≤ 1024、
//!   正文累计 ≤ 1 MiB、事件 ≤ chunk 数 + 2；超限显式标记 `truncated`，不静默截断
//!   （FR-206／FR-208、NFR-215）。
//! - **凭据不落输出面**：凭据只经 `CredentialRef::resolve` 求值，`Secret` 的
//!   `Debug`／`Serialize` 恒为 `[REDACTED]`；事件只带计数与标识，不带提示词、
//!   正文片段与密钥（FR-203、NFR-213）。
//! - **取消跨进程**：`llm run` 在标记目录写 `<request_id>.live`，每个 chunk 前检查
//!   `<request_id>.cancel`；`llm cancel` 只在 `.live` 存在时写取消标记（未知请求返回
//!   `not_found`，不制造无主标记文件）。标记目录默认系统临时目录下
//!   `whcode-llm-cancel`，可由 `WHCODE_LLM_CANCEL_DIR` 显式覆盖（不在仓库内）。
//!   取消在**下一个 chunk 之前**生效（T166 尚未交付真实 HTTP 读循环取消语义）。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::Serialize;
use serde_json::{Value, json};
use whcode_domain::{ErrorCode, WhError};
use whcode_llm::http::profile::ProviderProfile;
use whcode_llm::{CancelToken, LlmRequest, Provider, TransportSnapshot, Usage};

use crate::authz::{Actor, Capability, Policy, require};
use crate::session::show_session;

/// 剖面文件路径的环境变量名（仓库外显式 JSON 文件；凭据本身另由剖面 `auth` 声明）。
pub const PROFILES_ENV: &str = "WHCODE_LLM_PROFILES";
/// 取消标记目录的环境变量名（缺省为系统临时目录下的 `whcode-llm-cancel`）。
pub const CANCEL_DIR_ENV: &str = "WHCODE_LLM_CANCEL_DIR";
/// 剖面文件大小上限（字节）：解析前先看元数据，超限直接拒绝。
pub const MAX_PROFILE_FILE_BYTES: u64 = 64 << 10;
/// 剖面数量上限（有界累积，NFR-215）。
pub const MAX_PROFILES: usize = 32;
/// 单次调用收集的 chunk 上限。
pub const MAX_STREAM_CHUNKS: usize = 1024;
/// 单次调用正文累计上限（字节）。
pub const MAX_OUTPUT_BYTES: usize = 1 << 20;
/// 请求 ID 最大长度（同时用于标记文件名，防止路径膨胀）。
pub const MAX_REQUEST_ID_LEN: usize = 64;

/// 活跃标记后缀（`llm run` 持有；RAII 清理）。
const LIVE_SUFFIX: &str = ".live";
/// 取消标记后缀（`llm cancel` 写入；`llm run` 的下一次 chunk 前读取）。
const CANCEL_SUFFIX: &str = ".cancel";

/// 一个真实供应商接入点集合（剖面注册表）。
///
/// 注册表只持有剖面声明，不含密钥值；凭据在 [`run`] 求值时才读取环境变量或文件。
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderRegistry {
    profiles: Vec<ProviderProfile>,
}

impl ProviderRegistry {
    /// 从 JSON 文本构造：接受 `{"profiles": [...]}` 或裸数组 `[...]`。
    ///
    /// # Errors
    ///
    /// 非 JSON、空表、条目超过 [`MAX_PROFILES`]、名称为空／重复、端点非法时返回
    /// [`ErrorCode::InvalidState`]（消息可操作，不含任何密钥值）。
    pub fn from_json(raw: &str) -> Result<Self, WhError> {
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum ProfileFile {
            Wrapped { profiles: Vec<ProviderProfile> },
            Bare(Vec<ProviderProfile>),
        }
        let parsed: ProfileFile = serde_json::from_str(raw)
            .map_err(|err| WhError::invalid_state(format!("剖面文件不是合法 JSON：{err}")))?;
        let profiles = match parsed {
            ProfileFile::Wrapped { profiles } | ProfileFile::Bare(profiles) => profiles,
        };
        Self::from_profiles(profiles)
    }

    /// 从剖面列表构造并校验（名称唯一、端点合法、数量有界）。
    ///
    /// # Errors
    ///
    /// 与 [`ProviderRegistry::from_json`] 相同的校验失败语义。
    pub fn from_profiles(profiles: Vec<ProviderProfile>) -> Result<Self, WhError> {
        if profiles.is_empty() {
            return Err(WhError::invalid_state(
                "剖面文件为空：至少声明一个剖面（ADR-031：无剖面即无请求）",
            ));
        }
        if profiles.len() > MAX_PROFILES {
            return Err(WhError::invalid_state(format!(
                "剖面数量 {} 超过上限 {MAX_PROFILES}（NFR-215：有界累积）",
                profiles.len()
            )));
        }
        let mut seen = BTreeSet::new();
        for profile in &profiles {
            let name = profile.name.trim();
            if name.is_empty() {
                return Err(WhError::invalid_state("剖面缺少 name（不可为空）"));
            }
            if !seen.insert(name.to_string()) {
                return Err(WhError::invalid_state(format!(
                    "剖面名重复：{name}（每个剖面必须唯一）"
                )));
            }
            profile.validate_endpoint()?;
        }
        Ok(Self { profiles })
    }

    /// 读取剖面文件（仓库外显式路径；先查元数据大小再读，超限不解析）。
    ///
    /// # Errors
    ///
    /// 文件不可读、超过 [`MAX_PROFILE_FILE_BYTES`] 或内容非法时返回 [`ErrorCode::InvalidState`]。
    pub fn load(path: &Path) -> Result<Self, WhError> {
        let meta = std::fs::metadata(path).map_err(|err| {
            WhError::invalid_state(format!("剖面文件不可读（{}）：{err}", path.display()))
        })?;
        if meta.len() > MAX_PROFILE_FILE_BYTES {
            return Err(WhError::invalid_state(format!(
                "剖面文件 {} 为 {} 字节，超过上限 {MAX_PROFILE_FILE_BYTES} 字节",
                path.display(),
                meta.len()
            )));
        }
        let raw = std::fs::read_to_string(path).map_err(|err| {
            WhError::invalid_state(format!("剖面文件不可读（{}）：{err}", path.display()))
        })?;
        Self::from_json(&raw)
    }

    /// 按环境变量 [`PROFILES_ENV`] 读取剖面文件。
    ///
    /// # Errors
    ///
    /// 环境变量未设置／为空、或文件非法时返回 [`ErrorCode::InvalidState`]（消息可操作）。
    pub fn from_env() -> Result<Self, WhError> {
        let path = profiles_path_from_env()?;
        Self::load(&path)
    }

    /// 已注册剖面名（错误提示用；不含任何秘密）。
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.profiles
            .iter()
            .map(|profile| profile.name.clone())
            .collect()
    }

    /// 按名查找剖面（未注册返回 `None`，由调用方转为结构化错误，不静默回落）。
    #[must_use]
    pub fn find(&self, name: &str) -> Option<&ProviderProfile> {
        self.profiles.iter().find(|profile| profile.name == name)
    }

    /// 剖面数量。
    #[must_use]
    pub fn len(&self) -> usize {
        self.profiles.len()
    }

    /// 是否为空（构造时已拒绝空表，保留给调用方的防御性判断）。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }
}

/// `llm run` 的请求（呈现无关；前端负责参数解析与文案）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmRunRequest {
    /// 会话 ID（必须先存在；缺会话不伪装成空成功）。
    pub session_id: String,
    /// 剖面名（用户显式选择；ADR-031 决策 4）。
    pub provider: String,
    /// 提示词正文（不进入事件与错误正文）。
    pub prompt: String,
    /// 模型覆盖（`None` 用剖面 `model`）。
    pub model: Option<String>,
    /// `max_tokens` 覆盖（`None` 用剖面值）。
    pub max_tokens: Option<u64>,
    /// 预置请求 ID（供 `llm cancel` 在同机另一进程使用；`None` 自动生成）。
    pub request_id: Option<String>,
}

/// 单个 chunk 的呈现视图（**只带计数**，不带正文——事件与日志不复制模型输出）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LlmChunkView {
    /// chunk 序号（从 0 起）。
    pub index: u64,
    /// 该 chunk 的字符数。
    pub chars: usize,
    /// 是否终结 chunk。
    pub final_chunk: bool,
}

/// `llm run` 的结果（JSON 形状由前端信封包裹）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LlmRunOutcome {
    /// 信封 schema 版本（各端统一）。
    pub schema_version: u32,
    /// 请求 ID（`llm cancel` 的键）。
    pub request_id: String,
    /// 剖面名。
    pub provider: String,
    /// 线协议稳定标识。
    pub protocol: String,
    /// 实际使用的模型。
    pub model: String,
    /// chunk 视图（有界；`truncated = true` 时被截断）。
    pub chunks: Vec<LlmChunkView>,
    /// 拼接后的正文（有界）。
    pub text: String,
    /// 归一化终止原因（供应商未给出时为 `None`）。
    pub finish_reason: Option<String>,
    /// 是否读到终结 chunk（缺失即完整性缺口，FR-208）。
    pub completed: bool,
    /// 是否因上限截断（chunk 数或正文累计）。
    pub truncated: bool,
    /// chunk 序号不连续的次数（完整性标记，FR-208）。
    pub integrity_gaps: u64,
    /// 计量（供应商返回值或本地估算，估算带 `cost_is_estimate = true`）。
    pub usage: Option<Usage>,
    /// 有界事件序列（只含标识与计数）。
    pub events: Vec<Value>,
    /// 传输只读快照（真实 Provider 提供；桩 / 待接线为 `None`）。
    pub transport: Option<TransportSnapshot>,
}

/// `llm cancel` 的结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LlmCancelOutcome {
    /// 信封 schema 版本。
    pub schema_version: u32,
    /// 请求 ID。
    pub request_id: String,
    /// 稳定状态名。
    pub state: String,
    /// 取消生效时机（`before_next_chunk`）。
    pub effective: String,
}

/// 真实 Provider 的装配接口（由装配根实现；app 只见 trait，零网络依赖）。
pub trait ProviderFactory {
    /// 由剖面构造 Provider。
    ///
    /// 凭据不作为实参传入：剖面的 `auth` 声明凭据来源，由传输层自行求值并校验；
    /// 已求值的密钥值不得流经工厂边界（它不属于装配协议，且会扩大密钥的传播面）。
    ///
    /// # Errors
    ///
    /// 传输层未就绪、凭据不可用或构造失败时返回结构化错误（不得伪造成功）。
    fn create(&self, profile: &ProviderProfile) -> Result<Box<dyn Provider>, WhError>;
}

/// 四端共用的 LLM 调用上下文（由装配根构造并注入）。
pub struct LlmContext<'a> {
    /// 剖面注册表；`None` 时用 `registry_error` 说明原因（装配根未接线／环境未配置）。
    pub registry: Option<&'a ProviderRegistry>,
    /// 注册表不可用的可操作原因（不含秘密）。
    pub registry_error: Option<&'a str>,
    /// 真实 Provider 工厂。
    pub factory: &'a dyn ProviderFactory,
    /// 取消标记目录（仓库外；见模块文档）。
    pub cancel_dir: &'a Path,
}

/// 环境变量 [`PROFILES_ENV`] 指向的剖面路径。
///
/// # Errors
///
/// 环境变量未设置或为空时返回 [`ErrorCode::InvalidState`]（默认离线：无剖面即无请求）。
pub fn profiles_path_from_env() -> Result<PathBuf, WhError> {
    std::env::var_os(PROFILES_ENV)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| {
            WhError::invalid_state(format!(
                "未配置 LLM 剖面：请设置环境变量 {PROFILES_ENV} 指向仓库外的剖面 JSON\
                 （ADR-031：无剖面即无请求；默认离线）"
            ))
        })
}

/// 取消标记目录：`WHCODE_LLM_CANCEL_DIR`，缺省系统临时目录下 `whcode-llm-cancel`。
#[must_use]
pub fn cancel_dir_from_env() -> PathBuf {
    std::env::var_os(CANCEL_DIR_ENV)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| std::env::temp_dir().join("whcode-llm-cancel"))
}

/// 执行一次 `llm run`（同步；权限、会话、剖面、凭据、流式收集与取消全在本函数内）。
///
/// # Errors
///
/// - 权限不足 → `permission_denied`；
/// - 会话不存在 → `not_found`；
/// - 未配置剖面／未知剖面／空提示词／凭据缺失／端点非法 → `invalid_state`；
/// - 传输层未就绪由 [`ProviderFactory`] 返回结构化错误（不伪造成功）；
/// - Provider 执行错误原样返回（超时、IO、状态码映射见 `whcode-llm::http::errors`）；
/// - 用户取消 → `invalid_state`，`details.effective = before_next_chunk`。
pub fn run(
    conn: &duckdb::Connection,
    policy: &dyn Policy,
    actor: &Actor,
    context: &LlmContext<'_>,
    request: &LlmRunRequest,
    now_ms: u64,
) -> Result<LlmRunOutcome, WhError> {
    require(policy, actor, Capability::SessionCreate)?;
    let session = request.session_id.trim();
    if session.is_empty() {
        return Err(WhError::invalid_state("llm run 需要非空 session ID"));
    }
    // 会话存在性：缺会话不伪装成空成功（FR-622 的“正确的空”口径）。
    show_session(conn, policy, actor, session)?;

    let prompt = request.prompt.trim();
    if prompt.is_empty() {
        return Err(WhError::invalid_state(
            "llm run 需要非空提示词：请用 --prompt 显式提供（不隐式读取已存内容）",
        ));
    }
    let registry = context
        .registry
        .ok_or_else(|| match context.registry_error {
            Some(message) => WhError::invalid_state(format!("LLM 剖面不可用：{message}")),
            None => registry_unavailable(),
        })?;
    let provider_name = request.provider.trim();
    let profile = registry.find(provider_name).ok_or_else(|| {
        WhError::invalid_state(format!(
            "未注册剖面 {provider_name}；已注册：{}（ADR-031：未注册名不静默回落）",
            registry.names().join(", ")
        ))
    })?;
    profile.validate_endpoint()?;

    let model = request
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(profile.model.as_str())
        .to_string();
    if model.is_empty() {
        return Err(WhError::invalid_state(
            "模型标识为空：请在剖面声明 model 或用 --model 显式覆盖",
        ));
    }
    let max_tokens = request.max_tokens.unwrap_or(profile.max_tokens);
    let request_id = request
        .request_id
        .as_deref()
        .map(validate_request_id)
        .transpose()?
        .map(str::to_string)
        .unwrap_or_else(|| generate_request_id(session, provider_name, prompt, now_ms));

    let (_, _, total) = profile.timeouts();
    let marker = CancelMarker::begin(context.cancel_dir, &request_id)?;
    let cancel = CancelToken::default();
    let provider = context.factory.create(profile)?;
    if !provider.is_available() {
        return Err(WhError::invalid_state(format!(
            "Provider {} 不可用：检查剖面与凭据来源后重试",
            provider.name()
        )));
    }

    let llm_request = LlmRequest {
        request_id: request_id.clone(),
        session_id: session.to_string(),
        task_id: None,
        model: model.clone(),
        messages: vec![whcode_llm::provider::LlmMessage::user(prompt.to_string())],
        max_tokens,
        timeout: Some(total),
    };
    let started = Instant::now();
    let stream = provider.execute(&llm_request, &cancel)?;

    let mut events = Vec::with_capacity(MAX_STREAM_CHUNKS + 2);
    events.push(json!({
        "kind": "llm.request_started",
        "request_id": request_id,
        "provider": profile.name,
        "protocol": profile.protocol.as_str(),
        "model": model,
        "max_tokens": max_tokens,
    }));
    let mut chunks: Vec<LlmChunkView> = Vec::new();
    let mut text = String::new();
    let mut text_bytes = 0usize;
    let mut expected_index = 0u64;
    let mut integrity_gaps = 0u64;
    let mut finish_reason = None;
    let mut provider_usage = None;
    let mut truncated = false;

    for item in stream {
        if marker.is_cancelled() {
            cancel.cancel();
        }
        if cancel.is_cancelled() {
            return Err(cancelled_error(&request_id));
        }
        let chunk = match item {
            Ok(chunk) => chunk,
            Err(err) => {
                if cancel.is_cancelled() {
                    return Err(cancelled_error(&request_id));
                }
                return Err(err);
            }
        };
        if chunk.index != expected_index {
            integrity_gaps = integrity_gaps.saturating_add(1);
        }
        expected_index = chunk.index.saturating_add(1);
        let chars = chunk.text.chars().count();
        if chunks.len() >= MAX_STREAM_CHUNKS
            || text_bytes.saturating_add(chunk.text.len()) > MAX_OUTPUT_BYTES
        {
            truncated = true;
            cancel.cancel();
            break;
        }
        text_bytes = text_bytes.saturating_add(chunk.text.len());
        text.push_str(&chunk.text);
        if chunk.final_chunk {
            finish_reason.clone_from(&chunk.finish_reason);
            provider_usage = chunk.usage;
        }
        events.push(json!({
            "kind": "llm.delta",
            "index": chunk.index,
            "chars": chars,
        }));
        let final_chunk = chunk.final_chunk;
        chunks.push(LlmChunkView {
            index: chunk.index,
            chars,
            final_chunk,
        });
        if final_chunk {
            break;
        }
    }

    let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let usage = match provider_usage {
        Some(usage) if usage.latency_ms > 0 => usage,
        Some(usage) => usage.with_latency(latency_ms),
        None => Usage::estimate(prompt, &text).with_latency(latency_ms),
    };
    let completed = finish_reason.is_some();
    events.push(json!({
        "kind": "llm.finished",
        "request_id": request_id,
        "chunks": chunks.len(),
        "completed": completed,
        "truncated": truncated,
        "integrity_gaps": integrity_gaps,
        "finish_reason": finish_reason,
        "usage": usage.completion_payload(),
    }));
    let transport = provider.transport_snapshot();
    drop(marker);

    Ok(LlmRunOutcome {
        schema_version: whcode_domain::SCHEMA_VERSION,
        request_id,
        provider: profile.name.clone(),
        protocol: profile.protocol.as_str().to_string(),
        model,
        chunks,
        text,
        finish_reason,
        completed,
        truncated,
        integrity_gaps,
        usage: Some(usage),
        events,
        transport,
    })
}

/// 取消一次同机正在运行的调用（`llm cancel`）。
///
/// 只有 `.live` 标记存在时才写取消标记：未知请求返回 `not_found`，不制造无主文件。
///
/// # Errors
///
/// 权限不足 → `permission_denied`；请求 ID 非法 → `invalid_state`；
/// 无活跃调用 → `not_found`；标记写入失败 → `io_error`。
pub fn cancel(
    policy: &dyn Policy,
    actor: &Actor,
    context: &LlmContext<'_>,
    request_id: &str,
) -> Result<LlmCancelOutcome, WhError> {
    require(policy, actor, Capability::SessionCreate)?;
    let request_id = validate_request_id(request_id)?;
    let live = marker_path(context.cancel_dir, request_id, LIVE_SUFFIX);
    if !live.exists() {
        return Err(WhError::new(
            ErrorCode::NotFound,
            format!(
                "没有活跃调用 {request_id}：取消只对同机正在运行的 llm run 生效\
                 （标记目录由 {CANCEL_DIR_ENV} 指定）"
            ),
        )
        .with_details(json!({
            "request_id": request_id,
            "cancel_dir": context.cancel_dir.display().to_string(),
        })));
    }
    let target = marker_path(context.cancel_dir, request_id, CANCEL_SUFFIX);
    std::fs::write(&target, b"cancel").map_err(|err| {
        WhError::new(
            ErrorCode::IoError,
            format!(
                "写入取消标记失败（{}）：{err}；请求 {request_id} 未被取消",
                target.display()
            ),
        )
    })?;
    Ok(LlmCancelOutcome {
        schema_version: whcode_domain::SCHEMA_VERSION,
        request_id: request_id.to_string(),
        state: "cancel_requested".to_string(),
        effective: "before_next_chunk".to_string(),
    })
}

/// 注册表不可用时的结构化错误（区分“未配置”与“配置损坏”）。
fn registry_unavailable() -> WhError {
    WhError::invalid_state(
        "LLM 剖面不可用：装配根未注入剖面（或环境变量 WHCODE_LLM_PROFILES 指向的仓库外文件\
         未通过校验）；ADR-031：无剖面即无请求，默认离线",
    )
}

/// 请求 ID 校验：只允许 `[A-Za-z0-9_-]`，长度 1..=[`MAX_REQUEST_ID_LEN`]（作为文件名安全）。
fn validate_request_id(request_id: &str) -> Result<&str, WhError> {
    let trimmed = request_id.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_REQUEST_ID_LEN {
        return Err(WhError::invalid_state(format!(
            "请求 ID 长度必须在 1..={MAX_REQUEST_ID_LEN} 之间（当前 {}）",
            trimmed.len()
        )));
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err(WhError::invalid_state(
            "请求 ID 只允许 ASCII 字母、数字、`-` 与 `_`（作为取消标记文件名）",
        ));
    }
    Ok(trimmed)
}

/// 自动请求 ID：`llm-<now>-<hash>`（不含提示词正文，桶号稳定可复用）。
fn generate_request_id(session: &str, provider: &str, prompt: &str, now_ms: u64) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    session.hash(&mut hasher);
    provider.hash(&mut hasher);
    prompt.hash(&mut hasher);
    now_ms.hash(&mut hasher);
    format!("llm-{now_ms}-{:016x}", hasher.finish())
}

/// 取消标记路径。
fn marker_path(dir: &Path, request_id: &str, suffix: &str) -> PathBuf {
    dir.join(format!("{request_id}{suffix}"))
}

/// 用户取消的结构化错误（生效时机显式写入，不谎称立即中止）。
fn cancelled_error(request_id: &str) -> WhError {
    WhError::invalid_state(format!(
        "调用已被取消（请求 {request_id}）：取消在下一个 chunk 之前生效；\
         真实 HTTP 读循环中的取消语义见 T166"
    ))
    .with_details(json!({
        "request_id": request_id,
        "effective": "before_next_chunk",
    }))
}

/// 活跃标记（RAII）：创建 `<request_id>.live`，析构时清理 `.live` 与 `.cancel`。
///
/// 不新增线程与锁；每个进程至多一个活跃标记（单次调用占用调用线程）。
struct CancelMarker {
    dir: PathBuf,
    request_id: String,
}

impl CancelMarker {
    fn begin(dir: &Path, request_id: &str) -> Result<Self, WhError> {
        std::fs::create_dir_all(dir).map_err(|err| {
            WhError::new(
                ErrorCode::IoError,
                format!("取消标记目录不可用（{}）：{err}", dir.display()),
            )
        })?;
        let live = marker_path(dir, request_id, LIVE_SUFFIX);
        std::fs::write(&live, b"live").map_err(|err| {
            WhError::new(
                ErrorCode::IoError,
                format!("写入活跃标记失败（{}）：{err}", live.display()),
            )
        })?;
        Ok(Self {
            dir: dir.to_path_buf(),
            request_id: request_id.to_string(),
        })
    }

    fn is_cancelled(&self) -> bool {
        marker_path(&self.dir, &self.request_id, CANCEL_SUFFIX).exists()
    }
}

impl Drop for CancelMarker {
    fn drop(&mut self) {
        // 尽力清理：失败不改变调用结果（残留标记只影响同名请求的下一次取消判定）。
        let _ = std::fs::remove_file(marker_path(&self.dir, &self.request_id, LIVE_SUFFIX));
        let _ = std::fs::remove_file(marker_path(&self.dir, &self.request_id, CANCEL_SUFFIX));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use whcode_domain::ErrorCode;
    use whcode_llm::StubProvider;
    use whcode_llm::http::profile::{AuthScheme, CredentialRef};
    use whcode_store::apply_migrations;

    use crate::{DefaultPolicy, NewSession, actor_with_all_capabilities, new_session};

    /// 桩工厂：返回 `StubProvider`（不联网）。
    struct StubFactory;

    impl ProviderFactory for StubFactory {
        fn create(&self, _profile: &ProviderProfile) -> Result<Box<dyn Provider>, WhError> {
            Ok(Box::new(StubProvider::new()))
        }
    }

    /// 待接线工厂：模拟 `whcode-llm` HTTP 客户端尚未交付的装配路径。
    struct PendingFactory;

    impl ProviderFactory for PendingFactory {
        fn create(&self, _profile: &ProviderProfile) -> Result<Box<dyn Provider>, WhError> {
            Err(WhError::invalid_state(
                "待接线：whcode-llm 的 HTTP 客户端未就绪（结构化失败，不伪造调用成功）",
            ))
        }
    }

    fn migrated() -> duckdb::Connection {
        let conn = duckdb::Connection::open_in_memory().expect("内存库");
        apply_migrations(&conn).expect("迁移");
        conn
    }

    fn profile(name: &str) -> ProviderProfile {
        ProviderProfile {
            name: name.to_string(),
            protocol: whcode_llm::http::profile::Protocol::OpenAi,
            base_url: "https://api.example.com".to_string(),
            path: "/v1/chat/completions".to_string(),
            query: Vec::new(),
            model: "test-model".to_string(),
            auth: AuthScheme::None,
            extra_headers: Vec::new(),
            extra_body: serde_json::Value::Null,
            connect_timeout_ms: 1_000,
            read_timeout_ms: 2_000,
            total_timeout_ms: 3_000,
            max_tokens: 64,
        }
    }

    fn registry(name: &str) -> ProviderRegistry {
        ProviderRegistry::from_profiles(vec![profile(name)]).expect("注册表")
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("whcode-llm-app-test-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("临时目录");
        dir
    }

    /// 建一个会话并返回其 ID（`llm run` 的前置）。
    fn session(conn: &mut duckdb::Connection, now_ms: u64) -> String {
        let row = new_session(
            conn,
            &DefaultPolicy,
            &actor_with_all_capabilities("llm-test"),
            NewSession {
                title: "LLM 测试会话".to_string(),
                model: None,
            },
            now_ms,
        )
        .expect("新建会话");
        row.id
    }

    fn request(session_id: &str, prompt: &str) -> LlmRunRequest {
        LlmRunRequest {
            session_id: session_id.to_string(),
            provider: "p".to_string(),
            prompt: prompt.to_string(),
            model: None,
            max_tokens: None,
            request_id: None,
        }
    }

    #[test]
    fn registry_parses_bare_array_and_wrapper() {
        let bare = r#"[{"name":"p","protocol":"openai",
            "base_url":"https://api.example.com","model":"test-model",
            "auth":{"type":"none"}}]"#;
        assert_eq!(
            ProviderRegistry::from_json(bare).expect("裸数组").names(),
            vec!["p".to_string()]
        );
        let wrapped = format!("{{\"profiles\":{bare}}}");
        assert_eq!(
            ProviderRegistry::from_json(&wrapped)
                .expect("包装对象")
                .names(),
            vec!["p".to_string()]
        );
        let empty = ProviderRegistry::from_json("[]").expect_err("空表必须拒绝");
        assert_eq!(empty.code, ErrorCode::InvalidState);
    }

    #[test]
    fn registry_rejects_duplicate_names_and_oversized_files() {
        let duplicate = ProviderRegistry::from_profiles(vec![profile("p"), profile("p")])
            .expect_err("重名必须拒绝");
        assert_eq!(duplicate.code, ErrorCode::InvalidState);
        assert!(duplicate.message.contains("重复"), "{duplicate}");

        let dir = temp_dir("oversized");
        let path = dir.join("profiles.json");
        // 超过 64 KiB 的内容：解析前先按元数据拒绝（不把大文件读进内存）。
        let padding = " ".repeat(usize::try_from(MAX_PROFILE_FILE_BYTES).expect("上限") + 1);
        std::fs::write(&path, format!("[{padding}]")).expect("写入");
        let err = ProviderRegistry::load(&path).expect_err("超限必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(err.message.contains("上限"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_rejects_unknown_session_provider_and_empty_prompt() {
        let mut conn = migrated();
        let actor = actor_with_all_capabilities("llm-test");
        let policy = DefaultPolicy;
        let registry = registry("p");
        let factory = StubFactory;
        let dir = temp_dir("reject");
        let context = LlmContext {
            registry: Some(&registry),
            registry_error: None,
            factory: &factory,
            cancel_dir: &dir,
        };
        let id = session(&mut conn, 1_700_000_000_000);

        let unknown_session = run(
            &conn,
            &policy,
            &actor,
            &context,
            &request("S-不存在", "hello"),
            1,
        )
        .expect_err("未知会话必须拒绝");
        assert_eq!(unknown_session.code, ErrorCode::NotFound);

        let empty_prompt = run(&conn, &policy, &actor, &context, &request(&id, "  "), 1)
            .expect_err("空提示词必须拒绝");
        assert_eq!(empty_prompt.code, ErrorCode::InvalidState);

        let mut unknown_provider = request(&id, "hello");
        unknown_provider.provider = "not-registered".to_string();
        let err = run(&conn, &policy, &actor, &context, &unknown_provider, 1)
            .expect_err("未知剖面必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(err.message.contains("p"), "错误必须列出已注册剖面：{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_streams_through_factory_with_bounded_events() {
        let mut conn = migrated();
        let actor = actor_with_all_capabilities("llm-test");
        let policy = DefaultPolicy;
        let registry = registry("p");
        let factory = StubFactory;
        let dir = temp_dir("stream");
        let context = LlmContext {
            registry: Some(&registry),
            registry_error: None,
            factory: &factory,
            cancel_dir: &dir,
        };
        let id = session(&mut conn, 1_700_000_000_000);
        let prompt = "a".repeat(100);
        let outcome =
            run(&conn, &policy, &actor, &context, &request(&id, &prompt), 42).expect("桩调用成功");

        assert_eq!(outcome.text, prompt, "正文必须完整拼接");
        assert_eq!(outcome.chunks.len(), 4, "100 字符按 32 切块");
        assert!(outcome.completed);
        assert!(!outcome.truncated);
        assert_eq!(outcome.integrity_gaps, 0);
        assert_eq!(outcome.usage.expect("计量").prompt_tokens, 25);
        assert_eq!(outcome.events.len(), 4 + 2, "开始 + 每 chunk + 完成");
        let serialized_events = serde_json::to_string(&outcome.events).expect("序列化");
        assert!(
            !serialized_events.contains(&prompt),
            "事件不得复制提示词正文（FR-203）"
        );
        assert!(
            !marker_path(&dir, &outcome.request_id, LIVE_SUFFIX).exists(),
            "活跃标记必须在调用结束后清理"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_caps_chunks_and_marks_truncation() {
        let mut conn = migrated();
        let actor = actor_with_all_capabilities("llm-test");
        let policy = DefaultPolicy;
        let registry = registry("p");
        let factory = StubFactory;
        let dir = temp_dir("cap");
        let context = LlmContext {
            registry: Some(&registry),
            registry_error: None,
            factory: &factory,
            cancel_dir: &dir,
        };
        let id = session(&mut conn, 1_700_000_000_000);
        // 40 000 字符 → 1250 chunk，超过 1024 上限。
        let outcome = run(
            &conn,
            &policy,
            &actor,
            &context,
            &request(&id, &"b".repeat(40_000)),
            7,
        )
        .expect("桩调用成功");
        assert!(outcome.truncated, "超限必须显式标记，不得静默截断");
        assert_eq!(outcome.chunks.len(), MAX_STREAM_CHUNKS);
        assert!(outcome.text.len() <= MAX_OUTPUT_BYTES);
        assert!(!outcome.completed, "截断时不应伪造终结 chunk");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_surfaces_pending_factory_error_without_faking_success() {
        let mut conn = migrated();
        let actor = actor_with_all_capabilities("llm-test");
        let policy = DefaultPolicy;
        let registry = registry("p");
        let factory = PendingFactory;
        let dir = temp_dir("pending");
        let context = LlmContext {
            registry: Some(&registry),
            registry_error: None,
            factory: &factory,
            cancel_dir: &dir,
        };
        let id = session(&mut conn, 1_700_000_000_000);
        let err = run(&conn, &policy, &actor, &context, &request(&id, "hello"), 1)
            .expect_err("待接线必须结构化失败");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(err.message.contains("待接线"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancel_requires_live_marker_and_takes_effect_before_next_chunk() {
        let actor = actor_with_all_capabilities("llm-test");
        let policy = DefaultPolicy;
        let registry = registry("p");
        let factory = StubFactory;
        let dir = temp_dir("cancel");
        let context = LlmContext {
            registry: Some(&registry),
            registry_error: None,
            factory: &factory,
            cancel_dir: &dir,
        };

        let missing =
            cancel(&policy, &actor, &context, "no-such-request").expect_err("未知请求必须拒绝");
        assert_eq!(missing.code, ErrorCode::NotFound);

        let marker = CancelMarker::begin(&dir, "req-1").expect("活跃标记");
        let outcome = cancel(&policy, &actor, &context, "req-1").expect("取消请求");
        assert_eq!(outcome.state, "cancel_requested");
        assert_eq!(outcome.effective, "before_next_chunk");
        assert!(marker.is_cancelled(), "取消标记必须被 run 侧读取");
        drop(marker);
        assert!(
            !marker_path(&dir, "req-1", LIVE_SUFFIX).exists()
                && !marker_path(&dir, "req-1", CANCEL_SUFFIX).exists(),
            "活跃与取消标记都必须随调用结束清理"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancelled_run_reports_effective_point() {
        let mut conn = migrated();
        let actor = actor_with_all_capabilities("llm-test");
        let policy = DefaultPolicy;
        let registry = registry("p");
        let factory = StubFactory;
        let dir = temp_dir("cancelled-run");
        let context = LlmContext {
            registry: Some(&registry),
            registry_error: None,
            factory: &factory,
            cancel_dir: &dir,
        };
        let id = session(&mut conn, 1_700_000_000_000);
        // 预先放置取消标记：调用在第一个 chunk 之前停止。
        std::fs::write(marker_path(&dir, "req-pre", CANCEL_SUFFIX), b"cancel").expect("取消标记");
        let mut req = request(&id, &"c".repeat(200));
        req.request_id = Some("req-pre".to_string());
        let err = run(&conn, &policy, &actor, &context, &req, 1).expect_err("取消必须生效");
        assert_eq!(err.code, ErrorCode::InvalidState);
        let details = err.details.expect("details");
        assert_eq!(details["effective"], "before_next_chunk");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn credential_value_never_appears_in_results_or_debug() {
        let mut conn = migrated();
        let actor = actor_with_all_capabilities("llm-test");
        let policy = DefaultPolicy;
        let dir = temp_dir("secret");
        let secret_path = dir.join("api-key.txt");
        std::fs::write(&secret_path, "sk-test-secret\n").expect("凭据文件");
        let mut with_auth = profile("p");
        with_auth.auth = AuthScheme::Header {
            name: "x-api-key".to_string(),
            prefix: String::new(),
            credential: CredentialRef::File(secret_path),
        };
        let registry = ProviderRegistry::from_profiles(vec![with_auth]).expect("注册表");
        let factory = StubFactory;
        let context = LlmContext {
            registry: Some(&registry),
            registry_error: None,
            factory: &factory,
            cancel_dir: &dir,
        };
        let id = session(&mut conn, 1_700_000_000_000);
        let outcome =
            run(&conn, &policy, &actor, &context, &request(&id, "hello"), 1).expect("桩调用成功");
        let serialized = serde_json::to_string(&outcome).expect("序列化");
        assert!(!serialized.contains("sk-test-secret"), "结果不得含密钥");
        assert!(
            !format!("{registry:?}").contains("sk-test-secret"),
            "Debug 不得含密钥"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_capability_is_permission_denied() {
        let mut conn = migrated();
        let registry = registry("p");
        let factory = StubFactory;
        let dir = temp_dir("denied");
        let context = LlmContext {
            registry: Some(&registry),
            registry_error: None,
            factory: &factory,
            cancel_dir: &dir,
        };
        let id = session(&mut conn, 1_700_000_000_000);
        let weak = Actor::new("weak", []);
        let err = run(
            &conn,
            &DefaultPolicy,
            &weak,
            &context,
            &request(&id, "hi"),
            1,
        )
        .expect_err("无能力必须拒绝");
        assert_eq!(err.code, ErrorCode::PermissionDenied);
        assert!(!err.retryable);
        let cancel_err =
            cancel(&DefaultPolicy, &weak, &context, "req").expect_err("取消同样受权限约束");
        assert_eq!(cancel_err.code, ErrorCode::PermissionDenied);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
