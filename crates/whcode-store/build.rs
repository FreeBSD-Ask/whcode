//! 构建脚本：把预编译 DuckDB 动态库放到产物目录旁。
//!
//! 背景：`libduckdb-sys`（非 bundled，`DUCKDB_LIB_DIR` 模式）只负责**链接**，
//! 不会把 `duckdb.dll` 复制到产物目录；而 Windows 的 DLL 搜索路径不含 vendor 目录，
//! 二进制与测试进程启动时即加载失败。本脚本把 `duckdb.dll` 复制到
//! `<target>/<profile>/`（最终二进制旁）、`<target>/<profile>/deps/`（cargo ≤1.99 的
//! 测试二进制所在目录）与本包 `OUT_DIR`（cargo 1.100 起单测二进制所在目录，
//! `build/<pkg>/<hash>/out`，cargo#17287 布局重组——ADR-069 修订 2026-11-14）。
//!
//! 纪律：
//! - **不做平台判定**（NFR-101：平台差异必须集中在 `whcode-platform`）。判据是
//!   “`DUCKDB_LIB_DIR` 下是否存在 `duckdb.dll`”——官方 Windows 包才有该文件；
//!   BSD/Linux 包为 `.so`/`.dylib`，因此自然跳过，无需任何平台条件编译或构建期平台环境变量；
//! - `DUCKDB_LIB_DIR` 未设置或目录缺失时**跳过复制**——链接阶段的缺失由
//!   `libduckdb-sys` 显式报错，不在本脚本吞掉；
//! - **相对 `DUCKDB_LIB_DIR` 按工作区根解析**：构建脚本的 cwd 是 crate 目录，直接拼相对
//!   路径会把 `vendor/...` 判成不存在而静默跳过复制；链接期 rustc 的 cwd 却是工作区根，
//!   于是产物缺 dll 却编译通过、运行期才 `STATUS_DLL_NOT_FOUND`（run 40691533216／40691532314；
//!   1.100 的 windows-arm64 通道再次复现——工作流导出的相对 `DUCKDB_LIB_DIR` 绕过了
//!   `[env] relative = true` 的绝对化，已在工作流侧改为绝对路径）；
//! - **复制失败显式失败**（ADR-069 修订 2026-11-14，信号链完备）：此前的 `let _ = copy`
//!   把 ACL/占用等失败吞成“错误的空”——链接照常通过，运行期才以不可归因的
//!   `STATUS_DLL_NOT_FOUND` 暴露（run 43493963908 的 measurements）。复制失败现在
//!   直接使构建失败，不得静默。

use std::{
    env,
    path::{Path, PathBuf},
};

fn main() {
    println!("cargo:rerun-if-env-changed=DUCKDB_LIB_DIR");
    let Ok(raw_lib_dir) = env::var("DUCKDB_LIB_DIR") else {
        return;
    };
    let lib_dir = resolve_lib_dir(&raw_lib_dir);

    // 只处理 Windows 形态的载荷；其他平台没有该文件，跳过复制（不做平台判定）。
    let dll = lib_dir.join("duckdb.dll");
    println!("cargo:rerun-if-changed={}", dll.display());
    if !dll.is_file() {
        return;
    }

    let Some((profile_dir, deps_dir, out_dir)) = profile_dirs() else {
        return;
    };
    for dest in [
        profile_dir.join("duckdb.dll"),
        deps_dir.join("duckdb.dll"),
        out_dir.join("duckdb.dll"),
    ] {
        if dest == dll {
            continue;
        }
        let parent = dest.parent().unwrap_or_else(|| {
            panic!("duckdb.dll 目标路径无父目录：{}", dest.display())
        });
        std::fs::create_dir_all(parent)
            .unwrap_or_else(|err| panic!("创建目录 {} 失败：{err}", parent.display()));
        std::fs::copy(&dll, &dest).unwrap_or_else(|err| {
            panic!(
                "复制 duckdb.dll 到 {} 失败：{err}——静默缺 DLL 会在运行期变成\
                 不可归因的 STATUS_DLL_NOT_FOUND（信号链完备：显式失败）",
                dest.display()
            )
        });
    }
}

/// 相对 `DUCKDB_LIB_DIR` 按**工作区根**解析（与 `.cargo/config.toml` 的 `relative = true` 同口径）。
///
/// 构建脚本进程的 cwd 是 crate 目录（如 `crates/whcode-store`），而 rustc 链接期的 cwd 是
/// 工作区根；两处对同一相对路径的解析基准不同，正是 dll 未复制却链接通过的成因。
fn resolve_lib_dir(raw: &str) -> PathBuf {
    let path = Path::new(raw);
    if path.is_absolute() {
        return path.to_path_buf();
    }
    workspace_root().join(path)
}

/// 从 `CARGO_MANIFEST_DIR` 逐级上溯，返回含 `[workspace]` 节的 Cargo.toml 所在目录。
///
/// 上溯失败时退回当前目录（保持既有相对路径行为，不额外造错）。
fn workspace_root() -> PathBuf {
    let Ok(manifest_dir) = env::var("CARGO_MANIFEST_DIR") else {
        return PathBuf::from(".");
    };
    let mut dir = PathBuf::from(manifest_dir);
    loop {
        let manifest = dir.join("Cargo.toml");
        if let Ok(text) = std::fs::read_to_string(&manifest)
            && text
                .lines()
                .any(|line| line.trim_start().starts_with("[workspace]"))
        {
            return dir;
        }
        if !dir.pop() {
            return PathBuf::from(".");
        }
    }
}

/// 从 Cargo 的 `OUT_DIR` 布局推导 `<target>/<profile>/`、`deps/` 与本包 `OUT_DIR`。
///
/// 布局兼容（ADR-069 修订 2026-11-14）：cargo ≤1.99 为
/// `<target-dir>/[<triple>/]<profile>/build/<pkg>-<hash>/out`，1.100 起中间产物重组为
/// `.../build/<pkg>/<hash>/out`（hash 上移一层，cargo#17287）。不假设层级数：
/// 从 `out` 逐级上溯到名为 `build` 的目录（有界 4 级），其父即 profile 根。
/// 识别失败返回 `None`（跳过复制，不影响编译）。
fn profile_dirs() -> Option<(PathBuf, PathBuf, PathBuf)> {
    let out_dir = PathBuf::from(env::var("OUT_DIR").ok()?);
    if out_dir.file_name()? != "out" {
        return None;
    }
    let mut dir = out_dir.parent()?;
    for _ in 0..4 {
        let name = dir.file_name()?;
        if name == "build" {
            let profile = dir.parent()?;
            return Some((profile.to_path_buf(), profile.join("deps"), out_dir));
        }
        dir = dir.parent()?;
    }
    None
}
