//! 审计记录与写入钩子（任务书 §10.3、FR-312）。
//!
//! 下列操作**必须**有审计记录：手动越过门禁、修改 Constitution、清理或压缩数据库、修改
//! 运行时配置、执行破坏性工具、权限配置变化、数据导出、认证失败的策略性汇总。审计数据
//! **不得**随普通会话 TTL 自动清除，除非管理员显式设置审计保留策略。

use serde::{Deserialize, Serialize};

use crate::error::WhError;

/// 必须审计的动作类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditAction {
    /// 人工强制越过门禁（`spec force-advance`，须带 reason）。
    GateOverride,
    /// 修改 Constitution。
    ConstitutionChange,
    /// 清理/删除历史数据。
    DbPurge,
    /// 数据库压缩或重建。
    DbCompact,
    /// 修改运行时配置（启用/禁用/安装）。
    RuntimeConfigChange,
    /// 执行破坏性工具调用。
    DestructiveTool,
    /// 权限配置变化。
    PermissionChange,
    /// 数据导出。
    DataExport,
    /// 认证失败的策略性汇总（非逐条失败记录）。
    AuthFailureSummary,
}

impl AuditAction {
    /// 稳定序列化名。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GateOverride => "gate_override",
            Self::ConstitutionChange => "constitution_change",
            Self::DbPurge => "db_purge",
            Self::DbCompact => "db_compact",
            Self::RuntimeConfigChange => "runtime_config_change",
            Self::DestructiveTool => "destructive_tool",
            Self::PermissionChange => "permission_change",
            Self::DataExport => "data_export",
            Self::AuthFailureSummary => "auth_failure_summary",
        }
    }
}

impl std::fmt::Display for AuditAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 审计记录（`audit` 表逻辑字段；data-model.md §1）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditRecord {
    /// 记录 ID。
    pub id: String,
    /// 执行者。
    pub actor: String,
    /// 动作类别。
    pub action: AuditAction,
    /// 作用对象（会话、任务、数据库路径、门禁 ID 等）。
    pub target: String,
    /// 附加细节（理由、跳过的门禁、产物版本等；不得含秘密）。
    ///
    /// 对外字段名为 `detail_json`，与 `data-model.md` §1 的 `audit.detail_json` 列一致。
    #[serde(rename = "detail_json")]
    pub detail: serde_json::Value,
    /// Unix epoch 毫秒（UTC）。
    pub ts_ms: u64,
}

/// 审计写入钩子。
///
/// 具体存储实现属 `whcode-store`（M1）；本 trait 让 domain/app 不依赖存储实现，同时使
/// "必须审计"成为可测试的接口约束——实现方返回错误时调用方**不得**把操作标记为成功。
pub trait AuditSink: Send + Sync {
    /// 写入一条审计记录。
    fn record(&self, record: &AuditRecord) -> Result<(), WhError>;
}

/// 内存审计缓冲，供测试与早期原型使用。
///
/// 注意：生产路径必须使用持久化实现（M1/T039）；本类型不满足"不随会话 TTL 清除"的
/// 持久化要求，不得作为发布路径的默认实现。
#[derive(Debug, Default)]
pub struct CollectingAuditSink {
    records: std::sync::Mutex<Vec<AuditRecord>>,
}

impl CollectingAuditSink {
    /// 空缓冲。
    pub fn new() -> Self {
        Self::default()
    }

    /// 当前已记录的审计条目快照。
    pub fn records(&self) -> Vec<AuditRecord> {
        self.records
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// 已记录条目数。
    pub fn len(&self) -> usize {
        self.records.lock().map(|guard| guard.len()).unwrap_or(0)
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl AuditSink for CollectingAuditSink {
    fn record(&self, record: &AuditRecord) -> Result<(), WhError> {
        let mut guard = self
            .records
            .lock()
            .map_err(|_| WhError::internal("审计缓冲区锁中毒：记录可能丢失，操作不得视为成功"))?;
        guard.push(record.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn override_record() -> AuditRecord {
        AuditRecord {
            id: "A-001".to_string(),
            actor: "ykla".to_string(),
            action: AuditAction::GateOverride,
            target: "gate:clarify".to_string(),
            detail: json!({"reason": "M0 平台阻塞，人工放行", "skipped": ["clarify"]}),
            ts_ms: 1_700_000_000_000,
        }
    }

    #[test]
    fn audit_record_carries_actor_reason_target_and_time() {
        let sink = CollectingAuditSink::new();
        sink.record(&override_record()).expect("记录应写入");
        let records = sink.records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].actor, "ykla");
        assert_eq!(records[0].action, AuditAction::GateOverride);
        assert_eq!(records[0].detail["reason"], json!("M0 平台阻塞，人工放行"));
        assert_eq!(records[0].ts_ms, 1_700_000_000_000);
    }

    #[test]
    fn all_required_actions_are_represented() {
        let all = [
            AuditAction::GateOverride,
            AuditAction::ConstitutionChange,
            AuditAction::DbPurge,
            AuditAction::DbCompact,
            AuditAction::RuntimeConfigChange,
            AuditAction::DestructiveTool,
            AuditAction::PermissionChange,
            AuditAction::DataExport,
            AuditAction::AuthFailureSummary,
        ];
        assert_eq!(all.len(), 9);
        assert_eq!(all[2].as_str(), "db_purge");
    }
}
