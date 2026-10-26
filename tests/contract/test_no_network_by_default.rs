//! 默认不联网契约测试（NFR-213、ADR-031 决策 4）：
//! 网络依赖（`ureq`/`rustls`）只进 `whcode-llm` 的可选 `http` feature，唯一启用者为
//! `whcode-bin`；四个前端与默认构建的依赖树不含任何网络栈——无剖面即无请求，
//! 以可执行的依赖树断言代替此前 `cargo tree` 人工核验（技术债登记 2026-10-25）。
//!
//! 口径：`cargo tree --locked -e normal` 的输出文本（-e normal 排除 dev/build 依赖，
//! 与运行期实际链接的依赖一致）；`--locked` 防止测试改动锁文件。

use std::process::Command;

fn workspace_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("tests/ 的父目录即 workspace 根")
        .to_path_buf()
}

/// 运行 `cargo tree` 并返回输出；失败（非零退出）时 panic 并附 stderr。
fn cargo_tree(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO"))
        .args(args)
        .current_dir(workspace_root())
        .output()
        .expect("运行 cargo tree");
    assert!(
        output.status.success(),
        "cargo tree 失败：{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

/// NFR-213：默认构建（无 http feature）的 whcode-llm 依赖树不含网络栈。
#[test]
fn llm_default_features_exclude_network_stack() {
    let tree = cargo_tree(&[
        "tree",
        "--locked",
        "-e",
        "normal",
        "--no-default-features",
        "-p",
        "whcode-llm",
    ]);
    for forbidden in ["ureq", "rustls"] {
        assert!(
            !tree.contains(forbidden),
            "无 http feature 的 whcode-llm 依赖树不得含 {forbidden}（默认不联网，NFR-213）"
        );
    }
}

/// ADR-031：网络栈经 http feature 进入，且只挂在 whcode-llm 之下（启用者仅装配根）。
#[test]
fn http_feature_introduces_network_stack_via_llm_only() {
    let tree = cargo_tree(&[
        "tree",
        "--locked",
        "-e",
        "normal",
        "-p",
        "whcode-llm",
        "--features",
        "http",
    ]);
    assert!(
        tree.contains("ureq"),
        "http feature 必须把 ureq 引入依赖树（否则本门禁测不到任何东西）"
    );
}

/// FR-203：四个前端的**直接依赖**不含 whcode-llm（网络能力不进前端；llm 能力只经
/// whcode-app 命令面传递，`--depth 1` 排除经 whcode-app 的间接路径）。
#[test]
fn frontends_do_not_depend_on_llm() {
    for frontend in ["whcode-tui", "whcode-cui", "whcode-web", "whcode-gui"] {
        let tree = cargo_tree(&[
            "tree", "--locked", "-e", "normal", "--depth", "1", "-p", frontend,
        ]);
        assert!(
            !tree.contains("whcode-llm"),
            "{frontend} 的直接依赖不得含 whcode-llm（网络能力只进装配根，ADR-031）"
        );
    }
}
