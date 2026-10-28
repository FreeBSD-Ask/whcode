//! # 工作区路径绑定（FR-729、ADR-048 §2.7）
//!
//! 工具的一切路径参数都解析为**工作区内相对路径**：拒绝绝对路径（含 Windows 盘符与
//! UNC root）与包含 `..` 分量的逃逸尝试——BH finding-bound 拓扑绑定的 fail-closed
//! 教训（`research/better-harness/` finding-bound-fix.md：拒绝绝对/逃逸路径）。
//!
//! 语义：词法规范化（不做 `canonicalize`——写路径的目标可能尚不存在）；规范化后仍保留
//! `..` 或前缀逃逸即拒绝，不回退、不猜测替代路径。

use std::path::{Component, Path, PathBuf};

use whcode_domain::WhError;

/// 把模型声明的路径参数绑定到工作区内：拒绝绝对路径与 `..` 逃逸（fail-closed）。
///
/// # Errors
///
/// 绝对路径、包含 `..` 分量或空路径返回 [`ErrorCode::InvalidState`]。
pub fn resolve_in_workspace(workspace: &Path, raw: &str) -> Result<PathBuf, WhError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(WhError::invalid_state("路径参数不得为空"));
    }
    let candidate = Path::new(trimmed);
    if candidate.is_absolute() || candidate.has_root() {
        return Err(WhError::invalid_state(format!(
            "拒绝绝对路径（须为工作区内相对路径）：{trimmed}"
        )));
    }
    let mut resolved = workspace.to_path_buf();
    for component in candidate.components() {
        match component {
            Component::Normal(part) => resolved.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(WhError::invalid_state(format!(
                    "拒绝包含 .. 的路径（工作区逃逸）：{trimmed}"
                )));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(WhError::invalid_state(format!(
                    "拒绝路径前缀/根分量：{trimmed}"
                )));
            }
        }
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use whcode_domain::ErrorCode;

    #[test]
    fn rejects_absolute_paths_portably() {
        // 不用 cfg 分支（平台差异只在 whcode-platform）：运行时取当前平台的绝对路径
        // （temp_dir 恒为绝对），断言两种盘符/根形态都被拒绝。
        let workspace = Path::new("D:/WHCode");
        let absolute = std::env::temp_dir();
        let raw = absolute.to_string_lossy().into_owned();
        let err = resolve_in_workspace(workspace, &raw).expect_err("绝对路径必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        let err = resolve_in_workspace(workspace, "/etc/passwd").expect_err("根分量必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
    }

    #[test]
    fn rejects_parent_escape_and_empty() {
        let workspace = Path::new("D:/WHCode");
        for raw in ["../outside", "a/../../b", "", "   "] {
            let err = resolve_in_workspace(workspace, raw).expect_err(raw);
            assert_eq!(err.code, ErrorCode::InvalidState, "{raw}");
        }
    }

    #[test]
    fn resolves_normal_relative_paths_inside_workspace() {
        let workspace = Path::new("D:/WHCode");
        // 即便可回退到工作区内，`..` 也必须拒绝（fail-closed，不做词法回退猜测）。
        let err = resolve_in_workspace(workspace, "crates/../whcode-tools/src/lib.rs")
            .expect_err(".. 分量必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        let ok = resolve_in_workspace(workspace, "src/lib.rs").expect("普通相对路径");
        assert_eq!(ok, Path::new("D:/WHCode").join("src/lib.rs"));
        let dotted = resolve_in_workspace(workspace, "./src/lib.rs").expect("CurDir 分量剔除");
        assert_eq!(dotted, Path::new("D:/WHCode").join("src/lib.rs"));
    }
}
