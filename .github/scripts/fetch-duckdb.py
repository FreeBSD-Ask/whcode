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

目标三元组与载荷三元组的区别（ADR-073，2026-11-14）：
  本仓库的 Windows 验收目标已改为 `x86_64-pc-windows-gnullvm`，但 DuckDB 官方只发布
  MSVC 载荷，且 libduckdb 是 C ABI（`duckdb.h`），MSVC 导入库在 lld 下可链接、可运行
  （ADR-073 §3 实测）。因此 `PAYLOAD_TRIPLE` 把编译目标映射到载荷目录名：目录名保持
  发行方口径（`*-windows-msvc`），映射关系写死在表里、可审计；不得“就近找别的目录”。

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
import gzip
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

#: fts 扩展（ADR-099）：载荷三元组 -> (官方平台名, .gz SHA256, .gz 字节数)。
#: URL 固定为 `https://extensions.duckdb.org/v<版本>/<平台>/fts.duckdb_extension.gz`；
#: 哈希为本机下载后复算值（非官方公布值），与 duckdb.dll 同一纪律：校验失败即中止。
#: 未登记的平台显式报错，不静默跳过——缺失时运行期会回退到子串检索（ADR-099 决策 4）。
FTS_EXTENSIONS: dict[str, tuple[str, str, int]] = {
    "x86_64-pc-windows-msvc": (
        "windows_amd64",
        "d5328ff1569098d3e8651ab158e5cff8f4eb292986a7e1752300a653b3435e13",
        14_751_504,
    ),
}

#: 扩展下载基址（版本与 DUCKDB_VERSION 绑定）。
FTS_BASE_URL = "https://extensions.duckdb.org"

#: 归档内必须存在的成员（缺任一项即视为资产不完整，链接或运行必失败）。
REQUIRED_MEMBERS = ("duckdb.h", "duckdb.dll", "duckdb.lib")

#: 编译目标三元组 -> 载荷三元组（ADR-073）。gnullvm 是编译目标，官方只有 MSVC 载荷；
#: 未登记的三元组原样返回，由 PACKAGES/NO_PREBUILT 判定报错。
PAYLOAD_TRIPLE: dict[str, str] = {
    "x86_64-pc-windows-gnullvm": "x86_64-pc-windows-msvc",
    "aarch64-pc-windows-gnullvm": "aarch64-pc-windows-msvc",
}


def payload_target(target: str) -> str:
    """把编译目标三元组归一为载荷三元组（目录名按发行方口径）。"""
    return PAYLOAD_TRIPLE.get(target, target)


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
    """推断当前主机对应的**载荷**三元组（覆盖本项目平台矩阵中的 Windows 两架构）。

    返回的是载荷三元组而非编译目标三元组：Windows 主机无论 rustc 主机是 msvc 还是
    gnullvm（ADR-073），可取的都是官方 MSVC 载荷。
    """
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
    # extensions.duckdb.org 拒绝默认的 Python-urllib UA（403）；显式 UA 与 curl 行为一致。
    request = urllib.request.Request(url, headers={"User-Agent": "whcode-fetch-duckdb/1"})
    with urllib.request.urlopen(request, timeout=600) as resp, tmp.open("wb") as handle:
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


def fetch_fts(target: str) -> int:
    """下载并解压 fts 扩展到 `<目标目录>/fts.duckdb_extension`（ADR-099）。

    校验 .gz 的 SHA256 与字节数；失败即中止，不留半成品。
    未登记的平台返回 2（调用方据此提示“该平台扩展未登记”，不静默降级）。
    """
    entry = FTS_EXTENSIONS.get(target)
    if entry is None:
        print(
            f"fts 扩展未登记平台 {target}：检索将回退到子串路径（ADR-099 决策 4）。"
            "如需启用全文检索，请实测该平台扩展的 .gz SHA256 与字节数后登记本表。",
            file=sys.stderr,
        )
        return 2
    platform_name, expected, expected_size = entry
    url = f"{FTS_BASE_URL}/v{DUCKDB_VERSION}/{platform_name}/fts.duckdb_extension.gz"
    cache = cache_dir() / f"fts-{platform_name}-{DUCKDB_VERSION}.duckdb_extension.gz"
    cache.parent.mkdir(parents=True, exist_ok=True)
    if not (cache.exists() and sha256_of(cache) == expected):
        print(f"下载 fts 扩展：{url}")
        download(url, cache)
    actual = sha256_of(cache)
    size = cache.stat().st_size
    if actual != expected or size != expected_size:
        print(
            f"fts 扩展校验失败：期望 {expected} / {expected_size} 字节，"
            f"实际 {actual} / {size} 字节",
            file=sys.stderr,
        )
        return 1
    dest_dir = vendor_dir(target)
    dest_dir.mkdir(parents=True, exist_ok=True)
    dest = dest_dir / "fts.duckdb_extension"
    with gzip.open(cache, "rb") as src, open(dest, "wb") as out:
        shutil.copyfileobj(src, out)
    print(f"fts 扩展已就位：{dest}（{dest.stat().st_size} 字节）")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description="拉取并校验预编译 DuckDB")
    parser.add_argument("--check", action="store_true", help="只校验，不下载")
    parser.add_argument(
        "--target",
        default=None,
        choices=sorted(set(PACKAGES) | set(PAYLOAD_TRIPLE)),
        help="编译目标三元组（默认按主机推断；gnullvm 会映射到官方 MSVC 载荷，ADR-073）",
    )
    args = parser.parse_args()

    target = payload_target(args.target if args.target is not None else host_target())
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
    rc = verify_extracted(target)
    if rc != 0:
        return rc
    # ADR-099：fts 扩展随包分发；未登记平台只提示（检索回退），不阻断构建。
    fts_rc = fetch_fts(target)
    return 0 if fts_rc in (0, 2) else 1


if __name__ == "__main__":
    sys.exit(main())
