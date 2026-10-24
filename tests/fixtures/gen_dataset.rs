//! M2 数据集生成器（T053）：10 万事件会话与 1 万任务（固定阶段/状态分布）。
//!
//! 作为模块被集成测试经 `#[path = "../fixtures/gen_dataset.rs"]` 引入（cargo 只
//! 自动发现 `tests/*.rs` 与 `tests/*/main.rs`，fixtures 目录须显式引入）。
//!
//! 确定性：事件 `seq` 连续、任务按索引取模固定分配阶段与状态，两次生成同参数
//! 数据集内容一致（V-101/V-102 的可复现夹具）。写入按批（有界内存，无 OFFSET）。

use std::collections::BTreeMap;

use duckdb::Connection;
use whcode_domain::TaskStatus;
use whcode_store::directory;

/// 单批插入行数（有界内存；参数量 200×列数 在 DuckDB 参数上限内）。
pub const ROWS_PER_BATCH: usize = 200;

/// 固定阶段分布：`(阶段, 权重百分比)`，合计 100。
pub const STAGE_WEIGHTS: [(&str, usize); 5] = [
    ("plan", 20),
    ("spec", 10),
    ("implement", 40),
    ("verify", 20),
    ("converge", 10),
];

/// 固定状态分布：`(状态, 权重百分比)`，合计 100；保证 done/blocked/active 非零。
pub const STATUS_WEIGHTS: [(TaskStatus, usize); 5] = [
    (TaskStatus::Completed, 30),
    (TaskStatus::Running, 10),
    (TaskStatus::Blocked, 10),
    (TaskStatus::Ready, 20),
    (TaskStatus::Pending, 30),
];

/// 生成结果（含规模断言所需的分布）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatasetReport {
    /// 会话 ID。
    pub session_id: String,
    /// 事件行数。
    pub events: usize,
    /// 任务行数。
    pub tasks: usize,
    /// 实际阶段分布。
    pub by_stage: BTreeMap<String, usize>,
    /// 实际状态分布（稳定名 → 行数）。
    pub by_status: BTreeMap<String, usize>,
}

/// 按权重返回第 `index` 个元素命中的桶下标（累积权重扫描）。
fn bucket_index(index: usize, weights: &[usize]) -> usize {
    let mut slot = index % 100;
    for (position, weight) in weights.iter().enumerate() {
        if slot < *weight {
            return position;
        }
        slot -= *weight;
    }
    weights.len() - 1
}

/// 阶段权重序列（与 [`STAGE_WEIGHTS`] 同序）。
const STAGE_WEIGHT_VALUES: [usize; 5] = [20, 10, 40, 20, 10];

/// 状态权重序列（与 [`STATUS_WEIGHTS`] 同序）。
const STATUS_WEIGHT_VALUES: [usize; 5] = [30, 10, 10, 20, 30];

/// 在既已迁移的库上生成数据集；会话由本函数创建并返回报告。
///
/// # Panics
///
/// 任何 SQL 失败时 panic（夹具失败即测试失败）。
pub fn generate(
    conn: &mut Connection,
    session_id: &str,
    events: usize,
    tasks: usize,
) -> DatasetReport {
    assert!(events > 0 && tasks > 0, "数据集规模必须为正");
    directory::create_session_with_id(conn, session_id, "性能夹具", "active", 1_000)
        .expect("创建夹具会话");

    // 事件：分批多行 VALUES（seq 连续、payload 带 64 字节填充模拟真实载荷）。
    // events.seq 是工作区级主键：同库多次生成时从当前最大值续起，不与既有数据冲突。
    let seq_base: usize = conn
        .query_row("SELECT COALESCE(MAX(seq), 0) FROM events", [], |row| {
            row.get(0)
        })
        .map(|max: i64| max as usize)
        .expect("读取事件游标");
    let mut start = seq_base + 1;
    let end = seq_base + events;
    while start <= end {
        let count = ROWS_PER_BATCH.min(end - start + 1);
        let mut values = Vec::with_capacity(count);
        let mut binds: Vec<duckdb::types::Value> = Vec::with_capacity(count * 4);
        for seq in start..start + count {
            values.push(
                "(?, ?, ?, NULL, NULL, NULL, 'StreamingChunk', CAST(? AS JSON)::VARIANT, current_timestamp, 'redacted')"
                    .to_string(),
            );
            binds.push(duckdb::types::Value::BigInt(seq as i64));
            binds.push(duckdb::types::Value::Text(format!("E-{seq}")));
            binds.push(duckdb::types::Value::Text(session_id.to_string()));
            binds.push(duckdb::types::Value::Text(format!(
                "{{\"pad\":\"{}\",\"seq\":{seq}}}",
                "x".repeat(64)
            )));
        }
        let sql = format!(
            "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                                 kind, payload_json, ts, persistence_level)
             VALUES {}",
            values.join(", ")
        );
        conn.execute(sql.as_str(), duckdb::params_from_iter(binds.iter()))
            .expect("批量插入事件");
        start += count;
    }

    // 任务：分批插入，阶段/状态按索引固定分配。
    let mut by_stage: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_status: BTreeMap<String, usize> = BTreeMap::new();
    for task_index in 1..=tasks {
        let stage = STAGE_WEIGHTS[bucket_index(task_index, &STAGE_WEIGHT_VALUES)].0;
        let status = STATUS_WEIGHTS[bucket_index(task_index, &STATUS_WEIGHT_VALUES)].0;
        *by_stage.entry((*stage).to_string()).or_insert(0) += 1;
        *by_status.entry(status.as_str().to_string()).or_insert(0) += 1;
    }
    let mut task_start = 1usize;
    while task_start <= tasks {
        let count = ROWS_PER_BATCH.min(tasks - task_start + 1);
        let mut values = Vec::with_capacity(count);
        let mut binds: Vec<duckdb::types::Value> = Vec::with_capacity(count * 4);
        for offset in 0..count {
            let task_index = task_start + offset;
            let stage = STAGE_WEIGHTS[bucket_index(task_index, &STAGE_WEIGHT_VALUES)].0;
            values.push(
                "(?, ?, ?, ?, 'pending', NULL, '[{\"check\":\"可执行断言占位\"}]', 0, \
                 current_timestamp)"
                    .to_string(),
            );
            // ID 带会话前缀：同库多会话生成时键不冲突，且保持确定性。
            binds.push(duckdb::types::Value::Text(format!(
                "{session_id}-T-{task_index}"
            )));
            binds.push(duckdb::types::Value::Text(session_id.to_string()));
            binds.push(duckdb::types::Value::Text(format!("任务 {task_index}")));
            binds.push(duckdb::types::Value::Text((*stage).to_string()));
        }
        let sql = format!(
            "INSERT INTO tasks (id, session_id, title, stage, status, parent_id,
                                acceptance_json, retry_count, updated_at)
             VALUES {}",
            values.join(", ")
        );
        conn.execute(sql.as_str(), duckdb::params_from_iter(binds.iter()))
            .expect("批量插入任务");
        task_start += count;
    }

    // 状态按固定分布落库（pending 插入已完成，其余逐行更新；有界更新语句数 = 任务数）。
    for task_index in 1..=tasks {
        let status = STATUS_WEIGHTS[bucket_index(task_index, &STATUS_WEIGHT_VALUES)].0;
        if status != TaskStatus::Pending {
            conn.execute(
                "UPDATE tasks SET status = ? WHERE id = ?",
                duckdb::params![status.as_str(), format!("{session_id}-T-{task_index}")],
            )
            .expect("更新任务状态");
        }
    }

    // 预聚合概览一次（切换视图的最终一致来源）。
    whcode_store::rebuild_overview(conn, session_id).expect("重建概览");

    DatasetReport {
        session_id: session_id.to_string(),
        events,
        tasks,
        by_stage,
        by_status,
    }
}
