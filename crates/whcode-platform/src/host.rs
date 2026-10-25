//! # 本机标识与文件权限小工具（T187、ADR-040 决策 1／3）
//!
//! 两个平台差异必须收敛在本 crate（NFR-101）：
//!
//! 1. [`hostname`]：本机名的读取顺序与空值判定。入站自签证书的 SAN 集合需要本机名
//!    （ADR-040 决策 3）；读取失败时返回 `None`，由调用方决定是否省略该 SAN，不得以
//!    空串冒充“已知本机名”。
//! 2. [`restrict_private_file`]：私钥文件的最小权限。Unix 上设为 `0600`；Windows 依赖
//!    文件继承 ACL（与工作区数据库同一信任边界，ADR-040 威胁模型），因此返回 `Ok(())`
//!    而不做平台专有 ACL 改写。
//!
//! 边界：不创建文件、不读取文件内容；调用方负责先写入再收紧权限。

use std::path::Path;

use whcode_domain::WhError;

/// 本机名（优先 `COMPUTERNAME`，其次 `HOSTNAME`；trim 后为空视为缺失）。
///
/// 返回 `None` 表示两个环境变量都缺失或为空。本函数不做 DNS 解析、不保证可路由。
#[must_use]
pub fn hostname() -> Option<String> {
    for key in ["COMPUTERNAME", "HOSTNAME"] {
        let Some(value) = std::env::var_os(key) else {
            continue;
        };
        let text = value.to_string_lossy();
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    None
}

/// 收紧私钥文件权限：Unix 设 `0600`，Windows 返回 `Ok(())`（继承 ACL）。
///
/// # Errors
///
/// Unix 上读取元数据或设置权限失败时返回 [`ErrorCode::IoError`]，消息含文件路径与
/// 补救方向（检查文件是否仍存在、当前用户是否为文件所有者）。
pub fn restrict_private_file(path: &Path) -> Result<(), WhError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let metadata = std::fs::metadata(path).map_err(|err| {
            WhError::new(
                whcode_domain::ErrorCode::IoError,
                format!(
                    "读取私钥文件 {} 的权限失败：{err}；确认文件已写入且当前用户可访问",
                    path.display()
                ),
            )
        })?;
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o600);
        std::fs::set_permissions(path, permissions).map_err(|err| {
            WhError::new(
                whcode_domain::ErrorCode::IoError,
                format!(
                    "设置私钥文件 {} 权限为 0600 失败：{err}；确认当前用户为文件所有者",
                    path.display()
                ),
            )
        })?;
    }
    #[cfg(not(unix))]
    {
        // Windows 依赖文件继承 ACL（ADR-040 决策 1）；此处不引入平台专有 ACL 改写，
        // 以与工作区数据库同一信任边界为准。
        let _ = path;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    #[cfg(unix)]
    use whcode_domain::ErrorCode;

    fn temp_file(tag: &str) -> PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let dir =
            std::env::temp_dir().join(format!("whcode-host-test-{}-{stamp}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("创建测试临时目录");
        dir.join(format!("{tag}.pem"))
    }

    /// 本机名：缺失或非空，且已 trim（不出现首尾空白）。
    #[test]
    fn hostname_is_absent_or_trimmed_nonempty() {
        match hostname() {
            Some(name) => {
                assert!(!name.is_empty(), "本机名不得为空串");
                assert_eq!(name.trim(), name, "本机名必须已 trim");
            }
            None => {
                let source_missing = ["COMPUTERNAME", "HOSTNAME"]
                    .iter()
                    .all(|key| std::env::var_os(key).is_none_or(|value| value.is_empty()));
                assert!(
                    source_missing,
                    "仅当两个环境变量都缺失或为空时才允许返回 None"
                );
            }
        }
    }

    /// 私钥权限：Unix 断言 `0600`；Windows 断言不报错（依赖继承 ACL）。
    #[test]
    fn restrict_private_file_sets_mode_on_unix_and_is_noop_on_windows() {
        let path = temp_file("key");
        std::fs::write(&path, b"test-only").expect("写测试文件");
        restrict_private_file(&path).expect("收紧权限必须成功");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path)
                .expect("读取元数据")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "私钥文件权限必须为 0600");
        }

        let _ = std::fs::remove_file(&path);
    }

    /// 不存在的路径在 Unix 上返回可操作 `io_error`（不 panic）；Windows 返回 `Ok`。
    #[test]
    fn restrict_private_file_reports_missing_path_on_unix() {
        let path = temp_file("missing");
        let result = restrict_private_file(&path);
        #[cfg(unix)]
        {
            let err = result.expect_err("Unix 上缺失文件必须报错");
            assert_eq!(err.code, ErrorCode::IoError);
            assert!(err.message.contains("私钥文件"), "{}", err.message);
            assert!(
                err.message.contains(&path.display().to_string()),
                "错误须含路径：{}",
                err.message
            );
        }
        #[cfg(not(unix))]
        {
            result.expect("Windows 不读取文件，直接返回 Ok");
        }
    }
}
