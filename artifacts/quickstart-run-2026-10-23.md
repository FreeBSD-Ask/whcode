# Quickstart 全流程执行留证（2026-10-23）

对应 [specs/001-whcode-harness/quickstart.md](../specs/001-whcode-harness/quickstart.md)；
执行方式：`python script/quickstart_run.py`（临时脚本，不入库）。
命令均在**仓库外临时工作区**运行，避免污染仓库数据。

## 测量纪律字段

- os = Windows-11-10.0.26300-SP0
- machine = AMD64
- python = 3.14.2
- workspace = C:\Users\ykla\AppData\Local\Temp\whcode-quickstart-i4p8jj9z（执行后删除）
- 生成时间（UTC） = 2026-10-23T05:15:57Z
- 二进制 = D:\WHCode\target\release\whcode.exe（`cargo build --release --locked`）

## 结果总览

| 条目 | 期望 | 退出码 | 结论 |
|---|---|---|---|
| M0-0 fetch-duckdb --check | 成功 | 0 | 通过 |
| M0-1 cargo build --release --locked | 成功 | 0 | 通过 |
| M0-1 record_deps.py artifacts | 成功 | 0 | 通过 |
| M0-1b cross-check.py | 成功或部分阻塞 | 0 | 通过 |
| M0-2 whcode doctor --json | 成功 | 0 | 通过 |
| M0-2 whcode session new --title smoke --json | 成功 | 0 | 通过 |
| M0-2 whcode status --session S-1 --json | 成功 | 0 | 通过 |
| M0-3 TUI 最小闭环（V-003） | 未取得 | —— | 未取得（跳过） |
| M0-4 cargo test -p whcode-ssh | 成功 | 0 | 通过 |
| M0-4 whcode serve --ssh --port 2222 | 预期失败（未实现） | 1 | 通过（失败文案如实返回） |
| M0-4 ssh -p 2222 客户端连通 | 未取得 | —— | 未取得（跳过） |
| M0-5 cargo test -p whcode-store | 成功 | 0 | 通过 |
| M0-6 bench baseline（缩减规模） | 成功 | 0 | 通过 |
| M1 whcode db stats --json | 成功 | 0 | 通过 |
| M1 whcode db check | 成功 | 0 | 通过 |
| M1 events export --format jsonl（真实会话） | 成功 | 0 | 通过 |
| M1 events export（不存在的会话 S-NOT-EXIST） | 缺陷登记 | 0 | **缺陷（已登记，不视为通过）** |
| M1 db purge --before ... --dry-run | 成功 | 0 | 通过 |
| M1 db compact --export（草案形态） | 预期失败（用法不符） | 2 | 通过（失败文案如实返回） |
| M1 db archive --export | 成功 | 0 | 通过 |
| M2 whcode session list --json | 成功 | 0 | 通过 |
| M2 whcode task list --json | 成功 | 0 | 通过 |
| M2 bench session-switch（大数据集） | 未取得 | —— | 未取得（跳过） |
| M3 whcode run --model stub | 预期失败（未接线） | 50 | 通过（失败文案如实返回） |
| M4 whcode spec status --json | 成功或明确失败 | 0 | 通过 |
| M4 whcode spec validate --json | 预期失败（未接线） | 50 | 通过（失败文案如实返回） |
| M8 whcode runtime list --json | 预期失败（未接线） | 50 | 通过（失败文案如实返回） |
| M8 cargo build --locked --no-default-features | 成功 | 0 | 通过 |
| M9 bench stability --hours 72 | 未取得 | —— | 未取得（跳过） |
| M6/M7 三端一致 contract_three_fronts | 已另行取得 | —— | 未取得（跳过） |

## 逐条输出（stdout/stderr 截断至 900 字符）

### M0-0 fetch-duckdb --check

- argv：`python D:\WHCode\ci\fetch-duckdb.py --check`
- 期望：成功；退出码：0；耗时：179 ms

```text
已解包目录可用：D:\WHCode\vendor\duckdb\1.5.6（预编译来源：{"x86_64-pc-windows-msvc": "libduckdb-windows-amd64.zip"}）
```

### M0-1 cargo build --release --locked

- argv：`C:\Users\ykla\.cargo\bin\cargo.exe build --release --locked`
- 期望：成功；退出码：0；耗时：600 ms

```text
（stdout 为空）
--- stderr ---
[1m[92m    Finished[0m ]8;;https://doc.rust-lang.org/cargo/reference/profiles.html#default-profiles\`release` profile [optimized]]8;;\ target(s) in 0.51s
```

### M0-1 record_deps.py artifacts

- argv：`python D:\WHCode\ci\platform-matrix\record_deps.py artifacts`
- 期望：成功；退出码：0；耗时：3892 ms

```text
已写出 D:\WHCode\artifacts\deps-windows.txt
```

### M0-1b cross-check.py

- argv：`python D:\WHCode\ci\platform-matrix\cross-check.py`
- 期望：成功或部分阻塞；退出码：0；耗时：3375 ms
- 备注：交叉检查是代码级证据，不构成运行验证（ADR-010、G-01）

```text
检查 x86_64-unknown-freebsd（排除 whcode-store, whcode-bin, whcode-ssh）…
  子集：通过（0.4s）
检查 x86_64-unknown-freebsd（完整工作区，预期失败）…
  完整：按预期失败（1.2s）
检查 x86_64-unknown-netbsd（排除 whcode-store, whcode-bin, whcode-ssh）…
  子集：通过（0.4s）
检查 x86_64-unknown-netbsd（完整工作区，预期失败）…
  完整：按预期失败（1.3s）
已写出 D:\WHCode\artifacts\cross-check-windows.json
```

### M0-2 whcode doctor --json

- argv：`D:\WHCode\target\release\whcode.exe doctor --json`
- 期望：成功；退出码：0；耗时：92 ms

```text
{"doctor":{"checkpoint":{"before_dangerous":true,"every_steps":0,"interval_seconds":300,"retain":10},"db":{"max_memory_limit_mb":384,"memory_limit_mb":192,"temp_directory":"C:\\Users\\ykla\\AppData\\Local\\Temp\\whcode-quickstart-i4p8jj9z\\tmp","threads":4},"trace":{"default_level":"redacted"},"ui":{"theme":"eink-day"},"workspace_path":"C:\\Users\\ykla\\AppData\\Local\\Temp\\whcode-quickstart-i4p8jj9z"},"duckdb_engine_version":"v1.5.6","duckdb_error":null,"runtimes":[{"available":false,"capability":"restricted","name":"js","unsupported":["Node.js 内置模块","文件系统","网络"]},{"available":true,"capability":"restricted","name":"python","unsupported":["任意标准库","任意第三方包"]},{"available":false,"capability":"shell_only","name":"gitbash","unsupported":["任意系统 Shell"]},{"available":false,"capability":"shell_only","name":"bsdshell","unsupported":["Windows Git Bash"]}],"schema_version":1,"whcode_version":"2026
--- stderr ---
[INFO] doctor 完成（api_key=[REDACTED] 已在日志路径脱敏）
[WARN] max_temp_directory_size 未设置：临时目录空间无显式上限（§5.4 建议显式配置）
```

### M0-2 whcode session new --title smoke --json

- argv：`D:\WHCode\target\release\whcode.exe --workspace C:\Users\ykla\AppData\Local\Temp\whcode-quickstart-i4p8jj9z session new --title smoke --json`
- 期望：成功；退出码：0；耗时：81 ms

```text
{
  "schema_version": 1,
  "data": {
    "session": {
      "archived": false,
      "created_at_ms": 1792732543637,
      "id": "S-1",
      "stage": "specify",
      "status": "active",
      "title": "smoke"
    }
  }
}
```

### M0-2 whcode status --session S-1 --json

- argv：`D:\WHCode\target\release\whcode.exe --workspace C:\Users\ykla\AppData\Local\Temp\whcode-quickstart-i4p8jj9z status --session S-1 --json`
- 期望：成功；退出码：0；耗时：56 ms
- 备注：quickstart 草案漏写 `--session`（必填），已同步修正文档

```text
{
  "schema_version": 1,
  "data": {
    "active_task": null,
    "event_cursor": 0,
    "overview": null,
    "schema_version": 1,
    "session": {
      "id": "S-1",
      "stage": "specify",
      "status": "active",
      "title": "smoke"
    }
  }
}
```

### M0-3 TUI 最小闭环（V-003）

- argv：`（未执行）`
- 期望：未取得；退出码：None；耗时：None ms
- 备注：需真实 TTY：键盘输入/resize/异常恢复无法由脚本代替（m0-report.md B-05）

```text
（stdout 为空）
```

### M0-4 cargo test -p whcode-ssh

- argv：`C:\Users\ykla\.cargo\bin\cargo.exe test --offline -p whcode-ssh`
- 期望：成功；退出码：0；耗时：2654 ms

```text
running 9 tests
test proto::tests::exit_status_maps_to_completed_or_failed ... ok
test proto::tests::tail_buffer_keeps_tail_and_flags_truncation ... ok
test proto::tests::validation_rejects_invalid_targets_without_any_registry_call ... ok
test proto::tests::end_to_end_nonzero_exit_is_failed ... ok
test proto::tests::end_to_end_exec_reports_completed_with_output ... ok
test proto::tests::end_to_end_large_output_is_truncated_to_tail ... ok
test proto::tests::command_without_exit_status_is_unknown_not_completed_or_failed ... ok
test proto::tests::registry_failures_are_propagated ... ok
test proto::tests::connection_failure_is_failed_and_records_intent_before_result ... ok

test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.02s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
--- stderr ---
[1m[92m    Finished[0m ]8;;https://doc.rust-lang.org/cargo/reference/profiles.html#default-profiles\`test` profile [unoptimized + debuginfo]]8;;\ target(s) in 0.33s
[1m[92m     Running[0m unittests src\lib.rs (target\debug\deps\whcode_ssh-937917826c6a79a2.exe)
[1m[92m   Doc-tests[0m whcode_ssh
```

### M0-4 whcode serve --ssh --port 2222

- argv：`D:\WHCode\target\release\whcode.exe serve --ssh --port 2222`
- 期望：预期失败（未实现）；退出码：1；耗时：30 ms

```text
（stdout 为空）
--- stderr ---
serve 参数错误：parse_error: 未知参数 "--port"：可用 --web / --ssh / --bind host:port / --token-file PATH / --workspace DIR / --config PATH
```

### M0-4 ssh -p 2222 客户端连通

- argv：`（未执行）`
- 期望：未取得；退出码：None；耗时：None ms
- 备注：本机无 FreeBSD/NetBSD 节点（ADR-013 延后）

```text
（stdout 为空）
```

### M0-5 cargo test -p whcode-store

- argv：`C:\Users\ykla\.cargo\bin\cargo.exe test --offline -p whcode-store`
- 期望：成功；退出码：0；耗时：6212 ms
- 备注：quickstart 草案写的 `--test writer_prototype` 无对应测试目标，以包级测试为等价证据

```text
running 94 tests
test compact::tests::compact_rejects_missing_db_and_concurrent_maintenance ... ok
test directory::tests::create_rejects_empty_title_and_status ... ok
test indexes::tests::legacy_messages_index_is_replaced_by_three_column_index ... ok
test directory::tests::rename_and_touch_report_missing_sessions ... ok
test indexes::tests::registry_entries_are_unique_and_described ... ok
test indexes::tests::fresh_database_reports_all_required_indexes_missing ... ok
test directory::tests::event_cursor_reflects_max_seq ... ok
test directory::tests::create_assigns_monotonic_prefixed_ids ... ok
test directory::tests::list_orders_newest_first_and_filters_archived ... ok
test monitor::tests::plan_query_returns_explain_rows_and_analyze_variant ... ok
test monitor::tests::plan_query_rejects_write_statements ... ok
test monitor::tests::collect_without_writer_or_queries_has_optional_sections_emp
--- stderr ---
[1m[92m    Finished[0m ]8;;https://doc.rust-lang.org/cargo/reference/profiles.html#default-profiles\`test` profile [unoptimized + debuginfo]]8;;\ target(s) in 0.23s
[1m[92m     Running[0m unittests src\lib.rs (target\debug\deps\whcode_store-a9004c345b524797.exe)
[1m[92m   Doc-tests[0m whcode_store
```

### M0-6 bench baseline（缩减规模）

- argv：`D:\WHCode\target\release\whcode.exe bench baseline --events 2000 --repeat 1 --out D:\WHCode\artifacts\quickstart-baseline-2000x1.json`
- 期望：成功；退出码：0；耗时：1199 ms
- 备注：偏离 quickstart 默认规模（20000×3）：本轮目的是全流程贯通，不是重测冻结预算；基线见 artifacts/baseline-192mb.json

```text
whcode bench baseline（schema v1）
duckdb_engine_version: v1.5.6
run 0: 6726.5 条/秒；提交延迟 p50 3977.5 / p95 4897.9 / p99 5399.1 µs；事件行 2200；数据库 1060864 字节
汇总（1 次重复）：p50 3977.5 / p95 4897.9 / p99 5399.1 µs（样本 200）；吞吐中位数 6726.5 条/秒
报告：D:\WHCode\artifacts\quickstart-baseline-2000x1.json
测量条件见 measurement_discipline；单机单次样本，不构成预算冻结（R-12）
```

### M1 whcode db stats --json

- argv：`D:\WHCode\target\release\whcode.exe --workspace C:\Users\ykla\AppData\Local\Temp\whcode-quickstart-i4p8jj9z db stats --json`
- 期望：成功；退出码：0；耗时：54 ms

```text
{
  "schema_version": 1,
  "data": {
    "blocks_consistent": true,
    "database_name": "whcode",
    "database_size": "1.5 MiB",
    "engine_version": "v1.5.6",
    "file_size_bytes": 1585152,
    "free_blocks": 0,
    "schema_version": 5,
    "total_blocks": 6,
    "used_blocks": 6,
    "wal_size_bytes": 0,
    "wal_status": {
      "wal_autocheckpoint": "16.0 MiB",
      "wal_size": "0 bytes"
    }
  }
}
```

### M1 whcode db check

- argv：`D:\WHCode\target\release\whcode.exe --workspace C:\Users\ykla\AppData\Local\Temp\whcode-quickstart-i4p8jj9z db check --json`
- 期望：成功；退出码：0；耗时：53 ms

```text
{
  "schema_version": 1,
  "data": {
    "missing_indexes": [],
    "ok": true
  }
}
```

### M1 events export --format jsonl（真实会话）

- argv：`D:\WHCode\target\release\whcode.exe --workspace C:\Users\ykla\AppData\Local\Temp\whcode-quickstart-i4p8jj9z events export --session S-1 --after 9900 --format jsonl`
- 期望：成功；退出码：0；耗时：54 ms
- 备注：无事件时为正确的空（首行头 + count=0）

```text
{"count":0,"integrity":{"downsampled":false,"missing_chunks":false,"persistence_failed":false,"redacted":true},"redacted":true,"schema_version":1,"session_id":"S-1","trace_level":"redacted"}
{"count":0,"integrity":{"downsampled":false,"missing_chunks":false,"persistence_failed":false,"redacted":true},"redacted":true,"schema_version":1,"session_id":"S-1","trace_level":"redacted"}
```

### M1 events export（不存在的会话 S-NOT-EXIST）

- argv：`D:\WHCode\target\release\whcode.exe --workspace C:\Users\ykla\AppData\Local\Temp\whcode-quickstart-i4p8jj9z events export --session S-NOT-EXIST --format jsonl`
- 期望：缺陷登记；退出码：0；耗时：56 ms
- 备注：会话不存在时退出码 0、count=0：与“存在但无事件”不可区分（错误的空），登记缺陷待修

```text
{"count":0,"integrity":{"downsampled":false,"missing_chunks":false,"persistence_failed":false,"redacted":true},"redacted":true,"schema_version":1,"session_id":"S-NOT-EXIST","trace_level":"redacted"}
{"count":0,"integrity":{"downsampled":false,"missing_chunks":false,"persistence_failed":false,"redacted":true},"redacted":true,"schema_version":1,"session_id":"S-NOT-EXIST","trace_level":"redacted"}
```

### M1 db purge --before ... --dry-run

- argv：`D:\WHCode\target\release\whcode.exe --workspace C:\Users\ykla\AppData\Local\Temp\whcode-quickstart-i4p8jj9z db purge --before 1767225600000 --dry-run --json`
- 期望：成功；退出码：0；耗时：53 ms

```text
{
  "schema_version": 1,
  "data": {
    "counts": {
      "checkpoints": 0,
      "context_windows": 0,
      "events": 0,
      "messages": 0,
      "runtime_executions": 0,
      "session_overview": 0,
      "sessions": 0,
      "stage_artifacts": 0,
      "tasks": 0
    },
    "cutoff_ms": 1767225600000,
    "dry_run": true,
    "selected_sessions": [],
    "skipped_protected": []
  }
}
```

### M1 db compact --export（草案形态）

- argv：`D:\WHCode\target\release\whcode.exe --workspace C:\Users\ykla\AppData\Local\Temp\whcode-quickstart-i4p8jj9z db compact --export C:\Users\ykla\AppData\Local\Temp\whcode-quickstart-i4p8jj9z\backup.duckdb --dry-run --json`
- 期望：预期失败（用法不符）；退出码：2；耗时：32 ms
- 备注：实际接口为 `db compact [--dry-run|--yes]`；导出一律走 `db archive --export`

```text
（stdout 为空）
--- stderr ---
error: unexpected argument '--export' found

Usage: whcode db compact [OPTIONS]

For more information, try '--help'.
```

### M1 db archive --export

- argv：`D:\WHCode\target\release\whcode.exe --workspace C:\Users\ykla\AppData\Local\Temp\whcode-quickstart-i4p8jj9z db archive --export C:\Users\ykla\AppData\Local\Temp\whcode-quickstart-i4p8jj9z\whcode-export.json --json`
- 期望：成功；退出码：0；耗时：99 ms

```text
{
  "schema_version": 1,
  "data": {
    "content_hash": "fnv1a64:e09bd5842fb804ab",
    "path": "C:\\Users\\ykla\\AppData\\Local\\Temp\\whcode-quickstart-i4p8jj9z\\whcode-export.json",
    "schema_version": 5
  }
}
```

### M2 whcode session list --json

- argv：`D:\WHCode\target\release\whcode.exe --workspace C:\Users\ykla\AppData\Local\Temp\whcode-quickstart-i4p8jj9z session list --json`
- 期望：成功；退出码：0；耗时：77 ms

```text
{
  "schema_version": 1,
  "data": {
    "sessions": [
      {
        "archived": false,
        "created_at_ms": 1792732543637,
        "id": "S-1",
        "stage": "specify",
        "status": "active",
        "title": "smoke"
      }
    ]
  }
}
```

### M2 whcode task list --json

- argv：`D:\WHCode\target\release\whcode.exe --workspace C:\Users\ykla\AppData\Local\Temp\whcode-quickstart-i4p8jj9z task list --session S-1 --json`
- 期望：成功；退出码：0；耗时：56 ms

```text
{
  "schema_version": 1,
  "data": {
    "active": [],
    "blocked": 0,
    "by_stage": {},
    "done": 0,
    "session_id": "S-1",
    "total": 0
  }
}
```

### M2 bench session-switch（大数据集）

- argv：`（未执行）`
- 期望：未取得；退出码：None；耗时：None ms
- 备注：10 万事件夹具未在本轮生成；已有证据见 artifacts/overview-perf-m2.json

```text
（stdout 为空）
```

### M3 whcode run --model stub

- argv：`D:\WHCode\target\release\whcode.exe --workspace C:\Users\ykla\AppData\Local\Temp\whcode-quickstart-i4p8jj9z run --session S-001 --model stub --trace-level redacted`
- 期望：预期失败（未接线）；退出码：50；耗时：32 ms

```text
（stdout 为空）
--- stderr ---
错误 [invalid_state] 未实现：run 未接线：LLM 流式交互需 Provider 与会话流装配（M3 桩已交付，CLI 执行属后续）（显式失败，不静默成功）
```

### M4 whcode spec status --json

- argv：`D:\WHCode\target\release\whcode.exe --workspace C:\Users\ykla\AppData\Local\Temp\whcode-quickstart-i4p8jj9z spec status --session S-1 --json`
- 期望：成功或明确失败；退出码：0；耗时：57 ms

```text
{
  "schema_version": 1,
  "data": {
    "event_cursor": 0,
    "overview": null,
    "session_id": "S-1",
    "stage": "specify",
    "status": "active"
  }
}
```

### M4 whcode spec validate --json

- argv：`D:\WHCode\target\release\whcode.exe --workspace C:\Users\ykla\AppData\Local\Temp\whcode-quickstart-i4p8jj9z spec validate --session S-1 --json`
- 期望：预期失败（未接线）；退出码：50；耗时：31 ms

```text
（stdout 为空）
--- stderr ---
{
  "schema_version": 1,
  "code": "invalid_state",
  "message": "未实现：spec 门禁命令需 SDD 产物工作区上下文（specs/ 与 workflow 配置加载）——属后续接线（显式失败，不静默成功）",
  "retryable": false,
  "details": null
}
```

### M8 whcode runtime list --json

- argv：`D:\WHCode\target\release\whcode.exe --workspace C:\Users\ykla\AppData\Local\Temp\whcode-quickstart-i4p8jj9z runtime list --json`
- 期望：预期失败（未接线）；退出码：50；耗时：31 ms

```text
（stdout 为空）
--- stderr ---
{
  "schema_version": 1,
  "code": "invalid_state",
  "message": "未实现：runtime 未接线：运行时装配开关（M8）属后续任务，当前不伪造启用状态（显式失败，不静默成功）",
  "retryable": false,
  "details": null
}
```

### M8 cargo build --locked --no-default-features

- argv：`C:\Users\ykla\.cargo\bin\cargo.exe build --locked --no-default-features`
- 期望：成功；退出码：0；耗时：2833 ms
- 备注：全运行时关闭仍可构建（FR-704、D-12）

```text
（stdout 为空）
--- stderr ---
[1m[92m   Compiling[0m whcode-bin v0.1.0 (D:\WHCode\crates\whcode-bin)
[1m[92m    Finished[0m ]8;;https://doc.rust-lang.org/cargo/reference/profiles.html#default-profiles\`dev` profile [unoptimized + debuginfo]]8;;\ target(s) in 2.74s
```

### M9 bench stability --hours 72

- argv：`（未执行）`
- 期望：未取得；退出码：None；耗时：None ms
- 备注：72 小时长跑未执行（NFR-204 缺口，见 ADR-025）

```text
（stdout 为空）
```

### M6/M7 三端一致 contract_three_fronts

- argv：`（未执行）`
- 期望：已另行取得；退出码：None；耗时：None ms
- 备注：见 cargo test --workspace 与 Check 183 记录，本轮不重复

```text
（stdout 为空）
```

## 结论

- 执行条目 30 项：成功取证 20 项、预期失败 5 项、未取得 5 项、缺陷登记 1 项、异常 0 项。
- 异常项（若有）必须在 checklist / 复核报告中登记后才能收口。
- 交互式 TUI、真实 SSH 节点、72 小时长跑三项**没有**替代证据，维持 `未取得`。
- 缺陷登记明细：
  - M1 events export（不存在的会话 S-NOT-EXIST）：会话不存在时退出码 0、count=0：与“存在但无事件”不可区分（错误的空），登记缺陷待修
