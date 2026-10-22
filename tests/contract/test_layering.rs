//! 分层与平台隔离契约测试（tasks.md T019、T014；ADR-003、ADR-005、NFR-101）。
//!
//! 这些断言把架构基线直接编码为可执行检查，避免"约定"随实现漂移：
//!
//! 1. 前端（TUI/CUI/Web/SSH）**不得**依赖 `whcode-store`——前端只能经应用服务访问状态
//!    （ADR-003；SSH 同样不得自行打开数据库，ADR-005）；
//! 2. `whcode-domain` 不得依赖任何其他内部 crate（领域类型是依赖图的叶子之一，§6.2）；
//! 3. 前端之间不得互相依赖（避免 Web 复用 TUI、或以端到端耦合绕过能力矩阵）；
//! 4. 平台条件编译（`cfg(target_os …)`）不得出现在 `whcode-platform` 之外（NFR-101）。

use std::fs;
use std::path::{Path, PathBuf};

/// 前端适配器 crate。
const FRONTENDS: [&str; 4] = ["whcode-tui", "whcode-cui", "whcode-web", "whcode-ssh"];

/// workspace 全部成员 crate（tasks.md T001）。
const ALL_CRATES: [&str; 13] = [
    "whcode-domain",
    "whcode-app",
    "whcode-llm",
    "whcode-store",
    "whcode-speckit",
    "whcode-tools",
    "whcode-runtime",
    "whcode-tui",
    "whcode-cui",
    "whcode-web",
    "whcode-ssh",
    "whcode-platform",
    "whcode-bin",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn crate_dir(name: &str) -> PathBuf {
    repo_root().join("crates").join(name)
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("读取 {} 失败：{e}", path.display()))
}

fn manifest(name: &str) -> String {
    read(&crate_dir(name).join("Cargo.toml"))
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("读取 {} 失败：{e}", dir.display()));
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// 平台分支的判定模式（NFR-101）。
///
/// 覆盖易漏写法：`cfg(all(target_os …))`、`cfg_attr`、`target_family`、裸 `cfg(windows)`/
/// `cfg(unix)`、**宏形态 `cfg!(windows)`/`cfg!(unix)`**（宏调用不是 `cfg(...)` 属性，
/// 早期 marker 表漏检，实测已在 `whcode-runtime` 的测试代码中命中），以及构建脚本常用的
/// `CARGO_CFG_*` 环境判定。审查结论：只匹配三种字面量会让门禁形同虚设（例如构建脚本用
/// `CARGO_CFG_WINDOWS` 直接判平台而不被检出）。
const PLATFORM_BRANCH_MARKERS: [&str; 12] = [
    "cfg(target_os",
    "cfg!(target_os",
    "cfg(any(target_os",
    "cfg(all(target_os",
    "cfg_attr(",
    "target_family",
    "cfg(windows)",
    "cfg(unix)",
    "cfg!(windows",
    "cfg!(unix",
    "CARGO_CFG_WINDOWS",
    "CARGO_CFG_TARGET_OS",
];

/// 收集某 crate 的全部 Rust 源，**包含** crate 根下的 `build.rs`（构建脚本同样是代码）。
fn crate_rust_sources(name: &str) -> Vec<PathBuf> {
    let mut sources = Vec::new();
    let src_dir = crate_dir(name).join("src");
    if src_dir.is_dir() {
        rust_sources(&src_dir, &mut sources);
    }
    let build_script = crate_dir(name).join("build.rs");
    if build_script.is_file() {
        sources.push(build_script);
    }
    sources
}

#[test]
fn every_workspace_member_crate_exists() {
    for name in ALL_CRATES {
        let cargo_toml = crate_dir(name).join("Cargo.toml");
        assert!(
            cargo_toml.is_file(),
            "{name} 缺失 Cargo.toml：{}",
            cargo_toml.display()
        );
        assert!(
            crate_dir(name).join("src").is_dir(),
            "{name} 缺失 src/ 目录"
        );
    }
}

#[test]
fn frontends_do_not_depend_on_store() {
    for front in FRONTENDS {
        let text = manifest(front);
        assert!(
            !text.contains("whcode-store"),
            "{front} 不得依赖 whcode-store：前端只经应用服务访问状态（ADR-003/005）"
        );
    }
}

#[test]
fn domain_has_no_internal_dependencies() {
    let text = manifest("whcode-domain");
    // 排除包自身的 `name = "whcode-domain"` 行后，不得出现任何内部 crate 引用。
    let without_own_name: String = text
        .lines()
        .filter(|line| !line.trim_start().starts_with("name ="))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !without_own_name.contains("whcode-"),
        "whcode-domain 不得依赖任何内部 crate（§6.2 依赖原则）"
    );
}

#[test]
fn frontends_do_not_depend_on_each_other() {
    for front in FRONTENDS {
        let text = manifest(front);
        for other in FRONTENDS {
            if front == other {
                continue;
            }
            assert!(
                !text.contains(other),
                "{front} 不得依赖 {other}：三端对等必须基于应用服务能力，而非端间复用"
            );
        }
    }
}

/// `[lints]` 段是否继承 workspace 配置（NFR-215：`unsafe_code = "forbid"` 靠继承生效）。
fn inherits_workspace_lints(text: &str) -> bool {
    let mut in_lints = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_lints = trimmed == "[lints]";
            continue;
        }
        if in_lints && trimmed == "workspace = true" {
            return true;
        }
    }
    false
}

#[test]
fn crates_forbid_unsafe_code_via_workspace_lints() {
    let workspace = read(&repo_root().join("Cargo.toml"));
    assert!(
        workspace.contains("unsafe_code = \"forbid\""),
        "workspace 必须在 [workspace.lints.rust] 中设置 unsafe_code = \"forbid\"（NFR-215）"
    );

    let mut offenders: Vec<&str> = Vec::new();
    for name in ALL_CRATES {
        if !inherits_workspace_lints(&manifest(name)) {
            offenders.push(name);
        }
    }
    let tests_manifest = read(&repo_root().join("tests").join("Cargo.toml"));
    if !inherits_workspace_lints(&tests_manifest) {
        offenders.push("tests");
    }
    assert!(
        offenders.is_empty(),
        "以下成员未继承 workspace lints，unsafe 禁令不会生效：{offenders:?}（NFR-215）"
    );
}

#[test]
fn platform_cfg_is_confined_to_platform_crate() {
    let mut offenders: Vec<String> = Vec::new();
    for name in ALL_CRATES {
        if name == "whcode-platform" {
            continue;
        }
        for source in crate_rust_sources(name) {
            let text = read(&source);
            let hits: Vec<&str> = PLATFORM_BRANCH_MARKERS
                .iter()
                .copied()
                .filter(|marker| text.contains(marker))
                .collect();
            if !hits.is_empty() {
                offenders.push(format!("{}（命中 {hits:?}）", source.display()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "平台条件编译与平台判定必须集中在 whcode-platform（NFR-101），违规文件：{offenders:?}"
    );
}
