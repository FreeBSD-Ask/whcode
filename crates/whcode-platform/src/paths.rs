//! 路径约定：临时目录与工作区配置目录（§5.4）。
//!
//! 临时目录必须落在工作区配置的专用目录下，且启动时检查可写性与磁盘余量——"写到系统
//! 临时目录"只是各平台默认值的来源，不是绕过检查的理由。

use std::path::{Path, PathBuf};

/// 返回当前平台的默认临时目录（供配置默认值使用；实际目录仍须通过工作区配置与启动检查）。
#[must_use]
pub fn default_temp_dir() -> PathBuf {
    std::env::temp_dir()
}

/// 工作区配置目录：`<workspace>/.whcode`。
///
/// 传入的工作区路径应为绝对路径；本函数不做存在性检查（由配置加载与 `doctor` 负责）。
#[must_use]
pub fn workspace_config_dir(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".whcode")
}

/// 工作区专用临时目录：`<workspace>/tmp`（§5.4 `temp_directory`）。
///
/// DuckDB `temp_directory` 必须指向工作区内受控目录，而非系统临时目录——这样磁盘余量与
/// `max_temp_directory_size` 的约束才有明确归属；可写性检查在启动期进行（§5.4）。
#[must_use]
pub fn workspace_temp_dir(workspace_root: &Path) -> PathBuf {
    workspace_root.join("tmp")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_dir_is_dot_whcode_under_workspace() {
        let root = Path::new("/tmp/ws");
        assert_eq!(workspace_config_dir(root), PathBuf::from("/tmp/ws/.whcode"));
    }

    #[test]
    fn temp_dir_is_inside_workspace() {
        let root = Path::new("/tmp/ws");
        assert_eq!(workspace_temp_dir(root), PathBuf::from("/tmp/ws/tmp"));
    }

    #[test]
    fn default_temp_dir_is_nonempty() {
        assert!(!default_temp_dir().as_os_str().is_empty());
    }
}
