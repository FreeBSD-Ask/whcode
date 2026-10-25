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

/// 工作区证书库目录：`<workspace>/.whcode/certs`（ADR-040 决策 1）。
///
/// 与工作区数据库同一信任边界（同用户、同 ACL）；本函数不做存在性检查与创建，
/// 目录初始化与文件编排属 `whcode-app` 的证书库模块。
#[must_use]
pub fn workspace_certs_dir(workspace_root: &Path) -> PathBuf {
    workspace_config_dir(workspace_root).join("certs")
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

    /// ADR-040 决策 1：证书库落在工作区配置目录下的 `certs`，与数据库同一信任边界。
    #[test]
    fn certs_dir_is_under_workspace_config_dir() {
        let root = Path::new("/tmp/ws");
        assert_eq!(
            workspace_certs_dir(root),
            PathBuf::from("/tmp/ws/.whcode/certs")
        );
        assert_eq!(
            workspace_certs_dir(root),
            workspace_config_dir(root).join("certs"),
            "证书库目录必须与配置目录拼接一致（不另立第二套路径口径）"
        );
    }

    #[test]
    fn default_temp_dir_is_nonempty() {
        assert!(!default_temp_dir().as_os_str().is_empty());
    }
}
