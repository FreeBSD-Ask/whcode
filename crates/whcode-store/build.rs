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
//!   `libduckdb-sys` 显式报错，不在本脚本吞掉。

use std::{
    env,
    path::{Path, PathBuf},
};

fn main() {
    println!("cargo:rerun-if-env-changed=DUCKDB_LIB_DIR");
    let Ok(lib_dir) = env::var("DUCKDB_LIB_DIR") else {
        return;
    };

    // 只处理 Windows 形态的载荷；其他平台没有该文件，跳过复制（不做平台判定）。
    let dll = Path::new(&lib_dir).join("duckdb.dll");
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
