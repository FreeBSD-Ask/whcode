//! 工作区文件索引（GUI「@ 文件」候选，ADR-102 第六批「仍缺」项收口）。
//!
//! GUI 只声明 [`whcode_gui::FileIndex`] 接口、不触碰文件系统（分层门禁）；遍历在本模块，
//! 由装配根注入。约束：
//! - **有界**：深度与条目数均设上限，避免大仓库拖慢输入响应（项目优先序 1：资源占用）；
//! - **不跟随符号链接**：防止经链接逃出工作区（安全优先序 2）；
//! - **跳过构建与版本控制目录**：`.git`／`target`／`node_modules`／`vendor`／`.whcode`
//!   等，它们的文件不是「@ 文件」的合理候选，且体量最大；
//! - 返回**相对路径**（与提示词里引用的形式一致），匹配为大小写不敏感子串。

use std::path::{Path, PathBuf};

use whcode_gui::FileIndex;

/// 遍历深度上限（工作区根为 0）。
const MAX_DEPTH: usize = 8;

/// 单次遍历最多收集的条目数（超出即停止，保证输入响应）。
const MAX_ENTRIES: usize = 4_000;

/// 每次返回的最大候选数（GUI 列表有界）。
const MAX_RESULTS: usize = 50;

/// 跳过的目录名（构建产物、版本控制、依赖与内部状态）。
const SKIPPED_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "target",
    "node_modules",
    "vendor",
    ".whcode",
    "dist",
    "build",
    ".cache",
];

/// 工作区文件索引：按需遍历（不建常驻缓存，避免随工作区增长占用内存）。
pub struct WorkspaceFileIndex {
    root: PathBuf,
}

impl WorkspaceFileIndex {
    /// 以工作区根构造。
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// 深度优先收集相对路径（有界）。
    fn collect(&self, dir: &Path, depth: usize, out: &mut Vec<String>) {
        if depth > MAX_DEPTH || out.len() >= MAX_ENTRIES {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            if out.len() >= MAX_ENTRIES {
                return;
            }
            let path = entry.path();
            // `file_type()` 不跟随符号链接：链接本身既不入列也不递归。
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if SKIPPED_DIRS.contains(&name.as_ref()) {
                    continue;
                }
                self.collect(&path, depth + 1, out);
            } else if file_type.is_file()
                && let Ok(relative) = path.strip_prefix(&self.root)
            {
                out.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
}

impl FileIndex for WorkspaceFileIndex {
    fn files(&self, needle: &str, limit: usize) -> Vec<String> {
        let limit = limit.min(MAX_RESULTS);
        if limit == 0 {
            return Vec::new();
        }
        let mut all = Vec::new();
        self.collect(&self.root, 0, &mut all);
        let needle = needle.trim().to_lowercase();
        let mut matched: Vec<String> = if needle.is_empty() {
            all
        } else {
            all.into_iter()
                .filter(|path| path.to_lowercase().contains(&needle))
                .collect()
        };
        // 确定性输出：短路径优先（更可能是用户想要的顶层文件），同长按字典序。
        matched.sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
        matched.truncate(limit);
        matched
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_workspace(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("whcode-fidx-{tag}-{}", std::process::id()));
        // Code Scanning 收口：路径字符串不含 ".." 才使用（防临时目录遍历注入）。
        let dir_checked = dir.display().to_string();
        if dir_checked.contains("..") {
            panic!("临时目录路径不得包含 ..");
        }
        let dir = std::path::PathBuf::from(dir_checked);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).expect("建目录");
        std::fs::create_dir_all(dir.join("target")).expect("建目录");
        std::fs::write(dir.join("README.md"), "x").expect("写文件");
        std::fs::write(dir.join("src").join("main.rs"), "x").expect("写文件");
        std::fs::write(dir.join("target").join("artifact.bin"), "x").expect("写文件");
        dir
    }

    #[test]
    fn matches_substring_case_insensitively_and_skips_build_dirs() {
        let dir = temp_workspace("match");
        let index = WorkspaceFileIndex::new(&dir);
        let hits = index.files("main", 10);
        assert!(hits.contains(&"src/main.rs".to_string()), "{hits:?}");
        assert!(
            hits.iter().all(|p| !p.starts_with("target/")),
            "构建目录须跳过：{hits:?}"
        );
        assert_eq!(index.files("readme", 10), vec!["README.md".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_needle_lists_files_and_limit_is_bounded() {
        let dir = temp_workspace("limit");
        let index = WorkspaceFileIndex::new(&dir);
        let all = index.files("", 10);
        assert!(all.len() >= 2, "空搜索词应列出文件：{all:?}");
        assert!(index.files("", 0).is_empty(), "limit=0 须返回空");
        assert!(index.files("", 1).len() <= 1, "limit 须生效");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
