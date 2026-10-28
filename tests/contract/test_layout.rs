//! 仓库目录分类契约测试（docs/guides/repository-layout.md 的可执行门禁）。
//!
//! 目标：目录分类不靠自觉，靠测试强制——任何新增顶层条目、crate 命名、测试/文档/CI
//! 子目录都必须符合 [repository-layout.md] 的五类分层（产品/验证/交付/治理/本地）。
//!
//! 规则要点：
//! 1. 顶层只允许既定目录与既定根文件；点开头条目（.cargo/.qoder/.git 等工具目录）不限制；
//! 2. `crates/` 成员必须以 `whcode-` 为前缀（snake_case），与 workspace 声明一致；
//! 3. `tests/` 子目录限于六类（unit/contract/integration/crash/stability/fixtures）且各有 README；
//! 4. `docs/` 子目录限于 adr/matrices/guides；
//! 5. `specs/` 子目录遵循 `<三位编号>-<slug>` 命名；
//! 6. 本地与产物目录（target/vendor/local/artifacts）不要求存在（机器本地、可再生成），
//!    但若存在则不得含入库形态的源码（防“顺手把代码写进临时目录”）。
//!
//! [repository-layout.md]: ../../docs/guides/repository-layout.md

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn read_dir_names(dir: &Path) -> Vec<String> {
    let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("读取 {} 失败：{e}", dir.display()));
    let mut names: Vec<String> = entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// 只取**子目录**名：`docs/`、`.github/scripts/` 等目录允许承载根级散文件（如 `fetch-duckdb.py`），
/// 分类约束只针对“类别目录”。
fn read_subdir_names(dir: &Path) -> Vec<String> {
    let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("读取 {} 失败：{e}", dir.display()));
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn assert_subset(actual: &[String], allowed: &[&str], context: &str) {
    let offenders: Vec<&String> = actual
        .iter()
        .filter(|name| !allowed.contains(&name.as_str()))
        .collect();
    assert!(
        offenders.is_empty(),
        "{context} 出现未登记条目 {offenders:?}；分类规则见 docs/guides/repository-layout.md \
         （新增类别须先更新该指南与本测试）"
    );
}

#[test]
fn top_level_entries_follow_layout() {
    let root = repo_root();
    let allowed_dirs = [
        "artifacts",
        // 可再分发的内置资源（字体与许可）；只放 OFL 等允许再分发的文件。
        "assets",
        "crates",
        "docs",
        // 外部参照与调研证据（对照/差距/参照路径/存疑项）；不承载需求承诺，ADR-045。
        "research",
        // 可移植工程准则（供其他项目复制；不依赖本项目代码）。
        "engineering-doctrine",
        "local",
        // 过渡期登记：script/ 已更名 local/（2026-10-24），双忽略 + 本地提示防并行复建；
        // 并行会话迁移完成后可删除本条与目录。
        "script",
        "specs",
        "target",
        "tests",
        "vendor",
        // 环境工具会自动重建的本地状态目录（不入库；.gitignore 已登记）。
        "state",
    ];
    let allowed_files = [
        "Cargo.lock",
        "Cargo.toml",
        // Python 开发依赖声明（本地验证工具；见文件头注释）。
        "requirements-dev.txt",
        "rust-toolchain.toml",
        "rustfmt.toml",
        // 标准开源流程文件（BSD-2-Clause；见 CONTRIBUTING.md）。
        "LICENSE",
        "README.md",
        "CONTRIBUTING.md",
        "SECURITY.md",
        // 变更记录（版本标记；见 CHANGELOG.md 的版本约定）。
        "CHANGELOG.md",
        // 代理协作约定（Claude/Codex 等 AI 代理的仓库级入口；见文件自身说明）。
        "AGENTS.md",
    ];

    for name in read_dir_names(&root) {
        if name.starts_with('.') {
            continue; // 工具目录（.git/.cargo/.qoder 等）与配置文件不在分类约束内
        }
        let path = root.join(&name);
        if path.is_dir() {
            assert!(
                allowed_dirs.contains(&name.as_str()),
                "仓库根出现未登记目录“{name}”；按 repository-layout.md 归入五类之一，\
                 或更新指南与本测试"
            );
        } else {
            assert!(
                allowed_files.contains(&name.as_str()),
                "仓库根出现未登记文件“{name}”；根文件只允许工程清单，源码/脚本/文档必须入目录"
            );
        }
    }

    // 必须存在的分类主干（入库形态，任何检出都应具备）。
    for required in ["crates", "tests", "docs", "specs"] {
        assert!(
            root.join(required).is_dir(),
            "缺少分类主干目录“{required}”（repository-layout.md §1）"
        );
    }
}

#[test]
fn crates_follow_naming_convention() {
    let crates_dir = repo_root().join("crates");
    for name in read_dir_names(&crates_dir) {
        assert!(
            name.starts_with("whcode-")
                && name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '-' || c.is_ascii_digit()),
            "crate 目录“{name}”不符合命名约定：必须为 whcode- 前缀小写 kebab-case"
        );
        let dir = crates_dir.join(&name);
        assert!(dir.join("Cargo.toml").is_file(), "{name} 缺少 Cargo.toml");
        assert!(dir.join("src").is_dir(), "{name} 缺少 src/");
    }
}

#[test]
fn tests_dir_uses_six_categories() {
    let tests_dir = repo_root().join("tests");
    assert_subset(
        &read_dir_names(&tests_dir),
        &[
            "unit",
            "contract",
            "integration",
            "crash",
            "stability",
            "fixtures",
            "src",
            "Cargo.toml",
        ],
        "tests/",
    );
    // 六类测试目录各有 README 说明用途（repository-layout.md §1）。
    for category in [
        "unit",
        "contract",
        "integration",
        "crash",
        "stability",
        "fixtures",
    ] {
        let readme = tests_dir.join(category).join("README.md");
        assert!(
            readme.is_file(),
            "tests/{category} 缺少 README.md（用途说明）"
        );
    }
}

#[test]
fn docs_dir_uses_three_kinds() {
    assert_subset(
        &read_subdir_names(&repo_root().join("docs")),
        &["adr", "matrices", "guides"],
        "docs/",
    );
}

#[test]
fn specs_follow_change_id_naming() {
    for name in read_dir_names(&repo_root().join("specs")) {
        let chars: Vec<char> = name.chars().collect();
        let well_formed = chars.len() > 4
            && chars[..3].iter().all(|c| c.is_ascii_digit())
            && chars[3] == '-'
            && chars[4..]
                .iter()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-');
        assert!(
            well_formed,
            "specs/ 子目录“{name}”不符合 <三位编号>-<slug> 命名（如 001-whcode-harness）"
        );
    }
}

#[test]
fn local_dirs_do_not_host_commitable_sources() {
    // 本地/产物目录允许不存在；存在时不得混入源码形态文件（分类纪律的防漂移检查）。
    for local in ["target", "vendor", "local", "artifacts"] {
        let dir = repo_root().join(local);
        if !dir.is_dir() {
            continue;
        }
        for name in read_dir_names(&dir) {
            let offender = name.ends_with(".rs")
                || name.ends_with(".toml") && name != "Cargo.toml"
                || name.ends_with(".py") && local != "local";
            assert!(
                !offender,
                "{local}/{name} 不应存在：本地/产物目录不得承载源码与工程清单 \
                 （repository-layout.md §1 入库规则）"
            );
        }
    }
}
