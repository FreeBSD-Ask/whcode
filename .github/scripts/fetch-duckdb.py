#!/usr/bin/env python3
"""拉取并校验预编译 DuckDB（唯一默认持久化后端，ADR-001 / tasks.md T022）。

纪律：
  - 只使用官方发布物（DuckDB 官方预编译包，MIT 许可）；来源 URL 与 SHA256 固定在
    本文件，不得从其他来源替换；
  - 校验失败即中止，不得以未校验的文件继续构建（ADR-010 报告纪律）；
  - 解包到 `vendor/duckdb/<version>/`；该目录不入库（见 .gitignore），但拉取后
    会在其中写入 PROVENANCE.txt 作为本机留证。

平台覆盖（M0 实测，见 docs/adr/ADR-DB-BUILD.md）：
  - Windows x86_64（x86_64-pc-windows-msvc）：官方提供 `libduckdb-windows-amd64.zip`；
  - FreeBSD / NetBSD / OpenBSD / DragonFly：官方**不提供**预编译包，须经 pkg/ports 或
    以 `bundled-cmake` 源码构建；属后续真实平台验证项，本脚本显式报错而**不**静默降级。

用法：
    python ci/fetch-duckdb.py            # 拉取 + 校验 + 解包
    python ci/fetch-duckdb.py --check    # 只校验已存在的缓存/已解包目录
"""

from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import platform
import shutil
import sys
import urllib.request
import zipfile

# ---- 固定值（改动必须同步 docs/adr/ADR-DB-BUILD.md 与依赖记录） ----

#: DuckDB 版本，由 duckdb crate 版本映射而来（crate 1.10506.1 -> DuckDB 1.5.6）。
DUCKDB_VERSION = "1.5.6"

#: 官方预编译包：目标三元组 -> (归档名, SHA256)。
#: linux/macos 包存在但本项目平台矩阵不包含它们，故不登记（不引入无用的"未验证承诺"）。
PACKAGES: dict[str, tuple[str, str]] = {
    "x86_64-pc-windows-msvc": (
        "libduckdb-windows-amd64.zip",
        "44cf59583f9951d2cb09b1bf115a63ecb2d8901e363903029d86c7d8683fe96a",
    ),
}

#: 平台矩阵中**无**官方预编译包的目标（须走 pkg/ports 或源码构建）。
NO_PREBUILT = (
    "x86_64-unknown-freebsd",
    "x86_64-unknown-netbsd",
    "x86_64-unknown-openbsd",
    "x86_64-unknown-dragonfly",
)

RELEASE_URL = "https://github.com/duckdb/duckdb/releases/download/v{version}/{archive}"
REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent


def host_target() -> str:
    """推断当前主机对应的 Rust 目标三元组（仅覆盖本项目平台矩阵中的 Windows）。"""
    system = platform.system().lower()
    machine = platform.machine().lower()
    if system == "windows" and machine in {"amd64", "x86_64"}:
        return "x86_64-pc-windows-msvc"
    return f"{machine}-unknown-{system}"


def sha256_of(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def vendor_dir() -> pathlib.Path:
    return REPO_ROOT / "vendor" / "duckdb" / DUCKDB_VERSION


def cache_dir() -> pathlib.Path:
    return REPO_ROOT / "vendor" / "duckdb" / ".cache"


def download(url: str, dest: pathlib.Path) -> None:
    dest.parent.mkdir(parents=True, exist_ok=True)
    tmp = dest.with_suffix(dest.suffix + ".part")
    print(f"下载 {url}")
    with urllib.request.urlopen(url, timeout=600) as resp, tmp.open("wb") as handle:
        shutil.copyfileobj(resp, handle)
    tmp.replace(dest)


def fetch(target: str, archive: str, expected: str) -> pathlib.Path:
    url = RELEASE_URL.format(version=DUCKDB_VERSION, archive=archive)
    cached = cache_dir() / archive
    if cached.exists() and sha256_of(cached) == expected:
        print(f"复用已校验缓存 {cached}")
    else:
        download(url, cached)

    actual = sha256_of(cached)
    if actual != expected:
        sys.exit(
            f"SHA256 不匹配，已中止：\n  期望 {expected}\n  实际 {actual}\n"
            f"  文件 {cached}\n禁止以未校验的库继续构建。"
        )
    print(f"SHA256 校验通过：{actual}")

    dest = vendor_dir()
    if dest.exists():
        shutil.rmtree(dest)
    dest.mkdir(parents=True)
    with zipfile.ZipFile(cached) as zf:
        zf.extractall(dest)
    print(f"解包到 {dest}")

    (dest / "PROVENANCE.txt").write_text(
        "\n".join(
            [
                "来源（官方发布物，MIT 许可）",
                f"  版本: DuckDB {DUCKDB_VERSION}",
                f"  归档: {archive}",
                f"  URL : {url}",
                f"  SHA256: {expected}",
                f"  目标三元组: {target}",
                "",
                "由 ci/fetch-duckdb.py 生成；本目录不入库。",
                "",
            ]
        ),
        encoding="utf-8",
        newline="\n",
    )
    return dest


def verify_extracted() -> int:
    dest = vendor_dir()
    if not dest.is_dir():
        print(f"未找到 {dest}：请先执行 python ci/fetch-duckdb.py")
        return 1
    missing = [name for name in ("duckdb.h",) if not (dest / name).is_file()]
    if missing:
        print(f"{dest} 缺少必需文件：{missing}")
        return 1
    sources = json.dumps(
        {target: archive for target, (archive, _) in PACKAGES.items()},
        ensure_ascii=False,
    )
    print(f"已解包目录可用：{dest}（预编译来源：{sources}）")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description="拉取并校验预编译 DuckDB")
    parser.add_argument("--check", action="store_true", help="只校验，不下载")
    args = parser.parse_args()

    if args.check:
        return verify_extracted()

    target = host_target()
    if target in NO_PREBUILT:
        sys.exit(
            f"{target} 无官方预编译包：请改用系统包（pkg/ports）或以 bundled-cmake "
            "源码构建，并在 docs/adr/ADR-DB-BUILD.md 记录结论（不得静默降级默认后端）。"
        )
    if target not in PACKAGES:
        supported = ", ".join(sorted(PACKAGES))
        sys.exit(f"未登记预编译来源的目标 {target}；已登记：{supported}")

    archive, expected = PACKAGES[target]
    fetch(target, archive, expected)
    return verify_extracted()


if __name__ == "__main__":
    sys.exit(main())
