//! # 工作区导出/导入（T040、FR-508、§7.4）
//!
//! 导出格式 `whcode-export` v1：UTF-8 JSON 文件，顶层结构
//! `{"format", "format_version", "manifest", "tables", "chunk_checksums"}`。
//!
//! ## 范围
//!
//! 导出 11 张业务表（[`EXPORT_TABLES`]）；**不含** [`EXCLUDED_TABLES`]：
//!
//! - `audit`：安全审计随原库保留，不随导出文件扩散（§10.3）；
//! - `runtime_state`：机器本地运行时配置，不属于工作区数据；
//! - `schema_migrations`：由目标库自身的迁移管理，导入方不得照搬。
//!
//! ## 校验（Check 40、V-106）
//!
//! 导入前依次执行，任一失败即整体拒绝、不做部分导入：
//!
//! 1. 文件内容哈希（调用方提供预期值时）；
//! 2. 格式标识与 `format_version`；
//! 3. schema 兼容：目标库必须已迁移，且导出 `schema_version` ≤ 目标版本
//!    （v1—v3 数据表结构一致，旧导出可入新库；反之拒绝）；
//! 4. 分块校验和：每表每 1000 行一块的 FNV-1a 64 清单逐块复核；
//! 5. 列一致：行键与目标表 catalog 列名完全一致；
//! 6. 主键/唯一键冲突：导出行在目标库已存在即拒绝并列出冲突值。
//!
//! ## 边界与纪律
//!
//! - 校验和为 FNV-1a 64，针对**意外损坏**的完整性检测，不防蓄意篡改；防篡改须外部签名。
//! - 导出按行流式写出（单条 `SELECT`，DuckDB 按内部向量分块交付，无 `OFFSET`），
//!   内存不随行数线性增长；导入把文件整体解析进内存，规模以数据量为界——超大库
//!   的迁移走 compact 而非导入。
//! - 导出成功后写 `data_export` 审计（§10.3 强制清单）；导入不在强制审计清单内，
//!   不写审计——此为已记录的决策。
//! - 时间戳列在 SQL 内 `CAST` 为 VARCHAR 导出、`CAST` 回 TIMESTAMP 导入，格式由
//!   DuckDB 自己负责，本模块不做日期运算。

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use duckdb::Connection;
use duckdb::types::{Value, ValueRef};
use serde::{Deserialize, Serialize};
use whcode_domain::{AuditAction, ErrorCode, WhError};

use crate::schema::schema_version;

/// 导出格式标识。
pub const EXPORT_FORMAT: &str = "whcode-export";

/// 导出格式版本；不兼容的结构变更必须递增。
pub const EXPORT_FORMAT_VERSION: u32 = 1;

/// 导出与导入的表清单（亦为导入插入顺序：先父后子）。
pub const EXPORT_TABLES: &[&str] = &[
    "workspace_meta",
    "sessions",
    "tasks",
    "checkpoints",
    "runtime_executions",
    "tool_executions",
    "stage_artifacts",
    "events",
    "messages",
    "context_windows",
    "session_overview",
];

/// 明确排除的表及理由见模块文档。
pub const EXCLUDED_TABLES: &[&str] = &["audit", "runtime_state", "schema_migrations"];

/// 分块校验和的块大小（行数）。
pub const ROWS_PER_CHUNK: usize = 1000;

/// 单行导出数据：列名 → JSON 值（`BTreeMap` 保证序列化确定性，分块校验和依赖它）。
pub type Row = BTreeMap<String, serde_json::Value>;

/// 各表用于冲突检测的主键/唯一键列。
pub const TABLE_KEY_COLUMNS: &[(&str, &[&str])] = &[
    ("workspace_meta", &["workspace_id"]),
    ("sessions", &["id"]),
    ("tasks", &["id"]),
    ("checkpoints", &["id"]),
    ("runtime_executions", &["id"]),
    ("tool_executions", &["id"]),
    ("stage_artifacts", &["id"]),
    ("events", &["seq", "id"]),
    ("messages", &["id"]),
    ("context_windows", &["id"]),
    ("session_overview", &["session_id"]),
];

/// 导出文件清单。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportManifest {
    /// 导出时源库的 schema 版本。
    pub schema_version: u32,
    /// 工作区 ID（`workspace_meta` 为空时为 `None`）。
    pub workspace_id: Option<String>,
    /// 导出时刻（Unix 毫秒，由调用方传入）。
    pub exported_at_ms: u64,
    /// 各表行数清单。
    pub counts: BTreeMap<String, i64>,
}

/// 导出文件整体结构。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportFile {
    /// 格式标识（必须等于 [`EXPORT_FORMAT`]）。
    pub format: String,
    /// 格式版本（必须等于 [`EXPORT_FORMAT_VERSION`]）。
    pub format_version: u32,
    /// 清单。
    pub manifest: ExportManifest,
    /// 表数据：表名 → 行数组。
    pub tables: BTreeMap<String, Vec<Row>>,
    /// 分块校验和：表名 → 逐块 FNV-1a 64。
    pub chunk_checksums: BTreeMap<String, Vec<String>>,
}

/// 导出结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportReport {
    /// 写出的文件路径。
    pub path: String,
    /// 文件清单。
    pub manifest: ExportManifest,
    /// 整个文件字节的 FNV-1a 64（意外损坏检测）。
    pub content_hash: String,
}

/// 导入结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImportReport {
    /// 各表实际插入行数。
    pub inserted: BTreeMap<String, i64>,
}

/// FNV-1a 64 增量哈希（与 `schema::checksum` 同算法、同标签）。
struct Fnv64 {
    hash: u64,
}

impl Fnv64 {
    fn new() -> Self {
        Self {
            hash: 0xcbf2_9ce4_8422_2325,
        }
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.hash ^= u64::from(*byte);
            self.hash = self.hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    fn finish(self) -> String {
        format!("fnv1a64:{:016x}", self.hash)
    }
}

/// 边写文件边算哈希的 `Write` 包装。
struct HashingWriter<W: Write> {
    inner: W,
    hash: Fnv64,
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.hash.write(buf);
        self.inner.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn export_err(context: &str, err: impl std::fmt::Display) -> WhError {
    WhError::new(ErrorCode::IoError, format!("{context}：{err}"))
}

fn invalid(context: &str) -> WhError {
    WhError::new(ErrorCode::InvalidState, context.to_string())
}

/// 双引号包裹标识符并转义内部双引号。
fn quote_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// 目标列是否按时间戳处理（导出 CAST 为 VARCHAR，导入 CAST 回原类型）。
fn is_timestampish(data_type: &str) -> bool {
    data_type.contains("TIMESTAMP")
}

/// 目标列是否为 VARIANT（schema v6 的 `events.payload_json`，ADR-034）。
///
/// 导出必须显式转回 JSON 文本，导入必须显式从 JSON 解析；两侧都不能依赖
/// 隐式转换——直接把文本 CAST 成 VARIANT 会得到包着 JSON 的 VARCHAR 变体。
fn is_variant(data_type: &str) -> bool {
    data_type == "VARIANT"
}

/// 预检库已迁移（`schema_migrations` 表存在），否则给出明确的未迁移错误。
fn require_migrated(conn: &Connection, side: &str) -> Result<(), WhError> {
    let present: i64 = conn
        .query_row(
            "SELECT count(*) FROM information_schema.tables
             WHERE table_schema = 'main' AND table_name = 'schema_migrations'",
            [],
            |row| row.get(0),
        )
        .map_err(|err| export_err("检查迁移状态", err))?;
    if present == 0 {
        Err(invalid(&format!("{side}尚未迁移（schema 版本 0）")))
    } else {
        Ok(())
    }
}

/// 读取表的 catalog 列（列名、类型，按 column_index 排序）。
fn table_columns(conn: &Connection, table: &str) -> Result<Vec<(String, String)>, WhError> {
    let mut stmt = conn
        .prepare(
            "SELECT column_name, data_type FROM duckdb_columns()
             WHERE schema_name = 'main' AND table_name = ? ORDER BY column_index",
        )
        .map_err(|err| export_err("读取列目录", err))?;
    let rows = stmt
        .query_map([table], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|err| export_err("读取列目录", err))?;
    let mut columns = Vec::new();
    for row in rows {
        columns.push(row.map_err(|err| export_err("读取列目录", err))?);
    }
    if columns.is_empty() {
        return Err(invalid(&format!("表 {table} 不存在或没有列")));
    }
    Ok(columns)
}

/// 把 DuckDB 值转为 JSON；schema 之外的类型显式报错，不静默丢弃。
fn value_ref_to_json(column: &str, value: ValueRef<'_>) -> Result<serde_json::Value, WhError> {
    match value {
        ValueRef::Null => Ok(serde_json::Value::Null),
        ValueRef::Boolean(flag) => Ok(serde_json::Value::Bool(flag)),
        ValueRef::TinyInt(v) => {
            serde_json::to_value(i64::from(v)).map_err(|err| export_err("序列化整数", err))
        }
        ValueRef::SmallInt(v) => {
            serde_json::to_value(i64::from(v)).map_err(|err| export_err("序列化整数", err))
        }
        ValueRef::Int(v) => {
            serde_json::to_value(i64::from(v)).map_err(|err| export_err("序列化整数", err))
        }
        ValueRef::BigInt(v) => serde_json::to_value(v).map_err(|err| export_err("序列化整数", err)),
        ValueRef::Text(bytes) => {
            let text = std::str::from_utf8(bytes)
                .map_err(|err| export_err(&format!("列 {column} 含非 UTF-8 文本"), err))?;
            Ok(serde_json::Value::String(text.to_string()))
        }
        // 时间戳已在 SELECT 内 CAST 为 VARCHAR；到达此分支说明列目录与 SELECT 不一致。
        ValueRef::Timestamp(..) => Err(invalid(&format!(
            "列 {column} 返回未转换的时间戳：导出 SELECT 与列目录不一致"
        ))),
        _ => Err(invalid(&format!(
            "列 {column} 的类型不受导出支持；schema 只允许文本/布尔/整数/时间戳"
        ))),
    }
}

/// 单表流式导出：单条 SELECT（无 OFFSET），逐行转 JSON 写出，每 1000 行记一块校验和。
fn export_table(
    conn: &Connection,
    writer: &mut HashingWriter<std::io::BufWriter<std::fs::File>>,
    table: &str,
    columns: &[(String, String)],
) -> Result<Vec<String>, WhError> {
    let select_list = columns
        .iter()
        .map(|(name, data_type)| {
            if is_timestampish(data_type) {
                format!(
                    "CAST({} AS VARCHAR) AS {}",
                    quote_identifier(name),
                    quote_identifier(name)
                )
            } else if is_variant(data_type) {
                format!(
                    "CAST(CAST({} AS JSON) AS VARCHAR) AS {}",
                    quote_identifier(name),
                    quote_identifier(name)
                )
            } else {
                quote_identifier(name)
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!("SELECT {select_list} FROM {}", quote_identifier(table));
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|err| export_err(&format!("准备导出 {table}"), err))?;
    let column_names: Vec<String> = columns.iter().map(|(name, _)| name.clone()).collect();
    let mut rows = stmt
        .query([])
        .map_err(|err| export_err(&format!("导出 {table}"), err))?;

    let mut chunk_checksums = Vec::new();
    let mut chunk = Fnv64::new();
    let mut chunk_len = 0usize;
    let mut first_row = true;
    while let Some(row) = rows
        .next()
        .map_err(|err| export_err(&format!("导出 {table}"), err))?
    {
        let mut map = Row::new();
        for (index, name) in column_names.iter().enumerate() {
            let value = row
                .get_ref(index)
                .map_err(|err| export_err(&format!("读取 {table}.{name}"), err))?;
            map.insert(name.clone(), value_ref_to_json(name, value)?);
        }
        let serialized = serde_json::to_vec(&map).map_err(|err| export_err("序列化行", err))?;
        chunk.write(&serialized);
        chunk_len += 1;
        if chunk_len == ROWS_PER_CHUNK {
            chunk_checksums.push(chunk.finish());
            chunk = Fnv64::new();
            chunk_len = 0;
        }
        let separator: &[u8] = if first_row { b" " } else { b", " };
        writer
            .write_all(separator)
            .and_then(|_| serde_json::to_writer(&mut *writer, &map).map_err(std::io::Error::from))
            .map_err(|err| export_err(&format!("写出 {table} 行"), err))?;
        first_row = false;
    }
    if chunk_len > 0 {
        chunk_checksums.push(chunk.finish());
    }
    Ok(chunk_checksums)
}

/// 导出整个工作区到 `out_path`（UTF-8 JSON），成功后写 `data_export` 审计。
///
/// # Errors
///
/// 列目录缺失、行含不受支持类型、文件写出或审计写入失败时返回 [`WhError`]；
/// 审计失败时文件已在磁盘上，调用方应删除。
pub fn export_workspace(
    conn: &Connection,
    out_path: &Path,
    actor: &str,
    now_ms: u64,
) -> Result<ExportReport, WhError> {
    require_migrated(conn, "源库")?;
    let current_schema = schema_version(conn)?;
    if current_schema == 0 {
        return Err(invalid("源库尚未迁移（schema 版本 0），无可导出数据"));
    }
    let workspace_id: Option<String> = conn
        .query_row(
            "SELECT workspace_id FROM workspace_meta LIMIT 1",
            [],
            |row| row.get(0),
        )
        .map(Some)
        .or_else(|err| match err {
            duckdb::Error::QueryReturnedNoRows => Ok(None),
            other => Err(export_err("读取工作区 ID", other)),
        })?;

    let mut counts = BTreeMap::new();
    for table in EXPORT_TABLES {
        let rows: i64 = conn
            .query_row(
                &format!("SELECT count(*) FROM {}", quote_identifier(table)),
                [],
                |row| row.get(0),
            )
            .map_err(|err| export_err(&format!("统计 {table}"), err))?;
        counts.insert((*table).to_string(), rows);
    }
    let manifest = ExportManifest {
        schema_version: current_schema,
        workspace_id,
        exported_at_ms: now_ms,
        counts,
    };

    let file = std::fs::File::create(out_path)
        .map_err(|err| export_err(&format!("创建 {}", out_path.display()), err))?;
    let mut writer = HashingWriter {
        inner: std::io::BufWriter::new(file),
        hash: Fnv64::new(),
    };

    let mut chunk_checksums: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let write_err = (|| -> Result<(), WhError> {
        writer
            .write_all(b"{\"format\":")
            .and_then(|_| {
                serde_json::to_writer(&mut writer, &EXPORT_FORMAT).map_err(std::io::Error::from)
            })
            .and_then(|_| writer.write_all(b",\"format_version\":"))
            .and_then(|_| {
                serde_json::to_writer(&mut writer, &EXPORT_FORMAT_VERSION)
                    .map_err(std::io::Error::from)
            })
            .and_then(|_| writer.write_all(b",\"manifest\":"))
            .and_then(|_| {
                serde_json::to_writer(&mut writer, &manifest).map_err(std::io::Error::from)
            })
            .and_then(|_| writer.write_all(b",\"tables\":{"))
            .map_err(|err| export_err("写出文件头", err))?;

        for (index, table) in EXPORT_TABLES.iter().enumerate() {
            let columns = table_columns(conn, table)?;
            if index > 0 {
                writer
                    .write_all(b",")
                    .map_err(|err| export_err("写出表分隔符", err))?;
            }
            writer
                .write_all(serde_json::to_string(table).expect("表名序列化").as_bytes())
                .and_then(|_| writer.write_all(b":["))
                .map_err(|err| export_err(&format!("写出 {table} 头"), err))?;
            let checksums = export_table(conn, &mut writer, table, &columns)?;
            writer
                .write_all(b"]")
                .map_err(|err| export_err(&format!("收尾 {table}"), err))?;
            chunk_checksums.insert((*table).to_string(), checksums);
        }

        writer
            .write_all(b"},\"chunk_checksums\":")
            .and_then(|_| {
                serde_json::to_writer(&mut writer, &chunk_checksums).map_err(std::io::Error::from)
            })
            .and_then(|_| writer.write_all(b"}"))
            .and_then(|_| writer.flush())
            .map_err(|err| export_err("写出文件尾", err))?;
        Ok(())
    })();

    write_err?;
    let content_hash = writer.hash.finish();

    let detail = serde_json::json!({
        "format": EXPORT_FORMAT,
        "format_version": EXPORT_FORMAT_VERSION,
        "schema_version": manifest.schema_version,
        "counts": manifest.counts,
        "content_hash": content_hash,
        "excluded_tables": EXCLUDED_TABLES,
    });
    conn.execute(
        "INSERT INTO audit (id, actor, action, target, detail_json, ts)
         VALUES (?, ?, ?, ?, ?, current_timestamp)",
        duckdb::params![
            format!("export-{now_ms}-{content_hash}"),
            actor,
            AuditAction::DataExport.as_str(),
            out_path.display().to_string(),
            detail.to_string(),
        ],
    )
    .map_err(|err| export_err("写入导出审计", err))?;

    Ok(ExportReport {
        path: out_path.display().to_string(),
        manifest,
        content_hash,
    })
}

/// 把 JSON 行按目标列类型转为绑定值；类型不符显式报错。
fn json_to_bind(column: &str, value: &serde_json::Value) -> Result<Value, WhError> {
    match value {
        serde_json::Value::Null => Ok(Value::Null),
        serde_json::Value::Bool(flag) => Ok(Value::Boolean(*flag)),
        serde_json::Value::Number(number) => {
            if let Some(int) = number.as_i64() {
                Ok(Value::BigInt(int))
            } else {
                Err(invalid(&format!(
                    "列 {column} 的数值越界或不为整数：{number}"
                )))
            }
        }
        serde_json::Value::String(text) => Ok(Value::Text(text.clone())),
        other => Err(invalid(&format!("列 {column} 的值类型不受支持：{other}"))),
    }
}

/// 每表冲突检查列的查询：返回目标库中已存在的冲突值（至多 `limit` 个）。
fn find_collisions(
    conn: &Connection,
    table: &str,
    column: &str,
    values: &[serde_json::Value],
    limit: usize,
) -> Result<Vec<String>, WhError> {
    let mut found = Vec::new();
    for chunk in values.chunks(crate::purge::IDS_PER_QUERY) {
        let mut binds: Vec<Value> = Vec::with_capacity(chunk.len());
        for value in chunk {
            binds.push(json_to_bind(column, value)?);
        }
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let sql = format!(
            "SELECT CAST({} AS VARCHAR) FROM {} WHERE {} IN ({placeholders}) LIMIT {limit}",
            quote_identifier(column),
            quote_identifier(table),
            quote_identifier(column)
        );
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|err| export_err(&format!("冲突检查 {table}.{column}"), err))?;
        let rows = stmt
            .query_map(duckdb::params_from_iter(binds.iter()), |row| {
                row.get::<_, String>(0)
            })
            .map_err(|err| export_err(&format!("冲突检查 {table}.{column}"), err))?;
        for row in rows {
            found.push(row.map_err(|err| export_err("冲突检查", err))?);
            if found.len() >= limit {
                return Ok(found);
            }
        }
    }
    Ok(found)
}

/// 校验导出行的列与目标 catalog 完全一致。
fn check_columns_match(
    table: &str,
    rows: &[Row],
    catalog: &[(String, String)],
) -> Result<(), WhError> {
    let expected: std::collections::BTreeSet<&str> =
        catalog.iter().map(|(name, _)| name.as_str()).collect();
    for row in rows {
        let actual: std::collections::BTreeSet<&str> = row.keys().map(String::as_str).collect();
        if actual != expected {
            return Err(invalid(&format!(
                "表 {table} 的行列集合与目标库不一致：导出 {} 项，目标 {} 项",
                actual.len(),
                expected.len()
            )));
        }
    }
    Ok(())
}

/// 导入前校验并插入全部数据（单一事务）；任一校验失败即整体拒绝。
///
/// `expected_content_hash` 非空时先核对文件字节哈希。
///
/// # Errors
///
/// 格式/版本不兼容、schema 不兼容、分块校验和不符、列不一致、主键或唯一键冲突、
/// 或数据库读写失败时返回 [`WhError`]。
pub fn import_workspace(
    conn: &mut Connection,
    in_path: &Path,
    expected_content_hash: Option<&str>,
) -> Result<ImportReport, WhError> {
    let bytes = std::fs::read(in_path)
        .map_err(|err| export_err(&format!("读取 {}", in_path.display()), err))?;
    if let Some(expected) = expected_content_hash {
        let mut hash = Fnv64::new();
        hash.write(&bytes);
        let actual = hash.finish();
        if actual != expected {
            return Err(invalid(&format!(
                "导出文件内容哈希不符：预期 {expected}，实际 {actual}"
            )));
        }
    }

    let file: ExportFile = serde_json::from_slice(&bytes)
        .map_err(|err| WhError::parse_error(format!("导出文件解析失败：{err}")))?;
    if file.format != EXPORT_FORMAT || file.format_version != EXPORT_FORMAT_VERSION {
        return Err(invalid(&format!(
            "导出格式不兼容：预期 {EXPORT_FORMAT} v{EXPORT_FORMAT_VERSION}，实际 {} v{}",
            file.format, file.format_version
        )));
    }

    require_migrated(conn, "目标库")?;
    let target_schema = schema_version(conn)?;
    if target_schema == 0 {
        return Err(invalid("目标库尚未迁移（schema 版本 0），拒绝导入"));
    }
    if file.manifest.schema_version > target_schema {
        return Err(invalid(&format!(
            "导出 schema 版本 {} 高于目标库版本 {}，不兼容；请先升级目标库",
            file.manifest.schema_version, target_schema
        )));
    }

    for table in EXPORT_TABLES {
        let Some(rows) = file.tables.get(*table) else {
            return Err(invalid(&format!("导出文件缺少表 {table}")));
        };
        let Some(checksums) = file.chunk_checksums.get(*table) else {
            return Err(invalid(&format!("导出文件缺少表 {table} 的分块校验和")));
        };
        let mut recomputed = Vec::new();
        for chunk in rows.chunks(ROWS_PER_CHUNK) {
            // 与导出侧一致：逐行序列化后按块拼接哈希（BTreeMap 保证字节确定性）。
            let mut hash = Fnv64::new();
            for row in chunk {
                let serialized =
                    serde_json::to_vec(row).map_err(|err| export_err("重算分块校验和", err))?;
                hash.write(&serialized);
            }
            recomputed.push(hash.finish());
        }
        if &recomputed != checksums {
            return Err(invalid(&format!(
                "表 {table} 分块校验和不符：文件 {} 块，重算 {} 块或内容漂移",
                checksums.len(),
                recomputed.len()
            )));
        }
    }

    for &(table, key_columns) in TABLE_KEY_COLUMNS {
        let rows = file
            .tables
            .get(table)
            .ok_or_else(|| invalid(&format!("导出文件缺少表 {table}")))?;
        let catalog = table_columns(conn, table)?;
        check_columns_match(table, rows, &catalog)?;
        for column in key_columns {
            let mut values = Vec::with_capacity(rows.len());
            for row in rows {
                let value = row
                    .get(*column)
                    .ok_or_else(|| invalid(&format!("表 {table} 的行缺少键列 {column}")))?;
                values.push(value.clone());
            }
            let collisions = find_collisions(conn, table, column, &values, 20)?;
            if !collisions.is_empty() {
                return Err(invalid(&format!(
                    "表 {table} 的键列 {column} 在目标库已存在 {} 个冲突（示例：{}）；导入拒绝，不重复创建（V-106）",
                    collisions.len(),
                    collisions.join(", ")
                )));
            }
        }
    }

    let tx = conn
        .transaction()
        .map_err(|err| export_err("开始导入事务", err))?;
    let mut inserted = BTreeMap::new();
    for table in EXPORT_TABLES {
        let rows = file
            .tables
            .get(*table)
            .ok_or_else(|| invalid(&format!("导出文件缺少表 {table}")))?;
        if rows.is_empty() {
            inserted.insert((*table).to_string(), 0);
            continue;
        }
        let catalog = table_columns(&tx, table)?;
        let column_list = catalog
            .iter()
            .map(|(name, _)| quote_identifier(name))
            .collect::<Vec<_>>()
            .join(", ");
        let cast_values = catalog
            .iter()
            .map(|(_, data_type)| {
                if is_variant(data_type) {
                    // 导出文件里载荷是 JSON 文本，必须先解析为 JSON 再转 VARIANT。
                    "CAST(CAST(? AS JSON) AS VARIANT)".to_string()
                } else {
                    format!("CAST(? AS {data_type})")
                }
            })
            .collect::<Vec<_>>()
            .join(", ");

        let mut count = 0i64;
        for chunk in rows.chunks(64) {
            let row_placeholder = format!("({cast_values})");
            let values_sql = vec![row_placeholder; chunk.len()].join(", ");
            let sql = format!(
                "INSERT INTO {} ({column_list}) VALUES {values_sql}",
                quote_identifier(table)
            );
            let mut binds: Vec<Value> = Vec::with_capacity(chunk.len() * catalog.len());
            for row in chunk {
                for (name, _) in &catalog {
                    let value = row
                        .get(name)
                        .ok_or_else(|| invalid(&format!("表 {table} 的行缺少列 {name}")))?;
                    binds.push(json_to_bind(name, value)?);
                }
            }
            let affected = tx
                .execute(sql.as_str(), duckdb::params_from_iter(binds.iter()))
                .map_err(|err| export_err(&format!("导入 {table}"), err))?;
            count += i64::try_from(affected).unwrap_or(i64::MAX);
        }
        inserted.insert((*table).to_string(), count);
    }
    tx.commit().map_err(|err| export_err("提交导入事务", err))?;
    Ok(ImportReport { inserted })
}

/// 目标库是否已有工作区数据：`sessions`/`events`/`tasks` 任一有行即非空。
///
/// 恢复（[`import_workspace`]）的空库前置校验——拒绝发生在任何写入之前。
/// 尚未迁移的库（三表均不存在）视为空：迁移缺失由 `import_workspace` 自己给出
/// 明确的 `invalid_state`，本原语不抢先报存储错误。
///
/// # Errors
///
/// 数据库查询失败时返回 [`WhError`]。
pub fn has_any_workspace_data(conn: &Connection) -> Result<bool, WhError> {
    let present: i64 = conn
        .query_row(
            "SELECT count(*) FROM information_schema.tables
             WHERE table_schema = 'main'
               AND table_name IN ('sessions', 'events', 'tasks')",
            [],
            |row| row.get(0),
        )
        .map_err(|err| export_err("检查工作区表是否存在", err))?;
    if present < 3 {
        return Ok(false);
    }
    let total: i64 = conn
        .query_row(
            "SELECT (SELECT count(*) FROM sessions)
                  + (SELECT count(*) FROM events)
                  + (SELECT count(*) FROM tasks)",
            [],
            |row| row.get(0),
        )
        .map_err(|err| export_err("检查目标库是否为空", err))?;
    Ok(total > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::apply_migrations;
    use duckdb::params;

    fn temp_path(tag: &str) -> std::path::PathBuf {
        crate::test_util::dir(tag).join(format!("{tag}.json"))
    }

    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().expect("内存数据库");
        apply_migrations(&conn).expect("迁移");
        conn
    }

    fn seed_workspace(conn: &Connection, session_id: &str, event_count: i64) {
        conn.execute(
            "INSERT INTO workspace_meta (workspace_id, schema_version, created_at)
             VALUES ('WS-1', ?, current_timestamp)",
            params![crate::schema::STORE_SCHEMA_VERSION as i64],
        )
        .expect("插入工作区元数据");
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, last_active_at, model, archived, status)
             VALUES (?, '导出测试', current_timestamp, current_timestamp, NULL, FALSE, 'idle')",
            params![session_id],
        )
        .expect("插入会话");
        conn.execute(
            "INSERT INTO tasks (id, session_id, title, stage, status, parent_id,
                                acceptance_json, retry_count, updated_at)
             VALUES ('T-1', ?, '任务', 'plan', 'pending', NULL, '[]', 0, current_timestamp)",
            params![session_id],
        )
        .expect("插入任务");
        for seq in 1..=event_count {
            conn.execute(
                "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                                     kind, payload_json, ts, persistence_level)
                 VALUES (?, ?, ?, 'T-1', NULL, NULL, 'StreamingChunk', CAST(? AS JSON)::VARIANT,
                         current_timestamp, 'redacted')",
                params![
                    seq,
                    format!("E-{seq}"),
                    session_id,
                    format!("{{\"n\":{seq}}}")
                ],
            )
            .expect("插入事件");
        }
        conn.execute(
            "INSERT INTO messages (id, session_id, role, content_ref, token_count, ts, parent_id)
             VALUES ('M-1', ?, 'user', 'ref', 7, current_timestamp, NULL)",
            params![session_id],
        )
        .expect("插入消息");
        conn.execute(
            "INSERT INTO checkpoints (id, task_id, step, delta_json, message_cursor,
                                      artifact_refs_json, ts)
             VALUES ('C-1', 'T-1', 1, '{}', NULL, '[]', current_timestamp)",
            [],
        )
        .expect("插入检查点");
        conn.execute(
            "INSERT INTO tool_executions (id, tool_call_id, state, input_hash, started_at,
                                          finished_at, output_ref, side_effect_ref)
             VALUES ('X-1', 'call-1', 'completed', 'h', current_timestamp, current_timestamp,
                     'o', 's')",
            [],
        )
        .expect("插入工具执行");
        conn.execute(
            "INSERT INTO stage_artifacts (id, session_id, task_id, stage, artifact_type,
                                          content, refs_json, parser_version, validated,
                                          version, created_at)
             VALUES ('A-1', ?, 'T-1', 'plan', 'spec', 'c', '[]', 'v1', TRUE, 1, current_timestamp)",
            params![session_id],
        )
        .expect("插入阶段产物");
        conn.execute(
            "INSERT INTO context_windows (id, session_id, window_start_seq, window_end_seq,
                                          total_tokens, summary_ref, created_at)
             VALUES ('W-1', ?, 1, 3, 30, NULL, current_timestamp)",
            params![session_id],
        )
        .expect("插入上下文窗口");
        conn.execute(
            "INSERT INTO session_overview (session_id, by_stage_json, total, done, blocked,
                                           refreshed_at)
             VALUES (?, '{}', 1, 0, 0, current_timestamp)",
            params![session_id],
        )
        .expect("插入会话概览");
    }

    fn read_file_json(path: &Path) -> serde_json::Value {
        let bytes = std::fs::read(path).expect("读取导出文件");
        serde_json::from_slice(&bytes).expect("解析导出文件")
    }

    fn write_file_json(path: &Path, value: &serde_json::Value) {
        std::fs::write(path, serde_json::to_vec(value).expect("序列化")).expect("写出");
    }

    #[test]
    fn export_import_round_trip_preserves_all_tables() {
        let source = migrated();
        seed_workspace(&source, "S-1", 1_200);
        let path = temp_path("roundtrip");
        let report = export_workspace(&source, &path, "tester", 1_700_000_000_000).expect("导出");
        assert_eq!(report.manifest.counts.get("events"), Some(&1_200));
        assert_eq!(report.manifest.workspace_id.as_deref(), Some("WS-1"));
        assert!(report.content_hash.starts_with("fnv1a64:"));

        let file = read_file_json(&path);
        assert_eq!(file["format"], EXPORT_FORMAT);
        assert_eq!(file["format_version"], EXPORT_FORMAT_VERSION);
        let checksums = file["chunk_checksums"]["events"]
            .as_array()
            .expect("分块清单");
        assert_eq!(checksums.len(), 2, "1200 行应分成两块校验和");

        let mut target = migrated();
        let import =
            import_workspace(&mut target, &path, Some(&report.content_hash)).expect("导入");
        assert_eq!(import.inserted.get("events"), Some(&1_200));
        assert_eq!(import.inserted.get("sessions"), Some(&1));

        // 数据一致：行数与关键值（含时间戳微秒精度）必须逐一对应。
        for table in EXPORT_TABLES {
            let expected: i64 = report.manifest.counts.get(*table).copied().unwrap_or(0);
            let actual: i64 = target
                .query_row(
                    &format!("SELECT count(*) FROM {}", quote_identifier(table)),
                    [],
                    |row| row.get(0),
                )
                .expect("统计目标表");
            assert_eq!(actual, expected, "表 {table} 行数必须一致");
        }
        let (src_ts, tgt_ts): (i64, i64) = {
            let source_ts: i64 = source
                .query_row("SELECT epoch_us(ts) FROM events WHERE seq = 7", [], |r| {
                    r.get(0)
                })
                .expect("源时间戳");
            let target_ts: i64 = target
                .query_row("SELECT epoch_us(ts) FROM events WHERE seq = 7", [], |r| {
                    r.get(0)
                })
                .expect("目标时间戳");
            (source_ts, target_ts)
        };
        assert_eq!(src_ts, tgt_ts, "时间戳必须经 VARCHAR 往返无损");
        let payload: String = target
            .query_row(
                "SELECT CAST(CAST(payload_json AS JSON) AS VARCHAR) FROM events WHERE seq = 7",
                [],
                |r| r.get(0),
            )
            .expect("目标 payload");
        assert_eq!(payload, "{\"n\":7}");

        let audit_rows: i64 = source
            .query_row(
                "SELECT count(*) FROM audit WHERE action = 'data_export'",
                [],
                |r| r.get(0),
            )
            .expect("审计行");
        assert_eq!(audit_rows, 1, "导出必须写 data_export 审计");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn import_rejects_duplicate_ids_against_populated_target() {
        let source = migrated();
        seed_workspace(&source, "S-1", 3);
        let path = temp_path("dup2");
        export_workspace(&source, &path, "tester", 1).expect("导出");
        // 目标库带同名数据：事件 seq 与 sessions.id 全部冲突。
        let mut target = migrated();
        seed_workspace(&target, "S-1", 3);
        let err = import_workspace(&mut target, &path, None).expect_err("重复 ID 必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(
            err.to_string().contains("V-106"),
            "错误须援引幂等语义：{err}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn import_rejects_format_and_schema_mismatch() {
        let source = migrated();
        seed_workspace(&source, "S-1", 2);
        let path = temp_path("mismatch");
        export_workspace(&source, &path, "tester", 1).expect("导出");

        let mut file: serde_json::Value = read_file_json(&path);
        file["format"] = serde_json::json!("other-format");
        write_file_json(&path, &file);
        let mut target = migrated();
        let err = import_workspace(&mut target, &path, None).expect_err("格式不符必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);

        let mut file = read_file_json(&path);
        file["format"] = serde_json::json!(EXPORT_FORMAT);
        file["format_version"] = serde_json::json!(EXPORT_FORMAT_VERSION + 1);
        write_file_json(&path, &file);
        let err = import_workspace(&mut target, &path, None).expect_err("版本不符必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);

        let mut file = read_file_json(&path);
        file["format_version"] = serde_json::json!(EXPORT_FORMAT_VERSION);
        file["manifest"]["schema_version"] =
            serde_json::json!(crate::schema::STORE_SCHEMA_VERSION + 1);
        write_file_json(&path, &file);
        let err = import_workspace(&mut target, &path, None)
            .expect_err("导出 schema 版本高于目标必须拒绝");
        assert!(err.to_string().contains("不兼容"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn import_rejects_checksum_drift_and_wrong_hash() {
        let source = migrated();
        seed_workspace(&source, "S-1", 2);
        let path = temp_path("drift");
        let report = export_workspace(&source, &path, "tester", 1).expect("导出");

        let mut file = read_file_json(&path);
        file["tables"]["events"][0]["payload_json"] = serde_json::json!("tampered");
        write_file_json(&path, &file);
        let mut target = migrated();
        let err = import_workspace(&mut target, &path, None).expect_err("内容漂移必须拒绝");
        assert!(err.to_string().contains("校验和"), "{err}");

        let mut target = migrated();
        let err = import_workspace(&mut target, &path, Some("fnv1a64:deadbeefdeadbeef"))
            .expect_err("预期哈希不符必须拒绝");
        assert!(err.to_string().contains("哈希不符"));

        // 未提供预期哈希时文件可被解析——分块校验和仍会拦截内容漂移；本例已演示。
        let _ = report;
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn export_requires_migrated_source_and_lists_excluded_tables() {
        let raw = Connection::open_in_memory().expect("内存库");
        let path = temp_path("unmigrated");
        let err = export_workspace(&raw, &path, "tester", 1).expect_err("未迁移源库必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);

        let source = migrated();
        seed_workspace(&source, "S-1", 1);
        let path2 = temp_path("excluded");
        export_workspace(&source, &path2, "tester", 1).expect("导出");
        let file = read_file_json(&path2);
        let tables = file["tables"].as_object().expect("表清单");
        for excluded in EXCLUDED_TABLES {
            assert!(
                !tables.contains_key(*excluded),
                "表 {excluded} 不得出现在导出中"
            );
        }
        assert_eq!(tables.len(), EXPORT_TABLES.len());
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&path2);
    }

    #[test]
    fn import_rejects_unmigrated_target() {
        let source = migrated();
        seed_workspace(&source, "S-1", 1);
        let path = temp_path("unmigrated-target");
        export_workspace(&source, &path, "tester", 1).expect("导出");
        let mut raw = Connection::open_in_memory().expect("内存库");
        let err = import_workspace(&mut raw, &path, None).expect_err("未迁移目标必须拒绝");
        assert!(err.to_string().contains("尚未迁移"));
        let _ = std::fs::remove_file(&path);
    }

    /// 恢复的空库前置：三表任一有行即非空；未迁移库视为空（迁移错误由导入给出）。
    #[test]
    fn has_any_workspace_data_reflects_any_of_the_three_tables() {
        let conn = migrated();
        assert!(!has_any_workspace_data(&conn).expect("空库"));

        // 仅 events 有行（session_id 可空）。
        let conn = migrated();
        conn.execute(
            "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                                 kind, payload_json, ts, persistence_level)
             VALUES (1, 'E-1', NULL, NULL, NULL, NULL, 'SessionStarted',
                     CAST('{}' AS JSON)::VARIANT, current_timestamp, 'redacted')",
            [],
        )
        .expect("插入事件");
        assert!(has_any_workspace_data(&conn).expect("仅 events 非空"));

        // 仅 tasks 有行（schema 无外键约束，session_id 允许指向不存在会话）。
        let conn = migrated();
        conn.execute(
            "INSERT INTO tasks (id, session_id, title, stage, status, parent_id,
                                acceptance_json, retry_count, updated_at)
             VALUES ('T-1', 'S-x', '任务', 'plan', 'pending', NULL, '[]', 0, current_timestamp)",
            [],
        )
        .expect("插入任务");
        assert!(has_any_workspace_data(&conn).expect("仅 tasks 非空"));

        // 仅 sessions 有行。
        let conn = migrated();
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, last_active_at, status)
             VALUES ('S-1', '会话', current_timestamp, current_timestamp, 'active')",
            [],
        )
        .expect("插入会话");
        assert!(has_any_workspace_data(&conn).expect("仅 sessions 非空"));

        // 未迁移库：三表不存在，视为空。
        let raw = Connection::open_in_memory().expect("内存库");
        assert!(!has_any_workspace_data(&raw).expect("未迁移库视为空"));
    }
}
