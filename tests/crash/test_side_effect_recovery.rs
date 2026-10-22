//! 三时点崩溃注入与不确定恢复（T114、V-105/V-307、SC-005、FR-409/410、ADR-007）。
//!
//! 注入时点（与 `specs/001-whcode-harness/checklist.md` 的 Check 139/158/196/207 对应）：
//!
//! 1. **步骤边界**：状态已提交、下一步未开始；
//! 2. **意图提交后（外部操作前）**：`prepared`；
//! 3. **执行期间（副作用已发生、结果未登记）**：`running` + 副作用落盘；
//! 4. **登记后**：终态已写入——恢复不得把已知结果误标 `unknown`。
//!
//! 注入方式与 `test_durability.rs` 同款：父测试把自身以 `--exact --ignored` 再启动，子进程
//! 完成对应写入后 `std::process::abort()`（不运行析构、不关连接、不检查点）。父测试随后重开
//! 数据库并执行 `recover_uncertain`，断言“意图—执行—登记”协议的恢复结果。
//!
//! 断言边界（不得放宽）：
//! - `prepared`/`running` 必须变 `unknown`：既不宣称“未发生”，也不宣称“已发生但未登记不存在”；
//! - 已登记终态保持原状；
//! - 恢复**不重放副作用**：副作用标记文件只保留子进程写的那一行；
//! - 恢复幂等（第二次调用为空）。
//!
//! 子进程测试标注 `#[ignore]`：正常 `cargo test` 跳过；由父测试带环境变量调用。

use std::path::{Path, PathBuf};
use std::process::Command;

use duckdb::Connection;
use whcode_domain::{RuntimeExecutionStatus, TaskStatus};
use whcode_store::tasks::NewTask;
use whcode_store::{
    apply_migrations, create_session_with_id, get_session, get_task, insert_task, load,
    mark_running, record_intent, record_result, recover_uncertain, update_task_status,
};

const NOW: u64 = 1_700_000_000_000;

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("whcode-crash-{}-{tag}", std::process::id()));
    // 上次被中断时目录会遗留；Windows 复用 PID，先清再建（夹具幂等）。
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("创建临时目录");
    dir
}

fn append_side_effect(marker: &Path) {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(marker)
        .expect("打开副作用标记");
    writeln!(file, "side-effect").expect("写入副作用标记");
    file.sync_all().expect("落盘副作用标记");
}

fn side_effect_lines(marker: &Path) -> usize {
    std::fs::read_to_string(marker)
        .map(|text| text.lines().count())
        .unwrap_or(0)
}

/// 子进程入口：按注入点写入状态后强杀自身。
#[test]
#[ignore = "仅由父测试以子进程调用（三时点崩溃注入）"]
fn child_crash_point() {
    let Ok(point) = std::env::var("WHCODE_CRASH_POINT") else {
        return; // 直接 --ignored 运行：静默跳过，不产生副作用。
    };
    let db = std::env::var("WHCODE_CRASH_DB").expect("父进程须提供数据库路径");
    let marker = PathBuf::from(std::env::var("WHCODE_CRASH_MARKER").expect("父进程须提供标记路径"));
    let conn = Connection::open(&db).expect("子进程打开库");
    apply_migrations(&conn).expect("子进程迁移");

    match point.as_str() {
        // 时点 1：步骤边界——会话与任务状态已提交，下一步（工具执行）未开始。
        "boundary" => {
            create_session_with_id(&conn, "S-crash", "崩溃注入", "active", NOW).expect("建会话");
            insert_task(
                &conn,
                &NewTask {
                    id: "T-crash",
                    session_id: "S-crash",
                    title: "边界任务",
                    stage: "implement",
                    parent_id: None,
                    acceptance_json: "[]",
                },
                NOW,
            )
            .expect("建任务");
            update_task_status(&conn, "T-crash", TaskStatus::Running, 0, NOW + 1).expect("置状态");
        }
        // 时点 2：意图已提交、外部操作未开始。
        "intent" => {
            record_intent(&conn, "X-intent", "C-1", "hash-1").expect("登记意图");
        }
        // 时点 3：执行期间——副作用已发生，结果尚未登记。
        "during" => {
            record_intent(&conn, "X-during", "C-2", "hash-2").expect("登记意图");
            mark_running(&conn, "X-during", NOW).expect("进入执行");
            append_side_effect(&marker);
        }
        // 时点 4：结果已登记（终态），崩溃发生在登记之后。
        "registered" => {
            record_intent(&conn, "X-done", "C-3", "hash-3").expect("登记意图");
            mark_running(&conn, "X-done", NOW).expect("进入执行");
            record_result(
                &conn,
                "X-done",
                RuntimeExecutionStatus::Completed,
                Some("out-1"),
                Some("side-1"),
                NOW + 2,
            )
            .expect("登记结果");
        }
        other => panic!("未知注入点：{other}"),
    }
    eprintln!("子进程：注入点 {point} 已写入，即将 abort");
    std::process::abort();
}

/// 以子进程运行指定注入点，返回子进程 stderr（断言其确实非正常退出）。
fn inject(point: &str, dir: &Path) -> String {
    let db = dir.join("crash.duckdb");
    let marker = dir.join("side_effect.marker");
    {
        let conn = Connection::open(&db).expect("父进程建库");
        apply_migrations(&conn).expect("迁移");
    }
    let exe = std::env::current_exe().expect("定位当前测试二进制");
    let output = Command::new(exe)
        .args(["child_crash_point", "--exact", "--ignored", "--nocapture"])
        .env("WHCODE_CRASH_POINT", point)
        .env("WHCODE_CRASH_DB", &db)
        .env("WHCODE_CRASH_MARKER", &marker)
        .output()
        .expect("启动子进程");
    assert!(
        !output.status.success(),
        "子进程必须以 abort 非正常退出；注入点 {point}：{output:?}"
    );
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn reopen(dir: &Path) -> Connection {
    Connection::open(dir.join("crash.duckdb")).expect("重开库")
}

#[test]
fn boundary_crash_keeps_committed_state_and_recovers_nothing() {
    let dir = temp_dir("boundary");
    let stderr = inject("boundary", &dir);
    assert!(stderr.contains("boundary"), "子进程未按预期执行：{stderr}");

    let conn = reopen(&dir);
    let session = get_session(&conn, "S-crash")
        .expect("查会话")
        .expect("会话必须存活");
    assert_eq!(session.id, "S-crash");
    let task = get_task(&conn, "T-crash")
        .expect("查任务")
        .expect("任务必须存活");
    assert_eq!(task.status, TaskStatus::Running, "已提交状态不得被恢复改写");

    let mut conn = conn;
    let report = recover_uncertain(&mut conn).expect("恢复");
    assert!(
        report.is_empty(),
        "步骤边界崩溃不应产生不确定执行：{report:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn intent_crash_marks_unknown_before_any_side_effect() {
    let dir = temp_dir("intent");
    inject("intent", &dir);

    let mut conn = reopen(&dir);
    let report = recover_uncertain(&mut conn).expect("恢复");
    assert_eq!(report.len(), 1, "意图记录必须被恢复：{report:?}");
    assert_eq!(report.recovered[0].id, "X-intent");
    assert_eq!(
        report.recovered[0].previous_state,
        RuntimeExecutionStatus::Prepared
    );
    assert!(!report.truncated);
    assert!(report.next_action.contains("核对"), "必须给出处置建议");

    let row = load(&conn, "X-intent").expect("查").expect("存在");
    assert_eq!(row.state, RuntimeExecutionStatus::Unknown);
    assert_eq!(row.started_at_ms, None, "未进入执行，started_at 必须为空");
    assert_eq!(row.side_effect_ref, None);
    assert_eq!(
        side_effect_lines(&dir.join("side_effect.marker")),
        0,
        "意图阶段不应产生副作用"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn side_effect_before_register_marks_unknown_and_is_not_replayed() {
    let dir = temp_dir("during");
    inject("during", &dir);

    let marker = dir.join("side_effect.marker");
    assert_eq!(side_effect_lines(&marker), 1, "子进程必须已产生一次副作用");

    let mut conn = reopen(&dir);
    let report = recover_uncertain(&mut conn).expect("恢复");
    assert_eq!(report.len(), 1, "{report:?}");
    assert_eq!(
        report.recovered[0].previous_state,
        RuntimeExecutionStatus::Running
    );

    let row = load(&conn, "X-during").expect("查").expect("存在");
    assert_eq!(
        row.state,
        RuntimeExecutionStatus::Unknown,
        "副作用已发生但未登记，必须标 unknown"
    );
    assert!(row.started_at_ms.is_some(), "执行已开始");
    assert_eq!(row.finished_at_ms, None, "结果未登记，不得伪造结束时间");
    assert_eq!(
        row.side_effect_ref, None,
        "未登记即无副作用凭证——这正是需要核对的原因"
    );

    assert_eq!(
        side_effect_lines(&marker),
        1,
        "恢复不得重放副作用（标记文件仍只有一行）"
    );

    // 幂等：再次恢复不得重复报告或改写。
    let again = recover_uncertain(&mut conn).expect("再恢复");
    assert!(again.is_empty(), "恢复必须幂等：{again:?}");
    assert_eq!(side_effect_lines(&marker), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn registered_result_is_not_marked_unknown() {
    let dir = temp_dir("registered");
    inject("registered", &dir);

    let mut conn = reopen(&dir);
    let report = recover_uncertain(&mut conn).expect("恢复");
    assert!(report.is_empty(), "已登记结果不得进入恢复清单：{report:?}");
    let row = load(&conn, "X-done").expect("查").expect("存在");
    assert_eq!(row.state, RuntimeExecutionStatus::Completed);
    assert_eq!(row.output_ref.as_deref(), Some("out-1"));
    assert_eq!(row.side_effect_ref.as_deref(), Some("side-1"));
    assert!(row.finished_at_ms.is_some());
    let _ = std::fs::remove_dir_all(&dir);
}
