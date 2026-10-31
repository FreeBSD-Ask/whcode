//! # 技能扫描器（FR-735、ADR-053）
//!
//! 扫描 `.zcode/skills/<name>/SKILL.md`，解析 frontmatter（name + description），
//! 返回结构化列表。坏文件跳过并记 warning（不 panic、不静默）。
//!
//! 治理（BH 骨，ADR-053 §2.2）：SKILL.md 引用的相对路径文件必须存在——可达性
//! 门禁由 [`validate_references`] 强制。

use std::path::Path;
use whcode_domain::WhError;

/// 已解析的技能描述（ADR-053 §2.3）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillEntry {
    /// 技能名（frontmatter `name` 或目录名回退）。
    pub name: String,
    /// 描述（frontmatter `description`，截 200 字符）。
    pub description: String,
    /// SKILL.md 绝对路径。
    pub path: std::path::PathBuf,
}

/// 扫描工作区的 `.zcode/skills/` 目录（FR-735）。
///
/// 不递归（与 ZCode 一致）；每个子目录下有 `SKILL.md` 即收录。
/// 坏文件（无 frontmatter 或解析失败）跳过并返回 warnings。
///
/// # Errors
///
/// 工作区不存在时返回 [`WhError`]（目录缺失返回空列表，不是错误）。
pub fn scan_skills(workspace: &Path) -> Result<(Vec<SkillEntry>, Vec<String>), WhError> {
    let mut entries = Vec::new();
    let mut warnings = Vec::new();
    let skills_dir = workspace.join(".zcode").join("skills");
    let Ok(read_dir) = std::fs::read_dir(&skills_dir) else {
        return Ok((entries, warnings)); // 无技能目录 = 空列表，不是错误
    };
    let mut dirs: Vec<_> = read_dir
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.path())
        .collect();
    dirs.sort();
    for dir in dirs {
        let skill_path = dir.join("SKILL.md");
        let Ok(content) = std::fs::read_to_string(&skill_path) else {
            warnings.push(format!("跳过（无 SKILL.md）：{}", dir.display()));
            continue;
        };
        let (name, description) = match parse_frontmatter(&content) {
            Ok(pair) => pair,
            Err(err) => {
                warnings.push(format!("跳过（{}）：{}", skill_path.display(), err));
                continue;
            }
        };
        entries.push(SkillEntry {
            name,
            description,
            path: skill_path,
        });
    }
    Ok((entries, warnings))
}

/// 解析 SKILL.md frontmatter：`---\nname: X\ndescription: Y\n---`。
/// 坏 frontmatter（无闭合或无 name）返回 [`WhError`]。
fn parse_frontmatter(content: &str) -> Result<(String, String), WhError> {
    let err = |msg: &str| WhError::invalid_state(format!("SKILL.md frontmatter 解析失败：{msg}"));
    let trimmed = content.trim_start();
    let rest = trimmed
        .strip_prefix("---")
        .ok_or_else(|| err("缺少 --- 起始"))?;
    let end = rest.find("\n---").ok_or_else(|| err("缺少 --- 闭合"))?;
    let frontmatter = &rest[..end];
    let mut name = String::new();
    let mut description = String::new();
    for line in frontmatter.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("name:") {
            name = value.trim().to_string();
        } else if let Some(value) = line.strip_prefix("description:") {
            description = value.trim().to_string();
        }
    }
    if name.is_empty() {
        return Err(err("frontmatter 缺少 name 字段"));
    }
    if description.chars().count() > 200 {
        description = description.chars().take(200).collect();
    }
    Ok((name, description))
}

/// 可达性校验（FR-735、ADR-053 §2.2）：SKILL.md 中 `](relative/path)` 引用的
/// 相对路径文件必须存在于同一技能目录下；缺失返回错误列表（空 = 全通过）。
pub fn validate_references(skill_dir: &Path, content: &str) -> Vec<String> {
    let mut missing = Vec::new();
    for line in content.lines() {
        // 匹配 markdown 链接 ](path) 形态
        let mut search = line;
        while let Some(pos) = search.find("](") {
            let rest = &search[pos + 2..];
            let Some(close) = rest.find(')') else { break };
            let path = &rest[..close];
            if path.starts_with("http")
                || path.starts_with('#')
                || path.contains("://")
                || path.is_empty()
            {
                search = &rest[close + 1..];
                continue;
            }
            let clean = path.trim_start_matches("./");
            if !clean.is_empty() && !clean.contains("..") {
                let full = skill_dir.join(clean);
                if !full.exists() {
                    missing.push(format!("引用不存在：{}", full.display()));
                }
            }
            search = &rest[close + 1..];
        }
    }
    missing
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> std::path::PathBuf {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("whcode-skills-test-{}-{seq}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let skill_dir = dir.join(".zcode/skills/read-guide");
        std::fs::create_dir_all(&skill_dir).expect("建技能目录");
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: read-guide\ndescription: 读取文件的指南\n---\n参见 [detail](detail.md)\n",
        )
        .expect("写 SKILL.md");
        std::fs::write(skill_dir.join("detail.md"), "详细内容\n").expect("写引用文件");
        dir
    }

    #[test]
    fn scan_finds_skills_with_frontmatter() {
        let ws = workspace();
        let (entries, warnings) = scan_skills(&ws).expect("扫描");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "read-guide");
        assert_eq!(entries[0].description, "读取文件的指南");
        assert!(warnings.is_empty(), "{warnings:?}");
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn empty_skills_dir_returns_empty() {
        let dir = std::env::temp_dir().join(format!("whcode-skills-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建空目录");
        let (entries, warnings) = scan_skills(&dir).expect("无技能目录 = 空列表");
        assert!(entries.is_empty());
        assert!(warnings.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bad_frontmatter_is_skipped_with_warning() {
        let ws = workspace();
        let bad_dir = ws.join(".zcode/skills/broken");
        std::fs::create_dir_all(&bad_dir).expect("建坏目录");
        std::fs::write(bad_dir.join("SKILL.md"), "无 frontmatter").expect("写坏文件");
        let (entries, warnings) = scan_skills(&ws).expect("扫描");
        assert_eq!(entries.len(), 1, "好技能仍收录");
        assert_eq!(warnings.len(), 1, "坏文件记 warning");
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn reference_validation_catches_missing_files() {
        let ws = workspace();
        let skill_dir = ws.join(".zcode/skills/read-guide");
        let content = std::fs::read_to_string(skill_dir.join("SKILL.md")).expect("读");
        let missing = validate_references(&skill_dir, &content);
        assert!(missing.is_empty(), "已存在的引用不应报缺失");
        let with_missing = "参见 [nope](ghost.md)";
        let missing = validate_references(&skill_dir, with_missing);
        assert_eq!(missing.len(), 1);
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn description_truncated_at_200_chars() {
        let long = "长".repeat(300);
        let (name, description) =
            parse_frontmatter(&format!("---\nname: test\ndescription: {long}\n---\n正文"))
                .expect("解析");
        assert_eq!(name, "test");
        assert_eq!(description.chars().count(), 200, "描述截 200 字符");
    }
}
