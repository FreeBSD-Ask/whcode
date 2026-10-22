//! # 进程清理零残留验证（Check 201、FR-409、§10.2）
//!
//! 复核期间实测到 `durability` 子进程残留（进度复核报告 §9），说明“终止成功”不等于
//! “无残留”。本测试启动一棵真实的进程树（`cmd` → `ping`），用
//! `whcode_platform::terminate_tree` 终止后逐个断言父、子进程都已消失。
//!
//! 平台边界（不伪装）：`child_pids` / `process_exists` 在非 Windows 平台返回结构化
//! 错误（BSD 进程组属 M7，ADR-013 延后）；此时用例显式打印“未执行”并返回，
//! 不产生通过结论。信号链：若未观测到子进程，用例失败——否则“零残留”无从判定。

use std::time::{Duration, Instant};

use whcode_platform::{child_pids, process_exists, sleep_command, terminate_tree};

/// 轮询等待子进程出现（`cmd` 启动 `ping` 需要毫秒级时间）。
fn wait_for_children(parent: u32, timeout: Duration) -> Result<Vec<u32>, String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match child_pids(parent) {
            Ok(pids) if !pids.is_empty() => return Ok(pids),
            Ok(_) => std::thread::sleep(Duration::from_millis(100)),
            Err(err) => return Err(err.to_string()),
        }
    }
    Err(format!("等待 {timeout:?} 仍未观测到 {parent} 的子进程"))
}

/// 轮询等待进程消失（终止是异步的）。
fn wait_until_gone(pid: u32, timeout: Duration) -> Result<bool, String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match process_exists(pid) {
            Ok(false) => return Ok(true),
            Ok(true) => std::thread::sleep(Duration::from_millis(100)),
            Err(err) => return Err(err.to_string()),
        }
    }
    Ok(false)
}

/// Check 201：终止进程树后父与子进程均无残留。
#[test]
fn terminated_process_tree_leaves_no_residual_processes() {
    // `sleep_command` 保证子进程活得比本用例长（30 秒延时），避免“自己先退出”。
    let mut child = sleep_command(30)
        .spawn()
        .expect("启动受测进程树（cmd → ping）");
    let parent = child.id();

    let grandchildren = match wait_for_children(parent, Duration::from_secs(10)) {
        Ok(pids) => pids,
        Err(err) if err.contains("延后") => {
            // 非 Windows：平台查询未实现，显式跳过（不产生通过结论）。
            let _ = child.kill();
            let _ = child.wait();
            eprintln!("未执行零残留验证（平台未实现查询，BSD 属 M7 延后）：{err}");
            return;
        }
        Err(err) => panic!("无法观测子进程，零残留判据无从成立：{err}"),
    };
    assert!(
        !grandchildren.is_empty(),
        "未观测到子进程（parent={parent}），测试无意义"
    );

    terminate_tree(parent).expect("终止进程树");
    let _ = child.wait();

    assert!(
        wait_until_gone(parent, Duration::from_secs(10)).expect("父进程查询"),
        "父进程 {parent} 终止后仍存在（残留）"
    );
    for pid in &grandchildren {
        assert!(
            wait_until_gone(*pid, Duration::from_secs(10)).expect("子进程查询"),
            "子进程 {pid} 在树终止后仍存在（残留）"
        );
    }
}
