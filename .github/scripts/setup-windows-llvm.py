#!/usr/bin/env python3
"""安装 Windows 的 LLVM 系列构建工具链（llvm-mingw ＋ rustup gnullvm，ADR-073）。

背景：Windows 验收目标已由 `x86_64-pc-windows-msvc` 改为 `x86_64-pc-windows-gnullvm`
（2026-11-14 用户指令“能切 llvm 系列的都切，不用 GNU 那套”）。rustup 的 gnullvm 主机工具链
提供 rustc 与 lld 配套，但**不含 C 依赖所需的 mingw 头文件与 Windows 导入库**——rust-mingw
自包含组件只有 crt2.o 与 kernel32，缺 advapi32/ole32/oleaut32 等（尖峰实测，ADR-073 §3）。
llvm-mingw 提供 clang 驱动、mingw-w64 UCRT 头、自包含 CRT 与全部导入库，且属 LLVM 系列
（clang／lld／llvm-ar），不引入 GCC/binutils，符合 D-01 修订后的口径。

纪律：
  - 版本与 SHA256 固定在本文件；校验失败即中止，禁止“换一个能下载的版本继续”（ADR-010）；
  - 只从上游官方发布页取包（mstorsjo/llvm-mingw 的 release 资产）；
  - 解包拒绝绝对路径、`..` 穿越、盘符与反斜杠成员名（与 fetch-duckdb.py 同口径）；
  - 不做静默回退：驱动缺文件、工具链装不上都显式失败。

用法：
    python .github/scripts/setup-windows-llvm.py            # CI：写 GITHUB_PATH／GITHUB_ENV
    python .github/scripts/setup-windows-llvm.py --print-env # 本地：打印需要导出的环境变量

在 GitHub Actions 中，PATH 与 RUSTUP_TOOLCHAIN 通过 `$GITHUB_PATH`／`$GITHUB_ENV` 传给
后续步骤；本地运行时脚本无法修改父 shell 环境，故只打印可复制的命令。
"""

from __future__ import annotations

import argparse
import hashlib
import os
import pathlib
import platform
import shutil
import subprocess
import sys
import tempfile
import zipfile

#: llvm-mingw 版本（固定）。升级必须同步 ADR-073 与 rust-toolchain.toml 的前置条件说明。
LLVM_MINGW_VERSION = "20261104"

#: UCRT 变体、x86_64 宿主包（内含 aarch64/armv7/i686 交叉编译器，本仓库只用 x86_64）。
ARCHIVE = f"llvm-mingw-{LLVM_MINGW_VERSION}-ucrt-x86_64.zip"

#: 官方 release 资产 SHA256 与字节数（2026-11-14 下载复算，非上游公布值）。
ARCHIVE_SHA256 = "552191b5754c49a5a20723276a6fc56541ea1922c4c50b7b669022333fc1b40f"
ARCHIVE_SIZE = 191_037_046

#: 上游下载地址（GitHub release；Actions 直连，本机可用加速前缀，见 --mirror）。
RELEASE_URL = (
    "https://github.com/mstorsjo/llvm-mingw/releases/download/{version}/{archive}"
)

#: rustup 工具链名：版本锁 1.100.0（ADR-069），主机三元组 gnullvm（ADR-073）。
RUSTUP_TOOLCHAIN = "1.100.0-x86_64-pc-windows-gnullvm"

#: 必须存在的驱动（缺任一项即视为解包不完整）。
REQUIRED_BINARIES = ("x86_64-w64-mingw32-clang.exe", "x86_64-w64-mingw32-clang++.exe")

#: 解包后的顶层目录名（归档内固定）。
EXTRACT_DIR_NAME = f"llvm-mingw-{LLVM_MINGW_VERSION}-ucrt-x86_64"


def fail(message: str) -> int:
    print(f"FAIL: {message}")
    return 1


def cache_dir() -> pathlib.Path:
    """下载缓存目录：可由 WHCODE_TOOLCACHE 覆盖；默认落在系统临时目录下（clean.py 可回收）。"""
    override = os.environ.get("WHCODE_TOOLCACHE")
    base = pathlib.Path(override) if override else pathlib.Path(tempfile.gettempdir()) / "whcode-toolcache"
    return base


def sha256_of(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def download(url: str, dest: pathlib.Path) -> None:
    dest.parent.mkdir(parents=True, exist_ok=True)
    tmp = dest.with_suffix(dest.suffix + ".part")
    print(f"下载 {url}")
    # 只用标准库：urllib 跟随 GitHub 的 302 到 objects.githubusercontent.com。
    import urllib.request

    with urllib.request.urlopen(url, timeout=600) as resp, tmp.open("wb") as handle:
        shutil.copyfileobj(resp, handle)
    tmp.replace(dest)


def safe_extract(archive: pathlib.Path, dest: pathlib.Path) -> list[str]:
    """解包 zip，拒绝路径穿越与绝对路径；返回成员名列表。"""
    members: list[str] = []
    dest.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(archive) as bundle:
        for info in bundle.infolist():
            name = info.filename
            pure = pathlib.PurePosixPath(name)
            if (
                pure.is_absolute()
                or ".." in pure.parts
                or "\\" in name
                or ":" in name.split("/")[0]
            ):
                raise RuntimeError(f"归档成员路径不安全，拒绝解包：{name}")
            members.append(name)
            if name.endswith("/"):
                continue
            target = dest.joinpath(*pure.parts)
            target.parent.mkdir(parents=True, exist_ok=True)
            with bundle.open(info) as source, target.open("wb") as handle:
                shutil.copyfileobj(source, handle)
    return members


def ensure_archive(mirror: str | None) -> pathlib.Path:
    """取得并校验归档；已缓存的同哈希文件直接复用。"""
    cached = cache_dir() / ARCHIVE
    if cached.is_file() and sha256_of(cached) == ARCHIVE_SHA256:
        print(f"复用已校验缓存 {cached}")
        return cached

    url = RELEASE_URL.format(version=LLVM_MINGW_VERSION, archive=ARCHIVE)
    if mirror:
        url = f"{mirror.rstrip('/')}/{url}"
    download(url, cached)

    actual = sha256_of(cached)
    size = cached.stat().st_size
    if actual != ARCHIVE_SHA256 or size != ARCHIVE_SIZE:
        raise RuntimeError(
            "校验不匹配，已中止：\n"
            f"  期望 SHA256 {ARCHIVE_SHA256} / {ARCHIVE_SIZE} 字节\n"
            f"  实际 SHA256 {actual} / {size} 字节\n"
            f"  文件 {cached}\n"
            "禁止以未校验的工具链继续构建（ADR-010／ADR-073 §8）。"
        )
    print(f"SHA256 校验通过：{actual}（{size} 字节）")
    return cached


def install_toolchain_dir(archive: pathlib.Path) -> pathlib.Path:
    """解包到缓存目录的同级目录并返回 bin 路径（已存在则复用）。"""
    root = cache_dir() / EXTRACT_DIR_NAME
    bin_dir = root / "bin"
    if all((bin_dir / name).is_file() for name in REQUIRED_BINARIES):
        print(f"复用已解包工具链 {root}")
        return bin_dir

    if root.exists():
        shutil.rmtree(root)
    print(f"解包到 {root}")
    safe_extract(archive, cache_dir())
    missing = [name for name in REQUIRED_BINARIES if not (bin_dir / name).is_file()]
    if missing:
        raise RuntimeError(f"解包后缺少驱动 {missing}：{root}")
    return bin_dir


def ensure_rustup_toolchain() -> None:
    """确认 rustup 工具链存在；缺失则安装（含 rustfmt/clippy，门禁依赖）。"""
    rustup = shutil.which("rustup") or shutil.which("rustup.exe")
    if rustup is None:
        raise RuntimeError("未找到 rustup：gnullvm 工具链需由 rustup 安装（ADR-073）")
    listed = subprocess.run(
        [rustup, "toolchain", "list"],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    if RUSTUP_TOOLCHAIN in listed.stdout:
        print(f"复用已有 rustup 工具链 {RUSTUP_TOOLCHAIN}")
        return
    print(f"安装 rustup 工具链 {RUSTUP_TOOLCHAIN}")
    install = subprocess.run(
        [
            rustup,
            "toolchain",
            "install",
            RUSTUP_TOOLCHAIN,
            "--profile",
            "minimal",
            "--component",
            "rustfmt",
            "--component",
            "clippy",
            "--no-self-update",
        ],
        check=False,
    )
    if install.returncode != 0:
        raise RuntimeError(f"rustup 安装 {RUSTUP_TOOLCHAIN} 失败（退出码 {install.returncode}）")


def append_env_file(name: str, value: str) -> bool:
    """把值写入 Actions 的环境文件；不在 Actions 中时返回 False。"""
    path = os.environ.get(name)
    if not path:
        return False
    with open(path, "a", encoding="utf-8", newline="\n") as handle:
        handle.write(value + "\n")
    return True


def main() -> int:
    parser = argparse.ArgumentParser(description="安装 Windows LLVM 系列构建工具链（ADR-073）")
    parser.add_argument(
        "--mirror",
        default=os.environ.get("WHCODE_GH_MIRROR"),
        help="GitHub 加速前缀（本机直连受限时使用，如 https://ghfast.top/）",
    )
    parser.add_argument(
        "--print-env",
        action="store_true",
        help="只打印需要导出的环境变量（本地 shell 用），不改 Actions 环境文件",
    )
    args = parser.parse_args()

    if platform.system() != "Windows":
        return fail(f"本脚本只为 Windows amd64 验收目标准备工具链，当前为 {platform.platform()}（ADR-013）")

    try:
        archive = ensure_archive(args.mirror)
        bin_dir = install_toolchain_dir(archive)
        ensure_rustup_toolchain()
    except RuntimeError as err:
        return fail(str(err))

    print(f"llvm-mingw bin: {bin_dir}")
    print(f"rustup 工具链: {RUSTUP_TOOLCHAIN}")

    if args.print_env:
        print("")
        print("# 复制到当前 shell（PowerShell）：")
        print(f'$env:Path = "{bin_dir};$env:Path"')
        print(f'$env:RUSTUP_TOOLCHAIN = "{RUSTUP_TOOLCHAIN}"')
        return 0

    wrote_path = append_env_file("GITHUB_PATH", str(bin_dir))
    wrote_env = append_env_file("GITHUB_ENV", f"RUSTUP_TOOLCHAIN={RUSTUP_TOOLCHAIN}")
    if not (wrote_path and wrote_env):
        print("")
        print("不在 GitHub Actions 中：本进程无法修改父 shell 环境，请用 --print-env 取导出命令。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
