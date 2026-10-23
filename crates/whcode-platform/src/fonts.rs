//! # 用户级字体安装的平台实现（FR-717、ADR-028 修订 1）
//!
//! 注册表经 `reg.exe` 子进程读写 HKCU（用户级，无需管理员）：零 unsafe（NFR-215）、
//! 零新依赖（同 `term.rs` 的子进程先例，ADR-021）。`whcode_app::fonts::FontRegistry`
//! 的实现 glue 在装配根（`whcode-bin`），本模块只提供实现材料——避免 platform
//! 反向依赖 app。
//!
//! 非 Windows 平台：显式返回 [`ErrorCode::RuntimeUnavailable`]，不伪装成功。

use std::path::PathBuf;

use whcode_domain::{ErrorCode, WhError};

/// HKCU 用户级字体注册键（值名 = 显示名 + (TrueType)/(OpenType)，数据 = 文件完整路径）。
pub const HKCU_FONTS_KEY: &str = r"Software\Microsoft\Windows NT\CurrentVersion\Fonts";

/// 带根前缀的完整键路径（reg.exe 参数）。
const HKCU_FONTS_KEY_FULL: &str = concat!(
    "HKCU\\",
    r"Software\Microsoft\Windows NT\CurrentVersion\Fonts"
);

/// 用户级字体目录：`%LOCALAPPDATA%\Microsoft\Windows\Fonts`。
///
/// `LOCALAPPDATA` 缺失时回退 `%USERPROFILE%\AppData\Local`；两者都缺失返回空路径
/// （调用方 `create_dir_all` 时自然失败，不伪装）。
#[must_use]
pub fn user_fonts_dir() -> PathBuf {
    match local_appdata() {
        Some(base) => base.join(r"Microsoft\Windows\Fonts"),
        None => PathBuf::new(),
    }
}

/// Windows Terminal settings.json 候选路径（Store 稳定版 → 预览版 → 解包安装）。
///
/// 只读检测用：全部未命中时由调用方返回“未检测到”，不视为错误（FR-717）。
/// 第三处是解包安装（scoop 等）的社区常见位置，官方文档未单列，标 `[待验证]`；
/// 漏检只影响声明检测，不影响安装本身。
#[must_use]
pub fn windows_terminal_settings_paths() -> Vec<PathBuf> {
    let Some(base) = local_appdata() else {
        return Vec::new();
    };
    vec![
        base.join(r"Packages\Microsoft.WindowsTerminal_8wekyb3d8bbwe\LocalState\settings.json"),
        base.join(
            r"Packages\Microsoft.WindowsTerminalPreview_8wekyb3d8bbwe\LocalState\settings.json",
        ),
        base.join(r"Microsoft\Windows Terminal\settings.json"),
    ]
}

/// `%LOCALAPPDATA%`，缺失时回退 `%USERPROFILE%\AppData\Local`。
fn local_appdata() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("LOCALAPPDATA")
        && !dir.is_empty()
    {
        return Some(PathBuf::from(dir));
    }
    std::env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join(r"AppData\Local"))
}

/// 查询 HKCU Fonts 键的值（存在返回 REG_SZ 数据，本模块注册的值是字体文件完整路径）。
///
/// # Errors
///
/// 非 Windows 平台或 reg.exe 启动失败时返回 [`WhError`]；值不存在返回 `Ok(None)`。
pub fn reg_query_value(value_name: &str) -> Result<Option<String>, WhError> {
    #[cfg(target_os = "windows")]
    {
        let output = run_reg(["query", HKCU_FONTS_KEY_FULL, "/v", value_name])?;
        if !output.status.success() {
            // reg query 对不存在的值返回非零退出码——这是“未注册”的正常路径。
            return Ok(None);
        }
        Ok(Some(reg_sz_data(&String::from_utf8_lossy(&output.stdout))))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = value_name;
        Err(not_windows())
    }
}

/// 写入 REG_SZ 值（指向字体文件完整路径；已存在时覆盖）。
///
/// # Errors
///
/// 非 Windows 平台或 reg.exe 失败时返回 [`WhError`]。
pub fn reg_set_value(value_name: &str, data: &str) -> Result<(), WhError> {
    #[cfg(target_os = "windows")]
    {
        let output = run_reg([
            "add",
            HKCU_FONTS_KEY_FULL,
            "/v",
            value_name,
            "/t",
            "REG_SZ",
            "/d",
            data,
            "/f",
        ])?;
        if output.status.success() {
            return Ok(());
        }
        Err(reg_failed("add", value_name, &output))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (value_name, data);
        Err(not_windows())
    }
}

/// 删除值；返回是否实际删除（本就不存在返回 `false`）。
///
/// 先 query 再 delete：reg delete 对“不存在”的失败输出随系统语言变化，不能靠
/// 文本匹配区分——退出码 + 预查询才是可靠判定。
///
/// # Errors
///
/// 非 Windows 平台、reg.exe 启动失败或删除被拒时返回 [`WhError`]。
pub fn reg_delete_value(value_name: &str) -> Result<bool, WhError> {
    #[cfg(target_os = "windows")]
    {
        if reg_query_value(value_name)?.is_none() {
            return Ok(false);
        }
        let output = run_reg(["delete", HKCU_FONTS_KEY_FULL, "/v", value_name, "/f"])?;
        if output.status.success() {
            return Ok(true);
        }
        Err(reg_failed("delete", value_name, &output))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = value_name;
        Err(not_windows())
    }
}

#[cfg(target_os = "windows")]
fn run_reg<const N: usize>(args: [&str; N]) -> Result<std::process::Output, WhError> {
    std::process::Command::new("reg")
        .args(args)
        .output()
        .map_err(|err| WhError::new(ErrorCode::IoError, format!("reg.exe 启动失败：{err}")))
}

#[cfg(target_os = "windows")]
fn reg_failed(action: &str, value_name: &str, output: &std::process::Output) -> WhError {
    WhError::new(
        ErrorCode::IoError,
        format!(
            "reg {action} {value_name} 失败（退出码 {:?}）：{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    )
}

/// 从 reg query 输出中提取 REG_SZ 行的数据段（`名字    REG_SZ    数据`）。
#[cfg(target_os = "windows")]
fn reg_sz_data(stdout: &str) -> String {
    stdout
        .lines()
        .find(|line| line.contains("REG_SZ"))
        .and_then(|line| line.split("REG_SZ").nth(1))
        .map_or_else(String::new, |data| data.trim().to_string())
}

#[cfg(not(target_os = "windows"))]
fn not_windows() -> WhError {
    WhError::new(
        ErrorCode::RuntimeUnavailable,
        "字体安装仅在 Windows 平台可用（ADR-013 验收目标；其他平台未实现，不伪装成功）",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hkcu_fonts_key_is_user_scope_current_version() {
        assert!(HKCU_FONTS_KEY.starts_with(r"Software\Microsoft\Windows NT"));
        assert!(HKCU_FONTS_KEY.contains(r"CurrentVersion\Fonts"));
        assert!(HKCU_FONTS_KEY_FULL.starts_with("HKCU\\"));
    }

    #[test]
    fn terminal_paths_cover_store_preview_and_unpackaged() {
        match local_appdata() {
            // 无 LOCALAPPDATA 也无 USERPROFILE 的环境（精简 CI 容器）：显式为空。
            None => {
                assert!(windows_terminal_settings_paths().is_empty());
                assert!(user_fonts_dir().as_os_str().is_empty());
            }
            Some(base) => {
                let paths = windows_terminal_settings_paths();
                assert_eq!(paths.len(), 3);
                assert!(paths[0].starts_with(&base));
                assert!(
                    paths[0]
                        .to_string_lossy()
                        .contains(r"Microsoft.WindowsTerminal_8wekyb3d8bbwe"),
                    "Store 版包路径固定：{}",
                    paths[0].display()
                );
                assert!(user_fonts_dir().ends_with(r"Microsoft\Windows\Fonts"));
            }
        }
    }
}
