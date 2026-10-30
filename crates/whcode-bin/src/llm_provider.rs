//! # 真实 LLM Provider 的装配根接线（T167、ADR-031 决策 1）
//!
//! 装配根是**唯一**知道网络依赖的地方：剖面注册表、凭据求值与取消标记目录由
//! `whcode-app::llm` 编排，本模块只提供 [`ProviderFactory`] 实现——把剖面转换成
//! `whcode-llm` 的真实 Provider。
//!
//! 现状（2026-10-25）：
//!
//! - [`whcode_llm::http::client::HttpClient`] 已交付（T163—T166）：阻塞式 `ureq::Agent`、
//!   有界流式读取、chunk 归一、传输快照、时限／重试／取消。
//! - [`LocalProviderFactory::create`] 在 `http` feature 下把剖面交给 `HttpClient`；有工作区
//!   信任材料时经 [`whcode_llm::http::client::tls_config_from_roots`] 注入（ADR-040 出站
//!   档位），`--no-default-features` 下返回结构化 `not_supported`。
//! - 两条路径都不伪造调用成功；真实请求只在 `execute` 时发生。
//!
//! 边界：本模块不做凭据读取（`HttpClient` 经剖面 `auth` 求值），不写日志、不把密钥
//! 放进错误或事件。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(not(feature = "http"))]
use serde_json::json;
use whcode_app::cert::{CertPaths, TrustMaterial, ensure_outbound_initialized, trust_material};
use whcode_app::llm::{
    LlmContext, ProviderFactory, ProviderRegistry, cancel_dir_from_env, profiles_path_from_env,
};
use whcode_domain::WhError;
use whcode_llm::Provider;
use whcode_llm::http::profile::ProviderProfile;

/// 装配根的真实 Provider 工厂。
///
/// 工厂由装配根持有并注入应用层；它不缓存连接、不创建线程（每次调用一次性构造，
/// ADR-031 决策 3）。`trust` 为工作区出站信任材料（ADR-040）；`None` 时使用 `ureq`
/// 默认信任根（仅测试与未初始化路径）。
#[derive(Debug, Clone, Default)]
pub struct LocalProviderFactory {
    trust: Option<TrustMaterial>,
}

impl LocalProviderFactory {
    /// 为工作区构造：确保证书库已初始化并读取出站信任材料。
    ///
    /// # Errors
    ///
    /// 证书库无法初始化或信任材料读失败时返回可操作错误（不静默回落到其他信任源）。
    pub fn for_workspace(workspace: &Path) -> Result<Self, WhError> {
        let paths = CertPaths::for_workspace(workspace);
        ensure_outbound_initialized(&paths, now_ms())?;
        Ok(Self {
            trust: Some(trust_material(&paths)?),
        })
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

impl ProviderFactory for LocalProviderFactory {
    fn create(&self, profile: &ProviderProfile) -> Result<Box<dyn Provider>, WhError> {
        #[cfg(feature = "http")]
        {
            let tls = match &self.trust {
                Some(material) => {
                    let load = whcode_llm::http::tls::load_roots(
                        material.mode,
                        &material.builtin_pem,
                        &material.extra_pem,
                    )?;
                    Some(whcode_llm::http::client::tls_config_from_roots(
                        &load.roots,
                    )?)
                }
                None => None,
            };
            let client = whcode_llm::http::client::HttpClient::with_tls(profile.clone(), tls)?;
            Ok(Box::new(client))
        }
        #[cfg(not(feature = "http"))]
        {
            let _ = profile;
            Err(WhError::not_supported(
                "该构建未启用 http feature：llm run 不装配真实 Provider（默认离线，不联网）",
            )
            .with_details(json!({
                "surface_id": "llm.run",
                "state": "not_supported",
                "feature": "http",
            })))
        }
    }
}

/// `serve --web` 的 LLM 装配上下文（剖面 + 工厂 + 取消目录）。
///
/// 注册表读取失败时保留可操作原因（`registry_error`），不把“未配置”伪装成
/// “配置为空”——Web 请求会据此返回结构化错误。
pub struct ServeLlm {
    /// 剖面注册表；`None` 表示未配置或校验失败。
    pub registry: Option<ProviderRegistry>,
    /// 注册表不可用的原因（不含秘密）。
    pub registry_error: Option<String>,
    /// 真实 Provider 工厂。
    pub factory: LocalProviderFactory,
    /// 取消标记目录（`WHCODE_LLM_CANCEL_DIR` 或系统临时目录）。
    pub cancel_dir: PathBuf,
}

impl ServeLlm {
    /// 由环境变量与工作区装配（启动时解析一次；不做网络请求）。
    ///
    /// # Errors
    ///
    /// 出站证书库无法初始化或信任材料不可读时返回可操作错误。
    pub fn from_env(workspace: &Path) -> Result<Self, WhError> {
        let (registry, registry_error) = match profiles_path_from_env() {
            Ok(path) => match ProviderRegistry::load(&path) {
                Ok(registry) => (Some(registry), None),
                Err(err) => (None, Some(err.message)),
            },
            Err(err) => (None, Some(err.message)),
        };
        Ok(Self {
            registry,
            registry_error,
            factory: LocalProviderFactory::for_workspace(workspace)?,
            cancel_dir: cancel_dir_from_env(),
        })
    }

    /// 构造应用层上下文（借用本结构；每次请求即时构造，不长期持有）。
    #[must_use]
    pub fn context(&self) -> LlmContext<'_> {
        LlmContext {
            registry: self.registry.as_ref(),
            registry_error: self.registry_error.as_deref(),
            factory: &self.factory,
            cancel_dir: &self.cancel_dir,
        }
    }
}

/// TUI 的 LLM 执行器（T167）：复用 [`ServeLlm`] 装配（剖面 + 工厂 + 取消目录），
/// 经 `whcode_app::llm` 真实编排——与 CUI/Web 同一入口、同一权限与会话校验。
/// 连接与 TUI 写命令执行器共享同一条工作区连接（Check 98 口径：单连接为唯一写路径）。
pub struct TuiLlmExecutor {
    llm: ServeLlm,
    conn: std::rc::Rc<std::cell::RefCell<duckdb::Connection>>,
    workspace: PathBuf,
}

impl TuiLlmExecutor {
    /// 由环境变量、工作区与共享连接装配（启动时解析一次；不做网络请求）。
    ///
    /// # Errors
    ///
    /// 出站证书库无法初始化或信任材料不可读时返回可操作错误（TUI 以
    /// `llm_executor: None` 降级，不阻断界面）。
    pub fn from_env(
        workspace: &Path,
        conn: std::rc::Rc<std::cell::RefCell<duckdb::Connection>>,
    ) -> Result<Self, WhError> {
        Ok(Self {
            llm: ServeLlm::from_env(workspace)?,
            conn,
            workspace: workspace.to_path_buf(),
        })
    }
}

impl whcode_tui::LlmExecutor for TuiLlmExecutor {
    fn run(&self, session_id: &str, provider: &str, prompt: &str) -> Result<String, WhError> {
        let outcome = whcode_app::turn::run_turn(
            &mut self.conn.borrow_mut(),
            &whcode_app::DefaultPolicy,
            &whcode_app::actor_with_all_capabilities("tui-local"),
            &self.llm.context(),
            &whcode_app::turn::TurnRequest {
                session_id: session_id.to_string(),
                provider: provider.to_string(),
                prompt: prompt.to_string(),
                model: None,
                max_tokens: None,
                request_id: None,
                iterations_limit: None,
                budget_tokens: None,
                workspace_root: Some(self.workspace.clone()),
            },
            now_ms(),
        )?;
        // 摘要不带正文（事件与日志不复制模型输出；正文只在本状态行显示）。
        Ok(format!(
            "provider={} model={} chunks={} completed={} truncated={} request_id={}",
            outcome.provider,
            outcome.model,
            outcome.chunks.len(),
            outcome.completed,
            outcome.truncated,
            outcome.request_id,
        ))
    }

    fn cancel(&self, request_id: &str) -> Result<String, WhError> {
        let outcome = whcode_app::llm::cancel(
            &whcode_app::DefaultPolicy,
            &whcode_app::actor_with_all_capabilities("tui-local"),
            &self.llm.context(),
            request_id,
        )?;
        Ok(format!(
            "request={} state={} effective={}",
            outcome.request_id, outcome.state, outcome.effective
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(not(feature = "http"))]
    use whcode_domain::ErrorCode;
    use whcode_llm::http::profile::{AuthScheme, Protocol};

    fn profile() -> ProviderProfile {
        ProviderProfile {
            name: "p".to_string(),
            protocol: Protocol::OpenAi,
            base_url: "https://api.example.com".to_string(),
            path: "/v1/chat/completions".to_string(),
            query: Vec::new(),
            model: "m".to_string(),
            auth: AuthScheme::None,
            extra_headers: Vec::new(),
            extra_body: serde_json::Value::Null,
            connect_timeout_ms: 1_000,
            read_timeout_ms: 1_000,
            total_timeout_ms: 1_000,
            max_tokens: 32,
        }
    }

    /// 装配路径：`http` 开启时真实构造 Provider（不发起请求）；关闭时结构化
    /// `not_supported`（不联网、不伪造成功）。
    #[test]
    fn factory_builds_provider_or_reports_disabled_feature() {
        #[cfg(feature = "http")]
        {
            let provider = LocalProviderFactory::default()
                .create(&profile())
                .expect("http feature 下应构造真实 Provider");
            assert_eq!(provider.name(), "p");
            assert!(
                provider.is_available(),
                "构造成功即为可用（请求仍只在 execute 时发生）"
            );
        }
        #[cfg(not(feature = "http"))]
        {
            let err = LocalProviderFactory::default()
                .create(&profile())
                .expect_err("未启用 feature 必须结构化失败");
            assert_eq!(err.code, ErrorCode::NotSupported);
            assert!(err.message.contains("http feature"), "{err}");
            assert!(!err.retryable, "装配缺失不可重试（需改配置或交付实现）");
        }
    }
}
