//! # whcode-domain
//!
//! 核心领域类型：状态常量、事件类型、门禁结果、错误与审计记录。
//!
//! 依赖原则（任务书 §6.2）：**不**依赖 UI、网络与数据库具体实现。其他 crate 不得
//! 复制本 crate 的状态枚举——状态名在代码、数据库、CUI JSON Schema 与 Web 协议中
//! 必须使用同一套定义（§3.1）。

pub mod audit;
pub mod checkpoint;
pub mod error;
pub mod event;
pub mod gate;
pub mod i18n;
pub mod state;
pub mod tls;
pub mod trace;

pub use audit::{AuditAction, AuditRecord, AuditSink, CollectingAuditSink};
pub use checkpoint::{
    CheckpointPolicy, CheckpointState, CheckpointTrigger, DEFAULT_INTERVAL_SECONDS, DEFAULT_RETAIN,
    MAX_EVERY_STEPS, MAX_INTERVAL_SECONDS, MAX_RETAINED, retain_recent,
};
pub use error::{ErrEnvelope, ErrorCode, OkEnvelope, SCHEMA_VERSION, WhError};
pub use event::{
    EventCriticality, EventKind, EventRecord, MAX_PAYLOAD_BYTES, PersistenceLevel,
    epoch_ms_to_iso8601, iso8601_to_epoch_ms,
};
pub use gate::{GateId, GateResult, StageTransition};
pub use state::{LlmRequestStatus, RuntimeExecutionStatus, SddStage, TaskStatus};
pub use tls::{
    BUILTIN_CA_FILE, CA_BUNDLE_MAX_BYTES, CERT_JSON_MAX_BYTES, CERT_RENEW_WARN_DAYS,
    CERT_VALIDITY_DAYS, CertFileStatus, EXTRA_CA_FILE, InboundMode, MAX_CERT_COUNT, OutboundStatus,
    SEED_MANIFEST_FILE, SERVER_CERT_FILE, SERVER_CERT_MAX_BYTES, SERVER_KEY_FILE,
    SERVER_KEY_MAX_BYTES, SETTINGS_FILE, SETTINGS_SCHEMA_VERSION, ServerCertStatus, TlsSettings,
    TlsStatus, TrustMode,
};
pub use trace::TraceLevel;
