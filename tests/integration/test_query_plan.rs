//! 查询计划基准实验（T036、§7.2、TBD-03）。
//!
//! 目标：在真实规模夹具上记录关键查询的 `EXPLAIN` / `EXPLAIN ANALYZE` 原文与耗时，
//! 并核对必需索引齐全。**不规定顺序扫描一律失败**：DuckDB 的 ART 索引不提供有序迭代，
//! 首屏或大范围查询选择 `SEQ_SCAN + TOP_N` 可能是正确计划，报告逐条给出理由。
//!
//! 默认 `#[ignore]`：
//!
//! ```text
//! cargo test -p whcode-tests --test query_plan -- --ignored --nocapture
//! ```
//!
//! 结果写入 `artifacts/query-plan-report.json`（UTF-8 无 BOM、LF）。

use std::fs;
use std::path::Path;
use std::time::Instant;

use duckdb::Connection;
use serde_json::json;
use whcode_store::{REQUIRED_INDEXES, apply_migrations, installed_indexes, verify_indexes};

/// 夹具规模：单会话，足以让索引与顺序扫描的代价差异显现。
const EVENTS: usize = 50_000;
const MESSAGES: usize = 20_000;
const TASKS: usize = 5_000;
/// 每条查询的计时重复次数（取中位数）。
const REPEATS: usize = 5;

/// 单条基准查询。
struct Case {
    name: &'static str,
    sql: &'static str,
    rationale: &'static str,
}

const CASES: &[Case] = &[
    Case {
        name: "events_forward_selective",
        sql: "SELECT seq FROM events WHERE session_id = 'S-1' AND seq > 40000 \
              ORDER BY seq LIMIT 100",
        rationale: "会话 + 排他游标，命中后 20% 区间；本机实测为顺序扫描，\
                    复合 (session_id, seq) 索引在 DuckDB 1.5.6 与 2.0.0 上均未被用于扫描。\
                    50k 规模可接受，更大规模由 T054 复测。",
    },
    Case {
        name: "events_first_page",
        sql: "SELECT seq FROM events WHERE session_id = 'S-1' ORDER BY seq LIMIT 100",
        rationale: "首屏无下界：需要全序最前 100 条。ART 索引不提供有序迭代，\
                    SEQ_SCAN + TOP_N 是合理计划。",
    },
    Case {
        name: "events_by_request",
        sql: "SELECT seq FROM events WHERE request_id = 'R-777' ORDER BY seq",
        rationale: "高选择性等值查询（约 50 行/5 万），应走 v2 的 request_id 索引。",
    },
    Case {
        name: "events_by_tool_call",
        sql: "SELECT seq FROM events WHERE tool_call_id = 'C-777' ORDER BY seq",
        rationale: "高选择性等值查询，应走 v2 的 tool_call_id 索引。",
    },
    Case {
        name: "messages_forward_cursor",
        sql: "SELECT id FROM messages \
              WHERE session_id = 'S-1' \
                AND (ts > make_timestamp(2500000) \
                     OR (ts = make_timestamp(2500000) AND id > 'M-10000')) \
              ORDER BY ts, id LIMIT 100",
        rationale: "T035 的消息游标：会话 + 时间范围 + id 破平局。复合三列索引存在，\
                    但本机实测为顺序扫描；DuckDB 1.5.6 与 2.0.0 均未使用复合索引扫描，\
                    该限制与收益由 T054 在 10 万事件规模复测。",
    },
    Case {
        name: "messages_exact_timestamp",
        sql: "SELECT id FROM messages \
              WHERE session_id = 'S-1' AND ts = make_timestamp(2500000)",
        rationale: "等值时间戳（4 行/2 万）本应高选择性，但复合索引未被使用，实测为顺序扫描。\
                    是否追加单列时间索引需由更大规模基准决定，不在本任务凭直觉添加。",
    },
    Case {
        name: "tasks_by_session_stage",
        sql: "SELECT id FROM tasks WHERE session_id = 'S-1' AND stage = 'Implement'",
        rationale: "会话 + 阶段过滤命中约 50%，顺序扫描比索引查找更便宜，属合理计划；\
                    索引仍用于等值/索引连接等场景，存在性由 REQUIRED_INDEXES 保证。",
    },
    Case {
        name: "tasks_by_session_status",
        sql: "SELECT id FROM tasks WHERE session_id = 'S-1' AND status = 'blocked'",
        rationale: "会话 + 状态过滤命中约 33%，顺序扫描比索引查找更便宜，属合理计划。",
    },
];

/// 执行 `EXPLAIN`（或 `EXPLAIN ANALYZE`），返回 `(key, value)` 行。
fn explain(
    conn: &Connection,
    sql: &str,
    analyze: bool,
) -> Result<Vec<(String, String)>, duckdb::Error> {
    let prefix = if analyze {
        "EXPLAIN ANALYZE "
    } else {
        "EXPLAIN "
    };
    let mut stmt = conn.prepare(&format!("{prefix}{sql}"))?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    rows.collect()
}

/// 执行查询并统计返回行数（不收集内容，避免把结果集开销混入计划证据）。
fn count_rows(conn: &Connection, sql: &str) -> Result<usize, duckdb::Error> {
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query([])?;
    let mut count = 0usize;
    while rows.next()?.is_some() {
        count += 1;
    }
    Ok(count)
}

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("tests 包必须位于 workspace 根下")
}

/// VARIANT 专用用例（Task 8）：`events.payload_json` 在 schema v6 已是 `VARIANT`。
///
/// 这组用例**不并入**上面八个用例，以保持与 1.5.6 报告的数量口径可直接对比
///（2.0.0 报告同样保留 8 例主口径）。
/// 目标只有一个：确认 VARIANT 判定与 JSON 文本转换没有把有界游标查询退化为全表扫描。
///
/// 函数名在 2.0.0 上实测：`variant_type(...)` 返回 `OBJECT`，`variant_extract(v, 'text')`
/// 以裸字段名取值（2.0 实测 `'$.text'` 路径写法返回 NULL），`variant_to_json` 不存在；
/// 项目读路径统一使用 `CAST(CAST(payload_json AS JSON) AS VARCHAR)`（见 `store::query`）。
const VARIANT_CASES: &[Case] = &[
    Case {
        name: "events_variant_type_object",
        sql: "SELECT seq FROM events WHERE variant_type(payload_json) = 'OBJECT' LIMIT 100",
        rationale: "对 payload_json 做 VARIANT 类型判定：payload 上没有索引，全表扫描是唯一计划；\
                    本用例只记录扫描形状与耗时，用来确认判定函数本身不引入额外算子。",
    },
    Case {
        name: "events_variant_json_roundtrip",
        sql: "SELECT CAST(CAST(payload_json AS JSON) AS VARCHAR) FROM events \
              WHERE session_id = 'S-1' AND seq > 40000 ORDER BY seq LIMIT 100",
        rationale: "与 events_forward_selective 同形状，只增加读路径的 VARIANT→JSON 文本投影；\
                    计划形状应与基线一致，若退化为额外全表扫描则说明转换函数有代价。",
    },
];

/// 单条用例的测量结果。
struct MeasuredCase {
    kind: &'static str,
    uses_index: bool,
    entry: serde_json::Value,
}

/// 逐案测量：两份计划原文、实际返回行数、5 次计时中位数，并打印一行摘要。
fn measure_case(conn: &Connection, case: &Case) -> MeasuredCase {
    let plan = explain(conn, case.sql, false).expect("EXPLAIN");
    let analyze = explain(conn, case.sql, true).expect("EXPLAIN ANALYZE");
    let plan_text = plan
        .iter()
        .map(|(key, value)| format!("{key}: {value}"))
        .collect::<Vec<_>>()
        .join("\n");
    let analyze_text = analyze
        .iter()
        .map(|(key, value)| format!("{key}: {value}"))
        .collect::<Vec<_>>()
        .join("\n");
    // DuckDB 的 `EXPLAIN` 与实际执行算子命名不同：优化器计划写 `SEQ_SCAN`，
    // 而 `EXPLAIN ANALYZE` 在真正走索引时显示 `TABLE_SCAN ... Type: Index Scan`。
    // 判定以实际执行计划为准，并保留两份原文供复核。
    let uses_index =
        analyze_text.contains("Type: Index Scan") || analyze_text.contains("INDEX_SCAN");
    let kind = if uses_index {
        "index_scan"
    } else if analyze_text.contains("Type: Sequential Scan") || analyze_text.contains("SEQ_SCAN") {
        "seq_scan"
    } else {
        "other"
    };
    let used_indexes: Vec<&str> = REQUIRED_INDEXES
        .iter()
        .filter(|index| plan_text.contains(index.name))
        .map(|index| index.name)
        .collect();

    let rows = count_rows(conn, case.sql).expect("执行查询");
    assert!(rows > 0, "查询 {} 不应返回空结果", case.name);
    assert!(!plan.is_empty(), "查询 {} 必须产出计划", case.name);

    let mut samples = Vec::with_capacity(REPEATS);
    for _ in 0..REPEATS {
        let started = Instant::now();
        let executed = count_rows(conn, case.sql).expect("计时执行");
        assert_eq!(executed, rows, "重复执行的返回行数必须一致");
        samples.push(started.elapsed().as_micros() as u64);
    }
    samples.sort_unstable();
    let median_us = samples[samples.len() / 2];
    println!(
        "{}：{kind}，{rows} 行，中位 {median_us} µs{}",
        case.name,
        if used_indexes.is_empty() {
            String::new()
        } else {
            format!("，使用 {}", used_indexes.join(", "))
        }
    );

    MeasuredCase {
        kind,
        uses_index,
        entry: json!({
            "name": case.name,
            "sql": case.sql,
            "plan_kind": kind,
            "uses_index": uses_index,
            "used_indexes": used_indexes,
            "rows": rows,
            "latency_us": {
                "samples": samples.len(),
                "median": median_us,
                "max": samples.last().copied().unwrap_or(0),
            },
            "explain": plan.iter().map(|(key, value)| json!({"key": key, "value": value})).collect::<Vec<_>>(),
            "explain_analyze": analyze.iter().map(|(key, value)| json!({"key": key, "value": value})).collect::<Vec<_>>(),
            "rationale": case.rationale,
        }),
    }
}

/// 查询计划实验：5 万事件 / 2 万消息 / 5 千任务。
#[test]
#[ignore = "实验记录器：需用 --ignored 显式运行，结果写入 artifacts/query-plan-report.json"]
fn query_plan_experiment() {
    let workspace = workspace_root().join("target").join(format!(
        "query-plan-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|delta| delta.as_millis())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&workspace).expect("创建临时工作目录");
    let db = workspace.join("query-plan.duckdb");

    let conn = Connection::open(&db).expect("打开基准库");
    apply_migrations(&conn).expect("迁移");
    verify_indexes(&conn).expect("必需索引必须齐全");

    conn.execute_batch(&format!(
        "INSERT INTO events
         (seq, id, session_id, task_id, request_id, tool_call_id, kind, payload_json, ts,
          persistence_level)
         SELECT i, 'E-'||i, 'S-1', 'T-'||(i%100), 'R-'||(i%1000), 'C-'||(i%1000),
                'StreamingChunk', CAST('{{}}' AS JSON)::VARIANT, make_timestamp(i*1000), 'redacted'
         FROM range(1, {EVENTS}+1) t(i);

         INSERT INTO messages (id, session_id, role, content_ref, token_count, ts, parent_id)
         SELECT 'M-'||i, 'S-1', 'user', 'ref-'||i, NULL, make_timestamp((i//4)*1000), NULL
         FROM range(1, {MESSAGES}+1) t(i);

         INSERT INTO tasks
         (id, session_id, title, stage, status, parent_id, acceptance_json, retry_count, updated_at)
         SELECT 'T-'||i, 'S-1', 'task-'||i,
                CASE WHEN i%2=0 THEN 'Implement' ELSE 'Plan' END,
                CASE WHEN i%3=0 THEN 'blocked' ELSE 'pending' END,
                NULL, CAST('[]' AS JSON)::VARIANT, 0, now()
         FROM range(1, {TASKS}+1) t(i);"
    ))
    .expect("生成夹具");

    let installed = installed_indexes(&conn).expect("索引目录");
    println!(
        "== 查询计划基准（T036）==\n夹具：{EVENTS} 事件 / {MESSAGES} 消息 / {TASKS} 任务；\
         已安装 {} 个必需索引",
        installed.len()
    );

    let mut cases_json = Vec::with_capacity(CASES.len());
    let mut index_scan_cases = 0usize;
    let mut seq_scan_cases = 0usize;
    for case in CASES {
        let measured = measure_case(&conn, case);
        if measured.uses_index {
            index_scan_cases += 1;
        } else if measured.kind == "seq_scan" {
            seq_scan_cases += 1;
        }
        cases_json.push(measured.entry);
    }

    println!("-- VARIANT 专用用例（payload_json 已于 schema v6 变为 VARIANT）--");
    let mut variant_cases_json = Vec::with_capacity(VARIANT_CASES.len());
    for case in VARIANT_CASES {
        variant_cases_json.push(measure_case(&conn, case).entry);
    }

    let total = CASES.len();
    let conclusion = format!(
        "{index_scan_cases}/{total} 个高选择性单列索引查询（request_id、tool_call_id）走索引扫描；\
         {seq_scan_cases}/{total} 个走顺序扫描：首屏/大范围游标因 ART 不提供有序迭代而合理，\
         tasks 过滤命中 33%—50% 时顺序扫描更便宜，消息复合索引在本机 DuckDB 2.0.0 未触发索引扫描。\
         VARIANT 用例单独记录，不参与本句的 8 例统计；\
         复合索引与更大规模（10 万事件）的收益由 T054/T055 复测；本任务不以直觉增删索引（§7.2、TBD-03）。"
    );
    println!("== 结论：{conclusion} ==");

    let report = json!({
        "schema_version": 1,
        "experiment": "query-plan",
        "task": "T036",
        "environment": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "logical_cores": std::thread::available_parallelism()
                .map(std::num::NonZeroUsize::get)
                .unwrap_or(1),
            "duckdb": whcode_store::engine_version().unwrap_or_else(|err| format!("<不可用：{err}>")),
            "machine_note": "Windows 11 10.0.26300 / i7-1260P / 16.9 GB（本机环境事实）",
        },
        "fixture": {
            "events": EVENTS,
            "messages": MESSAGES,
            "tasks": TASKS,
            "session": "S-1",
            "schema_version": whcode_store::STORE_SCHEMA_VERSION,
            "installed_indexes": installed.iter().cloned().collect::<Vec<_>>(),
        },
        "summary": {
            "cases": total,
            "index_scan_cases": index_scan_cases,
            "seq_scan_cases": seq_scan_cases,
        },
        "cases": cases_json,
        "variant_measurement": {
            "note": "events.payload_json 于 schema v6 变为 VARIANT（ADR-034）；这组用例单独记录，\
                     不并入上面八个用例的统计。函数名以 2.0.0 实测为准：variant_type(...) 返回 OBJECT，\
                     variant_extract(v, 'text') 使用裸字段名，variant_to_json 不存在；\
                     读路径使用 CAST(CAST(payload_json AS JSON) AS VARCHAR)。",
            "cases": variant_cases_json,
        },
        "conclusion": conclusion,
        "notes": [
            "EXPLAIN 记录优化器计划；EXPLAIN ANALYZE 记录实际执行与耗时，二者原文保存",
            "DuckDB 的 EXPLAIN 写 SEQ_SCAN，而实际走索引时 EXPLAIN ANALYZE 显示 Type: Index Scan；\
             index_scan 判定以实际执行为准",
            "索引存在不等于必然使用；选择性、范围大小、排序与限额共同决定计划（TBD-03）",
            "本实验是记录器，不冻结性能预算；单机单次样本不作为平台承诺（V-006）",
            "夹具为单会话 5 万事件/2 万消息/5 千任务；其他数据分布可能产生不同计划",
            "payload_json 上没有任何索引；对它做类型判定或提取必然全表扫描，属数据形状决定，\
             不是索引缺口（不得因此增删索引）",
        ],
    });

    let out = workspace_root()
        .join("artifacts")
        .join("query-plan-report.json");
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent).expect("创建 artifacts 目录");
    }
    fs::write(
        &out,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&report).expect("报告必须可序列化")
        ),
    )
    .expect("写入计划证据");
    println!("证据已写入 {}", out.display());

    drop(conn);
    let _ = fs::remove_dir_all(&workspace);
}
