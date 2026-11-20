//! 运行时启用状态（T395／FR-701—710／ADR-089 批 6）：`runtime_state` 表的
//! 唯一 SQL 落点（ADR-018）。表结构来自 0001 既有骨架（name/enabled/backend/
//! version/installed_at/payload_path）——不新增迁移；表无行即全部关闭
//! （FR-704 默认 off），空闲态零常驻资源。
//!
//! 列语义（v1 口径，ADR-089 已知限制）：
//! - `backend`：执行后端标识，enable 时填运行时名（真实后端装配随 M8 执行链路）；
//! - `version`／`installed_at`／`payload_path`：install 登记（payload 完整
//!   校验流程随 M8 执行链路；本命令面只做哈希比对与登记）。

use duckdb::{Connection, params};
use whcode_domain::{ErrorCode, WhError};

/// 单个运行时的持久化状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeStateRow {
    /// 运行时名。
    pub name: String,
    /// 是否启用（enable/disable 写入）。
    pub enabled: bool,
    /// 执行后端标识（enable 写运行时名；M8 执行链路接管真实装配）。
    pub backend: String,
    /// 已登记版本（install 写入；未安装为空串）。
    pub version: String,
    /// 已登记 payload 路径（install 写入；未安装为 `None`）。
    pub payload_path: Option<String>,
}

const SELECT_COLUMNS: &str = "name, enabled, backend, version, payload_path";

fn rt_err(context: &str, err: duckdb::Error) -> WhError {
    WhError::new(
        ErrorCode::IoError,
        format!("runtime_state {context}失败：{err}"),
    )
}

fn map_row(row: &duckdb::Row<'_>) -> Result<RuntimeStateRow, duckdb::Error> {
    Ok(RuntimeStateRow {
        name: row.get(0)?,
        enabled: row.get(1)?,
        backend: row.get(2)?,
        version: row.get(3)?,
        payload_path: row.get(4)?,
    })
}

/// 读取一个运行时的状态；无行即默认关闭（正确的空，不伪造）。
///
/// # Errors
///
/// 数据库查询失败时返回 [`WhError`]。
pub fn get(conn: &Connection, name: &str) -> Result<Option<RuntimeStateRow>, WhError> {
    let sql = format!("SELECT {SELECT_COLUMNS} FROM runtime_state WHERE name = ?");
    let mut stmt = conn.prepare(&sql).map_err(|err| rt_err("读取", err))?;
    let mut rows = stmt
        .query_map([name], map_row)
        .map_err(|err| rt_err("读取", err))?;
    match rows.next() {
        Some(row) => Some(row.map_err(|err| rt_err("读取", err))).transpose(),
        None => Ok(None),
    }
}

/// 全量读取（按名字排序；清单规模有界）。
///
/// # Errors
///
/// 数据库查询失败时返回 [`WhError`]。
pub fn list(conn: &Connection) -> Result<Vec<RuntimeStateRow>, WhError> {
    let sql = format!("SELECT {SELECT_COLUMNS} FROM runtime_state ORDER BY name");
    let mut stmt = conn.prepare(&sql).map_err(|err| rt_err("读取", err))?;
    let rows = stmt
        .query_map([], map_row)
        .map_err(|err| rt_err("读取", err))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| rt_err("读取", err))?;
    Ok(rows)
}

/// 写启用状态（enable/disable；upsert，保留已登记的安装信息）。
///
/// # Errors
///
/// 数据库写入失败时返回 [`WhError`]。
pub fn set_enabled(conn: &Connection, name: &str, enabled: bool) -> Result<(), WhError> {
    let sql = "INSERT INTO runtime_state (name, enabled, backend, version, installed_at, payload_path) \
               VALUES (?, ?, ?, '', NULL, NULL) \
               ON CONFLICT (name) DO UPDATE SET enabled = excluded.enabled, backend = excluded.backend";
    conn.execute(sql, params![name, enabled, name])
        .map(|_| ())
        .map_err(|err| rt_err("写启用", err))
}

/// 写安装记录（install；upsert，保留 enabled 位）。
///
/// # Errors
///
/// 数据库写入失败时返回 [`WhError`]。
pub fn set_installed(
    conn: &Connection,
    name: &str,
    backend: &str,
    version: &str,
    payload_path: &str,
) -> Result<(), WhError> {
    let sql = "INSERT INTO runtime_state (name, enabled, backend, version, installed_at, payload_path) \
               VALUES (?, FALSE, ?, ?, current_timestamp, ?) \
               ON CONFLICT (name) DO UPDATE SET backend = excluded.backend, \
               version = excluded.version, installed_at = excluded.installed_at, \
               payload_path = excluded.payload_path";
    conn.execute(sql, [name, backend, version, payload_path])
        .map(|_| ())
        .map_err(|err| rt_err("写安装", err))
}
