//! # 进程清理零残留验证（Check 201、FR-409、§10.2）
//!
//! 复核期间实测到 `durability` 子进程残留（进度复核报告 §9），说明“终止成功”不等于
//! “无残留”。本测试启动一棵真实的进程树（Windows `cmd` → `ping`；类 Unix
//! `sh` → `sleep` ×2），用 `whcode_platform::terminate_tree` 终止后逐个断言父、子
//! 进程都已消失。
//!
//! 平台等价（2026-10-31 用户指令，解除 BSD 延后）：`child_pids`／`process_exists` 在
//! 类 Unix 经 `ps`／`kill -s 0` 实现，两条路径都在 CI 真实执行；任何“查询失败”都是
//! 失败而非跳过。信号链：若未观测到子进程，用例失败——否则“零残留”无从判定。

use std::time::{Duration, Instant};

use whcode_platform::{
    child_pids, configure_child_process_group, is_windows, process_exists, shell_command,
    sleep_command, terminate_tree,
};

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
    // 两层树（运行时分支，不用 cfg——平台差异经 whcode-platform 收敛，NFR-101）：
    // Windows `cmd` → `ping`（sleep_command）；类 Unix `sh` → `sleep` ×2，`wait`
    // 保住 shell 使其存在可观测的子进程。子进程活得比本用例长（30 秒延时），
    // 避免“自己先退出”。类 Unix 必须先配置进程组，terminate_tree 才覆盖整组。
    let mut command = if is_windows() {
        sleep_command(30)
    } else {
        shell_command("sleep 30 & sleep 30 & wait")
    };
    configure_child_process_group(&mut command);
    let mut child = command.spawn().expect("启动受测进程树");
    let parent = child.id();

    let grandchildren = match wait_for_children(parent, Duration::from_secs(10)) {
        Ok(pids) => pids,
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
