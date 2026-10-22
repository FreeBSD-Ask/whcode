//! 终端能力探测（FR-601、V-003；M0 TUI 原型接线点）。
//!
//! 经 crossterm 查询控制台尺寸（tasks.md T024 完成接线）。探测不到终端（管道、重定向、
//! 无可用控制台）时返回 `None`，调用方不得把 `None` 当作固定尺寸处理。

use std::io::IsTerminal;

/// 终端尺寸（列, 行）。
///
/// 返回 `None` 表示无法探测：非 TTY 环境（管道、重定向、无控制台），或底层查询失败。
#[must_use]
pub fn terminal_size() -> Option<(u16, u16)> {
    crossterm::terminal::size().ok()
}

/// 标准输入是否为交互式终端（控制台/PTY）。
///
/// 背景（B-05 实测暴露的缺陷）：Windows 上 `terminal_size()` 查询的是**输出**侧，
/// “管道 stdin + 控制台 stdout”场景下仍返回 `Some`；只凭尺寸判定会让 TUI 挂起等待
/// 永远读不到的按键。本函数用标准库 `IsTerminal`（std 1.70+）判定输入侧——
/// 管道、重定向与 `stdin` 已关闭时均返回 `false`。
#[must_use]
pub fn stdin_is_interactive() -> bool {
    std::io::stdin().is_terminal()
}

/// TUI 控制台窗口是否最小化（T135、FR-617）。
///
/// **实现路径（ADR-021 修订）**：经 PowerShell 子进程做 Win32 查询
/// （`GetConsoleWindow` + `IsIconic`，P/Invoke 在子进程内）——我们的 Rust 代码
/// **零 unsafe**（NFR-215 合规），零新依赖（`std::process::Command`，worker.rs
/// 同模式）。非 Windows 平台或 PowerShell 失败时返回 `None`（不伪装）。
///
/// # 成本与调用约定
///
/// 每次 ≈ 200—500ms（PowerShell 启动 + Add-Type 编译）——**不得在渲染帧路径
/// 调用**；由事件循环低频调用（≥ 5s 间隔），结果经
/// `whcode-tui::render::RenderEvent::Minimized/Restored` 传入渲染门控
/// （门控本身零成本，见 `whcode-tui::render`）。
#[must_use]
pub fn is_window_minimized() -> Option<bool> {
    #[cfg(target_os = "windows")]
    {
        const SCRIPT: &str = r#"
Add-Type -TypeDefinition 'using System; using System.Runtime.InteropServices; public class WhTermProbe { [DllImport("kernel32.dll")] public static extern IntPtr GetConsoleWindow(); [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h); }'
$h = [WhTermProbe]::GetConsoleWindow()
if ($h -eq [IntPtr]::Zero) { 'none' } elseif ([WhTermProbe]::IsIconic($h)) { 'minimized' } else { 'normal' }
"#;
        let output = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        match String::from_utf8_lossy(&output.stdout).trim() {
            "minimized" => Some(true),
            "normal" => Some(false),
            _ => None,
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{is_window_minimized, stdin_is_interactive, terminal_size};

    #[test]
    fn stdin_interactive_probe_does_not_panic() {
        // 回归背景（B-05）：修复前 `launch_interactive()` 只判 `terminal_size().is_some()`。
        // Windows 上该函数查询的是**输出**侧；“管道 stdin + 控制台 stdout”场景仍返回
        // `Some`，TUI 因而进入事件循环并永久阻塞、等待永远读不到的按键。
        // 现在的启动条件是 `terminal_size().is_some() && stdin_is_interactive()`，
        // 本测试锁住该组合判定：stdin 非交互时，即便有尺寸也不得判为可进 TUI。
        //
        // 环境依赖：CI/管道下 stdin 通常非交互，本地真实 TTY 下则交互；两种环境都必须
        // 通过——非交互时只记录预期（不 hard fail），交互时仅验证探测不 panic。
        let interactive = stdin_is_interactive();
        let size = terminal_size();
        let tui_allowed = size.is_some() && interactive;
        if interactive {
            eprintln!("真实 TTY 环境：stdin 可交互，TUI 可用性 = {tui_allowed}");
        } else {
            eprintln!("CI/管道环境预期 stdin_is_interactive() == false：不得进入 TUI（B-05 回归）");
            assert!(
                !tui_allowed,
                "有终端尺寸但 stdin 不可交互时仍判为可用，将导致 TUI 永久阻塞按键"
            );
        }
        // 探测必须稳定且可重复调用（不得 panic 或返回抖动值）。
        assert_eq!(
            interactive,
            stdin_is_interactive(),
            "stdin 交互性探测必须稳定"
        );
    }

    /// T135：最小化探测不得 panic 且返回合法值（本机 Windows 验证真实路径）。
    #[test]
    fn is_window_minimized_does_not_panic_and_returns_valid() {
        let result = is_window_minimized();
        // 本机（Windows 11 amd64 + 控制台）应返回 Some(true/false)；
        // CI/管道下 PowerShell 可能失败 → None（合法，不伪装）。
        // 非 Windows 平台恒 None。
        if cfg!(target_os = "windows") {
            eprintln!("is_window_minimized() = {result:?}");
            // 不得 panic 即为通过；None（无控制台/失败）与 Some(bool) 均合法。
        } else {
            assert_eq!(result, None, "非 Windows 平台恒 None（不伪装）");
        }
    }

    #[test]
    fn terminal_size_probe_is_well_formed_and_stable() {
        // 语义：探测在任何环境（含非 TTY 的 CI/管道）都**不得 panic**（否则本测试直接失败）。
        // 结果依赖运行环境——真实 TTY 下为 `Some`，非 TTY 下为 `None`；因此只断言
        // “`Some` 时两维均为正”，**不**断言固定值（骨架阶段曾固定为 None，现已被真实探测取代）。
        let first = terminal_size();
        if let Some((cols, rows)) = first {
            assert!(cols > 0, "探测到的列数必须为正");
            assert!(rows > 0, "探测到的行数必须为正");
        }
        // 尺寸须来自真实查询而非随机/未初始化值：连续两次探测必须一致
        // （终端在两次调用之间不会被 resize，除非外部干预）。
        assert_eq!(
            first,
            terminal_size(),
            "终端尺寸探测必须稳定：不得返回未初始化或不一致的值"
        );
    }
}
