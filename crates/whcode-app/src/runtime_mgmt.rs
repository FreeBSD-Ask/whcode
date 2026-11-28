//! # 运行时管理命令面（T395／FR-701—710、ADR-089 批 6）
//!
//! `runtime.status`／`runtime.enable`／`runtime.disable`／`runtime.install` 的
//! 应用服务编排：声明（能力等级，FR-702）＋ 探测（仅显式命令路径，界面刷新
//! 路径不探测）＋ 持久化启用状态（`runtime_state` 表，FR-704 默认关闭）。
//!
//! ## 拒绝策略（不伪造启用）
//!
//! - `js`：内置引擎不交付（ADR-024 决策 1）——enable/install 结构化拒绝；
//! - `bsdshell`：M7 延后（ADR-013）——enable/install 结构化拒绝；
//! - `python`／`gitbash`：允许 enable/disable；install 只登记本地 payload 的
//!   版本与哈希（内容哈希经 [`whcode_runtime::payload::fnv64`] 与调用方比对），
//!   不做网络下载（ADR-089 已知限制 2）。
//!
//! 分层：本模块是运行时声明清单与启用策略的**唯一事实源**（自 whcode-bin
//! 下沉，bin 侧 `runtimes` 模块改为 re-export）——doctor／TUI 摘要与 Web
//! `/api/runtimes` 共用同一清单。

use duckdb::Connection;
use serde::Serialize;
use whcode_domain::{AuditAction, AuditRecord, ErrorCode, WhError};
use whcode_runtime::RuntimeProvider;
use whcode_runtime::payload::fnv64;
use whcode_runtime::providers::{BsdShellProvider, GitBashProvider, JsProvider, PythonProvider};
use whcode_store::runtime_state;

use crate::authz::{Actor, Capability, Policy, require};

/// 单个运行时的展示条目（数据字段不翻译：`capability` 用稳定名）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuntimeEntry {
    /// 运行时名。
    pub name: String,
    /// 能力等级稳定名（`full`／`restricted`／`shell_only`）。
    pub capability: &'static str,
    /// 是否可用；`None` 表示本次未探测。
    pub available: Option<bool>,
    /// 未覆盖能力（D-08：缺失项必须显式列出）。
    pub unsupported: Vec<String>,
    /// 是否默认启用（FR-704：当前四项全部为 false）。
    pub default_enabled: bool,
    /// 启用条件（默认关闭时的可操作说明）。
    pub enable_requirement: &'static str,
}

/// 运行时管理命令的输出（声明＋探测＋持久化状态合并）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuntimeStatusView {
    /// 事件 schema 版本。
    pub schema_version: u32,
    /// 运行时名。
    pub name: String,
    /// 能力等级稳定名。
    pub capability: &'static str,
    /// 可用性（`None`＝未探测）。
    pub available: Option<bool>,
    /// 未覆盖能力。
    pub unsupported: Vec<String>,
    /// 持久化启用状态（表无行＝false，FR-704 默认关闭）。
    pub enabled: bool,
    /// 已登记版本（install 写入；未安装为空串）。
    pub version: String,
    /// 已登记 payload 路径。
    pub payload_path: Option<String>,
    /// 默认启用策略（可见性字段，改默认必须走 ADR）。
    pub default_enabled: bool,
    /// 启用条件说明。
    pub enable_requirement: String,
}

/// 四个运行时的声明（顺序固定：js、python、gitbash、bsdshell）。
#[must_use]
pub fn inventory(probe: bool) -> Vec<RuntimeEntry> {
    let providers: [&dyn RuntimeProvider; 4] = [
        &JsProvider::default(),
        &PythonProvider::default(),
        &GitBashProvider::default(),
        &BsdShellProvider::default(),
    ];
    providers
        .iter()
        .map(|provider| {
            let name = provider.name();
            let (default_enabled, enable_requirement) = enable_policy(name);
            RuntimeEntry {
                name: name.to_string(),
                capability: provider.declaration().capability.as_str(),
                available: probe.then(|| provider.is_available()),
                unsupported: provider.declaration().unsupported.clone(),
                default_enabled,
                enable_requirement,
            }
        })
        .collect()
}

/// 运行时默认启用策略（P5.6／FR-784／ADR-024 修订）：**四项全部默认关闭**——
/// 资源铁律（空闲态不得引入常驻进程）、安全边界（执行面显式 opt-in）、
/// 自由原则（默认值可见且可撤销）的交点。未登记的运行时按「默认关闭＋先补
/// ADR」处理，不静默当作可启用。
#[must_use]
pub fn enable_policy(name: &str) -> (bool, &'static str) {
    match name {
        "js" => (
            false,
            "内置引擎不交付（ADR-024 决策 1）：执行请求返回 runtime_unavailable",
        ),
        "python" => (
            false,
            "需安装解释器并显式 opt-in（FR-704）；执行经受控 worker（超时/输出上限/凭据剥离）",
        ),
        "gitbash" => (
            false,
            "需安装 Git Bash payload 并通过来源/版本/哈希/平台四重校验（FR-703）",
        ),
        "bsdshell" => (
            false,
            "M7 延后（ADR-013）：当前不交付，SSH 远程开发目标不要求原生引擎",
        ),
        _ => (
            false,
            "未登记的运行时默认关闭：启用前先补 ADR、能力声明与门禁",
        ),
    }
}

/// enable/install 的结构化拒绝（js／bsdshell 不交付执行面，不伪造启用）。
fn ensure_installable(name: &str) -> Result<(), WhError> {
    match name {
        "python" | "gitbash" => Ok(()),
        "js" => Err(WhError::invalid_state(
            "js 不交付内置引擎（ADR-024 决策 1）：enable/install 拒绝；执行请求返回 runtime_unavailable",
        )),
        "bsdshell" => Err(WhError::invalid_state(
            "bsdshell M7 延后（ADR-013）：enable/install 拒绝；SSH 远程目标不要求原生引擎",
        )),
        other => Err(WhError::new(
            ErrorCode::NotFound,
            format!(
                "未登记的运行时：{other}（清单固定 js／python／gitbash／bsdshell；新增须先补 ADR）"
            ),
        )),
    }
}

/// 查询单个运行时状态（FR-702；`runtime.status`，T395）。
///
/// `probe = true` 启动子进程探测（显式管理命令路径允许；界面刷新路径必须传
/// false——与 doctor／TUI 摘要的口径一致）。
///
/// # Errors
///
/// 权限拒绝、未登记运行时（`not_found`）或存储失败时返回 [`WhError`]。
pub fn status(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    name: &str,
    probe: bool,
) -> Result<RuntimeStatusView, WhError> {
    require(policy, actor, Capability::RuntimeManage)?;
    let entry = inventory(probe)
        .into_iter()
        .find(|entry| entry.name == name)
        .ok_or_else(|| not_found(name))?;
    let state = runtime_state::get(conn, name)?;
    Ok(RuntimeStatusView {
        schema_version: whcode_domain::SCHEMA_VERSION,
        name: entry.name,
        capability: entry.capability,
        available: entry.available,
        unsupported: entry.unsupported,
        enabled: state.as_ref().is_some_and(|row| row.enabled),
        version: state
            .as_ref()
            .map_or(String::new(), |row| row.version.clone()),
        payload_path: state.as_ref().and_then(|row| row.payload_path.clone()),
        default_enabled: entry.default_enabled,
        enable_requirement: entry.enable_requirement.to_string(),
    })
}

/// 启用运行时（FR-704 显式 opt-in；`runtime.enable`，T395）。
///
/// 写 `runtime_state`＋审计；`js`／`bsdshell` 结构化拒绝（不伪造启用）。
/// enable 只写状态不创造能力——执行路径继续按探测结果返回 `runtime_unavailable`。
///
/// # Errors
///
/// 权限拒绝、拒绝策略命中、未登记运行时或存储失败时返回 [`WhError`]。
pub fn enable(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    name: &str,
    now_ms: u64,
) -> Result<RuntimeStatusView, WhError> {
    require(policy, actor, Capability::RuntimeManage)?;
    ensure_installable(name)?;
    runtime_state::set_enabled(conn, name, true)?;
    audit(conn, actor, "runtime.enable", name, now_ms)?;
    status(conn, policy, actor, name, false)
}

/// 停用运行时（可撤销；`runtime.disable`，T395）。
///
/// # Errors
///
/// 权限拒绝、未登记运行时或存储失败时返回 [`WhError`]。
pub fn disable(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    name: &str,
    now_ms: u64,
) -> Result<RuntimeStatusView, WhError> {
    require(policy, actor, Capability::RuntimeManage)?;
    inventory(false)
        .into_iter()
        .find(|entry| entry.name == name)
        .ok_or_else(|| not_found(name))?;
    runtime_state::set_enabled(conn, name, false)?;
    audit(conn, actor, "runtime.disable", name, now_ms)?;
    status(conn, policy, actor, name, false)
}

/// 登记本地 payload 安装（FR-703；`runtime.install`，T395）。
///
/// `payload_path` 指向**本地已取得**的 payload 文件（本命令不做网络下载）；
/// 内容哈希在本地计算（[`fnv64`]）与调用方声明的 `hash` 比对——不一致即拒绝
/// （校验先于登记）。来源白名单与平台兼容性的完整执行面校验随 M8 执行链路
/// （ADR-089 已知限制 2）。
///
/// # Errors
///
/// 权限拒绝、拒绝策略命中、payload 不可读、哈希不一致或存储失败时返回
/// [`WhError`]。
#[allow(clippy::too_many_arguments)]
pub fn install(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    name: &str,
    version: &str,
    declared_hash: &str,
    payload_path: &std::path::Path,
    now_ms: u64,
) -> Result<RuntimeStatusView, WhError> {
    require(policy, actor, Capability::RuntimeManage)?;
    ensure_installable(name)?;
    let bytes = std::fs::read(payload_path).map_err(|err| {
        WhError::new(
            ErrorCode::IoError,
            format!("读取本地 payload 失败（{}）：{err}", payload_path.display()),
        )
    })?;
    let actual = fnv64(&bytes);
    if actual != declared_hash.trim() {
        return Err(WhError::invalid_state(format!(
            "payload 哈希不一致：声明 {declared_hash}，实际 {actual}（FR-703 校验先于登记）"
        )));
    }
    if version.trim().is_empty() {
        return Err(WhError::invalid_state("版本号不能为空（FR-703）"));
    }
    let now = i64::try_from(now_ms).unwrap_or(0);
    let _ = now;
    runtime_state::set_installed(
        conn,
        name,
        name,
        version.trim(),
        &payload_path.to_string_lossy(),
    )?;
    audit(conn, actor, "runtime.install", name, now_ms)?;
    status(conn, policy, actor, name, false)
}

/// 管理动作审计（enable/disable/install 各一条；失败即操作失败）。
fn audit(
    conn: &Connection,
    actor: &Actor,
    action: &str,
    name: &str,
    now_ms: u64,
) -> Result<(), WhError> {
    let record = AuditRecord {
        id: format!("A-{action}-{name}-{now_ms}"),
        actor: actor.id.clone(),
        action: AuditAction::RuntimeConfigChange,
        target: format!("runtime:{name}"),
        detail: serde_json::json!({ "managed_action": action }),
        ts_ms: now_ms,
    };
    conn.execute(
        "INSERT INTO audit (id, actor, action, target, detail_json, ts) \
         VALUES (?, ?, ?, ?, CAST(? AS JSON)::VARIANT, current_timestamp)",
        [
            &record.id,
            &record.actor,
            record.action.as_str(),
            &record.target,
            &record.detail.to_string(),
        ],
    )
    .map(|_| ())
    .map_err(|err| WhError::new(ErrorCode::IoError, format!("runtime 审计写入失败：{err}")))
}

fn not_found(name: &str) -> WhError {
    WhError::new(
        ErrorCode::NotFound,
        format!("未登记的运行时：{name}（清单固定 js／python／gitbash／bsdshell；新增须先补 ADR）"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authz::{DefaultPolicy, actor_with_all_capabilities};
    use whcode_store::{apply_migrations, create_session_with_id};

    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().expect("内存库");
        apply_migrations(&conn).expect("迁移");
        conn
    }

    /// T395：四运行时声明齐全、默认全部关闭（FR-704／P5.6）。
    #[test]
    fn inventory_lists_four_runtimes_all_default_off() {
        let entries = inventory(false);
        assert_eq!(entries.len(), 4);
        assert_eq!(
            entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            vec!["js", "python", "gitbash", "bsdshell"]
        );
        for entry in &entries {
            assert!(!entry.default_enabled, "{} 默认必须关闭", entry.name);
        }
    }

    /// T395：enable 落库＋审计；js/bsdshell 结构化拒绝；disable 可撤销。
    #[test]
    fn enable_persists_and_rejects_undeliverable_runtimes() {
        let conn = migrated();
        create_session_with_id(&conn, "S-1", "运行时", "active", 1_000).expect("创建");
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");

        let js = enable(&conn, &policy, &actor, "js", 1_000).expect_err("js 必须拒绝（ADR-024）");
        assert_eq!(js.code, ErrorCode::InvalidState);
        let bsd = enable(&conn, &policy, &actor, "bsdshell", 1_000)
            .expect_err("bsdshell 必须拒绝（ADR-013）");
        assert_eq!(bsd.code, ErrorCode::InvalidState);

        let view = enable(&conn, &policy, &actor, "python", 1_000).expect("启用");
        assert!(view.enabled);
        let audits: i64 = conn
            .query_row(
                "SELECT count(*) FROM audit WHERE action = 'runtime_config_change'",
                [],
                |row| row.get(0),
            )
            .expect("审计计数");
        assert_eq!(audits, 1, "enable 必须落一条审计");

        let off = disable(&conn, &policy, &actor, "python", 2_000).expect("停用");
        assert!(!off.enabled, "disable 必须可撤销");

        let err =
            enable(&conn, &policy, &actor, "no-such", 3_000).expect_err("未登记必须 not_found");
        assert_eq!(err.code, ErrorCode::NotFound);
    }

    /// T395：install 哈希比对先于登记（FR-703）；哈希不一致拒绝。
    #[test]
    fn install_verifies_payload_hash_before_recording() {
        let conn = migrated();
        create_session_with_id(&conn, "S-1", "安装", "active", 1_000).expect("创建");
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");

        let payload =
            std::env::temp_dir().join(format!("whcode-rt-payload-{}.txt", std::process::id()));
        // Code Scanning 收口：路径字符串不含 ".." 才使用（防临时目录遍历注入）。
        let payload_checked = payload.display().to_string();
        if payload_checked.contains("..") {
            panic!("临时目录路径不得包含 ..");
        }
        let payload = std::path::PathBuf::from(payload_checked);
        std::fs::write(&payload, b"payload-bytes").expect("写 payload");
        let declared = fnv64(b"payload-bytes");

        let view = install(
            &conn, &policy, &actor, "gitbash", "2.50", &declared, &payload, 1_000,
        )
        .expect("安装");
        assert_eq!(view.version, "2.50");
        assert_eq!(
            view.payload_path.as_deref(),
            Some(payload.to_string_lossy().as_ref())
        );
        assert!(!view.enabled, "install 不改变启用位");

        let err = install(
            &conn,
            &policy,
            &actor,
            "gitbash",
            "2.50",
            "fnv1a64:deadbeefdeadbeef",
            &payload,
            2_000,
        )
        .expect_err("哈希不一致必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);

        let err = install(
            &conn, &policy, &actor, "js", "1", &declared, &payload, 3_000,
        )
        .expect_err("js 必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        let _ = std::fs::remove_file(&payload);
    }

    /// T395：无 RuntimeManage 能力同码拒绝；表为空时全部读为关闭（正确的空）。
    #[test]
    fn status_requires_capability_and_defaults_to_off() {
        let conn = migrated();
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        let view = status(&conn, &policy, &actor, "python", false).expect("状态");
        assert!(!view.enabled, "表空＝关闭（FR-704 默认）");
        assert_eq!(view.available, None, "界面路径不探测");

        let weak = Actor::new("weak", []);
        let err =
            status(&conn, &policy, &weak, "python", false).expect_err("无 RuntimeManage 必须拒绝");
        assert_eq!(err.code, ErrorCode::PermissionDenied);
        assert!(!err.retryable);
    }
}
