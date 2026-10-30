//! # 内建工具六件套（FR-726、ADR-048 §2.2）
//!
//! `read_file` / `write_file` / `edit_file` / `list_dir` / `glob` / `grep`——纯 `std::fs`
//! 实现（无 cfg、无新依赖），全部有界：读 ≤ [`MAX_READ_BYTES`]、遍历目录数与结果条目
//! 有上限、输出按 [`crate::MAX_TOOL_OUTPUT_BYTES`] 截断并显式标记（R-05 对策）。
//!
//! grep v1 为**字面子串**匹配（单正则需引新依赖，按 ADR-048 留待真实需求立项——
//! 覆盖阶梯：先字面，正则未立项前不实现）。
//!
//! 写工具产出**diff 摘要**（行数增减 + 最多 5 行样例变更），不回传全文（FR-726）。

use std::fs;
use std::io::Read;
use std::path::Path;

use serde_json::Value;
use whcode_domain::WhError;

use crate::MAX_TOOL_OUTPUT_BYTES;
use crate::paths::resolve_in_workspace;
use crate::registry::{
    ParamType, SideEffectScope, Tool, ToolDescriptor, ToolOutput, ToolParameter, validate_arguments,
};

/// 单次读取上限（1 MiB；超限截断并标记，FR-726）。
pub const MAX_READ_BYTES: u64 = 1024 * 1024;
/// 目录遍历的条目与目录数上限（有界性，NFR-215）。
pub const MAX_WALK_ENTRIES: usize = 500;
pub const MAX_WALK_DIRS: usize = 200;
/// glob/grep 结果条目上限。
pub const MAX_MATCH_ENTRIES: usize = 200;
/// diff 摘要样例行数上限。
const DIFF_SAMPLE_LINES: usize = 5;
/// 行级 diff 计算的行数上限（超出则退化为字节级摘要，防止 O(n²) 无界）。
const DIFF_MAX_LINES: usize = 2000;

fn invalid(message: String) -> WhError {
    WhError::invalid_state(message)
}

fn bound_output(content: String) -> ToolOutput {
    let byte_len = content.len();
    if byte_len <= MAX_TOOL_OUTPUT_BYTES {
        return ToolOutput {
            content,
            truncated: false,
        };
    }
    let mut cut = MAX_TOOL_OUTPUT_BYTES;
    while cut > 0 && !content.is_char_boundary(cut) {
        cut -= 1;
    }
    ToolOutput {
        content: format!(
            "{}\n[truncated: 原 {} 字节超过 {} 字节上限，R-05 对策显式截断]",
            &content[..cut],
            byte_len,
            MAX_TOOL_OUTPUT_BYTES
        ),
        truncated: true,
    }
}

fn read_bounded(path: &Path) -> Result<(String, bool), WhError> {
    let metadata = fs::metadata(path).map_err(|err| invalid(format!("读取元数据失败：{err}")))?;
    let truncated_meta = metadata.len() > MAX_READ_BYTES;
    let file = fs::File::open(path).map_err(|err| invalid(format!("打开文件失败：{err}")))?;
    let mut bytes = Vec::new();
    file.take(MAX_READ_BYTES)
        .read_to_end(&mut bytes)
        .map_err(|err| invalid(format!("读取失败：{err}")))?;
    let mut content = String::from_utf8_lossy(&bytes).into_owned();
    let truncated = truncated_meta;
    if truncated {
        content.push_str("\n[truncated: 文件超过 1 MiB 读取上限]");
    }
    Ok((content, truncated))
}

fn string_argument<'a>(arguments: &'a Value, name: &str) -> Result<&'a str, WhError> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(format!("参数 {name} 必须为字符串")))
}

fn optional_integer(arguments: &Value, name: &str) -> Result<Option<usize>, WhError> {
    match arguments.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(|n| Some(n as usize))
            .ok_or_else(|| invalid(format!("参数 {name} 必须为非负整数"))),
    }
}

/// 相对路径统一以 `/` 分隔（Windows `to_string_lossy` 会产生 `\`，模式匹配统一口径）。
fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// 行级 diff 摘要（旧/新行数 + 最多 5 行样例变更；超行数上限退化为字节级摘要）。
fn diff_summary(old_text: &str, new_text: &str) -> String {
    let old_lines: Vec<&str> = old_text.lines().collect();
    let new_lines: Vec<&str> = new_text.lines().collect();
    if old_lines.len() > DIFF_MAX_LINES || new_lines.len() > DIFF_MAX_LINES {
        return format!(
            "（文件过大，行级摘要省略：{} → {} 字节）",
            old_text.len(),
            new_text.len()
        );
    }
    let removed = old_lines
        .iter()
        .filter(|line| !new_lines.contains(line))
        .count();
    let added = new_lines
        .iter()
        .filter(|line| !old_lines.contains(line))
        .count();
    let mut samples: Vec<String> = Vec::new();
    for line in new_lines.iter().filter(|line| !old_lines.contains(line)) {
        if samples.len() == DIFF_SAMPLE_LINES {
            break;
        }
        samples.push(format!("+{line}"));
    }
    for line in old_lines.iter().filter(|line| !new_lines.contains(line)) {
        if samples.len() == DIFF_SAMPLE_LINES {
            break;
        }
        samples.push(format!("-{line}"));
    }
    format!("+{added}/-{removed} 行\n{}", samples.join("\n"))
}

// ---------- read_file ----------

/// 读取工作区内文本文件（≤1 MiB，超限截断标记；FR-726）。
pub struct ReadFile;

impl Tool for ReadFile {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "read_file",
            description: "读取工作区内文本文件内容（≤1 MiB，超限截断标记）",
            parameters: vec![ToolParameter {
                name: "path",
                kind: ParamType::String,
                required: true,
                description: "工作区内相对路径",
            }],
            side_effect_scope: SideEffectScope::ReadOnly,
        }
    }

    fn execute(&self, workspace: &Path, arguments: &Value) -> Result<ToolOutput, WhError> {
        let descriptor = Tool::descriptor(self);
        validate_arguments(&descriptor, arguments)?;
        let raw = string_argument(arguments, "path")?;
        let path = resolve_in_workspace(workspace, raw)?;
        let (content, _) = read_bounded(&path)?;
        Ok(bound_output(content))
    }
}

// ---------- write_file ----------

/// 写工作区内文件（创建或覆盖；输出 diff 摘要，不回传全文；FR-726）。
pub struct WriteFile;

impl Tool for WriteFile {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "write_file",
            description: "创建或覆盖工作区内文本文件；结果为 diff 摘要而非全文",
            parameters: vec![
                ToolParameter {
                    name: "path",
                    kind: ParamType::String,
                    required: true,
                    description: "工作区内相对路径",
                },
                ToolParameter {
                    name: "content",
                    kind: ParamType::String,
                    required: true,
                    description: "完整文件内容",
                },
            ],
            side_effect_scope: SideEffectScope::WorkspaceWrite,
        }
    }

    fn execute(&self, workspace: &Path, arguments: &Value) -> Result<ToolOutput, WhError> {
        let descriptor = Tool::descriptor(self);
        validate_arguments(&descriptor, arguments)?;
        let raw = string_argument(arguments, "path")?;
        let content = string_argument(arguments, "content")?;
        let path = resolve_in_workspace(workspace, raw)?;
        let existed = path.exists();
        let old_text = if existed {
            read_bounded(&path)?.0
        } else {
            String::new()
        };
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).map_err(|err| invalid(format!("创建目录失败：{err}")))?;
        }
        fs::write(&path, content).map_err(|err| invalid(format!("写入失败：{err}")))?;
        let action = if existed { "updated" } else { "created" };
        Ok(bound_output(format!(
            "{action} {raw}（{} 字节）\n{}",
            content.len(),
            diff_summary(&old_text, content)
        )))
    }
}

// ---------- edit_file ----------

/// 精确串替换（唯一性校验：出现次数不等于预期即拒绝；FR-726）。
pub struct EditFile;

impl Tool for EditFile {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "edit_file",
            description: "按精确子串替换编辑文件；出现次数与预期不符即拒绝（不做模糊匹配）",
            parameters: vec![
                ToolParameter {
                    name: "path",
                    kind: ParamType::String,
                    required: true,
                    description: "工作区内相对路径",
                },
                ToolParameter {
                    name: "old_string",
                    kind: ParamType::String,
                    required: true,
                    description: "待替换的精确原文",
                },
                ToolParameter {
                    name: "new_string",
                    kind: ParamType::String,
                    required: true,
                    description: "替换后的文本",
                },
                ToolParameter {
                    name: "expected_count",
                    kind: ParamType::Integer,
                    required: false,
                    description: "预期出现次数（默认 1）",
                },
            ],
            side_effect_scope: SideEffectScope::WorkspaceWrite,
        }
    }

    fn execute(&self, workspace: &Path, arguments: &Value) -> Result<ToolOutput, WhError> {
        let descriptor = Tool::descriptor(self);
        validate_arguments(&descriptor, arguments)?;
        let raw = string_argument(arguments, "path")?;
        let old_string = string_argument(arguments, "old_string")?;
        let new_string = string_argument(arguments, "new_string")?;
        let expected = optional_integer(arguments, "expected_count")?.unwrap_or(1);
        if expected == 0 {
            return Err(invalid("expected_count 须为正整数".to_string()));
        }
        let path = resolve_in_workspace(workspace, raw)?;
        let (old_text, _) = read_bounded(&path)?;
        let occurrences = old_text.matches(old_string).count();
        if occurrences != expected {
            return Err(invalid(format!(
                "old_string 出现 {occurrences} 次，预期 {expected}（唯一性校验失败，拒绝模糊替换）"
            )));
        }
        let new_text = old_text.replacen(old_string, new_string, expected);
        fs::write(&path, &new_text).map_err(|err| invalid(format!("写入失败：{err}")))?;
        Ok(bound_output(format!(
            "edited {raw}（替换 {expected} 处）\n{}",
            diff_summary(&old_text, &new_text)
        )))
    }
}

// ---------- list_dir ----------

/// 列出工作区内目录条目（有界 500，按名排序；FR-726）。
pub struct ListDir;

impl Tool for ListDir {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "list_dir",
            description: "列出工作区内目录条目（名称 + file/dir，上限 500）",
            parameters: vec![ToolParameter {
                name: "path",
                kind: ParamType::String,
                required: false,
                description: "工作区内相对目录（默认根）",
            }],
            side_effect_scope: SideEffectScope::ReadOnly,
        }
    }

    fn execute(&self, workspace: &Path, arguments: &Value) -> Result<ToolOutput, WhError> {
        let descriptor = Tool::descriptor(self);
        validate_arguments(&descriptor, arguments)?;
        let raw = arguments.get("path").and_then(Value::as_str).unwrap_or(".");
        let path = resolve_in_workspace(workspace, raw)?;
        let mut names: Vec<String> = Vec::new();
        for entry in fs::read_dir(&path).map_err(|err| invalid(format!("读目录失败：{err}")))?
        {
            if names.len() >= MAX_WALK_ENTRIES {
                break;
            }
            let entry = entry.map_err(|err| invalid(format!("遍历失败：{err}")))?;
            let kind = if entry.path().is_dir() { "dir" } else { "file" };
            names.push(format!("{}\t{}", entry.file_name().to_string_lossy(), kind));
        }
        names.sort();
        let truncated = names.len() >= MAX_WALK_ENTRIES;
        let mut content = names.join("\n");
        if truncated {
            content.push_str("\n[truncated: 条目数达到上限]");
        }
        Ok(bound_output(content))
    }
}

// ---------- glob ----------

/// 有限 glob（`*`/`?`/`**`），有界遍历与结果上限；不引依赖（FR-726）。
pub struct Glob;

impl Tool for Glob {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "glob",
            description: "按 glob 模式（* ? **）列出工作区内文件（相对路径，上限 200）",
            parameters: vec![ToolParameter {
                name: "pattern",
                kind: ParamType::String,
                required: true,
                description: "相对工作区根的模式，如 src/**/*.rs",
            }],
            side_effect_scope: SideEffectScope::ReadOnly,
        }
    }

    fn execute(&self, workspace: &Path, arguments: &Value) -> Result<ToolOutput, WhError> {
        let descriptor = Tool::descriptor(self);
        validate_arguments(&descriptor, arguments)?;
        let pattern = string_argument(arguments, "pattern")?;
        if pattern.contains("..") {
            return Err(invalid("glob 模式不得包含 ..（工作区绑定）".to_string()));
        }
        let mut matches: Vec<String> = Vec::new();
        let mut visited_dirs = 0usize;
        walk(
            workspace,
            workspace,
            pattern,
            &mut matches,
            &mut visited_dirs,
        );
        matches.sort();
        let truncated = matches.len() >= MAX_MATCH_ENTRIES;
        let mut content = matches.join("\n");
        if truncated {
            content.push_str("\n[truncated: 结果数达到上限]");
        }
        Ok(bound_output(content))
    }
}

/// 有界递归遍历：目录数 [`MAX_WALK_DIRS`]、结果数 [`MAX_MATCH_ENTRIES`] 双上限。
fn walk(
    root: &Path,
    dir: &Path,
    pattern: &str,
    matches: &mut Vec<String>,
    visited_dirs: &mut usize,
) {
    if matches.len() >= MAX_MATCH_ENTRIES || *visited_dirs >= MAX_WALK_DIRS {
        return;
    }
    *visited_dirs += 1;
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return, // 不可读目录跳过（有界遍历不中断）
    };
    for entry in entries {
        if matches.len() >= MAX_MATCH_ENTRIES {
            return;
        }
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path.is_dir() {
            walk(root, &path, pattern, matches, visited_dirs);
        } else {
            let relative = relative_display(root, &path);
            if glob_match(pattern, &relative) {
                matches.push(relative);
            }
        }
    }
}

/// 极简 glob：`**/` 跨任意层目录，段内 `*`/`?`；其余按 `/` 分段字面比对。
fn glob_match(pattern: &str, candidate: &str) -> bool {
    if let Some(suffix) = pattern.strip_prefix("**/") {
        let mut rest = candidate;
        loop {
            if glob_match(suffix, rest) {
                return true;
            }
            match rest.split_once('/') {
                Some((_, tail)) => rest = tail,
                None => return false,
            }
        }
    }
    let pattern_segments: Vec<&str> = pattern.split('/').collect();
    let candidate_segments: Vec<&str> = candidate.split('/').collect();
    pattern_segments.len() == candidate_segments.len()
        && pattern_segments
            .iter()
            .zip(candidate_segments.iter())
            .all(|(p, t)| segment_match(p, t))
}

/// 单段通配匹配（`*` 任意、`?` 单字符；跨 `/` 不匹配）。
fn segment_match(pattern: &str, text: &str) -> bool {
    if !pattern.contains('*') && !pattern.contains('?') {
        return pattern == text;
    }
    let pattern_chars: Vec<char> = pattern.chars().collect();
    let text_chars: Vec<char> = text.chars().collect();
    segment_match_inner(&pattern_chars, &text_chars)
}

fn segment_match_inner(pattern: &[char], text: &[char]) -> bool {
    match (pattern.first(), text.first()) {
        (None, None) => true,
        (None, Some(_)) => false,
        (Some('*'), _) => {
            (0..=text.len()).any(|skip| segment_match_inner(&pattern[1..], &text[skip..]))
        }
        (Some('?'), Some(_)) => segment_match_inner(&pattern[1..], &text[1..]),
        (Some(p), Some(t)) if p == t => segment_match_inner(&pattern[1..], &text[1..]),
        _ => false,
    }
}

// ---------- grep ----------

/// 逐行字面子串搜索（结果有界；正则匹配待真实需求立项，见模块注释；FR-726）。
pub struct Grep;

impl Tool for Grep {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "grep",
            description: "工作区内逐行字面子串搜索（path:line:text，上限 200 行；正则待立项）",
            parameters: vec![
                ToolParameter {
                    name: "pattern",
                    kind: ParamType::String,
                    required: true,
                    description: "字面搜索串",
                },
                ToolParameter {
                    name: "glob",
                    kind: ParamType::String,
                    required: false,
                    description: "可选：文件名过滤模式（如 *.rs）",
                },
            ],
            side_effect_scope: SideEffectScope::ReadOnly,
        }
    }

    fn execute(&self, workspace: &Path, arguments: &Value) -> Result<ToolOutput, WhError> {
        let descriptor = Tool::descriptor(self);
        validate_arguments(&descriptor, arguments)?;
        let needle = string_argument(arguments, "pattern")?;
        let filter = arguments.get("glob").and_then(Value::as_str);
        let mut lines: Vec<String> = Vec::new();
        let mut visited_dirs = 0usize;
        grep_walk(
            workspace,
            workspace,
            needle,
            filter,
            &mut lines,
            &mut visited_dirs,
        );
        let truncated = lines.len() >= MAX_MATCH_ENTRIES;
        let mut content = lines.join("\n");
        if truncated {
            content.push_str("\n[truncated: 结果数达到上限]");
        }
        Ok(bound_output(content))
    }
}

fn grep_walk(
    root: &Path,
    dir: &Path,
    needle: &str,
    filter: Option<&str>,
    lines: &mut Vec<String>,
    visited_dirs: &mut usize,
) {
    if lines.len() >= MAX_MATCH_ENTRIES || *visited_dirs >= MAX_WALK_DIRS {
        return;
    }
    *visited_dirs += 1;
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries {
        if lines.len() >= MAX_MATCH_ENTRIES {
            return;
        }
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path.is_dir() {
            grep_walk(root, &path, needle, filter, lines, visited_dirs);
            continue;
        }
        if let Some(pattern) = filter
            && !segment_match(pattern, &entry.file_name().to_string_lossy())
        {
            continue;
        }
        grep_file(root, &path, needle, lines);
    }
}

fn grep_file(root: &Path, path: &Path, needle: &str, lines: &mut Vec<String>) {
    let Ok((content, _)) = read_bounded(path) else {
        return;
    };
    let relative = relative_display(root, path);
    for (index, line) in content.lines().enumerate() {
        if lines.len() >= MAX_MATCH_ENTRIES {
            return;
        }
        if line.contains(needle) {
            lines.push(format!("{relative}:{}:{}", index + 1, line.trim_end()));
        }
    }
}

/// 默认注册表：六件套全部注册（FR-725；供 whcode-app 装配）。
///
/// # Errors
///
/// 注册冲突（不应发生：稳定名互异）返回错误。
pub fn default_registry() -> Result<crate::registry::ToolRegistry, WhError> {
    let mut registry = crate::registry::ToolRegistry::new();
    registry.register(std::sync::Arc::new(ReadFile))?;
    registry.register(std::sync::Arc::new(WriteFile))?;
    registry.register(std::sync::Arc::new(EditFile))?;
    registry.register(std::sync::Arc::new(ListDir))?;
    registry.register(std::sync::Arc::new(Glob))?;
    registry.register(std::sync::Arc::new(Grep))?;
    Ok(registry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use whcode_domain::ErrorCode;

    fn workspace() -> PathBuf {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("whcode-tools-test-{}-{seq}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).expect("建夹具目录");
        fs::write(dir.join("src/lib.rs"), "fn main() {}\nfn helper() {}\n").expect("写夹具");
        fs::write(dir.join("README.md"), "# WHCode tools test\n").expect("写夹具");
        dir
    }

    use std::path::PathBuf;

    #[test]
    fn read_file_returns_content_and_rejects_escape() {
        let ws = workspace();
        let output = ReadFile
            .execute(&ws, &json_args(r#"{"path": "src/lib.rs"}"#))
            .expect("读取");
        assert!(output.content.contains("fn helper()"));
        assert!(!output.truncated);
        let err = ReadFile
            .execute(&ws, &json_args(r#"{"path": "../escape"}"#))
            .expect_err("逃逸必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        let err = ReadFile
            .execute(&ws, &json_args(r#"{"path": "C:/Windows/hosts"}"#))
            .expect_err("绝对路径必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn write_then_edit_produces_diff_summary_and_enforces_uniqueness() {
        let ws = workspace();
        let output = WriteFile
            .execute(
                &ws,
                &json_args(r#"{"path": "src/new.rs", "content": "a\nb\nc\n"}"#),
            )
            .expect("创建");
        assert!(output.content.contains("created"), "{}", output.content);
        let output = WriteFile
            .execute(
                &ws,
                &json_args(r#"{"path": "src/new.rs", "content": "a\nx\nc\n"}"#),
            )
            .expect("覆盖");
        assert!(output.content.contains("+1/-1 行"), "{}", output.content);
        let err = EditFile
            .execute(
                &ws,
                &json_args(
                    r#"{"path": "src/new.rs", "old_string": "c\n", "new_string": "d\n", "expected_count": 2}"#,
                ),
            )
            .expect_err("出现次数不符必须拒绝");
        assert!(err.message.contains("唯一性校验失败"), "{err}");
        let output = EditFile
            .execute(
                &ws,
                &json_args(r#"{"path": "src/new.rs", "old_string": "x", "new_string": "y"}"#),
            )
            .expect("唯一替换");
        assert!(output.content.contains("替换 1 处"));
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn list_glob_grep_are_bounded_and_ordered() {
        let ws = workspace();
        let listing = ListDir.execute(&ws, &json_args(r#"{}"#)).expect("列目录");
        assert!(listing.content.contains("src\tdir"), "{}", listing.content);
        assert!(listing.content.contains("README.md\tfile"));

        Glob.execute(&ws, &json_args(r#"{"pattern": "../x"}"#))
            .expect_err(".. 拒绝");
        let found = Glob
            .execute(&ws, &json_args(r#"{"pattern": "**/*.rs"}"#))
            .expect("glob");
        assert!(found.content.contains("src/lib.rs"), "{}", found.content);

        let hits = Grep
            .execute(
                &ws,
                &json_args(r#"{"pattern": "fn helper", "glob": "*.rs"}"#),
            )
            .expect("grep");
        assert!(hits.content.contains("src/lib.rs:2:"), "{}", hits.content);
        let misses = Grep
            .execute(&ws, &json_args(r#"{"pattern": "不存在的串"}"#))
            .expect("无命中");
        assert!(misses.content.is_empty());
        let _ = fs::remove_dir_all(&ws);
    }

    #[test]
    fn default_registry_registers_six_tools_without_conflict() {
        let registry = default_registry().expect("默认注册表");
        assert_eq!(registry.len(), 6);
        for name in [
            "read_file",
            "write_file",
            "edit_file",
            "list_dir",
            "glob",
            "grep",
        ] {
            assert!(registry.get(name).is_some(), "{name} 应已注册");
        }
        assert_eq!(registry.provider_manifest().len(), 6);
    }

    #[test]
    fn oversized_output_is_truncated_with_marker() {
        let output = bound_output("x".repeat(MAX_TOOL_OUTPUT_BYTES + 10));
        assert!(output.truncated);
        assert!(output.content.contains("[truncated:"));
    }

    fn json_args(raw: &str) -> Value {
        serde_json::from_str(raw).expect("夹具 JSON")
    }
}
