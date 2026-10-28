#!/usr/bin/env python3
"""拉取并校验预编译 DuckDB（唯一默认持久化后端，ADR-001 / tasks.md T022）。

纪律：
  - 只使用官方发布物（DuckDB 官方预编译包，MIT 许可）；来源 URL 与 SHA256 固定在
    本文件，不得从其他来源替换；
  - 校验失败即中止，不得以未校验的文件继续构建（ADR-010 报告纪律）；
  - 解包到 `vendor/duckdb/<版本>/<目标三元组>/`；该目录不入库（见 .gitignore），但
    拉取后会在其中写入 PROVENANCE.txt 作为本机留证。按三元组分目录后，同一版本
    的多架构载荷（amd64 与 arm64，ADR-043）可共存，互不覆盖。

平台覆盖（实测，见 docs/adr/ADR-DB-BUILD.md 与 docs/adr/ADR-043-windows-arm64-build-target.md）：
  - Windows x86_64（x86_64-pc-windows-msvc）：官方提供
    `duckdb-shared-libs-windows-amd64.tar.gz`（tar.gz，非 zip）；
  - Windows ARM64（aarch64-pc-windows-msvc）：官方提供
    `duckdb-shared-libs-windows-arm64.tar.gz`（ADR-043：构建交付目标，验收仍为 amd64）；
  - FreeBSD / NetBSD / OpenBSD / DragonFly：官方**不提供**预编译包，须经 pkg/ports 或
    以 `bundled-cmake` 源码构建；属后续真实平台验证项，本脚本显式报错而**不**静默降级。

安全解包（ADR-DB-BUILD）：先校验 SHA256 与字节数，再解包；拒绝绝对路径、`..` 路径穿越、
含反斜杠或盘符的成员名；归档内符号链接等其他类型不落盘。

用法：
    python .github/scripts/fetch-duckdb.py            # 按主机三元组拉取 + 校验 + 解包
    python .github/scripts/fetch-duckdb.py --check    # 只校验已解包目录
    python .github/scripts/fetch-duckdb.py --target aarch64-pc-windows-msvc
                                                      # 交叉构建前置：显式拉取目标三元组载荷
"""

from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import platform
import shutil
import sys
import tarfile
import urllib.request

# ---- 固定值（改动必须同步 docs/adr/ADR-DB-BUILD.md 与依赖记录） ----

#: DuckDB 版本，由 duckdb crate 版本映射而来（crate 1.20000.0 -> DuckDB 2.0.0）。
DUCKDB_VERSION = "2.0.0"

#: 官方预编译包：目标三元组 -> (归档名, SHA256, 字节数)。
#: linux/macos 包存在但本项目平台矩阵不包含它们，故不登记（不引入无用的“未验证承诺”）。
PACKAGES: dict[str, tuple[str, str, int]] = {
    "x86_64-pc-windows-msvc": (
        "duckdb-shared-libs-windows-amd64.tar.gz",
        "2b83169128239de5f45fe21de80f99bf436e53aa4bb24020dd951a1ba8d4b95f",
        19_833_650,
    ),
    # ADR-043（2026-10-28）：SHA256 与字节数为官方发布物实测值（下载后复算），非官方公布值。
    "aarch64-pc-windows-msvc": (
        "duckdb-shared-libs-windows-arm64.tar.gz",
        "159ce60eb587112b8c5f290b8beea56af5f13be4d13679de127ad9bfaafb7447",
        18_389_170,
    ),
}

#: 归档内必须存在的成员（缺任一项即视为资产不完整，链接或运行必失败）。
REQUIRED_MEMBERS = ("duckdb.h", "duckdb.dll", "duckdb.lib")

#: PROVENANCE.txt 登记的归档成员（按下列固定顺序输出）。
PROVENANCE_MEMBERS = (
    "duckdb.dll",
    "duckdb.lib",
    "duckdb.h",
    "duckdb_v2.h",
    "duckdb_extension.h",
    "duckdb_extension_v2.h",
    "duckdb_static_extension.h",
)

#: 平台矩阵中**无**官方预编译包的目标（须走 pkg/ports 或源码构建）。
NO_PREBUILT = (
    "x86_64-unknown-freebsd",
    "x86_64-unknown-netbsd",
    "x86_64-unknown-openbsd",
    "x86_64-unknown-dragonfly",
)

RELEASE_URL = "https://github.com/duckdb/duckdb/releases/download/v{version}/{archive}"
REPO_ROOT = pathlib.Path(__file__).resolve().parents[2]


def host_target() -> str:
    """推断当前主机对应的 Rust 目标三元组（覆盖本项目平台矩阵中的 Windows 两架构）。"""
    system = platform.system().lower()
    machine = platform.machine().lower()
    if system == "windows":
        if machine in {"amd64", "x86_64"}:
            return "x86_64-pc-windows-msvc"
        if machine in {"arm64", "aarch64"}:
            return "aarch64-pc-windows-msvc"
    return f"{machine}-unknown-{system}"


def sha256_of(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def vendor_dir(target: str | None = None) -> pathlib.Path:
    base = REPO_ROOT / "vendor" / "duckdb" / DUCKDB_VERSION
    return base if target is None else base / target


def cache_dir() -> pathlib.Path:
    return REPO_ROOT / "vendor" / "duckdb" / ".cache"


def download(url: str, dest: pathlib.Path) -> None:
    dest.parent.mkdir(parents=True, exist_ok=True)
    tmp = dest.with_suffix(dest.suffix + ".part")
    print(f"下载 {url}")
    with urllib.request.urlopen(url, timeout=600) as resp, tmp.open("wb") as handle:
        shutil.copyfileobj(resp, handle)
    tmp.replace(dest)


def fetch(target: str, archive: str, expected: str, expected_size: int) -> pathlib.Path:
    url = RELEASE_URL.format(version=DUCKDB_VERSION, archive=archive)
    cached = cache_dir() / archive
    if cached.exists() and sha256_of(cached) == expected:
        print(f"复用已校验缓存 {cached}")
    else:
        download(url, cached)

    actual = sha256_of(cached)
    actual_size = cached.stat().st_size
    if actual != expected or actual_size != expected_size:
        sys.exit(
            f"校验不匹配，已中止（未解包）：\n  期望 SHA256 {expected} / {expected_size} 字节\n"
            f"  实际 SHA256 {actual} / {actual_size} 字节\n  文件 {cached}\n"
            "禁止以未校验的库继续构建。"
        )
    print(f"SHA256 校验通过：{actual}（{actual_size} 字节）")

    dest = vendor_dir(target)
    if dest.exists():
        shutil.rmtree(dest)
    dest.mkdir(parents=True)
    members = extract_tar_gz(cached, dest)
    print(f"解包到 {dest}（{len(members)} 个成员）")

    missing = [name for name in REQUIRED_MEMBERS if name not in members]
    if missing:
        sys.exit(f"归档缺少必需成员 {missing}：{cached}")

    (dest / "PROVENANCE.txt").write_text(
        "\n".join(
            [
                "来源（官方发布物，MIT 许可）",
                f"  版本: DuckDB {DUCKDB_VERSION}",
                f"  归档: {archive}",
                f"  URL : {url}",
                f"  SHA256: {expected}",
                f"  字节数: {actual_size}",
                f"  目标三元组: {target}",
                "",
                "归档成员（名称: 解包后字节数）",
                *[
                    f"  {name}: {members[name] if name in members else '缺失'}"
                    for name in PROVENANCE_MEMBERS
                ],
                "",
                "由 .github/scripts/fetch-duckdb.py 生成；本目录不入库。",
                "",
            ]
        ),
        encoding="utf-8",
        newline="\n",
    )
    return dest


def reject_unsafe_member(name: str) -> None:
    """拒绝绝对路径、`..` 路径穿越、含反斜杠或盘符的成员名（不落盘直接中止）。"""
    pure = pathlib.PurePosixPath(name)
    if (
        pure.is_absolute()
        or any(part == ".." for part in pure.parts)
        or "\\" in name
        or ":" in name
    ):
        sys.exit(f"拒绝不安全的归档成员（路径穿越）：{name!r}")


def extract_tar_gz(archive_path: pathlib.Path, dest: pathlib.Path) -> dict[str, int]:
    """安全解包 tar.gz，返回“归档成员名 -> 字节数”。

    先校验全部成员名再落盘：任一成员含绝对路径、`..`、反斜杠或盘符即整体中止，
    不会写出目标目录之外，也不会留下半解包状态。只解包普通文件，符号链接、硬链接、
    设备文件等一律跳过（官方包不含这些类型，出现即视为可疑）。
    """
    with tarfile.open(archive_path, "r:gz") as archive:
        files: list[tarfile.TarInfo] = []
        for member in archive.getmembers():
            if not member.isfile():
                print(f"跳过非普通文件成员：{member.name}")
                continue
            reject_unsafe_member(member.name)
            files.append(member)

        extracted: dict[str, int] = {}
        for member in files:
            name = member.name
            target = dest.joinpath(*pathlib.PurePosixPath(name).parts)
            source = archive.extractfile(member)
            if source is None:
                sys.exit(f"无法读取归档成员：{name!r}")
            target.parent.mkdir(parents=True, exist_ok=True)
            with source, target.open("wb") as handle:
                shutil.copyfileobj(source, handle)
            if target.stat().st_size != member.size:
                sys.exit(f"解包字节数不符：{name!r}（期望 {member.size}）")
            extracted[name] = member.size
    return extracted


def extracted_targets() -> list[str]:
    """已解包的目标三元组列表（vendor/duckdb/<版本>/ 下的三元组子目录，排除 .cache）。"""
    root = vendor_dir()
    if not root.is_dir():
        return []
    return sorted(p.name for p in root.iterdir() if p.is_dir() and p.name != ".cache")


def verify_extracted(target: str | None = None) -> int:
    if target is not None:
        dest = vendor_dir(target)
        if not dest.is_dir():
            print(f"未找到 {dest}：请先执行 python .github/scripts/fetch-duckdb.py --target {target}")
            return 1
        missing = [name for name in ("duckdb.h", "duckdb.dll") if not (dest / name).is_file()]
        if missing:
            print(f"{dest} 缺少必需文件：{missing}")
            return 1
        print(f"已解包目录可用：{dest}")
        return 0

    targets = extracted_targets()
    if not targets:
        print(
            f"未找到 {vendor_dir()}/<目标三元组>："
            "请先执行 python .github/scripts/fetch-duckdb.py [--target <三元组>]"
        )
        return 1
    ready: dict[str, str] = {}
    for name in targets:
        dest = vendor_dir(name)
        missing = [item for item in ("duckdb.h", "duckdb.dll") if not (dest / item).is_file()]
        ready[name] = (
            f"{dest}（预编译来源：{json.dumps({name: archive for target, (archive, _, _) in PACKAGES.items() if target == name}, ensure_ascii=False)}）"
            if not missing
            else f"{dest} 缺少必需文件：{missing}"
        )
    for name, line in ready.items():
        print(line)
    return 0 if all("缺少" not in line for line in ready.values()) else 1


def main() -> int:
    parser = argparse.ArgumentParser(description="拉取并校验预编译 DuckDB")
    parser.add_argument("--check", action="store_true", help="只校验，不下载")
    parser.add_argument(
        "--target",
        default=None,
        choices=sorted(PACKAGES),
        help="目标三元组（默认按主机推断；交叉构建时显式指定目标架构）",
    )
    args = parser.parse_args()

    target = args.target if args.target is not None else host_target()
    if target in NO_PREBUILT:
        sys.exit(
            f"{target} 无官方预编译包：请改用系统包（pkg/ports）或以 bundled-cmake "
            "源码构建，并在 docs/adr/ADR-DB-BUILD.md 记录结论（不得静默降级默认后端）。"
        )
    if target not in PACKAGES:
        supported = ", ".join(sorted(PACKAGES))
        sys.exit(f"未登记预编译来源的目标 {target}；已登记：{supported}")

    if args.check:
        return verify_extracted(target)

    archive, expected, expected_size = PACKAGES[target]
    fetch(target, archive, expected, expected_size)
    return verify_extracted(target)


if __name__ == "__main__":
    sys.exit(main())
