//! 构建脚本：把预编译 DuckDB 动态库放到产物目录旁。
//!
//! 背景：`libduckdb-sys`（非 bundled，`DUCKDB_LIB_DIR` 模式）只负责**链接**，
//! 不会把 `duckdb.dll` 复制到产物目录；而 Windows 的 DLL 搜索路径不含 vendor 目录，
//! 二进制与测试进程启动时即加载失败。本脚本把 `duckdb.dll` 复制到
//! `<target>/<profile>/` 与 `<target>/<profile>/deps/`（测试二进制所在目录）。
//!
//! 纪律：
//! - **不做平台判定**（NFR-101：平台差异必须集中在 `whcode-platform`）。判据是
//!   “`DUCKDB_LIB_DIR` 下是否存在 `duckdb.dll`”——官方 Windows 包才有该文件；
//!   BSD/Linux 包为 `.so`/`.dylib`，因此自然跳过，无需任何平台条件编译或构建期平台环境变量；
//! - `DUCKDB_LIB_DIR` 未设置或目录缺失时**跳过复制**——链接阶段的缺失由
//!   `libduckdb-sys` 显式报错，不在本脚本吞掉；
//! - **相对 `DUCKDB_LIB_DIR` 按工作区根解析**：构建脚本的 cwd 是 crate 目录，直接拼相对
//!   路径会把 `vendor/...` 判成不存在而静默跳过复制；链接期 rustc 的 cwd 却是工作区根，
//!   于是产物缺 dll 却编译通过、运行期才 `STATUS_DLL_NOT_FOUND`（run 40691533216／40691532314）。
//!   本解析与 `.cargo/config.toml` 的 `relative = true` 同口径。

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

    let Some((profile_dir, deps_dir)) = profile_dirs() else {
        return;
    };
    for dest in [profile_dir.join("duckdb.dll"), deps_dir.join("duckdb.dll")] {
        if dest == dll {
            continue;
        }
        if let Some(parent) = dest.parent() {
            let _ = std::fs::create_dir_all(parent);
            let _ = std::fs::copy(&dll, &dest);
        }
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

/// 从 Cargo 的 `OUT_DIR` 布局推导 `<target>/<profile>/` 与其 `deps/`。
///
/// Cargo 当前布局：`<target-dir>/[<triple>/]<profile>/build/<pkg>-<hash>/out`。
/// 这是实现细节，识别失败时返回 `None`（跳过复制，不影响编译）。
fn profile_dirs() -> Option<(PathBuf, PathBuf)> {
    let out_dir = PathBuf::from(env::var("OUT_DIR").ok()?);
    if out_dir.file_name()? != "out" {
        return None;
    }
    let build_dir = out_dir.parent()?.parent()?;
    if build_dir.file_name()? != "build" {
        return None;
    }
    let profile = build_dir.parent()?;
    Some((profile.to_path_buf(), profile.join("deps")))
}
